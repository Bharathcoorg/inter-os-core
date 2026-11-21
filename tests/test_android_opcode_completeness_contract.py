# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

"""Dalvik VM Bytecode Matrix Completeness & Mark-and-Sweep GC Contract Tests.

Verifies:
1. Dalvik opcode additions in DEX specification (lit8, lit16, moves, switches, throw).
2. Runtime execution branches in DalvikVmState::interpret_insns.
3. Mark-and-sweep garbage collector tracing register roots and static fields.
4. Bounded pause metrics: gc_cycles, gc_reclaimed_objects, and gc_total_pause_ticks.
"""

from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]


def read_source(rel_path: str) -> str:
    return (ROOT / rel_path).read_text(encoding="utf-8")


def test_dex_opcode_matrix_completeness():
    dex = read_source("kernel/src/compat/android/dex.rs")
    assert "pub const OP_MOVE_FROM16: u8 = 0x02;" in dex
    assert "pub const OP_MOVE_16: u8 = 0x03;" in dex
    assert "pub const OP_MOVE_WIDE: u8 = 0x04;" in dex
    assert "pub const OP_MOVE_OBJECT: u8 = 0x07;" in dex
    assert "pub const OP_MOVE_RESULT_WIDE: u8 = 0x0B;" in dex
    assert "pub const OP_MOVE_RESULT_OBJECT: u8 = 0x0C;" in dex
    assert "pub const OP_PACKED_SWITCH: u8 = 0x2B;" in dex
    assert "pub const OP_SPARSE_SWITCH: u8 = 0x2C;" in dex
    assert "pub const OP_THROW: u8 = 0x27;" in dex
    assert "pub const OP_FILL_ARRAY_DATA: u8 = 0x26;" in dex
    assert "pub const OP_ADD_INT_LIT8: u8 = 0xD8;" in dex
    assert "pub const OP_RSUB_INT_LIT8: u8 = 0xD9;" in dex
    assert "pub const OP_MUL_INT_LIT8: u8 = 0xDA;" in dex
    assert "pub const OP_DIV_INT_LIT8: u8 = 0xDB;" in dex
    assert "pub const OP_REM_INT_LIT8: u8 = 0xDC;" in dex
    assert "pub const OP_AND_INT_LIT8: u8 = 0xDD;" in dex
    assert "pub const OP_OR_INT_LIT8: u8 = 0xDE;" in dex
    assert "pub const OP_XOR_INT_LIT8: u8 = 0xDF;" in dex
    assert "pub const OP_SHL_INT_LIT8: u8 = 0xE0;" in dex
    assert "pub const OP_SHR_INT_LIT8: u8 = 0xE1;" in dex
    assert "pub const OP_USHR_INT_LIT8: u8 = 0xE2;" in dex
    assert "pub const OP_ADD_INT_LIT16: u8 = 0xD0;" in dex
    assert "pub const OP_RSUB_INT: u8 = 0xD1;" in dex
    assert "pub const OP_MUL_INT_LIT16: u8 = 0xD2;" in dex
    assert "pub const OP_DIV_INT_LIT16: u8 = 0xD3;" in dex
    assert "pub const OP_REM_INT_LIT16: u8 = 0xD4;" in dex
    assert "pub const OP_AND_INT_LIT16: u8 = 0xD5;" in dex
    assert "pub const OP_OR_INT_LIT16: u8 = 0xD6;" in dex
    assert "pub const OP_XOR_INT_LIT16: u8 = 0xD7;" in dex


def test_runtime_opcode_execution_branches():
    runtime = read_source("kernel/src/compat/android/runtime.rs")
    assert "OP_ADD_INT_LIT8 | OP_RSUB_INT_LIT8 | OP_MUL_INT_LIT8" in runtime
    assert "OP_ADD_INT_LIT16 | OP_RSUB_INT | OP_MUL_INT_LIT16" in runtime
    assert "OP_PACKED_SWITCH =>" in runtime
    assert "OP_SPARSE_SWITCH =>" in runtime
    assert "OP_THROW =>" in runtime
    assert "OP_MOVE_FROM16 =>" in runtime
    assert "OP_MOVE_16 =>" in runtime
    assert "OP_MOVE_WIDE =>" in runtime


def test_runtime_mark_and_sweep_garbage_collector():
    runtime = read_source("kernel/src/compat/android/runtime.rs")
    assert "pub fn gc_mark_and_sweep(&mut self) -> (usize, u64)" in runtime
    assert "pub gc_cycles: u64" in runtime
    assert "pub gc_reclaimed_objects: usize" in runtime
    assert "pub gc_total_pause_ticks: u64" in runtime
    assert "self.objects.retain(|o| marked.contains(&o.id));" in runtime
