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
| M09 | PID1-owned service lifecycle ABI | 🟡 In progress |
| M10 | Service supervision, recovery and backoff | 🟡 In progress |

## Phase B — process and service architecture

| ID | Milestone | Status |
|---|---|---|
| M11 | Lifecycle unit/integration tests | ⬜ Next |
| M12 | QEMU PID1/service lifecycle assertions | ⬜ Pending |
| M13 | Service parent/reaping invariants | ⬜ Pending |
| M14 | Service capability isolation tests | ⬜ Pending |
| M15 | Trusted userspace service ABI | ⬜ Pending |
| M16 | Signed service package loading | ⬜ Pending |
| M17 | Userspace service manager replaces kernel workers | ⬜ Pending |
| M18 | Service crash/restart end-to-end validation | ⬜ Pending |

## Storage

| ID | Milestone | Status |
|---|---|---|
| M19 | Persistent block-device abstraction | ⬜ Pending |
| M20 | Real filesystem/storage backend | ⬜ Pending |
| M21 | Persistent VFS metadata | ⬜ Pending |
| M22 | Atomic writes and crash recovery | ⬜ Pending |
| M23 | Encrypted storage boundary | ⬜ Pending |
| M24 | Storage service userspace implementation | ⬜ Pending |

## Graphics and input

| ID | Milestone | Status |
|---|---|---|
| M25 | Framebuffer/display driver boundary | ⬜ Pending |
| M26 | Input device event pipeline | ⬜ Pending |
| M27 | Compositor protocol | ⬜ Pending |
| M28 | Window/surface management | ⬜ Pending |
| M29 | Touch/gesture input integration | ⬜ Pending |
| M30 | Hardware-backed graphics path | ⬜ Pending |

## Networking and smartphone services

| ID | Milestone | Status |
|---|---|---|
| M31 | Network device/driver abstraction | ⬜ Pending |
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

**M09 → M10 → M11 → M12 → M13 → M14 → M15 → M16 → M17 → M18 → M19**

The existing demo/reference applications are not counted as production-complete until the underlying service, storage, graphics, networking and hardware boundaries they depend on are real.
