# InterOS Core: System Architecture & Security Specification

> **Target Architectures:** x86_64 (`x86_64-unknown-none`), AArch64 (`aarch64-unknown-none`), RISC-V 64 (`riscv64gc-unknown-none-elf`)  
> **Core Language:** Bare-Metal Pure Rust (`#![no_std]`) with minimal assembly primitives  
> **Security Model:** Hardware-Enforced Capability Microkernel (Ring 3 / EL0 / U-Mode Userspace Isolation)  
> **Canonical Development Target:** x86_64 QEMU PC (`q35` BIOS disk image)  
> **Physical Hardware Status:** Experimental bring-up (AArch64 PinePhone Allwinner A64 research target)

---

## 1. Architectural Philosophy & Isolation Model

InterOS Core is engineered from first principles as an open-source, memory-safe mobile operating system foundation:

```text
+-------------------------------------------------------------------------+
|                  InterOS Native Smartphone Applications                 |
|   17 Ring-3 Apps: Launcher, Settings, Messages, Dialer, Camera, Gallery,|
|   Files, Calculator, Vault, Calendar, Browser, Music, Notes,            |
|   Video Player, Agent Manager, Compositor, Webview Demo                 |
+-------------------------------------------------------------------------+
|                Userspace System Services & SDK (Ring 3 / EL0 / U-Mode)  |
|   - Window Compositor (DRM/KMS Scanout)         - Audio Service (audiod) |
|   - Cellular RIL / Serial AT Modem (modemd)     - Touch Digitizer (inputd)|
|   - Block Storage & Records (storaged)          - Network Stack (netd)   |
|   - Power Controller (powerd)                   - Sensor Service (sensord)|
|   - Service Coordinator (init, PID 1)           - Camera Service (camerad)|
+-------------------------------------------------------------------------+
|                 Capability-Based Shared-Memory IPC Bus                  |
|       Shared-Memory Ring Buffers + Atomic Futex Synchronization         |
+-------------------------------------------------------------------------+
|                   InterOS Microkernel Core (Ring 0 / EL1 / S-Mode)      |
|   - Preemptive Priority Scheduler (100 Hz)   - Memory Paging & MMU       |
|   - Hardware Keystore & Vault Controller      - Capability Token Matrix  |
+-------------------------------------------------------------------------+
|                   Hardware Abstraction Layer (HAL)                      |
|      x86_64 (Canonical QEMU) |       AArch64 (QEMU / PinePhone) | RISC-V 64|
|   - APIC / PIC / PIT         |   - PSCI / GICv2 / ArchTimer     | - SBI / PLIC|
|   - Linear DRM/KMS VBE       |   - SimpleFB Framebuffer         | - NS16550A  |
+-------------------------------------------------------------------------+
```

### The Fundamental Security Invariants
Untrusted user processes cannot:
1. Access kernel memory (enforced by hardware page tables and `UserPtr` syscall validation).
2. Access another process's memory (enforced by per-task address space switching).
3. Access arbitrary MMIO (peripheral registers mapped exclusively to authorized drivers).
4. Forge capabilities (capabilities are opaque kernel table indices with generation tags).
5. Invoke unauthorized IPC (endpoints are capability-governed with caller verification).
6. Escape filesystem namespaces (sandboxes restrict paths to authorized subtrees).
7. Exhaust kernel resources without bounds (quotas on tasks, endpoints, and file descriptors).
8. Crash the kernel through malformed syscall pointers or invalid memory buffers.

---

## 2. Hardware Memory Isolation & Page Table Management

Every process executes within its own isolated virtual address space. Memory mappings are strictly partitioned into user and kernel regions:

### x86_64 (QEMU-Verified)
- **4-Level Paging (PML4 -> PDPT -> PD -> PT)**:
  - Higher-half kernel space (`0xFFFF_8000_0000_0000..0xFFFF_FFFF_FFFF_FFFF`) is mapped with supervisor privileges (`U/S = 0`).
  - Lower-half user space (`0x0000_0000_0040_0000..0x0000_7FFF_FFFF_FFFF`) is mapped with user privileges (`U/S = 1`).
  - Context switches reload `CR3` with the target task's page table root.
  - **W^X Enforcement**: User code pages are mapped read-only and executable; data/heap/stack pages are mapped read-write with No-Execute (`NX = 1`).

### AArch64 (Source-Level / QEMU Virt)
- **Split Translation Tables (TTBR0_EL1 and TTBR1_EL1)**:
  - `TTBR0_EL1`: Dedicated to userspace virtual addresses (`0x0000_0000_0000_0000..0x0000_FFFF_FFFF_FFFF`).
  - `TTBR1_EL1`: Dedicated to kernel virtual addresses (`0xFFFF_0000_0000_0000..0xFFFF_FFFF_FFFF_FFFF`).
  - User pages use `AP = 0b01` (`DESC_AP_RW_EL0`) and `PXN = 1` (Privileged Execute-Never).
  - Context switches reload `TTBR0_EL1` and issue `TLBI VMALLE1IS` with memory barrier `ISB`.

### RISC-V 64 (Source-Level / QEMU Virt)
- **Sv39 3-Level Paging**:
  - `satp` register configured for Mode 8 (Sv39).
  - Kernel DRAM (`0x8000_0000..0xBFFF_FFFF`) mapped with `PTE_R | PTE_W | PTE_X | PTE_G`.
  - User pages require `PTE_U` permission bit. Supervisor mode cannot execute user pages.
  - Context switches reload `satp` and issue `sfence.vma`.

---

## 3. Syscall Pointer Safety & Validation

All userspace addresses entering the kernel via system calls must pass strict validation before dereferencing:

```rust
pub struct UserPtr<T> {
    addr: usize,
    _marker: PhantomData<T>,
}

pub struct UserSlice {
    addr: usize,
    len: usize,
}
```

### Validation Invariants:
1. **Null Exclusion**: Reject pointers `< 0x1000` (guard page protection).
2. **Canonical Bounds**:
   - x86_64 / AArch64: `addr + len < 0x0000_8000_0000_0000`.
   - RISC-V 64: `addr + len < 0x8000_0000` (excluding kernel DRAM at `0x8000_0000`).
3. **Integer Overflow**: `addr.checked_add(len)` must not overflow `usize`.
4. **Kernel Address & MMIO Exclusion**: Rejection with `-EFAULT` if any portion overlaps physical RAM or hardware MMIO.
5. **Ownership & Permissions**: On x86_64, `TaskPageTable::validate_user_range` verifies page-by-page mapping, user permissions, and frame ownership.

For detailed specification, see [Syscall Pointer Safety](SYSCALL_POINTER_SAFETY.md).

---

## 4. Capability Security Model

Authority in InterOS Core is based on object capabilities rather than ambient user IDs:

1. **Zero Ambient Authority**: A process begins execution with an empty capability table.
2. **Capability Handles**: Opaque indices into a per-task kernel capability table (`CapabilityTable`), guarded by rights bitmasks (`READ`, `WRITE`, `EXECUTE`, `GRANT`) and generation tags.
3. **Object Binding**: Each capability references a specific kernel object:
   - `FileSystemRead` / `FileSystemWrite`: Bound to specific Inode handles.
   - `IpcSend` / `IpcReceive`: Bound to specific IPC endpoints.
   - `ShmMap`: Bound to specific physical frames allocated for shared memory.
   - `SystemControl`: Scoped to specific hardware domains (power, telephony, audio, camera, sensors).
4. **No Privilege Escalation**: Delegation can attenuate or transfer rights, but cannot mint rights not possessed by the delegating capability.

---

## 5. UI Rendering & Compositor Architecture

InterOS Core features a native smartphone UI architecture designed for bare-metal `#![no_std]` environments:

### Coordinate Spaces & Scanout Transform
- **Logical Portrait Resolution**: `720 × 1440` (1:2 aspect ratio).
- **Safe Area Insets**: Status bar height: 48px; Navigation pill height: 64px.
- **Scanout Mode**: `720 × 1440` native portrait ARGB8888 scanout in QEMU (`VGA`/stdvga, `edid=on,xres=720,yres=1440`), 60 Hz.
- **Direct Scanout**: The kernel engages native 720×1440×32bpp mode through Bochs/VBE DISPI registers (`kernel/src/framebuffer.rs`, `engage_native_mode`). The 720×1440 logical canvas maps 1:1 pixel-exact to the scanout buffer without downsampling or letterboxing.

### Typography Engine (`interos-std::design::font`)
- Proportional 16×24 master typeface (`FONT`, `FONT_BOLD`) with 17-row cap height, 12-row x-height, and 4-row descender depth.
- Tabular figures (11px fixed advance at 1×) ensuring clock digits never jitter sideways.
- Proportional character metrics (e.g. `i` at 6px advance, `W` at 16px advance).
- 4-bit grayscale coverage anti-aliasing (`test_text_aa`) blending fractional edge alpha into background pixels with fast-path early exits.
- Text measurement (`measure`), optical centering (`center_x`), line wrapping (`wrap_text`), and ellipsization (`ellipsize`).

### Frame-Plan Architecture
- Layout origins, text measurements, and bounds are calculated once per frame plan (`Frame::measure`).
- Rasterization reads precalculated geometry, avoiding expensive per-pixel text walks or layout recalculation.

---

## 5a. The Content Layer: Document Index & Intent Registry

Two questions about the whole device used to be answered inside the apps that
were asking them. "What files does this device hold?" was answered by the
gallery's own storage scan and again by the file manager's own directory
listing, with two parsers and two notions of what a file is. "Which program
opens this file?" was answered by a table inside the file manager, and by
nothing at all in the gallery, the browser or the dialer.

Both questions are now answered by one kernel subsystem, `kernel/src/content/`:

```text
+-------------------------------------------------------------+
| Document index (kernel/src/content/index.rs)                |
| - one walk of the shared storage area (/storage)            |
| - per document: stable id, path, byte length, media type,   |
|   modification time, and the PROVENANCE of the media type   |
+-------------------------------------------------------------+
| Media type detection (kernel/src/content/mime.rs)           |
| - bytes first: PNG/JPEG/GIF/BMP/QOI/RIFF/ftyp/PDF/PPM/...   |
| - the file's name only as a fallback                         |
| - the record says which of the two produced the answer       |
+-------------------------------------------------------------+
| Intent registry (kernel/src/content/intent.rs)               |
| - (kind, value) -> package id, built from signed manifests  |
| - a pending intent, visible only to the package it names    |
+-------------------------------------------------------------+
```

### Authority

Both halves are reachable only through a capability, so "who may see the
index" is answered by the capability table and not by a constant:

| Capability | Identifier | Grants |
| :--- | :--- | :--- |
| `ContentIndexRead` | `25` | Read the index, and read the registry of declared handlers |
| `ContentIntentDispatch` | `26` | Dispatch an intent to the package that declared a handler for it |

`ContentIntentDispatch` is deliberately **not** `ProcessSpawn`: dispatching an
intent is authority over one named relationship — "the package that declared
this media type" — which the kernel resolves from the signed manifests.

### The index reads through the filesystem's own policy

The index describes the shared storage area, and the answer to a query is
filtered through `Vfs::enforce_app_path` before a record is returned. The
per-app path policy is unchanged: the index reads through it rather than
keeping a second copy of the rules.

### The system call surface

| Syscall | Number | Purpose |
| :--- | :--- | :--- |
| `ContentIndexQuery` | `140` | List the index, search it by name, search it by media type, or look a document up by its identity |
| `ContentIndexRefresh` | `141` | Force a fresh walk of the shared storage area |
| `IntentResolve` | `142` | The package that handles a media type, scheme or action |
| `IntentList` | `143` | Every handler the installed packages have declared |
| `IntentDispatch` | `144` | Hand an intent (and its payload) to the declared handler |
| `IntentPending` | `145` | The intent dispatched to the calling package |

The SDK wraps these in `interos_std::content` with the same shape as the other
platform calls, and the calls take the caller's authority rather than a
capability slot: a slot is a position in the task's table, while the
capability *type* is what the operation means.

### Truth

- A media type that no installed package declares has **no handler**. The
  platform refuses such an intent (`TargetNotFound`) and names nothing; the
  app says "no app handles this file" and names nothing.
- The index says where every media type answer came from — `bytes`,
  `extension` or `unknown` — so a guess is never presented as a measurement.
- The registry is built from the packages' own signed manifests, registered at
  install and withdrawn at uninstall. The first-party applications run as
  kernel-resident images rather than as installed packages, so their
  declarations are seeded from the kernel table that mirrors their
  `manifest.json`; `tests/test_content_intent_contract.py` compares the two
  so they cannot drift apart.

### The interlinks

| Interlink | Path |
| :--- | :--- |
| Gallery "set as wallpaper" | gallery → `IntentDispatch(set_wallpaper)` → settings app → `ui.wallpaper.home` / `ui.wallpaper.home.document` → launcher paints it |
| "Open with" | gallery or file manager → `IntentResolve` + `IntentDispatch(mime)` → the package that declared that media type |
| Browser downloads | browser → `/storage/downloads/…` → the index → the file manager and the gallery list it |
| "Set as ringtone" | file manager or gallery → `IntentDispatch(set_ringtone)` → settings app → `audio.ringtone` → the dialer shows and plays it |
| The file manager's view options | the registry's own listing, filtered by the document's media type |

---

## 6. Cryptographic `.rapp` Package Format

Applications and system services are packaged using the native, deterministic `.rapp` binary format with Ed25519 signature enforcement:

```text
+-------------------------------------------------------------+
| RAPP Header (180 bytes / 0xB4)                              |
| - Magic: b"RAPP" (4 bytes)                                  |
| - Version: 2 (2 bytes) | Arch: 0=x86, 1=arm64, 2=rv64 (2B)  |
| - Package ID: UTF-8, null-padded (32 bytes)                 |
| - Publisher Ed25519 Public Key (32 bytes)                   |
| - Semantic Version (3x u16: 6 bytes)                        |
| - Capability Count: N (2 bytes)                             |
| - Payload SHA-256 Digest (32 bytes)                         |
| - Ed25519 Signature (64 bytes)                              |
| - Payload Length: LE u32 (4 bytes)                          |
+-------------------------------------------------------------+
| Capability IDs: N x u16 (2 * N bytes)                       |
+-------------------------------------------------------------+
| Executable ELF Binary Payload (Payload Length bytes)        |
+-------------------------------------------------------------+
```

### Installation & Verification Pipeline:
1. **Header Validation**: Checks `b"RAPP"` magic, version `2` (or `3` with the dependency header, or `4` with the dependency and intent headers), and target architecture code.
2. **Buffer Bounds**: Strictly validates `data.len() == 0xB4 + 2 * cap_count + payload_len` for v2, and `0xB8 + 2 * cap_count + section_len + payload_len` for v3 and v4.
3. **Cryptographic Attestation**:
   - Computes SHA-256 digest over the embedded ELF payload and verifies against the header digest.
   - Evaluates publisher public key against the kernel trust root registry.
   - Verifies Ed25519 signature over canonical signing message.
4. **Isolated Process Execution**:
   - Resolves declared capability IDs into per-task capability rights.
   - Maps ELF sections into a dedicated address space (`W^X` enforced).
   - Launches child process in Ring 3 / EL0 userspace.

For complete binary layout details, see [RAPP Package Format](RAPP_FORMAT.md).

---

## 7. Android Compatibility Runtime (ACR) Boundary

InterOS Core provides an isolated compatibility subsystem for Android application packages (`.apk`):

1. **Subsystem Scope**: The ACR is an isolated compatibility subsystem, **not** a claim of full Android application ecosystem parity. Native `.rapp` applications remain the primary platform target.
2. **Unified Shared Library**: `#![no_std]` `interos-android-shared` crate shared across kernel architectures.
3. **Safe Path Normalization**: Android guest filesystem access is strictly bounded to `/data/data/<pkg>` and `/sdcard/Android/data/<pkg>`. Path traversal attempts are rejected.
4. **Signature Verification**:
   - APK Signature Scheme v2/v3 signing block detection via `APK Sig Block 42` magic and block IDs `0x7109871a` / `0xf05368c0`.
   - Fallback JAR signature verification requiring `META-INF/MANIFEST.MF` and valid certificate structures (`.RSA` / `.DSA` / `.EC`).
5. **Bytecode Auditor & Dalvik VM**:
   - Adler-32 header checksum verification.
   - 32 virtual register state machine (`[u32; 32]`).
   - Instruction bounds checking preventing arbitrary jumps or stack overflow.

---

## 8. Local Server & Emulator Security

The developer emulator backend (`emulator/server.py`) operates under strict isolation:

1. **Localhost Binding**: Server binds exclusively to `127.0.0.1:4300`, preventing remote network access.
2. **SSRF Defense on `/api/browser/proxy`**:
   - Resolves target hostnames and validates destination IP addresses.
   - Blocks loopback (`127.0.0.0/8`), private subnets (`10.0.0.0/8`, `172.16.0.0/12`, `192.168.0.0/16`), link-local (`169.254.0.0/16`), and cloud metadata endpoints (`169.254.169.254`).
   - Restricts protocols strictly to `http` and `https`.

---

## 9. Cross-Architecture Status & Parity Matrix

| Subsystem | x86_64 | AArch64 | RISC-V 64 |
| :--- | :--- | :--- | :--- |
| **Boot Chain** | BIOS MBR via `bootloader` 0.11 | QEMU `-kernel` / Fastboot v2 | InterOS Boot Firmware (SBI v1.0+) |
| **MMU / Paging** | 4-Level CR3 Task Tables | 4-Level TTBR0_EL1 Tables | Sv39 3-Level SATP Mode 8 |
| **Hardware Isolation** | Ring 0 / Ring 3 | EL1 / EL0 | S-Mode / U-Mode |
| **Trap / Syscall Entry** | MSR Fast Syscall (LSTAR) | Synchronous EL0 Exception | `stvec` Direct Trap Handler |
| **Syscall Pointer Safety** | Full per-task page-table validation | Bounded user-copy helpers | Bounded user-copy helpers |
| **Per-Task Kernel Stack** | TSS RSP0 / IST Double Fault | SP_EL1 per-thread | `sscratch` saved frame |
| **Capability Model** | QEMU-enforced | Unit-tested | Unit-tested |
| **Framebuffer GUI** | 720x1440 software KMS scanout (1:1 composition from the 720x1440 authoring space) | SimpleFB Framebuffer | NS16550A Serial |
| **ACR Container** | Shared crate ready | Shared crate ready | Shared crate ready |
| **Verification State** | **QEMU-verified** | **Unit-tested / QEMU virt** | **Unit-tested / QEMU virt** |

---

## 10. Known Limitations & Ongoing Engineering Work

1. **DMA / IOMMU**: Device DMA currently relies on physical frame allocation from a reserved frame region. Full SMMU / IOMMU page table integration for hardware peripherals is ongoing.
2. **GPU Acceleration**: Display rendering utilizes linear software framebuffers (`SimpleFB` / DRM KMS). Hardware-accelerated GPU pipelines are planned for future revisions.
3. **Physical Hardware Deployment**: Fastboot v2 Android boot images (`boot-aarch64.img`) compile and package correctly. Physical device testing is strictly experimental bring-up targeting laboratory development boards (e.g. PinePhone Allwinner A64).
