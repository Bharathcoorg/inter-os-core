# InterOS Core — VFS & Network Threat Model

**Document Version:** 1.1.0  
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
3. **Kernel / Hardware DMA Boundary**: The network controller (E1000) performs DMA to physical RAM. Descriptors and buffer ownership are arbitrated via status bits, volatile accesses, and hardware memory barriers.
4. **Link-Local Network Boundary**: Broadcast and multicast traffic from the local network segment is untrusted. NDP, ARP, and ICMPv6 packets are validated against spoofing, DoS, and cache poisoning.
5. **Remote Network Boundary**: Off-link IP packets and DNS responses are untrusted. Fragments are dropped, and DNS answers must strictly match in-flight query names.

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
| **THREAT-NET-01** | **E1000 DMA Memory Corruption & Stride Mismatch**<br>NIC DMAs up to 2048 bytes into 1514-byte descriptor buffers, or unaligned stride crosses page boundary. | Critical | Allocates 8 pages (32 KiB) for 16 RX buffers strided at 2048 bytes; synchronizes `transmit()` to compute buffers with `TX_BUFFER_SIZE` (2048). Enforces `Ordering::SeqCst` memory fences. |
| **THREAT-NET-02** | **E1000 DMA Race & Non-EOP Deadlock**<br>NIC overwrites receive buffer while CPU reads packet, or multi-descriptor packets deadlock RX ring. | High | Copy-before-recycle: copies payload before recycling descriptor. Non-EOP detector recycles descriptor, updates `RDT`, advances `rx_index`, and reports `InvalidPacket` to eliminate ring freeze. |
| **THREAT-NET-03** | **NDP Off-Link Spoofing & Malformed Options**<br>Off-link attacker injects fake Neighbor Advertisement, or corrupt options bypass validation. | High | RFC 4861 strict Hop Limit == 255 and Code == 0 enforcement in live receive dispatch. Fail-closed option parsing: SLLA allowed only on NS, TLLA only on NA, lengths verified to 8 bytes. |
| **THREAT-NET-04** | **Neighbor Cache Poisoning & DoS**<br>Injecting multicast MAC or ignoring Override flag to redirect traffic; unbounded routing table growth. | Medium | Validates unicast MAC; honors RFC 4861 Override (O) flag; evicts closest-to-expiry on saturation. Deduplicates routing table entries on prefix collisions. |
| **THREAT-NET-05** | **ARP Cache Gateway Eviction & MAC Spoofing**<br>Flooding ARP requests to evict default gateway; inconsistent Ethernet vs ARP MACs. | Medium | Pinned gateway with `is_permanent` flag preventing eviction or downgrade under saturation. Cross-layer consistency drops frames where `frame.src_mac != arp.sender_mac`. |
| **THREAT-NET-06** | **IPv4 Fragment Reassembly Evasion**<br>Sending fragments to bypass port or protocol filtering. | Medium | Explicitly drops any IPv4 packet with More Fragments (MF) set or non-zero fragment offset. Rejects multicast/broadcast source addresses. |
| **THREAT-NET-07** | **Kaminsky DNS Cache Poisoning**<br>Injecting fake A records for unrequested domains in DNS answers; spoofing from rogue servers. | High | Verifies that question section of received DNS answer strictly matches the active queried domain name. Enforces responses only from configured gateway resolver `10.0.2.3:53`. |
| **THREAT-NET-08** | **TCP Retransmission Starvation**<br>Maxed-out retransmission slot halting queue traversal. | Medium | Loop replaces slot with `None` and continues scanning remaining slots. |

---

## 4. Residual Risks & Next Steps
- **Hardware Cryptographic RNG**: Continued integration with physical TRNG when physical hardware target is activated.
- **Hardware TCP/UDP Checksum Offload**: Optional future enhancement for gigabit line-rate throughput once virtio-net is added alongside E1000.
