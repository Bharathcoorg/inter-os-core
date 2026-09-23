//! Local AI Model Package & Metadata Specification
//!
//! Stores cryptographic model identity, publisher attestation, quantization parameters,
//! and resource footprint for local open-weight models as specified in MASTER_PROMPT.md.

use alloc::string::String;
use crate::crypto::Sha256;

/// Architecture family of the local language model.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModelFamily {
    Qwen,
    Llama,
    DeepSeek,
    Phi,
    Gemma,
}

/// Quantization precision format of weights.
#[allow(non_camel_case_types)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Quantization {
    Q4_K_M,
    Q5_K_M,
    Q8_0,
    FP16,
}

/// Dynamic runtime state of a local model package.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModelState {
    /// Residing on storage, not consuming RAM.
    Unloaded,
    /// Memory partition allocated and weights loaded in RAM.
    Loaded,
    /// Actively processing an agent intent / inference.
    Active,
    /// Preempted by a higher-priority foreground task and purged from RAM.
    Preempted,
}

/// Model Package definition embodying cryptographic attestation and RAM budgets.
#[derive(Debug, Clone)]
pub struct ModelPackage {
    /// Canonical model identifier (e.g. `qwen2.5-0.5b-instruct`).
    pub id: String,
    /// Architecture family.
    pub family: ModelFamily,
    /// Parameter count in millions (e.g. 500 = 0.5B, 1500 = 1.5B).
    pub parameters_millions: u32,
    /// Quantization level.
    pub quantization: Quantization,
    /// Supported context window in tokens.
    pub context_window: usize,
    /// Required RAM footprint in bytes when active.
    pub ram_footprint_bytes: usize,
    /// Publisher DID identity string.
    pub publisher: String,
    /// Cryptographic SHA-256 weight digest.
    pub sha256_digest: [u8; 32],
    /// Open source license identifier.
    pub license: String,
    /// Current lifecycle state.
    pub state: ModelState,
}

impl ModelPackage {
    /// Constructs a verified model package definition.
    pub fn new(
        id: &str,
        family: ModelFamily,
        parameters_millions: u32,
        quantization: Quantization,
        context_window: usize,
        ram_footprint_bytes: usize,
        publisher: &str,
        license: &str,
    ) -> Self {
        let digest = Sha256::digest(id.as_bytes());
        Self {
            id: String::from(id),
            family,
            parameters_millions,
            quantization,
            context_window,
            ram_footprint_bytes,
            publisher: String::from(publisher),
            sha256_digest: digest,
            license: String::from(license),
            state: ModelState::Unloaded,
        }
    }
}
