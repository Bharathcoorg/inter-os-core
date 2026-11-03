# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

"""Contract tests for the browser's start page and address field.

These rules exist because the start page and the omnibox are the two
places where the browser is most likely to say something it does not
know. Each test names the claim that was being made and what the screen
is allowed to say instead.
"""

from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]


def read(path: str) -> str:
    return (ROOT / path).read_text(encoding="utf-8")


def _code_only(source: str) -> str:
    """Strips comments so a rule is asserted against the program.

    WHY: a contract that scans comments is testing the author's prose.
    Several of the literals these tests forbid are quoted in the
    comments that explain why they were removed.
    """
    import re

    without_block = re.sub(r"/\*.*?\*/", " ", source, flags=re.S)
    return re.sub(r"//[^\n]*", " ", without_block)


def test_the_start_page_does_not_present_its_own_links_as_the_users_favourites():
    """The four tiles on the start page are a list the app shipped with.

    WHY this matters: the heading used to read "Favorites", directly
    above the user's own bookmark list -- which says it is empty. A person
    reading the screen was told their favourites were InterOS, YouTube,
    GitHub and Wikipedia, and nothing in the app had ever recorded that.
    """
    source = read("apps/browser/src/main.rs")
    code = _code_only(source)

    assert 'b"Favorites"' not in code, "the start page still calls its own links the user's favourites"
    assert 'b"Quick links"' in code, "the start page's shipped links are not labelled as such"

    # The user's own list keeps its own heading, so the two are not
    # confusable.
    assert 'b"Your pages"' in code, "the user's own bookmark list lost its heading"


def test_the_start_page_tiles_do_not_impersonate_the_sites_they_link_to():
    """A quick link is a destination, not that destination's identity.

    WHY this matters: each tile was painted in the site's own brand
    colour -- YouTube in its red, GitHub in its near-black, Wikipedia in
    its blue -- behind a monogram. A row of four tiles that each look
    like a site icon is a set of logos the app copied, and the browser
    holds no licence to them and no link to them.
    """
    source = read("apps/browser/src/main.rs")
    code = _code_only(source)

    for brand_colour in (
        "0xFF3A1014",  # the tile behind the play triangle
        "0xFFFF0000",  # the border of that tile
        "0xFF21262D",  # the tile behind the source monogram
        "0xFF30363D",  # the border of that tile
        "0xFF182230",  # the tile behind the encyclopedia monogram
        "0xFF388BFD",  # the border of the first tile
        "0xFF58A6FF",  # the monogram ink of the first and last tiles
    ):
        assert brand_colour not in code, (
            f"the start page still paints a tile in a site's own colour ({brand_colour})"
        )


def test_the_omnibox_does_not_claim_a_secure_connection_it_never_made():
    """A padlock says the connection was verified.

    WHY this matters: the mark was green for any address naming an
    https origin, and it was drawn on the error screen too -- so a
    navigation that had sent nothing showed a verified lock beside a URL
    that had never been contacted. This browser reads a page over plain
    HTTP from the device's local gateway; it performs no TLS handshake of
    its own, so it has no verified channel to report.
    """
    source = read("apps/browser/src/main.rs")
    code = _code_only(source)

    # The mark is drawn only for a page that actually loaded.
    assert "app.status != PageStatus::Loaded" in code, (
        "the address field draws its security mark without checking that a page loaded"
    )

    # And it is never green.
    assert "CHROME_GREEN" not in code, "the browser paints a verified-connection mark it cannot verify"

    # The emerald is gone from the palette entirely, so it cannot be
    # reintroduced by accident.
    assert "0xFF34D399" not in code, "the verified-connection colour is still in the browser's palette"


def test_the_error_screen_names_the_failure_it_had():
    """Every failure used to print the same badge.

    WHY this matters: an invalid address, a refused local read and a
    gateway that answered nothing all produced
    "ERR_NETWORK_TRANSPORT_REFUSED", so the badge was decoration rather
    than a report.
    """
    source = read("apps/browser/src/main.rs")
    code = _code_only(source)

    assert "ERR_NETWORK_TRANSPORT_REFUSED" not in code, (
        "the error screen still stamps every failure with one code"
    )
    for specific in (
        "NO RESPONSE FROM THE GATEWAY",
        "INVALID ADDRESS",
        "REFUSED BY THE DEVICE",
    ):
        assert specific in code, f"the error screen does not name {specific!r}"


def test_a_local_document_is_read_rather_than_assumed_to_be_unavailable():
    """The browser holds a filesystem read capability.

    WHY this matters: the local-document branch logged that the
    filesystem refused the document and that the app held no filesystem
    capability, without asking the filesystem and without checking the
    capability. The refusal it reported was one it had assumed.
    """
    source = read("apps/browser/src/main.rs")
    code = _code_only(source)

    assert "fn load_local_document" in code, "the browser does not read a local document"
    assert "std::fs_open" in code, "the local document is not opened from the filesystem"
    assert "cap_type::FILE_SYSTEM_READ" in code, (
        "the local document is not gated on a filesystem read capability"
    )


def test_a_sheet_is_drawn_over_a_dimmed_screen():
    """A dropdown that overlaps content with nothing dimming it reads as a fault.

    WHY this matters: the overflow menu drew its card straight over the
    page title with the content at full brightness underneath, so the
    overlap looked accidental rather than layered.
    """
    source = read("apps/browser/src/main.rs")
    code = _code_only(source)

    assert "blend_toward" in code, "no scrim is drawn behind an open sheet"
    assert "app.overlay != SheetOverlay::None" in code or "app.overlay == SheetOverlay::None" in code, (
        "the sheet overlay is not composed over the content"
    )
    # The scrim must not be computed from the content function it is
    # drawn over, or the screen recurses into itself.
    assert code.count("get_browser_pixel_content(") == 2, (
        "the scrim path and the content path must be two distinct calls"
    )


def test_a_local_failure_is_reported_as_the_failure_that_happened():
    """A refused read and an empty file are different failures.

    WHY this matters: the error screen printed one paragraph for every
    local failure, so a document that opened and whose read was refused
    was reported as an app that held no filesystem capability -- which it
    did hold. The screen now names the failure.
    """
    source = read("apps/browser/src/main.rs")
    code = _code_only(source)

    for reason in (
        "NO PATH GIVEN",
        "NO FILESYSTEM CAPABILITY",
        "REFUSED BY THE DEVICE",
        "THE FILE IS EMPTY",
        "THE READ WAS REFUSED",
    ):
        assert reason in code, f"the error screen does not name {reason!r}"

    # The read's own result is kept rather than collapsed into a zero.
    assert "unwrap_or(0)" not in code, (
        "the local read discards its error, so a refused read is reported as an empty file"
    )
    assert "LocalFailure::ReadRefused" in code, "a refused local read is not distinguished"


def _one_fn(source: str, header: str) -> str:
    """The body of one function, from its `fn` line to the next one.

    WHY: several of these rules are about where a call happens rather than
    whether it happens at all, and scanning the whole file cannot tell a read
    taken once per frame from one taken once per pixel.
    """
    start = source.index(header)
    end = source.find("\nfn ", start + 1)
    return source[start:] if end == -1 else source[start:end]


def test_the_top_band_is_the_platforms_and_its_clock_is_read_once_a_frame():
    """The browser used to paint forty-eight blank rows above its toolbar.

    WHY this matters: a blank inset reads as a layout fault, and it left the
    person using the browser with no way to see the time. The band belongs to
    the platform, so the app draws it through the SDK's component rather than
    keeping its own copy of the clock's place and the glyphs' geometry -- the
    copy that let four screens disagree about one bar.

    The second half is the cost of doing it: the renderer answers one pixel at
    a time, so a band that asked the RTC where it paints would take thousands
    of system calls to draw a frame. The reading is taken once, in the frame.
    """
    source = read("apps/browser/src/main.rs")
    code = _code_only(source)

    band = _one_fn(code, "fn get_browser_pixel_content")
    assert "components::StatusBar::new(W)" in band
    assert "components::Instruments::unmeasured()" in band
    assert "if y < STATUS_H {" in band

    frame = _one_fn(code, "fn build(app: &BrowserApp)")
    assert "std::rtc::Reading::now()" in frame, "the frame carries no clock reading"
    assert "now.clock_text()" in frame
    assert "rtc::Reading" not in band, (
        "the per-pixel renderer reads the clock, so one frame costs a syscall per pixel"
    )


def test_a_tap_answers_the_control_the_painter_drew_under_it():
    """Two screens addressed their own geometry by literal, and the two
    readings of the same pixels disagreed.

    WHY this matters: the overflow menu painted its rows from y=130 and counted
    them from the card's top edge at y=116, so the fourteen pixels at the top of
    every row belonged to the item above -- "New Tab" opened Bookmarks and "Save
    Page" reloaded the page. The tab switcher asked only `x >= 630`, which is the
    whole right edge of a 672-pixel card, so a tap on the padding beside a title
    closed the tab instead of switching to it, while the glyph itself was only
    seventeen pixels across.

    Both now read the same constant the painter does, and the close target is
    drawn as the box that answers the tap.
    """
    source = read("apps/browser/src/main.rs")
    code = _code_only(source)

    # Menu: one origin and one row height, shared by the painter and the tap.
    assert "const MENU_ROW_Y0: usize = 130;" in code
    assert "const MENU_ROW_H: usize = 44;" in code
    assert "MENU_ROW_Y0 + idx * MENU_ROW_H" in code
    assert "y.saturating_sub(MENU_ROW_Y0)) / MENU_ROW_H" in code
    assert "130 + idx * 44" not in code, "the menu painter went back to its own literal"
    assert "(y - 116) / 44" not in code, "the menu tap counter went back to the card's top edge"

    # Tab switcher: the painted target and the hit box are the same rectangle.
    assert "fn tab_close_rect(card_y: usize) -> Rect" in code
    assert "let close_rect = tab_close_rect(card_y);" in code
    assert "tab_close_rect(card_y).contains(x, y)" in code
    assert "if x >= 630 {" not in code, (
        "the tab card closes a tab for a tap anywhere along its right edge"
    )
    assert "MIN_TOUCH_TARGET," in code, "the close target is not sized to the platform's floor"
