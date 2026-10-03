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

use super::system_space::{
    darken, decimal, push, round_contains, run_center, run_left, VALUE_MAX,
};
use crate::LauncherState;
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

/// Paints one pill: rounded fill, hairline outline, a centred label with a
/// centred status line under it.
fn pill_pixel(
    x: usize,
    y: usize,
    rect: &Rect,
    theme: &Theme,
    label: &[u8],
    status: &[u8],
    skin: &PillSkin,
) -> u32 {
    let corner = radius::SHEET.min(rect.h / 2);
    if !round_contains(rect, corner, x, y) {
        return theme.bg;
    }
    let inner = rect.inset(1);
    if !round_contains(&inner, corner.saturating_sub(1), x, y) {
        return skin.border;
    }

    // WHY the two lines are laid out as one block: a label and its status read
    // as a single control when they share a baseline group, and the block is
    // centred against the pill rather than each line against the pill. A pill
    // with no status line centres its label on its own, because a label
    // floating high in a button reads as misaligned rather than as quiet.
    let label_h = text::line_height(type_scale::BODY);
    let status_h = text::line_height(type_scale::CAPTION);
    let block_h = if status.is_empty() {
        label_h
    } else {
        label_h + space::XS + status_h
    };
    let top = rect.y + rect.h.saturating_sub(block_h) / 2;
    let label_rect = Rect::new(rect.x + space::MD, top, rect.w - space::MD * 2, label_h);
    let status_rect = Rect::new(
        rect.x + space::MD,
        top + label_h + space::XS,
        rect.w - space::MD * 2,
        status_h,
    );

    let (label_run, label_len) = text::ellipsize(label, type_scale::BODY, label_rect.w);
    if let Some(c) = run_center(
        x,
        y,
        &label_rect,
        type_scale::BODY,
        &label_run[..label_len],
        skin.label_ink,
    ) {
        return c;
    }
    let (status_run, status_len) = text::ellipsize(status, type_scale::CAPTION, status_rect.w);
    if let Some(c) = run_center(
        x,
        y,
        &status_rect,
        type_scale::CAPTION,
        &status_run[..status_len],
        skin.status_ink,
    ) {
        return c;
    }
    skin.fill
}

pub fn get_power_menu_color(x: usize, y: usize, state: &LauncherState) -> u32 {
    let theme = THEME;

    // Status bar: the same chrome every other surface draws, so the drawer does
    // not read as a different device.
    let bar = Rect::new(0, 0, SCREEN_WIDTH, STATUS_BAR_HEIGHT);
    if y < STATUS_BAR_HEIGHT {
        if state.clock_source_ok {
            let area = Rect::new(bar.x + CONTENT_PADDING, bar.y, bar.w - CONTENT_PADDING * 2, bar.h);
            if let Some(c) = run_left(
                x,
                y,
                &area,
                type_scale::CAPTION,
                &state.clock_time,
                layout::ink_on(&theme, theme.overlay),
            ) {
                return c;
            }
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

    let panel = Rect::new(DRAWER_X, STATUS_BAR_HEIGHT, SCREEN_WIDTH - DRAWER_X, SCREEN_HEIGHT);
    let pad = panel.inset(space::LG);

    // Title and the count of what the shell can actually do.
    let title_rect = Rect::new(
        pad.x,
        pad.y + space::MD,
        pad.w,
        text::line_height(type_scale::TITLE),
    );
    if let Some(c) = run_left(x, y, &title_rect, type_scale::TITLE, b"Power", theme.text) {
        return c;
    }
    let sub_rect = Rect::new(
        pad.x,
        title_rect.y + title_rect.h + space::XS,
        pad.w,
        text::line_height(type_scale::CAPTION),
    );
    // WHY the summary is built only once the pixel is on that line: counting the
    // live actions walks the package registry, and this function runs for every
    // pixel of the drawer.
    if y >= sub_rect.y && y < sub_rect.y + sub_rect.h {
        let (summary, summary_len) = action_summary(state);
        if let Some(c) = run_left(
            x,
            y,
            &sub_rect,
            type_scale::CAPTION,
            &summary[..summary_len],
            theme.text_muted,
        ) {
            return c;
        }
    }

    // The actions, each drawn only as live when the shell can perform it.
    for index in 0..ACTION_COUNT {
        let rect = PILLS[index];
        if !round_contains(&rect, radius::SHEET, x, y) {
            continue;
        }
        let (label, can) = action(state, index);
        let skin = action_skin(&theme, index, &can);
        return pill_pixel(x, y, &rect, &theme, label, can.status(), &skin);
    }

    if round_contains(&DISMISS_PILL, radius::CONTROL, x, y) {
        return pill_pixel(x, y, &DISMISS_PILL, &theme, b"Cancel", b"", &PillSkin::live(&theme));
    }

    // Home indicator, over the drawer only: the scrim belongs to the screen
    // underneath and is not this surface's to annotate.
    let indicator_w = space::XL * 4;
    let indicator_h = space::XS;
    let indicator = Rect::new(
        panel.x + (panel.w - indicator_w) / 2,
        SCREEN_HEIGHT - space::MD - indicator_h,
        indicator_w,
        indicator_h,
    );
    if round_contains(&indicator, indicator_h / 2, x, y) {
        return Theme::blend(theme.text_muted, theme.bg, 140);
    }

    theme.bg
}