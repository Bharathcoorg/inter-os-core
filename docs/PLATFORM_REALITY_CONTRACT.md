# Platform Reality Contract

InterOS Core separates implementation from verification. A subsystem being present in source does not by itself mean that hardware exists or that the feature has been verified on physical hardware.

## Runtime states

Every hardware-facing subsystem must distinguish:

- **Unavailable** — no backend is attached; user-visible telemetry must not invent values.
- **Emulated** — a QEMU or other virtual backend supplies the behavior.
- **Present** — a real hardware backend is attached and has supplied telemetry.

Verification is similarly explicit:

- Source-only
- Unit-tested
- QEMU-verified
- Hardware-verified

These states are independent. QEMU verification is not physical-hardware verification.

## Kernel contract

The platform contract module provides the common contract used during kernel bootstrap. The current x86_64 runtime contract is explicitly QEMU/emulated and never upgrades itself to hardware-backed status.

Persistent state, signed packages, and isolated userspace are required platform invariants.

## UI contract

Consumer UI must not display fixed demonstration values as live device state. Clock, location, battery, radio, sensor, camera, modem, and similar values must come from an authoritative service/backend or be rendered as unavailable.

The launcher therefore no longer embeds a fixed clock, location, or synthetic battery percentage.

## Evidence policy

Device Studio and developer tooling must report the source of a value. A missing backend is represented as unavailable rather than a plausible fixture. Hardware claims require hardware-specific evidence.

This keeps the public InterOS Core repository honest while allowing platform-specific bring-up work to evolve independently.
