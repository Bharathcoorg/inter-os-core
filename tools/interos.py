#!/usr/bin/env python3
"""
InterOS Core Developer CLI (interos)
=====================================
Unified CLI tool for building, packaging, verifying, testing, and hot-deploying
InterOS Core kernels and sovereign Rust Mobile Application Packages (.rapp).

Usage:
  python tools/interos.py doctor
  python tools/interos.py build [--target x86_64|aarch64|riscv64|all|<app_name>]
  python tools/interos.py test [--target unit|kernel|x86_64|aarch64|riscv64|security|adversarial|qemu|lifecycle|contracts|stress|recovery|persistence|storage_recovery|crash|watchdog|panic|all]
  python tools/interos.py new <app_name> [--template basic|authenticator|sensor|meshchat]
  python tools/interos.py package <app_name> [--output dist/<app_name>.rapp]
  python tools/interos.py sign <package_or_file> [--key <key_path>]
  python tools/interos.py verify <package.rapp>
  python tools/interos.py install <package_path> [--port 4300]
  python tools/interos.py trust <package.rapp> [--publisher developer] [--port 4300]
  python tools/interos.py run <app_name> [--target x86_64-unknown-none] [--port 4300]
  python tools/interos.py stop <app_name> [--port 4300]
  python tools/interos.py restart <app_name> [--port 4300]
  python tools/interos.py logs [--port 4300] [--serial]
  python tools/interos.py devices
  python tools/interos.py snapshot [--name <name>] [--port 4300]
  python tools/interos.py inspect <package_or_target>
  python tools/interos.py list [--port 4300]
  python tools/interos.py uninstall <app_name> [--port 4300]
"""

import argparse
import hashlib
import json
import os
import platform
import shutil
import subprocess
import socket
import time
import sys
import urllib.error
import urllib.parse
import urllib.request
import zipfile
from pathlib import Path

WORKSPACE_ROOT = Path(__file__).resolve().parent.parent
APPS_DIR = WORKSPACE_ROOT / "apps"
DIST_DIR = WORKSPACE_ROOT / "dist"
PACKAGES_DIR = WORKSPACE_ROOT / "packages"
TARGET_DIR = WORKSPACE_ROOT / "target"

RAPP_CAPABILITY_IDS = {
    "CAP_LOGGING": 1, "CAP_FRAMEBUFFER": 2, "CAP_TIMER": 3, "CAP_HARDWARE_IO": 4,
    "CAP_IPC_SEND": 5, "CAP_IPC_RECEIVE": 6, "CAP_PROCESS_SPAWN": 7, "CAP_PROCESS_KILL": 8,
    "CAP_FILESYSTEM_READ": 9, "CAP_FILESYSTEM_WRITE": 10, "CAP_STORAGE_RAW": 11,
    "CAP_NETWORK_SOCKET": 12, "CAP_NETWORK_SEND": 13, "CAP_NETWORK_RECEIVE": 14,
    "CAP_SHM_PRODUCER": 15, "CAP_SHM_CONSUMER": 16, "CAP_EVENT_POLL": 17, "CAP_SYSTEM_CONTROL": 18,
    # Domain-scoped SystemControl identifiers from the same ABI table.
    "CAP_SYSTEM_CONTROL_POWER": 19,
    "CAP_SYSTEM_CONTROL_TELEPHONY": 20,
    "CAP_SYSTEM_CONTROL_AUDIO": 21,
    "CAP_SYSTEM_CONTROL_CAMERA": 22,
    "CAP_SYSTEM_CONTROL_SENSOR": 23,
    "CAP_REALTIME_CLOCK": 24,
    # The content layer: the shared document index and the system-wide intent
    # registry. CAP_CONTENT_INDEX reads the index and the registry; it does not
    # open the documents the index describes. CAP_CONTENT_INTENT dispatches an
    # intent to the package that declared a handler for it; it is not
    # CAP_PROCESS_SPAWN, because it can only reach the one package the
    # registry names.
    "CAP_CONTENT_INDEX": 25,
    "CAP_CONTENT_INTENT": 26,
    # AC'97 PCM output. Separate from CAP_SYSTEM_CONTROL_AUDIO (21), which is
    # the mixer/feedback control object: a player needs exactly the authority
    # to open, feed and close the PCM stream, not system audio policy.
    "CAP_AUDIO_STREAM": 28,

    # Template-facing aliases.
    #
    # WHY every alias here must resolve to a capability whose documented type
    # actually delivers what the name promises: an alias is how a template ends
    # up holding authority nobody intended to hand it. An alias may only
    # restate an identifier above or narrow it to a documented sub-domain.
    "CAP_WINDOW_FOCUS": 17,          # EventPoll: input and event queues.
    "CAP_SECURE_STORAGE": 10,        # FileSystemWrite: sandboxed VFS persistence.
    "CAP_SENSOR_ACCEL": 23,          # SystemControl scoped to the sensor domain.
    "CAP_SENSOR_GYRO": 23,
    "CAP_SENSOR_HR": 23,
    "CAP_MESH_RADIO": 12,            # NetworkSocket: create UDP/TCP transports.
    "CAP_P2P_SOCKET": 13,            # NetworkSend: transmit packets.
}

# Identifiers a template-facing alias must never resolve to, because handing one
# to a generated app would grant authority unrelated to what the template does:
# raw hardware bus access, raw block storage, IPC to a chosen endpoint, process
# control, or device-wide administration.
ALIAS_FORBIDDEN_CAPABILITY_IDS = frozenset({4, 5, 6, 7, 8, 11, 18})

TEMPLATES = {
    "basic": {
        "title": "Basic App",
        "description": "Standard #![no_std] Rust mobile app with UI frontend",
        "permissions": ["CAP_LOGGING", "CAP_WINDOW_FOCUS"],
    },
    "authenticator": {
        "title": "TOTP Authenticator",
        # WHY this description does not mention a hardware enclave: RAPP v2
        # defines no keystore or enclave capability, so there is nothing to
        # request. Secrets are persisted in the app's sandboxed VFS mount, which
        # is encrypted-at-rest by the kernel's storage vault but is ordinary
        # file storage as far as the app is concerned.
        "description": (
            "Cryptographic TOTP authenticator. Seeds are persisted in the app's "
            "sandboxed VFS mount (encrypted at rest by the kernel vault). RAPP v2 "
            "defines no hardware keystore or enclave capability, so none is claimed."
        ),
        "permissions": ["CAP_SECURE_STORAGE"],
    },
    "sensor": {
        "title": "Sensor Telemetry Hub",
        # WHY the sensor permissions are the sensor domain of SystemControl:
        # sys_sensor_read is gated on SERVICE_SENSOR_CONTROL_OBJECT, which is
        # capability 23. Mapping these to raw port I/O or to the timer would have
        # granted a telemetry app hardware bus access it cannot use.
        "description": (
            "Reads the IMU, accelerometer and biometric sensors through the "
            "sensor domain of SystemControl (capability 23)"
        ),
        "permissions": ["CAP_SENSOR_ACCEL", "CAP_SENSOR_GYRO", "CAP_SENSOR_HR"],
    },
    "meshchat": {
        "title": "Mesh Messenger",
        "description": "Decentralized peer-to-peer mesh networking and encrypted messaging",
        "permissions": ["CAP_MESH_RADIO", "CAP_P2P_SOCKET"],
    },
}

SUPPORTED_DEVICES = {
    "x86_64": {
        "name": "InterOS x86_64 PC / VM",
        "arch": "x86_64",
        "machine": "pc-q35",
        "cpu": "qemu64",
        "boot_type": "BIOS/MBR Bootloader",
        "image": "target/inter-os-bios.img",
        "status": "Canonical QEMU Phone contract; build/runtime validation is explicit and evidence-based",
    },
    "aarch64": {
        "name": "InterOS AArch64 Virt Mobile",
        "arch": "aarch64",
        "machine": "virt",
        "cpu": "cortex-a53",
        "boot_type": "Direct Kernel ELF",
        "image": "target/aarch64-unknown-none/debug/kernel-aarch64",
        "status": "Source-level architecture target; runtime validation pending",
    },
    "riscv64": {
        "name": "InterOS RISC-V Virt Mobile",
        "arch": "riscv64",
        "machine": "virt",
        "cpu": "rv64",
        "boot_type": "OpenSBI / Direct Kernel ELF",
        "image": "target/riscv64gc-unknown-none-elf/debug/kernel-riscv64",
        "status": "Source-level architecture target; runtime validation pending",
    },
}

if sys.platform == "win32":
    try:
        sys.stdout.reconfigure(encoding='utf-8', errors='replace')
        sys.stderr.reconfigure(encoding='utf-8', errors='replace')
    except Exception:
        pass


def log(msg, prefix="[*]"):
    print(f"{prefix} {msg}")


def log_success(msg):
    print(f"[OK] {msg}")


def log_error(msg):
    print(f"[ERROR] {msg}", file=sys.stderr)


def log_warn(msg):
    print(f"[WARN] {msg}")


def find_binary(name):
    """Find binary in PATH or common user tool directories."""
    p = shutil.which(name)
    if p:
        return p
    common_dirs = [
        Path.home() / "qemu",
        Path(os.environ.get("ProgramFiles", "C:/Program Files")) / "qemu",
    ]
    if "QEMU_PATH" in os.environ:
        common_dirs.insert(0, Path(os.environ["QEMU_PATH"]))
    if "QEMU_DIR" in os.environ:
        common_dirs.insert(0, Path(os.environ["QEMU_DIR"]))
    for d in common_dirs:
        candidate = d / f"{name}.exe" if sys.platform == "win32" else d / name
        if candidate.exists():
            return str(candidate)
    return None


# =============================================================================
# COMMAND: doctor
# =============================================================================
def cmd_doctor(args):
    print("=" * 64)
    print("  InterOS Core Environment Diagnostic & Doctor")
    print("=" * 64)

    all_passed = True

    # 1. Rust Compiler
    rustc_path = shutil.which("rustc")
    if rustc_path:
        res = subprocess.run(["rustc", "--version"], capture_output=True, text=True)
        v = res.stdout.strip()
        log_success(f"Rust compiler: {v}")
    else:
        log_error("rustc not found in PATH")
        all_passed = False

    # 2. Cargo
    cargo_path = shutil.which("cargo")
    if cargo_path:
        res = subprocess.run(["cargo", "--version"], capture_output=True, text=True)
        log_success(f"Cargo: {res.stdout.strip()}")
    else:
        log_error("cargo not found in PATH")
        all_passed = False

    # 3. Rust Targets
    target_triples = [
        "x86_64-unknown-none",
        "aarch64-unknown-none",
        "riscv64gc-unknown-none-elf",
    ]
    installed_targets = []
    try:
        res = subprocess.run(["rustup", "target", "list", "--installed"], capture_output=True, text=True)
        if res.returncode == 0:
            installed_targets = [line.strip() for line in res.stdout.splitlines() if line.strip()]
    except Exception:
        pass

    for t in target_triples:
        if t in installed_targets:
            log_success(f"Rust target installed: {t}")
        else:
            log_warn(f"Rust target missing: {t} (Run: rustup target add {t})")
            all_passed = False

    # 4. QEMU Emulators
    qemu_binaries = {
        "x86_64": "qemu-system-x86_64",
        "aarch64": "qemu-system-aarch64",
        "riscv64": "qemu-system-riscv64",
    }
    for arch, qbin in qemu_binaries.items():
        found = find_binary(qbin)
        if found:
            log_success(f"QEMU {arch} emulator: {found}")
        else:
            log_error(f"QEMU {arch} emulator ({qbin}) not found")
            all_passed = False

    # 5. Python Cryptography
    try:
        import cryptography
        log_success(f"Python cryptography: v{cryptography.__version__}")
    except ImportError:
        log_error("Python 'cryptography' library not installed (Run: pip install cryptography)")
        all_passed = False

    # 6. Disk Builder (boot crate)
    boot_cargo = WORKSPACE_ROOT / "boot" / "Cargo.toml"
    if boot_cargo.exists():
        log_success(f"BIOS Disk image builder: {boot_cargo.relative_to(WORKSPACE_ROOT)}")
    else:
        log_error("boot/Cargo.toml missing")
        all_passed = False

    # 7. Canonical QEMU Phone manifest
    manifest = WORKSPACE_ROOT / "platform" / "x86_64" / "userspace-manifest.json"
    if manifest.exists():
        log_success(f"Canonical QEMU userspace manifest: {manifest.relative_to(WORKSPACE_ROOT)}")
    else:
        log_error(f"Canonical QEMU userspace manifest missing: {manifest}")
        all_passed = False

    # 7. Device Studio Port Status
    port = getattr(args, "port", 4300)
    try:
        req = urllib.request.Request(f"http://127.0.0.1:{port}/api/status", method="GET")
        with urllib.request.urlopen(req, timeout=1) as resp:
            data = json.loads(resp.read().decode("utf-8"))
            log_success(f"Device Studio server active on port {port} (Platform: {data.get('platform', 'InterOS')})")
    except Exception:
        log_warn(f"Device Studio server not running on port {port} (Run: python emulator/server.py)")

    print("-" * 64)
    if all_passed:
        log_success("All core prerequisites and compilation targets are ready!")
        return 0
    else:
        log_warn("Some prerequisites are missing or need configuration.")
        return 1


# =============================================================================
# COMMAND: build
# =============================================================================
USERSPACE_MANIFEST = WORKSPACE_ROOT / "platform" / "x86_64" / "userspace-manifest.json"

def _load_x86_userspace_manifest():
    if not USERSPACE_MANIFEST.exists():
        raise RuntimeError(f"Canonical userspace manifest missing: {USERSPACE_MANIFEST}")
    try:
        data = json.loads(USERSPACE_MANIFEST.read_text(encoding="utf-8"))
    except Exception as exc:
        raise RuntimeError(f"Invalid userspace manifest: {exc}") from exc
    required = data.get("required")
    if not isinstance(required, list) or not required:
        raise RuntimeError("Canonical userspace manifest has no required artifacts")
    return data


def _cargo_build_manifest(manifest_path, target, bin_name=None):
    cmd = [
        "cargo", "build", "--locked",
        "--manifest-path", str(WORKSPACE_ROOT / manifest_path),
        "--target", target,
    ]
    if bin_name:
        cmd.extend(["--bin", bin_name])
    log(f"Building {bin_name or manifest_path} [{target}]...")
    env = dict(os.environ)
    if target == "x86_64-unknown-none":
        env["RUSTFLAGS"] = "-C relocation-model=static -C link-arg=-Ttext=0x00400000 -C link-arg=-no-pie -C opt-level=3 -C link-arg=-z -C link-arg=norelro --cfg curve25519_dalek_backend=\"fiat\""
    return subprocess.run(cmd, cwd=str(WORKSPACE_ROOT), env=env).returncode


def _build_x86_userspace():
    ps1 = WORKSPACE_ROOT / "tools" / "build_userspace.ps1"
    if sys.platform == "win32" and ps1.exists():
        log("Building canonical x86_64 userspace via tools/build_userspace.ps1...")
        rc = subprocess.run(["powershell", "-NoProfile", "-ExecutionPolicy", "Bypass", "-File", str(ps1)], cwd=str(WORKSPACE_ROOT)).returncode
        if rc == 0:
            return True
        log_warn("build_userspace.ps1 returned error, falling back to manifest builder")
    manifest = _load_x86_userspace_manifest()
    target = manifest["target_triple"]
    output_root = WORKSPACE_ROOT / manifest["output_root"]
    output_root.mkdir(parents=True, exist_ok=True)

    for item in manifest["required"]:
        name = item["name"]
        if item["kind"] == "system-service":
            rc = _cargo_build_manifest(item["manifest"], target, name)
        else:
            rc = _cargo_build_manifest(item["manifest"], target)
        if rc != 0:
            log_error(f"Userspace artifact build failed: {name}")
            return False

        built = output_root / name
        if not built.exists():
            alt = output_root / f"{name}.elf"
            if alt.exists():
                built = alt
        if not built.exists():
            log_error(f"Expected userspace ELF was not produced: {built}")
            return False

        # Keep the exact path consumed by kernel/src/task/images.rs.
        if built != output_root / name:
            shutil.copy2(built, output_root / name)

    missing = [item["name"] for item in manifest["required"] if not (output_root / item["name"]).is_file()]
    if missing:
        log_error("Canonical userspace graph is incomplete: " + ", ".join(missing))
        return False
    log_success(f"Canonical x86_64 userspace graph ready ({len(manifest['required'])} ELF artifacts).")
    return True


def _build_x86_kernel_and_image():
    if not _build_x86_userspace():
        return 1

    log("Building InterOS Kernel [x86_64-unknown-none]...")
    # WHY release is the default here: the kernel IS the compositor and the
    # framebuffer writer, so its profile is the frame cost. Measured on this
    # repo, under QEMU's TCG accelerator at 720x1440, the same boot flips in a
    # 32.5M-rdtsc-cycle median as a release kernel against 44.3M as a debug
    # kernel, and the debug worst case is 94.3M against release's 41.8M. That is
    # the guest's single largest fixed cost, so shipping a debug kernel pays for
    # a speed the device does not have.
    #
    # WHY debug is still reachable: a profile switch is only trustworthy if it
    # can be turned back. `INTEROS_KERNEL_PROFILE=debug` builds the original
    # artefact so the two can be compared without editing the build.
    kernel_profile = os.environ.get("INTEROS_KERNEL_PROFILE", "release").strip().lower()
    if kernel_profile not in ("release", "debug"):
        kernel_profile = "release"
    cargo_build = [
        "cargo", "build", "--locked",
        "--manifest-path", "kernel/Cargo.toml",
        "--target", "x86_64-unknown-none"
    ]
    if kernel_profile == "release":
        cargo_build.append("--release")
    res = subprocess.run(cargo_build, cwd=str(WORKSPACE_ROOT))
    if res.returncode != 0:
        log_error("x86_64 kernel build failed")
        return res.returncode

    k1 = WORKSPACE_ROOT / "kernel" / "target" / "x86_64-unknown-none" / kernel_profile / "kernel"
    k2 = WORKSPACE_ROOT / "target" / "x86_64-unknown-none" / kernel_profile / "kernel"
    kernel_path = k1 if (k1.exists() and (not k2.exists() or k1.stat().st_mtime >= k2.stat().st_mtime)) else k2
    if not kernel_path.exists():
        log_error(f"Kernel artifact missing after successful cargo build: {kernel_path}")
        return 1

    if kernel_path == k1:
        k2.parent.mkdir(parents=True, exist_ok=True)
        shutil.copy2(k1, k2)
    elif kernel_path == k2 and k1.parent.exists():
        shutil.copy2(k2, k1)

    log("Packaging canonical QEMU Phone BIOS image...")
    res = subprocess.run([
        "cargo", "run", "--locked",
        "--manifest-path", "boot/Cargo.toml",
        "--", str(kernel_path), "target/inter-os-bios.img"
    ], cwd=str(WORKSPACE_ROOT))
    if res.returncode != 0:
        log_error("BIOS disk image packaging failed")
        return res.returncode

    img_path = TARGET_DIR / "inter-os-bios.img"
    if not img_path.exists():
        log_error(f"Boot image missing after packaging: {img_path}")
        return 1
    log_success(f"x86_64 QEMU Phone image ready: {img_path.relative_to(WORKSPACE_ROOT)} ({img_path.stat().st_size} bytes)")
    return 0


def cmd_build(args):
    target_arg = getattr(args, "target", None) or getattr(args, "name", "all")
    target_lower = target_arg.lower().replace("-", "_")

    # Known application: build it for the explicitly selected architecture.
    app_dir = APPS_DIR / target_lower
    if app_dir.exists():
        arch_target = getattr(args, "arch", None) or "x86_64-unknown-none"
        log(f"Compiling application '{target_lower}' for target {arch_target}...")
        env = dict(os.environ)
        if arch_target == "x86_64-unknown-none":
            env["RUSTFLAGS"] = "-C relocation-model=static -C link-arg=-Ttext=0x00400000 -C link-arg=-z -C link-arg=norelro --cfg curve25519_dalek_backend=\"fiat\""
        res = subprocess.run([
            "cargo", "build", "--locked", "--manifest-path", str(app_dir / "Cargo.toml"),
            "--target", arch_target
        ], cwd=str(WORKSPACE_ROOT), env=env)
        if res.returncode != 0:
            log_error(f"Application build failed for {target_lower}")
            return res.returncode
        bin_dir = app_dir / "bin"
        bin_dir.mkdir(exist_ok=True)
        out_elf = bin_dir / f"{target_lower}.elf"
        elf_src = WORKSPACE_ROOT / "target" / arch_target / "debug" / target_lower
        if not elf_src.exists():
            elf_src = WORKSPACE_ROOT / "target" / arch_target / "debug" / f"{target_lower}.exe"
        if not elf_src.exists():
            log_error(f"Cargo completed but ELF output was not found: {elf_src}")
            return 1
        shutil.copy2(elf_src, out_elf)
        log_success(f"Built ELF binary: {out_elf.relative_to(WORKSPACE_ROOT)} ({out_elf.stat().st_size} bytes)")
        return 0

    if target_lower in ("demo", "qemu", "phone"):
        return _build_x86_kernel_and_image()

    if target_lower in ("userspace", "apps", "services"):
        return 0 if _build_x86_userspace() else 1

    targets_to_build = []
    if target_lower in ["x86_64", "x86", "pc"]:
        targets_to_build.append("x86_64")
    elif target_lower in ["aarch64", "arm64"]:
        targets_to_build.append("aarch64")
    elif target_lower in ["riscv64", "riscv"]:
        targets_to_build.append("riscv64")
    elif target_lower in ["all", "kernels"]:
        targets_to_build = ["x86_64", "aarch64", "riscv64"]
    else:
        log_error(f"Unknown build target or application: {target_arg}")
        return 1

    for t in targets_to_build:
        if t == "x86_64":
            rc = _build_x86_kernel_and_image()
            if rc != 0:
                return rc
        elif t == "aarch64":
            log("Building InterOS Kernel [aarch64-unknown-none]...")
            res = subprocess.run([
                "cargo", "build", "--locked", "--manifest-path", "kernel-aarch64/Cargo.toml",
                "--target", "aarch64-unknown-none"
            ], cwd=str(WORKSPACE_ROOT))
            if res.returncode != 0:
                log_error("aarch64 kernel build failed")
                return res.returncode
        elif t == "riscv64":
            log("Building InterOS Kernel [riscv64gc-unknown-none-elf]...")
            res = subprocess.run([
                "cargo", "build", "--locked", "--manifest-path", "kernel-riscv64/Cargo.toml",
                "--target", "riscv64gc-unknown-none-elf"
            ], cwd=str(WORKSPACE_ROOT))
            if res.returncode != 0:
                log_error("riscv64 kernel build failed")
                return res.returncode

    log_success("Requested build(s) completed successfully.")
    return 0


# =============================================================================
# COMMAND: test
# =============================================================================
# WHY this exists: `cargo test --manifest-path kernel/Cargo.toml` is the command
# AGENTS.md requires before every commit, but nothing in the tree ran it. A run
# that failed therefore left no record of which test failed -- a single
# "288 passed; 1 failed" with no test name is not diagnosable. This wrapper keeps
# the full output, names the failing tests and classifies the failure, so a
# future failure is actionable instead of anecdotal.
KERNEL_TEST_CMD = ["cargo", "test", "--manifest-path", "kernel/Cargo.toml", "--no-fail-fast"]
TEST_LOG_DIR = WORKSPACE_ROOT / "target" / "test-logs"

# Ordered most-specific first: the first pattern that matches names the category.
_FAILURE_PATTERNS = (
    ("process launch error", ("could not execute", "no such file or directory", "is not recognized")),
    ("compile error", ("error[E", "error: could not compile", "error: aborting")),
    ("timeout", ("timed out", "timeout", "deadline exceeded")),
    ("panic", ("panicked at", "thread '", "RUST_BACKTRACE")),
    ("assertion failure", ("assertion failed", "assert `left == right` failed", "assertion `")),
)


def classify_test_failure(output):
    """Classify a failed cargo test run from its own output.

    WHY this reads the log rather than the exit code: cargo reports 101 for an
    assertion, a panic and a compile error alike, so the exit code alone cannot
    tell a real regression from a broken toolchain. Returns the categories that
    matched, in the order above, and an empty list when nothing matched -- an
    unclassified failure is still reported, never silently dropped.
    """
    lowered = output.lower()
    found = [name for name, needles in _FAILURE_PATTERNS if any(n in lowered for n in needles)]
    return found


def failing_test_names(output):
    """Extract the names of failing tests from libtest output.

    Covers both the per-test `test <name> ... FAILED` lines and the `failures:`
    block, which lists the same names again. Names are returned in first-seen
    order and de-duplicated so one failure is not reported twice.
    """
    names = []
    in_failures = False
    for raw in output.splitlines():
        line = raw.strip()
        if line == "failures:":
            in_failures = True
            continue
        if in_failures and line and not line.startswith(" ") and not line.startswith("\t"):
            in_failures = False
        if line.startswith("test ") and line.endswith("FAILED"):
            candidate = line[len("test "):-len(" FAILED")].strip()
            candidate = candidate.split(" ...")[0].strip()
            if candidate and candidate not in names:
                names.append(candidate)
            continue
        if in_failures and line.startswith("    ") and not line.startswith("     "):
            candidate = line.strip()
            if candidate and candidate not in names:
                names.append(candidate)
    return names


def test_count_summary(output):
    """Pull the `N passed; M failed` totals out of libtest output, or None.

    WHY the first result with any tests in it rather than the last match:
    `cargo test` runs more than one test binary (here the 289 unit tests, then a
    second binary that reports none), and the trailing one is routinely "0 passed".
    Taking the last match reports "0 passed" for a suite that really ran 289 --
    which is exactly the kind of wrong-but-plausible number this exists to stop.
    """
    import re
    matches = re.findall(r"(\d+) passed; (\d+) failed", output)
    for passed, failed in matches:
        if int(passed) + int(failed) > 0:
            return (int(passed), int(failed))
    if matches:
        return (0, 0)
    passed = re.search(r"(\d+) passed", output)
    if passed:
        return (int(passed.group(1)), 0)
    return None


def _diagnostic_environment():
    """Environment facts that explain a kernel test failure, with no secrets.

    WHY this list: a `no_std` kernel suite fails differently under a different
    toolchain or with a different test-threads setting, and both are worth seeing.
    Anything that could carry a credential is deliberately excluded.
    """
    return {
        "cwd": str(WORKSPACE_ROOT),
        "os": f"{sys.platform} {platform.release()}",
        "python": sys.version.split()[0],
        "RUSTUP_TOOLCHAIN": os.environ.get("RUSTUP_TOOLCHAIN", "<unset>"),
        "RUSTFLAGS": os.environ.get("RUSTFLAGS", "<unset>"),
        "CARGO_TARGET_DIR": os.environ.get("CARGO_TARGET_DIR", "<unset>"),
        "RUST_TEST_THREADS": os.environ.get("RUST_TEST_THREADS", "<unset>"),
        "num_cpus": str(os.cpu_count()),
    }


def run_kernel_tests():
    """Run the mandated kernel unit suite, preserving full diagnostics on failure.

    Returns the cargo exit code. Output is written to a timestamped log under
    target/test-logs/ on every run, so a failure that only appears once still has
    its evidence on disk afterwards.
    """
    command = " ".join(KERNEL_TEST_CMD)
    log(f"Running kernel unit tests: {command}")

    try:
        completed = subprocess.run(
            KERNEL_TEST_CMD,
            cwd=str(WORKSPACE_ROOT),
            stdout=subprocess.PIPE,
            stderr=subprocess.STDOUT,
            text=True,
            errors="replace",
        )
        returncode = completed.returncode
        output = completed.stdout or ""
    except OSError as exc:
        # A missing toolchain must be reported as itself, not as a test failure.
        returncode = 127
        output = f"failed to execute the test command: {exc}\n"

    TEST_LOG_DIR.mkdir(parents=True, exist_ok=True)
    stamp = time.strftime("%Y%m%d-%H%M%S", time.gmtime())
    log_path = TEST_LOG_DIR / f"kernel-tests-{stamp}.log"
    log_path.write_text(output, encoding="utf-8", errors="replace")

    counts = test_count_summary(output)
    if returncode == 0:
        log_success(f"Kernel tests passed ({counts[0]} passed)." if counts else "Kernel tests passed.")
        log(f"Full output: {log_path}")
        return 0

    log_error(f"Kernel tests FAILED (exit code {returncode}).")
    log_error(f"Command: {command}")
    log_error(f"Working directory: {WORKSPACE_ROOT}")
    if counts:
        log_error(f"Result: {counts[0]} passed; {counts[1]} failed")
    categories = classify_test_failure(output)
    log_error(f"Failure classification: {', '.join(categories) if categories else 'unclassified (see log)'}")
    names = failing_test_names(output)
    if names:
        log_error(f"Failing test{'s' if len(names) != 1 else ''} ({len(names)}):")
        for name in names:
            log_error(f"  - {name}")
    else:
        log_error("No individual test name was reported; the run failed before or outside libtest.")
    log_error(f"Complete stdout+stderr: {log_path}")
    for key, value in _diagnostic_environment().items():
        log(f"  {key}={value}")
    return returncode


def cmd_test(args):
    test_target = getattr(args, "target", "all").lower()
    valid_targets = ["unit", "kernel", "x86_64", "x86", "aarch64", "riscv64", "security", "adversarial", "qemu", "lifecycle", "contracts", "m10_m12", "stress", "recovery", "persistence", "storage_recovery", "crash", "watchdog", "panic", "all"]
    if test_target not in valid_targets:
        log_error(f"Unknown test target: '{test_target}'. Valid targets: {', '.join(valid_targets)}")
        return 1

    if test_target in ["kernel", "all"]:
        rc = run_kernel_tests()
        if rc != 0:
            overall_rc = rc
            if test_target == "kernel":
                return rc

    log(f"Executing InterOS test suite (Target: {test_target})...")
    overall_rc = 0

    if test_target in ["contracts", "all"]:
        contracts_script = WORKSPACE_ROOT / "tests" / "run_all_contracts.py"
        if contracts_script.exists():
            log("Executing InterOS Core Contract Test Suite...")
            res = subprocess.run([sys.executable, str(contracts_script)], cwd=str(WORKSPACE_ROOT))
            if res.returncode != 0:
                overall_rc = res.returncode
                if test_target == "contracts":
                    return res.returncode

    if test_target in ["unit", "all"]:
        log("Running cargo test on host-compatible crates...")
        res = subprocess.run(["cargo", "test", "--locked", "-p", "interos-std"], cwd=str(WORKSPACE_ROOT))
        if res.returncode != 0:
            overall_rc = res.returncode
            if test_target == "unit":
                return res.returncode
        if (WORKSPACE_ROOT / "boot" / "Cargo.toml").exists():
            res_boot = subprocess.run(["cargo", "test", "--locked", "--manifest-path", "boot/Cargo.toml"], cwd=str(WORKSPACE_ROOT))
            if res_boot.returncode != 0:
                overall_rc = res_boot.returncode

    if test_target in ["x86_64", "x86", "all"]:
        x86_script = WORKSPACE_ROOT / "tests" / "verify_x86_boot.ps1"
        if x86_script.exists() and sys.platform == "win32":
            log("Executing QEMU x86_64 boot and serial verification...")
            res = subprocess.run(["powershell", "-ExecutionPolicy", "Bypass", "-File", str(x86_script)])
            if res.returncode != 0:
                overall_rc = res.returncode
                if test_target in ["x86_64", "x86"]:
                    return res.returncode


    if test_target in ["qemu", "lifecycle", "all"]:
        if test_target in ["qemu", "all"]:
            boot_script = WORKSPACE_ROOT / "tests" / "verify_x86_boot.py"
            if boot_script.exists():
                log("Executing canonical x86_64 QEMU boot verification...")
                res_boot = subprocess.run([sys.executable, "-u", str(boot_script)], cwd=str(WORKSPACE_ROOT))
                if res_boot.returncode != 0:
                    overall_rc = res_boot.returncode
                    if test_target == "qemu":
                        return res_boot.returncode

        lifecycle_script = WORKSPACE_ROOT / "tests" / "test_interactive_lifecycle.py"
        if lifecycle_script.exists():
            log("Executing QEMU full interactive smartphone lifecycle verification...")
            res = subprocess.run([sys.executable, "-u", str(lifecycle_script)], cwd=str(WORKSPACE_ROOT))
            if res.returncode != 0:
                overall_rc = res.returncode
                if test_target in ["qemu", "lifecycle"]:
                    return res.returncode

    if test_target in ["stress"]:
        stress_script = WORKSPACE_ROOT / "tests" / "test_lifecycle_stress.py"
        if stress_script.exists():
            log("Executing QEMU multi-round lifecycle stress & resource verification...")
            res = subprocess.run([sys.executable, "-u", str(stress_script)], cwd=str(WORKSPACE_ROOT))
            if res.returncode != 0:
                return res.returncode

    if test_target in ["recovery"]:
        rec_script = WORKSPACE_ROOT / "tests" / "test_reboot_recovery.py"
        if rec_script.exists():
            log("Executing QEMU clean reboot recovery verification...")
            res = subprocess.run([sys.executable, "-u", str(rec_script)], cwd=str(WORKSPACE_ROOT))
            if res.returncode != 0:
                return res.returncode

    if test_target in ["persistence"]:
        pers_script = WORKSPACE_ROOT / "tests" / "test_persistence.py"
        if pers_script.exists():
            log("Executing QEMU cross-process persistent storage verification...")
            res = subprocess.run([sys.executable, "-u", str(pers_script)], cwd=str(WORKSPACE_ROOT))
            if res.returncode != 0:
                return res.returncode

    if test_target in ["storage_recovery", "crash"]:
        sr_script = WORKSPACE_ROOT / "tests" / "test_storage_recovery.py"
        if sr_script.exists():
            log("Executing QEMU power-loss crash recovery & transactional rollback verification...")
            res = subprocess.run([sys.executable, "-u", str(sr_script)], cwd=str(WORKSPACE_ROOT))
            if res.returncode != 0:
                return res.returncode

    if test_target in ["watchdog", "panic"]:
        wd_script = WORKSPACE_ROOT / "tests" / "test_crash_watchdog.py"
        if wd_script.exists():
            log("Executing QEMU controlled panic & watchdog recovery verification...")
            res = subprocess.run([sys.executable, "-u", str(wd_script)], cwd=str(WORKSPACE_ROOT))
            if res.returncode != 0:
                return res.returncode

    if test_target in ["aarch64", "all"]:
        arm_script = WORKSPACE_ROOT / "tests" / "test_aarch64.ps1"
        if arm_script.exists() and sys.platform == "win32":
            log("Executing QEMU AArch64 boot verification...")
            res = subprocess.run(["powershell", "-ExecutionPolicy", "Bypass", "-File", str(arm_script)])
            if res.returncode != 0:
                overall_rc = res.returncode
                if test_target == "aarch64":
                    return res.returncode

    if test_target in ["riscv64", "all"]:
        riscv_script = WORKSPACE_ROOT / "tests" / "test_riscv64.ps1"
        if riscv_script.exists() and sys.platform == "win32":
            log("Executing QEMU RISC-V boot verification...")
            res = subprocess.run(["powershell", "-ExecutionPolicy", "Bypass", "-File", str(riscv_script)])
            if res.returncode != 0:
                overall_rc = res.returncode
                if test_target == "riscv64":
                    return res.returncode

    if test_target in ["security", "adversarial", "all"]:
        sec_script = WORKSPACE_ROOT / "tests" / "test_adversarial_security.py"
        if sec_script.exists():
            log("Executing Zero-Trust Adversarial Security Verification Suite...")
            res = subprocess.run([sys.executable, str(sec_script)])
            if res.returncode != 0:
                log_error("Adversarial security test suite failed!")
                overall_rc = res.returncode
                if test_target in ["security", "adversarial"]:
                    return res.returncode
            else:
                log_success("All adversarial security invariants validated.")

        studio_sec_script = WORKSPACE_ROOT / "tests" / "test_studio_security.py"
        if studio_sec_script.exists():
            log("Executing Studio Adversarial Security Verification Suite...")
            res = subprocess.run([sys.executable, str(studio_sec_script)])
            if res.returncode != 0:
                log_error("Studio security test suite failed!")
                overall_rc = res.returncode
                if test_target in ["security", "adversarial"]:
                    return res.returncode
            else:
                log_success("Studio adversarial security invariants validated.")

    if overall_rc != 0:
        log_error(f"Test execution finished with failure (code: {overall_rc}).")
        return overall_rc

    log_success("Test execution finished successfully.")
    return 0


# =============================================================================
# COMMAND: package (alias: pack)
# =============================================================================
def _rapp_v2_encode(manifest_data, publisher_key, signature, payload, arch_name=None):
    arch_map = {"x86_64": 0, "aarch64": 1, "riscv64": 2}
    arch_name = arch_name or manifest_data.get("architecture", "x86_64")
    if arch_name not in arch_map:
        raise ValueError(f"Unsupported architecture: {arch_name}")
    version = tuple(int(x) for x in str(manifest_data["version"]).split("."))
    if len(version) != 3 or any(x < 0 or x > 65535 for x in version):
        raise ValueError("version must be MAJOR.MINOR.PATCH with u16 components")
    caps = [RAPP_CAPABILITY_IDS[p] for p in manifest_data.get("permissions", [])]
    if len(set(caps)) != len(caps):
        raise ValueError("duplicate capability permissions are not allowed")
    if len(caps) > 1024:
        raise ValueError("too many capabilities")
    pkg = manifest_data["id"].encode("utf-8")
    if not pkg or len(pkg) > 32:
        raise ValueError("package id must be 1..32 UTF-8 bytes")
    digest = hashlib.sha256(payload).digest()
    header = bytearray()
    header += b"RAPP" + (2).to_bytes(2, "little") + arch_map[arch_name].to_bytes(2, "little")
    header += pkg.ljust(32, b"\0")
    header += publisher_key
    header += version[0].to_bytes(2, "little") + version[1].to_bytes(2, "little") + version[2].to_bytes(2, "little")
    header += len(caps).to_bytes(2, "little")
    header += digest
    header += signature
    header += len(payload).to_bytes(4, "little")
    for cap in caps:
        header += int(cap).to_bytes(2, "little")
    return bytes(header) + payload


def _rapp_v2_signing_bytes(package_bytes):
    data = bytearray(package_bytes)
    data[0x70:0xB0] = b"\0" * 64
    return bytes(data[:0xB4] + data[0xB4:])


# The intent kinds a RAPP v4 manifest may declare, and their wire values.
RAPP_INTENT_KINDS = {"mime": 0, "scheme": 1, "action": 2}
RAPP_MAX_INTENTS = 64
RAPP_MAX_INTENT_VALUE_LEN = 96


def _rapp_intent_table(manifest_data):
    """Encodes the manifest's declared handlers into the RAPP v4 intent table.

    WHY the table is inside the signed package: "this package handles
    image/png" is a claim the device acts on when it resolves an intent, and a
    claim that could be edited after signing would be no claim at all.
    """
    intents = manifest_data.get("intents") or {}
    if not isinstance(intents, dict):
        raise ValueError("intents must be an object with mime/scheme/action lists")
    entries = []
    for kind in ("mime", "scheme", "action"):
        values = intents.get(kind, []) or []
        if not isinstance(values, list):
            raise ValueError(f"intents.{kind} must be a list")
        for value in values:
            if not isinstance(value, str) or not value:
                raise ValueError(f"intents.{kind} entries must be non-empty strings")
            encoded = value.encode("utf-8")
            if len(encoded) > RAPP_MAX_INTENT_VALUE_LEN:
                raise ValueError(f"intent value too long: {value}")
            entries.append((RAPP_INTENT_KINDS[kind], encoded))
    if len(entries) > RAPP_MAX_INTENTS:
        raise ValueError("too many intent declarations")
    table = bytearray(len(entries).to_bytes(2, "little"))
    for kind, encoded in entries:
        table += bytes([kind, len(encoded)]) + encoded
    return bytes(table)


def _rapp_v4_encode(manifest_data, publisher_key, signature, payload, arch_name=None):
    """Encodes a RAPP v4 package: the v3 header plus a signed intent table.

    WHY v4 and not an extension of v2: a package that declares no handlers
    must still be describable, and the intent table is a different kind of
    declaration from a dependency. v4 leaves every earlier format's bytes
    unchanged.
    """
    arch_map = {"x86_64": 0, "aarch64": 1, "riscv64": 2}
    arch_name = arch_name or manifest_data.get("architecture", "x86_64")
    if arch_name not in arch_map:
        raise ValueError(f"Unsupported architecture: {arch_name}")
    version = tuple(int(x) for x in str(manifest_data["version"]).split("."))
    if len(version) != 3 or any(x < 0 or x > 65535 for x in version):
        raise ValueError("version must be MAJOR.MINOR.PATCH with u16 components")
    caps = [RAPP_CAPABILITY_IDS[p] for p in manifest_data.get("permissions", [])]
    if len(set(caps)) != len(caps):
        raise ValueError("duplicate capability permissions are not allowed")
    if len(caps) > 1024:
        raise ValueError("too many capabilities")
    pkg = manifest_data["id"].encode("utf-8")
    if not pkg or len(pkg) > 32:
        raise ValueError("package id must be 1..32 UTF-8 bytes")
    intent_table = _rapp_intent_table(manifest_data)
    dep_table = b"\0\0"
    section = (
        len(dep_table).to_bytes(4, "little")
        + len(intent_table).to_bytes(4, "little")
        + dep_table
        + intent_table
    )
    digest = hashlib.sha256(payload).digest()
    header = bytearray()
    header += b"RAPP" + (4).to_bytes(2, "little") + arch_map[arch_name].to_bytes(2, "little")
    header += pkg.ljust(32, b"\0")
    header += publisher_key
    header += version[0].to_bytes(2, "little") + version[1].to_bytes(2, "little") + version[2].to_bytes(2, "little")
    header += len(caps).to_bytes(2, "little")
    header += digest
    header += signature
    header += len(payload).to_bytes(4, "little")
    header += len(section).to_bytes(4, "little")
    for cap in caps:
        header += int(cap).to_bytes(2, "little")
    return bytes(header) + section + payload


def cmd_pack(args):
    app_name = args.name.lower().replace("-", "_")
    app_dir = APPS_DIR / app_name
    if not app_dir.exists():
        log_error(f"App not found at {app_dir}")
        return 1
    manifest_file = app_dir / "manifest.json"
    if not manifest_file.exists():
        log_error(f"manifest.json missing in {app_dir}")
        return 1
    try:
        manifest_data = json.loads(manifest_file.read_text(encoding="utf-8"))
    except Exception as e:
        log_error(f"Invalid JSON in manifest.json: {e}")
        return 1
    for field in ["id", "name", "version", "entrypoint", "permissions"]:
        if field not in manifest_data:
            log_error(f"Manifest missing required field: {field}")
            return 1
    bin_dir = app_dir / "bin"
    elf_candidates = list(bin_dir.glob("*.elf")) if bin_dir.exists() else []
    if not elf_candidates:
        build_ret = cmd_build(argparse.Namespace(name=app_name, target=None, arch="x86_64-unknown-none"))
        if build_ret != 0:
            return build_ret
        elf_candidates = list(bin_dir.glob("*.elf")) if bin_dir.exists() else []
        if not elf_candidates:
            log_error("No ELF payload found in app/bin after the automatic build")
            return 1
    payload = elf_candidates[0].read_bytes()
    try:
        from cryptography.hazmat.primitives.asymmetric import ed25519
        from cryptography.hazmat.primitives import serialization
        key_dir = Path.home() / ".interos"
        key_dir.mkdir(exist_ok=True)
        key_file = key_dir / "developer_ed25519.key"
        if key_file.exists():
            raw = key_file.read_bytes()
            try:
                priv_key = serialization.load_pem_private_key(raw, password=None)
            except ValueError:
                priv_key = ed25519.Ed25519PrivateKey.from_private_bytes(raw)
        else:
            priv_key = ed25519.Ed25519PrivateKey.generate()
            key_file.write_bytes(priv_key.private_bytes(serialization.Encoding.PEM, serialization.PrivateFormat.PKCS8, serialization.NoEncryption()))
            log(f"Generated developer signing key at {key_file}")
        pub = priv_key.public_key().public_bytes(serialization.Encoding.Raw, serialization.PublicFormat.Raw)
        # WHY the format is chosen by the manifest: a package that declares
        # handlers needs the intent table, and a package that declares none
        # stays on the older format so that a reader that predates v4 can
        # still install it.
        declares_intents = bool(manifest_data.get("intents"))
        encode = _rapp_v4_encode if declares_intents else _rapp_v2_encode
        unsigned = encode(manifest_data, pub, b"\0" * 64, payload, getattr(args, "arch", None))
        signature = priv_key.sign(_rapp_v2_signing_bytes(unsigned))
        package_bytes = encode(manifest_data, pub, signature, payload, getattr(args, "arch", None))
    except Exception as e:
        log_error(f"RAPP signing failed: {e}")
        return 1
    DIST_DIR.mkdir(parents=True, exist_ok=True)
    output = Path(getattr(args, "output", None) or DIST_DIR / f"{app_name}.rapp")
    output.parent.mkdir(parents=True, exist_ok=True)
    output.write_bytes(package_bytes)
    log_success(f"RAPP package created: {output} ({len(package_bytes)} bytes)")
    log(f"  • Publisher key: {pub.hex()}")
    log(f"  • Ed25519 signature: {signature.hex()[:24]}...")
    log(f"  • Architecture: {manifest_data.get('architecture', 'x86_64')}")
    if declares_intents:
        declared = sum(len(v) for v in (manifest_data.get("intents") or {}).values())
        log(f"  • Declared handlers: {declared} (signed into the intent table)")
    return 0


def cmd_verify(args):
    pkg_path = Path(args.package_path)
    if not pkg_path.exists():
        log_error(f"Package file not found: {pkg_path}")
        return 1
    data = pkg_path.read_bytes()
    if len(data) < 0xB4 or data[:4] != b"RAPP":
        log_error("Invalid RAPP v2 package")
        return 1
    if int.from_bytes(data[4:6], "little") != 2:
        log_error("Rejected: package is not RAPP v2")
        return 1
    pub = data[0x28:0x48]
    sig = data[0x70:0xB0]
    payload_len = int.from_bytes(data[0xB0:0xB4], "little")
    cap_count = int.from_bytes(data[0x4E:0x50], "little")
    payload_offset = 0xB4 + cap_count * 2
    if payload_offset + payload_len != len(data):
        log_error("Rejected: malformed payload bounds")
        return 1
    digest = data[0x50:0x70]
    if hashlib.sha256(data[payload_offset:]).digest() != digest:
        log_error("Rejected: payload digest mismatch")
        return 1
    try:
        from cryptography.hazmat.primitives.asymmetric import ed25519
        key = ed25519.Ed25519PublicKey.from_public_bytes(pub)
        key.verify(sig, _rapp_v2_signing_bytes(data))
    except Exception as e:
        log_error(f"Rejected: Ed25519 verification failed: {e}")
        return 1
    pkg_id = data[8:40].split(b"\0", 1)[0].decode("utf-8", errors="strict")
    version = (int.from_bytes(data[0x48:0x4A], "little"), int.from_bytes(data[0x4A:0x4C], "little"), int.from_bytes(data[0x4C:0x4E], "little"))
    log_success(f"RAPP v2 '{pkg_id}' cryptographically verified")
    log(f"  Ã¢â‚¬Â¢ Publisher key: {pub.hex()}")
    log(f"  Ã¢â‚¬Â¢ Version: {version[0]}.{version[1]}.{version[2]}")
    log(f"  Ã¢â‚¬Â¢ Payload: {payload_len} bytes")
    log("  Ã¢â‚¬Â¢ Device trust: must be provisioned by the kernel trust store")
    return 0



# =============================================================================
# COMMAND: reality
# =============================================================================
def cmd_reality(args):
    """Report the repository's explicit implementation/verification contract."""
    print("=" * 64)
    print("  InterOS Platform Reality Contract")
    print("=" * 64)
    print("Hardware state:      unavailable unless a backend reports presence")
    print("QEMU state:          emulated; never physical-hardware evidence")
    print("Verification levels: source-only / unit-tested / QEMU-verified / hardware-verified")
    print("Package trust:       Ed25519 signature + payload digest")
    print("Userspace isolation: required platform invariant")
    print("Persistent state:    required platform invariant")
    print("-" * 64)
    print("Use 'interos devices' for target profiles and 'interos contracts' for static checks.")
    return 0

# =============================================================================
# COMMAND: devices
# =============================================================================
def cmd_devices(args):
    print("=" * 64)
    print("  InterOS Supported Target Device Matrix")
    print("=" * 64)
    for dev_id, d in SUPPORTED_DEVICES.items():
        print(f"Profile: [{dev_id}] {d['name']}")
        print(f"  Architecture:  {d['arch']}")
        print(f"  QEMU Machine:  {d['machine']}")
        print(f"  CPU Model:     {d['cpu']}")
        print(f"  Boot Scheme:   {d['boot_type']}")
        print(f"  Target Image:  {d['image']}")
        print(f"  Status:        {d['status']}")
        print("-" * 64)
    return 0


# =============================================================================
# COMMAND: inspect
# =============================================================================
def cmd_inspect(args):
    target = getattr(args, "target", "").strip()
    if not target:
        log_error("Missing target argument for inspect (expected package path or host[:port])")
        return 1

    p = Path(target)
    if p.suffix == ".rapp" or (not ":" in target and not target.replace(".", "").isdigit() and target not in ("localhost", "127.0.0.1") and not target.startswith("http")):
        if p.exists() and p.suffix == ".rapp":
            return cmd_verify(argparse.Namespace(package_path=str(p)))
        log_error(f"Package or file not found: {target}")
        return 1

    # Otherwise query running studio
    port = getattr(args, "port", 4300)
    host = "127.0.0.1"
    if target.startswith("http://") or target.startswith("https://"):
        parsed = urllib.parse.urlparse(target)
        host = parsed.hostname or host
        if parsed.port:
            port = parsed.port
    elif ":" in target:
        parts = target.split(":", 1)
        host = parts[0]
        try:
            port = int(parts[1])
        except ValueError:
            log_error(f"Invalid port in inspect target: {target}")
            return 1
    elif target in ("localhost", "127.0.0.1") or target.replace(".", "").isdigit():
        host = target

    url = f"http://{host}:{port}/api/status"
    log(f"Inspecting running InterOS Studio instance at {url}...")
    try:
        req = urllib.request.Request(url, method="GET")
        with urllib.request.urlopen(req, timeout=3) as resp:
            data = json.loads(resp.read().decode("utf-8"))
            print(json.dumps(data, indent=2))
            return 0
    except Exception as e:
        log_error(f"Failed to query Studio at {url}: {e}")
        return 1


# =============================================================================
# COMMAND: new
# =============================================================================
def cmd_new(args):
    app_name = args.name.lower().replace("-", "_")
    app_dir = APPS_DIR / app_name
    template_key = args.template
    tmpl = TEMPLATES.get(template_key, TEMPLATES["basic"])

    if app_dir.exists():
        log_error(f"App directory already exists: {app_dir}")
        return 1

    log(f"Scaffolding new .rapp application '{app_name}' (template: {template_key})...")
    app_dir.mkdir(parents=True, exist_ok=True)
    (app_dir / "src").mkdir(exist_ok=True)
    (app_dir / "ui").mkdir(exist_ok=True)

    cargo_toml = f"""[package]
name = "{app_name}"
version = "0.1.0"
edition = "2021"
authors = ["InterOS Community <dev@interos.org>"]
description = "{tmpl['description']}"

[dependencies]
interos-std = {{ path = "../../sdk/rust/interos-std" }}

[[bin]]
name = "{app_name}"
path = "src/main.rs"
"""
    (app_dir / "Cargo.toml").write_text(cargo_toml, encoding="utf-8")

    manifest = {
        "id": f"org.interos.{app_name}",
        "name": app_name.replace("_", " ").title(),
        "version": "1.0.0",
        "category": "utility" if template_key != "meshchat" else "communication",
        "description": tmpl["description"],
        "icon": "ui/icon.svg",
        "entrypoint": {
            "ui": "ui/index.html",
            "backend": f"bin/{app_name}.elf"
        },
        "permissions": tmpl["permissions"],
        "min_sdk_version": "0.8.0"
    }
    (app_dir / "manifest.json").write_text(json.dumps(manifest, indent=2), encoding="utf-8")

    main_rs = f"""//! InterOS Core Application: {app_name}
#![no_std]
#![no_main]

use interos_std::ipc;
use interos_std::log;

#[no_mangle]
pub extern "C" fn _start() -> ! {{
    log::init("org.interos.{app_name}");
    log::info("{app_name} initialized inside Ring 3 sandbox");

    loop {{
        let mut msg_buf = [0u8; 512];
        if let Ok((sender_id, msg_type, _len)) = ipc::receive_any(&mut msg_buf) {{
            log::info("Received IPC packet from sender");
            let _ = ipc::send(sender_id, msg_type + 1, b"ACK");
        }}
        interos_std::yield_now();
    }}
}}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {{
    loop {{}}
}}
"""
    (app_dir / "src" / "main.rs").write_text(main_rs, encoding="utf-8")
    (app_dir / "ui" / "index.html").write_text("""<!doctype html>
<html lang="en"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><title>InterOS App</title>
<style>html,body{margin:0;min-height:100%;font-family:system-ui,sans-serif;background:#101318;color:#f5f7fa}main{max-width:720px;margin:12vh auto;padding:32px}button{padding:10px 16px;border-radius:10px;border:0}</style>
</head><body><main><h1>InterOS App</h1><p>InterOS application scaffold. Build the Ring-3 backend and implement UI behavior here.</p></main></body></html>
""", encoding="utf-8")
    (app_dir / "ui" / "icon.svg").write_text("""<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 128 128"><rect width="128" height="128" rx="28" fill="#151922"/><path d="M32 36h64v56H32z" fill="none" stroke="#fff" stroke-width="8"/><circle cx="48" cy="60" r="6" fill="#fff"/><circle cx="80" cy="60" r="6" fill="#fff"/></svg>
""", encoding="utf-8")

    root_cargo = WORKSPACE_ROOT / "Cargo.toml"
    if root_cargo.exists():
        content = root_cargo.read_text(encoding="utf-8")
        member_line = f'    "apps/{app_name}",'
        if member_line not in content and 'members = [' in content:
            new_content = content.replace('members = [', f'members = [\n{member_line}')
            root_cargo.write_text(new_content, encoding="utf-8")
            log(f"Registered apps/{app_name} in root Cargo.toml")

    log_success(f"Created application at {app_dir.relative_to(WORKSPACE_ROOT)}")
    return 0


# =============================================================================
# COMMAND: run
# =============================================================================
def _ensure_studio(port):
    """Return True when Device Studio is reachable, starting it when necessary."""
    url = f"http://127.0.0.1:{port}/api/status"
    try:
        with urllib.request.urlopen(urllib.request.Request(url, method="GET"), timeout=1):
            return True
    except Exception:
        pass

    server = WORKSPACE_ROOT / "emulator" / "server.py"
    if not server.exists():
        log_error(f"Device Studio server missing: {server}")
        return False

    log("Device Studio is offline; starting the local server...")
    kwargs = {"cwd": str(WORKSPACE_ROOT)}
    if sys.platform == "win32":
        kwargs["creationflags"] = getattr(subprocess, "CREATE_NEW_PROCESS_GROUP", 0)
    subprocess.Popen([sys.executable, str(server)], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, **kwargs)

    deadline = time.time() + 10
    while time.time() < deadline:
        try:
            with urllib.request.urlopen(urllib.request.Request(url, method="GET"), timeout=1):
                log_success(f"Device Studio is ready on port {port}.")
                return True
        except Exception:
            time.sleep(0.25)

    log_error(f"Device Studio did not become ready on port {port}.")
    return False


def cmd_run(args):
    target_name = args.name.lower().replace("-", "_")
    target = getattr(args, "target", "x86_64-unknown-none")
    port = getattr(args, "port", 4300)

    # Architecture profile boot mode
    if target_name in ("x86_64", "aarch64", "riscv64", "qemu", "phone", "demo", "kernel"):
        arch = "x86_64" if target_name in ("qemu", "phone", "demo", "kernel") else target_name
        log(f"Building {arch} microkernel...")
        b_args = argparse.Namespace(name=arch, target=None)
        if cmd_build(b_args) != 0:
            return 1
        if not _ensure_studio(port):
            return 1
        url = f"http://127.0.0.1:{port}/api/qemu/launch"
        log(f"Launching QEMU {arch} microkernel via Device Studio API...")
        try:
            req = urllib.request.Request(
                url,
                data=json.dumps({"profile": arch}).encode("utf-8"),
                headers={"Content-Type": "application/json"},
                method="POST"
            )
            with urllib.request.urlopen(req, timeout=10) as resp:
                data = json.loads(resp.read().decode("utf-8"))
                if data.get("success"):
                    log_success(f"{data.get('message')}")
                    log(f"Connect to serial logs: .\\interos logs -f")
                    log(f"Connect to serial shell: .\\interos shell")
                    return 0
                else:
                    log_error(f"Launch failed: {data.get('message')}")
                    return 1
        except Exception as e:
            log_error(f"Failed to communicate with Studio: {e}")
            return 1

    app_name = target_name
    if not _ensure_studio(port):
        return 1

    # The application path requires a live canonical x86 QEMU guest. Start one
    # only when Studio reports that no guest is currently active.
    try:
        with urllib.request.urlopen(
            urllib.request.Request(f"http://127.0.0.1:{port}/api/status", method="GET"),
            timeout=3,
        ) as resp:
            studio_status = json.loads(resp.read().decode("utf-8"))
        if not studio_status.get("qemu_alive", False):
            launch_req = urllib.request.Request(
                f"http://127.0.0.1:{port}/api/qemu/launch",
                data=json.dumps({"profile": "x86_64"}).encode("utf-8"),
                headers={"Content-Type": "application/json"},
                method="POST",
            )
            with urllib.request.urlopen(launch_req, timeout=10) as resp:
                launch_data = json.loads(resp.read().decode("utf-8"))
            if not launch_data.get("success"):
                log_error(f"Unable to start canonical QEMU guest: {launch_data.get('message')}")
                return 1
    except Exception as e:
        log_error(f"Unable to prepare QEMU guest: {e}")
        return 1

    # 1. Build
    build_args = argparse.Namespace(name=app_name, arch=target)
    ret = cmd_build(build_args)
    if ret != 0:
        return ret

    # 2. Package
    rapp_path = DIST_DIR / f"{app_name}.rapp"
    pack_args = argparse.Namespace(name=app_name, output=str(rapp_path), arch=("aarch64" if "aarch64" in target else ("riscv64" if "riscv64" in target else "x86_64")))
    ret = cmd_pack(pack_args)
    if ret != 0:
        return ret

    # 3. Deploy
    url = f"http://127.0.0.1:{port}/api/rapp/install"
    log(f"Deploying {rapp_path.name} to InterOS Device Studio at {url}...")
    try:
        data = rapp_path.read_bytes()
        # WHY the trust root is provisioned here: the guest OS trusts no
        # publisher until the owner of the device installs one, and deploying
        # an app to a local device is that owner's decision to trust the
        # publisher of it.
        trust_ok, trust_detail = _provision_guest_trust(data, "developer", port)
        if not trust_ok:
            log_error(f"The guest does not trust this publisher yet: {trust_detail}")
            return 1
        log(f"Publisher 'developer' trusted in the guest trust store: {trust_detail}")
        req = urllib.request.Request(
            url,
            data=data,
            headers={
                "Content-Type": "application/octet-stream",
                "X-Package-Name": rapp_path.name
            },
            method="POST"
        )
        with urllib.request.urlopen(req, timeout=5) as resp:
            body = resp.read().decode("utf-8")
            log_success(f"Installed to emulator: {body}")

        launch_url = f"http://127.0.0.1:{port}/api/rapp/launch"
        launch_req = urllib.request.Request(
            launch_url,
            data=json.dumps({"app_id": f"org.interos.{app_name}"}).encode("utf-8"),
            headers={"Content-Type": "application/json"},
            method="POST"
        )
        with urllib.request.urlopen(launch_req, timeout=5) as resp:
            log_success(f"App {app_name} launched on Device Studio display!")
    except urllib.error.URLError as e:
        log_error(f"Could not connect to InterOS Studio on port {port}: {e}")
        return 1
    return 0


# =============================================================================
# COMMAND: contracts
# =============================================================================
def cmd_contracts(args):
    """Run fast static system-contract checks without building the kernel."""
    test_file = WORKSPACE_ROOT / "tests" / "test_system_contracts.py"
    if not test_file.exists():
        log_error("System contract test file is missing.")
        return 1
    log("Running InterOS system contract checks...")
    result = subprocess.run([sys.executable, str(test_file)], cwd=str(WORKSPACE_ROOT))
    if result.returncode == 0:
        log_success("System contract checks passed.")
    else:
        log_error("System contract checks failed.")
    return result.returncode


# =============================================================================
# COMMAND: list, uninstall, install
# =============================================================================
def cmd_list(args):
    port = getattr(args, "port", 4300)
    url = f"http://127.0.0.1:{port}/api/rapp/list"
    log(f"Querying installed packages from {url}...")
    try:
        req = urllib.request.Request(url, method="GET")
        with urllib.request.urlopen(req, timeout=5) as resp:
            data = json.loads(resp.read().decode("utf-8"))
            apps = data.get("apps", [])
            print(f"\nFound {len(apps)} installed packages:")
            for a in apps:
                print(f"  Ã¢â‚¬Â¢ {a.get('name', 'Unknown')} ({a.get('id')}) v{a.get('version')} [{a.get('category')}]")
                print(f"    Permissions: {', '.join(a.get('permissions', []))}")
            print()
    except Exception as e:
        log_error(f"Failed to query emulator: {e}")
        return 1
    return 0


def cmd_uninstall(args):
    target = args.name
    port = getattr(args, "port", 4300)
    if not _ensure_studio(port):
        return 1
    url = f"http://127.0.0.1:{port}/api/apps/uninstall"
    payload = json.dumps({"app_id": target, "name": target}).encode("utf-8")
    req = urllib.request.Request(url, data=payload, headers={"Content-Type": "application/json"})
    try:
        with urllib.request.urlopen(req, timeout=5) as resp:
            data = json.loads(resp.read().decode("utf-8"))
            if data.get("success"):
                log_success(f"Package '{target}' uninstalled.")
                return 0
            else:
                log_error(f"Uninstallation failed: {data.get('error')}")
                return 1
    except Exception as e:
        log_error(f"Could not connect to emulator: {e}")
        return 1


def _provision_guest_trust(data, publisher, port):
    """Installs a publisher's public key into the guest's publisher trust store.

    WHY the guest needs this: the OS carries no publisher trust root of its
    own, so a package whose publisher the owner never installed is refused by
    the kernel. The owner of a local device is whoever drives the Studio
    console, and asking to install a package is that owner's decision to
    trust the publisher of it -- the same decision the `trust` command
    records on its own.

    Returns (ok, detail) so a caller can report what the guest said.
    """
    if len(data) < 0xB0 or data[:4] != b"RAPP":
        return False, "Invalid RAPP package"
    public_key = data[0x28:0x48].hex()
    url = f"http://127.0.0.1:{port}/api/rapp/trust"
    payload = json.dumps({"public_key": public_key, "publisher": publisher}).encode("utf-8")
    req = urllib.request.Request(url, data=payload, headers={"Content-Type": "application/json"}, method="POST")
    try:
        with urllib.request.urlopen(req, timeout=15) as resp:
            result = json.loads(resp.read().decode("utf-8"))
    except Exception as e:
        return False, str(e)
    if result.get("success"):
        return True, public_key
    return False, result.get("message", result.get("error", "unknown error"))


def cmd_install(args):
    pkg_path = Path(args.package_path)
    if not pkg_path.exists():
        log_error(f"Package file not found: {pkg_path}")
        return 1

    # Verify first
    ver_ret = cmd_verify(args)
    if ver_ret != 0:
        log_error("Package installation aborted due to signature verification failure.")
        return ver_ret

    port = getattr(args, "port", 4300)
    if not _ensure_studio(port):
        return 1
    filename = pkg_path.name
    data = pkg_path.read_bytes()
    publisher = getattr(args, "publisher", "developer")
    trust_ok, trust_detail = _provision_guest_trust(data, publisher, port)
    if not trust_ok:
        log_error(f"The guest does not trust this publisher yet: {trust_detail}")
        log_error(f"Provision it with 'interos.py trust {filename}' and install again.")
        return 1
    log(f"Publisher '{publisher}' trusted in the guest trust store: {trust_detail}")
    log(f"Uploading verified package '{filename}' to InterOS Studio (port {port})...")
    url = f"http://127.0.0.1:{port}/api/rapp/install"
    req = urllib.request.Request(
        url,
        data=data,
        headers={
            "Content-Type": "application/octet-stream",
            "X-Package-Name": filename
        },
        method="POST"
    )
    try:
        with urllib.request.urlopen(req, timeout=10) as resp:
            log_success(f"Package '{filename}' installed successfully into guest sandbox!")
            return 0
    except Exception as e:
        log_error(f"Installation failed: {e}")
        return 1


def cmd_trust(args):
    pkg_path = Path(args.package_path)
    if not pkg_path.exists():
        log_error(f"Package file not found: {pkg_path}")
        return 1
    data = pkg_path.read_bytes()
    publisher = getattr(args, "publisher", "developer")
    port = getattr(args, "port", 4300)
    if not _ensure_studio(port):
        return 1
    ok, detail = _provision_guest_trust(data, publisher, port)
    if ok:
        log_success(f"Publisher trust root provisioned in QEMU guest: {detail}")
        return 0
    log_error(f"Guest trust provisioning failed: {detail}")
    return 1


# =============================================================================
# COMMANDS: sign, stop, restart, logs, shell, snapshot, trace
# =============================================================================
def cmd_sign(args):
    """Cryptographically signs an arbitrary file or .rapp with Ed25519."""
    target_path = Path(args.target)
    if not target_path.exists():
        log_error(f"Target file not found: {target_path}")
        return 1

    try:
        from cryptography.hazmat.primitives.asymmetric import ed25519
        from cryptography.hazmat.primitives import serialization
    except ImportError:
        log_error("Python 'cryptography' library required. Run: pip install cryptography")
        return 1

    key_path = Path(args.key) if getattr(args, "key", None) else (PACKAGES_DIR / "developer.key")
    if key_path.exists():
        priv_key = ed25519.Ed25519PrivateKey.from_private_bytes(key_path.read_bytes())
    else:
        log(f"Generating new developer Ed25519 key at {key_path}...")
        key_path.parent.mkdir(parents=True, exist_ok=True)
        priv_key = ed25519.Ed25519PrivateKey.generate()
        key_path.write_bytes(priv_key.private_bytes(
            encoding=serialization.Encoding.Raw,
            format=serialization.PrivateFormat.Raw,
            encryption_algorithm=serialization.NoEncryption()
        ))

    file_bytes = target_path.read_bytes()
    file_sha = hashlib.sha256(file_bytes).hexdigest()
    sig_bytes = priv_key.sign(file_sha.encode("utf-8"))
    sig_out = target_path.with_suffix(target_path.suffix + ".sig")
    sig_doc = {
        "file": target_path.name,
        "sha256": file_sha,
        "public_key": priv_key.public_key().public_bytes(
            encoding=serialization.Encoding.Raw,
            format=serialization.PublicFormat.Raw
        ).hex(),
        "signature": sig_bytes.hex()
    }
    sig_out.write_text(json.dumps(sig_doc, indent=2), encoding="utf-8")
    log_success(f"Signature written to {sig_out}")
    log(f"  Ã¢â‚¬Â¢ SHA-256: {file_sha}")
    log(f"  Ã¢â‚¬Â¢ Ed25519: {sig_bytes.hex()[:24]}...")
    return 0


def cmd_stop(args):
    """Stops the running QEMU microkernel instance."""
    port = getattr(args, "port", 4300)
    url = f"http://127.0.0.1:{port}/api/qemu/stop"
    log(f"Sending stop signal to QEMU via Studio (port {port})...")
    try:
        req = urllib.request.Request(url, data=b"{}", headers={"Content-Type": "application/json"}, method="POST")
        with urllib.request.urlopen(req, timeout=5) as resp:
            data = json.loads(resp.read().decode("utf-8"))
            if data.get("success"):
                log_success("QEMU microkernel process terminated successfully.")
                return 0
    except Exception as e:
        log_warn(f"Studio API unreachable ({e}). QEMU process must be stopped via its owning session.")
        return 1
    return 1


def cmd_restart(args):
    """Triggers hardware reset or warm restart of the microkernel."""
    port = getattr(args, "port", 4300)
    url = f"http://127.0.0.1:{port}/api/qemu/reset"
    log(f"Dispatching hardware reset signal to QEMU (port {port})...")
    try:
        req = urllib.request.Request(url, data=b"{}", headers={"Content-Type": "application/json"}, method="POST")
        with urllib.request.urlopen(req, timeout=5) as resp:
            log_success("System reset signal acknowledged by QEMU microkernel.")
            return 0
    except Exception as e:
        log_error(f"Failed to reset: {e}")
        return 1


def cmd_logs(args):
    """Displays or follows live serial UART logs."""
    port = getattr(args, "port", 4300)
    follow = getattr(args, "follow", False)
    after = 0
    log("Connecting to live serial log stream...")
    try:
        while True:
            url = f"http://127.0.0.1:{port}/api/serial?after={after}"
            try:
                with urllib.request.urlopen(url, timeout=3) as resp:
                    data = json.loads(resp.read().decode("utf-8"))
                    lines = data.get("lines", [])
                    for l in lines:
                        print(l)
                    after = data.get("next_index", after + len(lines))
            except Exception:
                pass
            if not follow:
                break
            time.sleep(0.5)
        return 0
    except KeyboardInterrupt:
        return 0


def cmd_shell(args):
    """Opens an interactive serial shell session to the microkernel."""
    port = getattr(args, "port", 4301)
    host = "127.0.0.1"
    log(f"Connecting to guest serial UART at {host}:{port}...")
    try:
        with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as s:
            s.connect((host, port))
            log_success("Connected to InterOS Serial UART. Press Ctrl+C to exit.")
            s.setblocking(False)
            while True:
                try:
                    data = s.recv(1024)
                    if data:
                        sys.stdout.write(data.decode("utf-8", errors="replace"))
                        sys.stdout.flush()
                except BlockingIOError:
                    pass
                time.sleep(0.05)
    except ConnectionRefusedError:
        log_error(f"Cannot connect to serial console at {host}:{port}. Is QEMU running with serial TCP active?")
        return 1
    except KeyboardInterrupt:
        print("\n[Shell disconnected]")
        return 0


def cmd_snapshot(args):
    """Manages QEMU snapshots (save, load, list)."""
    port = getattr(args, "port", 4300)
    action = getattr(args, "action", "list")
    name = getattr(args, "name", "snap1")
    url = f"http://127.0.0.1:{port}/api/qemu/snapshot"
    log(f"Dispatching snapshot action '{action}' (name='{name}')...")
    payload = json.dumps({"action": action, "name": name}).encode("utf-8")
    try:
        req = urllib.request.Request(url, data=payload, headers={"Content-Type": "application/json"}, method="POST")
        with urllib.request.urlopen(req, timeout=10) as resp:
            data = json.loads(resp.read().decode("utf-8"))
            out = data.get("output", "")
            if out:
                print(out)
            log_success(f"Snapshot operation '{action}' completed.")
            return 0
    except Exception as e:
        log_error(f"Snapshot command failed: {e}")
        return 1


def cmd_trace(args):
    """Queries and displays live guest telemetry and execution trace."""
    port = getattr(args, "port", 4300)
    url = f"http://127.0.0.1:{port}/api/qemu/telemetry"
    try:
        with urllib.request.urlopen(url, timeout=3) as resp:
            data = json.loads(resp.read().decode("utf-8"))
            print("================================================================")
            print("  InterOS Guest Execution Trace & Telemetry")
            print("================================================================")
            print(f"Status:      {data.get('status')} (Mode: {data.get('mode')})")
            print(f"QEMU Alive:  {data.get('qemu_alive')}")
            cpu = data.get("cpu", {})
            print(f"CPU Arch:    {cpu.get('arch')} (Thread: {cpu.get('thread_id')}, Halted: {cpu.get('halted')})")
            print(f"RIP/PC:      {cpu.get('rip')}")
            regs = cpu.get("registers", {})
            if regs:
                print("\nRegisters:")
                for k, v in sorted(regs.items()):
                    print(f"  {k:8s} = {v}")
            else:
                print("\nRegisters:   (Offline / Standby)")
            print("================================================================")
            return 0
    except Exception as e:
        log_error(f"Could not connect to Studio telemetry API: {e}")
        return 1


def main():
    parser = argparse.ArgumentParser(
        description="InterOS Core Developer CLI (interos)",
        formatter_class=argparse.RawDescriptionHelpFormatter
    )
    subparsers = parser.add_subparsers(dest="command", help="Command to run")

    # doctor
    p_doc = subparsers.add_parser("doctor", help="Inspect and validate development environment")
    p_doc.add_argument("--port", type=int, default=4300, help="Device Studio port")
    p_doc.set_defaults(func=cmd_doctor)

    # build
    p_bld = subparsers.add_parser("build", help="Build the canonical QEMU demo, kernels, or an application")
    p_bld.add_argument("name", nargs="?", default="all", help="demo/qemu, architecture, or app name")
    p_bld.add_argument("--target", default=None, help="Target architecture or triple")
    p_bld.add_argument("--arch", default=None, help="Application Rust target triple")
    p_bld.set_defaults(func=cmd_build)

    # test
    p_tst = subparsers.add_parser("test", help="Execute automated test suite across targets")
    p_tst.add_argument("--target", choices=["unit", "kernel", "x86_64", "x86", "aarch64", "riscv64", "security", "adversarial", "qemu", "lifecycle", "contracts", "m10_m12", "stress", "recovery", "persistence", "storage_recovery", "crash", "watchdog", "panic", "all"], default="all", help="Select a specific local test suite")
    p_tst.set_defaults(func=cmd_test)

    # new
    p_new = subparsers.add_parser("new", help="Scaffold a new .rapp project")
    p_new.add_argument("name", help="App name")
    p_new.add_argument("--template", choices=list(TEMPLATES.keys()), default="basic", help="App template")
    p_new.set_defaults(func=cmd_new)

    # package (pack)
    p_pack = subparsers.add_parser("package", aliases=["pack"], help="Bundle and sign app into .rapp package")
    p_pack.add_argument("name", help="App name")
    p_pack.add_argument("--output", help="Output file path (default: dist/<name>.rapp)")
    p_pack.add_argument("--arch", choices=["x86_64", "aarch64", "riscv64"], default=None, help="RAPP target architecture")
    p_pack.set_defaults(func=cmd_pack)

    # verify
    p_ver = subparsers.add_parser("verify", help="Cryptographically verify a .rapp package")
    p_ver.add_argument("package_path", help="Path to .rapp package")
    p_ver.set_defaults(func=cmd_verify)

    # contracts
    p_contracts = subparsers.add_parser("contracts", help="Run fast static system contract checks")
    p_contracts.set_defaults(func=cmd_contracts)

    # reality
    p_real = subparsers.add_parser("reality", help="Report implementation and hardware verification reality")
    p_real.set_defaults(func=cmd_reality)

    # devices
    p_dev = subparsers.add_parser("devices", help="List supported target device profiles")
    p_dev.set_defaults(func=cmd_devices)

    # inspect
    p_insp = subparsers.add_parser("inspect", help="Inspect .rapp package or running Studio state")
    p_insp.add_argument("target", nargs="?", default="", help="Package path or target")
    p_insp.add_argument("--port", type=int, default=4300, help="Studio port")
    p_insp.set_defaults(func=cmd_inspect)

    # run
    p_run = subparsers.add_parser("run", help="Build and launch the canonical QEMU demo or deploy an application")
    p_run.add_argument("name", help="demo/qemu or app name")
    p_run.add_argument("--target", default="x86_64-unknown-none", help="Application target triple")
    p_run.add_argument("--port", type=int, default=4300, help="Studio port")
    p_run.set_defaults(func=cmd_run)

    # list
    p_lst = subparsers.add_parser("list", help="List installed packages on emulator")
    p_lst.add_argument("--port", type=int, default=4300, help="Studio port")
    p_lst.set_defaults(func=cmd_list)

    # trust
    p_trust = subparsers.add_parser("trust", help="Explicitly provision an RAPP publisher key into the live QEMU developer guest")
    p_trust.add_argument("package_path", help="RAPP package whose publisher key should be trusted")
    p_trust.add_argument("--publisher", default="developer", help="Publisher label stored with the trust root")
    p_trust.add_argument("--port", type=int, default=4300, help="Studio port")
    p_trust.set_defaults(func=cmd_trust)

    # install
    p_inst = subparsers.add_parser("install", help="Verify and install package onto emulator")
    p_inst.add_argument("package_path", help="Path to package")
    p_inst.add_argument("--publisher", default="developer", help="Publisher label stored with the trust root")
    p_inst.add_argument("--port", type=int, default=4300, help="Studio port")
    p_inst.set_defaults(func=cmd_install)

    # uninstall
    p_un = subparsers.add_parser("uninstall", help="Uninstall package from emulator")
    p_un.add_argument("name", help="App ID or name")
    p_un.add_argument("--port", type=int, default=4300, help="Studio port")
    p_un.set_defaults(func=cmd_uninstall)

    # sign
    p_sgn = subparsers.add_parser("sign", help="Sign a file or package with Ed25519")
    p_sgn.add_argument("target", help="File or package to sign")
    p_sgn.add_argument("--key", help="Path to Ed25519 private key")
    p_sgn.set_defaults(func=cmd_sign)

    # stop
    p_stp = subparsers.add_parser("stop", help="Stop running QEMU microkernel")
    p_stp.add_argument("--port", type=int, default=4300, help="Studio port")
    p_stp.set_defaults(func=cmd_stop)

    # restart
    p_rst = subparsers.add_parser("restart", help="Reset/restart QEMU microkernel")
    p_rst.add_argument("--port", type=int, default=4300, help="Studio port")
    p_rst.set_defaults(func=cmd_restart)

    # logs
    p_log = subparsers.add_parser("logs", help="Display or follow live serial UART logs")
    p_log.add_argument("--port", type=int, default=4300, help="Studio port")
    p_log.add_argument("-f", "--follow", action="store_true", help="Follow log stream continuously")
    p_log.set_defaults(func=cmd_logs)

    # shell
    p_shl = subparsers.add_parser("shell", help="Connect to microkernel serial UART console")
    p_shl.add_argument("--port", type=int, default=4301, help="Serial TCP port")
    p_shl.set_defaults(func=cmd_shell)

    # snapshot
    p_snp = subparsers.add_parser("snapshot", help="Manage QEMU snapshots (save, load, list)")
    p_snp.add_argument("--action", choices=["save", "load", "list", "delete"], default="list", help="Action")
    p_snp.add_argument("--name", default="snap1", help="Snapshot name")
    p_snp.add_argument("--port", type=int, default=4300, help="Studio port")
    p_snp.set_defaults(func=cmd_snapshot)

    # trace
    p_trc = subparsers.add_parser("trace", help="Display real-time CPU registers and guest telemetry")
    p_trc.add_argument("--port", type=int, default=4300, help="Studio port")
    p_trc.set_defaults(func=cmd_trace)

    args = parser.parse_args()
    if not hasattr(args, "func"):
        parser.print_help()
        return 1

    return args.func(args)


if __name__ == "__main__":
    sys.exit(main())
