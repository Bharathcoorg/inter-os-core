// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

//! Quick Controls: the control centre.
//!
//! WHY the composition follows the reference shade: `Ui Samples/07_quick_controls.png`
//! lays out four radio tiles in a two-up grid, brightness and volume readings and
//! three secondary controls on a dark surface, and that order -- radios, then
//! levels, then secondary controls -- is the order of importance a phone's
//! control centre has. This panel keeps it.
//!
//! WHY every tile here says what is missing instead of a state: the guest has no
//! radio controller (`kernel/src/net/wireless.rs` reports `hardware_present:
//! false` for every band and telephony is forced to PowerDown/NoSim), no backlight
//! register it can program, and no flash. A tile that said "Connected", or a
//! trough filled to 100%, would be indistinguishable on screen from a radio that
//! works. So every unavailable row prints the reason in the kernel's own terms and
//! none of them is tappable.
//!
//! WHY volume is the exception: it is the one level on this surface the guest can
//! both read and write. `AudioManager::set_volume` programs the AC'97 master volume
//! register when the controller is bound and `get_volume` reads that register back,
//! so the percentage here is the mixer's own level and not a stored request -- the
//! same number the volume HUD and the Music app's meter already use. The row is
//! drawn and answered as the control it is.
//!
//! WHY three controls answer a tap: the volume steps, screen lock and the Power
//! menu each have a real backend (`std::audio_set_volume`, `std::user_lock` and the
//! guest power controller). Every other row returns `QuickControlAction::NoOp`, so
//! a press can never change a preference that no hardware would honour. The panel
//! is still fully drawn so the feature remains discoverable; it simply never
//! pretends to act.
//!
//! WHY the type is larger here than the settings rows elsewhere: this shade is
//! read at a glance while the phone is held at arm's length, and it carries only
//! ten rows. Section labels are BODY, row labels SECTION and values BODY, so each
//! line has a visible place in the hierarchy instead of collapsing into one
//! weight.

#![allow(dead_code)]

use launcher::layout::Rect;
use launcher::text;
use launcher::theme::{
    radius, space, type_scale, Theme, MIN_TOUCH_TARGET, SCREEN_HEIGHT, SCREEN_WIDTH,
    STATUS_BAR_HEIGHT,
};

use crate::screens::draw::{round_contains, Reading};
use crate::LauncherState;

/// WHY `Theme::SHELL_DARK`: the reference shade is a dark surface, and a control
/// centre is opened over the home surface rather than replacing it -- a light
/// sheet here would read as a different app.
const THEME: Theme = launcher::theme::SHELL_DARK;

// ---------------------------------------------------------------------------
// Panel geometry
// ---------------------------------------------------------------------------

/// Left inset of the panel.
const PANEL_X: usize = 36;

/// Panel width: the surface the shade occupies between its insets.
const PANEL_W: usize = SCREEN_WIDTH - PANEL_X * 2;

/// Side of the square icon box every row's glyph is drawn in.
const ICON: usize = 32;

/// Horizontal inset from a row's left edge to its text column: the icon gap, the
/// icon, the icon gap. Shared by every row so the glyphs line up.
const ROW_TEXT_INSET: usize = space::MD + ICON + space::MD;

/// Row label scale: a section title, large enough to read at a glance.
const ROW_SCALE: usize = type_scale::SECTION;

/// Value scale: one rung below the label, which is what makes a value a value
/// and not a second heading.
const VALUE_SCALE: usize = type_scale::BODY;

/// Section label scale: body copy above body rows.
const SECTION_SCALE: usize = type_scale::BODY;

/// Heading scale: the panel's own title.
const HEADING_SCALE: usize = type_scale::SECTION;

/// Heights of the type's line boxes, straight from the outline face. Using the
/// face's metrics (rather than the bitmap cell's 24 px multiples) keeps the
/// rows compact while leaving outline ink inside its box.
const ROW_H_LINE: usize = launcher::font::line_height(ROW_SCALE);
const VALUE_H: usize = launcher::font::line_height(VALUE_SCALE);
const SECTION_H: usize = launcher::font::line_height(SECTION_SCALE);
const HEADING_H: usize = launcher::font::line_height(HEADING_SCALE);

/// Gap between the heading and the summary line under it.
const HEADING_GAP: usize = space::XS;

/// Height of a flat reading row: the icon and a full line of type, with equal
/// padding above and below.
const ROW_H: usize = space::MD + ROW_H_LINE + space::MD;

/// Gap between one block's last pixel and the next section's label.
const SECTION_GAP: usize = space::XL;

/// Gap between a section label and the first row of its section.
const SECTION_LEAD: usize = SECTION_H + space::SM;

/// Everything one section header occupies above its content.
const SECTION_BLOCK: usize = SECTION_GAP + SECTION_LEAD;

/// Gutter between the two blocks of the level section, and between the footer's
/// reading and the Power pill.
const LEVEL_GUTTER: usize = space::LG;

/// Gutter between the tiles of the two-up radio grid.
const TILE_GUTTER: usize = space::MD;

/// Width of one radio tile: the panel split in two by that gutter.
const TILE_W: usize = (PANEL_W - TILE_GUTTER) / 2;

/// Rows of radio tiles.
const TILE_ROWS: usize = 2;

/// Horizontal padding inside a radio tile.
const TILE_PAD_X: usize = space::LG;

/// Text column inside a radio tile: what is left of the tile once the padding,
/// the icon and its gap are taken out.
const TILE_TEXT_W: usize = TILE_W - TILE_PAD_X - ICON - space::MD - TILE_PAD_X;

/// Vertical padding inside a radio tile, and the gap between its two lines.
const TILE_PAD_Y: usize = space::LG;
const TILE_TEXT_GAP: usize = space::XS;

/// Height of one radio tile: two lines of type inside that padding.
const TILE_H: usize = TILE_PAD_Y + ROW_H_LINE + TILE_TEXT_GAP + VALUE_H + TILE_PAD_Y;

/// Height of the two-up radio grid.
const GRID_H: usize = TILE_H * TILE_ROWS + TILE_GUTTER * (TILE_ROWS - 1);

/// Rows in the secondary controls section.
const SYSTEM_ROWS: usize = 3;

/// Height of that section, and of the level section's two reading rows.
const SYSTEM_LIST_H: usize = ROW_H * SYSTEM_ROWS;
const LEVEL_LIST_H: usize = ROW_H * 2;

/// Height of the footer's device reading row.
const FOOTER_H: usize = ROW_H;

/// Height of the Power entry pill.
const POWER_H: usize = space::LG + VALUE_H + space::LG;

/// Width of the Power entry pill.
///
/// WHY it is content-derived: the 32 px power glyph, a gap, the room the value
/// rung's own line needs for "Power menu", and `space::XL` of padding inside each
/// end. That room is carried as the 132 literal rather than a `text::measure` call,
/// pinned by the textharness contract that measuring and rasterising agree; the
/// outline advance the run answers at the BODY rung is a couple of pixels wider
/// than it today, and the pill's own padding absorbs the difference.
const POWER_BTN_W: usize = ICON + space::MD + 132 + space::XL * 2;

/// Home indicator, drawn at the foot of the surface.
const INDICATOR_W: usize = space::XL * 5;
const INDICATOR_H: usize = space::XS;

/// The panel's vertical band.
const LIST_BOTTOM: usize = SCREEN_HEIGHT - space::MD - INDICATOR_H - space::XS;
const AVAIL_TOP: usize = STATUS_BAR_HEIGHT + space::LG;
const AVAIL_BOT: usize = LIST_BOTTOM;

/// Height of the panel's content, block by block.
const CONTENT_H: usize = (HEADING_H + HEADING_GAP + VALUE_H)
    + SECTION_BLOCK
    + GRID_H
    + SECTION_BLOCK
    + LEVEL_LIST_H
    + SECTION_BLOCK
    + SYSTEM_LIST_H
    + SECTION_BLOCK
    + (FOOTER_H + LEVEL_GUTTER + POWER_H);

/// Vertical offset that centres the panel in its band.
const CONTENT_DY: usize = (AVAIL_BOT - AVAIL_TOP - CONTENT_H) / 2;
const HEADING_Y: usize = AVAIL_TOP + CONTENT_DY;
const SUMMARY_Y: usize = HEADING_Y + HEADING_H + HEADING_GAP;

const CONNECTIVITY_LABEL_Y: usize = SUMMARY_Y + VALUE_H + SECTION_GAP;
const CONNECTIVITY_TOP: usize = CONNECTIVITY_LABEL_Y + SECTION_LEAD;
const LEVEL_LABEL_Y: usize = CONNECTIVITY_TOP + GRID_H + SECTION_GAP;
const LEVEL_TOP: usize = LEVEL_LABEL_Y + SECTION_LEAD;
const SYSTEM_LABEL_Y: usize = LEVEL_TOP + LEVEL_LIST_H + SECTION_GAP;
const SYSTEM_TOP: usize = SYSTEM_LABEL_Y + SECTION_LEAD;
const FOOTER_LABEL_Y: usize = SYSTEM_TOP + SYSTEM_LIST_H + SECTION_GAP;
const FOOTER_Y: usize = FOOTER_LABEL_Y + SECTION_LEAD;
const POWER_Y: usize = FOOTER_Y + FOOTER_H + LEVEL_GUTTER;

/// The brightness reading's row.
pub const BRIGHTNESS_ROW: Rect = Rect::new(PANEL_X, LEVEL_TOP, PANEL_W, ROW_H);

/// The volume reading's row.
pub const VOLUME_ROW: Rect = Rect::new(PANEL_X, LEVEL_TOP + ROW_H, PANEL_W, ROW_H);

/// Side of a volume step square.
///
/// WHY the touch floor rather than the icon box: these are the two smallest
/// things a hand has to hit on this surface, and a square that is 32 px drawn
/// and 44 px answered is the defect this platform has already fixed everywhere
/// else.
const STEP_W: usize = MIN_TOUCH_TARGET;

/// Gap between the two step squares.
const STEP_GAP: usize = space::SM;

/// The volume row's step squares, as `[down, up]`.
///
/// WHY one function owns both the painting and the hit test: the recurring
/// failure on this surface has been a painter and a hit box that each carry
/// their own copy of the arithmetic and drift a few rows apart. Here there is
/// one definition and two callers, so they cannot disagree.
const fn volume_steps() -> [Rect; 2] {
    let up_x = VOLUME_ROW.x + VOLUME_ROW.w - space::MD - STEP_W;
    let down_x = up_x - STEP_GAP - STEP_W;
    let y = VOLUME_ROW.y + (VOLUME_ROW.h - STEP_W) / 2;
    [
        Rect::new(down_x, y, STEP_W, STEP_W),
        Rect::new(up_x, y, STEP_W, STEP_W),
    ]
}

const VOLUME_STEPS: [Rect; 2] = volume_steps();

/// The rows a volume step moves: the row carrying the reading and its squares.
///
/// WHY the whole row and not the two squares: the percentage sits in the same
/// card, so repainting only the wells would leave the old number next to the new
/// one.
pub const fn level_band() -> (usize, usize) {
    (VOLUME_ROW.y, VOLUME_ROW.y + VOLUME_ROW.h)
}

const _: () = assert!(
    VOLUME_STEPS[1].x + STEP_W <= VOLUME_ROW.x + VOLUME_ROW.w - space::MD,
    "the step squares must stay inside the row's right inset",
);
const _: () = assert!(
    VOLUME_STEPS[0].x > PANEL_X + ROW_TEXT_INSET,
    "the step squares must not land on the row's label column",
);

/// The Power entry pill.
pub const POWER_BTN: Rect = Rect::new(PANEL_X + (PANEL_W - POWER_BTN_W) / 2, POWER_Y, POWER_BTN_W, POWER_H);

const _: () = assert!(
    POWER_BTN.x + POWER_BTN.w <= 600,
    "the Power pill must sit clear of the shade's x >= 600 dismiss strip",
);
const _: () = assert!(
    ROW_H >= MIN_TOUCH_TARGET && POWER_BTN.h >= MIN_TOUCH_TARGET,
    "the exported controls must clear the minimum touch target",
);
const _: () = assert!(
    HEADING_Y >= AVAIL_TOP && HEADING_Y + CONTENT_H <= AVAIL_BOT,
    "the panel must fit the band between the status bar and the home indicator",
);

// ---------------------------------------------------------------------------
// The reading list
// ---------------------------------------------------------------------------

/// One row this panel can draw.
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

/// The two level readings: one flat, one with a control.
const LEVEL_LIST: [PanelRow; 2] = [PanelRow::Brightness, PanelRow::Volume];

/// The secondary controls.
const SYSTEM_LIST: [PanelRow; 3] = [PanelRow::Torch, PanelRow::DoNotDisturb, PanelRow::RotationLock];

/// The footer's device reading.
const FOOTER_LIST: [PanelRow; 1] = [PanelRow::Lock];

/// Every row, in drawing order.
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
            PanelRow::Wifi => Value::inactive(b"Unavailable"),
            PanelRow::Bluetooth => Value::inactive(b"Unavailable"),
            PanelRow::Cellular => Value::inactive(b"Unavailable"),
            PanelRow::Airplane => Value::inactive(b"Not applicable"),
            PanelRow::Brightness => Value::inactive(b"No backlight"),
            // WHY this one is a real number: the mixer is the level control this
            // guest actually has -- the AC'97 master volume is programmed by the
            // kernel and the shell already polls it for the volume HUD. Saying "no
            // mixer" here was an under-claim of a device the platform does have.
            PanelRow::Volume => {
                if state.volume_state_valid {
                    let mut buf = [0u8; 5];
                    let run = percent_run(state.last_volume_percent, &mut buf);
                    Value::active(run)
                } else {
                    Value::inactive(b"Unavailable")
                }
            }
            PanelRow::Torch => Value::inactive(b"No flash"),
            PanelRow::DoNotDisturb => Value::inactive(b"Not wired"),
            PanelRow::RotationLock => Value::inactive(b"Not wired"),
            PanelRow::Lock => Value::active(b"Tap to lock"),
        }
    }

    /// Whether a tap on this row may change anything.
    fn actionable(self) -> bool {
        matches!(self, PanelRow::Lock | PanelRow::Volume)
    }
}

/// One reading, plus the active state.
struct Value {
    reading: Reading,
    is_active: bool,
}

impl Value {
    fn active(run: &[u8]) -> Value {
        Value {
            reading: Reading::of(run),
            is_active: true,
        }
    }

    fn inactive(reason: &[u8]) -> Value {
        Value {
            reading: Reading::unavailable(reason),
            is_active: false,
        }
    }
}

/// Writes `NN%` into `buf` and returns the run it holds.
fn percent_run<'a>(percent: u8, buf: &'a mut [u8; 5]) -> &'a [u8] {
    let value = percent.min(100) as u16;
    let mut at = 0usize;
    if value >= 100 {
        buf[at] = b'1';
        at += 1;
        buf[at] = b'0';
        at += 1;
        buf[at] = b'0';
    } else if value >= 10 {
        buf[at] = b'0' + (value / 10) as u8;
        at += 1;
        buf[at] = b'0' + (value % 10) as u8;
    } else {
        buf[at] = b'0' + value as u8;
    }
    at += 1;
    buf[at] = b'%';
    &buf[..at + 1]
}

/// Row index of `item` in the published set, or 0.
fn item_index(item: PanelRow) -> usize {
    ALL_ROWS.iter().position(|row| *row == item).unwrap_or(0)
}

/// One reading per row, in `ALL_ROWS` order.
fn build_values(state: &LauncherState) -> [Value; ALL_ROWS.len()] {
    core::array::from_fn(|i| ALL_ROWS[i].reading(state))
}

// ---------------------------------------------------------------------------
// Frame
// ---------------------------------------------------------------------------

/// One run of text, measured and ink-resolved, ready for the pixel path.
///
/// WHY the box is resolved here and not in the pixel path: `pixel_aa` runs once
/// per pixel in the band the label covers, and all it needs is the run's own
/// box. Deriving that box per pixel meant re-measuring the whole string -- and
/// `text::measure` walks every byte through the advance table -- twice over for
/// every pixel of every tile, row and section label on the shade, which is
/// order a million measure calls for one repaint. `place` already measured the
/// run to decide where to lay it out, so the answer is stored and the per-pixel
/// path reads two fields instead. Nothing about the placement changes: the
/// stored `x0` is the same value `place` used to position the outline run.
struct LabelRun {
    scale: usize,
    len: usize,
    buf: [u8; 128],
    ink: u32,
    em: u32,
    w: usize,
    x0: usize,
    y0: usize,
    top: usize,
    box_h: usize,
    outline: Option<launcher::outline_text::OutlineRun>,
}

#[derive(Clone, Copy)]
enum Align {
    Left,
    Right,
}

impl LabelRun {
    const EMPTY: LabelRun = LabelRun {
        scale: 1,
        len: 0,
        buf: [0u8; 128],
        ink: 0,
        em: 0,
        w: 0,
        x0: 0,
        y0: 0,
        top: 0,
        box_h: 0,
        outline: None,
    };

    /// Measures `run` and left-aligns it inside `rect`, vertically centred in
    /// the rect's own height.
    fn left(rect: Rect, scale: usize, run: &[u8], ink: u32) -> LabelRun {
        Self::place(Align::Left, rect, scale, run, ink)
    }

    /// Measures `run` and right-aligns it inside `rect`, vertically centred.
    fn right(rect: Rect, scale: usize, run: &[u8], ink: u32) -> LabelRun {
        Self::place(Align::Right, rect, scale, run, ink)
    }

    fn place(align: Align, rect: Rect, scale: usize, run: &[u8], ink: u32) -> LabelRun {
        let len = run.len().min(128);
        let mut buf = [0u8; 128];
        buf[..len].copy_from_slice(&run[..len]);
        let em = launcher::outline_text::em_for_scale(scale);
        let (asc, desc) = interos_std::design::typeface::line_metrics(em);
        // WHY the face's line box and not the bitmap cell: the outline face is
        // what draws these runs, so centring on its own metrics puts the ink in
        // the middle of the row instead of leaving a bitmap cell's worth of
        // space below it.
        let line_h = launcher::font::line_height(scale);
        let baseline = (rect.y + rect.h.saturating_sub(line_h) / 2
            + launcher::font::ascent_for_scale(scale)).max(asc as usize);
        let top = baseline.saturating_sub(asc as usize);
        let w = text::measure(&buf[..len], scale)
            .max(interos_std::design::typeface::text_advance_px(&buf[..len], em) as usize);
        let x0 = match align {
            Align::Left => rect.x,
            Align::Right => rect.x + rect.w.saturating_sub(w),
        };
        // The bitmap fallback centres the run on this row; the outline path
        // never reads it, but the stored box has to stay the same box either
        // way, so it is resolved from the same two inputs.
        let y0 = rect.y + rect.h.saturating_sub(line_h) / 2;
        let outline = if len <= launcher::outline_text::KEY_MAX {
            launcher::outline_text::OutlineRun::place(x0, baseline, em, &buf[..len])
        } else {
            None
        };
        LabelRun {
            scale,
            len,
            buf,
            ink,
            em,
            w,
            x0,
            y0,
            top,
            box_h: (baseline + desc as usize).saturating_sub(top),
            outline,
        }
    }

    fn pixel_aa(&self, x: usize, y: usize, ink: u32, bg: u32) -> Option<u32> {
        if x < self.x0 || x - self.x0 >= self.w || y < self.top || y - self.top >= self.box_h {
            return None;
        }
        if let Some(ref run) = self.outline {
            return run.pixel(x, y, ink, bg);
        }
        crate::test_text_aa(x, y, self.x0, self.y0, self.scale, &self.buf[..self.len], ink, bg)
    }

    fn pixel_on(&self, x: usize, y: usize, bg: u32) -> Option<u32> {
        self.pixel_aa(x, y, self.ink, bg)
    }
}

/// One radio tile: a two-up grid cell with the label above the reason.
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

/// One row: a flat reading, or the one tappable card in the footer.
struct RowFrame {
    rect: Rect,
    corner: usize,
    fill: Option<u32>,
    border: Option<u32>,
    ink: u32,
    icon: QcIcon,
    icon_x: usize,
    icon_y: usize,
    label: LabelRun,
    value: LabelRun,
    divider_y: Option<usize>,
    /// The row's step-down and step-up squares, when it has them.
    steps: Option<[Rect; 2]>,
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
        divider_y: None,
        steps: None,
    };
}

struct QuickControlsFrame {
    clock: LabelRun,
    heading: LabelRun,
    summary: LabelRun,
    sections: [LabelRun; 4],
    radios: [TileFrame; CONNECTIVITY_LIST.len()],
    levels: [RowFrame; LEVEL_LIST.len()],
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
        levels: [RowFrame::EMPTY; LEVEL_LIST.len()],
        controls: [RowFrame::EMPTY; SYSTEM_LIST.len()],
        lock: RowFrame::EMPTY,
        power_icon_x: 0,
        power_icon_y: 0,
        power_label: LabelRun::EMPTY,
    };

    fn new(state: &LauncherState) -> QuickControlsFrame {
        let theme = &THEME;
        let values = build_values(state);

        // The clock is the shell's canonical status clock: left, 40 px in, on
        // the body line. A reading the kernel refused leaves the strip blank
        // rather than showing a plausible time.
        let clock = if state.clock_source_ok {
            LabelRun::left(
                Rect::new(40, 8, SCREEN_WIDTH - 80, VALUE_H),
                type_scale::BODY,
                &state.clock_time,
                theme.on_surface,
            )
        } else {
            LabelRun::EMPTY
        };

        let heading = LabelRun::left(
            Rect::new(PANEL_X, HEADING_Y, PANEL_W, HEADING_H),
            HEADING_SCALE,
            b"Quick Controls",
            theme.on_surface,
        );
        let summary = LabelRun::left(
            Rect::new(PANEL_X, SUMMARY_Y, PANEL_W, VALUE_H),
            VALUE_SCALE,
            b"Volume, screen lock and power answer here.",
            theme.on_surface_variant,
        );
        let sections = [
            LabelRun::left(Rect::new(PANEL_X, CONNECTIVITY_LABEL_Y, PANEL_W, SECTION_H), SECTION_SCALE, b"Connectivity", theme.on_surface_variant),
            LabelRun::left(Rect::new(PANEL_X, LEVEL_LABEL_Y, PANEL_W, SECTION_H), SECTION_SCALE, b"Brightness and volume", theme.on_surface_variant),
            LabelRun::left(Rect::new(PANEL_X, SYSTEM_LABEL_Y, PANEL_W, SECTION_H), SECTION_SCALE, b"Secondary controls", theme.on_surface_variant),
            LabelRun::left(Rect::new(PANEL_X, FOOTER_LABEL_Y, PANEL_W, SECTION_H), SECTION_SCALE, b"Device and power", theme.on_surface_variant),
        ];

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

        // Brightness stays a flat reading: there is no backlight register this
        // guest can program, so there is no level to draw a trough for and no
        // square worth tapping. Volume gets the pair, because it is the one
        // reading here the device both reports and accepts.
        const LEVEL_DIVIDER: usize = LEVEL_TOP + ROW_H;
        let levels = [
            build_row(
                BRIGHTNESS_ROW,
                LEVEL_LIST[0],
                &values[item_index(LEVEL_LIST[0])],
                None,
                None,
                theme,
            ),
            build_row(
                VOLUME_ROW,
                LEVEL_LIST[1],
                &values[item_index(LEVEL_LIST[1])],
                Some(LEVEL_DIVIDER),
                Some(VOLUME_STEPS),
                theme,
            ),
        ];

        let controls = {
            let mut out = [RowFrame::EMPTY; SYSTEM_LIST.len()];
            let mut i = 0;
            while i < SYSTEM_LIST.len() {
                let y = SYSTEM_TOP + i * ROW_H;
                out[i] = build_row(
                    Rect::new(PANEL_X, y, PANEL_W, ROW_H),
                    SYSTEM_LIST[i],
                    &values[item_index(SYSTEM_LIST[i])],
                    (i > 0).then_some(y),
                    None,
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
            None,
            theme,
        );

        let power_run = ICON + space::MD + text::measure(b"Power menu", VALUE_SCALE);
        let power_icon_x = POWER_BTN.x + (POWER_BTN.w - power_run) / 2;
        let power_icon_y = POWER_BTN.center_y() - ICON / 2;
        let power_label = LabelRun::left(
            Rect::new(power_icon_x + ICON + space::MD, POWER_BTN.y, POWER_BTN.w, POWER_BTN.h),
            VALUE_SCALE,
            b"Power menu",
            theme.on_surface,
        );

        QuickControlsFrame {
            clock,
            heading,
            summary,
            sections,
            radios,
            levels,
            controls,
            lock,
            power_icon_x,
            power_icon_y,
            power_label,
        }
    }
}

/// Builds one radio tile.
fn build_tile(rect: Rect, item: PanelRow, value: &Value, theme: &Theme) -> TileFrame {
    // An unavailable radio is a recessed slot: no accent fill, full-strength
    // label so it can be read, and the kernel's reason in muted ink.
    let fill = theme.surface_container;
    let border = theme.border;
    let label_ink = theme.on_surface;
    let value_ink = theme.on_surface_variant;
    let text_x = rect.x + TILE_PAD_X + ICON + space::MD;
    let label_y = rect.y + TILE_PAD_Y;
    let value_y = label_y + ROW_H_LINE + TILE_TEXT_GAP;
    TileFrame {
        rect,
        icon: item.icon(),
        icon_x: rect.x + TILE_PAD_X,
        icon_y: rect.center_y() - ICON / 2,
        fill,
        border,
        ink: theme.on_surface,
        label: LabelRun::left(
            Rect::new(text_x, label_y, TILE_TEXT_W, ROW_H_LINE),
            ROW_SCALE,
            item.label(),
            label_ink,
        ),
        value: LabelRun::left(
            Rect::new(text_x, value_y, TILE_TEXT_W, VALUE_H),
            VALUE_SCALE,
            value.reading.as_slice(),
            value_ink,
        ),
    }
}

/// Builds a flat reading row: icon, label and value on the page background.
///
/// `steps` is the pair of squares a level row is adjusted with. Only the mixer
/// has them: it is the one reading on this surface the device can both report and
/// change, so the row that shows it is also the row that answers a tap on either
/// end of it.
fn build_row(
    rect: Rect,
    item: PanelRow,
    value: &Value,
    divider_y: Option<usize>,
    steps: Option<[Rect; 2]>,
    theme: &Theme,
) -> RowFrame {
    // A raised card is what tells a reading apart from a control, so any row
    // with steps is drawn as one whether or not its value arrived.
    let (fill, border) = if item.actionable() {
        (Some(theme.surface_container), Some(theme.border))
    } else {
        (None, None)
    };
    // WHY the reserve is measured here: the value is right-aligned inside what
    // is left of the squares, and a value that ran under a square would be a
    // number and a button answering the same pixel.
    let reserve = steps.map_or(0, |s| rect.x + rect.w - s[0].x + space::SM);
    let (label, value_run) = row_runs(rect, rect.y, rect.h, item.label(), &value.reading, reserve, theme);
    RowFrame {
        rect,
        corner: radius::CONTROL,
        fill,
        border,
        ink: theme.on_surface,
        icon: item.icon(),
        icon_x: rect.x + space::MD,
        icon_y: rect.center_y() - ICON / 2,
        label,
        value: value_run,
        divider_y,
        steps,
    }
}

/// The label and value runs of a full-width row, sharing one baseline.
fn row_runs(
    rect: Rect,
    text_y: usize,
    text_h: usize,
    label: &'static [u8],
    value: &Reading,
    reserve: usize,
    theme: &Theme,
) -> (LabelRun, LabelRun) {
    let text_x = rect.x + ROW_TEXT_INSET;
    let text_w = rect.w.saturating_sub(ROW_TEXT_INSET + space::MD + reserve);
    let value_w = text::measure(value.as_slice(), VALUE_SCALE).min(text_w / 2) + space::SM;
    let label_h = launcher::font::line_height(ROW_SCALE);
    let value_h = launcher::font::line_height(VALUE_SCALE);
    let label_y = text_y + text_h.saturating_sub(label_h) / 2;
    // WHY the value box is shifted by the ascent difference: the two runs are
    // different sizes, so centring each on its own line box would leave their
    // baselines a few pixels apart. Sharing a baseline is what makes a value
    // sit on the same optical line as its label.
    let baseline_delta = launcher::font::ascent_for_scale(ROW_SCALE)
        .saturating_sub(launcher::font::ascent_for_scale(VALUE_SCALE));
    let value_y = label_y + baseline_delta;
    // WHY the value ends where the reserve begins: a number that ran under the
    // square beside it would be a reading and a button answering the same pixel.
    let content_right = rect.x + rect.w - space::MD - reserve;
    let value_col = Rect::new(content_right - value_w, value_y, value_w, value_h);
    let label_col = Rect::new(text_x, label_y, content_right - value_w - text_x, label_h);
    (
        LabelRun::left(label_col, ROW_SCALE, label, theme.on_surface),
        LabelRun::right(value_col, VALUE_SCALE, value.as_slice(), theme.on_surface_variant),
    )
}

// ---------------------------------------------------------------------------
// Painting
// ---------------------------------------------------------------------------

static mut FRAME_CACHE: Option<QuickControlsFrame> = None;

/// Drops the cached frame so the next read rebuilds it from the current state.
///
/// WHY this has to be callable: the cache rebuilds when a pixel read starts at
/// (0,0), which is what a whole-surface pass always does and a single band never
/// does. A volume step moves one row, and without this the shade would keep
/// drawing the number from before the step.
pub fn invalidate_frame_cache() {
    unsafe {
        let slot: *mut Option<QuickControlsFrame> = core::ptr::addr_of_mut!(FRAME_CACHE);
        *slot = None;
    }
}

#[inline(never)]
pub fn get_quick_controls_color(x: usize, y: usize, state: &LauncherState) -> u32 {
    let cached = unsafe {
        let slot: *mut Option<QuickControlsFrame> = core::ptr::addr_of_mut!(FRAME_CACHE);
        if (x == 0 && y == 0) || (*slot).is_none() {
            *slot = Some(QuickControlsFrame::new(state));
        }
        (*slot).as_ref()
    };
    // WHY a reference to the theme: `Theme` is four hundred and sixty-four bytes
    // of colour roles, and this function is called once per pixel. Copying it per
    // call put half a gigabyte of stack traffic through one shade repaint for no
    // colour change at all; the roles read identically from the const.
    let Some(frame) = cached else {
        return THEME.surface;
    };
    quick_controls_pixel(x, y, frame, &THEME)
}

fn quick_controls_pixel(x: usize, y: usize, frame: &QuickControlsFrame, theme: &Theme) -> u32 {
    if y < STATUS_BAR_HEIGHT {
        return status_bar_pixel(x, y, frame, theme);
    }
    if y >= LIST_BOTTOM {
        return home_indicator_pixel(x, y, theme);
    }
    if y < HEADING_Y {
        return theme.surface;
    }
    if y < SUMMARY_Y {
        if let Some(c) = frame.heading.pixel_on(x, y, theme.surface) {
            return c;
        }
        return theme.surface;
    }
    if y < CONNECTIVITY_LABEL_Y {
        if let Some(c) = frame.summary.pixel_on(x, y, theme.surface) {
            return c;
        }
        return theme.surface;
    }
    if y < CONNECTIVITY_TOP {
        if let Some(c) = frame.sections[0].pixel_on(x, y, theme.surface) {
            return c;
        }
        return theme.surface;
    }
    if y < LEVEL_LABEL_Y {
        for tile in &frame.radios {
            if tile.rect.contains(x, y) {
                return tile_pixel(x, y, tile, theme);
            }
        }
        return theme.surface;
    }
    if y < LEVEL_TOP {
        if let Some(c) = frame.sections[1].pixel_on(x, y, theme.surface) {
            return c;
        }
        return theme.surface;
    }
    if y < SYSTEM_LABEL_Y {
        for row in &frame.levels {
            if row.rect.contains(x, y) {
                return row_pixel(x, y, row, theme);
            }
        }
        return theme.surface;
    }
    if y < SYSTEM_TOP {
        if let Some(c) = frame.sections[2].pixel_on(x, y, theme.surface) {
            return c;
        }
        return theme.surface;
    }
    if y < FOOTER_LABEL_Y {
        for row in &frame.controls {
            if row.rect.contains(x, y) {
                return row_pixel(x, y, row, theme);
            }
        }
        return theme.surface;
    }
    if y < FOOTER_Y {
        if let Some(c) = frame.sections[3].pixel_on(x, y, theme.surface) {
            return c;
        }
        return theme.surface;
    }
    if y < POWER_Y {
        if frame.lock.rect.contains(x, y) {
            return row_pixel(x, y, &frame.lock, theme);
        }
        return theme.surface;
    }
    if POWER_BTN.contains(x, y) {
        return power_pixel(x, y, frame, theme);
    }
    theme.surface
}

fn tile_pixel(x: usize, y: usize, tile: &TileFrame, theme: &Theme) -> u32 {
    if !round_contains(&tile.rect, radius::CARD, x, y) {
        return theme.surface;
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

fn row_pixel(x: usize, y: usize, row: &RowFrame, theme: &Theme) -> u32 {
    if let Some(divider_y) = row.divider_y {
        if y == divider_y && x >= row.rect.x && x < row.rect.x + row.rect.w {
            return theme.border;
        }
    }

    let base = match row.fill {
        Some(fill) => {
            if !round_contains(&row.rect, row.corner, x, y) {
                return theme.surface;
            }
            if !round_contains(&row.rect.inset(1), row.corner - 1, x, y) {
                return row.border.unwrap_or(theme.border);
            }
            fill
        }
        None => theme.surface,
    };

    // WHY the squares are recessed wells on a raised card: the row's reading is
    // a number and these two are something a hand can press, and the only way
    // a flat surface says that without a label is the fill.
    if let Some(steps) = row.steps {
        let mut i = 0;
        while i < steps.len() {
            let square = steps[i];
            if round_contains(&square, radius::CONTROL, x, y) {
                return step_pixel(i, square, x, y, theme);
            }
            i += 1;
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

/// One volume step square: `index` 0 is the minus, 1 the plus.
///
/// WHY the stroke is 5 px and 25 px across: it matches the weight and the reach
/// of the row's own glyphs, so a minus here and a minus in the status bar read
/// as the same mark rather than as two different alphabets.
fn step_pixel(index: usize, square: Rect, x: usize, y: usize, theme: &Theme) -> u32 {
    let fill = theme.surface_container_low;
    let cx = square.center_x();
    let cy = square.center_y();
    let within = |centre: usize, extent: usize, value: usize| {
        value >= centre.saturating_sub(extent) && value < centre + extent + 1
    };
    let bar = within(cx, space::LG / 2, x) && within(cy, 2, y);
    let stem = index == 1 && within(cx, 2, x) && within(cy, space::LG / 2, y);
    if bar || stem {
        return theme.on_surface;
    }
    fill
}

fn power_pixel(x: usize, y: usize, frame: &QuickControlsFrame, theme: &Theme) -> u32 {
    if !round_contains(&POWER_BTN, POWER_BTN.h / 2, x, y) {
        return theme.surface;
    }
    if !round_contains(&POWER_BTN.inset(1), POWER_BTN.h / 2 - 1, x, y) {
        return theme.error;
    }
    let bg = theme.error_container;
    if icon_hits(QcIcon::Power, frame.power_icon_x, frame.power_icon_y, x, y) {
        return theme.error;
    }
    if let Some(c) = frame.power_label.pixel_aa(x, y, theme.on_surface, bg) {
        return c;
    }
    bg
}

/// Status bar: the shell's canonical clock and the same honest telemetry the
/// home screen draws, on the dark ink of this surface.
fn status_bar_pixel(x: usize, y: usize, frame: &QuickControlsFrame, theme: &Theme) -> u32 {
    crate::screens::status::pixel_with(
        x,
        y,
        |x, y| frame.clock.pixel_on(x, y, theme.surface),
        theme,
    )
}

fn home_indicator_pixel(x: usize, y: usize, theme: &Theme) -> u32 {
    let iy = LIST_BOTTOM + space::MD;
    let ix = (SCREEN_WIDTH - INDICATOR_W) / 2;
    let rect = Rect::new(ix, iy, INDICATOR_W, INDICATOR_H);
    if !rect.contains(x, y) {
        return theme.surface;
    }
    if !round_contains(&rect, rect.h / 2, x, y) {
        return theme.surface;
    }
    Theme::blend(theme.on_surface_variant, theme.surface, 140)
}

// ---------------------------------------------------------------------------
// Icons
// ---------------------------------------------------------------------------

/// Glyphs this panel draws, as 32x32 bit patterns.
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
        // Three arcs of a fan plus a dot at their centre. The arcs are resolved
        // per column: an annulus written as a squared-distance band lands on
        // sparse lattice points at these radii and reads as a dashed cloud.
        QcIcon::Wifi => {
            let dx = x as i64 - 16;
            if (-1..=0).contains(&dx) && (25..=26).contains(&y) {
                return true;
            }
            let dy = 26i64 - y as i64;
            if dy > 0 {
                for r in [5i64, 10, 15] {
                    if dx.abs() <= r - 2 {
                        let d2 = dx * dx + dy * dy;
                        if (d2 - r * r).abs() <= 3 * r / 2 {
                            return true;
                        }
                    }
                }
            }
            false
        }
        // The rune: a stem with two diagonals that meet at the right at
        // mid-height.
        QcIcon::Bluetooth => {
            icon_line(x, y, 16, 3, 16, 29, 1)
                || icon_line(x, y, 16, 3, 25, 10, 1)
                || icon_line(x, y, 25, 10, 16, 16, 1)
                || icon_line(x, y, 16, 16, 25, 22, 1)
                || icon_line(x, y, 25, 22, 16, 29, 1)
                || icon_line(x, y, 16, 16, 8, 9, 1)
                || icon_line(x, y, 16, 16, 8, 23, 1)
        }
        // Four ascending bars.
        QcIcon::Cellular => on(3, 22, 4, 7) || on(10, 16, 4, 13) || on(17, 10, 4, 19) || on(24, 4, 4, 25),
        // A disc with eight crisp two-pixel rays. The earlier version gated a
        // bounding box per ray, which filled each diagonal ray as a block and
        // left the glyph reading as a dashed gear.
        QcIcon::Sun => {
            let dx = x as i64 - 16;
            let dy = y as i64 - 16;
            if dx * dx + dy * dy <= 4 * 4 {
                return true;
            }
            for (ux, uy) in [
                (0i64, 1i64),
                (1, 0),
                (0, -1),
                (-1, 0),
                (1, 1),
                (1, -1),
                (-1, 1),
                (-1, -1),
            ] {
                if icon_line(
                    x,
                    y,
                    16 + ux * 8,
                    16 + uy * 8,
                    16 + ux * 13,
                    16 + uy * 13,
                    1,
                ) {
                    return true;
                }
            }
            false
        }
        QcIcon::Speaker => {
            if on(5, 12, 5, 8) || on(10, 9, 4, 14) || on(14, 12, 3, 8) {
                return true;
            }
            let dx = x as i64 - 18;
            let dy = y as i64 - 16;
            if dx > 0 {
                let d2 = dx * dx + dy * dy;
                for r in [5i64, 9] {
                    if (d2 - r * r).abs() <= 3 * r / 2 {
                        return true;
                    }
                }
            }
            false
        }
        // A torch seen from the side: lamp head, neck, body, tail and two
        // short beam rays. The previous shape was a stem with two floating
        // blocks, which read as an exclamation mark rather than a flashlight.
        QcIcon::Torch => {
            on(11, 7, 10, 6)
                || on(14, 13, 4, 3)
                || on(13, 16, 6, 11)
                || on(14, 27, 4, 3)
                || icon_line(x, y, 10, 6, 6, 2, 1)
                || icon_line(x, y, 22, 6, 26, 2, 1)
        }
        QcIcon::Rotate => {
            let dx = x as i64 - 16;
            let dy = y as i64 - 16;
            let d2 = dx * dx + dy * dy;
            let ring = d2 <= 12 * 12 && d2 >= 10 * 10 && !(dx > 0 && dy < 0);
            ring || icon_line(x, y, 23, 4, 30, 11, 1) || icon_line(x, y, 23, 18, 30, 11, 1)
        }
        QcIcon::Moon => {
            let dx = x as i64 - 16;
            let dy = y as i64 - 16;
            if dx * dx + dy * dy > 12 * 12 {
                return false;
            }
            let ex = x as i64 - 24;
            let ey = y as i64 - 15;
            ex * ex + ey * ey > 10 * 10
        }
        QcIcon::Airplane => {
            if on(15, 2, 3, 28) || on(3, 15, 26, 3) || on(8, 22, 16, 2) || on(9, 9, 14, 2) {
                return true;
            }
            let dx = x as i64 - 16;
            let dy = y as i64 - 4;
            (dx * dx + dy * dy) as usize <= 3 * 3
        }
        QcIcon::Lock => {
            if on(7, 15, 19, 12) || on(10, 6, 3, 10) || on(20, 6, 3, 10) || on(10, 5, 13, 3) {
                return true;
            }
            on(15, 20, 3, 3)
        }
        QcIcon::Power => {
            let dx = x as i64 - 16;
            let dy = y as i64 - 16;
            let d = (dx * dx + dy * dy) as usize;
            if d >= 10 * 10 && d <= 13 * 13 && dy <= 1 {
                return true;
            }
            on(15, 2, 3, 14)
        }
    }
}

// ---------------------------------------------------------------------------
// Hit testing
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum QuickControlAction {
    LockScreen,
    PowerMenu,
    /// One 10-point notch off the mixer's master volume.
    VolumeDown,
    /// One 10-point notch onto the mixer's master volume.
    VolumeUp,
    /// A tap on a row with no backend. Deliberately not a dismiss: pressing a
    /// control that cannot act must leave the shade where the user found it.
    NoOp,
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
            return QuickControlAction::NoOp;
        }
    }

    // 2. Brightness and volume readings.
    //
    // WHY the squares are tested before their own row: the row is a reading and
    // the squares are controls, and a tap inside the row that lands on a square
    // must change the level rather than be swallowed by the row's no-op.
    if let Some(square) = VOLUME_STEPS.iter().position(|s| s.contains(x, y)) {
        return if square == 1 {
            QuickControlAction::VolumeUp
        } else {
            QuickControlAction::VolumeDown
        };
    }
    if BRIGHTNESS_ROW.contains(x, y) || VOLUME_ROW.contains(x, y) {
        return QuickControlAction::NoOp;
    }

    // 3. Secondary controls (vertical rows)
    for i in 0..SYSTEM_LIST.len() {
        let rect = Rect::new(PANEL_X, SYSTEM_TOP + i * ROW_H, PANEL_W, ROW_H);
        if rect.contains(x, y) {
            return QuickControlAction::NoOp;
        }
    }

    // 4. Lock row: the one row here with a backend.
    let lock_rect = Rect::new(PANEL_X, FOOTER_Y, PANEL_W, FOOTER_H);
    if lock_rect.contains(x, y) {
        return QuickControlAction::LockScreen;
    }

    // 5. Power entry
    if POWER_BTN.contains(x, y) {
        return QuickControlAction::PowerMenu;
    }

    QuickControlAction::Dismiss
}
