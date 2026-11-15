# InterOS Core — VFS & Network Threat Model

**Document Version:** 1.0.0  
**Audit Scope:** Virtual Filesystem (VFS) and Network Stack  
**Classification:** Threat Model & Security Architecture  
**Status:** Canonical Reference  

---

## 1. Overview & Trust Boundaries

The InterOS security architecture is designed around zero ambient authority, object capabilities, hardware-enforced memory isolation, and pure `#![no_std]` Rust memory safety.

```
       +-------------------------------------------------------+
       |             Sandboxed Applications (.rapp)            |
       +-------------------------------------------------------+
                | Syscalls (Pointer validation & CapHandle)
       +--------v----------------------------------------------+
       |                InterOS Microkernel Core                |
       |  +--------------------+      +---------------------+  |
       |  |  Capability Broker |      | Task & Memory Mgmt  |  |
       |  +--------------------+      +---------------------+  |
       |  +--------------------+      +---------------------+  |
       |  | Virtual Filesystem |      |  Network Stack      |  |
       |  +--------------------+      +---------------------+  |
       +--------+---------------------------------+------------+
                | Inode Spill / Storage I/O       | DMA Rings & Descriptors
       +--------v--------+             +----------v------------+
       |   Block Device  |             |  PCI NIC (E1000)      |
       | (InterFS / ATA) |             |  (Ethernet PHY)       |
       +-----------------+             +-----------------------+
```

### Key Trust Boundaries:
1. **Application / Kernel Boundary**: Applications run in unprivileged user mode (Ring 3 / EL0 / U-mode). All interactions occur via syscall traps. Syscall parameters (pointers, lengths, offsets) are treated as untrusted and strictly validated for userspace boundaries and integer overflow.
2. **Inter-Application Boundary**: Applications cannot communicate directly or access each other's memory or storage. Storage is segregated by reverse-DNS package identifier under `/user/<uid>/apps/<pkg>`.
3. **Kernel / Hardware DMA Boundary**: The network controller (E1000) performs DMA to physical RAM. Descriptors and buffer ownership are arbitrated via status bits and hardware memory barriers.
4. **Link-Local Network Boundary**: Broadcast and multicast traffic from the local network segment is untrusted. NDP, ARP, and ICMPv6 packets are validated against spoofing, DoS, and cache poisoning.
5. **Remote Network Boundary**: Off-link IP packets and DNS responses are untrusted. Fragments are dropped, and DNS answers must strictly match query names.

---

## 2. Threat Actor Classes

| Threat Actor | Capabilities | Objectives |
| :--- | :--- | :--- |
| **Malicious Local App** | Unprivileged userspace code execution; capability requests; worker thread spawning. | Escape sandbox; read/modify other apps' data; crash kernel (DoS); consume kernel heap. |
| **Adjacent Link Attacker** | Sends arbitrary Ethernet frames, ARP packets, and IPv6 NDP messages on local LAN. | MitM network traffic; poison ARP/NDP caches; blackhole traffic via gateway eviction. |
| **Remote Internet Attacker**| Sends unsolicited IP datagrams; responds to DNS queries with forged records. | Poison DNS cache (Kaminsky attack); fragment evasion; exploit IP parser flaws. |
| **Untrusted Device / DMA** | Emulated PCI controller modifying memory. | DMA buffer overrun; race condition exploitation on descriptor recycling. |

---

## 3. Threat Analysis & Mitigations

### 3.1 VFS Subsystem

| Threat ID | Threat Description | Severity | Mitigation in InterOS Core |
| :--- | :--- | :--- | :--- |
| **THREAT-VFS-01** | **Path Traversal / Escape**<br>Using `..` or leading slashes to escape sandbox. | High | Strict path canonicalization: strips duplicate slashes, rejects `..` components, enforces `/user/<uid>/apps/<pkg>` root. |
| **THREAT-VFS-02** | **Worker Thread Sandbox Escape**<br>Spawning thread named `"user_thread"` to bypass app package identity checks. | High | `app_package_for_task` resolves the process group leader in scheduler task table when child thread requests storage. |
| **THREAT-VFS-03** | **Pre-Authorization Inode Bombing**<br>Calling `open(CREATE)` without write capability to pollute VFS with phantom files. | Medium | Atomic rollback: `open` tracks `newly_created` and calls `self.unlink(&normalized)` if capability check or descriptor allocation fails. |
| **THREAT-VFS-04** | **Directory Hierarchy Loops**<br>Renaming directory into its own subdirectory, causing infinite recursion. | High | `rename` verifies that destination path is not prefixed by source path. |
| **THREAT-VFS-05** | **Illegal Inode Modification**<br>Unlinking or truncating directory, system config, or `/dev/*` nodes. | High | `unlink` and `truncate` strictly enforce file types, rejecting directories with `IsADirectory` and read-only nodes. |
| **THREAT-VFS-06** | **64-bit Integer Overflow in Syscalls**<br>Passing `u64::MAX` offset to wrap pointers during `FsRead`/`FsWrite`. | High | `checked_add` bounds checking on all cursor calculations in `sys_dispatch`. |

### 3.2 Network Subsystem

| Threat ID | Threat Description | Severity | Mitigation in InterOS Core |
| :--- | :--- | :--- | :--- |
| **THREAT-NET-01** | **E1000 DMA Memory Corruption**<br>NIC DMAs up to 2048 bytes into 1514-byte descriptor buffers. | Critical | Allocates 8 pages (32 KiB) for 16 descriptors strided at 2048 bytes, matching hardware `BSIZE = 2048`. |
| **THREAT-NET-02** | **E1000 DMA Race Condition**<br>NIC overwrites receive buffer while CPU reads packet. | High | Copy-before-recycle: kernel copies payload to buffer before clearing descriptor status and advancing `RDT`. |
| **THREAT-NET-03** | **NDP Off-Link Spoofing**<br>Off-link attacker injects fake Neighbor Advertisement. | High | RFC 4861 strict Hop Limit == 255 verification; packets with Hop Limit < 255 are dropped immediately. |
| **THREAT-NET-04** | **Neighbor Cache Poisoning**<br>Injecting multicast MAC or ignoring Override flag to redirect traffic. | Medium | Validates unicast MAC; honors RFC 4861 Override (O) flag; evicts closest-to-expiry on saturation. |
| **THREAT-NET-05** | **ARP Cache Gateway Eviction DoS**<br>Flooding ARP requests to evict slot 0 (default gateway). | Medium | Cache saturation evicts entry with minimum `expires_at`, preserving live default gateway. |
| **THREAT-NET-06** | **IPv4 Fragment Reassembly Evasion**<br>Sending fragments to bypass port or protocol filtering. | Medium | Explicitly drops any IPv4 packet with More Fragments (MF) set or non-zero fragment offset. |
| **THREAT-NET-07** | **Kaminsky DNS Cache Poisoning**<br>Injecting fake A records for unrequested domains in DNS answers. | High | `parse_a_answer` requires answer RR name to match query name (pointer or literal check). |
| **THREAT-NET-08** | **TCP Retransmission Starvation**<br>Maxed-out retransmission slot halting queue traversal. | Medium | Loop replaces slot with `None` and continues scanning remaining slots. |

---

## 4. Residual Risks & Next Steps
- **Hardware Cryptographic RNG**: Continued integration with physical TRNG when physical hardware target is activated.
- **Hardware TCP/UDP Checksum Offload**: Optional future enhancement for gigabit line-rate throughput once virtio-net is added alongside E1000.
