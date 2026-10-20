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
