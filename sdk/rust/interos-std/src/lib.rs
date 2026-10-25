#![no_std]

//! Inter OS Userspace Standard Library (interos-std)
//!
//! Provides safe, ergonomic Rust bindings for Inter OS hardware fast system calls
//! (`syscall` instruction) executing in Ring 3 User Mode.

pub mod ai;
pub mod app;
pub mod content;
pub mod design;
pub mod font;
pub mod font_fhd;
pub mod image;
pub mod settings;
pub mod wav;

// WHY std under test only: the guest build of this crate is `#![no_std]` and must
// stay that way, but the unit tests want `Vec` and the other collections to
// build scratch surfaces. This is compiled only for `cargo test`, never for the
// bare-metal target, so it adds nothing to the image.
#[cfg(test)]
extern crate std;
pub mod service;
pub mod compositor {
    pub const MAGIC: [u8; 4] = *b"ICMP";
    pub const VERSION: u8 = 1;
    pub const MSG_TYPE: u32 = 0x434F_4D50;
    pub const HEADER_SIZE: usize = 40;

    #[repr(u8)]
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub enum Op {
        CreateSurface = 1,
        WritePixels = 2,
        SetGeometry = 3,
        DestroySurface = 4,
        Commit = 5,
        Focus = 6,
    }

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub struct Request {
        pub op: Op,
        pub surface_id: u32,
        pub width: u32,
        pub height: u32,
        pub x: i32,
        pub y: i32,
        pub z_order: u32,
        pub visible: bool,
        pub pixel_offset: u32,
    }

    impl Request {
        pub fn encode(&self, out: &mut [u8]) -> Result<usize, ()> {
            if out.len() < HEADER_SIZE {
                return Err(());
            }
            out[..HEADER_SIZE].fill(0);
            out[..4].copy_from_slice(&MAGIC);
            out[4] = VERSION;
            out[5] = self.op as u8;
            out[8..12].copy_from_slice(&self.surface_id.to_le_bytes());
            out[12..16].copy_from_slice(&self.width.to_le_bytes());
            out[16..20].copy_from_slice(&self.height.to_le_bytes());
            out[20..24].copy_from_slice(&self.pixel_offset.to_le_bytes());
            out[24..28].copy_from_slice(&self.x.to_le_bytes());
            out[28..32].copy_from_slice(&self.y.to_le_bytes());
            out[32..36].copy_from_slice(&self.z_order.to_le_bytes());
            out[36] = self.visible as u8;
            Ok(HEADER_SIZE)
        }
    }
}

/// The capability-slot argument reserved in the syscall ABI.
///
/// WHY it is reserved: a capability slot is a *position* in the task's
/// capability table, and the position depends on the order the kernel
/// inserted the capabilities in. The kernel therefore authorises by
/// capability *type* and ignores this argument, which is what
/// `PackageRegistry` already did. Making the caller name a slot made every
/// caller responsible for its own table layout: the launcher passed slot 0
/// while its `FileSystemRead` capability sat at slot 3, and the call was
/// refused with nothing on screen to say why.
pub const RESERVED_CAP_SLOT: u64 = 0;

/// Well-known system call identifiers matching the Inter OS microkernel.
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
    FsSync = 125,
    WasmExecute = 16,
    ResourceQuery = 19,
    NetSocket = 21,
    NetSendTo = 22,
    NetRecvFrom = 23,
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
    ThermalQuery = 40,
    SecureIpcSession = 41,
    VaultCrypt = 43,
    WirelessQuery = 45,
    DevFsIo = 46,
    RevocationAudit = 48,
    CapDelegate = 49,
    SurfacePhysics = 51,
    ApkInstallAudit = 52,
    DexVerifyExec = 53,
    CameraCapture = 54,
    DexInvokeMethod = 55,
    CapRequestConsent = 57,
    ShareSheetDispatch = 58,
    ShmPipeCreate = 59,
    ShmPipeTransfer = 60,
    ShmPipeReceive = 61,
    EventPoll = 62,
    WaitPid = 63,
    ExitWithCode = 64,
    TelephonyDial = 65,
    TelephonyHangup = 66,
    TelephonySendSms = 67,
    TelephonySetData = 68,
    SurfaceCreate = 69,
    SurfaceFlip = 70,
    TouchPoll = 71,
    PowerSetProfile = 72,
    SurfaceWrite = 73,
    SurfaceDestroy = 74,
    PackageList = 75,
    PackageRegistry = 76,
    WindowList = 77,
    PackageRollback = 78,
    WindowSetGeometry = 79,
    WindowFocus = 80,
    ServiceList = 81,
    SettingsGet = 82,
    SettingsSet = 83,
    StorageQuery = 84,
    FsList = 85,
    ServiceStart = 86,
    ServiceStop = 87,
    ServiceRecoverFailed = 88,
    ServicePackageVerify = 89,
    ServiceHeartbeat = 90,
    ServicePackageLaunch = 91,
    InputEventPoll = 92,
    NetworkPoll = 93,
    TcpConnect = 94,
    NetSendSocket = 95,
    NetCloseSocket = 96,
    TcpSend = 97,
    TcpRecv = 98,
    DnsResolve = 99,
    TcpClose = 100,
    AppLaunch = 101,
    ServiceQuery = 102,
    IpcRecvMeta = 103,
    WindowSetInput = 104,
    UserGetCurrent = 105,
    UserSwitch = 106,
    UserLockState = 107,
    UserUnlock = 108,
    UserLock = 109,
    NotificationPost = 110,
    NotificationCancel = 111,
    NotificationQuery = 112,
    PermissionRevoke = 113,
    SettingsHardwareStatus = 114,
    AppStorageOpen = 115,
    AppBackground = 116,
    AppForeground = 117,
    AppSuspend = 118,
    AppWake = 119,
    PackageUninstall = 120,
    AppStop = 121,
    AppRestart = 122,
    AppRuntimeList = 123,
    AppFocus = 124,
    UserHasCredential = 129,
    UserClearCredential = 128,
    UserSetCredential = 126,
    DevicePower = 127,
    // 130 and 131 are the first free numbers above the whole declared table,
    // so no existing ABI number moves.
    DisplaySetMode = 130,
    DisplaySetOrientation = 131,
    CapLookup = 132,
    FsUnlink = 133,
    /// The shared document index and the system-wide intent registry.
    ContentIndexQuery = 140,
    ContentIndexRefresh = 141,
    IntentResolve = 142,
    IntentList = 143,
    IntentDispatch = 144,
    IntentPending = 145,
    /// Reads another client's display surface, scaled, into the caller's
    /// buffer.
    ///
    /// WHY it is its own number and not an extension of `SurfaceWrite`: reading
    /// a *running* app's pixels is a different authority from writing your
    /// own, and the platform's own kernel shell has no framebuffer-grab call
    /// at all -- the recents screen had to draw a picture it never received.
    /// The call is authorised by the `SurfaceCapture` capability, so "who may
    /// see another app's screen" is a capability decision rather than a
    /// property of the caller.
    SurfaceCapture = 134,
    MemoryTelemetry = 135,
    TaskEnumerate = 136,
    /// AC'97 PCM output. Authorised by `CapabilityType::AudioStream`; WRITE
    /// covers open/write/close and READ covers status.
    AudioStreamOpen = 146,
    AudioStreamWrite = 147,
    AudioStreamStatus = 148,
    AudioStreamClose = 149,
    /// Resolves a capability the caller already holds to its generation-counted
    /// handle. It grants nothing; it only names an existing slot and generation.
    CapHandleLookup = 150,
}

// -----------------------------------------------------------------------------
// Low-Level Fast System Call Wrappers (Multi-Architecture: AMD64, AArch64 & RISC-V 64)
// -----------------------------------------------------------------------------

#[inline(always)]
pub unsafe fn syscall0(num: SyscallNumber) -> i64 {
    let ret: i64;
    #[cfg(target_arch = "x86_64")]
    core::arch::asm!(
        "syscall",
        in("rax") num as u64,
        lateout("rax") ret,
        lateout("rcx") _,
        lateout("r11") _,
        options(nostack)
    );
    #[cfg(target_arch = "aarch64")]
    core::arch::asm!(
        "svc #0",
        in("x8") num as u64,
        lateout("x0") ret,
        options(nostack)
    );
    #[cfg(target_arch = "riscv64")]
    core::arch::asm!(
        "ecall",
        in("a7") num as u64,
        lateout("a0") ret,
        options(nostack)
    );
    #[cfg(not(any(
        target_arch = "x86_64",
        target_arch = "aarch64",
        target_arch = "riscv64"
    )))]
    {
        let _ = num;
        ret = 0;
    }
    ret
}

#[inline(always)]
pub unsafe fn syscall1(num: SyscallNumber, a1: u64) -> i64 {
    let ret: i64;
    #[cfg(target_arch = "x86_64")]
    core::arch::asm!(
        "syscall",
        in("rax") num as u64,
        in("rdi") a1,
        lateout("rax") ret,
        lateout("rcx") _,
        lateout("r11") _,
        options(nostack)
    );
    #[cfg(target_arch = "aarch64")]
    core::arch::asm!(
        "svc #0",
        in("x8") num as u64,
        in("x0") a1,
        lateout("x0") ret,
        options(nostack)
    );
    #[cfg(target_arch = "riscv64")]
    core::arch::asm!(
        "ecall",
        in("a7") num as u64,
        in("a0") a1,
        lateout("a0") ret,
        options(nostack)
    );
    #[cfg(not(any(
        target_arch = "x86_64",
        target_arch = "aarch64",
        target_arch = "riscv64"
    )))]
    {
        let _ = (num, a1);
        ret = 0;
    }
    ret
}

#[inline(always)]
pub unsafe fn syscall2(num: SyscallNumber, a1: u64, a2: u64) -> i64 {
    let ret: i64;
    #[cfg(target_arch = "x86_64")]
    core::arch::asm!(
        "syscall",
        in("rax") num as u64,
        in("rdi") a1,
        in("rsi") a2,
        lateout("rax") ret,
        lateout("rcx") _,
        lateout("r11") _,
        options(nostack)
    );
    #[cfg(target_arch = "aarch64")]
    core::arch::asm!(
        "svc #0",
        in("x8") num as u64,
        in("x0") a1,
        in("x1") a2,
        lateout("x0") ret,
        options(nostack)
    );
    #[cfg(target_arch = "riscv64")]
    core::arch::asm!(
        "ecall",
        in("a7") num as u64,
        in("a0") a1,
        in("a1") a2,
        lateout("a0") ret,
        options(nostack)
    );
    #[cfg(not(any(
        target_arch = "x86_64",
        target_arch = "aarch64",
        target_arch = "riscv64"
    )))]
    {
        let _ = (num, a1, a2);
        ret = 0;
    }
    ret
}

#[inline(always)]
pub unsafe fn syscall3(num: SyscallNumber, a1: u64, a2: u64, a3: u64) -> i64 {
    let ret: i64;
    #[cfg(target_arch = "x86_64")]
    core::arch::asm!(
        "syscall",
        in("rax") num as u64,
        in("rdi") a1,
        in("rsi") a2,
        in("rdx") a3,
        lateout("rax") ret,
        lateout("rcx") _,
        lateout("r11") _,
        options(nostack)
    );
    #[cfg(target_arch = "aarch64")]
    core::arch::asm!(
        "svc #0",
        in("x8") num as u64,
        in("x0") a1,
        in("x1") a2,
        in("x2") a3,
        lateout("x0") ret,
        options(nostack)
    );
    #[cfg(target_arch = "riscv64")]
    core::arch::asm!(
        "ecall",
        in("a7") num as u64,
        in("a0") a1,
        in("a1") a2,
        in("a2") a3,
        lateout("a0") ret,
        options(nostack)
    );
    #[cfg(not(any(
        target_arch = "x86_64",
        target_arch = "aarch64",
        target_arch = "riscv64"
    )))]
    {
        let _ = (num, a1, a2, a3);
        ret = 0;
    }
    ret
}

#[inline(always)]
pub unsafe fn syscall4(num: SyscallNumber, a1: u64, a2: u64, a3: u64, a4: u64) -> i64 {
    let ret: i64;
    #[cfg(target_arch = "x86_64")]
    core::arch::asm!(
        "syscall",
        in("rax") num as u64,
        in("rdi") a1,
        in("rsi") a2,
        in("rdx") a3,
        in("r10") a4,
        lateout("rax") ret,
        lateout("rcx") _,
        lateout("r11") _,
        options(nostack)
    );
    #[cfg(target_arch = "aarch64")]
    core::arch::asm!(
        "svc #0",
        in("x8") num as u64,
        in("x0") a1,
        in("x1") a2,
        in("x2") a3,
        in("x3") a4,
        lateout("x0") ret,
        options(nostack)
    );
    #[cfg(target_arch = "riscv64")]
    core::arch::asm!(
        "ecall",
        in("a7") num as u64,
        in("a0") a1,
        in("a1") a2,
        in("a2") a3,
        in("a3") a4,
        lateout("a0") ret,
        options(nostack)
    );
    #[cfg(not(any(
        target_arch = "x86_64",
        target_arch = "aarch64",
        target_arch = "riscv64"
    )))]
    {
        let _ = (num, a1, a2, a3, a4);
        ret = 0;
    }
    ret
}

#[inline(always)]
pub unsafe fn syscall5(num: SyscallNumber, a1: u64, a2: u64, a3: u64, a4: u64, a5: u64) -> i64 {
    let ret: i64;
    #[cfg(target_arch = "x86_64")]
    core::arch::asm!(
        "syscall",
        in("rax") num as u64,
        in("rdi") a1,
        in("rsi") a2,
        in("rdx") a3,
        in("r10") a4,
        in("r8") a5,
        lateout("rax") ret,
        lateout("rcx") _,
        lateout("r11") _,
        options(nostack)
    );
    #[cfg(target_arch = "aarch64")]
    core::arch::asm!(
        "svc #0",
        in("x8") num as u64,
        in("x0") a1,
        in("x1") a2,
        in("x2") a3,
        in("x3") a4,
        in("x4") a5,
        lateout("x0") ret,
        options(nostack)
    );
    #[cfg(target_arch = "riscv64")]
    core::arch::asm!(
        "ecall",
        in("a7") num as u64,
        in("a0") a1,
        in("a1") a2,
        in("a2") a3,
        in("a3") a4,
        in("a4") a5,
        lateout("a0") ret,
        options(nostack)
    );
    #[cfg(not(any(
        target_arch = "x86_64",
        target_arch = "aarch64",
        target_arch = "riscv64"
    )))]
    {
        let _ = (num, a1, a2, a3, a4, a5);
        ret = 0;
    }
    ret
}

#[inline(always)]
pub unsafe fn syscall6(
    num: SyscallNumber,
    a1: u64,
    a2: u64,
    a3: u64,
    a4: u64,
    a5: u64,
    a6: u64,
) -> i64 {
    let ret: i64;
    #[cfg(target_arch = "x86_64")]
    core::arch::asm!(
        "syscall",
        in("rax") num as u64,
        in("rdi") a1,
        in("rsi") a2,
        in("rdx") a3,
        in("r10") a4,
        in("r8") a5,
        in("r9") a6,
        lateout("rax") ret,
        lateout("rcx") _,
        lateout("r11") _,
        options(nostack)
    );
    #[cfg(target_arch = "aarch64")]
    core::arch::asm!(
        "svc #0",
        in("x8") num as u64,
        in("x0") a1,
        in("x1") a2,
        in("x2") a3,
        in("x3") a4,
        in("x4") a5,
        in("x5") a6,
        lateout("x0") ret,
        options(nostack)
    );
    #[cfg(target_arch = "riscv64")]
    core::arch::asm!(
        "ecall",
        in("a7") num as u64,
        in("a0") a1,
        in("a1") a2,
        in("a2") a3,
        in("a3") a4,
        in("a4") a5,
        in("a5") a6,
        lateout("a0") ret,
        options(nostack)
    );
    #[cfg(not(any(
        target_arch = "x86_64",
        target_arch = "aarch64",
        target_arch = "riscv64"
    )))]
    {
        let _ = (num, a1, a2, a3, a4, a5, a6);
        ret = 0;
    }
    ret
}

// -----------------------------------------------------------------------------
// High-Level Safe Userspace APIs
// -----------------------------------------------------------------------------

/// Yields the remainder of the task's time slice to the scheduler.
#[inline(always)]
pub fn yield_now() {
    unsafe { syscall0(SyscallNumber::Yield) };
}

/// Terminates the current userspace task.
#[inline(always)]
pub fn exit() -> ! {
    exit_with_code(0);
}

/// Terminates the current userspace task with a specific exit status code.
#[inline(always)]
pub fn exit_with_code(code: i32) -> ! {
    unsafe { syscall1(SyscallNumber::ExitWithCode, code as u64) };
    loop {}
}

/// Waits for a child process to terminate and reaps its exit code.
/// Returns Ok(()) on successful reap, or Err(code) on failure or when child is still running.
#[inline(always)]
pub fn waitpid(child_id: u64) -> Result<(), i64> {
    let ret = unsafe { syscall1(SyscallNumber::WaitPid, child_id) };
    if ret == 0 {
        Ok(())
    } else {
        Err(ret)
    }
}

/// Emits a diagnostic log string through the kernel serial console.
/// Requires the caller to hold a `SerialOutput` capability in `cap_slot`.
pub fn log(msg: &str) -> Result<(), i64> {
    let ret = unsafe {
        syscall3(
            SyscallNumber::Log,
            RESERVED_CAP_SLOT,
            msg.as_ptr() as u64,
            msg.len() as u64,
        )
    };
    if ret >= 0 {
        Ok(())
    } else {
        Err(ret)
    }
}

/// Queries the hardware timer ticks since boot.
/// Requires the caller to hold a `TimerTickRead` capability in `cap_slot`.
/// Launches a signed trusted system-service RAPP after kernel verification.
pub fn service_package_launch(path: &str) -> Result<u64, i64> {
    let ret = unsafe {
        syscall3(
            SyscallNumber::ServicePackageLaunch,
            RESERVED_CAP_SLOT,
            path.as_ptr() as u64,
            path.len() as u64,
        )
    };
    if ret >= 0 {
        Ok(ret as u64)
    } else {
        Err(ret)
    }
}

pub fn get_ticks() -> Result<u64, i64> {
    let ret = unsafe { syscall1(SyscallNumber::GetTicks, RESERVED_CAP_SLOT) };
    if ret >= 0 {
        Ok(ret as u64)
    } else {
        Err(ret)
    }
}

/// Queries the task identifier assigned by the kernel scheduler.
pub fn get_task_id() -> u64 {
    unsafe { syscall0(SyscallNumber::GetTaskId) as u64 }
}

/// Transmits a message through zero-trust IPC.
pub fn ipc_send(
    target_task_id: u64,
    msg_type: u32,
    payload: &[u8],
) -> Result<(), i64> {
    let ret = unsafe {
        syscall5(
            SyscallNumber::IpcSend,
            RESERVED_CAP_SLOT,
            target_task_id,
            msg_type as u64,
            payload.as_ptr() as u64,
            payload.len() as u64,
        )
    };
    if ret >= 0 {
        Ok(())
    } else {
        Err(ret)
    }
}

/// Receives a message from the task IPC mailbox.
pub fn ipc_recv(out_buf: &mut [u8]) -> Result<usize, i64> {
    let ret = unsafe {
        syscall3(
            SyscallNumber::IpcRecv,
            RESERVED_CAP_SLOT,
            out_buf.as_mut_ptr() as u64,
            out_buf.len() as u64,
        )
    };
    if ret >= 0 {
        Ok(ret as usize)
    } else {
        Err(ret)
    }
}

/// Ergonomic logging helper module matching application templates.
pub mod log {
    pub fn init(_pkg: &str) {}
    pub fn info(msg: &str) {
        let _ = crate::log(msg);
    }
    pub fn warn(msg: &str) {
        let _ = crate::log(msg);
    }
    pub fn error(msg: &str) {
        let _ = crate::log(msg);
    }
}

/// Ergonomic IPC helper module matching application templates.
pub mod ipc {
    pub fn send(target_task_id: u64, msg_type: u32, payload: &[u8]) -> Result<(), i64> {
        crate::ipc_send(target_task_id, msg_type, payload)
    }

    pub fn receive_any(out_buf: &mut [u8]) -> Result<(u64, u32, usize), i64> {
        let mut sender = 0u64;
        let mut msg_type = 0u32;
        let ret = unsafe {
            crate::syscall5(
                crate::SyscallNumber::IpcRecvMeta,
                0,
                out_buf.as_mut_ptr() as u64,
                out_buf.len() as u64,
                &mut sender as *mut u64 as u64,
                &mut msg_type as *mut u32 as u64,
            )
        };
        if ret >= 0 {
            Ok((sender, msg_type, ret as usize))
        } else {
            Err(ret)
        }
    }
}

/// Creates and binds a UDP socket to a port.
pub fn net_socket(port: u16) -> Result<u32, i64> {
    let ret = unsafe { syscall1(SyscallNumber::NetSocket, port as u64) };
    if ret >= 0 {
        Ok(ret as u32)
    } else {
        Err(ret)
    }
}

/// Transmits a UDP packet over the network stack.
pub fn net_sendto(dst_ip: [u8; 4], dst_port: u16, payload: &[u8]) -> Result<(), i64> {
    let ip_u32 = u32::from_be_bytes(dst_ip) as u64;
    let ret = unsafe {
        syscall4(
            SyscallNumber::NetSendTo,
            ip_u32,
            dst_port as u64,
            payload.as_ptr() as u64,
            payload.len() as u64,
        )
    };
    if ret >= 0 {
        Ok(())
    } else {
        Err(ret)
    }
}

/// Receives a packet from a UDP socket.
/// Drives the userspace network service receive path.
pub fn net_send_socket(
    socket_id: u32,
    dst_ip: [u8; 4],
    dst_port: u16,
    payload: &[u8],
) -> Result<(), i64> {
    let ip = u32::from_be_bytes(dst_ip) as u64;
    let ret = unsafe {
        syscall5(
            SyscallNumber::NetSendSocket,
            socket_id as u64,
            ip,
            dst_port as u64,
            payload.as_ptr() as u64,
            payload.len() as u64,
        )
    };
    if ret == 0 {
        Ok(())
    } else {
        Err(ret)
    }
}

/// Closes a UDP socket owned by the current task.
pub fn net_close_socket(socket_id: u32) -> Result<(), i64> {
    let ret = unsafe { syscall1(SyscallNumber::NetCloseSocket, socket_id as u64) };
    if ret == 0 {
        Ok(())
    } else {
        Err(ret)
    }
}

pub fn tcp_send(connection_id: u32, payload: &[u8]) -> Result<(), i64> {
    let r = unsafe {
        syscall3(
            SyscallNumber::TcpSend,
            connection_id as u64,
            payload.as_ptr() as u64,
            payload.len() as u64,
        )
    };
    if r == 0 {
        Ok(())
    } else {
        Err(r)
    }
}
pub fn tcp_recv(connection_id: u32, out: &mut [u8]) -> Result<usize, i64> {
    let r = unsafe {
        syscall3(
            SyscallNumber::TcpRecv,
            connection_id as u64,
            out.as_mut_ptr() as u64,
            out.len() as u64,
        )
    };
    if r >= 0 {
        Ok(r as usize)
    } else {
        Err(r)
    }
}
pub fn tcp_close(connection_id: u32) -> Result<(), i64> {
    let ret = unsafe { syscall1(SyscallNumber::TcpClose, connection_id as u64) };
    if ret == 0 {
        Ok(())
    } else {
        Err(ret)
    }
}

pub fn tcp_connect(dst_ip: [u8; 4], dst_port: u16, local_port: u16, seq: u32) -> Result<u32, i64> {
    let ip = u32::from_be_bytes(dst_ip) as u64;
    let ret = unsafe {
        syscall4(
            SyscallNumber::TcpConnect,
            ip,
            dst_port as u64,
            local_port as u64,
            seq as u64,
        )
    };
    if ret >= 0 {
        Ok(ret as u32)
    } else {
        Err(ret)
    }
}

/// Resolves an IPv4 hostname through the capability-governed system DNS service.
pub fn dns_resolve(host: &str) -> Result<[u8; 4], i64> {
    let ret = unsafe {
        syscall2(
            SyscallNumber::DnsResolve,
            host.as_ptr() as u64,
            host.len() as u64,
        )
    };
    if ret >= 0 {
        Ok((ret as u32).to_be_bytes())
    } else {
        Err(ret)
    }
}

pub fn network_poll() -> Result<usize, i64> {
    let ret = unsafe { syscall0(SyscallNumber::NetworkPoll) };
    if ret >= 0 {
        Ok(ret as usize)
    } else {
        Err(ret)
    }
}

pub fn net_recvfrom(sock_id: u32, out_buf: &mut [u8]) -> Result<usize, i64> {
    let ret = unsafe {
        syscall3(
            SyscallNumber::NetRecvFrom,
            sock_id as u64,
            out_buf.as_mut_ptr() as u64,
            out_buf.len() as u64,
        )
    };
    if ret >= 0 {
        Ok(ret as usize)
    } else {
        Err(ret)
    }
}

/// Queries hardware battery, thermals, and throttling status.
pub fn resource_query() -> (u8, u8, bool) {
    let mut battery = 0u8;
    let mut temp = 0u8;
    let mut throttling = false;
    unsafe {
        syscall3(
            SyscallNumber::ResourceQuery,
            &mut battery as *mut u8 as u64,
            &mut temp as *mut u8 as u64,
            &mut throttling as *mut bool as u64,
        );
    }
    (battery, temp, throttling)
}

/// Performs a zero-trust pre-flight capability audit for a marketplace package.
/// Returns audit verdict (0 = Approved, 1 = RequiresConsent, 2 = BlockedForbidden, 3 = SignatureInvalid).
pub fn package_audit(pkg_id: &str) -> Result<u32, i64> {
    let ret = unsafe {
        syscall2(
            SyscallNumber::PackageAudit,
            pkg_id.as_ptr() as u64,
            pkg_id.len() as u64,
        )
    };
    if ret >= 0 {
        Ok(ret as u32)
    } else {
        Err(ret)
    }
}

/// Installs a verified marketplace package onto persistent storage.
/// Requires the caller to hold a `FileSystemWrite` capability in `cap_slot`.
pub fn package_install(pkg_id: &str, user_consent: bool) -> Result<(), i64> {
    let ret = unsafe {
        syscall4(
            SyscallNumber::PackageInstall,
            RESERVED_CAP_SLOT,
            pkg_id.as_ptr() as u64,
            pkg_id.len() as u64,
            user_consent as u64,
        )
    };
    if ret >= 0 {
        Ok(())
    } else {
        Err(ret)
    }
}

/// Rolls back a first-party package registry activation transaction.
pub fn package_rollback(pkg_id: &str, user_consent: bool) -> Result<(), i64> {
    let ret = unsafe {
        syscall4(
            SyscallNumber::PackageRollback,
            RESERVED_CAP_SLOT,
            pkg_id.as_ptr() as u64,
            pkg_id.len() as u64,
            user_consent as u64,
        )
    };
    if ret >= 0 {
        Ok(())
    } else {
        Err(ret)
    }
}

/// Reads the live compositor window registry.
pub fn window_list(out_buf: &mut [u8]) -> Result<usize, i64> {
    let ret = unsafe {
        syscall2(
            SyscallNumber::WindowList,
            out_buf.as_mut_ptr() as u64,
            out_buf.len() as u64,
        )
    };
    if ret >= 0 {
        Ok(ret as usize)
    } else {
        Err(ret)
    }
}

/// Reads the kernel package lifecycle registry. Requires FileSystemRead capability in cap_slot.
pub fn package_registry(out_buf: &mut [u8]) -> Result<usize, i64> {
    let ret = unsafe {
        syscall3(
            SyscallNumber::PackageRegistry,
            RESERVED_CAP_SLOT,
            out_buf.as_mut_ptr() as u64,
            out_buf.len() as u64,
        )
    };
    if ret >= 0 {
        Ok(ret as usize)
    } else {
        Err(ret)
    }
}

/// Verifies a signed `.rapp` package on disk and writes its manifest summary.
///
/// The summary is a newline-terminated, pipe-delimited line in the same shape
/// `package_registry` and `fs_list` produce, so one parser reads all three:
/// `package_id|major.minor.patch|payload_len`.
///
/// WHY a summary and not a verdict flag: verification either runs and the kernel
/// reports `IntegrityViolation`, or it does not and the caller sees a specific
/// error. An `Ok` here means the file's magic, digest, signature, publisher
/// trust and revocation state all passed, which is exactly what an installer
/// needs to decide whether to offer the install.
///
/// Requires the `FileSystemRead` capability in `cap_slot`.
pub fn verify_package(path: &str, out_buf: &mut [u8]) -> Result<usize, i64> {
    let ret = unsafe {
        syscall5(
            SyscallNumber::VerifyPackage,
            RESERVED_CAP_SLOT,
            path.as_ptr() as u64,
            path.len() as u64,
            out_buf.as_mut_ptr() as u64,
            out_buf.len() as u64,
        )
    };
    if ret >= 0 {
        Ok(ret as usize)
    } else {
        Err(ret)
    }
}

/// Submits WebAssembly bytecode to the kernel for execution.
///
/// WHY this always refuses instead of pretending: the kernel's `WasmExecute`
/// verb has no implementation behind it. There is no wasm binary decoder in
/// `kernel/src/wasm/` (its `WasmModule` is built in code, one function at a
/// time, not decoded from a `\0asm` module), no kernel-side VM instance store to
/// hold a submitted module between calls, and no `WasmExecute` capability in this
/// architecture's capability table to gate execution with. So the kernel answers
/// `FeatureDisabled`, and this wrapper returns it rather than manufacturing an
/// `Ok` that would have a live guest believe bytecode ran.
///
/// Calling it is honest reporting, not a supported path; a caller that wants to
/// run bytecode has to wait for the decoder, the instance store and the
/// capability described at the kernel arm.
pub fn wasm_execute(_module: &[u8], _export: &str) -> Result<(), i64> {
    Err(-19)
}

/// Queries active mobile SoC platform parameters into `out_buf` JSON string.
pub fn hal_query_platform(out_buf: &mut [u8]) -> Result<usize, i64> {
    let ret = unsafe {
        syscall2(
            SyscallNumber::HalQueryPlatform,
            out_buf.as_mut_ptr() as u64,
            out_buf.len() as u64,
        )
    };
    if ret >= 0 {
        Ok(ret as usize)
    } else {
        Err(ret)
    }
}

/// Queries the active mobile CPU DVFS power governor (0=Perf, 1=Powersave, 2=Interactive, 3=Schedutil).
pub fn power_get_governor() -> Result<u32, i64> {
    let ret = unsafe { syscall2(SyscallNumber::PowerGovernor, 0, 0) };
    if ret >= 0 {
        Ok(ret as u32)
    } else {
        Err(ret)
    }
}

/// Changes the active mobile CPU DVFS power governor.
pub fn power_set_governor(governor_id: u8) -> Result<(), i64> {
    let ret = unsafe { syscall2(SyscallNumber::PowerGovernor, 1, governor_id as u64) };
    if ret >= 0 {
        Ok(())
    } else {
        Err(ret)
    }
}

/// Queries the active cellular modem and SIM status into `out_buf` JSON string.
pub fn telephony_query(out_buf: &mut [u8]) -> Result<usize, i64> {
    let ret = unsafe {
        syscall2(
            SyscallNumber::TelephonyQuery,
            out_buf.as_mut_ptr() as u64,
            out_buf.len() as u64,
        )
    };
    if ret >= 0 {
        Ok(ret as usize)
    } else {
        Err(ret)
    }
}

/// Fills `out_buf` with cryptographically secure random bytes from the hardware TRNG.
pub fn get_random(out_buf: &mut [u8]) -> Result<usize, i64> {
    let ret = unsafe {
        syscall2(
            SyscallNumber::GetRandom,
            out_buf.as_mut_ptr() as u64,
            out_buf.len() as u64,
        )
    };
    if ret >= 0 {
        Ok(ret as usize)
    } else {
        Err(ret)
    }
}

/// Triggers a mobile haptic touch feedback click via PC Speaker.
pub fn audio_haptic_click() -> Result<(), i64> {
    let ret = unsafe { syscall2(SyscallNumber::AudioControl, 0, 0) };
    if ret >= 0 {
        Ok(())
    } else {
        Err(ret)
    }
}

/// Plays a smartphone notification chime.
pub fn audio_notification_chime() -> Result<(), i64> {
    let ret = unsafe { syscall2(SyscallNumber::AudioControl, 1, 0) };
    if ret >= 0 {
        Ok(())
    } else {
        Err(ret)
    }
}

/// Plays a custom frequency audio tone in Hertz.
pub fn audio_play_tone(freq_hz: u32) -> Result<(), i64> {
    let ret = unsafe { syscall2(SyscallNumber::AudioControl, 2, freq_hz as u64) };
    if ret >= 0 {
        Ok(())
    } else {
        Err(ret)
    }
}

/// Stops any active audio tone.
pub fn audio_silence() -> Result<(), i64> {
    let ret = unsafe { syscall2(SyscallNumber::AudioControl, 3, 0) };
    if ret >= 0 {
        Ok(())
    } else {
        Err(ret)
    }
}

/// Derives an application-isolated 32-byte cryptographic key from hardware key slot.
pub fn keystore_derive(slot_index: u32, out_buf: &mut [u8; 32]) -> Result<usize, i64> {
    let ret = unsafe {
        syscall3(
            SyscallNumber::KeystoreDerive,
            slot_index as u64,
            out_buf.as_mut_ptr() as u64,
            out_buf.len() as u64,
        )
    };
    if ret >= 0 {
        Ok(ret as usize)
    } else {
        Err(ret)
    }
}

/// Queries mobile hardware sensor telemetry (orientation, 3-axis accel, ambient lux, proximity).
pub fn sensor_read(out: &mut [u8]) -> Result<usize, i64> {
    let ret = unsafe {
        syscall2(
            SyscallNumber::SensorRead,
            out.as_mut_ptr() as u64,
            out.len() as u64,
        )
    };
    if ret >= 0 {
        Ok(ret as usize)
    } else {
        Err(ret)
    }
}

/// Controls the authoritative virtual-smartphone power path.
/// 0=restart, 1=power off, 2=toggle display, 3=volume up, 4=volume down, 5=display state.
pub fn device_power(action: u32) -> Result<u32, i64> { let ret = unsafe { syscall1(SyscallNumber::DevicePower, action as u64) }; if ret >= 0 { Ok(ret as u32) } else { Err(ret) } }

/// Queries mobile battery fuel gauge telemetry (voltage, SoC %, capacity, charging status).
pub fn audio_query_volume() -> Result<u32, i64> {
    let ret = unsafe { syscall2(SyscallNumber::AudioControl, 8, 0) };
    if ret >= 0 {
        Ok(ret as u32)
    } else {
        Err(ret)
    }
}

/// Sets the guest audio manager's master volume and returns the applied value.
pub fn audio_set_volume(volume_percent: u32) -> Result<u32, i64> {
    let ret = unsafe { syscall2(SyscallNumber::AudioControl, 7, volume_percent.min(100) as u64) };
    if ret >= 0 { Ok(ret as u32) } else { Err(ret) }
}

/// Packed format word meaning "the controller's native format": 48 kHz,
/// stereo, 16-bit. This is the only format the kernel's AC'97 path accepts.
pub const AUDIO_STREAM_FORMAT_NATIVE: u64 = 0;

/// Size of the status record written by [`audio_stream_status`].
pub const AUDIO_STREAM_STATUS_BYTES: usize = 40;

/// Honest snapshot of the PCM output path.
///
/// The flags say whether a card answered and whether a batch is running. The
/// counters are what the kernel moved (`frames_written`, `frames_consumed`,
/// `queued_frames`) or observed (`underruns`), not an estimate of what the
/// speaker produced.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct AudioStreamStatus {
    pub device_present: bool,
    pub device_running: bool,
    pub stream_open: bool,
    pub frames_written: u64,
    pub frames_consumed: u64,
    pub queued_frames: u64,
    pub underruns: u32,
    pub capacity_frames: u32,
    pub sample_rate: u32,
}

/// Opens the AC'97 PCM output stream and returns its id.
///
/// `cap_handle` is the generation-counted `AudioStream` handle from
/// [`capability_handle`]`(cap_type::AUDIO_STREAM, 0)`; the kernel refuses the
/// call without it. `format` is [`AUDIO_STREAM_FORMAT_NATIVE`] (0) or a word
/// packing the rate in bits 0..16, channel count in bits 16..24 and bit depth
/// in bits 24..32. Only 48 kHz stereo 16-bit is accepted by the current driver.
pub fn audio_stream_open(cap_handle: u64, format: u64) -> Result<u32, i64> {
    let ret = unsafe { syscall2(SyscallNumber::AudioStreamOpen, cap_handle, format) };
    if ret >= 0 { Ok(ret as u32) } else { Err(ret) }
}

/// Enqueues interleaved 16-bit stereo PCM frames through `cap_handle`.
///
/// The slice length must be a whole number of 4-byte frames. Returns the
/// number of bytes accepted, which is short when the bounded ring filled.
pub fn audio_stream_write(cap_handle: u64, pcm: &[u8]) -> Result<usize, i64> {
    let ret = unsafe {
        syscall3(
            SyscallNumber::AudioStreamWrite,
            cap_handle,
            pcm.as_ptr() as u64,
            pcm.len() as u64,
        )
    };
    if ret >= 0 { Ok(ret as usize) } else { Err(ret) }
}

/// Polls the controller and returns the measured stream state.
pub fn audio_stream_status(cap_handle: u64) -> Result<AudioStreamStatus, i64> {
    let mut record = [0u8; AUDIO_STREAM_STATUS_BYTES];
    let ret = unsafe {
        syscall3(
            SyscallNumber::AudioStreamStatus,
            cap_handle,
            record.as_mut_ptr() as u64,
            record.len() as u64,
        )
    };
    if ret < 0 {
        return Err(ret);
    }
    if ret as usize != AUDIO_STREAM_STATUS_BYTES {
        return Err(-3); // InvalidArguments: a short record cannot be decoded.
    }
    let read_u32 = |at: usize| {
        let mut bytes = [0u8; 4];
        bytes.copy_from_slice(&record[at..at + 4]);
        u32::from_le_bytes(bytes)
    };
    let read_u64 = |at: usize| {
        let mut bytes = [0u8; 8];
        bytes.copy_from_slice(&record[at..at + 8]);
        u64::from_le_bytes(bytes)
    };
    let flags = read_u32(28);
    Ok(AudioStreamStatus {
        device_present: flags & (1 << 0) != 0,
        device_running: flags & (1 << 1) != 0,
        stream_open: flags & (1 << 2) != 0,
        frames_written: read_u64(0),
        frames_consumed: read_u64(8),
        queued_frames: read_u64(16),
        underruns: read_u32(24),
        capacity_frames: read_u32(32),
        sample_rate: read_u32(36),
    })
}

/// Stops the controller and drops the PCM stream.
pub fn audio_stream_close(cap_handle: u64) -> Result<(), i64> {
    let ret = unsafe { syscall1(SyscallNumber::AudioStreamClose, cap_handle) };
    if ret >= 0 { Ok(()) } else { Err(ret) }
}

pub fn battery_query(out_buf: &mut [u8]) -> Result<usize, i64> {
    let ret = unsafe {
        syscall2(
            SyscallNumber::BatteryQuery,
            out_buf.as_mut_ptr() as u64,
            out_buf.len() as u64,
        )
    };
    if ret >= 0 {
        Ok(ret as usize)
    } else {
        Err(ret)
    }
}

/// Dispatches an Android cross-zone intent through the ACR mediator.
pub fn android_bridge_intent(
    action: &str,
    data_uri: &str,
    out_buf: &mut [u8],
) -> Result<usize, i64> {
    let ret = unsafe {
        syscall5(
            SyscallNumber::AndroidBridgeIntent,
            action.as_ptr() as u64,
            action.len() as u64,
            data_uri.as_ptr() as u64,
            data_uri.len() as u64,
            out_buf.as_mut_ptr() as u64,
        )
    };
    if ret >= 0 {
        Ok(ret as usize)
    } else {
        Err(ret)
    }
}

/// Queries current real-time clock (RTC / CMOS) wall-clock time into `out_buf` JSON string.
pub fn rtc_get_time(out_buf: &mut [u8]) -> Result<usize, i64> {
    let ret = unsafe {
        syscall2(
            SyscallNumber::RtcGetTime,
            out_buf.as_mut_ptr() as u64,
            out_buf.len() as u64,
        )
    };
    if ret >= 0 {
        Ok(ret as usize)
    } else {
        Err(ret)
    }
}

/// Queries multi-zone mobile thermal temperatures and cooling policy JSON string.
pub fn thermal_query(out_buf: &mut [u8]) -> Result<usize, i64> {
    let ret = unsafe {
        syscall2(
            SyscallNumber::ThermalQuery,
            out_buf.as_mut_ptr() as u64,
            out_buf.len() as u64,
        )
    };
    if ret >= 0 {
        Ok(ret as usize)
    } else {
        Err(ret)
    }
}

/// Canonical Capability Types for deterministic discovery via `cap_lookup`.
pub mod cap_type {
    pub const SERIAL_OUTPUT: u64 = 1;
    pub const FRAMEBUFFER_DIRECT: u64 = 2;
    pub const TIMER_TICK_READ: u64 = 3;
    pub const HARDWARE_PORT_IO: u64 = 4;
    pub const IPC_SEND: u64 = 5;
    pub const IPC_RECEIVE: u64 = 6;
    pub const PROCESS_SPAWN: u64 = 7;
    pub const PROCESS_KILL: u64 = 8;
    pub const FILE_SYSTEM_READ: u64 = 9;
    pub const FILE_SYSTEM_WRITE: u64 = 10;
    pub const STORAGE_RAW_BLOCK: u64 = 11;
    pub const NETWORK_SOCKET: u64 = 12;
    pub const NETWORK_SEND: u64 = 13;
    pub const NETWORK_RECEIVE: u64 = 14;
    pub const SHM_PRODUCER: u64 = 15;
    pub const SHM_CONSUMER: u64 = 16;
    pub const EVENT_POLL: u64 = 17;
    pub const SYSTEM_CONTROL: u64 = 18;
    pub const REALTIME_CLOCK_READ: u64 = 24;
    /// Permission to read the shared document index and the intent registry.
    ///
    /// WHY this is not `FILE_SYSTEM_READ`: the index *describes* documents --
    /// their paths, lengths, media types and identities -- and reading a
    /// description is not reading a file. An app that may browse the shared
    /// area's catalogue does not thereby gain the ability to open the
    /// documents in it.
    pub const CONTENT_INDEX: u64 = 25;
    /// Permission to dispatch an intent to the package that declared a
    /// handler for it.
    ///
    /// WHY this is not `PROCESS_SPAWN`: an intent names one relationship --
    /// "the package that declared this media type" -- and the kernel resolves
    /// it from the signed manifests. An app that may ask the device to open a
    /// document may not thereby spawn arbitrary programs.
    pub const CONTENT_INTENT: u64 = 26;
    /// Permission to read another client's display surface.
    ///
    /// WHY this is not `FRAMEBUFFER_DIRECT`: the framebuffer capability is the
    /// display service's own authority over the scanout, and it authorises
    /// writing the surfaces a task owns. Reading the pixels of a *running*
    /// task's surface is a separate question -- "may this task see another
    /// app's screen?" -- and it gets its own type so the answer is a
    /// capability decision rather than a side effect of holding display
    /// authority.
    pub const SURFACE_CAPTURE: u64 = 27;
    /// Permission to open, feed, observe and close the kernel's PCM output
    /// stream.
    ///
    /// WHY this is not `SYSTEM_CONTROL`: emitting samples is an output side
    /// effect, not power, telephony or device policy, so a media player can
    /// hold exactly this authority. The stream calls take the capability's
    /// generation-counted handle, which [`capability_handle`] resolves from
    /// the caller's own table.
    pub const AUDIO_STREAM: u64 = 28;
}

/// Dynamically discovers the slot index of an active capability matching `cap_type`
/// and optional `object_id`. If `object_id == 0`, any capability of that type matches.
pub fn cap_lookup(cap_type: u64, object_id: u64) -> Result<usize, i64> {
    let ret = unsafe {
        syscall2(
            SyscallNumber::CapLookup,
            cap_type,
            object_id,
        )
    };
    if ret >= 0 {
        Ok(ret as usize)
    } else {
        Err(ret)
    }
}

/// Resolves a capability the caller already holds to its generation-counted
/// handle, for syscalls that name an explicit `CapHandle` (the PCM stream).
///
/// This mints no authority: the kernel only answers with a handle for a
/// capability already present in the calling task's table, and the returned
/// handle becomes stale the moment that grant is revoked.
pub fn capability_handle(cap_type: u64, object_id: u64) -> Result<u64, i64> {
    let ret = unsafe {
        syscall2(
            SyscallNumber::CapHandleLookup,
            cap_type,
            object_id,
        )
    };
    if ret >= 0 {
        Ok(ret as u64)
    } else {
        Err(ret)
    }
}

/// Establishes a cryptographic secure IPC channel with a peer task.
pub fn secure_ipc_session(peer_task: u64, out_session_id: &mut u64) -> Result<(), i64> {
    let ret = unsafe {
        syscall2(
            SyscallNumber::SecureIpcSession,
            peer_task,
            out_session_id as *mut u64 as u64,
        )
    };
    if ret >= 0 {
        Ok(())
    } else {
        Err(ret)
    }
}

/// Encrypts a 512-byte sector using the sovereign encrypted storage vault.
pub fn vault_encrypt_sector(
    block_index: u64,
    buf: &mut [u8; 512],
    iv: &[u8; 16],
) -> Result<(), i64> {
    let ret = unsafe {
        syscall5(
            SyscallNumber::VaultCrypt,
            0,
            block_index,
            buf.as_mut_ptr() as u64,
            512,
            iv.as_ptr() as u64,
        )
    };
    if ret >= 0 {
        Ok(())
    } else {
        Err(ret)
    }
}

/// Decrypts a 512-byte sector using the sovereign encrypted storage vault.
pub fn vault_decrypt_sector(
    block_index: u64,
    buf: &mut [u8; 512],
    iv: &[u8; 16],
) -> Result<(), i64> {
    let ret = unsafe {
        syscall5(
            SyscallNumber::VaultCrypt,
            1,
            block_index,
            buf.as_mut_ptr() as u64,
            512,
            iv.as_ptr() as u64,
        )
    };
    if ret >= 0 {
        Ok(())
    } else {
        Err(ret)
    }
}

/// Queries mobile wireless HAL status (Wi-Fi and BLE beacon) into `out_buf` JSON string.
pub fn wireless_query(out_buf: &mut [u8]) -> Result<usize, i64> {
    let ret = unsafe {
        syscall2(
            SyscallNumber::WirelessQuery,
            out_buf.as_mut_ptr() as u64,
            out_buf.len() as u64,
        )
    };
    if ret >= 0 {
        Ok(ret as usize)
    } else {
        Err(ret)
    }
}

/// Reads bytes from a virtual character device node in `/dev/`.
pub fn devfs_read(path: &str, buf: &mut [u8]) -> Result<usize, i64> {
    let ret = unsafe {
        syscall5(
            SyscallNumber::DevFsIo,
            0,
            path.as_ptr() as u64,
            path.len() as u64,
            buf.as_mut_ptr() as u64,
            buf.len() as u64,
        )
    };
    if ret >= 0 {
        Ok(ret as usize)
    } else {
        Err(ret)
    }
}

/// Writes bytes to a virtual character device node in `/dev/`.
pub fn devfs_write(path: &str, buf: &[u8]) -> Result<usize, i64> {
    let ret = unsafe {
        syscall5(
            SyscallNumber::DevFsIo,
            1,
            path.as_ptr() as u64,
            path.len() as u64,
            buf.as_ptr() as u64,
            buf.len() as u64,
        )
    };
    if ret >= 0 {
        Ok(ret as usize)
    } else {
        Err(ret)
    }
}

/// Checks if a package is revoked on the sovereign Certificate Revocation List (CRL).
pub fn revocation_is_package_revoked(package_id: &str) -> Result<bool, i64> {
    let ret = unsafe {
        syscall5(
            SyscallNumber::RevocationAudit,
            0,
            package_id.as_ptr() as u64,
            package_id.len() as u64,
            0,
            0,
        )
    };
    if ret >= 0 {
        Ok(ret != 0)
    } else {
        Err(ret)
    }
}

/// Places a malicious or compromised package on the sovereign CRL.
pub fn revocation_revoke_package(package_id: &str) -> Result<(), i64> {
    let ret = unsafe {
        syscall5(
            SyscallNumber::RevocationAudit,
            1,
            package_id.as_ptr() as u64,
            package_id.len() as u64,
            0,
            0,
        )
    };
    if ret >= 0 {
        Ok(())
    } else {
        Err(ret)
    }
}

/// Delegates an attenuated capability token to a target task with usage quota and duration.
pub fn cap_delegate(
    cap_slot: usize,
    target_task: u64,
    requested_rights: u32,
    max_invocations: u32,
    duration_ticks: u64,
) -> Result<usize, i64> {
    let ret = unsafe {
        syscall5(
            SyscallNumber::CapDelegate,
            cap_slot as u64,
            target_task,
            requested_rights as u64,
            max_invocations as u64,
            duration_ticks,
        )
    };
    if ret >= 0 {
        Ok(ret as usize)
    } else {
        Err(ret)
    }
}

/// Interacts with the microkernel fluid inertial physics scroller (0=down, 1=move, 2=up, 3=tick).
/// Creates a surface through the privileged Ring-3 display service boundary.
pub fn surface_create(
    width: u32,
    height: u32,
    z_order: u32,
) -> Result<u32, i64> {
    let ret = unsafe {
        syscall5(
            SyscallNumber::SurfaceCreate,
            RESERVED_CAP_SLOT,
            width as u64,
            height as u64,
            z_order as u64,
            0,
        )
    };
    // WHY zero is an error and not a surface id: surface ids start at one,
    // so a zero return means the create was refused upstream, and handing
    // the app a surface it cannot write to only moves the failure from the
    // create to the first pixel write.
    if ret > 0 {
        Ok(ret as u32)
    } else {
        Err(ret)
    }
}

/// Writes ARGB8888 pixels through the display service boundary.
pub fn surface_write(
    surface_id: u32,
    offset: usize,
    pixels: &[u32],
) -> Result<usize, i64> {
    let ret = unsafe {
        syscall5(
            SyscallNumber::SurfaceWrite,
            RESERVED_CAP_SLOT,
            surface_id as u64,
            offset as u64,
            pixels.as_ptr() as u64,
            pixels.len() as u64,
        )
    };
    if ret >= 0 {
        Ok(ret as usize)
    } else {
        Err(ret)
    }
}

/// Reads a running client's display surface, scaled, into the caller's buffer.
///
/// The surface does not have to belong to the caller: that is the point of
/// the call, and the reason it is a separate capability. The kernel refuses
/// the read unless the caller holds the `SurfaceCapture` capability, so
/// "may this task see another app's screen" is answered by the capability
/// table and not by the identity of the caller.
///
/// `out` receives `dest_width * dest_height` ARGB8888 pixels, box-filtered
/// from the source surface. The kernel returns:
/// * the number of pixels written, on success;
/// * `PermissionDenied` (`-1`) when the caller holds no capture capability;
/// * `TargetNotFound` (`-6`) when no surface carries that id;
/// * `NotReady` (`-30`) when the surface has not drawn a frame yet.
pub fn surface_capture(
    surface_id: u32,
    dest_width: u32,
    dest_height: u32,
    out: &mut [u32],
) -> Result<usize, i64> {
    if dest_width == 0 || dest_height == 0 {
        return Err(-3);
    }
    let needed = (dest_width as usize).saturating_mul(dest_height as usize);
    if out.len() < needed {
        return Err(-3);
    }
    let ret = unsafe {
        syscall5(
            SyscallNumber::SurfaceCapture,
            surface_id as u64,
            dest_width as u64,
            dest_height as u64,
            out.as_mut_ptr() as u64,
            needed as u64,
        )
    };
    if ret >= 0 {
        Ok(ret as usize)
    } else {
        Err(ret)
    }
}

/// Destroys a surface owned by the display service.
pub fn surface_destroy(surface_id: u32) -> Result<(), i64> {
    let ret = unsafe {
        syscall2(
            SyscallNumber::SurfaceDestroy,
            RESERVED_CAP_SLOT,
            surface_id as u64,
        )
    };
    if ret == 0 {
        Ok(())
    } else {
        Err(ret)
    }
}

/// Composites and atomically flips the display scanout.
pub fn surface_flip() -> Result<u64, i64> {
    let ret = unsafe { syscall5(SyscallNumber::SurfaceFlip, RESERVED_CAP_SLOT, 0, 0, 0, 0) };
    if ret >= 0 {
        Ok(ret as u64)
    } else {
        Err(ret)
    }
}

/// Enables or disables pointer/touch hit-testing for an owned window.
pub fn window_set_input(window_id: u32, enabled: bool) -> Result<(), i64> {
    let ret = unsafe {
        syscall3(
            SyscallNumber::WindowSetInput,
            RESERVED_CAP_SLOT,
            window_id as u64,
            enabled as u64,
        )
    };
    if ret == 0 {
        Ok(())
    } else {
        Err(ret)
    }
}

/// Reprograms the scanout resolution and refresh rate.
///
/// `display_cap_slot` is the slot holding the `FrameBufferDirect` capability,
/// exactly as `surface_create` and `surface_flip` take it: the mode is hardware
/// state, so it is addressable only by the display service, never by an app.
/// A width or height that does not fit the hardware's 16-bit geometry registers,
/// or a refresh the panel rejects, comes back as `InvalidArguments` and the
/// previous mode stays in effect.
pub fn display_set_mode(
    width: u32,
    height: u32,
    refresh_hz: u16,
) -> Result<(), i64> {
    let ret = unsafe {
        syscall5(
            SyscallNumber::DisplaySetMode,
            RESERVED_CAP_SLOT,
            width as u64,
            height as u64,
            refresh_hz as u64,
            0,
        )
    };
    if ret == 0 {
        Ok(())
    } else {
        Err(ret)
    }
}

/// Rotates the composited scanout.
///
/// Why this takes a raw `u32` code instead of a Rust enum: this crate is
/// `#![no_std]` and is consumed by every arch, and the codes are the kernel's
/// `DisplayOrientation` declaration order. A caller that invents a code outside
/// 0..=3 gets `InvalidArguments` rather than a silent no-op.
///
/// 0 = Portrait, 1 = LandscapeRight, 2 = PortraitInverted, 3 = LandscapeLeft.
pub fn display_set_orientation(orientation: u32) -> Result<(), i64> {
    let ret = unsafe {
        syscall5(
            SyscallNumber::DisplaySetOrientation,
            RESERVED_CAP_SLOT,
            orientation as u64,
            0,
            0,
            0,
        )
    };
    if ret == 0 {
        Ok(())
    } else {
        Err(ret)
    }
}

/// Normalized input event returned by the kernel input pipeline.
#[derive(Clone, Copy, Debug)]
pub struct InputEvent {
    pub kind: u8,
    pub x: u32,
    pub y: u32,
    pub dx: i32,
    pub dy: i32,
    pub scale_milli: u32,
}

/// Kind value used by the kernel's normalized input pipeline for keyboard events.
pub const INPUT_EVENT_KIND_KEY: u8 = 11;

/// Resolves a capability slot by the capability's type.
///
/// WHY this exists: a slot number is a position in a table, and the
/// position depends on the order the task's capabilities were inserted
/// in. Two apps with the same capabilities can number them differently,
/// and a number that is right in one app is wrong in the next. The
/// consequences were not subtle -- an app reading a file through the
/// slot that held its write capability, and a keystore call addressed
/// to the slot that held the serial port.
///
/// Every capability-taking syscall wants one of these. None of them
/// wants a number.
pub mod cap {
    use super::cap_lookup;
    use super::cap_type;

    /// The slot holding a filesystem read capability.
    #[inline]
    pub fn fs_read_slot() -> Option<usize> {
        cap_lookup(cap_type::FILE_SYSTEM_READ, 0).ok()
    }

    /// The slot holding a filesystem write capability.
    #[inline]
    pub fn fs_write_slot() -> Option<usize> {
        cap_lookup(cap_type::FILE_SYSTEM_WRITE, 0).ok()
    }

    /// The slot holding a direct framebuffer capability.
    #[inline]
    pub fn framebuffer_slot() -> Option<usize> {
        cap_lookup(cap_type::FRAMEBUFFER_DIRECT, 0).ok()
    }

    /// The slot holding a timer tick read capability.
    #[inline]
    pub fn timer_slot() -> Option<usize> {
        cap_lookup(cap_type::TIMER_TICK_READ, 0).ok()
    }

    /// The slot holding an event poll capability.
    #[inline]
    pub fn event_poll_slot() -> Option<usize> {
        cap_lookup(cap_type::EVENT_POLL, 0).ok()
    }

    /// The slot holding a network socket capability.
    #[inline]
    pub fn network_socket_slot() -> Option<usize> {
        cap_lookup(cap_type::NETWORK_SOCKET, 0).ok()
    }

    /// The slot holding a network send capability.
    #[inline]
    pub fn network_send_slot() -> Option<usize> {
        cap_lookup(cap_type::NETWORK_SEND, 0).ok()
    }

    /// The slot holding a network receive capability.
    #[inline]
    pub fn network_receive_slot() -> Option<usize> {
        cap_lookup(cap_type::NETWORK_RECEIVE, 0).ok()
    }

    /// The slot holding a real-time clock read capability.
    #[inline]
    pub fn rtc_slot() -> Option<usize> {
        cap_lookup(cap_type::REALTIME_CLOCK_READ, 0).ok()
    }
}

/// Reserved key codes used by the normalized keyboard pipeline.
pub mod input_key_code {
    pub const ENTER: u32 = 0x10000;
    pub const ESCAPE: u32 = 0x10001;
    pub const BACKSPACE: u32 = 0x10002;
    pub const TAB: u32 = 0x10003;
    pub const ARROW_UP: u32 = 0x10004;
    pub const ARROW_DOWN: u32 = 0x10005;
    pub const ARROW_LEFT: u32 = 0x10006;
    pub const ARROW_RIGHT: u32 = 0x10007;
    pub const HOME: u32 = 0x10008;
    pub const END: u32 = 0x10009;
    pub const PAGE_UP: u32 = 0x1000A;
    pub const PAGE_DOWN: u32 = 0x1000B;
    pub const SCREENSHOT: u32 = 0x1000C;
    pub const VOLUME_UP: u32 = 0x1000D;
    pub const VOLUME_DOWN: u32 = 0x1000E;
    pub const POWER: u32 = 0x1000F;
}

/// Keyboard event decoded from the normalized 24-byte input record.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InputKeyEvent {
    pub code: u32,
    pub pressed: bool,
    pub shift: bool,
    pub ctrl: bool,
    pub alt: bool,
    pub meta: bool,
}

impl InputEvent {
    /// Returns a keyboard event when this normalized input record is a key event.
    /// The x field carries the code point/key code, y carries key-down state,
    /// and scale_milli carries modifier bits for this event kind.
    pub fn key_event(&self) -> Option<InputKeyEvent> {
        if self.kind != INPUT_EVENT_KIND_KEY {
            return None;
        }
        let modifiers = self.scale_milli;
        Some(InputKeyEvent {
            code: self.x,
            pressed: self.y != 0,
            shift: modifiers & 1 != 0,
            ctrl: modifiers & 2 != 0,
            alt: modifiers & 4 != 0,
            meta: modifiers & 8 != 0,
        })
    }
}

/// Polls the normalized touch/input event pipeline.
pub fn input_event_poll(out: &mut InputEvent) -> Result<(), i64> {
    let mut buf = [0u8; 24];
    let ret = unsafe {
        syscall2(
            SyscallNumber::InputEventPoll,
            buf.as_mut_ptr() as u64,
            buf.len() as u64,
        )
    };
    if ret < 0 {
        return Err(ret);
    }
    if ret != 24 {
        return Err(-3);
    }
    out.kind = buf[0];
    out.x = u32::from_le_bytes(buf[4..8].try_into().unwrap());
    out.y = u32::from_le_bytes(buf[8..12].try_into().unwrap());
    out.dx = i32::from_le_bytes(buf[12..16].try_into().unwrap());
    out.dy = i32::from_le_bytes(buf[16..20].try_into().unwrap());
    out.scale_milli = u32::from_le_bytes(buf[20..24].try_into().unwrap());
    Ok(())
}

pub fn surface_physics(action: u32, y: i32, viewport_h: i32, content_h: i32) -> Result<i32, i64> {
    let ret = unsafe {
        syscall4(
            SyscallNumber::SurfacePhysics,
            action as u64,
            y as u64,
            viewport_h as u64,
            content_h as u64,
        )
    };
    if ret >= 0 {
        Ok(ret as i32)
    } else {
        Err(ret)
    }
}

pub const FS_READ: u32 = 1;
pub const FS_WRITE: u32 = 2;

/// Opens a file in the Virtual Filesystem with requested rights.
pub fn fs_open(path: &str, rights: u32) -> Result<u32, i64> {
    let ret = unsafe {
        syscall4(
            SyscallNumber::FsOpen,
            RESERVED_CAP_SLOT,
            path.as_ptr() as u64,
            path.len() as u64,
            rights as u64,
        )
    };
    if ret >= 0 {
        Ok(ret as u32)
    } else {
        Err(ret)
    }
}

/// Moves the calling application to background execution.
pub fn app_focus(task_id: u64) -> Result<(), i64> {
    let ret = unsafe { syscall4(SyscallNumber::AppFocus, task_id, 0, 0, 0) };
    if ret == 0 {
        Ok(())
    } else {
        Err(ret)
    }
}

pub fn app_runtime_list(out: &mut [u8]) -> Result<usize, i64> {
    let ret = unsafe {
        syscall4(
            SyscallNumber::AppRuntimeList,
            out.as_mut_ptr() as u64,
            out.len() as u64,
            0,
            0,
        )
    };
    if ret >= 0 {
        Ok(ret as usize)
    } else {
        Err(ret)
    }
}

pub fn app_stop(task_id: u64) -> Result<(), i64> {
    let ret = unsafe { syscall4(SyscallNumber::AppStop, task_id, 0, 0, 0) };
    if ret == 0 {
        Ok(())
    } else {
        Err(ret)
    }
}

pub fn app_restart(package_id: &str) -> Result<u64, i64> {
    let ret = unsafe {
        syscall4(
            SyscallNumber::AppRestart,
            package_id.as_ptr() as u64,
            package_id.len() as u64,
            0,
            0,
        )
    };
    if ret >= 0 {
        Ok(ret as u64)
    } else {
        Err(ret)
    }
}

pub fn package_uninstall(package_id: &str, user_consent: bool) -> Result<(), i64> {
    let ret = unsafe {
        syscall4(
            SyscallNumber::PackageUninstall,
            RESERVED_CAP_SLOT,
            package_id.as_ptr() as u64,
            package_id.len() as u64,
            user_consent as u64,
        )
    };
    if ret == 0 {
        Ok(())
    } else {
        Err(ret)
    }
}

pub fn app_background() -> Result<(), i64> {
    app_background_task(0)
}

/// Moves the specified application task to the background, suspending its camera,
/// microphone, and network DMA tokens unless explicit background permission is active.
pub fn app_background_task(task_id: u64) -> Result<(), i64> {
    let ret = unsafe { syscall4(SyscallNumber::AppBackground, task_id, 0, 0, 0) };
    if ret == 0 {
        Ok(())
    } else {
        Err(ret)
    }
}

/// Returns the calling application to foreground execution.
pub fn app_foreground() -> Result<(), i64> {
    let ret = unsafe { syscall4(SyscallNumber::AppForeground, 0, 0, 0, 0) };
    if ret == 0 {
        Ok(())
    } else {
        Err(ret)
    }
}

/// Suspends the calling application until an explicit wake event.
pub fn app_suspend() -> Result<(), i64> {
    let ret = unsafe { syscall4(SyscallNumber::AppSuspend, 0, 0, 0, 0) };
    if ret == 0 {
        Ok(())
    } else {
        Err(ret)
    }
}

/// Requests wake of the calling application.
pub fn app_wake() -> Result<(), i64> {
    let ret = unsafe { syscall4(SyscallNumber::AppWake, 0, 0, 0, 0) };
    if ret == 0 {
        Ok(())
    } else {
        Err(ret)
    }
}

/// Opens a file inside the calling application's private storage namespace.
pub fn app_storage_open(relative_path: &str, rights: u32) -> Result<u32, i64> {
    let ret = unsafe {
        syscall4(
            SyscallNumber::AppStorageOpen,
            RESERVED_CAP_SLOT,
            relative_path.as_ptr() as u64,
            relative_path.len() as u64,
            rights as u64,
        )
    };
    if ret >= 0 {
        Ok(ret as u32)
    } else {
        Err(ret)
    }
}

/// Reads data from an open file descriptor.
pub fn fs_read(fd: u32, buf: &mut [u8]) -> Result<usize, i64> {
    let ret = unsafe {
        syscall4(
            SyscallNumber::FsRead,
            RESERVED_CAP_SLOT,
            fd as u64,
            buf.as_mut_ptr() as u64,
            buf.len() as u64,
        )
    };
    if ret >= 0 {
        Ok(ret as usize)
    } else {
        Err(ret)
    }
}

/// Writes data to an open file descriptor.
pub fn fs_write(fd: u32, buf: &[u8]) -> Result<usize, i64> {
    let ret = unsafe {
        syscall4(
            SyscallNumber::FsWrite,
            RESERVED_CAP_SLOT,
            fd as u64,
            buf.as_ptr() as u64,
            buf.len() as u64,
        )
    };
    if ret >= 0 {
        Ok(ret as usize)
    } else {
        Err(ret)
    }
}

/// Closes an open file descriptor.
pub fn fs_close(fd: u32) -> Result<(), i64> {
    let ret = unsafe { syscall1(SyscallNumber::FsClose, fd as u64) };
    if ret == 0 {
        Ok(())
    } else {
        Err(ret)
    }
}

/// Flushes an open file's data and metadata to persistent storage.
///
/// Writes already flush before they report success, so this is a barrier for
/// callers that need one before a rename, a shutdown or a power transition.
/// A flush failure is reported rather than ignored: silently discarding it would
/// claim a durability the device does not have.
pub fn fs_fsync(fd: u32) -> Result<(), i64> {
    let ret = unsafe { syscall1(SyscallNumber::FsSync, fd as u64) };
    if ret == 0 {
        Ok(())
    } else {
        Err(ret)
    }
}

/// Unlinks (deletes) a file from the Virtual Filesystem.
pub fn fs_unlink(path: &str) -> Result<(), i64> {
    let ret = unsafe {
        syscall4(
            SyscallNumber::FsUnlink,
            RESERVED_CAP_SLOT,
            path.as_ptr() as u64,
            path.len() as u64,
            0,
        )
    };
    if ret == 0 {
        Ok(())
    } else {
        Err(ret)
    }
}

/// Size of the fixed-layout `FsStat` record the kernel writes back.
///
/// WHY these byte offsets rather than a `#[repr(C)]` mirror of the kernel struct:
/// the offsets have to stay checkable on both sides of the boundary, and the
/// parity contract pins `SyscallNumber` rather than kernel structs. Every field
/// below is little-endian.
///
/// `0..8 inode | 8..12 uid | 12..16 gid | 16..18 mode | 18..20 nlink |
/// 20..28 atime | 28..36 mtime | 36..44 ctime | 44..48 file type | 48..56 read
/// only | 56..88 sha256 | 88..96 size`
///
/// `file type` and `read only` are written as 4-byte and 8-byte fields because
/// the kernel's `FileStat` uses those types; the meaningful value is in the first
/// byte of each, so the accessors below read `buf[44]` and `buf[48]` and a tag
/// wider than one byte is still consumed correctly.
pub const FILE_STAT_BYTES: usize = 96;

/// A `FileStat` record as the kernel wrote it, decoded from `fs_stat`'s buffer.
///
/// WHY a struct and not `fs_stat` returning it directly: `fs_stat` is a syscall
/// bound to a 96-byte out buffer, and a caller has to know whether its buffer
/// held the whole record. `fs_stat` returns the byte count it wrote, so a short
/// read is visible; this type only decodes a complete one and refuses a short or
/// corrupt buffer rather than filling the missing fields with zeroes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FileStatRecord {
    pub inode: u64,
    pub uid: u32,
    pub gid: u32,
    pub mode: u16,
    pub nlink: u16,
    /// 0 = regular file, 1 = directory, 2 = read-only system, 3 = device node.
    pub file_type: u8,
    pub atime: u64,
    pub mtime: u64,
    pub ctime: u64,
    pub read_only: bool,
    pub sha256: [u8; 32],
    pub size: u64,
}

impl FileStatRecord {
    /// Decodes the record, or returns `None` for a short or corrupt buffer.
    ///
    /// WHY every field is included: a caller that only wants a size can ignore
    /// the rest, but one that needs the digest to cross-check the bytes it read
    /// has to be able to get it, and a stat that hides its own digest is a stat
    /// the caller has to take on faith.
    pub fn parse(buf: &[u8]) -> Option<FileStatRecord> {
        use core::convert::TryInto;
        if buf.len() < FILE_STAT_BYTES {
            return None;
        }
        Some(FileStatRecord {
            inode: u64::from_le_bytes(buf[0..8].try_into().ok()?),
            uid: u32::from_le_bytes(buf[8..12].try_into().ok()?),
            gid: u32::from_le_bytes(buf[12..16].try_into().ok()?),
            mode: u16::from_le_bytes(buf[16..18].try_into().ok()?),
            nlink: u16::from_le_bytes(buf[18..20].try_into().ok()?),
            atime: u64::from_le_bytes(buf[20..28].try_into().ok()?),
            mtime: u64::from_le_bytes(buf[28..36].try_into().ok()?),
            ctime: u64::from_le_bytes(buf[36..44].try_into().ok()?),
            file_type: buf[44],
            read_only: buf[48] != 0,
            sha256: {
                let mut digest = [0u8; 32];
                digest.copy_from_slice(&buf[56..88]);
                digest
            },
            size: u64::from_le_bytes(buf[88..96].try_into().ok()?),
        })
    }
}

/// Reads a path's metadata without opening a file descriptor.
///
/// `cap_slot` must hold the `FileSystemRead` capability the VFS checks, and
/// `out` must be at least `FILE_STAT_BYTES` bytes: a shorter buffer is refused
/// rather than partially decoded, because a truncated stat would report a short
/// size and a short digest that both look self-consistent.
pub fn fs_stat(path: &str, out: &mut [u8]) -> Result<usize, i64> {
    if out.len() < FILE_STAT_BYTES {
        return Err(-3);
    }
    let ret = unsafe {
        syscall5(
            SyscallNumber::FsStat,
            RESERVED_CAP_SLOT,
            path.as_ptr() as u64,
            path.len() as u64,
            out.as_mut_ptr() as u64,
            out.len().min(FILE_STAT_BYTES) as u64,
        )
    };
    if ret >= 0 {
        Ok(ret as usize)
    } else {
        Err(ret)
    }
}

/// Audits an Android APK package, decodes its binary manifest, and maps permissions to
/// Inter OS zero-trust capabilities.
pub fn apk_install_audit(apk_data: &[u8], out_buf: &mut [u8]) -> Result<usize, i64> {
    let ret = unsafe {
        syscall4(
            SyscallNumber::ApkInstallAudit,
            apk_data.as_ptr() as u64,
            apk_data.len() as u64,
            out_buf.as_mut_ptr() as u64,
            out_buf.len() as u64,
        )
    };
    if ret >= 0 {
        Ok(ret as usize)
    } else {
        Err(ret)
    }
}

/// Audits and verifies an Android Dalvik Executable (classes.dex) bytecode payload.
/// Rejects forbidden reflection, process spawning, or private host filesystem escapes.
pub fn dex_verify_exec(dex_data: &[u8]) -> Result<u32, i64> {
    let ret = unsafe {
        syscall2(
            SyscallNumber::DexVerifyExec,
            dex_data.as_ptr() as u64,
            dex_data.len() as u64,
        )
    };
    if ret >= 0 {
        Ok(ret as u32)
    } else {
        Err(ret)
    }
}

/// Captures a raw preview frame from the mobile camera sensor and runs the ISP & scene classifier.
/// Returns the number of pixel bytes written to `preview_buf`.
pub fn camera_capture(preview_buf: &mut [u8], meta_out: &mut [u32; 4]) -> Result<usize, i64> {
    let ret = unsafe {
        syscall4(
            SyscallNumber::CameraCapture,
            0,
            preview_buf.as_mut_ptr() as u64,
            preview_buf.len() as u64,
            meta_out.as_mut_ptr() as u64,
        )
    };
    if ret >= 0 {
        Ok(ret as usize)
    } else {
        Err(ret)
    }
}

/// Toggles the mobile camera LED flash torch state.
pub fn camera_autofocus_state() -> Result<bool, i64> {
    let mut dummy = [0u8; 1];
    let ret = unsafe {
        syscall4(
            SyscallNumber::CameraCapture,
            2,
            dummy.as_mut_ptr() as u64,
            dummy.len() as u64,
            0,
        )
    };
    if ret >= 0 {
        Ok(ret != 0)
    } else {
        Err(ret)
    }
}

pub fn camera_toggle_flash() -> Result<bool, i64> {
    let mut dummy = [0u8; 1];
    let ret = unsafe {
        syscall4(
            SyscallNumber::CameraCapture,
            3,
            dummy.as_mut_ptr() as u64,
            1,
            0,
        )
    };
    if ret >= 0 {
        Ok(ret == 1)
    } else {
        Err(ret)
    }
}

/// Invokes a verified Dalvik method on a parsed DEX binary inside the sandboxed ACR Virtual Machine.
/// Format: `method_spec` = "Lpackage/Class;->methodName"
pub fn dex_invoke_method(dex_data: &[u8], method_spec: &str) -> Result<u32, i64> {
    let ret = unsafe {
        syscall4(
            SyscallNumber::DexInvokeMethod,
            dex_data.as_ptr() as u64,
            dex_data.len() as u64,
            method_spec.as_ptr() as u64,
            method_spec.len() as u64,
        )
    };
    if ret >= 0 {
        Ok(ret as u32)
    } else {
        Err(ret)
    }
}

/// Sensitive hardware and system resource categories requiring user capability consent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum SensitivePermission {
    TelephonyModem = 1,
    StorageSectorCrypt = 2,
    NetworkSend = 3,
    CameraCapture = 4,
    AudioSynthesis = 5,
    RtcClock = 6,
}

/// Requests dynamic capability consent from the kernel Permission Manager and Mobile Shell.
/// Returns the assigned capability slot index on success.
pub fn cap_request_consent(
    perm: SensitivePermission,
    app_name: &str,
    auto_approve: bool,
) -> Result<u32, i64> {
    let ret = unsafe {
        syscall4(
            SyscallNumber::CapRequestConsent,
            perm as u64,
            app_name.as_ptr() as u64,
            app_name.len() as u64,
            if auto_approve { 1 } else { 0 },
        )
    };
    if ret >= 0 {
        Ok(ret as u32)
    } else {
        Err(ret)
    }
}

/// Dispatches a share transaction to a recipient app or triggers the interactive share drawer.
pub fn share_sheet_dispatch(
    mime_type: &str,
    uri: &str,
    target_app: Option<&str>,
) -> Result<u64, i64> {
    let mut target_buf = [0u8; 64];
    let target_ptr = if let Some(target) = target_app {
        let len = target.len().min(63);
        target_buf[..len].copy_from_slice(&target.as_bytes()[..len]);
        target_buf[len] = 0;
        target_buf.as_ptr() as u64
    } else {
        0
    };

    let ret = unsafe {
        syscall5(
            SyscallNumber::ShareSheetDispatch,
            mime_type.as_ptr() as u64,
            mime_type.len() as u64,
            uri.as_ptr() as u64,
            uri.len() as u64,
            target_ptr,
        )
    };
    if ret >= 0 {
        Ok(ret as u64)
    } else {
        Err(ret)
    }
}

pub use design::display_list::PIPE_COMPOSITOR_DISPLAY_LIST;

/// Creates or configures a Zero-Copy Shared Memory DMA Pipe.
pub fn shm_pipe_create(pipe_id: u32, pages: u32, peer_task: u32) -> Result<u32, i64> {
    let ret = unsafe {
        syscall3(
            SyscallNumber::ShmPipeCreate,
            pipe_id as u64,
            pages as u64,
            peer_task as u64,
        )
    };
    if ret >= 0 {
        Ok(ret as u32)
    } else {
        Err(ret)
    }
}

/// Streams high-throughput zero-copy data into a Shared Memory DMA Pipe.
pub fn shm_pipe_transfer(pipe_id: u32, data: &[u8], flags: u32) -> Result<u64, i64> {
    let ret = unsafe {
        syscall4(
            SyscallNumber::ShmPipeTransfer,
            pipe_id as u64,
            data.as_ptr() as u64,
            data.len() as u64,
            flags as u64,
        )
    };
    if ret >= 0 {
        Ok(ret as u64)
    } else {
        Err(ret)
    }
}

/// Receives high-throughput zero-copy data from a Shared Memory Pipe.
pub fn shm_pipe_receive(pipe_id: u32, out_buf: &mut [u8]) -> Result<usize, i64> {
    let ret = unsafe {
        syscall3(
            SyscallNumber::ShmPipeReceive,
            pipe_id as u64,
            out_buf.as_mut_ptr() as u64,
            out_buf.len() as u64,
        )
    };
    if ret >= 0 {
        Ok(ret as usize)
    } else {
        Err(ret)
    }
}

// -----------------------------------------------------------------------------
// Sovereign Asynchronous Event Reactor Bitmasks & Syscall Wrapper
// -----------------------------------------------------------------------------

/// Inbound IPC mailbox message available.
pub const EVENT_IPC_MSG: u32 = 1 << 0; // 0x01
/// Zero-Copy DMA Shared Memory pipe has data available to consume.
pub const EVENT_SHM_READABLE: u32 = 1 << 1; // 0x02
/// Zero-Copy DMA Shared Memory pipe has free slots available to stream.
pub const EVENT_SHM_WRITABLE: u32 = 1 << 2; // 0x04
/// Hardware system timer interval tick or deadline elapsed.
pub const EVENT_TIMER_TICK: u32 = 1 << 3; // 0x08
/// Capacitive touch digitizer input event frame available.
pub const EVENT_TOUCH_INPUT: u32 = 1 << 4; // 0x10
/// Inbound network socket datagram or packet received.
pub const EVENT_NETWORK_PACKET: u32 = 1 << 5; // 0x20
/// Cellular baseband modem unsolicited result code (URC) arrived.
pub const EVENT_MODEM_URC: u32 = 1 << 6; // 0x40

/// Polls for asynchronous system events with cooperative wait-queue blocking.
pub fn event_poll(interest_mask: u32, timeout_ticks: u32) -> Result<u32, i64> {
    let ret = unsafe {
        syscall2(
            SyscallNumber::EventPoll,
            interest_mask as u64,
            timeout_ticks as u64,
        )
    };
    if ret >= 0 {
        Ok(ret as u32)
    } else {
        Err(ret)
    }
}

/// Initiates an outgoing cellular call (`ATD<number>;`).
pub fn telephony_dial(number: &str) -> Result<(), i64> {
    let ret = unsafe {
        syscall2(
            SyscallNumber::TelephonyDial,
            number.as_ptr() as u64,
            number.len() as u64,
        )
    };
    if ret == 0 {
        Ok(())
    } else {
        Err(ret)
    }
}

/// Terminates an active cellular call (`ATH`).
pub fn telephony_hangup() -> Result<(), i64> {
    let ret = unsafe { syscall0(SyscallNumber::TelephonyHangup) };
    if ret == 0 {
        Ok(())
    } else {
        Err(ret)
    }
}

/// Transmits an SMS message to a destination phone number (`AT+CMGS`).
pub fn telephony_send_sms(destination: &str, body: &str) -> Result<u32, i64> {
    let ret = unsafe {
        syscall4(
            SyscallNumber::TelephonySendSms,
            destination.as_ptr() as u64,
            destination.len() as u64,
            body.as_ptr() as u64,
            body.len() as u64,
        )
    };
    if ret >= 0 {
        Ok(ret as u32)
    } else {
        Err(ret)
    }
}

/// Toggles cellular packet data connection (PDP context attach/detach).
pub fn telephony_set_data(enabled: bool) -> Result<bool, i64> {
    let ret = unsafe { syscall1(SyscallNumber::TelephonySetData, if enabled { 1 } else { 0 }) };
    if ret >= 0 {
        Ok(ret != 0)
    } else {
        Err(ret)
    }
}

/// Polls recognized gestures from the multi-touch digitizer engine.
pub fn touch_poll() -> Result<u32, i64> {
    let ret = unsafe { syscall0(SyscallNumber::TouchPoll) };
    if ret >= 0 {
        Ok(ret as u32)
    } else {
        Err(ret)
    }
}

/// Sets the operational DVFS power performance profile.
pub fn power_set_profile(profile_idx: u32) -> Result<u32, i64> {
    let ret = unsafe { syscall1(SyscallNumber::PowerSetProfile, profile_idx as u64) };
    if ret >= 0 {
        Ok(ret as u32)
    } else {
        Err(ret)
    }
}

pub fn package_list(out_buf: &mut [u8]) -> Result<usize, i64> {
    let ret = unsafe {
        syscall3(
            SyscallNumber::PackageList,
            RESERVED_CAP_SLOT,
            out_buf.as_mut_ptr() as u64,
            out_buf.len() as u64,
        )
    };
    if ret >= 0 {
        Ok(ret as usize)
    } else {
        Err(ret)
    }
}

/// Moves an owned window in the compositor.
pub fn window_set_geometry(
    id: u32,
    x: i32,
    y: i32,
    z: u32,
    visible: bool,
) -> Result<(), i64> {
    let packed = (z as u64) | if visible { 1u64 << 63 } else { 0 };
    let r = unsafe {
        syscall5(
            SyscallNumber::WindowSetGeometry,
            RESERVED_CAP_SLOT,
            id as u64,
            x as i64 as u64,
            y as i64 as u64,
            packed,
        )
    };
    if r == 0 {
        Ok(())
    } else {
        Err(r)
    }
}
/// Focuses the topmost live surface through the display service.
pub fn window_focus(x: u32, y: u32) -> Result<(), i64> {
    let r = unsafe {
        syscall3(
            SyscallNumber::WindowFocus,
            RESERVED_CAP_SLOT,
            x as u64,
            y as u64,
        )
    };
    if r == 0 {
        Ok(())
    } else {
        Err(r)
    }
}

/// Verifies a signed Ring-3 system-service RAPP and returns its service ID.
/// Requires the PID1 service-manager authority and a FileSystemRead capability.
pub fn service_package_verify(path: &str) -> Result<u8, i64> {
    let r = unsafe {
        syscall3(
            SyscallNumber::ServicePackageVerify,
            RESERVED_CAP_SLOT,
            path.as_ptr() as u64,
            path.len() as u64,
        )
    };
    if r >= 0 {
        Ok(r as u8)
    } else {
        Err(r)
    }
}

/// Publishes a heartbeat for the current registered userspace service.
pub fn service_heartbeat(service_id: u8) -> Result<(), i64> {
    let r = unsafe { syscall1(SyscallNumber::ServiceHeartbeat, service_id as u64) };
    if r == 0 {
        Ok(())
    } else {
        Err(r)
    }
}

/// Reads service lifecycle state.
pub fn service_list(out: &mut [u8]) -> Result<usize, i64> {
    let r = unsafe {
        syscall2(
            SyscallNumber::ServiceList,
            out.as_mut_ptr() as u64,
            out.len() as u64,
        )
    };
    if r >= 0 {
        Ok(r as usize)
    } else {
        Err(r)
    }
}
/// Queries stable service metadata. The response is a 12-byte ABI record.
pub fn service_query(service_id: u8, out: &mut [u8; 12]) -> Result<usize, i64> {
    let r = unsafe {
        syscall2(
            SyscallNumber::ServiceQuery,
            service_id as u64,
            out.as_mut_ptr() as u64,
        )
    };
    if r >= 0 {
        Ok(r as usize)
    } else {
        Err(r)
    }
}

/// Starts a declared system service. Only PID1/service-manager capability domains may invoke this.
pub fn service_start(service_id: u8) -> Result<u64, i64> {
    let r = unsafe { syscall1(SyscallNumber::ServiceStart, service_id as u64) };
    if r >= 0 {
        Ok(r as u64)
    } else {
        Err(r)
    }
}
/// Stops a declared system service. Only PID1/service-manager capability domains may invoke this.
pub fn service_stop(service_id: u8) -> Result<(), i64> {
    let r = unsafe { syscall1(SyscallNumber::ServiceStop, service_id as u64) };
    if r == 0 {
        Ok(())
    } else {
        Err(r)
    }
}
/// Requests recovery of services currently marked failed.
pub fn service_recover_failed(service_id: u8) -> Result<u64, i64> {
    let r = unsafe { syscall1(SyscallNumber::ServiceRecoverFailed, service_id as u64) };
    if r >= 0 {
        Ok(r as u64)
    } else {
        Err(r)
    }
}

/// Reads a persistent setting.
pub fn settings_get(key: &str, out: &mut [u8]) -> Result<usize, i64> {
    let r = unsafe {
        syscall5(
            SyscallNumber::SettingsGet,
            RESERVED_CAP_SLOT,
            key.as_ptr() as u64,
            key.len() as u64,
            out.len() as u64,
            out.as_mut_ptr() as u64,
        )
    };
    if r >= 0 {
        Ok(r as usize)
    } else {
        Err(r)
    }
}
/// Updates a persistent setting.
pub fn settings_set(key: &str, value: &[u8]) -> Result<(), i64> {
    let r = unsafe {
        syscall5(
            SyscallNumber::SettingsSet,
            RESERVED_CAP_SLOT,
            key.as_ptr() as u64,
            key.len() as u64,
            value.as_ptr() as u64,
            value.len() as u64,
        )
    };
    if r == 0 {
        Ok(())
    } else {
        Err(r)
    }
}

/// Returns live block-device capacity in bytes.
pub fn storage_query() -> Result<u64, i64> {
    let r = unsafe { syscall0(SyscallNumber::StorageQuery) };
    if r >= 0 {
        Ok(r as u64)
    } else {
        Err(r)
    }
}

/// Lists immediate VFS directory children using a read capability.
pub fn fs_list(path: &str, out: &mut [u8]) -> Result<usize, i64> {
    let r = unsafe {
        syscall4(
            SyscallNumber::FsList,
            RESERVED_CAP_SLOT,
            path.as_ptr() as u64,
            path.len() as u64,
            out.as_mut_ptr() as u64,
        )
    };
    if r >= 0 {
        Ok(r as usize)
    } else {
        Err(r)
    }
}

/// Launches an application or verified package in Ring 3.
pub fn app_launch(app_id: &str) -> Result<u64, i64> {
    let r = unsafe {
        syscall2(
            SyscallNumber::AppLaunch,
            app_id.as_ptr() as u64,
            app_id.len() as u64,
        )
    };
    if r >= 0 {
        Ok(r as u64)
    } else {
        Err(r)
    }
}

/// Queries the active foreground user ID.
pub fn user_get_current() -> u32 {
    unsafe { syscall0(SyscallNumber::UserGetCurrent) as u32 }
}

/// Requests switching the active foreground user.
pub fn user_switch(target_uid: u32) -> Result<(), i64> {
    let r = unsafe { syscall1(SyscallNumber::UserSwitch, target_uid as u64) };
    if r == 0 {
        Ok(())
    } else {
        Err(r)
    }
}

/// User lock/unlock status codes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UserLockState {
    Unlocked = 0,
    Locked = 1,
    Uninitialized = 2,
    Lockout = 3,
}

/// Queries user lock state. Returns Unlocked, Locked, Uninitialized, or Lockout.
pub fn user_get_lock_state(uid: u32) -> Result<UserLockState, i64> {
    let r = unsafe { syscall1(SyscallNumber::UserLockState, uid as u64) };
    match r {
        0 => Ok(UserLockState::Unlocked),
        1 => Ok(UserLockState::Locked),
        2 => Ok(UserLockState::Uninitialized),
        3 => Ok(UserLockState::Lockout),
        err => Err(err),
    }
}

/// Establishes or updates credentials for a user account.
/// If an existing credential is set, `old_secret` must verify before updating.
/// For first-boot credential setup (no prior secret), `old_secret` may be None.
pub fn user_change_credential(uid: u32, secret: &[u8], old_secret: Option<&[u8]>) -> Result<(), i64> {
    let (old_ptr, old_len) = match old_secret {
        Some(s) => (s.as_ptr() as u64, s.len() as u64),
        None => (0, 0),
    };
    let r = unsafe {
        syscall5(
            SyscallNumber::UserSetCredential,
            uid as u64,
            secret.as_ptr() as u64,
            secret.len() as u64,
            old_ptr,
            old_len,
        )
    };
    if r == 0 {
        Ok(())
    } else {
        Err(r)
    }
}

/// Unlocks a user account with candidate credentials.
pub fn user_unlock(uid: u32, secret: &[u8]) -> Result<(), i64> {
    let r = unsafe {
        syscall3(
            SyscallNumber::UserUnlock,
            uid as u64,
            secret.as_ptr() as u64,
            secret.len() as u64,
        )
    };
    if r == 0 {
        Ok(())
    } else {
        Err(r)
    }
}


/// Establishes the first credential for a locked, credential-less account.
/// The kernel rejects this operation once a credential already exists.
pub fn user_set_credential(uid: u32, secret: &[u8]) -> Result<(), i64> {
    // WHY five arguments: the kernel's UserSetCredential takes a1=uid,
    // a2=secret_ptr, a3=secret_len, a4=old_secret_ptr, a5=old_secret_len. This
    // call establishes a credential on an account that has none, so the old
    // secret is `None` and must be passed as explicit zeros. Passing three
    // arguments left a4/a5 holding whatever the register file carried, and the
    // kernel read a nonzero garbage length as a request to copy an old secret
    // from a wild pointer, which it refuses - so every first-boot PIN setup
    // failed its write while the shell reported success.
    let r = unsafe {
        syscall5(
            SyscallNumber::UserSetCredential,
            uid as u64,
            secret.as_ptr() as u64,
            secret.len() as u64,
            0,
            0,
        )
    };
    if r == 0 { Ok(()) } else { Err(r) }
}

/// Whether the account holds a credential.
///
/// WHY this is not `user_get_lock_state`: `Unlocked` is what a credential-less
/// account reports once it has been swiped past, so the lock state cannot say
/// whether a PIN exists. `Err` means the account layer refused the query, which
/// is not the same answer as "no".
pub fn user_has_credential(uid: u32) -> Result<bool, i64> {
    let r = unsafe { syscall1(SyscallNumber::UserHasCredential, uid as u64) };
    match r {
        0 => Ok(false),
        1 => Ok(true),
        err => Err(err),
    }
}

/// Removes the screen lock from an account, leaving it credential-less again.
///
/// `secret` is the credential currently set: the kernel verifies it before it
/// clears anything, because this is the one credential operation that lowers
/// what the device demands. An account with no credential rejects the call.
pub fn user_clear_credential(uid: u32, secret: &[u8]) -> Result<(), i64> {
    let r = unsafe {
        syscall3(
            SyscallNumber::UserClearCredential,
            uid as u64,
            secret.as_ptr() as u64,
            secret.len() as u64,
        )
    };
    if r == 0 { Ok(()) } else { Err(r) }
}
/// Locks a user account.
pub fn user_lock(uid: u32) -> Result<(), i64> {
    let r = unsafe { syscall1(SyscallNumber::UserLock, uid as u64) };
    if r == 0 {
        Ok(())
    } else {
        Err(r)
    }
}

/// Posts a notification from the calling application.
pub fn notification_post(pkg: &str, channel: &str) -> Result<u64, i64> {
    let r = unsafe {
        syscall4(
            SyscallNumber::NotificationPost,
            pkg.as_ptr() as u64,
            pkg.len() as u64,
            channel.as_ptr() as u64,
            channel.len() as u64,
        )
    };
    if r >= 0 {
        Ok(r as u64)
    } else {
        Err(r)
    }
}

/// Cancels an active notification.
pub fn notification_cancel(pkg: &str, notif_id: u64) -> Result<(), i64> {
    let r = unsafe {
        syscall3(
            SyscallNumber::NotificationCancel,
            pkg.as_ptr() as u64,
            pkg.len() as u64,
            notif_id,
        )
    };
    if r == 0 {
        Ok(())
    } else {
        Err(r)
    }
}

/// Queries count of active notifications for a user.
pub fn notification_query(user_id: u32, is_locked: bool) -> Result<usize, i64> {
    let r = unsafe {
        syscall2(
            SyscallNumber::NotificationQuery,
            user_id as u64,
            if is_locked { 1 } else { 0 },
        )
    };
    if r >= 0 {
        Ok(r as usize)
    } else {
        Err(r)
    }
}

/// Queries active notification records for a user into a serialized buffer.
pub fn notification_query_records(user_id: u32, is_locked: bool, buf: &mut [u8]) -> Result<usize, i64> {
    let r = unsafe {
        syscall4(
            SyscallNumber::NotificationQuery,
            user_id as u64,
            if is_locked { 1 } else { 0 },
            buf.as_mut_ptr() as u64,
            buf.len() as u64,
        )
    };
    if r >= 0 {
        Ok(r as usize)
    } else {
        Err(r)
    }
}

/// Revokes a granted sensitive permission from a task.
pub fn permission_revoke(task_id: u64, perm_id: u32) -> Result<(), i64> {
    let r = unsafe { syscall2(SyscallNumber::PermissionRevoke, task_id, perm_id as u64) };
    if r == 0 {
        Ok(())
    } else {
        Err(r)
    }
}

/// Queries hardware availability status for a setting/hardware category (0=available, 1=simulated, 2=unavailable).
pub fn settings_hardware_status(category: &str) -> Result<i64, i64> {
    let r = unsafe {
        syscall2(
            SyscallNumber::SettingsHardwareStatus,
            category.as_ptr() as u64,
            category.len() as u64,
        )
    };
    if r >= 0 {
        Ok(r)
    } else {
        Err(r)
    }
}

/// Physical RAM telemetry returned by `memory_telemetry()`.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MemoryTelemetry {
    pub total_bytes: u64,
    pub used_bytes: u64,
    pub free_bytes: u64,
}

/// Enumerable task record returned by `task_enumerate()`.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TaskRecord {
    pub task_id: u64,
    pub name: [u8; 16],
    pub name_len: u8,
    pub state: u8, // 0=Ready, 1=Running, 2=Blocked
    pub _reserved: [u8; 6],
    pub cpu_ticks: u64,
    pub mem_bytes: u64,
}

impl TaskRecord {
    pub fn name_str(&self) -> &str {
        let len = (self.name_len as usize).min(16);
        core::str::from_utf8(&self.name[..len]).unwrap_or("unknown")
    }
}

/// Queries physical RAM telemetry from the kernel.
pub fn memory_telemetry() -> Result<MemoryTelemetry, i64> {
    let mut telem = MemoryTelemetry {
        total_bytes: 0,
        used_bytes: 0,
        free_bytes: 0,
    };
    let r = unsafe {
        syscall1(
            SyscallNumber::MemoryTelemetry,
            core::ptr::addr_of_mut!(telem) as u64,
        )
    };
    if r == 0 {
        Ok(telem)
    } else {
        Err(r)
    }
}

/// Enumerates active running tasks from the kernel scheduler.
pub fn task_enumerate(out: &mut [TaskRecord]) -> Result<usize, i64> {
    if out.is_empty() {
        return Ok(0);
    }
    let r = unsafe {
        syscall2(
            SyscallNumber::TaskEnumerate,
            out.as_mut_ptr() as u64,
            out.len() as u64,
        )
    };
    if r >= 0 {
        Ok(r as usize)
    } else {
        Err(r)
    }
}

#[cfg(test)]
mod abi_tests {
    use super::*;

    /// Pins the `FsStat` record the kernel writes, byte for byte.
    ///
    /// WHY this test lives here and not only in the kernel: both sides describe
    /// the same 96 bytes independently, and a drift would not fail a compile --
    /// it would hand a caller a plausible-looking wrong stat. The kernel's own
    /// `fs_stat_record_layout_is_pinned` writes these offsets; this one decodes
    /// them.
    #[test]
    fn fs_stat_record_is_decoded_at_the_documented_offsets() {
        let mut buf = [0u8; FILE_STAT_BYTES];
        buf[0..8].copy_from_slice(&0x0102_0304_0506_0708u64.to_le_bytes());
        buf[8..12].copy_from_slice(&0x1122_3344u32.to_le_bytes());
        buf[12..16].copy_from_slice(&0x5566_7788u32.to_le_bytes());
        buf[16..18].copy_from_slice(&0x99AAu16.to_le_bytes());
        buf[18..20].copy_from_slice(&0xBBCCu16.to_le_bytes());
        buf[20..28].copy_from_slice(&0x1122_3344_5566_7788u64.to_le_bytes());
        buf[28..36].copy_from_slice(&0x8877_6655_4433_2211u64.to_le_bytes());
        buf[36..44].copy_from_slice(&0xAABB_CCDD_EEFF_0011u64.to_le_bytes());
        buf[44..48].copy_from_slice(&2u32.to_le_bytes());
        buf[48] = 1;
        buf[56..88].copy_from_slice(&[0xA5u8; 32]);
        buf[88..96].copy_from_slice(&0x1234_5678_9ABC_DEF0u64.to_le_bytes());

        let parsed = FileStatRecord::parse(&buf).expect("a full record decodes");
        assert_eq!(parsed.inode, 0x0102_0304_0506_0708);
        assert_eq!(parsed.uid, 0x1122_3344);
        assert_eq!(parsed.gid, 0x5566_7788);
        assert_eq!(parsed.mode, 0x99AA);
        assert_eq!(parsed.nlink, 0xBBCC);
        assert_eq!(parsed.atime, 0x1122_3344_5566_7788);
        assert_eq!(parsed.mtime, 0x8877_6655_4433_2211);
        assert_eq!(parsed.ctime, 0xAABB_CCDD_EEFF_0011);
        assert_eq!(parsed.file_type, 2, "ReadOnlySystem is declared second");
        assert!(parsed.read_only);
        assert_eq!(parsed.sha256, [0xA5u8; 32]);
        assert_eq!(parsed.size, 0x1234_5678_9ABC_DEF0);
    }

    /// A short buffer must be refused, not filled with zeroes: a truncated stat
    /// would otherwise report a wrong size that looks self-consistent.
    #[test]
    fn a_short_stat_record_is_refused() {
        assert!(FileStatRecord::parse(&[0u8; FILE_STAT_BYTES - 1]).is_none());
        assert!(FileStatRecord::parse(&[]).is_none());
    }

    #[test]
    fn cap_type_constants_match_kernel_encoding() {
        assert_eq!(cap_type::SERIAL_OUTPUT, 1);
        assert_eq!(cap_type::FRAMEBUFFER_DIRECT, 2);
        assert_eq!(cap_type::TIMER_TICK_READ, 3);
        assert_eq!(cap_type::HARDWARE_PORT_IO, 4);
        assert_eq!(cap_type::IPC_SEND, 5);
        assert_eq!(cap_type::IPC_RECEIVE, 6);
        assert_eq!(cap_type::PROCESS_SPAWN, 7);
        assert_eq!(cap_type::PROCESS_KILL, 8);
        assert_eq!(cap_type::FILE_SYSTEM_READ, 9);
        assert_eq!(cap_type::FILE_SYSTEM_WRITE, 10);
        assert_eq!(cap_type::STORAGE_RAW_BLOCK, 11);
        assert_eq!(cap_type::NETWORK_SOCKET, 12);
        assert_eq!(cap_type::NETWORK_SEND, 13);
        assert_eq!(cap_type::NETWORK_RECEIVE, 14);
        assert_eq!(cap_type::SHM_PRODUCER, 15);
        assert_eq!(cap_type::SHM_CONSUMER, 16);
        assert_eq!(cap_type::EVENT_POLL, 17);
        assert_eq!(cap_type::SYSTEM_CONTROL, 18);
        assert_eq!(cap_type::REALTIME_CLOCK_READ, 24);
    }
}