#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

"""The compose screen has to be able to type.

WHAT THIS FILE IS FOR: tapping the recipient field or the message body
gave the field focus and logged that it was focused, and no keyboard ever
appeared on the screen. The compose screen was a dead end -- there was no
way to type a message at all. Every test below pins one part of the fix,
so the dead end cannot come back unnoticed.

HOW THE CONTRACT CAN FAIL: each detector takes a source string rather
than reading the file itself, and `test_the_contract_can_fail` runs the
same detectors over a synthetic source carrying the fault. A contract that
cannot fail is a comment.

WHERE THE SOURCE IS: the messages app is a SYSTEM app, so its canonical
crate lives in this repository and is copied out to the interos-apps
mirror. The copy has to be the same bytes, and that is a test too.

Run it directly, or under pytest.
"""

from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]

# The canonical crate, and the mirror it is copied out to.
#
# WHY the mirror's path has no `apps` in it: the system apps are copied
# out of the core repository into the mirror repository's own top level,
# and `tools/mirror_apps.py` is what puts them there.
CORE = ROOT / "apps" / "messages"
MIRROR = ROOT.parent / "interos-apps" / "messages"


def read(path: str) -> str:
    return (ROOT / path).read_text(encoding="utf-8")


def messages_main() -> str:
    return read("apps/messages/src/main.rs")


def messages_keyboard() -> str:
    return read("apps/messages/src/keyboard.rs")


def messages_lib() -> str:
    return read("apps/messages/src/lib.rs")


def _tap_handler(source: str) -> str:
    """The tap handler's own source, without the rest of the file.

    WHY it is cut out and not searched whole: the ordering that matters is
    the ordering inside one handler, and a search over the whole file would
    find the same name in a different function and call it a hit.
    """
    start = source.index("fn handle_tap_inner")
    end = source.index("fn handle_messages_key")
    return source[start:end]


def _key_handler(source: str) -> str:
    """The on-screen keyboard's key handler, without the rest of the file."""
    start = source.index("fn apply_keyboard_key")
    end = source.index("fn edit_recipient")
    return source[start:end]


# -----------------------------------------------------------------------------
# The detectors
# -----------------------------------------------------------------------------

def _draws_the_platforms_own_keyboard(main: str, keyboard: str) -> bool:
    """The screen shows the platform's soft keyboard, not a private one."""
    return (
        "use interos_std::design::components::SoftKeyboard;" in keyboard
        and "render_pixel(x, y, &THEME)" in main
        and "y >= KB_Y0 && y < KB_Y1" in main
        # WHY no key table of its own: a second table of keys is a second
        # answer to where a key is, and the key under the finger would not
        # be the key that is drawn.
        and "KeyDef" not in keyboard
    )


def _focus_lives_in_the_keyboard(main: str, keyboard: str) -> bool:
    """The focused field is the keyboard's own fact, not three flags."""
    return (
        "pub keyboard: Keyboard," in main
        and "pub mod keyboard;" in read("apps/messages/src/lib.rs")
        and "compose_focused" not in main
        and "search_focused" not in main
        and "recip_focused" not in main
        and "pub enum Field" in keyboard
    )


def _bands_follow_the_keyboard(main: str, keyboard: str) -> bool:
    """The editor bands sit where the keyboard is, not at a typed-in row."""
    body = keyboard.split("pub fn bands_top", 1)[-1].split("\n}", 1)[0]
    return (
        "keyboard.is_visible()" in body
        and "messages::keyboard::bands_top(&app.keyboard)" in main
        # WHY the typed-in rows are gone: a band whose Y is written down
        # is a band that stays where it was written when the keyboard
        # takes the bottom of the screen.
        and "const RECIPIENT_BAND_Y0" not in main
        and "const COMPOSE_BAND_Y0" not in main
        and "const RECIPIENT_BAND_Y1" not in main
        and "const COMPOSE_BAND_Y1" not in main
    )


def _redraw_covers_the_dock_and_both_band_places(main: str, keyboard: str) -> bool:
    """A keyboard change repaints the dock and both places the bands can be."""
    body = keyboard.split("pub fn dirty_region", 1)[-1].split("\n}", 1)[0]
    return (
        "BANDS_TOP_IDLE" in body
        and "BANDS_TOP_KEYBOARD" in body
        and "KB_Y1" in body
        and "messages::keyboard::dirty_region(&messages.keyboard)" in main
    )


def _index_of(handler: str, needle: str) -> int:
    """Where the needle is in the handler, or a number past its end.

    WHY a missing needle is not an error: the detector asks about the
    order of the checks that are there, and a handler that does not have
    one of them has nothing behind the keyboard to fall through to.
    """
    found = handler.find(needle)
    return found if found >= 0 else len(handler)


def _keyboard_is_hit_before_the_screen_behind_it(main: str, keyboard: str) -> bool:
    """A press on the keys never reaches the list under the keyboard."""
    handler = _tap_handler(main)
    dock = _index_of(handler, "y >= KB_Y0 && y < KB_Y1")
    if dock == len(handler):
        return False
    for behind in (
        "SEND_CX as isize",
        "in_rect(compose_field(app), x, y)",
        "in_rect(recipient_field(app), x, y)",
        "in_rect(SEARCH_FIELD, x, y)",
    ):
        if _index_of(handler, behind) < dock:
            return False
    return True


def _a_key_reaches_the_focused_field(main: str, keyboard: str) -> bool:
    """A key press goes to the field the caret is in, and to no other."""
    return (
        "pub fn apply_to(" in keyboard
        and "pub search: Editor<'a>," in keyboard
        and "pub recipient: Editor<'a>," in keyboard
        and "pub body: Editor<'a>," in keyboard
        and "app.keyboard.apply_to(action, &mut editors)" in main
        # WHY the low-level path may not be used by the app: it takes the
        # buffer as an argument, so a caller could type into a field the
        # caret is not in.
        and "keyboard.apply(" not in main
    )


def _the_keyboard_cannot_claim_a_delivery(main: str, keyboard: str) -> bool:
    """The keyboard types; it never says a message was sent."""
    return (
        "send_composed" not in keyboard
        and "DeliveryStatus" not in keyboard
        and "telephony_send_sms" not in keyboard
        # WHY the on-screen enter key may not send: sending is the one
        # action in this app that asks a transport whether it happened, and
        # it belongs to the control that says so out loud.
        and "send_composed" not in _key_handler(main)
        # The send path itself still asks the transport, and keeps saying
        # what the transport said.
        and "fn attempt_transport(" in main
        and "TransportOutcome::Accepted => DeliveryStatus::Sent" in main
    )


# -----------------------------------------------------------------------------
# The faults that were removed
# -----------------------------------------------------------------------------

def test_the_compose_screen_draws_the_platforms_own_keyboard():
    """The compose screen used to show no keyboard at all.

    The field took focus, the app logged that it was focused, and the
    screen stayed exactly as it was. A keyboard that is not drawn is a
    keyboard that does not exist, whatever the state says.
    """
    assert _draws_the_platforms_own_keyboard(messages_main(), messages_keyboard())


def test_the_focus_flags_that_left_the_compose_screen_a_dead_end_are_gone():
    """Three booleans said which field had focus and nothing drew a keyboard.

    The booleans are how the dead end was spelled: a field could be
    focused with nothing on the screen to type with. The focus is now the
    keyboard's own fact, so a focused field and a visible keyboard cannot
    disagree.
    """
    assert _focus_lives_in_the_keyboard(messages_main(), messages_keyboard())


def test_the_editor_bands_follow_the_keyboard():
    """The bands used to sit at a row that was written into the source.

    The keyboard takes the bottom of the screen, and a compose control the
    keyboard covers is a control the user cannot reach, so the bands have
    to move with the keyboard.
    """
    assert _bands_follow_the_keyboard(messages_main(), messages_keyboard())


def test_the_keyboard_change_repaints_the_dock_and_both_band_places():
    """The bands move, so a redraw that covered one place left the other.

    The old pixels of the bands would stay on the screen at the place the
    bands used to be, and the composer would look like it was in two
    places at once.
    """
    assert _redraw_covers_the_dock_and_both_band_places(
        messages_main(), messages_keyboard()
    )


def test_a_press_on_the_keys_never_reaches_the_screen_behind_them():
    """The keyboard is the topmost thing on the screen while it is up.

    A press that fell through to the conversation list under the dock
    would open a thread while the user was typing a message.
    """
    assert _keyboard_is_hit_before_the_screen_behind_it(
        messages_main(), messages_keyboard()
    )


def test_a_key_press_reaches_the_focused_field_and_no_other():
    """The three editors are named by the keyboard, not by the caller.

    A caller that picked the buffer itself could type into a field the
    caret is not in, and the user would see a character appear in the
    wrong field.
    """
    assert _a_key_reaches_the_focused_field(messages_main(), messages_keyboard())


def test_the_keyboard_types_and_does_not_claim_a_delivery():
    """A keyboard is not a transport.

    The on-screen enter key dismisses the keyboard, and the send button
    is the only way a message leaves this app -- and that button still
    asks the transport and reports what the transport said.
    """
    assert _the_keyboard_cannot_claim_a_delivery(messages_main(), messages_keyboard())


def test_the_mirrored_crate_is_the_canonical_crate():
    """The public repository's copy has to be the canonical one.

    A contract that reads a stale copy tests nothing, so when both copies
    are present they must be the same bytes.
    """
    if MIRROR.is_dir():
        for name in ("lib.rs", "main.rs", "keyboard.rs"):
            a = (CORE / "src" / name).read_bytes()
            b = (MIRROR / "src" / name).read_bytes()
            assert a == b, (
                f"the mirrored {name} is not the canonical one; "
                "run tools/mirror_apps.py messages in interos-apps"
            )


# -----------------------------------------------------------------------------
# The contract has to be able to fail
# -----------------------------------------------------------------------------

def test_the_contract_can_fail():
    """A contract that cannot fail is a comment.

    Every detector is run here against a source that carries the fault it
    exists to catch, and the detector has to catch it.
    """
    good_main = messages_main()
    good_keyboard = messages_keyboard()

    # A keyboard that is never drawn.
    assert not _draws_the_platforms_own_keyboard(good_main.replace(
        "render_pixel(x, y, &THEME)", "fill_rect(x, y, 10, 10)"
    ), good_keyboard)

    # The focus flags that left the compose screen a dead end.
    assert not _focus_lives_in_the_keyboard(
        good_main.replace("pub keyboard: Keyboard,", "pub compose_focused: bool,"),
        good_keyboard,
    )

    # Bands at a row that was written into the source.
    assert not _bands_follow_the_keyboard(
        good_main.replace(
            "messages::keyboard::bands_top(&app.keyboard)", "1284"
        ),
        good_keyboard,
    )

    # A redraw that covers only the place the bands are in now.
    assert not _redraw_covers_the_dock_and_both_band_places(
        good_main.replace(
            "messages::keyboard::dirty_region(&messages.keyboard)",
            "(BANDS_TOP_KEYBOARD, KB_Y1)",
        ),
        good_keyboard,
    )

    # A press on the keys that falls through to the screen behind them.
    faulty_handler = (
        "fn handle_tap_inner(app: &mut MessagesApp, px: u32, py: u32) -> Tap<'static> {\n"
        "    if in_rect(SEARCH_FIELD, x, y) { return Tap::None; }\n"
        "    if app.keyboard.is_visible() && y >= KB_Y0 && y < KB_Y1 {\n"
        "        return Tap::None;\n"
        "    }\n"
        "}\n"
        "fn handle_messages_key() {}\n"
    )
    assert not _keyboard_is_hit_before_the_screen_behind_it(
        faulty_handler, good_keyboard
    )

    # A key that is typed into a buffer the caller picked.
    assert not _a_key_reaches_the_focused_field(
        good_main.replace(
            "app.keyboard.apply_to(action, &mut editors)",
            "app.keyboard.apply(action, &mut app.compose, &mut app.compose_len, "
            "&mut app.compose_select_all)",
        ),
        good_keyboard,
    )

    # A keyboard that sends the message.
    assert not _the_keyboard_cannot_claim_a_delivery(
        good_main.replace(
            "KeyAction::Hide | KeyAction::Enter => {",
            "KeyAction::Hide | KeyAction::Enter => { send_composed(app);",
        ),
        good_keyboard,
    )


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
