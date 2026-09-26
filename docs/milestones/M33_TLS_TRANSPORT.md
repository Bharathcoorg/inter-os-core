# M33 — TLS Transport Boundary

## Status

Complete as a **transport framing boundary**.

## Delivered

- TLS record header parsing with strict 5-byte header validation.
- Supported TLS content types: ChangeCipherSpec, Alert, Handshake, ApplicationData.
- TLS legacy record version pinned to `0x0303`, as required by the TLS 1.3 record framing model.
- Bounded 16 KiB plaintext records.
- Bounded record payload capacity of 16 KiB + 256 bytes for protocol overhead.
- Serialize/parse round-trip coverage.
- Invalid content-type rejection.
- Invalid/truncated record-length rejection.
- Invalid record-version rejection.
- Public export through the kernel network module.
- Dedicated source-level regression contract.

## Deliberate scope boundary

M33 does **not** claim a complete TLS implementation. No TLS handshake state machine, certificate validation, key exchange, AEAD encryption/decryption, or trust-store policy is implemented by this boundary. Parsed records must not be treated as authenticated until a real cryptographic TLS provider is integrated.

## Acceptance

The network stack now has a stable, bounded TLS record-layer interface that later TLS cryptography can consume without changing the Ethernet/IP/TCP transport layers.
