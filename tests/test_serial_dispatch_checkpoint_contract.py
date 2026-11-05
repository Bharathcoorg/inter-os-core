# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

"""Gates where the kernel is allowed to notice a serial input command.

A tap reaches the guest as a line on COM1. The line is read by the interrupt
handler into a queue, and nothing dispatches it until a task calls
`service_serial_commands` -- so the wait a person feels is the distance from the
queue to the next checkpoint. Before the dispatcher entry became one of those
checkpoints, every checkpoint sat in a syscall a busy application does not make
while it is painting: an app writing its frame calls neither `event_poll` nor
`sys_yield`, and a tap landing mid-paint waited out the whole paint. Measured on
this tree, 190 ms for a second tap that landed in a browser repaint, 3.0 s behind
the browser's first page.

Two properties hold this up and both are cheap to lose by accident:

  1. The dispatcher pumps the queue before it looks at the syscall number, so the
     checkpoint is reached by every task that is not blocked, including the one
     that is drawing.
  2. The pump never runs in interrupt context. `kernel/src/serial.rs` says so in
     its own comment, because a command handler prints, and printing from the
     timer path would block the handler that fills the queue.

The kernel's host suite cannot exercise either property -- dispatching a real
line needs a running task and a UART -- so these are structural gates over the
kernel source. Nothing here runs QEMU.
"""

from pathlib import Path
import re

ROOT = Path(__file__).resolve().parents[1]
KERNEL = ROOT / "kernel" / "src"
SYSCALLS = KERNEL / "syscall" / "mod.rs"
SERIAL = KERNEL / "serial.rs"

PUMP = "crate::serial::service_serial_commands()"


def sys_dispatch_body() -> str:
    """The dispatcher, from its signature to the next top-level item."""
    source = SYSCALLS.read_text(encoding="utf-8", errors="replace")
    signature = "pub extern \"C\" fn sys_dispatch(num: u64"
    start = source.index(signature)
    lines = source[start:].split("\n")
    for offset, line in enumerate(lines[1:], start=1):
        if not line.strip():
            continue
        if line.startswith("}") or line.startswith("#[no_mangle]") or line.startswith("pub fn "):
            if offset > 1:
                return "\n".join(lines[:offset])
    return "\n".join(lines)


# ---------------------------------------------------------------------------
# 1. The dispatcher itself is a checkpoint, ahead of the syscall arm.
# ---------------------------------------------------------------------------


def test_the_dispatcher_pumps_the_serial_queue_before_the_syscall_runs():
    body = sys_dispatch_body()
    pump = body.index(PUMP)
    arm = body.index("match num {")
    assert pump < arm, (
        "the serial pump must run before the syscall is chosen: a task that is"
        " painting calls no poll syscall, so this entry is the only checkpoint it"
        " reaches while a tap is waiting"
    )


def test_the_pump_is_the_first_thing_the_dispatcher_does():
    """Nothing may be taken or allocated before it, or the pump inherits the lock.

    WHY this is a separate gate: the check above only proves order. The pump is
    safe at this site because the dispatcher holds nothing yet -- `service_serial_commands`
    takes the receive buffer's lock and prints, and either would be a deadlock if
    an arm ahead of it already held a kernel lock.
    """
    body = sys_dispatch_body()
    between = body[: body.index(PUMP)]
    statements = [
        line.strip()
        for line in between.split("\n")
        if line.strip()
        and not line.strip().startswith(("//", "/*", "*", "*/", "#", "pub extern"))
    ]
    assert not statements, (
        f"the dispatcher now does something before pumping the serial queue:"
        f" {statements[:3]}"
    )


# ---------------------------------------------------------------------------
# 2. No checkpoint is in the interrupt path.
# ---------------------------------------------------------------------------


def test_no_interrupt_handler_dispatches_serial_commands():
    interrupts = (KERNEL / "interrupts.rs").read_text(encoding="utf-8", errors="replace")
    assert "service_serial_commands" not in interrupts, (
        "a command handler prints, and printing from the timer interrupt blocks the"
        " handler that fills the queue -- see the note above `poll_serial_rx`"
    )


def test_the_serial_module_still_forbids_dispatch_in_irq_context():
    """The rule this gate defends lives in a comment; pin the comment too.

    WHY: the prohibition is the reason the checkpoints are syscalls. If the note
    is deleted the next reader has nothing to disagree with, and the failure mode
    is a guest that stops answering the keyboard under load.
    """
    serial = SERIAL.read_text(encoding="utf-8", errors="replace")
    assert re.search(r"[Ii][RQ]|interrupt", serial[: serial.index("pub fn poll_serial_rx")]), (
        "the serial module no longer states where dispatch must not happen"
    )


def test_the_cooperative_checkpoints_are_all_still_reachable():
    """A blocked task never reaches the dispatcher, so the poll sites stay.

    Counted rather than listed: the point is that the set does not silently
    shrink to one entry point that only an unblocked task can use.
    """
    source = SYSCALLS.read_text(encoding="utf-8", errors="replace")
    sites = source.count(PUMP)
    assert sites >= 7, (
        f"only {sites} dispatcher-side checkpoints remain; the tree carries the"
        " dispatcher entry plus the six syscall arms a blocked task can wake into,"
        " and a task parked anywhere else sees none of them"
    )
    main = (KERNEL / "main.rs").read_text(encoding="utf-8", errors="replace")
    assert "kernel::serial::service_serial_commands()" in main, (
        "the idle loop must still drain the queue, or a tap arriving while every"
        " task is blocked waits for the next wakeup at all"
    )
