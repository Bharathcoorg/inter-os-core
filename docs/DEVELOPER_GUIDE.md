# InterOS Developer Guide

## Core workflow

Use the interos CLI as the primary developer entry point:

- interos doctor — inspect the development environment.
- interos build --target <arch> — build a target.
- interos test --target <suite> — run the relevant test suite.
- interos new <name> — scaffold an application.
- interos package <name> — create a .rapp package.
- interos verify <package> — verify package integrity.
- interos run <name> — deploy through Device Studio.
- interos snapshot — manage QEMU snapshots.
- interos trace — inspect live guest telemetry.

## Reliability workflow

Before hardware bring-up:

1. Run environment diagnostics.
2. Exercise the target in QEMU.
3. Inspect serial output and guest telemetry.
4. Use snapshots for repeatable recovery scenarios.
5. Run adversarial and contract tests.
6. Only then move to a physical target with the explicit flashing safety gates.

The public core must describe only capabilities that are actually implemented and verified.
