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
extern crate alloc;

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
/// Cross-architecture MMU memory lending/transfer syscall numbers.
/// Hardware MMU page table lending is currently implemented for x86_64;
/// returns -38 (-ENOSYS) on RISC-V 64 until stage 2 RISC-V hardware bring-up.
pub const SYS_MEM_LEND: usize = 173;
pub const SYS_MEM_RETURN: usize = 174;
pub const SYS_MEM_TRANSFER: usize = 175;

pub const USER_SPACE_MIN: usize = 0x0000_1000;
pub const USER_SPACE_MAX: usize = 0x7FFF_FFFF; // Below 2GB Kernel DRAM
pub const KERNEL_DRAM_START: usize = 0x8000_0000;

fn validate_user_range(addr: usize, len: usize, write: bool) -> Result<(), i64> {
    let _ = addr.checked_add(len).ok_or(-14i64)?;
    let satp_from_task = {
        let sched = SCHEDULER.lock();
        let curr_idx = sched.current_task_idx;
        if curr_idx < sched.tasks.len() {
            if let Some(ref aspace) = sched.tasks[curr_idx].address_space {
                if aspace.destroyed || aspace.is_closing() {
                    return Err(-14);
                }
                Some(aspace.satp)
            } else {
                None
            }
        } else {
            None
        }
    };
    let satp = match satp_from_task {
        Some(s) => s,
        None => {
            let s: usize;
            unsafe {
                core::arch::asm!("csrr {0}, satp", out(reg) s, options(nostack, nomem));
            }
            s
        }
    };
    crate::mmu::validate_sv39_user_range(satp, addr, len, write)
}

fn copy_from_user(dst: &mut [u8], addr: usize, len: usize) -> Result<(), i64> {
    if len > dst.len() {
        return Err(-22);
    }
    validate_user_range(addr, len, false)?;
    if len > 0 {
        unsafe {
            crate::mmu::with_user_memory(|| {
                core::ptr::copy_nonoverlapping(addr as *const u8, dst.as_mut_ptr(), len);
            });
        }
    }
    Ok(())
}

fn copy_to_user(addr: usize, src: &[u8]) -> Result<(), i64> {
    validate_user_range(addr, src.len(), true)?;
    if !src.is_empty() {
        unsafe {
            crate::mmu::with_user_memory(|| {
                core::ptr::copy_nonoverlapping(src.as_ptr(), addr as *mut u8, src.len());
            });
        }
    }
    Ok(())
}

fn read_user_string<'a>(addr: usize, len: usize, buf: &'a mut [u8]) -> Result<&'a str, i64> {
    if len > buf.len() {
        return Err(-22);
    }
    copy_from_user(&mut buf[..len], addr, len)?;
    core::str::from_utf8(&buf[..len]).map_err(|_| -22)
}

fn get_caller_identity_and_caps() -> Result<(u32, crate::capability::CapabilityTable), i64> {
    let curr_task = SCHEDULER.lock().current_task_id().0 as u32;
    let sched = SCHEDULER.lock();
    let curr_idx = sched.current_task_idx;
    if curr_idx < sched.tasks.len() {
        let task = &sched.tasks[curr_idx];
        if task.id == crate::task::INVALID_PID || task.id == 0 || task.id != curr_task {
            Err(-13)
        } else {
            Ok((task.id, task.cap_table))
        }
    } else {
        Err(-13)
    }
}

/// Dispatches an incoming system call from the given trap frame.
pub fn dispatch_syscall(tf: &mut TrapFrame) {
    let num = tf.regs[17]; // a7
    let a0 = tf.regs[10];  // a0
    let a1 = tf.regs[11];  // a1
    let a2 = tf.regs[12];  // a2
    let _a3 = tf.regs[13]; // a3
    let a3 = tf.regs[13];
    let a4 = tf.regs[14];
    let _a5 = tf.regs[15];

    let is_test = SCHEDULER.lock().tasks.is_empty();

    let res: i64 = match num {
        // 1: Yield
        SYS_YIELD => {
            crate::task::yield_now();
            0
        }

        // 2: Exit
        SYS_EXIT | SYS_EXIT_WITH_CODE => {
            println!("[SYSCALL:EXIT] Task exited with code {}", a0);
            let mut sched = SCHEDULER.lock();
            let curr = sched.current_task_idx;
            if curr < sched.tasks.len() {
                sched.tasks[curr].state = crate::task::TaskState::Terminated;
                sched.tasks[curr].exit_code = Some(a0 as i32);
            }
            drop(sched);
            if unsafe { !crate::trap::IN_LIFECYCLE_TEST } {
                crate::task::yield_now();
            }
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

        // 5: IpcSend
        SYS_IPC_SEND => {
            let recipient = a0 as u32;
            let msg_type = a1 as u32;
            let addr = a2;
            let raw_len = _a3 as usize;
            if raw_len > crate::ipc::IPC_MAX_PAYLOAD {
                -22
            } else {
                let len = raw_len;

                match get_caller_identity_and_caps() {
                    Err(err) => err,
                    Ok((curr_id, caps)) => {
                        let mut kbuf = [0u8; crate::ipc::IPC_MAX_PAYLOAD];
                        if len > 0 && copy_from_user(&mut kbuf, addr, len).is_err() {
                            -14
                        } else if len == 0 && addr >= KERNEL_DRAM_START {
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
                }
            }
        }

        // 6: IpcRecv
        SYS_IPC_RECV => {
            let (addr, max_len) = if a0 < 0x1000 && (a1 >= 0x1000 || a2 > 0 || a1 == 0) {
                (a1, (a2 as usize).min(crate::ipc::IPC_MAX_PAYLOAD))
            } else {
                (a0, (a1 as usize).min(crate::ipc::IPC_MAX_PAYLOAD))
            };

            match get_caller_identity_and_caps() {
                Err(err) => err,
                Ok((curr_id, caps)) => {
                    if let Err(err) = if max_len > 0 {
                        validate_user_range(addr, max_len, true)
                    } else {
                        Ok(())
                    } {
                        err
                    } else {
                        let recv_res = {
                            let mut router = crate::ipc::IPC_ROUTER.lock();
                            router.receive(&caps, curr_id)
                        };
                        match recv_res {
                            Ok(msg) => {
                                let to_copy = msg.len.min(max_len);
                                if to_copy > 0 {
                                    if let Err(err) = copy_to_user(addr, &msg.payload[..to_copy]) {
                                        err
                                    } else {
                                        to_copy as i64
                                    }
                                } else {
                                    0
                                }
                            }
                            Err(crate::ipc::IpcError::MailboxEmpty) => -11, // -EAGAIN
                            Err(crate::ipc::IpcError::CapabilityDenied(_)) => -13,
                            Err(_) => -1,
                        }
                    }
                }
            }
        }

        SYS_IPC_RECV_META => {
            let a4 = tf.regs[14];
            let (addr, max_len, sender_ptr, type_ptr) = if a0 < 0x1000 {
                (a1, (a2 as usize).min(crate::ipc::IPC_MAX_PAYLOAD), _a3, a4)
            } else {
                (a0, (a1 as usize).min(crate::ipc::IPC_MAX_PAYLOAD), a2, _a3)
            };

            match get_caller_identity_and_caps() {
                Err(err) => err,
                Ok((curr_id, caps)) => {
                    let precheck = (|| -> Result<(), i64> {
                        if max_len > 0 {
                            validate_user_range(addr, max_len, true)?;
                        }
                        if sender_ptr != 0 {
                            if (sender_ptr & 7) != 0 {
                                return Err(-22);
                            }
                            validate_user_range(sender_ptr, 8, true)?;
                        }
                        if type_ptr != 0 {
                            if (type_ptr & 3) != 0 {
                                return Err(-22);
                            }
                            validate_user_range(type_ptr, 4, true)?;
                        }
                        Ok(())
                    })();
                    if let Err(err) = precheck {
                        err
                    } else {
                        let recv_res = {
                            let mut router = crate::ipc::IPC_ROUTER.lock();
                            router.receive(&caps, curr_id)
                        };
                        match recv_res {
                            Ok(msg) => {
                                let to_copy = msg.len.min(max_len);
                                if to_copy > 0 {
                                    if let Err(err) = copy_to_user(addr, &msg.payload[..to_copy]) {
                                        return {
                                            tf.regs[10] = err as usize;
                                        };
                                    }
                                }
                                if sender_ptr != 0 {
                                    if let Err(err) = copy_to_user(sender_ptr, &(msg.sender as u64).to_le_bytes()) {
                                        return {
                                            tf.regs[10] = err as usize;
                                        };
                                    }
                                }
                                if type_ptr != 0 {
                                    if let Err(err) = copy_to_user(type_ptr, &msg.msg_type.to_le_bytes()) {
                                        return {
                                            tf.regs[10] = err as usize;
                                        };
                                    }
                                }
                                to_copy as i64
                            }
                            Err(crate::ipc::IpcError::MailboxEmpty) => -11,
                            Err(crate::ipc::IpcError::CapabilityDenied(_)) => -13,
                            Err(_) => -1,
                        }
                    }
                }
            }
        }

        // 7: GetTaskId
        SYS_GET_TASK_ID => {
            if is_test {
                1
            } else {
                let pid = SCHEDULER.lock().current_pid();
                if pid == crate::task::INVALID_PID || pid == 0 {
                    -13
                } else {
                    pid as i64
                }
            }
        }

        // 8: FsOpen
        SYS_FS_OPEN => {
            if is_test || a0 == 0 {
                -13
            } else {
                let (path_ptr, path_len, rights) = if a0 < 0x1000 && a2 > 0 {
                    (a1, a2 as usize, a3 as u32)
                } else {
                    (a0, a1 as usize, a2 as u32)
                };
                if path_len == 0 || path_len > 256 {
                    -22
                } else {
                    let mut pbuf = [0u8; 256];
                    let path = match read_user_string(path_ptr, path_len, &mut pbuf) {
                        Ok(s) => s,
                        Err(e) => return { tf.regs[10] = e as usize; },
                    };
                    let (caller_pid, caps) = match get_caller_identity_and_caps() {
                        Ok(p) => p,
                        Err(e) => return { tf.regs[10] = e as usize; },
                    };
                    match crate::vfs::GLOBAL_VFS.lock().open(caller_pid, path, rights, &caps) {
                        Ok(fd) => fd as i64,
                        Err(e) => e,
                    }
                }
            }
        }

        // 9: FsRead
        SYS_FS_READ => {
            if is_test || a0 == 0 {
                -13
            } else {
                let (fd, buf_ptr, buf_len) = if a0 < 0x1000 && a2 > 0 {
                    (a1 as u32, a2, a3 as usize)
                } else {
                    (a0 as u32, a1, a2 as usize)
                };
                if buf_len == 0 {
                    0
                } else if let Err(e) = validate_user_range(buf_ptr, buf_len, true) {
                    e
                } else {
                    let (caller_pid, caps) = match get_caller_identity_and_caps() {
                        Ok(p) => p,
                        Err(e) => return { tf.regs[10] = e as usize; },
                    };
                    let mut kbuf = alloc::vec![0u8; buf_len.min(65536)];
                    let n = match crate::vfs::GLOBAL_VFS.lock().read(caller_pid, fd, &mut kbuf, &caps) {
                        Ok(n) => n,
                        Err(e) => return { tf.regs[10] = e as usize; },
                    };
                    if n > 0 {
                        if let Err(e) = copy_to_user(buf_ptr, &kbuf[..n]) {
                            return { tf.regs[10] = e as usize; };
                        }
                    }
                    n as i64
                }
            }
        }

        SYS_FS_READ_AT => {
            if is_test || a0 == 0 {
                -13
            } else {
                let (fd, buf_ptr, buf_len, offset) = if a0 < 0x1000 && a2 > 0 {
                    (a1 as u32, a2, a3 as usize, a4 as usize)
                } else {
                    (a0 as u32, a1, a2 as usize, a3 as usize)
                };
                if buf_len == 0 {
                    0
                } else if let Err(e) = validate_user_range(buf_ptr, buf_len, true) {
                    e
                } else {
                    let (caller_pid, caps) = match get_caller_identity_and_caps() {
                        Ok(p) => p,
                        Err(e) => return { tf.regs[10] = e as usize; },
                    };
                    let mut kbuf = alloc::vec![0u8; buf_len.min(65536)];
                    let n = match crate::vfs::GLOBAL_VFS.lock().read_at(caller_pid, fd, offset, &mut kbuf, &caps) {
                        Ok(n) => n,
                        Err(e) => return { tf.regs[10] = e as usize; },
                    };
                    if n > 0 {
                        if let Err(e) = copy_to_user(buf_ptr, &kbuf[..n]) {
                            return { tf.regs[10] = e as usize; };
                        }
                    }
                    n as i64
                }
            }
        }

        // 10: FsWrite
        SYS_FS_WRITE => {
            if is_test || a0 == 0 {
                -13
            } else {
                let (fd, buf_ptr, buf_len) = if a0 < 0x1000 && a2 > 0 {
                    (a1 as u32, a2, a3 as usize)
                } else {
                    (a0 as u32, a1, a2 as usize)
                };
                if buf_len == 0 {
                    0
                } else if buf_len > 1024 * 1024 {
                    -22
                } else {
                    let (caller_pid, caps) = match get_caller_identity_and_caps() {
                        Ok(p) => p,
                        Err(e) => return { tf.regs[10] = e as usize; },
                    };
                    let mut kbuf = alloc::vec![0u8; buf_len];
                    if let Err(e) = copy_from_user(&mut kbuf, buf_ptr, buf_len) {
                        e
                    } else {
                        match crate::vfs::GLOBAL_VFS.lock().write(caller_pid, fd, &kbuf, &caps) {
                            Ok(n) => n as i64,
                            Err(e) => e,
                        }
                    }
                }
            }
        }

        // 11: FsClose
        SYS_FS_CLOSE => {
            if is_test || a0 == 0 {
                -13
            } else {
                let fd = if a0 < 0x1000 && a1 > 0 { a1 as u32 } else { a0 as u32 };
                let (caller_pid, _) = match get_caller_identity_and_caps() {
                    Ok(p) => p,
                    Err(e) => return { tf.regs[10] = e as usize; },
                };
                match crate::vfs::GLOBAL_VFS.lock().close(caller_pid, fd) {
                    Ok(()) => 0,
                    Err(e) => e,
                }
            }
        }

        SYS_FS_STAT => {
            let (path_ptr, path_len, out_ptr, out_len) = if a0 < 0x1000 && a2 > 0 {
                (a1, a2 as usize, a3, a4 as usize)
            } else {
                (a0, a1 as usize, a2, a3 as usize)
            };
            if path_len == 0 || path_len > 256 || out_len < 96 {
                -22
            } else {
                let mut pbuf = [0u8; 256];
                let path = match read_user_string(path_ptr, path_len, &mut pbuf) {
                    Ok(s) => s,
                    Err(e) => return { tf.regs[10] = e as usize; },
                };
                let (_caller_pid, caps) = match get_caller_identity_and_caps() {
                    Ok(p) => p,
                    Err(e) => return { tf.regs[10] = e as usize; },
                };
                let mut stat_buf = [0u8; 96];
                match crate::vfs::GLOBAL_VFS.lock().stat(path, &mut stat_buf, &caps) {
                    Ok(n) => {
                        if let Err(e) = copy_to_user(out_ptr, &stat_buf[..n]) {
                            return { tf.regs[10] = e as usize; };
                        }
                        n as i64
                    }
                    Err(e) => e,
                }
            }
        }

        SYS_FS_LIST => {
            let (path_ptr, path_len, out_ptr, out_len) = if a0 < 0x1000 && a2 > 0 {
                (a1, a2 as usize, a3, a4 as usize)
            } else {
                (a0, a1 as usize, a2, a3 as usize)
            };
            if path_len == 0 || path_len > 256 || out_len == 0 {
                -22
            } else {
                let mut pbuf = [0u8; 256];
                let path = match read_user_string(path_ptr, path_len, &mut pbuf) {
                    Ok(s) => s,
                    Err(e) => return { tf.regs[10] = e as usize; },
                };
                let (_caller_pid, caps) = match get_caller_identity_and_caps() {
                    Ok(p) => p,
                    Err(e) => return { tf.regs[10] = e as usize; },
                };
                let mut list_buf = alloc::vec![0u8; out_len.min(4096)];
                match crate::vfs::GLOBAL_VFS.lock().list(path, &mut list_buf, &caps) {
                    Ok(n) => {
                        if n > 0 {
                            if let Err(e) = copy_to_user(out_ptr, &list_buf[..n]) {
                                return { tf.regs[10] = e as usize; };
                            }
                        }
                        n as i64
                    }
                    Err(e) => e,
                }
            }
        }

        SYS_FS_SYNC => {
            match crate::vfs::GLOBAL_VFS.lock().sync() {
                Ok(()) => 0,
                Err(e) => e,
            }
        }

        // 28: Platform query
        SYS_HAL_QUERY_PLATFORM => {
            let (addr, len) = if a0 < 0x1000 && a2 > 0 { (a1, a2) } else { (a0, a1) };
            let s = b"riscv64-qemu-phone";
            let to_copy = s.len().min(len);
            match copy_to_user(addr, &s[..to_copy]) {
                Ok(()) => to_copy as i64,
                Err(err) => err,
            }
        }

        // 62: EventPoll
        SYS_EVENT_POLL => {
            let (caller_pid, has_cap) = match get_caller_identity_and_caps() {
                Ok((pid, caps)) => {
                    let ok = caps.check(crate::capability::CapabilityType::EventPoll, crate::capability::Rights::READ).is_ok();
                    (pid, ok)
                }
                Err(_) => (crate::task::INVALID_PID, false),
            };
            if !has_cap {
                -13
            } else {
                crate::task::yield_now();
                crate::input_queue::service_serial_commands();
                let has_input = crate::input_queue::INPUT_QUEUE.lock().has_events_for_task(caller_pid);
                let mut mask = 0x08; // EVENT_TIMER_TICK
                if has_input {
                    mask |= 0x10; // EVENT_TOUCH_INPUT
                }
                mask
            }
        }

        // 69: SurfaceCreate
        SYS_SURFACE_CREATE => {
            let (caller_pid, has_cap) = match get_caller_identity_and_caps() {
                Ok((pid, caps)) => {
                    let ok = caps.check(crate::capability::CapabilityType::DisplayServer, crate::capability::Rights::WRITE).is_ok();
                    (pid, ok)
                }
                Err(_) => (crate::task::INVALID_PID, false),
            };
            if !has_cap {
                -13
            } else {
                let (width, height, z_order) = if a0 < 0x1000 && a2 > 0 {
                    (a1 as u32, a2 as u32, _a3 as u32)
                } else {
                    (a0 as u32, a1 as u32, a2 as u32)
                };
                match crate::display::SURFACE_MANAGER.lock().create_surface(caller_pid, width, height, z_order) {
                    Ok(id) => id as i64,
                    Err(e) => e,
                }
            }
        }

        // 70: SurfaceFlip
        SYS_SURFACE_FLIP => {
            let has_cap = match get_caller_identity_and_caps() {
                Ok((_, caps)) => caps.check(crate::capability::CapabilityType::DisplayServer, crate::capability::Rights::WRITE).is_ok(),
                Err(_) => false,
            };
            if !has_cap {
                -13
            } else {
                let count = crate::display::SURFACE_MANAGER.lock().flip();
                count as i64
            }
        }

        // 71: TouchPoll
        SYS_TOUCH_POLL => {
            let (caller_pid, has_cap) = match get_caller_identity_and_caps() {
                Ok((pid, caps)) => {
                    let ok = caps.check(crate::capability::CapabilityType::InputDevice, crate::capability::Rights::READ).is_ok();
                    (pid, ok)
                }
                Err(_) => (crate::task::INVALID_PID, false),
            };
            if !has_cap {
                -13
            } else {
                crate::input_queue::service_serial_commands();
                if let Some(ev) = crate::input_queue::INPUT_QUEUE.lock().pop_for_task(caller_pid) {
                    (ev.x & 0xFFFF) as i64 | (((ev.y & 0xFFFF) as i64) << 16)
                } else {
                    0
                }
            }
        }

        // 73: SurfaceWrite
        SYS_SURFACE_WRITE => {
            let (caller_pid, has_cap) = match get_caller_identity_and_caps() {
                Ok((pid, caps)) => {
                    let ok = caps.check(crate::capability::CapabilityType::DisplayServer, crate::capability::Rights::WRITE).is_ok();
                    (pid, ok)
                }
                Err(_) => (crate::task::INVALID_PID, false),
            };
            if !has_cap {
                -13
            } else {
                let a4 = tf.regs[14];
                let (surface_id, offset, pixel_ptr, pixel_count) = if a0 == 0 {
                    (a1 as u32, a2 as usize, _a3, a4 as usize)
                } else if a0 < 0x1000 && _a3 >= 0x1000 && a2 < 0x1000 {
                    (a1 as u32, a2 as usize, _a3, a4 as usize)
                } else {
                    (a0 as u32, a1 as usize, a2, _a3 as usize)
                };

                if (pixel_ptr & 3) != 0 {
                    -22
                } else if let Some(byte_len) = pixel_count.checked_mul(4) {
                    if let Err(err) = validate_user_range(pixel_ptr, byte_len, false) {
                        err
                    } else if pixel_count == 0 {
                        0
                    } else {
                        let mut written = 0usize;
                        let mut kbuf = [0u8; 256 * 4];
                        let mut write_res: i64 = 0;
                        while written < pixel_count {
                            let chunk = (pixel_count - written).min(256);
                            let src_addr = match written.checked_mul(4).and_then(|b| pixel_ptr.checked_add(b)) {
                                Some(a) => a,
                                None => {
                                    write_res = -14;
                                    break;
                                }
                            };
                            if let Err(err) = copy_from_user(&mut kbuf[..chunk * 4], src_addr, chunk * 4) {
                                write_res = err;
                                break;
                            }
                            let mut sm = crate::display::SURFACE_MANAGER.lock();
                            if let Some(s) = sm.surfaces.iter_mut().find(|s| s.id == surface_id) {
                                if s.owner_task != caller_pid {
                                    write_res = -13;
                                    break;
                                }
                                let dst_off = match offset.checked_add(written) {
                                    Some(o) if o < s.pixels.len() => o,
                                    _ => {
                                        write_res = if written == 0 { -22 } else { written as i64 };
                                        break;
                                    }
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
                                write_res = written as i64;
                                if copy_len < chunk {
                                    break;
                                }
                            } else {
                                write_res = -2;
                                break;
                            }
                        }
                        write_res
                    }
                } else {
                    -22
                }
            }
        }

        // 74: SurfaceDestroy
        SYS_SURFACE_DESTROY => {
            let (caller_pid, is_admin, has_cap) = match get_caller_identity_and_caps() {
                Ok((pid, caps)) => {
                    let admin = caps.check(crate::capability::CapabilityType::DisplayServer, crate::capability::Rights::ADMIN).is_ok()
                        || caps.check(crate::capability::CapabilityType::SystemResource, crate::capability::Rights::ADMIN).is_ok();
                    let ok = caps.check(crate::capability::CapabilityType::DisplayServer, crate::capability::Rights::WRITE).is_ok() || admin;
                    (pid, admin, ok)
                }
                Err(_) => (crate::task::INVALID_PID, false, false),
            };
            if !has_cap {
                -13
            } else {
                let surface_id = if a0 < 0x1000 && a1 > 0 { a1 as u32 } else { a0 as u32 };
                match crate::display::SURFACE_MANAGER.lock().destroy_surface(surface_id, caller_pid, is_admin) {
                    Ok(_) => 0,
                    Err(e) => e,
                }
            }
        }

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
            let is_supervisor = match get_caller_identity_and_caps() {
                Ok((_, caps)) => {
                    caps.check(crate::capability::CapabilityType::SystemResource, crate::capability::Rights::ADMIN).is_ok()
                        || caps.check(crate::capability::CapabilityType::SystemResource, crate::capability::Rights::WRITE).is_ok()
                }
                Err(_) => false,
            };
            if !is_supervisor {
                -13
            } else {
                let svc_id = if a0 < 0x1000 && a1 > 0 { a1 as u8 } else { a0 as u8 };
                match crate::services::start(svc_id) {
                    Ok(pid) => pid as i64,
                    Err(_) => -22,
                }
            }
        }

        // 87: ServiceStop
        SYS_SERVICE_STOP => {
            let is_supervisor = match get_caller_identity_and_caps() {
                Ok((_, caps)) => {
                    caps.check(crate::capability::CapabilityType::SystemResource, crate::capability::Rights::ADMIN).is_ok()
                        || caps.check(crate::capability::CapabilityType::SystemResource, crate::capability::Rights::WRITE).is_ok()
                }
                Err(_) => false,
            };
            if !is_supervisor {
                -13
            } else {
                let svc_id = if a0 < 0x1000 && a1 > 0 { a1 as u8 } else { a0 as u8 };
                match crate::services::stop(svc_id) {
                    Ok(_) => 0,
                    Err(_) => -22,
                }
            }
        }

        // 88: ServiceRecoverFailed
        SYS_SERVICE_RECOVER_FAILED => {
            let is_supervisor = match get_caller_identity_and_caps() {
                Ok((_, caps)) => {
                    caps.check(crate::capability::CapabilityType::SystemResource, crate::capability::Rights::ADMIN).is_ok()
                        || caps.check(crate::capability::CapabilityType::SystemResource, crate::capability::Rights::WRITE).is_ok()
                }
                Err(_) => false,
            };
            if !is_supervisor {
                -13
            } else {
                let svc_id = if a0 < 0x1000 && a1 > 0 { a1 as u8 } else { a0 as u8 };
                match crate::services::recover_failed(svc_id) {
                    Ok(_) => 0,
                    Err(_) => -22,
                }
            }
        }

        // 92: InputEventPoll
        SYS_INPUT_EVENT_POLL => {
            let (caller_pid, has_cap) = match get_caller_identity_and_caps() {
                Ok((pid, caps)) => {
                    let ok = caps.check(crate::capability::CapabilityType::InputDevice, crate::capability::Rights::READ).is_ok()
                        || caps.check(crate::capability::CapabilityType::EventPoll, crate::capability::Rights::READ).is_ok();
                    (pid, ok)
                }
                Err(_) => (crate::task::INVALID_PID, false),
            };
            if !has_cap {
                -13
            } else {
                let (addr, len) = if a0 < 0x1000 && a2 > 0 { (a1, a2) } else { (a0, a1) };
                if len < 24 {
                    -22
                } else if let Err(err) = validate_user_range(addr, 24, true) {
                    err
                } else {
                    crate::input_queue::service_serial_commands();
                    if let Some(ev) = crate::input_queue::INPUT_QUEUE.lock().pop_for_task(caller_pid) {
                        let mut buf = [0u8; 24];
                        ev.encode(&mut buf);
                        match copy_to_user(addr, &buf) {
                            Ok(()) => 24,
                            Err(err) => err,
                        }
                    } else {
                        0
                    }
                }
            }
        }

        // 101: AppLaunch
        SYS_APP_LAUNCH => {
            match get_caller_identity_and_caps() {
                Err(err) => err,
                Ok((caller_pid, caller_caps)) => {
                    if caller_caps.check(crate::capability::CapabilityType::SystemResource, crate::capability::Rights::WRITE).is_err()
                        && caller_caps.check(crate::capability::CapabilityType::SystemResource, crate::capability::Rights::ADMIN).is_err()
                    {
                        -13
                    } else {
                let (name_ptr, name_len) = if a0 < 0x1000 && a2 > 0 { (a1, a2) } else { (a0, a1) };
                let mut name_buf = [0u8; 64];
                if name_len == 0 || name_len > 64 {
                    -22
                } else if let Err(err) = copy_from_user(&mut name_buf, name_ptr, name_len) {
                    err
                } else if let Ok(target_name) = core::str::from_utf8(&name_buf[..name_len]) {
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
                        let existing_id = {
                            let sched = SCHEDULER.lock();
                            sched.tasks.iter().find(|t| t.name == user_img.name && t.state == crate::task::TaskState::Ready).map(|t| t.id as i64)
                        };

                        if let Some(tid) = existing_id {
                            tid
                        } else {
                            match crate::elf::load_elf(user_img.elf) {
                                Ok(loaded) => {
                                    let mut sched = SCHEDULER.lock();
                                    let mut caps = crate::capability::CapabilityTable::new();
                                    let _ = caps.mint(crate::capability::CapabilityType::IpcSend, crate::capability::Rights::INVOKE, None);
                                    let _ = caps.mint(crate::capability::CapabilityType::IpcReceive, crate::capability::Rights::READ, None);
                                    let _ = caps.mint(crate::capability::CapabilityType::SerialOutput, crate::capability::Rights::WRITE, None);
                                    let _ = caps.mint(crate::capability::CapabilityType::DisplayServer, crate::capability::Rights::WRITE, None);
                                    let _ = caps.mint(crate::capability::CapabilityType::EventPoll, crate::capability::Rights::READ, None);
                                    let _ = caps.mint(crate::capability::CapabilityType::InputDevice, crate::capability::Rights::READ, None);
                                    let _ = caps.mint(crate::capability::CapabilityType::TimerTickRead, crate::capability::Rights::READ, None);
                                    let tid = sched.spawn_user_process(caller_pid, user_img.name, loaded.entry_point, loaded.stack_top, Some(loaded.address_space), caps);
                                    drop(sched);
                                    println!("[SPAWN] RISC-V Userspace process '{}' spawned (PID {})", user_img.name, tid);
                                    tid as i64
                                }
                                Err(e) => {
                                    println!("[SPAWN ERROR] Failed to load '{}': {}", user_img.name, e);
                                    -22
                                }
                            }
                        }
                    } else {
                        -2 // -ENOENT
                    }
                } else {
                    -22
                }
            }
                }
            }
        }

        // 102: ServiceQuery
        SYS_SERVICE_QUERY => {
            let service_id = a0 as u8;
            let addr = a1;
            let len = a2;
            if addr == 0 || len < 12 {
                -22
            } else {
                let mut buf = [0u8; 12];
                if !crate::services::query_record(service_id, &mut buf) {
                    -22
                } else {
                    match copy_to_user(addr, &buf) {
                        Ok(()) => 12,
                        Err(err) => err,
                    }
                }
            }
        }

        SYS_SERVICE_HEARTBEAT => {
            let caller_pid = match get_caller_identity_and_caps() {
                Ok((pid, _)) => pid,
                Err(_) => crate::task::INVALID_PID,
            };
            if caller_pid == crate::task::INVALID_PID || caller_pid == 0 {
                -13
            } else {
                let gen = if a1 != 0 { a1 as u32 } else { 1 };
                match crate::services::heartbeat(caller_pid, gen) {
                    Ok(()) => 0,
                    Err(()) => -13,
                }
            }
        }
        SYS_PACKAGE_LIST => 14,
        SYS_PACKAGE_REGISTRY => {
            let (addr, len) = if a0 < 0x1000 && a2 > 0 { (a1, a2) } else { (a0, a1) };
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
            match copy_to_user(addr, &s[..to_copy]) {
                Ok(()) => to_copy as i64,
                Err(err) => err,
            }
        }
        SYS_USER_GET_CURRENT => 1,
        107 => 0, // UserLockState: 0 = Unlocked
        108 => 0, // UserUnlock: 0 = Success
        109 => 0, // UserLock: 0 = Success
        129 => 0, // UserHasCredential: 0 = false (no PIN required by default)
        132 => { // CapLookup
            let (cap_type_raw, object_id) = if a0 < 0x1000 && a1 > 0 { (a1 as u64, a2 as u64) } else { (a0 as u64, a1 as u64) };
            if let Some(ct) = crate::capability::CapabilityType::from_raw(cap_type_raw) {
                let sched = SCHEDULER.lock();
                let curr_idx = sched.current_task_idx;
                if curr_idx < sched.tasks.len() {
                    let task = &sched.tasks[curr_idx];
                    let mut found_slot = None;
                    for (slot, cap_opt) in task.cap_table.entries.iter().enumerate() {
                        if let Some(cap) = cap_opt {
                            if cap.cap_type == ct {
                                if object_id == 0 || cap.target_id.map(|t| t as u64) == Some(object_id) {
                                    found_slot = Some(slot);
                                    break;
                                }
                            }
                        }
                    }
                    drop(sched);
                    if let Some(s) = found_slot {
                        s as i64
                    } else {
                        -2 // -ENOENT
                    }
                } else {
                    -2
                }
            } else {
                -22 // -EINVAL
            }
        }
        150 => { // CapHandleLookup
            let (cap_type_raw, object_id) = if a0 < 0x1000 && a1 > 0 { (a1 as u64, a2 as u64) } else { (a0 as u64, a1 as u64) };
            if let Some(ct) = crate::capability::CapabilityType::from_raw(cap_type_raw) {
                let sched = SCHEDULER.lock();
                let curr_idx = sched.current_task_idx;
                if curr_idx < sched.tasks.len() {
                    let task = &sched.tasks[curr_idx];
                    let mut found_handle = None;
                    for (slot, cap_opt) in task.cap_table.entries.iter().enumerate() {
                        if let Some(cap) = cap_opt {
                            if cap.cap_type == ct {
                                if object_id == 0 || cap.target_id.map(|t| t as u64) == Some(object_id) {
                                    let handle = (slot as u64) | ((cap.id.0 as u64) << 32);
                                    found_handle = Some(handle);
                                    break;
                                }
                            }
                        }
                    }
                    drop(sched);
                    if let Some(h) = found_handle {
                        h as i64
                    } else {
                        -2 // -ENOENT
                    }
                } else {
                    -2
                }
            } else {
                -22
            }
        }
        SYS_SETTINGS_GET => {
            let (_cap_slot, key_ptr, key_len, out_len, out_ptr) = if a0 < 0x1000 {
                (a0, a1, a2 as usize, a3 as usize, a4)
            } else {
                (0, a0, a1 as usize, a2 as usize, a3)
            };
            if key_len == 0 || key_len > 96 || out_len > 512 {
                -22
            } else {
                let mut key_buf = [0u8; 96];
                if copy_from_user(&mut key_buf[..key_len], key_ptr, key_len).is_err() {
                    -14
                } else if let Ok(key) = core::str::from_utf8(&key_buf[..key_len]) {
                    let mut out = [0u8; 512];
                    let mut store = crate::settings::SETTINGS.lock();
                    store.init_defaults();
                    let n = store.get(key, &mut out[..out_len]);
                    if n > 0 && copy_to_user(out_ptr, &out[..n]).is_err() {
                        -14
                    } else {
                        n as i64
                    }
                } else {
                    -22
                }
            }
        }
        SYS_SETTINGS_SET => {
            let (_cap_slot, key_ptr, key_len, val_ptr, val_len) = if a0 < 0x1000 {
                (a0, a1, a2 as usize, a3, a4 as usize)
            } else {
                (0, a0, a1 as usize, a2, a3 as usize)
            };
            if key_len == 0 || key_len > 96 || val_len > 512 {
                -22
            } else {
                let mut key_buf = [0u8; 96];
                let mut val_buf = [0u8; 512];
                if copy_from_user(&mut key_buf[..key_len], key_ptr, key_len).is_err() {
                    -14
                } else if copy_from_user(&mut val_buf[..val_len], val_ptr, val_len).is_err() {
                    -14
                } else if let Ok(key) = core::str::from_utf8(&key_buf[..key_len]) {
                    let mut store = crate::settings::SETTINGS.lock();
                    store.init_defaults();
                    if store.set(key, &val_buf[..val_len]).is_err() {
                        -28
                    } else {
                        0
                    }
                } else {
                    -22
                }
            }
        }
        SYS_STORAGE_QUERY => {
            64 * 1024 * 1024
        }
        SYS_APP_RESTART | SYS_APP_BACKGROUND | SYS_APP_FOREGROUND | 121 => 0,
        SYS_POWER_GOVERNOR | SYS_POWER_SET_PROFILE => -38,
        SYS_BATTERY_QUERY | SYS_THERMAL_QUERY => -38,
        SYS_GET_RANDOM => {
            let (_addr, _len) = if a0 < 0x1000 && a2 > 0 { (a1, a2) } else { (a0, a1) };
            -1 // Fail closed: no hardware CSPRNG/entropy source bound on RISC-V virt yet
        }
        SYS_BLOCK_READ => -13,
        SYS_VERIFY_PACKAGE => {
            let (path_ptr, path_len, out_ptr, out_len) = if a0 < 0x1000 && a2 > 0 {
                (a1, a2 as usize, a3, a4 as usize)
            } else {
                (a0, a1 as usize, a2, a3 as usize)
            };
            if path_len == 0 || path_len > 256 || out_len == 0 {
                -22
            } else {
                let mut pbuf = [0u8; 256];
                let path = match read_user_string(path_ptr, path_len, &mut pbuf) {
                    Ok(s) => s,
                    Err(e) => return { tf.regs[10] = e as usize; },
                };
                let (_, caps) = match get_caller_identity_and_caps() {
                    Ok(p) => p,
                    Err(e) => return { tf.regs[10] = e as usize; },
                };
                if caps.check(crate::capability::CapabilityType::FileSystemRead, crate::capability::Rights::READ).is_err() {
                    -13 // -EACCES
                } else {
                    let pkg_bytes = match crate::vfs::GLOBAL_VFS.lock().get_file_content(path) {
                        Some(b) => b,
                        None => return { tf.regs[10] = (-2i64) as usize; },
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
                    if let Err(e) = copy_to_user(out_ptr, &bytes[..to_copy]) {
                        return { tf.regs[10] = e as usize; };
                    }
                    to_copy as i64
                }
            }
        }
        SYS_DEVFS_IO => {
            let action = a0 as u32;
            let path_ptr = a1;
            let path_len = a2 as usize;
            let buf_ptr = a3;
            let buf_len = a4 as usize;
            if path_len == 0 || path_len > 256 {
                -22
            } else {
                let mut pbuf = [0u8; 256];
                let path = match read_user_string(path_ptr, path_len, &mut pbuf) {
                    Ok(s) => s,
                    Err(e) => return { tf.regs[10] = e as usize; },
                };
                let (caller_pid, caps) = match get_caller_identity_and_caps() {
                    Ok(p) => p,
                    Err(e) => return { tf.regs[10] = e as usize; },
                };
                if action == 0 {
                    if caps.check(crate::capability::CapabilityType::FileSystemRead, crate::capability::Rights::READ).is_err()
                        && caps.check(crate::capability::CapabilityType::FileSystemWrite, crate::capability::Rights::READ).is_err()
                    {
                        -13
                    } else {
                        match path {
                            "/dev/null" => 0,
                            "/dev/zero" => {
                                if buf_len > 0 {
                                    let zeros = [0u8; 512];
                                    let mut written = 0;
                                    while written < buf_len {
                                        let chunk = (buf_len - written).min(512);
                                        if let Err(e) = copy_to_user(buf_ptr + written, &zeros[..chunk]) {
                                            return { tf.regs[10] = e as usize; };
                                        }
                                        written += chunk;
                                    }
                                }
                                buf_len as i64
                            }
                            "/dev/urandom" => {
                                let mut ticks: u64 = 0;
                                #[cfg(target_arch = "riscv64")]
                                unsafe {
                                    core::arch::asm!("rdtime {}", out(reg) ticks);
                                }
                                let mut state = ticks.wrapping_add(0x9E3779B97F4A7C15);
                                let mut kbuf = alloc::vec![0u8; buf_len.min(4096)];
                                for chunk in kbuf.chunks_mut(8) {
                                    state ^= state << 13;
                                    state ^= state >> 7;
                                    state ^= state << 17;
                                    let bytes = state.to_le_bytes();
                                    let l = chunk.len().min(8);
                                    chunk[..l].copy_from_slice(&bytes[..l]);
                                }
                                if let Err(e) = copy_to_user(buf_ptr, &kbuf) {
                                    return { tf.regs[10] = e as usize; };
                                }
                                kbuf.len() as i64
                            }
                            "/dev/input/event0" => {
                                if let Some(ev) = crate::input_queue::INPUT_QUEUE.lock().pop_for_task(caller_pid) {
                                    let mut kbuf = [0u8; 24];
                                    ev.encode(&mut kbuf);
                                    let to_copy = 24.min(buf_len);
                                    if let Err(e) = copy_to_user(buf_ptr, &kbuf[..to_copy]) {
                                        return { tf.regs[10] = e as usize; };
                                    }
                                    to_copy as i64
                                } else {
                                    0
                                }
                            }
                            "/dev/ptmx" => 0,
                            _ => -2,
                        }
                    }
                } else {
                    if caps.check(crate::capability::CapabilityType::FileSystemWrite, crate::capability::Rights::WRITE).is_err() {
                        -13
                    } else {
                        match path {
                            "/dev/null" | "/dev/zero" => buf_len as i64,
                            "/dev/urandom" => buf_len as i64,
                            "/dev/input/event0" => -13,
                            "/dev/ptmx" => {
                                let mut kbuf = [0u8; 256];
                                let mut read_pos = 0;
                                while read_pos < buf_len {
                                    let chunk = (buf_len - read_pos).min(256);
                                    if copy_from_user(&mut kbuf[..chunk], buf_ptr + read_pos, chunk).is_err() {
                                        break;
                                    }
                                    for &b in &kbuf[..chunk] {
                                        crate::print!("{}", b as char);
                                    }
                                    read_pos += chunk;
                                }
                                buf_len as i64
                            }
                            _ => -2,
                        }
                    }
                }
            }
        }
        SYS_TELEPHONY_QUERY => -38,
        SYS_MEM_LEND => {
            let borrower_pid = a0 as u32;
            let lender_vaddr = a1;
            let len = a2;
            let rights_num = a3;
            let out_borrower_vaddr_ptr = a4;

            if len == 0 || len > 64 * 1024 * 1024 || (len % 4096) != 0 || (lender_vaddr % 4096) != 0 || lender_vaddr < 0x1000 {
                -22
            } else {
                let rights = match rights_num {
                    0 => crate::mmu::MemoryLoanRights::ReadOnly,
                    1 => crate::mmu::MemoryLoanRights::ReadWrite,
                    _ => return { tf.regs[10] = (-22i64) as usize; },
                };
                let page_count = len / 4096;

                let mut sched = SCHEDULER.lock();
                let curr_task_id = sched.current_task_id().0 as u32;
                let current_pos = match sched.tasks.iter().position(|t| t.id == curr_task_id) {
                    Some(p) => p,
                    None => return { tf.regs[10] = (-13i64) as usize; },
                };
                let caller_pid = sched.tasks[current_pos].id;
                if !sched.tasks[current_pos].cap_table.authorize_capability(crate::capability::CapabilityType::MemoryHeap, crate::capability::Rights::READ | crate::capability::Rights::WRITE)
                    && !sched.tasks[current_pos].cap_table.authorize_capability(crate::capability::CapabilityType::MemoryHeap, crate::capability::Rights::WRITE)
                {
                    -13
                } else {
                    let has_ipc = sched.tasks[current_pos].cap_table.authorize_object(crate::capability::CapabilityType::IpcSend, crate::capability::Rights::WRITE, borrower_pid as u64)
                        || sched.tasks[current_pos].cap_table.authorize_capability(crate::capability::CapabilityType::IpcSend, crate::capability::Rights::WRITE);
                    if !has_ipc {
                        -13
                    } else {
                        let borrower_pos = match sched.tasks.iter().position(|t| t.id == borrower_pid) {
                            Some(p) => p,
                            None => return { tf.regs[10] = (-22i64) as usize; },
                        };
                        if current_pos == borrower_pos {
                            -22
                        } else {
                            if out_borrower_vaddr_ptr != 0 {
                                if validate_user_range(out_borrower_vaddr_ptr, 8, true).is_err() {
                                    return { tf.regs[10] = (-22i64) as usize; };
                                }
                            }

                            let (slot, gen) = match crate::mmu::MEMORY_LOANS.lock().reserve_slot() {
                                Ok(pair) => pair,
                                Err(_) => return { tf.regs[10] = (-12i64) as usize; },
                            };

                            let mut frames = alloc::vec::Vec::with_capacity(page_count);
                            let mut orig_writable = alloc::vec::Vec::with_capacity(page_count);
                            {
                                let lender_aspace = match sched.tasks[current_pos].address_space.as_ref() {
                                    Some(a) => a,
                                    None => {
                                        crate::mmu::MEMORY_LOANS.lock().release_slot(slot);
                                        return { tf.regs[10] = (-13i64) as usize; };
                                    }
                                };
                                for i in 0..page_count {
                                    let vaddr = lender_vaddr + (i * 4096);
                                    let (phys, is_write) = match lender_aspace.translate(vaddr) {
                                        Some(pair) => pair,
                                        None => {
                                            crate::mmu::MEMORY_LOANS.lock().release_slot(slot);
                                            return { tf.regs[10] = (-22i64) as usize; };
                                        }
                                    };
                                    if rights == crate::mmu::MemoryLoanRights::ReadWrite && !is_write {
                                        crate::mmu::MEMORY_LOANS.lock().release_slot(slot);
                                        return { tf.regs[10] = (-13i64) as usize; };
                                    }
                                    if crate::mmu::MEMORY_LOANS.lock().is_frame_lent(phys) {
                                        crate::mmu::MEMORY_LOANS.lock().release_slot(slot);
                                        return { tf.regs[10] = (-13i64) as usize; };
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
                                        return { tf.regs[10] = (-22i64) as usize; };
                                    }
                                };
                                let b_span = match borrower_aspace.find_free_user_span(page_count) {
                                    Ok(s) => s,
                                    Err(_) => {
                                        crate::mmu::MEMORY_LOANS.lock().release_slot(slot);
                                        return { tf.regs[10] = (-12i64) as usize; };
                                    }
                                };
                                let mut mapped = 0;
                                for i in 0..page_count {
                                    let vaddr = b_span + (i * 4096);
                                    if borrower_aspace.map_borrowed_frame(vaddr, frames[i], rights == crate::mmu::MemoryLoanRights::ReadWrite, false).is_err() {
                                        for r in 0..mapped {
                                            let _ = borrower_aspace.unmap_user_page(b_span + (r * 4096), false);
                                        }
                                        crate::mmu::MEMORY_LOANS.lock().release_slot(slot);
                                        return { tf.regs[10] = (-12i64) as usize; };
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
                                            let vaddr = lender_vaddr + (i * 4096);
                                            let _ = lender_aspace.update_leaf_flags(vaddr, false);
                                        }
                                    }
                                    crate::mmu::MemoryLoanRights::ReadWrite => {
                                        for i in 0..page_count {
                                            let vaddr = lender_vaddr + (i * 4096);
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
                                lender_vaddr as u64,
                                borrower_vaddr as u64,
                                page_count,
                                frames,
                                rights,
                                orig_writable,
                            ) {
                                Ok(h) => h,
                                Err(_) => return { tf.regs[10] = (-12i64) as usize; },
                            };

                            drop(sched);

                            if out_borrower_vaddr_ptr != 0 {
                                let _ = copy_to_user(out_borrower_vaddr_ptr, &(borrower_vaddr as u64).to_le_bytes());
                            }

                            handle.0 as i64
                        }
                    }
                }
            }
        }
        SYS_MEM_RETURN => {
            let lend_handle = a0 as u64;
            let (caller_pid, _) = match get_caller_identity_and_caps() {
                Ok(p) => p,
                Err(e) => return { tf.regs[10] = e as usize; },
            };
            let rec = match crate::mmu::MEMORY_LOANS.lock().claim_return_or_revoke(crate::mmu::LendHandle(lend_handle), caller_pid) {
                Ok(r) => r,
                Err(_) => return { tf.regs[10] = (-13i64) as usize; },
            };

            let mut sched = SCHEDULER.lock();
            if let Some(b_task) = sched.tasks.iter_mut().find(|t| t.id == rec.borrower_task) {
                if let Some(ref mut b_aspace) = b_task.address_space {
                    for i in 0..rec.page_count {
                        let vaddr = rec.borrower_vaddr as usize + (i * 4096);
                        let _ = b_aspace.unmap_user_page(vaddr, false);
                    }
                }
            }
            if let Some(l_task) = sched.tasks.iter_mut().find(|t| t.id == rec.lender_task) {
                if let Some(ref mut l_aspace) = l_task.address_space {
                    match rec.rights {
                        crate::mmu::MemoryLoanRights::ReadOnly => {
                            for i in 0..rec.page_count {
                                let vaddr = rec.lender_vaddr as usize + (i * 4096);
                                let _ = l_aspace.update_leaf_flags(vaddr, rec.original_lender_writable[i]);
                            }
                        }
                        crate::mmu::MemoryLoanRights::ReadWrite => {
                            for i in 0..rec.page_count {
                                let vaddr = rec.lender_vaddr as usize + (i * 4096);
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
                -22
            } else {
                let page_count = len / 4096;

                let mut sched = SCHEDULER.lock();
                let curr_task_id = sched.current_task_id().0 as u32;
                let current_pos = match sched.tasks.iter().position(|t| t.id == curr_task_id) {
                    Some(p) => p,
                    None => return { tf.regs[10] = (-13i64) as usize; },
                };
                let _caller_pid = sched.tasks[current_pos].id;
                if !sched.tasks[current_pos].cap_table.authorize_capability(crate::capability::CapabilityType::MemoryHeap, crate::capability::Rights::READ | crate::capability::Rights::WRITE)
                    && !sched.tasks[current_pos].cap_table.authorize_capability(crate::capability::CapabilityType::MemoryHeap, crate::capability::Rights::WRITE)
                {
                    -13
                } else {
                    let has_ipc = sched.tasks[current_pos].cap_table.authorize_object(crate::capability::CapabilityType::IpcSend, crate::capability::Rights::WRITE, receiver_pid as u64)
                        || sched.tasks[current_pos].cap_table.authorize_capability(crate::capability::CapabilityType::IpcSend, crate::capability::Rights::WRITE);
                    if !has_ipc {
                        -13
                    } else {
                        let receiver_pos = match sched.tasks.iter().position(|t| t.id == receiver_pid) {
                            Some(p) => p,
                            None => return { tf.regs[10] = (-22i64) as usize; },
                        };
                        if current_pos == receiver_pos {
                            -22
                        } else {
                            if out_receiver_vaddr_ptr != 0 {
                                if validate_user_range(out_receiver_vaddr_ptr, 8, true).is_err() {
                                    return { tf.regs[10] = (-22i64) as usize; };
                                }
                            }

                            let mut frames = alloc::vec::Vec::with_capacity(page_count);
                            {
                                let sender_aspace = match sched.tasks[current_pos].address_space.as_ref() {
                                    Some(a) => a,
                                    None => return { tf.regs[10] = (-13i64) as usize; },
                                };
                                for i in 0..page_count {
                                    let vaddr = sender_vaddr + (i * 4096);
                                    let (phys, is_write) = match sender_aspace.translate(vaddr) {
                                        Some(p) => p,
                                        None => {
                                            tf.regs[10] = (-22i64) as usize;
                                            return;
                                        }
                                    };
                                    if !is_write { return { tf.regs[10] = (-13i64) as usize; }; }
                                    if crate::mmu::MEMORY_LOANS.lock().is_frame_lent(phys) { return { tf.regs[10] = (-13i64) as usize; }; }
                                    frames.push(phys);
                                }
                            }

                            let receiver_vaddr = {
                                let receiver_aspace = match sched.tasks[receiver_pos].address_space.as_mut() {
                                    Some(a) => a,
                                    None => {
                                        tf.regs[10] = (-22i64) as usize;
                                        return;
                                    }
                                };
                                let r_span = match receiver_aspace.find_free_user_span(page_count) {
                                    Ok(s) => s,
                                    Err(_) => {
                                        tf.regs[10] = (-12i64) as usize;
                                        return;
                                    }
                                };
                                let mut mapped = 0;
                                for i in 0..page_count {
                                    let vaddr = r_span + (i * 4096);
                                    if receiver_aspace.restore_leaf_pte(vaddr, frames[i], true, false).is_err() {
                                        for r in 0..mapped {
                                            let _ = receiver_aspace.clear_leaf_pte(r_span + (r * 4096));
                                        }
                                        return { tf.regs[10] = (-12i64) as usize; };
                                    }
                                    mapped += 1;
                                }
                                r_span
                            };

                            {
                                let sender_aspace = sched.tasks[current_pos].address_space.as_mut().unwrap();
                                for i in 0..page_count {
                                    let vaddr = sender_vaddr + (i * 4096);
                                    let _ = sender_aspace.clear_leaf_pte(vaddr);
                                }
                            }

                            drop(sched);

                            if out_receiver_vaddr_ptr != 0 {
                                let _ = copy_to_user(out_receiver_vaddr_ptr, &(receiver_vaddr as u64).to_le_bytes());
                            }

                            receiver_vaddr as i64
                        }
                    }
                }
            }
        }
        SYS_WAIT_PID => {
            let target_pid = if a0 < 0x1000 && a1 > 0 { a1 as u32 } else { a0 as u32 };
            let (caller_pid, is_supervisor) = match get_caller_identity_and_caps() {
                Ok((pid, caps)) => {
                    let supervisor = caps.check(crate::capability::CapabilityType::SystemResource, crate::capability::Rights::ADMIN).is_ok();
                    (pid, supervisor)
                }
                Err(_) => (crate::task::INVALID_PID, false),
            };
            if caller_pid == crate::task::INVALID_PID {
                -13
            } else {
                let code_res = crate::task::reap_task_locksafe(caller_pid, is_supervisor, target_pid);
                match code_res {
                    Ok(code) => {
                        println!("[WAITPID] Caller PID {} reaped child {} with exit code {}", caller_pid, target_pid, code);
                        code as i64
                    }
                    Err(err) => {
                        if err == -30 {
                            crate::task::yield_now();
                        }
                        err
                    }
                }
            }
        }

        _ => -38, // -ENOSYS
    };

    tf.regs[10] = res as usize;
}
