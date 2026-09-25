# Phase 3 & 4 Runtime Contract

## Phase 3 — User experience and live data

InterOS Core now treats the launcher, system line, window registry and service supervisor as one runtime state graph.

- Launcher consumes the kernel package, window and service registries.
- System line compositor consumes live scheduler/window/service state.
- Touch gestures are routed to the topmost focused surface.
- Native applications share semantic design tokens from the interos_std design module.
- File Manager obtains storage capacity and /data directory entries from live block/VFS services rather than embedded capacity or file tables.
- Calendar derives its civil date from the RTC response.
- Settings power-profile changes are persisted through the kernel settings service.

## Phase 4 — Platform services and QEMU contract

- Persistent settings use journal kind 3, isolated from package and revocation metadata.
- The journal now reserves independent slot pairs for four record classes.
- Settings Get/Set are capability checked at the syscall boundary.
- Storage capacity and VFS directory listing are capability-controlled.
- Window geometry/focus, service lifecycle and settings are exposed through the native Rust SDK.
- CI validates the x86_64 QEMU kernel target and boot-image builder.

### QEMU runtime

The x86_64 boot image is produced by:

    python tools/interos.py build --target x86_64

Then boot with:

    qemu-system-x86_64 -drive format=raw,file=target/inter-os-bios.img -serial stdio -display none

The CI build contract intentionally uses cargo check for the bare-metal kernel: current nightly LLVM code generation has produced a sha2 backend failure during full x86_64 image compilation, while the same target passes compilation checks. CI therefore does not claim a runtime boot smoke test until code generation is stable.
