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

InterOS does not bind the OS to one model name. `aid` selects a userspace backend by a verified model manifest containing format, architecture, quantization, tokenizer, operations, hardware and resource requirements. Unknown combinations are rejected as unsupported; an installed model never makes an absent runtime appear available. This permits user-selected future models without granting model code ambient authority or teaching the kernel model formats.

The initial test set is:

| Model | Intended operation | Package | Current InterOS execution state |
| :--- | :--- | :--- | :--- |
| EmbeddingGemma 2 Text 270M | semantic retrieval and content indexing | `.litertlm` | Container and provenance validation implemented; native Rust graph execution not implemented |
| Gemma 4 E2B | local generation | `.litertlm` | Container and provenance validation implemented; native Rust graph execution not implemented |
| compatible GGUF models | generation or embedding, according to parsed tensor graph | `.gguf` | native Rust CPU backend present; model compatibility and QEMU execution must be verified per artifact |

The two pinned LiteRT-LM packages occupy about 2.56 GiB together. On a fixed 6 GiB data image the installer must also preserve space for filesystem overhead, applications, indexes, updates and user data; it therefore checks the observed free-storage value instead of assuming the nominal image size. Model files are data, not part of the kernel or OS executable.

At run time, admission uses current free-RAM telemetry and a backend estimate split into weights, KV cache, activations, scratch, tokenizer, indexes, audio buffers and runtime overhead. A foreground reserve scales with detected RAM. Reservations are reclaimed on cancellation, session destruction, owner death and backend failure. The mechanism supports dynamic RAM configurations; it does not promise that a particular model fits every configuration.

Run `python tools/download_official_gemma4.py --verify-only --with-embedding` to verify both pinned LiteRT-LM packages by exact length and SHA-256. A normal download also writes a small `.modelmanifest` beside each artifact. `userspace/system-service/src/litertlm.rs` validates the official 32-byte envelope, FlatBuffer section inventory, 16 KiB alignment and payload bounds without allocation or an external FlatBuffers crate. That inspection is not inference. A pure-Rust LiteRT-LM backend still requires TFLite graph parsing, metadata and tokenizer parsing, supported operators, quantized kernels, dynamic tensor/KV-cache handling, scheduling and numerical conformance tests.

### System-wide AI path

Launcher global search and authorized applications send the versioned AI IPC protocol to the service alias resolved to the registered `aid` task. The kernel verifies the peer and checks authority on every operation: inference requires `InferenceService`, retrieval additionally requires `SearchService` plus `ContentIndexRead`, and installation additionally requires `FileSystemWrite`. Revocation therefore affects the next request. The broker treats model output and indexed content as untrusted, uses bounded frames and buffers, and gives the model no shell, root, arbitrary VFS or hardware access.

`aid` owns provider selection, model verification, memory admission, cancellation and streaming. Search uses the same retrieval service for the launcher and applications; when a neural embedding backend is unavailable, the response is explicitly labelled as the lexical baseline rather than as EmbeddingGemma output. Generative requests likewise return unavailable or unsupported instead of substituting a host process and calling it on-device inference.

---

## 5. Verification Reality Boundary

In accordance with the [Platform Reality Contract](PLATFORM_REALITY_CONTRACT.md):
- Services running in QEMU consume emulated virtual platform backends (e.g. QEMU serial, loopback networking, virtual display).
- An unavailable hardware peripheral (such as a physical 5G modem, physical camera sensor, or physical PMIC) reports `Unavailable` rather than fabricating simulated hardware data.
