# Recovery and Secure Updates

InterOS Core is an open-source research prototype of a smartphone operating system, running in QEMU only. It is not a commercial product. External contributions are not accepted at this stage and will open after the first stable release.

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
- **Durable Confirmation Boundary**: The kernel confirms a boot only after all registered critical userspace services report `Running`, and the confirmation is journalled before the recovery state advances.
- **Durable Failure Semantics**: Recovery is fail-closed. If the journal cannot be read, is malformed, or a transition cannot be persisted, the kernel does not claim a healthy boot and leaves confirmation disabled.
- **Bounded Rollback Policy**: When a pending boot reaches `max_attempts = 3`, the recovery controller exposes `last_good` as the recovery slot. Selecting and switching the physical boot slot remains a platform bootloader/update-backend responsibility; the common kernel does not claim to perform that hardware-specific switch.

---

## 3. Platform & Emulation Reality Boundary

- **Kernel Policy vs. Storage Drivers**: The microkernel provides the transaction state machine, cryptographic validation, and recovery policy.
- **QEMU Verification**: There is no physical device, silicon, or over-the-air update deployment. Update transactions are verified strictly in virtual storage and QEMU developer targets.
