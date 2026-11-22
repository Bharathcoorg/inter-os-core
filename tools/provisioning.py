# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

"""Explicit provisioning of external test artifacts.

External binary artifacts (upstream release APKs, model weights) are never
committed and normal test runs never fetch them: unit and contract suites must
stay deterministic and network-free. This module is the single canonical,
cross-platform entry point used by `python tools/interos.py provision ...` to
fetch such an artifact once, verify its pinned SHA-256 fail-closed, and install
it into its ignored location. Acceptance suites that need the artifact fail
with a clear instruction when it has not been provisioned this way.
"""

import hashlib
import sys
import tempfile
import urllib.request
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent

# Fossify File Manager 1.6.1 (GPLv3+), an upstream release artifact used as the
# genuine external APK acceptance input. Pinned release URL and digest; the
# file itself is ignored under emulator/data/apps/.
ANDROID_APK_PATH = ROOT / "emulator/data/apps/fossify_file_manager.apk"
ANDROID_APK_URL = (
    "https://github.com/FossifyOrg/File-Manager/releases/download/1.6.1/"
    "file-manager-13-foss-release.apk"
)
ANDROID_APK_SHA256 = "9e97d2faf55fb2702386c3dc51e38a6b073781016b125701d3ab668419a70008"


def verify_digest(path: Path, expected_sha256: str) -> bool:
    digest = hashlib.sha256()
    with open(path, "rb") as handle:
        for chunk in iter(lambda: handle.read(1 << 20), b""):
            digest.update(chunk)
    return digest.hexdigest() == expected_sha256


def provision_android_apk() -> Path:
    """Fetch the pinned acceptance APK once and verify it fail-closed.

    Raises SystemExit on a digest mismatch or download failure rather than
    installing bytes the pinned provenance does not vouch for.
    """
    if ANDROID_APK_PATH.is_file():
        if verify_digest(ANDROID_APK_PATH, ANDROID_APK_SHA256):
            print(f"APK already provisioned and verified: {ANDROID_APK_PATH}")
            return ANDROID_APK_PATH
        print(
            f"Existing APK digest mismatch at {ANDROID_APK_PATH}; refusing to use it.",
            file=sys.stderr,
        )
        raise SystemExit(1)

    print(f"Downloading pinned acceptance APK from {ANDROID_APK_URL} ...")
    with tempfile.NamedTemporaryFile(delete=False) as tmp:
        tmp_path = Path(tmp.name)
        try:
            with urllib.request.urlopen(ANDROID_APK_URL) as response:  # noqa: S310
                while True:
                    chunk = response.read(1 << 20)
                    if not chunk:
                        break
                    tmp.write(chunk)
        except Exception as exc:
            tmp_path.unlink(missing_ok=True)
            print(f"Download failed: {exc}", file=sys.stderr)
            raise SystemExit(1) from exc

    if not verify_digest(tmp_path, ANDROID_APK_SHA256):
        tmp_path.unlink(missing_ok=True)
        print("REFUSING to install: SHA-256 mismatch.", file=sys.stderr)
        print(f"  expected: {ANDROID_APK_SHA256}", file=sys.stderr)
        raise SystemExit(1)

    ANDROID_APK_PATH.parent.mkdir(parents=True, exist_ok=True)
    tmp_path.replace(ANDROID_APK_PATH)
    print(f"Provisioned and verified: {ANDROID_APK_PATH}")
    return ANDROID_APK_PATH
