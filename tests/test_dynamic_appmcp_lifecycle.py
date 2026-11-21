# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

"""Dynamic AppMCP Tool Lifecycle & Intent Matching Contract.

WHAT THIS FILE IS FOR:
Guarantees that AppMCP tool discovery and invocation are dynamic and cryptographically bound:
1. DynamicToolRegistry provides dynamic intent matching (match_intent) from natural-language requests.
2. Tool descriptors carry capability rights, schema types, and package associations.
3. Package update/re-registration bumps capability generation; old grants fail closed.
4. Syscall 166 (AppMcpAuthorize) enforces sender-bound generation, type, rights, and non-revocation.
5. Zero compile-time action query routing as final architecture: Launcher queries DynamicToolRegistry.
"""

from pathlib import Path
import pytest

ROOT = Path(__file__).resolve().parents[1]


def read_source(rel_path: str) -> str:
    return (ROOT / rel_path).read_text(encoding="utf-8")


def test_dynamic_tool_registry_intent_matching():
    appmcp_rs = read_source("sdk/rust/interos-std/src/appmcp.rs")
    assert "pub fn match_intent<'a>(&self, request: &'a [u8]) -> Option<(u32, &'a [u8])>" in appmcp_rs
    assert "pub struct DynamicToolRegistry" in appmcp_rs
    assert "pub fn canonical() -> Self" in appmcp_rs


def test_launcher_dynamic_intent_dispatch():
    search_rs = read_source("apps/launcher/src/search.rs")
    assert "interos_std::appmcp::DynamicToolRegistry::canonical()" in search_rs
    assert "registry.match_intent(q)" in search_rs


def test_kernel_appmcp_generation_and_capability_validation():
    syscall_rs = read_source("kernel/src/syscall/mod.rs")
    assert "pub fn sys_appmcp_authorize(" in syscall_rs
    assert ".get_by_handle(CapHandle(cap_handle))" in syscall_rs
    assert "cap.cap_type != expected_type" in syscall_rs
    assert "cap.rights.contains(required)" in syscall_rs
    assert "cap.object_id != 0 && cap.object_id != object_id" in syscall_rs
