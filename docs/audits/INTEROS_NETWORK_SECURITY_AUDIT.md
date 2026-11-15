# InterOS Core — Network Subsystem Security Architecture & Hardening Audit

**Document Version:** 1.0.0  
**Audit Target:** `kernel/src/net/` (`ipv6.rs`, `ipv4.rs`, `arp.rs`, `dns.rs`, `tcp.rs`, `e1000.rs`)  
**Classification:** Core Operating System Security Audit  
**Status:** Remediated & Verified  

---

## 1. Executive Summary

An adversarial security audit of the InterOS Core network stack and network interface driver was conducted. The audit analyzed protocol compliance, packet validation, memory safety in DMA paths, cache poisoning vulnerabilities, and denial-of-service resilience across IPv6, IPv4, ARP, TCP, DNS, and the Intel 82540EM (E1000) PCI network driver.

Prior to remediation, multiple critical security weaknesses were discovered:
1. **E1000 Hardware DMA Buffer Overrun**: Driver configured hardware RCTL with `BSIZE = 2048` (00b), but allocated only 6 pages (24 KiB) and strided descriptors by 1514 bytes. Packet writes exceeding 1514 bytes triggered out-of-bounds DMA corrupting adjacent memory.
2. **E1000 Receive Descriptor Recycling Race**: `receive()` released descriptors and updated `RDT` before copying packet payloads, permitting the NIC to overwrite packet data while the kernel was reading it.
3. **Missing NDP Hop Limit Validation (RFC 4861)**: Neighbor Solicitation, Neighbor Advertisement, and Router Advertisement packets did not verify `hop_limit == 255`, allowing off-link attackers to inject spoofed NDP packets.
4. **Missing ICMPv6 Checksum Engine**: Checksum verification was absent or incomplete for incoming ICMPv6 messages.
5. **Routing Table Saturation DoS**: IPv6 routing table appended duplicate prefix entries without deduplication, enabling table exhaustion.
6. **Neighbor Cache Poisoning & Expiry Inversion**: Neighbor cache accepted multicast MACs and failed to honor the RFC 4861 Override flag.
7. **IPv4 Fragment Injection**: Incomplete handling of IP fragments allowed evasion of upper-layer security checks.
8. **Deterministic ARP Gateway Eviction DoS**: Saturated ARP cache evicted slot 0 unconditionally, allowing attackers on the local link to trivially evict the default gateway.
9. **Kaminsky DNS Cache Poisoning**: DNS answer parser accepted A-record answers without verifying that the answer RR matched the domain name in the question section.
10. **TCP Retransmission Queue Starvation**: `TcpReliability::due()` halted queue traversal upon encountering a slot with maximum retransmissions, starving all subsequent slots.

All identified vulnerabilities have been remediated in pure `#![no_std]` Rust with zero dynamic allocations in packet processing loops.

---

## 2. E1000 Hardware Driver & DMA Hardening

### 2.1 Hardware Buffer Size Alignment
The Intel 82540EM Gigabit Ethernet controller hardware interprets receive control register `RCTL.BSIZE` bits `[17:16] = 00b` as 2048-byte buffer sizes. Previously, `E1000DmaResources` allocated 6 pages (24 KiB) for 16 descriptors strided at 1514 bytes. When an MTU 1500 frame plus preamble, CRC, and padding arrived, the hardware DMA controller wrote up to 2048 bytes, overflowing descriptor boundaries by 534 bytes.

**Remediation**:
- Defined `RX_BUFFER_SIZE = 2048;`
- Increased DMA buffer allocation from 6 to 8 frames (32 KiB) in `E1000DmaResources`.
- Strided descriptors strictly by 2048 bytes:
```rust
for (i, desc) in self.rx_ring.descriptors.iter_mut().enumerate() {
    let addr = self.rx_buffer_phys + (i * RX_BUFFER_SIZE) as u64;
    desc.buffer_addr = addr;
    desc.status = 0;
}
```

### 2.2 Atomic Copy-Before-Recycle
Previously, `receive()` wrote `RDT = idx` and cleared descriptor status before copying packet data into the caller's buffer. If network traffic was heavy, the E1000 DMA engine could DMA a new packet into the same buffer while the CPU was executing the copy loop.

**Remediation**:
Packet data is now completely copied out to the kernel buffer *before* resetting descriptor status and advancing `RDT`:
```rust
out[..len].copy_from_slice(&buf[..len]);
core::sync::atomic::fence(core::sync::atomic::Ordering::Release);
desc.status = 0;
self.write_reg(REG_RDT, idx as u32);
```

---

## 3. IPv6 & Neighbor Discovery Protocol (NDP)

### 3.1 Strict Hop Limit == 255 Enforcement
RFC 4861 requires that all NDP packets (Router Solicitation, Router Advertisement, Neighbor Solicitation, Neighbor Advertisement, Redirect) have an IPv6 Hop Limit of 255. Any packet with Hop Limit < 255 was forwarded by a router and originated from off-link; it must be discarded immediately.

```rust
if hop_limit != 255 {
    return Err(NetError::InvalidPayload);
}
```

### 3.2 RFC 4443 / RFC 8200 ICMPv6 Pseudo-Header Checksum
Implemented standard IPv6 pseudo-header checksum calculation and validation:
- Source and Destination IPv6 addresses (16 bytes each)
- Upper-Layer Packet Length (32-bit big endian)
- Next Header = 58 (ICMPv6)
- One's complement sum over pseudo-header and ICMPv6 message.

### 3.3 Neighbor Cache Hardening
- **MAC Validation**: Rejects multicast MAC (`mac[0] & 1 != 0`) and all-zero MAC.
- **IP Validation**: Rejects multicast (`ff00::/8`) and unspecified (`::`) neighbor addresses.
- **Override Flag (RFC 4861 § 7.2.5)**: If an advertisement arrives with a different MAC for an existing entry, the MAC is updated only if the `Override` (O) flag is set.
- **LRU/Expiry Eviction**: When the cache is full, the entry closest to expiration is evicted instead of evicting deterministic or arbitrary slots.

---

## 4. IPv4, ARP, TCP & DNS Hardening

### 4.1 IPv4 Fragment Rejection & Source Validation
InterOS does not support IPv4 fragment reassembly. To prevent fragment-based firewall and capability bypass attacks, fragmented datagrams are explicitly dropped:
```rust
let frag_flags_offset = u16::from_be_bytes([buf[6], buf[7]]);
if (frag_flags_offset & 0x3FFF) != 0 {
    return Err("IPv4 fragments not supported; packet dropped");
}
```
Multicast (`>= 224.0.0.0`) and broadcast (`255.255.255.255`) source IP addresses are rejected as invalid.

### 4.2 ARP Cache DoS Prevention
Previously, when `ArpTable` reached capacity (16 entries), it always evicted entry 0. Since the default gateway is typically resolved first during boot and stored in slot 0, local attackers could send bogus ARP replies to immediately evict the gateway.
**Remediation**:
`ArpTable` now scans all entries and evicts the entry with the minimum `expires_at` timestamp.

### 4.3 DNS Kaminsky Cache Poisoning Mitigation
The DNS parser previously validated only the transaction ID and extracted the first A record in the answer section. Attackers could spoof DNS responses containing arbitrary records.
**Remediation**:
`parse_a_answer()` verifies that each answer RR name exactly matches the question name (either via compression pointer `0xC00C` back to the question or literal label comparison). Answers for unasked domains are rejected.

### 4.4 TCP Retransmission Starvation Bug
In `TcpReliability::due()`, encountering an exhausted slot previously executed `return None;`, halting the scan and starving any subsequent pending slots.
**Remediation**:
Replaced early return with `*slot = None; continue;`, allowing all pending retransmissions in the queue to be serviced.

---

## 5. Verification & Evidence

All network security fixes were validated through automated unit tests:
- `net::ipv6::tests` (10 tests)
- `net::ipv4::tests` (3 tests)
- `net::arp::tests` (3 tests)
- `net::dns::*` (5 tests)
- `net::tcp::tests` (3 tests)
- `net::socket::tcp_reliability_tests` (10 tests)

Total network tests passing: **34 core network unit tests**, 0 failures.
