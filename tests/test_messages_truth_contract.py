"""Truth contract for the InterOS messages application.

Messaging is where a fabricated delivery state does the most damage: a
message that never left the device, labelled "Sent", is a lie the user acts
on. Every assertion below pins a specific defect found by reading the app,
and each one is a rule that has to keep holding.
"""

from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]


def read(path: str) -> str:
    return (ROOT / path).read_text(encoding="utf-8")


def messages_main() -> str:
    return read("apps/messages/src/main.rs")


def messages_lib() -> str:
    return read("apps/messages/src/lib.rs")


def messages_store() -> str:
    return read("apps/messages/src/store.rs")


def test_a_delivery_state_comes_from_the_transport_not_from_hardware_presence():
    """The delivery state used to be derived from a modem-presence query.

    A present modem therefore produced "Sent" without a single message ever
    being handed to the transport. The app has to ask the transport and keep
    the transport's answer.
    """
    source = messages_main()
    assert "std::telephony_send_sms(" in source
    assert "fn attempt_transport(" in source
    assert "TransportOutcome::Accepted => DeliveryStatus::Sent" in source
    assert "if app.modem_present {\n        DeliveryStatus::Sent" not in source
    assert "DeliveryStatus::Sent" in source
    # The presence probe may stay, but it may not decide the delivery state.
    assert "modem_present" not in source.split("fn send_composed")[1].split("fn ")[0]


def test_no_state_claims_a_queue_or_a_delivery_the_app_cannot_support():
    """`Pending` says a queue exists. No queue exists.

    The bubble subline has to say what is true: "Sent", "Failed",
    "Refused - No Transport", "Local only - not sent" or "Not sent".
    """
    lib = messages_lib()
    assert 'Self::Pending => "Not sent"' in lib
    assert 'Self::Pending => "Pending"' not in lib
    assert 'Self::LocalOnly => "Local only - not sent"' in lib
    assert 'Self::Sent => "Sent"' in lib
    assert 'Self::Failed => "Failed"' in lib


def test_the_thread_store_capacity_is_derived_from_the_record_width():
    """The store claimed nine threads and the record width made that false.

    A record is 78 bytes and the settings kernel caps a value at 512 bytes,
    so six threads fit and seven do not. The old constant said nine, so the
    seventh thread was accepted, refused by the kernel, and the refusal
    thrown away.
    """
    store = messages_store()
    assert "pub const SETTINGS_VALUE_MAX: usize = 512;" in store
    assert "pub const MAX_THREADS: usize = (SETTINGS_VALUE_MAX - 2) / RECORD_BYTES;" in store
    assert "const MAX_THREADS: usize = 9;" not in store
    assert "const _: () = assert!(STORE_BLOB_BYTES <= SETTINGS_VALUE_MAX);" in store
    # The old comment claimed a 52-byte record and a 470-byte blob.
    assert "2 + 9 * 52 = 470" not in store


def test_a_refused_store_read_reaches_the_screen():
    """The store recorded that its read was refused and nothing drew it.

    An unreadable store and a device with no conversations are different
    facts, and the thread list has to say which one it is.
    """
    source = messages_main()
    assert "StoreState::ReadRefused" in source
    assert "StoreState::Invalid" in source
    assert "Stored conversations could not be read" in source
    assert "Stored conversations failed validation" in source


def test_a_refused_store_write_reaches_the_screen():
    """The append's result used to be discarded.

    The store is bounded, so an append past the cap is refused, and a thread
    that could not be persisted looked exactly like one that could.
    """
    source = messages_main()
    assert "HeaderStatus::StoreRefused" in source
    assert 'HeaderStatus::StoreRefused => b"STORE REFUSED"' in source
    assert "if app.store.append(" in source


def test_the_threat_banner_describes_the_pattern_not_the_sender():
    """The banner called the sender unverified.

    The app verified nothing. What it has is a local pattern match on the
    message text, and the two severities used to read identically, so a
    scam-script match and a mere urgency match were indistinguishable.
    """
    source = messages_main()
    assert "Caution: Unverified urgent request" not in source
    assert "Caution: text matches a known scam script" in source
    assert "Caution: text matches urgency language" in source


def test_the_startup_line_does_not_announce_a_wire_protocol():
    """The app bound no transport and never had a P2P wire.

    A startup line claiming a wire protocol was online was a claim about a
    channel the app did not have.
    """
    source = messages_main()
    assert "Sovereign Encrypted Messaging & P2P Wire Protocol online" not in source
    assert "no messaging transport is bound" in source


def test_the_message_text_is_drawn_with_the_outline_typeface():
    """The panel is 1080x2160 and the app paints at 720x1440.

    A bitmap run is magnified 3:2 on the way to the screen and reads as
    soft. The outline face rasterises the real outline and reads as sharp.
    """
    source = messages_main()
    assert "use interos_std::design::outline_text;" in source
    assert "outline_text::place_run" in source
    assert "outline_text::run_box" in source


def test_the_character_counter_limit_comes_from_the_compose_capacity():
    """The denominator was typed in as "160".

    The compose buffer's capacity is the limit the app enforces, and a
    typed-in denominator drifts away from it the first time the capacity
    changes.
    """
    source = messages_main()
    assert "let mut x = len.min(COMPOSE_CAP);" in source
    assert "out[o + 1] = b'1';" not in source
    assert "let mut lv = COMPOSE_CAP;" in source


def test_a_local_handle_is_not_handed_to_the_transport():
    """A name is not a number a modem can dial.

    Handing one to a transport is not a transmission, it is a mistake, and
    the message has to stay local with the app saying so.
    """
    lib = messages_lib()
    assert "pub fn is_dialable(" in lib
    assert "TransportOutcome::NotDialable => DeliveryStatus::LocalOnly" in messages_main()
