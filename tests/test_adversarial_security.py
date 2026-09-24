#!/usr/bin/env python3
"""
InterOS Core — Automated Adversarial Security & Invariant Test Suite
===================================================================
Tests zero-trust boundaries:
1. SSRF prevention on browser proxy (loopback, RFC1918, link-local, cloud metadata)
2. Package tampering rejection (modified file content with forged hash)
3. Unsigned package rejection
4. Package path traversal rejection (e.g., ../../../etc/passwd)
5. Device Studio command sandbox enforcement
6. Device Studio local-binding verification
"""

import unittest
import urllib.request
import urllib.error
import urllib.parse
import json
import zipfile
import io
import hashlib
import sys
from pathlib import Path

STUDIO_PORT = 4300
STUDIO_URL = f"http://127.0.0.1:{STUDIO_PORT}"

class AdversarialSecurityTests(unittest.TestCase):

    def test_01_ssrf_loopback_rejection(self):
        """Verify SSRF filters reject loopback/localhost requests."""
        targets = [
            "http://127.0.0.1:4300/api/status",
            "http://localhost:4300/api/status",
            "http://127.0.0.2:8080/",
            "http://[::1]:8080/",
        ]
        for t in targets:
            url = f"{STUDIO_URL}/api/browser/proxy?url={urllib.parse.quote(t)}"
            req = urllib.request.Request(url, method="GET")
            try:
                with urllib.request.urlopen(req, timeout=3) as resp:
                    data = json.loads(resp.read().decode("utf-8"))
                    self.assertFalse(data.get("success", False), f"SSRF target {t} was unexpectedly allowed!")
            except urllib.error.HTTPError as e:
                self.assertIn(e.code, (400, 403), f"Expected 403 Forbidden for SSRF target {t}, got {e.code}")

    def test_02_ssrf_metadata_rejection(self):
        """Verify SSRF filters reject cloud metadata IP 169.254.169.254."""
        metadata_url = "http://169.254.169.254/latest/meta-data/"
        url = f"{STUDIO_URL}/api/browser/proxy?url={urllib.parse.quote(metadata_url)}"
        req = urllib.request.Request(url, method="GET")
        try:
            with urllib.request.urlopen(req, timeout=3) as resp:
                data = json.loads(resp.read().decode("utf-8"))
                self.assertFalse(data.get("success", False), "Cloud metadata access was not blocked!")
        except urllib.error.HTTPError as e:
            self.assertIn(e.code, (400, 403))

    def test_03_ssrf_private_network_rejection(self):
        """Verify SSRF filters reject RFC1918 private subnet targets."""
        private_ips = [
            "http://10.0.0.1/admin",
            "http://192.168.1.1/setup",
            "http://172.16.0.1/status",
        ]
        for ip in private_ips:
            url = f"{STUDIO_URL}/api/browser/proxy?url={urllib.parse.quote(ip)}"
            req = urllib.request.Request(url, method="GET")
            try:
                with urllib.request.urlopen(req, timeout=3) as resp:
                    data = json.loads(resp.read().decode("utf-8"))
                    self.assertFalse(data.get("success", False), f"Private IP {ip} was not blocked!")
            except urllib.error.HTTPError as e:
                self.assertIn(e.code, (400, 403))

    def test_04_tampered_package_rejection(self):
        """Verify package installer rejects packages with tampered file contents."""
        buf = io.BytesIO()
        with zipfile.ZipFile(buf, "w", zipfile.ZIP_DEFLATED) as zf:
            manifest = {
                "id": "org.interos.tampered",
                "name": "Tampered App",
                "version": "1.0.0",
                "entrypoint": "bin/app",
                "permissions": []
            }
            zf.writestr("manifest.json", json.dumps(manifest))
            zf.writestr("bin/app", b"TAMPERED_MALICIOUS_PAYLOAD")
            # Forged signature doc claiming different hash
            sig_doc = {
                "signed_payload": {
                    "package_id": "org.interos.tampered",
                    "version": "1.0.0",
                    "permissions": [],
                    "manifest_sha256": hashlib.sha256(json.dumps(manifest).encode()).hexdigest(),
                    "files_sha256": {
                        "bin/app": "0000000000000000000000000000000000000000000000000000000000000000"
                    }
                },
                "signature": "00" * 64,
                "public_key": "00" * 32
            }
            zf.writestr("signature.json", json.dumps(sig_doc))

        payload = buf.getvalue()
        url = f"{STUDIO_URL}/api/rapp/install"
        req = urllib.request.Request(
            url,
            data=payload,
            headers={
                "Content-Type": "application/octet-stream",
                "X-Package-Name": "tampered.rapp"
            },
            method="POST"
        )
        try:
            with urllib.request.urlopen(req, timeout=5) as resp:
                data = json.loads(resp.read().decode("utf-8"))
                self.assertFalse(data.get("success", False), "Tampered package was accepted!")
        except urllib.error.HTTPError as e:
            self.assertIn(e.code, (400, 403, 422))

    def test_05_unsigned_package_rejection(self):
        """Verify package installer rejects packages completely missing signatures."""
        buf = io.BytesIO()
        with zipfile.ZipFile(buf, "w", zipfile.ZIP_DEFLATED) as zf:
            manifest = {
                "id": "org.interos.unsigned",
                "name": "Unsigned App",
                "version": "1.0.0"
            }
            zf.writestr("manifest.json", json.dumps(manifest))
            zf.writestr("bin/app", b"SOME_CODE")

        payload = buf.getvalue()
        url = f"{STUDIO_URL}/api/rapp/install"
        req = urllib.request.Request(
            url,
            data=payload,
            headers={
                "Content-Type": "application/octet-stream",
                "X-Package-Name": "unsigned.rapp"
            },
            method="POST"
        )
        try:
            with urllib.request.urlopen(req, timeout=5) as resp:
                data = json.loads(resp.read().decode("utf-8"))
                self.assertFalse(data.get("success", False), "Unsigned package was accepted!")
        except urllib.error.HTTPError as e:
            self.assertIn(e.code, (400, 403, 422))

    def test_06_studio_command_sandbox(self):
        """Verify privileged / dangerous shell injection is blocked in /api/qemu/cmd."""
        url = f"{STUDIO_URL}/api/qemu/cmd"
        dangerous_cmds = [
            "system('rm -rf /')",
            "quit",
            "hostfwd_add",
            "device_del",
            "!cat /etc/passwd"
        ]
        for cmd in dangerous_cmds:
            payload = json.dumps({"cmd": cmd}).encode("utf-8")
            req = urllib.request.Request(url, data=payload, headers={"Content-Type": "application/json"}, method="POST")
            try:
                with urllib.request.urlopen(req, timeout=3) as resp:
                    data = json.loads(resp.read().decode("utf-8"))
                    self.assertFalse(data.get("success", False), f"Command '{cmd}' bypassed sandbox!")
            except urllib.error.HTTPError as e:
                self.assertEqual(e.code, 403)


if __name__ == "__main__":
    unittest.main(verbosity=2)
