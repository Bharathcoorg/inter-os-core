//! Quick Controls: the control centre.
//!
//! WHY the composition follows the reference shade: `Ui Samples/07_quick_controls.png`
//! lays out four radio tiles in a two-up grid, a brightness slider, a volume
//! slider and three secondary tiles on a dark surface, and that order -- radios,
//! then levels, then secondary controls -- is the order of importance a phone's
//! control centre has. This panel keeps it.
//!
//! WHY every tile here says what is missing instead of a state: the guest has no
//! radio controller (`kernel/src/net/wireless.rs` reports `hardware_present:
//! false` for every band and telephony is forced to PowerDown/NoSim), no
//! backlight to dim, and no output level to read (`kernel/src/audio/mod.rs`
//! records volume as a stored request that no mixer consumes). A tile that said
//! "Connected", or a trough filled to 100%, would be indistinguishable on screen
//! from a radio that works. So the four radios are recessed slots printing the
//! kernel's own answer, and neither level control draws a fill.
//!
//! WHY three materials and not one card shape: on this surface a card means "a
//! tap here does something". The volume capsule is a card because a tap on it
//! stores a volume request and `main.rs` answers it. The four radio tiles are
//! recessed slots (`surface_sunken`, muted ink) because nothing answers them.
//! Every other reading is a flat row on the page background, separated by a
//! hairline. Making an inert control a card is how the previous version taught
//! the user that this panel lies.
//!
//! WHY only two rectangles are exported: `main.rs` hit-tests `VOLUME_SLIDER` and
//! `POWER_BTN`, and it reads both back from this module, so the drawn control
//! and the hit region are the same rectangle by construction. Every other row is
//! decorative -- nothing answers it -- so it is drawn as a reading rather than
//! as a control. `BRIGHTNESS_ROW` stays exported for the same "ask the painter"
//! reason, even though the brightness row is the one level control no backend
//! exists for.
//!
//! WHY the brightness row has no trough: with no backlight backend there is no
//! level to read, so a trough could never fill, and an empty trough is a dimmer
//! that does not exist. The row is therefore drawn as a reading -- the same flat
//! row the other inert controls use -- and `brightness_available` in
//! `QuickControlsFrame::new` is the one flag that would give it a track back.
//!
//! WHY the volume capsule has a trough but no fill: a tap on it *does* something
//! (`std::audio_set_volume` stores the request and the shell reports the applied
//! value in its HUD), so the well a tap lands in is honest. Nothing on this
//! platform measures emitted audio, so there is no level to fill it to; the
//! caption says what a tap does instead of painting a number, and the kernel's
//! own query refuses rather than answering 0 or a seeded 100.
//!
//! WHY the panel still has a dead strip: `main.rs` treats a tap with `x >= 600`
//! and `y >= 70` as "close the shade", and that strip crosses the drawn panel.
//! The Power pill is placed clear of it (asserted below). The volume capsule is
//! not, because its rectangle is also the linear tap-to-percent mapping and
//! narrowing it would misreport where a tap lands; the cost is that the right
//! ~13% of the capsule dismisses the shade instead of setting a volume. That is
//! a limitation of the handler in `main.rs`, not of the painter, and it is
//! called out here rather than papered over.

#![allow(dead_code)]

use launcher::layout::Rect;
use launcher::text;
use launcher::theme::{
    radius, space, type_scale, Theme, MIN_TOUCH_TARGET, SCREEN_HEIGHT, SCREEN_WIDTH,
    STATUS_BAR_HEIGHT,
};

use crate::screens::system_space::{decimal, push, round_contains, Reading};
use crate::LauncherState;

/// WHY `Theme::DARK`: the reference shade is a dark surface, and a control centre
/// is opened over the home surface rather than replacing it -- a light sheet here
/// would read as a different app. `Theme::DARK` is also the theme whose
/// `surface_sunken` exists to be a well, which is what an unavailable control on
/// this surface is.
const THEME: Theme = Theme::DARK;

/// The colour a bound level would fill its trough with.
///
/// WHY the brightest ink in the theme: the reference slider's fill is the one
/// high-contrast element on its dark surface, and `Theme::DARK.text` is that ink
/// without inventing a colour. Nothing on this platform reads a level today, so
/// this is the day-one value of a fill, not a painted one.
const LEVEL_FILL: u32 = Theme::DARK.text;

// ---------------------------------------------------------------------------
// Panel geometry
// ---------------------------------------------------------------------------

/// Left inset of the panel.
///
/// WHY 36: it gives the 720-wide scanout a visible margin on both sides, and it
/// leaves the dismiss strip `main.rs` reserves at `x >= 600` crossing the panel
/// rather than owning its edge -- which is why the two exported rectangles are
/// positioned with that strip in mind (see `POWER_BTN` and the asserts below).
const PANEL_X: usize = 36;

/// Panel width: the surface the shade occupies between its insets.
const PANEL_W: usize = SCREEN_WIDTH - PANEL_X * 2;

/// Side of the square icon box every row's glyph is drawn in.
const ICON: usize = 28;

/// Horizontal inset from a row's left edge to its text column: the icon gap, the
/// icon, the icon gap. Shared by every row so the glyphs line up.
const ROW_TEXT_INSET: usize = space::MD + ICON + space::MD;

/// Row label scale.
///
/// WHY BODY: a row has to hold one whole glyph cell of the shell's 16x24 face,
/// and BODY (2) is the smallest rung that reads as a row on a 720-wide surface.
/// The rungs are not changed here: this face renders at integer scales of its
/// cell only, so hierarchy comes from spacing and ink, not from a half-step the
/// rasteriser cannot draw.
const ROW_SCALE: usize = type_scale::BODY;

/// Value caption scale: one rung below the label, which is what makes a value a
/// value and not a second heading.
const VALUE_SCALE: usize = type_scale::CAPTION;

/// Height of a line at each scale, straight from the face.
const BODY_H: usize = text::GLYPH_H * ROW_SCALE;
const CAPTION_H: usize = text::GLYPH_H * VALUE_SCALE;

/// Gap between the heading and the summary line under it.
const HEADING_GAP: usize = space::XS;

/// Height of a flat reading row: the 48px cell plus `space::MD` above and below
/// it, which is the same rhythm the design system's rows use and keeps the icon
/// optically centred on the text rather than on the row's top edge.
const ROW_H: usize = space::MD + BODY_H + space::MD;

/// Height of a section label: one caption line.
const SECTION_H: usize = CAPTION_H;

/// Gap between one block's last pixel and the next section's label.
///
/// WHY `space::XL` and not `space::LG`: the panel is a shade with five blocks in
/// it, and a 16px gap reads as one long list rather than separate sections. The
/// label's own lead below is what ties it to the block it names.
const SECTION_GAP: usize = space::XL;

/// Gap between a section label and the first row of its section.
const SECTION_LEAD: usize = SECTION_H + space::SM;

/// Everything one section header occupies above its content.
const SECTION_BLOCK: usize = SECTION_GAP + SECTION_LEAD;

/// Gutter between the two blocks of the level section, and between the footer's
/// reading and the Power pill.
const LEVEL_GUTTER: usize = space::LG;

/// Height of a level control's trough.
///
/// WHY `space::MD`: a trough is a well, and at this scale that is visibly a
/// track without becoming a slab. It is rounded to a pill, so it never reads as
/// a flat bar with ends.
const LEVEL_TRACK_H: usize = space::MD;

/// Height of a level control: one text line, then its trough.
const LEVEL_H: usize = space::SM + BODY_H + space::SM + LEVEL_TRACK_H + space::MD;

/// Gutter between the tiles of the two-up radio grid.
const TILE_GUTTER: usize = space::MD;

/// Width of one radio tile: the panel split in two by that gutter.
const TILE_W: usize = (PANEL_W - TILE_GUTTER) / 2;

/// Rows of radio tiles.
///
/// WHY two: the reference composes the four radios as a two-up grid, and a
/// 316-wide tile holds a BODY label plus a CAPTION reason without ellipsising
/// them -- the widest label here measures 216px against the tile's 224px text
/// column.
const TILE_ROWS: usize = 2;

/// Horizontal padding inside a radio tile.
const TILE_PAD_X: usize = space::LG;

/// Text column inside a radio tile: what is left of the tile once the padding,
/// the icon and its gap are taken out.
const TILE_TEXT_W: usize = TILE_W - TILE_PAD_X - ICON - space::MD - TILE_PAD_X;

/// Vertical padding inside a radio tile, and the gap between its two lines.
const TILE_PAD_Y: usize = space::XL;
const TILE_TEXT_GAP: usize = space::SM;

/// Height of one radio tile: two lines of type inside that padding.
const TILE_H: usize = TILE_PAD_Y + BODY_H + TILE_TEXT_GAP + CAPTION_H + TILE_PAD_Y;

/// Height of the two-up radio grid.
const GRID_H: usize = TILE_H * TILE_ROWS + TILE_GUTTER * (TILE_ROWS - 1);

/// Rows in the secondary controls section.
const SYSTEM_ROWS: usize = 3;

/// Height of that section.
///
/// WHY the rows are contiguous with no gutter: they are separated by a hairline
/// instead, which is what makes a settings list read as one list. A gutter would
/// turn each row into its own chip.
const SYSTEM_LIST_H: usize = ROW_H * SYSTEM_ROWS;

/// Height of the footer's device reading row.
const FOOTER_H: usize = ROW_H;

/// Height of the Power entry pill.
///
/// WHY it is taller than a row: it is the panel's one primary action and the
/// design system's PrimaryButton, and 48px of type inside `space::LG` of padding
/// clears the minimum target without turning it into a slab.
const POWER_H: usize = space::LG + BODY_H + space::LG;

/// Width of the Power entry pill.
///
/// WHY it is content-derived: the 28px power glyph, a gap, the measured width of
/// "Power menu" at the caption scale, and `space::XL` of padding inside each
/// end, so the label can never touch the rounded edge. The measured run is
/// written as a literal because `text::measure` is not `const`; the tests in
/// `launcher::text` assert that measuring and rasterising agree, so the two
/// cannot drift apart.
const POWER_BTN_W: usize = ICON + space::MD + 116 + space::XL * 2;

/// Home indicator, drawn at the foot of the surface.
///
/// WHY the panel stops short of it: the gesture region below it is the system's,
/// and a row that ran under it would be a reading nobody can read.
const INDICATOR_W: usize = space::XL * 5;
const INDICATOR_H: usize = space::XS;

/// The panel's vertical band.
///
/// WHY these two anchors: the status bar is system chrome drawn above the
/// panel's first pixel, and the home indicator is reserved at the foot of the
/// scanout. Everything between them is this panel.
const LIST_BOTTOM: usize = SCREEN_HEIGHT - space::MD - INDICATOR_H - space::XS;
const AVAIL_TOP: usize = STATUS_BAR_HEIGHT + space::LG;
const AVAIL_BOT: usize = LIST_BOTTOM;

/// Height of the panel's content, block by block.
const CONTENT_H: usize = (BODY_H + HEADING_GAP + CAPTION_H)
    + SECTION_BLOCK
    + GRID_H
    + SECTION_BLOCK
    + (LEVEL_H + LEVEL_GUTTER + LEVEL_H)
    + SECTION_BLOCK
    + SYSTEM_LIST_H
    + SECTION_BLOCK
    + (FOOTER_H + LEVEL_GUTTER + POWER_H);

/// Vertical offset that centres the panel in its band.
///
/// WHY centred rather than top-anchored: the old panel ran out of things to say
/// about two thirds of the way down and left a dead band above the home
/// indicator. This one is tall enough that centring leaves balanced air above
/// and below instead of a trailing void, and the air is what keeps the shade
/// from reading as a page.
const CONTENT_DY: usize = (AVAIL_BOT - AVAIL_TOP - CONTENT_H) / 2;
const HEADING_Y: usize = AVAIL_TOP + CONTENT_DY;
const SUMMARY_Y: usize = HEADING_Y + BODY_H + HEADING_GAP;

const CONNECTIVITY_LABEL_Y: usize = SUMMARY_Y + CAPTION_H + SECTION_GAP;
const CONNECTIVITY_TOP: usize = CONNECTIVITY_LABEL_Y + SECTION_LEAD;
const LEVEL_LABEL_Y: usize = CONNECTIVITY_TOP + GRID_H + SECTION_GAP;
const LEVEL_TOP: usize = LEVEL_LABEL_Y + SECTION_LEAD;
const SYSTEM_LABEL_Y: usize = LEVEL_TOP + LEVEL_H + LEVEL_GUTTER + LEVEL_H + SECTION_GAP;
const SYSTEM_TOP: usize = SYSTEM_LABEL_Y + SECTION_LEAD;
const FOOTER_LABEL_Y: usize = SYSTEM_TOP + SYSTEM_LIST_H + SECTION_GAP;
const FOOTER_Y: usize = FOOTER_LABEL_Y + SECTION_LEAD;
const POWER_Y: usize = FOOTER_Y + FOOTER_H + LEVEL_GUTTER;

/// The brightness reading's row.
pub const BRIGHTNESS_ROW: Rect = Rect::new(PANEL_X, LEVEL_TOP, PANEL_W, LEVEL_H);

/// The volume capsule: the rectangle `main.rs` hit-tests, and the rectangle this
/// module draws, from one constant.
pub const VOLUME_SLIDER: Rect = Rect::new(PANEL_X, LEVEL_TOP + LEVEL_H + LEVEL_GUTTER, PANEL_W, LEVEL_H);

/// The Power entry pill.
///
/// WHY it is centred in the panel instead of sat at the right edge: the handler
/// in `main.rs` treats a tap with `x >= 600` (and `y >= 70`) as "close the
/// shade", so a pill at the panel's right edge would have its right third
/// dismiss the shade rather than open the Power menu. Centring keeps the whole
/// drawn pill inside the answerable region, and the assert below is what pins
/// that if the width ever changes.
pub const POWER_BTN: Rect = Rect::new(PANEL_X + (PANEL_W - POWER_BTN_W) / 2, POWER_Y, POWER_BTN_W, POWER_H);

/// The trough of a level control.
///
/// WHY a `const fn`: the trough's geometry has to be derived from its control's
/// rectangle inside a `const` initialiser, so the two cannot drift. A pill
/// inset from the control's sides, one text line below its top edge.
const fn level_track(control: Rect) -> Rect {
    Rect::new(
        control.x + space::MD,
        control.y + space::SM + BODY_H + space::SM,
        control.w - space::MD * 2,
        LEVEL_TRACK_H,
    )
}

/// The trough the brightness control would fill if a backlight backend existed.
///
/// WHY it exists while `brightness_available` is false: it is the geometry the
/// flag in `QuickControlsFrame::new` hands to the level-control builder, and
/// deriving it from `BRIGHTNESS_ROW` means the day a backend appears nothing about
/// the row moves except the trough arriving.
const BRIGHTNESS_TRACK: Rect = level_track(BRIGHTNESS_ROW);

/// The trough the volume capsule's tap lands in.
///
/// WHY the trough is inset from the capsule's edges: the capsule is a card, and
/// a track that ran to its rounded corners would read as a seam rather than a
/// well. The fill arm in `row_pixel` is proportional to this trough, so the day
/// a level can be read it fills the well that is drawn.
const VOLUME_TRACK: Rect = level_track(VOLUME_SLIDER);

/// WHY these are checked by the compiler rather than trusted: `VOLUME_SLIDER` and
/// `POWER_BTN` are exported into a file this one cannot see, and a layout that
/// pushed the Power pill into the shade's dismiss strip, or a capsule below the
/// minimum target, would be a control that cannot be pressed with no way to
/// notice during review.
const _: () = assert!(
    POWER_BTN.x + POWER_BTN.w <= 600,
    "the Power pill must sit clear of the shade's x >= 600 dismiss strip",
);
const _: () = assert!(
    VOLUME_SLIDER.h >= MIN_TOUCH_TARGET && POWER_BTN.h >= MIN_TOUCH_TARGET,
    "both exported controls must clear the minimum touch target",
);
const _: () = assert!(
    HEADING_Y >= AVAIL_TOP && HEADING_Y + CONTENT_H <= AVAIL_BOT,
    "the panel must fit the band between the status bar and the home indicator",
);

// ---------------------------------------------------------------------------
// The reading list
// ---------------------------------------------------------------------------

/// One row this panel can draw.
///
/// WHY an enum and not a struct per row: this surface's honesty lives in one
/// question -- "is there a reading behind this row?" -- and an enum forces every
/// new row to answer it in `reading()` instead of at the call site.
#[derive(Clone, Copy, PartialEq, Eq)]
enum PanelRow {
    Wifi,
    Bluetooth,
    Cellular,
    Airplane,
    Brightness,
    Volume,
    Torch,
    DoNotDisturb,
    RotationLock,
    Lock,
}

/// The four radios, in the reference's order, laid out as a two-up grid.
const CONNECTIVITY_LIST: [PanelRow; 4] = [
    PanelRow::Wifi,
    PanelRow::Bluetooth,
    PanelRow::Cellular,
    PanelRow::Airplane,
];

/// The level controls: one reading with no backend behind it, and the one
/// capsule a tap answers.
const LEVEL_LIST: [PanelRow; 2] = [PanelRow::Brightness, PanelRow::Volume];

/// The secondary controls.
const SYSTEM_LIST: [PanelRow; 3] = [PanelRow::Torch, PanelRow::DoNotDisturb, PanelRow::RotationLock];

/// The footer's device reading.
const FOOTER_LIST: [PanelRow; 1] = [PanelRow::Lock];

/// Every row, in drawing order.
///
/// WHY this list is the published set: it is the denominator of the heading's
/// "N of M live readings" line, and it is asserted to be exactly the sum of the
/// section lists so a row added to a section and not to this list is a compile
/// error rather than a silently uncounted reading.
const ALL_ROWS: [PanelRow; 10] = [
    PanelRow::Wifi,
    PanelRow::Bluetooth,
    PanelRow::Cellular,
    PanelRow::Airplane,
    PanelRow::Brightness,
    PanelRow::Volume,
    PanelRow::Torch,
    PanelRow::DoNotDisturb,
    PanelRow::RotationLock,
    PanelRow::Lock,
];
const ALL_ROWS_LEN: usize = ALL_ROWS.len();

const _: () = assert!(
    ALL_ROWS_LEN == CONNECTIVITY_LIST.len() + LEVEL_LIST.len() + SYSTEM_LIST.len() + FOOTER_LIST.len(),
    "every row must be in exactly one section list",
);

impl PanelRow {
    /// The label at the left of the row.
    fn label(self) -> &'static [u8] {
        match self {
            PanelRow::Wifi => b"Wi-Fi",
            PanelRow::Bluetooth => b"Bluetooth",
            PanelRow::Cellular => b"Mobile data",
            PanelRow::Airplane => b"Airplane",
            PanelRow::Brightness => b"Brightness",
            PanelRow::Volume => b"Volume",
            PanelRow::Torch => b"Torch",
            PanelRow::DoNotDisturb => b"Do not disturb",
            PanelRow::RotationLock => b"Rotation lock",
            PanelRow::Lock => b"Screen lock",
        }
    }

    /// The glyph at the left of the row.
    fn icon(self) -> QcIcon {
        match self {
            PanelRow::Wifi => QcIcon::Wifi,
            PanelRow::Bluetooth => QcIcon::Bluetooth,
            PanelRow::Cellular => QcIcon::Cellular,
            PanelRow::Airplane => QcIcon::Airplane,
            PanelRow::Brightness => QcIcon::Sun,
            PanelRow::Volume => QcIcon::Speaker,
            PanelRow::Torch => QcIcon::Torch,
            PanelRow::DoNotDisturb => QcIcon::Moon,
            PanelRow::RotationLock => QcIcon::Rotate,
            PanelRow::Lock => QcIcon::Lock,
        }
    }

    /// The reading, or the reason there is none.
    ///
    /// WHY availability is decided on this line and nowhere else: the one rule
    /// this surface must not break is printing a value for hardware the platform
    /// cannot produce, and the only reliable place to enforce that is the line
    /// that decides whether the value exists.
    fn reading(self, state: &LauncherState) -> Value {
        match self {
            PanelRow::Wifi => if !state.wifi_enabled {
                Value::absent(b"Disabled")
            } else {
                hardware("wifi")
            },
            PanelRow::Bluetooth => if !state.bluetooth_enabled {
                Value::absent(b"Disabled")
            } else {
                hardware("bluetooth")
            },
            PanelRow::Cellular => if !state.cellular_enabled {
                Value::absent(b"Disabled")
            } else {
                hardware("cellular")
            },
            PanelRow::Airplane => if state.airplane_mode {
                Value::of(b"Active")
            } else {
                Value::absent(b"Off")
            },
            PanelRow::Brightness => Value::absent(b"Unavailable"),
            PanelRow::Volume => Value::absent(b"Tap to set level"),
            PanelRow::Torch => Value::absent(b"N/A"),
            PanelRow::DoNotDisturb => if state.dnd_enabled {
                Value::of(b"On")
            } else {
                Value::absent(b"Off")
            },
            PanelRow::RotationLock => if state.rotation_lock {
                Value::of(b"Active")
            } else {
                Value::absent(b"Unlocked")
            },
            PanelRow::Lock => Value::of(b"Secure"),
        }
    }
}

/// One reading, plus the flag that produced it.
struct Value {
    reading: Reading,
    available: bool,
}

impl Value {
    fn of(run: &'static [u8]) -> Value {
        Value { reading: Reading::of(run), available: true }
    }

    fn absent(reason: &'static [u8]) -> Value {
        Value { reading: Reading::unavailable(reason), available: false }
    }
}

fn hardware(category: &str) -> Value {
    match interos_std::settings_hardware_status(category) {
        Ok(0) => Value::of(b"Available"),
        Ok(1) => Value::of(b"Simulated"),
        Ok(_) => Value::absent(b"Unavailable"),
        Err(_) => Value::absent(b"Unavailable"),
    }
}

fn brightness_level(_state: &LauncherState) -> Option<u32> {
    None
}

fn volume_level(state: &LauncherState) -> Option<u32> {
    if state.volume_state_valid {
        Some(state.last_volume_percent as u32)
    } else {
        None
    }
}

/// Row index of `item` in the published set, or 0.
///
/// WHY no panic: every list above is built from `ALL_ROWS`, and the assert on
/// that const is what keeps it true; the fallback is here so a future edit that
/// breaks the invariant fails visibly at render time instead of in the kernel.
fn item_index(item: PanelRow) -> usize {
    ALL_ROWS.iter().position(|row| *row == item).unwrap_or(0)
}

/// One reading per row, in `ALL_ROWS` order.
///
/// WHY the values are measured once and passed down: each one is a syscall away,
/// and the heading counts the same values the rows are drawn from, so reading
/// them twice is how a heading ends up disagreeing with the list under it.
fn build_values(state: &LauncherState) -> [Value; ALL_ROWS.len()] {
    core::array::from_fn(|i| ALL_ROWS[i].reading(state))
}

// ---------------------------------------------------------------------------
// Frame
// ---------------------------------------------------------------------------

/// One run of text, measured and ink-resolved, ready for the pixel path.
///
/// WHY measuring belongs to the layout step and not to the painter: every
/// centring decision on this surface is a division that must agree with the
/// rasteriser, and the only way to guarantee that is to do both from the same
/// measurement in the same place.
struct LabelRun {
    /// Horizontal placement rule and the column it places the run in.
    align: Align,
    rect: Rect,
    scale: usize,
    /// Byte offset and length into the run buffer.
    off: usize,
    len: usize,
    buf: [u8; 128],
    ink: u32,
    /// Pixels per em for this run's outline rendering.
    ///
    /// WHY a separate number from `scale`: the bitmap face is a 16x24 cell
    /// multiplied by `scale`, so its cap height is 17*`scale` pixels, while the
    /// outline face's cap height is 1456 of 2048 em. The em that matches the same
    /// cap height is `17 * scale * 2048 / 1456`, and `outline_text::em_for_scale`
    /// is that derivation kept in one place so every screen that migrates a run
    /// derives it the same way.
    em: u32,
    /// Width of the box covering BOTH renderers, measured at construction.
    w: usize,
    /// Top row of the box covering BOTH renderers, not the bitmap cell's.
    ///
    /// WHY the box is a union: the outline run's line box is not the bitmap cell.
    /// At a scale-2 caption the outline's ascent reaches 50 rows above the run's
    /// baseline while the bitmap's own ascender reaches 8 rows into its cell, and
    /// its descent reaches 13 rows past the baseline where the bitmap's descender
    /// stops at row 4 of 4 below it, so a box measured for the cell alone clips a
    /// descender off the bottom. `home.rs` hit exactly this and carried the
    /// lesson here.
    top: usize,
    /// Height of that union box.
    box_h: usize,
    outline: Option<launcher::outline_text::OutlineRun>,
}

/// Where a run sits inside its rect.
#[derive(Clone, Copy)]
enum Align {
    Left,
    Right,
}

impl LabelRun {
    const EMPTY: LabelRun = LabelRun {
        align: Align::Left,
        rect: Rect::new(0, 0, 0, 0),
        scale: 1,
        off: 0,
        len: 0,
        buf: [0u8; 128],
        ink: 0,
        em: 0,
        w: 0,
        top: 0,
        box_h: 0,
        outline: None,
    };

    /// Measures `run` and left-aligns it inside `rect`, vertically centred in
    /// the rect's own height.
    ///
    /// WHY a rect rather than a point: the same rect is what `right` and the
    /// column arithmetic use, so a column is described once and every caller
    /// agrees on where it starts and ends.
    fn left(rect: Rect, scale: usize, run: &[u8], ink: u32) -> LabelRun {
        let len = run.len().min(128);
        let mut buf = [0u8; 128];
        buf[..len].copy_from_slice(&run[..len]);
        let em = launcher::outline_text::em_for_scale(scale);
        let (asc, desc) = interos_std::design::typeface::line_metrics(em);
        let baseline = (rect.y + (rect.h.saturating_sub(text::line_height(scale))) / 2
            + launcher::font::BASELINE * scale).max(asc as usize);
        let top = baseline.saturating_sub(asc as usize);
        // WHY the width is the wider of the two measures: the outline face is
        // condensed but is not always the narrower one. "Do not disturb"
        // measures 135 bitmap advances at scale 2 (a 270px measure) and a 260px
        // outline advance, so the cell is the wider face for that run; the union
        // is taken rather than assuming which way it goes.
        let w = text::measure(&buf[..len], scale)
            .max(interos_std::design::typeface::text_advance_px(&buf[..len], em) as usize);
        let outline = if scale >= 3 && len <= launcher::outline_text::KEY_MAX {
            launcher::outline_text::OutlineRun::place(rect.x, baseline, em, &buf[..len])
        } else {
            None
        };
        LabelRun {
            align: Align::Left,
            rect,
            scale,
            off: 0,
            len,
            buf,
            ink,
            em,
            w,
            top,
            box_h: (baseline + desc as usize).saturating_sub(top),
            outline,
        }
    }

    /// Measures `run` and right-aligns it inside `rect`, vertically centred.
    fn right(rect: Rect, scale: usize, run: &[u8], ink: u32) -> LabelRun {
        let len = run.len().min(128);
        let mut buf = [0u8; 128];
        buf[..len].copy_from_slice(&run[..len]);
        let em = launcher::outline_text::em_for_scale(scale);
        let (asc, desc) = interos_std::design::typeface::line_metrics(em);
        let baseline = (rect.y + (rect.h.saturating_sub(text::line_height(scale))) / 2
            + launcher::font::BASELINE * scale).max(asc as usize);
        let top = baseline.saturating_sub(asc as usize);
        let w = text::measure(&buf[..len], scale)
            .max(interos_std::design::typeface::text_advance_px(&buf[..len], em) as usize);
        let x0 = rect.x + rect.w.saturating_sub(w);
        let outline = if scale >= 3 && len <= launcher::outline_text::KEY_MAX {
            launcher::outline_text::OutlineRun::place(x0, baseline, em, &buf[..len])
        } else {
            None
        };
        LabelRun {
            align: Align::Right,
            rect,
            scale,
            off: 0,
            len,
            buf,
            ink,
            em,
            w,
            top,
            box_h: (baseline + desc as usize).saturating_sub(top),
            outline,
        }
    }

    /// Where the run's first glyph starts on this surface.
    fn x0(&self) -> usize {
        let w = text::measure(&self.buf[..self.len], self.scale);
        match self.align {
            Align::Left => self.rect.x,
            Align::Right => self.rect.x + self.rect.w.saturating_sub(w),
        }
    }

    /// Top of the run's line box: the rect's height minus the cell, halved.
    ///
    /// WHY the cell and not the cap height: the cell is what the rasteriser can
    /// touch, so a band that starts inside it is a run whose descenders get
    /// sliced off. Centring the whole cell is what keeps "Volume" and "Do not
    /// disturb" sitting on one optical line.
    fn y0(&self) -> usize {
        let h = text::line_height(self.scale);
        self.rect.y + self.rect.h.saturating_sub(h) / 2
    }

    /// Anti-aliased ink for `(x, y)`, or `None` when the pixel is not on the run.
    ///
    /// WHY the ink and the background are both parameters: glyph edges blend
    /// against whatever the run is drawn on, and this panel draws the same run
    /// on the page background, on a card and on a pill -- three different
    /// grounds for the same glyphs.
    fn pixel_aa(&self, x: usize, y: usize, ink: u32, bg: u32) -> Option<u32> {
        let y0 = self.y0();
        let x0 = self.x0();
        if x < x0 || x - x0 >= self.w || y < self.top || y - self.top >= self.box_h {
            return None;
        }
        if let Some(ref run) = self.outline {
            return run.pixel(x, y, ink, bg);
        }
        // The bitmap face is the refusal path, not the baseline: a run the
        // rasteriser declines still has to draw as text.
        crate::test_text_aa(x, y, x0, y0, self.scale, &self.buf[..self.len], ink, bg)
    }

    /// Ink for `(x, y)`, anti-aliased against `bg`.
    fn pixel_on(&self, x: usize, y: usize, bg: u32) -> Option<u32> {
        self.pixel_aa(x, y, self.ink, bg)
    }
}

/// One radio tile: a two-up grid cell with the label above the reason.
///
/// WHY a separate frame from a row: a tile's two lines are stacked in a column
/// beside its icon, while a row's two runs are carved out of one text line. They
/// are different layouts of the same three pieces, and modelling them as one
/// struct with two optional columns is how a label ends up centred in a box
/// nobody drew.
struct TileFrame {
    rect: Rect,
    icon: QcIcon,
    icon_x: usize,
    icon_y: usize,
    fill: u32,
    border: u32,
    ink: u32,
    label: LabelRun,
    value: LabelRun,
}

impl TileFrame {
    const EMPTY: TileFrame = TileFrame {
        rect: Rect::new(0, 0, 0, 0),
        icon: QcIcon::Lock,
        icon_x: 0,
        icon_y: 0,
        fill: 0,
        border: 0,
        ink: 0,
        label: LabelRun::EMPTY,
        value: LabelRun::EMPTY,
    };
}

/// One row: a flat reading, or a level control with a trough.
///
/// WHY one frame covers both: the difference between them is exactly three
/// optional fields -- the card fill, the trough and the hairline -- and each one
/// is a decision made at build time where the row's own reason for being there
/// is written down. The painter then has one path instead of two that must agree.
struct RowFrame {
    rect: Rect,
    /// Corner radius of the card, when the row is one.
    corner: usize,
    /// Card ground. `None` means a flat row drawn on the page background.
    fill: Option<u32>,
    /// Hairline drawn on the card's edge, when the row is a card.
    border: Option<u32>,
    ink: u32,
    icon: QcIcon,
    icon_x: usize,
    icon_y: usize,
    label: LabelRun,
    value: LabelRun,
    /// Trough a level would fill, when the row is a level control.
    track: Option<Rect>,
    /// How much of that trough a bound level would fill. Zero on this platform,
    /// and deliberately reachable only through a level that was read.
    fill_w: usize,
    /// Hairline drawn at this row's top edge, when the row is the second or
    /// later of a contiguous list.
    divider_y: Option<usize>,
}

impl RowFrame {
    const EMPTY: RowFrame = RowFrame {
        rect: Rect::new(0, 0, 0, 0),
        corner: radius::CONTROL,
        fill: None,
        border: None,
        ink: 0,
        icon: QcIcon::Lock,
        icon_x: 0,
        icon_y: 0,
        label: LabelRun::EMPTY,
        value: LabelRun::EMPTY,
        track: None,
        fill_w: 0,
        divider_y: None,
    };
}

/// One laid-out shade.
struct QuickControlsFrame {
    clock: LabelRun,
    heading: LabelRun,
    summary: LabelRun,
    /// Section labels, in the order the sections are drawn.
    sections: [LabelRun; 4],
    radios: [TileFrame; CONNECTIVITY_LIST.len()],
    brightness: RowFrame,
    volume: RowFrame,
    controls: [RowFrame; SYSTEM_LIST.len()],
    lock: RowFrame,
    power_icon_x: usize,
    power_icon_y: usize,
    power_label: LabelRun,
}

impl QuickControlsFrame {
    const EMPTY: QuickControlsFrame = QuickControlsFrame {
        clock: LabelRun::EMPTY,
        heading: LabelRun::EMPTY,
        summary: LabelRun::EMPTY,
        sections: [LabelRun::EMPTY; 4],
        radios: [TileFrame::EMPTY; CONNECTIVITY_LIST.len()],
        brightness: RowFrame::EMPTY,
        volume: RowFrame::EMPTY,
        controls: [RowFrame::EMPTY; SYSTEM_LIST.len()],
        lock: RowFrame::EMPTY,
        power_icon_x: 0,
        power_icon_y: 0,
        power_label: LabelRun::EMPTY,
    };

    fn new(state: &LauncherState) -> QuickControlsFrame {
        let theme = &THEME;
        let values = build_values(state);
        let live = ALL_ROWS.iter().filter(|row| values[item_index(**row)].available).count();

        let (mut buf, mut at) = decimal(live);
        push(&mut buf, &mut at, b" of ");
        let (right, right_len) = decimal(ALL_ROWS.len());
        push(&mut buf, &mut at, &right[..right_len]);
        push(&mut buf, &mut at, b" live readings");

        // The clock keeps its existing home in the status strip. WHY it is
        // right-aligned and only drawn when the shell was actually given a
        // reading: a clock the kernel refused to provide must leave the strip
        // blank rather than show a plausible time.
        let clock = if state.clock_source_ok {
            LabelRun::right(
                Rect::new(0, STATUS_BAR_H - STATUS_H, SCREEN_WIDTH, STATUS_H),
                type_scale::CAPTION,
                &state.clock_time,
                theme.text,
            )
        } else {
            LabelRun::EMPTY
        };

        let heading = LabelRun::left(
            Rect::new(PANEL_X, HEADING_Y, PANEL_W, BODY_H),
            type_scale::BODY,
            b"Quick Controls",
            theme.text,
        );
        let summary = LabelRun::left(
            Rect::new(PANEL_X, SUMMARY_Y, PANEL_W, CAPTION_H),
            type_scale::CAPTION,
            &buf[..at],
            theme.text_muted,
        );
        let sections = [
            LabelRun::left(Rect::new(PANEL_X, CONNECTIVITY_LABEL_Y, PANEL_W, SECTION_H), type_scale::CAPTION, b"Connectivity", theme.text_muted),
            LabelRun::left(Rect::new(PANEL_X, LEVEL_LABEL_Y, PANEL_W, SECTION_H), type_scale::CAPTION, b"Brightness and volume", theme.text_muted),
            LabelRun::left(Rect::new(PANEL_X, SYSTEM_LABEL_Y, PANEL_W, SECTION_H), type_scale::CAPTION, b"Secondary controls", theme.text_muted),
            LabelRun::left(Rect::new(PANEL_X, FOOTER_LABEL_Y, PANEL_W, SECTION_H), type_scale::CAPTION, b"Device and power", theme.text_muted),
        ];

        // The two-up radio grid, row-major in the reference's order.
        let radios = {
            let mut out = [TileFrame::EMPTY; CONNECTIVITY_LIST.len()];
            let mut i = 0;
            while i < CONNECTIVITY_LIST.len() {
                let col = i % TILE_ROWS;
                let row = i / TILE_ROWS;
                let rect = Rect::new(
                    PANEL_X + col * (TILE_W + TILE_GUTTER),
                    CONNECTIVITY_TOP + row * (TILE_H + TILE_GUTTER),
                    TILE_W,
                    TILE_H,
                );
                out[i] = build_tile(rect, CONNECTIVITY_LIST[i], &values[item_index(CONNECTIVITY_LIST[i])], theme);
                i += 1;
            }
            out
        };

        // The level block. WHY `brightness_available` is decided here, in the
        // function that lays the block out, and not inside a reader: it is the
        // single decision that keeps this panel honest about brightness. No
        // virtual backlight is wired to this guest, so there is no level to read
        // and no trough to draw, and the flag is what hands the row its shape --
        // a flat reading while it is false, a level control with a track the
        // moment it is true. `brightness_level()` returning `None` is today's
        // answer to the second half of that; the flag is the rule.
        let brightness_available = false;
        let brightness = if brightness_available {
            build_level(
                BRIGHTNESS_ROW,
                LEVEL_LIST[0],
                &values[item_index(LEVEL_LIST[0])],
                brightness_level(state),
                Some(BRIGHTNESS_TRACK),
                theme,
            )
        } else {
            build_row(
                BRIGHTNESS_ROW,
                LEVEL_LIST[0],
                &values[item_index(LEVEL_LIST[0])],
                None,
                theme,
            )
        };
        let volume = build_level(
            VOLUME_SLIDER,
            LEVEL_LIST[1],
            &values[item_index(LEVEL_LIST[1])],
            volume_level(state),
            Some(VOLUME_TRACK),
            theme,
        );

        // The secondary controls: contiguous rows, a hairline sealing each seam.
        let controls = {
            let mut out = [RowFrame::EMPTY; SYSTEM_LIST.len()];
            let mut i = 0;
            while i < SYSTEM_LIST.len() {
                let y = SYSTEM_TOP + i * ROW_H;
                out[i] = build_row(
                    Rect::new(PANEL_X, y, PANEL_W, ROW_H),
                    SYSTEM_LIST[i],
                    &values[item_index(SYSTEM_LIST[i])],
                    // WHY the hairline belongs to the row below the seam: the
                    // pixel path tests a row's own rectangle, and a row's rect
                    // stops at its last pixel -- a divider stored on the row
                    // above would sit one pixel outside the rect that owns it
                    // and be checked by nobody.
                    (i > 0).then_some(y),
                    theme,
                );
                i += 1;
            }
            out
        };

        let lock = build_row(
            Rect::new(PANEL_X, FOOTER_Y, PANEL_W, FOOTER_H),
            FOOTER_LIST[0],
            &values[item_index(FOOTER_LIST[0])],
            None,
            theme,
        );

        // The Power entry: glyph and label centred as one group inside the pill,
        // so the pill can be any width without the label drifting off centre.
        let power_run = ICON + space::MD + text::measure(b"Power menu", type_scale::CAPTION);
        let power_icon_x = POWER_BTN.x + (POWER_BTN.w - power_run) / 2;
        let power_icon_y = POWER_BTN.center_y() - ICON / 2;
        let power_label = LabelRun::left(
            Rect::new(power_icon_x + ICON + space::MD, POWER_BTN.y, POWER_BTN.w, POWER_BTN.h),
            type_scale::CAPTION,
            b"Power menu",
            theme.surface,
        );

        QuickControlsFrame {
            clock,
            heading,
            summary,
            sections,
            radios,
            brightness,
            volume,
            controls,
            lock,
            power_icon_x,
            power_icon_y,
            power_label,
        }
    }
}

/// Builds one radio tile.
///
/// WHY the fill and the value ink both come from `Reading::skin`: an available
/// value gets the surface and full ink, an absent one gets the recessed well and
/// muted ink, and that single skin is what makes "this slot is not a switch"
/// readable without a second colour anywhere.
fn build_tile(rect: Rect, item: PanelRow, value: &Value, theme: &Theme) -> TileFrame {
    let (fill, value_ink) = value.reading.skin(theme);
    let ink = if value.available { theme.text } else { theme.text_muted };
    let text_x = rect.x + TILE_PAD_X + ICON + space::MD;
    let label_y = rect.y + TILE_PAD_Y;
    let value_y = label_y + BODY_H + TILE_TEXT_GAP;
    TileFrame {
        rect,
        icon: item.icon(),
        icon_x: rect.x + TILE_PAD_X,
        icon_y: rect.center_y() - ICON / 2,
        fill,
        border: theme.border,
        ink,
        label: LabelRun::left(
            Rect::new(text_x, label_y, TILE_TEXT_W, BODY_H),
            ROW_SCALE,
            item.label(),
            ink,
        ),
        value: LabelRun::left(
            Rect::new(text_x, value_y, TILE_TEXT_W, CAPTION_H),
            VALUE_SCALE,
            value.reading.as_slice(),
            value_ink,
        ),
    }
}

/// Builds a flat reading row: icon, label and value on the page background.
fn build_row(
    rect: Rect,
    item: PanelRow,
    value: &Value,
    divider_y: Option<usize>,
    theme: &Theme,
) -> RowFrame {
    let ink = if value.available { theme.text } else { theme.text_muted };
    let (label, value_run) = row_runs(rect, rect.y, rect.h, item.label(), &value.reading, ink, theme);
    RowFrame {
        rect,
        corner: radius::CONTROL,
        fill: None,
        border: None,
        ink,
        icon: item.icon(),
        icon_x: rect.x + space::MD,
        icon_y: rect.center_y() - ICON / 2,
        label,
        value: value_run,
        track: None,
        fill_w: 0,
        divider_y,
    }
}

/// Builds a level control.
///
/// WHY the card and the trough arrive together: a level control is the one thing
/// on this panel a tap can act on, so it is raised and outlined; a level with no
/// trough is a reading, and readings are flat rows. `fill_w` is derived from the
/// level that was actually read, so the fill arm below is unreachable unless a
/// backend exists -- which is the same rule the brightness row already lives by.
fn build_level(
    rect: Rect,
    item: PanelRow,
    value: &Value,
    level: Option<u32>,
    track: Option<Rect>,
    theme: &Theme,
) -> RowFrame {
    let ink = if value.available { theme.text } else { theme.text_muted };
    let (label, value_run) =
        row_runs(rect, rect.y + space::SM, BODY_H, item.label(), &value.reading, ink, theme);
    let fill_w = match (track, level) {
        (Some(track), Some(percent)) => (track.w.saturating_mul(percent.min(100) as usize)) / 100,
        _ => 0,
    };
    RowFrame {
        rect,
        corner: radius::CONTROL,
        fill: Some(theme.surface),
        border: Some(theme.border),
        ink,
        icon: item.icon(),
        icon_x: rect.x + space::MD,
        icon_y: rect.y + space::SM + (BODY_H - ICON) / 2,
        label,
        value: value_run,
        track,
        fill_w,
        divider_y: None,
    }
}

/// The label and value runs of a full-width row.
///
/// `text_y` and `text_h` are where the row's text line sits: the whole row for a
/// flat reading, which centres its runs, and one line below a level control's top
/// padding, whose trough lives underneath. Passing the band rather than deriving
/// it from the rect is what keeps a level control's label off its own top edge.
///
/// WHY the value column is capped at half the text column: a value that could
/// take the whole line is a label with extra steps, and the cap is what keeps
/// "Volume" from being pushed off the left edge of its own row. Overflow is
/// ellipsised by the same `text::ellipsize` every other surface here uses, so a
/// longer reason is clipped rather than overlapping the label.
fn row_runs(
    rect: Rect,
    text_y: usize,
    text_h: usize,
    label: &'static [u8],
    value: &Reading,
    ink: u32,
    theme: &Theme,
) -> (LabelRun, LabelRun) {
    let text_x = rect.x + ROW_TEXT_INSET;
    let text_w = rect.w.saturating_sub(ROW_TEXT_INSET + space::MD);
    let (_fill, value_ink) = value.skin(theme);
    let value_w = text::measure(value.as_slice(), VALUE_SCALE).min(text_w / 2) + space::SM;
    let value_col = Rect::new(rect.x + rect.w - space::MD - value_w, text_y, value_w, text_h);
    let label_col = Rect::new(text_x, text_y, text_w - value_w, text_h);
    (
        LabelRun::left(label_col, ROW_SCALE, label, ink),
        LabelRun::right(value_col, VALUE_SCALE, value.as_slice(), value_ink),
    )
}

// ---------------------------------------------------------------------------
// Painting
// ---------------------------------------------------------------------------

/// One measured shade, kept for the length of a redraw.
///
/// WHY a cache at all: the readings behind this panel cost four syscalls
/// (`settings_hardware_status` three times, `user_get_lock_state` once) and a
/// string scan, and the shell asks for a colour once per pixel.
static mut FRAME_CACHE: Option<QuickControlsFrame> = None;

/// Colours the shade at `(x, y)`.
///
/// WHY the frame is rebuilt at (0, 0): the shell walks the surface in raster
/// order from the top-left pixel, so (0, 0) is the first pixel of every frame and
/// the only pixel within a frame that repeats it. `LauncherState` is borrowed
/// immutably for the whole frame, so rebuilding there is once per frame and the
/// plan can never lag the state it describes.
#[inline(never)]
pub fn get_quick_controls_color(x: usize, y: usize, state: &LauncherState) -> u32 {
    let theme = THEME;
    let cached = unsafe {
        let slot: *mut Option<QuickControlsFrame> = core::ptr::addr_of_mut!(FRAME_CACHE);
        if (x == 0 && y == 0) || (*slot).is_none() {
            *slot = Some(QuickControlsFrame::new(state));
        }
        (*slot).as_ref()
    };
    // WHY the background and not a panic: the slot was filled on the line above,
    // so `None` is not reachable through this function. Asserting it with
    // `unwrap()` would take the whole shell down over one pixel, and a pixel with
    // nothing behind it honestly has the surface's own colour. The next frame
    // begins at (0, 0) and rebuilds the plan.
    let Some(frame) = cached else {
        return theme.bg;
    };
    quick_controls_pixel(x, y, frame, &theme)
}

/// The pixel path of the shade.
fn quick_controls_pixel(x: usize, y: usize, frame: &QuickControlsFrame, theme: &Theme) -> u32 {
    if y < STATUS_BAR_HEIGHT {
        return status_bar_pixel(x, y, frame, theme);
    }
    if y >= LIST_BOTTOM {
        return home_indicator_pixel(x, y, theme);
    }
    if y < HEADING_Y {
        return theme.bg;
    }
    if y < SUMMARY_Y {
        if let Some(c) = frame.heading.pixel_on(x, y, theme.bg) {
            return c;
        }
        return theme.bg;
    }
    if y < CONNECTIVITY_LABEL_Y {
        if let Some(c) = frame.summary.pixel_on(x, y, theme.bg) {
            return c;
        }
        return theme.bg;
    }
    if y < CONNECTIVITY_TOP {
        if let Some(c) = frame.sections[0].pixel_on(x, y, theme.bg) {
            return c;
        }
        return theme.bg;
    }
    if y < LEVEL_LABEL_Y {
        for tile in &frame.radios {
            if tile.rect.contains(x, y) {
                return tile_pixel(x, y, tile, theme);
            }
        }
        return theme.bg;
    }
    if y < LEVEL_TOP {
        if let Some(c) = frame.sections[1].pixel_on(x, y, theme.bg) {
            return c;
        }
        return theme.bg;
    }
    if y < VOLUME_SLIDER.y {
        if frame.brightness.rect.contains(x, y) {
            return row_pixel(x, y, &frame.brightness, theme);
        }
        return theme.bg;
    }
    if y < SYSTEM_LABEL_Y {
        if frame.volume.rect.contains(x, y) {
            return row_pixel(x, y, &frame.volume, theme);
        }
        return theme.bg;
    }
    if y < SYSTEM_TOP {
        if let Some(c) = frame.sections[2].pixel_on(x, y, theme.bg) {
            return c;
        }
        return theme.bg;
    }
    if y < FOOTER_LABEL_Y {
        for row in &frame.controls {
            if row.rect.contains(x, y) {
                return row_pixel(x, y, row, theme);
            }
        }
        return theme.bg;
    }
    if y < FOOTER_Y {
        if let Some(c) = frame.sections[3].pixel_on(x, y, theme.bg) {
            return c;
        }
        return theme.bg;
    }
    if y < POWER_Y {
        if frame.lock.rect.contains(x, y) {
            return row_pixel(x, y, &frame.lock, theme);
        }
        return theme.bg;
    }
    if POWER_BTN.contains(x, y) {
        return power_pixel(x, y, frame, theme);
    }
    theme.bg
}

/// One radio tile.
///
/// WHY the tile draws its own border before anything else: a card's edge is its
/// identity on this surface, and an unavailable slot still needs one -- a well
/// with no edge would read as a hole in the background rather than as a disabled
/// control.
fn tile_pixel(x: usize, y: usize, tile: &TileFrame, theme: &Theme) -> u32 {
    if !round_contains(&tile.rect, radius::CARD, x, y) {
        return theme.bg;
    }
    if !round_contains(&tile.rect.inset(1), radius::CARD - 1, x, y) {
        return tile.border;
    }
    if icon_hits(tile.icon, tile.icon_x, tile.icon_y, x, y) {
        return tile.ink;
    }
    if let Some(c) = tile.label.pixel_aa(x, y, tile.label.ink, tile.fill) {
        return c;
    }
    if let Some(c) = tile.value.pixel_aa(x, y, tile.value.ink, tile.fill) {
        return c;
    }
    tile.fill
}

/// One row: flat reading or level control.
///
/// WHY the trough is drawn above the icon and the text: a fill has to be able to
/// claim the trough's whole width, and an icon drawn after it would sit on top
/// of a fill that is supposed to be the control's content.
fn row_pixel(x: usize, y: usize, row: &RowFrame, theme: &Theme) -> u32 {
    if let Some(divider_y) = row.divider_y {
        if y == divider_y && x >= row.rect.x && x < row.rect.x + row.rect.w {
            return theme.border;
        }
    }

    // A card paints its own ground; a flat row leaves the page background. Both
    // kinds of row fall through to `base`, which is why one function serves
    // them.
    let base = match row.fill {
        Some(fill) => {
            if !round_contains(&row.rect, row.corner, x, y) {
                return theme.bg;
            }
            if !round_contains(&row.rect.inset(1), row.corner - 1, x, y) {
                return row.border.unwrap_or(theme.border);
            }
            fill
        }
        None => theme.bg,
    };

    if let Some(track) = row.track {
        if round_contains(&track, track.h / 2, x, y) {
            return if row.fill_w > 0 && x <= track.x + row.fill_w {
                LEVEL_FILL
            } else {
                theme.surface_sunken
            };
        }
    }
    if icon_hits(row.icon, row.icon_x, row.icon_y, x, y) {
        return row.ink;
    }
    if let Some(c) = row.label.pixel_aa(x, y, row.label.ink, base) {
        return c;
    }
    if let Some(c) = row.value.pixel_aa(x, y, row.value.ink, base) {
        return c;
    }
    base
}

/// The Power entry pill.
///
/// WHY it is the design system's PrimaryButton and not the danger-ringed card
/// that used to sit here: the shade's job is to be calm, and the ring made a
/// shortcut look like an emergency. The destructive choices live in the Power
/// Menu this opens, where they belong, and the pill is the one bright element on
// the surface -- which is what makes the panel's primary action findable.
fn power_pixel(x: usize, y: usize, frame: &QuickControlsFrame, theme: &Theme) -> u32 {
    if !round_contains(&POWER_BTN, POWER_BTN.h / 2, x, y) {
        return theme.bg;
    }
    let ink = theme.surface;
    if icon_hits(QcIcon::Power, frame.power_icon_x, frame.power_icon_y, x, y) {
        return ink;
    }
    if let Some(c) = frame.power_label.pixel_aa(x, y, ink, theme.text) {
        return c;
    }
    theme.text
}

/// Status bar: the clock, if the shell was given one, right-aligned, with the
/// hairline under it.
///
/// WHY nothing else lives here: the reference's strip carries system indicators,
/// and this guest has no radio, no battery gauge and no signal to show. A blank
/// strip with a clock and a rule is the honest version of that bar.
fn status_bar_pixel(x: usize, y: usize, frame: &QuickControlsFrame, theme: &Theme) -> u32 {
    if let Some(c) = frame.clock.pixel_on(x, y, theme.bg) {
        return c;
    }
    if y == STATUS_BAR_H - 1 && x >= PANEL_X && x < PANEL_X + PANEL_W {
        return theme.border;
    }
    theme.bg
}

/// Home indicator.
///
/// WHY it is drawn here and not left to the wallpaper: the shade sits over the
/// wallpaper, which is a gradient, and a gesture pill nobody can see is a
/// gesture nobody takes.
fn home_indicator_pixel(x: usize, y: usize, theme: &Theme) -> u32 {
    let iy = LIST_BOTTOM + space::MD;
    let ix = (SCREEN_WIDTH - INDICATOR_W) / 2;
    let rect = Rect::new(ix, iy, INDICATOR_W, INDICATOR_H);
    if !rect.contains(x, y) {
        return theme.bg;
    }
    if !round_contains(&rect, rect.h / 2, x, y) {
        return theme.bg;
    }
    Theme::blend(theme.text_muted, theme.bg, 140)
}

// ---------------------------------------------------------------------------
// Icons
// ---------------------------------------------------------------------------

/// Glyphs this panel draws, as 28x28 bit patterns.
///
/// WHY local bitmaps rather than the launcher's icon catalogue: this surface's
/// glyphs are drawn at one fixed box size inside rows this module owns, and the
/// catalogue is sized for app tiles. Keeping them here also keeps the panel's
/// drawing independent of a catalogue a future screen may restyle.
#[derive(Clone, Copy)]
enum QcIcon {
    Wifi,
    Bluetooth,
    Cellular,
    Sun,
    Speaker,
    Torch,
    Rotate,
    Moon,
    Airplane,
    Lock,
    Power,
}

/// Whether `(x, y)` lands on an inked pixel of `icon` inside its 28x28 box.
fn icon_hits(icon: QcIcon, box_x: usize, box_y: usize, x: usize, y: usize) -> bool {
    let dx = match x.checked_sub(box_x) {
        Some(dx) if dx < ICON => dx,
        _ => return false,
    };
    let dy = match y.checked_sub(box_y) {
        Some(dy) if dy < ICON => dy,
        _ => return false,
    };
    icon_bits(icon, dx, dy)
}

/// Whether `(x, y)` is within `half` pixels of the segment `(x0,y0)-(x1,y1)`.
///
/// WHY a distance test rather than a run of rectangles: a diagonal drawn as
/// rectangles either staircases or has to be hand-placed per slope, and the
/// Bluetooth glyph and the rotation arrowhead are both nearly diagonal. The
/// comparison is done on squared distances scaled by the segment's own length,
/// so it stays exact without a square root.
fn icon_line(x: usize, y: usize, x0: i64, y0: i64, x1: i64, y1: i64, half: i64) -> bool {
    let px = x as i64;
    let py = y as i64;
    let ex = x1 - x0;
    let ey = y1 - y0;
    let len2 = ex * ex + ey * ey;
    if len2 == 0 {
        return (px - x0) * (px - x0) + (py - y0) * (py - y0) <= half * half;
    }
    let mut t = (px - x0) * ex + (py - y0) * ey;
    if t < 0 {
        t = 0;
    }
    if t > len2 {
        t = len2;
    }
    let dx = px * len2 - x0 * len2 - t * ex;
    let dy = py * len2 - y0 * len2 - t * ey;
    (dx * dx + dy * dy) <= (half * len2) * (half * len2)
}

#[allow(clippy::too_many_lines)]
fn icon_bits(icon: QcIcon, x: usize, y: usize) -> bool {
    let on = |x0: usize, y0: usize, w: usize, h: usize| x >= x0 && x < x0 + w && y >= y0 && y < y0 + h;
    match icon {
        // Three arcs of a fan plus a dot at their centre: the one drawing of
        // Wi-Fi that reads at 28 px. WHY arcs and not the single ring this panel
        // used to draw: a ring with a dot inside it reads as a location marker,
        // which is a different feature, and a control centre's radio tiles are
        // the last place a user should have to guess which icon is which.
        QcIcon::Wifi => {
            let dx = x as i64 - 14;
            let dy = y as i64 - 25;
            let d2 = dx * dx + dy * dy;
            for r in [7i64, 12, 17] {
                if dy <= 1 && d2 <= r * r && d2 >= (r - 2) * (r - 2) {
                    return true;
                }
            }
            d2 <= 2 * 2
        }
        // The rune: a stem with two diagonals that meet at the right at
        // mid-height. The previous glyph was a stem with three stubby bars and
        // two diagonals, which read as a scratch at this size.
        QcIcon::Bluetooth => {
            icon_line(x, y, 14, 3, 14, 25, 1)
                || icon_line(x, y, 14, 3, 23, 14, 1)
                || icon_line(x, y, 23, 14, 14, 25, 1)
        }
        // Four ascending bars. WHY the stray bar above them is gone: two bars
        // plus a floating rectangle read as a battery before they read as
        // signal.
        QcIcon::Cellular => on(3, 19, 4, 6) || on(9, 14, 4, 11) || on(15, 9, 4, 16) || on(21, 4, 4, 21),
        QcIcon::Sun => {
            // WHY squared distances and not `sqrt`: `core` has no floating-point
            // maths, and a square root here would be a per-pixel `f64` on a
            // surface that draws a million of them a frame.
            let dx = x as i64 - 14;
            let dy = y as i64 - 14;
            if dx * dx + dy * dy <= 20 {
                return true;
            }
            let rays = [
                (14usize, 2usize, 14usize, 6usize),
                (14, 22, 14, 26),
                (2, 14, 6, 14),
                (22, 14, 26, 14),
                (5, 5, 8, 8),
                (20, 20, 23, 23),
                (20, 5, 23, 8),
                (5, 20, 8, 23),
            ];
            for (x0, y0, x1, y1) in rays {
                if x >= x0 && x <= x1 && y >= y0 && y <= y1 && dx * dx + dy * dy >= 30 {
                    return true;
                }
            }
            false
        }
        QcIcon::Speaker => {
            if on(5, 11, 5, 6) || on(9, 7, 5, 14) || on(13, 10, 4, 8) {
                return true;
            }
            let arcs = [(18usize, 14usize, 5usize), (22, 14, 9)];
            for (ax, ay, r) in arcs {
                let dx = x as i64 - ax as i64;
                let dy = y as i64 - ay as i64;
                let d = (dx * dx + dy * dy) as usize;
                if d <= r * r && d >= (r - 2) * (r - 2) {
                    return true;
                }
            }
            false
        }
        QcIcon::Torch => {
            if on(12, 4, 4, 10) || on(11, 14, 6, 10) || on(6, 16, 4, 3) || on(18, 16, 4, 3) {
                return true;
            }
            let dx = x as i64 - 14;
            let dy = y as i64 - 2;
            (dx * dx + dy * dy) as usize <= 2 * 2
        }
        QcIcon::Rotate => {
            // A ring with a gap on its right and an arrowhead closing it: the
            // rotation glyph. The previous version drew a ring plus four
            // satellites, which read as an atom rather than as a rotation.
            let dx = x as i64 - 14;
            let dy = y as i64 - 14;
            let d2 = dx * dx + dy * dy;
            let ring = d2 <= 10 * 10 && d2 >= 8 * 8 && !(dx > 0 && dy < 0);
            ring || icon_line(x, y, 20, 4, 26, 10, 1) || icon_line(x, y, 20, 16, 26, 10, 1)
        }
        QcIcon::Moon => {
            // A crescent: a disc with an offset disc subtracted.
            //
            // WHY the subtraction is spelled out: the first version of this arm
            // tested the *intersection* of the two discs, which draws their
            // overlap -- a small blob that read as a filled dot rather than as a
            // moon. `Do not disturb` is the row that wears this glyph, and a
            // reader who mistakes it for a bullet has nothing to attach the
            // row's reason to.
            let dx = x as i64 - 14;
            let dy = y as i64 - 14;
            if dx * dx + dy * dy > 11 * 11 {
                return false;
            }
            let ex = x as i64 - 21;
            let ey = y as i64 - 13;
            ex * ex + ey * ey > 9 * 9
        }
        QcIcon::Airplane => {
            if on(13, 2, 3, 24) || on(2, 13, 24, 2) || on(7, 18, 14, 2) || on(7, 8, 14, 2) {
                return true;
            }
            let dx = x as i64 - 14;
            let dy = y as i64 - 5;
            (dx * dx + dy * dy) as usize <= 3 * 3
        }
        QcIcon::Lock => {
            if on(6, 14, 17, 10) || on(9, 6, 3, 9) || on(17, 6, 3, 9) || on(9, 5, 11, 3) {
                return true;
            }
            on(13, 18, 3, 3)
        }
        QcIcon::Power => {
            let dx = x as i64 - 14;
            let dy = y as i64 - 14;
            let d = (dx * dx + dy * dy) as usize;
            if d >= 9 * 9 && d <= 12 * 12 && dy <= 1 {
                return true;
            }
            on(13, 2, 2, 12)
        }
    }
}

// ---------------------------------------------------------------------------
// Status strip metrics
// ---------------------------------------------------------------------------

/// The height of the status strip, and of the caption line inside it.
///
/// WHY these two are named rather than written: the strip is the one part of the
/// surface that is not this panel's, and every place it is measured -- the rule
/// under the clock, the pixel path's first branch -- reads the same pair.
const STATUS_BAR_H: usize = STATUS_BAR_HEIGHT;
const STATUS_H: usize = CAPTION_H;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum QuickControlAction {
    ToggleWifi,
    ToggleBluetooth,
    ToggleCellular,
    ToggleAirplane,
    SetBrightness(u8),
    SetVolume(u8),
    ToggleTorch,
    ToggleDnd,
    ToggleRotationLock,
    LockScreen,
    PowerMenu,
    Dismiss,
}

pub fn quick_controls_hit(_state: &LauncherState, x: usize, y: usize) -> QuickControlAction {
    // 1. Radio tiles (2x2 grid)
    for i in 0..CONNECTIVITY_LIST.len() {
        let col = i % TILE_ROWS;
        let row = i / TILE_ROWS;
        let rect = Rect::new(
            PANEL_X + col * (TILE_W + TILE_GUTTER),
            CONNECTIVITY_TOP + row * (TILE_H + TILE_GUTTER),
            TILE_W,
            TILE_H,
        );
        if rect.contains(x, y) {
            return match CONNECTIVITY_LIST[i] {
                PanelRow::Wifi => QuickControlAction::ToggleWifi,
                PanelRow::Bluetooth => QuickControlAction::ToggleBluetooth,
                PanelRow::Cellular => QuickControlAction::ToggleCellular,
                PanelRow::Airplane => QuickControlAction::ToggleAirplane,
                _ => QuickControlAction::Dismiss,
            };
        }
    }

    // 2. Brightness slider
    if BRIGHTNESS_ROW.contains(x, y) {
        let percent = slider_percent(x, BRIGHTNESS_TRACK);
        return QuickControlAction::SetBrightness(percent);
    }

    // 3. Volume slider
    if VOLUME_SLIDER.contains(x, y) {
        let percent = slider_percent(x, VOLUME_TRACK);
        return QuickControlAction::SetVolume(percent);
    }

    // 4. Secondary controls (vertical rows)
    for (i, &row_item) in SYSTEM_LIST.iter().enumerate() {
        let row_y = SYSTEM_TOP + i * ROW_H;
        let rect = Rect::new(PANEL_X, row_y, PANEL_W, ROW_H);
        if rect.contains(x, y) {
            return match row_item {
                PanelRow::Torch => QuickControlAction::ToggleTorch,
                PanelRow::DoNotDisturb => QuickControlAction::ToggleDnd,
                PanelRow::RotationLock => QuickControlAction::ToggleRotationLock,
                _ => QuickControlAction::Dismiss,
            };
        }
    }

    // 5. Lock row
    let lock_rect = Rect::new(PANEL_X, FOOTER_Y, PANEL_W, FOOTER_H);
    if lock_rect.contains(x, y) {
        return QuickControlAction::LockScreen;
    }

    // 6. Power button
    if POWER_BTN.contains(x, y) {
        return QuickControlAction::PowerMenu;
    }

    QuickControlAction::Dismiss
}

fn slider_percent(x: usize, track: Rect) -> u8 {
    if x <= track.x {
        0
    } else if x >= track.x + track.w {
        100
    } else {
        (((x - track.x) * 100) / track.w.max(1)).min(100) as u8
    }
}

