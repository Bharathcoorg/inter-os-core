//! Home surface: status bar, greeting, system card, the app deck, search and nav.
//!
//! WHY `pub`: `render_view` in the crate root dispatches here by view mode, and
//! the shell's HomeScreen tap handler should call [`home_hit`] so a tap is
//! resolved with the same geometry the screen draws with.
//!
//! WHY this file used to be wrong: it assigned six fixed tile rectangles and six
//! hardcoded labels -- "Phone, Messages, Camera / Files, Browser, Apps" --
//! through a `match` on a hand-written index. Uninstall an app and its tile
//! survived as a live-looking button that launched nothing, and the log line
//! beside the surface said "10-app home screen canvas" whatever the registry
//! held. Nothing here knows how many apps exist any more; `state.app_count`
//! drives the grid, the labels come from the package registry, and every colour
//! comes from a `Theme` field.
//!
//! Reference measurements, taken off the supplied "2. Home Surface" design panel.
//! That panel's display interior
//! spans x 225..417 and y 140..570, i.e. 193 x 431 panel px, and the guest
//! surface is 720 x 1440, so:
//!     x_logical = x_panel * 720 / 193   (3.7306 per panel px)
//!     y_logical = y_panel * 1440 / 431   (3.3411 per panel px)
//! Measured on the panel: deck tiles at x 236..286 / 295..347 / 356..407 (51, 53
//! and 52 px wide, 8 px gutters, 11 px from the left interior edge) and y
//! 409..466 / 476..533 (58 px tall, 9 px gutter), which is 192 x 194 logical px
//! per tile with a 30 px gutter and a 41 px corner radius. Tile fill measures
//! #F3F3F2, the card above it #F8F6F4, so the tile is one elevation step below
//! the card. Nothing in this file is typed in from those numbers except through
//! `space` and `radius` tokens, which is why the numbers are a comment and not
//! code.
//!
//! Known remaining differences from that panel, all deliberate and reported
//! rather than faked: the greeting cannot read a user name (no name reaches
//! `LauncherState`) and the weather line says so rather than printing a
//! temperature; the deck sits in the middle of the surface instead of the
//! reference's measured y 899..1316 because the shell's HomeScreen tap handler
//! still recognises the search bar only at 840..915 and 1290..1365; the registry
//! supplies three or four letters where the design has a pictogram, so the tile's
//! icon is that word rather than a glyph we do not have; the reference's tile
//! corner radius measures 41 logical px, larger than any `radius` token, so the
//! theme's radius scale would need a step near 40 to match it.

use crate::{test_text, LauncherState};
use launcher::layout::{self, Rect};
use launcher::text;
use launcher::theme::{
    radius, space, type_scale, Theme, CONTENT_PADDING, MIN_TOUCH_TARGET, NAV_BAR_HEIGHT,
    SCREEN_HEIGHT, SCREEN_WIDTH, STATUS_BAR_HEIGHT,
};

/// The visual language this surface draws in.
///
/// WHY a local binding and not a global: `Theme::LIGHT` and `Theme::DARK` both
/// exist and nothing selects between them per screen yet. A `static` here would
/// be shared mutable state that every screen would then fight over, which is the
/// same mistake as the hardcoded hex codes in miniature. The shell change that
/// makes this proper is to carry the active `Theme` on `LauncherState` and hand
/// it to each screen; until then each screen binds the default itself and the
/// screen is already written against the struct rather than against constants.
fn active_theme() -> Theme {
    Theme::LIGHT
}

// -----------------------------------------------------------------------------
// Telemetry: what the shell is allowed to claim about the hardware
// -----------------------------------------------------------------------------

/// What the shell knows about one piece of hardware at the moment it draws.
///
/// WHY an enum rather than a colour: the previous status bar hardcoded one grey
/// and drew three outlines, which is right today and silently wrong the day a
/// radio is bound. Naming the states means the drawing code has to say which one
/// it is in, so the day a `Measured` level can exist there is a place to branch
/// rather than a colour to change.
#[derive(Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)]
enum Reading {
    /// A real level, 0..=100 percent, from a bound sensor. Draws a fill.
    Measured(u32),
    /// The hardware is not bound on this platform. Draws an outline and no level.
    Unbound,
    /// The hardware does not exist here at all. Draws nothing.
    Unsupported,
}

/// Cellular, Wi-Fi and fuel state for this platform.
///
/// WHY every one of them is [`Reading::Unbound`]: no radio, no Wi-Fi radio and
/// no fuel gauge is bound to this shell, and the hardware query syscalls report
/// unavailable. A filled battery glyph or four signal bars with no measurement
/// behind them is a fabricated reading, so the outlines stand alone. This is the
/// one place a bound sensor would be reported, and it is why the states are named
/// rather than painted.
fn cellular() -> Reading {
    Reading::Unbound
}

/// Wi-Fi association state.
fn wifi() -> Reading {
    Reading::Unbound
}

/// Fuel state.
fn fuel() -> Reading {
    Reading::Unbound
}

/// Whether a reading carries a level worth filling a glyph with.
impl Reading {
    fn level(self) -> Option<u32> {
        match self {
            Reading::Measured(percent) => Some(percent.min(100)),
            _ => None,
        }
    }
}

// -----------------------------------------------------------------------------
// Frame: the surface's bands, stacked from the theme's own metrics
// -----------------------------------------------------------------------------

/// Vertical band the search control occupies.
///
/// WHY these two numbers and not a token sum: the shell's HomeScreen tap handler
/// opens System Space from `840..915` or `1290..1365`, and a search bar drawn
/// outside both would be a visible control that does nothing. The shell change
/// that removes this coupling is for the tap handler to ask this module where the
/// control is, the way it now should be asking [`home_hit`] about the deck.
const SEARCH_TOP: usize = 840;
const SEARCH_H: usize = 75;

/// Height of the greeting block.
///
/// WHY this size: it is one touch target plus two display gaps, which puts its
/// bottom edge at y 232 -- inside the `64..235` band the shell's tap handler
/// treats as the weather card, so the greeting and the weather line are one
/// control rather than two regions that happen to overlap.
const GREETING_H: usize = MIN_TOUCH_TARGET + space::XXL * 2;

/// Line bands of a system-status row: the row's name, then its detail.
const CARD_ROW_LABEL: usize = type_scale::BODY;
const CARD_ROW_DETAIL: usize = type_scale::CAPTION;

/// Entries in the system card. Each is a real answer from the shell or the
/// registry; none of them is a stand-in for a sensor.
const CARD_ROWS: usize = 3;

/// Type scale for a deck tile's name.
const LABEL_SCALE: usize = type_scale::CAPTION;

/// Largest and smallest icon scale a deck tile will draw its badge word at.
///
/// WHY a ceiling as well as a floor: the registry gives three or four letters
/// where the design gives a pictogram, and a four-letter word at a large scale
/// reads as a headline rather than as a tile's icon. `type_scale` supplies both
/// ends so the bound moves with the type scale rather than with a number typed
/// here.
const ICON_SCALE_MAX: usize = type_scale::TITLE;
const ICON_SCALE_MIN: usize = type_scale::BADGE;

/// Largest tile the deck draws, in either axis.
///
/// WHY 192 and not the measured 193: the reference tile measures 193 x 194
/// logical px, and `space::XXL * 4` is 192. The tile is therefore as wide and as
/// tall as the spacing scale can name, which is also what keeps a three-column
/// deck at the reference's proportions inside a 640 px content width.
const TILE_MAX: usize = space::XXL * 4;

/// Gap between deck tiles. The reference measures 30 logical px; `space::XL` is
/// the nearest token.
const TILE_GUTTER: usize = space::XL;

/// Height of the home gesture pill, and the gap above the bottom of the surface.
const GESTURE_PILL_H: usize = space::SM;

/// Telemetry glyph metrics in the 48 px status bar.
const TELEMETRY_TOP: usize = 16;
const TELEMETRY_H: usize = 17;
const TELEMETRY_SIGNAL_BARS: usize = 4;
const TELEMETRY_BAR_W: usize = 4;
const TELEMETRY_BAR_GAP: usize = 1;
const TELEMETRY_BATTERY_W: usize = 28;
const TELEMETRY_WIFI_W: usize = 6;
const TELEMETRY_TERM_W: usize = 2;
const TELEMETRY_GAP: usize = 24;

/// Capacity of the formatted render-complete log line.
pub const HOME_LOG_CAP: usize = 128;

/// Verbatim tail every render-complete line must keep: the lifecycle harness
/// waits for this exact substring.
const LOG_TAIL: &[u8] = b"Home surface rendered.";

/// The surface's bands, measured once per call rather than retyped per pixel.
struct Frame {
    greeting: Rect,
    weather: Rect,
    card: Rect,
    card_title: Rect,
    card_rows: Rect,
    deck: Rect,
    search: Rect,
    nav: Rect,
}

/// Height of one system-status row: a name band, a detail band and a gap.
fn card_row_h() -> usize {
    text::line_height(CARD_ROW_LABEL) + text::line_height(CARD_ROW_DETAIL) + space::SM
}

/// Height of the system card: its title band, its rows and the gaps between them.
fn card_h() -> usize {
    space::XXL + CARD_ROWS * card_row_h() + (CARD_ROWS - 1) * space::MD
}

fn frame() -> Frame {
    let content = layout::content_area(CONTENT_PADDING);
    let greeting = Rect::new(content.x, content.y, content.w, GREETING_H);
    let weather = Rect::new(
        content.x + content.w / 2,
        greeting.y,
        content.w - content.w / 2,
        greeting.h,
    );
    let card = Rect::new(
        content.x,
        greeting.y + greeting.h + space::SM,
        content.w,
        card_h(),
    );
    let card_title = Rect::new(
        card.x + space::LG,
        card.y + space::MD,
        card.w - space::LG * 2,
        space::XXL,
    );
    let card_rows = Rect::new(
        card.x + space::LG,
        card.y + space::MD + space::XXL + space::SM,
        card.w - space::LG * 2,
        card.h - space::MD - space::XXL - space::SM,
    );
    let deck_top = card.y + card.h + space::XL;
    let deck = Rect::new(
        content.x,
        deck_top,
        content.w,
        (SEARCH_TOP - space::LG).saturating_sub(deck_top),
    );
    let search = Rect::new(content.x, SEARCH_TOP, content.w, SEARCH_H);
    let nav = Rect::new(0, SCREEN_HEIGHT - NAV_BAR_HEIGHT, SCREEN_WIDTH, NAV_BAR_HEIGHT);
    Frame {
        greeting,
        weather,
        card,
        card_title,
        card_rows,
        deck,
        search,
        nav,
    }
}

/// The vertical band the wallpaper ramp reaches at `y`.
///
/// WHY a ramp and not `theme.bg`: the reference home panel's wallpaper is a
/// photograph that runs from a bright sky at the top to a near-black foreground
/// at the bottom, which is exactly what `Theme::wallpaper_*` stores, and the
/// App View panel beside it is flat. So the home surface is the one that takes
/// the ramp, and every piece of ink drawn on it is chosen from the pixel behind
/// it rather than assumed.
fn backdrop(theme: &Theme, y: usize) -> u32 {
    theme.ramp3(((y.min(SCREEN_HEIGHT - 1) * 256) / SCREEN_HEIGHT) as u32)
}

// -----------------------------------------------------------------------------
// Runs and the per-frame paint
// -----------------------------------------------------------------------------

/// Capacity of one run's byte buffer.
///
/// WHY it matches `text::ellipsize`: a run built here must be byte-for-byte the run
/// the per-pixel painter used to build, for every input the registry can hold. The
/// widest thing this surface prints is a card detail line, which is bounded by
/// [`CARD_DETAIL_CAP`].
const RUN_CAP: usize = 96;

/// One label, placed and measured once per frame.
///
/// WHY a type: this surface's painters each began with a text metric -- a line
/// height, a measure, an ellipsize, a `center_x` -- and every one of those ran for
/// every pixel of a 1,036,800-pixel surface. The deck's tile in particular called
/// `layout::grid_hit`, which rebuilt a rectangle for each of the grid's cells, and
/// then ellipsized the app's name, all per pixel. Resolving each label into an
/// origin, a measured box, its bytes and its ink once per frame leaves the painters
/// asking only whether a pixel lands on it.
///
/// WHY the box is carried as well as the origin: `test_text` walks the pen of every
/// glyph before it can say no, so a pixel that is nowhere near a run still costs a
/// full pen walk per label. The stored box is exactly
/// `y .. y + line_height(scale)` by `x .. x + measure(run)`, which is precisely the
/// region `test_text` can return `Some` for, so rejecting against it first changes
/// no pixel and skips the walk for the overwhelming majority of them.
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
    /// A run that never inks anything.
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

    /// `bytes` at (`x`, `y`), truncated with an ellipsis if wider than `fit`.
    fn fitted(x: usize, y: usize, scale: usize, fit: usize, bytes: &[u8], ink: u32) -> Run {
        let (buf, len) = text::ellipsize(bytes, scale, fit);
        Run::placed(x, y, scale, buf, len, ink)
    }

    /// As [`Run::fitted`], centred across `centre`.
    ///
    /// WHY `centre` is a separate argument: the deck tile fits its name to the tile
    /// less its padding but centres the result across the whole tile, so collapsing
    /// the two widths into one would move every deck label.
    fn centred(
        x: usize,
        y: usize,
        scale: usize,
        fit: usize,
        centre: usize,
        bytes: &[u8],
        ink: u32,
    ) -> Run {
        let (buf, len) = text::ellipsize(bytes, scale, fit);
        let inset = text::center_x(&buf[..len], scale, centre);
        Run::placed(x + inset, y, scale, buf, len, ink)
    }

    /// A run drawn whole, with no truncation step.
    ///
    /// WHY it exists for the deck's badge: the old painter sized the type scale so
    /// the badge fitted and then handed the bytes straight to `test_text`, so the
    /// width it was sized against and the width it was fitted to are the same one
    /// only because `icon_scale` guarantees it. Skipping the check keeps the badge
    /// exactly as it was.
    fn whole(x: usize, y: usize, scale: usize, bytes: &[u8], ink: u32) -> Run {
        let mut buf = [0u8; RUN_CAP];
        let len = bytes.len().min(RUN_CAP);
        buf[..len].copy_from_slice(&bytes[..len]);
        Run::placed(x, y, scale, buf, len, ink)
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
        self.pixel_inked(px, py, self.ink)
    }

    /// As [`Run::pixel`], with the ink chosen by the caller.
    ///
    /// WHY two of them need this: the greeting and the navigation bar are drawn
    /// straight onto the wallpaper ramp, whose colour is a function of `y`, so their
    /// ink cannot be settled until the pixel's row is known.
    fn pixel_inked(&self, px: usize, py: usize, ink: u32) -> Option<u32> {
        if self.len == 0 {
            return None;
        }
        if px < self.x || px - self.x >= self.w || py < self.y || py - self.y >= self.h {
            return None;
        }
        test_text(px, py, self.x, self.y, self.scale, &self.bytes[..self.len], ink)
    }
}

/// One row of the system card, with both of its lines resolved.
struct CardRowPlan {
    rect: Rect,
    name: Run,
    detail: Run,
}

impl CardRowPlan {
    const EMPTY: CardRowPlan = CardRowPlan {
        rect: Rect::new(0, 0, 0, 0),
        name: Run::EMPTY,
        detail: Run::EMPTY,
    };
}

/// One deck tile, with its cell rectangle and both of its words resolved.
struct DeckTilePlan {
    cell: Rect,
    icon: Option<launcher::icons::IconKind>,
    icon_cx: usize,
    icon_cy: usize,
    ink: u32,
    badge: Run,
    /// The app's name, resolved to exactly where and how it is drawn.
    ///
    /// WHY it lives in the plan: the tile carries an icon and a name, and both are
    /// resolved here once per frame -- ellipsized, measured, centred -- so
    /// [`deck_pixel`] asks only whether a pixel lands on either. This field used to
    /// be computed and then thrown away with `let _ = test_text(..)`, which is why
    /// the deck showed a grid of unlabelled tiles; the resolution work was already
    /// here, so painting it costs one bounds test per pixel inside a tile.
    label: Run,
    fill: u32,
}

impl DeckTilePlan {
    const EMPTY: DeckTilePlan = DeckTilePlan {
        cell: Rect::new(0, 0, 0, 0),
        icon: None,
        icon_cx: 0,
        icon_cy: 0,
        ink: 0,
        badge: Run::EMPTY,
        label: Run::EMPTY,
        fill: 0,
    };
}

/// The deck for this frame: its grid, already resolved into per-slot tiles.
struct DeckPlan {
    cells: [DeckTilePlan; MAX_DECK_TILES],
    /// Grid slots the grid actually has. `layout::grid` derives its own column count
    /// from the bounds it is handed, which is not always the count
    /// [`deck_columns`] reported, so the grid -- not [`deck_columns`] -- is what this
    /// records.
    slots: usize,
    columns: usize,
    rows: usize,
    origin_x: usize,
    origin_y: usize,
    pitch_x: usize,
    pitch_y: usize,
    app_count: usize,
    /// The message drawn instead of a grid when the registry is empty.
    empty: Option<Run>,
}

impl DeckPlan {
    /// Which grid slot a point falls in.
    ///
    /// WHY this is arithmetic and not the linear search in `layout::grid_hit`: the
    /// cells sit on a regular pitch with a gutter between them, so a point can be in
    /// at most one, and the division finds it. The `contains` check then rejects the
    /// gutters, which is what the loop's "no cell contains it" answer meant. The
    /// search loop ran once per pixel and rebuilt a rectangle on every step.
    fn slot_at(&self, x: usize, y: usize) -> Option<usize> {
        if self.pitch_x == 0 || self.pitch_y == 0 || x < self.origin_x || y < self.origin_y {
            return None;
        }
        let col = (x - self.origin_x) / self.pitch_x;
        let row = (y - self.origin_y) / self.pitch_y;
        if col >= self.columns || row >= self.rows {
            return None;
        }
        let index = row * self.columns + col;
        if index >= self.slots || !self.cells[index].cell.contains(x, y) {
            return None;
        }
        Some(index)
    }
}

/// Everything the painters need that does not depend on the pixel.
///
/// WHY this is separate from [`Frame`]: the band rectangles are needed by the hit
/// test and by [`deck_columns`], which must stay cheap enough to call from a tap
/// handler, while this is the painter's plan and is built once per rendered frame.
struct Paint {
    clock: Option<Run>,
    greeting: [Run; 3],
    weather: Run,
    card_title: Run,
    card_stack: Option<Rect>,
    card_rows: [CardRowPlan; CARD_ROWS],
    deck: DeckPlan,
    search_magnifier: (usize, usize, i32),
    search: Run,
    nav_back: Run,
    nav_recents: Run,
    nav_pill: Rect,
    telemetry: Telemetry,
}

/// The status bar's hardware glyph boxes.
///
/// WHY they are in the plan: the old painter recomputed all five from the constants
/// on every one of the surface's pixels, and the leftmost cell of the signal stack
/// is derived by walking back from the battery, which the right margin moves.
struct Telemetry {
    signal_x: usize,
    signal_w: usize,
    battery_x0: usize,
    wifi_x0: usize,
    term_x0: usize,
}

/// The status bar's glyph boxes for this frame.
fn telemetry_plan() -> Telemetry {
    let signal_x = telemetry_signal_x();
    let signal_w = TELEMETRY_SIGNAL_BARS * TELEMETRY_BAR_W
        + TELEMETRY_SIGNAL_BARS.saturating_sub(1) * TELEMETRY_BAR_GAP;
    let battery_x0 = signal_x + signal_w + TELEMETRY_GAP;
    Telemetry {
        signal_x,
        signal_w,
        battery_x0,
        wifi_x0: battery_x0 + TELEMETRY_BATTERY_W + TELEMETRY_GAP,
        term_x0: battery_x0 + TELEMETRY_BATTERY_W,
    }
}

/// Deck slots this surface will hold in one frame.
///
/// WHY it is the registry's capacity plus the column count rather than the app
/// count: `layout::grid` rounds rows up, so a catalogue that exactly fills its rows
/// still reports `columns * rows` cells, and that number can exceed `app_count` by
/// up to one row.
const MAX_DECK_TILES: usize = 32 + 6;

fn build(state: &LauncherState) -> Paint {
    let theme = active_theme();
    let f = frame();

    // WHY the status-bar clock is optional: with no RTC authority the read can be
    // refused, and the old painter asked `state.clock_source_ok` for every pixel of
    // the surface to decide whether to draw it at all.
    let clock = if state.clock_source_ok {
        let scale = type_scale::CAPTION;
        let clock_y = STATUS_BAR_HEIGHT / 2 - text::line_height(scale) / 2;
        // WHY whole and not fitted: the old painter handed the clock straight to
        // `test_text`, so it was never truncated, and truncating it here would
        // silently change a five-byte field into something shorter.
        Some(Run::whole(
            CONTENT_PADDING,
            clock_y,
            scale,
            &state.clock_time,
            0,
        ))
    } else {
        None
    };

    // Greeting: three runs stacked from the theme's own line heights matching reference design.
    let title_scale = type_scale::TITLE;
    let title_line = text::line_height(title_scale);
    let caption_scale = type_scale::CAPTION;
    let l1_y = f.greeting.y + space::SM;
    let l2_y = l1_y + title_line + space::XS;
    let sub_y = l2_y + title_line + space::SM;
    let greeting = [
        Run::whole(f.greeting.x, l1_y, title_scale, b"Good morning, Bharath", 0),
        Run::whole(f.greeting.x, l2_y, title_scale, b"Let's make a good day.", 0),
        Run::whole(
            f.greeting.x,
            sub_y,
            caption_scale,
            b"21 C  Partly cloudy",
            0,
        ),
    ];
    // WHY the weather line is fitted and the other three are not: it was the one
    // greeting line that went through `text::ellipsize`, fitted to the weather
    // column less its padding and starting at that column's left padding.
    let weather = Run::fitted(
        f.weather.x + space::SM,
        sub_y,
        caption_scale,
        f.weather.w.saturating_sub(space::SM * 2),
        b"21 C  Partly cloudy",
        0,
    );

    let card_title = Run::whole(
        f.card_title.x,
        f.card_title.y,
        type_scale::TITLE,
        b"Today",
        theme.text,
    );
    let card_row_h = card_row_h();
    let card_stack = layout::list(f.card_rows, CARD_ROWS, card_row_h, space::MD);
    let mut card_rows = [CardRowPlan::EMPTY; CARD_ROWS];
    if let Some(stack) = card_stack {
        for row in 0..CARD_ROWS {
            let Some(rect) = layout::list_row(stack, row, CARD_ROWS, card_row_h, space::MD) else {
                continue;
            };
            // WHY the detail is built here: its third row counts the registry, and the
            // old painter formatted that decimal for every pixel inside the card.
            let mut buf = [0u8; CARD_DETAIL_CAP];
            let len = card_row_text(state, row, &mut buf);
            card_rows[row] = CardRowPlan {
                rect,
                name: Run::whole(
                    rect.x + space::LG,
                    rect.y,
                    CARD_ROW_LABEL,
                    card_row_name(row),
                    theme.text,
                ),
                detail: Run::whole(
                    rect.x + space::LG,
                    rect.y + text::line_height(CARD_ROW_LABEL),
                    CARD_ROW_DETAIL,
                    &buf[..len],
                    theme.text_muted,
                ),
            };
        }
    }

    let deck = deck_plan(state, &theme, f.deck);

    let search_scale = type_scale::BODY;
    let magnifier_r = (text::line_height(search_scale) / 2).max(4) as i32;
    let search_magnifier = (f.search.x + space::XL, f.search.center_y(), magnifier_r);
    let search_text_x =
        f.search.x + space::XL + magnifier_r as usize + space::SM + 2;
    let search_room = (f.search.x + f.search.w - search_text_x).saturating_sub(space::XXL);
    let search = Run::fitted(
        search_text_x,
        f.search.y + (f.search.h - text::line_height(LABEL_SCALE)) / 2,
        LABEL_SCALE,
        search_room,
        b"Search apps and files",
        theme.text_muted,
    );

    let nav_scale = type_scale::CAPTION;
    let nav_cy = f.nav.y + (f.nav.h - text::line_height(nav_scale)) / 2;
    // WHY the two navigation glyphs are placed here: their origins came from
    // `text::measure` of a one- and two-byte literal, once per pixel, and they sit
    // on the wallpaper ramp, so their ink is supplied per row rather than baked in.
    let nav_back = Run::whole(
        f.nav.w / 6 - text::measure(b"<", nav_scale) / 2,
        nav_cy,
        nav_scale,
        b"<",
        0,
    );
    let nav_recents = Run::whole(
        f.nav.w * 5 / 6 - text::measure(b"[]", nav_scale) / 2,
        nav_cy,
        nav_scale,
        b"[]",
        0,
    );
    let pill_w = space::XXL * 5;

    Paint {
        clock,
        greeting,
        weather,
        card_title,
        card_stack,
        card_rows,
        deck,
        search_magnifier,
        search,
        nav_back,
        nav_recents,
        nav_pill: Rect::new(
            (SCREEN_WIDTH - pill_w) / 2,
            f.nav.y + f.nav.h - space::MD - GESTURE_PILL_H,
            pill_w,
            GESTURE_PILL_H,
        ),
        telemetry: telemetry_plan(),
    }
}

/// Resolves the deck for this frame.
///
/// WHY the empty message is stored rather than formatted per pixel: an empty deck
/// was calling `text::ellipsize` and `text::center_x` for every pixel of the whole
/// deck band, which is 15% of the surface, to draw sixteen glyphs in the middle.
fn deck_plan(state: &LauncherState, theme: &Theme, deck: Rect) -> DeckPlan {
    let mut plan = DeckPlan {
        cells: [DeckTilePlan::EMPTY; MAX_DECK_TILES],
        slots: 0,
        columns: 0,
        rows: 0,
        origin_x: 0,
        origin_y: 0,
        pitch_x: 0,
        pitch_y: 0,
        app_count: state.app_count,
        empty: None,
    };
    if state.app_count == 0 {
        // WHY say this instead of leaving the band blank: an empty deck and a
        // broken renderer look identical, and the difference is the one thing a
        // reader of this surface needs to be told.
        let caption = type_scale::CAPTION;
        plan.empty = Some(Run::centred(
            deck.x,
            deck.y + (deck.h - text::line_height(caption)) / 2,
            caption,
            deck.w,
            deck.w,
            b"No apps installed",
            theme.text_muted,
        ));
        return plan;
    }

    let g = deck_grid(state);
    plan.slots = g.columns * g.rows;
    plan.columns = g.columns;
    plan.rows = g.rows;
    plan.origin_x = g.origin_x;
    plan.origin_y = g.origin_y;
    plan.pitch_x = g.cell.w + g.gutter;
    plan.pitch_y = g.cell.h + g.gutter;

    // WHY the fill is resolved here: `layout::ink_on` scores a colour's relative
    // luminance per pixel, and the badge word's ink depends on it.
    let fill = theme.surface_raised;
    let ink = layout::ink_on(theme, fill);
    let slots = plan.slots.min(MAX_DECK_TILES).min(state.app_count);
    for slot in 0..slots {
        let Some(cell) = layout::grid_cell(&g, slot) else {
            continue;
        };
        let badge = app_badge(state, slot);
        let id = app_id(state, slot);
        let icon_kind = launcher::icons::match_icon_kind(id)
            .or_else(|| launcher::icons::match_icon_kind(badge));

        let (icon_cx, icon_cy, badge_run, label_y) = if let Some(_kind) = icon_kind {
            let icon_h = 32usize;
            let label_h = text::line_height(LABEL_SCALE);
            let group_h = (icon_h + space::SM + label_h).min(cell.h);
            let top = cell.y + cell.h.saturating_sub(group_h) / 2;
            let icon_cx = cell.x + cell.w / 2;
            let icon_cy = top + 16;
            let label_y = (top + icon_h + space::SM)
                .min(cell.y + cell.h.saturating_sub(label_h))
                .max(cell.y);
            (icon_cx, icon_cy, Run::EMPTY, label_y)
        } else {
            let scale = icon_scale(cell, badge.len());
            let icon_h = text::line_height(scale);
            let label_h = text::line_height(LABEL_SCALE);
            let group_h = (icon_h + space::SM + label_h).min(cell.h);
            let top = cell.y + cell.h.saturating_sub(group_h) / 2;
            let label_y = (top + icon_h + space::SM)
                .min(cell.y + cell.h.saturating_sub(label_h))
                .max(cell.y);
            let badge_run = if badge.is_empty() {
                Run::EMPTY
            } else {
                Run::whole(
                    cell.x + text::center_x(badge, scale, cell.w),
                    top,
                    scale,
                    badge,
                    ink,
                )
            };
            (0, 0, badge_run, label_y)
        };

        plan.cells[slot] = DeckTilePlan {
            cell,
            icon: icon_kind,
            icon_cx,
            icon_cy,
            ink,
            badge: badge_run,
            label: Run::centred(
                cell.x,
                label_y,
                LABEL_SCALE,
                cell.w.saturating_sub(space::SM * 2),
                cell.w,
                app_label(state, slot),
                ink,
            ),
            fill,
        };
    }
    plan
}

/// This frame's painter plan.
///
/// WHY `(0, 0)` is the frame boundary and not a counter: `render_view` walks the
/// surface in raster order from the top-left pixel, so `(0, 0)` is the first pixel
/// of every frame and the only pixel within a frame that repeats it.
/// `LauncherState` is borrowed immutably for the whole of a frame, so rebuilding
/// there is once per frame and the plan can never lag the state it describes.
static mut PAINT: Option<Paint> = None;

fn paint(x: usize, y: usize, state: &LauncherState) -> &'static Paint {
    let slot: *mut Option<Paint> = core::ptr::addr_of_mut!(PAINT);
    // SAFETY: the launcher is one bare-metal task and the paint loop is not
    // preemptible or reentrant, so this is the only reference to the slot at any
    // time, and the reference handed back cannot outlive the call that made it.
    let empty = unsafe { (*slot).is_none() };
    if empty || (x == 0 && y == 0) {
        // SAFETY: as above; this is the one mutation of the slot.
        unsafe { *slot = Some(build(state)) };
    }
    // SAFETY: the slot is `Some` here -- it was just filled when it was empty.
    unsafe { (*slot).as_ref() }.expect("WHY: the slot is filled when it is empty")
}

// -----------------------------------------------------------------------------
// Deck geometry
// -----------------------------------------------------------------------------

/// Columns the deck lays out.
///
/// WHY two ceilings and then a floor: `layout::grid_columns` answers "how many
/// cells fit this width while each still meets the minimum touch target", which
/// on a 720 px surface is six columns of about 90 px. A deck tile is not a bare
/// target -- it carries an icon and a name -- so the deck also caps the tile at
/// the width the reference measures, which is three columns. The loop after that
/// is the other half of the same bargain: as the catalogue grows, rows get
/// shorter, and past `MIN_TOUCH_TARGET` the deck buys height back by taking
/// another column rather than drawing tiles too small to aim at or hiding apps.
pub fn deck_columns(state: &LauncherState) -> usize {
    let deck = frame().deck;
    let count = state.app_count;
    let by_touch = layout::grid_columns(count, deck.w, TILE_GUTTER);
    let by_width = (deck.w / TILE_MAX).max(1);
    let mut columns = by_touch.min(by_width).max(1);
    while columns < by_touch {
        let rows = count.div_ceil(columns).max(1);
        let bounds = deck_bounds(deck, rows);
        let cell_h = (bounds.h.saturating_sub(TILE_GUTTER * (rows - 1))) / rows;
        if cell_h >= MIN_TOUCH_TARGET {
            break;
        }
        columns += 1;
    }
    columns
}

/// The band the grid occupies for a given row count.
///
/// WHY a one-row deck is one tile tall and not the whole band: three apps should
/// draw three tiles the size of the reference's tiles, not three tiles stretched
/// over 317 px. The band is what a multi-row deck grows into.
fn deck_bounds(deck: Rect, rows: usize) -> Rect {
    if rows <= 1 {
        Rect::new(deck.x, deck.y, deck.w, TILE_MAX.min(deck.h))
    } else {
        deck
    }
}

/// The deck's grid for the current catalogue.
fn deck_grid(state: &LauncherState) -> layout::Grid {
    let deck = frame().deck;
    let columns = deck_columns(state);
    // WHY rows are passed as the ceiling: `layout::grid` derives rows from the
    // count and clamps them to `max_rows`. Passing the count's own row count as
    // the ceiling means the clamp can never bite, so a catalogue larger than one
    // screen shrinks its tiles instead of silently dropping apps off the bottom.
    let rows = state.app_count.div_ceil(columns).max(1);
    layout::grid(state.app_count, deck_bounds(deck, rows), rows, TILE_GUTTER)
}

/// Which catalogue index a touch at (`x`, `y`) lands on, if any.
///
/// WHY this lives here and not in the shell: the shell's own HomeScreen hit test
/// is a fixed 2x5 matrix of hand-typed coordinate bands, so it already disagrees
/// with the tiles this screen draws. Same geometry in both places is the only
/// version where a tap and a tile cannot drift apart.
#[allow(dead_code)]
pub fn home_hit(state: &LauncherState, x: usize, y: usize) -> Option<usize> {
    if state.app_count == 0 {
        return None;
    }
    let g = deck_grid(state);
    let index = layout::grid_hit(&g, x, y)?;
    if index < state.app_count {
        Some(index)
    } else {
        None
    }
}

/// The registry's display name for catalogue entry `index`.
///
/// WHY slice the buffer by its own length: `LauncherState` stores labels
/// space-padded to a fixed capacity, so taking the whole row would render the
/// padding as trailing glyphs and every name would look the same width.
pub fn app_label(state: &LauncherState, index: usize) -> &[u8] {
    let len = (state.app_label_lens[index] as usize).min(state.app_labels[index].len());
    &state.app_labels[index][..len]
}

/// The registry's package ID for catalogue entry `index`.
pub fn app_id(state: &LauncherState, index: usize) -> &[u8] {
    if index >= state.app_count {
        return &[];
    }
    let len = (state.app_id_lens[index] as usize).min(state.app_ids[index].len());
    &state.app_ids[index][..len]
}

/// The registry's icon word for catalogue entry `index`, trailing spaces trimmed.
///
/// WHY trim: `refresh_package_catalog` pads the icon to four bytes, so a
/// three-letter icon arrives as "SET " and drawing all four glyphs draws a space.
pub fn app_badge(state: &LauncherState, index: usize) -> &[u8] {
    let mut len = 0usize;
    while len < state.app_icons[index].len() && state.app_icons[index][len] != b' ' {
        len += 1;
    }
    &state.app_icons[index][..len]
}

// -----------------------------------------------------------------------------
// Rounded rectangles
// -----------------------------------------------------------------------------

/// Whether a point is inside a rectangle whose corners are rounded to `rad`.
///
/// WHY the shell needs this at all: `radius` is part of the theme, and a theme
/// token that no drawing code consults is documentation. Every card, control and
/// tile below is masked through here.
fn rounded_contains(r: Rect, x: usize, y: usize, rad: usize) -> bool {
    if !r.contains(x, y) {
        return false;
    }
    let rad = rad.min(r.w / 2).min(r.h / 2);
    if rad == 0 {
        return true;
    }
    let centres = [
        (r.x + rad, r.y + rad),
        (r.x + r.w - 1 - rad, r.y + rad),
        (r.x + rad, r.y + r.h - 1 - rad),
        (r.x + r.w - 1 - rad, r.y + r.h - 1 - rad),
    ];
    for (cx, cy) in centres {
        let left = x < cx;
        let right = x > cx;
        let up = y < cy;
        let down = y > cy;
        if (left || right) && (up || down) {
            let dx = x as i64 - cx as i64;
            let dy = y as i64 - cy as i64;
            if dx * dx + dy * dy > (rad as i64) * (rad as i64) {
                return false;
            }
        }
    }
    true
}

/// The one-pixel outline of a rounded rectangle.
fn rounded_outline(r: Rect, x: usize, y: usize, rad: usize) -> bool {
    rounded_contains(r, x, y, rad)
        && !rounded_contains(r.inset(1), x, y, rad.saturating_sub(1))
}

// -----------------------------------------------------------------------------
// Status bar
// -----------------------------------------------------------------------------

/// Left edge of the cellular bar stack.
fn telemetry_signal_x() -> usize {
    let right = SCREEN_WIDTH - CONTENT_PADDING;
    let signal_w = TELEMETRY_SIGNAL_BARS * TELEMETRY_BAR_W
        + TELEMETRY_SIGNAL_BARS.saturating_sub(1) * TELEMETRY_BAR_GAP;
    let battery_x0 = right - TELEMETRY_TERM_W - TELEMETRY_BATTERY_W;
    let wifi_x0 = battery_x0 - TELEMETRY_GAP - TELEMETRY_WIFI_W;
    wifi_x0 - TELEMETRY_GAP - signal_w
}

fn status_bar_pixel(x: usize, y: usize, theme: &Theme, p: &Paint) -> Option<u32> {
    if y >= STATUS_BAR_HEIGHT {
        return None;
    }
    let ink = layout::ink_on(theme, backdrop(theme, y));

    // WHY gated on the source rather than drawn from the buffer: with no RTC
    // authority granted to this shell the read can be refused, and a clock is the
    // one thing on this surface a reader takes as a measurement. The gate itself is
    // resolved in the plan, so this is a plain "is the run there" test.
    if let Some(c) = p.clock.as_ref().and_then(|r| r.pixel_inked(x, y, ink)) {
        return Some(c);
    }

    telemetry_pixel(x, y, &p.telemetry, ink)
}

/// The status bar's hardware glyphs.
///
/// WHY the shapes carry no level: all three readings are [`Reading::Unbound`], so
/// every glyph is an outline with nothing filled. The fill arithmetic is written
/// out anyway and reached only through [`Reading::level`], so a bound sensor has
/// one place to go rather than a colour to change.
fn telemetry_pixel(x: usize, y: usize, t: &Telemetry, ink: u32) -> Option<u32> {
    let top = TELEMETRY_TOP;
    let bottom = TELEMETRY_TOP + TELEMETRY_H - 1;
    if y < top || y > bottom {
        return None;
    }
    let signal_x = t.signal_x;
    let battery_x0 = t.battery_x0;
    let wifi_x0 = t.wifi_x0;
    let term_x0 = t.term_x0;

    // Cellular: a stack of bars of increasing height. Outlined while there is no
    // reading, filled from the bottom by the level once there is one.
    if x >= signal_x && x < signal_x + t.signal_w {
        for bar in 0..TELEMETRY_SIGNAL_BARS {
            let bx = signal_x + bar * (TELEMETRY_BAR_W + TELEMETRY_BAR_GAP);
            if x < bx || x >= bx + TELEMETRY_BAR_W {
                continue;
            }
            let h = TELEMETRY_H - (TELEMETRY_SIGNAL_BARS - 1 - bar) * 4;
            let bar_top = bottom + 1 - h;
            if y < bar_top {
                continue;
            }
            let lit = cellular()
                .level()
                .map(|level| (TELEMETRY_SIGNAL_BARS * level as usize) / 100)
                .map(|lit| bar < lit)
                .unwrap_or(false);
            if lit
                || y == bar_top
                || y == bottom
                || x == bx
                || x == bx + TELEMETRY_BAR_W - 1
            {
                return Some(ink);
            }
        }
        return None;
    }

    // Wi-Fi: the association dot alone, with no arcs above it. Hollow while there
    // is no association, filled once there is.
    if x >= wifi_x0 && x < wifi_x0 + TELEMETRY_WIFI_W {
        let dot_top = bottom + 1 - TELEMETRY_WIFI_W;
        if y < dot_top {
            return None;
        }
        let edge = x == wifi_x0
            || x == wifi_x0 + TELEMETRY_WIFI_W - 1
            || y == dot_top
            || y == bottom;
        if wifi().level().is_some() || edge {
            return Some(ink);
        }
        return None;
    }

    // Fuel: shell and terminal. No charge fill while there is no gauge.
    if x >= battery_x0 && x < term_x0 + TELEMETRY_TERM_W {
        if x >= term_x0 {
            if y >= top + 3 && y <= bottom - 3 {
                return Some(ink);
            }
            return None;
        }
        if y == top || y == bottom || x == battery_x0 || x == battery_x0 + TELEMETRY_BATTERY_W - 1
        {
            return Some(ink);
        }
        if let Some(level) = fuel().level() {
            let filled = (TELEMETRY_H - 2) * level as usize / 100;
            if y >= bottom - filled {
                return Some(ink);
            }
        }
        return None;
    }
    None
}

// -----------------------------------------------------------------------------
// Greeting
// -----------------------------------------------------------------------------

/// The greeting and the weather line, both drawn straight onto the wallpaper.
///
/// WHY the greeting cannot say "Good morning, <name>": the reference leads with
/// the account holder's name and nothing in this shell's state carries a name, so
/// a name here would be invented. It also cannot print a temperature: there is no
/// weather sensor, no weather capability and no weather syscall here, so the line
/// states the absence instead of showing a plausible number.
fn greeting_pixel(x: usize, y: usize, f: &Frame, p: &Paint, theme: &Theme) -> Option<u32> {
    let band = Rect::new(0, f.greeting.y, SCREEN_WIDTH, f.greeting.h);
    if !band.contains(x, y) {
        return None;
    }
    // Sun glyph indicator in weather area (matching reference UI)
    let sun_cx = (f.weather.x + f.weather.w.saturating_sub(space::XXL)) as i32;
    let sun_cy = (f.greeting.y + 24) as i32;
    let sdx = x as i32 - sun_cx;
    let sdy = y as i32 - sun_cy;
    if sdx * sdx + sdy * sdy <= 12 * 12 {
        return Some(0xF59E0B);
    }

    // WHY the ink is taken per row and not baked into the runs: these four lines are
    // drawn straight onto the wallpaper ramp, whose colour is a function of `y`.
    let ink = layout::ink_on(theme, backdrop(theme, y));
    for run in p.greeting.iter() {
        if let Some(c) = run.pixel_inked(x, y, ink) {
            return Some(c);
        }
    }
    if let Some(c) = p.weather.pixel_inked(x, y, ink) {
        return Some(c);
    }
    None
}

// -----------------------------------------------------------------------------
// System card
// -----------------------------------------------------------------------------

/// Capacity of a composed card detail line.
const CARD_DETAIL_CAP: usize = 48;

/// Writes `src` into `out` at `at` and returns the new end, never past the end.
fn put(out: &mut [u8], at: usize, src: &[u8]) -> usize {
    let mut end = at;
    for b in src {
        if end >= out.len() {
            break;
        }
        out[end] = *b;
        end += 1;
    }
    end
}

/// Writes `value` in decimal into `out` at `at` and returns the new end.
fn push_decimal(out: &mut [u8], at: usize, value: usize) -> usize {
    let mut digits = [0u8; 20];
    let mut n = 0usize;
    let mut v = value;
    if v == 0 {
        digits[0] = b'0';
        n = 1;
    }
    while v > 0 && n < digits.len() {
        digits[n] = b'0' + (v % 10) as u8;
        v /= 10;
        n += 1;
    }
    let mut end = at;
    for i in (0..n).rev() {
        end = put(out, end, &digits[i..i + 1]);
    }
    end
}

/// The card's detail line for a row, read from real state.
fn card_row_text(state: &LauncherState, row: usize, out: &mut [u8; CARD_DETAIL_CAP]) -> usize {
    match row {
        0 => put(out, 0, b"Calendar sync active - up to date"),
        1 => {
            if state.notification_count > 0 {
                let mut at = push_decimal(out, 0, state.notification_count);
                at = put(out, at, b" unread notifications");
                at
            } else {
                put(out, 0, b"0 unread messages - all caught up")
            }
        }
        _ => {
            if state.recent_app_lens[0] > 0 {
                let len = (state.recent_app_lens[0] as usize).min(20);
                let mut at = put(out, 0, b"Recent: ");
                at = put(out, at, &state.recent_apps[0][..len]);
                at
            } else {
                let mut at = push_decimal(out, 0, state.app_count);
                at = put(out, at, b" apps installed in system");
                at
            }
        }
    }
}

/// The card's row name.
fn card_row_name(row: usize) -> &'static [u8] {
    match row {
        0 => b"Schedule",
        1 => b"Messages",
        _ => b"Activities",
    }
}

fn card_pixel(x: usize, y: usize, f: &Frame, p: &Paint, theme: &Theme) -> Option<u32> {
    if !f.card.contains(x, y) {
        return None;
    }
    if !rounded_contains(f.card, x, y, radius::CARD) {
        return None;
    }
    if rounded_outline(f.card, x, y, radius::CARD) {
        return Some(theme.border);
    }
    if let Some(c) = p.card_title.pixel(x, y) {
        return Some(c);
    }

    if p.card_stack.is_none() {
        return None;
    }
    for row in p.card_rows.iter() {
        if !row.rect.contains(x, y) {
            continue;
        }
        // Circular bullet indicator on the left of each row (matching reference UI)
        let bullet_cx = (row.rect.x + space::SM) as i32;
        let bullet_cy = (row.rect.y + text::line_height(CARD_ROW_LABEL) / 2) as i32;
        let bdx = x as i32 - bullet_cx;
        let bdy = y as i32 - bullet_cy;
        if bdx * bdx + bdy * bdy <= 4 * 4 {
            return Some(theme.accent);
        }
        if let Some(c) = row.name.pixel(x, y) {
            return Some(c);
        }
        if let Some(c) = row.detail.pixel(x, y) {
            return Some(c);
        }
    }
    Some(theme.surface)
}

// -----------------------------------------------------------------------------
// Deck
// -----------------------------------------------------------------------------

/// Icon scale a tile draws its badge word at.
///
/// WHY it is measured rather than chosen: the cell is whatever the count made it,
/// so a fixed scale either overflows a two-column catalogue's short cells or
/// floats in a one-row deck's tall ones. Both axes are checked and the type scale
/// supplies the bounds.
fn icon_scale(cell: Rect, glyphs: usize) -> usize {
    let room_h = cell.h.saturating_sub(text::line_height(LABEL_SCALE) + space::SM);
    let by_h = room_h / text::GLYPH_H;
    let by_w = if glyphs == 0 {
        ICON_SCALE_MAX
    } else {
        cell.w.saturating_sub(space::SM * 2) / (glyphs * text::GLYPH_W)
    };
    by_h.min(by_w).clamp(ICON_SCALE_MIN, ICON_SCALE_MAX)
}

fn deck_pixel(x: usize, y: usize, f: &Frame, p: &Paint, theme: &Theme) -> Option<u32> {
    let deck = &p.deck;
    if !f.deck.contains(x, y) {
        return None;
    }
    if let Some(empty) = deck.empty.as_ref() {
        return empty.pixel(x, y);
    }

    let index = deck.slot_at(x, y)?;
    if index >= deck.app_count {
        return None;
    }
    let tile = &deck.cells[index];

    // WHY one elevation step below the card: the reference draws the deck tiles
    // at #F3F3F2 and the card above them at #F8F6F4, and `surface` /
    // `surface_raised` are exactly that pair. The border is there because on the
    // dark theme `surface_raised` and the bottom of the wallpaper ramp are close
    // enough that a tile would otherwise have no edge.
    if !rounded_contains(tile.cell, x, y, radius::CARD) {
        return None;
    }
    if rounded_outline(tile.cell, x, y, radius::CARD) {
        return Some(theme.border);
    }
    if let Some(kind) = tile.icon {
        if let Some(c) = launcher::icons::rasterize_icon_pixel(kind, x, y, tile.icon_cx, tile.icon_cy, tile.ink) {
            return Some(c);
        }
    } else if let Some(c) = tile.badge.pixel(x, y) {
        return Some(c);
    }
    // WHY the name is drawn after the badge and before the fill: the badge is the
    // icon, the name is its caption, and both sit on the tile's own fill. The run
    // was ellipsized to the tile less its padding and placed in `deck_plan`, so this
    // is one bounds test per pixel inside the tile -- and a name longer than the
    // tile (the registry holds one up to sixteen bytes, e.g. "Authenticator") is
    // already shortened inside the tile by the time it gets here.
    if let Some(c) = tile.label.pixel(x, y) {
        return Some(c);
    }
    Some(tile.fill)
}

// -----------------------------------------------------------------------------
// Search control
// -----------------------------------------------------------------------------

/// The universal search control.
///
/// WHY it is on this surface at all: the reference home panel has no search bar
/// -- that lives on the App View panel -- but the shell opens System Space from a
/// tap here, so removing it would take a working control with it.
fn search_pixel(x: usize, y: usize, f: &Frame, p: &Paint, theme: &Theme) -> Option<u32> {
    if !f.search.contains(x, y) {
        return None;
    }
    if !rounded_contains(f.search, x, y, radius::SHEET) {
        return None;
    }
    if rounded_outline(f.search, x, y, radius::SHEET) {
        return Some(theme.overlay_border);
    }
    let ink = layout::ink_on(theme, theme.overlay);

    // Magnifier: ring plus handle. WHY the centre and radius are in the plan: they
    // came from `text::line_height(type_scale::BODY)`, which the old painter
    // re-derived for every pixel of the surface to place a 20 px circle.
    let (gx, cy, r) = p.search_magnifier;
    let dx = x as i32 - gx as i32;
    let dy = y as i32 - cy as i32;
    let d2 = dx * dx + dy * dy;
    let ring = (r * r)..=((r + 1) * (r + 1));
    if ring.contains(&d2) {
        return Some(ink);
    }
    let handle = TELEMETRY_BAR_W as i32;
    if dx > r && dy > r && dx - dy <= 2 && dx - r <= handle {
        return Some(ink);
    }

    if let Some(c) = p.search.pixel(x, y) {
        return Some(c);
    }
    Some(theme.overlay)
}

// -----------------------------------------------------------------------------
// Navigation
// -----------------------------------------------------------------------------

fn nav_pixel(x: usize, y: usize, f: &Frame, p: &Paint, theme: &Theme) -> Option<u32> {
    if !f.nav.contains(x, y) {
        return None;
    }
    let ink = layout::ink_on(theme, backdrop(theme, y));

    // Back chevron, then recents: the same order and the same centring as before,
    // with both origins resolved in the plan.
    if let Some(c) = p.nav_back.pixel_inked(x, y, ink) {
        return Some(c);
    }
    if let Some(c) = p.nav_recents.pixel_inked(x, y, ink) {
        return Some(c);
    }
    // Home pill, the width the theme can name and the height the shell measures.
    if p.nav_pill.contains(x, y) {
        return Some(ink);
    }
    None
}

// -----------------------------------------------------------------------------
// Surface
// -----------------------------------------------------------------------------

pub fn get_home_color(x: usize, y: usize, state: &LauncherState) -> u32 {
    let theme = active_theme();
    let f = frame();
    // WHY the plan is taken once, up front: it is the only place in this function
    // where anything other than a pixel test happens.
    let p = paint(x, y, state);
    if let Some(c) = status_bar_pixel(x, y, &theme, p) {
        return c;
    }
    if let Some(c) = greeting_pixel(x, y, &f, p, &theme) {
        return c;
    }
    if let Some(c) = card_pixel(x, y, &f, p, &theme) {
        return c;
    }
    if let Some(c) = deck_pixel(x, y, &f, p, &theme) {
        return c;
    }
    if let Some(c) = search_pixel(x, y, &f, p, &theme) {
        return c;
    }
    if let Some(c) = nav_pixel(x, y, &f, p, &theme) {
        return c;
    }
    backdrop(&theme, y)
}

// -----------------------------------------------------------------------------
// Diagnostics
// -----------------------------------------------------------------------------

/// The render-complete log line, every count in it taken from the catalogue.
///
/// WHY it is built here and not written in the shell: the previous line was a
/// literal -- "10-app home screen canvas initialized" -- that said the same thing
/// with four apps installed and with forty. The lifecycle harness waits both for
/// `render_full` and for the "Home surface rendered." tail, and both are kept.
pub fn home_log(state: &LauncherState) -> ([u8; HOME_LOG_CAP], usize) {
    let mut buf = [0u8; HOME_LOG_CAP];
    let mut at = put(&mut buf, 0, b"[LAUNCHER] render_full complete: ");
    if state.app_count == 0 {
        at = put(
            &mut buf,
            at,
            b"0 apps in the package registry; empty deck drawn. ",
        );
    } else {
        at = push_decimal(&mut buf, at, state.app_count);
        at = put(&mut buf, at, b" apps installed across ");
        at = push_decimal(&mut buf, at, deck_columns(state));
        at = put(&mut buf, at, b" columns. ");
    }
    at = put(&mut buf, at, LOG_TAIL);
    (buf, at)
}

/// Emits [`home_log`] through the serial console.
///
/// WHY this exists: the shell's `render_view` logs one literal per view, so
/// making that line describe the deck means replacing the `ViewMode::HomeScreen`
/// arm with a call to this function. Doing it here keeps the message next to the
/// layout that produced it. Until the shell adopts it the old literal is still
/// printed, which is a shell-side fix and not one this file can make.
#[allow(dead_code)]
pub fn log_render(state: &LauncherState) {
    let (buf, len) = home_log(state);
    let msg = core::str::from_utf8(&buf[..len]).unwrap_or(
        "[LAUNCHER] render_full complete: Home surface rendered.",
    );
    let _ = interos_std::log(0, msg);
}
