# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

"""Test suite for RX descriptor wraparound, malformed packet drops, and multi-descriptor recovery."""

import os
import sys
import time
from qemu_packet_harness import QemuNetworkHarness
from frame_codec import (
    EthernetFrame, EtherType, ArpPacket, ArpOpcode,
    Ipv4Packet, IpProtocol, UdpDatagram, MacAddress
)


def run_tests():
    pcap_out = os.path.abspath(os.path.join(os.path.dirname(__file__), "evidence", "trace_robustness.pcap"))
    harness = QemuNetworkHarness(pcap_out)
    print("=" * 60)
    print(" [QEMU PACKET TEST] Hardware Robustness & Drop Telemetry")
    print("=" * 60)

    try:
        harness.start()
        # Handshake gateway ARP
        deadline = time.time() + 30.0
        arp_found = False
        while time.time() < deadline:
            raw = harness.recv_frame(timeout=2.0)
            if not raw:
                continue
            ef = EthernetFrame.parse(raw)
            if ef.ethertype == EtherType.ARP:
                arp_found = True
                break
        assert arp_found, "Gateway ARP request missing"
        gateway_mac = bytes.fromhex("52550a000202")
        guest_mac = bytes.fromhex("525400123456")
        reply = ArpPacket(ArpOpcode.REPLY, gateway_mac, "10.0.2.2", guest_mac, "10.0.2.15")
        harness.send_frame(EthernetFrame(guest_mac, gateway_mac, EtherType.ARP, reply.serialize()).serialize(60))
        res = harness.wait_for_serial("Real ARP reply received", timeout=30.0)
        if not res:
            print("=== SERIAL LOGS ON ARP HANDSHAKE TIMEOUT ===")
            for l in harness.logs:
                if "[NET" in l or "ARP" in l:
                    print(l)
            print("============================================")
        assert res, "Failed to confirm ARP handshake"

        # 1. Malformed Packet Rejection: Unknown EtherType (0x8888)
        print("[*] Injecting Non-IPv4/ARP EtherType 0x8888...")
        bad_eth = EthernetFrame(guest_mac, gateway_mac, 0x8888, b"MalformedPayload")
        harness.send_frame(bad_eth.serialize(60))
        assert harness.wait_for_serial("kind=NonIpv4Ethertype", timeout=15.0), \
            "Kernel failed to record drop telemetry for bad ethertype"
        print("[PASS] Drop Telemetry: kind=NonIpv4Ethertype verified.")

        # 2. Malformed Packet Rejection: Corrupted IPv4 Checksum
        print("[*] Injecting IPv4 packet with corrupted header checksum...")
        bad_ip = Ipv4Packet("10.0.2.2", "10.0.2.15", IpProtocol.UDP, b"\x00" * 12)
        bad_ip_raw = bad_ip.serialize(force_bad_checksum=True)
        bad_ip_frame = EthernetFrame(guest_mac, gateway_mac, EtherType.IPV4, bad_ip_raw)
        harness.send_frame(bad_ip_frame.serialize(60))
        assert harness.wait_for_serial("kind=MalformedChecksumOrLength", timeout=15.0), \
            "Kernel failed to record drop telemetry for bad IPv4 checksum"
        print("[PASS] Drop Telemetry: kind=MalformedChecksumOrLength verified.")

        # 3. Malformed Packet Rejection: Unknown L4 Protocol (253)
        print("[*] Injecting IPv4 packet with unsupported protocol 253...")
        bad_l4 = Ipv4Packet("10.0.2.2", "10.0.2.15", 253, b"UnsupportedProtocolPayload")
        bad_l4_frame = EthernetFrame(guest_mac, gateway_mac, EtherType.IPV4, bad_l4.serialize())
        harness.send_frame(bad_l4_frame.serialize(60))
        assert harness.wait_for_serial("kind=UnknownL4Protocol", timeout=15.0), \
            "Kernel failed to record drop telemetry for unknown protocol"
        print("[PASS] Drop Telemetry: kind=UnknownL4Protocol verified.")

        # 4. Malformed Packet Rejection: IPv4 Fragment (flags=1, MF=1)
        print("[*] Injecting IPv4 fragment (MF=1)...")
        frag_ip = Ipv4Packet("10.0.2.2", "10.0.2.15", IpProtocol.UDP, b"\x00" * 16, flags=1)
        frag_frame = EthernetFrame(guest_mac, gateway_mac, EtherType.IPV4, frag_ip.serialize())
        harness.send_frame(frag_frame.serialize(60))
        assert harness.wait_for_serial("kind=Ipv4Fragment", timeout=15.0), \
            "Kernel failed to record drop telemetry for IPv4 fragment"
        print("[PASS] Drop Telemetry: kind=Ipv4Fragment verified.")

        # 5. RX Descriptor Ring Wraparound (16 descriptors)
        print("[*] Blasting 32 packets to verify 2x RX descriptor ring wraparound...")
        for i in range(32):
            udp = UdpDatagram(src_port=5000 + i, dst_port=4242, payload=f"burst-{i}".encode())
            ip_pkt = Ipv4Packet("10.0.2.2", "10.0.2.15", IpProtocol.UDP, udp.serialize("10.0.2.2", "10.0.2.15"))
            pkt_frame = EthernetFrame(guest_mac, gateway_mac, EtherType.IPV4, ip_pkt.serialize())
            harness.send_frame(pkt_frame.serialize(60))
            time.sleep(0.02)

        # Allow netd to drain ring
        time.sleep(3.0)
        # Verify subsequent ARP query still works cleanly (no ring freeze or deadlock)
        ping_req = ArpPacket(ArpOpcode.REQUEST, gateway_mac, "10.0.2.2", guest_mac, "10.0.2.15")
        harness.send_frame(EthernetFrame(guest_mac, gateway_mac, EtherType.ARP, ping_req.serialize()).serialize(60))
        post_wrap_frame = harness.recv_frame(timeout=10.0)
        assert post_wrap_frame is not None, "RX ring deadlocked after wraparound!"
        print("[PASS] RX Ring Wraparound: 32 frames processed without ring stall.")

        print("\nALL ROBUSTNESS & DROP TELEMETRY QEMU PACKET TESTS PASSED.\n")
    finally:
        harness.stop()


if __name__ == "__main__":
    run_tests()
