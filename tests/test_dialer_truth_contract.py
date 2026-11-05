# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

"""Truth contract for the InterOS dialer application.

A dialer that shows "Call active" for a call that never connected is the
clearest possible instance of the Platform Truth Contract being broken: the
user is told something about the telephone that the app cannot know. Every
assertion below pins a specific defect found by reading the app.
"""

from pathlib import Path

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
