//! InterOS Sovereign P2P Mesh Messenger
//!
//! A mesh messenger that tells the truth about having nobody to talk to.
//!
//! The previous version of this app logged "Sovereign P2P Mesh Daemon online on
//! 868MHz" and then logged "Multi-hop mesh datagram routed" whenever an IPC
//! event arrived. Neither line was true of this platform, and there is no 868 MHz
//! radio anywhere in the kernel: `CapabilityType` has no mesh or P2P variant at
//! all, and `WirelessManager` reports `hardware_present: false` with Wi-Fi and
//! Bluetooth both `Disabled`. A chat log that fills itself with invented
//! conversation is worse than an empty one, so there is none here.
//!
//! What this app does instead is report the transport state it can actually
//! measure, and then exercise the ONE local path that genuinely exists.
//!
//! Transport state, from real sources:
//!   - `wireless_query()`     -> the radio HAL's own Wi-Fi/BLE fields
//!   - `settings_hardware_status("wifi" | "bluetooth")` -> the settings inventory
//!   - `net_socket()`         -> whether a socket could be bound at all
//!   - `network_poll()`       -> how many frames the NIC has actually processed
//!
//! The local path: the kernel's UDP stack resolves loopback to MAC 00:00:00:00:00:01
//! and transmits into the real `NETWORK_DEVICE` (kernel/src/net/mod.rs:121), so a
//! datagram this app sends to 127.0.0.1 genuinely traverses the same
//! serialise/transmit/receive path a remote packet would. Sending one and reading
//! it back is a real round trip over the local interface, and the app reports the
//! byte count the kernel actually returned. It is labelled as loopback, because
//! proving the local stack works is not the same as reaching a peer.
//!
//! Peers: there are none, and no source on this platform enumerates any. The
//! conversation area is therefore empty and says why. Nothing in this file can
//! put a message from another person on screen.

#![no_std]
#![no_main]

mod font;

use core::panic::PanicInfo;
use interos_std as std;

const W: usize = 720;
const H: usize = 1440;
const CHUNK: usize = 4096;

/// The loopback port this app binds for its self-test.
const LOOPBACK_PORT: u16 = 49052;

/// Service id of the network service in the kernel service registry.
const NETWORK_SERVICE_ID: u8 = 5;

/// Payload of the loopback self-test. Its bytes are compared on receipt, so a
/// round trip that returned different data would be reported as a failure rather
/// than as a success.
const LOOPBACK_PROBE: &[u8] = b"interos-meshchat-loopback-probe";

/// The version string compiled into this binary from the crate manifest.
const BUILD_VERSION: &str = env!("CARGO_PKG_VERSION");

// -----------------------------------------------------------------------------
// Measured state
// -----------------------------------------------------------------------------

/// What `wireless_query` said about the radios.
enum Radios {
    /// The HAL returned a payload. `hardware_present` is not in the JSON the
    /// kernel emits, so the app does not claim a radio exists on this evidence
    /// alone; it reports the state words the payload does carry.
    Reported { wifi: [u8; 24], wifi_len: usize, ssid: [u8; 32], ssid_len: usize, rssi: i64, freq: i64, ble: bool },
    Denied(i64),
    Malformed,
}

/// Outcome of the loopback self-test. Each variant corresponds to a real step
/// that either succeeded or returned a real status code.
enum Loopback {
    /// Not attempted yet.
    Pending,
    /// A socket was bound.
    NoSocket(i64),
    /// The datagram was handed to the kernel for transmission.
    SendRefused(i64),
    /// The datagram was transmitted but nothing came back within the wait.
    Sent { socket: u32, bytes_sent: usize, note: &'static [u8] },
    /// A datagram came back and its bytes matched the probe exactly.
    RoundTrip { socket: u32, bytes_sent: usize, bytes_received: usize },
    /// A datagram came back but its contents did not match the probe.
    Mismatch { socket: u32, bytes_sent: usize, bytes_received: usize },
    /// The receive call itself was refused.
    RecvRefused { socket: u32, bytes_sent: usize, code: i64 },
}

struct MeshChat {
    task_id: u64,
    surface_id: u32,
    radios: Radios,
    loopback: Loopback,
    /// How many frames `network_poll` says the NIC has processed. This counts
    /// real hardware activity and is shown as such, never as a message count.
    frames_processed: Option<usize>,
    inv_wifi: Option<i64>,
    inv_bluetooth: Option<i64>,
    net_service: Option<(u8, u32)>,
    /// Messages genuinely received over IPC, with their real sender and type.
    /// Only what a peer actually sent can appear here.
    inbox: [InboxEntry; 4],
    inbox_count: usize,
    status: [u8; 64],
    status_len: usize,
    status_color: u32,
}

/// One real inbound IPC message.
#[derive(Clone, Copy)]
struct InboxEntry {
    sender: u64,
    msg_type: u32,
    len: usize,
}

// -----------------------------------------------------------------------------
// Minimal flat-JSON reader
// -----------------------------------------------------------------------------

fn json_value(buf: &[u8], key: &[u8]) -> Option<usize> {
    let mut i = 0usize;
    while i + key.len() + 2 < buf.len() {
        if buf[i] == b'"'
            && &buf[i + 1..i + 1 + key.len()] == key
            && buf[i + 1 + key.len()] == b'"'
            && (i == 0 || buf[i - 1] == b'{' || buf[i - 1] == b',')
        {
            let after = i + key.len() + 2;
            if after < buf.len() && buf[after] == b':' {
                return Some(after + 1);
            }
        }
        i += 1;
    }
    None
}

fn json_bool(buf: &[u8], key: &[u8]) -> Option<bool> {
    let i = json_value(buf, key)?;
    if i >= buf.len() {
        return None;
    }
    match buf[i] {
        b't' if buf[i..].starts_with(b"true") => Some(true),
        b'f' if buf[i..].starts_with(b"false") => Some(false),
        _ => None,
    }
}

fn json_i64(buf: &[u8], key: &[u8]) -> Option<i64> {
    let mut i = json_value(buf, key)?;
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

fn json_str(buf: &[u8], key: &[u8], out: &mut [u8]) -> Option<usize> {
    let mut i = json_value(buf, key)?;
    if i >= buf.len() || buf[i] != b'"' {
        return None;
    }
    i += 1;
    let mut n = 0usize;
    while i < buf.len() && buf[i] != b'"' {
        if n < out.len() {
            out[n] = buf[i];
        }
        n += 1;
        i += 1;
    }
    Some(n.min(out.len()))
}

// -----------------------------------------------------------------------------
// Real sources
// -----------------------------------------------------------------------------

fn read_radios() -> Radios {
    let mut buf = [0u8; 256];
    let n = match std::wireless_query(&mut buf) {
        Ok(n) => n,
        Err(e) => return Radios::Denied(e),
    };
    let payload = &buf[..n.min(buf.len())];

    let mut wifi = [0u8; 24];
    let wifi_len = match json_str(payload, b"wifi_state", &mut wifi) {
        Some(k) => k,
        None => return Radios::Malformed,
    };
    let mut ssid = [0u8; 32];
    let ssid_len = json_str(payload, b"ssid", &mut ssid).unwrap_or(0);
    let rssi = match json_i64(payload, b"rssi_dbm") {
        Some(v) => v,
        None => return Radios::Malformed,
    };
    let freq = match json_i64(payload, b"freq_mhz") {
        Some(v) => v,
        None => return Radios::Malformed,
    };
    let ble = match json_bool(payload, b"ble_adv") {
        Some(v) => v,
        None => return Radios::Malformed,
    };
    Radios::Reported { wifi, wifi_len, ssid, ssid_len, rssi, freq, ble }
}

fn read_inventory(category: &str) -> Option<i64> {
    std::settings_hardware_status(category).ok()
}

fn read_net_service() -> Option<(u8, u32)> {
    let mut out = [0u8; 12];
    match std::service_query(NETWORK_SERVICE_ID, &mut out) {
        Ok(n) if n >= 12 => Some((
            out[1],
            u32::from_le_bytes([out[6], out[7], out[8], out[9]]),
        )),
        _ => None,
    }
}

fn service_state_word(state: u8) -> &'static [u8] {
    // ServiceState: Declared=0, Starting=1, Running=2, Stopped=3, Failed=4.
    match state {
        0 => b"declared",
        1 => b"starting",
        2 => b"running",
        3 => b"stopped",
        4 => b"failed",
        _ => b"unknown",
    }
}

fn inventory_word(code: i64) -> &'static [u8] {
    match code {
        0 => b"available",
        1 => b"simulated",
        2 => b"unavailable",
        _ => b"unknown",
    }
}

fn status_name(code: i64) -> &'static [u8] {
    match code {
        -1 => b"PermissionDenied",
        -2 => b"InvalidSlot",
        -3 => b"InvalidArguments",
        -6 => b"TargetNotFound",
        -12 => b"DeviceError",
        _ => b"refused",
    }
}

/// Exercises the real local datagram path.
///
/// This is a genuine round trip and not a simulation: `sys_net_sendto` detects
/// the loopback destination, resolves MAC 00:00:00:00:00:01, serialises UDP, IPv4
/// and an Ethernet frame, and transmits into the same `NETWORK_DEVICE` any remote
/// packet would use. The datagram is then read back off the socket and compared
/// byte for byte against what was sent, so only a real delivery is reported as
/// one.
fn loopback_round_trip() -> Loopback {
    let socket = match std::net_socket(LOOPBACK_PORT) {
        Ok(s) => s,
        Err(e) => return Loopback::NoSocket(e),
    };

    match std::net_sendto([127, 0, 0, 1], LOOPBACK_PORT, LOOPBACK_PROBE) {
        Ok(()) => {}
        Err(e) => {
            // The socket is left to be reclaimed with the task; closing it on a
            // path that may already have failed would add noise, not clarity.
            return Loopback::SendRefused(e);
        }
    }
    let bytes_sent = LOOPBACK_PROBE.len();

    // Drive the receive path. A datagram already queued is returned immediately;
    // otherwise the poll is a no-op and the receive reports an empty mailbox,
    // which is a truthful "nothing came back" rather than a failure to try.
    let _ = std::network_poll();

    let mut buf = [0u8; 256];
    match std::net_recvfrom(socket, &mut buf) {
        Ok(0) => Loopback::Sent {
            socket,
            bytes_sent,
            note: b"The receive path returned an empty datagram, so the",
        },
        Ok(n) => {
            let n = n.min(buf.len());
            if buf[..n] == *LOOPBACK_PROBE {
                Loopback::RoundTrip { socket, bytes_sent, bytes_received: n }
            } else {
                Loopback::Mismatch { socket, bytes_sent, bytes_received: n }
            }
        }
        Err(e) => Loopback::RecvRefused { socket, bytes_sent, code: e },
    }
}

// -----------------------------------------------------------------------------
// Formatting helpers
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
    let mut n = 0usize;
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

// -----------------------------------------------------------------------------
// Palette and layout
// -----------------------------------------------------------------------------

const BG: u32 = 0xFF070B12;
const CARD: u32 = 0xFF111B2B;
const CARD_RAISED: u32 = 0xFF17243A;
const EDGE: u32 = 0xFF26354D;
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
    Card { x0: 30, y0: 158, x1: 690, y1: 500, caption: b"TRANSPORT STATE" },
    Card { x0: 30, y0: 520, x1: 690, y1: 830, caption: b"LOCAL PATH, EXERCISED FOR REAL" },
    Card { x0: 30, y0: 850, x1: 690, y1: 1150, caption: b"CONVERSATION" },
    Card { x0: 30, y0: 1170, x1: 690, y1: 1404, caption: b"WHY THERE IS NO PEER" },
];

// -----------------------------------------------------------------------------
// Rendering
// -----------------------------------------------------------------------------

fn render(surface: u32, app: &MeshChat) {
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
fn pixel(x: usize, y: usize, app: &MeshChat) -> u32 {
    if y < 96 {
        if y >= 94 {
            return EDGE;
        }
        if (24..100).contains(&x) && (28..68).contains(&y) {
            let border = y == 28 || y == 67 || x == 24 || x == 99;
            let chev = (x as isize - 62).abs() + (y as isize - 48).abs() <= 11 && x <= 62;
            return if border || chev { TEXT } else { CARD_RAISED };
        }
        if font::text_at(x, y, 116, 32, 3, b"Mesh Chat") {
            return TEXT;
        }
        return CARD;
    }

    // Status band: the one-line verdict, at a size the display can carry.
    if (96..130).contains(&y) {
        if font::text_at(x, y, 30, 104, 2, &app.status[..app.status_len]) {
            return app.status_color;
        }
        return BG;
    }

    for card in CARDS.iter() {
        if y >= card.y0 && y < card.y1 && x >= card.x0 && x < card.x1 {
            if y == card.y0 || y == card.y1 - 1 || x == card.x0 || x == card.x1 - 1 {
                return EDGE;
            }
            if font::text_at(x, y, card.x0 + 14, card.y0 + 10, 2, card.caption) {
                return ACCENT;
            }
            return CARD;
        }
    }

    if (158..500).contains(&y) && (30..690).contains(&x) {
        if let Some(c) = transport_card(x, y, app) {
            return c;
        }
        return CARD;
    }
    if (520..830).contains(&y) && (30..690).contains(&x) {
        if let Some(c) = loopback_card(x, y, app) {
            return c;
        }
        return CARD;
    }
    if (850..1150).contains(&y) && (30..690).contains(&x) {
        if let Some(c) = conversation_card(x, y, app) {
            return c;
        }
        return CARD;
    }
    if (1170..1404).contains(&y) && (30..690).contains(&x) {
        if let Some(c) = peer_card(x, y, app) {
            return c;
        }
        return CARD;
    }

    // Home gesture pill
    if (1418..1424).contains(&y) && (260..460).contains(&x) {
        return MUTED;
    }
    BG
}

#[inline(never)]
fn row(x: usize, y: usize, top: usize, label: &[u8], verdict: &[u8], color: u32, source: &[u8]) -> Option<u32> {
    if font::text_at(x, y, 44, top, 2, label) {
        return Some(MUTED);
    }
    let vx = 676usize.saturating_sub(font::text_width(verdict, 2));
    if font::text_at(x, y, vx, top, 2, verdict) {
        return Some(color);
    }
    if font::text_at(x, y, 44, top + 18, 2, source) {
        return Some(FAINT);
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

/// Card 1: what the radios and the settings inventory actually report.
#[inline(never)]
fn transport_card(x: usize, y: usize, app: &MeshChat) -> Option<u32> {
    // The HAL's own words. `wireless_query` does not serialise the manager's
    // `hardware_present` flag, so the app reports the state words it does carry
    // and does not infer a working radio from their absence.
    match &app.radios {
        Radios::Reported { wifi, wifi_len, ssid, ssid_len, rssi, freq, ble } => {
            if let Some(c) = row(
                x,
                y,
                190,
                b"Wi-Fi state",
                &wifi[..*wifi_len],
                if wifi.starts_with(b"Disabled") { WARN } else { OK },
                b"wireless_query() -> wifi_state",
            ) {
                return Some(c);
            }
            // An empty SSID is reported as such rather than as a blank name.
            let ssid_word: &[u8] = if *ssid_len == 0 { b"none associated".as_slice() } else { &ssid[..*ssid_len] };
            if let Some(c) = row(x, y, 232, b"SSID", ssid_word, MUTED, b"wireless_query() -> ssid") {
                return Some(c);
            }
            let mut w = [0u8; 40];
            let n = write_lit(b"rssi ", &mut w);
            let n = n + fmt_i64(*rssi, &mut w[n..]);
            let n = n + write_lit(b" dBm, ", &mut w[n..]);
            let n = n + fmt_i64(*freq, &mut w[n..]);
            let n = n + write_lit(b" MHz", &mut w[n..]);
            if let Some(c) = row(x, y, 274, b"Link figures", &w[..n], MUTED, b"wireless_query() -> rssi_dbm, freq_mhz") {
                return Some(c);
            }
            let ble_word: &[u8] = if *ble { b"advertising" } else { b"not advertising" };
            if let Some(c) = row(x, y, 316, b"BLE beacon", ble_word, MUTED, b"wireless_query() -> ble_adv") {
                return Some(c);
            }
        }
        Radios::Denied(e) => {
            let mut w = [0u8; 40];
            let mut n = write_lit(b"refused (", &mut w);
            n = n + fmt_i64(*e, &mut w[n..]);
            let n = n + write_lit(b" ", &mut w[n..]);
            let n = n + write_lit(status_name(*e), &mut w[n..]);
            if let Some(c) = row(x, y, 190, b"Radio HAL", &w[..n], WARN, b"wireless_query() needs SystemControl") {
                return Some(c);
            }
        }
        Radios::Malformed => {
            if let Some(c) = row(x, y, 190, b"Radio HAL", b"no data", WARN, b"wireless_query() payload lacked fields") {
                return Some(c);
            }
        }
    }

    // The settings inventory, as a separate and separately-named source.
    let mut w = [0u8; 24];
    let (color, wn) = match app.inv_wifi {
        Some(code) => {
            let n = write_lit(inventory_word(code), &mut w);
            (if code == 0 { OK } else { WARN }, n)
        }
        None => {
            let n = write_lit(b"refused", &mut w);
            (WARN, n)
        }
    };
    if let Some(c) = row(x, y, 358, b"Inventory, Wi-Fi", &w[..wn], color, b"settings_hardware_status(\"wifi\")") {
        return Some(c);
    }

    let mut w = [0u8; 24];
    let (color, wn) = match app.inv_bluetooth {
        Some(code) => {
            let n = write_lit(inventory_word(code), &mut w);
            (if code == 0 { OK } else { WARN }, n)
        }
        None => {
            let n = write_lit(b"refused", &mut w);
            (WARN, n)
        }
    };
    if let Some(c) = row(x, y, 400, b"Inventory, BLE", &w[..wn], color, b"settings_hardware_status(\"bluetooth\")") {
        return Some(c);
    }

    // Frames the NIC has actually processed. This is real hardware activity and
    // is deliberately not described as a message count.
    let mut w = [0u8; 40];
    let (color, wn) = match app.frames_processed {
        Some(n) => {
            let k = write_lit(b"", &mut w);
            let k = k + fmt_u64(n as u64, &mut w[k..]);
            (MUTED, k)
        }
        None => {
            let n = write_lit(b"refused", &mut w);
            (WARN, n)
        }
    };
    if let Some(c) = row(x, y, 442, b"NIC frames", &w[..wn], color, b"network_poll() -> frames processed") {
        return Some(c);
    }

    let mut w = [0u8; 32];
    let (svc_color, wn) = match app.net_service {
        Some((2, gen)) => {
            let n = write_lit(service_state_word(2), &mut w);
            let n = n + write_lit(b" gen ", &mut w[n..]);
            (OK, n + fmt_u64(gen as u64, &mut w[n..]))
        }
        Some((state, gen)) => {
            let n = write_lit(service_state_word(state), &mut w);
            let n = n + write_lit(b" gen ", &mut w[n..]);
            (WARN, n + fmt_u64(gen as u64, &mut w[n..]))
        }
        None => (WARN, write_lit(b"no record", &mut w)),
    };
    if let Some(c) = row(x, y, 484, b"Network service", &w[..wn], svc_color, b"service_query(5)") {
        return Some(c);
    }
    None
}

/// Card 2: the local path, and exactly what it proved.
#[inline(never)]
fn loopback_card(x: usize, y: usize, app: &MeshChat) -> Option<u32> {
    let mut w = [0u8; 40];
    match &app.loopback {
        Loopback::Pending => {
            if let Some(c) = row(x, y, 566, b"Loopback", b"not attempted", MUTED, b"net_socket(49052)") {
                return Some(c);
            }
        }
        Loopback::NoSocket(e) => {
            let mut n = write_lit(b"no socket (", &mut w);
            n = n + fmt_i64(*e, &mut w[n..]);
            n = n + write_lit(b" ", &mut w[n..]);
            n = n + write_lit(status_name(*e), &mut w[n..]);
            if let Some(c) = row(x, y, 566, b"Loopback", &w[..n], WARN, b"net_socket(49052) refused") {
                return Some(c);
            }
        }
        Loopback::SendRefused(e) => {
            let mut n = write_lit(b"send failed (", &mut w);
            n = n + fmt_i64(*e, &mut w[n..]);
            n = n + write_lit(b" ", &mut w[n..]);
            n = n + write_lit(status_name(*e), &mut w[n..]);
            if let Some(c) = row(x, y, 566, b"Loopback", &w[..n], WARN, b"net_sendto(127.0.0.1) refused") {
                return Some(c);
            }
        }
        Loopback::Sent { socket, bytes_sent, note } => {
            let mut n = write_lit(b"sent ", &mut w);
            n = n + fmt_u64(*bytes_sent as u64, &mut w[n..]);
            n = n + write_lit(b" B, no reply", &mut w[n..]);
            if let Some(c) = row(x, y, 566, b"Loopback", &w[..n], WARN, b"net_sendto -> net_recvfrom") {
                return Some(c);
            }
            if line(x, y, 608, note, MUTED).is_some() {
                return Some(MUTED);
            }
            if line(x, y, 630, b"kernel returned no bytes, and none are shown.", FAINT).is_some() {
                return Some(FAINT);
            }
            let mut w2 = [0u8; 24];
            let k = fmt_u64(*socket as u64, &mut w2);
            if let Some(c) = row(x, y, 662, b"Socket id", &w2[..k], MUTED, b"net_socket() -> socket") {
                return Some(c);
            }
        }
        Loopback::RoundTrip { socket, bytes_sent, bytes_received } => {
            let mut n = write_lit(b"round trip ", &mut w);
            n = n + fmt_u64(*bytes_received as u64, &mut w[n..]);
            n = n + write_lit(b" B", &mut w[n..]);
            if let Some(c) = row(x, y, 566, b"Loopback", &w[..n], OK, b"net_sendto -> net_recvfrom, bytes matched") {
                return Some(c);
            }
            let mut w2 = [0u8; 40];
            let mut k = write_lit(b"sent ", &mut w2);
            k = k + fmt_u64(*bytes_sent as u64, &mut w2[k..]);
            k = k + write_lit(b", got ", &mut w2[k..]);
            k = k + fmt_u64(*bytes_received as u64, &mut w2[k..]);
            k = k + write_lit(b", socket ", &mut w2[k..]);
            k = k + fmt_u64(*socket as u64, &mut w2[k..]);
            if let Some(c) = row(x, y, 608, b"Verified", &w2[..k], OK, b"payload compared byte for byte") {
                return Some(c);
            }
        }
        Loopback::Mismatch { socket, bytes_sent, bytes_received } => {
            let mut n = write_lit(b"mismatch ", &mut w);
            n = n + fmt_u64(*bytes_received as u64, &mut w[n..]);
            n = n + write_lit(b" B", &mut w[n..]);
            if let Some(c) = row(x, y, 566, b"Loopback", &w[..n], WARN, b"net_recvfrom returned other bytes") {
                return Some(c);
            }
            let mut w2 = [0u8; 40];
            let mut k = write_lit(b"sent ", &mut w2);
            k = k + fmt_u64(*bytes_sent as u64, &mut w2[k..]);
            k = k + write_lit(b", socket ", &mut w2[k..]);
            k = k + fmt_u64(*socket as u64, &mut w2[k..]);
            if let Some(c) = row(x, y, 608, b"Detail", &w2[..k], MUTED, b"probe payload did not come back intact") {
                return Some(c);
            }
        }
        Loopback::RecvRefused { socket, bytes_sent, code } => {
            let mut n = write_lit(b"sent ", &mut w);
            n = n + fmt_u64(*bytes_sent as u64, &mut w[n..]);
            n = n + write_lit(b" B, recv (", &mut w[n..]);
            n = n + fmt_i64(*code, &mut w[n..]);
            n = n + write_lit(b" ", &mut w[n..]);
            n = n + write_lit(status_name(*code), &mut w[n..]);
            if let Some(c) = row(x, y, 566, b"Loopback", &w[..n], WARN, b"net_recvfrom refused") {
                return Some(c);
            }
            let mut w2 = [0u8; 24];
            let k = fmt_u64(*socket as u64, &mut w2);
            if let Some(c) = row(x, y, 608, b"Socket id", &w2[..k], MUTED, b"net_socket() -> socket") {
                return Some(c);
            }
        }
    }

    // WHY the scope note is here rather than in the caption: a successful
    // loopback proves the local stack works. It does not prove a peer exists, and
    // reading it as "mesh online" is exactly the claim this app must not make.
    if line(x, y, 700, b"This proves the local stack works. It is not a peer.", MUTED).is_some() {
        return Some(MUTED);
    }
    if line(x, y, 730, b"127.0.0.1 resolves to MAC 00:00:00:00:00:01 and", FAINT).is_some() {
        return Some(FAINT);
    }
    line(x, y, 754, b"is transmitted into the real network device.", FAINT)
}

/// Card 3: the conversation area. Empty unless a peer really sent something.
#[inline(never)]
fn conversation_card(x: usize, y: usize, app: &MeshChat) -> Option<u32> {
    if app.inbox_count == 0 {
        // The empty state names the reason rather than implying a quiet chat.
        if font::text_at(x, y, font::text_center(60, 660, 2, b"No messages."), 960, 2, b"No messages.") {
            return Some(TEXT);
        }
        if font::text_at(
            x,
            y,
            font::text_center(60, 660, 2, b"Nothing was received, because"),
            1000,
            2,
            b"Nothing was received, because",
        ) {
            return Some(MUTED);
        }
        if font::text_at(
            x,
            y,
            font::text_center(60, 660, 2, b"no peer is bound to this device."),
            1030,
            2,
            b"no peer is bound to this device.",
        ) {
            return Some(MUTED);
        }
        if font::text_at(
            x,
            y,
            font::text_center(60, 660, 2, b"No placeholder conversation."),
            1074,
            2,
            b"No placeholder conversation.",
        ) {
            return Some(FAINT);
        }
        return None;
    }

    // Real inbound messages, from the IPC mailbox only. Each line is the sender
    // id, message type and byte count the kernel reported.
    for i in 0..app.inbox_count {
        let top = 890 + i * 52;
        let e = &app.inbox[i];
        let mut w = [0u8; 40];
        let n = write_lit(b"from task ", &mut w);
        let n = n + fmt_u64(e.sender, &mut w[n..]);
        if let Some(c) = row(x, y, top, b"IPC message", &w[..n], OK, b"ipc::receive_any() -> sender_id") {
            return Some(c);
        }
        let mut w = [0u8; 40];
        let n = write_lit(b"type ", &mut w);
        let n = n + fmt_u64(e.msg_type as u64, &mut w[n..]);
        let n = n + write_lit(b", ", &mut w[n..]);
        let n = n + fmt_u64(e.len as u64, &mut w[n..]);
        let n = n + write_lit(b" bytes", &mut w[n..]);
        if let Some(c) = row(x, y, top + 22, b"Detail", &w[..n], MUTED, b"ipc::receive_any() -> msg_type, len") {
            return Some(c);
        }
    }
    None
}

/// Card 4: the scope statement, plus this app's own identity.
#[inline(never)]
fn peer_card(x: usize, y: usize, app: &MeshChat) -> Option<u32> {
    let lines: [&[u8]; 6] = [
        b"There is no mesh radio in this kernel:",
        b"CapabilityType has no mesh or P2P variant, and",
        b"WirelessManager reports no hardware present.",
        b"There is also no syscall that enumerates peers,",
        b"so the peer list cannot be shown even in",
        b"principle, and it is not invented here.",
    ];
    for (i, text) in lines.iter().enumerate() {
        let top = 1214 + i * 22;
        if font::text_at(x, y, 44, top, 2, text) {
            return Some(MUTED);
        }
    }

    let mut v = [0u8; 24];
    let n = fmt_u64(app.task_id, &mut v);
    if let Some(c) = row(x, y, 1348, b"Task id", &v[..n], MUTED, b"get_task_id()") {
        return Some(c);
    }
    let n = fmt_u64(app.surface_id as u64, &mut v);
    if let Some(c) = row(x, y, 1372, b"Surface", &v[..n], MUTED, b"surface_create(6, 720x1440, z=200)") {
        return Some(c);
    }
    let _ = BUILD_VERSION;
    None
}

// -----------------------------------------------------------------------------
// Entry point
// -----------------------------------------------------------------------------

fn status(app: &mut MeshChat, text: &[u8], color: u32) {
    let n = text.len().min(app.status.len());
    app.status[..n].copy_from_slice(&text[..n]);
    app.status_len = n;
    app.status_color = color;
}

/// Reads any IPC that a real peer has sent, and records it verbatim.
fn drain_inbox(app: &mut MeshChat) {
    let mut buf = [0u8; 512];
    while app.inbox_count < app.inbox.len() {
        match std::ipc::receive_any(&mut buf) {
            Ok((sender, msg_type, len)) => {
                app.inbox[app.inbox_count] = InboxEntry { sender, msg_type, len };
                app.inbox_count += 1;
            }
            Err(_) => break,
        }
    }
}

#[no_mangle]
pub extern "C" fn _start() -> ! {
    std::log(0, "[MESHCHAT (Ring 3)] Starting. Probing transport state.").ok();

    let surface = match std::surface_create(6, W as u32, H as u32, 200) {
        Ok(s) => s,
        Err(_) => {
            std::log(0, "[MESHCHAT] No display surface available; nothing to draw.").ok();
            std::exit_with_code(1);
        }
    };

    let mut app = MeshChat {
        task_id: std::get_task_id(),
        surface_id: surface,
        radios: Radios::Malformed,
        loopback: Loopback::Pending,
        frames_processed: None,
        inv_wifi: None,
        inv_bluetooth: None,
        net_service: None,
        inbox: [InboxEntry { sender: 0, msg_type: 0, len: 0 }; 4],
        inbox_count: 0,
        status: [0; 64],
        status_len: 0,
        status_color: MUTED,
    };

    app.radios = read_radios();
    app.inv_wifi = read_inventory("wifi");
    app.inv_bluetooth = read_inventory("bluetooth");
    app.net_service = read_net_service();
    app.frames_processed = std::network_poll().ok();
    app.loopback = loopback_round_trip();
    drain_inbox(&mut app);

    // The console reports what the loopback actually did. It never says the mesh
    // is online, because nothing here establishes that. The status text is
    // chosen first and applied after the match, because the match borrows `app`
    // immutably and `status` needs it mutably.
    let (status_text, status_color): (&[u8], u32) = match &app.loopback {
        Loopback::RoundTrip { .. } => {
            std::log(0, "[MESHCHAT] Loopback datagram round trip verified: bytes matched on receipt.").ok();
            (b"loopback verified; still no peer on this device", ACCENT)
        }
        Loopback::Sent { .. } => {
            std::log(0, "[MESHCHAT] Loopback datagram was transmitted; nothing was received back.").ok();
            (b"loopback sent, no reply; no peer on this device", ACCENT)
        }
        Loopback::NoSocket(_) | Loopback::SendRefused(_) | Loopback::RecvRefused { .. } => {
            std::log(0, "[MESHCHAT] The local datagram path refused this app; reporting the refusal.").ok();
            (b"local datagram path refused; reported as refused", ACCENT)
        }
        Loopback::Mismatch { .. } => {
            std::log(0, "[MESHCHAT] Loopback returned bytes that did not match the probe.").ok();
            (b"loopback returned unexpected bytes", WARN)
        }
        Loopback::Pending => (b"transport probed", ACCENT),
    };
    status(&mut app, status_text, status_color);

    if app.inbox_count > 0 {
        std::log(0, "[MESHCHAT] Received a real IPC message from a peer task.").ok();
    } else {
        std::log(0, "[MESHCHAT] No peer is bound to this device; the conversation stays empty.").ok();
    }

    render(surface, &app);
    let _ = std::surface_flip(6);
    std::log(0, "[MESHCHAT] Initial frame presented to hardware scanout.").ok();

    loop {
        if let Ok(events) = std::event_poll(
            std::EVENT_TOUCH_INPUT | std::EVENT_TIMER_TICK | std::EVENT_IPC_MSG,
            100,
        ) {
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
                        std::log(0, "[MESHCHAT] Navigation gesture. Exiting to launcher.").ok();
                        let _ = std::surface_destroy(6, surface);
                        std::exit_with_code(0);
                    }
                    if input.kind == 1 {
                        let tapped_back = (24..100).contains(&input.x) && (28..68).contains(&input.y);
                        if tapped_back || input.y >= 1400 {
                            let _ = std::surface_destroy(6, surface);
                            std::exit_with_code(0);
                        }
                        // Re-probe everything, so the screen only ever shows a
                        // transport state that was just measured.
                        app.radios = read_radios();
                        app.inv_wifi = read_inventory("wifi");
                        app.inv_bluetooth = read_inventory("bluetooth");
                        app.net_service = read_net_service();
                        app.frames_processed = std::network_poll().ok();
                        app.loopback = loopback_round_trip();
                        drain_inbox(&mut app);
                        status(&mut app, b"re-probed radios, network and loopback path", ACCENT);
                        render(surface, &app);
                        let _ = std::surface_flip(6);
                    }
                }
            }
            if (events & std::EVENT_IPC_MSG) != 0 {
                // An IPC event is only a hint that the mailbox is non-empty. The
                // message is read and recorded; nothing is inferred from the flag.
                drain_inbox(&mut app);
                render(surface, &app);
                let _ = std::surface_flip(6);
            }
        }
        std::yield_now();
    }
}

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    std::log(0, "[PANIC] Mesh chat app encountered an unrecoverable error!").ok();
    std::exit_with_code(1);
}
