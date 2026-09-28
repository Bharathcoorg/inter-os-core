//! Inter OS userspace PID 1 / service-manager bootstrap.
//!
//! PID 1 deliberately contains no hardware-policy demos or application logic.
//! The kernel owns only the minimum primitives; this process owns userspace
//! boot ordering, service observation, and the transition to the normal event loop.

#![no_std]
#![no_main]

use interos_std as std;
use core::panic::PanicInfo;

const SERVICE_BUFFER: usize = 512;
const POLL_INTERVAL_TICKS: u64 = 100;

#[no_mangle]
pub extern "C" fn _start() -> ! {
    std::log(0, "[PID1] Inter OS userspace init started in Ring 3.").ok();
    // Service-manager bootstrap. PID 1, not kernel_main, owns the
    // startup ordering of the userspace service set.
    const SERVICES: [u8; 9] = [1, 2, 3, 4, 5, 6, 7, 8, 11];
    for service_id in SERVICES {
        match std::service_start(service_id) {
            Ok(_) => { let _ = std::log(0, "[PID1] Service started through lifecycle ABI."); },
            Err(_) => { let _ = std::log(0, "[PID1] Service start request rejected."); },
        }
    }


    // Deterministic lifecycle smoke: exercise the real stop -> start path
    // once after bootstrap. This makes QEMU validate more than initial spawning.
    match std::service_stop(1) {
        Ok(_) => {
            let _ = std::log(0, "[PID1] Service stop lifecycle contract exercised.");
            match std::service_start(1) {
                Ok(_) => { let _ = std::log(0, "[PID1] Service restart lifecycle contract exercised."); }
                Err(_) => { let _ = std::log(0, "[PID1] Service restart lifecycle contract failed."); }
            }
        }
        Err(_) => { let _ = std::log(0, "[PID1] Service stop lifecycle contract failed."); }
    }

    // Establish the first userspace service-manager contract: PID 1 observes
    // the authoritative kernel service registry rather than hard-coding task IDs.
    let mut service_buf = [0u8; SERVICE_BUFFER];
    if let Ok(len) = std::service_list(&mut service_buf) {
        if registry_has_all_running_services(&service_buf[..len]) {
            let _ = std::log(0, "[PID1] All system services running after bootstrap.");
        } else {
            let _ = std::log(0, "[PID1] Service bootstrap validation failed.");
        }
    }

    // Launch UI subsystem services: Compositor (9) and Launcher (10)
    const UI_SERVICES: [u8; 2] = [9, 10];
    for service_id in UI_SERVICES {
        let _ = std::service_start(service_id);
    }
    let mut last_poll = 0u64;
    let mut ipc_buf = [0u8; 64];

    loop {
        let now = std::get_ticks(0).unwrap_or(last_poll);

        if now.saturating_sub(last_poll) >= POLL_INTERVAL_TICKS || last_poll == 0 {
            if let Ok(len) = std::service_list(&mut service_buf) {
                if len > 0 {
                    std::log(0, "[PID1] Service registry observed; userspace lifecycle loop active.").ok();
                } else {
                    std::log(0, "[PID1] Service registry returned no active services.").ok();
                }
            }
            for service_id in [1u8,2,3,4,5,6,7,8,9,10,11] {
                if std::service_recover_failed(service_id).is_ok() {
                    std::log(0, "[PID1] Recovered a failed service through the lifecycle ABI.").ok();
                }
            }
            last_poll = now;
        }

        // Check for incoming IPC requests from launcher
        if let Ok(msg_len) = std::ipc_recv(10, &mut ipc_buf) {
            if msg_len > 0 {
                let _ = std::log(0, "[PID1] Application launch request received via IPC from launcher.");
                let app_id = core::str::from_utf8(&ipc_buf[..msg_len]).unwrap_or("").trim_matches(|c: char| c.is_whitespace() || c == '\0');
                if !app_id.is_empty() {
                    match std::app_launch(app_id) {
                        Ok(_) => { let _ = std::log(0, "[PID1] Target application launched successfully in Ring 3."); }
                        Err(_) => { let _ = std::log(0, "[PID1] Target application launch rejected."); }
                    }
                }
            }
        }

        // Block cooperatively until an IPC/timer/input event instead of
        // continuously spinning in Ring 3.
        let _ = std::event_poll(std::EVENT_IPC_MSG | std::EVENT_TIMER_TICK, 20);
    }
}

fn registry_has_all_running_services(buf: &[u8]) -> bool {
    // Registry records are newline-delimited: service_id|task_id|state. PID 1
    // only accepts the exact eight non-init system services as healthy when all
    // have a non-zero task ID and the literal running state.
    let mut seen = 0u8;
    for line in buf.split(|b| *b == b'\n') {
        if line.is_empty() { continue; }
        let mut fields = line.split(|b| *b == b'|');
        let Some(id_bytes) = fields.next() else { continue; };
        let Some(task_bytes) = fields.next() else { continue; };
        let Some(state) = fields.next() else { continue; };
        if state != b"running" || task_bytes == b"0" { continue; }
        if id_bytes.len() == 1 && (b'1'..=b'8').contains(&id_bytes[0]) || id_bytes == b"11" {
            seen |= 1 << (id_bytes[0] - b'1');
        }
    }
    seen == 0xff
}

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    loop {
        core::hint::spin_loop();
    }
}
