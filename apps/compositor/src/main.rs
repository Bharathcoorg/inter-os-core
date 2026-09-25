#![no_std]
#![no_main]

use core::panic::PanicInfo;
use interos_std as std;

const DISPLAY_W: u32 = 720;
const SYSTEM_LINE_H: u32 = 96;

#[no_mangle]
pub extern "C" fn _start() -> ! {
    std::log(0, "[COMPOSITOR USERSPACE] Ring-3 compositor service online.").ok();

    match std::surface_create(DISPLAY_W, SYSTEM_LINE_H, 100) {
        Ok(surface) => {
            std::log(0, "[COMPOSITOR USERSPACE] System-line surface allocated through DRM/KMS syscall.").ok();
            let mut pixels = [0u32; 720 * 96];
            pixels.fill(0xFF0B0F19);
            // Minimal real system-line surface: reserved display region owned by compositor.
            for y in 0..96usize {
                for x in 0..720usize {
                    if y < 2 || y >= 94 || x < 2 || x >= 718 {
                        pixels[y * 720 + x] = 0xFF253044;
                    }
                }
            }
            for chunk in pixels.chunks(1024) {
                let _ = std::surface_write(surface, 0, chunk);
            }
            let _ = std::surface_flip();
            std::log(0, "[COMPOSITOR USERSPACE] First composition committed with atomic page flip.").ok();
        }
        Err(_) => { std::log(0, "[COMPOSITOR USERSPACE] Surface allocation unavailable; service remains alive.").ok(); }
    }

    loop {
        let _ = std::event_poll(std::EVENT_TOUCH_INPUT | std::EVENT_TIMER_TICK | std::EVENT_IPC_MSG, 100);
        let _ = std::touch_poll();
        std::yield_now();
    }
}

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    std::log(0, "[COMPOSITOR USERSPACE] Panic; terminating compositor task.").ok();
    std::exit();
}
