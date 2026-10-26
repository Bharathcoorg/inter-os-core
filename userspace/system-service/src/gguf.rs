//! Lightweight, zero-allocation GGUF metadata & tensor parser for InterOS AI service (`aid`).
//!
//! Strictly `#![no_std]` and zero heap allocation.
//! Parses GGUF file headers (magic `0x46554747`, version, tensor count, kv count)
//! from raw byte slices and reads model headers & tensor descriptors from `/system/models/` or `/data/models/`.

use interos_std as std;

/// GGUF file format magic number: `0x46554747` (ASCII `b"GGUF"` in little-endian).
pub const GGUF_MAGIC: u32 = 0x46554747;

/// Canonical on-device path for EmbeddingGemma 2 weights in `/data/models`.
pub const PATH_EMBEDDING_MODEL_DATA: &str = "/data/models/embeddinggemma-2.gguf";
/// Fallback system path for EmbeddingGemma 2 weights in `/system/models`.
pub const PATH_EMBEDDING_MODEL_SYSTEM: &str = "/system/models/embeddinggemma-2.gguf";

/// Canonical on-device path for Gemma 4 2B generative weights in `/data/models`.
pub const PATH_GENERATIVE_MODEL_DATA: &str = "/data/models/gemma4-2b.gguf";
/// Fallback system path for Gemma 4 2B generative weights in `/system/models`.
pub const PATH_GENERATIVE_MODEL_SYSTEM: &str = "/system/models/gemma4-2b.gguf";

/// Maximum length for a tensor name in zero-allocation buffer.
pub const MAX_TENSOR_NAME_LEN: usize = 64;

/// Error types for GGUF parsing and model probing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GgufError {
    /// Provided buffer is smaller than the minimum 24-byte header prefix or required record.
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
    /// Metadata corruption or unparseable value type.
    InvalidMetadata,
    /// Arithmetic overflow in checked dimension or offset calculations.
    ArithmeticOverflow,
    /// Tensor descriptor or block data structure corruption.
    CorruptedTensor,
}

/// Bounded sequential GGUF data stream reader (zero heap allocation).
///
/// Supports reading from either an in-memory byte slice or a streaming file descriptor
/// with sequential chunk buffering, enabling bounded parsing of arbitrarily large models
/// without consuming unbounded memory.
pub enum GgufStream<'a> {
    Slice { data: &'a [u8], pos: usize },
    File { fd: u32, pos: usize },
}

impl<'a> GgufStream<'a> {
    pub fn from_slice(data: &'a [u8]) -> Self {
        Self::Slice { data, pos: 0 }
    }

    pub fn from_file(fd: u32) -> Self {
        Self::File { fd, pos: 0 }
    }

    pub fn position(&self) -> usize {
        match self {
            Self::Slice { pos, .. } => *pos,
            Self::File { pos, .. } => *pos,
        }
    }

    pub fn read_exact(&mut self, out: &mut [u8]) -> Result<(), GgufError> {
        let req_len = out.len();
        if req_len == 0 {
            return Ok(());
        }
        match self {
            Self::Slice { data, pos } => {
                let next_pos = pos.checked_add(req_len).ok_or(GgufError::BufferTooShort)?;
                if next_pos > data.len() {
                    return Err(GgufError::BufferTooShort);
                }
                out.copy_from_slice(&data[*pos..next_pos]);
                *pos = next_pos;
                Ok(())
            }
            Self::File { fd, pos } => {
                let mut filled = 0usize;
                while filled < req_len {
                    let n = std::fs_read(*fd, &mut out[filled..]).map_err(|_| GgufError::ReadFailed)?;
                    if n == 0 {
                        return Err(GgufError::BufferTooShort);
                    }
                    filled = filled.checked_add(n).ok_or(GgufError::ArithmeticOverflow)?;
                }
                *pos = pos.checked_add(req_len).ok_or(GgufError::ArithmeticOverflow)?;
                Ok(())
            }
        }
    }

    pub fn skip(&mut self, mut count: usize) -> Result<(), GgufError> {
        if count == 0 {
            return Ok(());
        }
        match self {
            Self::Slice { data, pos } => {
                let next_pos = pos.checked_add(count).ok_or(GgufError::BufferTooShort)?;
                if next_pos > data.len() {
                    return Err(GgufError::BufferTooShort);
                }
                *pos = next_pos;
                Ok(())
            }
            Self::File { fd, pos } => {
                let mut scratch = [0u8; 512];
                while count > 0 {
                    let chunk = count.min(scratch.len());
                    let n = std::fs_read(*fd, &mut scratch[..chunk]).map_err(|_| GgufError::ReadFailed)?;
                    if n == 0 {
                        return Err(GgufError::BufferTooShort);
                    }
                    count = count.checked_sub(n).ok_or(GgufError::ArithmeticOverflow)?;
                    *pos = pos.checked_add(n).ok_or(GgufError::ArithmeticOverflow)?;
                }
                Ok(())
            }
        }
    }

    pub fn read_u32(&mut self) -> Result<u32, GgufError> {
        let mut b = [0u8; 4];
        self.read_exact(&mut b)?;
        Ok(u32::from_le_bytes(b))
    }

    pub fn read_u64(&mut self) -> Result<u64, GgufError> {
        let mut b = [0u8; 8];
        self.read_exact(&mut b)?;
        Ok(u64::from_le_bytes(b))
    }
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

    /// Parses a GGUF header prefix from a streaming reader.
    pub fn parse_stream(stream: &mut GgufStream) -> Result<Self, GgufError> {
        let mut buf = [0u8; Self::HEADER_SIZE];
        stream.read_exact(&mut buf)?;
        Self::parse(&buf)
    }

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

/// GGUF tensor descriptor (zero heap allocation).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GgufTensorInfo {
    /// Byte length of tensor name in `name`.
    pub name_len: usize,
    /// ASCII / UTF-8 tensor name bytes (e.g. `token_embd.weight`).
    pub name: [u8; MAX_TENSOR_NAME_LEN],
    /// Number of dimensions (1 to 4).
    pub n_dimensions: u32,
    /// Dimensions (e.g. [768, 256000, 0, 0]).
    pub dimensions: [u64; 4],
    /// GGML data type identifier (0 = F32, 1 = F16, 2 = Q4_0, 8 = Q8_0, etc.).
    pub tensor_type: u32,
    /// Offset relative to the start of binary tensor data.
    pub offset: u64,
}

impl GgufTensorInfo {
    pub const fn empty() -> Self {
        Self {
            name_len: 0,
            name: [0u8; MAX_TENSOR_NAME_LEN],
            n_dimensions: 0,
            dimensions: [0u64; 4],
            tensor_type: 0,
            offset: 0,
        }
    }

    pub fn name_as_bytes(&self) -> &[u8] {
        &self.name[..self.name_len.min(MAX_TENSOR_NAME_LEN)]
    }

    pub fn name_as_str(&self) -> Result<&str, core::str::Utf8Error> {
        core::str::from_utf8(self.name_as_bytes())
    }

    pub fn matches_name(&self, expected: &[u8]) -> bool {
        self.name_as_bytes() == expected
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

/// Zero-allocation helper to skip a single metadata value in a GGUF byte buffer.
pub fn skip_metadata_value(buf: &[u8], offset: &mut usize, val_type: u32) -> Result<(), GgufError> {
    match val_type {
        0 | 1 | 7 => {
            // Uint8, Int8, Bool: 1 byte
            if *offset + 1 > buf.len() {
                return Err(GgufError::BufferTooShort);
            }
            *offset += 1;
        }
        2 | 3 => {
            // Uint16, Int16: 2 bytes
            if *offset + 2 > buf.len() {
                return Err(GgufError::BufferTooShort);
            }
            *offset += 2;
        }
        4 | 5 | 6 => {
            // Uint32, Int32, Float32: 4 bytes
            if *offset + 4 > buf.len() {
                return Err(GgufError::BufferTooShort);
            }
            *offset += 4;
        }
        10 | 11 | 12 => {
            // Uint64, Int64, Float64: 8 bytes
            if *offset + 8 > buf.len() {
                return Err(GgufError::BufferTooShort);
            }
            *offset += 8;
        }
        8 => {
            // String: 8 bytes length + string bytes
            if *offset + 8 > buf.len() {
                return Err(GgufError::BufferTooShort);
            }
            let str_len = u64::from_le_bytes([
                buf[*offset], buf[*offset + 1], buf[*offset + 2], buf[*offset + 3],
                buf[*offset + 4], buf[*offset + 5], buf[*offset + 6], buf[*offset + 7],
            ]) as usize;
            *offset += 8;
            if *offset + str_len > buf.len() {
                return Err(GgufError::BufferTooShort);
            }
            *offset += str_len;
        }
        9 => {
            // Array: 4 bytes element_type + 8 bytes element_count + elements
            if *offset + 12 > buf.len() {
                return Err(GgufError::BufferTooShort);
            }
            let elem_type = u32::from_le_bytes([
                buf[*offset], buf[*offset + 1], buf[*offset + 2], buf[*offset + 3],
            ]);
            let elem_count = u64::from_le_bytes([
                buf[*offset + 4], buf[*offset + 5], buf[*offset + 6], buf[*offset + 7],
                buf[*offset + 8], buf[*offset + 9], buf[*offset + 10], buf[*offset + 11],
            ]) as usize;
            *offset += 12;

            match elem_type {
                0 | 1 | 7 => {
                    let total = elem_count;
                    if *offset + total > buf.len() {
                        return Err(GgufError::BufferTooShort);
                    }
                    *offset += total;
                }
                2 | 3 => {
                    let total = elem_count.checked_mul(2).ok_or(GgufError::InvalidMetadata)?;
                    if *offset + total > buf.len() {
                        return Err(GgufError::BufferTooShort);
                    }
                    *offset += total;
                }
                4 | 5 | 6 => {
                    let total = elem_count.checked_mul(4).ok_or(GgufError::InvalidMetadata)?;
                    if *offset + total > buf.len() {
                        return Err(GgufError::BufferTooShort);
                    }
                    *offset += total;
                }
                10 | 11 | 12 => {
                    let total = elem_count.checked_mul(8).ok_or(GgufError::InvalidMetadata)?;
                    if *offset + total > buf.len() {
                        return Err(GgufError::BufferTooShort);
                    }
                    *offset += total;
                }
                8 => {
                    // Array of strings
                    for _ in 0..elem_count {
                        if *offset + 8 > buf.len() {
                            return Err(GgufError::BufferTooShort);
                        }
                        let s_len = u64::from_le_bytes([
                            buf[*offset], buf[*offset + 1], buf[*offset + 2], buf[*offset + 3],
                            buf[*offset + 4], buf[*offset + 5], buf[*offset + 6], buf[*offset + 7],
                        ]) as usize;
                        *offset += 8;
                        if *offset + s_len > buf.len() {
                            return Err(GgufError::BufferTooShort);
                        }
                        *offset += s_len;
                    }
                }
                _ => return Err(GgufError::InvalidMetadata),
            }
        }
        _ => return Err(GgufError::InvalidMetadata),
    }
    Ok(())
}

/// Zero-allocation helper to skip all key-value pairs in a GGUF buffer.
pub fn skip_all_kv_pairs(buf: &[u8], offset: &mut usize, kv_count: u64) -> Result<(), GgufError> {
    for _ in 0..kv_count {
        if *offset + 8 > buf.len() {
            return Err(GgufError::BufferTooShort);
        }
        let key_len = u64::from_le_bytes([
            buf[*offset], buf[*offset + 1], buf[*offset + 2], buf[*offset + 3],
            buf[*offset + 4], buf[*offset + 5], buf[*offset + 6], buf[*offset + 7],
        ]) as usize;
        *offset += 8;

        if *offset + key_len + 4 > buf.len() {
            return Err(GgufError::BufferTooShort);
        }
        *offset += key_len;

        let val_type = u32::from_le_bytes([
            buf[*offset], buf[*offset + 1], buf[*offset + 2], buf[*offset + 3],
        ]);
        *offset += 4;

        skip_metadata_value(buf, offset, val_type)?;
    }
    Ok(())
}

/// Parses a single GGUF tensor info entry from a byte buffer at `offset`, advancing `offset`.
pub fn parse_tensor_info(buf: &[u8], offset: &mut usize) -> Result<GgufTensorInfo, GgufError> {
    if *offset + 8 > buf.len() {
        return Err(GgufError::BufferTooShort);
    }
    let name_len = u64::from_le_bytes([
        buf[*offset], buf[*offset + 1], buf[*offset + 2], buf[*offset + 3],
        buf[*offset + 4], buf[*offset + 5], buf[*offset + 6], buf[*offset + 7],
    ]) as usize;
    *offset += 8;

    if *offset + name_len + 4 > buf.len() {
        return Err(GgufError::BufferTooShort);
    }
    let mut info = GgufTensorInfo::empty();
    let copy_len = name_len.min(MAX_TENSOR_NAME_LEN);
    info.name[..copy_len].copy_from_slice(&buf[*offset..*offset + copy_len]);
    info.name_len = name_len;
    *offset += name_len;

    let n_dims = u32::from_le_bytes([
        buf[*offset], buf[*offset + 1], buf[*offset + 2], buf[*offset + 3],
    ]);
    *offset += 4;
    info.n_dimensions = n_dims;

    let dims_bytes_len = (n_dims as usize).checked_mul(8).ok_or(GgufError::InvalidMetadata)?;
    if *offset + dims_bytes_len + 4 + 8 > buf.len() {
        return Err(GgufError::BufferTooShort);
    }

    for d in 0..(n_dims as usize).min(4) {
        let dim_val = u64::from_le_bytes([
            buf[*offset + d * 8], buf[*offset + d * 8 + 1],
            buf[*offset + d * 8 + 2], buf[*offset + d * 8 + 3],
            buf[*offset + d * 8 + 4], buf[*offset + d * 8 + 5],
            buf[*offset + d * 8 + 6], buf[*offset + d * 8 + 7],
        ]);
        info.dimensions[d] = dim_val;
    }
    *offset += dims_bytes_len;

    info.tensor_type = u32::from_le_bytes([
        buf[*offset], buf[*offset + 1], buf[*offset + 2], buf[*offset + 3],
    ]);
    *offset += 4;

    info.offset = u64::from_le_bytes([
        buf[*offset], buf[*offset + 1], buf[*offset + 2], buf[*offset + 3],
        buf[*offset + 4], buf[*offset + 5], buf[*offset + 6], buf[*offset + 7],
    ]);
    *offset += 8;

    Ok(info)
}

/// Zero-allocation helper to skip a single metadata value in a streaming GGUF reader.
pub fn skip_metadata_value_stream(stream: &mut GgufStream, val_type: u32) -> Result<(), GgufError> {
    match val_type {
        0 | 1 | 7 => stream.skip(1), // Uint8, Int8, Bool: 1 byte
        2 | 3 => stream.skip(2),     // Uint16, Int16: 2 bytes
        4 | 5 | 6 => stream.skip(4), // Uint32, Int32, Float32: 4 bytes
        10 | 11 | 12 => stream.skip(8), // Uint64, Int64, Float64: 8 bytes
        8 => {
            // String: 8 bytes length + string bytes
            let str_len = stream.read_u64()? as usize;
            stream.skip(str_len)
        }
        9 => {
            // Array: 4 bytes element_type + 8 bytes element_count + elements
            let elem_type = stream.read_u32()?;
            let elem_count = stream.read_u64()? as usize;
            match elem_type {
                0 | 1 | 7 => stream.skip(elem_count),
                2 | 3 => {
                    let total = elem_count.checked_mul(2).ok_or(GgufError::ArithmeticOverflow)?;
                    stream.skip(total)
                }
                4 | 5 | 6 => {
                    let total = elem_count.checked_mul(4).ok_or(GgufError::ArithmeticOverflow)?;
                    stream.skip(total)
                }
                10 | 11 | 12 => {
                    let total = elem_count.checked_mul(8).ok_or(GgufError::ArithmeticOverflow)?;
                    stream.skip(total)
                }
                8 => {
                    for _ in 0..elem_count {
                        let s_len = stream.read_u64()? as usize;
                        stream.skip(s_len)?;
                    }
                    Ok(())
                }
                _ => Err(GgufError::InvalidMetadata),
            }
        }
        _ => Err(GgufError::InvalidMetadata),
    }
}

/// Zero-allocation helper to skip all key-value pairs in a streaming GGUF reader.
pub fn skip_all_kv_pairs_stream(stream: &mut GgufStream, kv_count: u64) -> Result<(), GgufError> {
    for _ in 0..kv_count {
        let key_len = stream.read_u64()? as usize;
        stream.skip(key_len)?;
        let val_type = stream.read_u32()?;
        skip_metadata_value_stream(stream, val_type)?;
    }
    Ok(())
}

/// Parses a single GGUF tensor info entry from a streaming reader with checked arithmetic.
pub fn parse_tensor_info_stream(stream: &mut GgufStream) -> Result<GgufTensorInfo, GgufError> {
    let name_len = stream.read_u64()? as usize;
    let mut info = GgufTensorInfo::empty();
    info.name_len = name_len;
    let copy_len = name_len.min(MAX_TENSOR_NAME_LEN);
    stream.read_exact(&mut info.name[..copy_len])?;
    if name_len > copy_len {
        let rem = name_len.checked_sub(copy_len).ok_or(GgufError::ArithmeticOverflow)?;
        stream.skip(rem)?;
    }

    let n_dims = stream.read_u32()?;
    if n_dims > 8 {
        return Err(GgufError::InvalidMetadata);
    }
    info.n_dimensions = n_dims;

    for d in 0..n_dims as usize {
        let dim = stream.read_u64()?;
        if d < 4 {
            info.dimensions[d] = dim;
        }
    }

    info.tensor_type = stream.read_u32()?;
    info.offset = stream.read_u64()?;
    Ok(info)
}

/// Scans a streaming GGUF source for a tensor by exact name without heap allocation.
pub fn find_tensor_stream(
    stream: &mut GgufStream,
    target_name: &[u8],
) -> Result<Option<GgufTensorInfo>, GgufError> {
    let header = GgufHeader::parse_stream(stream)?;
    skip_all_kv_pairs_stream(stream, header.kv_count)?;

    for _ in 0..header.tensor_count {
        let tensor = parse_tensor_info_stream(stream)?;
        if tensor.matches_name(target_name) {
            return Ok(Some(tensor));
        }
    }

    Ok(None)
}

/// Calculates the byte size of a tensor from its GGML type and dimensions with checked arithmetic.
pub fn tensor_byte_size(
    tensor_type: u32,
    dims: &[u64; 4],
    n_dims: u32,
) -> Result<usize, GgufError> {
    if n_dims == 0 || n_dims > 4 {
        return Err(GgufError::InvalidMetadata);
    }
    let mut total_elements: usize = 1;
    for d in 0..n_dims as usize {
        let dim = dims[d] as usize;
        total_elements = total_elements
            .checked_mul(dim)
            .ok_or(GgufError::ArithmeticOverflow)?;
    }
    match tensor_type {
        0 => total_elements
            .checked_mul(4)
            .ok_or(GgufError::ArithmeticOverflow), // F32
        1 => total_elements
            .checked_mul(2)
            .ok_or(GgufError::ArithmeticOverflow), // F16
        2 => {
            // Q4_0: 32 elements per 18 bytes
            if total_elements % 32 != 0 {
                return Err(GgufError::CorruptedTensor);
            }
            let blocks = total_elements / 32;
            blocks
                .checked_mul(18)
                .ok_or(GgufError::ArithmeticOverflow)
        }
        8 => {
            // Q8_0: 32 elements per 34 bytes
            if total_elements % 32 != 0 {
                return Err(GgufError::CorruptedTensor);
            }
            let blocks = total_elements / 32;
            blocks
                .checked_mul(34)
                .ok_or(GgufError::ArithmeticOverflow)
        }
        12 => {
            // Q4_K: 256 elements per 144 bytes
            if total_elements % 256 != 0 {
                return Err(GgufError::CorruptedTensor);
            }
            let blocks = total_elements / 256;
            blocks
                .checked_mul(144)
                .ok_or(GgufError::ArithmeticOverflow)
        }
        _ => Err(GgufError::InvalidMetadata),
    }
}

/// Scans a GGUF byte buffer for a tensor by exact name without allocating.
pub fn find_tensor(buf: &[u8], target_name: &[u8]) -> Result<Option<GgufTensorInfo>, GgufError> {
    let mut stream = GgufStream::from_slice(buf);
    find_tensor_stream(&mut stream, target_name)
}

/// Reads a model file sequentially using bounded chunk reads and scans for a specific tensor name.
///
/// Eliminates fixed buffer limits: handles arbitrarily large metadata and tensor tables safely.
pub fn scan_file_for_tensor(
    path: &str,
    target_name: &[u8],
) -> Result<Option<GgufTensorInfo>, GgufError> {
    let fd = std::fs_open(path, std::FS_READ).map_err(|_| GgufError::FileNotFound)?;
    let mut stream = GgufStream::from_file(fd);
    let res = find_tensor_stream(&mut stream, target_name);
    let _ = std::fs_close(fd);
    res
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

        if key == target_key && val_type == 8 {
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
            return Some(&buf[offset..offset + str_len]);
        }

        if skip_metadata_value(buf, &mut offset, val_type).is_err() {
            return None;
        }
    }

    None
}

/// Reads and parses the GGUF header from a specific filesystem path.
///
/// Follows strict `#![no_std]` and zero-heap allocation principles.
pub fn read_gguf_header(path: &str) -> Result<GgufHeader, GgufError> {
    let fd = std::fs_open(path, std::FS_READ).map_err(|_| GgufError::FileNotFound)?;
    let mut stream = GgufStream::from_file(fd);
    let res = GgufHeader::parse_stream(&mut stream);
    let _ = std::fs_close(fd);
    res
}

/// Candidate model paths checked in priority order for general model probing.
pub const MODEL_CANDIDATE_PATHS: &[&str] = &[
    PATH_EMBEDDING_MODEL_DATA,
    PATH_EMBEDDING_MODEL_SYSTEM,
    "/system/models/embeddinggemma-2-740m.gguf",
    PATH_GENERATIVE_MODEL_DATA,
    PATH_GENERATIVE_MODEL_SYSTEM,
    "/system/models/gemma-2b.gguf",
    "/data/models/gemma-2b.gguf",
    "/system/models/model.gguf",
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

/// Holds probed GGUF model headers for both embedding and generative models.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ProbedModels {
    /// Physical EmbeddingGemma 2 model header and path, if found on storage.
    pub embedding: Option<(GgufHeader, &'static str)>,
    /// Physical Gemma 4 2B generative model header and path, if found on storage.
    pub generative: Option<(GgufHeader, &'static str)>,
}

impl ProbedModels {
    pub const fn empty() -> Self {
        Self {
            embedding: None,
            generative: None,
        }
    }

    pub fn has_embedding(&self) -> bool {
        self.embedding.is_some()
    }

    pub fn has_generative(&self) -> bool {
        self.generative.is_some()
    }
}

/// Probes specifically for physical EmbeddingGemma 2 model weights.
///
/// Priority: `/data/models/embeddinggemma-2.gguf`, then `/system/models/embeddinggemma-2.gguf`.
pub fn probe_embedding_model() -> Result<(GgufHeader, &'static str), GgufError> {
    const CANDIDATES: &[&str] = &[
        PATH_EMBEDDING_MODEL_DATA,
        PATH_EMBEDDING_MODEL_SYSTEM,
        "/system/models/embeddinggemma-2-740m.gguf",
        "/data/models/model.gguf",
        "/system/models/model.gguf",
    ];
    for &path in CANDIDATES {
        if let Ok(hdr) = read_gguf_header(path) {
            return Ok((hdr, path));
        }
    }
    Err(GgufError::NoModelsFound)
}

/// Probes specifically for physical Gemma 4 2B generative model weights.
///
/// Priority: `/data/models/gemma4-2b.gguf`, then `/system/models/gemma4-2b.gguf`.
pub fn probe_generative_model() -> Result<(GgufHeader, &'static str), GgufError> {
    const CANDIDATES: &[&str] = &[
        PATH_GENERATIVE_MODEL_DATA,
        PATH_GENERATIVE_MODEL_SYSTEM,
        "/data/models/gemma-2b.gguf",
        "/system/models/gemma-2b.gguf",
    ];
    for &path in CANDIDATES {
        if let Ok(hdr) = read_gguf_header(path) {
            return Ok((hdr, path));
        }
    }
    Err(GgufError::NoModelsFound)
}

/// Probes all candidate locations for both embedding and generative models.
pub fn probe_all_models() -> ProbedModels {
    ProbedModels {
        embedding: probe_embedding_model().ok(),
        generative: probe_generative_model().ok(),
    }
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

    #[test]
    fn test_find_metadata_with_array_skip() {
        let mut buf = [0u8; 256];
        let header = GgufHeader {
            magic: GGUF_MAGIC,
            version: 3,
            tensor_count: 1,
            kv_count: 2,
        };
        header.write_to(&mut buf).unwrap();

        let mut offset = 24;
        // KV 1: key "dimensions", value type Array (9) of uint32 (4 elements)
        let key1 = b"dimensions";
        buf[offset..offset + 8].copy_from_slice(&(key1.len() as u64).to_le_bytes());
        offset += 8;
        buf[offset..offset + key1.len()].copy_from_slice(key1);
        offset += key1.len();

        buf[offset..offset + 4].copy_from_slice(&9u32.to_le_bytes()); // Array
        offset += 4;
        buf[offset..offset + 4].copy_from_slice(&4u32.to_le_bytes()); // element type = uint32 (4)
        offset += 4;
        buf[offset..offset + 8].copy_from_slice(&4u64.to_le_bytes()); // 4 elements
        offset += 8;
        // 4 x 4 bytes = 16 bytes
        buf[offset..offset + 16].copy_from_slice(&[1, 0, 0, 0, 2, 0, 0, 0, 3, 0, 0, 0, 4, 0, 0, 0]);
        offset += 16;

        // KV 2: key "architecture", value type String (8), value "gemma4"
        let key2 = b"architecture";
        let val2 = b"gemma4";
        buf[offset..offset + 8].copy_from_slice(&(key2.len() as u64).to_le_bytes());
        offset += 8;
        buf[offset..offset + key2.len()].copy_from_slice(key2);
        offset += key2.len();

        buf[offset..offset + 4].copy_from_slice(&8u32.to_le_bytes()); // String
        offset += 4;
        buf[offset..offset + 8].copy_from_slice(&(val2.len() as u64).to_le_bytes());
        offset += 8;
        buf[offset..offset + val2.len()].copy_from_slice(val2);

        // find_metadata_string should successfully skip the array and find "architecture"
        let found = find_metadata_string(&buf, b"architecture");
        assert_eq!(found, Some(&val2[..]));
    }

    #[test]
    fn test_tensor_parsing_and_scanning() {
        let mut buf = [0u8; 512];
        let header = GgufHeader {
            magic: GGUF_MAGIC,
            version: 3,
            tensor_count: 2,
            kv_count: 0,
        };
        header.write_to(&mut buf).unwrap();

        let mut offset = 24;

        // Tensor 1: "token_embd.weight", 2 dims: [768, 256000], type 0 (F32), offset 0
        let t1_name = b"token_embd.weight";
        buf[offset..offset + 8].copy_from_slice(&(t1_name.len() as u64).to_le_bytes());
        offset += 8;
        buf[offset..offset + t1_name.len()].copy_from_slice(t1_name);
        offset += t1_name.len();

        buf[offset..offset + 4].copy_from_slice(&2u32.to_le_bytes()); // 2 dims
        offset += 4;
        buf[offset..offset + 8].copy_from_slice(&768u64.to_le_bytes()); // dim 0
        offset += 8;
        buf[offset..offset + 8].copy_from_slice(&256000u64.to_le_bytes()); // dim 1
        offset += 8;
        buf[offset..offset + 4].copy_from_slice(&0u32.to_le_bytes()); // type F32
        offset += 4;
        buf[offset..offset + 8].copy_from_slice(&0u64.to_le_bytes()); // offset 0
        offset += 8;

        // Tensor 2: "output.weight", 2 dims: [768, 256000], type 2 (Q4_0), offset 786432000
        let t2_name = b"output.weight";
        buf[offset..offset + 8].copy_from_slice(&(t2_name.len() as u64).to_le_bytes());
        offset += 8;
        buf[offset..offset + t2_name.len()].copy_from_slice(t2_name);
        offset += t2_name.len();

        buf[offset..offset + 4].copy_from_slice(&2u32.to_le_bytes()); // 2 dims
        offset += 4;
        buf[offset..offset + 8].copy_from_slice(&768u64.to_le_bytes()); // dim 0
        offset += 8;
        buf[offset..offset + 8].copy_from_slice(&256000u64.to_le_bytes()); // dim 1
        offset += 8;
        buf[offset..offset + 4].copy_from_slice(&2u32.to_le_bytes()); // type Q4_0
        offset += 4;
        buf[offset..offset + 8].copy_from_slice(&786432000u64.to_le_bytes()); // offset

        // Scan for tensor 1
        let found_t1 = find_tensor(&buf, b"token_embd.weight").expect("scan succeeds");
        assert!(found_t1.is_some());
        let t1 = found_t1.unwrap();
        assert_eq!(t1.name_as_bytes(), b"token_embd.weight");
        assert_eq!(t1.n_dimensions, 2);
        assert_eq!(t1.dimensions[0], 768);
        assert_eq!(t1.dimensions[1], 256000);
        assert_eq!(t1.tensor_type, 0);
        assert_eq!(t1.offset, 0);

        // Scan for tensor 2
        let found_t2 = find_tensor(&buf, b"output.weight").expect("scan succeeds");
        assert!(found_t2.is_some());
        let t2 = found_t2.unwrap();
        assert_eq!(t2.name_as_bytes(), b"output.weight");
        assert_eq!(t2.tensor_type, 2);
        assert_eq!(t2.offset, 786432000);

        // Scan for nonexistent tensor
        let not_found = find_tensor(&buf, b"nonexistent.weight").expect("scan succeeds");
        assert!(not_found.is_none());
    }

    #[test]
    fn test_probed_models_empty() {
        let probed = ProbedModels::empty();
        assert!(!probed.has_embedding());
        assert!(!probed.has_generative());
    }

    #[test]
    fn test_tensor_byte_size_calculations() {
        // F32: 768 elements -> 768 * 4 = 3072 bytes
        assert_eq!(tensor_byte_size(0, &[768, 1, 0, 0], 1).unwrap(), 3072);
        // F16: 768 elements -> 768 * 2 = 1536 bytes
        assert_eq!(tensor_byte_size(1, &[768, 1, 0, 0], 1).unwrap(), 1536);
        // Q4_0: 768 elements -> (768 / 32) * 18 = 24 * 18 = 432 bytes
        assert_eq!(tensor_byte_size(2, &[768, 1, 0, 0], 1).unwrap(), 432);
        // Q8_0: 768 elements -> (768 / 32) * 34 = 24 * 34 = 816 bytes
        assert_eq!(tensor_byte_size(8, &[768, 1, 0, 0], 1).unwrap(), 816);
        // Q4_K: 256 elements -> (256 / 256) * 144 = 144 bytes
        assert_eq!(tensor_byte_size(12, &[256, 1, 0, 0], 1).unwrap(), 144);

        // Invalid block alignment error
        assert_eq!(
            tensor_byte_size(2, &[30, 1, 0, 0], 1),
            Err(GgufError::CorruptedTensor)
        );
    }

    #[test]
    fn test_gguf_stream_bounded_parsing() {
        let mut buf = [0u8; 1024];
        let header = GgufHeader {
            magic: GGUF_MAGIC,
            version: 3,
            tensor_count: 3,
            kv_count: 2,
        };
        header.write_to(&mut buf).unwrap();

        let mut offset = 24;
        // KV 1: string "model.arch" = "gemma"
        let k1 = b"model.arch";
        let v1 = b"gemma";
        buf[offset..offset + 8].copy_from_slice(&(k1.len() as u64).to_le_bytes());
        offset += 8;
        buf[offset..offset + k1.len()].copy_from_slice(k1);
        offset += k1.len();
        buf[offset..offset + 4].copy_from_slice(&8u32.to_le_bytes()); // string
        offset += 4;
        buf[offset..offset + 8].copy_from_slice(&(v1.len() as u64).to_le_bytes());
        offset += 8;
        buf[offset..offset + v1.len()].copy_from_slice(v1);
        offset += v1.len();

        // KV 2: uint32 "context_length" = 2048
        let k2 = b"context_length";
        buf[offset..offset + 8].copy_from_slice(&(k2.len() as u64).to_le_bytes());
        offset += 8;
        buf[offset..offset + k2.len()].copy_from_slice(k2);
        offset += k2.len();
        buf[offset..offset + 4].copy_from_slice(&4u32.to_le_bytes()); // uint32
        offset += 4;
        buf[offset..offset + 4].copy_from_slice(&2048u32.to_le_bytes());
        offset += 4;

        // Tensor 1: "blk.0.attn_q.weight", 2 dims: [64, 64], type 0 (F32), offset 0
        let t1 = b"blk.0.attn_q.weight";
        buf[offset..offset + 8].copy_from_slice(&(t1.len() as u64).to_le_bytes());
        offset += 8;
        buf[offset..offset + t1.len()].copy_from_slice(t1);
        offset += t1.len();
        buf[offset..offset + 4].copy_from_slice(&2u32.to_le_bytes());
        offset += 4;
        buf[offset..offset + 8].copy_from_slice(&64u64.to_le_bytes());
        offset += 8;
        buf[offset..offset + 8].copy_from_slice(&64u64.to_le_bytes());
        offset += 8;
        buf[offset..offset + 4].copy_from_slice(&0u32.to_le_bytes());
        offset += 4;
        buf[offset..offset + 8].copy_from_slice(&0u64.to_le_bytes());
        offset += 8;

        // Tensor 2: "blk.0.attn_k.weight"
        let t2 = b"blk.0.attn_k.weight";
        buf[offset..offset + 8].copy_from_slice(&(t2.len() as u64).to_le_bytes());
        offset += 8;
        buf[offset..offset + t2.len()].copy_from_slice(t2);
        offset += t2.len();
        buf[offset..offset + 4].copy_from_slice(&2u32.to_le_bytes());
        offset += 4;
        buf[offset..offset + 8].copy_from_slice(&64u64.to_le_bytes());
        offset += 8;
        buf[offset..offset + 8].copy_from_slice(&64u64.to_le_bytes());
        offset += 8;
        buf[offset..offset + 4].copy_from_slice(&0u32.to_le_bytes());
        offset += 4;
        buf[offset..offset + 8].copy_from_slice(&16384u64.to_le_bytes());
        offset += 8;

        // Tensor 3: "blk.0.attn_v.weight"
        let t3 = b"blk.0.attn_v.weight";
        buf[offset..offset + 8].copy_from_slice(&(t3.len() as u64).to_le_bytes());
        offset += 8;
        buf[offset..offset + t3.len()].copy_from_slice(t3);
        offset += t3.len();
        buf[offset..offset + 4].copy_from_slice(&2u32.to_le_bytes());
        offset += 4;
        buf[offset..offset + 8].copy_from_slice(&64u64.to_le_bytes());
        offset += 8;
        buf[offset..offset + 8].copy_from_slice(&64u64.to_le_bytes());
        offset += 8;
        buf[offset..offset + 4].copy_from_slice(&0u32.to_le_bytes());
        offset += 4;
        buf[offset..offset + 8].copy_from_slice(&32768u64.to_le_bytes());
        offset += 8;

        let mut stream = GgufStream::from_slice(&buf[..offset]);
        let found = find_tensor_stream(&mut stream, b"blk.0.attn_k.weight").unwrap();
        assert!(found.is_some());
        let t = found.unwrap();
        assert_eq!(t.name_as_bytes(), b"blk.0.attn_k.weight");
        assert_eq!(t.offset, 16384);
    }
}
