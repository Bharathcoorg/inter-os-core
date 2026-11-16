# InterOS Core — Storage, Persistence & Crash Recovery Adversarial Verification

**Document:** `docs/audits/STORAGE_AND_RECOVERY_VERIFICATION.md`  
**Repository:** `Bharathcoorg/inter-os-core`  
**Audit Baseline Commit:** `a38f8d14dea627b47d5d8ce74b73a83920888fc6`  
**Classification Standard:** The Platform Truth Contract  
**Lead Auditor:** Google Gemini 4 Hydrogen (`gemini-4-hydrogen`)  

---

## 1. Executive Summary

This audit independently investigates the storage stack of InterOS Core, including the Virtual File System (`kernel/src/fs/vfs.rs`), the block device abstraction (`kernel/src/storage/`), the ATA IDE driver (`kernel/src/drivers/ata.rs`), and the crash-recovery / journal engine (`kernel/src/storage/interfs.rs`).

### Core Audit Findings:
1. **Architectural Separation (In-Memory vs Block-Backed Storage):**
   - **In-Memory VFS:** Files outside `/data/` (or before `/data` block device initialization) exist as an in-memory capability-governed `BTreeMap` hierarchy.
   - **Block-Backed InterFS:** The `/data` mount point is backed by a genuine raw block device (`ATA_PRIMARY`, PIIX3 IDE unit 1, Primary Slave disk). Writes to `/data/*` write through to disk sectors and are flushed via `vfs.sync()`.
2. **Non-Snapshot Persistence Verified:**
   - Previous audit reports flagged a potential false positive where `test_reboot_recovery.py` ran with `-snapshot` and noted persistence as not implemented.
   - We updated `test_reboot_recovery.py` and verified `test_persistence.py`: raw 64 MiB disk images attached without snapshot reliably preserve file payloads across completely independent QEMU processes.
3. **Crash Consistency & Atomic Rollback Proven:**
   - `test_storage_recovery.py` verified three adversarial crash scenarios:
     1. Abrupt power loss (SIGKILL during operation): committed baseline intact, FSCK clean.
     2. Incomplete transaction rollback: injected uncommitted `IFTX` journal transaction rolled back to previous consistent generation on reboot.
     3. Corrupted journal slot fallback: damaged journal slot rejected, previous valid generation restored.

---

## 2. Storage Subsystem Architecture

```mermaid
flowchart TD
    App["Userspace Application (Ring 3)"] -->|CapHandle(Storage) / sys_open| VFS["VFS Manager (fs/vfs.rs)<br/>Capability-Governed BTreeMap"]
    VFS -->|"/data/* Mount Point"| InterFS["InterFS (storage/interfs.rs)<br/>Inode Table, Extents, Journal"]
    InterFS -->|"Metadata Journal (IFTX)<br/>Superblock v1 / v2 Generations"| ATA["ATA IDE Driver (drivers/ata.rs)<br/>ATA_PRIMARY (Bus 0, Unit 1)"]
    ATA -->|"Sector R/W (PIO 28-bit / 48-bit LBA)<br/>Port 0x01F0 - 0x01F7"| DISK["QEMU IDE Hard Disk Image<br/>(-device ide-hd,bus=ide.0,unit=1,drive=datad)"]
```

### 2.1 Disk Layout & InterFS On-Disk Structure

In `kernel/src/storage/filesystem.rs` and `kernel/src/storage/interfs.rs`:

```
+-----------------------------------------------------------------------------------+
| LBA 0 .. 1: MBR / Partition Table & Reserved Boot Code                             |
| LBA 2 .. 3: Superblock Slot 0 (Magic: 'IFSB', Generation, CRC32, Inode Root)     |
| LBA 4 .. 5: Superblock Slot 1 (Alternating Double-Buffered Commit Slot)           |
| LBA 6 .. 15: Journal Header & Transaction Slots ('IFTX', Opcode, LBA, Data)       |
| LBA 16 .. 255: Inode Table (64-byte Inode Descriptors, Mode, Size, Extents, CRC32)|
| LBA 256 .. 511: Block Allocation Bitmap (1 bit per 4096-byte cluster)             |
| LBA 512 .. End: Data Clusters (Extent-allocated storage for file payloads)        |
+-----------------------------------------------------------------------------------+
```

#### Key Invariants:
- **Double-Buffered Superblock:** Commit alternates between Slot 0 and Slot 1. The generation counter and CRC32 are updated atomically. If power fails while writing Slot 1, Slot 0 remains valid.
- **Journal Transaction Log:** Uncommitted transactions carry the magic `IFTX`. On mount, `InterFs::mount_region` checks journal slots. If a transaction was not committed before crash, it is discarded and the disk rolls back to the latest valid superblock generation.
- **Data Extents:** Large files allocate contiguous extents, supporting multi-megabyte payloads without fragmentation.

---

## 3. Adversarial Recovery & Persistence Verification

### 3.1 Non-Snapshot Cross-Process Persistence (`test_persistence.py`)

- **Execution Command:** `python tests/test_persistence.py`
- **Methodology:**
  1. Created 64 MiB raw disk image `target/test_persistence.img`.
  2. Attached to QEMU via:
     ```
     -drive file=target/test_persistence.img,format=raw,if=none,id=datad
     -device ide-hd,bus=ide.0,unit=1,drive=datad
     ```
     *(Notice: No `-snapshot` flag)*.
  3. **Session 1:** Cold boot, InterFS formatted and mounted on `/data`, wrote `/data/token.txt` (`PERSISTENCE_TOKEN_ALPHA_77492`) and `/data/config.json`, verified immediate readback, dispatched `SHUTDOWN`, verified clean QEMU process exit with code 0.
  4. **Session 2:** Fresh QEMU process started with the identical disk image. Read `/data/token.txt` and `/data/config.json`: byte-for-byte identical. Verified `FSCK` status `CLEAN`. Updated `/data/config.json` with generation v2, clean shutdown.
  5. **Session 3:** Fresh QEMU process started. Read updated `/data/config.json`: verified v2 persisted. Exercised security boundary: negative tests confirmed path traversal (`/data/../etc/passwd`) and non-`/data` ambient writes were rejected. Verified `FSCK` clean.
- **Result:** `[SUCCESS] Cross-Process Persistent Block Storage FULLY VERIFIED!`.

### 3.2 Power-Loss & Crash Recovery (`test_storage_recovery.py`)

- **Execution Command:** `python tests/test_storage_recovery.py`
- **Scenarios Tested:**
  1. **Abrupt SIGKILL Simulation:** Wrote baseline data, performed sudden SIGKILL on QEMU process during active execution. On subsequent boot, InterFS mounted cleanly and `FSCK` verified 0 orphan clusters and 0 corrupted inodes.
  2. **Incomplete Transaction Rollback:** Manually injected an uncommitted `IFTX` journal transaction into the raw disk image superblock. On boot, the recovery engine detected the uncommitted transaction, rolled back to the previous valid generation, and kept baseline files intact.
  3. **Corrupted Journal Slot Fallback:** Deliberately corrupted journal slot 0 CRC32. On boot, the filesystem rejected slot 0, successfully fell back to the older valid generation, and preserved file contents.
- **Result:** `[SUCCESS] All Crash, Transaction Rollback & Journal Recovery Tests PASSED!`.

### 3.3 Reboot Recovery & Application State (`test_reboot_recovery.py`)

- **Execution Command:** `python tests/test_reboot_recovery.py`
- **Methodology:**
  1. Attached dedicated raw persistent disk `target/recovery_storage.img` without snapshot to both Session 1 and Session 2.
  2. Session 1: Cold boot, Calculator app launched and evaluated $(7 + 5 = 12)$, wrote `/data/reboot_token.txt` (`REBOOT_DURABLE_TOKEN_ABC123_PROOF`), flushed VFS buffers via `SYNC`, confirmed `[FSCK] status=CLEAN`, dispatched clean ACPI `SHUTDOWN`. QEMU exited with code 0.
  3. Session 2: Booted fresh QEMU process with same disk. Settings app launched, tab switched, navigated back to Home. Read `/data/reboot_token.txt`: exact string `REBOOT_DURABLE_TOKEN_ABC123_PROOF` restored. Verified `[FSCK] status=CLEAN`. Clean shutdown with code 0.
- **Report Output:** `target/recovery_report.json` records `persistence_status: "VERIFIED - REAL BLOCK-BACKED InterFS DURABLE STORAGE"`.

---

## 4. Storage Security & Isolation Audit

### 4.1 Zero Ambient Authority
- In `kernel/src/fs/vfs.rs`, any task attempting to open or write files without a valid `CapabilityType::FileSystem` or `CapabilityType::Storage` is denied (`SyscallResult::AccessDenied`).
- Applications are restricted to their package sandbox (e.g. `/data/app/<package_id>/`).

### 4.2 Path Traversal Hardening
- In `kernel/src/serial.rs` and `kernel/src/fs/vfs.rs`:
  - Paths containing `..`, `//`, or null bytes are rejected upfront with `status=ERR_REJECTED`.
  - Absolute paths must resolve within mounted root or `/data/`.

---

## 5. Evidence Classification Summary

| Component | Audit Classification | Evidence |
| :--- | :--- | :--- |
| **In-Memory VFS Hierarchy** | `UNIT TESTED` & `QEMU VERIFIED` | Unit tests in `vfs.rs`; live runtime in QEMU. |
| **ATA Primary Slave IDE Driver** | `QEMU VERIFIED` | Sector read/write on PIIX3 IDE unit 1 in QEMU. |
| **InterFS On-Disk File System** | `QEMU VERIFIED` | Formatted, mounted, and checksummed on raw block storage. |
| **Non-Snapshot Persistence** | `QEMU VERIFIED` | Proven across fresh QEMU instances (`test_persistence.py`). |
| **Crash & Transaction Recovery** | `QEMU VERIFIED` | SIGKILL and injected `IFTX` rollback proven (`test_storage_recovery.py`). |
| **UFS 3.0 Hardware Storage** | `NOT IMPLEMENTED` | Physical UFS host controller deferred; RAM boot required. |
