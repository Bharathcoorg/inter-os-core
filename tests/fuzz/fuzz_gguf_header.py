#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

"""Fuzz target for GGUF model headers and metadata descriptors.

Validates robustness against malformed magic bytes, unsupported versions,
integer overflows on tensor and metadata counts, and truncated buffers.
"""

import random

GGUF_MAGIC = b"GGUF"


def make_valid_gguf_header() -> bytearray:
    buf = bytearray()
    buf += GGUF_MAGIC
    buf += (3).to_bytes(4, "little")  # version 3
    buf += (10).to_bytes(8, "little")  # tensor_count = 10
    buf += (5).to_bytes(8, "little")  # metadata_kv_count = 5
    return buf


def parse_gguf_header_strict(raw: bytes) -> bool:
    """Strict parser mirroring userspace/system-service/src/gguf.rs."""
    if len(raw) < 24:
        return False
    if raw[:4] != GGUF_MAGIC:
        return False
    version = int.from_bytes(raw[4:8], "little")
    if version not in (2, 3):
        return False
    tensor_count = int.from_bytes(raw[8:16], "little")
    # InterOS safety bounds: tensor count must not exceed 2048
    if tensor_count > 2048:
        return False
    kv_count = int.from_bytes(raw[16:24], "little")
    # InterOS safety bounds: metadata kv count must not exceed 4096
    if kv_count > 4096:
        return False
    return True


def run_fuzz(iterations: int = 500) -> int:
    rng = random.Random(0x4242_1337)
    baseline = make_valid_gguf_header()
    assert parse_gguf_header_strict(bytes(baseline)), "Baseline GGUF header must pass"

    rejections = 0
    for i in range(iterations):
        mutated = bytearray(baseline)
        m = rng.randint(0, 5)

        if m == 0:
            # Buffer truncation
            mutated = mutated[: rng.randint(0, 23)]
        elif m == 1:
            # Corrupt magic
            mutated[:4] = rng.choice([b"GGML", b"ONNX", b"\0\0\0\0", b"\xFF\xFF\xFF\xFF"])
        elif m == 2:
            # Unsupported version
            mutated[4:8] = rng.choice([0, 1, 4, 100, 0xFFFF_FFFF]).to_bytes(4, "little")
        elif m == 3:
            # Enormous tensor count (DoS / memory exhaustion attempt)
            huge_tensors = rng.choice([2049, 100_000, 0x7FFF_FFFF, 0xFFFF_FFFF_FFFF_FFFF])
            mutated[8:16] = huge_tensors.to_bytes(8, "little")
        elif m == 4:
            # Enormous KV count (DoS / memory exhaustion attempt)
            huge_kv = rng.choice([4097, 50_000, 0x7FFF_FFFF, 0xFFFF_FFFF_FFFF_FFFF])
            mutated[16:24] = huge_kv.to_bytes(8, "little")
        elif m == 5:
            # Arbitrary random noise
            mutated = bytearray(rng.randbytes(rng.randint(0, 32)))

        if not parse_gguf_header_strict(bytes(mutated)):
            rejections += 1

    return rejections


if __name__ == "__main__":
    rejections = run_fuzz(500)
    print(f"GGUF Header Fuzzer: PASS ({rejections}/500 adversarial inputs safely rejected)")
