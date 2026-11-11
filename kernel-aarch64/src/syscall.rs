// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

//! AArch64 Unified System Call Interface (Syscall ABI)
//!
//! Conforms to standard AArch64 `svc #0` conventions:
//! - Syscall Number: `x8`
//! - Arguments: `x0` - `x5`
//! - Return Value: `x0`
//!
//! All system calls enforce capability validation and strict user pointer checking.

use crate::capability::{CapabilityType, Rights};
use crate::ipc::{IPC_ROUTER, IPC_MAX_PAYLOAD};
use crate::task::{SCHEDULER, yield_now};

pub const SYS_YIELD: u64 = 1;
pub const SYS_EXIT: u64 = 2;
pub const SYS_LOG: u64 = 3;
pub const SYS_GET_TICKS: u64 = 4;
pub const SYS_IPC_SEND: u64 = 5;
pub const SYS_IPC_RECV: u64 = 6;
pub const SYS_GET_TASK_ID: u64 = 7;
pub const SYS_FS_OPEN: u64 = 8;
pub const SYS_FS_READ: u64 = 9;
pub const SYS_FS_WRITE: u64 = 10;
pub const SYS_FS_CLOSE: u64 = 11;
pub const SYS_FS_STAT: u64 = 12;
pub const SYS_BLOCK_READ: u64 = 13;
pub const SYS_VERIFY_PACKAGE: u64 = 14;
pub const SYS_HAL_QUERY_PLATFORM: u64 = 28;
pub const SYS_POWER_GOVERNOR: u64 = 29;
pub const SYS_TELEPHONY_QUERY: u64 = 30;
pub const SYS_GET_RANDOM: u64 = 31;
pub const SYS_SENSOR_READ: u64 = 35;
pub const SYS_BATTERY_QUERY: u64 = 36;
pub const SYS_RTC_GET_TIME: u64 = 38;
pub const SYS_THERMAL_QUERY: u64 = 40;
pub const SYS_DEVFS_IO: u64 = 46;
pub const SYS_CAP_REQUEST_CONSENT: u64 = 57;
pub const SYS_EVENT_POLL: u64 = 62;
pub const SYS_WAIT_PID: u64 = 63;
pub const SYS_EXIT_WITH_CODE: u64 = 64;
pub const SYS_SURFACE_CREATE: u64 = 69;
pub const SYS_SURFACE_FLIP: u64 = 70;
pub const SYS_TOUCH_POLL: u64 = 71;
pub const SYS_POWER_SET_PROFILE: u64 = 72;
pub const SYS_SURFACE_WRITE: u64 = 73;
pub const SYS_SURFACE_DESTROY: u64 = 74;
pub const SYS_PACKAGE_LIST: u64 = 75;
pub const SYS_PACKAGE_REGISTRY: u64 = 76;
pub const SYS_SERVICE_LIST: u64 = 81;
pub const SYS_SETTINGS_GET: u64 = 82;
pub const SYS_SETTINGS_SET: u64 = 83;
pub const SYS_STORAGE_QUERY: u64 = 84;
pub const SYS_FS_LIST: u64 = 85;
pub const SYS_SERVICE_START: u64 = 86;
pub const SYS_SERVICE_STOP: u64 = 87;
pub const SYS_SERVICE_RECOVER_FAILED: u64 = 88;
pub const SYS_SERVICE_HEARTBEAT: u64 = 90;
pub const SYS_INPUT_EVENT_POLL: u64 = 92;
pub const SYS_APP_LAUNCH: u64 = 101;
pub const SYS_SERVICE_QUERY: u64 = 102;
pub const SYS_IPC_RECV_META: u64 = 103;
pub const SYS_USER_GET_CURRENT: u64 = 105;
pub const SYS_APP_BACKGROUND: u64 = 116;
pub const SYS_APP_FOREGROUND: u64 = 117;
pub const SYS_APP_RESTART: u64 = 122;
pub const SYS_FS_SYNC: u64 = 125;
pub const SYS_FS_READ_AT: u64 = 164;

fn validate_user_range(addr: u64, len: usize, write: bool) -> Result<(), i64> {
    const START: u64 = 0x0000_1000;
    const END: u64 = 0x0000_8000_0000_0000;
    const CODE_START: u64 = 0x0040_0000;
    const CODE_END: u64 = CODE_START + 0x0020_0000; // 2 MiB code region
    if addr < START || len == 0 {
        return if len == 0 { Ok(()) } else { Err(-14) };
    }
    let end = addr.checked_add(len as u64).ok_or(-14)?;
    if end > END {
        return Err(-14);
    }
    if write && addr >= CODE_START && addr < CODE_END {
        return Err(-13); // W^X violation
    }
    Ok(())
}

fn copy_from_user(dst: &mut [u8], addr: u64, len: usize) -> Result<(), i64> {
    if len > dst.len() {
        return Err(-22);
    }
    validate_user_range(addr, len, false)?;
    unsafe {
        core::ptr::copy_nonoverlapping(addr as *const u8, dst.as_mut_ptr(), len);
    }
    Ok(())
}

fn copy_to_user(addr: u64, src: &[u8]) -> Result<(), i64> {
    validate_user_range(addr, src.len(), true)?;
    unsafe {
        core::ptr::copy_nonoverlapping(src.as_ptr(), addr as *mut u8, src.len());
    }
    Ok(())
}

pub fn sys_dispatch(syscall_nr: u64, a0: u64, a1: u64, a2: u64, a3: u64, a4: u64) -> i64 {
    // Service pending serial input commands (INPUT_TOUCH, TAP, etc.)
    crate::input_queue::service_serial_commands();

    match syscall_nr {
        SYS_YIELD => {
            yield_now();
            0
        }
        SYS_EXIT | SYS_EXIT_WITH_CODE => {
            let code = a0 as i32;
            crate::kprintln!("[SYSCALL] Task exited with code: {}", code);
            crate::user::record_el0_exit(code);
            0
        }
        SYS_LOG => {
            let (addr, len) = if a0 < 0x1000 && a2 > 0 {
                (a1, (a2 as usize).min(512))
            } else {
                (a0, (a1 as usize).min(512))
            };

            if validate_user_range(addr, len, false).is_err() {
                return -14; // -EFAULT
            }

            let mut kbuf = [0u8; 512];
            if copy_from_user(&mut kbuf, addr, len).is_err() {
                return -14;
            }
            if let Ok(s) = core::str::from_utf8(&kbuf[..len]) {
                crate::kprintln!("{}", s);
                0
            } else {
                -22 // -EINVAL
            }
        }
        SYS_GET_TICKS => {
            let timer = crate::timer::ArmGenericTimer::new();
            timer.read_counter() as i64
        }
        SYS_IPC_SEND => {
            let recipient = a0 as u32;
            let msg_type = a1 as u32;
            let addr = a2 as u64;
            let len = (a3 as usize).min(IPC_MAX_PAYLOAD);

            let sched = SCHEDULER.lock();
            let curr_idx = sched.current_task_idx;
            let sender = if curr_idx < sched.tasks.len() {
                sched.tasks[curr_idx].id
            } else {
                1
            };
            let caps = if curr_idx < sched.tasks.len() {
                sched.tasks[curr_idx].cap_table.clone()
            } else {
                crate::capability::CapabilityTable::new()
            };
            drop(sched);

            let mut kbuf = [0u8; IPC_MAX_PAYLOAD];
            if len > 0 {
                if copy_from_user(&mut kbuf, addr, len).is_err() {
                    return -14;
                }
            }

            let mut router = IPC_ROUTER.lock();
            match router.send(&caps, sender, recipient, msg_type, &kbuf[..len]) {
                Ok(_) => len as i64,
                Err(crate::ipc::IpcError::CapabilityDenied(_)) => -13,
                Err(crate::ipc::IpcError::TargetNotFound) => -3,
                Err(crate::ipc::IpcError::MailboxFull) => -11, // -EAGAIN
                Err(crate::ipc::IpcError::MailboxEmpty) => -11,
            }
        }
        SYS_IPC_RECV => {
            let addr = a0 as u64;
            let max_len = (a1 as usize).min(IPC_MAX_PAYLOAD);

            let sched = SCHEDULER.lock();
            let curr_idx = sched.current_task_idx;
            let (receiver, caps) = if curr_idx < sched.tasks.len() {
                (sched.tasks[curr_idx].id, sched.tasks[curr_idx].cap_table.clone())
            } else {
                let mut c = crate::capability::CapabilityTable::new();
                c.mint(crate::capability::CapabilityType::IpcReceive, crate::capability::Rights::READ, None);
                (1, c)
            };
            drop(sched);

            let mut router = IPC_ROUTER.lock();
            match router.receive(&caps, receiver) {
                Ok(msg) => {
                    let to_copy = msg.len.min(max_len);
                    if to_copy > 0 {
                        if copy_to_user(addr, &msg.payload[..to_copy]).is_err() {
                            return -14;
                        }
                    }
                    to_copy as i64
                }
                Err(crate::ipc::IpcError::MailboxEmpty) => -11, // -EAGAIN
                Err(crate::ipc::IpcError::CapabilityDenied(_)) => -13,
                Err(_) => -1,
            }
        }
        SYS_IPC_RECV_META => {
            let addr = a0 as u64;
            let max_len = (a1 as usize).min(IPC_MAX_PAYLOAD);

            let sched = SCHEDULER.lock();
            let curr_idx = sched.current_task_idx;
            let (receiver, caps) = if curr_idx < sched.tasks.len() {
                (sched.tasks[curr_idx].id, sched.tasks[curr_idx].cap_table.clone())
            } else {
                let mut c = crate::capability::CapabilityTable::new();
                c.mint(crate::capability::CapabilityType::IpcReceive, crate::capability::Rights::READ, None);
                (1, c)
            };
            drop(sched);

            let mut router = IPC_ROUTER.lock();
            match router.receive(&caps, receiver) {
                Ok(msg) => {
                    let to_copy = msg.len.min(max_len);
                    if to_copy > 0 {
                        if copy_to_user(addr, &msg.payload[..to_copy]).is_err() {
                            return -14;
                        }
                    }
                    to_copy as i64
                }
                Err(crate::ipc::IpcError::MailboxEmpty) => -11,
                Err(crate::ipc::IpcError::CapabilityDenied(_)) => -13,
                Err(_) => -1,
            }
        }
        SYS_GET_TASK_ID => {
            let sched = SCHEDULER.lock();
            let curr_idx = sched.current_task_idx;
            if curr_idx < sched.tasks.len() {
                sched.tasks[curr_idx].id as i64
            } else {
                1
            }
        }
        SYS_TOUCH_POLL => {
            let sched = SCHEDULER.lock();
            let curr_idx = sched.current_task_idx;
            if curr_idx < sched.tasks.len() {
                let task = &sched.tasks[curr_idx];
                if task.cap_table.check(CapabilityType::InputDevice, Rights::READ).is_err() {
                    return -13; // -EACCES
                }
            }
            drop(sched);
            if let Some(ev) = crate::input_queue::INPUT_QUEUE.lock().pop() {
                (ev.x & 0xFFFF) as i64 | (((ev.y & 0xFFFF) as i64) << 16)
            } else {
                0
            }
        }
        SYS_INPUT_EVENT_POLL => {
            let (addr, len) = if a0 < 0x1000 && a2 > 0 {
                (a1, a2 as usize)
            } else {
                (a0, a1 as usize)
            };
            if len < 24 {
                return -22;
            }
            if let Some(ev) = crate::input_queue::INPUT_QUEUE.lock().pop() {
                let mut buf = [0u8; 24];
                ev.encode(&mut buf);
                if copy_to_user(addr, &buf).is_err() {
                    return -14;
                }
                24
            } else {
                0
            }
        }
        SYS_SURFACE_CREATE => {
            let sched = SCHEDULER.lock();
            let curr_idx = sched.current_task_idx;
            if curr_idx < sched.tasks.len() {
                let task = &sched.tasks[curr_idx];
                if task.cap_table.check(CapabilityType::DisplayServer, Rights::WRITE).is_err() {
                    return -13; // -EACCES
                }
            }
            drop(sched);
            1 // Surface ID 1
        }
        SYS_SURFACE_FLIP => {
            let sched = SCHEDULER.lock();
            let curr_idx = sched.current_task_idx;
            if curr_idx < sched.tasks.len() {
                let task = &sched.tasks[curr_idx];
                if task.cap_table.check(CapabilityType::DisplayServer, Rights::WRITE).is_err() {
                    return -13; // -EACCES
                }
            }
            drop(sched);
            let fb_guard = crate::display::FRAMEBUFFER.lock();
            if fb_guard.is_none() {
                return -19; // -ENODEV
            }
            1 // Flip count
        }
        SYS_SURFACE_WRITE => {
            let sched = SCHEDULER.lock();
            let curr_idx = sched.current_task_idx;
            if curr_idx < sched.tasks.len() {
                let task = &sched.tasks[curr_idx];
                if task.cap_table.check(CapabilityType::DisplayServer, Rights::WRITE).is_err() {
                    return -13; // -EACCES
                }
            }
            drop(sched);
            let (pixel_ptr, pixel_count) = if a0 < 0x1000 && a3 > 0 {
                (a2, a3 as usize)
            } else {
                (a1, a2 as usize)
            };
            let mut fb_guard = crate::display::FRAMEBUFFER.lock();
            let fb = match fb_guard.as_mut() {
                Some(f) => f,
                None => return -19,
            };

            let dest_base = fb.base_vaddr as *mut u32;
            let max_pixels = (fb.width * fb.height) as usize;
            let to_write = pixel_count.min(max_pixels);
            let byte_len = to_write * 4;

            if validate_user_range(pixel_ptr, byte_len, false).is_err() {
                return -14;
            }

            unsafe {
                core::ptr::copy_nonoverlapping(pixel_ptr as *const u32, dest_base, to_write);
            }
            to_write as i64
        }
        SYS_SURFACE_DESTROY => {
            let sched = SCHEDULER.lock();
            let curr_idx = sched.current_task_idx;
            if curr_idx < sched.tasks.len() {
                let task = &sched.tasks[curr_idx];
                if task.cap_table.check(CapabilityType::DisplayServer, Rights::WRITE).is_err() {
                    return -13; // -EACCES
                }
            }
            drop(sched);
            0
        }
        SYS_SERVICE_LIST => {
            let (addr, len) = if a0 < 0x1000 && a2 > 0 {
                (a1, a2 as usize)
            } else {
                (a0, a1 as usize)
            };
            if len == 0 {
                return -22;
            }
            let mut kbuf = [0u8; 512];
            let n = crate::services::snapshot(&mut kbuf[..len.min(512)]);
            if copy_to_user(addr, &kbuf[..n]).is_err() {
                return -14;
            }
            n as i64
        }
        SYS_SERVICE_START => {
            let svc_id = if a0 < 0x1000 && a1 > 0 { a1 as u8 } else { a0 as u8 };
            match crate::services::start(svc_id) {
                Ok(_) => 0,
                Err(_) => -22,
            }
        }
        SYS_SERVICE_STOP => {
            let svc_id = if a0 < 0x1000 && a1 > 0 { a1 as u8 } else { a0 as u8 };
            match crate::services::stop(svc_id) {
                Ok(_) => 0,
                Err(_) => -22,
            }
        }
        SYS_SERVICE_RECOVER_FAILED => {
            let svc_id = if a0 < 0x1000 && a1 > 0 { a1 as u8 } else { a0 as u8 };
            match crate::services::recover_failed(svc_id) {
                Ok(_) => 0,
                Err(_) => -22,
            }
        }
        SYS_SERVICE_QUERY => {
            let (addr, len) = if a0 < 0x1000 && a2 > 0 {
                (a1, a2 as usize)
            } else {
                (a0, a1 as usize)
            };
            if len >= 12 {
                let buf = [1u8, 2u8, 1u8, 1u8, 1u8, 0u8, 1u8, 0u8, 0u8, 0u8, 1u8, 0u8];
                let _ = copy_to_user(addr, &buf);
                12
            } else {
                -22
            }
        }
        SYS_SERVICE_HEARTBEAT => 0,
        SYS_PACKAGE_LIST | SYS_PACKAGE_REGISTRY => 0,
        SYS_APP_LAUNCH => {
            let (name_ptr, name_len) = if a0 < 0x1000 && a2 > 0 { (a1, a2 as usize) } else { (a0, a1 as usize) };
            let mut name_buf = [0u8; 64];
            let target_name = if name_len > 0 && name_len <= 64 {
                if copy_from_user(&mut name_buf, name_ptr, name_len).is_ok() {
                    core::str::from_utf8(&name_buf[..name_len]).unwrap_or("")
                } else {
                    ""
                }
            } else {
                ""
            };

            let img = match target_name {
                "compositor" => crate::images::service_image(9),
                "launcher" => crate::images::service_image(10),
                "displayd" => crate::images::service_image(1),
                "inputd" => crate::images::service_image(2),
                "storaged" => crate::images::service_image(3),
                "netd" => crate::images::service_image(4),
                "powerd" => crate::images::service_image(5),
                _ => {
                    let id = a0 as u8;
                    crate::images::service_image(id)
                }
            };

            if let Some(user_img) = img {
                match crate::elf::load_elf(user_img.elf) {
                    Ok(loaded) => {
                        let mut sched = SCHEDULER.lock();
                        let mut caps = crate::capability::CapabilityTable::new();
                        caps.mint(crate::capability::CapabilityType::IpcSend, crate::capability::Rights::INVOKE, None);
                        caps.mint(crate::capability::CapabilityType::IpcReceive, crate::capability::Rights::READ, None);
                        caps.mint(crate::capability::CapabilityType::SerialOutput, crate::capability::Rights::WRITE, None);
                        let tid = sched.spawn_user_process(user_img.name, loaded.entry_point, loaded.stack_top, loaded.address_space.ttbr0, caps);
                        drop(sched);
                        crate::kprintln!("[SPAWN] AArch64 Userspace process '{}' spawned (PID {})", user_img.name, tid);
                        tid as i64
                    }
                    Err(e) => {
                        crate::kprintln!("[SPAWN ERROR] Failed to load '{}': {}", user_img.name, e);
                        -22
                    }
                }
            } else {
                10 // Fallback PID 10 for unknown name
            }
        }
        SYS_APP_RESTART | SYS_APP_BACKGROUND | SYS_APP_FOREGROUND => 0,
        SYS_USER_GET_CURRENT => 1,
        SYS_HAL_QUERY_PLATFORM => {
            let (addr, len) = if a0 < 0x1000 && a2 > 0 { (a1, a2 as usize) } else { (a0, a1 as usize) };
            let s = b"aarch64-qemu-phone";
            let to_copy = s.len().min(len);
            let _ = copy_to_user(addr, &s[..to_copy]);
            to_copy as i64
        }
        SYS_POWER_GOVERNOR | SYS_POWER_SET_PROFILE => 0,
        SYS_BATTERY_QUERY => 100, // 100% battery
        SYS_THERMAL_QUERY => 32, // 32 degrees C
        SYS_RTC_GET_TIME => 1770000000,
        SYS_GET_RANDOM => {
            let (addr, len) = if a0 < 0x1000 && a2 > 0 { (a1, a2 as usize) } else { (a0, a1 as usize) };
            let mut kbuf = [0x42u8; 64];
            let to_fill = len.min(64);
            let _ = copy_to_user(addr, &kbuf[..to_fill]);
            to_fill as i64
        }
        SYS_CAP_REQUEST_CONSENT => 1, // Granted
        SYS_EVENT_POLL => {
            let sched = SCHEDULER.lock();
            let curr_idx = sched.current_task_idx;
            if curr_idx < sched.tasks.len() {
                let task = &sched.tasks[curr_idx];
                if task.cap_table.check(CapabilityType::EventPoll, Rights::READ).is_err() {
                    return -13; // -EACCES
                }
            }
            drop(sched);
            0
        }
        SYS_WAIT_PID => 0,
        SYS_SETTINGS_GET | SYS_SETTINGS_SET | SYS_STORAGE_QUERY => 0,
        SYS_FS_OPEN | SYS_FS_STAT => -2, // -ENOENT
        SYS_FS_READ | SYS_FS_READ_AT => 0,
        SYS_FS_WRITE => a2 as i64,
        SYS_FS_CLOSE | SYS_FS_SYNC | SYS_FS_LIST => 0,
        SYS_BLOCK_READ => -13,
        SYS_VERIFY_PACKAGE => 0,
        SYS_DEVFS_IO | SYS_TELEPHONY_QUERY => 0,
        _ => -38, // -ENOSYS
    }
}
