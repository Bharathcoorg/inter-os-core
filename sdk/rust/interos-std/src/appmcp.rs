// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

//! Sovereign AppMCP (Model Context Protocol) Wire Contract & Tool Orchestration
//!
//! WHAT THIS MODULE IS FOR: a pure `#![no_std]`, zero-heap-allocation tool execution
//! protocol that allows AI models (Gemma 4 Mobile Lite, EmbeddingGemma 2, Whisper, Kokoro)
//! and the Agent Manager to invoke application functions autonomously.
//!
//! ZERO AMBIENT AUTHORITY: Every tool request carries an explicit, generation-counted
//! `cap_handle`. The receiving app does NOT inspect that integer itself -- it asks the
//! kernel (syscall 166, [`crate::appmcp_authorize`]) whether the *sender* genuinely holds
//! the capability, passing the TaskId the IPC router stamped on the message. The kernel
//! resolves the handle by generation in the sender's own capability table and checks type,
//! rights, object scope, global revocation and delegated-lease validity, failing closed.
//! [`authorize_tool_sender`] is the gate every receiver calls before performing any
//! mutation or reading protected state.
//!
//! WHY the capability is named in the KERNEL's numeric space: the authority to run agent
//! tools is `CapabilityType::AgentTask = 33`. `SensitivePermission::AgentTask = 15` and the
//! manifest ids `0x5000_0000 + slot` are different spaces that happen to describe the same
//! permission; passing either of those numbers here would ask the kernel about a capability
//! that does not exist (15 is `ShmProducer`).
//!
//! BOUNDED IPC INVARIANT: All requests and responses are exactly 64 bytes (`repr(C)`),
//! fitting squarely into the InterOS microkernel's `IPC_MAX_PAYLOAD = 64` mailbox.
//! Payloads larger than 36 bytes (such as long note bodies or file streams) pass through
//! InterOS shared memory (SHM) pipe slots (`pipe_id`), preserving zero-copy performance.

/// Magic identifier for AppMCP Request: "MCPQ" (0x4D43_5051)
pub const APPMCP_MAGIC_REQUEST: u32 = 0x4D43_5051;

/// Magic identifier for AppMCP Response: "MCPR" (0x4D43_5052)
pub const APPMCP_MAGIC_RESPONSE: u32 = 0x4D43_5052;

/// IPC message type for tool invocation requests: "MCPI"
pub const APPMCP_MSG_INVOKE: u32 = 0x4D43_5049;

/// IPC message type for tool response replies: "MCPR"
pub const APPMCP_MSG_RESPONSE: u32 = 0x4D43_5052;

/// IPC message type for tool discovery queries: "MCPD"
pub const APPMCP_MSG_DISCOVER: u32 = 0x4D43_5044;

/// IPC message type for tool manifest notices: "MCPM"
pub const APPMCP_MSG_MANIFEST: u32 = 0x4D43_504D;

// Status codes returned in `AppMcpResponse::status`
pub const APPMCP_STATUS_SUCCESS: u32            = 0;
pub const APPMCP_STATUS_PERMISSION_DENIED: u32  = 1;
pub const APPMCP_STATUS_INVALID_HANDLE: u32     = 2;
pub const APPMCP_STATUS_TOOL_NOT_FOUND: u32     = 3;
pub const APPMCP_STATUS_INVALID_ARGUMENTS: u32  = 4;
pub const APPMCP_STATUS_STORAGE_FULL: u32       = 5;
pub const APPMCP_STATUS_UNAVAILABLE: u32        = 6;
pub const APPMCP_STATUS_INTERNAL_ERROR: u32     = 7;
pub const APPMCP_STATUS_NOT_FOUND: u32          = 8;

// Predefined Tool Action IDs across canonical native applications
// Notes:
pub const TOOL_ID_NOTES_CREATE: u32             = 0x4E54_4352; // b"NTCR"
pub const TOOL_ID_NOTES_LIST: u32               = 0x4E54_4C53; // b"NTLS"
pub const TOOL_ID_NOTES_READ: u32               = 0x4E54_5244; // b"NTRD"
pub const TOOL_ID_NOTES_DELETE: u32             = 0x4E54_444C; // b"NTDL"

// Messages:
pub const TOOL_ID_MESSAGES_SEND_SMS: u32        = 0x4D53_534E; // b"MSSN"
pub const TOOL_ID_MESSAGES_LIST_THREADS: u32    = 0x4D53_4C53; // b"MSLS"
pub const TOOL_ID_MESSAGES_LIST: u32            = TOOL_ID_MESSAGES_LIST_THREADS;
pub const TOOL_ID_MESSAGES_READ: u32            = 0x4D53_5244; // b"MSRD"
pub const TOOL_ID_MESSAGES_DELETE: u32          = 0x4D53_444C; // b"MSDL"

// Calendar:
pub const TOOL_ID_CALENDAR_SCHEDULE: u32        = 0x4341_5343; // b"CASC"
pub const TOOL_ID_CALENDAR_LIST_EVENTS: u32     = 0x4341_4C53; // b"CALS"
pub const TOOL_ID_CALENDAR_LIST: u32            = TOOL_ID_CALENDAR_LIST_EVENTS;
pub const TOOL_ID_CALENDAR_GET_DATE: u32        = 0x4341_4454; // b"CADT"

// File Manager:
pub const TOOL_ID_FILES_LIST: u32               = 0x464D_4C53; // b"FMLS"
pub const TOOL_ID_FILES_READ: u32               = 0x464D_5244; // b"FMRD"
pub const TOOL_ID_FILES_WRITE: u32              = 0x464D_5752; // b"FMWR"
pub const TOOL_ID_FILES_STAT: u32               = 0x464D_5354; // b"FMST"

// Settings:
pub const TOOL_ID_SETTINGS_GET: u32             = 0x5354_4754; // b"STGT"
pub const TOOL_ID_SETTINGS_SET: u32             = 0x5354_5354; // b"STST"

// Calculator:
pub const TOOL_ID_CALCULATOR_EVAL: u32          = 0x434C_4556; // b"CLEV"

// Dialer:
pub const TOOL_ID_DIALER_CALL: u32              = 0x444C_434C; // b"DLCL"
pub const TOOL_ID_DIALER_HANGUP: u32            = 0x444C_4847; // b"DLHG"
pub const TOOL_ID_DIALER_LOGS: u32              = 0x444C_4C47; // b"DLLG"

// Music:
pub const TOOL_ID_MUSIC_PLAY: u32               = 0x4D43_504C; // b"MCPL"
pub const TOOL_ID_MUSIC_PAUSE: u32              = 0x4D43_5053; // b"MCPS"
pub const TOOL_ID_MUSIC_STATUS: u32             = 0x4D43_5354; // b"MCST"
pub const TOOL_ID_MUSIC_LIST: u32               = 0x4D43_4C53; // b"MCLS"

/// The kernel capability type that authorises running an agent tool.
///
/// This is `CapabilityType::AgentTask` in the kernel's own encoding, which is
/// the space syscall 166's `cap_type_raw` argument reads. It is deliberately
/// not `SensitivePermission::AgentTask` (15) -- in the kernel's space 15 is
/// `ShmProducer`, so passing the permission number would verify the wrong
/// authority entirely.
pub const TOOL_CAP_TYPE: u64 = crate::cap_type::AGENT_TASK;

/// `Rights::INVOKE` (1 << 5) in the kernel's rights encoding: the right to
/// exercise a capability rather than merely read or write the object it names.
pub const TOOL_CAP_RIGHTS: u64 = 1 << 5;

/// The object the agent-tool grant is scoped to: the whole bounded tool domain.
///
/// WHY this mask and not 0: the consent broker and the app spawn path mint
/// `AgentTask` with `object_id = 0x1F` (the five bounded tools, bits 0..4), and
/// syscall 166 scopes by exact match -- a grant naming `0x1F` does not satisfy
/// a request for object 0. Asking for the domain the broker actually mints is
/// what keeps the legitimate Agent Manager path working. An unscoped grant
/// (`object_id = 0`, which the userspace Agent service holds) satisfies any
/// requested object, so it passes this gate too.
///
/// WHY one domain object and not one object per tool: the broker grants the
/// agent-task domain as a whole, and each tool's own syscall still checks the
/// resource authority that tool needs (telephony, filesystem, audio) on its
/// own. This gate is the outer limit, not the inner one.
pub const TOOL_CAP_OBJECT: u64 = 0x1F;

/// Whether the kernel confirmed that `sender` holds the agent-tool capability
/// named by `cap_handle`.
///
/// `sender` MUST be the TaskId the IPC router delivered with the message, not a
/// field of the request: the request is the caller's to write, the router's
/// stamp is not. Allocation-free -- one syscall, no buffer, no heap.
///
/// Fails closed: a zero handle, a forged non-zero handle, a wrong capability
/// type, insufficient rights, a different object scope, a revoked or
/// stale-generation handle and a terminated sender are all refusals, as is any
/// kernel answer this SDK does not recognise.
pub fn authorize_tool_sender(sender: u64, cap_handle: u64) -> bool {
    crate::appmcp_authorize(
        sender,
        cap_handle,
        TOOL_CAP_TYPE,
        TOOL_CAP_RIGHTS,
        TOOL_CAP_OBJECT,
    )
    .is_ok()
}

/// Exactly 64-byte AppMCP Request structure.
///
/// Layout (Natural 8-byte alignment, 0 padding bytes):
/// - cap_handle: u64 (offset 0..8)
/// - magic: u32 (offset 8..12)
/// - tool_id: u32 (offset 12..16)
/// - token: u32 (offset 16..20)
/// - pipe_id: u32 (offset 20..24)
/// - payload_len: u32 (offset 24..28)
/// - inline_payload: [u8; 36] (offset 28..64)
/// Total = 64 bytes.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AppMcpRequest {
    pub cap_handle: u64,
    pub magic: u32,
    pub tool_id: u32,
    pub token: u32,
    pub pipe_id: u32,
    pub payload_len: u32,
    pub inline_payload: [u8; 36],
}

const _: () = assert!(core::mem::size_of::<AppMcpRequest>() == 64);

impl AppMcpRequest {
    /// Creates a new AppMCP request with inline payload.
    pub const fn new(cap_handle: u64, tool_id: u32, token: u32) -> Self {
        Self {
            cap_handle,
            magic: APPMCP_MAGIC_REQUEST,
            tool_id,
            token,
            pipe_id: u32::MAX,
            payload_len: 0,
            inline_payload: [0u8; 36],
        }
    }

    /// Sets inline payload data up to 36 bytes.
    pub fn with_inline_payload(mut self, data: &[u8]) -> Self {
        let copy_len = if data.len() < 36 { data.len() } else { 36 };
        let mut idx = 0;
        while idx < copy_len {
            self.inline_payload[idx] = data[idx];
            idx += 1;
        }
        self.payload_len = copy_len as u32;
        self
    }

    /// Sets shared memory pipe reference for payloads exceeding 36 bytes.
    pub const fn with_pipe(mut self, pipe_id: u32, total_bytes: u32) -> Self {
        self.pipe_id = pipe_id;
        self.payload_len = total_bytes;
        self
    }

    /// Serializes into a 64-byte array for microkernel IPC transfer.
    pub fn to_bytes(&self) -> [u8; 64] {
        let mut buf = [0u8; 64];
        buf[0..8].copy_from_slice(&self.cap_handle.to_le_bytes());
        buf[8..12].copy_from_slice(&self.magic.to_le_bytes());
        buf[12..16].copy_from_slice(&self.tool_id.to_le_bytes());
        buf[16..20].copy_from_slice(&self.token.to_le_bytes());
        buf[20..24].copy_from_slice(&self.pipe_id.to_le_bytes());
        buf[24..28].copy_from_slice(&self.payload_len.to_le_bytes());
        buf[28..64].copy_from_slice(&self.inline_payload);
        buf
    }

    /// Deserializes from a slice if it matches 64 bytes and magic identifier.
    pub fn from_bytes(raw: &[u8]) -> Option<Self> {
        if raw.len() < 64 {
            return None;
        }
        let magic = u32::from_le_bytes([raw[8], raw[9], raw[10], raw[11]]);
        if magic != APPMCP_MAGIC_REQUEST {
            return None;
        }
        let cap_handle = u64::from_le_bytes([
            raw[0], raw[1], raw[2], raw[3], raw[4], raw[5], raw[6], raw[7],
        ]);
        let tool_id = u32::from_le_bytes([raw[12], raw[13], raw[14], raw[15]]);
        let token = u32::from_le_bytes([raw[16], raw[17], raw[18], raw[19]]);
        let pipe_id = u32::from_le_bytes([raw[20], raw[21], raw[22], raw[23]]);
        let payload_len = u32::from_le_bytes([raw[24], raw[25], raw[26], raw[27]]);
        if pipe_id == u32::MAX && payload_len > 36 {
            return None;
        }
        let mut inline_payload = [0u8; 36];
        inline_payload.copy_from_slice(&raw[28..64]);

        Some(Self {
            cap_handle,
            magic,
            tool_id,
            token,
            pipe_id,
            payload_len,
            inline_payload,
        })
    }

    /// Returns a slice to the inline payload bytes.
    pub fn payload(&self) -> &[u8] {
        let len = (self.payload_len as usize).min(36);
        &self.inline_payload[..len]
    }

    /// Attaches an inline payload slice (convenience alias for with_inline_payload).
    pub fn with_payload(self, data: &[u8]) -> Self {
        self.with_inline_payload(data)
    }
}

/// Exactly 64-byte AppMCP Response structure.
///
/// Layout:
/// - magic (4 bytes)
/// - status (4 bytes)
/// - tool_id (4 bytes)
/// - token (4 bytes)
/// - pipe_id (4 bytes)
/// - output_len (4 bytes)
/// - inline_output (40 bytes)
/// Total = 64 bytes.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AppMcpResponse {
    pub magic: u32,
    pub status: u32,
    pub tool_id: u32,
    pub token: u32,
    pub pipe_id: u32,
    pub output_len: u32,
    pub inline_output: [u8; 40],
}

const _: () = assert!(core::mem::size_of::<AppMcpResponse>() == 64);

impl AppMcpResponse {
    /// Creates a success response.
    pub const fn success(tool_id: u32, token: u32) -> Self {
        Self {
            magic: APPMCP_MAGIC_RESPONSE,
            status: APPMCP_STATUS_SUCCESS,
            tool_id,
            token,
            pipe_id: u32::MAX,
            output_len: 0,
            inline_output: [0u8; 40],
        }
    }

    /// Creates an error response.
    pub const fn error(status: u32, tool_id: u32, token: u32) -> Self {
        Self {
            magic: APPMCP_MAGIC_RESPONSE,
            status,
            tool_id,
            token,
            pipe_id: u32::MAX,
            output_len: 0,
            inline_output: [0u8; 40],
        }
    }

    /// Attaches an inline output slice up to 40 bytes.
    pub fn with_inline_output(mut self, data: &[u8]) -> Self {
        let copy_len = if data.len() < 40 { data.len() } else { 40 };
        let mut idx = 0;
        while idx < copy_len {
            self.inline_output[idx] = data[idx];
            idx += 1;
        }
        self.output_len = copy_len as u32;
        self
    }

    /// Attaches an inline output slice (convenience alias for with_inline_output).
    pub fn with_payload(self, data: &[u8]) -> Self {
        self.with_inline_output(data)
    }

    /// Returns a slice to the inline output bytes.
    pub fn output(&self) -> &[u8] {
        let len = (self.output_len as usize).min(40);
        &self.inline_output[..len]
    }

    /// Convenience alias returning the inline payload slice.
    pub fn payload(&self) -> &[u8] {
        self.output()
    }

    /// Attaches a shared memory pipe for outputs exceeding 40 bytes.
    pub const fn with_pipe(mut self, pipe_id: u32, total_bytes: u32) -> Self {
        self.pipe_id = pipe_id;
        self.output_len = total_bytes;
        self
    }

    /// Serializes into a 64-byte array for microkernel IPC transfer.
    pub fn to_bytes(&self) -> [u8; 64] {
        let mut buf = [0u8; 64];
        buf[0..4].copy_from_slice(&self.magic.to_le_bytes());
        buf[4..8].copy_from_slice(&self.status.to_le_bytes());
        buf[8..12].copy_from_slice(&self.tool_id.to_le_bytes());
        buf[12..16].copy_from_slice(&self.token.to_le_bytes());
        buf[16..20].copy_from_slice(&self.pipe_id.to_le_bytes());
        buf[20..24].copy_from_slice(&self.output_len.to_le_bytes());
        buf[24..64].copy_from_slice(&self.inline_output);
        buf
    }

    /// Deserializes from a 64-byte buffer.
    pub fn from_bytes(raw: &[u8]) -> Option<Self> {
        if raw.len() < 64 {
            return None;
        }
        let magic = u32::from_le_bytes([raw[0], raw[1], raw[2], raw[3]]);
        if magic != APPMCP_MAGIC_RESPONSE {
            return None;
        }
        let status = u32::from_le_bytes([raw[4], raw[5], raw[6], raw[7]]);
        let tool_id = u32::from_le_bytes([raw[8], raw[9], raw[10], raw[11]]);
        let token = u32::from_le_bytes([raw[12], raw[13], raw[14], raw[15]]);
        let pipe_id = u32::from_le_bytes([raw[16], raw[17], raw[18], raw[19]]);
        let output_len = u32::from_le_bytes([raw[20], raw[21], raw[22], raw[23]]);
        if pipe_id == u32::MAX && output_len > 40 {
            return None;
        }
        let mut inline_output = [0u8; 40];
        inline_output.copy_from_slice(&raw[24..64]);

        Some(Self {
            magic,
            status,
            tool_id,
            token,
            pipe_id,
            output_len,
            inline_output,
        })
    }
}

/// Static descriptor for application tool discovery.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct AppToolDescriptor {
    pub tool_id: u32,
    pub name: [u8; 24],
    pub description: [u8; 48],
    pub required_cap_type: u64,
}

impl AppToolDescriptor {
    pub const fn new(tool_id: u32, name_bytes: &[u8], desc_bytes: &[u8], cap_type: u64) -> Self {
        let mut name = [0u8; 24];
        let mut desc = [0u8; 48];
        let mut i = 0;
        while i < name_bytes.len() && i < 24 {
            name[i] = name_bytes[i];
            i += 1;
        }
        let mut j = 0;
        while j < desc_bytes.len() && j < 48 {
            desc[j] = desc_bytes[j];
            j += 1;
        }
        Self {
            tool_id,
            name,
            description: desc,
            required_cap_type: cap_type,
        }
    }
}

/// Classification of tool risk for authorization and user consent policies.
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolRiskClass {
    /// Read-only observation with no side effects (e.g. list calendar, read battery, search files).
    Observe = 0,
    /// Reversible device or app state mutation (e.g. play/pause music, set brightness).
    Reversible = 1,
    /// Personal data mutation (e.g. create note, edit calendar event).
    PersonalMutation = 2,
    /// External communication or financial consequence (e.g. send message, make phone call).
    ExternalConsequence = 3,
    /// Security, credential, or vault sensitive operation (e.g. grant permission, change security policy).
    SecuritySensitive = 4,
}

impl ToolRiskClass {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Observe => "Observe",
            Self::Reversible => "Reversible",
            Self::PersonalMutation => "PersonalMutation",
            Self::ExternalConsequence => "ExternalConsequence",
            Self::SecuritySensitive => "SecuritySensitive",
        }
    }
}

/// User consent requirement and persistent grant policy for AppMCP tools.
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolConsentClass {
    /// Always prompt the user before each invocation.
    AlwaysAsk = 0,
    /// Allowed for a single immediate invocation.
    AllowOnce = 1,
    /// Allowed for the duration of the current interactive session.
    AllowSession = 2,
    /// Persistently allowed across sessions until explicitly revoked.
    AlwaysAllow = 3,
    /// Strictly blocked from execution.
    NeverAllow = 4,
}

impl ToolConsentClass {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::AlwaysAsk => "AlwaysAsk",
            Self::AllowOnce => "AllowOnce",
            Self::AllowSession => "AllowSession",
            Self::AlwaysAllow => "AlwaysAllow",
            Self::NeverAllow => "NeverAllow",
        }
    }
}

/// Deterministically computes a 32-bit tool ID from a package identity and tool name.
pub const fn compute_dynamic_tool_id(package_id: &[u8], tool_name: &[u8]) -> u32 {
    let mut hash = 0x811c9dc5u32;
    let prime = 0x01000193u32;
    let mut i = 0;
    while i < package_id.len() {
        hash ^= package_id[i] as u32;
        hash = hash.wrapping_mul(prime);
        i += 1;
    }
    hash ^= b'/' as u32;
    hash = hash.wrapping_mul(prime);
    let mut j = 0;
    while j < tool_name.len() {
        hash ^= tool_name[j] as u32;
        hash = hash.wrapping_mul(prime);
        j += 1;
    }
    hash
}

/// Maximum dynamic tools tracked in the platform AppMCP discovery registry.
pub const MAX_DYNAMIC_TOOLS: usize = 128;

/// A bounded, extensible, dynamic AppMCP tool descriptor declared by native or third-party applications.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct DynamicToolDescriptor {
    /// Unique 32-bit tool identifier (canonical ID or hash from package+name).
    pub tool_id: u32,
    /// Stable tool name (e.g. `notes.create`, `calculator.eval`).
    pub name: [u8; 32],
    pub name_len: u8,
    /// Human-readable label for UI rendering.
    pub label: [u8; 32],
    pub label_len: u8,
    /// Concise description of the action and parameters.
    pub description: [u8; 64],
    pub description_len: u8,
    /// Originating package identity (e.g. `org.interos.notes`).
    pub package_id: [u8; 48],
    pub package_id_len: u8,
    /// Tool revision / version.
    pub version: u32,
    /// Risk class for authorization policy.
    pub risk_class: ToolRiskClass,
    /// Current consent policy.
    pub consent_class: ToolConsentClass,
    /// Required kernel capability type.
    pub required_cap_type: u64,
}

impl Default for DynamicToolDescriptor {
    fn default() -> Self {
        Self {
            tool_id: 0,
            name: [0u8; 32],
            name_len: 0,
            label: [0u8; 32],
            label_len: 0,
            description: [0u8; 64],
            description_len: 0,
            package_id: [0u8; 48],
            package_id_len: 0,
            version: 1,
            risk_class: ToolRiskClass::Observe,
            consent_class: ToolConsentClass::AlwaysAsk,
            required_cap_type: TOOL_CAP_TYPE,
        }
    }
}

impl DynamicToolDescriptor {
    pub fn new(
        tool_id: u32,
        name: &[u8],
        label: &[u8],
        description: &[u8],
        package_id: &[u8],
        version: u32,
        risk_class: ToolRiskClass,
        consent_class: ToolConsentClass,
        required_cap_type: u64,
    ) -> Self {
        let mut desc = Self {
            tool_id,
            version,
            risk_class,
            consent_class,
            required_cap_type,
            ..Self::default()
        };
        let nl = name.len().min(32);
        desc.name[..nl].copy_from_slice(&name[..nl]);
        desc.name_len = nl as u8;

        let ll = label.len().min(32);
        desc.label[..ll].copy_from_slice(&label[..ll]);
        desc.label_len = ll as u8;

        let dl = description.len().min(64);
        desc.description[..dl].copy_from_slice(&description[..dl]);
        desc.description_len = dl as u8;

        let pl = package_id.len().min(48);
        desc.package_id[..pl].copy_from_slice(&package_id[..pl]);
        desc.package_id_len = pl as u8;

        desc
    }

    pub fn name_str(&self) -> &str {
        core::str::from_utf8(&self.name[..self.name_len as usize]).unwrap_or("")
    }

    pub fn label_str(&self) -> &str {
        core::str::from_utf8(&self.label[..self.label_len as usize]).unwrap_or("")
    }

    pub fn description_str(&self) -> &str {
        core::str::from_utf8(&self.description[..self.description_len as usize]).unwrap_or("")
    }

    pub fn package_id_str(&self) -> &str {
        core::str::from_utf8(&self.package_id[..self.package_id_len as usize]).unwrap_or("")
    }
}

pub const MAX_INVOCATION_GRANTS: usize = 16;

/// Specific user consent grant bound to an exact tool ID and arguments digest.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InvocationGrant {
    pub tool_id: u32,
    pub args_crc: u32,
    pub valid: bool,
}

impl Default for InvocationGrant {
    fn default() -> Self {
        Self {
            tool_id: 0,
            args_crc: 0,
            valid: false,
        }
    }
}

/// Dynamic registry of discovered application tools.
#[derive(Debug, Clone, Copy)]
pub struct DynamicToolRegistry {
    pub tools: [Option<DynamicToolDescriptor>; MAX_DYNAMIC_TOOLS],
    pub count: usize,
    pub invocation_grants: [InvocationGrant; MAX_INVOCATION_GRANTS],
    pub grant_count: usize,
}

impl Default for DynamicToolRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl DynamicToolRegistry {
    pub const fn new() -> Self {
        Self {
            tools: [None; MAX_DYNAMIC_TOOLS],
            count: 0,
            invocation_grants: [InvocationGrant { tool_id: 0, args_crc: 0, valid: false }; MAX_INVOCATION_GRANTS],
            grant_count: 0,
        }
    }

    /// Pre-populates the registry with all canonical platform tools.
    pub fn canonical() -> Self {
        let mut reg = Self::new();
        reg.populate_canonical();
        reg
    }

    pub fn populate_canonical(&mut self) {
        let list: &[(u32, &[u8], &[u8], &[u8], &[u8], ToolRiskClass, ToolConsentClass)] = &[
            (TOOL_ID_NOTES_CREATE, b"notes.create", b"Create Note", b"Append text to user notes", b"org.interos.notes", ToolRiskClass::PersonalMutation, ToolConsentClass::AlwaysAsk),
            (TOOL_ID_NOTES_LIST, b"notes.list", b"List Notes", b"List existing note summaries", b"org.interos.notes", ToolRiskClass::Observe, ToolConsentClass::AlwaysAllow),
            (TOOL_ID_NOTES_READ, b"notes.read", b"Read Note", b"Read full text of a note by id", b"org.interos.notes", ToolRiskClass::Observe, ToolConsentClass::AlwaysAllow),
            (TOOL_ID_NOTES_DELETE, b"notes.delete", b"Delete Note", b"Remove a note permanently", b"org.interos.notes", ToolRiskClass::PersonalMutation, ToolConsentClass::AlwaysAsk),

            (TOOL_ID_MESSAGES_SEND_SMS, b"messages.send", b"Send SMS", b"Send text message to contact", b"org.interos.messages", ToolRiskClass::ExternalConsequence, ToolConsentClass::AlwaysAsk),
            (TOOL_ID_MESSAGES_LIST, b"messages.list", b"List Messages", b"List conversation threads", b"org.interos.messages", ToolRiskClass::Observe, ToolConsentClass::AlwaysAllow),
            (TOOL_ID_MESSAGES_READ, b"messages.read", b"Read Thread", b"Read messages in thread", b"org.interos.messages", ToolRiskClass::Observe, ToolConsentClass::AlwaysAllow),

            (TOOL_ID_CALENDAR_SCHEDULE, b"calendar.schedule", b"Schedule Event", b"Create calendar appointment", b"org.interos.calendar", ToolRiskClass::PersonalMutation, ToolConsentClass::AlwaysAsk),
            (TOOL_ID_CALENDAR_LIST, b"calendar.list", b"List Events", b"List scheduled calendar events", b"org.interos.calendar", ToolRiskClass::Observe, ToolConsentClass::AlwaysAllow),
            (TOOL_ID_CALENDAR_GET_DATE, b"calendar.date", b"Get Date", b"Read current system date and time", b"org.interos.calendar", ToolRiskClass::Observe, ToolConsentClass::AlwaysAllow),

            (TOOL_ID_FILES_LIST, b"files.list", b"List Files", b"List files in authorized directory", b"org.interos.file_manager", ToolRiskClass::Observe, ToolConsentClass::AlwaysAllow),
            (TOOL_ID_FILES_READ, b"files.read", b"Read File", b"Read file contents", b"org.interos.file_manager", ToolRiskClass::Observe, ToolConsentClass::AlwaysAllow),
            (TOOL_ID_FILES_WRITE, b"files.write", b"Write File", b"Write data to file", b"org.interos.file_manager", ToolRiskClass::PersonalMutation, ToolConsentClass::AlwaysAsk),
            (TOOL_ID_FILES_STAT, b"files.stat", b"File Stat", b"Query file attributes and size", b"org.interos.file_manager", ToolRiskClass::Observe, ToolConsentClass::AlwaysAllow),

            (TOOL_ID_SETTINGS_GET, b"settings.get", b"Get Setting", b"Read system configuration value", b"org.interos.settings", ToolRiskClass::Observe, ToolConsentClass::AlwaysAllow),
            (TOOL_ID_SETTINGS_SET, b"settings.set", b"Change Setting", b"Modify system setting", b"org.interos.settings", ToolRiskClass::Reversible, ToolConsentClass::AlwaysAsk),

            (TOOL_ID_CALCULATOR_EVAL, b"calculator.eval", b"Calculator", b"Evaluate arithmetic expression", b"org.interos.calculator", ToolRiskClass::Observe, ToolConsentClass::AlwaysAllow),

            (TOOL_ID_DIALER_CALL, b"dialer.call", b"Place Call", b"Initiate telephony voice call", b"org.interos.dialer", ToolRiskClass::ExternalConsequence, ToolConsentClass::AlwaysAsk),
            (TOOL_ID_DIALER_LOGS, b"dialer.logs", b"Call Logs", b"Read recent call history", b"org.interos.dialer", ToolRiskClass::Observe, ToolConsentClass::AlwaysAllow),

            (TOOL_ID_MUSIC_PLAY, b"music.play", b"Play Music", b"Start audio playback", b"org.interos.music", ToolRiskClass::Reversible, ToolConsentClass::AlwaysAllow),
            (TOOL_ID_MUSIC_PAUSE, b"music.pause", b"Pause Music", b"Pause active playback", b"org.interos.music", ToolRiskClass::Reversible, ToolConsentClass::AlwaysAllow),
            (TOOL_ID_MUSIC_STATUS, b"music.status", b"Playback Status", b"Query track and playing state", b"org.interos.music", ToolRiskClass::Observe, ToolConsentClass::AlwaysAllow),
        ];

        for &(id, name, label, desc, pkg, risk, consent) in list {
            let desc = DynamicToolDescriptor::new(
                id,
                name,
                label,
                desc,
                pkg,
                1,
                risk,
                consent,
                TOOL_CAP_TYPE,
            );
            let _ = self.register(desc);
        }
    }

    /// Registers a tool into the dynamic registry.
    ///
    /// SECURITY INVARIANTS:
    /// 1. Collision-Safe Identity: If another tool already has `descriptor.tool_id`
    ///    with a DIFFERENT package or name, registration is REJECTED (`Err(())`).
    ///    One application can NEVER overwrite or impersonate another application's tool.
    /// 2. If the tool is re-registered by the SAME package and name, its descriptor is updated.
    /// 3. High-Risk Policy: `ExternalConsequence` and `SecuritySensitive` actions
    ///    can never be permanently autonomous. If `AlwaysAllow` is requested, it is
    ///    clamped by platform security policy to `AlwaysAsk`.
    pub fn register(&mut self, mut descriptor: DynamicToolDescriptor) -> Result<u32, ()> {
        // Enforce platform risk policy: external consequences or security-sensitive tools
        // cannot claim autonomous AlwaysAllow
        if matches!(
            descriptor.risk_class,
            ToolRiskClass::ExternalConsequence | ToolRiskClass::SecuritySensitive
        ) && descriptor.consent_class == ToolConsentClass::AlwaysAllow {
            descriptor.consent_class = ToolConsentClass::AlwaysAsk;
        }

        // Check for existing tool by package_id and name (update scenario)
        for slot in &mut self.tools {
            if let Some(existing) = slot {
                if &existing.package_id[..existing.package_id_len as usize] == &descriptor.package_id[..descriptor.package_id_len as usize]
                    && &existing.name[..existing.name_len as usize] == &descriptor.name[..descriptor.name_len as usize]
                {
                    *existing = descriptor;
                    return Ok(descriptor.tool_id);
                }
            }
        }

        // Check for collision: tool_id matches an existing tool belonging to ANOTHER package/name
        for slot in &self.tools {
            if let Some(existing) = slot {
                if existing.tool_id == descriptor.tool_id {
                    // Collision detected! Fails closed: reject impersonation attempt
                    return Err(());
                }
            }
        }

        // Insert into first available slot
        for slot in &mut self.tools {
            if slot.is_none() {
                *slot = Some(descriptor);
                self.count += 1;
                return Ok(descriptor.tool_id);
            }
        }
        Err(())
    }

    /// Unregisters a single tool by package ID and tool name.
    pub fn unregister_tool(&mut self, package_id: &[u8], name: &[u8]) -> bool {
        for slot in &mut self.tools {
            if let Some(desc) = slot {
                if &desc.package_id[..desc.package_id_len as usize] == package_id
                    && &desc.name[..desc.name_len as usize] == name
                {
                    *slot = None;
                    self.count = self.count.saturating_sub(1);
                    return true;
                }
            }
        }
        false
    }

    /// Unregisters all tools declared by a package (e.g. upon app uninstall).
    pub fn unregister_package(&mut self, package_id: &[u8]) -> usize {
        self.invalidate_package(package_id)
    }

    /// Consumes single-use consent (reverting `AllowOnce` back to `AlwaysAsk`).
    pub fn consume_consent(&mut self, tool_id: u32) -> bool {
        for slot in &mut self.tools {
            if let Some(desc) = slot {
                if desc.tool_id == tool_id {
                    if desc.consent_class == ToolConsentClass::AllowOnce {
                        desc.consent_class = ToolConsentClass::AlwaysAsk;
                        return true;
                    }
                    return false;
                }
            }
        }
        false
    }

    /// Expires all session-bound consent grants (`AllowSession` -> `AlwaysAsk`).
    /// Invoked upon device lock, logout, or session termination.
    pub fn expire_session_grants(&mut self) {
        for slot in &mut self.tools {
            if let Some(desc) = slot {
                if desc.consent_class == ToolConsentClass::AllowSession {
                    desc.consent_class = ToolConsentClass::AlwaysAsk;
                }
            }
        }
        self.clear_invocation_grants();
    }

    /// Grants user consent for a specific tool invocation bound to the arguments digest.
    pub fn grant_invocation_consent(&mut self, tool_id: u32, args_crc: u32) -> bool {
        for g in &mut self.invocation_grants {
            if g.valid && g.tool_id == tool_id && g.args_crc == args_crc {
                return true;
            }
        }
        for g in &mut self.invocation_grants {
            if !g.valid {
                *g = InvocationGrant { tool_id, args_crc, valid: true };
                self.grant_count = self.grant_count.saturating_add(1);
                return true;
            }
        }
        false
    }

    /// Verifies and consumes user consent for a specific tool invocation.
    pub fn consume_invocation_consent(&mut self, tool_id: u32, args_crc: u32) -> bool {
        for g in &mut self.invocation_grants {
            if g.valid && g.tool_id == tool_id && g.args_crc == args_crc {
                g.valid = false;
                self.grant_count = self.grant_count.saturating_sub(1);
                return true;
            }
        }
        false
    }

    /// Checks whether user consent exists for a specific tool invocation.
    pub fn has_invocation_consent(&self, tool_id: u32, args_crc: u32) -> bool {
        for g in &self.invocation_grants {
            if g.valid && g.tool_id == tool_id && g.args_crc == args_crc {
                return true;
            }
        }
        false
    }

    /// Clears all active invocation consent grants.
    pub fn clear_invocation_grants(&mut self) {
        for g in &mut self.invocation_grants {
            g.valid = false;
        }
        self.grant_count = 0;
    }

    /// Finds a tool descriptor by its 32-bit tool_id.
    pub fn find_by_id(&self, tool_id: u32) -> Option<&DynamicToolDescriptor> {
        for slot in &self.tools {
            if let Some(desc) = slot {
                if desc.tool_id == tool_id {
                    return Some(desc);
                }
            }
        }
        None
    }

    /// Finds a tool descriptor by package_id and tool name.
    pub fn find_by_name(&self, package_id: &[u8], name: &[u8]) -> Option<&DynamicToolDescriptor> {
        for slot in &self.tools {
            if let Some(desc) = slot {
                if &desc.package_id[..desc.package_id_len as usize] == package_id
                    && &desc.name[..desc.name_len as usize] == name
                {
                    return Some(desc);
                }
            }
        }
        None
    }

    /// Updates the user consent class for a specific tool.
    pub fn set_consent(&mut self, tool_id: u32, consent: ToolConsentClass) -> bool {
        for slot in &mut self.tools {
            if let Some(desc) = slot {
                if desc.tool_id == tool_id {
                    desc.consent_class = consent;
                    return true;
                }
            }
        }
        false
    }

    /// Invalidates all registered tools belonging to a package (e.g., when updated or uninstalled).
    /// This prevents stale grants from authorizing changed semantics under the platform security contract.
    pub fn invalidate_package(&mut self, package_id: &[u8]) -> usize {
        let mut removed = 0;
        for slot in &mut self.tools {
            if let Some(desc) = slot {
                if &desc.package_id[..desc.package_id_len as usize] == package_id {
                    *slot = None;
                    self.count = self.count.saturating_sub(1);
                    removed += 1;
                }
            }
        }
        removed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_appmcp_request_exact_size_and_roundtrip() {
        assert_eq!(core::mem::size_of::<AppMcpRequest>(), 64);
        let req = AppMcpRequest::new(0x1234_5678_9ABC_DEF0, TOOL_ID_NOTES_CREATE, 42)
            .with_inline_payload(b"Buy milk and fresh fruit");
        
        assert_eq!(req.payload_len, 24);
        let bytes = req.to_bytes();
        assert_eq!(bytes.len(), 64);

        let parsed = AppMcpRequest::from_bytes(&bytes).expect("failed to parse valid request");
        assert_eq!(parsed.magic, APPMCP_MAGIC_REQUEST);
        assert_eq!(parsed.cap_handle, 0x1234_5678_9ABC_DEF0);
        assert_eq!(parsed.tool_id, TOOL_ID_NOTES_CREATE);
        assert_eq!(parsed.token, 42);
        assert_eq!(parsed.payload_len, 24);
        assert_eq!(&parsed.inline_payload[..24], b"Buy milk and fresh fruit");
    }

    #[test]
    fn test_appmcp_response_exact_size_and_roundtrip() {
        assert_eq!(core::mem::size_of::<AppMcpResponse>(), 64);
        let resp = AppMcpResponse::success(TOOL_ID_NOTES_CREATE, 42)
            .with_inline_output(&1001u32.to_le_bytes());

        assert_eq!(resp.output_len, 4);
        let bytes = resp.to_bytes();
        assert_eq!(bytes.len(), 64);

        let parsed = AppMcpResponse::from_bytes(&bytes).expect("failed to parse valid response");
        assert_eq!(parsed.magic, APPMCP_MAGIC_RESPONSE);
        assert_eq!(parsed.status, APPMCP_STATUS_SUCCESS);
        assert_eq!(parsed.tool_id, TOOL_ID_NOTES_CREATE);
        assert_eq!(parsed.token, 42);
        assert_eq!(parsed.output_len, 4);
        assert_eq!(&parsed.inline_output[..4], &1001u32.to_le_bytes());
    }

    #[test]
    fn test_appmcp_error_response() {
        let resp = AppMcpResponse::error(APPMCP_STATUS_PERMISSION_DENIED, TOOL_ID_MESSAGES_SEND_SMS, 77);
        let bytes = resp.to_bytes();
        let parsed = AppMcpResponse::from_bytes(&bytes).unwrap();
        assert_eq!(parsed.status, APPMCP_STATUS_PERMISSION_DENIED);
        assert_eq!(parsed.tool_id, TOOL_ID_MESSAGES_SEND_SMS);
        assert_eq!(parsed.token, 77);
    }

    #[test]
    fn test_appmcp_convenience_payload_helpers() {
        let req = AppMcpRequest::new(100, TOOL_ID_FILES_LIST, 99)
            .with_payload(b"/storage/photos");
        assert_eq!(req.payload(), b"/storage/photos");
        assert_eq!(req.pipe_id, u32::MAX);

        let resp = AppMcpResponse::success(TOOL_ID_FILES_LIST, 99)
            .with_payload(b"entries:5");
        assert_eq!(resp.output(), b"entries:5");

        let piped_req = AppMcpRequest::new(100, TOOL_ID_FILES_READ, 101)
            .with_pipe(7, 4096);
        assert_eq!(piped_req.pipe_id, 7);
        assert_eq!(piped_req.payload_len, 4096);

        let piped_resp = AppMcpResponse::success(TOOL_ID_FILES_READ, 101)
            .with_pipe(7, 4096);
        assert_eq!(piped_resp.pipe_id, 7);
        assert_eq!(piped_resp.output_len, 4096);
    }

    /// The capability the receivers ask the kernel about, pinned to the kernel's
    /// own encoding.
    ///
    /// WHY the other two spaces are named here: this repository has three
    /// disjoint numeric spaces that all describe "may run agent tools", and
    /// confusing them is a silent wrong-authority bug rather than a compile
    /// error. In the kernel's capability space 15 is `ShmProducer`, so passing
    /// the `SensitivePermission` number would verify shared-memory authority
    /// and call it tool authority.
    #[test]
    fn tool_authority_names_the_kernel_agent_task_capability() {
        assert_eq!(TOOL_CAP_TYPE, 33);
        assert_eq!(TOOL_CAP_TYPE, crate::cap_type::AGENT_TASK);

        assert_eq!(crate::SensitivePermission::AgentTask as u64, 15);
        assert_ne!(TOOL_CAP_TYPE, crate::SensitivePermission::AgentTask as u64);
        assert_ne!(TOOL_CAP_TYPE, crate::cap_type::SHM_PRODUCER);
        assert_eq!(crate::cap_type::SHM_PRODUCER, 15);

        // Rights::INVOKE in the kernel's encoding: the right to exercise the
        // capability, not merely to read or write the object it names.
        assert_eq!(TOOL_CAP_RIGHTS, 1 << 5);

        // The object the consent broker and the app spawn path mint for the
        // bounded tool domain (five tools, bits 0..4).
        assert_eq!(TOOL_CAP_OBJECT, 0x1F);
        assert_eq!(TOOL_CAP_OBJECT, 0b1_1111);
    }

    /// The premise of the defect this module's gate closes: the wire layer
    /// carries whatever integer the caller wrote and grants nothing by it.
    ///
    /// A forged non-zero handle parses exactly as readily as a real one, so
    /// `from_bytes` returning `Some` is not authorization and must never be
    /// treated as one. Only `authorize_tool_sender` answers that question, and
    /// it answers it by asking the kernel about the sender's own table.
    #[test]
    fn parsing_a_request_grants_no_authority() {
        for forged in [0u64, 1, 0xDEAD_BEEF, 0x1234_5678_9ABC_DEF0, u64::MAX] {
            let req = AppMcpRequest::new(forged, TOOL_ID_NOTES_CREATE, 7)
                .with_payload(b"anything");
            let parsed = AppMcpRequest::from_bytes(&req.to_bytes())
                .expect("the wire layer parses a claim without judging it");
            assert_eq!(parsed.cap_handle, forged);
            assert_eq!(parsed.tool_id, TOOL_ID_NOTES_CREATE);
        }
    }

    #[test]
    fn test_dynamic_tool_registry_canonical_and_custom() {
        let mut reg = DynamicToolRegistry::canonical();
        assert!(reg.count >= 20);

        // Canonical lookup by ID
        let notes_create = reg.find_by_id(TOOL_ID_NOTES_CREATE).expect("notes.create registered");
        assert_eq!(notes_create.name_str(), "notes.create");
        assert_eq!(notes_create.package_id_str(), "org.interos.notes");
        assert_eq!(notes_create.risk_class, ToolRiskClass::PersonalMutation);
        assert_eq!(notes_create.consent_class, ToolConsentClass::AlwaysAsk);

        // Canonical lookup by name
        let calc = reg.find_by_name(b"org.interos.calculator", b"calculator.eval").expect("calculator.eval");
        assert_eq!(calc.tool_id, TOOL_ID_CALCULATOR_EVAL);
        assert_eq!(calc.risk_class, ToolRiskClass::Observe);
        assert_eq!(calc.consent_class, ToolConsentClass::AlwaysAllow);

        // Third-party tool dynamic registration without SDK enum changes
        let third_party_pkg = b"com.example.budget";
        let third_party_tool = b"budget.record_expense";
        let custom_id = compute_dynamic_tool_id(third_party_pkg, third_party_tool);
        let custom_desc = DynamicToolDescriptor::new(
            custom_id,
            third_party_tool,
            b"Record Expense",
            b"Add an expense to the user's budget log",
            third_party_pkg,
            1,
            ToolRiskClass::ExternalConsequence,
            ToolConsentClass::AlwaysAsk,
            TOOL_CAP_TYPE,
        );

        let initial_count = reg.count;
        let registered_id = reg.register(custom_desc).expect("register custom tool");
        assert_eq!(registered_id, custom_id);
        assert_eq!(reg.count, initial_count + 1);

        // Discovered and queried dynamically
        let retrieved = reg.find_by_id(custom_id).expect("retrieved custom tool");
        assert_eq!(retrieved.name_str(), "budget.record_expense");
        assert_eq!(retrieved.package_id_str(), "com.example.budget");
        assert_eq!(retrieved.risk_class, ToolRiskClass::ExternalConsequence);

        // Consent change
        assert!(reg.set_consent(custom_id, ToolConsentClass::AllowSession));
        assert_eq!(reg.find_by_id(custom_id).unwrap().consent_class, ToolConsentClass::AllowSession);

        // Invalidation on package update / uninstall
        let removed = reg.invalidate_package(third_party_pkg);
        assert_eq!(removed, 1);
        assert_eq!(reg.count, initial_count);
        assert!(reg.find_by_id(custom_id).is_none());
    }

    #[test]
    fn test_hash_collision_prevention() {
        let mut reg = DynamicToolRegistry::new();
        let tool_a = DynamicToolDescriptor::new(
            0x1234_5678,
            b"tool.alpha",
            b"Alpha",
            b"Alpha tool",
            b"org.interos.pkg_a",
            1,
            ToolRiskClass::Observe,
            ToolConsentClass::AlwaysAllow,
            TOOL_CAP_TYPE,
        );
        assert!(reg.register(tool_a).is_ok());

        // Attempt to register a different package/tool with colliding tool_id
        let colliding_tool = DynamicToolDescriptor::new(
            0x1234_5678, // Same tool_id
            b"tool.beta", // Different name
            b"Beta",
            b"Impersonator tool",
            b"com.adversary.pkg_b", // Different package
            1,
            ToolRiskClass::Observe,
            ToolConsentClass::AlwaysAllow,
            TOOL_CAP_TYPE,
        );

        // Registry must refuse collision: cannot overwrite pkg_a's descriptor!
        assert!(reg.register(colliding_tool).is_err(), "colliding tool_id from different package must be rejected");

        // Verify pkg_a's descriptor remains intact
        let existing = reg.find_by_id(0x1234_5678).expect("tool exists");
        assert_eq!(existing.package_id_str(), "org.interos.pkg_a");
        assert_eq!(existing.name_str(), "tool.alpha");
    }

    #[test]
    fn test_high_risk_actions_prevent_autonomous_always_allow() {
        let mut reg = DynamicToolRegistry::new();
        let high_risk_desc = DynamicToolDescriptor::new(
            0x9999_0001,
            b"telephony.call_emergency",
            b"Call",
            b"External consequence action",
            b"org.interos.dialer",
            1,
            ToolRiskClass::ExternalConsequence,
            ToolConsentClass::AlwaysAllow, // Asks for AlwaysAllow
            TOOL_CAP_TYPE,
        );

        assert!(reg.register(high_risk_desc).is_ok());
        let registered = reg.find_by_id(0x9999_0001).expect("registered");
        // Platform policy outranks app: clamped to AlwaysAsk!
        assert_eq!(registered.consent_class, ToolConsentClass::AlwaysAsk);
    }

    #[test]
    fn test_allow_once_and_session_consent_lifecycle() {
        let mut reg = DynamicToolRegistry::new();
        let tool = DynamicToolDescriptor::new(
            0x5555_0001,
            b"notes.delete",
            b"Delete",
            b"Delete note",
            b"org.interos.notes",
            1,
            ToolRiskClass::PersonalMutation,
            ToolConsentClass::AllowOnce,
            TOOL_CAP_TYPE,
        );
        reg.register(tool).expect("register");

        assert_eq!(reg.find_by_id(0x5555_0001).unwrap().consent_class, ToolConsentClass::AllowOnce);

        // Consuming consent reverts AllowOnce to AlwaysAsk
        assert!(reg.consume_consent(0x5555_0001));
        assert_eq!(reg.find_by_id(0x5555_0001).unwrap().consent_class, ToolConsentClass::AlwaysAsk);
        // Second consumption fails because it is already AlwaysAsk
        assert!(!reg.consume_consent(0x5555_0001));

        // Test AllowSession expiration
        reg.set_consent(0x5555_0001, ToolConsentClass::AllowSession);
        assert_eq!(reg.find_by_id(0x5555_0001).unwrap().consent_class, ToolConsentClass::AllowSession);
        reg.expire_session_grants();
        assert_eq!(reg.find_by_id(0x5555_0001).unwrap().consent_class, ToolConsentClass::AlwaysAsk);
    }

    #[test]
    fn test_unregister_single_tool() {
        let mut reg = DynamicToolRegistry::new();
        let tool = DynamicToolDescriptor::new(
            0x7777_0001,
            b"music.pause",
            b"Pause",
            b"Pause playback",
            b"org.interos.music",
            1,
            ToolRiskClass::Reversible,
            ToolConsentClass::AlwaysAllow,
            TOOL_CAP_TYPE,
        );
        reg.register(tool).expect("register");
        assert_eq!(reg.count, 1);

        assert!(reg.unregister_tool(b"org.interos.music", b"music.pause"));
        assert_eq!(reg.count, 0);
        assert!(reg.find_by_id(0x7777_0001).is_none());
    }

    #[test]
    fn test_invocation_consent_exact_binding() {
        let mut reg = DynamicToolRegistry::new();
        let tool_id = 0x8888_0001;
        let args_crc_a = 0x1234_5678;
        let args_crc_b = 0x8765_4321;

        assert!(!reg.has_invocation_consent(tool_id, args_crc_a));

        // Grant consent for args_crc_a
        reg.grant_invocation_consent(tool_id, args_crc_a);
        assert!(reg.has_invocation_consent(tool_id, args_crc_a));
        assert!(!reg.has_invocation_consent(tool_id, args_crc_b));

        // Consume for wrong args fails
        assert!(!reg.consume_invocation_consent(tool_id, args_crc_b));
        assert!(reg.has_invocation_consent(tool_id, args_crc_a));

        // Consume for correct args succeeds and removes grant
        assert!(reg.consume_invocation_consent(tool_id, args_crc_a));
        assert!(!reg.has_invocation_consent(tool_id, args_crc_a));
        assert!(!reg.consume_invocation_consent(tool_id, args_crc_a));

        // Clear invocation grants
        reg.grant_invocation_consent(tool_id, args_crc_b);
        assert!(reg.has_invocation_consent(tool_id, args_crc_b));
        reg.clear_invocation_grants();
        assert!(!reg.has_invocation_consent(tool_id, args_crc_b));
    }
}
