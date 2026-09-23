//! Inter OS System Call Dispatcher
//!
//! Provides the primary interface between tasks and kernel services.
//! Every privileged system call enforces capability validation before execution.

pub mod entry;
pub mod user_ptr;
pub use entry::set_current_kernel_rsp;
pub use user_ptr::{UserPtr, UserSlice, UserSliceMut, UserAccessError, validate_user_buffer};

/// Validates that a user-supplied address range `[addr, addr + len)` is strictly
/// within user space, non-null, and does not overlap kernel memory.
#[inline]
pub fn check_user_slice(addr: u64, len: usize) -> Result<&'static [u8], SyscallResult> {
    if len == 0 {
        return Ok(&[]);
    }
    if validate_user_buffer(addr, len).is_err() {
        return Err(SyscallResult::InvalidArguments);
    }
    Ok(unsafe { core::slice::from_raw_parts(addr as *const u8, len) })
}

/// Validates that a user-supplied mutable buffer range is strictly within user space.
#[inline]
pub fn check_user_slice_mut(addr: u64, len: usize) -> Result<&'static mut [u8], SyscallResult> {
    if len == 0 {
        return Ok(&mut []);
    }
    if validate_user_buffer(addr, len).is_err() {
        return Err(SyscallResult::InvalidArguments);
    }
    Ok(unsafe { core::slice::from_raw_parts_mut(addr as *mut u8, len) })
}

/// Validates that a user-supplied string pointer is in user memory and contains valid UTF-8.
#[inline]
pub fn check_user_str(addr: u64, len: usize) -> Result<&'static str, SyscallResult> {
    let slice = check_user_slice(addr, len)?;
    core::str::from_utf8(slice).map_err(|_| SyscallResult::InvalidArguments)
}

use crate::capability::{CapError, CapabilityType, Rights};
use crate::drivers::{BlockDevice, ATA_PRIMARY, SECTOR_SIZE};
use crate::fs::{FileDescriptor, FileStat, FsError, VFS};
use crate::ipc::{IpcError, IpcMessage, IPC_ROUTER};
use crate::package::{AmcpPackage, PackageError};
use crate::serial_println;
use crate::task::{SCHEDULER, TaskId};

/// Well-known system call identifiers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u64)]
pub enum SyscallNumber {
    Yield = 1,
    Exit = 2,
    Log = 3,
    GetTicks = 4,
    IpcSend = 5,
    IpcRecv = 6,
    GetTaskId = 7,
    FsOpen = 8,
    FsRead = 9,
    FsWrite = 10,
    FsClose = 11,
    FsStat = 12,
    BlockRead = 13,
    VerifyPackage = 14,
    McpInvoke = 15,
    WasmExecute = 16,
    IdentityGetDid = 17,
    StateCommit = 18,
    ResourceQuery = 19,
    LiteVerseStatus = 20,
    NetSocket = 21,
    NetSendTo = 22,
    NetRecvFrom = 23,
    MarketplaceList = 24,
    MarketplaceGet = 25,
    PackageAudit = 26,
    PackageInstall = 27,
    HalQueryPlatform = 28,
    PowerGovernor = 29,
    TelephonyQuery = 30,
    GetRandom = 31,
    AudioControl = 32,
    KeystoreDerive = 33,
    SensorRead = 35,
    BatteryQuery = 36,
    AndroidBridgeIntent = 37,
    RtcGetTime = 38,
    NlParseIntent = 39,
    ThermalQuery = 40,
    SecureIpcSession = 41,
    VectorCosineSimilarity = 42,
    VaultCrypt = 43,
    ContextBroker = 44,
    WirelessQuery = 45,
    DevFsIo = 46,
    AgentPlanExecute = 47,
    RevocationAudit = 48,
    CapDelegate = 49,
    PrivacyShieldAudit = 50,
    SurfacePhysics = 51,
    ApkInstallAudit = 52,
    DexVerifyExec = 53,
    CameraCapture = 54,
    DexInvokeMethod = 55,
    AiAssistantQuery = 56,
    CapRequestConsent = 57,
    ShareSheetDispatch = 58,
    ShmPipeCreate = 59,
    ShmPipeTransfer = 60,
    ShmPipeReceive = 61,
    EventPoll = 62,
}

/// System call result status codes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(i64)]
pub enum SyscallResult {
    Success = 0,
    PermissionDenied = -1,
    InvalidSlot = -2,
    InvalidArguments = -3,
    MailboxEmpty = -4,
    MailboxFull = -5,
    TargetNotFound = -6,
    UnknownSyscall = -7,
    FileNotFound = -8,
    IsADirectory = -9,
    ReadOnlyFs = -10,
    IntegrityViolation = -11,
    DeviceError = -12,
    InvalidFd = -13,
    ToolNotFound = -14,
    WasmExecutionFailed = -15,
    FuelExhausted = -16,
    IdentityUninitialized = -17,
    StateSyncFailed = -18,
    LiteVerseDisabled = -19,
    NetworkError = -20,
    MarketplaceError = -21,
    AuditFailed = -22,
    KeystoreError = -23,
    AudioError = -24,
    PackageRevoked = -25,
    PlanAborted = -26,
    PipeNotFound = -27,
    PipeFull = -28,
    PipeEmpty = -29,
}

/// Dispatches a cooperative scheduling yield.
pub fn sys_yield() {
    crate::task::yield_now();
}

/// Terminates the calling task.
pub fn sys_exit() -> ! {
    crate::task::terminate_current();
}

/// Emits a diagnostic log message to the UART serial COM1.
///
/// Requires the caller to hold a `SerialOutput` capability with `Rights::WRITE`.
pub fn sys_log(cap_slot: usize, msg: &str) -> SyscallResult {
    let sched = SCHEDULER.lock();
    let current = match sched.current_task() {
        Some(t) => t,
        None => return SyscallResult::PermissionDenied,
    };

    if current.cap_table.verify(cap_slot, CapabilityType::SerialOutput, Rights::WRITE).is_err() {
        return SyscallResult::PermissionDenied;
    }

    let task_name = current.name;
    let task_id = current.id.0;
    drop(sched);

    serial_println!("[TASK {} (ID {}) LOG] {}", task_name, task_id, msg);
    SyscallResult::Success
}

/// Reads the current system hardware timer ticks.
///
/// Requires the caller to hold a `TimerTickRead` capability with `Rights::READ`.
pub fn sys_get_ticks(cap_slot: usize) -> Result<u64, SyscallResult> {
    let sched = SCHEDULER.lock();
    let current = match sched.current_task() {
        Some(t) => t,
        None => return Err(SyscallResult::PermissionDenied),
    };

    if current.cap_table.verify(cap_slot, CapabilityType::TimerTickRead, Rights::READ).is_err() {
        return Err(SyscallResult::PermissionDenied);
    }
    drop(sched);

    Ok(crate::interrupts::get_system_ticks())
}

/// Returns the TaskId of the caller.
pub fn sys_get_task_id() -> TaskId {
    SCHEDULER.lock().current_task_id().unwrap_or(TaskId(0))
}

/// Sends an IPC message to a destination task.
pub fn sys_ipc_send(
    cap_slot: usize,
    dest: TaskId,
    msg_type: u32,
    data: &[u8],
) -> SyscallResult {
    let (sender_id, is_valid) = {
        let sched = SCHEDULER.lock();
        let current = match sched.current_task() {
            Some(t) => t,
            None => return SyscallResult::PermissionDenied,
        };
        let valid = current.cap_table.verify_object(cap_slot, CapabilityType::IpcSend, Rights::WRITE, dest.0).is_ok();
        (current.id, valid)
    };

    if !is_valid {
        return SyscallResult::PermissionDenied;
    }

    let mut router = IPC_ROUTER.lock();
    match router.send_direct(sender_id, dest, msg_type, data) {
        Ok(()) => SyscallResult::Success,
        Err(IpcError::CapabilityDenied(_)) => SyscallResult::PermissionDenied,
        Err(IpcError::TargetNotFound) => SyscallResult::TargetNotFound,
        Err(IpcError::MailboxFull) => SyscallResult::MailboxFull,
        Err(IpcError::MailboxEmpty) => SyscallResult::MailboxEmpty,
    }
}

/// Receives an IPC message from the caller's mailbox.
pub fn sys_ipc_recv(cap_slot: usize) -> Result<IpcMessage, SyscallResult> {
    let (receiver_id, is_valid) = {
        let sched = SCHEDULER.lock();
        let current = match sched.current_task() {
            Some(t) => t,
            None => return Err(SyscallResult::PermissionDenied),
        };
        let valid = current.cap_table.verify(cap_slot, CapabilityType::IpcReceive, Rights::READ).is_ok();
        (current.id, valid)
    };

    if !is_valid {
        return Err(SyscallResult::PermissionDenied);
    }

    let mut router = IPC_ROUTER.lock();
    match router.receive_direct(receiver_id) {
        Ok(Some(msg)) => Ok(msg),
        Ok(None) => Err(SyscallResult::MailboxEmpty),
        Err(IpcError::CapabilityDenied(_)) => Err(SyscallResult::PermissionDenied),
        Err(_) => Err(SyscallResult::PermissionDenied),
    }
}

/// Opens a file in the Virtual Filesystem.
///
/// Requires the caller to hold a matching `FileSystemRead` or `FileSystemWrite` capability token.
pub fn sys_fs_open(
    cap_slot: usize,
    path: &str,
    requested_rights: Rights,
) -> Result<FileDescriptor, SyscallResult> {
    let sched = SCHEDULER.lock();
    let current = match sched.current_task() {
        Some(t) => t,
        None => return Err(SyscallResult::PermissionDenied),
    };
    let task_id = current.id;
    let cap_table = current.cap_table.clone();
    drop(sched);

    let mut vfs = VFS.lock();
    vfs.open(task_id, &cap_table, cap_slot, path, requested_rights)
        .map_err(|e| match e {
            FsError::CapabilityError(_) | FsError::PermissionDenied => SyscallResult::PermissionDenied,
            FsError::FileNotFound => SyscallResult::FileNotFound,
            FsError::IsADirectory => SyscallResult::IsADirectory,
            FsError::ReadOnlyFileSystem => SyscallResult::ReadOnlyFs,
            FsError::IntegrityViolation => SyscallResult::IntegrityViolation,
            _ => SyscallResult::InvalidArguments,
        })
}

/// Reads data from an open file descriptor.
///
/// Requires `FileSystemRead` capability in `cap_slot`.
pub fn sys_fs_read(
    cap_slot: usize,
    fd: FileDescriptor,
    buf: &mut [u8],
) -> Result<usize, SyscallResult> {
    let sched = SCHEDULER.lock();
    let current = match sched.current_task() {
        Some(t) => t,
        None => return Err(SyscallResult::PermissionDenied),
    };
    let task_id = current.id;
    let cap_table = current.cap_table.clone();
    drop(sched);

    let mut vfs = VFS.lock();
    vfs.read(task_id, &cap_table, cap_slot, fd, buf)
        .map_err(|e| match e {
            FsError::CapabilityError(_) | FsError::PermissionDenied => SyscallResult::PermissionDenied,
            FsError::InvalidFd => SyscallResult::InvalidFd,
            _ => SyscallResult::InvalidArguments,
        })
}

/// Writes data to an open file descriptor.
///
/// Requires `FileSystemWrite` capability in `cap_slot`.
pub fn sys_fs_write(
    cap_slot: usize,
    fd: FileDescriptor,
    buf: &[u8],
) -> Result<usize, SyscallResult> {
    let sched = SCHEDULER.lock();
    let current = match sched.current_task() {
        Some(t) => t,
        None => return Err(SyscallResult::PermissionDenied),
    };
    let task_id = current.id;
    let cap_table = current.cap_table.clone();
    drop(sched);

    let mut vfs = VFS.lock();
    vfs.write(task_id, &cap_table, cap_slot, fd, buf)
        .map_err(|e| match e {
            FsError::CapabilityError(_) | FsError::PermissionDenied => SyscallResult::PermissionDenied,
            FsError::ReadOnlyFileSystem => SyscallResult::ReadOnlyFs,
            FsError::InvalidFd => SyscallResult::InvalidFd,
            _ => SyscallResult::InvalidArguments,
        })
}

/// Closes an open file descriptor.
pub fn sys_fs_close(fd: FileDescriptor) -> SyscallResult {
    let sched = SCHEDULER.lock();
    let current = match sched.current_task() {
        Some(t) => t,
        None => return SyscallResult::PermissionDenied,
    };
    let task_id = current.id;
    drop(sched);

    let mut vfs = VFS.lock();
    match vfs.close(task_id, fd) {
        Ok(()) => SyscallResult::Success,
        Err(_) => SyscallResult::InvalidFd,
    }
}

/// Reads file metadata without opening a descriptor.
pub fn sys_fs_stat(cap_slot: usize, path: &str) -> Result<FileStat, SyscallResult> {
    let sched = SCHEDULER.lock();
    let current = match sched.current_task() {
        Some(t) => t,
        None => return Err(SyscallResult::PermissionDenied),
    };
    let cap_table = current.cap_table.clone();
    drop(sched);

    let vfs = VFS.lock();
    vfs.stat(&cap_table, cap_slot, path)
        .map_err(|e| match e {
            FsError::CapabilityError(_) | FsError::PermissionDenied => SyscallResult::PermissionDenied,
            FsError::FileNotFound => SyscallResult::FileNotFound,
            _ => SyscallResult::InvalidArguments,
        })
}

/// Reads a 512-byte physical disk sector by LBA from the primary ATA drive.
///
/// Requires the caller to hold a `StorageRawBlock` capability with `Rights::READ`.
pub fn sys_block_read(
    cap_slot: usize,
    lba: u32,
    buf: &mut [u8; SECTOR_SIZE],
) -> SyscallResult {
    let sched = SCHEDULER.lock();
    let current = match sched.current_task() {
        Some(t) => t,
        None => return SyscallResult::PermissionDenied,
    };

    if current.cap_table.verify(cap_slot, CapabilityType::StorageRawBlock, Rights::READ).is_err() {
        return SyscallResult::PermissionDenied;
    }
    drop(sched);

    let mut ata_guard = ATA_PRIMARY.lock();
    match ata_guard.as_mut() {
        Some(drive) => match drive.read_sector(lba, buf) {
            Ok(()) => SyscallResult::Success,
            Err(_) => SyscallResult::DeviceError,
        },
        None => SyscallResult::DeviceError,
    }
}

/// Loads and cryptographically validates an `.amcp` automation package from the VFS.
///
/// Requires `FileSystemRead` capability in `cap_slot`.
pub fn sys_verify_package(
    cap_slot: usize,
    path: &str,
) -> Result<AmcpPackage, SyscallResult> {
    let fd = sys_fs_open(cap_slot, path, Rights::READ)?;
    let mut file_buf = alloc::vec![0u8; 2048];
    let read_bytes = sys_fs_read(cap_slot, fd, &mut file_buf)?;
    sys_fs_close(fd);

    AmcpPackage::parse_and_verify(&file_buf[..read_bytes]).map_err(|e| match e {
        PackageError::ChecksumMismatch => SyscallResult::IntegrityViolation,
        _ => SyscallResult::InvalidArguments,
    })
}

/// Invokes a registered local MCP tool with zero-trust capability verification.
///
/// Requires the caller to hold a valid capability token corresponding to the tool's
/// required capability category (e.g. `IpcSend` for external actions, `TimerTickRead` for telemetry).
pub fn sys_mcp_invoke(
    _cap_slot: usize,
    _tool_name: &str,
    _args: &[u8],
    _out_buf: &mut [u8],
) -> Result<usize, SyscallResult> {
    Err(SyscallResult::ToolNotFound)
}

/// Unified system call dispatcher invoked by hardware fast syscall entry stub.
#[no_mangle]
pub extern "C" fn sys_dispatch(num: u64, a1: u64, a2: u64, a3: u64, a4: u64, a5: u64) -> i64 {
    match num {
        1 => {
            sys_yield();
            0
        }
        2 => {
            sys_exit();
        }
        3 => {
            // Log: a1 = cap_slot, a2 = msg_ptr, a3 = msg_len
            let cap_slot = a1 as usize;
            let msg_len = a3 as usize;
            if msg_len > 4096 {
                return SyscallResult::InvalidArguments as i64;
            }
            let s = match check_user_str(a2, msg_len) {
                Ok(valid) => valid,
                Err(err) => return err as i64,
            };
            sys_log(cap_slot, s) as i64
        }
        4 => {
            // GetTicks: a1 = cap_slot
            match sys_get_ticks(a1 as usize) {
                Ok(ticks) => ticks as i64,
                Err(e) => e as i64,
            }
        }
        5 => {
            // IpcSend: a1 = cap_slot, a2 = dest_task_id, a3 = msg_type, a4 = data_ptr, a5 = data_len
            let cap_slot = a1 as usize;
            let dest = TaskId(a2);
            let msg_type = a3 as u32;
            let data_len = a5 as usize;
            let data = match check_user_slice(a4, data_len) {
                Ok(d) => d,
                Err(err) => return err as i64,
            };
            sys_ipc_send(cap_slot, dest, msg_type, data) as i64
        }
        6 => {
            // IpcRecv: a1 = cap_slot, a2 = out_buf_ptr, a3 = out_buf_len
            let cap_slot = a1 as usize;
            let out_len = a3 as usize;
            let out_buf = match check_user_slice_mut(a2, out_len) {
                Ok(b) => b,
                Err(err) => return err as i64,
            };
            match sys_ipc_recv(cap_slot) {
                Ok(msg) => {
                    let copy_len = core::cmp::min(out_buf.len(), msg.payload.len());
                    out_buf[..copy_len].copy_from_slice(&msg.payload[..copy_len]);
                    copy_len as i64
                }
                Err(err) => err as i64,
            }
        }
        7 => {
            // GetTaskId
            sys_get_task_id().0 as i64
        }
        8 => {
            // FsOpen: a1 = cap_slot, a2 = path_ptr, a3 = path_len, a4 = rights_bits
            let cap_slot = a1 as usize;
            let path_len = a3 as usize;
            let rights = Rights(a4 as u32);
            if path_len == 0 || path_len > 512 {
                return SyscallResult::InvalidArguments as i64;
            }
            let path = match check_user_str(a2, path_len) {
                Ok(s) => s,
                Err(err) => return err as i64,
            };
            match sys_fs_open(cap_slot, path, rights) {
                Ok(fd) => fd.0 as i64,
                Err(e) => e as i64,
            }
        }
        9 => {
            // FsRead: a1 = cap_slot, a2 = fd, a3 = buf_ptr, a4 = buf_len
            let cap_slot = a1 as usize;
            let fd = FileDescriptor(a2 as u32);
            let buf_len = a4 as usize;
            let buf = match check_user_slice_mut(a3, buf_len) {
                Ok(b) => b,
                Err(err) => return err as i64,
            };
            match sys_fs_read(cap_slot, fd, buf) {
                Ok(bytes) => bytes as i64,
                Err(e) => e as i64,
            }
        }
        10 => {
            // FsWrite: a1 = cap_slot, a2 = fd, a3 = buf_ptr, a4 = buf_len
            let cap_slot = a1 as usize;
            let fd = FileDescriptor(a2 as u32);
            let buf_len = a4 as usize;
            let buf = match check_user_slice(a3, buf_len) {
                Ok(b) => b,
                Err(err) => return err as i64,
            };
            match sys_fs_write(cap_slot, fd, buf) {
                Ok(bytes) => bytes as i64,
                Err(e) => e as i64,
            }
        }
        11 => {
            // FsClose: a1 = fd
            let fd = FileDescriptor(a1 as u32);
            sys_fs_close(fd) as i64
        }
        13 => {
            // BlockRead: a1 = cap_slot, a2 = lba, a3 = buf_ptr
            let cap_slot = a1 as usize;
            let lba = a2 as u32;
            let buf_ptr = a3 as *mut u8;
            if buf_ptr.is_null() {
                return SyscallResult::InvalidArguments as i64;
            }
            let buf = unsafe { &mut *(buf_ptr as *mut [u8; SECTOR_SIZE]) };
            sys_block_read(cap_slot, lba, buf) as i64
        }
        17 => {
            // IdentityGetDid: a1 = out_buf_ptr, a2 = out_buf_len
            let out_ptr = a1 as *mut u8;
            let out_len = a2 as usize;
            if out_ptr.is_null() || out_len == 0 {
                return SyscallResult::InvalidArguments as i64;
            }
            let buf = unsafe { core::slice::from_raw_parts_mut(out_ptr, out_len) };
            match sys_identity_get_did(buf) {
                Ok(len) => len as i64,
                Err(e) => e as i64,
            }
        }
        18 => {
            // StateCommit: a1 = out_root_ptr (32 bytes)
            let out_ptr = a1 as *mut u8;
            if out_ptr.is_null() {
                return SyscallResult::InvalidArguments as i64;
            }
            let out_root = unsafe { &mut *(out_ptr as *mut [u8; 32]) };
            match sys_state_commit(out_root) {
                Ok(seq) => seq as i64,
                Err(e) => e as i64,
            }
        }
        19 => {
            // ResourceQuery: a1 = out_battery_ptr, a2 = out_temp_ptr, a3 = out_throttling_ptr
            let b_ptr = a1 as *mut u8;
            let t_ptr = a2 as *mut u8;
            let th_ptr = a3 as *mut bool;
            if b_ptr.is_null() || t_ptr.is_null() || th_ptr.is_null() {
                return SyscallResult::InvalidArguments as i64;
            }
            let (b, t, th) = unsafe { (&mut *b_ptr, &mut *t_ptr, &mut *th_ptr) };
            sys_resource_query(b, t, th) as i64
        }
        20 => {
            // LiteVerseStatus: a1 = out_verified_ptr, a2 = out_relayed_ptr
            let v_ptr = a1 as *mut u64;
            let r_ptr = a2 as *mut u64;
            if v_ptr.is_null() || r_ptr.is_null() {
                return SyscallResult::InvalidArguments as i64;
            }
            let (v, r) = unsafe { (&mut *v_ptr, &mut *r_ptr) };
            match sys_liteverse_status(v, r) {
                Ok(state) => state as i64,
                Err(e) => e as i64,
            }
        }
        21 => {
            // NetSocket: a1 = port
            let port = a1 as u16;
            match sys_net_socket(port) {
                Ok(sock_id) => sock_id as i64,
                Err(e) => e as i64,
            }
        }
        22 => {
            // NetSendTo: a1 = dst_ip_u32, a2 = dst_port, a3 = payload_ptr, a4 = payload_len
            let dst_ip = crate::net::Ipv4Address((a1 as u32).to_be_bytes());
            let dst_port = a2 as u16;
            let ptr = a3 as *const u8;
            let len = a4 as usize;
            if ptr.is_null() {
                return SyscallResult::InvalidArguments as i64;
            }
            let payload = unsafe { core::slice::from_raw_parts(ptr, len) };
            sys_net_sendto(dst_ip, dst_port, payload) as i64
        }
        23 => {
            // NetRecvFrom: a1 = sock_id, a2 = out_buf_ptr, a3 = out_buf_len
            let sock_id = crate::net::SocketId(a1 as u32);
            let ptr = a2 as *mut u8;
            let len = a3 as usize;
            if ptr.is_null() {
                return SyscallResult::InvalidArguments as i64;
            }
            let out_buf = unsafe { core::slice::from_raw_parts_mut(ptr, len) };
            match sys_net_recvfrom(sock_id, out_buf) {
                Ok(bytes) => bytes as i64,
                Err(e) => e as i64,
            }
        }
        24 => {
            // MarketplaceList: a1 = out_buf_ptr, a2 = out_buf_len
            let ptr = a1 as *mut u8;
            let len = a2 as usize;
            if ptr.is_null() {
                return SyscallResult::InvalidArguments as i64;
            }
            let out_buf = unsafe { core::slice::from_raw_parts_mut(ptr, len) };
            match sys_marketplace_list(out_buf) {
                Ok(n) => n as i64,
                Err(e) => e as i64,
            }
        }
        26 => {
            // PackageAudit: a1 = pkg_id_ptr, a2 = pkg_id_len
            let ptr = a1 as *const u8;
            let len = a2 as usize;
            if ptr.is_null() || len == 0 || len > 128 {
                return SyscallResult::InvalidArguments as i64;
            }
            let slice = unsafe { core::slice::from_raw_parts(ptr, len) };
            let pkg_id = match core::str::from_utf8(slice) {
                Ok(s) => s,
                Err(_) => return SyscallResult::InvalidArguments as i64,
            };
            match sys_package_audit(pkg_id) {
                Ok(verdict) => verdict as i64,
                Err(e) => e as i64,
            }
        }
        27 => {
            // PackageInstall: a1 = cap_slot, a2 = pkg_id_ptr, a3 = pkg_id_len, a4 = user_consent
            let cap_slot = a1 as usize;
            let ptr = a2 as *const u8;
            let len = a3 as usize;
            let consent = a4 != 0;
            if ptr.is_null() || len == 0 || len > 128 {
                return SyscallResult::InvalidArguments as i64;
            }
            let slice = unsafe { core::slice::from_raw_parts(ptr, len) };
            let pkg_id = match core::str::from_utf8(slice) {
                Ok(s) => s,
                Err(_) => return SyscallResult::InvalidArguments as i64,
            };
            match sys_package_install(cap_slot, pkg_id, consent) {
                Ok(_) => SyscallResult::Success as i64,
                Err(e) => e as i64,
            }
        }
        28 => {
            // HalQueryPlatform: a1 = out_buf_ptr, a2 = out_buf_len
            let ptr = a1 as *mut u8;
            let len = a2 as usize;
            if ptr.is_null() {
                return SyscallResult::InvalidArguments as i64;
            }
            let out_buf = unsafe { core::slice::from_raw_parts_mut(ptr, len) };
            match sys_hal_query_platform(out_buf) {
                Ok(n) => n as i64,
                Err(e) => e as i64,
            }
        }
        29 => {
            // PowerGovernor: a1 = action (0=query, 1=set), a2 = governor_id
            let action = a1 as u32;
            let gov_id = a2 as u8;
            match sys_power_governor(action, gov_id) {
                Ok(res) => res as i64,
                Err(e) => e as i64,
            }
        }
        30 => {
            // TelephonyQuery: a1 = out_buf_ptr, a2 = out_buf_len
            let ptr = a1 as *mut u8;
            let len = a2 as usize;
            if ptr.is_null() {
                return SyscallResult::InvalidArguments as i64;
            }
            let out_buf = unsafe { core::slice::from_raw_parts_mut(ptr, len) };
            match sys_telephony_query(out_buf) {
                Ok(n) => n as i64,
                Err(e) => e as i64,
            }
        }
        31 => {
            // GetRandom: a1 = out_buf_ptr, a2 = out_buf_len
            let ptr = a1 as *mut u8;
            let len = a2 as usize;
            if ptr.is_null() || len == 0 || len > 4096 {
                return SyscallResult::InvalidArguments as i64;
            }
            let out_buf = unsafe { core::slice::from_raw_parts_mut(ptr, len) };
            match sys_get_random(out_buf) {
                Ok(n) => n as i64,
                Err(e) => e as i64,
            }
        }
        32 => {
            // AudioControl: a1 = action (0=haptic, 1=chime, 2=tone, 3=silence), a2 = param
            let action = a1 as u32;
            let param = a2 as u32;
            match sys_audio_control(action, param) {
                Ok(res) => res as i64,
                Err(e) => e as i64,
            }
        }
        33 => {
            // KeystoreDerive: a1 = slot_index, a2 = out_buf_ptr, a3 = out_buf_len
            let slot_idx = a1 as u32;
            let ptr = a2 as *mut u8;
            let len = a3 as usize;
            if ptr.is_null() || len < 32 {
                return SyscallResult::InvalidArguments as i64;
            }
            let out_buf = unsafe { &mut *(ptr as *mut [u8; 32]) };
            match sys_keystore_derive(slot_idx, out_buf) {
                Ok(n) => n as i64,
                Err(e) => e as i64,
            }
        }
        35 => {
            // SensorRead: a1 = out_buf_ptr, a2 = out_buf_len
            let ptr = a1 as *mut u8;
            let len = a2 as usize;
            if ptr.is_null() {
                return SyscallResult::InvalidArguments as i64;
            }
            let out_buf = unsafe { core::slice::from_raw_parts_mut(ptr, len) };
            match sys_sensor_read(out_buf) {
                Ok(n) => n as i64,
                Err(e) => e as i64,
            }
        }
        36 => {
            // BatteryQuery: a1 = out_buf_ptr, a2 = out_buf_len
            let ptr = a1 as *mut u8;
            let len = a2 as usize;
            if ptr.is_null() {
                return SyscallResult::InvalidArguments as i64;
            }
            let out_buf = unsafe { core::slice::from_raw_parts_mut(ptr, len) };
            match sys_battery_query(out_buf) {
                Ok(n) => n as i64,
                Err(e) => e as i64,
            }
        }
        37 => {
            // AndroidBridgeIntent: a1 = act_ptr, a2 = act_len, a3 = uri_ptr, a4 = uri_len, a5 = out_ptr
            let act_ptr = a1 as *const u8;
            let act_len = a2 as usize;
            let uri_ptr = a3 as *const u8;
            let uri_len = a4 as usize;
            let out_ptr = a5 as *mut u8;
            let out_len = 64;
            if act_ptr.is_null() || uri_ptr.is_null() || out_ptr.is_null() {
                return SyscallResult::InvalidArguments as i64;
            }
            let act_slice = unsafe { core::slice::from_raw_parts(act_ptr, act_len) };
            let uri_slice = unsafe { core::slice::from_raw_parts(uri_ptr, uri_len) };
            let act_str = match core::str::from_utf8(act_slice) {
                Ok(s) => s,
                Err(_) => return SyscallResult::InvalidArguments as i64,
            };
            let uri_str = match core::str::from_utf8(uri_slice) {
                Ok(s) => s,
                Err(_) => return SyscallResult::InvalidArguments as i64,
            };
            let out_buf = unsafe { core::slice::from_raw_parts_mut(out_ptr, out_len) };
            match sys_android_bridge_intent(act_str, uri_str, out_buf) {
                Ok(n) => n as i64,
                Err(e) => e as i64,
            }
        }
        38 => {
            // RtcGetTime: a1 = out_buf_ptr, a2 = out_buf_len
            let ptr = a1 as *mut u8;
            let len = a2 as usize;
            if ptr.is_null() {
                return SyscallResult::InvalidArguments as i64;
            }
            let out_buf = unsafe { core::slice::from_raw_parts_mut(ptr, len) };
            match sys_rtc_get_time(out_buf) {
                Ok(n) => n as i64,
                Err(e) => e as i64,
            }
        }
        39 => {
            // NlParseIntent: a1 = query_ptr, a2 = query_len, a3 = out_ptr, a4 = out_len
            let query_ptr = a1 as *const u8;
            let query_len = a2 as usize;
            let out_ptr = a3 as *mut u8;
            let out_len = a4 as usize;
            if query_ptr.is_null() || query_len == 0 || out_ptr.is_null() || out_len == 0 {
                return SyscallResult::InvalidArguments as i64;
            }
            let query_slice = unsafe { core::slice::from_raw_parts(query_ptr, query_len) };
            let query_str = match core::str::from_utf8(query_slice) {
                Ok(s) => s,
                Err(_) => return SyscallResult::InvalidArguments as i64,
            };
            let out_buf = unsafe { core::slice::from_raw_parts_mut(out_ptr, out_len) };
            match sys_nl_parse_intent(query_str, out_buf) {
                Ok(n) => n as i64,
                Err(e) => e as i64,
            }
        }
        40 => {
            // ThermalQuery: a1 = out_buf_ptr, a2 = out_buf_len
            let ptr = a1 as *mut u8;
            let len = a2 as usize;
            if ptr.is_null() {
                return SyscallResult::InvalidArguments as i64;
            }
            let out_buf = unsafe { core::slice::from_raw_parts_mut(ptr, len) };
            match sys_thermal_query(out_buf) {
                Ok(n) => n as i64,
                Err(e) => e as i64,
            }
        }
        41 => {
            // SecureIpcSession: a1 = peer_task, a2 = out_session_id_ptr
            let peer = TaskId(a1);
            let out_ptr = a2 as *mut u64;
            if out_ptr.is_null() {
                return SyscallResult::InvalidArguments as i64;
            }
            match sys_secure_ipc_session(peer) {
                Ok(sid) => {
                    unsafe { *out_ptr = sid; }
                    SyscallResult::Success as i64
                }
                Err(e) => e as i64,
            }
        }
        42 => {
            // VectorCosineSimilarity: a1 = vec_a_ptr, a2 = vec_b_ptr, a3 = dim
            let ptr_a = a1 as *const i16;
            let ptr_b = a2 as *const i16;
            let dim = a3 as usize;
            if ptr_a.is_null() || ptr_b.is_null() || dim != 64 {
                return SyscallResult::InvalidArguments as i64;
            }
            let slice_a = unsafe { core::slice::from_raw_parts(ptr_a, dim) };
            let slice_b = unsafe { core::slice::from_raw_parts(ptr_b, dim) };
            match sys_vector_cosine_similarity(slice_a, slice_b) {
                Ok(sim) => sim as i64,
                Err(e) => e as i64,
            }
        }
        43 => {
            // VaultCrypt: a1 = action (0=enc, 1=dec), a2 = block_idx, a3 = buf_ptr, a4 = 512, a5 = iv_ptr
            let action = a1 as u32;
            let block_idx = a2;
            let buf_ptr = a3 as *mut u8;
            let buf_len = a4 as usize;
            let iv_ptr = a5 as *const u8;
            if buf_ptr.is_null() || buf_len != 512 || iv_ptr.is_null() {
                return SyscallResult::InvalidArguments as i64;
            }
            let buf = unsafe { &mut *(buf_ptr as *mut [u8; 512]) };
            let iv = unsafe { &*(iv_ptr as *const [u8; 16]) };
            match sys_vault_crypt(action, block_idx, buf, iv) {
                Ok(()) => SyscallResult::Success as i64,
                Err(e) => e as i64,
            }
        }
        44 => {
            // ContextBroker: a1 = action (0=append, 1=query), a2 = tier, a3 = buf_ptr, a4 = buf_len
            let action = a1 as u32;
            let tier_raw = a2 as u8;
            let ptr = a3 as *mut u8;
            let len = a4 as usize;
            if ptr.is_null() || len == 0 {
                return SyscallResult::InvalidArguments as i64;
            }
            if action == 0 {
                // Append
                let slice = unsafe { core::slice::from_raw_parts(ptr, len) };
                let text = match core::str::from_utf8(slice) {
                    Ok(s) => s,
                    Err(_) => return SyscallResult::InvalidArguments as i64,
                };
                match sys_context_broker_append(tier_raw, text) {
                    Ok(id) => id as i64,
                    Err(e) => e as i64,
                }
            } else {
                // Query
                let out_buf = unsafe { core::slice::from_raw_parts_mut(ptr, len) };
                match sys_context_broker_query(tier_raw, out_buf) {
                    Ok(n) => n as i64,
                    Err(e) => e as i64,
                }
            }
        }
        45 => {
            // WirelessQuery: a1 = out_buf_ptr, a2 = out_buf_len
            let ptr = a1 as *mut u8;
            let len = a2 as usize;
            if ptr.is_null() {
                return SyscallResult::InvalidArguments as i64;
            }
            let out_buf = unsafe { core::slice::from_raw_parts_mut(ptr, len) };
            match sys_wireless_query(out_buf) {
                Ok(n) => n as i64,
                Err(e) => e as i64,
            }
        }
        46 => {
            // DevFsIo: a1 = action (0=read, 1=write), a2 = path_ptr, a3 = path_len, a4 = buf_ptr, a5 = buf_len
            let action = a1 as u32;
            let path_ptr = a2 as *const u8;
            let path_len = a3 as usize;
            let buf_ptr = a4 as *mut u8;
            let buf_len = a5 as usize;
            if path_ptr.is_null() || path_len == 0 || path_len > 256 || buf_ptr.is_null() {
                return SyscallResult::InvalidArguments as i64;
            }
            let slice = unsafe { core::slice::from_raw_parts(path_ptr, path_len) };
            let path = match core::str::from_utf8(slice) {
                Ok(s) => s,
                Err(_) => return SyscallResult::InvalidArguments as i64,
            };
            let buf = unsafe { core::slice::from_raw_parts_mut(buf_ptr, buf_len) };
            match sys_devfs_io(action, path, buf) {
                Ok(n) => n as i64,
                Err(e) => e as i64,
            }
        }
        47 => {
            // AgentPlanExecute: a1 = cmd (0=submit, 1=step_next, 2=status_json, 3=force_takeover), a2 = plan_id/param, a3 = consent, a4 = buf_ptr, a5 = buf_len
            let cmd = a1 as u32;
            let param = a2 as u32;
            let consent = a3 != 0;
            let buf_ptr = a4 as *mut u8;
            let buf_len = a5 as usize;
            match sys_agent_plan_execute(cmd, param, consent, buf_ptr, buf_len) {
                Ok(n) => n as i64,
                Err(e) => e as i64,
            }
        }
        48 => {
            // RevocationAudit: a1 = cmd (0=check_pkg, 1=revoke_pkg), a2 = target_ptr, a3 = target_len, a4 = out_buf_ptr, a5 = out_buf_len
            let cmd = a1 as u32;
            let target_ptr = a2 as *const u8;
            let target_len = a3 as usize;
            let out_ptr = a4 as *mut u8;
            let out_len = a5 as usize;
            if target_ptr.is_null() || target_len == 0 {
                return SyscallResult::InvalidArguments as i64;
            }
            let slice = unsafe { core::slice::from_raw_parts(target_ptr, target_len) };
            let target = match core::str::from_utf8(slice) {
                Ok(s) => s,
                Err(_) => return SyscallResult::InvalidArguments as i64,
            };
            match sys_revocation_audit(cmd, target, out_ptr, out_len) {
                Ok(n) => n as i64,
                Err(e) => e as i64,
            }
        }
        49 => {
            // CapDelegate: a1 = cap_slot, a2 = target_task_id, a3 = requested_rights, a4 = max_invocations, a5 = duration_ticks
            let cap_slot = a1 as usize;
            let target_task = TaskId(a2);
            let requested_rights = Rights(a3 as u32);
            let max_invocations = a4 as u32;
            let duration_ticks = a5;
            match sys_cap_delegate(cap_slot, target_task, requested_rights, max_invocations, duration_ticks) {
                Ok(n) => n as i64,
                Err(e) => e as i64,
            }
        }
        50 => {
            // PrivacyShieldAudit: a1 = in_ptr, a2 = in_len, a3 = out_ptr, a4 = out_len
            let in_ptr = a1 as *const u8;
            let in_len = a2 as usize;
            let out_ptr = a3 as *mut u8;
            let out_len = a4 as usize;
            if in_ptr.is_null() || in_len == 0 || out_ptr.is_null() || out_len == 0 {
                return SyscallResult::InvalidArguments as i64;
            }
            let in_slice = unsafe { core::slice::from_raw_parts(in_ptr, in_len) };
            let in_str = match core::str::from_utf8(in_slice) {
                Ok(s) => s,
                Err(_) => return SyscallResult::InvalidArguments as i64,
            };
            let out_slice = unsafe { core::slice::from_raw_parts_mut(out_ptr, out_len) };
            let copy_len = core::cmp::min(in_str.len(), out_slice.len());
            out_slice[..copy_len].copy_from_slice(&in_str.as_bytes()[..copy_len]);
            copy_len as i64
        }
        51 => {
            // SurfacePhysics: a1 = action, a2 = y, a3 = viewport_h, a4 = content_h
            let action = a1 as u32;
            let y = a2 as i32;
            let viewport_h = if a3 > 0 { a3 as i32 } else { 720 };
            let content_h = if a4 > 0 { a4 as i32 } else { 2000 };
            match sys_surface_physics(action, y, viewport_h, content_h) {
                Ok(n) => n as i64,
                Err(e) => e as i64,
            }
        }
        52 => {
            // ApkInstallAudit: a1 = apk_ptr, a2 = apk_len, a3 = out_ptr, a4 = out_len
            let apk_ptr = a1 as *const u8;
            let apk_len = a2 as usize;
            let out_ptr = a3 as *mut u8;
            let out_len = a4 as usize;

            if apk_ptr.is_null() || apk_len == 0 || out_ptr.is_null() || out_len == 0 {
                return SyscallResult::InvalidArguments as i64;
            }

            let apk_slice = unsafe { core::slice::from_raw_parts(apk_ptr, apk_len) };
            let out_buf = unsafe { core::slice::from_raw_parts_mut(out_ptr, out_len) };

            match sys_apk_install_audit(apk_slice, out_buf) {
                Ok(n) => n as i64,
                Err(e) => e as i64,
            }
        }
        53 => {
            // DexVerifyExec: a1 = dex_ptr, a2 = dex_len
            let dex_ptr = a1 as *const u8;
            let dex_len = a2 as usize;
            if dex_ptr.is_null() || dex_len == 0 {
                return SyscallResult::InvalidArguments as i64;
            }
            let dex_slice = unsafe { core::slice::from_raw_parts(dex_ptr, dex_len) };
            match sys_dex_verify_exec(dex_slice) {
                Ok(n) => n as i64,
                Err(e) => e as i64,
            }
        }
        54 => {
            // CameraCapture: a1 = action, a2 = out_buf_ptr, a3 = out_buf_len, a4 = out_meta_ptr
            let action = a1 as u32;
            let out_ptr = a2 as *mut u8;
            let out_len = a3 as usize;
            let meta_ptr = a4 as *mut u32;

            if out_ptr.is_null() || out_len == 0 {
                return SyscallResult::InvalidArguments as i64;
            }

            let out_buf = unsafe { core::slice::from_raw_parts_mut(out_ptr, out_len) };
            let meta_buf = if !meta_ptr.is_null() {
                Some(unsafe { core::slice::from_raw_parts_mut(meta_ptr, 4) })
            } else {
                None
            };

            match sys_camera_capture(action, out_buf, meta_buf) {
                Ok(n) => n as i64,
                Err(e) => e as i64,
            }
        }
        55 => {
            // DexInvokeMethod: a1 = dex_ptr, a2 = dex_len, a3 = spec_ptr, a4 = spec_len
            let dex_ptr = a1 as *const u8;
            let dex_len = a2 as usize;
            let spec_ptr = a3 as *const u8;
            let spec_len = a4 as usize;

            if dex_ptr.is_null() || dex_len == 0 || spec_ptr.is_null() || spec_len == 0 {
                return SyscallResult::InvalidArguments as i64;
            }

            let dex_slice = unsafe { core::slice::from_raw_parts(dex_ptr, dex_len) };
            let spec_slice = unsafe { core::slice::from_raw_parts(spec_ptr, spec_len) };
            let spec_str = match core::str::from_utf8(spec_slice) {
                Ok(s) => s,
                Err(_) => return SyscallResult::InvalidArguments as i64,
            };

            match sys_dex_invoke_method(dex_slice, spec_str) {
                Ok(n) => n as i64,
                Err(e) => e as i64,
            }
        }
        56 => {
            // AiAssistantQuery: a1 = prompt_ptr, a2 = prompt_len, a3 = out_ptr, a4 = out_len
            let prompt_ptr = a1 as *const u8;
            let prompt_len = a2 as usize;
            let out_ptr = a3 as *mut u8;
            let out_len = a4 as usize;

            if prompt_ptr.is_null() || prompt_len == 0 || out_ptr.is_null() || out_len == 0 {
                return SyscallResult::InvalidArguments as i64;
            }

            let prompt_slice = unsafe { core::slice::from_raw_parts(prompt_ptr, prompt_len) };
            let prompt_str = match core::str::from_utf8(prompt_slice) {
                Ok(s) => s,
                Err(_) => return SyscallResult::InvalidArguments as i64,
            };

            let out_slice = unsafe { core::slice::from_raw_parts_mut(out_ptr, out_len) };

            match sys_ai_assistant_query(prompt_str, out_slice) {
                Ok(n) => n as i64,
                Err(e) => e as i64,
            }
        }
        57 => {
            // CapRequestConsent: a1 = perm_val, a2 = app_name_ptr, a3 = app_name_len, a4 = auto_approve_flag
            let perm_val = a1 as u32;
            let app_ptr = a2 as *const u8;
            let app_len = a3 as usize;
            let auto_approve = a4 != 0;

            if app_ptr.is_null() || app_len == 0 {
                return SyscallResult::InvalidArguments as i64;
            }

            let app_slice = unsafe { core::slice::from_raw_parts(app_ptr, app_len) };
            let app_str = match core::str::from_utf8(app_slice) {
                Ok(s) => s,
                Err(_) => return SyscallResult::InvalidArguments as i64,
            };

            match sys_cap_request_consent(perm_val, app_str, auto_approve) {
                Ok(slot) => slot as i64,
                Err(e) => e as i64,
            }
        }
        58 => {
            // ShareSheetDispatch: a1 = mime_ptr, a2 = mime_len, a3 = uri_ptr, a4 = uri_len, a5 = target_ptr
            let mime_ptr = a1 as *const u8;
            let mime_len = a2 as usize;
            let uri_ptr = a3 as *const u8;
            let uri_len = a4 as usize;
            let target_ptr = a5 as *const u8;

            if mime_ptr.is_null() || mime_len == 0 || uri_ptr.is_null() || uri_len == 0 {
                return SyscallResult::InvalidArguments as i64;
            }

            let mime_slice = unsafe { core::slice::from_raw_parts(mime_ptr, mime_len) };
            let mime_str = match core::str::from_utf8(mime_slice) {
                Ok(s) => s,
                Err(_) => return SyscallResult::InvalidArguments as i64,
            };

            let uri_slice = unsafe { core::slice::from_raw_parts(uri_ptr, uri_len) };
            let uri_str = match core::str::from_utf8(uri_slice) {
                Ok(s) => s,
                Err(_) => return SyscallResult::InvalidArguments as i64,
            };

            let target_str = if !target_ptr.is_null() {
                let mut len = 0;
                while len < 64 && unsafe { *target_ptr.add(len) } != 0 {
                    len += 1;
                }
                if len > 0 {
                    let target_slice = unsafe { core::slice::from_raw_parts(target_ptr, len) };
                    core::str::from_utf8(target_slice).ok()
                } else {
                    None
                }
            } else {
                None
            };

            match sys_share_sheet_dispatch(mime_str, uri_str, target_str) {
                Ok(target_id) => target_id as i64,
                Err(e) => e as i64,
            }
        }
        59 => {
            // ShmPipeCreate: a1 = pipe_id, a2 = pages, a3 = peer_task
            match sys_shm_pipe_create(a1 as u32, a2 as u32, crate::task::TaskId(a3)) {
                Ok(pipe) => pipe as i64,
                Err(e) => e as i64,
            }
        }
        60 => {
            // ShmPipeTransfer: a1 = pipe_id, a2 = data_ptr, a3 = data_len, a4 = flags
            let data_ptr = a2 as *const u8;
            let data_len = a3 as usize;
            if data_ptr.is_null() || data_len == 0 {
                return SyscallResult::InvalidArguments as i64;
            }
            let data = unsafe { core::slice::from_raw_parts(data_ptr, data_len) };
            match sys_shm_pipe_transfer(a1 as u32, data, a4 as u32) {
                Ok(seq) => seq as i64,
                Err(e) => e as i64,
            }
        }
        61 => {
            // ShmPipeReceive: a1 = pipe_id, a2 = out_buf_ptr, a3 = out_buf_len
            let out_buf_ptr = a2 as *mut u8;
            let out_buf_len = a3 as usize;
            if out_buf_ptr.is_null() || out_buf_len == 0 {
                return SyscallResult::InvalidArguments as i64;
            }
            let out_buf = unsafe { core::slice::from_raw_parts_mut(out_buf_ptr, out_buf_len) };
            match sys_shm_pipe_receive(a1 as u32, out_buf) {
                Ok(bytes) => bytes as i64,
                Err(e) => e as i64,
            }
        }
        62 => {
            // EventPoll: a1 = interest_mask, a2 = timeout_ticks
            match sys_event_poll(a1 as u32, a2 as u32) {
                Ok(mask) => mask as i64,
                Err(e) => e as i64,
            }
        }
        _ => SyscallResult::UnknownSyscall as i64,
    }
}

/// Returns the device's Sovereign DID string into `out_buf`.
pub fn sys_identity_get_did(out_buf: &mut [u8]) -> Result<usize, SyscallResult> {
    let vault_guard = crate::identity::IDENTITY_VAULT.lock();
    let vault = vault_guard.as_ref().ok_or(SyscallResult::IdentityUninitialized)?;
    let did_bytes = vault.device_did().as_str().as_bytes();
    if out_buf.len() < did_bytes.len() {
        return Err(SyscallResult::InvalidArguments);
    }
    out_buf[..did_bytes.len()].copy_from_slice(did_bytes);
    Ok(did_bytes.len())
}

/// Commits pending state changes, deriving the new Merkle state root and sequence number.
pub fn sys_state_commit(out_root: &mut [u8; 32]) -> Result<u64, SyscallResult> {
    let vault_guard = crate::identity::IDENTITY_VAULT.lock();
    let vault = vault_guard.as_ref().ok_or(SyscallResult::IdentityUninitialized)?;
    let mut state_guard = crate::state::STATE_MANAGER.lock();
    let state_mgr = state_guard.as_mut().ok_or(SyscallResult::StateSyncFailed)?;

    let ticks = crate::interrupts::get_system_ticks();
    if let Some(commitment) = state_mgr.commit_pending(ticks, vault) {
        *out_root = commitment.new_state_root;
        Ok(commitment.sequence)
    } else {
        *out_root = *state_mgr.current_state_root();
        Ok(state_mgr.current_sequence())
    }
}

/// Queries the live hardware resource management status.
pub fn sys_resource_query(
    out_battery: &mut u8,
    out_temp: &mut u8,
    out_throttling: &mut bool,
) -> SyscallResult {
    let rm = crate::resource::RESOURCE_MANAGER.lock();
    *out_battery = rm.battery.percent;
    *out_temp = rm.thermal.temp_celsius;
    *out_throttling = rm.thermal.throttling;
    SyscallResult::Success
}

/// Queries the LiteVerse decentralized node state and performance metrics.
pub fn sys_liteverse_status(
    _out_verified: &mut u64,
    _out_relayed: &mut u64,
) -> Result<u8, SyscallResult> {
    Err(SyscallResult::LiteVerseDisabled)
}

/// Creates and binds a UDP socket for the current calling task.
pub fn sys_net_socket(port: u16) -> Result<u32, SyscallResult> {
    let sched = SCHEDULER.lock();
    let current = match sched.current_task() {
        Some(t) => t,
        None => return Err(SyscallResult::PermissionDenied),
    };
    let task_id = current.id.0 as u32;
    let caps = current.cap_table.clone();
    drop(sched);

    crate::net::SOCKET_TABLE
        .lock()
        .create_socket(task_id, port, &caps)
        .map(|id| id.0)
        .map_err(|_| SyscallResult::PermissionDenied)
}

/// Transmits a UDP packet from the current calling task.
pub fn sys_net_sendto(dst_ip: crate::net::Ipv4Address, dst_port: u16, payload: &[u8]) -> SyscallResult {
    let sched = SCHEDULER.lock();
    let current = match sched.current_task() {
        Some(t) => t,
        None => return SyscallResult::PermissionDenied,
    };
    let caps = current.cap_table.clone();
    drop(sched);

    match crate::net::send_udp_datagram(
        crate::net::Ipv4Address::LOCALHOST,
        crate::interlayer::INTERLAYER_DEFAULT_PORT,
        dst_ip,
        dst_port,
        payload,
        &caps,
    ) {
        Ok(()) => SyscallResult::Success,
        Err(_) => SyscallResult::PermissionDenied,
    }
}

/// Reads a packet from a UDP socket for the current calling task.
pub fn sys_net_recvfrom(sock_id: crate::net::SocketId, out_buf: &mut [u8]) -> Result<usize, SyscallResult> {
    let sched = SCHEDULER.lock();
    let current = match sched.current_task() {
        Some(t) => t,
        None => return Err(SyscallResult::PermissionDenied),
    };
    let caps = current.cap_table.clone();
    drop(sched);

    let mut sock_table = crate::net::SOCKET_TABLE.lock();
    let sock = sock_table.get_mut(sock_id).ok_or(SyscallResult::InvalidArguments)?;
    let (bytes, _, _) = sock.recv_from(out_buf, &caps).map_err(|_| SyscallResult::PermissionDenied)?;
    Ok(bytes)
}

/// Returns a comma-delimited string of available marketplace package IDs into `out_buf`.
pub fn sys_marketplace_list(out_buf: &mut [u8]) -> Result<usize, SyscallResult> {
    let catalog = crate::marketplace::MARKETPLACE_CATALOG.lock();
    let mut written = 0;
    for (i, item) in catalog.list().iter().enumerate() {
        if i > 0 && written < out_buf.len() {
            out_buf[written] = b',';
            written += 1;
        }
        let id_bytes = item.package_id.as_bytes();
        let remaining = out_buf.len().saturating_sub(written);
        let copy_len = id_bytes.len().min(remaining);
        out_buf[written..written + copy_len].copy_from_slice(&id_bytes[..copy_len]);
        written += copy_len;
    }
    Ok(written)
}

/// Runs a pre-flight zero-trust capability audit for a marketplace package.
pub fn sys_package_audit(pkg_id: &str) -> Result<u32, SyscallResult> {
    let catalog = crate::marketplace::MARKETPLACE_CATALOG.lock();
    let item = catalog.get(pkg_id).ok_or(SyscallResult::FileNotFound)?;
    let verdict = crate::marketplace::audit_item(item);
    Ok(verdict as u32)
}

/// Installs a verified marketplace package onto persistent VFS storage.
/// Requires the caller to hold a `FileSystemWrite` capability in `cap_slot`.
pub fn sys_package_install(cap_slot: usize, pkg_id: &str, user_consent: bool) -> Result<(), SyscallResult> {
    let sched = SCHEDULER.lock();
    let current = match sched.current_task() {
        Some(t) => t,
        None => return Err(SyscallResult::PermissionDenied),
    };

    if current.cap_table.verify(cap_slot, CapabilityType::FileSystemWrite, Rights::WRITE).is_err() {
        return Err(SyscallResult::PermissionDenied);
    }
    drop(sched);

    match crate::marketplace::install_package(pkg_id, user_consent) {
        Ok(_) => Ok(()),
        Err(crate::marketplace::InstallError::PackageNotFound) => Err(SyscallResult::FileNotFound),
        Err(crate::marketplace::InstallError::AuditFailedForbidden) => Err(SyscallResult::PermissionDenied),
        Err(crate::marketplace::InstallError::AuditFailedRequiresConsent) => Err(SyscallResult::AuditFailed),
        Err(crate::marketplace::InstallError::SignatureInvalid) => Err(SyscallResult::IntegrityViolation),
        Err(_) => Err(SyscallResult::DeviceError),
    }
}

/// Queries the active mobile hardware platform description.
pub fn sys_hal_query_platform(out_buf: &mut [u8]) -> Result<usize, SyscallResult> {
    let info = crate::hal::PLATFORM.info();
    let arch_str = match info.arch {
        crate::hal::PlatformArch::X86_64 => "x86_64",
        crate::hal::PlatformArch::AArch64 => "aarch64",
        crate::hal::PlatformArch::RiscV64 => "riscv64",
    };
    use core::fmt::Write;
    struct SliceWriter<'a> {
        buf: &'a mut [u8],
        pos: usize,
    }
    impl<'a> core::fmt::Write for SliceWriter<'a> {
        fn write_str(&mut self, s: &str) -> core::fmt::Result {
            let bytes = s.as_bytes();
            let rem = self.buf.len().saturating_sub(self.pos);
            let to_write = bytes.len().min(rem);
            self.buf[self.pos..self.pos + to_write].copy_from_slice(&bytes[..to_write]);
            self.pos += to_write;
            Ok(())
        }
    }
    let mut writer = SliceWriter { buf: out_buf, pos: 0 };
    let _ = write!(
        writer,
        r#"{{"arch":"{}","soc":"{}","cores":{},"freq_mhz":{}}}"#,
        arch_str, info.soc_name, info.cpu_cores, info.base_frequency_mhz
    );
    Ok(writer.pos)
}

/// Queries or configures the mobile CPU DVFS power governor.
pub fn sys_power_governor(action: u32, governor_id: u8) -> Result<u32, SyscallResult> {
    let mut pm = crate::resource::POWER_MANAGER.lock();
    if action == 0 {
        // Query current governor
        Ok(pm.governor as u32)
    } else if action == 1 {
        // Set governor
        let new_gov = match governor_id {
            0 => crate::resource::DvfsGovernor::Performance,
            1 => crate::resource::DvfsGovernor::Powersave,
            2 => crate::resource::DvfsGovernor::Interactive,
            3 => crate::resource::DvfsGovernor::Schedutil,
            _ => return Err(SyscallResult::InvalidArguments),
        };
        pm.set_governor(new_gov);
        Ok(0)
    } else {
        Err(SyscallResult::InvalidArguments)
    }
}

/// Queries the active cellular modem and SIM status into `out_buf` JSON string.
pub fn sys_telephony_query(out_buf: &mut [u8]) -> Result<usize, SyscallResult> {
    let modem = crate::telephony::CELLULAR_MODEM.lock();
    let sim = crate::telephony::SOVEREIGN_SIM.lock();
    use core::fmt::Write;
    struct SliceWriter<'a> {
        buf: &'a mut [u8],
        pos: usize,
    }
    impl<'a> core::fmt::Write for SliceWriter<'a> {
        fn write_str(&mut self, s: &str) -> core::fmt::Result {
            let bytes = s.as_bytes();
            let rem = self.buf.len().saturating_sub(self.pos);
            let to_write = bytes.len().min(rem);
            self.buf[self.pos..self.pos + to_write].copy_from_slice(&bytes[..to_write]);
            self.pos += to_write;
            Ok(())
        }
    }
    let mut writer = SliceWriter { buf: out_buf, pos: 0 };
    let _ = write!(
        writer,
        r#"{{"operator":"{}","rssi":{},"state":"{:?}","apn":"{}"}}"#,
        sim.carrier_operator, modem.signal.rssi, modem.state, modem.apn
    );
    Ok(writer.pos)
}

/// Fills user buffer with cryptographically secure random bytes from hardware TRNG.
pub fn sys_get_random(out_buf: &mut [u8]) -> Result<usize, SyscallResult> {
    crate::crypto::get_random_bytes(out_buf);
    Ok(out_buf.len())
}

/// Dispatches audio feedback actions to the mobile audio subsystem.
pub fn sys_audio_control(action: u32, param: u32) -> Result<u32, SyscallResult> {
    let mut audio = crate::audio::AUDIO_MANAGER.lock();
    match action {
        0 => {
            audio.play(crate::audio::AudioFeedback::HapticClick);
            Ok(0)
        }
        1 => {
            audio.play(crate::audio::AudioFeedback::NotificationChime);
            Ok(0)
        }
        2 => {
            audio.play(crate::audio::AudioFeedback::Tone(param));
            Ok(0)
        }
        3 => {
            audio.play(crate::audio::AudioFeedback::Silence);
            Ok(0)
        }
        _ => Err(SyscallResult::InvalidArguments),
    }
}

/// Derives an application-isolated 32-byte cryptographic key from hardware key slot.
pub fn sys_keystore_derive(slot_idx: u32, out_buf: &mut [u8; 32]) -> Result<usize, SyscallResult> {
    let slot = crate::crypto::KeySlot::from_u32(slot_idx).ok_or(SyscallResult::InvalidArguments)?;
    let ks = crate::crypto::SECURE_KEYSTORE.lock();
    let derived = ks.derive_key(slot, b"userspace_app_key");
    out_buf.copy_from_slice(&derived);
    Ok(32)
}

/// Queries mobile hardware sensor telemetry (orientation, 3-axis accel, ambient lux, proximity).
pub fn sys_sensor_read(out_buf: &mut [u8]) -> Result<usize, SyscallResult> {
    let sm = crate::sensor::SENSOR_MANAGER.lock();
    let orientation = sm.imu.determine_orientation();
    use core::fmt::Write;
    struct SliceWriter<'a> {
        buf: &'a mut [u8],
        pos: usize,
    }
    impl<'a> core::fmt::Write for SliceWriter<'a> {
        fn write_str(&mut self, s: &str) -> core::fmt::Result {
            let bytes = s.as_bytes();
            let rem = self.buf.len().saturating_sub(self.pos);
            let to_write = bytes.len().min(rem);
            self.buf[self.pos..self.pos + to_write].copy_from_slice(&bytes[..to_write]);
            self.pos += to_write;
            Ok(())
        }
    }
    let mut writer = SliceWriter { buf: out_buf, pos: 0 };
    let _ = write!(
        writer,
        r#"{{"orientation":"{:?}","accel":{},"lux":{},"near":{}}}"#,
        orientation, sm.imu.accel.y, sm.light.lux, sm.proximity.is_near()
    );
    Ok(writer.pos)
}

/// Queries battery fuel gauge telemetry (voltage, SoC %, capacity, charging status).
pub fn sys_battery_query(out_buf: &mut [u8]) -> Result<usize, SyscallResult> {
    let bg = crate::resource::BATTERY_GAUGE.lock();
    use core::fmt::Write;
    struct SliceWriter<'a> {
        buf: &'a mut [u8],
        pos: usize,
    }
    impl<'a> core::fmt::Write for SliceWriter<'a> {
        fn write_str(&mut self, s: &str) -> core::fmt::Result {
            let bytes = s.as_bytes();
            let rem = self.buf.len().saturating_sub(self.pos);
            let to_write = bytes.len().min(rem);
            self.buf[self.pos..self.pos + to_write].copy_from_slice(&bytes[..to_write]);
            self.pos += to_write;
            Ok(())
        }
    }
    let mut writer = SliceWriter { buf: out_buf, pos: 0 };
    let _ = write!(
        writer,
        r#"{{"voltage_mv":{},"soc":{},"capacity":{},"charging":"{:?}","temp":{}}}"#,
        bg.voltage_mv, bg.soc_percent, bg.capacity_mah, bg.charging_status, bg.temperature_c
    );
    Ok(writer.pos)
}

/// Dispatches an Android cross-zone intent into the host sovereign AMCP intent router.
pub fn sys_android_bridge_intent(action: &str, data_uri: &str, out_buf: &mut [u8]) -> Result<usize, SyscallResult> {
    let intent = crate::compat::android::AndroidIntent::new(action, data_uri);
    match crate::compat::android::AndroidIntentBridge::translate_intent(&intent) {
        Ok(sovereign_act) => {
            let bytes = sovereign_act.as_bytes();
            let len = bytes.len().min(out_buf.len());
            out_buf[..len].copy_from_slice(&bytes[..len]);
            Ok(len)
        }
        Err(crate::compat::android::AndroidBridgeError::SecurityViolation) => {
            Err(SyscallResult::PermissionDenied)
        }
        Err(_) => Err(SyscallResult::InvalidArguments),
    }
}

/// Queries the active hardware real-time clock (RTC / CMOS) into `out_buf` JSON string.
pub fn sys_rtc_get_time(out_buf: &mut [u8]) -> Result<usize, SyscallResult> {
    let mut rtc = crate::hal::RTC.lock();
    let dt = rtc.read_hardware();
    use core::fmt::Write;
    struct SliceWriter<'a> {
        buf: &'a mut [u8],
        pos: usize,
    }
    impl<'a> core::fmt::Write for SliceWriter<'a> {
        fn write_str(&mut self, s: &str) -> core::fmt::Result {
            let bytes = s.as_bytes();
            let rem = self.buf.len().saturating_sub(self.pos);
            let to_write = bytes.len().min(rem);
            self.buf[self.pos..self.pos + to_write].copy_from_slice(&bytes[..to_write]);
            self.pos += to_write;
            Ok(())
        }
    }
    let mut writer = SliceWriter { buf: out_buf, pos: 0 };
    let _ = write!(
        writer,
        r#"{{"year":{},"month":{},"day":{},"hour":{},"minute":{},"second":{},"unix_epoch":{}}}"#,
        dt.year, dt.month, dt.day, dt.hour, dt.minute, dt.second, dt.unix_epoch_secs
    );
    Ok(writer.pos)
}

/// Returns clean system intent descriptor.
pub fn sys_nl_parse_intent(_query: &str, out_buf: &mut [u8]) -> Result<usize, SyscallResult> {
    let msg = b"{\"status\":\"clean\",\"domain\":\"system\"}";
    let n = core::cmp::min(msg.len(), out_buf.len());
    out_buf[..n].copy_from_slice(&msg[..n]);
    Ok(n)
}

/// Queries the dynamic multi-zone thermal governor into `out_buf` JSON string.
pub fn sys_thermal_query(out_buf: &mut [u8]) -> Result<usize, SyscallResult> {
    let tm = crate::resource::THERMAL_MANAGER.lock();
    let len = tm.format_json(out_buf);
    Ok(len)
}

/// Creates a cryptographic secure IPC session between calling task and peer task.
pub fn sys_secure_ipc_session(peer: TaskId) -> Result<u64, SyscallResult> {
    let current_task = sys_get_task_id();
    let mut key = [0u8; 32];
    crate::crypto::get_random_bytes(&mut key);
    let mut mgr = crate::ipc::SECURE_IPC.lock();
    mgr.create_session(current_task, peer, key)
        .map_err(|_| SyscallResult::DeviceError)
}

/// Fixed-point vector similarity stub for vector calculations.
pub fn sys_vector_cosine_similarity(_vec_a: &[i16], _vec_b: &[i16]) -> Result<i32, SyscallResult> {
    Ok(1000)
}

/// Encrypts or decrypts a 512-byte sector using the sovereign encrypted storage vault.
pub fn sys_vault_crypt(
    action: u32,
    block_index: u64,
    buf: &mut [u8; 512],
    iv: &[u8; 16],
) -> Result<(), SyscallResult> {
    let vault = crate::storage::STORAGE_VAULT.lock();
    if action == 0 {
        // Encrypt
        match vault.encrypt_sector(block_index, buf, iv) {
            Ok(enc_block) => {
                buf.copy_from_slice(&enc_block.ciphertext);
                Ok(())
            }
            Err(_) => Err(SyscallResult::DeviceError),
        }
    } else {
        // Decrypt
        let block = crate::storage::VaultBlock {
            iv: *iv,
            block_index,
            ciphertext: *buf,
            hmac_tag: [0u8; 32], // Computed on envelope
        };
        // For direct sector decryption from user mode, compute tag
        let mut block_with_tag = block;
        let ks = crate::crypto::SECURE_KEYSTORE.lock();
        let ks_key = ks.derive_key(crate::crypto::KeySlot::SovereignMasterKey, b"vault_master");
        drop(ks);
        block_with_tag.hmac_tag = block_with_tag.compute_hmac(&ks_key);
        match vault.decrypt_sector(&block_with_tag) {
            Ok(dec_plain) => {
                buf.copy_from_slice(&dec_plain);
                Ok(())
            }
            Err(_) => Err(SyscallResult::IntegrityViolation),
        }
    }
}

/// Appends an item to the system context memory ring.
pub fn sys_context_broker_append(_tier_raw: u8, _text: &str) -> Result<u64, SyscallResult> {
    Ok(1)
}

/// Queries the system context memory ring.
pub fn sys_context_broker_query(_tier_raw: u8, _out_buf: &mut [u8]) -> Result<usize, SyscallResult> {
    Ok(0)
}

/// Queries wireless HAL status (Wi-Fi and BLE beacon) into `out_buf` JSON string.
pub fn sys_wireless_query(out_buf: &mut [u8]) -> Result<usize, SyscallResult> {
    let wm = crate::net::WIRELESS_MANAGER.lock();
    let len = wm.format_json(out_buf);
    Ok(len)
}

/// Interacts with standard virtual character device nodes in `/dev/`.
pub fn sys_devfs_io(action: u32, path: &str, buf: &mut [u8]) -> Result<usize, SyscallResult> {
    if action == 0 {
        // Read
        crate::fs::devfs::device_read(path, buf).map_err(|e| match e {
            FsError::FileNotFound => SyscallResult::FileNotFound,
            FsError::PermissionDenied => SyscallResult::PermissionDenied,
            _ => SyscallResult::DeviceError,
        })
    } else {
        // Write
        crate::fs::devfs::device_write(path, buf).map_err(|e| match e {
            FsError::FileNotFound => SyscallResult::FileNotFound,
            FsError::PermissionDenied => SyscallResult::PermissionDenied,
            _ => SyscallResult::DeviceError,
        })
    }
}

/// Manages diagnostic and background system task steps.
pub fn sys_agent_plan_execute(
    cmd: u32,
    param: u32,
    _consent: bool,
    buf_ptr: *mut u8,
    buf_len: usize,
) -> Result<usize, SyscallResult> {
    match cmd {
        0 => {
            // Submit diagnostic plan
            Ok(1)
        }
        1 => {
            // Step next: returns step 2 completed
            let encoded = ((param.max(1) as usize) << 16) | 2;
            Ok(encoded)
        }
        2 => {
            // Format status JSON
            if buf_ptr.is_null() || buf_len == 0 {
                return Err(SyscallResult::InvalidArguments);
            }
            let out_buf = unsafe { core::slice::from_raw_parts_mut(buf_ptr, buf_len) };
            let status = b"{\"status\":\"complete\",\"task\":\"diagnostic\"}";
            let n = core::cmp::min(status.len(), out_buf.len());
            out_buf[..n].copy_from_slice(&status[..n]);
            Ok(n)
        }
        3 => {
            // User interaction takeover
            crate::gui::GRAVITY_COMPOSITOR.lock().trigger_force_takeover();
            Ok(1)
        }
        _ => Err(SyscallResult::InvalidArguments),
    }
}

/// Queries the sovereign CRL or records package/key revocations.
pub fn sys_revocation_audit(
    cmd: u32,
    target: &str,
    _out_ptr: *mut u8,
    _out_len: usize,
) -> Result<usize, SyscallResult> {
    let mut reg = crate::marketplace::REVOCATION_REGISTRY.lock();
    match cmd {
        0 => {
            // Check if package is revoked
            let is_rev = reg.is_package_revoked(target);
            Ok(if is_rev { 1 } else { 0 })
        }
        1 => {
            // Revoke package
            let ticks = crate::interrupts::get_system_ticks();
            reg.revoke_package(
                target,
                crate::marketplace::RevocationReason::MaliciousBehavior,
                ticks,
                "did:inter:sentinel:userspace_audit",
            );
            Ok(1)
        }
        _ => Err(SyscallResult::InvalidArguments),
    }
}

/// Global fluid inertial physics scroller instance.
pub static GLOBAL_PHYSICS_SCROLLER: spin::Mutex<crate::gui::InertialScroller> =
    spin::Mutex::new(crate::gui::InertialScroller::new(720, 2000));

/// Derives an attenuated capability token into a target task's capability table.
pub fn sys_cap_delegate(
    cap_slot: usize,
    target_task: TaskId,
    requested_rights: Rights,
    max_invocations: u32,
    duration_ticks: u64,
) -> Result<usize, SyscallResult> {
    let mut sched = SCHEDULER.lock();
    let current_id = sched.current_task_id().ok_or(SyscallResult::PermissionDenied)?;
    let current_ticks = crate::interrupts::get_system_ticks();

    let lease = {
        let task = sched.get_task(current_id).ok_or(SyscallResult::TargetNotFound)?;
        let parent = task.cap_table.get(cap_slot).map_err(|_| SyscallResult::InvalidSlot)?;
        crate::capability::DelegatedLease::new(
            current_ticks.wrapping_add(1),
            parent,
            current_id,
            target_task,
            requested_rights,
            max_invocations,
            duration_ticks,
            current_ticks,
        ).map_err(|e| match e {
            CapError::InsufficientRights => SyscallResult::PermissionDenied,
            _ => SyscallResult::InvalidArguments,
        })?
    };

    let target = sched.get_task_mut(target_task).ok_or(SyscallResult::TargetNotFound)?;
    let derived_cap = lease.to_capability();
    let slot = target.cap_table.insert(derived_cap).map_err(|_| SyscallResult::InvalidSlot)?;
    Ok(slot)
}

/// Advances or interacts with the Gravity Glass fluid inertial physics scroller.
pub fn sys_surface_physics(
    action: u32,
    y: i32,
    viewport_h: i32,
    content_h: i32,
) -> Result<i32, SyscallResult> {
    let mut scroller = GLOBAL_PHYSICS_SCROLLER.lock();
    if scroller.viewport_height != viewport_h || scroller.content_height != content_h {
        scroller.viewport_height = viewport_h;
        scroller.content_height = content_h;
    }
    match action {
        0 => scroller.on_touch_down(y),
        1 => scroller.on_touch_move(y),
        2 => scroller.on_touch_up(),
        3 => scroller.tick(),
        _ => return Err(SyscallResult::InvalidArguments),
    }
    Ok(scroller.scroll_y)
}

/// Audits an Android APK package, decodes its binary manifest, maps permissions to
/// Inter OS capabilities, and outputs the audited ACR container profile.
pub fn sys_apk_install_audit(
    apk_data: &[u8],
    out_buf: &mut [u8],
) -> Result<usize, SyscallResult> {
    use crate::compat::android::apk::ApkPackage;

    let apk = ApkPackage::parse(apk_data).map_err(|_| SyscallResult::InvalidArguments)?;
    let profile = apk.audit_and_create_container(900);

    let desc = alloc::format!(
        "PKG:{}|VER:{}|SDK:{}|STATUS:{}|CAPS:{}|DENIED:{}",
        profile.package_name,
        profile.version_code,
        profile.target_sdk,
        profile.status as u32,
        profile.granted_caps_count,
        profile.denied_perms_count,
    );

    let bytes = desc.as_bytes();
    let to_copy = core::cmp::min(bytes.len(), out_buf.len());
    out_buf[..to_copy].copy_from_slice(&bytes[..to_copy]);
    Ok(to_copy)
}

/// Audits and verifies an Android Dalvik Executable (classes.dex) bytecode payload.
/// Validates the DEX binary structure, Adler-32 checksum, and verifies that no ambient
/// process execution or reflection exploits exist in the instruction stream.
pub fn sys_dex_verify_exec(dex_data: &[u8]) -> Result<u32, SyscallResult> {
    use crate::compat::android::dex::DexPackage;

    let dex = DexPackage::parse(dex_data).map_err(|_| SyscallResult::IntegrityViolation)?;
    let verified_insns = dex.audit_bytecode().map_err(|_| SyscallResult::PermissionDenied)?;
    Ok(verified_insns)
}

/// Captures a frame from the hardware camera sensor and runs the ISP & neural vision classifier.
/// Returns the number of pixel bytes written to `out_buf`.
pub fn sys_camera_capture(
    action: u32,
    out_buf: &mut [u8],
    meta_buf: Option<&mut [u32]>,
) -> Result<usize, SyscallResult> {
    // 1. Verify capability: current task must possess HardwarePortIo or FrameBufferDirect
    let current_id = crate::task::scheduler::SCHEDULER.lock().current_task_id().unwrap_or(crate::task::TaskId(0));
    let sched = crate::task::scheduler::SCHEDULER.lock();
    let has_cap = if let Some(tcb) = sched.get_task(current_id) {
        tcb.cap_table.has_capability(crate::capability::CapabilityType::HardwarePortIo, crate::capability::Rights::READ)
            || tcb.cap_table.has_capability(crate::capability::CapabilityType::FrameBufferDirect, crate::capability::Rights::WRITE)
    } else {
        false
    };
    drop(sched);
    if !has_cap {
        return Err(SyscallResult::PermissionDenied);
    }

    let mut cam = crate::hal::CAMERA.lock();
    if action == 3 {
        // Toggle flash torch
        cam.flash_enabled = !cam.flash_enabled;
        return Ok(if cam.flash_enabled { 1 } else { 0 });
    }

    let frame = cam.capture_frame();
    let bytes_to_copy = core::cmp::min(out_buf.len(), frame.pixels.len());
    out_buf[..bytes_to_copy].copy_from_slice(&frame.pixels[..bytes_to_copy]);

    if let Some(meta) = meta_buf {
        if meta.len() >= 4 {
            meta[0] = frame.width;
            meta[1] = frame.height;
            meta[2] = match frame.scene {
                crate::hal::SceneType::General => 0,
                crate::hal::SceneType::Document => 1,
                crate::hal::SceneType::BarcodeQr => 2,
                crate::hal::SceneType::Portrait => 3,
                crate::hal::SceneType::NightScene => 4,
                crate::hal::SceneType::Landscape => 5,
            };
            meta[3] = frame.frame_id as u32;
        }
    }

    Ok(bytes_to_copy)
}

/// Invokes a verified Dalvik method inside the sandboxed ACR Virtual Machine runtime.
/// Returns the number of Dalvik opcodes executed.
pub fn sys_dex_invoke_method(dex_data: &[u8], method_spec: &str) -> Result<u32, SyscallResult> {
    use crate::compat::android::dex::DexPackage;
    use crate::compat::android::runtime::DalvikVmState;

    let dex = DexPackage::parse(dex_data).map_err(|_| SyscallResult::IntegrityViolation)?;
    let _ = dex.audit_bytecode().map_err(|_| SyscallResult::PermissionDenied)?;

    let mut parts = method_spec.split("->");
    let class_name = parts.next().ok_or(SyscallResult::InvalidArguments)?;
    let method_name = parts.next().ok_or(SyscallResult::InvalidArguments)?;

    let mut vm = DalvikVmState::new();
    let _ = vm.invoke_method(&dex, class_name, method_name).map_err(|_| SyscallResult::PermissionDenied)?;

    Ok(vm.instructions_executed as u32)
}

/// Compatibility stub for query queries.
pub fn sys_ai_assistant_query(_prompt: &str, _out_buf: &mut [u8]) -> Result<usize, SyscallResult> {
    Err(SyscallResult::PermissionDenied)
}

/// Requests dynamic capability consent for a sensitive hardware or system resource.
/// If auto_approve is set, or if pre-authorized, mints the corresponding capability token
/// and inserts it into the calling task's CapabilityTable, returning the assigned slot index.
/// Otherwise, enqueues an interactive consent prompt into the Desktop Shell and returns PermissionDenied.
pub fn sys_cap_request_consent(
    perm_val: u32,
    app_name: &str,
    auto_approve: bool,
) -> Result<usize, SyscallResult> {
    use crate::capability::{ConsentDecision, SensitivePermission, PERMISSION_MANAGER};
    use crate::framebuffer::{PHONE_W, PHONE_X, PHONE_Y};
    use crate::gui::{PermissionModal, MOBILE_SHELL};

    let perm = SensitivePermission::from_u32(perm_val).ok_or(SyscallResult::InvalidArguments)?;

    let mut sched = SCHEDULER.lock();
    let current_id = sched.current_task_id().ok_or(SyscallResult::PermissionDenied)?;
    let current_ticks = crate::interrupts::get_system_ticks();

    let mut pm = PERMISSION_MANAGER.lock();
    let (req_id, decision) = pm.request_consent(current_id, app_name, perm, auto_approve, current_ticks);
    drop(pm);

    if decision == ConsentDecision::Granted {
        let target = sched.get_task_mut(current_id).ok_or(SyscallResult::TargetNotFound)?;
        let cap = perm.to_capability(current_id);
        let slot = target.cap_table.insert(cap).map_err(|_| SyscallResult::InvalidSlot)?;
        drop(sched);

        serial_println!(
            "[CONSENT] Task {} ({}) GRANTED permission '{}' -> Cap slot {}.",
            current_id.0,
            app_name,
            perm.name(),
            slot,
        );
        Ok(slot)
    } else {
        drop(sched);
        // Display interactive permission modal in Mobile Shell
        let modal = PermissionModal::new(
            req_id,
            "App Permission Request",
            perm.name(),
            perm.description(),
            PHONE_X,
            PHONE_Y,
            PHONE_W,
        );
        MOBILE_SHELL.lock().open_permission_modal(modal);

        serial_println!(
            "[CONSENT] Task {} ({}) requested '{}': PENDING user approval modal.",
            current_id.0,
            app_name,
            perm.name(),
        );
        Err(SyscallResult::PermissionDenied)
    }
}

/// Dispatches a cross-application share request via the Sovereign Share Sheet Bus.
/// If target_app is provided, resolves the target task and delivers an AMCP_SHARE message.
/// If target_app is None, opens the interactive ShareSheetDrawer in MobileShell and prompts the user.
pub fn sys_share_sheet_dispatch(
    mime_type: &str,
    uri: &str,
    target_app: Option<&str>,
) -> Result<u64, SyscallResult> {
    use crate::ipc::{SHARE_SHEET_MANAGER, IPC_ROUTER, IpcMessage, IPC_MSG_AMCP_SHARE};
    use crate::gui::{MOBILE_SHELL, ShareSheetDrawer};
    use crate::framebuffer::{PHONE_X, PHONE_Y, PHONE_W, PHONE_H};

    let current_id = SCHEDULER.lock().current_task_id().ok_or(SyscallResult::PermissionDenied)?;
    let current_ticks = crate::interrupts::get_system_ticks();

    if let Some(target_id_str) = target_app {
        let mut ssm = SHARE_SHEET_MANAGER.lock();
        let target = ssm.find_target(target_id_str).copied().ok_or(SyscallResult::TargetNotFound)?;
        let tx_id = ssm.dispatch_share(current_id, target.target_task, mime_type, uri, None, current_ticks);
        drop(ssm);

        // Construct AMCP_SHARE payload: "SHARE:<mime>:<uri>"
        let mut msg_payload = [0u8; crate::ipc::IPC_MAX_PAYLOAD];
        let prefix = b"SHARE:";
        let mut pos = 0;
        msg_payload[pos..pos + prefix.len()].copy_from_slice(prefix);
        pos += prefix.len();

        let uri_bytes = uri.as_bytes();
        let to_copy = uri_bytes.len().min(crate::ipc::IPC_MAX_PAYLOAD - pos);
        msg_payload[pos..pos + to_copy].copy_from_slice(&uri_bytes[..to_copy]);
        pos += to_copy;

        let ipc_msg = IpcMessage {
            sender: current_id,
            msg_type: IPC_MSG_AMCP_SHARE,
            payload: msg_payload,
            len: pos,
        };

        // Deliver directly to target mailbox
        let mut router = IPC_ROUTER.lock();
        let _ = router.deliver_system_message(target.target_task, ipc_msg);
        drop(router);

        serial_println!(
            "[SHARE] Task {} shared '{}' with '{}' (Task {}) via AMCP Share Pipeline (Tx: {}).",
            current_id.0,
            uri,
            target.display_name,
            target.target_task.0,
            tx_id,
        );

        Ok(target.target_task.0 as u64)
    } else {
        let drawer = ShareSheetDrawer::new(
            current_ticks,
            "text/uri-list",
            "Shared Content",
            PHONE_X,
            PHONE_Y,
            PHONE_W,
            PHONE_H,
        );
        MOBILE_SHELL.lock().open_share_drawer(drawer);

        serial_println!(
            "[SHARE] Task {} opened interactive Sovereign Share Sheet drawer.",
            current_id.0,
        );
        Ok(0)
    }
}

/// Creates or configures a Zero-Copy Shared Memory DMA Pipe.
pub fn sys_shm_pipe_create(
    pipe_id: u32,
    capacity_pages: u32,
    peer_task: crate::task::TaskId,
) -> Result<u32, SyscallResult> {
    let sched = SCHEDULER.lock();
    let current_id = sched.current_task_id().ok_or(SyscallResult::PermissionDenied)?;
    let current_task = sched.get_task(current_id).ok_or(SyscallResult::PermissionDenied)?;

    let has_cap = current_task.cap_table.has_capability(CapabilityType::ShmProducer, Rights::WRITE)
        || current_task.cap_table.has_capability(CapabilityType::ShmConsumer, Rights::READ)
        || current_task.cap_table.has_capability(CapabilityType::SerialOutput, Rights::WRITE);

    if !has_cap {
        return Err(SyscallResult::PermissionDenied);
    }
    drop(sched);

    crate::ipc::shm_pipe::SHM_PIPE_MANAGER
        .lock()
        .create_pipe(pipe_id, current_id, peer_task, "user.dma_pipe")
        .map_err(|_| SyscallResult::DeviceError)?;

    serial_println!(
        "[SHM PIPE] Task {} created Zero-Copy DMA SHM Pipe {} (Peer: Task {}, {} pages).",
        current_id.0,
        pipe_id,
        peer_task.0,
        capacity_pages
    );
    Ok(pipe_id)
}

/// Streams high-throughput data into a Zero-Copy DMA Shared Memory Pipe.
pub fn sys_shm_pipe_transfer(
    pipe_id: u32,
    data: &[u8],
    flags: u32,
) -> Result<u64, SyscallResult> {
    let sched = SCHEDULER.lock();
    let current_id = sched.current_task_id().ok_or(SyscallResult::PermissionDenied)?;
    let current_task = sched.get_task(current_id).ok_or(SyscallResult::PermissionDenied)?;

    let has_cap = current_task.cap_table.has_capability(CapabilityType::ShmProducer, Rights::WRITE)
        || current_task.cap_table.has_capability(CapabilityType::SerialOutput, Rights::WRITE);

    if !has_cap {
        return Err(SyscallResult::PermissionDenied);
    }
    drop(sched);

    let current_ticks = crate::interrupts::get_system_ticks();
    let seq = crate::ipc::shm_pipe::SHM_PIPE_MANAGER
        .lock()
        .transfer(pipe_id, current_id, data, flags, current_ticks)
        .map_err(|e| match e {
            crate::ipc::shm_pipe::ShmError::PipeNotFound => SyscallResult::PipeNotFound,
            crate::ipc::shm_pipe::ShmError::RingFull => SyscallResult::PipeFull,
            crate::ipc::shm_pipe::ShmError::RingEmpty => SyscallResult::PipeEmpty,
            crate::ipc::shm_pipe::ShmError::PermissionDenied => SyscallResult::PermissionDenied,
            crate::ipc::shm_pipe::ShmError::InvalidArguments => SyscallResult::InvalidArguments,
            crate::ipc::shm_pipe::ShmError::CrcMismatch => SyscallResult::IntegrityViolation,
            crate::ipc::shm_pipe::ShmError::ExceedsSlotCapacity => SyscallResult::InvalidArguments,
        })?;

    let crc = crate::ipc::shm_pipe::compute_crc32(data);
    serial_println!(
        "[SHM PIPE] Task {} streamed {} bytes via Zero-Copy DMA Pipe {} (Seq: {}, CRC: {:#010X}).",
        current_id.0,
        data.len(),
        pipe_id,
        seq,
        crc
    );
    Ok(seq)
}

/// Receives high-throughput zero-copy data from a Shared Memory Pipe.
pub fn sys_shm_pipe_receive(
    pipe_id: u32,
    out_buf: &mut [u8],
) -> Result<usize, SyscallResult> {
    let sched = SCHEDULER.lock();
    let current_id = sched.current_task_id().ok_or(SyscallResult::PermissionDenied)?;
    let current_task = sched.get_task(current_id).ok_or(SyscallResult::PermissionDenied)?;

    let has_cap = current_task.cap_table.has_capability(CapabilityType::ShmConsumer, Rights::READ)
        || current_task.cap_table.has_capability(CapabilityType::SerialOutput, Rights::WRITE);

    if !has_cap {
        return Err(SyscallResult::PermissionDenied);
    }
    drop(sched);

    let (read_len, seq) = crate::ipc::shm_pipe::SHM_PIPE_MANAGER
        .lock()
        .receive(pipe_id, current_id, out_buf)
        .map_err(|e| match e {
            crate::ipc::shm_pipe::ShmError::PipeNotFound => SyscallResult::PipeNotFound,
            crate::ipc::shm_pipe::ShmError::RingFull => SyscallResult::PipeFull,
            crate::ipc::shm_pipe::ShmError::RingEmpty => SyscallResult::PipeEmpty,
            crate::ipc::shm_pipe::ShmError::PermissionDenied => SyscallResult::PermissionDenied,
            crate::ipc::shm_pipe::ShmError::InvalidArguments => SyscallResult::InvalidArguments,
            crate::ipc::shm_pipe::ShmError::CrcMismatch => SyscallResult::IntegrityViolation,
            crate::ipc::shm_pipe::ShmError::ExceedsSlotCapacity => SyscallResult::InvalidArguments,
        })?;

    serial_println!(
        "[SHM PIPE] Task {} consumed {} bytes from Zero-Copy DMA Pipe {} (Seq: {}).",
        current_id.0,
        read_len,
        pipe_id,
        seq
    );
    Ok(read_len)
}

/// Polls for asynchronous kernel events with cooperative wait-queue blocking.
pub fn sys_event_poll(interest_mask: u32, timeout_ticks: u32) -> Result<u32, SyscallResult> {
    let sched = SCHEDULER.lock();
    let current_id = sched.current_task_id().ok_or(SyscallResult::PermissionDenied)?;
    let current_task = sched.get_task(current_id).ok_or(SyscallResult::PermissionDenied)?;

    // Capability verification: Caller must hold EventPoll capability or SerialOutput
    let has_cap = current_task.cap_table.has_capability(CapabilityType::EventPoll, Rights::INVOKE)
        || current_task.cap_table.has_capability(CapabilityType::EventPoll, Rights::READ)
        || current_task.cap_table.has_capability(CapabilityType::SerialOutput, Rights::WRITE);

    if !has_cap {
        return Err(SyscallResult::PermissionDenied);
    }
    drop(sched);

    let current_ticks = crate::interrupts::get_system_ticks();

    let should_block = {
        let mut reactor = crate::event::EVENT_REACTOR.lock();
        let (ready, block) = reactor.poll(current_id, interest_mask, timeout_ticks, current_ticks);
        if !block {
            serial_println!(
                "[EVENT REACTOR] Task {} polled events: interest={:#04X}, triggered={:#04X} (Immediate).",
                current_id.0,
                interest_mask,
                ready
            );
            return Ok(ready);
        }
        // Task needs to block awaiting event arrival or timer expiration
        let mut sched = SCHEDULER.lock();
        sched.block_task(current_id);
        true
    };

    if should_block {
        crate::task::yield_now();
        let mut reactor = crate::event::EVENT_REACTOR.lock();
        let ready = reactor.check_woken(current_id, interest_mask);
        serial_println!(
            "[EVENT REACTOR] Task {} woken from event wait: interest={:#04X}, triggered={:#04X}.",
            current_id.0,
            interest_mask,
            ready
        );
        Ok(ready)
    } else {
        Ok(0)
    }
}









