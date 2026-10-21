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
/// WHY the id is 13: the platform defines services 1 through 12, and an
/// inference service is the next one. The id is a name, not a claim that
/// such a service exists.
pub const SERVICE_INFERENCE: u8 = 13;

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
