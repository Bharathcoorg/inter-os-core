# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

"""Truth contract for the InterOS vault application.

A vault is the one surface where a claim that the platform did not make
is most dangerous: a "keystore online" banner and a "Encrypted" badge
both tell the user their data is protected when the platform may have
refused every request. Every assertion below pins a specific defect
found by reading the app.
"""

import re
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]

# The SDK functions that used to take a capability slot as their first
# argument. The kernel now authorises every one of them by the
# capability's *type*, so the app names the authority it wants and the
# kernel finds it.
SLOT_ARG_SDK_CALLS = (
    "app_storage_open",
    "display_set_mode",
    "display_set_orientation",
    "fs_list",
    "fs_open",
    "fs_read",
    "fs_stat",
    "fs_unlink",
    "fs_write",
    "get_ticks",
    "ipc_recv",
    "ipc_send",
    "log",
    "package_install",
    "package_list",
    "package_registry",
    "package_rollback",
    "package_uninstall",
    "service_package_launch",
    "service_package_verify",
    "settings_get",
    "settings_set",
    "surface_create",
    "surface_destroy",
    "surface_flip",
    "surface_write",
    "verify_package",
    "window_focus",
    "window_set_geometry",
    "window_set_input",
)

# A capability slot named in a comment or in a user-visible label is not
# a call, so the call patterns are matched against the code with both
# removed.
_COMMENT_OR_LITERAL = re.compile(
    r"//[^\n]*"
    r"|/\*.*?\*/"
    r'|"(?:[^"\\]|\\.)*"'
    r'|(?:b|br|r#*)"(?:[^"\\]|\\.)*"#*',
    re.DOTALL,
)


def code_only(source: str) -> str:
    """Return `source` with comments and string literals removed."""
    return _COMMENT_OR_LITERAL.sub(" ", source)


def slot_arguments(source: str) -> list[str]:
    """Return the SDK calls in `source` that still take a capability slot.

    A slot argument is either a bare number or the name of a resolved
    slot, and a call that names one is a call that pinned a position in
    the task's capability table.
    """
    code = code_only(source)
    return [
        name
        for name in SLOT_ARG_SDK_CALLS
        if re.search(
            r"\b" + name + r"\s*\(\s*(?:\d|[A-Za-z_][A-Za-z0-9_]*_slot\b|\bslot\b)",
            code,
        )
    ]


def read(path: str) -> str:
    return (ROOT / path).read_text(encoding="utf-8")


def vault_main() -> str:
    return read("apps/vault/src/main.rs")


def vault_source() -> str:
    """Everything the vault app is made of."""
    return "\n".join(
        path.read_text(encoding="utf-8")
        for path in sorted((ROOT / "apps/vault/src").rglob("*.rs"))
    )


def test_the_banner_does_not_announce_a_keystore_that_was_never_probed():
    """The startup banner said the keystore was online.

    The banner was written before the app had asked, and the kernel
    refuses to export raw key material to a Ring 3 app, so the claim was
    false on this platform. The banner now says the app is probing, and
    the result is reported after the probe.
    """
    source = vault_main()
    assert "Keystore online" not in source
    assert "probing the platform's storage and keystore" in source
    assert "if self.keys_derived() {" in source
    assert "no key is claimed" in source


def test_the_status_frame_is_not_called_a_backup():
    """The control was labelled "Export backup".

    What it writes is two counters and a checksum, in the clear, to the
    settings store. It is not a copy of the vault and it is not
    encrypted, so the label overstated what the user gets.

    WHY the assertion reads the button's own label rather than quoting a
    sentence: the wording has since been re-written for the person on the
    phone, and a gate that pinned one full sentence failed a rename that kept
    every claim true. What must never come back is the word "backup" on a
    control that copies nothing out of the vault, or a caption that names the
    internal check figure instead of its result.
    """
    source = vault_main()
    assert "Export backup" not in source

    save_buttons = [
        label
        for label in button_labels(source)
        if "save" in label.lower() or "backup" in label.lower()
    ]
    assert save_buttons, "the control that writes the status record is gone"
    for label in save_buttons:
        assert "backup" not in label.lower(), (
            f"the button {label!r} calls a record of two counters a backup"
        )

    # The row the control drives must say the record sits there unprotected.
    # WHY measured as label+caption together, and over a set of equivalent
    # wordings: a gate that named one sentence failed every rename that kept
    # the claim, while a gate that asks only whether *some* caption says it
    # passes when an unrelated row happens to carry the phrase.
    record_rows = [
        caption
        for label, caption in metric_rows(source)
        if "record" in label.lower()
    ]
    assert record_rows, "no metric row names the record this control writes"
    for caption in record_rows:
        assert re.search(
            r"unencrypted|not encrypted|in the clear|plain text|no protection",
            caption.lower(),
        ), (
            f"the saved-record row reads {caption!r}, which never tells the "
            "reader the bytes sit there unprotected"
        )


def test_the_filesystem_capability_is_resolved_by_its_type():
    """A capability slot is a position in a table, not an identity.

    WHY this contract changed: the kernel now authorises every platform
    call by the capability's *type*, and the SDK's functions no longer take
    a slot argument at all, so the app names the authority it wants -- a
    filesystem read -- and the kernel finds it. Pinning a slot number
    pinned an accident: the launcher proved it by passing slot 0 while its
    `FileSystemRead` capability sat at slot 3, so the developer-terminal
    flag it wanted was never read and the refusal reached the user with
    nothing on screen to say why. The old contract ("the read has to
    be resolved by its type") becomes "the app names no slot at all".
    """
    source = vault_source()
    assert slot_arguments(source) == []
    # The app resolves no slot: not through the SDK's slot helpers, and not
    # by looking a capability type up for itself.
    assert "std::cap::" not in source
    assert "std::cap_lookup(" not in source
    # The vault still reads the identity file it is given, and the file it
    # reads is the one it asked for.
    assert "std::fs_open(" in source
    assert "std::fs_read(" in source
    assert '"/system/etc/device_id"' in source


def test_the_display_capability_is_resolved_by_its_type():
    """The framebuffer capability is found by the kernel, not by the app.

    WHY this contract changed: a slot is a position in the task's
    capability table, and the position depends on the order the kernel
    inserted the capabilities in, so the app cannot name one. The kernel
    now authorises the surface by the capability's *type* and the SDK
    takes no slot argument, so there is nothing left for the app to look
    up. Pinning a slot number pinned an accident: the launcher proved it
    by passing slot 0 while its `FileSystemRead` capability sat at slot 3,
    so the developer-terminal flag it wanted was never read.
    """
    source = vault_source()
    assert slot_arguments(source) == []
    assert "std::cap::" not in source
    assert "std::cap_lookup(" not in source
    # The vault still opens a surface, presents it and tears it down.
    assert "std::surface_create(" in source
    assert "std::surface_flip(" in source
    assert "std::surface_destroy(" in source


def test_the_keystore_slot_is_not_confused_with_a_capability_slot():
    """The keystore argument is a keystore slot, not a capability slot."""
    source = vault_main()
    assert "pub const ROOT_KEY_SLOT: u32 = 0;" in source
    assert "pub const STORAGE_KEY_SLOT: u32 = 1;" in source
    assert "std::keystore_derive(ROOT_KEY_SLOT," in source
    assert "std::keystore_derive(STORAGE_KEY_SLOT," in source


def test_the_home_gesture_only_leaves_the_app_from_the_home_strip():
    """A pan or a fling from anywhere returned to the launcher.

    WHY this contract changed: the app answered a hand-written strip,
    `input.y >= 1400 && input.x >= 240 && input.x <= 480` -- 241 columns
    by 40 rows, off-centre about column 360.5 and under the platform's
    44-row touch floor, so a thumb one row clear of the box did nothing
    and read as a hung device. The app now answers the SDK's home-band
    declaration: `home_band::HIT` is 240 columns centred on a 720-wide
    panel (x 240..=479), 48 rows -- the preferred touch target -- from
    row 1392 to the panel's last row 1439, and it covers the 200x6 pill
    ink at (260, 1418) that the app now paints through
    `home_band::pixel`. The gate pins the reference to that declaration
    and the declaration's own numbers in the SDK source; it is not
    weakened -- the old literals may not come back.
    """
    source = vault_main()
    assert (
        "let is_home_gesture = std::design::home_band::contains("
        "input.x as usize, input.y as usize);" in source
    )
    assert "if is_home_gesture" in source
    # The hand-written 241-by-40 strip may not return.
    assert "input.y >= 1400" not in source
    assert "input.x <= 480" not in source
    # The mark and the tap answer are the SDK's, not three app-local copies.
    assert "std::design::home_band::pixel(x, y, THEME.on_surface_variant)" in source
    assert "const GESTURE_MARK_X0: usize = 260;" not in source
    assert "const GESTURE_BAND_H: usize = 40;" not in source

    sdk = read("sdk/rust/interos-std/src/design/mod.rs")
    # The answering box the app points at, as the SDK declares it.
    assert "const HIT_W: usize = 240;" in sdk
    assert "const HIT_H: usize = super::touch::PREFERRED_TARGET;" in sdk
    assert "pub const PREFERRED_TARGET: usize = 48;" in sdk
    assert "pub const MIN_TARGET: usize = 44;" in sdk
    # The pill the box must cover, and the panel both are centred on.
    assert "pub const PILL_W: usize = 200;" in sdk
    assert "pub const PILL_H: usize = 6;" in sdk
    assert "pub const LOGICAL_WIDTH: usize = 720;" in sdk
    assert "pub const LOGICAL_HEIGHT: usize = 1440;" in sdk
    assert "super::viewport::LOGICAL_HEIGHT - 22," in sdk  # the pill's row: 1418


def test_no_lock_badge_appears_without_a_platform_confirmation():
    """A lock icon or an "Encrypted" badge must not be drawn unless the
    platform confirmed the item's state.

    The only mark the app draws is the round-trip verdict, and the code
    sets it only after an encrypt, a decrypt and a comparison all
    succeeded.

    WHY the passing word is read off the tile's own match and not quoted
    here: the wording belongs to the screen, not to this file. What is pinned
    is that the tiles have exactly one passing state, that it is the arm the
    round-trip flag sets, and that no fabricated lock badge has been painted
    beside it.
    """
    source = vault_main()
    for badge in ('b"Encrypted"', 'b"Locked"', 'b"Unlocked"', 'b"Sealed"'):
        assert badge not in source

    passing = [
        word
        for pattern, word, line in tile_verdict_arms(source)
        if pattern == "Some(true)"
    ]
    assert len(passing) == 1, f"the tile's passing state is not one word: {passing}"
    assert "success" in str(tile_verdict_arms(source)[0][2]).lower(), (
        "the passing tile verdict no longer wears the success ink"
    )


# ---------------------------------------------------------------------------
# The screen in a person's words
#
# WHY this section exists: the live-device review of rv_app_vault.png found a
# Vault screen whose copy was written to the next engineer rather than to the
# reader -- "unlocks every other secret; asked of the keystore", "not probed",
# "16 bytes read", "all zero bytes", "sector round-trip FAILED (see log)". The
# three things a reader came for are what is kept here, what was checked, and
# what could not be checked. None of those needs a syscall name, a device
# node, a privilege ring, a byte count or a pointer at a serial log.
#
# WHY the gate measures the app's bytes instead of quoting sentences: an
# expected sentence pasted into a test is wording the app is then forced to
# keep, so the next plain-words pass fails for renames that kept every claim
# true. Every assertion below reads the strings off the real paint sites --
# the same discipline tests/test_settings_truth_contract.py's row_head_claims()
# gives the Settings rows -- and then asserts properties of what it found.
# ---------------------------------------------------------------------------

BYTE_LITERAL = re.compile(r'b"([^"\\]*)"')
PLAIN_LITERAL = re.compile(r'"([^"\\]*)"')

#: Byte literals that reach storage or the keypad instead of the glass: the
#: marker the identity file is compared against, the PIN the unlock path
#: accepts, the digits and edit glyphs printed on the keypad discs.
NON_GLASS_LITERALS = frozenset(
    {"DID:", "1234", "0", "1", "2", "3", "4", "5", "6", "7", "8", "9",
     "CLR", "DEL", "0123456789abcdef"}
)

#: Payload tags the app seals into a block. They name what is written, not
#: what is shown, so they are not the reader's copy.
GLASS_TAG_PREFIX = "INTER_"

#: Functions that build or read bytes and never paint a sentence. A gate over
#: screen copy must not read their innards.
NON_GLASS_FNS = (
    "fn hex_encode",
    "fn hex_nibble",
    "fn hex_decode",
    "fn fletcher32",
    "fn write_u64",
    "fn write_i64",
    "fn font_copy",
)

#: Machine vocabulary: pattern, and why a reader of this screen does not need
#: it. Matched case-insensitively except for the acronyms, which are their own
#: patterns below.
MACHINE_VOCABULARY = {
    r"keystore": "it names the service instead of what the service did",
    r"\ring\s*3\b": "a privilege ring the reader has no part in",
    r"/dev/": "a device node path",
    r"/system/": "a file path on the device",
    r"\bsyscalls?\b": "the mechanism that fetched the answer",
    r"\bbytes?\b": "a developer's measurement",
    r"round-?trips?\b": "the app's own name for its self-test",
    r"\bprob(?:e|es|ed|ing)\b": "asking the platform is the mechanism, not the answer",
    r"\bcrypt\w*\b": "a cipher this app cannot demonstrate",
    r"\bchecksums?\b": "an internal check figure",
    r"\bfletcher\b": "an internal check figure",
    r"\bentrop\w*\b": "the technical name of the randomness source",
    r"\bslots?\b": "an index in the platform's table",
    r"\bbuffers?\b": "an allocation the reader has no use for",
    r"\bmagic\b": "a file's internal marker",
    r"\bderiv\w*\b": "the key call, not its consequence",
    r"\bexport\w*\b": "nothing leaves the device",
    r"\bcounters?\b": "a developer's word for a tally",
    r"\bhex\b": "an encoding",
    r"\battestation\b": "a guarantee the app never asked for",
    r"\blog\b": "a serial log the reader cannot open from here",
    r"\bnot tested\b": "the engineer's note, not the reader's answer",
    r"\bread-back\b": "an internal round trip over a buffer",
}

#: Acronyms and shout-words, matched with their own case kept: "did not match"
#: is plain English, "DID:" is a file marker, and FAILED is a shout where a
#: sentence belongs.
CASED_MACHINE_VOCABULARY = {
    r"\bIPC\b": "an internal acronym",
    r"\bIV\b": "an initialisation vector the reader never sees",
    r"\bDID:": "the identity file's internal marker",
    r"\b0x[0-9a-fA-F]": "a hex dump",
    r"\b(?:FAILED|ERROR|DENIED)\b": "a shout standing in for a sentence",
}

#: A state a check could not reach must keep saying so, in these words.
NOT_CHECKED_OPENING = ("not", "never", "nothing")

#: ... and must never be dressed as a result.
PASS_WORDS = frozenset(
    {"ok", "okay", "fine", "good", "done", "passed", "verified", "yes",
     "available", "working", "ready", "success", "active"}
)

METRIC_CALL = "metric_row("
BUTTON_CALL = "button_pixel("
CAPTION_CALL = "caption_pixel("
INK_CALLS = ("font::ink(", "font::ink_center(")
STATUS_CALL = "status("
TILE_VERDICT_ANCHOR = "match vault.sectors[index].map(|s| s.authenticated)"
BACKUP_VERDICT_ANCHOR = "match vault.backup {"


def strip_comments(source: str) -> str:
    """`source` with comments gone and every literal left exactly as written.

    WHY it is hand-rolled rather than a regex: the vault's paint calls carry
    their WHY comments inside their own argument lists, so a regex that ate
    comments and one that ate strings disagree about where a literal ends.
    """
    out = []
    i, n = 0, len(source)
    while i < n:
        if source.startswith("//", i):
            j = source.find("\n", i)
            i = n if j == -1 else j
        elif source.startswith("/*", i):
            j = source.find("*/", i)
            i = n if j == -1 else j + 2
        elif source[i] == '"':
            j = i + 1
            while j < n and source[j] != '"':
                j += 2 if source[j] == "\\" else 1
            j = min(j + 1, n)
            out.append(source[i:j])
            i = j
        elif source[i] == "'":
            head = source[i:i + 4]
            if len(head) >= 4 and head[1] == "\\" and head[3] == "'":
                out.append(head)
                i += 4
            elif len(head) >= 3 and head[2] == "'":
                out.append(head[:3])
                i += 3
            else:
                out.append(source[i])
                i += 1
        else:
            out.append(source[i])
            i += 1
    return "".join(out)


def balanced(source: str, open_at: int, opening: str = "(", closing: str = ")") -> str:
    """The text inside the brackets that open at index `open_at`.

    WHY an index and not a snippet of the source: this took a string and re-found
    it with `source.index`, so every call site resolved to the first place that
    snippet appears -- and `back_button_pixel(`, the home-gesture chip that came
    with the nav band, ends with `button_pixel(`. The scan for button labels
    landed on the chip's arguments and reported no labels at all, which is a
    silent loss of coverage rather than a failure.
    """
    depth = 0
    for i in range(open_at, len(source)):
        if source[i] == opening:
            depth += 1
        elif source[i] == closing:
            depth -= 1
            if depth == 0:
                return source[open_at + 1:i]
    raise AssertionError(f"the bracket at {open_at} never closes")


def body_after(source: str, anchor: str) -> str:
    """The `{...}` block that follows the first `{` after `anchor`."""
    start = source.index("{", source.index(anchor))
    depth = 0
    for i in range(start, len(source)):
        if source[i] == "{":
            depth += 1
        elif source[i] == "}":
            depth -= 1
            if depth == 0:
                return source[start:i + 1]
    raise AssertionError(f"the block after {anchor!r} never closes")


def vault_code() -> str:
    """The vault painter with comments and the byte-shuffling helpers removed."""
    code = strip_comments(vault_main())
    for name in NON_GLASS_FNS:
        if name in code:
            cut = body_after(code, name)
            code = code.replace(cut, " " * len(cut))
    return code


def painted_literals() -> list:
    """Every string this app can put on the glass.

    WHY the counter word is added by hand: it is built from two fragments at
    run time and seeded with a `\\0`-padded byte array, so a single-literal
    scan sees neither piece on its own -- which is exactly how "0 enc, 0 dec"
    could have stayed on the screen behind a green light.
    """
    code = vault_code()
    found = [
        text
        for text in BYTE_LITERAL.findall(code)
        if text not in NON_GLASS_LITERALS and not text.startswith(GLASS_TAG_PREFIX)
    ]
    seed = re.search(r'counter_buf: \*b"([^"\\]*)', code)
    if seed:
        found.append(seed.group(1))
    return found


def literals_per_call(source: str, call: str) -> list:
    """The byte literals of each separate call site, in the order written.

    WHY the name is matched whole: `button_pixel(` is also the tail of
    `back_button_pixel(`, and a scan that lets a tail match answer for the
    control it merely resembles has measured a chip that paints no label.
    """
    groups = []
    for match in re.finditer(rf"(?<![A-Za-z0-9_]){re.escape(call)}", source):
        groups.append(BYTE_LITERAL.findall(balanced(source, match.end() - 1)))
    return groups


def literals_assigned(source: str, name: str) -> list:
    """The byte literal on the right of each `let name = ` binding, in order written.

    WHY separate from `literals_per_call`: the tally fragments are bindings, not
    arguments, so there is no bracketed call to measure -- and a scan that treated
    `let mid` as a call name would read whatever paren came next, which in this
    file belongs to a different statement entirely.

    WHY `let` is required: `end` is also a method call and a local elsewhere, and
    a scan that reads any `end =` answers for a sentence the app never paints.
    """
    out = []
    for match in re.finditer(rf"(?<![A-Za-z0-9_])let\s+(?:mut\s+)?{re.escape(name)}\s*=\s*", source):
        literal = BYTE_LITERAL.match(source, match.end())
        if literal:
            out.append([literal.group(1)])
    return out


def metric_rows(source: str) -> list:
    """`(label, caption)` for every metric row the painter lays out."""
    rows = []
    for label_group in literals_per_call(source, METRIC_CALL):
        if len(label_group) >= 2:
            rows.append((label_group[0], label_group[-1]))
    return rows


def metric_captions(source: str) -> list:
    return [caption for _label, caption in metric_rows(source)]


def button_labels(source: str) -> list:
    """The labels of every control the app paints as a button."""
    return [
        group[0]
        for group in literals_per_call(source, BUTTON_CALL)
        if group
    ]


def card_captions(source: str) -> list:
    """The captions that head each card."""
    return [
        group[0]
        for group in literals_per_call(source, CAPTION_CALL)
        if group
    ]


def status_lines(source: str) -> list:
    """What the app tells the reader has just happened."""
    return [
        group[0]
        for group in literals_per_call(source, STATUS_CALL)
        if group
    ]


ARM = re.compile(r"([A-Za-z][\w:]*(?:\([^)]*\))?)\s*=>\s*\(?\s*b\"([^\"\\]*)\"")


def arms_of(block: str) -> list:
    """`(state, word, line)` for each match arm that paints a verdict word."""
    out = []
    for match in ARM.finditer(block):
        line_end = block.find("\n", match.start())
        line_start = block.rfind("\n", 0, match.start()) + 1
        out.append((match.group(1), match.group(2), block[line_start:line_end]))
    return out


def verdict_arms(source: str, anchor: str) -> list:
    """The arms of one verdict decision, as the painter wrote them."""
    return arms_of(body_after(source, anchor))


def tile_verdict_arms(source: str) -> list:
    code = strip_comments(source)
    return arms_of(body_after(code, TILE_VERDICT_ANCHOR))


def lib_glass_strings() -> list:
    """The words the library hands the painter: the key row and the tile hint.

    WHY the library is read at all: two rows take their verdict and their
    provenance from `key_material_row`, so a caption ban that stopped at
    main.rs would let the mechanism come back through the door next door. The
    library's own tests are cut off first -- they quote the screen to measure
    it and are not the screen.
    """
    source = read("apps/vault/src/lib.rs").split("#[cfg(test)]")[0]
    out = []
    for anchor in ("pub fn key_material_row", "pub fn sector_tile_hint"):
        body = body_after(source, anchor)
        out.extend(PLAIN_LITERAL.findall(body))
        out.extend(BYTE_LITERAL.findall(body))
    return out


# ---------------------------------------------------------------------------
# 1. No painted string may require the reader to know the platform's internals
# ---------------------------------------------------------------------------


def test_no_painted_string_leaves_the_reader_in_the_mechanism():
    """The screen is read by a person, not by whoever debugs the next build."""
    measured = painted_literals() + lib_glass_strings()
    assert measured, "the scan found no painted copy at all, so it measures nothing"

    for literal in measured:
        for pattern, reason in MACHINE_VOCABULARY.items():
            assert not re.search(pattern, literal, re.IGNORECASE), (
                f"the screen paints {literal!r}, which still says "
                f"{pattern!r} -- {reason}"
            )
        for pattern, reason in CASED_MACHINE_VOCABULARY.items():
            assert not re.search(pattern, literal), (
                f"the screen paints {literal!r}, which still says "
                f"{pattern!r} -- {reason}"
            )


def test_the_copy_gate_measures_the_real_paint_sites():
    """A gate that quietly stopped finding call sites would pass on anything.

    WHY the floors are this blunt: every property above is a property of what
    the scan collected, so the scan itself has to be proved to be looking at
    the whole screen -- both cards, the tiles, the buttons, the status line and
    the verdict helpers.
    """
    code = vault_code()
    measured = painted_literals()
    assert len(measured) >= 55, (
        f"only {len(measured)} painted strings were measured; a screen this "
        "dense has rows, tiles, buttons, captions and a status line"
    )
    assert len(metric_rows(code)) >= 10, "the metric rows escaped the gate"
    assert len(button_labels(code)) >= 4, "the controls escaped the gate"
    assert len(card_captions(code)) >= 5, "the card headings escaped the gate"
    assert len(status_lines(code)) >= 12, "the status line escaped the gate"
    for anchor in ("fn key_verdict", "fn entropy_verdict", "fn identity_verdict"):
        arms = verdict_arms(code, anchor)
        assert len(arms) >= 4, f"{anchor} no longer paints one word per state"
    assert len(tile_verdict_arms(code)) == 3, "the tiles no longer have three states"
    assert len(verdict_arms(code, BACKUP_VERDICT_ANCHOR)) == 6, (
        "the saved-record row no longer reports all six outcomes"
    )


# ---------------------------------------------------------------------------
# 2. A check that could not run must still read as a check that did not run
# ---------------------------------------------------------------------------


def test_a_check_that_could_not_be_made_still_says_so_in_plain_words():
    """Renaming "not probed" to a person's words must not promote the state.

    The never-run state is the one a copy pass can destroy by accident: "not
    probed" and "OK" are both two letters' worth of ink in the same plate. So
    this pins the shape of the answer rather than its spelling -- the state the
    platform was never asked about opens with a plain negation, never reads as
    a result, never wears the ink a result wears, and never shares its word
    with the state beside it.
    """
    code = vault_code()
    never_run_states = [
        ("fn key_verdict", "KeyProbe::NotRun"),
        ("fn entropy_verdict", "EntropyProbe::NotRun"),
        ("fn identity_verdict", "IdentityState::NotChecked"),
        (BACKUP_VERDICT_ANCHOR, "BackupState::NotAttempted"),
        (TILE_VERDICT_ANCHOR, "Some(false)"),
    ]
    for anchor, state in never_run_states:
        arms = verdict_arms(code, anchor)
        words = {pattern: (word, line) for pattern, word, line in arms}
        assert state in words, f"{anchor} no longer paints a word for {state}"
        word, line = words[state]
        lowered = word.strip().lower()
        assert lowered.startswith(NOT_CHECKED_OPENING), (
            f"the {state} state reads {word!r}; a check that was never made "
            "has to open by saying it was not made"
        )
        assert lowered not in PASS_WORDS, (
            f"the {state} state reads {word!r}, which is a result word"
        )
        assert "THEME.success" not in line, (
            f"the {state} state wears success ink: {line.strip()}"
        )
        others = [other for pattern, (other, _l) in words.items() if pattern != state]
        assert word not in others, (
            f"{state} shares its word {word!r} with a state that was reached"
        )


def test_the_screen_never_points_at_a_log_the_reader_cannot_open():
    """The failure lines used to end in "(see log)".

    WHY they may not: the serial log is not on this screen, and a pointer at
    it is not an answer. A failure either says in words what did not happen or
    says nothing -- it may not send the reader somewhere they cannot go.
    """
    for literal in painted_literals() + lib_glass_strings():
        for pointer in ("see log", "see above", "serial", "the log"):
            assert pointer not in literal.lower(), (
                f"the screen paints {literal!r} and points at {pointer!r}, "
                "which the reader cannot open from here"
            )

    failures = [
        line
        for line in status_lines(vault_code())
        if "not" in line.lower() or "fail" in line.lower()
    ]
    assert failures, "the app no longer reports a failure on the status line"
    for line in failures:
        assert len(line) >= 20, (
            f"{line!r} shouts a result word where it owes the reader a "
            "sentence about what did not happen"
        )


# ---------------------------------------------------------------------------
# 3. A developer's measurement is not an answer
# ---------------------------------------------------------------------------


def test_the_randomness_row_answers_the_question_it_was_asked():
    """The entropy row printed "16 bytes read" and "all zero bytes".

    Those are the app's own measurements. The reader's question is whether the
    device can produce real randomness at all, because without it nothing the
    vault seals is worth anything -- so the row states the reading's meaning
    and the plate it sits on states what it is for.

    WHY the count row is here too: the tally of the app's own operations was
    painted as "N enc, M dec", which is an abbreviation, not an answer.
    """
    code = vault_code()
    entropy = {pattern: word for pattern, word, _line in verdict_arms(code, "fn entropy_verdict")}
    assert entropy, "the randomness row has no verdict words to read"
    for state, word in entropy.items():
        assert not re.search(r"\d", word), (
            f"the {state} state still reports a measurement ({word!r}) "
            "instead of what it means"
        )

    counter_fragments = literals_assigned(code, "mid") + literals_assigned(code, "end")
    assert counter_fragments, "the tally sentence has no fragments left to read"
    tally_words = [word for group in counter_fragments for word in group]
    for word in tally_words:
        assert not re.search(r"\b(?:enc|dec)\b", word), (
            f"the tally still reads {word!r}, which is an abbreviation of the "
            "app's own operation names"
        )

