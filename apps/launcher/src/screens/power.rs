//! Power Menu: lock, restart, power off, emergency.
//!
//! WHY `pub`: `render_view` in the crate root dispatches here by view mode.
//! Geometry and colour come from `launcher::theme`, `launcher::layout` and
//! `launcher::text`, never from literals local to this file.
//!
//! WHY an action that the shell cannot perform is drawn as unavailable rather
//! than as a live button: the previous version drew Lock, Restart, Power off and
//! Emergency as four identical cards. Tapping Restart logged "System restart
//! confirmed" and returned to Home without restarting anything, and Power off
//! did the same. There is no reboot call and no power-off call anywhere in the
//! kernel's syscall surface, so those two buttons were a promise the platform
//! could not keep -- and the log line made the broken promise look like a
//! success. An action that cannot run is now drawn recessed, muted, and labelled
//! with the reason, so the control stops advertising a capability.

#![allow(unused_imports)]

use super::system_space::{darken, decimal, push, round_contains, VALUE_MAX};
use crate::{test_text, LauncherState};
use launcher::layout::{self, Rect};
use launcher::text;
use launcher::theme::*;

/// WHY `Theme::DARK`: the reference design splits its palette by context, and
/// the Power drawer is the near-black surface -- a dark page, lifted cards,
/// light ink -- with a single red reserved for the emergency action. Reserving
/// `danger` for that one control is why nothing else on this screen is red.
const THEME: Theme = Theme::DARK;

/// Why the shell cannot perform an action, or what it does instead.
///
/// WHY an enum and not a bool: a control that is unavailable still has to say
/// why, and a bool leaves nowhere to put the reason except in the caption of a
/// button that still looks live.
enum Can {
    /// Tapping it changes real state.
    Yes(&'static [u8]),
    /// Nothing in the kernel performs it.
    No(&'static [u8]),
}

impl Can {
    fn is_yes(&self) -> bool {
        matches!(self, Can::Yes(_))
    }

    fn status(&self) -> &'static [u8] {
        match self {
            Can::Yes(status) => status,
            Can::No(reason) => reason,
        }
    }
}

const ACTION_LOCK: usize = 0;
const ACTION_RESTART: usize = 1;
const ACTION_POWER_OFF: usize = 2;
const ACTION_EMERGENCY: usize = 3;
const ACTION_COUNT: usize = 4;

/// The four references the design gives this drawer. Every one of them is
/// checked against the shell's own capabilities before it is drawn as live.
///
/// WHY the id is compared against the live registry rather than assumed: the
/// emergency action launches `org.interos.dialer`, and a launch that fails does
/// nothing at all, so on an image without that package the button is not an
/// emergency control, it is a dead control.
const EMERGENCY_PACKAGE: &[u8] = b"org.interos.dialer";

/// Left edge of the drawer.
///
/// WHY the touch rectangles below are literals: the shell's handler in `main.rs`
/// hit-tests them and that file is not part of this change. A control drawn
/// anywhere other than where the input layer looks is a control whose hit area
/// disagrees with its picture, which is the same class of bug as drawing a
/// button that does nothing. Exporting these next to the handler -- the lock
/// screen already exports its button geometry that way -- is what would let them
/// be computed.
const DRAWER_X: usize = 340;
const LOCK_PILL: Rect = Rect::new(365, 190, 330, 75);
const RESTART_PILL: Rect = Rect::new(365, 285, 330, 75);
const POWER_OFF_PILL: Rect = Rect::new(365, 380, 330, 75);
const EMERGENCY_PILL: Rect = Rect::new(365, 475, 330, 75);
const PILLS: [Rect; ACTION_COUNT] =
    [LOCK_PILL, RESTART_PILL, POWER_OFF_PILL, EMERGENCY_PILL];
/// The drawer dismisses on a tap outside it, on a tap below `DISMISS_PILL`, or
/// on a swipe, so this pill is a labelled convenience rather than the only way
/// out -- which is why it is drawn as a quiet control.
const DISMISS_PILL: Rect = Rect::new(420, 580, 220, 55);

/// The four fills a pill can take. Emergency keeps the red; everything else that
/// cannot run is drawn with the theme's own recessed fill.
struct PillSkin {
    fill: u32,
    border: u32,
    label_ink: u32,
    status_ink: u32,
}

impl PillSkin {
    /// An action the shell performs.
    fn live(theme: &Theme) -> PillSkin {
        PillSkin {
            fill: theme.surface_raised,
            border: theme.border,
            label_ink: theme.text,
            status_ink: theme.text_muted,
        }
    }

    /// An action nothing in the kernel performs: flat, muted, and outlined in
    /// the theme's hairline rather than in a colour that invites a tap.
    ///
    /// WHY `surface` and not `surface_sunken` here while the light screens recess
    /// their unavailable rows: a sunken fill is darker than the page, and on a
    /// near-black page a control that is darker than its background reads as a
    /// hole rather than as a disabled button. On a light page the same idea is
    /// carried by a fill darker than the card, which is what `surface_sunken`
    /// is for.
    fn unavailable(theme: &Theme) -> PillSkin {
        PillSkin {
            fill: theme.surface,
            border: theme.border,
            label_ink: theme.text_muted,
            status_ink: theme.text_muted,
        }
    }

    /// The emergency accent, on the emergency control alone.
    fn emergency(theme: &Theme) -> PillSkin {
        // WHY the accent is darkened by hand rather than through `theme::veil`: `veil`
    // with `light = false` blends toward white, which turned this pill pale pink.
    let fill = darken(theme.danger, 85);
        PillSkin {
            fill,
            border: theme.danger,
            label_ink: layout::ink_on(theme, fill),
            status_ink: layout::ink_on(theme, fill),
        }
    }
}

/// The label and the real capability behind action `index`.
///
/// WHY these are derived per frame rather than a fixed table with flags: the
/// emergency action depends on whether the dialer is in the live registry, and a
/// table that assumed it was would draw an emergency control on an image where
/// the launch fails.
fn action(state: &LauncherState, index: usize) -> (&'static [u8], Can) {
    match index {
        ACTION_LOCK => (
            b"Lock device",
            // WHY the wording is what the shell does and not what the reference
            // calls it: the handler switches to the lock surface and the session
            // is locked, so "lock the session" is the whole truth.
            Can::Yes(b"lock the session"),
        ),
        ACTION_RESTART => (
            b"Restart",
            // WHY "no reboot call" and not "coming soon": there is no reboot
            // entry in the kernel's syscall table to ship behind a promise.
            Can::No(b"no reboot call"),
        ),
        ACTION_POWER_OFF => (
            b"Power off",
            Can::No(b"no power-off call"),
        ),
        ACTION_EMERGENCY => (
            b"Emergency",
            if catalogue_has(state, EMERGENCY_PACKAGE) {
                Can::Yes(b"open the dialer")
            } else {
                Can::No(b"no dialer package")
            },
        ),
        _ => (b"", Can::No(b"")),
    }
}

/// Whether `id` is in the live package registry.
///
/// WHY search the catalogue instead of a built-in list: the registry is read
/// from the kernel at start-up and filtered to packages whose status is active,
/// so it is the only answer to "can this be launched" that the shell actually
/// has.
fn catalogue_has(state: &LauncherState, id: &[u8]) -> bool {
    let count = state.app_count.min(state.app_ids.len());
    for i in 0..count {
        let len = state.app_id_lens[i] as usize;
        if len == id.len() && &state.app_ids[i][..len] == id {
            return true;
        }
    }
    false
}

/// How many of the actions this drawer offers the shell can perform.
fn available_actions(state: &LauncherState) -> usize {
    let mut n = 0;
    for i in 0..ACTION_COUNT {
        if action(state, i).1.is_yes() {
            n += 1;
        }
    }
    n
}

/// `<available> of <total> actions available` under the title.
fn action_summary(state: &LauncherState) -> ([u8; VALUE_MAX], usize) {
    let (mut buf, mut at) = decimal(available_actions(state));
    push(&mut buf, &mut at, b" of ");
    let (total, total_len) = decimal(ACTION_COUNT);
    push(&mut buf, &mut at, &total[..total_len]);
    push(&mut buf, &mut at, b" available");
    (buf, at)
}

/// The skin an action is drawn with.
fn action_skin(theme: &Theme, index: usize, can: &Can) -> PillSkin {
    match (index, can) {
        (ACTION_EMERGENCY, Can::Yes(_)) => PillSkin::emergency(theme),
        (_, Can::Yes(_)) => PillSkin::live(theme),
        // WHY the emergency control keeps a red hairline when it cannot run: the
        // user is looking for it in a hurry, and hiding it entirely would be
        // worse than showing it inert.
        (ACTION_EMERGENCY, Can::No(_)) => PillSkin {
            fill: theme.surface,
            border: theme.danger,
            label_ink: theme.text_muted,
            status_ink: theme.text_muted,
        },
        (_, Can::No(_)) => PillSkin::unavailable(theme),
    }
}

// -----------------------------------------------------------------------------
// Runs and the per-frame frame
// -----------------------------------------------------------------------------

/// Capacity of one run's byte buffer. WHY it matches `text::ellipsize`, so a run
/// built once per frame is byte-for-byte the run the per-pixel painter built.
const RUN_CAP: usize = 96;

/// One label, placed and measured once per frame.
///
/// WHY this type: the drawer's title measured five glyphs for every pixel of its
/// band, each pill ellipsized its label and its status and re-measured both to centre
/// them for every pixel of the pill, and `action_summary` -- which counts the live
/// actions by walking the package registry -- was rebuilt for every pixel of the line
/// under the title. Nothing about any of that depends on the pixel.
///
/// WHY the box is carried as well as the origin: `test_text` walks the pen of every
/// glyph before it can answer "no", so a pixel nowhere near a run still costs a pen
/// walk per label. The stored box is exactly
/// `y .. y + line_height(scale)` by `x .. x + measure(run)`, which is the region
/// `test_text` can return `Some` for.
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

    /// `bytes` centred across `width` from `x`, at `y`.
    ///
    /// WHY `y` is passed rather than a box: the two lines of a pill are already
    /// centred vertically against the block the old painter computed, and a vertical
    /// centre against a per-line box is the same number written once.
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

    /// `bytes` drawn whole with no truncation step -- the title and the count under
    /// it, which the old painter never offered a width to fit.
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
        if self.len == 0 {
            return None;
        }
        if px < self.x || px - self.x >= self.w || py < self.y || py - self.y >= self.h {
            return None;
        }
        test_text(px, py, self.x, self.y, self.scale, &self.bytes[..self.len], self.ink)
    }
}

/// One pill, with its skin and both of its lines resolved.
struct PillPlan {
    rect: Rect,
    inner: Rect,
    corner: usize,
    skin: PillSkin,
    label: Run,
    status: Run,
}

impl PillPlan {
    const EMPTY: PillPlan = PillPlan {
        rect: Rect::new(0, 0, 0, 0),
        inner: Rect::new(0, 0, 0, 0),
        corner: 0,
        skin: PillSkin {
            fill: 0,
            border: 0,
            label_ink: 0,
            status_ink: 0,
        },
        label: Run::EMPTY,
        status: Run::EMPTY,
    };
}

/// Everything the drawer needs that does not depend on the pixel.
struct Frame {
    clock: Option<Run>,
    title: Run,
    summary: Run,
    pills: [PillPlan; ACTION_COUNT],
    dismiss: PillPlan,
    indicator: Rect,
}

/// Resolves one pill.
///
/// WHY the two lines are laid out as one block: a label and its status read as a
/// single control when they share a baseline group, and the block is centred against
/// the pill rather than each line against the pill. A pill with no status line
/// centres its label on its own, because a label floating high in a button reads as
/// misaligned rather than as quiet.
fn pill_plan(rect: Rect, label: &[u8], status: &[u8], skin: PillSkin) -> PillPlan {
    let label_h = text::line_height(type_scale::BODY);
    let status_h = text::line_height(type_scale::CAPTION);
    let block_h = if status.is_empty() {
        label_h
    } else {
        label_h + space::XS + status_h
    };
    let top = rect.y + rect.h.saturating_sub(block_h) / 2;
    let text_w = rect.w - space::MD * 2;
    PillPlan {
        rect,
        inner: rect.inset(1),
        corner: radius::SHEET.min(rect.h / 2),
        label: Run::centred(
            rect.x + space::MD,
            top,
            type_scale::BODY,
            text_w,
            label,
            skin.label_ink,
        ),
        status: if status.is_empty() {
            Run::EMPTY
        } else {
            Run::centred(
                rect.x + space::MD,
                top + label_h + space::XS,
                type_scale::CAPTION,
                text_w,
                status,
                skin.status_ink,
            )
        },
        skin,
    }
}

/// Builds this frame's plan.
///
/// WHY the summary is built here: `available_actions` asks each action whether it can
/// run, and the emergency action answers by walking the package registry for the
/// dialer. That walk ran for every pixel of the line under the title.
fn build(state: &LauncherState) -> Frame {
    let theme = THEME;

    let bar = Rect::new(0, 0, SCREEN_WIDTH, STATUS_BAR_HEIGHT);
    let panel = Rect::new(DRAWER_X, STATUS_BAR_HEIGHT, SCREEN_WIDTH - DRAWER_X, SCREEN_HEIGHT);
    let pad = panel.inset(space::LG);

    let title_h = text::line_height(type_scale::TITLE);
    let caption_h = text::line_height(type_scale::CAPTION);
    let title_y = pad.y + space::MD;
    let sub_y = title_y + title_h + space::XS;

    let (summary, summary_len) = action_summary(state);

    let mut pills = [PillPlan::EMPTY; ACTION_COUNT];
    for index in 0..ACTION_COUNT {
        let (label, can) = action(state, index);
        let skin = action_skin(&theme, index, &can);
        pills[index] = pill_plan(PILLS[index], label, can.status(), skin);
    }

    let indicator_w = space::XL * 4;
    let indicator_h = space::XS;

    Frame {
        clock: if state.clock_source_ok {
            Some(Run::whole(
                bar.x + CONTENT_PADDING,
                bar.y + (bar.h - caption_h) / 2,
                type_scale::CAPTION,
                &state.clock_time,
                layout::ink_on(&theme, theme.overlay),
            ))
        } else {
            None
        },
        title: Run::whole(pad.x, title_y, type_scale::TITLE, b"Power", theme.text),
        summary: Run::whole(pad.x, sub_y, type_scale::CAPTION, &summary[..summary_len], theme.text_muted),
        pills,
        dismiss: pill_plan(DISMISS_PILL, b"Cancel", b"", PillSkin::live(&theme)),
        indicator: Rect::new(
            panel.x + (panel.w - indicator_w) / 2,
            SCREEN_HEIGHT - space::MD - indicator_h,
            indicator_w,
            indicator_h,
        ),
    }
}

/// This frame's plan.
///
/// WHY `(0, 0)` is the frame boundary: `render_view` walks the surface in raster
/// order from the top-left pixel, so `(0, 0)` is the first pixel of every frame and
/// the only pixel within a frame that repeats it. `LauncherState` is borrowed
/// immutably for the whole of a frame, so rebuilding there is once per frame -- which
/// is also the only cadence at which the registry can have changed underneath it.
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
/// Paints one pill from a resolved plan: rounded fill, hairline outline, a centred
/// label with a centred status line under it.
fn pill_pixel(x: usize, y: usize, theme: &Theme, plan: &PillPlan) -> u32 {
    if !round_contains(&plan.rect, plan.corner, x, y) {
        return theme.bg;
    }
    if !round_contains(&plan.inner, plan.corner.saturating_sub(1), x, y) {
        return plan.skin.border;
    }
    if let Some(c) = plan.label.pixel(x, y) {
        return c;
    }
    if let Some(c) = plan.status.pixel(x, y) {
        return c;
    }
    plan.skin.fill
}

pub fn get_power_menu_color(x: usize, y: usize, state: &LauncherState) -> u32 {
    let theme = THEME;

    // WHY the plan is fetched before anything is drawn: it carries every rectangle,
    // every line and every skin for this frame, and fetching it is the only place in
    // this function where anything other than a pixel test happens.
    let f = frame(x, y, state);

    // Status bar: the same chrome every other surface draws, so the drawer does
    // not read as a different device.
    if y < STATUS_BAR_HEIGHT {
        if let Some(c) = f.clock.as_ref().and_then(|r| r.pixel(x, y)) {
            return c;
        }
        return theme.overlay;
    }
    if y == STATUS_BAR_HEIGHT {
        return theme.overlay_border;
    }

    // WHY the scrim rather than a flat fill of the previous screen's colour: a
    // modal drawer dims what is behind it, and `darken` is the one place that
    // knows how dark that is.
    if x < DRAWER_X {
        return darken(theme.bg, 55);
    }
    if x == DRAWER_X {
        return theme.border;
    }

    // Title and the count of what the shell can actually do.
    if let Some(c) = f.title.pixel(x, y) {
        return c;
    }
    // WHY the summary is the count and nothing else: each of the four actions is
    // checked against the shell's own capabilities, and an action the kernel cannot
    // perform says so on its own pill rather than in a word here.
    if let Some(c) = f.summary.pixel(x, y) {
        return c;
    }

    // The actions, each drawn only as live when the shell can perform it. WHY the
    // order is kept: the pills are scanned in the same order as before, and the
    // first one containing the pixel is the one drawn.
    for index in 0..ACTION_COUNT {
        if !round_contains(&f.pills[index].rect, radius::SHEET, x, y) {
            continue;
        }
        return pill_pixel(x, y, &theme, &f.pills[index]);
    }

    if round_contains(&f.dismiss.rect, radius::CONTROL, x, y) {
        return pill_pixel(x, y, &theme, &f.dismiss);
    }

    // Home indicator, over the drawer only: the scrim belongs to the screen
    // underneath and is not this surface's to annotate.
    if round_contains(&f.indicator, f.indicator.h / 2, x, y) {
        return Theme::blend(theme.text_muted, theme.bg, 140);
    }

    theme.bg
}
