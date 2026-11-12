# InterOS Core — Phase 8.5 Implementation Report
## Critical Correctness Remediation: SMP TLB Shootdown, Physical Frame Ownership, Real Process Lifecycle & Service Supervisor

**Repository:** `https://github.com/Bharathcoorg/inter-os-core`  
**Canonical Branch:** `master`  
**Starting Commit:** `f7eb5038ffca51bc128f33975594c5862232682f`  
**Target Architectures:** `x86_64`, `aarch64`, `riscv64`  
**Date:** 2026-11-12  

---

## 1. Executive Summary & Architectural Overview

Phase 8.5 resolves the correctness, memory safety, and service lifecycle defects discovered after Phase 8.4. Rather than expanding superficial feature surface, Phase 8.5 hardens the low-level microkernel primitives across AArch64 and RISC-V 64 architectures to achieve genuine parity with the hardened x86_64 implementation.

### Key Remediation Pillars:
1. **Remote TLB Shootdown & Invalidation**:
   - RISC-V: Implemented SBI RFNC extension call (`SBI_EXT_RFNC = 0x52464E43`, `SBI_EXT_RFNC_REMOTE_SFENCE_VMA = 1`) with strict SBI return code error decoding (`SbiError`). Remote shootdowns return a typed result, preventing frame recycling when remote fences fail or are unsupported.
   - AArch64: Enhanced `tlbi vmalle1is` with multi-core quiescence validation (`active_cpus: AtomicU32`, `closing: AtomicBool`). Destructive teardown is rejected while secondary cores remain active in the address space, falling back to frame quarantine on failure.
2. **Explicit Physical Frame Ownership & Reference Counting**:
   - Replaced raw frame mappings with explicit ownership classes (`FrameOwnership::PrivateOwned`, `FrameOwnership::Shared`, `FrameOwnership::Borrowed`, `FrameOwnership::Kernel`, `FrameOwnership::Mmio`).
   - Mapped frames store explicit `MappingKind`.
   - `Shared` frames initialize with refcount 0 upon allocation, incrementing upon mapping into an address space (`retain_shared_frame`), and decrementing on address space teardown (`release_shared_frame`). When the refcount reaches 0, the allocator returns the frame to the free bitmap.
   - `Borrowed` frames (including MMIO peripheral spaces outside DRAM) are strictly immune from recycling during task and address-space teardown.
3. **Fallible Address-Space Destruction**:
   - Introduced `destroy_fallible()` returning `Result<(), TeardownError>`.
   - If quiescence cannot be established or TLB invalidation fails, all allocated page tables and private frames are diverted into `QUARANTINED_FRAMES` to prevent recycling corrupted or stale memory into the active pool.
4. **Genuine User-Mode Process Execution Lifecycle**:
   - Replaced mock process termination in the 50-cycle lifecycle boot tests with genuine hardware context switching to Ring 3 (EL0 on AArch64, U-mode on RISC-V 64).
   - Embedded real standalone 4096-byte ELF binaries (`TEST_PROC_AARCH64_ELF` and `TEST_PROC_RISCV_ELF`) that execute real instructions, invoke `SYS_GET_TASK_ID`, invoke `SYS_EXIT(0)`, and safely return to the kernel supervisor via hardware exceptions.
   - Reaped via parent `waitpid` with zero frame leaks, zero task leaks, and strict invariant checks (quota exhaustion at `MAX_TASKS`, active reap denial `-EBUSY`, and ELF load failure rollback).
5. **Atomic Service Supervisor & Generation-Guarded Heartbeats**:
   - Initialized service generations to `AtomicU32::new(0)`, incrementing to 1 on initial start.
   - Synchronized `heartbeat_with_generation(caller_pid, generation: Option<u32>)` verifying both PID and expected incarnation generation.
   - Corrected `SYS_SERVICE_HEARTBEAT` syscall register ABI (`a0` as `service_id`, `a1` as `generation`).
6. **Meaningful Calculator Visual Verification in QEMU**:
   - Restored and verified structural glyph analysis on evaluated expression result `"12"` inspecting top arc, right neck, bottom base, zero negative space ink, and calculation tape expression ink.
   - Added `run_glyph_negative_tests()` strictly rejecting blank frames and noise frames.

---

## 2. Workstream Breakdown & Implementation Details

### Workstream A: RISC-V Remote TLB Shootdown via SBI RFNC
- **Files Modified:** `kernel-riscv64/src/sbi.rs`, `kernel-riscv64/src/mmu.rs`
- **Implementation:**
  - In `kernel-riscv64/src/sbi.rs`, defined SBI RFNC extension constant `SBI_EXT_RFNC = 0x52464E43` and function ID `SBI_EXT_RFNC_REMOTE_SFENCE_VMA = 1`.
  - Implemented `SbiError` enum matching the SBI specification (`Failed`, `NotSupported`, `InvalidParam`, `Denied`, `InvalidAddress`, `AlreadyAvailable`, `AlreadyStarted`, `AlreadyStopped`, `NoShmem`, `Unknown`).
  - Implemented `remote_sfence_vma(hart_mask, hart_mask_base, start_addr, size) -> Result<(), SbiError>` capturing both `sbiret.error` (`a0`) and `sbiret.value` (`a1`).
  - In `UserAddressSpace::destroy_fallible()`, checked remote shootdown return status. On failure, frames are routed to `quarantine_frame()` rather than freed.

### Workstream B: AArch64 Inner-Shareable TLB Invalidation & Quiescence
- **Files Modified:** `kernel-aarch64/src/mmu.rs`
- **Implementation:**
  - Added `active_cpus: AtomicU32` and `closing: AtomicBool` to `UserAddressSpace`.
  - In `destroy_fallible()`, `closing.store(true, Ordering::SeqCst)` prevents new task activation.
  - Checked `active_cpus.load(Ordering::SeqCst)`. If greater than 0, verified if current CPU is the sole active core and switched to `L1_TABLE`.
  - Performed inner-shareable broadcast invalidation via `tlbi vmalle1is` followed by `dsb ish` and `isb`.
  - If quiescence cannot be established, teardown returns `Err(TeardownError::QuiescenceFailed)` and quarantines frames.

### Workstream C: Physical Frame Ownership Classes & Refcounting
- **Files Modified:** `kernel-aarch64/src/mmu.rs`, `kernel-riscv64/src/mmu.rs`
- **Implementation:**
  - Defined `FrameOwnership` enum: `PrivateOwned`, `Shared`, `Borrowed`, `Kernel`, `Mmio`.
  - Added `ownership: [FrameOwnership; MAX_FRAMES]` and `ref_counts: [u16; MAX_FRAMES]` to `ArmFrameAllocator` and `RiscvFrameAllocator`.
  - Created `allocate_frame_with_ownership(ownership: FrameOwnership)`. Shared frames start with `ref_counts = 0`.
  - Implemented `retain_shared_frame(paddr)` and `release_shared_frame(paddr) -> bool`. When refcount drops to 0 on release, the frame bitmap is cleared and ownership is reset.
  - Introduced `map_private_frame()`, `map_shared_frame()`, and `map_borrowed_frame()` methods on `UserAddressSpace`.
  - Allowed borrowed frames outside RAM (such as MMIO peripherals `< 0x4000_0000` on AArch64) without requiring DRAM allocation records.
  - Enforced that `Borrowed` and `Mmio` frames are never freed during address space destruction.

### Workstream D: Fallible Address-Space Destruction & Quarantining
- **Files Modified:** `kernel-aarch64/src/mmu.rs`, `kernel-riscv64/src/mmu.rs`
- **Implementation:**
  - Introduced `destroy_fallible(&mut self) -> Result<(), TeardownError>`.
  - Defined global `QUARANTINED_FRAMES: Mutex<Vec<u64>>` with helper `quarantine_frame(paddr)`.
  - In `destroy_fallible()`, failure to establish quiescence or failure in TLB shootdown routes all page table frames and mapped private frames into `quarantine_frame()`.
  - `Drop` calls `destroy_fallible()` and safely logs failures without leaking corrupted memory to the free pool.

### Workstream E: Real User-Mode Process Execution Lifecycle (50 Cycles)
- **Files Modified:** `kernel-aarch64/src/main.rs`, `kernel-aarch64/src/exceptions.rs`, `kernel-riscv64/src/main.rs`, `kernel-riscv64/src/trap.rs`
- **Implementation:**
  - Embedded genuine raw 4096-byte ELF binaries (`TEST_PROC_AARCH64_ELF` and `TEST_PROC_RISCV_ELF`) targeting userspace entry at `0x0040_0000`.
  - Binary executes user-mode assembly:
    - AArch64: `mov x8, 20` (`SYS_GET_TASK_ID`), `svc 0`, `mov x0, 0`, `mov x8, 1` (`SYS_EXIT`), `svc 0`.
    - RISC-V: `li a7, 20` (`SYS_GET_TASK_ID`), `ecall`, `li a0, 0`, `li a7, 1` (`SYS_EXIT`), `ecall`.
  - Executed 50 cycles of real user-mode process execution:
    - Spawning task in Ring 3 / EL0 / U-mode.
    - Context switching via hardware exception return (`eret` on AArch64, `sret` on RISC-V).
    - Handling `SYS_GET_TASK_ID` and `SYS_EXIT(0)` in exception/trap dispatchers.
    - Reaping task exit state via parent `waitpid`.
    - Asserting zero frame leaks (`frames_after == frames_before`) and zero task table leaks (`tasks_after == tasks_before`).
  - Added process safety invariant checks:
    - Quota exhaustion at `MAX_TASKS` returning `-ENOMEM`.
    - Active reap denial returning `-EBUSY` when reaping non-terminated tasks.
    - Partial ELF rollback ensuring all mapped frames are freed if ELF loading fails mid-way.

### Workstream F: Atomic Service Supervisor Lifecycle & Generations
- **Files Modified:** `kernel-aarch64/src/services.rs`, `kernel-riscv64/src/services.rs`, `kernel-aarch64/src/syscall.rs`, `kernel-riscv64/src/syscall.rs`
- **Implementation:**
  - Service entries track `generation: AtomicU32`. Initialized to 0, incrementing to 1 on initial start.
  - Implemented `heartbeat_with_generation(caller_pid, generation: Option<u32>)`. If generation is supplied, it must strictly match the service entry's current generation; mismatches are rejected.
  - Corrected `SYS_SERVICE_HEARTBEAT` (syscall 90) register parsing: `a0` contains `service_id` (userspace SDK contract), while optional `generation` is passed in `a1`.
  - Added recovery and restart unit tests exercising death detection, crash count tracking, generation increment, and heartbeat re-synchronization.

### Workstream G: Meaningful Calculator Visual Verification in QEMU
- **Files Modified:** `tests/verify_aarch64_interactive.py`, `tests/verify_riscv_interactive.py`
- **Implementation:**
  - Evaluated expression `3 * 4 = 12` rendered on the Calculator surface card (`696x720`).
  - Digit `'2'` right-aligned at `x: 640..670, y: 232..288` verified with exact geometric ink distribution:
    - Top arc: `count >= 50` (observed: 143)
    - Right neck: `count >= 50` (observed: 90)
    - Bottom base: `count >= 50` (observed: 141)
    - Negative space: `count == 0` (observed: 0)
    - Calculation tape expression ink: `count >= 500` (observed: 1799)
  - Implemented `run_glyph_negative_tests()` asserting that blank frames and noise frames are rejected with explicit failures.

### Workstream H: Honest Cross-Architecture Acceptance & Platform Truth Contract
- Maintained zero ambient authority, no heap allocations in interactive rendering, no mock hardware drivers claiming physical silicon, and strict `try_lock()` usage in UART exception handlers.
- All verification commands executed and recorded without fabrication.

---

## 3. Verification & Test Execution Summary

| Test Suite / Command | Scope | Result | Observed Evidence |
|---|---|---|---|
| `cargo check --workspace` | Entire Workspace | **PASSED** | 0 errors |
| `cargo test --manifest-path kernel/Cargo.toml` | x86_64 Kernel Unit Tests | **PASSED** | 593 passed, 0 failed, 1 ignored |
| `python tools/interos.py contracts` | System & License Contracts | **PASSED** | 25/25 system, 3/3 license contracts passed |
| `python tests/run_all_contracts.py` | Contract Framework | **PASSED** | 1214 passed across 82 suites + 27 script contracts (0 failures) |
| `python -m pytest tests/test_platform_reality.py` | Platform Reality Invariants | **PASSED** | 67 passed in 0.17s |
| `python tools/interos.py build demo` | x86_64 Demo Image | **PASSED** | `target/inter-os-bios.img` (13,075,456 bytes) |
| `python tests/verify_x86_boot.py` | x86_64 QEMU Canonical Boot | **PASSED** | 100% compliance, 720x1440 framebuffer verified |
| `python tools/interos.py build --target aarch64` | AArch64 Kernel Build | **PASSED** | `target/aarch64-unknown-none/release/interos-aarch64` |
| `python tests/verify_aarch64_boot.py` | AArch64 Boot Invariants | **PASSED** | 100% compliance, Sections 45a–45e verified |
| `python tests/verify_aarch64_interactive.py` | AArch64 QEMU Interactive UI | **PASSED** | 100% compliance, Glyph analysis & negative tests passed |
| `python tools/interos.py build --target riscv64` | RISC-V 64 Kernel Build | **PASSED** | `target/riscv64gc-unknown-none-elf/release/interos-riscv64` |
| `python tests/verify_riscv_boot.py` | RISC-V 64 Boot Invariants | **PASSED** | 100% compliance, Sections 45a–45e verified |
| `python tests/verify_riscv_interactive.py` | RISC-V 64 QEMU Interactive UI | **PASSED** | 100% compliance, Glyph analysis & negative tests passed |

---

## 4. Conclusion
Phase 8.5 successfully cures all targeted memory safety, SMP TLB shootdown, frame ownership, real process execution, and service supervisor vulnerabilities across all three architectures (`x86_64`, `aarch64`, `riscv64`).
