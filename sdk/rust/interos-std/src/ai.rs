//! On-Device Local AI & EmbeddingGemma 2 Vector Primitives
//!
//! Zero-allocation `#![no_std]` data structures and integer fixed-point vector
//! arithmetic for on-device multimodal embeddings (Google EmbeddingGemma 2)
//! and Gemma 4 local inference daemon (`aid`).
//!
//! Supports Matryoshka Representation Learning (MRL) dimension truncations
//! (128, 256, 512, 768) and Q7 fixed-point cosine similarity calculation.

// -----------------------------------------------------------------------------
// The inference service interface
// -----------------------------------------------------------------------------

/// The version of the inference service interface this SDK speaks.
///
/// WHY a version at all: the platform's service registry reports an ABI
/// version for every service it knows, so a service can change its
/// contract without the device pretending it did not. An app that speaks
/// one version can be told that the service speaks another, which is a
/// fact it can report, rather than sending a request that will be
/// misread.
pub const INFERENCE_ABI_VERSION: u16 = 1;

/// The service id the platform's inference service answers to.
///
/// WHY a service id and not a new kind of object: the platform's service
/// registry already carries an id, a state and an ABI version per
/// service. A narrow, versioned service interface therefore needs
/// nothing but an id and a version to be discoverable, and it is
/// discoverable by the same call that finds every other service.
///
/// WHY the id is 12: the platform defines services 0 through 11, and an
/// inference service is the next one. The id is a name, not a claim that
/// such a service exists.
pub const SERVICE_INFERENCE: u8 = 12;

/// Virtual TaskId used to route IPC directly to the system inference service (`aid`, Service #12).
pub const SERVICE_INFERENCE_TASK_ID: u64 = 0xFFFF_000C;

/// Virtual TaskId used to route IPC directly to the semantic search service (Service #13).
pub const SERVICE_SEARCH_TASK_ID: u64 = 0xFFFF_000D;

/// IPC message type for vector embedding queries: `0x454D_4244` (ASCII `b"EMBD"`).
pub const MSG_TYPE_EMBED: u32 = 0x454D_4244;

/// IPC message type for GGUF model metadata / header queries: `0x4747_5546` (ASCII `b"GGUF"`).
pub const MSG_TYPE_GGUF: u32 = 0x4747_5546;

/// IPC message type for generative token stream queries: `0x4745_4E52` (ASCII `b"GENR"`).
pub const MSG_TYPE_GENERATE: u32 = 0x4745_4E52;

/// Alternate ASCII alias for generative queries: `0x4745_4E45` (ASCII `b"GENE"`).
pub const MSG_TYPE_GENERATE_ALT: u32 = 0x4745_4E45;

/// IPC message type for semantic search & RAG queries: `0x5352_4348` (ASCII `b"SRCH"`).
pub const MSG_TYPE_SEARCH: u32 = 0x5352_4348;

/// IPC message type for streaming generation chunks: `0x4745_4E43` (ASCII `b"GENC"`).
pub const MSG_TYPE_GENERATE_CHUNK: u32 = 0x4745_4E43;

/// IPC message type for cancelling active streaming generation: `0x4341_4E43` (ASCII `b"CANC"`).
pub const MSG_TYPE_CANCEL: u32 = 0x4341_4E43;

/// Status code: Success.
pub const STATUS_SUCCESS: u32 = 0;
/// Status code: Model weights unavailable or absent from storage.
pub const STATUS_UNAVAILABLE: u32 = 1;
/// Status code: Execution error (e.g. corrupted tensor or internal failure).
pub const STATUS_ERROR: u32 = 2;
/// Status code: Unsupported architecture or quantization format.
pub const STATUS_UNSUPPORTED: u32 = 3;
/// Status code: Generation cancelled or timed out.
pub const STATUS_CANCELLED: u32 = 4;

/// Valid Matryoshka representation dimensions for EmbeddingGemma 2.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum MatryoshkaDim {
    Dim128 = 128,
    Dim256 = 256,
    Dim512 = 512,
    Dim768 = 768,
}

impl MatryoshkaDim {
    pub const fn as_usize(self) -> usize {
        self as usize
    }

    pub const fn from_u32(val: u32) -> Option<Self> {
        match val {
            128 => Some(Self::Dim128),
            256 => Some(Self::Dim256),
            512 => Some(Self::Dim512),
            768 => Some(Self::Dim768),
            _ => None,
        }
    }
}

/// Computes integer square root via binary restoration / Newton's method.
pub const fn integer_sqrt(val: u64) -> u64 {
    if val == 0 {
        return 0;
    }
    let mut x0 = val / 2;
    if x0 == 0 {
        return 1;
    }
    let mut x1 = (x0 + val / x0) / 2;
    while x1 < x0 {
        x0 = x1;
        x1 = (x0 + val / x0) / 2;
    }
    x0
}

/// Computes the cosine similarity between two unit-normalized Q7 quantized vectors.
///
/// Returns a Q15 fixed-point integer in range -32768 to 32767 representing -1.0 to +1.0.
/// Does not allocate any heap memory. Truncates comparison to the shortest vector length
/// or matching Matryoshka dimension.
pub fn cosine_similarity_q7(a: &[i8], b: &[i8]) -> i16 {
    let len = if a.len() < b.len() { a.len() } else { b.len() };
    if len == 0 {
        return 0;
    }

    let mut dot: i64 = 0;
    let mut norm_a: u64 = 0;
    let mut norm_b: u64 = 0;

    for i in 0..len {
        let va = a[i] as i64;
        let vb = b[i] as i64;
        dot += va * vb;
        norm_a += (va * va) as u64;
        norm_b += (vb * vb) as u64;
    }

    if norm_a == 0 || norm_b == 0 {
        return 0;
    }

    let denom = integer_sqrt(norm_a * norm_b);
    if denom == 0 {
        return 0;
    }

    let scaled = (dot * 32767) / (denom as i64);
    if scaled > 32767 {
        32767
    } else if scaled < -32768 {
        -32768
    } else {
        scaled as i16
    }
}

/// Software single-precision floating-point square root via Newton-Raphson.
pub fn f32_sqrt(val: f32) -> f32 {
    if val <= 0.0 {
        return 0.0;
    }
    let i = val.to_bits();
    let i = (i >> 1) + 0x1fbb4000;
    let mut x = f32::from_bits(i);
    x = 0.5 * (x + val / x);
    x = 0.5 * (x + val / x);
    x = 0.5 * (x + val / x);
    x = 0.5 * (x + val / x);
    x
}

#[inline]
fn hash_bytes_with_seed(bytes: &[u8], seed: u64) -> u64 {
    let mut h = seed;
    for &b in bytes {
        let c = b.to_ascii_lowercase();
        h ^= c as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    h ^= h >> 30;
    h = h.wrapping_mul(0xbf58476d1ce4e5b9);
    h ^= h >> 27;
    h = h.wrapping_mul(0x94d049bb133111eb);
    h ^= h >> 31;
    h
}

const PROJECTION_SEEDS: [u64; 12] = [
    0x517cc1b727220a95,
    0x6c62272e07bb0142,
    0x9e3779b97f4a7c15,
    0x243f6a8885a308d3,
    0x13198a2e03707344,
    0xa4093822299f31d0,
    0x712feb58476d1ce4,
    0x3c6ef372fe94f82b,
    0xbb67ae8584caa73b,
    0x9b05688c2b3e6c1f,
    0x1f83d9abfb41bd6b,
    0x5be0cd19137e2179,
];

#[inline]
fn is_token_boundary(b: u8) -> bool {
    matches!(
        b,
        b' ' | b'\t' | b'\n' | b'\r' | b'.' | b',' | b';' | b':' | b'!' | b'?' | b'/' | b'\\'
            | b'-' | b'_' | b'(' | b')' | b'[' | b']' | b'{' | b'}' | b'"' | b'\''
    )
}

fn project_token(token: &[u8], token_idx: usize, out: &mut [f32], dim: usize) {
    if token.is_empty() || dim == 0 {
        return;
    }
    let pos_weight = 1.0f32 / (1.0f32 + 0.03f32 * (token_idx as f32));
    for &seed in &PROJECTION_SEEDS {
        let h = hash_bytes_with_seed(token, seed);
        let idx = (h as usize) % dim;
        let sign = if (h >> 32) & 1 == 0 { 1.0f32 } else { -1.0f32 };
        out[idx] += sign * pos_weight;
    }
    if token.len() >= 2 {
        for j in 0..=token.len() - 2 {
            let bigram = &token[j..j + 2];
            let h = hash_bytes_with_seed(bigram, 0x3c6ef372fe94f82b);
            let idx = (h as usize) % dim;
            let sign = if (h >> 32) & 1 == 0 { 0.35f32 } else { -0.35f32 };
            out[idx] += sign * pos_weight;
        }
    }
    if token.len() >= 3 {
        for j in 0..=token.len() - 3 {
            let trigram = &token[j..j + 3];
            let h = hash_bytes_with_seed(trigram, 0x9e3779b97f4a7c15);
            let idx = (h as usize) % dim;
            let sign = if (h >> 32) & 1 == 0 { 0.5f32 } else { -0.5f32 };
            out[idx] += sign * pos_weight;
        }
    }
}

/// Computes an L2-normalized float embedding vector for arbitrary target dimensions.
pub fn compute_text_embedding_dim_f32(text: &[u8], out: &mut [f32], target_dim: usize) {
    let dim = target_dim.min(out.len());
    if dim == 0 {
        return;
    }
    for v in out[..dim].iter_mut() {
        *v = 0.0;
    }
    if text.is_empty() {
        return;
    }
    let mut token_start = 0usize;
    let mut in_token = false;
    let mut token_index = 0usize;
    let len = text.len();
    for i in 0..=len {
        let is_end = i == len || is_token_boundary(text[i]);
        if is_end {
            if in_token {
                let token = &text[token_start..i];
                project_token(token, token_index, out, dim);
                token_index = token_index.saturating_add(1);
                in_token = false;
            }
        } else if !in_token {
            token_start = i;
            in_token = true;
        }
    }
    let mut sum_sq = 0.0f32;
    for &v in out[..dim].iter() {
        sum_sq += v * v;
    }
    let norm = f32_sqrt(sum_sq);
    if norm > 1e-12 {
        let inv_norm = 1.0 / norm;
        for v in out[..dim].iter_mut() {
            *v *= inv_norm;
        }
    }
}

/// Quantizes an L2-normalized float embedding slice into Q7 fixed-point representation (`[-127, 127]`).
pub fn quantize_f32_to_q7(f32_vec: &[f32], q7_out: &mut [i8]) {
    let len = f32_vec.len().min(q7_out.len());
    for i in 0..len {
        let scaled = f32_vec[i] * 127.0f32;
        let rounded = if scaled >= 0.0 { scaled + 0.5f32 } else { scaled - 0.5f32 };
        let clamped = if rounded > 127.0 { 127.0 } else if rounded < -127.0 { -127.0 } else { rounded };
        q7_out[i] = clamped as i8;
    }
}

/// Computes a normalized Q7 fixed-point embedding directly from input text into `out`.
pub fn compute_text_embedding_q7(text: &[u8], out: &mut [i8]) {
    let dim = out.len().min(768);
    let mut temp = [0.0f32; 768];
    compute_text_embedding_dim_f32(text, &mut temp[..dim], dim);
    quantize_f32_to_q7(&temp[..dim], &mut out[..dim]);
}

/// Zero-copy wire descriptor for sending inference requests to Ring-3 `aid`.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AiInferenceRequest {
    /// Target model identifier (e.g., 1 = EmbeddingGemma 2 740M, 2 = Gemma 4 2B).
    pub model_id: u32,
    /// Requested output Matryoshka dimension (128, 256, 512, 768).
    pub target_dim: u32,
    /// Input byte length in shared memory buffer.
    pub input_len: u32,
    /// Modality and execution flags (bit 0: text, bit 1: image, bit 2: audio).
    pub flags: u32,
    /// Byte offset into the shared memory zero-copy DMA ring pipe.
    pub input_offset: u64,
}

/// Zero-copy wire descriptor for receiving inference responses from Ring-3 `aid`.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AiInferenceResponse {
    /// Execution status code (0 = Success, non-zero = error).
    pub status: u32,
    /// Resulting vector dimension produced.
    pub output_dim: u32,
    /// Cold/warm inference latency in microseconds.
    pub latency_micros: u64,
    /// Byte offset into shared memory holding the output embedding vector.
    pub output_offset: u64,
}

/// The first shared-memory channel id a service may claim for its own answers.
///
/// WHY the number is published: the platform compiles the channels below this one
/// in at boot, so a service that picks an id by guesswork can only guess wrong,
/// and a wrong guess is a refused transfer it cannot tell the person about.
pub const ANSWER_PIPE_FIRST: u32 = 5;

/// How many answer channels exist at once, which is how many apps the device can
/// be carrying an answer for at the same moment.
pub const ANSWER_PIPE_COUNT: u32 = 3;

/// The most one ring slot of an answer channel carries.
pub const ANSWER_SLOT_BYTES: usize = 1024;

/// No channel: the answer could not be streamed at all.
pub const NO_ANSWER_PIPE: u32 = u32::MAX;

/// The notice that says an answer is waiting on a shared-memory channel.
///
/// WHY the answer does not travel by IPC: a mailbox message carries 64 bytes, and
/// a model's answer is not 64 bytes. Sent through the mailbox alone, everything
/// past the 64th byte is dropped by the kernel while the response header goes on
/// reporting the full length. The bytes therefore go over the channel the platform
/// already provides, and this short pointer is what travels by IPC: which channel,
/// how many bytes, and whether the answer succeeded at all.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AiStreamNotice {
    /// Execution status, using the same codes as [`AiInferenceResponse`].
    pub status: u32,
    /// The channel the answer was streamed onto, or [`NO_ANSWER_PIPE`].
    pub pipe_id: u32,
    /// Bytes the service actually handed to the channel.
    pub streamed_len: u32,
    /// The answer's full length, which can exceed what the channel could carry.
    pub total_len: u32,
}

impl AiStreamNotice {
    /// The notice's length on the wire.
    pub const WIRE_LEN: usize = 16;

    /// Writes the notice into `out` and reports how many bytes it took.
    ///
    /// Returns zero when the buffer cannot hold a notice, because a half-written
    /// pointer is worse than none: the reader would act on a channel id it invented.
    pub fn to_bytes(&self, out: &mut [u8]) -> usize {
        if out.len() < Self::WIRE_LEN {
            return 0;
        }
        out[0..4].copy_from_slice(&self.status.to_le_bytes());
        out[4..8].copy_from_slice(&self.pipe_id.to_le_bytes());
        out[8..12].copy_from_slice(&self.streamed_len.to_le_bytes());
        out[12..16].copy_from_slice(&self.total_len.to_le_bytes());
        Self::WIRE_LEN
    }

    /// Reads a notice, or reports that the bytes are not one.
    pub fn from_bytes(bytes: &[u8]) -> Option<Self> {
        if bytes.len() < Self::WIRE_LEN {
            return None;
        }
        let read = |offset: usize| u32::from_le_bytes([bytes[offset], bytes[offset + 1], bytes[offset + 2], bytes[offset + 3]]);
        Some(Self {
            status: read(0),
            pipe_id: read(4),
            streamed_len: read(8),
            total_len: read(12),
        })
    }
}

/// Zero-copy wire descriptor for sending semantic search & RAG queries to Ring-3 `aid`.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AiSearchRequest {
    /// Target Matryoshka dimension (128, 256, 512, 768).
    pub target_dim: u32,
    /// Maximum top-K matches to return (e.g. 3, 5).
    pub top_k: u32,
    /// Minimum similarity confidence threshold in percent (0-100%).
    pub min_confidence: u32,
    /// Input query byte length following this header.
    pub query_len: u32,
}

/// Zero-copy wire descriptor for a single RAG search hit returned from `aid`.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AiSearchHit {
    /// Stable 64-bit deterministic chunk ID.
    pub chunk_id: u64,
    /// Start character/byte offset in source document.
    pub start_offset: u32,
    /// End character/byte offset in source document.
    pub end_offset: u32,
    /// Q15 cosine similarity score (-32768 to 32767).
    pub score_q15: i16,
    /// Normalized confidence percentage (0 to 100).
    pub confidence_percent: u8,
    /// Platform Truth Contract: 1 if physical neural weights were used, 0 if lexical fallback.
    pub is_neural: u8,
    /// Byte length of the source document path string following this hit.
    pub path_len: u16,
    /// Byte length of the text snippet following this hit.
    pub snippet_len: u16,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_matryoshka_dimensions() {
        assert_eq!(MatryoshkaDim::from_u32(128), Some(MatryoshkaDim::Dim128));
        assert_eq!(MatryoshkaDim::from_u32(256), Some(MatryoshkaDim::Dim256));
        assert_eq!(MatryoshkaDim::from_u32(512), Some(MatryoshkaDim::Dim512));
        assert_eq!(MatryoshkaDim::from_u32(768), Some(MatryoshkaDim::Dim768));
        assert_eq!(MatryoshkaDim::from_u32(1024), None);
    }

    #[test]
    fn test_cosine_similarity() {
        let v1 = [64i8; 128];
        let v2 = [64i8; 128];
        // Identical vectors -> similarity ~ 1.0 (32767)
        let sim = cosine_similarity_q7(&v1, &v2);
        assert_eq!(sim, 32767);

        let mut v3 = [0i8; 128];
        for i in 0..128 {
            v3[i] = -v1[i];
        }
        // Opposite vectors -> similarity ~ -1.0 (-32767)
        let sim_opp = cosine_similarity_q7(&v1, &v3);
        assert_eq!(sim_opp, -32767);

        let mut v_orth = [0i8; 128];
        for i in 0..64 {
            v_orth[i] = 64;
        }
        for i in 64..128 {
            v_orth[i] = -64;
        }
        // Dot product of [64; 128] and [64.. -64..] is 64*64*64 - 64*64*64 = 0
        let sim_orth = cosine_similarity_q7(&v1, &v_orth);
        assert_eq!(sim_orth, 0);
    }
}
