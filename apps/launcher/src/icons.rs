//! First-party vector icon engine for the InterOS launcher.
//!
//! Provides geometric vector pictograms for first-party system apps, matching
//! the clean mobile OS design language. Falls back to proportional acronym
//! badges for third-party packages with no built-in vector glyph.

#![allow(dead_code)]

/// Supported first-party pictogram identifiers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IconKind {
    Phone,
    Messages,
    Camera,
    Browser,
    Files,
    Gallery,
    Settings,
    Music,
    Calculator,
    Terminal,
    Vault,
    Health,
    MeshChat,
    Sensors,
    Apps,
    Calendar,
    Notes,
    Contacts,
    Maps,
    Briefcase,
    CardDemo,
}

/// Identifies if a package ID or process name maps to a first-party vector pictogram.
pub fn match_icon_kind(id: &[u8]) -> Option<IconKind> {
    if id.is_empty() {
        return None;
    }
    // Match package IDs, short process names, or badge acronyms
    if id == b"org.interos.dialer" || id == b"dialer" || id == b"app_dialer" || id == b"CALL" || id == b"TEL"
        || id == b"Phone" || id == b"phone" || id == b"Dialer"
    {
        Some(IconKind::Phone)
    } else if id == b"org.interos.messages" || id == b"messages" || id == b"app_messages" || id == b"MSG" || id == b"SMS"
        || id == b"Messages" || id == b"sms"
    {
        Some(IconKind::Messages)
    } else if id == b"org.interos.camera" || id == b"camera" || id == b"app_camera"
        || id == b"CAM"
        || id == b"Camera" || id == b"Camera Preview"
    {
        Some(IconKind::Camera)
    } else if id == b"org.interos.browser" || id == b"browser" || id == b"app_browser" || id == b"WEB" || id == b"BROW"
        || id == b"Browser"
    {
        Some(IconKind::Browser)
    } else if id == b"org.interos.file_manager" || id == b"file_manager" || id == b"files" || id == b"app_file_manager" || id == b"FILE"
        || id == b"Files" || id == b"File Manager"
    {
        Some(IconKind::Files)
    } else if id == b"org.interos.gallery" || id == b"gallery" || id == b"photos" || id == b"app_gallery" || id == b"GAL" || id == b"PHO"
        || id == b"PIC" || id == b"Gallery" || id == b"Photos"
    {
        Some(IconKind::Gallery)
    } else if id == b"org.interos.settings" || id == b"settings" || id == b"app_settings" || id == b"SET" || id == b"CFG"
        || id == b"Settings"
    {
        Some(IconKind::Settings)
    } else if id == b"org.interos.music" || id == b"music" || id == b"app_music" || id == b"MUS" || id == b"AUD"
        || id == b"Music" || id == b"Media" || id == b"Media Player"
    {
        Some(IconKind::Music)
    } else if id == b"org.interos.calculator" || id == b"calculator" || id == b"app_calculator" || id == b"CALC" || id == b"NUM"
        || id == b"Calculator"
    {
        Some(IconKind::Calculator)
    } else if id == b"org.interos.terminal" || id == b"terminal" || id == b"app_terminal" || id == b"TERM" || id == b"CLI"
        || id == b"Terminal"
    {
        Some(IconKind::Terminal)
    } else if id == b"org.interos.vault" || id == b"vault" || id == b"app_vault"
        || id == b"VAUL" || id == b"VAULT" || id == b"PASS"
        || id == b"Vault"
    {
        Some(IconKind::Vault)
    } else if id == b"org.interos.health" || id == b"health" || id == b"HLTH" || id == b"FIT"
        || id == b"Health"
    {
        Some(IconKind::Health)
    } else if id == b"org.interos.meshchat" || id == b"meshchat" || id == b"MESH" || id == b"CHAT"
        || id == b"MeshChat"
    {
        Some(IconKind::MeshChat)
    } else if id == b"org.interos.sensor_dashboard" || id == b"sensor_dashboard" || id == b"sensors" || id == b"SENS"
        || id == b"Sensors" || id == b"Sensor Dashboard"
    {
        Some(IconKind::Sensors)
    } else if id == b"org.interos.apps" || id == b"apps" || id == b"APPS" || id == b"drawer"
        || id == b"Apps"
    {
        Some(IconKind::Apps)
    } else if id == b"org.interos.calendar" || id == b"calendar" || id == b"app_calendar" || id == b"Calendar" || id == b"CAL" {
        Some(IconKind::Calendar)
    } else if id == b"org.interos.notes" || id == b"notes" || id == b"Notes" || id == b"NOTE" {
        Some(IconKind::Notes)
    } else if id == b"org.interos.contacts" || id == b"contacts" || id == b"Contacts" || id == b"CONT" {
        Some(IconKind::Contacts)
    } else if id == b"org.interos.maps" || id == b"maps" || id == b"Maps" || id == b"MAP" {
        Some(IconKind::Maps)
    } else if id == b"briefcase" || id == b"work" || id == b"Work" || id == b"WORK" {
        Some(IconKind::Briefcase)
    } else if id == b"org.interos.card_demo" || id == b"card_demo" || id == b"app_card_demo"
        || id == b"Card Demo" || id == b"CARD" || id == b"Card"
    {
        Some(IconKind::CardDemo)
    } else {
        None
    }
}

/// Rasterises a pictogram into a larger box than the standard `[-16..=16]`.
///
/// `extent` is the half-size of the output box in pixels, so `16` reproduces
/// [`rasterize_icon_pixel`] exactly and anything larger magnifies the glyph.
pub fn rasterize_icon_pixel_scaled(
    kind: IconKind,
    px: usize,
    py: usize,
    cx: usize,
    cy: usize,
    color: u32,
    extent: i32,
) -> Option<u32> {
    let extent = extent.max(16);
    let dx = px as i32 - cx as i32;
    let dy = py as i32 - cy as i32;
    if dx.abs() > extent || dy.abs() > extent {
        return None;
    }
    // Map output (px, py) to canonical 32x32 space [-16..=16] with round-to-nearest
    let rx = (dx * 32 + (if dx >= 0 { extent } else { -extent })) / (2 * extent);
    let ry = (dy * 32 + (if dy >= 0 { extent } else { -extent })) / (2 * extent);
    let sx = (cx as i32 + rx) as usize;
    let sy = (cy as i32 + ry) as usize;
    rasterize_icon_pixel(kind, sx, sy, cx, cy, color)
}

/// Anti-aliased scaled rasteriser using 4x subpixel supersampling.
///
/// Smooths curves, diagonal edges, and fine details over the given background color.
pub fn rasterize_icon_pixel_scaled_aa(
    kind: IconKind,
    px: usize,
    py: usize,
    cx: usize,
    cy: usize,
    color: u32,
    bg: u32,
    extent: i32,
) -> Option<u32> {
    let extent = extent.max(16);
    let dx = px as i32 - cx as i32;
    let dy = py as i32 - cy as i32;
    if dx.abs() > extent || dy.abs() > extent {
        return None;
    }
    // 4 subpixel sample positions at (±0.25, ±0.25)
    let mut hits = 0u32;
    const OFFSETS: [(i32, i32); 4] = [(-1, -1), (1, -1), (-1, 1), (1, 1)];
    for &(ox, oy) in &OFFSETS {
        let sub_dx = dx * 4 + ox;
        let sub_dy = dy * 4 + oy;
        let rx = (sub_dx * 32 + (if sub_dx >= 0 { extent * 4 } else { -extent * 4 })) / (8 * extent);
        let ry = (sub_dy * 32 + (if sub_dy >= 0 { extent * 4 } else { -extent * 4 })) / (8 * extent);
        let sx = (cx as i32 + rx) as usize;
        let sy = (cy as i32 + ry) as usize;
        if rasterize_icon_pixel(kind, sx, sy, cx, cy, color).is_some() {
            hits += 1;
        }
    }
    match hits {
        0 => None,
        4 => Some(color),
        h => {
            let cov = (h * 255) / 4;
            Some(interos_std::design::font::blend(color, bg, cov as u8))
        }
    }
}

/// Evaluates whether the pixel at `(px, py)` falls inside the vector pictogram
/// of `kind` centered at `(cx, cy)` with standard size bounds `[-16..=16]`.
///
/// Returns `Some(color)` if inked, or `None` if background/transparent.
pub fn rasterize_icon_pixel(
    kind: IconKind,
    px: usize,
    py: usize,
    cx: usize,
    cy: usize,
    color: u32,
) -> Option<u32> {
    let rx = (px as i32) - (cx as i32);
    let ry = (py as i32) - (cy as i32);

    if rx < -16 || rx > 16 || ry < -16 || ry > 16 {
        return None;
    }

    let inked = match kind {
        IconKind::Phone => {
            // Earpiece bulb at upper right (5, -6)
            let ear = (rx - 5) * (rx - 5) + (ry + 6) * (ry + 6) <= 16;
            // Mouthpiece bulb at lower left (-6, 5)
            let mouth = (rx + 6) * (rx + 6) + (ry - 5) * (ry - 5) <= 16;
            // Sweeping handset handle connecting them, bowing toward (-4, -4)
            let dx = rx - 5;
            let dy = ry - 5;
            let d2 = dx * dx + dy * dy;
            let handle = (d2 >= 70 && d2 <= 145) && (rx <= 5 && ry <= 5) && (rx + ry <= 1);
            ear || mouth || handle
        }
        IconKind::Messages => {
            // Bubble body in rx in -11..11, ry in -9..4, rounded corners r=3
            let in_box = rx.abs() <= 11 && ry >= -9 && ry <= 4;
            let cx = if rx.abs() > 8 { rx.abs() - 8 } else { 0 };
            let cy = if ry < -6 { -6 - ry } else if ry > 1 { ry - 1 } else { 0 };
            let in_bubble = in_box && (cx * cx + cy * cy <= 9);
            let in_inner = (rx.abs() <= 9 && ry >= -7 && ry <= 2)
                && ((if rx.abs() > 7 { rx.abs() - 7 } else { 0 }) * (if rx.abs() > 7 { rx.abs() - 7 } else { 0 })
                    + (if ry < -5 { -5 - ry } else if ry > 0 { ry } else { 0 }) * (if ry < -5 { -5 - ry } else if ry > 0 { ry } else { 0 }) <= 4);
            let bubble_rim = in_bubble && !in_inner;
            // Tail pointing bottom-left
            let tail = (ry >= 3 && ry <= 9)
                && (rx >= -10 && rx <= -3)
                && (rx <= -3 - (ry - 3))
                && (rx >= -10 + (ry - 3) / 2);
            let tail_inner = (ry >= 4 && ry <= 7)
                && (rx >= -8 && rx <= -5)
                && (rx <= -5 - (ry - 4));
            // Two text lines inside bubble
            let line1 = (ry >= -5 && ry <= -4) && (rx >= -6 && rx <= 6);
            let line2 = (ry >= -1 && ry <= 0) && (rx >= -6 && rx <= 3);
            (bubble_rim || (tail && !tail_inner)) || line1 || line2
        }
        IconKind::Camera => {
            let in_box = rx.abs() <= 11 && ry >= -6 && ry <= 9;
            let cx = if rx.abs() > 8 { rx.abs() - 8 } else { 0 };
            let cy = if ry < -3 { -3 - ry } else if ry > 6 { ry - 6 } else { 0 };
            let in_body = in_box && (cx * cx + cy * cy <= 9);
            let in_inner = (rx.abs() <= 9 && ry >= -4 && ry <= 7)
                && ((if rx.abs() > 7 { rx.abs() - 7 } else { 0 }) * (if rx.abs() > 7 { rx.abs() - 7 } else { 0 })
                    + (if ry < -2 { -2 - ry } else if ry > 5 { ry - 5 } else { 0 }) * (if ry < -2 { -2 - ry } else if ry > 5 { ry - 5 } else { 0 }) <= 4);
            let body_rim = in_body && !in_inner;
            let prism = (rx.abs() <= 5 && ry >= -9 && ry <= -6) && (ry >= -9 + rx.abs() - 3);
            let prism_inner = rx.abs() <= 3 && ry >= -7 && ry <= -6;
            let prism_rim = prism && !prism_inner;
            let lx = rx;
            let ly = ry - 2;
            let lr2 = lx * lx + ly * ly;
            let lens_rim = lr2 >= 9 && lr2 <= 25;
            let flash = (rx - 6) * (rx - 6) + (ry + 2) * (ry + 2) <= 2;
            body_rim || prism_rim || lens_rim || flash
        }
        IconKind::Browser => {
            let r2 = rx * rx + ry * ry;
            // Outer circular rim: radius ~10
            let rim = r2 >= 75 && r2 <= 115;
            // Equator
            let eq = ry.abs() <= 1 && rx.abs() <= 9;
            // Prime meridian
            let prime = rx.abs() <= 1 && ry.abs() <= 9;
            // Latitude parallels
            let lat = ((ry.abs() - 5).abs() <= 1) && (r2 <= 100);
            // Elliptical meridian: (rx * 2)^2 + ry^2
            let mr2 = (rx * 2) * (rx * 2) + ry * ry;
            let meridian = (mr2 >= 75 && mr2 <= 125) && (r2 <= 105);
            rim || eq || prime || lat || meridian
        }
        IconKind::Files => {
            let tab = rx >= -11 && rx <= -2 && ry >= -9 && ry <= -6;
            let in_box = rx.abs() <= 11 && ry >= -6 && ry <= 9;
            let cx = if rx.abs() > 8 { rx.abs() - 8 } else { 0 };
            let cy = if ry > 6 { ry - 6 } else { 0 };
            let in_body = in_box && (cx * cx + cy * cy <= 9);
            let in_inner = (rx.abs() <= 9 && ry >= -4 && ry <= 7)
                && ((if rx.abs() > 7 { rx.abs() - 7 } else { 0 }) * (if rx.abs() > 7 { rx.abs() - 7 } else { 0 })
                    + (if ry > 5 { ry - 5 } else { 0 }) * (if ry > 5 { ry - 5 } else { 0 }) <= 4);
            let body_rim = (in_body || tab) && !in_inner;
            let flap = (ry == -3) && rx.abs() <= 9;
            body_rim || flap
        }
        IconKind::Gallery => {
            let frame = if rx.abs() <= 11 && ry.abs() <= 9 {
                !(rx.abs() <= 9 && ry.abs() <= 7)
            } else {
                false
            };
            let sun = (rx - 5) * (rx - 5) + (ry + 3) * (ry + 3) <= 4;
            let peak1 = (ry >= 0 && ry <= 7) && ((rx + 3).abs() <= (ry - 0));
            let peak2 = (ry >= 3 && ry <= 7) && ((rx - 4).abs() <= (ry - 3));
            frame || sun || peak1 || peak2
        }
        IconKind::Settings => {
            let r2 = rx * rx + ry * ry;
            let rim = r2 >= 20 && r2 <= 64;
            let tooth_h = (rx.abs() <= 11 && ry.abs() <= 2) && r2 >= 36;
            let tooth_v = (rx.abs() <= 2 && ry.abs() <= 11) && r2 >= 36;
            let tooth_d1 = ((rx - ry).abs() <= 2 && (rx + ry).abs() <= 12) && r2 >= 36;
            let tooth_d2 = ((rx + ry).abs() <= 2 && (rx - ry).abs() <= 12) && r2 >= 36;
            (rim || tooth_h || tooth_v || tooth_d1 || tooth_d2) && r2 >= 9
        }
        IconKind::Music => {
            let head1 = (rx + 5) * (rx + 5) * 3 + (ry - 4) * (ry - 4) * 2 <= 20;
            let head2 = (rx - 4) * (rx - 4) * 3 + (ry - 2) * (ry - 2) * 2 <= 20;
            let stem1 = (rx >= -3 && rx <= -2) && (ry >= -8 && ry <= 4);
            let stem2 = (rx >= 6 && rx <= 7) && (ry >= -10 && ry <= 2);
            let beam = (rx >= -3 && rx <= 7)
                && (ry >= -10 - (rx - 7) * 2 / 10 && ry <= -8 - (rx - 7) * 2 / 10);
            head1 || head2 || stem1 || stem2 || beam
        }
        IconKind::Calculator => {
            // Rounded calculator body
            let in_body = if rx.abs() <= 9 && ry.abs() <= 11 {
                let cx = if rx.abs() > 6 { rx.abs() - 6 } else { 0 };
                let cy = if ry.abs() > 8 { ry.abs() - 8 } else { 0 };
                cx * cx + cy * cy <= 9
            } else {
                false
            };
            // LCD display screen bar at top
            let in_lcd = rx.abs() <= 6 && (ry >= -8 && ry <= -5);
            // 6 keypad buttons in 3x2 grid
            let btn = (rx.abs() >= 2 && rx.abs() <= 6)
                && ((ry >= -1 && ry <= 0) || (ry >= 3 && ry <= 4) || (ry >= 7 && ry <= 8));
            in_body && !(in_lcd || btn)
        }
        IconKind::Terminal => {
            let d = rx - (-2 - ry.abs());
            let chevron = (ry >= -7 && ry <= 7) && (d >= 0 && d <= 1) && rx <= -2;
            let cursor = (rx >= 1 && rx <= 9) && (ry >= 5 && ry <= 7);
            chevron || cursor
        }
        IconKind::Vault => {
            // Shackle arch and straight legs
            let arch_r2 = rx * rx + (ry + 2) * (ry + 2);
            let shackle_arch = arch_r2 <= 36 && arch_r2 >= 16 && ry <= -2;
            let shackle_legs = (rx.abs() >= 4 && rx.abs() <= 6) && (ry >= -2 && ry <= 0);
            // Padlock body
            let body = if rx.abs() <= 9 && ry >= 0 && ry <= 11 {
                let cx = if rx.abs() > 7 { rx.abs() - 7 } else { 0 };
                let cy = if ry > 9 { ry - 9 } else { 0 };
                cx * cx + cy * cy <= 4
            } else {
                false
            };
            // Keyhole cutout
            let key_circle = rx * rx + (ry - 4) * (ry - 4) <= 3;
            let key_slot = rx.abs() <= 1 && (ry >= 4 && ry <= 8);
            (shackle_arch || shackle_legs || body) && !(key_circle || key_slot)
        }
        IconKind::Health => {
            // Heartbeat ECG pulse wave
            let baseline1 = (rx >= -12 && rx <= -6) && (ry >= -1 && ry <= 1);
            let peak_up = (rx >= -6 && rx <= -2) && (ry == -2 * (rx + 6) || ry == -2 * (rx + 6) + 1);
            let peak_down = (rx >= -2 && rx <= 2) && (ry == 3 * rx || ry == 3 * rx + 1);
            let peak_return = (rx >= 2 && rx <= 6) && (ry == -2 * (rx - 6) || ry == -2 * (rx - 6) + 1);
            let baseline2 = (rx >= 6 && rx <= 12) && (ry >= -1 && ry <= 1);
            baseline1 || peak_up || peak_down || peak_return || baseline2
        }
        IconKind::MeshChat => {
            // 3 nodes in triangle with interconnecting links
            let node1 = rx * rx + (ry + 7) * (ry + 7) <= 9;
            let node2 = (rx + 7) * (rx + 7) + (ry - 6) * (ry - 6) <= 9;
            let node3 = (rx - 7) * (rx - 7) + (ry - 6) * (ry - 6) <= 9;
            let link1 = (rx.abs() <= 1) && (ry >= -7 && ry <= 6);
            let link2 = (ry >= 5 && ry <= 7) && (rx >= -7 && rx <= 7);
            node1 || node2 || node3 || link1 || link2
        }
        IconKind::Sensors => {
            let dx = rx;
            let dy = ry - 7;
            let r2 = dx * dx + dy * dy;
            let core = r2 <= 4;
            let arc1 = r2 >= 25 && r2 <= 42 && dy <= 0;
            let arc2 = r2 >= 81 && r2 <= 110 && dy <= 0;
            let arc3 = r2 >= 169 && r2 <= 210 && dy <= 0;
            let in_wedge = dx.abs() * 10 <= (-dy) * 13;
            core || ((arc1 || arc2 || arc3) && in_wedge)
        }
        IconKind::Apps => {
            let in_tile_x = (rx >= -9 && rx <= -2) || (rx >= 2 && rx <= 9);
            let in_tile_y = (ry >= -9 && ry <= -2) || (ry >= 2 && ry <= 9);
            in_tile_x && in_tile_y
        }
        IconKind::Calendar => {
            let loop_binder = (rx == -4 || rx == 4) && (ry >= -11 && ry <= -6);
            let body = if rx.abs() <= 9 && ry >= -7 && ry <= 10 {
                let cx = if rx.abs() > 7 { rx.abs() - 7 } else { 0 };
                let cy = if ry > 8 { ry - 8 } else { 0 };
                cx * cx + cy * cy <= 4
            } else {
                false
            };
            let header = body && (ry >= -7 && ry <= -3);
            let rim = body && (rx.abs() == 9 || ry == 10);
            // Digit 1
            let one_stem = (rx == -4 || rx == -3) && (ry >= 1 && ry <= 7);
            let one_serif = (rx == -5) && (ry >= 2 && ry <= 3);
            let one_base = (rx >= -6 && rx <= -2) && (ry == 7);
            let digit_one = one_stem || one_serif || one_base;
            // Digit 7
            let seven_top = (rx >= 0 && rx <= 5) && (ry >= 1 && ry <= 2);
            let seven_diag = (ry >= 2 && ry <= 7) && ((5 - (ry - 2)) == rx);
            let digit_seven = seven_top || seven_diag;
            loop_binder || header || rim || digit_one || digit_seven
        }
        IconKind::Notes => {
            // Document outline with 3 horizontal writing lines
            let sheet = rx >= -9 && rx <= 9 && ry >= -11 && ry <= 11;
            let line1 = rx >= -6 && rx <= 6 && (ry == -4 || ry == -3);
            let line2 = rx >= -6 && rx <= 6 && (ry == 1 || ry == 2);
            let line3 = rx >= -6 && rx <= 2 && (ry == 6 || ry == 7);
            let margin = rx == -7 && (ry >= -9 && ry <= 9);
            sheet && (line1 || line2 || line3 || margin)
        }
        IconKind::Contacts => {
            // Person silhouette: head circle + shoulder curve
            let head = rx * rx + (ry + 3) * (ry + 3) <= 16;
            let shoulders = (ry >= 3 && ry <= 10) && (rx.abs() <= 9) && (rx * rx + (ry - 3) * (ry - 3) <= 49);
            head || shoulders
        }
        IconKind::Maps => {
            // Map location teardrop pin
            let pin_circle = rx * rx + (ry + 2) * (ry + 2) <= 36;
            let pin_hole = rx * rx + (ry + 2) * (ry + 2) <= 9;
            let pin_tip = (ry >= 3 && ry <= 10) && (rx.abs() <= (10 - ry));
            (pin_circle && !pin_hole) || pin_tip
        }
        IconKind::Briefcase => {
            // Arched handle
            let handle = (rx.abs() <= 5 && ry >= -10 && ry <= -5) && !(rx.abs() <= 3 && ry >= -8 && ry <= -5);
            // Main rectangular briefcase body
            let body = rx >= -11 && rx <= 11 && ry >= -4 && ry <= 10;
            // Center latch
            let latch = rx.abs() <= 2 && ry >= -1 && ry <= 2;
            handle || body || latch
        }
        IconKind::CardDemo => {
            // Credit / smart card body: rx in -11..11, ry in -7..7
            let in_body = if rx.abs() <= 11 && ry.abs() <= 7 {
                let cx = if rx.abs() > 9 { rx.abs() - 9 } else { 0 };
                let cy = if ry.abs() > 5 { ry.abs() - 5 } else { 0 };
                cx * cx + cy * cy <= 4
            } else {
                false
            };
            // Outer card border
            let is_border = if in_body {
                let inner = if rx.abs() <= 10 && ry.abs() <= 6 {
                    let cx = if rx.abs() > 8 { rx.abs() - 8 } else { 0 };
                    let cy = if ry.abs() > 4 { ry.abs() - 4 } else { 0 };
                    cx * cx + cy * cy <= 4
                } else {
                    false
                };
                !inner
            } else {
                false
            };
            // Magnetic stripe
            let stripe = in_body && (ry >= -4 && ry <= -2);
            // EMV Smart Chip
            let chip = (rx >= -8 && rx <= -4) && (ry >= 1 && ry <= 4);
            // Contactless waves
            let dx = rx - 3;
            let dy = ry - 2;
            let r2 = dx * dx + dy * dy;
            let nfc = (dx > 0) && ((r2 >= 9 && r2 <= 16) || (r2 >= 25 && r2 <= 36)) && dy.abs() <= dx;
            is_border || stripe || chip || nfc
        }
    };

    if inked {
        Some(color)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_first_party_app_has_an_icon_kind() {
        assert_eq!(match_icon_kind(b"org.interos.dialer"), Some(IconKind::Phone));
        assert_eq!(match_icon_kind(b"org.interos.messages"), Some(IconKind::Messages));
        assert_eq!(match_icon_kind(b"org.interos.camera"), Some(IconKind::Camera));
        assert_eq!(match_icon_kind(b"org.interos.browser"), Some(IconKind::Browser));
        assert_eq!(match_icon_kind(b"org.interos.file_manager"), Some(IconKind::Files));
        assert_eq!(match_icon_kind(b"org.interos.gallery"), Some(IconKind::Gallery));
        assert_eq!(match_icon_kind(b"org.interos.settings"), Some(IconKind::Settings));
        assert_eq!(match_icon_kind(b"org.interos.music"), Some(IconKind::Music));
        assert_eq!(match_icon_kind(b"org.interos.calculator"), Some(IconKind::Calculator));
        assert_eq!(match_icon_kind(b"org.interos.terminal"), Some(IconKind::Terminal));
        assert_eq!(match_icon_kind(b"org.interos.vault"), Some(IconKind::Vault));
        assert_eq!(match_icon_kind(b"org.interos.health"), Some(IconKind::Health));
        assert_eq!(match_icon_kind(b"org.interos.meshchat"), Some(IconKind::MeshChat));
        assert_eq!(match_icon_kind(b"org.interos.sensor_dashboard"), Some(IconKind::Sensors));
        assert_eq!(match_icon_kind(b"org.interos.calendar"), Some(IconKind::Calendar));
        assert_eq!(match_icon_kind(b"org.interos.notes"), Some(IconKind::Notes));
        assert_eq!(match_icon_kind(b"org.interos.contacts"), Some(IconKind::Contacts));
        assert_eq!(match_icon_kind(b"org.interos.maps"), Some(IconKind::Maps));
        assert_eq!(match_icon_kind(b"work"), Some(IconKind::Briefcase));
        assert_eq!(match_icon_kind(b"org.interos.card_demo"), Some(IconKind::CardDemo));
        assert_eq!(match_icon_kind(b"org.custom.unknown"), None);
    }

    #[test]
    fn icons_render_non_empty_within_bounds() {
        let kinds = [
            IconKind::Phone,
            IconKind::Messages,
            IconKind::Camera,
            IconKind::Browser,
            IconKind::Files,
            IconKind::Gallery,
            IconKind::Settings,
            IconKind::Music,
            IconKind::Calculator,
            IconKind::Terminal,
            IconKind::Vault,
            IconKind::Health,
            IconKind::MeshChat,
            IconKind::Sensors,
            IconKind::Apps,
            IconKind::Calendar,
            IconKind::Notes,
            IconKind::Contacts,
            IconKind::Maps,
            IconKind::Briefcase,
            IconKind::CardDemo,
        ];

        for &kind in &kinds {
            let mut count = 0;
            let cx = 100;
            let cy = 100;
            for y in 84..=116 {
                for x in 84..=116 {
                    if rasterize_icon_pixel(kind, x, y, cx, cy, 0xFFFFFFFF).is_some() {
                        count += 1;
                    }
                }
            }
            assert!(count > 10, "{:?} must render at least 10 inked pixels, got {}", kind, count);
            // Verify transparent outside 32x32 bounding box
            assert_eq!(rasterize_icon_pixel(kind, 80, 80, cx, cy, 0xFFFFFFFF), None);
            assert_eq!(rasterize_icon_pixel(kind, 120, 120, cx, cy, 0xFFFFFFFF), None);
        }
    }
}
