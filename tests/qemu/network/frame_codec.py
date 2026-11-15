# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

"""Pure-Python L2/L3/L4 packet codec and libpcap writer for InterOS QEMU network testing.

Provides framing, serialization, parsing, and RFC-compliant checksum calculation for:
- Ethernet II (IEEE 802.3)
- ARP (RFC 826)
- IPv4 (RFC 791)
- UDP (RFC 768)
- TCP (RFC 793, RFC 9293)
- DNS (RFC 1035)
- IPv6 (RFC 8200)
- ICMPv6 (RFC 4443) and NDP (RFC 4861, RFC 4862)
- Libpcap 2.4 capture files
"""

import struct
import socket
import time
from typing import Tuple, Dict, Any, List, Optional


def internet_checksum(data: bytes) -> int:
    """RFC 1071 16-bit one's complement Internet checksum."""
    sum_val = 0
    i = 0
    while i + 1 < len(data):
        sum_val += (data[i] << 8) | data[i + 1]
        i += 2
    if i < len(data):
        sum_val += data[i] << 8
    while (sum_val >> 16) != 0:
        sum_val = (sum_val & 0xFFFF) + (sum_val >> 16)
    return (~sum_val) & 0xFFFF


class MacAddress:
    BROADCAST = bytes([0xFF] * 6)
    ZERO = bytes([0x00] * 6)

    @staticmethod
    def parse(s: str) -> bytes:
        parts = s.replace("-", ":").split(":")
        if len(parts) != 6:
            raise ValueError(f"Invalid MAC format: {s}")
        return bytes(int(p, 16) for p in parts)

    @staticmethod
    def fmt(b: bytes) -> str:
        return ":".join(f"{x:02x}" for x in b[:6])


class EtherType:
    IPV4 = 0x0800
    ARP = 0x0806
    IPV6 = 0x86DD


class EthernetFrame:
    def __init__(self, dst_mac: bytes, src_mac: bytes, ethertype: int, payload: bytes):
        self.dst_mac = dst_mac[:6]
        self.src_mac = src_mac[:6]
        self.ethertype = ethertype
        self.payload = payload

    def serialize(self, min_len: int = 60) -> bytes:
        raw = self.dst_mac + self.src_mac + struct.pack("!H", self.ethertype) + self.payload
        if len(raw) < min_len:
            raw += b"\x00" * (min_len - len(raw))
        return raw

    @classmethod
    def parse(cls, data: bytes):
        if len(data) < 14:
            raise ValueError("Ethernet frame too short (< 14 bytes)")
        dst_mac = data[0:6]
        src_mac = data[6:12]
        ethertype = struct.unpack("!H", data[12:14])[0]
        payload = data[14:]
        return cls(dst_mac, src_mac, ethertype, payload)


class ArpOpcode:
    REQUEST = 1
    REPLY = 2


class ArpPacket:
    def __init__(self, opcode: int, sender_mac: bytes, sender_ip: str, target_mac: bytes, target_ip: str):
        self.hw_type = 1
        self.proto_type = 0x0800
        self.hw_size = 6
        self.proto_size = 4
        self.opcode = opcode
        self.sender_mac = sender_mac[:6]
        self.sender_ip = socket.inet_aton(sender_ip) if isinstance(sender_ip, str) else sender_ip
        self.target_mac = target_mac[:6]
        self.target_ip = socket.inet_aton(target_ip) if isinstance(target_ip, str) else target_ip

    def serialize(self) -> bytes:
        return (
            struct.pack("!HHBBH", self.hw_type, self.proto_type, self.hw_size, self.proto_size, self.opcode)
            + self.sender_mac + self.sender_ip + self.target_mac + self.target_ip
        )

    @classmethod
    def parse(cls, data: bytes):
        if len(data) < 28:
            raise ValueError("ARP packet too short (< 28 bytes)")
        hw_type, proto_type, hw_size, proto_size, opcode = struct.unpack("!HHBBH", data[:8])
        if hw_type != 1 or proto_type != 0x0800 or hw_size != 6 or proto_size != 4:
            raise ValueError("Unsupported ARP parameters")
        sender_mac = data[8:14]
        sender_ip = socket.inet_ntoa(data[14:18])
        target_mac = data[18:24]
        target_ip = socket.inet_ntoa(data[24:28])
        return cls(opcode, sender_mac, sender_ip, target_mac, target_ip)


class IpProtocol:
    ICMP = 1
    TCP = 6
    UDP = 17
    EXPERIMENTAL = 253


class Ipv4Packet:
    def __init__(self, src: str, dst: str, protocol: int, payload: bytes,
                 ttl: int = 64, identification: int = 1, flags: int = 2, frag_offset: int = 0):
        self.version = 4
        self.ihl = 5
        self.dscp_ecn = 0
        self.identification = identification
        self.flags = flags
        self.frag_offset = frag_offset
        self.ttl = ttl
        self.protocol = protocol
        self.src = socket.inet_aton(src) if isinstance(src, str) else src
        self.dst = socket.inet_aton(dst) if isinstance(dst, str) else dst
        self.payload = payload

    def serialize(self, force_bad_checksum: bool = False) -> bytes:
        total_len = 20 + len(self.payload)
        flags_frag = ((self.flags & 0x7) << 13) | (self.frag_offset & 0x1FFF)
        hdr_no_csum = (
            struct.pack("!BBHHHBB", (self.version << 4) | self.ihl, self.dscp_ecn, total_len,
                        self.identification, flags_frag, self.ttl, self.protocol)
            + b"\x00\x00" + self.src + self.dst
        )
        csum = 0xDEAD if force_bad_checksum else internet_checksum(hdr_no_csum)
        hdr = hdr_no_csum[:10] + struct.pack("!H", csum) + hdr_no_csum[12:]
        return hdr + self.payload

    @classmethod
    def parse(cls, data: bytes):
        if len(data) < 20:
            raise ValueError("IPv4 header too short (< 20 bytes)")
        v_ihl = data[0]
        version = v_ihl >> 4
        ihl = v_ihl & 0xF
        if version != 4 or ihl < 5:
            raise ValueError("Invalid IPv4 version/IHL")
        hdr_len = ihl * 4
        total_len = struct.unpack("!H", data[2:4])[0]
        ident = struct.unpack("!H", data[4:6])[0]
        flags_frag = struct.unpack("!H", data[6:8])[0]
        flags = flags_frag >> 13
        frag_offset = flags_frag & 0x1FFF
        ttl = data[8]
        protocol = data[9]
        if internet_checksum(data[:hdr_len]) != 0:
            raise ValueError("IPv4 header checksum mismatch")
        src = socket.inet_ntoa(data[12:16])
        dst = socket.inet_ntoa(data[16:20])
        payload = data[hdr_len:total_len]
        return cls(src, dst, protocol, payload, ttl=ttl, identification=ident, flags=flags, frag_offset=frag_offset)


class UdpDatagram:
    def __init__(self, src_port: int, dst_port: int, payload: bytes):
        self.src_port = src_port
        self.dst_port = dst_port
        self.payload = payload

    def serialize(self, src_ip: str, dst_ip: str, force_bad_checksum: bool = False) -> bytes:
        length = 8 + len(self.payload)
        hdr_no_csum = struct.pack("!HHHH", self.src_port, self.dst_port, length, 0)
        src_bytes = socket.inet_aton(src_ip)
        dst_bytes = socket.inet_aton(dst_ip)
        pseudo = src_bytes + dst_bytes + struct.pack("!BBH", 0, 17, length) + hdr_no_csum + self.payload
        csum = 0xBADC if force_bad_checksum else internet_checksum(pseudo)
        if csum == 0:
            csum = 0xFFFF
        return struct.pack("!HH", self.src_port, self.dst_port) + struct.pack("!H", length) + struct.pack("!H", csum) + self.payload

    @classmethod
    def parse(cls, data: bytes):
        if len(data) < 8:
            raise ValueError("UDP datagram too short (< 8 bytes)")
        src_port, dst_port, length, _ = struct.unpack("!HHHH", data[:8])
        payload = data[8:length]
        return cls(src_port, dst_port, payload)


class TcpFlags:
    FIN = 0x01
    SYN = 0x02
    RST = 0x04
    PSH = 0x08
    ACK = 0x10
    URG = 0x20


class TcpSegment:
    def __init__(self, src_port: int, dst_port: int, seq: int, ack: int, flags: int,
                 window: int = 65535, payload: bytes = b""):
        self.src_port = src_port
        self.dst_port = dst_port
        self.seq = seq
        self.ack = ack
        self.data_offset = 5
        self.flags = flags
        self.window = window
        self.urgent_ptr = 0
        self.payload = payload

    def serialize(self, src_ip: str, dst_ip: str, force_bad_checksum: bool = False) -> bytes:
        hdr_len = self.data_offset * 4
        hdr_no_csum = struct.pack(
            "!HHIIBBHHH",
            self.src_port, self.dst_port, self.seq, self.ack,
            (self.data_offset << 4), self.flags, self.window,
            0, self.urgent_ptr
        )
        src_bytes = socket.inet_aton(src_ip)
        dst_bytes = socket.inet_aton(dst_ip)
        tcp_len = hdr_len + len(self.payload)
        pseudo = src_bytes + dst_bytes + struct.pack("!BBH", 0, 6, tcp_len) + hdr_no_csum + self.payload
        csum = 0xBADF if force_bad_checksum else internet_checksum(pseudo)
        hdr = hdr_no_csum[:16] + struct.pack("!H", csum) + hdr_no_csum[18:]
        return hdr + self.payload

    @classmethod
    def parse(cls, data: bytes, src_ip: str = None, dst_ip: str = None):
        if len(data) < 20:
            raise ValueError("TCP segment too short (< 20 bytes)")
        src_port, dst_port, seq, ack, offset_reserved, flags, window, csum, urg = struct.unpack("!HHIIBBHHH", data[:20])
        data_offset = offset_reserved >> 4
        hdr_len = data_offset * 4
        payload = data[hdr_len:]
        if src_ip and dst_ip:
            src_bytes = socket.inet_aton(src_ip)
            dst_bytes = socket.inet_aton(dst_ip)
            pseudo = src_bytes + dst_bytes + struct.pack("!BBH", 0, 6, len(data)) + data
            if internet_checksum(pseudo) != 0:
                raise ValueError("TCP checksum mismatch")
        seg = cls(src_port, dst_port, seq, ack, flags, window=window, payload=payload)
        seg.data_offset = data_offset
        seg.urgent_ptr = urg
        return seg


class DnsCodec:
    @staticmethod
    def encode_name(name: str) -> bytes:
        out = b""
        for part in name.strip(".").split("."):
            b_part = part.encode("ascii")
            out += bytes([len(b_part)]) + b_part
        return out + b"\x00"

    @staticmethod
    def decode_name(data: bytes, offset: int = 0) -> Tuple[str, int]:
        parts = []
        i = offset
        jumped = False
        bytes_consumed = 0
        while i < len(data):
            length = data[i]
            if length == 0:
                if not jumped:
                    bytes_consumed = (i - offset) + 1
                break
            if (length & 0xC0) == 0xC0:
                ptr = struct.unpack("!H", data[i:i+2])[0] & 0x3FFF
                if not jumped:
                    bytes_consumed = (i - offset) + 2
                    jumped = True
                i = ptr
                continue
            i += 1
            parts.append(data[i:i+length].decode("latin-1"))
            i += length
        return ".".join(parts), bytes_consumed

    @classmethod
    def parse_query(cls, data: bytes) -> Dict[str, Any]:
        if len(data) < 12:
            raise ValueError("DNS packet too short")
        qid, flags, qdcount, _, _, _ = struct.unpack("!HHHHHH", data[:12])
        offset = 12
        questions = []
        for _ in range(qdcount):
            qname, consumed = cls.decode_name(data, offset)
            offset += consumed
            qtype, qclass = struct.unpack("!HH", data[offset:offset+4])
            offset += 4
            questions.append({"name": qname, "type": qtype, "class": qclass})
        return {"id": qid, "flags": flags, "questions": questions}

    @classmethod
    def build_response(cls, qid: int, qname: str, a_ip: str, ttl: int = 300) -> bytes:
        flags = 0x8180
        hdr = struct.pack("!HHHHHH", qid, flags, 1, 1, 0, 0)
        qname_bytes = cls.encode_name(qname)
        question = qname_bytes + struct.pack("!HH", 1, 1)
        ip_bytes = socket.inet_aton(a_ip)
        answer = b"\xc0\x0c" + struct.pack("!HHIH", 1, 1, ttl, 4) + ip_bytes
        return hdr + question + answer


class Ipv6Packet:
    def __init__(self, src: str, dst: str, next_header: int, payload: bytes, hop_limit: int = 64):
        self.version = 6
        self.traffic_class = 0
        self.flow_label = 0
        self.next_header = next_header
        self.hop_limit = hop_limit
        self.src = socket.inet_pton(socket.AF_INET6, src) if isinstance(src, str) else src
        self.dst = socket.inet_pton(socket.AF_INET6, dst) if isinstance(dst, str) else dst
        self.payload = payload

    def serialize(self) -> bytes:
        vtc_fl = (6 << 28) | ((self.traffic_class & 0xFF) << 20) | (self.flow_label & 0xFFFFF)
        hdr = struct.pack("!IHBB", vtc_fl, len(self.payload), self.next_header, self.hop_limit) + self.src + self.dst
        return hdr + self.payload

    @classmethod
    def parse(cls, data: bytes):
        if len(data) < 40:
            raise ValueError("IPv6 packet too short (< 40 bytes)")
        vtc_fl, payload_len, next_hdr, hop_limit = struct.unpack("!IHBB", data[:8])
        if (vtc_fl >> 28) != 6:
            raise ValueError("Not an IPv6 packet")
        src = socket.inet_ntop(socket.AF_INET6, data[8:24])
        dst = socket.inet_ntop(socket.AF_INET6, data[24:40])
        payload = data[40:40+payload_len]
        return cls(src, dst, next_hdr, payload, hop_limit=hop_limit)


class Icmpv6Codec:
    @staticmethod
    def checksum(src_ip: str, dst_ip: str, payload: bytes) -> int:
        src_bytes = socket.inet_pton(socket.AF_INET6, src_ip)
        dst_bytes = socket.inet_pton(socket.AF_INET6, dst_ip)
        pseudo = src_bytes + dst_bytes + struct.pack("!II", len(payload), 58) + payload
        csum = internet_checksum(pseudo)
        return 0xFFFF if csum == 0 else csum

    @classmethod
    def build_echo_request(cls, src_ip: str, dst_ip: str, ident: int = 1, seq: int = 1, data: bytes = b"InterOS-Echo") -> bytes:
        raw_no_csum = struct.pack("!BBHHH", 128, 0, 0, ident, seq) + data
        csum = cls.checksum(src_ip, dst_ip, raw_no_csum)
        return struct.pack("!BBHHH", 128, 0, csum, ident, seq) + data

    @classmethod
    def build_router_advertisement(cls, router_ip: str, prefix_str: str, prefix_len: int = 64,
                                   valid_life: int = 86400, pref_life: int = 14400) -> bytes:
        ra_base = struct.pack("!BBHBBHII", 134, 0, 0, 64, 0, 1800, 0, 0)
        prefix_bytes = socket.inet_pton(socket.AF_INET6, prefix_str)
        pio = struct.pack("!BBBBIII", 3, 4, prefix_len, 0xC0, valid_life, pref_life, 0) + prefix_bytes
        msg = ra_base + pio
        csum = cls.checksum(router_ip, "ff02::1", msg)
        return struct.pack("!BBH", 134, 0, csum) + msg[4:]

    @classmethod
    def build_neighbor_solicitation(cls, src_ip: str, target_ip: str, slla_mac: bytes = None) -> bytes:
        target_bytes = socket.inet_pton(socket.AF_INET6, target_ip)
        base = struct.pack("!BBHI", 135, 0, 0, 0) + target_bytes
        if slla_mac:
            base += struct.pack("!BB", 1, 1) + slla_mac[:6]
        csum = cls.checksum(src_ip, "ff02::1", base)
        return struct.pack("!BBH", 135, 0, csum) + base[4:]


class PcapWriter:
    """Writes frames in standard libpcap 2.4 format."""
    def __init__(self, filename: str):
        self.filename = filename
        self._f = open(filename, "wb")
        self._f.write(struct.pack("!IHHiIII", 0xA1B2C3D4, 2, 4, 0, 0, 65535, 1))
        self._f.flush()

    def write(self, frame: bytes, ts: Optional[float] = None):
        if ts is None:
            ts = time.time()
        sec = int(ts)
        usec = int((ts - sec) * 1_000_000)
        plen = len(frame)
        self._f.write(struct.pack("!IIII", sec, usec, plen, plen) + frame)
        self._f.flush()

    def close(self):
        if self._f and not self._f.closed:
            self._f.close()
