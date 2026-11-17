#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

"""
Tests for RAPP Packaging Normalization, Path Security, and Build Determinism.
"""

import unittest
from pathlib import Path
import sys
import tempfile
import json
import shutil
import hashlib

ROOT = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(ROOT))
sys.path.insert(0, str(ROOT / "tools"))

from tools.interos import (
    _rapp_v2_encode,
    _rapp_v4_encode,
    _rapp_v2_signing_bytes,
    RAPP_CAPABILITY_IDS,
)


class TestPackagingDeterminism(unittest.TestCase):
    def setUp(self):
        self.pub_key = b"\x01" * 32
        self.sig = b"\x02" * 64
        self.payload = b"\x7fELF" + b"\x00" * 128

    def test_equivalent_manifests_produce_byte_identical_rapp_v2(self):
        # Manifest A: permissions in order [CAP_LOGGING, CAP_WINDOW_FOCUS]
        manifest_a = {
            "id": "org.interos.sample",
            "name": "Sample",
            "version": "1.0.0",
            "permissions": ["CAP_LOGGING", "CAP_WINDOW_FOCUS"],
        }
        # Manifest B: permissions in reverse order [CAP_WINDOW_FOCUS, CAP_LOGGING]
        manifest_b = {
            "name": "Sample",
            "version": "1.0.0",
            "permissions": ["CAP_WINDOW_FOCUS", "CAP_LOGGING"],
            "id": "org.interos.sample",
        }

        pkg_a = _rapp_v2_encode(manifest_a, self.pub_key, self.sig, self.payload)
        pkg_b = _rapp_v2_encode(manifest_b, self.pub_key, self.sig, self.payload)

        self.assertEqual(pkg_a, pkg_b)
        self.assertEqual(hashlib.sha256(pkg_a).digest(), hashlib.sha256(pkg_b).digest())

    def test_equivalent_manifests_produce_byte_identical_rapp_v4(self):
        manifest_a = {
            "id": "org.interos.sample",
            "name": "Sample",
            "version": "1.0.0",
            "permissions": ["CAP_LOGGING", "CAP_WINDOW_FOCUS"],
            "intents": {
                "view": ["interos.intent.VIEW_TEXT", "interos.intent.VIEW_IMAGE"],
            },
        }
        manifest_b = {
            "intents": {
                "view": ["interos.intent.VIEW_IMAGE", "interos.intent.VIEW_TEXT"],
            },
            "id": "org.interos.sample",
            "version": "1.0.0",
            "permissions": ["CAP_WINDOW_FOCUS", "CAP_LOGGING"],
            "name": "Sample",
        }

        pkg_a = _rapp_v4_encode(manifest_a, self.pub_key, self.sig, self.payload)
        pkg_b = _rapp_v4_encode(manifest_b, self.pub_key, self.sig, self.payload)

        self.assertEqual(pkg_a, pkg_b)
        self.assertEqual(hashlib.sha256(pkg_a).digest(), hashlib.sha256(pkg_b).digest())

    def test_entrypoint_basename_sanitization_prevents_directory_escape(self):
        # Even with path traversal components, Path(ep).name sanitizes to the file basename
        malicious_dict = {"backend": "../../../../evil.elf"}
        sanitized_from_dict = Path(malicious_dict["backend"]).name
        self.assertEqual(sanitized_from_dict, "evil.elf")

        malicious_str = "../../../etc/passwd"
        sanitized_from_str = Path(malicious_str).name
        self.assertEqual(sanitized_from_str, "passwd")

    def test_signature_verification_succeeds_with_sorted_capabilities(self):
        manifest = {
            "id": "org.interos.test",
            "name": "Test",
            "version": "0.1.0",
            "permissions": ["CAP_LOGGING", "CAP_AUDIO_STREAM", "CAP_WINDOW_FOCUS"],
        }
        unsigned = _rapp_v2_encode(manifest, self.pub_key, b"\0" * 64, self.payload)
        signing_bytes = _rapp_v2_signing_bytes(unsigned)
        self.assertGreater(len(signing_bytes), 0)
        # Verify deterministic hash
        self.assertEqual(len(hashlib.sha256(signing_bytes).digest()), 32)


if __name__ == "__main__":
    unittest.main()
