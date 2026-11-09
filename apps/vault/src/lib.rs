// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

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
///
/// WHY the provenance is plain words: it used to quote the syscall and the
/// privilege ring it returned through. The verdict keeps the platform's own
/// word -- a reading the app did not name is not the app's to rename -- but
/// the line under it answers the question the reader actually arrived with.
pub fn key_material_row(derived: bool) -> (&'static str, &'static str) {
    if derived {
        (
            "handed to this app",
            "the device handed the raw key to this app",
        )
    } else {
        (
            "never shown",
            "the device refused to hand over the key",
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
        // WHY measured by what the reading must carry rather than by one
        // sentence: this app's copy belongs to the person reading it, and a
        // gate that named the whole sentence failed every rewording that kept
        // the claim. What may never come back is a derived reading that hides
        // whose memory the raw key now sits in.
        assert_ne!(value, "never shown", "a derived key is reported as if none was");
        assert!(
            value.contains("this app"),
            "the derived verdict {value:?} never says the key reached this app"
        );
        // WHY the words changed: the provenance line quoted the syscall
        // signature and the privilege ring it returned through. A person
        // choosing whether to trust this device owns neither; what they own
        // is the consequence, which is that the raw key is now in an app.
        assert_plain_words(source);
        assert!(
            source.contains("raw key") && source.contains("this app"),
            "the exported reading must still say whose key this is and where \
             it now sits: {source:?}"
        );
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
        use interos_std::design::typography;
        use interos_std::font;
        // The rung this app draws its body at is the platform's Body rung, so
        // a change to the ladder moves this screen with every other one
        // instead of leaving it behind.
        assert_eq!(
            font::cap_for_scale(2),
            typography::BODY_CAP_PX,
            "rung 2 must draw the Body cap the platform names"
        );
        assert_eq!(
            font::line_height(2),
            typography::BODY_LINE_PX,
            "rung 2 must reserve the Body line box the platform names"
        );
        // A line box only separates rows if it is taller than the caps it
        // carries, which is the whole point of raising the ladder.
        assert!(
            font::line_height(2) > font::cap_for_scale(2) as usize,
            "the Body line box has no air above and below its caps"
        );
    }

    #[test]
    fn test_vault_ui_text_bounds_and_badges() {
        use interos_std::font;
        let card_w = 672;
        let footer_note = b"The PIN opens this session; it releases no device key";
        assert!(font::measure(footer_note, 1) <= card_w);

        // Managed sector verdicts must fit within sector tile inner width (iw = 292, badge text available <= 266)
        // The three words are the ones the tiles paint; a gate left naming a
        // retired verdict measures nothing, so `the_keystore_and_entropy_...`
        // tests re-read them from the painter and check they appear here.
        let verdicts = [
            b"opens back the same".as_slice(),
            b"not checked yet".as_slice(),
            b"no record".as_slice(),
        ];
        for v in verdicts {
            let vw = font::measure_bold(v, 1);
            assert!(vw <= 266, "verdict {:?} exceeds badge text available space", v);
            let badge_w = vw + 34;
            assert!(badge_w <= 292, "badge width exceeds tile inner width");
        }

        // Status frame metric values must fit within metric row half badge capacity
        let metric_values = [
            b"written and validated".as_slice(),
            b"copy does not match".as_slice(),
            b"copy came back short".as_slice(),
            b"could not be read".as_slice(),
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
        // WHY measured as the note the modal actually paints, and over any
        // phrasing of the denial: the sentence has since been re-worded for
        // the person on the phone, and a gate quoting one sentence -- down to
        // the platform's own name for its keys -- failed a rename that kept
        // every claim true and asked for a word the glass may not use.
        let open = modal
            .find(r#"b"The PIN opens this session"#)
            .expect("the modal still explains what the PIN does and does not do");
        let note = literal_at(modal, open);
        assert!(
            (note.contains("no ") || note.contains("not ")) && note.contains("key"),
            "the PIN note {note:?} no longer tells the reader the session opens \
             without releasing a key"
        );
        assert!(
            !note.contains("keystore"),
            "the PIN note {note:?} answers with the platform's internal name for \
             its keys instead of the device the reader is holding"
        );
    }

    /// The inner width a `metric_row` caption is drawn into: the card minus
    /// its two paddings, matching `INNER_W` in the painter.
    const CAPTION_W: usize = 624;

    /// How many `metric_row` call sites this test expects to find at most.
    const MAX_ROWS: usize = 32;

    /// Fails unless `words` reads as an answer to a person rather than as a
    /// note to the next engineer: no syscall signature, no device node, no
    /// privilege ring, no acronym, no trailing developer shrug.
    ///
    /// WHY a denylist and not an allowlist of sentences: the point is not that
    /// the app says one particular thing, it is that the caption stops
    /// requiring the reader to know the platform's internals to use it.
    fn assert_plain_words(words: &str) {
        for jargon in [
            "keystore_derive",
            "derive(",
            "_read(",
            "Ring 3",
            "/dev/",
            "IVs",
            "IV ",
            "syscall",
            "probe;",
            "not tested",
        ] {
            assert!(
                !words.contains(jargon),
                "the screen still quotes the mechanism {jargon:?} instead of answering: {words:?}"
            );
        }
    }

    /// Reads the contents of the byte literal whose opening quote pair begins
    /// at `open`, so a caption can be tested as the painter wrote it.
    fn literal_at(src: &str, open: usize) -> &str {
        let body = &src[open + 2..];
        let close = body.find('"').expect("the byte literal closes");
        &body[..close]
    }

    /// Collects (label, caption) for every `metric_row` call site in the
    /// painter, as the strings actually written there.
    ///
    /// WHY it reads the painter instead of a list kept in this file: a gate
    /// that names its own expected strings keeps passing after production
    /// moves on, which is how a screen ends up quoting a syscall again.
    fn metric_row_captions<'a>(
        main: &'a str,
        out: &mut [Option<(&'a str, &'a str)>; MAX_ROWS],
    ) -> usize {
        let mut n = 0;
        let mut at = 0;
        while let Some(rel) = main[at..].find("if let Some(c) = metric_row(") {
            let start = at + rel + "if let Some(c) = metric_row(".len();
            let rest = &main[start..];
            // A call written on one line hands its caption to a variable, so
            // there is nothing to read here; skipping it beats borrowing the
            // argument list of the next row.
            if !rest.starts_with('\n') {
                at = start;
                continue;
            }
            // Stop at the next call site so the segment cannot run past this
            // row's own argument list.
            let seg = &rest[..rest.find("metric_row(").unwrap_or(rest.len())];
            if let Some(close) = seg.find("\n        ) {") {
                let args = &seg[..close];
                if let (Some(first), Some(last)) = (args.find("b\""), args.rfind("b\"")) {
                    if n < out.len() {
                        out[n] = Some((literal_at(args, first), literal_at(args, last)));
                    }
                    n += 1;
                }
            }
            at = start;
        }
        n
    }

    /// The key-slot and entropy captions must tell a person what the answer
    /// means for them, in both views that draw them: the home card and the
    /// status-frame view are separate call sites, and retitling one leaves the
    /// other quoting the kernel at the reader.
    #[test]
    fn the_keystore_and_entropy_captions_answer_a_person() {
        let main = include_str!("main.rs");
        let mut rows = [None; MAX_ROWS];
        let found = metric_row_captions(main, &mut rows);
        assert!(found >= 8, "the metric rows vanished from the painter: {found}");

        // What each row owes the reader: the root and storage keys name what
        // they unlock and where the key that unlocks it lives, the entropy row
        // names what the bytes are for and what happens when they are missing.
        // WHY the place is "this device" and not the platform's own name for
        // its key store: the reader is deciding whether to trust the phone in
        // their hand, and a caption that answers with an internal component
        // leaves them holding the question.
        let owed: [(&str, &[&str]); 3] = [
            ("Root key", &["unlocks", "this device"]),
            ("Storage key", &["unlocks", "this device"]),
            ("Random source", &["random", "without it"]),
        ];
        for (label, markers) in owed {
            let mut seen = 0usize;
            for row in rows.iter().flatten() {
                if row.0 != label {
                    continue;
                }
                let caption = row.1;
                seen += 1;
                assert_plain_words(caption);
                for marker in markers {
                    assert!(
                        caption.contains(marker),
                        "the {label:?} caption {caption:?} never says what the answer means \
                         for the reader (it must mention {marker:?})"
                    );
                }
                assert!(
                    interos_std::font::measure(caption.as_bytes(), 1) <= CAPTION_W,
                    "the {label:?} caption is wider than the card: {caption:?}",
                );
            }
            assert!(
                seen >= 2,
                "only {seen} view(s) draw the {label:?} row; both the home card and the \
                 status-frame view must answer for it"
            );
        }
    }

    /// Every caption the painter passes to `metric_row`, and every managed
    /// sector verdict, must read as an answer: no syscall signature, no device
    /// node, no privilege ring, no engineer's note left on the glass.
    #[test]
    fn no_caption_on_glass_quotes_a_mechanism() {
        let main = include_str!("main.rs");
        let mut rows = [None; MAX_ROWS];
        let found = metric_row_captions(main, &mut rows);
        let call_sites = main.matches("if let Some(c) = metric_row(\n").count();
        assert_eq!(
            found, call_sites,
            "the scan read {found} of {call_sites} metric rows, so a caption escaped the gate"
        );
        for row in rows.iter().flatten() {
            assert_plain_words(row.1);
        }
    }

    /// The sector tiles paint one of three verdicts. A record that has no
    /// round-trip evidence either way used to print the engineer's own note,
    /// "not tested", next to two real verdicts; it now says what the reader
    /// can conclude, and it keeps the neutral ink -- an absent proof is never
    /// a success.
    #[test]
    fn the_sector_verdicts_report_evidence_and_never_a_dev_note() {
        let main = include_str!("main.rs");
        let anchor = "match vault.sectors[index].map(|s| s.authenticated)";
        let start = main
            .find(anchor)
            .expect("the sector tiles still decide their verdict from the round-trip flag");
        let rest = &main[start..];
        let end = rest
            .find("\n            };")
            .expect("the sector verdict match closes");
        let block = &rest[..end];

        let mut verdicts = [""; 4];
        let mut inks = [""; 4];
        let mut n = 0;
        let mut at = 0;
        while let Some(rel) = block[at..].find("b\"") {
            let open = at + rel;
            let word = literal_at(&block[open..], 0);
            // The whole arm line, so the ink the painter chose for this
            // verdict can be read together with the verdict itself.
            let line_start = block[..open].rfind('\n').map_or(0, |i| i + 1);
            let line_end = open + block[open..].find('\n').unwrap_or(block.len() - open);
            let arm = block[line_start..line_end].trim();
            if n < verdicts.len() {
                verdicts[n] = word;
                inks[n] = arm;
            }
            n += 1;
            at = open + word.len() + 3;
        }
        assert_eq!(n, 3, "the sector verdict is no longer a three-way match: {n} arms");
        // WHY this no longer quotes the sentence, and now forbids a word in it:
        // the gate used to require "round-trip verified", so the screen could
        // only ever answer with the engineer's name for the check. What the
        // arm owes the reader is a stated result of the sector's own record,
        // reached from the state that record is in.
        assert!(inks[0].contains("Some(true)"), "the arm order changed: {}", inks[0]);
        assert!(
            !verdicts[0].is_empty() && verdicts[0].len() >= 8,
            "the verified verdict must read as a result, got {:?}",
            verdicts[0]
        );
        assert!(
            !verdicts[0].contains("round-trip"),
            "the success verdict {:?} still names the check instead \
             of what the check showed",
            verdicts[0]
        );

        // The middle arm is the one that leaked the developer's note.
        assert!(inks[1].contains("Some(false)"), "the arm order changed: {}", inks[1]);
        assert_ne!(
            verdicts[1], "not tested",
            "a tile still prints the engineer's note instead of the reader's conclusion"
        );
        assert!(
            !verdicts[1].is_empty() && verdicts[1].len() >= 8,
            "the no-evidence verdict must say so in words a reader can use, got {:?}",
            verdicts[1]
        );
        assert!(
            inks[1].contains("THEME.on_surface_variant") && !inks[1].contains("THEME.success"),
            "an absent proof may never wear success ink: {}",
            inks[1]
        );
        // Nor may it be confused with the third arm, the missing record.
        assert_ne!(verdicts[1], verdicts[2], "the two no-evidence states read identically");
        assert!(
            inks[2].contains("THEME.warning"),
            "a sector with no record is not a neutral state: {}",
            inks[2]
        );

        // The width gate in this file must be measuring the verdicts the app
        // actually paints; a gate left naming a retired string proves nothing.
        let lib = include_str!("lib.rs");
        for v in verdicts {
            assert!(
                lib.contains(v),
                "the badge-width gate no longer measures the painted verdict {v:?}"
            );
            assert!(
                interos_std::font::measure_bold(v.as_bytes(), 1) + 34 <= 292,
                "the verdict {v:?} no longer fits its tile"
            );
        }
    }
}

