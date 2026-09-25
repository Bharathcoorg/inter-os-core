#![no_std]
#![no_main]

use core::panic::PanicInfo;
use interos_std as std;

const DISPLAY_W: u32 = 720;
const SYSTEM_LINE_H: u32 = 96;
const CHUNK: usize = 1024;

fn render_chunk(surface: u32, offset: usize, count: usize) {
    let mut pixels = [0u32; CHUNK];
    pixels.fill(0xFF0B0F19);

    for i in 0..count {
        let pixel = offset + i;
        let x = pixel % DISPLAY_W as usize;
        let y = pixel / DISPLAY_W as usize;
        if y < 2
            || y >= SYSTEM_LINE_H as usize - 2
            || x < 2
            || x >= DISPLAY_W as usize - 2
        {
            pixels[i] = 0xFF253044;
        }
    }

    let _ = std::surface_write(surface, offset as u32, &pixels[..count]);
}

#[no_mangle]
pub extern "C" fn _start() -> ! {
    std::log(0, "[COMPOSITOR USERSPACE] Ring-3 compositor service online.").ok();

    match std::surface_create(DISPLAY_W, SYSTEM_LINE_H, 100) {
        Ok(surface) => {
            let total = (DISPLAY_W * SYSTEM_LINE_H) as usize;
            let mut offset = 0usize;

            while offset < total {
                let count = (total - offset).min(CHUNK);
                render_chunk(surface, offset, count);
                offset += count;
            }

            let _ = std::surface_flip();
            std::log(
                0,
                "[COMPOSITOR USERSPACE] Bounded system-line frame committed.",
            )
            .ok();
        }
        Err(_) => {
            std::log(
                0,
                "[COMPOSITOR USERSPACE] Surface allocation unavailable; service remains alive.",
            )
            .ok();
        }
    }

    loop {
        let _ = std::event_poll(
            std::EVENT_TOUCH_INPUT | std::EVENT_TIMER_TICK | std::EVENT_IPC_MSG,
            100,
        );
        let _ = std::touch_poll();
        std::yield_now();
    }
}

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    std::log(
        0,
        "[COMPOSITOR USERSPACE] Panic; terminating compositor task.",
    )
    .ok();
    std::exit();
}
