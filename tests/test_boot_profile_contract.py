# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

"""The boot profile is what makes a tiered measurement mean anything.

Applications are created by the launch syscall on demand, so the only part of a
device a boot-time choice can change is the set of always-on services the kernel
declares for PID 1 to start. That set has to come from the device rather than
from the build, or the tiers are three binaries and any difference between them
could be a difference between compiles.
"""

from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
SERVICES = ROOT / "kernel/src/services.rs"
MAIN = ROOT / "kernel/src/main.rs"

AI_SERVICES = {"ServiceId::Inference", "ServiceId::Search", "ServiceId::Agent",
               "ServiceId::Voice"}


def _fn_body(source: str, signature: str) -> str:
    """Return one function body, so a rule about a function cannot be met by another."""
    start = source.index(signature)
    brace = source.index("{", start)
    depth = 0
    for index in range(brace, len(source)):
        if source[index] == "{":
            depth += 1
        elif source[index] == "}":
            depth -= 1
            if depth == 0:
                return source[start:index + 1]
    raise AssertionError(f"unbalanced body for {signature}")


def _const_array(source: str, name: str) -> set:
    """The `ServiceId::X` entries of one `const NAME: [ServiceId; n] = [ ... ];`."""
    declaration = source.index(f"const {name}: [ServiceId")
    opener = source.index("[", source.index("] =", declaration))
    body = source[opener:source.index("];", opener)]
    return {"ServiceId::" + rest.split(",")[0].strip()
            for rest in body.split("ServiceId::")[1:]}


def _const_text(source: str, name: str) -> str:
    """The whole text of one `const NAME: [ServiceId ...] = ...;` item."""
    start = source.index(f"const {name}: [ServiceId")
    return source[start:source.index("\n};", start)]


def _arm_target(source: str, profile: str) -> str:
    """Which const `declared_services` hands the `profile` arm."""
    body = _fn_body(source, "pub fn declared_services(profile: BootProfile)")
    marker = f"BootProfile::{profile} => &"
    start = body.index(marker) + len(marker)
    return body[start:].split(",")[0].split("\n")[0].strip()


def _boot_profile_defects(services: str, main: str) -> list:
    defects = []

    base_name = _arm_target(services, "OsOnly")
    agents_name = _arm_target(services, "Agents")
    if base_name == agents_name:
        defects.append("both profile arms return the same service set, so choosing is moot")
    base = _const_array(services, base_name)
    if not base:
        defects.append(f"{base_name} declares no service at all")
    leaked = sorted(base & AI_SERVICES)
    if leaked:
        defects.append(f"the os profile still declares {leaked}")

    # The agents set is derived, so the gate checks the derivation names both
    # halves instead of restating a list the source deliberately does not hold.
    derived = _const_text(services, agents_name)
    if f"{base_name}[index]" not in derived or "AI_SERVICES[extra]" not in derived:
        defects.append(
            f"{agents_name} no longer copies {base_name} and AI_SERVICES, so the two profiles "
            "can drift apart by someone editing one list")
    extra = _const_array(services, "AI_SERVICES")
    if extra != {"ServiceId::Inference"}:
        defects.append(
            f"the agents profile adds {sorted(extra)} rather than the inference daemon alone")
    if "BootProfile::Agents" not in services or "BootProfile::OsOnly" not in services:
        defects.append("BootProfile lost one of its two variants")

    init = _fn_body(services, "pub fn init()")
    if "declared_services(" not in init:
        defects.append("init() registers a list of its own instead of the profile's set")
    if "ServiceId::Inference" in init:
        defects.append("init() names the inference service itself, so no profile can hide it")

    if "pub const DEFAULT_BOOT_PROFILE: BootProfile = BootProfile::Agents;" not in services:
        defects.append(
            "the default profile is no longer 'agents'; a device that cannot read its profile "
            "file would quietly lose a service it has always run")

    loader = _fn_body(services, "pub fn load_boot_profile_from_device()")
    if "BOOT_PROFILE_PATH" not in loader or "load_data_file(" not in loader:
        defects.append("the loader does not read the profile from the block filesystem")
    if 'BOOT_PROFILE_PATH: &str = "/provision/' not in services:
        defects.append(
            "the profile path left /provision/, the one namespace the VFS never creates; a "
            "VFS-shaped name is a file the kernel cannot see and a host tool cannot write")
    if "BootProfile::parse" not in loader:
        defects.append("the loader trusts the bytes it read instead of parsing a token")
    applied = [line for line in loader.splitlines() if "set_boot_profile(" in line]
    if len(applied) != 1:
        defects.append(
            f"the loader applies a profile {len(applied)} times; an unreadable token has to "
            "leave the default in place rather than guess which tier was meant")

    read_at = main.find("kernel::services::load_boot_profile_from_device();")
    declared_at = main.find("kernel::services::init();")
    if read_at < 0:
        defects.append("boot never reads the profile, so every device boots the default")
    elif declared_at < 0 or read_at > declared_at:
        defects.append(
            "boot reads the profile after registering the service set, which is too late to "
            "change what was registered")
    return defects


def test_the_boot_profile_selects_the_declared_service_set():
    assert _boot_profile_defects(SERVICES.read_text(encoding="utf-8"),
                                 MAIN.read_text(encoding="utf-8")) == []


def test_the_boot_profile_contract_can_fail():
    services = SERVICES.read_text(encoding="utf-8")
    main = MAIN.read_text(encoding="utf-8")
    assert _boot_profile_defects(services, main) == []

    # The pre-profile source: init() carried the service list itself, so a
    # profile could be read, logged and ignored.
    hardcoded = services.replace(
        "for &id in declared_services(profile) {",
        "for id in [ServiceId::Display, ServiceId::Input, ServiceId::Storage, "
        "ServiceId::Network, ServiceId::Power, ServiceId::Modem, ServiceId::Audio, "
        "ServiceId::Camera, ServiceId::Sensor, ServiceId::Compositor, ServiceId::Launcher, "
        "ServiceId::Inference] {",
    )
    assert hardcoded != services, "the pin on what init() iterates did not match the source"
    assert _boot_profile_defects(hardcoded, main), \
        "an init() that names the inference service itself passed the profile gate"

    # A profile that is not one: the inference daemon inside the base list.
    leaky = services.replace(
        "const BASE_SERVICES: [ServiceId; 11] = [",
        "const BASE_SERVICES: [ServiceId; 12] = [ServiceId::Inference,",
    )
    assert leaky != services, "the pin on BASE_SERVICES did not match the source"
    assert _boot_profile_defects(leaky, main), \
        "an os profile still declaring the inference daemon passed as AI-free"

    # Reading the profile after the registry has been built from the default.
    late = main.replace("    kernel::services::load_boot_profile_from_device();\n", "")
    late = late.replace(
        "    kernel::services::init();",
        "    kernel::services::init();\n    kernel::services::load_boot_profile_from_device();",
    )
    assert late != main, "the pin on the boot call order did not match the source"
    assert _boot_profile_defects(services, late), \
        "a profile applied after the service set was registered passed as a boot-time choice"

    # A loader that guesses a tier out of bytes it could not name.
    guessing = services.replace(
        "        None => crate::serial_println!(",
        "        None => {\n            set_boot_profile(BootProfile::OsOnly)\n        }\n"
        "        _Named => crate::serial_println!(",
    )
    assert guessing != services, "the pin on the loader's unknown-token arm did not match"
    assert _boot_profile_defects(guessing, main), \
        "a loader that downgrades on an unrecognised token passed the fail-loudly gate"


if __name__ == "__main__":
    test_the_boot_profile_selects_the_declared_service_set()
    test_the_boot_profile_contract_can_fail()
    print("boot profile contract: ok")
