#![no_std]
#![no_main]

use core::panic::PanicInfo;
use interos_std as std;
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

/// The colour the page is set on, and the colour its chrome continues into.
///
/// WHY repeated here: `InterWebView` paints its own container colour for every
/// row inside its band and answers nothing at all outside it, so the rows above
/// and below can only be set by the app that owns the surface. Anything other
/// than the page's own colour would read as a second panel behind the document.
const PAGE_BG: u32 = 0xFF121212;

const _: () = assert!(W > 0 && H > W);

/// One pixel of the demo's surface: the document's own pixel where the
/// controller claims the position, and the page background everywhere else.
///
/// WHY the app has to answer the rows the controller does not: the kernel only
/// publishes a surface whose pixels have all been written, and
/// `InterWebView::render` writes the band it was configured with -- rows
/// `y .. y + h` of a 1440-row canvas. The rows outside it stay untouched, so the
/// frame is never marked complete, `composite` skips the surface, and the demo
/// shows nothing at all while logging that it presented a frame.
fn demo_pixel(x: usize, y: usize, page: &InterWebView) -> u32 {
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

    let mut input = std::InputEvent {
        kind: 0,
        x: 0,
        y: 0,
        dx: 0,
        dy: 0,
        scale_milli: 1000,
    };

    loop {
        let events = match std::event_poll(std::EVENT_TOUCH_INPUT | std::EVENT_TIMER_TICK, 100) {
            Ok(ev) => ev,
            Err(_) => break,
        };

        if events & std::EVENT_TOUCH_INPUT != 0 {
            while std::input_event_poll(&mut input).is_ok() {
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
