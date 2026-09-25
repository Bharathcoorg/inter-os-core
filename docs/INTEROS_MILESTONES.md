# InterOS Development Milestones

Canonical branch: `master`

This roadmap tracks the remaining work in dependency order. A milestone is **complete only when its implementation, tests, and relevant CI/QEMU evidence are green**.

## Foundation — current

| ID | Milestone | Status |
|---|---|---|
| M01 | Canonical master consolidation and branch policy | ✅ Complete |
| M02 | Multi-architecture kernel compile baseline | ✅ Complete |
| M03 | Memory mapping / frame reuse hardening | ✅ Complete |
| M04 | Capability authorization and object scoping | ✅ Complete |
| M05 | RAPP v2 package trust boundary | ✅ Complete |
| M06 | Persistent trust roots / revocation primitives | ✅ Complete |
| M07 | Remove kernel hardcoded app/service startup | ✅ Complete |
| M08 | PID1 Ring-3 bootstrap | ✅ Complete |
| M09 | PID1-owned service lifecycle ABI | ✅ Complete |
| M10 | Service supervision, recovery and backoff | ✅ Complete |

## Phase B — process and service architecture

| ID | Milestone | Status |
|---|---|---|
| M11 | Lifecycle unit/integration tests | ✅ Complete |
| M12 | QEMU PID1/service lifecycle assertions | 🟡 In progress — implementation complete; CI evidence pending |
| M13 | Service parent/reaping invariants | 🟡 In progress — implementation + contract tests complete; CI evidence pending |
| M14 | Service capability isolation tests | 🟡 In progress — implementation + contract tests complete; CI evidence pending |
| M15 | Trusted userspace service ABI | 🟡 In progress — implementation complete; CI evidence pending |
| M16 | Signed service package loading | 🟡 Implementation complete — verified RAPP execution handoff is lifecycle-owned; CI evidence pending |
| M17 | Userspace service manager replaces kernel workers | 🟡 Implementation complete — Ring-3 services, PID1 ownership, and verified-package lifecycle integration; CI evidence pending |
| M18 | Service crash/restart end-to-end validation | 🟡 Implementation complete — deterministic CI fault injection + PID1 recovery path; QEMU evidence pending |

## Storage

| ID | Milestone | Status |
|---|---|---|
| M19 | Persistent block-device abstraction | 🟡 Implementation complete — ATA + SDHCI share the sector-level `BlockDevice` boundary; persistent journal default API fixed; CI evidence pending |
| M20 | Real filesystem/storage backend | 🟡 Implementation complete — bounded block-backed InterFS is mounted before VFS, `/data` persistence uses it, region/bitmap/zero-length/corruption paths are hardened, and storage contracts are updated; CI/QEMU evidence pending |
| M21 | Persistent VFS metadata | 🟡 Implementation complete — mutable `/data` inode metadata, stable inode IDs, parent/child reconstruction, and metadata restore-before-payload flow are implemented; CI evidence pending |
| M22 | Atomic writes and crash recovery | 🟡 Implementation complete — InterFS create/write/delete operations publish a checksummed transaction intent before inode publication, recover unfinished transactions during mount, and clear the intent after metadata completion; CI/fault-injection evidence pending |
| M23 | Encrypted storage boundary | 🟡 Implementation complete — persistent device storage root is provisioned through the keystore, mutable VFS metadata/payloads use authenticated ENC1 encryption at rest, and legacy plaintext snapshots migrate on restore; CI evidence pending |
| M24 | Storage service userspace implementation | 🟡 Implementation complete — `storaged` now runs a dedicated Ring-3 storage loop, heartbeats through the lifecycle ABI, performs filesystem I/O through its scoped FileSystem capability, and no longer uses only the generic service loop; CI/QEMU evidence pending |

## Graphics and input

| ID | Milestone | Status |
|---|---|---|
| M25 | Framebuffer/display driver boundary | 🟡 Implementation complete — neutral `DisplayDriver`/KMS boundary, display-service-only framebuffer capability, surface write/destroy ABI, and contract tests added; CI evidence pending |
| M26 | Input device event pipeline | 🟡 Implementation complete — normalized touch/gesture event queue, bounded userspace event ABI, dedicated `inputd`, and contract tests added; CI/QEMU evidence pending |
| M27 | Compositor protocol | 🟡 Implementation complete — versioned bounded `ICMP` request/response protocol, kernel/SDK definitions, and CI contract added; runtime IPC evidence pending |
| M28 | Window/surface management | 🟡 Implementation complete — surface geometry/focus operations are routed through the display-service capability boundary and ownership contracts updated; CI/runtime evidence pending |
| M29 | Touch/gesture input integration | 🟡 Implementation complete — normalized touch events now route to the focused surface owner over bounded IPC; contract evidence pending |
| M30 | Hardware-backed graphics path | 🟡 Implementation complete — KMS page flips now publish the composed scanout to the bootloader framebuffer with bounded, aspect-preserving presentation; CI/QEMU evidence pending |

## Networking and smartphone services

| ID | Milestone | Status |
|---|---|---|
| M31 | Network device/driver abstraction | 🟡 Implementation complete — protocol stack now consumes a canonical kernel network-device manager with explicit device identity, MTU/link state, and loopback backend; CI evidence pending |
| M32 | Userspace network service | ⬜ Pending |
| M33 | TCP/IP + DNS + TLS integration | ⬜ Pending |
| M34 | Audio service and device pipeline | ⬜ Pending |
| M35 | Camera sensor/CSI pipeline | ⬜ Pending |
| M36 | Modem/RIL service boundary | ⬜ Pending |
| M37 | Power/thermal management service | ⬜ Pending |

## Package and application platform

| ID | Milestone | Status |
|---|---|---|
| M38 | Production package manager | ⬜ Pending |
| M39 | Repository/index metadata | ⬜ Pending |
| M40 | Install/update/rollback transactions | ⬜ Pending |
| M41 | Application sandbox policy | ⬜ Pending |
| M42 | First-party launcher | ⬜ Pending |
| M43 | Settings/system UI | ⬜ Pending |
| M44 | Files/storage UI | ⬜ Pending |
| M45 | Terminal/developer shell | ⬜ Pending |
| M46 | Core phone apps integration | ⬜ Pending |

## Device and compatibility

| ID | Milestone | Status |
|---|---|---|
| M47 | ARM64 reference board bring-up | ⬜ Pending |
| M48 | Real display/input hardware bring-up | ⬜ Pending |
| M49 | Real storage/network hardware bring-up | ⬜ Pending |
| M50 | Battery/charging/modem hardware integration | ⬜ Pending |
| M51 | Android compatibility architecture | ⬜ Pending |
| M52 | Android app/runtime compatibility prototype | ⬜ Pending |

## Production hardening

| ID | Milestone | Status |
|---|---|---|
| M53 | Secure boot / verified boot chain | ⬜ Pending |
| M54 | Key provisioning and device identity | ⬜ Pending |
| M55 | OTA update and rollback | ⬜ Pending |
| M56 | Kernel fuzzing and syscall fuzzing | ⬜ Pending |
| M57 | Memory-safety / concurrency audit | ⬜ Pending |
| M58 | Fault-injection and recovery testing | ⬜ Pending |
| M59 | Reproducible release builds | ⬜ Pending |
| M60 | Production security audit | ⬜ Pending |

## Release milestones

| ID | Milestone | Status |
|---|---|---|
| M61 | Developer Preview 0 — bootable core | ⬜ Pending |
| M62 | Developer Preview 1 — userspace services | ⬜ Pending |
| M63 | Developer Preview 2 — storage + GUI | ⬜ Pending |
| M64 | Developer Preview 3 — networking + device services | ⬜ Pending |
| M65 | Alpha — reference ARM64 device | ⬜ Pending |
| M66 | Beta — smartphone platform | ⬜ Pending |
| M67 | Release Candidate — production hardening complete | ⬜ Pending |
| M68 | InterOS 1.0 | ⬜ Pending |

## Current execution order

**M25 → M26 → M27 → M28 → M29 → M30 → M31 → M32 → M33 → M34 → M35 → M36 → M37 → M38 → M39 → M40 → M41 → M42 → M43 → M44 → M45 → M46 → M47 → M48 → M49 → M50 → M51 → M52 → M53 → M54 → M55 → M56 → M57 → M58 → M59 → M60 → M61 → M62 → M63 → M64 → M65 → M66 → M67 → M68**

The existing demo/reference applications are not counted as production-complete until the underlying service, storage, graphics, networking and hardware boundaries they depend on are real.
