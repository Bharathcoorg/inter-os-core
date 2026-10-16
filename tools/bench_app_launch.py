"""Measures native application launch latency in the real x86_64 QEMU guest.

WHY this is separate from `tools/bench_boot.py`: boot timing needs a serial
*file* sink and polls it, while launching an app needs a serial *socket* so a
command can be written to the guest. The measurement state machine, the
classification and the summary statistics are imported from `bench_boot.py`
rather than reimplemented, so both tools report the same way.

WHY there is exactly one attempt per trial. `tests/test_interactive_lifecycle.py`
re-sends a tap when the first one does not produce the expected marker, which is
correct for a pass/fail lifecycle test and fatal for a latency number: a retry
measures the second attempt plus whatever went wrong in the first, and the result
still reads as a clean single-shot sample. Here `retries` exists only so a record
carrying one can be invalidated, and nothing in this file ever increments it.

Every timestamp comes from `time.perf_counter()`, which is monotonic, so a wall
clock adjustment during a trial cannot produce a negative or inflated interval.

Endpoint naming matters more than the number. The end of the interval is the
moment the host reads the application's own log line, emitted by the app after
`surface_flip` returned. That call composes the surfaces, programs the scanout
and writes the hardware framebuffer (`sys_surface_flip` ->
`KmsDisplayDriver::composite_and_flip`), so the endpoint is a frame presented to
the scanout path and not merely a buffer queued for later. It is still not
photon latency: QEMU runs with `-display none` and nothing can observe when
pixels leave the emulated device.
"""

import argparse
import json
import os
import socket
import statistics  # noqa: F401  (re-exported for callers that summarise launch results)
import subprocess
import sys
import time

REPO_ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
sys.path.insert(0, os.path.join(REPO_ROOT, "tools"))

from bench_boot import (  # noqa: E402
    _find_qemu,
    _kill_port_owner,
    describe_artifact,
    summarize,
)

# WHY these markers: each is printed by the guest itself on the real code path.
# `launch_ack` is the kernel confirming the task was created, `process_entry` is
# the task dropping to Ring 3, and `ready` is the application reporting that its
# first frame went out through `surface_flip`. None is a timer or a sleep.
APP_TARGETS = {
    "calculator": {
        "launch_id": "calculator",
        "launch_ack": "[PROCESS] Guest application launched id=calculator",
        "process_entry": "[PROCESS ENTRY]",
        "ready": "[CALCULATOR] Initial frame presented to hardware scanout.",
        "endpoint": (
            "host wrote the launch command to the guest serial socket -> the "
            "application returned from SurfaceFlip, which composited its "
            "surfaces, programmed the scanout and wrote the hardware "
            "framebuffer"
        ),
    },
    "card_demo": {
        "launch_id": "card_demo",
        "launch_ack": "[PROCESS] Guest application launched id=card_demo",
        "process_entry": "[PROCESS ENTRY]",
        "ready": "[CARD_DEMO] Initial frame presented via hardware scanout.",
        "endpoint": (
            "host wrote the launch command to the guest serial socket -> the "
            "application returned from surface_flip, which composited its "
            "surfaces, programmed the scanout and wrote the hardware "
            "framebuffer"
        ),
    },
}

# The guest must be fully up before a launch means anything; timing a launch
# against a half-booted device would measure boot, not launch.
BOOT_READY_MARKER = "Step 5: Initial presentation complete"

QEMU_ARGS_BASE = [
    "-machine", "q35",
    "-cpu", "qemu64",
    "-m", "1024M",
    "-display", "none",
    "-snapshot",
    "-no-reboot",
]


# ---------------------------------------------------------------------------
# Pure helpers: no QEMU, no sockets. Exercised by tests/test_app_launch_bench.py.
# ---------------------------------------------------------------------------

def phase_offsets(sent_at, marks):
    """Convert absolute monotonic marker times into deltas from the send.

    WHY deltas and not absolutes in the record: the absolute clock is meaningless
    outside the run that produced it, while the deltas are the measurement. The
    send instant is the origin because that is the last moment the host controls.
    """
    out = {}
    for name, at in marks.items():
        if at is None or sent_at is None:
            out[name + "_ms"] = None
        else:
            out[name + "_ms"] = round((at - sent_at) * 1000.0, 1)
    return out


def classify_launch(outcome, retries):
    """Classify a launch trial so no failure can read as a successful sample.

    A trial that needed a retry is `invalid_retry` even if it eventually reached
    readiness: the interval would describe the retry, not the launch.
    """
    if retries:
        return "invalid_retry"
    return outcome


def is_valid_sample(record):
    """True only for a trial that reached readiness on its single attempt."""
    return record.get("outcome") == "ok" and record.get("retries", 0) == 0


def newest_source_mtime(root):
    """Modification time of the most recently changed build input in the tree.

    WHY this exists rather than reusing `bench_boot.is_stale`: that helper asks
    "was this artifact produced by the run that is asking", which only makes sense
    when the tool itself did the build. This tool consumes an image built earlier,
    so the meaningful question is the opposite -- does the image still describe
    this source tree, or would the number be attributed to code that has since
    changed?

    Only directories whose contents are embedded in the image are scanned. Build
    and benchmark scripts are excluded on purpose: editing this file must not
    invalidate an image that does not contain it.
    """
    roots = ["apps", "kernel", "kernel-aarch64", "kernel-riscv64", "sdk", "boot"]
    newest = 0.0
    for entry in roots:
        path = os.path.join(root, entry)
        if not os.path.exists(path):
            continue
        if os.path.isfile(path):
            newest = max(newest, os.path.getmtime(path))
            continue
        for dirpath, dirnames, filenames in os.walk(path):
            dirnames[:] = [d for d in dirnames
                           if d not in ("target", ".git", "node_modules")]
            for name in filenames:
                if not name.endswith((".rs", ".toml", ".ld", ".ps1", ".py")):
                    continue
                try:
                    newest = max(newest, os.path.getmtime(os.path.join(dirpath, name)))
                except OSError:
                    continue
    return newest


def image_predates_sources(artifact_path, root):
    """True when the boot image is older than the sources it should embed."""
    if not os.path.exists(artifact_path):
        return True
    return os.path.getmtime(artifact_path) < newest_source_mtime(root)


def summarize_trials(records):
    """Aggregate per-phase statistics over valid trials only.

    Invalid and failed trials stay visible as counts rather than being dropped
    silently, because a latency figure with no denominator is not a measurement.
    """
    valid = [r for r in records if is_valid_sample(r)]
    failed = [r for r in records if not is_valid_sample(r)]
    result = {
        "attempts": len(records),
        "valid_samples": len(valid),
        "failed_or_invalid": len(failed),
    }
    for phase in ("launch_ack", "process_entry", "ready"):
        samples = [r[phase + "_ms"] for r in valid if r.get(phase + "_ms") is not None]
        result[phase] = summarize(samples)
    result["failure_reasons"] = sorted({r.get("detail", "unspecified")[:80] for r in failed})
    return result


class LaunchSession:
    """One serial conversation with the guest, with no retry anywhere.

    WHY `consumed` grows: a readiness marker can legitimately appear more than
    once over a long-lived session (an app that re-presents a frame logs again).
    Matching against the whole buffer would let an *earlier* occurrence satisfy a
    later request, so a trial would be scored on a frame from a previous one. Each
    `await_marker` call only ever considers bytes that arrived after it started.
    """

    def __init__(self, sock, timeout_s=0.5):
        self.sock = sock
        self.timeout_s = timeout_s
        self.buf = b""
        self.lines = []

    def send_once(self, command):
        """Write one command line and stamp the instant. Never retries."""
        payload = (command + "\n").encode("utf-8")
        sent_at = time.perf_counter()
        self.sock.sendall(payload)
        return sent_at

    def drain(self, quiet_s=0.4, max_s=3.0):
        """Read pending guest output until it goes quiet.

        WHY this exists before a trial's send instant: anything still sitting in
        the socket buffer belongs to boot. Draining it means the post-send
        `await_marker` can only ever match output this trial caused, so a boot-time
        line cannot be mistaken for the app having launched.
        """
        hard_deadline = time.monotonic() + max_s
        quiet_deadline = time.monotonic() + quiet_s
        while time.monotonic() < hard_deadline and time.monotonic() < quiet_deadline:
            try:
                data = self.sock.recv(4096)
            except socket.timeout:
                break
            except OSError:
                break
            if not data:
                break
            batch_at = time.perf_counter()
            self.buf += data
            while b"\n" in self.buf:
                raw, self.buf = self.buf.split(b"\n", 1)
                text = raw.decode("utf-8", errors="replace").strip()
                if text:
                    self.lines.append((batch_at, text))
            quiet_deadline = time.monotonic() + quiet_s
        return len(self.lines)

    def await_marker(self, marker, timeout_s):
        """Wait for one marker, stamping when it was read. Never retries.

        Returns the read instant in monotonic seconds, or None on timeout.
        """
        deadline = time.monotonic() + timeout_s
        seen_at = None
        while time.monotonic() < deadline:
            try:
                data = self.sock.recv(4096)
            except socket.timeout:
                continue
            except OSError:
                return seen_at
            if not data:
                return seen_at
            self.buf += data
            # WHY one timestamp for the whole batch: these lines became visible
            # to the host at the same instant. Stamping each line separately
            # would invent an ordering the host cannot observe and would report a
            # sub-millisecond separation between two events it only knows arrived
            # together.
            batch_at = time.perf_counter()
            while b"\n" in self.buf:
                raw, self.buf = self.buf.split(b"\n", 1)
                text = raw.decode("utf-8", errors="replace").strip()
                if not text:
                    continue
                self.lines.append((batch_at, text))
                if seen_at is None and marker in text:
                    seen_at = batch_at
            if seen_at is not None:
                return seen_at
        return seen_at

    def first_mark_time_after(self, marker, after_time):
        """Timestamp of the first `marker` read at or after `after_time`.

        WHY this is not "any marker in the session": `[PROCESS ENTRY]` is printed
        by every task that drops to Ring 3, including the services that start
        during boot. Matching it anywhere in the session credits a boot-time
        service as the launched application. Anchoring it after the launch ack
        keeps the phase on the task that this trial actually created.

        `>=` rather than `>` on purpose. The kernel prints the launch
        acknowledgement and the new task's Ring 3 transition microseconds apart,
        so both usually land in one serial read batch and share a timestamp.
        Excluding equal timestamps would drop the phase entirely instead of
        reporting it as unresolvable, and `phases_in_one_read_batch` is what says
        the separation is below what the host can measure.
        """
        for at, text in self.lines:
            if at >= after_time and marker in text:
                return at
        return None


# ---------------------------------------------------------------------------
# QEMU driving.
# ---------------------------------------------------------------------------

def run_trial(app, trial_index, root, results_dir, timeout_s, boot_timeout_s):
    """Boot a fresh guest, launch the app once, and record one raw trial."""
    import threading

    cfg = APP_TARGETS[app]
    artifact = os.path.join(root, "target", "inter-os-bios.img")
    qemu_exe = _find_qemu("qemu-system-x86_64.exe")

    record = {
        "app": app,
        "trial": trial_index,
        "arch": "x86_64",
        "endpoint": cfg["endpoint"],
        "ready_marker": cfg["ready"],
        "outcome": None,
        "retries": 0,
        "detail": "",
        "sent_at_ms": None,
        "launch_ack_ms": None,
        "process_entry_ms": None,
        "ready_ms": None,
        "phases_in_one_read_batch": None,
    }

    if not os.path.exists(artifact):
        record["outcome"] = "missing_artifact"
        record["detail"] = "boot artifact not found: %s" % artifact
        return record

    ser_port = 5740 + trial_index
    _kill_port_owner(ser_port)

    ser_log = os.path.join(results_dir, "launch-%s-trial%02d.log" % (app, trial_index))
    if os.path.exists(ser_log):
        os.remove(ser_log)
    record["serial_log"] = ser_log

    args = [qemu_exe] + QEMU_ARGS_BASE + [
        "-drive", "format=raw,file=%s" % artifact,
        # The socket server is what lets the host write a launch command; the
        # transcript is mirrored to a file so the raw guest output survives.
        "-chardev", "socket,id=ser0,host=127.0.0.1,port=%d,server=on,wait=off" % ser_port,
        "-serial", "chardev:ser0",
    ]
    record["qemu_args"] = args
    record["qemu_exe"] = qemu_exe

    proc = None
    session = None
    try:
        try:
            proc = subprocess.Popen(args, stdout=subprocess.DEVNULL,
                                    stderr=subprocess.DEVNULL)
        except Exception as exc:
            record["outcome"] = "launch_failure"
            record["detail"] = "could not start QEMU: %s" % exc
            return record

        sock = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
        sock.settimeout(1.0)
        connect_deadline = time.monotonic() + 20.0
        connected = False
        while time.monotonic() < connect_deadline:
            try:
                sock.connect(("127.0.0.1", ser_port))
                connected = True
                break
            except OSError:
                if proc.poll() is not None:
                    break
                time.sleep(0.05)
        if not connected:
            record["outcome"] = "launch_failure"
            record["detail"] = "could not connect to the guest serial socket"
            return record
        sock.setsockopt(socket.IPPROTO_TCP, socket.TCP_NODELAY, 1)
        session = LaunchSession(sock)
        transcript = []

        if session.await_marker(BOOT_READY_MARKER, boot_timeout_s) is None:
            record["outcome"] = "boot_timeout"
            record["detail"] = "guest never reached %r" % BOOT_READY_MARKER
            return record

        # Let boot-time chatter drain so a launch marker cannot be satisfied by
        # a line the guest emitted before this trial's command.
        session.drain()
        before = len(session.lines)

        sent_at = session.send_once("APP_LAUNCH %s" % cfg["launch_id"])
        record["sent_at_ms"] = 0.0

        ack_at = session.await_marker(cfg["launch_ack"], timeout_s)
        if ack_at is None:
            record["outcome"] = "launch_ack_timeout"
            record["detail"] = "kernel never acknowledged the launch of %s" % cfg["launch_id"]
            record.update(phase_offsets(sent_at, {"launch_ack": None, "process_entry": None,
                                                  "ready": None}))
            return record

        entry_at = session.first_mark_time_after(cfg["process_entry"], ack_at)
        ready_at = session.await_marker(cfg["ready"], timeout_s)

        # WHY this flag exists: every line read in one recv() batch gets the same
        # timestamp, so two phases that land in one batch report an identical delta
        # that is really "faster than this host can resolve". Publishing the number
        # without saying so would present a measurement artefact as a result.
        record["phases_in_one_read_batch"] = bool(
            entry_at is not None and abs(entry_at - ack_at) < 1e-9
        )

        record.update(phase_offsets(sent_at, {
            "launch_ack": ack_at,
            "process_entry": entry_at,
            "ready": ready_at,
        }))
        record["sent_at_ms"] = 0.0

        if ready_at is None:
            record["outcome"] = "ready_timeout"
            record["detail"] = (
                "task was created (ack at %s ms) but the app never reported its "
                "first frame" % record["launch_ack_ms"]
            )
        else:
            record["outcome"] = "ok"
        return record
    finally:
        # The raw, timestamped guest transcript is the raw data behind the number.
        # It is written for failed trials too, so a timeout can be inspected
        # rather than simply re-run.
        if session is not None:
            origin = session.lines[0][0] if session.lines else 0.0
            with open(ser_log, "w", encoding="utf-8") as fh:
                fh.write("# app=%s trial=%d arch=x86_64\n" % (app, trial_index))
                fh.write("# t+ms is host monotonic time relative to the first line read\n")
                for at, text in session.lines:
                    fh.write("%+9.1f  %s\n" % ((at - origin) * 1000.0, text))
        if proc is not None and proc.poll() is None:
            try:
                proc.kill()
                proc.wait(timeout=10)
            except Exception:
                pass


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--app", default="calculator", choices=sorted(APP_TARGETS),
                        help="application to launch")
    parser.add_argument("--trials", type=int, default=10,
                        help="number of fresh-boot launch trials")
    parser.add_argument("--timeout", type=float, default=60.0,
                        help="seconds to wait for each launch phase")
    parser.add_argument("--boot-timeout", type=float, default=180.0,
                        help="seconds to wait for the launcher to reach readiness")
    parser.add_argument("--out", default=os.path.join(REPO_ROOT, "target", "bench",
                                                      "app-launch"))
    args = parser.parse_args()

    run_start = time.time()
    results_dir = args.out
    os.makedirs(results_dir, exist_ok=True)

    artifact = os.path.join(REPO_ROOT, "target", "inter-os-bios.img")
    if not os.path.exists(artifact):
        print("ERROR: boot artifact not found: %s" % artifact, file=sys.stderr)
        print("Run `dev.ps1 build` (or `qemu/build.ps1`) first.", file=sys.stderr)
        return 2
    if image_predates_sources(artifact, REPO_ROOT):
        newest = time.strftime("%Y-%m-%d %H:%M:%S",
                               time.localtime(newest_source_mtime(REPO_ROOT)))
        built = time.strftime("%Y-%m-%d %H:%M:%S",
                              time.localtime(os.path.getmtime(artifact)))
        print("ERROR: boot artifact %s was built at %s but sources changed at %s."
              % (artifact, built, newest), file=sys.stderr)
        print("Rebuild before measuring, or the result describes code that is no "
              "longer in the tree.", file=sys.stderr)
        return 2

    identity = describe_artifact(REPO_ROOT, os.path.join("target", "inter-os-bios.img"))

    records = []
    for trial in range(1, args.trials + 1):
        print("[bench] %s trial %d/%d" % (args.app, trial, args.trials), flush=True)
        record = run_trial(args.app, trial, REPO_ROOT, results_dir,
                           args.timeout, args.boot_timeout)
        records.append(record)
        print("[bench]   outcome=%s retries=%d ack=%sms entry=%sms ready=%sms %s"
              % (record["outcome"], record["retries"], record["launch_ack_ms"],
                 record["process_entry_ms"], record["ready_ms"], record["detail"]),
              flush=True)

    summary = summarize_trials(records)
    payload = {
        "tool": "bench_app_launch",
        "app": args.app,
        "arch": "x86_64",
        "endpoint": APP_TARGETS[args.app]["endpoint"],
        "ready_marker": APP_TARGETS[args.app]["ready"],
        "boot_ready_marker": BOOT_READY_MARKER,
        "measurement": "host write of APP_LAUNCH -> guest marker read by host",
        "clock": "time.perf_counter (monotonic)",
        "artifact": identity,
        "raw": records,
        "summary": summary,
    }
    out_path = os.path.join(results_dir, "%s.json" % args.app)
    with open(out_path, "w", encoding="utf-8") as fh:
        json.dump(payload, fh, indent=2)

    print("\n=== app launch: %s (x86_64) ===" % args.app)
    print("endpoint : %s" % payload["endpoint"])
    print("raw      : %s" % out_path)
    print("attempts : %d   valid : %d   failed/invalid : %d"
          % (summary["attempts"], summary["valid_samples"], summary["failed_or_invalid"]))
    for phase, label in (("launch_ack", "kernel task created"),
                         ("process_entry", "task entered Ring 3"),
                         ("ready", "app first frame presented")):
        stats = summary[phase]
        if stats["count"]:
            print("%-26s n=%-3d median=%-8s mean=%-8s min=%-8s max=%s ms"
                  % (label, stats["count"], stats["median_ms"], stats["mean_ms"],
                     stats["min_ms"], stats["max_ms"]))
        else:
            print("%-26s no valid samples" % label)
    if summary["failure_reasons"]:
        print("failures : %s" % "; ".join(summary["failure_reasons"]))
    return 0 if summary["valid_samples"] else 1


if __name__ == "__main__":
    sys.exit(main())