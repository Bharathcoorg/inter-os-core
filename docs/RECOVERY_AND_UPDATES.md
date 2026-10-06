# Recovery and Secure Updates

InterOS Core treats operating system updates as atomic, authenticated transactions rather than raw flash overwrites (`kernel/src/update.rs`, `kernel/src/recovery.rs`).

---

## 1. Update Transaction Flow

```text
  [Idle] ──> [Staged] ──> [Verified] ──> [Committed] ──> [Pending Boot]
               │             │                 │
               ▼             ▼                 ▼
          [Rejected]    [Rejected]         [Rollback]
```

1. **Update Manifest Construction**: The update is described by an immutable `UpdateManifest`:
   - `format_version`: Format identifier (`1`).
   - `version`: Monotonically increasing 64-bit integer version counter.
   - `payload_len`: Exact byte length of the staged payload.
   - `payload_sha256`: SHA-256 cryptographic digest of the payload.
2. **Monotonicity & Format Validation**:
   - Rejects version numbers less than or equal to the currently installed version (`UpdateError::Downgrade`).
   - Rejects empty payloads, format version mismatches, and length mismatches.
3. **Payload Digest Verification**: Computes the SHA-256 hash over the staged payload buffer and asserts exact equality with `payload_sha256`.
4. **Publisher Attestation**: Verifies the Ed25519 digital signature over the 50-byte canonical manifest bytes (`format_version` + `version` + `payload_len` + `payload_sha256`) against the trusted InterOS publisher public key.
5. **A/B Slot Staging**: Stages the verified image into the inactive boot slot (`BootSlot::A` or `BootSlot::B`).
6. **Persistent Commitment**: Writes the pending activation record to persistent storage before rebooting.

---

## 2. Boot Health & Recovery Policy

The boot recovery controller manages slot state transitions across system reboots:

- **Dual-Slot Structure**: `BootSlot::A` and `BootSlot::B`.
- **Boot States**: `Unknown`, `Pending`, `Success`, `Failed`.
- **Boot Attempt Budget**: Tracks boot attempts with a maximum retry ceiling (`max_attempts = 3`).
- **Confirmation Boundary**: Upon reaching a healthy userspace state (PID 1 and launcher initialized), the system marks `BootOutcome::Success` and updates `last_good = active`.
- **Automatic Rollback**: If the new slot fails to boot or crashes before confirmation (`attempts >= 3`), the bootloader deterministically selects `last_good`, rolling back to the previous known-healthy system partition.

---

## 3. Platform & Hardware Reality Boundary

- **Kernel Policy vs. Storage Drivers**: The microkernel provides the transaction state machine, cryptographic validation, and recovery policy. Physical block writing depends on platform-specific flash/block drivers.
- **Physical Device Status**: Production over-the-air (OTA) updates on consumer smartphones are **not** currently deployed. Update transactions are currently verified in virtual storage and QEMU developer targets.
