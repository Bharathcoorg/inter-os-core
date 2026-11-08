# Sovereign AppMCP Specification (Model Context Protocol for InterOS Apps)

The **Sovereign AppMCP (Application Model Context Protocol)** is a `#![no_std]`, zero-heap-allocation IPC protocol designed for InterOS Core. It enables local on-device AI inference engines (`aid`, PID 13) and the system Agent Manager (`agent_manager`, PID 15) to discover, inspect, and invoke tool actions across native applications in both headless and interactive graphical modes.

---

## 1. Wire Protocol Specification (`IPC_MAX_PAYLOAD = 64`)

All AppMCP messages conform strictly to the 64-byte microkernel mailbox capacity defined in `kernel/src/ipc/mod.rs`:

```rust
pub const APPMCP_MAGIC_REQUEST: u32  = 0x4D43_5051; // b"MCPQ"
pub const APPMCP_MAGIC_RESPONSE: u32 = 0x4D43_5052; // b"MCPR"

pub const APPMCP_MSG_INVOKE: u32     = 0x4D43_5049; // b"MCPI"
pub const APPMCP_MSG_RESPONSE: u32   = 0x4D43_5052; // b"MCPR"
pub const APPMCP_MSG_DISCOVER: u32   = 0x4D43_5044; // b"MCPD"
pub const APPMCP_MSG_MANIFEST: u32   = 0x4D43_504D; // b"MCPM"
```

### 1.1 `AppMcpRequest` (64 bytes, 8-byte natural alignment)

```rust
#[repr(C)]
pub struct AppMcpRequest {
    pub cap_handle: u64,           // Bytes 0..8: Generation-counted capability handle
    pub magic: u32,                // Bytes 8..12: 0x4D43_5051
    pub tool_id: u32,              // Bytes 12..16: 32-bit unique Tool ID
    pub token: u32,                // Bytes 16..20: Correlation sequence token
    pub pipe_id: u32,              // Bytes 20..24: SHM pipe ID (u32::MAX if inline)
    pub payload_len: u32,          // Bytes 24..28: Payload byte count
    pub inline_payload: [u8; 36],  // Bytes 28..64: Inline parameters
}
const _: () = assert!(core::mem::size_of::<AppMcpRequest>() == 64);
```

### 1.2 `AppMcpResponse` (64 bytes, 4-byte natural alignment)

```rust
#[repr(C)]
pub struct AppMcpResponse {
    pub magic: u32,               // Bytes 0..4: 0x4D43_5052
    pub status: u32,              // Bytes 4..8: Status code (0 = SUCCESS)
    pub tool_id: u32,             // Bytes 8..12: Tool ID that was executed
    pub token: u32,               // Bytes 12..16: Correlation sequence token
    pub pipe_id: u32,             // Bytes 16..20: SHM pipe ID (u32::MAX if inline)
    pub output_len: u32,          // Bytes 20..24: Output byte count
    pub inline_output: [u8; 40],  // Bytes 24..64: Inline result buffer
}
const _: () = assert!(core::mem::size_of::<AppMcpResponse>() == 64);
```

### 1.3 Status Codes

```rust
pub const APPMCP_STATUS_SUCCESS: u32            = 0;
pub const APPMCP_STATUS_PERMISSION_DENIED: u32  = 1;
pub const APPMCP_STATUS_INVALID_HANDLE: u32     = 2;
pub const APPMCP_STATUS_TOOL_NOT_FOUND: u32     = 3;
pub const APPMCP_STATUS_INVALID_ARGUMENTS: u32  = 4;
pub const APPMCP_STATUS_STORAGE_FULL: u32       = 5;
pub const APPMCP_STATUS_UNAVAILABLE: u32        = 6;
pub const APPMCP_STATUS_INTERNAL_ERROR: u32     = 7;
pub const APPMCP_STATUS_NOT_FOUND: u32          = 8;
```

---

## 2. Tool Action Catalog

| Application | Tool ID | Constant | Required Capability | Mutation & Output |
| :--- | :--- | :--- | :--- | :--- |
| `notes` | `0x4E54_4352` | `TOOL_ID_NOTES_CREATE` | `AgentTask` (Write) | Appends note to `NotesStore`, syncs with Fletcher-32 CRC. Returns note count. |
| `notes` | `0x4E54_4C53` | `TOOL_ID_NOTES_LIST` | `AgentTask` (Read) | Returns note count and active note summaries. |
| `notes` | `0x4E54_444C` | `TOOL_ID_NOTES_DELETE` | `AgentTask` (Write) | Deletes note by index or title. |
| `messages` | `0x4D53_534E` | `TOOL_ID_MESSAGES_SEND_SMS` | `AgentTask` (Invoke) | Appends SMS to thread store, synchronizes ledger. |
| `messages` | `0x4D53_4C53` | `TOOL_ID_MESSAGES_LIST` | `AgentTask` (Read) | Returns thread count (`threads:N`). |
| `messages` | `0x4D53_444C` | `TOOL_ID_MESSAGES_DELETE` | `AgentTask` (Write) | Removes conversation thread by recipient. |
| `calendar` | `0x4341_5343` | `TOOL_ID_CALENDAR_SCHEDULE` | `AgentTask` (Write) | Schedules civil calendar event with RTC sync. |
| `calendar` | `0x4341_4454` | `TOOL_ID_CALENDAR_GET_DATE` | `AgentTask` (Read) | Returns current civil date and active events count. |
| `calendar` | `0x4341_4C53` | `TOOL_ID_CALENDAR_LIST` | `AgentTask` (Read) | Lists events for selected day. |
| `settings` | `0x5354_4754` | `TOOL_ID_SETTINGS_GET` | `AgentTask` (Read) | Reads system setting key (`search.dimension`, etc.). |
| `settings` | `0x5354_5354` | `TOOL_ID_SETTINGS_SET` | `AgentTask` (Write) | Updates platform setting key/value pair. |
| `file_manager` | `0x464D_4C53` | `TOOL_ID_FILES_LIST` | `AgentTask` (Read) | Lists directory contents and returns entry count (`entries:N`). |
| `file_manager` | `0x464D_5244` | `TOOL_ID_FILES_READ` | `AgentTask` (Read) | Reads file content safely from VFS storage. |
| `file_manager` | `0x464D_5752` | `TOOL_ID_FILES_WRITE` | `AgentTask` (Write) | Writes file content safely via format `path\|data`. |

---

## 3. Dual-Mode Reactor Architecture

Native InterOS applications execute in one of two modes:
1. **Interactive Graphical Mode**: When launched by Launcher, a KMS client surface (`z=200`) is allocated. The event loop polls both `EVENT_TOUCH_INPUT` and `EVENT_IPC_MSG`. Tool requests received while running update the internal store and trigger a clean UI redraw and `surface_flip()`.
2. **Headless Agent Mode**: When launched without an allocated display surface, the application inspects the IPC mailbox. If an `APPMCP_MSG_INVOKE` message is pending, it executes the tool action, replies with `APPMCP_MSG_RESPONSE`, and exits cleanly with status 0. If no tool intent is pending, it exits with status 1.

---

## 4. Zero Ambient Authority Guarantees

- **No Root or Arbitrary Shell**: AI models and agent processes possess zero ambient authority. They cannot execute arbitrary shell commands, raw memory mutations, or unbrokered VFS accesses.
- **Explicit Broker Grants**: Tool requests with `cap_handle == 0` or carrying revoked capabilities are rejected with `APPMCP_STATUS_PERMISSION_DENIED`.
- **Model Output Untrusted**: Model proposals are treated as unvalidated input and must pass through strict bounds checking, argument delimiters (`\|`), and type parsers before state mutation.
