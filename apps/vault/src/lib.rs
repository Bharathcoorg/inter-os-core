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
