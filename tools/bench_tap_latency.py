"""Measures calculator tap-to-post-flip latency in the real x86_64 QEMU guest.

Metric: `tap_to_post_surface_flip_ms`.

    start : the host writes one `TAP x y` line to the guest serial socket
    end   : the host reads the calculator's own marker, logged immediately
            after `surface_flip` returned for the redraw that tap caused

WHAT THIS IS NOT. This is not input-to-photon latency, not hardware scanout
completion, and not physical display latency. `surface_flip` returns after
`KmsDisplayDriver::composite_and_flip` has committed the surfaces, composited
them into the front buffer, swapped front and back, and written the composited
image into the guest boot framebuffer. That is the strongest endpoint the
existing code can guarantee: no vsync is awaited, no readback confirms a
controller fetched the pixels, and QEMU runs with `-display none`, so nothing
is ever emitted as light. A consumer of this number must not read it as
photon latency.

WHY a marker had to be added. Every calculator log line for a key press is
emitted while handling the tap, which is before `repaint` and `surface_flip`.
Nothing in the app could therefore show that the frame carrying the new state
reached the display path, and `[KMS PERF]` cannot stand in: it logs only the
first 15 flips and every 30th after that, so the flip behind an arbitrary input
is almost never observed.

WHY correlation is sound. The marker is emitted only inside the
`dirty != Dirty::None` branch, which is the only path in the app that repaints,
so it cannot come from an inert tap or from any periodic redraw. The trial
records how many markers the guest had already printed, waits for exactly one
more, and separately requires the kernel's `[COM1 INPUT] Hardware touch tap
received at (x, y)` line and the app's own `[CALCULATOR] Keypad digit pressed.`
line after the send. A tap that missed the keypad, was rejected as out of
bounds, or produced no redraw therefore fails the trial instead of being
scored against some other frame.

The TAP is sent exactly once. `retries` exists only so a record that somehow
acquired one could be invalidated, and nothing here increments it.
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

from bench_app_launch import (  # noqa: E402
    BOOT_READY_MARKER,
    QEMU_ARGS_BASE,
    LaunchSession,
    image_predates_sources,
    newest_source_mtime,
)
from bench_boot import _find_qemu, _kill_port_owner, describe_artifact, summarize  # noqa: E402
from bench_heap import parse_diag_state  # noqa: E402

# Digit '7' is keypad row 1, column 0. COL_X[0]=28 + BTN_W/2=72 -> x=100;
# ROW_Y[1]=610 + BTN_H/2=70 -> y=680. Derived from the app's own geometry
# constants rather than guessed, so a tap cannot land between two keys.
KEYPAD_X = 28 + 144 // 2
KEYPAD_Y = 610 + 140 // 2

APP_LAUNCH_ID = "calculator"
APP_READY_MARKER = "[CALCULATOR] Initial frame presented to hardware scanout."
INPUT_ACCEPTED_MARKER = "[COM1 INPUT] Hardware touch tap received at (%d, %d)."
STATE_CHANGED_MARKER = "[CALCULATOR] Keypad digit pressed."
POST_FLIP_MARKER = "[CALCULATOR] Post-input frame presented."
DIAG_STATE_MARKER = "[DIAG_STATE]"

METRIC_NAME = "tap_to_post_surface_flip_ms"
ENDPOINT = (
    "host wrote TAP %d %d to the guest serial socket -> the calculator returned "
    "from surface_flip for the redraw that tap caused, so its pixels were "
    "composited, the buffers swapped and the image written into the guest boot "
    "framebuffer" % (KEYPAD_X, KEYPAD_Y)
)
ENDPOINT_NOTE = (
    "not input-to-photon latency: surface_flip awaits no vsync and confirms no "
    "scanout fetch, and QEMU runs with -display none"
)


def classify(outcome, retries):
    """A retried trial is invalid even if it eventually produced a marker."""
    if retries:
        return "invalid_retry"
    return outcome


def is_valid_sample(record):
    return record.get("outcome") == "ok" and record.get("retries", 0) == 0


# Summary key -> the per-trial field it is computed from. The metric is its own
# key; the intermediate phases are stored with a unit suffix.
PHASES = (
    ("input_accepted", "input_accepted_ms"),
    ("state_changed", "state_changed_ms"),
    (METRIC_NAME, METRIC_NAME),
)


def summarize_trials(records):
    """Summaries over valid trials only, with every attempt still counted."""
    valid = [r for r in records if is_valid_sample(r)]
    failed = [r for r in records if not is_valid_sample(r)]
    out = {
        "attempts": len(records),
        "valid_samples": len(valid),
        "failed_or_invalid": len(failed),
    }
    for key, field in PHASES:
        samples = [r.get(field) for r in valid if r.get(field) is not None]
        out[key] = summarize(samples)
    costs = [r["guest_frame_cycles"] for r in valid if r.get("guest_frame_cycles")]
    out["guest_frame_cycles"] = summarize(costs)
    out["failure_reasons"] = sorted({r.get("detail", "unspecified")[:100] for r in failed})
    return out


def run_trial(trial_index, root, results_dir, timeout_s, boot_timeout_s):
    """Boot a fresh guest, send one TAP, record one raw trial."""
    artifact = os.path.join(root, "target", "inter-os-bios.img")
    record = {
        "app": APP_LAUNCH_ID,
        "trial": trial_index,
        "arch": "x86_64",
        "metric": METRIC_NAME,
        "endpoint": ENDPOINT,
        "tap": [KEYPAD_X, KEYPAD_Y],
        "outcome": None,
        "retries": 0,
        "detail": "",
        "input_accepted_ms": None,
        "state_changed_ms": None,
        "tap_to_post_surface_flip_ms": None,
        "guest_flip": None,
        "guest_compose_cycles": None,
        "guest_present_cycles": None,
        "guest_frame_cycles": None,
        "post_flip_markers_before": None,
        "post_flip_markers_after": None,
    }

    if not os.path.exists(artifact):
        record["outcome"] = "missing_artifact"
        record["detail"] = "boot artifact not found: %s" % artifact
        return record

    qemu_exe = _find_qemu("qemu-system-x86_64.exe")
    port = 5810 + trial_index
    _kill_port_owner(port)
    ser_log = os.path.join(results_dir, "tap-trial%02d.log" % trial_index)
    if os.path.exists(ser_log):
        os.remove(ser_log)
    record["serial_log"] = ser_log

    args = [qemu_exe] + QEMU_ARGS_BASE + [
        "-drive", "format=raw,file=%s" % artifact,
        "-chardev", "socket,id=ser0,host=127.0.0.1,port=%d,server=on,wait=off" % port,
        "-serial", "chardev:ser0",
    ]
    record["qemu_args"] = args
    record["qemu_exe"] = qemu_exe

    proc = None
    session = None
    try:
        try:
            proc = subprocess.Popen(args, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
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
                sock.connect(("127.0.0.1", port))
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

        if session.await_marker(BOOT_READY_MARKER, boot_timeout_s) is None:
            record["outcome"] = "boot_timeout"
            record["detail"] = "guest never reached %r" % BOOT_READY_MARKER
            return record

        # Known calculator state: launched fresh, so the expression is empty and
        # nothing has been typed yet.
        session.send_once("APP_LAUNCH %s" % APP_LAUNCH_ID)
        if session.await_marker(APP_READY_MARKER, boot_timeout_s) is None:
            record["outcome"] = "app_not_ready"
            record["detail"] = "calculator never reported its first frame"
            return record

        # Everything the guest printed before this point belongs to boot and to
        # the app's own start-up. Counting those occurrences is what stops one of
        # them from satisfying the post-tap wait.
        session.drain()
        before = session.marker_count(POST_FLIP_MARKER)
        record["post_flip_markers_before"] = before

        sent_at = session.send_once("TAP %d %d" % (KEYPAD_X, KEYPAD_Y))

        accepted_at = session.await_marker(INPUT_ACCEPTED_MARKER % (KEYPAD_X, KEYPAD_Y),
                                           timeout_s)
        state_at = session.await_marker(STATE_CHANGED_MARKER, timeout_s)
        record["input_accepted_ms"] = _delta(sent_at, accepted_at)
        record["state_changed_ms"] = _delta(sent_at, state_at)

        if accepted_at is None:
            record["outcome"] = "input_rejected"
            record["detail"] = "the kernel never acknowledged the tap"
            return record
        if state_at is None:
            record["outcome"] = "no_state_change"
            record["detail"] = ("the tap was accepted but no keypad key responded, "
                                "so no input-triggered redraw was expected")
            return record

        # Wait for exactly one more post-flip marker than existed before the send.
        post_at = session.await_nth_marker(POST_FLIP_MARKER, before + 1, timeout_s)
        record["post_flip_markers_after"] = session.marker_count(POST_FLIP_MARKER)
        record["tap_to_post_surface_flip_ms"] = _delta(sent_at, post_at)

        if post_at is None:
            record["outcome"] = "post_flip_timeout"
            record["detail"] = ("the keypad accepted the tap and the state changed, "
                                "but no post-input frame marker followed")
            return record

        record["outcome"] = "ok"
        # Asked only after the endpoint, so the DIAG_STATE round trip cannot
        # lengthen the measured interval.
        frame = read_guest_frame_cost(session)
        if frame:
            record["guest_flip"] = frame["flip"]
            record["guest_compose_cycles"] = frame["compose_cycles"]
            record["guest_present_cycles"] = frame["present_cycles"]
            record["guest_frame_cycles"] = frame["compose_cycles"] + frame["present_cycles"]
        return record
    finally:
        if session is not None:
            origin = session.lines[0][0] if session.lines else 0.0
            with open(ser_log, "w", encoding="utf-8") as fh:
                fh.write("# app=%s trial=%d arch=x86_64 tap=%d,%d\n"
                         % (APP_LAUNCH_ID, trial_index, KEYPAD_X, KEYPAD_Y))
                fh.write("# t+ms is host monotonic time relative to the first line read\n")
                for at, text in session.lines:
                    fh.write("%+9.1f  %s\n" % ((at - origin) * 1000.0, text))
        if proc is not None and proc.poll() is None:
            try:
                proc.kill()
                proc.wait(timeout=10)
            except Exception:
                pass


def _delta(start, end):
    if start is None or end is None:
        return None
    return round((end - start) * 1000.0, 1)


def read_guest_frame_cost(session, timeout_s=5.0):
    """Ask the guest what its last flip cost, in CPU cycles.

    WHY this exists: the wall-clock interval mixes two very different things --
    work the guest did rendering the frame, and the host not scheduling QEMU.
    Without the guest's own numbers an outlier cannot be told apart, and the
    only printed source, `[KMS PERF]`, samples the first 15 flips and every 30th
    after that, so the flip carrying the input is almost never in it. The kernel
    publishes every flip's cost and answers on the existing `DIAG_STATE`
    channel, so this is one command and one reply.

    Returns None when the guest does not answer, which is reported as unknown
    rather than guessed at. The flip number comes back too, because two reads
    showing the same flip would mean the second sampled the same frame again
    rather than a cheaper one.
    """
    session.send_once("DIAG_STATE")
    at = session.await_marker(DIAG_STATE_MARKER, timeout_s=timeout_s)
    if at is None:
        return None
    for _ts, text in reversed(session.lines):
        reading = parse_diag_state(text)
        if not reading:
            continue
        # -1 is the parser's "field absent" marker. Accepting it would report a
        # free frame for a guest that never measured one.
        if reading["last_flip_compose_cycles"] < 0 or reading["last_flip"] < 0:
            return None
        return {
            "flip": reading["last_flip"],
            "compose_cycles": reading["last_flip_compose_cycles"],
            "present_cycles": reading["last_flip_present_cycles"],
        }
    return None


def attribute_outlier(record, fast_cycles):
    """Decide whether a trial's frame was expensive or the host merely stalled.

    WHY a threshold: the guest reports one flip's cost in its own cycles. A
    trial whose flip cost is near the typical value cannot have spent its extra
    time rendering, so the time went somewhere the guest did not account for --
    host scheduling. A trial whose flip cost is proportionally higher did do
    proportionally more work. Stating the comparison rather than a bare verdict
    keeps the conclusion auditable.
    """
    cost = record.get("guest_frame_cycles")
    if cost is None or not fast_cycles:
        return "unknown (guest did not report a flip cost)"
    if cost > fast_cycles:
        return "guest work: flip cost %d cycles exceeds the typical %d" % (cost, fast_cycles)
    return "host scheduling: flip cost %d cycles is not above the typical %d" % (cost, fast_cycles)


def preflight(artifact, root):
    """Refuse to measure without a usable image. Returns None, or an error string.

    WHY this is separate from main: it must be testable without an emulator.
    An earlier version of this check lived inline in main, and the test that
    covered it actually booted QEMU against the developer's real image, which is
    both slow and a measurement taken during a unit test.

    `image_predates_sources` is the load-bearing half. `dev.ps1 build` only
    rebuilds userspace when an embedded image is missing, not ET_EXEC, or still
    carries a symbol table -- it does not compare sources against artifacts. So
    an edited calculator source can leave the image holding the previous build,
    and every number measured against it would describe code that is no longer
    in the tree.
    """
    if not os.path.exists(artifact):
        return "boot artifact not found: %s (run `dev.ps1 build` first)" % artifact
    if image_predates_sources(artifact, root):
        return ("boot artifact %s predates the sources it should embed.\n"
                "  image  : %s\n"
                "  sources: %s\n"
                "  dev.ps1 build does not rebuild userspace on a source change, so "
                "force it (delete target\\x86_64-unknown-none\\debug\\calculator) "
                "and rebuild before measuring."
                % (artifact,
                   time.strftime("%Y-%m-%d %H:%M:%S",
                                 time.localtime(os.path.getmtime(artifact))),
                   time.strftime("%Y-%m-%d %H:%M:%S",
                                 time.localtime(newest_source_mtime(root)))))
    return None


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--trials", type=int, default=10)
    parser.add_argument("--timeout", type=float, default=30.0)
    parser.add_argument("--boot-timeout", type=float, default=180.0)
    parser.add_argument("--out", default=os.path.join(REPO_ROOT, "target", "bench", "tap"))
    args = parser.parse_args()

    os.makedirs(args.out, exist_ok=True)
    artifact = os.path.join(REPO_ROOT, "target", "inter-os-bios.img")
    problem = preflight(artifact, REPO_ROOT)
    if problem:
        print("ERROR: %s" % problem, file=sys.stderr)
        return 2

    records = []
    for trial in range(1, args.trials + 1):
        print("[tap] trial %d/%d" % (trial, args.trials), flush=True)
        rec = run_trial(trial, REPO_ROOT, args.out, args.timeout, args.boot_timeout)
        records.append(rec)
        print("[tap]   outcome=%-18s retries=%d accepted=%sms state=%sms %s=%sms %s"
              % (rec["outcome"], rec["retries"], rec["input_accepted_ms"],
                 rec["state_changed_ms"], METRIC_NAME,
                 rec["tap_to_post_surface_flip_ms"], rec["detail"]), flush=True)

    summary = summarize_trials(records)
    payload = {
        "tool": "bench_tap_latency",
        "app": APP_LAUNCH_ID,
        "arch": "x86_64",
        "metric": METRIC_NAME,
        "start_event": "host wrote 'TAP %d %d' to the guest serial socket" % (KEYPAD_X, KEYPAD_Y),
        "end_event": "calculator logged %r after surface_flip returned" % POST_FLIP_MARKER,
        "endpoint": ENDPOINT,
        "endpoint_caveat": ENDPOINT_NOTE,
        "clock": "time.perf_counter (monotonic)",
        "tap_retried": False,
        "artifact": describe_artifact(REPO_ROOT, os.path.join("target", "inter-os-bios.img")),
        "raw": records,
        "summary": summary,
    }
    out_path = os.path.join(args.out, "tap-latency.json")
    with open(out_path, "w", encoding="utf-8") as fh:
        json.dump(payload, fh, indent=2)

    print("\n=== calculator %s (x86_64) ===" % METRIC_NAME)
    print("start    : %s" % payload["start_event"])
    print("end      : %s" % payload["end_event"])
    print("caveat   : %s" % ENDPOINT_NOTE)
    print("artifact : %s" % out_path)
    print("attempts : %d   valid : %d   failed/invalid : %d"
          % (summary["attempts"], summary["valid_samples"], summary["failed_or_invalid"]))
    for phase, label in (("input_accepted", "kernel accepted the tap"),
                         ("state_changed", "keypad changed state"),
                         (METRIC_NAME, "TAP -> post-input surface_flip")):
        st = summary[phase]
        if st["count"]:
            print("%-28s n=%-3d median=%-8s mean=%-8s min=%-8s max=%s ms"
                  % (label, st["count"], st["median_ms"], st["mean_ms"],
                     st["min_ms"], st["max_ms"]))
        else:
            print("%-28s no valid samples" % label)
    if summary["failure_reasons"]:
        print("failures : %s" % "; ".join(summary["failure_reasons"]))

    cost_stats = summary["guest_frame_cycles"]
    print("\n=== outlier attribution (guest work vs host scheduling) ===")
    if not cost_stats["count"]:
        print("The guest did not report a flip cost, so no trial can be attributed.")
    else:
        typical = cost_stats["median_ms"] if cost_stats["median_ms"] else cost_stats["min_ms"]
        print("guest flip cost: median=%s cycles, range %s..%s (n=%d)"
              % (cost_stats["median_ms"], cost_stats["min_ms"], cost_stats["max_ms"],
                 cost_stats["count"]))
        print("%-6s %-10s %-14s %s" % ("trial", "interval", "flip cycles", "attribution"))
        for rec in records:
            if not is_valid_sample(rec):
                continue
            cost = rec.get("guest_frame_cycles")
            verdict = attribute_outlier(rec, typical)
            print("%-6d %-10s %-14s %s"
                  % (rec["trial"], rec[METRIC_NAME], cost if cost else "unknown", verdict))

    if summary["valid_samples"] == 0:
        return 1
    if summary["failed_or_invalid"]:
        return 1
    if summary["attempts"] < args.trials:
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())