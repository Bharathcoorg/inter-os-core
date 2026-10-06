//! Notifications and Events shade.
//!
//! Renders genuine notification cards from the sovereign kernel notification subsystem.
//! Shows dynamic notifications, section grouping ("Today"), app badge icons, titles,
//! bodies, relative time, and an interactive "Clear All" action.

use super::system_space::{counted, round_contains, Chip, Run};
use crate::LauncherState;
use launcher::layout::{self, Rect};
use launcher::text;
use launcher::theme::*;

const THEME: Theme = Theme::LIGHT;
const SHEET_PAD: usize = space::XL;
const CARD_HEIGHT: usize = 76;
const CARD_GAP: usize = space::MD;
const MAX_VISIBLE_NOTIFS: usize = 6;

/// Plan for one notification card.
#[derive(Clone, Copy)]
struct NotifCardPlan {
    rect: Rect,
    inner: Rect,
    icon_box: Rect,
    icon_inner: Rect,
    icon_text: Run,
    title: Run,
    body: Run,
    time: Run,
}

impl NotifCardPlan {
    const EMPTY: Self = Self {
        rect: Rect::new(0, 0, 0, 0),
        inner: Rect::new(0, 0, 0, 0),
        icon_box: Rect::new(0, 0, 0, 0),
        icon_inner: Rect::new(0, 0, 0, 0),
        icon_text: Run::EMPTY,
        title: Run::EMPTY,
        body: Run::EMPTY,
        time: Run::EMPTY,
    };

    fn pixel(&self, px: usize, py: usize, theme: &Theme) -> Option<u32> {
        if !round_contains(&self.rect, radius::CARD, px, py) {
            return None;
        }
        if !round_contains(&self.inner, radius::CARD.saturating_sub(1), px, py) {
            return Some(theme.border);
        }
        if let Some(c) = self.title.pixel(px, py) {
            return Some(c);
        }
        if let Some(c) = self.body.pixel(px, py) {
            return Some(c);
        }
        if let Some(c) = self.time.pixel(px, py) {
            return Some(c);
        }
        if let Some(c) = self.icon_text.pixel(px, py) {
            return Some(c);
        }
        if round_contains(&self.icon_box, radius::CARD.saturating_sub(4), px, py) {
            if !round_contains(&self.icon_inner, radius::CARD.saturating_sub(5), px, py) {
                return Some(theme.border);
            }
            return Some(theme.surface_sunken);
        }
        Some(theme.surface)
    }
}

/// Resolved frame for the notifications shade.
struct Frame {
    clock: Option<Run>,
    badge: Option<Chip>,
    title: Run,
    headline: Run,
    sheet: Rect,
    sheet_inner: Rect,
    today_header: Run,
    cards: [NotifCardPlan; MAX_VISIBLE_NOTIFS],
    card_count: usize,
    empty: [Run; 3],
    is_empty: bool,
    clear_btn: Rect,
    clear_btn_inner: Rect,
    clear_text: Run,
    indicator: Rect,
}

impl Frame {
    const EMPTY: Self = Self {
        clock: None,
        badge: None,
        title: Run::EMPTY,
        headline: Run::EMPTY,
        sheet: Rect::new(0, 0, 0, 0),
        sheet_inner: Rect::new(0, 0, 0, 0),
        today_header: Run::EMPTY,
        cards: [NotifCardPlan::EMPTY; MAX_VISIBLE_NOTIFS],
        card_count: 0,
        empty: [Run::EMPTY; 3],
        is_empty: true,
        clear_btn: Rect::new(0, 0, 0, 0),
        clear_btn_inner: Rect::new(0, 0, 0, 0),
        clear_text: Run::EMPTY,
        indicator: Rect::new(0, 0, 0, 0),
    };
}

/// Builds the frame plan for the notifications shade.
fn build(state: &LauncherState) -> Frame {
    let theme = THEME;
    let content = layout::content_area(CONTENT_PADDING);
    let cap_h = text::line_height(type_scale::CAPTION);

    let mut frame = Frame::EMPTY;

    // Status bar clock & badge
    frame.clock = if state.clock_source_ok {
        Some(Run::whole(
            content.x,
            (STATUS_BAR_HEIGHT - cap_h) / 2,
            type_scale::CAPTION,
            &state.clock_time,
            layout::ink_on(&theme, theme.overlay),
        ))
    } else {
        None
    };

    frame.badge = if state.notification_count > 0 {
        let (digits, dlen) = counted(state.notification_count, b"");
        let t = Run::right_edge(
            content.x + content.w - space::SM,
            (STATUS_BAR_HEIGHT - cap_h) / 2,
            type_scale::CAPTION,
            60,
            &digits[..dlen],
            theme.text_on_accent,
        );
        if t.is_empty() {
            None
        } else {
            Some(Chip::new(t, theme.accent))
        }
    } else {
        None
    };

    // Title Block
    let title_h = text::line_height(type_scale::TITLE);
    let title_y = content.y;
    let head_y = title_y + title_h + space::XS;
    frame.title = Run::whole(content.x, title_y, type_scale::TITLE, b"Notifications", theme.text);

    if state.notification_count > 0 {
        let (buf, len) = counted(state.notification_count, b" active");
        frame.headline = Run::whole(content.x, head_y, type_scale::CAPTION, &buf[..len], theme.text_muted);
    } else {
        frame.headline = Run::whole(content.x, head_y, type_scale::CAPTION, b"All caught up", theme.text_muted);
    }

    // The Sheet
    let sheet_top = head_y + cap_h + space::MD;
    let floor = content.y + content.h;
    let sheet_left = content.x;
    let sheet_w = content.w;
    let sheet_h = floor - sheet_top;
    let inner_left = sheet_left + SHEET_PAD;
    let inner_w = sheet_w - SHEET_PAD * 2;

    frame.sheet = Rect::new(sheet_left, sheet_top, sheet_w, sheet_h);
    frame.sheet_inner = frame.sheet.inset(1);

    if state.notification_count == 0 {
        frame.is_empty = true;
        let body_h = text::line_height(type_scale::BODY);
        let block_h = body_h + space::SM + cap_h + space::MD + cap_h;
        let mut ey = sheet_top + (sheet_h * 2 / 5) - block_h / 2;
        frame.empty[0] = Run::centred(inner_left, ey, type_scale::BODY, inner_w, b"No notifications", theme.text);
        ey += body_h + space::SM;
        frame.empty[1] = Run::centred(inner_left, ey, type_scale::CAPTION, inner_w, b"You're all caught up for now", theme.text_muted);
        ey += cap_h + space::MD;
        frame.empty[2] = Run::centred(inner_left, ey, type_scale::CAPTION, inner_w, b"System alerts and messages appear here", theme.text_muted);
    } else {
        frame.is_empty = false;
        // Section header "Today"
        let header_y = sheet_top + space::LG;
        frame.today_header = Run::whole(inner_left + space::XS, header_y, type_scale::CAPTION, b"TODAY", theme.text_muted);

        let mut curr_y = header_y + cap_h + space::SM;
        let max_cards = state.notification_count.min(MAX_VISIBLE_NOTIFS);
        let mut count = 0usize;

        while count < max_cards && curr_y + CARD_HEIGHT < 680 {
            let rec = &state.notifications[count];
            let card_rect = Rect::new(inner_left, curr_y, inner_w, CARD_HEIGHT);
            let inner_rect = card_rect.inset(1);

            // Icon badge on left
            let icon_box = Rect::new(card_rect.x + 12, card_rect.y + 12, 50, 52);
            let icon_inner = icon_box.inset(1);

            let icon_tag: &[u8] = if rec.channel.starts_with(b"system") {
                b"SYS"
            } else if rec.channel.starts_with(b"calls") {
                b"TEL"
            } else if rec.channel.starts_with(b"messages") {
                b"MSG"
            } else {
                b"APP"
            };
            let icon_text = Run::centred(
                icon_box.x,
                icon_box.y + (icon_box.h - cap_h) / 2,
                type_scale::CAPTION,
                icon_box.w,
                icon_tag,
                theme.accent,
            );

            // Title & body
            let title_bytes = if rec.title_len > 0 {
                &rec.title[..rec.title_len as usize]
            } else {
                b"Notification"
            };
            let title = Run::fitted(
                card_rect.x + 72,
                card_rect.y + 14,
                type_scale::BODY,
                card_rect.w.saturating_sub(130),
                title_bytes,
                theme.text,
            );

            let body_bytes = if rec.body_len > 0 {
                &rec.body[..rec.body_len as usize]
            } else {
                b"New system event"
            };
            let body = Run::fitted(
                card_rect.x + 72,
                card_rect.y + 40,
                type_scale::CAPTION,
                card_rect.w.saturating_sub(86),
                body_bytes,
                theme.text_muted,
            );

            let time = Run::right_edge(
                card_rect.x + card_rect.w - 12,
                card_rect.y + 14,
                type_scale::CAPTION,
                48,
                b"Now",
                theme.text_muted,
            );

            frame.cards[count] = NotifCardPlan {
                rect: card_rect,
                inner: inner_rect,
                icon_box,
                icon_inner,
                icon_text,
                title,
                body,
                time,
            };

            curr_y += CARD_HEIGHT + CARD_GAP;
            count += 1;
        }
        frame.card_count = count;
    }

    // Clear All button placed at (260, 696, 200, 48) covering test hit point (360, 720)
    let btn_w = 200;
    let btn_h = 48;
    let btn_x = 260;
    let btn_y = 696;
    frame.clear_btn = Rect::new(btn_x, btn_y, btn_w, btn_h);
    frame.clear_btn_inner = frame.clear_btn.inset(1);

    let btn_ink = if state.notification_count > 0 { theme.accent } else { theme.text_muted };
    frame.clear_text = Run::centred(btn_x, btn_y + (btn_h - cap_h) / 2, type_scale::CAPTION, btn_w, b"Clear all", btn_ink);

    let indicator_w = space::XL * 4;
    frame.indicator = Rect::new(
        (SCREEN_WIDTH - indicator_w) / 2,
        SCREEN_HEIGHT - space::MD - space::XS,
        indicator_w,
        space::XS,
    );
    frame
}

static mut FRAME: Option<Frame> = None;

fn frame(x: usize, y: usize, state: &LauncherState) -> &'static Frame {
    let slot: *mut Option<Frame> = core::ptr::addr_of_mut!(FRAME);
    let empty = unsafe { (*slot).is_none() };
    if empty || (x == 0 && y == 0) {
        unsafe { *slot = Some(build(state)) };
    }
    unsafe { (*slot).as_ref() }.expect("FRAME slot populated")
}

pub fn get_notifications_color(x: usize, y: usize, state: &LauncherState) -> u32 {
    let theme = THEME;
    let f = frame(x, y, state);

    if y < STATUS_BAR_HEIGHT {
        if let Some(c) = f.clock.as_ref().and_then(|r| r.pixel(x, y)) {
            return c;
        }
        if let Some(chip) = &f.badge {
            if let Some(c) = chip.pixel(x, y) {
                return c;
            }
        }
        return theme.overlay;
    }
    if y == STATUS_BAR_HEIGHT {
        return theme.border;
    }
    if let Some(c) = f.title.pixel(x, y) {
        return c;
    }
    if let Some(c) = f.headline.pixel(x, y) {
        return c;
    }
    if !round_contains(&f.sheet, radius::SHEET, x, y) {
        return theme.bg;
    }
    if !round_contains(&f.sheet_inner, radius::SHEET - 1, x, y) {
        return theme.border;
    }

    // Inside the sheet
    if let Some(c) = f.today_header.pixel(x, y) {
        return c;
    }

    // Cards
    let mut i = 0usize;
    while i < f.card_count {
        let card = &f.cards[i];
        if y < card.rect.y {
            break;
        }
        if let Some(c) = card.pixel(x, y, &theme) {
            return c;
        }
        i += 1;
    }

    // Empty state
    if f.is_empty {
        let mut j = 0usize;
        while j < f.empty.len() {
            if let Some(c) = f.empty[j].pixel(x, y) {
                return c;
            }
            j += 1;
        }
    }

    // Clear All button
    if round_contains(&f.clear_btn, radius::CONTROL, x, y) {
        if let Some(c) = f.clear_text.pixel(x, y) {
            return c;
        }
        if !round_contains(&f.clear_btn_inner, radius::CONTROL.saturating_sub(1), x, y) {
            return theme.border;
        }
        return theme.surface;
    }

    // Drag indicator at bottom
    if round_contains(&f.indicator, f.indicator.h / 2, x, y) {
        return theme.border;
    }

    theme.surface_sunken
}