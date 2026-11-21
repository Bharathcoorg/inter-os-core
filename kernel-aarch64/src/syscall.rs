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
    let _ = addr.checked_add(len as u64).ok_or(-14i64)?;
    let ttbr0_from_task = {
        let sched = SCHEDULER.lock();
        if let Some(pos) = sched.current_task_pos() {
            if let Some(ref aspace) = sched.tasks[pos].address_space {
                if aspace.destroyed || aspace.is_closing() {
                    return Err(-14);
                }
                Some(aspace.ttbr0)
            } else {
                None
            }
        } else {
            None
        }
    };
    let ttbr0 = match ttbr0_from_task {
        Some(t) => t,
        None => {
            #[cfg(target_arch = "aarch64")]
            {
                let t: u64;
                unsafe {
                    core::arch::asm!("mrs {}, ttbr0_el1", out(reg) t, options(nostack, nomem));
                }
                t & 0x0000_FFFF_FFFF_F000
            }
            #[cfg(not(target_arch = "aarch64"))]
            {
                return Err(-14);
            }
        }
    };
    crate::mmu::validate_ttbr0_user_range(ttbr0, addr, len, write)
}

pub(crate) fn copy_from_user(dst: &mut [u8], addr: u64, len: usize) -> Result<(), i64> {
    if len > dst.len() {
        return Err(-22);
    }
    validate_user_range(addr, len, false)?;
    if len > 0 {
        unsafe {
            core::ptr::copy_nonoverlapping(addr as *const u8, dst.as_mut_ptr(), len);
        }
    }
    Ok(())
}

fn copy_to_user(addr: u64, src: &[u8]) -> Result<(), i64> {
    validate_user_range(addr, src.len(), true)?;
    if !src.is_empty() {
        unsafe {
            core::ptr::copy_nonoverlapping(src.as_ptr(), addr as *mut u8, src.len());
        }
    }
    Ok(())
}

fn get_caller_identity_and_caps() -> Result<(u32, crate::capability::CapabilityTable), i64> {
    let sched = SCHEDULER.lock();
    if let Some(pos) = sched.current_task_pos() {
        let t = &sched.tasks[pos];
        Ok((t.id, t.cap_table.clone()))
    } else {
        Err(-13) // -EACCES: fail closed
    }
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
            let raw_len = a3 as usize;
            if raw_len > IPC_MAX_PAYLOAD {
                return -22;
            }
            let len = raw_len;

            let (sender, caps) = match get_caller_identity_and_caps() {
                Ok(pair) => pair,
                Err(err) => return err,
            };

            let mut kbuf = [0u8; IPC_MAX_PAYLOAD];
            if len > 0 {
                if copy_from_user(&mut kbuf, addr, len).is_err() {
                    return -14;
                }
            } else if addr != 0 && validate_user_range(addr, 0, false).is_err() {
                return -14;
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
            let (addr, max_len) = if a0 < 0x1000 && (a1 >= 0x1000 || a2 > 0 || a1 == 0) {
                (a1, (a2 as usize).min(IPC_MAX_PAYLOAD))
            } else {
                (a0, (a1 as usize).min(IPC_MAX_PAYLOAD))
            };

            let (receiver, caps) = match get_caller_identity_and_caps() {
                Ok(pair) => pair,
                Err(err) => return err,
            };
            if max_len > 0 {
                if let Err(err) = validate_user_range(addr, max_len, true) {
                    return err;
                }
            }

            let recv_res = {
                let mut router = IPC_ROUTER.lock();
                router.receive(&caps, receiver)
            };
            match recv_res {
                Ok(msg) => {
                    let to_copy = msg.len.min(max_len);
                    if to_copy > 0 {
                        if let Err(err) = copy_to_user(addr, &msg.payload[..to_copy]) {
                            return err;
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
            let (addr, max_len, sender_ptr, type_ptr) = if a0 < 0x1000 {
                (a1, (a2 as usize).min(IPC_MAX_PAYLOAD), a3, a4)
            } else {
                (a0, (a1 as usize).min(IPC_MAX_PAYLOAD), a2, a3)
            };

            let (receiver, caps) = match get_caller_identity_and_caps() {
                Ok(pair) => pair,
                Err(err) => return err,
            };
            if max_len > 0 {
                if let Err(err) = validate_user_range(addr, max_len, true) {
                    return err;
                }
            }
            if sender_ptr != 0 {
                if (sender_ptr & 7) != 0 {
                    return -22;
                }
                if let Err(err) = validate_user_range(sender_ptr, 8, true) {
                    return err;
                }
            }
            if type_ptr != 0 {
                if (type_ptr & 3) != 0 {
                    return -22;
                }
                if let Err(err) = validate_user_range(type_ptr, 4, true) {
                    return err;
                }
            }

            let recv_res = {
                let mut router = IPC_ROUTER.lock();
                router.receive(&caps, receiver)
            };
            match recv_res {
                Ok(msg) => {
                    let to_copy = msg.len.min(max_len);
                    if to_copy > 0 {
                        if let Err(err) = copy_to_user(addr, &msg.payload[..to_copy]) {
                            return err;
                        }
                    }
                    if sender_ptr != 0 {
                        if let Err(err) = copy_to_user(sender_ptr, &(msg.sender as u64).to_le_bytes()) {
                            return err;
                        }
                    }
                    if type_ptr != 0 {
                        if let Err(err) = copy_to_user(type_ptr, &msg.msg_type.to_le_bytes()) {
                            return err;
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
            let pid = sched.current_pid();
            if pid == crate::task::INVALID_PID {
                -13
            } else {
                pid as i64
            }
        }
        SYS_TOUCH_POLL => {
            let (caller_pid, caps) = match get_caller_identity_and_caps() {
                Ok(pair) => pair,
                Err(err) => return err,
            };
            if caps.check(CapabilityType::InputDevice, Rights::READ).is_err() && caps.check(CapabilityType::EventPoll, Rights::READ).is_err() {
                return -13; // -EACCES
            }
            if let Some(ev) = crate::input_queue::INPUT_QUEUE.lock().pop_for_task(caller_pid) {
                (ev.x & 0xFFFF) as i64 | (((ev.y & 0xFFFF) as i64) << 16)
            } else {
                0
            }
        }
        SYS_INPUT_EVENT_POLL => {
            let (caller_pid, caps) = match get_caller_identity_and_caps() {
                Ok(pair) => pair,
                Err(err) => return err,
            };
            if caps.check(CapabilityType::InputDevice, Rights::READ).is_err() && caps.check(CapabilityType::EventPoll, Rights::READ).is_err() {
                return -13; // -EACCES: capability check enforced fail-closed
            }

            let (addr, len) = if a0 < 0x1000 && a2 > 0 {
                (a1, a2 as usize)
            } else {
                (a0, a1 as usize)
            };
            if len < 24 {
                return -22;
            }
            if let Err(err) = validate_user_range(addr, 24, true) {
                return err;
            }
            if let Some(ev) = crate::input_queue::INPUT_QUEUE.lock().pop_for_task(caller_pid) {
                let mut buf = [0u8; 24];
                ev.encode(&mut buf);
                if let Err(err) = copy_to_user(addr, &buf) {
                    return err;
                }
                24
            } else {
                0
            }
        }
        SYS_SURFACE_CREATE => {
            let (caller_pid, caps) = match get_caller_identity_and_caps() {
                Ok(pair) => pair,
                Err(err) => return err,
            };
            if caps.check(CapabilityType::DisplayServer, Rights::WRITE).is_err() {
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
            let (_caller_pid, caps) = match get_caller_identity_and_caps() {
                Ok(pair) => pair,
                Err(err) => return err,
            };
            if caps.check(CapabilityType::DisplayServer, Rights::WRITE).is_err() {
                return -13; // -EACCES
            }
            let count = crate::display::SURFACE_MANAGER.lock().flip();
            count as i64
        }
        SYS_SURFACE_WRITE => {
            let (caller_pid, caps) = match get_caller_identity_and_caps() {
                Ok(pair) => pair,
                Err(err) => return err,
            };
            if caps.check(CapabilityType::DisplayServer, Rights::WRITE).is_err() {
                return -13; // -EACCES
            }
            let (surface_id, offset, pixel_ptr, pixel_count) = if a0 == 0 {
                (a1 as u32, a2 as usize, a3, a4 as usize)
            } else if a0 < 0x1000 && a3 >= 0x1000 && a2 < 0x1000 {
                (a1 as u32, a2 as usize, a3, a4 as usize)
            } else {
                (a0 as u32, a1 as usize, a2, a3 as usize)
            };

            if (pixel_ptr & 3) != 0 {
                return -22;
            }
            let byte_len = match pixel_count.checked_mul(4) {
                Some(b) => b,
                None => return -22,
            };
            if let Err(err) = validate_user_range(pixel_ptr, byte_len, false) {
                return err;
            }
            if pixel_count == 0 {
                return 0;
            }

            let mut written = 0usize;
            let mut kbuf = [0u8; 256 * 4];
            while written < pixel_count {
                let chunk = (pixel_count - written).min(256);
                let src_addr = match written.checked_mul(4).and_then(|b| pixel_ptr.checked_add(b as u64)) {
                    Some(a) => a,
                    None => return -14,
                };
                if let Err(err) = copy_from_user(&mut kbuf[..chunk * 4], src_addr, chunk * 4) {
                    return err;
                }
                let mut sm = crate::display::SURFACE_MANAGER.lock();
                if let Some(s) = sm.surfaces.iter_mut().find(|s| s.id == surface_id) {
                    if s.owner_task != caller_pid {
                        return -13; // -EACCES: caller does not own this surface
                    }
                    let dst_off = match offset.checked_add(written) {
                        Some(o) if o < s.pixels.len() => o,
                        _ => return if written == 0 { -22 } else { written as i64 },
                    };
                    let copy_len = chunk.min(s.pixels.len() - dst_off);
                    for i in 0..copy_len {
                        let base = i * 4;
                        s.pixels[dst_off + i] = u32::from_ne_bytes([
                            kbuf[base],
                            kbuf[base + 1],
                            kbuf[base + 2],
                            kbuf[base + 3],
                        ]);
                    }
                    written += copy_len;
                    if copy_len < chunk {
                        break;
                    }
                } else {
                    return -2; // -ENOENT
                }
            }
            written as i64
        }
        SYS_SURFACE_DESTROY => {
            let (caller_pid, caps) = match get_caller_identity_and_caps() {
                Ok(pair) => pair,
                Err(err) => return err,
            };
            if caps.check(CapabilityType::DisplayServer, Rights::WRITE).is_err() {
                return -13; // -EACCES
            }
            let is_admin = caps.check(CapabilityType::DisplayServer, Rights::ADMIN).is_ok();
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
            let service_id = a0 as u8;
            let addr = a1;
            let len = a2 as usize;
            if addr == 0 || len < 12 {
                return -22;
            }
            let mut buf = [0u8; 12];
            if !crate::services::query_record(service_id, &mut buf) {
                return -22;
            }
            if let Err(err) = copy_to_user(addr, &buf) {
                return err;
            }
            12
        }
        SYS_SERVICE_START => {
            let (_caller_pid, caps) = match get_caller_identity_and_caps() {
                Ok(pair) => pair,
                Err(err) => return err,
            };
            let is_supervisor = caps.check(CapabilityType::SystemResource, Rights::ADMIN).is_ok()
                || caps.check(CapabilityType::SystemResource, Rights::WRITE).is_ok();
            if !is_supervisor {
                return -13; // -EACCES: missing supervisor capability
            }
            let id = if a0 < 0x1000 && a1 > 0 { a1 as u8 } else { a0 as u8 };
            match crate::services::start(id) {
                Ok(pid) => pid as i64,
                Err(()) => -1,
            }
        }
        SYS_SERVICE_STOP => {
            let (_caller_pid, caps) = match get_caller_identity_and_caps() {
                Ok(pair) => pair,
                Err(err) => return err,
            };
            let is_supervisor = caps.check(CapabilityType::SystemResource, Rights::ADMIN).is_ok()
                || caps.check(CapabilityType::SystemResource, Rights::WRITE).is_ok();
            if !is_supervisor {
                return -13; // -EACCES: missing supervisor capability
            }
            let id = if a0 < 0x1000 && a1 > 0 { a1 as u8 } else { a0 as u8 };
            match crate::services::stop(id) {
                Ok(()) => 0,
                Err(()) => -1,
            }
        }
        SYS_SERVICE_RECOVER_FAILED => {
            let (_caller_pid, caps) = match get_caller_identity_and_caps() {
                Ok(pair) => pair,
                Err(err) => return err,
            };
            let is_supervisor = caps.check(CapabilityType::SystemResource, Rights::ADMIN).is_ok()
                || caps.check(CapabilityType::SystemResource, Rights::WRITE).is_ok();
            if !is_supervisor {
                return -13; // -EACCES: missing supervisor capability
            }
            let id = if a0 < 0x1000 && a1 > 0 { a1 as u8 } else { a0 as u8 };
            match crate::services::recover_failed(id) {
                Ok(()) => 0,
                Err(()) => -1,
            }
        }
        SYS_SERVICE_HEARTBEAT => {
            let (caller_pid, _caps) = match get_caller_identity_and_caps() {
                Ok(pair) => pair,
                Err(err) => return err,
            };
            let gen = if a1 != 0 { a1 as u32 } else { 1 };
            match crate::services::heartbeat(caller_pid, gen) {
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
            if let Err(err) = copy_to_user(addr, &s[..to_copy]) {
                return err;
            }
            to_copy as i64
        }
        SYS_APP_LAUNCH => {
            let (caller_pid, caller_caps) = match get_caller_identity_and_caps() {
                Ok(pair) => pair,
                Err(err) => return err,
            };
            if caller_caps.check(CapabilityType::SystemResource, Rights::WRITE).is_err()
                && caller_caps.check(CapabilityType::SystemResource, Rights::ADMIN).is_err()
            {
                return -13; // -EACCES: process launch requires SystemResource capability
            }
            let (name_ptr, name_len) = if a0 < 0x1000 && a2 > 0 { (a1, a2 as usize) } else { (a0, a1 as usize) };
            if name_len == 0 || name_len > 64 {
                return -22; // -EINVAL
            }
            let mut name_buf = [0u8; 64];
            if let Err(err) = copy_from_user(&mut name_buf, name_ptr, name_len) {
                return err;
            }
            let target_name = match core::str::from_utf8(&name_buf[..name_len]) {
                Ok(s) => s,
                Err(_) => return -22,
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
                    _ => None,
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
                        let mut caps = crate::capability::CapabilityTable::new();
                        caps.mint(crate::capability::CapabilityType::IpcSend, crate::capability::Rights::INVOKE, None);
                        caps.mint(crate::capability::CapabilityType::IpcReceive, crate::capability::Rights::READ, None);
                        caps.mint(crate::capability::CapabilityType::SerialOutput, crate::capability::Rights::WRITE, None);
                        caps.mint(crate::capability::CapabilityType::DisplayServer, crate::capability::Rights::WRITE, None);
                        caps.mint(crate::capability::CapabilityType::EventPoll, crate::capability::Rights::READ, None);
                        caps.mint(crate::capability::CapabilityType::InputDevice, crate::capability::Rights::READ, None);
                        caps.mint(crate::capability::CapabilityType::TimerTickRead, crate::capability::Rights::READ, None);
                        if user_img.name == "launcher" || user_img.name == "compositor" {
                            caps.mint(crate::capability::CapabilityType::SystemResource, crate::capability::Rights::WRITE, None);
                        }
                        if matches!(user_img.name, "browser" | "calendar" | "messages" | "vault") {
                            caps.mint(crate::capability::CapabilityType::CryptoEntropyRead, crate::capability::Rights::READ, None);
                        }
                        let tid = {
                            let mut sched = SCHEDULER.lock();
                            sched.spawn_user_process(caller_pid, user_img.name, loaded.entry_point, loaded.stack_top, Some(loaded.address_space), caps)
                        };
                        if tid > 0 {
                            let self_bit = 1u32 << crate::task::current_cpu_logical_id();
                            crate::gic::broadcast_sgi_to_mask(0, 0xFFu32 & !self_bit);
                        }
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
        SYS_APP_RESTART | SYS_APP_BACKGROUND | SYS_APP_FOREGROUND => -38,
        SYS_USER_GET_CURRENT => 1,
        SYS_HAL_QUERY_PLATFORM => {
            let (addr, len) = if a0 < 0x1000 && a2 > 0 { (a1, a2 as usize) } else { (a0, a1 as usize) };
            let s = b"aarch64-qemu-phone";
            let to_copy = s.len().min(len);
            if let Err(err) = copy_to_user(addr, &s[..to_copy]) {
                return err;
            }
            to_copy as i64
        }
        SYS_POWER_GOVERNOR | SYS_POWER_SET_PROFILE => -38,
        SYS_BATTERY_QUERY | SYS_THERMAL_QUERY | SYS_RTC_GET_TIME | SYS_TELEPHONY_QUERY => -38,
        SYS_SENSOR_READ => {
            let (addr, len) = if a0 < 0x1000 && a2 > 0 { (a1, a2 as usize) } else { (a0, a1 as usize) };
            let s = b"{\"present\":false,\"status\":\"NOT_IMPLEMENTED\",\"error\":\"No physical IMU or ambient sensor detected\"}";
            let to_copy = s.len().min(len);
            if let Err(err) = copy_to_user(addr, &s[..to_copy]) {
                return err;
            }
            to_copy as i64
        }
        SYS_GET_RANDOM => {
            let (_caller_pid, caps) = match get_caller_identity_and_caps() {
                Ok(pair) => pair,
                Err(err) => return err,
            };
            if caps.check(CapabilityType::CryptoEntropyRead, Rights::READ).is_err() {
                return -13; // -EACCES: missing CryptoEntropyRead
            }
            let (addr, len) = if a0 < 0x1000 && a2 > 0 { (a1, a2 as usize) } else { (a0, a1 as usize) };
            if len == 0 {
                return 0;
            }
            let to_fill = len.min(64);
            if let Err(err) = validate_user_range(addr, to_fill, true) {
                return err;
            }
            let mut kbuf = [0u8; 64];
            let res = if crate::crypto::try_get_random_bytes(&mut kbuf[..to_fill]).is_ok() {
                match copy_to_user(addr, &kbuf[..to_fill]) {
                    Ok(()) => to_fill as i64,
                    Err(err) => err,
                }
            } else {
                -1 // Fail closed: refusing to provide predictable material when entropy is unavailable
            };
            crate::crypto::zeroize_slice(&mut kbuf);
            res
        }
        SYS_CAP_REQUEST_CONSENT => {
            let (_caller_pid, caps) = match get_caller_identity_and_caps() {
                Ok(pair) => pair,
                Err(err) => return err,
            };
            if caps.check(CapabilityType::SystemResource, Rights::ADMIN).is_ok() {
                1
            } else {
                -13 // -EACCES: interactive consent broker required for unprivileged callers
            }
        }
        SYS_EVENT_POLL => {
            let (caller_pid, has_cap) = match get_caller_identity_and_caps() {
                Ok((pid, caps)) => {
                    let ok = caps.check(CapabilityType::EventPoll, Rights::READ).is_ok();
                    (pid, ok)
                }
                Err(err) => return err,
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
        108 => -38, // UserUnlock: -ENOSYS (fail-closed until full auth provider ported)
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
                38 => Some(CapabilityType::CryptoEntropyRead),
                _ => None,
            };
            let Some(ct) = cap_type else {
                return -22; // -EINVAL
            };
            let sched = SCHEDULER.lock();
            if let Some(curr_idx) = sched.current_task_pos() {
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
            let (caller_pid, is_supervisor) = match get_caller_identity_and_caps() {
                Ok((pid, caps)) => {
                    let supervisor = caps.check(CapabilityType::SystemResource, Rights::ADMIN).is_ok();
                    (pid, supervisor)
                }
                Err(err) => return err,
            };
            let code_res = crate::task::reap_task_locksafe(caller_pid, is_supervisor, target_pid);
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
        SYS_SETTINGS_GET | SYS_SETTINGS_SET | SYS_STORAGE_QUERY => -38,
        SYS_FS_OPEN | SYS_FS_STAT => -2, // -ENOENT
        SYS_FS_READ | SYS_FS_READ_AT => -38,
        SYS_FS_WRITE => {
            let (_caller_pid, caps) = match get_caller_identity_and_caps() {
                Ok(pair) => pair,
                Err(err) => return err,
            };
            if caps.check(CapabilityType::FileSystemWrite, Rights::WRITE).is_err() {
                return -13; // -EACCES
            }
            -38 // -ENOSYS: VFS write path not implemented on AArch64 yet
        }
        SYS_FS_CLOSE | SYS_FS_SYNC | SYS_FS_LIST => -38,
        SYS_BLOCK_READ => -13,
        SYS_VERIFY_PACKAGE => -38, // -ENOSYS: must not fabricate cryptographic verification success
        SYS_DEVFS_IO => -38,
        _ => -38, // -ENOSYS
    }
}
