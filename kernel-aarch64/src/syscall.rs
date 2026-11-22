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

/// Masks interrupts for the guard's lifetime. Used to keep a user-pointer
/// validation and its raw copy in one epoch of the address space: the task
/// cannot be descheduled between the two, so the aspace cannot be torn down
/// underneath the copy.
struct IrqMaskGuard {
    saved: u64,
}

impl IrqMaskGuard {
    fn new() -> Self {
        let saved: u64;
        unsafe {
            core::arch::asm!("mrs {0}, daif", out(reg) saved, options(nomem, nostack));
            core::arch::asm!("msr daifset, #2", options(nomem, nostack));
        }
        Self { saved }
    }
}

impl Drop for IrqMaskGuard {
    fn drop(&mut self) {
        unsafe {
            core::arch::asm!("msr daif, {0}", in(reg) self.saved, options(nomem, nostack));
        }
    }
}

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
/// Cross-architecture MMU memory lending/transfer syscall numbers.
/// Hardware MMU page table lending is currently implemented for x86_64;
/// returns -38 (-ENOSYS) on AArch64 until stage 2 ARM64 hardware bring-up.
pub const SYS_MEM_LEND: u64 = 173;
pub const SYS_MEM_RETURN: u64 = 174;
pub const SYS_MEM_TRANSFER: u64 = 175;

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
    // Validation and the copy must observe one epoch of the address space.
    // Preemption is pinned across both: a task descheduled in between loses
    // its "active on this cpu" lifecycle bit, so a concurrent destroy could
    // detach and recycle the page tables this raw copy is about to walk.
    let _no_preempt = IrqMaskGuard::new();
    validate_user_range(addr, len, false)?;
    if len > 0 {
        unsafe {
            core::ptr::copy_nonoverlapping(addr as *const u8, dst.as_mut_ptr(), len);
        }
    }
    Ok(())
}

fn copy_to_user(addr: u64, src: &[u8]) -> Result<(), i64> {
    let _no_preempt = IrqMaskGuard::new();
    validate_user_range(addr, src.len(), true)?;
    if !src.is_empty() {
        unsafe {
            core::ptr::copy_nonoverlapping(src.as_ptr(), addr as *mut u8, src.len());
        }
    }
    Ok(())
}

fn read_user_string<'a>(addr: u64, len: usize, buf: &'a mut [u8]) -> Result<&'a str, i64> {
    if len > buf.len() {
        return Err(-22);
    }
    copy_from_user(&mut buf[..len], addr, len)?;
    core::str::from_utf8(&buf[..len]).map_err(|_| -22)
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
        SYS_USER_GET_CURRENT => 1,
        107 => 0, // UserLockState: 0 = Unlocked
        108 => 0, // UserUnlock: 0 = Success
        109 => 0, // UserLock: 0 = Success
        129 => 0, // UserHasCredential: 0 = false (no PIN required by default)
        132 => { // CapLookup: a1 = cap_type, a2 = object_id
            let (cap_type_raw, object_id) = if a0 < 0x1000 && a1 > 0 { (a1, a2) } else { (a0, a1) };
            let Some(ct) = CapabilityType::from_raw(cap_type_raw) else {
                return -22; // -EINVAL
            };
            let sched = SCHEDULER.lock();
            if let Some(curr_idx) = sched.current_task_pos() {
                let task = &sched.tasks[curr_idx];
                for (slot, cap) in task.cap_table.entries.iter().enumerate() {
                    if cap.cap_type == ct {
                        if object_id == 0 || cap.target_id.map(|t| t as u64) == Some(object_id) {
                            return slot as i64;
                        }
                    }
                }
            }
            -2 // -ENOENT
        }
        150 => { // CapHandleLookup: a1 = cap_type, a2 = object_id
            let (cap_type_raw, object_id) = if a0 < 0x1000 && a1 > 0 { (a1, a2) } else { (a0, a1) };
            let Some(ct) = CapabilityType::from_raw(cap_type_raw) else {
                return -22;
            };
            let sched = SCHEDULER.lock();
            if let Some(curr_idx) = sched.current_task_pos() {
                let task = &sched.tasks[curr_idx];
                for (slot, cap) in task.cap_table.entries.iter().enumerate() {
                    if cap.cap_type == ct {
                        if object_id == 0 || cap.target_id.map(|t| t as u64) == Some(object_id) {
                            let handle = (slot as u64) | ((cap.id.0 as u64) << 32);
                            return handle as i64;
                        }
                    }
                }
            }
            -2 // -ENOENT
        }
        SYS_SETTINGS_GET => {
            let (_cap_slot, key_ptr, key_len, out_len, out_ptr) = if a0 < 0x1000 {
                (a0, a1, a2 as usize, a3 as usize, a4)
            } else {
                (0, a0, a1 as usize, a2 as usize, a3)
            };
            if key_len == 0 || key_len > 96 || out_len > 512 {
                return -22; // -EINVAL
            }
            let mut key_buf = [0u8; 96];
            if copy_from_user(&mut key_buf[..key_len], key_ptr, key_len).is_err() {
                return -14; // -EFAULT
            }
            let Ok(key) = core::str::from_utf8(&key_buf[..key_len]) else {
                return -22;
            };
            let mut out = [0u8; 512];
            let mut store = crate::settings::SETTINGS.lock();
            store.init_defaults();
            let n = store.get(key, &mut out[..out_len]);
            if n > 0 {
                if copy_to_user(out_ptr, &out[..n]).is_err() {
                    return -14;
                }
            }
            n as i64
        }
        SYS_SETTINGS_SET => {
            let (_cap_slot, key_ptr, key_len, val_ptr, val_len) = if a0 < 0x1000 {
                (a0, a1, a2 as usize, a3, a4 as usize)
            } else {
                (0, a0, a1 as usize, a2, a3 as usize)
            };
            if key_len == 0 || key_len > 96 || val_len > 512 {
                return -22;
            }
            let mut key_buf = [0u8; 96];
            let mut val_buf = [0u8; 512];
            if copy_from_user(&mut key_buf[..key_len], key_ptr, key_len).is_err() {
                return -14;
            }
            if copy_from_user(&mut val_buf[..val_len], val_ptr, val_len).is_err() {
                return -14;
            }
            let Ok(key) = core::str::from_utf8(&key_buf[..key_len]) else {
                return -22;
            };
            let mut store = crate::settings::SETTINGS.lock();
            store.init_defaults();
            if store.set(key, &val_buf[..val_len]).is_err() {
                return -28; // -ENOSPC
            }
            0
        }
        SYS_STORAGE_QUERY => {
            64 * 1024 * 1024 // 64 MiB available storage
        }
        SYS_APP_BACKGROUND | SYS_APP_FOREGROUND | SYS_APP_RESTART | 121 => 0,
        146 => { // AudioStreamOpen
            let (_caller_pid, caps) = match get_caller_identity_and_caps() {
                Ok(pair) => pair,
                Err(err) => return err,
            };
            if caps.check(CapabilityType::AudioStreamDirect, Rights::WRITE).is_err() {
                return -13; // -EACCES
            }
            let mut driver = crate::audio::AUDIO_DRIVER.lock();
            driver.init_hardware(crate::audio::SampleRate::Rate48kHz, crate::audio::ChannelMode::Stereo);
            driver.set_hardware_present(true);
            1 // Stream ID 1
        }
        147 => { // AudioStreamWrite: a1 = stream_handle, a2 = pcm_ptr, a3 = byte_len
            let (_caller_pid, caps) = match get_caller_identity_and_caps() {
                Ok(pair) => pair,
                Err(err) => return err,
            };
            let (pcm_ptr, byte_len) = if a0 < 0x1000 { (a1, a2 as usize) } else { (a0, a1 as usize) };
            if byte_len == 0 || (byte_len % 2) != 0 {
                return -22;
            }
            let sample_count = (byte_len / 2).min(crate::audio::PCM_BUFFER_CAPACITY);
            let mut kbuf = [0i16; crate::audio::PCM_BUFFER_CAPACITY];
            let byte_count = sample_count * 2;
            let mut raw_bytes = [0u8; crate::audio::PCM_BUFFER_CAPACITY * 2];
            if copy_from_user(&mut raw_bytes[..byte_count], pcm_ptr, byte_count).is_err() {
                return -14;
            }
            for i in 0..sample_count {
                kbuf[i] = i16::from_le_bytes([raw_bytes[i * 2], raw_bytes[i * 2 + 1]]);
            }
            let mut driver = crate::audio::AUDIO_DRIVER.lock();
            match driver.stream_pcm(&kbuf[..sample_count], crate::audio::AudioStreamType::BackgroundMediaMusic, &caps) {
                Ok(written_samples) => (written_samples * 2) as i64,
                Err(_) => -5, // -EIO
            }
        }
        148 => { // AudioStreamStatus: a1 = stream_handle, a2 = out_ptr, a3 = out_len
            let (out_ptr, out_len) = if a0 < 0x1000 { (a1, a2 as usize) } else { (a0, a1 as usize) };
            if out_len < 40 {
                return -22;
            }
            let driver = crate::audio::AUDIO_DRIVER.lock();
            let mut status = [0u8; 40];
            status[0..8].copy_from_slice(&driver.samples_rendered.to_le_bytes());
            status[8..16].copy_from_slice(&(driver.ring_buffer.len() as u64).to_le_bytes());
            status[16] = if driver.enabled { 1 } else { 0 };
            status[17] = driver.gain_percent;
            if copy_to_user(out_ptr, &status).is_err() {
                return -14;
            }
            40
        }
        149 => { // AudioStreamClose
            let mut driver = crate::audio::AUDIO_DRIVER.lock();
            driver.set_hardware_present(false);
            0
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
        SYS_FS_OPEN => {
            let (path_ptr, path_len, rights) = if a0 < 0x1000 && a2 > 0 {
                (a1, a2 as usize, a3 as u32)
            } else {
                (a0, a1 as usize, a2 as u32)
            };
            if path_len == 0 || path_len > 256 { return -22; }
            let mut pbuf = [0u8; 256];
            let path = match read_user_string(path_ptr, path_len, &mut pbuf) {
                Ok(s) => s,
                Err(e) => return e,
            };
            let (caller_pid, caps) = match get_caller_identity_and_caps() {
                Ok(p) => p,
                Err(e) => return e,
            };
            match crate::vfs::GLOBAL_VFS.lock().open(caller_pid, path, rights, &caps) {
                Ok(fd) => fd as i64,
                Err(e) => e,
            }
        }
        SYS_FS_READ => {
            let (fd, buf_ptr, buf_len) = if a0 < 0x1000 && a2 > 0 {
                (a1 as u32, a2, a3 as usize)
            } else {
                (a0 as u32, a1, a2 as usize)
            };
            if buf_len == 0 { return 0; }
            if let Err(e) = validate_user_range(buf_ptr, buf_len, true) { return e; }
            let (caller_pid, caps) = match get_caller_identity_and_caps() {
                Ok(p) => p,
                Err(e) => return e,
            };
            let mut kbuf = alloc::vec![0u8; buf_len.min(65536)];
            let n = match crate::vfs::GLOBAL_VFS.lock().read(caller_pid, fd, &mut kbuf, &caps) {
                Ok(n) => n,
                Err(e) => return e,
            };
            if n > 0 {
                if let Err(e) = copy_to_user(buf_ptr, &kbuf[..n]) { return e; }
            }
            n as i64
        }
        SYS_FS_READ_AT => {
            let (fd, buf_ptr, buf_len, offset) = if a0 < 0x1000 && a2 > 0 {
                (a1 as u32, a2, a3 as usize, a4 as usize)
            } else {
                (a0 as u32, a1, a2 as usize, a3 as usize)
            };
            if buf_len == 0 { return 0; }
            if let Err(e) = validate_user_range(buf_ptr, buf_len, true) { return e; }
            let (caller_pid, caps) = match get_caller_identity_and_caps() {
                Ok(p) => p,
                Err(e) => return e,
            };
            let mut kbuf = alloc::vec![0u8; buf_len.min(65536)];
            let n = match crate::vfs::GLOBAL_VFS.lock().read_at(caller_pid, fd, offset, &mut kbuf, &caps) {
                Ok(n) => n,
                Err(e) => return e,
            };
            if n > 0 {
                if let Err(e) = copy_to_user(buf_ptr, &kbuf[..n]) { return e; }
            }
            n as i64
        }
        SYS_FS_WRITE => {
            let (fd, buf_ptr, buf_len) = if a0 < 0x1000 && a2 > 0 {
                (a1 as u32, a2, a3 as usize)
            } else {
                (a0 as u32, a1, a2 as usize)
            };
            if buf_len == 0 { return 0; }
            if buf_len > 1024 * 1024 { return -22; }
            let (caller_pid, caps) = match get_caller_identity_and_caps() {
                Ok(p) => p,
                Err(e) => return e,
            };
            let mut kbuf = alloc::vec![0u8; buf_len];
            if let Err(e) = copy_from_user(&mut kbuf, buf_ptr, buf_len) { return e; }
            match crate::vfs::GLOBAL_VFS.lock().write(caller_pid, fd, &kbuf, &caps) {
                Ok(n) => n as i64,
                Err(e) => e,
            }
        }
        SYS_FS_CLOSE => {
            let fd = if a0 < 0x1000 && a1 > 0 { a1 as u32 } else { a0 as u32 };
            let (caller_pid, _) = match get_caller_identity_and_caps() {
                Ok(p) => p,
                Err(e) => return e,
            };
            match crate::vfs::GLOBAL_VFS.lock().close(caller_pid, fd) {
                Ok(()) => 0,
                Err(e) => e,
            }
        }
        SYS_FS_STAT => {
            let (path_ptr, path_len, out_ptr, out_len) = if a0 < 0x1000 && a2 > 0 {
                (a1, a2 as usize, a3, a4 as usize)
            } else {
                (a0, a1 as usize, a2, a3 as usize)
            };
            if path_len == 0 || path_len > 256 || out_len < 96 { return -22; }
            let mut pbuf = [0u8; 256];
            let path = match read_user_string(path_ptr, path_len, &mut pbuf) {
                Ok(s) => s,
                Err(e) => return e,
            };
            let (_caller_pid, caps) = match get_caller_identity_and_caps() {
                Ok(p) => p,
                Err(e) => return e,
            };
            let mut stat_buf = [0u8; 96];
            match crate::vfs::GLOBAL_VFS.lock().stat(path, &mut stat_buf, &caps) {
                Ok(n) => {
                    if let Err(e) = copy_to_user(out_ptr, &stat_buf[..n]) { return e; }
                    n as i64
                }
                Err(e) => e,
            }
        }
        SYS_FS_LIST => {
            let (path_ptr, path_len, out_ptr, out_len) = if a0 < 0x1000 && a2 > 0 {
                (a1, a2 as usize, a3, a4 as usize)
            } else {
                (a0, a1 as usize, a2, a3 as usize)
            };
            if path_len == 0 || path_len > 256 || out_len == 0 { return -22; }
            let mut pbuf = [0u8; 256];
            let path = match read_user_string(path_ptr, path_len, &mut pbuf) {
                Ok(s) => s,
                Err(e) => return e,
            };
            let (_caller_pid, caps) = match get_caller_identity_and_caps() {
                Ok(p) => p,
                Err(e) => return e,
            };
            let mut list_buf = alloc::vec![0u8; out_len.min(4096)];
            match crate::vfs::GLOBAL_VFS.lock().list(path, &mut list_buf, &caps) {
                Ok(n) => {
                    if n > 0 {
                        if let Err(e) = copy_to_user(out_ptr, &list_buf[..n]) { return e; }
                    }
                    n as i64
                }
                Err(e) => e,
            }
        }
        SYS_FS_SYNC => {
            match crate::vfs::GLOBAL_VFS.lock().sync() {
                Ok(()) => 0,
                Err(e) => e,
            }
        }
        SYS_BLOCK_READ => -13,
        SYS_VERIFY_PACKAGE => {
            let (path_ptr, path_len, out_ptr, out_len) = if a0 < 0x1000 && a2 > 0 {
                (a1, a2 as usize, a3, a4 as usize)
            } else {
                (a0, a1 as usize, a2, a3 as usize)
            };
            if path_len == 0 || path_len > 256 || out_len == 0 { return -22; }
            let mut pbuf = [0u8; 256];
            let path = match read_user_string(path_ptr, path_len, &mut pbuf) {
                Ok(s) => s,
                Err(e) => return e,
            };
            let (_, caps) = match get_caller_identity_and_caps() {
                Ok(p) => p,
                Err(e) => return e,
            };
            if caps.check(CapabilityType::FileSystemRead, Rights::READ).is_err() {
                return -13; // -EACCES
            }
            let pkg_bytes = match crate::vfs::GLOBAL_VFS.lock().get_file_content(path) {
                Some(b) => b,
                None => return -2, // -ENOENT
            };
            let summary = if pkg_bytes.starts_with(b"RAPP") && pkg_bytes.len() >= 8 {
                let rest = &pkg_bytes[8..];
                let mut parts = rest.split(|&b| b == 0);
                let pkg_id = parts.next().and_then(|s| core::str::from_utf8(s).ok()).unwrap_or("unknown");
                let ver = parts.next().and_then(|s| core::str::from_utf8(s).ok()).unwrap_or("1.0.0");
                alloc::format!("{}|{}|{}\n", pkg_id, ver, pkg_bytes.len())
            } else {
                alloc::format!("package|1.0.0|{}\n", pkg_bytes.len())
            };
            let bytes = summary.as_bytes();
            let to_copy = bytes.len().min(out_len);
            if let Err(e) = copy_to_user(out_ptr, &bytes[..to_copy]) { return e; }
            to_copy as i64
        }
        SYS_DEVFS_IO => {
            let action = a0 as u32;
            let path_ptr = a1;
            let path_len = a2 as usize;
            let buf_ptr = a3;
            let buf_len = a4 as usize;
            if path_len == 0 || path_len > 256 { return -22; }
            let mut pbuf = [0u8; 256];
            let path = match read_user_string(path_ptr, path_len, &mut pbuf) {
                Ok(s) => s,
                Err(e) => return e,
            };
            let (caller_pid, caps) = match get_caller_identity_and_caps() {
                Ok(p) => p,
                Err(e) => return e,
            };
            if action == 0 {
                // Read: requires FileSystemRead or FileSystemWrite(READ)
                if caps.check(CapabilityType::FileSystemRead, Rights::READ).is_err()
                    && caps.check(CapabilityType::FileSystemWrite, Rights::READ).is_err()
                {
                    return -13; // -EACCES
                }
                match path {
                    "/dev/null" => 0,
                    "/dev/zero" => {
                        if buf_len > 0 {
                            let zeros = [0u8; 512];
                            let mut written = 0;
                            while written < buf_len {
                                let chunk = (buf_len - written).min(512);
                                if let Err(e) = copy_to_user(buf_ptr + written as u64, &zeros[..chunk]) {
                                    return e;
                                }
                                written += chunk;
                            }
                        }
                        buf_len as i64
                    }
                    "/dev/urandom" => {
                        let mut kbuf = alloc::vec![0u8; buf_len.min(4096)];
                        let _ = crate::crypto::try_get_random_bytes(&mut kbuf);
                        if let Err(e) = copy_to_user(buf_ptr, &kbuf) { return e; }
                        kbuf.len() as i64
                    }
                    "/dev/input/event0" => {
                        if let Some(ev) = crate::input_queue::INPUT_QUEUE.lock().pop_for_task(caller_pid) {
                            let mut kbuf = [0u8; 24];
                            ev.encode(&mut kbuf);
                            let to_copy = 24.min(buf_len);
                            if let Err(e) = copy_to_user(buf_ptr, &kbuf[..to_copy]) { return e; }
                            to_copy as i64
                        } else {
                            0
                        }
                    }
                    "/dev/ptmx" => 0,
                    _ => -2, // -ENOENT
                }
            } else {
                // Write: requires FileSystemWrite(WRITE)
                if caps.check(CapabilityType::FileSystemWrite, Rights::WRITE).is_err() {
                    return -13; // -EACCES
                }
                match path {
                    "/dev/null" | "/dev/zero" => buf_len as i64,
                    "/dev/urandom" => {
                        let chunk_len = buf_len.min(64);
                        let mut kbuf = [0u8; 64];
                        if copy_from_user(&mut kbuf[..chunk_len], buf_ptr, chunk_len).is_ok() {
                            let mut entropy = 0u64;
                            for (i, &b) in kbuf.iter().take(8).enumerate() {
                                entropy |= (b as u64) << (i * 8);
                            }
                            crate::crypto::TRNG.lock().mix_entropy(entropy);
                        }
                        buf_len as i64
                    }
                    "/dev/input/event0" => -13, // -EACCES
                    "/dev/ptmx" => {
                        let mut kbuf = [0u8; 256];
                        let mut read_pos = 0;
                        while read_pos < buf_len {
                            let chunk = (buf_len - read_pos).min(256);
                            if copy_from_user(&mut kbuf[..chunk], buf_ptr + read_pos as u64, chunk).is_err() {
                                break;
                            }
                            for &b in &kbuf[..chunk] {
                                crate::kprint!("{}", b as char);
                            }
                            read_pos += chunk;
                        }
                        buf_len as i64
                    }
                    _ => -2, // -ENOENT
                }
            }
        }
        SYS_MEM_LEND => {
            let borrower_pid = a0 as u32;
            let lender_vaddr = a1;
            let len = a2;
            let rights_num = a3;
            let out_borrower_vaddr_ptr = a4;

            if len == 0 || len > 64 * 1024 * 1024 || (len % 4096) != 0 || (lender_vaddr % 4096) != 0 || lender_vaddr < 0x1000 {
                return -22;
            }
            let rights = match rights_num {
                0 => crate::mmu::MemoryLoanRights::ReadOnly,
                1 => crate::mmu::MemoryLoanRights::ReadWrite,
                _ => return -22,
            };
            let page_count = (len / 4096) as usize;

            let mut sched = SCHEDULER.lock();
            let current_pos = match sched.current_task_pos() {
                Some(p) => p,
                None => return -13,
            };
            let caller_pid = sched.tasks[current_pos].id;
            if !sched.tasks[current_pos].cap_table.authorize_capability(CapabilityType::MemoryHeap, Rights::READ | Rights::WRITE)
                && !sched.tasks[current_pos].cap_table.authorize_capability(CapabilityType::MemoryHeap, Rights::WRITE)
            {
                return -13;
            }
            let has_ipc = sched.tasks[current_pos].cap_table.authorize_object(CapabilityType::IpcSend, Rights::WRITE, borrower_pid as u64)
                || sched.tasks[current_pos].cap_table.authorize_capability(CapabilityType::IpcSend, Rights::WRITE);
            if !has_ipc {
                return -13;
            }

            let borrower_pos = match sched.tasks.iter().position(|t| t.id == borrower_pid) {
                Some(p) => p,
                None => return -22,
            };

            if current_pos == borrower_pos {
                return -22;
            }

            if out_borrower_vaddr_ptr != 0 {
                if let Some(ref aspace) = sched.tasks[current_pos].address_space {
                    if aspace.validate_user_range(out_borrower_vaddr_ptr, 8, true).is_err() {
                        return -22;
                    }
                }
            }

            let (slot, gen) = match crate::mmu::MEMORY_LOANS.lock().reserve_slot() {
                Ok(pair) => pair,
                Err(_) => return -12,
            };

            let mut frames = alloc::vec::Vec::with_capacity(page_count);
            let mut orig_writable = alloc::vec::Vec::with_capacity(page_count);
            {
                let lender_aspace = match sched.tasks[current_pos].address_space.as_ref() {
                    Some(a) => a,
                    None => {
                        crate::mmu::MEMORY_LOANS.lock().release_slot(slot);
                        return -13;
                    }
                };
                for i in 0..page_count {
                    let vaddr = lender_vaddr + (i as u64 * 4096);
                    let (phys, is_write) = match lender_aspace.translate(vaddr) {
                        Some(pair) => pair,
                        None => {
                            crate::mmu::MEMORY_LOANS.lock().release_slot(slot);
                            return -22;
                        }
                    };
                    if rights == crate::mmu::MemoryLoanRights::ReadWrite && !is_write {
                        crate::mmu::MEMORY_LOANS.lock().release_slot(slot);
                        return -13;
                    }
                    if crate::mmu::MEMORY_LOANS.lock().is_frame_lent(phys) {
                        crate::mmu::MEMORY_LOANS.lock().release_slot(slot);
                        return -13;
                    }
                    frames.push(phys);
                    orig_writable.push(is_write);
                }
            }

            let borrower_vaddr = {
                let borrower_aspace = match sched.tasks[borrower_pos].address_space.as_mut() {
                    Some(a) => a,
                    None => {
                        crate::mmu::MEMORY_LOANS.lock().release_slot(slot);
                        return -22;
                    }
                };
                let b_span = match borrower_aspace.find_free_user_span(page_count) {
                    Ok(s) => s,
                    Err(_) => {
                        crate::mmu::MEMORY_LOANS.lock().release_slot(slot);
                        return -12;
                    }
                };
                let mut mapped = 0;
                for i in 0..page_count {
                    let vaddr = b_span + (i as u64 * 4096);
                    if borrower_aspace.map_borrowed_frame(vaddr, frames[i], rights == crate::mmu::MemoryLoanRights::ReadWrite, false).is_err() {
                        for r in 0..mapped {
                            let _ = borrower_aspace.unmap_user_page(b_span + (r as u64 * 4096), false);
                        }
                        crate::mmu::MEMORY_LOANS.lock().release_slot(slot);
                        return -12;
                    }
                    mapped += 1;
                }
                b_span
            };

            {
                let lender_aspace = sched.tasks[current_pos].address_space.as_mut().unwrap();
                match rights {
                    crate::mmu::MemoryLoanRights::ReadOnly => {
                        for i in 0..page_count {
                            let vaddr = lender_vaddr + (i as u64 * 4096);
                            let _ = lender_aspace.update_leaf_flags(vaddr, false);
                        }
                    }
                    crate::mmu::MemoryLoanRights::ReadWrite => {
                        for i in 0..page_count {
                            let vaddr = lender_vaddr + (i as u64 * 4096);
                            let _ = lender_aspace.clear_leaf_pte(vaddr);
                        }
                    }
                }
            }

            for &f in &frames {
                let _ = crate::mmu::set_frame_ownership(f, crate::mmu::FrameOwnership::Borrowed);
            }

            let handle = match crate::mmu::MEMORY_LOANS.lock().activate_loan(
                slot,
                gen,
                caller_pid,
                borrower_pid,
                lender_vaddr,
                borrower_vaddr,
                page_count,
                frames,
                rights,
                orig_writable,
            ) {
                Ok(h) => h,
                Err(_) => return -12,
            };

            drop(sched);

            if out_borrower_vaddr_ptr != 0 {
                let _ = copy_to_user(out_borrower_vaddr_ptr, &borrower_vaddr.to_le_bytes());
            }

            handle.0 as i64
        }
        SYS_MEM_RETURN => {
            let lend_handle = a0;
            let (caller_pid, _) = match get_caller_identity_and_caps() {
                Ok(p) => p,
                Err(e) => return e,
            };
            let rec = match crate::mmu::MEMORY_LOANS.lock().claim_return_or_revoke(crate::mmu::LendHandle(lend_handle), caller_pid) {
                Ok(r) => r,
                Err(_) => return -13,
            };

            let mut sched = SCHEDULER.lock();
            if let Some(b_task) = sched.tasks.iter_mut().find(|t| t.id == rec.borrower_task) {
                if let Some(ref mut b_aspace) = b_task.address_space {
                    for i in 0..rec.page_count {
                        let vaddr = rec.borrower_vaddr + (i as u64 * 4096);
                        let _ = b_aspace.unmap_user_page(vaddr, false);
                    }
                }
            }
            if let Some(l_task) = sched.tasks.iter_mut().find(|t| t.id == rec.lender_task) {
                if let Some(ref mut l_aspace) = l_task.address_space {
                    match rec.rights {
                        crate::mmu::MemoryLoanRights::ReadOnly => {
                            for i in 0..rec.page_count {
                                let vaddr = rec.lender_vaddr + (i as u64 * 4096);
                                let _ = l_aspace.update_leaf_flags(vaddr, rec.original_lender_writable[i]);
                            }
                        }
                        crate::mmu::MemoryLoanRights::ReadWrite => {
                            for i in 0..rec.page_count {
                                let vaddr = rec.lender_vaddr + (i as u64 * 4096);
                                let _ = l_aspace.restore_leaf_pte(vaddr, rec.frames[i], rec.original_lender_writable[i], false);
                            }
                        }
                    }
                }
            }
            drop(sched);

            for &f in &rec.frames {
                let _ = crate::mmu::set_frame_ownership(f, crate::mmu::FrameOwnership::PrivateOwned);
            }

            let handle = crate::mmu::LendHandle(lend_handle);
            crate::mmu::MEMORY_LOANS.lock().complete_return_or_revoke(handle.slot(), handle.generation());
            0
        }
        SYS_MEM_TRANSFER => {
            let receiver_pid = a0 as u32;
            let sender_vaddr = a1;
            let len = a2;
            let out_receiver_vaddr_ptr = a3;

            if len == 0 || len > 64 * 1024 * 1024 || (len % 4096) != 0 || (sender_vaddr % 4096) != 0 || sender_vaddr < 0x1000 {
                return -22;
            }
            let page_count = (len / 4096) as usize;

            let mut sched = SCHEDULER.lock();
            let current_pos = match sched.current_task_pos() {
                Some(p) => p,
                None => return -13,
            };
            let _caller_pid = sched.tasks[current_pos].id;
            if !sched.tasks[current_pos].cap_table.authorize_capability(CapabilityType::MemoryHeap, Rights::READ | Rights::WRITE)
                && !sched.tasks[current_pos].cap_table.authorize_capability(CapabilityType::MemoryHeap, Rights::WRITE)
            {
                return -13;
            }
            let has_ipc = sched.tasks[current_pos].cap_table.authorize_object(CapabilityType::IpcSend, Rights::WRITE, receiver_pid as u64)
                || sched.tasks[current_pos].cap_table.authorize_capability(CapabilityType::IpcSend, Rights::WRITE);
            if !has_ipc {
                return -13;
            }

            let receiver_pos = match sched.tasks.iter().position(|t| t.id == receiver_pid) {
                Some(p) => p,
                None => return -22,
            };
            if current_pos == receiver_pos {
                return -22;
            }

            if out_receiver_vaddr_ptr != 0 {
                if let Some(ref aspace) = sched.tasks[current_pos].address_space {
                    if aspace.validate_user_range(out_receiver_vaddr_ptr, 8, true).is_err() {
                        return -22;
                    }
                }
            }

            let mut frames = alloc::vec::Vec::with_capacity(page_count);
            {
                let sender_aspace = match sched.tasks[current_pos].address_space.as_ref() {
                    Some(a) => a,
                    None => return -13,
                };
                for i in 0..page_count {
                    let vaddr = sender_vaddr + (i as u64 * 4096);
                    let (phys, is_write) = match sender_aspace.translate(vaddr) {
                        Some(p) => p,
                        None => return -22,
                    };
                    if !is_write { return -13; }
                    if crate::mmu::MEMORY_LOANS.lock().is_frame_lent(phys) { return -13; }
                    frames.push(phys);
                }
            }

            let receiver_vaddr = {
                let receiver_aspace = match sched.tasks[receiver_pos].address_space.as_mut() {
                    Some(a) => a,
                    None => return -22,
                };
                let r_span = match receiver_aspace.find_free_user_span(page_count) {
                    Ok(s) => s,
                    Err(_) => return -12,
                };
                let mut mapped = 0;
                for i in 0..page_count {
                    let vaddr = r_span + (i as u64 * 4096);
                    if receiver_aspace.restore_leaf_pte(vaddr, frames[i], true, false).is_err() {
                        for r in 0..mapped {
                            let _ = receiver_aspace.clear_leaf_pte(r_span + (r as u64 * 4096));
                        }
                        return -12;
                    }
                    mapped += 1;
                }
                r_span
            };

            {
                let sender_aspace = sched.tasks[current_pos].address_space.as_mut().unwrap();
                for i in 0..page_count {
                    let vaddr = sender_vaddr + (i as u64 * 4096);
                    let _ = sender_aspace.clear_leaf_pte(vaddr);
                }
            }

            drop(sched);

            if out_receiver_vaddr_ptr != 0 {
                let _ = copy_to_user(out_receiver_vaddr_ptr, &receiver_vaddr.to_le_bytes());
            }

            receiver_vaddr as i64
        }
        _ => -38, // -ENOSYS
    }
}
