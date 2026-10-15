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

Declared capability IDs map into per-process capability token table rights and object scoping:

| Capability ID | Capability Type | Default Rights | Object Scope / Target | Description |
| :--- | :--- | :--- | :--- | :--- |
| `1` | `SerialOutput` | `WRITE` | `0` (Global) | Serial logging and COM port character output |
| `2` | `FrameBufferDirect` | `WRITE` | `0` (Global) | Direct framebuffer pixel drawing and display hardware presentation |
| `3` | `TimerTickRead` | `READ` | `0` (Global) | High-precision system timer tick polling |
| `4` | `HardwarePortIo` | `READ \| WRITE` | `0` (Global) | Direct port I/O hardware bus access |
| `5` | `IpcSend` | `WRITE` | IPC Target Endpoint | Send messages through dedicated IPC channel |
| `6` | `IpcReceive` | `READ` | IPC Target Endpoint | Receive messages through dedicated IPC channel |
| `7` | `ProcessSpawn` | `INVOKE` | `0` (Global) | Spawn child user processes and containers |
| `8` | `ProcessKill` | `INVOKE` | `0` (Global) | Terminate owned child processes |
| `9` | `FileSystemRead` | `READ` | `0` (Sandboxed) | Read files within sandboxed VFS mount |
| `10` | `FileSystemWrite` | `WRITE` | `0` (Sandboxed) | Write/persist files within sandboxed VFS mount |
| `11` | `StorageRawBlock` | `READ \| WRITE` | `0` (Global) | Direct block-level ATA/NVMe storage operations |
| `12` | `NetworkSocket` | `WRITE` | `0` (Global) | Create UDP/TCP transport sockets |
| `13` | `NetworkSend` | `WRITE` | `0` (Global) | Transmit network packets |
| `14` | `NetworkReceive` | `READ` | `0` (Global) | Receive network packets |
| `15` | `ShmProducer` | `WRITE` | `0` (Global) | Shared memory ring-buffer producer |
| `16` | `ShmConsumer` | `READ` | `0` (Global) | Shared memory ring-buffer consumer |
| `17` | `EventPoll` | `READ \| INVOKE` | `0` (Global) | Poll input and event queues |
| `18` | `SystemControl` | `INVOKE` | `0` (Global) | Global system administration and power management |
| `19` | `SystemControl` | `INVOKE` | `SERVICE_POWER_CONTROL_OBJECT` | Scoped power governor and battery telemetry |
| `20` | `SystemControl` | `INVOKE` | `SERVICE_TELEPHONY_CONTROL_OBJECT` | Scoped cellular modem, SMS, and dialer control |
| `21` | `SystemControl` | `INVOKE` | `SERVICE_AUDIO_CONTROL_OBJECT` | Scoped hardware audio synthesis and playback control |
| `22` | `SystemControl` | `INVOKE` | `SERVICE_CAMERA_CONTROL_OBJECT` | Scoped camera sensor frame capture |
| `23` | `SystemControl` | `INVOKE` | `SERVICE_SENSOR_CONTROL_OBJECT` | Scoped IMU, accelerometer, and sensor reading |

### Capabilities that RAPP v2 does not define

These are called out explicitly so a publisher does not assume an ABI entry is
merely undocumented:

- **No hardware keystore, enclave, or secure-element capability exists.** Secret
  material for a package is persisted in that package's sandboxed VFS mount,
  which the kernel's storage vault encrypts at rest. That is ordinary
  `FileSystemWrite` (id `10`) to the package, not a separate authority, so a
  manifest cannot request enclave access and must not claim to have it.
- **No device-wide administration is grantable to a package.** Id `18` is
  reserved for the system supervisor: the kernel mints it only for a publisher
  key the trust store records as a system publisher, and the runtime gate
  additionally requires a kernel-assigned privileged process identity. Until a
  trusted provisioning path can install such a publisher key, no package can
  obtain id `18`, and any manifest that declares it is refused at install and at
  launch.

An alias or template in the tooling that resolves a name to one of these
capabilities must resolve it to the identifier that actually delivers what the
name promises. `tools/interos.py` enforces this with
`ALIAS_FORBIDDEN_CAPABILITY_IDS`, and `tests/test_rapp_capability_abi.py` pins
it against this table.
| `24` | `RealtimeClockRead` | `READ` | `0` (Global) | Read wall-clock hardware real-time clock (RTC) |

Values outside `1..=24` (such as `0` or `25+`) are strictly unknown and rejected during package verification (`RappError::UnknownCapability`).

---

## 4. Package Lifecycle in Userspace

1. **Scaffolding & Packaging**: Developers scaffold projects via `interos new` and bundle binaries via `interos package <name>`.
2. **Signing**: Packages are signed with the publisher's Ed25519 private key via `interos sign <package> --key <key_file>`.
3. **Trust Provisioning**: Trust roots are established via firmware-anchored keys or authenticated provisioning. Unauthenticated serial interfaces (COM1) explicitly reject dynamic root-of-trust injection (`RAPP_TRUST`) to prevent unauthorized UART privilege escalation.

   Platform-authority publisher identities are an explicit allowlist: the exact
   strings `system` and `platform`, plus the delimited namespace `org.interos.`.
   The trailing dot is part of the namespace, so `org.interos-evil` and
   `org.interosX` are not platform publishers.

   **Current state of provisioning.** `RevocationRegistry::trust_key` is the
   intended provisioning entry point and it journals the record, but nothing in
   this tree calls it: there is no firmware-anchored provisioning path, and no
   userspace syscall exists to grant a publisher root. The trust store is
   therefore populated only by `restore_trusted_keys` from a persistent record
   written previously. On a device with no such record, **no publisher is a
   platform publisher and capability 18 cannot be granted**. This is deliberate
   fail-closed behaviour and it is the current state, not a pending detail:
   until a trusted provisioning path is implemented, capability 18 must be
   treated as reserved rather than unavailable.
4. **Installation & Storage**: `interos install <package>` transfers the signed bytes to the guest over QEMU COM1, verifies integrity and signatures inside Ring 0 against the active trust store, and commits the package blob into persistent storage.
5. **Execution**: The launcher or PID 1 spawns an isolated Ring-3 child process with dedicated page tables (`W^X` enforced) and passes minted capability tokens.
