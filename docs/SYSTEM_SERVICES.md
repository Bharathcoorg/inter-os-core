# InterOS System Services Specification

InterOS Core is an open-source research prototype of a smartphone operating system, running in QEMU only. It is not a commercial product. External contributions are not accepted at this stage and will open after the first stable release.

InterOS Core isolates platform policy outside the microkernel. System services execute as standalone unprivileged userspace processes (x86_64 Ring 3 / AArch64 EL0 / RISC-V U-Mode, built from `userspace/system-service/`), acquire only the capability rights required for their specific operational role, and communicate with PID 1 via structured heartbeats and lifecycle protocols.

---

## 1. Canonical Service Registry

| Service ID | Binary / Module | Primary Responsibility | Failure Recovery Policy | Implementation State | Verification State |
| :--- | :--- | :--- | :--- | :--- | :--- |
| **1** | `displayd` | Surface creation, framebuffer scanout boundary | Restart on failure | Present | **QEMU-verified** |
| **2** | `inputd` | Normalized touch/gesture delivery | Restart on failure | Present | **QEMU-verified** |
| **3** | `storaged` | Persistent VFS storage and file operations | Restart on failure | Present | **QEMU-verified** |
| **4** | `netd` | Network socket polling and packet datapath | Restart on failure | Present | **QEMU-verified (loopback)** |
| **5** | `powerd` | Battery status, power profiles, DVFS boundary | Restart on failure | Present | **Unit-tested (emulated)** |
| **6** | `modemd` | Cellular baseband, AT parser, SMS inbox | Restart on failure | Present | **Unit-tested (emulated)** |
| **7** | `audiod` | Audio stream buffer playback and feedback | Restart on failure | Present | **Unit-tested (emulated)** |
| **8** | `camerad` | Camera sensor control and capture boundary | Restart on failure | Present | **Unit-tested (emulated)** |
| **9** | `compositor` | Window composition and DRM/KMS presentation | Restart on failure | Present | **QEMU-verified** |
| **10** | `launcher` | Package catalog, shell UI, gesture navigation| Restart on failure | Present | **QEMU-verified** |
| **11** | `sensord` | Accelerometer, gyroscope, light sensor | Restart on failure | Present | **Unit-tested (emulated)** |
| **12** | `aid` | Capability-gated AI requests from Launcher search and authorized apps, with a lexical search fallback | Restart on failure | Present in the `agents` boot profile | **QEMU-verified** startup. Product model execution in the guest remains unverified |

> *Note: Daemons 1–8, 11, and 12 are built from `userspace/system-service/`. Compositor (9) and Launcher (10) are standalone userspace applications in `apps/` with system service registration.*

---

## 2. Security & Capability Boundaries

Services do not receive unrestricted hardware authority by default:
- **Domain-Scoped `SystemControl`**: Peripheral control APIs require object-scoped capabilities (`SystemControlPower`, `SystemControlTelephony`, `SystemControlAudio`, `SystemControlCamera`, `SystemControlSensor`). Generic `SystemControl` remains reserved strictly for privileged kernel initialization.
- **Storage Partitioning**: The storage service (`storaged`) holds dedicated `FileSystemRead` and `FileSystemWrite` capabilities. Unsandboxed raw-block storage operations require explicit `StorageRawBlock` rights held by the vault controller.
- **Network Boundaries**: Network access is split between socket creation (`NetworkSocket`), datagram transmit (`NetworkTransmit`), and packet reception (`NetworkReceive`).
- **Display Boundary**: Direct framebuffer flipping is restricted to `displayd` and `compositor` holding `FrameBufferDirect` tokens.

---

## 3. Lifecycle & Supervision Architecture

- **Supervisor Role**: PID 1 (`userspace/init`) supervises service processes.
- **Heartbeat Protocol**: Active services issue regular `sys_service_heartbeat(service_id)` syscalls. The kernel rejects heartbeats whose task PID does not match the registered task ID.
- **Dependency Ordering**: A dependency service (e.g. `storaged`) cannot be stopped while dependent services are running.
- **Failure Backoff & Recovery**: Unresponsive or crashed workers enter the failed state. PID 1 reaps the terminated process and restarts a clean instance with fresh capability allocations.

---

## 4. Resource Budgets & Isolation

- **Enforced Budgets**:
  - Maximum thread counts and maximum open file descriptors are enforced at the process and VFS boundaries before service registration completes.
- **Reserved Contract Fields**:
  - IPC endpoint counts and address-space byte limit fields are declared in the process resource descriptor as architectural invariants; they are reserved for future quota accounting and are **not** currently enforced as active hardware counters.

### AI model deployment

The format-neutral userspace backend and manifest contracts describe format, architecture, quantization, tokenizer, operations, hardware and resource requirements. They are not yet connected to `aid` model selection or installation; the current execution path still probes specific GGUF slots. A manifest digest alone establishes byte consistency, not publisher authenticity. User-selected models require a production registry, authenticated provenance and backend integration before they can be managed through this interface.

The initial test set is:

| Model | Intended operation | Package | Current InterOS execution state |
| :--- | :--- | :--- | :--- |
| EmbeddingGemma 2 Text 270M | semantic retrieval and content indexing | `.litertlm` | Container and provenance validation implemented; native Rust graph execution not implemented |
| Gemma 4 E2B | local generation | `.litertlm` | Container and provenance validation implemented; native Rust graph execution not implemented |
| compatible GGUF models | generation or embedding, according to parsed tensor graph | `.gguf` | native Rust CPU backend present; model compatibility and QEMU execution must be verified per artifact |

The two pinned LiteRT-LM packages occupy about 2.56 GiB together. A fixed 6 GiB data image must also reserve space for filesystem overhead, applications, indexes, updates and user data. The ledger has a storage admission check, but the current model installer does not yet use it. Model files are data, not part of the kernel or OS executable.

The current generation path checks free-RAM telemetry against an estimated working set and a foreground reserve. The separate ledger models resident and session reservations and reclamation, but is not yet authoritative long-lived `aid` state. Embedding admission and actual allocation accounting still need integration. Dynamic RAM does not guarantee that a particular model fits every configuration.

Run `python tools/download_official_gemma4.py --verify-only --with-embedding` to verify both pinned LiteRT-LM packages by exact length and SHA-256. A normal download also writes a small `.modelmanifest` beside each artifact. `userspace/system-service/src/litertlm.rs` validates the official 32-byte envelope, FlatBuffer section inventory, 16 KiB alignment and payload bounds without allocation or an external FlatBuffers crate. That inspection is not inference. A pure-Rust LiteRT-LM backend still requires TFLite graph parsing, metadata and tokenizer parsing, supported operators, quantized kernels, dynamic tensor/KV-cache handling, scheduling and numerical conformance tests.

### System-wide AI path

Launcher global search and authorized applications send the versioned AI IPC protocol to the service alias resolved to the registered `aid` task. The kernel verifies the peer and checks authority on every operation: inference requires `InferenceService`, retrieval additionally requires `SearchService` plus `ContentIndexRead`, and installation additionally requires `FileSystemWrite`. Revocation therefore affects the next request. The broker treats model output and indexed content as untrusted, uses bounded frames and buffers, and gives the model no shell, root, arbitrary VFS or hardware access.

`aid` currently handles GGUF probing, generation, embedding requests, cancellation and replies. Its format-neutral provider selection and manifest verification are not yet wired into those paths. Search uses the retrieval service for launcher and authorized applications; the lexical baseline is labelled separately from neural output. End-to-end system-wide semantic retrieval has not been verified in the guest.

---

## 5. Verification Reality Boundary

In accordance with the [Platform Reality Contract](PLATFORM_REALITY_CONTRACT.md):
- Services running in QEMU consume emulated virtual platform backends (e.g. QEMU serial, loopback networking, virtual display).
- An unavailable hardware peripheral (such as a physical 5G modem, physical camera sensor, or physical PMIC) reports `Unavailable` rather than fabricating simulated hardware data.
