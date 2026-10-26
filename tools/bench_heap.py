"""Reads the kernel's heap accounting from a live guest at documented checkpoints.

WHY this measures rather than estimates. `kernel/src/allocator.rs` exposes
`heap_stats()`, which returns `LockedHeap::used()` and `LockedHeap::free()` from
`linked_list_allocator`. Those are the allocator's own bookkeeping: `used` is the
sum of the linked-list nodes it is currently holding and `free` is
`HEAP_SIZE - used`. They are not derived from total physical memory, from QEMU's
RSS, or from the configured capacity, and the host configuration says so
explicitly in its `capability` field.

The invariant `used + free == HEAP_SIZE` is checked on every reading rather than
assumed, so a parser that silently mis-reads the line fails instead of producing
a plausible number.

This is used and free. It is not a high-water mark: `linked_list_allocator`
retains freed nodes on its free list, so a peak that has since been released is
not observable from this interface without changing the allocator.
"""

import argparse
import json
import os
import socket
import subprocess
import sys
import time

REPO_ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
sys.path.insert(0, os.path.join(REPO_ROOT, "tools"))

from bench_boot import _find_qemu, _kill_port_owner, describe_artifact  # noqa: E402

HEAP_SIZE = 64 * 1024 * 1024  # kernel/src/allocator.rs: HEAP_SIZE

# WHY these checkpoints: each is a marker the guest itself prints on the real
# path, so a reading is attributed to a known state rather than to a sleep.
CHECKPOINTS = [
    ("services_up", "Step 5: Initial presentation complete",
     "launcher presented its first frame; services are running"),
    ("after_app_launch", "[CALCULATOR] Initial frame presented to hardware scanout.",
     "calculator launched and presented its first frame"),
]

DIAG_STATE_COMMAND = "DIAG_STATE"
DIAG_STATE_MARKER = "[DIAG_STATE]"


def parse_diag_state(line):
    """Parse one `[DIAG_STATE]` line into a dict, or None if it is not one.

    Returns None rather than a partial dict for a line that lacks the fields this
    tool reports, so an unrelated serial line cannot be mistaken for a reading.
    """
    if DIAG_STATE_MARKER not in line:
        return None
    fields = {}
    for token in line.split():
        if "=" not in token:
            continue
        key, _, value = token.partition("=")
        fields[key] = value
    try:
        return {
            "heap_used": int(fields["heap_used"]),
            "heap_free": int(fields["heap_free"]),
            "tasks_total": int(fields.get("tasks_total", -1)),
            "tasks_active": int(fields.get("tasks_active", -1)),
            "surfaces": int(fields.get("surfaces", -1)),
            # Compositor cost of the most recent flip, published by every flip
            # rather than only the sampled ones. -1 means the kernel did not
            # report them, so an older guest stays readable without a field
            # silently defaulting to zero and looking like a free frame.
            "last_flip": int(fields.get("last_flip", -1)),
            "last_flip_compose_cycles": int(fields.get("last_flip_compose_cycles", -1)),
            "last_flip_present_cycles": int(fields.get("last_flip_present_cycles", -1)),
        }
    except (KeyError, ValueError):
        return None


def check_invariant(reading):
    """Return the problem with a reading, or None when it is self-consistent."""
    if reading is None:
        return "reading is None"
    total = reading["heap_used"] + reading["heap_free"]
    if total != HEAP_SIZE:
        return "used+free=%d does not equal the configured heap size %d" % (total, HEAP_SIZE)
    return None


def summarize_readings(readings):
    """Summarize the readings, refusing to average over inconsistent ones."""
    problems = [(r["checkpoint"], check_invariant(r["reading"]))
                for r in readings if check_invariant(r["reading"])]
    valid = [r["reading"] for r in readings if check_invariant(r["reading"]) is None]
    return {
        "checkpoints": len(readings),
        "consistent_readings": len(valid),
        "inconsistent": problems,
        "heap_size_bytes": HEAP_SIZE,
        "peak_used_bytes": max((r["heap_used"] for r in valid), default=None),
        "final_used_bytes": valid[-1]["heap_used"] if valid else None,
        "final_free_bytes": valid[-1]["heap_free"] if valid else None,
        "peak_utilisation_pct": (
            round(100.0 * max(r["heap_used"] for r in valid) / HEAP_SIZE, 4) if valid else None
        ),
    }


def run(root, timeout_s, boot_timeout_s, results_dir):
    artifact = os.path.join(root, "target", "inter-os-bios.img")
    if not os.path.exists(artifact):
        raise SystemExit("boot artifact not found: %s" % artifact)

    qemu_exe = _find_qemu("qemu-system-x86_64.exe")
    port = 5790
    _kill_port_owner(port)
    ser_log = os.path.join(results_dir, "heap-serial.log")

    args = [
        qemu_exe, "-machine", "q35", "-cpu", "qemu64", "-m", "1024M",
        "-drive", "format=raw,file=%s" % artifact,
        "-snapshot", "-display", "none", "-no-reboot",
        "-chardev", "socket,id=ser0,host=127.0.0.1,port=%d,server=on,wait=off" % port,
        "-serial", "chardev:ser0",
    ]

    proc = subprocess.Popen(args, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    sock = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    sock.settimeout(1.0)
    deadline = time.monotonic() + 20.0
    while True:
        try:
            sock.connect(("127.0.0.1", port))
            break
        except OSError:
            if time.monotonic() > deadline or proc.poll() is not None:
                raise SystemExit("could not connect to the guest serial socket")
            time.sleep(0.05)
    sock.setsockopt(socket.IPPROTO_TCP, socket.TCP_NODELAY, 1)

    buf = b""
    transcript = []

    def pump(seconds):
        nonlocal buf
        end = time.monotonic() + seconds
        while time.monotonic() < end:
            try:
                data = sock.recv(8192)
            except socket.timeout:
                continue
            except OSError:
                return
            if not data:
                return
            buf += data
            while b"\n" in buf:
                raw, buf = buf.split(b"\n", 1)
                text = raw.decode("utf-8", errors="replace").strip()
                if text:
                    transcript.append((time.perf_counter(), text))

    def wait_for(marker, timeout):
        end = time.monotonic() + timeout
        while time.monotonic() < end:
            for _at, text in transcript:
                if marker in text:
                    return True
            pump(0.05)
        return any(marker in text for _at, text in transcript)

    def ask_diag_state():
        start = len(transcript)
        sock.sendall((DIAG_STATE_COMMAND + "\n").encode("utf-8"))
        end = time.monotonic() + 10.0
        while time.monotonic() < end:
            pump(0.1)
            for _at, text in transcript[start:]:
                reading = parse_diag_state(text)
                if reading:
                    return reading
        return None

    readings = []

    def record(name, description, reading):
        readings.append({"checkpoint": name, "description": description, "reading": reading})

    try:
        # Checkpoint 1: services running and the launcher has presented a frame.
        boot_marker = CHECKPOINTS[0][1]
        if not wait_for(boot_marker, boot_timeout_s):
            raise SystemExit("guest never reached the launcher-ready marker %r" % boot_marker)
        pump(0.5)
        record(CHECKPOINTS[0][0], CHECKPOINTS[0][2], ask_diag_state())

        # Checkpoint 2: a real application launched and presented its first frame.
        sock.sendall(b"APP_LAUNCH calculator\n")
        if not wait_for(CHECKPOINTS[1][1], boot_timeout_s):
            raise SystemExit("guest never reached the app-ready marker %r" % CHECKPOINTS[1][1])
        pump(0.5)
        record(CHECKPOINTS[1][0], CHECKPOINTS[1][2], ask_diag_state())

        for entry in readings:
            reading = entry["reading"]
            print("[heap] %-18s %s" % (
                entry["checkpoint"],
                "no [DIAG_STATE] response" if reading is None
                else "used=%d free=%d tasks=%d" % (reading["heap_used"], reading["heap_free"],
                                                   reading["tasks_total"])), flush=True)
    finally:
        with open(ser_log, "w", encoding="utf-8") as fh:
            origin = transcript[0][0] if transcript else 0.0
            for at, text in transcript:
                fh.write("%+9.1f  %s\n" % ((at - origin) * 1000.0, text))
        try:
            proc.kill()
            proc.wait(timeout=10)
        except Exception:
            pass

    payload = {
        "tool": "bench_heap",
        "arch": "x86_64",
        "source": "kernel/src/allocator.rs heap_stats() via the DIAG_STATE serial command",
        "includes": "bytes the allocator is currently holding as list nodes",
        "excludes": (
            "freed nodes retained on the allocator free list, kernel static memory, "
            "stack frames, and anything outside the kernel heap; NOT a high-water mark"
        ),
        "heap_size_bytes": HEAP_SIZE,
        "artifact": describe_artifact(root, os.path.join("target", "inter-os-bios.img")),
        "serial_log": ser_log,
        "readings": readings,
        "summary": summarize_readings(readings),
    }
    out_path = os.path.join(results_dir, "heap.json")
    with open(out_path, "w", encoding="utf-8") as fh:
        json.dump(payload, fh, indent=2)
    return payload


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--out", default=os.path.join(REPO_ROOT, "target", "bench", "heap"))
    parser.add_argument("--timeout", type=float, default=60.0)
    parser.add_argument("--boot-timeout", type=float, default=180.0)
    args = parser.parse_args()
    os.makedirs(args.out, exist_ok=True)

    payload = run(REPO_ROOT, args.timeout, args.boot_timeout, args.out)
    summary = payload["summary"]
    print("\n=== kernel heap (x86_64) ===")
    print("configured heap : %d bytes (64.0 MiB)" % summary["heap_size_bytes"])
    print("consistent reads : %d of %d checkpoints" % (summary["consistent_readings"],
                                                        summary["checkpoints"]))
    for checkpoint, problem in summary["inconsistent"]:
        print("  INCONSISTENT %s: %s" % (checkpoint, problem))
    if summary["peak_used_bytes"] is not None:
        print("peak used        : %d bytes (%.4f%% of the heap)"
              % (summary["peak_used_bytes"], summary["peak_utilisation_pct"]))
        print("final used/free  : %d / %d bytes" % (summary["final_used_bytes"],
                                                      summary["final_free_bytes"]))
    print("raw              : %s" % os.path.join(args.out, "heap.json"))
    return 0 if summary["consistent_readings"] else 1


if __name__ == "__main__":
    sys.exit(main())