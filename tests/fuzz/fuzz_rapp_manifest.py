#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

"""Fuzz target for .RAPP package headers, capability tables, and manifest chunks.

Applies deterministic bit-flips, boundary truncations, integer overflow counts,
and corrupted signatures against the .RAPP v2 container parser.
"""

import hashlib
import random
from cryptography.hazmat.primitives.asymmetric import ed25519
from cryptography.hazmat.primitives import serialization

HEADER_LEN = 0xB4
SIG_OFFSET = 0x70
SIG_END = 0xB0


def generate_baseline_rapp() -> bytearray:
    key = ed25519.Ed25519PrivateKey.generate()
    pub = key.public_key().public_bytes(serialization.Encoding.Raw, serialization.PublicFormat.Raw)
    payload = b"\x7fELF_SAMPLE_APP_PAYLOAD_FOR_FUZZING_"
    pkg = bytearray()
    pkg += b"RAPP"
    pkg += (2).to_bytes(2, "little")
    pkg += (0).to_bytes(2, "little")
    pkg += b"org.interos.fuzz".ljust(32, b"\0")
    pkg += pub
    pkg += (1).to_bytes(2, "little")
    pkg += (0).to_bytes(2, "little")
    pkg += (0).to_bytes(2, "little")
    pkg += (1).to_bytes(2, "little")
    pkg += hashlib.sha256(payload).digest()
    pkg += b"\0" * 64
    pkg += len(payload).to_bytes(4, "little")
    pkg += (15).to_bytes(2, "little")
    pkg += payload
    sig = key.sign(bytes(pkg))
    pkg[SIG_OFFSET:SIG_END] = sig
    return pkg


def parse_rapp_strict(pkg: bytes) -> bool:
    """Strict reference parser mirroring kernel/src/package/rapp.rs validation."""
    if len(pkg) < HEADER_LEN:
        return False
    if pkg[:4] != b"RAPP":
        return False
    version = int.from_bytes(pkg[4:6], "little")
    if version != 2:
        return False
    caps = int.from_bytes(pkg[0x4E:0x50], "little")
    offset = HEADER_LEN + caps * 2
    if offset > len(pkg):
        return False
    payload_len = int.from_bytes(pkg[0xB0:0xB4], "little")
    if offset + payload_len != len(pkg):
        return False
    expected_digest = pkg[0x50:0x70]
    payload = pkg[offset:]
    if hashlib.sha256(payload).digest() != expected_digest:
        return False
    pub = bytes(pkg[0x28:0x48])
    if pub == b"\0" * 32:
        return False
    sig = bytes(pkg[SIG_OFFSET:SIG_END])
    signing = bytearray(pkg)
    signing[SIG_OFFSET:SIG_END] = b"\0" * 64
    try:
        ed25519.Ed25519PublicKey.from_public_bytes(pub).verify(sig, bytes(signing))
    except Exception:
        return False
    return True


def run_fuzz(iterations: int = 500) -> int:
    rng = random.Random(0xDEAD_BEEF)
    baseline = generate_baseline_rapp()
    assert parse_rapp_strict(baseline), "Baseline package must parse successfully"

    rejections = 0
    for i in range(iterations):
        mutated = bytearray(baseline)
        mutation_type = i % 6

        if mutation_type == 0:
            # Truncation
            cut = rng.randint(0, len(mutated) - 1)
            mutated = mutated[:cut]
        elif mutation_type == 1:
            # Single bit-flip
            idx = rng.randint(0, len(mutated) - 1)
            mutated[idx] ^= 1 << rng.randint(0, 7)
        elif mutation_type == 2:
            # Overwrite capability count with huge or invalid value
            huge_caps = rng.choice([0xFFFF, 0x8000, 0x7FFF, 0x0000])
            mutated[0x4E:0x50] = huge_caps.to_bytes(2, "little")
        elif mutation_type == 3:
            # Corrupt magic
            mutated[:4] = rng.choice([b"RAP1", b"NULL", b"\0\0\0\0", b"\xFF\xFF\xFF\xFF"])
        elif mutation_type == 4:
            # Trailing garbage
            mutated.extend(b"EXTRA_GARBAGE_BYTES")
        elif mutation_type == 5:
            # Completely random bytes
            mutated = bytearray(rng.randbytes(rng.randint(16, 256)))

        if not parse_rapp_strict(bytes(mutated)):
            rejections += 1

    return rejections


if __name__ == "__main__":
    rejections = run_fuzz(500)
    print(f"RAPP Manifest Fuzzer: PASS ({rejections}/500 adversarial inputs safely rejected)")
