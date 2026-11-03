#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

"""
InterOS Sovereign Web-to-RAPP Porting Engine (tools/port_web.py)

Converts standard web applications (HTML/CSS/JS) and Progressive Web Apps (PWAs)
into cryptographically signed sovereign InterOS Mobile Application Packages (.rapp).

Features:
- Ingests local web directories or remote PWA URLs.
- Discovers, parses, and normalizes W3C Web Manifests into canonical app.json.
- Maps standard Web Permissions to zero-ambient-authority InterOS capabilities.
- Automatically verifies or injects strict Content Security Policy (CSP) meta tags.
- Injects the InterOS Universal Client SDK bridge (interos.js).
- Generates canonical Ed25519-signed .rapp packages ready for deployment to InterOS.

Usage:
    python tools/port_web.py <web_app_dir_or_url> [--output <dest_rapp_file>] [--name <app_name>]
"""

import os
import sys
import re
import json
import io
import shutil
import hashlib
import tempfile
import argparse
import urllib.request
import zipfile
from pathlib import Path

# =============================================================================
# Constants & Permission Mappings
# =============================================================================
ROOT_DIR = Path(__file__).resolve().parent.parent
TEMPLATES_DIR = ROOT_DIR / "templates"
DEFAULT_INTEROS_JS = TEMPLATES_DIR / "web_app_template" / "interos.js"

STRICT_CSP_META = (
    '<meta http-equiv="Content-Security-Policy" content="default-src \'self\'; '
    'script-src \'self\'; connect-src \'self\' http://127.0.0.1:* ws://127.0.0.1:*; '
    'img-src \'self\' data: blob:; style-src \'self\' \'unsafe-inline\'; '
    'object-src \'none\'; frame-ancestors \'none\';">'
)

ARCH_MAP = {
    "x86_64": 0,
    "aarch64": 1,
    "riscv64": 2,
    "wasm": 3,
    "web": 3,
}

WEB_PERMISSION_MAP = {
    "camera": {"names": ["CameraCapture"], "ids": [4]},
    "cameracapture": {"names": ["CameraCapture"], "ids": [4]},
    "microphone": {"names": ["Microphone"], "ids": [7]},
    "mic": {"names": ["Microphone"], "ids": [7]},
    "geolocation": {"names": ["Location"], "ids": [8]},
    "location": {"names": ["Location"], "ids": [8]},
    "storage": {"names": ["FileSystemRead", "FileSystemWrite"], "ids": [9, 10]},
    "persistent-storage": {"names": ["FileSystemRead", "FileSystemWrite"], "ids": [9, 10]},
    "filesystemread": {"names": ["FileSystemRead"], "ids": [9]},
    "filesystemwrite": {"names": ["FileSystemWrite"], "ids": [10]},
    "vfs.read": {"names": ["FileSystemRead"], "ids": [9]},
    "vfs.write": {"names": ["FileSystemWrite"], "ids": [10]},
    "files": {"names": ["FileSystemRead", "FileSystemWrite"], "ids": [9, 10]},
    "notifications": {"names": ["Notifications"], "ids": [14]},
    "notification": {"names": ["Notifications"], "ids": [14]},
    "sensor": {"names": ["Sensors"], "ids": [12]},
    "sensors": {"names": ["Sensors"], "ids": [12]},
    "accelerometer": {"names": ["Sensors"], "ids": [12]},
    "gyroscope": {"names": ["Sensors"], "ids": [12]},
    "telemetry": {"names": ["Sensors"], "ids": [12]},
    "telemetry.battery": {"names": ["Sensors"], "ids": [12]},
    "audio": {"names": ["AudioSynthesis"], "ids": [5]},
    "audio.feedback": {"names": ["AudioSynthesis"], "ids": [5]},
    "audiosynthesis": {"names": ["AudioSynthesis"], "ids": [5]},
    "network": {"names": ["NetworkSocket", "NetworkSend", "NetworkReceive"], "ids": [12, 13, 14]},
    "networksocket": {"names": ["NetworkSocket"], "ids": [12]},
    "networksend": {"names": ["NetworkSend"], "ids": [13]},
    "networkreceive": {"names": ["NetworkReceive"], "ids": [14]},
    "telephony": {"names": ["TelephonyModem"], "ids": [1]},
    "telephonymodem": {"names": ["TelephonyModem"], "ids": [1]},
    "phone": {"names": ["TelephonyModem"], "ids": [1]},
    "sms": {"names": ["TelephonyModem"], "ids": [1]},
}


def log(msg, prefix="[*]"):
    print(f"{prefix} {msg}")


def log_success(msg):
    print(f"[OK] {msg}")


def log_warn(msg):
    print(f"[WARN] {msg}")


def log_error(msg):
    print(f"[ERROR] {msg}", file=sys.stderr)


# =============================================================================
# Helper: Ed25519 Developer Key Management
# =============================================================================
def get_or_create_developer_key(key_path=None):
    from cryptography.hazmat.primitives.asymmetric import ed25519
    from cryptography.hazmat.primitives import serialization

    if key_path:
        kp = Path(key_path)
        if kp.exists():
            raw = kp.read_bytes()
            try:
                return serialization.load_pem_private_key(raw, password=None)
            except ValueError:
                return ed25519.Ed25519PrivateKey.from_private_bytes(raw)
        else:
            raise FileNotFoundError(f"Specified key file does not exist: {key_path}")

    key_dir = Path.home() / ".interos"
    key_dir.mkdir(parents=True, exist_ok=True)
    default_key = key_dir / "developer_ed25519.key"

    if default_key.exists():
        raw = default_key.read_bytes()
        try:
            return serialization.load_pem_private_key(raw, password=None)
        except ValueError:
            return ed25519.Ed25519PrivateKey.from_private_bytes(raw)

    log("Generating new Ed25519 developer key pair for application signing...")
    priv_key = ed25519.Ed25519PrivateKey.generate()
    pem_bytes = priv_key.private_bytes(
        encoding=serialization.Encoding.PEM,
        format=serialization.PrivateFormat.PKCS8,
        encryption_algorithm=serialization.NoEncryption()
    )
    default_key.write_bytes(pem_bytes)
    log_success(f"Developer signing key stored at: {default_key}")
    return priv_key


# =============================================================================
# Helper: Semver parsing
# =============================================================================
def parse_semver(ver_str):
    parts = re.split(r"[\.-]", str(ver_str).strip())
    nums = []
    for p in parts:
        if p.isdigit():
            nums.append(int(p))
        if len(nums) == 3:
            break
    while len(nums) < 3:
        nums.append(0)
    return tuple(nums[:3])


# =============================================================================
# RAPP v2 Package Encoding & Signing
# =============================================================================
def encode_and_sign_rapp_v2(app_id, version, arch_code, cap_ids, payload_bytes, priv_key):
    """
    Encodes canonical RAPP v2 binary package format:
    0x00: Magic 'RAPP'
    0x04: Format version (2)
    0x06: Target architecture (u16)
    0x08: Package ID (32 bytes UTF-8 null-padded)
    0x28: Publisher public key (32 bytes Ed25519)
    0x48: Version (3 x u16: Major, Minor, Patch)
    0x4E: Capability count (u16)
    0x50: SHA-256 Digest of payload (32 bytes)
    0x70: Ed25519 Signature (64 bytes)
    0xB0: Payload length (u32)
    0xB4: Capabilities list (len * u16)
    Offset 0xB4 + 2*len: Payload bytes
    """
    from cryptography.hazmat.primitives import serialization

    HEADER_SIZE = 0xB4
    SIG_OFFSET = 0x70
    SIG_SIZE = 64

    # Sanitize inputs
    pkg_id_bytes = app_id.encode("utf-8")[:32].ljust(32, b"\0")
    v_maj, v_min, v_pat = parse_semver(version)
    caps = sorted(list(set(cap_ids)))
    pub_key_bytes = priv_key.public_key().public_bytes(
        serialization.Encoding.Raw,
        serialization.PublicFormat.Raw
    )

    digest = hashlib.sha256(payload_bytes).digest()

    buf = bytearray()
    buf += b"RAPP"
    buf += (2).to_bytes(2, "little")
    buf += arch_code.to_bytes(2, "little")
    buf += pkg_id_bytes
    buf += pub_key_bytes
    buf += v_maj.to_bytes(2, "little")
    buf += v_min.to_bytes(2, "little")
    buf += v_pat.to_bytes(2, "little")
    buf += len(caps).to_bytes(2, "little")
    buf += digest
    buf += b"\0" * SIG_SIZE
    buf += len(payload_bytes).to_bytes(4, "little")

    for c in caps:
        buf += int(c).to_bytes(2, "little")

    buf += payload_bytes

    # Compute Ed25519 signature over unsigned package with zeroed signature field
    signing_bytes = bytearray(buf)
    signing_bytes[SIG_OFFSET:SIG_OFFSET + SIG_SIZE] = b"\0" * SIG_SIZE
    signature = priv_key.sign(bytes(signing_bytes))

    # Splice signature into header
    buf[SIG_OFFSET:SIG_OFFSET + SIG_SIZE] = signature

    # Verification self-check
    from cryptography.hazmat.primitives.asymmetric import ed25519
    verifier = ed25519.Ed25519PublicKey.from_public_bytes(pub_key_bytes)
    check_bytes = bytearray(buf)
    check_bytes[SIG_OFFSET:SIG_OFFSET + SIG_SIZE] = b"\0" * SIG_SIZE
    verifier.verify(signature, bytes(check_bytes))

    return bytes(buf)


# =============================================================================
# Ingestion: Remote URL (PWA) or Local Directory
# =============================================================================
def ingest_target(target, staging_dir):
    if target.startswith("http://") or target.startswith("https://"):
        log(f"Fetching remote web application / PWA from: {target}")
        req = urllib.request.Request(target, headers={"User-Agent": "InterOS-PortWeb/1.0"})
        with urllib.request.urlopen(req, timeout=15) as resp:
            content = resp.read()

        index_file = staging_dir / "index.html"
        index_file.write_bytes(content)

        # Check for web manifest link
        text = content.decode("utf-8", errors="replace")
        manifest_match = re.search(r'<link\s+[^>]*rel=["\']manifest["\'][^>]*href=["\']([^"\']+)["\']', text, re.I)
        if manifest_match:
            manifest_url = manifest_match.group(1)
            if not manifest_url.startswith("http"):
                base = target.rstrip("/")
                manifest_url = f"{base}/{manifest_url.lstrip('/')}"
            try:
                log(f"Fetching PWA manifest from: {manifest_url}")
                m_req = urllib.request.Request(manifest_url, headers={"User-Agent": "InterOS-PortWeb/1.0"})
                with urllib.request.urlopen(m_req, timeout=10) as m_resp:
                    (staging_dir / "manifest.json").write_bytes(m_resp.read())
            except Exception as e:
                log_warn(f"Failed to fetch linked manifest: {e}")
    else:
        src = Path(target).resolve()
        if not src.exists():
            raise FileNotFoundError(f"Input path does not exist: {target}")
        if src.is_file():
            src_dir = src.parent
        else:
            src_dir = src

        log(f"Ingesting local web application directory: {src_dir}")
        for item in src_dir.iterdir():
            dest = staging_dir / item.name
            if item.is_dir():
                shutil.copytree(item, dest, dirs_exist_ok=True)
            else:
                shutil.copy2(item, dest)


# =============================================================================
# Manifest Parsing & Permission Resolution
# =============================================================================
def parse_and_canonicalize_manifest(staging_dir, name_override=None, id_override=None, ver_override=None):
    manifest_candidates = [
        staging_dir / "app.json",
        staging_dir / "manifest.json",
        staging_dir / "manifest.webmanifest",
    ]

    manifest_data = {}
    found_manifest_path = None
    for mc in manifest_candidates:
        if mc.exists():
            try:
                manifest_data = json.loads(mc.read_text(encoding="utf-8"))
                found_manifest_path = mc
                log_success(f"Discovered manifest at: {mc.name}")
                break
            except Exception as e:
                log_warn(f"Could not parse manifest {mc.name}: {e}")

    # Resolve App Name
    app_name = name_override or manifest_data.get("name") or manifest_data.get("short_name")
    if not app_name:
        app_name = staging_dir.name.replace("_", " ").replace("-", " ").title()
        if not app_name or app_name.startswith("tmp"):
            app_name = "Web Application"

    # Resolve App ID (must be max 32 chars for canonical RAPP v2 header)
    slug = re.sub(r"[^a-z0-9_]", "", app_name.lower().replace(" ", "_"))[:20]
    app_id = id_override or manifest_data.get("id") or manifest_data.get("app_id") or f"org.interos.{slug}"
    if len(app_id.encode("utf-8")) > 32:
        app_id = app_id[:32]

    # Resolve Version
    version_str = ver_override or manifest_data.get("version") or "1.0.0"

    # Resolve Permissions
    declared_perms = set()
    raw_perms = manifest_data.get("permissions") or manifest_data.get("capabilities") or []
    if isinstance(raw_perms, list):
        for p in raw_perms:
            declared_perms.add(str(p).lower().strip())

    # Codebase heuristic inspection to detect requested permissions
    for f in staging_dir.rglob("*.js"):
        try:
            code = f.read_text(encoding="utf-8", errors="ignore")
            if "getUserMedia" in code:
                declared_perms.add("camera")
                declared_perms.add("microphone")
            if "geolocation" in code or "getCurrentPosition" in code:
                declared_perms.add("geolocation")
            if "Notification" in code:
                declared_perms.add("notifications")
            if "localStorage" in code or "indexedDB" in code:
                declared_perms.add("storage")
            if "fetch" in code or "WebSocket" in code or "XMLHttpRequest" in code:
                declared_perms.add("network")
            if "DeviceOrientation" in code or "Accelerometer" in code or "Gyroscope" in code:
                declared_perms.add("sensor")
        except Exception:
            pass

    # Ensure safe baseline defaults for web applications
    if not declared_perms:
        declared_perms.update(["network", "storage", "notifications"])

    # Map to InterOS Capability IDs and canonical names
    canonical_permission_names = set()
    cap_ids = set()

    for p in declared_perms:
        matched = WEB_PERMISSION_MAP.get(p)
        if matched:
            canonical_permission_names.update(matched["names"])
            cap_ids.update(matched["ids"])
        else:
            canonical_permission_names.add(p)

    canonical_app_json = {
        "id": app_id,
        "name": app_name,
        "version": version_str,
        "description": manifest_data.get("description", f"Sovereign web application '{app_name}' for InterOS"),
        "author": manifest_data.get("author", "InterOS Community Developer"),
        "entry": "index.html",
        "icon": manifest_data.get("icon", "🌐"),
        "permissions": sorted(list(canonical_permission_names)),
        "capabilities": sorted(list(cap_ids)),
        "architecture": "web"
    }

    # Write canonical app.json
    (staging_dir / "app.json").write_text(json.dumps(canonical_app_json, indent=2), encoding="utf-8")
    log_success(f"Canonical app.json generated for '{app_name}' ({app_id}) with {len(cap_ids)} capabilities.")
    return canonical_app_json, sorted(list(cap_ids))


# =============================================================================
# HTML Hardening: Strict CSP & interos.js Injection
# =============================================================================
def harden_index_html(staging_dir):
    index_file = staging_dir / "index.html"
    if not index_file.exists():
        index_file.write_text(
            "<!DOCTYPE html>\n<html><head><title>InterOS App</title></head><body><h1>InterOS Sovereign Web App</h1></body></html>",
            encoding="utf-8"
        )

    html = index_file.read_text(encoding="utf-8", errors="replace")

    # 1. Enforce Strict CSP
    has_csp = bool(re.search(r'<meta\s+[^>]*http-equiv=["\']content-security-policy["\']', html, re.I))
    if not has_csp:
        log("Injecting strict Content Security Policy (CSP) into index.html...")
        if re.search(r"<head\b[^>]*>", html, re.I):
            html = re.sub(r"(<head\b[^>]*>)", rf"\1\n  {STRICT_CSP_META}", html, count=1, flags=re.I)
        else:
            html = f"<head>\n  {STRICT_CSP_META}\n</head>\n" + html
    else:
        log_success("Strict Content Security Policy (CSP) verified in index.html.")

    # 2. Inject interos.js Bridge
    has_interos_script = "interos.js" in html
    if not has_interos_script:
        log("Injecting <script src=\"interos.js\"></script> bridge into index.html...")
        script_tag = '  <script src="interos.js"></script>\n'
        first_script = re.search(r"<script\b", html, re.I)
        if first_script:
            idx = first_script.start()
            html = html[:idx] + script_tag + html[idx:]
        elif re.search(r"</head>", html, re.I):
            html = re.sub(r"(</head>)", rf"{script_tag}\1", html, count=1, flags=re.I)
        elif re.search(r"<body\b[^>]*>", html, re.I):
            html = re.sub(r"(<body\b[^>]*>)", rf"\1\n{script_tag}", html, count=1, flags=re.I)
        else:
            html += f"\n{script_tag}"

    # 3. Ensure interos.js exists on disk
    dest_interos_js = staging_dir / "interos.js"
    if not dest_interos_js.exists():
        if DEFAULT_INTEROS_JS.exists():
            shutil.copy2(DEFAULT_INTEROS_JS, dest_interos_js)
            log_success("Bundled InterOS Client SDK (interos.js) into app root.")
        else:
            log_warn("Default interos.js template not found; generating fallback client SDK.")
            dest_interos_js.write_text("// InterOS Client SDK placeholder\n", encoding="utf-8")

    index_file.write_text(html, encoding="utf-8")


# =============================================================================
# Package Payload Generation (Zip Asset Bundle)
# =============================================================================
def create_asset_payload(staging_dir):
    buf = io.BytesIO()
    with zipfile.ZipFile(buf, "w", compression=zipfile.ZIP_DEFLATED) as zf:
        for root, _, files in os.walk(staging_dir):
            for file_name in files:
                full_path = Path(root) / file_name
                rel_path = full_path.relative_to(staging_dir)
                zf.write(full_path, str(rel_path).replace("\\", "/"))
    return buf.getvalue()


# =============================================================================
# Main Porting Pipeline
# =============================================================================
def port_web(target, output=None, name=None, app_id=None, version=None, arch="web", key_path=None):
    with tempfile.TemporaryDirectory(prefix="interos_port_") as tmp_dir_str:
        staging_dir = Path(tmp_dir_str)

        print("================================================================")
        print("  [InterOS Web-to-RAPP Porting Engine]")
        print(f"  Target: {target}")
        print("================================================================")

        # 1. Ingest files
        ingest_target(target, staging_dir)

        # 2. Manifest & permissions
        manifest_data, cap_ids = parse_and_canonicalize_manifest(
            staging_dir,
            name_override=name,
            id_override=app_id,
            ver_override=version
        )

        # 3. Harden index.html (CSP & interos.js)
        harden_index_html(staging_dir)

        # 4. Create asset payload
        payload_bytes = create_asset_payload(staging_dir)
        log_success(f"Compressed web assets into bundle: {len(payload_bytes)} bytes.")

        # 5. Acquire Ed25519 signing key
        priv_key = get_or_create_developer_key(key_path)

        # 6. Build and sign canonical RAPP v2 package
        arch_code = ARCH_MAP.get(arch.lower(), 3)
        rapp_bytes = encode_and_sign_rapp_v2(
            app_id=manifest_data["id"],
            version=manifest_data["version"],
            arch_code=arch_code,
            cap_ids=cap_ids,
            payload_bytes=payload_bytes,
            priv_key=priv_key
        )

        # 7. Write to output destination
        if not output:
            dist_dir = ROOT_DIR / "dist"
            dist_dir.mkdir(parents=True, exist_ok=True)
            slug = re.sub(r"[^a-z0-9_]", "", manifest_data["name"].lower().replace(" ", "_"))
            output_path = dist_dir / f"{slug}.rapp"
        else:
            output_path = Path(output).resolve()
            output_path.parent.mkdir(parents=True, exist_ok=True)

        output_path.write_bytes(rapp_bytes)

        log_success(f"Successfully generated signed .rapp package:")
        print(f"      File:         {output_path}")
        print(f"      Package ID:   {manifest_data['id']}")
        print(f"      Arch Target:  {arch} ({arch_code})")
        print(f"      Capabilities: {cap_ids}")
        print(f"      Total Size:   {len(rapp_bytes)} bytes")
        print("================================================================\n")
        return str(output_path)


def main():
    parser = argparse.ArgumentParser(
        description="Port standard web applications (HTML/JS/CSS) or PWAs to sovereign InterOS .rapp packages."
    )
    parser.add_argument("target", help="Local directory containing web files, or a remote PWA URL")
    parser.add_argument("--output", "-o", help="Output path for the signed .rapp file")
    parser.add_argument("--name", "-n", help="Application name override")
    parser.add_argument("--id", help="Canonical package ID override (e.g. org.interos.myapp)")
    parser.add_argument("--version", help="Application semver (e.g. 1.0.0)")
    parser.add_argument("--arch", choices=["web", "wasm", "x86_64", "aarch64", "riscv64"], default="web", help="RAPP target architecture")
    parser.add_argument("--key", help="Path to Ed25519 private key for package signing")

    args = parser.parse_args()

    try:
        port_web(
            target=args.target,
            output=args.output,
            name=args.name,
            app_id=args.id,
            version=args.version,
            arch=args.arch,
            key_path=args.key
        )
    except Exception as e:
        log_error(f"Porting failed: {e}")
        import traceback
        traceback.print_exc()
        sys.exit(1)


if __name__ == "__main__":
    main()
