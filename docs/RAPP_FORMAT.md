# InterOS RAPP Binary Package Specification

The **RAPP** (Rust Application Package) binary format is the native, deterministic execution package format for InterOS Core (`kernel/src/package/rapp.rs`).

---

## 1. Binary Layout & Header Structure

A `.rapp` file is a contiguous, non-archive binary structure consisting of a fixed header, a capability ID table, and an embedded native ELF executable payload:

```text
+-------------------------------------------------------------+
| RAPP Header (180 bytes / 0xB4)                              |
| - Magic: b"RAPP" (4 bytes)                                  |
| - Format Version: 2 (2 bytes, LE u16)                       |
| - Architecture: 0=x86_64, 1=aarch64, 2=riscv64 (2B, LE u16) |
| - Package ID: UTF-8, null-padded (32 bytes)                 |
| - Publisher Ed25519 Public Key (32 bytes)                   |
| - Semantic Version: (major, minor, patch) (6 bytes, 3x u16) |
| - Capability Count: N (2 bytes, LE u16)                     |
| - Payload SHA-256 Digest (32 bytes)                         |
| - Ed25519 Signature (64 bytes)                              |
| - Payload Length (4 bytes, LE u32)                          |
+-------------------------------------------------------------+
| Capability IDs: N x LE u16 (2 * N bytes)                    |
+-------------------------------------------------------------+
| Executable ELF Payload (Payload Length bytes)               |
+-------------------------------------------------------------+
```

### Exact Field Offsets (RAPP v2 & v3)

| Byte Range | Length | Type | Field Description |
| :--- | :--- | :--- | :--- |
| `0x00 - 0x03` | 4 bytes | `[u8; 4]` | Magic identifier: ASCII `b"RAPP"` (`0x52, 0x41, 0x50, 0x50`) |
| `0x04 - 0x05` | 2 bytes | `u16` (LE) | Format version: `2` (standard) or `3` (dependency extension) |
| `0x06 - 0x07` | 2 bytes | `u16` (LE) | Target CPU architecture (`0` = x86_64, `1` = AArch64, `2` = RISC-V 64) |
| `0x08 - 0x27` | 32 bytes | `[u8; 32]` | Package identifier (UTF-8, null-padded, max 32 chars) |
| `0x28 - 0x47` | 32 bytes | `[u8; 32]` | Publisher Ed25519 public key |
| `0x48 - 0x4D` | 6 bytes | `[u16; 3]` | Semantic version tuple: `(major, minor, patch)` |
| `0x4E - 0x4F` | 2 bytes | `u16` (LE) | Declared capability count `N` (maximum 1024) |
| `0x50 - 0x6F` | 32 bytes | `[u8; 32]` | SHA-256 cryptographic digest of the embedded ELF payload |
| `0x70 - 0xAF` | 64 bytes | `[u8; 64]` | Ed25519 digital signature over canonical signing message |
| `0xB0 - 0xB3` | 4 bytes | `u32` (LE) | Embedded ELF payload length in bytes |
| `0xB4 - 0xB7` | 4 bytes | `u32` (LE) | *(RAPP v3 only)* Dependency table length in bytes (`dependency_len`) |

### Payload & Table Offsets

- **RAPP v2 Header**: 180 bytes (`0xB4`).
  - Capabilities table: `0xB4 .. 0xB4 + 2*N` (Array of declared capability IDs, `u16` LE).
  - ELF executable binary payload: `0xB4 + 2*N .. 0xB4 + 2*N + payload_len`.
  - Exact total length: `0xB4 + 2*N + payload_len`.
- **RAPP v3 Header**: 184 bytes (`0xB8`).
  - Capabilities table: `0xB8 .. 0xB8 + 2*N`.
  - Dependency table: `0xB8 + 2*N .. 0xB8 + 2*N + dependency_len`.
    - `dep_count`: 2 bytes (`u16` LE, max 64).
    - Each dependency entry: `id_len` (1B u8) + `package_id` (`id_len` bytes UTF-8) + `kind` (1B: 0=Exact, 1=Minimum, 2=Major) + `major` (2B) + `minor` (2B) + `patch` (2B).
  - ELF executable binary payload: follows immediately after dependency table for `payload_len` bytes.
  - Exact total length: `0xB8 + 2*N + dependency_len + payload_len`.

---

## 2. Cryptographic Attestation & Verification Pipeline

When `parse_rapp(data: &[u8])` executes in the microkernel:

1. **Magic & Format Check**: Asserts `data[0..4] == b"RAPP"` and format version equals `2` (or `3`).
2. **Architecture Match**: Rejects binaries if the target architecture does not match the active kernel architecture.
3. **Exact Buffer Bounds**: Total slice length must strictly equal `0xB4 + 2*N + payload_len` (for v2) or `0xB8 + 2*N + dependency_len + payload_len` (for v3). Packages with trailing garbage, truncated payloads, or length mismatches are rejected with `RappError::BufferTooShort`, `RappError::PayloadOverflow`, or `RappError::InvalidPayload`.
4. **Payload Digest Verification**: Computes the SHA-256 digest over the ELF payload bytes and asserts exact equality with the header `payload_digest`.
5. **Trust Root Evaluation**: Checks the publisher's public key against the kernel's active trust store (`REVOCATION_REGISTRY`). Untrusted or revoked keys return `RappError::PublisherUntrusted` or `RappError::PublisherRevoked`.
6. **Canonical Signature Verification**:
   - The signing message is reconstructed by copying the header with the 64-byte signature field (`0x70..0xB0`) set to zero, followed by the capability array, followed by the dependency table (for v3), followed by the ELF payload bytes.
   - The signature is verified using `ed25519_dalek::Verifier`. Packages with zeroed, corrupt, or invalid signatures are rejected with `RappError::SignatureInvalid`.

---

## 3. Standard Capability Identifiers

Declared capability IDs map into per-process capability token table rights:

| Capability ID | Name | Permission Granted |
| :--- | :--- | :--- |
| `1` | `VFS_READ` | Read access within application-sandboxed storage |
| `2` | `VFS_WRITE` | Write and persist data in application-sandboxed storage |
| `3` | `DISPLAY` | Surface creation and framebuffer presentation |
| `4` | `TOUCH_INPUT` | Receive touch digitizer and gesture events |
| `5` | `AUDIO_OUT` | PCM audio stream playback access |
| `6` | `AUDIO_IN` | Microphone audio stream capture access |
| `7` | `NETWORK` | Socket creation and network operations |
| `8` | `CELLULAR` | Telephony dialer and SMS messaging service |
| `9` | `IPC_CHANNEL` | Dedicated bidirectional IPC channel |
| `10 - 18` | `EXTENDED_*` | Subsystem-specific hardware resources |

---

## 4. Package Lifecycle in Userspace

1. **Scaffolding & Packaging**: Developers scaffold projects via `interos new` and bundle binaries via `interos package <name>`.
2. **Signing**: Packages are signed with the publisher's Ed25519 private key via `interos sign <package> --key <key_file>`.
3. **Trust Provisioning**: Trust roots are established via firmware-anchored keys or authenticated provisioning. Unauthenticated serial interfaces (COM1) explicitly reject dynamic root-of-trust injection (`RAPP_TRUST`) to prevent unauthorized UART privilege escalation.
4. **Installation & Storage**: `interos install <package>` transfers the signed bytes to the guest over QEMU COM1, verifies integrity and signatures inside Ring 0 against the active trust store, and commits the package blob into persistent storage.
5. **Execution**: The launcher or PID 1 spawns an isolated Ring-3 child process with dedicated page tables (`W^X` enforced) and passes minted capability tokens.
