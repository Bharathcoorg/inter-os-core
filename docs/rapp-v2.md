# InterOS RAPP v2 Trust-Boundary Specification

Status: implementation target for Phase 22.

## Canonical package

RAPP v2 is a deterministic binary package. JSON/ZIP bundles are developer interchange artifacts only and are not kernel-loadable packages.

Header fields, in order:
- magic: 4 bytes, RAPP
- format: LE u16, 2
- architecture: LE u16 (0=x86_64, 1=aarch64, 2=riscv64)
- package id: 32-byte UTF-8, NUL padded
- publisher public key: 32-byte Ed25519 raw public key
- semantic version: three LE u16 values
- capability count: LE u16
- SHA-256 payload digest: 32 bytes
- Ed25519 signature: 64 bytes
- payload length: LE u32
- capability ids: capability_count × LE u16
- ELF payload

The signature message is the complete header with the signature field zeroed, followed by the capability array and ELF payload. The SHA-256 digest covers only the ELF payload.

## Trust policy

A package is loadable only when:
1. the format and target architecture are supported;
2. the ELF payload is structurally valid for the selected architecture;
3. the payload digest matches;
4. the Ed25519 signature verifies against the embedded publisher key;
5. the publisher key is trusted and not revoked;
6. every requested capability is recognized and policy-allowed;
7. package identity/version policy permits installation.

Unsigned packages, zero signatures, malformed keys, unknown capabilities, revoked publishers, and digest mismatches are rejected before execution.

## Migration

The old v1 binary format and ZIP/JSON developer bundle must not be accepted as a production execution format. The CLI must emit v2 and may retain an explicitly named conversion command for development tooling.
