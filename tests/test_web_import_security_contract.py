#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

"""Adversarial contract for the DEVELOPER-HOST web import boundary.

Scope: tools/port_web.py. This is a host-side defence, not a guest-kernel one.
`/api/web/import` in emulator/server.py forwards a caller-supplied `folder` /
`url` / `source` string to this tool, and the tool is what opens the socket and
copies the bytes, so the boundary is enforced here.

The three defects this pins:
  1. SSRF -- the URL branch called urlopen with no address validation, followed
     redirects implicitly, and performed a second-order manifest fetch from
     attacker-controlled HTML. A hostile page could walk the importer onto
     loopback or cloud metadata (169.254.169.254).
  2. Arbitrary host-path read -- the folder branch resolved any path and
     copytree'd it, so `folder: "~/.ssh"` exfiltrated credentials into a package.
  3. Auto-signing -- the result was signed with ~/.interos/developer_ed25519.key
     by default, which is what turns an exfiltrated file into an installable
     package under a trusted publisher.

No test here reads any real private key. The signing test generates an ephemeral
key in a temp directory; the default-path test asserts the key loader is never
called at all.
"""

import http.client
import http.server
import io
import json
import os
import shutil
import socket
import subprocess
import sys
import tempfile
import threading
import unittest
import urllib.error
import urllib.request
from pathlib import Path
from unittest import mock

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "tools"))

import port_web  # noqa: E402

PORT_WEB = ROOT / "tools" / "port_web.py"
TEMPLATE = ROOT / "templates" / "web_app_template"


def _make_webapp(parent, name="webapp", permissions=None, html=None):
    app = parent / name
    app.mkdir(parents=True, exist_ok=True)
    (app / "index.html").write_text(
        html or "<!DOCTYPE html><html><head><title>t</title></head><body>hi</body></html>",
        encoding="utf-8",
    )
    if permissions is not None:
        (app / "manifest.json").write_text(
            json.dumps({"name": name, "version": "1.0.0", "permissions": permissions}),
            encoding="utf-8",
        )
    return app


def _rapp_fields(data):
    caps_count = int.from_bytes(data[0x4E:0x50], "little")
    caps = [int.from_bytes(data[0xB4 + i * 2:0xB4 + (i + 1) * 2], "little") for i in range(caps_count)]
    return {
        "publisher": data[0x28:0x48],
        "signature": data[0x70:0xB0],
        "caps": caps,
        "payload": data[0xB4 + caps_count * 2:],
    }


class _SilentHandler(http.server.BaseHTTPRequestHandler):
    def log_message(self, *args):
        pass


class TestWebImportSsrfGuard(unittest.TestCase):
    """H16.1: remote ingestion must not reach non-public address space."""

    def setUp(self):
        self.tmp = tempfile.mkdtemp(prefix="interos_import_ssrf_")
        self.staging = Path(self.tmp) / "staging"
        self.staging.mkdir()

    def tearDown(self):
        shutil.rmtree(self.tmp, ignore_errors=True)

    def test_01_file_and_other_non_http_schemes_are_refused(self):
        for url in (
            "file:///etc/passwd",
            "file:///C:/Windows/win.ini",
            "ftp://example.com/app",
            "data:text/html,<h1>hi</h1>",
            "gopher://example.com/",
            "jar:file:///tmp/x.jar!/",
        ):
            with self.assertRaises(port_web.ImportRefusedError, msg=url):
                port_web.resolve_import_url(url)
            with self.assertRaises(port_web.ImportRefusedError, msg=url):
                port_web.ingest_target(url, self.staging)
        self.assertEqual(list(self.staging.iterdir()), [], "a refused scheme staged content")

    def test_02_loopback_and_cloud_metadata_are_refused(self):
        for url in (
            "http://127.0.0.1/",
            "http://127.0.0.1:8080/admin",
            "http://169.254.169.254/latest/meta-data/",
            "http://169.254.169.254/latest/meta-data/iam/security-credentials/",
            "http://localhost/",
            "http://localhost:6283/api/qemu",
            "http://metadata.google.internal/computeMetadata/v1/",
            "http://instance-data/",
            "http://[::1]/",
            "http://[::ffff:127.0.0.1]/",
            "http://[::ffff:169.254.169.254]/",
        ):
            with self.assertRaises(port_web.ImportRefusedError, msg=url):
                port_web.resolve_import_url(url)

    def test_03_private_link_local_ula_multicast_and_reserved_are_refused(self):
        for url in (
            "http://10.0.0.1/",
            "http://192.168.1.1/",
            "http://172.16.0.1/",
            "http://100.64.0.1/",          # CGNAT, not flagged is_private on every version
            "http://0.0.0.0/",
            "http://224.0.0.1/",          # multicast
            "http://240.0.0.1/",          # reserved
            "http://192.0.2.1/",          # TEST-NET-1
            "http://[fc00::1]/",          # unique-local
            "http://[fd12:3456::1]/",
            "http://[fe80::1]/",          # link-local
            "http://[::]/",
            "http://[2001:db8::1]/",
        ):
            with self.assertRaises(port_web.ImportRefusedError, msg=url):
                port_web.resolve_import_url(url)

    def test_04_public_unicast_addresses_are_accepted(self):
        for addr in ("93.184.216.34", "8.8.8.8", "2001:4860:4860::8888"):
            self.assertFalse(port_web._address_is_forbidden(__import__("ipaddress").ip_address(addr)), addr)
        with mock.patch.object(socket, "getaddrinfo", return_value=[
            (socket.AF_INET, socket.SOCK_STREAM, 6, "", ("93.184.216.34", 80)),
        ]):
            host, port, pinned = port_web.resolve_import_url("http://public-origin.test/index.html")
        self.assertEqual((host, port, pinned), ("public-origin.test", 80, "93.184.216.34"))

    def test_05_every_resolved_address_is_checked_not_just_the_first(self):
        """A multi-A-record answer containing one private address is refused."""
        answers = [
            (socket.AF_INET, socket.SOCK_STREAM, 6, "", ("93.184.216.34", 80)),
            (socket.AF_INET, socket.SOCK_STREAM, 6, "", ("127.0.0.1", 80)),
        ]
        with mock.patch.object(socket, "getaddrinfo", return_value=answers):
            with self.assertRaises(port_web.ImportRefusedError):
                port_web.resolve_import_url("http://public-origin.test/")

    def test_06_unresolvable_host_fails_closed(self):
        with mock.patch.object(socket, "getaddrinfo", side_effect=socket.gaierror("no such host")):
            with self.assertRaises(port_web.ImportRefusedError):
                port_web.resolve_import_url("http://does-not-exist.test/")
        with mock.patch.object(socket, "getaddrinfo", return_value=[]):
            with self.assertRaises(port_web.ImportRefusedError):
                port_web.resolve_import_url("http://does-not-exist.test/")

    def test_07_socket_is_pinned_to_the_validated_address_and_keeps_the_hostname(self):
        """DNS rebinding between check and connect cannot move the fetch."""
        cls = port_web._pinned_connection_class(http.client.HTTPConnection, "93.184.216.34")
        conn = cls("public-origin.test", timeout=5)
        self.assertEqual(conn.host, "public-origin.test", "Host header / SNI must still name the origin")
        with mock.patch.object(socket, "create_connection") as cc:
            conn._create_connection(("public-origin.test", 80), 5, None)
        cc.assert_called_once_with(("93.184.216.34", 80), 5, None)

    def test_08_every_redirect_hop_is_revalidated(self):
        handler = port_web._GuardedRedirectHandler()
        req = urllib.request.Request("http://public-origin.test/")
        for target in (
            "http://127.0.0.1:6283/",
            "http://169.254.169.254/latest/meta-data/",
            "http://[::1]/",
            "file:///etc/passwd",
            "http://10.0.0.1/",
        ):
            with self.assertRaises(port_web.ImportRefusedError, msg=target):
                handler.redirect_request(req, None, 302, "Found", {}, target)

    def test_09_redirect_from_a_public_origin_to_loopback_is_refused_end_to_end(self):
        class Redirector(_SilentHandler):
            def do_GET(self):
                self.send_response(302)
                self.send_header("Location", "http://127.0.0.1:%d/secret" % self.server.server_address[1])
                self.end_headers()

        srv = http.server.HTTPServer(("127.0.0.1", 0), Redirector)
        threading.Thread(target=srv.serve_forever, daemon=True).start()
        real_gai, real_cc = socket.getaddrinfo, socket.create_connection

        def gai(host, *a, **kw):
            if host == "public-origin.test":
                return [(socket.AF_INET, socket.SOCK_STREAM, 6, "", ("93.184.216.34", a[0] if a else 80))]
            return real_gai(host, *a, **kw)

        def cc(address, *a, **kw):
            if address[0] == "93.184.216.34":
                return real_cc(("127.0.0.1", srv.server_address[1]), *a, **kw)
            return real_cc(address, *a, **kw)

        try:
            with mock.patch.object(socket, "getaddrinfo", gai), mock.patch.object(socket, "create_connection", cc):
                with self.assertRaises(port_web.ImportRefusedError):
                    port_web.fetch_remote_document("http://public-origin.test/", timeout=10)
        finally:
            srv.shutdown()
            srv.server_close()
        self.assertEqual(list(self.staging.iterdir()), [])

    def test_10_second_order_manifest_fetch_is_guarded(self):
        """The manifest link comes from attacker-controlled HTML."""
        class ManifestLink(_SilentHandler):
            def do_GET(self):
                body = (b'<html><head><link rel="manifest" '
                        b'href="http://169.254.169.254/latest/meta-data/"></head></html>')
                self.send_response(200)
                self.send_header("Content-Length", str(len(body)))
                self.end_headers()
                self.wfile.write(body)

        srv = http.server.HTTPServer(("127.0.0.1", 0), ManifestLink)
        threading.Thread(target=srv.serve_forever, daemon=True).start()
        real_gai, real_cc = socket.getaddrinfo, socket.create_connection

        def gai(host, *a, **kw):
            if host == "public-origin.test":
                return [(socket.AF_INET, socket.SOCK_STREAM, 6, "", ("93.184.216.34", a[0] if a else 80))]
            return real_gai(host, *a, **kw)

        def cc(address, *a, **kw):
            if address[0] == "93.184.216.34":
                return real_cc(("127.0.0.1", srv.server_address[1]), *a, **kw)
            return real_cc(address, *a, **kw)

        try:
            with mock.patch.object(socket, "getaddrinfo", gai), mock.patch.object(socket, "create_connection", cc):
                with self.assertRaises(port_web.ImportRefusedError):
                    port_web.ingest_target("http://public-origin.test/", self.staging)
        finally:
            srv.shutdown()
            srv.server_close()
        self.assertFalse((self.staging / "manifest.json").exists(), "a forbidden manifest was written")

    def test_11_manifest_href_resolves_against_the_final_url_and_is_guarded(self):
        class RelativeManifest(_SilentHandler):
            def do_GET(self):
                if self.path.endswith(".webmanifest"):
                    body = b'{"name":"ok"}'
                    self.send_response(200)
                    self.send_header("Content-Length", str(len(body)))
                    self.end_headers()
                    self.wfile.write(body)
                    return
                body = b'<html><head><link rel="manifest" href="app.webmanifest"></head></html>'
                self.send_response(200)
                self.send_header("Content-Length", str(len(body)))
                self.end_headers()
                self.wfile.write(body)

        srv = http.server.HTTPServer(("127.0.0.1", 0), RelativeManifest)
        threading.Thread(target=srv.serve_forever, daemon=True).start()
        real_gai, real_cc = socket.getaddrinfo, socket.create_connection

        def gai(host, *a, **kw):
            if host == "public-origin.test":
                return [(socket.AF_INET, socket.SOCK_STREAM, 6, "", ("93.184.216.34", a[0] if a else 80))]
            return real_gai(host, *a, **kw)

        def cc(address, *a, **kw):
            if address[0] == "93.184.216.34":
                return real_cc(("127.0.0.1", srv.server_address[1]), *a, **kw)
            return real_cc(address, *a, **kw)

        try:
            with mock.patch.object(socket, "getaddrinfo", gai), mock.patch.object(socket, "create_connection", cc):
                port_web.ingest_target("http://public-origin.test/", self.staging)
        finally:
            srv.shutdown()
            srv.server_close()
        self.assertTrue((self.staging / "manifest.json").exists())
        self.assertEqual(json.loads((self.staging / "manifest.json").read_text())["name"], "ok")

    def test_12_opener_cannot_open_file_ftp_or_use_a_proxy(self):
        opener = port_web._guarded_opener()
        for forbidden in (urllib.request.FileHandler, urllib.request.FTPHandler,
                          urllib.request.ProxyHandler, urllib.request.DataHandler):
            self.assertFalse(
                any(isinstance(h, forbidden) for h in opener.handlers),
                f"guarded opener still installs {forbidden.__name__}",
            )
        with self.assertRaises(urllib.error.URLError):
            opener.open("file:///etc/passwd", timeout=5)

    def test_13_oversized_remote_document_is_refused(self):
        class FakeResponse:
            def __init__(self, total):
                self.remaining = total

            def read(self, n):
                take = min(n, self.remaining)
                self.remaining -= take
                return b"x" * take

        with self.assertRaises(port_web.ImportRefusedError):
            port_web._read_bounded(FakeResponse(1024), 512, "probe document")
        self.assertEqual(len(port_web._read_bounded(FakeResponse(256), 512, "probe document")), 256)

    def test_14_scheme_detection_does_not_mistake_a_drive_letter_for_a_url(self):
        """`data:` has no `//`; a substring test would route it to the folder branch."""
        for target, expected in (
            ("http://example.com/", "http"),
            ("HTTPS://example.com/", "https"),
            ("file:///etc/passwd", "file"),
            ("file:relative/path", "file"),
            ("data:text/html,<h1>hi</h1>", "data"),
            ("mailto:a@b.test", "mailto"),
            ("C:/Users/dev/app", ""),
            ("C:\\Users\\dev\\app", ""),
            ("templates/web_app_template", ""),
            ("./relative/app", ""),
            ("\\\\server\\share\\app", ""),
        ):
            self.assertEqual(port_web._import_target_scheme(target), expected, target)
        with self.assertRaises(port_web.ImportRefusedError):
            port_web.ingest_target("data:text/html,<h1>hi</h1>", self.staging)
        with self.assertRaises(port_web.ImportRefusedError):
            port_web.ingest_target("file:relative/path", self.staging)


class TestWebImportFolderPolicy(unittest.TestCase):
    """H16.2: local ingestion is confined to an explicit allowlist."""

    def setUp(self):
        self.tmp = tempfile.mkdtemp(prefix="interos_import_folder_")
        self.work = Path(self.tmp)
        self.staging = self.work / "staging"
        self.staging.mkdir()

    def tearDown(self):
        shutil.rmtree(self.tmp, ignore_errors=True)

    def test_01_real_host_paths_outside_the_allowlist_are_refused(self):
        roots = port_web.allowed_import_roots()
        for target in (Path.home(), ROOT.parent, Path(tempfile.gettempdir()).parent):
            if any(port_web._is_within(target.resolve(), r) for r in roots):
                continue  # this host genuinely keeps the workspace there
            with self.assertRaises(port_web.ImportRefusedError, msg=str(target)):
                port_web.resolve_import_folder(target)

    def test_02_copy_of_a_directory_outside_the_allowlist_is_refused(self):
        """Narrows the allowlist so the refusal is exercised on a real copy path."""
        allowed = self.work / "allowed_root"
        allowed.mkdir()
        outside = _make_webapp(self.work, "outside_app")
        with mock.patch.object(port_web, "ROOT_DIR", allowed), \
             mock.patch.object(port_web.tempfile, "gettempdir", return_value=str(allowed)):
            self.assertEqual(port_web.allowed_import_roots(), [allowed.resolve()])
            with self.assertRaises(port_web.ImportRefusedError):
                port_web.ingest_target(str(outside), self.staging)
        self.assertEqual(list(self.staging.iterdir()), [], "an out-of-allowlist tree was copied")

    def test_03_designated_import_root_is_honoured(self):
        allowed = self.work / "allowed_root"
        allowed.mkdir()
        app = _make_webapp(self.work, "designated")
        with mock.patch.object(port_web, "ROOT_DIR", allowed), \
             mock.patch.object(port_web.tempfile, "gettempdir", return_value=str(allowed)):
            with self.assertRaises(port_web.ImportRefusedError):
                port_web.resolve_import_folder(app)
            self.assertEqual(port_web.resolve_import_folder(app, extra_roots=[self.work]), app.resolve())
            with mock.patch.dict(os.environ, {"INTEROS_WEB_IMPORT_ROOTS": str(self.work)}):
                self.assertEqual(port_web.resolve_import_folder(app), app.resolve())

    def test_04_symlink_escape_inside_an_allowed_root_is_refused(self):
        app = _make_webapp(self.work, "linked_app")
        secret = self.work / "secret_dir"
        secret.mkdir()
        (secret / "credential.txt").write_text("SECRET", encoding="utf-8")
        try:
            os.symlink(secret, app / "escape", target_is_directory=True)
        except (OSError, NotImplementedError) as exc:
            self.skipTest(f"symlink creation unavailable on this host: {exc}")
        with self.assertRaises(port_web.ImportRefusedError):
            port_web.ingest_target(str(app), self.staging)
        self.assertFalse((self.staging / "escape").exists())
        self.assertFalse((self.staging / "credential.txt").exists())

    def test_05_symlinked_source_root_resolves_and_is_checked(self):
        real = _make_webapp(self.work, "real_app")
        link = self.work / "link_to_real"
        try:
            os.symlink(real, link, target_is_directory=True)
        except (OSError, NotImplementedError) as exc:
            self.skipTest(f"symlink creation unavailable on this host: {exc}")
        self.assertEqual(port_web.resolve_import_folder(link), real.resolve())
        outside = self.work / "outside_target"
        outside.mkdir()
        escape = self.work / "link_to_outside"
        try:
            os.symlink(outside, escape, target_is_directory=True)
        except (OSError, NotImplementedError) as exc:
            self.skipTest(f"symlink creation unavailable on this host: {exc}")
        with mock.patch.object(port_web, "ROOT_DIR", real), \
             mock.patch.object(port_web.tempfile, "gettempdir", return_value=str(real)):
            with self.assertRaises(port_web.ImportRefusedError):
                port_web.resolve_import_folder(escape)

    def test_06_key_material_and_credential_directories_are_refused(self):
        app = _make_webapp(self.work, "keyed_app")
        (app / "developer.key").write_text("NOT-A-REAL-KEY", encoding="utf-8")
        with self.assertRaises(port_web.ImportRefusedError):
            port_web.ingest_target(str(app), self.staging)

        app2 = _make_webapp(self.work, "pem_app")
        (app2 / "server.pem").write_text("NOT-A-REAL-PEM", encoding="utf-8")
        with self.assertRaises(port_web.ImportRefusedError):
            port_web.ingest_target(str(app2), self.staging)

        app3 = _make_webapp(self.work, "ssh_app")
        (app3 / ".ssh").mkdir()
        (app3 / ".ssh" / "id_rsa").write_text("NOT-A-REAL-KEY", encoding="utf-8")
        with self.assertRaises(port_web.ImportRefusedError):
            port_web.ingest_target(str(app3), self.staging)

    def test_07_repository_signing_key_directory_is_never_importable(self):
        """packages/ holds repository key material; it must never be copied."""
        with self.assertRaises(port_web.ImportRefusedError):
            port_web.resolve_import_folder(ROOT / "packages")
        key_path = ROOT / "packages" / "developer.key"
        if key_path.exists():
            # A single-file target copies its PARENT, so this must be refused too.
            with self.assertRaises(port_web.ImportRefusedError):
                port_web.resolve_import_folder(key_path)
        with self.assertRaises(port_web.ImportRefusedError):
            port_web.resolve_import_folder(ROOT / ".git")

    def test_08_nested_sensitive_entry_is_refused_during_the_copy(self):
        app = _make_webapp(self.work, "nested_app")
        nested = app / "assets" / "vendor"
        nested.mkdir(parents=True)
        (nested / "index.html").write_text("<html></html>", encoding="utf-8")
        (nested / "id_ed25519").write_text("NOT-A-REAL-KEY", encoding="utf-8")
        with self.assertRaises(port_web.ImportRefusedError):
            port_web.ingest_target(str(app), self.staging)

    def test_09_legitimate_workspace_and_staged_trees_still_import(self):
        app = _make_webapp(self.work, "good_app")
        (app / "assets").mkdir()
        (app / "assets" / "style.css").write_text("body{}", encoding="utf-8")
        port_web.ingest_target(str(app), self.staging)
        self.assertTrue((self.staging / "index.html").exists())
        self.assertTrue((self.staging / "assets" / "style.css").exists())
        # The Studio default target is the in-repo web template.
        staging2 = self.work / "staging2"
        staging2.mkdir()
        port_web.ingest_target(str(TEMPLATE), staging2)
        self.assertTrue((staging2 / "index.html").exists())

    def test_10_missing_path_is_still_a_file_not_found_error(self):
        with self.assertRaises(FileNotFoundError):
            port_web.ingest_target(str(self.work / "nope"), self.staging)


class TestWebImportSigningPolicy(unittest.TestCase):
    """H16.3: imported content is unsigned unless signing is explicitly requested."""

    def setUp(self):
        self.tmp = tempfile.mkdtemp(prefix="interos_import_sign_")
        self.work = Path(self.tmp)

    def tearDown(self):
        shutil.rmtree(self.tmp, ignore_errors=True)

    def test_01_default_output_is_unsigned_and_never_loads_a_developer_key(self):
        out = self.work / "unsigned.rapp"
        with mock.patch.object(
            port_web, "get_or_create_developer_key",
            side_effect=AssertionError("the default import path must not touch any signing key"),
        ) as loader:
            port_web.port_web(str(TEMPLATE), output=str(out))
        loader.assert_not_called()
        fields = _rapp_fields(out.read_bytes())
        self.assertEqual(fields["publisher"], b"\0" * 32, "unsigned package claims a publisher key")
        self.assertEqual(fields["signature"], b"\0" * 64, "unsigned package carries a signature")
        self.assertEqual(out.read_bytes()[:4], b"RAPP")

    def test_02_explicit_sign_opt_in_produces_a_verifiable_signature(self):
        try:
            from cryptography.hazmat.primitives.asymmetric import ed25519
        except ImportError:
            self.skipTest("cryptography is not installed; the signed path cannot be exercised")
        key = ed25519.Ed25519PrivateKey.generate()
        key_file = self.work / "ephemeral_ed25519.key"
        from cryptography.hazmat.primitives import serialization
        key_file.write_bytes(key.private_bytes(
            encoding=serialization.Encoding.PEM,
            format=serialization.PrivateFormat.PKCS8,
            encryption_algorithm=serialization.NoEncryption(),
        ))
        out = self.work / "signed.rapp"
        port_web.port_web(str(TEMPLATE), output=str(out), sign=True, key_path=str(key_file))
        data = out.read_bytes()
        fields = _rapp_fields(data)
        self.assertNotEqual(fields["signature"], b"\0" * 64)
        unsigned = bytearray(data)
        unsigned[0x70:0xB0] = b"\0" * 64
        ed25519.Ed25519PublicKey.from_public_bytes(fields["publisher"]).verify(
            fields["signature"], bytes(unsigned)
        )

    def test_03_key_without_sign_is_refused(self):
        with self.assertRaises(port_web.ImportRefusedError):
            port_web.port_web(str(TEMPLATE), output=str(self.work / "x.rapp"),
                              key_path=str(self.work / "some.key"))

    def test_04_cli_default_is_unsigned_and_cli_sign_is_opt_in(self):
        out = self.work / "cli_unsigned.rapp"
        res = subprocess.run(
            [sys.executable, str(PORT_WEB), str(TEMPLATE), "--output", str(out)],
            capture_output=True, text=True, cwd=str(ROOT), timeout=180,
        )
        self.assertEqual(res.returncode, 0, res.stderr + res.stdout)
        self.assertIn("UNSIGNED", res.stdout + res.stderr)
        self.assertEqual(_rapp_fields(out.read_bytes())["signature"], b"\0" * 64)

        res2 = subprocess.run(
            [sys.executable, str(PORT_WEB), str(TEMPLATE), "--output", str(self.work / "cli_key.rapp"),
             "--key", str(self.work / "absent.key")],
            capture_output=True, text=True, cwd=str(ROOT), timeout=180,
        )
        self.assertNotEqual(res2.returncode, 0, "--key without --sign must be refused")

    def test_05_cli_sign_opt_in_with_an_explicit_key_verifies(self):
        """The exact invocation a caller must use to get an installable package."""
        try:
            from cryptography.hazmat.primitives import serialization
            from cryptography.hazmat.primitives.asymmetric import ed25519
        except ImportError:
            self.skipTest("cryptography is not installed; the signed path cannot be exercised")
        key = ed25519.Ed25519PrivateKey.generate()
        key_file = self.work / "ephemeral_ed25519.key"
        key_file.write_bytes(key.private_bytes(
            encoding=serialization.Encoding.PEM,
            format=serialization.PrivateFormat.PKCS8,
            encryption_algorithm=serialization.NoEncryption(),
        ))
        out = self.work / "cli_signed.rapp"
        res = subprocess.run(
            [sys.executable, str(PORT_WEB), str(TEMPLATE), "--output", str(out),
             "--sign", "--key", str(key_file)],
            capture_output=True, text=True, cwd=str(ROOT), timeout=180,
        )
        self.assertEqual(res.returncode, 0, res.stderr + res.stdout)
        data = out.read_bytes()
        fields = _rapp_fields(data)
        self.assertNotEqual(fields["signature"], b"\0" * 64)
        unsigned = bytearray(data)
        unsigned[0x70:0xB0] = b"\0" * 64
        ed25519.Ed25519PublicKey.from_public_bytes(fields["publisher"]).verify(
            fields["signature"], bytes(unsigned)
        )

    def test_06_cli_help_documents_the_sign_opt_in_and_import_roots(self):
        res = subprocess.run([sys.executable, str(PORT_WEB), "--help"],
                             capture_output=True, text=True, cwd=str(ROOT), timeout=120)
        self.assertEqual(res.returncode, 0)
        self.assertIn("--sign", res.stdout)
        self.assertIn("--import-root", res.stdout)


class TestWebImportPermissionFailClosed(unittest.TestCase):
    """C2/H16 crossover: an unknown permission must not produce a package."""

    def setUp(self):
        self.tmp = tempfile.mkdtemp(prefix="interos_import_perm_")
        self.work = Path(self.tmp)

    def tearDown(self):
        shutil.rmtree(self.tmp, ignore_errors=True)

    def test_01_unknown_permission_in_a_manifest_refuses_the_whole_import(self):
        app = _make_webapp(self.work, "unknown_perm", permissions=["camera", "not-a-real-permission"])
        out = self.work / "unknown.rapp"
        res = subprocess.run(
            [sys.executable, str(PORT_WEB), str(app), "--output", str(out)],
            capture_output=True, text=True, cwd=str(ROOT), timeout=180,
        )
        self.assertNotEqual(res.returncode, 0, "an unknown permission produced a package")
        self.assertFalse(out.exists(), "a package was written for an unresolved permission")
        self.assertIn("unknown web permission", res.stderr + res.stdout)

    def test_02_unknown_permission_never_yields_ambient_network(self):
        for unknown in ("", "  ", "admin", "root", "shell", "debugger", "network-all", "*"):
            with self.assertRaises(port_web.ImportRefusedError, msg=repr(unknown)):
                port_web.map_web_permission(unknown)

    def test_03_camera_microphone_geolocation_sensor_emit_the_kernel_ids(self):
        app = _make_webapp(self.work, "sensors_app",
                           permissions=["camera", "microphone", "geolocation", "sensor"])
        out = self.work / "sensors.rapp"
        res = subprocess.run(
            [sys.executable, str(PORT_WEB), str(app), "--output", str(out)],
            capture_output=True, text=True, cwd=str(ROOT), timeout=180,
        )
        self.assertEqual(res.returncode, 0, res.stderr + res.stdout)
        fields = _rapp_fields(out.read_bytes())
        self.assertEqual(fields["caps"], [21, 22, 23])
        for forbidden in (4, 7, 8):
            self.assertNotIn(forbidden, fields["caps"],
                             f"web permission prompt granted forbidden manifest id {forbidden}")
        import zipfile
        with zipfile.ZipFile(io.BytesIO(fields["payload"])) as zf:
            app_json = json.loads(zf.read("app.json").decode("utf-8"))
        self.assertEqual(app_json["capabilities"], [21, 22, 23])
        self.assertEqual(app_json["consent_permissions"], [4, 7, 8, 12])

    def test_04_numeric_manifest_declaration_is_ladder_checked(self):
        self.assertEqual(port_web.map_web_permission("22"), ([22], [], ["SystemControlCamera"]))
        for bad in ("0", "31", "39", "999", "-1"):
            with self.assertRaises(port_web.ImportRefusedError, msg=bad):
                port_web.map_web_permission(bad)


if __name__ == "__main__":
    unittest.main()