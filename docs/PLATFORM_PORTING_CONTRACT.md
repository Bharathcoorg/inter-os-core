# InterOS Platform Porting Contract

InterOS separates kernel policy from board-specific hardware through explicit platform contracts.

---

## 1. Porting Layers

1. **Architecture**: CPU execution, interrupt entry, architectural timers, MMU/address space switching, and context save/restore.
2. **Board Descriptor**: CPU topology, memory discovery, interrupt routing, and peripheral discovery.
3. **Drivers**: Display scanout, block storage, USB, touch digitizer, audio, camera, cellular modem, wireless, and hardware sensors.
4. **Services**: Userspace policy and service supervision; services must consume normalized HAL abstractions rather than assuming board-specific hardware.
5. **Applications**: Native applications consume standard package, IPC, permission, and UI contracts.

---

## 2. Storage Abstraction

Filesystems consume the platform-neutral `BlockDevice` contract rather than driving controllers directly. A board port supplies the appropriate controller driver and maps controller faults to the bounded storage error ABI.

---

## 3. System Updates

The microkernel verifies update manifests, payload digests, signatures, and monotonic versions. Platform update backends are responsible for writing the inactive A/B slot and marking it pending. A board is not considered hardware-backed merely because a driver struct exists.

---

## 4. Reality Contract

In accordance with the [Platform Reality Contract](PLATFORM_REALITY_CONTRACT.md), hardware availability and verification levels must remain truthful:
- QEMU virtual devices are explicitly classified as `Emulated`.
- Unsupported hardware peripherals remain `Unavailable` until a verified physical driver backend is attached.
- Public platform documentation must describe only reproducible, public-source technical architectures.
