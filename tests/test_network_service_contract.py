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
        "std::net_sendto([127, 0, 0, 1], 4242",
        "std::net_recvfrom(socket",
        "std::tcp_connect([127, 0, 0, 1], 4243, 4244, 1)",
        "std::EVENT_NETWORK_PACKET",
    ):
        must(svc, needle, needle)
    must(bin_, "interos_system_service::run_network()", "netd entrypoint")
    must(sdk, "NetworkPoll = 93", "network poll syscall ABI")
    must(sdk, "pub fn network_poll()", "network poll SDK")
    must(sdk, "TcpConnect = 94", "TCP connect syscall ABI")
    must(sdk, "pub fn tcp_connect(", "TCP connect SDK")
    must(syscall, "pub fn sys_network_poll()", "kernel network poll")
    must(syscall, "sock.owner_task_id != current_id.0 as u32", "socket owner isolation")
    must(syscall, "Ipv4Address::LOCALHOST", "loopback source selection")
    must(syscall, "DEFAULT_HOST_IP", "non-loopback source selection")
    must(syscall, "pub fn sys_tcp_connect(", "TCP connect kernel API")
    must(syscall, "94 => match sys_tcp_connect", "TCP connect dispatcher")
    must(syscall, "93 => { match sys_network_poll()", "network poll dispatcher")
    must(user, "CapabilityType::NetworkSend", "netd TX capability")
    must(user, "CapabilityType::NetworkReceive", "netd RX capability")
    tcp = (ROOT / "kernel/src/net/tcp.rs").read_text()
    dns = (ROOT / "kernel/src/net/dns.rs").read_text()
    must(tcp, "serialize_with_checksum", "TCP checksum serialization")
    must(tcp, "tcp_checksum", "TCP pseudo-header checksum")
    must(tcp, "SynReceived", "TCP state machine")
    must(dns, "encode_query", "DNS query encoder")
    must(dns, "parse_a_answer", "DNS A-record parser")
    print("userspace network service contract: PASS")

if __name__ == "__main__":
    main()
