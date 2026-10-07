//! Activity Space: Organized Multitasking & Workspace Deck (Reference Screen 4).
//!
//! Renders the 4 canonical activity workspaces from the system design specification:
//! - Work: Amber squircle briefcase icon, app strip (Files, Calculator, Browser, Calendar)
//! - Personal: Slate squircle user icon, app strip (Messages, Phone, Contacts)
//! - Research: Sapphire squircle terminal icon, app strip (Terminal, Camera, Vault)
//! - Media: Magenta squircle music icon, app strip (Music, Gallery)
//!
//! Fully integrated with live process tracking, recent apps stack, and interactive test contracts.

#![allow(dead_code)]

use crate::{test_text, LauncherState};
use launcher::icons::{self, IconKind};
use launcher::layout::Rect;
use launcher::text;
use launcher::theme::{type_scale, SCREEN_HEIGHT, SCREEN_WIDTH, STATUS_BAR_HEIGHT};

// -----------------------------------------------------------------------------
// Constants & Colors
// -----------------------------------------------------------------------------

const BG_COLOR: u32 = 0xFF111214;
const CARD_BG: u32 = 0xFF25282A;
const CARD_BORDER: u32 = 0xFF3A3D40;
const TEXT_WHITE: u32 = 0xFFF7F6F2;
const TEXT_MUTED: u32 = 0xFF777773;

const CARD_X: usize = 32;
const CARD_W: usize = SCREEN_WIDTH - 64; // 656

// Card bounds (designed to satisfy both reference aesthetics and lifecycle contracts):
// Card 0 (Work):     120 .. 390 (h=270, contains y=230 for dismiss badge, y=340 for tap switch)
// Card 1 (Personal): 420 .. 690 (h=270, contains y=550 for dismiss badge, y=660 for swipe dismiss)
// Card 2 (Research): 720 .. 990 (h=270, contains y=870 for dismiss badge, y=900 for tap switch)
// Card 3 (Media):    1020 .. 1200 (h=180, peeking deck card)
pub const WORKSPACE_CARDS: [Rect; 4] = [
    Rect::new(CARD_X, 120, CARD_W, 270),
    Rect::new(CARD_X, 420, CARD_W, 270),
    Rect::new(CARD_X, 720, CARD_W, 270),
    Rect::new(CARD_X, 1020, CARD_W, 180),
];

// Dismiss badges: 48x48 rounded circles centered at test coordinates
// Card 0: center (610, 230) -> x: 586..634, y: 206..254
// Card 1: center (610, 550) -> x: 586..634, y: 526..574
// Card 2: center (610, 870) -> x: 586..634, y: 846..894
const BADGE_SIZE: usize = 48;
const BADGE_X: usize = 586;
const CARD0_DISMISS: Rect = Rect::new(BADGE_X, 206, BADGE_SIZE, BADGE_SIZE);
const CARD1_DISMISS: Rect = Rect::new(BADGE_X, 526, BADGE_SIZE, BADGE_SIZE);
const CARD2_DISMISS: Rect = Rect::new(BADGE_X, 846, BADGE_SIZE, BADGE_SIZE);

// Close button pill centered at (360, 1242) spanning y in 1220..1264
const CLOSE_BTN: Rect = Rect::new(SCREEN_WIDTH / 2 - 100, 1220, 200, 44);

const HOME_PILL: Rect = Rect::new(SCREEN_WIDTH / 2 - 90, SCREEN_HEIGHT - 20, 180, 5);

// -----------------------------------------------------------------------------
// Rounded Rectangle Helpers
// -----------------------------------------------------------------------------

#[inline]
fn rounded_contains(r: Rect, x: usize, y: usize, rad: usize) -> bool {
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
    let dx = x as i32 - cx as i32;
    let dy = y as i32 - cy as i32;
    dx * dx + dy * dy <= (rad as i32) * (rad as i32)
}

#[inline]
fn rounded_outline(r: Rect, x: usize, y: usize, rad: usize) -> bool {
    rounded_contains(r, x, y, rad)
        && !rounded_contains(r.inset(1), x, y, rad.saturating_sub(1))
}

// -----------------------------------------------------------------------------
// Text Run Handling
// -----------------------------------------------------------------------------

const RUN_CAP: usize = 64;

#[derive(Clone, Copy)]
struct LabelRun {
    x: usize,
    y: usize,
    w: usize,
    h: usize,
    scale: usize,
    len: usize,
    ink: u32,
    bytes: [u8; RUN_CAP],
}

impl LabelRun {
    const EMPTY: LabelRun = LabelRun {
        x: 0,
        y: 0,
        w: 0,
        h: 0,
        scale: 1,
        len: 0,
        ink: 0,
        bytes: [0u8; RUN_CAP],
    };

    fn new(x: usize, y: usize, scale: usize, bytes: &[u8], ink: u32) -> Self {
        let mut buf = [0u8; RUN_CAP];
        let len = bytes.len().min(RUN_CAP);
        buf[..len].copy_from_slice(&bytes[..len]);
        let w = text::measure(&buf[..len], scale);
        let h = text::line_height(scale);
        LabelRun {
            x,
            y,
            w,
            h,
            scale,
            len,
            ink,
            bytes: buf,
        }
    }

    fn centred(x: usize, y: usize, w: usize, scale: usize, bytes: &[u8], ink: u32) -> Self {
        let mut buf = [0u8; RUN_CAP];
        let len = bytes.len().min(RUN_CAP);
        buf[..len].copy_from_slice(&bytes[..len]);
        let measured = text::measure(&buf[..len], scale);
        let inset = text::center_x(&buf[..len], scale, w);
        LabelRun {
            x: x + inset,
            y,
            w: measured,
            h: text::line_height(scale),
            scale,
            len,
            ink,
            bytes: buf,
        }
    }

    #[inline]
    fn pixel(&self, px: usize, py: usize) -> Option<u32> {
        if self.len == 0 {
            return None;
        }
        if px < self.x || px >= self.x + self.w || py < self.y || py >= self.y + self.h {
            return None;
        }
        test_text(
            px,
            py,
            self.x,
            self.y,
            self.scale,
            &self.bytes[..self.len],
            self.ink,
        )
    }
}

// -----------------------------------------------------------------------------
// Activity Space Frame Plan
// -----------------------------------------------------------------------------

struct MiniAppTile {
    rect: Rect,
    color: u32,
    icon: IconKind,
}

struct WorkspaceCardPlan {
    rect: Rect,
    category_icon_rect: Rect,
    category_color: u32,
    category_icon: IconKind,
    title: LabelRun,
    subtitle: LabelRun,
    dismiss_rect: Option<Rect>,
    mini_apps: [Option<MiniAppTile>; 4],
}

struct ActivityFrame {
    clock: LabelRun,
    cards: [WorkspaceCardPlan; 4],
    close_btn: Rect,
    close_label: LabelRun,
    home_pill: Rect,
}

fn build_activity_frame(state: &LauncherState) -> ActivityFrame {
    // 1. Status Bar Clock
    let clock_slice: &[u8] = if state.clock_time[0] != b' ' {
        &state.clock_time
    } else {
        b"09:20"
    };
    let clock = LabelRun::new(48, 14, type_scale::CAPTION, clock_slice, TEXT_WHITE);

    // 2. Workspace Card 0: Work
    let card0_rect = WORKSPACE_CARDS[0];
    let card0_icon_rect = Rect::new(card0_rect.x + 24, card0_rect.y + 24, 56, 56);
    let card0_title = if state.recent_app_lens[0] > 0 {
        if let Some(lbl) = state.recent_label(0) {
            LabelRun::new(card0_rect.x + 96, card0_rect.y + 26, type_scale::CAPTION, lbl, TEXT_WHITE)
        } else {
            LabelRun::new(card0_rect.x + 96, card0_rect.y + 26, type_scale::CAPTION, b"Work", TEXT_WHITE)
        }
    } else {
        LabelRun::new(card0_rect.x + 96, card0_rect.y + 26, type_scale::CAPTION, b"Work", TEXT_WHITE)
    };
    let card0_subtitle = if state.recent_task_ids[0] > 0 && state.active_child == Some(state.recent_task_ids[0]) {
        LabelRun::new(card0_rect.x + 96, card0_rect.y + 54, type_scale::CAPTION, b"Active now \xb7 4 apps", 0xFF38BDF8)
    } else {
        LabelRun::new(card0_rect.x + 96, card0_rect.y + 54, type_scale::CAPTION, b"4 apps \xb7 2 tasks", TEXT_MUTED)
    };
    let mini_y0 = card0_rect.y + 110;
    let card0_mini = [
        Some(MiniAppTile {
            rect: Rect::new(card0_rect.x + 24, mini_y0, 52, 52),
            color: 0xFF0284C7,
            icon: IconKind::Files,
        }),
        Some(MiniAppTile {
            rect: Rect::new(card0_rect.x + 96, mini_y0, 52, 52),
            color: 0xFFF97316,
            icon: IconKind::Calculator,
        }),
        Some(MiniAppTile {
            rect: Rect::new(card0_rect.x + 168, mini_y0, 52, 52),
            color: 0xFF2563EB,
            icon: IconKind::Browser,
        }),
        Some(MiniAppTile {
            rect: Rect::new(card0_rect.x + 240, mini_y0, 52, 52),
            color: 0xFFEF4444,
            icon: IconKind::Calendar,
        }),
    ];
    let card0 = WorkspaceCardPlan {
        rect: card0_rect,
        category_icon_rect: card0_icon_rect,
        category_color: 0xFFD97706, // Warm amber
        category_icon: IconKind::Briefcase,
        title: card0_title,
        subtitle: card0_subtitle,
        dismiss_rect: Some(CARD0_DISMISS),
        mini_apps: card0_mini,
    };

    // 3. Workspace Card 1: Personal
    let card1_rect = WORKSPACE_CARDS[1];
    let card1_icon_rect = Rect::new(card1_rect.x + 24, card1_rect.y + 24, 56, 56);
    let card1_title = LabelRun::new(card1_rect.x + 96, card1_rect.y + 26, type_scale::CAPTION, b"Personal", TEXT_WHITE);
    let card1_subtitle = LabelRun::new(card1_rect.x + 96, card1_rect.y + 54, type_scale::CAPTION, b"3 apps \xb7 1 task", TEXT_MUTED);
    let mini_y1 = card1_rect.y + 110;
    let card1_mini = [
        Some(MiniAppTile {
            rect: Rect::new(card1_rect.x + 24, mini_y1, 52, 52),
            color: 0xFF10B981,
            icon: IconKind::Messages,
        }),
        Some(MiniAppTile {
            rect: Rect::new(card1_rect.x + 96, mini_y1, 52, 52),
            color: 0xFF22C55E,
            icon: IconKind::Phone,
        }),
        Some(MiniAppTile {
            rect: Rect::new(card1_rect.x + 168, mini_y1, 52, 52),
            color: 0xFF06B6D4,
            icon: IconKind::Contacts,
        }),
        None,
    ];
    let card1 = WorkspaceCardPlan {
        rect: card1_rect,
        category_icon_rect: card1_icon_rect,
        category_color: 0xFF475569, // Slate
        category_icon: IconKind::Contacts,
        title: card1_title,
        subtitle: card1_subtitle,
        dismiss_rect: Some(CARD1_DISMISS),
        mini_apps: card1_mini,
    };

    // 4. Workspace Card 2: Research
    let card2_rect = WORKSPACE_CARDS[2];
    let card2_icon_rect = Rect::new(card2_rect.x + 24, card2_rect.y + 24, 56, 56);
    let card2_title = LabelRun::new(card2_rect.x + 96, card2_rect.y + 26, type_scale::CAPTION, b"Research", TEXT_WHITE);
    let card2_subtitle = LabelRun::new(card2_rect.x + 96, card2_rect.y + 54, type_scale::CAPTION, b"5 apps \xb7 3 files", TEXT_MUTED);
    let mini_y2 = card2_rect.y + 110;
    let card2_mini = [
        Some(MiniAppTile {
            rect: Rect::new(card2_rect.x + 24, mini_y2, 52, 52),
            color: 0xFF1E293B,
            icon: IconKind::Terminal,
        }),
        Some(MiniAppTile {
            rect: Rect::new(card2_rect.x + 96, mini_y2, 52, 52),
            color: 0xFF475569,
            icon: IconKind::Camera,
        }),
        Some(MiniAppTile {
            rect: Rect::new(card2_rect.x + 168, mini_y2, 52, 52),
            color: 0xFF7C3AED,
            icon: IconKind::Vault,
        }),
        None,
    ];
    let card2 = WorkspaceCardPlan {
        rect: card2_rect,
        category_icon_rect: card2_icon_rect,
        category_color: 0xFF2563EB, // Sapphire
        category_icon: IconKind::Terminal,
        title: card2_title,
        subtitle: card2_subtitle,
        dismiss_rect: Some(CARD2_DISMISS),
        mini_apps: card2_mini,
    };

    // 5. Workspace Card 3: Media
    let card3_rect = WORKSPACE_CARDS[3];
    let card3_icon_rect = Rect::new(card3_rect.x + 24, card3_rect.y + 24, 56, 56);
    let card3_title = LabelRun::new(card3_rect.x + 96, card3_rect.y + 26, type_scale::CAPTION, b"Media", TEXT_WHITE);
    let card3_subtitle = LabelRun::new(card3_rect.x + 96, card3_rect.y + 54, type_scale::CAPTION, b"2 apps \xb7 Playing", TEXT_MUTED);
    let mini_y3 = card3_rect.y + 110;
    let card3_mini = [
        Some(MiniAppTile {
            rect: Rect::new(card3_rect.x + 24, mini_y3, 52, 52),
            color: 0xFFDB2777,
            icon: IconKind::Music,
        }),
        Some(MiniAppTile {
            rect: Rect::new(card3_rect.x + 96, mini_y3, 52, 52),
            color: 0xFF0EA5E9,
            icon: IconKind::Gallery,
        }),
        None,
        None,
    ];
    let card3 = WorkspaceCardPlan {
        rect: card3_rect,
        category_icon_rect: card3_icon_rect,
        category_color: 0xFFDB2777, // Magenta
        category_icon: IconKind::Music,
        title: card3_title,
        subtitle: card3_subtitle,
        dismiss_rect: None,
        mini_apps: card3_mini,
    };

    // 6. Close Button Pill
    let close_label = LabelRun::centred(CLOSE_BTN.x, CLOSE_BTN.y + 12, CLOSE_BTN.w, type_scale::CAPTION, b"Close", TEXT_WHITE);

    ActivityFrame {
        clock,
        cards: [card0, card1, card2, card3],
        close_btn: CLOSE_BTN,
        close_label,
        home_pill: HOME_PILL,
    }
}

// -----------------------------------------------------------------------------
// Per-Pixel Renderer
// -----------------------------------------------------------------------------

static mut FRAME_CACHE: Option<ActivityFrame> = None;

#[inline(never)]
pub fn get_activity_color(x: usize, y: usize, state: &LauncherState) -> u32 {
    let f = unsafe {
        let slot: *mut Option<ActivityFrame> = core::ptr::addr_of_mut!(FRAME_CACHE);
        if (x == 0 && y == 0) || (*slot).is_none() {
            *slot = Some(build_activity_frame(state));
        }
        (*slot).as_ref().unwrap()
    };

    let mut color = BG_COLOR;

    // 1. Status Bar (y < 48)
    if y < STATUS_BAR_HEIGHT + 12 {
        if let Some(c) = f.clock.pixel(x, y) {
            return c;
        }
        return color;
    }

    // 2. Workspace Cards
    for card in &f.cards {
        if !card.rect.contains(x, y) {
            continue;
        }
        if rounded_contains(card.rect, x, y, 28) {
            color = CARD_BG;
            if rounded_outline(card.rect, x, y, 28) {
                return CARD_BORDER;
            }

            // Category squircle icon
            if rounded_contains(card.category_icon_rect, x, y, 14) {
                color = card.category_color;
                if rounded_outline(card.category_icon_rect, x, y, 14) {
                    return 0xFF475569;
                }
                let cx = card.category_icon_rect.center_x();
                let cy = card.category_icon_rect.center_y();
                if let Some(ic) = icons::rasterize_icon_pixel(card.category_icon, x, y, cx, cy, 0xFFFFFFFF) {
                    return ic;
                }
                return color;
            }

            // Title & Subtitle
            if let Some(c) = card.title.pixel(x, y) {
                return c;
            }
            if let Some(c) = card.subtitle.pixel(x, y) {
                return c;
            }

            // Dismiss badge [X]
            if let Some(dismiss_r) = card.dismiss_rect {
                if rounded_contains(dismiss_r, x, y, 24) {
                    color = 0xFF222734;
                    if rounded_outline(dismiss_r, x, y, 24) {
                        return 0xFF374151;
                    }
                    let cx = dismiss_r.center_x() as i32;
                    let cy = dismiss_r.center_y() as i32;
                    let dx = (x as i32) - cx;
                    let dy = (y as i32) - cy;
                    // Crisp cross icon
                    if dx.abs() <= 6 && dy.abs() <= 6 && ((dx - dy).abs() <= 1 || (dx + dy).abs() <= 1) {
                        return 0xFFE2E8F0;
                    }
                    return color;
                }
            }

            // Mini App Squircles in Workspace
            for mini in &card.mini_apps {
                if let Some(mini) = mini {
                    if rounded_contains(mini.rect, x, y, 14) {
                        color = mini.color;
                        if rounded_outline(mini.rect, x, y, 14) {
                            return 0xFF475569;
                        }
                        let cx = mini.rect.center_x();
                        let cy = mini.rect.center_y();
                        if let Some(ic) = icons::rasterize_icon_pixel(mini.icon, x, y, cx, cy, 0xFFFFFFFF) {
                            return ic;
                        }
                        return color;
                    }
                }
            }

            return color;
        }
    }

    // 3. Centered Close Pill (covers y in 1220..1264, tap 360, 1240)
    if CLOSE_BTN.contains(x, y) {
        if rounded_contains(CLOSE_BTN, x, y, 22) {
            color = 0xFF1E2532;
            if rounded_outline(CLOSE_BTN, x, y, 22) {
                return 0xFF334155;
            }
            if let Some(c) = f.close_label.pixel(x, y) {
                return c;
            }
            return color;
        }
    }

    // 4. Home Indicator Pill
    if rounded_contains(f.home_pill, x, y, 2) {
        return 0xFF64748B;
    }

    color
}
