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
}

/// Identifies if a package ID or process name maps to a first-party vector pictogram.
pub fn match_icon_kind(id: &[u8]) -> Option<IconKind> {
    if id.is_empty() {
        return None;
    }
    // Match package IDs, short process names, or badge acronyms
    if id == b"org.interos.dialer" || id == b"dialer" || id == b"app_dialer" || id == b"CALL" || id == b"TEL"
        || id == b"Phone" || id == b"phone"
    {
        Some(IconKind::Phone)
    } else if id == b"org.interos.messages" || id == b"messages" || id == b"app_messages" || id == b"MSG" || id == b"SMS"
        || id == b"Messages" || id == b"sms"
    {
        Some(IconKind::Messages)
    } else if id == b"org.interos.camera" || id == b"camera" || id == b"app_camera"
        || id == b"org.interos.camera_preview" || id == b"camera_preview" || id == b"CAM"
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
        || id == b"Gallery" || id == b"Photos"
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
        || id == b"org.interos.authenticator" || id == b"authenticator" || id == b"VAUL" || id == b"AUTH" || id == b"PASS"
        || id == b"Vault" || id == b"Authenticator"
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
    } else if id == b"org.interos.calendar" || id == b"calendar" || id == b"Calendar" || id == b"CAL" {
        Some(IconKind::Calendar)
    } else if id == b"org.interos.notes" || id == b"notes" || id == b"Notes" || id == b"NOTE" {
        Some(IconKind::Notes)
    } else if id == b"org.interos.contacts" || id == b"contacts" || id == b"Contacts" || id == b"CONT" {
        Some(IconKind::Contacts)
    } else if id == b"org.interos.maps" || id == b"maps" || id == b"Maps" || id == b"MAP" {
        Some(IconKind::Maps)
    } else {
        None
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
            // Handset silhouette: earpiece, grip, mouthpiece angled gently
            let top_earpiece = rx >= -5 && rx <= 5 && ry >= -12 && ry <= -8;
            let bottom_mouthpiece = rx >= -5 && rx <= 5 && ry >= 8 && ry <= 12;
            let handle = rx >= -3 && rx <= 1 && ry >= -9 && ry <= 9;
            top_earpiece || bottom_mouthpiece || handle
        }
        IconKind::Messages => {
            // Rounded speech bubble with bottom-left tail
            let bubble = rx >= -11 && rx <= 11 && ry >= -10 && ry <= 5;
            let tail = (ry >= 5 && ry <= 11) && (rx >= -11 && rx <= -5) && (ry - rx <= 17);
            let not_corner = (rx == -11 || rx == 11) && (ry == -10 || ry == 5);
            // 3 small dots inside
            let is_dot = (ry >= -3 && ry <= -1) && (
                (rx >= -6 && rx <= -4) || (rx >= -1 && rx <= 1) || (rx >= 4 && rx <= 6)
            );
            (bubble || tail) && !not_corner && !is_dot
        }
        IconKind::Camera => {
            // Viewfinder body, lens ring, top shutter bump
            let body = rx >= -12 && rx <= 12 && ry >= -7 && ry <= 10;
            let shutter = rx >= -9 && rx <= -4 && ry >= -10 && ry <= -7;
            let lens_r2 = rx * rx + ry * ry;
            let lens_cutout = lens_r2 <= 20; // 4.5^2
            let lens_pupil = lens_r2 <= 4;
            (body || shutter) && (!lens_cutout || lens_pupil)
        }
        IconKind::Browser => {
            // Globe with circular rim, equator, and central meridian
            let r2 = rx * rx + ry * ry;
            let in_circle = r2 <= 121; // 11^2
            let rim = r2 >= 81 && in_circle; // 9^2 .. 11^2
            let equator = in_circle && (ry >= -1 && ry <= 1);
            let meridian = in_circle && ((rx * 2) * (rx * 2) + ry * ry <= 121) && ((rx * 2) * (rx * 2) + ry * ry >= 49);
            rim || equator || meridian
        }
        IconKind::Files => {
            // Folder with top tab and main body
            let tab = rx >= -12 && rx <= -3 && ry >= -10 && ry <= -6;
            let body = rx >= -12 && rx <= 12 && ry >= -6 && ry <= 9;
            let fold_line = (ry == -2) && (rx >= -10 && rx <= 10);
            (tab || body) && !fold_line
        }
        IconKind::Gallery => {
            // Frame with sun circle and mountain slopes
            let rim = (rx >= -11 && rx <= 11 && ry >= -9 && ry <= 9)
                && !(rx >= -9 && rx <= 9 && ry >= -7 && ry <= 7);
            let sun = (rx - 5) * (rx - 5) + (ry + 3) * (ry + 3) <= 4;
            // Mountain peak slopes
            let peak1 = (ry >= 1 && ry <= 7) && (rx >= -8 && rx <= 2) && (7 - ry >= (rx + 3).abs());
            let peak2 = (ry >= 3 && ry <= 7) && (rx >= 0 && rx <= 8) && (7 - ry >= (rx - 4).abs());
            rim || sun || peak1 || peak2
        }
        IconKind::Settings => {
            // Gear: outer ring with 8 teeth and central circular cutout
            let r2 = rx * rx + ry * ry;
            let rim = r2 >= 16 && r2 <= 64; // r in 4..8
            let tooth_h = (rx.abs() <= 11 && ry.abs() <= 2) && r2 >= 36;
            let tooth_v = (rx.abs() <= 2 && ry.abs() <= 11) && r2 >= 36;
            let tooth_d1 = ((rx - ry).abs() <= 2 && (rx + ry).abs() <= 12) && r2 >= 36;
            let tooth_d2 = ((rx + ry).abs() <= 2 && (rx - ry).abs() <= 12) && r2 >= 36;
            (rim || tooth_h || tooth_v || tooth_d1 || tooth_d2) && r2 >= 9
        }
        IconKind::Music => {
            // Beamed pair of eighth notes
            let note1 = (rx + 5) * (rx + 5) + (ry - 5) * (ry - 5) <= 9;
            let note2 = (rx - 4) * (rx - 4) + (ry - 3) * (ry - 3) <= 9;
            let stem1 = (rx >= -3 && rx <= -2) && (ry >= -9 && ry <= 5);
            let stem2 = (rx >= 6 && rx <= 7) && (ry >= -11 && ry <= 3);
            let beam = (rx >= -3 && rx <= 7) && (ry >= -11 && ry <= -8);
            note1 || note2 || stem1 || stem2 || beam
        }
        IconKind::Calculator => {
            // Minimal 2x2 grid representing +, -, ×, =
            let plus = ((rx >= -8 && rx <= -2) && ry == -5) || (rx == -5 && (ry >= -8 && ry <= -2));
            let minus = (rx >= 2 && rx <= 8) && (ry >= -6 && ry <= -4);
            let mul = (rx + 5).abs() == (ry - 5).abs() && (rx + 5).abs() <= 3;
            let eq = (rx >= 2 && rx <= 8) && (ry == 3 || ry == 6);
            plus || minus || mul || eq
        }
        IconKind::Terminal => {
            // Prompt chevron `>` and cursor `_`
            let chevron = (rx >= -9 && rx <= -3) && (ry.abs() <= (rx + 9) && (rx + 9) - ry.abs() <= 1);
            let cursor = (rx >= 0 && rx <= 8) && (ry >= 4 && ry <= 6);
            chevron || cursor
        }
        IconKind::Vault => {
            // Shield outline with center keyhole
            let in_shield = (rx >= -10 && rx <= 10 && ry >= -10 && ry <= 0)
                || ((ry > 0 && ry <= 11) && ry <= 11 - rx.abs());
            let key_circle = rx * rx + (ry + 2) * (ry + 2) <= 4;
            let key_slot = rx.abs() <= 1 && (ry >= 0 && ry <= 4);
            in_shield && !(key_circle || key_slot)
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
            // Radar arcs radiating from bottom center
            let r2 = rx * rx + (ry - 8) * (ry - 8);
            let arc1 = r2 >= 36 && r2 <= 49 && ry <= 8;
            let arc2 = r2 >= 100 && r2 <= 121 && ry <= 8;
            let arc3 = r2 >= 196 && r2 <= 225 && ry <= 8;
            arc1 || arc2 || arc3
        }
        IconKind::Apps => {
            // 4 rounded square tiles (2x2 grid) matching reference Screen 2 Apps tile
            let in_tile_x = (rx >= -9 && rx <= -2) || (rx >= 2 && rx <= 9);
            let in_tile_y = (ry >= -9 && ry <= -2) || (ry >= 2 && ry <= 9);
            in_tile_x && in_tile_y
        }
        IconKind::Calendar => {
            // Calendar with top header bar, hanger loops, and date body
            let body = rx >= -10 && rx <= 10 && ry >= -7 && ry <= 10;
            let loops = (rx == -5 || rx == 5) && (ry >= -10 && ry <= -7);
            let header_bar = rx >= -10 && rx <= 10 && (ry >= -7 && ry <= -3);
            let date_line1 = (rx >= -6 && rx <= 6) && (ry == 1);
            let date_line2 = (rx >= -6 && rx <= 2) && (ry == 5);
            (body || loops) && (!header_bar || ry >= -5) && (header_bar || date_line1 || date_line2)
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
