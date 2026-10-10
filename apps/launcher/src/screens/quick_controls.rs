//! Quick Controls Shade (Reference Screen 7).
//!
//! Renders the canonical "Quick Controls — Essential controls" panel from design specification:
//! - 2x2 Network & Connectivity Tiles (Wi-Fi, Bluetooth, Mobile, Airplane)
//! - Dual Interactive Slider Capsules (Brightness & Volume with vector icons)
//! - Quick Toggles Row (Torch, Orientation Lock, DND, Location)
//! - Integrated Power & Safety quick button at (408, 644, 272, 72) matching main.rs
//! - Bottom home indicator pill

#![allow(dead_code)]

use crate::{test_text, LauncherState};
use interos_std as std;
use launcher::layout::Rect;
use launcher::text;
use launcher::theme::{type_scale, SCREEN_HEIGHT, SCREEN_WIDTH, STATUS_BAR_HEIGHT};

// -----------------------------------------------------------------------------
// Constants & Colors
// -----------------------------------------------------------------------------

const BG_COLOR: u32 = 0xFF111214;
const TILE_BG: u32 = 0xFF25282A;
const TILE_BORDER: u32 = 0xFF3A3D40;
const TEXT_WHITE: u32 = 0xFFF7F6F2;
const TEXT_MUTED: u32 = 0xFF777773;

const SLIDER_BG: u32 = 0xFF35393D;
const SLIDER_FILL: u32 = 0xFFFFFFFF;

/// Width of a slider's position indicator, and how far in from each end of the
/// capsule it stops.
///
/// WHY it is drawn at all: a capsule that is filled white from end to end is the
/// same drawing as 100 %. Without a mark at the fill's end there is nothing on
/// the control that says *where* the level is, so a filled bar read as a maximum
/// rather than as a position.
const SLIDER_THUMB_W: usize = 8;
const SLIDER_THUMB_INSET: usize = 14;

/// Where a slider's caption starts, and the width it may occupy.
///
/// WHY it clears the vector icon: the sun and the speaker are centred 44 px in and
/// reach 16 px past that, so anything set at the capsule's own left edge is drawn
/// underneath them.
const SLIDER_CAPTION_X: usize = 80;
const SLIDER_CAPTION_INSET: usize = 16;

/// Height of a quick-toggle button.
///
/// WHY it is 96 and not 72: at 72 the button is exactly twice its corner radius,
/// so it has no straight middle band at all -- it is a disc -- and a 24-row line
/// of type cannot be centred inside a 28-row disc. The line was hung off the
/// bottom instead (`toggle.y + 54`), which put its cell at y 698..722 while the
/// button's own guard stops at 716, so the last five rows of every glyph were
/// discarded and the label read as a sliced one. At 96 the pill's straight band is
/// `top..top + line_height`, which is exactly one line box, so a line centred in
/// the button is also a line that is entirely inside it.
const TOGGLE_H: usize = 96;

/// Where the toggle's icon sits inside the button.
///
/// WHY it is above the label and not on top of it: the icon's disc is 28 rows
/// across and the label is 24, and 96 - 24 = 72 of rows sit above the label's
/// centre line. The icon takes the upper 28 of them, leaving a gap before the
/// label's ink starts.
const TOGGLE_ICON_DY: usize = 22;

const POWER_BTN: Rect = Rect::new(408, 644, 272, 72);
const HOME_PILL: Rect = Rect::new(SCREEN_WIDTH / 2 - 90, SCREEN_HEIGHT - 20, 180, 5);

// -----------------------------------------------------------------------------
// Rounded Geometry Helpers
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
// Typography Helpers
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

    /// A run truncated with an ellipsis if it is wider than `fit`.
    ///
    /// WHY this exists: `new` copies up to `RUN_CAP` bytes and draws all of them,
    /// so a long registry string ran off the card it was centred in. `pixel` only
    /// rejects pixels outside the *measured* box, and that box is however wide the
    /// string turned out to be, so nothing on screen caught it.
    fn fitted(x: usize, y: usize, scale: usize, fit: usize, bytes: &[u8], ink: u32) -> Self {
        let (buf, len) = text::ellipsize(bytes, scale, fit);
        let n = len.min(RUN_CAP);
        let mut held = [0u8; RUN_CAP];
        held[..n].copy_from_slice(&buf[..n]);
        LabelRun {
            x,
            y,
            w: text::measure(&held[..n], scale),
            h: text::line_height(scale),
            scale,
            len: n,
            ink,
            bytes: held,
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
// Quick Controls Plan
// -----------------------------------------------------------------------------

#[derive(Clone, Copy)]
enum QcIcon {
    Wifi,
    Bluetooth,
    Mobile,
    Airplane,
    Torch,
    Rotate,
    Moon,
    Power,
}

#[derive(Clone, Copy)]
struct ControlTilePlan {
    rect: Rect,
    icon: QcIcon,
    title: LabelRun,
    subtitle: LabelRun,
}

struct QuickControlsFrame {
    clock: LabelRun,
    tiles: [ControlTilePlan; 4],
    brightness_rect: Rect,
    brightness_fill_w: usize,
    brightness_available: bool,
    brightness_caption: LabelRun,
    volume_rect: Rect,
    volume_fill_w: usize,
    volume_available: bool,
    volume_thumb: Rect,
    volume_caption: LabelRun,
    toggles: [Rect; 4],
    toggle_icons: [QcIcon; 4],
    toggle_labels: [LabelRun; 4],
    power_btn: Rect,
    power_label: LabelRun,
    home_pill: Rect,
}

fn build_qc_frame(state: &LauncherState) -> QuickControlsFrame {
    // WHY the clock is gated on the source flag and not on the buffer: with no RTC
    // authority granted to this shell the read can be refused, and the buffer keeps
    // whatever it last held. `notifications.rs` and `home.rs` both gate on the same
    // flag, so a refused read leaves this status bar empty rather than showing a
    // stale value, and it is the same condition the row below the clock is drawn
    // under.
    let clock_slice: &[u8] = if state.clock_source_ok && state.clock_time[0] != b' ' {
        &state.clock_time
    } else {
        b""
    };
    let clock = LabelRun::new(48, 14, type_scale::CAPTION, clock_slice, TEXT_WHITE);
    let wifi = match std::settings_hardware_status("wifi") { Ok(1) => b"Virtual".as_slice(), _ => b"Unavailable".as_slice() };
    let bluetooth = match std::settings_hardware_status("bluetooth") { Ok(1) => b"Virtual".as_slice(), _ => b"Unavailable".as_slice() };
    let mobile = match std::settings_hardware_status("cellular") { Ok(1) => b"Virtual".as_slice(), _ => b"Unavailable".as_slice() };

    let tile_w = 312; let tile_h = 136; let x0 = 36; let x1 = 372; let y0 = 90; let y1 = 246;
    let tile_wifi = ControlTilePlan { rect: Rect::new(x0,y0,tile_w,tile_h), icon: QcIcon::Wifi,
        title: LabelRun::new(x0+28,y0+64,type_scale::CAPTION,b"Wi-Fi",TEXT_WHITE),
        subtitle: LabelRun::new(x0+28,y0+92,type_scale::CAPTION,wifi,TEXT_MUTED) };
    let tile_bt = ControlTilePlan { rect: Rect::new(x1,y0,tile_w,tile_h), icon: QcIcon::Bluetooth,
        title: LabelRun::new(x1+28,y0+64,type_scale::CAPTION,b"Bluetooth",TEXT_WHITE),
        subtitle: LabelRun::new(x1+28,y0+92,type_scale::CAPTION,bluetooth,TEXT_MUTED) };
    let tile_mobile = ControlTilePlan { rect: Rect::new(x0,y1,tile_w,tile_h), icon: QcIcon::Mobile,
        title: LabelRun::new(x0+28,y1+64,type_scale::CAPTION,b"Mobile",TEXT_WHITE),
        subtitle: LabelRun::new(x0+28,y1+92,type_scale::CAPTION,mobile,TEXT_MUTED) };
    let tile_air = ControlTilePlan { rect: Rect::new(x1,y1,tile_w,tile_h), icon: QcIcon::Airplane,
        title: LabelRun::new(x1+28,y1+64,type_scale::CAPTION,b"Airplane",TEXT_WHITE),
        subtitle: LabelRun::new(x1+28,y1+92,type_scale::CAPTION,b"Unavailable",TEXT_MUTED) };

    let slider_x=36; let slider_w=648; let slider_h=76;
    let brightness_rect=Rect::new(slider_x,404,slider_w,slider_h);
    let volume_rect=Rect::new(slider_x,500,slider_w,slider_h);
    // QEMU has no virtual backlight device wired to the guest yet. A stored
    // preference is not proof that changing brightness affects emitted pixels.
    // Keep this control visibly disabled until a real display-power backend exists.
    let brightness_available = false;
    let brightness_value = 0usize;
    let volume=std::audio_query_volume();
    let volume_available=volume.is_ok();
    let volume_value=volume.unwrap_or(0).min(100) as usize;
    let volume_fill_w=(slider_w*volume_value)/100;

    // WHY a caption on the brightness capsule: `brightness_available` is false, so
    // `is_filled` is false for every pixel of it, the fill width is 0 and the
    // capsule drew a sun glyph on an empty track and nothing else. That is the same
    // drawing as a decoration, and a user cannot tell it from a working slider.
    // The toggles 164 px below already say "N/A"; this one now names the missing
    // thing. It prints no number, because there is no backlight to read one from.
    let caption_y = brightness_rect.y + (slider_h - text::line_height(type_scale::CAPTION)) / 2;
    let caption_fit = slider_w - SLIDER_CAPTION_X - SLIDER_CAPTION_INSET;
    let brightness_caption = LabelRun::fitted(
        slider_x + SLIDER_CAPTION_X,
        caption_y,
        type_scale::CAPTION,
        caption_fit,
        if brightness_available {
            b"Brightness"
        } else {
            b"Brightness unavailable - no backlight"
        },
        if brightness_available { TEXT_WHITE } else { TEXT_MUTED },
    );

    // WHY the volume caption is conditional: when the guest audio manager answers,
    // the fill and its thumb already say where the level is and a second line would
    // only repeat it. When it does not, there is no level to fill and no position
    // to mark, and the control has to say so instead of drawing an empty track that
    // looks like a slider at zero.
    let volume_caption_y = volume_rect.y + (slider_h - text::line_height(type_scale::CAPTION)) / 2;
    let volume_caption = if volume_available {
        LabelRun::EMPTY
    } else {
        LabelRun::fitted(
            slider_x + SLIDER_CAPTION_X,
            volume_caption_y,
            type_scale::CAPTION,
            caption_fit,
            b"Volume unavailable - no audio level",
            TEXT_MUTED,
        )
    };

    // WHY the thumb is clamped inside the capsule: the fill is measured from the
    // capsule's left edge, so a fill of the full width puts the position mark one
    // step past the right edge, where the capsule's own rounded test would cut it.
    let volume_thumb = Rect::new(
        (volume_rect.x + volume_fill_w)
            .saturating_sub(SLIDER_THUMB_W / 2)
            .clamp(volume_rect.x, volume_rect.x + volume_rect.w - SLIDER_THUMB_W),
        volume_rect.y + SLIDER_THUMB_INSET,
        SLIDER_THUMB_W,
        slider_h - SLIDER_THUMB_INSET * 2,
    );

    let toggles=[Rect::new(36,644,76,TOGGLE_H),Rect::new(128,644,76,TOGGLE_H),Rect::new(220,644,76,TOGGLE_H),Rect::new(312,644,76,TOGGLE_H)];
    let toggle_icons=[QcIcon::Torch,QcIcon::Rotate,QcIcon::Moon,QcIcon::Power];
    // These controls have no QEMU-backed flashlight, rotation sensor, DND policy or
    // airplane-mode service yet. Show them as disabled, not as live toggles that
    // silently do nothing.
    //
    // WHY the label is centred on the button's own middle: it used to be placed at
    // `toggle.y + 54`, which is 22 px below the button's centre line and whose cell
    // ran past the bottom of the button entirely.
    let toggle_labels = core::array::from_fn(|i| {
        LabelRun::centred(
            toggles[i].x,
            toggles[i].y + (toggles[i].h - text::line_height(type_scale::CAPTION)) / 2,
            toggles[i].w,
            type_scale::CAPTION,
            b"N/A",
            TEXT_MUTED,
        )
    });
    let power_label=LabelRun::centred(POWER_BTN.x+40,POWER_BTN.y+24,POWER_BTN.w-40,type_scale::CAPTION,b"Power Menu",TEXT_WHITE);
    QuickControlsFrame {
        clock, tiles:[tile_wifi,tile_bt,tile_mobile,tile_air], brightness_rect,
        brightness_fill_w:(slider_w*brightness_value)/100, volume_rect,
        volume_fill_w, brightness_available, brightness_caption, volume_available,
        volume_thumb, volume_caption, toggles, toggle_icons, toggle_labels,
        power_btn:POWER_BTN, power_label, home_pill:HOME_PILL,
    }
}

// -----------------------------------------------------------------------------
// Vector Icon Drawers
// -----------------------------------------------------------------------------

#[inline]
fn draw_qc_icon(icon: QcIcon, x: usize, y: usize, cx: usize, cy: usize, tint: u32) -> Option<u32> {
    let dx = x as i32 - cx as i32;
    let dy = y as i32 - cy as i32;

    match icon {
        QcIcon::Wifi => {
            // Wi-Fi arcs
            let dist_sq = dx * dx + dy * dy;
            if dy <= 4 {
                // Arc 1: r in 16..20
                if dist_sq >= 256 && dist_sq <= 400 && dx.abs() <= 18 {
                    return Some(tint);
                }
                // Arc 2: r in 9..13
                if dist_sq >= 81 && dist_sq <= 169 && dx.abs() <= 11 {
                    return Some(tint);
                }
            }
            // Dot: r <= 4
            if dist_sq <= 16 && dy >= 0 {
                return Some(tint);
            }
            None
        }
        QcIcon::Bluetooth => {
            // Central stem dx == 0, dy in -16..16
            if dx == 0 && dy >= -16 && dy <= 16 {
                return Some(tint);
            }
            // Upper and lower diagonals
            if dx > 0 && dx <= 10 {
                if (dy - (dx - 10)).abs() <= 1 && dy >= 0 && dy <= 10 {
                    return Some(tint);
                }
                if (dy + (dx - 10)).abs() <= 1 && dy >= -10 && dy <= 0 {
                    return Some(tint);
                }
                if (dy - dx).abs() <= 1 && dy >= -10 && dy <= 10 {
                    return Some(tint);
                }
                if (dy + dx).abs() <= 1 && dy >= -10 && dy <= 10 {
                    return Some(tint);
                }
            }
            None
        }
        QcIcon::Mobile => {
            // Cellular antenna / signal bars
            if dx >= -12 && dx <= 12 {
                let bar = (dx + 12) / 7;
                let h = (bar + 1) * 6;
                if dy >= 12 - h && dy <= 12 && (dx % 7) <= 4 {
                    return Some(tint);
                }
            }
            None
        }
        QcIcon::Airplane => {
            // Fuselage
            if dx.abs() <= 2 && dy >= -16 && dy <= 16 {
                return Some(tint);
            }
            // Wings
            if dy >= -4 && dy <= 2 && dx.abs() <= 16 {
                return Some(tint);
            }
            // Tail
            if dy >= 10 && dy <= 14 && dx.abs() <= 8 {
                return Some(tint);
            }
            None
        }
        QcIcon::Torch => {
            // Flashlight body
            if dx.abs() <= 6 && dy >= -4 && dy <= 14 {
                return Some(tint);
            }
            // Flashlight head
            if dx.abs() <= 10 && dy >= -12 && dy <= -4 {
                return Some(tint);
            }
            None
        }
        QcIcon::Rotate => {
            // Circular arrow
            let r2 = dx * dx + dy * dy;
            if r2 >= 100 && r2 <= 196 && (dx < 6 || dy > 0) {
                return Some(tint);
            }
            if dx >= 4 && dx <= 12 && dy >= -14 && dy <= -4 {
                return Some(tint);
            }
            None
        }
        QcIcon::Moon => {
            // Crescent moon
            let r_outer = dx * dx + dy * dy;
            let dx_in = dx - 4;
            let dy_in = dy + 2;
            let r_inner = dx_in * dx_in + dy_in * dy_in;
            if r_outer <= 196 && r_inner >= 120 && dx <= 12 {
                return Some(tint);
            }
            None
        }
        QcIcon::Power => {
            // Circle arc
            let r2 = dx * dx + dy * dy;
            if r2 >= 100 && r2 <= 196 && (dy >= -2 || dx.abs() >= 6) {
                return Some(tint);
            }
            // Central tick
            if dx.abs() <= 2 && dy >= -14 && dy <= 0 {
                return Some(tint);
            }
            None
        }
    }
}

// -----------------------------------------------------------------------------
// Per-Pixel Renderer
// -----------------------------------------------------------------------------

static mut FRAME_CACHE: Option<QuickControlsFrame> = None;

#[inline(never)]
pub fn get_quick_controls_color(x: usize, y: usize, state: &LauncherState) -> u32 {
    let f = unsafe {
        let slot: *mut Option<QuickControlsFrame> = core::ptr::addr_of_mut!(FRAME_CACHE);
        if (x == 0 && y == 0) || (*slot).is_none() {
            *slot = Some(build_qc_frame(state));
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

    // 2. 2x2 Network Tiles
    for tile in &f.tiles {
        if tile.rect.contains(x, y) {
            if rounded_contains(tile.rect, x, y, 28) {
                color = TILE_BG;
                if rounded_outline(tile.rect, x, y, 28) {
                    return TILE_BORDER;
                }
                // Vector icon
                let icon_cx = tile.rect.x + 36;
                let icon_cy = tile.rect.y + 36;
                if let Some(ic) = draw_qc_icon(tile.icon, x, y, icon_cx, icon_cy, TEXT_WHITE) {
                    return ic;
                }
                // Title & Subtitle
                if let Some(c) = tile.title.pixel(x, y) {
                    return c;
                }
                if let Some(c) = tile.subtitle.pixel(x, y) {
                    return c;
                }
                return color;
            }
        }
    }

    // 3. Brightness Slider Capsule
    if f.brightness_rect.contains(x, y) {
        if rounded_contains(f.brightness_rect, x, y, 38) {
            let is_filled = f.brightness_available && x <= f.brightness_rect.x + f.brightness_fill_w;
            color = if is_filled { SLIDER_FILL } else { SLIDER_BG };

            // Sun icon at left
            let sun_cx = f.brightness_rect.x + 44;
            let sun_cy = f.brightness_rect.center_y();
            let dx = x as i32 - sun_cx as i32;
            let dy = y as i32 - sun_cy as i32;
            let icon_ink = if is_filled { 0xFF0F172A } else { TEXT_WHITE };

            // Sun central core r <= 7
            if dx * dx + dy * dy <= 49 {
                return icon_ink;
            }
            // Sun rays
            if (dx.abs() <= 1 && (dy.abs() >= 10 && dy.abs() <= 16))
                || (dy.abs() <= 1 && (dx.abs() >= 10 && dx.abs() <= 16))
                || ((dx - dy).abs() <= 1 && (dx.abs() >= 7 && dx.abs() <= 12))
                || ((dx + dy).abs() <= 1 && (dx.abs() >= 7 && dx.abs() <= 12))
            {
                return icon_ink;
            }

            if let Some(c) = f.brightness_caption.pixel(x, y) {
                return c;
            }

            return color;
        }
    }

    // 4. Volume Slider Capsule
    if f.volume_rect.contains(x, y) {
        if rounded_contains(f.volume_rect, x, y, 38) {
            let is_filled = f.volume_available && x <= f.volume_rect.x + f.volume_fill_w;
            color = if is_filled { SLIDER_FILL } else { SLIDER_BG };

            // Speaker icon at left
            let spk_cx = f.volume_rect.x + 44;
            let spk_cy = f.volume_rect.center_y();
            let dx = x as i32 - spk_cx as i32;
            let dy = y as i32 - spk_cy as i32;
            let icon_ink = if is_filled { 0xFF0F172A } else { TEXT_WHITE };

            // Speaker box
            if dx >= -12 && dx <= -4 && dy.abs() <= 6 {
                return icon_ink;
            }
            // Speaker cone
            if dx >= -4 && dx <= 4 && dy.abs() <= (dx + 8) {
                return icon_ink;
            }
            // Sound wave arc
            let r2 = (dx - 4) * (dx - 4) + dy * dy;
            if r2 >= 64 && r2 <= 120 && dx >= 6 {
                return icon_ink;
            }

            // WHY the position mark is drawn only when there is a level: a thumb
            // is a claim about where the level is, and with the query refused there
            // is no level, so a mark would be a claim with nothing behind it.
            if f.volume_available && f.volume_thumb.contains(x, y) {
                return SLIDER_BG;
            }

            if let Some(c) = f.volume_caption.pixel(x, y) {
                return c;
            }

            return color;
        }
    }

    // 5. Quick Toggles Row (Torch, Rotate, Moon)
    for (i, toggle) in f.toggles.iter().enumerate() {
        if toggle.contains(x, y) {
            if rounded_contains(*toggle, x, y, 36) {
                color = TILE_BG;
                if rounded_outline(*toggle, x, y, 36) {
                    return TILE_BORDER;
                }
                let cx = toggle.center_x();
                let cy = toggle.y + TOGGLE_ICON_DY;
                // These controls have no QEMU-backed flashlight, rotation sensor,
                // DND policy or airplane-mode service yet. Show them as disabled,
                // not as live toggles that silently do nothing.
                if let Some(ic) = draw_qc_icon(f.toggle_icons[i], x, y, cx, cy, TEXT_MUTED) {
                    return ic;
                }
                if let Some(c) = f.toggle_labels[i].pixel(x, y) {
                    return c;
                }
                return color;
            }
        }
    }

    // 6. Power Menu Quick Action Button
    if f.power_btn.contains(x, y) {
        if rounded_contains(f.power_btn, x, y, 36) {
            color = TILE_BG;
            if rounded_outline(f.power_btn, x, y, 36) {
                return 0xFFEF4444; // Red accent outline
            }
            // Red power icon
            let cx = f.power_btn.x + 36;
            let cy = f.power_btn.center_y();
            if let Some(ic) = draw_qc_icon(QcIcon::Power, x, y, cx, cy, 0xFFEF4444) {
                return ic;
            }
            if let Some(c) = f.power_label.pixel(x, y) {
                return c;
            }
            return color;
        }
    }

    // 7. Home Indicator Pill
    if rounded_contains(f.home_pill, x, y, 2) {
        return 0xFF94A3B8;
    }

    color
}
