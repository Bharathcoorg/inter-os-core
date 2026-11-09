# Security Policy

InterOS Core is an open-source research prototype of a smartphone operating system, running in QEMU only. It is not a commercial product. External contributions are not accepted at this stage and will open after the first stable release.

InterOS Core implements a zero-ambient-authority, capability-enforced microkernel architecture for mobile systems. Security is foundational to the design, from virtual privilege separation to userspace isolation and host-backed AI runtime boundaries.

## Supported Versions

| Version / Branch | Supported | Notes |
| :--- | :--- | :--- |
| `master` | Yes | Active canonical development branch |
| Tagged releases | Not applicable yet | No `v*` tag exists in this repository; security fixes land on `master` |
| Development branches | No | Experimental; not for production use |

## Reporting a Vulnerability

We take the security of InterOS Core seriously. If you discover a vulnerability or security defect, **please do not disclose it publicly or open a public GitHub issue**.

### Reporting Channel

Please report vulnerabilities using **GitHub Private Vulnerability Reporting**:

- **Advisory URL:** [Submit a Security Advisory](https://github.com/Bharathcoorg/inter-os-core/security/advisories/new)
- Alternatively, email the maintainer at `bharathcoorg7@gmail.com` with the subject tag `[SECURITY: InterOS]`.

### Information to Include

To help us triage and resolve the issue quickly, please provide:

1. **Description:** A clear summary of the vulnerability and its potential impact.
2. **Component:** Affected subsystem (e.g., `kernel`, `cap_table`, `ipc`, `aid`, `drivers`, `vfs`).
3. **Reproduction Steps:** Minimal proof-of-concept (PoC), test case, or step-by-step instructions.
4. **Environment:** Architecture (`x86_64`, `aarch64`, or `riscv64`), execution environment (canonical x86_64 QEMU emulator or QEMU virt), toolchain version.
5. **Mitigation / Suggested Fix:** If you have identified a fix or mitigation, please include details.

### Response Timeline

- **Acknowledgment:** Within 48 hours of receiving your report.
- **Triage & Reproduction:** Within 7 business days.
- **Remediation & Advisory:** We will coordinate a fix, verify it against test suites, and publish an advisory alongside the patched release.

---

## Threat Model & Security Architecture

InterOS Core enforces the following architectural security invariants:

1. **Zero Ambient Authority:**
   No process or thread receives ambient access to memory, hardware registers, VFS inodes, or IPC endpoints. All operations require an explicit, unforgeable `CapHandle` checked by the kernel capability broker.
2. **Process Isolation:**
   Kernel and userspace memory spaces are strictly segregated with architecture-specific page tables (PML4 on x86_64, TTBR0/TTBR1 on AArch64, satp on RISC-V 64). Userspace applications run at Ring 3 / EL0 / U-Mode.
3. **Safe Syscall Memory Handling:**
   All userspace pointers passed across syscall boundaries undergo strict range validation, alignment verification, and bounds enforcement before dereferencing.
4. **Untrusted Model Content:**
   AI inference outputs, embeddings, and retrieved context from local open models are treated as untrusted inputs. AI services (`aid`) execute in unprivileged userspace without ambient filesystem or networking authority.
5. **Host-Guest Boundary:**
   In development emulation, host-backed inference services connect over isolated virtio-serial channels with typed JSON-RPC framing and rate limiting.

### Out of Scope

The following threat vectors are considered out of scope for software security advisories:

- Attacks requiring physical access to the development host machine before hypervisor launch.
- Compromised host operating system or hypervisor under QEMU emulation.
- Denial of Service via intentional exhaustion of virtual machine CPU/RAM on development hosts.
