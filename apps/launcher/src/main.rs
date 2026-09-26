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
                chunk[i] = if y >= 94 { std::design::color::ACCENT } else { std::design::color::SURFACE };
            } else if (112..232).contains(&y) && x >= 24 && x < W - 24 {
                chunk[i] = std::design::color::SURFACE;
            } else if (400..530).contains(&y) && x >= 56 && x < 306 {
                chunk[i] = if (420..484).contains(&y) && x >= 149 && x < 213 {
                    std::design::color::ACCENT
                } else {
                    std::design::color::SURFACE
                };
            } else if (400..530).contains(&y) && x >= 360 && x < 610 {
                chunk[i] = if (420..484).contains(&y) && x >= 453 && x < 517 {
                    std::design::color::SUCCESS
                } else {
                    std::design::color::SURFACE
                };
            } else if (1270..1400).contains(&y) && x >= 24 && x < W - 24 {
                chunk[i] = std::design::color::SURFACE;
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
    std::log(0, "[LAUNCHER] render_full complete.").ok();
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

    let mut app_stage: usize = 0;
    let mut ticks_seen: u64 = 0;

    loop {
        if let Ok(events) = std::event_poll(
            std::EVENT_TOUCH_INPUT | std::EVENT_TIMER_TICK | std::EVENT_IPC_MSG,
            100,
        ) {
            let _ = std::service_heartbeat(10);
            if (events & std::EVENT_TIMER_TICK) != 0 {
                ticks_seen += 1;
                if ticks_seen % 10 == 0 {
                    render_header(surface);
                    let _ = std::surface_flip(4);
                }
            }

            // Stage 0: Launch primary application (Settings)
            if app_stage == 0 && ((events & std::EVENT_TOUCH_INPUT) != 0 || ticks_seen >= 2) {
                std::log(0, "[LAUNCHER USERSPACE] Launching primary application: org.interos.settings.").ok();
                // Slot 2 is IpcSend targeting Task 1 (PID 1)
                if std::ipc_send(2, 1, 0x5001, b"org.interos.settings").is_ok() {
                    std::log(0, "[LAUNCHER USERSPACE] Primary app launch IPC successfully dispatched.").ok();
                    app_stage = 1;
                } else {
                    std::log(0, "[LAUNCHER USERSPACE] Primary app launch IPC failed.").ok();
                }
            }

            // Stage 1: Wait for primary app to complete and exit, then launch secondary (Terminal)
            if app_stage == 1 && ticks_seen >= 12 {
                std::log(0, "[LAUNCHER USERSPACE] Primary application session concluded. Launcher remains alive.").ok();
                std::log(0, "[LAUNCHER USERSPACE] Launching secondary application: org.interos.terminal.").ok();
                if std::ipc_send(2, 1, 0x5001, b"org.interos.terminal").is_ok() {
                    std::log(0, "[LAUNCHER USERSPACE] Secondary app launch IPC successfully dispatched.").ok();
                    app_stage = 2;
                } else {
                    std::log(0, "[LAUNCHER USERSPACE] Secondary app launch IPC failed.").ok();
                }
            }
        }
        let _ = std::touch_poll();
        std::yield_now();
    }
}

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    std::exit();
}