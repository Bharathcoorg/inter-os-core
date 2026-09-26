#![no_std]
#![no_main]

use core::panic::PanicInfo;
use interos_std as std;

const W: usize = 720;
const H: usize = 1440;
const CHUNK: usize = 4096;

fn render_full(surface: u32) {
    let mut chunk = [0u32; CHUNK];
    let total = W * H;
    let mut offset = 0;
    while offset < total {
        let count = (total - offset).min(CHUNK);
        let mut x = offset % W;
        let mut y = offset / W;
        for i in 0..count {
            if y < 96 {
                // Status Bar / Header
                chunk[i] = if y >= 94 { std::design::color::ACCENT } else { std::design::color::SURFACE };
            } else if (112..232).contains(&y) && x >= 24 && x < W - 24 {
                // Hero Widget / Clock & Search Banner
                chunk[i] = if y >= 230 { std::design::color::ACCENT } else { std::design::color::SURFACE };
            } else if (280..420).contains(&y) {
                // Row 0: Settings (Card 0) & Terminal (Card 1)
                if x >= 56 && x < 306 {
                    chunk[i] = if (330..394).contains(&y) && x >= 149 && x < 213 {
                        std::design::color::ACCENT
                    } else {
                        std::design::color::SURFACE
                    };
                } else if x >= 360 && x < 610 {
                    chunk[i] = if (330..394).contains(&y) && x >= 453 && x < 517 {
                        std::design::color::SUCCESS
                    } else {
                        std::design::color::SURFACE
                    };
                } else {
                    chunk[i] = std::design::color::BACKGROUND;
                }
            } else if (450..590).contains(&y) {
                // Row 1: Calculator (Card 2) & Vault (Card 3)
                if x >= 56 && x < 306 {
                    chunk[i] = if (500..564).contains(&y) && x >= 149 && x < 213 {
                        std::design::color::WARNING
                    } else {
                        std::design::color::SURFACE
                    };
                } else if x >= 360 && x < 610 {
                    chunk[i] = if (500..564).contains(&y) && x >= 453 && x < 517 {
                        std::design::color::DANGER
                    } else {
                        std::design::color::SURFACE
                    };
                } else {
                    chunk[i] = std::design::color::BACKGROUND;
                }
            } else if (620..760).contains(&y) {
                // Row 2: Camera (Card 4) & Dialer (Card 5)
                if x >= 56 && x < 306 {
                    chunk[i] = if (670..734).contains(&y) && x >= 149 && x < 213 {
                        std::design::color::ACCENT
                    } else {
                        std::design::color::SURFACE
                    };
                } else if x >= 360 && x < 610 {
                    chunk[i] = if (670..734).contains(&y) && x >= 453 && x < 517 {
                        std::design::color::SUCCESS
                    } else {
                        std::design::color::SURFACE
                    };
                } else {
                    chunk[i] = std::design::color::BACKGROUND;
                }
            } else if (790..930).contains(&y) {
                // Row 3: Browser (Card 6) & Messages (Card 7)
                if x >= 56 && x < 306 {
                    chunk[i] = if (840..904).contains(&y) && x >= 149 && x < 213 {
                        std::design::color::TEXT_MUTED
                    } else {
                        std::design::color::SURFACE
                    };
                } else if x >= 360 && x < 610 {
                    chunk[i] = if (840..904).contains(&y) && x >= 453 && x < 517 {
                        std::design::color::WARNING
                    } else {
                        std::design::color::SURFACE
                    };
                } else {
                    chunk[i] = std::design::color::BACKGROUND;
                }
            } else if (1260..1400).contains(&y) && x >= 24 && x < W - 24 {
                // Bottom Quick-Launch Dock
                let is_dock_icon = (x >= 60 && x < 124) || (x >= 220 && x < 284)
                    || (x >= 380 && x < 444) || (x >= 540 && x < 604);
                if (1298..1362).contains(&y) && is_dock_icon {
                    chunk[i] = std::design::color::ACCENT;
                } else {
                    chunk[i] = std::design::color::SURFACE_RAISED;
                }
            } else {
                chunk[i] = std::design::color::BACKGROUND;
            }
            x += 1;
            if x == W {
                x = 0;
                y += 1;
            }
        }
        let _ = std::surface_write(4, surface, offset, &chunk[..count]);
        offset += count;
    }
    std::log(0, "[LAUNCHER] render_full complete: 8-app home screen canvas initialized.").ok();
}

fn render_header(surface: u32) {
    let mut chunk = [0u32; CHUNK];
    let total = W * 96;
    let mut offset = 0;
    let ticks = std::get_ticks(1).unwrap_or(0);
    let activity = ((ticks % 600) as usize).min(W - 48);
    while offset < total {
        let count = (total - offset).min(CHUNK);
        let mut x = offset % W;
        let mut y = offset / W;
        for i in 0..count {
            if y >= 94 {
                chunk[i] = std::design::color::ACCENT;
            } else if y >= 40 && y < 44 && x >= 24 && x < 24 + activity {
                chunk[i] = std::design::color::ACCENT;
            } else {
                chunk[i] = std::design::color::SURFACE;
            }
            x += 1;
            if x == W {
                x = 0;
                y += 1;
            }
        }
        let _ = std::surface_write(4, surface, offset, &chunk[..count]);
        offset += count;
    }
}

fn resolve_app_at_touch(x: u32, y: u32) -> Option<&'static str> {
    let x = x as usize;
    let y = y as usize;
    // Row 0
    if (280..420).contains(&y) {
        if (56..306).contains(&x) { return Some("org.interos.settings"); }
        if (360..610).contains(&x) { return Some("org.interos.terminal"); }
    }
    // Row 1
    if (450..590).contains(&y) {
        if (56..306).contains(&x) { return Some("org.interos.calculator"); }
        if (360..610).contains(&x) { return Some("org.interos.vault"); }
    }
    // Row 2
    if (620..760).contains(&y) {
        if (56..306).contains(&x) { return Some("org.interos.camera"); }
        if (360..610).contains(&x) { return Some("org.interos.dialer"); }
    }
    // Row 3
    if (790..930).contains(&y) {
        if (56..306).contains(&x) { return Some("org.interos.browser"); }
        if (360..610).contains(&x) { return Some("org.interos.messages"); }
    }
    // Bottom Dock (y in 1260..1400)
    if (1260..1400).contains(&y) {
        if (40..160).contains(&x) { return Some("org.interos.dialer"); }
        if (200..320).contains(&x) { return Some("org.interos.messages"); }
        if (360..480).contains(&x) { return Some("org.interos.browser"); }
        if (520..640).contains(&x) { return Some("org.interos.terminal"); }
    }
    None
}

#[no_mangle]
pub extern "C" fn _start() -> ! {
    std::log(0, "[LAUNCHER USERSPACE] Live launcher online.").ok();
    
    std::log(0, "[LAUNCHER] Step 1: Allocating desktop surface (z=0)...").ok();
    let surface = match std::surface_create(4, W as u32, H as u32, 0) {
        Ok(id) => {
            std::log(0, "[LAUNCHER] Step 2: Surface created successfully.").ok();
            id
        }
        Err(_) => {
            std::log(0, "[LAUNCHER] Step 2: Surface creation failed!").ok();
            std::exit_with_code(1)
        }
    };

    std::log(0, "[LAUNCHER] Step 3: Rendering launcher UI canvas...").ok();
    render_full(surface);
    std::log(0, "[LAUNCHER] Step 4: Presenting initial frame to scanout...").ok();
    let _ = std::surface_flip(4);
    std::log(0, "[LAUNCHER] Step 5: Initial presentation complete.").ok();

    let mut refresh_ticks: u64 = 0;

    loop {
        if let Ok(events) = std::event_poll(
            std::EVENT_TOUCH_INPUT | std::EVENT_TIMER_TICK | std::EVENT_IPC_MSG,
            100,
        ) {
            let _ = std::service_heartbeat(10);
            if (events & std::EVENT_TIMER_TICK) != 0 {
                refresh_ticks += 1;
                if refresh_ticks % 10 == 0 {
                    render_header(surface);
                    let _ = std::surface_flip(4);
                }
            }

            // Consume the normalized kernel input pipeline. The kernel already
            // routes hardware gestures to the focused surface; the launcher owns
            // the focused surface while it is visible.
            if (events & std::EVENT_TOUCH_INPUT) != 0 {
                let mut input = std::InputEvent {
                    kind: 0,
                    x: 0,
                    y: 0,
                    dx: 0,
                    dy: 0,
                    scale_milli: 1000,
                };
                while std::input_event_poll(&mut input).is_ok() {
                    // A tap is the only launcher action. Pan/zoom gestures remain
                    // available to the input pipeline without launching apps.
                    if input.kind == 1 {
                        if let Some(target_app) = resolve_app_at_touch(input.x, input.y) {
                            match std::app_launch(target_app) {
                                Ok(task_id) => {
                                    std::log(0, "[LAUNCHER] Launched application from real touch input.").ok();
                                    let _ = task_id;
                                }
                                Err(_) => {
                                    std::log(0, "[LAUNCHER] Application launch rejected by kernel.").ok();
                                }
                            }
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
    std::log(0, "[LAUNCHER] Panic occurred!").ok();
    loop {
        std::yield_now();
    }
}