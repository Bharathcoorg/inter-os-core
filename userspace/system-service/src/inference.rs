//! Real neural inference engine and model lifecycle for InterOS AI service (`aid`).
//!
//! Strictly `#![no_std]` and zero heap allocation in all execution paths: every
//! activation below is a fixed-size array on the caller's stack.
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
//! Nothing in the forward path falls back. A declaration wider than a buffer, a
//! quantisation the engine has no layout for, or an embedding row the file does
//! not hold is reported as the refusal it is. An invented stand-in would let an
//! answer computed from nothing be reported as the installed model's answer.

pub const MAX_HIDDEN_DIM: usize = 256;
pub const MAX_INTERMEDIATE_DIM: usize = 512;
pub const MAX_VOCAB_SIZE: usize = 320;
pub const MAX_SEQ_LEN: usize = 64;
pub const MAX_LAYERS: usize = 2;
/// Width of one KV cache row. The cache is `[[f32; 32]; MAX_SEQ_LEN]`, so a
/// head the file declares wider than this has nowhere to be stored. Published
/// Gemma files state this width as `<arch>.attention.key_length`.
pub const MAX_HEAD_DIM: usize = 32;

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
    /// no such key), and the bound the engine is compiled to.
    ModelDeclaration(&'static str, Option<u32>, usize),
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
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ModelConfig {
    pub hidden_dim: usize,
    pub intermediate_dim: usize,
    pub num_layers: usize,
    pub num_heads: usize,
    pub num_kv_heads: usize,
    pub head_dim: usize,
    pub vocab_size: usize,
    pub rms_norm_eps: f32,
    pub rope_theta: f32,
}

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
            rms_norm_eps: 1e-6,
            rope_theta: 10000.0,
        }
    }

    /// Builds the engine's parameters from what the model file declares about
    /// itself, refusing a declaration the engine's static buffers cannot hold.
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
        fn fits(key: &'static str, declared: Option<u32>, limit: usize) -> Result<usize, InferenceError> {
            let Some(declared) = declared else {
                return Err(InferenceError::ModelDeclaration(key, None, limit));
            };
            let Ok(size) = usize::try_from(declared) else {
                return Err(InferenceError::ModelDeclaration(key, Some(declared), limit));
            };
            if size == 0 || size > limit {
                return Err(InferenceError::ModelDeclaration(key, Some(declared), limit));
            }
            Ok(size)
        }

        let hidden_dim = fits("<arch>.embedding_length", info.hidden_dim, MAX_HIDDEN_DIM)?;
        let intermediate_dim = fits(
            "<arch>.feed_forward_length",
            info.intermediate_dim,
            MAX_INTERMEDIATE_DIM,
        )?;
        let num_layers = fits("<arch>.block_count", info.num_layers, MAX_LAYERS)?;
        let head_dim = fits("<arch>.attention.key_length", info.head_dim, MAX_HEAD_DIM)?;
        let num_heads = fits("<arch>.attention.head_count", info.num_heads, MAX_HIDDEN_DIM)?;
        let vocab_size = fits("tokenizer.ggml.tokens", info.vocab_size, MAX_VOCAB_SIZE)?;

        // The head loop walks `num_heads` slices of `head_dim` out of a buffer
        // that is `hidden_dim` long, so a file whose heads do not tile its
        // residual stream would index past it.
        let heads_span = num_heads
            .checked_mul(head_dim)
            .ok_or(InferenceError::ModelDeclaration(
                "<arch>.attention.head_count",
                info.num_heads,
                hidden_dim,
            ))?;
        if heads_span > hidden_dim {
            return Err(InferenceError::ModelDeclaration(
                "<arch>.attention.head_count x key_length",
                u32::try_from(heads_span).ok(),
                hidden_dim,
            ));
        }

        let num_kv_heads = match info.num_kv_heads {
            Some(kv) if kv > 0 && usize::try_from(kv).unwrap_or(usize::MAX) <= num_heads => {
                kv as usize
            }
            _ => num_heads,
        };

        // The cache holds one K row and one V row per (layer, position), which is
        // exactly what a model with a single key/value head needs. A file that
        // declares more asks for rows this shape cannot address, so the refusal
        // names the count rather than letting every head read head zero's cached
        // keys. A file that states no count is the format's own multi-head case:
        // one KV head per query head.
        if num_kv_heads > 1 {
            let declared = info
                .num_kv_heads
                .or_else(|| u32::try_from(num_kv_heads).ok());
            return Err(InferenceError::ModelDeclaration(
                "<arch>.attention.head_count_kv",
                declared,
                1,
            ));
        }

        Ok(Self {
            hidden_dim,
            intermediate_dim,
            num_layers,
            num_heads,
            num_kv_heads,
            head_dim,
            vocab_size,
            rms_norm_eps: info.rms_norm_eps.unwrap_or(1e-6),
            rope_theta: info.rope_theta.unwrap_or(10000.0),
        })
    }
}

/// References to layer weights (supports zero-copy referencing from memory or test fixture).
pub struct LayerWeights<'a> {
    pub attn_norm: &'a [f32],
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
    pub weight_type: u32,
}

/// References to model weights.
pub struct ModelWeights<'a> {
    pub token_embd: &'a [u8],
    pub token_embd_type: u32,
    pub layers: [Option<LayerWeights<'a>>; MAX_LAYERS],
    pub final_norm: &'a [f32],
    pub output_weight: &'a [u8],
    pub output_weight_type: u32,
}

/// KV cache for attention mechanisms with zero heap allocation.
pub struct KvCache {
    pub k: [[[f32; 32]; MAX_SEQ_LEN]; MAX_LAYERS],
    pub v: [[[f32; 32]; MAX_SEQ_LEN]; MAX_LAYERS],
    pub len: usize,
}

impl KvCache {
    pub const fn empty() -> Self {
        Self {
            k: [[[0.0f32; 32]; MAX_SEQ_LEN]; MAX_LAYERS],
            v: [[[0.0f32; 32]; MAX_SEQ_LEN]; MAX_LAYERS],
            len: 0,
        }
    }

    pub fn reset(&mut self) {
        self.len = 0;
    }
}

// -----------------------------------------------------------------------------
// Neural Forward Pass & Generation
// -----------------------------------------------------------------------------

/// Executes a single forward step of the neural network for one token at `pos`.
pub fn forward_step(
    token_id: u32,
    pos: usize,
    model: &ModelWeights,
    config: &ModelConfig,
    kv_cache: &mut KvCache,
    out_logits: &mut [f32],
    out_hidden: &mut [f32],
) -> Result<(), InferenceError> {
    if pos >= MAX_SEQ_LEN {
        return Err(InferenceError::ContextLengthExceeded);
    }
    let h_dim = config.hidden_dim;
    if h_dim > MAX_HIDDEN_DIM {
        return Err(InferenceError::BufferTooSmall);
    }
    // A declaration wider than a buffer is refused, never truncated to fit. The
    // cache row is `MAX_HEAD_DIM` and the FFN buffers are
    // `MAX_INTERMEDIATE_DIM`, so clipping either would let an answer computed at
    // 32 wide be reported as the answer of a model that declared 256.
    if config.head_dim > MAX_HEAD_DIM || config.num_kv_heads > 1 {
        return Err(InferenceError::BufferTooSmall);
    }
    if config.intermediate_dim > MAX_INTERMEDIATE_DIM {
        return Err(InferenceError::BufferTooSmall);
    }
    // The head loop indexes `num_heads x head_dim` of a stream that is `h_dim`
    // long, so heads that do not tile it would read past the projection that
    // produced them. `from_gguf` refuses this pairing already; a hand-built
    // config is checked here for the same reason.
    let heads_span = config
        .num_heads
        .checked_mul(config.head_dim)
        .unwrap_or(usize::MAX);
    if heads_span > h_dim {
        return Err(InferenceError::BufferTooSmall);
    }
    // Checked before any work, so a call this engine will not complete leaves
    // the cache holding what it held when it arrived.
    if !out_hidden.is_empty() && out_hidden.len() < h_dim {
        return Err(InferenceError::BufferTooSmall);
    }

    // 1. Embedding lookup. The row is read as floats because that is the only
    // layout this path can address; a quantised embedding belongs to a loader
    // that dequantises it, and reading its blocks as floats would be noise.
    if model.token_embd_type != 0 {
        return Err(InferenceError::UnsupportedQuantization(model.token_embd_type));
    }
    let mut x = [0.0f32; MAX_HIDDEN_DIM];
    let embd_idx = (token_id as usize) % config.vocab_size;
    let embd_stride = h_dim * 4;
    let embd_offset = embd_idx * embd_stride;
    if model.token_embd.len() < embd_offset + embd_stride {
        // The row the file does not hold is not invented here. A stand-in built
        // from the token id would let an answer be reported as the installed
        // model's while nothing of the model had been read.
        return Err(InferenceError::CorruptedWeights);
    }
    for j in 0..h_dim {
        let b_idx = embd_offset + j * 4;
        x[j] = f32::from_le_bytes([
            model.token_embd[b_idx],
            model.token_embd[b_idx + 1],
            model.token_embd[b_idx + 2],
            model.token_embd[b_idx + 3],
        ]);
    }

    // Scale embedding by sqrt(hidden_dim) (Gemma standard)
    let embd_scale = f32_sqrt(h_dim as f32);
    for v in x[..h_dim].iter_mut() {
        *v *= embd_scale;
    }

    // 2. Transformer layers
    let mut residual = [0.0f32; MAX_HIDDEN_DIM];
    let mut x_norm = [0.0f32; MAX_HIDDEN_DIM];
    let mut q = [0.0f32; MAX_HIDDEN_DIM];
    let mut k = [0.0f32; MAX_HIDDEN_DIM];
    let mut v_vec = [0.0f32; MAX_HIDDEN_DIM];
    let mut attn_out = [0.0f32; MAX_HIDDEN_DIM];
    let mut attn_proj = [0.0f32; MAX_HIDDEN_DIM];
    let mut gate = [0.0f32; MAX_INTERMEDIATE_DIM];
    let mut up = [0.0f32; MAX_INTERMEDIATE_DIM];
    let mut ffn_hidden = [0.0f32; MAX_INTERMEDIATE_DIM];
    let mut ffn_out = [0.0f32; MAX_HIDDEN_DIM];

    for l in 0..config.num_layers {
        if l >= MAX_LAYERS {
            break;
        }
        let lw = match &model.layers[l] {
            Some(w) => w,
            None => continue,
        };

        // Pre-Attention RMSNorm
        residual[..h_dim].copy_from_slice(&x[..h_dim]);
        x_norm[..h_dim].copy_from_slice(&x[..h_dim]);
        rms_norm(&mut x_norm[..h_dim], lw.attn_norm, config.rms_norm_eps);

        // Attention Q, K, V projections via GEMV
        gemv(lw.attn_q, lw.weight_type, &x_norm[..h_dim], &mut q[..h_dim], h_dim, h_dim)?;
        gemv(lw.attn_k, lw.weight_type, &x_norm[..h_dim], &mut k[..h_dim], h_dim, h_dim)?;
        gemv(lw.attn_v, lw.weight_type, &x_norm[..h_dim], &mut v_vec[..h_dim], h_dim, h_dim)?;

        // Apply RoPE
        for h in 0..config.num_heads {
            let head_start = h * config.head_dim;
            let head_end = head_start + config.head_dim;
            apply_rope(&mut q[head_start..head_end], pos, config.head_dim, config.rope_theta);
            apply_rope(&mut k[head_start..head_end], pos, config.head_dim, config.rope_theta);
        }

        // Cache K and V. The row is the whole declared head: `config.head_dim`
        // was checked against `MAX_HEAD_DIM` above, so nothing is dropped here.
        let hd = config.head_dim;
        kv_cache.k[l][pos][..hd].copy_from_slice(&k[..hd]);
        kv_cache.v[l][pos][..hd].copy_from_slice(&v_vec[..hd]);

        // Attention dot product & Softmax
        let scale_att = 1.0f32 / f32_sqrt(config.head_dim as f32);
        let mut scores = [0.0f32; MAX_SEQ_LEN];
        let seq_len_cur = pos + 1;

        // One key/value head, so every query head attends to the same cached
        // row; a file that declares more is refused above rather than read as
        // this one. The destination is cleared first: heads span at most
        // `num_heads x head_dim` of the residual stream, and whatever lies past
        // that belongs to this layer, not the last one.
        for v in attn_out[..h_dim].iter_mut() {
            *v = 0.0f32;
        }
        for h in 0..config.num_heads {
            let h_base = h * config.head_dim;
            for t in 0..seq_len_cur {
                let mut dot = 0.0f32;
                for d in 0..hd {
                    dot += q[h_base + d] * kv_cache.k[l][t][d];
                }
                scores[t] = dot * scale_att;
            }
            softmax(&mut scores[..seq_len_cur]);

            // Weighted sum of V
            for d in 0..hd {
                let mut v_sum = 0.0f32;
                for t in 0..seq_len_cur {
                    v_sum += scores[t] * kv_cache.v[l][t][d];
                }
                attn_out[h_base + d] = v_sum;
            }
        }

        // Attention output projection
        gemv(lw.attn_o, lw.weight_type, &attn_out[..h_dim], &mut attn_proj[..h_dim], h_dim, h_dim)?;
        rms_norm(&mut attn_proj[..h_dim], lw.post_attn_norm, config.rms_norm_eps);

        // Residual addition
        for i in 0..h_dim {
            x[i] = residual[i] + attn_proj[i];
        }

        // Pre-FFN RMSNorm
        residual[..h_dim].copy_from_slice(&x[..h_dim]);
        x_norm[..h_dim].copy_from_slice(&x[..h_dim]);
        rms_norm(&mut x_norm[..h_dim], lw.ffn_norm, config.rms_norm_eps);

        // FFN: Gate and Up projections
        let inter_dim = config.intermediate_dim;
        gemv(lw.ffn_gate, lw.weight_type, &x_norm[..h_dim], &mut gate[..inter_dim], inter_dim, h_dim)?;
        gemv(lw.ffn_up, lw.weight_type, &x_norm[..h_dim], &mut up[..inter_dim], inter_dim, h_dim)?;

        // SwiGLU activation
        swiglu(&gate[..inter_dim], &up[..inter_dim], &mut ffn_hidden[..inter_dim]);

        // Down projection
        gemv(lw.ffn_down, lw.weight_type, &ffn_hidden[..inter_dim], &mut ffn_out[..h_dim], h_dim, inter_dim)?;
        rms_norm(&mut ffn_out[..h_dim], lw.post_ffn_norm, config.rms_norm_eps);

        // Residual addition
        for i in 0..h_dim {
            x[i] = residual[i] + ffn_out[i];
        }
    }

    // 3. Final RMSNorm
    rms_norm(&mut x[..h_dim], model.final_norm, config.rms_norm_eps);

    // The normalised residual stream is what a pooling head consumes, so it
    // leaves through `out_hidden` rather than being re-derived from the logits
    // below. An empty slice means the caller only wanted the next token.
    if !out_hidden.is_empty() {
        out_hidden[..h_dim].copy_from_slice(&x[..h_dim]);
    }

    // 4. Output projection to compute logits
    let v_size = config.vocab_size.min(out_logits.len());
    gemv(
        model.output_weight,
        model.output_weight_type,
        &x[..h_dim],
        &mut out_logits[..v_size],
        v_size,
        h_dim,
    )?;

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
    let mut kv_cache = KvCache::empty();
    let mut logits = [0.0f32; MAX_VOCAB_SIZE];

    // 1. Pre-fill prompt tokens
    let prefill_len = prompt_tokens.len().min(MAX_SEQ_LEN);
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
            &mut logits[..config.vocab_size],
            &mut [],
        )?;
    }

    // 2. Sample first generated token from prompt's final logits
    let mut next_token = sample_token(&logits[..config.vocab_size], temperature);
    let mut generated_count = 0usize;
    let mut cur_pos = prefill_len;

    // 3. Autoregressive loop
    while generated_count < max_new_tokens && cur_pos < MAX_SEQ_LEN {
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
            &mut logits[..config.vocab_size],
            &mut [],
        )?;

        cur_pos += 1;
        next_token = sample_token(&logits[..config.vocab_size], temperature);
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
    let mut kv_cache = KvCache::empty();
    let mut logits = [0.0f32; MAX_VOCAB_SIZE];
    let count = tokens.len().min(MAX_SEQ_LEN);

    // Mean pooling over the positions, which is what an embedding file declares
    // when it states `pooling_type` 1. Each step's normalised residual stream is
    // summed as it is produced, so the vector is the model's own hidden
    // representation rather than a slice of the cache or a re-use of the logits.
    let dim = config.hidden_dim.min(out_embedding.len());
    let mut pooled = [0.0f32; MAX_HIDDEN_DIM];
    let mut hidden = [0.0f32; MAX_HIDDEN_DIM];

    for pos in 0..count {
        forward_step(
            tokens[pos],
            pos,
            model,
            config,
            &mut kv_cache,
            &mut logits[..config.vocab_size],
            &mut hidden,
        )?;
        for i in 0..dim {
            pooled[i] += hidden[i];
        }
    }

    let inv_count = 1.0f32 / (count as f32);
    for i in 0..dim {
        out_embedding[i] = pooled[i] * inv_count;
    }

    // L2-normalize vector
    let mut sum_sq = 0.0f32;
    for &v in out_embedding[..dim].iter() {
        sum_sq += v * v;
    }
    let norm = f32_sqrt(sum_sq);
    if norm > 1e-12 {
        let inv_norm = 1.0f32 / norm;
        for v in out_embedding[..dim].iter_mut() {
            *v *= inv_norm;
        }
    }

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
    struct LittleModel {
        ones: [f32; MAX_HIDDEN_DIM],
        id_w: [u8; 64 * 64 * 4],
        ffn_gate_w: [u8; 128 * 64 * 4],
        ffn_down_w: [u8; 64 * 128 * 4],
        head_w: [u8; 290 * 64 * 4],
        embd: [u8; 290 * 64 * 4],
    }

    impl LittleModel {
        fn new() -> Self {
            let mut model = Self {
                ones: [1.0f32; MAX_HIDDEN_DIM],
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

        fn weights(&self) -> ModelWeights<'_> {
            let layer = LayerWeights {
                attn_norm: &self.ones[..64],
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
            };
            ModelWeights {
                token_embd: &self.embd,
                token_embd_type: 0,
                layers: [Some(layer), None],
                final_norm: &self.ones[..64],
                output_weight: &self.head_w,
                output_weight_type: 0,
            }
        }
    }

    #[test]
    fn test_forward_step_and_generate_on_real_fixture() {
        let fixture = LittleModel::new();
        let config = LittleModel::config();
        let model = fixture.weights();

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
    /// stream, so it leaves through `out_hidden` instead of being re-derived from
    /// somewhere else. A destination too small for the stream the file declares is
    /// refused: a vector truncated to 8 dims is not that model's embedding.
    #[test]
    fn test_forward_step_hands_back_the_normalised_residual_stream() {
        let fixture = LittleModel::new();
        let config = LittleModel::config();
        let model = fixture.weights();
        let mut kv_cache = KvCache::empty();
        let mut logits = [0.0f32; MAX_VOCAB_SIZE];
        let mut hidden = [0.0f32; MAX_HIDDEN_DIM];

        forward_step(
            crate::tokenizer::TOKEN_BOS,
            0,
            &model,
            &config,
            &mut kv_cache,
            &mut logits[..config.vocab_size],
            &mut hidden,
        )
        .expect("the fixture holds the row this token asks for");

        assert!(
            hidden[..config.hidden_dim].iter().any(|&v| v != 0.0),
            "the residual stream left the step empty"
        );
        // This fixture's output head is the identity, so the logits are the stream
        // read straight through. Matching them dimension by dimension says the
        // vector handed back is the one the model's own head was fed, which is the
        // property a pooling head depends on.
        for i in 0..config.hidden_dim {
            assert!(
                (hidden[i] - logits[i]).abs() < 1e-6,
                "dim {} handed back {} while the output head was fed {}",
                i,
                hidden[i],
                logits[i]
            );
        }
        assert_eq!(kv_cache.len, 1);

        let mut too_narrow = [0.0f32; 8];
        assert_eq!(
            forward_step(
                crate::tokenizer::TOKEN_BOS,
                1,
                &model,
                &config,
                &mut kv_cache,
                &mut logits[..config.vocab_size],
                &mut too_narrow,
            ),
            Err(InferenceError::BufferTooSmall)
        );
        // Refused before the graph ran, so the cache is still what it was.
        assert_eq!(kv_cache.len, 1);
    }

    /// A declaration wider than a buffer is refused, never clipped to fit. An
    /// answer computed at 32 dims wide and reported as the answer of a model that
    /// declared 256 would be the engine describing a model it never ran.
    #[test]
    fn test_forward_step_refuses_a_shape_wider_than_its_buffers() {
        let empty = ModelWeights {
            token_embd: &[],
            token_embd_type: 0,
            layers: [None, None],
            final_norm: &[],
            output_weight: &[],
            output_weight_type: 0,
        };
        let base = LittleModel::config();
        let mut kv_cache = KvCache::empty();
        let mut logits = [0.0f32; MAX_VOCAB_SIZE];
        let no_out: &mut [f32] = &mut [];

        let wider_head = ModelConfig {
            head_dim: MAX_HEAD_DIM + 8,
            ..base
        };
        assert_eq!(
            forward_step(
                1,
                0,
                &empty,
                &wider_head,
                &mut kv_cache,
                &mut logits,
                no_out
            ),
            Err(InferenceError::BufferTooSmall)
        );

        let wider_ffn = ModelConfig {
            intermediate_dim: MAX_INTERMEDIATE_DIM + 1,
            ..base
        };
        assert_eq!(
            forward_step(1, 0, &empty, &wider_ffn, &mut kv_cache, &mut logits, no_out),
            Err(InferenceError::BufferTooSmall)
        );

        let shared_kv = ModelConfig {
            num_kv_heads: 2,
            ..base
        };
        assert_eq!(
            forward_step(1, 0, &empty, &shared_kv, &mut kv_cache, &mut logits, no_out),
            Err(InferenceError::BufferTooSmall)
        );

        let wider_stream = ModelConfig {
            hidden_dim: MAX_HIDDEN_DIM + 1,
            ..base
        };
        assert_eq!(
            forward_step(
                1,
                0,
                &empty,
                &wider_stream,
                &mut kv_cache,
                &mut logits,
                no_out
            ),
            Err(InferenceError::BufferTooSmall)
        );

        // Four heads of 32 do not tile a 64-wide stream, so the fourth would read
        // past the projection that filled it.
        let overhanging = ModelConfig {
            num_heads: 4,
            ..base
        };
        assert_eq!(
            forward_step(1, 0, &empty, &overhanging, &mut kv_cache, &mut logits, no_out),
            Err(InferenceError::BufferTooSmall)
        );
    }

    /// The row a token asks for either is in the file or is not. Building one
    /// from the token id made every number downstream of it a description of
    /// nothing, while reading a Q8_0 tensor as floats would read block headers
    /// as activations.
    #[test]
    fn test_forward_step_refuses_an_embedding_the_file_does_not_hold() {
        let fixture = LittleModel::new();
        let config = LittleModel::config();
        let mut no_rows = fixture.weights();
        no_rows.token_embd = &[];
        let mut kv_cache = KvCache::empty();
        let mut logits = [0.0f32; MAX_VOCAB_SIZE];
        let mut hidden = [0.0f32; MAX_HIDDEN_DIM];

        assert_eq!(
            forward_step(
                261,
                0,
                &no_rows,
                &config,
                &mut kv_cache,
                &mut logits[..config.vocab_size],
                &mut hidden,
            ),
            Err(InferenceError::CorruptedWeights)
        );

        // A table that stops inside the last row is the same lie as no table at
        // all: the row is read as far as it exists and the rest would be invented.
        let mut truncated = fixture.weights();
        let one_row_short = fixture.embd.len() - 4;
        truncated.token_embd = &fixture.embd[..one_row_short];
        assert_eq!(
            forward_step(
                289,
                0,
                &truncated,
                &config,
                &mut kv_cache,
                &mut logits[..config.vocab_size],
                &mut hidden,
            ),
            Err(InferenceError::CorruptedWeights)
        );

        let mut quantised = fixture.weights();
        quantised.token_embd_type = 8; // Q8_0: 34 bytes per 32 values, not floats
        assert_eq!(
            forward_step(
                261,
                0,
                &quantised,
                &config,
                &mut kv_cache,
                &mut logits[..config.vocab_size],
                &mut hidden,
            ),
            Err(InferenceError::UnsupportedQuantization(8))
        );
    }

    /// What the search index stores is the mean of the positions the model ran,
    /// L2-normalised. Neither the last row of the KV cache nor a slice of the
    /// logits is a sentence embedding, so this checks the pooled vector against
    /// the same positions run one at a time.
    #[test]
    fn test_extract_embedding_mean_pools_the_positions_and_normalises_them() {
        let fixture = LittleModel::new();
        let config = LittleModel::config();
        let model = fixture.weights();
        let tokens = [crate::tokenizer::TOKEN_BOS, 261, 4];
        let mut embedding = [0.0f32; MAX_HIDDEN_DIM];

        let dim = extract_embedding(&tokens, &model, &config, &mut embedding)
            .expect("the fixture is a runnable model");
        assert_eq!(dim, config.hidden_dim);

        let mut kv_cache = KvCache::empty();
        let mut logits = [0.0f32; MAX_VOCAB_SIZE];
        let mut hidden = [0.0f32; MAX_HIDDEN_DIM];
        let mut summed = [0.0f32; MAX_HIDDEN_DIM];
        for (pos, &token) in tokens.iter().enumerate() {
            forward_step(
                token,
                pos,
                &model,
                &config,
                &mut kv_cache,
                &mut logits[..config.vocab_size],
                &mut hidden,
            )
            .expect("the same tokens run the same graph");
            for i in 0..dim {
                summed[i] += hidden[i];
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
        info.architecture_len = arch.len();
        info.hidden_dim = Some(hidden);
        info.intermediate_dim = Some(ffn);
        info.num_layers = Some(layers);
        info.num_heads = Some(heads);
        info.head_dim = Some(head_dim);
        info.vocab_size = Some(vocab);
        info
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
    }

    #[test]
    fn test_model_config_refuses_a_dimension_the_engine_cannot_hold() {
        // The real Gemma 2B: 2304 wide over 26 blocks with a 256,000 vocabulary.
        let info = declared(2304, 4608, 26, 8, 256, 256_000);
        assert_eq!(
            ModelConfig::from_gguf(&info),
            Err(InferenceError::ModelDeclaration(
                "<arch>.embedding_length",
                Some(2304),
                MAX_HIDDEN_DIM
            ))
        );
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
                MAX_LAYERS
            ))
        );

        let mut zeroed = declared(64, 128, 0, 2, 32, 290);
        zeroed.hidden_dim = Some(0);
        assert_eq!(
            ModelConfig::from_gguf(&zeroed),
            Err(InferenceError::ModelDeclaration(
                "<arch>.embedding_length",
                Some(0),
                MAX_HIDDEN_DIM
            ))
        );
    }

    #[test]
    fn test_model_config_refuses_heads_that_overrun_the_residual_stream() {
        // The head loop walks num_heads slices of head_dim out of a buffer that is
        // hidden_dim long, so four heads of 32 do not fit inside 64.
        let info = declared(64, 128, 1, 4, 32, 290);
        assert_eq!(
            ModelConfig::from_gguf(&info),
            Err(InferenceError::ModelDeclaration(
                "<arch>.attention.head_count x key_length",
                Some(128),
                64
            ))
        );
    }

    #[test]
    fn test_model_config_bounds_a_head_wider_than_the_kv_cache_row() {
        let info = declared(64, 128, 1, 1, 64, 290);
        assert_eq!(
            ModelConfig::from_gguf(&info),
            Err(InferenceError::ModelDeclaration(
                "<arch>.attention.key_length",
                Some(64),
                MAX_HEAD_DIM
            ))
        );
    }

    /// The numbers recorded from the published `embeddinggemma-300M-Q8_0.gguf`
    /// (hidden 768, feed-forward 1152, 24 blocks, 3 heads 256 wide, 262,144
    /// tokens), held here so the refusal this engine owes a real model is a test
    /// and not a claim.
    ///
    /// WHY the first bound is the one named: `from_gguf` checks in the order the
    /// graph is built, and the residual stream is the widest array the engine
    /// sizes -- so the person is told the declaration that actually binds them
    /// first, with the file's own number next to the engine's own limit.
    #[test]
    fn test_model_config_refuses_the_published_embedding_model_by_its_bound() {
        let info = declared(768, 1152, 24, 3, 256, 262_144);
        assert_eq!(
            ModelConfig::from_gguf(&info),
            Err(InferenceError::ModelDeclaration(
                "<arch>.embedding_length",
                Some(768),
                MAX_HIDDEN_DIM
            ))
        );
    }
}
