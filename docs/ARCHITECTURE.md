# InterOS Core: System Architecture & Security Specification

> **Target Platforms:** RISC-V 64 (`riscv64gc-unknown-none-elf`), ARM64 (`aarch64-unknown-none`), x86_64 (`x86_64-unknown-none`)  
> **Core Language:** 100% Bare-Metal Rust (`#![no_std]`)  
> **Security Model:** Hardware-Enforced Capability Microkernel (Ring 3 / EL0 / U-Mode Userspace Isolation)  
> **Project Status:** Engineering Prototype / Active Bring-Up Phase (Tested in QEMU virt/PC)

---

## 1. Architectural Philosophy & Isolation Model

InterOS Core is engineered from first principles as an open-source, memory-safe mobile operating system foundation:

```
+-------------------------------------------------------------------------+
|                  InterOS Native Smartphone Applications                 |
|    (Phone, Messages, Music Player, Calendar, Files, Camera, Settings)   |
+-------------------------------------------------------------------------+
|                Userspace System Services & SDK (Ring 3 / EL0 / U-Mode)  |
|   - Window Compositor (DRM/KMS Framebuffer)    - Audio Synthesizer Engine|
|   - Cellular RIL / Serial AT Modem Daemon       - Touch Input Digitizer  |
|   - Package Sideload Manager (.rapp/.apk)       - interos-std Syscalls   |
+-------------------------------------------------------------------------+
|                 Capability-Based Zero-Copy IPC Bus                      |
|       Shared-Memory Ring Buffers + Atomic Futex Synchronization         |
+-------------------------------------------------------------------------+
|                   InterOS Microkernel Core (Ring 0 / EL1 / S-Mode)      |
|   - Preemptive Priority Scheduler (100 Hz)   - Memory Paging & MMU       |
|   - Hardware Keystore & Vault Controller      - Capability Token Matrix  |
+-------------------------------------------------------------------------+
|                   Hardware Abstraction Layer (HAL)                      |
|      RISC-V 64 (QEMU Virt)   |   ARM64 (Cortex-A / PinePhone)   |  x86_64|
|   - InterOS SBI / PLIC / CLINT |   - PSCI / GICv2/v3 / ArchTimer  |  - APIC|
|   - NS16550A Serial UART     |   - SimpleFB & Goodix Touch      |  - VGA |
+-------------------------------------------------------------------------+
```

### The Fundamental Security Invariant
Untrusted user processes cannot:
1. Access kernel memory (enforced by MMU hardware page tables and syscall pointer validation).
2. Access another process's memory (enforced by per-task page table switching).
3. Access arbitrary MMIO (peripheral registers are mapped exclusively to authorized drivers).
4. Forge capabilities (capabilities are kernel-managed table indices with generation tags).
5. Invoke unauthorized IPC (endpoints are capability-governed with caller verification).
6. Escape filesystem namespaces (sandboxes restrict paths to authorized subtrees).
7. Exhaust kernel resources without bounds (quotas on tasks, endpoints, and file descriptors).
8. Crash the kernel through malformed syscall pointers or invalid buffers.

---

## 2. Hardware Memory Isolation & Page Table Management

Every process has its own isolated address space. Memory mappings are strictly partitioned into user and kernel regions:

### x86_64
- **4-Level Paging (PML4 -> PDPT -> PD -> PT)**:
  - Higher-half kernel space (`0xFFFF_8000_0000_0000..0xFFFF_FFFF_FFFF_FFFF`) is mapped with supervisor privileges (`U/S = 0`).
  - Lower-half user space (`0x0000_0000_0040_0000..0x0000_7FFF_FFFF_FFFF`) is mapped with user privileges (`U/S = 1`).
  - Context switches reload `CR3` with the target task's page table root.
  - W^X enforcement: User text pages are mapped read-only and executable; data/heap/stack pages are mapped read-write and no-execute (`NX = 1`).

### AArch64
- **Split Translation Tables (TTBR0_EL1 and TTBR1_EL1)**:
  - `TTBR0_EL1`: Dedicated to userspace virtual addresses (`0x0000_0000_0000_0000..0x0000_FFFF_FFFF_FFFF`).
  - `TTBR1_EL1`: Dedicated to kernel virtual addresses (`0xFFFF_0000_0000_0000..0xFFFF_FFFF_FFFF_FFFF`).
  - User pages use `AP = 0b01` (`DESC_AP_RW_EL0`) and `PXN = 1` (Privileged Execute-Never).
  - Context switch reloads `TTBR0_EL1` and issues `TLBI VMALLE1IS` with memory barrier `ISB`.

### RISC-V 64
- **Sv39 3-Level Paging**:
  - `satp` register configured for Mode 8 (Sv39).
  - Kernel DRAM (`0x8000_0000..0xBFFF_FFFF`) mapped with `PTE_R | PTE_W | PTE_X | PTE_G`.
  - User pages require `PTE_U` permission bit. Supervisor mode cannot execute user pages (enforced by hardware).
  - Context switch reloads `satp` and issues `sfence.vma`.

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
2. **Canonicality & Upper Bound**:
   - x86_64 / AArch64: `addr + len < 0x0000_8000_0000_0000`.
   - RISC-V 64 QEMU virt: `addr + len < 0x8000_0000` (excluding kernel DRAM at `0x8000_0000`).
3. **Integer Overflow**: `addr.checked_add(len)` must not overflow `usize`.
4. **Kernel Address & MMIO Exclusion**: Rejection with `-EFAULT` if any portion overlaps physical RAM or hardware MMIO.

---

## 4. Capability Security Model

Authority in InterOS Core is based on object capabilities rather than ambient user IDs:

1. **Zero Ambient Authority**: A process begins execution with an empty capability table.
2. **Capability Handles**: Opaque indices into a per-task kernel capability table (`CapabilityTable`), guarded by rights bitmasks (`READ`, `WRITE`, `EXECUTE`, `GRANT`).
3. **Object Binding**: Each capability references a specific kernel object:
   - `FileSystemRead` / `FileSystemWrite`: Bound to specific Inode handles.
   - `IpcSend` / `IpcReceive`: Bound to specific IPC endpoints.
   - `ShmMap`: Bound to specific physical frames allocated for shared memory.
4. **No Privilege Escalation**: Delegation can attenuate or transfer rights, but cannot mint rights not possessed by the delegating capability.

---

## 5. Cryptographic `.rapp` Package Format

Applications are packaged using standard ZIP containers with cryptographic integrity checks:

```
my_app.rapp (ZIP Archive)
├── manifest.json       (Canonical application manifest)
├── bin/
│   └── app.elf         (Native #![no_std] ELF binary)
├── ui/
│   ├── index.html      (HTML5 interface)
│   ├── app.js          (Client application logic)
│   └── styles.css      (Visual styling)
└── signature.json      (Ed25519 signature & SHA-256 manifest)
```

### Installation Security Pipeline:
1. **Zip-Slip & Path Traversal Defense**: Reject entries containing `..`, absolute paths, or leading slashes.
2. **Zip-Bomb Mitigation**: Reject archives where uncompressed size exceeds limits (default 50 MB) or compression ratio exceeds 100:1.
3. **Cryptographic Attestation**:
   - Compute SHA-256 hashes of all bundled files and match against `signature.json`.
   - Verify Ed25519 asymmetric signature over canonical package digest using publisher public key.
4. **Atomic Staging & Rollback**:
   - Extract into `_staging_<id>/`.
   - Verify all digests.
   - Atomic rename to final installation directory.
   - Rollback and directory purge on any validation failure.

---

## 6. Android Compatibility Runtime (ACR) Boundary

InterOS Core provides an isolated runtime for Android application packages (`.apk`):

1. **Unified Shared Library**: `#![no_std]` `interos-android-shared` crate shared across kernel architectures.
2. **Safe Path Normalization**: Android guest filesystem access is strictly bounded to `/data/data/<pkg>` and `/sdcard/Android/data/<pkg>`. Path traversal attempts are rejected.
3. **Signature Verification**:
   - APK Signature Scheme v2/v3 signing block detection via `APK Sig Block 42` magic and block IDs `0x7109871a` / `0xf05368c0`.
   - Fallback JAR signature verification requiring `META-INF/MANIFEST.MF` and valid certificate structures (`.RSA` / `.DSA` / `.EC`).
4. **Bytecode Auditor & Dalvik VM**:
   - Adler-32 header checksum verification.
   - 32 virtual register state machine (`[u32; 32]`).
   - Instruction bounds checking preventing arbitrary jumps or stack overflow.

---

## 7. Local Server & Emulator Security

The developer emulator backend (`emulator/server.py`) operates under strict isolation:

1. **Localhost Binding**: Server binds exclusively to `127.0.0.1:4300`, preventing remote network access.
2. **SSRF Defense on `/api/browser/proxy`**:
   - Resolves target hostnames and validates destination IP addresses.
   - Blocks `127.0.0.0/8`, private subnets (`10.0.0.0/8`, `172.16.0.0/12`, `192.168.0.0/16`), link-local (`169.254.0.0/16`), cloud metadata (`169.254.169.254`), and loopback addresses.
   - Restricts protocols strictly to `http` and `https`.

---

## 8. Cross-Architecture Status & Parity Matrix

| Subsystem | x86_64 | AArch64 | RISC-V 64 |
| :--- | :--- | :--- | :--- |
| **Boot Chain** | BIOS MBR via `bootloader` 0.11 | QEMU `-kernel` / Fastboot v2 | InterOS Boot Firmware (SBI v1.0+) |
| **MMU / Paging** | 4-Level CR3 Task Tables | 4-Level TTBR0_EL1 Tables | Sv39 3-Level SATP Mode 8 |
| **Hardware Isolation** | Ring 0 / Ring 3 | EL1 / EL0 | S-Mode / U-Mode |
| **Trap / Syscall Entry** | MSR Fast Syscall (LSTAR) | Synchronous EL0 Exception | `stvec` Direct Trap Handler |
| **Syscall Pointer Safety** | `UserPtr` `< 0x0000_8000_0000_0000` | `UserPtr` `< 0x0000_8000_0000_0000` | Bounds check `< 0x8000_0000` |
| **Per-Task Kernel Stack** | TSS RSP0 / IST Double Fault | SP_EL1 per-thread | `sscratch` saved frame |
| **Capability Model** | 100% Enforced | 100% Enforced | 100% Enforced |
| **Framebuffer GUI** | 1280x720 Linear Framebuffer | SimpleFB 1080x2400 | NS16550A Serial |
| **ACR Container** | Full VFS + Dalvik VM | Full VFS + Dalvik VM | Shared crate ready |
| **Tested Environment** | QEMU x86_64 PC | QEMU virt Cortex-A53 | QEMU virt RV64 |

---

## 9. Known Limitations & Ongoing Engineering Work

1. **DMA / IOMMU**: Device DMA currently relies on physical frame allocation from a reserved DMA region. Full SMMU / IOMMU page table integration for hardware peripherals is ongoing.
2. **GPU Acceleration**: Display rendering utilizes linear software framebuffers (`SimpleFB`). Hardware-accelerated DRM/KMS Vulkan driver stacks are planned for future revisions.
3. **Physical Smartphone Flashing**: Fastboot v2 Android boot images (`boot.img`, `boot-aarch64.img`) compile and package correctly. Physical device testing is currently targeting PinePhone (Allwinner A64) development boards.
