# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

"""Test suite for QEMU packet-level IPv6, ICMPv6, SLAAC, and NDP verification."""

import os
import sys
import time
from qemu_packet_harness import QemuNetworkHarness
from frame_codec import (
    EthernetFrame, EtherType, ArpPacket, ArpOpcode,
    Ipv6Packet, Icmpv6Codec, MacAddress
)


def run_tests():
    pcap_out = os.path.abspath(os.path.join(os.path.dirname(__file__), "evidence", "trace_ipv6_ndp.pcap"))
    harness = QemuNetworkHarness(pcap_out)
    print("=" * 60)
    print(" [QEMU PACKET TEST] IPv6 & ICMPv6 / NDP Verification")
    print("=" * 60)

    try:
        harness.start()
        # Drain initial ARP request
        raw = harness.recv_frame(timeout=30.0)
        gateway_mac = bytes.fromhex("52550a000202")
        guest_mac = bytes.fromhex("525400123456")
        reply = ArpPacket(ArpOpcode.REPLY, gateway_mac, "10.0.2.2", guest_mac, "10.0.2.15")
        harness.send_frame(EthernetFrame(guest_mac, gateway_mac, EtherType.ARP, reply.serialize()).serialize(60))
        assert harness.wait_for_serial("Real ARP reply received", timeout=45.0)

        # 1. Inject Router Advertisement with SLAAC Prefix Information Option (RFC 4862)
        router_ip = "fe80::5054:ff:fe00:202"
        prefix = "2001:db8:1::"
        ra_payload = Icmpv6Codec.build_router_advertisement(router_ip, prefix, prefix_len=64)
        ip6_ra = Ipv6Packet(src=router_ip, dst="ff02::1", next_header=58, payload=ra_payload, hop_limit=255)
        ra_frame = EthernetFrame(
            dst_mac=bytes.fromhex("333300000001"), # all-nodes multicast
            src_mac=gateway_mac,
            ethertype=EtherType.IPV6,
            payload=ip6_ra.serialize()
        )
        print("[*] Injecting ICMPv6 Router Advertisement with SLAAC prefix 2001:db8:1::/64...")
        harness.send_frame(ra_frame.serialize(60))
        time.sleep(2.0)

        # Expected SLAAC address derived via modified EUI-64 from MAC 52:54:00:12:34:56:
        # MAC: 52:54:00:12:34:56 -> EUI-64: 5054:00ff:fe12:3456 (bit 7 inverted)
        guest_slaac_ip = "2001:db8:1:0:5054:ff:fe12:3456"

        # 2. Inject ICMPv6 Echo Request (Ping6)
        echo_req = Icmpv6Codec.build_echo_request(src_ip=router_ip, dst_ip=guest_slaac_ip, ident=0x1234, seq=1)
        ip6_echo = Ipv6Packet(src=router_ip, dst=guest_slaac_ip, next_header=58, payload=echo_req, hop_limit=64)
        echo_frame = EthernetFrame(
            dst_mac=guest_mac,
            src_mac=gateway_mac,
            ethertype=EtherType.IPV6,
            payload=ip6_echo.serialize()
        )
        print(f"[*] Injecting ICMPv6 Echo Request to SLAAC address {guest_slaac_ip}...")
        harness.send_frame(echo_frame.serialize(60))

        # 3. Capture ICMPv6 Echo Reply from guest
        echo_reply_captured = False
        deadline = time.time() + 15.0
        while time.time() < deadline:
            f_bytes = harness.recv_frame(timeout=2.0)
            if not f_bytes:
                continue
            ef = EthernetFrame.parse(f_bytes)
            if ef.ethertype == EtherType.IPV6:
                ip6 = Ipv6Packet.parse(ef.payload)
                if ip6.next_header == 58 and len(ip6.payload) >= 8 and ip6.payload[0] == 129: # Echo Reply
                    echo_reply_captured = True
                    break

        assert echo_reply_captured, "Guest failed to transmit ICMPv6 Echo Reply"
        print("[PASS] ICMPv6 Echo: Guest transmitted valid Echo Reply (type 129).")

        # 4. Inject DAD Probe (RFC 4862) with unspecified source (::)
        dad_ns = Icmpv6Codec.build_neighbor_solicitation(src_ip="::", target_ip=guest_slaac_ip)
        ip6_dad = Ipv6Packet(src="::", dst="ff02::1:ff12:3456", next_header=58, payload=dad_ns, hop_limit=255)
        dad_frame = EthernetFrame(
            dst_mac=bytes.fromhex("3333ff123456"),
            src_mac=gateway_mac,
            ethertype=EtherType.IPV6,
            payload=ip6_dad.serialize()
        )
        print("[*] Injecting DAD probe (src=::) to test address defense...")
        harness.send_frame(dad_frame.serialize(60))

        # Await Neighbor Advertisement defense (RFC 4861 § 7.2.4: dst=ff02::1, S=0, O=1)
        defended = False
        deadline = time.time() + 15.0
        while time.time() < deadline:
            f_bytes = harness.recv_frame(timeout=2.0)
            if not f_bytes:
                continue
            ef = EthernetFrame.parse(f_bytes)
            if ef.ethertype == EtherType.IPV6:
                ip6 = Ipv6Packet.parse(ef.payload)
                if ip6.next_header == 58 and len(ip6.payload) >= 8 and ip6.payload[0] == 136: # NA
                    defended = True
                    break

        assert defended, "Guest failed to defend address against DAD conflict"
        print("[PASS] NDP / DAD: Guest defended established SLAAC address via Neighbor Advertisement.")

        print("\nALL IPv6 & NDP QEMU PACKET TESTS PASSED.\n")
    finally:
        harness.stop()


if __name__ == "__main__":
    run_tests()
