# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

"""Truth contract for the InterOS messages application.

Messaging is where a fabricated delivery state does the most damage: a
message that never left the device, labelled "Sent", is a lie the user acts
on. Every assertion below pins a specific defect found by reading the app,
and each one is a rule that has to keep holding.
"""

from pathlib import Path
import re

ROOT = Path(__file__).resolve().parents[1]

_COMMENT_OR_LITERAL = re.compile(
    r"//[^\n]*"
    r"|/\*.*?\*/"
    r'|"(?:[^"\\]|\\.)*"'
    r'|(?:b|br|r#*)"(?:[^"\\]|\\.)*"#*',
    re.DOTALL,
)


def _code_only(source: str) -> str:
    """`source` with its comments and string literals dropped.

    WHY a refusal is looked for in the code alone: a comment that quotes the defect
    is a note about it, not the defect, and it must neither trip a pin nor satisfy
    one.
    """
    return _COMMENT_OR_LITERAL.sub(" ", source)


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

    WHY the words and the pairing are checked in two files: the sentence the
    card paints is a layout constant in `lib.rs` and the choice of which one
    the store's state selects is a match in `main.rs`, so pinning both in
    whichever file happens to hold the literal is how a rename of either half
    passes while the screen paints the wrong fact.
    """
    source = messages_main()
    layout = messages_lib()
    assert "StoreState::ReadRefused" in source
    assert "StoreState::Invalid" in source
    # The state names the subline the card draws, and that subline says what
    # happened to the stored conversations.
    assert "layout::EMPTY_STORE_REFUSED_SUBLINE" in source
    assert "layout::EMPTY_STORE_INVALID_SUBLINE" in source
    assert 'b"Stored conversations could not be read"' in layout
    assert 'b"Stored conversations failed validation"' in layout
    # The refused arm names the refused subline, not the transport's: the two
    # were one card before, and a name that is merely present somewhere in the
    # file would still pass if the arm reached for the wrong constant.
    refused_arm = source.split("StoreState::ReadRefused => (")[1].split("StoreState::")[0]
    assert "EMPTY_STORE_REFUSED_SUBLINE" in refused_arm
    assert "EMPTY_TRANSPORT_SUBLINE" not in refused_arm


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
    assert "Caution: text matches urgency language" in source


def test_the_threat_banner_does_not_claim_a_scam_script_database():
    """The banner said the text "matches a known scam script".

    There is no scam-script database on the device. The analyzer holds a
    fixed list of phishing words and a fixed list of link prefixes and
    matches the text against them, so a badge that implies a database
    claims more evidence than the code has.
    """
    source = messages_main()
    assert "Caution: text matches a known scam script" not in source
    assert "Caution: text matches local phishing keywords" in source
    # The analyzer is a keyword list, and the badge has to say so.
    gsm = read("apps/messages/src/gsm.rs")
    assert "PHISHING_PATTERNS" in gsm
    assert "SUSPICIOUS_LINK_PATTERNS" in gsm


def test_an_incoming_record_says_it_was_written_on_this_device():
    """The bubble said "Received (no transport)".

    Nothing can arrive: no transport is bound to this app. A record whose
    direction is incoming can only be one this device wrote, so the label
    has to say that.
    """
    lib = messages_lib()
    assert "Received (no transport)" not in read("apps/messages/src/main.rs")
    assert "pub const INCOMING_LOCAL_LABEL" in lib
    assert "never received" in lib


def test_the_thread_list_and_the_conversation_scroll():
    """A long thread was cut off with no way out of it.

    A thread holds sixteen messages and the visible band holds twelve, so
    the oldest four were drawn off the top of the screen with no way to
    reach them.
    """
    source = messages_main()
    assert "use interos_std::design::scroll::ScrollState;" in source
    assert "pub list_scroll: ScrollState" in source
    assert "pub convo_scroll: ScrollState" in source
    assert "convo_scroll.apply_drag(" in source
    assert "list_scroll.apply_drag(" in source


def test_the_sealed_thread_is_published_as_an_indexed_document():
    """The sealed thread lived in a vault sector, invisible to every other app.

    A vault sector is a secret, so the file manager could not list the
    thread and the launcher could not find it. The app writes a transcript
    into the shared storage area, where the platform's document index
    describes it.
    """
    publish = read("apps/messages/src/publish.rs")
    assert 'pub const STORAGE_ROOT: &str = "/storage";' in publish
    assert "std::fs_open(" in publish
    assert "std::content::content_index_refresh()" in publish
    # The hand-off names the media type the registry knows.
    assert "std::content::intent_resolve(" in publish
    assert "std::content::intent_dispatch(" in publish
    assert 'pub const THREAD_DOC_MIME: &str = "text/plain";' in publish


def test_the_privacy_setting_lives_in_the_platform_settings_store():
    """A per-contact privacy setting has to outlive the app that set it.

    A flag kept in the app's own memory is forgotten the moment the app
    closes, and a privacy control that forgets itself is not a privacy
    control.
    """
    privacy = read("apps/messages/src/privacy.rs")
    assert "std::settings_get(" in privacy
    assert "std::settings_set(" in privacy
    assert 'pub const PRIVACY_KEY_PREFIX: &str = "messages.privacy.v1.";' in privacy


def test_the_unavailable_platform_features_are_reported_not_drawn():
    """The design document asks for read receipts, notifications and files.

    The platform offers none of them to this app, so the details screen has
    to say so. A control that does nothing is worse than a missing one.
    """
    source = messages_main()
    assert "Unavailable: no transport is bound" in source
    assert "Unavailable: no channel is bound" in source
    assert "Unavailable: no attachment service" in source


def test_the_startup_line_does_not_announce_a_wire_protocol():
    """The app bound no transport and never had a P2P wire.

    A startup line claiming a wire protocol was online was a claim about a
    channel the app did not have.
    """
    source = messages_main()
    assert "Sovereign Encrypted Messaging & P2P Wire Protocol online" not in source
    assert "no messaging transport is bound" in source


def test_the_message_text_is_drawn_with_the_outline_typeface():
    """The panel and the app's surface are both 720x1440.

    The compositor copies the surface 1:1. The 16x24 bitmap face would be
    magnified to the screen's type sizes and read soft; the outline face
    rasterises the real outline and reads as sharp.
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


def test_the_send_reports_what_happened_instead_of_a_fixed_summary():
    """The send site printed "Message composed and processed." on every path.

    A press with no recipient had already been answered with "nothing was
    stored locally", and the summary line printed straight over the top of it,
    so the log claimed a completed send for a message the app had thrown away.
    The transport was the other half of the gap: it said nothing when it
    accepted a message and nothing when the recipient was not dialable, so the
    only positive statement on those paths was the unearned summary.

    Each outcome has to be said by the code that observed it, and the summary
    has to be gone.
    """
    source = messages_main()
    assert "Message composed and processed" not in source
    assert "The telephony service accepted the composed message" in source
    assert "The recipient is not a dialable number" in source
    assert "Telephony refused the composed message" in source
    # The paths that already reported their own answer keep reporting it.
    assert "Send pressed with no recipient; nothing was stored locally" in source
    assert "Send pressed with an empty composer; nothing was sealed" in source
    # And the accepted and not-dialable statements live inside attempt_transport,
    # not at a call site that could print them before the call returns.
    transport = source.split("fn attempt_transport(")[1].split("\nfn ")[0]
    assert "The telephony service accepted the composed message" in transport
    assert "The recipient is not a dialable number" in transport


def _refused_trim_faults(lib: str, main: str) -> list[str]:
    """Every half of the refused-trim rule, as faults rather than as asserts.

    `font::ellipsize` answers `0` when the lane cannot even hold its three-byte
    mark, and the answer to that refusal has to be the leading glyphs the lane does
    hold, copied into the caller's plate. The old answer was the label whole -- the
    run the plate had just said it could not hold, inked past its own edge.

    WHY the walks are pinned: the cut and the trim spend the same lane, and two
    walks disagree about what a glyph costs, which is how a cut landed past the
    buffer the painter was handed.
    """
    faults: list[str] = []

    def block(source: str, start: str, end: str, tag: str) -> str:
        """The slice between two markers, or nothing plus a named fault.

        WHY a fault instead of an exception: a marker that moved is the same news as
        a needle that went -- the gate can no longer see the shape it watches -- and
        a crash hides which one it was.
        """
        at = source.find(start)
        if at < 0:
            faults.append(f"{tag}: `{start}` is gone, so the block this pin reads is not there")
            return ""
        after = source.find(end, at + len(start))
        if after < 0:
            faults.append(f"{tag}: `{end}` is gone, so the block this pin reads has no end")
            return ""
        return source[at:after]

    def needle(block_text: str, text: str, tag: str, why: str) -> None:
        if text not in block_text:
            faults.append(f"{tag}: {why}")

    fitted = block(lib, "pub fn fitted_run", "fn longest_fit(", "lib")
    needle(
        fitted,
        "let len = interos_std::design::font::ellipsize(label, rung, lane, buf);",
        "lib.trim",
        "the fitted run no longer asks the trim whose refusal it answers",
    )
    needle(
        fitted,
        "let fit = longest_fit(label, em, lane, buf.len());",
        "lib.cut",
        "a refused trim is handing the caller the whole label again, so a name wider "
        "than its row prints past the plate that framed it instead of into it",
    )
    needle(
        fitted,
        "let em = interos_std::design::font::em_for_scale(rung);",
        "lib.em",
        "the cut no longer charges the advances of the trim it backs, so the lane is "
        "spent twice at two different widths",
    )
    needle(
        fitted,
        "buf[..fit].copy_from_slice(&label[..fit]);",
        "lib.copy",
        "the cut is read straight back out of a buffer the refusal never wrote, so a "
        "refused trim would ink the plate's stale bytes instead of the label's front",
    )

    # The binary's report rows answered their own refusal the same way, and the
    # walk that cuts them has to hold the mark's three bytes back out of the plate
    # before it spends it -- the bound the platform's own trim was fixed to.
    plain = _code_only(main)
    if "} else { label };" in plain:
        faults.append("main.whole: a card row hands back the whole label when its trim refuses")
    if "} else { value };" in plain:
        faults.append("main.whole: a card row hands back the whole value when its trim refuses")
    needle(
        main,
        "fitted_run(label, layout::ROW_CAPTION_RUNG, max_w, false, &mut lbl_buf);",
        "main.label-call",
        "the row's label stopped going through the fitted run, so its refusal answer "
        "is the label again",
    )
    needle(
        main,
        "fitted_run(value, layout::ROW_BODY_RUNG, max_w, false, &mut val_buf);",
        "main.value-call",
        "the row's value stopped going through the fitted run, so its refusal answer "
        "is the value again",
    )
    trim = block(main, "fn fit_run(", "fn trim_em(", "main.trim")
    needle(
        trim,
        "let fit_len = longest_fit(text, em, budget, out.len() - 3);",
        "main.walk",
        "the trim and the hard cut no longer spend the lane in one walk",
    )
    walk = block(main, "fn longest_fit(", "fn fitted_run<'a>(", "main.walk")
    needle(
        walk,
        "if current_w + adv > max_w || fit_len + 1 > room {",
        "main.room",
        "the walk charges the plate before taking the glyph, so the mark's three "
        "bytes are copied past the end of the buffer the painter was handed",
    )
    cut = block(main, "fn fitted_run<'a>(", "fn fitted_run_w(", "main.cut")
    needle(
        cut,
        "let fit = longest_fit(text, trim_em(scale), max_w, out.len());",
        "main.cut",
        "a refused row trim is handing the painter nothing at all or the label whole "
        "again, instead of the leading glyphs the lane holds",
    )
    needle(
        cut,
        "out[..fit].copy_from_slice(&text[..fit]);",
        "main.copy",
        "the row's cut is read straight back out of a buffer the refusal never wrote",
    )
    return faults


def test_a_refused_trim_is_cut_to_the_lane_and_never_handed_back_whole():
    """A lane too narrow for the three-byte mark makes the trim answer 0.

    The old answer to that refusal was the label whole -- precisely the run the
    container had just said it could not hold, inked past the plate that framed it.
    Both halves of the new answer are pinned: the cut is walked in the same
    arithmetic as the trim, and its bytes are copied into the caller's plate before
    they are read back, because a refusal is the one answer that writes nothing.
    """
    faults = _refused_trim_faults(messages_lib(), messages_main())
    assert not faults, faults


def test_the_refused_trim_contract_can_fail():
    """A gate nobody has seen fail is a comment.

    Each arm puts one half of the old answer back into a copy of the honest source
    and runs the same detector the gate above runs, so the check is proved to fire
    on the defect it exists for -- and an arm that no longer matches its source is
    refused, because a drifted arm would otherwise go quiet.
    """
    lib = messages_lib()
    main = messages_main()
    honest = _refused_trim_faults(lib, main)
    assert not honest, f"the detector fires on the app as it stands: {honest}"

    old_label_row = (
        "let lbl_len =\n"
        "            fit_run(label, layout::ROW_CAPTION_RUNG, max_w, false, &mut lbl_buf);\n"
        "        let lbl_disp = if lbl_len > 0 { &lbl_buf[..lbl_len] } else { label };"
    )
    arms = (
        (
            "hands the caller the whole label back",
            lib,
            "let fit = longest_fit(label, em, lane, buf.len());",
            "return label;",
            "lib.cut",
        ),
        (
            "cuts the plate's front out of the buffer it never wrote",
            lib,
            "buf[..fit].copy_from_slice(&label[..fit]);",
            "",
            "lib.copy",
        ),
        (
            "spends the row's lane in a second walk",
            main,
            "let fit_len = longest_fit(text, em, budget, out.len() - 3);",
            "let mut fit_len = 0;\n"
            "    while fit_len < text.len() && fit_len + 3 < out.len() {\n"
            "        fit_len += 1;\n"
            "    }",
            "main.walk",
        ),
        (
            "walks the plate before taking the glyph",
            main,
            "if current_w + adv > max_w || fit_len + 1 > room {",
            "if current_w + adv > max_w || fit_len + 3 > room + 2 {",
            "main.room",
        ),
        (
            "answers a refused row with the label whole",
            main,
            "let lbl_disp =\n"
            "            fitted_run(label, layout::ROW_CAPTION_RUNG, max_w, false, &mut lbl_buf);",
            old_label_row,
            "main.whole",
        ),
    )
    for label, source, marker, replacement, tag in arms:
        assert source.count(marker) == 1, (
            f"the arm that {label} no longer matches its source once, so it proves nothing"
        )
        arm = source.replace(marker, replacement)
        faults = _refused_trim_faults(arm, main) if source is lib else _refused_trim_faults(lib, arm)
        assert any(f.startswith(f"{tag}:") for f in faults), (
            f"the detector does not catch a source that {label}: {faults}"
        )
