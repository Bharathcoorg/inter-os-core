# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

"""Adversarial guard for emulator/server.py anti-DNS-rebind SSRF pinning.

Fails if the pinning, the deny-set, or the redirect re-validation regresses.
"""
import ipaddress
import os
import sys
import unittest

WORKSPACE_ROOT = os.environ.get(
    "INTEROS_REPO",
    os.path.dirname(os.path.dirname(os.path.abspath(__file__))),
)
sys.path.insert(0, os.path.join(WORKSPACE_ROOT, "emulator"))
import server  # noqa: E402


def _forbidden(ip_str):
    return server._proxy_address_is_forbidden(ipaddress.ip_address(ip_str))


def _resolve(url):
    return server.resolve_pinned_proxy_ip(url)


class SsrfPinningAdversarialTests(unittest.TestCase):
    def test_cgnat_range_is_blocked(self):
        for a in ("100.64.0.0", "100.64.0.1", "100.100.100.200", "100.127.255.254"):
            self.assertTrue(_forbidden(a), f"CGNAT {a} must be forbidden")
            safe, _r, pinned = _resolve(f"http://{a}/")
            self.assertFalse(safe)
            self.assertIsNone(pinned)

    def test_cloud_metadata_blocked(self):
        self.assertTrue(_forbidden("169.254.169.254"))
        safe, _r, pinned = _resolve("http://169.254.169.254/latest/meta-data/")
        self.assertFalse(safe)
        self.assertIsNone(pinned)
        for host in ("metadata.google.internal", "instance-data",
                     "instance-data.ec2.internal", "metadata.azure.internal"):
            safe, _r, pinned = _resolve(f"http://{host}/")
            self.assertFalse(safe, host)
            self.assertIsNone(pinned)

    def test_loopback_private_linklocal_etc_blocked(self):
        for a in ("127.0.0.1", "10.0.0.1", "192.168.1.1", "172.16.0.1", "0.0.0.0",
                  "224.0.0.1", "255.255.255.255", "198.18.0.1", "192.0.2.5",
                  "203.0.113.9", "::1", "fd00::1", "fe80::1", "::"):
            self.assertTrue(_forbidden(a), a)

    def test_public_addresses_allowed_and_pinned(self):
        for a in ("8.8.8.8", "1.1.1.1", "2606:4700::1111"):
            self.assertFalse(_forbidden(a), a)
        safe, reason, pinned = _resolve("http://8.8.8.8/")
        self.assertTrue(safe, reason)
        self.assertEqual(pinned, "8.8.8.8")

    def test_non_http_schemes_rejected(self):
        for u in ("file:///etc/passwd", "ftp://8.8.8.8/x", "gopher://8.8.8.8/",
                  "data:text/plain,hi", "jar:http://8.8.8.8/a"):
            safe, _r, pinned = _resolve(u)
            self.assertFalse(safe, u)
            self.assertIsNone(pinned)

    def test_obfuscated_loopback_rejected(self):
        for u in ("http://2130706433/", "http://0x7f000001/", "http://0177.0.0.1/",
                  "http://127.1/", "http://0/"):
            safe, _r, pinned = _resolve(u)
            self.assertFalse(safe, u)
            self.assertIsNone(pinned)

    def test_userinfo_does_not_bypass(self):
        safe, _r, pinned = _resolve("http://8.8.8.8@127.0.0.1/")
        self.assertFalse(safe)
        self.assertIsNone(pinned)

    def test_anti_rebind_wiring_present(self):
        src = open(os.path.join(WORKSPACE_ROOT, "emulator", "server.py"), encoding="utf-8").read()
        self.assertIn("def resolve_pinned_proxy_ip(", src)
        self.assertIn("def _pinned_proxy_connection_class(", src)
        self.assertIn("def _browser_proxy_opener(", src)
        self.assertIn("socket.create_connection((pinned_ip, address[1])", src)
        self.assertIn("_safe, _reason, pinned_ip = resolve_pinned_proxy_ip(target_url)", src)
        self.assertIn("opener = _browser_proxy_opener(pinned_ip)", src)
        self.assertIn("100.64.0.0/10", src)
        self.assertIn("class _ProxyRedirectPassthrough", src)


if __name__ == "__main__":
    unittest.main(verbosity=2)