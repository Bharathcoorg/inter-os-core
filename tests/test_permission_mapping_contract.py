#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

"""Pins the three web-permission mappers against the kernel's own ABI.

WHY this parses the kernel instead of hardcoding a copy of the table: the bug
this exists to catch was three hand-written copies drifting apart. `camera`
resolved to manifest id 4 in the Go and TypeScript SDKs, and id 4 in the kernel
raw type space is `HardwarePortIo` (kernel/src/capability/mod.rs,
CapabilityType::to_raw) -- unrestricted x86 port I/O. `microphone` resolved to 7
(`ProcessSpawn`) and `geolocation` to 8 (`ProcessKill`). The minter in
kernel/src/task/user.rs used to pass those ids straight through, so the comments
claiming CameraCapture/Microphone/Location were describing a *different* id
space: the consent broker's `SensitivePermission`
(kernel/src/capability/permission.rs:45,51,53). A fourth copy in
tools/port_web.py had camera right (22) but microphone at 23 (the sensor
object), geolocation at 24 (RealtimeClockRead) and sensor at 25
(ContentIndexRead).

The kernel minter has since been hardened to refuse manifest 4/7/8 without
system platform trust and to reshape them through `sensitive_manifest_shape`.
That is defence in depth at the kernel boundary; it does not make 4/7/8 the
right declaration for a media prompt, so the mappers must still never emit them.
test_01 pins both halves: the raw-space collision and the hardened minter arm.

The authoritative sources read here are:
  * kernel/src/task/user.rs           -- capabilities_from_rapp_with_trust, the
                                         minter that turns a header id into a
                                         capability token. This IS the manifest ABI.
  * kernel/src/capability/mod.rs      -- CapabilityType::to_raw / from_raw.
  * kernel/src/package/dependency.rs  -- PackagePolicy::from_rapp_capabilities,
                                         the consent policy derived from ids.
  * kernel/src/capability/permission.rs -- SensitivePermission, the consent id
                                         space, and the objects it mints.

The mappers under test:
  * sdk/go/rapp/builder.go        (webPermissionGrants)
  * sdk/typescript/src/builder.ts (WEB_PERMISSION_GRANTS)
  * tools/port_web.py             (WEB_PERMISSION_MAP)

Go is checked statically because a Go toolchain is not present on every host;
the parse resolves the SDK's own named constants, so a wrong constant value is
still caught. TypeScript is additionally executed under Node when Node can run
.ts directly, and Python is executed by import.
"""

import json
import os
import re
import shutil
import subprocess
import sys
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "tools"))

import port_web  # noqa: E402

KERNEL_MINTER = ROOT / "kernel" / "src" / "task" / "user.rs"
KERNEL_CAP_TABLE = ROOT / "kernel" / "src" / "capability" / "mod.rs"
KERNEL_POLICY = ROOT / "kernel" / "src" / "package" / "dependency.rs"
KERNEL_CONSENT = ROOT / "kernel" / "src" / "capability" / "permission.rs"
GO_BUILDER = ROOT / "sdk" / "go" / "rapp" / "builder.go"
GO_TYPES = ROOT / "sdk" / "go" / "rapp" / "types.go"
TS_BUILDER = ROOT / "sdk" / "typescript" / "src" / "builder.ts"

# Manifest ids that are raw hardware bus access or process control. No web
# permission prompt may ever resolve to one of these: they are not camera,
# microphone or location authority in the manifest id space.
FORBIDDEN_MANIFEST_IDS = frozenset({4, 7, 8})


# ---------------------------------------------------------------------------
# Kernel-side parsers
# ---------------------------------------------------------------------------
def kernel_minter_table():
    """id -> (CapabilityType variant, rights expression, object expression)."""
    text = KERNEL_MINTER.read_text(encoding="utf-8")
    start = text.index("fn capabilities_from_rapp_with_trust")
    end = text.index('_ => return Err("Unknown RAPP v2 capability")', start)
    table = {}
    for m in re.finditer(
        r"^\s*(\d+)\s*=>\s*\(\s*CapabilityType::(\w+)\s*,\s*([^,]+?)\s*,\s*(.+?)\s*\)\s*,\s*$",
        text[start:end],
        re.M,
    ):
        table[int(m.group(1))] = (m.group(2), m.group(3).strip(), m.group(4).strip())
    return table


def kernel_minter_body():
    """Raw source of capabilities_from_rapp_with_trust, up to the reject arm."""
    text = KERNEL_MINTER.read_text(encoding="utf-8")
    start = text.index("fn capabilities_from_rapp_with_trust")
    end = text.index('_ => return Err("Unknown RAPP v2 capability")', start)
    return text[start:end]


def kernel_sensitive_shape_table():
    """manifest id -> SensitivePermission variant, from sensitive_manifest_shape().

    The kernel converts the colliding manifest ids 4/7/8 through the consent
    space rather than minting the unscoped raw type. This parses that conversion
    so the contract pins the kernel's actual behaviour, not an assumed one.
    """
    text = KERNEL_MINTER.read_text(encoding="utf-8")
    start = text.index("fn sensitive_manifest_shape")
    end = text.index("permission.to_capability", start)
    return {
        int(m.group(1)): m.group(2)
        for m in re.finditer(r"^\s*(\d+)\s*=>\s*SensitivePermission::(\w+)\s*,", text[start:end], re.M)
    }


def kernel_raw_table():
    """CapabilityType variant -> raw id, from to_raw()."""
    text = KERNEL_CAP_TABLE.read_text(encoding="utf-8")
    start = text.index("pub const fn to_raw(self) -> u64")
    end = text.index("pub const fn from_raw", start)
    return {
        m.group(1): int(m.group(2))
        for m in re.finditer(r"CapabilityType::(\w+)\s*=>\s*(\d+)\s*,", text[start:end])
    }


def kernel_policy_table():
    """manifest id -> consent Permission variant, from from_rapp_capabilities()."""
    text = KERNEL_POLICY.read_text(encoding="utf-8")
    start = text.index("pub fn from_rapp_capabilities")
    end = text.index("if let Some(p) = permission", start)
    table = {}
    for m in re.finditer(r"([\d\s|]+)=>\s*Some\(Permission::(\w+)\)", text[start:end]):
        for token in m.group(1).split("|"):
            token = token.strip()
            if token.isdigit():
                table[int(token)] = m.group(2)
    return table


def kernel_consent_table():
    """SensitivePermission variant -> consent id."""
    text = KERNEL_CONSENT.read_text(encoding="utf-8")
    start = text.index("pub enum SensitivePermission")
    end = text.index("impl SensitivePermission", start)
    return {
        m.group(1): int(m.group(2))
        for m in re.finditer(r"^\s*(\w+)\s*=\s*(\d+)\s*,", text[start:end], re.M)
    }


# ---------------------------------------------------------------------------
# SDK-side parsers
# ---------------------------------------------------------------------------
def _brace_block(text, marker):
    """Returns the inside of the first balanced {...} after marker."""
    idx = text.index(marker)
    start = text.index("{", idx)
    depth = 0
    for j in range(start, len(text)):
        if text[j] == "{":
            depth += 1
        elif text[j] == "}":
            depth -= 1
            if depth == 0:
                return text[start + 1:j]
    raise AssertionError(f"unterminated block after {marker!r}")


def _entries(block):
    """key -> body for each `key: { ... }` entry, honouring nested braces."""
    out = {}
    i = 0
    key_re = re.compile(r'(?:"([^"]+)"|([A-Za-z0-9_.$-]+))\s*:\s*\{')
    while True:
        m = key_re.search(block, i)
        if not m:
            return out
        key = m.group(1) if m.group(1) is not None else m.group(2)
        depth = 1
        j = m.end()
        while j < len(block) and depth:
            if block[j] == "{":
                depth += 1
            elif block[j] == "}":
                depth -= 1
            j += 1
        out[key] = block[m.end():j - 1]
        i = j


def _tokens(body, list_re):
    m = re.search(list_re, body)
    if not m:
        return []
    return [t.strip() for t in m.group(1).split(",") if t.strip()]


def go_constants(kind="CapabilityType"):
    """Named Go constants -> value, for ONE id space.

    WHY kind is a parameter: the two spaces reuse the same numbers (manifest 19
    is SystemControlPower, consent 19 is AgentToolSettings), so a merged
    value->name map would silently resolve to the wrong space.
    """
    text = GO_TYPES.read_text(encoding="utf-8")
    consts = {}
    for m in re.finditer(
        r"^\s*(Cap\w+|Perm\w+)\s+(CapabilityType|SensitivePermission)\s*=\s*(\d+)\s*$",
        text,
        re.M,
    ):
        if m.group(2) == kind:
            consts[m.group(1)] = int(m.group(3))
    return consts


def go_grants():
    manifest_consts = go_constants("CapabilityType")
    consent_consts = go_constants("SensitivePermission")
    block = _brace_block(GO_BUILDER.read_text(encoding="utf-8"), "var webPermissionGrants")
    grants = {}
    for key, body in _entries(block).items():
        manifest = []
        for tok in _tokens(body, r"manifest:\s*\[\]CapabilityType\{([^}]*)\}"):
            manifest.append(manifest_consts[tok] if tok in manifest_consts else int(tok))
        consent = []
        for tok in _tokens(body, r"consent:\s*\[\]SensitivePermission\{([^}]*)\}"):
            consent.append(consent_consts[tok] if tok in consent_consts else int(tok))
        grants[key] = (sorted(manifest), sorted(consent))
    return grants


def ts_constants():
    text = TS_BUILDER.read_text(encoding="utf-8")
    manifest = {
        m.group(1): int(m.group(2))
        for m in re.finditer(
            r"^\s*(\w+):\s*(\d+),", _brace_block(text, "export const ManifestCapability"), re.M
        )
    }
    consent = {
        m.group(1): int(m.group(2))
        for m in re.finditer(
            r"^\s*(\w+):\s*(\d+),", _brace_block(text, "export const SensitivePermission"), re.M
        )
    }
    return manifest, consent


def ts_grants():
    manifest_names, consent_names = ts_constants()
    block = _brace_block(TS_BUILDER.read_text(encoding="utf-8"), "const WEB_PERMISSION_GRANTS")
    grants = {}
    for key, body in _entries(block).items():
        manifest = []
        for tok in _tokens(body, r"manifest:\s*\[([^\]]*)\]"):
            name = tok.split(".", 1)[1]
            manifest.append(manifest_names[name])
        consent = []
        for tok in _tokens(body, r"consent:\s*\[([^\]]*)\]"):
            consent.append(consent_names[tok.split(".", 1)[1]])
        grants[key] = (sorted(manifest), sorted(consent))
    return grants


def python_grants():
    return {
        key: (sorted(val["ids"]), sorted(val["consent"]))
        for key, val in port_web.WEB_PERMISSION_MAP.items()
    }


def node_typescript_grants():
    """Executes the real TypeScript mapper under Node and returns its table.

    Returns None when Node cannot run .ts directly, so the static parse remains
    the checked path rather than the suite silently reporting a pass.
    """
    node = shutil.which("node")
    if not node:
        return None
    script = (
        "const m = await import(%s);"
        "const keys = %s;"
        "const out = {};"
        "for (const k of keys) {"
        "  const g = m.resolveWebPermission(k);"
        "  out[k] = [g.capabilities.map(Number).sort((a,b)=>a-b), g.consent.map(Number).sort((a,b)=>a-b)];"
        "}"
        "process.stdout.write(JSON.stringify(out));"
    ) % (
        json.dumps((TS_BUILDER).as_uri()),
        json.dumps(sorted(ts_grants())),
    )
    try:
        res = subprocess.run(
            [node, "--input-type=module", "-e", script],
            capture_output=True, text=True, timeout=120,
        )
    except (OSError, subprocess.TimeoutExpired):
        return None
    if res.returncode != 0:
        return None
    try:
        return {k: (sorted(v[0]), sorted(v[1])) for k, v in json.loads(res.stdout).items()}
    except (ValueError, TypeError, IndexError):
        return None


class TestPermissionMappingContract(unittest.TestCase):
    """C2: one authoritative web-permission mapping, fail-closed on unknown."""

    @classmethod
    def setUpClass(cls):
        cls.minter = kernel_minter_table()
        cls.raw = kernel_raw_table()
        cls.policy = kernel_policy_table()
        cls.consent = kernel_consent_table()
        cls.go = go_grants()
        cls.ts = ts_grants()
        cls.py = python_grants()

    # -- the kernel table is what we think it is -----------------------------
    def test_01_manifest_ids_4_7_8_collide_with_consent_and_are_not_unscoped(self):
        """4/7/8 are raw port I/O and process control in the kernel type space.

        The same numbers are CameraCapture/Microphone/Location in the consent
        space, which is why the old mappers looked plausible. Two things must
        hold: the collision is real (so the SDKs must not emit these ids for a
        media prompt), and the manifest minter never hands a package the
        unscoped shape for them.
        """
        self.assertEqual(self.raw["HardwarePortIo"], 4)
        self.assertEqual(self.raw["ProcessSpawn"], 7)
        self.assertEqual(self.raw["ProcessKill"], 8)
        # And the same numbers mean something else entirely in the consent space.
        self.assertEqual(self.consent["CameraCapture"], 4)
        self.assertEqual(self.consent["Microphone"], 7)
        self.assertEqual(self.consent["Location"], 8)

        body = kernel_minter_body()
        for cid, variant in ((4, "HardwarePortIo"), (7, "ProcessSpawn"), (8, "ProcessKill")):
            self.assertNotRegex(
                body,
                rf"\b{cid}\s*=>\s*\(\s*CapabilityType::{variant}\b",
                f"the minter maps manifest id {cid} straight through to unscoped {variant}",
            )

        # The colliding ids must be gated on system platform trust and reshaped
        # through the consent permission space, never granted ambiently.
        arm = re.search(r"\b4\s*\|\s*7\s*\|\s*8\s*=>\s*\{", body)
        self.assertIsNotNone(arm, "the minter must handle manifest ids 4/7/8 explicitly")
        arm_body = _brace_block(body, arm.group(0)[:-1])
        self.assertIn("is_system_authority", arm_body)
        self.assertIn("return Err(", arm_body)
        self.assertIn("sensitive_manifest_shape(id)", arm_body)

        shape = kernel_sensitive_shape_table()
        self.assertEqual(shape, {4: "CameraCapture", 7: "Microphone", 8: "Location"})

    def test_02_camera_resolves_to_the_camera_service_object(self):
        for name, table in (("go", self.go), ("typescript", self.ts), ("python", self.py)):
            for key in ("camera", "cameracapture"):
                ids, _ = table[key]
                self.assertEqual(ids, [22], f"{name}: {key} must resolve to manifest id 22")
                self.assertNotIn(4, ids, f"{name}: {key} must NEVER yield HardwarePortIo (4)")
        self.assertEqual(self.minter[22][0], "SystemControl")
        self.assertIn("SERVICE_CAMERA_CONTROL_OBJECT", self.minter[22][2])
        self.assertEqual(self.policy[22], "Camera")

    def test_03_microphone_resolves_to_the_audio_service_object(self):
        for name, table in (("go", self.go), ("typescript", self.ts), ("python", self.py)):
            for key in ("microphone", "mic"):
                ids, _ = table[key]
                self.assertEqual(ids, [21], f"{name}: {key} must resolve to manifest id 21")
                self.assertNotIn(7, ids, f"{name}: {key} must NEVER yield ProcessSpawn (7)")
        self.assertEqual(self.minter[21][0], "SystemControl")
        self.assertIn("SERVICE_AUDIO_CONTROL_OBJECT", self.minter[21][2])
        self.assertEqual(self.policy[21], "Microphone")

    def test_04_geolocation_never_yields_process_kill_and_has_no_manifest_id(self):
        """The ladder defines no GNSS object, so location is consent-only."""
        for name, table in (("go", self.go), ("typescript", self.ts), ("python", self.py)):
            for key in ("geolocation", "location"):
                ids, consent = table[key]
                self.assertNotIn(8, ids, f"{name}: {key} must NEVER yield ProcessKill (8)")
                self.assertNotIn(24, ids, f"{name}: {key} must not yield RealtimeClockRead (24)")
                self.assertEqual(ids, [], f"{name}: {key} has no manifest id in the RAPP v2 ladder")
                self.assertEqual(consent, [self.consent["Location"]])
        self.assertNotIn("Location", self.policy.values())
        self.assertEqual(self.consent["Location"], 8)
        # The consent broker mints location as a scoped camera-family object.
        consent_src = KERNEL_CONSENT.read_text(encoding="utf-8")
        self.assertIn("0xCA02", consent_src)

    def test_05_sensor_resolves_to_the_sensor_service_object(self):
        for name, table in (("go", self.go), ("typescript", self.ts), ("python", self.py)):
            for key in ("sensor", "sensors", "accelerometer", "gyroscope"):
                ids, _ = table[key]
                self.assertEqual(ids, [23], f"{name}: {key} must resolve to manifest id 23")
                self.assertNotIn(12, ids, f"{name}: {key} must not yield NetworkSocket (12)")
                self.assertNotIn(25, ids, f"{name}: {key} must not yield ContentIndexRead (25)")
        self.assertEqual(self.minter[23][0], "SystemControl")
        self.assertIn("SERVICE_SENSOR_CONTROL_OBJECT", self.minter[23][2])
        self.assertEqual(self.policy[23], "Sensors")

    def test_06_notifications_are_consent_only_not_event_poll_or_network_receive(self):
        for name, table in (("go", self.go), ("typescript", self.ts), ("python", self.py)):
            for key in ("notifications", "notification"):
                ids, consent = table[key]
                self.assertEqual(ids, [], f"{name}: {key} has no manifest id")
                self.assertNotIn(14, ids, f"{name}: {key} must not yield NetworkReceive (14)")
                self.assertNotIn(17, ids, f"{name}: {key} must not yield EventPoll (17)")
                self.assertEqual(consent, [self.consent["Notifications"]])
        self.assertNotIn("Notifications", self.policy.values())

    # -- the three mappers agree --------------------------------------------
    def test_07_all_three_mappers_have_identical_key_sets(self):
        self.assertTrue(self.go, "Go grant table parsed empty -- the parse stopped testing anything")
        self.assertTrue(self.ts, "TypeScript grant table parsed empty")
        self.assertTrue(self.py, "Python grant table parsed empty")
        self.assertEqual(set(self.go), set(self.py), "Go and Python permission keys differ")
        self.assertEqual(set(self.ts), set(self.py), "TypeScript and Python permission keys differ")

    def test_08_all_three_mappers_resolve_every_permission_identically(self):
        for key in sorted(self.py):
            expected = self.py[key]
            self.assertEqual(self.go[key], expected, f"Go mapper disagrees on {key!r}")
            self.assertEqual(self.ts[key], expected, f"TypeScript mapper disagrees on {key!r}")

    def test_09_typescript_mapper_executes_to_the_same_table(self):
        """Runs the real TS code, not just its source text, when Node allows it."""
        executed = node_typescript_grants()
        if executed is None:
            self.skipTest("Node cannot execute .ts directly here; static parse is the checked path")
        self.assertEqual(executed, self.py, "executed TypeScript mapper disagrees with Python")

    def test_10_no_web_permission_ever_grants_port_io_or_process_control(self):
        for name, table in (("go", self.go), ("typescript", self.ts), ("python", self.py)):
            for key, (ids, _) in table.items():
                self.assertFalse(
                    FORBIDDEN_MANIFEST_IDS.intersection(ids),
                    f"{name}: permission {key!r} grants forbidden manifest ids "
                    f"{sorted(FORBIDDEN_MANIFEST_IDS.intersection(ids))}",
                )

    def test_11_every_emitted_id_is_inside_the_kernel_manifest_ladder(self):
        for name, table in (("go", self.go), ("typescript", self.ts), ("python", self.py)):
            for key, (ids, _) in table.items():
                for cid in ids:
                    self.assertIn(cid, self.minter, f"{name}: {key!r} emits id {cid} the kernel cannot mint")
                    self.assertTrue(1 <= cid <= 30, f"{name}: {key!r} emits out-of-ladder id {cid}")

    def test_12_consent_ids_are_real_sensitive_permissions(self):
        valid = set(self.consent.values())
        for name, table in (("go", self.go), ("typescript", self.ts), ("python", self.py)):
            for key, (_, consent) in table.items():
                for cid in consent:
                    self.assertIn(cid, valid, f"{name}: {key!r} emits unknown consent id {cid}")

    # -- fail closed ---------------------------------------------------------
    def test_13_python_unknown_permission_is_refused(self):
        for unknown in ("", "   ", "totally-unknown", "admin", "root", "shell", "debugger",
                        "camera;rm -rf /", "CAMERA_EXTRA", "network-all", "0", "31", "999"):
            with self.assertRaises(port_web.ImportRefusedError, msg=f"unknown {unknown!r} was not refused"):
                port_web.map_web_permission(unknown)

    def test_14_python_unknown_permission_grants_nothing_not_network(self):
        """The old default handed out NetworkSocket/Send/Receive for any string."""
        try:
            ids, consent, _ = port_web.map_web_permission("not-a-permission")
        except port_web.ImportRefusedError:
            return
        self.fail(f"unknown permission returned a grant instead of failing closed: {ids} {consent}")

    def test_15_go_mapper_fails_closed_and_has_no_network_default(self):
        src = GO_BUILDER.read_text(encoding="utf-8")
        self.assertIn("func MapWebPermission(perm string) ([]CapabilityType, error)", src)
        self.assertIn("ErrUnknownWebPermission", src)
        body = src[src.index("func resolveWebPermission"):]
        body = body[:body.index("\n}\n")]
        self.assertIn("if !ok", body, "Go resolver must branch on a table miss")
        self.assertIn("ErrUnknownWebPermission", body, "a table miss must return the unknown error")
        self.assertNotIn("CapNetworkSocket", body, "the resolver must not fall back to network")
        # The old fail-open switch default must be gone from the whole file.
        self.assertNotRegex(src, r"default:\s*\n\s*return \[\]CapabilityType\{CapNetworkSocket")

    def test_16_typescript_mapper_throws_and_has_no_network_default(self):
        src = TS_BUILDER.read_text(encoding="utf-8")
        self.assertIn("export function resolveWebPermission", src)
        start = src.index("export function resolveWebPermission")
        body = src[start:src.index("\n}\n", start)]
        self.assertIn("if (!grant)", body)
        self.assertIn("throw new Error", body)
        self.assertNotIn("NetworkSocket", body, "the resolver must not fall back to network")
        self.assertNotIn("default:", src[src.index("const WEB_PERMISSION_GRANTS"):src.index("export class RappBuilder")])
        node = shutil.which("node")
        if not node:
            return
        script = (
            "const m = await import(%s);"
            "let threw = false;"
            "try { m.mapWebPermissionToCapabilities('not-a-permission'); }"
            "catch (e) { threw = true; }"
            "process.stdout.write(threw ? 'THREW' : 'DID-NOT-THROW');"
        ) % json.dumps(TS_BUILDER.as_uri())
        res = subprocess.run([node, "--input-type=module", "-e", script],
                             capture_output=True, text=True, timeout=120)
        if res.returncode != 0:
            self.skipTest("Node cannot execute .ts directly here; static check is the checked path")
        self.assertEqual(res.stdout.strip(), "THREW", "TS mapper did not throw for an unknown permission")

    def test_17_python_manifest_encoding_rejects_out_of_ladder_ids(self):
        for bad in (0, 31, 39, 999):
            with self.assertRaises(port_web.ImportRefusedError):
                port_web.encode_rapp_v2("org.interos.contract", "1.0.0", 3, [bad], b"payload")

    # -- the SDK tables can express the correct grants -----------------------
    def test_18_go_capability_table_extends_past_18_and_matches_the_kernel(self):
        consts = go_constants("CapabilityType")
        by_value = {v: k for k, v in consts.items()}
        for cid in range(19, 31):
            self.assertIn(cid, by_value, f"Go SDK cannot express manifest id {cid}")
        # 19..23 are all SystemControl, distinguished by their service object.
        for cid, obj in ((19, "POWER"), (20, "TELEPHONY"), (21, "AUDIO"), (22, "CAMERA"), (23, "SENSOR")):
            self.assertEqual(self.minter[cid][0], "SystemControl")
            self.assertIn(f"SERVICE_{obj}_CONTROL_OBJECT", self.minter[cid][2])
            self.assertTrue(by_value[cid].startswith("CapSystemControl"), by_value[cid])
        # 24..30 carry the kernel's own variant names.
        for cid in range(24, 31):
            variant = self.minter[cid][0]
            self.assertEqual(self.raw[variant], cid)
            self.assertEqual(by_value[cid], "Cap" + variant, f"Go name for id {cid} does not match the kernel")
        self.assertEqual(consts["CapHardwarePortIo"], 4)
        self.assertEqual(consts["CapProcessSpawn"], 7)
        self.assertEqual(consts["CapProcessKill"], 8)

    def test_19_go_consent_table_matches_the_kernel_permission_enum(self):
        consts = go_constants("SensitivePermission")
        for variant, cid in self.consent.items():
            name = "Perm" + variant
            self.assertIn(name, consts, f"Go SDK is missing consent permission {variant}")
            self.assertEqual(consts[name], cid, f"Go consent id for {variant} differs from the kernel")

    def test_20_typescript_capability_table_extends_past_18_and_matches_the_kernel(self):
        manifest, consent = ts_constants()
        by_value = {v: k for k, v in manifest.items()}
        for cid in range(1, 31):
            self.assertIn(cid, by_value, f"TypeScript SDK cannot express manifest id {cid}")
        for cid in range(24, 31):
            self.assertEqual(by_value[cid], self.minter[cid][0], f"TS name for id {cid} differs")
        for cid, obj in ((19, "Power"), (20, "Telephony"), (21, "Audio"), (22, "Camera"), (23, "Sensor")):
            self.assertEqual(by_value[cid], "SystemControl" + obj)
        self.assertEqual(manifest["HardwarePortIo"], 4)
        self.assertEqual(manifest["ProcessSpawn"], 7)
        self.assertEqual(manifest["ProcessKill"], 8)
        for variant, cid in self.consent.items():
            self.assertIn(variant, consent, f"TS SDK is missing consent permission {variant}")
            self.assertEqual(consent[variant], cid)

    def test_21_sdk_ladder_bounds_match_package_verification(self):
        rapp = (ROOT / "kernel" / "src" / "package" / "rapp.rs").read_text(encoding="utf-8")
        self.assertIn("if !(1..=30).contains(&cap)", rapp)
        self.assertIn("ManifestCapabilityMax CapabilityType = 30", GO_TYPES.read_text(encoding="utf-8"))
        ts = TS_BUILDER.read_text(encoding="utf-8")
        self.assertIn("export const MANIFEST_CAPABILITY_MAX = 30", ts)
        self.assertEqual(port_web.MANIFEST_CAPABILITY_MAX, 30)
        self.assertTrue(port_web.is_manifest_capability(30))
        self.assertFalse(port_web.is_manifest_capability(31))
        self.assertFalse(port_web.is_manifest_capability(0))

    def test_22_python_consent_names_match_the_kernel_enum(self):
        for variant, cid in self.consent.items():
            self.assertEqual(
                port_web.SENSITIVE_PERMISSION_NAMES.get(cid), variant,
                f"port_web consent name for id {cid} does not match SensitivePermission::{variant}",
            )

    def test_23_python_manifest_names_match_the_kernel_variants(self):
        for cid, (variant, _rights, _obj) in self.minter.items():
            expected = port_web.RAPP_MANIFEST_CAPABILITY_NAMES.get(cid)
            self.assertIsNotNone(expected, f"port_web has no name for manifest id {cid}")
            if cid in (19, 20, 21, 22, 23):
                # Domain-scoped SystemControl: the name must say which domain.
                self.assertTrue(expected.startswith("SystemControl"), expected)
                self.assertEqual(variant, "SystemControl")
            else:
                self.assertEqual(expected, variant, f"port_web name for id {cid} differs from the kernel")


if __name__ == "__main__":
    unittest.main()