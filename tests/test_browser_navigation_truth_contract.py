# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

"""Contract tests for the browser's navigation and address-row chrome.

WHY these rules exist as their own file: the browser's screen carried two
complete navigation sets, a window-close mark inside the field a person types
an address into, and the tab count twice -- one copy of each defect was pinned
by `tests/test_native_ui_truth_contract.py`, which is why the duplicates were
never edited out. These gates name the model the app is allowed to have:

* one navigation set, in the bottom band, which is where this device puts
  back/forward/recents -- `apps/launcher/src/nav.rs` splits its bottom band into
  Back, Home and Recents thirds, and `design::home_band` owns the pill under
  every app;
* one way out of the app, enumerated at every real exit site rather than
  quoted from a sentence: a tap on the painted home box, and only the
  shell's Home gesture (kernel kind 7) answered inside that same box.
  Kinds 4 (Pan) and 10 (Fling) are the scroll gesture, kind 6 is Back --
  the bar's own painted tile answers it -- and kind 8 is the shell's
  Recents; none of those may quit the browser, because none of them is
  painted here as a way out;
* one tab count.

WHY a source gate and not a host test: `apps/browser/Cargo.toml` declares the
bin with `test = false`, so neither `toolbar_pixel` nor `handle_tap` can be
called from a Rust test. The geometry the gates below depend on *is* host-tested
-- it lives in `apps/browser/src/chrome.rs`, whose boxes these assertions
require the renderer and the press path to name.
"""

from pathlib import Path

import re

ROOT = Path(__file__).resolve().parents[1]

# The kernel's normalized input kinds, as `InputEventKind` in
# `kernel/src/drivers/input.rs` writes them onto the wire the SDK reads.
# The exit gate enumerates guards against this table, so a bare
# `input.kind == N => exit` is named by what N actually means.
GESTURE_KINDS = {
    "1": "Tap",
    "2": "DoubleTap",
    "3": "LongPress",
    "4": "Pan (the drag a person uses to scroll)",
    "5": "Pinch",
    "6": "Back (the shell's edge gesture)",
    "7": "Home (the shell's gesture, painted by design::home_band)",
    "8": "Recents (the shell's own surface)",
    "9": "Cancel",
    "10": "Fling (the release of a fast scroll)",
}


def read(path: str) -> str:
    return (ROOT / path).read_text(encoding="utf-8")


def _code_only(source: str) -> str:
    """Strips comments so a rule is asserted against the program.

    WHY: the comments in this app quote, at length, the very literals these
    rules forbid -- a chip's rectangle, a close mark, a second tab badge.
    """
    without_block = re.sub(r"/\*.*?\*/", " ", source, flags=re.S)
    return re.sub(r"//[^\n]*", " ", without_block)


def _rust_fn(source: str, name: str) -> str:
    """The text of one Rust function, found by its `fn NAME(` and brace matching."""
    start = source.index(f"fn {name}(")
    open_brace = source.index("{", start)
    depth = 0
    for index in range(open_brace, len(source)):
        if source[index] == "{":
            depth += 1
        elif source[index] == "}":
            depth -= 1
            if depth == 0:
                return source[start:index + 1]
    raise AssertionError(f"`fn {name}` never closes")


def _block_headers(code: str, start: int) -> list:
    """The header of every block enclosing offset `start`, innermost first.

    WHY walk instead of quote: a gate that matches one exit sentence measures
    the sentence. Walking the blocks that actually enclose a call measures
    the program -- a second, newly invented exit path is caught even when
    its prose, spacing or shape differ from the one it copies.

    WHY closed siblings are paired, not captured: scanning backwards, a `}`
    left of the call closes some block. If a matched `{` is found for it,
    that pair is a completed sibling statement inside our own enclosing
    block and carries no guard for us; only a `{` seen at depth zero opens a
    block that actually encloses the call. The first version of this walk
    stepped over the `;{}` stop character after a captured header, which
    let the guards of earlier sibling `if ... { }` statements above a later
    one leak into its chain.
    """
    headers = []
    i = start
    depth = 0
    while i >= 0:
        ch = code[i]
        if ch == "}":
            depth += 1
        elif ch == "{":
            if depth > 0:
                depth -= 1
            else:
                j = i - 1
                while j >= 0 and code[j] not in "};{":
                    j -= 1
                header = code[j + 1:i].strip()
                if not header:
                    break
                headers.append(header)
                if header.startswith("fn "):
                    break
                i = j
                continue  # let the loop process the stop character itself
        i -= 1
    return headers


def _sites(code: str, call: str) -> list:
    """Every real call site of `call` (the definition excluded), as header chains."""
    found = []
    for match in re.finditer(re.escape(call), code):
        if code[:match.start()].rstrip().endswith("fn"):
            continue
        found.append(_block_headers(code, match.start()))
    return found


BROWSER_MAIN = "apps/browser/src/main.rs"


def test_the_browser_carries_one_navigation_set_and_it_is_the_bottom_row():
    """A back chevron in the header and a back/forward row under the page.

    WHY this matters: the person on the device reported two competing
    navigation models on one screen. Both worked, which is worse than one of
    them being dead -- the header chip and the bar's left tile answered the same
    request from two places, and only the bar said whether there was anywhere to
    go back to.

    The shell's convention settles which one stays: `apps/launcher/src/nav.rs`
    puts Back in the bottom band, and every app's home affordance is
    `design::home_band` at the panel's foot. The browser's navigation belongs on
    that bar, so the header chip stops being painted and stops being hit-tested.
    The bar's tiles are enumerated, not quoted: every rectangle the bar inks
    must stand on the derived row and be answered by the same name, so
    painted == hit stays structural for tiles added later too.
    """
    code = _code_only(read(BROWSER_MAIN))

    # The header carries no second back affordance -- neither ink nor tap.
    assert "back_button::pixel_for_theme" not in code, (
        "the header still paints the platform's Back chip beside the browser's "
        "own back/forward row"
    )
    assert "back_button::contains(" not in code, (
        "the press path still answers the header's Back chip"
    )

    # Enumerate the tiles the bar actually paints, and require each one to
    # be pressed by the same rect it was inked with, derived from the row.
    bar = _rust_fn(code, "bottom_bar_pixel")
    painted = re.findall(r"chrome_[a-z_]+\([ ]*x, y, ([A-Z][A-Z_]*)", bar)
    assert {"BACK_BTN", "FWD_BTN"} <= set(painted), (
        "the bottom row no longer paints the back/forward pair it is for"
    )
    for name in painted:
        assert f"if {name}.contains(x, y)" in code, (
            f"{name} is inked on the bottom bar but no press answers the "
            "rectangle it was painted in"
        )
        decl = re.search(rf"const {name}: Rect = Rect::new\(([^)]*)\)", code)
        assert decl, f"{name} is painted without an authored rectangle to derive from"
        assert "NAV_ROW_Y" in decl.group(1) and "NAV_BTN_H" in decl.group(1), (
            f"{name} no longer stands on the row the bar derives -- a plate "
            "with a remembered y answers rows the bar never inks"
        )

    # The SDK chip is still the *measure* the bar is built from -- its size
    # is what `NAV_BTN_H` reads -- so removing the affordance must not have
    # removed the derivation.
    assert "const NAV_BTN_H: usize = back_button::SIZE;" in code


def test_the_address_row_holds_no_window_close_control():
    """A `✕` in the field a person types an address into reads as "close this page".

    WHY this matters: the omnibox row wore a close tile whose whole job was to
    quit the app -- a second, louder copy of the shell's home gesture, sitting
    where a browser puts go/reload. The row already has a reload control, and the
    keyboard's accessory strip already has a Clear chip for the field's contents
    (`chrome::ACC_CLEAR`), so the mark had no sane job left in this row.
    """
    code = _code_only(read(BROWSER_MAIN))

    assert "CLOSE_BTN" not in code, "the address row still carries a close control"
    toolbar = _rust_fn(code, "toolbar_pixel")
    assert "monochrome::Icon::Close" not in toolbar, (
        "the address row still inks a close mark"
    )

    # The field's clear affordance stays where it belongs: the accessory strip,
    # beside the keyboard, and only while the field is up.
    assert "chrome::ACC_CLEAR.contains(x, y)" in code
    clear = _rust_fn(code, "toolbar_pixel")
    assert "ACC_CLEAR" not in clear, (
        "a clear control moved into the address row, where it is not the field's"
    )


def test_the_home_band_is_the_only_way_out_of_the_app():
    """Every exit path is enumerated and matched to a painted affordance.

    WHY this matters: the header chip and the close tile quit the app, and
    the event loop quit it on `input.kind == 4 || 6 || 7 || 8 || 10` from
    outside `handle_tap`, with no position check -- so the very event a
    person produces to scroll the page (Pan, kind 4, and its release Fling,
    kind 10) threw them out of the browser. Kind 6 is the shell's Back and
    kind 8 its Recents; the only kind that corresponds to something this
    screen paints as a way out is kind 7, Home, answered inside `HOME_HIT`
    -- the box around the home pill the app inks through `design::home_band`
    and answers to a tap.

    This gate does not quote a sentence. It walks the blocks enclosing every
    `destroy_and_exit(` call and every `return Tap::Leave`, so a second,
    secret exit path fails the gate whether or not the first one still
    passes.
    """
    code = _code_only(read(BROWSER_MAIN))

    # 1. The press path: exactly one leave, and it is the home box's.
    body = _rust_fn(code, "handle_tap")
    leaves = _sites(body, "return Tap::Leave")
    assert len(leaves) == 1, "the press path leaves the app from more than one control"
    assert "HOME_HIT.contains" in " ".join(leaves[0]), (
        "the one press exit is not answered inside the painted home box"
    )

    # 2. The event loop: every destroy_and_exit call is either the arm that
    # relays that press exit, or a guard naming kind 7 only, inside HOME_HIT.
    exits = _sites(code, "destroy_and_exit(")
    assert exits, "no exit path was found at all -- check the walk before trusting the gate"
    for chain in exits:
        joined = " ".join(chain)
        if "Tap::Leave" in joined:
            continue  # the sanctioned relay of the home-box press, checked above
        kinds = set(re.findall(r"input\.kind\s*==\s*(\d+)", joined))
        assert kinds == {"7"}, (
            "the event loop exits on gesture kind(s) "
            + ", ".join(sorted(kinds, key=int))
            + " ("
            + "; ".join(GESTURE_KINDS.get(k, "unknown") for k in sorted(kinds, key=int))
            + "); only kind 7 (Home) corresponds to the pill the screen "
              "paints, and a Pan/Fling exit quits the app on a page scroll"
        )
        assert "HOME_HIT.contains" in joined, (
            "the Home exit fires at any position, so a near-miss anywhere on "
            "the panel quits the app -- it must answer the painted home box"
        )


def test_scroll_gestures_scroll_and_never_quit():
    """The drag/scroll branch must consume the gestures the exit block ate.

    WHY this matters: with the exit removed from kinds 4 and 10, a swipe
    that did nothing would be as wrong as a swipe that quit -- the reader
    comment in the event loop states the platform's scroll model, so the
    scroll condition itself has to name the Pan and Fling kinds the kernel
    emits for a page swipe.
    """
    code = _code_only(read(BROWSER_MAIN))

    guards = re.findall(
        r"if ([^\n{]*?)\s*&&\s*browser\.status == PageStatus::Loaded", code
    )
    scroll_guards = [g for g in guards if "input.kind" in g]
    assert scroll_guards, "the drag/scroll branch is gone -- swipes no longer scroll the page"
    joined = " ".join(scroll_guards)
    for kind in ("4", "10"):
        assert f"input.kind == {kind}" in joined, (
            f"the scroll branch does not consume kind {kind} "
            f"({GESTURE_KINDS[kind]}), so a page swipe has no effect at all"
        )


def test_the_home_exit_pill_is_painted_where_it_is_answered():
    """An exit is only honest if the affordance it points at is on the screen.

    WHY: `HOME_HIT` answers the gesture and `design::home_band::pixel` inks
    the pill at the panel's foot. If one of the two disappears, the other is
    a lie -- an invisible way out, or a mark that quits nothing.
    """
    code = _code_only(read(BROWSER_MAIN))

    assert re.search(r"const HOME_HIT: Rect = home_band::hit_from\(", code), (
        "the answering box is no longer derived from design::home_band"
    )
    painter = _rust_fn(code, "get_browser_pixel_content")
    assert "home_band::pixel(" in painter, (
        "the band that answers the exit no longer inks the home pill"
    )
    body = _rust_fn(code, "handle_tap")
    assert "HOME_HIT.contains(x, y)" in body


def test_the_tab_count_is_painted_once():
    """The same count, on two tiles, in two rows, opening one sheet.

    WHY this matters: the header's tab tile and the bar's tab tile both toggled
    `SheetOverlay::TabSwitcher` and both inked `app.tabs.tab_count()`, so the
    screen reported one fact twice and a person could not tell whether the two
    numbers were the same number.
    """
    code = _code_only(read(BROWSER_MAIN))

    assert "TABS_TOP_BTN" not in code, (
        "the header still carries a tab-count tile beside the bar's"
    )
    assert "const TABS_BTN: Rect" in code, "the bar lost its tab-count tile"

    # One badge painter call, and one read of the count for painting.
    assert code.count("tab_badge_box(") == 2, (
        "the tab badge is inked into more than one tile"
    )
    assert code.count("app.tabs.tab_count()") == 1, (
        "the tab count is read for painting more than once"
    )


def test_the_address_row_is_derived_from_the_platform_tokens():
    """No control in the row may be a rectangle somebody remembered.

    WHY this matters: the row's four tiles were `Rect::new(486, 60, 44, 44)`,
    `Rect::new(596, 60, 44, 44)` and `Rect::new(650, 60, 44, 44)`, and the field
    was `476 - (back_button::X + back_button::SIZE + spacing::GRID_16)` wide --
    every one of them an edge of the row owned by a literal, in the file that
    paints it, beside the file that answers it. Removing two controls from a row
    built that way leaves two gaps nobody can explain.

    The row is now `chrome`'s: the band's rows come from the platform's status
    bar and the touch floor, the field's height from the rung its address is set
    at, and the tiles from the control floor, right-aligned to the gutter the
    rest of the app keeps. `apps/browser/src/chrome.rs` carries the gates that
    prove the ink fits the plate and the plate fits the band.
    """
    code = _code_only(read(BROWSER_MAIN))

    for authored in (
        "Rect::new(486, 60, 44, 44)",
        "Rect::new(540, 60, 46, 44)",
        "Rect::new(596, 60, 44, 44)",
        "Rect::new(650, 60, 44, 44)",
    ):
        assert authored not in code, f"the address row still authors a plate: {authored}"

    assert "chrome::OMNIBOX_FIELD" in code, "the address field is not chrome's box"
    assert "chrome::omnibox_tile(" in code, "the row's tiles are not chrome's boxes"

    # The field's run must be inside the field that carries it, at the rung the
    # painter actually asks for -- the clipped-descender rule.
    painter = _rust_fn(code, "toolbar_pixel")
    assert "chrome::FIELD_TEXT_Y" in painter, (
        "the address is inked at a row the field does not derive"
    )
    assert "font::line_height(1)" not in painter, (
        "the renderer reserves a line box the ladder does not name"
    )
