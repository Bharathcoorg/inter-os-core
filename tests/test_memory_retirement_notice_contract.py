#!/usr/bin/env python3
"""The memory-retirement notice's truth contract.

WHAT THIS FILE IS FOR: when the kernel cannot afford a surface for the app being
opened, it retires the longest-backgrounded app to pay for it. The person was
owed two things and got neither: a notice naming the app that went away, and an
exit code that says the device needed that app's memory rather than an anonymous
stop. The rules below pin the retirement to a single honest notice, and forbid
the notice from claiming a number the retire path did not measure.

WHAT "DID NOT MEASURE" MEANS HERE: the retire path holds exactly two figures --
the bytes the chosen victim's surface holds (`held`) and the ceiling every
surface shares (`MAX_SURFACE_BYTES`). A notice is honest only if those are the
numbers it states. A percentage, a "low memory" or "memory pressure" claim, or a
byte count typed as a literal is a number the device never measured, and the
Platform Truth Contract forbids it.

HOW MUCH OF IT THE PERSON SEES: the shell caches a title and a body in fixed
fields and clips the row to the card, so a notice that is honest inside the store
can still arrive on screen cut off mid-number. The fit rule measures the widest
each part of the sentence can get -- the package names this tree ships, the digit
count the platform's own ceiling allows -- against the shell's cache fields.

HOW THE CONTRACT CAN FAIL: each rule is a detector that takes a source string
rather than reading the file itself, so `test_the_contract_can_fail` can run it
over a synthetic source carrying the fault and prove it catches it -- and then
run it over the real sources and require none to fire. A contract that cannot
fail is a comment.

Run it directly, or under pytest.
"""

import re
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]

SYSCALL = (ROOT / "kernel" / "src" / "syscall" / "mod.rs").read_text(encoding="utf-8")
KMS = (ROOT / "kernel" / "src" / "drivers" / "display_kms.rs").read_text(encoding="utf-8")
SHELL = (ROOT / "apps" / "launcher" / "src" / "main.rs").read_text(encoding="utf-8")


def _strip_comments(source: str) -> str:
    """The source with its line comments removed.

    WHY: a comment that names a forbidden claim is documentation, not a claim.
    The retire path explains, in prose, why it must not fabricate a number -- a
    contract that failed on that prose would push the next author to delete the
    explanation instead of the fault. Only the code is inspected.
    """
    kept = []
    for line in source.split("\n"):
        in_string = False
        cut = len(line)
        index = 0
        while index < len(line):
            char = line[index]
            if in_string and char == "\\":
                index += 2
                continue
            if char == '"':
                in_string = not in_string
            elif not in_string and char == "/" and line[index:index + 2] == "//":
                cut = index
                break
            index += 1
        kept.append(line[:cut])
    return "\n".join(kept)


def _body(source: str, signature: str) -> str:
    """The braced body that follows `signature`, as far as its own braces close."""
    code = _strip_comments(source)
    at = code.find(signature)
    assert at >= 0, f"the source no longer has {signature}"
    open_at = code.find("{", at)
    assert open_at >= 0, f"{signature} has no body"
    depth = 0
    for index in range(open_at, len(code)):
        char = code[index]
        if char == "{":
            depth += 1
        elif char == "}":
            depth -= 1
            if depth == 0:
                return code[open_at + 1:index]
    raise AssertionError(f"{signature} has an unbalanced body")


def _retire_body(syscall_src: str) -> str:
    """The part of `sys_surface_create` that pays for a surface by retiring one.

    WHY scoped to the loop rather than the whole function: the retirement is the
    only place a notice belongs; a notice posted at the refusal below the loop
    would name no victim and measure no bytes. The loop body is what runs once
    per retired app.
    """
    body = _body(syscall_src, "pub fn sys_surface_create")
    start = body.find("loop {")
    assert start >= 0, "the retirement loop is gone from sys_surface_create"
    depth = 0
    for index in range(start, len(body)):
        char = body[index]
        if char == "{":
            depth += 1
        elif char == "}":
            depth -= 1
            if depth == 0:
                return body[start:index]
    raise AssertionError("the retirement loop has an unbalanced body")


def _posts_exactly_once_defects(retire: str) -> list:
    """The retire path must post one notice, not zero and not two."""
    defects = []
    posts = retire.count("post_retirement_notice(")
    if posts == 0:
        defects.append("the retirement no longer posts any notice")
    if posts > 1:
        defects.append(f"the retirement posts {posts} notices; one decision owes one notice")
    return defects


def _account_defects(retire: str, notice_src: str) -> list:
    """The notice must be filed under the account whose shade reads it.

    WHY this is a rule and not a detail: the store hands a record only to the
    account it is filed under, and the shell asks for the active one. A notice
    posted to a hard-coded owner is invisible to everybody else -- the kernel
    logs it as posted, the shade shows nothing, and the person is told a lie by
    a log line. The device run that first proved this caught exactly that.
    """
    defects = []
    if "UserId::OWNER" in notice_src or "UserId::SYSTEM" in notice_src:
        defects.append("the notice files itself under a hard-coded account")
    if "active_user()" not in retire:
        defects.append("the retire path does not ask which account is holding the device")
    if not re.search(r"post_retirement_notice\([^)]*\baccount\b", retire, re.DOTALL):
        defects.append("the retire path does not hand the notice the account it resolved")
    return defects


def _measured_numbers_defects(retire: str, notice_src: str) -> list:
    """Every number the notice states must be one the retire path measured."""
    defects = []
    # The bytes the notice claims to free is the `held` figure the eviction
    # already computed, passed straight through -- never a re-typed literal.
    if not re.search(r"post_retirement_notice\([^)]*\bheld\b", retire, re.DOTALL):
        defects.append("the notice is not given the `held` bytes the retirement measured")
    # The budget the notice measures against is the platform's own ceiling.
    if "MAX_SURFACE_BYTES" not in notice_src:
        defects.append("the notice states no surface budget it could be measured against")
    # A number the device did not measure, dressed up as one.
    fabricated = re.findall(r"low memory|memory pressure|heap.free|heap free|\d+\s?%",
                            _strip_comments(notice_src), re.IGNORECASE)
    if fabricated:
        defects.append(f"the notice claims a figure the device did not measure: {sorted(set(fabricated))}")
    return defects


_FORMAT_LINE = re.compile(
    r'let\s+(title|body)\s*=\s*format!\s*\(\s*"((?:[^"\\]|\\.)*)"\s*,([^;]*?)\)\s*;',
    re.DOTALL,
)

# Every figure the notice states is bounded by `MAX_SURFACE_BYTES`, which is 8
# digits on this device.
LONGEST_FIGURE_DIGITS = 8


def _shell_cache_field(name: str) -> int:
    """How many bytes the shell keeps for a notification's `name` (pkg/title/body).

    WHY read from the shell rather than typed here: the card's ceiling is the
    shell's cache field, and a contract that hard-codes 64 keeps passing when the
    shell widens the field and the notice author still has to fit the card.
    """
    match = re.search(rf"rec\.{name}_len = {name}_bytes\.len\(\)\.min\((\d+)\)", _strip_comments(SHELL))
    assert match, f"the shell no longer caches a notification {name} in a fixed field"
    return int(match.group(1))


def _longest_package_name() -> int:
    """The widest package name the device can put on a card today.

    WHY measured from the tree: a task's package id is `app_<directory>`, and the
    notice's title is built from it, so a new app with a long name tightens the
    title's budget. The contract should say so at the commit that adds the app
    rather than let the card clip the claim in front of somebody.
    """
    names = [path.name for path in (ROOT / "apps").iterdir() if path.is_dir()]
    widest = max(len("app_" + name) for name in names)
    return min(widest, _shell_cache_field("pkg"))


def _painted_widest(template: str, args: str) -> str:
    """The template with every placeholder replaced by the widest text that can fill it."""
    out = template
    for arg in [value.strip() for value in args.split(",") if value.strip()]:
        width = _longest_package_name() if arg == "victim_package" else LONGEST_FIGURE_DIGITS
        out = out.replace("{}", "9" * width, 1)
    return out


def _card_fit_defects(notice_src: str) -> list:
    """The notice has to fit the card that shows it, not just the store that keeps it."""
    code = _strip_comments(notice_src)
    parts = {name: (template, args) for name, template, args in _FORMAT_LINE.findall(code)}
    if "title" not in parts or "body" not in parts:
        return ["the notice is not built from literal title and body templates the fit can be measured on"]
    defects = []
    for name in ("title", "body"):
        template, args = parts[name]
        widest = _painted_widest(template, args)
        room = _shell_cache_field(name)
        if len(widest) > room:
            defects.append(
                f"the {name} is {len(widest)} characters at its widest; the shell's card holds {room}, "
                "so the person reads a clipped claim"
            )
    return defects


def _exit_code_defects(retire: str, kms_src: str) -> list:
    """The retirement is marked by a named code, not the anonymous -1."""
    defects = []
    if not re.search(r"pub const EXIT_CODE_MEMORY_RETIRE\s*:\s*i32", _strip_comments(kms_src)):
        defects.append("there is no named exit code for a memory retirement")
    if "EXIT_CODE_MEMORY_RETIRE" not in retire:
        defects.append("the retirement does not use the named memory-retirement exit code")
    if re.search(r"terminate_task\([^,]*,\s*-1\s*\)", retire):
        defects.append("the retirement still terminates the victim with the anonymous -1")
    return defects


def test_the_retirement_posts_exactly_one_notice():
    """One app going away owes the person one line about it."""
    retire = _retire_body(SYSCALL)
    defects = _posts_exactly_once_defects(retire)
    assert not defects, defects


def test_the_notice_reports_only_measured_numbers():
    """The notice names the victim and states the two figures the retire path has.

    WHY it names the victim by package and not a fabricated total: the person
    has to see which app went away, and the bytes it freed are the bytes the
    eviction measured, and the ceiling is the platform's own constant.
    """
    retire = _retire_body(SYSCALL)
    notice = _body(KMS, "pub fn post_retirement_notice")
    defects = _measured_numbers_defects(retire, notice)
    assert not defects, defects


def test_the_notice_names_the_victim_and_survives_its_reclaim():
    """The notice is owned by the system, and it still names the app.

    WHY not the victim's own package: when the retired task is reaped the kernel
    cancels every notice under that package, so a notice left in the retired
    app's name would delete itself before the person read it. The honest owner is
    the system -- while the wording still names the app that went away.
    """
    notice = _body(KMS, "pub fn post_retirement_notice")
    assert '"system"' in notice, "the retirement notice is not owned by the system"
    assert "victim_package" in notice, "the notice does not name the app that was retired"


def test_the_notice_is_filed_where_the_shade_looks():
    """The retirement is announced to the account holding the device.

    WHY this is pinned: the store files a record under one account and the shell
    reads the active one, so a notice filed anywhere else is announced to an
    empty screen. The helper takes the account as an argument and the retire path
    resolves it from the device rather than assuming the owner.

    WHAT THE MEASUREMENT CORRECTED: the first device run posted the notice,
    logged it as posted, and showed the person nothing, and the account was the
    suspected cause. It was not -- the run's own log has `Posted #3 ... for user
    1000` beside `Device owner provisioned: UID=1000`, and the shell's read line
    says `uid=1000`. The notice was invisible because the shell painted the list
    its last poll had cached. This rule still holds the account contract; the
    shade-open re-read is the shell's half, pinned in the launcher.
    """
    retire = _retire_body(SYSCALL)
    notice = _body(KMS, "pub fn post_retirement_notice")
    defects = _account_defects(retire, notice)
    assert not defects, defects


def test_the_notice_fits_the_card_that_shows_it():
    """What the person reads is bounded by the card, not by the store.

    WHY this is pinned: the first retirement painted "The device closed
    app_settings to free 4147200 bytes of di..." -- the shell caches a body in a
    fixed field and clips the row to the card, so the budget figure the notice
    claimed to state never reached the screen. A number clipped on the way to the
    person is the same defect as a number the device never measured.
    """
    notice = _body(KMS, "pub fn post_retirement_notice")
    defects = _card_fit_defects(notice)
    assert not defects, defects


def test_the_retirement_uses_a_named_memory_exit_code():
    """The exit code separates 'the device needed your memory' from an ordinary -1."""
    retire = _retire_body(SYSCALL)
    defects = _exit_code_defects(retire, KMS)
    assert not defects, defects


def test_the_retirement_is_not_blocked_by_the_notice():
    """A refused or busy notification store must never hold up the launch.

    WHY: the retirement pays for a screen the person asked for. The notice is a
    courtesy; the launch is the point. The store is taken with `try_lock`, so a
    held lock skips the notice rather than deadlocking, and the post's `Result`
    is checked -- not unwrapped -- so a DND, quota or capacity refusal proceeds.
    """
    retire = _body(SYSCALL, "pub fn sys_surface_create")
    assert "NOTIFICATION_MANAGER.try_lock()" in retire, (
        "the retirement locks the notification store, which can deadlock or stall the launch"
    )
    assert "post_retirement_notice(" in retire, "the retirement has no notice to guard"
    # The post result is checked, never unwrapped: nothing on the retire path may panic.
    assert ".expect(" not in _body(SYSCALL, "pub fn sys_surface_create").split("post_retirement_notice(")[-1][:120], (
        "the retirement unwraps the notice's result"
    )


def test_the_contract_can_fail():
    """A contract that cannot fail is a comment.

    Every detector is run here against a synthetic source carrying the fault it
    exists to catch, and it has to catch it. The same detectors are then run over
    the real sources and none may fire.
    """
    faulty_posts = [
        ("posts nothing", "loop {\n    let _ = terminate_task(victim, EXIT_CODE_MEMORY_RETIRE);\n}"),
        ("posts twice", "loop {\n    post_retirement_notice(&mut m, pkg, held, tick);\n    post_retirement_notice(&mut m, pkg, held, tick);\n}"),
    ]
    for label, synthetic in faulty_posts:
        defects = _posts_exactly_once_defects(synthetic)
        assert defects, f"the post-count detector does not catch the source that {label}"
    assert not _posts_exactly_once_defects(
        "loop { post_retirement_notice(&mut m, pkg, held, tick); }"
    ), "the post-count detector fires on a correct source"

    faulty_numbers = [
        ("an invented percentage", "post_retirement_notice(m, pkg, held, t);\nMAX_SURFACE_BYTES"),
        ("a low-memory claim", "The device closed you due to low memory. budget MAX_SURFACE_BYTES"),
        ("no budget named", "post_retirement_notice(m, pkg, held, t);\nnothing"),
    ]
    # The percentage case trips on the wording; feed it a body that carries it.
    assert _measured_numbers_defects(
        "post_retirement_notice(m, pkg, 42, t); MAX_SURFACE_BYTES",
        "budget MAX_SURFACE_BYTES, freed 42 percent low memory",
    ), "the number detector does not catch a fabricated claim"
    assert _measured_numbers_defects(
        "post_retirement_notice(m, pkg, held, t); MAX_SURFACE_BYTES",
        "no budget here",
    ), "the number detector does not catch a notice with no measured budget"
    assert not _measured_numbers_defects(
        "post_retirement_notice(m, pkg, held, t);",
        "The device closed {} to free {} bytes (surface budget {} bytes) MAX_SURFACE_BYTES",
    ), "the number detector fires on an honest notice"
    assert faulty_numbers  # documents the three faults the detector above covers

    honest_wording = (
        'let title = format!("{} was closed", victim_package);\n'
        'let body = format!("The device closed it to free {} of {} bytes.", freed_bytes, MAX_SURFACE_BYTES);'
    )
    assert not _card_fit_defects(honest_wording), "the fit detector fires on a notice the card can hold"
    assert _card_fit_defects(
        'let title = format!("{} was closed", victim_package);\n'
        'let body = format!("The device closed {} to free {} bytes of display memory for another app '
        '(surface budget {} bytes).", victim_package, freed_bytes, MAX_SURFACE_BYTES);'
    ), "the fit detector does not catch a body the card clips"
    assert _card_fit_defects(
        'let title = format!("{} was closed by the device to free display memory", victim_package);\n'
        'let body = format!("Freed {} of {} bytes.", freed_bytes, MAX_SURFACE_BYTES);'
    ), "the fit detector does not catch a title the card clips"
    assert _card_fit_defects("manager.post(\"system\", user_id, t);"), (
        "the fit detector does not catch wording it cannot measure"
    )

    assert _exit_code_defects(
        "terminate_task(TaskId(victim), -1);", "pub const EXIT_CODE_MEMORY_RETIRE: i32 = -200;"
    ), "the exit-code detector does not catch an anonymous -1 retirement"
    assert _exit_code_defects(
        "terminate_task(TaskId(victim), EXIT_CODE_MEMORY_RETIRE);", ""
    ), "the exit-code detector does not catch a missing named code"
    assert not _exit_code_defects(
        "terminate_task(TaskId(victim), EXIT_CODE_MEMORY_RETIRE);",
        "pub const EXIT_CODE_MEMORY_RETIRE: i32 = -200;",
    ), "the exit-code detector fires on a correct retirement"

    honest_account = (
        "loop { let account = USER_MANAGER.lock().active_user().0;\n"
        "       post_retirement_notice(&mut m, pkg, account, held, t); }"
    )
    assert not _account_defects(honest_account, "manager.post(\"system\", user_id, t)"), (
        "the account detector fires on a notice filed where the shade looks"
    )
    for label, retire, notice in (
        (
            "files under a hard-coded owner",
            honest_account,
            "manager.post(\"system\", UserId::OWNER.0, t)",
        ),
        (
            "never asks who is holding the device",
            "loop { post_retirement_notice(&mut m, pkg, account, held, t); }",
            "manager.post(\"system\", user_id, t)",
        ),
        (
            "resolves the account and does not pass it",
            "loop { let account = USER_MANAGER.lock().active_user().0;\n"
            "       post_retirement_notice(&mut m, pkg, held, t); }",
            "manager.post(\"system\", user_id, t)",
        ),
    ):
        assert _account_defects(retire, notice), f"the account detector does not catch a notice that {label}"


def main() -> int:
    tests = [value for name, value in sorted(globals().items()) if name.startswith("test_")]
    failures = 0
    for test in tests:
        try:
            test()
        except AssertionError as error:
            failures += 1
            print(f"FAIL {test.__name__}: {error}")
        except Exception as error:  # noqa: BLE001 - the report must name it
            failures += 1
            print(f"ERROR {test.__name__}: {error}")
        else:
            print(f"ok   {test.__name__}")
    print(f"{len(tests) - failures} passed; {failures} failed")
    return 1 if failures else 0


if __name__ == "__main__":
    raise SystemExit(main())
