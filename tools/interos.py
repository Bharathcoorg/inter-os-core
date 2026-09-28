#!/usr/bin/env python3
"""
InterOS Core Developer CLI (interos)
=====================================
Unified CLI tool for building, packaging, verifying, testing, and hot-deploying
InterOS Core kernels and sovereign Rust Mobile Application Packages (.rapp).

Usage:
  python tools/interos.py doctor
  python tools/interos.py build [--target x86_64|aarch64|riscv64|all|<app_name>]
  python tools/interos.py test [--target x86_64|aarch64|riscv64|unit]
  python tools/interos.py new <app_name> [--template basic|authenticator|sensor|meshchat]
  python tools/interos.py package <app_name> [--output dist/<app_name>.rapp]
  python tools/interos.py sign <package_or_file> [--key <key_path>]
  python tools/interos.py verify <package.rapp>
  python tools/interos.py install <package_path> [--port 4300]
  python tools/interos.py run <app_name> [--target aarch64-unknown-none] [--port 4300]
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
    # High-level template and manifest permission aliases
    "CAP_WINDOW_FOCUS": 17,
    "CAP_HARDWARE_KEYSTORE": 18,
    "CAP_SECURE_STORAGE": 10,
    "CAP_SENSOR_ACCEL": 4,
    "CAP_SENSOR_GYRO": 3,
    "CAP_SENSOR_HR": 17,
    "CAP_MESH_RADIO": 12,
    "CAP_P2P_SOCKET": 13,
}

TEMPLATES = {
    "basic": {
        "title": "Basic App",
        "description": "Standard #![no_std] Rust mobile app with UI frontend",
        "permissions": ["CAP_LOGGING", "CAP_WINDOW_FOCUS"],
    },
    "authenticator": {
        "title": "Keystore Authenticator",
        "description": "Cryptographic TOTP authenticator backed by Ring 0 hardware enclave",
        "permissions": ["CAP_HARDWARE_KEYSTORE", "CAP_SECURE_STORAGE"],
    },
    "sensor": {
        "title": "Sensor Telemetry Hub",
        "description": "Real-time IMU, accelerometer, and biometrics monitor",
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
        Path("C:/Users/bhara/qemu"),
        Path("C:/Program Files/qemu"),
    ]
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
        "cargo", "build",
        "--manifest-path", str(WORKSPACE_ROOT / manifest_path),
        "--target", target,
    ]
    if bin_name:
        cmd.extend(["--bin", bin_name])
    log(f"Building {bin_name or manifest_path} [{target}]...")
    return subprocess.run(cmd, cwd=str(WORKSPACE_ROOT)).returncode


def _build_x86_userspace():
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
    res = subprocess.run([
        "cargo", "build",
        "--manifest-path", "kernel/Cargo.toml",
        "--target", "x86_64-unknown-none"
    ], cwd=str(WORKSPACE_ROOT))
    if res.returncode != 0:
        log_error("x86_64 kernel build failed")
        return res.returncode

    kernel_path = WORKSPACE_ROOT / "target" / "x86_64-unknown-none" / "debug" / "kernel"
    if not kernel_path.exists():
        log_error(f"Kernel artifact missing after successful cargo build: {kernel_path}")
        return 1

    log("Packaging canonical QEMU Phone BIOS image...")
    res = subprocess.run([
        "cargo", "run",
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
        res = subprocess.run([
            "cargo", "build", "--manifest-path", str(app_dir / "Cargo.toml"),
            "--target", arch_target
        ], cwd=str(WORKSPACE_ROOT))
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
                "cargo", "build", "--manifest-path", "kernel-aarch64/Cargo.toml",
                "--target", "aarch64-unknown-none"
            ], cwd=str(WORKSPACE_ROOT))
            if res.returncode != 0:
                log_error("aarch64 kernel build failed")
                return res.returncode
        elif t == "riscv64":
            log("Building InterOS Kernel [riscv64gc-unknown-none-elf]...")
            res = subprocess.run([
                "cargo", "build", "--manifest-path", "kernel-riscv64/Cargo.toml",
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
def cmd_test(args):
    test_target = getattr(args, "target", "all").lower()
    log(f"Executing InterOS test suite (Target: {test_target})...")

    if test_target in ["unit", "all"]:
        log("Running cargo test on host-compatible crates...")
        res = subprocess.run(["cargo", "test", "-p", "interos-std"], cwd=str(WORKSPACE_ROOT))
        if res.returncode != 0 and test_target == "unit":
            return res.returncode
        if (WORKSPACE_ROOT / "boot" / "Cargo.toml").exists():
            subprocess.run(["cargo", "test", "--manifest-path", "boot/Cargo.toml"], cwd=str(WORKSPACE_ROOT))

    if test_target in ["x86_64", "x86", "all"]:
        x86_script = WORKSPACE_ROOT / "tests" / "verify_x86_boot.ps1"
        if x86_script.exists() and sys.platform == "win32":
            log("Executing QEMU x86_64 boot and serial verification...")
            res = subprocess.run(["powershell", "-ExecutionPolicy", "Bypass", "-File", str(x86_script)])
            if res.returncode != 0 and test_target == "x86_64":
                return res.returncode

    if test_target in ["aarch64", "all"]:
        arm_script = WORKSPACE_ROOT / "tests" / "test_aarch64.ps1"
        if arm_script.exists() and sys.platform == "win32":
            log("Executing QEMU AArch64 boot verification...")
            res = subprocess.run(["powershell", "-ExecutionPolicy", "Bypass", "-File", str(arm_script)])
            if res.returncode != 0 and test_target == "aarch64":
                return res.returncode

    if test_target in ["riscv64", "all"]:
        riscv_script = WORKSPACE_ROOT / "tests" / "test_riscv64.ps1"
        if riscv_script.exists() and sys.platform == "win32":
            log("Executing QEMU RISC-V boot verification...")
            res = subprocess.run(["powershell", "-ExecutionPolicy", "Bypass", "-File", str(riscv_script)])
            if res.returncode != 0 and test_target == "riscv64":
                return res.returncode

    if test_target in ["security", "adversarial", "all"]:
        sec_script = WORKSPACE_ROOT / "tests" / "test_adversarial_security.py"
        if sec_script.exists():
            log("Executing Zero-Trust Adversarial Security Verification Suite...")
            res = subprocess.run([sys.executable, str(sec_script)])
            if res.returncode != 0:
                log_error("Adversarial security test suite failed!")
                return res.returncode
            log_success("All adversarial security invariants validated.")

    log_success("Test execution finished.")
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
        log_error("No ELF payload found in app/bin")
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
        unsigned = _rapp_v2_encode(manifest_data, pub, b"\0" * 64, payload, getattr(args, "arch", None))
        signature = priv_key.sign(_rapp_v2_signing_bytes(unsigned))
        package_bytes = _rapp_v2_encode(manifest_data, pub, signature, payload, getattr(args, "arch", None))
    except Exception as e:
        log_error(f"RAPP v2 signing failed: {e}")
        return 1
    DIST_DIR.mkdir(parents=True, exist_ok=True)
    output = Path(getattr(args, "output", None) or DIST_DIR / f"{app_name}.rapp")
    output.parent.mkdir(parents=True, exist_ok=True)
    output.write_bytes(package_bytes)
    log_success(f"RAPP v2 package created: {output} ({len(package_bytes)} bytes)")
    log(f"  • Publisher key: {pub.hex()}")
    log(f"  • Ed25519 signature: {signature.hex()[:24]}...")
    log(f"  • Architecture: {manifest_data.get('architecture', 'x86_64')}")
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
    log(f"  • Publisher key: {pub.hex()}")
    log(f"  • Version: {version[0]}.{version[1]}.{version[2]}")
    log(f"  • Payload: {payload_len} bytes")
    log("  • Device trust: must be provisioned by the kernel trust store")
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
    target = args.target
    p = Path(target)
    if p.exists() and p.suffix == ".rapp":
        return cmd_verify(argparse.Namespace(package_path=str(p)))

    # Otherwise query running studio
    port = getattr(args, "port", 4300)
    url = f"http://127.0.0.1:{port}/api/status"
    log(f"Inspecting running InterOS Studio instance at {url}...")
    try:
        req = urllib.request.Request(url, method="GET")
        with urllib.request.urlopen(req, timeout=3) as resp:
            data = json.loads(resp.read().decode("utf-8"))
            print(json.dumps(data, indent=2))
            return 0
    except Exception as e:
        log_error(f"Failed to query Studio: {e}")
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
    test_file = ROOT_DIR / "tests" / "test_system_contracts.py"
    if not test_file.exists():
        log_error("System contract test file is missing.")
        return 1
    log("Running InterOS system contract checks...")
    result = subprocess.run([sys.executable, str(test_file)], cwd=str(ROOT_DIR))
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
                print(f"  • {a.get('name', 'Unknown')} ({a.get('id')}) v{a.get('version')} [{a.get('category')}]")
                print(f"    Permissions: {', '.join(a.get('permissions', []))}")
            print()
    except Exception as e:
        log_error(f"Failed to query emulator: {e}")
        return 1
    return 0


def cmd_uninstall(args):
    target = args.name
    port = getattr(args, "port", 4300)
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
    filename = pkg_path.name
    log(f"Uploading verified package '{filename}' to InterOS Studio (port {port})...")
    data = pkg_path.read_bytes()
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
    if len(data) < 0xB0 or data[:4] != b"RAPP":
        log_error("Invalid RAPP package")
        return 1
    public_key = data[0x28:0x48].hex()
    publisher = getattr(args, "publisher", "developer")
    port = getattr(args, "port", 4300)
    if not _ensure_studio(port):
        return 1
    url = f"http://127.0.0.1:{port}/api/rapp/trust"
    payload = json.dumps({"public_key": public_key, "publisher": publisher}).encode("utf-8")
    req = urllib.request.Request(url, data=payload, headers={"Content-Type": "application/json"}, method="POST")
    try:
        with urllib.request.urlopen(req, timeout=15) as resp:
            result = json.loads(resp.read().decode("utf-8"))
        if result.get("success"):
            log_success(f"Publisher trust root provisioned in QEMU guest: {public_key}")
            return 0
        log_error(f"Guest trust provisioning failed: {result.get('message', result.get('error'))}")
        return 1
    except Exception as e:
        log_error(f"Could not provision guest trust root: {e}")
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
    log(f"  • SHA-256: {file_sha}")
    log(f"  • Ed25519: {sig_bytes.hex()[:24]}...")
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
        log_warn(f"Studio API unreachable ({e}). Attempting direct process termination...")
        if sys.platform == "win32":
            subprocess.run(["taskkill", "/F", "/IM", "qemu-system-*"], capture_output=True)
        else:
            subprocess.run(["pkill", "-f", "qemu-system"], capture_output=True)
        log_success("Terminated local QEMU processes.")
        return 0
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
    p_tst.add_argument("--target", default="all", help="Target: x86_64, aarch64, riscv64, unit, all")
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
