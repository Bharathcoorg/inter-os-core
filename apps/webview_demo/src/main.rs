// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

#![no_std]
#![no_main]

use core::panic::PanicInfo;
use interos_std as std;
use std::design::back_button;
use std::design::home_band;
use std::design::viewport;
use std::webview::InterWebView;
use webview_demo::{demo_pixel, WebViewDemoApp, KERNEL_TAP};

/// The canvas this app is asked to answer, in the platform's own logical units.
///
/// WHY from the viewport module and not typed: the surface is created at exactly
/// these dimensions and the kernel's publication rule is about them -- see
/// `render_demo`. Two numbers copied from a header file agree only until one of
/// them is edited, and the failure is silent: the surface simply never appears.
const W: usize = viewport::LOGICAL_WIDTH;
const H: usize = viewport::LOGICAL_HEIGHT;

const _: () = assert!(W > 0 && H > W);

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
                if input.kind == KERNEL_TAP && back_button::contains(input.x as usize, input.y as usize) {
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

                if input.kind == KERNEL_TAP && home_band::contains(input.x as usize, input.y as usize) {
                    // The pill painted at the foot of the panel: go home, rather
                    // than unwind the document the way the back control does.
                    std::log("[WEBVIEW_DEMO] Home gesture pressed. Exiting to launcher.").ok();
                    std::surface_destroy(surface).ok();
                    std::exit_with_code(0);
                }

                // The document answers the gesture next. It used to be reached
                // only through the controller's own touch path, and that path
                // moves a page on a `2` -- which the kernel calls a double tap,
                // and which carries no delta at all -- while it left the `4` a
                // drag really arrives as to the branch above, which read it as
                // "the back gesture" and closed the app. A reader who scrolled
                // this page was thrown out of the demo, and a reader who did not
                // could never reach its last lines.
                if app.handle_document_input(&input) {
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
