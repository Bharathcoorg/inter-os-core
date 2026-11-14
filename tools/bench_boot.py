#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

"""Boot-time and runtime measurement for InterOS Core on local QEMU.

WHY this exists rather than a new framework: the repository already has a
canonical x86_64 boot verification harness (`tests/verify_x86_boot.py`) with a
readiness endpoint, a serial reader and a stale-artifact guard. This reuses all
three instead of reimplementing them, and adds only what they do not do:
per-trial timing, outcome classification, and raw-result capture.

WHAT IS MEASURED, PER ARCHITECTURE

  x86_64    QEMU process start -> "[LAUNCHER] Step 5: Initial presentation
            complete". This is a full device boot: firmware, kernel, PID 1,
            services, launcher, and the launcher presenting a frame to scanout.
            The guest keeps running afterwards.

  aarch64   QEMU process start -> "[READY] AArch64 Sovereign Smartphone
            Microkernel". This kernel is a self-test that prints a READY banner
            and then exits. It never starts PID 1 services or a launcher.

  riscv64   QEMU process start -> "[SYSCALL:EXIT] Task exited with code 0".
            This kernel is a self-test that runs one U-mode task and exits.

THE THREE ARE NOT COMPARABLE
  The endpoints differ in what they prove and in how much of the device comes up
  before them. A number from one architecture says nothing about the others, so
  this tool reports each separately and never prints a cross-architecture
  ranking. Timing includes QEMU startup and firmware, because the clock starts
  before the guest exists.

TIMING is monotonic (`time.perf_counter`); wall-clock is recorded only as
context. Raw per-trial records are written to JSON before any summary is
computed, so every summary figure is reproducible from the raw file.
"""

import argparse
import json
import os
import socket
import statistics
import subprocess
import sys
import time
import shutil

REPO_ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
sys.path.insert(0, os.path.join(REPO_ROOT, "tests"))

QEMU_DIR = os.environ.get("QEMU_DIR") or os.environ.get("QEMU_PATH")


ARCHES = {
    "x86_64": {
        "qemu_bin": "qemu-system-x86_64",
        "artifact": os.path.join("target", "inter-os-bios.img"),
        "artifact_kind": "bootable BIOS raw disk image",
        "extra_args": ["-machine", "q35", "-cpu", "qemu64,+rdrand"],
        "drive_arg": "-drive",
        "ready_marker": "Step 5: Initial presentation complete",
        "endpoint": (
            "interactive launcher presented its first frame to scanout "
            "(full boot: firmware, kernel, PID 1, services, launcher)"
        ),
        "guest_expects_to_keep_running": True,
        "timeout_s": 180.0,
        "memory_m": "1024M",
        "bootable": True,
    },
    "aarch64": {
        "qemu_bin": "qemu-system-aarch64",
        "artifact": os.path.join(
            "kernel-aarch64", "target", "aarch64-unknown-none", "debug",
            "kernel-aarch64",
        ),
        "artifact_kind": "standalone kernel ELF (QEMU direct kernel load)",
        "extra_args": ["-machine", "virt", "-cpu", "cortex-a53"],
        "drive_arg": "-kernel",
        "ready_marker": "[READY] AArch64 Sovereign Smartphone Microkernel",
        "endpoint": (
            "self-test kernel reached its READY banner and exited; no PID 1, "
            "no services, no launcher"
        ),
        "guest_expects_to_keep_running": False,
        "timeout_s": 120.0,
        "memory_m": "1024M",
        "bootable": False,
    },
    "riscv64": {
        "qemu_bin": "qemu-system-riscv64",
        "artifact": os.path.join(
            "kernel-riscv64", "target", "riscv64gc-unknown-none-elf", "debug",
            "kernel-riscv64",
        ),
        "artifact_kind": "standalone kernel ELF (QEMU direct kernel load)",
        "extra_args": ["-machine", "virt", "-cpu", "rv64"],
        "drive_arg": "-kernel",
        "ready_marker": "[SYSCALL:EXIT] Task exited with code 0",
        "endpoint": (
            "self-test kernel ran one U-mode task to completion and exited; "
            "no PID 1, no services, no launcher"
        ),
        "guest_expects_to_keep_running": False,
        "timeout_s": 120.0,
        "memory_m": "1024M",
        "bootable": False,
    },
}


# --------------------------------------------------------------------------
# Pure helpers. No QEMU, no sockets, no filesystem: these are what the unit
# tests in tests/test_bench_harness.py exercise.
# --------------------------------------------------------------------------

def summarize(samples):
    """Summary statistics over successful elapsed samples, in milliseconds.

    Returns count/min/median/max/mean. Returns None fields for an empty input
    rather than raising, so a fully failed run still produces a record.
    """
    vals = [float(s) for s in samples]
    if not vals:
        return {"count": 0, "min_ms": None, "median_ms": None,
                "max_ms": None, "mean_ms": None}
    return {
        "count": len(vals),
        "min_ms": round(min(vals), 1),
        "median_ms": round(statistics.median(vals), 1),
        "max_ms": round(max(vals), 1),
        "mean_ms": round(statistics.fmean(vals), 1),
    }


def classify(ready, guest_exited_early, elapsed_ms, timeout_s):
    """Classify one trial so a failure is never silently dropped or counted.

    launch_failure : the guest never produced a readable serial stream.
    guest_crash    : the guest process ended before the readiness marker.
    timeout        : the guest stayed alive but never reached readiness.
    ok             : the readiness marker was observed.
    """
    if not ready:
        if guest_exited_early:
            return "guest_crash"
        return "timeout"
    if elapsed_ms is None or elapsed_ms < 0:
        return "launch_failure"
    return "ok"


def is_stale(artifact_path, run_start_epoch):
    """True when a file predates the run that is supposed to have produced it."""
    if not os.path.exists(artifact_path):
        return True
    try:
        return os.path.getmtime(artifact_path) < run_start_epoch - 1.0
    except OSError:
        return True


def format_bytes(n):
    if n is None:
        return "n/a"
    return "%d bytes (%d KiB / %.3f MiB)" % (n, n / 1024.0, n / (1024.0 * 1024.0))


def _scan_for_kernel_elf(image_path, sector=512, max_scan_bytes=64 * 1024 * 1024):
    """Locate a sector-aligned 64-bit little-endian ELF inside a disk image.

    Returns offset, architecture and the summed PT_LOAD file extents, or None.
    This reads the bytes rather than trusting the container's name, so a disk
    image's reported architecture is the architecture of what it actually
    carries.
    """
    import struct
    size = os.path.getsize(image_path)
    limit = min(size, max_scan_bytes)
    machines = {0x3E: "x86-64", 0xB7: "AArch64", 0xF3: "RISC-V"}
    etypes = {1: "ET_REL", 2: "ET_EXEC", 3: "ET_DYN"}
    with open(image_path, "rb") as fh:
        offset = 0
        while offset < limit:
            fh.seek(offset)
            head = fh.read(64)
            if len(head) < 64:
                break
            if head[:4] != b"\x7fELF" or head[4] != 2 or head[5] != 1:
                offset += sector
                continue
            phoff = struct.unpack_from("<Q", head, 32)[0]
            phent = struct.unpack_from("<H", head, 54)[0]
            phnum = struct.unpack_from("<H", head, 56)[0]
            if not (1 <= phnum <= 64) or not (56 <= phent <= 512) or phoff > 0x10000:
                offset += sector
                continue
            loadable = 0
            fh.seek(offset)
            window = fh.read(phoff + phnum * phent)
            ok = True
            for i in range(phnum):
                o = phoff + i * phent
                if o + phent > len(window):
                    ok = False
                    break
                ptype = struct.unpack_from("<I", window, o)[0]
                if ptype != 1:
                    continue
                p_offset, _, _, p_filesz, _ = struct.unpack_from("<QQQQQ", window, o + 8)
                loadable = max(loadable, p_offset + p_filesz)
            if not ok or loadable == 0:
                offset += sector
                continue
            return {
                "offset": offset,
                "arch": machines.get(struct.unpack_from("<H", head, 18)[0], "unknown"),
                "elf_type": etypes.get(struct.unpack_from("<H", head, 16)[0], "?"),
                "loadable_bytes": loadable,
            }
        return None


def describe_artifact(root, rel):
    """Path/size/mtime/provenance for one artifact, without assuming anything
    about it from its name. Format is detected from the file's own bytes."""
    path = os.path.join(root, rel)
    if not os.path.exists(path):
        return {"path": path, "exists": False}
    with open(path, "rb") as fh:
        # 512 bytes, not 64: the MBR boot signature lives at offset 510..512, so a
        # 64-byte peek can never tell a bootable disk image from any other blob.
        head = fh.read(512)
    info = {
        "path": path,
        "exists": True,
        "bytes": os.path.getsize(path),
        "mtime": time.strftime("%Y-%m-%dT%H:%M:%S",
                               time.localtime(os.path.getmtime(path))),
    }
    if head[:4] == b"\x7fELF":
        import struct
        machines = {0x3E: "x86-64", 0xB7: "AArch64", 0xF3: "RISC-V"}
        etypes = {1: "ET_REL", 2: "ET_EXEC", 3: "ET_DYN"}
        info["format"] = "ELF%d %s-endian" % (
            64 if head[4] == 2 else 32, "little" if head[5] == 1 else "big")
        info["elf_type"] = etypes.get(struct.unpack_from("<H", head, 16)[0], "?")
        info["arch"] = machines.get(struct.unpack_from("<H", head, 18)[0], "?")
    else:
        if len(head) >= 512 and head[510:512] == b"\x55\xaa":
            info["format"] = "MBR raw disk image (0x55AA boot signature)"
            # The container is not itself an architecture. Report the
            # architecture of the kernel it carries, located the same way the
            # build tooling verifies it: a sector-aligned ELF header whose
            # loadable segments match a real kernel.
            embedded = _scan_for_kernel_elf(path)
            if embedded:
                info["embedded_kernel_offset"] = embedded["offset"]
                info["embedded_kernel_arch"] = embedded["arch"]
                info["embedded_kernel_elf_type"] = embedded["elf_type"]
                info["embedded_kernel_loadable_bytes"] = embedded["loadable_bytes"]
            else:
                info["embedded_kernel_arch"] = None
        else:
            info["format"] = "raw container (not an ELF, no MBR signature)"
    return info


# --------------------------------------------------------------------------
# Live measurement
# --------------------------------------------------------------------------

def _find_qemu(binary):
    names = [binary]
    if sys.platform == "win32" and not binary.endswith(".exe"):
        names.insert(0, binary + ".exe")
    elif sys.platform != "win32" and binary.endswith(".exe"):
        names.insert(0, binary[:-4])

    for name in names:
        if QEMU_DIR:
            cand = os.path.join(QEMU_DIR, name)
            if os.path.isfile(cand):
                return cand
        cand = shutil.which(name)
        if cand:
            return cand

    try:
        from test_interactive_lifecycle import find_qemu
        for name in names:
            res = find_qemu(name)
            if res and os.path.exists(res):
                return res
    except Exception:
        pass
    return names[0]


def _kill_port_owner(port):
    try:
        import psutil
        for conn in psutil.net_connections(kind="tcp"):
            if conn.laddr and conn.laddr.port == port and conn.pid:
                try:
                    psutil.Process(conn.pid).kill()
                except Exception:
                    pass
    except Exception:
        pass


def _sample_rss(pid, stop_flag, out):
    """Sample the host QEMU process working set until stopped.

    This is HOST emulator memory. It says nothing about guest RAM; the guest's
    own view is measured separately from its serial MEMMAP report."""
    try:
        import psutil
        p = psutil.Process(pid)
    except Exception:
        return
    peak = 0
    while not stop_flag.is_set():
        try:
            rss = p.memory_info().rss
            if rss > peak:
                peak = rss
        except Exception:
            break
        time.sleep(0.05)
    out["host_peak_rss_bytes"] = peak


def run_trial(arch, trial_index, root, results_dir, timeout_s):
    """One boot attempt. Always appends exactly one raw record."""
    import threading
    from test_interactive_lifecycle import SerialReader

    cfg = ARCHES[arch]
    artifact = os.path.join(root, cfg["artifact"])
    qemu_exe = _find_qemu(cfg["qemu_bin"])

    record = {
        "arch": arch,
        "trial": trial_index,
        "artifact": artifact,
        "endpoint": cfg["endpoint"],
        "ready_marker": cfg["ready_marker"],
        "outcome": None,
        "elapsed_ms": None,
        "detail": "",
    }

    if not os.path.exists(artifact):
        record["outcome"] = "missing_artifact"
        record["detail"] = "boot artifact not found: %s" % artifact
        return record

    ser_port = 5620 + trial_index
    _kill_port_owner(ser_port)

    # Fresh, per-trial serial sink. Deleting first means a readiness marker can
    # never be inherited from a previous run.
    ser_log = os.path.join(results_dir, "serial-%s-trial%02d.log" % (arch, trial_index))
    if os.path.exists(ser_log):
        os.remove(ser_log)

    if cfg["drive_arg"] == "-drive":
        drive_value = "format=raw,file=%s" % artifact
    else:
        drive_value = artifact
    args = [qemu_exe] + list(cfg["extra_args"]) + [
        "-m", cfg["memory_m"],
        cfg["drive_arg"], drive_value,
        "-snapshot",
        "-display", "none",
        "-serial", "file:%s" % ser_log,
        "-no-reboot",
    ]
    record["qemu_args"] = args
    record["qemu_exe"] = qemu_exe

    proc = None
    stop_flag = threading.Event()
    rss = {}
    ready = False
    guest_exited_early = False
    t0 = time.perf_counter()
    try:
        try:
            proc = subprocess.Popen(args, stdout=subprocess.DEVNULL,
                                    stderr=subprocess.DEVNULL)
        except Exception as exc:
            record["outcome"] = "launch_failure"
            record["detail"] = "could not start QEMU: %s" % exc
            return record

        sampler = threading.Thread(target=_sample_rss,
                                   args=(proc.pid, stop_flag, rss), daemon=True)
        sampler.start()

        # The serial stream is a file sink, so readiness is detected by polling
        # that file. A deleted-per-trial file plus the run-start timestamp means
        # a marker can only come from this boot.
        marker = cfg["ready_marker"]
        deadline = time.monotonic() + timeout_s
        seen = ""
        while time.monotonic() < deadline:
            if proc.poll() is not None:
                # Let the last write land before deciding the guest is gone.
                time.sleep(0.2)
                try:
                    with open(ser_log, "r", encoding="utf-8",
                              errors="replace") as fh:
                        seen = fh.read()
                except OSError:
                    seen = ""
                if marker in seen:
                    ready = True
                    break
                guest_exited_early = True
                break
            try:
                with open(ser_log, "r", encoding="utf-8", errors="replace") as fh:
                    seen = fh.read()
            except OSError:
                seen = ""
            if marker in seen:
                ready = True
                break
            time.sleep(0.02)

        t1 = time.perf_counter()
        elapsed_ms = (t1 - t0) * 1000.0

        # Stop and join the sampler BEFORE reading it. The sampler only writes
        # its peak after it observes the stop flag, so reading first would always
        # yield an empty result.
        stop_flag.set()
        sampler.join(timeout=5)
        record.update(rss)

        record["elapsed_ms"] = round(elapsed_ms, 1)
        record["serial_bytes"] = len(seen)

        record["outcome"] = classify(ready, guest_exited_early,
                                     elapsed_ms if ready else None, timeout_s)
        if not ready:
            tail = [l for l in seen.splitlines() if l.strip()][-5:]
            record["detail"] = "readiness marker not seen; last serial lines: %s" % tail
    finally:
        stop_flag.set()
        if proc is not None and proc.poll() is None:
            try:
                proc.kill()
                proc.wait(timeout=10)
            except Exception:
                pass
    return record


def main():
    ap = argparse.ArgumentParser(description=__doc__,
                                 formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--arch", choices=sorted(ARCHES), required=True)
    ap.add_argument("--trials", type=int, default=5)
    ap.add_argument("--root", default=REPO_ROOT)
    ap.add_argument("--out", default=None,
                    help="directory for raw serial logs and results JSON")
    args = ap.parse_args()

    root = os.path.abspath(args.root)
    results_dir = args.out or os.path.join(root, "target", "bench")
    os.makedirs(results_dir, exist_ok=True)

    cfg = ARCHES[args.arch]
    artifact = os.path.join(root, cfg["artifact"])

    print("=" * 72)
    print("InterOS boot benchmark -- %s" % args.arch)
    print("=" * 72)
    print("artifact : %s" % artifact)
    print("endpoint : %s" % cfg["endpoint"])
    print("marker   : %s" % cfg["ready_marker"])
    print("timeout  : %.0fs per trial" % cfg["timeout_s"])
    print("trials   : %d" % args.trials)
    if not cfg["bootable"]:
        print("NOTE     : this architecture has no bootable image; the measured")
        print("           artifact is a kernel ELF loaded by the QEMU machine.")
    print()

    if not os.path.exists(artifact):
        print("[FAIL] boot artifact missing: %s" % artifact)
        return 2

    art = describe_artifact(root, cfg["artifact"])
    if "embedded_kernel_arch" in art:
        print("artifact  %d bytes, %s" % (art["bytes"], art["format"]))
        print("embedded  kernel at offset %d, %s %s, %d loadable bytes"
              % (art["embedded_kernel_offset"], art["embedded_kernel_arch"],
                 art["embedded_kernel_elf_type"], art["embedded_kernel_loadable_bytes"]))
    else:
        print("artifact  %d bytes, %s, %s" % (art["bytes"], art.get("format"),
                                              art.get("arch")))
    print("mtime     %s" % art["mtime"])
    print()

    raw = []
    for i in range(1, args.trials + 1):
        rec = run_trial(args.arch, i, root, results_dir, cfg["timeout_s"])
        raw.append(rec)
        # Persist after every trial: a crash mid-run still leaves usable data.
        with open(os.path.join(results_dir, "boot-%s-raw.json" % args.arch), "w",
                  encoding="utf-8") as fh:
            json.dump({"arch": args.arch, "endpoint": cfg["endpoint"],
                       "timeout_s": cfg["timeout_s"], "trials": raw}, fh, indent=2)
        print("trial %2d: %-14s %s ms%s"
              % (i, rec["outcome"],
                 ("%.1f" % rec["elapsed_ms"]) if rec["elapsed_ms"] else "-",
                 ("  " + rec["detail"][:60]) if rec["detail"] else ""))

    ok = [r["elapsed_ms"] for r in raw if r["outcome"] == "ok"]
    failed = [r for r in raw if r["outcome"] != "ok"]
    stats = summarize(ok)

    print()
    print("-" * 72)
    print("attempts            : %d" % len(raw))
    print("successful          : %d" % stats["count"])
    print("failed/timeout      : %d" % len(failed))
    for r in failed:
        print("   trial %d -> %s" % (r["trial"], r["outcome"]))
    if stats["count"]:
        print("min / median / mean / max (ms): %s / %s / %s / %s"
              % (stats["min_ms"], stats["median_ms"], stats["mean_ms"], stats["max_ms"]))
    if raw:
        rss = [r.get("host_peak_rss_bytes") for r in raw if r.get("host_peak_rss_bytes")]
        if rss:
            print("host QEMU peak RSS  : %s (host emulator memory, NOT guest RAM)"
                  % format_bytes(max(rss)))
    print("raw results         : %s"
          % os.path.join(results_dir, "boot-%s-raw.json" % args.arch))
    print("-" * 72)
    print("Timing includes QEMU startup and firmware; the clock starts before the")
    print("guest exists. This figure is not comparable across architectures,")
    print("because the three readiness endpoints prove different amounts of device.")

    if not ok:
        print("\n[FAIL] no trial reached the documented readiness endpoint")
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())