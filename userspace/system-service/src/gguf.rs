//! Lightweight, zero-allocation GGUF metadata parser for InterOS AI service (`aid`).
//!
//! Strictly `#![no_std]` and zero heap allocation.
//! Parses GGUF file headers (magic `0x46554747`, version, tensor count, kv count)
//! from raw byte slices and reads model headers from `/system/models/` or `/data/models/`.

use interos_std as std;

/// GGUF file format magic number: `0x46554747` (ASCII `b"GGUF"` in little-endian).
pub const GGUF_MAGIC: u32 = 0x46554747;

/// Error types for GGUF parsing and model probing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GgufError {
    /// Provided buffer is smaller than the minimum 24-byte header prefix.
    BufferTooShort,
    /// Magic number did not match `0x46554747` (`b"GGUF"`).
    InvalidMagic,
    /// Header version is unsupported (only versions 1, 2, and 3 are supported).
    UnsupportedVersion(u32),
    /// File was not found on the filesystem.
    FileNotFound,
    /// Reading from the filesystem failed.
    ReadFailed,
    /// No valid GGUF models were located in `/system/models/` or `/data/models/`.
    NoModelsFound,
}

/// GGUF file header prefix representation (24 bytes).
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GgufHeader {
    /// File format magic: must equal `GGUF_MAGIC` (`0x46554747`).
    pub magic: u32,
    /// Format version (e.g., 2 or 3).
    pub version: u32,
    /// Number of tensors contained in the model file.
    pub tensor_count: u64,
    /// Number of key-value metadata pairs stored in the header.
    pub kv_count: u64,
}

impl GgufHeader {
    /// Minimum byte length of the GGUF header prefix:
    /// - magic: 4 bytes
    /// - version: 4 bytes
    /// - tensor_count: 8 bytes
    /// - kv_count: 8 bytes
    pub const HEADER_SIZE: usize = 24;

    /// Parses a GGUF header prefix from a contiguous byte buffer.
    ///
    /// Requires zero heap allocations.
    pub fn parse(buf: &[u8]) -> Result<Self, GgufError> {
        if buf.len() < Self::HEADER_SIZE {
            return Err(GgufError::BufferTooShort);
        }

        let magic = u32::from_le_bytes([buf[0], buf[1], buf[2], buf[3]]);
        if magic != GGUF_MAGIC {
            return Err(GgufError::InvalidMagic);
        }

        let version = u32::from_le_bytes([buf[4], buf[5], buf[6], buf[7]]);
        if version == 0 || version > 3 {
            return Err(GgufError::UnsupportedVersion(version));
        }

        let tensor_count = u64::from_le_bytes([
            buf[8], buf[9], buf[10], buf[11], buf[12], buf[13], buf[14], buf[15],
        ]);

        let kv_count = u64::from_le_bytes([
            buf[16], buf[17], buf[18], buf[19], buf[20], buf[21], buf[22], buf[23],
        ]);

        Ok(Self {
            magic,
            version,
            tensor_count,
            kv_count,
        })
    }

    /// Serializes the 24-byte GGUF header into a byte slice.
    pub fn write_to(&self, out: &mut [u8]) -> Result<usize, GgufError> {
        if out.len() < Self::HEADER_SIZE {
            return Err(GgufError::BufferTooShort);
        }
        out[0..4].copy_from_slice(&self.magic.to_le_bytes());
        out[4..8].copy_from_slice(&self.version.to_le_bytes());
        out[8..16].copy_from_slice(&self.tensor_count.to_le_bytes());
        out[16..24].copy_from_slice(&self.kv_count.to_le_bytes());
        Ok(Self::HEADER_SIZE)
    }
}

/// GGUF metadata value types as defined in the official specification.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum GgufMetadataValueType {
    Uint8 = 0,
    Int8 = 1,
    Uint16 = 2,
    Int16 = 3,
    Uint32 = 4,
    Int32 = 5,
    Float32 = 6,
    Bool = 7,
    String = 8,
    Array = 9,
    Uint64 = 10,
    Int64 = 11,
    Float64 = 12,
}

impl GgufMetadataValueType {
    pub fn from_u32(val: u32) -> Option<Self> {
        match val {
            0 => Some(Self::Uint8),
            1 => Some(Self::Int8),
            2 => Some(Self::Uint16),
            3 => Some(Self::Int16),
            4 => Some(Self::Uint32),
            5 => Some(Self::Int32),
            6 => Some(Self::Float32),
            7 => Some(Self::Bool),
            8 => Some(Self::String),
            9 => Some(Self::Array),
            10 => Some(Self::Uint64),
            11 => Some(Self::Int64),
            12 => Some(Self::Float64),
            _ => None,
        }
    }
}

/// Zero-allocation helper to look up a string metadata value by key in a GGUF buffer.
///
/// Returns the slice of UTF-8 bytes for the string value if found.
pub fn find_metadata_string<'a>(buf: &'a [u8], target_key: &[u8]) -> Option<&'a [u8]> {
    let header = GgufHeader::parse(buf).ok()?;
    let mut offset = GgufHeader::HEADER_SIZE;

    for _ in 0..header.kv_count {
        if offset + 8 > buf.len() {
            return None;
        }
        let key_len = u64::from_le_bytes([
            buf[offset], buf[offset + 1], buf[offset + 2], buf[offset + 3],
            buf[offset + 4], buf[offset + 5], buf[offset + 6], buf[offset + 7],
        ]) as usize;
        offset += 8;

        if offset + key_len + 4 > buf.len() {
            return None;
        }
        let key = &buf[offset..offset + key_len];
        offset += key_len;

        let val_type = u32::from_le_bytes([
            buf[offset], buf[offset + 1], buf[offset + 2], buf[offset + 3],
        ]);
        offset += 4;

        match val_type {
            8 => {
                // String type
                if offset + 8 > buf.len() {
                    return None;
                }
                let str_len = u64::from_le_bytes([
                    buf[offset], buf[offset + 1], buf[offset + 2], buf[offset + 3],
                    buf[offset + 4], buf[offset + 5], buf[offset + 6], buf[offset + 7],
                ]) as usize;
                offset += 8;
                if offset + str_len > buf.len() {
                    return None;
                }
                let str_val = &buf[offset..offset + str_len];
                offset += str_len;
                if key == target_key {
                    return Some(str_val);
                }
            }
            0 | 1 | 7 => offset += 1, // u8, i8, bool
            2 | 3 => offset += 2,     // u16, i16
            4 | 5 | 6 => offset += 4, // u32, i32, f32
            10 | 11 | 12 => offset += 8, // u64, i64, f64
            _ => return None, // Unsupported or complex types (like nested array) break scanning
        }
    }

    None
}

/// Reads and parses the GGUF header from a specific filesystem path.
///
/// Follows strict `#![no_std]` and zero-heap allocation principles.
pub fn read_gguf_header(path: &str) -> Result<GgufHeader, GgufError> {
    let fd = std::fs_open(path, std::FS_READ).map_err(|_| GgufError::FileNotFound)?;
    let mut buf = [0u8; GgufHeader::HEADER_SIZE];
    let read_res = std::fs_read(fd, &mut buf);
    let _ = std::fs_close(fd);

    let n = read_res.map_err(|_| GgufError::ReadFailed)?;
    if n < GgufHeader::HEADER_SIZE {
        return Err(GgufError::BufferTooShort);
    }
    GgufHeader::parse(&buf)
}

/// Candidate model paths checked in priority order.
pub const MODEL_CANDIDATE_PATHS: &[&str] = &[
    "/system/models/embeddinggemma-2.gguf",
    "/system/models/embeddinggemma-2-740m.gguf",
    "/system/models/model.gguf",
    "/data/models/embeddinggemma-2.gguf",
    "/data/models/model.gguf",
];

/// Probes standard model directories (`/system/models/` and `/data/models/`)
/// for valid GGUF model headers.
///
/// Respects the Platform Truth Contract: returns `Err(GgufError::NoModelsFound)`
/// if physical model files are absent.
pub fn probe_gguf_header() -> Result<(GgufHeader, &'static str), GgufError> {
    for &path in MODEL_CANDIDATE_PATHS {
        if let Ok(hdr) = read_gguf_header(path) {
            return Ok((hdr, path));
        }
    }

    // Directory listing probe
    let mut dir_buf = [0u8; 512];
    for &dir in &["/system/models", "/data/models"] {
        if let Ok(n) = std::fs_list(dir, &mut dir_buf) {
            let payload = &dir_buf[..n.min(dir_buf.len())];
            for line in payload.split(|&b| b == b'\n') {
                if line.is_empty() {
                    continue;
                }
                let end = line.iter().position(|&b| b == b'|').unwrap_or(line.len());
                let name = &line[..end];
                if name.len() >= 5 && name.ends_with(b".gguf") {
                    let mut path_buf = [0u8; 128];
                    let dir_bytes = dir.as_bytes();
                    let total_len = dir_bytes.len() + 1 + name.len();
                    if total_len < path_buf.len() {
                        path_buf[..dir_bytes.len()].copy_from_slice(dir_bytes);
                        path_buf[dir_bytes.len()] = b'/';
                        path_buf[dir_bytes.len() + 1..total_len].copy_from_slice(name);
                        if let Ok(path_str) = core::str::from_utf8(&path_buf[..total_len]) {
                            if let Ok(hdr) = read_gguf_header(path_str) {
                                return Ok((hdr, dir));
                            }
                        }
                    }
                }
            }
        }
    }

    Err(GgufError::NoModelsFound)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_gguf_header_parse_valid() {
        let mut buf = [0u8; 24];
        buf[0..4].copy_from_slice(&GGUF_MAGIC.to_le_bytes());
        buf[4..8].copy_from_slice(&3u32.to_le_bytes()); // version 3
        buf[8..16].copy_from_slice(&144u64.to_le_bytes()); // 144 tensors
        buf[16..24].copy_from_slice(&32u64.to_le_bytes()); // 32 kv pairs

        let header = GgufHeader::parse(&buf).expect("should parse valid header");
        assert_eq!(header.magic, GGUF_MAGIC);
        assert_eq!(header.version, 3);
        assert_eq!(header.tensor_count, 144);
        assert_eq!(header.kv_count, 32);
    }

    #[test]
    fn test_gguf_header_invalid_magic() {
        let mut buf = [0u8; 24];
        buf[0..4].copy_from_slice(&0x12345678u32.to_le_bytes());
        buf[4..8].copy_from_slice(&2u32.to_le_bytes());
        assert_eq!(GgufHeader::parse(&buf), Err(GgufError::InvalidMagic));
    }

    #[test]
    fn test_gguf_header_buffer_too_short() {
        let buf = [0u8; 20];
        assert_eq!(GgufHeader::parse(&buf), Err(GgufError::BufferTooShort));
    }

    #[test]
    fn test_gguf_header_unsupported_version() {
        let mut buf = [0u8; 24];
        buf[0..4].copy_from_slice(&GGUF_MAGIC.to_le_bytes());
        buf[4..8].copy_from_slice(&99u32.to_le_bytes());
        assert_eq!(
            GgufHeader::parse(&buf),
            Err(GgufError::UnsupportedVersion(99))
        );
    }

    #[test]
    fn test_gguf_header_serialization_roundtrip() {
        let original = GgufHeader {
            magic: GGUF_MAGIC,
            version: 2,
            tensor_count: 280,
            kv_count: 18,
        };
        let mut buf = [0u8; 24];
        assert_eq!(original.write_to(&mut buf), Ok(24));
        let parsed = GgufHeader::parse(&buf).expect("roundtrip parse");
        assert_eq!(original, parsed);
    }

    #[test]
    fn test_find_metadata_string() {
        let mut buf = [0u8; 128];
        let header = GgufHeader {
            magic: GGUF_MAGIC,
            version: 3,
            tensor_count: 1,
            kv_count: 1,
        };
        header.write_to(&mut buf).unwrap();

        // KV 1: key "general.name", value type string (8), value "embeddinggemma"
        let key = b"general.name";
        let val = b"embeddinggemma";

        let mut offset = 24;
        buf[offset..offset + 8].copy_from_slice(&(key.len() as u64).to_le_bytes());
        offset += 8;
        buf[offset..offset + key.len()].copy_from_slice(key);
        offset += key.len();

        buf[offset..offset + 4].copy_from_slice(&8u32.to_le_bytes()); // string type
        offset += 4;
        buf[offset..offset + 8].copy_from_slice(&(val.len() as u64).to_le_bytes());
        offset += 8;
        buf[offset..offset + val.len()].copy_from_slice(val);

        let found = find_metadata_string(&buf, b"general.name");
        assert_eq!(found, Some(&val[..]));

        let not_found = find_metadata_string(&buf, b"general.architecture");
        assert_eq!(not_found, None);
    }
}
