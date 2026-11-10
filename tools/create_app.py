#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

"""
InterOS Core — Sovereign Mobile Application Scaffolding Generator
Usage:
    python tools/create_app.py <app_name> [--template basic|audio|sensor]
"""

import sys
import os
import argparse
from pathlib import Path

def main():
    parser = argparse.ArgumentParser(description="Scaffold a new sovereign InterOS mobile application.")
    parser.add_argument("app_name", nargs="?", default=None, help="Name of the application (snake_case, e.g. podcast_player)")
    parser.add_argument("--template", choices=["basic", "audio", "sensor"], default="basic", help="App template type")
    parser.add_argument("--from-web", dest="from_web", default=None, help="Port an existing web application or PWA URL to InterOS RAPP")
    parser.add_argument("--output", "-o", default=None, help="Destination .rapp file path when using --from-web")
    args = parser.parse_args()

    if args.from_web:
        import subprocess
        port_script = Path(__file__).resolve().parent / "port_web.py"
        cmd = [sys.executable, str(port_script), args.from_web]
        if args.app_name:
            cmd.extend(["--name", args.app_name])
        if args.output:
            cmd.extend(["--output", args.output])
        sys.exit(subprocess.call(cmd))

    if not args.app_name:
        parser.error("the following arguments are required: app_name")

    app_name = args.app_name.lower().replace("-", "_")
    root_dir = Path(__file__).resolve().parent.parent
    apps_dir = root_dir / "apps"
    target_app_dir = apps_dir / app_name

    if target_app_dir.exists():
        print(f"[-] Error: App directory already exists at: {target_app_dir}")
        sys.exit(1)

    print("================================================================")
    print(f"  [InterOS App Scaffolder] Creating '{app_name}' ({args.template})")
    print("================================================================")

    # 1. Create directory structure
    src_dir = target_app_dir / "src"
    src_dir.mkdir(parents=True, exist_ok=True)

    # 2. Generate Cargo.toml
    cargo_content = f"""[package]
name = "{app_name}"
version = "0.1.0"
edition = "2021"
authors = ["InterOS Developer"]
description = "Sovereign {app_name} native mobile application for InterOS Core"

[dependencies]
interos-std = {{ path = "../../sdk/rust/interos-std" }}

[profile.dev]
panic = "abort"

[profile.release]
panic = "abort"
lto = true
"""
    (target_app_dir / "Cargo.toml").write_text(cargo_content, encoding="utf-8")

    # 3. Generate src/main.rs based on template
    if args.template == "audio":
        main_content = f"""//! Sovereign {app_name} Audio Application for InterOS Core
#![no_std]
#![no_main]

use core::panic::PanicInfo;
use interos_std as std;

#[no_mangle]
pub extern "C" fn _start() -> ! {{
    std::log(0, "[{app_name.upper()}] Initializing sovereign audio app in Ring 3...").ok();

    // Derive media encryption key from Hardware Keystore Slot 1
    let mut media_key = [0u8; 32];
    if std::keystore_derive(1, &mut media_key).is_ok() {{
        std::log(0, "[{app_name.upper()}] Hardware keystore derived session key.").ok();
    }}

    // Play 3-note initialization fanfare (C4 -> E4 -> G4)
    std::audio_play_tone(261).ok();
    std::audio_play_tone(329).ok();
    std::audio_play_tone(392).ok();
    std::audio_silence().ok();
    std::audio_haptic_click().ok();

    // Event loop polling for IPC playback commands
    loop {{
        let _ = std::event_poll(std::EVENT_IPC_MSG | std::EVENT_TIMER_TICK, 100);
        std::yield_now();
    }}
}}

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {{
    std::log(0, "[{app_name.upper()}] Panic detected! Terminating task...").ok();
    std::exit();
}}
"""
    elif args.template == "sensor":
        main_content = f"""//! Sovereign {app_name} Telemetry & Sensor Application for InterOS Core
#![no_std]
#![no_main]

use core::panic::PanicInfo;
use interos_std as std;

#[no_mangle]
pub extern "C" fn _start() -> ! {{
    std::log(0, "[{app_name.upper()}] Sensor tracking service online in Ring 3.").ok();

    // Query platform hardware telemetry service
    let mut status = [0u8; 16];
    let (battery_pct, temp_celsius) = if let Ok(n) = std::ipc_recv(&mut status) {{
        if n >= 2 {{ (status[0], status[1]) }} else {{ (0, 0) }}
    }} else {{
        (0, 0)
    }};

    if battery_pct > 0 {{
        std::log(0, "[{app_name.upper()}] Telemetry acquired from sensor service.").ok();
    }} else {{
        std::log(0, "[{app_name.upper()}] Hardware sensor telemetry: UNAVAILABLE / OFFLINE.").ok();
    }}

    // Stream sensor telemetry frame over IPC bus
    let payload = b"SENSOR_TELEMETRY_PKT_V1";
    let _ = std::ipc_send(0x08, payload);

    loop {{
        let _ = std::event_poll(std::EVENT_TIMER_TICK, 250);
        std::yield_now();
    }}
}}

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {{
    std::log(0, "[{app_name.upper()}] Panic! Terminating task...").ok();
    std::exit();
}}
"""
    else: # basic
        main_content = f"""//! Sovereign {app_name} Application for InterOS Core
#![no_std]
#![no_main]

use core::panic::PanicInfo;
use interos_std as std;

#[no_mangle]
pub extern "C" fn _start() -> ! {{
    std::log(0, "[{app_name.upper()}] Application initialized in Ring 3.").ok();

    // Derive isolation key from Hardware Keystore
    let mut isolation_key = [0u8; 32];
    std::keystore_derive(0, &mut isolation_key).ok();

    // Synthesize haptic confirmation
    std::audio_haptic_click().ok();

    // Event loop polling for IPC messages
    loop {{
        let _ = std::event_poll(std::EVENT_IPC_MSG | std::EVENT_TIMER_TICK, 100);
        std::yield_now();
    }}
}}

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {{
    std::log(0, "[{app_name.upper()}] Panic! Terminating task...").ok();
    std::exit();
}}
"""
    (src_dir / "main.rs").write_text(main_content, encoding="utf-8")

    # 4. Generate manifest.json
    manifest_content = f"""{{
  "app_id": "org.interos.{app_name}",
  "name": "{app_name.replace('_', ' ').title()}",
  "version": "1.0.0",
  "author": "InterOS Community",
  "entry_point": "src/main.rs",
  "capabilities": [
    "cap:audio",
    "cap:ipc",
    "cap:keystore",
    "cap:ring3_sandbox"
  ],
  "sandbox_ring": 3,
  "icon": "📱"
}}
"""
    (target_app_dir / "manifest.json").write_text(manifest_content, encoding="utf-8")

    # 5. Register in root Cargo.toml workspace
    root_cargo = root_dir / "Cargo.toml"
    if root_cargo.exists():
        content = root_cargo.read_text(encoding="utf-8")
        member_entry = f'    "apps/{app_name}",\n'
        if member_entry not in content:
            insert_marker = '    "tools/mkbootimg",\n'
            if insert_marker in content:
                new_content = content.replace(insert_marker, member_entry + insert_marker)
                root_cargo.write_text(new_content, encoding="utf-8")
                print(f"[+] Added 'apps/{app_name}' to root Cargo.toml workspace members.")

    print(f"[+] Successfully scaffolded '{app_name}' at: {target_app_dir}")
    print("\nNext Steps:")
    print(f"  1. Test compilation:")
    print(f"     cargo check --manifest-path apps/{app_name}/Cargo.toml --target riscv64gc-unknown-none-elf")
    print(f"  2. Build release binary:")
    print(f"     cargo build --manifest-path apps/{app_name}/Cargo.toml --target riscv64gc-unknown-none-elf --release")
    print(f"  3. Sideload into Device Studio:")
    print(f"     Drag and drop your compiled .app file at http://localhost:4300\n")

if __name__ == "__main__":
    main()
