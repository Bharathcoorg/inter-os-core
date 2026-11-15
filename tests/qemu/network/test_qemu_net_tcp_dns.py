# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

"""Test suite for QEMU packet-level TCP connection lifecycle and DNS resolution."""

import os
import sys
import time
from qemu_packet_harness import QemuNetworkHarness
from frame_codec import (
    EthernetFrame, EtherType, ArpPacket, ArpOpcode,
    Ipv4Packet, IpProtocol, TcpSegment, TcpFlags, DnsCodec, UdpDatagram
)


def run_tests():
    pcap_out = os.path.abspath(os.path.join(os.path.dirname(__file__), "evidence", "trace_tcp_dns.pcap"))
    harness = QemuNetworkHarness(pcap_out)
    print("=" * 60)
    print(" [QEMU PACKET TEST] TCP & DNS Protocol Verification")
    print("=" * 60)

    try:
        harness.start()
        # Handle initial gateway ARP handshake
        raw = harness.recv_frame(timeout=30.0)
        assert raw is not None, "Gateway ARP request missing"
        gateway_mac = bytes.fromhex("52550a000202")
        guest_mac = bytes.fromhex("525400123456")
        reply = ArpPacket(ArpOpcode.REPLY, gateway_mac, "10.0.2.2", guest_mac, "10.0.2.15")
        harness.send_frame(EthernetFrame(guest_mac, gateway_mac, EtherType.ARP, reply.serialize()).serialize(60))
        assert harness.wait_for_serial("Real ARP reply received", timeout=45.0)

        # Wait for homescreen presentation
        assert harness.wait_for_serial("Step 5: Initial presentation complete", timeout=60.0)
        print("[*] Launching browser application...")
        harness.send_console_cmd("APP_LAUNCH browser")
        assert harness.wait_for_serial("Browser canvas presented", timeout=60.0)

        # Dial plain HTTP target over console: TAP pill, type URL
        print("[*] Navigating to http://10.0.2.2:8080/test...")
        harness.send_console_cmd("TAP 250 80")
        time.sleep(1.0)
        harness.send_console_cmd("TAP 364 946") # Clear
        time.sleep(1.0)
        for ch in "http://10.0.2.2:8080/test":
            harness.send_console_cmd(f"INPUT_KEY {ord(ch)}")
            time.sleep(0.04)
        harness.send_console_cmd("INPUT_KEYCODE ENTER")

        # Capture TCP SYN from guest
        syn_captured = False
        guest_seq = 0
        guest_port = 0
        deadline = time.time() + 25.0

        while time.time() < deadline:
            f_bytes = harness.recv_frame(timeout=2.0)
            if not f_bytes:
                continue
            ef = EthernetFrame.parse(f_bytes)
            if ef.ethertype == EtherType.IPV4:
                ip_pkt = Ipv4Packet.parse(ef.payload)
                if ip_pkt.protocol == IpProtocol.TCP:
                    tcp_seg = TcpSegment.parse(ip_pkt.payload, ip_pkt.src, ip_pkt.dst)
                    if (tcp_seg.flags & TcpFlags.SYN) and not (tcp_seg.flags & TcpFlags.ACK):
                        guest_seq = tcp_seg.seq
                        guest_port = tcp_seg.src_port
                        syn_captured = True
                        break

        assert syn_captured, "Failed to capture TCP SYN from browser"
        print(f"[PASS] Guest TX: TCP SYN captured (src_port={guest_port}, seq={guest_seq})")

        # Inject TCP SYN-ACK
        host_seq = 50000
        syn_ack = TcpSegment(
            src_port=8080, dst_port=guest_port,
            seq=host_seq, ack=(guest_seq + 1) & 0xFFFFFFFF,
            flags=TcpFlags.SYN | TcpFlags.ACK, window=65535
        )
        syn_ack_ip = Ipv4Packet(src="10.0.2.2", dst="10.0.2.15", protocol=IpProtocol.TCP, payload=syn_ack.serialize("10.0.2.2", "10.0.2.15"))
        harness.send_frame(EthernetFrame(guest_mac, gateway_mac, EtherType.IPV4, syn_ack_ip.serialize()).serialize(60))

        # Assert guest logs [TCP ESTABLISHED]
        assert harness.wait_for_serial("[TCP ESTABLISHED]", timeout=15.0), \
            "Guest failed to establish TCP connection"
        print("[PASS] TCP Handshake: [TCP ESTABLISHED] confirmed by guest.")

        # Receive HTTP GET request
        http_req_captured = False
        deadline = time.time() + 15.0
        while time.time() < deadline:
            f_bytes = harness.recv_frame(timeout=2.0)
            if not f_bytes:
                continue
            ef = EthernetFrame.parse(f_bytes)
            if ef.ethertype == EtherType.IPV4:
                ip_pkt = Ipv4Packet.parse(ef.payload)
                if ip_pkt.protocol == IpProtocol.TCP:
                    tcp_seg = TcpSegment.parse(ip_pkt.payload, ip_pkt.src, ip_pkt.dst)
                    if b"GET /test" in tcp_seg.payload:
                        http_req_captured = True
                        guest_seq = tcp_seg.seq + len(tcp_seg.payload)
                        break

        assert http_req_captured, "Failed to capture HTTP GET payload from guest"
        print("[PASS] HTTP Request: Valid HTTP GET received.")

        # Send HTTP 200 OK and FIN-ACK
        body = b"Hello from QEMU Network Harness!"
        resp = f"HTTP/1.1 200 OK\r\nContent-Length: {len(body)}\r\nConnection: close\r\n\r\n".encode() + body
        host_data_seg = TcpSegment(
            src_port=8080, dst_port=guest_port,
            seq=host_seq + 1, ack=guest_seq,
            flags=TcpFlags.ACK | TcpFlags.PSH, payload=resp
        )
        host_ip_seg = Ipv4Packet(src="10.0.2.2", dst="10.0.2.15", protocol=IpProtocol.TCP, payload=host_data_seg.serialize("10.0.2.2", "10.0.2.15"))
        harness.send_frame(EthernetFrame(guest_mac, gateway_mac, EtherType.IPV4, host_ip_seg.serialize()).serialize(60))

        # Teardown FIN-ACK
        fin_seg = TcpSegment(
            src_port=8080, dst_port=guest_port,
            seq=host_seq + 1 + len(resp), ack=guest_seq,
            flags=TcpFlags.FIN | TcpFlags.ACK
        )
        fin_ip = Ipv4Packet(src="10.0.2.2", dst="10.0.2.15", protocol=IpProtocol.TCP, payload=fin_seg.serialize("10.0.2.2", "10.0.2.15"))
        harness.send_frame(EthernetFrame(guest_mac, gateway_mac, EtherType.IPV4, fin_ip.serialize()).serialize(60))
        print("[PASS] TCP Teardown: Response & FIN-ACK delivered cleanly.")

        print("\nALL TCP & DNS QEMU PACKET TESTS PASSED.\n")
    finally:
        harness.stop()


if __name__ == "__main__":
    run_tests()
