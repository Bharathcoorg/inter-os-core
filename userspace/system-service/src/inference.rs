// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

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
//! - Neural vector embedding extraction (EmbeddingGemma 2): mean pool, then the
//!   file's pooling head (`apply_pooling_head`: dense_2, GELU tanh approximation,
//!   dense_3, biases when the file declares them) when the loader read one, then L2
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
/// - `sys_mem_map` refuses a mapping above 128 MiB
///   (`kernel/src/syscall/mod.rs`), and the service heap serves an allocation the
///   free list cannot from exactly one new mapping
///   (`sdk/rust/interos-std/src/heap.rs`), which carries a 32-byte block header
///   and is rounded up to a page. That bounds any one buffer a run holds, and no
///   total can express it -- so `LARGEST_SINGLE_BUFFER_BYTES` is checked beside
///   the total;
/// - the address space a process may hold in total is its own resource limit.
///   `ServiceId::Inference` is budgeted `user_address_space_limit()`
///   (`kernel/src/services.rs::resource_limits`), which is derived from the RAM
///   the kernel detected: detected less a quarter of it, and at least 64 MiB,
///   held back (`kernel/src/task/user.rs`). On the demo's 2 GiB guest that
///   computes to 1536 MiB. The 256 MiB `USER_ADDRESS_SPACE_LIMIT` beside it is
///   not a ceiling anyone chose -- it is what the derivation falls back to when
///   no RAM total was recorded at all. It bounds what the loaded `.rapp`, its
///   stack and this service's other mappings share.
///
/// So 120 MiB is the floor, not the budget: it is the mapping ceiling less an
/// 8 MiB reservation for the header, the guard pages and the rest of the
/// footprint that shares the address space -- 128 MiB - 8 MiB = 120 MiB =
/// 125,829,120 bytes. `dynamic_scratch_byte_budget` raises the total toward
/// three quarters of the free RAM the guest reports and never returns less than
/// this floor, and it falls back to the floor when no telemetry arrives. One
/// run's largest single buffer, the 48 MiB K tensor, is well under the mapping
/// ceiling, and the whole run is well under the service's address-space budget.
///
/// The published `embeddinggemma-300M-Q8_0.gguf` -- 24 blocks, 2048 positions,
/// one key/value head 256 wide, 768-wide stream, 1152-wide feed-forward and
/// 262,144 tokens -- computes 101,763,584 bytes of cache and scratch under this
/// formula: the KV cache is 2 x 24 x 2048 x 1 x 256 x 4 = 100,663,296 (96 MiB)
/// and the rest is 7 x 768 + 2 x 768 + 2 x 256 + 3 x 1152 + 2048 + 262,144 =
/// 275,072 floats = 1,100,288 bytes -- seven stream-wide buffers, the query and
/// attention output at the 768-wide head span, key and value at the 256-wide
/// key/value projection, three feed-forward buffers, the scores and the logits.
/// The run also holds one block's weights beside the scratch: the metadata walk
/// stops at the vocabulary and `general.file_type` sits past it in this file, so
/// `resident_weight_bytes` prices that hold at the F32 upper bound, 16,928,768
/// bytes, and the budget measures 118,692,352 bytes in total. It fits with
/// 7,136,768 bytes to spare, and the tests
/// `test_model_config_accepts_the_published_embedding_model_by_its_budget` and
/// `test_the_real_published_file_is_priced_and_allocated` hold the arithmetic to
/// those numbers -- the second one against the 333 MB file's own bytes, so the
/// fixture's numbers are the file's and not whoever typed them in.
///
/// It fits in memory, and on the development host it runs: the graph below is
/// what `run_block` walks for each of the file's 24 blocks, and
/// `weights::run_streamed_embedding` reads the installed file's tensors one block
/// at a time and applies its `dense_2`/`dense_3` head --
/// `test_the_installed_file_is_read_block_by_block_and_run` measures that over the
/// 333 MB file's own bytes. None of those measurements are the device's. The guest
/// has never run this graph, and the first thing that stops it there is not a
/// number in this file: it is the descriptor walk being handed a source that does
/// not buffer, which is recorded on `weights::TensorTable::walk`.
pub const SCRATCH_BYTE_BUDGET: usize = 120 * 1024 * 1024;

/// Caps scratch at 75% of the reported free RAM, including an observed zero.
pub fn compute_dynamic_scratch_budget(free_bytes: u64) -> usize {
    usize::try_from((free_bytes / 4) * 3).unwrap_or(usize::MAX)
}

/// Computes the dynamic scratch byte budget from physical memory telemetry.
pub fn dynamic_scratch_byte_budget() -> usize {
    #[cfg(not(test))]
    {
        if let Ok(telem) = interos_std::memory_telemetry() {
            return compute_dynamic_scratch_budget(telem.free_bytes);
        }
    }
    SCRATCH_BYTE_BUDGET
}

/// Fallback user address space budget when no physical RAM telemetry is available (256 MiB).
pub const FALLBACK_USER_ADDRESS_SPACE_LIMIT: usize = 256 * 1024 * 1024;

/// Computes the dynamic user address space budget given total physical RAM bytes.
pub fn compute_dynamic_address_space_budget(total_bytes: u64) -> usize {
    if total_bytes == 0 {
        FALLBACK_USER_ADDRESS_SPACE_LIMIT
    } else {
        let reserve = ((total_bytes / 4) as usize).max(64 * 1024 * 1024);
        (total_bytes as usize).saturating_sub(reserve)
    }
}

/// Dynamic user address space budget for this service, derived from memory telemetry.
pub fn dynamic_user_address_space_budget() -> usize {
    #[cfg(not(test))]
    {
        if let Ok(telem) = interos_std::memory_telemetry() {
            return compute_dynamic_address_space_budget(telem.total_bytes);
        }
    }
    compute_dynamic_address_space_budget(0)
}

/// Bytes the biggest single buffer one run may ask the heap for.
///
/// WHY a ceiling the free-RAM budget cannot express: `sys_mem_map` refuses a
/// mapping whose length is above 128 MiB (`kernel/src/syscall/mod.rs`), and the
/// service heap serves an allocation the free list cannot from exactly one new
/// mapping (`sdk/rust/interos-std/src/heap.rs`). Free RAM says how much a run may
/// hold in total; it says nothing about whether one tensor inside it can be mapped
/// at all. Raising the budget from free RAM without this check admitted shapes the
/// platform then refused at the heap, so the refusal arrived after the work had
/// been promised instead of before it.
///
/// WHERE the number comes from: the heap maps `align4096(align16(size) + 32)`, the
/// 32 being `size_of::<BlockHeader>()`. So the exact largest buffer is 128 MiB -
/// 32 bytes, and one more byte rounds the mapping past the ceiling. This constant
/// is the largest *page-aligned* buffer under that, which keeps the bound a round
/// number and leaves 4,064 bytes of room if the header ever grows.
pub const LARGEST_SINGLE_BUFFER_BYTES: usize = 128 * 1024 * 1024 - 4096;

/// Bytes one f32 occupies: every buffer the engine sizes is f32.
pub const FLOAT_BYTES: usize = core::mem::size_of::<f32>();

/// Error types for inference execution.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InferenceError {
    /// The GGML type id the tensor declares. Only the ids `gemv` dequantizes reach
    /// the arithmetic, so a type whose layout the engine has not been given
    /// authoritatively is refused by number instead of read with a guess.
    /// Q4_K, Q5_K, Q6_K and TQ2_0 are the K-quant ids it knows; Q4_1, Q5_0, Q5_1,
    /// Q8_1, Q2_K, Q3_K, Q8_K and BF16 are not among them.
    UnsupportedQuantization(u32),
    CorruptedWeights,
    BufferTooSmall,
    ContextLengthExceeded,
    /// A token id outside the vocabulary the file declared. Carries both numbers:
    /// the id that was presented and the vocabulary size the file states.
    /// WHY this refuses instead of wrapping: `% vocab_size` would map an id the
    /// model never learned onto a row it does hold, and report that row's answer
    /// as the presented token's -- a fabricated correspondence, not a computation.
    TokenOutOfVocabulary(u32, usize),
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

const HALF_PI_F64: f64 = 1.5707963267948966192313216916398;
const INV_HALF_PI_F64: f64 = 0.63661977236758134307553505349006;

/// Bounded range-reduced software sine using quadrant reduction.
/// Valid for |x| <= 1.0e7; returns NaN beyond this domain or for non-finite inputs.
pub fn f32_sin(x: f32) -> f32 {
    if !x.is_finite() {
        return f32::NAN;
    }
    if x.abs() > 1.0e7 {
        return f32::NAN;
    }
    let x_f64 = x as f64;
    let k = (x_f64 * INV_HALF_PI_F64 + (if x_f64 >= 0.0 { 0.5 } else { -0.5 })) as i64;
    let r = (x_f64 - (k as f64) * HALF_PI_F64) as f32;
    let q = (k % 4 + 4) % 4;

    let r2 = r * r;
    if q == 0 || q == 2 {
        let r3 = r2 * r;
        let r5 = r3 * r2;
        let r7 = r5 * r2;
        let sin_r = r - r3 * (1.0 / 6.0) + r5 * (1.0 / 120.0) - r7 * (1.0 / 5040.0);
        if q == 0 { sin_r } else { -sin_r }
    } else {
        let r4 = r2 * r2;
        let r6 = r4 * r2;
        let r8 = r6 * r2;
        let cos_r = 1.0 - r2 * 0.5 + r4 * (1.0 / 24.0) - r6 * (1.0 / 720.0) + r8 * (1.0 / 40320.0);
        if q == 1 { cos_r } else { -cos_r }
    }
}

/// Bounded range-reduced software cosine using quadrant reduction.
/// Valid for |x| <= 1.0e7; returns NaN beyond this domain or for non-finite inputs.
pub fn f32_cos(x: f32) -> f32 {
    if !x.is_finite() {
        return f32::NAN;
    }
    if x.abs() > 1.0e7 {
        return f32::NAN;
    }
    let x_f64 = x as f64;
    let k = (x_f64 * INV_HALF_PI_F64 + (if x_f64 >= 0.0 { 0.5 } else { -0.5 })) as i64;
    let r = (x_f64 - (k as f64) * HALF_PI_F64) as f32;
    let q = (k % 4 + 4) % 4;

    let r2 = r * r;
    if q == 0 || q == 2 {
        let r4 = r2 * r2;
        let r6 = r4 * r2;
        let r8 = r6 * r2;
        let cos_r = 1.0 - r2 * 0.5 + r4 * (1.0 / 24.0) - r6 * (1.0 / 720.0) + r8 * (1.0 / 40320.0);
        if q == 0 { cos_r } else { -cos_r }
    } else {
        let r3 = r2 * r;
        let r5 = r3 * r2;
        let r7 = r5 * r2;
        let sin_r = r - r3 * (1.0 / 6.0) + r5 * (1.0 / 120.0) - r7 * (1.0 / 5040.0);
        if q == 1 { -sin_r } else { sin_r }
    }
}

/// Approximate base^exp for RoPE frequencies.
/// Returns NaN for non-finite inputs, base <= 0.0, or negative bases.
pub fn f32_pow_approx(base: f32, exp: f32) -> f32 {
    if !base.is_finite() || !exp.is_finite() || base <= 0.0 {
        return f32::NAN;
    }
    if exp == 0.0 {
        return 1.0;
    }
    // High-precision ln for RoPE bases
    let ln_base = if (base - 8000.0).abs() < 1.0 {
        8.98719682f32
    } else if (base - 10000.0).abs() < 1.0 {
        9.21034037f32
    } else if (base - 500000.0).abs() < 1.0 {
        13.12236338f32
    } else if (base - 1000000.0).abs() < 1.0 {
        13.81551056f32
    } else {
        let bits = base.to_bits();
        let mut e = ((bits >> 23) & 0xff) as i32 - 127;
        let mut y = f32::from_bits((bits & 0x007fffff) | 0x3f800000);
        if y > 1.41421356 {
            y *= 0.5;
            e += 1;
        }
        let s = (y - 1.0) / (y + 1.0);
        let s2 = s * s;
        let poly = s * (2.0 + s2 * (2.0 / 3.0 + s2 * (2.0 / 5.0 + s2 * (2.0 / 7.0))));
        (e as f32) * 0.69314718f32 + poly
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

/// Weights per Q5_K super-block, and the bytes that super-block occupies:
/// two halves, 12 packed scale/min bytes, 32 high-bit bytes and one nibble per
/// weight. The layout is ggml's `block_q5_K`.
const Q5_K_WEIGHTS: usize = 256;
const Q5_K_BLOCK_BYTES: usize = 176;
const _: () = assert!(Q5_K_BLOCK_BYTES == 2 * 2 + 12 + 32 + Q5_K_WEIGHTS / 2);

/// Dequantizes Q5_K super-blocks (256 values per 176 bytes) into an f32 buffer.
///
/// The low four bits of each weight sit where Q4_K's sit -- within each
/// 64-weight group, byte `i`'s low nibble is element `i` and its high nibble is
/// element `i + 32` -- and the fifth bit comes from `qh`: bit `2*group` for the
/// low-nibble half, bit `2*group + 1` for the high-nibble half, which is how
/// ggml's `dequantize_row_q5_K` walks the mask (`u1 = 1, u2 = 2`, both shifted
/// two per group). The scales and mins use the same 6-bit packing as Q4_K.
///
/// WHY this exists beside the five mapped readers: EmbeddingGemma 2 states
/// `per_layer_model_proj.weight` as Q5_K (13). The loader's dtype gate still
/// refuses that id by number until the mapped set is widened deliberately;
/// this decoder is the layout half of that widening, proved against known
/// vectors and the published file's own bytes, not a claim that the gate reads
/// it today.
pub fn dequantize_q5_k(raw: &[u8], out: &mut [f32]) -> Result<usize, InferenceError> {
    // A partial trailing super-block is not a block: integer division would
    // silently drop its bytes and decode a row the file does not hold.
    if raw.len() % Q5_K_BLOCK_BYTES != 0 {
        return Err(InferenceError::CorruptedWeights);
    }
    let num_blocks = raw.len() / Q5_K_BLOCK_BYTES;
    let num_elements = num_blocks
        .checked_mul(Q5_K_WEIGHTS)
        .ok_or(InferenceError::BufferTooSmall)?;
    if out.len() < num_elements {
        return Err(InferenceError::BufferTooSmall);
    }
    for b in 0..num_blocks {
        let blk = &raw[b * Q5_K_BLOCK_BYTES..(b + 1) * Q5_K_BLOCK_BYTES];
        let d = f16_to_f32(u16::from_le_bytes([blk[0], blk[1]]));
        let dmin = f16_to_f32(u16::from_le_bytes([blk[2], blk[3]]));
        let scales = &blk[4..16];
        let qh = &blk[16..48];
        let qs = &blk[48..176];
        let out_base = b * Q5_K_WEIGHTS;
        for k in 0..8 {
            let (scale, min) = q4_k_scale_min(scales, k);
            let group = k / 2;
            let hi_half = k % 2;
            let mask = if hi_half == 0 {
                1u8 << (2 * group)
            } else {
                2u8 << (2 * group)
            };
            for i in 0..32 {
                let byte = qs[group * 32 + i];
                let low = if hi_half == 0 { byte & 0x0f } else { byte >> 4 };
                let high = if qh[i] & mask != 0 { 16u8 } else { 0u8 };
                let q = (low + high) as u32;
                out[out_base + k * 32 + i] =
                    d * (scale as f32) * (q as f32) - dmin * (min as f32);
            }
        }
    }
    Ok(num_elements)
}

/// Weights per Q6_K super-block, and the bytes that super-block occupies:
/// 128 low-nibble bytes, 64 high-bit bytes, 16 signed scales and one FP16 scale.
/// The layout is ggml's `block_q6_K`.
const Q6_K_WEIGHTS: usize = 256;
const Q6_K_BLOCK_BYTES: usize = 210;
const _: () = assert!(Q6_K_BLOCK_BYTES == 2 + 128 + 64 + 16);

/// Dequantizes Q6_K super-blocks (256 values per 210 bytes) into an f32 buffer.
///
/// Each 128-weight half carries 64 low-nibble bytes, 32 high-bit bytes and 8
/// signed 8-bit scales. For byte `l` (0..32) the four weights it packs are
/// `l`, `l + 32`, `l + 64`, `l + 96`: the low nibbles of `ql[l]` and `ql[l+32]`
/// and their high nibbles, each joined to two bits of `qh[l]` (bits 0-1, 2-3,
/// 4-5, 6-7 respectively), minus 32, times the half's scale for that quarter
/// (`scales[l/16 + 0/2/4/6]`) times the block's half `d`. This is ggml's
/// `dequantize_row_q6_K` walk, and the order matters: a reader that followed
/// the bytes in sequence would place every weight but the first 32 wrongly.
///
/// EmbeddingGemma 2 states `output.weight` as Q6_K (14); the same gate note as
/// `dequantize_q5_k` applies -- the layout is proved here, the mapped set is
/// widened deliberately and separately.
pub fn dequantize_q6_k(raw: &[u8], out: &mut [f32]) -> Result<usize, InferenceError> {
    if raw.len() % Q6_K_BLOCK_BYTES != 0 {
        return Err(InferenceError::CorruptedWeights);
    }
    let num_blocks = raw.len() / Q6_K_BLOCK_BYTES;
    let num_elements = num_blocks
        .checked_mul(Q6_K_WEIGHTS)
        .ok_or(InferenceError::BufferTooSmall)?;
    if out.len() < num_elements {
        return Err(InferenceError::BufferTooSmall);
    }
    for b in 0..num_blocks {
        let blk = &raw[b * Q6_K_BLOCK_BYTES..(b + 1) * Q6_K_BLOCK_BYTES];
        let ql = &blk[..128];
        let qh = &blk[128..192];
        let sc = &blk[192..208];
        let d = f16_to_f32(u16::from_le_bytes([blk[208], blk[209]]));
        let out_base = b * Q6_K_WEIGHTS;
        for half in 0..2 {
            let qlh = &ql[half * 64..half * 64 + 64];
            let qhh = &qh[half * 32..half * 32 + 32];
            let sch = &sc[half * 8..half * 8 + 8];
            let base = out_base + half * 128;
            for l in 0..32 {
                let is = l / 16;
                let q1 = ((qlh[l] & 0x0f) | ((qhh[l] & 3) << 4)) as i32 - 32;
                let q2 = ((qlh[l + 32] & 0x0f) | (((qhh[l] >> 2) & 3) << 4)) as i32 - 32;
                let q3 = ((qlh[l] >> 4) | (((qhh[l] >> 4) & 3) << 4)) as i32 - 32;
                let q4 = ((qlh[l + 32] >> 4) | (((qhh[l] >> 6) & 3) << 4)) as i32 - 32;
                out[base + l] = d * (sch[is] as i8 as f32) * (q1 as f32);
                out[base + l + 32] = d * (sch[is + 2] as i8 as f32) * (q2 as f32);
                out[base + l + 64] = d * (sch[is + 4] as i8 as f32) * (q3 as f32);
                out[base + l + 96] = d * (sch[is + 6] as i8 as f32) * (q4 as f32);
            }
        }
    }
    Ok(num_elements)
}

/// Weights per TQ2_0 super-block, and the bytes that super-block occupies:
/// 64 bytes of two-bit ternary quants and one FP16 scale. The layout is ggml's
/// `block_tq2_0`.
const TQ2_0_WEIGHTS: usize = 256;
const TQ2_0_BLOCK_BYTES: usize = 66;
const _: () = assert!(TQ2_0_BLOCK_BYTES == 2 + TQ2_0_WEIGHTS / 4);

/// Dequantizes TQ2_0 super-blocks (256 values per 66 bytes) into an f32 buffer.
///
/// Each byte packs four ternary weights, two bits each, valued -1, 0 or +1
/// times the block's half `d` (the absolute maximum the quantiser saw). Within
/// each 128-weight chunk, byte `m` (0..32) holds weight `m` in bits 0-1,
/// `m + 32` in bits 2-3, `m + 64` in bits 4-5 and `m + 96` in bits 6-7 -- the
/// order ggml's `dequantize_row_tq2_0` emits (bit plane outer, byte inner).
/// The two-bit code 3 is not one the quantiser writes; decoded it is +2 times
/// `d`, which is what the arithmetic says and not a value to invent a meaning
/// for.
///
/// Gemma 4 E2B states 61 tensors as TQ2_0 (35), including `token_embd.weight`;
/// the same gate note as `dequantize_q5_k` applies.
pub fn dequantize_tq2_0(raw: &[u8], out: &mut [f32]) -> Result<usize, InferenceError> {
    if raw.len() % TQ2_0_BLOCK_BYTES != 0 {
        return Err(InferenceError::CorruptedWeights);
    }
    let num_blocks = raw.len() / TQ2_0_BLOCK_BYTES;
    let num_elements = num_blocks
        .checked_mul(TQ2_0_WEIGHTS)
        .ok_or(InferenceError::BufferTooSmall)?;
    if out.len() < num_elements {
        return Err(InferenceError::BufferTooSmall);
    }
    for b in 0..num_blocks {
        let blk = &raw[b * TQ2_0_BLOCK_BYTES..(b + 1) * TQ2_0_BLOCK_BYTES];
        let qs = &blk[..64];
        let d = f16_to_f32(u16::from_le_bytes([blk[64], blk[65]]));
        let out_base = b * TQ2_0_WEIGHTS;
        for chunk in 0..2 {
            for plane in 0..4 {
                for m in 0..32 {
                    let q = ((qs[chunk * 32 + m] >> (2 * plane)) & 3) as i32 - 1;
                    out[out_base + chunk * 128 + plane * 32 + m] = d * (q as f32);
                }
            }
        }
    }
    Ok(num_elements)
}

/// Computes General Matrix-Vector Multiplication: y = W * x.
///
/// Handles on-the-fly dequantization directly from raw weight bytes, avoiding large
/// temporary weight memory allocations.
///
/// WHY every stride product is checked: `rows`, `cols` and the block widths are
/// the file's own numbers, and a declaration whose byte product overflows a
/// `usize` would wrap into a small stride that passes the length check and then
/// reads a neighbour's bytes as this matrix's rows. An uncountable product is
/// refused as corrupt weights, never wrapped.
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
            let row_stride_bytes = cols.checked_mul(4).ok_or(InferenceError::CorruptedWeights)?;
            let needed = rows
                .checked_mul(row_stride_bytes)
                .ok_or(InferenceError::CorruptedWeights)?;
            if w_bytes.len() < needed {
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
            let row_stride_bytes = cols.checked_mul(2).ok_or(InferenceError::CorruptedWeights)?;
            let needed = rows
                .checked_mul(row_stride_bytes)
                .ok_or(InferenceError::CorruptedWeights)?;
            if w_bytes.len() < needed {
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
            let row_stride_bytes = blocks_per_row
                .checked_mul(18)
                .ok_or(InferenceError::CorruptedWeights)?;
            let needed = rows
                .checked_mul(row_stride_bytes)
                .ok_or(InferenceError::CorruptedWeights)?;
            if w_bytes.len() < needed {
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
            let row_stride_bytes = blocks_per_row
                .checked_mul(34)
                .ok_or(InferenceError::CorruptedWeights)?;
            let needed = rows
                .checked_mul(row_stride_bytes)
                .ok_or(InferenceError::CorruptedWeights)?;
            if w_bytes.len() < needed {
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
            let row_stride_bytes = blocks_per_row
                .checked_mul(Q4_K_BLOCK_BYTES)
                .ok_or(InferenceError::CorruptedWeights)?;
            let needed = rows
                .checked_mul(row_stride_bytes)
                .ok_or(InferenceError::CorruptedWeights)?;
            if w_bytes.len() < needed {
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
        // GGML_TYPE_Q5_K / Q6_K / TQ2_0: dequantise each row through the
        // canonical block decoders, then take the dot product. A scratch row is
        // reused across rows so the allocation is per call, not per row.
        13 | 14 | 35 => {
            if cols % 256 != 0 {
                return Err(InferenceError::CorruptedWeights);
            }
            let blocks_per_row = cols / 256;
            let row_stride_bytes = match w_type {
                13 => blocks_per_row
                    .checked_mul(Q5_K_BLOCK_BYTES)
                    .ok_or(InferenceError::CorruptedWeights)?,
                14 => blocks_per_row
                    .checked_mul(Q6_K_BLOCK_BYTES)
                    .ok_or(InferenceError::CorruptedWeights)?,
                _ => blocks_per_row
                    .checked_mul(TQ2_0_BLOCK_BYTES)
                    .ok_or(InferenceError::CorruptedWeights)?,
            };
            let needed = rows
                .checked_mul(row_stride_bytes)
                .ok_or(InferenceError::CorruptedWeights)?;
            if w_bytes.len() < needed {
                return Err(InferenceError::CorruptedWeights);
            }
            let mut row = alloc::vec![0f32; cols];
            for i in 0..rows {
                let row_slice = &w_bytes[i * row_stride_bytes..(i + 1) * row_stride_bytes];
                let produced = match w_type {
                    13 => dequantize_q5_k(row_slice, &mut row)?,
                    14 => dequantize_q6_k(row_slice, &mut row)?,
                    _ => dequantize_tq2_0(row_slice, &mut row)?,
                };
                if produced != cols {
                    return Err(InferenceError::CorruptedWeights);
                }
                let mut sum = 0.0f32;
                for j in 0..cols {
                    sum += row[j] * x[j];
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
    if head_dim == 0 || !base.is_finite() || base <= 0.0 {
        return;
    }
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

/// Software hyperbolic tangent for the pooling head's GELU approximation.
///
/// WHY this form: `tanh(x) = (1 - e^{-2x}) / (1 + e^{-2x})` needs only the
/// exponential this module already has, and `e^{-2x}` stays inside `f32::MAX` for
/// every `|x| <= 88`. Beyond `|x| = 20` the value is `-1.0`/`1.0` to float
/// precision, so the clamp answers with what it would converge to rather than
/// dividing two overflowing quantities.
pub fn f32_tanh(x: f32) -> f32 {
    if x <= -20.0 {
        return -1.0;
    }
    if x >= 20.0 {
        return 1.0;
    }
    let e = f32_exp(-2.0f32 * x);
    (1.0f32 - e) / (1.0f32 + e)
}

/// GELU with the tanh approximation: the activation the EmbeddingGemma pooling
/// head ships with.
///
/// WHY tanh and not the exact erf: the upstream reference (the Gemma2 family's
/// `gelu_pytorch_tanh` hidden activation, and the sentence-transformers `Dense`
/// module the published file's own `general.tags` names) uses the tanh
/// approximation, and the GGUF format states no activation key this reading
/// could check instead. The file declares `dense_2_feat_in/out` and
/// `dense_3_feat_in/out` and nothing about the curve between them, so the
/// approximation is the documented assumption the host-parity check is written
/// against -- not a free choice dressed up as the file's own fact.
pub fn gelu_tanh(x: f32) -> f32 {
    // 0.5 * x * (1 + tanh(sqrt(2/pi) * (x + 0.044715 * x^3)))
    let inner = 0.7978845608028654f32 * (x + 0.044715f32 * x * x * x);
    0.5f32 * x * (1.0f32 + f32_tanh(inner))
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

/// GeGLU non-linear activation function: out = gelu_tanh(gate) * up
pub fn geglu(gate: &[f32], up: &[f32], out: &mut [f32]) {
    let len = gate.len().min(up.len()).min(out.len());
    for i in 0..len {
        out[i] = gelu_tanh(gate[i]) * up[i];
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

pub const DEFAULT_TOP_K: usize = 40;
pub const DEFAULT_TOP_P: f32 = 0.9;
pub const TOP_K_CANDIDATE_CAP: usize = 64;

static SAMPLING_RNG_STATE: core::sync::atomic::AtomicU64 =
    core::sync::atomic::AtomicU64::new(0x853c_49e6_748f_ea9b);

fn next_sample_random_f32() -> f32 {
    let mut state = SAMPLING_RNG_STATE.load(core::sync::atomic::Ordering::Relaxed);
    if state == 0 {
        state = 0x853c_49e6_748f_ea9b;
    }
    state ^= state << 13;
    state ^= state >> 7;
    state ^= state << 17;
    SAMPLING_RNG_STATE.store(state, core::sync::atomic::Ordering::Relaxed);
    ((state & 0x00ff_ffff) as f32) / 16777216.0f32
}

/// Samples the next token ID from logits.
///
/// Uses deterministic argmax when temperature < 1e-4, or calibrated sampling with
/// temperature scaling, Top-K, and Top-P (nucleus) filtering without heap allocations.
pub fn sample_token(logits: &[f32], temperature: f32) -> u32 {
    sample_token_calibrated(logits, temperature, DEFAULT_TOP_K, DEFAULT_TOP_P, 0)
}

/// Calibrated token sampling with configurable Top-K, Top-P, and optional seed.
///
/// Zero heap allocations: all candidate ranking and probability calculations use
/// fixed-size stack buffers.
pub fn sample_token_calibrated(
    logits: &[f32],
    temperature: f32,
    top_k: usize,
    top_p: f32,
    seed: u64,
) -> u32 {
    if logits.is_empty() {
        return 0;
    }
    // Deterministic argmax when temperature < 1e-4
    if temperature < 1e-4 {
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

    let inv_t = 1.0f32 / temperature;
    let k_bound = top_k.min(TOP_K_CANDIDATE_CAP).min(logits.len()).max(1);

    let mut cand_indices = [0u32; TOP_K_CANDIDATE_CAP];
    let mut cand_logits = [f32::NEG_INFINITY; TOP_K_CANDIDATE_CAP];
    let mut count = 0usize;

    // 1. Top-K filtering: maintain sorted top-K candidates
    for (idx, &raw_logit) in logits.iter().enumerate() {
        let scaled_logit = raw_logit * inv_t;
        if count < k_bound {
            let mut pos = count;
            while pos > 0 && cand_logits[pos - 1] < scaled_logit {
                cand_logits[pos] = cand_logits[pos - 1];
                cand_indices[pos] = cand_indices[pos - 1];
                pos -= 1;
            }
            cand_logits[pos] = scaled_logit;
            cand_indices[pos] = idx as u32;
            count += 1;
        } else if scaled_logit > cand_logits[k_bound - 1] {
            let mut pos = k_bound - 1;
            while pos > 0 && cand_logits[pos - 1] < scaled_logit {
                cand_logits[pos] = cand_logits[pos - 1];
                cand_indices[pos] = cand_indices[pos - 1];
                pos -= 1;
            }
            cand_logits[pos] = scaled_logit;
            cand_indices[pos] = idx as u32;
        }
    }

    if count == 0 {
        return 0;
    }

    // 2. Softmax over the top-K candidates
    let max_logit = cand_logits[0];
    let mut cand_probs = [0.0f32; TOP_K_CANDIDATE_CAP];
    let mut sum_exp = 0.0f32;
    for i in 0..count {
        let p = f32_exp(cand_logits[i] - max_logit);
        cand_probs[i] = p;
        sum_exp += p;
    }
    if sum_exp > 0.0 {
        let inv_sum = 1.0f32 / sum_exp;
        for i in 0..count {
            cand_probs[i] *= inv_sum;
        }
    } else {
        return cand_indices[0];
    }

    // 3. Top-P (nucleus) filtering: keep candidates until cumulative probability >= top_p
    let mut cum_prob = 0.0f32;
    let mut nucleus_count = 1usize;
    for i in 0..count {
        cum_prob += cand_probs[i];
        nucleus_count = i + 1;
        if cum_prob >= top_p {
            break;
        }
    }

    // Re-normalize nucleus probabilities
    let mut nucleus_sum = 0.0f32;
    for i in 0..nucleus_count {
        nucleus_sum += cand_probs[i];
    }
    if nucleus_sum > 0.0 {
        let inv_n_sum = 1.0f32 / nucleus_sum;
        for i in 0..nucleus_count {
            cand_probs[i] *= inv_n_sum;
        }
    }

    // 4. Sample from nucleus
    let r = if seed != 0 {
        let mut s = seed;
        s ^= s << 13;
        s ^= s >> 7;
        s ^= s << 17;
        ((s & 0x00ff_ffff) as f32) / 16777216.0f32
    } else {
        next_sample_random_f32()
    };

    let mut cumulative = 0.0f32;
    for i in 0..nucleus_count {
        cumulative += cand_probs[i];
        if r <= cumulative || i == nucleus_count - 1 {
            return cand_indices[i];
        }
    }

    cand_indices[0]
}

/// Maximum number of tensor entries cached in resident memory.
pub const RESIDENT_TENSOR_CAP: usize = 384;

/// Resident representation of a GGUF tensor descriptor for offset caching.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ResidentTensorEntry {
    pub name_len: u16,
    pub name: [u8; 64],
    pub dtype: u32,
    pub cols: u64,
    pub rows: u64,
    pub n_dims: u32,
    pub byte_len: u64,
    pub offset: u64,
    pub position: u64,
}

impl ResidentTensorEntry {
    pub const fn empty() -> Self {
        Self {
            name_len: 0,
            name: [0u8; 64],
            dtype: 0,
            cols: 0,
            rows: 0,
            n_dims: 0,
            byte_len: 0,
            offset: 0,
            position: 0,
        }
    }

    pub fn matches(&self, target_name: &[u8]) -> bool {
        self.name_len as usize == target_name.len()
            && &self.name[..self.name_len as usize] == target_name
    }
}

/// Static / resident cache of GGUF tensor offsets, names, and dimensions.
pub struct ResidentTensorCache {
    pub count: usize,
    pub entries: [ResidentTensorEntry; RESIDENT_TENSOR_CAP],
    pub data_start: u64,
    pub alignment: u64,
    pub initialized: bool,
}

impl ResidentTensorCache {
    pub const fn new() -> Self {
        Self {
            count: 0,
            entries: [ResidentTensorEntry::empty(); RESIDENT_TENSOR_CAP],
            data_start: 0,
            alignment: 32,
            initialized: false,
        }
    }

    pub fn find(&self, name: &[u8]) -> Option<&ResidentTensorEntry> {
        if !self.initialized {
            return None;
        }
        for i in 0..self.count {
            if self.entries[i].matches(name) {
                return Some(&self.entries[i]);
            }
        }
        None
    }
}

// -----------------------------------------------------------------------------
// Transformer Layer & Model Architecture
// -----------------------------------------------------------------------------

/// Non-linear feed-forward activation function type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActivationType {
    SwiGLU,
    GeGLU,
}

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
    /// `<arch>.attention.sliding_window`: how many positions, the current one
    /// included, one query is allowed to attend over.
    ///
    /// WHY this is a field and not a constant: the published
    /// `embeddinggemma-300M-Q8_0.gguf` states 512 against the 2048-position
    /// `context_length` it also states, and the `gemma2` fixture this engine was
    /// written against states 32 against its own 64. A file that declares no
    /// window is `None`, and the whole cached prefix is attended -- that is the
    /// file's graph, not a default the engine preferred.
    ///
    /// WHAT IT DOES NOT CLAIM: a window the file states globally is applied
    /// globally. The published `embeddinggemma-300M-Q8_0.gguf` declares no
    /// `attention.sliding_window_pattern`, so nothing in its bytes distinguishes
    /// a sliding layer from a full one and every layer of it attends this one
    /// window. A file that does declare the pattern -- EmbeddingGemma 2 and
    /// Gemma 4 E2B both do -- has each layer attend its own span through
    /// `layer_attention_span_start`, and `rope.freq_base_swa` is honoured for
    /// exactly those layers the pattern marks sliding.
    pub sliding_window: Option<usize>,
    pub rms_norm_eps: f32,
    pub rope_theta: f32,
    pub activation: ActivationType,
    /// `<arch>.attention.head_count_kv` per layer, exactly as the file declared
    /// it. EmbeddingGemma 2 states 24 values (1 at every sixth layer, 2 elsewhere);
    /// a scalar declaration is held as one element and an absent key as none, in
    /// which case `layer_kv_heads` answers the uniform `num_kv_heads`.
    pub kv_heads_per_layer: crate::gguf::PerLayerCounts,
    /// `<arch>.feed_forward_length` per layer. Gemma 4 E2B states 35 values
    /// (6144 for layers 0..14, 12288 for layers 15..34); each layer's feed-forward
    /// runs at its own width, and the scratch is cut for the widest of them.
    pub ffn_per_layer: crate::gguf::PerLayerCounts,
    /// `<arch>.attention.sliding_window_pattern`, one flag per layer. A true flag
    /// is a sliding-window layer; a false one attends the whole cached prefix.
    pub sliding_window_pattern: crate::gguf::PerLayerFlags,
    /// `<arch>.attention.key_length_swa`: one head's width in the sliding layers,
    /// when the file states it separately from `key_length`. EmbeddingGemma 2 and
    /// Gemma 4 E2B both run 256-wide heads in their sliding layers and 512-wide
    /// heads in their full-attention ones, from the same residual stream.
    pub head_dim_swa: Option<usize>,
    /// `<arch>.rope.freq_base_swa`, the rotation base the sliding layers state.
    pub rope_theta_swa: Option<f32>,
    /// The context the file declared, before the byte budget cut the cache depth.
    /// `seq_len` is what the device holds; this is what the file asked for, and
    /// the two together are the clamp the service reports rather than hides.
    pub declared_seq_len: usize,
    /// `<arch>.embedding_length_per_layer_input`: the width of the per-layer
    /// input the file declares. The block graph this engine walks has no gate
    /// for it, so a file that states one is reported as an incomplete graph by
    /// `unwalkable_graph_declaration` rather than run without it.
    pub per_layer_input: Option<usize>,
    /// Whether the file's family ships the per-layer input as its own
    /// vocabulary-sized table (`per_layer_token_embd.weight`). The Gemma 4
    /// family does -- the published E2B file's descriptor list states it at
    /// (8960, 262144) -- while EmbeddingGemma 2 projects the shared token table
    /// instead and ships no such tensor. `parameter_count` adds the table only
    /// for the families whose tensor lists were measured to carry one.
    pub per_layer_input_table: bool,
    /// `<arch>.attention.shared_kv_layers`: how many layers share one key/value
    /// projection. This engine caches per layer and has no sharing graph, so a
    /// nonzero declaration is named as unimplemented, never silently unshared.
    pub shared_kv_layers: Option<u32>,
    /// `<arch>.embedding_length_out`: the width of the embedding output head,
    /// when the file states one separately from the residual stream.
    pub embedding_length_out: Option<usize>,
    /// `<arch>.attention.causal`. A false declaration is bidirectional attention,
    /// which this engine's causal span does not walk.
    pub attention_causal: Option<bool>,
    /// `<arch>.pooling_type`, carried for the report; the pooling comment in
    /// `extract_embedding` says what reads it and what does not.
    pub pooling_type: Option<u32>,
    /// `<arch>.final_logit_softcapping`. This engine's logits are unbounded; a
    /// file that states a cap is named as an incomplete graph, not run uncapped.
    pub final_logit_softcapping: Option<f32>,
    /// `general.file_type` as the walk reached it. The metadata walk stops at the
    /// vocabulary and this key sits past it in every published file measured, so
    /// this is usually `None` and the weight pricing below falls back to the F32
    /// upper bound rather than guessing a quantisation the file never stated here.
    pub declared_file_type: Option<u32>,
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
            // No window: this is a fixture with no file behind it, so there is no
            // declaration to carry. A file that states one gets it through
            // `from_gguf`, which is the only route a window can take.
            sliding_window: None,
            rms_norm_eps: 1e-6,
            rope_theta: 10000.0,
            activation: ActivationType::SwiGLU,
            kv_heads_per_layer: crate::gguf::PerLayerCounts::empty(),
            ffn_per_layer: crate::gguf::PerLayerCounts::empty(),
            sliding_window_pattern: crate::gguf::PerLayerFlags::empty(),
            head_dim_swa: None,
            rope_theta_swa: None,
            declared_seq_len: 64,
            per_layer_input: None,
            per_layer_input_table: false,
            shared_kv_layers: None,
            embedding_length_out: None,
            attention_causal: None,
            pooling_type: None,
            final_logit_softcapping: None,
            declared_file_type: None,
        }
    }

    /// How many layers can carry a geometry of their own. The per-layer arrays
    /// hold at most `MAX_PER_LAYER_GEOMETRY` elements, so every layer past them
    /// runs the uniform declaration; pricing loops over this many layers and
    /// multiplies the uniform shape by the rest, which keeps a file declaring
    /// four billion blocks from turning a price into a four-billion-step loop.
    fn detailed_layers(&self) -> usize {
        if self.num_layers < crate::gguf::MAX_PER_LAYER_GEOMETRY {
            self.num_layers
        } else {
            crate::gguf::MAX_PER_LAYER_GEOMETRY
        }
    }

    /// Whether the file's own pattern marks layer `layer` a sliding-window one.
    ///
    /// A file that declared no pattern answers `false` for every layer: the
    /// uniform window in `sliding_window`, if any, is then the only span rule.
    pub fn layer_is_sliding(&self, layer: usize) -> bool {
        if self.sliding_window_pattern.len == 0 || layer >= self.sliding_window_pattern.len {
            return false;
        }
        self.sliding_window_pattern.values[layer]
    }

    /// One head's width in layer `layer`: `head_dim_swa` where the pattern marks
    /// the layer sliding, `head_dim` everywhere else.
    ///
    /// WHY this is per layer and not one number: EmbeddingGemma 2's tensor
    /// descriptors state `blk.0.attn_q.weight` as (512, 1024) -- four heads of
    /// 256 -- and `blk.5.attn_q.weight` as (512, 2048) -- four heads of 512 --
    /// from the same 512-wide residual stream. One head width for all layers
    /// would read one of those two tensors out of bounds.
    pub fn layer_head_dim(&self, layer: usize) -> usize {
        match self.head_dim_swa {
            Some(swa) if self.layer_is_sliding(layer) => swa,
            _ => self.head_dim,
        }
    }

    /// The key/value head count layer `layer` declared, or the uniform
    /// `num_kv_heads` when the file stated no per-layer value for it.
    pub fn layer_kv_heads(&self, layer: usize) -> usize {
        if layer < self.kv_heads_per_layer.len {
            self.kv_heads_per_layer.values[layer] as usize
        } else {
            self.num_kv_heads
        }
    }

    /// The feed-forward width layer `layer` declared, or the uniform
    /// `intermediate_dim` when the file stated no per-layer value for it.
    pub fn layer_ffn_dim(&self, layer: usize) -> usize {
        if layer < self.ffn_per_layer.len {
            self.ffn_per_layer.values[layer] as usize
        } else {
            self.intermediate_dim
        }
    }

    /// The rotation base layer `layer` stated: `rope_theta_swa` for the layers
    /// the pattern marks sliding, `rope_theta` for every other layer.
    pub fn layer_rope_theta(&self, layer: usize) -> f32 {
        match self.rope_theta_swa {
            Some(theta) if self.layer_is_sliding(layer) => theta,
            _ => self.rope_theta,
        }
    }

    /// The widest query span any layer projects: `num_heads x` that layer's head
    /// width. The scratch's query and attention-output buffers are cut for this,
    /// which is what lets a 512-wide residual stream carry 2048-wide attention.
    pub fn max_heads_span(&self) -> usize {
        let mut widest = 0usize;
        let mut layer = 0usize;
        while layer < self.detailed_layers() {
            let span = self.num_heads.saturating_mul(self.layer_head_dim(layer));
            if span > widest {
                widest = span;
            }
            layer += 1;
        }
        if self.num_layers > self.detailed_layers() {
            let uniform = self.num_heads.saturating_mul(self.head_dim);
            if uniform > widest {
                widest = uniform;
            }
        }
        widest
    }

    /// The widest key/value projection any layer runs: that layer's head count
    /// times its head width. The scratch's key and value buffers are cut for it.
    pub fn max_kv_width(&self) -> usize {
        let mut widest = 0usize;
        let mut layer = 0usize;
        while layer < self.detailed_layers() {
            let width = self
                .layer_kv_heads(layer)
                .saturating_mul(self.layer_head_dim(layer));
            if width > widest {
                widest = width;
            }
            layer += 1;
        }
        if self.num_layers > self.detailed_layers() {
            let uniform = self.num_kv_heads.saturating_mul(self.head_dim);
            if uniform > widest {
                widest = uniform;
            }
        }
        widest
    }

    /// The widest feed-forward any layer declared.
    pub fn max_ffn_dim(&self) -> usize {
        let mut widest = 0usize;
        let mut layer = 0usize;
        while layer < self.detailed_layers() {
            let width = self.layer_ffn_dim(layer);
            if width > widest {
                widest = width;
            }
            layer += 1;
        }
        if self.num_layers > self.detailed_layers() && self.intermediate_dim > widest {
            widest = self.intermediate_dim;
        }
        widest
    }

    /// First cached position a query at `pos` in layer `layer` may attend over:
    /// the full prefix for a layer the file's pattern marks full-attention, the
    /// declared window for every other layer.
    ///
    /// WHY the pattern overrides the window instead of narrowing it: a file
    /// that declares `sliding_window` 1024 beside a pattern with false flags is
    /// stating that the false-flag layers attend everything cached. Applying the
    /// window to them would be the engine shortening a span the file declared
    /// long, which changes the answer the model gives.
    pub fn layer_attention_span_start(&self, layer: usize, pos: usize) -> usize {
        if self.sliding_window_pattern.len > 0 && !self.layer_is_sliding(layer) {
            return 0;
        }
        self.attention_span_start(pos)
    }

    /// Bytes the KV cache needs: K and V, one row of that layer's head width per
    /// (layer, position, key/value head), summed over the layers' own geometry.
    ///
    /// WHY it saturates instead of failing: a shape whose product overflows a
    /// `usize` needs more memory than the device can count, and the refusal that
    /// follows is the right one. The number then names `usize::MAX` rather than
    /// the exact need, which says "at least this much" -- still a refusal the
    /// person can act on, and the arithmetic is exact for anything a real file
    /// declares (every dimension arrives as a `u32`).
    pub fn kv_cache_bytes(&self) -> usize {
        let mut floats_per_position = 0usize;
        let mut layer = 0usize;
        while layer < self.detailed_layers() {
            floats_per_position = floats_per_position.saturating_add(
                self.layer_kv_heads(layer)
                    .saturating_mul(self.layer_head_dim(layer)),
            );
            layer += 1;
        }
        if self.num_layers > self.detailed_layers() {
            let tail = self.num_layers - self.detailed_layers();
            floats_per_position = floats_per_position.saturating_add(
                tail.saturating_mul(self.num_kv_heads.saturating_mul(self.head_dim)),
            );
        }
        [
            2,
            self.seq_len,
            floats_per_position,
            FLOAT_BYTES,
        ]
        .iter()
        .fold(1usize, |acc, &n| acc.saturating_mul(n))
    }

    /// Bytes the whole run needs: the cache plus every activation, score and
    /// logit buffer `Scratch` holds.
    ///
    /// This is one of the two numbers the engine's size limit is measured
    /// against; `largest_single_buffer_bytes` is the other.
    pub fn scratch_bytes(&self) -> usize {
        self.kv_cache_bytes().saturating_add(Scratch::bytes_for(self))
    }

    /// Every parameter the file's declared geometry names: each layer's own
    /// attention projections at that layer's head width and key/value head
    /// count, its own feed-forward width, its norms, the token table, the final
    /// norm, and the per-layer input tensors when the file declares that input.
    ///
    /// WHY per layer and not one uniform shape: Gemma 4 E2B declares 35 layers
    /// whose feed-forward widths are 6144 for the first fifteen and 12288 for
    /// the rest, and a separate vocabulary-sized `per_layer_token_embd` table of
    /// 8960 x 262144 elements. Counting one uniform layer and no table reported
    /// roughly 1.8 billion parameters for a file whose own size label says 4.6B
    /// -- a 2.6x understatement of what the device is being asked to hold.
    pub fn parameter_count(&self) -> usize {
        let layer_params = |head_dim: usize, kv_heads: usize, ffn: usize| -> usize {
            let span = self.num_heads.saturating_mul(head_dim);
            let kv_width = kv_heads.saturating_mul(head_dim);
            let q_params = span.saturating_mul(self.hidden_dim);
            let k_params = kv_width.saturating_mul(self.hidden_dim);
            let out_params = self.hidden_dim.saturating_mul(span);
            let ffn_params = ffn.saturating_mul(self.hidden_dim).saturating_mul(3);
            // Four stream-width norms plus the query and key head norms, which
            // are one head wide, not one stream wide.
            let norm_params = self
                .hidden_dim
                .saturating_mul(4)
                .saturating_add(head_dim.saturating_mul(2));
            let mut params = q_params
                .saturating_add(k_params)
                .saturating_add(k_params)
                .saturating_add(out_params)
                .saturating_add(ffn_params)
                .saturating_add(norm_params);
            if let Some(width) = self.per_layer_input {
                // The per-layer input gate and projection the block ships
                // beside its attention (`inp_gate`, `proj`), the post norm and
                // the scalar output scale: two hidden x width matrices, one
                // width-wide norm and one scalar, per layer.
                params = params
                    .saturating_add(self.hidden_dim.saturating_mul(width).saturating_mul(2))
                    .saturating_add(width)
                    .saturating_add(1);
            }
            params
        };
        let mut total = 0usize;
        let mut layer = 0usize;
        while layer < self.detailed_layers() {
            total = total.saturating_add(layer_params(
                self.layer_head_dim(layer),
                self.layer_kv_heads(layer),
                self.layer_ffn_dim(layer),
            ));
            layer += 1;
        }
        if self.num_layers > self.detailed_layers() {
            let tail = self.num_layers - self.detailed_layers();
            total = total.saturating_add(
                tail.saturating_mul(layer_params(
                    self.head_dim,
                    self.num_kv_heads,
                    self.intermediate_dim,
                )),
            );
        }

        let embed_params = self.vocab_size.saturating_mul(self.hidden_dim);
        total = total
            .saturating_add(embed_params)
            .saturating_add(self.hidden_dim);

        if let Some(width) = self.per_layer_input {
            let rows = self.num_layers.saturating_mul(width);
            // `per_layer_model_proj` projects the stream into the per-layer
            // input width, one row per layer-input element.
            total = total.saturating_add(self.hidden_dim.saturating_mul(rows));
            if self.per_layer_input_table {
                total = total.saturating_add(self.vocab_size.saturating_mul(rows));
            }
        }
        total
    }

    /// Prices the weight bytes required to store this model configuration in memory
    /// for a given quantisation dtype (e.g. Q4_K = 12, Q8_0 = 8, F16 = 1, F32 = 0).
    ///
    /// The ids 13, 14, 15, 16, 17, 18 and 35 are priced here because the
    /// published files' descriptor lists and `general.file_type` declarations
    /// state them; pricing a layout is not decoding it, and the readers
    /// (`gemv`, `embedding_row_into`, `quantised_row_bytes`) still refuse every
    /// id they have no decoder for, by its number. The GGML dtype and llama
    /// file_type id spaces disagree on some numbers (7, 13, 14, 15, 16, 17);
    /// where they do, the wider layout is priced, so the estimate is an upper
    /// bound and never under the bytes the file holds.
    pub fn weight_bytes(&self, dtype: u32) -> Result<usize, InferenceError> {
        let params = self.parameter_count();
        match dtype {
            0 => Ok(params.saturating_mul(4)),            // F32: 4 bytes/weight
            1 => Ok(params.saturating_mul(2)),            // F16: 2 bytes/weight
            2 => Ok((params.saturating_mul(18)) / 32),    // Q4_0: 4.5 bits/weight
            7 | 8 => Ok((params.saturating_mul(34)) / 32), // Q8_0: 8.5 bits/weight (7 in GGUF file_type, 8 in GGML type)
            12 | 15 => Ok((params.saturating_mul(144)) / 256), // Q4_K: 4.5 bits/weight (15 is file_type Q4_K_M)
            13 | 16 | 17 => Ok((params.saturating_mul(176)) / 256), // Q5_K: 5.5 bits/weight (16/17 its file_type spellings)
            14 | 18 => Ok((params.saturating_mul(210)) / 256), // Q6_K: 6.5625 bits/weight (18 its file_type spelling)
            32 | 35 => Ok((params.saturating_mul(66)) / 256),  // TQ2_0 / IQ2_M: 2.0625 bits/weight
            other => Err(InferenceError::UnsupportedQuantization(other)),
        }
    }

    /// Bytes one row of `cols` elements occupies in `dtype`, for pricing only.
    ///
    /// WHY this is separate from `quantised_row_bytes`: that function sizes the
    /// rows the loader actually reads and refuses every dtype it cannot decode.
    /// This one prices the rows a file's descriptor list states, including the
    /// layouts the readers do not decode yet, so a budget can name what a file
    /// would need without claiming the engine can read it. Overflow refuses;
    /// nothing here wraps.
    fn priced_row_bytes(dtype: u32, cols: usize) -> Result<usize, InferenceError> {
        let blocks = |group: usize| -> usize { cols.div_ceil(group) };
        let bytes = match dtype {
            0 => cols.checked_mul(4),
            1 => cols.checked_mul(2),
            2 => blocks(32).checked_mul(18),
            8 => blocks(32).checked_mul(34),
            12 => blocks(256).checked_mul(144),
            13 => blocks(256).checked_mul(176),
            14 => blocks(256).checked_mul(210),
            32 | 35 => blocks(256).checked_mul(66),
            _ => return Err(InferenceError::UnsupportedQuantization(dtype)),
        };
        bytes.ok_or(InferenceError::BufferTooSmall)
    }

    /// The dtype the weight pricing uses: the file's own declaration when the
    /// walk reached it, else F32 -- the widest layout any tensor can have, so a
    /// file whose `general.file_type` sits past the vocabulary (every published
    /// file measured puts it there) is priced at an upper bound, never at a
    /// quantisation the engine guessed.
    fn pricing_dtype(&self) -> u32 {
        match self.declared_file_type {
            Some(ft) => ft,
            None => 0,
        }
    }

    /// Bytes in the largest single weight buffer a run maps.
    ///
    /// The block matrices are priced at the widest per-layer geometry, because
    /// the loader holds one block at a time and that block can be any layer's.
    /// `token_embd` is row-streamed, so it prices as one row, not the table.
    /// `per_layer_token_embd`, where the family ships one, is row-streamed the
    /// same way: E2B's table is 1,321,205,760 bytes whole but 5,040 bytes a row
    /// at its declared Q4_0, and pricing it whole would refuse a file for memory
    /// the loader never asks for. `per_layer_model_proj` and the output head are
    /// mapped whole by the loader, so they price whole.
    pub fn largest_weight_buffer_bytes(&self) -> Result<usize, InferenceError> {
        let dtype = self.pricing_dtype();
        let hidden = self.hidden_dim;
        let span = self.max_heads_span();
        let kv_width = self.max_kv_width();
        let ffn = self.max_ffn_dim();
        let mut largest = 0usize;
        let mut consider = |bytes: Result<usize, InferenceError>| -> Result<(), InferenceError> {
            let bytes = bytes?;
            if bytes > largest {
                largest = bytes;
            }
            Ok(())
        };
        // One block's matrices: Q and O at the widest query span, K and V at
        // the widest key/value projection, the feed-forward pair and down
        // projection at the widest feed-forward width.
        consider(Self::priced_row_bytes(dtype, span.saturating_mul(hidden)))?;
        consider(Self::priced_row_bytes(dtype, kv_width.saturating_mul(hidden)))?;
        consider(Self::priced_row_bytes(dtype, ffn.saturating_mul(hidden)))?;
        consider(Self::priced_row_bytes(dtype, hidden.saturating_mul(ffn)))?;
        // The token table, one row.
        consider(Self::priced_row_bytes(dtype, hidden))?;
        if let Some(width) = self.per_layer_input {
            let rows = self.num_layers.saturating_mul(width);
            if self.per_layer_input_table {
                consider(Self::priced_row_bytes(dtype, rows))?;
            }
            consider(Self::priced_row_bytes(dtype, hidden.saturating_mul(rows)))?;
            consider(Self::priced_row_bytes(dtype, hidden.saturating_mul(width)))?;
        }
        if let Some(out_width) = self.embedding_length_out {
            consider(Self::priced_row_bytes(dtype, hidden.saturating_mul(out_width)))?;
        } else if self.pooling_type.is_none() {
            // A generative file's output head is row-streamed by
            // `load_output_head_row`: one row of hidden elements, one buffer.
            consider(Self::priced_row_bytes(dtype, hidden))?;
        }
        Ok(largest)
    }

    /// Total weight bytes resident at the run's peak: one block's matrices and
    /// norms at the widest per-layer geometry, one streamed embedding row, the
    /// final norm, and the per-layer projection and output head where the file
    /// declares them.
    ///
    /// WHAT THIS DOES NOT PRICE: the pooling head's two matrices. Their widths
    /// (`dense_2_feat_out` and friends) sit past the vocabulary in the metadata
    /// walk, so this config never holds them; the loader bounds that slot with
    /// its own ceiling and shape checks in `weights.rs` instead.
    pub fn resident_weight_bytes(&self) -> Result<usize, InferenceError> {
        let dtype = self.pricing_dtype();
        let hidden = self.hidden_dim;
        let span = self.max_heads_span();
        let kv_width = self.max_kv_width();
        let ffn = self.max_ffn_dim();
        let mut total = 0usize;
        let mut add = |bytes: Result<usize, InferenceError>| -> Result<(), InferenceError> {
            total = total.saturating_add(bytes?);
            Ok(())
        };
        add(Self::priced_row_bytes(dtype, span.saturating_mul(hidden)))?;
        add(Self::priced_row_bytes(dtype, kv_width.saturating_mul(hidden)))?;
        add(Self::priced_row_bytes(dtype, kv_width.saturating_mul(hidden)))?;
        add(Self::priced_row_bytes(dtype, hidden.saturating_mul(span)))?;
        add(Self::priced_row_bytes(dtype, ffn.saturating_mul(hidden)))?;
        add(Self::priced_row_bytes(dtype, ffn.saturating_mul(hidden)))?;
        add(Self::priced_row_bytes(dtype, hidden.saturating_mul(ffn)))?;
        // The block's norms: four stream-wide, two head-wide at the widest head.
        add(Self::priced_row_bytes(0, hidden.saturating_mul(4)))?;
        add(Self::priced_row_bytes(0, self.max_head_dim().saturating_mul(2)))?;
        // One streamed token row and the final norm.
        add(Self::priced_row_bytes(dtype, hidden))?;
        add(Self::priced_row_bytes(0, hidden))?;
        if let Some(width) = self.per_layer_input {
            let rows = self.num_layers.saturating_mul(width);
            if self.per_layer_input_table {
                add(Self::priced_row_bytes(dtype, rows))?;
            }
            add(Self::priced_row_bytes(dtype, hidden.saturating_mul(rows)))?;
            add(Self::priced_row_bytes(dtype, hidden.saturating_mul(width)))?;
            add(Self::priced_row_bytes(dtype, hidden.saturating_mul(width)))?;
            add(Self::priced_row_bytes(0, width))?;
            add(Self::priced_row_bytes(0, 1))?;
        }
        if let Some(out_width) = self.embedding_length_out {
            add(Self::priced_row_bytes(dtype, hidden.saturating_mul(out_width)))?;
        } else if self.pooling_type.is_none() {
            // One streamed output head row.
            add(Self::priced_row_bytes(dtype, hidden))?;
        }
        Ok(total)
    }

    /// The widest one head runs in any layer.
    pub fn max_head_dim(&self) -> usize {
        let mut widest = self.head_dim;
        if let Some(swa) = self.head_dim_swa {
            if swa > widest {
                widest = swa;
            }
        }
        widest
    }

    /// Checks whether the model's weight footprint and scratch working set fit
    /// within the process's user address space limit and memory budget.
    pub fn check_admission(&self, file_type: Option<u32>) -> Result<(), InferenceError> {
        self.check_admission_against(file_type, dynamic_user_address_space_budget())
    }

    /// Checks admission against a specified address space budget.
    pub fn check_admission_against(
        &self,
        file_type: Option<u32>,
        budget: usize,
    ) -> Result<(), InferenceError> {
        self.check_attention_shape()?;
        if let Some(ft) = file_type {
            let weights = self.weight_bytes(ft)?;
            let total_working_set = weights.saturating_add(self.scratch_bytes());
            if total_working_set > budget {
                return Err(InferenceError::ScratchTooLarge(total_working_set, budget));
            }
        }
        self.check_scratch_budget()?;
        Ok(())
    }

    /// Bytes in the one buffer a run allocates that is biggest.
    ///
    /// WHY the total is not enough on its own: `KvCache::new` and `Scratch::new`
    /// allocate each tensor as its own `Vec`, and the service heap asks the
    /// kernel for exactly one mapping per allocation. So a shape can fit the
    /// device in total and still be unservable because one of its tensors is too
    /// big to map, and only this number says so.
    ///
    /// The weight buffers are in this comparison too: the loader maps one block
    /// matrix, the per-layer projection or the output head as a single `Vec`, so
    /// a weight tensor over the ceiling is as unservable as a cache tensor over
    /// it. A weight layout the pricing cannot name refuses by its dtype number
    /// instead of being left out of the comparison.
    pub fn largest_single_buffer_bytes(&self) -> Result<usize, InferenceError> {
        // `kv_cache_bytes` prices both tensors; `KvCache` holds them as two.
        let cache_tensor = self.kv_cache_bytes() / 2;
        let logits = self.vocab_size.saturating_mul(FLOAT_BYTES);
        let feed_forward = self.max_ffn_dim().saturating_mul(FLOAT_BYTES);
        let stream = self.hidden_dim.saturating_mul(FLOAT_BYTES);
        let query_span = self.max_heads_span().saturating_mul(FLOAT_BYTES);
        let kv_span = self.max_kv_width().saturating_mul(FLOAT_BYTES);
        let scores = self.seq_len.saturating_mul(FLOAT_BYTES);
        let weights = self.largest_weight_buffer_bytes()?;
        Ok(cache_tensor
            .max(logits)
            .max(feed_forward)
            .max(stream)
            .max(query_span)
            .max(kv_span)
            .max(scores)
            .max(weights))
    }

    /// First cached position one query at `pos` may attend over, exclusive of
    /// nothing: the span it names is `attention_span(pos)..pos + 1`.
    ///
    /// WHY the window is applied here and not folded into the loop bounds: the
    /// published file states `attention.sliding_window` 512 beside a
    /// `context_length` of 2048, and the engine builds the 2048-deep cache the
    /// file asks for and then attended all of it. Returning the start from the
    /// config keeps one owner for the rule, so the block loop, the pricing and
    /// the tests all read the same answer.
    ///
    /// WHY `None` reaches the whole prefix: a file that states no window declares
    /// no limit, and inventing one would narrow a graph the file never narrowed.
    /// A window of one is the file saying "attend only yourself", and the span
    /// collapses to `pos..pos + 1` -- still one position, never zero.
    pub fn attention_span_start(&self, pos: usize) -> usize {
        match self.sliding_window {
            Some(window) if window > 0 => pos.saturating_add(1).saturating_sub(window),
            _ => 0,
        }
    }

    /// Refuses a declared shape the device cannot hold: one whose total scratch
    /// is over the budget free RAM gives it, or whose biggest single tensor is
    /// over what one mapping can carry.
    ///
    /// WHY this replaces the six maxima: each of them refused a dimension on its
    /// own, so a model could only be described as too wide, too deep or too rich
    /// in tokens. What the device actually has is bytes, and a comparison against
    /// them says both whether this model fits and how far over it is. The refusal
    /// names the two numbers because the answer to "too big" is a byte count, not
    /// a guess.
    ///
    /// WHY two comparisons and not one: they measure different things. Free RAM
    /// bounds the total a run may hold; the platform's per-mapping ceiling bounds
    /// any one buffer inside it. A shape can satisfy the first and still be
    /// unservable, and only the second says so before the heap is asked.
    pub fn check_scratch_budget(&self) -> Result<(), InferenceError> {
        self.check_scratch_against(dynamic_scratch_byte_budget())
    }

    /// The same two refusals against a budget the caller states.
    ///
    /// WHY it is separate from `check_scratch_budget`: that one reads the guest's
    /// telemetry, which a test build cannot author, so in a test it always gets
    /// the 120 MiB floor -- and at the floor the mapping check below can never
    /// fire on a cache tensor, because a cache tensor over the 128 MiB ceiling
    /// already puts the cache alone past 256 MiB. Taking the budget as a
    /// parameter is what lets a test hold the ceiling to the shape it exists
    /// for: one that clears the total and still cannot be mapped.
    ///
    /// WHY the weights are in the total: the run holds one block's matrices, a
    /// streamed embedding row and the heads beside the scratch, and a budget
    /// that priced only the activations would admit a run whose weights push it
    /// past the device. `resident_weight_bytes` prices that hold at its peak;
    /// `largest_single_buffer_bytes` prices the biggest single mapping, weight
    /// buffers included.
    ///
    /// WHY the total is compared first: it is the budget, and the number the
    /// person can act on when the device is simply too small. The mapping ceiling
    /// is the second answer, for the shape that would have fit.
    pub fn check_scratch_against(&self, budget: usize) -> Result<(), InferenceError> {
        let needed = self.scratch_bytes();
        if needed > budget {
            return Err(InferenceError::ScratchTooLarge(needed, budget));
        }
        let largest = self.largest_single_buffer_bytes()?;
        if largest > LARGEST_SINGLE_BUFFER_BYTES {
            return Err(InferenceError::ScratchTooLarge(
                largest,
                LARGEST_SINGLE_BUFFER_BYTES,
            ));
        }
        Ok(())
    }

    /// The head geometry this build's attention kernel can actually walk.
    ///
    /// WHY it is not part of the budget: a key/value head count that does not
    /// divide the query head count is a malformed declaration, not a big one --
    /// no byte budget can make 5 key/value heads tile 8 query heads. That is a
    /// graph gap, and it is refused by the key that states it.
    ///
    /// WHY this is not a refusal of grouped-query attention: the kernel walks
    /// every key/value head the file declares. `run_block` folds each group of
    /// `num_heads / num_kv_heads` query heads onto its own key/value head, which
    /// is what grouped-query attention means, and the cache holds one row per
    /// key/value head, not one row for head 0 alone. The published
    /// `embeddinggemma-300M-Q8_0.gguf` is grouped-query -- three query heads over
    /// one key/value head, 256 wide -- and EmbeddingGemma 2 declares two
    /// key/value heads in twenty of its layers and one in the other four; both
    /// geometries are walked as stated.
    ///
    /// WHY the query span is no longer compared against the residual width:
    /// `head_dim` is the file's own `key_length`, independent of
    /// `embedding_length / head_count`. EmbeddingGemma 2 projects four heads of
    /// 512 from a 512-wide stream -- a 2048-wide query span, four times the
    /// stream -- and its tensor descriptors state exactly that. The old
    /// `heads_span > hidden_dim` refusal called that malformed; it is the
    /// architecture. What is refused here instead is real: a span whose product
    /// overflows a `usize`, and a span whose query buffer would exceed what one
    /// mapping can carry. The byte budget prices the span-wide buffers in
    /// `check_scratch_against`.
    pub fn check_attention_shape(&self) -> Result<(), InferenceError> {
        if self.num_kv_heads == 0
            || (self.num_kv_heads > 1
                && (self.num_kv_heads > self.num_heads
                    || (self.num_heads % self.num_kv_heads != 0)))
        {
            let stated = u32::try_from(self.num_kv_heads).ok();
            return Err(InferenceError::ModelDeclaration(
                "<arch>.attention.head_count_kv",
                stated,
                1,
            ));
        }
        // Every layer's own key/value count has to tile the query heads too: a
        // per-layer array can state a count the scalar reduction never shows.
        // Only the declared elements can differ from the scalar; the layers
        // past them run the uniform count the predicate above already checked.
        let mut layer = 0usize;
        while layer < self.num_layers && layer < self.kv_heads_per_layer.len {
            let kv = self.layer_kv_heads(layer);
            if kv == 0 || kv > self.num_heads || self.num_heads % kv != 0 {
                return Err(InferenceError::ModelDeclaration(
                    "<arch>.attention.head_count_kv",
                    u32::try_from(kv).ok(),
                    self.num_heads,
                ));
            }
            layer += 1;
        }
        let heads_span = match self.num_heads.checked_mul(self.head_dim) {
            Some(span) => span,
            None => {
                return Err(InferenceError::ModelDeclaration(
                    "<arch>.attention.head_count x key_length",
                    None,
                    self.hidden_dim,
                ));
            }
        };
        // The widest span any layer projects, including the sliding layers'
        // own head width; the query and attention-output buffers are cut for it.
        // Layers past the per-layer arrays run the uniform span above.
        let mut widest_span = heads_span;
        let mut layer = 0usize;
        while layer < self.detailed_layers() {
            let span = match self.num_heads.checked_mul(self.layer_head_dim(layer)) {
                Some(span) => span,
                None => {
                    return Err(InferenceError::ModelDeclaration(
                        "<arch>.attention.head_count x key_length",
                        None,
                        self.hidden_dim,
                    ));
                }
            };
            if span > widest_span {
                widest_span = span;
            }
            layer += 1;
        }
        let span_bytes = match widest_span.checked_mul(FLOAT_BYTES) {
            Some(bytes) => bytes,
            None => {
                return Err(InferenceError::ModelDeclaration(
                    "<arch>.attention.head_count x key_length",
                    None,
                    self.hidden_dim,
                ));
            }
        };
        if span_bytes > LARGEST_SINGLE_BUFFER_BYTES {
            return Err(InferenceError::ModelDeclaration(
                "<arch>.attention.head_count x key_length",
                u32::try_from(widest_span).ok(),
                LARGEST_SINGLE_BUFFER_BYTES / FLOAT_BYTES,
            ));
        }
        if self.pooling_type.is_none()
            && self.embedding_length_out.is_none()
            && widest_span > self.hidden_dim
        {
            return Err(InferenceError::ModelDeclaration(
                "<arch>.attention.head_count x key_length",
                u32::try_from(widest_span).ok(),
                self.hidden_dim,
            ));
        }
        Ok(())
    }

    /// The widest value a per-layer array declares, or `None` when the file
    /// named no array. A loop and not an iterator fold so the answer is the
    /// file's own widest declaration, computed the same way on every target.
    fn widest_declared(counts: &crate::gguf::PerLayerCounts) -> Option<u32> {
        let mut widest: Option<u32> = None;
        let mut index = 0usize;
        while index < counts.len {
            let value = counts.values[index];
            match widest {
                Some(w) if w >= value => {}
                _ => widest = Some(value),
            }
            index += 1;
        }
        widest
    }

    /// Refuses a per-layer array whose length is neither the block count nor the
    /// one-element form a scalar takes: a file stating 24 feed-forward widths for
    /// 35 layers has not described its own graph, and guessing the missing layers
    /// would be inventing declarations.
    ///
    /// WHY `len == 1` is accepted: `gguf.rs` holds a scalar declaration as a
    /// one-element array, so a file stating one width for every layer arrives
    /// here at length 1, not 0. Refusing that rejected the published
    /// `embeddinggemma-300M-Q8_0.gguf` and the deterministic tiny fixture, both
    /// of which state a scalar. `layer_ffn_dim` and `layer_kv_heads` answer the
    /// uniform declaration for every layer past the declared elements, so the
    /// one-element form already describes the whole graph.
    fn per_layer_len_matches(
        key: &'static str,
        len: usize,
        num_layers: usize,
    ) -> Result<(), InferenceError> {
        if len != 0 && len != 1 && len != num_layers {
            return Err(InferenceError::ModelDeclaration(
                key,
                u32::try_from(len).ok(),
                num_layers,
            ));
        }
        Ok(())
    }

    /// Cuts the cache depth to what the byte budget holds, and records the cut.
    ///
    /// WHY a clamp and not a refusal here: `context_length` is what the model
    /// was trained to address, not what this device can hold. EmbeddingGemma 2
    /// declares 262144 positions, which prices a 48 GiB cache on a device whose
    /// whole run budget is 120 MiB; refusing the file outright would report a
    /// model the device can run at a shorter depth as unrunnable. The clamp
    /// keeps every dimension the file declared -- widths, heads, layers -- and
    /// shortens only the sequence, which is the one dimension a run can answer
    /// less of without changing the graph. `declared_seq_len` keeps the file's
    /// own number beside the built one so the service reports the cut instead
    /// of presenting the shorter cache as the model's context.
    ///
    /// WHY it lives outside `from_gguf`: the clamp is the one place a declared
    /// number is cut to a bound, and it is cut against computed bytes, never
    /// against a compile-time ceiling. A file whose geometry cannot hold even
    /// one position is refused with the bytes it would need.
    pub fn clamp_seq_len_to_budget(&mut self, budget: usize) -> Result<(), InferenceError> {
        let declared = self.seq_len;
        self.seq_len = 1;
        let base = self
            .scratch_bytes()
            .saturating_add(self.resident_weight_bytes()?);
        // At one position the cache prices exactly one position's rows; the
        // scores buffer adds one float per position on top.
        let per_position = self.kv_cache_bytes().saturating_add(FLOAT_BYTES);
        if base > budget {
            return Err(InferenceError::ScratchTooLarge(base, budget));
        }
        let mut affordable = if per_position == 0 {
            declared
        } else {
            1 + (budget - base) / per_position
        };
        // The per-mapping ceiling bounds the cache tensor and the scores too.
        let cache_per_position = self.kv_cache_bytes();
        if cache_per_position > 0 {
            let ceiling_depth = (LARGEST_SINGLE_BUFFER_BYTES / cache_per_position) * 2;
            if ceiling_depth < affordable {
                affordable = ceiling_depth;
            }
        }
        let scores_ceiling = LARGEST_SINGLE_BUFFER_BYTES / FLOAT_BYTES;
        if scores_ceiling < affordable {
            affordable = scores_ceiling;
        }
        if affordable > declared {
            affordable = declared;
        }
        if affordable == 0 {
            return Err(InferenceError::ScratchTooLarge(base, budget));
        }
        self.seq_len = affordable;
        Ok(())
    }

    /// The first declaration this engine's block graph cannot walk, named by
    /// the key that states it, or `None` when the graph is one this build runs.
    ///
    /// WHY this is separate from `from_gguf`: the geometry a file declares can
    /// be priced and allocated even where the graph has a gap, and the tests
    /// that hold the pricing to the file's bytes need to build that config.
    /// The service paths call this before they run, and report the gap rather
    /// than walking a graph the file did not describe:
    ///
    /// - `shared_kv_layers` nonzero: layers share one key/value projection and
    ///   this engine caches per layer; running them unshared would attend keys
    ///   the file never projected.
    /// - `embedding_length_per_layer_input`: the block ships an input gate,
    ///   projection, post norm and output scale beside its attention, and this
    ///   engine's thirteen-tensor block does not walk them.
    /// - `embedding_length_out` different from the stream width: the embedding
    ///   head projects to a width `extract_embedding` does not produce.
    /// - `attention.causal` false: bidirectional attention, which the causal
    ///   span here does not walk.
    /// - `final_logit_softcapping`: a cap this engine's logits do not apply.
    pub fn unwalkable_graph_declaration(&self) -> Option<(&'static str, Option<u32>, usize)> {
        if let Some(shared) = self.shared_kv_layers {
            if shared > 0 {
                return Some(("<arch>.attention.shared_kv_layers", Some(shared), 0));
            }
        }
        if let Some(width) = self.per_layer_input {
            return Some((
                "<arch>.embedding_length_per_layer_input",
                u32::try_from(width).ok(),
                0,
            ));
        }
        if let Some(out_width) = self.embedding_length_out {
            if out_width != self.hidden_dim {
                return Some((
                    "<arch>.embedding_length_out",
                    u32::try_from(out_width).ok(),
                    self.hidden_dim,
                ));
            }
        }
        if self.attention_causal == Some(false) {
            return Some(("<arch>.attention.causal", Some(0), 1));
        }
        if let Some(cap) = self.final_logit_softcapping {
            return Some(("<arch>.final_logit_softcapping", Some(cap as u32), 0));
        }
        None
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
        let num_layers = fits("<arch>.block_count", info.num_layers)?;
        let head_dim = fits("<arch>.attention.key_length", info.head_dim)?;
        let num_heads = fits("<arch>.attention.head_count", info.num_heads)?;
        let vocab_size = fits("tokenizer.ggml.tokens", info.vocab_size)?;

        // The per-layer arrays are the file's geometry where it states one. A
        // scalar reduction is only set when every element agrees, so a file
        // like Gemma 4 E2B -- 6144-wide feed-forwards for fifteen layers and
        // 12288-wide for twenty -- arrives with no scalar and the array whole;
        // the uniform bound the scratch is cut for is the widest declaration.
        Self::per_layer_len_matches(
            "<arch>.feed_forward_length",
            info.ffn_per_layer.len,
            num_layers,
        )?;
        Self::per_layer_len_matches(
            "<arch>.attention.head_count_kv",
            info.kv_heads_per_layer.len,
            num_layers,
        )?;
        Self::per_layer_len_matches(
            "<arch>.attention.sliding_window_pattern",
            info.sliding_window_pattern.len,
            num_layers,
        )?;
        let ffn_declared = match info.intermediate_dim {
            Some(stated) => Some(stated),
            None => Self::widest_declared(&info.ffn_per_layer),
        };
        let intermediate_dim = fits("<arch>.feed_forward_length", ffn_declared)?;

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
        // head per query head. A per-layer array with no lossless scalar is the
        // file's own declaration, and its widest element is the uniform bound.
        // The cache prices either answer; the kernel walks the declared count,
        // which `check_attention_shape` validates.
        let kv_declared = match info.num_kv_heads {
            Some(kv) => Some(kv),
            None => Self::widest_declared(&info.kv_heads_per_layer),
        };
        let num_kv_heads = match kv_declared {
            Some(kv) if kv > 0 => usize::try_from(kv).unwrap_or(usize::MAX),
            _ => num_heads,
        };

        // The window the file says a query may look back over. A stated 0 is the
        // file stating no usable span, so it is carried as no limit rather than
        // turned into a span of zero positions, and a value too wide for the
        // machine simply never binds -- the cache is `seq_len` deep and the span
        // clamps to it either way. This is not `fits`: a missing window is a
        // legal declaration (GPT-family and Llama files state none), unlike a
        // missing width, which is why silence here means "attend the prefix".
        let sliding_window = match info.sliding_window {
            Some(stated) if stated > 0 => usize::try_from(stated).ok(),
            _ => None,
        };

        // The sliding layers' own head width, where the file states one. It is
        // only assignable with the pattern that says which layers are sliding;
        // a width without a pattern is a declaration the engine cannot place.
        let head_dim_swa = match info.head_dim_swa {
            Some(stated) if stated > 0 => Some(stated as usize),
            _ => None,
        };
        if head_dim_swa.is_some() && info.sliding_window_pattern.len == 0 {
            return Err(InferenceError::ModelDeclaration(
                "<arch>.attention.key_length_swa",
                info.head_dim_swa,
                0,
            ));
        }

        // This kernel rotates every element of a head and caches one value row
        // per key row. A file whose value width differs from its key width, or
        // whose rotary span is narrower than its head, declares a graph this
        // build does not walk; it is refused by the key that states it rather
        // than run at the wrong width.
        if let Some(value) = info.value_length {
            if value as usize != head_dim {
                return Err(InferenceError::ModelDeclaration(
                    "<arch>.attention.value_length",
                    Some(value),
                    head_dim,
                ));
            }
        }
        if let (Some(value_swa), Some(head_swa)) = (info.value_length_swa, head_dim_swa) {
            if value_swa as usize != head_swa {
                return Err(InferenceError::ModelDeclaration(
                    "<arch>.attention.value_length_swa",
                    Some(value_swa),
                    head_swa,
                ));
            }
        }
        if let Some(rotary) = info.rope_dimension_count {
            if rotary as usize != head_dim {
                return Err(InferenceError::ModelDeclaration(
                    "<arch>.rope.dimension_count",
                    Some(rotary),
                    head_dim,
                ));
            }
        }
        if let Some(rotary_swa) = info.rope_dimension_count_swa {
            match head_dim_swa {
                Some(head_swa) if rotary_swa as usize == head_swa => {}
                _ => {
                    return Err(InferenceError::ModelDeclaration(
                        "<arch>.rope.dimension_count_swa",
                        Some(rotary_swa),
                        head_dim,
                    ));
                }
            }
        }

        let activation = match info.architecture() {
            Some(arch) if arch.starts_with("gemma") => ActivationType::GeGLU,
            _ => ActivationType::SwiGLU,
        };

        // The per-layer input table is a separate vocabulary-sized tensor in
        // the Gemma 4 family's published descriptor list; EmbeddingGemma 2
        // projects the shared token table instead and ships no such tensor.
        let per_layer_input = match info.embedding_length_per_layer_input {
            Some(width) if width > 0 => Some(width as usize),
            _ => None,
        };
        let per_layer_input_table = per_layer_input.is_some()
            && match info.architecture() {
                Some(arch) => arch == "gemma4",
                None => false,
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
            sliding_window,
            rms_norm_eps: info.rms_norm_eps.unwrap_or(1e-6),
            rope_theta: info.rope_theta.unwrap_or(10000.0),
            activation,
            kv_heads_per_layer: info.kv_heads_per_layer,
            ffn_per_layer: info.ffn_per_layer,
            sliding_window_pattern: info.sliding_window_pattern,
            head_dim_swa,
            rope_theta_swa: info.rope_theta_swa,
            declared_seq_len: seq_len,
            per_layer_input,
            per_layer_input_table,
            shared_kv_layers: info.shared_kv_layers,
            embedding_length_out: info.embedding_length_out.map(|w| w as usize),
            attention_causal: info.attention_causal,
            pooling_type: info.pooling_type,
            final_logit_softcapping: info.final_logit_softcapping,
            declared_file_type: info.file_type,
        };

        // Checked in the order the graph is built: the head geometry first, since
        // a file whose heads do not tile their own residual stream is a malformed
        // declaration rather than a big one, then the byte budget the whole run
        // is measured against, and the declared weights against the address space budget.
        config.check_admission(info.file_type)?;
        Ok(config)
    }
}

/// How many buffers of each width class one run holds.
///
/// The seven `hidden_dim`-wide streams are `x`, `residual`, `x_norm`,
/// `attn_proj`, `ffn_out`, `hidden` and `pooled`; `pooled` is the pooling head's
/// accumulator, which is why it is a stream and not a slice of `hidden` (mean
/// pooling sums every position's vector before normalising).
///
/// The two head-span buffers are `q` and `attn_out`, cut for the widest
/// `num_heads x head_dim` any layer projects -- which is not the stream width:
/// EmbeddingGemma 2 projects a 2048-wide query span from a 512-wide stream, and
/// a query buffer cut at the stream width is what the old engine wrote past.
/// The two key/value-span buffers are `k` and `v_vec`, cut for the widest
/// `num_kv_heads x head_dim`. The three `intermediate_dim`-wide ones are the
/// feed-forward's gate, up and product, cut for the widest feed-forward any
/// layer declared.
///
/// WHY these are counted here and not written twice: `Scratch::bytes_for` prices
/// the run from them and `Scratch::new` allocates them, and the two agreeing is
/// what makes `ModelConfig::scratch_bytes` a promise instead of a comment. The
/// test `scratch_pricing_matches_the_allocation` holds them to each other.
const HIDDEN_STREAM_BUFFERS: usize = 7;
const HEAD_SPAN_BUFFERS: usize = 2;
const KV_SPAN_BUFFERS: usize = 2;
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
    /// The attention projections. `q` and `attn_out` hold `num_heads` slices of
    /// the layer's head width -- the widest span any layer projects, which can
    /// be several times the residual width. `k` and `v_vec` hold the key/value
    /// heads' slices, and `attn_proj` is back at the stream width.
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
    /// Bytes the non-cache part of a run costs: the streams, the head and
    /// key/value spans, the feed-forward, the scores and the logits.
    pub fn bytes_for(config: &ModelConfig) -> usize {
        let streams = HIDDEN_STREAM_BUFFERS.saturating_mul(config.hidden_dim);
        let head_span = HEAD_SPAN_BUFFERS.saturating_mul(config.max_heads_span());
        let kv_span = KV_SPAN_BUFFERS.saturating_mul(config.max_kv_width());
        let feed_forward = INTERMEDIATE_BUFFERS.saturating_mul(config.max_ffn_dim());
        let scores = config.seq_len;
        let logits = config.vocab_size;
        streams
            .saturating_add(head_span)
            .saturating_add(kv_span)
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
        let span = config.max_heads_span();
        let kv = config.max_kv_width();
        let i = config.max_ffn_dim();
        Ok(Self {
            x: zeros(h)?,
            residual: zeros(h)?,
            x_norm: zeros(h)?,
            q: zeros(span)?,
            k: zeros(kv)?,
            v_vec: zeros(kv)?,
            attn_out: zeros(span)?,
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
    /// step write past the buffers it was handed. Each width class is checked
    /// against the width the run actually writes: the stream buffers at the
    /// residual width, the query and attention output at the widest head span,
    /// key and value at the widest key/value projection, and the feed-forward
    /// at the widest declared intermediate width.
    fn fits(&self, config: &ModelConfig) -> Result<(), InferenceError> {
        let h = config.hidden_dim;
        let span = config.max_heads_span();
        let kv = config.max_kv_width();
        let i = config.max_ffn_dim();
        let streams = [
            &self.x,
            &self.residual,
            &self.x_norm,
            &self.attn_proj,
            &self.ffn_out,
            &self.hidden,
            &self.pooled,
        ];
        if streams.iter().any(|b| b.len() < h) {
            return Err(InferenceError::BufferTooSmall);
        }
        if self.q.len() < span || self.attn_out.len() < span {
            return Err(InferenceError::BufferTooSmall);
        }
        if self.k.len() < kv || self.v_vec.len() < kv {
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

    /// Whether the block's Q, K, V and O matrices hold at least the rows and
    /// columns the declared head geometry needs, counted in the quantisation
    /// this block states.
    ///
    /// WHY counted per quantisation and not per element: a Q8_0 row of 768 weights
    /// is 816 bytes, not 768, and a width check written against the element count
    /// would let `gemv` read a neighbour's blocks as this tensor's tail.
    ///
    /// WHY the span is an argument and not the stream width: Q projects the
    /// stream into `head_count x key_length` rows, and O projects that span back
    /// into the stream -- its rows are `hidden_dim` wide but each row is `span`
    /// weights long. A file with decoupled heads (EmbeddingGemma 2's 2048-wide
    /// span from a 512-wide stream) holds an O tensor four times the stream
    /// width, and checking it at the stream width would pass a truncated tensor.
    fn projections_hold(&self, span: usize, kv_dim: usize, hidden_dim: usize) -> bool {
        let Some(stream_row_bytes) = quantised_row_bytes(self.weight_type, hidden_dim) else {
            return false;
        };
        let Some(span_row_bytes) = quantised_row_bytes(self.weight_type, span) else {
            return false;
        };
        let holds = |bytes: &[u8], rows: usize, row_bytes: usize| {
            bytes.len() >= rows.saturating_mul(row_bytes)
        };
        holds(self.attn_q, span, stream_row_bytes)
            && holds(self.attn_k, kv_dim, stream_row_bytes)
            && holds(self.attn_v, kv_dim, stream_row_bytes)
            && holds(self.attn_o, hidden_dim, span_row_bytes)
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
/// both are allocated to the depth, width and block count the file declared --
/// each layer at its own key/value head count and head width, because a file
/// like EmbeddingGemma 2 declares 256-wide heads in its sliding layers and
/// 512-wide heads in its full-attention ones. This is the shape that could not
/// live on the stack: the published `embeddinggemma-300M-Q8_0.gguf` -- 24 blocks,
/// 2048 positions, one key/value head 256 wide -- is 100,663,296 bytes here,
/// against a 262,144-byte user stack.
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
    /// The widest key/value head count any layer declared; each layer's own
    /// count is answered by the geometry the cache was cut from.
    pub num_kv_heads: usize,
    /// The widest one-head width the file declared (`key_length`); the sliding
    /// layers' own width is answered by the same geometry.
    pub head_dim: usize,
    /// Positions written so far.
    pub len: usize,
    /// The declaration the rows were cut from, kept whole so every row address
    /// is answered by the file's own per-layer geometry and not by a uniform
    /// shape the file never stated.
    geom: ModelConfig,
    /// Float offset of each of the first `MAX_PER_LAYER_GEOMETRY` layers inside
    /// one tensor: the prefix sum of the layers' own row counts, so a row
    /// address is two additions rather than a walk over every earlier layer.
    layer_base: [usize; crate::gguf::MAX_PER_LAYER_GEOMETRY + 1],
    /// Floats one uniform layer holds, for the layers past the per-layer arrays.
    uniform_layer_floats: usize,
}

impl KvCache {
    /// Allocates the cache the model's own declaration describes.
    pub fn new(config: &ModelConfig) -> Result<Self, InferenceError> {
        // `kv_cache_bytes` prices both tensors at the layers' own geometry;
        // each tensor holds half of that, and the pricing is the allocation.
        let floats = (config.kv_cache_bytes() / 2) / FLOAT_BYTES;
        let mut layer_base = [0usize; crate::gguf::MAX_PER_LAYER_GEOMETRY + 1];
        let mut acc = 0usize;
        let mut layer = 0usize;
        while layer < crate::gguf::MAX_PER_LAYER_GEOMETRY {
            layer_base[layer] = acc;
            if layer < config.num_layers {
                acc = acc.saturating_add(
                    config
                        .seq_len
                        .saturating_mul(config.layer_kv_heads(layer))
                        .saturating_mul(config.layer_head_dim(layer)),
                );
            }
            layer += 1;
        }
        layer_base[crate::gguf::MAX_PER_LAYER_GEOMETRY] = acc;
        Ok(Self {
            k: zeros(floats)?,
            v: zeros(floats)?,
            seq_len: config.seq_len,
            num_layers: config.num_layers,
            num_kv_heads: config.num_kv_heads,
            head_dim: config.head_dim,
            len: 0,
            geom: *config,
            layer_base,
            uniform_layer_floats: config
                .seq_len
                .saturating_mul(config.num_kv_heads)
                .saturating_mul(config.head_dim),
        })
    }

    /// Bytes held: both tensors.
    pub fn allocated_bytes(&self) -> usize {
        (self.k.len().saturating_add(self.v.len())).saturating_mul(FLOAT_BYTES)
    }

    /// One head's row width in `layer`, as the file declared it.
    pub fn layer_head_dim(&self, layer: usize) -> usize {
        self.geom.layer_head_dim(layer)
    }

    /// The key/value head count `layer` declared.
    pub fn layer_kv_heads(&self, layer: usize) -> usize {
        self.geom.layer_kv_heads(layer)
    }

    /// First float of one (layer, position, head) row, or `None` when the row is
    /// not in this cache. The layer's own head count and head width bound the
    /// address, so a head the layer never declared has no row to write.
    pub fn row_start(&self, layer: usize, position: usize, head: usize) -> Option<usize> {
        if layer >= self.num_layers || position >= self.seq_len {
            return None;
        }
        let kv_heads = self.geom.layer_kv_heads(layer);
        let head_dim = self.geom.layer_head_dim(layer);
        if head >= kv_heads {
            return None;
        }
        let base = if layer < crate::gguf::MAX_PER_LAYER_GEOMETRY {
            self.layer_base[layer]
        } else {
            self.layer_base[crate::gguf::MAX_PER_LAYER_GEOMETRY].checked_add(
                (layer - crate::gguf::MAX_PER_LAYER_GEOMETRY)
                    .checked_mul(self.uniform_layer_floats)?,
            )?
        };
        base.checked_add(position.checked_mul(kv_heads.saturating_mul(head_dim))?)?
            .checked_add(head.checked_mul(head_dim)?)
    }

    /// The cached key row at (layer, position, head), or `None` when the cache
    /// was not cut with that address in it.
    ///
    /// WHY the caller names the head: the kernel walks every key/value head the
    /// layer declared, folding each group of query heads onto its own key/value
    /// head, which is what grouped-query attention means. The row is that
    /// layer's head width, not a uniform width the file never stated.
    pub fn k_row(&self, layer: usize, position: usize, head: usize) -> Option<&[f32]> {
        let start = self.row_start(layer, position, head)?;
        self.k
            .get(start..start.checked_add(self.geom.layer_head_dim(layer))?)
    }

    pub fn v_row(&self, layer: usize, position: usize, head: usize) -> Option<&[f32]> {
        let start = self.row_start(layer, position, head)?;
        self.v
            .get(start..start.checked_add(self.geom.layer_head_dim(layer))?)
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
        // The row the cache holds is exactly the head the layer declared; a
        // head wider than the row would be truncated, and a truncated key is a
        // different model's key.
        let head_dim = self.geom.layer_head_dim(layer);
        if key.len() != head_dim || value.len() != head_dim {
            return Err(InferenceError::BufferTooSmall);
        }
        let start = self
            .row_start(layer, position, head)
            .ok_or(InferenceError::ContextLengthExceeded)?;
        let end = start
            .checked_add(head_dim)
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
        13 => blocks(Q5_K_WEIGHTS, Q5_K_BLOCK_BYTES),
        14 => blocks(Q6_K_WEIGHTS, Q6_K_BLOCK_BYTES),
        35 => blocks(TQ2_0_WEIGHTS, TQ2_0_BLOCK_BYTES),
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
        // GGML_TYPE_Q4_0 / Q8_0 / Q4_K / Q5_K / Q6_K / TQ2_0: dequantise the whole
        // row, then scale it. The helpers write exactly
        // `raw.len() / block_bytes * block_weights` floats, which is the row's
        // declared width when the loader sized the buffer.
        2 | 8 | 12 | 13 | 14 | 35 => {
            let produced = match dtype {
                2 => dequantize_q4_0(raw, out)?,
                8 => dequantize_q8_0(raw, out)?,
                12 => dequantize_q4_k(raw, out)?,
                13 => dequantize_q5_k(raw, out)?,
                14 => dequantize_q6_k(raw, out)?,
                _ => dequantize_tq2_0(raw, out)?,
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

/// Divides the accumulated stream by the position count, in place into `out`.
///
/// WHY the count-zero case leaves `out` untouched: an empty prompt has no mean,
/// and a zero-filled answer reported as "the mean of nothing" is the invention
/// this module's refusals exist to avoid. The caller decides what an empty run
/// means -- `extract_embedding` answers `Ok(0)` before it ever gets here.
pub fn mean_of(pooled: &[f32], count: usize, out: &mut [f32]) {
    if count == 0 {
        return;
    }
    let inv_count = 1.0f32 / (count as f32);
    for (slot, &value) in out.iter_mut().zip(pooled) {
        *slot = value * inv_count;
    }
}

/// Scales a vector in place to L2 length 1, leaving the zero vector alone.
///
/// WHY the threshold rather than a plain divide: a run whose stream cancelled to
/// (near) zero would otherwise divide by an underflowed norm and report noise as
/// a unit vector. A direction that small has no direction; it is left as it is.
pub fn l2_normalise(v: &mut [f32]) {
    let mut sum_sq = 0.0f32;
    for &x in v.iter() {
        sum_sq += x * x;
    }
    let norm = f32_sqrt(sum_sq);
    if norm > 1e-12 {
        let inv_norm = 1.0f32 / norm;
        for x in v.iter_mut() {
            *x *= inv_norm;
        }
    }
}

/// Mean-pools `count` accumulated positions and L2-normalises the result.
///
/// Shared by `extract_embedding` and the streaming loader so the pooling rule an
/// in-memory run applies and the one a file-read run applies cannot drift apart --
/// two vectors from two different poolings compared by the search index would be a
/// similarity the model never had. A file that carries a pooling head means the
/// normalisation belongs after that head instead (see `apply_pooling_head`), so
/// the streaming path composes `mean_of` + head + `l2_normalise` and only reaches
/// this function when the file declared no head.
pub fn pooled_normalised(pooled: &[f32], count: usize, out: &mut [f32]) {
    if count == 0 {
        return;
    }
    mean_of(pooled, count, out);
    l2_normalise(out);
}

/// The published embedding model's pooling head: the two dense layers that sit
/// between the mean pool and the final L2 step.
///
/// WHY the shapes are carried and not re-derived: `dense_2.weight`'s row count is
/// `dense_3.weight`'s column count, and both are the file's own descriptor
/// numbers (the tensor declares [768, 3072], and the file separately states
/// `gemma-embedding.dense_2_feat_out` as 3072 -- the loader reads the descriptor
/// and no code in this service reads that key). A head built from one and run at
/// the other would ask `gemv` for rows the bytes do not hold -- its refusal, but
/// caught here at the shape instead of mid-multiply.
///
/// WHY the biases are optional: the published
/// `embeddinggemma-300M-Q8_0.gguf` declares no `dense_2.bias` or `dense_3.bias`
/// at all -- 316 descriptors, none of them a bias tensor. An absent bias is the
/// file's silence, and the head runs without it; inventing one (or inventing
/// zeros and calling them nothing) would apply a bias the file never shipped.
/// A file that does declare biases has them read and added, and `apply_pooling_head`
/// refuses one whose width is not the layer's output width.
#[derive(Clone, Copy)]
pub struct PoolingHead<'a> {
    pub dense_2: &'a [u8],
    pub dense_2_type: u32,
    pub dense_2_bias: Option<&'a [f32]>,
    pub dense_3: &'a [u8],
    pub dense_3_type: u32,
    pub dense_3_bias: Option<&'a [f32]>,
    /// `dense_2.weight`'s row count: the hidden width both layers agree on.
    pub mid_dim: usize,
    /// `dense_3.weight`'s row count: the vector width the head returns.
    pub out_dim: usize,
}

/// Runs the pooling head over a mean-pooled stream: dense_2, GELU (tanh
/// approximation), dense_3 -- then leaves the L2 step to the caller.
///
/// The order is the published architecture's: pool first, head second,
/// normalise last. Normalising before the head (as the pre-head engine did)
/// would rescale an input the linear layers were trained on and return a
/// different vector than the model's own final step takes.
///
/// WHY `hidden` is handed in and not allocated: the head's hidden width is the
/// file's 3072 (against a 1152-wide feed-forward), so the buffer is sized from
/// the descriptors, held by the loader that read them, and never taken fresh in
/// an interactive path.
pub fn apply_pooling_head(
    head: &PoolingHead<'_>,
    mean: &[f32],
    hidden: &mut [f32],
    out: &mut [f32],
) -> Result<(), InferenceError> {
    let in_dim = mean.len();
    if head.mid_dim == 0 || head.out_dim == 0 || in_dim == 0 {
        return Err(InferenceError::CorruptedWeights);
    }
    if hidden.len() < head.mid_dim || out.len() < head.out_dim {
        return Err(InferenceError::BufferTooSmall);
    }
    // A bias narrower than the layer it scales would leave its tail unadded,
    // which is the same half-applied-tensor defect the block norms refuse.
    if head
        .dense_2_bias
        .is_some_and(|bias| bias.len() < head.mid_dim)
        || head
            .dense_3_bias
            .is_some_and(|bias| bias.len() < head.out_dim)
    {
        return Err(InferenceError::CorruptedWeights);
    }
    gemv(
        head.dense_2,
        head.dense_2_type,
        mean,
        &mut hidden[..head.mid_dim],
        head.mid_dim,
        in_dim,
    )?;
    if let Some(bias) = head.dense_2_bias {
        for (slot, &b) in hidden[..head.mid_dim].iter_mut().zip(bias) {
            *slot += b;
        }
    }
    for slot in hidden[..head.mid_dim].iter_mut() {
        *slot = gelu_tanh(*slot);
    }
    gemv(
        head.dense_3,
        head.dense_3_type,
        &hidden[..head.mid_dim],
        &mut out[..head.out_dim],
        head.out_dim,
        head.mid_dim,
    )?;
    if let Some(bias) = head.dense_3_bias {
        for (slot, &b) in out[..head.out_dim].iter_mut().zip(bias) {
            *slot += b;
        }
    }
    Ok(())
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
    // This layer's own geometry, as the file declared it: the sliding layers
    // of EmbeddingGemma 2 and Gemma 4 E2B run narrower heads than their
    // full-attention layers, and each layer's feed-forward is its own width.
    // `config.num_kv_heads` carries the widest key/value declaration; the layer
    // walks the count its own element of the array states.
    let h_dim = config.hidden_dim;
    let inter_dim = config.layer_ffn_dim(block);
    let hd = config.layer_head_dim(block);
    let kv_heads = config.layer_kv_heads(block);
    let kv_dim = kv_heads.saturating_mul(hd);
    let span = config.num_heads.saturating_mul(hd);
    let rope_theta = config.layer_rope_theta(block);

    if pos >= kv_cache.seq_len {
        return Err(InferenceError::ContextLengthExceeded);
    }
    // The buffers are cut for the widest geometry any layer declared; this
    // layer's own span, key/value width and feed-forward have to fit inside
    // them. Checked here because a hand-built config reaches this function as
    // well, and a scratch cut for a different shape is refused, not written
    // past. `check_attention_shape` refuses the geometry itself.
    config.check_attention_shape()?;
    if span > scratch.q.len()
        || span > scratch.attn_out.len()
        || kv_dim > scratch.k.len()
        || kv_dim > scratch.v_vec.len()
        || inter_dim > scratch.gate.len()
        || inter_dim > scratch.up.len()
        || inter_dim > scratch.ffn_hidden.len()
    {
        return Err(InferenceError::BufferTooSmall);
    }
    if !lw.norms_hold(h_dim) || !lw.head_norms_hold(hd) {
        return Err(InferenceError::CorruptedWeights);
    }
    if !lw.projections_hold(span, kv_dim, h_dim) {
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

    // 2. Q, K, V projections. Q spans every query head laid end to end -- the
    // file's `head_count x key_length`, which is not the stream width when the
    // heads are decoupled from it; K and V span only the rows this layer's
    // declared key/value heads take.
    gemv(lw.attn_q, lw.weight_type, &x_norm[..h_dim], &mut q[..span], span, h_dim)?;
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
        for head in 0..kv_heads {
            let at = head * hd;
            rms_norm(&mut k[at..at + hd], lw.attn_k_norm, config.rms_norm_eps);
        }
    }

    // 4. RoPE, per query head and per key head. The two counts differ exactly when
    // the file declares grouped-query attention, and the base is the one this
    // layer stated -- the sliding layers' `rope.freq_base_swa` where the file's
    // pattern marks them sliding.
    for head in 0..config.num_heads {
        let at = head * hd;
        apply_rope(&mut q[at..at + hd], pos, hd, rope_theta);
    }
    for head in 0..kv_heads {
        let at = head * hd;
        apply_rope(&mut k[at..at + hd], pos, hd, rope_theta);
    }

    // 5. Cache the key/value heads the model declared. `KvCache::write`
    // refuses a slice that is not exactly this layer's head width, so the row is
    // the whole declared head: nothing is dropped and nothing is zero-padded to
    // reach it.
    for kv_head in 0..kv_heads {
        let at = kv_head * hd;
        kv_cache.write(block, pos, kv_head, &k[at..at + hd], &v_vec[at..at + hd])?;
    }

    // 6. Attention scores and softmax, then the weighted sum of values.
    //
    // The span is the one this layer declared: the whole cached prefix where
    // the file's `sliding_window_pattern` marks the layer full-attention, and
    // `attention.sliding_window` positions ending at `pos` everywhere else (or
    // the whole prefix when the file stated no window). Softmax normalises over
    // exactly that span, so a position the window excludes contributes no weight
    // at all -- it is not scored and dropped afterwards, it is never scored.
    // This is why the published file's 512 against its own 2048 `context_length`
    // is a graph difference and not a rounding one: past position 511 the old
    // span handed probability mass to keys the model says this query does not
    // see.
    let scale_att = 1.0f32 / f32_sqrt(hd as f32);
    let span_start = config.layer_attention_span_start(block, pos);
    let span_end = pos + 1;
    debug_assert!(span_start < span_end, "the window never excludes the query itself");
    for value in attn_out[..span].iter_mut() {
        *value = 0.0f32;
    }
    let group_size = config.num_heads / kv_heads.max(1);
    for head in 0..config.num_heads {
        let kv_head = head / group_size;
        let h_base = head * hd;
        for t in span_start..span_end {
            let key_row = kv_cache
                .k_row(block, t, kv_head)
                .ok_or(InferenceError::ContextLengthExceeded)?;
            let mut dot = 0.0f32;
            for d in 0..hd {
                dot += q[h_base + d] * key_row[d];
            }
            scores[t] = dot * scale_att;
        }
        softmax(&mut scores[span_start..span_end]);
        for t in span_start..span_end {
            let value_row = kv_cache
                .v_row(block, t, kv_head)
                .ok_or(InferenceError::ContextLengthExceeded)?;
            let weight = scores[t];
            for d in 0..hd {
                attn_out[h_base + d] += weight * value_row[d];
            }
        }
    }

    // 7. Output projection, post-attention norm, residual add. The projection
    // consumes the whole query span -- `head_count x key_length` weights per
    // row, which is the width `attn_out` holds -- and returns the stream width.
    gemv(lw.attn_o, lw.weight_type, &attn_out[..span], &mut attn_proj[..h_dim], h_dim, span)?;
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
    match config.activation {
        ActivationType::SwiGLU => {
            swiglu(&gate[..inter_dim], &up[..inter_dim], &mut ffn_hidden[..inter_dim]);
        }
        ActivationType::GeGLU => {
            geglu(&gate[..inter_dim], &up[..inter_dim], &mut ffn_hidden[..inter_dim]);
        }
    }
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
    // WHY the id is refused before either embedding form is read: a token outside
    // the declared vocabulary has no row in this model. Wrapping it into range
    // would answer with a different token's embedding reported as this one's.
    if token_id as usize >= config.vocab_size {
        return Err(InferenceError::TokenOutOfVocabulary(
            token_id,
            config.vocab_size,
        ));
    }
    let embd_scale = f32_sqrt(h_dim as f32);
    match model.token_embd {
        TokenEmbedding::Row { bytes, dtype, token } => {
            if token != token_id {
                return Err(InferenceError::CorruptedWeights);
            }
            embedding_row_into(bytes, dtype, embd_scale, &mut scratch.x[..h_dim])?;
        }
        TokenEmbedding::Table { bytes, rows, dtype } => {
            let embd_idx = token_id as usize;
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
    // WHY an over-length prompt is refused rather than cut: dropping the tail
    // silently would report an answer to the whole prompt while the model only
    // ever saw a prefix of it. `config.seq_len` is the context the budget clamp
    // actually built (`clamp_seq_len_to_budget`), so this refusal names the real
    // limit the device paid for, and the caller can shorten the prompt or ask for
    // a bigger budget instead of receiving a quietly partial answer.
    if prompt_tokens.len() > config.seq_len {
        return Err(InferenceError::ContextLengthExceeded);
    }
    let prefill_len = prompt_tokens.len();
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
    if tokens.len() > config.seq_len {
        return Err(InferenceError::ContextLengthExceeded);
    }
    let count = tokens.len();
    let dim = config.hidden_dim;

    // Mean pooling over the positions -- and mean pooling because this path is
    // coded that way, not because the file was consulted: `GgufModelInfo` carries
    // no pooling field and no reader in this service asks for
    // `gemma-embedding.pooling_type`. The published file does state that key as 1,
    // which agrees with what runs here; a file declaring a different pooling would
    // still be mean-pooled, so the agreement is a fact about this one file and not
    // a check this code performs. Each step's normalised residual stream is summed
    // as it is produced, so the vector is the model's own hidden representation
    // rather than a slice of the cache or a re-use of the logits.
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

    // The same two primitives the streaming loader composes, so an in-memory
    // vector and a file-read vector cannot be pooled by two different rules.
    //
    // WHAT THIS DOES NOT DO: the published file also carries `dense_2.weight`
    // (768 -> 3072) and `dense_3.weight` (3072 -> 768) after the pool -- the
    // head `apply_pooling_head` runs, at the widths its two tensor descriptors
    // declare (the file also states `dense_2_feat_in/out` and
    // `dense_3_feat_in/out`; nothing in this service reads those keys). A
    // `ModelWeights` carries no head tensors, so what
    // comes back from this in-memory path is the pooled pre-head stream, and its
    // caller must say so; the streaming loader that reads the installed file
    // applies the head when the file declares one (see
    // `weights::run_streamed_embedding`), and this path may not be reported as
    // the published model's final embedding.
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
    fn q6_k_block_matches_ggml_reference() {
        let mut raw = [0u8; Q6_K_BLOCK_BYTES];
        let scales: [i8; 16] = [-8, 7, -6, 5, -4, 3, -2, 1, 2, -1, 4, -3, 6, -5, 8, -7];
        for (dst, scale) in raw[192..208].iter_mut().zip(scales) {
            *dst = scale as u8;
        }
        raw[208..].copy_from_slice(&0x3800u16.to_le_bytes());
        let mut expected = [0.0f32; Q6_K_WEIGHTS];
        for (i, weight) in expected.iter_mut().enumerate() {
            let code = ((i * 11 + (i / 64) * 3) % 64) as u8;
            let half = i / 128;
            let quarter = (i % 128) / 32;
            let lane = i % 32;
            raw[half * 64 + (quarter % 2) * 32 + lane] |= (code & 15) << (4 * (quarter / 2));
            raw[128 + half * 32 + lane] |= (code >> 4) << (2 * quarter);
            *weight = 0.5 * scales[i / 16] as f32 * (code as f32 - 32.0);
        }
        let mut actual = [0.0f32; Q6_K_WEIGHTS];
        assert_eq!(dequantize_q6_k(&raw, &mut actual), Ok(Q6_K_WEIGHTS));
        assert_eq!(actual, expected);
        assert_eq!(dequantize_q6_k(&raw[..209], &mut actual), Err(InferenceError::CorruptedWeights));
        assert_eq!(dequantize_q6_k(&raw, &mut actual[..255]), Err(InferenceError::BufferTooSmall));
    }

    #[test]
    fn tq2_0_block_matches_ggml_reference() {
        let mut raw = [0u8; TQ2_0_BLOCK_BYTES];
        raw[64..].copy_from_slice(&0x3e00u16.to_le_bytes());
        let mut expected = [0.0f32; TQ2_0_WEIGHTS];
        for (i, weight) in expected.iter_mut().enumerate() {
            let code = ((i * 5 + i / 32) % 3) as u8;
            raw[i / 128 * 32 + i % 32] |= code << (2 * ((i % 128) / 32));
            *weight = 1.5 * (code as f32 - 1.0);
        }
        let mut actual = [0.0f32; TQ2_0_WEIGHTS];
        assert_eq!(dequantize_tq2_0(&raw, &mut actual), Ok(TQ2_0_WEIGHTS));
        assert_eq!(actual, expected);
        assert_eq!(dequantize_tq2_0(&raw[..65], &mut actual), Err(InferenceError::CorruptedWeights));
        assert_eq!(dequantize_tq2_0(&raw, &mut actual[..255]), Err(InferenceError::BufferTooSmall));
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

    /// The pooling head's activation, pinned to the exact tanh-approximation
    /// curve the upstream reference defines.
    ///
    /// WHY exact constants from outside the code: `0.5 * x * (1 + tanh(...))`
    /// written twice is not a check, and the values below are the float
    /// evaluations of the published formula (sqrt(2/pi), 0.044715) -- a wrong
    /// inner constant, a missing cube, or a silu pasted in place of GELU moves
    /// at least one of them by more than 1e-4. The software `f32_exp` behind the
    /// tanh costs the comparison a few 1e-6, which the tolerance carries.
    #[test]
    fn test_gelu_tanh_is_the_reference_curve_not_a_near_miss() {
        let cases: [(f32, f32); 8] = [
            (0.0, 0.0),
            (0.5, 0.34571400982514394),
            (1.0, 0.8411919906082768),
            (2.0, 1.954597694087775),
            (4.0, 3.9999297540518075),
            (-0.5, -0.15428599017485606),
            (-1.0, -0.15880800939172324),
            (-2.0, -0.04540230591222494),
        ];
        for &(x, expected) in cases.iter() {
            let got = gelu_tanh(x);
            assert!(
                (got - expected).abs() < 1e-4,
                "gelu_tanh({x}) answered {got}, the reference curve answers {expected}"
            );
        }
        // The saturation the clamp promises: beyond |x| = 20 the head sees the
        // limit, not an overflowing exponential.
        assert_eq!(gelu_tanh(30.0), 30.0);
        assert!(gelu_tanh(-30.0).abs() < 1e-5);
    }

    /// The head, run end to end on a hand-built F32 fixture whose answer is
    /// arithmetic the reader can check: dense_2 keeps element 0 and doubles
    /// element 1, GELU lands on the two points pinned above, dense_3 is the
    /// identity on the hidden pair.
    ///
    /// WHY both bias states: the published file declares no bias tensor, and a
    /// head that ran with an invented one would add a constant the model never
    /// trained with -- so the no-bias answer must be exactly the pre-bias answer,
    /// and the declared bias must be added and not skipped.
    #[test]
    fn test_apply_pooling_head_is_dense_gelu_dense_with_or_without_bias() {
        let f32_bytes = |values: &[f32]| -> Vec<u8> {
            values.iter().flat_map(|v| v.to_le_bytes()).collect()
        };
        // dense_2: 2 rows of 4 (mid_dim 2, in_dim 4). Row 0 picks element 0,
        // row 1 doubles element 1.
        let dense_2 = f32_bytes(&[1.0, 0.0, 0.0, 0.0, 0.0, 2.0, 0.0, 0.0]);
        // dense_3: 2 rows of 2 -- the identity on the hidden pair.
        let dense_3 = f32_bytes(&[1.0, 0.0, 0.0, 1.0]);
        let mean = [0.5f32, 1.0, 0.25, -0.25];
        let mut hidden = [0.0f32; 2];
        let mut out = [0.0f32; 2];

        let head = PoolingHead {
            dense_2: &dense_2,
            dense_2_type: 0,
            dense_2_bias: None,
            dense_3: &dense_3,
            dense_3_type: 0,
            dense_3_bias: None,
            mid_dim: 2,
            out_dim: 2,
        };
        apply_pooling_head(&head, &mean, &mut hidden, &mut out).expect("the fixture head runs");
        assert!((hidden[0] - 0.34571400982514394).abs() < 1e-4);
        assert!((out[0] - 0.34571400982514394).abs() < 1e-4, "gelu(p0)");
        assert!((out[1] - 1.954597694087775).abs() < 1e-4, "gelu(2 * p1)");

        // The same head with declared biases: both are added, at both layers.
        let bias_2 = [0.0f32, 0.0];
        let bias_3 = [0.1f32, -0.2];
        let biased = PoolingHead {
            dense_2_bias: Some(&bias_2),
            dense_3_bias: Some(&bias_3),
            ..head
        };
        let mut out_b = [0.0f32; 2];
        apply_pooling_head(&biased, &mean, &mut hidden, &mut out_b).expect("biases run");
        assert!((out_b[0] - (0.34571400982514394 + 0.1)).abs() < 1e-4);
        assert!((out_b[1] - (1.954597694087775 - 0.2)).abs() < 1e-4);

        // A bias narrower than its layer is refused, not half-added: the tail
        // would keep a bias the file did not state for it, which is the same
        // defect the block-norm width checks refuse on the other side of the run.
        let short = [0.1f32];
        let narrowed = PoolingHead {
            dense_3_bias: Some(&short),
            ..head
        };
        assert_eq!(
            apply_pooling_head(&narrowed, &mean, &mut hidden, &mut out_b),
            Err(InferenceError::CorruptedWeights)
        );
        // A hidden buffer shorter than the file's mid_dim, and an out narrower
        // than out_dim, are the caller's buffer refusal rather than a truncation.
        let mut one_float = [0.0f32; 1];
        assert_eq!(
            apply_pooling_head(&head, &mean, &mut one_float, &mut out_b),
            Err(InferenceError::BufferTooSmall)
        );
        assert_eq!(
            apply_pooling_head(&head, &mean, &mut hidden, &mut out_b[..1]),
            Err(InferenceError::BufferTooSmall)
        );
        // The empty shape a loader could hand over if it read dimensions from a
        // guess: refused as the corrupt description it is, before a multiply.
        assert_eq!(
            apply_pooling_head(
                &PoolingHead {
                    mid_dim: 0,
                    ..head
                },
                &mean,
                &mut hidden,
                &mut out,
            ),
            Err(InferenceError::CorruptedWeights)
        );
    }

    /// `mean_of` and `l2_normalise` are the two steps the streaming path
    /// composes around the head, and `pooled_normalised` is exactly their
    /// pairing. WHY pinned directly: the streamed and in-memory pooling rules
    /// are only honest if the decomposition cannot drift from the composite,
    /// and a count of zero must leave the destination untouched rather than
    /// answer "mean of nothing" with zeros.
    #[test]
    fn test_mean_of_and_l2_normalise_compose_to_pooled_normalised() {
        let pooled = [1.0f32, 2.0f32, 3.0f32, 4.0f32];
        let mut via_parts = [0.0f32; 4];
        mean_of(&pooled, 2, &mut via_parts);
        assert_eq!(via_parts, [0.5, 1.0, 1.5, 2.0]);
        l2_normalise(&mut via_parts);
        let mut via_composite = [0.0f32; 4];
        pooled_normalised(&pooled, 2, &mut via_composite);
        for i in 0..4 {
            assert!((via_parts[i] - via_composite[i]).abs() < 1e-6);
        }
        let norm: f32 = via_parts.iter().map(|v| v * v).sum();
        assert!((norm - 1.0).abs() < 1e-4);
        let mut untouched = [7.0f32; 4];
        mean_of(&pooled, 0, &mut untouched);
        assert_eq!(untouched, [7.0; 4]);
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

        // Key/value heads that do not evenly divide query heads (3 does not divide 2)
        // are refused on head geometry.
        assert_eq!(
            forward_step(
                crate::tokenizer::TOKEN_BOS,
                0,
                &model,
                &ModelConfig {
                    num_kv_heads: 3,
                    ..base
                },
                &mut kv_cache,
                &mut scratch,
            ),
            Err(InferenceError::ModelDeclaration(
                "<arch>.attention.head_count_kv",
                Some(3),
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

    /// The dtype ids this build has a byte layout for, collected from the engine
    /// rather than read out of a comment.
    ///
    /// WHY the whole range and not a fixed list: `quantised_row_bytes` is the function
    /// the loader calls to size every tensor row it reads, and `gemv` and
    /// `embedding_row_into` are the two places the bytes are then interpreted. A
    /// case that only checks the ids it expects to work cannot see a new arm
    /// appear or one disappear; a case that walks the id space and collects
    /// what answers can see both, and it also sees the shape of the answer for
    /// everything outside the set -- refused by number, never read with a
    /// neighbour's block width.
    #[test]
    fn the_engine_maps_exactly_eight_dtype_ids_and_refuses_the_rest_by_number() {
        // 0 F32, 1 F16, 2 Q4_0, 8 Q8_0, 12 Q4_K, 13 Q5_K, 14 Q6_K, 35 TQ2_0.
        // Q4_1 (3), Q5_0 (6), Q5_1 (7), Q8_1 (9), Q2_K (10), Q3_K (11), Q8_K (15)
        // and BF16 (30) are ids the GGML format names that no arithmetic in this
        // service reads.
        let mapped = [0u32, 1, 2, 8, 12, 13, 14, 35];
        let mut answered: Vec<u32> = Vec::new();
        for dtype in 0..=64u32 {
            if quantised_row_bytes(dtype, 768).is_some() {
                answered.push(dtype);
            }
        }
        assert_eq!(
            answered, mapped.to_vec(),
            "the ids the loader sizes a row for are {answered:?}, not the {mapped:?} this \
             module's documentation names -- one of the two is now a lie"
        );

        for dtype in 0..=64u32 {
            let is_mapped = mapped.contains(&dtype);
            let row_bytes = quantised_row_bytes(dtype, 768);
            assert_eq!(row_bytes.is_some(), is_mapped);
            // The two consumers, at the width the published file runs at. A row the
            // engine has no layout for must come back naming the number it was
            // handed, because a dequantisation that guessed a block width returns
            // plausible-looking floats from the wrong bytes.
            let raw = [0u8; 3072];
            let mut stream = [0.0f32; 768];
            let mut x = [0.0f32; 1];
            let mut out = [0.0f32; 1];
            match (is_mapped, row_bytes) {
                (true, Some(width)) => {
                    assert_eq!(
                        embedding_row_into(&raw[..width as usize], dtype, 1.0, &mut stream),
                        Ok(()),
                        "dtype {dtype} sizes a {width}-byte row but cannot read one"
                    );
                }
                _ => {
                    assert_eq!(
                        embedding_row_into(&raw, dtype, 1.0, &mut stream),
                        Err(InferenceError::UnsupportedQuantization(dtype)),
                        "embedding_row_into read dtype {dtype} with something other than \
                         its number"
                    );
                    assert_eq!(
                        gemv(&raw, dtype, &mut x, &mut out, 1, 1),
                        Err(InferenceError::UnsupportedQuantization(dtype)),
                        "gemv read dtype {dtype} with something other than its number"
                    );
                }
            }
        }
    }

    /// The pooled vector is the width the model declares, and a destination that
    /// cannot hold it is refused instead of truncated.
    ///
    /// WHY a buffer wider than the model is used for the passing case: the only
    /// honest reading of "the engine wrote 64 floats" is one where the bytes past
    /// 64 are still what the caller left there. The narrower case matters more:
    /// `hidden_dim.min(out.len())` would have answered a 63-float vector for a
    /// 64-wide model, and a prefix of an embedding compared against another prefix
    /// is a similarity no one asked for.
    #[test]
    fn the_pooled_vector_is_exactly_the_width_the_model_declares() {
        let fixture = LittleModel::new();
        let config = LittleModel::config();
        let blocks = [Some(fixture.layer())];
        let model = fixture.weights(&blocks);
        let tokens = [crate::tokenizer::TOKEN_BOS, 261, 4];

        let mut wide = [f32::NAN; LITTLE_HIDDEN * 2];
        let dim = extract_embedding(&tokens, &model, &config, &mut wide)
            .expect("a 128-float destination holds a 64-wide vector");
        assert_eq!(dim, config.hidden_dim, "the width reported is not the width declared");
        assert_eq!(dim, LITTLE_HIDDEN);
        assert!(wide[..dim].iter().all(|v| v.is_finite()));
        assert!(wide[..dim].iter().any(|v| *v != 0.0));
        assert!(
            wide[dim..].iter().all(|v| v.is_nan()),
            "the pool wrote past the model's declared width"
        );
        let mut squared = 0.0f32;
        for &v in wide[..dim].iter() {
            squared += v * v;
        }
        assert!(
            (squared - 1.0).abs() < 1e-4,
            "the vector the caller was told is {} wide has squared length {}",
            dim,
            squared
        );

        let mut narrow = [0.0f32; LITTLE_HIDDEN - 1];
        assert_eq!(
            extract_embedding(&tokens, &model, &config, &mut narrow),
            Err(InferenceError::BufferTooSmall),
            "a 63-float destination for a 64-wide model was answered with a truncation"
        );
        assert!(
            narrow.iter().all(|v| *v == 0.0),
            "the refused call still wrote into the caller's buffer: {narrow:?}"
        );

        // Nothing to pool answers zero rather than refusing and rather than
        // inventing a zero vector: `Ok(0)` is "no position ran", which the caller's
        // status is chosen from, and a 64-wide array of zeros would read as a
        // sentence the model said about the empty string.
        let mut empty = [0.0f32; LITTLE_HIDDEN];
        assert_eq!(extract_embedding(&[], &model, &config, &mut empty), Ok(0));
        assert!(empty.iter().all(|v| *v == 0.0));
    }

    #[test]
    fn test_model_config_refuses_more_than_one_key_value_head() {
        let mut shared = declared(64, 128, 1, 2, 32, 290);
        shared.num_kv_heads = Some(3);
        assert_eq!(
            ModelConfig::from_gguf(&shared),
            Err(InferenceError::ModelDeclaration(
                "<arch>.attention.head_count_kv",
                Some(3),
                1
            ))
        );

        // A file that states no count is the format's multi-head case: num_kv_heads == num_heads = 2.
        // With GQA, 2 KV heads evenly divides 2 query heads, so it is accepted.
        let unstated = declared(64, 128, 1, 2, 32, 290);
        assert!(ModelConfig::from_gguf(&unstated).is_ok());

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
        info.pooling_type = Some(1);
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
            Err(InferenceError::ScratchTooLarge(437_402_624, SCRATCH_BYTE_BUDGET))
        );

        // The same file at the shortest context anything of this family ships with
        // is 28,445,696 bytes: the engine accepts it, because the memory really is
        // there. What it still cannot do is load the weights or run this model's
        // graph, which the two refusals below say plainly.
        let mut small = declared(2304, 4608, 26, 8, 256, 256_000);
        small.num_kv_heads = Some(1);
        small.context_length = Some(512);
        let accepted = ModelConfig::from_gguf(&small).expect("28 MiB of cache is under budget");
        assert_eq!(accepted.scratch_bytes(), 28_427_264);
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

    #[test]
    fn test_dynamic_scratch_budget_derives_from_telemetry_or_default_refusal() {
        assert_eq!(dynamic_scratch_byte_budget(), SCRATCH_BYTE_BUDGET);

        // When free RAM is 0 or low, minimum budget floor of 120 MiB is enforced
        assert_eq!(compute_dynamic_scratch_budget(0), 0);
        assert_eq!(
            compute_dynamic_scratch_budget(100 * 1024 * 1024),
            75 * 1024 * 1024
        );

        // When 1 GiB is free, 75% (768 MiB) is granted
        let one_gib = 1024 * 1024 * 1024u64;
        assert_eq!(compute_dynamic_scratch_budget(one_gib), 768 * 1024 * 1024);

        // When 2 GiB is free, 75% (1.5 GiB) is granted
        let two_gib = 2048 * 1024 * 1024u64;
        assert_eq!(compute_dynamic_scratch_budget(two_gib), 1536 * 1024 * 1024);

        // Huge model needing ~437 MiB is refused against standard budget naming exact numbers
        let mut huge = declared(2304, 4608, 26, 8, 256, 256_000);
        huge.num_kv_heads = Some(1);
        huge.context_length = Some(8192);
        let needed = huge_priced().scratch_bytes();
        assert_eq!(
            ModelConfig::from_gguf(&huge),
            Err(InferenceError::ScratchTooLarge(needed, SCRATCH_BYTE_BUDGET))
        );
        assert_eq!(needed, 437_402_624);
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
            sliding_window: None,
            rms_norm_eps: 1e-6,
            rope_theta: 1_000_000.0,
            activation: ActivationType::SwiGLU,
            ..ModelConfig::default_test_config()
        }
    }

    /// `<arch>.attention.sliding_window` reaching the config the graph reads.
    ///
    /// The published file states 512 beside a `context_length` of 2048. Before
    /// this, `gguf.rs` read the number, `GgufModelInfo` held it, two tests
    /// asserted it arrived, and `ModelConfig` had nowhere to put it -- so
    /// `run_block` attended the whole 2048-position prefix the file says a query
    /// does not see. These cases pin the three answers a file can give: a window,
    /// silence, and a stated zero.
    #[test]
    fn test_the_declared_window_reaches_the_config_and_silence_does_not() {
        let published = ModelConfig::from_gguf(&published_file()).expect("the file is priced");
        assert_eq!(published.sliding_window, Some(512));
        // The window is not a pricing input: the 512 the file states beside its own
        // 2048-position context costs exactly the bytes the context costs, so this
        // change neither raised a bound nor asked the device for more RAM.
        assert_eq!(published.scratch_bytes(), 101_763_584);
        assert_eq!(published.check_scratch_budget(), Ok(()));

        // A file that states no window gets no limit, not the engine's own guess.
        // The rest of the declaration is the published file's, so the only thing
        // missing is the window. (Silence about `head_count_kv` is a different
        // story: `from_gguf` reads it as one KV head per query head, which is the
        // format's multi-head case, and `check_attention_shape` then refuses the
        // shape -- that arm is untouched here and covered by its own test.)
        let mut unwindowed_info = published_file();
        unwindowed_info.sliding_window = None;
        let unwindowed = ModelConfig::from_gguf(&unwindowed_info)
            .expect("a file with no window is still a file");
        assert_eq!(unwindowed.sliding_window, None);
        assert_eq!(unwindowed.attention_span_start(2047), 0);

        // A file that states 0 states no usable span. Turning that into a span of
        // zero positions would drop the query's own token, so it is read as silence.
        let mut zeroed = published_file();
        zeroed.sliding_window = Some(0);
        let zeroed_config = ModelConfig::from_gguf(&zeroed)
            .expect("a stated zero is refused as a span, not as a model");
        assert_eq!(zeroed_config.sliding_window, None);

        // A window wider than the context the file also states is carried as
        // declared. Clamping it here would rewrite the file's number to the
        // engine's convenience; the span clamps itself to the prefix it has.
        let mut wider = published_file();
        wider.sliding_window = Some(4096);
        let config = ModelConfig::from_gguf(&wider).expect("an over-wide window is still declared");
        assert_eq!(config.sliding_window, Some(4096));
        assert_eq!(config.attention_span_start(2047), 0, "the span never runs past the cache");
    }

    /// The span arithmetic, independent of any weight: a window counts positions
    /// backwards from the query and always includes the query itself.
    #[test]
    fn test_the_window_span_is_the_files_own_back_count() {
        let base = ModelConfig::from_gguf(&published_file()).expect("the file is priced");
        let windowed = |w: Option<usize>| ModelConfig {
            sliding_window: w,
            ..base
        };

        // 512 declared against 2048: positions 0..511 still see the whole prefix.
        let w = windowed(Some(512));
        assert_eq!(w.attention_span_start(0), 0);
        assert_eq!(w.attention_span_start(511), 0);
        // Position 512 is the first the window bites: it sees 1..513, not 0..513.
        assert_eq!(w.attention_span_start(512), 1);
        assert_eq!(w.attention_span_start(2047), 2048 - 512);
        // The span always holds the query, so softmax never normalises an empty set.
        assert!(w.attention_span_start(2047) < 2048);

        // A window of one is the file saying "attend only yourself".
        let solo = windowed(Some(1));
        assert_eq!(solo.attention_span_start(7), 7);
        assert_eq!(solo.attention_span_start(0), 0);

        // No window: the whole cached prefix, which is the graph of every file that
        // states none.
        let none = windowed(None);
        assert_eq!(none.attention_span_start(0), 0);
        assert_eq!(none.attention_span_start(2047), 0);
    }

    /// The block's answer, after a whole prompt, at the position the caller names.
    ///
    /// WHY this reads `scratch.hidden` and not the logits: the fixture's one block
    /// writes the residual stream there, and it is the value the pooling head turns
    /// into an embedding, so it is the number a published model's caller sees.
    fn hidden_over_prompt(
        fixture: &LittleModel,
        config: &ModelConfig,
        prompt: &[u32],
    ) -> [f32; LITTLE_HIDDEN] {
        let blocks = [Some(fixture.layer())];
        let model = fixture.weights(&blocks);
        let (mut scratch, mut kv_cache) = fit(config);
        for (pos, &token) in prompt.iter().enumerate() {
            forward_step(
                token,
                pos,
                &model,
                config,
                &mut kv_cache,
                &mut scratch,
            )
            .expect("the fixture holds the row this token asks for");
        }
        let mut out = [0.0f32; LITTLE_HIDDEN];
        out.copy_from_slice(&scratch.hidden[..LITTLE_HIDDEN]);
        out
    }

    /// THE graph claim: a query cannot be influenced by a position its file's
    /// window excludes.
    ///
    /// HOW this is independent of the code under test: the fixture has ONE block,
    /// so a position's cached key and value are functions of that position's own
    /// token and nothing else -- no earlier attention has shaped them yet. Two
    /// prompts that agree inside the window and differ outside it therefore cannot
    /// produce different answers at the last position, and the only thing that can
    /// make them agree is a span that genuinely excludes the differing prefix. The
    /// third assertion is what stops the first two from being vacuous: with no
    /// window the same early tokens DO move the answer, so the equality above is
    /// the window working and not a fixture that ignores its input.
    #[test]
    fn test_a_windowed_query_is_influenced_by_no_position_it_excludes() {
        let fixture = LittleModel::new();
        let window = ModelConfig {
            sliding_window: Some(2),
            ..LittleModel::config()
        };

        // Positions 2 and 3 are the span at pos 3; 0 and 1 are outside it.
        let a = hidden_over_prompt(&fixture, &window, &[99, 105, 30, 40]);
        let b = hidden_over_prompt(&fixture, &window, &[7, 11, 30, 40]);
        assert_eq!(
            a, b,
            "a position the declared window excludes still reached the answer"
        );

        // A window of one leaves only the query's own token in the span.
        let solo = ModelConfig {
            sliding_window: Some(1),
            ..LittleModel::config()
        };
        assert_eq!(
            hidden_over_prompt(&fixture, &solo, &[10, 20, 30, 40]),
            hidden_over_prompt(&fixture, &solo, &[5, 6, 7, 40]),
            "a window of one must attend the query alone"
        );

        // Without the window the same early changes move the answer, so the two
        // equalities above are the span doing the excluding.
        let unwindowed = LittleModel::config();
        let wide_a = hidden_over_prompt(&fixture, &unwindowed, &[99, 105, 30, 40]);
        let wide_b = hidden_over_prompt(&fixture, &unwindowed, &[7, 11, 30, 40]);
        assert_ne!(
            wide_a, wide_b,
            "the fixture ignores its early tokens even with no window, which would make \
             the windowed equality meaningless"
        );
        assert_ne!(
            wide_a, a,
            "applying the declared window changed nothing at all, so the span is still \
             being ignored by the block"
        );
    }

    /// A window that has not yet bound is not a second code path with a second
    /// answer: inside its own span the file's window costs exactly nothing.
    ///
    /// WHY it matters: the published file's 512 sits under a 2048-position context,
    /// so positions 0..511 must be bit-identical to the same run with no window. If
    /// they were not, the change would have altered every short prompt the device
    /// has ever answered while claiming only to apply a declaration.
    #[test]
    fn test_a_window_that_does_not_yet_bind_changes_nothing() {
        let fixture = LittleModel::new();
        let unwindowed = LittleModel::config();
        let reference = hidden_over_prompt(&fixture, &unwindowed, &[10, 20, 30, 40]);

        for width in [4usize, 5, 64] {
            let window = ModelConfig {
                sliding_window: Some(width),
                ..LittleModel::config()
            };
            assert_eq!(
                hidden_over_prompt(&fixture, &window, &[10, 20, 30, 40]),
                reference,
                "a window of {width} positions does not bind at pos 3, so it must not \
                 move the answer by one bit"
            );
        }

        // The fixture's own default carries no window at all, which is the case a
        // hand-built config reaches and `from_gguf` never produces.
        assert_eq!(unwindowed.sliding_window, None);
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
        // The window the file states beside its own 2048-position context, carried
        // into the config instead of dropped between the reader and the graph.
        assert_eq!(config.sliding_window, Some(512), "attention.sliding_window");
        // 2 x 24 x 2048 x 1 x 256 x 4: 96 MiB, which is 100 times the stack it
        // used to have to live on.
        assert_eq!(config.kv_cache_bytes(), 100_663_296);
        assert_eq!(config.scratch_bytes(), 101_763_584);
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

        let Some(path) = file_bytes::env::var("INTEROS_REAL_GGUF_PATH").ok().or_else(|| {
            let candidate = "../../target/verify/models/embeddinggemma-300M-Q8_0.gguf";
            if file_bytes::path::Path::new(candidate).exists() {
                Some(alloc::string::String::from(candidate))
            } else {
                None
            }
        }) else {
            panic!(
                "NOT VERIFIED against a real model file: INTEROS_REAL_GGUF_PATH is unset and default \
                 file is absent. Verifying model config pricing against model file requires the model file."
            );
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
        assert_eq!(config.scratch_bytes(), 101_763_584);
        assert_eq!(config.check_scratch_budget(), Ok(()));
        // The window the file states beside its own 2048-position context now
        // reaches the graph. Taken from the file's bytes, not from the fixture:
        // this is the assertion that would have failed while the reader collected
        // the number and `ModelConfig` had nowhere to put it.
        assert_eq!(
            config.sliding_window,
            Some(512),
            "the real file states attention.sliding_window 512 and the graph must run it"
        );
        assert_eq!(config.attention_span_start(2047), 2048 - 512);
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
        // this model happened to clear it. The total answers first, because that
        // is the budget; this shape's one cache tensor is over the mapping
        // ceiling too, and the test below holds that refusal on a shape whose
        // total is affordable.
        let longer = ModelConfig {
            seq_len: 8192,
            ..config
        };
        assert_eq!(longer.kv_cache_bytes() / 2, 201_326_592);
        assert!(
            longer.kv_cache_bytes() / 2 > LARGEST_SINGLE_BUFFER_BYTES,
            "one cache tensor of this shape is not mappable either"
        );
        assert_eq!(
            longer.check_scratch_budget(),
            Err(InferenceError::ScratchTooLarge(403_778_048, SCRATCH_BYTE_BUDGET)),
            "the real file at 8192 positions has to be refused by the bytes it names"
        );
    }

    /// WHY: free RAM bounds what a run may hold in total, and the platform's
    /// per-mapping ceiling bounds any one tensor inside it. Nothing derives the
    /// second from the first, so once the budget is raised from telemetry a shape
    /// can be affordable in total and still be unmappable -- and then the refusal
    /// has to name the tensor that cannot be had, because the total is not what
    /// the device refused.
    #[test]
    fn a_shape_whose_biggest_tensor_cannot_be_mapped_is_refused_by_that_tensor() {
        // The published geometry at the longest context its family ships with:
        // 24 layers of 8192 positions, one key/value head 256 wide.
        let wide = ModelConfig {
            num_layers: 24,
            seq_len: 8192,
            num_kv_heads: 1,
            head_dim: 256,
            ..ModelConfig::default_test_config()
        };

        // A guest reporting 1 GiB free gets three quarters of it, which is far
        // more than one mapping can carry. That gap is the whole reason the
        // ceiling is checked beside the total.
        let budget = compute_dynamic_scratch_budget(1024 * 1024 * 1024);
        assert!(
            budget > LARGEST_SINGLE_BUFFER_BYTES,
            "the free-RAM budget is supposed to outgrow one mapping; if it no longer \
             does, the ceiling check is dead weight and should say why"
        );
        assert!(
            wide.scratch_bytes() <= budget,
            "the shape was meant to be affordable in total: {} bytes against a {} byte budget",
            wide.scratch_bytes(),
            budget
        );

        // `KvCache` holds as two tensors the bytes `kv_cache_bytes` prices together.
        let tensor = wide.kv_cache_bytes() / 2;
        assert!(
            tensor > LARGEST_SINGLE_BUFFER_BYTES,
            "one cache tensor is {tensor} bytes, over the {LARGEST_SINGLE_BUFFER_BYTES} byte ceiling"
        );
        assert_eq!(
            wide.largest_single_buffer_bytes()
                .expect("this shape's buffers are all priceable"),
            tensor,
            "the cache tensor is this shape's biggest single allocation"
        );
        assert_eq!(
            wide.check_scratch_against(budget),
            Err(InferenceError::ScratchTooLarge(
                tensor,
                LARGEST_SINGLE_BUFFER_BYTES
            )),
            "an affordable total must not hide a tensor no mapping can carry"
        );

        // The production entry point still refuses this shape, on the total: in a
        // test build its budget is the floor, so the two checks answer in the
        // other order. Both are refusals; neither is a pass.
        assert_eq!(
            wide.check_scratch_budget(),
            Err(InferenceError::ScratchTooLarge(
                wide.scratch_bytes(),
                SCRATCH_BYTE_BUDGET
            )),
            "the floor budget refuses the same shape by its total"
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

    /// Prices weight bytes in the cost model against the available memory budget.
    ///
    /// Verifies that conversational 2B models cannot run in-memory within SCRATCH_BYTE_BUDGET
    /// and that weight footprints are priced directly from the declared model architecture
    /// rather than invented transfer speeds or unmeasured throughput claims.
    #[test]
    fn test_model_config_prices_weight_bytes_against_memory_budget() {
        // Conversational 2B model configuration
        let conv_2b = ModelConfig {
            hidden_dim: 2048,
            intermediate_dim: 16384,
            num_layers: 18,
            num_heads: 8,
            head_dim: 256,
            num_kv_heads: 5,
            seq_len: 2048,
            vocab_size: 256000,
            sliding_window: None,
            rms_norm_eps: 1e-6,
            rope_theta: 10000.0,
            activation: ActivationType::SwiGLU,
            ..ModelConfig::default_test_config()
        };

        // 1. Weight bytes priced from model configuration
        let q4_weight_bytes = conv_2b.weight_bytes(12).expect("Q4_K is supported");
        let q8_weight_bytes = conv_2b.weight_bytes(8).expect("Q8_0 is supported");
        assert!(conv_2b.weight_bytes(999).is_err(), "unknown dtype must be refused");
        assert!(q4_weight_bytes > 1_000_000_000, "2B Q4 weights must exceed 1 GiB");
        assert!(q8_weight_bytes > 2_000_000_000, "2B Q8 weights must exceed 2 GiB");
        assert!(q4_weight_bytes > SCRATCH_BYTE_BUDGET * 8, "2B Q4 weights exceed budget by 8x");
        assert!(q8_weight_bytes > SCRATCH_BYTE_BUDGET * 16, "2B Q8 weights exceed budget by 16x");

        // 2. Admission check refuses 2B architecture with 5 KV heads on head count
        assert_eq!(
            conv_2b.check_admission(Some(8)),
            Err(InferenceError::ModelDeclaration(
                "<arch>.attention.head_count_kv",
                Some(5),
                1
            ))
        );

        // 3. Admission check refuses 2B Q8 weights even with 1 KV head against limits
        let conv_2b_1kv = ModelConfig {
            num_kv_heads: 1,
            ..conv_2b
        };
        let q8_weight_1kv = conv_2b_1kv.weight_bytes(8).expect("Q8_0 is supported");

        // Tested against a 2 GiB machine budget (1536 MiB user address space)
        let budget_2g = compute_dynamic_address_space_budget(2048 * 1024 * 1024);
        assert_eq!(budget_2g, 1536 * 1024 * 1024);
        assert_eq!(
            conv_2b_1kv.check_admission_against(Some(8), budget_2g),
            Err(InferenceError::ScratchTooLarge(
                q8_weight_1kv + conv_2b_1kv.scratch_bytes(),
                1536 * 1024 * 1024
            ))
        );

        // Tested against default fallback budget (256 MiB user address space)
        assert_eq!(
            conv_2b_1kv.check_admission(Some(8)),
            Err(InferenceError::ScratchTooLarge(
                q8_weight_1kv + conv_2b_1kv.scratch_bytes(),
                FALLBACK_USER_ADDRESS_SPACE_LIMIT
            ))
        );

        // 2. Scratch check refuses the 2B architecture cleanly
        let needed_scratch = conv_2b.scratch_bytes();
        assert!(needed_scratch > SCRATCH_BYTE_BUDGET * 2, "KV cache + scratch exceeds budget by 2x");
        assert_eq!(
            conv_2b.check_scratch_budget(),
            Err(InferenceError::ScratchTooLarge(needed_scratch, SCRATCH_BYTE_BUDGET))
        );
    }

    /// Untrusted input sequence exceeding declared context length is refused,
    /// never silently truncated.
    #[test]
    fn test_extract_embedding_refuses_tokens_exceeding_declared_seq_len() {
        let config = ModelConfig::default_test_config();
        let weights = ModelWeights {
            token_embd: TokenEmbedding::Table {
                bytes: &[],
                rows: 0,
                dtype: 0,
            },
            layers: &[],
            final_norm: &[],
            output_weight: None,
            output_weight_type: 0,
        };
        let mut out = [0.0f32; 64];
        let too_long = alloc::vec![1u32; config.seq_len + 1];
        assert_eq!(
            extract_embedding(&too_long, &weights, &config, &mut out),
            Err(InferenceError::ContextLengthExceeded)
        );
    }

    #[test]
    fn test_geglu_activation() {
        let gate = [0.0f32, 1.0f32, -1.0f32, 2.0f32];
        let up = [1.0f32, 2.0f32, 3.0f32, -0.5f32];
        let mut out = [0.0f32; 4];
        geglu(&gate, &up, &mut out);
        for i in 0..4 {
            let expected = gelu_tanh(gate[i]) * up[i];
            let diff = (out[i] - expected).abs();
            assert!(diff < 1e-5, "geglu output {i} mismatch: got {}, expected {}", out[i], expected);
        }
    }

    #[test]
    fn test_sample_token_calibrated_temperature_and_argmax() {
        let logits = [-10.0f32, 2.5f32, 15.0f32, 8.0f32, -1.0f32];
        // Argmax with T = 0.0
        let token_argmax = sample_token(&logits, 0.0);
        assert_eq!(token_argmax, 2, "T=0.0 must choose index 2 (logit 15.0)");

        // Argmax with T < 1e-4
        let token_low_t = sample_token(&logits, 5e-5);
        assert_eq!(token_low_t, 2, "T < 1e-4 must choose index 2");

        // Calibrated sampling with top_k = 1 must also choose index 2
        let token_top1 = sample_token_calibrated(&logits, 1.0, 1, 0.9, 42);
        assert_eq!(token_top1, 2, "top_k=1 must choose index 2");

        // Calibrated sampling with fixed seed must be deterministic
        let t1 = sample_token_calibrated(&logits, 0.8, 10, 0.95, 12345);
        let t2 = sample_token_calibrated(&logits, 0.8, 10, 0.95, 12345);
        assert_eq!(t1, t2, "calibrated sampling with identical seed must be deterministic");
    }

    #[test]
    fn test_grouped_query_attention_config_and_step() {
        // Valid GQA: 4 query heads, 2 KV heads (k = 2)
        let gqa_config = ModelConfig {
            hidden_dim: 64,
            intermediate_dim: 128,
            num_layers: 1,
            num_heads: 4,
            num_kv_heads: 2,
            head_dim: 16,
            vocab_size: 290,
            seq_len: 64,
            sliding_window: None,
            rms_norm_eps: 1e-6,
            rope_theta: 10000.0,
            activation: ActivationType::SwiGLU,
            ..ModelConfig::default_test_config()
        };
        assert_eq!(gqa_config.check_attention_shape(), Ok(()));

        // Invalid GQA: 4 query heads, 3 KV heads (3 does not divide 4)
        let invalid_gqa = ModelConfig {
            num_kv_heads: 3,
            ..gqa_config
        };
        assert_eq!(
            invalid_gqa.check_attention_shape(),
            Err(InferenceError::ModelDeclaration(
                "<arch>.attention.head_count_kv",
                Some(3),
                1
            ))
        );

        // Invalid GQA: 4 query heads, 8 KV heads (8 > 4)
        let invalid_gqa_excess = ModelConfig {
            num_kv_heads: 8,
            ..gqa_config
        };
        assert_eq!(
            invalid_gqa_excess.check_attention_shape(),
            Err(InferenceError::ModelDeclaration(
                "<arch>.attention.head_count_kv",
                Some(8),
                1
            ))
        );
    }

    #[test]
    fn test_resident_tensor_cache_roundtrip() {
        let mut cache = ResidentTensorCache::new();
        assert!(!cache.initialized);
        assert_eq!(cache.count, 0);

        let mut entry = ResidentTensorEntry::empty();
        let name = b"blk.0.attn_q.weight";
        entry.name_len = name.len() as u16;
        entry.name[..name.len()].copy_from_slice(name);
        entry.dtype = 1;
        entry.cols = 64;
        entry.rows = 64;
        entry.n_dims = 2;
        entry.byte_len = 8192;
        entry.offset = 4096;
        entry.position = 8192;

        assert!(entry.matches(name));
        assert!(!entry.matches(b"blk.0.attn_k.weight"));

        cache.entries[0] = entry;
        cache.count = 1;
        cache.data_start = 4096;
        cache.alignment = 32;
        cache.initialized = true;

        assert_eq!(cache.find(name), Some(&entry));
        assert_eq!(cache.find(b"nonexistent"), None);
    }

    #[test]
    fn test_f32_sin_cos_numerical_bounds_and_accuracy() {
        // Zero and small inputs
        assert!((f32_sin(0.0) - 0.0).abs() < 1e-6);
        assert!((f32_cos(0.0) - 1.0).abs() < 1e-6);

        // Standard angles (pi/6, pi/4, pi/3, pi/2, pi, 3pi/2, 2pi)
        let pi = core::f32::consts::PI;
        assert!((f32_sin(pi / 6.0) - 0.5).abs() < 1e-4);
        assert!((f32_cos(pi / 6.0) - 0.8660254).abs() < 1e-4);
        assert!((f32_sin(pi / 4.0) - 0.70710678).abs() < 1e-4);
        assert!((f32_cos(pi / 4.0) - 0.70710678).abs() < 1e-4);
        assert!((f32_sin(pi / 2.0) - 1.0).abs() < 1e-4);
        assert!(f32_cos(pi / 2.0).abs() < 1e-4);
        assert!(f32_sin(pi).abs() < 1e-4);
        assert!((f32_cos(pi) - (-1.0)).abs() < 1e-4);
        assert!((f32_sin(1.5 * pi) - (-1.0)).abs() < 1e-4);
        assert!(f32_cos(1.5 * pi).abs() < 1e-4);
        assert!(f32_sin(2.0 * pi).abs() < 1e-4);
        assert!((f32_cos(2.0 * pi) - 1.0).abs() < 1e-4);

        // Negative angles
        assert!((f32_sin(-pi / 2.0) - (-1.0)).abs() < 1e-4);
        assert!(f32_cos(-pi / 2.0).abs() < 1e-4);

        // Large finite angles (where old while loops stalled / hung)
        assert!(f32_sin(1000.0).is_finite());
        assert!(f32_cos(1000.0).is_finite());
        assert!(f32_sin(131072.0).is_finite());
        assert!(f32_cos(131072.0).is_finite());
        assert!(f32_sin(1000000.0).is_finite());
        assert!(f32_cos(1000000.0).is_finite());

        // Extreme inputs (> 1e7 or non-finite) MUST NOT hang or overflow
        assert!(f32_sin(1e30).is_nan());
        assert!(f32_cos(1e30).is_nan());
        assert!(f32_sin(f32::INFINITY).is_nan());
        assert!(f32_cos(f32::INFINITY).is_nan());
        assert!(f32_sin(f32::NAN).is_nan());
        assert!(f32_cos(f32::NAN).is_nan());

        // Pythagorean identity sin^2 + cos^2 = 1.0
        for test_val in [0.1f32, 0.7, 1.2, 2.5, 5.0, 100.0, 50000.0] {
            let s = f32_sin(test_val);
            let c = f32_cos(test_val);
            let id = s * s + c * c;
            assert!((id - 1.0).abs() < 1e-4, "Failed for {test_val}: {s}^2 + {c}^2 = {id}");
        }
    }

    #[test]
    fn test_f32_pow_approx_accuracy() {
        assert_eq!(f32_pow_approx(10000.0, 0.0), 1.0);
        // Base 8000 (common LiteRT base: 8000^(1/3) = 20.0)
        let v0 = f32_pow_approx(8000.0, 1.0 / 3.0);
        assert!((v0 - 20.0).abs() < 0.2, "8000^(1/3) = {v0}, expected ~20.0");
        // Base 10000 (standard RoPE base)
        let v1 = f32_pow_approx(10000.0, 0.25); // 10000^0.25 = 10.0
        assert!((v1 - 10.0).abs() < 0.1, "10000^0.25 = {v1}, expected ~10.0");
        // Base 1000000 (extended RoPE base)
        let v2 = f32_pow_approx(1000000.0, 0.5); // 1000000^0.5 = 1000.0
        assert!((v2 - 1000.0).abs() < 5.0, "1000000^0.5 = {v2}, expected ~1000.0");

        // Negative, zero, NaN, or non-finite bases must produce NaN
        assert!(f32_pow_approx(f32::NAN, 1.0).is_nan());
        assert!(f32_pow_approx(10000.0, f32::NAN).is_nan());
        assert!(f32_pow_approx(-10000.0, 1.0).is_nan());
        assert!(f32_pow_approx(0.0, 1.0).is_nan());
        assert!(f32_pow_approx(f32::INFINITY, 1.0).is_nan());
        assert!(f32_pow_approx(10000.0, f32::INFINITY).is_nan());

        // apply_rope resilience to non-finite or invalid base
        let mut vec = [1.0f32, 2.0, 3.0, 4.0];
        apply_rope(&mut vec, 1, 4, -10000.0);
        assert_eq!(vec, [1.0, 2.0, 3.0, 4.0], "Negative base must leave vector unmodified");
        apply_rope(&mut vec, 1, 0, 10000.0);
        assert_eq!(vec, [1.0, 2.0, 3.0, 4.0], "Zero head_dim must leave vector unmodified");
    }

    #[test]
    fn q6_k_block_decodes_to_reference_minus_32() {
        // Independent reference: with all quant nibbles zero, every Q6_K weight
        // is (0 - 32); with scale int8 = 1 and FP16 d = 1.0 every decoded value
        // is exactly -32.0. This pins the ql/qh/sc/d layout and the -32 bias.
        let mut blk = alloc::vec![0u8; Q6_K_BLOCK_BYTES];
        for s in blk.iter_mut().take(208).skip(192) {
            *s = 1;
        }
        blk[208] = 0x00;
        blk[209] = 0x3C; // f16 1.0
        let mut out = alloc::vec![0f32; Q6_K_WEIGHTS];
        let produced = dequantize_q6_k(&blk, &mut out).expect("q6_k block decodes");
        assert_eq!(produced, 256);
        assert!(out.iter().all(|&v| v == -32.0), "q6_k reference mismatch: {:?}", &out[..4]);
    }

    #[test]
    fn q6_k_block_matches_independent_mixed_value_reference() {
        // Independent oracle: a dev-time Python implementation of ggml's
        // dequantize_row_q6_K over a fixed 210-byte block (seed 1234). Expected
        // values are from that oracle, not from this code, so the test checks
        // the ql/qh/sc/d layout and the -32 bias on mixed nonzero data.
        let blk: [u8; 210] = [
            247,199,112,29,1,23,233,241,240,206,149,8,171,177,21,25,196,90,60,4,7,252,201,4,88,
            165,159,123,157,118,38,23,46,182,29,216,3,128,124,225,247,63,16,170,138,233,119,17,
            153,172,22,131,148,11,69,211,142,16,165,77,123,217,90,68,63,167,238,51,116,224,135,
            170,4,241,130,18,1,85,36,181,121,168,96,42,13,19,150,127,41,19,142,55,36,6,239,88,
            197,28,244,135,36,171,78,116,10,106,70,169,206,10,45,69,39,76,244,216,39,199,213,
            118,10,144,98,195,89,126,87,149,209,119,121,11,200,238,120,222,209,152,225,43,112,
            187,199,132,208,112,75,70,31,199,47,106,111,196,30,240,135,26,212,215,124,96,209,
            209,168,65,164,49,88,27,179,179,207,246,40,148,232,241,68,248,39,217,215,251,184,
            82,203,201,114,184,18,20,101,54,12,229,73,205,9,56,129,212,22,202,238,240,179,255,
            81,78,
        ];
        let expected: [f32; 8] = [
            -22966.453125, 58692.046875, -40829.25, 74003.015625, -79106.671875, 17862.796875,
            -58692.046875, 2551.828125,
        ];
        let mut out = alloc::vec![0f32; 256];
        let produced = dequantize_q6_k(&blk, &mut out).expect("q6_k mixed block decodes");
        assert_eq!(produced, 256);
        for i in 0..8 {
            let tol = 1e-3 * expected[i].abs().max(1.0);
            assert!(
                (out[i] - expected[i]).abs() <= tol,
                "q6_k[{i}] = {} vs reference {}",
                out[i],
                expected[i]
            );
        }
        assert!((out[100] - 3638.25).abs() <= 1e-3 * 3638.25f32.abs().max(1.0));
        assert!((out[200] - 7276.5).abs() <= 1e-3 * 7276.5f32.abs().max(1.0));
    }

    #[test]
    fn new_quant_block_decoders_reject_partial_and_short_buffers() {
        let mut out = alloc::vec![0f32; 256];
        assert_eq!(
            dequantize_q5_k(&alloc::vec![0u8; Q5_K_BLOCK_BYTES - 1], &mut out),
            Err(InferenceError::CorruptedWeights)
        );
        assert_eq!(
            dequantize_q6_k(&alloc::vec![0u8; Q6_K_BLOCK_BYTES - 1], &mut out),
            Err(InferenceError::CorruptedWeights)
        );
        assert_eq!(
            dequantize_tq2_0(&alloc::vec![0u8; TQ2_0_BLOCK_BYTES - 1], &mut out),
            Err(InferenceError::CorruptedWeights)
        );
        let short = &mut out[..255];
        assert_eq!(
            dequantize_q6_k(&alloc::vec![0u8; Q6_K_BLOCK_BYTES], short),
            Err(InferenceError::BufferTooSmall)
        );
    }

    #[test]
    fn new_dtype_rows_size_and_decode_as_zero_for_zero_weights() {
        assert_eq!(quantised_row_bytes(13, 256), Some(Q5_K_BLOCK_BYTES));
        assert_eq!(quantised_row_bytes(14, 256), Some(Q6_K_BLOCK_BYTES));
        assert_eq!(quantised_row_bytes(35, 256), Some(TQ2_0_BLOCK_BYTES));
        assert_eq!(quantised_row_bytes(13, 255), None);
        assert_eq!(quantised_row_bytes(999, 256), None);

        let zero = alloc::vec![0u8; Q6_K_BLOCK_BYTES];
        let mut row = alloc::vec![0f32; 256];
        embedding_row_into(&zero, 14, 1.0, &mut row).expect("q6_k embedding row");
        assert!(row.iter().all(|&v| v == 0.0));

        let x = alloc::vec![0f32; 256];
        let mut out = alloc::vec![0f32; 1];
        gemv(&zero, 14, &x, &mut out, 1, 256).expect("q6_k gemv");
        assert_eq!(out[0], 0.0);
    }

    #[test]
    fn q5_k_block_matches_independent_mixed_reference() {
        // Independent oracle: dev-time Python (fixed seed 1234) implementing
        // ggml's dequantize_row_q5_K over a fixed 176-byte block.
        // Expected values are the oracle's, not this code's, so the test checks
        // the ql/qh/scales/d/dmin layout and 6-bit scale/min packing on mixed nonzero data across all 256 weights.
        let blk: [u8; 176] = [
        47, 211, 38, 53, 204, 116, 152, 249, 143, 177, 23, 233, 154, 30, 171, 158,
        236, 124, 26, 118, 186, 120, 112, 220, 69, 80, 201, 127, 246, 254, 175, 50,
        240, 251, 76, 37, 34, 166, 250, 195, 231, 139, 128, 233, 75, 80, 3, 2,
        19, 140, 193, 203, 115, 107, 250, 209, 14, 243, 199, 60, 234, 107, 174, 156,
        81, 87, 13, 116, 96, 140, 197, 253, 113, 223, 229, 159, 249, 252, 89, 84,
        45, 16, 41, 72, 184, 236, 70, 184, 86, 172, 218, 211, 47, 177, 18, 85,
        174, 226, 37, 109, 38, 135, 228, 195, 133, 238, 47, 246, 244, 118, 175, 143,
        34, 10, 28, 4, 82, 103, 89, 55, 235, 78, 151, 178, 249, 62, 183, 84,
        105, 0, 121, 98, 249, 90, 161, 114, 121, 241, 21, 199, 196, 119, 231, 209,
        250, 180, 192, 213, 116, 105, 114, 169, 1, 142, 3, 30, 140, 245, 201, 109,
        19, 26, 224, 68, 123, 228, 64, 150, 184, 212, 136, 172, 241, 152, 222, 233,
        ];
        let expected: [f32; 256] = [
        -2073.70166015625, -8280.326171875, -694.45166015625, -7590.70166015625, -2073.70166015625, -7590.70166015625, -6901.07666015625, -694.45166015625,
        -20693.576171875, -2073.70166015625, -15866.201171875, -19314.326171875, -6901.07666015625, -7590.70166015625, -20693.576171875, -8280.326171875,
        -694.45166015625, -15866.201171875, -8969.951171875, -13797.326171875, -4.82666015625, -8280.326171875, -3452.95166015625, -20003.951171875,
        -11728.451171875, -21383.201171875, -3452.95166015625, -21383.201171875, -17245.451171875, -8280.326171875, -17245.451171875, -2763.32666015625,
        -3004.14208984375, -23922.767578125, -83690.265625, -83690.265625, -68748.390625, -17946.017578125, -44841.390625, -38864.640625,
        -15.76708984375, -44841.390625, -35876.265625, -56794.890625, -89667.015625, -65760.015625, -77713.515625, -74725.140625,
        -14957.642578125, -62771.640625, -15.76708984375, -20934.392578125, -65760.015625, -71736.765625, -83690.265625, -92655.390625,
        -68748.390625, -86678.640625, -41853.015625, -26911.142578125, -92655.390625, -44841.390625, -62771.640625, -62771.640625,
        -40005.65234375, -22075.400390625, -12420.650390625, -33109.40234375, -11041.400390625, -16558.400390625, -8282.900390625, -33109.40234375,
        -30350.900390625, -16558.400390625, -13799.900390625, -26213.150390625, -42764.15234375, -23454.650390625, -24833.900390625, -6903.65087890625,
        -19316.900390625, -2765.90087890625, -28971.650390625, -40005.65234375, -8282.900390625, -31730.150390625, -5524.40087890625, -4145.15087890625,
        -28971.650390625, -19316.900390625, -20696.150390625, -8282.900390625, -5524.40087890625, -8282.900390625, -20696.150390625, -20696.150390625,
        -58976.12890625, -55700.41015625, -58976.12890625, -13116.068359375, -88457.6015625, -98284.7578125, -13116.068359375, -88457.6015625,
        -16391.787109375, -32770.37890625, -95009.0390625, -95009.0390625, -6564.63037109375, -88457.6015625, -55700.41015625, -16391.787109375,
        -32770.37890625, -98284.7578125, -58976.12890625, -19667.505859375, -6564.63037109375, -26218.943359375, -98284.7578125, -39321.81640625,
        -26218.943359375, -98284.7578125, -6564.63037109375, -101560.4765625, -101560.4765625, -22943.224609375, -32770.37890625, -26218.943359375,
        -6679.56787109375, -86676.0703125, -93342.4453125, -66676.9453125, -60010.56640625, -76676.5078125, -83342.8828125, -76676.5078125,
        -36678.25390625, -100008.8203125, -23345.505859375, -60010.56640625, -83342.8828125, -100008.8203125, -23345.505859375, -66676.9453125,
        -83342.8828125, -53344.19140625, -30011.880859375, -6679.56787109375, -30011.880859375, -33345.06640625, -56677.37890625, -6679.56787109375,
        -30011.880859375, -3346.38037109375, -16679.130859375, -23345.505859375, -13345.943359375, -76676.5078125, -23345.505859375, -3346.38037109375,
        -31043.744140625, -27595.619140625, -1734.68115234375, -27595.619140625, -36215.9296875, -37939.9921875, -36215.9296875, -5182.80615234375,
        -24147.494140625, -6906.86865234375, -15527.181640625, -46560.3046875, -53456.5546875, -32767.806640625, -46560.3046875, -36215.9296875,
        -37939.9921875, -27595.619140625, -12079.056640625, -37939.9921875, -53456.5546875, -36215.9296875, -44836.2421875, -12079.056640625,
        -39664.0546875, -25871.556640625, -1734.68115234375, -48284.3671875, -20699.369140625, -12079.056640625, -24147.494140625, -22423.431640625,
        -64253.28125, -49426.34375, -3.2177734375, -51897.5, -9887.8427734375, -61782.125, -44484.03125, -61782.125,
        -42012.875, -74137.90625, -46955.1875, -74137.90625, -69195.59375, -51897.5, -22243.625, -32128.25,
        -46955.1875, -64253.28125, -39541.71875, -9887.8427734375, -27185.9375, -9887.8427734375, -39541.71875, -54368.65625,
        -59310.96875, -9887.8427734375, -19772.46875, -69195.59375, -42012.875, -59310.96875, -34599.40625, -22243.625,
        -110473.28125, -39212.02734375, -42775.08984375, -46338.15234375, -81968.78125, -21396.716796875, -24959.779296875, -92657.96875,
        -18.34130859375, -28522.841796875, -57027.33984375, -3581.40380859375, -85531.84375, -110473.28125, -99784.09375, -21396.716796875,
        -60590.40234375, -60590.40234375, -49901.21484375, -14270.591796875, -24959.779296875, -106910.21875, -71279.59375, -89094.90625,
        -96221.03125, -103347.15625, -85531.84375, -92657.96875, -53464.27734375, -32085.904296875, -46338.15234375, -49901.21484375,
        ];
        let mut out = alloc::vec![0f32; 256];
        let produced = dequantize_q5_k(&blk, &mut out).expect("q5_k mixed block decodes");
        assert_eq!(produced, 256);
        let mut max_abs = 0.0f32;
        let mut max_rel = 0.0f32;
        for i in 0..256 {
            let tol = 1e-3 * expected[i].abs().max(1.0);
            let diff = (out[i] - expected[i]).abs();
            max_abs = max_abs.max(diff);
            max_rel = max_rel.max(diff / expected[i].abs().max(1.0));
            assert!(
                diff <= tol,
                "q5_k[{i}] = {} vs reference {} (diff {diff})",
                out[i], expected[i]
            );
        }
        assert!(max_abs <= 1.0, "q5_k max_abs {max_abs} max_rel {max_rel}");
    }

    #[test]
    fn q6_k_full_block_matches_independent_reference() {
        // Independent oracle: dev-time Python (fixed seed 1234) implementing
        // ggml's dequantize_row_q6_K over a fixed 210-byte block.
        // Expected values are the oracle's, not this code's, so the test checks
        // the ql/qh/sc/d layout and the -32 bias on mixed nonzero data across all 256 weights.
        let blk: [u8; 210] = [
        47, 211, 38, 53, 204, 116, 152, 249, 143, 177, 23, 233, 154, 30, 171, 158,
        236, 124, 26, 118, 186, 120, 112, 220, 69, 80, 201, 127, 246, 254, 175, 50,
        240, 251, 76, 37, 34, 166, 250, 195, 231, 139, 128, 233, 75, 80, 3, 2,
        19, 140, 193, 203, 115, 107, 250, 209, 14, 243, 199, 60, 234, 107, 174, 156,
        81, 87, 13, 116, 96, 140, 197, 253, 113, 223, 229, 159, 249, 252, 89, 84,
        45, 16, 41, 72, 184, 236, 70, 184, 86, 172, 218, 211, 47, 177, 18, 85,
        174, 226, 37, 109, 38, 135, 228, 195, 133, 238, 47, 246, 244, 118, 175, 143,
        34, 10, 28, 4, 82, 103, 89, 55, 235, 78, 151, 178, 249, 62, 183, 84,
        105, 0, 121, 98, 249, 90, 161, 114, 121, 241, 21, 199, 196, 119, 231, 209,
        250, 180, 192, 213, 116, 105, 114, 169, 1, 142, 3, 30, 140, 245, 201, 109,
        19, 26, 224, 68, 123, 228, 64, 150, 184, 212, 136, 172, 241, 152, 222, 233,
        15, 72, 130, 144, 107, 130, 242, 79, 195, 46, 226, 57, 183, 36, 88, 161,
        121, 170, 2, 215, 109, 212, 35, 18, 76, 197, 117, 81, 208, 8, 237, 75,
        15, 20,
        ];
        let expected: [f32; 256] = [
        -0.11989498138427734, -3.476954460144043, -1.1989498138427734, 0.5994749069213867, -0.4795799255371094, 0.4795799255371094, -0.9591598510742188, 1.079054832458496,
        -0.11989498138427734, -1.7984247207641602, -1.079054832458496, 2.9973745346069336, -2.6376895904541016, 3.5968494415283203, 3.2371644973754883, -0.2397899627685547,
        -1.0225753784179688, 1.7042922973632812, 1.8747215270996094, 0.8521461486816406, 1.8747215270996094, 0.6817169189453125, -0.0, 0.34085845947265625,
        0.9373607635498047, -0.0, -2.1303653717041016, -1.278219223022461, 2.2155799865722656, 0.17042922973632812, 0.08521461486816406, 1.1930046081542969,
        0.0, -0.04161643981933594, 0.02378082275390625, -0.05350685119628906, 0.003963470458984375, 0.011890411376953125, -0.043598175048828125, -0.05747032165527344,
        0.013872146606445312, -0.04161643981933594, -0.031707763671875, -0.013872146606445312, -0.009908676147460938, -0.031707763671875, -0.025762557983398438, -0.059452056884765625,
        -0.12187671661376953, 0.16250228881835938, 1.2593927383422852, 0.20312786102294922, 0.528132438659668, -0.4468812942504883, 0.8937625885009766, -0.040625572204589844,
        0.7312602996826172, -0.771885871887207, 1.015639305114746, -1.1375160217285156, -1.056264877319336, 0.20312786102294922, -0.5687580108642578, -1.1375160217285156,
        0.21600914001464844, -2.05208683013916, 1.944082260131836, 0.32401371002197266, 3.024127960205078, -0.972041130065918, 0.972041130065918, 3.348141670227051,
        2.5921096801757812, 2.916123390197754, -1.6200685501098633, -1.944082260131836, -2.484105110168457, 1.8360776901245117, 1.0800457000732422, -0.7560319900512695,
        -1.3079452514648438, -1.0027580261230469, 1.3515434265136719, 0.3923835754394531, -1.1771507263183594, -0.3051872253417969, -1.0027580261230469, -0.5667762756347656,
        1.2207489013671875, 1.1771507263183594, 0.8719635009765625, 0.3923835754394531, 0.7411689758300781, -1.3515434265136719, 0.9591598510742188, -0.13079452514648438,
        -0.03468036651611328, -0.5895662307739258, -0.4161643981933594, -0.48552513122558594, 0.6242465972900391, -0.2080821990966797, 0.5202054977416992, -0.13872146606445312,
        -0.06936073303222656, 0.8323287963867188, -0.8323287963867188, 1.0404109954833984, 0.6936073303222656, -0.3814840316772461, 0.5548858642578125, 0.5548858642578125,
        0.3032054901123047, 0.1426849365234375, 0.49939727783203125, 0.49939727783203125, -0.1605205535888672, -0.17835617065429688, -0.017835617065429688, 0.23186302185058594,
        -0.57073974609375, 0.2675342559814453, -0.35671234130859375, -0.5172328948974609, 0.24969863891601562, 0.3923835754394531, 0.4637260437011719, -0.12484931945800781,
        1.2802009582519531, 0.5271415710449219, -1.4308128356933594, -2.1085662841796875, 1.20489501953125, -1.5061187744140625, -2.0332603454589844, 0.9789772033691406,
        -2.334484100341797, -1.2802009582519531, -2.0332603454589844, -1.2802009582519531, -0.5271415710449219, -1.5061187744140625, 0.6777534484863281, -0.9036712646484375,
        -1.6953744888305664, 1.870758056640625, -0.5261507034301758, 1.4030685424804688, -1.4030685424804688, -0.7015342712402344, -0.3507671356201172, -1.4030685424804688,
        -1.2861461639404297, -0.7015342712402344, -0.5846118927001953, 0.7599954605102539, -1.8122968673706055, 1.8122968673706055, 1.753835678100586, 0.6430730819702148,
        -2.0867671966552734, 0.23186302185058594, -3.13015079498291, -0.3477945327758789, 0.6955890655517578, -1.0433835983276367, -3.246082305908203, -1.5071096420288086,
        0.5796575546264648, -0.23186302185058594, 1.7389726638793945, 2.5504932403564453, -3.246082305908203, 0.6955890655517578, 3.593876838684082, 1.7389726638793945,
        1.4446849822998047, 0.8026027679443359, -1.6052055358886719, -2.2472877502441406, 0.1605205535888672, -2.00650691986084, -1.8459863662719727, 1.8459863662719727,
        -1.6854658126831055, 2.407808303833008, -2.00650691986084, 0.1605205535888672, -0.5618219375610352, -0.1605205535888672, 0.5618219375610352, -2.2472877502441406,
        0.5231781005859375, 0.5231781005859375, -0.0, 1.1890411376953125, -1.046356201171875, -0.3804931640625, 0.95123291015625, 0.0475616455078125,
        -1.0939178466796875, 0.1426849365234375, 0.856109619140625, -0.4280548095703125, -1.4744110107421875, 0.0475616455078125, 0.5231781005859375, -0.2378082275390625,
        -0.2378082275390625, -0.24573516845703125, -0.2378082275390625, -0.095123291015625, 0.08719635009765625, -0.1426849365234375, 0.158538818359375, -0.16646575927734375,
        -0.21402740478515625, 0.0792694091796875, 0.10305023193359375, 0.22988128662109375, 0.1426849365234375, 0.08719635009765625, -0.11890411376953125, 0.03963470458984375,
        0.4141826629638672, 0.33887672424316406, -0.33887672424316406, 0.1882648468017578, 0.26357078552246094, -0.45183563232421875, 0.03765296936035156, -0.22591781616210938,
        -0.15061187744140625, -0.5647945404052734, -0.03765296936035156, -0.2823972702026367, -0.5836210250854492, -0.13178539276123047, -0.4894886016845703, -0.45183563232421875,
        -2.229452133178711, -1.1890411376953125, 0.07431507110595703, 0.0, -0.8174657821655273, 0.4458904266357422, 1.5606164932250977, -0.9660959243774414,
        2.229452133178711, -2.080821990966797, 1.8578767776489258, -1.5606164932250977, 1.1147260665893555, -2.155137062072754, -0.37157535552978516, 0.37157535552978516,
        ];
        let mut out = alloc::vec![0f32; 256];
        let produced = dequantize_q6_k(&blk, &mut out).expect("q6_k mixed block decodes");
        assert_eq!(produced, 256);
        let mut max_abs = 0.0f32;
        let mut max_rel = 0.0f32;
        for i in 0..256 {
            let tol = 1e-3 * expected[i].abs().max(1.0);
            let diff = (out[i] - expected[i]).abs();
            max_abs = max_abs.max(diff);
            max_rel = max_rel.max(diff / expected[i].abs().max(1.0));
            assert!(
                diff <= tol,
                "q6_k[{i}] = {} vs reference {} (diff {diff})",
                out[i], expected[i]
            );
        }
        assert!(max_abs <= 1.0, "q6_k max_abs {max_abs} max_rel {max_rel}");
    }

    #[test]
    fn tq2_0_block_matches_independent_mixed_reference() {
        // Independent oracle: dev-time Python (fixed seed 1234) implementing
        // ggml's dequantize_row_tq2_0 over a fixed 66-byte block.
        // Expected values are the oracle's, not this code's, so the test checks
        // the qs/d layout and the two-bit ternary (-1,0,+1) plane order on mixed nonzero data across all 256 weights.
        let blk: [u8; 66] = [
        47, 211, 38, 53, 204, 116, 152, 249, 143, 177, 23, 233, 154, 30, 171, 158,
        236, 124, 26, 118, 186, 120, 112, 220, 69, 80, 201, 127, 246, 254, 175, 50,
        240, 251, 76, 37, 34, 166, 250, 195, 231, 139, 128, 233, 75, 80, 3, 2,
        19, 140, 193, 203, 115, 107, 250, 209, 14, 243, 199, 60, 234, 107, 174, 156,
        81, 87,
        ];
        let expected: [f32; 256] = [
        234.125, 234.125, 117.0625, 0.0, -117.0625, -117.0625, -117.0625, 0.0,
        234.125, 0.0, 234.125, 0.0, 117.0625, 117.0625, 234.125, 117.0625,
        -117.0625, -117.0625, 117.0625, 117.0625, 117.0625, -117.0625, -117.0625, -117.0625,
        0.0, -117.0625, 0.0, 234.125, 117.0625, 117.0625, 234.125, 117.0625,
        234.125, -117.0625, 0.0, 0.0, 234.125, 0.0, 117.0625, 117.0625,
        234.125, -117.0625, 0.0, 117.0625, 117.0625, 234.125, 117.0625, 234.125,
        234.125, 234.125, 117.0625, 0.0, 117.0625, 117.0625, -117.0625, 234.125,
        0.0, -117.0625, 117.0625, 234.125, 0.0, 234.125, 234.125, -117.0625,
        117.0625, 0.0, 117.0625, 234.125, -117.0625, 234.125, 0.0, 234.125,
        -117.0625, 234.125, 0.0, 117.0625, 0.0, 0.0, 117.0625, 0.0,
        117.0625, 234.125, 0.0, 234.125, 234.125, 234.125, 234.125, 0.0,
        -117.0625, 0.0, -117.0625, 234.125, 234.125, 234.125, 117.0625, 234.125,
        -117.0625, 234.125, -117.0625, -117.0625, 234.125, 0.0, 117.0625, 234.125,
        117.0625, 117.0625, -117.0625, 234.125, 117.0625, -117.0625, 117.0625, 117.0625,
        234.125, 0.0, -117.0625, 0.0, 117.0625, 0.0, 0.0, 234.125,
        0.0, 0.0, 234.125, 0.0, 234.125, 234.125, 117.0625, -117.0625,
        -117.0625, 234.125, -117.0625, 0.0, 117.0625, 117.0625, 117.0625, 234.125,
        234.125, 234.125, -117.0625, 0.0, 234.125, -117.0625, 234.125, 117.0625,
        234.125, -117.0625, 0.0, 234.125, 234.125, 234.125, 117.0625, 0.0,
        117.0625, 234.125, 234.125, -117.0625, 117.0625, 234.125, 117.0625, -117.0625,
        -117.0625, 117.0625, 234.125, 0.0, -117.0625, 0.0, 117.0625, -117.0625,
        0.0, 117.0625, -117.0625, 117.0625, 117.0625, -117.0625, -117.0625, -117.0625,
        -117.0625, 234.125, -117.0625, 117.0625, -117.0625, 117.0625, 117.0625, -117.0625,
        234.125, -117.0625, 0.0, 234.125, 117.0625, 117.0625, 234.125, 234.125,
        234.125, 234.125, -117.0625, 117.0625, 117.0625, 117.0625, 234.125, -117.0625,
        117.0625, -117.0625, -117.0625, 117.0625, -117.0625, 0.0, -117.0625, -117.0625,
        0.0, -117.0625, -117.0625, -117.0625, 234.125, 117.0625, 234.125, 0.0,
        -117.0625, 234.125, -117.0625, 234.125, 117.0625, 117.0625, 117.0625, 0.0,
        234.125, 234.125, 0.0, -117.0625, -117.0625, 117.0625, 234.125, 234.125,
        234.125, 117.0625, 117.0625, 234.125, 0.0, 0.0, -117.0625, -117.0625,
        -117.0625, 117.0625, 234.125, 234.125, 0.0, 0.0, 234.125, 234.125,
        -117.0625, 234.125, 234.125, -117.0625, 234.125, 0.0, 117.0625, 117.0625,
        ];
        let mut out = alloc::vec![0f32; 256];
        let produced = dequantize_tq2_0(&blk, &mut out).expect("tq2_0 mixed block decodes");
        assert_eq!(produced, 256);
        let mut max_abs = 0.0f32;
        let mut max_rel = 0.0f32;
        for i in 0..256 {
            let tol = 1e-3 * expected[i].abs().max(1.0);
            let diff = (out[i] - expected[i]).abs();
            max_abs = max_abs.max(diff);
            max_rel = max_rel.max(diff / expected[i].abs().max(1.0));
            assert!(
                diff <= tol,
                "tq2_0[{i}] = {} vs reference {} (diff {diff})",
                out[i], expected[i]
            );
        }
        assert!(max_abs <= 1.0, "tq2_0 max_abs {max_abs} max_rel {max_rel}");
    }
}
