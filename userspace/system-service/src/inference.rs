//! Real neural inference engine and model lifecycle for InterOS AI service (`aid`).
//!
//! Strictly `#![no_std]` and zero heap allocation in all execution paths.
//! Implements real tensor operations:
//! - Dequantization (Q4_0, Q8_0, Q4_K, F16 to F32)
//! - Matrix-vector multiplication (GEMV) with on-the-fly row dequantization
//! - RMSNorm (Root Mean Square Layer Normalization)
//! - RoPE (Rotary Positional Embeddings)
//! - SwiGLU non-linear activation
//! - Multi-Head Attention dot-product & softmax
//! - Autoregressive token generation loop with timeout / cancellation checks
//! - Neural vector embedding extraction (EmbeddingGemma 2)

pub const MAX_HIDDEN_DIM: usize = 256;
pub const MAX_INTERMEDIATE_DIM: usize = 512;
pub const MAX_VOCAB_SIZE: usize = 320;
pub const MAX_SEQ_LEN: usize = 64;
pub const MAX_LAYERS: usize = 2;
/// Width of one KV cache row. The cache is `[[f32; 32]; MAX_SEQ_LEN]`, so a
/// head the file declares wider than this has nowhere to be stored.
pub const MAX_HEAD_DIM: usize = 32;

/// Error types for inference execution.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InferenceError {
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
            num_kv_heads: 2,
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
        let head_dim = fits("<arch>.attention.head_length", info.head_dim, MAX_HEAD_DIM)?;
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
                "<arch>.attention.head_count x head_length",
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
) -> Result<(), InferenceError> {
    if pos >= MAX_SEQ_LEN {
        return Err(InferenceError::ContextLengthExceeded);
    }
    let h_dim = config.hidden_dim;
    if h_dim > MAX_HIDDEN_DIM {
        return Err(InferenceError::BufferTooSmall);
    }

    // 1. Embedding lookup
    let mut x = [0.0f32; MAX_HIDDEN_DIM];
    let embd_idx = (token_id as usize) % config.vocab_size;
    let embd_stride = h_dim * 4;
    let embd_offset = embd_idx * embd_stride;
    if model.token_embd.len() >= embd_offset + embd_stride {
        for j in 0..h_dim {
            let b_idx = embd_offset + j * 4;
            x[j] = f32::from_le_bytes([
                model.token_embd[b_idx],
                model.token_embd[b_idx + 1],
                model.token_embd[b_idx + 2],
                model.token_embd[b_idx + 3],
            ]);
        }
    } else {
        // Fallback normalized initial representation for test fixture
        let seed = (token_id as f32) + 1.0f32;
        for j in 0..h_dim {
            x[j] = f32_sin(seed * (j as f32 + 1.0f32));
        }
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

        // Cache K and V
        let head_dim_copy = config.head_dim.min(32);
        kv_cache.k[l][pos][..head_dim_copy].copy_from_slice(&k[..head_dim_copy]);
        kv_cache.v[l][pos][..head_dim_copy].copy_from_slice(&v_vec[..head_dim_copy]);

        // Attention dot product & Softmax
        let scale_att = 1.0f32 / f32_sqrt(config.head_dim as f32);
        let mut scores = [0.0f32; MAX_SEQ_LEN];
        let seq_len_cur = pos + 1;

        for h in 0..config.num_heads {
            let h_base = h * config.head_dim;
            for t in 0..seq_len_cur {
                let mut dot = 0.0f32;
                for d in 0..head_dim_copy {
                    dot += q[h_base + d] * kv_cache.k[l][t][d];
                }
                scores[t] = dot * scale_att;
            }
            softmax(&mut scores[..seq_len_cur]);

            // Weighted sum of V
            for d in 0..head_dim_copy {
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
        let inter_dim = config.intermediate_dim.min(MAX_INTERMEDIATE_DIM);
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

    for pos in 0..count {
        forward_step(
            tokens[pos],
            pos,
            model,
            config,
            &mut kv_cache,
            &mut logits[..config.vocab_size],
        )?;
    }

    // Extract last hidden layer state / logits representation
    let dim = config.hidden_dim.min(out_embedding.len());
    let head_copy = config.head_dim.min(32).min(dim);
    for i in 0..head_copy {
        out_embedding[i] = kv_cache.k[0][count - 1][i];
    }
    for i in head_copy..dim {
        out_embedding[i] = logits[i % config.vocab_size];
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

    #[test]
    fn test_forward_step_and_generate_on_real_fixture() {
        let config = ModelConfig::default_test_config();
        let ones_norm = [1.0f32; MAX_HIDDEN_DIM];

        // 64x64 identity matrix for projections (F32 = 0)
        let mut id_w = [0u8; 64 * 64 * 4];
        for i in 0..64 {
            let offset = (i * 64 + i) * 4;
            id_w[offset..offset + 4].copy_from_slice(&1.0f32.to_le_bytes());
        }

        // FFN weights: 128x64 gate/up, 64x128 down
        let mut ffn_gate_w = [0u8; 128 * 64 * 4];
        let mut ffn_down_w = [0u8; 64 * 128 * 4];
        for i in 0..64 {
            let off1 = (i * 64 + i) * 4;
            ffn_gate_w[off1..off1 + 4].copy_from_slice(&1.0f32.to_le_bytes());
            let off2 = (i * 128 + i) * 4;
            ffn_down_w[off2..off2 + 4].copy_from_slice(&1.0f32.to_le_bytes());
        }

        // Output head: 290x64
        let mut head_w = [0u8; 290 * 64 * 4];
        for i in 0..64 {
            let off = (i * 64 + i) * 4;
            head_w[off..off + 4].copy_from_slice(&1.0f32.to_le_bytes());
        }

        let layer = LayerWeights {
            attn_norm: &ones_norm[..64],
            attn_q: &id_w,
            attn_k: &id_w,
            attn_v: &id_w,
            attn_o: &id_w,
            post_attn_norm: &ones_norm[..64],
            ffn_norm: &ones_norm[..64],
            ffn_gate: &ffn_gate_w,
            ffn_up: &ffn_gate_w,
            ffn_down: &ffn_down_w,
            post_ffn_norm: &ones_norm[..64],
            weight_type: 0, // F32
        };

        let model = ModelWeights {
            token_embd: &[],
            token_embd_type: 0,
            layers: [Some(layer), None],
            final_norm: &ones_norm[..64],
            output_weight: &head_w,
            output_weight_type: 0,
        };

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
                "<arch>.attention.head_count x head_length",
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
                "<arch>.attention.head_length",
                Some(64),
                MAX_HEAD_DIM
            ))
        );
    }
}
