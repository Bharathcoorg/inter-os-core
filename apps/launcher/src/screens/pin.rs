//! Credential entry surface for unlocking the active user.
//!
//! The lock screen never transitions directly to Home. Unlocking is an
//! authenticated kernel operation; this surface only collects a short numeric
//! PIN and passes it to the user-account syscall.

use crate::{test_text_aa, LauncherState};
use launcher::layout::Rect;
use launcher::text;
use launcher::theme::{SCREEN_WIDTH, STATUS_BAR_HEIGHT};

/// How many entry dots the pad draws: the longest PIN this surface accepts, which
/// is the `pin_buffer`'s own length in `main.rs`.
const DOT_COUNT: u8 = 8;

/// Row the entry dots sit on.
///
/// WHY named: the dots and the band the shell repaints when one fills or empties
/// are the same number, and they were a literal inside the painter and a second
/// literal inside the shell when the whole surface was repainted anyway.
const DOT_Y: usize = 300;
/// Radius of an entry dot. The painter's gate is `dx*dx + dy*dy <= R * R`.
const DOT_R: usize = 6;

const KEY_W: usize = 140;
const KEY_H: usize = 104;
const KEY_GAP: usize = 24;
const KEY_X: usize = 130;
const KEY_Y: usize = 520;
/// The corner radius the keys are drawn with -- and the one [`key_at`] and the
/// painter's own control test hit against, which is why it is one number.
const KEY_RADIUS: usize = 18;

/// The line box a run painted through `crate::test_text_aa` at `scale` reserves.
///
/// WHY `font::` and not `text::`: `test_text_aa` hands the painter the run's origin
/// and lets the face solve its own baseline, and at rung 2 and above that face is
/// the outline one, whose box is the typeface's ascent plus descent at the rung's
/// em. `text::line_height` answers the 16x24 bitmap cell instead -- 48 rows for a
/// rung-2 run that inks 36, 72 for the rung-3 prompt that inks 48 -- so centring or
/// striding on it puts the ink off the middle of the box the surface reserved for
/// it.
const fn lh(scale: usize) -> usize {
    launcher::font::line_height(scale)
}

const fn rows_max(a: usize, b: usize) -> usize {
    if a > b {
        a
    } else {
        b
    }
}

/// Row the status line under the dots starts on, and the rung it is set at.
const STATUS_Y: usize = 360;
const STATUS_SCALE: usize = 1;
/// Row the prompt starts on, and its rung.
const PROMPT_Y: usize = 150;
const PROMPT_SCALE: usize = 3;
/// The keypad figures' rung, and the two action labels' -- the same rung the
/// status line uses.
const KEY_DIGIT_SCALE: usize = 2;
const ACTION_LABEL_SCALE: usize = 1;

/// Rows a rung-1 run can ink, which is the rung every caption on this surface is
/// set at: the union of the two faces the funnel answers with. The outline box is
/// `lh(1)` = 27 rows; when the outline face declines a glyph the run falls back to
/// the bitmap cell at the nearest whole magnification its cap can reach, which is
/// one -- `text::line_height(1)` = `GLYPH_H * 1` = 24 rows. A repaint band narrower
/// than either leaves the tail of that run on screen. Which of the two is wider is
/// a property of the ladder and not of this surface -- since the rungs rose it is the
/// outline box, and at rung 2 and above it always was -- so the union is spelled out
/// here rather than `lh` alone, and the gate below names which side wins.
const CAPTION_BOX_H: usize = rows_max(lh(STATUS_SCALE), text::GLYPH_H * STATUS_SCALE);
/// The prompt's and the figures' boxes, which need no union for the reason above.
const PROMPT_BOX_H: usize = lh(PROMPT_SCALE);
const KEY_DIGIT_BOX_H: usize = lh(KEY_DIGIT_SCALE);

/// Rows from a key's top to its figure's line-box origin: the cap box centred on
/// the key, exactly the rule `text::cap_centred_top` applies for rung 2 and above,
/// solved from the ladder -- `KEY_H/2 + cap/2 - ascent`.
///
/// WHY not a line-box centring: `(KEY_H - KEY_DIGIT_BOX_H)/2` answers 34, and what it
/// centres is the *line box* -- 29 rows of ascent above the origin against only 7 of
/// descent below it, wrapped round a cap that is 20 px tall -- so the cap box's middle
/// lands below the key's middle. The cap-box rule above answers 33, a row up. The
/// keys, their radius and their hit test do not move.
const KEY_DIGIT_Y: usize = KEY_H / 2
    + launcher::font::cap_for_scale(KEY_DIGIT_SCALE) as usize / 2
    - launcher::font::ascent_for_scale(KEY_DIGIT_SCALE);

/// The two action buttons' row: where it starts, how tall it is, and each button's
/// own left edge and width. `get_pin_color` draws these and [`key_at`] taps them,
/// and until now both spelled the same four numbers out.
const ACTION_Y: usize = 1036;
const ACTION_H: usize = 90;
const UNLOCK_X: usize = 130;
const UNLOCK_W: usize = 300;
const DEL_X: usize = 490;
const DEL_W: usize = 100;
/// The action labels' row: their outline box centred on the button row. It is the row
/// the literal `1070` answered while the rung-1 box was 22 rows; the box is 27, so the
/// centre has moved two rows up and the gate at the bottom of this file re-checks that
/// the taller box still closes inside the button.
const ACTION_LABEL_Y: usize = ACTION_Y + ACTION_H / 2 - lh(ACTION_LABEL_SCALE) / 2;

/// The status clock's origin and rung, named so the band below and the run it
/// gates cannot be edited apart. Both are the numbers this surface already drew
/// with, as `48, 14, 1` inside a `y < 48` test that said neither the platform's
/// band nor this run's extent.
const STATUS_TIME_X: usize = 48;
const STATUS_TIME_Y: usize = 14;
const STATUS_TIME_SCALE: usize = 1;

/// The prompt's left edge: the surface's own gutter, the same one the status clock
/// is set on.
const PROMPT_X: usize = 48;
/// The status line's left edge, and the two action labels'.
///
/// WHY these three are remembered numbers and not `text::center_x` results: every
/// run set from them is at rung 1, and at rung 1 `text::measure` walks the bitmap
/// cell while `test_text_aa` rasterises the outline face -- "Enter your device PIN"
/// measures 202 px and inks 165, "Incorrect PIN" measures 124 and inks 103. A
/// centring done here would therefore be wrong by half the difference, and wrong in
/// the direction that depends on the string: the longer the caption, the further
/// left its ink would sit of the middle. The literal is the lesser defect until
/// `text::advance`'s scale-1 branch answers the face the run is drawn with, which
/// is a change in `text.rs` and not on this surface.
const STATUS_X: usize = 250;
const DEL_X_LABEL: usize = 524;
const UNLOCK_X_LABEL: usize = 238;

/// Truth gates: what each ladder rung answers for these boxes, and that every run
/// on this surface closes inside the room the layout gives it.
///
/// WHY the caption gate is a comparison against the two faces and not the rows each
/// one answers: the union is only the outline box while the outline box is the wider
/// face, and which of them wins is the ladder's business. Stating it as an order keeps
/// the gate honest when the rungs move again -- and it fails loudly, in the one place
/// a fallback run could be clipped, if the ladder ever drops below the bitmap cell.
const _: () = assert!(
    CAPTION_BOX_H == lh(STATUS_SCALE) && lh(STATUS_SCALE) >= text::GLYPH_H * STATUS_SCALE,
    "the caption band must hold both faces; the outline box is only the union while it is the wider",
);
/// WHY: `CAPTION_BOX_H` is measured once, for `STATUS_SCALE`, and the status clock
/// and the two action labels reserve it as though it were theirs. It is only theirs
/// while all four are the same rung.
const _: () = assert!(
    STATUS_TIME_SCALE == STATUS_SCALE && ACTION_LABEL_SCALE == STATUS_SCALE,
    "the captions on this surface share one rung, and so one box"
);
/// The prompt and the figures reserve their rung's own outline box, so these gates
/// track the ladder instead of pinning it: a rung that moves moves the box with it.
const _: () = assert!(
    PROMPT_BOX_H == launcher::font::line_height(PROMPT_SCALE)
        && KEY_DIGIT_BOX_H == launcher::font::line_height(KEY_DIGIT_SCALE),
    "the prompt and the figures reserve the ladder's own boxes for their rungs",
);
/// The figures' row is the cap box centred on the key, restated as the invariant that
/// defines it -- baseline at the key's middle plus half the cap -- rather than the row
/// number it currently happens to answer.
const _: () = assert!(
    KEY_DIGIT_Y + launcher::font::ascent_for_scale(KEY_DIGIT_SCALE)
        == KEY_H / 2 + launcher::font::cap_for_scale(KEY_DIGIT_SCALE) as usize / 2,
    "the figure's baseline is the key's middle plus half the cap, not the line box's middle",
);
const _: () = assert!(STATUS_TIME_Y + CAPTION_BOX_H <= STATUS_BAR_HEIGHT);
const _: () = assert!(PROMPT_Y + PROMPT_BOX_H <= DOT_Y - DOT_R);
const _: () = assert!(DOT_Y + DOT_R + 1 <= STATUS_Y);
const _: () = assert!(STATUS_Y + CAPTION_BOX_H <= KEY_Y);
const _: () = assert!(KEY_DIGIT_Y + KEY_DIGIT_BOX_H <= KEY_H);
const _: () = assert!(ACTION_LABEL_Y + CAPTION_BOX_H <= ACTION_Y + ACTION_H);

/// Rows the entry dots occupy.
///
/// WHY read back from here: `get_pin_color` gates a dot's ink on the same radius
/// this band is derived from, so the shell repainting `dots_band()` repaints
/// exactly the rows a digit can fill or empty.
pub fn dots_band() -> (usize, usize) {
    (DOT_Y - DOT_R, DOT_Y + DOT_R + 1)
}

/// Rows the status line under the dots occupy: the error line, the confirmation
/// hint, or the prompt for the entry.
///
/// WHY `CAPTION_BOX_H` and not `text::line_height(1)` spelled out here: this is
/// the same run the painter calls at `STATUS_Y`, and a band that is not the run's
/// own extent is a band that silently stops covering it when the rung moves.
pub fn status_band() -> (usize, usize) {
    (STATUS_Y, STATUS_Y + CAPTION_BOX_H)
}

/// Rows from the prompt down to the bottom of the status line.
///
/// WHY one band for all of it: every change this surface can make without
/// leaving it -- a digit, a backspace, a failed submit, the setup step moving
/// forward -- moves something in this extent and nothing below it. The keypad
/// rows are invariant state.
///
/// WHY it starts at `PROMPT_Y` and not a row above it: the prompt's own box is
/// `PROMPT_BOX_H` rows from `PROMPT_Y` and the gate at the foot of the constants
/// above asserts it closes before the dots begin, so the top of the run is the
/// top of what a keystroke can move.
pub fn entry_band() -> (usize, usize) {
    (PROMPT_Y, STATUS_Y + CAPTION_BOX_H)
}

    /// The colour of (`x`, `y`) when the run at (`x0`, `y0`) on `scale` inks it,
    /// drawn by the outline rasteriser when it can place the run and by the
    /// surface's own funnel when it cannot.
    ///
    /// WHY the band is what it is: the run's box is the outline face's, solved from
    /// the rung the caller names -- `baseline - ascent` to `baseline + descent`,
    /// which is exactly `lh(scale)` rows from `y0` because `ascent_for_scale` and
    /// the typeface's own metrics divide the same em the same way. Concretely 27
    /// rows at rung 1, 36 at rung 2 and 48 at rung 3, against the bitmap cell's 24,
    /// 48 and 72; a band measured on the cell clips the larger rungs into the row
    /// the run never reaches and hands the smaller ones rows they cannot ink.
    /// `CAPTION_BOX_H` above is the one place on this surface that still takes the
    /// wider of the two, and why.
    ///
    /// WHY the caller's own control test still comes first: the keys, the discs and
    /// the status bar keep the hit tests and borders they had, so the wider band can
    /// only add rows the control already covers.
    ///
    /// Why the attribute, and why nothing calls this: every run on this surface
    /// already goes through `crate::test_text_aa`, which at these three rungs
    /// reaches the same outline face with the same origin convention -- the digits
    /// are rung 2 outlines, not the bold cell the lock pad's figures are, and the
    /// captions are rung 1 outlines, not cells at their native size. Routing them
    /// through this funnel would only stand a second band between the run and the
    /// rows the shell repaints. It is kept because it is the shape a caller here
    /// needs the day a run has to be placed once for a whole band and refused to
    /// the cell glyph by glyph, and the doc comment records what it does.
    #[allow(dead_code)]
    fn text_pixel(
    x: usize,
    y: usize,
    x0: usize,
    y0: usize,
    scale: usize,
    bytes: &[u8],
    ink: u32,
    bg: u32,
) -> Option<u32> {
    let em = launcher::outline_text::em_for_scale(scale);
    let (asc, desc) = interos_std::design::typeface::line_metrics(em);
    let baseline = (y0 + launcher::font::ascent_for_scale(scale)).max(asc as usize);
    let top = baseline.saturating_sub(asc as usize);
    if x < x0 || y < top || y >= baseline + desc as usize {
        return None;
    }
    // Choose the renderer ONCE for the run, not once per pixel. The placement is
    // a pure function of the run's origin, em and bytes, so every pixel of this
    // run reaches the same face. A per-pixel fallback would draw the glyph from
    // both faces at once -- an outline glyph under a hard-edged bitmap ghost.
    if scale >= 1 && bytes.len() <= launcher::outline_text::KEY_MAX {
        if let Some(run) = launcher::outline_text::OutlineRun::place(x0, baseline, em, bytes) {
            return run.pixel(x, y, ink, bg);
        }
    }
    // The bitmap face is the refusal path, not the baseline: a run the rasteriser
    // declines still has to draw as text.
    test_text_aa(x, y, x0, y0, scale, bytes, ink, bg)
}

#[inline]
fn round_contains(rect: &Rect, radius: usize, x: usize, y: usize) -> bool {
    if !rect.contains(x, y) { return false; }
    let rx = radius.min(rect.w / 2).min(rect.h / 2);
    let left = rect.x + rx;
    let right = rect.x + rect.w.saturating_sub(rx);
    let top = rect.y + rx;
    let bottom = rect.y + rect.h.saturating_sub(rx);
    if x >= left && x < right || y >= top && y < bottom { return true; }
    let cx = if x < left { left } else { right.saturating_sub(1) };
    let cy = if y < top { top } else { bottom.saturating_sub(1) };
    let dx = x as i32 - cx as i32;
    let dy = y as i32 - cy as i32;
    dx * dx + dy * dy <= (rx as i32) * (rx as i32)
}

#[inline]
fn key_rect(col: usize, row: usize) -> Rect {
    Rect::new(
        KEY_X + col * (KEY_W + KEY_GAP),
        KEY_Y + row * (KEY_H + KEY_GAP),
        KEY_W,
        KEY_H,
    )
}

#[inline]
fn key_digit(col: usize, row: usize) -> u8 {
    match (row, col) {
        (0, 0) => b'1', (0, 1) => b'2', (0, 2) => b'3',
        (1, 0) => b'4', (1, 1) => b'5', (1, 2) => b'6',
        (2, 0) => b'7', (2, 1) => b'8', (2, 2) => b'9',
        _ => b'0',
    }
}

pub fn get_pin_color(x: usize, y: usize, state: &LauncherState) -> u32 {
    let theme = launcher::theme::SHELL_DARK;
    if y < STATUS_BAR_HEIGHT {
        if state.clock_source_ok
            && y >= STATUS_TIME_Y
            && y < STATUS_TIME_Y + CAPTION_BOX_H
        {
            if let Some(c) = test_text_aa(
                x,
                y,
                STATUS_TIME_X,
                STATUS_TIME_Y,
                STATUS_TIME_SCALE,
                &state.clock_time,
                theme.on_surface,
                theme.surface,
            ) {
                return c;
            }
        }
        return theme.surface;
    }

    // WHY the step is shown: first boot asks for the PIN twice, and a second
    // screen captioned "Set Device PIN" reads as a reset of the first rather
    // than a confirmation of it.
    let heading = if state.pin_setup_mode {
        if state.pin_setup_step == 0 { b"Set Device PIN".as_slice() } else { b"Confirm Device PIN".as_slice() }
    } else {
        b"Enter PIN".as_slice()
    };
    if let Some(c) = test_text_aa(x, y, PROMPT_X, PROMPT_Y, PROMPT_SCALE, heading, theme.on_surface, theme.surface) {
        return c;
    }

    let dot_y = DOT_Y;
    // WHY derived and not a literal: `286` was a typed start, and it put the row's
    // centre (between the fourth and fifth dot) at 363 rather than on the surface's
    // own middle, 360 -- the eight dots were drawn three pixels right of centre on
    // every entry attempt. The pitch and the count are both still literals because
    // they are this keypad's own spacing, but the start is now solved for the
    // centre, so a change to either only moves the row, never off-centres it.
    let dot_pitch = 22usize;
    let dot_start = SCREEN_WIDTH / 2 - (DOT_COUNT as usize - 1) * dot_pitch / 2;
    for i in 0..DOT_COUNT as usize {
        let cx = dot_start + i * dot_pitch;
        let active = i < state.pin_len as usize;
        let dx = x as i32 - cx as i32;
        let dy = y as i32 - dot_y as i32;
        if dx * dx + dy * dy <= (DOT_R * DOT_R) as i32 {
            return if active { theme.primary } else { theme.border };
        }
    }

    if state.pin_error != 0 {
        let msg = if state.pin_error == 2 { b"No credential configured".as_slice() } else { b"Incorrect PIN".as_slice() };
        if let Some(c) = test_text_aa(x, y, STATUS_X, STATUS_Y, STATUS_SCALE, msg, theme.error, theme.surface) {
            return c;
        }
    } else if state.pin_setup_mode && state.pin_setup_step == 1 {
        if let Some(c) = test_text_aa(x, y, STATUS_X, STATUS_Y, STATUS_SCALE, b"Re-enter the same PIN", theme.on_surface_variant, theme.surface) {
            return c;
        }
    } else if let Some(c) = test_text_aa(x, y, STATUS_X, STATUS_Y, STATUS_SCALE, if state.pin_setup_mode { b"Choose 4 to 8 digits".as_slice() } else { b"Enter your device PIN".as_slice() }, theme.on_surface_variant, theme.surface) {
        return c;
    }

    // A figure's width is `text::measure`'s, because at rung 2 that is the advance
    // the outline face draws with: 12 px, tabular, so one call serves all twelve
    // keys. The `61` this replaces was remembered, and it left every figure three
    // pixels left of the middle of its key. Asked for inside the control test, so
    // a pixel that is not on a key never pays for it.
    for row in 0..3 {
        for col in 0..3 {
            let rect = key_rect(col, row);
            if round_contains(&rect, KEY_RADIUS, x, y) {
                let digit_x = (KEY_W - text::measure(b"0", KEY_DIGIT_SCALE)) / 2;
                if let Some(c) = test_text_aa(
                    x, y, rect.x + digit_x, rect.y + KEY_DIGIT_Y, KEY_DIGIT_SCALE,
                    &[key_digit(col, row)], theme.on_surface, theme.surface_container
                ) { return c; }
                if x == rect.x || y == rect.y || x + 1 == rect.x + rect.w || y + 1 == rect.y + rect.h {
                    return theme.border;
                }
                return theme.surface_container;
            }
        }
    }

    let zero = key_rect(1, 3);
    if round_contains(&zero, KEY_RADIUS, x, y) {
        let digit_x = (KEY_W - text::measure(b"0", KEY_DIGIT_SCALE)) / 2;
        if let Some(c) = test_text_aa(x, y, zero.x + digit_x, zero.y + KEY_DIGIT_Y, KEY_DIGIT_SCALE, b"0", theme.on_surface, theme.surface_container) { return c; }
        return theme.surface_container;
    }

    let back = Rect::new(DEL_X, ACTION_Y, DEL_W, ACTION_H);
    if round_contains(&back, KEY_RADIUS, x, y) {
        if let Some(c) = test_text_aa(x, y, DEL_X_LABEL, ACTION_LABEL_Y, ACTION_LABEL_SCALE, b"DEL", theme.on_surface_variant, theme.surface_container) { return c; }
        return theme.surface_container;
    }

    let enter = Rect::new(UNLOCK_X, ACTION_Y, UNLOCK_W, ACTION_H);
    if round_contains(&enter, KEY_RADIUS, x, y) {
        if let Some(c) = test_text_aa(x, y, UNLOCK_X_LABEL, ACTION_LABEL_Y, ACTION_LABEL_SCALE, b"UNLOCK", theme.on_surface, theme.primary) { return c; }
        return theme.primary;
    }

    theme.surface
}

/// The key a point on the PIN surface presses, or `None` outside every key.
///
/// WHY this lives beside [`key_rect`]: the hit test and the drawing must agree
/// about where a key is, and they used to be two separate geometries - the
/// drawer's own pad table placed its rows at 700/840/980/1120 while this surface
/// draws them at 572/700/828/956. The top row therefore did not hit-test at all
/// and the bottom row answered as the row above it, which is why entering a PIN
/// through the surface could register the wrong digits. Resolving a point
/// against `key_rect` means there is exactly one pad geometry to change.
///
/// `'E'` is the UNLOCK action and `'D'` the DEL action, matching the two buttons
/// this surface draws beneath the keys.
#[allow(dead_code)]
pub fn key_at(x: usize, y: usize) -> Option<char> {
    for row in 0..3 {
        for col in 0..3 {
            let rect = key_rect(col, row);
            if round_contains(&rect, KEY_RADIUS, x, y) {
                return Some(key_digit(col, row) as char);
            }
        }
    }
    let zero = key_rect(1, 3);
    if round_contains(&zero, KEY_RADIUS, x, y) {
        return Some('0');
    }
    let back = Rect::new(DEL_X, ACTION_Y, DEL_W, ACTION_H);
    if round_contains(&back, KEY_RADIUS, x, y) {
        return Some('D');
    }
    let enter = Rect::new(UNLOCK_X, ACTION_Y, UNLOCK_W, ACTION_H);
    if round_contains(&enter, KEY_RADIUS, x, y) {
        return Some('E');
    }
    None
}
