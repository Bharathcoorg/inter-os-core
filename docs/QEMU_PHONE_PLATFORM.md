# InterOS QEMU Phone Platform Contract

This document defines the canonical x86_64 developer-demo platform for InterOS Core.

## Purpose

The QEMU Phone profile is a reproducible virtual development target. It is not a claim of physical smartphone hardware equivalence.

## Canonical machine

- Architecture: x86_64
- Rust target: x86_64-unknown-none
- QEMU machine: q35
- CPU: qemu64
- RAM: 1024 MiB
- Boot: BIOS disk image
- Disk image: target/inter-os-bios.img
- Serial TCP: 127.0.0.1:4301
- HMP monitor: 127.0.0.1:44445
- VNC: 127.0.0.1:5901 (display :1)
- Device Studio WebSocket bridge: 127.0.0.1:6081
- Guest scanout contract: 1280x720 RGB/BGR-compatible framebuffer as reported by the guest
- Persistent disk: the boot image is the canonical system artifact; mutable developer state is kept outside the kernel build tree until a guest storage backend is attached.

## Canonical developer flow

1. `python tools/interos.py doctor`
2. `python tools/interos.py build demo`
3. `python tools/interos.py run qemu`
4. Open Device Studio at `http://127.0.0.1:4300`.
5. Use `interos logs -f` for the authoritative guest UART stream.

`interos build demo` is the only supported clean-checkout build entry point for the x86_64 QEMU demo. It resolves userspace dependencies before compiling the kernel and boot image.

## Artifact rule

Every ELF referenced by `kernel/src/task/images.rs` must exist under `target/x86_64-unknown-none/debug/` before the kernel is compiled. The manifest in `platform/x86_64/userspace-manifest.json` is the authoritative list.

A missing artifact is a hard build error. The build must never manufacture an empty, placeholder, or synthetic ELF.

## Display/input boundary

Device Studio exposes the real QEMU VNC framebuffer through websockify. Browser pointer/keyboard events are sent to the VNC session. This Phase 1 contract establishes the transport boundary; a native virtual touchscreen driver is a later platform phase and must not be represented as present until the guest consumes it.

## Verification language

- "built" means the artifact was produced by the canonical build graph.
- "QEMU-ready" means the complete x86_64 artifact graph and QEMU profile are available.
- "hardware-ready" is reserved for a physical board with verified drivers.

The profile deliberately separates those claims.
