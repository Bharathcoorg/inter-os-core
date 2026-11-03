#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

"""Adversarial RAPP v2 format and Ed25519 trust-boundary tests."""
import hashlib
from cryptography.hazmat.primitives.asymmetric import ed25519
from cryptography.hazmat.primitives import serialization
from cryptography.exceptions import InvalidSignature

HEADER = 0xB4
SIG_OFF = 0x70
SIG_END = 0xB0

def make_package(payload=b"\\x7fELF-test"):
    key = ed25519.Ed25519PrivateKey.generate()
    pub = key.public_key().public_bytes(serialization.Encoding.Raw, serialization.PublicFormat.Raw)
    pkg = bytearray()
    pkg += b"RAPP" + (2).to_bytes(2,"little") + (0).to_bytes(2,"little")
    pkg += b"org.interos.test".ljust(32,b"\0") + pub
    pkg += (1).to_bytes(2,"little") * 3 + (1).to_bytes(2,"little")
    pkg += hashlib.sha256(payload).digest() + b"\0"*64 + len(payload).to_bytes(4,"little")
    pkg += (1).to_bytes(2,"little") + payload
    signing = bytes(pkg)
    sig = key.sign(signing)
    pkg[SIG_OFF:SIG_END] = sig
    return pkg, pub, key

def verify(pkg):
    assert pkg[:4] == b"RAPP" and int.from_bytes(pkg[4:6],"little") == 2
    caps = int.from_bytes(pkg[0x4E:0x50],"little")
    off = HEADER + caps*2
    assert off + int.from_bytes(pkg[0xB0:0xB4],"little") == len(pkg)
    assert hashlib.sha256(pkg[off:]).digest() == pkg[0x50:0x70]
    pub = bytes(pkg[0x28:0x48])
    sig = bytes(pkg[SIG_OFF:SIG_END])
    signing = bytearray(pkg); signing[SIG_OFF:SIG_END] = b"\0"*64
    ed25519.Ed25519PublicKey.from_public_bytes(pub).verify(sig, bytes(signing))

def expect_reject(blob, label):
    try:
        verify(blob)
    except Exception:
        return
    raise AssertionError(f"{label} accepted")


def main():
    pkg, pub, key = make_package()
    verify(pkg)
    tampered = bytearray(pkg); tampered[-1] ^= 1
    expect_reject(tampered, "payload tamper")
    tampered_sig = bytearray(pkg); tampered_sig[SIG_OFF] ^= 1
    expect_reject(tampered_sig, "signature tamper")
    trailing = bytearray(pkg) + b"TRAILING"
    expect_reject(trailing, "trailing bytes")
    zero_key = bytearray(pkg); zero_key[0x28:0x48] = b"\0"*32
    expect_reject(zero_key, "zero publisher key")
    print("RAPP v2 adversarial tests: PASS")

if __name__ == "__main__": main()
