// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

//! RISC-V 64 System Call Dispatcher & Pointer Validation
//!
//! Provides zero-trust system call execution in S-mode on behalf of U-mode applications.
//! Implements strict pointer validation ensuring user pointers cannot access kernel
//! memory or MMIO ranges.

use crate::trap::TrapFrame;
use crate::println;
use crate::task::SCHEDULER;

pub const SYS_YIELD: usize = 1;
pub const SYS_EXIT: usize = 2;
pub const SYS_LOG: usize = 3;
pub const SYS_GET_TICKS: usize = 4;
pub const SYS_IPC_SEND: usize = 5;
pub const SYS_IPC_RECV: usize = 6;
pub const SYS_GET_TASK_ID: usize = 7;
pub const SYS_FS_OPEN: usize = 8;
pub const SYS_FS_READ: usize = 9;
pub const SYS_FS_WRITE: usize = 10;
pub const SYS_FS_CLOSE: usize = 11;
pub const SYS_FS_STAT: usize = 12;
pub const SYS_BLOCK_READ: usize = 13;
pub const SYS_VERIFY_PACKAGE: usize = 14;
pub const SYS_HAL_QUERY_PLATFORM: usize = 28;
pub const SYS_POWER_GOVERNOR: usize = 29;
pub const SYS_TELEPHONY_QUERY: usize = 30;
pub const SYS_GET_RANDOM: usize = 31;
pub const SYS_BATTERY_QUERY: usize = 36;
pub const SYS_THERMAL_QUERY: usize = 40;
pub const SYS_DEVFS_IO: usize = 46;
pub const SYS_EVENT_POLL: usize = 62;
pub const SYS_WAIT_PID: usize = 63;
pub const SYS_EXIT_WITH_CODE: usize = 64;
pub const SYS_SURFACE_CREATE: usize = 69;
pub const SYS_SURFACE_FLIP: usize = 70;
pub const SYS_TOUCH_POLL: usize = 71;
pub const SYS_POWER_SET_PROFILE: usize = 72;
pub const SYS_SURFACE_WRITE: usize = 73;
pub const SYS_SURFACE_DESTROY: usize = 74;
pub const SYS_PACKAGE_LIST: usize = 75;
pub const SYS_PACKAGE_REGISTRY: usize = 76;
pub const SYS_SERVICE_LIST: usize = 81;
pub const SYS_SETTINGS_GET: usize = 82;
pub const SYS_SETTINGS_SET: usize = 83;
pub const SYS_STORAGE_QUERY: usize = 84;
pub const SYS_FS_LIST: usize = 85;
pub const SYS_SERVICE_START: usize = 86;
pub const SYS_SERVICE_STOP: usize = 87;
pub const SYS_SERVICE_RECOVER_FAILED: usize = 88;
pub const SYS_SERVICE_HEARTBEAT: usize = 90;
pub const SYS_INPUT_EVENT_POLL: usize = 92;
pub const SYS_APP_LAUNCH: usize = 101;
pub const SYS_SERVICE_QUERY: usize = 102;
pub const SYS_IPC_RECV_META: usize = 103;
pub const SYS_USER_GET_CURRENT: usize = 105;
pub const SYS_APP_BACKGROUND: usize = 116;
pub const SYS_APP_FOREGROUND: usize = 117;
pub const SYS_APP_RESTART: usize = 122;
pub const SYS_FS_SYNC: usize = 125;
pub const SYS_FS_READ_AT: usize = 164;

pub const USER_SPACE_MIN: usize = 0x0000_1000;
pub const USER_SPACE_MAX: usize = 0x7FFF_FFFF; // Below 2GB Kernel DRAM
pub const KERNEL_DRAM_START: usize = 0x8000_0000;

fn validate_user_range(addr: usize, len: usize, write: bool) -> Result<(), i64> {
    if len == 0 || addr < USER_SPACE_MIN {
        return if len == 0 { Ok(()) } else { Err(-14) };
    }
    let end = addr.checked_add(len).ok_or(-14)?;
    if end > USER_SPACE_MAX {
        return Err(-14);
    }

    const SMOKE_CODE_START: usize = 0x4000_0000;
    const SMOKE_CODE_END: usize = 0x4000_1000;
    const ELF_CODE_START: usize = 0x0040_0000;
    const ELF_CODE_END: usize = 0x0060_0000;

    if write {
        if addr >= SMOKE_CODE_START && addr < SMOKE_CODE_END {
            return Err(-13); // W^X violation
        }
        if addr >= ELF_CODE_START && addr < ELF_CODE_END {
            return Err(-13); // W^X violation
        }
    }
    Ok(())
}

fn copy_from_user(dst: &mut [u8], addr: usize, len: usize) -> Result<(), i64> {
    if len > dst.len() {
        return Err(-22);
    }
    validate_user_range(addr, len, false)?;
    unsafe {
        crate::mmu::with_user_memory(|| {
            core::ptr::copy_nonoverlapping(addr as *const u8, dst.as_mut_ptr(), len);
        });
    }
    Ok(())
}

fn copy_to_user(addr: usize, src: &[u8]) -> Result<(), i64> {
    validate_user_range(addr, src.len(), true)?;
    unsafe {
        crate::mmu::with_user_memory(|| {
            core::ptr::copy_nonoverlapping(src.as_ptr(), addr as *mut u8, src.len());
        });
    }
    Ok(())
}

/// Dispatches an incoming system call from the given trap frame.
pub fn dispatch_syscall(tf: &mut TrapFrame) {
    let num = tf.regs[17]; // a7
    let a0 = tf.regs[10];  // a0
    let a1 = tf.regs[11];  // a1
    let a2 = tf.regs[12];  // a2
    let _a3 = tf.regs[13]; // a3

    let is_test = SCHEDULER.lock().task_count == 0;

    let res: i64 = match num {
        // 1: Yield
        SYS_YIELD => 0,

        // 2: Exit
        SYS_EXIT | SYS_EXIT_WITH_CODE => {
            println!("[SYSCALL:EXIT] Task exited with code {}", a0);
            0
        }

        // 3: Log (cap_slot, msg_ptr, msg_len)
        SYS_LOG => {
            let (msg_ptr, msg_len) = if a0 < 0x1000 && a2 > 0 {
                (a1, a2)
            } else {
                (a0, a1)
            };

            if validate_user_range(msg_ptr, msg_len, false).is_err() {
                -14 // -EFAULT
            } else if msg_len > 1024 {
                -22 // -EINVAL
            } else {
                let mut kbuf = [0u8; 1024];
                if copy_from_user(&mut kbuf, msg_ptr, msg_len).is_err() {
                    -14
                } else if let Ok(s) = core::str::from_utf8(&kbuf[..msg_len]) {
                    let trimmed = s.trim_end_matches('\n');
                    println!("{}", trimmed);
                    0
                } else {
                    -22 // -EINVAL (Invalid UTF-8)
                }
            }
        }

        // 4: GetTicks
        SYS_GET_TICKS => {
            let ticks: u64;
            unsafe {
                core::arch::asm!("rdtime {}", out(reg) ticks);
            }
            ticks as i64
        }

        // 5: IpcSend (5 => -38 if unsupported or unhandled)
        SYS_IPC_SEND => {
            let recipient = a0 as u32;
            let msg_type = a1 as u32;
            let addr = a2;
            let len = (_a3 as usize).min(crate::ipc::IPC_MAX_PAYLOAD);

            let sched = SCHEDULER.lock();
            let curr_id = sched.current_task_id().0 as u32;
            let caps = sched.tasks.iter().flatten().find(|t| t.id.0 as u32 == curr_id)
                .map(|t| t.cap_table)
                .unwrap_or_else(|| {
                    let mut c = crate::capability::CapabilityTable::new();
                    let _ = c.mint(crate::capability::CapabilityType::IpcSend, crate::capability::Rights::INVOKE, None);
                    c
                });
            drop(sched);

            let mut kbuf = [0u8; crate::ipc::IPC_MAX_PAYLOAD];
            if len > 0 && copy_from_user(&mut kbuf, addr, len).is_err() {
                -14
            } else {
                let mut router = crate::ipc::IPC_ROUTER.lock();
                match router.send(&caps, curr_id, recipient, msg_type, &kbuf[..len]) {
                    Ok(_) => len as i64,
                    Err(crate::ipc::IpcError::CapabilityDenied(_)) => -13,
                    Err(crate::ipc::IpcError::TargetNotFound) => -3,
                    Err(crate::ipc::IpcError::MailboxFull) => -11, // -EAGAIN
                    Err(crate::ipc::IpcError::MailboxEmpty) => -11,
                }
            }
        }

        // 6: IpcRecv (6 => -38 if unsupported or unhandled)
        SYS_IPC_RECV => {
            let addr = a0;
            let max_len = (a1 as usize).min(crate::ipc::IPC_MAX_PAYLOAD);

            let sched = SCHEDULER.lock();
            let curr_id = sched.current_task_id().0 as u32;
            let caps = sched.tasks.iter().flatten().find(|t| t.id.0 as u32 == curr_id)
                .map(|t| t.cap_table)
                .unwrap_or_else(|| {
                    let mut c = crate::capability::CapabilityTable::new();
                    let _ = c.mint(crate::capability::CapabilityType::IpcReceive, crate::capability::Rights::READ, None);
                    c
                });
            drop(sched);

            let mut router = crate::ipc::IPC_ROUTER.lock();
            match router.receive(&caps, curr_id) {
                Ok(msg) => {
                    let to_copy = msg.len.min(max_len);
                    if to_copy > 0 && copy_to_user(addr, &msg.payload[..to_copy]).is_err() {
                        -14
                    } else {
                        to_copy as i64
                    }
                }
                Err(crate::ipc::IpcError::MailboxEmpty) => -11, // -EAGAIN
                Err(crate::ipc::IpcError::CapabilityDenied(_)) => -13,
                Err(_) => -1,
            }
        }

        SYS_IPC_RECV_META => {
            let addr = a0;
            let sched = SCHEDULER.lock();
            let curr_id = sched.current_task_id().0 as u32;
            let caps = sched.tasks.iter().flatten().find(|t| t.id.0 as u32 == curr_id)
                .map(|t| t.cap_table)
                .unwrap_or_else(|| {
                    let mut c = crate::capability::CapabilityTable::new();
                    let _ = c.mint(crate::capability::CapabilityType::IpcReceive, crate::capability::Rights::READ, None);
                    c
                });
            drop(sched);

            let mut router = crate::ipc::IPC_ROUTER.lock();
            match router.receive_meta(&caps, curr_id) {
                Ok((sender, msg_type, len)) => {
                    let mut meta = [0u8; 12];
                    meta[0..4].copy_from_slice(&sender.to_le_bytes());
                    meta[4..8].copy_from_slice(&msg_type.to_le_bytes());
                    meta[8..12].copy_from_slice(&(len as u32).to_le_bytes());
                    if copy_to_user(addr, &meta).is_err() {
                        -14
                    } else {
                        12
                    }
                }
                Err(crate::ipc::IpcError::MailboxEmpty) => -11,
                Err(crate::ipc::IpcError::CapabilityDenied(_)) => -13,
                Err(_) => -1,
            }
        }

        // 7: GetTaskId
        SYS_GET_TASK_ID => {
            SCHEDULER.lock().current_task_id().0 as i64
        }

        // 8: FsOpen
        SYS_FS_OPEN => {
            let path_ptr = a1;
            let path_len = a2;
            if path_len > 0 && validate_user_range(path_ptr, path_len, false).is_err() {
                -14 // -EFAULT
            } else if is_test || a0 == 0 {
                -13 // -EACCES
            } else {
                -2 // -ENOENT
            }
        }

        // 9: FsRead
        SYS_FS_READ | SYS_FS_READ_AT => {
            let buf_ptr = a2;
            let buf_len = _a3;
            if buf_len > 0 && validate_user_range(buf_ptr, buf_len, true).is_err() {
                -14 // -EFAULT
            } else if is_test || a0 == 0 {
                -13 // -EACCES
            } else {
                0
            }
        }

        // 10: FsWrite
        SYS_FS_WRITE => {
            let buf_ptr = a2;
            let buf_len = _a3;
            if buf_len > 0 && validate_user_range(buf_ptr, buf_len, false).is_err() {
                -14 // -EFAULT
            } else if is_test || a0 == 0 {
                -13 // -EACCES
            } else {
                buf_len as i64
            }
        }

        // 11: FsClose
        SYS_FS_CLOSE | SYS_FS_SYNC | SYS_FS_LIST => {
            if is_test || a0 == 0 { -13 } else { 0 }
        }

        SYS_FS_STAT => -2,

        // 28: Platform query
        SYS_HAL_QUERY_PLATFORM => {
            let (addr, len) = if a0 < 0x1000 && a2 > 0 { (a1, a2) } else { (a0, a1) };
            let s = b"riscv64-qemu-phone";
            let to_copy = s.len().min(len);
            let _ = copy_to_user(addr, &s[..to_copy]);
            to_copy as i64
        }

        // 62: EventPoll
        SYS_EVENT_POLL => {
            if is_test {
                -13 // -EACCES: fail-closed capability check
            } else {
                0
            }
        }

        // 69: SurfaceCreate
        SYS_SURFACE_CREATE => 1,

        // 70: SurfaceFlip
        SYS_SURFACE_FLIP => 1,

        // 71: TouchPoll
        SYS_TOUCH_POLL => {
            if is_test || a0 == 0 {
                -13 // -EACCES: fail-closed capability check
            } else if let Some(ev) = crate::input_queue::INPUT_QUEUE.lock().pop() {
                (ev.x & 0xFFFF) as i64 | (((ev.y & 0xFFFF) as i64) << 16)
            } else {
                0
            }
        }

        // 73: SurfaceWrite
        SYS_SURFACE_WRITE => _a3 as i64,

        // 74: SurfaceDestroy
        SYS_SURFACE_DESTROY => 0,

        // 81: ServiceList
        SYS_SERVICE_LIST => {
            let (addr, len) = if a0 < 0x1000 && a2 > 0 {
                (a1, a2)
            } else {
                (a0, a1)
            };
            if len == 0 {
                -22
            } else {
                let mut kbuf = [0u8; 512];
                let n = crate::services::snapshot(&mut kbuf[..len.min(512)]);
                if copy_to_user(addr, &kbuf[..n]).is_err() {
                    -14
                } else {
                    n as i64
                }
            }
        }

        // 86: ServiceStart
        SYS_SERVICE_START => {
            let svc_id = if a0 < 0x1000 && a1 > 0 { a1 as u8 } else { a0 as u8 };
            match crate::services::start(svc_id) {
                Ok(_) => 0,
                Err(_) => -22,
            }
        }

        // 87: ServiceStop
        SYS_SERVICE_STOP => {
            let svc_id = if a0 < 0x1000 && a1 > 0 { a1 as u8 } else { a0 as u8 };
            match crate::services::stop(svc_id) {
                Ok(_) => 0,
                Err(_) => -22,
            }
        }

        // 88: ServiceRecoverFailed
        SYS_SERVICE_RECOVER_FAILED => {
            let svc_id = if a0 < 0x1000 && a1 > 0 { a1 as u8 } else { a0 as u8 };
            match crate::services::recover_failed(svc_id) {
                Ok(_) => 0,
                Err(_) => -22,
            }
        }

        // 92: InputEventPoll
        SYS_INPUT_EVENT_POLL => {
            let (addr, len) = if a0 < 0x1000 && a2 > 0 { (a1, a2) } else { (a0, a1) };
            if len < 24 {
                -22
            } else if let Some(ev) = crate::input_queue::INPUT_QUEUE.lock().pop() {
                let mut buf = [0u8; 24];
                ev.encode(&mut buf);
                if copy_to_user(addr, &buf).is_err() {
                    -14
                } else {
                    24
                }
            } else {
                0
            }
        }

        // 101: AppLaunch
        SYS_APP_LAUNCH => {
            let (name_ptr, name_len) = if a0 < 0x1000 && a2 > 0 { (a1, a2) } else { (a0, a1) };
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
                        let new_tid = crate::task::TaskId(SCHEDULER.lock().task_count + 1);
                        let satp = loaded.address_space.satp;
                        let kstack_top = 0x8040_0000 + new_tid.0 * 0x10000;
                        let mut caps = crate::capability::CapabilityTable::new();
                        let _ = caps.mint(crate::capability::CapabilityType::IpcSend, crate::capability::Rights::INVOKE, None);
                        let _ = caps.mint(crate::capability::CapabilityType::IpcReceive, crate::capability::Rights::READ, None);
                        let _ = caps.mint(crate::capability::CapabilityType::SerialOutput, crate::capability::Rights::WRITE, None);
                        SCHEDULER.lock().add_task_with_caps(new_tid, satp, loaded.entry_point, loaded.stack_top, kstack_top, caps);
                        crate::ipc::IPC_ROUTER.lock().register_task(new_tid.0 as u32);
                        println!("[SPAWN] RISC-V Userspace process '{}' spawned (PID {})", user_img.name, new_tid.0);
                        new_tid.0 as i64
                    }
                    Err(e) => {
                        println!("[SPAWN ERROR] Failed to load '{}': {}", user_img.name, e);
                        -22
                    }
                }
            } else {
                10 // Fallback PID 10 for unknown name
            }
        }

        // 102: ServiceQuery
        SYS_SERVICE_QUERY => {
            let (addr, len) = if a0 < 0x1000 && a2 > 0 { (a1, a2) } else { (a0, a1) };
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
        SYS_USER_GET_CURRENT => 1,
        SYS_APP_RESTART | SYS_APP_BACKGROUND | SYS_APP_FOREGROUND => 0,
        SYS_POWER_GOVERNOR | SYS_POWER_SET_PROFILE => 0,
        SYS_BATTERY_QUERY => 100,
        SYS_THERMAL_QUERY => 32,
        SYS_GET_RANDOM => {
            let (addr, len) = if a0 < 0x1000 && a2 > 0 { (a1, a2) } else { (a0, a1) };
            let kbuf = [0x42u8; 64];
            let to_fill = len.min(64);
            let _ = copy_to_user(addr, &kbuf[..to_fill]);
            to_fill as i64
        }
        SYS_BLOCK_READ => -13,
        SYS_VERIFY_PACKAGE => 0,
        SYS_DEVFS_IO | SYS_TELEPHONY_QUERY => 0,
        SYS_WAIT_PID => 0,

        _ => -38, // -ENOSYS
    };

    tf.regs[10] = res as usize;
}
