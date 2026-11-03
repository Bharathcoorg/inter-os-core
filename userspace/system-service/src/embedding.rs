// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

//! Lexical n-gram feature projection baseline for InterOS on-device text search
//! and semantic indexing (`aid`).
//!
//! Strictly `#![no_std]` and zero heap allocation in all execution loops.
//! Computes L2-normalized 768-dimensional float and fixed-point (Q7) lexical embedding
//! vectors compatible with Matryoshka representation dimensions (128, 256, 512, 768)
//! and `interos_std::ai::cosine_similarity_q7`.
//!
//! Note: This is a fast, deterministic lexical feature hashing algorithm and baseline,
//! not physical neural EmbeddingGemma 2 model weights.

#[cfg(test)]
use interos_std as std;

/// EmbeddingGemma 2 canonical full embedding vector dimension.
pub const EMBEDDING_DIM: usize = 768;

/// Supported Matryoshka sub-dimensions.
pub const MATRYOSHKA_DIMS: [usize; 4] = [128, 256, 512, 768];

/// Computes software single-precision floating-point square root via Newton-Raphson.
///
/// Fully `#![no_std]` compliant without linking `libm` or external math runtimes.
pub fn f32_sqrt(val: f32) -> f32 {
    if val <= 0.0 {
        return 0.0;
    }
    // Bit-level initial reciprocal square root guess (fast inverse sqrt style)
    let i = val.to_bits();
    let i = (i >> 1) + 0x1fbb4000;
    let mut x = f32::from_bits(i);

    // 4 iterations of Newton-Raphson refinement: x_{n+1} = 0.5 * (x_n + val / x_n)
    x = 0.5 * (x + val / x);
    x = 0.5 * (x + val / x);
    x = 0.5 * (x + val / x);
    x = 0.5 * (x + val / x);
    x
}

/// 64-bit FNV-1a hash with SplitMix64 avalanche permutation.
#[inline]
fn hash_bytes_with_seed(bytes: &[u8], seed: u64) -> u64 {
    let mut h = seed;
    for &b in bytes {
        let c = b.to_ascii_lowercase();
        h ^= c as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    // SplitMix64 finalizer
    h ^= h >> 30;
    h = h.wrapping_mul(0xbf58476d1ce4e5b9);
    h ^= h >> 27;
    h = h.wrapping_mul(0x94d049bb133111eb);
    h ^= h >> 31;
    h
}

/// Prime seed constants for multi-hash feature projection.
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

/// Checks whether a byte is an ASCII token boundary (whitespace or punctuation).
#[inline]
fn is_token_boundary(b: u8) -> bool {
    matches!(
        b,
        b' ' | b'\t' | b'\n' | b'\r' | b'.' | b',' | b';' | b':' | b'!' | b'?' | b'/' | b'\\'
            | b'-' | b'_' | b'(' | b')' | b'[' | b']' | b'{' | b'}' | b'"' | b'\''
    )
}

/// Computes an L2-normalized float embedding vector for arbitrary target dimensions.
///
/// Truncates to `target_dim` (e.g. 128, 256, 512, 768) and L2-normalizes the vector.
pub fn compute_text_embedding_dim_f32(text: &[u8], out: &mut [f32], target_dim: usize) {
    let dim = target_dim.min(out.len()).min(EMBEDDING_DIM);
    if dim == 0 {
        return;
    }

    // Zero out the target slice
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

    // L2-normalize the output vector
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

/// Projects a single token into the embedding vector using multi-hash feature mapping
/// and subword character n-grams.
#[inline]
fn project_token(token: &[u8], token_idx: usize, out: &mut [f32], dim: usize) {
    if token.is_empty() || dim == 0 {
        return;
    }

    // Position-dependent soft weighting: earlier words have slightly higher salience
    let pos_weight = 1.0f32 / (1.0f32 + 0.03f32 * (token_idx as f32));

    // 1. Whole-word multi-hash projection
    for &seed in &PROJECTION_SEEDS {
        let h = hash_bytes_with_seed(token, seed);
        let idx = (h as usize) % dim;
        let sign = if (h >> 32) & 1 == 0 { 1.0f32 } else { -1.0f32 };
        out[idx] += sign * pos_weight;
    }

    // 2. Character subword n-gram feature projection for morphological resilience
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

/// Quantizes an L2-normalized float embedding slice into Q7 fixed-point representation (`[-127, 127]`).
pub fn quantize_f32_to_q7(f32_vec: &[f32], q7_out: &mut [i8]) {
    let len = f32_vec.len().min(q7_out.len());
    for i in 0..len {
        let scaled = f32_vec[i] * 127.0f32;
        let rounded = if scaled >= 0.0 {
            scaled + 0.5f32
        } else {
            scaled - 0.5f32
        };
        let clamped = if rounded > 127.0 {
            127.0
        } else if rounded < -127.0 {
            -127.0
        } else {
            rounded
        };
        q7_out[i] = clamped as i8;
    }
}

/// Computes a normalized 768-dimensional L2-normalized float embedding from an input text query.
///
/// Guaranteed zero heap allocations and `#![no_std]`.
pub fn compute_text_embedding_f32(text: &[u8], out: &mut [f32; EMBEDDING_DIM]) {
    compute_text_embedding_dim_f32(text, out, EMBEDDING_DIM);
}

/// Computes a normalized 768-dimensional L2-normalized Q7 fixed-point embedding from an input text query.
///
/// Output matches `interos_std::ai::cosine_similarity_q7` conventions.
/// Guaranteed zero heap allocations and `#![no_std]`.
pub fn compute_text_embedding_q7(text: &[u8], out: &mut [i8; EMBEDDING_DIM]) {
    let mut temp = [0.0f32; EMBEDDING_DIM];
    compute_text_embedding_f32(text, &mut temp);
    quantize_f32_to_q7(&temp, out);
}

/// Universal embedding generator supporting selectable dimensions (128, 256, 512, 768)
/// and writing to optional float and/or Q7 fixed-point buffers.
pub fn compute_embedding_768(
    text: &[u8],
    target_dim: usize,
    out_f32: Option<&mut [f32]>,
    out_q7: Option<&mut [i8]>,
) {
    let dim = target_dim.clamp(1, EMBEDDING_DIM);
    let mut temp = [0.0f32; EMBEDDING_DIM];
    compute_text_embedding_dim_f32(text, &mut temp, dim);

    if let Some(f) = out_f32 {
        let copy_len = dim.min(f.len());
        f[..copy_len].copy_from_slice(&temp[..copy_len]);
    }
    if let Some(q) = out_q7 {
        let copy_len = dim.min(q.len());
        quantize_f32_to_q7(&temp[..copy_len], &mut q[..copy_len]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_f32_sqrt_accuracy() {
        assert_eq!(f32_sqrt(0.0), 0.0);
        assert!((f32_sqrt(1.0) - 1.0).abs() < 1e-6);
        assert!((f32_sqrt(4.0) - 2.0).abs() < 1e-6);
        assert!((f32_sqrt(9.0) - 3.0).abs() < 1e-6);
        assert!((f32_sqrt(16.0) - 4.0).abs() < 1e-6);
        assert!((f32_sqrt(0.25) - 0.5).abs() < 1e-6);
    }

    #[test]
    fn test_compute_embedding_f32_dimension_and_l2_normalization() {
        let mut emb = [0.0f32; EMBEDDING_DIM];
        compute_text_embedding_f32(b"What is the system storage status?", &mut emb);

        // Check dimension
        assert_eq!(emb.len(), 768);

        // Check L2 norm is 1.0 (within single-precision tolerance)
        let mut sum_sq = 0.0f32;
        for &v in emb.iter() {
            sum_sq += v * v;
        }
        let norm = f32_sqrt(sum_sq);
        assert!(
            (norm - 1.0).abs() < 1e-4,
            "norm should be 1.0, got {}",
            norm
        );
    }

    #[test]
    fn test_compute_embedding_q7_bounds() {
        let mut q7 = [0i8; EMBEDDING_DIM];
        compute_text_embedding_q7(b"Open camera app with autofocus", &mut q7);

        assert_eq!(q7.len(), 768);
        let mut non_zero_count = 0;
        for &val in q7.iter() {
            assert!(val >= -127);
            if val != 0 {
                non_zero_count += 1;
            }
        }
        assert!(non_zero_count > 30, "embedding must have rich representation, got {}", non_zero_count);
    }

    #[test]
    fn test_cosine_similarity_identical_queries() {
        let mut v1 = [0i8; EMBEDDING_DIM];
        let mut v2 = [0i8; EMBEDDING_DIM];

        compute_text_embedding_q7(b"wireless network connection", &mut v1);
        compute_text_embedding_q7(b"wireless network connection", &mut v2);

        let sim = std::ai::cosine_similarity_q7(&v1, &v2);
        // Identical vectors should have similarity near maximum Q15 (32767)
        assert!(sim > 32000, "expected near 32767, got {}", sim);
    }

    #[test]
    fn test_cosine_similarity_similar_vs_dissimilar_queries() {
        let mut v_base = [0i8; EMBEDDING_DIM];
        let mut v_sim = [0i8; EMBEDDING_DIM];
        let mut v_diff = [0i8; EMBEDDING_DIM];

        compute_text_embedding_q7(b"network wifi connection settings", &mut v_base);
        compute_text_embedding_q7(b"wifi network settings configuration", &mut v_sim);
        compute_text_embedding_q7(b"fresh organic banana smoothie recipe", &mut v_diff);

        let sim_close = std::ai::cosine_similarity_q7(&v_base, &v_sim);
        let sim_far = std::ai::cosine_similarity_q7(&v_base, &v_diff);

        assert!(
            sim_close > sim_far,
            "similar query similarity ({}) must exceed dissimilar query similarity ({})",
            sim_close,
            sim_far
        );
        assert!(sim_close > 10000, "close similarity should be distinctly positive");
    }

    #[test]
    fn test_matryoshka_subdimension_truncation() {
        for &dim in &MATRYOSHKA_DIMS {
            let mut emb = [0.0f32; EMBEDDING_DIM];
            compute_text_embedding_dim_f32(b"battery power management", &mut emb, dim);

            let mut sum_sq = 0.0f32;
            for &v in emb[..dim].iter() {
                sum_sq += v * v;
            }
            let norm = f32_sqrt(sum_sq);
            assert!(
                (norm - 1.0).abs() < 1e-4,
                "dimension {} must be L2 normalized, got norm {}",
                dim,
                norm
            );

            // Everything beyond dim should remain 0
            for &v in emb[dim..].iter() {
                assert_eq!(v, 0.0);
            }
        }
    }
}
