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
}
