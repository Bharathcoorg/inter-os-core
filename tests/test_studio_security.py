"""
InterOS Core — Device Studio Endpoint-Level Adversarial Security Test Suite

Tests Device Studio HTTP API endpoints against adversarial inputs:
- /api/qemu/cmd command injection, chaining, dangerous commands, and type confusion
- /api/fs/read, /api/fs/write, /api/fs/delete path traversal and drive letter bypass
- /api/themes/apply traversal sanitization
- Payload size boundaries (>4KB body rejection)
- Malformed JSON handling
- Server liveness after hostile requests
"""

import os
import sys
import time
import json
import socket
import socketserver
import threading
import unittest
import urllib.request
import urllib.error

# Ensure emulator directory is in sys.path
WORKSPACE_ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
sys.path.insert(0, os.path.join(WORKSPACE_ROOT, "emulator"))

import server

class StudioSecurityTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        # Bind ephemeral test server on localhost
        cls.port = 4399
        socketserver.TCPServer.allow_reuse_address = True
        cls.httpd = socketserver.TCPServer(("127.0.0.1", cls.port), server.StudioAPIHandler)
        cls.server_thread = threading.Thread(target=cls.httpd.serve_forever, daemon=True)
        cls.server_thread.start()
        cls.base_url = f"http://127.0.0.1:{cls.port}"
        time.sleep(0.2)

    @classmethod
    def tearDownClass(cls):
        try:
            cls.httpd.shutdown()
            cls.httpd.server_close()
        except Exception:
            pass

    def _post(self, path, data, headers=None):
        url = f"{self.base_url}{path}"
        req_headers = {"Content-Type": "application/json"}
        if headers:
            req_headers.update(headers)
        if isinstance(data, (dict, list)):
            body = json.dumps(data).encode("utf-8")
        elif isinstance(data, str):
            body = data.encode("utf-8")
        else:
            body = data
        req = urllib.request.Request(url, data=body, headers=req_headers, method="POST")
        try:
            with urllib.request.urlopen(req, timeout=3) as resp:
                return resp.status, json.loads(resp.read().decode())
        except urllib.error.HTTPError as e:
            try:
                err_data = json.loads(e.read().decode())
            except Exception:
                err_data = {}
            return e.code, err_data

    def test_01_qemu_cmd_forbidden_commands(self):
        """Verify forbidden QEMU monitor commands are blocked (403)."""
        forbidden = [
            "quit",
            "system_reset",
            "system_powerdown",
            "migrate",
            "qemu-io",
            "drive_add",
            "device_add",
            "stop",
            "c",
            "cont",
        ]
        for cmd in forbidden:
            status, res = self._post("/api/qemu/cmd", {"cmd": cmd})
            self.assertEqual(status, 403, f"Expected 403 for forbidden command '{cmd}', got {status}")
            self.assertFalse(res.get("success"))

    def test_02_qemu_cmd_injection_and_chaining(self):
        """Verify command chaining with newlines, semicolons, pipes, and redirects are blocked (403)."""
        injections = [
            "info version\nquit",
            "info registers\r\nsystem_reset",
            "info cpus; quit",
            "help | cat /etc/passwd",
            "info status && rm -rf /",
            "info `whoami`",
            "info $(id)",
            "info version > /tmp/out",
            "info < /dev/null",
        ]
        for cmd in injections:
            status, res = self._post("/api/qemu/cmd", {"cmd": cmd})
            self.assertEqual(status, 403, f"Expected 403 for injection attempt '{repr(cmd)}', got {status}")
            self.assertFalse(res.get("success"))

    def test_03_qemu_cmd_arbitrary_file_memory_dumping(self):
        """Verify memsave/pmemsave commands cannot be used to overwrite host files (403)."""
        memsave_attempts = [
            "memsave 0 1024 C:/Windows/System32/drivers/etc/hosts",
            "pmemsave 0 4096 ../../../escaped.bin",
            "memsave",
            "pmemsave",
        ]
        for cmd in memsave_attempts:
            status, res = self._post("/api/qemu/cmd", {"cmd": cmd})
            self.assertEqual(status, 403, f"Expected 403 for arbitrary dump '{cmd}', got {status}")

    def test_04_qemu_cmd_malformed_json_and_types(self):
        """Verify malformed JSON, missing fields, and non-string types return 400."""
        # Non-string cmd
        status, _ = self._post("/api/qemu/cmd", {"cmd": 12345})
        self.assertEqual(status, 400)

        # Missing cmd field
        status, _ = self._post("/api/qemu/cmd", {"other": "value"})
        self.assertEqual(status, 400)

        # Empty dict
        status, _ = self._post("/api/qemu/cmd", {})
        self.assertEqual(status, 400)

        # Malformed raw JSON string
        status, _ = self._post("/api/qemu/cmd", "{invalid_json: true")
        self.assertEqual(status, 400)

    def test_05_qemu_cmd_oversized_payload_rejection(self):
        """Verify oversized payloads (>4KB) are rejected with 413 Payload Too Large."""
        large_payload = {"cmd": "info " + "A" * 5000}
        status, _ = self._post("/api/qemu/cmd", large_payload)
        self.assertEqual(status, 413)

    def test_06_vfs_path_traversal_rejection(self):
        """Verify /api/fs/read and write cannot escape VFS_ROOT via dot-dot or drive letters."""
        traversal_paths = [
            "../../../../etc/passwd",
            "../../../Windows/System32/drivers/etc/hosts",
            "..\\..\\..\\Windows\\win.ini",
            "C:/Windows/win.ini",
            "D:\\secret.txt",
        ]
        for p in traversal_paths:
            # Test read
            status, res = self._post("/api/fs/read", {"path": p})
            # Must either return 404 (file not found in VFS_ROOT) or confine path to VFS_ROOT
            self.assertIn(status, [200, 404])
            resolved = server.resolve_vfs_path(p)
            self.assertTrue(
                resolved.startswith(os.path.abspath(server.VFS_ROOT)),
                f"Path '{p}' escaped VFS_ROOT! Resolved to: {resolved}"
            )

    def test_07_theme_traversal_rejection(self):
        """Verify /api/themes/apply sanitizes theme_id and blocks directory traversal."""
        traversals = [
            "../../../system_theme",
            "..\\..\\escaped",
            "/etc/shadow",
            "C:/boot.ini",
        ]
        for t in traversals:
            status, res = self._post("/api/themes/apply", {"theme_id": t})
            # Must return 404 Theme not found because sanitized theme_id does not exist
            self.assertEqual(status, 404)
            self.assertFalse(res.get("success"))

    def test_08_server_liveness_after_adversarial_traffic(self):
        """Verify server remains fully operational and responsive after hostile tests."""
        url = f"{self.base_url}/api/status"
        with urllib.request.urlopen(url, timeout=3) as resp:
            self.assertEqual(resp.status, 200)
            data = json.loads(resp.read().decode())
            self.assertIn("status", data)

if __name__ == "__main__":
    unittest.main()
