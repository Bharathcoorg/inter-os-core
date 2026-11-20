#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

"""Parser Robustness & Security Fuzzing Contract.

Orchestrates deterministic fuzzing passes against high-risk platform parsers:
1. .RAPP v2 container headers and manifest chunks (`fuzz_rapp_manifest`).
2. AppMCP 64-byte IPC frames and JSON tool descriptors (`fuzz_appmcp_descriptor`).
3. Companion slot markers and encrypted state records (`fuzz_companion_state`).
4. GGUF tensor descriptors and metadata bounds (`fuzz_gguf_header`).
"""

import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "tests" / "fuzz"))

import fuzz_rapp_manifest
import fuzz_appmcp_descriptor
import fuzz_companion_state
import fuzz_gguf_header


def test_rapp_manifest_fuzz_robustness() -> None:
    rejections = fuzz_rapp_manifest.run_fuzz(200)
    assert rejections >= 180, f"Expected high rejection rate for corrupted RAPP containers, got {rejections}"


def test_appmcp_descriptor_fuzz_robustness() -> None:
    rejections = fuzz_appmcp_descriptor.run_fuzz(200)
    assert rejections >= 180, f"Expected high rejection rate for corrupted AppMCP descriptors, got {rejections}"


def test_companion_state_fuzz_robustness() -> None:
    rejections = fuzz_companion_state.run_fuzz(200)
    assert rejections >= 180, f"Expected high rejection rate for corrupted slot markers, got {rejections}"


def test_gguf_header_fuzz_robustness() -> None:
    rejections = fuzz_gguf_header.run_fuzz(200)
    assert rejections >= 180, f"Expected high rejection rate for corrupted GGUF headers, got {rejections}"


def main() -> None:
    test_rapp_manifest_fuzz_robustness()
    test_appmcp_descriptor_fuzz_robustness()
    test_companion_state_fuzz_robustness()
    test_gguf_header_fuzz_robustness()
    print("Parser fuzz contract: PASS")


if __name__ == "__main__":
    main()
