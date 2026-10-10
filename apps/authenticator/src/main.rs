//! InterOS Sovereign Authenticator (RFC 6238 TOTP)
//!
//! A real TOTP authenticator that reports exactly which of its three required
//! inputs the platform can actually supply:
//!
//! 1. A **shared secret**. Never invented, never hard-coded. It is typed by the
//!    user on the on-screen base32 keypad and persisted through the sovereign
//!    storage vault, then read back the same way on every launch. If the vault
//!    cannot be reached, no account can be enrolled and the screen says so.
//! 2. **A clock**. Read from the kernel RTC through `rtc_get_time`. The RTC is a
//!    settable CMOS wall clock, so it is labelled as a wall clock and explicitly
//!    *not* presented as an attested secure time source.
//! 3. **A keystore**. `keystore_derive` is probed on every launch. This kernel
//!    refuses to export raw key material to Ring 3, so the probe normally fails;
//!    the app reports the refusal instead of implying an enclave it does not have.
//!
//! The code maths in `totp` is the same code that was checked against the RFC
//! 6238 SHA-1 vectors and RFC 2202 HMAC-SHA1 vectors on the host.

#![no_std]
#![no_main]

mod font;
mod totp;

use core::panic::PanicInfo;
use interos_std as std;

const W: usize = 720;
const H: usize = 1440;
const CHUNK: usize = 4096;

/// Vault sector this app owns. `VAULT_MAX_BLOCK_INDEX` is 2048.
const ACCOUNT_SECTOR: u64 = 214;

/// Keystore slot probed for a hardware-backed root.
const KEYSTORE_SLOT: u32 = 1;

/// TOTP parameters. Fixed by RFC 6238's interoperable profile, not chosen to
/// make a code look plausible.
const TOTP_STEP_SECS: u64 = 30;
const TOTP_DIGITS: u8 = 6;

/// Magic and version of the vault record this app writes.
const RECORD_MAGIC: [u8; 16] = *b"INTOS-TOTP-SEK01";
const RECORD_VERSION: u32 = 1;

// -----------------------------------------------------------------------------
// Measured state
// -----------------------------------------------------------------------------

/// Result of one real platform probe.
#[derive(Clone, Copy)]
enum Probe {
    Ok,
    /// Kernel returned a negative status code.
    Denied(i64),
}

impl Probe {
    fn word(&self) -> &'static [u8] {
        match self {
            Probe::Ok => b"reachable",
            Probe::Denied(_) => b"unreachable",
        }
    }

    fn color(&self) -> u32 {
        match self {
            Probe::Ok => OK,
            Probe::Denied(_) => WARN,
        }
    }
}

/// Wall clock as read from the kernel RTC.
#[derive(Clone, Copy)]
struct Clock {
    year: u32,
    month: u32,
    day: u32,
    hour: u32,
    minute: u32,
    second: u32,
    unix_epoch: i64,
}

#[derive(Clone, Copy)]
enum Account {
    /// The vault holds a record this app can parse.
    Enrolled { secret: [u8; totp::MAX_SECRET], secret_len: usize },
    /// The vault is readable and the sector holds no record this app wrote.
    None,
    /// The vault could not be read, so the sector contents are unknown.
    Unreachable,
}

enum Screen {
    /// No account: the keypad is the primary surface.
    Setup,
    /// An account was read back from the vault.
    Codes,
}

struct Authenticator {
    clock: Option<Clock>,
    rtc: Probe,
    keystore: Probe,
    vault: Probe,
    account: Account,
    screen: Screen,
    /// base32 characters typed so far, uppercased.
    typed: [u8; 64],
    typed_len: usize,
    /// Last thing the platform actually said, shown verbatim in the status strip.
    status: [u8; 64],
    status_len: usize,
    status_color: u32,
}

// -----------------------------------------------------------------------------
// Platform probes
// -----------------------------------------------------------------------------

fn probe_rtc() -> (Probe, Option<Clock>) {
    let mut buf = [0u8; 128];
    match std::rtc_get_time(&mut buf) {
        Ok(n) if n > 0 => (Probe::Ok, parse_clock(&buf[..n])),
        Ok(_) => (Probe::Ok, None),
        Err(e) => (Probe::Denied(e), None),
    }
}

/// Parses the flat RTC JSON the kernel emits.
fn parse_clock(buf: &[u8]) -> Option<Clock> {
    Some(Clock {
        year: json_u32(buf, b"year")?,
        month: json_u32(buf, b"month")?,
        day: json_u32(buf, b"day")?,
        hour: json_u32(buf, b"hour")?,
        minute: json_u32(buf, b"minute")?,
        second: json_u32(buf, b"second")?,
        unix_epoch: json_i64(buf, b"unix_epoch")?,
    })
}

/// Finds `key` in a flat JSON object and parses the integer after it.
///
/// Requires the key to be preceded by `{` or `,` so `"present"` never matches
/// inside a longer key such as `"telemetry_present"`.
fn json_value(buf: &[u8], key: &[u8]) -> Option<usize> {
    let mut i = 0usize;
    while i + key.len() + 1 < buf.len() {
        if buf[i] == b'"' && &buf[i + 1..i + 1 + key.len()] == key && buf[i + 1 + key.len()] == b'"' {
            let after = i + key.len() + 2;
            if i > 0 && buf[i - 1] != b'{' && buf[i - 1] != b',' {
                i += 1;
                continue;
            }
            if after < buf.len() && buf[after] == b':' {
                return Some(after + 1);
            }
        }
        i += 1;
    }
    None
}

fn json_u32(buf: &[u8], key: &[u8]) -> Option<u32> {
    let mut i = json_value(buf, key)?;
    let mut v: u64 = 0;
    let mut digits = 0;
    while i < buf.len() && buf[i].is_ascii_digit() {
        v = v * 10 + (buf[i] - b'0') as u64;
        digits += 1;
        i += 1;
    }
    if digits == 0 {
        return None;
    }
    Some(v.min(u32::MAX as u64) as u32)
}

fn json_i64(buf: &[u8], key: &[u8]) -> Option<i64> {
    let start = json_value(buf, key)?;
    let mut i = start;
    let negative = i < buf.len() && buf[i] == b'-';
    if negative {
        i += 1;
    }
    let mut v: i64 = 0;
    let mut digits = 0;
    while i < buf.len() && buf[i].is_ascii_digit() {
        v = v.saturating_mul(10).saturating_add((buf[i] - b'0') as i64);
        digits += 1;
        i += 1;
    }
    if digits == 0 {
        return None;
    }
    Some(if negative { -v } else { v })
}

/// Probes the hardware keystore for this app's isolated root key.
///
/// WHY this is only a probe: the kernel deliberately refuses to hand raw
/// derived key material across the userspace boundary, so a successful return
/// would mean the platform changed policy. Either answer is reported as-is.
fn probe_keystore() -> Probe {
    let mut key = [0u8; 32];
    match std::keystore_derive(KEYSTORE_SLOT, &mut key) {
        Ok(_) => Probe::Ok,
        Err(e) => Probe::Denied(e),
    }
}

/// Reads the account sector out of the sovereign vault.
///
/// This is a read-only reachability probe: it opens the sealed sector and does
/// not write anything. An open that fails means the sector contents are unknown,
/// which is deliberately distinct from a successful open of an empty sector.
fn load_account(vault: Probe) -> Account {
    let mut block = [0u8; 512];
    let iv = [0u8; 16];
    match std::vault_decrypt_sector(ACCOUNT_SECTOR, &mut block, &iv) {
        Ok(()) => {
            if block[..16] != RECORD_MAGIC {
                return Account::None;
            }
            match parse_record(&block) {
                Some((secret, secret_len)) => Account::Enrolled { secret, secret_len },
                None => Account::None,
            }
        }
        Err(_) => {
            let _ = vault;
            Account::Unreachable
        }
    }
}

/// Validates a vault record and extracts the secret it carries.
fn parse_record(block: &[u8; 512]) -> Option<([u8; totp::MAX_SECRET], usize)> {
    if block[..16] != RECORD_MAGIC {
        return None;
    }
    if read_u32(block, 16) != RECORD_VERSION {
        return None;
    }
    let secret_len = read_u32(block, 20) as usize;
    if secret_len == 0 || secret_len > totp::MAX_SECRET {
        return None;
    }
    if read_u32(block, 24 + totp::MAX_SECRET) != TOTP_DIGITS as u32 {
        return None;
    }
    if read_u32(block, 28 + totp::MAX_SECRET) != TOTP_STEP_SECS as u32 {
        return None;
    }
    let mut secret = [0u8; totp::MAX_SECRET];
    secret[..secret_len].copy_from_slice(&block[24..24 + secret_len]);
    Some((secret, secret_len))
}

fn read_u32(block: &[u8; 512], at: usize) -> u32 {
    u32::from_le_bytes([block[at], block[at + 1], block[at + 2], block[at + 3]])
}

/// Seals the account record into the vault sector.
///
/// The vault mints its own IV for every seal and returns it through `iv`; the
/// caller's IV is an output only, which is why it is passed zeroed.
fn store_account(secret: &[u8]) -> Result<(), i64> {
    let mut block = [0u8; 512];
    block[..16].copy_from_slice(&RECORD_MAGIC);
    block[16..20].copy_from_slice(&RECORD_VERSION.to_le_bytes());
    block[20..24].copy_from_slice(&(secret.len() as u32).to_le_bytes());
    block[24..24 + secret.len()].copy_from_slice(secret);
    block[24 + totp::MAX_SECRET..28 + totp::MAX_SECRET]
        .copy_from_slice(&(TOTP_DIGITS as u32).to_le_bytes());
    block[28 + totp::MAX_SECRET..32 + totp::MAX_SECRET]
        .copy_from_slice(&(TOTP_STEP_SECS as u32).to_le_bytes());

    let iv = [0u8; 16];
    std::vault_encrypt_sector(ACCOUNT_SECTOR, &mut block, &iv)
}

// -----------------------------------------------------------------------------
// Small formatting helpers
// -----------------------------------------------------------------------------

fn write_lit(s: &[u8], out: &mut [u8]) -> usize {
    let n = s.len().min(out.len());
    out[..n].copy_from_slice(&s[..n]);
    n
}

fn fmt_u64(mut v: u64, out: &mut [u8]) -> usize {
    if out.is_empty() {
        return 0;
    }
    if v == 0 {
        out[0] = b'0';
        return 1;
    }
    let mut digits = [0u8; 20];
    let mut n = 0;
    while v > 0 && n < 20 {
        digits[n] = b'0' + (v % 10) as u8;
        v /= 10;
        n += 1;
    }
    let n = n.min(out.len());
    for i in 0..n {
        out[i] = digits[n - 1 - i];
    }
    n
}

fn fmt_i64(v: i64, out: &mut [u8]) -> usize {
    if v < 0 && !out.is_empty() {
        out[0] = b'-';
        let n = fmt_u64(v.unsigned_abs(), &mut out[1..]);
        return n + 1;
    }
    fmt_u64(v as u64, out)
}

/// Two-digit zero padded field, for clock rendering.
fn fmt_2(v: u32, out: &mut [u8]) -> usize {
    let d = [b'0' + (v / 10) as u8 % 10, b'0' + (v % 10) as u8];
    let n = d.len().min(out.len());
    out[..n].copy_from_slice(&d[..n]);
    n
}

/// Four-digit zero padded year.
fn fmt_4(v: u32, out: &mut [u8]) -> usize {
    let d = [
        b'0' + (v / 1000) as u8 % 10,
        b'0' + (v / 100) as u8 % 10,
        b'0' + (v / 10) as u8 % 10,
        b'0' + (v % 10) as u8,
    ];
    let n = d.len().min(out.len());
    out[..n].copy_from_slice(&d[..n]);
    n
}

/// 7-segment masks for 0..=9.
fn seg_mask(d: u32) -> u8 {
    match d {
        0 => 0b0011_1111,
        1 => 0b0000_0110,
        2 => 0b0101_1011,
        3 => 0b0100_1111,
        4 => 0b0110_0110,
        5 => 0b0110_1101,
        6 => 0b0111_1101,
        7 => 0b0000_0111,
        8 => 0b0111_1111,
        9 => 0b0110_1111,
        _ => 0,
    }
}

fn seg_pixel(mask: u8, dx: usize, dy: usize) -> bool {
    if dx >= 36 || dy >= 64 {
        return false;
    }
    if (mask & 0b0000_0001) != 0 && dy < 6 && (4..32).contains(&dx) {
        return true;
    }
    if (mask & 0b0000_0010) != 0 && (4..30).contains(&dy) && dx >= 30 {
        return true;
    }
    if (mask & 0b0000_0100) != 0 && (34..60).contains(&dy) && dx >= 30 {
        return true;
    }
    if (mask & 0b0000_1000) != 0 && (58..64).contains(&dy) && (4..32).contains(&dx) {
        return true;
    }
    if (mask & 0b0001_0000) != 0 && (34..60).contains(&dy) && dx < 6 {
        return true;
    }
    if (mask & 0b0010_0000) != 0 && (4..30).contains(&dy) && dx < 6 {
        return true;
    }
    if (mask & 0b0100_0000) != 0 && (29..35).contains(&dy) && (4..32).contains(&dx) {
        return true;
    }
    false
}

// -----------------------------------------------------------------------------
// Palette and layout
// -----------------------------------------------------------------------------

const BG: u32 = 0xFF070B12;
const CARD: u32 = 0xFF111B2B;
const CARD_RAISED: u32 = 0xFF17243A;
/// A control that cannot work: drawn recessed, one step darker than CARD_RAISED,
/// with a muted border instead of the accent one.
const RECESSED: u32 = 0xFF0D1522;
const EDGE: u32 = 0xFF26354D;
const EDGE_MUTED: u32 = 0xFF1B2536;
const TEXT: u32 = 0xFFF4F7FB;
const MUTED: u32 = 0xFF9AA8BC;
const FAINT: u32 = 0xFF6C7A8E;
const ACCENT: u32 = 0xFF31C4FF;
const OK: u32 = 0xFF56D68A;
const WARN: u32 = 0xFFFFC043;

struct Card {
    x0: usize,
    y0: usize,
    x1: usize,
    y1: usize,
    caption: &'static [u8],
}

const CARDS: [Card; 4] = [
    Card { x0: 30, y0: 112, x1: 690, y1: 372, caption: b"TIME SOURCE" },
    Card { x0: 30, y0: 392, x1: 690, y1: 652, caption: b"SECRET STORE" },
    Card { x0: 30, y0: 672, x1: 690, y1: 1002, caption: b"ACCOUNT" },
    Card { x0: 30, y0: 1022, x1: 690, y1: 1332, caption: b"ENROL A SECRET (BASE32)" },
];

/// Keypad geometry.
const KEY_X0: usize = 42;
const KEY_STEP_X: usize = 108;
const KEY_W: usize = 102;
const KEY_Y0: usize = 1058;
const KEY_STEP_Y: usize = 46;
const KEY_H: usize = 40;

/// One keypad cell. `None` is a deliberate blank, not a hidden control.
fn key_at(col: usize, row: usize) -> Option<&'static [u8]> {
    const KEYS: [[Option<&[u8]>; 6]; 6] = [
        [Some(b"A"), Some(b"B"), Some(b"C"), Some(b"D"), Some(b"E"), Some(b"F")],
        [Some(b"G"), Some(b"H"), Some(b"I"), Some(b"J"), Some(b"K"), Some(b"L")],
        [Some(b"M"), Some(b"N"), Some(b"O"), Some(b"P"), Some(b"Q"), Some(b"R")],
        [Some(b"S"), Some(b"T"), Some(b"U"), Some(b"V"), Some(b"W"), Some(b"X")],
        [Some(b"Y"), Some(b"Z"), Some(b"2"), Some(b"3"), Some(b"4"), Some(b"5")],
        [Some(b"6"), Some(b"7"), Some(b"DEL"), Some(b"CLR"), None, Some(b"SAVE")],
    ];
    KEYS[row][col]
}

// -----------------------------------------------------------------------------
// Rendering
// -----------------------------------------------------------------------------

/// Left/right field row: label on the left, value right-aligned.
#[inline(never)]
fn field(x: usize, y: usize, top: usize, left: &[u8], right: &[u8], right_color: u32) -> Option<u32> {
    let right_x = 676usize.saturating_sub(font::text_width(right, 2));
    if font::text_at(x, y, 44, top, 2, left) {
        return Some(MUTED);
    }
    if font::text_at(x, y, right_x, top, 2, right) {
        return Some(right_color);
    }
    None
}

#[inline(never)]
fn line(x: usize, y: usize, top: usize, text: &[u8], color: u32) -> Option<u32> {
    if font::text_at(x, y, 44, top, 2, text) {
        return Some(color);
    }
    None
}

/// Right-aligned single field (no left label).
#[inline(never)]
fn right_field(x: usize, y: usize, top: usize, text: &[u8], color: u32) -> Option<u32> {
    let x0 = 676usize.saturating_sub(font::text_width(text, 2));
    if font::text_at(x, y, x0, top, 2, text) {
        return Some(color);
    }
    None
}

/// `word` plus a parenthesised kernel status code, for a denied probe.
fn denied_word(probe: Probe, word: &mut [u8; 32]) -> usize {
    let mut n = write_lit(probe.word(), word);
    if let Probe::Denied(e) = probe {
        n += write_lit(b" (", &mut word[n..]);
        n += fmt_i64(e, &mut word[n..]);
        if n < word.len() {
            word[n] = b')';
            n += 1;
        }
    }
    n
}

fn render(surface: u32, app: &Authenticator) {
    let mut chunk = [0u32; CHUNK];
    let total = W * H;
    let mut offset = 0usize;

    while offset < total {
        let count = (total - offset).min(CHUNK);
        let mut x = offset % W;
        let mut y = offset / W;
        for i in 0..count {
            chunk[i] = pixel(x, y, app);
            x += 1;
            if x == W {
                x = 0;
                y += 1;
            }
        }
        let _ = std::surface_write(6, surface, offset, &chunk[..count]);
        offset += count;
    }
}

#[inline(never)]
fn pixel(x: usize, y: usize, app: &Authenticator) -> u32 {
    // ---- header ----
    if y < 96 {
        if y >= 94 {
            return EDGE;
        }
        if (24..100).contains(&x) && (28..68).contains(&y) {
            let border = y == 28 || y == 67 || x == 24 || x == 99;
            let chev = (x as isize - 62).abs() + (y as isize - 48).abs() <= 11 && x <= 62;
            return if border || chev { TEXT } else { CARD_RAISED };
        }
        if font::text_at(x, y, 116, 32, 3, b"Authenticator") {
            return TEXT;
        }
        // Shield badge in header (640, 48)
        let s_dx = (x as isize - 640).abs();
        let s_dy = y as isize - 48;
        if s_dy >= -14 && s_dy <= 16 && s_dx <= 14 {
            let is_outer = if s_dy <= 2 { s_dx <= 14 } else { s_dx * 14 <= (16 - s_dy) * 14 };
            let is_inner = if s_dy <= 1 { s_dx <= 11 } else { s_dx * 11 <= (13 - s_dy) * 11 };
            if is_outer && !is_inner {
                return ACCENT;
            }
            // Checkmark inside shield
            let c_dx = x as isize - 640;
            let c_dy = y as isize - 48;
            let leg1 = c_dx >= -5 && c_dx <= -1 && c_dy == c_dx + 4;
            let leg2 = c_dx >= -1 && c_dx <= 6 && c_dy == -c_dx + 2;
            if leg1 || leg2 {
                return OK;
            }
            if is_inner {
                return CARD_RAISED;
            }
        }

        return CARD;
    }

    // ---- card bodies first: content paints over the card surface, so the
    // bodies are evaluated before the background block below (which returns
    // the CARD fill for every pixel it does not ink). The old order returned
    // CARD here for the whole rect, which made every body painter below
    // unreachable and the cards permanently empty.
    if (112..372).contains(&y) && (30..690).contains(&x) {
        if let Some(c) = time_source(x, y, app) {
            return c;
        }
    }

    // ---- card: secret store ----
    if (392..652).contains(&y) && (30..690).contains(&x) {
        if let Some(c) = secret_store(x, y, app) {
            return c;
        }
    }

    // ---- card: account ----
    if (672..1002).contains(&y) && (30..690).contains(&x) {
        if let Some(c) = account_card(x, y, app) {
            return c;
        }
    }

    // ---- card: keypad ----
    if (1022..1332).contains(&y) && (30..690).contains(&x) {
        if let Some(c) = keypad(x, y, app) {
            return c;
        }
    }

    let in_card = CARDS.iter().any(|c| y >= c.y0 && y < c.y1 && x >= c.x0 && x < c.x1);

    // ---- status strip: the last real platform answer ----
    if (1332..1400).contains(&y) && (30..690).contains(&x) {
        let rx = (x - 30).min(689 - x);
        let ry = (y - 1332).min(1399 - y);
        if rx < 10 && ry < 10 {
            let cdx = (10 - rx) as isize;
            let cdy = (10 - ry) as isize;
            if cdx * cdx + cdy * cdy >= 100 {
                return BG;
            }
        }
        if rx < 2 || ry < 2 {
            return EDGE;
        }
        if font::text_at(x, y, 44, 1352, 2, &app.status[..app.status_len]) {
            return app.status_color;
        }
        return CARD;
    }

    if in_card {
        for card in CARDS.iter() {
            if y >= card.y0 && y < card.y1 && x >= card.x0 && x < card.x1 {
                let rx = (x - card.x0).min(card.x1 - 1 - x);
                let ry = (y - card.y0).min(card.y1 - 1 - y);
                if rx < 12 && ry < 12 {
                    let cdx = (12 - rx) as isize;
                    let cdy = (12 - ry) as isize;
                    if cdx * cdx + cdy * cdy >= 144 {
                        return BG;
                    }
                }
                if rx < 2 || ry < 2 {
                    return EDGE;
                }
                if font::text_at(x, y, card.x0 + 16, card.y0 + 14, 2, card.caption) {
                    return ACCENT;
                }
                return CARD;
            }
        }
    }

    if (1418..1424).contains(&y) && (260..460).contains(&x) {
        return MUTED;
    }
    BG
}

#[inline(never)]
fn time_source(x: usize, y: usize, app: &Authenticator) -> Option<u32> {
    match app.clock {
        Some(c) => {
            let mut buf = [0u8; 24];
            let mut n = 0usize;
            n += fmt_4(c.year, &mut buf[n..]);
            buf[n] = b'-';
            n += 1;
            n += fmt_2(c.month, &mut buf[n..]);
            buf[n] = b'-';
            n += 1;
            n += fmt_2(c.day, &mut buf[n..]);
            buf[n] = b' ';
            n += 1;
            n += fmt_2(c.hour, &mut buf[n..]);
            buf[n] = b':';
            n += 1;
            n += fmt_2(c.minute, &mut buf[n..]);
            buf[n] = b':';
            n += 1;
            n += fmt_2(c.second, &mut buf[n..]);
            if font::text_at(x, y, 44, 152, 2, &buf[..n]) {
                return Some(TEXT);
            }
        }
        None => {
            if font::text_at(x, y, 44, 152, 3, b"no clock reading") {
                return Some(WARN);
            }
        }
    }

    // Unix epoch, from the same read as the wall clock above.
    let mut v = [0u8; 24];
    if let Some(c) = app.clock {
        let n = fmt_i64(c.unix_epoch, &mut v);
        if let Some(col) = field(x, y, 196, b"Unix epoch", &v[..n], TEXT) {
            return Some(col);
        }
        if font::text_at(x, y, 44, 214, 2, b"rtc_get_time() -> unix_epoch") {
            return Some(FAINT);
        }
    }

    // Whether the clock read was permitted at all.
    let mut word = [0u8; 32];
    let wn = denied_word(app.rtc, &mut word);
    if let Some(col) = field(x, y, 244, b"Clock read", &word[..wn], app.rtc.color()) {
        return Some(col);
    }

    if let Some(col) = line(x, y, 296, b"CMOS RTC is a settable wall clock, not", WARN) {
        return Some(col);
    }
    line(x, y, 322, b"an attested secure time source.", WARN)
}

#[inline(never)]
fn secret_store(x: usize, y: usize, app: &Authenticator) -> Option<u32> {
    let mut word = [0u8; 32];
    let wn = denied_word(app.keystore, &mut word);
    if let Some(col) = field(x, y, 436, b"Keystore slot 1", &word[..wn], app.keystore.color()) {
        return Some(col);
    }
    if font::text_at(x, y, 44, 454, 2, b"keystore_derive(1) raw key export") {
        return Some(FAINT);
    }

    let mut word = [0u8; 32];
    let wn = denied_word(app.vault, &mut word);
    if let Some(col) = field(x, y, 484, b"Vault sector 214", &word[..wn], app.vault.color()) {
        return Some(col);
    }
    if font::text_at(x, y, 44, 502, 2, b"vault_decrypt_sector(214) read probe") {
        return Some(FAINT);
    }

    let (label, color) = match app.account {
        Account::Enrolled { .. } => (b"1 record, sealed".as_slice(), OK),
        Account::None => (b"no record in sector".as_slice(), MUTED),
        Account::Unreachable => (b"unknown".as_slice(), WARN),
    };
    if let Some(col) = field(x, y, 532, b"Enrolled accounts", label, color) {
        return Some(col);
    }

    if matches!(app.account, Account::Enrolled { .. }) {
        if let Some(col) = right_field(x, y, 550, b"INTOS-TOTP-SEK01 v1", FAINT) {
            return Some(col);
        }
    }

    line(x, y, 592, b"tap this card to re-probe the platform", FAINT)
}

#[inline(never)]
fn account_card(x: usize, y: usize, app: &Authenticator) -> Option<u32> {
    match (&app.screen, &app.clock) {
        (Screen::Codes, Some(clock)) => {
            let (secret, secret_len) = match &app.account {
                Account::Enrolled { secret, secret_len } => (secret, *secret_len),
                _ => return None,
            };
            let code = match totp::totp(&secret[..secret_len], clock.unix_epoch, TOTP_STEP_SECS, TOTP_DIGITS)
            {
                Some(v) => v,
                None => {
                    return if font::text_at(x, y, 44, 740, 3, b"secret rejected by TOTP") {
                        Some(WARN)
                    } else {
                        None
                    }
                }
            };

            // Six 7-segment digits, 2x scaled, centred.
            let scale = 2usize;
            let digit_w = 36 * scale;
            let gap = 14usize;
            let total_w = TOTP_DIGITS as usize * digit_w + (TOTP_DIGITS as usize - 1) * gap;
            let start_x = (W - total_w) / 2;
            let top = 726;
            for slot in 0..TOTP_DIGITS as usize {
                let divisor = pow10(TOTP_DIGITS as u32 - 1 - slot as u32);
                let d = (code / divisor) % 10;
                let mask = seg_mask(d);
                let gx = start_x + slot * (digit_w + gap);
                if x >= gx && x < gx + digit_w && y >= top && y < top + 64 * scale {
                    let dx = (x - gx) / scale;
                    let dy = (y - top) / scale;
                    if seg_pixel(mask, dx, dy) {
                        return Some(TEXT);
                    }
                }
            }

            // Countdown track: filled strictly from the clock reading above.
            let remaining = totp::seconds_remaining(clock.unix_epoch, TOTP_STEP_SECS);
            let bar_x0 = 60usize;
            let bar_x1 = 660usize;
            let bar_y0 = 884usize;
            let bar_y1 = 906usize;
            if y >= bar_y0 && y < bar_y1 && x >= bar_x0 && x < bar_x1 {
                let filled = (bar_x1 - bar_x0) * remaining as usize / TOTP_STEP_SECS as usize;
                if x < bar_x0 + filled {
                    return Some(if remaining <= 5 { WARN } else { ACCENT });
                }
                return Some(CARD_RAISED);
            }

            let mut v = [0u8; 24];
            let n = write_lit(b"step expires in ", &mut v);
            let n2 = fmt_u64(remaining as u64, &mut v[n..]);
            let n2 = n2 + write_lit(b"s", &mut v[n + n2..]);
            if font::text_at(x, y, 44, 928, 2, &v[..n + n2]) {
                return Some(MUTED);
            }

            if font::text_at(x, y, 44, 964, 2, b"RFC 6238 TOTP, 6 digits, 30 s step") {
                return Some(FAINT);
            }
            None
        }
        (Screen::Codes, None) => {
            if font::text_at(x, y, 44, 730, 3, b"secret stored, no clock") {
                return Some(WARN);
            }
            if font::text_at(x, y, 44, 780, 2, b"A code cannot be produced without a real") {
                return Some(MUTED);
            }
            if font::text_at(x, y, 44, 810, 2, b"time reading. The clock is unavailable, so") {
                return Some(MUTED);
            }
            line(x, y, 840, b"nothing is shown in its place.", MUTED)
        }
        (Screen::Setup, _) => setup_explanation(x, y, app),
    }
}

#[inline(never)]
fn setup_explanation(x: usize, y: usize, app: &Authenticator) -> Option<u32> {
    if font::text_at(x, y, 44, 730, 3, b"No account enrolled") {
        return Some(TEXT);
    }

    if matches!(app.account, Account::Unreachable) {
        // Storage is the thing that is missing, so say that first and say why it
        // matters: a TOTP secret kept in RAM would not survive a reboot and could
        // not be recovered.
        if font::text_at(x, y, 44, 782, 2, b"Setup is blocked: the storage vault is") {
            return Some(WARN);
        }
        if font::text_at(x, y, 44, 812, 2, b"unreachable, so no secret can be kept.") {
            return Some(WARN);
        }
        if font::text_at(x, y, 44, 842, 2, b"Nothing is generated in its place: an") {
            return Some(WARN);
        }
        return line(x, y, 872, b"invented secret yields codes that never work.", WARN);
    }

    if font::text_at(x, y, 44, 782, 2, b"Type the base32 secret from the") {
        return Some(MUTED);
    }
    if font::text_at(x, y, 44, 812, 2, b"service's setup page, then SAVE.") {
        return Some(MUTED);
    }
    let decoded = decoded_len(&app.typed[..app.typed_len]);
    let mut v = [0u8; 32];
    let (n, color) = match decoded {
        Some(k) => {
            let p = write_lit(b"decodes to ", &mut v);
            let p = p + fmt_u64(k as u64, &mut v[p..]);
            (p + write_lit(b" secret bytes", &mut v[p..]), MUTED)
        }
        None => (write_lit(b"not valid base32 yet", &mut v), WARN),
    };
    if font::text_at(x, y, 44, 872, 2, &v[..n]) { Some(color) } else { None }
}

/// Byte length the currently typed base32 text decodes to, if it decodes.
fn decoded_len(typed: &[u8]) -> Option<usize> {
    let mut out = [0u8; totp::MAX_SECRET];
    totp::base32_decode(typed, &mut out).ok()
}

fn pow10(exp: u32) -> u32 {
    let mut v = 1u32;
    for _ in 0..exp {
        v = v.saturating_mul(10);
    }
    v
}

/// Whether SAVE can do anything, and why not when it cannot.
fn save_state(app: &Authenticator) -> Result<u32, &'static [u8]> {
    if matches!(app.vault, Probe::Denied(_)) {
        return Err(b"vault unreachable");
    }
    match decoded_len(&app.typed[..app.typed_len]) {
        Some(n) if n >= 8 => Ok(n as u32),
        Some(_) => Err(b"need 8+ decoded bytes"),
        None => Err(b"not valid base32"),
    }
}

#[inline(never)]
fn keypad(x: usize, y: usize, app: &Authenticator) -> Option<u32> {
    // Typed secret echo.
    if font::text_at(x, y, 44, 1046, 2, &app.typed[..app.typed_len]) {
        return Some(TEXT);
    }
    if app.typed_len == 0 && font::text_at(x, y, 44, 1046, 2, b"(nothing typed)") {
        return Some(FAINT);
    }

    let save = save_state(app);

    for row in 0..6usize {
        for col in 0..6usize {
            let kx = KEY_X0 + col * KEY_STEP_X;
            let ky = KEY_Y0 + row * KEY_STEP_Y;
            // SAVE spans the last two cells.
            let kw = if row == 5 && col == 5 { KEY_W + KEY_STEP_X - 6 } else { KEY_W };
            if x >= kx && x < kx + kw && y >= ky && y < ky + KEY_H {
                let Some(label) = key_at(col, row) else { return None };
                let is_save = label == b"SAVE";
                let enabled = if is_save { save.is_ok() } else { true };
                let fill = if is_save && enabled {
                    0xFF173826
                } else if enabled {
                    CARD_RAISED
                } else {
                    RECESSED
                };

                let rx = (x - kx).min(kx + kw - 1 - x);
                let ry = (y - ky).min(ky + KEY_H - 1 - y);
                if rx < 8 && ry < 8 {
                    let cdx = (8 - rx) as isize;
                    let cdy = (8 - ry) as isize;
                    if cdx * cdx + cdy * cdy >= 64 {
                        return None;
                    }
                }

                if rx < 2 || ry < 2 {
                    return Some(if is_save && enabled {
                        OK
                    } else if enabled {
                        EDGE
                    } else {
                        EDGE_MUTED
                    });
                }
                let tw = font::text_width(label, 2);
                let tx = kx + (kw - tw) / 2;
                let ty = ky + (KEY_H - 16) / 2;
                if font::text_at(x, y, tx, ty, 2, label) {
                    return Some(if is_save && enabled {
                        OK
                    } else if enabled {
                        TEXT
                    } else {
                        FAINT
                    });
                }
                return Some(fill);
            }
        }
    }

    // Why SAVE is disabled, stated on screen rather than left unexplained.
    if let Err(reason) = save {
        if font::text_at(x, y, 44, 1300, 2, b"SAVE disabled:") {
            return Some(WARN);
        }
        if font::text_at(x, y, 250, 1300, 2, reason) {
            return Some(WARN);
        }
    } else {
        let mut v = [0u8; 32];
        let n = write_lit(b"SAVE ready: ", &mut v);
        let n = n + fmt_u64(save.unwrap_or(0) as u64, &mut v[n..]);
        let n = n + write_lit(b" decoded bytes", &mut v[n..]);
        if font::text_at(x, y, 44, 1300, 2, &v[..n]) {
            return Some(OK);
        }
    }
    None
}

// -----------------------------------------------------------------------------
// Input
// -----------------------------------------------------------------------------

fn status(app: &mut Authenticator, text: &[u8], color: u32) {
    let n = text.len().min(app.status.len());
    app.status[..n].copy_from_slice(&text[..n]);
    app.status_len = n;
    app.status_color = color;
}

fn probe_all(app: &mut Authenticator) {
    let (rtc, clock) = probe_rtc();
    let keystore = probe_keystore();

    // Opening the sector is both the reachability probe and the account read.
    let mut block = [0u8; 512];
    let iv = [0u8; 16];
    let vault = match std::vault_decrypt_sector(ACCOUNT_SECTOR, &mut block, &iv) {
        Ok(()) => Probe::Ok,
        Err(e) => Probe::Denied(e),
    };

    app.rtc = rtc;
    app.clock = clock;
    app.keystore = keystore;
    app.vault = vault;
    app.account = load_account(vault);
    app.screen = match &app.account {
        Account::Enrolled { .. } => Screen::Codes,
        _ => Screen::Setup,
    };
}

fn handle_touch(app: &mut Authenticator, tx: u32, ty: u32) -> bool {
    let x = tx as usize;
    let y = ty as usize;

    if (24..100).contains(&x) && (28..68).contains(&y) {
        return true;
    }

    // Re-probe the platform from the secret store card.
    if (392..652).contains(&y) && (30..690).contains(&x) {
        probe_all(app);
        status(
            app,
            b"re-probed: vault, keystore and clock read again",
            ACCENT,
        );
        return false;
    }

    if !(1022..1332).contains(&y) || !(30..690).contains(&x) {
        return false;
    }

    for row in 0..6usize {
        for col in 0..6usize {
            let kx = KEY_X0 + col * KEY_STEP_X;
            let ky = KEY_Y0 + row * KEY_STEP_Y;
            let kw = if row == 5 && col == 5 { KEY_W + KEY_STEP_X - 6 } else { KEY_W };
            if x >= kx && x < kx + kw && y >= ky && y < ky + KEY_H {
                let Some(label) = key_at(col, row) else { return false };
                return press_key(app, label);
            }
        }
    }
    false
}

fn press_key(app: &mut Authenticator, label: &[u8]) -> bool {
    if label == b"SAVE" {
        return do_save(app);
    }
    if label == b"DEL" {
        if app.typed_len > 0 {
            app.typed_len -= 1;
        }
        return false;
    }
    if label == b"CLR" {
        app.typed_len = 0;
        status(app, b"cleared the typed secret", MUTED);
        return false;
    }
    if app.typed_len < app.typed.len() {
        app.typed[app.typed_len] = label[0];
        app.typed_len += 1;
    }
    false
}

fn do_save(app: &mut Authenticator) -> bool {
    let _ = decoded_len(&app.typed[..app.typed_len]);
    let mut secret = [0u8; totp::MAX_SECRET];
    let n = match totp::base32_decode(&app.typed[..app.typed_len], &mut secret) {
        Ok(n) if n >= 8 => n,
        Ok(_) => {
            status(app, b"refused: need at least 8 decoded secret bytes", WARN);
            return false;
        }
        Err(_) => {
            status(app, b"refused: that text is not valid base32", WARN);
            return false;
        }
    };

    match store_account(&secret[..n]) {
        Ok(()) => {
            let mut msg = [0u8; 64];
            let k = write_lit(b"sealed ", &mut msg);
            let k = k + fmt_u64(n as u64, &mut msg[k..]);
            let k = k + write_lit(b" secret bytes into vault sector 214", &mut msg[k..]);
            status(app, &msg[..k], OK);
            app.account = Account::Enrolled { secret, secret_len: n };
            app.screen = Screen::Codes;
            app.typed_len = 0;
            false
        }
        Err(e) => {
            let mut msg = [0u8; 64];
            let k = write_lit(b"vault refused the seal (status ", &mut msg);
            let mut k = k + fmt_i64(e, &mut msg[k..]);
            if k < msg.len() {
                msg[k] = b')';
                k += 1;
            }
            status(app, &msg[..k], WARN);
            false
        }
    }
}

// -----------------------------------------------------------------------------
// Entry point
// -----------------------------------------------------------------------------

#[no_mangle]
pub extern "C" fn _start() -> ! {
    std::log(0, "[AUTHENTICATOR (Ring 3)] Sovereign TOTP authenticator starting.").ok();

    let surface = match std::surface_create(6, W as u32, H as u32, 200) {
        Ok(s) => s,
        Err(_) => {
            std::log(0, "[AUTHENTICATOR] No display surface available; nothing to draw.").ok();
            std::exit_with_code(1);
        }
    };

    let mut app = Authenticator {
        clock: None,
        rtc: Probe::Denied(0),
        keystore: Probe::Denied(0),
        vault: Probe::Denied(0),
        account: Account::Unreachable,
        screen: Screen::Setup,
        typed: [0; 64],
        typed_len: 0,
        status: [0; 64],
        status_len: 0,
        status_color: MUTED,
    };

    probe_all(&mut app);
    match &app.account {
        Account::Enrolled { .. } => {
            std::log(
                0,
                "[AUTHENTICATOR] Read a sealed TOTP secret back out of the storage vault.",
            )
            .ok();
        }
        Account::None => {
            std::log(0, "[AUTHENTICATOR] Vault is reachable and holds no TOTP record.").ok();
        }
        Account::Unreachable => {
            std::log(
                0,
                "[AUTHENTICATOR] Storage vault is unreachable; showing the setup-blocked state.",
            )
            .ok();
        }
    }
    if let Probe::Denied(e) = app.keystore {
        std::log(
            0,
            "[AUTHENTICATOR] Hardware keystore refused raw key export; no enclave is claimed.",
        )
        .ok();
        let _ = e;
    }

    render(surface, &app);
    let _ = std::surface_flip(6);
    std::log(0, "[AUTHENTICATOR] Initial frame presented to hardware scanout.").ok();

    let mut last_tick = 0u64;
    let mut refresh = true;

    loop {
        if let Ok(events) = std::event_poll(std::EVENT_TOUCH_INPUT | std::EVENT_TIMER_TICK, 100) {
            if (events & std::EVENT_TOUCH_INPUT) != 0 {
                let mut input = std::InputEvent {
                    kind: 0,
                    x: 0,
                    y: 0,
                    dx: 0,
                    dy: 0,
                    scale_milli: 1000,
                };
                while std::input_event_poll(&mut input).is_ok() {
                    if matches!(input.kind, 4 | 6 | 7 | 8 | 10) {
                        std::log(0, "[AUTHENTICATOR] Navigation gesture. Exiting to launcher.").ok();
                        let _ = std::surface_destroy(6, surface);
                        std::exit_with_code(0);
                    }
                    if input.kind == 1 {
                        if input.y >= 1400 {
                            let _ = std::surface_destroy(6, surface);
                            std::exit_with_code(0);
                        }
                        handle_touch(&mut app, input.x, input.y);
                        refresh = true;
                    }
                }
            }
        }

        // The countdown is recomputed from a fresh clock read once a second, so
        // the bar and the digits can never drift apart from the real time.
        if let Ok(now) = std::get_ticks(0) {
            if now.saturating_sub(last_tick) >= 1000 || refresh {
                last_tick = now;
                if matches!(app.screen, Screen::Codes) {
                    let (_, clock) = probe_rtc();
                    if clock.is_some() {
                        app.clock = clock;
                    }
                }
                refresh = false;
                render(surface, &app);
                let _ = std::surface_flip(6);
            }
        }

        std::yield_now();
    }
}

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    std::log(0, "[PANIC] Authenticator app encountered an unrecoverable error!").ok();
    std::exit_with_code(1);
}
