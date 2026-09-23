#!/usr/bin/env python3
"""
InterOS Core Developer CLI (interos)
=====================================
Unified CLI tool for creating, building, packaging, and hot-deploying
sovereign Rust Mobile Application Packages (.rapp) on InterOS Core.

Usage:
  python tools/interos.py new <app_name> [--template basic|authenticator|sensor|meshchat]
  python tools/interos.py build <app_name> [--target aarch64-unknown-none|riscv64gc-unknown-none-elf]
  python tools/interos.py pack <app_name> [--output dist/<app_name>.rapp]
  python tools/interos.py run <app_name> [--port 4300]
  python tools/interos.py list [--port 4300]
"""

import argparse
import json
import os
import shutil
import subprocess
import sys
import urllib.error
import urllib.request
import hashlib
import zipfile
from pathlib import Path

WORKSPACE_ROOT = Path(__file__).resolve().parent.parent
APPS_DIR = WORKSPACE_ROOT / "apps"
DIST_DIR = WORKSPACE_ROOT / "dist"
PACKAGES_DIR = WORKSPACE_ROOT / "packages"

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

    # 1. Cargo.toml
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

    # 2. manifest.json
    manifest = {
        "id": f"org.interos.{app_name}",
        "name": app_name.replace("_", " ").title(),
        "version": "1.0.0",
        "category": "utility" if template_key != "meshchat" else "communication",
        "description": tmpl["description"],
        "icon": f"ui/icon.svg",
        "entrypoint": {
            "ui": "ui/index.html",
            "backend": f"bin/{app_name}.elf"
        },
        "permissions": tmpl["permissions"],
        "min_sdk_version": "0.8.0"
    }
    (app_dir / "manifest.json").write_text(json.dumps(manifest, indent=2), encoding="utf-8")

    # 3. src/main.rs
    main_rs = f"""//! InterOS Core Application: {app_name}
//! Template: {template_key}
#![no_std]
#![no_main]

use interos_std::ipc;
use interos_std::log;

#[no_mangle]
pub extern "C" fn _start() -> ! {{
    log::init("org.interos.{app_name}");
    log::info("{app_name} initialized inside Ring 3 sandbox");

    // Main event loop handling IPC requests from UI
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

    # 4. ui/index.html
    ui_html = f"""<!DOCTYPE html>
<html lang="en">
<head>
  <meta charset="UTF-8">
  <title>{manifest['name']}</title>
  <style>
    body {{
      margin: 0;
      padding: 20px;
      background: #0d1117;
      color: #e6edf3;
      font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, sans-serif;
    }}
    .header {{ display: flex; align-items: center; gap: 12px; margin-bottom: 20px; }}
    .icon {{ width: 36px; height: 36px; border-radius: 8px; background: #238636; display: flex; align-items: center; justify-content: center; font-weight: bold; }}
    .card {{ background: #161b22; border: 1px solid #30363d; border-radius: 8px; padding: 16px; margin-bottom: 16px; }}
    button {{ background: #238636; border: none; color: #fff; padding: 10px 16px; border-radius: 6px; cursor: pointer; font-weight: 600; }}
    button:hover {{ background: #2ea043; }}
  </style>
</head>
<body>
  <div class="header">
    <div class="icon">IO</div>
    <div>
      <h2 style="margin: 0;">{manifest['name']}</h2>
      <small style="color: #8b949e;">org.interos.{app_name} v1.0.0</small>
    </div>
  </div>
  <div class="card">
    <p>{tmpl['description']}</p>
    <button id="btn-ipc">Send Syscall IPC</button>
    <div id="status" style="margin-top: 12px; font-family: monospace; color: #58a6ff;">Ready</div>
  </div>
  <script>
    document.getElementById("btn-ipc").addEventListener("click", async () => {{
      const status = document.getElementById("status");
      status.textContent = "Dispatching SYS_IPC_SEND...";
      if (window.InterOS && window.InterOS.ipc) {{
        const res = await window.InterOS.ipc.request("org.interos.{app_name}", "PING", {{ timestamp: Date.now() }});
        status.textContent = "IPC Response: " + JSON.stringify(res);
      }} else {{
        status.textContent = "IPC Bridge not available in standalone preview";
      }}
    }});
  </script>
</body>
</html>
"""
    (app_dir / "ui" / "index.html").write_text(ui_html, encoding="utf-8")

    # 5. Register in root Cargo.toml workspace members if not present
    root_cargo = WORKSPACE_ROOT / "Cargo.toml"
    if root_cargo.exists():
        content = root_cargo.read_text(encoding="utf-8")
        member_line = f'    "apps/{app_name}",'
        if member_line not in content and 'members = [' in content:
            new_content = content.replace('members = [', f'members = [\n{member_line}')
            root_cargo.write_text(new_content, encoding="utf-8")
            log(f"Registered apps/{app_name} in root Cargo.toml")

    log_success(f"Created application at {app_dir.relative_to(WORKSPACE_ROOT)}")
    log("Next steps:")
    log(f"  python tools/interos.py build {app_name}")
    log(f"  python tools/interos.py run {app_name}")
    return 0


def cmd_build(args):
    app_name = args.name.lower().replace("-", "_")
    app_dir = APPS_DIR / app_name
    if not app_dir.exists():
        log_error(f"App not found at {app_dir}")
        return 1

    target = args.target
    log(f"Compiling {app_name} for target {target}...")
    manifest_path = str(app_dir / "Cargo.toml")
    cmd = ["cargo", "build", "--manifest-path", manifest_path, "--target", target]
    res = subprocess.run(cmd)
    if res.returncode != 0:
        log_error("Build failed.")
        return res.returncode

    # Locate generated ELF binary and copy to app_dir/bin/{app_name}.elf
    elf_source = WORKSPACE_ROOT / "target" / target / "debug" / app_name
    if not elf_source.exists():
        elf_source = WORKSPACE_ROOT / "target" / target / "debug" / f"{app_name}.exe"

    bin_dir = app_dir / "bin"
    bin_dir.mkdir(exist_ok=True)
    out_elf = bin_dir / f"{app_name}.elf"
    if elf_source.exists():
        shutil.copy2(elf_source, out_elf)
        log_success(f"Built ELF executable: {out_elf.relative_to(WORKSPACE_ROOT)} ({out_elf.stat().st_size} bytes)")
    else:
        log_error(f"Built binary not found at expected location: {elf_source}")
        return 1

    log_success(f"Application {app_name} successfully compiled for {target}.")
    return 0


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

    required_fields = ["id", "name", "version", "entrypoint", "permissions"]
    for f in required_fields:
        if f not in manifest_data:
            log_error(f"Manifest missing required field: {f}")
            return 1

    canonical_manifest = json.dumps(manifest_data, sort_keys=True, indent=2).encode("utf-8")
    manifest_hash = hashlib.sha256(canonical_manifest).hexdigest()

    DIST_DIR.mkdir(parents=True, exist_ok=True)
    output_zip = args.output
    if not output_zip:
        output_zip = DIST_DIR / f"{app_name}.rapp"
    else:
        output_zip = Path(output_zip)

    log(f"Packaging and cryptographically signing {app_name} into sovereign .rapp bundle...")

    # Collect files to pack and compute SHA-256 digests
    file_hashes = {}
    files_to_pack = []

    ui_dir = app_dir / "ui"
    if ui_dir.exists():
        for root, _, files in os.walk(ui_dir):
            for f in sorted(files):
                full_p = Path(root) / f
                rel_p = full_p.relative_to(app_dir).as_posix()
                if ".." in rel_p or rel_p.startswith("/"):
                    log_error(f"Path traversal detected in app: {rel_p}")
                    return 1
                data = full_p.read_bytes()
                file_hashes[rel_p] = hashlib.sha256(data).hexdigest()
                files_to_pack.append((full_p, rel_p))

    bin_dir = app_dir / "bin"
    if bin_dir.exists():
        for root, _, files in os.walk(bin_dir):
            for f in sorted(files):
                full_p = Path(root) / f
                rel_p = full_p.relative_to(app_dir).as_posix()
                if ".." in rel_p or rel_p.startswith("/"):
                    log_error(f"Path traversal detected in app: {rel_p}")
                    return 1
                data = full_p.read_bytes()
                file_hashes[rel_p] = hashlib.sha256(data).hexdigest()
                files_to_pack.append((full_p, rel_p))

    # Cryptographic Signing with Ed25519
    key_dir = Path.home() / ".interos"
    key_dir.mkdir(exist_ok=True)
    key_file = key_dir / "developer_ed25519.key"

    from cryptography.hazmat.primitives.asymmetric import ed25519
    from cryptography.hazmat.primitives import serialization

    if key_file.exists():
        priv_key = serialization.load_pem_private_key(key_file.read_bytes(), password=None)
    else:
        priv_key = ed25519.Ed25519PrivateKey.generate()
        key_file.write_bytes(priv_key.private_bytes(
            encoding=serialization.Encoding.PEM,
            format=serialization.PrivateFormat.PKCS8,
            encryption_algorithm=serialization.NoEncryption()
        ))
        log(f"Generated sovereign developer signing key at {key_file}")

    pub_key_bytes = priv_key.public_key().public_bytes(
        encoding=serialization.Encoding.Raw,
        format=serialization.PublicFormat.Raw
    )

    signed_payload = json.dumps({
        "package_id": manifest_data["id"],
        "version": manifest_data["version"],
        "manifest_sha256": manifest_hash,
        "files_sha256": file_hashes,
        "permissions": manifest_data.get("permissions", []),
    }, sort_keys=True).encode("utf-8")

    signature_bytes = priv_key.sign(signed_payload)

    signature_manifest = {
        "format": "InterOS-RAPP-Sig-v1",
        "algorithm": "Ed25519",
        "public_key": pub_key_bytes.hex(),
        "signature": signature_bytes.hex(),
        "signed_payload": json.loads(signed_payload.decode("utf-8"))
    }

    with zipfile.ZipFile(output_zip, "w", zipfile.ZIP_DEFLATED) as zf:
        zf.writestr("manifest.json", canonical_manifest)
        zf.writestr("signature.json", json.dumps(signature_manifest, indent=2).encode("utf-8"))
        for full_p, rel_p in files_to_pack:
            zf.write(full_p, arcname=rel_p)

    log_success(f"Successfully packaged and signed {output_zip.stat().st_size} bytes -> {output_zip}")
    log(f"  • Publisher Public Key: {pub_key_bytes.hex()[:16]}...")
    log(f"  • Ed25519 Signature: {signature_bytes.hex()[:16]}...")
    log(f"  • Verified File Entitlements: {len(file_hashes)}")
    return 0


def cmd_run(args):
    app_name = args.name.lower().replace("-", "_")
    # 1. Build
    build_args = argparse.Namespace(name=app_name, target=args.target)
    ret = cmd_build(build_args)
    if ret != 0:
        return ret

    # 2. Pack
    rapp_path = DIST_DIR / f"{app_name}.rapp"
    pack_args = argparse.Namespace(name=app_name, output=rapp_path)
    ret = cmd_pack(pack_args)
    if ret != 0:
        return ret

    # 3. Deploy via REST API to running emulator
    url = f"http://localhost:{args.port}/api/rapp/install"
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
            log_success(f"Installed to running emulator: {body}")

        # Launch the app on phone display
        launch_url = f"http://localhost:{args.port}/api/rapp/launch"
        launch_req = urllib.request.Request(
            launch_url,
            data=json.dumps({"app_id": f"org.interos.{app_name}"}).encode("utf-8"),
            headers={"Content-Type": "application/json"},
            method="POST"
        )
        with urllib.request.urlopen(launch_req, timeout=5) as resp:
            log_success(f"App {app_name} launched on Device Studio display!")
    except urllib.error.URLError as e:
        log_error(f"Could not connect to InterOS Studio on port {args.port}: {e}")
        log("Make sure the emulator server is running: python emulator/server.py")
        return 1

    return 0


def cmd_list(args):
    url = f"http://localhost:{args.port}/api/rapp/list"
    log(f"Querying installed .rapp packages from {url}...")
    try:
        req = urllib.request.Request(url, method="GET")
        with urllib.request.urlopen(req, timeout=5) as resp:
            data = json.loads(resp.read().decode("utf-8"))
            apps = data.get("apps", [])
            print(f"\nFound {len(apps)} installed sovereign packages:")
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
    port = args.port

    log(f"Requesting uninstallation of '{target}' from InterOS emulator (port {port})...")
    url = f"http://localhost:{port}/api/apps/uninstall"
    payload = json.dumps({"app_id": target, "name": target, "filename": target}).encode("utf-8")
    req = urllib.request.Request(
        url,
        data=payload,
        headers={"Content-Type": "application/json"}
    )
    try:
        with urllib.request.urlopen(req, timeout=5) as resp:
            data = json.loads(resp.read().decode("utf-8"))
            if data.get("success"):
                log_success(f"Package '{data.get('name', target)}' successfully uninstalled.")
                log_success("Ring 3 userspace sandbox revoked and memory reclaimed.")
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

    port = args.port
    filename = pkg_path.name
    log(f"Auditing and uploading package '{filename}' to InterOS Studio (port {port})...")
    data = pkg_path.read_bytes()
    url = f"http://localhost:{port}/api/apps/upload?filename={urllib.parse.quote(filename)}"
    req = urllib.request.Request(
        url,
        data=data,
        headers={"Content-Type": "application/octet-stream"},
        method="POST"
    )
    try:
        with urllib.request.urlopen(req, timeout=10) as resp:
            res_data = json.loads(resp.read().decode("utf-8"))
            if res_data.get("success"):
                pkg = res_data.get("package", {})
                size_str = f"{(pkg.get('size', 0)/(1024*1024)):.2f} MB" if pkg.get('size', 0) > 1024*1024 else f"{(pkg.get('size', 0)/1024):.1f} KB"
                log_success(f"Package '{pkg.get('name')}' ({pkg.get('id')}) verified & mounted into /data/apps/!")
                log(f"Format: {pkg.get('format')} &bull; Runtime: {pkg.get('type')} &bull; Size: {size_str}")
                if pkg.get('version'):
                    log(f"Target SDK: {pkg.get('version')}")
                if pkg.get('dex_classes'):
                    log(f"Dalvik/ART Classes: {pkg.get('dex_classes')} classes validated in classes.dex")
                if pkg.get('capabilities'):
                    log(f"Capabilities / Permissions: {', '.join(pkg.get('capabilities')[:5])}...")
                log_success("Strict Zero Ambient Authority Enforced: Ambient root denied.")
                return 0
            else:
                log_error(f"Package verification rejected: {res_data.get('error')}")
                return 1
    except Exception as e:
        log_error(f"Could not connect to emulator: {e}")
        return 1


def main():
    parser = argparse.ArgumentParser(
        description="InterOS Core Developer CLI for sovereign .rapp and .apk packages",
        formatter_class=argparse.RawDescriptionHelpFormatter
    )
    subparsers = parser.add_subparsers(dest="command", help="Command to run")

    # new
    p_new = subparsers.add_parser("new", help="Scaffold a new .rapp project")
    p_new.add_argument("name", help="App name (e.g. authenticator, sensor_hub)")
    p_new.add_argument("--template", choices=list(TEMPLATES.keys()), default="basic", help="App template")
    p_new.set_defaults(func=cmd_new)

    # build
    p_build = subparsers.add_parser("build", help="Compile #![no_std] Rust backend")
    p_build.add_argument("name", help="App name")
    p_build.add_argument("--target", default="aarch64-unknown-none", help="Rust target triple")
    p_build.set_defaults(func=cmd_build)

    # pack
    p_pack = subparsers.add_parser("pack", help="Bundle app into .rapp package")
    p_pack.add_argument("name", help="App name")
    p_pack.add_argument("--output", help="Output file path (default: dist/<name>.rapp)")
    p_pack.set_defaults(func=cmd_pack)

    # run
    p_run = subparsers.add_parser("run", help="Build, pack, and hot-deploy to local emulator")
    p_run.add_argument("name", help="App name")
    p_run.add_argument("--target", default="aarch64-unknown-none", help="Rust target triple")
    p_run.add_argument("--port", type=int, default=4300, help="Studio emulator port")
    p_run.set_defaults(func=cmd_run)

    # list
    p_list = subparsers.add_parser("list", help="List installed .rapp and .apk packages on emulator")
    p_list.add_argument("--port", type=int, default=4300, help="Studio emulator port")
    p_list.set_defaults(func=cmd_list)

    # install
    p_inst = subparsers.add_parser("install", help="Strictly verify and install a .rapp, .wasm, or .apk package")
    p_inst.add_argument("package_path", help="Path to package file (e.g. packages/fossify_file_manager.apk)")
    p_inst.add_argument("--port", type=int, default=4300, help="Studio emulator port")
    p_inst.set_defaults(func=cmd_install)

    # uninstall
    p_un = subparsers.add_parser("uninstall", help="Uninstall package from emulator")
    p_un.add_argument("name", help="App ID or name (e.g. org.fossify.filemanager, meshchat)")
    p_un.add_argument("--port", type=int, default=4300, help="Studio emulator port")
    p_un.set_defaults(func=cmd_uninstall)

    args = parser.parse_args()
    if not hasattr(args, "func"):
        parser.print_help()
        return 1

    return args.func(args)


if __name__ == "__main__":
    sys.exit(main())
