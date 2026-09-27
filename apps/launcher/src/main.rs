#![no_std]
#![no_main]

use core::panic::PanicInfo;
use interos_std as std;

const W: usize = 720;
const H: usize = 1440;
const CHUNK: usize = 4096;

#[inline(always)]
fn get_color(x: usize, y: usize) -> u32 {
    if y < 96 {
        return if y >= 94 { std::design::color::ACCENT } else { std::design::color::SURFACE };
    }
    if y < 112 { return std::design::color::BACKGROUND; }
    if y < 232 {
        return if x >= 24 && x < W - 24 {
            if y >= 230 { std::design::color::ACCENT } else { std::design::color::SURFACE }
        } else {
            std::design::color::BACKGROUND
        };
    }
    if y < 280 { return std::design::color::BACKGROUND; }
    if y < 420 {
        if x >= 56 && x < 306 {
            return if y >= 330 && y < 394 && x >= 149 && x < 213 { std::design::color::ACCENT } else { std::design::color::SURFACE };
        }
        if x >= 360 && x < 610 {
            return if y >= 330 && y < 394 && x >= 453 && x < 517 { std::design::color::SUCCESS } else { std::design::color::SURFACE };
        }
        return std::design::color::BACKGROUND;
    }
    if y < 450 { return std::design::color::BACKGROUND; }
    if y < 590 {
        if x >= 56 && x < 306 {
            return if y >= 500 && y < 564 && x >= 149 && x < 213 { std::design::color::WARNING } else { std::design::color::SURFACE };
        }
        if x >= 360 && x < 610 {
            return if y >= 500 && y < 564 && x >= 453 && x < 517 { std::design::color::DANGER } else { std::design::color::SURFACE };
        }
        return std::design::color::BACKGROUND;
    }
    if y < 620 { return std::design::color::BACKGROUND; }
    if y < 760 {
        if x >= 56 && x < 306 {
            return if y >= 670 && y < 734 && x >= 149 && x < 213 { std::design::color::ACCENT } else { std::design::color::SURFACE };
        }
        if x >= 360 && x < 610 {
            return if y >= 670 && y < 734 && x >= 453 && x < 517 { std::design::color::SUCCESS } else { std::design::color::SURFACE };
        }
        return std::design::color::BACKGROUND;
    }
    if y < 790 { return std::design::color::BACKGROUND; }
    if y < 930 {
        if x >= 56 && x < 306 {
            return if y >= 840 && y < 904 && x >= 149 && x < 213 { std::design::color::TEXT_MUTED } else { std::design::color::SURFACE };
        }
        if x >= 360 && x < 610 {
            return if y >= 840 && y < 904 && x >= 453 && x < 517 { std::design::color::WARNING } else { std::design::color::SURFACE };
        }
        return std::design::color::BACKGROUND;
    }
    if y < 960 { return std::design::color::BACKGROUND; }
    if y < 1100 {
        if x >= 56 && x < 306 {
            return if y >= 1010 && y < 1074 && x >= 149 && x < 213 { std::design::color::ACCENT } else { std::design::color::SURFACE };
        }
        if x >= 360 && x < 610 {
            return if y >= 1010 && y < 1074 && x >= 453 && x < 517 { std::design::color::SUCCESS } else { std::design::color::SURFACE };
        }
        return std::design::color::BACKGROUND;
    }
    if y >= 1260 && y < 1400 && x >= 24 && x < W - 24 {
        let is_dock_icon = (x >= 60 && x < 124) || (x >= 220 && x < 284)
            || (x >= 380 && x < 444) || (x >= 540 && x < 604);
        return if y >= 1298 && y < 1362 && is_dock_icon {
            std::design::color::ACCENT
        } else {
            std::design::color::SURFACE_RAISED
        };
    }
    if y >= 1418 && y < 1424 && x >= 260 && x < 460 {
        return std::design::color::TEXT_MUTED;
    }
    std::design::color::BACKGROUND
}

fn render_full(surface: u32) {
    let mut chunk = [0u32; CHUNK];
    let total = W * H;
    let mut offset = 0;
    while offset < total {
        let count = (total - offset).min(CHUNK);
        let mut x = offset % W;
        let mut y = offset / W;
        for i in 0..count {
            chunk[i] = get_color(x, y);
            x += 1;
            if x == W {
                x = 0;
                y += 1;
            }
        }
        let _ = std::surface_write(4, surface, offset, &chunk[..count]);
        offset += count;
    }
    std::log(0, "[LAUNCHER] render_full complete: 10-app home screen canvas initialized.").ok();
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
    // Row 4
    if (960..1100).contains(&y) {
        if (56..306).contains(&x) { return Some("org.interos.calendar"); }
        if (360..610).contains(&x) { return Some("org.interos.music"); }
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

#[inline(always)]
fn get_overview_color(x: usize, y: usize) -> u32 {
    // Header
    if y < 96 {
        return if y >= 94 { std::design::color::ACCENT } else { std::design::color::SURFACE };
    }
    // Card 0: y in 200..480, x in 56..(W - 56)
    if y >= 200 && y < 480 && x >= 56 && x < (W - 56) {
        if y < 250 { return std::design::color::ACCENT; }
        if y >= 476 || x < 60 || x >= (W - 60) { return std::design::color::BORDER; }
        if y >= 330 && y < 370 && x >= 100 && x < (W - 100) { return std::design::color::SURFACE_RAISED; }
        return std::design::color::SURFACE;
    }
    // Card 1: y in 520..800, x in 56..(W - 56)
    if y >= 520 && y < 800 && x >= 56 && x < (W - 56) {
        if y < 570 { return std::design::color::SUCCESS; }
        if y >= 796 || x < 60 || x >= (W - 60) { return std::design::color::BORDER; }
        if y >= 650 && y < 690 && x >= 100 && x < (W - 100) { return std::design::color::SURFACE_RAISED; }
        return std::design::color::SURFACE;
    }
    // Card 2: y in 840..1120, x in 56..(W - 56)
    if y >= 840 && y < 1120 && x >= 56 && x < (W - 56) {
        if y < 890 { return std::design::color::WARNING; }
        if y >= 1116 || x < 60 || x >= (W - 60) { return std::design::color::BORDER; }
        if y >= 970 && y < 1010 && x >= 100 && x < (W - 100) { return std::design::color::SURFACE_RAISED; }
        return std::design::color::SURFACE;
    }
    // Clear / Return button: y in 1200..1280, x in 180..(W - 180)
    if y >= 1200 && y < 1280 && x >= 180 && x < (W - 180) {
        if y >= 1276 || x < 184 || x >= (W - 184) { return std::design::color::ACCENT; }
        return std::design::color::SURFACE_RAISED;
    }
    // Bottom Home Gesture Pill
    if y >= 1418 && y < 1424 && x >= 260 && x < 460 {
        return std::design::color::TEXT_MUTED;
    }
    std::design::color::BACKGROUND
}

fn render_overview(surface: u32) {
    let mut chunk = [0u32; CHUNK];
    let total = W * H;
    let mut offset = 0;
    while offset < total {
        let count = (total - offset).min(CHUNK);
        let mut x = offset % W;
        let mut y = offset / W;
        for i in 0..count {
            chunk[i] = get_overview_color(x, y);
            x += 1;
            if x == W {
                x = 0;
                y += 1;
            }
        }
        let _ = std::surface_write(4, surface, offset, &chunk[..count]);
        offset += count;
    }
    std::log(0, "[LAUNCHER] render_overview complete: Recent apps overview deck displayed.").ok();
}

fn push_recent(recents: &mut [&'static str; 3], app: &'static str) {
    if recents[0] == app { return; }
    if recents[1] == app {
        recents.swap(0, 1);
        return;
    }
    if recents[2] == app {
        let temp = recents[2];
        recents[2] = recents[1];
        recents[1] = recents[0];
        recents[0] = temp;
        return;
    }
    recents[2] = recents[1];
    recents[1] = recents[0];
    recents[0] = app;
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
    let mut active_child: Option<u64> = None;
    let mut in_overview = false;
    let mut recent_apps: [&'static str; 3] = [
        "org.interos.calculator",
        "org.interos.settings",
        "org.interos.terminal",
    ];

    loop {
        if let Ok(events) = std::event_poll(
            std::EVENT_TOUCH_INPUT | std::EVENT_TIMER_TICK | std::EVENT_IPC_MSG,
            100,
        ) {
            let _ = std::service_heartbeat(10);

            // Reap the foreground application when it exits. This is the
            // authoritative parent-side lifecycle boundary; without it a
            // completed app would remain as a Zombie TCB indefinitely.
            if let Some(child_id) = active_child {
                if std::waitpid(child_id).is_ok() {
                    active_child = None;
                    in_overview = false;
                    let mut discard = std::InputEvent {
                        kind: 0,
                        x: 0,
                        y: 0,
                        dx: 0,
                        dy: 0,
                        scale_milli: 1000,
                    };
                    while std::input_event_poll(&mut discard).is_ok() {}
                    render_full(surface);
                    let _ = std::surface_flip(4);
                    std::log(0, "[LAUNCHER] Foreground application exited and was reaped.").ok();
                }
            }

            if active_child.is_none() && !in_overview && (events & std::EVENT_TIMER_TICK) != 0 {
                refresh_ticks += 1;
                if refresh_ticks % 10 == 0 {
                    render_header(surface);
                    let _ = std::surface_flip(4);
                }
            }

            // Consume the normalized kernel input pipeline only when no child application
            // is active. When a child app is active, it owns the foreground focused surface
            // and consumes touch gestures directly.
            if active_child.is_none() && (events & std::EVENT_TOUCH_INPUT) != 0 {
                let mut input = std::InputEvent {
                    kind: 0,
                    x: 0,
                    y: 0,
                    dx: 0,
                    dy: 0,
                    scale_milli: 1000,
                };
                while std::input_event_poll(&mut input).is_ok() {
                    if in_overview {
                        // In Overview / Recents Switcher mode
                        if input.kind == 4 && input.dy >= 60 {
                            // Swipe down to dismiss overview
                            in_overview = false;
                            render_full(surface);
                            let _ = std::surface_flip(4);
                            std::log(0, "[LAUNCHER] Returned to home grid from recents via swipe.").ok();
                            break;
                        }
                        if input.kind == 1 {
                            let x = input.x as usize;
                            let y = input.y as usize;
                            // Tap Card 0
                            if (200..480).contains(&y) && (56..664).contains(&x) {
                                let target = recent_apps[0];
                                push_recent(&mut recent_apps, target);
                                if let Ok(task_id) = std::app_launch(target) {
                                    active_child = Some(task_id);
                                    in_overview = false;
                                    std::log(0, "[LAUNCHER] Switched to recent app from overview card 0.").ok();
                                    break;
                                }
                            }
                            // Tap Card 1
                            else if (520..800).contains(&y) && (56..664).contains(&x) {
                                let target = recent_apps[1];
                                push_recent(&mut recent_apps, target);
                                if let Ok(task_id) = std::app_launch(target) {
                                    active_child = Some(task_id);
                                    in_overview = false;
                                    std::log(0, "[LAUNCHER] Switched to recent app from overview card 1.").ok();
                                    break;
                                }
                            }
                            // Tap Card 2
                            else if (840..1120).contains(&y) && (56..664).contains(&x) {
                                let target = recent_apps[2];
                                push_recent(&mut recent_apps, target);
                                if let Ok(task_id) = std::app_launch(target) {
                                    active_child = Some(task_id);
                                    in_overview = false;
                                    std::log(0, "[LAUNCHER] Switched to recent app from overview card 2.").ok();
                                    break;
                                }
                            }
                            // Return / Close button or Home bar
                            else if (1200..1300).contains(&y) || y >= 1400 {
                                in_overview = false;
                                render_full(surface);
                                let _ = std::surface_flip(4);
                                std::log(0, "[LAUNCHER] Returned to home grid from recents.").ok();
                                break;
                            }
                        }
                    } else {
                        // Standard Home Grid mode
                        // 1. Swipe-up from bottom gesture bar opens Overview / Recents
                        if input.kind == 4 && input.dy <= -60 && input.y <= 1380 {
                            in_overview = true;
                            let mut win_buf = [0u8; 1024];
                            let _ = std::window_list(&mut win_buf);
                            render_overview(surface);
                            let _ = std::surface_flip(4);
                            std::log(0, "[LAUNCHER] Overview / Recent Apps switcher opened via swipe-up gesture.").ok();
                            break;
                        }
                        // 2. Tap
                        if input.kind == 1 {
                            // Tapping the bottom home pill when already on home screen opens Overview
                            if input.x >= 240 && input.x <= 480 && input.y >= 1400 {
                                in_overview = true;
                                let mut win_buf = [0u8; 1024];
                                let _ = std::window_list(&mut win_buf);
                                render_overview(surface);
                                let _ = std::surface_flip(4);
                                std::log(0, "[LAUNCHER] Overview / Recent Apps switcher opened via home bar tap.").ok();
                                break;
                            }
                            if let Some(target_app) = resolve_app_at_touch(input.x, input.y) {
                                push_recent(&mut recent_apps, target_app);
                                match std::app_launch(target_app) {
                                    Ok(task_id) => {
                                        active_child = Some(task_id);
                                        std::log(0, "[LAUNCHER] Launched application from real touch input.").ok();
                                        break;
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