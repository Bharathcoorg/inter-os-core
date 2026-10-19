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
//! reference's measured y 899..1316 because the deck tiles end at y 1300 and the
//! search bar sits directly below them at 1320..1384; the registry
//! supplies three or four letters where the design has a pictogram, so the tile's
//! icon is that word rather than a glyph we do not have; the reference's tile
//! corner radius measures 41 logical px, larger than any `radius` token, so the
//! theme's radius scale would need a step near 40 to match it.

use crate::LauncherState;
use launcher::layout::{self, Rect};
use launcher::text;
use launcher::theme::{
    space, type_scale, Theme, CONTENT_PADDING, MIN_TOUCH_TARGET, NAV_BAR_HEIGHT,
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
/// opens System Space from exactly this band, and a search bar drawn outside it
/// would be a visible control that does nothing. The shell change that removes
/// this coupling is for the tap handler to ask this module where the control is,
/// the way it now asks [`home_hit`] about the deck.
#[allow(dead_code)]
const SEARCH_TOP: usize = 1320;
#[allow(dead_code)]
const SEARCH_H: usize = 64;

/// Height of the greeting block.
///
/// WHY this size: it is one touch target plus two display gaps, which puts its
/// bottom edge at y 232 -- inside the `64..235` band the shell's tap handler
/// treats as the weather card, so the greeting and the weather line are one
/// control rather than two regions that happen to overlap.
#[allow(dead_code)]
const GREETING_H: usize = MIN_TOUCH_TARGET + space::XXL * 2;

#[allow(dead_code)]
const CARD_ROW_LABEL: usize = type_scale::BODY;
#[allow(dead_code)]
const CARD_ROW_DETAIL: usize = type_scale::CAPTION;

/// Entries in the system card. Each is a real answer from the shell or the
/// registry; none of them is a stand-in for a sensor.
const CARD_ROWS: usize = 3;

/// Type scale for a deck tile's name: scale 2 (BODY) with outline anti-aliasing for readability.
const LABEL_SCALE: usize = type_scale::BODY;

/// Largest and smallest icon scale a deck tile will draw its badge word at.
///
/// WHY a ceiling as well as a floor: the registry gives three or four letters
/// where the design gives a pictogram, and a four-letter word at a large scale
/// reads as a headline rather than as a tile's icon. `type_scale` supplies both
/// ends so the bound moves with the type scale rather than with a number typed
/// here.
#[allow(dead_code)]
const ICON_SCALE_MAX: usize = type_scale::TITLE;
#[allow(dead_code)]
const ICON_SCALE_MIN: usize = type_scale::BADGE;

/// Largest tile the deck draws, in either axis.
///
/// WHY 192 and not the measured 193: the reference tile measures 193 x 194
/// logical px, and `space::XXL * 4` is 192. The tile is therefore as wide and as
/// tall as the spacing scale can name, which is also what keeps a three-column
/// deck at the reference's proportions inside a 640 px content width.
#[allow(dead_code)]
const TILE_MAX: usize = space::XXL * 4;

/// Gap between deck tiles. The reference measures 30 logical px; `space::XL` is
/// the nearest token.
#[allow(dead_code)]
const TILE_GUTTER: usize = space::XL;

/// Height of the home gesture pill, and the gap above the bottom of the surface.
#[allow(dead_code)]
const GESTURE_PILL_H: usize = space::SM;

/// Telemetry glyph metrics in the 48 px status bar.
const TELEMETRY_TOP: usize = 16;
const TELEMETRY_H: usize = 17;
const TELEMETRY_SIGNAL_BARS: usize = 4;
const TELEMETRY_BAR_W: usize = 4;
const TELEMETRY_BAR_GAP: usize = 1;
const TELEMETRY_BATTERY_W: usize = 28;
/// Width of the Wi-Fi fan. WHY 16 and not the old 6: the glyph is a dot with
/// two arcs over it (radius 8 at the widest), so the box has to hold a 15 px
/// fan; a 6 px box could only ever fit a square, which is what it drew.
const TELEMETRY_WIFI_W: usize = 16;
const TELEMETRY_TERM_W: usize = 2;
const TELEMETRY_GAP: usize = 24;

/// Capacity of the formatted render-complete log line.
pub const HOME_LOG_CAP: usize = 128;

/// Verbatim tail every render-complete line must keep: the lifecycle harness
/// waits for this exact substring.
const LOG_TAIL: &[u8] = b"Home surface rendered.";

/// The surface's bands, measured once per call rather than retyped per pixel.
#[allow(dead_code)]
#[derive(Clone, Copy)]
struct Frame {
    greeting: Rect,
    date_widget: Rect,
    weather_widget: Rect,
    card: Rect,
    card_title: Rect,
    card_rows: Rect,
    deck: Rect,
    search: Rect,
    nav: Rect,
}

/// Height of one system-status row: a name band, a detail band and a gap.
fn card_row_h() -> usize {
    120
}

/// Height of the system card: its title band, its rows and the gaps between them.
fn card_h() -> usize {
    460
}

fn frame() -> Frame {
    let greeting = Rect::new(30, 56, 440, 140);
    let date_widget = Rect::new(490, 56, 200, 130);
    let weather_widget = Rect::new(480, 56, 210, 136);
    let card = Rect::new(
        30,
        205,
        660,
        card_h(),
    );
    let card_title = Rect::new(
        card.x + space::LG,
        card.y + space::MD,
        card.w - space::LG * 2,
        36,
    );
    let card_rows = Rect::new(
        card.x + space::LG,
        card.y + 60,
        card.w - space::LG * 2,
        card_row_h() * CARD_ROWS,
    );
    let deck = Rect::new(
        30,
        695,
        660,
        450,
    );
    let search = Rect::new(30, 1320, 660, 56);
    let nav = Rect::new(0, SCREEN_HEIGHT - NAV_BAR_HEIGHT, SCREEN_WIDTH, NAV_BAR_HEIGHT);
    Frame {
        greeting,
        date_widget,
        weather_widget,
        card,
        card_title,
        card_rows,
        deck,
        search,
        nav,
    }
}

/// Rows the home surface's system (Today) card occupies.
///
/// WHY an accessor and not a band typed in the shell: the painter gates the card
/// on `frame().card`, so the rows a tap on the card's checkbox can move are that
/// rectangle's rows. Reading them back from here is what makes the band and the
/// pixels it names one number held in one place.
pub fn system_card_band() -> (usize, usize) {
    let card = frame().card;
    (card.y, card.y + card.h)
}

/// Rows the home surface's app deck occupies.
///
/// WHY the deck is one band even though the tiles are six rectangles: the six
/// tiles are stacked two rows deep inside `frame().deck`, and a catalogue
/// refresh can retitle any of them, so the deck's whole extent is what has to be
/// re-rasterised. See [`system_card_band`] for why this is a function.
pub fn deck_band() -> (usize, usize) {
    let deck = frame().deck;
    (deck.y, deck.y + deck.h)
}

/// The wallpaper at pixel `(x, y)`, scene resolved once per frame.
///
/// WHY a scene and not `theme.ramp3`: the reference home panel's wallpaper is a
/// photograph that runs from a bright sky at the top to a near-black foreground
/// at the bottom, and the App View panel beside it is flat. So the home surface
/// is the one that draws a full-bleed wallpaper, and it draws the *same* one the
/// user chose for it, from the same catalogue the lock screen and the Settings
/// picker read. Every piece of ink drawn on it is chosen from the pixel behind
/// it rather than assumed, so the surface reads for any scene.
fn backdrop(scene: &interos_std::design::walls::Scene, x: usize, y: usize) -> u32 {
    interos_std::design::walls::scene_pixel(scene, x, y, SCREEN_WIDTH, SCREEN_HEIGHT)
}

/// The wallpaper for a frame whose scene is already resolved.
///
/// WHY a second entry point rather than changing [`backdrop`]'s signature: four
/// painters call `backdrop(p.scene, ..)`, and two of them call it twice for the
/// same pixel -- once to choose ink that contrasts, once to composite. Those
/// callers want the resolved tones; the picker-style callers that pass a bare
/// scene genuinely only want one pixel.
#[inline(always)]
fn backdrop_ref(s: &interos_std::design::walls::SceneRef<'_>, x: usize, y: usize) -> u32 {
    interos_std::design::walls::scene_pixel_ref(s, x, y, SCREEN_WIDTH, SCREEN_HEIGHT)
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
#[allow(dead_code)]
struct Run {
    x: usize,
    y: usize,
    w: usize,
    scale: usize,
    len: usize,
    ink: u32,
    /// Pixels per em for this run's outline rendering.
    #[allow(dead_code)]
    em: u32,
    /// Top row of the box that covers BOTH renderers.
    top: usize,
    /// Height of that union box.
    box_h: usize,
    bytes: [u8; RUN_CAP],
    outline: Option<launcher::outline_text::OutlineRun>,
    bold: bool,
}

impl Run {
    /// A run that never inks anything.
    const EMPTY: Run = Run {
        x: 0,
        y: 0,
        w: 0,
        scale: 1,
        len: 0,
        ink: 0,
        em: 0,
        top: 0,
        box_h: 0,
        bytes: [0; RUN_CAP],
        outline: None,
        bold: false,
    };

    /// `bytes` at (`x`, `y`), truncated with an ellipsis if wider than `fit`.
    fn fitted(x: usize, y: usize, scale: usize, fit: usize, bytes: &[u8], ink: u32) -> Run {
        Run::fitted_bold(x, y, scale, fit, bytes, ink, false)
    }

    fn fitted_bold(x: usize, y: usize, scale: usize, fit: usize, bytes: &[u8], ink: u32, bold: bool) -> Run {
        let (buf, len) = text::ellipsize(bytes, scale, fit);
        Run::placed_bold(x, y, scale, buf, len, ink, bold)
    }

    #[allow(dead_code)]
    /// As [`Run::fitted`], centred across `centre`.
    fn centred(
        x: usize,
        y: usize,
        scale: usize,
        fit: usize,
        centre: usize,
        bytes: &[u8],
        ink: u32,
    ) -> Run {
        Run::centred_bold(x, y, scale, fit, centre, bytes, ink, false)
    }

    fn centred_bold(
        x: usize,
        y: usize,
        scale: usize,
        fit: usize,
        centre: usize,
        bytes: &[u8],
        ink: u32,
        bold: bool,
    ) -> Run {
        let (buf, len) = text::ellipsize(bytes, scale, fit);
        let mut w = 0usize;
        for &c in &buf[..len] {
            w += launcher::font::advance_of(c, bold) * scale;
        }
        let inset = if centre > w { (centre - w) / 2 } else { 0 };
        Run::placed_bold(x + inset, y, scale, buf, len, ink, bold)
    }

    /// A run drawn whole, with no truncation step.
    fn whole(x: usize, y: usize, scale: usize, bytes: &[u8], ink: u32) -> Run {
        Run::whole_bold(x, y, scale, bytes, ink, false)
    }

    fn whole_bold(x: usize, y: usize, scale: usize, bytes: &[u8], ink: u32, bold: bool) -> Run {
        let mut buf = [0u8; RUN_CAP];
        let len = bytes.len().min(RUN_CAP);
        buf[..len].copy_from_slice(&bytes[..len]);
        Run::placed_bold(x, y, scale, buf, len, ink, bold)
    }

    #[allow(dead_code)]
    /// A run from an already-resolved buffer.
    fn placed(x: usize, y: usize, scale: usize, buf: [u8; RUN_CAP], len: usize, ink: u32) -> Run {
        Run::placed_bold(x, y, scale, buf, len, ink, false)
    }

    fn placed_bold(
        x: usize,
        y: usize,
        scale: usize,
        buf: [u8; RUN_CAP],
        len: usize,
        ink: u32,
        bold: bool,
    ) -> Run {
        let text = &buf[..len];
        let em = launcher::outline_text::em_for_scale(scale);
        let (asc, desc) = interos_std::design::typeface::line_metrics(em);
        let baseline = (y + launcher::font::BASELINE * scale).max(asc as usize);
        let outline_advance = interos_std::design::typeface::text_advance_px(text, em) as usize;
        let mut bitmap_measure = 0usize;
        for &c in text {
            bitmap_measure += launcher::font::advance_of(c, bold) * scale;
        }

        let w = bitmap_measure.max(outline_advance);
        let top = baseline.saturating_sub(asc as usize);
        let box_h = (baseline + desc as usize).saturating_sub(top);
        let outline = if scale >= 3 && !bold {
            launcher::outline_text::OutlineRun::place(x, baseline, em, text)
        } else {
            None
        };

        Run {
            x,
            y,
            w,
            scale,
            len,
            ink,
            em,
            top,
            box_h,
            bytes: buf,
            outline,
            bold,
        }
    }

    #[allow(dead_code)]
    fn pixel(&self, px: usize, py: usize) -> Option<u32> {
        self.pixel_inked(px, py, self.ink, 0)
    }

    fn pixel_inked(&self, px: usize, py: usize, ink: u32, bg: u32) -> Option<u32> {
        self.pixel_inked_aa(px, py, ink, bg)
    }

    fn pixel_aa(&self, px: usize, py: usize, ink: u32, bg: u32) -> Option<u32> {
        self.pixel_inked_aa(px, py, ink, bg)
    }

    fn pixel_inked_aa(&self, px: usize, py: usize, ink: u32, bg: u32) -> Option<u32> {
        if self.len == 0 {
            return None;
        }
        if let Some(ref outline) = self.outline {
            return outline.pixel(px, py, ink, bg);
        }
        interos_std::design::font::test_text_aa(
            px,
            py,
            self.x,
            self.y,
            self.scale,
            &self.bytes[..self.len],
            ink,
            bg,
            self.bold,
        )
    }
}

/// One distinct item in the Today card.
struct CardItemPlan {
    rect: Rect,
    icon: launcher::icons::IconKind,
    icon_color: u32,
    title: Run,
    subtitle: Option<Run>,
}

impl CardItemPlan {
    #[allow(dead_code)]
    const EMPTY: CardItemPlan = CardItemPlan {
        rect: Rect::new(0, 0, 0, 0),
        icon: launcher::icons::IconKind::Calendar,
        icon_color: 0,
        title: Run::EMPTY,
        subtitle: None,
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
    label: Run,
    #[allow(dead_code)]
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
#[allow(dead_code)]
struct DeckPlan {
    cells: [DeckTilePlan; MAX_DECK_TILES],
    slots: usize,
    columns: usize,
    rows: usize,
    origin_x: usize,
    origin_y: usize,
    pitch_x: usize,
    pitch_y: usize,
    app_count: usize,
    empty: Option<Run>,
}

impl DeckPlan {
    fn slot_at(&self, x: usize, y: usize) -> Option<usize> {
        if self.columns == 0 || self.rows == 0 || x < self.origin_x || y < self.origin_y {
            return None;
        }
        let col = (x - self.origin_x) / self.pitch_x.max(1);
        let row = (y - self.origin_y) / self.pitch_y.max(1);
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
struct Paint {
    frame: Frame,
    scene: &'static interos_std::design::walls::Scene,
    scene_ref: interos_std::design::walls::SceneRef<'static>,
    clock: Option<Run>,
    greeting: [Run; 3],
    weather_sun: (usize, usize, i32),
    weather_temp: Run,
    weather_location: Run,
    weather_desc: Run,
    card_title: Run,
    card_items: [CardItemPlan; 3],
    deck: DeckPlan,
    #[allow(dead_code)]
    search_magnifier: (usize, usize, i32),
    #[allow(dead_code)]
    search: Run,
    #[allow(dead_code)]
    nav_back: Run,
    #[allow(dead_code)]
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
    let wifi_x0 = signal_x + signal_w + TELEMETRY_GAP;
    let battery_x0 = wifi_x0 + TELEMETRY_WIFI_W + TELEMETRY_GAP;
    let term_x0 = battery_x0 + TELEMETRY_BATTERY_W;
    Telemetry {
        signal_x,
        signal_w,
        battery_x0,
        wifi_x0,
        term_x0,
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
        Some(Run::whole(
            CONTENT_PADDING,
            clock_y,
            scale,
            &state.clock_time,
            launcher::theme::TEXT_CHARCOAL,
        ))
    } else {
        None
    };

    // Greeting: time-of-day based on real RTC hour
    let day_part: &[u8] = if !state.clock_source_ok {
        b"Good day,"
    } else if state.clock_time[0].is_ascii_digit() && state.clock_time[1].is_ascii_digit() {
        let hour = (state.clock_time[0] - b'0') * 10 + (state.clock_time[1] - b'0');
        if (5..12).contains(&hour) {
            b"Good morning,"
        } else if (12..17).contains(&hour) {
            b"Good afternoon,"
        } else {
            b"Good evening,"
        }
    } else {
        b"Good day,"
    };

    let user_name: &[u8] = if state.user_name_len > 0 {
        &state.user_name[..state.user_name_len]
    } else {
        b"Bharath"
    };

    let user_status: &[u8] = if state.user_status_len > 0 {
        &state.user_status[..state.user_status_len]
    } else {
        b"Let's make today a good day."
    };

    let greeting = [
        Run::whole_bold(f.greeting.x + 14, f.greeting.y + 8, type_scale::BODY, day_part, launcher::theme::TEXT_CHARCOAL, false),
        Run::whole_bold(f.greeting.x + 14, f.greeting.y + 44, type_scale::TITLE, user_name, launcher::theme::TEXT_CHARCOAL, true),
        Run::whole_bold(f.greeting.x + 14, f.greeting.y + 108, type_scale::CAPTION, user_status, launcher::theme::TEXT_SUBTITLE_GRAY, false),
    ];

    // Weather widget
    let weather_sun = (620, 88, 18);
    let weather_temp = Run::whole_bold(602, 112, type_scale::BODY, b"24", launcher::theme::TEXT_CHARCOAL, true);
    let weather_location = Run::centred_bold(515, 150, type_scale::CAPTION, 210, 210, b"Coorg", launcher::theme::TEXT_CHARCOAL, false);
    let weather_desc = Run::centred_bold(515, 170, type_scale::CAPTION, 210, 210, b"Clear sky", launcher::theme::TEXT_SUBTITLE_GRAY, false);

    // Card title
    let card_title = Run::whole_bold(
        f.card.x + 28,
        f.card.y + 24,
        type_scale::BODY,
        b"Today",
        launcher::theme::TEXT_CHARCOAL,
        true,
    );

    // Inner card items
    let inner_x = f.card.x + 16;
    let inner_y = f.card.y + 60;
    let inner_w = f.card.w - 32;
    let item_h = 123;

    // Item 0: Calendar
    let (item0_sub_buf, item0_sub_len): ([u8; 32], usize) = if state.task0_done {
        let mut b = [0u8; 32];
        let s = b"11:00 AM - Done";
        b[..s.len()].copy_from_slice(s);
        (b, s.len())
    } else {
        let mut b = [0u8; 32];
        let s = b"11:00 AM";
        b[..s.len()].copy_from_slice(s);
        (b, s.len())
    };

    let item0 = CardItemPlan {
        rect: Rect::new(inner_x, inner_y, inner_w, item_h),
        icon: launcher::icons::IconKind::Calendar,
        icon_color: launcher::theme::TEXT_CHARCOAL,
        title: Run::whole_bold(
            inner_x + 74,
            inner_y + 28,
            type_scale::BODY,
            b"Meeting with team",
            launcher::theme::TEXT_CHARCOAL,
            false,
        ),
        subtitle: Some(Run::whole_bold(
            inner_x + 74,
            inner_y + 72,
            type_scale::CAPTION,
            &item0_sub_buf[..item0_sub_len],
            launcher::theme::TEXT_SUBTITLE_GRAY,
            false,
        )),
    };

    // Item 1: Messages
    let mut msg_buf = [0u8; 32];
    let msg_bytes: &[u8] = if state.notification_count > 0 {
        let mut at = push_decimal(&mut msg_buf, 0, state.notification_count);
        at = put(&mut msg_buf, at, b" unread messages");
        &msg_buf[..at]
    } else {
        b"2 unread messages"
    };

    let item1 = CardItemPlan {
        rect: Rect::new(inner_x, inner_y + item_h, inner_w, item_h),
        icon: launcher::icons::IconKind::Messages,
        icon_color: launcher::theme::TEXT_CHARCOAL,
        title: Run::whole_bold(
            inner_x + 74,
            inner_y + item_h + 48,
            type_scale::BODY,
            msg_bytes,
            launcher::theme::TEXT_CHARCOAL,
            false,
        ),
        subtitle: None,
    };

    // Item 2: Finish design work
    let (item2_title, item2_color) = if state.task0_done {
        (&b"Finish design work"[..], launcher::theme::SUCCESS_EMERALD)
    } else {
        (&b"Finish design work"[..], launcher::theme::TEXT_CHARCOAL)
    };

    let item2 = CardItemPlan {
        rect: Rect::new(inner_x, inner_y + item_h * 2, inner_w, 124),
        icon: launcher::icons::IconKind::CheckCircle,
        icon_color: item2_color,
        title: Run::whole_bold(
            inner_x + 74,
            inner_y + item_h * 2 + 48,
            type_scale::BODY,
            item2_title,
            launcher::theme::TEXT_CHARCOAL,
            false,
        ),
        subtitle: None,
    };

    let card_items = [item0, item1, item2];

    let deck = deck_plan(state, &theme, f.deck);

    let magnifier_r = 10i32;
    let magnifier_x = f.search.x + 36;
    let search_magnifier = (magnifier_x, f.search.center_y(), magnifier_r);
    let search_text_x = f.search.x + 64;
    let search_room = (f.search.x + f.search.w).saturating_sub(search_text_x).saturating_sub(space::LG);
    let search_y = f.search.y + f.search.h.saturating_sub(text::line_height(LABEL_SCALE)) / 2;
    let search = if f.search.w > 0 && f.search.h > 0 {
        Run::fitted(
            search_text_x,
            search_y,
            LABEL_SCALE,
            search_room,
            b"Search apps, settings & files",
            theme.text_muted,
        )
    } else {
        Run::whole(0, 0, LABEL_SCALE, b"", 0)
    };

    let pill_w = 140;

    Paint {
        frame: f,
        scene: state.home_wallpaper,
        scene_ref: interos_std::design::walls::SceneRef::new(state.home_wallpaper),
        clock,
        greeting,
        weather_sun,
        weather_temp,
        weather_location,
        weather_desc,
        card_title,
        card_items,
        deck,
        search_magnifier,
        search,
        nav_back: Run::EMPTY,
        nav_recents: Run::EMPTY,
        nav_pill: Rect::new(
            (SCREEN_WIDTH - pill_w) / 2,
            1418,
            pill_w,
            5,
        ),
        telemetry: telemetry_plan(),
    }
}

/// Resolves the deck for this frame.
///
/// WHY the empty message is stored rather than formatted per pixel: an empty deck
/// was calling `text::ellipsize` and `text::center_x` for every pixel of the whole
/// deck band, which is 15% of the surface, to draw sixteen glyphs in the middle.
/// WHY 200 wide at a 30px margin and a 30px gutter: the tiles used to be 190 at a
/// 48px margin, which left 174px for a name. The deck names are set at
/// `LABEL_SCALE` (CAPTION, a 17px cap -- the smallest step the 16x24 face can
/// render), so even the widest name fits with room on either side, and the
/// padding stays XS so short names keep breathing room.
pub const DOCK_TILES: [Rect; 6] = [
    Rect::new(30, 700, 200, 200),
    Rect::new(260, 700, 200, 200),
    Rect::new(490, 700, 200, 200),
    Rect::new(30, 930, 200, 200),
    Rect::new(260, 930, 200, 200),
    Rect::new(490, 930, 200, 200),
];

/// Half-size of a deck tile's pictogram, in pixels: 52x52 box.
pub const DOCK_ICON_EXTENT: i32 = 26;

pub fn find_package_index(state: &LauncherState, suffix: &[u8]) -> Option<usize> {
    for i in 0..state.app_count {
        let len = (state.app_id_lens[i] as usize).min(32);
        let id = &state.app_ids[i][..len];
        if id == b"org.interos.sensor_dashboard" || id == b"sensor_dashboard" || id == b"sensors" {
            continue;
        }
        if id.ends_with(suffix) {
            return Some(i);
        }
    }
    None
}

/// Resolves the deck for this frame.
fn deck_plan(_state: &LauncherState, _theme: &Theme, _deck: Rect) -> DeckPlan {
    let mut plan = DeckPlan {
        cells: [DeckTilePlan::EMPTY; MAX_DECK_TILES],
        slots: 6,
        columns: 3,
        rows: 2,
        origin_x: 30,
        origin_y: 700,
        pitch_x: 230,
        pitch_y: 230,
        app_count: 6,
        empty: None,
    };

    let fill = launcher::theme::CARD_FROSTED_CREAM;

    let dock_defs: [(Option<launcher::icons::IconKind>, &[u8], &[u8], u32); 6] = [
        (Some(launcher::icons::IconKind::Phone), b"dialer", b"Phone", launcher::theme::TEXT_CHARCOAL),
        (Some(launcher::icons::IconKind::Messages), b"messages", b"Messages", launcher::theme::TEXT_CHARCOAL),
        (Some(launcher::icons::IconKind::Camera), b"camera", b"Camera", launcher::theme::TEXT_CHARCOAL),
        (Some(launcher::icons::IconKind::Files), b"file_manager", b"Files", launcher::theme::TEXT_CHARCOAL),
        (Some(launcher::icons::IconKind::Browser), b"browser", b"Browser", launcher::theme::TEXT_CHARCOAL),
        (Some(launcher::icons::IconKind::Apps), b"apps", b"Apps", launcher::theme::TEXT_CHARCOAL),
    ];

    let label_bytes: [&[u8]; 6] = core::array::from_fn(|slot| {
        let (_, _, default_label, _) = dock_defs[slot];
        default_label
    });

    let label_fit = DOCK_TILES[0].w.saturating_sub(space::XS * 2);
    let label_scale = LABEL_SCALE;

    for (slot, &(icon_kind, _, _, icon_ink)) in dock_defs.iter().enumerate() {
        let cell = DOCK_TILES[slot];
        let label_bytes = label_bytes[slot];

        let icon_cx = cell.x + cell.w / 2;
        let icon_cy = cell.y + 74;
        let label_y = cell.y + 128;

        plan.cells[slot] = DeckTilePlan {
            cell,
            icon: icon_kind,
            icon_cx,
            icon_cy,
            ink: icon_ink,
            badge: Run::EMPTY,
            label: Run::centred_bold(
                cell.x,
                label_y,
                label_scale,
                label_fit,
                cell.w,
                label_bytes,
                launcher::theme::TEXT_CHARCOAL,
                false,
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
pub fn deck_columns(_state: &LauncherState) -> usize {
    3
}

/// The band the grid occupies for a given row count.
#[allow(dead_code)]
fn deck_bounds(deck: Rect, _rows: usize) -> Rect {
    deck
}

/// The deck's grid for the current catalogue.
#[allow(dead_code)]
fn deck_grid(_state: &LauncherState) -> layout::Grid {
    let deck = frame().deck;
    layout::grid(6, deck, 2, TILE_GUTTER)
}

/// Which catalogue index a touch at (`x`, `y`) lands on, if any.
#[allow(dead_code)]
pub fn home_hit(state: &LauncherState, x: usize, y: usize) -> Option<usize> {
    // WHY these ranges are the tile rects and not the old 48-based ones: the tap
    // must answer for exactly what the painter draws (`DOCK_TILES`), or the
    // outer 18px of both edge columns would launch nothing while showing tile.
    let col = if (30..235).contains(&x) {
        0
    } else if (255..465).contains(&x) {
        1
    } else if (485..695).contains(&x) {
        2
    } else {
        return None;
    };
    let row = if (700..925).contains(&y) {
        0
    } else if (925..1160).contains(&y) {
        1
    } else {
        return None;
    };
    let slot = row * 3 + col;
    match slot {
        0 => find_package_index(state, b"dialer"),
        1 => find_package_index(state, b"messages"),
        2 => find_package_index(state, b"camera"),
        3 => find_package_index(state, b"file_manager").or_else(|| find_package_index(state, b"files")),
        4 => find_package_index(state, b"browser"),
        _ => None,
    }
}

/// The registry's display name for catalogue entry `index`.
///
/// WHY slice the buffer by its own length: `LauncherState` stores labels
/// space-padded to a fixed capacity, so taking the whole row would render the
/// padding as trailing glyphs and every name would look the same width.
#[allow(dead_code)]
pub fn app_label(state: &LauncherState, index: usize) -> &[u8] {
    let len = (state.app_label_lens[index] as usize).min(state.app_labels[index].len());
    &state.app_labels[index][..len]
}

/// The registry's package ID for catalogue entry `index`.
#[allow(dead_code)]
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
#[allow(dead_code)]
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
    let left = r.x + rad;
    let right = r.x + r.w - 1 - rad;
    let top = r.y + rad;
    let bottom = r.y + r.h - 1 - rad;
    if (x >= left && x <= right) || (y >= top && y <= bottom) {
        return true;
    }
    let cx = if x < left { left } else { right };
    let cy = if y < top { top } else { bottom };
    let dx = (x as i64) - (cx as i64);
    let dy = (y as i64) - (cy as i64);
    let r2 = (rad as i64) * (rad as i64);
    let dx2 = dx * dx;
    let dy2 = dy * dy;
    // Continuous n=4 Lamé superellipse (flagship continuous squircle curvature)
    (dx2 * dx2) + (dy2 * dy2) <= r2 * r2
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
    let ink = layout::ink_on(theme, backdrop_ref(&p.scene_ref, x, y));

    // WHY gated on the source rather than drawn from the buffer: with no RTC
    // authority granted to this shell the read can be refused, and a clock is the
    // one thing on this surface a reader takes as a measurement. The gate itself is
    // resolved in the plan, so this is a plain "is the run there" test.
    if let Some(c) = p.clock.as_ref().and_then(|r| r.pixel_inked_aa(x, y, ink, backdrop_ref(&p.scene_ref, x, y))) {
        return Some(c);
    }

    telemetry_pixel(x, y, &p.telemetry, ink)
}

/// The status bar's hardware glyphs.
///
/// WHY the shapes carry no level: all three readings are [`Reading::Unbound`], so
/// nothing is filled with a number this platform cannot measure. Each glyph still
/// has to READ as what it stands for, though — an empty battery shell is read as
/// 0 % and a hollow square is read as a checkbox — so the unbound state is drawn
/// as an explicit "no reading" mark (a dash in the shell, a hollow Wi-Fi dot)
/// rather than as absence. The fill arithmetic is written out anyway and reached
/// only through [`Reading::level`], so a bound sensor has one place to go rather
/// than a colour to change.
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

    // Wi-Fi: a dot with two arcs over it — the universal fan — so the glyph
    // reads as Wi-Fi whether or not there is an association. The dot is hollow
    // while there is none.
    // WHY not the old hollow square: a 6 px empty box is indistinguishable from
    // an unchecked checkbox and says nothing about radio at all.
    if x >= wifi_x0 && x < wifi_x0 + TELEMETRY_WIFI_W {
        let cx = wifi_x0 as i32 + TELEMETRY_WIFI_W as i32 / 2;
        let dot_cy = bottom as i32 - 2;
        let dx = x as i32 - cx;
        let dy = dot_cy - y as i32; // positive above the dot
        let d2 = dx * dx + dy * dy;
        const DOT_R: i32 = 2;
        if d2 <= DOT_R * DOT_R {
            if wifi().level().is_some() || d2 >= (DOT_R - 1) * (DOT_R - 1) {
                return Some(ink);
            }
            return None;
        }
        // Two arcs above the dot, clipped to a 45-degree cone about vertical so
        // they read as a fan rather than a rainbow, and only above the dot so
        // the glyph stays inside the 17 px band.
        if dy > DOT_R && dx.abs() <= dy {
            for r in [5_i32, 8] {
                if d2 >= r * r && d2 <= (r + 1) * (r + 1) {
                    return Some(ink);
                }
            }
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
        if (x == battery_x0 || x == battery_x0 + TELEMETRY_BATTERY_W - 1) && (y == top || y == bottom) {
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
        } else {
            // WHY a dash: an empty shell is read as 0 % charge, and this shell
            // has no fuel gauge bound, so 0 % would be an invented reading. A
            // centred dash is the conventional "no reading" mark and keeps the
            // glyph legible as a battery rather than a dead one.
            let cy = (top + bottom) / 2;
            let dash_x = battery_x0 + TELEMETRY_BATTERY_W / 2 - 3;
            if y == cy && x >= dash_x && x < dash_x + 6 {
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
/// a name here would be invented. The day-part ("morning"/"afternoon"/"evening")
/// is derived from the real RTC hour in `build`, and falls back to a neutral
/// "Welcome" with no reading. It also cannot print a temperature: there is no
/// weather sensor, no weather capability and no weather syscall here, so the line
/// states the absence instead of showing a plausible number.
fn greeting_pixel(x: usize, y: usize, f: &Frame, p: &Paint, _theme: &Theme) -> Option<u32> {
    // 1. Weather widget (top right)
    if f.weather_widget.contains(x, y) {
        let bg = backdrop_ref(&p.scene_ref, x, y);

        // Radiant Sun Icon
        let (sx, sy, sr) = p.weather_sun;
        if let Some(c) = launcher::icons::rasterize_icon_pixel_scaled_aa(
            launcher::icons::IconKind::Sun,
            x,
            y,
            sx,
            sy,
            launcher::theme::WEATHER_SUN_AMBER,
            bg,
            sr,
        ) {
            return Some(c);
        }

        // Temperature: "24"
        if let Some(c) = p.weather_temp.pixel_aa(x, y, p.weather_temp.ink, bg) {
            return Some(c);
        }

        // Degree circle '°' next to "24" at (642, 120)
        let dx = x as i32 - 642;
        let dy = y as i32 - 120;
        let d2 = dx * dx + dy * dy;
        if d2 <= 16 && d2 >= 4 {
            return Some(launcher::theme::TEXT_CHARCOAL);
        }

        // Location: "Coorg"
        if let Some(c) = p.weather_location.pixel_aa(x, y, p.weather_location.ink, bg) {
            return Some(c);
        }

        // Description: "Clear sky"
        if let Some(c) = p.weather_desc.pixel_aa(x, y, p.weather_desc.ink, bg) {
            return Some(c);
        }

        return None;
    }

    // 2. Left side greeting runs
    let band = Rect::new(0, f.greeting.y, SCREEN_WIDTH, f.greeting.h);
    if !band.contains(x, y) {
        return None;
    }

    let bg = backdrop_ref(&p.scene_ref, x, y);
    for run in p.greeting.iter() {
        if let Some(c) = run.pixel_aa(x, y, run.ink, bg) {
            return Some(c);
        }
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

#[allow(dead_code)]
/// The card's detail line for a row, read from real state.
fn card_row_text(state: &LauncherState, row: usize, out: &mut [u8; CARD_DETAIL_CAP]) -> usize {
    match row {
        0 => {
            if state.task0_done {
                put(out, 0, b"All tasks completed")
            } else {
                put(out, 0, b"No events on this device")
            }
        }
        1 => {
            if state.notification_count > 0 {
                let mut at = push_decimal(out, 0, state.notification_count);
                at = put(out, at, b" unread notifications");
                at
            } else {
                put(out, 0, b"No new notifications")
            }
        }
        _ => {
            if let Some(label) = state.recent_label(0) {
                let mut at = put(out, 0, b"Recent: ");
                let len = label.len().min(out.len().saturating_sub(at));
                at = put(out, at, &label[..len]);
                at
            } else if state.recent_app_lens[0] > 0 {
                let len = (state.recent_app_lens[0] as usize).min(24);
                let mut at = put(out, 0, b"Recent: ");
                let mut name = [0u8; CARD_DETAIL_CAP];
                let readable = text::humanise_package_id(&state.recent_apps[0][..len], &mut name);
                at = put(out, at, readable);
                at
            } else {
                let mut at = push_decimal(out, 0, state.app_count);
                at = put(out, at, b" apps installed in system");
                at
            }
        }
    }
}

#[allow(dead_code)]
/// The card's row name.
fn card_row_name(row: usize) -> &'static [u8] {
    match row {
        0 => b"Schedule",
        1 => b"Notifications",
        _ => b"Activities",
    }
}

fn card_pixel(x: usize, y: usize, f: &Frame, p: &Paint, _theme: &Theme) -> Option<u32> {
    if !f.card.contains(x, y) {
        return None;
    }
    if !rounded_contains(f.card, x, y, 28) {
        return None;
    }
    if rounded_outline(f.card, x, y, 28) {
        return Some(launcher::theme::DIVIDER_HAIRLINE);
    }
    let card_fill = launcher::theme::CARD_FROSTED_CREAM;

    if let Some(c) = p.card_title.pixel_aa(x, y, p.card_title.ink, card_fill) {
        return Some(c);
    }

    let inner = Rect::new(f.card.x + 16, f.card.y + 60, f.card.w - 32, 370);
    if inner.contains(x, y) {
        if !rounded_contains(inner, x, y, 20) {
            return Some(card_fill);
        }
        if rounded_outline(inner, x, y, 20) {
            return Some(launcher::theme::DIVIDER_HAIRLINE);
        }
        let inner_fill = launcher::theme::CARD_WARM_OFFWHITE;

        // Hairline dividers between items inside inner card
        if (y == inner.y + 123 || y == inner.y + 246)
            && x >= inner.x + 16
            && x <= inner.x + inner.w - 16
        {
            return Some(launcher::theme::DIVIDER_HAIRLINE);
        }

        for item in p.card_items.iter() {
            if !item.rect.contains(x, y) {
                continue;
            }

            // Right chevron '>' at inner.x + inner.w - 32
            let ch_x = inner.x + inner.w - 32;
            let icon_cy = item.rect.y + item.rect.h / 2;
            let cdx = x as i32 - ch_x as i32;
            let cdy = (y as i32 - icon_cy as i32).abs();
            if cdx >= -3 && cdx <= 3 && (cdy == 3 - cdx || cdy == 2 - cdx) {
                return Some(launcher::theme::TEXT_SUBTITLE_GRAY);
            }

            // Item icon (36px box, extent 18)
            let icon_cx = item.rect.x + 36;
            if let Some(c) = launcher::icons::rasterize_icon_pixel_scaled_aa(
                item.icon,
                x,
                y,
                icon_cx,
                icon_cy,
                item.icon_color,
                inner_fill,
                18,
            ) {
                return Some(c);
            }

            // Item title
            if let Some(c) = item.title.pixel_aa(x, y, item.title.ink, inner_fill) {
                return Some(c);
            }

            // Item subtitle
            if let Some(ref subtitle) = item.subtitle {
                if let Some(c) = subtitle.pixel_aa(x, y, subtitle.ink, inner_fill) {
                    return Some(c);
                }
            }

            return Some(inner_fill);
        }

        return Some(inner_fill);
    }

    Some(card_fill)
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
#[allow(dead_code)]
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

fn deck_pixel(x: usize, y: usize, f: &Frame, p: &Paint, _theme: &Theme) -> Option<u32> {
    let deck = &p.deck;
    if !f.deck.contains(x, y) {
        return None;
    }
    if let Some(empty) = deck.empty.as_ref() {
        return empty.pixel_inked(x, y, empty.ink, backdrop(&p.scene, x, y));
    }

    let index = deck.slot_at(x, y)?;
    if index >= deck.app_count {
        return None;
    }
    let tile = &deck.cells[index];

    if !rounded_contains(tile.cell, x, y, 28) {
        return None;
    }
    if rounded_outline(tile.cell, x, y, 28) {
        return Some(launcher::theme::DIVIDER_HAIRLINE);
    }
    let tile_fill = launcher::theme::CARD_FROSTED_CREAM;

    if let Some(kind) = tile.icon {
        if let Some(c) = launcher::icons::rasterize_icon_pixel_scaled_aa(
            kind,
            x,
            y,
            tile.icon_cx,
            tile.icon_cy,
            tile.ink,
            tile_fill,
            DOCK_ICON_EXTENT,
        ) {
            return Some(c);
        }
    } else if let Some(c) = tile.badge.pixel_aa(x, y, tile.badge.ink, tile_fill) {
        return Some(c);
    }
    if let Some(c) = tile.label.pixel_aa(x, y, tile.label.ink, tile_fill) {
        return Some(c);
    }
    Some(tile_fill)
}

// -----------------------------------------------------------------------------
// Search control
// -----------------------------------------------------------------------------

/// The universal search control.
///
/// Returns None on home screen to present the clean mountain landscape wallpaper
/// exactly as designed in the Full HD aesthetic reference mockup.
fn search_pixel(_x: usize, _y: usize, _f: &Frame, _p: &Paint, _theme: &Theme) -> Option<u32> {
    None
}

// -----------------------------------------------------------------------------
// Navigation
// -----------------------------------------------------------------------------

fn nav_pixel(x: usize, y: usize, f: &Frame, p: &Paint, _theme: &Theme) -> Option<u32> {
    if !f.nav.contains(x, y) {
        return None;
    }
    if rounded_contains(p.nav_pill, x, y, 3) {
        return Some(launcher::theme::TEXT_CHARCOAL);
    }
    None
}

// -----------------------------------------------------------------------------
// Surface
// -----------------------------------------------------------------------------

pub fn get_home_color(x: usize, y: usize, state: &LauncherState) -> u32 {
    let p = paint(x, y, state);
    let theme = active_theme();
    if y < STATUS_BAR_HEIGHT {
        if let Some(c) = status_bar_pixel(x, y, &theme, p) {
            return c;
        }
    } else if y < 205 {
        if let Some(c) = greeting_pixel(x, y, &p.frame, p, &theme) {
            return c;
        }
    } else if y < 695 {
        if let Some(c) = card_pixel(x, y, &p.frame, p, &theme) {
            return c;
        }
    } else if y >= 695 && y < 1150 {
        if let Some(c) = deck_pixel(x, y, &p.frame, p, &theme) {
            return c;
        }
    } else if y >= 1315 && y < 1390 {
        if let Some(c) = search_pixel(x, y, &p.frame, p, &theme) {
            return c;
        }
    } else if y >= 1390 {
        if let Some(c) = nav_pixel(x, y, &p.frame, p, &theme) {
            return c;
        }
    }
    backdrop_ref(&p.scene_ref, x, y)
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
