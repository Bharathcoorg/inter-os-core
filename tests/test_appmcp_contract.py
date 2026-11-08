# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

"""The Sovereign AppMCP Protocol & Headless Dual-Mode Truth Contract.

WHAT THIS FILE IS FOR: AppMCP is the zero-allocation model context protocol
allowing local AI models (Gemma 4 Mobile Lite, EmbeddingGemma 2, Whisper, Kokoro)
and the Agent Manager to invoke application functions autonomously.

ZERO AMBIENT AUTHORITY: Every tool request must carry an explicit CapHandle
(e.g., SensitivePermission::AgentTask = 15). A tool request lacking a capability
token is strictly rejected with APPMCP_STATUS_PERMISSION_DENIED.

BOUNDED INVARIANT: Request and Response layouts must be exactly 64 bytes (`repr(C)`),
matching the microkernel's IPC_MAX_PAYLOAD = 64 mailbox capacity.
"""

from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]

def _read_appmcp_rs() -> str:
    path = ROOT / "sdk" / "rust" / "interos-std" / "src" / "appmcp.rs"
    assert path.is_file(), f"missing {path}"
    return path.read_text(encoding="utf-8")

def _read_notes_main_rs() -> str:
    path = ROOT / "apps" / "notes" / "src" / "main.rs"
    assert path.is_file(), f"missing {path}"
    return path.read_text(encoding="utf-8")

def _read_messages_main_rs() -> str:
    path = ROOT / "apps" / "messages" / "src" / "main.rs"
    assert path.is_file(), f"missing {path}"
    return path.read_text(encoding="utf-8")

def _read_calendar_main_rs() -> str:
    path = ROOT / "apps" / "calendar" / "src" / "main.rs"
    assert path.is_file(), f"missing {path}"
    return path.read_text(encoding="utf-8")

def _read_settings_main_rs() -> str:
    path = ROOT / "apps" / "settings" / "src" / "main.rs"
    assert path.is_file(), f"missing {path}"
    return path.read_text(encoding="utf-8")

def _read_file_manager_main_rs() -> str:
    path = ROOT / "apps" / "file_manager" / "src" / "main.rs"
    assert path.is_file(), f"missing {path}"
    return path.read_text(encoding="utf-8")

def test_appmcp_struct_sizes_are_compile_time_asserted_to_64_bytes():
    src = _read_appmcp_rs()
    assert "assert!(core::mem::size_of::<AppMcpRequest>() == 64)" in src, (
        "AppMcpRequest must be compile-time asserted to exactly 64 bytes"
    )
    assert "assert!(core::mem::size_of::<AppMcpResponse>() == 64)" in src, (
        "AppMcpResponse must be compile-time asserted to exactly 64 bytes"
    )

def test_appmcp_magic_and_message_types_are_canonical():
    src = _read_appmcp_rs()
    assert "APPMCP_MAGIC_REQUEST: u32 = 0x4D43_5051" in src # b"MCPQ"
    assert "APPMCP_MAGIC_RESPONSE: u32 = 0x4D43_5052" in src # b"MCPR"
    assert "APPMCP_MSG_INVOKE: u32 = 0x4D43_5049" in src # b"MCPI"
    assert "APPMCP_MSG_RESPONSE: u32 = 0x4D43_5052" in src # b"MCPR"
    assert "APPMCP_MSG_DISCOVER: u32 = 0x4D43_5044" in src # b"MCPD"
    assert "APPMCP_MSG_MANIFEST: u32 = 0x4D43_504D" in src # b"MCPM"

def test_appmcp_status_codes_and_tool_ids_are_defined():
    src = _read_appmcp_rs()
    assert "APPMCP_STATUS_SUCCESS: u32            = 0;" in src
    assert "APPMCP_STATUS_PERMISSION_DENIED: u32  = 1;" in src
    assert "TOOL_ID_NOTES_CREATE: u32             = 0x4E54_4352;" in src # b"NTCR"
    assert "TOOL_ID_NOTES_LIST: u32               = 0x4E54_4C53;" in src # b"NTLS"
    assert "TOOL_ID_NOTES_DELETE: u32             = 0x4E54_444C;" in src # b"NTDL"
    assert "TOOL_ID_MESSAGES_SEND_SMS: u32        = 0x4D53_534E;" in src # b"MSSN"
    assert "TOOL_ID_CALENDAR_SCHEDULE: u32        = 0x4341_5343;" in src # b"CASC"
    assert "TOOL_ID_SETTINGS_GET: u32             = 0x5354_4754;" in src # b"STGT"
    assert "TOOL_ID_SETTINGS_SET: u32             = 0x5354_5354;" in src # b"STST"

def test_notes_dual_mode_reactor_handles_headless_appmcp():
    notes_src = _read_notes_main_rs()
    assert "fn handle_appmcp_request" in notes_src, "Notes app must implement handle_appmcp_request"
    assert "std::appmcp::APPMCP_STATUS_PERMISSION_DENIED" in notes_src, (
        "Notes AppMCP must enforce capability check and return PERMISSION_DENIED on zero cap_handle"
    )
    assert "std::appmcp::TOOL_ID_NOTES_CREATE" in notes_src
    assert "std::appmcp::TOOL_ID_NOTES_LIST" in notes_src
    assert "std::appmcp::TOOL_ID_NOTES_DELETE" in notes_src
    assert "Headless AppMCP tool executed successfully." in notes_src
    assert "std::EVENT_IPC_MSG" in notes_src, "Notes event poll must listen for EVENT_IPC_MSG"

def test_messages_dual_mode_reactor_handles_headless_appmcp():
    msg_src = _read_messages_main_rs()
    assert "fn handle_appmcp_request" in msg_src, "Messages app must implement handle_appmcp_request"
    assert "std::appmcp::APPMCP_STATUS_PERMISSION_DENIED" in msg_src
    assert "std::appmcp::TOOL_ID_MESSAGES_SEND_SMS" in msg_src
    assert "std::appmcp::TOOL_ID_MESSAGES_LIST" in msg_src
    assert "std::appmcp::TOOL_ID_MESSAGES_DELETE" in msg_src
    assert "Headless AppMCP tool invocation handled successfully." in msg_src
    assert "std::EVENT_IPC_MSG" in msg_src

def test_calendar_dual_mode_reactor_handles_headless_appmcp():
    cal_src = _read_calendar_main_rs()
    assert "fn handle_appmcp_request" in cal_src, "Calendar app must implement handle_appmcp_request"
    assert "std::appmcp::APPMCP_STATUS_PERMISSION_DENIED" in cal_src
    assert "std::appmcp::TOOL_ID_CALENDAR_SCHEDULE" in cal_src
    assert "std::appmcp::TOOL_ID_CALENDAR_GET_DATE" in cal_src
    assert "std::appmcp::TOOL_ID_CALENDAR_LIST" in cal_src
    assert "Headless AppMCP tool invocation handled successfully." in cal_src
    assert "std::EVENT_IPC_MSG" in cal_src

def test_settings_dual_mode_reactor_handles_headless_appmcp():
    set_src = _read_settings_main_rs()
    assert "fn handle_appmcp_request" in set_src, "Settings app must implement handle_appmcp_request"
    assert "std::appmcp::APPMCP_STATUS_PERMISSION_DENIED" in set_src
    assert "std::appmcp::TOOL_ID_SETTINGS_GET" in set_src
    assert "std::appmcp::TOOL_ID_SETTINGS_SET" in set_src
    assert "Headless AppMCP tool invocation handled successfully." in set_src
    assert "std::EVENT_IPC_MSG" in set_src

def test_file_manager_dual_mode_appmcp_reactor():
    fm_src = _read_file_manager_main_rs()
    assert "fn handle_appmcp_request" in fm_src, "File Manager must implement handle_appmcp_request"
    assert "std::appmcp::APPMCP_STATUS_PERMISSION_DENIED" in fm_src
    assert "std::appmcp::TOOL_ID_FILES_LIST" in fm_src
    assert "std::appmcp::TOOL_ID_FILES_READ" in fm_src
    assert "std::appmcp::TOOL_ID_FILES_WRITE" in fm_src
    assert "Headless AppMCP tool invocation handled successfully." in fm_src
    assert "std::EVENT_IPC_MSG" in fm_src

