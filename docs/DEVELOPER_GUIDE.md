# InterOS Developer Guide

InterOS Core is an open-source research prototype of a smartphone operating system, running in QEMU only. It is not a commercial product. External contributions are not accepted at this stage and will open after the first stable release.

The unified developer CLI (`tools/interos.py`, with root wrappers `interos.ps1` and `interos.cmd`) is the authoritative command interface for building, testing, packaging, and managing InterOS Core.

---

## 1. Environment & Diagnostics

### `interos doctor`
Inspects and validates host prerequisites, installed toolchains, target triples, QEMU binaries, and Device Studio availability:
```bash
python tools/interos.py doctor [--port 4300]
```
Checks:
- Rust compiler and Cargo version.
- Rust target triples: `x86_64-unknown-none`, `aarch64-unknown-none`, `riscv64gc-unknown-none-elf`.
- QEMU emulator binaries: `qemu-system-x86_64`, `qemu-system-aarch64`, `qemu-system-riscv64`.
- Python `cryptography` module.
- BIOS disk image builder and userspace manifest.

### `interos reality`
Displays the current platform reality contract, distinguishing implementation state from verification evidence:
```bash
python tools/interos.py reality
```

### `interos devices`
Lists supported virtual machine profiles, CPU architectures, and their implementation and verification tiers:
```bash
python tools/interos.py devices
```

---

## 2. Building & Compilation

### `interos build`
Builds the canonical demo, microkernels, or specific userspace applications:
```bash
# Build the complete canonical QEMU x86_64 demo (compiles all userspace ELFs before assembling the BIOS image):
python tools/interos.py build demo

# Build specific microkernel architectures:
python tools/interos.py build --target x86_64
python tools/interos.py build --target aarch64
python tools/interos.py build --target riscv64

# Build a specific userspace application crate:
python tools/interos.py build calculator
```

---

## 3. Testing & Contracts

### `interos test`
Executes automated test suites across targets:
```bash
# Run workspace unit tests:
python tools/interos.py test --target unit

# Run static system contract checks:
python tools/interos.py test --target contracts

# Run adversarial security suite:
python tools/interos.py test --target security

# Run lifecycle stress and recovery tests:
python tools/interos.py test --target lifecycle
python tools/interos.py test --target recovery
```

### `interos contracts`
Runs fast static assertions checking syscall pointer boundaries, service ABI limits, and packaging invariants:
```bash
python tools/interos.py contracts
```

### Empirical Benchmarks
Runs reproducible benchmark suites measuring image footprints, RAM overhead, boot latency, and heap invariants:
```bash
# Measure this tree's footprint and the guest's own telemetry markers:
python tools/bench_interos.py

# Run boot time latency harness:
python tools/bench_boot.py

# Verify zero-heap-allocation render and interactive event invariants:
python -m pytest tests/test_heap_bench.py

# Verify tap latency and correlation harness:
python -m pytest tests/test_tap_latency_bench.py
```

---

## 4. Application Packaging & Cryptography

### `interos new`
Scaffolds a new `.rapp` project from an official template:
```bash
python tools/interos.py new my_app --template basic
```

### `interos package` (alias: `pack`)
Compiles and bundles an application into a deterministic binary `.rapp` package:
```bash
python tools/interos.py package my_app [--output dist/my_app.rapp] [--arch x86_64]
```

### `interos sign`
Signs a `.rapp` package or payload binary with an Ed25519 private key:
```bash
python tools/interos.py sign dist/my_app.rapp --key dev.key
```

### `interos verify`
Cryptographically verifies `.rapp` header fields, SHA-256 payload digest, and Ed25519 signature:
```bash
python tools/interos.py verify dist/my_app.rapp
```

### `interos inspect`
Inspects header metadata, declared capabilities, architecture code, and hashes of a `.rapp` package:
```bash
python tools/interos.py inspect dist/my_app.rapp
```

---

## 5. QEMU Runtime & Package Lifecycle

### `interos run`
Launches the canonical QEMU virtual smartphone environment or deploys a specific package:
```bash
# Launch the canonical QEMU x86_64 environment:
python tools/interos.py run qemu

# Deploy and launch a specific application:
python tools/interos.py run my_app
```

### `interos trust`
Requests provisioning of a publisher's Ed25519 public key via the Device Studio API (note: unauthenticated raw serial injection is blocked by the kernel security policy; keys must be authorized via authenticated provisioning or pre-seeded trust roots):
```bash
python tools/interos.py trust dist/my_app.rapp [--publisher developer]
```

### `interos install`
Validates a signed package on the host, transfers it over the QEMU COM1 serial protocol, verifies it inside the guest kernel, and persists it in the guest package registry:
```bash
python tools/interos.py install dist/my_app.rapp
```

### `interos list`
Queries the active guest package catalog and lists installed packages:
```bash
python tools/interos.py list
```

### `interos uninstall`
Revokes an active package registration and removes its persistent blob from the guest store:
```bash
python tools/interos.py uninstall org.interos.my_app
```

---

## 6. Runtime Management & Observability

### `interos logs`
Streams or follows live microkernel serial UART logs (Ring 0 / EL1 / S-Mode):
```bash
# View recent UART output:
python tools/interos.py logs

# Follow log output continuously:
python tools/interos.py logs -f
```

### `interos shell`
Opens an interactive terminal session connected to the guest serial UART console (TCP port 4301):
```bash
python tools/interos.py shell [--port 4301]
```

### `interos trace`
Queries and displays live x86_64 CPU registers (`CR0`, `CR3`, `RIP`, `EAX`) directly from the QEMU monitor:
```bash
python tools/interos.py trace
```

### `interos snapshot`
Manages live QEMU execution snapshots for reproducible debugging:
```bash
# Save a snapshot:
python tools/interos.py snapshot --action save --name baseline

# List existing snapshots:
python tools/interos.py snapshot --action list

# Restore a snapshot:
python tools/interos.py snapshot --action load --name baseline
```

### `interos stop` & `interos restart`
Controls the QEMU emulator process lifecycle:
```bash
# Gracefully terminate the running QEMU guest:
python tools/interos.py stop

# Reset and restart the QEMU guest:
python tools/interos.py restart
```
