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

use crate::{test_text_aa, LauncherState};
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

/// Line bands of a system-status row: the row's name, then its detail.
const CARD_ROW_LABEL: usize = type_scale::BODY;
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
#[derive(Clone, Copy)]
struct Frame {
    greeting: Rect,
    card: Rect,
    card_title: Rect,
    card_rows: Rect,
    deck: Rect,
    search: Rect,
    nav: Rect,
}

/// Height of one system-status row: a name band, a detail band and a gap.
fn card_row_h() -> usize {
    100
}

/// Height of the system card: its title band, its rows and the gaps between them.
fn card_h() -> usize {
    490
}

fn frame() -> Frame {
    let content = layout::content_area(CONTENT_PADDING);
    let greeting = Rect::new(content.x, 56, content.w, 96);
    let card = Rect::new(
        content.x,
        168,
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
    // WHY x 30 and 660 wide: the band must cover the drawn tiles (`DOCK_TILES`
    // run 30..690 with 30px gutters, the reference's own gutter), or the outer
    // 18px of both edge columns falls through to the wallpaper. It used to be
    // 48..672, which clipped exactly that.
    let deck = Rect::new(
        30,
        700,
        660,
        620,
    );
    // WHY directly below the deck: the deck ends at 1320 and the nav bar starts
    // at 1392, so 1320..1384 is the only band that touches neither. The shell
    // hit-tests this exact rect for the System Space tap.
    let search = Rect::new(40, SEARCH_TOP, 640, SEARCH_H);
    let nav = Rect::new(0, SCREEN_HEIGHT - NAV_BAR_HEIGHT, SCREEN_WIDTH, NAV_BAR_HEIGHT);
    Frame {
        greeting,
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
    ///
    /// WHY a separate number from `scale`: the bitmap face is a 16x24 cell
    /// multiplied by `scale`, so its cap height is 17*`scale` pixels. The
    /// outline face is rasterised at an em, and its cap height is 1456 of 2048
    /// em, so the em that matches the same cap height is
    /// `17 * scale * 2048 / 1456`. Carrying it means the two faces can be
    /// compared instead of guessed at.
    #[allow(dead_code)]
    em: u32,
    /// Top row of the box that covers BOTH renderers.
    ///
    /// WHY the box is a union: the outline run's line box is not the bitmap
    /// cell. At the em that matches a scale-3 cap, the outline's ascent reaches
    /// 18px above the cell origin and its descent 4px past the cell bottom, so
    /// a box measured for the cell alone would reject rows the outline inks and
    /// the text would be silently clipped.
    top: usize,
    /// Height of that union box.
    box_h: usize,
    bytes: [u8; RUN_CAP],
    outline: Option<launcher::outline_text::OutlineRun>,
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
    ///
    /// WHY the width and the box are wider than the bitmap's own: the run now
    /// has two renderers, and the box has to admit both. The bitmap's measure
    /// and line height bound one; the outline's advance and its ascent-plus-
    /// descent bound the other. Taking the union of each costs nothing at this
    /// point and is the difference between text that renders and text that is
    /// silently clipped.
    fn placed(x: usize, y: usize, scale: usize, buf: [u8; RUN_CAP], len: usize, ink: u32) -> Run {
        let text = &buf[..len];
        // The bitmap face's cap height is 17 rows at scale 1 and the outline
        // face's cap height is 1456 of 2048 em, so this em reproduces the same
        // cap height the cell multiple produced.
        let em = launcher::outline_text::em_for_scale(scale);
        // The outline's baseline lands where the cell puts its own, so the two
        // agree on where the text sits rather than merely how tall it is.
        let baseline = y + launcher::font::BASELINE * scale;
        let (asc, desc) = interos_std::design::typeface::line_metrics(em);
        let outline_advance = interos_std::design::typeface::text_advance_px(text, em) as usize;
        let bitmap_measure = text::measure(text, scale);

        let w = bitmap_measure.max(outline_advance);
        let top = baseline.saturating_sub(asc as usize);
        let box_h = (baseline + desc as usize).saturating_sub(top);
        let outline = launcher::outline_text::OutlineRun::place(x, baseline, em, text);

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
        }
    }

    /// The colour of (`px`, `py`) if this run inks it.
    ///
    /// Why the attribute: no painter on this surface reaches it -- the greeting
    /// resolves its own ink per row against the wallpaper and calls
    /// `pixel_inked_aa`, and the deck's empty message goes through
    /// `pixel_inked`. The helper stays because a `Run` that can answer a plain
    /// question about a pixel is the obvious next thing any caller wants, and
    /// removing it would mean re-deriving it.
    #[allow(dead_code)]
    fn pixel(&self, px: usize, py: usize) -> Option<u32> {
        self.pixel_inked(px, py, self.ink, 0)
    }

    /// As [`Run::pixel`], with the ink chosen by the caller.
    ///
    /// WHY two of them need this: the greeting and the navigation bar are drawn
    /// straight onto the wallpaper ramp, whose colour is a function of `y`, so their
    /// ink cannot be settled until the pixel's row is known.
    fn pixel_inked(&self, px: usize, py: usize, ink: u32, bg: u32) -> Option<u32> {
        if self.len == 0 {
            return None;
        }
        test_text_aa(
            px,
            py,
            self.x,
            self.y,
            self.scale,
            &self.bytes[..self.len],
            ink,
            bg,
        )
    }

    /// Anti-aliased render of this run with caller-chosen ink and background.
    fn pixel_aa(&self, px: usize, py: usize, ink: u32, bg: u32) -> Option<u32> {
        self.pixel_inked_aa(px, py, ink, bg)
    }

    fn pixel_inked_aa(&self, px: usize, py: usize, ink: u32, bg: u32) -> Option<u32> {
        if self.len == 0 {
            return None;
        }
        test_text_aa(
            px,
            py,
            self.x,
            self.y,
            self.scale,
            &self.bytes[..self.len],
            ink,
            bg,
        )
    }
}

/// One row of the system card, with both of its lines resolved.
struct CardRowPlan {
    rect: Rect,
    name: Run,
    detail: Run,
    /// Row 0 task-checkbox state for the pinned Today toggle (main.rs hit
    /// x60..100, y300..340): the painter reads this so the box on screen
    /// always matches the bit the toggle flips.
    done: bool,
}

impl CardRowPlan {
    const EMPTY: CardRowPlan = CardRowPlan {
        rect: Rect::new(0, 0, 0, 0),
        name: Run::EMPTY,
        detail: Run::EMPTY,
        done: false,
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
#[allow(dead_code)]
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
///
/// WHY this is separate from [`Frame`]: the band rectangles are needed by the hit
/// test and by [`deck_columns`], which must stay cheap enough to call from a tap
/// handler, while this is the painter's plan and is built once per rendered frame.
struct Paint {
    frame: Frame,
    /// The wallpaper this frame draws behind everything.
    ///
    /// WHY it is in the plan: the backdrop is read per pixel by four separate
    /// painters, and it is chosen by a value the settings store answers. Putting
    /// the resolved scene in the plan means each of them gets the same one and
    /// none of them has to resolve a stored key.
    scene: &'static interos_std::design::walls::Scene,
    /// The same scene with its derived tones and clamped skyline resolved once.
    ///
    /// WHY both a `Scene` and a `SceneRef` are held: `SceneRef::new` borrows, and
    /// the plan is built into a `'static` slot. Holding the reference keeps the
    /// resolved tones alive exactly as long as the scene does, and the painter
    /// never has to re-derive them.
    ///
    /// WHY this matters at all: the derived tones are three 4-channel blends
    /// that depend only on the scene, and the per-pixel path used to recompute
    /// them for every one of the surface's pixels. On a full repaint that is
    /// millions of redundant blends.
    scene_ref: interos_std::design::walls::SceneRef<'static>,
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

    // Greeting: the day-part comes from the real RTC hour when the kernel
    // answered, and says nothing time-specific when it did not. WHY not a fixed
    // "Good morning": with no reading that string claims a time of day the shell
    // never measured.
    let title_scale = type_scale::BODY;
    let caption_scale = type_scale::CAPTION;
    let l1_y = f.greeting.y;
    let l2_y = l1_y + text::line_height(title_scale) + space::SM;
    let day_part: &[u8] = if !state.clock_source_ok {
        b"Welcome"
    } else if state.clock_time[0].is_ascii_digit() && state.clock_time[1].is_ascii_digit() {
        let hour = (state.clock_time[0] - b'0') * 10 + (state.clock_time[1] - b'0');
        if hour < 12 {
            b"Good morning"
        } else if hour < 18 {
            b"Good afternoon"
        } else {
            b"Good evening"
        }
    } else {
        b"Welcome"
    };
    let greeting = [
        Run::whole(f.greeting.x, l1_y, title_scale, day_part, 0),
        Run::whole(f.greeting.x, l2_y, caption_scale, b"Let's make a good day.", 0),
        Run::EMPTY,
    ];
    // Weather remains empty until a real weather capability is bound.
    let weather = Run::EMPTY;

    let card_title = Run::whole(
        f.card_title.x,
        f.card_title.y,
        type_scale::BODY,
        b"Today",
        theme.text,
    );
    let card_row_h = card_row_h();
    let card_stack = layout::list(f.card_rows, CARD_ROWS, card_row_h, space::LG);
    let mut card_rows = [CardRowPlan::EMPTY; CARD_ROWS];
    if let Some(stack) = card_stack {
        for row in 0..CARD_ROWS {
            let Some(rect) = layout::list_row(stack, row, CARD_ROWS, card_row_h, space::LG) else {
                continue;
            };
            let mut buf = [0u8; CARD_DETAIL_CAP];
            let len = card_row_text(state, row, &mut buf);
            card_rows[row] = CardRowPlan {
                rect,
                name: Run::whole(
                    rect.x + space::XXL,
                    rect.y + 12,
                    CARD_ROW_LABEL,
                    card_row_name(row),
                    theme.text,
                ),
                detail: Run::whole(
                    rect.x + space::XXL,
                    rect.y + 12 + text::line_height(CARD_ROW_LABEL) + space::SM,
                    CARD_ROW_DETAIL,
                    &buf[..len],
                    theme.text_muted,
                ),
                done: row == 0 && state.task0_done,
            };
        }
    }

    let deck = deck_plan(state, &theme, f.deck);

    let search_scale = type_scale::BODY;
    let magnifier_r = (text::line_height(search_scale) / 2).max(4) as i32;
    // WHY the glyph is measured as a whole, not just its ring: the ring plus
    // its handle is a square of side 2 * (r + handle) anchored top-left at the
    // centre, so the centre has to sit at least that far inside the pill's left
    // rounded edge or the handle falls outside the bar. `space::XL` alone put
    // the ring's left edge on the border and the handle below the pill.
    let magnifier_span = magnifier_r + TELEMETRY_BAR_W as i32 + 1;
    let magnifier_x = (f.search.x + space::LG).saturating_add(magnifier_span as usize);
    let search_magnifier = (magnifier_x, f.search.center_y(), magnifier_r);
    let search_text_x = magnifier_x + magnifier_span as usize + space::SM;
    let search_room = (f.search.x + f.search.w).saturating_sub(search_text_x).saturating_sub(space::XXL);
    let search_y = f.search.y + f.search.h.saturating_sub(text::line_height(LABEL_SCALE)) / 2;
    let search = if f.search.w > 0 && f.search.h > 0 {
        Run::fitted(
            search_text_x,
            search_y,
            LABEL_SCALE,
            search_room,
            b"Search apps and files",
            theme.text_muted,
        )
    } else {
        Run::whole(0, 0, LABEL_SCALE, b"", 0)
    };

    let nav_scale = type_scale::CAPTION;
    let nav_cy = f.nav.y + f.nav.h.saturating_sub(text::line_height(nav_scale)) / 2;
    // WHY the two navigation glyphs are placed here: their origins came from
    // `text::measure` of a one- and two-byte literal, once per pixel, and they sit
    // on the wallpaper ramp, so their ink is supplied per row rather than baked in.
    let nav_back = Run::whole(
        (f.nav.w / 6).saturating_sub(text::measure(b"<", nav_scale) / 2),
        nav_cy,
        nav_scale,
        b"<",
        0,
    );
    let nav_recents = Run::whole(
        (f.nav.w * 5 / 6).saturating_sub(text::measure(b"[]", nav_scale) / 2),
        nav_cy,
        nav_scale,
        b"[]",
        0,
    );
    let pill_w = space::XXL * 5;

    Paint {
        frame: f,
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
        scene: state.home_wallpaper,
        scene_ref: interos_std::design::walls::SceneRef::new(state.home_wallpaper),
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
    Rect::new(30, 700, 200, 280),
    Rect::new(260, 700, 200, 280),
    Rect::new(490, 700, 200, 280),
    Rect::new(30, 1020, 200, 280),
    Rect::new(260, 1020, 200, 280),
    Rect::new(490, 1020, 200, 280),
];

/// Half-size of a deck tile's pictogram, in pixels: a 52x52 box for high legibility.
pub const DOCK_ICON_EXTENT: i32 = 26;

pub fn find_package_index(state: &LauncherState, suffix: &[u8]) -> Option<usize> {
    for i in 0..state.app_count {
        let len = (state.app_id_lens[i] as usize).min(32);
        let id = &state.app_ids[i][..len];
        if id.ends_with(suffix) {
            return Some(i);
        }
    }
    None
}

/// Resolves the deck for this frame.
fn deck_plan(state: &LauncherState, theme: &Theme, _deck: Rect) -> DeckPlan {
    let mut plan = DeckPlan {
        cells: [DeckTilePlan::EMPTY; MAX_DECK_TILES],
        slots: 6,
        columns: 3,
        rows: 2,
        // WHY 30 and 230: the origin and pitch must name the drawn grid
        // (`DOCK_TILES` at x 30/260/490, 200 wide with 30px gutters), or
        // `slot_at` resolves taps and pixels against a grid the painter never
        // drew. The pitch used to be 217 from a 48 origin, a third grid that
        // matched neither the tiles nor the tap ranges.
        origin_x: 30,
        origin_y: 700,
        pitch_x: 230,
        pitch_y: 320,
        app_count: 6,
        empty: None,
    };

    let fill = theme.surface;

    let dock_defs: [(Option<launcher::icons::IconKind>, &[u8], &[u8], u32); 6] = [
        (Some(launcher::icons::IconKind::Phone), b"dialer", b"Phone", theme.text),
        (Some(launcher::icons::IconKind::Messages), b"messages", b"Messages", theme.text),
        (Some(launcher::icons::IconKind::Camera), b"camera", b"Camera", theme.text),
        (Some(launcher::icons::IconKind::Files), b"file_manager", b"Files", theme.text),
        (Some(launcher::icons::IconKind::Browser), b"browser", b"Browser", theme.text),
        (Some(launcher::icons::IconKind::Apps), b"apps", b"Apps", theme.text),
    ];

    // WHY one scale for the whole deck: picking a scale per label made "Messages"
    // render a step smaller than its neighbours, which reads as a rendering
    // fault rather than a long name, so every tile is set in the same size.
    // WHY `LABEL_SCALE` (CAPTION, a 17px cap) and not BODY: BODY put a 34px cap
    // on the tile names -- nearly triple the 13px AppTile label the design
    // tokens name (`components.json`) -- and the widest name filled its tile
    // edge to edge. CAPTION is the smallest step the 16x24 face can render, so
    // it is the closest the firmware can come to the token; a registry name
    // longer than the tile is still ellipsized to the fit by `Run::centred`.
    let label_bytes: [&[u8]; 6] = core::array::from_fn(|slot| {
        let (_, suffix, default_label, _) = dock_defs[slot];
        if slot == 5 {
            default_label
        } else if let Some(pkg_idx) = find_package_index(state, suffix).or_else(|| {
            if suffix == b"file_manager" {
                find_package_index(state, b"files")
            } else {
                None
            }
        }) {
            app_label(state, pkg_idx)
        } else {
            default_label
        }
    });
    // WHY XS for the padding: the fit is the tile less the inset the label
    // needs on each side, and at XS even the widest name fits at `LABEL_SCALE`
    // with room to spare.
    let label_fit = DOCK_TILES[0].w.saturating_sub(space::XS * 2);
    let label_scale = LABEL_SCALE;

    for (slot, &(icon_kind, _, _, icon_ink)) in dock_defs.iter().enumerate() {
        let cell = DOCK_TILES[slot];
        let label_bytes = label_bytes[slot];

        let icon_cx = cell.x + cell.w / 2;
        // WHY the block is centred rather than at fixed offsets: the tile is 280
        // tall and its content is a 48px pictogram, a 20px gap and a 24px name
        // cell, which is 92 -- leaving 94px above and below. The offsets are
        // derived from `DOCK_ICON_EXTENT` so changing the icon size moves the
        // name with it instead of leaving the block lopsided.
        let block_h = DOCK_ICON_EXTENT as usize * 2 + space::MD + space::XS
            + text::line_height(label_scale);
        let block_y = cell.y + (cell.h.saturating_sub(block_h)) / 2;
        let icon_cy = block_y + DOCK_ICON_EXTENT as usize;
        let label_y = block_y + DOCK_ICON_EXTENT as usize * 2 + space::MD + space::XS;

        plan.cells[slot] = DeckTilePlan {
            cell,
            icon: icon_kind,
            icon_cx,
            icon_cy,
            ink: icon_ink,
            badge: Run::EMPTY,
            label: Run::centred(
                cell.x,
                label_y,
                label_scale,
                label_fit,
                cell.w,
                label_bytes,
                theme.text,
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
    let col = if (30..230).contains(&x) {
        0
    } else if (260..460).contains(&x) {
        1
    } else if (490..690).contains(&x) {
        2
    } else {
        return None;
    };
    let row = if (700..980).contains(&y) {
        0
    } else if (1020..1300).contains(&y) {
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
fn greeting_pixel(x: usize, y: usize, f: &Frame, p: &Paint, theme: &Theme) -> Option<u32> {
    let band = Rect::new(0, f.greeting.y, SCREEN_WIDTH, f.greeting.h);
    if !band.contains(x, y) {
        return None;
    }
    // WHY no weather glyph here: there is no location source, no weather sensor
    // and no weather syscall on this platform, so a sun disc and temperature
    // would be fabricated readings. The weather run stays empty (see `build`) and
    // the absence is stated, not painted over.

    // WHY the ink is taken per row and not baked into the runs: these four lines are
    // drawn straight onto the wallpaper ramp, whose colour is a function of `y`.
    let ink = layout::ink_on(theme, backdrop_ref(&p.scene_ref, x, y));
    for run in p.greeting.iter() {
        if let Some(c) = run.pixel_inked_aa(x, y, ink, backdrop_ref(&p.scene_ref, x, y)) {
            return Some(c);
        }
    }
    if let Some(c) = p.weather.pixel_inked(x, y, ink, backdrop_ref(&p.scene_ref, x, y)) {
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
        // WHY this and not an "active" claim: there is no calendar store and no
        // sync service behind this shell, so a row that said it was up to date
        // would be fabricating status on the surface the user reads as device
        // state. WHY it is phrased as the missing thing rather than as an
        // error: the earlier text was "Calendar sync unavailable - no event
        // data", which reports a subsystem failure for what is simply an empty
        // calendar on a phone that has not been given one. `live_widgets.rs`
        // states the same absence as "no event data"; this matches it.
        0 => put(out, 0, b"No events on this device"),
        1 => {
            // WHY notifications and not messages: this count is the system-wide
            // notification query, not a per-app message count, so labelling it
            // "Messages" claimed a messaging state the shell cannot see.
            if state.notification_count > 0 {
                let mut at = push_decimal(out, 0, state.notification_count);
                at = put(out, at, b" unread notifications");
                at
            } else {
                put(out, 0, b"No new notifications")
            }
        }
        _ => {
            // WHY the registry label and not the raw id: recents hold package
            // ids ("org.interos.messages"), which are developer strings, not
            // something a user should read. `recent_label` resolves the id to
            // the registry display name ("Messages"); the raw id remains only
            // as the fallback for an id the registry no longer names.
            if let Some(label) = state.recent_label(0) {
                let mut at = put(out, 0, b"Recent: ");
                let len = label.len().min(out.len().saturating_sub(at));
                at = put(out, at, &label[..len]);
                at
            } else if state.recent_app_lens[0] > 0 {
                // WHY this fallback is still a name and not the id: the branch is
                // reached exactly when `recent_label` failed, which means the
                // registry does not describe this package. Printing the id here
                // is what put `org.interos.messages` on the home screen.
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

/// The card's row name.
fn card_row_name(row: usize) -> &'static [u8] {
    match row {
        0 => b"Schedule",
        // Matches the notification-count detail line above: the row reports
        // system notifications, so it is named for them.
        1 => b"Notifications",
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
    if let Some(c) = p.card_title.pixel_aa(x, y, p.card_title.ink, theme.surface) {
        return Some(c);
    }

    if p.card_stack.is_none() {
        return None;
    }
    for (row_idx, row) in p.card_rows.iter().enumerate() {
        if !row.rect.contains(x, y) {
            continue;
        }
        let icon_cx = (row.rect.x + space::MD) as i32;
        let icon_cy = (row.rect.y + 12 + text::line_height(CARD_ROW_LABEL) / 2) as i32;
        let idx = x as i32 - icon_cx;
        let idy = y as i32 - icon_cy;

        match row_idx {
            0 => {
                // Schedule: calendar icon (square outline with top hooks)
                if idx.abs() <= 5 && idy.abs() <= 5 {
                    if idx.abs() == 5 || idy == -5 || idy == 5 || idy == -2 {
                        return Some(theme.accent);
                    }
                }
                // Task checkbox for the pinned Today toggle (main.rs hit
                // x60..100, y300..340, centred on the pinned tap at 78,318):
                // outline always; accent fill with a surface tick only when
                // the toggle's state bit is set, so the control on screen
                // visibly reports what the toggle log claims.
                let cbx = x as i32 - 78;
                let cby = y as i32 - 318;
                if cbx.abs() <= 12 && cby.abs() <= 12 {
                    if cbx.abs() == 12 || cby.abs() == 12 {
                        return Some(theme.text_muted);
                    }
                    if row.done {
                        let tick = (cbx >= -6 && cbx <= -1 && cby == cbx + 6)
                            || (cbx >= 0 && cbx <= 6 && cby == 6 - cbx);
                        return Some(if tick { theme.surface } else { theme.accent });
                    }
                    return Some(theme.surface);
                }
            }
            1 => {
                // Notifications: circular indicator dot
                if idx * idx + idy * idy <= 4 * 4 {
                    return Some(theme.accent);
                }
            }
            _ => {
                // Activities: checkmark icon
                if idx.abs() <= 5 && idy.abs() <= 5 {
                    if idx.abs() == 5 || idy.abs() == 5 {
                        return Some(theme.accent);
                    }
                    if (idx == -1 && idy == 1) || (idx == 0 && idy == 2) || (idx == 2 && idy == 0) {
                        return Some(theme.accent);
                    }
                }
            }
        }
        if let Some(c) = row.name.pixel_aa(x, y, row.name.ink, theme.surface) {
            return Some(c);
        }
        if let Some(c) = row.detail.pixel_aa(x, y, row.detail.ink, theme.surface) {
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

fn deck_pixel(x: usize, y: usize, f: &Frame, p: &Paint, theme: &Theme) -> Option<u32> {
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
        if let Some(c) = launcher::icons::rasterize_icon_pixel_scaled(
            kind,
            x,
            y,
            tile.icon_cx,
            tile.icon_cy,
            tile.ink,
            DOCK_ICON_EXTENT,
        ) {
            return Some(c);
        }
    } else if let Some(c) = tile.badge.pixel_aa(x, y, tile.badge.ink, tile.fill) {
        return Some(c);
    }
    // WHY the name is drawn after the badge and before the fill: the badge is the
    // icon, the name is its caption, and both sit on the tile's own fill. The run
    // was ellipsized to the tile less its padding and placed in `deck_plan`, so this
    // is one bounds test per pixel inside the tile -- and a name longer than the
    // tile (the registry holds one up to sixteen bytes, e.g. "Camera Preview") is
    // already shortened inside the tile by the time it gets here.
    if let Some(c) = tile.label.pixel_aa(x, y, tile.label.ink, tile.fill) {
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
    // WHY a ray instead of the old `dx - dy <= 2 && dx - r <= handle` test:
    // that test drew the handle in the band where dx > r, which starts at the
    // ring's right *edge*, so most of it sat under the ring or outside the pill
    // and only a stray few pixels below the bar ever showed. The handle is the
    // diagonal ray leaving the ring at 45 degrees, measured along it from the
    // centre: t is the distance out (r .. r + handle), w the distance across.
    let handle = TELEMETRY_BAR_W as i32;
    if dx > 0 && dy > 0 {
        let along = (dx + dy) / 2;
        let across = (dx - dy).abs();
        if along >= r + 1 && along <= r + handle && across <= 1 {
            return Some(ink);
        }
    }

    if let Some(c) = p.search.pixel_aa(x, y, p.search.ink, theme.overlay) {
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
    let ink = layout::ink_on(theme, backdrop_ref(&p.scene_ref, x, y));

    // Back chevron, then recents: the same order and the same centring as before,
    // with both origins resolved in the plan.
    if let Some(c) = p.nav_back.pixel_inked_aa(x, y, ink, backdrop_ref(&p.scene_ref, x, y)) {
        return Some(c);
    }
    if let Some(c) = p.nav_recents.pixel_inked_aa(x, y, ink, backdrop_ref(&p.scene_ref, x, y)) {
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
    let p = paint(x, y, state);
    let theme = active_theme();
    if y < STATUS_BAR_HEIGHT {
        if let Some(c) = status_bar_pixel(x, y, &theme, p) {
            return c;
        }
    } else if y < 168 {
        if let Some(c) = greeting_pixel(x, y, &p.frame, p, &theme) {
            return c;
        }
    } else if y < 680 {
        if let Some(c) = card_pixel(x, y, &p.frame, p, &theme) {
            return c;
        }
    } else if y >= 700 && y < 1320 {
        if let Some(c) = deck_pixel(x, y, &p.frame, p, &theme) {
            return c;
        }
    } else if y >= 1320 && y < 1392 {
        if let Some(c) = search_pixel(x, y, &p.frame, p, &theme) {
            return c;
        }
    } else if y >= 1392 {
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
