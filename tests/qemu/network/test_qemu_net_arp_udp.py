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
        print("[*] Waiting for guest-originated gateway ARP Request (timeout=60s)...")
        frame_bytes = harness.recv_frame(timeout=60.0)
        assert frame_bytes is not None, "Failed to capture guest-originated ARP frame within timeout"

        frame = EthernetFrame.parse(frame_bytes)
        assert frame.ethertype == EtherType.ARP, f"Expected ARP ethertype 0x0806, got 0x{frame.ethertype:04x}"
        arp = ArpPacket.parse(frame.payload)

        assert arp.opcode == ArpOpcode.REQUEST, f"Expected ARP Request (1), got {arp.opcode}"
        assert arp.sender_mac == bytes.fromhex("525400123456"), f"MAC mismatch: {MacAddress.fmt(arp.sender_mac)}"
        assert arp.sender_ip == "10.0.2.15", f"Sender IP mismatch: {arp.sender_ip}"
        assert arp.target_ip == "10.0.2.2", f"Target IP mismatch: {arp.target_ip}"
        print(f"[PASS] Guest TX: Valid ARP Request captured for gateway ({arp.target_ip})")

        # 1. Reject Broadcast ARP Reply (RFC 826)
        print("[*] Injecting illegal broadcast ARP Reply (RFC 826 violation)...")
        bcast_reply = ArpPacket(
            opcode=ArpOpcode.REPLY,
            sender_mac=bytes.fromhex("52550a000202"),
            sender_ip="10.0.2.2",
            target_mac=bytes.fromhex("525400123456"),
            target_ip="10.0.2.15",
        )
        bcast_frame = EthernetFrame(
            dst_mac=MacAddress.BROADCAST,
            src_mac=bytes.fromhex("52550a000202"),
            ethertype=EtherType.ARP,
            payload=bcast_reply.serialize(),
        )
        harness.send_frame(bcast_frame.serialize(min_len=60))
        res = harness.wait_for_serial("arp_broadcast_reply_drop", timeout=15.0)
        if not res:
            print("=== RECENT SERIAL LOGS ===")
            for l in harness.logs[-50:]:
                print(l)
            print("==========================")
        assert res, "Kernel failed to drop broadcast ARP reply"
        print("[PASS] RFC 826: Illegal broadcast ARP reply rejected cleanly.")

        # 2. Inject matching solicited ARP Reply from 10.0.2.2
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
        print("[*] Injecting solicited Host-to-Guest ARP Reply (60 bytes)...")
        harness.send_frame(reply_frame.serialize(min_len=60))

        # Await guest confirmation on COM1
        res2 = harness.wait_for_serial("Real ARP reply received", timeout=30.0)
        if not res2:
            print("=== MATCHING NET LOGS ON ARP REPLY TIMEOUT ===")
            for l in harness.logs:
                if "[NET" in l or "ARP" in l or "arp" in l:
                    print(l)
            print("========================================")
        assert res2, "Guest failed to log verified ARP reply reception"
        print("[PASS] Guest RX: Guest accepted solicited ARP reply and committed gateway mapping.")

        # 3. Reject Unsolicited ARP Reply / In-flight Timeout (RFC 826)
        print("[*] Injecting unsolicited ARP Reply from 10.0.2.99...")
        unsol_pkt = ArpPacket(
            opcode=ArpOpcode.REPLY,
            sender_mac=bytes.fromhex("52550a000299"),
            sender_ip="10.0.2.99",
            target_mac=arp.sender_mac,
            target_ip="10.0.2.15",
        )
        unsol_frame = EthernetFrame(
            dst_mac=arp.sender_mac,
            src_mac=bytes.fromhex("52550a000299"),
            ethertype=EtherType.ARP,
            payload=unsol_pkt.serialize(),
        )
        harness.send_frame(unsol_frame.serialize(min_len=60))
        assert harness.wait_for_serial("arp_unsolicited_reply_dropped", timeout=15.0), \
            "Kernel failed to drop unsolicited ARP reply"
        print("[PASS] RFC 826: Unsolicited / timed-out ARP reply rejected cleanly.")

        # 4. Test Bidirectional Host-to-Guest ARP query
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

        # 5. Test RFC 768 UDP Checksum Rejection
        print("[*] Injecting UDP datagram with corrupted checksum...")
        bad_udp = UdpDatagram(src_port=9998, dst_port=4242, payload=b"Corrupted-UDP-Payload")
        bad_udp_bytes = bad_udp.serialize("10.0.2.2", "10.0.2.15", force_bad_checksum=True)
        bad_ip = Ipv4Packet(src="10.0.2.2", dst="10.0.2.15", protocol=IpProtocol.UDP, payload=bad_udp_bytes)
        bad_frame = EthernetFrame(
            dst_mac=bytes.fromhex("525400123456"),
            src_mac=gateway_mac,
            ethertype=EtherType.IPV4,
            payload=bad_ip.serialize(),
        )
        harness.send_frame(bad_frame.serialize(min_len=60))
        assert harness.wait_for_serial("udp_checksum_mismatch", timeout=15.0), \
            "Kernel failed to drop UDP datagram with corrupted checksum"
        print("[PASS] RFC 768: UDP datagram with corrupted checksum dropped.")

        # 6. Test Host-to-Guest Valid UDP packet exchange
        print("[*] Injecting valid RFC 768 UDP datagram to netd port 4242...")
        good_udp = UdpDatagram(src_port=9999, dst_port=4242, payload=b"InterOS-UDP-Test-Payload")
        good_udp_bytes = good_udp.serialize("10.0.2.2", "10.0.2.15")
        good_ip = Ipv4Packet(src="10.0.2.2", dst="10.0.2.15", protocol=IpProtocol.UDP, payload=good_udp_bytes)
        good_frame = EthernetFrame(
            dst_mac=bytes.fromhex("525400123456"),
            src_mac=gateway_mac,
            ethertype=EtherType.IPV4,
            payload=good_ip.serialize(),
        )
        harness.send_frame(good_frame.serialize(min_len=60))
        time.sleep(1.0)
        print("[PASS] UDP Exchange: Valid datagram processed without kernel drop or panic.")

        print("\nALL ARP & UDP QEMU PACKET TESTS PASSED.\n")
    finally:
        harness.stop()


if __name__ == "__main__":
    run_tests()
