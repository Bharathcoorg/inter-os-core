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

## Phase 2 package lifecycle

The QEMU developer transport now provides a real guest package lifecycle:

1. `interos trust <package.rapp>` explicitly provisions that package's publisher public key into the guest trust store.
2. `interos install <package.rapp>` verifies the RAPP on the host, transfers the exact signed bytes over QEMU COM1, verifies the RAPP again inside the guest, persists the package blob and activates its registry entry.
3. `interos run <package-id>` uses the guest package launcher to create a Ring-3 process from the persistent verified blob.
4. `interos uninstall <package-id>` revokes the active registry entry and removes the persistent package blob.

The UART transport is deliberately bounded to the kernel's development command protocol. It is a QEMU development bridge, not a replacement for a physical-device package transport.

The current persistent package store accepts RAPP blobs up to 60 KiB per package and is bounded by the kernel's persistent storage record capacity. Packages that exceed the configured store limits are rejected rather than truncated.

No host-side package catalog entry is treated as proof of guest installation. Device Studio reports guest transfer failures as installation failures.
