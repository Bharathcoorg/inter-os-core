#!/usr/bin/env python3
"""M32 userspace network service contracts."""
from pathlib import Path
ROOT = Path(__file__).resolve().parents[1]

def must(text, needle, label):
    assert needle in text, f"missing {label}: {needle}"

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
        "std::net_send_socket(socket, [127, 0, 0, 1], 4242,
        "std::net_recvfrom(socket",
        "std::tcp_connect([127, 0, 0, 1], 4243, 4244, 1)",
        "std::EVENT_NETWORK_PACKET",
    ):
        must(svc, needle, needle)
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
    must(syscall, "pub fn sys_network_poll()", "kernel network poll")
    must(syscall, "sock.owner_task_id != current_id.0 as u32", "socket owner isolation")
    must(syscall, "pub fn sys_net_send_socket(", "socket-bound send kernel API")
    must(syscall, "pub fn sys_net_close_socket(", "socket close kernel API")
    must(syscall, "release_owner", "task socket cleanup")
    must(syscall, "copy_from_user(&mut payload[..len]", "network payload user validation")
    must(syscall, "Ipv4Address::LOCALHOST", "loopback source selection")
    must(syscall, "DEFAULT_HOST_IP", "non-loopback source selection")
    must(syscall, "pub fn sys_tcp_connect(", "TCP connect kernel API")
    must(syscall, "pub fn sys_tcp_send(", "TCP send kernel API")
    must(syscall, "pub fn sys_tcp_recv(", "TCP receive kernel API")
    must(syscall, "97 =>", "TCP send dispatcher")
    must(syscall, "98 =>", "TCP receive dispatcher")
    must(syscall, "94 => match sys_tcp_connect", "TCP connect dispatcher")
    must(syscall, "93 => { match sys_network_poll()", "network poll dispatcher")
    must(syscall, "99 =>", "DNS resolver dispatcher")
    must(syscall, "pub fn sys_dns_resolve(", "runtime DNS resolver")
    must(syscall, "DnsResolver::new()", "DNS resolver state integration")
    must(syscall, "DNS_SERVER", "DNS server integration")
    must(user, "CapabilityType::NetworkSend", "netd TX capability")
    must(user, "CapabilityType::NetworkReceive", "netd RX capability")
    tcp = (ROOT / "kernel/src/net/tcp.rs").read_text()
    dns = (ROOT / "kernel/src/net/dns.rs").read_text()
    must(tcp, "serialize_with_checksum", "TCP checksum serialization")
    must(tcp, "pub fn seq_lt", "TCP wrapping sequence comparison")
    must(tcp, "pub fn seq_gt", "TCP wrapping ACK comparison")
    must(net, "crate::net::tcp::seq_gt(segment.ack, conn.send_seq)", "wrap-safe ACK validation")
    sock = (ROOT / "kernel/src/net/socket.rs").read_text()
    must(sock, "pub fn accept_rx_segment", "in-order TCP receive validation")
    must(sock, "seq != conn.recv_seq", "TCP sequence enforcement")
    must(net, "segment.ack != conn.send_seq.wrapping_add(1)", "SYN-ACK acknowledgement validation")
    must(tcp, "tcp_checksum", "TCP pseudo-header checksum")
    must(tcp, "SynReceived", "TCP state machine")
    must(net, "segment.flags & 0x12 == 0x12", "TCP SYN-ACK integration")
    must(net, "poll_tcp_retransmissions", "TCP retransmission scheduler")
    must(net, "acknowledge_tx", "TCP acknowledgement retirement")
    must(net, "transmit_tcp_payload(", "kernel-internal TCP retransmission path")
    must(net, "get_system_ticks()", "TCP retransmission timer source")
    must(sock, "TCP_MAX_RETRIES", "bounded TCP retry count")
    must(sock, "arm_retransmit", "TCP retransmission state")
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
