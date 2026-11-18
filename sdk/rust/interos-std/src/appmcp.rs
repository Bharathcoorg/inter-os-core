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
}
