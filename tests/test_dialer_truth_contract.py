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
