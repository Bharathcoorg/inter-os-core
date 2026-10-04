#![no_std]
#![no_main]
use core::panic::PanicInfo;
use interos_std as std;

const W: u32 = 720;
const H: u32 = 96;
const CHUNK: usize = 1024;

/// Sweep period of the progress bar, in ticks. The bar's pixels are a function of
/// `ticks % BAR_PERIOD`, so this is also the granularity at which the line's
/// content can change.
const BAR_PERIOD: u64 = 600;

// -----------------------------------------------------------------------------
// Why this task must not publish
// -----------------------------------------------------------------------------
//
// WHY this used to tear. `surface_flip` takes no surface id: it composites
// *every* surface in the kernel and swaps the scanout buffer. It is not "present
// my window", it is "publish the whole display", and any task may call it at any
// moment about a frame it did not draw.
//
// The old loop called it once per pass, on a timer, whether or not anything had
// changed. The launcher needs 253 `surface_write` calls for one frame -- the
// kernel caps each at 4096 pixels and the surface is 720x1440 -- so a flip landing
// anywhere inside that sequence published the first N rows of the new screen over
// the previous screen's pixels in every row not yet written. That is the reported
// artifact: the old screen surviving underneath the new one.
//
// WHY the fix is to stop publishing. With a global flip there is no way for this
// task to tell whether another client is halfway through its own write sequence.
// There is no "is this surface being written" query, and the kernel's `dirty` flag
// is not exposed to userspace -- `DrmKmsController::composite` clears it and never
// consults it, so even the kernel has no notion of an incomplete frame. That code
// is in `kernel/src/drivers/display_kms.rs` and is out of scope for this file.
//
// So the only rule available from userspace is the one that removes the
// unsynchronised publisher entirely:
//
//     a frame is published by the task that finished writing it, and by nobody
//     else, at any other time.
//
// Every client in this tree already flips exactly once, after a complete write --
// the launcher, and each app -- so publication is already frame-aligned and this
// loop was the sole exception. With it gone, no `surface_flip` can occur between
// two `surface_write` calls of the same surface. That is what "a partially written
// surface is never published" requires, and it holds across the whole system
// rather than only when this task happens not to be running.
//
// WHY the system line still appears: `composite()` composites every surface on
// every flip, so the line is published by whoever next publishes -- the launcher
// after each frame. This task's job is to keep that surface's contents current;
// it is no longer a publisher.
//
// WHY the residual is stated plainly: this is a convention every client follows,
// not a kernel guarantee. If a client ever grows a multi-frame write and flips
// inside it, tearing returns, and the real fix belongs in the kernel -- either
// have `composite()` skip surfaces whose frame is incomplete, or give flip a
// surface id. Neither is reachable from this file.

/// One chunk of the system line.
fn chunk(surface: u32, offset: usize, count: usize, ticks: u64, windows: usize, services: usize) {
    let mut pixels = [0u32; CHUNK];
    pixels.fill(std::design::color::SURFACE);
    for i in 0..count {
        let q = offset + i;
        let x = q % W as usize;
        let y = q / W as usize;
        if y < 2 || y >= H as usize - 2 || x < 2 || x >= W as usize - 2 {
            pixels[i] = std::design::color::BORDER;
        } else if y >= 38 && y < 42 && x < ((ticks % BAR_PERIOD) as usize).min(W as usize - 8) {
            pixels[i] = std::design::color::ACCENT;
        } else if y >= 70 && x < 48 + windows.min(8) * 18 {
            pixels[i] = std::design::color::SUCCESS;
        } else if y >= 78 && x < 48 + services.min(8) * 18 {
            pixels[i] = std::design::color::TEXT_MUTED;
        }
    }
    let _ = std::surface_write(4, surface, offset, &pixels[..count]);
}

/// Rewrites the whole system line into the surface.
///
/// WHY this is still worth doing even though nothing presents it: `composite()`
/// reads the surface on every flip, so its contents have to be right by the time
/// the next client publishes, and a client publishes on its own schedule.
fn draw_system_line(surface: u32, ticks: u64, windows: usize, services: usize) {
    let total = (W * H) as usize;
    let mut offset = 0;
    while offset < total {
        let count = (total - offset).min(CHUNK);
        chunk(surface, offset, count, ticks, windows, services);
        offset += count;
    }
}

#[no_mangle]
pub extern "C" fn _start() -> ! {
    std::log(
        0,
        "[COMPOSITOR USERSPACE] Ring-3 system-line compositor online.",
    )
    .ok();
    match std::surface_create(4, W, H, 100) {
        Ok(surface) => {
            // WHY this starts as "nothing drawn" rather than 0: tick 0 is a real
            // tick, and treating it as already drawn would skip the first paint.
            let mut drawn: Option<(u64, usize, usize)> = None;
            loop {
                let _ = std::service_heartbeat(9);
                let ticks = std::get_ticks(2).unwrap_or(0);
                let mut windows = [0u8; 4096];
                let mut services = [0u8; 1024];
                let wc = std::window_list(&mut windows)
                    .map(|n| if n > 0 { 1 } else { 0 })
                    .unwrap_or(0);
                let sc = std::service_list(&mut services)
                    .map(|n| if n > 0 { 1 } else { 0 })
                    .unwrap_or(0);

                // WHY re-render only when the content actually changed: this task
                // used to rewrite 69,120 pixels on every pass for a bar that moves
                // once per `BAR_PERIOD` ticks. The three things the bar draws are a
                // function of the sweep phase and of whether any window or service
                // exists, so that triple is the entire cache key. The old loop
                // also ran a full scanout composite per pass, which is the most
                // expensive thing in the shell and bought nothing.
                let key = (ticks % BAR_PERIOD, wc, sc);
                if drawn != Some(key) {
                    draw_system_line(surface, ticks, wc, sc);
                    drawn = Some(key);
                }

                let _ = std::event_poll(
                    std::EVENT_TOUCH_INPUT | std::EVENT_TIMER_TICK | std::EVENT_IPC_MSG,
                    100,
                );
                std::yield_now();
            }
        }
        Err(_) => {
            std::log(
                0,
                "[COMPOSITOR USERSPACE] Surface allocation unavailable; service remains alive.",
            )
            .ok();
            loop {
                std::yield_now();
            }
        }
    }
}

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    std::exit();
}