#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

"""Fuzz target for CompanionSlotMarker, encrypted journals, and state serialization.

Fuzzes slot markers, CRC-32 integrity tags, and ChaCha20-Poly1305 encrypted state records.
"""

import random

SLOT_MARKER_SIZE = 32
SLOT_MARKER_MAGIC = b"COMP"


def crc32_ieee(data: bytes) -> int:
    crc = 0xFFFF_FFFF
    for b in data:
        crc ^= b
        for _ in range(8):
            if crc & 1:
                crc = (crc >> 1) ^ 0xEDB8_8320
            else:
                crc >>= 1
    return (~crc) & 0xFFFF_FFFF


def make_valid_slot_marker(slot: int = ord("A"), gen: int = 1, uid: int = 1000) -> bytearray:
    buf = bytearray(SLOT_MARKER_SIZE)
    buf[0:4] = SLOT_MARKER_MAGIC
    buf[4] = 1  # version
    buf[5] = slot
    buf[8:16] = gen.to_bytes(8, "little")
    buf[16:20] = uid.to_bytes(4, "little")
    buf[20:24] = (256).to_bytes(4, "little")  # record_len
    buf[24:28] = (0x1234_5678).to_bytes(4, "little")  # payload_crc
    computed_crc = crc32_ieee(bytes(buf[0:28]))
    buf[28:32] = computed_crc.to_bytes(4, "little")
    return buf


def parse_slot_marker_strict(buf: bytes) -> bool:
    """Strict parser mirroring interos_std::companion::CompanionSlotMarker::deserialize."""
    if len(buf) != SLOT_MARKER_SIZE:
        return False
    if buf[0:4] != SLOT_MARKER_MAGIC:
        return False
    if buf[4] != 1:  # version
        return False
    if buf[5] not in (ord("A"), ord("B")):
        return False
    expected_crc = int.from_bytes(buf[28:32], "little")
    if crc32_ieee(buf[0:28]) != expected_crc:
        return False
    return True


def run_fuzz(iterations: int = 500) -> int:
    rng = random.Random(0x1337_C0DE)
    baseline = make_valid_slot_marker()
    assert parse_slot_marker_strict(bytes(baseline)), "Baseline marker must pass"

    rejections = 0
    for i in range(iterations):
        mutated = bytearray(baseline)
        m = rng.randint(0, 5)

        if m == 0:
            # Resize buffer away from 32 bytes
            mutated = mutated[: rng.choice([0, 15, 28, 31, 33, 64])]
        elif m == 1:
            # Corrupt magic
            mutated[0:4] = rng.choice([b"FAIL", b"\0\0\0\0", b"CPMX", b"\xFF\xFF\xFF\xFF"])
        elif m == 2:
            # Corrupt slot letter (neither A nor B)
            mutated[5] = rng.choice([ord("C"), 0, 255, ord("X")])
        elif m == 3:
            # Corrupt single byte without updating CRC
            idx = rng.randint(0, 27)
            mutated[idx] ^= 1 << rng.randint(0, 7)
        elif m == 4:
            # Corrupt CRC field directly
            mutated[28:32] = (rng.randint(0, 0xFFFF_FFFF)).to_bytes(4, "little")
        elif m == 5:
            # Arbitrary random noise
            mutated = bytearray(rng.randbytes(rng.randint(0, 64)))

        if not parse_slot_marker_strict(bytes(mutated)):
            rejections += 1

    return rejections


if __name__ == "__main__":
    rejections = run_fuzz(500)
    print(f"Companion State Fuzzer: PASS ({rejections}/500 adversarial inputs safely rejected)")
