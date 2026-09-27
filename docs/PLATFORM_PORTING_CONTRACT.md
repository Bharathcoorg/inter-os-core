# InterOS Platform Porting Contract

InterOS separates kernel policy from board-specific hardware through explicit platform contracts.

## Porting layers

1. **Architecture** — CPU execution, interrupt entry, timers, MMU/address spaces and context switching.
2. **Board descriptor** — CPU count, memory discovery, timer and controller availability.
3. **Drivers** — display, storage, USB, touch/input, audio, camera, modem, Wi-Fi, Bluetooth and sensors.
4. **Services** — userspace policy and service lifecycle; services must not assume a specific board.
5. **Applications** — consume stable package, IPC, permission and UI contracts.

## Storage

Filesystems consume the platform-neutral BlockDevice contract, not ATA/SDHCI directly. A board port supplies the appropriate block backend and maps controller failures to the bounded storage error contract.

## Updates

The kernel verifies update manifests, payload digests, signatures and monotonic versions. A platform update backend is responsible for writing and verifying the inactive slot and activating it. No board is considered hardware-backed merely because a driver type exists.

## Reality contract

Hardware availability and verification level must remain truthful. QEMU/emulated devices are explicitly marked as emulated; unsupported phone peripherals remain unavailable until a real backend exists.

## Public-source rule

Board ports and examples belong in public platform code only when they are reproducible and free of private infrastructure, credentials, internal services or Agentic OS implementation details.
