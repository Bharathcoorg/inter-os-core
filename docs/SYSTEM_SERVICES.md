# InterOS System Services Specification

InterOS Core is an open-source research prototype of a smartphone operating system, running in QEMU only. It is not a commercial product. External contributions are not accepted at this stage and will open after the first stable release.

InterOS Core isolates platform policy outside the microkernel. System services execute as standalone Ring-3 user processes (`userspace/system-service/`), acquire only the capability rights required for their specific operational role, and communicate with PID 1 via structured heartbeats and lifecycle protocols.

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
| **12** | `aid` | AI request broker over the development-host bridge: capability-gated routing, retrieval for global search, and the refusals a request gets back when it cannot be answered | Restart on failure | Present | **QEMU-verified** for the service, its IPC bridge and its refusals. Inference itself is not verified: no model weights are installed in the image, so generative and embedding requests come back as refusals ("...no local model weights loaded") rather than answers |

> *Note: Daemons 1–8, 11, and 12 are built from `userspace/system-service/`. Compositor (9) and Launcher (10) are standalone Ring-3 applications in `apps/` with system service registration.*

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

---

## 5. Verification Reality Boundary

In accordance with the [Platform Reality Contract](PLATFORM_REALITY_CONTRACT.md):
- Services running in QEMU consume emulated virtual platform backends (e.g. QEMU serial, loopback networking, virtual display).
- An unavailable hardware peripheral (such as a physical 5G modem, physical camera sensor, or physical PMIC) reports `Unavailable` rather than fabricating simulated hardware data.
