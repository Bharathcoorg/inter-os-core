#![no_std]

pub use interos_std as std;

/// The app's view of the platform's design system.
pub mod theme;

/// Managed storage sector definitions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VaultSectorType {
    RootKeystore = 100,
    MediaContainers = 200,
    CameraPhotos = 201,
    EncryptedMessages = 202,
    CalendarEvents = 203,
    MusicTracks = 204,
}

impl VaultSectorType {
    pub fn sector_id(&self) -> u64 {
        *self as u64
    }

    pub fn description(&self) -> &'static str {
        match self {
            VaultSectorType::RootKeystore => "Hardware Keystore Root Sector",
            VaultSectorType::MediaContainers => "Encrypted Media Containers",
            VaultSectorType::CameraPhotos => "Camera Photos Storage",
            VaultSectorType::EncryptedMessages => "Sovereign Messages Database",
            VaultSectorType::CalendarEvents => "Calendar & Chrono Planner Events",
            VaultSectorType::MusicTracks => "Music Player Local Library",
        }
    }
}

/// Outcome of one `keystore_derive` probe.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyProbe {
    NotRun,
    Derived,
    Short,
    Refused(i64),
}

/// Outcome of the 16-byte `/dev/urandom` read probe.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntropyProbe {
    NotRun,
    Read16,
    Short,
    AllZero,
    Refused(i64),
}

/// Outcome of the `/system/etc/device_id` read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IdentityState {
    NotChecked,
    Present,
    Malformed,
    Unreadable,
}

/// Outcome of the sovereign backup export, including read-back validation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackupState {
    NotAttempted,
    WrittenAndValid,
    WrittenButChecksumMismatch,
    WrittenButTruncated,
    ReadRefused,
    WriteRefused,
}

/// Computes a Fletcher-32 checksum over `data` (both sums taken mod 255).
pub fn fletcher32(data: &[u8]) -> u32 {
    let mut sum1: u32 = 0;
    let mut sum2: u32 = 0;
    for &byte in data.iter() {
        sum1 = (sum1 + byte as u32) % 255;
        sum2 = (sum2 + sum1) % 255;
    }
    (sum2 << 8) | sum1
}

/// Hex-encodes `data` into `out` (two characters per byte).
/// Returns the number of bytes written.
pub fn hex_encode(data: &[u8], out: &mut [u8]) -> usize {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut i = 0usize;
    for &b in data.iter() {
        if i + 2 > out.len() {
            break;
        }
        out[i] = DIGITS[(b >> 4) as usize];
        out[i + 1] = DIGITS[(b & 0x0F) as usize];
        i += 2;
    }
    i
}

pub fn hex_nibble(c: u8) -> Option<u8> {
    match c {
        b'0'..=b'9' => Some(c - b'0'),
        b'a'..=b'f' => Some(c - b'a' + 10),
        b'A'..=b'F' => Some(c - b'A' + 10),
        _ => None,
    }
}

/// Decodes hex characters in `hex` into `out`.
/// Returns the decoded length, or None on odd length, bad characters, or an
/// output buffer that is too small.
pub fn hex_decode(hex: &[u8], out: &mut [u8]) -> Option<usize> {
    if hex.len() % 2 != 0 || hex.len() / 2 > out.len() {
        return None;
    }
    let mut i = 0usize;
    while i < hex.len() {
        let hi = hex_nibble(hex[i])?;
        let lo = hex_nibble(hex[i + 1])?;
        out[i / 2] = (hi << 4) | lo;
        i += 2;
    }
    Some(hex.len() / 2)
}

/// The block this app reserves for its own crypto self-test.
///
/// WHY the self-test has a block of its own: `vault_encrypt_sector` is not a
/// dry run - the kernel seals the caller's payload over the block it is
/// given and writes the envelope to the storage vault. The managed sectors
/// are live data: the camera, the messages app, the calendar and the media
/// store seal their own records there. A self-test that ran against one of
/// them would replace that record with a test payload, so the probe runs
/// against a block no other app seals.
///
/// WHY 1900: the vault's region holds 2048 blocks and every block an app
/// uses in this tree is at or below 204, so a block well above them and
/// below the limit is free.
pub const SELF_TEST_BLOCK: u64 = 1900;

/// The blocks the managed sectors live at, in the order the tiles show them.
pub const MANAGED_SECTORS: [u64; 6] = [100, 200, 201, 202, 203, 204];

/// The hint a sector tile carries, and its length.
///
/// WHY the hint names the block the probe writes to: the tap runs the
/// vault's crypto probe against the app's own block, never against the
/// sector the row is about, and a hint that read "tap to test" after the
/// sector's number invited the reader to believe the opposite.
pub fn sector_tile_hint(index: usize) -> ([u8; 24], usize) {
    let mut out = [0u8; 24];
    let prefix = b"Sector ";
    out[..prefix.len()].copy_from_slice(prefix);
    let mut n = prefix.len();
    let block = MANAGED_SECTORS.get(index).copied().unwrap_or(0);
    n += decimal(block, &mut out[n..]);
    let tail = b" - block ";
    let take = tail.len().min(out.len() - n);
    out[n..n + take].copy_from_slice(&tail[..take]);
    n += take;
    n += decimal(SELF_TEST_BLOCK, &mut out[n..]);
    (out, n)
}
/// Writes `value` as decimal ASCII into `out`; returns the byte count.
///
/// WHY it is written by hand: the app is `no_std` with no allocator, so the
/// formatting crates that turn a number into text are not available.
fn decimal(value: u64, out: &mut [u8]) -> usize {
    let mut digits = [0u8; 20];
    let mut count = 0usize;
    let mut v = value;
    while v > 0 && count < digits.len() {
        digits[count] = b'0' + (v % 10) as u8;
        count += 1;
        v /= 10;
    }
    let n = count.min(out.len());
    for i in 0..n {
        out[i] = digits[count - 1 - i];
    }
    n
}

/// The verdict and the provenance for the "Key material on screen" row.
///
/// WHY it follows the probe: the row used to say "never shown" and "keys stay
/// inside the syscall boundary" with a success ink, whatever the keystore
/// had answered. When the keystore does export a key, the key is in this
/// app's memory and that sentence is false.
pub fn key_material_row(derived: bool) -> (&'static str, &'static str) {
    if derived {
        (
            "exported to this app",
            "keystore_derive returned the key to Ring 3",
        )
    } else {
        (
            "never shown",
            "the keystore refused to export raw key material",
        )
    }
}

/// The radius of the circular PIN pad discs in pixels.
/// Diameter is 96px, which comfortably exceeds the 48px minimum touch target requirement.
pub const PIN_DISC_RADIUS: usize = 48;

/// Minimum touch target size in pixels on the InterOS design system.
pub const PIN_MIN_TOUCH_TARGET: usize = 48;

/// Column centers for the 3x4 circular PIN keypad on a 720px portrait screen.
pub const PIN_COLS: [usize; 3] = [160, 360, 560];

/// Row centers for the 4 rows of the circular PIN keypad.
///
/// WHY these sit 26 rows lower than the grid this pad was first laid out on: the
/// rows are measured from the top of the screen, and the app's header band had to
/// grow by 26 rows before it could hold the whole line box of its own title and
/// subtitle. The keypad is the band under the header, so the header growing moved
/// the pad down with it. The 120-row pitch and the disc radius are the platform's
/// touch geometry and did not move.
pub const PIN_ROWS: [usize; 4] = [386, 506, 626, 746];

/// Key matrix for the circular keypad.
/// Row 3 contains Clear ('C'), Zero ('0'), and Backspace ('B').
pub const PIN_KEY_MATRIX: [[u8; 3]; 4] = [
    [b'1', b'2', b'3'],
    [b'4', b'5', b'6'],
    [b'7', b'8', b'9'],
    [b'C', b'0', b'B'],
];

/// Resolves whether point (x, y) lands inside any circular PIN disc.
/// Returns Some(key) if hit-tested inside a disc.
pub fn hit_test_pin_disc(x: usize, y: usize) -> Option<u8> {
    for row in 0..4 {
        for col in 0..3 {
            let cx = PIN_COLS[col];
            let cy = PIN_ROWS[row];
            let dx = x as i32 - cx as i32;
            let dy = y as i32 - cy as i32;
            if (dx * dx + dy * dy) as usize <= PIN_DISC_RADIUS * PIN_DISC_RADIUS {
                return Some(PIN_KEY_MATRIX[row][col]);
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_fletcher32_checksum() {
        let data1 = b"abcde";
        let cs1 = fletcher32(data1);
        assert_ne!(cs1, 0);

        let data2 = b"abcdef";
        let cs2 = fletcher32(data2);
        assert_ne!(cs1, cs2);

        // Deterministic
        assert_eq!(fletcher32(data1), cs1);
    }

    #[test]
    fn test_hex_encode_and_decode_roundtrip() {
        let raw = [0x00, 0x12, 0x34, 0xAB, 0xCD, 0xEF, 0xFF];
        let mut hex = [0u8; 14];
        let encoded_len = hex_encode(&raw, &mut hex);
        assert_eq!(encoded_len, 14);
        assert_eq!(&hex, b"001234abcdef\x66\x66"); // 0xff is "ff"

        let mut out = [0u8; 7];
        let decoded_len = hex_decode(&hex, &mut out).expect("hex decode");
        assert_eq!(decoded_len, 7);
        assert_eq!(&out, &raw);
    }

    #[test]
    fn test_hex_decode_invalid() {
        let mut out = [0u8; 4];
        // Odd length
        assert!(hex_decode(b"123", &mut out).is_none());
        // Invalid character
        assert!(hex_decode(b"123g", &mut out).is_none());
        // Buffer too small
        let mut small_out = [0u8; 1];
        assert!(hex_decode(b"1234", &mut small_out).is_none());
    }

    #[test]
    fn test_vault_sector_types() {
        assert_eq!(VaultSectorType::RootKeystore.sector_id(), 100);
        assert_eq!(VaultSectorType::CameraPhotos.sector_id(), 201);
        assert_eq!(VaultSectorType::MusicTracks.sector_id(), 204);
        assert!(VaultSectorType::RootKeystore.description().contains("Keystore"));
    }

    #[test]
    fn the_sector_tile_hints_fit_their_tile() {
        // WHY: the hint names the block the self-test writes to, and a hint
        // the tile ellipsises would hide the very fact it exists to report.
        for index in 0..MANAGED_SECTORS.len() {
            let (hint, len) = sector_tile_hint(index);
            let hint = &hint[..len];
            let w = interos_std::font::measure(hint, 1);
            assert!(
                w <= 272,
                "the hint {:?} is {}px wide and the tile reserves 272px",
                core::str::from_utf8(hint).unwrap(),
                w
            );
        }
    }

    #[test]
    fn the_self_test_never_writes_over_a_managed_sector() {
        // WHY: `vault_encrypt_sector` seals the caller's payload over the
        // block it is given and persists it, so a self-test against a managed
        // sector would replace the record the camera, the messages app or
        // the calendar stored there.
        for sector in MANAGED_SECTORS.iter() {
            assert_ne!(
                *sector, SELF_TEST_BLOCK,
                "the self-test block is a managed sector"
            );
        }
        // And the block is inside the vault's region, so the probe reaches a
        // real block rather than failing at the edge of the range.
        assert!(SELF_TEST_BLOCK < 2048, "the self-test block is past the vault's range");
    }

    #[test]
    fn the_tile_hint_names_its_own_sector_and_the_test_block() {
        for (index, sector) in MANAGED_SECTORS.iter().enumerate() {
            let (hint, len) = sector_tile_hint(index);
            let text = core::str::from_utf8(&hint[..len]).expect("the hint is text");
            assert!(
                text.starts_with("Sector "),
                "the hint does not name a sector: {}",
                text
            );
            let mut digits = [0u8; 20];
            let n = decimal(*sector, &mut digits);
            let sector_text = core::str::from_utf8(&digits[..n]).expect("the block is text");
            assert!(
                text.contains(sector_text),
                "the hint does not name its own sector: {}",
                text
            );
            assert!(
                text.contains("1900"),
                "the hint does not name the block the probe writes to: {}",
                text
            );
        }
    }

    #[test]
    fn the_key_material_row_follows_the_probe() {
        // WHY: the row used to claim the keys stayed inside the syscall
        // boundary whatever the keystore answered, and a keystore that
        // exports a key puts that key in this app's memory.
        let (value, source) = key_material_row(false);
        assert_eq!(value, "never shown");
        assert!(source.contains("refused"));
        let (value, source) = key_material_row(true);
        assert_eq!(value, "exported to this app");
        assert!(source.contains("keystore_derive"));
        // The two readings must not say the same thing.
        assert_ne!(value, "never shown");
    }

    #[test]
    fn test_text_aa() {
        let text = b"A";
        let color = 0xFFFFFFFF;
        let bg = 0xFF000000;
        let mut has_solid = false;
        let mut has_blended = false;
        let mut has_none = false;

        for y in 0..24 {
            for x in 0..16 {
                match std::design::font::test_text_aa(x, y, 0, 0, 1, text, color, bg, false) {
                    Some(c) if c == color => has_solid = true,
                    Some(c) if c == bg => panic!("test_text_aa should never return bare background"),
                    Some(c) => {
                        assert_ne!(c, color);
                        assert_ne!(c, bg);
                        has_blended = true;
                    }
                    None => has_none = true,
                }
            }
        }
        assert!(has_solid, "should contain solid foreground pixels");
        assert!(has_blended, "should contain anti-aliased 4-bit blended edge pixels");
        assert!(has_none, "should contain transparent (None) pixels");
    }
    /// The outline face has to draw at the size the ladder names.
    ///
    /// WHY this is a test and not a print: the two faces used to disagree
    /// about where a run's ink sits and how tall it was, and an app that
    /// mixed them drew a label a few pixels above the line it was meant to
    /// sit on. The vault found it, and the finding was a diagnostic that
    /// printed its numbers and asserted nothing, so nothing held the two
    /// faces together.
    ///
    /// WHY these three properties: the cap height is the size the type
    /// scale names, the run has to fit the line box the scale reserves for
    /// it, and the two faces have to agree about a run's width, because an
    /// app measures a label with one of them and draws it with the other.
    #[test]
    fn the_outline_face_draws_at_the_size_the_ladder_names() {
        use interos_std::design::outline_text;
        use interos_std::design::typeface;
        use interos_std::font;

        // 1456 is the cap height in 1/2048 of an em and 2048 is the em.
        for scale in [1usize, 2, 3, 4] {
            let cap = font::cap_for_scale(scale);
            let em = outline_text::em_for_scale(scale);
            let drawn_cap = (em as u64 * 1456 + 1024) / 2048;
            assert!(
                drawn_cap.abs_diff(cap as u64) <= 1,
                "scale {scale}: the outline face draws a {drawn_cap} pixel cap and the ladder names {cap}"
            );

            let run = outline_text::place_run_cap(0, 0, cap, b"Root keystore", None, false)
                .expect("a run should place at the ladder's cap height");
            let (_x, _y, w, h) = run.bounds();
            let line = font::line_height(scale);
            assert!(
                h as usize <= line,
                "scale {scale}: the run is {h} pixels tall and the line box reserves {line}"
            );
            assert!(w > 0, "scale {scale}: the run has no width");

            // The two faces have to agree about a run's width, because an
            // app measures a label with one and draws it with the other.
            let bitmap = font::measure(b"Root keystore", scale);
            let outline = outline_text::drawn_advance(b"Root keystore", scale, None)
                .expect("the outline face should measure a run");
            let drift = (bitmap as i64 - outline as i64).abs();
            assert!(
                drift <= (bitmap as i64 / 8).max(2),
                "scale {scale}: the bitmap face measures {bitmap} and the outline face {outline}"
            );

            // The ascent the outline places a baseline at is the one the
            // ladder's line box reserves above the baseline.
            let (ascent, _descent) = typeface::line_metrics(em);
            assert!(
                ascent as usize <= line,
                "scale {scale}: the ascent is {ascent} and the line box reserves {line}"
            );

            // The cap height the ladder names and the size it draws at have
            // to be the same size, and the reverse map has to agree with the
            // forward one: an app that asks for a cap height and a renderer
            // that asks for a size have to arrive at the same run.
            assert_eq!(
                outline_text::em_for_scale(scale),
                font::em_for_cap_scale(cap),
                "scale {scale}: the cap height {cap} does not map back to the size the ladder names"
            );
        }
    }

    #[test]
    fn the_pin_keypad_discs_meet_touch_target_requirements() {
        // Requirement: Touch targets must be >= 48px, non-overlapping, with clean gaps.
        assert!(PIN_DISC_RADIUS * 2 >= PIN_MIN_TOUCH_TARGET);
        assert_eq!(PIN_DISC_RADIUS, 48);

        // Check horizontal positioning on 720px screen:
        for &cx in PIN_COLS.iter() {
            assert!(cx >= PIN_DISC_RADIUS, "disc left edge extends off screen");
            assert!(cx + PIN_DISC_RADIUS <= 720, "disc right edge extends off screen");
        }

        // Horizontal gap between columns must be generous (>= 24px)
        for i in 0..PIN_COLS.len() - 1 {
            let left_disc_right = PIN_COLS[i] + PIN_DISC_RADIUS;
            let right_disc_left = PIN_COLS[i + 1] - PIN_DISC_RADIUS;
            assert!(right_disc_left > left_disc_right, "columns collide or overlap");
            let gap = right_disc_left - left_disc_right;
            assert!(gap >= 24, "column gap is too small: {}", gap);
        }

        // Vertical gap between rows must be generous (>= 16px)
        for j in 0..PIN_ROWS.len() - 1 {
            let top_disc_bottom = PIN_ROWS[j] + PIN_DISC_RADIUS;
            let bottom_disc_top = PIN_ROWS[j + 1] - PIN_DISC_RADIUS;
            assert!(bottom_disc_top > top_disc_bottom, "rows collide or overlap");
            let gap = bottom_disc_top - top_disc_bottom;
            assert!(gap >= 16, "row gap is too small: {}", gap);
        }
    }

    #[test]
    fn the_pin_keypad_clear_and_backspace_discs_hit_test() {
        // Clear disc is row 3, col 0 -> 'C'
        let clr_cx = PIN_COLS[0];
        let clr_cy = PIN_ROWS[3];
        assert_eq!(hit_test_pin_disc(clr_cx, clr_cy), Some(b'C'));
        // Edge of Clear disc (at distance PIN_DISC_RADIUS - 1)
        assert_eq!(hit_test_pin_disc(clr_cx + PIN_DISC_RADIUS - 1, clr_cy), Some(b'C'));

        // Zero disc is row 3, col 1 -> '0'
        let zero_cx = PIN_COLS[1];
        let zero_cy = PIN_ROWS[3];
        assert_eq!(hit_test_pin_disc(zero_cx, zero_cy), Some(b'0'));

        // Backspace disc is row 3, col 2 -> 'B'
        let bksp_cx = PIN_COLS[2];
        let bksp_cy = PIN_ROWS[3];
        assert_eq!(hit_test_pin_disc(bksp_cx, bksp_cy), Some(b'B'));
        // Edge of Backspace disc
        assert_eq!(hit_test_pin_disc(bksp_cx, bksp_cy + PIN_DISC_RADIUS - 1), Some(b'B'));

        // Point outside discs returns None
        assert_eq!(hit_test_pin_disc(0, 0), None);
        assert_eq!(hit_test_pin_disc(clr_cx + PIN_DISC_RADIUS + 5, clr_cy), None);
    }

    #[test]
    fn the_typography_scale_2_body_meets_line_height() {
        use interos_std::font;
        let cap = font::cap_for_scale(2);
        assert!(
            (15..=18).contains(&cap),
            "Scale 2 Body cap height {} must stay inside the documented 15..=18 band",
            cap
        );
        let line = font::line_height(2);
        assert!(line >= 24, "Scale 2 line height should reserve at least 24px");
    }

    #[test]
    fn test_vault_ui_text_bounds_and_badges() {
        use interos_std::font;
        let card_w = 672;
        let footer_note = b"The PIN opens this session; it does not release a keystore key";
        assert!(font::measure(footer_note, 1) <= card_w);

        // Managed sector verdicts must fit within sector tile inner width (iw = 292, badge text available <= 266)
        let verdicts = [b"round-trip verified".as_slice(), b"not tested".as_slice(), b"no record".as_slice()];
        for v in verdicts {
            let vw = font::measure_bold(v, 1);
            assert!(vw <= 266, "verdict {:?} exceeds badge text available space", v);
            let badge_w = vw + 34;
            assert!(badge_w <= 292, "badge width exceeds tile inner width");
        }

        // Status frame metric values must fit within metric row half badge capacity
        let metric_values = [
            b"written and validated".as_slice(),
            b"checksum mismatch".as_slice(),
            b"read back truncated".as_slice(),
            b"read-back refused".as_slice(),
            b"settings write refused".as_slice(),
            b"not written yet".as_slice(),
        ];
        for mv in metric_values {
            let mw = font::measure_bold(mv, 1);
            let badge_w = mw + 34;
            assert!(badge_w <= 324, "metric badge width exceeds row space");
        }
    }

    /// The credential screen used to print a Wi-Fi PSK, an ed25519 identity
    /// key and an hsm0 master seed that no code in this app ever read, and to
    /// assert "ENFORCED" over capabilities it cannot query. It now prints only
    /// what its own probes answered.
    #[test]
    fn the_credentials_screen_reports_probes_and_not_invented_secrets() {
        let main = include_str!("main.rs");
        let view = {
            let start = main
                .find("fn credentials_view_pixel")
                .expect("the credentials view is still drawn");
            let rest = &main[start..];
            let end = rest
                .find("\nfn cred_action_rect")
                .or_else(|| rest.find("\nconst STATUS_Y"))
                .expect("the credentials view ends");
            &rest[..end]
        };
        for invented in [
            "wpa3-psk",
            "ed25519",
            "hsm0",
            "STORED SECRETS",
            "SECURITY AUDIT",
            "Zero Ambient Authority",
            "Zero-heap stack isolation active",
            "Hardware keystore root protected",
            "AUTHENTICATED",
        ] {
            assert!(
                !view.contains(invented),
                "the credential screen still prints {invented:?}, which no probe reports"
            );
        }
        // Every row is now derived from state the probes write.
        for read in [
            "vault.is_authenticated",
            "key_verdict(vault.key_probe[0])",
            "key_verdict(vault.key_probe[1])",
            "entropy_verdict(vault.entropy_probe)",
            "identity_verdict(vault.identity)",
            "count_label(verified, vault.sectors.len()",
            "count_label(derived, vault.key_probe.len()",
        ] {
            assert!(view.contains(read), "the credential screen does not read {read}");
        }
    }

    /// The PIN modal sits in front of the keystore probe and used to claim a
    /// hardware root of protection anyway. Nothing in the PIN path releases a
    /// key, so the note may only say what opening the session does.
    #[test]
    fn the_pin_modal_claims_no_keystore_protection() {
        let main = include_str!("main.rs");
        let start = main
            .find("fn pin_modal_pixel")
            .expect("the pin modal is still drawn");
        let end = main[start..]
            .find("fn credentials_view_pixel")
            .expect("the modal ends where the credential screen begins");
        let modal = &main[start..start + end];
        assert!(!modal.contains("Hardware keystore root protected"));
        assert!(!modal.contains("Zero heap allocations"));
        assert!(modal.contains("The PIN opens this session; it does not release a keystore key"));
    }
}

