# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

"""Pins the tooling's capability aliases against the documented RAPP v2 ABI.

WHY this parses `docs/RAPP_FORMAT.md` instead of hardcoding a copy: an alias
table and an ABI table drift apart silently, and the only thing that stops a
template from acquiring authority nobody intended to hand it is that the
documented table is the thing being checked.

The failure this exists to catch is concrete. `tools/interos.py` previously
mapped `CAP_HARDWARE_KEYSTORE` to id 5 and `CAP_SENSOR_ACCEL` to id 4. Id 5 is
`IpcSend` scoped to an IPC endpoint and id 4 is `HardwarePortIo` with raw bus
read/write, so a TOTP template silently acquired IPC send authority and a
sensor-telemetry template silently acquired raw port I/O. Both names were
plausible; neither identifier delivered the capability.
"""

import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "tools"))

import interos  # noqa: E402


def documented_capabilities():
    """Parses the RAPP v2 capability table out of the format documentation.

    WHY this splits on cells instead of matching one big regular expression:
    several rows carry an escaped pipe in the rights column (`READ \\| WRITE`),
    which a naive `[^|]+` matcher silently drops, and dropping rows would make
    this test pass by omission.
    """
    text = (ROOT / "docs" / "RAPP_FORMAT.md").read_text(encoding="utf-8")
    table = {}
    for raw in text.splitlines():
        line = raw.strip()
        if not line.startswith("| `"):
            continue
        # Protect escaped pipes so they do not act as cell separators.
        cells = [c.strip() for c in line.replace("\\|", "\x00").split("|")]
        if len(cells) < 6:
            continue
        cid_match = re.fullmatch(r"`(\d+)`", cells[1])
        type_match = re.fullmatch(r"`([A-Za-z]+)`", cells[2])
        if not cid_match or not type_match:
            continue
        table[int(cid_match.group(1))] = {
            "type": type_match.group(1),
            "rights": cells[3].replace("\x00", "|"),
            "object_scope": cells[4].replace("\x00", "|"),
            "description": cells[5].replace("\x00", "|"),
        }
    return table


def must(condition, message):
    if not condition:
        print(f"FAIL: {message}")
        sys.exit(1)


def main():
    table = documented_capabilities()
    must(len(table) >= 24, f"expected the full RAPP v2 capability table, parsed only {sorted(table)}")

    ids = interos.RAPP_CAPABILITY_IDS

    # 1. Every identifier the tooling can emit must exist in the documented ABI.
    for name, cid in sorted(ids.items()):
        must(
            cid in table,
            f"{name} maps to capability id {cid}, which docs/RAPP_FORMAT.md does not define",
        )

    # 2. Every template must resolve, and must only use names the tooling knows.
    for template, spec in sorted(interos.TEMPLATES.items()):
        for perm in spec["permissions"]:
            must(
                perm in ids,
                f"template {template!r} requests unknown permission {perm!r}",
            )

    # 3. A template-facing alias must never resolve to authority unrelated to the
    #    template's purpose. This is the check that fails for CAP_HARDWARE_IO,
    #    CAP_IPC_*, CAP_PROCESS_*, CAP_STORAGE_RAW and CAP_SYSTEM_CONTROL.
    forbidden = interos.ALIAS_FORBIDDEN_CAPABILITY_IDS
    template_permissions = {
        perm for spec in interos.TEMPLATES.values() for perm in spec["permissions"]
    }
    for name in sorted(template_permissions):
        cid = ids[name]
        must(
            cid not in forbidden,
            f"template permission {name} resolves to capability id {cid} "
            f"({table[cid]['type']}, scope {table[cid]['object_scope']}), which is "
            f"authority unrelated to what the template does",
        )

    # 4. RAPP v2 defines no keystore or enclave capability, so the tooling must
    #    not offer one, and a template may mention those words only in order to
    #    deny having them.
    must(
        not any("KEYSTORE" in name or "ENCLAVE" in name for name in ids),
        "RAPP v2 defines no keystore or enclave capability, so no alias may claim one",
    )
    claims = ("enclave", "keystore", "ring 0", "secure element")
    negations = ("no ", "not ", "none", "never", "without", "cannot", "defines no")
    for template, spec in sorted(interos.TEMPLATES.items()):
        for sentence in re.split(r"[.;]", f"{spec['title']}. {spec['description']}"):
            low = sentence.lower()
            if not any(claim in low for claim in claims):
                continue
            must(
                any(neg in low for neg in negations),
                f"template {template!r} claims secure-hardware support in "
                f"{sentence.strip()!r}, which RAPP v2 does not provide",
            )

    # 5. Sensor access is gated on the sensor domain of SystemControl, so the
    #    sensor aliases must resolve to that domain rather than to a capability
    #    that happens to be numerically adjacent.
    must(
        ids["CAP_SENSOR_ACCEL"] == 23
        and table[ids["CAP_SENSOR_ACCEL"]]["type"] == "SystemControl"
        and "SENSOR_CONTROL_OBJECT" in table[ids["CAP_SENSOR_ACCEL"]]["object_scope"],
        "sensor aliases must resolve to capability 23, the sensor domain of SystemControl",
    )

    # 6. Device-wide administration is reserved for the system supervisor and
    #    must not be reachable from any template.
    must(
        all(ids[p] != 18 for p in template_permissions),
        "no template may request capability 18, which is reserved for the system supervisor",
    )

    # 7. A package's minimum SDK version is a promise about the device it can
    #    run on, so it may not name a version the platform never published.
    sdk = interos.sdk_version()
    must(
        re.fullmatch(r"\d+\.\d+\.\d+", sdk) is not None,
        f"the SDK crate's version is {sdk!r}, which is not a version a manifest can require",
    )
    manifests = sorted((ROOT / "apps").glob("*/manifest.json"))
    must(manifests, "no manifests found -- the scan has stopped testing anything")
    for path in manifests:
        declared = re.search(r'"min_sdk_version":\s*"([^"]+)"', path.read_text(encoding="utf-8"))
        if declared is None:
            continue
        must(
            declared.group(1) == sdk,
            f"{path.name} requires SDK {declared.group(1)!r} but the platform's SDK is {sdk!r}",
        )
    must(
        f'min_sdk_version": "0.8.0' not in (ROOT / "tools" / "interos.py").read_text(encoding="utf-8"),
        "the scaffold still stamps an SDK version literal instead of the published one",
    )

    print(f"RAPP capability ABI contract: PASS ({len(table)} documented capabilities, {len(template_permissions)} template permissions)")


if __name__ == "__main__":
    main()