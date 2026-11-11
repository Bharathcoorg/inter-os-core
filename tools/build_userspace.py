#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

"""
Build, verify and strip all 28 Ring-3 userspace and app binaries across
supported CPU target architectures (x86_64, aarch64, riscv64).

Entry base: 0x00400000 (fixed static relocation)
Binary format: 64-bit ELF, ET_EXEC, no symbol table
"""

import argparse
import glob
import json
import os
import shutil
import struct
import subprocess
import sys
from pathlib import Path

WORKSPACE_ROOT = Path(__file__).resolve().parent.parent

ARCH_TARGET_MAP = {
    "x86_64": {
        "triple": "x86_64-unknown-none",
        "e_machine": 0x3E, # EM_X86_64
        "platform_dir": "platform/x86_64",
    },
    "aarch64": {
        "triple": "aarch64-unknown-none",
        "e_machine": 0xB7, # EM_AARCH64
        "platform_dir": "platform/aarch64",
    },
    "riscv64": {
        "triple": "riscv64gc-unknown-none-elf",
        "e_machine": 0xF3, # EM_RISCV
        "platform_dir": "platform/riscv64",
    },
}

SERVICES = [
    "displayd", "inputd", "storaged", "netd", "powerd",
    "modemd", "audiod", "camerad", "sensord", "aid"
]

APPS = [
    "compositor", "launcher", "settings", "dialer", "vault",
    "camera", "calculator", "browser", "messages", "calendar",
    "music", "file_manager", "gallery", "agent_manager", "notes",
    "video_player", "webview_demo"
]

STATIC_FLAGS = [
    "-C", "relocation-model=static",
    "-C", "link-arg=-Ttext=0x00400000",
    "-C", "link-arg=-no-pie",
    "-C", "opt-level=3",
]


def resolve_strip_tool() -> Path:
    """Find llvm-strip from rustc sysroot or system PATH."""
    try:
        r = subprocess.run(["rustc", "--print", "sysroot"], capture_output=True, text=True, check=True)
        sysroot = Path(r.stdout.strip())
        pattern = "llvm-strip.exe" if sys.platform == "win32" else "llvm-strip"
        matches = list(sysroot.glob(f"lib/rustlib/**/{pattern}"))
        if matches:
            return matches[0]
    except Exception:
        pass

    on_path = shutil.which("llvm-strip")
    if on_path:
        return Path(on_path)

    raise RuntimeError("llvm-strip was not found in rustc sysroot or system PATH")


def assert_executable_elf(path: Path, expected_machine: int):
    """Assert binary is a valid 64-bit ET_EXEC ELF for the expected machine architecture."""
    data = path.read_bytes()
    if len(data) < 64:
        raise ValueError(f"{path} is too small to be an ELF header ({len(data)} bytes)")

    if data[:4] != b"\x7fELF":
        raise ValueError(f"{path} does not start with ELF magic")

    ei_class = data[4]
    if ei_class != 2:
        raise ValueError(f"{path} is not 64-bit ELF (class {ei_class})")

    e_type, e_machine = struct.unpack_from("<HH", data, 16)
    if e_type != 2: # ET_EXEC
        kinds = {1: "ET_REL", 2: "ET_EXEC", 3: "ET_DYN"}
        raise ValueError(f"{path} has ELF type {kinds.get(e_type, str(e_type))}, expected ET_EXEC")

    if e_machine != expected_machine:
        raise ValueError(f"{path} has e_machine 0x{e_machine:X}, expected 0x{expected_machine:X}")


def assert_no_symbol_table(path: Path):
    """Assert that .symtab and .strtab have been completely stripped."""
    data = path.read_bytes()
    if len(data) < 64:
        raise ValueError(f"{path} too small for ELF header")

    e_shoff, = struct.unpack_from("<Q", data, 40)
    e_shentsize, e_shnum, e_shstrndx = struct.unpack_from("<HHH", data, 58)

    if e_shoff == 0 or e_shnum == 0:
        return # No section header table at all -> stripped

    if (e_shoff + e_shnum * e_shentsize) > len(data):
        raise ValueError(f"{path} section header table exceeds file size")

    if e_shstrndx >= e_shnum:
        raise ValueError(f"{path} e_shstrndx {e_shstrndx} >= e_shnum {e_shnum}")

    str_hdr = e_shoff + (e_shstrndx * e_shentsize)
    str_offset, str_size = struct.unpack_from("<QQ", data, str_hdr + 24)

    if str_offset + str_size > len(data):
        raise ValueError(f"{path} section name table exceeds file size")

    names = []
    for i in range(e_shnum):
        hdr = e_shoff + (i * e_shentsize)
        name_offset, = struct.unpack_from("<I", data, hdr)
        if name_offset < str_size:
            end = data.find(b"\x00", str_offset + name_offset)
            if end != -1:
                name = data[str_offset + name_offset:end].decode("ascii", errors="ignore")
                if name in (".symtab", ".strtab"):
                    names.append(name)

    if names:
        raise ValueError(f"{path} still contains symbol table sections: {names}")


def build_binary(pkg: str, bin_name: str, target_triple: str, output_dir: Path, strip_tool: Path, expected_machine: int):
    """Build, verify and strip one Ring-3 binary."""
    args = ["cargo", "rustc", "-p", pkg]
    # For init, there is no separate lib, but passing --bin init is safe or we omit for init if not declared
    if pkg != "init":
        args += ["--bin", bin_name]
    args += ["--target", target_triple, "--"] + STATIC_FLAGS

    print(f"[*] Building {bin_name} ({target_triple})...")
    res = subprocess.run(args, cwd=WORKSPACE_ROOT)
    if res.returncode != 0:
        raise RuntimeError(f"Cargo build failed for {bin_name} on {target_triple} with code {res.returncode}")

    target_path = output_dir / bin_name
    if sys.platform == "win32" and not target_path.exists():
        # Target might or might not have .exe depending on cargo target config
        win_path = output_dir / f"{bin_name}.exe"
        if win_path.exists():
            target_path = win_path

    if not target_path.exists():
        raise FileNotFoundError(f"Expected output {target_path} does not exist")

    assert_executable_elf(target_path, expected_machine)

    strip_res = subprocess.run(
        [str(strip_tool), "--strip-debug", "--strip-unneeded", str(target_path)],
        capture_output=True, text=True
    )
    if strip_res.returncode != 0:
        raise RuntimeError(f"llvm-strip failed on {target_path}: {strip_res.stderr}")

    assert_executable_elf(target_path, expected_machine)
    assert_no_symbol_table(target_path)
    size_kb = target_path.stat().st_size / 1024
    print(f"  -> ET_EXEC verified and stripped: {bin_name} ({size_kb:.1f} KB)")


def generate_manifest(arch: str, info: dict):
    """Generate or update platform/<arch>/userspace-manifest.json."""
    manifest_path = WORKSPACE_ROOT / info["platform_dir"] / "userspace-manifest.json"
    manifest_path.parent.mkdir(parents=True, exist_ok=True)

    required = [
        {"name": "init", "kind": "binary", "manifest": "userspace/init/Cargo.toml"}
    ]
    for svc in SERVICES:
        required.append({
            "name": svc,
            "kind": "system-service",
            "manifest": "userspace/system-service/Cargo.toml"
        })
    for app in APPS:
        required.append({
            "name": app,
            "kind": "application",
            "manifest": f"apps/{app}/Cargo.toml"
        })

    data = {
        "schema_version": 1,
        "platform": arch,
        "profile": "qemu-phone",
        "target_triple": info["triple"],
        "output_root": f"target/{info['triple']}/debug",
        "required": required,
        "kernel": {
            "manifest": f"kernel-{arch}/Cargo.toml" if arch != "x86_64" else "kernel/Cargo.toml",
            "target": info["triple"]
        }
    }

    manifest_path.write_text(json.dumps(data, indent=2) + "\n", encoding="utf-8")
    print(f"[OK] Generated manifest at {manifest_path.relative_to(WORKSPACE_ROOT)}")


def build_for_arch(arch: str):
    """Build all 28 userspace binaries for the specified architecture."""
    info = ARCH_TARGET_MAP[arch]
    triple = info["triple"]
    machine = info["e_machine"]
    out_dir = WORKSPACE_ROOT / "target" / triple / "debug"
    out_dir.mkdir(parents=True, exist_ok=True)

    strip_tool = resolve_strip_tool()
    print("=" * 60)
    print(f" Building Userspace for {arch} ({triple}) ")
    print("=" * 60)

    # 1. Primordial init
    build_binary("init", "init", triple, out_dir, strip_tool, machine)

    # 2. System services (10 services)
    for svc in SERVICES:
        build_binary("interos-system-service", svc, triple, out_dir, strip_tool, machine)

    # 3. UI apps (17 apps)
    for app in APPS:
        build_binary(app, app, triple, out_dir, strip_tool, machine)

    # 4. Generate manifest
    generate_manifest(arch, info)
    print(f"[SUCCESS] All 28 Ring-3 binaries built, stripped, and verified for {arch}.\n")


def main():
    parser = argparse.ArgumentParser(description="Multi-architecture Userspace Builder for InterOS")
    parser.add_argument("--arch", choices=["x86_64", "aarch64", "riscv64", "all"], default="all",
                        help="Target architecture to compile (default: all)")
    args = parser.parse_args()

    archs = ["x86_64", "aarch64", "riscv64"] if args.arch == "all" else [args.arch]
    for arch in archs:
        build_for_arch(arch)

    print("[ALL ARCHITECTURES COMPLETE] Multi-architecture userspace build succeeded.")


if __name__ == "__main__":
    main()
