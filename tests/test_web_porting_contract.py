#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

"""
InterOS Core — Web-to-RAPP & Polyfill Engine Contract Test (tests/test_web_porting_contract.py)

Validates:
  1. CLI tool tools/port_web.py exists, parses arguments, and provides help.
  2. Local directory ingestion converts HTML/CSS/JS into signed .rapp bundle.
  3. Web permission mapping:
       camera -> CameraCapture (4)
       microphone -> Microphone (7)
       geolocation -> Location (8)
       storage / persistent-storage -> FileSystemRead (9), FileSystemWrite (10)
       notifications -> Notifications (14)
       sensor / accelerometer / gyroscope -> Sensors (12)
       network -> NetworkSocket (12), NetworkSend (13), NetworkReceive (14)
  4. Automatic Strict Content Security Policy (CSP) injection and validation.
  5. Automatic injection of <script src="interos.js"></script> into index.html.
  6. Cryptographic Ed25519 signature and SHA-256 digest verification of generated .rapp.
  7. tools/create_app.py --from-web delegation to tools/port_web.py.
  8. SDK constants ArchWasm = 3 / ArchWeb = 3 and permission helpers in TypeScript and Go.
"""

import os
import sys
import json
import shutil
import hashlib
import tempfile
import unittest
import subprocess
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "tools"))

from cryptography.hazmat.primitives.asymmetric import ed25519
from cryptography.hazmat.primitives import serialization


class TestWebPortingContract(unittest.TestCase):
    def setUp(self):
        self.temp_dir = tempfile.mkdtemp(prefix="interos_web_test_")
        self.work_dir = Path(self.temp_dir)

    def tearDown(self):
        shutil.rmtree(self.temp_dir, ignore_errors=True)

    def test_01_cli_help(self):
        """tools/port_web.py must respond to --help with exit code 0."""
        cmd = [sys.executable, str(ROOT / "tools" / "port_web.py"), "--help"]
        res = subprocess.run(cmd, capture_output=True, text=True)
        self.assertEqual(res.returncode, 0)
        self.assertIn("Port standard web applications", res.stdout)
        self.assertIn("--output", res.stdout)
        self.assertIn("--name", res.stdout)

    def test_02_web_permissions_mapping_and_rapp_generation(self):
        """Porting engine maps web permissions to capability IDs and builds signed RAPP."""
        app_dir = self.work_dir / "my_web_pwa"
        app_dir.mkdir()

        # Create basic web app files with standard webmanifest
        (app_dir / "index.html").write_text(
            "<!DOCTYPE html><html><head><title>Camera Geo App</title></head>"
            "<body><h1>Test App</h1><script src='app.js'></script></body></html>",
            encoding="utf-8"
        )
        (app_dir / "app.js").write_text(
            "// Request camera and geolocation\n"
            "navigator.mediaDevices.getUserMedia({ video: true });\n"
            "navigator.geolocation.getCurrentPosition(()=>{});\n",
            encoding="utf-8"
        )
        (app_dir / "manifest.json").write_text(
            json.dumps({
                "name": "Camera & Geo Test",
                "short_name": "cam_geo",
                "version": "2.1.0",
                "permissions": ["camera", "geolocation", "notifications", "storage"]
            }),
            encoding="utf-8"
        )

        out_rapp = self.work_dir / "cam_geo.rapp"
        cmd = [
            sys.executable,
            str(ROOT / "tools" / "port_web.py"),
            str(app_dir),
            "--output", str(out_rapp),
            "--arch", "web"
        ]
        res = subprocess.run(cmd, capture_output=True, text=True)
        self.assertEqual(res.returncode, 0, f"port_web failed: {res.stderr}\n{res.stdout}")
        self.assertTrue(out_rapp.exists())

        # Validate RAPP v2 structure
        data = out_rapp.read_bytes()
        self.assertGreaterEqual(len(data), 0xB4)
        self.assertEqual(data[:4], b"RAPP")
        self.assertEqual(int.from_bytes(data[4:6], "little"), 2)  # Format version 2
        self.assertEqual(int.from_bytes(data[6:8], "little"), 3)  # Arch 3 = Web

        # Verify capabilities against the kernel's MANIFEST id space
        # (kernel/src/capability/mod.rs, CapabilityType::to_raw):
        # camera       -> 22 (SystemControlCamera, the scoped camera service)
        # storage      -> 9 (FileSystemRead), 10 (FileSystemWrite)
        # geolocation  -> no manifest id; consent 8 (Location) only
        # notifications-> no manifest id; consent 14 (Notifications) only
        #
        # WHY 24 and 17 are asserted ABSENT: this contract used to demand them,
        # naming 24 "LocationAccess" and 17 "Notifications". Neither name exists
        # in the kernel. 24 is RealtimeClockRead (rtc.read) and 17 is EventPoll
        # (event.poll), so demanding them for a geolocation/notifications prompt
        # pinned the very id-space conflation that handed a web app clock and
        # event-polling authority it never asked for. The ladder defines no GNSS
        # object, and NotificationPost authorizes on the caller's own package
        # identity, so the only honest grant for either is a consent request.
        # tests/test_permission_mapping_contract.py pins the consent half across
        # the Go, TypeScript and Python mappers.
        caps_count = int.from_bytes(data[0x4E:0x50], "little")
        caps = [
            int.from_bytes(data[0xB4 + i*2 : 0xB4 + (i+1)*2], "little")
            for i in range(caps_count)
        ]
        self.assertIn(22, caps, "SystemControlCamera (22) missing from mapped capabilities")
        self.assertIn(9, caps, "FileSystemRead (9) missing from mapped capabilities")
        self.assertIn(10, caps, "FileSystemWrite (10) missing from mapped capabilities")
        self.assertNotIn(24, caps, "geolocation must not grant RealtimeClockRead (24)")
        self.assertNotIn(17, caps, "notifications must not grant EventPoll (17)")
        self.assertEqual(
            sorted(caps), [9, 10, 22],
            "a camera+geolocation+notifications+storage PWA must receive exactly the "
            "manifest ids those permissions can express, and no ambient extra",
        )

        # Payload digest and layout must still be canonical.
        pub_key = bytes(data[0x28:0x48])
        sig = bytes(data[0x70:0xB0])
        payload_len = int.from_bytes(data[0xB0:0xB4], "little")
        payload_offset = 0xB4 + caps_count * 2
        self.assertEqual(payload_offset + payload_len, len(data))

        payload = data[payload_offset:]
        expected_digest = hashlib.sha256(payload).digest()
        self.assertEqual(data[0x50:0x70], expected_digest)

        # WHY this asserts the package is UNSIGNED: porting third-party web
        # content used to sign it implicitly with the local developer key, which
        # turned any exfiltrated host file into an installable trusted package.
        # Signing is now an explicit opt-in (--sign), so the default output must
        # carry a zero publisher key and a zero signature and the kernel must
        # refuse it until a publisher deliberately signs it and the owner
        # provisions that publisher's trust root. test_02b covers the signed path.
        self.assertEqual(pub_key, b"\0" * 32, "default port must not embed a publisher key")
        self.assertEqual(sig, b"\0" * 64, "default port must not embed a signature")

    def test_02b_explicit_signing_produces_a_verifiable_package(self):
        """--sign with an explicit key signs the canonical bytes; the default does not."""
        app_dir = self.work_dir / "signed_web_pwa"
        app_dir.mkdir()
        (app_dir / "index.html").write_text(
            "<!DOCTYPE html><html><head><title>Signed</title></head><body>Hi</body></html>",
            encoding="utf-8"
        )
        (app_dir / "manifest.json").write_text(
            json.dumps({"name": "Signed PWA", "short_name": "signed_pwa", "version": "1.0.0",
                        "permissions": ["storage"]}),
            encoding="utf-8"
        )

        # An EPHEMERAL key generated inside this test's own work dir. The repo's
        # developer key is never read, copied or used here.
        from cryptography.hazmat.primitives.asymmetric import ed25519
        from cryptography.hazmat.primitives import serialization
        priv = ed25519.Ed25519PrivateKey.generate()
        key_path = self.work_dir / "ephemeral_ed25519.key"
        key_path.write_bytes(priv.private_bytes(
            serialization.Encoding.Raw,
            serialization.PrivateFormat.Raw,
            serialization.NoEncryption(),
        ))
        expected_pub = priv.public_key().public_bytes(
            serialization.Encoding.Raw, serialization.PublicFormat.Raw
        )

        out_rapp = self.work_dir / "signed_pwa.rapp"
        cmd = [
            sys.executable, str(ROOT / "tools" / "port_web.py"), str(app_dir),
            "--output", str(out_rapp), "--arch", "web",
            "--sign", "--key", str(key_path),
        ]
        res = subprocess.run(cmd, capture_output=True, text=True)
        self.assertEqual(res.returncode, 0, f"port_web --sign failed: {res.stderr}\n{res.stdout}")

        data = out_rapp.read_bytes()
        caps_count = int.from_bytes(data[0x4E:0x50], "little")
        self.assertEqual(bytes(data[0x28:0x48]), expected_pub)
        sig = bytes(data[0x70:0xB0])
        self.assertNotEqual(sig, b"\0" * 64, "--sign must produce a real signature")

        signing_bytes = bytearray(data)
        signing_bytes[0x70:0xB0] = b"\0" * 64
        verifier = ed25519.Ed25519PublicKey.from_public_bytes(expected_pub)
        verifier.verify(sig, bytes(signing_bytes))

        # The signature must cover the capability array too: flipping a declared
        # capability id has to invalidate it, or a manifest could be escalated
        # after signing without breaking the seal.
        self.assertGreater(caps_count, 0)
        tampered = bytearray(signing_bytes)
        tampered[0xB4] ^= 0x01
        with self.assertRaises(Exception):
            verifier.verify(sig, bytes(tampered))

    def test_03_csp_and_interos_js_injection(self):
        """port_web injects strict CSP meta tag and interos.js script into index.html."""
        app_dir = self.work_dir / "unhardened_web_app"
        app_dir.mkdir()
        (app_dir / "index.html").write_text(
            "<!DOCTYPE html><html><head><title>Unhardened</title></head><body>Hello</body></html>",
            encoding="utf-8"
        )

        out_rapp = self.work_dir / "unhardened.rapp"
        cmd = [
            sys.executable,
            str(ROOT / "tools" / "port_web.py"),
            str(app_dir),
            "--output", str(out_rapp)
        ]
        res = subprocess.run(cmd, capture_output=True, text=True)
        self.assertEqual(res.returncode, 0)

        # Inspect the bundled index.html inside the payload ZIP
        import zipfile, io
        data = out_rapp.read_bytes()
        caps_count = int.from_bytes(data[0x4E:0x50], "little")
        payload_offset = 0xB4 + caps_count * 2
        payload = data[payload_offset:]

        with zipfile.ZipFile(io.BytesIO(payload), "r") as zf:
            self.assertIn("index.html", zf.namelist())
            self.assertIn("interos.js", zf.namelist())
            self.assertIn("app.json", zf.namelist())

            bundled_html = zf.read("index.html").decode("utf-8")
            self.assertIn("Content-Security-Policy", bundled_html)
            self.assertIn("default-src 'self'", bundled_html)
            self.assertIn('<script src="interos.js"></script>', bundled_html)

    def test_04_create_app_delegation_from_web(self):
        """tools/create_app.py --from-web delegates to port_web.py."""
        app_dir = self.work_dir / "delegate_test_web"
        app_dir.mkdir()
        (app_dir / "index.html").write_text(
            "<!DOCTYPE html><html><head><title>Delegate</title></head><body>App</body></html>",
            encoding="utf-8"
        )
        out_rapp = self.work_dir / "delegated.rapp"
        cmd = [
            sys.executable,
            str(ROOT / "tools" / "create_app.py"),
            "--from-web", str(app_dir),
            "--output", str(out_rapp),
            "delegated_app"
        ]
        res = subprocess.run(cmd, capture_output=True, text=True)
        self.assertEqual(res.returncode, 0, f"create_app --from-web failed: {res.stderr}\n{res.stdout}")
        self.assertTrue(out_rapp.exists())
        self.assertEqual(out_rapp.read_bytes()[:4], b"RAPP")

    def test_05_sdk_constants_and_helpers(self):
        """TypeScript and Go SDKs export ArchWasm/ArchWeb = 3 and permission mappings."""
        ts_builder = (ROOT / "sdk" / "typescript" / "src" / "builder.ts").read_text(encoding="utf-8")
        self.assertIn("export const ArchWasm = 3", ts_builder)
        self.assertIn("export const ArchWeb = 3", ts_builder)
        self.assertIn("mapWebPermissionToCapabilities", ts_builder)

        go_types = (ROOT / "sdk" / "go" / "rapp" / "types.go").read_text(encoding="utf-8")
        self.assertIn("ArchWasm    Arch = 3", go_types)
        self.assertIn("ArchWeb     Arch = 3", go_types)

        go_builder = (ROOT / "sdk" / "go" / "rapp" / "builder.go").read_text(encoding="utf-8")
        self.assertIn("MapWebPermission", go_builder)


if __name__ == "__main__":
    unittest.main()
