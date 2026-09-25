#![no_std]
#![no_main]

use core::panic::PanicInfo;
use interos_std as std;

const W: usize = 720;
const H: usize = 1440;

fn fill_rect(p: &mut [u32], x: usize, y: usize, w: usize, h: usize, c: u32) {
    let x1 = (x + w).min(W);
    let y1 = (y + h).min(H);
    for yy in y..y1 {
        let row = yy * W;
        for xx in x..x1 { p[row + xx] = c; }
    }
}

fn icon(p: &mut [u32], x: usize, y: usize, c: u32) {
    fill_rect(p, x + 8, y, 64, 64, c);
    fill_rect(p, x, y + 8, 80, 48, c);
}

fn render_launcher(p: &mut [u32]) {
    p.fill(0xFF070B12);
    fill_rect(p, 0, 0, W, 96, 0xFF101827);
    fill_rect(p, 24, 112, W - 48, 120, 0xFF111B2B);
    fill_rect(p, 24, 248, W - 48, 2, 0xFF26354D);

    let colors = [0xFF31C4FF, 0xFF63E6BE, 0xFFFFC857, 0xFFA78BFA,
                  0xFFFF7AA2, 0xFF58A6FF, 0xFF7DD3FC, 0xFF94A3B8];
    let labels_y = [400usize, 580, 760, 940];
    for row in 0..4 {
        for col in 0..2 {
            let x = 56 + col * 304;
            let y = labels_y[row];
            fill_rect(p, x, y, 250, 130, 0xFF111B2B);
            icon(p, x + 85, y + 20, colors[row * 2 + col]);
            fill_rect(p, x + 60, y + 108, 130, 3, colors[row * 2 + col]);
        }
    }
    fill_rect(p, 24, 1270, W - 48, 130, 0xFF101827);
    for i in 0..4 {
        icon(p, 70 + i * 165, 1302, colors[i]);
    }
}

#[no_mangle]
pub extern "C" fn _start() -> ! {
    std::log(0, "[LAUNCHER USERSPACE] Real Rust launcher service online.").ok();

    let surface = match std::surface_create(W as u32, H as u32, 0) {
        Ok(id) => id,
        Err(_) => {
            std::log(0, "[LAUNCHER USERSPACE] Launcher surface allocation failed.").ok();
            std::exit_with_code(1);
        }
    };

    let mut pixels = [0u32; W * H];
    render_launcher(&mut pixels);

    for (offset, chunk) in pixels.chunks(1024).enumerate() {
        let _ = std::surface_write(surface, (offset * 1024) as u32, chunk);
    }
    let _ = std::surface_flip();
    std::log(0, "[LAUNCHER USERSPACE] Launcher frame committed from real userspace pixel buffer.").ok();

    // Package-backed first-party launch path: the launcher asks the package service
    // to resolve and launch stable package IDs; it does not embed app ELF knowledge.
    let _ = std::ipc_send(1, 2, 0x5001, b"org.interos.settings");

    loop {
        let _ = std::event_poll(std::EVENT_TOUCH_INPUT | std::EVENT_TIMER_TICK, 100);
        let _ = std::touch_poll();
        std::yield_now();
    }
}

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    std::log(0, "[LAUNCHER USERSPACE] Panic; terminating launcher task.").ok();
    std::exit();
}
