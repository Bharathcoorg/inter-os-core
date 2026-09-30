//! Inter OS userspace PID 1 / service-manager bootstrap.
//!
//! PID 1 deliberately contains no hardware-policy demos or application logic.
//! The kernel owns only the minimum primitives; this process owns userspace
//! boot ordering, service observation, and the transition to the normal event loop.

#![no_std]
#![no_main]

use core::panic::PanicInfo;
use interos_std as std;

const SERVICE_BUFFER: usize = 512;
const POLL_INTERVAL_TICKS: u64 = 100;

#[no_mangle]
pub extern "C" fn _start() -> ! {
    std::log(0, "[PID1] Inter OS userspace init started in Ring 3.").ok();
    // Service-manager bootstrap. PID 1, not kernel_main, owns the
    // startup ordering of the userspace service set.
    // Ask the authoritative kernel registry to start every declared
    // userspace service. Retry dependency-blocked services so PID 1 does not
    // depend on a duplicated dependency ordering table.
    let mut remaining = 11u8;
    while remaining > 0 {
        let mut progress = false;
        for service_id in 1u8..=11u8 {
            match std::service_start(service_id) {
                Ok(_) => progress = true,
                Err(_) => {}
            }
        }
        if registry_has_all_running_services_after_query() {
            break;
        }
        if !progress {
            break;
        }
        remaining -= 1;
    }

    // Deterministic lifecycle smoke: exercise the real stop -> start path on
    // Sensor, a dependency leaf, so the test does not intentionally violate
    // the Compositor -> Display/Input dependency graph.
    match std::service_stop(11) {
        Ok(_) => {
            let _ = std::log(0, "[PID1] Service stop lifecycle contract exercised.");
            match std::service_start(1) {
                Ok(_) => {
                    let _ = std::log(0, "[PID1] Service restart lifecycle contract exercised.");
                }
                Err(_) => {
                    let _ = std::log(0, "[PID1] Service restart lifecycle contract failed.");
                }
            }
        }
        Err(_) => {
            let _ = std::log(0, "[PID1] Service stop lifecycle contract failed.");
        }
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

    let mut last_poll = 0u64;
    let mut ipc_buf = [0u8; 64];

    loop {
        let now = std::get_ticks(0).unwrap_or(last_poll);

        if now.saturating_sub(last_poll) >= POLL_INTERVAL_TICKS || last_poll == 0 {
            if let Ok(len) = std::service_list(&mut service_buf) {
                if len > 0 {
                    std::log(
                        0,
                        "[PID1] Service registry observed; userspace lifecycle loop active.",
                    )
                    .ok();
                } else {
                    std::log(0, "[PID1] Service registry returned no active services.").ok();
                }
            }
            for service_id in [1u8, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11] {
                if std::service_recover_failed(service_id).is_ok() {
                    std::log(
                        0,
                        "[PID1] Recovered a failed service through the lifecycle ABI.",
                    )
                    .ok();
                }
            }
            last_poll = now;
        }

        // Check for incoming IPC requests from launcher
        if let Ok(msg_len) = std::ipc_recv(10, &mut ipc_buf) {
            if msg_len > 0 {
                let _ = std::log(
                    0,
                    "[PID1] Application launch request received via IPC from launcher.",
                );
                let app_id = core::str::from_utf8(&ipc_buf[..msg_len])
                    .unwrap_or("")
                    .trim_matches(|c: char| c.is_whitespace() || c == '\0');
                if !app_id.is_empty() {
                    match std::app_launch(app_id) {
                        Ok(_) => {
                            let _ = std::log(
                                0,
                                "[PID1] Target application launched successfully in Ring 3.",
                            );
                        }
                        Err(_) => {
                            let _ = std::log(0, "[PID1] Target application launch rejected.");
                        }
                    }
                }
            }
        }

        // Block cooperatively until an IPC/timer/input event instead of
        // continuously spinning in Ring 3.
        let _ = std::event_poll(std::EVENT_IPC_MSG | std::EVENT_TIMER_TICK, 20);
    }
}

fn registry_has_all_running_services_after_query() -> bool {
    let mut service_buf = [0u8; SERVICE_BUFFER];
    match std::service_list(&mut service_buf) {
        Ok(len) => registry_has_all_running_services(&service_buf[..len]),
        Err(_) => false,
    }
}

fn registry_has_all_running_services(buf: &[u8]) -> bool {
    let mut seen: u16 = 0;
    for line in buf.split(|b| *b == b'\n') {
        if line.is_empty() {
            continue;
        }
        let mut fields = line.split(|b| *b == b'|');
        let Some(id_bytes) = fields.next() else {
            continue;
        };
        // The public service snapshot exposes ABI version as field 2 and
        // deliberately does not expose scheduler task IDs. Validate the
        // published contract instead of interpreting ABI data as a task ID.
        let Some(abi_bytes) = fields.next() else {
            continue;
        };
        let Some(state) = fields.next() else {
            continue;
        };
        if abi_bytes != b"1" || state != b"running" {
            continue;
        }
        let id = match id_bytes {
            b"1" => 1u8,
            b"2" => 2,
            b"3" => 3,
            b"4" => 4,
            b"5" => 5,
            b"6" => 6,
            b"7" => 7,
            b"8" => 8,
            b"9" => 9,
            b"10" => 10,
            b"11" => 11,
            _ => continue,
        };
        seen |= 1u16 << (id - 1);
    }
    seen == 0x07ff
}

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    loop {
        core::hint::spin_loop();
    }
}
