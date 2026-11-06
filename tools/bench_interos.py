#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

"""
tools/bench_interos.py - InterOS footprint and telemetry measurement

Reports only what this tool measures here, from files and serial logs that exist
right now:
  1. Storage footprint (boot image, Ring-3 application crates and their sources)
  2. Kernel heap occupancy read from live [DIAG_STATE] serial telemetry
  3. Scanout cycle counters read from [KMS PERF] markers

WHY no other operating system appears in any output line: an idle-RAM, image or
boot-latency figure for a different platform means something only with the
device, the build and the method that produced it. Nothing here measured one,
so printing one beside a number this project did take would invite a reader to
compare the two -- which is the fabrication the platform truth rules forbid.
Every figure below traces to a file size on disk or a line of the guest's own
serial log; anything without such a source is printed as UNAVAILABLE, and the
report says outright that no comparable measurement of any other system was
made.

Outputs structured JSON (`target/benchmark_report.json`) and formatted terminal tables.
Fails closed when telemetry logs or build artifacts are absent.
"""

import os
import sys
import json
import re
import time
import argparse
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent

# WHY this file has no table of other operating systems' figures in it: a number
# with no device, no build and no method behind it is not a measurement, and two
# numbers side by side read as a comparison whether or not one was made. The tool
# therefore reports the sizes and telemetry it took from this tree, and lists the
# figures it did not take.
NOT_MEASURED = [
    "Cold boot latency: timing from process start to first scanout belongs to "
    "tools/bench_boot.py, which this tool does not run.",
    "Interactive render allocations: tools/bench_heap.py walks those with an "
    "allocation counter; nothing here observes an event loop.",
    "Per-application resident memory: the guest's serial log carries kernel heap "
    "totals only, and no per-process accounting is read.",
    "Idle RAM, image size and boot latency of any other mobile operating system: "
    "never measured on this rig, and no published figure is quoted without the "
    "device, build and method that produced it.",
]

def analyze_storage():
    """Measures physical image and package sizes without synthetic fallbacks."""
    results = {}
    target_dir = ROOT / "target"
    
    # 1. Search for canonical bootable image
    boot_img_candidates = [
        target_dir / "inter-os-bios.img",
        target_dir / "inter-os-release.img",
        target_dir / "inter-os-bios-release.img",
        target_dir / "inter-os-stripped.img",
    ]
    img_size = None
    img_name = None
    for cand in boot_img_candidates:
        if cand.exists():
            img_size = cand.stat().st_size
            img_name = cand.name
            break
            
    if img_size is not None:
        results["boot_image"] = {
            "name": img_name,
            "size_bytes": img_size,
            "size_mib": round(img_size / (1024 * 1024), 2),
            "status": "AVAILABLE",
        }
    else:
        results["boot_image"] = {
            "name": "UNAVAILABLE",
            "size_bytes": None,
            "size_mib": None,
            "status": "MISSING",
        }

    # 2. Search for Ring-3 apps
    apps_dir = ROOT / "apps"
    app_count = 0
    app_sources = 0
    if apps_dir.exists():
        for d in apps_dir.iterdir():
            if d.is_dir() and (d / "Cargo.toml").exists():
                app_count += 1
                for f in (d / "src").rglob("*.rs"):
                    app_sources += f.stat().st_size
                    
    results["apps"] = {
        "count": app_count,
        "total_source_bytes": app_sources,
        "total_source_kib": round(app_sources / 1024, 1),
    }
    
    return results

def parse_telemetry_log(log_path):
    """Parses serial output for authentic [DIAG_STATE], [KMS PERF], and [BOOT] markers.
    Fails closed with None for all fields when no valid log is provided.
    """
    telemetry = {
        "heap_used_bytes": None,
        "heap_free_bytes": None,
        "tasks_count": None,
        "kms_composite_cycles": None,
        "kms_present_cycles": None,
        "boot_ticks": None,
        "source": "none",
    }
    if not log_path or not os.path.exists(log_path):
        return telemetry

    telemetry["source"] = str(log_path)
    with open(log_path, "r", encoding="utf-8", errors="replace") as f:
        content = f.read()

    # Match DIAG_STATE: [DIAG_STATE] tasks_total=(\d+) ... heap_used=(\d+) heap_free=(\d+)
    m_heap_used = re.search(r"\[DIAG_STATE\].*?heap_used=(\d+)", content)
    m_heap_free = re.search(r"\[DIAG_STATE\].*?heap_free=(\d+)", content)
    m_tasks = re.search(r"\[DIAG_STATE\].*?tasks_total=(\d+)", content) or re.search(r"\[DIAG_STATE\].*?tasks=(\d+)", content)
    
    if m_heap_used:
        telemetry["heap_used_bytes"] = int(m_heap_used.group(1))
    if m_heap_free:
        telemetry["heap_free_bytes"] = int(m_heap_free.group(1))
    if m_tasks:
        telemetry["tasks_count"] = int(m_tasks.group(1))

    # Match KMS PERF: [KMS PERF] ... compose_cycles=(\d+) present_cycles=(\d+)
    m_kms_comp = re.search(r"\[KMS PERF\].*?(?:compose_cycles|composite)=(\d+)", content)
    m_kms_pres = re.search(r"\[KMS PERF\].*?(?:present_cycles|present)=(\d+)", content)
    if m_kms_comp:
        telemetry["kms_composite_cycles"] = int(m_kms_comp.group(1))
    if m_kms_pres:
        telemetry["kms_present_cycles"] = int(m_kms_pres.group(1))

    # Match boot ticks
    m_boot = re.search(r"\[BOOT\].*?ticks=(\d+)", content) or re.search(r"\[DIAG_STATE\].*?ticks=(\d+)", content)
    if m_boot:
        telemetry["boot_ticks"] = int(m_boot.group(1))

    return telemetry

def generate_report(storage, telemetry, output_json=None):
    """Prints the figures this tool measured, then names the ones it did not."""
    print("=" * 80)
    print(" INTEROS SOVEREIGN SMARTPHONE OS - MEASURED FOOTPRINT AND TELEMETRY")
    print("=" * 80)
    print()
    print(" Every figure below is read from a file in this tree or from the guest's own")
    print(" serial log. None is a figure for any other operating system.")
    print()

    boot_img = storage.get("boot_image", {})
    # WHY every size in this report is labelled with a binary prefix: each one is
    # divided by 1024 or 1024 * 1024, so the unit the number carries is KiB/MiB.
    # Labelling those MB and KB stated a decimal unit the arithmetic never
    # computed, and this report is the source the README's figures are quoted from.
    if boot_img.get("size_mib") is not None:
        print(f"[*] Boot Image Size  : {boot_img['size_mib']:.2f} MiB ({boot_img.get('name')})")
    else:
        print("[*] Boot Image Size  : UNAVAILABLE (run build first)")

    apps = storage.get("apps", {})
    print(f"[*] Ring-3 Apps      : {apps.get('count', 0)} crates, "
          f"{apps.get('total_source_kib', 0)} KiB of their own source")

    if telemetry.get("heap_used_bytes") is not None and telemetry.get("heap_free_bytes") is not None:
        used_mib = telemetry["heap_used_bytes"] / (1024 * 1024)
        free_mib = telemetry["heap_free_bytes"] / (1024 * 1024)
        print(f"[*] Kernel Memory    : {used_mib:.2f} MiB used / {free_mib:.2f} MiB free pool "
              f"(from {telemetry.get('source')})")
    else:
        print("[*] Kernel Memory    : UNAVAILABLE (no live serial telemetry log provided)")

    if telemetry.get("tasks_count") is not None:
        print(f"[*] Guest Tasks      : {telemetry['tasks_count']} (same log)")

    if telemetry.get("kms_composite_cycles") is not None:
        print(f"[*] Scanout Cycles   : {telemetry['kms_composite_cycles']} composite cycles / "
              f"{telemetry.get('kms_present_cycles', 'n/a')} present cycles (same log)")
    else:
        print("[*] Scanout Cycles   : UNAVAILABLE")

    print()
    print("-" * 80)
    print(" NOT MEASURED HERE -- reported absent, never quoted from elsewhere:")
    print("-" * 80)
    for note in NOT_MEASURED:
        print(f"  - {note}")
    print()
    print(" No comparable measurement of any other mobile operating system was made on")
    print(" this rig, so none is printed beside the numbers above. A QEMU guest and a")
    print(" published phone figure share no method, and neither number can support a")
    print(" comparison between them.")
    print("-" * 80)
    print()

    has_telemetry = telemetry.get("heap_used_bytes") is not None
    has_image = boot_img.get("size_mib") is not None
    overall_status = "COMPLETE" if (has_telemetry and has_image) else "INCOMPLETE"
    current_timestamp = int(time.strftime("%Y%m%d"))

    report_data = {
        "storage": storage,
        "telemetry": telemetry,
        "not_measured": NOT_MEASURED,
        "comparison": "none: this tool measured no operating system but this one",
        "status": overall_status,
        "timestamp": current_timestamp,
    }

    if output_json:
        os.makedirs(os.path.dirname(output_json), exist_ok=True)
        with open(output_json, "w", encoding="utf-8") as f:
            json.dump(report_data, f, indent=2)
        print(f"[{overall_status}] Measurement record written to: {output_json}")

    return report_data

def main():
    parser = argparse.ArgumentParser(description="InterOS footprint and telemetry measurement")
    parser.add_argument("--log", help="Path to a serial log carrying the boot and telemetry markers", default=None)
    parser.add_argument("--out", help="Output path for JSON report", default=str(ROOT / "target" / "benchmark_report.json"))
    args = parser.parse_args()

    storage = analyze_storage()
    telemetry = parse_telemetry_log(args.log)
    report = generate_report(storage, telemetry, args.out)
    # WHY a hand of the log forces the exit: once a caller supplies telemetry, an
    # unreadable image or an unparsed marker means the run collected nothing, and
    # a record of nothing must not leave looking like a record of a result.
    if report["status"] != "COMPLETE" and args.log is not None:
        sys.exit(1)

if __name__ == "__main__":
    main()
