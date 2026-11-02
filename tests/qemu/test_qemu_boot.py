"""Deterministic QEMU boot smoke tests for both kernel architectures.

These require QEMU and are therefore not part of `run_all_contracts.py`, which
only executes source-level contracts. Run them directly:

    python tests/qemu/test_qemu_boot.py

Each machine that has an artefact to boot is held for a fixed window with the
console captured to a file. A boot passes only when the kernel reaches its
ready banner without a kernel panic, so a hang or a crash is reported as a
failure rather than silently passing.

Four outcomes are reported, because four different facts are at stake:

  PASS       the artefact existed, the machine was booted, and it behaved.
  FAIL       the artefact existed and the boot misbehaved -- a panic, a missing
             ready banner, or a device tree that was never discovered. This is
             the only outcome that exits non-zero.
  NOT BUILT  the artefact was never produced, so the leg was never run. Naming
             an absent file a boot failure would blame the platform for a gap
             in the build; `docs/PLATFORM_REALITY_CONTRACT.md` keeps "never
             produced" and "crashed on boot" apart, and so does this suite.
  SKIP       QEMU itself is not on PATH, so nothing could be attempted.

A NOT BUILT or SKIP leg is therefore an *unverified* leg, never a passing one:
the run ends with a summary naming exactly which machines were booted and which
were not, so a green exit can never be read as "both architectures boot".

On the current tree both legs boot. Their artefacts are build output and are not
kept on a small disk, so a checkout that has not run the two builds reports both
legs NOT BUILT -- and this suite then exits non-zero, because a run that booted
nothing while QEMU is on PATH verified nothing. Build first:

    python tools/interos.py build --target aarch64
    python tools/interos.py build --target riscv64
"""

import os
import subprocess
import sys
import time
import tempfile

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), "..", ".."))

AARCH64_TARGET = "aarch64-unknown-none"
RISCV_TARGET = "riscv64gc-unknown-none-elf"

AARCH64_KERNEL = os.path.join(
    ROOT, "kernel-aarch64", "target", AARCH64_TARGET, "debug", "kernel-aarch64"
)
RISCV_KERNEL = os.path.join(
    ROOT, "kernel-riscv64", "target", RISCV_TARGET, "debug", "kernel-riscv64"
)

# The flat arm64 Image, the only artefact that can reach the device tree: for a
# flat file QEMU's direct boot runs its own stub whose first instruction loads
# the device tree address into x0, which is where this kernel reads it from. An
# ELF is entered at its own address with x0 = 0, so the ELF path can never
# discover a device tree no matter how healthy the kernel is.
AARCH64_FLAT_IMAGE = os.path.join(ROOT, "target", "kernel-aarch64.img")

# Which build would have to produce each artefact, so a NOT BUILT line names a
# responsible script instead of just an absent file.
AARCH64_KERNEL_PRODUCER = (
    "tools/interos.py build --target aarch64, which runs cargo build "
    "--manifest-path kernel-aarch64/Cargo.toml --target aarch64-unknown-none, "
    "would have to produce it"
)
AARCH64_IMAGE_PRODUCER = (
    "tools/interos.py build --target aarch64 packs it from the kernel ELF, so "
    "this leg means that build has not been run on this checkout"
)
RISCV_KERNEL_PRODUCER = (
    "tools/interos.py build --target riscv64, which runs cargo build "
    "--manifest-path kernel-riscv64/Cargo.toml "
    "--target riscv64gc-unknown-none-elf, would have to produce it"
)

# QEMU is available on PATH in CI images only if installed, so probe per binary.
AARCH64_QEMU = "qemu-system-aarch64"
RISCV_QEMU = "qemu-system-riscv64"

BOOT_WINDOW_SECONDS = 20

# The four outcomes `main()` prints and tallies.
PASS = "PASS"
FAIL = "FAIL"
NOT_BUILT = "NOT BUILT"
SKIP = "SKIP"


def _not_built(missing, producer):
    """Legs whose artefact was never produced are unrun, not failed."""
    return NOT_BUILT, f"{missing} ({producer})"


def _have(binary):
    try:
        subprocess.run(
            [binary, "--version"],
            capture_output=True,
            timeout=30,
            check=True,
        )
        return True
    except (OSError, subprocess.SubprocessError):
        return False


def _boot(qemu, args, log_path, seconds=BOOT_WINDOW_SECONDS):
    """Boots the machine, capturing the console, then terminates it."""
    cmd = [
        qemu,
        "-machine", "virt",
        "-m", "512",
        "-display", "none",
        "-monitor", "none",
        "-serial", "file:" + log_path,
        "-no-reboot",
    ] + args
    proc = subprocess.Popen(
        cmd, cwd=ROOT, stdout=subprocess.DEVNULL, stderr=subprocess.PIPE
    )
    try:
        deadline = time.time() + seconds
        while time.time() < deadline:
            if proc.poll() is not None:
                break
            time.sleep(0.5)
    finally:
        if proc.poll() is None:
            proc.terminate()
            try:
                proc.wait(timeout=10)
            except subprocess.TimeoutExpired:
                proc.kill()
                proc.wait(timeout=10)
    with open(log_path, "r", encoding="utf-8", errors="replace") as handle:
        return handle.read()


def check_aarch64():
    """AArch64 must reach its ready banner via the conformant arm64 Image path."""
    if not os.path.exists(AARCH64_KERNEL):
        return _not_built(
            "kernel ELF not built: " + AARCH64_KERNEL, AARCH64_KERNEL_PRODUCER
        )
    if not os.path.exists(AARCH64_FLAT_IMAGE):
        return _not_built(
            "flat image not built: " + AARCH64_FLAT_IMAGE, AARCH64_IMAGE_PRODUCER
        )

    with tempfile.TemporaryDirectory() as tmp:
        log = os.path.join(tmp, "aarch64.log")
        out = _boot(
            AARCH64_QEMU,
            ["-cpu", "cortex-a57", "-kernel", os.path.relpath(AARCH64_FLAT_IMAGE, ROOT)],
            log,
        )

    if "KERNEL PANIC" in out or "PANIC" in out:
        return FAIL, "kernel panicked"
    if "[READY]" not in out:
        return FAIL, "never reached the ready banner"
    # The device tree must be discovered, not assumed.
    if "Valid DTB discovered" not in out:
        return FAIL, "no device tree discovered"
    if "Physical RAM discovered" not in out:
        return FAIL, "physical RAM not discovered from the device tree"
    # A platform that exposes GICv2m must not be programmed as GICv2.
    if "arm,gic-v2m" not in out:
        return FAIL, "interrupt controller was not identified from the device tree"
    if "written through as GICv2" not in out:
        return FAIL, "interrupt routing was not declined for a non-GICv2 model"
    return PASS, "reached ready banner with device-tree discovery"


def check_riscv64():
    """RISC-V must drop to U-mode and return through the syscall path."""
    if not os.path.exists(RISCV_KERNEL):
        return _not_built(
            "kernel ELF not built: " + RISCV_KERNEL, RISCV_KERNEL_PRODUCER
        )

    with tempfile.TemporaryDirectory() as tmp:
        log = os.path.join(tmp, "riscv64.log")
        out = _boot(
            RISCV_QEMU,
            [
                "-cpu", "rv64",
                "-smp", "1",
                "-bios", "default",
                "-kernel", os.path.relpath(RISCV_KERNEL, ROOT),
            ],
            log,
        )

    if "KERNEL PANIC" in out:
        return FAIL, "kernel panicked"
    if "[USER:LOG]" not in out:
        return FAIL, "userspace never executed"
    if "[SYSCALL:EXIT]" not in out:
        return FAIL, "userspace exit was never observed"
    if "Userspace exit recorded by scheduler: true" not in out:
        return FAIL, "scheduler did not record the task exit"
    if "sstatus.SUM cleared" not in out:
        return FAIL, "supervisor user-memory access was not disabled"
    if "rejected (-EFAULT)" not in out:
        return FAIL, "kernel pointer was not rejected"
    return PASS, "U-mode syscall round trip with supervisor isolation"


def main():
    outcomes = []
    for name, binary, fn in (
        ("aarch64", AARCH64_QEMU, check_aarch64),
        ("riscv64", RISCV_QEMU, check_riscv64),
    ):
        if not _have(binary):
            print(f"{SKIP}: {name} ({binary} not available)")
            outcomes.append((name, SKIP))
            continue
        outcome, detail = fn()
        print(f"{outcome}: {name} -> {detail}")
        outcomes.append((name, outcome))

    # WHY the summary exists: `main` now exits 0 when a leg was never built, so
    # the tally alone cannot tell a booted platform apart from an unbuilt one.
    # Saying which machines were actually booted keeps a green run from being
    # read as a verification it never earned.
    booted = [name for name, outcome in outcomes if outcome in (PASS, FAIL)]
    unverified = [
        f"{name} ({outcome})"
        for name, outcome in outcomes
        if outcome not in (PASS, FAIL)
    ]
    print()
    print(f"Booted and judged: {', '.join(booted) or 'nothing'}.")
    if unverified:
        print(
            f"Never booted, so NOT verified: {', '.join(unverified)}."
            " A zero exit code means no boot misbehaved; it is not evidence"
            " that either architecture boots."
        )

    # WHY a run that booted nothing cannot be green: NOT BUILT is honest about a
    # single absent artefact, but if it silences every leg the suite has verified
    # nothing at all, and a suite that verifies nothing has no business exiting
    # zero. Only a machine with no QEMU on PATH is allowed to pass while booting
    # nothing, because that is an environment limitation and not a tree defect.
    judged_nothing = not booted
    qemu_absent = all(outcome == SKIP for _, outcome in outcomes)

    if any(outcome == FAIL for _, outcome in outcomes):
        sys.exit(1)
    if judged_nothing and not qemu_absent:
        print(
            "Nothing was booted and QEMU is on PATH, so this run verified no"
            " machine: that is a failure, not a pass."
        )
        sys.exit(1)


if __name__ == "__main__":
    main()
