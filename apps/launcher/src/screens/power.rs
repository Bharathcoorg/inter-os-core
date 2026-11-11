// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

//! Power Menu: full-screen modern modal sheet with Power off, Restart, Lock screen, and Emergency.
//!
//! Production flagship quality with large circular action buttons, bold typography,
//! color accents, and full integration with system power transitions and lock screen.
//!
//! ONE BOX PER CONTROL, AT THE RUNG THE SHELL CARRIES: every rectangle this modal
//! answers with is built once, by a `const fn` of the text level, and the painter and
//! the press test call the same one. The rule is not stylistic. The sheet used to keep
//! its Cancel pill in a painted constant and its Cancel press in a band solved from the
//! row above it plus four rows of hand-tuned air, and the two drifted: the top rows of
//! the visible pill answered Restart and Emergency instead of the button they were
//! inked on, so a press on the pill's own top edge could reboot the phone. The sides
//! and the bottom were wrong in the other direction, answering Cancel for pixels with no
//! ink anywhere near them. A press is only ever as honest as the box it is measured
//! against, so there is now exactly one box per control and both callers name it -- and
//! the pill is tested first, before the row bands.
//!
//! The same derivation carries the person's text size. Both callers resolve
//! [`scale::draw_level`] -- the rung this shell carries, which is the published rung as
//! the drawer's clamp leaves it -- once per call, and the header's two lines, the row
//! pitch, the plates, the pill and the sheet's own height are all solved from that one
//! answer through [`launcher::scale`]. [`stack_holds_at_every_level`] refuses the day the
//! grown stack stops closing inside the sheet it is drawn on. What is identity-pinned is
//! the geometry: at [`scale::PLATFORM_LEVEL`] every face below answers the row and the box
//! this modal was drawn with. One thing deliberately is not: an action's name is now
//! centred in the lane its press answers with the platform's painted-width centring,
//! which sets a name of odd width one column left of the old centre-minus-half
//! arithmetic. That is the rule the app drawer already follows, and it is the rule that
//! makes the ink and the box agree.

#![allow(unused_imports, dead_code)]

use super::draw::round_contains;
use crate::screens::status;
use crate::{test_text_aa_level, LauncherState};
use launcher::layout::Rect;
use launcher::scale;
use launcher::scale::later;
use launcher::text;
use launcher::theme::{type_scale, Theme, SCREEN_HEIGHT, SCREEN_WIDTH, STATUS_BAR_HEIGHT};

const COLOR_BG_TOP: u32 = launcher::theme::SHELL_DARK.surface;
const COLOR_BG_BOT: u32 = 0xFF05070A;

/// The sheet as it was authored, at [`scale::PLATFORM_LEVEL`].
///
/// WHY it stays as a constant rather than dissolving into the derivation: it is the
/// anchor [`sheet_rect_at`] is pinned to, so the claim "a reader who never raised Text
/// size sees the pixels they saw before" is testable instead of remembered. Nothing
/// paints or presses against this rectangle directly.
const SHEET_RECT: Rect = Rect::new(40, 60, 640, 545);
const SHEET_RADIUS: usize = 32;
const SHEET_FILL: u32 = 0xFF12151C;
const SHEET_BORDER: u32 = 0xFF282F3D;

/// Rows the sheet keeps below the Cancel pill: the grown sheet closes on the pill plus
/// this air, so the sheet's height is a consequence of the stack it holds rather than a
/// number the stack has to be slipped under.
const SHEET_BOTTOM_AIR: usize = 27;

const HANDLE_RECT: Rect = Rect::new(330, 76, 60, 5);
const HANDLE_COLOR: u32 = 0xFF3B4454;

// Sheet header: the title and the line of instruction under it.
//
// WHY these two rows are derived from each other and not both typed: the title is a
// rung-4 run and reserves its own line box from its top. `158` was chosen against the
// box the same rung reserved before that box rose, so the description began one row
// inside the title's own box again -- exactly the collision the description's own
// advance is there to prevent. The controls under the header start at the bottom of the
// description's box for the same reason, at every rung, which is why the row below is a
// `const fn` of the level rather than the constant this module used to hold.
const TITLE_SCALE: usize = 4;
const DESC_SCALE: usize = 2;
const TITLE_Y0: usize = 96;

/// Rows the header keeps between the description's box and the first circle.
///
/// WHY it is named: the first row of buttons sits at the later of its authored centre and
/// one circle radius plus this air below the header, so the air is the number that says
/// the header can never ride into the control it is stacked over.
const HEADER_AIR: usize = 5;

/// The first row the two top button plates answer: the row the whole description box
/// closes on, so a tap on the header is never read as the control painted under it.
///
/// WHY this replaces the literal `190`: 190 was the row the description's box closed on
/// before the ladder rose. The box grew, so its last rows were inside the Power Off and
/// Lock Screen ranges while the painter still drew ink there.
const fn row0_hit_top_at(level: u8) -> usize {
    desc_y0_at(level) + scale::line_at(DESC_SCALE, level)
}

// Circle button geometry: radius 36 (72x72px diameter)
const BTN_RADIUS: i32 = 36;
const BTN_R2: i32 = BTN_RADIUS * BTN_RADIUS;

/// Centers for the 2x2 grid
const COL0_CX: usize = 220;
const COL1_CX: usize = 500;
const ROW0_CY: usize = 240;

/// The rung a button's name is drawn at, and the rung its subject line is drawn at.
const NAME_SCALE: usize = 3;
const SUBJECT_SCALE: usize = 2;

/// Columns one action owns: the pitch between the two centres, and the half each side
/// gets.
///
/// WHY the pitch and not the drawn circle: the circle is 72 columns wide and the name
/// under it is wider, so the box a press is measured against has to be the lane the ink
/// is centred in -- and the two lanes have to tile the sheet with no dead column between
/// them and no shared column across them. `220 + 140 == 500 - 140`, which the gate below
/// refuses the day the centres stop being symmetric about it.
const COL_LANE: usize = COL1_CX - COL0_CX;
const COL_LANE_HALF: usize = COL_LANE / 2;

/// Columns each action keeps inside its lane on either side of the run it centres.
///
/// WHY four and not more: the widest name the top rung offers is exactly the lane less
/// these two pairs of columns, so a reader at extra-large gets every button's name whole
/// rather than elided, and the two lanes still cannot share a column.
const GRID_LANE_PAD: usize = 4;

/// Rows a button's two-line block reserves below its circle at a rung: the name's whole
/// line box plus the subject's.
///
/// WHY it is a face of the level: the row pitch, the boundary between the two rows, the
/// top of the Cancel pill and the sheet's own height are all solved from this one
/// number, so a rung that changes the boxes moves the modal instead of overrunning it.
const fn block_h_at(level: u8) -> usize {
    scale::line_at(NAME_SCALE, level) + scale::line_at(SUBJECT_SCALE, level)
}

/// WHY row 1 stands a whole block plus two circle radii under row 0 rather than the 140
/// it used to: a button's two-line block is a name box and a subject box, so the pitch
/// that clears row 0's ink and keeps row 1's subject off its own circle is
/// `2 * BTN_RADIUS + block_h`. Because the pitch is built from the same block that ends
/// row 0, the two rows' plates tile with no row between them at every rung -- which is
/// what the sweep below checks rather than hopes.
const fn row_pitch_at(level: u8) -> usize {
    2 * BTN_RADIUS as usize + block_h_at(level)
}

const fn desc_y0_at(level: u8) -> usize {
    TITLE_Y0 + scale::line_at(TITLE_SCALE, level)
}

/// The first row below the header's instruction line: where the sheet's controls begin.
const fn header_bottom_at(level: u8) -> usize {
    row0_hit_top_at(level)
}

const fn row0_cy_at(level: u8) -> usize {
    later(ROW0_CY, header_bottom_at(level) + BTN_RADIUS as usize + HEADER_AIR)
}

const fn row1_cy_at(level: u8) -> usize {
    row0_cy_at(level) + row_pitch_at(level)
}

const fn row_cy_at(row: usize, level: u8) -> usize {
    if row == 0 {
        row0_cy_at(level)
    } else {
        row1_cy_at(level)
    }
}

/// WHY the name sits a full button-radius below the centre: the circle's own ink ends at
/// its radius, so a name any higher is painted on the button it names rather than under
/// it.
const fn name_y0_at(row: usize, level: u8) -> usize {
    row_cy_at(row, level) + BTN_RADIUS as usize
}

/// WHY the subject's top is the name's whole line box below the name and not a fixed 36
/// rows: a name run reserves its own line box at its rung, and the old 36-row stride was
/// the 8x16 bitmap cell's, shorter than the name it followed, so the subject's first rows
/// painted straight through the name's descender. Starting exactly one line box lower
/// makes the two boxes adjacent, so no descender can be read through.
const fn subject_y0_at(row: usize, level: u8) -> usize {
    name_y0_at(row, level) + scale::line_at(NAME_SCALE, level)
}

/// The row band boundary between the two button rows: the top of row 1's circle, which is
/// also the row row 0's block closes on.
///
/// WHY the circle's own edge and not the midpoint of the two centres: the circle is the
/// primary target, so a tap on it must never be read as the row above, and a label block
/// that grew toward it is the row's own.
const fn row_boundary_at(level: u8) -> usize {
    row1_cy_at(level) - BTN_RADIUS as usize
}

/// One past the last row an action's block reserves.
const fn block_bottom_at(row: usize, level: u8) -> usize {
    subject_y0_at(row, level) + scale::line_at(SUBJECT_SCALE, level)
}

/// The first row an action's plate answers.
///
/// WHY the row above's end and not this row's circle: the plates have to tile. A plate
/// that started at its own circle would leave the rows between the header and the circle
/// unclaimed while the sheet paints them, and a tap there would dismiss a modal the
/// reader was reaching into.
const fn plate_top_at(row: usize, level: u8) -> usize {
    if row == 0 {
        header_bottom_at(level)
    } else {
        row_boundary_at(level)
    }
}

/// The whole box one action owns: its lane across the sheet, from the row the control
/// above it ends to the row its own block closes on.
///
/// WHY this is the only place the box is built: the painter clips an action's circle, icon
/// and two lines to it, and the press test returns the action it is drawn for. This
/// rectangle and [`cancel_rect_at`] are the two shapes the modal answers with, and neither
/// is written twice.
const fn action_plate_at(cx: usize, row: usize, level: u8) -> Rect {
    let top = plate_top_at(row, level);
    Rect::new(cx - COL_LANE_HALF, top, COL_LANE, block_bottom_at(row, level) - top)
}

fn action_plate(cx: usize, row: usize, level: u8) -> Rect {
    action_plate_at(cx, row, level)
}

/// The Cancel pill as it was authored, at [`scale::PLATFORM_LEVEL`], and the identity the
/// derivation below is pinned to.
///
/// WHY it survived as a constant: the pill's width and its corners are authored pixels the
/// shell has to keep drawing at Normal, and [`cancel_rect_at`] answers exactly this
/// rectangle at the platform's rung. Its top row is the one thing that may not stay typed:
/// painting the pill at 524 while pressing from 528 to 590 is the hazard this module
/// shipped with.
const CANCEL_RECT: Rect = Rect::new(200, 524, 320, 54);

/// The Cancel pill's whole box at a rung: the authored width and corner radius, the row
/// the second plate closes on, and a height that holds the grown label box plus the air the
/// authored pill gave it.
///
/// WHY its top is the plate's bottom and not a gap under it: the gap is what made the
/// hazard. The pill was painted from a constant row and pressed from the row the block above
/// it closed on plus four rows of air, so the pill's own first painted rows belonged to the
/// row above -- whose lane crosses both centres, which is why those rows answered Restart
/// (x 80..340) or Emergency (x 360..640) rather than the button inked on them. One
/// expression now answers both callers, and there is no air left to be wrong about: the
/// block's box and the pill's box are adjacent, which is what the sweep checks.
const fn cancel_rect_at(level: u8) -> Rect {
    Rect::new(
        CANCEL_RECT.x,
        block_bottom_at(1, level),
        CANCEL_RECT.w,
        scale::plate_at(CANCEL_RECT.h, NAME_SCALE, level),
    )
}

/// The pill the painter clips and the press test measures, at the rung this process draws
/// at.
///
/// WHY a function over the `const fn`: both callers take the rung as an argument, so a
/// frame resolves it once and cannot hand the two of them different answers.
fn cancel_pill(level: u8) -> Rect {
    cancel_rect_at(level)
}

const CANCEL_RADIUS: usize = 27;

/// Columns the sheet keeps between its running text and the edge of the box it is drawn on,
/// for the header lines and the pill's label.
///
/// WHY one number for both: they are the sheet's own text inset, and a second inset here
/// would be a second answer to how far from an edge the sheet sets its words.
const CANCEL_LABEL_PAD: usize = 12;

/// The row the "Cancel" label's box starts on: its own line box centred in the pill.
///
/// WHY derived and not the `533` this carried: 533 was remembered from when the rung
/// reserved a box shorter than the pill. The label is set at the same rung a button's name
/// is, so one face serves both, and [`scale::plate_at`] guarantees the box plus its
/// authored air closes inside the pill at every rung the sweep covers.
const fn cancel_label_y_at(level: u8) -> usize {
    let pill = cancel_rect_at(level);
    pill.y + (pill.h - scale::line_at(NAME_SCALE, level)) / 2
}

const COLOR_CANCEL_FILL: u32 = 0xFF1E2430;
const COLOR_CANCEL_BORDER: u32 = 0xFF333E50;
const COLOR_CANCEL_TEXT: u32 = 0xFFE2E8F0;

/// The sheet's own height at a rung: the authored box, or the stack it now holds.
///
/// WHY the later of the two: a modal that only ever grows keeps the authored pixels at
/// Normal and refuses to crop a pill a raised rung pushed below it.
const fn sheet_rect_at(level: u8) -> Rect {
    let pill = cancel_rect_at(level);
    Rect::new(
        SHEET_RECT.x,
        SHEET_RECT.y,
        SHEET_RECT.w,
        later(
            SHEET_RECT.h,
            pill.y + pill.h + SHEET_BOTTOM_AIR - SHEET_RECT.y,
        ),
    )
}

// -----------------------------------------------------------------------------
// Truth gates
// -----------------------------------------------------------------------------

/// The two rungs still stand in the order the buttons read in (a name above a subject),
/// and each box holds its rung's own caps.
///
/// WHY these no longer read `line_height(3) == 44` and `line_height(2) == 32`: those were
/// the previous ladder's boxes, a second copy of the ladder that failed the day it moved
/// while saying nothing about this layout. What the modal depends on is the ordering, the
/// containment and the tiling below.
const _: () = assert!(
    scale::line_at(NAME_SCALE, scale::PLATFORM_LEVEL)
        > scale::line_at(SUBJECT_SCALE, scale::PLATFORM_LEVEL),
    "a button's name box must reserve more rows than its subject box under it"
);
const _: () = assert!(
    scale::line_at(SUBJECT_SCALE, scale::PLATFORM_LEVEL)
        >= scale::cap_at(SUBJECT_SCALE, scale::PLATFORM_LEVEL) as usize,
    "a subject line box must at least hold its rung's cap height"
);
/// WHY: the header's description and a button's subject are the same rung, so the one box
/// the gates above measure is theirs in common. If either is re-tuned alone, the band
/// derived here stops matching the run it covers.
const _: () = assert!(
    DESC_SCALE == SUBJECT_SCALE,
    "the sheet's instruction line and a button's subject line share one rung, and so one box"
);
/// The two lanes tile the sheet: they meet on the column between the centres, and neither
/// crosses the sheet it is painted on.
const _: () = assert!(
    COL0_CX + COL_LANE_HALF == COL1_CX - COL_LANE_HALF,
    "the two action lanes must meet, leaving no dead column and no shared one"
);
const _: () = assert!(
    COL0_CX - COL_LANE_HALF >= SHEET_RECT.x
        && COL1_CX + COL_LANE_HALF <= SHEET_RECT.x + SHEET_RECT.w,
    "an action's lane must stay inside the sheet that paints it"
);
/// WHY the lane has a floor at all: a lane no wider than its two insets would make
/// [`GRID_LANE_PAD`] a truncation budget rather than an inset, and every button's name
/// would arrive elided. What this gate cannot check is the other half of the pad -- that
/// the widest name the ladder offers still fits the lane -- because the advance faces are
/// not `const` calls; that half is stated on [`GRID_LANE_PAD`] and paid for by the trim in
/// `line_pixel`, which is where a name that outgrew the lane would show.
const _: () = assert!(
    COL_LANE > 2 * GRID_LANE_PAD,
    "an action's lane must leave it columns to centre in"
);

/// The stack the modal draws, held inside its own container, at every rung a reader can be
/// handed.
///
/// WHY swept instead of spot-checked: the hazard this module shipped with was a band derived
/// from one control and a box painted from another, and the drift only showed at the rungs
/// nobody measured. Each rung the ladder offers is therefore measured for the containment
/// that can actually be lost: the header closes above the first circle, the label box fits
/// the pill, the pill sits inside the sheet, the sheet still fits the panel, and no plate
/// reaches past the pill it is stacked over.
///
/// WHY the tiling arms are here too: `block_bottom_at(0) == row_boundary_at`,
/// `plate_top_at(1) == row_boundary_at` and `pill.y == block_bottom_at(1)` hold by
/// definition today, and that is the point of writing them down -- an edit that re-typed
/// one of them into a constant of its own, which is exactly how the shipped hazard was
/// made, fails here rather than in a reader's thumb.
const fn stack_holds_at_every_level() -> bool {
    let mut level = 0usize;
    while level <= scale::MAX_LEVEL as usize {
        let l = level as u8;
        if header_bottom_at(l) > row0_cy_at(l) - BTN_RADIUS as usize {
            return false;
        }
        if block_bottom_at(0, l) != row_boundary_at(l) {
            return false;
        }
        if plate_top_at(1, l) != row_boundary_at(l) {
            return false;
        }
        let pill = cancel_rect_at(l);
        if pill.y != block_bottom_at(1, l) {
            return false;
        }
        let label_box = scale::line_at(NAME_SCALE, l);
        if label_box > pill.h {
            return false;
        }
        let label_top = cancel_label_y_at(l);
        if label_top < pill.y || label_top + label_box > pill.y + pill.h {
            return false;
        }
        let sheet = sheet_rect_at(l);
        if pill.y + pill.h + SHEET_BOTTOM_AIR > sheet.y + sheet.h {
            return false;
        }
        if pill.x < sheet.x || pill.x + pill.w > sheet.x + sheet.w {
            return false;
        }
        if sheet.y + sheet.h > SCREEN_HEIGHT {
            return false;
        }
        let plate = action_plate_at(COL1_CX, 1, l);
        if plate.y + plate.h > pill.y {
            return false;
        }
        level += 1;
    }
    true
}
const _: () = assert!(
    stack_holds_at_every_level(),
    "the grown sheet's stack must still close inside its own container at the top rung"
);

/// Identity: at the platform's rung every derived number is the row and the box this modal
/// was drawn with, so a reader who never raised "Text size" gets the same sheet.
const _: () = assert!(
    desc_y0_at(scale::PLATFORM_LEVEL) == TITLE_Y0 + launcher::font::line_height(TITLE_SCALE)
);
const _: () = assert!(
    header_bottom_at(scale::PLATFORM_LEVEL)
        == desc_y0_at(scale::PLATFORM_LEVEL) + launcher::font::line_height(DESC_SCALE)
);
const _: () = assert!(row0_cy_at(scale::PLATFORM_LEVEL) == ROW0_CY);
const _: () = assert!(
    row_pitch_at(scale::PLATFORM_LEVEL)
        == 2 * BTN_RADIUS as usize
            + launcher::font::line_height(NAME_SCALE)
            + launcher::font::line_height(SUBJECT_SCALE)
);
const _: () = assert!(
    row1_cy_at(scale::PLATFORM_LEVEL) == ROW0_CY + row_pitch_at(scale::PLATFORM_LEVEL)
);
const _: () = assert!(name_y0_at(0, scale::PLATFORM_LEVEL) == ROW0_CY + BTN_RADIUS as usize);
const _: () = assert!(
    subject_y0_at(0, scale::PLATFORM_LEVEL)
        == ROW0_CY + BTN_RADIUS as usize + launcher::font::line_height(NAME_SCALE)
);
const _: () = assert!(cancel_rect_at(scale::PLATFORM_LEVEL).x == CANCEL_RECT.x);
const _: () = assert!(cancel_rect_at(scale::PLATFORM_LEVEL).w == CANCEL_RECT.w);
const _: () = assert!(cancel_rect_at(scale::PLATFORM_LEVEL).h == CANCEL_RECT.h);
/// The pill's top row is *built* from `block_bottom_at(1, level)` rather than written
/// beside it, so the painter and the press test are never handed two rows: there is no
/// second number in this module for them to disagree about, and no air added to the row
/// before it is tested. Recorded as a gate so the derivation cannot be re-typed into a
/// constant of its own; the assert under it, against the authored `CANCEL_RECT.y`, is the
/// one that can actually fail.
const _: () = assert!(
    cancel_rect_at(scale::PLATFORM_LEVEL).y == block_bottom_at(1, scale::PLATFORM_LEVEL)
);
const _: () = assert!(cancel_rect_at(scale::PLATFORM_LEVEL).y == CANCEL_RECT.y);
const _: () = assert!(
    cancel_label_y_at(scale::PLATFORM_LEVEL)
        == CANCEL_RECT.y + (CANCEL_RECT.h - launcher::font::line_height(NAME_SCALE)) / 2
);
const _: () = assert!(sheet_rect_at(scale::PLATFORM_LEVEL).h == SHEET_RECT.h);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PowerMenuAction {
    PowerOff,
    LockScreen,
    Restart,
    Emergency,
    Cancel,
    Dismiss,
}

/// One action of the grid: what it answers with, where its lane is centred, what it reads,
/// and what draws it.
///
/// WHY a table instead of four copies of the same block: the press test and the painter need
/// the same action, and while each held its own copy of the geometry the two could -- and
/// did -- disagree about which rows belong to which control. There is now one description of
/// an action and both callers walk it in the same order, so row index `i / 2` means the same
/// row to both.
struct Action {
    which: PowerMenuAction,
    cx: usize,
    name: &'static [u8],
    subject: &'static [u8],
    fill: u32,
    ring: u32,
    subject_ink: u32,
    icon: fn(x: usize, y: usize, cx: usize, cy: usize, tint: u32) -> Option<u32>,
}

/// Row-major: indices 0 and 1 are the first row, 2 and 3 the second.
const ACTIONS: [Action; 4] = [
    Action {
        which: PowerMenuAction::PowerOff,
        cx: COL0_CX,
        name: b"Power off",
        subject: b"Shut down",
        fill: launcher::theme::SHELL_DARK.emergency,
        ring: 0xFFEF4444,
        subject_ink: 0xFFCBD5E1,
        icon: draw_power_icon,
    },
    Action {
        which: PowerMenuAction::LockScreen,
        cx: COL1_CX,
        name: b"Lock screen",
        subject: b"Secure device",
        fill: 0xFF334155,
        ring: 0xFF475569,
        subject_ink: 0xFFCBD5E1,
        icon: draw_lock_icon,
    },
    Action {
        which: PowerMenuAction::Restart,
        cx: COL0_CX,
        name: b"Restart",
        subject: b"Reboot OS",
        fill: 0xFF0369A1,
        ring: 0xFF0EA5E9,
        subject_ink: 0xFFCBD5E1,
        icon: draw_restart_icon,
    },
    Action {
        which: PowerMenuAction::Emergency,
        cx: COL1_CX,
        name: b"Emergency",
        subject: b"SOS / Dialer",
        fill: 0xFF991B1B,
        ring: 0xFFB91C1C,
        subject_ink: 0xFFFCA5A5,
        icon: draw_emergency_icon,
    },
];

/// The action a press on (`x`, `y`) asks for, measured against the boxes the painter draws
/// at the rung this process draws at.
///
/// WHY the pill is tested first: the pill's top row IS the row the plate above it closes
/// on, and `Rect::contains` is half-open, so the two boxes are disjoint today and neither
/// order can steal the other's row. The smallest target is still measured first because
/// that meeting row is what this modal shipped with wrong: the day a plate reached past its
/// own block, testing the plates first would answer a press on the pill's edge with the
/// control drawn above it.
///
/// WHY there is no band arithmetic here any more: every arm asks a rectangle the painter
/// builds too, from the same `const fn` of the same rung the frame resolved. A literal row
/// or column would be a second answer to where a control is, and this module has already
/// shown what happens to two answers.
pub fn power_menu_hit(x: usize, y: usize) -> PowerMenuAction {
    let level = scale::draw_level();

    if round_contains(&cancel_pill(level), CANCEL_RADIUS, x, y) {
        return PowerMenuAction::Cancel;
    }

    for (index, action) in ACTIONS.iter().enumerate() {
        if action_plate(action.cx, index / 2, level).contains(x, y) {
            return action.which;
        }
    }

    PowerMenuAction::Dismiss
}

/// The colour of (`x`, `y`) when the line centred in `lane`, set `pad` columns inside it on
/// a box starting at `y0` at `rung`, inks it -- trimmed to the lane the press is measured
/// against, so an ellipsis is the widest thing that can ever cross it.
///
/// WHY the trim and not the authored width: the lane is what a press answers, so a name that
/// grew past it would either be inked outside its own button or sit in a box that belongs to
/// the button beside it. [`scale::ellipsize_at`] spends the advances of the very rung the ink
/// is rasterised at, and [`scale::center_x_painted_at`] centres the width that same rung
/// reserves.
fn line_pixel(
    x: usize,
    y: usize,
    lane: Rect,
    y0: usize,
    rung: usize,
    bytes: &[u8],
    ink: u32,
    bg: u32,
    pad: usize,
    level: u8,
) -> Option<u32> {
    let line_h = scale::line_at(rung, level);
    if y < y0 || y >= y0 + line_h {
        return None;
    }
    if x < lane.x || x >= lane.x + lane.w {
        return None;
    }
    let available = lane.w.saturating_sub(2 * pad);
    let (buf, len) = scale::ellipsize_at(bytes, rung, available, level);
    let run = &buf[..len];
    if run.is_empty() {
        return None;
    }
    let x0 = lane.x + pad + scale::center_x_painted_at(run, rung, available, false, level);
    test_text_aa_level(x, y, x0, y0, rung, run, ink, bg, level)
}

/// [`line_pixel`] anchored at the lane's left inset instead of centred in it, for the run the
/// platform positions by its own left edge -- the status clock.
fn start_pixel(
    x: usize,
    y: usize,
    lane: Rect,
    y0: usize,
    rung: usize,
    bytes: &[u8],
    ink: u32,
    bg: u32,
    pad: usize,
    level: u8,
) -> Option<u32> {
    let line_h = scale::line_at(rung, level);
    if y < y0 || y >= y0 + line_h {
        return None;
    }
    if x < lane.x || x >= lane.x + lane.w {
        return None;
    }
    let available = lane.w.saturating_sub(pad);
    let (buf, len) = scale::ellipsize_at(bytes, rung, available, level);
    let run = &buf[..len];
    if run.is_empty() {
        return None;
    }
    test_text_aa_level(x, y, lane.x + pad, y0, rung, run, ink, bg, level)
}

#[inline]
fn draw_lock_icon(x: usize, y: usize, cx: usize, cy: usize, tint: u32) -> Option<u32> {
    let dx = (x as i32) - (cx as i32);
    let dy = (y as i32) - (cy as i32);

    // Shackle at top: dy in -16..-3, dx in -8..8
    if dy >= -16 && dy <= -3 && dx >= -8 && dx <= 8 {
        let is_shackle = if dy < -9 {
            let ady = dy + 9;
            let d2 = dx * dx + ady * ady;
            d2 >= 36 && d2 <= 64
        } else {
            dx.abs() >= 6 && dx.abs() <= 8
        };
        if is_shackle {
            return Some(tint);
        }
    }

    // Body at bottom: dy in -3..14, dx in -12..12
    if dy >= -3 && dy <= 14 && dx >= -12 && dx <= 12 {
        let is_border = dx.abs() >= 10 || dy <= -1 || dy >= 12;
        let is_keyhole = (dx.abs() <= 2 && dy >= 3 && dy <= 8) || (dx.abs() <= 1 && dy >= 8 && dy <= 10);
        if is_border || is_keyhole {
            return Some(tint);
        }
    }

    None
}

#[inline]
fn draw_restart_icon(x: usize, y: usize, cx: usize, cy: usize, tint: u32) -> Option<u32> {
    let dx = (x as i32) - (cx as i32);
    let dy = (y as i32) - (cy as i32);
    let d2 = dx * dx + dy * dy;

    // Circular ring arc, gap at top-right
    if d2 >= 121 && d2 <= 225 {
        if !(dx > 2 && dy < -4) {
            return Some(tint);
        }
    }

    // Arrowhead at top-right pointing clockwise
    if dx >= 3 && dx <= 13 && dy >= -16 && dy <= -6 {
        if (dx - dy >= 15 && dx <= 12) || (dx >= 7 && dy >= -10) {
            return Some(tint);
        }
    }

    None
}

#[inline]
fn draw_power_icon(x: usize, y: usize, cx: usize, cy: usize, tint: u32) -> Option<u32> {
    let dx = (x as i32) - (cx as i32);
    let dy = (y as i32) - (cy as i32);
    let d2 = dx * dx + dy * dy;

    // Circle ring with top gap
    if d2 >= 121 && d2 <= 225 {
        if !(dx.abs() <= 4 && dy < 0) {
            return Some(tint);
        }
    }

    // Vertical line in top gap
    if dx.abs() <= 2 && dy >= -16 && dy <= -2 {
        return Some(tint);
    }

    None
}

#[inline]
fn draw_emergency_icon(x: usize, y: usize, cx: usize, cy: usize, tint: u32) -> Option<u32> {
    let dx = (x as i32) - (cx as i32);
    let dy = (y as i32) - (cy as i32);
    let d2 = dx * dx + dy * dy;

    // Central hub
    if d2 <= 25 {
        return Some(tint);
    }

    // 8-spoke medical / alert star
    let is_v = dx.abs() <= 3 && dy.abs() <= 15;
    let is_h = dy.abs() <= 3 && dx.abs() <= 15;
    let is_d1 = (dx - dy).abs() <= 2 && dx.abs() <= 11 && dy.abs() <= 11;
    let is_d2 = (dx + dy).abs() <= 2 && dx.abs() <= 11 && dy.abs() <= 11;

    if is_v || is_h || is_d1 || is_d2 {
        return Some(tint);
    }

    None
}

pub fn get_power_menu_color(x: usize, y: usize, state: &LauncherState) -> u32 {
    // WHY once here and passed down rather than re-read by every face below: the rung is a
    // published cell, and a screen that read it again mid-frame could paint a plate at one
    // rung and its ink at another. Every box, baseline, lane and trim in this function is
    // solved from this one answer.
    let level = scale::draw_level();

    // 1. Top status bar (y < 48).
    //
    // WHY this screen reuses the shell's one status strip instead of painting its
    // own: the sheet covers the surface with an opaque backdrop, so it has to draw
    // a status bar, and a second copy is a second set of geometry and a second
    // set of readings. `status` owns the one clock position and the one unbound
    // telemetry glyph set.
    //
    // WHY the clock's rung is clamped to the band: the 48 status rows ARE the clock's
    // container, and at its rung its box already closes one row inside them. A reader who
    // raised "Text size" therefore gets the clock at the largest rung the band carries,
    // which today is the authored one: the ink does not grow here, and the reason it cannot
    // is the band's height, not this screen. The clamp only ever walks *down* from the rung
    // it is handed, so a compact reader still gets the smaller clock they asked for.
    if y < STATUS_BAR_HEIGHT {
        let clock_level = status::clock_rung_at(level);
        if state.clock_source_ok {
            if let Some(c) = start_pixel(
                x,
                y,
                Rect::new(0, 0, SCREEN_WIDTH, 0),
                status::CLOCK_Y,
                status::CLOCK_SCALE,
                &state.clock_time,
                0xFFCBD5E1,
                COLOR_BG_TOP,
                status::CLOCK_X,
                clock_level,
            ) {
                return c;
            }
        }
        if let Some(c) = status::marks(x, y, 0xFFCBD5E1) {
            return c;
        }
        return COLOR_BG_TOP;
    }

    // 2. The home mark, painted by the platform's own raster.
    //
    // WHY this replaced a filled box: the sheet used to ink these rows with
    // `y >= 1410 && y <= 1416 && x >= 260 && x <= 460`, which filled 201 columns
    // by SEVEN rows with square ends -- wider than the panel's own 200-column
    // mark, starting eight rows above it, and one more rectangle the shell kept
    // for a gesture the rest of the device shows one mark for. `home_band::pixel`
    // is the shape every
    // application copies, so the sheet now shows the mark the rest of the device
    // shows, and nothing else in these rows.
    //
    // WHAT DID NOT MOVE: what a tap here does. The sheet still dismisses on any
    // tap its own controls did not take (`power_menu_hit`'s `Dismiss` arm); only
    // the ink changed.
    if let Some(ink) = interos_std::design::home_band::pixel(x, y, 0x55FFFFFF) {
        return ink;
    }

    // 3. Main Modal Sheet Drawer.
    let sheet = sheet_rect_at(level);
    if round_contains(&sheet, SHEET_RADIUS, x, y) {
        let inner = sheet.inset(1);
        if !round_contains(&inner, SHEET_RADIUS.saturating_sub(1), x, y) {
            return SHEET_BORDER;
        }

        // Top drag handle pill
        if round_contains(&HANDLE_RECT, 2, x, y) {
            return HANDLE_COLOR;
        }

        // Header: the title and its instruction, both centred in the sheet's own text lane.
        // WHY the lane is the sheet inset rather than the display: the authored instruction
        // line fits this lane whole at the platform's rung -- and at the top rung it does not
        // fit the sheet at all, so a lane centred on the same axis is what keeps a grown line
        // off the border instead of across it.
        let header_lane = Rect::new(
            sheet.x + CANCEL_LABEL_PAD,
            0,
            sheet.w - 2 * CANCEL_LABEL_PAD,
            0,
        );
        if let Some(c) = line_pixel(
            x,
            y,
            header_lane,
            TITLE_Y0,
            TITLE_SCALE,
            b"Power & Safety",
            0xFFFFFFFF,
            SHEET_FILL,
            0,
            level,
        ) {
            return c;
        }
        if let Some(c) = line_pixel(
            x,
            y,
            header_lane,
            desc_y0_at(level),
            DESC_SCALE,
            b"Select an option or tap Cancel to return",
            0xFF94A3B8,
            SHEET_FILL,
            0,
            level,
        ) {
            return c;
        }

        // The four actions: one walk of the table the press test walks too.
        for (index, action) in ACTIONS.iter().enumerate() {
            let row = index / 2;
            let plate = action_plate(action.cx, row, level);
            if !plate.contains(x, y) {
                continue;
            }
            let cy = row_cy_at(row, level);
            let dx = (x as i32) - (action.cx as i32);
            let dy = (y as i32) - (cy as i32);
            let d2 = dx * dx + dy * dy;
            if d2 <= BTN_R2 {
                if let Some(c) = (action.icon)(x, y, action.cx, cy, 0xFFFFFFFF) {
                    return c;
                }
                if d2 >= (BTN_RADIUS - 2) * (BTN_RADIUS - 2) {
                    return action.ring;
                }
                return action.fill;
            }
            if let Some(c) = line_pixel(
                x,
                y,
                plate,
                name_y0_at(row, level),
                NAME_SCALE,
                action.name,
                0xFFFFFFFF,
                SHEET_FILL,
                GRID_LANE_PAD,
                level,
            ) {
                return c;
            }
            if let Some(c) = line_pixel(
                x,
                y,
                plate,
                subject_y0_at(row, level),
                SUBJECT_SCALE,
                action.subject,
                action.subject_ink,
                SHEET_FILL,
                GRID_LANE_PAD,
                level,
            ) {
                return c;
            }
            return SHEET_FILL;
        }

        // Cancel pill: the box the press test measures first, drawn here second.
        let pill = cancel_pill(level);
        if round_contains(&pill, CANCEL_RADIUS, x, y) {
            let c_inner = pill.inset(1);
            if !round_contains(&c_inner, CANCEL_RADIUS.saturating_sub(1), x, y) {
                return COLOR_CANCEL_BORDER;
            }
            if let Some(c) = line_pixel(
                x,
                y,
                pill,
                cancel_label_y_at(level),
                NAME_SCALE,
                b"Cancel",
                COLOR_CANCEL_TEXT,
                COLOR_CANCEL_FILL,
                CANCEL_LABEL_PAD,
                level,
            ) {
                return c;
            }
            return COLOR_CANCEL_FILL;
        }

        return SHEET_FILL;
    }

    // Smooth subtle vertical backdrop gradient outside the sheet
    let t = (y.saturating_sub(STATUS_BAR_HEIGHT) * 255) / (SCREEN_HEIGHT - STATUS_BAR_HEIGHT);
    let r1 = (COLOR_BG_TOP >> 16) & 0xFF;
    let g1 = (COLOR_BG_TOP >> 8) & 0xFF;
    let b1 = COLOR_BG_TOP & 0xFF;
    let r2 = (COLOR_BG_BOT >> 16) & 0xFF;
    let g2 = (COLOR_BG_BOT >> 8) & 0xFF;
    let b2 = COLOR_BG_BOT & 0xFF;
    // WHY i64 and a clamp: the delta is signed, and the old i32-to-u32 cast of a
    // negative quotient panicked on add in debug builds when the bottom stop was
    // darker than the top. Interpolating in i64 and clamping keeps every channel
    // inside 0..=255 for either stop order.
    let lerp = |a: u32, b: u32| -> u32 {
        let v = a as i64 + ((b as i64 - a as i64) * t as i64 / 255);
        v.clamp(0, 255) as u32
    };
    let r = lerp(r1, r2);
    let g = lerp(g1, g2);
    let b = lerp(b1, b2);

    0xFF000000 | (r << 16) | (g << 8) | b
}
