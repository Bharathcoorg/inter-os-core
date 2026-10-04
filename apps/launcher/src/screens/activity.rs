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
// -----------------------------------------------------------------------------
// Runs: every label, measured once per frame
// -----------------------------------------------------------------------------

/// Capacity of one run's byte buffer. WHY it matches `text::ellipsize`, so a run
/// built here is byte-for-byte the run the old per-pixel helper built for any input
/// the registry or the recent list can hold.
const RUN_CAP: usize = 96;

/// One label, placed and measured once per frame.
///
/// WHY this type: the old file centred text with `len() * 16 / 2`, which assumed one
/// glyph width and no clipping, so the helpers below existed to clip properly -- and
/// each of them called `text::ellipsize`, which returns a 96-byte buffer by value.
/// Six of them per pixel over a 1,036,800-pixel surface is where this screen's frame
/// went. Everything that depends on the string rather than the pixel -- the
/// ellipsis decision, the centring inset, the advance width -- is settled here.
///
/// WHY the box is carried too: `test_text` walks the pen of every glyph before it
/// can answer "no", so a pixel nowhere near a run still costs a pen walk per label.
/// The stored box is exactly `y .. y + line_height(scale)` by
/// `x .. x + measure(run)`, which is the region `test_text` can return `Some` for, so
/// rejecting against it changes no pixel and skips the walk for almost every one.
struct Run {
    x: usize,
    y: usize,
    w: usize,
    h: usize,
    scale: usize,
    len: usize,
    ink: u32,
    bytes: [u8; RUN_CAP],
}

impl Run {
    /// A run that never inks anything: the "this slot has no app" case.
    const EMPTY: Run = Run {
        x: 0,
        y: 0,
        w: 0,
        h: 0,
        scale: 1,
        len: 0,
        ink: 0,
        bytes: [0u8; RUN_CAP],
    };

    /// `bytes` with its left edge at `x` and its top row at `y`, ellipsized to
    /// `width` -- the old `label` helper, resolved once instead of per pixel.
    fn new(x: usize, y: usize, scale: usize, width: usize, bytes: &[u8], ink: u32) -> Run {
        let (buf, len) = text::ellipsize(bytes, scale, width);
        Run::placed(x, y, scale, buf, len, ink)
    }

    /// The same, centred across `width` from `x` -- the old `label_centred`.
    fn centred(
        x: usize,
        y: usize,
        scale: usize,
        width: usize,
        bytes: &[u8],
        ink: u32,
    ) -> Run {
        let (buf, len) = text::ellipsize(bytes, scale, width);
        let inset = text::center_x(&buf[..len], scale, width);
        Run::placed(x + inset, y, scale, buf, len, ink)
    }

    /// The same, ending at `x_end` -- the old `label_ending`.
    ///
    /// WHY it is kept exactly: that helper fitted the run to `x_end` rather than to
    /// the text column, which is harmless here only because the header's right-hand
    /// count is far narrower than the right margin. Narrowing the fit would be a
    /// tidier rule and a different screen, so the old arithmetic stands.
    fn ending(x_end: usize, y: usize, scale: usize, bytes: &[u8], ink: u32) -> Run {
        let (buf, len) = text::ellipsize(bytes, scale, x_end);
        let inset = x_end.saturating_sub(text::measure(&buf[..len], scale));
        Run::placed(inset, y, scale, buf, len, ink)
    }

    /// A run from an already-resolved buffer.
    fn placed(x: usize, y: usize, scale: usize, buf: [u8; RUN_CAP], len: usize, ink: u32) -> Run {
        Run {
            x,
            y,
            w: text::measure(&buf[..len], scale),
            h: text::line_height(scale),
            scale,
            len,
            ink,
            bytes: buf,
        }
    }

    /// The colour of (`px`, `py`) if this run inks it.
    fn pixel(&self, px: usize, py: usize) -> Option<u32> {
        if self.len == 0 {
            return None;
        }
        if px < self.x || px - self.x >= self.w || py < self.y || py - self.y >= self.h {
            return None;
        }
        test_text(px, py, self.x, self.y, self.scale, &self.bytes[..self.len], self.ink)
    }
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
// Frame: what this deck shows, resolved once
// -----------------------------------------------------------------------------

/// One band of the deck, with every rectangle and every line of it resolved.
struct CardPlan {
    rect: Rect,
    tile: Rect,
    /// The registry's icon token for this slot, centred in the tile. Empty when the
    /// package is not catalogued, which is also what decides the tile's fill.
    token: Run,
    title: Run,
    line: Run,
    id: Run,
    badge: Rect,
}

/// The empty-deck card: its rectangle and its two lines, resolved.
struct EmptyPlan {
    rect: Rect,
    head: Run,
    body: Run,
}

/// Everything the painter needs that does not depend on the pixel.
///
/// WHY this is built once per frame: the deck's card painter asked
/// `state.recent_icon` -- which scans the whole package catalogue for a matching id
/// -- for every pixel inside a card's icon tile, asked `card_state` and `deck_len`
/// for every pixel inside a card, and ellipsized four lines for every pixel of the
/// header, the clock and the close pill. That is 1,036,800 pixels going back to the
/// registry one card at a time.
struct Frame {
    clock: Option<Run>,
    header: Run,
    /// The "N apps" line on the right of the header, absent when the deck is empty.
    count: Option<Run>,
    rule: Rect,
    cards: [CardPlan; CARD_BANDS],
    /// `Some` only when the recent list is empty, which is the only case in which
    /// band 0 draws the empty card instead of an app.
    empty: Option<EmptyPlan>,
    close: Rect,
    close_label: Run,
    home: Rect,
    home_ink: u32,
}

impl CardPlan {
    const EMPTY: CardPlan = CardPlan {
        rect: Rect::new(0, 0, 0, 0),
        tile: Rect::new(0, 0, 0, 0),
        token: Run::EMPTY,
        title: Run::EMPTY,
        line: Run::EMPTY,
        id: Run::EMPTY,
        badge: Rect::new(0, 0, 0, 0),
    };
}

/// Resolves one band of the deck.
fn card_plan(state: &LauncherState, card: Rect, row: usize, theme: &Theme) -> CardPlan {
    let tile = Rect::new(
        card.x + space::MD,
        card.y + (CARD_H - CARD_TILE) / 2,
        CARD_TILE,
        CARD_TILE,
    );
    // WHY the token is looked up here: `recent_icon` walks the catalogue comparing
    // package ids, once per pixel of the icon tile, and the icon tile is the one
    // place on this screen that consults the registry while painting.
    let token = recent_icon(state, row);
    let token_run = match token {
        Some(bytes) => Run::centred(
            tile.x,
            tile.y + (CARD_TILE - type_scale::BODY * text::GLYPH_H) / 2,
            type_scale::BODY,
            CARD_TILE,
            bytes,
            theme.text_on_accent,
        ),
        None => Run::EMPTY,
    };
    // WHY the title falls back to the package id: `recent_label` returns nothing for
    // a package that is not in the catalogue, and the old painter then drew the id
    // instead. An empty run reproduces the case where neither exists.
    let text_x = card.x + CARD_TEXT_X;
    let title = state
        .recent_label(row)
        .map(|bytes| bytes as &[u8])
        .or_else(|| state.recent_id(row).map(|id| id.as_bytes()));
    let title_run = match title {
        Some(bytes) => Run::new(
            text_x,
            card.y + CARD_TITLE_DY,
            type_scale::BODY,
            CARD_TEXT_W,
            bytes,
            theme.text,
        ),
        None => Run::EMPTY,
    };
    let (line, focused) = card_state(state, row);
    let line_run = Run::new(
        text_x,
        card.y + CARD_SUBTITLE_DY,
        type_scale::CAPTION,
        CARD_TEXT_W,
        line,
        if focused { theme.accent } else { theme.text_muted },
    );
    let id_run = match state.recent_id(row) {
        Some(id) => Run::new(
            text_x,
            card.y + CARD_ID_DY,
            type_scale::CAPTION,
            CARD_TEXT_W,
            id.as_bytes(),
            theme.text_muted,
        ),
        None => Run::EMPTY,
    };
    CardPlan {
        rect: card,
        tile,
        token: token_run,
        title: title_run,
        line: line_run,
        id: id_run,
        badge: Rect::new(
            card.x + DECK_W - space::MD - BADGE_W,
            card.y + BADGE_DY,
            BADGE_W,
            BADGE_H,
        ),
    }
}

/// Builds this frame's plan.
///
/// WHY the bands come out of `layout::list` and `layout::list_row` here: the shell's
/// handler hit-tests the deck as three bands of `(200..480, 520..800, 840..1120)`
/// with a dismiss badge in `x` 580..664, so those rectangles have to be the same ones
/// the painter uses. Computing them from the same two calls in the same place is what
/// keeps them identical.
fn build(state: &LauncherState) -> Frame {
    let theme = Theme::DARK;
    let count = deck_len(state);

    let clock = if state.clock_source_ok {
        let row = (STATUS_BAR_HEIGHT - type_scale::CAPTION * text::GLYPH_H) / 2;
        Some(Run::new(
            CONTENT_PADDING,
            row,
            type_scale::CAPTION,
            space::XXL * 6,
            &state.clock_time,
            theme.text_muted,
        ))
    } else {
        None
    };

    let count_run = if count > 0 {
        let mut word = [b' '; 24];
        let len = app_word(count, &mut word);
        Some(Run::ending(
            SCREEN_WIDTH - CONTENT_PADDING,
            HEADER_Y + (type_scale::BODY - type_scale::CAPTION) * text::GLYPH_H,
            type_scale::CAPTION,
            &word[..len],
            theme.text_muted,
        ))
    } else {
        None
    };

    let close = Rect::new(
        CONTENT_PADDING,
        CLOSE_Y,
        SCREEN_WIDTH - 2 * CONTENT_PADDING,
        CLOSE_H,
    );

    let mut cards = [CardPlan::EMPTY; CARD_BANDS];
    let metrics = layout::list(DECK, CARD_BANDS, CARD_H, CARD_GAP);
    if let Some(metrics) = metrics {
        for row in 0..CARD_BANDS {
            let Some(card) = layout::list_row(metrics, row, CARD_BANDS, CARD_H, CARD_GAP) else {
                continue;
            };
            if state.recent_app_lens[row] > 0 {
                cards[row] = card_plan(state, card, row, &theme);
            }
        }
    }

    // WHY the empty card is resolved only when it can be drawn: the shell shows it on
    // band 0 and only when the recent list holds nothing at all.
    let empty = if count == 0 {
        metrics
            .and_then(|m| layout::list_row(m, 0, CARD_BANDS, CARD_H, CARD_GAP))
            .map(|card| EmptyPlan {
                rect: card,
                head: Run::centred(
                    card.x,
                    card.y + card.h / 2 - type_scale::BODY * text::GLYPH_H,
                    type_scale::BODY,
                    card.w,
                    b"No recent apps",
                    theme.text_muted,
                ),
                body: Run::centred(
                    card.x,
                    card.y + card.h / 2 + space::SM,
                    type_scale::CAPTION,
                    card.w,
                    b"Launch an app to fill the deck",
                    theme.text_muted,
                ),
            })
    } else {
        None
    };

    Frame {
        clock,
        header: Run::new(
            CONTENT_PADDING,
            HEADER_Y,
            type_scale::BODY,
            DECK_W,
            b"Activity Space",
            theme.text,
        ),
        count: count_run,
        rule: Rect::new(
            CONTENT_PADDING,
            RULE_Y,
            SCREEN_WIDTH - 2 * CONTENT_PADDING,
            1,
        ),
        cards,
        empty,
        close,
        close_label: Run::centred(
            close.x,
            close.y + (CLOSE_H - type_scale::BODY * text::GLYPH_H) / 2,
            type_scale::BODY,
            close.w,
            b"Close",
            theme.text,
        ),
        home: Rect::new(
            SCREEN_WIDTH / 2 - HOME_PILL_W / 2,
            HOME_PILL_Y,
            HOME_PILL_W,
            HOME_PILL_H,
        ),
        // WHY resolved here: it is channel arithmetic that the old painter ran for
        // every pixel of the surface to fill a 192 x 8 bar.
        home_ink: Theme::veil(theme.text, 90, true),
    }
}

/// This frame's plan.
///
/// WHY `(0, 0)` is the frame boundary: `render_view` walks the surface in raster
/// order from the top-left pixel, so `(0, 0)` is the first pixel of every frame and
/// the only pixel within a frame that repeats it. `LauncherState` is borrowed
/// immutably for the whole of a frame, so rebuilding there is once per frame and the
/// plan can never lag the state it describes.
static mut FRAME: Option<Frame> = None;

fn frame(x: usize, y: usize, state: &LauncherState) -> &'static Frame {
    let slot: *mut Option<Frame> = core::ptr::addr_of_mut!(FRAME);
    // SAFETY: the launcher is one bare-metal task and the paint loop is neither
    // preemptible nor reentrant, so this is the only reference to the slot at any
    // moment, and the reference returned cannot outlive the call that made it.
    let empty = unsafe { (*slot).is_none() };
    if empty || (x == 0 && y == 0) {
        // SAFETY: as above; this is the one mutation of the slot.
        unsafe { *slot = Some(build(state)) };
    }
    // SAFETY: the slot is `Some` here -- it was just filled when it was empty.
    unsafe { (*slot).as_ref() }.expect("WHY: the slot is filled when it is empty")
}
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

    // WHY the plan is fetched before anything is drawn: it carries every rectangle,
    // every line and every derived ink for this frame, and fetching it is the only
    // place in this function where anything other than a pixel test happens.
    let f = frame(x, y, state);

    // Status bar: the clock and nothing else. WHY no signal, Wi-Fi or battery
    // glyphs: `LauncherState` carries no reading for any of them, and a filled
    // bar would claim a connection this shell has not measured.
    if let Some(c) = f.clock.as_ref().and_then(|r| r.pixel(x, y)) {
        colour = c;
    }

    // Header. WHY the count on the right and a search glyph is absent: the deck
    // has no search behind it, and a magnifier that opens nothing is worse than no
    // magnifier.
    if let Some(c) = f.header.pixel(x, y) {
        colour = c;
    }
    if let Some(c) = f.count.as_ref().and_then(|r| r.pixel(x, y)) {
        colour = c;
    }

    // Hairline under the header.
    if f.rule.contains(x, y) {
        colour = theme.border;
    }

    // The deck.
    if y >= DECK_Y {
        let row = (y - DECK_Y) / (CARD_H + CARD_GAP);
        if row < CARD_BANDS && state.recent_app_lens[row] > 0 {
            if let Some(c) = card_pixel(&f.cards[row], x, y, &theme) {
                colour = c;
            }
        } else if row < CARD_BANDS && row == 0 {
            if let Some(empty) = f.empty.as_ref() {
                if let Some(c) = empty_deck_pixel(empty, x, y, &theme) {
                    colour = c;
                }
            }
        }
    }

    // Close pill.
    if rounded_contains(f.close, x, y, radius::SHEET) {
        colour = theme.surface_raised;
        if rounded_outline(f.close, x, y, radius::SHEET) {
            colour = theme.border;
        }
        if let Some(c) = f.close_label.pixel(x, y) {
            colour = c;
        }
    }

    // Home indicator.
    if rounded_contains(f.home, x, y, radius::CHIP) {
        colour = f.home_ink;
    }

    colour
}

/// One card's pixels. Returns `None` for a pixel that is not on the card, so the
/// caller keeps its backdrop.
fn card_pixel(plan: &CardPlan, x: usize, y: usize, theme: &Theme) -> Option<u32> {
    if !rounded_contains(plan.rect, x, y, radius::CARD) {
        return None;
    }
    let mut colour = theme.surface;
    if rounded_outline(plan.rect, x, y, radius::CARD) {
        colour = theme.border;
    }

    // The icon tile: the registry's own token for this package, or an empty tile
    // when the package is not catalogued. Which of the two it is was decided in the
    // plan, because it is decided by scanning the catalogue.
    if rounded_contains(plan.tile, x, y, radius::CONTROL) {
        colour = if plan.token.len > 0 {
            theme.accent
        } else {
            theme.surface_sunken
        };
        if rounded_outline(plan.tile, x, y, radius::CONTROL) {
            colour = theme.border;
        }
        if let Some(c) = plan.token.pixel(x, y) {
            colour = c;
        }
    }

    // Name, lifecycle line, package id: all three read the same slot, and all three
    // were ellipsized per pixel before.
    if let Some(c) = plan.title.pixel(x, y) {
        colour = c;
    }
    if let Some(c) = plan.line.pixel(x, y) {
        colour = c;
    }
    if let Some(c) = plan.id.pixel(x, y) {
        colour = c;
    }

    // Dismiss badge. WHY a cross and not the reference's arrow: the arrow in the
    // mock-up opens a group, and there is no group here to open. The cross is the
    // action `main.rs` actually performs on this rectangle.
    if rounded_contains(plan.badge, x, y, radius::CONTROL) {
        colour = theme.surface_raised;
        if rounded_outline(plan.badge, x, y, radius::CONTROL) {
            colour = theme.border;
        }
        let dx = x as i32 - (plan.badge.x + BADGE_W / 2) as i32;
        let dy = y as i32 - (plan.badge.y + BADGE_H / 2) as i32;
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
fn empty_deck_pixel(plan: &EmptyPlan, x: usize, y: usize, theme: &Theme) -> Option<u32> {
    if !rounded_contains(plan.rect, x, y, radius::CARD) {
        return None;
    }
    let mut colour = theme.surface_sunken;
    if rounded_outline(plan.rect, x, y, radius::CARD) {
        colour = theme.border;
    }
    if let Some(c) = plan.head.pixel(x, y) {
        colour = c;
    }
    if let Some(c) = plan.body.pixel(x, y) {
        colour = c;
    }
    Some(colour)
}
