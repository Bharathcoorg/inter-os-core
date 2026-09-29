# InterOS Core — M10 / M11 / M12 Engineering Gate

This document records the current implementation and evidence boundary for the next three milestones after the M8 developer-tooling and M9 native-SDK work.

## M8 — Developer tooling and reproducibility

Implemented contracts:

- canonical `interos doctor/build/test/contracts/package/verify/run` workflow
- locked Cargo builds/tests/runs
- canonical x86_64 QEMU image graph
- guest-authoritative package/filesystem inspection in Device Studio
- static contracts preventing synthetic launcher/runtime state

Evidence boundary: source/contract verification is distinct from local build and QEMU runtime verification.

## M9 — Native application SDK

Implemented contracts:

- `AppSession` lifecycle/window/input helpers
- private application storage
- IPC send/receive
- notifications
- network socket creation
- event waiting
- application background/foreground/suspend/wake
- application stop/restart/focus APIs
- kernel/SDK syscall-number parity contract

Evidence boundary: SDK source/API compatibility must still be exercised by the local Cargo/QEMU build.

## M10 — Trusted service ABI

Implemented:

- versioned ISVC wire header
- fixed 16-byte header
- service operation namespace
- service ID bounds
- bounded 4096-byte payload
- kernel decoder
- SDK encoder/decoder parity

Required evidence:

- service ABI contract tests
- SDK Cargo tests
- QEMU service bootstrap

## M11 — PID1-owned signed service lifecycle

Implemented:

- PID1-owned service startup
- dependency-aware start/stop
- parent-scoped lifecycle authority
- object-scoped service-manager capability
- signed RAPP service verification
- service-specific capability policy
- heartbeat ownership validation
- failure detection
- explicit recovery
- terminal-task recovery gate
- restart backoff
- resource budgets
- service generation tracking

The kernel must never silently replace a failed service. Recovery is explicit and parent-owned.

Required evidence:

- service contract tests
- service lifecycle contract tests
- system-service boundary contracts
- QEMU bootstrap/stop/restart/recovery sequence

## M12 — Canonical QEMU validation

The canonical x86_64 validation sequence is:

1. Build the complete userspace/kernel/BIOS artifact graph.
2. Boot the canonical QEMU phone profile.
3. Verify required boot milestones.
4. Verify PID1 and complete service bootstrap.
5. Verify launcher presentation.
6. Capture a fresh framebuffer artifact.
7. Run the interactive lifecycle gate.
8. Fail closed on any missing milestone.

Use:

```text
python tools/interos.py test --target m10_m12
```

for the source/contract gate.

Use:

```text
python tools/interos.py test --target qemu
```

for the runtime QEMU gate after the canonical image has been built.

### Verification rule

Passing a source contract does not mean QEMU passed. Passing QEMU does not mean physical hardware passed. InterOS keeps these evidence levels separate.
