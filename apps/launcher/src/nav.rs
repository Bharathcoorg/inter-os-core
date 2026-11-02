//! The shell's bottom navigation bar: three controls, one geometry.
//!
//! WHY this module exists: the bar's three tap bands were written in the shell
//! binary (`main.rs`'s `nav_target_at`) and its marks were painted in
//! `screens/home.rs`, from numbers each of them typed for itself. The paint and
//! the tap therefore had nothing in common but a comment saying they agreed, and
//! what they had actually drifted to is what the person on the device reported:
//! the home pill was drawn, the back and recents marks were built into the frame
//! plan and never read by a pixel function, so the left and right thirds of the
//! bottom of the screen were live to a tap and invisible to an eye. One band
//! that answers three things and shows one is not lag -- it is a control nobody
//! can find.
//!
//! Everything here is derived from [`MARK`], and the shell binary and the
//! painter both ask this module instead of restating a number. That is the whole
//! point of the split: a geometry the painter and the hit test share cannot
//! silently stop agreeing.
//!
//! WHAT IS DELIBERATELY NOT DERIVED: [`BAND_TOP`] and [`THIRD_W`] are the hit
//! test as committed. `tests/test_serial_tcp.py` and the lifecycle suites tap
//! the middle third of the bottom 60 rows, and a person's muscle memory is
//! trained on the same boxes; this task is to make the paint match the tap, not
//! to move the tap. The marks are then built to sit strictly inside the third
//! they answer, which the compile-time gates below enforce and
//! `apps/launcher/src/tests.rs` re-checks by walking the ink.

use crate::layout::Rect;
use crate::theme::{MIN_TOUCH_TARGET, SCREEN_HEIGHT, SCREEN_WIDTH};

/// The one constant every painted dimension in the bar is derived from.
///
/// WHY the platform's minimum touch target and not a number typed here: the SDK
/// names 44 px as the smallest thing a control may be (`design::touch::MIN_TARGET`),
/// and the marks on this bar were 5 px tall and 9 px wide -- under a quarter of
/// that -- which is precisely the "too small, is that a fault or a button" the
/// person on the device described. Binding the mark scale to the same token the
/// touch floor uses means a bar that grows its target grows its marks with it.
pub const MARK: usize = MIN_TOUCH_TARGET;

/// Topmost row of the bar's tap band.
///
/// WHY 1380 and not the drawn bar's top: the home surface fills the navigation
/// band as `SCREEN_HEIGHT - NAV_BAR_HEIGHT`, which is y 1376, but rows
/// 1376..1380 were excluded when the band was first cut because the universal
/// search control above it answered taps there. The committed runtime suites tap
/// from 1380, so this stays 1380 and the painter paints inside it.
pub const BAND_TOP: usize = 1380;

/// The row one past the band's last -- the bottom of the surface.
pub const BAND_BOTTOM: usize = SCREEN_HEIGHT;

/// Rows the band owns: 1380..1440 on a 1440-row panel.
pub const BAND_H: usize = BAND_BOTTOM - BAND_TOP;

/// Width of the band one control owns: the three equal thirds the shell answers.
pub const THIRD_W: usize = SCREEN_WIDTH / 3;

/// The row the marks are centred on.
pub const BAND_CENTRE_Y: usize = BAND_TOP + BAND_H / 2;

/// Height a mark is drawn at: three quarters of the band's mark unit.
///
/// WHY not the whole of `MARK`: the band is 60 rows and a mark that filled it
/// would press against the rows above and below; three quarters of the target
/// unit leaves the mark legible (33 rows, against the 5 the pill used to be)
/// without touching the band's edges.
const MARK_H: usize = MARK * 3 / 4;

/// Thickness of a stroke every mark is drawn with.
///
/// WHY a quarter of the touch unit over eleven: `MARK / 11` is 4 at the SDK's 44,
/// which is the same weight the magnifier's handle and ring already carry on this
/// surface, and it is what makes the difference between a mark and a hairline.
const STROKE: usize = MARK / 11;

/// Half the height of the back chevron.
const CHEVRON_HALF_H: usize = MARK_H / 2;
/// How far the chevron's vertex sits left of its centre, and its tips right of it.
///
/// WHY half of [`CHEVRON_HALF_H`]: the arms are drawn at 45 degrees, so an arm
/// that climbs `CHEVRON_HALF_H` rows runs the same number of columns, and the two
/// arms together span twice this width.
const CHEVRON_HALF_W: usize = CHEVRON_HALF_H / 2;
/// Width the chevron actually inks: the centre line plus the stroke on either end.
const CHEVRON_W: usize = 2 * (CHEVRON_HALF_W + STROKE - 1) + 1;
/// Height the chevron actually inks.
const CHEVRON_H: usize = 2 * CHEVRON_HALF_H + 1;

/// The recents glyph's side: the same height as the chevron, so the three marks
/// read as one set rather than as three leftovers.
const RECENTS_SIDE: usize = MARK_H;
/// Corner radius of the recents glyph.
const RECENTS_RADIUS: usize = MARK / 8;

/// Width of the home capsule -- two mark units across.
const HOME_W: usize = MARK * 2;
/// Height of the home capsule.
///
/// WHY a quarter and not the five rows it used to be: five rows on a 1,440-row
/// panel reads as a rendering fault, as the drawer's own note about its pill
/// already argued. Eleven is a shape a person can aim at.
const HOME_H: usize = MARK / 4;

/// Which of the three controls a completed tap selected.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum NavTarget {
    Back,
    Home,
    Recents,
}

impl NavTarget {
    /// Left-to-right position of this control's third.
    pub const fn index(self) -> usize {
        match self {
            NavTarget::Back => 0,
            NavTarget::Home => 1,
            NavTarget::Recents => 2,
        }
    }

    /// The column at the centre of this control's third.
    pub const fn centre_x(self) -> usize {
        self.index() * THIRD_W + THIRD_W / 2
    }

    /// The exact box this control's mark inks, in surface coordinates.
    pub const fn mark_box(self) -> Rect {
        match self {
            NavTarget::Back => centred_rect(self.centre_x(), CHEVRON_W, CHEVRON_H),
            NavTarget::Home => centred_rect(self.centre_x(), HOME_W, HOME_H),
            NavTarget::Recents => centred_rect(self.centre_x(), RECENTS_SIDE, RECENTS_SIDE),
        }
    }
}

/// A `w` x `h` box whose centre is (`cx`, [`BAND_CENTRE_Y`]).
const fn centred_rect(cx: usize, w: usize, h: usize) -> Rect {
    Rect::new(cx - w / 2, BAND_CENTRE_Y - h / 2, w, h)
}

/// The band, as one rectangle, for a painter that has to fill or clip it.
pub const fn band() -> Rect {
    Rect::new(0, BAND_TOP, SCREEN_WIDTH, BAND_H)
}

/// Which control a completed tap at (`x`, `y`) selected, or `None` when the tap
/// was nowhere near the bar.
///
/// WHY the bar is split into equal thirds: it is the committed hit test -- three
/// 240 x 60 bands at y 1380..1440 -- and the marks above are centred on those
/// thirds, so the box a person aims at is the box that answers.
#[must_use]
pub const fn target_at(x: usize, y: usize) -> Option<NavTarget> {
    if y < BAND_TOP || y >= BAND_BOTTOM || x >= SCREEN_WIDTH {
        return None;
    }
    if x < THIRD_W {
        Some(NavTarget::Back)
    } else if x < THIRD_W * 2 {
        Some(NavTarget::Home)
    } else {
        Some(NavTarget::Recents)
    }
}

/// Whether a pixel lies inside `r` with its corners rounded to `rad`.
///
/// WHY a circle and not the home surface's superellipse: this is the same test
/// the SDK's `back_button::in_chip` makes -- "inside the plate, but not inside
/// the plate one step in" -- which is what puts a ring on the boundary the shape
/// actually has, corner arcs included, instead of stopping the ring where its own
/// rounding begins.
fn in_rounded(r: Rect, x: usize, y: usize, rad: usize) -> bool {
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
    let r = rad as i64;
    dx * dx + dy * dy <= r * r
}

/// Whether (`x`, `y`) is ink on the back control's chevron.
///
/// WHY integer arithmetic rather than the `<` glyph: the frame plan used to build
/// a `Run` of the byte `<` at the caption rung, which is a 9 px mark, and no
/// painter ever read it. Two 45-degree arms measured against their own centre
/// line are the same shape at four times the size, drawn with adds and compares.
pub fn back_inks(x: usize, y: usize) -> bool {
    let dx = x as i64 - NavTarget::Back.centre_x() as i64;
    let dy = y as i64 - BAND_CENTRE_Y as i64;
    let half_h = CHEVRON_HALF_H as i64;
    let reach = CHEVRON_HALF_W as i64;
    let arm = STROKE as i64;
    if dy < -half_h || dy > half_h {
        return false;
    }
    // Both arms meet at dy == 0, where the two tests agree, so the vertex is one
    // point rather than a notch.
    if dy <= 0 {
        (dx + dy + reach).abs() < arm
    } else {
        (dx - dy + reach).abs() < arm
    }
}

/// Whether (`x`, `y`) is ink on the recents control's outline.
pub fn recents_inks(x: usize, y: usize) -> bool {
    let plate = NavTarget::Recents.mark_box();
    in_rounded(plate, x, y, RECENTS_RADIUS)
        && !in_rounded(plate.inset(STROKE), x, y, RECENTS_RADIUS.saturating_sub(STROKE))
}

/// Whether (`x`, `y`) is ink on the home control's capsule.
pub fn home_inks(x: usize, y: usize) -> bool {
    let bar = NavTarget::Home.mark_box();
    in_rounded(bar, x, y, bar.h / 2)
}

/// Which control owns this pixel's mark, or `None` when the pixel is on none.
///
/// WHY it names the control rather than answering a yes/no: the painter needs one
/// ink for a whole mark, and the way to get one is to ask the mark where it is.
pub fn mark_at(x: usize, y: usize) -> Option<NavTarget> {
    if back_inks(x, y) {
        Some(NavTarget::Back)
    } else if home_inks(x, y) {
        Some(NavTarget::Home)
    } else if recents_inks(x, y) {
        Some(NavTarget::Recents)
    } else {
        None
    }
}

// -----------------------------------------------------------------------------
// Gates: the paint must land inside the tap, and every number must come from MARK
// -----------------------------------------------------------------------------

const _: () = assert!(SCREEN_WIDTH % 3 == 0, "the bar cannot split into equal thirds");
const _: () = assert!(BAND_TOP < BAND_BOTTOM && BAND_BOTTOM == SCREEN_HEIGHT);
const _: () = assert!(
    MARK >= MIN_TOUCH_TARGET,
    "a mark bound to a target smaller than the touch floor is not a mark",
);

/// Every mark fits its own third, with room on either side.
const _: () = assert!(
    centred_rect(THIRD_W / 2, CHEVRON_W, CHEVRON_H).x + CHEVRON_W < THIRD_W,
    "the back chevron must be painted inside the third that answers it",
);
const _: () = assert!(
    centred_rect(THIRD_W + THIRD_W / 2, HOME_W, HOME_H).x + HOME_W < THIRD_W * 2,
    "the home capsule must be painted inside the third that answers it",
);
const _: () = assert!(
    centred_rect(THIRD_W * 2 + THIRD_W / 2, RECENTS_SIDE, RECENTS_SIDE).x + RECENTS_SIDE
        < SCREEN_WIDTH,
    "the recents outline must be painted inside the third that answers it",
);

/// Every mark fits the band's rows, so a painted control is never partly outside
/// the rows that answer it.
const _: () = assert!(
    BAND_CENTRE_Y - CHEVRON_H / 2 >= BAND_TOP
        && BAND_CENTRE_Y + CHEVRON_H / 2 < BAND_BOTTOM,
    "the chevron must sit inside the band's rows",
);
const _: () = assert!(
    BAND_CENTRE_Y - MARK_H / 2 >= BAND_TOP && BAND_CENTRE_Y + MARK_H / 2 < BAND_BOTTOM,
    "the recents square must sit inside the band's rows",
);
const _: () = assert!(
    STROKE >= 3,
    "a stroke under three rows is the hairline this module exists to replace",
);
