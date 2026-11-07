// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

#![no_std]
#![no_main]

use core::panic::PanicInfo;
use interos_std as std;
use std::design::back_button;
use std::design::home_band;
use std::design::monochrome;
use std::design::viewport;
use std::webview::InterWebView;
use webview_demo::WebViewDemoApp;

/// The canvas this app is asked to answer, in the platform's own logical units.
///
/// WHY from the viewport module and not typed: the surface is created at exactly
/// these dimensions and the kernel's publication rule is about them -- see
/// `render_demo`. Two numbers copied from a header file agree only until one of
/// them is edited, and the failure is silent: the surface simply never appears.
const W: usize = viewport::LOGICAL_WIDTH;
const H: usize = viewport::LOGICAL_HEIGHT;

/// The theme the back control is cut from: the dark half of the platform's
/// monochrome ladder, because this app's band is dark.
///
/// WHY not the light one the rest of the fleet hands `pixel_for_theme`: the chip
/// takes its ring, fill and ink from the theme it is given, and the light roles
/// put a `#EFEFE9` plate with a `#0A0A0A` outline on this app's `#121212` band --
/// a ring one hundredth of a step off the band it stands on, so the control
/// reads as a lit panel rather than a button. The dark variant trades paper for
/// ink: the plate lifts to `#232323` (1.19:1 over the band) and the ring and the
/// chevron carry `#FAFAFA`, 17.95:1 against the band and 15.06:1 against the
/// plate. All three are roles of one shipped theme; none is a colour here.
const THEME: std::design::Theme = monochrome::THEME_DARK;

/// The colour the page is set on, and the colour its chrome continues into.
///
/// WHY repeated here: `InterWebView` paints its own container colour for every
/// row inside its band and answers nothing at all outside it, so the rows above
/// and below can only be set by the app that owns the surface. Anything other
/// than the page's own colour would read as a second panel behind the document.
const PAGE_BG: u32 = 0xFF121212;

const _: () = assert!(W > 0 && H > W);

/// One pixel of the demo's surface: the platform's back control where it stands,
/// then the home pill the band answers, then the document's own pixel where the
/// controller claims the position, and the page background everywhere else.
///
/// WHY the pill is the SDK's ink and not a shape here: the demo painted nothing at
/// the foot of the panel, so the only way off the document was a gesture the
/// screen never advertised. The mark and the box that answers it are now one
/// declaration, and the rows they cover are the app's own background -- the
/// controller's band stops at row 1340, well above the pill.
///
/// WHY the app has to answer the rows the controller does not: the kernel only
/// publishes a surface whose pixels have all been written, and
/// `InterWebView::render` writes the band it was configured with -- rows
/// `y .. y + h` of a 1440-row canvas. The rows outside it stay untouched, so the
/// frame is never marked complete, `composite` skips the surface, and the demo
/// shows nothing at all while logging that it presented a frame.
///
/// WHY the control is asked before the document: the webview band starts at row
/// 84 and the control owns rows 56..104, so the top of the chip overlaps the
/// document. A control that a page can paint over is a control that isn't there.
fn demo_pixel(x: usize, y: usize, page: &InterWebView) -> u32 {
    if let Some(c) = back_button::pixel_for_theme(x, y, &THEME) {
        return c;
    }
    if let Some(c) = home_band::pixel(x, y, THEME.on_surface_variant) {
        return c;
    }

    let px0 = page.x as usize;
    let py0 = page.y as usize;
    let px1 = px0 + page.w as usize;
    let py1 = py0 + page.h as usize;

    if x >= px0 && x < px1 && y >= py0 && y < py1 {
        let c = page.get_pixel(x, y);
        // Zero alpha is the controller's own way of saying the position is not
        // its ink; it is never written into an opaque surface.
        if c & 0xFF000000 != 0 {
            return c;
        }
    }
    PAGE_BG
}

/// Presents a whole frame, row by row, so every pixel of the canvas is written.
///
/// WHY row-ordered chunks rather than the controller's own `render`: that call
/// stops at the edge of the document's band. Writing one full row per call covers
/// all `H` rows exactly once, which is the condition the kernel's surface
/// bookkeeping needs before it will present a frame at all.
///
/// WHY no heap: the buffer is one row of the canvas, sized by the same constant
/// the surface was created with.
fn render_demo(surface: u32, page: &InterWebView) {
    let mut row = [0u32; W];
    for y in 0..H {
        for x in 0..W {
            row[x] = demo_pixel(x, y, page);
        }
        let _ = std::surface_write(surface, y * W, &row);
    }
}

#[no_mangle]
pub extern "C" fn _start() -> ! {
    std::log("[WEBVIEW DEMO] Application starting in Ring 3.").ok();

    let surface = match std::surface_create(W as u32, H as u32, 200) {
        Ok(s) => s,
        Err(_) => std::exit_with_code(1),
    };

    let mut app = match WebViewDemoApp::new() {
        Ok(a) => a,
        Err(_) => std::exit_with_code(2),
    };

    // Render initial page
    render_demo(surface, &app.webview);
    std::surface_flip().ok();
    std::log("[WEBVIEW DEMO] Initial frame presented to hardware scanout.").ok();

    let mut last_scale = std::design::font::user_text_scale();

    let mut input = std::InputEvent {
        kind: 0,
        x: 0,
        y: 0,
        dx: 0,
        dy: 0,
        scale_milli: 1000,
    };

    loop {
        let cur_scale = std::design::font::user_text_scale();
        if cur_scale != last_scale {
            last_scale = cur_scale;
            render_demo(surface, &app.webview);
            std::surface_flip().ok();
        }

        let events = match std::event_poll(std::EVENT_TOUCH_INPUT | std::EVENT_TIMER_TICK, 100) {
            Ok(ev) => ev,
            Err(_) => break,
        };

        if events & std::EVENT_TOUCH_INPUT != 0 {
            while std::input_event_poll(&mut input).is_ok() {
                if input.kind == 1 && back_button::contains(input.x as usize, input.y as usize) {
                    // WHY answered before the controller and before the gesture
                    // exit: a tap where the control is painted is the control's,
                    // and the document underneath it cannot claim the point.
                    if app.at_first_screen() {
                        std::log("[WEBVIEW_DEMO] Back control pressed. Exiting to launcher.").ok();
                        std::surface_destroy(surface).ok();
                        std::exit_with_code(0);
                    }
                    if app.show_first_screen().is_ok() {
                        std::log("[WEBVIEW_DEMO] Back control returned to the demo document.").ok();
                        render_demo(surface, &app.webview);
                        std::surface_flip().ok();
                    }
                    continue;
                }

                if input.kind == 1 && home_band::contains(input.x as usize, input.y as usize) {
                    // The pill painted at the foot of the panel: go home, rather
                    // than unwind the document the way the back control does.
                    std::log("[WEBVIEW_DEMO] Home gesture pressed. Exiting to launcher.").ok();
                    std::surface_destroy(surface).ok();
                    std::exit_with_code(0);
                }

                if input.kind == 4 || input.kind == 10 {
                    // Back gesture -> Exit application
                    std::surface_destroy(surface).ok();
                    std::exit_with_code(0);
                }

                if app.webview.handle_touch(&input) {
                    render_demo(surface, &app.webview);
                    std::surface_flip().ok();
                }
            }
        }
    }

    std::surface_destroy(surface).ok();
    std::exit_with_code(0);
}

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    std::exit_with_code(1)
}
