# InterOS Core Repository Engineering Rules & Invariants

These rules govern all engineering work on `inter-os-core`. Every autonomous agent and developer must adhere to these rules without exception.

---

## 1. The Platform Truth Contract (Absolute Invariant)
- **Zero Fabricated Telemetry**: Never synthesize fake sensor readings, battery percentages, volume levels, or peripheral states. If hardware is unbound or disconnected, report `UNBOUND`, `NONE`, or return an explicit refusal.
- **Accurate Algorithmic Claims**: Describe rendering algorithms by their mathematical reality (e.g., "4-bit grayscale coverage anti-aliasing", NOT "subpixel anti-aliasing"). Describe compositors as "software KMS-style compositor" unless running over live VirtIO-GPU VirGL 3D scanout.
- **Fail-Closed Security**: When capabilities, permissions, or signatures fail validation, fail immediately and loudly. Never silently fall back to permissive mode.

---

## 2. Zero-Heap Allocation Invariant for Userspace Apps
- All 16 ring-3 applications (`apps/`) must compile under `#![no_std]` and `#![no_main]`.
- Interactive render loops, event handlers, and data stores must use statically bounded buffers, fixed-size stack arrays, or BSS storage. Dynamic heap allocation (`alloc::*`) in ring-3 interactive loops is strictly forbidden.
- Maximum package size and execution limits must be respected (`RAPP_HEADER_SIZE = 0xB4`, max blob size bounds).

---

## 3. Panic & Fault Resilience Rules
- **Non-Blocking Panic Handler**: `panic_handler` in `kernel/src/main.rs` must NEVER take unconditional spinlocks (`.lock()`). Any lock accessed during panic diagnostic capture must use `.try_lock()`.
- **IST Stack Segregation**: Hardware double-fault handlers must execute on a dedicated Interrupt Stack Table (IST) stack (`DOUBLE_FAULT_IST_INDEX`) without dynamic allocation.
- **Watchdog Liveness**: Watchdogs must be guarded against spinlock deadlocks. The system must reboot (`PanicPolicy::Reboot`) when unrecoverable faults occur in production.

---

## 4. Capability-First Security (Zero Ambient Authority)
- Resource access (VFS, IPC, Framebuffer, Network, Telephony, Audio, Keystore) must be gated by explicit `CapHandle` leases in the task's `CapabilityTable`.
- Processes running under a UID cannot access resources of another UID without a delegated capability token.
- Unauthenticated transports (such as raw COM1 UART) must NEVER be permitted to install publisher roots of trust (`RAPP_TRUST`).

---

## 5. Continuous Verification Gate Before Pushing
No commit may be pushed to `origin/master` without passing:
1. `python tests/run_all_contracts.py` (all 317 contract test cases must pass).
2. `cargo test --manifest-path kernel/Cargo.toml` (all 234 kernel unit tests must pass).
3. `cargo test -p interos-std` (all 77 SDK unit tests must pass).
4. `cargo test -p launcher` (all 64 launcher unit tests must pass).
5. Application host test suites (`cargo test -p messages`, `cargo test -p calculator`, `cargo test -p file_manager`, `cargo test -p dialer`, `cargo test -p terminal`).
6. `cargo check --workspace` (all 19 workspace crates compile cleanly).
7. Multi-architecture QEMU suites: `tests/test_aarch64.ps1` and `tests/test_riscv64.ps1` exit 0.

---

## 6. Continuous Improvement Priority Plan (Argon 4 Audit)
- **P0**: Deadlock-free panic state capture, atomic surface commit latches (prevent tearing), framebuffer write-combining (PAT/MAIR).
- **P1**: Shared-memory dual-mapped zero-copy IPC and surface buffers, VirtIO-GPU PCI 2D/3D driver integration.
- **P2**: Cryptographic TRNG everywhere (no static IVs), 64-bit decimal fixed-point calculator math, live VFS directory rendering in file manager, GGUF local AI daemon architecture.
