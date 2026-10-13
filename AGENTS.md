# InterOS Sovereign Smartphone Operating System — Agent Guidelines

Welcome to the **InterOS** (`inter-os-core`) project.

## Key Directives:
1. **The Platform Truth Contract**: Never fake hardware telemetry, never claim "subpixel AA" when the algorithm is 4-bit grayscale coverage AA, and never introduce mock drivers claiming to be physical silicon.
2. **Userspace Guarantees**: All 16 applications in `apps/` must remain `#![no_std]` and `#![no_main]` with zero heap allocations in interactive rendering and event loops.
3. **Panic & Fault Resilience**: Never take blocking `.lock()` calls in the panic handler (`kernel/src/main.rs`). Always use `.try_lock()`.
4. **Zero Ambient Authority**: Access to hardware, VFS, IPC, and keystore requires an explicit `CapHandle`.
5. **Continuous Verification**: Always run `python tests/run_all_contracts.py` and `cargo test --manifest-path kernel/Cargo.toml` before committing changes.
