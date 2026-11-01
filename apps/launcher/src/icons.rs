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
    Vault,
    Health,
    MeshChat,
    Apps,
    Calendar,
    Notes,
    Contacts,
    Maps,
    Briefcase,
    CardDemo,
    Sun,
    CheckCircle,
    AgentManager,
    VideoPlayer,
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
    } else if id == b"org.interos.agent_manager" || id == b"agent_manager" || id == b"app_agent_manager"
        || id == b"Agent Manager" || id == b"agent" || id == b"AI" || id == b"AGNT"
    {
        Some(IconKind::AgentManager)
    } else if id == b"org.interos.video_player" || id == b"video_player" || id == b"app_video_player"
        || id == b"Video Player" || id == b"video" || id == b"Video" || id == b"VID"
    {
        Some(IconKind::VideoPlayer)
    } else if id == b"briefcase" || id == b"work" || id == b"Work" || id == b"WORK" {
        Some(IconKind::Briefcase)
    } else if id == b"org.interos.card_demo" || id == b"card_demo" || id == b"app_card_demo"
        || id == b"Card Demo" || id == b"CARD" || id == b"Card"
    {
        Some(IconKind::CardDemo)
    } else if id == b"sun" || id == b"weather" || id == b"SUN" {
        Some(IconKind::Sun)
    } else if id == b"check" || id == b"check_circle" || id == b"DONE" {
        Some(IconKind::CheckCircle)
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
    let fx = (dx * 256) / extent;
    let fy = (dy * 256) / extent;
    if eval_icon_subpixel(kind, fx, fy) {
        Some(color)
    } else {
        None
    }
}

/// Anti-aliased scaled rasteriser using 9x subpixel supersampling (3x3 grid)
/// evaluated over high-definition 256-space vector curves.
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
    // 9 subpixel sample positions at (-1/3, 0, +1/3)
    let mut hits = 0u32;
    const OFFSETS: [i32; 3] = [-1, 0, 1];
    for &oy in &OFFSETS {
        for &ox in &OFFSETS {
            let sub_dx = dx * 3 + ox;
            let sub_dy = dy * 3 + oy;
            let fx = (sub_dx * 256) / (extent * 3);
            let fy = (sub_dy * 256) / (extent * 3);
            if eval_icon_subpixel(kind, fx, fy) {
                hits += 1;
            }
        }
    }
    match hits {
        0 => None,
        9 => Some(color),
        h => {
            let cov = (h * 255) / 9;
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

    let fx = rx * 16;
    let fy = ry * 16;
    if eval_icon_subpixel(kind, fx, fy) {
        Some(color)
    } else {
        None
    }
}

/// High-Definition Vector Pictogram Evaluation Engine.
///
/// Evaluates whether a point `(fx, fy)` in normalized fixed-point space `[-256..=256]`
/// falls inside the solid vector silhouette of `kind`. All geometry is calculated
/// using integer fixed-point math (`#![no_std]` compliant, zero heap allocation).
pub fn eval_icon_subpixel(kind: IconKind, fx: i32, fy: i32) -> bool {
    if fx < -256 || fx > 256 || fy < -256 || fy > 256 {
        return false;
    }

    match kind {
        IconKind::Phone => {
            // Earpiece bulb at upper right (70, -85)
            let ear = (fx - 70) * (fx - 70) + (fy + 85) * (fy + 85) <= 3025;
            // Mouthpiece bulb at lower left (-85, 70)
            let mouth = (fx + 85) * (fx + 85) + (fy - 70) * (fy - 70) <= 3025;
            // Sweeping ergonomic handset handle connecting them
            let d2 = (fx - 70) * (fx - 70) + (fy - 70) * (fy - 70);
            let handle = d2 >= 18000 && d2 <= 42000 && fx <= 70 && fy <= 70 && fx + fy <= 20;
            ear || mouth || handle
        }
        IconKind::Messages => {
            // Solid chat bubble with rounded corners and bottom-left pointer tail
            let cx = if fx.abs() > 95 { fx.abs() - 95 } else { 0 };
            let cy = if fy < -75 { -75 - fy } else if fy > 35 { fy - 35 } else { 0 };
            let in_bubble = fx.abs() <= 165 && fy >= -145 && fy <= 105 && (cx * cx + cy * cy <= 4900);
            let tail = fy >= 50 && fy <= 165 && fx >= -165 && fx <= -45 && (fx <= -45 - (fy - 50)) && (fx >= -165 + (fy - 50) * 2 / 3);
            in_bubble || tail
        }
        IconKind::Camera => {
            // Viewfinder prism on top
            let hump = fx.abs() <= 60 && fy >= -160 && fy <= -100 && !(fy < -140 && fx.abs() > 45);
            // Main camera body with rounded corners
            let cx = if fx.abs() > 120 { fx.abs() - 120 } else { 0 };
            let cy = if fy < -50 { -50 - fy } else if fy > 95 { fy - 95 } else { 0 };
            let in_body = fx.abs() <= 170 && fy >= -100 && fy <= 145 && (cx * cx + cy * cy <= 2500);
            if !hump && !in_body {
                return false;
            }
            // Center camera lens at (0, 25)
            let lr2 = fx * fx + (fy - 25) * (fy - 25);
            let lens_cutout_ring = lr2 >= 4225 && lr2 <= 6400; // concentric ring cutout
            let flash = (fx - 110) * (fx - 110) + (fy + 50) * (fy + 50) <= 324;
            hump || (in_body && !lens_cutout_ring) || flash
        }
        IconKind::Browser => {
            // Globe / planet with tilted orbital ring
            let r2 = fx * fx + fy * fy;
            let in_planet = r2 <= 8100; // radius 90

            // 35-degree tilted ring coordinate frame
            // cos(35) ~= 210/256, sin(35) ~= 147/256
            let u = (210 * fx + 147 * fy) / 256;
            let v = (-147 * fx + 210 * fy) / 256;

            let u2 = (u * u) as i64;
            let v2 = (v * v) as i64;
            // Outer ring: semi-major 180, semi-minor 55
            let is_outer = 3025 * u2 + 32400 * v2 <= 98_010_000;
            // Inner ring: semi-major 130, semi-minor 35
            let is_inner = 1225 * u2 + 16900 * v2 >= 20_702_500;
            let is_ring = is_outer && is_inner;

            // Halo cutout around front ring where it passes in front of the planet
            let is_cutout_outer = 4225 * u2 + 32400 * v2 <= 136_890_000;
            let is_cutout_inner = 900 * u2 + 16900 * v2 >= 15_210_000;
            let is_cutout = is_cutout_outer && is_cutout_inner && v > 0;

            if is_ring && (v > 0 || !in_planet) {
                true
            } else if in_planet && !is_cutout {
                true
            } else {
                false
            }
        }
        IconKind::Files => {
            // Folder top tab at upper left
            let tab = fx >= -150 && fx <= -30 && fy >= -145 && fy <= -80;
            // Main folder body with rounded corners
            let cx = if fx.abs() > 115 { fx.abs() - 115 } else { 0 };
            let cy = if fy > 95 { fy - 95 } else { 0 };
            let in_body = fx.abs() <= 165 && fy >= -85 && fy <= 145 && (cx * cx + cy * cy <= 2500);
            // Front flap accent line
            let flap_slit = fy >= -15 && fy <= -2 && fx.abs() <= 135;
            (tab || in_body) && !flap_slit
        }
        IconKind::Gallery => {
            // Photo card frame with scenic mountain and sun
            let is_outer = fx.abs() <= 165 && fy.abs() <= 125;
            let is_inner = fx.abs() <= 135 && fy.abs() <= 95;
            let frame = is_outer && !is_inner;
            let sun = (fx - 70) * (fx - 70) + (fy + 40) * (fy + 40) <= 729;
            let mtn1 = is_inner && fy >= -10 && (fx + 40).abs() <= (95 - fy) * 6 / 5;
            let mtn2 = is_inner && fy >= 25 && (fx - 60).abs() <= (95 - fy);
            frame || sun || mtn1 || mtn2
        }
        IconKind::Settings => {
            let r2 = fx * fx + fy * fy;
            let center_hole = r2 <= 2025;
            let gear_wheel = r2 <= 14400;
            let tooth_h = fx.abs() <= 175 && fy.abs() <= 24 && r2 >= 10000;
            let tooth_v = fy.abs() <= 175 && fx.abs() <= 24 && r2 >= 10000;
            let tooth_d1 = (fx - fy).abs() <= 34 && (fx + fy).abs() <= 245 && r2 >= 10000 && r2 <= 30625;
            let tooth_d2 = (fx + fy).abs() <= 34 && (fx - fy).abs() <= 245 && r2 >= 10000 && r2 <= 30625;
            (gear_wheel || tooth_h || tooth_v || tooth_d1 || tooth_d2) && !center_hole
        }
        IconKind::Music => {
            // Double eighth-notes connected by top beam
            let head1 = (fx + 75) * (fx + 75) * 3 + (fy - 60) * (fy - 60) * 2 <= 2900;
            let head2 = (fx - 55) * (fx - 55) * 3 + (fy - 30) * (fy - 30) * 2 <= 2900;
            let stem1 = fx >= -42 && fx <= -22 && fy >= -135 && fy <= 60;
            let stem2 = fx >= 88 && fx <= 108 && fy >= -160 && fy <= 30;
            let beam = fx >= -42 && fx <= 108 && fy >= -160 - (fx - 108) * 25 / 150 && fy <= -115 - (fx - 108) * 25 / 150;
            head1 || head2 || stem1 || stem2 || beam
        }
        IconKind::Calculator => {
            let cx = if fx.abs() > 95 { fx.abs() - 95 } else { 0 };
            let cy = if fy.abs() > 125 { fy.abs() - 125 } else { 0 };
            let in_body = fx.abs() <= 135 && fy.abs() <= 165 && (cx * cx + cy * cy <= 1600);
            let in_lcd = fx.abs() <= 95 && fy >= -125 && fy <= -65;
            let btn_x = fx.abs() >= 25 && fx.abs() <= 95;
            let btn_y = (fy >= -25 && fy <= 35) || (fy >= 65 && fy <= 125);
            in_body && !(in_lcd || (btn_x && btn_y))
        }
        IconKind::Vault => {
            let r2 = fx * fx + (fy + 30) * (fy + 30);
            let arch = r2 >= 2700 && r2 <= 7500 && fy <= -30;
            let legs = fx.abs() >= 52 && fx.abs() <= 86 && fy >= -30 && fy <= 10;
            let cx = if fx.abs() > 100 { fx.abs() - 100 } else { 0 };
            let cy = if fy > 120 { fy - 120 } else { 0 };
            let in_body = fx.abs() <= 140 && fy >= 10 && fy <= 165 && (cx * cx + cy * cy <= 1600);
            let keyhole = (fx * fx + (fy - 65) * (fy - 65) <= 400) || (fx.abs() <= 15 && fy >= 65 && fy <= 120);
            (arch || legs || in_body) && !keyhole
        }
        IconKind::Health => {
            let seg1 = fx >= -165 && fx <= -85 && fy.abs() <= 10;
            let up = fx >= -85 && fx <= -30 && (fy - (-3 * (fx + 85))).abs() <= 15;
            let down = fx >= -30 && fx <= 30 && (fy - (4 * fx)).abs() <= 15;
            let ret = fx >= 30 && fx <= 85 && (fy - (-3 * (fx - 85))).abs() <= 15;
            let seg2 = fx >= 85 && fx <= 165 && fy.abs() <= 10;
            seg1 || up || down || ret || seg2
        }
        IconKind::MeshChat => {
            let n1 = fx * fx + (fy + 95) * (fy + 95) <= 1500;
            let n2 = (fx + 95) * (fx + 95) + (fy - 75) * (fy - 75) <= 1500;
            let n3 = (fx - 95) * (fx - 95) + (fy - 75) * (fy - 75) <= 1500;
            let b1 = fx.abs() <= 12 && fy >= -95 && fy <= 75;
            let b2 = fy >= 63 && fy <= 87 && fx.abs() <= 95;
            n1 || n2 || n3 || b1 || b2
        }
        IconKind::Apps => {
            // 4 rounded squircle tiles in 2x2 grid
            let qx = fx.abs() - 72;
            let qy = fy.abs() - 72;
            let in_box = qx.abs() <= 50 && qy.abs() <= 50;
            let cx = if qx.abs() > 24 { qx.abs() - 24 } else { 0 };
            let cy = if qy.abs() > 24 { qy.abs() - 24 } else { 0 };
            in_box && (cx * cx + cy * cy <= 676)
        }
        IconKind::Calendar => {
            // 2 top binder rings with proper width
            let binder_left = (fx + 55).abs() <= 12 && fy >= -160 && fy <= -90;
            let binder_right = (fx - 55).abs() <= 12 && fy >= -160 && fy <= -90;
            // Rounded calendar sheet body
            let cx = if fx.abs() > 105 { fx.abs() - 105 } else { 0 };
            let cy = if fy < -90 { -90 - fy } else if fy > 120 { fy - 120 } else { 0 };
            let in_sheet = fx.abs() <= 130 && fy >= -115 && fy <= 145 && (cx * cx + cy * cy <= 625);
            // Inner cutout (hollow sheet outline with 20px thickness)
            let icx = if fx.abs() > 85 { fx.abs() - 85 } else { 0 };
            let icy = if fy < -70 { -70 - fy } else if fy > 100 { fy - 100 } else { 0 };
            let in_cutout = fx.abs() <= 110 && fy >= -95 && fy <= 125 && (icx * icx + icy * icy <= 576);
            // Top header bar divider
            let divider = fy >= -42 && fy <= -24 && fx.abs() <= 110;
            binder_left || binder_right || (in_sheet && !in_cutout) || divider
        }
        IconKind::Notes => {
            // WHY the sheet is drawn: the glyph was three rules and a margin line
            // with nothing holding them, which at tile size reads as a music staff
            // rather than as a page of notes. The outline is the same hollow sheet
            // the calendar tile draws, at the same wall thickness.
            let ox = if fx.abs() > 120 { fx.abs() - 120 } else { 0 };
            let oy = if fy.abs() > 150 { fy.abs() - 150 } else { 0 };
            let outer = fx.abs() <= 140 && fy.abs() <= 170 && ox * ox + oy * oy <= 400;
            let ix = if fx.abs() > 100 { fx.abs() - 100 } else { 0 };
            let iy = if fy.abs() > 130 { fy.abs() - 130 } else { 0 };
            let inner = fx.abs() <= 120 && fy.abs() <= 150 && ix * ix + iy * iy <= 400;
            let page = outer && !inner;
            let in_sheet = fx.abs() <= 135 && fy.abs() <= 165;
            let l1 = in_sheet && fx >= -85 && fx <= 85 && (fy >= -60 && fy <= -42);
            let l2 = in_sheet && fx >= -85 && fx <= 85 && (fy >= 0 && fy <= 18);
            let l3 = in_sheet && fx >= -85 && fx <= 35 && (fy >= 60 && fy <= 78);
            let margin = in_sheet && fx >= -105 && fx <= -90 && fy.abs() <= 135;
            page || l1 || l2 || l3 || margin
        }
        IconKind::Contacts => {
            let head = fx * fx + (fy + 55) * (fy + 55) <= 2500;
            let shoulders = fy >= 30 && fy <= 150 && fx.abs() <= 135 && (fx * fx + (fy - 40) * (fy - 40) <= 10000);
            head || shoulders
        }
        IconKind::Maps => {
            let head = fx * fx + (fy + 40) * (fy + 40) <= 6400;
            let hole = fx * fx + (fy + 40) * (fy + 40) <= 1600;
            let tip = fy >= 25 && fy <= 155 && fx.abs() <= (155 - fy) * 8 / 13;
            (head && !hole) || tip
        }
        IconKind::Briefcase => {
            let handle = fx.abs() <= 75 && fy >= -150 && fy <= -75 && !(fx.abs() <= 45 && fy >= -120 && fy <= -75);
            let body = fx.abs() <= 165 && fy >= -65 && fy <= 145;
            let latch = fx.abs() <= 30 && fy >= -25 && fy <= 25;
            handle || (body && !latch)
        }
        IconKind::CardDemo => {
            let body = fx.abs() <= 165 && fy.abs() <= 110;
            let stripe = body && (fy >= -60 && fy <= -25);
            let chip = fx >= -120 && fx <= -60 && fy >= 10 && fy <= 60;
            stripe || chip || (body && (fx.abs() >= 150 || fy.abs() >= 95))
        }
        IconKind::Sun => {
            let r2 = fx * fx + fy * fy;
            // Central solar disc
            if r2 <= 4900 {
                return true;
            }
            // 8 radiant rays
            let cardinal_v = fx.abs() <= 12 && fy.abs() >= 95 && fy.abs() <= 165;
            let cardinal_h = fy.abs() <= 12 && fx.abs() >= 95 && fx.abs() <= 165;
            if cardinal_v || cardinal_h {
                return true;
            }
            let diag1 = (fx - fy).abs() <= 17 && (fx + fy).abs() >= 134 && (fx + fy).abs() <= 233;
            let diag2 = (fx + fy).abs() <= 17 && (fx - fy).abs() >= 134 && (fx - fy).abs() <= 233;
            diag1 || diag2
        }
        IconKind::CheckCircle => {
            let r2 = fx * fx + fy * fy;
            // Outer circular ring
            let is_ring = r2 >= 12544 && r2 <= 20164;
            // Checkmark inside circle
            let short_leg = fx >= -64 && fx <= -12 && (fy - (fx + 67)).abs() <= 15;
            let long_leg = fx >= -18 && fx <= 72 && (fy - (37 - fx)).abs() <= 15;
            let vertex = (fx + 15) * (fx + 15) + (fy - 52) * (fy - 52) <= 225;
            is_ring || short_leg || long_leg || vertex
        }
        IconKind::AgentManager => {
            let ax = fx.abs();
            let ay = fy.abs();
            let in_core = (ax + ay <= 140) && (ax * ay <= 2800);
            let center_node = (fx * fx + fy * fy) <= 1200;
            let sx = (fx - 85).abs();
            let sy = (fy + 85).abs();
            let sat_spark = (sx + sy <= 45) && (sx * sy <= 300);
            in_core || center_node || sat_spark
        }
        IconKind::VideoPlayer => {
            let cx = if fx.abs() > 110 { fx.abs() - 110 } else { 0 };
            let cy = if fy.abs() > 75 { fy.abs() - 75 } else { 0 };
            let in_frame = fx.abs() <= 145 && fy.abs() <= 110 && (cx * cx + cy * cy <= 1225);
            let icx = if fx.abs() > 88 { fx.abs() - 88 } else { 0 };
            let icy = if fy.abs() > 53 { fy.abs() - 53 } else { 0 };
            let in_cutout = fx.abs() <= 123 && fy.abs() <= 88 && (icx * icx + icy * icy <= 900);
            let in_play = fx >= -35 && fx <= 50 && fy.abs() <= (50 - fx) * 50 / 85;
            (in_frame && !in_cutout) || in_play
        }
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
        assert_eq!(match_icon_kind(b"org.interos.vault"), Some(IconKind::Vault));
        assert_eq!(match_icon_kind(b"org.interos.health"), Some(IconKind::Health));
        assert_eq!(match_icon_kind(b"org.interos.meshchat"), Some(IconKind::MeshChat));
        assert_eq!(match_icon_kind(b"org.interos.calendar"), Some(IconKind::Calendar));
        assert_eq!(match_icon_kind(b"org.interos.notes"), Some(IconKind::Notes));
        assert_eq!(match_icon_kind(b"org.interos.video_player"), Some(IconKind::VideoPlayer));
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
                    IconKind::Vault,
            IconKind::Health,
            IconKind::MeshChat,
            IconKind::Apps,
            IconKind::Calendar,
            IconKind::Notes,
            IconKind::Contacts,
            IconKind::Maps,
            IconKind::Briefcase,
            IconKind::CardDemo,
            IconKind::Sun,
            IconKind::CheckCircle,
            IconKind::AgentManager,
            IconKind::VideoPlayer,
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
