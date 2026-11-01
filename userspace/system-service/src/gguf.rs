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

/// Where a model file the device installed itself is held.
///
/// WHY one fixed path: the install is the platform moving bytes the person put
/// in their own documents into the model namespace, and a namespace that grows
/// a file per request has no way to answer "which of these is the model?". One
/// named file, replaced by the next install, is what the screen reports on.
pub const PATH_LOCAL_MODEL_DATA: &str = "/data/models/local_model.gguf";

/// The trees an installed model file may be taken from.
///
/// WHY these and nothing else: an install moves bytes the device already holds
/// as a document, and there are two ways a model file becomes a document on
/// this device -- the browser's download directory and the directory the
/// platform's own delivery channel writes into. A request that names anywhere
/// else is not pointing at a document the person put there, so it is refused
/// before a single byte is read.
pub const MODEL_SOURCE_TREES: &[&str] = &["/storage/downloads", "/data/incoming"];

/// Maximum length for a tensor name in zero-allocation buffer.
pub const MAX_TENSOR_NAME_LEN: usize = 64;
/// Longest metadata key the configuration reader will name. A longer key is
/// not one of the keys it recognises, so it costs nothing to step over it.
pub const MAX_METADATA_KEY_LEN: usize = 48;
/// Architecture names are short, and the engine only ever reports this one
/// back to the user, so it is held in a fixed window.
pub const MAX_ARCH_NAME_LEN: usize = 32;
/// How many elements of a string array the reader is willing to step over.
///
/// WHY the vocabulary is not walked the way every other value is: a GGUF
/// string array is one length-prefixed string per element, and a Gemma
/// vocabulary is 256,000 of them -- two reads each, so half a million syscalls
/// to pass one key. On a model file the kernel holds on the data disk, where
/// every read remounts and rescans the inode table, that is not a slow path
/// but a dead one. The array's own 12-byte header carries its element count,
/// so the vocabulary size is read in one step and the elements are left alone.
pub const MAX_STRING_ARRAY_ELEMENTS: usize = 4096;
/// Longest `tokenizer.ggml.pre` name the reader will hold.
///
/// WHY so short: the value names a tokenizer family out of a fixed set -- `gemma`,
/// `default`, `nova2` -- and a file stating more than this is not naming a family
/// the engine could switch on anyway. Refusing it by length, with the length the
/// file stated, is the honest answer.
pub const MAX_TOKENIZER_PRE_LEN: usize = 32;
/// Longest `tokenizer.chat_template` the reader will hold.
///
/// WHY a cap at all: the template is a program in a template language, and a real
/// Gemma file carries one past a kilobyte. The reader holds what it can quote to
/// the screen and what the prompt formatter can actually apply; a template longer
/// than the window is reported with its length rather than cut down to something
/// that reads like the model's own formatting and is not.
pub const MAX_CHAT_TEMPLATE_LEN: usize = 1024;
/// How many bytes the buffered reader moves at a time.
///
/// WHY a window and not a request per value: a real model file puts its
/// 256,000-entry vocabulary in front of the keys the engine still needs, and a
/// string array states no total size -- each element's own length prefix is what
/// says where the next one starts. Reading a prefix per request would be half a
/// million requests to pass one key. The window takes this many bytes at a time
/// from the source and serves every prefix read and every step past it out of
/// them, so the array is passed by size arithmetic in memory and the source is
/// touched once per window.
pub const METADATA_WINDOW_BYTES: usize = 512;

/// Error types for GGUF parsing and model probing.
///
/// WHY no `Eq`: a refusal to configure the engine carries what the file did
/// declare, and a declared float is comparable without being an equivalence.
#[derive(Debug, Clone, Copy, PartialEq)]
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
    /// A parameter the engine cannot run without was not declared in the part of
    /// the metadata this reader can reach. Names the key and carries everything
    /// the file did declare, so the answer can quote the model rather than guess.
    MissingMetadata(&'static str, GgufModelInfo),
}

/// Bounded sequential GGUF data stream reader (zero heap allocation).
///
/// Supports reading from either an in-memory byte slice or a streaming file descriptor
/// with sequential chunk buffering, enabling bounded parsing of arbitrarily large models
/// without consuming unbounded memory.
pub enum GgufStream<'a> {
    Slice { data: &'a [u8], pos: usize },
    File { fd: u32, pos: usize },
    /// One source read through a fixed window of `METADATA_WINDOW_BYTES`.
    ///
    /// WHY the walk that owns it uses this and not a plain `File`: the reader that
    /// steps over a vocabulary does not know where the array ends until it has
    /// passed the last length prefix, so it must be able to pull more bytes than it
    /// is currently naming. A stream that hands bytes straight to the caller cannot
    /// over-read; a window that the walk itself owns can, because the bytes it
    /// pulled early are simply the next bytes it consumes.
    Buffered {
        src: BufferedSource<'a>,
        held: [u8; METADATA_WINDOW_BYTES],
        used: usize,
        filled: usize,
        pos: usize,
        pulls: usize,
    },
}

/// Where a `Buffered` stream's bytes come from.
pub enum BufferedSource<'a> {
    /// An in-memory file image, which is what the host-side tests drive the window
    /// with -- the same stepping code, no syscalls.
    Slice { data: &'a [u8], pos: usize },
    /// A descriptor the platform holds, read a window at a time.
    ///
    /// WHY it carries its own position: the window pulls from the source ahead of
    /// where the reader has got to, and a descriptor that only ever moves forward
    /// would tie the two together. Reading at a named position keeps them apart.
    File { fd: u32, pos: usize },
}

impl<'a> GgufStream<'a> {
    pub fn from_slice(data: &'a [u8]) -> Self {
        Self::Slice { data, pos: 0 }
    }

    pub fn from_file(fd: u32) -> Self {
        Self::File { fd, pos: 0 }
    }

    /// A slice read through the window, so the window's own bookkeeping is testable.
    pub fn buffered_from_slice(data: &'a [u8]) -> Self {
        Self::Buffered {
            src: BufferedSource::Slice { data, pos: 0 },
            held: [0u8; METADATA_WINDOW_BYTES],
            used: 0,
            filled: 0,
            pos: 0,
            pulls: 0,
        }
    }

    /// A descriptor read through the window.
    pub fn buffered_from_file(fd: u32) -> Self {
        Self::Buffered {
            src: BufferedSource::File { fd, pos: 0 },
            held: [0u8; METADATA_WINDOW_BYTES],
            used: 0,
            filled: 0,
            pos: 0,
            pulls: 0,
        }
    }

    /// How many times the window had to touch its source, and `0` for a stream
    /// that never buffers.
    ///
    /// WHY it is exposed: the claim being made about the window is a claim about
    /// requests, not about bytes, and a number a test can read is the only way to
    /// keep that claim honest.
    pub fn pulls(&self) -> usize {
        match self {
            Self::Buffered { pulls, .. } => *pulls,
            _ => 0,
        }
    }

    /// Moves the unread tail to the front of the window and fills what is left of
    /// the window from the source.
    ///
    /// WHY it fills the whole window rather than what was asked for: a request that
    /// took exactly the eight bytes of the next length prefix would put the source
    /// back at one request per token, which is the cost this window exists to avoid.
    fn refill_window(
        src: &mut BufferedSource<'a>,
        held: &mut [u8],
        used: &mut usize,
        filled: &mut usize,
        pulls: &mut usize,
    ) -> Result<(), GgufError> {
        let rest = *filled - *used;
        if *used > 0 {
            held[..*filled].copy_within(*used..*filled, 0);
            *used = 0;
        }
        *filled = rest;
        if rest >= held.len() {
            return Ok(());
        }
        let pulled = match src {
            BufferedSource::Slice { data, pos } => {
                let take = data.len().saturating_sub(*pos).min(held.len() - rest);
                held[rest..rest + take].copy_from_slice(&data[*pos..*pos + take]);
                *pos += take;
                take
            }
            BufferedSource::File { fd, pos } => {
                let asked = held.len() - rest;
                let pulled = std::fs_read_at(*fd, *pos as u64, &mut held[rest..])
                    .map_err(|_| GgufError::ReadFailed)?;
                // The window may only treat as present the bytes that fit the space it
                // asked for; see `GgufStream::read_exact` for why a wider count is a
                // failed read rather than a larger read.
                if pulled > asked {
                    return Err(GgufError::ReadFailed);
                }
                *pos = pos
                    .checked_add(pulled)
                    .ok_or(GgufError::ArithmeticOverflow)?;
                pulled
            }
        };
        *filled += pulled;
        *pulls += 1;
        Ok(())
    }

    pub fn position(&self) -> usize {
        match self {
            Self::Slice { pos, .. } => *pos,
            Self::File { pos, .. } => *pos,
            Self::Buffered { pos, .. } => *pos,
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
                    let remaining = req_len - filled;
                    let n = std::fs_read_at(
                        *fd,
                        pos.checked_add(filled).ok_or(GgufError::ArithmeticOverflow)? as u64,
                        &mut out[filled..],
                    )
                    .map_err(|_| GgufError::ReadFailed)?;
                    if n == 0 {
                        return Err(GgufError::BufferTooShort);
                    }
                    // WHY the count is bounded as well as tested for zero: the kernel
                    // never hands back more than it was asked for, but this parser is
                    // also built for the host, where the syscall number names no
                    // InterOS call and what comes back is the host's own status -- a
                    // positive one reads as a byte count, and the buffer it "filled"
                    // still holds what the caller filled it with. A header parsed out
                    // of that is answered as corrupt weights for a file that was never
                    // readable at all.
                    if n > remaining {
                        return Err(GgufError::ReadFailed);
                    }
                    filled = filled.checked_add(n).ok_or(GgufError::ArithmeticOverflow)?;
                }
                *pos = pos.checked_add(req_len).ok_or(GgufError::ArithmeticOverflow)?;
                Ok(())
            }
            Self::Buffered {
                src,
                held,
                used,
                filled,
                pos,
                pulls,
            } => {
                let mut done = 0usize;
                while done < req_len {
                    if *used == *filled {
                        Self::refill_window(src, held, used, filled, pulls)?;
                        if *filled == 0 {
                            return Err(GgufError::BufferTooShort);
                        }
                    }
                    let take = (req_len - done).min(*filled - *used);
                    out[done..done + take].copy_from_slice(&held[*used..*used + take]);
                    *used += take;
                    *pos += take;
                    done += take;
                }
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
            Self::File { pos, .. } => {
                // WHY a skip now costs no reads at all: bytes are fetched from the
                // position the caller names, so stepping over what sits between two
                // records no longer has to push every one of them through a syscall
                // and throw it away. Walking to the last tensor of a large file used to
                // read the whole file to get there. What the walk gave up is the end of
                // the file, which used to turn up on the way: the next read reports it,
                // and where the walk is about to report a verdict about the file
                // instead, `source_ends_early` asks the one question the step can no
                // longer answer on its own.
                *pos = pos.checked_add(count).ok_or(GgufError::ArithmeticOverflow)?;
                Ok(())
            }
            Self::Buffered {
                src,
                held,
                used,
                filled,
                pos,
                pulls,
            } => {
                while count > 0 {
                    if *used == *filled {
                        Self::refill_window(src, held, used, filled, pulls)?;
                        if *filled == 0 {
                            return Err(GgufError::BufferTooShort);
                        }
                    }
                    let take = count.min(*filled - *used);
                    *used += take;
                    *pos += take;
                    count -= take;
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

    /// Whether the source stops at the position this stream has stepped to.
    ///
    /// WHY only a descriptor is asked: a step over a slice is bounded by the slice
    /// and a step through a window pulls the bytes it steps over, so both meet the
    /// end of the source on the way and cannot report a position the source does not
    /// hold. A stepped descriptor moves a number and never sees the edge it passes,
    /// so it is the one that has to be asked.
    ///
    /// WHY a successful probe costs nothing: the byte it takes is the one the walk
    /// stepped onto, and every caller asks here only when it is about to hand back
    /// its verdict and stop reading.
    fn stepped_past_the_end(&mut self) -> bool {
        if !matches!(self, Self::File { .. }) {
            return false;
        }
        let mut probe = [0u8; 1];
        self.read_exact(&mut probe).is_err()
    }
}

/// Whether the walk stopped somewhere the file does not reach.
///
/// WHY the tensor count gates the question: a file that declares no tensor is
/// allowed to end with its metadata, and asking whether one more byte follows
/// would call a complete placeholder a truncated one. A file that declares tensors
/// promises a tensor section after the metadata, so a walk that stepped to the end
/// of the declared pairs and finds no byte there is looking at a file that stops
/// early -- and saying so is the answer, rather than the walk's own verdict about
/// what the file does or does not declare.
///
/// WHY it costs one read: before a step could leave the file behind without anyone
/// noticing, the walk reached a position by reading every byte up to it.
fn source_ends_early(stream: &mut GgufStream, tensors: u64) -> bool {
    tensors > 0 && stream.stepped_past_the_end()
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

/// What a model file declares about its own architecture.
///
/// Every field is `Option`: a file that does not name a parameter is reported as
/// not naming it, rather than being filled in with the value the engine happens
/// to have been compiled with.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GgufModelInfo {
    /// `general.architecture`, e.g. `gemma2`.
    pub architecture: [u8; MAX_ARCH_NAME_LEN],
    pub architecture_len: usize,
    /// `<arch>.embedding_length` -- the width every residual stream has.
    pub hidden_dim: Option<u32>,
    /// `<arch>.feed_forward_length`
    pub intermediate_dim: Option<u32>,
    /// `<arch>.block_count`
    pub num_layers: Option<u32>,
    /// `<arch>.attention.head_count`
    pub num_heads: Option<u32>,
    /// `<arch>.attention.head_count_kv`
    pub num_kv_heads: Option<u32>,
    /// `<arch>.attention.head_length`, or `key_head_length` where the file names
    /// the query head width separately.
    pub head_dim: Option<u32>,
    /// Length of `tokenizer.ggml.tokens`, read from the array's own header.
    pub vocab_size: Option<u32>,
    /// `<arch>.attention.layer_norm_rms_epsilon`
    pub rms_norm_eps: Option<f32>,
    /// `<arch>.rope.freq_base`
    pub rope_theta: Option<f32>,
}

impl GgufModelInfo {
    pub const fn empty() -> Self {
        Self {
            architecture: [0u8; MAX_ARCH_NAME_LEN],
            architecture_len: 0,
            hidden_dim: None,
            intermediate_dim: None,
            num_layers: None,
            num_heads: None,
            num_kv_heads: None,
            head_dim: None,
            vocab_size: None,
            rms_norm_eps: None,
            rope_theta: None,
        }
    }

    pub fn architecture(&self) -> Option<&str> {
        if self.architecture_len == 0 {
            return None;
        }
        core::str::from_utf8(&self.architecture[..self.architecture_len]).ok()
    }

    /// The first parameter the file has not declared, named by the key it would
    /// have been declared under. `None` means the engine has everything it asks
    /// the file for.
    pub fn missing_key(&self) -> Option<&'static str> {
        if self.architecture_len == 0 {
            return Some("general.architecture");
        }
        if self.hidden_dim.is_none() {
            return Some("<arch>.embedding_length");
        }
        if self.intermediate_dim.is_none() {
            return Some("<arch>.feed_forward_length");
        }
        if self.num_layers.is_none() {
            return Some("<arch>.block_count");
        }
        if self.num_heads.is_none() {
            return Some("<arch>.attention.head_count");
        }
        if self.head_dim.is_none() {
            return Some("<arch>.attention.head_length");
        }
        if self.vocab_size.is_none() {
            return Some("tokenizer.ggml.tokens");
        }
        None
    }

    /// Whether the engine can stop reading the metadata: every parameter it
    /// requires is known, so nothing further is worth the cost of reaching.
    fn is_complete(&self) -> bool {
        self.missing_key().is_none()
    }
}

/// The last dotted segment of a metadata key, which is where GGUF puts the
/// parameter name and the architecture prefix puts the model it belongs to.
fn metadata_field(key: &[u8]) -> &[u8] {
    match key.iter().rposition(|&b| b == b'.') {
        Some(dot) => &key[dot + 1..],
        None => key,
    }
}

/// Reads one metadata value as a count, whatever width the file declared it in.
fn read_count(stream: &mut GgufStream, val_type: u32) -> Result<u32, GgufError> {
    let wide = match val_type {
        0 | 1 | 7 => {
            let mut b = [0u8; 1];
            stream.read_exact(&mut b)?;
            b[0] as u64
        }
        2 | 3 => {
            let mut b = [0u8; 2];
            stream.read_exact(&mut b)?;
            u16::from_le_bytes(b) as u64
        }
        4 | 5 => {
            let mut b = [0u8; 4];
            stream.read_exact(&mut b)?;
            u32::from_le_bytes(b) as u64
        }
        10 | 11 => stream.read_u64()?,
        _ => return Err(GgufError::InvalidMetadata),
    };
    u32::try_from(wide).map_err(|_| GgufError::InvalidMetadata)
}

/// Reads one metadata value as a float, whether the file declared f32 or f64.
fn read_epsilon(stream: &mut GgufStream, val_type: u32) -> Result<f32, GgufError> {
    match val_type {
        6 => {
            let mut b = [0u8; 4];
            stream.read_exact(&mut b)?;
            Ok(f32::from_le_bytes(b))
        }
        12 => {
            let mut b = [0u8; 8];
            stream.read_exact(&mut b)?;
            Ok(f64::from_le_bytes(b) as f32)
        }
        _ => Err(GgufError::InvalidMetadata),
    }
}

/// Reads the one-byte flag GGUF type 7 carries.
///
/// WHY it is its own reader and not `read_epsilon` or `read_count`: a boolean is
/// one byte wide and no float width, so routing it through the float reader would
/// reject every file that declares one, and routing it through the count reader
/// would accept any byte at all -- including the 17 a corrupt file says is a
/// flag. A value that is neither 0 nor 1 is not a declared boolean, so the file
/// gets said so.
fn read_bool(stream: &mut GgufStream, val_type: u32) -> Result<bool, GgufError> {
    if val_type != 7 {
        return Err(GgufError::InvalidMetadata);
    }
    let mut byte = [0u8; 1];
    stream.read_exact(&mut byte)?;
    match byte[0] {
        0 => Ok(false),
        1 => Ok(true),
        _ => Err(GgufError::InvalidMetadata),
    }
}

/// Walks the metadata block of a GGUF stream as far as it has to go to configure
/// the engine, and no further.
///
/// WHY it stops early: the parameters the engine asks for are scalars, and the
/// one large value it asks for -- the vocabulary -- states its own length in a
/// 12-byte header. Continuing past that to reach anything else would mean
/// paying for the array, so a parameter a file declares after its vocabulary is
/// reported as unreachable instead of being read half-way.
pub fn read_model_info_stream(stream: &mut GgufStream) -> Result<GgufModelInfo, GgufError> {
    let header = GgufHeader::parse_stream(stream)?;
    let mut info = GgufModelInfo::empty();

    for _ in 0..header.kv_count {
        let key_len = stream.read_u64()? as usize;
        if key_len == 0 || key_len > MAX_METADATA_KEY_LEN {
            stream.skip(key_len)?;
            let val_type = stream.read_u32()?;
            skip_metadata_value_stream(stream, val_type)?;
            continue;
        }

        let mut key_buf = [0u8; MAX_METADATA_KEY_LEN];
        stream.read_exact(&mut key_buf[..key_len])?;
        let key = &key_buf[..key_len];
        let val_type = stream.read_u32()?;

        if key == b"general.architecture" {
            let str_len = stream.read_u64()? as usize;
            let copy_len = str_len.min(MAX_ARCH_NAME_LEN);
            stream.read_exact(&mut info.architecture[..copy_len])?;
            info.architecture_len = copy_len;
            if str_len > copy_len {
                stream.skip(str_len - copy_len)?;
            }
        } else if key == b"tokenizer.ggml.tokens" {
            let elem_type = stream.read_u32()?;
            let elem_count = stream.read_u64()?;
            if elem_type != 8 {
                return Err(GgufError::InvalidMetadata);
            }
            info.vocab_size = u32::try_from(elem_count).ok();
            if !info.is_complete() {
                if elem_count > MAX_STRING_ARRAY_ELEMENTS as u64 {
                    let missing = info
                        .missing_key()
                        .expect("incomplete by the branch above");
                    return Err(GgufError::MissingMetadata(missing, info));
                }
                for _ in 0..elem_count {
                    let s_len = stream.read_u64()? as usize;
                    stream.skip(s_len)?;
                }
            }
        } else {
            match metadata_field(key) {
                b"embedding_length" => info.hidden_dim = Some(read_count(stream, val_type)?),
                b"feed_forward_length" => {
                    info.intermediate_dim = Some(read_count(stream, val_type)?)
                }
                b"block_count" => info.num_layers = Some(read_count(stream, val_type)?),
                b"head_count" => info.num_heads = Some(read_count(stream, val_type)?),
                b"head_count_kv" => info.num_kv_heads = Some(read_count(stream, val_type)?),
                b"head_length" | b"key_head_length" => {
                    info.head_dim = Some(read_count(stream, val_type)?)
                }
                b"layer_norm_rms_epsilon" => {
                    info.rms_norm_eps = Some(read_epsilon(stream, val_type)?)
                }
                b"freq_base" => info.rope_theta = Some(read_epsilon(stream, val_type)?),
                _ => skip_metadata_value_stream(stream, val_type)?,
            }
        }

        if info.is_complete() {
            if source_ends_early(stream, header.tensor_count) {
                return Err(GgufError::BufferTooShort);
            }
            return Ok(info);
        }
    }

    if source_ends_early(stream, header.tensor_count) {
        return Err(GgufError::BufferTooShort);
    }
    match info.missing_key() {
        Some(missing) => Err(GgufError::MissingMetadata(missing, info)),
        None => Ok(info),
    }
}

/// Reads the declaring metadata of the model file at `path`.
pub fn read_model_info(path: &str) -> Result<GgufModelInfo, GgufError> {
    let fd = std::fs_open(path, std::FS_READ).map_err(|_| GgufError::FileNotFound)?;
    let mut stream = GgufStream::from_file(fd);
    let res = read_model_info_stream(&mut stream);
    let _ = std::fs_close(fd);
    res
}

/// The keys the tokenizer walk names, matched whole.
///
/// WHY whole and not by their last dotted segment: `metadata_field` answers with
/// the bytes after the last dot, so `tokenizer.ggml.pre` and any other family's
/// `something.else.pre` are the same name to it. The vocabulary keys are few,
/// fixed and short, so the walk spends the byte comparison rather than letting a
/// foreign key configure the tokenizer.
const KEY_TOKENS: &[u8] = b"tokenizer.ggml.tokens";
const KEY_BOS_ID: &[u8] = b"tokenizer.ggml.bos_token_id";
const KEY_EOS_ID: &[u8] = b"tokenizer.ggml.eos_token_id";
const KEY_PAD_ID: &[u8] = b"tokenizer.ggml.padding_token_id";
const KEY_ADD_BOS: &[u8] = b"tokenizer.ggml.add_bos_token";
const KEY_PRE: &[u8] = b"tokenizer.ggml.pre";
const KEY_CHAT_TEMPLATE: &[u8] = b"tokenizer.chat_template";
/// What the walk names a pair whose key it never got to read.
const KEY_PAIR: &[u8] = b"<metadata pair>";

/// The template the platform formats a prompt with when the model file names
/// none, and the template it reports as the default in that case.
///
/// WHY a built-in at all: a generative model that ships without a template is
/// still going to be asked to answer a turn, and the alternative to a stated
/// default is each caller inventing one. It is reported as the platform's own,
/// never as the file's.
pub const DEFAULT_CHAT_TEMPLATE: &str =
    "<start_of_turn>user\n{prompt}<end_of_turn>\n<start_of_turn>model\n";

/// Where the text the prompt formatter will use came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TemplateSource {
    /// The bytes the file declared, held whole by the walk.
    FromFile,
    /// The file declared no template the reader could hold, so the platform's
    /// own default is in use -- and says so.
    BuiltInDefault,
}

/// Why a prompt could not be formatted into the buffer it was offered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PromptFit {
    /// The template names no `{prompt}` marker, so the text has nowhere to go.
    NoMarker,
    /// Template plus text is longer than the buffer; the size that would have
    /// fitted is reported.
    TooLong { needed: usize },
}

/// What the walk did with a string it reached.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StringReading {
    /// The file does not name this key in the metadata the walk walked.
    Absent,
    /// The value was read whole: this many bytes are held by the reader.
    Read(usize),
    /// The file stated this many bytes, more than the reader's cap for this key.
    /// Nothing was copied; the walk stepped past the value and reports the length.
    RefusedTooLong(u64),
}

impl StringReading {
    pub fn bytes(&self) -> Option<usize> {
        match self {
            Self::Read(len) => Some(*len),
            _ => None,
        }
    }
}

/// What the walk did with `tokenizer.ggml.tokens`.
///
/// WHY a count is a complete answer here: the engine needs the vocabulary's width,
/// and a GGUF string array states that in its own 12-byte header. The elements
/// themselves are 256,000 strings the platform cannot hold and does not need to
/// read in order to say how many there are, so they are stepped over by their
/// length prefixes and never loaded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VocabReading {
    /// The file names no vocabulary.
    Absent,
    /// The array header stated this many string elements; none were loaded.
    Counted { elements: u32 },
    /// The array header stated more elements than a count can name, which no real
    /// vocabulary does. Nothing was stepped over.
    BeyondCount { stated: u64 },
    /// The array header stated this many elements and the file ended before the
    /// walk had passed all of them.
    Truncated { stated: u32 },
}

/// What the service knows about a model file's tokenizer, and how much of it the
/// file itself said.
///
/// Every field starts unset. Nothing here is filled in with the value the engine
/// was compiled with: a key the file did not declare is reported as a key the file
/// did not declare, and the prompt formatter then says it is using the platform's
/// default.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GgufTokenizerInfo {
    /// `tokenizer.ggml.tokens`, counted from the array's own header.
    pub vocab: VocabReading,
    /// `tokenizer.ggml.bos_token_id`
    pub bos_token_id: Option<u32>,
    /// `tokenizer.ggml.eos_token_id`
    pub eos_token_id: Option<u32>,
    /// `tokenizer.ggml.padding_token_id`
    pub pad_token_id: Option<u32>,
    /// `tokenizer.ggml.add_bos_token`, the one-byte flag of GGUF type 7.
    pub add_bos_token: Option<bool>,
    /// `tokenizer.ggml.pre`, the tokenizer family the file names.
    pub pre: [u8; MAX_TOKENIZER_PRE_LEN],
    pub pre_reading: StringReading,
    /// `tokenizer.chat_template`
    pub chat_template: [u8; MAX_CHAT_TEMPLATE_LEN],
    pub template_reading: StringReading,
    /// The key whose value the file did not finish writing, when it did not.
    ///
    /// WHY bytes and not `str`: the key the walk was holding is the bytes the file
    /// wrote, and a notice quoting them has no reason to convert them first.
    pub truncated_in: Option<&'static [u8]>,
    /// Metadata pairs the walk stepped through to get here.
    pub pairs_walked: u64,
    /// How many times the window touched its source.
    pub pulls: usize,
}

impl GgufTokenizerInfo {
    pub const fn empty() -> Self {
        Self {
            vocab: VocabReading::Absent,
            bos_token_id: None,
            eos_token_id: None,
            pad_token_id: None,
            add_bos_token: None,
            pre: [0u8; MAX_TOKENIZER_PRE_LEN],
            pre_reading: StringReading::Absent,
            chat_template: [0u8; MAX_CHAT_TEMPLATE_LEN],
            template_reading: StringReading::Absent,
            truncated_in: None,
            pairs_walked: 0,
            pulls: 0,
        }
    }

    pub fn vocab_size(&self) -> Option<u32> {
        match self.vocab {
            VocabReading::Counted { elements } => Some(elements),
            VocabReading::Truncated {
                stated,
            } => Some(stated),
            _ => None,
        }
    }

    /// Whether the vocabulary is larger than the reader is willing to load.
    ///
    /// WHY it is a question and not a failure: every real Gemma vocabulary is over
    /// that bound, and the reader's answer is a count it got from the array's own
    /// header, which is the number the engine runs on.
    pub fn vocab_counted_not_loaded(&self) -> bool {
        self.vocab_size()
            .map(|elements| elements as usize > MAX_STRING_ARRAY_ELEMENTS)
            .unwrap_or(false)
    }

    /// The template the file declared, when the walk reached and held it whole.
    pub fn chat_template(&self) -> Option<&[u8]> {
        match self.template_reading.bytes() {
            Some(len) => Some(&self.chat_template[..len]),
            None => None,
        }
    }

    pub fn pre_name(&self) -> Option<&str> {
        match self.pre_reading.bytes() {
            Some(len) => core::str::from_utf8(&self.pre[..len]).ok(),
            None => None,
        }
    }

    /// The template in use and where it came from: the file's own when the walk
    /// held it, the platform's default otherwise.
    pub fn active_chat_template(&self) -> (&[u8], TemplateSource) {
        match self.chat_template() {
            Some(template) => (template, TemplateSource::FromFile),
            None => (DEFAULT_CHAT_TEMPLATE.as_bytes(), TemplateSource::BuiltInDefault),
        }
    }

    /// Whether the file named none of the tokenizer keys the walk asks for.
    ///
    /// WHY this is its own answer: a model file with no tokenizer block at all is
    /// a real thing (an embedding model with a stripped vocabulary is one), and it
    /// loads -- the engine just has no template or token ids of the file's to use,
    /// which is a different statement from "the template is empty".
    pub fn declares_no_tokenizer(&self) -> bool {
        self.vocab == VocabReading::Absent
            && self.bos_token_id.is_none()
            && self.eos_token_id.is_none()
            && self.pad_token_id.is_none()
            && self.add_bos_token.is_none()
            && self.pre_reading == StringReading::Absent
            && self.template_reading == StringReading::Absent
    }

    /// Whether every key the walk names has been reached, so nothing further is
    /// worth the cost of stepping through.
    fn is_satisfied(&self) -> bool {
        self.vocab != VocabReading::Absent
            && self.bos_token_id.is_some()
            && self.eos_token_id.is_some()
            && self.pad_token_id.is_some()
            && self.add_bos_token.is_some()
            && self.pre_reading != StringReading::Absent
            && self.template_reading != StringReading::Absent
    }

    /// Formats one user turn into `out` at the template's `{prompt}` marker.
    ///
    /// WHY it does not try to run the template: a GGUF chat template is a program
    /// in a template language, with conditionals and loops over roles. This applies
    /// the one shape the platform asks for -- a single user turn spliced at the
    /// marker -- and reports `NoMarker` when the template has no such marker rather
    /// than guessing where the text goes.
    pub fn format_prompt(&self, user: &[u8], out: &mut [u8]) -> Result<usize, PromptFit> {
        let (template, _) = self.active_chat_template();
        let marker = template
            .windows(PROMPT_MARKER.len())
            .position(|window| window == PROMPT_MARKER);
        let at = match marker {
            Some(at) => at,
            None => return Err(PromptFit::NoMarker),
        };
        let head = &template[..at];
        let tail = &template[at + PROMPT_MARKER.len()..];
        let needed = head.len() + user.len() + tail.len();
        if needed > out.len() {
            return Err(PromptFit::TooLong { needed });
        }
        out[..head.len()].copy_from_slice(head);
        out[head.len()..head.len() + user.len()].copy_from_slice(user);
        let after = head.len() + user.len();
        out[after..after + tail.len()].copy_from_slice(tail);
        Ok(needed)
    }
}

/// The marker a chat template leaves for the user's own text.
pub const PROMPT_MARKER: &[u8] = b"{prompt}";

/// Steps over the elements of a string array without loading any of them.
///
/// WHY it reads the prefixes rather than the strings: a string array states no
/// total size, so an element's start is only known once the one before it has been
/// named. Over a windowed stream that is memory arithmetic -- the window is what
/// touches the source, once per `METADATA_WINDOW_BYTES` -- and the array is passed
/// by size instead of by a request per element.
fn step_over_string_array(stream: &mut GgufStream, elements: u64) -> Result<(), GgufError> {
    for _ in 0..elements {
        let len = stream.read_u64()?;
        let len = usize::try_from(len).map_err(|_| GgufError::ArithmeticOverflow)?;
        stream.skip(len)?;
    }
    Ok(())
}

/// Reads a declared string into `dst`, refusing it by length before a byte of it
/// is copied.
fn read_bounded_string(
    stream: &mut GgufStream,
    val_type: u32,
    dst: &mut [u8],
) -> Result<StringReading, GgufError> {
    if val_type != 8 {
        return Err(GgufError::InvalidMetadata);
    }
    let stated = stream.read_u64()?;
    let need = match usize::try_from(stated) {
        Ok(need) => need,
        // A length this wide is not a value the reader could hold or step over, so
        // it is refused with the number the file stated.
        Err(_) => return Ok(StringReading::RefusedTooLong(stated)),
    };
    if need > dst.len() {
        stream.skip(need)?;
        return Ok(StringReading::RefusedTooLong(stated));
    }
    stream.read_exact(&mut dst[..need])?;
    Ok(StringReading::Read(need))
}

/// Walks a model file's metadata block for the keys the tokenizer needs.
///
/// WHY a walk of its own, run through a window: `read_model_info_stream` configures
/// the engine from scalars and stops at the vocabulary, because the parameters it
/// wants sit before it and anything past a 256,000-entry array costs the file's
/// bytes to reach. `tokenizer.chat_template` sits exactly there in a real Gemma
/// file, so this walk takes the vocabulary's count from the array's header, steps
/// over the array by its length prefixes without loading it, and keeps going -- and
/// reports which of its keys the file never named.
pub fn read_tokenizer_info_stream(stream: &mut GgufStream) -> Result<GgufTokenizerInfo, GgufError> {
    let header = GgufHeader::parse_stream(stream)?;
    let mut info = GgufTokenizerInfo::empty();
    match walk_tokenizer_kv(stream, header.kv_count, &mut info) {
        Ok(()) => {}
        // WHY a file that runs out is an `Ok`: the walk's job is to say what the
        // file declared, and a truncated file declared things before it stopped
        // having bytes. `truncated_in` names where, and every key reached stays.
        Err(GgufError::BufferTooShort) | Err(GgufError::ReadFailed) => {}
        Err(other) => return Err(other),
    }
    info.pulls = stream.pulls();
    Ok(info)
}

/// Reads the model file at `path` for its tokenizer's declarations.
///
/// WHY the file is opened a second time rather than the first stream being shared:
/// the configuration walk stops at the vocabulary and cannot be asked to carry on
/// past it, and one pass over the metadata cannot serve two different stopping
/// rules. The descriptor is opened, walked through a window, and closed.
pub fn read_tokenizer_info(path: &str) -> Result<GgufTokenizerInfo, GgufError> {
    let fd = std::fs_open(path, std::FS_READ).map_err(|_| GgufError::FileNotFound)?;
    let mut stream = GgufStream::buffered_from_file(fd);
    let res = read_tokenizer_info_stream(&mut stream);
    let _ = std::fs_close(fd);
    res
}

/// One pair of the walk's metadata pass.
///
/// WHY the truncation handling is a macro: the pair being read is named by the key
/// the walk is holding at that moment, and a `?` would drop both the key and
/// everything the walk had already read.
macro_rules! tokenizer_pair {
    ($result:expr, $key:expr, $info:expr) => {
        match $result {
            Ok(value) => value,
            Err(GgufError::BufferTooShort) | Err(GgufError::ReadFailed) => {
                $info.truncated_in = Some($key);
                return Ok(());
            }
            Err(other) => return Err(other),
        }
    };
}

fn walk_tokenizer_kv(
    stream: &mut GgufStream,
    kv_count: u64,
    info: &mut GgufTokenizerInfo,
) -> Result<(), GgufError> {
    for index in 0..kv_count {
        if info.is_satisfied() {
            break;
        }
        let key_len = tokenizer_pair!(stream.read_u64(), KEY_PAIR, info) as usize;
        if key_len == 0 || key_len > MAX_METADATA_KEY_LEN {
            tokenizer_pair!(stream.skip(key_len), KEY_PAIR, info);
            let val_type = tokenizer_pair!(stream.read_u32(), KEY_PAIR, info);
            tokenizer_pair!(
                skip_metadata_value_stream(stream, val_type),
                KEY_PAIR,
                info
            );
            info.pairs_walked = index + 1;
            continue;
        }

        let mut key_buf = [0u8; MAX_METADATA_KEY_LEN];
        tokenizer_pair!(
            stream.read_exact(&mut key_buf[..key_len]),
            KEY_PAIR,
            info
        );
        let key = &key_buf[..key_len];
        let val_type = tokenizer_pair!(stream.read_u32(), KEY_PAIR, info);

        if key == KEY_TOKENS {
            if val_type != 9 {
                return Err(GgufError::InvalidMetadata);
            }
            let elem_type = tokenizer_pair!(stream.read_u32(), KEY_TOKENS, info);
            let stated = tokenizer_pair!(stream.read_u64(), KEY_TOKENS, info);
            if elem_type != 8 {
                return Err(GgufError::InvalidMetadata);
            }
            let elements = match u32::try_from(stated) {
                Ok(elements) => elements,
                // A vocabulary this wide cannot be stepped over by size either, so
                // the walk says the count is beyond naming and stops.
                Err(_) => {
                    info.vocab = VocabReading::BeyondCount { stated };
                    info.pairs_walked = index + 1;
                    return Ok(());
                }
            };
            info.vocab = VocabReading::Counted { elements };
            if !info.is_satisfied() {
                if let Err(e) = step_over_string_array(stream, elements as u64) {
                    match e {
                        GgufError::BufferTooShort | GgufError::ReadFailed => {
                            info.truncated_in = Some(KEY_TOKENS);
                            info.vocab = VocabReading::Truncated {
                                stated: elements,
                            };
                            info.pairs_walked = index + 1;
                            return Ok(());
                        }
                        other => return Err(other),
                    }
                }
            }
        } else if key == KEY_BOS_ID {
            info.bos_token_id = Some(tokenizer_pair!(
                read_count(stream, val_type),
                KEY_BOS_ID,
                info
            ));
        } else if key == KEY_EOS_ID {
            info.eos_token_id = Some(tokenizer_pair!(
                read_count(stream, val_type),
                KEY_EOS_ID,
                info
            ));
        } else if key == KEY_PAD_ID {
            info.pad_token_id = Some(tokenizer_pair!(
                read_count(stream, val_type),
                KEY_PAD_ID,
                info
            ));
        } else if key == KEY_ADD_BOS {
            info.add_bos_token = Some(tokenizer_pair!(read_bool(stream, val_type), KEY_ADD_BOS, info));
        } else if key == KEY_PRE {
            info.pre_reading = tokenizer_pair!(
                read_bounded_string(stream, val_type, &mut info.pre),
                KEY_PRE,
                info
            );
        } else if key == KEY_CHAT_TEMPLATE {
            info.template_reading = tokenizer_pair!(
                read_bounded_string(stream, val_type, &mut info.chat_template),
                KEY_CHAT_TEMPLATE,
                info
            );
        } else {
            tokenizer_pair!(
                skip_metadata_value_stream(stream, val_type),
                KEY_PAIR,
                info
            );
        }

        info.pairs_walked = index + 1;
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

/// Reports what a directory's own listing states about one of its files.
///
/// WHY the listing and not a fresh open: the question being asked is "how many
/// bytes does the device say it holds", and the answer a directory gives is the
/// same number a reader of that tree sees. Opening the file to ask would also
/// create it if it were not there.
pub fn listed_size(dir: &str, name: &str) -> Option<u64> {
    let mut listing = [0u8; 512];
    let Ok(read) = std::fs_list(dir, &mut listing) else {
        return None;
    };
    for line in listing[..read].split(|byte| *byte == b'\n') {
        let mut fields = line.split(|byte| *byte == b'|');
        if fields.next() == Some(name.as_bytes()) {
            return fields
                .next()
                .and_then(|text| core::str::from_utf8(text).ok())
                .and_then(|text| text.parse::<u64>().ok());
        }
    }
    None
}

/// Reports the bytes the model namespace says it holds for the installed file.
///
/// WHY it is spelled out from the directory and the name: a listing answers
/// about one entry of one directory, while `PATH_LOCAL_MODEL_DATA` names the
/// whole path the loader opens. Keeping the split here means the measurement and
/// the path can only ever disagree if someone edits one of the two constants.
pub fn local_model_size() -> Option<u64> {
    listed_size("/data/models", "local_model.gguf")
}

/// The model file an install may take, as a document the device already holds.
///
/// Writes the full source path into `out` and reports its length beside the
/// size the directory's own listing stated, or reports nothing when no file in
/// either tree is a model file.
///
/// WHY the header is read before the file is offered: an install that copied
/// first would place bytes in the model namespace that the platform then has to
/// call weights without having read a single one of them. Parsing the header is
/// the cheapest check that says whether the file claims to be a model at all.
///
/// WHY the listing's size is carried rather than measured later: the copy is
/// finished when the model namespace holds that many bytes, and a report of
/// "installed" needs a number the device already had.
pub fn find_installable_model(out: &mut [u8]) -> Option<(usize, u64)> {
    let mut listing = [0u8; 512];
    for &tree in MODEL_SOURCE_TREES {
        let Ok(read) = std::fs_list(tree, &mut listing) else {
            continue;
        };
        let tree_bytes = tree.as_bytes();
        for line in listing[..read].split(|byte| *byte == b'\n') {
            // The platform's directory line is `name|size|kind`.
            let mut fields = line.split(|byte| *byte == b'|');
            let (Some(name), Some(size)) = (fields.next(), fields.next()) else {
                continue;
            };
            if name.len() < 5 || !name.ends_with(b".gguf") {
                continue;
            }
            let total = tree_bytes.len() + 1 + name.len();
            if total + 1 > out.len() {
                continue;
            }
            out[..tree_bytes.len()].copy_from_slice(tree_bytes);
            out[tree_bytes.len()] = b'/';
            out[tree_bytes.len() + 1..total].copy_from_slice(name);
            let Ok(path) = core::str::from_utf8(&out[..total]) else {
                continue;
            };
            if read_gguf_header(path).is_err() {
                continue;
            }
            let stated = core::str::from_utf8(size)
                .ok()
                .and_then(|text| text.parse::<u64>().ok())
                .unwrap_or(0);
            return Some((total, stated));
        }
    }
    None
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
    PATH_LOCAL_MODEL_DATA,
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
        // WHY the installed file is a candidate: the Agent Manager's own
        // removal contract names `/data/models/local_model.gguf` as where a
        // loose model file sits once the device puts it there. A probe that
        // never looked at that path would answer "no weights" about a file the
        // screen had just offered to remove.
        PATH_LOCAL_MODEL_DATA,
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

    fn put_u32_at(buf: &mut [u8], off: &mut usize, value: u32) {
        buf[*off..*off + 4].copy_from_slice(&value.to_le_bytes());
        *off += 4;
    }

    fn put_u64_at(buf: &mut [u8], off: &mut usize, value: u64) {
        buf[*off..*off + 8].copy_from_slice(&value.to_le_bytes());
        *off += 8;
    }

    fn put_key(buf: &mut [u8], off: &mut usize, key: &[u8]) {
        put_u64_at(buf, off, key.len() as u64);
        buf[*off..*off + key.len()].copy_from_slice(key);
        *off += key.len();
    }

    fn put_string(buf: &mut [u8], off: &mut usize, key: &[u8], value: &[u8]) {
        put_key(buf, off, key);
        put_u32_at(buf, off, 8);
        put_u64_at(buf, off, value.len() as u64);
        buf[*off..*off + value.len()].copy_from_slice(value);
        *off += value.len();
    }

    fn put_count(buf: &mut [u8], off: &mut usize, key: &[u8], value: u32) {
        put_key(buf, off, key);
        put_u32_at(buf, off, 4);
        put_u32_at(buf, off, value);
    }

    fn put_float(buf: &mut [u8], off: &mut usize, key: &[u8], value: f32) {
        put_key(buf, off, key);
        put_u32_at(buf, off, 6);
        put_u32_at(buf, off, value.to_bits());
    }

    /// Writes a string array declaring `count` elements, whose element bytes the
    /// caller may leave unwritten when the reader is expected to stop at it.
    fn put_vocab(buf: &mut [u8], off: &mut usize, key: &[u8], count: u64, tokens: &[&[u8]]) {
        put_key(buf, off, key);
        put_u32_at(buf, off, 9);
        put_u32_at(buf, off, 8);
        put_u64_at(buf, off, count);
        for token in tokens {
            put_u64_at(buf, off, token.len() as u64);
            buf[*off..*off + token.len()].copy_from_slice(token);
            *off += token.len();
        }
    }

    /// Starts a metadata-only GGUF file with `kv_count` pairs to follow.
    fn begin_file(buf: &mut [u8], kv_count: u64) -> usize {
        let header = GgufHeader {
            magic: GGUF_MAGIC,
            version: 3,
            tensor_count: 0,
            kv_count,
        };
        header.write_to(buf).expect("header fits");
        GgufHeader::HEADER_SIZE
    }

    #[test]
    fn test_read_model_info_takes_the_dimensions_from_the_file() {
        let mut buf = [0u8; 512];
        let mut off = begin_file(&mut buf, 10);
        put_string(&mut buf, &mut off, b"general.architecture", b"gemma2");
        put_string(&mut buf, &mut off, b"general.name", b"Gemma 4 2B It");
        put_count(&mut buf, &mut off, b"gemma2.embedding_length", 64);
        put_count(&mut buf, &mut off, b"gemma2.feed_forward_length", 128);
        put_count(&mut buf, &mut off, b"gemma2.block_count", 2);
        put_count(&mut buf, &mut off, b"gemma2.attention.head_count", 2);
        put_count(&mut buf, &mut off, b"gemma2.attention.head_count_kv", 1);
        put_count(&mut buf, &mut off, b"gemma2.attention.head_length", 32);
        put_float(&mut buf, &mut off, b"gemma2.attention.layer_norm_rms_epsilon", 1e-6);
        // The vocabulary is declared as 290 entries and none of them are written:
        // the reader takes the width from the array's header and stops, so a file
        // whose token list is absent past that header still configures the engine.
        put_vocab(&mut buf, &mut off, b"tokenizer.ggml.tokens", 290, &[]);

        let info = read_model_info_stream(&mut GgufStream::from_slice(&buf[..off]))
            .expect("the file declares every dimension the engine asks for");
        assert_eq!(info.architecture(), Some("gemma2"));
        assert_eq!(info.hidden_dim, Some(64));
        assert_eq!(info.intermediate_dim, Some(128));
        assert_eq!(info.num_layers, Some(2));
        assert_eq!(info.num_heads, Some(2));
        assert_eq!(info.num_kv_heads, Some(1));
        assert_eq!(info.head_dim, Some(32));
        assert_eq!(info.rms_norm_eps, Some(1e-6));
        assert_eq!(info.vocab_size, Some(290));
        assert_eq!(info.missing_key(), None);
    }

    #[test]
    fn test_read_model_info_takes_a_head_width_named_by_the_key_file() {
        let mut buf = [0u8; 512];
        let mut off = begin_file(&mut buf, 8);
        put_string(&mut buf, &mut off, b"general.architecture", b"gemma");
        put_count(&mut buf, &mut off, b"gemma.embedding_length", 64);
        put_count(&mut buf, &mut off, b"gemma.feed_forward_length", 128);
        put_count(&mut buf, &mut off, b"gemma.block_count", 1);
        put_count(&mut buf, &mut off, b"gemma.attention.head_count", 2);
        put_count(&mut buf, &mut off, b"gemma.attention.key_head_length", 32);
        put_float(&mut buf, &mut off, b"gemma.rope.freq_base", 1_000_000.0);
        put_vocab(&mut buf, &mut off, b"tokenizer.ggml.tokens", 290, &[]);

        let info = read_model_info_stream(&mut GgufStream::from_slice(&buf[..off]))
            .expect("a Gemma file names its head width per key");
        assert_eq!(info.head_dim, Some(32));
        assert_eq!(info.rope_theta, Some(1_000_000.0));
        assert_eq!(info.missing_key(), None);
    }

    #[test]
    fn test_read_model_info_names_the_key_a_file_never_declares() {
        let mut buf = [0u8; 512];
        let mut off = begin_file(&mut buf, 5);
        put_string(&mut buf, &mut off, b"general.architecture", b"gemma2");
        put_count(&mut buf, &mut off, b"gemma2.embedding_length", 64);
        put_count(&mut buf, &mut off, b"gemma2.attention.head_count", 2);
        put_count(&mut buf, &mut off, b"gemma2.attention.head_length", 32);
        put_vocab(
            &mut buf,
            &mut off,
            b"tokenizer.ggml.tokens",
            2,
            &[b"hello", b"world"],
        );

        let result = read_model_info_stream(&mut GgufStream::from_slice(&buf[..off]));
        match result {
            Err(GgufError::MissingMetadata(key, partial)) => {
                assert_eq!(key, "<arch>.feed_forward_length");
                assert_eq!(partial.architecture(), Some("gemma2"));
                assert_eq!(partial.hidden_dim, Some(64));
                assert_eq!(partial.vocab_size, Some(2));
            }
            other => panic!("a file that states no feed-forward width must say so, got {:?}", other),
        }
    }

    #[test]
    fn test_read_model_info_refuses_to_walk_a_vocabulary_to_reach_a_later_key() {
        let mut buf = [0u8; 512];
        let mut off = begin_file(&mut buf, 4);
        put_string(&mut buf, &mut off, b"general.architecture", b"gemma2");
        put_count(&mut buf, &mut off, b"gemma2.embedding_length", 64);
        put_vocab(&mut buf, &mut off, b"tokenizer.ggml.tokens", 256_000, &[]);
        let after_vocab_header = off;
        put_count(&mut buf, &mut off, b"gemma2.block_count", 26);

        let mut stream = GgufStream::from_slice(&buf[..off]);
        let result = read_model_info_stream(&mut stream);
        match result {
            Err(GgufError::MissingMetadata(key, partial)) => {
                assert_eq!(key, "<arch>.feed_forward_length");
                assert_eq!(partial.vocab_size, Some(256_000));
            }
            other => panic!("256,000 tokens must not be walked to reach one key, got {:?}", other),
        }
        // WHY this is the assertion that matters: the walk would cost two reads per
        // token, and on a file the kernel holds on the data disk every one of those
        // remounts and rescans the inode table.
        assert_eq!(stream.position(), after_vocab_header);
    }

    #[test]
    fn test_read_model_info_rejects_a_vocabulary_that_is_not_strings() {
        let mut buf = [0u8; 256];
        let mut off = begin_file(&mut buf, 1);
        put_key(&mut buf, &mut off, b"tokenizer.ggml.tokens");
        put_u32_at(&mut buf, &mut off, 9);
        put_u32_at(&mut buf, &mut off, 4);
        put_u64_at(&mut buf, &mut off, 8);

        assert_eq!(
            read_model_info_stream(&mut GgufStream::from_slice(&buf[..off])),
            Err(GgufError::InvalidMetadata)
        );
    }

    // --- the tokenizer walk ---------------------------------------------------
    //
    // WHY these builders grow a vector instead of filling a fixed array: the shape
    // being tested is a real Gemma file's, and a real Gemma file puts 256,000
    // length-prefixed strings in front of the key the walk has to reach. That is
    // megabytes of metadata, which no stack window holds -- the walk reads it
    // through its own window, and only the test fixture needs room.
    extern crate std as host_std;

    fn v_key(out: &mut host_std::vec::Vec<u8>, key: &[u8]) {
        out.extend_from_slice(&(key.len() as u64).to_le_bytes());
        out.extend_from_slice(key);
    }

    fn v_string(out: &mut host_std::vec::Vec<u8>, key: &[u8], value: &[u8]) {
        v_key(out, key);
        out.extend_from_slice(&8u32.to_le_bytes());
        out.extend_from_slice(&(value.len() as u64).to_le_bytes());
        out.extend_from_slice(value);
    }

    fn v_u32(out: &mut host_std::vec::Vec<u8>, key: &[u8], value: u32) {
        v_key(out, key);
        out.extend_from_slice(&4u32.to_le_bytes());
        out.extend_from_slice(&value.to_le_bytes());
    }

    /// Writes GGUF's one-byte boolean, type 7.
    fn v_bool(out: &mut host_std::vec::Vec<u8>, key: &[u8], value: bool) {
        v_key(out, key);
        out.extend_from_slice(&7u32.to_le_bytes());
        out.push(if value { 1 } else { 0 });
    }

    /// Writes a string array of `elements` tokens, each `width` bytes, and returns
    /// how many pairs the file now declares.
    fn v_vocab(out: &mut host_std::vec::Vec<u8>, elements: u64, width: usize) {
        v_key(out, KEY_TOKENS);
        out.extend_from_slice(&9u32.to_le_bytes());
        out.extend_from_slice(&8u32.to_le_bytes());
        out.extend_from_slice(&elements.to_le_bytes());
        for index in 0..elements {
            out.extend_from_slice(&(width as u64).to_le_bytes());
            for byte in 0..width {
                out.push(b'a' + ((index as usize + byte) % 26) as u8);
            }
        }
    }

    fn begin_vec_file(out: &mut host_std::vec::Vec<u8>, kv_count: u64) {
        let header = GgufHeader {
            magic: GGUF_MAGIC,
            version: 3,
            tensor_count: 0,
            kv_count,
        };
        let mut buf = [0u8; GgufHeader::HEADER_SIZE];
        header.write_to(&mut buf).expect("header fits");
        out.extend_from_slice(&buf);
    }

    /// A file shaped the way a Gemma conversion leaves one: the vocabulary first,
    /// the chat template after it, and the tokenizer's scalars beside them.
    fn gemma_shaped_file(elements: u64, template: &[u8]) -> host_std::vec::Vec<u8> {
        let mut out = host_std::vec::Vec::new();
        // architecture, 3 token ids, add_bos, pre, tokens, template, one trailing key
        begin_vec_file(&mut out, 9);
        v_string(&mut out, b"general.architecture", b"gemma2");
        v_u32(&mut out, KEY_BOS_ID, 2);
        v_u32(&mut out, KEY_EOS_ID, 1);
        v_u32(&mut out, KEY_PAD_ID, 0);
        v_bool(&mut out, KEY_ADD_BOS, true);
        v_string(&mut out, KEY_PRE, b"gemma");
        v_vocab(&mut out, elements, 4);
        v_string(&mut out, KEY_CHAT_TEMPLATE, template);
        v_u32(&mut out, b"general.file_type", 2);
        out
    }

    #[test]
    fn test_tokenizer_walk_counts_a_vocabulary_far_beyond_its_own_load_cap() {
        let data = gemma_shaped_file(256_000, b"<start_of_turn>{{prompt}}");
        let mut stream = GgufStream::buffered_from_slice(&data);
        let info = read_tokenizer_info_stream(&mut stream).expect("the walk reaches both keys");

        // The count comes from the array's own header and no element was loaded.
        assert_eq!(info.vocab, VocabReading::Counted { elements: 256_000 });
        assert_eq!(info.vocab_size(), Some(256_000));
        assert!(info.vocab_counted_not_loaded());

        // WHY this is the assertion that carries the ticket: the key sits after
        // 256,000 strings, and reaching it must not cost a request per string.
        assert_eq!(
            info.chat_template(),
            Some(&b"<start_of_turn>{{prompt}}"[..]),
            "the template after the vocabulary was not reached"
        );
        assert_eq!(info.template_reading, StringReading::Read(25));
        assert_eq!(info.pre_name(), Some("gemma"));
        assert_eq!(info.bos_token_id, Some(2));
        assert_eq!(info.eos_token_id, Some(1));
        assert_eq!(info.pad_token_id, Some(0));
        assert_eq!(info.add_bos_token, Some(true));
        assert_eq!(info.truncated_in, None);

        // One window per 512 bytes, not two requests per token: the array alone is
        // over 3 megabytes, so the walk that paid per element would be measured in
        // hundreds of thousands of requests.
        let array_bytes = 256_000usize * (8 + 4);
        assert!(
            info.pulls * METADATA_WINDOW_BYTES >= array_bytes / 2,
            "the window did not stream the array: {} pulls",
            info.pulls
        );
        assert!(
            info.pulls < (256_000 / 8) as usize,
            "the walk paid a request per token: {} pulls",
            info.pulls
        );
    }

    #[test]
    fn test_tokenizer_walk_reports_a_file_that_ends_inside_the_vocabulary() {
        let mut data = gemma_shaped_file(256_000, b"{{prompt}}");
        // Cut the file six bytes into a token's payload, inside the array.
        let cut = GgufHeader::HEADER_SIZE + 200_000;
        data.truncate(cut);

        let mut stream = GgufStream::buffered_from_slice(&data);
        let info = read_tokenizer_info_stream(&mut stream)
            .expect("a file that stops mid-array is reported, not a panic");
        assert!(
            matches!(info.vocab, VocabReading::Truncated { stated } if stated == 256_000),
            "the walk lost the count the array stated: {:?}",
            info.vocab
        );
        assert_eq!(info.truncated_in, Some(KEY_TOKENS));
        // Everything reached before the cut stays said.
        assert_eq!(info.bos_token_id, Some(2));
        assert_eq!(info.pre_name(), Some("gemma"));
        // And the template past the cut was not invented.
        assert_eq!(info.template_reading, StringReading::Absent);
        assert_eq!(info.chat_template(), None);
    }

    #[test]
    fn test_tokenizer_walk_refuses_a_template_over_its_cap_with_the_length_it_stated() {
        // One byte past what the reader holds, so nothing may be copied.
        let template = [b'x'; MAX_CHAT_TEMPLATE_LEN + 1];
        let mut out = host_std::vec::Vec::new();
        begin_vec_file(&mut out, 3);
        v_string(&mut out, KEY_PRE, b"gemma");
        v_string(&mut out, KEY_CHAT_TEMPLATE, &template);
        v_u32(&mut out, b"general.file_type", 2);

        let mut stream = GgufStream::buffered_from_slice(&out);
        let info = read_tokenizer_info_stream(&mut stream).expect("a long template is refused, not fatal");
        assert_eq!(
            info.template_reading,
            StringReading::RefusedTooLong(MAX_CHAT_TEMPLATE_LEN as u64 + 1)
        );
        assert_eq!(info.chat_template(), None, "a cut template must not be quoted");
        // The value was stepped past, so the walk kept going and read the next key.
        assert_eq!(info.pre_name(), Some("gemma"));
        assert_eq!(info.pairs_walked, 3);
        // Refused means the platform's own template is in use, and it says so.
        assert_eq!(
            info.active_chat_template().1,
            TemplateSource::BuiltInDefault
        );
    }

    #[test]
    fn test_tokenizer_walk_reads_a_flag_as_bool_type_7_and_refuses_any_other_width() {
        let mut out = host_std::vec::Vec::new();
        begin_vec_file(&mut out, 2);
        v_bool(&mut out, KEY_ADD_BOS, false);
        v_string(&mut out, KEY_CHAT_TEMPLATE, b"{prompt}");
        let mut stream = GgufStream::buffered_from_slice(&out);
        let info = read_tokenizer_info_stream(&mut stream).expect("a declared false is a reading");
        assert_eq!(info.add_bos_token, Some(false), "type 7 must be read as a flag");

        // The same key declared as a float is not a boolean the reader will guess.
        let mut wide = host_std::vec::Vec::new();
        begin_vec_file(&mut wide, 1);
        v_key(&mut wide, KEY_ADD_BOS);
        wide.extend_from_slice(&6u32.to_le_bytes());
        wide.extend_from_slice(&1.0f32.to_bits().to_le_bytes());
        assert_eq!(
            read_tokenizer_info_stream(&mut GgufStream::buffered_from_slice(&wide)),
            Err(GgufError::InvalidMetadata)
        );

        // Nor is a type-7 byte that is neither 0 nor 1.
        let mut corrupt = host_std::vec::Vec::new();
        begin_vec_file(&mut corrupt, 1);
        v_key(&mut corrupt, KEY_ADD_BOS);
        corrupt.extend_from_slice(&7u32.to_le_bytes());
        corrupt.push(17);
        assert_eq!(
            read_tokenizer_info_stream(&mut GgufStream::buffered_from_slice(&corrupt)),
            Err(GgufError::InvalidMetadata)
        );
    }

    #[test]
    fn test_tokenizer_walk_reports_a_file_that_declares_no_tokenizer_but_still_loads() {
        let mut out = host_std::vec::Vec::new();
        begin_vec_file(&mut out, 2);
        v_string(&mut out, b"general.architecture", b"gemma2");
        v_u32(&mut out, b"gemma2.embedding_length", 64);

        let mut stream = GgufStream::buffered_from_slice(&out);
        let info = read_tokenizer_info_stream(&mut stream).expect("no tokenizer is a reading, not a failure");
        assert!(info.declares_no_tokenizer());
        assert_eq!(info.vocab, VocabReading::Absent);
        assert_eq!(info.truncated_in, None);
        assert_eq!(
            info.active_chat_template().1,
            TemplateSource::BuiltInDefault,
            "the engine runs the built-in default and must say that is what it runs"
        );
    }

    #[test]
    fn test_tokenizer_walk_names_its_keys_whole_and_ignores_another_family() {
        // WHY this is its own case: the configuration reader matches a key by the
        // segment after its last dot, so another family's `.pre` would land in the
        // tokenizer's field. The walk compares every byte instead.
        let mut out = host_std::vec::Vec::new();
        begin_vec_file(&mut out, 2);
        v_string(&mut out, b"some.other.pre", b"not-the-tokenizer");
        v_u32(&mut out, b"tokenizer.ggml.something.padding_token_id", 7);
        let mut stream = GgufStream::buffered_from_slice(&out);
        let info = read_tokenizer_info_stream(&mut stream).expect("unrelated keys cost a step, not a claim");
        assert_eq!(info.pre_name(), None);
        assert_eq!(info.pad_token_id, None);
        assert!(info.declares_no_tokenizer());
    }

    #[test]
    fn test_prompt_formatting_splices_at_the_marker_and_reports_why_it_could_not() {
        let mut out = [0u8; 256];

        // The file's own template wins when the walk held it.
        let mut file = host_std::vec::Vec::new();
        begin_vec_file(&mut file, 1);
        v_string(&mut file, KEY_CHAT_TEMPLATE, b"[user] {prompt} [/user]");
        let info = read_tokenizer_info_stream(&mut GgufStream::buffered_from_slice(&file))
            .expect("one pair is a whole file for this case");
        assert_eq!(
            info.active_chat_template().1,
            TemplateSource::FromFile,
            "the file's own template is what formats the prompt"
        );
        let written = info.format_prompt(b"hello", &mut out).expect("fits");
        assert_eq!(&out[..written], b"[user] hello [/user]");

        // A template with nowhere for the text to go is reported, not guessed at.
        let mut markerless = host_std::vec::Vec::new();
        begin_vec_file(&mut markerless, 1);
        v_string(&mut markerless, KEY_CHAT_TEMPLATE, b"<user> only a role");
        let info = read_tokenizer_info_stream(&mut GgufStream::buffered_from_slice(&markerless))
            .expect("a template without a marker still reads");
        assert_eq!(info.format_prompt(b"hello", &mut out), Err(PromptFit::NoMarker));

        // The built-in default applies to a file that named none, and says so.
        let none = GgufTokenizerInfo::empty();
        assert!(none.declares_no_tokenizer());
        let written = none.format_prompt(b"hi", &mut out).expect("default fits");
        assert_eq!(
            &out[..written],
            b"<start_of_turn>user\nhi<end_of_turn>\n<start_of_turn>model\n"
        );
        assert_eq!(
            none.active_chat_template().0,
            DEFAULT_CHAT_TEMPLATE.as_bytes()
        );

        // Too small a buffer is reported with the size that would have fitted.
        let mut tiny = [0u8; 8];
        assert_eq!(
            none.format_prompt(b"a prompt longer than eight bytes", &mut tiny),
            Err(PromptFit::TooLong {
                needed: DEFAULT_CHAT_TEMPLATE.len() - PROMPT_MARKER.len() + 32,
            })
        );
    }

    #[test]
    fn test_buffered_window_steps_by_size_and_counts_its_requests() {
        let data = [7u8; 3 * METADATA_WINDOW_BYTES + 40];
        let mut stream = GgufStream::buffered_from_slice(&data);

        let mut head = [0u8; 4];
        stream.read_exact(&mut head).expect("the window serves a read");
        assert_eq!(head, [7u8; 4]);

        stream.skip(100).expect("a step is served from the window");
        assert_eq!(stream.position(), 104);

        // The whole source is 1576 bytes; the window touches it a handful of times.
        let mut rest = [0u8; 1_000];
        stream.read_exact(&mut rest).expect("a request past the window still reads");
        assert_eq!(stream.position(), 1_104);
        // Between the requests a full window per pull allows and a small margin:
        // a reader that asked for one value at a time would be off by two orders.
        assert!(
            stream.pulls() >= data.len() / METADATA_WINDOW_BYTES
                && stream.pulls() <= data.len() / METADATA_WINDOW_BYTES + 2,
            "the window made {} requests for {} bytes",
            stream.pulls(),
            data.len()
        );

        // And past the end of the source the window reports the file ran out.
        assert_eq!(stream.skip(600), Err(GgufError::BufferTooShort));
    }
}
