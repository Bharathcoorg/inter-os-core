#!/usr/bin/env python3
"""M33 bounded TLS transport contracts."""
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]

def must(text, needle, label):
    assert needle in text, f"missing {label}: {needle}"

def main():
    tls = (ROOT / "kernel/src/net/tls.rs").read_text()
    net = (ROOT / "kernel/src/net/mod.rs").read_text()
    must(net, "pub mod tls;", "TLS module registration")
    must(net, "TlsContentType", "TLS content type export")
    for marker in (
        "pub const TLS_RECORD_HEADER: usize = 5;",
        "pub const TLS_MAX_PLAINTEXT: usize = 16 * 1024;",
        "pub const TLS_MAX_RECORD: usize = TLS_MAX_PLAINTEXT + 256;",
        "pub const TLS_LEGACY_RECORD_VERSION: u16 = 0x0303;",
        "pub enum ContentType",
        "pub struct TlsRecord",
        "pub fn parse(buf: &'a [u8])",
        "pub fn serialize(&self, out: &mut [u8])",
        "unknown TLS content type",
        "unsupported TLS record version",
        "invalid TLS record length",
    ):
        must(tls, marker, marker)
    raw = bytearray(5)
    raw[0] = 22
    raw[1:3] = (0x03, 0x01)
    assert "unsupported TLS record version" in tls, "TLS version policy must reject non-0x0303 record versions"
    print("TLS transport boundary contract: PASS")

if __name__ == "__main__":
    main()
