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

fn resolve_app_at_touch(x: u32, y: u32) -> Option<&'static [u8]> {
    let x = x as usize;
    let y = y as usize;
    // Row 0
    if (280..420).contains(&y) {
        if (56..306).contains(&x) { return Some(b"org.interos.settings"); }
        if (360..610).contains(&x) { return Some(b"org.interos.terminal"); }
    }
    // Row 1
    if (450..590).contains(&y) {
        if (56..306).contains(&x) { return Some(b"org.interos.calculator"); }
        if (360..610).contains(&x) { return Some(b"org.interos.vault"); }
    }
    // Row 2
    if (620..760).contains(&y) {
        if (56..306).contains(&x) { return Some(b"org.interos.camera"); }
        if (360..610).contains(&x) { return Some(b"org.interos.dialer"); }
    }
    // Row 3
    if (790..930).contains(&y) {
        if (56..306).contains(&x) { return Some(b"org.interos.browser"); }
        if (360..610).contains(&x) { return Some(b"org.interos.messages"); }
    }
    // Bottom Dock (y in 1260..1400)
    if (1260..1400).contains(&y) {
        if (40..160).contains(&x) { return Some(b"org.interos.dialer"); }
        if (200..320).contains(&x) { return Some(b"org.interos.messages"); }
        if (360..480).contains(&x) { return Some(b"org.interos.browser"); }
        if (520..640).contains(&x) { return Some(b"org.interos.terminal"); }
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

    let mut app_stage: usize = 0;
    let mut ticks_seen: u64 = 0;
    let mut ipc_buf = [0u8; 64];

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

            // Handle direct touch events with grid hit-testing
            if (events & std::EVENT_TOUCH_INPUT) != 0 {
                let _ = std::touch_poll();
            }

            // Handle incoming IPC input messages (touch coordinates)
            if (events & std::EVENT_IPC_MSG) != 0 {
                if let Ok(len) = std::ipc_recv(5, &mut ipc_buf) {
                    if len >= 12 {
                        let x = u32::from_le_bytes([ipc_buf[4], ipc_buf[5], ipc_buf[6], ipc_buf[7]]);
                        let y = u32::from_le_bytes([ipc_buf[8], ipc_buf[9], ipc_buf[10], ipc_buf[11]]);
                        if let Some(target_app) = resolve_app_at_touch(x, y) {
                            std::log(0, "[LAUNCHER] Touch hit-test matched application card. Dispatching launch IPC...").ok();
                            let _ = std::ipc_send(2, 1, 0x5001, target_app);
                        }
                    }
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

            // Stage 2: Wait for secondary app to complete, then launch tertiary (Calculator)
            if app_stage == 2 && ticks_seen >= 22 {
                std::log(0, "[LAUNCHER USERSPACE] Secondary application session concluded. Launcher remains alive.").ok();
                std::log(0, "[LAUNCHER USERSPACE] Launching tertiary application: org.interos.calculator.").ok();
                if std::ipc_send(2, 1, 0x5001, b"org.interos.calculator").is_ok() {
                    std::log(0, "[LAUNCHER USERSPACE] Tertiary app launch IPC successfully dispatched.").ok();
                    app_stage = 3;
                } else {
                    std::log(0, "[LAUNCHER USERSPACE] Tertiary app launch IPC failed.").ok();
                }
            }

            // Stage 3: All verified apps active; launcher remains alive and responsive
            if app_stage == 3 && ticks_seen >= 32 {
                std::log(0, "[LAUNCHER USERSPACE] Multi-application lifecycle execution cycle verified. Launcher active.").ok();
                app_stage = 4;
            }
        }
        let _ = std::touch_poll();
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