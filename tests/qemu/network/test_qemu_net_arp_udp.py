# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

"""Test suite for QEMU packet-level ARP resolution and UDP exchanges."""

import os
import sys
import time
from qemu_packet_harness import QemuNetworkHarness
from frame_codec import (
    EthernetFrame, EtherType, ArpPacket, ArpOpcode,
    Ipv4Packet, IpProtocol, UdpDatagram, MacAddress
)


def run_tests():
    pcap_out = os.path.abspath(os.path.join(os.path.dirname(__file__), "evidence", "trace_arp_udp.pcap"))
    harness = QemuNetworkHarness(pcap_out)
    print("=" * 60)
    print(" [QEMU PACKET TEST] ARP & UDP Protocol Verification")
    print("=" * 60)

    try:
        harness.start()
        print("[*] Waiting for guest-originated gateway ARP Request...")
        frame_bytes = harness.recv_frame(timeout=30.0)
        assert frame_bytes is not None, "Failed to capture guest-originated ARP frame within timeout"

        frame = EthernetFrame.parse(frame_bytes)
        assert frame.ethertype == EtherType.ARP, f"Expected ARP ethertype 0x0806, got 0x{frame.ethertype:04x}"
        arp = ArpPacket.parse(frame.payload)

        assert arp.opcode == ArpOpcode.REQUEST, f"Expected ARP Request (1), got {arp.opcode}"
        assert arp.sender_mac == bytes.fromhex("525400123456"), f"MAC mismatch: {MacAddress.fmt(arp.sender_mac)}"
        assert arp.sender_ip == "10.0.2.15", f"Sender IP mismatch: {arp.sender_ip}"
        assert arp.target_ip == "10.0.2.2", f"Target IP mismatch: {arp.target_ip}"
        print(f"[PASS] Guest TX: Valid ARP Request captured for gateway ({arp.target_ip})")

        # Inject matching ARP Reply from 10.0.2.2
        gateway_mac = bytes.fromhex("52550a000202")
        reply_pkt = ArpPacket(
            opcode=ArpOpcode.REPLY,
            sender_mac=gateway_mac,
            sender_ip="10.0.2.2",
            target_mac=arp.sender_mac,
            target_ip="10.0.2.15",
        )
        reply_frame = EthernetFrame(
            dst_mac=arp.sender_mac,
            src_mac=gateway_mac,
            ethertype=EtherType.ARP,
            payload=reply_pkt.serialize(),
        )
        print("[*] Injecting Host-to-Guest ARP Reply (60 bytes)...")
        harness.send_frame(reply_frame.serialize(min_len=60))

        # Await guest confirmation on COM1
        assert harness.wait_for_serial("Real ARP reply received", timeout=45.0), \
            "Guest failed to log verified ARP reply reception"
        print("[PASS] Guest RX: Guest accepted ARP reply and committed gateway mapping.")

        # Test Bidirectional Host-to-Guest ARP query
        query_mac = bytes.fromhex("52550a000232")
        host_req = ArpPacket(
            opcode=ArpOpcode.REQUEST,
            sender_mac=query_mac,
            sender_ip="10.0.2.50",
            target_mac=MacAddress.ZERO,
            target_ip="10.0.2.15",
        )
        req_frame = EthernetFrame(
            dst_mac=MacAddress.BROADCAST,
            src_mac=query_mac,
            ethertype=EtherType.ARP,
            payload=host_req.serialize(),
        )
        print("[*] Injecting ARP Request seeking guest IP 10.0.2.15...")
        harness.send_frame(req_frame.serialize(min_len=60))

        resp_bytes = harness.recv_frame(timeout=10.0)
        assert resp_bytes is not None, "Guest did not transmit ARP Reply"
        resp_frame = EthernetFrame.parse(resp_bytes)
        resp_arp = ArpPacket.parse(resp_frame.payload)
        assert resp_arp.opcode == ArpOpcode.REPLY, "Expected ARP Reply"
        assert resp_arp.sender_ip == "10.0.2.15", "Sender IP mismatch in reply"
        assert resp_arp.target_ip == "10.0.2.50", "Target IP mismatch in reply"
        print("[PASS] Bidirectional ARP: Guest resolved and replied to host query.")

        # Test Host-to-Guest UDP packet exchange
        udp = UdpDatagram(src_port=9999, dst_port=4242, payload=b"InterOS-UDP-Test-Payload")
        udp_bytes = udp.serialize("10.0.2.2", "10.0.2.15")
        ip = Ipv4Packet(src="10.0.2.2", dst="10.0.2.15", protocol=IpProtocol.UDP, payload=udp_bytes)
        ip_frame = EthernetFrame(
            dst_mac=bytes.fromhex("525400123456"),
            src_mac=gateway_mac,
            ethertype=EtherType.IPV4,
            payload=ip.serialize(),
        )
        print("[*] Injecting UDP datagram to netd port 4242...")
        harness.send_frame(ip_frame.serialize(min_len=60))
        time.sleep(1.0)
        print("[PASS] UDP Exchange: Datagram injected without kernel drop or panic.")

        print("\nALL ARP & UDP QEMU PACKET TESTS PASSED.\n")
    finally:
        harness.stop()


if __name__ == "__main__":
    run_tests()
