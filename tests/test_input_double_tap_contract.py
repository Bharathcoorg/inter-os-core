#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

"""Static contracts for the second of two quick presses.

WHAT THIS FILE IS FOR: the kernel's gesture router emits exactly one input
event per physical release. A second press within the double-tap window at
the same spot is classified `DoubleTap` (kind 2) instead of a second `Tap`
(kind 1), so the second press leaves the Tap stream. An application that
gates its press handling on kind 1 alone therefore loses that press: the
second of two identical letters in an on-screen keyboard never arrives, a
second press on the same button does nothing, and the person cannot tell the
difference between a device that ignored them and a device that is merely
slow.

The fault was found on the real guest, not by reading: typing a title on the
calendar's own soft keyboard delivered "QEMUACCEPT" with one of the doubled
letters missing, and the guest's own log said `got DoubleTap` for exactly
that press.

Every detector below is run against the source, and the last test runs each
detector against a deliberately broken copy of that source: a detector that
cannot catch the fault is a detector that tests nothing.
"""

import re
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]

# The applications that read the normalized input stream. Every one of them
# answers presses, so every one of them has to answer the second press too.
INPUT_APPS = (
    "agent_manager",
    "browser",
    "calculator",
    "calendar",
    "camera",
    "dialer",
    "file_manager",
    "gallery",
    "launcher",
    "messages",
    "music",
    "notes",
    "settings",
    "vault",
    "video_player",
    "webview_demo",
)

# The one application that owns a double tap of its own: the full-screen
# photo viewer toggles between fit and 2x on a double tap, and the gallery
# says so in the log line it prints when it does.
DOUBLE_TAP_OWNERS = ("gallery",)


def read(path: str) -> str:
    return (ROOT / path).read_text(encoding="utf-8")


def _code_only(source: str) -> str:
    """Strips comments so a rule is asserted against the program.

    WHY: the comments in these apps quote, at length, the very literals these
    rules forbid -- the kernel's kind numbers, the names of the gestures.
    """
    without_block = re.sub(r"/\*.*?\*/", " ", source, flags=re.S)
    return re.sub(r"//[^\n]*", " ", without_block)


def _app_code(app: str) -> str:
    """The whole application source, comments stripped, as one string."""
    parts = []
    for path in sorted((ROOT / "apps" / app / "src").rglob("*.rs")):
        parts.append(_code_only(path.read_text(encoding="utf-8")))
    return "\n".join(parts)


def _the_sdk_helper_is_whole(sdk: str) -> bool:
    """Whether the SDK's shared predicate answers both press kinds.

    WHY the body is checked and not just the name: a helper that was later
    narrowed to kind 1 alone would compile everywhere and silently bring the
    whole fault back.
    """
    if "pub const fn input_event_is_tap(kind: u8) -> bool" not in sdk:
        return False
    if "pub const INPUT_EVENT_KIND_TAP: u8 = 1;" not in sdk:
        return False
    if "pub const INPUT_EVENT_KIND_DOUBLE_TAP: u8 = 2;" not in sdk:
        return False
    body = sdk.split("pub const fn input_event_is_tap(kind: u8) -> bool", 1)[1]
    body = body.split("}", 1)[0]
    return "INPUT_EVENT_KIND_TAP" in body and "INPUT_EVENT_KIND_DOUBLE_TAP" in body


def _the_app_answers_both_presses(code: str) -> bool:
    """Whether an application answers kind 2 as a press.

    WHY two shapes are accepted: most apps call the SDK's shared predicate,
    and one app owns a predicate of its own beside the kind constants it
    already declares. What both shapes have in common is that they name the
    second press kind.
    """
    if "input_event_is_tap(input.kind)" in code:
        return True
    if re.search(r"fn\s+is_tap\s*\(kind:\s*u8\)\s*->\s*bool", code):
        body = code.split("fn is_tap(kind: u8) -> bool", 1)[1].split("}", 1)[0]
        return "DOUBLE_TAP" in body or "DOUBLE" in body
    return False


def _drops_the_second_press(code: str) -> bool:
    """Whether an application gates a press on kind 1 alone.

    WHY the comparison is looked for as text: the kernel's kind numbers are
    the ABI, and an app that writes `input.kind == 1` has said in as many
    words that it answers the first press and not the second.
    """
    return re.search(r"input\.kind\s*==\s*1\b", code) is not None


# -----------------------------------------------------------------------------
# The detectors
# -----------------------------------------------------------------------------


def test_the_kernel_names_both_press_kinds_and_keeps_one_event_per_release():
    """The kernel is the source of the split, so the split is pinned here.

    WHY this matters: the SDK and every application answer "kind 1 is a
    press, kind 2 is also a press". If the kernel renumbered those kinds, or
    started emitting two events for a double tap, every one of those
    answers would be answering the wrong thing.
    """
    kinds = read("kernel/src/drivers/input.rs")
    router = read("kernel/src/input/router.rs")

    assert "Tap = 1," in kinds, "the kernel's plain tap kind is no longer 1"
    assert "DoubleTap = 2," in kinds, "the kernel's double tap kind is no longer 2"
    assert "pub enum InputEventKind" in kinds
    assert "DoubleTap => InputEventKind::DoubleTap" in kinds, (
        "the canonical gesture stream no longer maps its double tap onto kind 2"
    )

    assert "DOUBLE_TAP_WINDOW_TICKS: u64 = 300" in router, (
        "the double-tap window is no longer the window the SDK's helper assumes"
    )
    assert "TAP_SLOP: i32 = 24" in router, (
        "the tap slop is gone, so the window no longer means 'the same spot'"
    )
    assert "GestureKind::DoubleTap" in router, (
        "the router no longer classifies the second of two quick presses"
    )


def test_the_sdk_offers_one_shared_answer_for_both_presses():
    """The shared predicate exists and answers both kinds.

    WHY the SDK and not a per-app constant: seventeen applications that each
    spell out "kind 1 or kind 2" is seventeen chances to spell it wrong, and
    one app did -- it answered kind 1 alone and lost every second press.
    """
    sdk = read("sdk/rust/interos-std/src/lib.rs")
    assert _the_sdk_helper_is_whole(sdk), (
        "the SDK's shared tap predicate is missing, or answers only one of the "
        "two press kinds"
    )


def test_no_application_drops_the_second_of_two_quick_presses():
    """Every application that reads input answers the second press.

    WHY this is the rule the guest broke: the calendar's soft keyboard
    delivered "QEMUACCEPT" with one doubled letter missing, because the
    second press on the same key arrived as kind 2 and the calendar answered
    kind 1 only.
    """
    for app in INPUT_APPS:
        code = _app_code(app)
        assert not _drops_the_second_press(code), (
            f"{app} gates a press on kind 1 alone, so the second of two quick "
            "presses on the same control is dropped"
        )
        assert _the_app_answers_both_presses(code), (
            f"{app} reads the input stream but never answers the second press "
            "kind"
        )


def test_the_gallery_keeps_the_double_tap_it_owns():
    """The one application with a double tap of its own keeps it.

    WHY this is pinned rather than waved through: the full-screen photo
    viewer toggles between fit and 2x on a double tap. A rule that answered
    every kind 2 as a plain tap would run the control under the finger a
    second time on top of the zoom -- so the app has to claim that press
    before the tap branch sees it.
    """
    for app in DOUBLE_TAP_OWNERS:
        code = _app_code(app)
        assert "input.kind == 2" in code, (
            f"{app} owned a double tap of its own and no longer claims it"
        )
        assert "double_tap_zoom" in code, (
            f"{app} no longer has the double-tap zoom its viewer draws"
        )
        assert re.search(r"!double_tap_claimed\s*&&", code), (
            f"{app} answers the double tap as a tap as well, so the zoom and "
            "the control under the finger both run"
        )


def test_the_detectors_catch_the_fault_they_exist_for():
    """Each detector is run against a source carrying the fault it pins.

    WHY: a detector that passes against a broken source is a detector that
    tests nothing, and this file's whole claim is that the second press is
    not lost.
    """
    sdk = read("sdk/rust/interos-std/src/lib.rs")
    assert not _the_sdk_helper_is_whole(
        sdk.replace(
            "kind == INPUT_EVENT_KIND_TAP || kind == INPUT_EVENT_KIND_DOUBLE_TAP",
            "kind == INPUT_EVENT_KIND_TAP",
        )
    ), "the SDK detector accepts a predicate that answers only kind 1"

    assert _drops_the_second_press(
        _code_only("if input.kind == 1 { handle_tap(); }")
    ), "the dropped-press detector accepts a source that answers kind 1 alone"

    assert not _the_app_answers_both_presses(
        _code_only("if input.kind == 1 { handle_tap(); }")
    ), "the answers-both-presses detector accepts a source that answers kind 1 alone"

    assert not _the_app_answers_both_presses(
        _code_only("fn is_tap(kind: u8) -> bool { kind == 1 }")
    ), "the answers-both-presses detector accepts a local predicate that answers kind 1 alone"


def main():
    test_the_kernel_names_both_press_kinds_and_keeps_one_event_per_release()
    print("input double-tap kernel contract: PASS")

    test_the_sdk_offers_one_shared_answer_for_both_presses()
    print("input double-tap SDK contract: PASS")

    test_no_application_drops_the_second_of_two_quick_presses()
    print("input double-tap application contract: PASS")

    test_the_gallery_keeps_the_double_tap_it_owns()
    print("input double-tap gallery contract: PASS")

    test_the_detectors_catch_the_fault_they_exist_for()
    print("input double-tap detector self-test: PASS")


if __name__ == "__main__":
    main()
