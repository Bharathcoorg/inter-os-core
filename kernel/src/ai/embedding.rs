//! On-Device Vector Embedding Similarity & Semantic Context Engine
//!
//! Provides deterministic zero-allocation fixed-point integer cosine similarity
//! and nearest-neighbor vector search over 64-dimensional semantic embeddings.
//! Enables local agentic semantic routing, personal knowledge graph ranking,
//! and privacy-preserving context search without remote vector databases.

/// Dimensionality of on-device compact semantic embeddings.
pub const EMBEDDING_DIM: usize = 64;

/// Errors returned by the vector embedding engine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EmbeddingError {
    DimensionMismatch,
    EmptyVector,
}

/// Computes integer square root of a 64-bit unsigned integer using Newton-Raphson iterations.
pub const fn isqrt(val: u64) -> u64 {
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

/// Computes fixed-point cosine similarity between two vectors.
/// Result is scaled by 10,000 (i.e. 10,000 = 1.0000 perfect match, 0 = orthogonal, -10,000 = opposite).
pub fn cosine_similarity(a: &[i16], b: &[i16]) -> Result<i32, EmbeddingError> {
    if a.len() != b.len() {
        return Err(EmbeddingError::DimensionMismatch);
    }
    if a.is_empty() {
        return Err(EmbeddingError::EmptyVector);
    }

    let mut dot: i64 = 0;
    let mut norm_a_sq: u64 = 0;
    let mut norm_b_sq: u64 = 0;

    for i in 0..a.len() {
        let va = a[i] as i64;
        let vb = b[i] as i64;
        dot += va * vb;
        norm_a_sq += (va * va) as u64;
        norm_b_sq += (vb * vb) as u64;
    }

    if norm_a_sq == norm_b_sq && dot == (norm_a_sq as i64) {
        return Ok(10_000);
    }
    if norm_a_sq == norm_b_sq && dot == -(norm_a_sq as i64) {
        return Ok(-10_000);
    }

    let norm_a = isqrt(norm_a_sq);
    let norm_b = isqrt(norm_b_sq);
    let denom = norm_a.saturating_mul(norm_b);

    if denom == 0 {
        return Ok(0);
    }

    let scaled = ((dot * 10_000) / (denom as i64)).clamp(-10_000, 10_000);
    Ok(scaled as i32)
}

/// Pre-indexed semantic anchor for on-device natural language classification.
pub struct SemanticAnchor {
    pub label: &'static str,
    pub domain: &'static str,
    pub vector: [i16; EMBEDDING_DIM],
}

/// Finds the nearest semantic anchor for a query vector.
pub fn find_nearest<'a>(
    query: &[i16],
    anchors: &'a [SemanticAnchor],
) -> Option<(&'a SemanticAnchor, i32)> {
    let mut best_anchor: Option<&'a SemanticAnchor> = None;
    let mut best_score = i32::MIN;

    for anchor in anchors {
        if let Ok(score) = cosine_similarity(query, &anchor.vector) {
            if score > best_score {
                best_score = score;
                best_anchor = Some(anchor);
            }
        }
    }

    best_anchor.map(|a| (a, best_score))
}

/// Initializes the Vector Embedding Similarity Engine and executes automated verification.
pub fn init() {
    // 1. Identical Vector Test: Cosine similarity must equal 10,000 (1.0000)
    let mut vec_a = [0i16; EMBEDDING_DIM];
    let mut vec_b = [0i16; EMBEDDING_DIM];
    for i in 0..EMBEDDING_DIM {
        vec_a[i] = (i as i16) * 10;
        vec_b[i] = (i as i16) * 10;
    }
    let sim_identical = cosine_similarity(&vec_a, &vec_b).expect("Identical similarity failed");
    assert_eq!(sim_identical, 10_000, "Identical vectors must have 10,000 cosine similarity");

    // 2. Opposite Vector Test: Cosine similarity must equal -10,000 (-1.0000)
    let mut vec_c = [0i16; EMBEDDING_DIM];
    for i in 0..EMBEDDING_DIM {
        vec_c[i] = -((i as i16) * 10);
    }
    let sim_opposite = cosine_similarity(&vec_a, &vec_c).expect("Opposite similarity failed");
    assert_eq!(sim_opposite, -10_000, "Opposite vectors must have -10,000 cosine similarity");

    // 3. Semantic Anchor Nearest-Neighbor Retrieval Test
    let mut quick_commerce_vec = [0i16; EMBEDDING_DIM];
    let mut telephony_vec = [0i16; EMBEDDING_DIM];
    quick_commerce_vec[0] = 5000;
    quick_commerce_vec[1] = 4000;
    telephony_vec[2] = 5000;
    telephony_vec[3] = 4000;

    let anchors = [
        SemanticAnchor {
            label: "blinkit.order_groceries",
            domain: "quick_commerce",
            vector: quick_commerce_vec,
        },
        SemanticAnchor {
            label: "telephony.dial_contact",
            domain: "telephony",
            vector: telephony_vec,
        },
    ];

    let mut query_vec = [0i16; EMBEDDING_DIM];
    query_vec[0] = 4800; // Query vector closely aligns with quick commerce
    query_vec[1] = 3900;

    let (matched, score) = find_nearest(&query_vec, &anchors).expect("Anchor match failed");
    assert_eq!(matched.label, "blinkit.order_groceries");
    assert!(score > 9500, "Confidence score must exceed 95%");

    crate::serial_println!(
        "[TEST] Vector Cosine Engine: 64-dim fixed-point Q15 math, identical=10000, nearest match '{}' (Score: {}/10000).",
        matched.label,
        score
    );
}
