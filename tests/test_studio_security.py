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

    def _request(self, method, path, data=None, headers=None):
        url = f"{self.base_url}{path}"
        req_headers = {}
        if headers:
            req_headers.update(headers)
        body = None
        if data is not None:
            if "Content-Type" not in req_headers:
                req_headers["Content-Type"] = "application/json"
            if isinstance(data, (dict, list)):
                body = json.dumps(data).encode("utf-8")
            elif isinstance(data, str):
                body = data.encode("utf-8")
            else:
                body = data
        req = urllib.request.Request(url, data=body, headers=req_headers, method=method)
        try:
            with urllib.request.urlopen(req, timeout=5) as resp:
                raw = resp.read().decode("utf-8", errors="replace")
                try:
                    res_data = json.loads(raw)
                except Exception:
                    res_data = {"raw": raw}
                return resp.status, res_data, dict(resp.headers)
        except urllib.error.HTTPError as e:
            raw = e.read().decode("utf-8", errors="replace")
            try:
                err_data = json.loads(raw)
            except Exception:
                err_data = {"raw": raw}
            return e.code, err_data, dict(e.headers)

    def _get(self, path, headers=None):
        return self._request("GET", path, headers=headers)

    def _post(self, path, data, headers=None):
        status, data, _ = self._request("POST", path, data=data, headers=headers)
        return status, data

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
            "info unauthorized_subcmd",
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
        """Verify /api/fs/read and write reject traversal paths via dot-dot or drive letters."""
        traversal_paths = [
            "../../../../etc/passwd",
            "../../../Windows/System32/drivers/etc/hosts",
            "..\\..\\..\\Windows\\win.ini",
            "C:/Windows/win.ini",
            "D:\\secret.txt",
        ]
        for p in traversal_paths:
            self.assertFalse(server.is_safe_guest_path(p), f"Path '{p}' should not be considered safe")
            status, res = self._post("/api/fs/read", {"path": p})
            self.assertEqual(status, 400)
            self.assertFalse(res.get("success", True))

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

    def test_09_qemu_vnc_loopback_binding_contract(self):
        """Verify that native QEMU VNC listeners explicitly bind to 127.0.0.1 in all launch paths."""
        studio_ps1 = os.path.join(WORKSPACE_ROOT, "qemu", "launch_studio.ps1")
        with open(studio_ps1, "r", encoding="utf-8") as f:
            studio_src = f.read()
        self.assertIn("vnc=127.0.0.1:1", studio_src, "launch_studio.ps1 must explicitly bind native VNC to 127.0.0.1:1")
        self.assertNotIn('"-display", "vnc=:1"', studio_src, "launch_studio.ps1 must not use wildcard vnc=:1")

        android_ps1 = os.path.join(WORKSPACE_ROOT, "qemu", "launch_android_compat.ps1")
        with open(android_ps1, "r", encoding="utf-8") as f:
            android_src = f.read()
        self.assertIn("vnc=127.0.0.1:$VncDisplay", android_src, "launch_android_compat.ps1 must bind native VNC to 127.0.0.1")
        self.assertNotIn('"-display", "vnc=:$VncDisplay"', android_src, "launch_android_compat.ps1 must not use wildcard vnc=:$VncDisplay")

        server_py = os.path.join(WORKSPACE_ROOT, "emulator", "server.py")
        with open(server_py, "r", encoding="utf-8") as f:
            server_src = f.read()
        self.assertIn('"-vnc", "127.0.0.1:1"', server_src, "emulator/server.py must pass 127.0.0.1:1 for VNC")

    def test_10_dns_rebinding_and_host_origin_validation(self):
        """Verify Host authority and Origin are strictly validated against permitted local loopback."""
        # 1. Matching attacker Host and Origin (DNS rebinding attempt) must be blocked (403)
        status, data, _ = self._get("/api/status", headers={
            "Host": f"attacker.com:{self.port}",
            "Origin": f"http://attacker.com:{self.port}",
        })
        self.assertEqual(status, 403, f"Expected 403 for matching attacker Host and Origin, got {status}")
        self.assertFalse(data.get("success", True))

        # 2. Attacker Host with localhost Origin must be blocked (403)
        status, data, _ = self._get("/api/status", headers={
            "Host": f"attacker.com:{self.port}",
            "Origin": f"http://127.0.0.1:{self.port}",
        })
        self.assertEqual(status, 403, f"Expected 403 for attacker Host, got {status}")

        # 3. Local Host with attacker Origin must be blocked (403)
        status, data, _ = self._get("/api/status", headers={
            "Host": f"127.0.0.1:{self.port}",
            "Origin": "http://evil.com",
        })
        self.assertEqual(status, 403, f"Expected 403 for external Origin, got {status}")

        # 4. Attacker subdomains must be blocked (403)
        status, _, _ = self._get("/api/status", headers={"Host": f"localhost.evil.com:{self.port}"})
        self.assertEqual(status, 403)
        status, _, _ = self._get("/api/status", headers={"Host": f"127.0.0.1.evil.com:{self.port}"})
        self.assertEqual(status, 403)

        # 5. Non-loopback IPs must be blocked (403)
        for ip in ("192.168.1.100", "10.0.0.1", "0.0.0.0", "169.254.169.254"):
            status, _, _ = self._get("/api/status", headers={"Host": f"{ip}:{self.port}"})
            self.assertEqual(status, 403, f"Expected 403 for non-loopback Host IP '{ip}', got {status}")

        # 6. Port mismatch must be blocked (403)
        status, _, _ = self._get("/api/status", headers={"Host": "127.0.0.1:9999"})
        self.assertEqual(status, 403, f"Expected 403 for Host port mismatch, got {status}")

        # 7. Allowed local Host and Origin combinations must succeed (200)
        status, data, _ = self._get("/api/status", headers={
            "Host": f"127.0.0.1:{self.port}",
            "Origin": f"http://127.0.0.1:{self.port}",
        })
        self.assertEqual(status, 200, f"Expected 200 for 127.0.0.1 Host and Origin, got {status}")
        self.assertIn("status", data)

        status, data, _ = self._get("/api/status", headers={
            "Host": f"localhost:{self.port}",
            "Origin": f"http://localhost:{self.port}",
        })
        self.assertEqual(status, 200, f"Expected 200 for localhost Host and Origin, got {status}")
        self.assertIn("status", data)

        status, data, _ = self._get("/api/status", headers={
            "Host": f"[::1]:{self.port}",
            "Origin": f"http://[::1]:{self.port}",
        })
        self.assertEqual(status, 200, f"Expected 200 for [::1] IPv6 Host and Origin, got {status}")
        self.assertIn("status", data)

    def test_11_missing_origin_trust_policy(self):
        """Verify requests with missing Origin headers enforce a deliberate local-only trust policy."""
        # 1. Missing Origin but cross-site fetch indicator must be blocked (403)
        status, data, _ = self._get("/api/status", headers={"Sec-Fetch-Site": "cross-site"})
        self.assertEqual(status, 403, f"Expected 403 for missing Origin with cross-site fetch, got {status}")

        # 2. Missing Origin with external Referer must be blocked (403)
        status, data, _ = self._get("/api/status", headers={"Referer": "http://attacker.com/page.html"})
        self.assertEqual(status, 403, f"Expected 403 for missing Origin with external Referer, got {status}")

        # 3. Missing Origin on POST with navigation intent must be blocked (403)
        status, data, _ = self._request("POST", "/api/audio/volume", data={"action": "UP"}, headers={
            "Sec-Fetch-Mode": "navigate",
            "Sec-Fetch-Dest": "document",
        })
        self.assertEqual(status, 403, f"Expected 403 for navigate-mode POST without local Origin, got {status}")

        # 4. Missing Origin with local Referer must succeed (200)
        status, data, _ = self._get("/api/status", headers={"Referer": f"http://127.0.0.1:{self.port}/"})
        self.assertEqual(status, 200, f"Expected 200 for local Referer, got {status}")

        # 5. Legitimate local CLI request (missing Origin, loopback Host, no external signals) must succeed (200)
        status, data, _ = self._get("/api/status")
        self.assertEqual(status, 200, f"Expected 200 for local CLI request without Origin, got {status}")
        self.assertIn("status", data)

    def test_12_audio_volume_state_changing_methods(self):
        """Verify state-changing audio volume operations require POST and reject GET/invalid methods."""
        # 1. State-changing GET requests with query parameters must return 405 Method Not Allowed
        status, data, headers = self._get("/api/audio/volume?action=UP")
        self.assertEqual(status, 405, f"Expected 405 for GET /api/audio/volume?action=UP, got {status}")
        self.assertIn("Allow", headers, "405 response must include Allow header")
        self.assertIn("POST", headers.get("Allow", ""))

        status, data, headers = self._get("/api/audio/volume?value=50")
        self.assertEqual(status, 405, f"Expected 405 for GET /api/audio/volume?value=50, got {status}")

        # 2. Pure read-only GET /api/audio/volume must NOT return 405
        status, data, _ = self._get("/api/audio/volume")
        self.assertNotEqual(status, 405, "Read-only GET /api/audio/volume must not return 405")
        self.assertIn(status, (200, 503))

        # 3. State-changing POST /api/audio/volume with valid parameters is permitted
        status, data, _ = self._request("POST", "/api/audio/volume", data={"action": "UP"})
        self.assertIn(status, (200, 503), f"Expected 200 or 503 for POST volume action UP, got {status}")

        status, data, _ = self._request("POST", "/api/audio/volume", data={"value": 75})
        self.assertIn(status, (200, 503), f"Expected 200 or 503 for POST volume value 75, got {status}")

        # 4. Invalid POST parameters return 400 Bad Request
        status, data, _ = self._request("POST", "/api/audio/volume", data={"value": -5})
        self.assertEqual(status, 400, f"Expected 400 for negative volume value, got {status}")

        status, data, _ = self._request("POST", "/api/audio/volume", data={"value": 120})
        self.assertEqual(status, 400, f"Expected 400 for out-of-range volume value, got {status}")

        status, data, _ = self._request("POST", "/api/audio/volume", data={"action": "DISASTER"})
        self.assertEqual(status, 400, f"Expected 400 for unknown volume action, got {status}")

        status, data, _ = self._request("POST", "/api/audio/volume", data={})
        self.assertEqual(status, 400, f"Expected 400 for empty volume payload, got {status}")

        # 5. Invalid methods (PUT, DELETE, PATCH) return 405 Method Not Allowed
        for method in ("PUT", "DELETE", "PATCH"):
            status, _, _ = self._request(method, "/api/audio/volume")
            self.assertEqual(status, 405, f"Expected 405 for {method} /api/audio/volume, got {status}")

    def test_13_protected_control_routes_enforcement(self):
        """Verify protected host-control endpoints enforce correct HTTP methods and reject cross-origin access."""
        # 1. POST-only control endpoints called via GET must return 405 Method Not Allowed
        post_only = [
            "/api/qemu/launch",
            "/api/qemu/stop",
            "/api/qemu/reboot",
            "/api/serial/write",
            "/api/qemu/cmd",
            "/api/rapp/install",
            "/api/apps/upload",
        ]
        for route in post_only:
            status, _, headers = self._get(route)
            self.assertEqual(status, 405, f"Expected 405 for GET {route}, got {status}")
            self.assertIn("POST", headers.get("Allow", ""))

        # 2. GET-only endpoints called via POST must return 405 Method Not Allowed
        get_only = [
            "/api/screen/raw",
            "/api/screen/metrics",
            "/api/qemu/telemetry",
            "/api/serial",
        ]
        for route in get_only:
            status, _, headers = self._request("POST", route, data={})
            self.assertEqual(status, 405, f"Expected 405 for POST {route}, got {status}")
            self.assertIn("GET", headers.get("Allow", ""))

        # 3. Cross-origin access to control endpoints must be blocked with 403 Forbidden
        status, _, _ = self._request("POST", "/api/serial/write", data={"command": "HELP"}, headers={"Origin": "http://malicious.org"})
        self.assertEqual(status, 403, f"Expected 403 for cross-origin /api/serial/write, got {status}")

        status, _, _ = self._request("POST", "/api/qemu/launch", data={"profile": "x86_64"}, headers={"Origin": "http://malicious.org"})
        self.assertEqual(status, 403, f"Expected 403 for cross-origin /api/qemu/launch, got {status}")

        status, _, _ = self._request("POST", "/api/qemu/cmd", data={"cmd": "info version"}, headers={"Origin": "http://malicious.org"})
        self.assertEqual(status, 403, f"Expected 403 for cross-origin /api/qemu/cmd, got {status}")

    def test_14_raw_socket_missing_host_header_rejection(self):
        """Verify requests without any Host header are rejected over raw TCP socket."""
        s = socket.create_connection(("127.0.0.1", self.port), timeout=3)
        try:
            # Send HTTP/1.1 request without Host header
            s.sendall(b"GET /api/status HTTP/1.1\r\nConnection: close\r\n\r\n")
            response = b""
            while True:
                chunk = s.recv(4096)
                if not chunk:
                    break
                response += chunk
            self.assertTrue(response.startswith(b"HTTP/1.0 403") or response.startswith(b"HTTP/1.1 403"),
                            f"Expected 403 Forbidden for request missing Host header, got: {response[:50]}")
        finally:
            s.close()

if __name__ == "__main__":
    unittest.main()
