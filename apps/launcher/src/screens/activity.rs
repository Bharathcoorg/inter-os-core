//! The task switcher: a card per running task, with a live preview of what the
//! task is showing right now.
//!
//! WHY this screen was rebuilt: the old one drew a dark deck of cards with a
//! hand-written palette, and the "preview" in each card was a set of invented
//! layouts -- a keypad for a phone app, bubbles for a messages app, a grid for
//! anything else. Those were pictures of what an app of that kind might look
//! like, not pictures of the app, and the surface said "No live preview" in
//! the corner while showing a drawing of one.
//!
//! WHY there is a real preview now: the kernel holds every client's surface,
//! and a client that holds the `SurfaceCapture` capability may read one. The
//! shell holds that capability, so the switcher can ask the platform what a
//! running task is actually showing and show that. When the platform will not
//! answer -- the app has not drawn a frame, the app has no surface, the
//! capture was refused -- the card says which of those it was, and draws no
//! picture at all.
//!
//! WHY the cards use the platform's tokens: the shell draws every other
//! surface with the platform's theme, and a switcher with its own dark
//! palette read as a different operating system inside the same one.

use crate::LauncherState;
use interos_std as std;
use launcher::icons::{self, IconKind};
use launcher::layout::Rect;
use launcher::recents::{self, Card, Preview, MAX_CARDS};
use launcher::text::Label;
use launcher::theme::{radius, type_scale, Theme, STATUS_BAR_HEIGHT};

/// The theme the surface is drawn with.
const THEME: Theme = Theme::LIGHT;

/// How many pixels one preview holds.
const PREVIEW_PIXELS: usize = recents::PREVIEW_W * recents::PREVIEW_H;

/// The captured previews, one per card.
///
/// WHY the pixels live here and not in the state: a 150x300 ARGB picture is
/// 180 KB, and the shell's state lives on a 256 KB user stack. The pixels are
/// per-frame data, not state, and the plan reads them straight out of here.
static mut PREVIEWS: [[u32; PREVIEW_PIXELS]; MAX_CARDS] = [[0; PREVIEW_PIXELS]; MAX_CARDS];

/// The package each picture belongs to.
///
/// WHY the picture is keyed by the package and not by the card: the shell's
/// recent list shifts when a new app is opened, and a picture that moved to
/// another app's card would be a picture of the wrong app.
static mut PICTURE_PACKAGE: [[u8; 40]; MAX_CARDS] = [[0; 40]; MAX_CARDS];
static mut PICTURE_PACKAGE_LEN: [usize; MAX_CARDS] = [0; MAX_CARDS];
/// Why the last capture of a picture failed, if it did.
static mut PICTURE_REASON: [&'static str; MAX_CARDS] = [""; MAX_CARDS];

/// The picture slot that holds `package`'s picture, if there is one.
fn picture_slot(package: &[u8]) -> Option<usize> {
    for slot in 0..MAX_CARDS {
        let len = unsafe { PICTURE_PACKAGE_LEN[slot] };
        if len > 0 && len == package.len() {
            // SAFETY: the launcher is one bare-metal task, so this is the only
            // reference to the table at any time.
            let held = unsafe { &PICTURE_PACKAGE[slot][..len] };
            if held == package {
                return Some(slot);
            }
        }
    }
    None
}

/// The slot a new picture for `package` would take.
fn free_picture_slot(package: &[u8]) -> usize {
    if let Some(slot) = picture_slot(package) {
        return slot;
    }
    for slot in 0..MAX_CARDS {
        // SAFETY: as above.
        if unsafe { PICTURE_PACKAGE_LEN[slot] } == 0 {
            return slot;
        }
    }
    0
}

/// Whether a picture of `package` is held.
pub fn has_picture(package: &[u8]) -> bool {
    picture_slot(package).is_some()
}

/// Why the last capture of `package` failed, if it did.
pub fn capture_reason(package: &[u8]) -> Option<&'static str> {
    let slot = picture_slot(package)?;
    // SAFETY: as above.
    let reason = unsafe { PICTURE_REASON[slot] };
    if reason.is_empty() {
        None
    } else {
        Some(reason)
    }
}

/// Captures a running task's surface and keeps it for its package.
///
/// WHY the shell captures a foreground app while it runs: an app in this
/// platform exits when the person leaves it, so the only way the switcher can
/// hold a picture of an app at all is to take it while the app is still
/// drawing. The picture is the app's own pixels, read through the platform's
/// capture capability -- not a drawing of what an app of that kind might look
/// like.
pub fn capture_package(package: &[u8], surface: u32) -> Preview {
    if package.is_empty() || package.len() > 40 || surface == 0 {
        return Preview::Unavailable("the app has no surface");
    }
    let slot = free_picture_slot(package);
    let slot_ptr: *mut [[u32; PREVIEW_PIXELS]; MAX_CARDS] = core::ptr::addr_of_mut!(PREVIEWS);
    // SAFETY: the launcher is one bare-metal task, so this is the only
    // reference to the buffer at any time, and the slice borrows it for the
    // length of the call that fills it.
    let dest: &mut [u32] = unsafe {
        let slot_ptr = &mut *slot_ptr;
        &mut slot_ptr[slot][..]
    };
    match std::surface_capture(
        surface,
        recents::PREVIEW_W as u32,
        recents::PREVIEW_H as u32,
        dest,
    ) {
        Ok(_) => {
            // SAFETY: as above.
            unsafe {
                PICTURE_PACKAGE[slot][..package.len()].copy_from_slice(package);
                PICTURE_PACKAGE_LEN[slot] = package.len();
                PICTURE_REASON[slot] = "";
            }
            Preview::Live {
                width: recents::PREVIEW_W as u16,
                height: recents::PREVIEW_H as u16,
            }
        }
        // WHY the reasons are the platform's answers and not a guess: the
        // kernel refuses a capture it will not authorise and a surface it
        // cannot find, and the card has to say which of those happened rather
        // than say "no preview" and leave the reader guessing.
        Err(-1) => {
            // SAFETY: as above.
            unsafe { PICTURE_REASON[slot] = "the shell may not read this app's screen" };
            Preview::Unavailable("the shell may not read this app's screen")
        }
        Err(-6) => {
            // SAFETY: as above.
            unsafe { PICTURE_REASON[slot] = "the app has no surface" };
            Preview::Unavailable("the app has no surface")
        }
        Err(-30) => {
            // SAFETY: as above.
            unsafe { PICTURE_REASON[slot] = "the app has not drawn a frame yet" };
            Preview::Unavailable("the app has not drawn a frame yet")
        }
        Err(_) => {
            // SAFETY: as above.
            unsafe { PICTURE_REASON[slot] = "the capture was refused" };
            Preview::Unavailable("the capture was refused")
        }
    }
}

/// One card, resolved.
#[derive(Clone, Copy)]
struct CardPlan {
    rect: Rect,
    preview: Rect,
    /// The picture slot this card's picture is in, if it has one.
    picture: Option<usize>,
    icon: Option<IconKind>,
    badge: [u8; 4],
    badge_len: usize,
    name: Label,
    package: Label,
    state: Label,
    reason: Label,
    close: Rect,
}

/// The frame's plan.
struct Plan {
    clock: Label,
    title: Label,
    count: Label,
    cards: [CardPlan; MAX_CARDS],
    card_count: usize,
    close_all: Rect,
    close_all_label: Label,
    empty: Label,
    empty_hint: Label,
    /// The card list this plan was built from.
    cached_fingerprint: u64,
}

/// This frame's plan.
static mut PLAN: Option<Plan> = None;

/// Invalidates the plan so the next frame is rebuilt.
///
/// WHY the shell calls this: the cards move when one is dismissed, and a
/// banded repaint does not pass through the top-left pixel, so the plan would
/// otherwise keep describing the card list before the dismissal.
pub fn invalidate_activity_frame() {
    let slot: *mut Option<Plan> = core::ptr::addr_of_mut!(PLAN);
    // SAFETY: the launcher is one bare-metal task; the shell owns this slot and
    // the paint loop is not reentrant.
    unsafe { *slot = None };
}

/// The rows the cards occupy.
pub fn recents_band() -> (usize, usize) {
    let close_all = close_all_rect();
    (recents::CARDS_Y0, close_all.y + close_all.h)
}

/// The close-all control.
pub fn close_all_rect() -> Rect {
    Rect::new(232, 1268, 256, 64)
}

/// The card whose close control holds (`x`, `y`).
pub fn activity_close_hit(state: &LauncherState, x: usize, y: usize) -> Option<usize> {
    recents::close_at(x, y, state.recents_count)
}

/// Dismisses the card in `slot`.
///
/// WHY this stops the task and not just the card: a card that is removed from
/// the switcher while its task keeps running is a card that comes back the
/// next time the shell asks the platform what is running.
pub fn activity_close_slot(state: &mut LauncherState, slot: usize) {
    if slot >= state.recents_count {
        return;
    }
    let task = state.recents_cards[slot].task_id;
    if task > 0 {
        let _ = std::app_stop(task);
    }
    let mut index = slot;
    while index + 1 < state.recents_count {
        state.recents_cards[index] = state.recents_cards[index + 1];
        state.recents_surfaces[index] = state.recents_surfaces[index + 1];
        index += 1;
    }
    if state.recents_count > 0 {
        state.recents_count -= 1;
    }
    state.recents_cards[state.recents_count] = Card::EMPTY;
    state.recents_surfaces[state.recents_count] = 0;
    invalidate_activity_frame();
}

/// The preview state for a card, given what the platform can say about it.
///
/// WHY the state is computed in one place: the shell rebuilds the card list
/// ten times a second and the switcher refreshes the pictures on its own
/// schedule, and two places answering "what does this card show?" would
/// disagree with each other between the two.
pub fn preview_state(package: &[u8], task_id: u64, surface: u32) -> Preview {
    if task_id != 0 && surface != 0 {
        // The app is running: the picture is of what it is showing now.
        if has_picture(package) {
            Preview::Live {
                width: recents::PREVIEW_W as u16,
                height: recents::PREVIEW_H as u16,
            }
        } else {
            Preview::Unavailable("not captured yet")
        }
    } else if has_picture(package) {
        // The app has closed, and this is the last thing it drew.
        Preview::LastFrame {
            width: recents::PREVIEW_W as u16,
            height: recents::PREVIEW_H as u16,
        }
    } else if let Some(reason) = capture_reason(package) {
        Preview::Unavailable(reason)
    } else {
        Preview::Unavailable("the app is not running")
    }
}

/// Refreshes the cards' previews from the pictures the shell holds.
///
/// WHY the shell captures a foreground app while it runs: an app in this
/// platform exits when the person leaves it, so the picture a card shows is
/// the app's own last frame rather than a live view of a task that is still
/// running. The card says which of the two it is.
pub fn refresh_previews(state: &mut LauncherState) {
    for index in 0..state.recents_count {
        let task_id = state.recents_cards[index].task_id;
        let surface = state.recents_surfaces[index];
        // WHY the copy: the state is borrowed mutably on the next line, and
        // the package has to be read before that.
        let mut package = [0u8; 40];
        let package_len = state.recents_cards[index].package().len().min(40);
        package[..package_len].copy_from_slice(&state.recents_cards[index].package()[..package_len]);
        if task_id != 0 && surface != 0 {
            // The app is still running: take a fresh picture of it.
            let _ = capture_package(&package[..package_len], surface);
        }
        state.recents_cards[index].preview = preview_state(&package[..package_len], task_id, surface);
    }
}

fn plan(x: usize, y: usize, state: &LauncherState) -> &'static Plan {
    let slot: *mut Option<Plan> = core::ptr::addr_of_mut!(PLAN);
    let fingerprint = fingerprint(state);
    // SAFETY: as in `invalidate_activity_frame`; the shell owns this slot.
    let should_rebuild = unsafe {
        match (*slot).as_ref() {
            None => true,
            Some(p) => (x == 0 && y == 0) || p.cached_fingerprint != fingerprint,
        }
    };
    if should_rebuild {
        // SAFETY: as above; this is the one mutation of the slot.
        unsafe { *slot = Some(build(state, fingerprint)) };
    }
    // SAFETY: the slot is `Some` here -- it was just filled when it was empty.
    unsafe { (*slot).as_ref() }.expect("WHY: the slot is filled when it is empty")
}

/// A fingerprint of the card list, so a plan is rebuilt when the cards change.
fn fingerprint(state: &LauncherState) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    hash ^= state.recents_count as u64;
    hash = hash.wrapping_mul(0x1000_0000_01b3);
    for index in 0..state.recents_count {
        let card = &state.recents_cards[index];
        for &byte in card.package() {
            hash ^= byte as u64;
            hash = hash.wrapping_mul(0x1000_0000_01b3);
        }
        hash ^= card.task_id;
        hash = hash.wrapping_mul(0x1000_0000_01b3);
        hash ^= match card.preview {
            Preview::Live { .. } => 1u64,
            Preview::LastFrame { .. } => 3u64,
            Preview::Unavailable(_) => 2u64,
        };
        hash = hash.wrapping_mul(0x1000_0000_01b3);
    }
    hash
}

fn build(state: &LauncherState, fingerprint: u64) -> Plan {
    let theme = THEME;
    let mut plan = Plan {
        clock: Label::at(
            48,
            14,
            type_scale::CAPTION,
            if state.clock_source_ok {
                &state.clock_time
            } else {
                b""
            },
            theme.on_surface,
        ),
        title: Label::at(32, 72, type_scale::TITLE, b"Recents", theme.on_surface),
        count: Label::at(0, 0, type_scale::CAPTION, b"", theme.on_surface_variant),
        cards: [CardPlan {
            rect: Rect::new(0, 0, 0, 0),
            preview: Rect::new(0, 0, 0, 0),
            picture: None,
            icon: None,
            badge: [0; 4],
            badge_len: 0,
            name: Label::EMPTY,
            package: Label::EMPTY,
            state: Label::EMPTY,
            reason: Label::EMPTY,
            close: Rect::new(0, 0, 0, 0),
        }; MAX_CARDS],
        card_count: 0,
        close_all: close_all_rect(),
        close_all_label: Label::at(
            close_all_rect().x
                + (close_all_rect().w - launcher::text::measure(b"Close all", type_scale::BODY)) / 2,
            close_all_rect().y
                + (close_all_rect().h - launcher::text::line_height(type_scale::BODY)) / 2,
            type_scale::BODY,
            b"Close all",
            theme.on_primary,
        ),
        empty: Label::EMPTY,
        empty_hint: Label::EMPTY,
        cached_fingerprint: fingerprint,
    };

    let count = state.recents_count.min(MAX_CARDS);
    let live = state
        .recents_cards
        .iter()
        .take(count)
        .filter(|card| card.preview.is_live())
        .count();
    let count_text = count_label(live, count, &mut [0u8; 32]);
    plan.count = Label::at(
        688 - launcher::text::measure(&count_text.0[..count_text.1], type_scale::CAPTION),
        96,
        type_scale::CAPTION,
        &count_text.0[..count_text.1],
        theme.on_surface_variant,
    );

    if count == 0 {
        plan.empty = Label::at(
            32,
            200,
            type_scale::TITLE,
            b"No recent apps",
            theme.on_surface,
        );
        plan.empty_hint = Label::at(
            32,
            280,
            type_scale::BODY,
            b"Open an app and it will appear here.",
            theme.on_surface_variant,
        );
        return plan;
    }

    for index in 0..count {
        let card = &state.recents_cards[index];
        let rect = recents::card_rect(index);
        let preview = recents::preview_rect(index);
        let icon = icons::match_icon_kind(card.package());
        let mut badge = [0u8; 4];
        let mut badge_len = 0usize;
        if icon.is_none() {
            let title = card.label();
            let mut at = 0usize;
            let mut in_word = false;
            for &c in title.iter() {
                if at >= 4 {
                    break;
                }
                let is_sep = c == b' ' || c == b'-' || c == b'_' || c == b'.';
                if !is_sep && !in_word {
                    badge[at] = c.to_ascii_uppercase();
                    at += 1;
                }
                in_word = !is_sep;
            }
            badge_len = at;
        }
        let reason = card.preview.reason();
        // WHY the text is fitted to its column: an app's name can be longer
        // than the card, and a label that runs under the close control is two
        // labels on top of each other.
        let column = recents::close_rect(index).x.saturating_sub(rect.x + 268 + 12);
        let (name, name_len) = launcher::text::ellipsize(card.label(), type_scale::BODY, column);
        let (package, package_len) =
            launcher::text::ellipsize(card.package(), type_scale::CAPTION, column);
        let (reason_text, reason_len) = launcher::text::ellipsize(
            reason.unwrap_or("").as_bytes(),
            type_scale::CAPTION,
            column,
        );
        // WHY the state line says which kind of picture the card holds: a
        // picture of what the app is showing now and a picture of what it
        // showed when it closed are both real, and they are not the same
        // claim.
        let state_text: &[u8] = match card.preview {
            Preview::Live { .. } => {
                if card.focused {
                    b"In front".as_slice()
                } else {
                    b"Running".as_slice()
                }
            }
            Preview::LastFrame { .. } => b"Last frame".as_slice(),
            Preview::Unavailable(_) => b"Not running".as_slice(),
        };
        plan.cards[index] = CardPlan {
            rect,
            preview,
            picture: picture_slot(card.package()),
            icon,
            badge,
            badge_len,
            name: Label::at(
                rect.x + 268,
                rect.y + 24,
                type_scale::BODY,
                &name[..name_len],
                theme.on_surface,
            ),
            package: Label::at(
                rect.x + 268,
                rect.y + 78,
                type_scale::CAPTION,
                &package[..package_len],
                theme.on_surface_variant,
            ),
            state: Label::at(
                rect.x + 268,
                rect.y + 112,
                type_scale::CAPTION,
                state_text,
                theme.on_surface_variant,
            ),
            reason: Label::at(
                rect.x + 268,
                rect.y + 146,
                type_scale::CAPTION,
                &reason_text[..reason_len],
                theme.error,
            ),
            close: recents::close_rect(index),
        };
    }
    plan.card_count = count;
    plan
}

/// The "N running" label.
///
/// WHY the count names both numbers when they differ: a card whose app is
/// still running shows a live picture and a card whose app is not shows the
/// reason it cannot, and the header has to say which is which. "3 running"
/// over a card that cannot show anything would be a lie.
fn count_label(live: usize, total: usize, buf: &mut [u8; 32]) -> ([u8; 32], usize) {
    let mut out = *buf;
    let mut at = 0usize;
    let write = |out: &mut [u8; 32], at: &mut usize, digits: &[u8]| {
        for &digit in digits {
            if *at + 1 >= out.len() {
                return;
            }
            out[*at] = digit;
            *at += 1;
        }
    };
    let digits = |value: usize, out: &mut [u8; 32], at: &mut usize| {
        let mut scratch = [0u8; 20];
        let mut n = 0usize;
        let mut v = value;
        while v > 0 && n < scratch.len() {
            scratch[n] = b'0' + (v % 10) as u8;
            v /= 10;
            n += 1;
        }
        for i in (0..n).rev() {
            write(out, at, &[scratch[i]]);
        }
    };
    digits(live, &mut out, &mut at);
    if live != total {
        write(&mut out, &mut at, b"/");
        digits(total, &mut out, &mut at);
    }
    write(&mut out, &mut at, b" running");
    (out, at)
}

/// The colour of (`x`, `y`) on the task switcher.
pub fn get_activity_color(x: usize, y: usize, state: &LauncherState) -> u32 {
    let p = plan(x, y, state);
    let theme = THEME;

    if y < STATUS_BAR_HEIGHT {
        if let Some(c) = p.clock.pixel(x, y, theme.surface) {
            return c;
        }
        return theme.surface;
    }

    if let Some(c) = p.title.pixel(x, y, theme.surface) {
        return c;
    }
    if let Some(c) = p.count.pixel(x, y, theme.surface) {
        return c;
    }

    for index in 0..p.card_count {
        let card = &p.cards[index];
        if !card.rect.contains(x, y) {
            continue;
        }
        let fill = theme.surface_container;
        if !inside_rounded(card.rect.inset(1), radius::CARD.saturating_sub(1), x, y) {
            return theme.border;
        }
        // The live preview, or the reason there is not one.
        if card.preview.contains(x, y) {
            if let Some(slot) = card.picture {
                if let Some(c) = picture_pixel(slot, x - card.preview.x, y - card.preview.y) {
                    return c;
                }
            }
            return preview_placeholder(x, y, &card.preview, &theme);
        }
        if let Some(c) = card_icon(x, y, card) {
            return c;
        }
        if let Some(c) = card.name.pixel(x, y, fill) {
            return c;
        }
        if let Some(c) = card.package.pixel(x, y, fill) {
            return c;
        }
        if let Some(c) = card.state.pixel(x, y, fill) {
            return c;
        }
        if let Some(c) = card.reason.pixel(x, y, fill) {
            return c;
        }
        if let Some(c) = close_button(x, y, &card.close, &theme) {
            return c;
        }
        return fill;
    }

    if p.card_count == 0 {
        if let Some(c) = p.empty.pixel(x, y, theme.surface) {
            return c;
        }
        if let Some(c) = p.empty_hint.pixel(x, y, theme.surface) {
            return c;
        }
    }

    // The close-all control.
    if inside_rounded(p.close_all, radius::PILL, x, y) {
        let fill = theme.primary;
        if let Some(c) = p.close_all_label.pixel(x, y, fill) {
            return c;
        }
        return fill;
    }

    theme.surface
}

/// The captured pixel at (`px`, `py`) of the picture in `slot`.
fn picture_pixel(slot: usize, px: usize, py: usize) -> Option<u32> {
    if px >= recents::PREVIEW_W || py >= recents::PREVIEW_H {
        return None;
    }
    let slot_ptr: *const [[u32; PREVIEW_PIXELS]; MAX_CARDS] = core::ptr::addr_of!(PREVIEWS);
    // SAFETY: the launcher is one bare-metal task, so this is the only
    // reference to the buffer at any time, and the read is inside it.
    let buffer = unsafe { &(*slot_ptr)[slot] };
    Some(buffer[py * recents::PREVIEW_W + px])
}

/// The well drawn where a preview is not available.
///
/// WHY it says so and why it names the reason: the old screen drew a picture
/// of what the app might be showing and put "No live preview" in the corner
/// of it. A reader could not tell which was the truth. The well now says what
/// the platform answered, and the card's text column repeats the reason.
fn preview_placeholder(x: usize, y: usize, rect: &Rect, theme: &Theme) -> u32 {
    let fill = theme.surface_container_low;
    let label = Label::at(
        rect.x + 12,
        rect.y + rect.h / 2 - 30,
        type_scale::CAPTION,
        b"No preview",
        theme.on_surface_variant,
    );
    if let Some(c) = label.pixel(x, y, fill) {
        return c;
    }
    fill
}

/// The card's pictogram, or its acronym.
fn card_icon(x: usize, y: usize, card: &CardPlan) -> Option<u32> {
    let cx = card.rect.x + 196 + 28;
    let cy = card.rect.y + 24 + 28;
    if let Some(kind) = card.icon {
        return icons::rasterize_icon_pixel_scaled(kind, x, y, cx, cy, THEME.on_surface, 28);
    }
    if card.badge_len == 0 {
        return None;
    }
    let scale = type_scale::CAPTION;
    let w = launcher::text::measure(&card.badge[..card.badge_len], scale);
    let h = launcher::text::line_height(scale);
    let x0 = cx.saturating_sub(w / 2);
    let y0 = cy.saturating_sub(h / 2);
    crate::test_text_aa(
        x,
        y,
        x0,
        y0,
        scale,
        &card.badge[..card.badge_len],
        THEME.on_surface,
        THEME.surface_container,
    )
}

/// The close control: a ring with a cross in it.
fn close_button(x: usize, y: usize, rect: &Rect, theme: &Theme) -> Option<u32> {
    if !rect.contains(x, y) {
        return None;
    }
    let cx = rect.center_x() as i32;
    let cy = rect.center_y() as i32;
    let dx = x as i32 - cx;
    let dy = y as i32 - cy;
    let d2 = dx * dx + dy * dy;
    // The ring.
    if d2 <= 144 && d2 >= 100 {
        return Some(theme.on_surface_variant);
    }
    // The cross.
    //
    // WHY the cross is diagonal: a vertical and a horizontal stroke in the
    // middle of a ring read as a plus, and a plus on a card reads as "add".
    if (dx - dy).abs() <= 1 && dx.abs() <= 7 {
        return Some(theme.on_surface_variant);
    }
    if (dx + dy).abs() <= 1 && dx.abs() <= 7 {
        return Some(theme.on_surface_variant);
    }
    None
}

/// Whether (`x`, `y`) is inside a rounded rectangle.
fn inside_rounded(rect: Rect, radius: usize, x: usize, y: usize) -> bool {
    if !rect.contains(x, y) {
        return false;
    }
    let r = radius.min(rect.w / 2).min(rect.h / 2);
    if r == 0 {
        return true;
    }
    let left = rect.x + r;
    let right = rect.x + rect.w - r;
    let top = rect.y + r;
    let bottom = rect.y + rect.h - r;
    let cx = if x < left {
        left
    } else if x >= right {
        right
    } else {
        x
    };
    let cy = if y < top {
        top
    } else if y >= bottom {
        bottom
    } else {
        y
    };
    let dx = x as i32 - cx as i32;
    let dy = y as i32 - cy as i32;
    (dx * dx + dy * dy) as usize <= r * r
}