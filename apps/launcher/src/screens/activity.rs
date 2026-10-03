//! Activity Space: the multitasking overview deck.
//!
//! WHY this file owns the whole surface: `main.rs` keeps the tap and swipe
//! handler that reaches this screen, and the two have to agree on every
//! rectangle in it. Splitting the picture from the handler is what let them
//! drift apart in the first place.
//!
//! WHY there is nothing here called Work, Personal, Research or Media: those are
//! cards in the design mock-up, not data this platform has. Nothing in the
//! package registry exposes a category this shell can read, so a card labelled
//! "Work" would be a fabrication with an app count attached to it. Every card
//! below is one real entry in the recent-apps list, named from the registry, and
//! when that list is empty the deck says so instead of inventing neighbours.

#![allow(unused_imports)]

use crate::{test_text, LauncherState};
use launcher::layout::{self, Rect};
use launcher::text;
use launcher::theme::*;

// -----------------------------------------------------------------------------
// Geometry
// -----------------------------------------------------------------------------

/// Height of one card. WHY typed rather than computed: `main.rs` hit-tests the
/// deck as three bands of `(200..480, 520..800, 840..1120)`, taps a card body for
/// `x` 56..580 and a dismiss badge for `x` 580..664. A card drawn outside its own
/// band leaves a live target nobody can see, and a badge drawn outside the badge
/// column turns a dismiss into a launch. `layout::list` is called below with the
/// full capacity of the recent list precisely so the bands are identical whether
/// one app is running or three, and band N is always slot N.
const CARD_H: usize = 280;
/// Gap between cards, which is what turns three bands into a deck.
const CARD_GAP: usize = 40;
/// The recent list holds three entries, so the deck has three bands.
const CARD_BANDS: usize = 3;
/// Horizontal inset of the deck, on the spacing scale.
const DECK_X: usize = space::XXL;
const DECK_W: usize = SCREEN_WIDTH - 2 * space::XXL;
/// Header text sits one step below the status bar.
const HEADER_Y: usize = STATUS_BAR_HEIGHT + space::MD;
/// Hairline under the header, one step below the title's glyph rows.
const RULE_Y: usize = HEADER_Y + type_scale::BODY * text::GLYPH_H + space::LG;
/// The deck starts two steps below the rule. This lands on 200, the first band
/// the handler tests, because the numbers above are the real ones.
const DECK_Y: usize = RULE_Y + space::XXL + CONTENT_PADDING;
const DECK: Rect = Rect::new(
    DECK_X,
    DECK_Y,
    DECK_W,
    CARD_BANDS * CARD_H + (CARD_BANDS - 1) * CARD_GAP,
);

/// The icon tile on a card. Square, and vertically centred in the card so the
/// three text lines beside it sit against its middle third.
const CARD_TILE: usize = 112;
/// Text column, measured from the card's own left edge.
const CARD_TEXT_X: usize = space::MD + CARD_TILE + space::LG;
/// Text column width: the card's width less the tile, the margin and the outer
/// padding.
const CARD_TEXT_W: usize = DECK_W - CARD_TEXT_X - space::MD;
const CARD_TITLE_DY: usize = (CARD_H - CARD_TILE) / 2 + space::MD;
const CARD_SUBTITLE_DY: usize =
    CARD_TITLE_DY + type_scale::BODY * text::GLYPH_H + space::SM;
const CARD_ID_DY: usize = CARD_SUBTITLE_DY + type_scale::CAPTION * text::GLYPH_H + space::SM;

/// The dismiss badge. WHY sized and placed against the handler: the region it has
/// to cover is `x` 580..664 in the first 60 rows of each band, and a badge that
/// pokes out of that column is a tap that launches the app instead.
const BADGE_W: usize = 72;
const BADGE_H: usize = 44;
const BADGE_DY: usize = space::SM;
/// Half-extent of the badge's cross.
const BADGE_ARM: i32 = 20;

/// WHY this rectangle is typed: `main.rs` returns to the home grid for a tap in
/// `(1200..1300)` or anywhere at `y >= 1400`, so the control that closes the
/// deck has to sit inside the first of those bands or it would be a button that
/// does nothing.
const CLOSE_H: usize = space::XXL + space::XL;
const CLOSE_Y: usize = DECK_Y + DECK_H_CONST + space::XXL + space::XXL;
/// Total deck height, spelled out because `Rect` fields are not const-readable in
/// a constant expression on every toolchain this shell builds with.
const DECK_H_CONST: usize = CARD_BANDS * CARD_H + (CARD_BANDS - 1) * CARD_GAP;

/// The home indicator, drawn where the reference draws it and hit-tested by the
/// same `y >= 1400` rule as the close pill.
const HOME_PILL_W: usize = space::XXL * 4;
const HOME_PILL_H: usize = space::SM;
const HOME_PILL_Y: usize = SCREEN_HEIGHT - space::XXL;

// -----------------------------------------------------------------------------
// Rounded rectangles
// -----------------------------------------------------------------------------

/// Whether a pixel belongs to a rectangle whose corners are rounded to `rad`.
///
/// WHY the shell needs this: `radius` is part of the theme, and a theme token no
/// drawing code consults is documentation. The reference's cards are strongly
/// rounded, so a square-cornered card is not a stylistic difference, it is a
/// different shape.
fn rounded_contains(r: Rect, x: usize, y: usize, rad: usize) -> bool {
    if !r.contains(x, y) {
        return false;
    }
    let rad = rad.min(r.w / 2).min(r.h / 2);
    if rad == 0 {
        return true;
    }
    let left = r.x + rad;
    let right = r.x + r.w - 1 - rad;
    let top = r.y + rad;
    let bottom = r.y + r.h - 1 - rad;
    // A pixel outside the corner boxes is inside the cross the rounded rectangle
    // shares with the plain one, so only corner pixels need the distance test.
    if (x >= left && x <= right) || (y >= top && y <= bottom) {
        return true;
    }
    let cx = if x < left { left } else { right };
    let cy = if y < top { top } else { bottom };
    let dx = x as i32 - cx as i32;
    let dy = y as i32 - cy as i32;
    dx * dx + dy * dy <= (rad as i32) * (rad as i32)
}

/// The one-pixel outline of a rounded rectangle.
fn rounded_outline(r: Rect, x: usize, y: usize, rad: usize) -> bool {
    rounded_contains(r, x, y, rad)
        && !rounded_contains(r.inset(1), x, y, rad.saturating_sub(1))
}

// -----------------------------------------------------------------------------
// Text helpers
// -----------------------------------------------------------------------------

/// Draws `bytes` with its left edge at `x` and its top row at `y`, cut with an
/// ellipsis when it is wider than `width`. WHY the helper: the old file centred
/// text with `len() * 16 / 2`, which assumed one glyph width and no clipping, so a
/// long app name was drawn off the card.
///
/// `px`/`py` are the pixel being rendered and `x`/`y` are the text's own origin.
/// They are different values, and `test_text` needs both: the origin is what it
/// divides by to find the glyph, and the pixel is what it tests.
fn label(
    px: usize,
    py: usize,
    x: usize,
    y: usize,
    scale: usize,
    width: usize,
    bytes: &[u8],
    ink: u32,
) -> Option<u32> {
    let (buf, len) = text::ellipsize(bytes, scale, width);
    if len == 0 {
        return None;
    }
    test_text(px, py, x, y, scale, &buf[..len], ink)
}

/// The same, centred across `width` from `x`.
fn label_centred(
    px: usize,
    py: usize,
    x: usize,
    y: usize,
    scale: usize,
    width: usize,
    bytes: &[u8],
    ink: u32,
) -> Option<u32> {
    let (buf, len) = text::ellipsize(bytes, scale, width);
    if len == 0 {
        return None;
    }
    let inset = text::center_x(&buf[..len], scale, width);
    test_text(px, py, x + inset, y, scale, &buf[..len], ink)
}

/// The same, ending at `x_end`.
fn label_ending(
    px: usize,
    py: usize,
    x_end: usize,
    y: usize,
    scale: usize,
    bytes: &[u8],
    ink: u32,
) -> Option<u32> {
    let (buf, len) = text::ellipsize(bytes, scale, x_end);
    if len == 0 {
        return None;
    }
    let inset = x_end.saturating_sub(text::measure(&buf[..len], scale));
    test_text(px, py, inset, y, scale, &buf[..len], ink)
}

/// Writes `n` in decimal into `buf` and returns the byte count. WHY not
/// `format!`: this is a `no_std` shell with no allocator.
fn decimal(n: usize, buf: &mut [u8]) -> usize {
    if n == 0 {
        if let Some(slot) = buf.first_mut() {
            *slot = b'0';
            return 1;
        }
        return 0;
    }
    let mut digits = [0u8; 8];
    let mut count = 0usize;
    let mut value = n;
    while value > 0 && count < digits.len() {
        digits[count] = b'0' + (value % 10) as u8;
        value /= 10;
        count += 1;
    }
    for (i, slot) in buf.iter_mut().enumerate().take(count) {
        *slot = digits[count - 1 - i];
    }
    count
}

/// `n` written as "1 app" or "3 apps". Both words are about the deck's own
/// contents, so the count can never claim more than the list holds.
fn app_word(n: usize, buf: &mut [u8]) -> usize {
    let count = decimal(n, buf);
    let tail: &[u8] = if n == 1 { b" app" } else { b" apps" };
    for (i, byte) in tail.iter().enumerate() {
        if let Some(slot) = buf.get_mut(count + i) {
            *slot = *byte;
        }
    }
    (count + tail.len()).min(buf.len())
}

// -----------------------------------------------------------------------------
// State
// -----------------------------------------------------------------------------

/// How many recent slots hold a real app.
///
/// WHY the prefix and not a tally: `push_recent` rewrites the array as a packed
/// prefix and `dismiss_recent` shifts the tail down, so the occupied slots are
/// `0..n` with no holes. Reading them as a prefix is what keeps band N in step
/// with slot N, which is what the handler in `main.rs` assumes.
fn deck_len(state: &LauncherState) -> usize {
    let mut n = 0usize;
    while n < state.recent_apps.len() && state.recent_app_lens[n] > 0 {
        n += 1;
    }
    n
}

/// The registry icon token for a recent slot, if the package is catalogued.
///
/// WHY this is a lookup and not a stored field: the recent list holds package ids
/// and the catalogue holds the icon next to the id it belongs to, so the two only
/// meet here. A slot whose package is not in the catalogue has no token, and the
/// card says so by drawing an empty tile rather than a made-up glyph.
fn recent_icon<'a>(state: &'a LauncherState, index: usize) -> Option<&'a [u8]> {
    let id = state.recent_id(index)?;
    let id = id.as_bytes();
    for i in 0..state.app_count {
        let len = state.app_id_lens[i] as usize;
        if len == 0 || len != id.len() || &state.app_ids[i][..len] != id {
            continue;
        }
        let raw = &state.app_icons[i];
        // The catalogue pads the token with spaces to its four-byte width, so the
        // first space is the end of the token.
        let len = raw.iter().position(|b| *b == b' ').unwrap_or(raw.len());
        return if len == 0 { None } else { Some(&raw[..len]) };
    }
    None
}

/// The lifecycle line on a card, from the two facts the shell actually holds:
/// whether a live task id was matched to this slot, and whether that task is the
/// one in the foreground.
fn card_state(state: &LauncherState, index: usize) -> (&'static [u8], bool) {
    let task = state.recent_task_ids[index];
    if task > 0 && state.active_child == Some(task) {
        (b"Focused and running", true)
    } else if task > 0 {
        (b"Running, not in front", false)
    } else {
        (b"In registry, not running", false)
    }
}

// -----------------------------------------------------------------------------
// Renderer
// -----------------------------------------------------------------------------

#[inline(never)]
pub fn get_activity_color(x: usize, y: usize, state: &LauncherState) -> u32 {
    // WHY `Theme::DARK` here rather than one inherited default: the reference
    // splits its palette by context. Activity Space, Quick Controls and the
    // Power Menu are the near-black system, because an overlay that lightens the
    // wallpaper stops reading as an overlay; Events, Files and Gallery are light
    // because they are documents rather than chrome. Binding the palette in the
    // screen that draws it keeps that split legible in the source and lets a
    // screen be re-toned without a global that every other screen has to agree
    // with.
    let theme = Theme::DARK;

    // WHY a flat backdrop: the reference's darkness is the point, and a wallpaper
    // ramp behind a deck of cards competes with the cards for attention.
    let mut colour = theme.bg;

    let count = deck_len(state);

    // Status bar: the clock and nothing else. WHY no signal, Wi-Fi or battery
    // glyphs: `LauncherState` carries no reading for any of them, and a filled
    // bar would claim a connection this shell has not measured.
    if state.clock_source_ok {
        let row = (STATUS_BAR_HEIGHT - type_scale::CAPTION * text::GLYPH_H) / 2;
        if let Some(c) = label(
            x,
            y,
            CONTENT_PADDING,
            row,
            type_scale::CAPTION,
            space::XXL * 6,
            &state.clock_time,
            theme.text_muted,
        ) {
            colour = c;
        }
    }

    // Header. WHY the count on the right and a search glyph is absent: the deck
    // has no search behind it, and a magnifier that opens nothing is worse than no
    // magnifier.
    if let Some(c) = label(
        x,
        y,
        CONTENT_PADDING,
        HEADER_Y,
        type_scale::BODY,
        DECK_W,
        b"Activity Space",
        theme.text,
    ) {
        colour = c;
    }
    if count > 0 {
        let mut word = [b' '; 24];
        let len = app_word(count, &mut word);
        if let Some(c) = label_ending(
            x,
            y,
            SCREEN_WIDTH - CONTENT_PADDING,
            HEADER_Y + (type_scale::BODY - type_scale::CAPTION) * text::GLYPH_H,
            type_scale::CAPTION,
            &word[..len],
            theme.text_muted,
        ) {
            colour = c;
        }
    }

    // Hairline under the header.
    let rule = Rect::new(
        CONTENT_PADDING,
        RULE_Y,
        SCREEN_WIDTH - 2 * CONTENT_PADDING,
        1,
    );
    if rule.contains(x, y) {
        colour = theme.border;
    }

    // The deck.
    if y >= DECK_Y {
        let row = (y - DECK_Y) / (CARD_H + CARD_GAP);
        if row < CARD_BANDS {
            let metrics = layout::list(DECK, CARD_BANDS, CARD_H, CARD_GAP);
            if let Some(card) = metrics.and_then(|m| layout::list_row(m, row, CARD_BANDS, CARD_H, CARD_GAP))
            {
                if state.recent_app_lens[row] > 0 {
                    if let Some(c) = card_pixel(x, y, card, row, state, &theme) {
                        colour = c;
                    }
                } else if count == 0 && row == 0 {
                    if let Some(c) = empty_deck_pixel(x, y, card, &theme) {
                        colour = c;
                    }
                }
            }
        }
    }

    // Close pill.
    let close = Rect::new(CONTENT_PADDING, CLOSE_Y, SCREEN_WIDTH - 2 * CONTENT_PADDING, CLOSE_H);
    if rounded_contains(close, x, y, radius::SHEET) {
        colour = theme.surface_raised;
        if rounded_outline(close, x, y, radius::SHEET) {
            colour = theme.border;
        }
        if let Some(c) = label_centred(
            x,
            y,
            close.x,
            close.y + (CLOSE_H - type_scale::BODY * text::GLYPH_H) / 2,
            type_scale::BODY,
            close.w,
            b"Close",
            theme.text,
        ) {
            colour = c;
        }
    }

    // Home indicator.
    let home = Rect::new(
        SCREEN_WIDTH / 2 - HOME_PILL_W / 2,
        HOME_PILL_Y,
        HOME_PILL_W,
        HOME_PILL_H,
    );
    if rounded_contains(home, x, y, radius::CHIP) {
        colour = Theme::veil(theme.text, 90, true);
    }

    colour
}

/// One card's pixels. Returns `None` for a pixel that is not on the card, so the
/// caller keeps its backdrop.
fn card_pixel(
    x: usize,
    y: usize,
    card: Rect,
    row: usize,
    state: &LauncherState,
    theme: &Theme,
) -> Option<u32> {
    if !rounded_contains(card, x, y, radius::CARD) {
        return None;
    }
    let mut colour = theme.surface;
    if rounded_outline(card, x, y, radius::CARD) {
        colour = theme.border;
    }

    // The icon tile: the registry's own token for this package, or an empty tile
    // when the package is not catalogued.
    let tile = Rect::new(
        card.x + space::MD,
        card.y + (CARD_H - CARD_TILE) / 2,
        CARD_TILE,
        CARD_TILE,
    );
    if rounded_contains(tile, x, y, radius::CONTROL) {
        let token = recent_icon(state, row);
        colour = if token.is_some() {
            theme.accent
        } else {
            theme.surface_sunken
        };
        if rounded_outline(tile, x, y, radius::CONTROL) {
            colour = theme.border;
        }
        if let Some(token) = token {
            if let Some(c) = label_centred(
                x,
                y,
                tile.x,
                tile.y + (CARD_TILE - type_scale::BODY * text::GLYPH_H) / 2,
                type_scale::BODY,
                CARD_TILE,
                token,
                theme.text_on_accent,
            ) {
                colour = c;
            }
        }
    }

    // Name, lifecycle line, package id: all three read the same slot.
    let text_x = card.x + CARD_TEXT_X;
    let title = state
        .recent_label(row)
        .map(|bytes| bytes as &[u8])
        .or_else(|| state.recent_id(row).map(|id| id.as_bytes()));
    if let Some(title) = title {
        if let Some(c) = label(
            x,
            y,
            text_x,
            card.y + CARD_TITLE_DY,
            type_scale::BODY,
            CARD_TEXT_W,
            title,
            theme.text,
        ) {
            colour = c;
        }
    }
    let (line, focused) = card_state(state, row);
    if let Some(c) = label(
        x,
        y,
        text_x,
        card.y + CARD_SUBTITLE_DY,
        type_scale::CAPTION,
        CARD_TEXT_W,
        line,
        if focused { theme.accent } else { theme.text_muted },
    ) {
        colour = c;
    }
    if let Some(id) = state.recent_id(row) {
        if let Some(c) = label(
            x,
            y,
            text_x,
            card.y + CARD_ID_DY,
            type_scale::CAPTION,
            CARD_TEXT_W,
            id.as_bytes(),
            theme.text_muted,
        ) {
            colour = c;
        }
    }

    // Dismiss badge. WHY a cross and not the reference's arrow: the arrow in the
    // mock-up opens a group, and there is no group here to open. The cross is the
    // action `main.rs` actually performs on this rectangle.
    let badge = Rect::new(
        card.x + DECK_W - space::MD - BADGE_W,
        card.y + BADGE_DY,
        BADGE_W,
        BADGE_H,
    );
    if rounded_contains(badge, x, y, radius::CONTROL) {
        colour = theme.surface_raised;
        if rounded_outline(badge, x, y, radius::CONTROL) {
            colour = theme.border;
        }
        let dx = x as i32 - (badge.x + BADGE_W / 2) as i32;
        let dy = y as i32 - (badge.y + BADGE_H / 2) as i32;
        if dx.abs() <= BADGE_ARM
            && dy.abs() <= BADGE_ARM
            && ((dx - dy).abs() <= 1 || (dx + dy).abs() <= 1)
        {
            colour = theme.text_muted;
        }
    }

    Some(colour)
}

/// The deck with nothing in it. WHY an outlined card rather than four category
/// cards: the shape tells the reader where cards will appear, and the two lines
/// say why none are here.
fn empty_deck_pixel(x: usize, y: usize, card: Rect, theme: &Theme) -> Option<u32> {
    if !rounded_contains(card, x, y, radius::CARD) {
        return None;
    }
    let mut colour = theme.surface_sunken;
    if rounded_outline(card, x, y, radius::CARD) {
        colour = theme.border;
    }
    if let Some(c) = label_centred(
        x,
        y,
        card.x,
        card.y + card.h / 2 - type_scale::BODY * text::GLYPH_H,
        type_scale::BODY,
        card.w,
        b"No recent apps",
        theme.text_muted,
    ) {
        colour = c;
    }
    if let Some(c) = label_centred(
        x,
        y,
        card.x,
        card.y + card.h / 2 + space::SM,
        type_scale::CAPTION,
        card.w,
        b"Launch an app to fill the deck",
        theme.text_muted,
    ) {
        colour = c;
    }
    Some(colour)
}
