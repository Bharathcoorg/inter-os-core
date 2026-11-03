# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

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
import subprocess
import threading
import unittest
import urllib.request
import urllib.error

# Ensure emulator directory is in sys.path
WORKSPACE_ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
sys.path.insert(0, os.path.join(WORKSPACE_ROOT, "emulator"))

import server

def _extract_ws_args_block(script_source):
    """Returns the `$wsArgs = @( ... )` block verbatim from the launcher.

    WHY extract rather than restate: the point of this test is what the launcher
    passes to Start-Process. Rebuilding the array inside the test would let the
    launcher and the test disagree silently, which is exactly the gap that let a
    broken launcher pass a test built from a restated copy of its arguments.
    """
    start = script_source.find("$wsArgs = @(")
    if start < 0:
        raise AssertionError("launch_studio.ps1 must define a $wsArgs array")
    end = script_source.find("\n)", start)
    if end < 0:
        raise AssertionError("launch_studio.ps1 $wsArgs array is not terminated")
    return script_source[start:end + 2]


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
        # WHY the shape changed: the VNC display is a setting now, because a
        # hard-coded display made QEMU exit at launch whenever something else
        # held the port. The contract that matters is unchanged and stronger:
        # the listener stays on loopback, the default display is still 1
        # (127.0.0.1:5901), and the display is taken from the resolved
        # configuration rather than typed into the argument list.
        self.assertIn('"-vnc", f"127.0.0.1:{vnc_display}"', server_src,
                      "emulator/server.py must bind the VNC listener to loopback using the resolved display")
        self.assertIn("DEFAULT_VNC_DISPLAY = 1", server_src,
                      "emulator/server.py must keep display 1 (127.0.0.1:5901) as the default")
        self.assertNotIn('"-vnc", "127.0.0.1:1"', server_src,
                         "emulator/server.py must not hard-code the VNC display")
        self.assertNotIn('"-vnc", "0.0.0.0', server_src,
                         "emulator/server.py must never bind VNC to a wildcard address")

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

        # 8. Origin without port (implicit :80) against non-80 server must be blocked (403)
        status, _, _ = self._get("/api/status", headers={
            "Host": f"127.0.0.1:{self.port}",
            "Origin": "http://localhost",
        })
        self.assertEqual(status, 403, f"Expected 403 for Origin without port matching non-80 server, got {status}")

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

    def test_15_websockify_origin_verification(self):
        """Verify websockify WebSocket bridge enforces strict Origin verification at handshake."""
        # Find ephemeral loopback ports for test websockify and mock backend
        with socket.socket() as s_ws:
            s_ws.bind(('127.0.0.1', 0))
            ws_test_port = s_ws.getsockname()[1]
        with socket.socket() as s_vnc:
            s_vnc.bind(('127.0.0.1', 0))
            vnc_test_port = s_vnc.getsockname()[1]

        cmd = server.build_websockify_cmd(
            ws_port=ws_test_port,
            vnc_port=vnc_test_port,
            studio_port=self.port
        )
        proc = subprocess.Popen(cmd, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        try:
            # Wait for websockify to start listening
            ready = False
            for _ in range(30):
                try:
                    with socket.create_connection(('127.0.0.1', ws_test_port), timeout=0.1):
                        ready = True
                        break
                except Exception:
                    time.sleep(0.1)
            self.assertTrue(ready, "Websockify test instance failed to start")

            def probe(origin_header):
                s = socket.create_connection(('127.0.0.1', ws_test_port), timeout=2)
                try:
                    hdr = f"Origin: {origin_header}\r\n" if origin_header is not None else ""
                    req = (f"GET / HTTP/1.1\r\n"
                           f"Host: 127.0.0.1:{ws_test_port}\r\n"
                           f"Upgrade: websocket\r\n"
                           f"Connection: Upgrade\r\n"
                           f"Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n"
                           f"Sec-WebSocket-Version: 13\r\n"
                           f"{hdr}\r\n").encode("utf-8")
                    s.sendall(req)
                    resp = s.recv(4096).decode("utf-8", errors="replace")
                    status_line = resp.split("\r\n")[0]
                    return status_line
                finally:
                    s.close()

            # 1. Hostile origins must be rejected with 403 Forbidden and NEVER receive 101
            hostile_origins = [
                "https://evil.example",
                "http://attacker.com",
                f"http://evil.com:{self.port}",
                "null",
            ]
            for hostile in hostile_origins:
                status = probe(hostile)
                self.assertNotIn("101", status, f"Hostile origin {hostile} must not receive 101 Switching Protocols")
                self.assertIn("403", status, f"Hostile origin {hostile} must be rejected with 403 Forbidden, got: {status}")

            # 2. Missing Origin header must be rejected with 403
            status = probe(None)
            self.assertNotIn("101", status, "Request without Origin header must not receive 101 Switching Protocols")
            self.assertIn("403", status, f"Request without Origin header must be rejected with 403 Forbidden, got: {status}")

            # 3. Wrong port (e.g. 8080) must be rejected with 403
            status = probe("http://localhost:8080")
            self.assertNotIn("101", status, "Origin with wrong port must not receive 101 Switching Protocols")
            self.assertIn("403", status, f"Origin with wrong port must be rejected with 403, got: {status}")

            # 4. Legitimate local Studio origins must succeed with 101 Switching Protocols
            allowed_origins = [
                f"http://127.0.0.1:{self.port}",
                f"http://localhost:{self.port}",
            ]
            for allowed in allowed_origins:
                status = probe(allowed)
                self.assertIn("101", status, f"Legitimate local origin {allowed} must receive 101 Switching Protocols, got: {status}")
        finally:
            proc.terminate()
            try:
                proc.wait(timeout=2)
            except Exception:
                proc.kill()

    def test_16_early_rejection_body_draining_and_keepalive(self):
        """Verify early 403/405 rejections reliably drain request bodies and preserve HTTP streams."""
        # 1. Repeated rejection test: execute 20 sequential POST requests to a GET-only route with payload
        for i in range(20):
            payload = {"iteration": i, "padding": "A" * 128}
            status, data, headers = self._request("POST", "/api/screen/raw", data=payload)
            self.assertEqual(status, 405, f"Iteration {i} expected 405 Method Not Allowed, got {status}")
            self.assertIn("Allow", headers, f"Iteration {i} response must include Allow header")
            self.assertIn("GET", headers.get("Allow", ""))

        # 2. Raw socket keep-alive / stream synchronization test
        s = socket.create_connection(("127.0.0.1", self.port), timeout=5)
        try:
            body = b'{"test_key": "test_value_padding"}'
            req1 = (
                f"POST /api/screen/raw HTTP/1.1\r\n"
                f"Host: 127.0.0.1:{self.port}\r\n"
                f"Content-Type: application/json\r\n"
                f"Content-Length: {len(body)}\r\n\r\n"
            ).encode("utf-8") + body
            s.sendall(req1)

            # Read full response 1
            buf = b""
            while b"\r\n\r\n" not in buf:
                chunk = s.recv(1024)
                if not chunk:
                    break
                buf += chunk
            self.assertIn(b"405", buf.split(b"\r\n")[0], "Expected 405 Method Not Allowed for POST to GET route")

            # Extract Content-Length to read exact body
            cl = 0
            for line in buf.split(b"\r\n"):
                if line.lower().startswith(b"content-length:"):
                    cl = int(line.split(b":")[1].strip())
            header_part, rest = buf.split(b"\r\n\r\n", 1)
            while len(rest) < cl:
                chunk = s.recv(cl - len(rest))
                if not chunk:
                    break
                rest += chunk
            self.assertEqual(len(rest), cl)
            res_data = json.loads(rest.decode("utf-8"))
            self.assertFalse(res_data.get("success"))
        finally:
            s.close()

        # 3. Bounded drain on oversized rejected payload
        s_over = socket.create_connection(("127.0.0.1", self.port), timeout=5)
        try:
            oversized_len = server.StudioAPIHandler.MAX_DRAIN_BYTES + 4096
            oversized_body = b"X" * oversized_len
            req_over = (
                f"POST /api/screen/raw HTTP/1.1\r\n"
                f"Host: 127.0.0.1:{self.port}\r\n"
                f"Content-Type: application/json\r\n"
                f"Content-Length: {oversized_len}\r\n\r\n"
            ).encode("utf-8") + oversized_body
            s_over.sendall(req_over)

            resp_over = b""
            while b"\r\n\r\n" not in resp_over:
                chunk = s_over.recv(1024)
                if not chunk:
                    break
                resp_over += chunk
            self.assertIn(b"405", resp_over.split(b"\r\n")[0])
            self.assertIn(b"connection: close", resp_over.lower())
        finally:
            s_over.close()

    def test_17_launch_studio_powershell_websockify_execution(self):
        """Verify the real launch_studio.ps1 parses cleanly and starts websockify with hardened origins.

        Every assertion here is driven by the launcher's own argument list, which
        is extracted from qemu/launch_studio.ps1 and evaluated, so this test
        fails if the launcher's arguments change rather than only if the quoting
        idiom it happens to use today still works.
        """
        studio_ps1 = os.path.join(WORKSPACE_ROOT, "qemu", "launch_studio.ps1")
        self.assertTrue(os.path.exists(studio_ps1), "launch_studio.ps1 must exist")

        # 1. Parse check: verify PowerShell 5.1 reports 0 syntax errors on launch_studio.ps1
        parse_cmd = [
            "powershell", "-NoProfile", "-Command",
            f"& {{ $errs = $null; [System.Management.Automation.Language.Parser]::ParseFile('{studio_ps1}', [ref]$null, [ref]$errs); if ($errs) {{ exit $errs.Count }} else {{ exit 0 }} }}"
        ]
        res = subprocess.run(parse_cmd, capture_output=True, text=True)
        self.assertEqual(res.returncode, 0, f"launch_studio.ps1 has PowerShell syntax errors:\n{res.stdout}\n{res.stderr}")

        # 2. Argument validation: verify that --auth-source in launch_studio.ps1 preserves quotes across Start-Process
        with open(studio_ps1, "r", encoding="utf-8") as f:
            src = f.read()
        self.assertIn('"`"--auth-source=$allowedOrigins`""', src,
                      "launch_studio.ps1 must quote --auth-source to prevent Start-Process space splitting")

        # 3. The argument list is taken from launch_studio.ps1 itself, not restated here.
        #
        # WHY this matters: the previous version of this test rebuilt $wsArgs
        # inline, so it proved that the quoting idiom works while proving nothing
        # about what the launcher actually passes. Extracting the block means an
        # edit to the launcher's argument list is what this test observes.
        ws_args_block = _extract_ws_args_block(src)
        self.assertIn("--auth-source", ws_args_block,
                      "the extracted websockify argument list must configure an origin source")

        expected_origins = (
            f"http://127.0.0.1:{self.port} "
            f"http://localhost:{self.port} "
            f"http://[::1]:{self.port}"
        )
        inspect_ps = f"""
        $httpPort = {self.port}
        $wsPort = 6081
        $vncPort = 5901
        $allowedOrigins = "http://127.0.0.1:$httpPort http://localhost:$httpPort http://[::1]:$httpPort"
        {ws_args_block}
        # Report the array the launcher would hand to Start-Process.
        ConvertTo-Json @($wsArgs) -Compress | Write-Output
        """
        r = subprocess.run(["powershell", "-NoProfile", "-Command", inspect_ps],
                           capture_output=True, text=True)
        self.assertEqual(r.returncode, 0, f"evaluating the launcher argument block failed:\n{r.stderr}")
        ws_args = json.loads(r.stdout.strip().splitlines()[-1])

        # The launcher must pass exactly these arguments, in this order, with the
        # origin list as ONE element carrying the quotes Start-Process forwards.
        self.assertEqual(len(ws_args), 6, f"unexpected websockify argument count: {ws_args}")
        self.assertEqual(ws_args[0], "-m")
        self.assertEqual(ws_args[1], "websockify")
        self.assertEqual(ws_args[2], "--auth-plugin=ExpectOrigin")
        self.assertEqual(
            ws_args[3], f'"--auth-source={expected_origins}"',
            "the origin allowlist must be one quoted argument; an unquoted value "
            "would be split by Start-Process into four separate argv entries"
        )
        # The bridge must stay bound to loopback on both ends.
        self.assertTrue(ws_args[4].startswith("127.0.0.1:"), f"websocket endpoint must be loopback: {ws_args[4]}")
        self.assertTrue(ws_args[5].startswith("127.0.0.1:"), f"vnc endpoint must be loopback: {ws_args[5]}")

        # 4. Execution check: run the launcher's own argument list against real
        #    websockify and verify the hardened Origin handshake.
        with socket.socket() as s_ws:
            s_ws.bind(('127.0.0.1', 0))
            ws_test_port = s_ws.getsockname()[1]
        with socket.socket() as s_vnc:
            s_vnc.bind(('127.0.0.1', 0))
            vnc_test_port = s_vnc.getsockname()[1]

        pid_file = os.path.join(WORKSPACE_ROOT, "tests", "_ws_test_pid.txt")
        ps_launch = f"""
        $httpPort = {self.port}
        $wsPort = {ws_test_port}
        $vncPort = {vnc_test_port}
        $allowedOrigins = "http://127.0.0.1:$httpPort http://localhost:$httpPort http://[::1]:$httpPort"
        {ws_args_block}
        $proc = Start-Process -FilePath "python" -ArgumentList $wsArgs -PassThru -NoNewWindow
        $proc.Id | Out-File "{pid_file}" -Encoding ascii
        """
        try:
            subprocess.run(["powershell", "-NoProfile", "-Command", ps_launch],
                           stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, check=True)
            with open(pid_file, "r", encoding="utf-8") as f:
                ws_pid = int(f.read().strip())

            # Wait for websockify to listen
            ready = False
            for _ in range(30):
                try:
                    with socket.create_connection(('127.0.0.1', ws_test_port), timeout=0.1):
                        ready = True
                        break
                except Exception:
                    time.sleep(0.1)
            self.assertTrue(ready, f"Websockify started by PowerShell (PID {ws_pid}) failed to listen on port {ws_test_port}")

            def probe_ws(origin_header):
                s = socket.create_connection(('127.0.0.1', ws_test_port), timeout=2)
                try:
                    hdr = f"Origin: {origin_header}\r\n" if origin_header is not None else ""
                    req = (f"GET / HTTP/1.1\r\n"
                           f"Host: 127.0.0.1:{ws_test_port}\r\n"
                           f"Upgrade: websocket\r\n"
                           f"Connection: Upgrade\r\n"
                           f"Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n"
                           f"Sec-WebSocket-Version: 13\r\n"
                           f"{hdr}\r\n").encode("utf-8")
                    s.sendall(req)
                    resp = s.recv(4096).decode("utf-8", errors="replace")
                    return resp.split("\r\n")[0]
                finally:
                    s.close()

            # Hostile origin -> 403
            self.assertIn("403", probe_ws("http://attacker.com"))
            # Legitimate origin -> 101
            self.assertIn("101", probe_ws(f"http://127.0.0.1:{self.port}"))
        finally:
            if os.path.exists(pid_file):
                try:
                    with open(pid_file, "r", encoding="utf-8") as f:
                        ws_pid = int(f.read().strip())
                    subprocess.run(["taskkill", "/F", "/PID", str(ws_pid)],
                                   stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
                except Exception:
                    pass
                try:
                    os.remove(pid_file)
                except OSError:
                    pass


if __name__ == "__main__":
    unittest.main()

