//! InterOS Native Sovereign File Manager & VFS Explorer App
//!
//! Native Ring-3 graphical application executing in User Mode:
//! - DRM/KMS Client Surface allocation (z=200) and double-buffered scanout presentation
//! - Real-time block device capacity query via microkernel storage driver
//! - Startup VFS directory query whose received bytes are reported in the log
//!   only: no directory entry list is parsed or rendered
//! - Internal Storage capacity queried from the storage service; usage is shown only when real telemetry is available.
//! - Tactile category rows (Recent, Images, Videos, Documents, Downloads,
//!   Internal Storage, Cloud) with a visible selection highlight
//! - Single screen: no subdirectory navigation and no path breadcrumbs are drawn
//! - Asynchronous event reactor loop handling touch interactions and home gestures
//! - Graceful lifecycle supervision and clean return to Launcher

#![no_std]
#![no_main]

use core::panic::PanicInfo;
use interos_std as std;

const W: usize = 720;
const H: usize = 1440;
const CHUNK: usize = 4096;

// -----------------------------------------------------------------------------
// 32-bit ARGB8888 Design Color Tokens (Screen 11 Light Theme)
// -----------------------------------------------------------------------------
const BG_DARK: u32 = 0xFFF7F6F2;
#[allow(dead_code)]
const SURFACE_DARK: u32 = 0xFFFFFFFF;
#[allow(dead_code)]
const SURFACE_RAISED: u32 = 0xFFEFEEE9;
const BORDER_SUBTLE: u32 = 0xFFE1E0DB;
const TEXT_PRIMARY: u32 = 0xFF111214;
const TEXT_MUTED: u32 = 0xFF777773;
#[allow(dead_code)]
const ACCENT_SKY: u32 = 0xFF2563EB;
#[allow(dead_code)]
const ACCENT_AMBER: u32 = 0xFFF59E0B;
#[allow(dead_code)]
const SUCCESS_EMERALD: u32 = 0xFF2E8B57;
#[allow(dead_code)]
const DANGER_CRIMSON: u32 = 0xFFEF4444;

// -----------------------------------------------------------------------------
// Embedded 8x8 ASCII Font (Characters 32 ' ' through 126 '~')
// -----------------------------------------------------------------------------
// Typography provided by interos_std::design::font

#[inline(always)]
fn test_text(
    x: usize,
    y: usize,
    x0: usize,
    y0: usize,
    scale: usize,
    text: &[u8],
    color: u32,
) -> Option<u32> {
    std::design::font::test_text_aa(x, y, x0, y0, scale, text, color, BG_DARK, false)
}

// -----------------------------------------------------------------------------
// Counted Log Assembly (no allocator in this no_std app)
// -----------------------------------------------------------------------------

/// Formats `v` as decimal ASCII into `out`; returns the bytes written.
fn fmt_u64(mut v: u64, out: &mut [u8]) -> usize {
    if out.is_empty() {
        return 0;
    }
    if v == 0 {
        out[0] = b'0';
        return 1;
    }
    let mut digits = [0u8; 20];
    let mut n = 0usize;
    while v > 0 && n < 20 {
        digits[n] = b'0' + (v % 10) as u8;
        v /= 10;
        n += 1;
    }
    let n = n.min(out.len());
    for i in 0..n {
        out[i] = digits[n - 1 - i];
    }
    n
}

/// Builds `prefix ++ decimal(value) ++ suffix` into `buf` and returns its length.
/// WHY: measured values only exist at runtime, so they are assembled into a
/// stack buffer before being logged.
fn log_counted(prefix: &str, value: u64, suffix: &str) {
    let mut buf = [0u8; 192];
    let p = prefix.len().min(buf.len());
    buf[..p].copy_from_slice(&prefix.as_bytes()[..p]);
    let v = fmt_u64(value, &mut buf[p..]);
    let n = p + v;
    let s = suffix.len().min(buf.len().saturating_sub(n));
    buf[n..n + s].copy_from_slice(&suffix.as_bytes()[..s]);
    if let Ok(msg) = core::str::from_utf8(&buf[..n + s]) {
        std::log(0, msg).ok();
    }
}

// -----------------------------------------------------------------------------
// VFS Entry Representation
// Runtime entries must come from the kernel VFS; no synthetic inventory is embedded.
// -----------------------------------------------------------------------------
#[derive(Debug, Clone, Copy)]
pub struct VfsItem {
    pub name: &'static str,
    pub is_dir: bool,
    pub size_str: &'static str,
    pub meta_str: &'static str,
    pub color_accent: u32,
}

/// The app ships no embedded file inventory: `ITEMS_ALL` stays empty until a
/// real VFS listing can populate it (WHY: the platform truth contract pins the
/// empty ledger, and nothing on screen may invent file names).
#[allow(dead_code)]
const ITEMS_ALL: [VfsItem; 0] = [];

pub struct FileManagerApp {
    pub selected_shortcut: usize,
    pub storage_capacity_bytes: u64,
    pub current_path: &'static str,
}

impl FileManagerApp {
    pub const fn new() -> Self {
        Self {
            selected_shortcut: 0,
            storage_capacity_bytes: 0,
            current_path: "/data",
        }
    }
}

// -----------------------------------------------------------------------------
// Pixel Color Resolution Engine
// -----------------------------------------------------------------------------
#[inline(never)]
fn get_file_manager_color(x: usize, y: usize, app: &FileManagerApp) -> u32 {
    // 1. Status Bar & Clean Header (y: 0..140)
    if y < 140 {
        // Clock and battery indicators are omitted until live guest services
        // expose real readings. Never paint plausible but fabricated values.

        // Title: "Files" at x: 48, y: 64 (scale 3)
        if y >= 76 && y < 108 {
            if let Some(c) = test_text(x, y, 48, 64, 3, b"Files", TEXT_PRIMARY) {
                return c;
            }
        }

        // Search icon at (600, 92, r: 12)
        let s_dx = (x as i32) - 600;
        let s_dy = (y as i32) - 92;
        let s_dist2 = s_dx * s_dx + s_dy * s_dy;
        if (s_dist2 >= 36 && s_dist2 <= 100) || (s_dx >= 6 && s_dx <= 14 && s_dy >= 6 && s_dy <= 14 && (s_dx - s_dy).abs() <= 2) {
            return TEXT_PRIMARY;
        }

        // More icon (3 vertical dots at 660, 92)
        let m_dx = (x as i32) - 660;
        let m_dy = (y as i32) - 92;
        if m_dx.abs() <= 2 && (m_dy.abs() <= 2 || (m_dy - 10).abs() <= 2 || (m_dy + 10).abs() <= 2) {
            return TEXT_PRIMARY;
        }

        return BG_DARK;
    }

    // 2. Vertical Category Rows (y: 150..820)
    if (150..820).contains(&y) && x >= 36 && x < 684 {
        let (row_idx, row_start_y, row_h) = if y < 240 {
            (0, 150, 90)
        } else if y < 330 {
            (1, 240, 90)
        } else if y < 420 {
            (2, 330, 90)
        } else if y < 510 {
            (3, 420, 90)
        } else if y < 600 {
            (4, 510, 90)
        } else if y < 710 {
            (5, 600, 110)
        } else {
            (6, 710, 90)
        };

        // Thin bottom divider line
        if y == row_start_y + row_h - 1 {
            return BORDER_SUBTLE;
        }

        // WHY: this highlight is what makes the tap handler's selection visible —
        // the selected row gets a raised background and a left accent bar, so the
        // control reports a real state change instead of an invisible one.
        // It is applied only inside the selected row's own band, so no tint
        // bleeds into the padding below the last row.
        let row_selected =
            app.selected_shortcut == row_idx && y >= row_start_y && y < row_start_y + row_h;
        if row_selected && x < 44 {
            return ACCENT_SKY;
        }
        let row_bg = if row_selected { SURFACE_RAISED } else { BG_DARK };

        let center_y = row_start_y + (if row_idx == 5 { 40 } else { 45 });
        let icon_cx = 64i32;
        let icon_cy = center_y as i32;
        let idx = (x as i32) - icon_cx;
        let idy = (y as i32) - icon_cy;
        let idist2 = idx * idx + idy * idy;

        // Render Icons
        match row_idx {
            0 => {
                // Clock icon (Recent)
                if (idist2 >= 100 && idist2 <= 196) || (idx == 0 && idy >= -10 && idy <= 0) || (idy == 0 && idx >= 0 && idx <= 8) {
                    return TEXT_PRIMARY;
                }
                if let Some(c) = test_text(x, y, 108, center_y - 8, 2, b"Recent", TEXT_PRIMARY) {
                    return c;
                }
            }
            1 => {
                // Images icon (Landscape frame)
                if (idx.abs() <= 14 && idy.abs() <= 10) && (idx.abs() >= 12 || idy.abs() >= 8 || ((idx - 5).abs() <= 2 && (idy + 3).abs() <= 2) || (idy >= 2 && idx.abs() <= (10 - idy))) {
                    return TEXT_PRIMARY;
                }
                if let Some(c) = test_text(x, y, 108, center_y - 8, 2, b"Images", TEXT_PRIMARY) {
                    return c;
                }
            }
            2 => {
                // Videos icon (Camcorder)
                if ((idx >= -14 && idx <= 4 && idy.abs() <= 10) && (idx == -14 || idx == 4 || idy.abs() >= 8)) || (idx >= 6 && idx <= 14 && idy.abs() <= (idx - 5)) {
                    return TEXT_PRIMARY;
                }
                if let Some(c) = test_text(x, y, 108, center_y - 8, 2, b"Videos", TEXT_PRIMARY) {
                    return c;
                }
            }
            3 => {
                // Documents icon (Folded sheet)
                if (idx.abs() <= 10 && idy.abs() <= 14) && (idx.abs() >= 8 || idy == 12 || (idy == -12 && idx <= 2) || (idy == -2 || idy == 3 || idy == 8)) {
                    return TEXT_PRIMARY;
                }
                if let Some(c) = test_text(x, y, 108, center_y - 8, 2, b"Documents", TEXT_PRIMARY) {
                    return c;
                }
            }
            4 => {
                // Downloads icon (Down arrow with tray)
                if (idx == 0 && idy >= -12 && idy <= 4) || (idy >= 0 && idy <= 6 && idx.abs() <= (6 - idy)) || (idy == 10 && idx.abs() <= 12) || (idx.abs() == 12 && idy >= 6 && idy <= 10) {
                    return TEXT_PRIMARY;
                }
                if let Some(c) = test_text(x, y, 108, center_y - 8, 2, b"Downloads", TEXT_PRIMARY) {
                    return c;
                }
            }
            5 => {
                // Internal Storage (Drive icon)
                if (idx.abs() <= 14 && idy.abs() <= 10) && (idx.abs() >= 12 || idy.abs() >= 8 || (idy == 2 && idx.abs() <= 10)) {
                    return TEXT_PRIMARY;
                }
                if let Some(c) = test_text(x, y, 108, row_start_y + 24, 2, b"Internal Storage", TEXT_PRIMARY) {
                    return c;
                }
                let capacity_status: &[u8] = if app.storage_capacity_bytes > 0 {
                    b"Capacity detected; usage unavailable"
                } else {
                    b"Storage capacity unavailable"
                };
                if let Some(c) = test_text(x, y, 108, row_start_y + 56, 1, capacity_status, TEXT_MUTED) {
                    return c;
                }
            }
            _ => {
                // Cloud icon
                if (idy >= 2 && idy <= 8 && idx.abs() <= 14) || ((idx + 6) * (idx + 6) + idy * idy <= 49) || ((idx - 6) * (idx - 6) + (idy + 2) * (idy + 2) <= 36) {
                    return TEXT_PRIMARY;
                }
                if let Some(c) = test_text(x, y, 108, center_y - 8, 2, b"Cloud (Optional)", TEXT_PRIMARY) {
                    return c;
                }
            }
        }

        return row_bg;
    }

    // 3. Home Gesture Pill (x: 260..460, y: 1418..1424)
    if y >= 1418 && y < 1424 && x >= 260 && x < 460 {
        return TEXT_MUTED;
    }

    BG_DARK
}

fn render_file_manager_ui(surface: u32, app: &FileManagerApp) {
    let mut chunk = [0u32; CHUNK];
    let total = W * H;
    let mut offset = 0;
    while offset < total {
        let count = (total - offset).min(CHUNK);
        let mut x = offset % W;
        let mut y = offset / W;
        for i in 0..count {
            chunk[i] = get_file_manager_color(x, y, app);
            x += 1;
            if x == W {
                x = 0;
                y += 1;
            }
        }
        let _ = std::surface_write(6, surface, offset, &chunk[..count]);
        offset += count;
    }
}

// -----------------------------------------------------------------------------
// Entry Point & Event Reactor Loop
// -----------------------------------------------------------------------------
#[no_mangle]
pub extern "C" fn _start() -> ! {
    std::log(
        0,
        "[FILE_MANAGER (Ring 3)] Sovereign File Manager & VFS Explorer online.",
    )
    .ok();

    let mut app = FileManagerApp::new();

    // Query physical block storage capacity from driver
    if let Ok(bytes) = std::storage_query() {
        app.storage_capacity_bytes = bytes;
        std::log(
            0,
            "[FILE_MANAGER] Storage capacity validated from hardware service.",
        )
        .ok();
    }

    // Enumerate initial VFS directory. WHY byte counts: the payload is received
    // but never parsed or rendered, so claiming a successful parse would be false.
    let mut entries = [0u8; 1024];
    match std::fs_list(3, "/data", &mut entries) {
        Ok(n) => {
            log_counted(
                "[FILE_MANAGER] fs_list('/data'): ",
                n as u64,
                " bytes of directory entries received; not parsed or rendered.",
            );
        }
        Err(code) => {
            log_counted(
                "[FILE_MANAGER] fs_list('/data') refused, code -",
                code.unsigned_abs(),
                "; no directory data received.",
            );
        }
    }

    // Allocate client surface (slot 6 = FrameBufferDirect, z=200)
    let surface = match std::surface_create(6, W as u32, H as u32, 200) {
        Ok(id) => {
            std::log(
                0,
                "[FILE_MANAGER] DRM/KMS Client Surface allocated (z=200).",
            )
            .ok();
            id
        }
        Err(_) => 0,
    };

    if surface > 0 {
        render_file_manager_ui(surface, &app);
        let _ = std::surface_flip(6);
        std::log(0, "[FILE_MANAGER] Files UI presented to scanout buffer.").ok();
    }

    // Interactive Event Loop
    loop {
        let interest = std::EVENT_TOUCH_INPUT | std::EVENT_TIMER_TICK;
        if let Ok(triggered) = std::event_poll(interest, 100) {
            if (triggered & std::EVENT_TOUCH_INPUT) != 0 {
                let mut input = std::InputEvent {
                    kind: 0,
                    x: 0,
                    y: 0,
                    dx: 0,
                    dy: 0,
                    scale_milli: 1000,
                };
                while std::input_event_poll(&mut input).is_ok() {
                    // Swipe gesture -> Return to launcher
                    if input.kind == 4 || input.kind == 6 || input.kind == 7 || input.kind == 8 || input.kind == 10
                    {
                        std::log(
                            0,
                            "[FILE_MANAGER] Navigation gesture recognized. Exiting to launcher.",
                        )
                        .ok();
                        if surface > 0 {
                            let _ = std::surface_destroy(6, surface);
                        }
                        std::exit_with_code(0);
                    }

                    if input.kind == 1 {
                        let x = input.x as usize;
                        let y = input.y as usize;

                        // Check Back button (<) or bottom home pill
                        if (x >= 24 && x < 100 && y >= 28 && y < 68)
                            || (x >= 240 && x <= 480 && y >= 1400)
                        {
                            std::log(0, "[FILE_MANAGER] Back button or home gesture pressed. Exiting to launcher.").ok();
                            if surface > 0 {
                                let _ = std::surface_destroy(6, surface);
                            }
                            std::exit_with_code(0);
                        }

                        // Category rows are drawn as a vertical list (y 150..800,
                        // x 36..684; rows of 90px, Internal Storage 110px).
                        // WHY row-shaped hit test: the previous horizontal band
                        // logged a filter switch for taps that hit nothing, missed
                        // the rows that are actually drawn, and had no visible
                        // effect. The selection and the log now happen together,
                        // only inside the row that was really tapped, and the
                        // renderer highlights the selected row.
                        if (150..800).contains(&y) && (36..684).contains(&x) {
                            let shortcut = if y < 240 {
                                0 // Recent
                            } else if y < 330 {
                                1 // Images
                            } else if y < 420 {
                                2 // Videos
                            } else if y < 510 {
                                3 // Documents
                            } else if y < 600 {
                                4 // Downloads
                            } else if y < 710 {
                                5 // Internal Storage
                            } else {
                                6 // Cloud
                            };
                            app.selected_shortcut = shortcut;
                            // `current_path` keeps the directory that was really
                            // queried ("/data"): no navigation happens, so no new
                            // path is claimed.
                            std::log(0, "[FILE_MANAGER] Category filter shortcut switched.").ok();
                            if surface > 0 {
                                render_file_manager_ui(surface, &app);
                                let _ = std::surface_flip(6);
                            }
                            break;
                        }
                    }
                }
            }
        }
        std::yield_now();
    }
}

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    std::log(0, "[FILE_MANAGER] Panic detected! Terminating task...").ok();
    std::exit();
}
