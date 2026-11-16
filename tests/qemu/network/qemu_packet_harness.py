# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

"""Real QEMU socket-level network test harness.

Spawns the x86_64 guest with an Intel 82540EM (E1000) attached to a QEMU socket
netdev backend. Bridges guest TX/RX frames directly with Python over a length-prefixed
stream socket and maintains a real-time libpcap trace file.
"""

import os
import sys
import time
import socket
import struct
import subprocess
import threading
from typing import Optional, List, Tuple

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), "..", "..", ".."))
sys.path.insert(0, ROOT)
sys.path.insert(0, os.path.join(ROOT, "tests"))

from test_interactive_lifecycle import find_qemu, SerialReader
from frame_codec import PcapWriter, EthernetFrame


class QemuNetworkHarness:
    def __init__(self, pcap_filename: Optional[str] = None, net_mode: str = "connect"):
        self.root = ROOT
        self.kernel_img = os.path.join(ROOT, "target", "inter-os-bios.img")
        self.data_disk = os.path.join(ROOT, "target", "inter-os-data-master.img")
        self.qemu_exe = find_qemu("qemu-system-x86_64.exe")
        self.net_mode = net_mode

        self.net_srv: Optional[socket.socket] = None
        if self.net_mode == "connect":
            self.net_srv = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
            self.net_srv.bind(("127.0.0.1", 0))
            self.net_port = self.net_srv.getsockname()[1]
            self.net_srv.listen(1)
        else:
            self.net_port = self._find_free_port()

        self.ser_port = self._find_free_port()
        self.mon_port = self._find_free_port()

        self.proc: Optional[subprocess.Popen] = None
        self.net_sock: Optional[socket.socket] = None
        self.ser_sock: Optional[socket.socket] = None
        self.mon_sock: Optional[socket.socket] = None

        self.reader: Optional[SerialReader] = None
        self.logs: List[str] = []

        self.pcap_path = pcap_filename or os.path.join(ROOT, "target", "qemu_packet_trace.pcap")
        os.makedirs(os.path.dirname(self.pcap_path), exist_ok=True)
        self.pcap = PcapWriter(self.pcap_path)

        self._running = False
        self._pump_thread: Optional[threading.Thread] = None

    def _find_free_port(self) -> int:
        with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as s:
            s.bind(("127.0.0.1", 0))
            return s.getsockname()[1]

    def start(self, boot_timeout: float = 60.0):
        if not os.path.exists(self.kernel_img):
            raise FileNotFoundError(f"Missing kernel boot image: {self.kernel_img}")

        if not os.path.exists(self.data_disk):
            with open(self.data_disk, "wb") as f:
                f.truncate(512 * 1024 * 1024)

        netdev_arg = (
            f"socket,id=net0,connect=127.0.0.1:{self.net_port}"
            if self.net_mode == "connect"
            else f"socket,id=net0,listen=127.0.0.1:{self.net_port}"
        )

        cmd = [
            self.qemu_exe,
            "-machine", "q35",
            "-cpu", "qemu64,+rdrand",
            "-device", "piix3-ide,id=ide",
            "-drive", f"file={self.kernel_img},format=raw,if=none,id=bootd,snapshot=on",
            "-device", "ide-hd,bus=ide.0,unit=0,drive=bootd,bootindex=1",
            "-drive", f"file={self.data_disk},format=raw,if=none,id=datad,snapshot=on",
            "-device", "ide-hd,bus=ide.0,unit=1,drive=datad",
            "-netdev", netdev_arg,
            "-device", "e1000,netdev=net0,mac=52:54:00:12:34:56",
            "-m", "1024M",
            "-display", "none",
            "-chardev", f"socket,id=ser0,host=127.0.0.1,port={self.ser_port},server=on,wait=off",
            "-serial", "chardev:ser0",
            "-chardev", f"socket,id=mon0,host=127.0.0.1,port={self.mon_port},server=on,wait=off",
            "-mon", "chardev=mon0,mode=readline",
            "-no-reboot",
        ]

        self.proc = subprocess.Popen(cmd, cwd=self.root)
        self._running = True

        # Connect or accept netdev socket
        if self.net_mode == "connect":
            self.net_srv.settimeout(15.0)
            self.net_sock, _ = self.net_srv.accept()
        else:
            deadline = time.time() + 15.0
            while time.time() < deadline:
                try:
                    self.net_sock = socket.create_connection(("127.0.0.1", self.net_port), timeout=1.0)
                    break
                except OSError:
                    time.sleep(0.2)
            if not self.net_sock:
                self.stop()
                raise RuntimeError("Failed to connect to QEMU netdev stream socket")

        # Connect serial
        deadline = time.time() + 15.0
        while time.time() < deadline:
            try:
                self.ser_sock = socket.create_connection(("127.0.0.1", self.ser_port), timeout=1.0)
                break
            except OSError:
                time.sleep(0.2)
        if not self.ser_sock:
            self.stop()
            raise RuntimeError("Failed to connect to guest COM1 serial socket")

        self.reader = SerialReader(self.ser_sock)
        self.reader.captured_logs = self.logs

        # Pump serial logs
        def pump():
            buf = b""
            while self._running:
                try:
                    data = self.ser_sock.recv(4096)
                    if not data:
                        time.sleep(0.05)
                        continue
                    buf += data
                    while b"\n" in buf:
                        line, buf = buf.split(b"\n", 1)
                        s = line.decode("utf-8", errors="replace").strip()
                        if s:
                            self.logs.append(s)
                except (socket.timeout, OSError):
                    pass
        self._pump_thread = threading.Thread(target=pump, daemon=True)
        self._pump_thread.start()

    def recv_frame(self, timeout: float = 5.0) -> Optional[bytes]:
        """Receives one Ethernet frame from the guest virtual NIC."""
        if not self.net_sock:
            return None
        self.net_sock.settimeout(timeout)
        try:
            hdr = b""
            while len(hdr) < 4:
                chunk = self.net_sock.recv(4 - len(hdr))
                if not chunk:
                    return None
                hdr += chunk
            plen = struct.unpack("!I", hdr)[0]
            frame = b""
            while len(frame) < plen:
                chunk = self.net_sock.recv(plen - len(frame))
                if not chunk:
                    return None
                frame += chunk
            self.pcap.write(frame)
            return frame
        except (socket.timeout, OSError):
            return None

    def send_frame(self, frame: bytes):
        """Injects one Ethernet frame into the guest virtual NIC RX descriptor ring."""
        if not self.net_sock:
            raise RuntimeError("Netdev socket not connected")
        self.pcap.write(frame)
        payload = struct.pack("!I", len(frame)) + frame
        self.net_sock.sendall(payload)

    def wait_for_serial(self, needle: str, timeout: float = 30.0, since: int = 0) -> bool:
        deadline = time.time() + timeout
        while time.time() < deadline:
            for l in self.logs[since:]:
                if needle in l:
                    return True
            time.sleep(0.2)
        return False

    def send_console_cmd(self, line: str):
        if self.ser_sock:
            cmd = line if line.endswith("\n") else line + "\n"
            try:
                self.ser_sock.sendall(cmd.encode("utf-8"))
            except OSError:
                pass

    def stop(self):
        self._running = False
        if self.pcap:
            self.pcap.close()
        for s in (self.net_sock, self.net_srv, self.ser_sock, self.mon_sock):
            if s:
                try:
                    s.close()
                except OSError:
                    pass
        if self.proc:
            try:
                self.proc.terminate()
                self.proc.wait(timeout=5)
            except Exception:
                try:
                    self.proc.kill()
                except Exception:
                    pass
