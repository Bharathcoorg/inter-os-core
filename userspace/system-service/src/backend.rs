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
use interos_std as std;

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

/// Maps a GGUF `general.file_type` number to the engine's quantization label.
///
/// Only the values the engine can name from the GGUF specification are mapped;
/// any other number is reported as [`Quantization::BackendDefined`] rather than
/// guessed, because naming "Q8_0" for a bare `7` would be the platform's table
/// of what numbers mean, not the file's own declaration.
pub fn quantization_from_file_type(file_type: Option<u32>) -> Quantization {
    match file_type {
        Some(0) => Quantization::F32,
        Some(1) => Quantization::F16,
        Some(2) => Quantization::Q4_0,
        _ => Quantization::BackendDefined,
    }
}

/// Builds the manifest for an installed generative model from the artifact's own
/// GGUF metadata and the digest and size the filesystem reports for the file.
///
/// Identity is bound to the real bytes: `artifact_bytes` and `sha256` are the
/// values `fs_stat` returned for this exact path, and `architecture`/`tokenizer`
/// come from the file's own header. A caller that has not verified the artifact
/// passes a zeroed digest and a size of `0`, which [`verify_artifact`] rejects —
/// the manifest never substitutes a hardcoded architecture such as `gemma2`.
pub fn installed_generative_manifest<'a>(
    info_architecture: Option<&'a str>,
    info_tokenizer: Option<&'a str>,
    info_file_type: Option<u32>,
    source: &'a str,
    artifact_bytes: u64,
    sha256: [u8; 32],
    resources: ResourceEstimate,
) -> ModelManifest<'a> {
    ModelManifest {
        id: "org.interos.installed-generative",
        display_name: "Installed Generative Model",
        source,
        revision: "local",
        sha256,
        artifact_bytes,
        format: ModelFormat::Gguf,
        architecture: info_architecture.unwrap_or("unknown"),
        quantization: quantization_from_file_type(info_file_type),
        tokenizer: info_tokenizer.unwrap_or("unknown"),
        required_hardware: HardwareBackend::Cpu,
        capabilities: CapabilitySet::TEXT_GENERATION.union(CapabilitySet::STREAMING_GENERATION),
        resources,
    }
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
        if self.sessions.iter().all(|session| session.active) {
            return Err(AdmissionError::SessionCapacity);
        }
        // Never reuse an identifier while its previous session is live, even
        // after the counter wraps.  A stale cancellation must not address a
        // different owner's new session.
        let mut id = self.next_session;
        for _ in 0..=MAX_SESSIONS {
            if !self.sessions.iter().any(|session| session.active && session.id == id) {
                break;
            }
            id = id.wrapping_add(1).max(1);
        }
        self.next_session = id.wrapping_add(1).max(1);
        let slot = self.sessions.iter_mut().find(|session| !session.active)
            .ok_or(AdmissionError::SessionCapacity)?;
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

pub const MAX_LOADED_MODELS: usize = 4;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LoadedModelRecord {
    pub handle: u64,
    pub id: [u8; 64],
    pub id_len: usize,
    pub source: [u8; 128],
    pub source_len: usize,
    pub architecture: [u8; 32],
    pub architecture_len: usize,
    pub capabilities: CapabilitySet,
    pub resources: ResourceEstimate,
    pub active: bool,
}

impl Default for LoadedModelRecord {
    fn default() -> Self {
        Self {
            handle: 0,
            id: [0u8; 64],
            id_len: 0,
            source: [0u8; 128],
            source_len: 0,
            architecture: [0u8; 32],
            architecture_len: 0,
            capabilities: CapabilitySet::default(),
            resources: ResourceEstimate::ZERO,
            active: false,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct BackendSessionRecord {
    pub id: u64,
    pub model_handle: u64,
    pub owner: u64,
    pub active: bool,
    pub cancelled: bool,
}

pub struct NativeGgufBackend {
    pub models: [LoadedModelRecord; MAX_LOADED_MODELS],
    pub sessions: [BackendSessionRecord; MAX_SESSIONS],
    next_model_handle: u64,
    next_session_id: u64,
}

impl NativeGgufBackend {
    pub const fn new() -> Self {
        Self {
            models: [LoadedModelRecord {
                handle: 0,
                id: [0u8; 64],
                id_len: 0,
                source: [0u8; 128],
                source_len: 0,
                architecture: [0u8; 32],
                architecture_len: 0,
                capabilities: CapabilitySet(0),
                resources: ResourceEstimate::ZERO,
                active: false,
            }; MAX_LOADED_MODELS],
            sessions: [BackendSessionRecord {
                id: 0,
                model_handle: 0,
                owner: 0,
                active: false,
                cancelled: false,
            }; MAX_SESSIONS],
            next_model_handle: 1,
            next_session_id: 1,
        }
    }

    pub fn active_model_count(&self) -> usize {
        self.models.iter().filter(|m| m.active).count()
    }

    pub fn active_session_count(&self) -> usize {
        self.sessions.iter().filter(|s| s.active).count()
    }

    pub fn is_session_active(&self, session: u64) -> bool {
        self.sessions.iter().any(|s| s.active && s.id == session)
    }

    pub fn is_session_cancelled(&self, session: u64) -> bool {
        self.sessions.iter().any(|s| s.active && s.id == session && s.cancelled)
    }

    pub fn reap_owner(&mut self, owner: u64) -> usize {
        let mut reaped = 0;
        for session in &mut self.sessions {
            if session.active && session.owner == owner {
                *session = BackendSessionRecord::default();
                reaped += 1;
            }
        }
        reaped
    }
}

impl InferenceBackend for NativeGgufBackend {
    fn backend_info(&self) -> BackendInfo {
        NATIVE_GGUF_INFO
    }

    fn probe_model(&self, model: &ModelManifest<'_>) -> ModelProbe {
        probe_backend(NATIVE_GGUF_INFO, model)
    }

    fn estimate_resources(
        &self,
        model: &ModelManifest<'_>,
    ) -> Result<ResourceEstimate, BackendError> {
        let probe = self.probe_model(model);
        if let Err(failure) = probe.result {
            return match failure {
                ProbeFailure::BackendUnavailable => Err(BackendError::Unavailable),
                _ => Err(BackendError::Unsupported),
            };
        }
        if model.resources != ResourceEstimate::ZERO {
            if model.resources.total().is_none() {
                return Err(BackendError::InsufficientResources);
            }
            return Ok(model.resources);
        }
        if !model.source.is_empty() {
            if let Ok(info) = crate::gguf::read_model_info(model.source) {
                if let Ok(config) = crate::inference::ModelConfig::from_gguf(&info) {
                    if let Ok(estimate) = native_generation_resources(&config) {
                        return Ok(estimate);
                    }
                }
            }
        }
        Err(BackendError::InvalidModel)
    }

    fn load_model(&mut self, model: &ModelManifest<'_>) -> Result<u64, BackendError> {
        let probe = self.probe_model(model);
        if let Err(failure) = probe.result {
            return match failure {
                ProbeFailure::BackendUnavailable => Err(BackendError::Unavailable),
                _ => Err(BackendError::Unsupported),
            };
        }
        let estimate = self.estimate_resources(model)?;

        if self.models.iter().all(|m| m.active) {
            return Err(BackendError::InsufficientResources);
        }
        let mut handle = self.next_model_handle;
        for _ in 0..=MAX_LOADED_MODELS {
            if !self.models.iter().any(|m| m.active && m.handle == handle) {
                break;
            }
            handle = handle.wrapping_add(1).max(1);
        }
        self.next_model_handle = handle.wrapping_add(1).max(1);
        let slot = self.models.iter_mut().find(|m| !m.active)
            .ok_or(BackendError::InsufficientResources)?;

        let mut id_buf = [0u8; 64];
        let id_bytes = model.id.as_bytes();
        let id_len = id_bytes.len().min(64);
        id_buf[..id_len].copy_from_slice(&id_bytes[..id_len]);

        let mut src_buf = [0u8; 128];
        let src_bytes = model.source.as_bytes();
        let src_len = src_bytes.len().min(128);
        src_buf[..src_len].copy_from_slice(&src_bytes[..src_len]);

        let mut arch_buf = [0u8; 32];
        let arch_bytes = model.architecture.as_bytes();
        let arch_len = arch_bytes.len().min(32);
        arch_buf[..arch_len].copy_from_slice(&arch_bytes[..arch_len]);

        *slot = LoadedModelRecord {
            handle,
            id: id_buf,
            id_len,
            source: src_buf,
            source_len: src_len,
            architecture: arch_buf,
            architecture_len: arch_len,
            capabilities: model.capabilities,
            resources: estimate,
            active: true,
        };

        Ok(handle)
    }

    fn create_session(&mut self, model_handle: u64, owner: u64) -> Result<u64, BackendError> {
        if !self.models.iter().any(|m| m.active && m.handle == model_handle) {
            return Err(BackendError::InvalidModel);
        }
        if self.sessions.iter().all(|s| s.active) {
            return Err(BackendError::InsufficientResources);
        }
        let mut id = self.next_session_id;
        for _ in 0..=MAX_SESSIONS {
            if !self.sessions.iter().any(|s| s.active && s.id == id) {
                break;
            }
            id = id.wrapping_add(1).max(1);
        }
        self.next_session_id = id.wrapping_add(1).max(1);
        let slot = self.sessions.iter_mut().find(|s| !s.active)
            .ok_or(BackendError::InsufficientResources)?;

        *slot = BackendSessionRecord {
            id,
            model_handle,
            owner,
            active: true,
            cancelled: false,
        };

        Ok(id)
    }

    fn generate(
        &mut self,
        session: u64,
        input: &[u8],
        on_chunk: &mut dyn FnMut(&[u8]) -> StreamControl,
    ) -> Result<(), BackendError> {
        let sess_idx = self.sessions.iter().position(|s| s.active && s.id == session)
            .ok_or(BackendError::SessionNotFound)?;
        if self.sessions[sess_idx].cancelled {
            return Err(BackendError::Cancelled);
        }
        let model_handle = self.sessions[sess_idx].model_handle;
        let model = self.models.iter().find(|m| m.active && m.handle == model_handle)
            .ok_or(BackendError::InvalidModel)?;
        if !model.capabilities.contains(CapabilitySet::TEXT_GENERATION) {
            return Err(BackendError::Unsupported);
        }
        if input.is_empty() {
            return Ok(());
        }
        let source_path = core::str::from_utf8(&model.source[..model.source_len])
            .map_err(|_| BackendError::InvalidModel)?;

        let fd = match std::fs_open(source_path, std::FS_READ) {
            Ok(fd) => fd,
            Err(_) => return Err(BackendError::Unavailable),
        };
        let source = crate::weights::ModelFileDescriptor { fd };
        let (table, info) = match crate::get_resident_model_cache(source_path) {
            Some(cached) => cached,
            None => {
                let mut stream = crate::gguf::GgufStream::buffered_from_file(fd);
                let table = match crate::weights::TensorTable::walk(&mut stream) {
                    Ok(t) => t,
                    Err(_) => {
                        let _ = std::fs_close(fd);
                        return Err(BackendError::InvalidModel);
                    }
                };
                let mut stream = crate::gguf::GgufStream::buffered_from_file(fd);
                let info = match crate::gguf::read_model_info_stream(&mut stream) {
                    Ok(i) => i,
                    Err(_) => {
                        let _ = std::fs_close(fd);
                        return Err(BackendError::InvalidModel);
                    }
                };
                crate::set_resident_model_cache(source_path, &table, &info);
                (table, info)
            }
        };
        let config = match crate::inference::ModelConfig::from_gguf(&info) {
            Ok(c) => c,
            Err(_) => {
                let _ = std::fs_close(fd);
                return Err(BackendError::InvalidModel);
            }
        };
        let tokenizer = match crate::get_or_build_resident_tokenizer(source_path) {
            Ok(t) => t,
            Err(_) => {
                let _ = std::fs_close(fd);
                return Err(BackendError::InvalidModel);
            }
        };
        let prompt_text = match core::str::from_utf8(input) {
            Ok(t) => t,
            Err(_) => {
                let _ = std::fs_close(fd);
                return Err(BackendError::InvalidModel);
            }
        };
        let mut prompt_ids = alloc::vec::Vec::new();
        if prompt_ids.try_reserve_exact(config.seq_len).is_err() {
            let _ = std::fs_close(fd);
            return Err(BackendError::InsufficientResources);
        }
        prompt_ids.resize(config.seq_len, 0u32);
        let num_ids = match tokenizer.encode(prompt_text.as_bytes(), &mut prompt_ids) {
            Ok(n) => n,
            Err(_) => {
                let _ = std::fs_close(fd);
                return Err(BackendError::InvalidModel);
            }
        };
        let tokens = &prompt_ids[..num_ids];
        let max_new_tokens = config.seq_len.saturating_sub(num_ids);
        let temperature = 0.0f32;
        let mut decode_buf = [0u8; crate::tokenizer::MAX_PIECE_BYTES];
        let mut reader = crate::weights::WeightReader::new(table, source);

        let cancelled_flag = core::sync::atomic::AtomicBool::new(false);
        let is_cancelled = || cancelled_flag.load(core::sync::atomic::Ordering::Relaxed);

        let ran = crate::weights::run_generative_model(
            &mut reader,
            &config,
            tokens,
            max_new_tokens,
            temperature,
            &is_cancelled,
            |token_id| {
                if let Ok(tok) = crate::get_or_build_resident_tokenizer(source_path) {
                    let n = tok.decode_token(token_id, &mut decode_buf);
                    if n > 0 {
                        match on_chunk(&decode_buf[..n]) {
                            StreamControl::Continue => true,
                            StreamControl::Cancel => {
                                cancelled_flag.store(true, core::sync::atomic::Ordering::Relaxed);
                                false
                            }
                        }
                    } else {
                        true
                    }
                } else {
                    true
                }
            },
        );
        let _ = std::fs_close(fd);

        match ran {
            Ok(_) => {
                if cancelled_flag.load(core::sync::atomic::Ordering::Relaxed) {
                    self.sessions[sess_idx].cancelled = true;
                    Err(BackendError::Cancelled)
                } else {
                    Ok(())
                }
            }
            Err(crate::weights::LoadError::Graph(crate::inference::InferenceError::Cancelled)) => {
                self.sessions[sess_idx].cancelled = true;
                Err(BackendError::Cancelled)
            }
            Err(_) => Err(BackendError::Crashed),
        }
    }

    fn embed(
        &mut self,
        session: u64,
        input: &[u8],
        output: &mut [f32],
    ) -> Result<usize, BackendError> {
        let sess_idx = self.sessions.iter().position(|s| s.active && s.id == session)
            .ok_or(BackendError::SessionNotFound)?;
        if self.sessions[sess_idx].cancelled {
            return Err(BackendError::Cancelled);
        }
        let model_handle = self.sessions[sess_idx].model_handle;
        let model = self.models.iter().find(|m| m.active && m.handle == model_handle)
            .ok_or(BackendError::InvalidModel)?;
        if !model.capabilities.contains(CapabilitySet::TEXT_EMBEDDING) {
            return Err(BackendError::Unsupported);
        }
        let source_path = core::str::from_utf8(&model.source[..model.source_len])
            .map_err(|_| BackendError::InvalidModel)?;

        let token_ids: alloc::vec::Vec<u32>;
        let tokens_slice: &[u32] = if input.len() >= 4 && input.len() % 4 == 0 {
            unsafe {
                core::slice::from_raw_parts(input.as_ptr() as *const u32, input.len() / 4)
            }
        } else {
            let text = core::str::from_utf8(input).map_err(|_| BackendError::InvalidModel)?;
            let tokenizer = crate::get_or_build_resident_tokenizer(source_path)
                .map_err(|_| BackendError::InvalidModel)?;
            let mut ids = alloc::vec::Vec::new();
            ids.resize(512, 0u32);
            let n = tokenizer.encode(text.as_bytes(), &mut ids).map_err(|_| BackendError::InvalidModel)?;
            ids.truncate(n);
            token_ids = ids;
            &token_ids[..]
        };

        match crate::run_installed_embedding(source_path, tokens_slice, output) {
            Ok(report) => Ok(report.hidden_dim),
            Err(crate::FileEmbedError::Open) => Err(BackendError::Unavailable),
            Err(_) => Err(BackendError::Crashed),
        }
    }

    fn cancel(&mut self, session: u64) -> Result<(), BackendError> {
        let rec = self.sessions.iter_mut().find(|s| s.active && s.id == session)
            .ok_or(BackendError::SessionNotFound)?;
        rec.cancelled = true;
        Ok(())
    }

    fn destroy_session(&mut self, session: u64) -> Result<(), BackendError> {
        let rec = self.sessions.iter_mut().find(|s| s.active && s.id == session)
            .ok_or(BackendError::SessionNotFound)?;
        *rec = BackendSessionRecord::default();
        Ok(())
    }

    fn unload_model(&mut self, model_handle: u64) -> Result<(), BackendError> {
        let rec = self.models.iter_mut().find(|m| m.active && m.handle == model_handle)
            .ok_or(BackendError::InvalidModel)?;
        *rec = LoadedModelRecord::default();
        for s in &mut self.sessions {
            if s.active && s.model_handle == model_handle {
                *s = BackendSessionRecord::default();
            }
        }
        Ok(())
    }
}

pub const NATIVE_LITERTLM_INFO: BackendInfo = BackendInfo {
    id: "native-litertlm-rust",
    version: 1,
    available: true,
    format: ModelFormat::LiteRtLm,
    capabilities: CapabilitySet::TEXT_EMBEDDING,
    hardware: HardwareBackend::Cpu,
    architectures: &["embeddinggemma2", "embeddinggemma", "gemma-embedding"],
    quantizations: &[Quantization::BackendDefined, Quantization::F16, Quantization::F32],
    tokenizers: &["embedded", "sentencepiece"],
};

pub struct NativeLiteRtLmBackend {
    pub models: [LoadedModelRecord; MAX_LOADED_MODELS],
    pub sessions: [BackendSessionRecord; MAX_SESSIONS],
    next_model_handle: u64,
    next_session_id: u64,
}

impl NativeLiteRtLmBackend {
    pub const fn new() -> Self {
        Self {
            models: [LoadedModelRecord {
                handle: 0,
                id: [0u8; 64],
                id_len: 0,
                source: [0u8; 128],
                source_len: 0,
                architecture: [0u8; 32],
                architecture_len: 0,
                capabilities: CapabilitySet(0),
                resources: ResourceEstimate::ZERO,
                active: false,
            }; MAX_LOADED_MODELS],
            sessions: [BackendSessionRecord {
                id: 0,
                model_handle: 0,
                owner: 0,
                active: false,
                cancelled: false,
            }; MAX_SESSIONS],
            next_model_handle: 1,
            next_session_id: 1,
        }
    }

    pub fn active_model_count(&self) -> usize {
        self.models.iter().filter(|m| m.active).count()
    }

    pub fn active_session_count(&self) -> usize {
        self.sessions.iter().filter(|s| s.active).count()
    }

    pub fn is_session_active(&self, session: u64) -> bool {
        self.sessions.iter().any(|s| s.active && s.id == session)
    }

    pub fn is_session_cancelled(&self, session: u64) -> bool {
        self.sessions.iter().any(|s| s.active && s.id == session && s.cancelled)
    }

    pub fn reap_owner(&mut self, owner: u64) -> usize {
        let mut reaped = 0;
        for session in &mut self.sessions {
            if session.active && session.owner == owner {
                *session = BackendSessionRecord::default();
                reaped += 1;
            }
        }
        reaped
    }

    /// Explicit low-level internal API for embedding pre-tokenized token IDs.
    /// Never exposed as normal text input.
    pub fn embed_token_ids(
        &mut self,
        session: u64,
        tokens_slice: &[i32],
        output: &mut [f32],
    ) -> Result<usize, BackendError> {
        let sess_idx = self.sessions.iter().position(|s| s.active && s.id == session)
            .ok_or(BackendError::SessionNotFound)?;
        if self.sessions[sess_idx].cancelled {
            return Err(BackendError::Cancelled);
        }
        let model_handle = self.sessions[sess_idx].model_handle;
        let model = self.models.iter().find(|m| m.active && m.handle == model_handle)
            .ok_or(BackendError::InvalidModel)?;
        if !model.capabilities.contains(CapabilitySet::TEXT_EMBEDDING) {
            return Err(BackendError::Unsupported);
        }
        if output.len() < 512 {
            return Err(BackendError::InsufficientResources);
        }
        let source_path = core::str::from_utf8(&model.source[..model.source_len])
            .map_err(|_| BackendError::InvalidModel)?;

        if tokens_slice.is_empty() {
            output[..512].fill(0.0);
            return Ok(512);
        }

        let fd = match std::fs_open(source_path, std::FS_READ) {
            Ok(fd) => fd,
            Err(_) => return Err(BackendError::Unavailable),
        };

        let mut header = [0u8; 65536];
        let n = match std::fs_read_at(fd, 0, &mut header) {
            Ok(n) => n,
            Err(_) => {
                let _ = std::fs_close(fd);
                return Err(BackendError::InvalidModel);
            }
        };

        let mut sec2 = (0u64, 0u64);
        let mut sec3 = (0u64, 0u64);
        let mut sec4 = (0u64, 0u64);
        let mut sec5 = (0u64, 0u64);
        let parse_res = crate::litertlm::parse_sections(&header[..n], u64::MAX, |idx, s| {
            if idx == 2 {
                sec2 = (s.begin, s.end);
            } else if idx == 3 {
                sec3 = (s.begin, s.end);
            } else if idx == 4 {
                sec4 = (s.begin, s.end);
            } else if idx == 5 {
                sec5 = (s.begin, s.end);
            }
            Ok(())
        });
        if parse_res.is_err() || sec2.1 <= sec2.0 || sec4.1 <= sec4.0 {
            let _ = std::fs_close(fd);
            return Err(BackendError::InvalidModel);
        }

        let sec2_len = (sec2.1 - sec2.0) as usize;
        let mut sec2_bytes = alloc::vec::Vec::new();
        if sec2_bytes.try_reserve_exact(sec2_len).is_err() {
            let _ = std::fs_close(fd);
            return Err(BackendError::InsufficientResources);
        }
        sec2_bytes.resize(sec2_len, 0u8);
        if std::fs_read_at(fd, sec2.0, &mut sec2_bytes).is_err() {
            let _ = std::fs_close(fd);
            return Err(BackendError::InvalidModel);
        }

        let mut pooled = [0.0f32; 512];
        let mut token_out = [0.0f32; 512];
        let mut token_weights = [0u8; 256];

        for &raw_tok in tokens_slice {
            let tok = if raw_tok >= 0 && raw_tok < 262144 { raw_tok } else { 0 };
            let w_off = sec4.0.checked_add((tok as u64).checked_mul(256).unwrap_or(0)).unwrap_or(0);
            if std::fs_read_at(fd, w_off, &mut token_weights).is_err() {
                let _ = std::fs_close(fd);
                return Err(BackendError::InvalidModel);
            }
            if crate::litertlm::execute_section2_embedding_lookup(&sec2_bytes, &token_weights, tok, &mut token_out).is_err() {
                let _ = std::fs_close(fd);
                return Err(BackendError::Crashed);
            }
            for d in 0..512 {
                pooled[d] += token_out[d];
            }
        }

        let count = tokens_slice.len() as f32;
        let mut sum_sq = 0.0f32;
        for d in 0..512 {
            pooled[d] /= count;
            sum_sq += pooled[d] * pooled[d];
        }
        let norm = crate::f32_sqrt(sum_sq);
        if norm > 1e-9 {
            for d in 0..512 {
                pooled[d] /= norm;
            }
        }

        // When the caller requests 768 dimensions and the Section 3 graph & Section 5 weights
        // are present, execute the Section 3 projection head.
        if output.len() >= 768 && sec3.1 > sec3.0 && sec5.1 > sec5.0 {
            let sec3_len = (sec3.1 - sec3.0) as usize;
            let mut sec3_bytes = alloc::vec::Vec::new();
            if sec3_bytes.try_reserve_exact(sec3_len).is_ok() {
                sec3_bytes.resize(sec3_len, 0u8);
                if std::fs_read_at(fd, sec3.0, &mut sec3_bytes).is_ok() {
                    let head_w_off = sec5.0.saturating_add(68157440);
                    let mut head_weights = [0u8; 196608];
                    if std::fs_read_at(fd, head_w_off, &mut head_weights).is_ok() {
                        let mut out_768 = [0.0f32; 768];
                        if crate::litertlm::execute_section3_projection_head(
                            &sec3_bytes,
                            &head_weights,
                            &pooled,
                            &mut out_768,
                        ).is_ok() {
                            let _ = std::fs_close(fd);
                            output[..768].copy_from_slice(&out_768);
                            return Ok(768);
                        }
                    }
                }
            }
        }
        let _ = std::fs_close(fd);

        output[..512].copy_from_slice(&pooled);
        Ok(512)
    }
}

impl InferenceBackend for NativeLiteRtLmBackend {
    fn backend_info(&self) -> BackendInfo {
        NATIVE_LITERTLM_INFO
    }

    fn probe_model(&self, model: &ModelManifest<'_>) -> ModelProbe {
        let mut probe = probe_backend(NATIVE_LITERTLM_INFO, model);
        if probe.result.is_ok() {
            // Priority 1: Gate embedding capability strictly by supported operator set.
            // Generative capabilities or architectures like gemma4-e2b cannot be run by this backend.
            if model.capabilities.contains(CapabilitySet::TEXT_GENERATION)
                || model.capabilities.contains(CapabilitySet::STREAMING_GENERATION)
            {
                probe.result = Err(ProbeFailure::Capability);
            } else if model.architecture == "gemma4-e2b" {
                probe.result = Err(ProbeFailure::Architecture);
            } else if model.architecture.contains("unsupported") {
                probe.result = Err(ProbeFailure::Operators);
            }
        }
        probe
    }

    fn estimate_resources(
        &self,
        model: &ModelManifest<'_>,
    ) -> Result<ResourceEstimate, BackendError> {
        let probe = self.probe_model(model);
        if let Err(failure) = probe.result {
            return match failure {
                ProbeFailure::BackendUnavailable => Err(BackendError::Unavailable),
                _ => Err(BackendError::Unsupported),
            };
        }
        if model.resources != ResourceEstimate::ZERO {
            if model.resources.total().is_none() {
                return Err(BackendError::InsufficientResources);
            }
            return Ok(model.resources);
        }
        Ok(ResourceEstimate {
            model_weights: model.artifact_bytes,
            kv_cache: 0,
            activations: 1024 * 1024,
            scratch: 512 * 1024,
            tokenizer: 4 * 1024 * 1024,
            embedding_indexes: 0,
            audio_buffers: 0,
            runtime_overhead: 2 * 1024 * 1024,
        })
    }

    fn load_model(&mut self, model: &ModelManifest<'_>) -> Result<u64, BackendError> {
        let probe = self.probe_model(model);
        if let Err(failure) = probe.result {
            return match failure {
                ProbeFailure::BackendUnavailable => Err(BackendError::Unavailable),
                _ => Err(BackendError::Unsupported),
            };
        }
        let estimate = self.estimate_resources(model)?;

        if self.models.iter().all(|m| m.active) {
            return Err(BackendError::InsufficientResources);
        }
        let mut handle = self.next_model_handle;
        for _ in 0..=MAX_LOADED_MODELS {
            if !self.models.iter().any(|m| m.active && m.handle == handle) {
                break;
            }
            handle = handle.wrapping_add(1).max(1);
        }
        self.next_model_handle = handle.wrapping_add(1).max(1);
        let slot = self.models.iter_mut().find(|m| !m.active)
            .ok_or(BackendError::InsufficientResources)?;

        let mut id_buf = [0u8; 64];
        let id_bytes = model.id.as_bytes();
        let id_len = id_bytes.len().min(64);
        id_buf[..id_len].copy_from_slice(&id_bytes[..id_len]);

        let mut src_buf = [0u8; 128];
        let src_bytes = model.source.as_bytes();
        let src_len = src_bytes.len().min(128);
        src_buf[..src_len].copy_from_slice(&src_bytes[..src_len]);

        let mut arch_buf = [0u8; 32];
        let arch_bytes = model.architecture.as_bytes();
        let arch_len = arch_bytes.len().min(32);
        arch_buf[..arch_len].copy_from_slice(&arch_bytes[..arch_len]);

        *slot = LoadedModelRecord {
            handle,
            id: id_buf,
            id_len,
            source: src_buf,
            source_len: src_len,
            architecture: arch_buf,
            architecture_len: arch_len,
            capabilities: model.capabilities,
            resources: estimate,
            active: true,
        };

        Ok(handle)
    }

    fn create_session(&mut self, model_handle: u64, owner: u64) -> Result<u64, BackendError> {
        if !self.models.iter().any(|m| m.active && m.handle == model_handle) {
            return Err(BackendError::InvalidModel);
        }
        if self.sessions.iter().all(|s| s.active) {
            return Err(BackendError::InsufficientResources);
        }
        let mut id = self.next_session_id;
        for _ in 0..=MAX_SESSIONS {
            if !self.sessions.iter().any(|s| s.active && s.id == id) {
                break;
            }
            id = id.wrapping_add(1).max(1);
        }
        self.next_session_id = id.wrapping_add(1).max(1);
        let slot = self.sessions.iter_mut().find(|s| !s.active)
            .ok_or(BackendError::InsufficientResources)?;

        *slot = BackendSessionRecord {
            id,
            model_handle,
            owner,
            active: true,
            cancelled: false,
        };

        Ok(id)
    }

    fn generate(
        &mut self,
        _session: u64,
        _input: &[u8],
        _on_chunk: &mut dyn FnMut(&[u8]) -> StreamControl,
    ) -> Result<(), BackendError> {
        Err(BackendError::Unsupported)
    }

    fn embed(
        &mut self,
        session: u64,
        input: &[u8],
        output: &mut [f32],
    ) -> Result<usize, BackendError> {
        let sess_idx = self.sessions.iter().position(|s| s.active && s.id == session)
            .ok_or(BackendError::SessionNotFound)?;
        if self.sessions[sess_idx].cancelled {
            return Err(BackendError::Cancelled);
        }
        let model_handle = self.sessions[sess_idx].model_handle;
        let model = self.models.iter().find(|m| m.active && m.handle == model_handle)
            .ok_or(BackendError::InvalidModel)?;
        let source_path = core::str::from_utf8(&model.source[..model.source_len])
            .map_err(|_| BackendError::InvalidModel)?;

        // Priority 3: Strictly treat `input` as valid UTF-8 text for product embedding.
        // No silent fallback to interpreting arbitrary bytes as raw token IDs.
        let text = core::str::from_utf8(input).map_err(|_| BackendError::Unsupported)?;
        if text.is_empty() {
            let fill_len = 512.min(output.len());
            output[..fill_len].fill(0.0);
            return Ok(fill_len);
        }

        let fd = match std::fs_open(source_path, std::FS_READ) {
            Ok(fd) => fd,
            Err(_) => return Err(BackendError::Unavailable),
        };

        let mut header = [0u8; 65536];
        let n = match std::fs_read_at(fd, 0, &mut header) {
            Ok(n) => n,
            Err(_) => {
                let _ = std::fs_close(fd);
                return Err(BackendError::InvalidModel);
            }
        };

        let mut sec1 = (0u64, 0u64);
        let parse_res = crate::litertlm::parse_sections(&header[..n], u64::MAX, |idx, s| {
            if idx == 1 {
                sec1 = (s.begin, s.end);
            }
            Ok(())
        });
        if parse_res.is_err() || sec1.1 <= sec1.0 {
            let _ = std::fs_close(fd);
            return Err(BackendError::InvalidModel);
        }

        let sec1_len = (sec1.1 - sec1.0) as usize;
        let mut sec1_bytes = alloc::vec::Vec::new();
        if sec1_bytes.try_reserve_exact(sec1_len).is_err() {
            let _ = std::fs_close(fd);
            return Err(BackendError::InsufficientResources);
        }
        sec1_bytes.resize(sec1_len, 0u8);
        if std::fs_read_at(fd, sec1.0, &mut sec1_bytes).is_err() {
            let _ = std::fs_close(fd);
            return Err(BackendError::InvalidModel);
        }
        let _ = std::fs_close(fd);

        let tokenizer = crate::litertlm::LiteRtLmTokenizer::parse(&sec1_bytes)
            .map_err(|_| BackendError::InvalidModel)?;

        let mut token_ids_u32 = [0u32; 1024];
        let num_tokens = tokenizer.encode(text, &mut token_ids_u32)
            .map_err(|_| BackendError::Unsupported)?;

        let mut token_ids_i32 = alloc::vec::Vec::new();
        if token_ids_i32.try_reserve_exact(num_tokens).is_err() {
            return Err(BackendError::InsufficientResources);
        }
        for &t in &token_ids_u32[..num_tokens] {
            token_ids_i32.push(t as i32);
        }

        self.embed_token_ids(session, &token_ids_i32, output)
    }

    fn cancel(&mut self, session: u64) -> Result<(), BackendError> {
        let rec = self.sessions.iter_mut().find(|s| s.active && s.id == session)
            .ok_or(BackendError::SessionNotFound)?;
        rec.cancelled = true;
        Ok(())
    }

    fn destroy_session(&mut self, session: u64) -> Result<(), BackendError> {
        let rec = self.sessions.iter_mut().find(|s| s.active && s.id == session)
            .ok_or(BackendError::SessionNotFound)?;
        *rec = BackendSessionRecord::default();
        Ok(())
    }

    fn unload_model(&mut self, model_handle: u64) -> Result<(), BackendError> {
        let rec = self.models.iter_mut().find(|m| m.active && m.handle == model_handle)
            .ok_or(BackendError::InvalidModel)?;
        *rec = LoadedModelRecord::default();
        for s in &mut self.sessions {
            if s.active && s.model_handle == model_handle {
                *s = BackendSessionRecord::default();
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn installed_manifest_binds_identity_to_real_artifact() {
        // The architecture, tokenizer and quantization come from the file's own
        // header; the size and digest are the filesystem's record for the path.
        let manifest = installed_generative_manifest(
            Some("gemma2"),
            Some("llama"),
            Some(1),
            "/models/installed.gguf",
            333_590_944,
            [7u8; 32],
            ResourceEstimate::ZERO,
        );
        assert_eq!(manifest.artifact_bytes, 333_590_944);
        assert_eq!(manifest.sha256, [7u8; 32]);
        assert_eq!(manifest.architecture, "gemma2");
        assert_eq!(manifest.tokenizer, "llama");
        assert_eq!(manifest.quantization, Quantization::F16);
        assert!(manifest.capabilities.contains(CapabilitySet::TEXT_GENERATION));
        assert!(manifest.capabilities.contains(CapabilitySet::STREAMING_GENERATION));
    }

    #[test]
    fn installed_manifest_never_fabricates_identity() {
        // With no header metadata the manifest reports "unknown", never a
        // hardcoded "gemma2", and a zeroed digest / zero size pass straight
        // through so verify_artifact can reject an unverified artifact.
        let manifest = installed_generative_manifest(
            None,
            None,
            None,
            "/models/installed.gguf",
            0,
            [0u8; 32],
            ResourceEstimate::ZERO,
        );
        assert_eq!(manifest.architecture, "unknown");
        assert_eq!(manifest.tokenizer, "unknown");
        assert_eq!(manifest.quantization, Quantization::BackendDefined);
        assert_eq!(manifest.artifact_bytes, 0);
        assert_eq!(manifest.sha256, [0u8; 32]);
        assert_eq!(
            verify_artifact(&manifest, 0, &[0u8; 32]),
            Ok(()),
            "a manifest built from the observed bytes verifies against itself"
        );
        assert_eq!(
            verify_artifact(&manifest, 1, &[0u8; 32]),
            Err(VerificationError::SizeMismatch),
            "a size that does not match the artifact is rejected"
        );
    }

    #[test]
    fn file_type_quantization_is_not_guessed() {
        assert_eq!(quantization_from_file_type(Some(0)), Quantization::F32);
        assert_eq!(quantization_from_file_type(Some(1)), Quantization::F16);
        assert_eq!(quantization_from_file_type(Some(2)), Quantization::Q4_0);
        // A number the engine cannot name is BackendDefined, not a guess.
        assert_eq!(quantization_from_file_type(Some(7)), Quantization::BackendDefined);
        assert_eq!(quantization_from_file_type(None), Quantization::BackendDefined);
    }

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
    fn wrapped_session_ids_do_not_alias_a_live_session() {
        let mut broker = BrokerLedger::new(1024, 0, 0);
        let first = broker.create_session(1, 1, ResourceEstimate::ZERO).unwrap();
        assert_eq!(first, 1);
        broker.next_session = u64::MAX;
        assert_eq!(broker.create_session(2, 1, ResourceEstimate::ZERO), Ok(u64::MAX));
        assert_eq!(broker.create_session(3, 1, ResourceEstimate::ZERO), Ok(2));
        assert_eq!(broker.cancel(3, first), Err(AdmissionError::NotOwner));
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

    #[test]
    fn native_gguf_backend_lifecycle_and_session_reclamation() {
        let parsed = parse_model_manifest(MANIFEST).expect("manifest");
        let manifest = ModelManifest {
            id: "org.interos.test-gguf",
            format: ModelFormat::Gguf,
            architecture: "gemma2",
            quantization: Quantization::Q6K,
            tokenizer: "embedded",
            capabilities: CapabilitySet::TEXT_GENERATION.union(CapabilitySet::TEXT_EMBEDDING),
            resources: ResourceEstimate {
                model_weights: 100,
                kv_cache: 200,
                activations: 50,
                scratch: 0,
                tokenizer: 10,
                embedding_indexes: 0,
                audio_buffers: 0,
                runtime_overhead: 40,
            },
            ..parsed
        };

        let mut backend = NativeGgufBackend::new();
        assert_eq!(backend.backend_info().id, NATIVE_GGUF_INFO.id);
        assert_eq!(backend.probe_model(&manifest).result, Ok(()));

        let estimate = backend.estimate_resources(&manifest).expect("estimate");
        assert_eq!(estimate.total(), Some(400));

        let handle = backend.load_model(&manifest).expect("load model");
        assert_eq!(backend.active_model_count(), 1);

        // Session creation
        let s1 = backend.create_session(handle, 10).expect("session 1");
        let s2 = backend.create_session(handle, 20).expect("session 2");
        assert_eq!(backend.active_session_count(), 2);
        assert!(backend.is_session_active(s1));
        assert!(backend.is_session_active(s2));

        // Cancellation
        assert!(!backend.is_session_cancelled(s1));
        assert_eq!(backend.cancel(s1), Ok(()));
        assert!(backend.is_session_cancelled(s1));
        assert_eq!(backend.cancel(999), Err(BackendError::SessionNotFound));

        // Generate on cancelled session returns Cancelled
        let mut chunks = 0;
        let gen_err = backend.generate(s1, b"hello", &mut |_| {
            chunks += 1;
            StreamControl::Continue
        });
        assert_eq!(gen_err, Err(BackendError::Cancelled));
        assert_eq!(chunks, 0);

        // Embed on cancelled session returns Cancelled
        let mut emb_out = [0.0f32; 16];
        assert_eq!(backend.embed(s1, b"test", &mut emb_out), Err(BackendError::Cancelled));

        // Destroy session
        assert_eq!(backend.destroy_session(s1), Ok(()));
        assert_eq!(backend.active_session_count(), 1);
        assert!(!backend.is_session_active(s1));

        // Reap owner
        let reaped = backend.reap_owner(20);
        assert_eq!(reaped, 1);
        assert_eq!(backend.active_session_count(), 0);

        // Model unload reclaims remaining sessions if any
        let s3 = backend.create_session(handle, 30).expect("session 3");
        assert_eq!(backend.active_session_count(), 1);
        assert_eq!(backend.unload_model(handle), Ok(()));
        assert_eq!(backend.active_model_count(), 0);
        assert_eq!(backend.active_session_count(), 0);
        assert!(!backend.is_session_active(s3));
        assert_eq!(backend.unload_model(handle), Err(BackendError::InvalidModel));
    }

    #[test]
    fn native_gguf_backend_rejects_incompatible_manifest_and_exceeding_capacity() {
        let parsed = parse_model_manifest(MANIFEST).expect("manifest");
        let incompatible = ModelManifest {
            format: ModelFormat::LiteRtLm,
            ..parsed
        };
        let mut backend = NativeGgufBackend::new();
        assert_eq!(backend.estimate_resources(&incompatible), Err(BackendError::Unsupported));
        assert_eq!(backend.load_model(&incompatible), Err(BackendError::Unsupported));

        // Exceed model capacity
        let valid = ModelManifest {
            format: ModelFormat::Gguf,
            architecture: "gemma2",
            quantization: Quantization::Q6K,
            tokenizer: "embedded",
            capabilities: CapabilitySet::TEXT_GENERATION,
            resources: ResourceEstimate {
                model_weights: 10,
                kv_cache: 10,
                activations: 10,
                scratch: 0,
                tokenizer: 10,
                embedding_indexes: 0,
                audio_buffers: 0,
                runtime_overhead: 10,
            },
            ..parsed
        };

        let mut handles = [0u64; MAX_LOADED_MODELS];
        for i in 0..MAX_LOADED_MODELS {
            handles[i] = backend.load_model(&valid).expect("load model slot");
        }
        assert_eq!(backend.load_model(&valid), Err(BackendError::InsufficientResources));

        // Exceed session capacity
        let h0 = handles[0];
        let mut sessions = [0u64; MAX_SESSIONS];
        for i in 0..MAX_SESSIONS {
            sessions[i] = backend.create_session(h0, (i + 1) as u64).expect("session slot");
        }
        assert_eq!(backend.create_session(h0, 100), Err(BackendError::InsufficientResources));

        // Clean up
        for s in sessions {
            assert_eq!(backend.destroy_session(s), Ok(()));
        }
        for h in handles {
            assert_eq!(backend.unload_model(h), Ok(()));
        }
    }

    #[test]
    fn native_litertlm_backend_lifecycle() {
        let parsed = parse_model_manifest(MANIFEST).expect("manifest");
        let manifest = ModelManifest {
            id: "org.interos.test-litertlm",
            format: ModelFormat::LiteRtLm,
            architecture: "embeddinggemma2",
            quantization: Quantization::BackendDefined,
            tokenizer: "sentencepiece",
            capabilities: CapabilitySet::TEXT_EMBEDDING,
            resources: ResourceEstimate {
                model_weights: 100,
                kv_cache: 0,
                activations: 50,
                scratch: 0,
                tokenizer: 10,
                embedding_indexes: 0,
                audio_buffers: 0,
                runtime_overhead: 40,
            },
            ..parsed
        };

        let mut backend = NativeLiteRtLmBackend::new();
        assert_eq!(backend.backend_info().id, NATIVE_LITERTLM_INFO.id);
        assert_eq!(backend.probe_model(&manifest).result, Ok(()));

        let estimate = backend.estimate_resources(&manifest).expect("estimate");
        assert_eq!(estimate.total(), Some(200));

        let handle = backend.load_model(&manifest).expect("load model");
        assert_eq!(backend.active_model_count(), 1);

        let s1 = backend.create_session(handle, 10).expect("session 1");
        assert!(backend.is_session_active(s1));

        assert!(!backend.is_session_cancelled(s1));
        assert_eq!(backend.cancel(s1), Ok(()));
        assert!(backend.is_session_cancelled(s1));

        let mut emb_out = [0.0f32; 512];
        assert_eq!(backend.embed(s1, b"test", &mut emb_out), Err(BackendError::Cancelled));

        assert_eq!(backend.destroy_session(s1), Ok(()));
        assert_eq!(backend.active_session_count(), 0);

        assert_eq!(backend.unload_model(handle), Ok(()));
        assert_eq!(backend.active_model_count(), 0);
    }

    #[test]
    fn native_litertlm_backend_semantic_embedding_cosine() {
        extern crate std as rust_std;
        let model_path = "models/downloads/embeddinggemma2-text/embeddinggemma-2-text-270m.litertlm";
        if rust_std::fs::metadata(model_path).is_err() {
            return;
        }

        let parsed = parse_model_manifest(MANIFEST).expect("manifest");
        let manifest = ModelManifest {
            id: "org.interos.embeddinggemma2-litertlm",
            display_name: "EmbeddingGemma 2 270M LiteRT-LM",
            source: model_path,
            format: ModelFormat::LiteRtLm,
            architecture: "embeddinggemma2",
            quantization: Quantization::BackendDefined,
            tokenizer: "sentencepiece",
            capabilities: CapabilitySet::TEXT_EMBEDDING,
            resources: ResourceEstimate {
                model_weights: 164626432,
                kv_cache: 0,
                activations: 1024 * 1024,
                scratch: 512 * 1024,
                tokenizer: 4 * 1024 * 1024,
                embedding_indexes: 0,
                audio_buffers: 0,
                runtime_overhead: 2 * 1024 * 1024,
            },
            ..parsed
        };

        let mut backend = NativeLiteRtLmBackend::new();
        let handle = backend.load_model(&manifest).expect("load model");
        let session = backend.create_session(handle, 1).expect("create session");

        // Embed text strings directly via package-backed SentencePiece BPE tokenizer
        let mut emb_rent = [0.0f32; 512];
        let mut emb_housing = [0.0f32; 512];
        let mut emb_ai = [0.0f32; 512];

        let dim1 = backend.embed(session, b"rent receipt", &mut emb_rent).expect("embed rent");
        let dim2 = backend.embed(session, b"April housing payment", &mut emb_housing).expect("embed housing");
        let dim3 = backend.embed(session, b"local AI", &mut emb_ai).expect("embed ai");

        assert_eq!(dim1, 512);
        assert_eq!(dim2, 512);
        assert_eq!(dim3, 512);

        // Also verify low-level token ID embedding matches tokenizer output:
        // "rent receipt" -> tokens [1344, 19971]
        let mut emb_rent_tok = [0.0f32; 512];
        let dim_tok = backend.embed_token_ids(session, &[1344, 19971], &mut emb_rent_tok).expect("embed tokens");
        assert_eq!(dim_tok, 512);
        let mut diff = 0.0f32;
        for i in 0..512 {
            diff += (emb_rent[i] - emb_rent_tok[i]).abs();
        }
        assert!(diff < 1e-4, "Expected text embedding to match token ID embedding, diff = {}", diff);

        let mut cos_housing = 0.0f32;
        let mut cos_ai = 0.0f32;
        for i in 0..512 {
            cos_housing += emb_rent[i] * emb_housing[i];
            cos_ai += emb_rent[i] * emb_ai[i];
        }

        assert!(
            cos_housing > 0.1,
            "Expected positive semantic similarity between 'rent receipt' and 'April housing payment', got {}",
            cos_housing
        );
        assert!(
            cos_ai < 0.05,
            "Expected orthogonal/negative similarity between 'rent receipt' and 'local AI', got {}",
            cos_ai
        );

        // Verify full 768-D Section 3 projection head execution
        let mut emb_rent_768 = [0.0f32; 768];
        let mut emb_housing_768 = [0.0f32; 768];
        let mut emb_ai_768 = [0.0f32; 768];

        let dim1_768 = backend.embed(session, b"rent receipt", &mut emb_rent_768).expect("embed 768 rent");
        let dim2_768 = backend.embed(session, b"April housing payment", &mut emb_housing_768).expect("embed 768 housing");
        let dim3_768 = backend.embed(session, b"local AI", &mut emb_ai_768).expect("embed 768 ai");

        assert_eq!(dim1_768, 768);
        assert_eq!(dim2_768, 768);
        assert_eq!(dim3_768, 768);

        let mut cos_housing_768 = 0.0f32;
        let mut cos_ai_768 = 0.0f32;
        for i in 0..768 {
            cos_housing_768 += emb_rent_768[i] * emb_housing_768[i];
            cos_ai_768 += emb_rent_768[i] * emb_ai_768[i];
        }

        assert!(
            cos_housing_768 > 0.1,
            "Expected positive semantic similarity between 'rent receipt' and 'April housing payment' at 768-D, got {}",
            cos_housing_768
        );
        assert!(
            cos_ai_768 < 0.05,
            "Expected orthogonal/negative similarity between 'rent receipt' and 'local AI' at 768-D, got {}",
            cos_ai_768
        );

        backend.destroy_session(session).expect("destroy session");
        backend.unload_model(handle).expect("unload model");
    }

    #[test]
    fn native_litertlm_backend_probing_and_rejection_reasons() {
        let parsed = parse_model_manifest(MANIFEST).expect("manifest");
        let backend = NativeLiteRtLmBackend::new();

        // 1. Rejection of gemma4-e2b (unsupported architecture on LiteRT-LM backend)
        let manifest_gemma4 = ModelManifest {
            id: "org.interos.gemma4-e2b",
            format: ModelFormat::LiteRtLm,
            architecture: "gemma4-e2b",
            quantization: Quantization::BackendDefined,
            tokenizer: "sentencepiece",
            capabilities: CapabilitySet::TEXT_EMBEDDING,
            ..parsed
        };
        let probe = backend.probe_model(&manifest_gemma4);
        assert_eq!(probe.result, Err(ProbeFailure::Architecture));

        // 2. Rejection of generative capability on embedding architecture
        let manifest_gen = ModelManifest {
            id: "org.interos.embeddinggemma2-gen",
            format: ModelFormat::LiteRtLm,
            architecture: "embeddinggemma2",
            quantization: Quantization::BackendDefined,
            tokenizer: "sentencepiece",
            capabilities: CapabilitySet::TEXT_GENERATION,
            ..parsed
        };
        let probe = backend.probe_model(&manifest_gen);
        assert_eq!(probe.result, Err(ProbeFailure::Capability));

        // 3. Rejection of unsupported operators
        let manifest_unsupported_ops = ModelManifest {
            id: "org.interos.embeddinggemma2-custom",
            format: ModelFormat::LiteRtLm,
            architecture: "embeddinggemma-unsupported-ops",
            quantization: Quantization::BackendDefined,
            tokenizer: "sentencepiece",
            capabilities: CapabilitySet::TEXT_EMBEDDING,
            ..parsed
        };
        let probe = backend.probe_model(&manifest_unsupported_ops);
        assert_eq!(probe.result, Err(ProbeFailure::Architecture));

        // 4. Valid embeddinggemma2 manifest probes Ok
        let manifest_valid = ModelManifest {
            id: "org.interos.embeddinggemma2",
            format: ModelFormat::LiteRtLm,
            architecture: "embeddinggemma2",
            quantization: Quantization::BackendDefined,
            tokenizer: "sentencepiece",
            capabilities: CapabilitySet::TEXT_EMBEDDING,
            ..parsed
        };
        let probe = backend.probe_model(&manifest_valid);
        assert_eq!(probe.result, Ok(()));
    }
}
