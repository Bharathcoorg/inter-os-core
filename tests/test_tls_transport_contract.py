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
        "pub const TLS_MAX_RECORD: usize = 16384;",
        "pub enum ContentType",
        "pub struct TlsRecord",
        "pub fn parse(buf: &'a [u8])",
        "pub fn serialize(&self, out: &mut [u8])",
        "unknown TLS content type",
        "unsupported TLS record version",
        "invalid TLS record length",
    ):
        must(tls, marker, marker)
    print("TLS transport boundary contract: PASS")

if __name__ == "__main__":
    main()
