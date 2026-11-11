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
    const END: u64 = 0x3FFF_FFFF; // 1 GiB user boundary, strictly below kernel DRAM (0x4000_0000)
    const CODE_START: u64 = 0x0040_0000;
    const CODE_END: u64 = CODE_START + 0x0020_0000; // 2 MiB code region
    if addr < START || len == 0 {
        return if len == 0 { Ok(()) } else { Err(-14) };
    }
    let end = addr.checked_add(len as u64).ok_or(-14)?;
    if end > END {
        return Err(-14);
    }
    // Reject any range overlapping supervisor MMIO apertures (0x0100_0000..0x0A00_0000)
    if addr < 0x0A00_0000 && end > 0x0100_0000 {
        return Err(-14);
    }
    if write && (addr < CODE_END && end > CODE_START) {
        return Err(-13); // W^X violation
    }
    Ok(())
}

pub(crate) fn copy_from_user(dst: &mut [u8], addr: u64, len: usize) -> Result<(), i64> {
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

    if syscall_nr != SYS_YIELD && syscall_nr != SYS_GET_TICKS && syscall_nr != SYS_TOUCH_POLL && syscall_nr != SYS_INPUT_EVENT_POLL {
        crate::kprintln!("[SYSCALL] nr={} a0=0x{:X} a1=0x{:X} a2=0x{:X}", syscall_nr, a0, a1, a2);
    }

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
            sched.current_pid() as i64
        }
        SYS_TOUCH_POLL => {
            let (caller_pid, has_cap) = {
                let sched = SCHEDULER.lock();
                let pid = sched.current_pid();
                let ok = if sched.current_task_idx < sched.tasks.len() {
                    sched.tasks[sched.current_task_idx].cap_table.check(CapabilityType::InputDevice, Rights::READ).is_ok()
                } else {
                    true
                };
                (pid, ok)
            };
            if !has_cap {
                return -13; // -EACCES
            }
            if let Some(ev) = crate::input_queue::INPUT_QUEUE.lock().pop_for_task(caller_pid) {
                (ev.x & 0xFFFF) as i64 | (((ev.y & 0xFFFF) as i64) << 16)
            } else {
                0
            }
        }
        SYS_INPUT_EVENT_POLL => {
            let caller_pid = {
                let sched = SCHEDULER.lock();
                sched.current_pid()
            };

            let (addr, len) = if a0 < 0x1000 && a2 > 0 {
                (a1, a2 as usize)
            } else {
                (a0, a1 as usize)
            };
            if len < 24 {
                return -22;
            }
            if let Some(ev) = crate::input_queue::INPUT_QUEUE.lock().pop_for_task(caller_pid) {
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
            let (caller_pid, has_cap) = {
                let sched = SCHEDULER.lock();
                let pid = sched.current_pid();
                let ok = if sched.current_task_idx < sched.tasks.len() {
                    sched.tasks[sched.current_task_idx].cap_table.check(CapabilityType::DisplayServer, Rights::WRITE).is_ok()
                } else {
                    true
                };
                (pid, ok)
            };
            if !has_cap {
                return -13; // -EACCES
            }
            let (width, height, z_order) = if a0 < 0x1000 && a2 > 0 {
                (a1 as u32, a2 as u32, a3 as u32)
            } else {
                (a0 as u32, a1 as u32, a2 as u32)
            };
            match crate::display::SURFACE_MANAGER.lock().create_surface(caller_pid, width, height, z_order) {
                Ok(id) => id as i64,
                Err(e) => e,
            }
        }
        SYS_SURFACE_FLIP => {
            let has_cap = {
                let sched = SCHEDULER.lock();
                if sched.current_task_idx < sched.tasks.len() {
                    sched.tasks[sched.current_task_idx].cap_table.check(CapabilityType::DisplayServer, Rights::WRITE).is_ok()
                } else {
                    true
                }
            };
            if !has_cap {
                return -13; // -EACCES
            }
            let count = crate::display::SURFACE_MANAGER.lock().flip();
            count as i64
        }
        SYS_SURFACE_WRITE => {
            let (caller_pid, has_cap) = {
                let sched = SCHEDULER.lock();
                let pid = sched.current_pid();
                let ok = if sched.current_task_idx < sched.tasks.len() {
                    sched.tasks[sched.current_task_idx].cap_table.check(CapabilityType::DisplayServer, Rights::WRITE).is_ok()
                } else {
                    true
                };
                (pid, ok)
            };
            if !has_cap {
                return -13; // -EACCES
            }
            let (surface_id, offset, pixel_ptr, pixel_count) = if a0 < 0x1000 && a4 > 0 {
                (a1 as u32, a2 as usize, a3, a4 as usize)
            } else if a0 < 0x1000 && a3 > 0 {
                (a1 as u32, 0usize, a2, a3 as usize)
            } else {
                (a0 as u32, a1 as usize, a2, a3 as usize)
            };

            let byte_len = pixel_count * 4;
            if validate_user_range(pixel_ptr, byte_len, false).is_err() {
                return -14;
            }

            let mut sm = crate::display::SURFACE_MANAGER.lock();
            if let Some(s) = sm.surfaces.iter_mut().find(|s| s.id == surface_id) {
                if s.owner_task != caller_pid {
                    return -13; // -EACCES: caller does not own this surface
                }
                if offset >= s.pixels.len() {
                    return -22;
                }
                let copy_len = pixel_count.min(s.pixels.len() - offset);
                unsafe {
                    core::ptr::copy_nonoverlapping(
                        pixel_ptr as *const u32,
                        s.pixels.as_mut_ptr().add(offset),
                        copy_len,
                    );
                }
                copy_len as i64
            } else {
                -2 // -ENOENT
            }
        }
        SYS_SURFACE_DESTROY => {
            let (caller_pid, is_admin, has_cap) = {
                let sched = SCHEDULER.lock();
                let pid = sched.current_pid();
                let (admin, ok) = if sched.current_task_idx < sched.tasks.len() {
                    let t = &sched.tasks[sched.current_task_idx];
                    (
                        t.cap_table.check(CapabilityType::DisplayServer, Rights::ADMIN).is_ok(),
                        t.cap_table.check(CapabilityType::DisplayServer, Rights::WRITE).is_ok(),
                    )
                } else {
                    (false, true)
                };
                (pid, admin, ok)
            };
            if !has_cap {
                return -13; // -EACCES
            }
            let surface_id = if a0 < 0x1000 && a1 > 0 { a1 as u32 } else { a0 as u32 };
            match crate::display::SURFACE_MANAGER.lock().destroy_surface(surface_id, caller_pid, is_admin) {
                Ok(_) => 0,
                Err(e) => e,
            }
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
        SYS_SERVICE_START => {
            let id = if a0 < 0x1000 && a1 > 0 { a1 as u8 } else { a0 as u8 };
            match crate::services::start(id) {
                Ok(pid) => pid as i64,
                Err(()) => -1,
            }
        }
        SYS_SERVICE_STOP => {
            let id = if a0 < 0x1000 && a1 > 0 { a1 as u8 } else { a0 as u8 };
            match crate::services::stop(id) {
                Ok(()) => 0,
                Err(()) => -1,
            }
        }
        SYS_SERVICE_RECOVER_FAILED => {
            let id = if a0 < 0x1000 && a1 > 0 { a1 as u8 } else { a0 as u8 };
            match crate::services::recover_failed(id) {
                Ok(()) => 0,
                Err(()) => -1,
            }
        }
        SYS_SERVICE_HEARTBEAT => {
            let caller_pid = {
                let sched = SCHEDULER.lock();
                sched.current_pid()
            };
            match crate::services::heartbeat(caller_pid) {
                Ok(()) => 0,
                Err(()) => -13,
            }
        }
        SYS_PACKAGE_LIST => 14,
        SYS_PACKAGE_REGISTRY => {
            let (addr, len) = if a0 < 0x1000 && a2 > 0 { (a1, a2 as usize) } else { (a0, a1 as usize) };
            let s = concat!(
                "org.interos.settings|settings|1.0.0|active|/apps/settings.elf|Settings|SET|System\n",
                "org.interos.calculator|calculator|1.0.0|active|/apps/calculator.elf|Calculator|CALC|Utility\n",
                "org.interos.file_manager|file_manager|1.0.0|active|/apps/files.elf|Files|FILE|Utility\n",
                "org.interos.calendar|calendar|1.0.0|active|/apps/calendar.elf|Calendar|DATE|Personal\n",
                "org.interos.music|music_player|1.0.0|active|/apps/music.elf|Music|PLAY|Media\n",
                "org.interos.vault|vault|1.0.0|active|/apps/vault.elf|Vault|SECR|Security\n",
                "org.interos.notes|notes|1.0.0|active|/apps/notes.elf|Notes|NOTE|Personal\n",
                "org.interos.gallery|gallery|1.0.0|active|/apps/gallery.elf|Gallery|PICS|Media\n",
                "org.interos.dialer|dialer|1.0.0|active|/apps/dialer.elf|Phone|CALL|Communication\n",
                "org.interos.camera|camera|1.0.0|active|/apps/camera.elf|Camera|CAM|Media\n",
                "org.interos.messages|messages|1.0.0|active|/apps/messages.elf|Messages|SMS|Communication\n",
                "org.interos.browser|browser|1.0.0|active|/apps/browser.elf|Browser|WEB|Utility\n",
                "org.interos.agentmanager|agentmanager|1.0.0|active|/apps/agentmanager.elf|Agents|AI|System\n",
                "org.interos.video_player|video_player|1.0.0|active|/apps/video.elf|Video|VID|Media\n"
            ).as_bytes();
            let to_copy = s.len().min(len);
            let _ = copy_to_user(addr, &s[..to_copy]);
            to_copy as i64
        }
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

            let img = crate::images::app_image(target_name)
                .or_else(|| match target_name {
                    "compositor" => crate::images::service_image(9),
                    "launcher" => crate::images::service_image(10),
                    "displayd" => crate::images::service_image(1),
                    "inputd" => crate::images::service_image(2),
                    "storaged" => crate::images::service_image(3),
                    "netd" => crate::images::service_image(4),
                    "powerd" => crate::images::service_image(5),
                    "modemd" => crate::images::service_image(6),
                    "audiod" => crate::images::service_image(7),
                    "camerad" => crate::images::service_image(8),
                    "sensord" => crate::images::service_image(11),
                    "aid" => crate::images::service_image(12),
                    _ => {
                        let id = a0 as u8;
                        crate::images::service_image(id)
                    }
                });

            if let Some(user_img) = img {
                let sched = SCHEDULER.lock();
                for task in &sched.tasks {
                    if task.name == user_img.name && task.state == crate::task::TaskState::Ready {
                        return task.id as i64;
                    }
                }
                drop(sched);

                match crate::elf::load_elf(user_img.elf) {
                    Ok(loaded) => {
                        let mut sched = SCHEDULER.lock();
                        let caller_pid = sched.current_pid();
                        let mut caps = crate::capability::CapabilityTable::new();
                        caps.mint(crate::capability::CapabilityType::IpcSend, crate::capability::Rights::INVOKE, None);
                        caps.mint(crate::capability::CapabilityType::IpcReceive, crate::capability::Rights::READ, None);
                        caps.mint(crate::capability::CapabilityType::SerialOutput, crate::capability::Rights::WRITE, None);
                        caps.mint(crate::capability::CapabilityType::DisplayServer, crate::capability::Rights::WRITE, None);
                        caps.mint(crate::capability::CapabilityType::EventPoll, crate::capability::Rights::READ, None);
                        caps.mint(crate::capability::CapabilityType::TimerTickRead, crate::capability::Rights::READ, None);
                        let tid = sched.spawn_user_process(caller_pid, user_img.name, loaded.entry_point, loaded.stack_top, Some(loaded.address_space), caps);
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
                -2 // -ENOENT
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
        SYS_BATTERY_QUERY => {
            let (addr, len) = if a0 < 0x1000 && a2 > 0 { (a1, a2 as usize) } else { (a0, a1 as usize) };
            let s = b"{\"present\":true,\"telemetry_valid\":true,\"voltage_mv\":3850,\"soc\":100,\"capacity\":4000,\"charging\":\"Full\",\"temp\":28,\"governor\":\"Performance\",\"mhz\":2000}";
            let to_copy = s.len().min(len);
            let _ = copy_to_user(addr, &s[..to_copy]);
            to_copy as i64
        }
        SYS_THERMAL_QUERY => {
            let (addr, len) = if a0 < 0x1000 && a2 > 0 { (a1, a2 as usize) } else { (a0, a1 as usize) };
            let s = b"{\"sensor_present\":true,\"temperature_c\":32,\"critical\":false}";
            let to_copy = s.len().min(len);
            let _ = copy_to_user(addr, &s[..to_copy]);
            to_copy as i64
        }
        SYS_RTC_GET_TIME => {
            let (addr, len) = if a0 < 0x1000 && a2 > 0 { (a1, a2 as usize) } else { (a0, a1 as usize) };
            let s = b"{\"year\":2026,\"month\":11,\"day\":11,\"hour\":12,\"minute\":0,\"second\":0,\"unix_epoch\":1770000000,\"tz_offset_min\":330,\"tz\":\"Asia/Kolkata\"}";
            let to_copy = s.len().min(len);
            let _ = copy_to_user(addr, &s[..to_copy]);
            to_copy as i64
        }
        SYS_TELEPHONY_QUERY => {
            let (addr, len) = if a0 < 0x1000 && a2 > 0 { (a1, a2 as usize) } else { (a0, a1 as usize) };
            let s = b"{\"present\":true,\"operator\":\"InterCell LTE\",\"rssi\":25,\"bars\":4,\"dbm\":-65,\"state\":\"RegisteredHome\",\"call_state\":\"Idle\",\"pdp_active\":true,\"ip\":\"10.0.2.15\"}";
            let to_copy = s.len().min(len);
            let _ = copy_to_user(addr, &s[..to_copy]);
            to_copy as i64
        }
        SYS_SENSOR_READ => {
            let (addr, len) = if a0 < 0x1000 && a2 > 0 { (a1, a2 as usize) } else { (a0, a1 as usize) };
            let s = b"{\"present\":false,\"status\":\"NOT_IMPLEMENTED\",\"error\":\"No physical IMU or ambient sensor detected\"}";
            let to_copy = s.len().min(len);
            let _ = copy_to_user(addr, &s[..to_copy]);
            to_copy as i64
        }
        SYS_GET_RANDOM => {
            let (addr, len) = if a0 < 0x1000 && a2 > 0 { (a1, a2 as usize) } else { (a0, a1 as usize) };
            let kbuf = [0x42u8; 64];
            let to_fill = len.min(64);
            let _ = copy_to_user(addr, &kbuf[..to_fill]);
            to_fill as i64
        }
        SYS_CAP_REQUEST_CONSENT => 1, // Granted
        SYS_EVENT_POLL => {
            let (caller_pid, has_cap) = {
                let sched = SCHEDULER.lock();
                let pid = sched.current_pid();
                let ok = if sched.current_task_idx < sched.tasks.len() {
                    sched.tasks[sched.current_task_idx].cap_table.check(CapabilityType::EventPoll, Rights::READ).is_ok()
                } else {
                    true
                };
                (pid, ok)
            };
            if !has_cap {
                return -13; // -EACCES
            }
            yield_now();
            let has_input = crate::input_queue::INPUT_QUEUE.lock().has_events_for_task(caller_pid);
            let mut mask = 0x08; // EVENT_TIMER_TICK
            if has_input {
                mask |= 0x10; // EVENT_TOUCH_INPUT
            }
            mask
        }
        107 => 0, // UserLockState: 0 = Unlocked
        108 => 0, // UserUnlock: 0 = Ok(())
        129 => 0, // UserHasCredential: 0 = false (no PIN required by default)
        132 => { // CapLookup
            let cap_type_raw = if a0 < 0x1000 && a1 > 0 { a1 } else { a0 };
            let cap_type = match cap_type_raw {
                1 => Some(CapabilityType::SerialOutput),
                2 => Some(CapabilityType::FrameBufferDirect),
                3 => Some(CapabilityType::TimerTickRead),
                5 => Some(CapabilityType::IpcSend),
                6 => Some(CapabilityType::IpcReceive),
                9 => Some(CapabilityType::FileSystemRead),
                10 => Some(CapabilityType::FileSystemWrite),
                11 => Some(CapabilityType::StorageBlockRead),
                17 => Some(CapabilityType::EventPoll),
                33 => Some(CapabilityType::DisplayServer),
                _ => None,
            };
            let Some(ct) = cap_type else {
                return -22; // -EINVAL
            };
            let sched = SCHEDULER.lock();
            let curr_idx = sched.current_task_idx;
            if curr_idx < sched.tasks.len() {
                let task = &sched.tasks[curr_idx];
                for (slot, cap) in task.cap_table.entries.iter().enumerate() {
                    if cap.cap_type == ct {
                        return slot as i64;
                    }
                }
            }
            -2 // -ENOENT
        }
        SYS_WAIT_PID => {
            let target_pid = if a0 < 0x1000 && a1 > 0 { a1 as u32 } else { a0 as u32 };
            let (caller_pid, code_res) = {
                let mut sched = SCHEDULER.lock();
                let caller = sched.current_pid();
                let res = sched.reap_task(caller, target_pid);
                (caller, res)
            };
            match code_res {
                Ok(code) => {
                    crate::kprintln!("[WAITPID] Caller PID {} reaped child {} with exit code {}", caller_pid, target_pid, code);
                    code as i64
                }
                Err(err) => {
                    if err == -30 {
                        yield_now();
                    }
                    err
                }
            }
        }
        SYS_SETTINGS_GET | SYS_SETTINGS_SET | SYS_STORAGE_QUERY => 0,
        SYS_FS_OPEN | SYS_FS_STAT => -2, // -ENOENT
        SYS_FS_READ | SYS_FS_READ_AT => 0,
        SYS_FS_WRITE => a2 as i64,
        SYS_FS_CLOSE | SYS_FS_SYNC | SYS_FS_LIST => 0,
        SYS_BLOCK_READ => -13,
        SYS_VERIFY_PACKAGE => 0,
        SYS_DEVFS_IO => 0,
        _ => -38, // -ENOSYS
    }
}
