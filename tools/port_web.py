#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

"""
InterOS Sovereign Web-to-RAPP Porting Engine (tools/port_web.py)

Converts standard web applications (HTML/CSS/JS) and Progressive Web Apps (PWAs)
into sovereign InterOS Mobile Application Packages (.rapp).

Features:
- Ingests local web directories or remote PWA URLs.
- Discovers, parses, and normalizes W3C Web Manifests into canonical app.json.
- Maps standard Web Permissions to zero-ambient-authority InterOS capabilities.
- Automatically verifies or injects strict Content Security Policy (CSP) meta tags.
- Injects the InterOS Universal Client SDK bridge (interos.js).
- Emits an UNSIGNED .rapp by default; signing is an explicit opt-in (--sign).

HOST-SIDE SECURITY BOUNDARY
---------------------------
This tool runs on the DEVELOPER HOST, not inside the guest. Its import policy is
a host defence and is deliberately separate from the guest kernel's capability
enforcement:

- Remote ingestion is guarded against SSRF. Only http/https is allowed, every
  resolved address (initial URL, every redirect hop, and the second-order PWA
  manifest fetch) must be a public unicast address, and the socket is pinned to
  the address that was validated so a DNS rebinding between check and connect
  cannot redirect the fetch at loopback or cloud metadata.
- Local ingestion is confined to an explicit allowlist of import roots, refuses
  symlinks, and refuses sensitive host locations (key material, credential
  directories, repository internals).
- Imported third-party content is NOT signed with the developer key by default.
  Signing is what turns an exfiltrated file into an installable trusted package,
  so it is a separate, explicit step: pass --sign (optionally with --key).

Usage:
    python tools/port_web.py <web_app_dir_or_url> [--output <dest_rapp_file>] [--name <app_name>]
    python tools/port_web.py <web_app_dir_or_url> --sign [--key <ed25519_private_key>]
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
import http.client
import ipaddress
import socket
import urllib.error
import urllib.parse
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


class ImportRefusedError(RuntimeError):
    """A web-import target was refused by the host-side import policy.

    Raised for SSRF attempts, paths outside the import allowlist, sensitive host
    locations, symlinks, and permission strings that cannot be resolved. Every
    one of these is a fail-closed refusal: the tool never falls back to a
    permissive default.
    """


# -----------------------------------------------------------------------------
# RAPP v2 manifest capability ladder (ids 1..=30).
#
# Pinned to kernel/src/capability/mod.rs (CapabilityType::to_raw),
# kernel/src/task/user.rs (capabilities_from_rapp_with_trust) and
# docs/RAPP_FORMAT.md section 3. Package verification accepts 1..=30 only
# (kernel/src/package/rapp.rs, parse_rapp).
#
# This is the MANIFEST id space, NOT the consent id space. In the kernel raw type
# space 4 is HardwarePortIo, 7 is ProcessSpawn and 8 is ProcessKill. In the
# consent space (kernel/src/capability/permission.rs, SensitivePermission) those
# same numbers mean CameraCapture / Microphone / Location. The minter now refuses
# manifest 4/7/8 without system platform trust and reshapes them through
# sensitive_manifest_shape; a web permission must still never name them.
# -----------------------------------------------------------------------------
RAPP_MANIFEST_CAPABILITY_NAMES = {
    1: "SerialOutput",
    2: "FrameBufferDirect",
    3: "TimerTickRead",
    4: "HardwarePortIo",
    5: "IpcSend",
    6: "IpcReceive",
    7: "ProcessSpawn",
    8: "ProcessKill",
    9: "FileSystemRead",
    10: "FileSystemWrite",
    11: "StorageRawBlock",
    12: "NetworkSocket",
    13: "NetworkSend",
    14: "NetworkReceive",
    15: "ShmProducer",
    16: "ShmConsumer",
    17: "EventPoll",
    18: "SystemControl",
    19: "SystemControlPower",      # SystemControl @ SERVICE_POWER_CONTROL_OBJECT
    20: "SystemControlTelephony",  # SystemControl @ SERVICE_TELEPHONY_CONTROL_OBJECT
    21: "SystemControlAudio",      # SystemControl @ SERVICE_AUDIO_CONTROL_OBJECT
    22: "SystemControlCamera",     # SystemControl @ SERVICE_CAMERA_CONTROL_OBJECT
    23: "SystemControlSensor",     # SystemControl @ SERVICE_SENSOR_CONTROL_OBJECT
    24: "RealtimeClockRead",
    25: "ContentIndexRead",
    26: "ContentIntentDispatch",
    27: "SurfaceCapture",
    28: "AudioStream",
    29: "MemoryHeap",
    30: "ProcessThreads",
}

MANIFEST_CAPABILITY_MIN = 1
MANIFEST_CAPABILITY_MAX = 30

# Consent-broker permission ids (kernel/src/capability/permission.rs:37-85).
# These are granted by a user consent prompt at runtime and are NEVER written
# into the manifest header.
SENSITIVE_PERMISSION_NAMES = {
    1: "TelephonyModem",
    2: "StorageSectorCrypt",
    3: "NetworkSend",
    4: "CameraCapture",      # mints HardwarePortIo @ 0xCA00
    5: "AudioSynthesis",
    6: "RtcClock",
    7: "Microphone",         # mints HardwarePortIo @ 0xCA01
    8: "Location",           # mints HardwarePortIo @ 0xCA02
    9: "Contacts",
    10: "Files",
    11: "Sms",
    12: "Sensors",           # mints HardwarePortIo @ 0x5E45
    13: "Bluetooth",
    14: "Notifications",     # mints IpcSend @ 0x9071
    15: "AgentTask",
    16: "AgentToolCalculator",
    17: "AgentToolClock",
    18: "AgentToolStorage",
    19: "AgentToolSettings",
    20: "AgentToolCapCheck",
}


def is_manifest_capability(cid):
    """True when cid is inside the RAPP v2 manifest ladder the kernel accepts."""
    return isinstance(cid, int) and MANIFEST_CAPABILITY_MIN <= cid <= MANIFEST_CAPABILITY_MAX


# -----------------------------------------------------------------------------
# Web permission -> RAPP manifest ids + consent ids.
#
# Mirrored by sdk/go/rapp/builder.go (webPermissionGrants) and
# sdk/typescript/src/builder.ts (WEB_PERMISSION_GRANTS).
# tests/test_permission_mapping_contract.py pins all three against each other
# and against the kernel table.
#
# Every manifest id below comes from the kernel's own minter
# (kernel/src/task/user.rs, capabilities_from_rapp_with_trust):
#   camera     -> 22 SystemControl @ SERVICE_CAMERA_CONTROL_OBJECT
#   microphone -> 21 SystemControl @ SERVICE_AUDIO_CONTROL_OBJECT
#   sensor     -> 23 SystemControl @ SERVICE_SENSOR_CONTROL_OBJECT
#   telephony  -> 20 SystemControl @ SERVICE_TELEPHONY_CONTROL_OBJECT
#   telemetry  -> 19 SystemControl @ SERVICE_POWER_CONTROL_OBJECT
#   audio      -> 28 AudioStream, AC'97 PCM output
# and the consent policy in kernel/src/package/dependency.rs
# (from_rapp_capabilities) agrees: 21 -> Microphone, 22 -> Camera, 23 -> Sensors.
#
# geolocation and notifications have NO manifest id: the ladder defines no GNSS
# object, and NotificationPost (syscall 110) authorizes on the caller's own
# package identity. dependency.rs derives neither Permission::Location nor
# Permission::Notifications from any manifest id, so their only honest grant is
# a consent request and their "ids" list is empty.
#
# There is NO default entry. An unrecognized permission string is refused.
# -----------------------------------------------------------------------------
WEB_PERMISSION_MAP = {
    "camera": {"names": ["CameraCapture"], "ids": [22], "consent": [4]},
    "cameracapture": {"names": ["CameraCapture"], "ids": [22], "consent": [4]},
    "microphone": {"names": ["Microphone"], "ids": [21], "consent": [7]},
    "mic": {"names": ["Microphone"], "ids": [21], "consent": [7]},
    "geolocation": {"names": ["Location"], "ids": [], "consent": [8]},
    "location": {"names": ["Location"], "ids": [], "consent": [8]},
    "storage": {"names": ["FileSystemRead", "FileSystemWrite"], "ids": [9, 10], "consent": [10]},
    "persistent-storage": {"names": ["FileSystemRead", "FileSystemWrite"], "ids": [9, 10], "consent": [10]},
    "filesystemread": {"names": ["FileSystemRead"], "ids": [9], "consent": []},
    "filesystemwrite": {"names": ["FileSystemWrite"], "ids": [10], "consent": []},
    "vfs.read": {"names": ["FileSystemRead"], "ids": [9], "consent": []},
    "vfs.write": {"names": ["FileSystemWrite"], "ids": [10], "consent": []},
    "files": {"names": ["FileSystemRead", "FileSystemWrite"], "ids": [9, 10], "consent": [10]},
    "notifications": {"names": ["Notifications"], "ids": [], "consent": [14]},
    "notification": {"names": ["Notifications"], "ids": [], "consent": [14]},
    "sensor": {"names": ["Sensors"], "ids": [23], "consent": [12]},
    "sensors": {"names": ["Sensors"], "ids": [23], "consent": [12]},
    "accelerometer": {"names": ["Sensors"], "ids": [23], "consent": [12]},
    "gyroscope": {"names": ["Sensors"], "ids": [23], "consent": [12]},
    "telemetry": {"names": ["SystemControlPower"], "ids": [19], "consent": []},
    "telemetry.battery": {"names": ["SystemControlPower"], "ids": [19], "consent": []},
    "audio": {"names": ["AudioStream"], "ids": [28], "consent": [5]},
    "audio.feedback": {"names": ["AudioStream"], "ids": [28], "consent": [5]},
    "audiosynthesis": {"names": ["AudioStream"], "ids": [28], "consent": [5]},
    "network": {"names": ["NetworkSocket", "NetworkSend", "NetworkReceive"], "ids": [12, 13, 14], "consent": [3]},
    "networksocket": {"names": ["NetworkSocket"], "ids": [12], "consent": []},
    "networksend": {"names": ["NetworkSend"], "ids": [13], "consent": []},
    "networkreceive": {"names": ["NetworkReceive"], "ids": [14], "consent": []},
    "telephony": {"names": ["TelephonyModem"], "ids": [20], "consent": [1]},
    "telephonymodem": {"names": ["TelephonyModem"], "ids": [20], "consent": [1]},
    "phone": {"names": ["TelephonyModem"], "ids": [20], "consent": [1]},
    "sms": {"names": ["TelephonyModem"], "ids": [20], "consent": [1]},
}


def map_web_permission(perm):
    """Resolve one web permission string to (manifest_ids, consent_ids, names).

    Raises ImportRefusedError for an unrecognized string. There is no fail-open
    default: an unknown permission must never yield ambient network access.
    """
    key = str(perm).lower().strip()
    matched = WEB_PERMISSION_MAP.get(key)
    if matched is not None:
        return list(matched["ids"]), list(matched["consent"]), list(matched["names"])
    # A manifest may declare numeric capability ids directly (the "capabilities"
    # field of a canonical app.json). Accept only ids inside the kernel ladder.
    if key.isdigit() and is_manifest_capability(int(key)):
        cid = int(key)
        return [cid], [], [RAPP_MANIFEST_CAPABILITY_NAMES[cid]]
    raise ImportRefusedError(
        f"unknown web permission {perm!r}: no canonical RAPP capability or consent "
        "permission resolves it, so no grant is issued (fail closed)"
    )


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
def encode_rapp_v2(app_id, version, arch_code, cap_ids, payload_bytes, priv_key=None):
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

    With priv_key=None the package is UNSIGNED: the publisher key and signature
    fields stay zero, so the kernel refuses it until a publisher deliberately
    signs it and the device owner provisions that publisher's trust root. This is
    the default for imported third-party content -- signing it with the local
    developer key is what would turn an exfiltrated host file into an installable
    trusted package.
    """
    HEADER_SIZE = 0xB4
    SIG_OFFSET = 0x70
    SIG_SIZE = 64

    # Sanitize inputs
    pkg_id_bytes = app_id.encode("utf-8")[:32].ljust(32, b"\0")
    v_maj, v_min, v_pat = parse_semver(version)
    caps = sorted(set(int(c) for c in cap_ids))
    for cid in caps:
        if not is_manifest_capability(cid):
            raise ImportRefusedError(
                f"capability {cid} is outside the RAPP v2 manifest ladder (1..=30); "
                "the kernel rejects it in parse_rapp's manifest capability check "
                "(!(1..=30).contains(&cap), kernel/src/package/rapp.rs)"
            )
    if priv_key is None:
        pub_key_bytes = b"\0" * 32
    else:
        from cryptography.hazmat.primitives import serialization
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
        buf += c.to_bytes(2, "little")

    buf += payload_bytes

    if priv_key is None:
        return bytes(buf)

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


def encode_and_sign_rapp_v2(app_id, version, arch_code, cap_ids, payload_bytes, priv_key):
    """Explicitly signed encoding. Requires a private key; never signs implicitly."""
    if priv_key is None:
        raise ValueError("encode_and_sign_rapp_v2 requires an explicit private key")
    return encode_rapp_v2(app_id, version, arch_code, cap_ids, payload_bytes, priv_key)


# =============================================================================
# Host-side import policy: SSRF guard for remote targets
# =============================================================================
# WHY this lives here and not only in the Studio endpoint: tools/port_web.py is
# the component that actually opens the socket and copies the bytes. The HTTP
# handler in emulator/server.py forwards a caller-supplied `folder`/`url` string
# to this tool, so the boundary has to be enforced where the fetch happens.
# This is a DEVELOPER-HOST defence; it is not a substitute for the guest kernel's
# capability enforcement.

BLOCKED_HOSTNAMES = frozenset({
    "localhost",
    "localhost.localdomain",
    "metadata.google.internal",
    "instance-data",
    "instance-data.ec2.internal",
    "169.254.169.254",
})

# Ranges that are not reachable on the public internet but that Python's
# is_private/is_reserved flags do not all cover (CGNAT 100.64/10, NAT64, the
# documentation and benchmarking blocks). Refused explicitly so the guard does
# not depend on one interpreter version's classification tables.
EXTRA_DENY_NETWORKS = tuple(
    ipaddress.ip_network(cidr)
    for cidr in (
        "0.0.0.0/8",
        "100.64.0.0/10",
        "192.0.0.0/24",
        "192.0.2.0/24",
        "198.18.0.0/15",
        "198.51.100.0/24",
        "203.0.113.0/24",
        "240.0.0.0/4",
        "255.255.255.255/32",
        "::/128",
        "64:ff9b::/96",
        "64:ff9b:1::/48",
        "2001:db8::/32",
        "3ffe::/16",
        "5f00::/16",
    )
)

# A fetched page or manifest is a web document, not a disk image. Bounding the
# read stops a hostile origin from exhausting host memory or disk.
MAX_REMOTE_DOCUMENT_BYTES = 32 * 1024 * 1024
MAX_REMOTE_MANIFEST_BYTES = 4 * 1024 * 1024


def _address_is_forbidden(ip):
    """True for any address that is not public unicast internet space."""
    # An IPv4-mapped IPv6 address (::ffff:127.0.0.1) must be judged on the IPv4
    # it embeds, not on the IPv6 wrapper.
    mapped = getattr(ip, "ipv4_mapped", None)
    if mapped is not None:
        ip = mapped
    if (
        ip.is_loopback
        or ip.is_private
        or ip.is_link_local
        or ip.is_reserved
        or ip.is_multicast
        or ip.is_unspecified
        or not ip.is_global
    ):
        return True
    return any(ip.version == net.version and ip in net for net in EXTRA_DENY_NETWORKS)


def resolve_import_url(url):
    """Validate a remote import URL and pin the address it may connect to.

    Returns (hostname, port, pinned_ip). Raises ImportRefusedError for a
    non-http(s) scheme, a reserved hostname, an unresolvable host, or any
    resolved address that is not public unicast space.

    WHY every resolved address is checked, not just the first: a hostile
    authoritative DNS can answer with a public record and a private one, and
    picking whichever connects first would let the private one through.

    WHY the address is returned: the caller pins the socket to it, so a second
    DNS answer (rebinding) between this check and the connection cannot move the
    fetch onto loopback or link-local metadata.
    """
    try:
        parsed = urllib.parse.urlsplit(url)
    except ValueError as exc:
        raise ImportRefusedError(f"invalid import URL {url!r}: {exc}") from exc

    if parsed.scheme not in ("http", "https"):
        raise ImportRefusedError(
            f"SSRF refusal: scheme {parsed.scheme!r} is not allowed for web import "
            "(only http and https)"
        )
    hostname = parsed.hostname
    if not hostname:
        raise ImportRefusedError(f"SSRF refusal: import URL {url!r} has no hostname")

    lowered = hostname.lower()
    if lowered in BLOCKED_HOSTNAMES or lowered.endswith(".local") or lowered.endswith(".internal"):
        raise ImportRefusedError(
            f"SSRF refusal: {hostname!r} is an internal/reserved hostname"
        )

    try:
        port = parsed.port or (443 if parsed.scheme == "https" else 80)
    except ValueError as exc:
        raise ImportRefusedError(f"invalid port in import URL {url!r}: {exc}") from exc

    try:
        infos = socket.getaddrinfo(hostname, port, type=socket.SOCK_STREAM)
    except OSError as exc:
        # Fail closed: an unresolvable host is not a reason to try something else.
        raise ImportRefusedError(f"SSRF refusal: could not resolve {hostname!r}: {exc}") from exc
    if not infos:
        raise ImportRefusedError(f"SSRF refusal: {hostname!r} resolved to no addresses")

    pinned = None
    for info in infos:
        ip_str = info[4][0]
        try:
            ip = ipaddress.ip_address(ip_str)
        except ValueError as exc:
            raise ImportRefusedError(f"SSRF refusal: unparseable address {ip_str!r}") from exc
        if _address_is_forbidden(ip):
            raise ImportRefusedError(
                f"SSRF refusal: {hostname!r} resolves to non-public address {ip_str} "
                "(loopback/private/link-local/unique-local/multicast/reserved space is forbidden)"
            )
        if pinned is None:
            pinned = ip_str
    return hostname, port, pinned


def _pinned_connection_class(base, pinned_ip):
    """A connection class that connects to the validated IP, keeping the hostname.

    Only `_create_connection` is replaced, so `self.host` stays the original
    hostname: the Host header, TLS SNI and certificate verification all still
    name the origin the caller asked for. What changes is the address the socket
    actually reaches, which is the one the SSRF guard approved.
    """
    class _PinnedConnection(base):
        def __init__(self, host, **kwargs):
            super().__init__(host, **kwargs)

            def _connect(address, timeout=None, source_address=None):
                return socket.create_connection((pinned_ip, address[1]), timeout, source_address)

            self._create_connection = _connect

    return _PinnedConnection


class _GuardedHTTPHandler(urllib.request.HTTPHandler):
    def http_open(self, req):
        _, _, pinned = resolve_import_url(req.full_url)
        return self.do_open(_pinned_connection_class(http.client.HTTPConnection, pinned), req)


class _GuardedHTTPSHandler(urllib.request.HTTPSHandler):
    def https_open(self, req):
        _, _, pinned = resolve_import_url(req.full_url)
        return self.do_open(
            _pinned_connection_class(http.client.HTTPSConnection, pinned),
            req,
            context=self._context,
        )


class _GuardedRedirectHandler(urllib.request.HTTPRedirectHandler):
    """Re-validates every redirect hop before it is followed.

    urllib follows redirects implicitly *after* the original URL was checked, so
    a public origin could answer 302 "Location: http://169.254.169.254/" and walk
    the importer onto cloud metadata. Each hop is a fresh URL and must pass the
    same guard. The base class still enforces its own hop and repeat limits.
    """

    def redirect_request(self, req, fp, code, msg, headers, newurl):
        resolve_import_url(newurl)
        return super().redirect_request(req, fp, code, msg, headers, newurl)


def _guarded_opener():
    """An opener that can only reach validated public http(s) origins.

    Built from an explicit handler list rather than build_opener(): the default
    opener also installs FileHandler, FTPHandler and DataHandler, which would
    happily open file:// and data: URLs, and a ProxyHandler that would honour
    HTTP_PROXY and send the request somewhere the pin never validated.

    UnknownHandler IS installed: without it OpenerDirector.open() returns None
    for an unhandled scheme instead of raising, which would turn a refusal into
    a silent no-op. It only raises URLError; it opens nothing.
    """
    opener = urllib.request.OpenerDirector()
    opener.add_handler(urllib.request.UnknownHandler())
    opener.add_handler(urllib.request.HTTPDefaultErrorHandler())
    opener.add_handler(urllib.request.HTTPErrorProcessor())
    opener.add_handler(_GuardedHTTPHandler())
    opener.add_handler(_GuardedHTTPSHandler())
    opener.add_handler(_GuardedRedirectHandler())
    return opener


def _read_bounded(resp, limit, what):
    chunks = []
    total = 0
    while True:
        chunk = resp.read(65536)
        if not chunk:
            break
        total += len(chunk)
        if total > limit:
            raise ImportRefusedError(f"{what} exceeds the {limit}-byte import limit")
        chunks.append(chunk)
    return b"".join(chunks)


def fetch_remote_document(url, limit=MAX_REMOTE_DOCUMENT_BYTES, timeout=15):
    """Fetch one remote document through the SSRF guard. Returns (bytes, final_url)."""
    resolve_import_url(url)  # validate before any socket is opened
    req = urllib.request.Request(url, headers={"User-Agent": "InterOS-PortWeb/1.0"})
    opener = _guarded_opener()
    with opener.open(req, timeout=timeout) as resp:
        final_url = resp.geturl()
        # The response URL is re-checked too: it is the origin the bytes came from.
        resolve_import_url(final_url)
        content = _read_bounded(resp, limit, f"remote document {url}")
    return content, final_url


# =============================================================================
# Host-side import policy: local folder allowlist
# =============================================================================
# WHY an allowlist and not a denylist: the Studio endpoint forwards a
# caller-supplied path straight to this tool, and `shutil.copytree` of an
# arbitrary host directory turns a web import into arbitrary host-file
# exfiltration. Only roots the operator has actually designated for import are
# readable, and sensitive locations are refused even inside an allowed root.

# Directory names that must never be copied into a package, wherever they appear.
SENSITIVE_DIR_NAMES = frozenset({
    ".git", ".hg", ".svn",
    ".ssh", ".interos", ".aws", ".azure", ".gnupg", ".kube", ".docker",
})

# File names that must never be copied into a package.
SENSITIVE_FILE_NAMES = frozenset({
    "developer.key", "developer_ed25519.key",
    "id_rsa", "id_dsa", "id_ecdsa", "id_ed25519",
    ".env", ".netrc", ".npmrc", ".pypirc",
    "credentials.json", "secrets.json", "service-account.json",
})

# Suffixes that carry private key / keystore material. A web bundle has no
# legitimate use for any of them, so they are refused rather than inspected.
SENSITIVE_FILE_SUFFIXES = (".key", ".pem", ".p12", ".pfx", ".jks", ".keystore")

# Absolute locations that are refused outright, even if an operator adds a root
# that contains them. packages/ holds the repository's signing key material.
SENSITIVE_ABSOLUTE_PATHS = (
    ROOT_DIR / "packages",
    Path.home() / ".interos",
    Path.home() / ".ssh",
    Path.home() / ".aws",
    Path.home() / ".gnupg",
)


def allowed_import_roots(extra_roots=None):
    """The explicit allowlist of directories a local web import may read.

    Default roots:
      * the repository workspace (this is where templates/ and apps/ live, and
        what Studio's default `template` target resolves to);
      * the host staging temp dir, because port_web itself stages into a
        TemporaryDirectory and the scaffolder hands it freshly staged trees.

    An operator may designate further roots with INTEROS_WEB_IMPORT_ROOTS
    (os.pathsep-separated) or with --import-root. Nothing else is readable.
    """
    roots = [ROOT_DIR.resolve(), Path(tempfile.gettempdir()).resolve()]
    env_roots = os.environ.get("INTEROS_WEB_IMPORT_ROOTS", "")
    candidates = list(extra_roots or []) + [p for p in env_roots.split(os.pathsep) if p.strip()]
    for candidate in candidates:
        roots.append(Path(candidate).expanduser().resolve())
    deduped = []
    for root in roots:
        if root not in deduped:
            deduped.append(root)
    return deduped


def _is_within(path, root):
    """True when path is root or lies underneath it (both already resolved)."""
    try:
        path.relative_to(root)
        return True
    except ValueError:
        return False


def _reject_sensitive_path(path):
    """Refuse credential directories, key material and repository internals."""
    resolved = Path(path).resolve()
    for sensitive in SENSITIVE_ABSOLUTE_PATHS:
        try:
            sensitive_resolved = sensitive.resolve()
        except OSError:
            continue
        if _is_within(resolved, sensitive_resolved):
            raise ImportRefusedError(
                f"import refused: {resolved} is inside sensitive host location {sensitive_resolved}"
            )
    for part in resolved.parts:
        if part.lower() in SENSITIVE_DIR_NAMES:
            raise ImportRefusedError(
                f"import refused: {resolved} contains sensitive directory component {part!r}"
            )
    name = resolved.name.lower()
    if name in SENSITIVE_FILE_NAMES or name.endswith(SENSITIVE_FILE_SUFFIXES):
        raise ImportRefusedError(f"import refused: {resolved} looks like private key or credential material")


def _reject_import_entry(entry):
    """Refuse symlinks and non-regular files inside an imported tree.

    WHY symlinks are refused outright rather than followed: `copytree` with the
    default symlinks=False dereferences them, so a link inside an allowed root
    pointing at ~/.ssh would copy the credential directory into the package. A
    link that stays inside the root can also form a cycle and recurse forever.
    Refusing every link is the fail-closed answer and a web bundle has no use for
    one.
    """
    if entry.is_symlink():
        raise ImportRefusedError(f"import refused: symlink {entry} is not allowed in a web import tree")
    _reject_sensitive_path(entry)
    if not (entry.is_file() or entry.is_dir()):
        raise ImportRefusedError(f"import refused: {entry} is not a regular file or directory")


def _copy_import_tree(src_dir, dest_dir):
    """Copy a validated tree, re-checking every entry as it is reached."""
    dest_dir.mkdir(parents=True, exist_ok=True)
    for entry in sorted(src_dir.iterdir()):
        _reject_import_entry(entry)
        dest = dest_dir / entry.name
        if entry.is_dir():
            _copy_import_tree(entry, dest)
        else:
            shutil.copy2(entry, dest)


def resolve_import_folder(target, extra_roots=None):
    """Resolve a local import target to a directory inside the allowlist.

    Raises ImportRefusedError when the resolved path is outside every allowed
    root or is a sensitive host location. Symlink escapes are handled by
    resolving first and checking containment on the real path.
    """
    src = Path(target).expanduser().resolve()
    if not src.exists():
        raise FileNotFoundError(f"Input path does not exist: {target}")
    src_dir = src.parent if src.is_file() else src

    roots = allowed_import_roots(extra_roots)
    if not any(_is_within(src_dir, root) for root in roots):
        raise ImportRefusedError(
            f"import refused: {src_dir} is outside the allowed import roots "
            f"({', '.join(str(r) for r in roots)}). Designate an import root with "
            "--import-root or INTEROS_WEB_IMPORT_ROOTS if this directory is intended."
        )
    _reject_sensitive_path(src_dir)
    return src_dir


# =============================================================================
# Ingestion: Remote URL (PWA) or Local Directory
# =============================================================================
def _import_target_scheme(target):
    """Return the URL scheme of an import target, or "" for a local host path.

    WHY not a substring test for "://": schemes such as `data:`, `mailto:` and
    `file:relative` carry no authority component, so a "://" test would silently
    route them into the local-folder branch and let the caller name a host path
    of their choosing. The scheme is parsed instead, and anything that is not
    http(s) is refused by the caller.

    A single-letter scheme is a Windows drive letter (`C:/...`, `C:\\...`), not a
    URL scheme, and must stay a local path.
    """
    text = str(target)
    try:
        parsed = urllib.parse.urlsplit(text)
    except ValueError as exc:
        raise ImportRefusedError(f"import refused: unparseable target {text!r}: {exc}") from exc
    scheme = (parsed.scheme or "").lower()
    if len(scheme) == 1 and scheme.isalpha():
        return ""  # drive letter
    return scheme


def ingest_target(target, staging_dir, extra_roots=None):
    """Stage a web app into staging_dir, enforcing the host-side import policy.

    Remote targets go through the SSRF guard (scheme, resolved addresses, every
    redirect hop, and the second-order manifest fetch). Local targets go through
    the import-root allowlist, the sensitive-location denylist and the symlink
    refusal. Neither path signs anything; signing is a separate opt-in step.
    """
    scheme = _import_target_scheme(target)
    if scheme in ("http", "https"):
        log(f"Fetching remote web application / PWA from: {target}")
        content, final_url = fetch_remote_document(target)
        (staging_dir / "index.html").write_bytes(content)

        # The linked manifest is attacker-controlled content from the page we
        # just fetched, so it is a second-order fetch and gets the same guard.
        text = content.decode("utf-8", errors="replace")
        manifest_match = re.search(
            r'<link\s+[^>]*rel=["\']manifest["\'][^>]*href=["\']([^"\']+)["\']', text, re.I
        )
        if manifest_match:
            # Resolve relative to the URL the bytes actually came from, which may
            # differ from the requested URL after a redirect.
            manifest_url = urllib.parse.urljoin(final_url, manifest_match.group(1))
            try:
                log(f"Fetching PWA manifest from: {manifest_url}")
                manifest_bytes, _ = fetch_remote_document(
                    manifest_url, limit=MAX_REMOTE_MANIFEST_BYTES, timeout=10
                )
                (staging_dir / "manifest.json").write_bytes(manifest_bytes)
            except ImportRefusedError:
                # A manifest link pointing at a forbidden origin is a policy
                # refusal, not a transient network error: propagate it.
                raise
            except (urllib.error.URLError, OSError, ValueError) as exc:
                log_warn(f"Failed to fetch linked manifest: {exc}")
    elif scheme:
        raise ImportRefusedError(
            f"import refused: unsupported URL scheme {scheme!r} in {target!r} "
            "(only http, https and local directories are importable)"
        )
    else:
        src_dir = resolve_import_folder(target, extra_roots)
        log(f"Ingesting local web application directory: {src_dir}")
        _copy_import_tree(src_dir, staging_dir)


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
            # WHY the constraints are read instead of granting both: a video-only
            # `getUserMedia({video:true})` used to declare microphone too, handing
            # the ported app SystemControlAudio(21) and a Microphone(7) consent
            # prompt for a request that never asked for audio. Least privilege
            # means the declaration matches what the code actually requests.
            for call in re.finditer(r"getUserMedia\s*\(", code):
                depth, end = 1, call.end()
                while end < len(code) and depth:
                    if code[end] == "(":
                        depth += 1
                    elif code[end] == ")":
                        depth -= 1
                    end += 1
                # Unbalanced argument text means the constraints are a variable
                # this scanner cannot read. Declare both then, rather than
                # silently dropping a permission the app may genuinely need.
                args = code[call.end():end - 1] if depth == 0 else None
                if args is None:
                    declared_perms.add("camera")
                    declared_perms.add("microphone")
                else:
                    if re.search(r"\bvideo\s*:\s*(?!false\b)", args):
                        declared_perms.add("camera")
                    if re.search(r"\baudio\s*:\s*(?!false\b)", args):
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

    # Zero Ambient Authority: undeclared permissions remain strictly empty
    if not declared_perms:
        declared_perms = set()

    # Map to InterOS Capability IDs and canonical names.
    #
    # Zero Ambient Authority: an unrecognized permission string is REFUSED, not
    # silently carried through as a name and not resolved to a default grant.
    # A permission the manifest ABI cannot express (geolocation, notifications)
    # contributes no capability id; it is recorded as a consent request instead.
    canonical_permission_names = set()
    cap_ids = set()
    consent_ids = set()

    for p in sorted(declared_perms):
        ids, consent, names = map_web_permission(p)
        cap_ids.update(ids)
        consent_ids.update(consent)
        canonical_permission_names.update(names)

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
        # Consent-broker permission ids (kernel/src/capability/permission.rs:37-85).
        # These are NOT manifest capabilities: the kernel mints them only after
        # the user approves at runtime.
        "consent_permissions": sorted(list(consent_ids)),
        "architecture": "web"
    }

    # Write canonical app.json
    (staging_dir / "app.json").write_text(json.dumps(canonical_app_json, indent=2), encoding="utf-8")
    log_success(
        f"Canonical app.json generated for '{app_name}' ({app_id}) with {len(cap_ids)} capabilities "
        f"and {len(consent_ids)} consent permissions."
    )
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
def port_web(target, output=None, name=None, app_id=None, version=None, arch="web",
             key_path=None, sign=False, import_roots=None):
    """Port a web app to a .rapp package.

    sign=False (the default) produces an UNSIGNED package. Imported third-party
    content is never signed with the local developer key implicitly: signing is
    what makes a package installable under a trusted publisher, so it is an
    explicit, separate opt-in step.
    """
    if key_path and not sign:
        raise ImportRefusedError("--key requires --sign: signing is an explicit opt-in step")

    with tempfile.TemporaryDirectory(prefix="interos_port_") as tmp_dir_str:
        staging_dir = Path(tmp_dir_str)

        print("================================================================")
        print("  [InterOS Web-to-RAPP Porting Engine]")
        print(f"  Target: {target}")
        print(f"  Signing: {'ENABLED (explicit opt-in)' if sign else 'disabled (unsigned package)'}")
        print("================================================================")

        # 1. Ingest files (host-side import policy enforced here)
        ingest_target(target, staging_dir, extra_roots=import_roots)

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

        # 5. Acquire an Ed25519 signing key ONLY when signing was requested.
        #    Without --sign no key is read, created or referenced.
        priv_key = get_or_create_developer_key(key_path) if sign else None

        # 6. Build the canonical RAPP v2 package (signed only on explicit opt-in)
        arch_code = ARCH_MAP.get(arch.lower(), 3)
        rapp_bytes = encode_rapp_v2(
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

        if sign:
            log_success("Successfully generated signed .rapp package:")
        else:
            log_warn("Generated UNSIGNED .rapp package. The kernel will refuse it until it is")
            log_warn("signed by a publisher and that publisher's key is provisioned as trusted.")
            log_warn("Re-run with --sign to sign this content with your developer key deliberately.")
        print(f"      File:         {output_path}")
        print(f"      Package ID:   {manifest_data['id']}")
        print(f"      Arch Target:  {arch} ({arch_code})")
        print(f"      Capabilities: {cap_ids}")
        print(f"      Consent:      {manifest_data.get('consent_permissions', [])}")
        print(f"      Signed:       {bool(sign)}")
        print(f"      Total Size:   {len(rapp_bytes)} bytes")
        print("================================================================\n")
        return str(output_path)


def main():
    parser = argparse.ArgumentParser(
        description="Port standard web applications (HTML/JS/CSS) or PWAs to sovereign InterOS .rapp packages."
    )
    parser.add_argument("target", help="Local directory containing web files, or a remote PWA URL")
    parser.add_argument("--output", "-o", help="Output path for the .rapp file")
    parser.add_argument("--name", "-n", help="Application name override")
    parser.add_argument("--id", help="Canonical package ID override (e.g. org.interos.myapp)")
    parser.add_argument("--version", help="Application semver (e.g. 1.0.0)")
    parser.add_argument("--arch", choices=["web", "wasm", "x86_64", "aarch64", "riscv64"], default="web", help="RAPP target architecture")
    parser.add_argument("--key", help="Path to Ed25519 private key for package signing (requires --sign)")
    parser.add_argument(
        "--sign", action="store_true",
        help="Explicitly sign the imported content with the developer Ed25519 key. "
             "Off by default: imported third-party content is emitted unsigned."
    )
    parser.add_argument(
        "--import-root", action="append", default=[], dest="import_roots",
        help="Additional allowed local import root (repeatable). Local imports are "
             "otherwise confined to the workspace and the host staging temp dir."
    )

    args = parser.parse_args()

    try:
        port_web(
            target=args.target,
            output=args.output,
            name=args.name,
            app_id=args.id,
            version=args.version,
            arch=args.arch,
            key_path=args.key,
            sign=args.sign,
            import_roots=args.import_roots,
        )
    except Exception as e:
        log_error(f"Porting failed: {e}")
        import traceback
        traceback.print_exc()
        sys.exit(1)


if __name__ == "__main__":
    main()
