//! The task switcher: recent activity, one app at a time.
//!
//! WHY one large card and a selector rail: an app's screen is 720x1440, so a
//! faithful preview is 1:2. Three previews stacked in the 984 px between the
//! header and the close control can be only 150 px wide each, which is
//! smaller than the app's own title and icon and tells nobody what the app
//! was showing. The switcher therefore shows the focused app's real frame at
//! 402x804, with a selector rail naming the other recent apps and their
//! state. The rail, not a fabricated chrome, is what says there is more than
//! one recent app.
//!
//! The cards come from `refresh_recents_cards`, which asks the platform which
//! tasks are live and which surfaces belong to them; the previews are the
//! apps' own captured pixels. A card that has no picture says why.
//!
//! Zero heap allocations: every plan is a fixed-size array, rebuilt in place.

use crate::LauncherState;
use interos_std as std;
use interos_std::design::theme::AccentPalette;
use launcher::icons::{self, IconKind};
use launcher::layout::Rect;
use launcher::recents::{self, Card, Preview, MAX_CARDS};
use launcher::text::Label;
use launcher::theme::{type_scale, Theme, STATUS_BAR_HEIGHT};

/// The theme the surface is drawn with.
const THEME: Theme = launcher::theme::SHELL;

/// How many pixels one preview holds.
const PREVIEW_PIXELS: usize = recents::PREVIEW_W * recents::PREVIEW_H;
const PALETTE_LIGHT: AccentPalette = AccentPalette::VIBRANT_LIGHT;

/// The captured previews, one per card.
static mut PREVIEWS: [[u32; PREVIEW_PIXELS]; MAX_CARDS] = [[0; PREVIEW_PIXELS]; MAX_CARDS];

/// The package each picture belongs to.
static mut PICTURE_PACKAGE: [[u8; 40]; MAX_CARDS] = [[0; 40]; MAX_CARDS];
static mut PICTURE_PACKAGE_LEN: [usize; MAX_CARDS] = [0; MAX_CARDS];
/// Why the last capture of a picture failed, if it did.
static mut PICTURE_REASON: [&'static str; MAX_CARDS] = [""; MAX_CARDS];

/// The picture slot that holds `package`'s picture, if there is one.
pub(crate) fn picture_slot(package: &[u8]) -> Option<usize> {
    for slot in 0..MAX_CARDS {
        let len = unsafe { PICTURE_PACKAGE_LEN[slot] };
        if len > 0 && len == package.len() {
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
    let reason = unsafe { PICTURE_REASON[slot] };
    if reason.is_empty() {
        None
    } else {
        Some(reason)
    }
}

/// Captures a running task's surface and keeps it for its package.
pub fn capture_package(package: &[u8], surface: u32) -> Preview {
    if package.is_empty() || package.len() > 40 || surface == 0 {
        return Preview::Unavailable("the app has no surface");
    }
    let slot = free_picture_slot(package);
    let slot_ptr: *mut [[u32; PREVIEW_PIXELS]; MAX_CARDS] = core::ptr::addr_of_mut!(PREVIEWS);
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
        Err(-1) => {
            unsafe { PICTURE_REASON[slot] = "the shell may not read this app's screen" };
            Preview::Unavailable("the shell may not read this app's screen")
        }
        Err(-6) => {
            unsafe { PICTURE_REASON[slot] = "the app has no surface" };
            Preview::Unavailable("the app has no surface")
        }
        Err(-30) => {
            unsafe { PICTURE_REASON[slot] = "the app has not drawn a frame yet" };
            Preview::Unavailable("the app has not drawn a frame yet")
        }
        Err(_) => {
            unsafe { PICTURE_REASON[slot] = "the capture was refused" };
            Preview::Unavailable("the capture was refused")
        }
    }
}

/// Checks whether `slot` contains real, non-zero captured frame pixels.
pub(crate) fn slot_has_captured_pixels(slot: usize) -> bool {
    if slot >= MAX_CARDS {
        return false;
    }
    let len = unsafe { PICTURE_PACKAGE_LEN[slot] };
    if len == 0 {
        return false;
    }
    let slot_ptr: *const [[u32; PREVIEW_PIXELS]; MAX_CARDS] = core::ptr::addr_of!(PREVIEWS);
    let buffer = unsafe { &(*slot_ptr)[slot] };
    // Probe multiple points to distinguish a rendered frame from an unpainted zero buffer
    buffer[PREVIEW_PIXELS / 8] != 0
        || buffer[PREVIEW_PIXELS / 4] != 0
        || buffer[PREVIEW_PIXELS / 2] != 0
        || buffer[PREVIEW_PIXELS * 3 / 4] != 0
        || buffer[PREVIEW_PIXELS * 7 / 8] != 0
}

/// The status classification of an application card.
#[derive(Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)]
enum CardStateKind {
    Active,
    Running,
    LastFrame,
    NotRunning,
}

/// The selector rail: one chip per recent app, naming it and its state.
pub const SELECTOR_Y: usize = 132;
pub const SELECTOR_H: usize = 72;
pub const SELECTOR_GAP: usize = 8;
/// The focused card's frame.
pub const HERO_Y: usize = 220;
pub const HERO_H: usize = 904;
/// The header band inside the card, above the preview well.
const HERO_HEADER_H: usize = 84;
const HERO_PAD: usize = 16;

pub const CARD_X: usize = 32;
pub const CARD_W: usize = 656;

/// One chip of the selector rail.
#[derive(Clone, Copy)]
struct SelectorPlan {
    rect: Rect,
    label: Label,
    dot_x: usize,
    dot_color: u32,
    selected: bool,
}

/// The focused card's app identity, drawn in its header.
#[derive(Clone, Copy)]
struct CardPlan {
    icon_box: Rect,
    icon: Option<IconKind>,
    accent_color: u32,
    badge: [u8; 4],
    badge_len: usize,
}

/// The frame's complete layout plan.
struct Plan {
    /// `None` when the RTC refused: a clock is a measurement, and a made-up
    /// reading is exactly what the status bar must not show.
    clock: Option<Label>,
    title: Label,
    count: Label,
    selector: [SelectorPlan; MAX_CARDS],
    selector_count: usize,
    card: Option<CardPlan>,
    focus: usize,
    preview: Rect,
    picture: Option<usize>,
    preview_state: Preview,
    name: Label,
    status: Label,
    status_dot: (usize, usize, u32),
    close: Rect,
    close_all: Rect,
    close_all_label: Label,
    empty: Label,
    empty_hint: Label,
    cached_fingerprint: u64,
}

/// Cached frame plan.
static mut PLAN: Option<Plan> = None;

/// Invalidates the plan so the next frame is rebuilt.
pub fn invalidate_activity_frame() {
    let slot: *mut Option<Plan> = core::ptr::addr_of_mut!(PLAN);
    unsafe { *slot = None };
}

/// The rows the switcher occupies.
pub fn recents_band() -> (usize, usize) {
    let close_all = close_all_rect();
    (SELECTOR_Y, close_all.y + close_all.h)
}

/// The close-all control: prominent pill button, below the card.
pub fn close_all_rect() -> Rect {
    Rect::new(230, 1160, 260, 56)
}

/// The frame of the focused card.
pub fn hero_rect() -> Rect {
    Rect::new(CARD_X, HERO_Y, CARD_W, HERO_H)
}

/// The close control of the focused card.
pub fn hero_close_rect() -> Rect {
    Rect::new(CARD_X + CARD_W - 64, HERO_Y + 16, 48, 48)
}

/// The picture well inside the focused card.
///
/// WHY the well is fitted by height: the app's frame is 1:2, and a preview
/// stretched into another shape is a picture of a different screen.
pub fn hero_preview_rect() -> Rect {
    let top = HERO_Y + HERO_HEADER_H;
    let avail_h = HERO_H - HERO_HEADER_H - HERO_PAD;
    let w = avail_h / 2;
    Rect::new(CARD_X + (CARD_W - w) / 2, top, w, avail_h)
}

/// The chip of the selector rail at `index`.
pub fn selector_rect(index: usize) -> Rect {
    let w = (CARD_W - (MAX_CARDS - 1) * SELECTOR_GAP) / MAX_CARDS;
    Rect::new(CARD_X + index * (w + SELECTOR_GAP), SELECTOR_Y, w, SELECTOR_H)
}

/// The selector chip at (`x`, `y`), if any.
pub fn selector_hit(state: &LauncherState, x: usize, y: usize) -> Option<usize> {
    for index in 0..state.recents_count.min(MAX_CARDS) {
        if selector_rect(index).contains(x, y) {
            return Some(index);
        }
    }
    None
}

/// The focused card's close control.
pub fn activity_close_hit(state: &LauncherState, x: usize, y: usize) -> Option<usize> {
    if state.recents_count == 0 {
        return None;
    }
    if hero_close_rect().contains(x, y) {
        Some(state.activity_focus.min(state.recents_count - 1))
    } else {
        None
    }
}

/// Hit test for the focused card's body.
pub fn card_hit_at(state: &LauncherState, x: usize, y: usize) -> Option<usize> {
    if state.recents_count == 0 {
        return None;
    }
    if hero_rect().contains(x, y) {
        Some(state.activity_focus.min(state.recents_count - 1))
    } else {
        None
    }
}

/// Dismisses the card in `slot`.
pub fn activity_close_slot(state: &mut LauncherState, slot: usize) {
    if slot >= state.recents_count {
        return;
    }
    let task = state.recents_cards[slot].task_id;
    if task > 0 {
        let _ = std::app_background_task(task);
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
    // Keep the focus on a card that still exists, and on the same app when the
    // dismissed one was below it.
    if state.recents_count == 0 {
        state.activity_focus = 0;
    } else {
        if slot < state.activity_focus {
            state.activity_focus -= 1;
        }
        if state.activity_focus >= state.recents_count {
            state.activity_focus = state.recents_count - 1;
        }
    }
    invalidate_activity_frame();
}

/// The preview state for a card, given what the platform can say about it.
pub fn preview_state(package: &[u8], task_id: u64, surface: u32) -> Preview {
    if task_id != 0 && surface != 0 {
        if has_picture(package) {
            Preview::Live {
                width: recents::PREVIEW_W as u16,
                height: recents::PREVIEW_H as u16,
            }
        } else {
            Preview::Unavailable("not captured yet")
        }
    } else if has_picture(package) {
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
pub fn refresh_previews(state: &mut LauncherState) {
    for index in 0..state.recents_count {
        let task_id = state.recents_cards[index].task_id;
        let surface = state.recents_surfaces[index];
        let mut package = [0u8; 40];
        let package_len = state.recents_cards[index].package().len().min(40);
        package[..package_len].copy_from_slice(&state.recents_cards[index].package()[..package_len]);
        if task_id != 0 && surface != 0 {
            let _ = capture_package(&package[..package_len], surface);
        }
        state.recents_cards[index].preview = preview_state(&package[..package_len], task_id, surface);
    }
}

fn plan(x: usize, y: usize, state: &LauncherState) -> &'static Plan {
    let slot: *mut Option<Plan> = core::ptr::addr_of_mut!(PLAN);
    let fingerprint = fingerprint(state);
    let should_rebuild = unsafe {
        match (*slot).as_ref() {
            None => true,
            Some(p) => (x == 0 && y == 0) || p.cached_fingerprint != fingerprint,
        }
    };
    if should_rebuild {
        unsafe { *slot = Some(build(state, fingerprint)) };
    }
    unsafe { (*slot).as_ref() }.expect("Plan must be populated")
}

/// A fingerprint of the card list and the focused card, so a plan is rebuilt
/// when either changes.
fn fingerprint(state: &LauncherState) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    hash ^= state.recents_count as u64;
    hash = hash.wrapping_mul(0x1000_0000_01b3);
    hash ^= state.activity_focus as u64;
    hash = hash.wrapping_mul(0x1000_0000_01b3);
    for index in 0..state.recents_count.min(MAX_CARDS) {
        let card = &state.recents_cards[index];
        for &byte in card.package() {
            hash ^= byte as u64;
            hash = hash.wrapping_mul(0x1000_0000_01b3);
        }
        for &byte in card.label() {
            hash ^= byte as u64;
            hash = hash.wrapping_mul(0x1000_0000_01b3);
        }
        hash ^= card.task_id;
        hash = hash.wrapping_mul(0x1000_0000_01b3);
        match card.preview {
            Preview::Live { .. } => hash ^= 1u64,
            Preview::LastFrame { .. } => hash ^= 3u64,
            Preview::Unavailable(reason) => {
                hash ^= 2u64;
                for &byte in reason.as_bytes() {
                    hash ^= byte as u64;
                    hash = hash.wrapping_mul(0x1000_0000_01b3);
                }
            }
        }
        hash = hash.wrapping_mul(0x1000_0000_01b3);
    }
    hash
}

/// The colour of a card's state dot: a live task is green, a card holding the
/// app's last frame is amber, and a card with no task is neutral.
fn state_color(card: &Card) -> u32 {
    if card.task_id != 0 {
        THEME.success
    } else if card.preview.has_picture() {
        THEME.warning
    } else {
        THEME.on_surface_variant
    }
}

/// What a card's header says about the app, and nothing it cannot back.
fn status_text(card: &Card) -> &'static [u8] {
    if card.task_id != 0 {
        if card.preview.is_live() {
            b"Running"
        } else {
            b"Running \xc2\xb7 no frame yet"
        }
    } else if card.preview.has_picture() {
        b"Not running \xc2\xb7 last frame"
    } else {
        b"Not running"
    }
}

fn build(state: &LauncherState, fingerprint: u64) -> Plan {
    let theme = THEME;
    let close_all = close_all_rect();
    let close_all_lbl_w = launcher::text::measure(b"Close all", type_scale::SECTION);
    let close_all_lbl_h = launcher::text::line_height(type_scale::SECTION);

    let mut plan = Plan {
        clock: if state.clock_source_ok && !state.clock_time.is_empty() {
            Some(Label::at(
                crate::screens::status::CLOCK_X,
                crate::screens::status::CLOCK_Y,
                crate::screens::status::CLOCK_SCALE,
                &state.clock_time,
                theme.on_surface,
            ))
        } else {
            None
        },
        title: Label::at(32, 62, type_scale::TITLE, b"Recents", theme.on_surface),
        count: Label::at(0, 0, type_scale::CAPTION, b"", theme.on_surface_variant),
        selector: [SelectorPlan {
            rect: Rect::new(0, 0, 0, 0),
            label: Label::EMPTY,
            dot_x: 0,
            dot_color: 0,
            selected: false,
        }; MAX_CARDS],
        selector_count: 0,
        card: None,
        focus: 0,
        preview: hero_preview_rect(),
        picture: None,
        preview_state: Preview::Unavailable("no task"),
        name: Label::EMPTY,
        status: Label::EMPTY,
        status_dot: (0, 0, 0),
        close: hero_close_rect(),
        close_all,
        close_all_label: Label::at(
            close_all.x + (close_all.w.saturating_sub(close_all_lbl_w)) / 2,
            close_all.y + (close_all.h.saturating_sub(close_all_lbl_h)) / 2,
            type_scale::SECTION,
            b"Close all",
            theme.on_primary,
        ),
        empty: Label::EMPTY,
        empty_hint: Label::EMPTY,
        cached_fingerprint: fingerprint,
    };

    let count = state.recents_count.min(MAX_CARDS);
    // WHY the running count asks the task ids: `Preview::Live` is only true
    // once a capture has landed, so a running app whose first frame has not
    // been read yet used to be reported as not running. The task id is the
    // platform's own answer to "is this app running".
    let running = state.recents_cards[..count]
        .iter()
        .filter(|card| card.task_id != 0)
        .count();
    let mut summary_buf = [0u8; 32];
    let summary = summary_label(count, running, &mut summary_buf);
    plan.count = Label::at(
        688 - launcher::text::measure(&summary.0[..summary.1], type_scale::CAPTION),
        74,
        type_scale::CAPTION,
        &summary.0[..summary.1],
        theme.on_surface_variant,
    );

    if count == 0 {
        plan.empty = Label::at(
            32,
            240,
            type_scale::TITLE,
            b"No recent apps",
            theme.on_surface,
        );
        plan.empty_hint = Label::at(
            32,
            310,
            type_scale::BODY,
            b"Open an app from Home or Drawer to see it here.",
            theme.on_surface_variant,
        );
        return plan;
    }

    let palette = if theme.dark {
        AccentPalette::VIBRANT_DARK
    } else {
        AccentPalette::VIBRANT_LIGHT
    };

    let focus = state.activity_focus.min(count - 1);
    plan.focus = focus;
    let label_y = SELECTOR_Y + (SELECTOR_H.saturating_sub(launcher::text::line_height(type_scale::BODY))) / 2;

    for index in 0..count {
        let card = &state.recents_cards[index];
        let rect = selector_rect(index);
        // The dot takes 22 px (6 px dot plus two 8 px margins), so the label
        // gets the rest of the chip and never runs under the rounded edge.
        let room = rect.w.saturating_sub(40);
        let (name, name_len) = launcher::text::ellipsize(card.label(), type_scale::BODY, room);
        plan.selector[index] = SelectorPlan {
            rect,
            label: Label::at(
                rect.x + 30,
                label_y,
                type_scale::BODY,
                &name[..name_len],
                if index == focus {
                    theme.on_primary
                } else {
                    theme.on_surface
                },
            ),
            dot_x: rect.x + 14,
            dot_color: state_color(card),
            selected: index == focus,
        };
    }
    plan.selector_count = count;

    let card = &state.recents_cards[focus];
    let pkg = card.package();
    let icon = icons::match_icon_kind(pkg);
    let accent_color = palette.for_app_bytes(pkg);

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

    let name_x = CARD_X + HERO_PAD + 64;
    let name_room = hero_close_rect().x.saturating_sub(name_x + 16);
    let (name, name_len) = launcher::text::ellipsize(card.label(), type_scale::SECTION, name_room);
    plan.name = Label::at(
        name_x,
        HERO_Y + 18,
        type_scale::SECTION,
        &name[..name_len],
        theme.on_surface,
    );
    plan.status = Label::at(
        name_x + 12,
        HERO_Y + 52,
        type_scale::CAPTION,
        status_text(card),
        theme.on_surface_variant,
    );
    plan.status_dot = (name_x + 1, HERO_Y + 61, state_color(card));
    plan.card = Some(CardPlan {
        icon_box: Rect::new(CARD_X + HERO_PAD, HERO_Y + 16, 48, 48),
        icon,
        accent_color,
        badge,
        badge_len,
    });
    plan.preview = hero_preview_rect();
    plan.picture = picture_slot(pkg);
    plan.preview_state = card.preview;
    plan
}

/// The header summary: how many recent apps, and how many of them the platform
/// says are running. A card with only a last frame is not counted as running.
fn summary_label(count: usize, running: usize, buf: &mut [u8; 32]) -> ([u8; 32], usize) {
    let mut out = [0u8; 32];
    let mut at = 0usize;
    put_num(&mut out, &mut at, count);
    put_bytes(&mut out, &mut at, if count == 1 { b" recent app" } else { b" recent apps" });
    if running > 0 {
        // WHY ASCII and not a UTF-8 middle dot: the bitmap face maps one byte
        // per glyph, so a two-byte sequence renders as two glyphs ("Â·") and
        // its measured advance no longer matches what is drawn.
        put_bytes(&mut out, &mut at, b", ");
        put_num(&mut out, &mut at, running);
        put_bytes(&mut out, &mut at, b" running");
    }
    *buf = out;
    (out, at)
}

fn put_bytes(buf: &mut [u8], at: &mut usize, bytes: &[u8]) {
    for &b in bytes {
        if *at < buf.len() {
            buf[*at] = b;
            *at += 1;
        }
    }
}

fn put_num(buf: &mut [u8], at: &mut usize, val: usize) {
    if val == 0 {
        put_bytes(buf, at, b"0");
        return;
    }
    let mut scratch = [0u8; 16];
    let mut n = 0;
    let mut v = val;
    while v > 0 && n < scratch.len() {
        scratch[n] = b'0' + (v % 10) as u8;
        v /= 10;
        n += 1;
    }
    for i in (0..n).rev() {
        put_bytes(buf, at, &[scratch[i]]);
    }
}

/// The colour of (`x`, `y`) on the task switcher.
pub fn get_activity_color(x: usize, y: usize, state: &LauncherState) -> u32 {
    let p = plan(x, y, state);
    let theme = THEME;

    // 1. The shell's canonical status bar.
    if y < STATUS_BAR_HEIGHT {
        return crate::screens::status::pixel(x, y, p.clock.as_ref(), &theme);
    }

    // 2. Header: title and the honest recent/running summary.
    if let Some(c) = p.title.pixel(x, y, theme.surface) {
        return c;
    }
    if let Some(c) = p.count.pixel(x, y, theme.surface) {
        return c;
    }

    // 3. Selector rail: one chip per recent app.
    for index in 0..p.selector_count {
        let chip = &p.selector[index];
        if !chip.rect.contains(x, y) {
            continue;
        }
        if !inside_rounded(chip.rect, 18, x, y) {
            return theme.surface;
        }
        let fill = if chip.selected {
            theme.on_surface
        } else {
            theme.surface_container
        };
        if !chip.selected && !inside_rounded(chip.rect.inset(1), 17, x, y) {
            return theme.border;
        }
        let dx = x as i32 - (chip.dot_x + 3) as i32;
        let dy = y as i32 - (SELECTOR_Y + SELECTOR_H / 2) as i32;
        if dx * dx + dy * dy <= 9 {
            return chip.dot_color;
        }
        if let Some(c) = chip.label.pixel(x, y, fill) {
            return c;
        }
        return fill;
    }

    // 4. The focused card.
    if let Some(card) = &p.card {
        let hero = hero_rect();
        if hero.contains(x, y) {
            if !inside_rounded(hero, 24, x, y) {
                return theme.surface;
            }
            if !inside_rounded(hero.inset(1), 23, x, y) {
                return theme.border;
            }
            let fill = theme.surface_container;

            // The preview well: the app's own pixels, or the reason there are
            // none. The well is fitted, never stretched, to the app's shape.
            if p.preview.contains(x, y) {
                if !inside_rounded(p.preview, 16, x, y) {
                    return fill;
                }
                if !inside_rounded(p.preview.inset(1), 15, x, y) {
                    return theme.border;
                }
                let px = ((x - p.preview.x) * recents::PREVIEW_W) / p.preview.w.max(1);
                let py = ((y - p.preview.y) * recents::PREVIEW_H) / p.preview.h.max(1);
                if let Some(slot) = p.picture {
                    if slot_has_captured_pixels(slot) {
                        if let Some(c) = picture_pixel(slot, px, py) {
                            if c != 0 {
                                return c;
                            }
                        }
                    }
                }
                return preview_placeholder(x, y, &p.preview, &theme, p.preview_state);
            }

            // The app's identity.
            if card.icon_box.contains(x, y) && inside_rounded(card.icon_box, 15, x, y) {
                if !inside_rounded(card.icon_box.inset(1), 14, x, y) {
                    return theme.border;
                }
                if let Some(c) = card_icon_glyph(x, y, card) {
                    return c;
                }
                return card.accent_color;
            }

            // The close control.
            if p.close.contains(x, y) {
                if let Some(c) = render_close_button(x, y, &p.close, &theme) {
                    return c;
                }
            }

            // The state dot, then the status line.
            let (dx0, dy0, color) = p.status_dot;
            let dx = x as i32 - (dx0 + 4) as i32;
            let dy = y as i32 - dy0 as i32;
            if dx * dx + dy * dy <= 16 {
                return color;
            }
            if let Some(c) = p.status.pixel(x, y, fill) {
                return c;
            }
            if let Some(c) = p.name.pixel(x, y, fill) {
                return c;
            }
            return fill;
        }
    }

    // 5. Empty state.
    if p.selector_count == 0 {
        if let Some(c) = p.empty.pixel(x, y, theme.surface) {
            return c;
        }
        if let Some(c) = p.empty_hint.pixel(x, y, theme.surface) {
            return c;
        }
    }

    // 6. Close-all pill.
    if p.selector_count > 0 && inside_rounded(p.close_all, 28, x, y) {
        let fill = theme.primary;
        if let Some(c) = p.close_all_label.pixel(x, y, fill) {
            return c;
        }
        return fill;
    }

    theme.surface
}

/// The captured pixel at (`px`, `py`) of the picture in `slot`.
pub(crate) fn picture_pixel(slot: usize, px: usize, py: usize) -> Option<u32> {
    if px >= recents::PREVIEW_W || py >= recents::PREVIEW_H {
        return None;
    }
    let slot_ptr: *const [[u32; PREVIEW_PIXELS]; MAX_CARDS] = core::ptr::addr_of!(PREVIEWS);
    let buffer = unsafe { &(*slot_ptr)[slot] };
    Some(buffer[py * recents::PREVIEW_W + px])
}

#[allow(dead_code)]
fn state_dot_color(kind: CardStateKind) -> u32 {
    match kind {
        CardStateKind::Active => PALETTE_LIGHT.phone,   // Vibrant Emerald Green
        CardStateKind::Running => 0xFF059669,  // Emerald Green
        CardStateKind::LastFrame => PALETTE_LIGHT.files, // Amber Orange
        CardStateKind::NotRunning => 0xFF94A3B8, // Slate Gray
    }
}

#[allow(dead_code)]
fn state_pill_bg(kind: CardStateKind) -> u32 {
    match kind {
        CardStateKind::Active => 0x2610B981,
        CardStateKind::Running => 0x20059669,
        CardStateKind::LastFrame => 0x22F59E0B,
        CardStateKind::NotRunning => 0x1894A3B8,
    }
}

#[allow(dead_code)]
fn state_pill_border(kind: CardStateKind) -> u32 {
    match kind {
        CardStateKind::Active => 0x4D10B981,
        CardStateKind::Running => 0x40059669,
        CardStateKind::LastFrame => 0x40F59E0B,
        CardStateKind::NotRunning => 0x3094A3B8,
    }
}

/// Vector glyph or proportional acronym badge inside the app icon squircle.
fn card_icon_glyph(x: usize, y: usize, card: &CardPlan) -> Option<u32> {
    let cx = card.icon_box.center_x();
    let cy = card.icon_box.center_y();
    if let Some(kind) = card.icon {
        return icons::rasterize_icon_pixel_scaled(kind, x, y, cx, cy, 0xFFFFFFFF, 15);
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
        0xFFFFFFFF,
        card.accent_color,
    )
}

/// Prominent circular close button (touch target >= 48px, circle radius 18px with crisp diagonal X).
fn render_close_button(x: usize, y: usize, rect: &Rect, theme: &Theme) -> Option<u32> {
    let cx = (rect.x + 24) as i32;
    let cy = (rect.y + 24) as i32;
    let dx = x as i32 - cx;
    let dy = y as i32 - cy;
    let d2 = dx * dx + dy * dy;

    // Circle radius: 18px (d2 <= 324)
    if d2 <= 324 {
        // Border ring
        if d2 >= 289 {
            return Some(theme.border);
        }
        // Crisp diagonal X cross
        if dx.abs() <= 6 && dy.abs() <= 6 && ((dx - dy).abs() <= 1 || (dx + dy).abs() <= 1) {
            return Some(theme.on_surface);
        }
        return Some(theme.surface_container_high);
    }
    None
}

/// Tests whether (`x`, `y`) is inside a rounded rectangle.
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

// =============================================================================
// Preview Placeholder (Platform Truth Contract)
// =============================================================================

/// The well a card shows when the platform has no picture to give.
///
/// WHY the reason is drawn and not a mock screen: a sketch of what the app
/// might look like is a picture of something that was never shown, and the
/// switcher's whole job is to answer "what was this app showing".
fn preview_placeholder(
    x: usize,
    y: usize,
    rect: &Rect,
    theme: &Theme,
    state: Preview,
) -> u32 {
    let fill = theme.surface_container_low;
    let rad = 16;
    if !inside_rounded(*rect, rad, x, y) {
        return theme.surface_container;
    }
    if !inside_rounded(rect.inset(1), rad.saturating_sub(1), x, y) {
        return theme.border;
    }

    let cx = rect.center_x();
    let cy = rect.center_y();

    // A single unbound-looking dot, not a fake app: it marks the well as
    // holding a status, not a picture.
    let dot_y = cy.saturating_sub(52);
    let dx = x as i32 - cx as i32;
    let dy = y as i32 - dot_y as i32;
    if dx * dx + dy * dy <= 36 {
        return theme.on_surface_variant;
    }

    let head = b"No preview";
    let head_w = launcher::text::measure(head, type_scale::BODY);
    let head_y = cy.saturating_sub(28);
    let head_label = Label::at(
        rect.x + (rect.w.saturating_sub(head_w)) / 2,
        head_y,
        type_scale::BODY,
        head,
        theme.on_surface,
    );
    if let Some(c) = head_label.pixel(x, y, fill) {
        return c;
    }

    let reason = state
        .reason()
        .unwrap_or("the app has not drawn a frame yet")
        .as_bytes();
    let (rbytes, rlen) =
        launcher::text::ellipsize(reason, type_scale::CAPTION, rect.w.saturating_sub(48));
    if rlen > 0 {
        let reason_w = launcher::text::measure(&rbytes[..rlen], type_scale::CAPTION);
        let reason_y = head_y + launcher::text::line_height(type_scale::BODY) + 8;
        let reason_label = Label::at(
            rect.x + (rect.w.saturating_sub(reason_w)) / 2,
            reason_y,
            type_scale::CAPTION,
            &rbytes[..rlen],
            theme.on_surface_variant,
        );
        if let Some(c) = reason_label.pixel(x, y, fill) {
            return c;
        }
    }
    fill
}
