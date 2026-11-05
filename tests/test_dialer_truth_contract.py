# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

"""Truth contract for the InterOS dialer application.

A dialer that shows "Call active" for a call that never connected is the
clearest possible instance of the Platform Truth Contract being broken: the
user is told something about the telephone that the app cannot know. Every
assertion below pins a specific defect found by reading the app.
"""

from pathlib import Path
import re

ROOT = Path(__file__).resolve().parents[1]


def read(path: str) -> str:
    return (ROOT / path).read_text(encoding="utf-8")


def dialer_main() -> str:
    return read("apps/dialer/src/main.rs")


def dialer_lib() -> str:
    return read("apps/dialer/src/lib.rs")


def test_the_app_never_claims_a_connected_call_it_did_not_read():
    """`telephony_dial` returns when the modem accepts the request.

    The modem enters its own `Dialing` state at that point, and only the
    modem knows whether a call later connected. The app turned the
    acceptance into "Call active", which the design document forbids
    outright.
    """
    source = dialer_main()
    assert 'CallOutcome::Active => b"Call active"' not in source
    assert 'b"Call active"' not in source
    assert 'CallOutcome::Connected => b"Call connected"' in source
    assert "query_modem_call_state()" in source
    assert "ModemCallState::Connected => CallOutcome::Connected" in source


def test_the_call_state_is_read_from_the_modem_telemetry():
    """The modem writes its own call state into the telemetry payload.

    The state has to be parsed from that field, not inferred from the
    acceptance of the dial request.
    """
    lib = dialer_lib()
    assert "pub fn parse_modem_call_state(" in lib
    assert 'b"\\"call_state\\":\\""' in lib
    assert "pub enum ModemCallState" in lib
    # The parser has to know that "Alerting" is not a connection.
    assert 'token.starts_with(b"Active")' in lib
    assert 'token.starts_with(b"Alerting")' not in lib


def test_the_send_message_control_does_something_real():
    """The pill used to log a dispatched messaging intent and stop there.

    The log line was a claim about an action that never happened, and the
    design document requires every action to map to real behaviour.
    """
    source = dialer_main()
    assert "Dispatched messaging intent" not in source
    assert "fn hand_off_to_messaging(" in source
    assert "std::app_launch(MESSAGING_APP_ID)" in source
    assert 'const MESSAGING_APP_ID: &str = "org.interos.messages";' in source


def test_the_messaging_app_consumes_the_handed_over_recipient():
    """The messaging app shows the recipient before the first send.

    It can only do that if the number reaches it, and the settings store is
    the only channel the two apps share.
    """
    source = dialer_main()
    messages = read("apps/messages/src/main.rs")
    assert 'const HANDOFF_KEY: &str = "dialer.handoff.recipient.v1";' in source
    assert 'const HANDOFF_KEY: &str = "dialer.handoff.recipient.v1";' in messages
    assert "pub fn take_handoff_recipient(" in messages
    assert "messages.take_handoff_recipient();" in messages


def test_the_signal_meter_is_not_filled_from_a_presence_boolean():
    """A present-but-idle radio is not a full-strength radio.

    The meter has to be drawn from the modem's own measured bar count.
    """
    source = dialer_main()
    assert "dialer.signal_bars = if bars > 4 { 4 } else { bars };" in source
    assert "(bar_idx as u8) < dialer.signal_bars" in source


def test_the_contact_capacity_notice_is_not_written_from_a_literal():
    """The store holds twelve contacts.

    The notice used to say the store was full at eight while the full flag
    only appeared at twelve, and the count was written as a single digit, so
    a store of ten contacts printed nine.
    """
    source = dialer_main()
    assert "n = put_u16(&mut line, n, MAX_CONTACTS as u16);" in source
    assert 'b"Saved "' in source
    assert "count.min(9)" not in source


def test_a_press_answers_only_the_pixels_the_pad_and_the_tab_row_ink():
    """A finger on bare page must dial nothing and change nothing.

    The pad inked a disc and answered the whole 180x124 cell drawn around it,
    so the 126 rows of page between two keys in a row typed a digit; the tab
    row inked a capsule and answered the whole 147x44 column, so a thumb on a
    capsule's cut corner changed section. Both defects are the same one: the
    painted box and the tap band were written apart. Each control now states
    its geometry once, in the accessor its painter calls, and the press asks
    that accessor -- so a future cell-band loop in a tap handler is a build
    failure of this contract, not a screen someone has to notice.
    """
    source = dialer_main()

    # The two accessors exist, and the ink asks them.
    assert "fn keypad_key_at(x: usize, y: usize) -> Option<(usize, usize)>" in source
    assert "fn tab_at(x: usize, y: usize) -> Option<usize>" in source
    assert "let (row, col) = match keypad_key_at(x, y) {" in source
    assert "let Some(i) = tab_at(x, y) else {" in source

    # And so does every press: the keypad section, the contact editor's pad,
    # and the tab row shared by both.
    assert source.count("if let Some((row, col)) = keypad_key_at(x, y) {") == 2
    assert "if let Some(i) = tab_at(x, y) {" in source

    # Neither press still walks the box it used to answer.
    assert "if x >= bx && x < bx + BTN_W" not in source
    assert "if x >= TAB1_X0 && x < TAB1_X1" not in source

    # The disc is now the target, so the disc itself carries the finger floor
    # the app states beside it -- the radius is derived from that floor, never
    # authored as a plate that happens to look round.
    assert "const KEY_RADIUS: usize = MIN_FINGER_PX / 2;" in source
    assert "assert!(KEY_RADIUS * 2 >= MIN_FINGER_PX);" in source
    assert "assert!(2 * KEY_RADIUS <= BTN_H);" in source
    assert "assert!(2 * KEY_RADIUS <= BTN_W);" in source


def dialer_sections(source: str) -> list[str]:
    """The section names the app actually has, read from its own enum."""
    body = source.split("pub enum DialerSection {", 1)[1].split("\n}", 1)[0]
    return re.findall(r"^    (\w+) = \d+,", body, re.M)


def tab_row_labels(source: str) -> list[bytes]:
    """The labels the single-sourced tab row lays out, from `lib.rs`."""
    body = source.split("pub mod tab_row {", 1)[1]
    count = re.search(r"pub const COUNT: usize = (\d+);", body)
    items = re.search(r"pub const LABELS: \[&\[u8\]; (\w+)\] = \[([^\]]*)\];", body)
    assert count and items, (
        "the tab row no longer states its tab count and its labels as one "
        "declaration each in `tab_row`, so this gate cannot compare the row "
        "with the sections the app has"
    )
    assert items.group(1) == "COUNT", (
        "the label array no longer length-checks itself against `COUNT`"
    )
    labels = re.findall(rb"b\"([A-Za-z ]+)\"", items.group(2).encode())
    assert len(labels) == int(count.group(1)), (
        f"`tab_row` declares {count.group(1)} tabs but letters {len(labels)}"
    )
    return labels


def test_exactly_one_navigation_bar_letters_the_app():
    """One screen, one selected tab.

    The app used to paint two section switchers at once: the segmented row at
    the top, lettered from `tab_row`, and a bottom dock lettered Favorites,
    Recents, Keypad, Contacts. Both read `dialer.section`, so the Keypad screen
    inked "Dial" selected at the top and "Keypad" selected at the bottom in the
    same frame, and the Settings screen inked one selected and none -- the dock
    had no slot for it. Two bars for one state is not a redundant control, it
    is two answers to the same question.

    The row wins: it names every section the enum has, its geometry is
    single-sourced in `tab_row` with build-time ties, and its painter and its
    press share one accessor. The dock is gone, and the one control only it
    offered -- the starred-contacts filter -- is still answered by the Contacts
    section's own disc, so nothing was lost with the bar.
    """
    source = dialer_main()
    sections = dialer_sections(source)

    # One switcher, and it is the one the library lays out.
    assert len(tab_row_labels(dialer_lib())) == len(sections), (
        "the tab row and the app's own section enum no longer agree on how many "
        "sections there are: one bar letters a screen the other cannot reach"
    )
    assert source.count("return tabs_pixel(x, y, dialer);") == 1
    assert source.count("fn tabs_pixel(") == 1

    # The dock is not merely hidden: its painter, its geometry and its press are
    # all absent, so no future screen can be added to one bar and not the other.
    for name in ("dock_pixel", "dock_label_y0", "DOCK_Y0", "DOCK_Y1", "DOCK_ITEM_W",
                 "DOCK_ITEM_X0", "DOCK_LABELS", "DOCK_ICON_Y0", "DOCK_ICON_H",
                 "DOCK_SCALE"):
        assert name not in source, f"the second navigation bar is still in the source: {name}"

    # Nothing else claims the free band below the content: it belongs to the
    # gesture area, so the app cannot grow a third bar into it.
    assert "(CONTENT_Y1..chrome::HOME_HIT.y).contains(&y)" in source
    assert "DOCK_Y0)" not in source

    # The capability the dock held alone is still live: the starred filter is
    # both lettered and answered inside the Contacts section.
    assert 'b"FAVS"' in source and 'b"ALL"' in source
    assert "dialer.contacts_fav_only = !dialer.contacts_fav_only;" in source

