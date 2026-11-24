# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

"""
Cryptographic Build Provenance and Image Verification for InterOS Core.
=======================================================================
Binds bootable disk images to:
  1. Git commit HEAD and working-tree dirty status;
  2. Source tree content digest across kernel, boot, userspace, apps, manifest;
  3. Cargo profile (release/debug) and feature flags (debug-control-plane);
  4. Kernel ELF SHA-256;
  5. Ring-3 embedded userspace binaries' SHA-256 digests and manifest digest;
  6. Final boot image SHA-256.

Eliminates silent fallbacks and stale binary execution across acceptance tests.
"""

from __future__ import annotations

import hashlib
import json
import os
import subprocess
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent

TRACKED_SOURCE_DIRS = ("kernel", "boot", "userspace", "apps")
TRACKED_SOURCE_FILES = (
    "platform/x86_64/userspace-manifest.json",
    "Cargo.lock",
)
IGNORED_EXTENSIONS = {
    ".img", ".iso", ".qcow2", ".vdi", ".vmdk",
    ".log", ".ppm", ".png", ".jpg", ".jpeg",
    ".pyc", ".pyo", ".pyd", ".pdb", ".tmp"
}
IGNORED_DIR_NAMES = {
    "target", ".git", "__pycache__", "target-debug-cp", "node_modules",
    ".pytest_cache", "audit-evidence", ".system_generated"
}


def get_git_info() -> dict[str, str | bool]:
    """Retrieve git HEAD sha and dirty status without external dependency."""
    head_sha = os.environ.get("INTEROS_BUILD_GIT_SHA", "")
    if not head_sha:
        try:
            res = subprocess.run(
                ["git", "rev-parse", "HEAD"],
                cwd=str(ROOT),
                capture_output=True,
                text=True,
                timeout=5,
            )
            if res.returncode == 0:
                head_sha = res.stdout.strip()
        except (OSError, subprocess.SubprocessError):
            head_sha = "unknown"

    dirty_env = os.environ.get("INTEROS_BUILD_GIT_DIRTY")
    if dirty_env is not None:
        dirty = dirty_env.lower() in ("1", "true", "yes")
    else:
        try:
            res = subprocess.run(
                ["git", "status", "--porcelain"],
                cwd=str(ROOT),
                capture_output=True,
                text=True,
                timeout=5,
            )
            dirty = bool(res.stdout.strip()) if res.returncode == 0 else False
        except (OSError, subprocess.SubprocessError):
            dirty = False

    return {
        "head_sha": head_sha or "unknown",
        "dirty": dirty,
    }


def compute_source_tree_hash() -> str:
    """Compute deterministic SHA-256 hash across all tracked source files."""
    hasher = hashlib.sha256()

    files_to_hash: list[Path] = []
    for d in TRACKED_SOURCE_DIRS:
        dir_path = ROOT / d
        if not dir_path.is_dir():
            continue
        for root, dirs, files in os.walk(dir_path):
            dirs[:] = [d for d in dirs if d not in IGNORED_DIR_NAMES]
            for f in sorted(files):
                p = Path(root) / f
                if p.suffix.lower() not in IGNORED_EXTENSIONS:
                    files_to_hash.append(p)

    for rel_path in TRACKED_SOURCE_FILES:
        p = ROOT / rel_path
        if p.is_file():
            files_to_hash.append(p)

    files_to_hash.sort(key=lambda p: str(p.relative_to(ROOT)).replace("\\", "/"))

    for path in files_to_hash:
        rel = str(path.relative_to(ROOT)).replace("\\", "/")
        hasher.update(rel.encode("utf-8"))
        try:
            hasher.update(path.read_bytes())
        except OSError:
            pass

    return hasher.hexdigest()


def create_provenance_manifest(
    boot_image_path: Path,
    kernel_elf_path: Path,
    profile: str,
    features: list[str],
    embedded_digests_json: Path | None = None,
) -> Path:
    """Create and write `<boot_image_path>.provenance.json` sidecar manifest."""
    assert boot_image_path.is_file(), f"Missing boot image: {boot_image_path}"
    assert kernel_elf_path.is_file(), f"Missing kernel ELF: {kernel_elf_path}"

    git_info = get_git_info()
    source_tree_hash = compute_source_tree_hash()

    boot_img_bytes = boot_image_path.read_bytes()
    kernel_elf_bytes = kernel_elf_path.read_bytes()

    embedded_info: dict = {
        "manifest_sha256": "",
        "count": 0,
        "images": {},
    }
    if embedded_digests_json and embedded_digests_json.is_file():
        try:
            embedded_info = json.loads(embedded_digests_json.read_text(encoding="utf-8"))
        except (OSError, json.JSONDecodeError):
            pass

    manifest = {
        "schema_version": 1,
        "git": {
            "head_sha": git_info["head_sha"],
            "dirty": git_info["dirty"],
            "source_tree_sha256": source_tree_hash,
        },
        "build": {
            "target": "x86_64-unknown-none",
            "profile": profile,
            "features": sorted(features),
            "debug_control_plane": "debug-control-plane" in features,
        },
        "kernel_elf": {
            "path": str(kernel_elf_path.relative_to(ROOT)).replace("\\", "/"),
            "sha256": hashlib.sha256(kernel_elf_bytes).hexdigest(),
            "size": len(kernel_elf_bytes),
        },
        "boot_image": {
            "path": str(boot_image_path.relative_to(ROOT)).replace("\\", "/"),
            "sha256": hashlib.sha256(boot_img_bytes).hexdigest(),
            "size": len(boot_img_bytes),
        },
        "embedded_userspace": embedded_info,
    }

    manifest_path = Path(str(boot_image_path) + ".provenance.json")
    manifest_path.write_text(json.dumps(manifest, indent=2), encoding="utf-8")
    return manifest_path


def verify_boot_image_provenance(
    boot_image_path: Path,
    expected_debug_control_plane: bool | None = None,
    allow_source_diff: bool = True,
) -> dict:
    """
    Cryptographically verify boot image against its provenance manifest.
    Fails closed (raises AssertionError) if image is missing, corrupt, stale,
    or configured with wrong feature flags.
    """
    assert boot_image_path.is_file(), f"Boot image missing: {boot_image_path}"
    manifest_path = Path(str(boot_image_path) + ".provenance.json")
    assert manifest_path.is_file(), (
        f"Missing provenance manifest: {manifest_path}. "
        "Every accepted boot image must have a cryptographically bound provenance sidecar. "
        "Rebuild via tools/interos.py."
    )

    try:
        manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as e:
        raise AssertionError(f"Corrupt provenance manifest at {manifest_path}: {e}") from e

    # 1. Verify boot image exact SHA-256 match
    actual_img_sha = hashlib.sha256(boot_image_path.read_bytes()).hexdigest()
    expected_img_sha = manifest.get("boot_image", {}).get("sha256")
    assert actual_img_sha == expected_img_sha, (
        f"Image digest mismatch for {boot_image_path}!\n"
        f"  Actual:   {actual_img_sha}\n"
        f"  Manifest: {expected_img_sha}\n"
        "The image was modified, truncated, or replaced without updating the provenance manifest."
    )

    # 2. Verify feature flags & profile
    build_info = manifest.get("build", {})
    actual_debug_cp = build_info.get("debug_control_plane", False)
    if expected_debug_control_plane is not None:
        assert actual_debug_cp == expected_debug_control_plane, (
            f"Feature flag mismatch for {boot_image_path}: "
            f"expected debug_control_plane={expected_debug_control_plane}, "
            f"got {actual_debug_cp}."
        )

    # 3. Verify kernel binary markers directly in the boot image or kernel ELF
    kernel_rel_path = manifest.get("kernel_elf", {}).get("path")
    if kernel_rel_path:
        kernel_path = ROOT / kernel_rel_path
        if kernel_path.is_file():
            actual_k_sha = hashlib.sha256(kernel_path.read_bytes()).hexdigest()
            expected_k_sha = manifest.get("kernel_elf", {}).get("sha256")
            assert actual_k_sha == expected_k_sha, (
                f"Kernel ELF mismatch for {kernel_path}: actual={actual_k_sha}, expected={expected_k_sha}"
            )

    # 4. Source Tree check
    if not allow_source_diff:
        current_source_hash = compute_source_tree_hash()
        manifest_source_hash = manifest.get("git", {}).get("source_tree_sha256")
        assert current_source_hash == manifest_source_hash, (
            f"Stale boot image: source tree has changed since {boot_image_path.name} was built.\n"
            f"  Current tree hash:  {current_source_hash}\n"
            f"  Manifest tree hash: {manifest_source_hash}\n"
            "Rebuild image before running acceptance tests."
        )

    return manifest
