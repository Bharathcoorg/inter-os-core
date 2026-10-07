#!/usr/bin/env python3
"""Userspace network service contracts."""
from pathlib import Path
ROOT = Path(__file__).resolve().parents[1]

def must(text, needle, label):
    assert needle in text, f"missing {label}: {needle}"

def must_absent(text, needle, label):
    assert needle not in text, f"{label} (found {needle!r})"

def main():
    svc = (ROOT / "userspace/system-service/src/lib.rs").read_text()
    bin_ = (ROOT / "userspace/system-service/src/bin/netd.rs").read_text()
    sdk = (ROOT / "sdk/rust/interos-std/src/lib.rs").read_text()
    syscall = (ROOT / "kernel/src/syscall/mod.rs").read_text()
    user = (ROOT / "kernel/src/task/user.rs").read_text()

    for needle in (
        "pub fn run_network() -> !",
        "std::net_socket(4242)",
        "std::network_poll()",
        "std::net_recvfrom(socket",
        "std::EVENT_NETWORK_PACKET",
    ):
        must(svc, needle, needle)

    # A loopback self-send is not network traffic. netd previously transmitted
    # to its own bound socket, read the same datagram back out of the loopback
    # ring and logged it as an externally received packet. The contract is that
    # no such self-send exists and no external arrival is claimed while no
    # network device is attached.
    must_absent(svc, "net_send_socket(socket, [127, 0, 0, 1], 4242",
                "netd must not transmit to its own loopback socket")
    must_absent(svc, "External UDP packet",
                "netd must not report loopback traffic as external")
    must(bin_, "interos_system_service::run_network()", "netd entrypoint")
    must(sdk, "NetworkPoll = 93", "network poll syscall ABI")
    must(sdk, "pub fn network_poll()", "network poll SDK")
    must(sdk, "DnsResolve = 99", "DNS resolver syscall ABI")
    must(sdk, "pub fn dns_resolve(", "DNS resolver SDK")
    must(sdk, "pub fn net_send_socket(", "socket-bound send SDK")
    must(sdk, "NetCloseSocket = 96", "socket close syscall ABI")
    must(sdk, "pub fn net_close_socket(", "socket close SDK")
    must(sdk, "TcpConnect = 94", "TCP connect syscall ABI")
    must(sdk, "pub fn tcp_connect(", "TCP connect SDK")
    must(sdk, "pub fn tcp_send(", "TCP send SDK")
    must(sdk, "pub fn tcp_recv(", "TCP receive SDK")
    must(sdk, "TcpClose = 100", "TCP close syscall ABI")
    must(sdk, "pub fn tcp_close(", "TCP close SDK")
    must(syscall, "pub fn sys_network_poll()", "kernel network poll")
    must(syscall, "sock.owner_task_id != current_id.0 as u32", "socket owner isolation")
    must(syscall, "pub fn sys_net_send_socket(", "socket-bound send kernel API")
    must(syscall, "pub fn sys_net_close_socket(", "socket close kernel API")
    must(syscall, "release_owner", "task socket cleanup")
    must(syscall, "TCP_TABLE.lock().release_owner(owner)", "task TCP connection cleanup")
    must(syscall, "copy_from_user(&mut payload[..len]", "network payload user validation")
    must(syscall, "Ipv4Address::LOCALHOST", "loopback source selection")
    must(syscall, "DEFAULT_HOST_IP", "non-loopback source selection")
    must(syscall, "pub fn sys_tcp_connect(", "TCP connect kernel API")
    must(syscall, "pub fn sys_tcp_send(", "TCP send kernel API")
    must(syscall, "pub fn sys_tcp_recv(", "TCP receive kernel API")
    must(syscall, "pub fn sys_tcp_close(", "TCP close kernel API")
    must(syscall, "conn.state != crate::net::TcpState::Established && conn.state != crate::net::TcpState::CloseWait", "TCP close accepts passive-close state")
    must(syscall, "rollback_fin(connection_id)", "TCP FIN rollback on transmit failure")
    must(syscall, "100 =>", "TCP close dispatcher")
    must(syscall, "crate::net::transmit_tcp_segment_with_window(", "TCP FIN transmission")
    assert "crate::net::crate::net::transmit_tcp_segment" not in syscall, "TCP close must use the canonical network transport path"
    must(syscall, "97 =>", "TCP send dispatcher")
    must(syscall, "98 =>", "TCP receive dispatcher")
    must(syscall, "94 => match sys_tcp_connect", "TCP connect dispatcher")
    must(syscall, "93 => { match sys_network_poll()", "network poll dispatcher")
    must(syscall, "99 =>", "DNS resolver dispatcher")
    must(syscall, "pub fn sys_dns_resolve(", "runtime DNS resolver")
    must(syscall, "crate::net::DNS_RESOLVER.lock()", "persistent DNS resolver state integration")
    must(syscall, "fn send_dns_datagram_with_arp_retry(", "DNS ARP-aware transmission helper")
    must(syscall, "NetError::NoPacketAvailable", "DNS ARP retry classification")
    must(syscall, "DNS_SERVER", "DNS server integration")
    must(syscall, "let timed_out = resolver.query(query_id)", "DNS retry deadline check")
    must(syscall, "resolver.build_query(query_id, &mut packet)", "DNS retry query rebuild")
    must(syscall, "if resolver.query(query_id).is_some()", "DNS bounded retry gate")
    must(user, "CapabilityType::NetworkSend", "netd TX capability")
    must(user, "CapabilityType::NetworkReceive", "netd RX capability")
    tcp = (ROOT / "kernel/src/net/tcp.rs").read_text()
    dns = (ROOT / "kernel/src/net/dns.rs").read_text()
    net = (ROOT / "kernel/src/net/mod.rs").read_text()
    must(tcp, "serialize_with_checksum", "TCP checksum serialization")
    must(tcp, "pub fn seq_lt", "TCP wrapping sequence comparison")
    must(tcp, "pub fn seq_gt", "TCP wrapping ACK comparison")
    must(tcp, "seq_gt(segment.ack, snd_nxt)", "wrap-safe ACK validation")
    sock = (ROOT / "kernel/src/net/socket.rs").read_text()
    must(sock, "pub fn accept_rx_segment", "in-order TCP receive validation")
    must(sock, "seq != conn.recv_seq", "TCP sequence enforcement")
    must(net, "valid_syn_ack(&segment, conn.send_seq.wrapping_add(1))", "SYN-ACK acknowledgement validation")
    must(tcp, "tcp_checksum", "TCP pseudo-header checksum")
    must(tcp, "SynReceived", "TCP state machine")
    must(tcp, "segment.flags == (SYN | ACK)", "TCP SYN-ACK integration")
    must(net, "poll_tcp_retransmissions", "TCP retransmission scheduler")
    must(net, "lookup_at(dst_ip, now)", "bounded ARP cache lookup")
    must(net, "let now = crate::interrupts::get_system_ticks();", "ARP cache lookup timestamp")
    must(net, "ARP_REQUEST_RETRY_TICKS", "bounded ARP request retry interval")
    must(net, "ARP_REQUESTS.lock()", "ARP request state tracking")
    must(net, "requested_at", "ARP request timestamp tracking")
    must(net, "saturating_sub(entry.requested_at)", "ARP request retry deadline")
    must(net, "entry.valid = false", "ARP request state cleanup")

    must(net, "insert_at(arp_pkt.sender_ip, arp_pkt.sender_mac", "bounded ARP cache refresh")
    must((ROOT / "kernel/src/net/arp.rs").read_text(), "ARP_CACHE_TICKS", "ARP cache aging")
    must(net, "acknowledge_tx", "TCP acknowledgement retirement")
    must(net, "transmit_tcp_payload(", "kernel-internal TCP retransmission path")
    must(net, "transmit_tcp_syn(", "kernel-internal TCP SYN retransmission path")
    must(net, "get_system_ticks()", "TCP retransmission timer source")
    must((ROOT / "kernel/src/interrupts.rs").read_text(), "PIT_BASE_HZ", "PIT clock source")
    must((ROOT / "kernel/src/interrupts.rs").read_text(), "SYSTEM_TICK_HZ: u32 = 1_000", "defined system tick frequency")
    must((ROOT / "kernel/src/interrupts.rs").read_text(), "command.write(0x36u8)", "PIT channel 0 mode configuration")
    must((ROOT / "kernel/src/main.rs").read_text(), "interrupts::init_pit()", "boot-time PIT initialization")
    must(net, "conn.syn_pending = false", "SYN acknowledgement retires retransmission state")
    must(net, "conn.syn_retries = 0", "SYN retry state resets after handshake")
    assert "TCP_TABLE.lock().acknowledge_syn" not in net, "SYN-ACK path must not re-lock TCP_TABLE while its guard is held"
    assert "TCP_TABLE.lock().acknowledge_tx" not in net, "ACK path must not re-lock TCP_TABLE while its guard is held"
    assert "TCP_TABLE.lock().acknowledge_fin" not in net, "FIN ACK path must not re-lock TCP_TABLE while its guard is held"
    must(sock, "TCP_MAX_RETRIES", "bounded TCP retry count")
    must(sock, "arm_retransmit", "TCP retransmission state")
    must(sock, "arm_fin", "TCP FIN lifecycle")
    must(sock, "due_fin_retransmit", "TCP FIN retransmission")
    must(sock, "due_syn_retransmit", "TCP SYN retransmission")
    must(sock, "conn.state = crate::net::TcpState::Closed", "TCP payload retry exhaustion closes connection")
    must(sock, "conn.tx_len = 0", "TCP payload retry exhaustion clears retransmit buffer")
    must(sock, "payload_retransmit_exhaustion_closes_connection", "TCP payload exhaustion regression test")
    must(dns, "pub fn abort(&mut self, id: u16)", "DNS inflight cancellation")
    must(dns, "DNS query id space exhausted", "DNS transaction ID exhaustion guard")
    must(dns, "query_ids_skip_active_ids_after_wrap", "DNS transaction ID collision regression test")
    must(syscall, "DNS_RESOLVER.lock().abort(query_id)", "DNS failed-query lifecycle cleanup")
    must(sock, "acknowledge_syn", "TCP SYN acknowledgement retirement")
    must(sock, "if local_port == 0 || remote_port == 0", "TCP port validation")
    must(sock, "conn.local_port == local_port", "TCP tuple collision protection")
    must(sock, "conn.remote_ip == remote_ip", "TCP remote tuple matching")
    must(sock, "conn.state != crate::net::TcpState::Closed", "TCP closed-entry reuse")
    must(net, "else { return reaped; }", "TCP reaping accounting preservation")
    must(sock, "pub fn release_owner(&mut self, owner_task_id: u32)", "TCP owner cleanup")
    must(sock, "reserve_network_socket(process)", "TCP per-process socket quota reservation")
    must(sock, "release_network_socket(process)", "TCP per-process socket quota release")
    must(sock, "pub fn reap_closed(&mut self)", "TCP closed-entry reaping")
    must(net, "reap_closed()", "TCP closed-entry network-poll reaping")
    must(net, "transmit_tcp_segment(", "kernel TCP ACK transmission")
    must(dns, "encode_query", "DNS query encoder")
    must(dns, "parse_a_answer", "DNS A-record parser")
    must(dns, "pub struct DnsResolver", "DNS runtime resolver state")
    must(dns, "DNS_RETRY_TICKS", "DNS retry deadline")
    must(dns, "pub fn accept_response", "DNS response integration")
    must(dns, "pub fn poll_timeouts", "DNS timeout handling")
    must(dns, "DnsCacheEntry", "DNS TTL cache")
    print("userspace network service contract: PASS")

if __name__ == "__main__":
    main()
