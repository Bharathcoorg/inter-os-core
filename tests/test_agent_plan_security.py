# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

"""Agent Plan Authorization, AppMCP Exact Grants, and IPC Security Invariants.

Guarantees Zone 4 & Agent Manager security properties:
1. MSG_TYPE_AGENT_PLAN proposals, approvals, and cancellations strictly require
   authenticated agent admin identity (is_authorized_agent_admin), returning
   UNAUTHORIZED_CALLER to unprivileged senders.
2. MSG_TYPE_TOOL_EXEC execution strictly requires authenticated agent admin
   identity, preventing confused-deputy tool invocation.
3. MSG_TYPE_COMPANION_QUERY opcode 8 (tool synchronization) is strictly gated
   behind admin authentication.
4. Screen lock (set_locked(true)) and plan cancellation revoke all invocation grants
   and cancel pending execution plans.
5. Legacy CRC32 consent methods fail closed; only exact 32-byte SHA-256 arguments
   digests authorize tool execution.
6. Invocation grants with wildcard user (user_id == 0), indefinite expiry (expiry_ticks == 0),
   or zero digest ([0u8; 32]) are strictly rejected.
"""

from pathlib import Path
import re
import pytest

ROOT = Path(__file__).resolve().parents[1]


def read_source(rel_path: str) -> str:
    return (ROOT / rel_path).read_text(encoding="utf-8")


def test_agent_plan_ipc_sender_authorization():
    service_lib = read_source("userspace/system-service/src/lib.rs")

    # MSG_TYPE_AGENT_PLAN must verify is_authorized_agent_admin before any opcode
    agent_plan_block = service_lib.split("std::ai::MSG_TYPE_AGENT_PLAN => {")[1].split("std::ai::MSG_TYPE_TOOL_EXEC => {")[0]
    assert "if !is_authorized_agent_admin(sender_task_id) {" in agent_plan_block
    assert 'b"UNAUTHORIZED_CALLER"' in agent_plan_block

    # Opcode 2 must verify plan_id if provided
    assert "req_plan_id != plan.id" in agent_plan_block
    assert 'b"PLAN_ID_MISMATCH"' in agent_plan_block


def test_tool_exec_ipc_sender_authorization():
    service_lib = read_source("userspace/system-service/src/lib.rs")

    # MSG_TYPE_TOOL_EXEC must verify is_authorized_agent_admin before execution
    tool_exec_block = service_lib.split("std::ai::MSG_TYPE_TOOL_EXEC => {")[1].split("emit_agent_ack(sender_task_id, msg_type, 0xFF, b\"UNSUPPORTED_MSG\", tx_buf);")[0]
    assert "if !is_authorized_agent_admin(sender_task_id) {" in tool_exec_block
    assert 'b"UNAUTHORIZED_CALLER"' in tool_exec_block


def test_companion_query_admin_guard_covers_all_sensitive_opcodes():
    service_lib = read_source("userspace/system-service/src/lib.rs")

    # Must include 1 (lock), 2 (learning mode), 3 (private), 4 (clear memory), 6 (grant), 7 (clear grants), 8 (sync tools)
    query_block = service_lib.split("std::ai::MSG_TYPE_COMPANION_QUERY => {")[1].split("std::ai::MSG_TYPE_AGENT_PLAN => {")[0]
    assert "matches!(payload[0], 1 | 2 | 3 | 4 | 6 | 7 | 8)" in query_block
    assert "!is_authorized_agent_admin(sender_task_id)" in query_block


def test_lock_and_cancel_revoke_grants_and_plans():
    service_lib = read_source("userspace/system-service/src/lib.rs")

    # Screen lock in MSG_TYPE_COMPANION_QUERY clears grants and cancels plan
    query_block = service_lib.split("std::ai::MSG_TYPE_COMPANION_QUERY => {")[1].split("std::ai::MSG_TYPE_AGENT_PLAN => {")[0]
    lock_branch = query_block.split("1 => {")[1].split("2 => {")[0]
    assert "tool_registry.clear_invocation_grants();" in lock_branch
    assert "plan.cancel();" in lock_branch

    # Plan cancel in MSG_TYPE_AGENT_PLAN clears grants
    agent_plan_block = service_lib.split("std::ai::MSG_TYPE_AGENT_PLAN => {")[1].split("std::ai::MSG_TYPE_TOOL_EXEC => {")[0]
    cancel_branch = agent_plan_block.split("3 => {")[1].split("4 => {")[0]
    assert "tool_registry.clear_invocation_grants();" in cancel_branch


def test_crc32_dual_grant_eliminated_from_service():
    service_lib = read_source("userspace/system-service/src/lib.rs")

    agent_plan_block = service_lib.split("std::ai::MSG_TYPE_AGENT_PLAN => {")[1].split("std::ai::MSG_TYPE_TOOL_EXEC => {")[0]

    # No legacy CRC32 consent installation in plan approval
    assert "grant_invocation_consent" not in agent_plan_block

    # No legacy CRC32 consent fallback in step execution
    assert "consume_invocation_consent" not in agent_plan_block

    # Opcode 6 in companion query must not call grant_invocation_consent
    query_block = service_lib.split("std::ai::MSG_TYPE_COMPANION_QUERY => {")[1].split("std::ai::MSG_TYPE_AGENT_PLAN => {")[0]
    opcode_6 = query_block.split("6 => {")[1].split("7 => {")[0]
    assert "grant_invocation_consent" not in opcode_6
    assert "payload.len() >= 37" in opcode_6


def test_appmcp_sdk_strict_invariants():
    appmcp_src = read_source("sdk/rust/interos-std/src/appmcp.rs")

    # grant_exact_invocation must reject user_id == 0, expiry_ticks == 0, args_digest == [0u8; 32]
    grant_fn = appmcp_src.split("pub fn grant_exact_invocation")[1].split("pub fn consume_exact_invocation")[0]
    assert "grant.user_id == 0" in grant_fn
    assert "grant.expiry_ticks == 0" in grant_fn
    assert "grant.args_digest == [0u8; 32]" in grant_fn

    # consume_exact_invocation must enforce unexpired deadline and user_id match
    consume_fn = appmcp_src.split("pub fn consume_exact_invocation")[1].split("pub fn has_exact_invocation")[0]
    assert "g.user_id == user_id" in consume_fn
    assert "g.user_id == 0" not in consume_fn
    assert "g.expiry_ticks > 0" in consume_fn
    assert "current_ticks <= g.expiry_ticks" in consume_fn

    # Legacy CRC32 methods must fail closed
    assert "pub fn grant_invocation_consent(&mut self, _tool_id: u32, _args_crc: u32) -> bool {\n        false\n    }" in appmcp_src
    assert "pub fn consume_invocation_consent(&mut self, _tool_id: u32, _args_crc: u32) -> bool {\n        false\n    }" in appmcp_src
    assert "pub fn has_invocation_consent(&self, _tool_id: u32, _args_crc: u32) -> bool {\n        false\n    }" in appmcp_src


def main():
    test_agent_plan_ipc_sender_authorization()
    test_tool_exec_ipc_sender_authorization()
    test_companion_query_admin_guard_covers_all_sensitive_opcodes()
    test_lock_and_cancel_revoke_grants_and_plans()
    test_crc32_dual_grant_eliminated_from_service()
    test_appmcp_sdk_strict_invariants()
    print("Agent Plan & AppMCP Security Invariants: PASS")


if __name__ == "__main__":
    main()
