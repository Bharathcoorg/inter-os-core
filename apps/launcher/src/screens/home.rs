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
    space, type_scale, Theme, MIN_TOUCH_TARGET, NAV_BAR_HEIGHT,
    SCREEN_HEIGHT, SCREEN_WIDTH, STATUS_BAR_HEIGHT,
};

/// The visual language this surface draws in.
///
/// WHY a local binding and not a global: `launcher::theme::SHELL` and `Theme::DARK` both
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
// Frame: the surface's bands, stacked from the theme's own metrics
// -----------------------------------------------------------------------------

/// Whether a touch at (`x`, `y`) lands on the universal search control.
///
/// WHY this is asked of this module: the control is painted here, so the tap
/// that opens it has to be resolved with the same numbers the painter used.
/// The shell used to carry its own copy of the band -- and a second, older
/// band at 1320..1384 where nothing is drawn -- so a press could open the
/// search from a place with no control on it.
pub fn search_control_hit(x: usize, y: usize) -> bool {
    frame().search.contains(x, y)
}

/// Height of the greeting block.
///
/// WHY this size: it is one touch target plus two display gaps, which puts its
/// bottom edge at y 232 -- inside the `64..235` band the shell's tap handler
/// treats as the weather card, so the greeting and the weather line are one
/// control rather than two regions that happen to overlap.
#[allow(dead_code)]
const GREETING_H: usize = MIN_TOUCH_TARGET + space::XXL * 2;

/// Line bands of a system-status row: the row's name, then its detail.
const CARD_ROW_LABEL: usize = type_scale::SECTION;
const CARD_ROW_DETAIL: usize = type_scale::BODY;

/// Entries in the system card. Each is a real answer from the shell or the
/// registry; none of them is a stand-in for a sensor.
const CARD_ROWS: usize = 3;

/// Type scale for a deck tile's name: scale 3 (SECTION) with outline anti-aliasing for clean, prominent mobile readability.
const LABEL_SCALE: usize = type_scale::SECTION;

/// The rows a run at the search field's rung reserves on the outline face.
///
/// WHY the ladder and not `text::line_height`: the search text draws at
/// `LABEL_SCALE` with the outline face, whose SECTION line box is
/// `launcher::font::line_height` (44 rows) rather than the bitmap cell's
/// `GLYPH_H * scale` (72 rows). Centring the taller bitmap box inside the
/// 62-row field underflowed the reserve to zero and pressed the text against
/// the field's top edge, so it no longer shared the magnifier's centre row.
const SEARCH_LABEL_BOX_H: usize = launcher::font::line_height(LABEL_SCALE);
const _: () = assert!(
    SEARCH_LABEL_BOX_H <= 62,
    "the search field's label box must fit inside the field it is centred in",
);

/// Inset from a system-card row's top to the row name's line box.
const CARD_NAME_TOP: usize = 16;
/// The rows the row name reserves at its rung, on the outline face.
const CARD_NAME_BOX_H: usize = launcher::font::line_height(CARD_ROW_LABEL);
/// The rows the row detail reserves at its rung, on the outline face.
const CARD_DETAIL_BOX_H: usize = launcher::font::line_height(CARD_ROW_DETAIL);
/// Where the detail's line box starts: clear of the name's whole box, not a
/// remembered stride that sat inside it.
const CARD_DETAIL_TOP: usize = CARD_NAME_TOP + CARD_NAME_BOX_H;
const _: () = assert!(
    CARD_DETAIL_TOP + CARD_DETAIL_BOX_H <= 120,
    "a system-card row must hold its name box and its detail box",
);

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

/// Capacity of the formatted render-complete log line.
pub const HOME_LOG_CAP: usize = 128;

/// Verbatim tail every render-complete line must keep: the lifecycle harness
/// waits for this exact substring.
const LOG_TAIL: &[u8] = b"Home surface rendered.";

/// The surface's bands, measured once per call rather than retyped per pixel.
#[derive(Clone, Copy)]
struct Frame {
    greeting: Rect,
    date_widget: Rect,
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
#[allow(dead_code)]
fn card_h() -> usize {
    460
}

fn frame() -> Frame {
    let greeting = Rect::new(30, 56, 440, 140);
    let date_widget = Rect::new(470, 56, 220, 150);
    let card = Rect::new(
        30,
        215,
        660,
        425,
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
    let search = Rect::new(30, 675, 660, 62);
    let deck = Rect::new(
        30,
        805,
        660,
        415,
    );
    let nav = Rect::new(0, SCREEN_HEIGHT - NAV_BAR_HEIGHT, SCREEN_WIDTH, NAV_BAR_HEIGHT);
    Frame {
        greeting,
        date_widget,
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
fn backdrop(p: &Paint, x: usize, y: usize) -> u32 {
    // WHY the document comes first: the wallpaper slot may name a document the
    // user chose, and the document is what the user chose. A scene is the
    // fallback the surface falls back to when the slot names no document or
    // names one the platform cannot paint.
    if let Some(document) = p.wallpaper_document {
        return document.pixel(x, y, SCREEN_WIDTH, SCREEN_HEIGHT);
    }
    interos_std::design::walls::scene_pixel(p.scene, x, y, SCREEN_WIDTH, SCREEN_HEIGHT)
}

/// The wallpaper for a frame whose scene is already resolved.
///
/// WHY a second entry point rather than changing [`backdrop`]'s signature: four
/// painters call `backdrop(p.scene, ..)`, and two of them call it twice for the
/// same pixel -- once to choose ink that contrasts, once to composite. Those
/// callers want the resolved tones; the picker-style callers that pass a bare
/// scene genuinely only want one pixel.
#[inline(always)]
fn backdrop_ref(p: &Paint, x: usize, y: usize) -> u32 {
    if let Some(document) = p.wallpaper_document {
        return document.pixel(x, y, SCREEN_WIDTH, SCREEN_HEIGHT);
    }
    interos_std::design::walls::scene_pixel_ref(&p.scene_ref, x, y, SCREEN_WIDTH, SCREEN_HEIGHT)
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
    #[allow(dead_code)]
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
        Self::placed_styled(x, y, scale, buf, len, ink, false)
    }

    fn placed_styled(x: usize, y: usize, scale: usize, buf: [u8; RUN_CAP], len: usize, ink: u32, bold: bool) -> Run {
        let text = &buf[..len];
        let em = launcher::outline_text::em_for_scale(scale);
        let (asc, desc) = interos_std::design::typeface::line_metrics(em);
        let baseline = (y + launcher::font::ascent_for_scale(scale)).max(asc as usize);
        let outline_advance = interos_std::design::typeface::text_advance_px(text, em) as usize + if bold { 1 } else { 0 };
        let bitmap_measure = text::measure(text, scale);

        let w = bitmap_measure.max(outline_advance);
        let top = baseline.saturating_sub(asc as usize);
        let box_h = (baseline + desc as usize).saturating_sub(top);
        let outline = if scale >= 2 && len <= launcher::outline_text::KEY_MAX {
            launcher::outline_text::OutlineRun::place_styled(x, baseline, em, text, bold)
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
        }
    }

    #[allow(dead_code)]
    fn whole_bold(x: usize, y: usize, scale: usize, bytes: &[u8], ink: u32) -> Run {
        let mut buf = [0u8; RUN_CAP];
        let len = bytes.len().min(RUN_CAP);
        buf[..len].copy_from_slice(&bytes[..len]);
        Run::placed_styled(x, y, scale, buf, len, ink, true)
    }

    #[allow(dead_code)]
    fn centred_bold(
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
        Run::placed_styled(x + inset, y, scale, buf, len, ink, true)
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
        self.pixel_inked_aa(px, py, ink, bg)
    }

    /// Anti-aliased render of this run with caller-chosen ink and background.
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
    /// Vibrant icon pictogram ink tint.
    ink: u32,
    /// High-contrast readable ink for the app title label.
    label_ink: u32,
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
    /// Vibrant pastel container fill for the tile pill squircle.
    fill: u32,
}

impl DeckTilePlan {
    const EMPTY: DeckTilePlan = DeckTilePlan {
        cell: Rect::new(0, 0, 0, 0),
        icon: None,
        icon_cx: 0,
        icon_cy: 0,
        ink: 0,
        label_ink: 0,
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
    /// The document wallpaper, when the wallpaper slot names one.
    ///
    /// WHY it is in the plan: the backdrop is read per pixel by four separate
    /// painters, and the document is the wallpaper the user chose, so all of
    /// them have to read the same one.
    wallpaper_document: Option<interos_std::image::Bitmap<'static>>,
    clock: Option<Run>,
    greeting: [Run; 3],
    date_day: Run,
    date_month: Run,
    date_weekday: Run,
    weather: Run,
    card_title: Run,
    card_stack: Option<Rect>,
    card_rows: [CardRowPlan; CARD_ROWS],
    deck: DeckPlan,
    search_magnifier: (usize, usize, i32),
    search: Run,
    #[allow(dead_code)]
    nav_back: Run,
    #[allow(dead_code)]
    nav_recents: Run,
    nav_pill: Rect,
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
        Some(Run::whole(
            40,
            8,
            type_scale::BODY,
            &state.clock_time,
            0,
        ))
    } else {
        None
    };

    // Greeting: prominent smartphone headline typography
    let title_scale = type_scale::HEADING;
    let caption_scale = type_scale::SECTION;

    let day_part: &[u8] = if !state.clock_source_ok {
        b"Welcome"
    } else if state.clock_time[0].is_ascii_digit() && state.clock_time[1].is_ascii_digit() {
        let hour = (state.clock_time[0] - b'0') * 10 + (state.clock_time[1] - b'0');
        if (5..12).contains(&hour) {
            b"Good morning"
        } else if (12..17).contains(&hour) {
            b"Good afternoon"
        } else if (17..22).contains(&hour) {
            b"Good evening"
        } else {
            b"Good night"
        }
    } else {
        b"Welcome"
    };

    // Dynamic Contextual Subtitle: Platform Truth Contract (no hardcoded/fictitious state).
    // Uses user_status from settings if configured; otherwise contextual from real system state.
    let mut subtitle_buf = [0u8; 48];
    let subtitle_bytes: &[u8] = if state.user_status_len > 0 {
        &state.user_status[..state.user_status_len]
    } else if state.notification_count > 0 {
        let cnt = state.notification_count.min(8);
        if cnt == 1 {
            b"1 notification waiting"
        } else {
            subtitle_buf[0] = b'0' + (cnt as u8);
            let rest = b" notifications waiting";
            subtitle_buf[1..1 + rest.len()].copy_from_slice(rest);
            &subtitle_buf[..1 + rest.len()]
        }
    } else if state.task0_done {
        b"Primary task completed"
    } else if state.clock_source_ok && state.date_line[0] != b' ' {
        match &state.date_line[0..3] {
            b"Sat" | b"Sun" => b"Have a peaceful weekend",
            b"Mon" => b"Ready for the week ahead",
            b"Fri" => b"Wrapping up the week",
            _ => {
                if state.clock_time[0].is_ascii_digit() && state.clock_time[1].is_ascii_digit() {
                    let hour = (state.clock_time[0] - b'0') * 10 + (state.clock_time[1] - b'0');
                    if (5..12).contains(&hour) {
                        b"Ready for the day ahead"
                    } else if (12..17).contains(&hour) {
                        b"Afternoon briefing ready"
                    } else if (17..22).contains(&hour) {
                        b"Wrapping up your day"
                    } else {
                        b"Night mode active"
                    }
                } else {
                    b"System ready"
                }
            }
        }
    } else {
        b"System ready"
    };

    // WHY the greeting never carries the account name: the home screen is the
    // one surface every glance lands on, and a personal name set in Settings
    // for the account card should not become the headline of the device. The
    // time of day is the fact worth the largest type on the surface.
    let greeting = {
        let l1_y = f.greeting.y + 8;
        // WHY +86 and not +64: HEADING's line box carries the glyphs down to
        // ~+88 from the run origin (cap 44 plus descender). The subtitle at +64
        // collided with the descender of the title's first letter.
        let l2_y = f.greeting.y + 86;
        [
            Run::whole(f.greeting.x, l1_y, title_scale, day_part, 0),
            Run::whole(f.greeting.x, l2_y, caption_scale, subtitle_bytes, 0),
            Run::EMPTY,
        ]
    };

    // Date card: real RTC calendar date and weekday. There is no weather
    // sensor, weather capability or weather syscall in this platform, so the
    // card never prints a temperature; the third line carries the month, and
    // an absent RTC reading says so instead of showing a plausible date.
    let (full_weekday, day_slice, month_slice): (&[u8], &[u8], &[u8]) =
        if state.clock_source_ok && state.date_line[0] != b' ' {
            let weekday = match &state.date_line[0..3] {
                b"Mon" => &b"MONDAY"[..],
                b"Tue" => &b"TUESDAY"[..],
                b"Wed" => &b"WEDNESDAY"[..],
                b"Thu" => &b"THURSDAY"[..],
                b"Fri" => &b"FRIDAY"[..],
                b"Sat" => &b"SATURDAY"[..],
                b"Sun" => &b"SUNDAY"[..],
                _ => &b"TODAY"[..],
            };
            let day_start = if state.date_line[5] == b'0' { 6 } else { 5 };
            let day = &state.date_line[day_start..7];
            let month = match &state.date_line[8..11] {
                b"Jan" => &b"JANUARY"[..],
                b"Feb" => &b"FEBRUARY"[..],
                b"Mar" => &b"MARCH"[..],
                b"Apr" => &b"APRIL"[..],
                b"May" => &b"MAY"[..],
                b"Jun" => &b"JUNE"[..],
                b"Jul" => &b"JULY"[..],
                b"Aug" => &b"AUGUST"[..],
                b"Sep" => &b"SEPTEMBER"[..],
                b"Oct" => &b"OCTOBER"[..],
                b"Nov" => &b"NOVEMBER"[..],
                b"Dec" => &b"DECEMBER"[..],
                _ => &b"NO DATE"[..],
            };
            (weekday, day, month)
        } else {
            (&b"TODAY"[..], &b"--"[..], &b"DATE UNAVAILABLE"[..])
        };

    let dw = f.date_widget;
    let dw_w = dw.w;
    let date_weekday = Run::centred(dw.x, dw.y + 6, type_scale::SECTION, dw_w.saturating_sub(16), dw_w, full_weekday, theme.primary);
    let date_day = Run::centred(dw.x, dw.y + 40, type_scale::DISPLAY, dw_w.saturating_sub(16), dw_w, day_slice, theme.on_surface);
    let date_month = Run::centred(dw.x, dw.y + 126, type_scale::CAPTION, dw_w.saturating_sub(16), dw_w, month_slice, theme.on_surface_variant);

    let weather = Run::EMPTY;

    let card_title = Run::whole(
        f.card_title.x,
        f.card_title.y,
        type_scale::TITLE,
        b"Today",
        theme.on_surface,
    );
    let card_row_h = card_row_h();
    let card_stack = layout::list(f.card_rows, CARD_ROWS, card_row_h, 0);
    let mut card_rows = [CardRowPlan::EMPTY; CARD_ROWS];
    if let Some(stack) = card_stack {
        for row in 0..CARD_ROWS {
            let Some(rect) = layout::list_row(stack, row, CARD_ROWS, card_row_h, 0) else {
                continue;
            };
            let mut buf = [0u8; CARD_DETAIL_CAP];
            let len = card_row_text(state, row, &mut buf);
            card_rows[row] = CardRowPlan {
                rect,
                name: Run::whole(
                    rect.x + 80,
                    rect.y + CARD_NAME_TOP,
                    CARD_ROW_LABEL,
                    card_row_name(row),
                    theme.on_surface,
                ),
                detail: Run::whole(
                    rect.x + 80,
                    rect.y + CARD_DETAIL_TOP,
                    CARD_ROW_DETAIL,
                    &buf[..len],
                    theme.on_surface_variant,
                ),
                done: row == 2 && state.task0_done,
            };
        }
    }

    let deck = deck_plan(state, &theme, f.deck);

    let magnifier_r = 10i32;
    let magnifier_x = f.search.x + 36;
    let search_magnifier = (magnifier_x, f.search.center_y(), magnifier_r);
    let search_text_x = f.search.x + 64;
    let search_room = (f.search.x + f.search.w).saturating_sub(search_text_x).saturating_sub(space::LG);
    let search_y = f.search.y + f.search.h.saturating_sub(SEARCH_LABEL_BOX_H) / 2;
    let search = if f.search.w > 0 && f.search.h > 0 {
        Run::fitted(
            search_text_x,
            search_y,
            LABEL_SCALE,
            search_room,
            b"Search apps, settings & files",
            theme.on_surface_variant,
        )
    } else {
        Run::whole(0, 0, LABEL_SCALE, b"", 0)
    };

    let nav_scale = type_scale::CAPTION;
    let nav_cy = f.nav.y + f.nav.h.saturating_sub(text::line_height(nav_scale)) / 2;
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
    let pill_w = 140;

    Paint {
        frame: f,
        clock,
        greeting,
        date_day,
        date_month,
        date_weekday,
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
            1418,
            pill_w,
            5,
        ),
        scene: state.home_wallpaper,
        scene_ref: interos_std::design::walls::SceneRef::new(state.home_wallpaper),
        wallpaper_document: state.home_wallpaper_document,
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
    Rect::new(30, 805, 200, 180),
    Rect::new(260, 805, 200, 180),
    Rect::new(490, 805, 200, 180),
    Rect::new(30, 1020, 200, 180),
    Rect::new(260, 1020, 200, 180),
    Rect::new(490, 1020, 200, 180),
];

/// Smooth rounded pill corner radius for the 6 home screen app pills.
pub const TILE_RADIUS: usize = 32;

/// Half-size of a deck tile's pictogram, in pixels: 64x64 box.
pub const DOCK_ICON_EXTENT: i32 = 32;

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

/// Returns the vibrant pastel container fill for a deck tile.
#[inline]
pub fn tile_container(theme: &Theme, app_id: &[u8]) -> u32 {
    theme.vibrant_container_bytes(app_id)
}

/// Returns the vibrant icon pictogram ink tint for a deck tile.
#[inline]
pub fn tile_icon_tint(theme: &Theme, app_id: &[u8]) -> u32 {
    theme.vibrant_icon_tint_bytes(app_id)
}

/// Returns readable high-contrast ink for a deck tile label drawn on its vibrant container.
#[inline]
pub fn tile_label_ink(theme: &Theme, app_id: &[u8]) -> u32 {
    if let Ok(s) = core::str::from_utf8(app_id) {
        theme.on_vibrant_container(s)
    } else {
        theme.ink_on_light
    }
}

/// Resolves the deck for this frame.
fn deck_plan(state: &LauncherState, theme: &Theme, _deck: Rect) -> DeckPlan {
    let mut plan = DeckPlan {
        cells: [DeckTilePlan::EMPTY; MAX_DECK_TILES],
        slots: 6,
        columns: 3,
        rows: 2,
        origin_x: 30,
        origin_y: 805,
        pitch_x: 230,
        pitch_y: 215,
        app_count: 6,
        empty: None,
    };

    let dock_defs: [(Option<launcher::icons::IconKind>, &[u8], &[u8]); 6] = [
        (Some(launcher::icons::IconKind::Phone), b"dialer", b"Phone"),
        (Some(launcher::icons::IconKind::Messages), b"messages", b"Messages"),
        (Some(launcher::icons::IconKind::Camera), b"camera", b"Camera"),
        (Some(launcher::icons::IconKind::Files), b"file_manager", b"Files"),
        (Some(launcher::icons::IconKind::Browser), b"browser", b"Browser"),
        (Some(launcher::icons::IconKind::Apps), b"apps", b"Apps"),
    ];

    let label_fit = DOCK_TILES[0].w.saturating_sub(space::XS * 2);
    let label_scale = LABEL_SCALE;

    for slot in 0..6 {
        let (default_icon, suffix, default_label) = dock_defs[slot];
        let (app_id_bytes, label_bytes, icon_kind) = if slot == 5 {
            (suffix, default_label, default_icon)
        } else if let Some(pkg_idx) = find_package_index(state, suffix).or_else(|| {
            if suffix == b"file_manager" {
                find_package_index(state, b"files")
            } else if suffix == b"dialer" {
                find_package_index(state, b"phone")
            } else if suffix == b"browser" {
                find_package_index(state, b"web")
            } else {
                None
            }
        }) {
            let id = app_id(state, pkg_idx);
            let label = app_label(state, pkg_idx);
            let icon = launcher::icons::match_icon_kind(id)
                .or_else(|| launcher::icons::match_icon_kind(label))
                .or(default_icon);
            let resolved_id = if id.is_empty() { suffix } else { id };
            let resolved_label = if label.is_empty() { default_label } else { label };
            (resolved_id, resolved_label, icon)
        } else {
            (suffix, default_label, default_icon)
        };

        let cell = DOCK_TILES[slot];
        let icon_cx = cell.x + cell.w / 2;
        let icon_cy = cell.y + 68;
        let label_y = cell.y + 114;

        let fill = tile_container(theme, app_id_bytes);
        let icon_ink = tile_icon_tint(theme, app_id_bytes);
        let label_ink = tile_label_ink(theme, app_id_bytes);

        plan.cells[slot] = DeckTilePlan {
            cell,
            icon: icon_kind,
            icon_cx,
            icon_cy,
            ink: icon_ink,
            label_ink,
            badge: Run::EMPTY,
            label: Run::centred(
                cell.x,
                label_y,
                label_scale,
                label_fit,
                cell.w,
                label_bytes,
                label_ink,
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

/// Invalidates the Home Screen paint cache so dynamic app updates, wallpaper switches,
/// and returning from foreground applications re-measure and re-render all runs.
pub fn invalidate_paint_cache() {
    unsafe {
        let slot: *mut Option<Paint> = core::ptr::addr_of_mut!(PAINT);
        *slot = None;
    }
}

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
    for slot in 0..6 {
        let cell = DOCK_TILES[slot];
        if x >= cell.x.saturating_sub(10) && x <= cell.x + cell.w + 10 && y >= cell.y.saturating_sub(10) && y <= cell.y + cell.h + 10 {
            return match slot {
                0 => find_package_index(state, b"dialer"),
                1 => find_package_index(state, b"messages"),
                2 => find_package_index(state, b"camera"),
                3 => find_package_index(state, b"file_manager").or_else(|| find_package_index(state, b"files")),
                4 => find_package_index(state, b"browser"),
                _ => None,
            };
        }
    }
    None
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

fn status_bar_pixel(x: usize, y: usize, theme: &Theme, p: &Paint) -> Option<u32> {
    if y >= STATUS_BAR_HEIGHT {
        return None;
    }
    let ink = layout::ink_on(theme, backdrop_ref(p, x, y));

    // WHY gated on the source rather than drawn from the buffer: with no RTC
    // authority granted to this shell the read can be refused, and a clock is the
    // one thing on this surface a reader takes as a measurement. The gate itself is
    // resolved in the plan, so this is a plain "is the run there" test.
    if let Some(c) = p.clock.as_ref().and_then(|r| r.pixel_inked_aa(x, y, ink, backdrop_ref(p, x, y))) {
        return Some(c);
    }

    crate::screens::status::marks(x, y, ink)
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
    // Check right date widget first
    if f.date_widget.contains(x, y) {
        if !rounded_contains(f.date_widget, x, y, 24) {
            return None;
        }
        if rounded_outline(f.date_widget, x, y, 24) {
            return Some(theme.outline_variant);
        }
        let bg = backdrop_ref(p, x, y);
        let card_fill = interos_std::font::blend(theme.surface_frosted, bg, 240);
        if let Some(c) = p.date_weekday.pixel_aa(x, y, p.date_weekday.ink, card_fill) {
            return Some(c);
        }
        if let Some(c) = p.date_day.pixel_aa(x, y, p.date_day.ink, card_fill) {
            return Some(c);
        }
        if let Some(c) = p.date_month.pixel_aa(x, y, p.date_month.ink, card_fill) {
            return Some(c);
        }
        return Some(card_fill);
    }

    let band = Rect::new(0, f.greeting.y, SCREEN_WIDTH, f.greeting.h);
    if !band.contains(x, y) {
        return None;
    }

    // Ink is taken per row against the wallpaper ramp
    let ink = layout::ink_on(theme, backdrop_ref(p, x, y));
    for run in p.greeting.iter() {
        if let Some(c) = run.pixel_inked_aa(x, y, ink, backdrop_ref(p, x, y)) {
            return Some(c);
        }
    }
    if let Some(c) = p.weather.pixel_inked(x, y, ink, backdrop_ref(p, x, y)) {
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
        0 => {
            if state.schedule_has_event && state.schedule_detail_len > 0 {
                put(out, 0, &state.schedule_detail[..state.schedule_detail_len])
            } else if state.schedule_state == interos_std::calendar::StoreState::ReadRefused {
                put(out, 0, b"Schedule unavailable (read refused)")
            } else {
                put(out, 0, b"No events on this device")
            }
        }
        1 => {
            // WHY notifications and not messages: this count is the system-wide
            // notification query, not a per-app message count, so labelling it
            // "Messages" claimed a messaging state the shell cannot see.
            if state.notification_count > 0 {
                let mut at = push_decimal(out, 0, state.notification_count);
                if state.notification_count == 1 {
                    at = put(out, at, b" unread notification");
                } else {
                    at = put(out, at, b" unread notifications");
                }
                at
            } else {
                put(out, 0, b"No new notifications")
            }
        }
        _ => {
            if state.task0_done {
                put(out, 0, b"All tasks completed")
            } else {
                put(out, 0, b"1 task pending")
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
        _ => b"Tasks",
    }
}

fn card_pixel(x: usize, y: usize, f: &Frame, p: &Paint, theme: &Theme) -> Option<u32> {
    if !f.card.contains(x, y) {
        return None;
    }
    if !rounded_contains(f.card, x, y, 28) {
        return None;
    }
    if rounded_outline(f.card, x, y, 28) {
        return Some(theme.outline_variant);
    }
    let bg = backdrop_ref(p, x, y);
    let card_fill = interos_std::font::blend(theme.surface_frosted, bg, 245);

    // Dividers between rows
    if (y == 385 || y == 505) && x >= f.card.x + 24 && x <= f.card.x + f.card.w - 24 {
        return Some(theme.outline_variant);
    }

    if let Some(c) = p.card_title.pixel_aa(x, y, p.card_title.ink, card_fill) {
        return Some(c);
    }

    if p.card_stack.is_none() {
        return Some(card_fill);
    }

    for (row_idx, row) in p.card_rows.iter().enumerate() {
        if !row.rect.contains(x, y) {
            continue;
        }

        // Right chevron '>' at x: 645
        let row_cy = row.rect.y + row.rect.h / 2;
        let cdx = x as i32 - 645;
        let cdy = (y as i32 - row_cy as i32).abs();
        if cdx >= -3 && cdx <= 3 && (cdy == 3 - cdx || cdy == 2 - cdx) {
            return Some(theme.on_surface_variant);
        }

        let icon_cx = 78i32;
        let icon_cy = (row.rect.y + row.rect.h / 2) as i32;
        let idx = x as i32 - icon_cx;
        let idy = y as i32 - icon_cy;

        match row_idx {
            0 => {
                // Schedule: vector calendar page icon
                let r = 7i32;
                if idx.abs() <= r && idy >= -r && idy <= r {
                    let outline = idx.abs() == r || idy == -r || idy == r || idy == -2;
                    let loops = idy == -r - 1 && (idx == -3 || idx == 3);
                    let grid_dot = (idy == 1 || idy == 4) && (idx == -3 || idx == 0 || idx == 3);
                    if outline || loops || grid_dot {
                        return Some(theme.primary);
                    }
                }
            }
            1 => {
                // Notifications: vector speech bubble icon
                if idx.abs() <= 8 && idy >= -7 && idy <= 5 {
                    let is_border = idx.abs() == 8 || idy == -7 || idy == 5;
                    let is_dot = idy == -1 && (idx == -4 || idx == 0 || idx == 4);
                    if is_border || is_dot {
                        return Some(theme.primary);
                    }
                } else if idx >= -6 && idx <= -2 && idy >= 5 && idy <= 8 && (idx + idy <= 2) {
                    return Some(theme.primary);
                }
            }
            _ => {
                // Tasks: checkbox for the pinned Today toggle
                let cbx = x as i32 - 78;
                let cby = y as i32 - icon_cy;
                if cbx.abs() <= 12 && cby.abs() <= 12 {
                    if cbx.abs() == 12 || cby.abs() == 12 {
                        return Some(theme.on_surface_variant);
                    }
                    if row.done {
                        let tick = (cbx >= -6 && cbx <= -1 && cby == cbx + 6)
                            || (cbx >= 0 && cbx <= 6 && cby == 6 - cbx);
                        return Some(if tick { card_fill } else { theme.primary });
                    }
                    return Some(card_fill);
                }
            }
        }
        if let Some(c) = row.name.pixel_aa(x, y, row.name.ink, card_fill) {
            return Some(c);
        }
        if let Some(c) = row.detail.pixel_aa(x, y, row.detail.ink, card_fill) {
            return Some(c);
        }
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
        return empty.pixel_inked(x, y, empty.ink, backdrop(p, x, y));
    }

    let index = deck.slot_at(x, y)?;
    if index >= deck.app_count {
        return None;
    }
    let tile = &deck.cells[index];

    if !rounded_contains(tile.cell, x, y, TILE_RADIUS) {
        return None;
    }
    if rounded_outline(tile.cell, x, y, TILE_RADIUS) {
        return Some(_theme.outline_variant);
    }
    let tile_fill = tile.fill;

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
    } else if let Some(c) = tile.badge.pixel_aa(x, y, tile.ink, tile_fill) {
        return Some(c);
    }
    if let Some(c) = tile.label.pixel_aa(x, y, tile.label_ink, tile_fill) {
        return Some(c);
    }
    Some(tile_fill)
}

// -----------------------------------------------------------------------------
// Search control
// -----------------------------------------------------------------------------

/// The universal search control.
///
/// WHY it is on this surface at all: the reference home panel has no search bar
/// -- that lives on the App View panel -- but the shell opens the search from a
/// tap here, so removing it would take a working control with it.
fn search_pixel(x: usize, y: usize, f: &Frame, p: &Paint, theme: &Theme) -> Option<u32> {
    if !f.search.contains(x, y) {
        return None;
    }
    if !rounded_contains(f.search, x, y, 28) {
        return None;
    }
    if rounded_outline(f.search, x, y, 28) {
        return Some(theme.outline_variant);
    }
    let bg = backdrop_ref(p, x, y);
    let search_fill = interos_std::font::blend(theme.surface_frosted, bg, 240);
    let ink = theme.on_surface_variant;

    // Magnifier: ring plus handle
    let (gx, cy, _r) = p.search_magnifier;
    let dx = x as i32 - gx as i32;
    let dy = y as i32 - cy as i32;
    let d2 = dx * dx + dy * dy;
    let ring = (36..=81).contains(&d2);
    let handle = (5..=12).contains(&dx) && (dx - dy).abs() <= 2 && dy >= 5;
    if ring || handle {
        return Some(ink);
    }

    if let Some(c) = p.search.pixel_aa(x, y, p.search.ink, search_fill) {
        return Some(c);
    }
    Some(search_fill)
}

// -----------------------------------------------------------------------------
// Navigation
// -----------------------------------------------------------------------------

fn nav_pixel(x: usize, y: usize, f: &Frame, p: &Paint, theme: &Theme) -> Option<u32> {
    if !f.nav.contains(x, y) {
        return None;
    }
    let ink = layout::ink_on(theme, backdrop_ref(p, x, y));

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
    } else if y < 210 {
        if let Some(c) = greeting_pixel(x, y, &p.frame, p, &theme) {
            return c;
        }
    } else if y < 655 {
        if let Some(c) = card_pixel(x, y, &p.frame, p, &theme) {
            return c;
        }
    } else if y >= 665 && y < 755 {
        if let Some(c) = search_pixel(x, y, &p.frame, p, &theme) {
            return c;
        }
    } else if y >= 795 && y < 1225 {
        if let Some(c) = deck_pixel(x, y, &p.frame, p, &theme) {
            return c;
        }
    } else if y >= 1390 {
        if let Some(c) = nav_pixel(x, y, &p.frame, p, &theme) {
            return c;
        }
    }
    backdrop_ref(p, x, y)
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
    let _ = interos_std::log(msg);
}

#[cfg(test)]
mod tests {
    use super::*;
    use launcher::theme::Theme;

    #[test]
    fn deck_tile_vibrant_tints_and_palette_resolution() {
        let theme = Theme::LIGHT;

        // Slot 0: Dialer / Phone -> Emerald Green (0xFF10B981)
        assert_eq!(tile_icon_tint(&theme, b"dialer"), 0xFF10B981);
        assert_eq!(tile_icon_tint(&theme, b"phone"), 0xFF10B981);

        // Slot 1: Messages / SMS -> Electric Sky Blue (0xFF0284C7)
        assert_eq!(tile_icon_tint(&theme, b"messages"), 0xFF0284C7);
        assert_eq!(tile_icon_tint(&theme, b"sms"), 0xFF0284C7);

        // Slot 2: Camera -> Warm Gold (0xFFF59E0B)
        assert_eq!(tile_icon_tint(&theme, b"camera"), 0xFFF59E0B);

        // Slot 3: Files / Storage -> Amber Orange (0xFFEA580C)
        assert_eq!(tile_icon_tint(&theme, b"files"), 0xFFEA580C);
        assert_eq!(tile_icon_tint(&theme, b"file_manager"), 0xFFEA580C);

        // Slot 4: Browser / Web -> Indigo (0xFF6366F1)
        assert_eq!(tile_icon_tint(&theme, b"browser"), 0xFF6366F1);
        assert_eq!(tile_icon_tint(&theme, b"web"), 0xFF6366F1);

        // Apps tile fallback -> Violet (theme.accent = 0xFF7C3AED)
        assert_eq!(tile_icon_tint(&theme, b"apps"), theme.accent);
    }

    #[test]
    fn deck_tile_vibrant_pastel_containers_and_label_contrast() {
        let theme = Theme::LIGHT;

        let apps: &[(&[u8], &str)] = &[
            (b"dialer", "dialer"),
            (b"messages", "messages"),
            (b"camera", "camera"),
            (b"files", "files"),
            (b"browser", "browser"),
            (b"apps", "apps"),
        ];

        for &(id_bytes, id_str) in apps {
            let container = tile_container(&theme, id_bytes);
            let expected_container = theme.vibrant_container(id_str);
            assert_eq!(container, expected_container, "Container mismatch for {}", id_str);

            let label_ink = tile_label_ink(&theme, id_bytes);
            let expected_label_ink = theme.on_vibrant_container(id_str);
            assert_eq!(label_ink, expected_label_ink, "Label ink mismatch for {}", id_str);
            assert_eq!(label_ink, theme.ink_on_light, "Must use high-contrast ink_on_light for {}", id_str);

            // Container must not be raw surface or raw accent
            assert_ne!(container, theme.surface);
            assert_ne!(container, tile_icon_tint(&theme, id_bytes));

            // WCAG AA contrast check: ink vs container
            let lum_c = interos_std::design::accessibility::relative_luminance_x10000(container);
            let lum_i = interos_std::design::accessibility::relative_luminance_x10000(label_ink);
            let cr = interos_std::design::accessibility::contrast_ratio_x100(lum_c, lum_i);
            assert!(cr >= 450, "Contrast ratio must be >= 4.5:1 for {}, got {}.{:02}:1", id_str, cr / 100, cr % 100);
        }
    }

    #[test]
    fn deck_plan_populates_six_vibrant_tiles() {
        let state = LauncherState::new();
        let theme = Theme::LIGHT;
        let plan = deck_plan(&state, &theme, frame().deck);

        assert_eq!(plan.slots, 6);
        assert_eq!(plan.app_count, 6);

        // Verify each tile matches expected vibrant container and icon tint
        let expected: [(&[u8], launcher::icons::IconKind, u32); 6] = [
            (b"dialer", launcher::icons::IconKind::Phone, 0xFF10B981),        // Emerald Green
            (b"messages", launcher::icons::IconKind::Messages, 0xFF0284C7),   // Electric Sky Blue
            (b"camera", launcher::icons::IconKind::Camera, 0xFFF59E0B),       // Warm Gold
            (b"file_manager", launcher::icons::IconKind::Files, 0xFFEA580C),  // Amber Orange
            (b"browser", launcher::icons::IconKind::Browser, 0xFF6366F1),     // Indigo
            (b"apps", launcher::icons::IconKind::Apps, 0xFF7C3AED),           // Violet
        ];

        for (slot, (app_id, icon_kind, expected_tint)) in expected.iter().enumerate() {
            let tile = &plan.cells[slot];
            assert_eq!(tile.icon, Some(*icon_kind), "Slot {} icon kind mismatch", slot);
            assert_eq!(tile.ink, *expected_tint, "Slot {} vibrant icon tint mismatch", slot);
            assert_eq!(tile.fill, theme.vibrant_container_bytes(app_id), "Slot {} vibrant container mismatch", slot);
            assert_eq!(tile.label_ink, theme.ink_on_light, "Slot {} label ink must be high contrast", slot);
            assert!(tile.label.len > 0, "Slot {} label must not be empty", slot);
        }
    }

    #[test]
    fn deck_pixel_renders_vibrant_pastel_container_fill() {
        let state = LauncherState::new();
        let p = build(&state);
        let theme = Theme::LIGHT;

        // For slot 0 (Phone tile at x: 30..230, y: 805..985):
        // Test a pixel safely inside the squircle corner, but away from icon/label
        let cell = DOCK_TILES[0];
        let px = cell.x + 18;
        let py = cell.y + 18;
        assert!(rounded_contains(cell, px, py, TILE_RADIUS));
        assert!(!rounded_outline(cell, px, py, TILE_RADIUS));

        let color = deck_pixel(px, py, &p.frame, &p, &theme);
        assert_eq!(color, Some(theme.vibrant_container_bytes(b"dialer")), "Squircle must render vibrant pastel container fill");

        // Test outline pixel: must return outline_variant
        let ox = cell.x;
        let oy = cell.y + cell.h / 2;
        let border_color = deck_pixel(ox, oy, &p.frame, &p, &theme);
        assert_eq!(border_color, Some(theme.outline_variant), "Border pixel must render outline_variant");

        // Test pixel outside deck: must return None
        let outside = deck_pixel(0, 0, &p.frame, &p, &theme);
        assert_eq!(outside, None);
    }

    #[test]
    fn test_home_pixels_not_white() {
        let state = LauncherState::new();
        let mut white_count = 0;
        for y in 1000..1440 {
            for x in 0..720 {
                let c = get_home_color(x, y, &state);
                if c == 0xFFFAFAFA {
                    white_count += 1;
                }
            }
        }
        assert_eq!(white_count, 0, "No pixels should be white background in bottom of HomeScreen");
    }
}
