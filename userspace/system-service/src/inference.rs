//! Real neural inference engine and model lifecycle for InterOS AI service (`aid`).
//!
//! Strictly `#![no_std]`. The graph's per-run footprint -- the KV cache and every
//! activation buffer -- is carved from the service's own memory capability
//! (`CapType::MemoryHeap`, granted at `kernel/src/services.rs`), not from the
//! calling thread's stack, so the engine is sized by what the model file declares
//! instead of by what a 256 KiB user stack happens to hold. The allocator behind
//! those buffers is `interos_std::heap::InterOsHeap`, declared once for this
//! crate at `src/lib.rs`; the capability that makes its mappings succeed is the
//! one `aid` holds, and no other daemon in the crate reaches a path that
//! allocates.
//! Implements real tensor operations:
//! - Dequantization (F32, F16, Q4_0, Q8_0, Q4_K to F32); any other type id is refused by number
//! - Matrix-vector multiplication (GEMV) with on-the-fly row dequantization
//! - RMSNorm (Root Mean Square Layer Normalization)
//! - RoPE (Rotary Positional Embeddings)
//! - SwiGLU non-linear activation
//! - Multi-Head Attention dot-product & softmax over the one key/value head
//! - Autoregressive token generation loop with timeout / cancellation checks
//! - Neural vector embedding extraction (EmbeddingGemma 2): mean pool, then L2
//!
//! Nothing in the forward path falls back. A declaration the file has not stated,
//! a quantisation the engine has no layout for, an embedding row the file does not
//! hold, or a shape whose scratch is over budget is reported as the refusal it is.
//! An invented stand-in would let an answer computed from nothing be reported as
//! the installed model's answer.

use alloc::vec::Vec;

/// Bytes of heap one inference run may be given, and the only size limit the
/// engine now carries.
///
/// WHY one computed budget instead of six compile-time maxima: the maxima
/// (`hidden 256, ffn 512, vocab 320, layers 2, head 32, sequence 64`) sat below
/// every model that has ever been published, so the engine could only ever refuse
/// them. What actually bounds a run is the memory the service can be handed, and
/// that is a byte count, not a per-dimension ceiling -- `ModelConfig::scratch_bytes`
/// computes what a declared shape needs and this constant says what the device
/// offers.
///
/// THE ARITHMETIC, from the device demo's 2 GiB of RAM and 6 GiB of storage:
///
/// ```text
///   device RAM                                    2,048 MiB
///   - kernel heap (kernel/src/allocator.rs)          64 MiB
///   - kernel image, page tables, frame metadata      32 MiB
///   - display: two 720x1440 BGRA buffers          ~  8 MiB
///   - the other 31 userspace tasks and the page cache
///     are left the rest, and one service taking a
///     twentieth of the machine is where it stops -> 128 MiB
/// ```
///
/// 128 MiB is also the ceiling the platform itself puts on one mapping, and the
/// budget has to live under it rather than at it:
/// - `sys_mem_map` refuses any single mapping above 128 MiB
///   (`kernel/src/syscall/mod.rs`), and the heap serves one big buffer from one
///   mapping plus a 16-byte block header and page rounding;
/// - the address space a process may hold in total is its own resource limit, and
///   `ServiceId::Inference` is budgeted 2 GiB of it
///   (`kernel/src/services.rs::resource_limits`) -- the 256 MiB
///   `USER_ADDRESS_SPACE_LIMIT` (`kernel/src/task/user.rs`) is the default for a
///   process that states no limit, and it bounds what the loaded `.rapp`, its
///   256 KiB stack and this service's other mappings would have to share.
///
/// So the budget is the mapping ceiling less an 8 MiB reservation for the header,
/// the guard pages and the rest of the footprint that shares the address space:
/// 128 MiB - 8 MiB = 120 MiB = 125,829,120 bytes. One run's largest single
/// buffer, the 48 MiB K tensor, is well under the mapping ceiling, and the whole
/// run is well under the service's address-space budget.
///
/// The published `embeddinggemma-300M-Q8_0.gguf` -- 24 blocks, 2048 positions,
/// one key/value head 256 wide, 768-wide stream, 1152-wide feed-forward and
/// 262,144 tokens -- computes 101,767,680 bytes under this formula: the KV cache
/// is 2 x 24 x 2048 x 1 x 256 x 4 = 100,663,296 (96 MiB) and the rest is
/// 11 x 768 + 3 x 1152 + 2048 + 262,144 = 276,096 floats = 1,104,384 bytes. It
/// fits with 24 MiB to spare, and the tests
/// `test_model_config_accepts_the_published_embedding_model_by_its_budget` and
/// `test_the_real_published_file_is_priced_and_allocated` hold the arithmetic to
/// that number -- the second one against the 333 MB file's own bytes, so the
/// fixture's numbers are the file's and not whoever typed them in.
///
/// It fits in memory; it does not yet run, because the graph and the weight
/// loader for it are separate work.
pub const SCRATCH_BYTE_BUDGET: usize = 120 * 1024 * 1024;

/// Bytes one f32 occupies: every buffer the engine sizes is f32.
pub const FLOAT_BYTES: usize = core::mem::size_of::<f32>();

/// Error types for inference execution.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InferenceError {
    /// The GGML type id the tensor declares. Only the ids `gemv` dequantizes reach
    /// the arithmetic, so a type whose layout the engine has not been given
    /// authoritatively is refused by number instead of read with a guess.
    /// Q4_K is the fifth id it knows; Q4_1, Q5_K and Q6_K are not among them.
    UnsupportedQuantization(u32),
    CorruptedWeights,
    BufferTooSmall,
    ContextLengthExceeded,
    Cancelled,
    ModelUnavailable,
    /// A declaration in the model file the engine cannot be built from: the key
    /// it was declared under, what the file stated (`None` when the file states
    /// no such key), and the bound the engine is compiled to. After the scratch
    /// budget there is no per-dimension bound left, so the third field carries
    /// the bound only where one exists -- the single key/value head the attention
    /// kernel walks -- and 0 where the file simply did not state the number.
    ModelDeclaration(&'static str, Option<u32>, usize),
    /// The shape the file declares needs more scratch than the device offers.
    /// Carries both byte counts: what this model would have taken, and the budget
    /// it was refused against. A refusal that named neither would leave the next
    /// stage guessing whether it needs a smaller model or a bigger machine.
    ScratchTooLarge(usize, usize),
    /// The heap refused the scratch the shape was accepted for. The budget said
    /// the device can hold it; this says it could not be handed out right now,
    /// which is the kernel's `MemoryHeap` mapping failing, not the model's shape.
    ScratchUnavailable,
}

// -----------------------------------------------------------------------------
// The Model's Identity, as the File States It
// -----------------------------------------------------------------------------

/// How the reading answered one string field of the file's self-description.
///
/// WHY four states and not an `Option<&str>`: `None` cannot tell the file's
/// silence from the reader's stopping point, and those are different answers to
/// give a person -- one says the file names no model, the other says the walk
/// stopped at a 262,144-entry vocabulary before it got there. A name wider than
/// the reader's window is a third answer again (the file did state one, and not
/// one byte of it was copied), and bytes that are not text a fourth. Collapsing
/// any of them into "absent" reports a silence the file never had, and the whole
/// reason this type exists is that the report has to be the file's.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StrField<'a> {
    /// The file declared it and the reader holds what it said, verbatim.
    Declared(&'a str),
    /// The pairs the walk read name no such key.
    Absent,
    /// The file stated this many bytes, more than the reader holds; the walk
    /// stepped past the value and copied none of it.
    RefusedTooLong(u64),
    /// The file stated this many bytes and they are not text the report can quote.
    NotText(u64),
    /// The key sits past where the walk stops, so this reading cannot say whether
    /// the file names it.
    NotReached,
}

impl<'a> StrField<'a> {
    /// The text the file stated, and `None` for every other reading.
    ///
    /// WHY this is the only accessor a caller gets: the four refusals are
    /// different facts and a caller that flattened them into `Option` would be
    /// back to not knowing whether the file named nothing or the walk stopped
    /// early -- which is the difference this type exists to keep.
    pub fn held(self) -> Option<&'a str> {
        match self {
            Self::Declared(text) => Some(text),
            _ => None,
        }
    }
}

/// How the reading answered one integer field of the file's self-description.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NumField {
    /// The file stated this number under this key.
    Declared(u64),
    /// The metadata the walk read names no such key.
    Absent,
    /// Past the walk's stopping point -- see `StrField::NotReached`.
    NotReached,
}

impl NumField {
    /// The number the file stated, and nothing for a file that did not state one.
    ///
    /// WHY there is no `unwrap_or(0)` friend of this: a report that printed `0`
    /// where the file printed nothing hands the reader a number the file gave,
    /// which is the defect this type replaces.
    pub fn value(self) -> Option<u64> {
        match self {
            Self::Declared(number) => Some(number),
            _ => None,
        }
    }
}

/// What the model file says it is, taken from the file's own declarations.
///
/// WHY a type of its own: the service names a model in three places -- the record
/// the probe holds, the notice that explains why a model did not run, and anything
/// it logs about the file -- and every one of them used the path the bytes were
/// opened at to decide what to call them. A file at
/// `/data/models/embeddinggemma-2.gguf` that declares `general.name` "Falcon
/// Junior" is Falcon Junior, and the platform saying otherwise is not a label
/// choice but an invented model. This type has no path in its inputs, so there is
/// nothing here for a filename to leak from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ModelIdentity<'a> {
    /// `general.name`
    pub name: StrField<'a>,
    /// `general.architecture`, e.g. `gemma2`
    pub architecture: StrField<'a>,
    /// `tokenizer.ggml.model`
    pub tokenizer_model: StrField<'a>,
    /// `general.file_type` -- the quantisation the file declares for its tensors,
    /// by the number the spec gives that type. The number is the report: the
    /// platform has no table here that would call a `7` "Q8_0" and say so as
    /// though the file had named the quantisation in words.
    pub quantization_type: NumField,
    /// `general.quantization_version`
    pub quantization_version: NumField,
    /// Tensor descriptors the header counted. `Absent` here means the reading
    /// carries no header at all -- a walk over a file's metadata always has both
    /// counts, because the header is where the walk starts.
    pub tensor_count: NumField,
    /// Metadata pairs the header counted, which is the number that makes an
    /// unread key provable as unread instead of merely assumed absent.
    pub metadata_pair_count: NumField,
    /// `<arch>.embedding_length` -- the width the residual stream declares.
    pub embedding_length: NumField,
}

impl<'a> ModelIdentity<'a> {
    /// Reads the identity out of what the file declared, field by field.
    ///
    /// WHY an absent field stays absent: the alternative is a plausible-looking
    /// stand-in -- the filename's stem, the architecture's vendor name, the
    /// dimensions of whichever fixture the code was written against -- and the
    /// platform-truth rule says a thing the file did not state is reported as not
    /// stated, even when the invention would read better on the screen.
    pub fn from_gguf(info: &'a crate::gguf::GgufModelInfo) -> Self {
        let reached = !info.walk_stopped_early();
        Self {
            name: string_field(info.name_reading, info.name(), reached),
            architecture: string_field(info.architecture_reading, info.architecture(), reached),
            tokenizer_model: string_field(
                info.tokenizer_model_reading,
                info.tokenizer_model(),
                reached,
            ),
            quantization_type: count_field(info.file_type.map(u64::from), reached),
            quantization_version: count_field(info.quantization_version.map(u64::from), reached),
            tensor_count: info.tensor_count.map(NumField::Declared).unwrap_or(NumField::Absent),
            metadata_pair_count: info
                .kv_pair_count
                .map(NumField::Declared)
                .unwrap_or(NumField::Absent),
            embedding_length: count_field(info.hidden_dim.map(u64::from), reached),
        }
    }

    /// Whether the file named itself at all: no name, no architecture and no
    /// tokenizer family, each of them absent rather than unread.
    ///
    /// WHY that is worth a question: a header that parses over metadata naming
    /// nothing is a placeholder the install left behind, and the notice for it has
    /// to say "this file declares no identity" instead of printing a model name
    /// the platform supplied.
    pub fn declares_nothing(&self) -> bool {
        matches!(
            (self.name, self.architecture, self.tokenizer_model),
            (StrField::Absent, StrField::Absent, StrField::Absent)
        )
    }
}

/// One string the walk may or may not have held.
fn string_field<'a>(
    reading: crate::gguf::StringReading,
    held: Option<&'a str>,
    reached: bool,
) -> StrField<'a> {
    use crate::gguf::StringReading;
    match reading {
        StringReading::Absent => {
            if reached {
                StrField::Absent
            } else {
                StrField::NotReached
            }
        }
        StringReading::RefusedTooLong(stated) => StrField::RefusedTooLong(stated),
        StringReading::Read(len) => match held {
            Some(text) => StrField::Declared(text),
            None => StrField::NotText(len as u64),
        },
    }
}

/// One number the walk may or may not have reached.
fn count_field(declared: Option<u64>, reached: bool) -> NumField {
    match declared {
        Some(number) => NumField::Declared(number),
        None if reached => NumField::Absent,
        None => NumField::NotReached,
    }
}

// -----------------------------------------------------------------------------
// Software Math Functions (#![no_std] without external runtime dependencies)
// -----------------------------------------------------------------------------

pub use crate::embedding::f32_sqrt;

/// Fast, bounded floating-point exponential: e^x.
pub fn f32_exp(x: f32) -> f32 {
    if x <= -88.0 {
        return 0.0;
    }
    if x >= 88.0 {
        return f32::MAX;
    }
    let z = x * 1.4426950408889634f32; // x * log2(e)
    let k = if z >= 0.0 {
        (z + 0.5f32) as i32
    } else {
        (z - 0.5f32) as i32
    };
    let r = x - (k as f32) * 0.6931471805599453f32; // x - k * ln(2)

    // Taylor polynomial for e^r: 1 + r + r^2/2 + r^3/6 + r^4/24 + r^5/120 + r^6/720
    let r2 = r * r;
    let r3 = r2 * r;
    let r4 = r2 * r2;
    let poly = 1.0f32
        + r
        + r2 * 0.5f32
        + r3 * (1.0f32 / 6.0f32)
        + r4 * (1.0f32 / 24.0f32)
        + (r4 * r) * (1.0f32 / 120.0f32)
        + (r4 * r2) * (1.0f32 / 720.0f32);

    let exp_bits = (k + 127) as u32;
    if exp_bits == 0 || k < -126 {
        0.0
    } else if exp_bits >= 255 || k > 127 {
        f32::MAX
    } else {
        let scale = f32::from_bits(exp_bits << 23);
        poly * scale
    }
}

const PI: f32 = 3.14159265358979323846;
const TWO_PI: f32 = 6.28318530717958647692;

/// Range-reduced software sine.
pub fn f32_sin(mut x: f32) -> f32 {
    while x > PI {
        x -= TWO_PI;
    }
    while x < -PI {
        x += TWO_PI;
    }
    let x2 = x * x;
    let x3 = x2 * x;
    let x5 = x3 * x2;
    let x7 = x5 * x2;
    x - x3 * (1.0 / 6.0) + x5 * (1.0 / 120.0) - x7 * (1.0 / 5040.0)
}

/// Range-reduced software cosine.
pub fn f32_cos(mut x: f32) -> f32 {
    while x > PI {
        x -= TWO_PI;
    }
    while x < -PI {
        x += TWO_PI;
    }
    let x2 = x * x;
    let x4 = x2 * x2;
    let x6 = x4 * x2;
    1.0 - x2 * 0.5 + x4 * (1.0 / 24.0) - x6 * (1.0 / 720.0)
}

/// Approximate base^exp for RoPE frequencies.
pub fn f32_pow_approx(base: f32, exp: f32) -> f32 {
    if exp == 0.0 {
        return 1.0;
    }
    // ln(10000.0) ~ 9.21034037
    let ln_base = if (base - 10000.0).abs() < 1.0 {
        9.21034037f32
    } else {
        // General ln via log2(base) * ln(2)
        let bits = base.to_bits();
        let e = ((bits >> 23) & 0xff) as i32 - 127;
        let m = f32::from_bits((bits & 0x007fffff) | 0x3f800000) - 1.0f32;
        let ln_m = m - 0.5 * m * m + (1.0 / 3.0) * m * m * m;
        (e as f32) * 0.69314718f32 + ln_m
    };
    f32_exp(exp * ln_base)
}

/// Converts IEEE 754 half-precision float (f16) to single-precision float (f32).
pub fn f16_to_f32(h: u16) -> f32 {
    let sign = ((h >> 15) & 1) as u32;
    let exp = ((h >> 10) & 0x1f) as u32;
    let mant = (h & 0x3ff) as u32;

    if exp == 0 {
        if mant == 0 {
            f32::from_bits(sign << 31)
        } else {
            let mut m = mant;
            let mut shift = 0;
            while (m & 0x400) == 0 {
                m <<= 1;
                shift += 1;
            }
            let exp32 = 127 - 15 - shift + 1;
            let mant32 = (m & 0x3ff) << 13;
            f32::from_bits((sign << 31) | (exp32 << 23) | mant32)
        }
    } else if exp == 0x1f {
        let mant32 = if mant != 0 { 0x400000 } else { 0 };
        f32::from_bits((sign << 31) | (0xff << 23) | mant32)
    } else {
        let exp32 = exp + 127 - 15;
        let mant32 = mant << 13;
        f32::from_bits((sign << 31) | (exp32 << 23) | mant32)
    }
}

// -----------------------------------------------------------------------------
// Dequantization & Vector Operations
// -----------------------------------------------------------------------------

/// Dequantizes Q4_0 blocks (32 values per 18 bytes) into an f32 buffer.
pub fn dequantize_q4_0(raw: &[u8], out: &mut [f32]) -> Result<usize, InferenceError> {
    let num_blocks = raw.len() / 18;
    let num_elements = num_blocks * 32;
    if out.len() < num_elements {
        return Err(InferenceError::BufferTooSmall);
    }
    for b in 0..num_blocks {
        let blk = &raw[b * 18..(b + 1) * 18];
        let d = f16_to_f32(u16::from_le_bytes([blk[0], blk[1]]));
        let qs = &blk[2..18];
        let out_base = b * 32;
        for j in 0..16 {
            let v = qs[j];
            let q0 = (v & 0x0f) as i8 - 8;
            let q1 = ((v >> 4) & 0x0f) as i8 - 8;
            out[out_base + j] = (q0 as f32) * d;
            out[out_base + j + 16] = (q1 as f32) * d;
        }
    }
    Ok(num_elements)
}

/// Dequantizes Q8_0 blocks (32 values per 34 bytes) into an f32 buffer.
pub fn dequantize_q8_0(raw: &[u8], out: &mut [f32]) -> Result<usize, InferenceError> {
    let num_blocks = raw.len() / 34;
    let num_elements = num_blocks * 32;
    if out.len() < num_elements {
        return Err(InferenceError::BufferTooSmall);
    }
    for b in 0..num_blocks {
        let blk = &raw[b * 34..(b + 1) * 34];
        let d = f16_to_f32(u16::from_le_bytes([blk[0], blk[1]]));
        let out_base = b * 32;
        for j in 0..32 {
            let q = blk[2 + j] as i8;
            out[out_base + j] = (q as f32) * d;
        }
    }
    Ok(num_elements)
}

/// Weights per Q4_K super-block, and the bytes that super-block occupies.
///
/// WHY the size is asserted rather than typed at each use: `2 halves + 12 scale
/// bytes + one nibble per two weights` is the layout the format fixes, and a
/// reader that got the byte count wrong would silently read the next block's
/// header as quants.
const Q4_K_WEIGHTS: usize = 256;
const Q4_K_BLOCK_BYTES: usize = 144;
const _: () = assert!(Q4_K_BLOCK_BYTES == 2 * 2 + 12 + Q4_K_WEIGHTS / 2);

/// The 6-bit scale and min that the 12 `scales` bytes pack for sub-block `k`.
///
/// WHY the arithmetic is here instead of a straight array: the eight sub-blocks
/// do not get a byte each. Four of their six bits come from bytes 0..4 (scales)
/// and 4..8 (mins), and the remaining four borrow their two top bits from the
/// high half of those same bytes -- byte `8+j` holds scale `4+j` in its low
/// nibble and min `4+j` in its high nibble, while `S[j] >> 6` and `S[4+j] >> 6`
/// carry the two bits neither nibble could hold.
fn q4_k_scale_min(scales: &[u8], k: usize) -> (u32, u32) {
    let j = k % 4;
    if k < 4 {
        ((scales[j] & 0x3f) as u32, (scales[4 + j] & 0x3f) as u32)
    } else {
        let scale = (scales[8 + j] & 0x0f) | ((scales[j] >> 6) << 4);
        let min = (scales[8 + j] >> 4) | ((scales[4 + j] >> 6) << 4);
        (scale as u32, min as u32)
    }
}

/// Dequantizes Q4_K super-blocks (256 values per 144 bytes) into an f32 buffer.
///
/// WHY the nibbles are read out of byte order: within each 64-weight group the
/// low nibble of byte `i` is that group's element `i` and its high nibble is
/// element `i + 32`, so the sub-blocks alternate low-half/high-half down the
/// bytes rather than following them. ggml's own portable dot product unpacks
/// them this way, and a loop that walked the bytes in order would answer with
/// the right shapes and the wrong numbers.
pub fn dequantize_q4_k(raw: &[u8], out: &mut [f32]) -> Result<usize, InferenceError> {
    let num_blocks = raw.len() / Q4_K_BLOCK_BYTES;
    let num_elements = num_blocks * Q4_K_WEIGHTS;
    if out.len() < num_elements {
        return Err(InferenceError::BufferTooSmall);
    }
    for b in 0..num_blocks {
        let blk = &raw[b * Q4_K_BLOCK_BYTES..(b + 1) * Q4_K_BLOCK_BYTES];
        let d = f16_to_f32(u16::from_le_bytes([blk[0], blk[1]]));
        let dmin = f16_to_f32(u16::from_le_bytes([blk[2], blk[3]]));
        let scales = &blk[4..16];
        let qs = &blk[16..144];
        let out_base = b * Q4_K_WEIGHTS;
        for k in 0..8 {
            let (scale, min) = q4_k_scale_min(scales, k);
            let group = k / 2;
            let hi_half = k % 2;
            for i in 0..32 {
                let byte = qs[group * 32 + i];
                let q = if hi_half == 0 {
                    (byte & 0x0f) as u32
                } else {
                    (byte >> 4) as u32
                };
                out[out_base + k * 32 + i] =
                    d * (scale as f32) * (q as f32) - dmin * (min as f32);
            }
        }
    }
    Ok(num_elements)
}

/// Computes General Matrix-Vector Multiplication: y = W * x.
///
/// Handles on-the-fly dequantization directly from raw weight bytes, avoiding large
/// temporary weight memory allocations.
pub fn gemv(
    w_bytes: &[u8],
    w_type: u32,
    x: &[f32],
    out: &mut [f32],
    rows: usize,
    cols: usize,
) -> Result<(), InferenceError> {
    if out.len() < rows || x.len() < cols {
        return Err(InferenceError::BufferTooSmall);
    }

    match w_type {
        // GGML_TYPE_F32
        0 => {
            let row_stride_bytes = cols * 4;
            if w_bytes.len() < rows * row_stride_bytes {
                return Err(InferenceError::CorruptedWeights);
            }
            for i in 0..rows {
                let row_slice = &w_bytes[i * row_stride_bytes..(i + 1) * row_stride_bytes];
                let mut sum = 0.0f32;
                for j in 0..cols {
                    let w_val = f32::from_le_bytes([
                        row_slice[j * 4],
                        row_slice[j * 4 + 1],
                        row_slice[j * 4 + 2],
                        row_slice[j * 4 + 3],
                    ]);
                    sum += w_val * x[j];
                }
                out[i] = sum;
            }
            Ok(())
        }
        // GGML_TYPE_F16
        1 => {
            let row_stride_bytes = cols * 2;
            if w_bytes.len() < rows * row_stride_bytes {
                return Err(InferenceError::CorruptedWeights);
            }
            for i in 0..rows {
                let row_slice = &w_bytes[i * row_stride_bytes..(i + 1) * row_stride_bytes];
                let mut sum = 0.0f32;
                for j in 0..cols {
                    let h = u16::from_le_bytes([row_slice[j * 2], row_slice[j * 2 + 1]]);
                    sum += f16_to_f32(h) * x[j];
                }
                out[i] = sum;
            }
            Ok(())
        }
        // GGML_TYPE_Q4_0
        2 => {
            if cols % 32 != 0 {
                return Err(InferenceError::CorruptedWeights);
            }
            let blocks_per_row = cols / 32;
            let row_stride_bytes = blocks_per_row * 18;
            if w_bytes.len() < rows * row_stride_bytes {
                return Err(InferenceError::CorruptedWeights);
            }
            for i in 0..rows {
                let row_slice = &w_bytes[i * row_stride_bytes..(i + 1) * row_stride_bytes];
                let mut sum = 0.0f32;
                for b in 0..blocks_per_row {
                    let blk = &row_slice[b * 18..(b + 1) * 18];
                    let d = f16_to_f32(u16::from_le_bytes([blk[0], blk[1]]));
                    let qs = &blk[2..18];
                    let x_base = b * 32;
                    let mut b_sum = 0.0f32;
                    for j in 0..16 {
                        let v = qs[j];
                        let q0 = (v & 0x0f) as i8 - 8;
                        let q1 = ((v >> 4) & 0x0f) as i8 - 8;
                        b_sum += (q0 as f32) * x[x_base + j];
                        b_sum += (q1 as f32) * x[x_base + j + 16];
                    }
                    sum += b_sum * d;
                }
                out[i] = sum;
            }
            Ok(())
        }
        // GGML_TYPE_Q8_0
        8 => {
            if cols % 32 != 0 {
                return Err(InferenceError::CorruptedWeights);
            }
            let blocks_per_row = cols / 32;
            let row_stride_bytes = blocks_per_row * 34;
            if w_bytes.len() < rows * row_stride_bytes {
                return Err(InferenceError::CorruptedWeights);
            }
            for i in 0..rows {
                let row_slice = &w_bytes[i * row_stride_bytes..(i + 1) * row_stride_bytes];
                let mut sum = 0.0f32;
                for b in 0..blocks_per_row {
                    let blk = &row_slice[b * 34..(b + 1) * 34];
                    let d = f16_to_f32(u16::from_le_bytes([blk[0], blk[1]]));
                    let x_base = b * 32;
                    let mut b_sum = 0.0f32;
                    for j in 0..32 {
                        let q = blk[2 + j] as i8;
                        b_sum += (q as f32) * x[x_base + j];
                    }
                    sum += b_sum * d;
                }
                out[i] = sum;
            }
            Ok(())
        }
        // GGML_TYPE_Q4_K
        12 => {
            if cols % Q4_K_WEIGHTS != 0 {
                return Err(InferenceError::CorruptedWeights);
            }
            let blocks_per_row = cols / Q4_K_WEIGHTS;
            let row_stride_bytes = blocks_per_row * Q4_K_BLOCK_BYTES;
            if w_bytes.len() < rows * row_stride_bytes {
                return Err(InferenceError::CorruptedWeights);
            }
            for i in 0..rows {
                let row_slice = &w_bytes[i * row_stride_bytes..(i + 1) * row_stride_bytes];
                let mut sum = 0.0f32;
                for b in 0..blocks_per_row {
                    let blk = &row_slice[b * Q4_K_BLOCK_BYTES..(b + 1) * Q4_K_BLOCK_BYTES];
                    let d = f16_to_f32(u16::from_le_bytes([blk[0], blk[1]]));
                    let dmin = f16_to_f32(u16::from_le_bytes([blk[2], blk[3]]));
                    let scales = &blk[4..16];
                    let qs = &blk[16..144];
                    // WHY two accumulators per sub-block: the min offsets every
                    // weight in it, so its contribution is the min times the sum
                    // of the activations, not times the quant products. Folding
                    // it into one loop keeps the row on the fly -- dequantizing
                    // into a buffer first would cost the whole row's width.
                    for k in 0..8 {
                        let (scale, min) = q4_k_scale_min(scales, k);
                        let group = k / 2;
                        let hi_half = k % 2;
                        let x_base = b * Q4_K_WEIGHTS + k * 32;
                        let mut products = 0.0f32;
                        let mut activations = 0.0f32;
                        for e in 0..32 {
                            let byte = qs[group * 32 + e];
                            let q = if hi_half == 0 {
                                (byte & 0x0f) as f32
                            } else {
                                (byte >> 4) as f32
                            };
                            products += q * x[x_base + e];
                            activations += x[x_base + e];
                        }
                        sum += d * (scale as f32) * products - dmin * (min as f32) * activations;
                    }
                }
                out[i] = sum;
            }
            Ok(())
        }
        _ => Err(InferenceError::UnsupportedQuantization(w_type)),
    }
}

// -----------------------------------------------------------------------------
// Transformer Layer Components (RMSNorm, RoPE, SwiGLU, Attention)
// -----------------------------------------------------------------------------

/// Root Mean Square Layer Normalization: y_i = (x_i / sqrt(mean(x^2) + eps)) * w_i
pub fn rms_norm(x: &mut [f32], w: &[f32], eps: f32) {
    let dim = x.len();
    if dim == 0 {
        return;
    }
    let mut sum_sq = 0.0f32;
    for &v in x.iter() {
        sum_sq += v * v;
    }
    let mean_sq = sum_sq / (dim as f32);
    let scale = 1.0f32 / f32_sqrt(mean_sq + eps);
    let w_len = w.len().min(dim);
    for i in 0..w_len {
        x[i] = x[i] * scale * w[i];
    }
}

/// Applies Rotary Positional Embeddings (RoPE) in-place to query or key components.
pub fn apply_rope(vec: &mut [f32], pos: usize, head_dim: usize, base: f32) {
    let half_dim = head_dim / 2;
    for j in 0..half_dim {
        let exponent = (2 * j) as f32 / (head_dim as f32);
        let freq = 1.0f32 / f32_pow_approx(base, exponent);
        let theta = (pos as f32) * freq;
        let c = f32_cos(theta);
        let s = f32_sin(theta);
        let i0 = j * 2;
        let i1 = j * 2 + 1;
        if i1 < vec.len() {
            let v0 = vec[i0];
            let v1 = vec[i1];
            vec[i0] = v0 * c - v1 * s;
            vec[i1] = v0 * s + v1 * c;
        }
    }
}

/// SwiGLU activation function: out = silu(gate) * up = (gate * sigmoid(gate)) * up
pub fn swiglu(gate: &[f32], up: &[f32], out: &mut [f32]) {
    let len = gate.len().min(up.len()).min(out.len());
    for i in 0..len {
        let g = gate[i];
        let sig = 1.0f32 / (1.0f32 + f32_exp(-g));
        let silu = g * sig;
        out[i] = silu * up[i];
    }
}

/// Softmax activation over a mutable slice in-place.
pub fn softmax(logits: &mut [f32]) {
    if logits.is_empty() {
        return;
    }
    let mut max_val = logits[0];
    for &v in logits.iter() {
        if v > max_val {
            max_val = v;
        }
    }
    let mut sum = 0.0f32;
    for v in logits.iter_mut() {
        *v = f32_exp(*v - max_val);
        sum += *v;
    }
    if sum > 1e-12 {
        let inv_sum = 1.0f32 / sum;
        for v in logits.iter_mut() {
            *v *= inv_sum;
        }
    }
}

/// Samples the next token ID from logits.
///
/// Uses argmax sampling when temperature <= 1e-4, or temperature-scaled sampling.
pub fn sample_token(logits: &[f32], temperature: f32) -> u32 {
    if logits.is_empty() {
        return 0;
    }
    if temperature <= 1e-4 {
        let mut best_id = 0u32;
        let mut best_score = logits[0];
        for (idx, &score) in logits.iter().enumerate() {
            if score > best_score {
                best_score = score;
                best_id = idx as u32;
            }
        }
        return best_id;
    }

    // Temperature-scaled argmax
    let inv_t = 1.0f32 / temperature;
    let mut best_id = 0u32;
    let mut best_score = logits[0] * inv_t;
    for (idx, &score) in logits.iter().enumerate() {
        let scaled = score * inv_t;
        if scaled > best_score {
            best_score = scaled;
            best_id = idx as u32;
        }
    }
    best_id
}

// -----------------------------------------------------------------------------
// Transformer Layer & Model Architecture
// -----------------------------------------------------------------------------

/// Architecture hyper-parameters for Gemma / EmbeddingGemma models.
///
/// Every field is a number the model file stated (or, for the two floats and the
/// run length, the documented default the format leaves open). They are the shape
/// the scratch is cut from, so `seq_len` is a field and not a constant: it is what
/// the cache is deep.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ModelConfig {
    pub hidden_dim: usize,
    pub intermediate_dim: usize,
    pub num_layers: usize,
    pub num_heads: usize,
    pub num_kv_heads: usize,
    pub head_dim: usize,
    pub vocab_size: usize,
    /// Positions one sequence addresses, and so the depth of the KV cache.
    pub seq_len: usize,
    pub rms_norm_eps: f32,
    pub rope_theta: f32,
}

/// Run length for a file that states no `<arch>.context_length`.
///
/// WHY a default exists at all: every published Gemma, Mistral and BERT-family
/// file states one, and the walk reads it before it stops; a file that declares
/// none still has to be given a cache depth or the engine cannot answer. 512 is
/// the shortest window any of them ships with, so the engine asks for the least
/// memory anything else would have needed. The config records the number, and
/// `ModelConfig::scratch_bytes` prices it, so nothing here is presented as the
/// file's own declaration.
pub const DEFAULT_SEQ_LEN: usize = 512;

impl ModelConfig {
    /// The dimensions the engine's own tests are written against.
    ///
    /// WHY this is not a fallback the service may reach for: these numbers
    /// describe a 64-wide graph over a 290-token vocabulary, which is a property
    /// of the fixture and not of anything the user installed. Reporting them as
    /// the model's configuration would let an answer computed at 64 wide be
    /// presented as an answer from the model on the disk.
    pub const fn default_test_config() -> Self {
        Self {
            hidden_dim: 64,
            intermediate_dim: 128,
            num_layers: 1,
            num_heads: 2,
            num_kv_heads: 1,
            head_dim: 32,
            vocab_size: 290,
            seq_len: 64,
            rms_norm_eps: 1e-6,
            rope_theta: 10000.0,
        }
    }

    /// Bytes the KV cache needs: K and V, one row of `head_dim` floats per
    /// (layer, position, key/value head).
    ///
    /// WHY it saturates instead of failing: a shape whose product overflows a
    /// `usize` needs more memory than the device can count, and the refusal that
    /// follows is the right one. The number then names `usize::MAX` rather than
    /// the exact need, which says "at least this much" -- still a refusal the
    /// person can act on, and the arithmetic is exact for anything a real file
    /// declares (every dimension arrives as a `u32`).
    pub fn kv_cache_bytes(&self) -> usize {
        [
            2,
            self.num_layers,
            self.seq_len,
            self.num_kv_heads,
            self.head_dim,
            FLOAT_BYTES,
        ]
        .iter()
        .fold(1usize, |acc, &n| acc.saturating_mul(n))
    }

    /// Bytes the whole run needs: the cache plus every activation, score and
    /// logit buffer `Scratch` holds.
    ///
    /// This is the one number the engine's size limit is measured against.
    pub fn scratch_bytes(&self) -> usize {
        self.kv_cache_bytes().saturating_add(Scratch::bytes_for(self))
    }

    /// Refuses a declared shape whose scratch is over `SCRATCH_BYTE_BUDGET`.
    ///
    /// WHY this replaces the six maxima: each of them refused a dimension on its
    /// own, so a model could only be described as too wide, too deep or too rich
    /// in tokens. What the device actually has is one pile of bytes, and one
    /// comparison against it says both whether this model fits and how far over it
    /// is. The refusal names the two numbers because the answer to "too big" is a
    /// byte count, not a guess.
    pub fn check_scratch_budget(&self) -> Result<(), InferenceError> {
        let needed = self.scratch_bytes();
        if needed > SCRATCH_BYTE_BUDGET {
            return Err(InferenceError::ScratchTooLarge(needed, SCRATCH_BYTE_BUDGET));
        }
        Ok(())
    }

    /// The head geometry this build's attention kernel can actually walk.
    ///
    /// WHY it is not part of the budget: two key/value heads fit in memory -- the
    /// cache prices them -- but the kernel below caches and attends to head 0
    /// alone, so a grouped-query file would get an answer read from one head and
    /// reported as the model's. That is a graph gap, not a byte gap, and grouped
    /// query attention is the separate task that closes it.
    pub fn check_attention_shape(&self) -> Result<(), InferenceError> {
        if self.num_kv_heads > 1 {
            let stated = u32::try_from(self.num_kv_heads).ok();
            return Err(InferenceError::ModelDeclaration(
                "<arch>.attention.head_count_kv",
                stated,
                1,
            ));
        }
        let heads_span = self
            .num_heads
            .checked_mul(self.head_dim)
            .unwrap_or(usize::MAX);
        if heads_span > self.hidden_dim {
            return Err(InferenceError::ModelDeclaration(
                "<arch>.attention.head_count x key_length",
                u32::try_from(heads_span).ok(),
                self.hidden_dim,
            ));
        }
        Ok(())
    }

    /// Builds the engine's parameters from what the model file declares about
    /// itself, refusing a declaration the engine has no number for and a shape
    /// whose scratch is over the device's budget.
    ///
    /// WHY the file is the only source of these numbers: the alternative is the
    /// engine running at whatever it was compiled with and describing the result
    /// as the installed model's answer. A refusal is the honest outcome when the
    /// two disagree, and it names the key that disagrees so the gap is legible.
    ///
    /// `rms_norm_eps` and `rope_theta` take the GGUF format's own defaults when
    /// the file states neither, which is what the format specifies for an absent
    /// key; every dimension comes from the file or the engine does not run.
    pub fn from_gguf(info: &crate::gguf::GgufModelInfo) -> Result<Self, InferenceError> {
        // One bound left for a declared dimension: the file has to state it, and
        // state it as something this machine can hold. Every published width fits
        // that, so this refuses nothing a real model would trip -- the size answer
        // is `check_scratch_budget` below, which prices the shape instead of
        // comparing each dimension against a number the engine was compiled with.
        fn fits(key: &'static str, declared: Option<u32>) -> Result<usize, InferenceError> {
            let Some(declared) = declared else {
                return Err(InferenceError::ModelDeclaration(key, None, 0));
            };
            let Ok(size) = usize::try_from(declared) else {
                return Err(InferenceError::ModelDeclaration(key, Some(declared), 0));
            };
            if size == 0 {
                return Err(InferenceError::ModelDeclaration(key, Some(declared), 0));
            }
            Ok(size)
        }

        let hidden_dim = fits("<arch>.embedding_length", info.hidden_dim)?;
        let intermediate_dim = fits("<arch>.feed_forward_length", info.intermediate_dim)?;
        let num_layers = fits("<arch>.block_count", info.num_layers)?;
        let head_dim = fits("<arch>.attention.key_length", info.head_dim)?;
        let num_heads = fits("<arch>.attention.head_count", info.num_heads)?;
        let vocab_size = fits("tokenizer.ggml.tokens", info.vocab_size)?;

        // The depth the cache is cut to. `context_length` is what the file calls
        // the sequence it addresses, and it is read before the metadata walk
        // stops; a file that states none gets the engine's own run length, which
        // the config then carries in the open rather than hiding behind a
        // constant.
        let seq_len = match info.context_length {
            Some(stated) if usize::try_from(stated).unwrap_or(usize::MAX) > 0 => {
                stated as usize
            }
            _ => DEFAULT_SEQ_LEN,
        };

        // A file that states no count is the format's own multi-head case: one KV
        // head per query head. The cache prices either answer; the kernel only
        // walks one, which `check_attention_shape` is left to say.
        let num_kv_heads = match info.num_kv_heads {
            Some(kv) if kv > 0 && usize::try_from(kv).unwrap_or(usize::MAX) <= num_heads => {
                kv as usize
            }
            _ => num_heads,
        };

        let config = Self {
            hidden_dim,
            intermediate_dim,
            num_layers,
            num_heads,
            num_kv_heads,
            head_dim,
            vocab_size,
            seq_len,
            rms_norm_eps: info.rms_norm_eps.unwrap_or(1e-6),
            rope_theta: info.rope_theta.unwrap_or(10000.0),
        };

        // Checked in the order the graph is built: the head geometry first, since
        // a file whose heads do not tile their own residual stream is a malformed
        // declaration rather than a big one, then the byte budget the whole run
        // is measured against.
        config.check_attention_shape()?;
        config.check_scratch_budget()?;
        Ok(config)
    }
}

/// How many `hidden_dim`-wide streams one run holds, and how many
/// `intermediate_dim`-wide ones: the feed-forward's gate, up and product.
///
/// The hidden streams are `x`, `residual`, `x_norm`, `q`, `k`, `v_vec`,
/// `attn_out`, `attn_proj`, `ffn_out`, `hidden` and `pooled`; the eleven is the
/// pooling head's accumulator, which is why it is a stream and not a slice of
/// `hidden` (mean pooling sums every position's vector before normalising).
///
/// WHY these are counted here and not written twice: `Scratch::bytes_for` prices
/// the run from them and `Scratch::new` allocates them, and the two agreeing is
/// what makes `ModelConfig::scratch_bytes` a promise instead of a comment. The
/// test `scratch_pricing_matches_the_allocation` holds them to each other.
const HIDDEN_STREAM_BUFFERS: usize = 11;
const INTERMEDIATE_BUFFERS: usize = 3;

/// Everything one forward run needs that a 256 KiB user stack cannot hold.
///
/// Moved off the stack because a 24-block model with a 2048-position sequence and
/// 256-wide heads is 96 MiB of cache on its own: no fixed array on any thread of
/// this service fits it, and the alternative to heap memory was the six compile
/// time maxima that refused every published model. Every buffer is sized from the
/// model's own declaration, so a 64-wide test fixture takes 64-wide buffers and
/// the published file takes 768-wide ones; none of them is a ceiling.
///
/// WHY one allocation per run rather than a static: `aid` answers one inference
/// request at a time and the scratch is dropped when it returns, so the budget is
/// the peak, not the sum of concurrent runs.
pub struct Scratch {
    /// The residual stream at the step's entry, and the copy the residual add
    /// reads back.
    pub x: Vec<f32>,
    pub residual: Vec<f32>,
    pub x_norm: Vec<f32>,
    /// The attention projections, each `num_heads` slices of `head_dim` wide.
    pub q: Vec<f32>,
    pub k: Vec<f32>,
    pub v_vec: Vec<f32>,
    pub attn_out: Vec<f32>,
    pub attn_proj: Vec<f32>,
    /// The feed-forward's gate half, up half, product, and the down projection.
    pub gate: Vec<f32>,
    pub up: Vec<f32>,
    pub ffn_hidden: Vec<f32>,
    pub ffn_out: Vec<f32>,
    /// One attention head's softmax over the positions it has cached.
    pub scores: Vec<f32>,
    /// The logits the output head writes, and the normalised residual stream the
    /// pooling head reads.
    pub logits: Vec<f32>,
    pub hidden: Vec<f32>,
    /// The pooling head's accumulator: one position's normalised stream at a time
    /// is summed here before the mean and its length are taken.
    pub pooled: Vec<f32>,
}

impl Scratch {
    /// Bytes the non-cache part of a run costs: the streams, the scores and the
    /// logits, plus every allocation's own overhead of one block header.
    pub fn bytes_for(config: &ModelConfig) -> usize {
        let streams = HIDDEN_STREAM_BUFFERS.saturating_mul(config.hidden_dim);
        let feed_forward = INTERMEDIATE_BUFFERS.saturating_mul(config.intermediate_dim);
        let scores = config.seq_len;
        let logits = config.vocab_size;
        streams
            .saturating_add(feed_forward)
            .saturating_add(scores)
            .saturating_add(logits)
            .saturating_mul(FLOAT_BYTES)
    }

    /// Carves the run's buffers from the service heap, refusing rather than
    /// falling back when a piece of it cannot be had.
    ///
    /// WHY `try_reserve` and not `vec![0.0; n]`: the plain form calls the alloc
    /// error handler and aborts the task when the kernel refuses the mapping. A
    /// service that answers queries has to answer this one with the number it
    /// could not get instead of dying, so the allocation is attempted and its
    /// failure is reported.
    pub fn new(config: &ModelConfig) -> Result<Self, InferenceError> {
        let h = config.hidden_dim;
        let i = config.intermediate_dim;
        Ok(Self {
            x: zeros(h)?,
            residual: zeros(h)?,
            x_norm: zeros(h)?,
            q: zeros(h)?,
            k: zeros(h)?,
            v_vec: zeros(h)?,
            attn_out: zeros(h)?,
            attn_proj: zeros(h)?,
            gate: zeros(i)?,
            up: zeros(i)?,
            ffn_hidden: zeros(i)?,
            ffn_out: zeros(h)?,
            scores: zeros(config.seq_len)?,
            logits: zeros(config.vocab_size)?,
            hidden: zeros(h)?,
            pooled: zeros(h)?,
        })
    }

    /// Bytes actually held.
    ///
    /// WHY it exists: the price the budget was checked against has to be the
    /// price of what was allocated, or the one remaining size limit is arithmetic
    /// nothing verified. `scratch_pricing_matches_the_allocation` holds the two to
    /// each other.
    pub fn allocated_bytes(&self) -> usize {
        [
            self.x.len(),
            self.residual.len(),
            self.x_norm.len(),
            self.q.len(),
            self.k.len(),
            self.v_vec.len(),
            self.attn_out.len(),
            self.attn_proj.len(),
            self.gate.len(),
            self.up.len(),
            self.ffn_hidden.len(),
            self.ffn_out.len(),
            self.scores.len(),
            self.logits.len(),
            self.hidden.len(),
            self.pooled.len(),
        ]
        .iter()
        .fold(0usize, |acc, &n| acc.saturating_add(n))
        .saturating_mul(FLOAT_BYTES)
    }

    /// Refuses a scratch that was not cut for this shape, rather than letting a
    /// step write past the buffers it was handed.
    fn fits(&self, config: &ModelConfig) -> Result<(), InferenceError> {
        let h = config.hidden_dim;
        let i = config.intermediate_dim;
        let streams = [
            &self.x,
            &self.residual,
            &self.x_norm,
            &self.q,
            &self.k,
            &self.v_vec,
            &self.attn_out,
            &self.attn_proj,
            &self.ffn_out,
            &self.hidden,
            &self.pooled,
        ];
        if streams.iter().any(|b| b.len() < h) {
            return Err(InferenceError::BufferTooSmall);
        }
        if [
            &self.gate,
            &self.up,
            &self.ffn_hidden,
            &self.logits,
            &self.scores,
        ]
        .iter()
        .any(|b| b.is_empty())
        {
            return Err(InferenceError::BufferTooSmall);
        }
        if self.gate.len() < i || self.up.len() < i || self.ffn_hidden.len() < i {
            return Err(InferenceError::BufferTooSmall);
        }
        if self.logits.len() < config.vocab_size || self.scores.len() < config.seq_len {
            return Err(InferenceError::BufferTooSmall);
        }
        Ok(())
    }
}

/// One transformer block's tensors, borrowed from whatever holds them.
///
/// The slices are borrows and not owned buffers because the loader that fills this
/// slot reads one block at a time from the model file: a 24-block Q8_0 model is
/// ~4.3 MiB of raw bytes per block, and holding all of them alongside the 96 MiB
/// key/value cache the same file declares is not something the device has room for.
/// The slot is refilled per block, so the peak is one block.
#[derive(Clone, Copy)]
pub struct LayerWeights<'a> {
    pub attn_norm: &'a [f32],
    /// Per-head RMSNorm applied to Q after the projection, before RoPE. Gemma 3 and
    /// the published `gemma-embedding` file ship `blk.N.attn_q_norm.weight`, one
    /// vector `head_dim` wide; a file that declares none leaves this empty and the
    /// step skips it. Empty is the file's silence, not an invented identity.
    pub attn_q_norm: &'a [f32],
    /// `blk.N.attn_k_norm.weight`, per key head, same rule as `attn_q_norm`.
    pub attn_k_norm: &'a [f32],
    pub attn_q: &'a [u8],
    pub attn_k: &'a [u8],
    pub attn_v: &'a [u8],
    pub attn_o: &'a [u8],
    pub post_attn_norm: &'a [f32],
    pub ffn_norm: &'a [f32],
    pub ffn_gate: &'a [u8],
    pub ffn_up: &'a [u8],
    pub ffn_down: &'a [u8],
    pub post_ffn_norm: &'a [f32],
    /// The GGML type every quantised tensor in this block declares. The loader
    /// reads it from the file's own tensor descriptors and refuses a block whose
    /// matrices disagree with it, so this is never a number the engine picked.
    pub weight_type: u32,
}

impl<'a> LayerWeights<'a> {
    /// Whether the four norms this layer applies are as wide as the stream.
    ///
    /// WHY the tensors are not checked here: `gemv` already refuses a row buffer
    /// shorter than the product it is asked for, and it is the one that knows each
    /// quantisation's block width. The norms are different because `rms_norm`
    /// walks the shorter of the two slices it is handed -- a norm that stopped
    /// early would leave the tail of the stream unscaled and the answer would
    /// still arrive looking like it came from this model.
    fn norms_hold(&self, hidden_dim: usize) -> bool {
        [
            self.attn_norm,
            self.post_attn_norm,
            self.ffn_norm,
            self.post_ffn_norm,
        ]
        .iter()
        .all(|w| w.len() >= hidden_dim)
    }

    /// Whether the two per-head norms, when the file declares them, are exactly one
    /// head wide.
    ///
    /// WHY `head_dim` exactly and not `>=`: a QK norm wider than the head it scales
    /// would be applied to a slice of itself and the tail would go unused, which is
    /// the same half-normalised-stream defect `norms_hold` refuses.
    fn head_norms_hold(&self, head_dim: usize) -> bool {
        (self.attn_q_norm.is_empty() || self.attn_q_norm.len() == head_dim)
            && (self.attn_k_norm.is_empty() || self.attn_k_norm.len() == head_dim)
    }

    /// Whether the block's Q, K and V matrices hold at least the rows the declared
    /// head geometry needs, counted in the quantisation this block states.
    ///
    /// WHY counted per quantisation and not per element: a Q8_0 row of 768 weights
    /// is 816 bytes, not 768, and a width check written against the element count
    /// would let `gemv` read a neighbour's blocks as this tensor's tail.
    fn projections_hold(&self, kv_dim: usize, hidden_dim: usize) -> bool {
        // Every one of these four matrices consumes the normalised stream, so a row
        // of each is `hidden_dim` weights wide; what differs is how many rows the
        // head geometry asks for, which is `kv_dim` for K and V and the whole stream
        // for Q and O.
        let Some(row_bytes) = quantised_row_bytes(self.weight_type, hidden_dim) else {
            return false;
        };
        let holds = |bytes: &[u8], rows: usize| bytes.len() >= rows.saturating_mul(row_bytes);
        holds(self.attn_q, hidden_dim)
            && holds(self.attn_k, kv_dim)
            && holds(self.attn_v, kv_dim)
            && holds(self.attn_o, hidden_dim)
    }
}

/// The token embedding tensor, as the file holds it.
///
/// WHY two shapes and not one: `ModelWeights` used to carry a single byte slice
/// indexed by `token_id * hidden_dim * 4`, which is only addressable for an
/// unquantised table -- and the published Q8_0 file's table is 213,909,504 bytes,
/// more than the device's whole scratch budget for the run. The loader that reads
/// that file takes one row per token (816 bytes at 768 wide) and says which token
/// it took, so a step cannot be handed one token's row and asked for another's.
#[derive(Clone, Copy)]
pub enum TokenEmbedding<'a> {
    /// Every row the file declares, addressed by token id.
    Table {
        bytes: &'a [u8],
        /// Rows the table holds, as the file's own dimensions state them.
        rows: usize,
        dtype: u32,
    },
    /// One row, read by the loader for the token named in `token`.
    Row {
        bytes: &'a [u8],
        dtype: u32,
        /// The token this row belongs to, taken from the id the loader looked the
        /// row up under. A step asking for a different token is refused.
        token: u32,
    },
}

/// References to model weights.
///
/// `layers` is a slice and not a fixed array because the block count is the
/// file's: a two-entry array could only ever describe a two-block model, which is
/// one of the ceilings this module no longer has.
pub struct ModelWeights<'a> {
    pub token_embd: TokenEmbedding<'a>,
    pub layers: &'a [Option<LayerWeights<'a>>],
    pub final_norm: &'a [f32],
    /// The output head, when the file has one.
    ///
    /// WHY an `Option` instead of an empty slice: an embedding model is not a
    /// generative model missing a tensor. `embeddinggemma-300M-Q8_0.gguf` carries
    /// 316 descriptors and none of them is `output.weight` -- it answers with a
    /// pooled vector, never with a next-token distribution. A `None` here says the
    /// file declared no head, which `generate` refuses on and `extract_embedding`
    /// does not need; an empty slice would have `gemv` report it as corrupt bytes.
    pub output_weight: Option<&'a [u8]>,
    pub output_weight_type: u32,
}

/// KV cache for attention mechanisms, cut from the service's memory capability.
///
/// The rows are `[layer][position][kv head][dimension]` in two flat buffers, and
/// both are allocated to the depth, width and block count the file declared. This
/// is the shape that could not live on the stack: the published
/// `embeddinggemma-300M-Q8_0.gguf` -- 24 blocks, 2048 positions, one key/value
/// head 256 wide -- is 100,663,296 bytes here, against a 262,144-byte user stack.
///
/// WHY the shape is recorded instead of inferred from `k.len()`: a step has to be
/// able to tell "position 2049 of this sequence" from "a cache built for a
/// different model", and those are different answers to give a person.
pub struct KvCache {
    pub k: Vec<f32>,
    pub v: Vec<f32>,
    /// Positions the cache was cut for: a step at `seq_len` is past the sequence,
    /// not wrapped around into row 0.
    pub seq_len: usize,
    pub num_layers: usize,
    pub num_kv_heads: usize,
    pub head_dim: usize,
    /// Positions written so far.
    pub len: usize,
}

impl KvCache {
    /// Allocates the cache the model's own declaration describes.
    pub fn new(config: &ModelConfig) -> Result<Self, InferenceError> {
        let rows = config
            .num_layers
            .saturating_mul(config.seq_len)
            .saturating_mul(config.num_kv_heads);
        let floats = rows.saturating_mul(config.head_dim);
        Ok(Self {
            k: zeros(floats)?,
            v: zeros(floats)?,
            seq_len: config.seq_len,
            num_layers: config.num_layers,
            num_kv_heads: config.num_kv_heads,
            head_dim: config.head_dim,
            len: 0,
        })
    }

    /// Bytes held: both tensors.
    pub fn allocated_bytes(&self) -> usize {
        (self.k.len().saturating_add(self.v.len())).saturating_mul(FLOAT_BYTES)
    }

    /// First float of one (layer, position, head) row, or `None` when the row is
    /// not in this cache.
    pub fn row_start(&self, layer: usize, position: usize, head: usize) -> Option<usize> {
        if layer >= self.num_layers || position >= self.seq_len || head >= self.num_kv_heads {
            return None;
        }
        let per_layer = self.seq_len.saturating_mul(self.num_kv_heads).saturating_mul(self.head_dim);
        layer
            .checked_mul(per_layer)?
            .checked_add(position.checked_mul(self.num_kv_heads.saturating_mul(self.head_dim))?)?
            .checked_add(head.checked_mul(self.head_dim)?)
    }

    /// The cached key row at (layer, position, head), or `None` when the cache
    /// was not cut with that address in it.
    ///
    /// WHY the caller names the head: this build's kernel walks one key/value
    /// head, so it passes 0 and `ModelConfig::check_attention_shape` is what keeps
    /// that honest. The addressing already carries a head index, which is the
    /// memory side of grouped-query attention being representable -- the kernel
    /// that reads more than one row is the separate task.
    pub fn k_row(&self, layer: usize, position: usize, head: usize) -> Option<&[f32]> {
        let start = self.row_start(layer, position, head)?;
        self.k.get(start..start.checked_add(self.head_dim)?)
    }

    pub fn v_row(&self, layer: usize, position: usize, head: usize) -> Option<&[f32]> {
        let start = self.row_start(layer, position, head)?;
        self.v.get(start..start.checked_add(self.head_dim)?)
    }

    /// Writes one position's K and V rows for one key/value head.
    pub fn write(
        &mut self,
        layer: usize,
        position: usize,
        head: usize,
        key: &[f32],
        value: &[f32],
    ) -> Result<(), InferenceError> {
        // The row the cache holds is exactly the head the file declared; a head
        // wider than the row would be truncated, and a truncated key is a
        // different model's key.
        if key.len() != self.head_dim || value.len() != self.head_dim {
            return Err(InferenceError::BufferTooSmall);
        }
        let start = self
            .row_start(layer, position, head)
            .ok_or(InferenceError::ContextLengthExceeded)?;
        let end = start
            .checked_add(self.head_dim)
            .ok_or(InferenceError::ContextLengthExceeded)?;
        self.k
            .get_mut(start..end)
            .ok_or(InferenceError::BufferTooSmall)?
            .copy_from_slice(key);
        self.v
            .get_mut(start..end)
            .ok_or(InferenceError::BufferTooSmall)?
            .copy_from_slice(value);
        Ok(())
    }

    pub fn reset(&mut self) {
        self.len = 0;
    }
}

/// One zeroed buffer from the service heap, or the refusal that says it was not
/// given the memory.
fn zeros(count: usize) -> Result<Vec<f32>, InferenceError> {
    let mut buffer = Vec::new();
    buffer
        .try_reserve_exact(count)
        .map_err(|_| InferenceError::ScratchUnavailable)?;
    buffer.resize(count, 0.0f32);
    Ok(buffer)
}

// -----------------------------------------------------------------------------
// Neural Forward Pass & Generation
// -----------------------------------------------------------------------------

/// Rows of `head_dim` the key/value side of a run holds.
///
/// WHY this is a function of the config and not of the residual width: the
/// published `embeddinggemma-300M-Q8_0.gguf` declares 3 query heads and **1**
/// key/value head of 256, so its `blk.N.attn_k.weight` is `[768, 256]` -- 256 rows,
/// not the 768 the residual stream is wide. A step that projected K and V as though
/// they were stream-wide would read 512 rows past the end of that tensor, which is
/// `gemv`'s `CorruptedWeights` refusal, and the fix is to ask it for the rows the
/// file declared rather than to widen the tensor.
pub fn kv_projection_width(config: &ModelConfig) -> usize {
    config.num_kv_heads.saturating_mul(config.head_dim)
}

/// Bytes one `width`-element row occupies in the quantisation the file declares.
///
/// WHY this is not `width * 4`: the published file's `token_embd.weight` is Q8_0,
/// so a 768-wide row is 816 bytes of blocks and not 3072 bytes of floats, and a
/// reader that sized a row by the float width would hand `gemv` the next token's
/// scale bytes as this token's tail. `None` is the refusal the caller reports --
/// a type the engine has no layout for, named by its number, not read as another.
pub fn quantised_row_bytes(dtype: u32, width: usize) -> Option<usize> {
    let blocks = |weight_divisor: usize, block_bytes: usize| -> Option<usize> {
        if width % weight_divisor != 0 {
            // A width that is not a whole number of blocks has no row: the format
            // does not let a block straddle two rows, so a number derived by
            // integer division here would be a row the file does not hold.
            return None;
        }
        (width / weight_divisor).checked_mul(block_bytes)
    };
    match dtype {
        0 => width.checked_mul(FLOAT_BYTES),
        1 => width.checked_mul(2),
        2 => blocks(32, 18),
        8 => blocks(32, 34),
        12 => blocks(Q4_K_WEIGHTS, Q4_K_BLOCK_BYTES),
        _ => None,
    }
}

/// Writes one embedding row, in the quantisation the file declares for it, into the
/// residual stream scaled by `scale`.
///
/// WHY the dtype is an argument and not a constant: `token_embd.weight` is F32 in
/// the fixtures this engine was written against and Q8_0 in the file that is
/// actually installed, and a lookup that read the Q8_0 blocks as floats would
/// produce noise the model never held. An id the loader has no layout for is
/// refused by number.
pub fn embedding_row_into(
    raw: &[u8],
    dtype: u32,
    scale: f32,
    out: &mut [f32],
) -> Result<(), InferenceError> {
    match dtype {
        // GGML_TYPE_F32: the row is already the values, so the copy is the scale.
        0 => {
            let count = out.len();
            if raw.len() < count.saturating_mul(FLOAT_BYTES) {
                return Err(InferenceError::CorruptedWeights);
            }
            for (j, slot) in out.iter_mut().enumerate() {
                let at = j * FLOAT_BYTES;
                *slot =
                    f32::from_le_bytes([raw[at], raw[at + 1], raw[at + 2], raw[at + 3]]) * scale;
            }
            Ok(())
        }
        // GGML_TYPE_F16
        1 => {
            let count = out.len();
            if raw.len() < count.saturating_mul(2) {
                return Err(InferenceError::CorruptedWeights);
            }
            for (j, slot) in out.iter_mut().enumerate() {
                let at = j * 2;
                *slot = f16_to_f32(u16::from_le_bytes([raw[at], raw[at + 1]])) * scale;
            }
            Ok(())
        }
        // GGML_TYPE_Q4_0 / Q8_0 / Q4_K: dequantise the whole row, then scale it. The
        // three helpers write exactly `raw.len() / block_bytes * block_weights`
        // floats, which is the row's declared width when the loader sized the buffer.
        2 | 8 | 12 => {
            let produced = match dtype {
                2 => dequantize_q4_0(raw, out)?,
                8 => dequantize_q8_0(raw, out)?,
                _ => dequantize_q4_k(raw, out)?,
            };
            if produced != out.len() {
                return Err(InferenceError::CorruptedWeights);
            }
            for slot in out.iter_mut() {
                *slot *= scale;
            }
            Ok(())
        }
        _ => Err(InferenceError::UnsupportedQuantization(dtype)),
    }
}

/// Mean-pools `count` accumulated positions and L2-normalises the result.
///
/// Shared by `extract_embedding` and the streaming loader so the pooling rule an
/// in-memory run applies and the one a file-read run applies cannot drift apart --
/// two vectors from two different poolings compared by the search index would be a
/// similarity the model never had.
pub fn pooled_normalised(pooled: &[f32], count: usize, out: &mut [f32]) {
    if count == 0 {
        return;
    }
    let inv_count = 1.0f32 / (count as f32);
    for (slot, &value) in out.iter_mut().zip(pooled) {
        *slot = value * inv_count;
    }
    let mut sum_sq = 0.0f32;
    for &v in out.iter() {
        sum_sq += v * v;
    }
    let norm = f32_sqrt(sum_sq);
    if norm > 1e-12 {
        let inv_norm = 1.0f32 / norm;
        for v in out.iter_mut() {
            *v *= inv_norm;
        }
    }
}

/// Runs one transformer block over the residual stream in `scratch`.
///
/// This is the body of the block loop `forward_step` walks, split out so the loader
/// that cannot hold all 24 blocks at once can run one, release its slot, and read
/// the next. Everything the block touches is checked on entry, and the streams are
/// the caller's: a block that fails leaves `scratch` holding what it held before it
/// ran, exactly as a failed step does.
pub fn run_block(
    block: usize,
    lw: &LayerWeights<'_>,
    pos: usize,
    config: &ModelConfig,
    kv_cache: &mut KvCache,
    scratch: &mut Scratch,
) -> Result<(), InferenceError> {
    let h_dim = config.hidden_dim;
    let inter_dim = config.intermediate_dim;
    let kv_dim = kv_projection_width(config);
    let hd = config.head_dim;

    if pos >= kv_cache.seq_len {
        return Err(InferenceError::ContextLengthExceeded);
    }
    // The scratch is `hidden_dim` wide everywhere, which bounds the key/value side
    // only while `num_kv_heads x head_dim` does not exceed the stream. It cannot:
    // `check_attention_shape` refuses kv heads beyond the query heads and heads
    // that do not tile the stream, and `from_gguf` refuses kv > query. Checked here
    // too because a hand-built config reaches this function as well.
    config.check_attention_shape()?;
    if kv_dim > h_dim || kv_dim > scratch.k.len() || kv_dim > scratch.v_vec.len() {
        return Err(InferenceError::BufferTooSmall);
    }
    if !lw.norms_hold(h_dim) || !lw.head_norms_hold(hd) {
        return Err(InferenceError::CorruptedWeights);
    }
    if !lw.projections_hold(kv_dim, h_dim) {
        return Err(InferenceError::CorruptedWeights);
    }

    // Destructured so the block holds disjoint borrows of the buffers it writes in
    // one pass -- the attention pass reads `q` while it writes `attn_out`, and the
    // residual add reads `residual` while it writes `x`.
    let Scratch {
        x,
        residual,
        x_norm,
        q,
        k,
        v_vec,
        attn_out,
        attn_proj,
        gate,
        up,
        ffn_hidden,
        ffn_out,
        scores,
        ..
    } = scratch;

    // 1. Pre-attention RMSNorm.
    residual[..h_dim].copy_from_slice(&x[..h_dim]);
    x_norm[..h_dim].copy_from_slice(&x[..h_dim]);
    rms_norm(&mut x_norm[..h_dim], lw.attn_norm, config.rms_norm_eps);

    // 2. Q, K, V projections. Q spans the whole stream (every query head laid end
    // to end); K and V span only the rows the declared key/value heads take.
    gemv(lw.attn_q, lw.weight_type, &x_norm[..h_dim], &mut q[..h_dim], h_dim, h_dim)?;
    gemv(lw.attn_k, lw.weight_type, &x_norm[..h_dim], &mut k[..kv_dim], kv_dim, h_dim)?;
    gemv(lw.attn_v, lw.weight_type, &x_norm[..h_dim], &mut v_vec[..kv_dim], kv_dim, h_dim)?;

    // 3. Per-head QK norm, when the file declares one. Gemma 3 and the published
    // embedding file each ship `attn_q_norm` / `attn_k_norm` one head wide; a file
    // that states neither leaves the head unnormalised, which is that file's graph.
    if !lw.attn_q_norm.is_empty() {
        for head in 0..config.num_heads {
            let at = head * hd;
            rms_norm(&mut q[at..at + hd], lw.attn_q_norm, config.rms_norm_eps);
        }
    }
    if !lw.attn_k_norm.is_empty() {
        for head in 0..config.num_kv_heads {
            let at = head * hd;
            rms_norm(&mut k[at..at + hd], lw.attn_k_norm, config.rms_norm_eps);
        }
    }

    // 4. RoPE, per query head and per key head. The two counts differ exactly when
    // the file declares grouped-query attention.
    for head in 0..config.num_heads {
        let at = head * hd;
        apply_rope(&mut q[at..at + hd], pos, hd, config.rope_theta);
    }
    for head in 0..config.num_kv_heads {
        let at = head * hd;
        apply_rope(&mut k[at..at + hd], pos, hd, config.rope_theta);
    }

    // 5. Cache the one key/value head this build's kernel walks. `KvCache::write`
    // refuses a slice that is not exactly `head_dim` long, so the row is the whole
    // declared head: nothing is dropped and nothing is zero-padded to reach it.
    kv_cache.write(block, pos, 0, &k[..hd], &v_vec[..hd])?;

    // 6. Attention scores and softmax, then the weighted sum of values.
    let scale_att = 1.0f32 / f32_sqrt(hd as f32);
    let seq_len_cur = pos + 1;
    for value in attn_out[..h_dim].iter_mut() {
        *value = 0.0f32;
    }
    for head in 0..config.num_heads {
        let h_base = head * hd;
        for t in 0..seq_len_cur {
            let key_row = kv_cache
                .k_row(block, t, 0)
                .ok_or(InferenceError::ContextLengthExceeded)?;
            let mut dot = 0.0f32;
            for d in 0..hd {
                dot += q[h_base + d] * key_row[d];
            }
            scores[t] = dot * scale_att;
        }
        softmax(&mut scores[..seq_len_cur]);
        for t in 0..seq_len_cur {
            let value_row = kv_cache
                .v_row(block, t, 0)
                .ok_or(InferenceError::ContextLengthExceeded)?;
            let weight = scores[t];
            for d in 0..hd {
                attn_out[h_base + d] += weight * value_row[d];
            }
        }
    }

    // 7. Output projection, post-attention norm, residual add.
    gemv(lw.attn_o, lw.weight_type, &attn_out[..h_dim], &mut attn_proj[..h_dim], h_dim, h_dim)?;
    rms_norm(&mut attn_proj[..h_dim], lw.post_attn_norm, config.rms_norm_eps);
    for i in 0..h_dim {
        x[i] = residual[i] + attn_proj[i];
    }

    // 8. Feed-forward: gate and up projections, SwiGLU, down projection.
    residual[..h_dim].copy_from_slice(&x[..h_dim]);
    x_norm[..h_dim].copy_from_slice(&x[..h_dim]);
    rms_norm(&mut x_norm[..h_dim], lw.ffn_norm, config.rms_norm_eps);
    gemv(lw.ffn_gate, lw.weight_type, &x_norm[..h_dim], &mut gate[..inter_dim], inter_dim, h_dim)?;
    gemv(lw.ffn_up, lw.weight_type, &x_norm[..h_dim], &mut up[..inter_dim], inter_dim, h_dim)?;
    swiglu(&gate[..inter_dim], &up[..inter_dim], &mut ffn_hidden[..inter_dim]);
    gemv(lw.ffn_down, lw.weight_type, &ffn_hidden[..inter_dim], &mut ffn_out[..h_dim], h_dim, inter_dim)?;
    rms_norm(&mut ffn_out[..h_dim], lw.post_ffn_norm, config.rms_norm_eps);
    for i in 0..h_dim {
        x[i] = residual[i] + ffn_out[i];
    }
    Ok(())
}

/// Executes a single forward step of the neural network for one token at `pos`.
///
/// The step's streams, scores and logits arrive through `scratch` and the cached
/// keys through `kv_cache`, both cut from the model's own declaration; the
/// normalised residual stream leaves as `scratch.hidden` and the next-token
/// distribution as `scratch.logits`.
pub fn forward_step(
    token_id: u32,
    pos: usize,
    model: &ModelWeights,
    config: &ModelConfig,
    kv_cache: &mut KvCache,
    scratch: &mut Scratch,
) -> Result<(), InferenceError> {
    let h_dim = config.hidden_dim;

    // Everything the step indexes with is checked before any work runs, so a call
    // this engine will not complete leaves the cache holding what it held when it
    // arrived.
    //
    // A declaration wider than the memory it was handed is refused, never
    // truncated to fit: an answer computed at 32 wide and reported as the answer
    // of a model that declared 256 would be the engine describing a model it
    // never ran.
    if pos >= kv_cache.seq_len {
        return Err(InferenceError::ContextLengthExceeded);
    }
    // The head loop indexes `num_heads x head_dim` of a stream that is `h_dim`
    // long, so heads that do not tile it would read past the projection that
    // produced them, and this build's kernel walks one key/value head.
    // `from_gguf` refuses both already; a hand-built config is checked here for
    // the same reason.
    config.check_attention_shape()?;
    scratch.fits(config)?;
    if kv_cache.head_dim != config.head_dim
        || kv_cache.num_kv_heads != config.num_kv_heads
        || kv_cache.num_layers < config.num_layers
        || kv_cache.seq_len < config.seq_len
    {
        return Err(InferenceError::BufferTooSmall);
    }
    // The file declares `num_layers` blocks; fewer than that handed over means the
    // run would skip transformer blocks and still be reported as this model's
    // answer.
    if model.layers.len() < config.num_layers {
        return Err(InferenceError::CorruptedWeights);
    }
    // The same argument as `LayerWeights::norms_hold` for the block norms: a final
    // norm narrower than the stream leaves its tail unscaled, and the logits and
    // the pooled vector would both be computed from a half-normalised stream.
    if model.final_norm.len() < h_dim {
        return Err(InferenceError::CorruptedWeights);
    }

    // 1. Embedding lookup, in whatever quantisation the file's own descriptor
    // declares for `token_embd.weight`.
    //
    // The table form indexes the row by token id; the row form is the loader's --
    // one row read from the file, tagged with the token it was read for. A step
    // given a row for a different token is refused rather than answered with it,
    // because the answer would then be one token's embedding reported as another's.
    let embd_scale = f32_sqrt(h_dim as f32);
    match model.token_embd {
        TokenEmbedding::Row { bytes, dtype, token } => {
            if token != token_id {
                return Err(InferenceError::CorruptedWeights);
            }
            embedding_row_into(bytes, dtype, embd_scale, &mut scratch.x[..h_dim])?;
        }
        TokenEmbedding::Table { bytes, rows, dtype } => {
            let embd_idx = (token_id as usize) % config.vocab_size;
            if embd_idx >= rows {
                // The row the file does not hold is not invented here. A stand-in
                // built from the token id would let an answer be reported as the
                // installed model's while nothing of the model had been read.
                return Err(InferenceError::CorruptedWeights);
            }
            let row_bytes = quantised_row_bytes(dtype, h_dim)
                .ok_or(InferenceError::UnsupportedQuantization(dtype))?;
            let embd_offset = embd_idx
                .checked_mul(row_bytes)
                .ok_or(InferenceError::CorruptedWeights)?;
            if bytes.len() < embd_offset + row_bytes {
                return Err(InferenceError::CorruptedWeights);
            }
            embedding_row_into(
                &bytes[embd_offset..embd_offset + row_bytes],
                dtype,
                embd_scale,
                &mut scratch.x[..h_dim],
            )?;
        }
    }

    // 2. Transformer blocks, each over the weights the file is held with.
    for block in 0..config.num_layers {
        let lw = match &model.layers[block] {
            Some(w) => *w,
            // A block the file declares and nobody loaded is a missing weight,
            // not a block to walk around: skipping it silently would report a
            // shallower network as this model's answer.
            None => return Err(InferenceError::CorruptedWeights),
        };
        run_block(block, &lw, pos, config, kv_cache, scratch)?;
    }

    // 3. Final RMSNorm
    rms_norm(&mut scratch.x[..h_dim], model.final_norm, config.rms_norm_eps);

    // The normalised residual stream is what a pooling head consumes, so it leaves
    // in `scratch.hidden` rather than being re-derived from the logits below.
    scratch.hidden[..h_dim].copy_from_slice(&scratch.x[..h_dim]);

    // 4. Output projection to compute logits -- only when the file has a head to
    // project with.
    //
    // WHY the head can be absent: `embeddinggemma-300M-Q8_0.gguf` names 316
    // tensors and none of them is an output head; it is a pooling model. Leaving
    // `scratch.logits` holding what it held is honest for that file, and `generate`
    // refuses to sample from it. An embedding run never reads the logits, so the
    // stream it pools is unaffected by the head it does not have.
    //
    // WHY the whole declared vocabulary and not a length taken from the caller's
    // buffer: `Scratch::fits` has already guaranteed the buffer holds
    // `config.vocab_size`, and a distribution truncated to whatever the caller
    // happened to hand over would still be sampled as though it were complete --
    // reporting a token chosen from half the model's vocabulary.
    if let Some(head) = model.output_weight {
        gemv(
            head,
            model.output_weight_type,
            &scratch.x[..h_dim],
            &mut scratch.logits[..config.vocab_size],
            config.vocab_size,
            h_dim,
        )?;
    }

    kv_cache.len = pos + 1;
    Ok(())
}

/// Autoregressive token generation loop with timeout / cancellation support.
///
/// Runs pre-fill across prompt tokens, then generates tokens sequentially, invoking
/// `on_token_generated` for each token until EOS or `max_new_tokens` is reached.
pub fn generate(
    prompt_tokens: &[u32],
    model: &ModelWeights,
    config: &ModelConfig,
    max_new_tokens: usize,
    temperature: f32,
    is_cancelled: &impl Fn() -> bool,
    mut on_token_generated: impl FnMut(u32) -> bool,
) -> Result<usize, InferenceError> {
    if prompt_tokens.is_empty() {
        return Ok(0);
    }
    // A file with no output head cannot answer with a next token.
    //
    // WHY this refuses before the budget is paid rather than after: sampling from
    // logits the model never wrote would report a token chosen from an untouched
    // buffer as the installed model's answer, and the installed
    // `embeddinggemma-300M-Q8_0.gguf` is exactly that file -- 316 tensors, none of
    // them a projection back to the vocabulary. The pooling path it does have is
    // `extract_embedding`.
    if model.output_weight.is_none() {
        return Err(InferenceError::ModelDeclaration("output.weight", None, 0));
    }
    // The budget is the first thing the run pays for: a model that does not fit
    // is refused before a single byte is asked for, and the refusal names both
    // numbers so the caller knows which one to change.
    config.check_scratch_budget()?;
    let mut scratch = Scratch::new(config)?;
    let mut kv_cache = KvCache::new(config)?;

    // 1. Pre-fill prompt tokens.
    //
    // A prompt longer than the sequence the file declared is cut to it rather than
    // run past the cache: `config.seq_len` comes from `context_length`, so the tail
    // the model could not have attended to is dropped by the model's own limit and
    // not by one invented here.
    let prefill_len = prompt_tokens.len().min(config.seq_len);
    for pos in 0..prefill_len {
        if is_cancelled() {
            return Err(InferenceError::Cancelled);
        }
        forward_step(
            prompt_tokens[pos],
            pos,
            model,
            config,
            &mut kv_cache,
            &mut scratch,
        )?;
    }

    // 2. Sample first generated token from prompt's final logits
    let mut next_token = sample_token(&scratch.logits[..config.vocab_size], temperature);
    let mut generated_count = 0usize;
    let mut cur_pos = prefill_len;

    // 3. Autoregressive loop
    while generated_count < max_new_tokens && cur_pos < config.seq_len {
        if is_cancelled() {
            return Err(InferenceError::Cancelled);
        }

        // Deliver generated token to callback
        let keep_going = on_token_generated(next_token);
        generated_count += 1;

        if !keep_going || next_token == crate::tokenizer::TOKEN_EOS {
            break;
        }

        // Forward step with new token
        forward_step(
            next_token,
            cur_pos,
            model,
            config,
            &mut kv_cache,
            &mut scratch,
        )?;

        cur_pos += 1;
        next_token = sample_token(&scratch.logits[..config.vocab_size], temperature);
    }

    Ok(generated_count)
}

/// Extracts real neural vector embeddings from the model for input tokens.
///
/// Runs forward pass across tokens, extracts hidden representations, and applies
/// L2-normalization for semantic similarity search.
pub fn extract_embedding(
    tokens: &[u32],
    model: &ModelWeights,
    config: &ModelConfig,
    out_embedding: &mut [f32],
) -> Result<usize, InferenceError> {
    if tokens.is_empty() {
        return Ok(0);
    }
    // WHY the caller's buffer is checked against the declared width instead of the
    // two being min'ed together: a 768-wide model that wrote 256 numbers and
    // reported 256 would hand back a prefix of a vector and let the caller compare
    // it to another prefix as though both were the whole embedding. Checked before
    // anything is allocated, because a destination this service cannot write into
    // is not a reason to take memory from the device first.
    if out_embedding.len() < config.hidden_dim {
        return Err(InferenceError::BufferTooSmall);
    }
    // Same order as `generate`: priced, refused if it does not fit, then allocated.
    config.check_scratch_budget()?;
    let mut scratch = Scratch::new(config)?;
    let mut kv_cache = KvCache::new(config)?;
    let count = tokens.len().min(config.seq_len);
    let dim = config.hidden_dim;

    // Mean pooling over the positions, which is what an embedding file declares
    // when it states `pooling_type` 1 -- the published file states exactly that
    // number. Each step's normalised residual stream is summed as it is produced,
    // so the vector is the model's own hidden representation rather than a slice
    // of the cache or a re-use of the logits.
    for pos in 0..count {
        forward_step(
            tokens[pos],
            pos,
            model,
            config,
            &mut kv_cache,
            &mut scratch,
        )?;
        for i in 0..dim {
            scratch.pooled[i] += scratch.hidden[i];
        }
    }

    // The same two lines the streaming loader runs, so an in-memory vector and a
    // file-read vector cannot be pooled by two different rules.
    //
    // WHAT THIS DOES NOT DO: the published file also carries `dense_2.weight`
    // (768 -> 3072) and `dense_3.weight` (3072 -> 768) after the pool, and their
    // dimensions are declared as `dense_2_feat_in/out` and `dense_3_feat_in/out`.
    // This engine has never applied that head, so what comes back from either path
    // is the pooled pre-head stream, not the published model's final embedding.
    pooled_normalised(&scratch.pooled[..dim], count, &mut out_embedding[..dim]);

    Ok(dim)
}

#[cfg(test)]
pub mod tests {
    use super::*;

    #[test]
    fn test_f32_exp_accuracy() {
        assert_eq!(f32_exp(0.0), 1.0);
        let e = f32_exp(1.0);
        assert!((e - 2.71828).abs() < 0.01, "e was {}", e);
        assert_eq!(f32_exp(-100.0), 0.0);
    }

    #[test]
    fn test_f16_to_f32() {
        // 0 in f16 = 0x0000
        assert_eq!(f16_to_f32(0x0000), 0.0);
        // 1.0 in f16: sign=0, exp=15 (01111), mant=0 -> 0x3c00
        assert_eq!(f16_to_f32(0x3c00), 1.0);
        // -2.0 in f16: sign=1, exp=16 (10000), mant=0 -> 0xc000
        assert_eq!(f16_to_f32(0xc000), -2.0);
    }

    #[test]
    fn test_dequantize_q4_0_and_q8_0() {
        // 1 Q4_0 block = 18 bytes: 2 bytes f16 delta (1.0), 16 bytes nibbles
        let mut q4_block = [0u8; 18];
        q4_block[0..2].copy_from_slice(&0x3c00u16.to_le_bytes()); // delta = 1.0
        for i in 2..18 {
            q4_block[i] = 0x88; // low nibble 8 -> 8-8=0, high nibble 8 -> 8-8=0
        }
        let mut out = [0.0f32; 32];
        dequantize_q4_0(&q4_block, &mut out).unwrap();
        for &v in out.iter() {
            assert_eq!(v, 0.0);
        }

        // 1 Q8_0 block = 34 bytes: 2 bytes f16 delta (1.0), 32 bytes quants
        let mut q8_block = [0u8; 34];
        q8_block[0..2].copy_from_slice(&0x3c00u16.to_le_bytes()); // delta = 1.0
        for i in 0..32 {
            q8_block[2 + i] = (i as i8) as u8;
        }
        dequantize_q8_0(&q8_block, &mut out).unwrap();
        for i in 0..32 {
            assert_eq!(out[i], i as f32);
        }
    }

    #[test]
    fn test_gemv_f32_and_q8() {
        let x = [1.0f32, 2.0f32];
        let mut w_f32 = [0u8; 16]; // 2x2 matrix: [[1.0, 2.0], [3.0, 4.0]]
        w_f32[0..4].copy_from_slice(&1.0f32.to_le_bytes());
        w_f32[4..8].copy_from_slice(&2.0f32.to_le_bytes());
        w_f32[8..12].copy_from_slice(&3.0f32.to_le_bytes());
        w_f32[12..16].copy_from_slice(&4.0f32.to_le_bytes());

        let mut out = [0.0f32; 2];
        gemv(&w_f32, 0, &x, &mut out, 2, 2).unwrap();
        assert_eq!(out[0], 5.0); // 1*1 + 2*2 = 5
        assert_eq!(out[1], 11.0); // 3*1 + 4*2 = 11
    }

    #[test]
    fn test_dequantize_q4_k_puts_each_nibble_in_the_sub_block_the_format_puts_it_in() {
        // One super-block: d = 1.0, dmin = 1.0, every sub-block scale 1, every min 0.
        let mut blk = [0u8; Q4_K_BLOCK_BYTES];
        blk[0..2].copy_from_slice(&0x3c00u16.to_le_bytes());
        blk[2..4].copy_from_slice(&0x3c00u16.to_le_bytes());
        for i in 0..4 {
            blk[4 + i] = 1; // scales 0..3
            blk[12 + i] = 1; // low nibbles of scales 4..7
        }
        // Byte 0 of the quants: nibble 1 then 2. Byte 32 of the quants: 3 then 4.
        blk[16] = 0x21;
        blk[48] = 0x43;
        let mut out = [0.0f32; Q4_K_WEIGHTS];
        assert_eq!(dequantize_q4_k(&blk, &mut out).unwrap(), Q4_K_WEIGHTS);
        // The low nibbles of both bytes are the first elements of sub-blocks 0 and 2,
        // the high nibbles land 32 weights later in sub-blocks 1 and 3 -- not in the
        // next byte, which is what a loop over the bytes in order would answer.
        assert_eq!(out[0], 1.0);
        assert_eq!(out[32], 2.0);
        assert_eq!(out[64], 3.0);
        assert_eq!(out[96], 4.0);
        assert_eq!(out.iter().filter(|&&v| v != 0.0).count(), 4);
    }

    #[test]
    fn test_dequantize_q4_k_borrows_the_two_top_scale_bits_the_six_bit_pair_cannot_hold() {
        let mut blk = [0u8; Q4_K_BLOCK_BYTES];
        blk[0..2].copy_from_slice(&0x3c00u16.to_le_bytes());
        blk[2..4].copy_from_slice(&0x3c00u16.to_le_bytes());
        blk[4] = 0xc1; // scale 0 = 1, and the top bits 3 that extend scale 4
        blk[8] = 0xc3; // min 0 = 3, and the top bits 3 that extend min 4
        blk[12] = 0x02; // scale 4 = 2 | 48, min 4 = 0 | 48
        blk[16] = 0x01; // sub-block 0, element 0: quant 1
        blk[80] = 0x01; // sub-block 4, element 0: the same nibble, 64-weight group 2
        let mut out = [0.0f32; Q4_K_WEIGHTS];
        dequantize_q4_k(&blk, &mut out).unwrap();
        // Every value here is a small integer, and d and dmin are exactly 1.0, so
        // the reference arithmetic and this one agree to the last bit: no tolerance.
        assert_eq!(out[0], 1.0 * 1.0 * 1.0 - 1.0 * 3.0); // -2
        for e in 1..32 {
            assert_eq!(out[e], -3.0, "sub-block 0 offsets an unset quant");
        }
        for e in 32..128 {
            assert_eq!(out[e], 0.0, "the six bytes that state no bits ink nothing");
        }
        assert_eq!(out[128], 50.0 - 48.0);
        for e in 129..160 {
            assert_eq!(out[e], -48.0);
        }
        for e in 160..Q4_K_WEIGHTS {
            assert_eq!(out[e], 0.0);
        }
    }

    #[test]
    fn test_gemv_reads_a_q4_k_row_without_materializing_it() {
        // Two rows of 256 weights, every quant nibble set, scale 1, min 0, d 1.0:
        // the row is the all-ones vector, so the product is the sum of x.
        let mut w = [0u8; 2 * Q4_K_BLOCK_BYTES];
        for r in 0..2 {
            let base = r * Q4_K_BLOCK_BYTES;
            w[base..base + 2].copy_from_slice(&0x3c00u16.to_le_bytes());
            w[base + 2..base + 4].copy_from_slice(&0x3c00u16.to_le_bytes());
            for i in 0..4 {
                w[base + 4 + i] = 1;
                w[base + 12 + i] = 1;
            }
            for q in 0..Q4_K_WEIGHTS / 2 {
                w[base + 16 + q] = 0x11;
            }
        }
        let x: [f32; Q4_K_WEIGHTS] = core::array::from_fn(|i| i as f32);
        let mut out = [0.0f32; 2];
        gemv(&w, 12, &x, &mut out, 2, Q4_K_WEIGHTS).unwrap();
        assert_eq!(out[0], 32640.0); // 0 + 1 + ... + 255
        assert_eq!(out[1], 32640.0);

        // A row that is not a whole number of super-blocks has nowhere for the
        // trailing weights to sit, and an id the engine was not given the layout
        // of is refused by number rather than read as if it were known.
        assert_eq!(
            gemv(&w, 12, &x, &mut out, 1, 128),
            Err(InferenceError::CorruptedWeights)
        );
        assert_eq!(
            gemv(&w, 3, &x, &mut out, 1, 1),
            Err(InferenceError::UnsupportedQuantization(3))
        );
    }

    #[test]
    fn test_rms_norm_and_swiglu() {
        let mut x = [3.0f32, 4.0f32];
        let w = [1.0f32, 1.0f32];
        rms_norm(&mut x, &w, 0.0);
        // mean sq = (9 + 16)/2 = 12.5. sqrt(12.5) ~ 3.5355
        assert!((x[0] - 3.0 / 3.5355).abs() < 0.01);

        let gate = [0.0f32, 2.0f32];
        let up = [1.0f32, 1.0f32];
        let mut out = [0.0f32; 2];
        swiglu(&gate, &up, &mut out);
        // silu(0) = 0 * 0.5 = 0
        assert_eq!(out[0], 0.0);
        assert!(out[1] > 0.0);
    }

    /// The deterministic little model the forward tests run against: identity
    /// projections, unit norms, and a real 290 x 64 embedding table the engine can
    /// read a row out of. An empty table would only ever exercise the refusal, and
    /// a stand-in row built from the token id would have these tests certify an
    /// answer computed from nothing.
    /// The width this fixture is built at.
    ///
    /// WHY it is a fixture's own number and not a constant of the engine: the
    /// engine used to carry a compile-time ceiling for every dimension and the
    /// tests sized themselves with those, which made the little graph look like
    /// the shape the engine could speak of. The published model is 768 wide, this
    /// one is 64, and both are described by the same `ModelConfig`; the only limit
    /// that applies to either is `SCRATCH_BYTE_BUDGET`.
    const LITTLE_HIDDEN: usize = 64;

    /// Scratch and cache cut for a config, the way every caller of `forward_step`
    /// must supply them.
    fn fit(config: &ModelConfig) -> (Scratch, KvCache) {
        (
            Scratch::new(config).expect("the fixture's shape is under budget"),
            KvCache::new(config).expect("the fixture's cache is under budget"),
        )
    }

    struct LittleModel {
        ones: [f32; LITTLE_HIDDEN],
        id_w: [u8; 64 * 64 * 4],
        ffn_gate_w: [u8; 128 * 64 * 4],
        ffn_down_w: [u8; 64 * 128 * 4],
        head_w: [u8; 290 * 64 * 4],
        embd: [u8; 290 * 64 * 4],
    }

    impl LittleModel {
        fn new() -> Self {
            let mut model = Self {
                ones: [1.0f32; LITTLE_HIDDEN],
                id_w: [0u8; 64 * 64 * 4],
                ffn_gate_w: [0u8; 128 * 64 * 4],
                ffn_down_w: [0u8; 64 * 128 * 4],
                head_w: [0u8; 290 * 64 * 4],
                embd: [0u8; 290 * 64 * 4],
            };
            for i in 0..64 {
                let at = (i * 64 + i) * 4;
                model.id_w[at..at + 4].copy_from_slice(&1.0f32.to_le_bytes());
                model.head_w[at..at + 4].copy_from_slice(&1.0f32.to_le_bytes());
                let gate = (i * 64 + i) * 4;
                model.ffn_gate_w[gate..gate + 4].copy_from_slice(&1.0f32.to_le_bytes());
                let down = (i * 128 + i) * 4;
                model.ffn_down_w[down..down + 4].copy_from_slice(&1.0f32.to_le_bytes());
            }
            for token in 0..290usize {
                for dim in 0..64usize {
                    let value = (((token * 7 + dim * 13) % 17) as f32 - 8.0) / 16.0;
                    let at = (token * 64 + dim) * 4;
                    model.embd[at..at + 4].copy_from_slice(&value.to_le_bytes());
                }
            }
            model
        }

        fn config() -> ModelConfig {
            ModelConfig::default_test_config()
        }

        /// The one block the fixture declares, loaded.
        ///
        /// The two per-head norms are empty because this fixture declares no QK
        /// norm, which is Gemma 2's graph; the published `gemma-embedding` file
        /// ships both, and `weights.rs`'s loader fills them from its bytes.
        fn layer(&self) -> LayerWeights<'_> {
            LayerWeights {
                attn_norm: &self.ones[..64],
                attn_q_norm: &[],
                attn_k_norm: &[],
                attn_q: &self.id_w,
                attn_k: &self.id_w,
                attn_v: &self.id_w,
                attn_o: &self.id_w,
                post_attn_norm: &self.ones[..64],
                ffn_norm: &self.ones[..64],
                ffn_gate: &self.ffn_gate_w,
                ffn_up: &self.ffn_gate_w,
                ffn_down: &self.ffn_down_w,
                post_ffn_norm: &self.ones[..64],
                weight_type: 0, // F32
            }
        }

        /// The fixture as the engine sees it, over the caller's block list.
        ///
        /// WHY the list arrives from the caller: its length is the model's declared
        /// block count, which is a property of the config under test and not of this
        /// fixture. A `ModelWeights` that could only carry two blocks was the same
        /// kind of limit as the ceilings this module no longer has.
        fn weights<'b>(&'b self, blocks: &'b [Option<LayerWeights<'b>>]) -> ModelWeights<'b> {
            ModelWeights {
                token_embd: TokenEmbedding::Table {
                    bytes: &self.embd,
                    rows: 290,
                    dtype: 0,
                },
                layers: blocks,
                final_norm: &self.ones[..64],
                output_weight: Some(&self.head_w),
                output_weight_type: 0,
            }
        }
    }

    #[test]
    fn test_forward_step_and_generate_on_real_fixture() {
        let fixture = LittleModel::new();
        let config = LittleModel::config();
        let blocks = [Some(fixture.layer())];
        let model = fixture.weights(&blocks);

        let prompt = [crate::tokenizer::TOKEN_BOS, 261]; // BOS, " the"
        let mut generated_tokens = [0u32; 4];
        let mut gen_idx = 0usize;

        let num_gen = generate(
            &prompt,
            &model,
            &config,
            3,
            0.0, // argmax sampling
            &|| false,
            |tok| {
                if gen_idx < generated_tokens.len() {
                    generated_tokens[gen_idx] = tok;
                    gen_idx += 1;
                }
                true
            },
        )
        .expect("generation executes");

        assert!(num_gen > 0);
        assert_eq!(num_gen, gen_idx);
        // Verify real tokens are generated
        for &tok in &generated_tokens[..num_gen] {
            assert!(tok < config.vocab_size as u32);
        }
    }

    /// The embedding service consumes the forward pass's normalised residual
    /// stream, so it leaves in `scratch.hidden` instead of being re-derived from
    /// somewhere else. A destination too small for the stream the file declares is
    /// refused by the pooling head that writes it: a vector truncated to 8 dims is
    /// not that model's embedding.
    #[test]
    fn test_forward_step_hands_back_the_normalised_residual_stream() {
        let fixture = LittleModel::new();
        let config = LittleModel::config();
        let blocks = [Some(fixture.layer())];
        let model = fixture.weights(&blocks);
        let (mut scratch, mut kv_cache) = fit(&config);

        forward_step(
            crate::tokenizer::TOKEN_BOS,
            0,
            &model,
            &config,
            &mut kv_cache,
            &mut scratch,
        )
        .expect("the fixture holds the row this token asks for");

        assert!(
            scratch.hidden[..config.hidden_dim].iter().any(|&v| v != 0.0),
            "the residual stream left the step empty"
        );
        // This fixture's output head is the identity, so the logits are the stream
        // read straight through. Matching them dimension by dimension says the
        // vector handed back is the one the model's own head was fed, which is the
        // property a pooling head depends on.
        for i in 0..config.hidden_dim {
            assert!(
                (scratch.hidden[i] - scratch.logits[i]).abs() < 1e-6,
                "dim {} handed back {} while the output head was fed {}",
                i,
                scratch.hidden[i],
                scratch.logits[i]
            );
        }
        assert_eq!(kv_cache.len, 1);
        // The row the step just wrote is in the cache where the addressing says it
        // is, and not only in the buffers the caller can see.
        assert!(kv_cache
            .k_row(0, 0, 0)
            .expect("position 0 of block 0 is addressed")
            .iter()
            .any(|&v| v != 0.0));

        let mut too_narrow = [0.0f32; 8];
        assert_eq!(
            extract_embedding(
                &[crate::tokenizer::TOKEN_BOS],
                &model,
                &config,
                &mut too_narrow,
            ),
            Err(InferenceError::BufferTooSmall)
        );
    }

    /// A declaration wider than the memory it was handed is refused, never
    /// clipped to fit. An answer computed at 64 dims wide and reported as the
    /// answer of a model that declared 72 would be the engine describing a model
    /// it never ran.
    ///
    /// WHY the scratch and the cache are cut once, for the fixture, and then asked
    /// to run the variants: that is the only way to show the check is on the shape
    /// of the memory rather than on a number compiled into the engine. Each variant
    /// outgrows one part of the fixture's allocation -- the stream, the
    /// feed-forward, the cached head's width, the sequence depth, the block count.
    #[test]
    fn test_forward_step_refuses_a_shape_wider_than_its_buffers() {
        let fixture = LittleModel::new();
        let base = LittleModel::config();
        let blocks = [Some(fixture.layer())];
        let model = fixture.weights(&blocks);
        let (mut scratch, mut kv_cache) = fit(&base);

        for wider in [
            ModelConfig {
                hidden_dim: 72,
                ..base
            },
            ModelConfig {
                intermediate_dim: 512,
                ..base
            },
            ModelConfig {
                num_heads: 1,
                head_dim: 40,
                ..base
            },
            ModelConfig {
                seq_len: 128,
                ..base
            },
            ModelConfig {
                num_layers: 2,
                ..base
            },
        ] {
            assert_eq!(
                forward_step(
                    crate::tokenizer::TOKEN_BOS,
                    0,
                    &model,
                    &wider,
                    &mut kv_cache,
                    &mut scratch,
                ),
                Err(InferenceError::BufferTooSmall),
                "{wider:?} ran on memory cut for {base:?}"
            );
        }

        // Two key/value heads where the kernel walks one is the graph's gap, and it
        // is reported as the declaration that says so rather than as a size.
        assert_eq!(
            forward_step(
                crate::tokenizer::TOKEN_BOS,
                0,
                &model,
                &ModelConfig {
                    num_kv_heads: 2,
                    ..base
                },
                &mut kv_cache,
                &mut scratch,
            ),
            Err(InferenceError::ModelDeclaration(
                "<arch>.attention.head_count_kv",
                Some(2),
                1
            ))
        );

        // Four heads of 32 do not tile a 64-wide stream, so the fourth would read
        // past the projection that filled it.
        assert_eq!(
            forward_step(
                crate::tokenizer::TOKEN_BOS,
                0,
                &model,
                &ModelConfig {
                    num_heads: 4,
                    ..base
                },
                &mut kv_cache,
                &mut scratch,
            ),
            Err(InferenceError::ModelDeclaration(
                "<arch>.attention.head_count x key_length",
                Some(128),
                64
            ))
        );

        // Every refusal above ran before the graph did, so the cache still holds
        // what it arrived holding: nothing.
        assert_eq!(kv_cache.len, 0);

        // A block the file declares and nobody loaded is a missing weight, not a
        // block to walk around -- and the memory here is deliberately big enough.
        let deeper = ModelConfig {
            num_layers: 2,
            ..base
        };
        let mut deep_cache = KvCache::new(&deeper).expect("two blocks of this shape fit");
        assert_eq!(
            forward_step(
                crate::tokenizer::TOKEN_BOS,
                0,
                &model,
                &deeper,
                &mut deep_cache,
                &mut scratch,
            ),
            Err(InferenceError::CorruptedWeights)
        );

        // A final norm narrower than the stream would leave its tail unscaled, and
        // `rms_norm` walks the shorter of the two, so the gap is refused outright.
        let mut bare = fixture.weights(&blocks);
        bare.final_norm = &fixture.ones[..8];
        assert_eq!(
            forward_step(
                crate::tokenizer::TOKEN_BOS,
                0,
                &bare,
                &base,
                &mut kv_cache,
                &mut scratch,
            ),
            Err(InferenceError::CorruptedWeights)
        );
    }

    /// The row a token asks for either is in the file or is not. Building one
    /// from the token id made every number downstream of it a description of
    /// nothing, while reading a quantised table with the wrong row width would
    /// read a neighbour's block headers as activations.
    #[test]
    fn test_forward_step_refuses_an_embedding_the_file_does_not_hold() {
        let fixture = LittleModel::new();
        let config = LittleModel::config();
        let blocks = [Some(fixture.layer())];
        let mut no_rows = fixture.weights(&blocks);
        no_rows.token_embd = TokenEmbedding::Table {
            bytes: &[],
            rows: 0,
            dtype: 0,
        };
        let (mut scratch, mut kv_cache) = fit(&config);

        assert_eq!(
            forward_step(
                261,
                0,
                &no_rows,
                &config,
                &mut kv_cache,
                &mut scratch,
            ),
            Err(InferenceError::CorruptedWeights)
        );

        // A table that stops inside the last row is the same lie as no table at
        // all: the row is read as far as it exists and the rest would be invented.
        let mut truncated = fixture.weights(&blocks);
        let one_row_short = fixture.embd.len() - 4;
        truncated.token_embd = TokenEmbedding::Table {
            bytes: &fixture.embd[..one_row_short],
            rows: 290,
            dtype: 0,
        };
        assert_eq!(
            forward_step(
                289,
                0,
                &truncated,
                &config,
                &mut kv_cache,
                &mut scratch,
            ),
            Err(InferenceError::CorruptedWeights)
        );

        // A dtype the engine has no row layout for is refused by its number, not
        // read as a type it does know. Q4_1 (3) is the one the published files use
        // that this engine has no dequantiser for.
        let mut unsupported = fixture.weights(&blocks);
        unsupported.token_embd = TokenEmbedding::Table {
            bytes: &fixture.embd,
            rows: 290,
            dtype: 3,
        };
        assert_eq!(
            forward_step(
                261,
                0,
                &unsupported,
                &config,
                &mut kv_cache,
                &mut scratch,
            ),
            Err(InferenceError::UnsupportedQuantization(3))
        );

        // A row tagged with the token it was read for answers only that token: the
        // loader that reads one row out of a 213 MB table cannot be handed the row
        // for token 5 and asked to answer token 6.
        let row_for_five = [0u8; 64 * FLOAT_BYTES];
        let mislabelled = fixture.weights(&blocks);
        let mut mislabelled = mislabelled;
        mislabelled.token_embd = TokenEmbedding::Row {
            bytes: &row_for_five,
            dtype: 0,
            token: 5,
        };
        assert_eq!(
            forward_step(
                6,
                0,
                &mislabelled,
                &config,
                &mut kv_cache,
                &mut scratch,
            ),
            Err(InferenceError::CorruptedWeights)
        );
        // None of the three wrote a position into the cache.
        assert_eq!(kv_cache.len, 0);
    }

    /// What the search index stores is the mean of the positions the model ran,
    /// L2-normalised. Neither the last row of the KV cache nor a slice of the
    /// logits is a sentence embedding, so this checks the pooled vector against
    /// the same positions run one at a time.
    #[test]
    fn test_extract_embedding_mean_pools_the_positions_and_normalises_them() {
        let fixture = LittleModel::new();
        let config = LittleModel::config();
        let blocks = [Some(fixture.layer())];
        let model = fixture.weights(&blocks);
        let tokens = [crate::tokenizer::TOKEN_BOS, 261, 4];
        let mut embedding = [0.0f32; LITTLE_HIDDEN];

        let dim = extract_embedding(&tokens, &model, &config, &mut embedding)
            .expect("the fixture is a runnable model");
        assert_eq!(dim, config.hidden_dim);

        // The same three positions run one at a time, by hand, against memory cut
        // for this config: the pooled vector has to be their mean and nothing else.
        let (mut scratch, mut kv_cache) = fit(&config);
        let mut summed = [0.0f32; LITTLE_HIDDEN];
        for (pos, &token) in tokens.iter().enumerate() {
            forward_step(
                token,
                pos,
                &model,
                &config,
                &mut kv_cache,
                &mut scratch,
            )
            .expect("the same tokens run the same graph");
            for i in 0..dim {
                summed[i] += scratch.hidden[i];
            }
        }
        let mut sum_sq = 0.0f32;
        for i in 0..dim {
            summed[i] /= tokens.len() as f32;
            sum_sq += summed[i] * summed[i];
        }
        let inv_norm = 1.0f32 / f32_sqrt(sum_sq);
        for i in 0..dim {
            let want = summed[i] * inv_norm;
            assert!(
                (embedding[i] - want).abs() < 1e-6,
                "dim {} holds {} where the mean of the positions is {}",
                i,
                embedding[i],
                want
            );
        }

        // Unit length is what ranking stored vectors by cosine assumes.
        let mut got_sq = 0.0f32;
        for &v in embedding[..dim].iter() {
            got_sq += v * v;
        }
        assert!(
            (got_sq - 1.0).abs() < 1e-4,
            "the pooled vector has squared length {}",
            got_sq
        );
    }

    #[test]
    fn test_model_config_refuses_more_than_one_key_value_head() {
        let mut shared = declared(64, 128, 1, 2, 32, 290);
        shared.num_kv_heads = Some(2);
        assert_eq!(
            ModelConfig::from_gguf(&shared),
            Err(InferenceError::ModelDeclaration(
                "<arch>.attention.head_count_kv",
                Some(2),
                1
            ))
        );

        // A file that states no count is the format's multi-head case, and the
        // refusal reports the count that follows rather than nothing at all.
        let unstated = declared(64, 128, 1, 2, 32, 290);
        assert_eq!(
            ModelConfig::from_gguf(&unstated),
            Err(InferenceError::ModelDeclaration(
                "<arch>.attention.head_count_kv",
                Some(2),
                1
            ))
        );

        // One shared head is what the cache is shaped for.
        let mut single = declared(64, 128, 1, 2, 32, 290);
        single.num_kv_heads = Some(1);
        assert_eq!(
            ModelConfig::from_gguf(&single).map(|c| c.num_kv_heads),
            Ok(1)
        );
    }

    /// A model file that states every dimension the engine asks it for.
    fn declared(
        hidden: u32,
        ffn: u32,
        layers: u32,
        heads: u32,
        head_dim: u32,
        vocab: u32,
    ) -> crate::gguf::GgufModelInfo {
        let mut info = crate::gguf::GgufModelInfo::empty();
        let arch = b"gemma2";
        info.architecture[..arch.len()].copy_from_slice(arch);
        info.architecture_reading = crate::gguf::StringReading::Read(arch.len());
        info.hidden_dim = Some(hidden);
        info.intermediate_dim = Some(ffn);
        info.num_layers = Some(layers);
        info.num_heads = Some(heads);
        info.head_dim = Some(head_dim);
        info.vocab_size = Some(vocab);
        info
    }

    // --- the identity the file declares for itself -----------------------------
    //
    // WHY these cases are about readings and not about names: the service used to
    // call a model by the path its bytes were opened at, and every name below is
    // what the file's own metadata states. `ModelIdentity::from_gguf` takes no
    // path, so a filename has nowhere to enter the report -- and each case below
    // refuses one of the four ways a reader can quietly invent a name anyway: the
    // stem of the file, the architecture's vendor, the shape of the fixture, or a
    // silence the walk never proved.

    /// Completes the pair counts of a hand-written record so the reading can say
    /// whether it got to the end of the file it describes.
    fn walked_every_pair(info: &mut crate::gguf::GgufModelInfo, pairs: u64) {
        info.kv_pair_count = Some(pairs);
        info.pairs_walked = pairs;
    }

    #[test]
    fn test_model_identity_is_the_name_the_file_gives_itself() {
        let mut info = declared(64, 128, 2, 2, 32, 290);
        let name = b"Falcon Junior";
        info.name[..name.len()].copy_from_slice(name);
        info.name_reading = crate::gguf::StringReading::Read(name.len());
        let family = b"gemma";
        info.tokenizer_model[..family.len()].copy_from_slice(family);
        info.tokenizer_model_reading = crate::gguf::StringReading::Read(family.len());
        info.file_type = Some(7);
        info.quantization_version = Some(1);
        info.tensor_count = Some(334);
        walked_every_pair(&mut info, 11);

        let identity = ModelIdentity::from_gguf(&info);
        assert_eq!(identity.name, StrField::Declared("Falcon Junior"));
        assert_eq!(identity.architecture, StrField::Declared("gemma2"));
        assert_eq!(identity.tokenizer_model, StrField::Declared("gemma"));
        assert_eq!(identity.quantization_type, NumField::Declared(7));
        assert_eq!(identity.quantization_version, NumField::Declared(1));
        assert_eq!(identity.tensor_count, NumField::Declared(334));
        assert_eq!(identity.metadata_pair_count, NumField::Declared(11));
        assert_eq!(identity.embedding_length, NumField::Declared(64));
        assert!(!identity.declares_nothing());
    }

    /// A file that names no model, no tokenizer family and no quantisation is
    /// reported as naming none of them. Every stand-in this could have used -- the
    /// filename's stem, "gemma2" turned into the vendor's product name, the
    /// `unknown` a placeholder file is usually shipped with -- would be the
    /// platform describing a model the model did not describe.
    #[test]
    fn test_model_identity_reports_a_field_the_file_never_named_as_unnamed() {
        let mut info = declared(64, 128, 2, 2, 32, 290);
        walked_every_pair(&mut info, 9);

        let identity = ModelIdentity::from_gguf(&info);
        assert_eq!(identity.name, StrField::Absent);
        assert_eq!(identity.tokenizer_model, StrField::Absent);
        assert_eq!(identity.quantization_type, NumField::Absent);
        assert_eq!(identity.quantization_version, NumField::Absent);
        assert_eq!(identity.tensor_count, NumField::Absent);
        assert_eq!(identity.metadata_pair_count, NumField::Declared(9));
        // What the file did state stays stated, so an unnamed file is not an
        // unread one.
        assert_eq!(identity.architecture, StrField::Declared("gemma2"));
        assert_eq!(identity.embedding_length, NumField::Declared(64));
        assert!(!identity.declares_nothing(), "the architecture is declared");
    }

    #[test]
    fn test_a_file_that_names_neither_model_nor_architecture_declares_nothing() {
        let unread = crate::gguf::GgufModelInfo::empty();
        let identity = ModelIdentity::from_gguf(&unread);
        assert_eq!(identity.name, StrField::Absent);
        assert_eq!(identity.architecture, StrField::Absent);
        assert_eq!(identity.tokenizer_model, StrField::Absent);
        assert!(identity.declares_nothing());
        assert_eq!(identity.name.held(), None, "an absent name has no stand-in");
        assert_eq!(identity.embedding_length.value(), None);
    }

    /// The two silences are not one silence: identical readings, and the header's
    /// own pair count decides whether the file left the key out or the walk never
    /// got that far. Collapsing them would tell the person that a real Gemma file
    /// named no model, when what happened is that the walk stopped at a 262,144
    /// entry vocabulary.
    #[test]
    fn test_model_identity_tells_an_unnamed_file_from_one_the_walk_did_not_finish() {
        let mut finished = declared(64, 128, 2, 2, 32, 290);
        finished.name_reading = crate::gguf::StringReading::Absent;
        walked_every_pair(&mut finished, 40);
        let identity = ModelIdentity::from_gguf(&finished);
        assert_eq!(identity.name, StrField::Absent);
        assert_eq!(identity.tokenizer_model, StrField::Absent);
        assert_eq!(identity.quantization_type, NumField::Absent);
        assert_eq!(identity.embedding_length, NumField::Declared(64));

        let mut stopped_early = finished;
        stopped_early.pairs_walked = 4;
        assert!(
            stopped_early.walk_stopped_early(),
            "the record must know it did not finish"
        );
        let identity = ModelIdentity::from_gguf(&stopped_early);
        assert_eq!(identity.name, StrField::NotReached);
        assert_eq!(identity.tokenizer_model, StrField::NotReached);
        assert_eq!(identity.quantization_type, NumField::NotReached);
        assert_eq!(identity.embedding_length, NumField::Declared(64));
        // A value the walk did hold is still held, whatever it later gave up on.
        assert_eq!(identity.architecture, StrField::Declared("gemma2"));
    }

    /// A name the reader will not copy is reported by the length the file stated,
    /// never as the prefix the window happened to have room for.
    #[test]
    fn test_model_identity_reports_a_refused_name_by_the_length_the_file_stated() {
        let mut info = declared(64, 128, 2, 2, 32, 290);
        walked_every_pair(&mut info, 9);
        info.name_reading = crate::gguf::StringReading::RefusedTooLong(60);
        let identity = ModelIdentity::from_gguf(&info);
        assert_eq!(identity.name, StrField::RefusedTooLong(60));
        assert_eq!(identity.name.held(), None);

        // Bytes that are held but are not text are a third answer again: the file
        // stated a length and the reader copied it, and what it copied cannot be
        // quoted as a name.
        let mut unprintable = info;
        unprintable.name_reading = crate::gguf::StringReading::Read(3);
        unprintable.name[..3].copy_from_slice(&[0xff, 0xfe, 0xfd]);
        let identity = ModelIdentity::from_gguf(&unprintable);
        assert_eq!(identity.name, StrField::NotText(3));
        assert_eq!(identity.name.held(), None);
    }

    #[test]
    fn test_model_config_is_built_from_what_the_file_declares() {
        let mut info = declared(64, 128, 2, 2, 32, 290);
        info.num_kv_heads = Some(1);
        info.rms_norm_eps = Some(2.5e-5);
        info.rope_theta = Some(1_000_000.0);

        let config = ModelConfig::from_gguf(&info).expect("the file configures the engine");
        assert_eq!(config.hidden_dim, 64);
        assert_eq!(config.intermediate_dim, 128);
        assert_eq!(config.num_layers, 2);
        assert_eq!(config.num_heads, 2);
        assert_eq!(config.num_kv_heads, 1);
        assert_eq!(config.head_dim, 32);
        assert_eq!(config.vocab_size, 290);
        assert_eq!(config.rms_norm_eps, 2.5e-5);
        assert_eq!(config.rope_theta, 1_000_000.0);

        // A file that states neither epsilon nor theta takes the format's own
        // defaults, which is what GGUF specifies for an absent key.
        info.rms_norm_eps = None;
        info.rope_theta = None;
        let defaulted = ModelConfig::from_gguf(&info).expect("the two floats are optional");
        assert_eq!(defaulted.rms_norm_eps, 1e-6);
        assert_eq!(defaulted.rope_theta, 10000.0);

        // The cache depth is declared too. A file that states no `context_length`
        // gets `DEFAULT_SEQ_LEN`, and it is recorded on the config where the budget
        // can price it rather than hidden in a constant the caller cannot see.
        assert_eq!(defaulted.seq_len, DEFAULT_SEQ_LEN);
        info.context_length = Some(2048);
        let deep = ModelConfig::from_gguf(&info).expect("the file states its own run length");
        assert_eq!(deep.seq_len, 2048);
        assert_eq!(
            deep.kv_cache_bytes(),
            2 * 2 * 2048 * 1 * 32 * FLOAT_BYTES,
            "two blocks of 2048 positions, one key/value head 32 wide"
        );
    }

    /// The published `embeddinggemma-300M-Q8_0.gguf` exactly as its own metadata
    /// states it: 768-wide stream, 1152-wide feed-forward, 24 blocks, 3 heads 256
    /// wide over one key/value head, 262,144 tokens, a 2048-position context and a
    /// 512 sliding window. The numbers are read out of the file with
    /// `tools/../target/verify` metadata dump and recorded here so the shape is a
    /// test and not a recollection.
    fn published_file() -> crate::gguf::GgufModelInfo {
        let mut info = declared(768, 1152, 24, 3, 256, 262_144);
        info.num_kv_heads = Some(1);
        info.context_length = Some(2048);
        info.sliding_window = Some(512);
        info.rms_norm_eps = Some(1e-6);
        info.rope_theta = Some(1_000_000.0);
        info
    }

    /// The one size limit the engine has: the scratch a declared shape needs,
    /// against the bytes the device offers. Tested from both ends, because a
    /// budget that only ever refuses is indistinguishable from a ceiling.
    #[test]
    fn test_a_shape_is_refused_by_the_bytes_it_needs_and_accepted_by_the_bytes_it_needs() {
        // Gemma 2B published at its own 8192-position context: the cache alone is
        // 2 x 26 x 8192 x 1 x 256 x 4 = 436,207,616 bytes, so the shape is over
        // budget by a factor of three and no dimension of it is individually odd.
        let mut huge = declared(2304, 4608, 26, 8, 256, 256_000);
        huge.num_kv_heads = Some(1);
        huge.context_length = Some(8192);
        assert_eq!(
            ModelConfig::from_gguf(&huge),
            Err(InferenceError::ScratchTooLarge(437_421_056, SCRATCH_BYTE_BUDGET))
        );

        // The same file at the shortest context anything of this family ships with
        // is 28,445,696 bytes: the engine accepts it, because the memory really is
        // there. What it still cannot do is load the weights or run this model's
        // graph, which the two refusals below say plainly.
        let mut small = declared(2304, 4608, 26, 8, 256, 256_000);
        small.num_kv_heads = Some(1);
        small.context_length = Some(512);
        let accepted = ModelConfig::from_gguf(&small).expect("28 MiB of cache is under budget");
        assert_eq!(accepted.scratch_bytes(), 28_445_696);
        assert_eq!(accepted.check_scratch_budget(), Ok(()));

        // Refusing on bytes is not the same as refusing on a dimension, and the
        // difference is what the person acting on the message needs: the number
        // that was wanted and the number that was given, both of them.
        let Err(InferenceError::ScratchTooLarge(wanted, budget)) =
            ModelConfig::from_gguf(&huge).map(|_| ())
        else {
            panic!("a 416 MiB cache was not refused for its bytes");
        };
        assert!(wanted > budget);
        assert_eq!(budget, SCRATCH_BYTE_BUDGET);
        assert_eq!(wanted, huge_priced().scratch_bytes());
    }

    /// The refused shape, priced directly from its declared dimensions.
    fn huge_priced() -> ModelConfig {
        ModelConfig {
            hidden_dim: 2304,
            intermediate_dim: 4608,
            num_layers: 26,
            num_heads: 8,
            num_kv_heads: 1,
            head_dim: 256,
            vocab_size: 256_000,
            seq_len: 8192,
            rms_norm_eps: 1e-6,
            rope_theta: 1_000_000.0,
        }
    }

    /// The published model, by the shape its own file states. This is the result
    /// this change exists to produce: the file is no longer refused for being a
    /// real model, and the numbers below are the arithmetic the budget comment
    /// claims.
    #[test]
    fn test_model_config_accepts_the_published_embedding_model_by_its_budget() {
        let config = ModelConfig::from_gguf(&published_file())
            .expect("the published file's shape is representable on this device");
        assert_eq!(config.hidden_dim, 768);
        assert_eq!(config.intermediate_dim, 1152);
        assert_eq!(config.num_layers, 24);
        assert_eq!(config.num_heads, 3);
        assert_eq!(config.num_kv_heads, 1);
        assert_eq!(config.head_dim, 256);
        assert_eq!(config.vocab_size, 262_144);
        assert_eq!(config.seq_len, 2048);
        assert_eq!(config.rope_theta, 1_000_000.0);
        // 2 x 24 x 2048 x 1 x 256 x 4: 96 MiB, which is 100 times the stack it
        // used to have to live on.
        assert_eq!(config.kv_cache_bytes(), 100_663_296);
        assert_eq!(config.scratch_bytes(), 101_767_680);
        assert_eq!(config.check_scratch_budget(), Ok(()));
        assert!(config.scratch_bytes() < SCRATCH_BYTE_BUDGET);
    }

    /// The claim the change is named for, tested against the allocation rather than
    /// against a type: a 24-block, 2048-position, 256-wide cache is memory the
    /// service can be handed, and every row of the file's own declaration is
    /// addressable in it.
    #[test]
    fn test_the_published_models_cache_is_representable_and_addressed() {
        let config = ModelConfig::from_gguf(&published_file()).expect("the shape is under budget");
        let mut cache = KvCache::new(&config).expect("the device has the 96 MiB");
        assert_eq!(cache.allocated_bytes(), 100_663_296);
        assert_eq!(cache.k.len(), 12_582_912);
        assert_eq!(cache.v.len(), 12_582_912);
        assert_eq!(cache.seq_len, 2048);
        assert_eq!(cache.num_layers, 24);

        // The last position of the last block: the row no fixed array in this
        // engine ever had, and the one a 2048-token sequence writes.
        let last = 12_582_912 - 256;
        assert_eq!(cache.row_start(23, 2047, 0), Some(last));
        cache
            .write(23, 2047, 0, &[7.5f32; 256], &[-2.0f32; 256])
            .expect("the last row is in the cache");
        assert_eq!(cache.k_row(23, 2047, 0).map(|row| row[255]), Some(7.5f32));
        assert_eq!(cache.v_row(23, 2047, 0).map(|row| row[0]), Some(-2.0f32));
        assert_eq!(cache.k[12_582_911], 7.5f32);

        // Past the declared sequence there is no row, and the cache says so instead
        // of wrapping into row 0. A head the file did not declare is the same.
        assert_eq!(cache.row_start(23, 2048, 0), None);
        assert_eq!(
            cache.write(23, 2048, 0, &[0.0f32; 256], &[0.0f32; 256]),
            Err(InferenceError::ContextLengthExceeded)
        );
        assert_eq!(cache.k_row(0, 0, 1), None);
        assert_eq!(cache.row_start(24, 0, 0), None);
        // A key wider than the row would be truncated, and a truncated key is a
        // different model's key.
        assert_eq!(
            cache.write(0, 0, 0, &[0.0f32; 128], &[0.0f32; 128]),
            Err(InferenceError::BufferTooSmall)
        );
    }

    /// The 333 MB file's own bytes, read, priced and allocated by the code that
    /// ships.
    ///
    /// WHY this exists beside the fixture test above: every other case here hands
    /// `from_gguf` a hand-written description of the published file, so all of them
    /// pass when the reader is wrong about a real file -- the fixture only has to
    /// repeat the mistake. This one runs `read_model_info_stream` over the first
    /// kilobytes of `embeddinggemma-300M-Q8_0.gguf` itself, holds what the reader
    /// took from those bytes against what the independent Python probe recorded
    /// (`target/verify/gguf_real_file_probe.py` writes
    /// `target/verify/models/gguf_metadata_dump.json`, and imports nothing of the
    /// kernel's parser), then prices the shape and takes the memory off the heap.
    ///
    /// WHY it is gated on an environment variable: the file is 333 MB and is not
    /// in the repository, so its host path arrives in `INTEROS_REAL_GGUF_PATH`.
    /// Without that variable the test prints what it did not check and returns --
    /// the record then says the real file went unverified, which is the honest
    /// alternative to a green test that silently read nothing.
    ///
    /// To run it:
    /// `INTEROS_REAL_GGUF_PATH=/c/path/to/embeddinggemma-300M-Q8_0.gguf cargo test -p interos-system-service real_published_file`
    #[test]
    fn test_the_real_published_file_is_priced_and_allocated() {
        extern crate std as file_bytes;
        use file_bytes::io::Read;

        let Some(path) = file_bytes::env::var("INTEROS_REAL_GGUF_PATH").ok() else {
            file_bytes::println!(
                "NOT VERIFIED against a real model file: INTEROS_REAL_GGUF_PATH is unset, so \
                 the 333 MB GGUF was never opened and only hand-written fixtures were priced."
            );
            return;
        };

        // The metadata the walk needs is under a kilobyte into a 6.5 MB block, but
        // the window is 64 KiB so the walk gets to be as greedy as it likes with
        // the keys before the vocabulary, and the position assertion below is what
        // proves it did not step the 262,144 strings.
        const WINDOW: usize = 64 * 1024;
        let mut bytes: file_bytes::vec::Vec<u8> = file_bytes::vec![0u8; WINDOW];
        let mut file = file_bytes::fs::File::open(&path)
            .unwrap_or_else(|error| panic!("the real file at {path} could not be opened: {error}"));
        let mut filled = 0usize;
        while filled < WINDOW {
            match file.read(&mut bytes[filled..]) {
                Ok(0) => break,
                Ok(got) => filled += got,
                Err(error) => panic!("the real file at {path} could not be read: {error}"),
            }
        }
        assert!(filled >= WINDOW / 2, "only {filled} bytes of the real file arrived");
        let head = &bytes[..filled];

        // 1. What the file declares, read by the reader that ships.
        let mut stream = crate::gguf::GgufStream::from_slice(head);
        let info = crate::gguf::read_model_info_stream(&mut stream).unwrap_or_else(|error| {
            panic!("the real file at {path} did not configure the engine: {error:?}")
        });
        assert_eq!(info.architecture(), Some("gemma-embedding"));
        assert_eq!(info.hidden_dim, Some(768), "embedding_length");
        assert_eq!(info.intermediate_dim, Some(1152), "feed_forward_length");
        assert_eq!(info.num_layers, Some(24), "block_count");
        assert_eq!(info.num_heads, Some(3), "attention.head_count");
        assert_eq!(info.num_kv_heads, Some(1), "attention.head_count_kv");
        assert_eq!(info.head_dim, Some(256), "attention.key_length");
        assert_eq!(info.context_length, Some(2048), "context_length");
        assert_eq!(info.sliding_window, Some(512), "attention.sliding_window");
        assert_eq!(info.rope_theta, Some(1_000_000.0), "rope.freq_base");
        assert_eq!(info.vocab_size, Some(262_144), "tokenizer.ggml.tokens, counted");
        assert!(
            stream.position() < WINDOW / 8,
            "the walk ran to byte {} of the metadata, so the vocabulary was loaded and \
             not merely counted",
            stream.position()
        );

        // 2. The price the file's own shape carries -- the same two numbers the
        // fixture test asserts, which is what ties that hand-written fixture to
        // the file rather than to whoever typed it in.
        let config = ModelConfig::from_gguf(&info)
            .unwrap_or_else(|error| panic!("the real file at {path} was refused: {error:?}"));
        assert_eq!(config.kv_cache_bytes(), 100_663_296);
        assert_eq!(config.scratch_bytes(), 101_767_680);
        assert_eq!(config.check_scratch_budget(), Ok(()));
        file_bytes::println!(
            "REAL FILE {path}: 24 x 2048 x 1 x 256 cache = {} bytes + {} scratch = {} of \
             the {SCRATCH_BYTE_BUDGET} byte budget, {spare} spare",
            config.kv_cache_bytes(),
            Scratch::bytes_for(&config),
            config.scratch_bytes(),
            spare = SCRATCH_BYTE_BUDGET - config.scratch_bytes(),
        );

        // 3. The memory is taken, off the stack, and priced the same way.
        let scratch = Scratch::new(&config).expect("the device holds the real file's scratch");
        assert_eq!(scratch.allocated_bytes(), Scratch::bytes_for(&config));
        let cache = KvCache::new(&config).expect("the device holds the real file's 96 MiB");
        assert_eq!(cache.allocated_bytes(), config.kv_cache_bytes());
        assert_eq!(
            cache.allocated_bytes() + scratch.allocated_bytes(),
            config.scratch_bytes()
        );
        // 96 MiB against the 256 KiB thread stack this engine used to run on: the
        // number is the whole reason for the change, and the last row of the last
        // block is the address no fixed array in the old engine could name.
        assert!(
            cache.allocated_bytes() > 256 * 1024,
            "the real file's cache is {} bytes, which is not stack-sized memory",
            cache.allocated_bytes()
        );
        assert_eq!(cache.row_start(23, 2047, 0), Some(12_582_912 - 256));
        assert_eq!(cache.row_start(23, 2048, 0), None);

        // 4. The same file at the longest context its family ships with is refused,
        // for its bytes and with both numbers: the budget was not loosened until
        // this model happened to clear it.
        let longer = ModelConfig {
            seq_len: 8192,
            ..config
        };
        assert_eq!(
            longer.check_scratch_budget(),
            Err(InferenceError::ScratchTooLarge(403_782_144, SCRATCH_BYTE_BUDGET)),
            "the real file at 8192 positions has to be refused by the bytes it names"
        );
    }

    /// WHY the budget can be trusted: `Scratch::bytes_for` is the number the check
    /// compares, and `Scratch::new` is the memory the run actually holds. If the two
    /// disagreed, the one size limit left in the engine would be arithmetic nothing
    /// verified, so they are held to each other at both the little shape and the
    /// published one.
    #[test]
    fn scratch_pricing_matches_the_allocation() {
        let published = ModelConfig::from_gguf(&published_file()).expect("under budget");
        for config in [LittleModel::config(), published] {
            let scratch = Scratch::new(&config).expect("both shapes are under budget");
            assert_eq!(
                scratch.allocated_bytes(),
                Scratch::bytes_for(&config),
                "the price and the allocation disagree at {config:?}"
            );
            let cache = KvCache::new(&config).expect("both caches are under budget");
            assert_eq!(cache.allocated_bytes(), config.kv_cache_bytes());
            assert_eq!(
                config.scratch_bytes(),
                scratch.allocated_bytes() + cache.allocated_bytes()
            );
            assert!(config.scratch_bytes() <= SCRATCH_BYTE_BUDGET);
        }
    }

    /// A scratch that was not cut for this shape is refused before the graph runs,
    /// so the run cannot quietly read a narrower buffer as a wider model's.
    #[test]
    fn scratch_refuses_a_config_it_was_not_cut_for() {
        let base = LittleModel::config();
        let scratch = Scratch::new(&base).expect("the fixture is a tiny shape");
        scratch.fits(&base).expect("its own config");
        for wider in [
            ModelConfig {
                hidden_dim: 65,
                ..base
            },
            ModelConfig {
                intermediate_dim: 129,
                ..base
            },
            ModelConfig {
                seq_len: 65,
                ..base
            },
            ModelConfig {
                vocab_size: 291,
                ..base
            },
        ] {
            assert_eq!(
                scratch.fits(&wider),
                Err(InferenceError::BufferTooSmall),
                "{wider:?} was accepted by a scratch cut for {base:?}"
            );
        }
    }

    #[test]
    fn test_model_config_refuses_a_file_that_states_no_dimension() {
        let mut info = declared(64, 128, 1, 2, 32, 290);
        info.num_layers = None;
        assert_eq!(
            ModelConfig::from_gguf(&info),
            Err(InferenceError::ModelDeclaration(
                "<arch>.block_count",
                None,
                0
            ))
        );

        let mut zeroed = declared(64, 128, 0, 2, 32, 290);
        zeroed.hidden_dim = Some(0);
        assert_eq!(
            ModelConfig::from_gguf(&zeroed),
            Err(InferenceError::ModelDeclaration(
                "<arch>.embedding_length",
                Some(0),
                0
            ))
        );
    }

    #[test]
    fn test_model_config_refuses_heads_that_overrun_the_residual_stream() {
        // The head loop walks num_heads slices of head_dim out of a buffer that is
        // hidden_dim long, so four heads of 32 do not fit inside 64. One key/value
        // head, because a file with four of those is refused for the kernel's own
        // reason first -- `test_model_config_refuses_more_than_one_key_value_head`
        // covers that, and this one is about the span.
        let mut overhanging_heads = declared(64, 128, 1, 4, 32, 290);
        overhanging_heads.num_kv_heads = Some(1);
        assert_eq!(
            ModelConfig::from_gguf(&overhanging_heads),
            Err(InferenceError::ModelDeclaration(
                "<arch>.attention.head_count x key_length",
                Some(128),
                64
            ))
        );

        // A single head wider than the stream it lives in is the same arithmetic:
        // 256 declared as one head inside a 64-wide residual. What is no longer
        // refused is a head of 64 in a stream of 64, which is a model whose heads
        // do tile their own stream and whose cache the budget can price.
        let overhanging = declared(64, 128, 1, 1, 128, 290);
        assert_eq!(
            ModelConfig::from_gguf(&overhanging),
            Err(InferenceError::ModelDeclaration(
                "<arch>.attention.head_count x key_length",
                Some(128),
                64
            ))
        );
        let matching = declared(64, 128, 1, 1, 64, 290);
        assert_eq!(
            ModelConfig::from_gguf(&matching).map(|c| c.head_dim),
            Ok(64)
        );
    }
}
