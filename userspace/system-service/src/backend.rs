// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

//! Format-neutral contracts owned by the userspace AI broker.
//!
//! `aid` uses these records to decide whether an installed backend can run an
//! installed model.  They deliberately contain no model-name switches.  A
//! backend advertises formats, operations, quantizations and usable hardware;
//! a model manifest declares the same properties plus verified provenance and
//! resource requirements.  Neural arithmetic remains in the backend.

use core::str;

pub const MAX_SESSIONS: usize = 8;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CapabilitySet(pub u32);

impl CapabilitySet {
    pub const TEXT_GENERATION: Self = Self(1 << 0);
    pub const TEXT_EMBEDDING: Self = Self(1 << 1);
    pub const STREAMING_GENERATION: Self = Self(1 << 2);
    pub const TOOL_CALL_OUTPUT: Self = Self(1 << 3);
    pub const AUDIO_INPUT: Self = Self(1 << 4);
    pub const AUDIO_OUTPUT: Self = Self(1 << 5);
    pub const VISION_INPUT: Self = Self(1 << 6);

    pub const fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }

    pub const fn contains(self, required: Self) -> bool {
        self.0 & required.0 == required.0
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ModelFormat {
    Gguf,
    LiteRtLm,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Quantization {
    BackendDefined,
    F32,
    F16,
    Q4_0,
    Q6K,
    Tq2_0,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HardwareBackend {
    Cpu,
    Gpu,
    Npu,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ResourceEstimate {
    pub model_weights: u64,
    pub kv_cache: u64,
    pub activations: u64,
    pub scratch: u64,
    pub tokenizer: u64,
    pub embedding_indexes: u64,
    pub audio_buffers: u64,
    pub runtime_overhead: u64,
}

impl ResourceEstimate {
    pub const ZERO: Self = Self {
        model_weights: 0,
        kv_cache: 0,
        activations: 0,
        scratch: 0,
        tokenizer: 0,
        embedding_indexes: 0,
        audio_buffers: 0,
        runtime_overhead: 0,
    };

    pub fn total(self) -> Option<u64> {
        self.model_weights
            .checked_add(self.kv_cache)?
            .checked_add(self.activations)?
            .checked_add(self.scratch)?
            .checked_add(self.tokenizer)?
            .checked_add(self.embedding_indexes)?
            .checked_add(self.audio_buffers)?
            .checked_add(self.runtime_overhead)
    }

    pub fn session_bytes(self) -> Option<u64> {
        self.kv_cache
            .checked_add(self.activations)?
            .checked_add(self.scratch)?
            .checked_add(self.audio_buffers)
    }

    pub fn resident_bytes(self) -> Option<u64> {
        self.model_weights
            .checked_add(self.tokenizer)?
            .checked_add(self.embedding_indexes)?
            .checked_add(self.runtime_overhead)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ModelManifest<'a> {
    pub id: &'a str,
    pub display_name: &'a str,
    pub source: &'a str,
    pub revision: &'a str,
    pub sha256: [u8; 32],
    pub artifact_bytes: u64,
    pub format: ModelFormat,
    pub architecture: &'a str,
    pub quantization: Quantization,
    pub tokenizer: &'a str,
    pub required_hardware: HardwareBackend,
    pub capabilities: CapabilitySet,
    pub resources: ResourceEstimate,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ManifestError {
    NotUtf8,
    MalformedLine,
    DuplicateField,
    MissingField,
    InvalidNumber,
    InvalidSha256,
    InvalidFormat,
    InvalidQuantization,
    InvalidHardware,
    InvalidCapability,
}

#[derive(Default)]
struct ManifestFields<'a> {
    id: Option<&'a str>,
    name: Option<&'a str>,
    source: Option<&'a str>,
    revision: Option<&'a str>,
    sha256: Option<&'a str>,
    artifact_bytes: Option<&'a str>,
    format: Option<&'a str>,
    architecture: Option<&'a str>,
    quantization: Option<&'a str>,
    tokenizer: Option<&'a str>,
    hardware: Option<&'a str>,
    capabilities: Option<&'a str>,
    model_weights: Option<&'a str>,
    kv_cache: Option<&'a str>,
    activations: Option<&'a str>,
    scratch: Option<&'a str>,
    tokenizer_bytes: Option<&'a str>,
    indexes: Option<&'a str>,
    audio: Option<&'a str>,
    overhead: Option<&'a str>,
}

fn set_once<'a>(slot: &mut Option<&'a str>, value: &'a str) -> Result<(), ManifestError> {
    if slot.replace(value).is_some() {
        Err(ManifestError::DuplicateField)
    } else {
        Ok(())
    }
}

fn parse_u64(value: Option<&str>) -> Result<u64, ManifestError> {
    value
        .ok_or(ManifestError::MissingField)?
        .parse::<u64>()
        .map_err(|_| ManifestError::InvalidNumber)
}

fn hex_nibble(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

fn parse_sha256(value: Option<&str>) -> Result<[u8; 32], ManifestError> {
    let raw = value.ok_or(ManifestError::MissingField)?.as_bytes();
    if raw.len() != 64 {
        return Err(ManifestError::InvalidSha256);
    }
    let mut out = [0u8; 32];
    for (index, byte) in out.iter_mut().enumerate() {
        let high = hex_nibble(raw[index * 2]).ok_or(ManifestError::InvalidSha256)?;
        let low = hex_nibble(raw[index * 2 + 1]).ok_or(ManifestError::InvalidSha256)?;
        *byte = high << 4 | low;
    }
    Ok(out)
}

fn parse_capabilities(value: Option<&str>) -> Result<CapabilitySet, ManifestError> {
    let mut result = CapabilitySet::default();
    for capability in value.ok_or(ManifestError::MissingField)?.split(',') {
        let parsed = match capability.trim() {
            "text-generation" => CapabilitySet::TEXT_GENERATION,
            "text-embedding" => CapabilitySet::TEXT_EMBEDDING,
            "streaming-generation" => CapabilitySet::STREAMING_GENERATION,
            "tool-call-output" => CapabilitySet::TOOL_CALL_OUTPUT,
            "audio-input" => CapabilitySet::AUDIO_INPUT,
            "audio-output" => CapabilitySet::AUDIO_OUTPUT,
            "vision-input" => CapabilitySet::VISION_INPUT,
            _ => return Err(ManifestError::InvalidCapability),
        };
        result = result.union(parsed);
    }
    Ok(result)
}

/// Parses a bounded, line-oriented model manifest.
///
/// The installer verifies the artifact bytes and supplies the observed digest
/// to [`verify_artifact`].  Parsing metadata is not treated as verification.
pub fn parse_model_manifest(input: &[u8]) -> Result<ModelManifest<'_>, ManifestError> {
    let text = str::from_utf8(input).map_err(|_| ManifestError::NotUtf8)?;
    let mut fields = ManifestFields::default();
    for raw_line in text.lines() {
        let line = raw_line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let (key, value) = line.split_once('=').ok_or(ManifestError::MalformedLine)?;
        let value = value.trim();
        if value.is_empty() {
            return Err(ManifestError::MalformedLine);
        }
        match key.trim() {
            "id" => set_once(&mut fields.id, value)?,
            "name" => set_once(&mut fields.name, value)?,
            "source" => set_once(&mut fields.source, value)?,
            "revision" => set_once(&mut fields.revision, value)?,
            "sha256" => set_once(&mut fields.sha256, value)?,
            "artifact_bytes" => set_once(&mut fields.artifact_bytes, value)?,
            "format" => set_once(&mut fields.format, value)?,
            "architecture" => set_once(&mut fields.architecture, value)?,
            "quantization" => set_once(&mut fields.quantization, value)?,
            "tokenizer" => set_once(&mut fields.tokenizer, value)?,
            "hardware" => set_once(&mut fields.hardware, value)?,
            "capabilities" => set_once(&mut fields.capabilities, value)?,
            "memory.model_weights" => set_once(&mut fields.model_weights, value)?,
            "memory.kv_cache" => set_once(&mut fields.kv_cache, value)?,
            "memory.activations" => set_once(&mut fields.activations, value)?,
            "memory.scratch" => set_once(&mut fields.scratch, value)?,
            "memory.tokenizer" => set_once(&mut fields.tokenizer_bytes, value)?,
            "memory.embedding_indexes" => set_once(&mut fields.indexes, value)?,
            "memory.audio_buffers" => set_once(&mut fields.audio, value)?,
            "memory.runtime_overhead" => set_once(&mut fields.overhead, value)?,
            _ => return Err(ManifestError::MalformedLine),
        }
    }

    let format = match fields.format.ok_or(ManifestError::MissingField)? {
        "gguf" => ModelFormat::Gguf,
        "litert-lm" => ModelFormat::LiteRtLm,
        _ => return Err(ManifestError::InvalidFormat),
    };
    let quantization = match fields.quantization.ok_or(ManifestError::MissingField)? {
        "backend-defined" => Quantization::BackendDefined,
        "f32" => Quantization::F32,
        "f16" => Quantization::F16,
        "q4_0" => Quantization::Q4_0,
        "q6_k" => Quantization::Q6K,
        "tq2_0" => Quantization::Tq2_0,
        _ => return Err(ManifestError::InvalidQuantization),
    };
    let required_hardware = match fields.hardware.ok_or(ManifestError::MissingField)? {
        "cpu" => HardwareBackend::Cpu,
        "gpu" => HardwareBackend::Gpu,
        "npu" => HardwareBackend::Npu,
        _ => return Err(ManifestError::InvalidHardware),
    };

    Ok(ModelManifest {
        id: fields.id.ok_or(ManifestError::MissingField)?,
        display_name: fields.name.ok_or(ManifestError::MissingField)?,
        source: fields.source.ok_or(ManifestError::MissingField)?,
        revision: fields.revision.ok_or(ManifestError::MissingField)?,
        sha256: parse_sha256(fields.sha256)?,
        artifact_bytes: parse_u64(fields.artifact_bytes)?,
        format,
        architecture: fields.architecture.ok_or(ManifestError::MissingField)?,
        quantization,
        tokenizer: fields.tokenizer.ok_or(ManifestError::MissingField)?,
        required_hardware,
        capabilities: parse_capabilities(fields.capabilities)?,
        resources: ResourceEstimate {
            model_weights: parse_u64(fields.model_weights)?,
            kv_cache: parse_u64(fields.kv_cache)?,
            activations: parse_u64(fields.activations)?,
            scratch: parse_u64(fields.scratch)?,
            tokenizer: parse_u64(fields.tokenizer_bytes)?,
            embedding_indexes: parse_u64(fields.indexes)?,
            audio_buffers: parse_u64(fields.audio)?,
            runtime_overhead: parse_u64(fields.overhead)?,
        },
    })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VerificationError {
    SizeMismatch,
    DigestMismatch,
}

pub fn verify_artifact(
    manifest: &ModelManifest<'_>,
    observed_bytes: u64,
    observed_sha256: &[u8; 32],
) -> Result<(), VerificationError> {
    if manifest.artifact_bytes != observed_bytes {
        return Err(VerificationError::SizeMismatch);
    }
    if !constant_time_eq(&manifest.sha256, observed_sha256) {
        return Err(VerificationError::DigestMismatch);
    }
    Ok(())
}

fn constant_time_eq(left: &[u8; 32], right: &[u8; 32]) -> bool {
    let mut difference = 0u8;
    for index in 0..32 {
        difference |= left[index] ^ right[index];
    }
    difference == 0
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BackendInfo {
    pub id: &'static str,
    pub version: u32,
    pub available: bool,
    pub format: ModelFormat,
    pub capabilities: CapabilitySet,
    pub hardware: HardwareBackend,
    pub architectures: &'static [&'static str],
    pub quantizations: &'static [Quantization],
    pub tokenizers: &'static [&'static str],
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProbeFailure {
    BackendUnavailable,
    Format,
    Architecture,
    Operators,
    Quantization,
    Tokenizer,
    Hardware,
    Capability,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ModelProbe {
    pub backend_id: &'static str,
    pub result: Result<(), ProbeFailure>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BackendError {
    Unavailable,
    Unsupported,
    InvalidModel,
    InsufficientResources,
    SessionNotFound,
    Cancelled,
    Crashed,
}

pub enum StreamControl {
    Continue,
    Cancel,
}

/// The runtime-facing backend interface.  Implementations live in userspace.
pub trait InferenceBackend {
    fn backend_info(&self) -> BackendInfo;
    fn probe_model(&self, model: &ModelManifest<'_>) -> ModelProbe;
    fn estimate_resources(
        &self,
        model: &ModelManifest<'_>,
    ) -> Result<ResourceEstimate, BackendError>;
    fn load_model(&mut self, model: &ModelManifest<'_>) -> Result<u64, BackendError>;
    fn create_session(&mut self, model_handle: u64, owner: u64) -> Result<u64, BackendError>;
    fn generate(
        &mut self,
        session: u64,
        input: &[u8],
        on_chunk: &mut dyn FnMut(&[u8]) -> StreamControl,
    ) -> Result<(), BackendError>;
    fn embed(
        &mut self,
        session: u64,
        input: &[u8],
        output: &mut [f32],
    ) -> Result<usize, BackendError>;
    fn cancel(&mut self, session: u64) -> Result<(), BackendError>;
    fn destroy_session(&mut self, session: u64) -> Result<(), BackendError>;
    fn unload_model(&mut self, model_handle: u64) -> Result<(), BackendError>;
}

pub const NATIVE_GGUF_INFO: BackendInfo = BackendInfo {
    id: "native-gguf-rust",
    version: 1,
    available: true,
    format: ModelFormat::Gguf,
    capabilities: CapabilitySet::TEXT_GENERATION
        .union(CapabilitySet::TEXT_EMBEDDING)
        .union(CapabilitySet::STREAMING_GENERATION),
    hardware: HardwareBackend::Cpu,
    architectures: &["gemma2", "gemma-embedding"],
    quantizations: &[
        Quantization::F32,
        Quantization::F16,
        Quantization::Q4_0,
        Quantization::Q6K,
        Quantization::Tq2_0,
    ],
    tokenizers: &["embedded"],
};

/// The LiteRT-LM descriptor is intentionally unavailable.  A `.litertlm` file
/// and host CLI do not constitute an InterOS backend.
pub const LITERT_LM_INFO: BackendInfo = BackendInfo {
    id: "litert-lm-cpu",
    version: 0,
    available: false,
    format: ModelFormat::LiteRtLm,
    capabilities: CapabilitySet::TEXT_GENERATION
        .union(CapabilitySet::TEXT_EMBEDDING)
        .union(CapabilitySet::STREAMING_GENERATION)
        .union(CapabilitySet::VISION_INPUT),
    hardware: HardwareBackend::Cpu,
    architectures: &["gemma4-e2b", "embeddinggemma2"],
    quantizations: &[Quantization::BackendDefined],
    tokenizers: &["embedded"],
};

pub fn probe_backend(info: BackendInfo, model: &ModelManifest<'_>) -> ModelProbe {
    let result = if !info.available {
        Err(ProbeFailure::BackendUnavailable)
    } else if info.format != model.format {
        Err(ProbeFailure::Format)
    } else if !info.architectures.contains(&model.architecture) {
        Err(ProbeFailure::Architecture)
    } else if !info.quantizations.contains(&model.quantization) {
        Err(ProbeFailure::Quantization)
    } else if !info.tokenizers.contains(&model.tokenizer) {
        Err(ProbeFailure::Tokenizer)
    } else if info.hardware != model.required_hardware {
        Err(ProbeFailure::Hardware)
    } else if !info.capabilities.contains(model.capabilities) {
        Err(ProbeFailure::Capability)
    } else {
        Ok(())
    };
    ModelProbe {
        backend_id: info.id,
        result,
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct SessionRecord {
    id: u64,
    owner: u64,
    backend_id: u32,
    reserved_bytes: u64,
    active: bool,
    cancelled: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AdmissionError {
    Arithmetic,
    InsufficientRam,
    InsufficientStorage,
    SessionCapacity,
    SessionNotFound,
    NotOwner,
}

/// Bounded lifecycle and reservation state owned by `aid`.
pub struct BrokerLedger {
    ram_available: u64,
    storage_available: u64,
    foreground_reserve: u64,
    resident_bytes: u64,
    session_bytes: u64,
    sessions: [SessionRecord; MAX_SESSIONS],
    next_session: u64,
}

impl BrokerLedger {
    pub const fn new(ram_available: u64, storage_available: u64, foreground_reserve: u64) -> Self {
        Self {
            ram_available,
            storage_available,
            foreground_reserve,
            resident_bytes: 0,
            session_bytes: 0,
            sessions: [SessionRecord {
                id: 0,
                owner: 0,
                backend_id: 0,
                reserved_bytes: 0,
                active: false,
                cancelled: false,
            }; MAX_SESSIONS],
            next_session: 1,
        }
    }

    pub fn admit_install(&self, artifact_bytes: u64) -> Result<(), AdmissionError> {
        if artifact_bytes > self.storage_available {
            Err(AdmissionError::InsufficientStorage)
        } else {
            Ok(())
        }
    }

    /// Checks one operation's complete peak working set without recording a
    /// fictional residency.  The backend's allocations remain the physical
    /// enforcement and are released by Rust ownership when the call returns.
    pub fn admit_operation(&self, estimate: ResourceEstimate) -> Result<(), AdmissionError> {
        let bytes = estimate.total().ok_or(AdmissionError::Arithmetic)?;
        self.reserve_ram(bytes)
    }

    pub fn reserve_resident(&mut self, estimate: ResourceEstimate) -> Result<(), AdmissionError> {
        let bytes = estimate
            .resident_bytes()
            .ok_or(AdmissionError::Arithmetic)?;
        self.reserve_ram(bytes)?;
        self.resident_bytes += bytes;
        Ok(())
    }

    pub fn release_resident(&mut self, estimate: ResourceEstimate) {
        if let Some(bytes) = estimate.resident_bytes() {
            self.resident_bytes = self.resident_bytes.saturating_sub(bytes);
        }
    }

    pub fn create_session(
        &mut self,
        owner: u64,
        backend_id: u32,
        estimate: ResourceEstimate,
    ) -> Result<u64, AdmissionError> {
        let bytes = estimate.session_bytes().ok_or(AdmissionError::Arithmetic)?;
        self.reserve_ram(bytes)?;
        let slot = self
            .sessions
            .iter_mut()
            .find(|session| !session.active)
            .ok_or(AdmissionError::SessionCapacity)?;
        let id = self.next_session;
        self.next_session = self.next_session.wrapping_add(1).max(1);
        *slot = SessionRecord {
            id,
            owner,
            backend_id,
            reserved_bytes: bytes,
            active: true,
            cancelled: false,
        };
        self.session_bytes += bytes;
        Ok(id)
    }

    pub fn cancel(&mut self, owner: u64, id: u64) -> Result<(), AdmissionError> {
        let session = self
            .sessions
            .iter_mut()
            .find(|session| session.active && session.id == id)
            .ok_or(AdmissionError::SessionNotFound)?;
        if session.owner != owner {
            return Err(AdmissionError::NotOwner);
        }
        session.cancelled = true;
        Ok(())
    }

    pub fn is_cancelled(&self, id: u64) -> bool {
        self.sessions
            .iter()
            .any(|session| session.active && session.id == id && session.cancelled)
    }

    pub fn destroy_session(&mut self, owner: u64, id: u64) -> Result<(), AdmissionError> {
        let session = self
            .sessions
            .iter_mut()
            .find(|session| session.active && session.id == id)
            .ok_or(AdmissionError::SessionNotFound)?;
        if session.owner != owner {
            return Err(AdmissionError::NotOwner);
        }
        self.session_bytes = self.session_bytes.saturating_sub(session.reserved_bytes);
        *session = SessionRecord::default();
        Ok(())
    }

    pub fn reap_owner(&mut self, owner: u64) -> usize {
        self.reap_matching(|session| session.owner == owner)
    }

    pub fn reap_backend(&mut self, backend_id: u32) -> usize {
        self.reap_matching(|session| session.backend_id == backend_id)
    }

    pub const fn reserved_bytes(&self) -> u64 {
        self.resident_bytes + self.session_bytes
    }

    pub fn active_sessions(&self) -> usize {
        self.sessions
            .iter()
            .filter(|session| session.active)
            .count()
    }

    fn reserve_ram(&self, bytes: u64) -> Result<(), AdmissionError> {
        let committed = self
            .reserved_bytes()
            .checked_add(bytes)
            .and_then(|value| value.checked_add(self.foreground_reserve))
            .ok_or(AdmissionError::Arithmetic)?;
        if committed > self.ram_available {
            Err(AdmissionError::InsufficientRam)
        } else {
            Ok(())
        }
    }

    fn reap_matching(&mut self, mut matches: impl FnMut(&SessionRecord) -> bool) -> usize {
        let mut reaped = 0;
        for session in &mut self.sessions {
            if session.active && matches(session) {
                self.session_bytes = self.session_bytes.saturating_sub(session.reserved_bytes);
                *session = SessionRecord::default();
                reaped += 1;
            }
        }
        reaped
    }
}

/// RAM retained for ordinary foreground phone operation while AI is active.
///
/// A quarter of detected RAM scales with the QEMU/device configuration.  The
/// 128 MiB floor protects small guests from an inference run that would leave
/// the compositor, launcher and storage service no useful working set.
pub fn foreground_ram_reserve(total_ram: u64) -> u64 {
    (total_ram / 4).max(128 * 1024 * 1024).min(total_ram)
}

/// Prices the native Rust backend from the model's parsed geometry.
pub fn native_generation_resources(
    config: &crate::inference::ModelConfig,
) -> Result<ResourceEstimate, crate::inference::InferenceError> {
    let kv_cache = u64::try_from(config.kv_cache_bytes())
        .map_err(|_| crate::inference::InferenceError::CorruptedWeights)?;
    let activations = u64::try_from(crate::inference::Scratch::bytes_for(config))
        .map_err(|_| crate::inference::InferenceError::CorruptedWeights)?;
    let model_weights = u64::try_from(config.resident_weight_bytes()?)
        .map_err(|_| crate::inference::InferenceError::CorruptedWeights)?;
    Ok(ResourceEstimate {
        model_weights,
        kv_cache,
        activations,
        // Temporary matrix storage is already included in `Scratch::bytes_for`.
        // Reporting it twice would refuse a run for memory it never allocates.
        scratch: 0,
        tokenizer: crate::tokenizer::TOKENIZER_HEAP_BUDGET as u64,
        embedding_indexes: 0,
        audio_buffers: 0,
        // Request ids, stream buffers and descriptor tables are bounded and
        // charged explicitly here rather than hidden in the weight estimate.
        runtime_overhead: 2 * 1024 * 1024,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const MANIFEST: &[u8] = b"\
id=org.interos.test-embed\n\
name=Test embedding model\n\
source=example/model\n\
revision=0123456789abcdef\n\
sha256=000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f\n\
artifact_bytes=165000000\n\
format=litert-lm\n\
architecture=embeddinggemma\n\
quantization=f16\n\
tokenizer=sentencepiece\n\
hardware=cpu\n\
capabilities=text-embedding\n\
memory.model_weights=165000000\n\
memory.kv_cache=0\n\
memory.activations=8000000\n\
memory.scratch=16000000\n\
memory.tokenizer=4000000\n\
memory.embedding_indexes=32000000\n\
memory.audio_buffers=0\n\
memory.runtime_overhead=12000000\n";

    #[test]
    fn manifest_parsing_preserves_provenance_and_resource_classes() {
        let manifest = parse_model_manifest(MANIFEST).expect("manifest");
        assert_eq!(manifest.id, "org.interos.test-embed");
        assert_eq!(manifest.format, ModelFormat::LiteRtLm);
        assert_eq!(manifest.sha256[31], 0x1f);
        assert_eq!(manifest.resources.embedding_indexes, 32_000_000);
        assert!(manifest
            .capabilities
            .contains(CapabilitySet::TEXT_EMBEDDING));
        assert_eq!(
            verify_artifact(&manifest, 165_000_000, &manifest.sha256),
            Ok(())
        );
    }

    #[test]
    fn failed_verification_never_becomes_an_installed_model() {
        let manifest = parse_model_manifest(MANIFEST).expect("manifest");
        let mut wrong = manifest.sha256;
        wrong[7] ^= 1;
        assert_eq!(
            verify_artifact(&manifest, manifest.artifact_bytes, &wrong),
            Err(VerificationError::DigestMismatch)
        );
        assert_eq!(
            verify_artifact(&manifest, manifest.artifact_bytes - 1, &manifest.sha256),
            Err(VerificationError::SizeMismatch)
        );
    }

    #[test]
    fn unsupported_formats_and_uninstalled_backends_are_distinct() {
        let manifest = parse_model_manifest(MANIFEST).expect("manifest");
        assert_eq!(
            probe_backend(NATIVE_GGUF_INFO, &manifest).result,
            Err(ProbeFailure::Format)
        );
        assert_eq!(
            probe_backend(LITERT_LM_INFO, &manifest).result,
            Err(ProbeFailure::BackendUnavailable)
        );
    }

    #[test]
    fn native_backend_rejects_unknown_architecture_quantization_and_tokenizer() {
        let parsed = parse_model_manifest(MANIFEST).expect("manifest");
        let native = ModelManifest {
            format: ModelFormat::Gguf,
            architecture: "gemma2",
            quantization: Quantization::Q6K,
            tokenizer: "embedded",
            capabilities: CapabilitySet::TEXT_GENERATION,
            ..parsed
        };
        assert_eq!(probe_backend(NATIVE_GGUF_INFO, &native).result, Ok(()));
        assert_eq!(
            probe_backend(
                NATIVE_GGUF_INFO,
                &ModelManifest {
                    architecture: "unknown",
                    ..native
                }
            )
            .result,
            Err(ProbeFailure::Architecture)
        );
        assert_eq!(
            probe_backend(
                NATIVE_GGUF_INFO,
                &ModelManifest {
                    quantization: Quantization::BackendDefined,
                    ..native
                }
            )
            .result,
            Err(ProbeFailure::Quantization)
        );
        assert_eq!(
            probe_backend(
                NATIVE_GGUF_INFO,
                &ModelManifest {
                    tokenizer: "external",
                    ..native
                }
            )
            .result,
            Err(ProbeFailure::Tokenizer)
        );
    }

    #[test]
    fn admission_preserves_foreground_ram_and_storage_headroom() {
        let estimate = ResourceEstimate {
            model_weights: 300,
            kv_cache: 200,
            activations: 100,
            scratch: 100,
            tokenizer: 50,
            embedding_indexes: 0,
            audio_buffers: 0,
            runtime_overhead: 50,
        };
        let mut broker = BrokerLedger::new(999, 500, 200);
        assert_eq!(
            broker.admit_install(501),
            Err(AdmissionError::InsufficientStorage)
        );
        assert_eq!(broker.reserve_resident(estimate), Ok(()));
        assert_eq!(
            broker.create_session(7, 1, estimate),
            Err(AdmissionError::InsufficientRam)
        );
        broker.release_resident(estimate);
        assert_eq!(broker.create_session(7, 1, estimate), Ok(1));
    }

    #[test]
    fn cancellation_revocation_and_normal_shutdown_reclaim_sessions() {
        let estimate = ResourceEstimate {
            kv_cache: 64,
            scratch: 32,
            ..ResourceEstimate::ZERO
        };
        let mut broker = BrokerLedger::new(1_024, 1_024, 128);
        let first = broker.create_session(41, 3, estimate).expect("first");
        let second = broker.create_session(42, 3, estimate).expect("second");
        assert_eq!(broker.cancel(42, first), Err(AdmissionError::NotOwner));
        assert_eq!(broker.cancel(41, first), Ok(()));
        assert!(broker.is_cancelled(first));
        assert_eq!(broker.destroy_session(41, first), Ok(()));
        assert_eq!(broker.active_sessions(), 1);
        assert_eq!(broker.reap_owner(42), 1);
        assert_eq!(broker.active_sessions(), 0);
        assert_eq!(broker.reserved_bytes(), 0);
        assert_eq!(
            broker.destroy_session(42, second),
            Err(AdmissionError::SessionNotFound)
        );
    }

    #[test]
    fn backend_crash_reclaims_only_that_backends_sessions() {
        let estimate = ResourceEstimate {
            activations: 50,
            ..ResourceEstimate::ZERO
        };
        let mut broker = BrokerLedger::new(1_024, 1_024, 0);
        broker.create_session(1, 10, estimate).expect("backend 10");
        broker.create_session(2, 11, estimate).expect("backend 11");
        assert_eq!(broker.reap_backend(10), 1);
        assert_eq!(broker.active_sessions(), 1);
        assert_eq!(broker.reserved_bytes(), 50);
    }

    #[test]
    fn foreground_reserve_scales_without_consuming_more_than_total_ram() {
        assert_eq!(foreground_ram_reserve(64 * 1024 * 1024), 64 * 1024 * 1024);
        assert_eq!(foreground_ram_reserve(512 * 1024 * 1024), 128 * 1024 * 1024);
        assert_eq!(
            foreground_ram_reserve(4 * 1024 * 1024 * 1024),
            1024 * 1024 * 1024
        );
    }
}
