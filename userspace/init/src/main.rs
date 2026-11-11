// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

//! Inter OS userspace PID 1 / service-manager bootstrap.
//!
//! PID 1 deliberately contains no hardware-policy demos or application logic.
//! The kernel owns only the minimum primitives; this process owns userspace
//! boot ordering, service observation, and the transition to the normal event loop.

#![no_std]
#![no_main]

use core::panic::PanicInfo;
use interos_std as std;

const SERVICE_BUFFER: usize = 1024;
const POLL_INTERVAL_TICKS: u64 = 100;

#[no_mangle]
pub extern "C" fn _start() -> ! {
    std::log("[PID1] Inter OS userspace init started in Ring 3.").ok();

    // PID 1 discovers declared services from the authoritative kernel registry.
    let mut remaining = SERVICE_RETRY_ROUNDS;
    while remaining > 0 {
        let mut service_buf = [0u8; SERVICE_BUFFER];
        let Ok(len) = std::service_list(&mut service_buf) else { break; };
        let progress = start_declared_services(&service_buf[..len]);
        let Ok(len) = std::service_list(&mut service_buf) else { break; };
        if registry_has_all_running_services(&service_buf[..len]) {
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
            let _ = std::log("[PID1] Service stop lifecycle contract exercised.");
            match std::service_start(11) {
                Ok(_) => {
                    let _ = std::log("[PID1] Service restart lifecycle contract exercised.");
                }
                Err(_) => {
                    let _ = std::log("[PID1] Service restart lifecycle contract failed.");
                }
            }
        }
        Err(_) => {
            let _ = std::log("[PID1] Service stop lifecycle contract failed.");
        }
    }

    // Establish the first userspace service-manager contract: PID 1 observes
    // the authoritative kernel service registry rather than hard-coding task IDs.
    let mut service_buf = [0u8; SERVICE_BUFFER];
    if let Ok(len) = std::service_list(&mut service_buf) {
        if registry_has_all_running_services(&service_buf[..len]) {
            let _ = std::log("[PID1] All system services running after bootstrap.");
        } else {
            let _ = std::log("[PID1] Service bootstrap validation failed.");
        }
    }

    let mut last_poll = 0u64;
    let mut ipc_buf = [0u8; 64];

    loop {
        let now = std::get_ticks().unwrap_or(last_poll);

        if now.saturating_sub(last_poll) >= POLL_INTERVAL_TICKS || last_poll == 0 {
            if let Ok(len) = std::service_list(&mut service_buf) {
                if len > 0 {
                    std::log("[PID1] Service registry observed; userspace lifecycle loop active.",
                    )
                    .ok();
                } else {
                    std::log("[PID1] Service registry returned no active services.").ok();
                }
            }
            let mut recovery_buf = [0u8; SERVICE_BUFFER];
            if let Ok(recovery_len) = std::service_list(&mut recovery_buf) {
                for service_id in declared_service_ids(&recovery_buf[..recovery_len]).iter().copied().flatten() {
                    if std::service_recover_failed(service_id).is_ok() {
                        std::log("[PID1] Recovered a failed service through the lifecycle ABI.",
                        )
                        .ok();
                    }
                }
            }
            last_poll = now;
        }

        // Check for incoming IPC requests from launcher
        if let Ok(msg_len) = std::ipc_recv(&mut ipc_buf) {
            if msg_len > 0 {
                let _ = std::log("[PID1] Application launch request received via IPC from launcher.",
                );
                let app_id = core::str::from_utf8(&ipc_buf[..msg_len])
                    .unwrap_or("")
                    .trim_matches(|c: char| c.is_whitespace() || c == '\0');
                if !app_id.is_empty() {
                    match std::app_launch(app_id) {
                        Ok(_) => {
                            let _ = std::log("[PID1] Target application launched successfully in Ring 3.",
                            );
                        }
                        Err(_) => {
                            let _ = std::log("[PID1] Target application launch rejected.");
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

const SERVICE_RETRY_ROUNDS: u8 = 16;

fn parse_service_id(bytes: &[u8]) -> Option<u8> {
    if bytes.is_empty() {
        return None;
    }
    let mut id = 0u16;
    for byte in bytes {
        if !byte.is_ascii_digit() {
            return None;
        }
        id = id.checked_mul(10)?.checked_add((byte - b'0') as u16)?;
        if id > u8::MAX as u16 {
            return None;
        }
    }
    let id = id as u8;
    if id == 0 {
        None
    } else {
        Some(id)
    }
}

fn declared_service_ids(buf: &[u8]) -> [Option<u8>; 32] {
    let mut ids = [None; 32];
    let mut count = 0usize;
    for line in buf.split(|b| *b == b'\n') {
        let Some(id_bytes) = line.split(|b| *b == b'|').next() else {
            continue;
        };
        let Some(id) = parse_service_id(id_bytes) else {
            continue;
        };
        if count < ids.len() {
            ids[count] = Some(id);
            count += 1;
        }
    }
    ids
}

fn start_declared_services(buf: &[u8]) -> bool {
    let mut progress = false;
    for service_id in declared_service_ids(buf).iter().copied().flatten() {
        if std::service_start(service_id).is_ok() {
            progress = true;
        }
    }
    progress
}

#[allow(dead_code)]
fn registry_has_all_running_services_after_query() -> bool {
    let mut service_buf = [0u8; SERVICE_BUFFER];
    match std::service_list(&mut service_buf) {
        Ok(len) => registry_has_all_running_services(&service_buf[..len]),
        Err(_) => false,
    }
}

fn registry_has_all_running_services(buf: &[u8]) -> bool {
    let mut declared = 0usize;
    let mut running = 0usize;
    for line in buf.split(|b| *b == b'\n') {
        if line.is_empty() {
            continue;
        }
        let mut fields = line.split(|b| *b == b'|');
        let Some(id_bytes) = fields.next() else {
            continue;
        };
        let Some(abi_bytes) = fields.next() else {
            continue;
        };
        let Some(state) = fields.next() else {
            continue;
        };
        if parse_service_id(id_bytes).is_none() {
            continue;
        }
        declared += 1;
        if abi_bytes != b"1" || state != b"running" {
            continue;
        }
        running += 1;
    }
    declared > 0 && running == declared
}

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    loop {
        core::hint::spin_loop();
    }
}
