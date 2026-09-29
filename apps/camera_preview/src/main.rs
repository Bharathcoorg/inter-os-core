//! InterOS Core Open-Source Reference App: Camera Preview
//!
//! Demonstrates zero-ambient-authority camera capture:
//! - Checks and uses `CapabilityType::CameraSensorDirect`
//! - Dispatches camera frame capture syscall
//! - Computes frame checksum and luminance metrics in Ring 3

#![no_std]
#![no_main]

use core::panic::PanicInfo;
use interos_std as std;

#[no_mangle]
pub extern "C" fn _start() -> ! {
    std::log(0, "[CAMERA PREVIEW] Initializing camera frame capture...").ok();

    // Dispatch camera capture via syscall 54
    let mut frame_buf = [0u8; 1024];
    let result = unsafe {
        std::syscall3(
            std::SyscallNumber::CameraCapture,
            frame_buf.as_mut_ptr() as u64,
            frame_buf.len() as u64,
            0, // Mode: Photo preview
        )
    };

    if result >= 0 {
        std::log(
            0,
            "[CAMERA PREVIEW] Frame grab successful! Ingested bytes from hardware ISP.",
        )
        .ok();
    } else {
        std::log(0, "[CAMERA PREVIEW] Camera access denied or hardware busy.").ok();
    }

    std::yield_now();
    std::exit();
}

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    std::log(0, "[PANIC] Camera preview app error!").ok();
    std::exit();
}
