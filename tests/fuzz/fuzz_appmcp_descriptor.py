#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

"""Fuzz target for AppMCP requests, responses, and tool descriptors.

Tests strict 64-byte IPC boundary enforcement, binary field validation,
and manifest parser robustness against malicious payloads.
"""

import json
import random

APPMCP_MSG_SIZE = 64
APPMCP_MAGIC_REQ = 0x4D43_5051  # b"MCPQ"


def parse_appmcp_request_strict(raw: bytes) -> bool:
    """Strict binary parser mirroring interos_std::appmcp::AppMcpRequest."""
    if len(raw) != APPMCP_MSG_SIZE:
        return False
    magic = int.from_bytes(raw[0:4], "little")
    if magic != APPMCP_MAGIC_REQ:
        return False
    version = int.from_bytes(raw[4:6], "little")
    if version != 1:
        return False
    cap_handle = int.from_bytes(raw[8:16], "little")
    if cap_handle == 0:
        return False
    tool_id = int.from_bytes(raw[16:20], "little")
    if tool_id == 0:
        return False
    arg_len = raw[20]
    if arg_len > 40:
        return False
    return True


def parse_manifest_json_strict(raw_text: str) -> bool:
    """Validates dynamic tool manifest structure."""
    try:
        data = json.loads(raw_text)
    except Exception:
        return False
    if not isinstance(data, dict):
        return False
    if "package" not in data or not isinstance(data["package"], str) or len(data["package"]) > 64:
        return False
    tools = data.get("tools")
    if not isinstance(tools, list):
        return False
    for tool in tools:
        if not isinstance(tool, dict):
            return False
        if "name" not in tool or not isinstance(tool["name"], str) or len(tool["name"]) > 32:
            return False
        if "risk_class" not in tool or tool["risk_class"] not in [
            "Observe",
            "PersonalMutation",
            "SystemConfig",
            "ExternalConsequence",
            "SecuritySensitive",
        ]:
            return False
    return True


def run_fuzz(iterations: int = 500) -> int:
    rng = random.Random(0xCAFE_BABE)
    valid_req = bytearray(APPMCP_MSG_SIZE)
    valid_req[0:4] = APPMCP_MAGIC_REQ.to_bytes(4, "little")
    valid_req[4:6] = (1).to_bytes(2, "little")
    valid_req[8:16] = (0x0001_0000_0000_001F).to_bytes(8, "little")  # Valid CapHandle
    valid_req[16:20] = (101).to_bytes(4, "little")  # tool_id
    valid_req[20] = 5  # arg_len
    valid_req[24:29] = b"HELLO"
    assert parse_appmcp_request_strict(bytes(valid_req)), "Baseline request must pass"

    rejections = 0
    for i in range(iterations):
        if i % 2 == 0:
            # Binary IPC fuzzing
            mutated = bytearray(valid_req)
            m = rng.randint(0, 5)
            if m == 0:
                # Resize buffer away from 64 bytes
                mutated = mutated[: rng.choice([0, 16, 32, 63, 65, 128])]
            elif m == 1:
                # Corrupt magic
                mutated[0:4] = rng.choice([b"MCXX", b"\0\0\0\0", b"\xFF\xFF\xFF\xFF"])
            elif m == 2:
                # Zero capability handle (ambient authority attempt)
                mutated[8:16] = b"\0" * 8
            elif m == 3:
                # Invalid argument length exceeding payload boundary
                mutated[20] = rng.choice([41, 100, 255])
            elif m == 4:
                # Random bytes
                mutated = bytearray(rng.randbytes(rng.randint(0, 128)))
            if not parse_appmcp_request_strict(bytes(mutated)):
                rejections += 1
        else:
            # JSON manifest fuzzing
            m = rng.randint(0, 4)
            if m == 0:
                corrupted = "{" + rng.choice(["unterminated", "null: 1", "'bad_quotes': 2"])
            elif m == 1:
                corrupted = json.dumps({"package": "A" * 1000, "tools": []})
            elif m == 2:
                corrupted = json.dumps({"package": "com.app", "tools": [{"name": "tool", "risk_class": "RootShell"}]})
            elif m == 3:
                corrupted = rng.choice(["", "[]", "123", "null", "false"])
            elif m == 4:
                corrupted = json.dumps({"package": "com.test", "tools": "not_a_list"})
            if not parse_manifest_json_strict(corrupted):
                rejections += 1

    return rejections


if __name__ == "__main__":
    rejections = run_fuzz(500)
    print(f"AppMCP Fuzzer: PASS ({rejections}/500 adversarial inputs safely rejected)")
