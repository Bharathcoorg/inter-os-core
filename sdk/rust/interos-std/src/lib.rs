pub mod service;
//! Inter OS Userspace Standard Library (interos-std)
//!
//! Provides safe, ergonomic Rust bindings for Inter OS hardware fast system calls
//! (`syscall` instruction) executing in Ring 3 User Mode.

#![no_std]

pub mod design;
pub mod compositor {
    pub const MAGIC: [u8; 4] = *b"ICMP";
    pub const VERSION: u8 = 1;
    pub const MSG_TYPE: u32 = 0x434F_4D50;
    pub const HEADER_SIZE: usize = 24;

    #[repr(u8)]
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub enum Op { CreateSurface = 1, WritePixels = 2, SetGeometry = 3, DestroySurface = 4, Commit = 5, Focus = 6 }

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub struct Request {
        pub op: Op, pub surface_id: u32, pub width: u32, pub height: u32, pub pixel_offset: u32,
    }

    impl Request {
        pub fn encode(&self, out: &mut [u8]) -> Result<usize, ()> {
            if out.len() < HEADER_SIZE { return Err(()); }
            out[..HEADER_SIZE].fill(0);
            out[..4].copy_from_slice(&MAGIC);
            out[4] = VERSION;
            out[5] = self.op as u8;
            out[8..12].copy_from_slice(&self.surface_id.to_le_bytes());
            out[12..16].copy_from_slice(&self.width.to_le_bytes());
            out[16..20].copy_from_slice(&self.height.to_le_bytes());
            out[20..24].copy_from_slice(&self.pixel_offset.to_le_bytes());
            Ok(HEADER_SIZE)
        }
    }
}

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
    #[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64", target_arch = "riscv64")))]
    { let _ = num; ret = 0; }
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
    #[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64", target_arch = "riscv64")))]
    { let _ = (num, a1); ret = 0; }
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
    #[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64", target_arch = "riscv64")))]
    { let _ = (num, a1, a2); ret = 0; }
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
    #[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64", target_arch = "riscv64")))]
    { let _ = (num, a1, a2, a3); ret = 0; }
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
    #[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64", target_arch = "riscv64")))]
    { let _ = (num, a1, a2, a3, a4); ret = 0; }
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
    #[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64", target_arch = "riscv64")))]
    { let _ = (num, a1, a2, a3, a4, a5); ret = 0; }
    ret
}

#[inline(always)]
pub unsafe fn syscall6(num: SyscallNumber, a1: u64, a2: u64, a3: u64, a4: u64, a5: u64, a6: u64) -> i64 {
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
    #[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64", target_arch = "riscv64")))]
    { let _ = (num, a1, a2, a3, a4, a5, a6); ret = 0; }
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
pub fn log(cap_slot: usize, msg: &str) -> Result<(), i64> {
    let ret = unsafe {
        syscall3(
            SyscallNumber::Log,
            cap_slot as u64,
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
pub fn service_package_launch(cap_slot: usize, path: &str) -> Result<u64, i64> {
    let ret = unsafe { syscall3(SyscallNumber::ServicePackageLaunch, cap_slot as u64, path.as_ptr() as u64, path.len() as u64) };
    if ret >= 0 { Ok(ret as u64) } else { Err(ret) }
}

pub fn get_ticks(cap_slot: usize) -> Result<u64, i64> {
    let ret = unsafe { syscall1(SyscallNumber::GetTicks, cap_slot as u64) };
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
pub fn ipc_send(cap_slot: usize, target_task_id: u64, msg_type: u32, payload: &[u8]) -> Result<(), i64> {
    let ret = unsafe {
        syscall5(
            SyscallNumber::IpcSend,
            cap_slot as u64,
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
pub fn ipc_recv(cap_slot: usize, out_buf: &mut [u8]) -> Result<usize, i64> {
    let ret = unsafe {
        syscall3(
            SyscallNumber::IpcRecv,
            cap_slot as u64,
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
        let _ = crate::log(0, msg);
    }
    pub fn warn(msg: &str) {
        let _ = crate::log(0, msg);
    }
    pub fn error(msg: &str) {
        let _ = crate::log(0, msg);
    }
}

/// Ergonomic IPC helper module matching application templates.
pub mod ipc {
    pub fn send(target_task_id: u64, msg_type: u32, payload: &[u8]) -> Result<(), i64> {
        crate::ipc_send(0, target_task_id, msg_type, payload)
    }

    pub fn receive_any(out_buf: &mut [u8]) -> Result<(u64, u32, usize), i64> {
        let n = crate::ipc_recv(0, out_buf)?;
        Ok((0, 0, n))
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
pub fn package_install(cap_slot: usize, pkg_id: &str, user_consent: bool) -> Result<(), i64> {
    let ret = unsafe {
        syscall4(
            SyscallNumber::PackageInstall,
            cap_slot as u64,
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
pub fn package_rollback(cap_slot:usize,pkg_id:&str,user_consent:bool)->Result<(),i64>{
    let ret=unsafe{syscall4(SyscallNumber::PackageRollback,cap_slot as u64,pkg_id.as_ptr() as u64,pkg_id.len() as u64,user_consent as u64)};
    if ret>=0{Ok(())}else{Err(ret)}
}

/// Reads the live compositor window registry.
pub fn window_list(out_buf:&mut [u8])->Result<usize,i64>{
    let ret=unsafe{syscall2(SyscallNumber::WindowList,out_buf.as_mut_ptr() as u64,out_buf.len() as u64)};
    if ret>=0{Ok(ret as usize)}else{Err(ret)}
}

/// Reads the kernel package lifecycle registry. Requires FileSystemRead capability in cap_slot.
pub fn package_registry(cap_slot: usize, out_buf: &mut [u8]) -> Result<usize, i64> {
    let ret=unsafe{syscall3(SyscallNumber::PackageRegistry,cap_slot as u64,out_buf.as_mut_ptr() as u64,out_buf.len() as u64)};
    if ret>=0{Ok(ret as usize)}else{Err(ret)}
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
pub fn sensor_read(out_buf: &mut [u8]) -> Result<usize, i64> {
    let ret = unsafe {
        syscall2(
            SyscallNumber::SensorRead,
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

/// Queries mobile battery fuel gauge telemetry (voltage, SoC %, capacity, charging status).
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
pub fn android_bridge_intent(action: &str, data_uri: &str, out_buf: &mut [u8]) -> Result<usize, i64> {
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
pub fn vault_encrypt_sector(block_index: u64, buf: &mut [u8; 512], iv: &[u8; 16]) -> Result<(), i64> {
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
pub fn vault_decrypt_sector(block_index: u64, buf: &mut [u8; 512], iv: &[u8; 16]) -> Result<(), i64> {
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
pub fn surface_create(display_cap_slot: usize, width: u32, height: u32, z_order: u32) -> Result<u32, i64> {
    let ret = unsafe { syscall5(SyscallNumber::SurfaceCreate, display_cap_slot as u64, width as u64, height as u64, z_order as u64, 0) };
    if ret >= 0 { Ok(ret as u32) } else { Err(ret) }
}

/// Writes ARGB8888 pixels through the display service boundary.
pub fn surface_write(display_cap_slot: usize, surface_id: u32, offset: usize, pixels: &[u32]) -> Result<usize, i64> {
    let ret = unsafe {
        syscall5(
            SyscallNumber::SurfaceWrite,
            display_cap_slot as u64,
            surface_id as u64,
            offset as u64,
            pixels.as_ptr() as u64,
            pixels.len() as u64,
        )
    };
    if ret >= 0 { Ok(ret as usize) } else { Err(ret) }
}

/// Destroys a surface owned by the display service.
pub fn surface_destroy(display_cap_slot: usize, surface_id: u32) -> Result<(), i64> {
    let ret = unsafe { syscall2(SyscallNumber::SurfaceDestroy, display_cap_slot as u64, surface_id as u64) };
    if ret == 0 { Ok(()) } else { Err(ret) }
}

/// Composites and atomically flips the display scanout.
pub fn surface_flip(display_cap_slot: usize) -> Result<u64, i64> {
    let ret = unsafe { syscall1(SyscallNumber::SurfaceFlip, display_cap_slot as u64) };
    if ret >= 0 { Ok(ret as u64) } else { Err(ret) }
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

/// Polls the normalized touch/input event pipeline.
pub fn input_event_poll(out: &mut InputEvent) -> Result<(), i64> {
    let mut buf = [0u8; 24];
    let ret = unsafe { syscall2(SyscallNumber::InputEventPoll, buf.as_mut_ptr() as u64, buf.len() as u64) };
    if ret < 0 { return Err(ret); }
    if ret != 24 { return Err(-3); }
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
pub fn fs_open(cap_slot: usize, path: &str, rights: u32) -> Result<u32, i64> {
    let ret = unsafe {
        syscall4(
            SyscallNumber::FsOpen,
            cap_slot as u64,
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

/// Reads data from an open file descriptor.
pub fn fs_read(cap_slot: usize, fd: u32, buf: &mut [u8]) -> Result<usize, i64> {
    let ret = unsafe {
        syscall4(
            SyscallNumber::FsRead,
            cap_slot as u64,
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
pub fn fs_write(cap_slot: usize, fd: u32, buf: &[u8]) -> Result<usize, i64> {
    let ret = unsafe {
        syscall4(
            SyscallNumber::FsWrite,
            cap_slot as u64,
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
    let ret = unsafe {
        syscall1(
            SyscallNumber::FsClose,
            fd as u64,
        )
    };
    if ret == 0 {
        Ok(())
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
pub const EVENT_IPC_MSG: u32        = 1 << 0; // 0x01
/// Zero-Copy DMA Shared Memory pipe has data available to consume.
pub const EVENT_SHM_READABLE: u32   = 1 << 1; // 0x02
/// Zero-Copy DMA Shared Memory pipe has free slots available to stream.
pub const EVENT_SHM_WRITABLE: u32   = 1 << 2; // 0x04
/// Hardware system timer interval tick or deadline elapsed.
pub const EVENT_TIMER_TICK: u32     = 1 << 3; // 0x08
/// Capacitive touch digitizer input event frame available.
pub const EVENT_TOUCH_INPUT: u32    = 1 << 4; // 0x10
/// Inbound network socket datagram or packet received.
pub const EVENT_NETWORK_PACKET: u32 = 1 << 5; // 0x20
/// Cellular baseband modem unsolicited result code (URC) arrived.
pub const EVENT_MODEM_URC: u32      = 1 << 6; // 0x40

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
    let ret = unsafe {
        syscall1(
            SyscallNumber::TelephonySetData,
            if enabled { 1 } else { 0 },
        )
    };
    if ret >= 0 {
        Ok(ret != 0)
    } else {
        Err(ret)
    }
}

/// Allocates a new hardware surface in the DRM/KMS compositor.
pub fn surface_create(width: u32, height: u32, z_order: u32) -> Result<u32, i64> {
    let ret = unsafe {
        syscall3(
            SyscallNumber::SurfaceCreate,
            width as u64,
            height as u64,
            z_order as u64,
        )
    };
    if ret >= 0 {
        Ok(ret as u32)
    } else {
        Err(ret)
    }
}

/// Swaps the DRM/KMS back buffer to the front scanout buffer (atomic page flip).
pub fn surface_flip() -> Result<u64, i64> {
    let ret = unsafe { syscall0(SyscallNumber::SurfaceFlip) };
    if ret >= 0 {
        Ok(ret as u64)
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











/// Writes ARGB pixels into a userspace-owned DRM/KMS surface.
pub fn surface_write(surface_id: u32, pixel_offset: u32, pixels: &[u32]) -> Result<usize, i64> {
    let ret = unsafe { syscall4(SyscallNumber::SurfaceWrite, surface_id as u64, pixel_offset as u64, pixels.as_ptr() as u64, pixels.len() as u64) };
    if ret >= 0 { Ok(ret as usize) } else { Err(ret) }
}

/// Destroys a userspace-owned DRM/KMS surface.
pub fn surface_destroy(surface_id: u32) -> Result<(), i64> {
    let ret = unsafe { syscall1(SyscallNumber::SurfaceDestroy, surface_id as u64) };
    if ret == 0 { Ok(()) } else { Err(ret) }
}

pub fn package_list(cap_slot: usize, out_buf: &mut [u8]) -> Result<usize, i64> {
    let ret=unsafe{syscall3(SyscallNumber::PackageList,cap_slot as u64,out_buf.as_mut_ptr() as u64,out_buf.len() as u64)};
    if ret>=0{Ok(ret as usize)}else{Err(ret)}
}


/// Moves an owned window in the compositor.
pub fn window_set_geometry(display_cap_slot: usize, id:u32,x:i32,y:i32,z:u32,visible:bool)->Result<(),i64>{
 let packed=(z as u64)|if visible{1u64<<63}else{0};
 let r=unsafe{syscall5(SyscallNumber::WindowSetGeometry,display_cap_slot as u64,id as u64,x as i64 as u64,y as i64 as u64,packed)};if r==0{Ok(())}else{Err(r)}
}
/// Focuses the topmost live surface through the display service.
pub fn window_focus(display_cap_slot: usize,x:u32,y:u32)->Result<(),i64>{let r=unsafe{syscall3(SyscallNumber::WindowFocus,display_cap_slot as u64,x as u64,y as u64)};if r==0{Ok(())}else{Err(r)}}

/// Verifies a signed Ring-3 system-service RAPP and returns its service ID.
/// Requires the PID1 service-manager authority and a FileSystemRead capability.
pub fn service_package_verify(cap_slot: usize, path: &str) -> Result<u8, i64> {
    let r = unsafe {
        syscall3(
            SyscallNumber::ServicePackageVerify,
            cap_slot as u64,
            path.as_ptr() as u64,
            path.len() as u64,
        )
    };
    if r >= 0 { Ok(r as u8) } else { Err(r) }
}

/// Publishes a heartbeat for the current registered userspace service.\npub fn service_heartbeat(service_id:u8)->Result<(),i64>{let r=unsafe{syscall1(SyscallNumber::ServiceHeartbeat,service_id as u64)};if r==0{Ok(())}else{Err(r)}}\n/// Reads service lifecycle state.
pub fn service_list(out:&mut[u8])->Result<usize,i64>{let r=unsafe{syscall2(SyscallNumber::ServiceList,out.as_mut_ptr() as u64,out.len() as u64)};if r>=0{Ok(r as usize)}else{Err(r)}}
/// Starts a declared system service. Only PID1/service-manager capability domains may invoke this.
pub fn service_start(service_id:u8)->Result<u64,i64>{let r=unsafe{syscall1(SyscallNumber::ServiceStart,service_id as u64)};if r>=0{Ok(r as u64)}else{Err(r)}}
/// Stops a declared system service. Only PID1/service-manager capability domains may invoke this.
pub fn service_stop(service_id:u8)->Result<(),i64>{let r=unsafe{syscall1(SyscallNumber::ServiceStop,service_id as u64)};if r==0{Ok(())}else{Err(r)}}
/// Requests recovery of services currently marked failed.
pub fn service_recover_failed()->Result<u64,i64>{let r=unsafe{syscall0(SyscallNumber::ServiceRecoverFailed)};if r>=0{Ok(r as u64)}else{Err(r)}}

/// Reads a persistent setting.
pub fn settings_get(cap_slot:usize,key:&str,out:&mut[u8])->Result<usize,i64>{let r=unsafe{syscall5(SyscallNumber::SettingsGet,cap_slot as u64,key.as_ptr() as u64,key.len() as u64,out.len() as u64,out.as_mut_ptr() as u64)};if r>=0{Ok(r as usize)}else{Err(r)}}
/// Updates a persistent setting.
pub fn settings_set(cap_slot:usize,key:&str,value:&[u8])->Result<(),i64>{let r=unsafe{syscall5(SyscallNumber::SettingsSet,cap_slot as u64,key.as_ptr() as u64,key.len() as u64,value.as_ptr() as u64,value.len() as u64)};if r==0{Ok(())}else{Err(r)}}


/// Returns live block-device capacity in bytes.
pub fn storage_query()->Result<u64,i64>{let r=unsafe{syscall0(SyscallNumber::StorageQuery)};if r>=0{Ok(r as u64)}else{Err(r)}}

/// Lists immediate VFS directory children using a read capability.
pub fn fs_list(cap_slot:usize,path:&str,out:&mut[u8])->Result<usize,i64>{let r=unsafe{syscall4(SyscallNumber::FsList,cap_slot as u64,path.as_ptr() as u64,path.len() as u64,out.as_mut_ptr() as u64)};if r>=0{Ok(r as usize)}else{Err(r)}}
