# InterOS RAPP Binary Package Specification

The **RAPP** (Rust Application Package) binary format is the canonical execution package format for InterOS Core. It provides deterministic, cryptographically signed, capability-governed native application packaging across all supported architectures (`x86_64`, `aarch64`, `riscv64`).

---

## 1. Binary Layout & Header Structure

A `.rapp` file is a contiguous binary consisting of a fixed 180-byte (`0xB4`) header, followed by a dynamic capability identifier array, followed by the native ELF binary payload:

```
+-------------------------------------------------------------+
| Header (180 bytes / 0xB4)                                   |
| - Magic: b"RAPP" (4 bytes)                                  |
| - Format Version: 2 (2 bytes, LE u16)                       |
| - Architecture: 0=x86_64, 1=aarch64, 2=riscv64 (2 bytes)   |
| - Package ID: UTF-8, null-padded (32 bytes)                 |
| - Publisher Ed25519 Public Key (32 bytes)                   |
| - Semantic Version: Major.Minor.Patch (6 bytes, 3x LE u16)  |
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

### Detailed Field Offsets

| Offset | Length | Type | Description |
| :--- | :--- | :--- | :--- |
| `0x00 - 0x03` | 4 bytes | `[u8; 4]` | Magic identifier: ASCII `b"RAPP"` (`0x52, 0x41, 0x50, 0x50`) |
| `0x04 - 0x05` | 2 bytes | `u16` (LE) | Format version: must equal `2` |
| `0x06 - 0x07` | 2 bytes | `u16` (LE) | Target CPU architecture (`0` = x86_64, `1` = aarch64, `2` = riscv64) |
| `0x08 - 0x27` | 32 bytes | `[u8; 32]` | Package identifier (UTF-8, null-padded, e.g. `interos.calculator\0...`) |
| `0x28 - 0x47` | 32 bytes | `[u8; 32]` | Publisher Ed25519 public key |
| `0x48 - 0x4D` | 6 bytes | `[u16; 3]` (LE) | Semantic version tuple: `(major, minor, patch)` |
| `0x4E - 0x4F` | 2 bytes | `u16` (LE) | Declared capability count `N` (maximum 1024) |
| `0x50 - 0x6F` | 32 bytes | `[u8; 32]` | SHA-256 digest of the ELF executable payload |
| `0x70 - 0xAF` | 64 bytes | `[u8; 64]` | Ed25519 signature over canonical signing message |
| `0xB0 - 0xB3` | 4 bytes | `u32` (LE) | Executable ELF payload byte length |
| `0xB4 ..` | `2 * N` bytes | `[u16; N]` (LE) | Declared capability IDs |
| `0xB4 + 2 * N ..` | Variable | `[u8]` | Architecture-specific ELF binary payload |

---

## 2. Cryptographic Security & Verification Pipeline

The kernel validates every `.rapp` package strictly before memory allocation or execution:

### Invariant Checks
1. **Magic & Version**: File must begin with `b"RAPP"` and format version `2`.
2. **Architecture**: Target architecture code must match the running kernel (`RappArch`).
3. **Buffer Bounds**: Total file size must match `0xB4 + (capability_count * 2) + payload_len` without truncation or trailing garbage.
4. **Payload Digest**: The computed SHA-256 hash over the ELF payload bytes must match the header `payload_digest`.
5. **Publisher Trust**: The publisher public key is validated against the kernel trust root registry (`REVOCATION_REGISTRY`). Untrusted or revoked keys are rejected immediately.
6. **Signature Verification**:
   - The Ed25519 signature covers the entire binary with the 64-byte signature field (`0x70..0xB0`) zeroed out, followed by the capability ID array, followed by the ELF payload.
   - Unsigned packages, zeroed signatures, and tampered bytes are rejected prior to execution.

---

## 3. Capability Mapping

InterOS Core enforces zero ambient authority. The declared capability IDs in the RAPP header map directly into kernel capability table rights:

| Capability ID | Name | Permission Granted |
| :--- | :--- | :--- |
| `1` | `VFS_READ` | Read access to application-sandboxed filesystem |
| `2` | `VFS_WRITE` | Write access to application-sandboxed storage |
| `3` | `DISPLAY` | Framebuffer presentation and surface management |
| `4` | `TOUCH_INPUT` | Input event queue reception |
| `5` | `AUDIO_OUT` | Audio playback PCM channel access |
| `6` | `AUDIO_IN` | Microphone audio capture channel |
| `7` | `NETWORK` | TCP/UDP socket creation and network operations |
| `8` | `CELLULAR` | Telephony and SMS baseband service |
| `9` | `IPC_CHANNEL` | Dedicated bidirectional IPC endpoint with parent |
| `10 - 18` | `EXTENDED_*` | Subsystem-specific hardware resources |

---

## 4. Execution Lifecycle

When launching a `.rapp` package:

1. **VFS Ingestion**: The kernel opens `/packages/<package_id>.rapp` via descriptor.
2. **Validation**: `crate::package::parse_rapp` parses the header, checks trust and revocation, verifies the SHA-256 digest, and checks the Ed25519 signature.
3. **Capability Minting**: Declared capabilities are filtered and bound to a fresh per-process `CapabilityTable`.
4. **Ring-3 Process Spawning**:
   - A dedicated user address space is instantiated with independent page tables.
   - The ELF executable is mapped into user memory with `W^X` enforcement.
   - Isolated thread registers (`RIP`, `RSP`, `CR3` on x86_64 / `PC`, `SP_EL0`, `TTBR0_EL1` on aarch64) are staged and enqueued in the scheduler.
