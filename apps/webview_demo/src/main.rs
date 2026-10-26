#![no_std]
#![no_main]

use core::panic::PanicInfo;
use interos_std as std;
use webview_demo::WebViewDemoApp;

const W: usize = 720;
const H: usize = 1440;

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
    app.webview.render(surface);
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
                    app.webview.render(surface);
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
