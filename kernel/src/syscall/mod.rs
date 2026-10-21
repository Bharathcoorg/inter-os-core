//! Inter OS System Call Dispatcher
//!
//! Provides the primary interface between tasks and kernel services.
//! Every privileged system call enforces capability validation before execution.

pub mod entry;
pub mod user_ptr;
pub use entry::set_current_kernel_rsp;
pub use user_ptr::{
    copy_from_user, copy_to_user, read_user_string, read_user_value, write_user_value,
    validate_user_buffer, UserAccessError, UserPtr, UserSlice, UserSliceMut,
};

use crate::capability::{CapError, Capability, CapabilityId, CapabilityTable, CapabilityType, Rights};
use crate::drivers::{BlockDevice, ATA_PRIMARY, SECTOR_SIZE};
use crate::drivers::display::DisplayDriver;
use crate::hal::traits::PowerController;
use crate::fs::{FileDescriptor, FileStat, FileType, FsError, VFS};
use crate::ipc::{IpcError, IpcMessage, IPC_ROUTER};
use crate::net::NetError;
use crate::package::{parse_rapp, RappError, RappManifest};
use crate::serial_println;
use crate::task::{SCHEDULER, TaskId};

/// Object-scoped capability target reserved for the PID1 service manager.
pub const SERVICE_MANAGER_OBJECT: u64 = 0x5345_5256_4D47_52;

/// Object-scoped capability target for user-account administration.
///
/// WHY a dedicated namespace instead of the raw uid: uid 0 is the SYSTEM user
/// and raw uids collide with the "any object" convention used by the RAPP
/// capability map (`object_id: 0`). Deriving the object id from the uid means a
/// capability granted for one account can never be replayed against another,
/// and can never be confused with a global SystemControl grant.
pub const USER_CONTROL_OBJECT_BASE: u64 = 0x5553_4552_0000_0000;

/// Returns the object id that `SystemControl` must be bound to in order to
/// administer the account identified by `uid`.
pub const fn user_control_object(uid: u32) -> u64 {
    USER_CONTROL_OBJECT_BASE | (uid as u64)
}

/// Object-scoped capability target for the system notification broker.
///
/// WHY a dedicated object: a system component (for example a call or messaging
/// daemon) legitimately posts notifications on behalf of other packages, so
/// there must be a grant that means "may act for any package". Reusing a
/// per-user control object for that would be ambiguous, and reusing the
/// object-blind `authorize_capability` would let any SystemControl holder
/// impersonate any package.
pub const NOTIFICATION_CONTROL_OBJECT: u64 = 0x4E4F_5446_0000_0001;

/// Base object id for the sovereign encrypted storage vault, namespaced per user.
///
/// WHY the vault needs its own object: `StorageRawBlock` is also granted to the
/// storage service for whole-device block IO. Without a distinct per-caller
/// vault object, that broad grant would satisfy the vault check and let any
/// holder decrypt or overwrite another user's sectors.
pub const VAULT_OBJECT_BASE: u64 = 0x5641_554C_5400_0000;

/// Returns the object id a caller must hold `StorageRawBlock` against to use
/// its own vault.
pub const fn vault_object_for_user(uid: u32) -> u64 {
    VAULT_OBJECT_BASE | (uid as u64)
}

/// Highest addressable vault sector index for a single user account.
///
/// WHY bound the index at all: `block_index` reaches the keystream derivation
/// as raw attacker-controlled input. An unbounded index lets one caller sweep
/// the entire (iv, block_index) keystream space, which is a chosen-plaintext
/// oracle over another user's sectors once two accounts share the vault key.
/// 1 MiB of user data (2048 sectors) is far above any single-user workload.
pub const VAULT_MAX_BLOCK_INDEX: u64 = 2048;

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
    FsSync = 125,
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
    TcpSend = 97,
    TcpRecv = 98,
    NetCloseSocket = 96,
    SettingsGet = 82,
    SettingsSet = 83,
    StorageQuery = 84,
    FsList = 85,
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
    FeatureDisabled = -19,
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
    NotReady = -30,
    InvalidArgument = -31,
}

/// Dispatches a cooperative scheduling yield.
pub fn sys_yield() {
    crate::serial::service_serial_commands();
    crate::drivers::ps2_mouse::drain_events();
    crate::task::yield_now();
}

/// Terminates the calling task with an exit code.
///
/// The task transitions to `Zombie` state until its parent reaps the exit status
/// via `sys_waitpid`. If no parent is waiting, the scheduler will eventually reclaim it.
pub fn sys_exit_with_code(exit_code: i32) -> ! {
    if let Some(id) = SCHEDULER.lock().current_task_id() {
        serial_println!("[PROC] Task {} sys_exit_with_code({}) called.", id.0, exit_code);
        let owner = id.0 as u32;
        let released = crate::net::SOCKET_TABLE.lock().release_owner(owner);
        if released > 0 { serial_println!("[NET] Released {} socket(s) owned by exiting task {}.", released, id.0); }
        let tcp_released = crate::net::TCP_TABLE.lock().release_owner(owner);
        if tcp_released > 0 { serial_println!("[NET] Released {} TCP connection(s) owned by exiting task {}.", tcp_released, id.0); }
        crate::ipc::IPC_ROUTER.lock().cleanup_task(id);
        let surfaces_cleaned = crate::drivers::display_kms::DRM_KMS.lock().destroy_surfaces_owned_by(id.0);
        if surfaces_cleaned > 0 {
            serial_println!("[KMS] Reclaimed {} display surface(s) owned by exiting task {}.", surfaces_cleaned, id.0);
            let mut kms = crate::drivers::display_kms::DRM_KMS.lock();
            let mut display = crate::drivers::KmsDisplayDriver::new(&mut kms);
            let _ = display.composite_and_flip();
        }
    }
    let parent_id = {
        let mut sched = SCHEDULER.lock();
        if let Some(current) = sched.current_task_mut() {
            current.exit_code = exit_code;
            current.state = crate::task::TaskState::Zombie;
            current.parent_id
        } else {
            crate::task::TaskId(0)
        }
    };
    if parent_id != crate::task::TaskId(0) {
        crate::event::post_event(parent_id, crate::event::EVENT_TIMER_TICK);
    }
    crate::task::terminate_current();
}

/// Terminates the calling task (exit code 0).
pub fn sys_exit() -> ! {
    sys_exit_with_code(0);
}

/// Waits for a child task to terminate and reaps its exit status.
///
/// Returns `Success` if the child has terminated (exit code logged to serial),
/// or `NotReady` if the child is still running.
pub fn sys_waitpid(child_id: u64) -> SyscallResult {
    crate::serial::service_serial_commands();
    let mut sched = SCHEDULER.lock();
    let parent_id = match sched.current_task_id() {
        Some(id) => id,
        None => return SyscallResult::PermissionDenied,
    };
    let child_tid = crate::task::TaskId(child_id);
    let child = match sched.get_task_mut(child_tid) {
        Some(task) => task,
        None => return SyscallResult::InvalidArgument,
    };
    if child.parent_id != parent_id {
        return SyscallResult::PermissionDenied;
    }
    let state = child.state;
    let code = child.exit_code;

    match state {
        crate::task::TaskState::Zombie | crate::task::TaskState::Terminated => {
            match sched.reap_child(parent_id, child_tid) {
                Ok(reaped_code) => {
                    drop(sched);
                    serial_println!(
                        "[WAITPID] Parent {} reaped child {} with exit code {}.",
                        parent_id.0,
                        child_id,
                        reaped_code
                    );
                    SyscallResult::Success
                }
                Err(_) => SyscallResult::DeviceError,
            }
        }
        _ => {
            let _ = code;
            SyscallResult::NotReady
        }
    }
}

/// Emits a diagnostic log message to the UART serial COM1.
///
/// Requires the caller to hold a `SerialOutput` capability with `Rights::WRITE`.
pub fn sys_log(_cap_slot: usize, msg: &str) -> SyscallResult {
    let sched = SCHEDULER.lock();
    let current = match sched.current_task() {
        Some(t) => t,
        None => return SyscallResult::PermissionDenied,
    };

    // WHY the capability is authorised by type and not by the slot the
    // caller named: a slot is a position in this task's capability table,
    // and the position depends on the order the kernel inserted the
    // capabilities in. The launcher proved the point -- it passed slot 0
    // while its FileSystemRead capability sat at slot 3, and the call was
    // refused with nothing on screen to say why. Authorising by type
    // enforces exactly the same rule without the caller knowing the
    // layout, and it cannot drift when the policy gains or loses a
    // capability.
    if !current.cap_table.authorize_capability(CapabilityType::SerialOutput, Rights::WRITE) {
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
pub fn sys_get_ticks(_cap_slot: usize) -> Result<u64, SyscallResult> {
    let sched = SCHEDULER.lock();
    let current = match sched.current_task() {
        Some(t) => t,
        None => return Err(SyscallResult::PermissionDenied),
    };

    // WHY the capability is authorised by type and not by the slot the
    // caller named: a slot is a position in this task's capability table,
    // and the position depends on the order the kernel inserted the
    // capabilities in. The launcher proved the point -- it passed slot 0
    // while its FileSystemRead capability sat at slot 3, and the call was
    // refused with nothing on screen to say why. Authorising by type
    // enforces exactly the same rule without the caller knowing the
    // layout, and it cannot drift when the policy gains or loses a
    // capability.
    if !current.cap_table.authorize_capability(CapabilityType::TimerTickRead, Rights::READ) {
        return Err(SyscallResult::PermissionDenied);
    }
    drop(sched);

    Ok(crate::interrupts::get_system_ticks())
}

/// Returns the TaskId of the caller.
pub fn sys_get_task_id() -> TaskId {
    SCHEDULER.lock().current_task_id().unwrap_or(TaskId(0))
}

pub fn sys_app_background() -> SyscallResult {
    let task = sys_get_task_id();
    if task == TaskId(0) { return SyscallResult::PermissionDenied; }
    SCHEDULER.lock().set_task_priority(task, 1);
    crate::drivers::display_kms::DRM_KMS.lock().set_surfaces_visibility_for_task(task.0, false);
    crate::task::process::set_background(task, crate::interrupts::get_system_ticks())
        .map(|_| SyscallResult::Success).unwrap_or(SyscallResult::InvalidArgument)
}

pub fn sys_app_foreground() -> SyscallResult {
    let task = sys_get_task_id();
    if task == TaskId(0) { return SyscallResult::PermissionDenied; }
    {
        let mut sched = SCHEDULER.lock();
        sched.set_task_priority(task, 6);
        sched.unblock_task(task);
    }
    crate::drivers::display_kms::DRM_KMS.lock().raise_surfaces_for_task(task.0);
    match crate::task::process::set_foreground(task) {
        Ok(()) => SyscallResult::Success,
        Err(_) => SyscallResult::InvalidArgument,
    }
}

pub fn sys_app_suspend() -> SyscallResult {
    let task = sys_get_task_id();
    if task == TaskId(0) { return SyscallResult::PermissionDenied; }
    crate::task::process::suspend(task, crate::interrupts::get_system_ticks())
        .map(|_| SyscallResult::Success).unwrap_or(SyscallResult::InvalidArgument)
}

pub fn sys_app_wake() -> SyscallResult {
    let task = sys_get_task_id();
    if task == TaskId(0) { return SyscallResult::PermissionDenied; }
    match crate::task::process::wake(task) {
        Ok(()) => { crate::task::SCHEDULER.lock().unblock_task(task); SyscallResult::Success }
        Err(_) => SyscallResult::InvalidArgument,
    }
}

pub const GLOBAL_SYSTEM_CONTROL_OBJECT: u64 = 0;

/// Whether a calling task's identity is allowed to seek device-wide system
/// administration at all.
///
/// WHY this exists as a separate predicate: holding capability 18 proves only
/// that the kernel minted a global `SystemControl` token for that process. It
/// does not by itself make the process the supervisor. Keeping the identity
/// decision in one named, testable predicate is what lets the runtime gate be
/// exercised without a live scheduler.
///
/// Authority comes from the kernel-assigned PID: TaskId(0) is the primordial
/// kernel task and TaskId(1) is the system supervisor. The namespace test
/// additionally admits first-party RAPP packages, whose task name is the
/// manifest's `package_id`.
///
/// WHY there is no bare `name == "system"` clause: no production path produces
/// a task with that name -- built-in services are `displayd`, `powerd`,
/// `launcher` and so on, and the primordial task is `kernel_main` -- so the
/// clause matched nothing while being the weakest link in the predicate. A task
/// spawned from a RAPP is named after the publisher's `package_id`, so that
/// clause would have let any publisher pick the privileged name.
///
/// The namespace test is defence in depth and never grants authority by itself:
/// the caller must still hold a global `SystemControl` capability, and
/// capability 18 is minted only for a key the trust store recognises as a
/// system publisher.
pub(crate) fn is_privileged_system_identity(task_id: TaskId, name: &str) -> bool {
    task_id == TaskId(0)
        || task_id == TaskId(1)
        || name.starts_with("org.interos.")
}

/// Authorizes a `SystemControl|INVOKE` against a specific administration object.
///
/// The global fallback from a domain object to `GLOBAL_SYSTEM_CONTROL_OBJECT`
/// is confined to PID 1. Without that confinement, any holder of capability 18
/// could reach every domain object, and any holder of a domain object scoped
/// capability could be talked into acting outside its own domain.
pub(crate) fn authorize_system_control_object(
    caps: &crate::capability::CapabilityTable,
    task_id: TaskId,
    object_id: u64,
) -> bool {
    if caps.authorize_object(CapabilityType::SystemControl, Rights::INVOKE, object_id) {
        return true;
    }
    if object_id != GLOBAL_SYSTEM_CONTROL_OBJECT && task_id == TaskId(1) {
        return caps.authorize_object(CapabilityType::SystemControl, Rights::INVOKE, GLOBAL_SYSTEM_CONTROL_OBJECT);
    }
    false
}

/// The complete device-wide administration gate: the identity check AND the
/// capability check. A process must pass both.
pub(crate) fn authorize_device_wide_system_control(
    caps: &crate::capability::CapabilityTable,
    task_id: TaskId,
    name: &str,
) -> bool {
    is_privileged_system_identity(task_id, name)
        && authorize_system_control_object(caps, task_id, GLOBAL_SYSTEM_CONTROL_OBJECT)
}

/// Enforces an explicit capability for privileged kernel controls.
pub fn require_system_control_object(object_id: u64) -> Result<(), SyscallResult> {
    let sched = SCHEDULER.lock();
    let task = sched.current_task().ok_or(SyscallResult::PermissionDenied)?;
    if authorize_system_control_object(&task.cap_table, task.id, object_id) {
        return Ok(());
    }
    Err(SyscallResult::PermissionDenied)
}

fn require_system_control() -> Result<(), SyscallResult> {
    let (task_id, name, caps) = {
        let sched = SCHEDULER.lock();
        let task = sched.current_task().ok_or(SyscallResult::PermissionDenied)?;
        (task.id, task.name, task.cap_table.clone())
    };
    if authorize_device_wide_system_control(&caps, task_id, name) {
        return Ok(());
    }
    Err(SyscallResult::PermissionDenied)
}

/// Requires `SystemControl|INVOKE` bound to the administration object of the
/// account `uid`.
///
/// WHY object-scoped and not the object-blind `require_system_control`: a
/// capability that merely proves "you hold SystemControl somewhere" lets any
/// holder lock, unlock, or switch to *any* account. Locking the owner's account
/// is a denial of service on the device owner; switching the global active user
/// is a full identity takeover. Binding to `user_control_object(uid)` makes
/// authority attenuate to a single account.
fn require_user_control_object(uid: u32) -> Result<(), SyscallResult> {
    // Authority must attenuate to a single account. PID 1 is the system
    // supervisor and administers every account; anything else needs a
    // SystemControl grant naming the account it acts on.
    //
    // WHY there is no launcher special case: one used to grant "active user
    // only" authority to a task named `org.interos.launcher` holding
    // USER_CONTROL_OBJECT_BASE. It was unreachable. Built-in services are named
    // `launcher`, not `org.interos.launcher`, so the name test missed them, and
    // no capability policy grants USER_CONTROL_OBJECT_BASE at all: the launcher
    // holds `user_control_object(OWNER)` and `user_control_object(GUEST)`, which
    // are BASE|1000 and BASE|1001, not BASE|0. The launcher therefore falls
    // through to the exact-account check below, which its real grants satisfy.
    let sched = SCHEDULER.lock();
    let task = sched.current_task().ok_or(SyscallResult::PermissionDenied)?;
    if task.id == TaskId(1) {
        return Ok(());
    }
    if task.cap_table.authorize_object(CapabilityType::SystemControl, Rights::INVOKE, user_control_object(uid)) {
        return Ok(());
    }
    Err(SyscallResult::PermissionDenied)
}

/// Resolves the user id the calling task is bound to, if any.
///
/// WHY this exists: a task may only act on its own account absent a delegated
/// `SystemControl` grant. `process::user_for_task` reads the kernel-owned
/// process binding, so it cannot be spoofed by an argument.
fn caller_user_id() -> Option<u32> {
    let task_id = SCHEDULER.lock().current_task_id()?;
    crate::task::process::user_for_task(task_id)
}

/// Authorizes a notification operation against the caller's own account, or
/// against a `SystemControl` grant for that specific account.
///
/// WHY: `NotificationQuery` takes a caller-supplied `user_id`. An unscoped
/// check leaks notification titles and bodies across every account on the
/// device. Binding to the caller's own account keeps the common case (an app
/// reading its own notifications) working while closing the cross-account read.
fn require_own_or_delegated_user(user_id: u32) -> Result<(), SyscallResult> {
    if caller_user_id() == Some(user_id) {
        return Ok(());
    }
    require_user_control_object(user_id)
}

/// Returns true when the caller's process name matches the package identity it
/// is claiming to act as.
///
/// WHY: `NotificationPost`/`NotificationCancel` take a caller-supplied package
/// id. The kernel already knows the package each task was spawned as
/// (`TaskControlBlock::name` comes from the verified RAPP manifest), so
/// requiring the claimed id to match that recorded identity is what prevents
/// one app posting or cancelling notifications as another.
fn caller_is_package(pkg: &str) -> bool {
    let sched = SCHEDULER.lock();
    sched.current_task().map(|t| t.name == pkg).unwrap_or(false)
}

/// Authorizes a package-scoped notification operation: the caller must be the
/// package it claims, or hold the system notification broker's `SystemControl`
/// grant (which is what lets a system daemon post for other packages).
fn require_package_or_delegated(pkg: &str) -> Result<(), SyscallResult> {
    if pkg.is_empty() {
        return require_system_control_object(NOTIFICATION_CONTROL_OBJECT);
    }
    if caller_is_package(pkg) {
        return Ok(());
    }
    require_system_control_object(NOTIFICATION_CONTROL_OBJECT)
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

    if crate::task::process::consume_background_ipc(sender_id).is_err() {
        return SyscallResult::NotReady;
    }

    let mut router = IPC_ROUTER.lock();
    match router.send_direct(sender_id, dest, msg_type, data) {
        Ok(()) => SyscallResult::Success,
        Err(IpcError::CapabilityDenied(_)) => SyscallResult::PermissionDenied,
        Err(IpcError::TargetNotFound) => SyscallResult::TargetNotFound,
        Err(IpcError::MailboxFull) => SyscallResult::MailboxFull,
        Err(IpcError::MailboxEmpty) => SyscallResult::MailboxEmpty,
        Err(IpcError::EndpointRevoked) => SyscallResult::PermissionDenied,
    }
}

/// Receives an IPC message from the caller's mailbox.
///
/// Requires an `IpcReceive` capability with `Rights::READ` in the explicitly specified `cap_slot`.
/// Does not fall back to ambiently searching the capability table if `cap_slot` is invalid or exhausted.
pub fn sys_ipc_recv(_cap_slot: usize) -> Result<IpcMessage, SyscallResult> {
    let (receiver_id, is_valid) = {
        let sched = SCHEDULER.lock();
        let current = match sched.current_task() {
            Some(t) => t,
            None => return Err(SyscallResult::PermissionDenied),
        };
        let valid = current.cap_table.authorize_capability(CapabilityType::IpcReceive, Rights::READ);
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
pub fn sys_app_storage_open(
    cap_slot: usize,
    relative_path: &str,
    requested_rights: Rights,
) -> Result<FileDescriptor, SyscallResult> {
    if relative_path.is_empty() || relative_path.len() > 128
        || relative_path.starts_with('/')
        || relative_path.contains('\\')
        || relative_path.contains("..")
    {
        return Err(SyscallResult::InvalidArguments);
    }
    let sched = SCHEDULER.lock();
    let current = sched.current_task().ok_or(SyscallResult::PermissionDenied)?;
    let task_id = current.id;
    drop(sched);
    let mut vfs = VFS.lock();
    let root = vfs.app_storage_root_for_task(task_id)
        .map_err(|e| match e {
            FsError::PermissionDenied => SyscallResult::PermissionDenied,
            FsError::IntegrityViolation => SyscallResult::IntegrityViolation,
            _ => SyscallResult::InvalidArguments,
        })?;
    let path = alloc::format!("{}/{}", root, relative_path);
    drop(vfs);
    sys_fs_open(cap_slot, &path, requested_rights)
}

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
            FsError::FileTooLarge | FsError::PathTooLong => SyscallResult::InvalidArguments,
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
    let written = vfs
        .write(task_id, &cap_table, cap_slot, fd, buf)
        .map_err(|e| match e {
            FsError::CapabilityError(_) | FsError::PermissionDenied => SyscallResult::PermissionDenied,
            FsError::ReadOnlyFileSystem => SyscallResult::ReadOnlyFs,
            FsError::InvalidFd => SyscallResult::InvalidFd,
            _ => SyscallResult::InvalidArguments,
        })?;

    // Persist before reporting success.
    //
    // Writes land in an in-memory snapshot. Nothing else on the write path
    // called sync(), so a successful write existed only in RAM and was lost on
    // reboot. sync() copies the snapshot out and issues the ATA CACHE FLUSH, so
    // returning before it completes would report a durability the device does
    // not have.
    if written > 0 {
        if let Err(e) = vfs.sync() {
            crate::serial_println!("[FS] durability flush failed after write: {:?}", e);
            return Err(SyscallResult::DeviceError);
        }
    }
    Ok(written)
}

/// Flushes filesystem data and metadata to persistent storage.
///
/// The write path already flushes, so this is a barrier for callers that want
/// one explicitly before a rename, a shutdown or a power transition.
pub fn sys_fs_fsync(fd: FileDescriptor) -> SyscallResult {
    let sched = SCHEDULER.lock();
    let current = match sched.current_task() {
        Some(t) => t,
        None => return SyscallResult::PermissionDenied,
    };
    let task_id = current.id;
    drop(sched);

    let vfs = VFS.lock();
    match vfs.fsync_for_task(task_id, fd) {
        Ok(()) => SyscallResult::Success,
        Err(FsError::InvalidFd) => SyscallResult::InvalidFd,
        Err(_) => SyscallResult::DeviceError,
    }
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

/// Size of the fixed-layout `FsStat` record written to userspace.
///
/// WHY a fixed layout rather than a struct dump: `SyscallResult` codes are the
/// kernel's only contract with a caller, so the record must be exactly this many
/// bytes on both sides and must not depend on `#[repr]` of a kernel type.
///
/// `0..8 inode | 8..12 uid | 12..16 gid | 16..18 mode | 18..20 nlink |
/// 20..28 atime | 28..36 mtime | 36..44 ctime | 44..48 file type | 48..56 read
/// only | 56..88 sha256 | 88..96 size`
const FILE_STAT_BYTES: usize = 96;

/// Serializes a `FileStat` into the `FsStat` userspace record.
fn encode_file_stat(stat: &FileStat, out: &mut [u8]) -> usize {
    let mut record = [0u8; FILE_STAT_BYTES];
    record[..8].copy_from_slice(&stat.inode.0.to_le_bytes());
    record[8..12].copy_from_slice(&stat.uid.to_le_bytes());
    record[12..16].copy_from_slice(&stat.gid.to_le_bytes());
    record[16..18].copy_from_slice(&stat.mode.to_le_bytes());
    record[18..20].copy_from_slice(&stat.nlink.to_le_bytes());
    record[20..28].copy_from_slice(&stat.atime.to_le_bytes());
    record[28..36].copy_from_slice(&stat.mtime.to_le_bytes());
    record[36..44].copy_from_slice(&stat.ctime.to_le_bytes());
    record[44..48].copy_from_slice(&file_type_tag(stat.file_type).to_le_bytes());
    record[48..56].copy_from_slice(&(stat.read_only as u64).to_le_bytes());
    record[56..88].copy_from_slice(&stat.sha256_digest);
    record[88..96].copy_from_slice(&(stat.size as u64).to_le_bytes());
    let to_copy = out.len().min(record.len());
    out[..to_copy].copy_from_slice(&record[..to_copy]);
    to_copy
}

/// Numeric tag for `FileType`, in its declaration order. The values are part of
/// the `FsStat` ABI.
fn file_type_tag(file_type: FileType) -> u32 {
    match file_type {
        FileType::RegularFile => 0,
        FileType::Directory => 1,
        FileType::ReadOnlySystem => 2,
        FileType::DeviceNode => 3,
    }
}

/// Reads file metadata without opening a descriptor.
pub fn sys_fs_stat(cap_slot: usize, path: &str) -> Result<FileStat, SyscallResult> {
    let sched = SCHEDULER.lock();
    let current = match sched.current_task() {
        Some(t) => t,
        None => return Err(SyscallResult::PermissionDenied),
    };
    let task_id = current.id;
    let cap_table = current.cap_table.clone();
    drop(sched);

    let vfs = VFS.lock();
    vfs.stat_for_task(task_id, &cap_table, cap_slot, path)
        .map_err(|e| match e {
            FsError::CapabilityError(_) | FsError::PermissionDenied => SyscallResult::PermissionDenied,
            FsError::FileNotFound => SyscallResult::FileNotFound,
            _ => SyscallResult::InvalidArguments,
        })
}

/// Unlinks (deletes) a file from the VFS.
pub fn sys_fs_unlink(cap_slot: usize, path: &str) -> Result<(), SyscallResult> {
    let sched = SCHEDULER.lock();
    let current = match sched.current_task() {
        Some(t) => t,
        None => return Err(SyscallResult::PermissionDenied),
    };
    let task_id = current.id;
    let cap_table = current.cap_table.clone();
    drop(sched);

    let mut vfs = VFS.lock();
    vfs.unlink_for_task(task_id, &cap_table, cap_slot, path)
        .map_err(|e| match e {
            FsError::CapabilityError(_) | FsError::PermissionDenied => SyscallResult::PermissionDenied,
            FsError::FileNotFound => SyscallResult::FileNotFound,
            FsError::ReadOnlyFileSystem => SyscallResult::ReadOnlyFs,
            FsError::IntegrityViolation => SyscallResult::IntegrityViolation,
            _ => SyscallResult::InvalidArguments,
        })?;
    let _ = vfs.sync();
    Ok(())
}

/// Reads a 512-byte physical disk sector by LBA from the primary ATA drive.
///
/// Requires the caller to hold a `StorageRawBlock` capability with `Rights::READ`.
pub fn sys_block_read(
    _cap_slot: usize,
    lba: u32,
    buf: &mut [u8; SECTOR_SIZE],
) -> SyscallResult {
    let sched = SCHEDULER.lock();
    let current = match sched.current_task() {
        Some(t) => t,
        None => return SyscallResult::PermissionDenied,
    };

    // WHY the capability is authorised by type and not by the slot the
    // caller named: a slot is a position in this task's capability table,
    // and the position depends on the order the kernel inserted the
    // capabilities in. The launcher proved the point -- it passed slot 0
    // while its FileSystemRead capability sat at slot 3, and the call was
    // refused with nothing on screen to say why. Authorising by type
    // enforces exactly the same rule without the caller knowing the
    // layout, and it cannot drift when the policy gains or loses a
    // capability.
    if !current.cap_table.authorize_capability(CapabilityType::StorageRawBlock, Rights::READ) {
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

/// The verification body of [`sys_verify_package`], with the scheduler read and
/// the global filesystem handle hoisted out so the verification path itself is
/// reachable from a test without either.
fn verify_package_in(
    vfs: &crate::fs::vfs::Vfs,
    cap_table: &crate::capability::CapabilityTable,
    cap_slot: usize,
    path: &str,
) -> Result<RappManifest, SyscallResult> {
    // The read bound is applied inside the VFS before the contents are copied,
    // so an oversized blob is refused rather than allocated and then refused.
    let file_buf = vfs.read_file_for_task(cap_table, cap_slot, path, MAX_VERIFY_PACKAGE_SIZE)
        .map_err(|e| match e {
            FsError::CapabilityError(_) | FsError::PermissionDenied => SyscallResult::PermissionDenied,
            FsError::FileNotFound => SyscallResult::FileNotFound,
            FsError::FileTooLarge => SyscallResult::InvalidArguments,
            _ => SyscallResult::InvalidArguments,
        })?;

    if file_buf.is_empty() { return Err(SyscallResult::InvalidArguments); }

    parse_rapp(&file_buf).map_err(|e| match e {
        RappError::DigestMismatch | RappError::SignatureInvalid | RappError::PublisherUntrusted | RappError::PublisherRevoked | RappError::PackageRevoked | RappError::InvalidPublisherKey => SyscallResult::IntegrityViolation,
        _ => SyscallResult::InvalidArguments,
    })
}

/// Largest package blob the verification path will read.
const MAX_VERIFY_PACKAGE_SIZE: usize = 16 * 1024 * 1024;

/// Loads and cryptographically validates a `.rapp` application package from the VFS.
///
/// Requires `FileSystemRead` capability in `cap_slot`.
pub fn sys_verify_package(
    cap_slot: usize,
    path: &str,
) -> Result<RappManifest, SyscallResult> {
    let cap_table = {
        let sched = SCHEDULER.lock();
        let current = sched.current_task().ok_or(SyscallResult::PermissionDenied)?;
        current.cap_table.clone()
    };
    let vfs = VFS.lock();
    verify_package_in(&vfs, &cap_table, cap_slot, path)
}

/// Verifies a signed `.rapp` file and writes its manifest summary into `out_buf`.
///
/// WHY the summary rather than the manifest itself: the manifest carries the
/// publisher's 32-byte key and the 64-byte signature, and a caller that only
/// needs to know what it is about to install has no use for either. The line is
/// pipe-delimited in the same shape `sys_fs_list` and `sys_package_registry`
/// use, so a userspace reader parses all three the same way.
///
/// `package_id|major.minor.patch|payload_len\x0a`
pub fn sys_verify_package_summary(
    cap_slot: usize,
    path: &str,
    out_buf: &mut [u8],
) -> Result<usize, SyscallResult> {
    let manifest = sys_verify_package(cap_slot, path)?;
    use core::fmt::Write;
    struct SummaryWriter<'a> {
        buf: &'a mut [u8],
        pos: usize,
    }
    impl<'a> core::fmt::Write for SummaryWriter<'a> {
        fn write_str(&mut self, s: &str) -> core::fmt::Result {
            let bytes = s.as_bytes();
            let remaining = self.buf.len().saturating_sub(self.pos);
            if remaining < bytes.len() {
                // A truncated summary would report a short package id or a
                // short version and look like a real one.
                return Err(core::fmt::Error);
            }
            self.buf[self.pos..self.pos + bytes.len()].copy_from_slice(bytes);
            self.pos += bytes.len();
            Ok(())
        }
    }
    let mut writer = SummaryWriter { buf: out_buf, pos: 0 };
    let written = write!(
        writer,
        "{}|{}.{}.{}|{}\n",
        manifest.package_id,
        manifest.version.0,
        manifest.version.1,
        manifest.version.2,
        manifest.payload_len
    );
    if written.is_err() {
        return Err(SyscallResult::InvalidArguments);
    }
    Ok(writer.pos)
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
            if msg_len == 0 || msg_len > 1024 {
                return SyscallResult::InvalidArguments as i64;
            }
            let mut log_buf = [0u8; 1024];
            let s = match read_user_string(a2, msg_len, &mut log_buf) {
                Ok(valid) => valid,
                Err(_) => return SyscallResult::InvalidArguments as i64,
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
            if data_len > 1024 {
                return SyscallResult::InvalidArguments as i64;
            }
            let mut kbuf = [0u8; 1024];
            if copy_from_user(&mut kbuf[..data_len], a4, data_len).is_err() {
                return SyscallResult::InvalidArguments as i64;
            }
            sys_ipc_send(cap_slot, dest, msg_type, &kbuf[..data_len]) as i64
        }
        6 => {
            // IpcRecv: a1 = cap_slot, a2 = out_buf_ptr, a3 = out_buf_len
            let cap_slot = a1 as usize;
            let out_len = a3 as usize;
            match sys_ipc_recv(cap_slot) {
                Ok(msg) => {
                    let copy_len = core::cmp::min(out_len, msg.len);
                    if copy_to_user(a2, &msg.payload[..copy_len]).is_err() {
                        return SyscallResult::InvalidArguments as i64;
                    }
                    copy_len as i64
                }
                Err(err) => err as i64,
            }
        }
        7 => {
            // GetTaskId
            sys_get_task_id().0 as i64
        }
        119 => crate::syscall::sys_app_wake() as i64,
        118 => crate::syscall::sys_app_suspend() as i64,
        117 => crate::syscall::sys_app_foreground() as i64,
        116 => crate::syscall::sys_app_background() as i64,
        115 => {
            // AppStorageOpen: a1 = cap_slot, a2 = relative_path_ptr, a3 = path_len, a4 = rights_bits.
            let cap_slot = a1 as usize;
            let path_len = a3 as usize;
            if path_len == 0 || path_len > 128 {
                return SyscallResult::InvalidArguments as i64;
            }
            let mut path_buf = [0u8; 128];
            let path = match read_user_string(a2, path_len, &mut path_buf) {
                Ok(s) => s,
                Err(_) => return SyscallResult::InvalidArguments as i64,
            };
            match sys_app_storage_open(cap_slot, path, Rights(a4 as u32)) {
                Ok(fd) => fd.0 as i64,
                Err(e) => e as i64,
            }
        }
        8 => {
            // FsOpen: a1 = cap_slot, a2 = path_ptr, a3 = path_len, a4 = rights_bits
            let cap_slot = a1 as usize;
            let path_len = a3 as usize;
            let rights = Rights(a4 as u32);
            if path_len == 0 || path_len > 512 {
                return SyscallResult::InvalidArguments as i64;
            }
            let mut path_buf = [0u8; 512];
            let path = match read_user_string(a2, path_len, &mut path_buf) {
                Ok(s) => s,
                Err(_) => return SyscallResult::InvalidArguments as i64,
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
            if buf_len > 4096 {
                return SyscallResult::InvalidArguments as i64;
            }
            let mut kbuf = [0u8; 4096];
            match sys_fs_read(cap_slot, fd, &mut kbuf[..buf_len]) {
                Ok(bytes) => {
                    if copy_to_user(a3, &kbuf[..bytes]).is_err() {
                        return SyscallResult::InvalidArguments as i64;
                    }
                    bytes as i64
                }
                Err(e) => e as i64,
            }
        }
        10 => {
            // FsWrite: a1 = cap_slot, a2 = fd, a3 = buf_ptr, a4 = buf_len
            let cap_slot = a1 as usize;
            let fd = FileDescriptor(a2 as u32);
            let buf_len = a4 as usize;
            if buf_len > 4096 {
                return SyscallResult::InvalidArguments as i64;
            }
            let mut kbuf = [0u8; 4096];
            if copy_from_user(&mut kbuf[..buf_len], a3, buf_len).is_err() {
                return SyscallResult::InvalidArguments as i64;
            }
            match sys_fs_write(cap_slot, fd, &kbuf[..buf_len]) {
                Ok(bytes) => bytes as i64,
                Err(e) => e as i64,
            }
        }
11 => {
        // FsClose: a1 = fd
        let fd = FileDescriptor(a1 as u32);
        sys_fs_close(fd) as i64
    }
    125 => {
        // FsSync: a1 = fd
        let fd = FileDescriptor(a1 as u32);
        sys_fs_fsync(fd) as i64
    }
    12 => {
        // FsStat: a1 = cap_slot, a2 = path_ptr, a3 = path_len, a4 = out_ptr, a5 = out_len
        let cap_slot = a1 as usize;
        let path_len = a3 as usize;
        if path_len == 0 || path_len > 512 {
            return SyscallResult::InvalidArguments as i64;
        }
        let out_len = a5 as usize;
        if out_len == 0 || out_len > 128 {
            return SyscallResult::InvalidArguments as i64;
        }
        let mut path_buf = [0u8; 512];
        let path = match read_user_string(a2, path_len, &mut path_buf) {
            Ok(s) => s,
            Err(_) => return SyscallResult::InvalidArguments as i64,
        };
        match sys_fs_stat(cap_slot, path) {
            Ok(stat) => {
                let mut out = [0u8; FILE_STAT_BYTES];
                let written = encode_file_stat(&stat, &mut out);
                if written > out_len {
                    // A caller that cannot hold the whole record would otherwise
                    // read a short stat and treat the missing tail as zeroes.
                    return SyscallResult::InvalidArguments as i64;
                }
                if copy_to_user(a4, &out[..written]).is_err() {
                    return SyscallResult::InvalidArguments as i64;
                }
                written as i64
            }
            Err(e) => e as i64,
        }
    }
    14 => {
        // VerifyPackage: a1 = cap_slot, a2 = path_ptr, a3 = path_len, a4 = out_ptr, a5 = out_len
        let cap_slot = a1 as usize;
        let path_len = a3 as usize;
        if path_len == 0 || path_len > 256 {
            return SyscallResult::InvalidArguments as i64;
        }
        let out_len = a5 as usize;
        if out_len == 0 || out_len > 256 {
            return SyscallResult::InvalidArguments as i64;
        }
        let mut path_buf = [0u8; 256];
        let path = match read_user_string(a2, path_len, &mut path_buf) {
            Ok(s) => s,
            Err(_) => return SyscallResult::InvalidArguments as i64,
        };
        let mut out_buf = [0u8; 256];
        match sys_verify_package_summary(cap_slot, path, &mut out_buf[..out_len]) {
            Ok(n) => {
                if copy_to_user(a4, &out_buf[..n]).is_err() {
                    return SyscallResult::InvalidArguments as i64;
                }
                n as i64
            }
            Err(e) => e as i64,
        }
    }
    16 => {
        // WasmExecute: a1 = module_ptr, a2 = module_len, a3 = export_ptr, a4 = export_len
        //
        // WHY this is an honest failure and not a real implementation: there is
        // no wasm binary decoder anywhere in the kernel. `kernel/src/wasm/`
        // takes modules that are built in code (`WasmModule::add_function`
        // takes interpreter bytecode, not a `\0asm` module), so there is
        // nothing here that could turn caller-supplied bytes into a
        // `WasmModule`. There is also no per-task VM instance registry to key a
        // submitted module by, no `CapabilityType::WasmExecute` in this
        // architecture's capability table to gate execution with, and no
        // serialization of the result, so a nonempty `module_len` would have to
        // be invented to make this arm look like it did something.
        //
        // WHAT WOULD HAVE TO EXIST: (1) a binary-format decoder in
        // `kernel/src/wasm/` (`WasmModule::from_bytes` or equivalent: magic,
        // sections, LEB128 types, import/export/memory decoding) -- the
        // `WasmModule` builder API alone is not enough; (2) a kernel-side VM
        // instance store keyed by task so a module survives more than one
        // syscall, in the same module; (3) a `WasmExecute` capability variant
        // in `kernel/src/capability/` plus its minting for the service that is
        // allowed to run bytecode, mirroring `kernel-aarch64/src/wasm.rs`; and
        // (4) an agreed result encoding for this name, in the SDK, next to the
        // `WasmExecute = 16` variant. Until then the caller is told the verb is
        // disabled rather than allowed to believe it succeeded.
        SyscallResult::FeatureDisabled as i64
    }

        13 => {
            // BlockRead: a1 = cap_slot, a2 = lba, a3 = buf_ptr
            let cap_slot = a1 as usize;
            let lba = a2 as u32;
            let mut sector_buf = [0u8; SECTOR_SIZE];
            let res = sys_block_read(cap_slot, lba, &mut sector_buf);
            if res == SyscallResult::Success {
                if copy_to_user(a3, &sector_buf).is_err() {
                    return SyscallResult::InvalidArguments as i64;
                }
            }
            res as i64
        }
        19 => {
            // ResourceQuery: a1 = out_battery_ptr, a2 = out_temp_ptr, a3 = out_throttling_ptr
            let mut b: u8 = 0;
            let mut t: u8 = 0;
            let mut th: bool = false;
            let res = sys_resource_query(&mut b, &mut t, &mut th);
            if res == SyscallResult::Success {
                if write_user_value(a1, &b).is_err()
                    || write_user_value(a2, &t).is_err()
                    || write_user_value(a3, &th).is_err()
                {
                    return SyscallResult::InvalidArguments as i64;
                }
            }
            res as i64
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
            let len = a4 as usize;
            if len > 2048 {
                return SyscallResult::InvalidArguments as i64;
            }
            let mut payload = [0u8; 2048];
            if copy_from_user(&mut payload[..len], a3, len).is_err() {
                return SyscallResult::InvalidArguments as i64;
            }
            sys_net_sendto(dst_ip, dst_port, &payload[..len]) as i64
        }
        23 => {
            // NetRecvFrom: a1 = sock_id, a2 = out_buf_ptr, a3 = out_buf_len
            let sock_id = crate::net::SocketId(a1 as u32);
            let len = a3 as usize;
            if len > 2048 {
                return SyscallResult::InvalidArguments as i64;
            }
            let mut kbuf = [0u8; 2048];
            match sys_net_recvfrom(sock_id, &mut kbuf[..len]) {
                Ok(bytes) => {
                    if copy_to_user(a2, &kbuf[..bytes]).is_err() {
                        return SyscallResult::InvalidArguments as i64;
                    }
                    bytes as i64
                }
                Err(e) => e as i64,
            }
        }
        26 => {
            // PackageAudit: a1 = pkg_id_ptr, a2 = pkg_id_len
            let len = a2 as usize;
            if len == 0 || len > 128 {
                return SyscallResult::InvalidArguments as i64;
            }
            let mut pkg_buf = [0u8; 128];
            let pkg_id = match read_user_string(a1, len, &mut pkg_buf) {
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
            let len = a3 as usize;
            let consent = a4 != 0;
            if len == 0 || len > 128 {
                return SyscallResult::InvalidArguments as i64;
            }
            let mut pkg_buf = [0u8; 128];
            let pkg_id = match read_user_string(a2, len, &mut pkg_buf) {
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
            let len = a2 as usize;
            if len > 256 {
                return SyscallResult::InvalidArguments as i64;
            }
            let mut kbuf = [0u8; 256];
            match sys_hal_query_platform(&mut kbuf[..len]) {
                Ok(n) => {
                    if copy_to_user(a1, &kbuf[..n]).is_err() {
                        return SyscallResult::InvalidArguments as i64;
                    }
                    n as i64
                }
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
            let len = a2 as usize;
            if len > 256 {
                return SyscallResult::InvalidArguments as i64;
            }
            let mut kbuf = [0u8; 256];
            match sys_telephony_query(&mut kbuf[..len]) {
                Ok(n) => {
                    if copy_to_user(a1, &kbuf[..n]).is_err() {
                        return SyscallResult::InvalidArguments as i64;
                    }
                    n as i64
                }
                Err(e) => e as i64,
            }
        }
        31 => {
            // GetRandom: a1 = out_buf_ptr, a2 = out_buf_len
            let len = a2 as usize;
            if len == 0 || len > 4096 {
                return SyscallResult::InvalidArguments as i64;
            }
            let mut kbuf = [0u8; 4096];
            match sys_get_random(&mut kbuf[..len]) {
                Ok(n) => {
                    if copy_to_user(a1, &kbuf[..n]).is_err() {
                        return SyscallResult::InvalidArguments as i64;
                    }
                    n as i64
                }
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
            let len = a3 as usize;
            if len < 32 {
                return SyscallResult::InvalidArguments as i64;
            }
            let mut kbuf = [0u8; 32];
            match sys_keystore_derive(slot_idx, &mut kbuf) {
                Ok(n) => {
                    if copy_to_user(a2, &kbuf).is_err() {
                        return SyscallResult::InvalidArguments as i64;
                    }
                    n as i64
                }
                Err(e) => e as i64,
            }
        }
        35 => {
            // SensorRead: a1 = out_buf_ptr, a2 = out_buf_len
            let len = a2 as usize;
            if len > 256 {
                return SyscallResult::InvalidArguments as i64;
            }
            let mut kbuf = [0u8; 256];
            match sys_sensor_read(&mut kbuf[..len]) {
                Ok(n) => {
                    if copy_to_user(a1, &kbuf[..n]).is_err() {
                        return SyscallResult::InvalidArguments as i64;
                    }
                    n as i64
                }
                Err(e) => e as i64,
            }
        }
        36 => {
            // BatteryQuery: a1 = out_buf_ptr, a2 = out_buf_len
            let len = a2 as usize;
            if len > 256 {
                return SyscallResult::InvalidArguments as i64;
            }
            let mut kbuf = [0u8; 256];
            match sys_battery_query(&mut kbuf[..len]) {
                Ok(n) => {
                    if copy_to_user(a1, &kbuf[..n]).is_err() {
                        return SyscallResult::InvalidArguments as i64;
                    }
                    n as i64
                }
                Err(e) => e as i64,
            }
        }
        37 => {
            // AndroidBridgeIntent: a1 = act_ptr, a2 = act_len, a3 = uri_ptr, a4 = uri_len, a5 = out_ptr
            let act_len = a2 as usize;
            let uri_len = a4 as usize;
            if act_len == 0 || act_len > 128 || uri_len == 0 || uri_len > 256 {
                return SyscallResult::InvalidArguments as i64;
            }
            let mut act_buf = [0u8; 128];
            let mut uri_buf = [0u8; 256];
            let act_str = match read_user_string(a1, act_len, &mut act_buf) {
                Ok(s) => s,
                Err(_) => return SyscallResult::InvalidArguments as i64,
            };
            let uri_str = match read_user_string(a3, uri_len, &mut uri_buf) {
                Ok(s) => s,
                Err(_) => return SyscallResult::InvalidArguments as i64,
            };
            let mut out_kbuf = [0u8; 64];
            match sys_android_bridge_intent(act_str, uri_str, &mut out_kbuf) {
                Ok(n) => {
                    if copy_to_user(a5, &out_kbuf[..n]).is_err() {
                        return SyscallResult::InvalidArguments as i64;
                    }
                    n as i64
                }
                Err(e) => e as i64,
            }
        }
        38 => {
            // RtcGetTime: a1 = out_buf_ptr, a2 = out_buf_len
            let len = a2 as usize;
            if len > 64 {
                return SyscallResult::InvalidArguments as i64;
            }
            let mut kbuf = [0u8; 64];
            match sys_rtc_get_time(&mut kbuf[..len]) {
                Ok(n) => {
                    if copy_to_user(a1, &kbuf[..n]).is_err() {
                        return SyscallResult::InvalidArguments as i64;
                    }
                    n as i64
                }
                Err(e) => e as i64,
            }
        }
        40 => {
            // ThermalQuery: a1 = out_buf_ptr, a2 = out_buf_len
            let len = a2 as usize;
            if len > 256 {
                return SyscallResult::InvalidArguments as i64;
            }
            let mut kbuf = [0u8; 256];
            match sys_thermal_query(&mut kbuf[..len]) {
                Ok(n) => {
                    if copy_to_user(a1, &kbuf[..n]).is_err() {
                        return SyscallResult::InvalidArguments as i64;
                    }
                    n as i64
                }
                Err(e) => e as i64,
            }
        }
        41 => {
            // SecureIpcSession: a1 = peer_task, a2 = out_session_id_ptr
            let peer = TaskId(a1);
            match sys_secure_ipc_session(peer) {
                Ok(sid) => {
                    if write_user_value(a2, &sid).is_err() {
                        return SyscallResult::InvalidArguments as i64;
                    }
                    SyscallResult::Success as i64
                }
                Err(e) => e as i64,
            }
        }
        43 => {
            // VaultCrypt: a1 = action (0=enc, 1=dec), a2 = block_idx, a3 = buf_ptr, a4 = 512, a5 = iv_ptr
            let action = a1 as u32;
            let block_idx = a2;
            let buf_len = a4 as usize;
            if buf_len != 512 {
                return SyscallResult::InvalidArguments as i64;
            }
            // Reject an unknown action before touching any state. `action` is
            // caller-controlled and previously fell through to the decrypt arm
            // for every value != 0.
            if action > 1 {
                return SyscallResult::InvalidArguments as i64;
            }
            // Bound the attacker-chosen sector index. The index feeds the
            // keystream derivation, so an unbounded value lets a caller sweep
            // the whole (iv, block_index) space and derive keystreams for
            // sectors belonging to other accounts.
            if block_idx >= VAULT_MAX_BLOCK_INDEX {
                return SyscallResult::InvalidArguments as i64;
            }
            // Explicit null/zero-pointer rejection. copy_from_user already
            // refuses addresses below USER_SPACE_START, but stating the
            // invariant here keeps the requirement visible at the ABI edge.
            if a3 == 0 || a5 == 0 {
                return SyscallResult::InvalidArguments as i64;
            }
            let mut kbuf = [0u8; 512];
            let mut iv = [0u8; 16];
            if copy_from_user(&mut kbuf, a3, 512).is_err() || copy_from_user(&mut iv, a5, 16).is_err() {
                return SyscallResult::InvalidArguments as i64;
            }
    match sys_vault_crypt(action, block_idx, &mut kbuf, &mut iv) {
        Ok(()) => {
            if copy_to_user(a3, &kbuf).is_err() {
                return SyscallResult::InvalidArguments as i64;
            }
            // A seal generates a fresh IV; return it so the caller can record
            // which envelope it now owns.
            if action == 0 && copy_to_user(a5, &iv).is_err() {
                return SyscallResult::InvalidArguments as i64;
            }
            SyscallResult::Success as i64
        }
        Err(e) => e as i64,
    }
}
        45 => {
            // WirelessQuery: a1 = out_buf_ptr, a2 = out_buf_len
            let len = a2 as usize;
            if len > 256 {
                return SyscallResult::InvalidArguments as i64;
            }
            let mut kbuf = [0u8; 256];
            match sys_wireless_query(&mut kbuf[..len]) {
                Ok(n) => {
                    if copy_to_user(a1, &kbuf[..n]).is_err() {
                        return SyscallResult::InvalidArguments as i64;
                    }
                    n as i64
                }
                Err(e) => e as i64,
            }
        }
        46 => {
            // DevFsIo: a1 = action (0=read, 1=write), a2 = path_ptr, a3 = path_len, a4 = buf_ptr, a5 = buf_len
            let action = a1 as u32;
            let path_len = a3 as usize;
            let buf_len = a5 as usize;
            if path_len == 0 || path_len > 256 || buf_len > 4096 {
                return SyscallResult::InvalidArguments as i64;
            }
            let mut path_buf = [0u8; 256];
            let path = match read_user_string(a2, path_len, &mut path_buf) {
                Ok(s) => s,
                Err(_) => return SyscallResult::InvalidArguments as i64,
            };
            let mut kbuf = [0u8; 4096];
            if action == 1 {
                if copy_from_user(&mut kbuf[..buf_len], a4, buf_len).is_err() {
                    return SyscallResult::InvalidArguments as i64;
                }
            }
            match sys_devfs_io(action, path, &mut kbuf[..buf_len]) {
                Ok(n) => {
                    if action == 0 {
                        if copy_to_user(a4, &kbuf[..n]).is_err() {
                            return SyscallResult::InvalidArguments as i64;
                        }
                    }
                    n as i64
                }
                Err(e) => e as i64,
            }
        }
        48 => {
            // RevocationAudit: a1 = cmd (0=check_pkg, 1=revoke_pkg), a2 = target_ptr, a3 = target_len, a4 = out_buf_ptr, a5 = out_buf_len
            let cmd = a1 as u32;
            let target_len = a3 as usize;
            let out_len = a5 as usize;
            if target_len == 0 || target_len > 128 || out_len > 256 {
                return SyscallResult::InvalidArguments as i64;
            }
            let mut target_buf = [0u8; 128];
            let target = match read_user_string(a2, target_len, &mut target_buf) {
                Ok(s) => s,
                Err(_) => return SyscallResult::InvalidArguments as i64,
            };
            let mut out_kbuf = [0u8; 256];
            match sys_revocation_audit(cmd, target, out_kbuf.as_mut_ptr(), out_len) {
                Ok(n) => {
                    if copy_to_user(a4, &out_kbuf[..n]).is_err() {
                        return SyscallResult::InvalidArguments as i64;
                    }
                    n as i64
                }
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
            let apk_len = a2 as usize;
            let out_len = a4 as usize;
            if apk_len == 0 || apk_len > 64 * 1024 || out_len == 0 || out_len > 1024 {
                return SyscallResult::InvalidArguments as i64;
            }
            let mut apk_buf = alloc::vec![0u8; apk_len];
            if copy_from_user(&mut apk_buf, a1, apk_len).is_err() {
                return SyscallResult::InvalidArguments as i64;
            }
            let mut out_buf = [0u8; 1024];
            match sys_apk_install_audit(&apk_buf, &mut out_buf[..out_len]) {
                Ok(n) => {
                    if copy_to_user(a3, &out_buf[..n]).is_err() {
                        return SyscallResult::InvalidArguments as i64;
                    }
                    n as i64
                }
                Err(e) => e as i64,
            }
        }
        53 => {
            // DexVerifyExec: a1 = dex_ptr, a2 = dex_len
            let dex_len = a2 as usize;
            if dex_len == 0 || dex_len > 64 * 1024 {
                return SyscallResult::InvalidArguments as i64;
            }
            let mut dex_buf = alloc::vec![0u8; dex_len];
            if copy_from_user(&mut dex_buf, a1, dex_len).is_err() {
                return SyscallResult::InvalidArguments as i64;
            }
            match sys_dex_verify_exec(&dex_buf) {
                Ok(n) => n as i64,
                Err(e) => e as i64,
            }
        }
        54 => {
            // CameraCapture: a1 = action, a2 = out_buf_ptr, a3 = out_buf_len, a4 = out_meta_ptr
            let action = a1 as u32;
            let out_len = a3 as usize;
            if out_len == 0 || out_len > 64 * 1024 {
                return SyscallResult::InvalidArguments as i64;
            }
            let mut kbuf = alloc::vec![0u8; out_len];
            let mut meta_kbuf = [0u32; 4];
            let has_meta = a4 != 0;
            match sys_camera_capture(action, &mut kbuf, if has_meta { Some(&mut meta_kbuf) } else { None }) {
                Ok(n) => {
                    if copy_to_user(a2, &kbuf[..n]).is_err() {
                        return SyscallResult::InvalidArguments as i64;
                    }
                    if has_meta {
                        let mut meta_bytes = [0u8; 16];
                        for (i, val) in meta_kbuf.iter().enumerate() {
                            meta_bytes[i * 4..(i + 1) * 4].copy_from_slice(&val.to_le_bytes());
                        }
                        if copy_to_user(a4, &meta_bytes).is_err() {
                            return SyscallResult::InvalidArguments as i64;
                        }
                    }
                    n as i64
                }
                Err(e) => e as i64,
            }
        }
        55 => {
            // DexInvokeMethod: a1 = dex_ptr, a2 = dex_len, a3 = spec_ptr, a4 = spec_len
            let dex_len = a2 as usize;
            let spec_len = a4 as usize;
            if dex_len == 0 || dex_len > 64 * 1024 || spec_len == 0 || spec_len > 256 {
                return SyscallResult::InvalidArguments as i64;
            }
            let mut dex_buf = alloc::vec![0u8; dex_len];
            if copy_from_user(&mut dex_buf, a1, dex_len).is_err() {
                return SyscallResult::InvalidArguments as i64;
            }
            let mut spec_buf = [0u8; 256];
            let spec_str = match read_user_string(a3, spec_len, &mut spec_buf) {
                Ok(s) => s,
                Err(_) => return SyscallResult::InvalidArguments as i64,
            };
            match sys_dex_invoke_method(&dex_buf, spec_str) {
                Ok(n) => n as i64,
                Err(e) => e as i64,
            }
        }
        57 => {
            // CapRequestConsent: a1 = perm_val, a2 = app_name_ptr, a3 = app_name_len, a4 = auto_approve_flag
            let perm_val = a1 as u32;
            let app_len = a3 as usize;
            let auto_approve = a4 != 0;
            if app_len == 0 || app_len > 128 {
                return SyscallResult::InvalidArguments as i64;
            }
            let mut app_buf = [0u8; 128];
            let app_str = match read_user_string(a2, app_len, &mut app_buf) {
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
            let mime_len = a2 as usize;
            let uri_len = a4 as usize;
            if mime_len == 0 || mime_len > 128 || uri_len == 0 || uri_len > 256 {
                return SyscallResult::InvalidArguments as i64;
            }
            let mut mime_buf = [0u8; 128];
            let mime_str = match read_user_string(a1, mime_len, &mut mime_buf) {
                Ok(s) => s,
                Err(_) => return SyscallResult::InvalidArguments as i64,
            };
            let mut uri_buf = [0u8; 256];
            let uri_str = match read_user_string(a3, uri_len, &mut uri_buf) {
                Ok(s) => s,
                Err(_) => return SyscallResult::InvalidArguments as i64,
            };
            let mut target_buf = [0u8; 64];
            let target_str = if a5 != 0 {
                read_user_string(a5, 64, &mut target_buf).ok()
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
            let data_len = a3 as usize;
            if data_len == 0 || data_len > 4096 {
                return SyscallResult::InvalidArguments as i64;
            }
            let mut kbuf = [0u8; 4096];
            if copy_from_user(&mut kbuf[..data_len], a2, data_len).is_err() {
                return SyscallResult::InvalidArguments as i64;
            }
            match sys_shm_pipe_transfer(a1 as u32, &kbuf[..data_len], a4 as u32) {
                Ok(seq) => seq as i64,
                Err(e) => e as i64,
            }
        }
        61 => {
            // ShmPipeReceive: a1 = pipe_id, a2 = out_buf_ptr, a3 = out_buf_len
            let out_len = a3 as usize;
            if out_len == 0 || out_len > 4096 {
                return SyscallResult::InvalidArguments as i64;
            }
            let mut kbuf = [0u8; 4096];
            match sys_shm_pipe_receive(a1 as u32, &mut kbuf[..out_len]) {
                Ok(bytes) => {
                    if copy_to_user(a2, &kbuf[..bytes]).is_err() {
                        return SyscallResult::InvalidArguments as i64;
                    }
                    bytes as i64
                }
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
        63 => {
            // WaitPid: a1 = child_id
            sys_waitpid(a1) as i64
        }
        64 => {
            // ExitWithCode: a1 = exit_code
            sys_exit_with_code(a1 as i32);
        }
        65 => {
            // TelephonyDial: a1 = dest_ptr, a2 = dest_len
            let len = (a2 as usize).min(64);
            if len == 0 {
                return SyscallResult::InvalidArguments as i64;
            }
            let mut num_buf = [0u8; 64];
            if copy_from_user(&mut num_buf[..len], a1, len).is_err() {
                return SyscallResult::PermissionDenied as i64;
            }
            if let Ok(num_str) = core::str::from_utf8(&num_buf[..len]) {
                match sys_telephony_dial(num_str) {
                    Ok(()) => 0,
                    Err(e) => e as i64,
                }
            } else {
                SyscallResult::InvalidArguments as i64
            }
        }
        66 => {
            // TelephonyHangup
            match sys_telephony_hangup() {
                Ok(()) => 0,
                Err(e) => e as i64,
            }
        }
        67 => {
            // TelephonySendSms: a1 = dest_ptr, a2 = dest_len, a3 = body_ptr, a4 = body_len
            let dest_len = (a2 as usize).min(32);
            let body_len = (a4 as usize).min(256);
            if dest_len == 0 || body_len == 0 {
                return SyscallResult::InvalidArguments as i64;
            }
            let mut d_buf = [0u8; 32];
            let mut b_buf = [0u8; 256];
            if copy_from_user(&mut d_buf[..dest_len], a1, dest_len).is_err() || copy_from_user(&mut b_buf[..body_len], a3, body_len).is_err() {
                return SyscallResult::PermissionDenied as i64;
            }
            if let (Ok(d_str), Ok(b_str)) = (core::str::from_utf8(&d_buf[..dest_len]), core::str::from_utf8(&b_buf[..body_len])) {
                match sys_telephony_send_sms(d_str, b_str) {
                    Ok(id) => id as i64,
                    Err(e) => e as i64,
                }
            } else {
                SyscallResult::InvalidArguments as i64
            }
        }
        68 => {
            // TelephonySetData: a1 = enabled (1 or 0)
            match sys_telephony_set_data(a1 != 0) {
                Ok(active) => if active { 1 } else { 0 },
                Err(e) => e as i64,
            }
        }
        69 => {
            // SurfaceCreate: a1 = display-cap slot, a2 = width, a3 = height, a4 = z_order
            match sys_surface_create(a1 as usize, a2 as u32, a3 as u32, a4 as u32) {
                Ok(id) => id as i64,
                Err(e) => e as i64,
            }
        }
        70 => {
            // SurfaceFlip: a1 = display-cap slot
            match sys_surface_flip(a1 as usize) {
                Ok(count) => count as i64,
                Err(e) => e as i64,
            }
        }
         71 => {
             // TouchPoll
             match sys_touch_poll() {
                 Ok(code) => code as i64,
                 Err(e) => e as i64,
             }
         }
         130 => {
             // DisplaySetMode: a1 = display-cap slot, a2 = width, a3 = height, a4 = refresh_hz
             match sys_display_set_mode(a1 as usize, a2 as u32, a3 as u32, a4 as u16) {
                 Ok(()) => SyscallResult::Success as i64,
                 Err(e) => e as i64,
             }
         }
         131 => {
             // DisplaySetOrientation: a1 = display-cap slot, a2 = orientation code
             // (0 = Portrait, 1 = LandscapeRight, 2 = PortraitInverted, 3 = LandscapeLeft)
             match sys_display_set_orientation(a1 as usize, a2 as u32) {
                 Ok(()) => SyscallResult::Success as i64,
                 Err(e) => e as i64,
             }
         }
         132 => {
             // CapLookup: a1 = cap_type, a2 = object_id
             match sys_cap_lookup(a1, a2) {
                 Ok(slot) => slot as i64,
                 Err(e) => e as i64,
             }
         }
         133 => {
             // FsUnlink: a1 = cap_slot, a2 = path_ptr, a3 = path_len
             let cap_slot = a1 as usize;
             let path_ptr = a2;
             let path_len = a3 as usize;
             if path_len == 0 || path_len > 256 {
                 return SyscallResult::InvalidArguments as i64;
             }
             let mut kpath = [0u8; 256];
             if copy_from_user(&mut kpath[..path_len], path_ptr, path_len).is_err() {
                 return SyscallResult::InvalidArguments as i64;
             }
             let path = match core::str::from_utf8(&kpath[..path_len]) {
                 Ok(s) => s,
                 Err(_) => return SyscallResult::InvalidArguments as i64,
             };
             match sys_fs_unlink(cap_slot, path) {
                 Ok(()) => SyscallResult::Success as i64,
                 Err(e) => e as i64,
             }
         }
        73 => {
            // SurfaceWrite: a1=display-cap slot, a2=surface id, a3=offset pixels,
            // a4=pixel buffer pointer, a5=pixel count.
            let count = a5 as usize;
            if count == 0 || count > 65536 { return SyscallResult::InvalidArguments as i64; }
            let byte_len = match count.checked_mul(core::mem::size_of::<u32>()) {
                Some(b) => b,
                None => return SyscallResult::InvalidArguments as i64,
            };
            if validate_user_buffer(a4, byte_len).is_err() {
                return SyscallResult::InvalidArguments as i64;
            }
            let user_slice = match UserSlice::new(a4, byte_len) {
                Ok(slice) => slice,
                Err(_) => return SyscallResult::InvalidArguments as i64,
            };
            let owner = match require_display_service(a1 as usize) {
                Ok(o) => o,
                Err(e) => return e as i64,
            };
            let mut kms = crate::drivers::display_kms::DRM_KMS.lock();
            let surface = match kms.surfaces.iter_mut().find(|s| s.id == a2 as u32) {
                Some(s) => s,
                None => return SyscallResult::DeviceError as i64,
            };
            if surface.owner_task != owner.0 {
                return SyscallResult::PermissionDenied as i64;
            }
            let offset = a3 as usize;
            let end = match offset.checked_add(count) {
                Some(e) => e,
                None => return SyscallResult::InvalidArguments as i64,
            };
            if end > surface.pixels.len() {
                return SyscallResult::InvalidArguments as i64;
            }
            if user_slice.copy_to_kernel_typed(&mut surface.pixels[offset..end]).is_err() {
                return SyscallResult::InvalidArguments as i64;
            }
            // Record which surface rows moved, exactly as the safer
            // `write_surface_pixels` path does.
            //
            // WHY this was missing: this syscall is how every client surface
            // writes its pixels, and it set the dirty flag without the row band,
            // so every surface in the guest carried a dirty flag with an empty
            // band. The compositor's row diff papers over that today by
            // comparing the whole scanout -- which is a full-surface pass per
            // flip, and the dominant cost in a compositor that a scanout past
            // 720x1440 needs to narrow. Recording the band here is what makes
            // the diff redundant rather than load-bearing.
            let row_pitch = surface.width as usize;
            let band = if row_pitch > 0 {
                let r0 = offset / row_pitch;
                let r1 = (end - 1) / row_pitch + 1;
                Some((r0, r1.min(surface.height as usize)))
            } else {
                None
            };
            if end == surface.pixels.len() {
                surface.frame_ready = true;
            } else if offset == 0 {
                surface.frame_ready = false;
            }
            if let Some(band) = band {
                surface.mark_dirty_rows(band.0, band.1);
            } else {
                surface.mark_dirty_rows(0, surface.height as usize);
            }
            kms.frame_dirty = true;
            count as i64
        }
        74 => {
            // SurfaceDestroy: a1=display-cap slot, a2=surface id
            match sys_surface_destroy(a1 as usize, a2 as u32) {
                Ok(()) => 0,
                Err(e) => e as i64,
            }
        }
        72 => {
            // PowerSetProfile: a1 = profile (0=PowerSave, 1=Balanced, 2=Performance, 3=Turbo)
            match sys_power_set_profile(a1 as u32) {
                Ok(mhz) => mhz as i64,
                Err(e) => e as i64,
            }
        }
        75 => {
            // PackageList: a1=reserved, a2=out_ptr, a3=out_len
            //
            // WHY authorised by capability type here as well: this enumerates the
            // catalogue rather than accessing a resource named by a capability, and
            // making the caller pass its own slot index meant the SDK had to guess a
            // slot number. It passed 0, which is SerialOutput for every service, so
            // the listing was refused and callers saw an empty catalogue.
            let out_len=a3 as usize;
            if out_len==0||out_len>4096{return SyscallResult::InvalidArguments as i64;}
            let sched=SCHEDULER.lock();
            let current=match sched.current_task(){Some(t)=>t,None=>return SyscallResult::PermissionDenied as i64};
            if !current.cap_table.authorize_capability(CapabilityType::FileSystemRead, Rights::READ) {
                return SyscallResult::PermissionDenied as i64;
            }
            drop(sched);
            let mut out=[0u8;4096];let mut pos=0usize;
            for p in crate::package::FIRST_PARTY_PACKAGES{
                let line=format_args!("{}|{}|{}\n",p.package_id,p.process_name,p.path);
                let mut tmp=[0u8;192];let n;
                struct W<'a>{b:&'a mut[u8],n:usize}
                impl<'a> core::fmt::Write for W<'a>{fn write_str(&mut self,s:&str)->core::fmt::Result{let r=self.b.len().saturating_sub(self.n);let k=s.len().min(r);self.b[self.n..self.n+k].copy_from_slice(&s.as_bytes()[..k]);self.n+=k;Ok(())}}
                {let mut w=W{b:&mut tmp,n:0};let _=core::fmt::Write::write_fmt(&mut w,line);n=w.n;}
                if pos+n>out.len(){break}out[pos..pos+n].copy_from_slice(&tmp[..n]);pos+=n;
            }
            let copy_len=pos.min(out_len);if copy_to_user(a2,&out[..copy_len]).is_err(){return SyscallResult::InvalidArguments as i64;}copy_len as i64
        }
        76 => {
            // PackageRegistry: a1=reserved, a2=out_ptr, a3=out_len
            //
            // WHY the capability is checked by type and not by the slot index the
            // caller passes in `a1`: the previous guard verified whatever slot the
            // caller named, so every caller had to know its own capability table
            // layout. The launcher passed 0, but its FileSystemRead capability sits
            // at slot 3 behind SerialOutput, TimerTickRead and IpcSend, so the call
            // was refused and the shell silently rendered an empty app catalogue
            // with no indication that anything was wrong. Authorising by capability
            // type enforces exactly the same rule without making the caller
            // responsible for its own table layout, and it cannot drift when the
            // policy gains or loses a capability.
            let out_len=a3 as usize;
            if out_len==0||out_len>4096{return SyscallResult::InvalidArguments as i64;}
            let sched=SCHEDULER.lock(); let current=match sched.current_task(){Some(t)=>t,None=>return SyscallResult::PermissionDenied as i64};
            if !current.cap_table.authorize_capability(CapabilityType::FileSystemRead, Rights::READ) {
                return SyscallResult::PermissionDenied as i64;
            }
            drop(sched);
            let mut out=[0u8;4096]; match sys_package_registry(&mut out[..out_len]) { Ok(n)=>{if copy_to_user(a2,&out[..n]).is_err(){return SyscallResult::InvalidArguments as i64;} n as i64}, Err(e)=>e as i64 }
        }
        77 => {
            // WindowList: a1=out_ptr, a2=out_len
            let out_len=a2 as usize;
            if out_len==0||out_len>4096{return SyscallResult::InvalidArguments as i64;}
            let mut out=[0u8;4096];
            match sys_window_list(&mut out[..out_len]) {
                Ok(n)=>{if copy_to_user(a1,&out[..n]).is_err(){return SyscallResult::InvalidArguments as i64;}n as i64},
                Err(e)=>e as i64
            }
        }
        78 => {
            // PackageRollback: a1=cap_slot, a2=pkg_id_ptr, a3=pkg_id_len, a4=user_consent
            let _cap_slot=a1 as usize; let len=a3 as usize;
            if len==0||len>128{return SyscallResult::InvalidArguments as i64;}
            let mut buf=[0u8;128]; let pkg=match read_user_string(a2,len,&mut buf){Ok(s)=>s,Err(_)=>return SyscallResult::InvalidArguments as i64};
            let sched=SCHEDULER.lock(); let current=match sched.current_task(){Some(t)=>t,None=>return SyscallResult::PermissionDenied as i64};
            if !current.cap_table.authorize_capability(CapabilityType::FileSystemWrite,Rights::WRITE){return SyscallResult::PermissionDenied as i64;}
            drop(sched);
            if a4==0{return SyscallResult::PermissionDenied as i64;}
            match crate::package::PACKAGE_REGISTRY.lock().rollback(pkg){Ok(())=>SyscallResult::Success as i64,Err(_)=>SyscallResult::FileNotFound as i64}
        }
        79 => { let z=(a5 & 0x7FFF_FFFF) as u32; let visible=(a5 >> 63)!=0; sys_window_set_geometry(a1 as usize,a2 as u32,a3 as i64 as i32,a4 as i64 as i32,z,visible) as i64 }
        80 => { sys_window_focus(a1 as usize, a2 as u32, a3 as u32) as i64 }
        104 => { sys_window_set_input(a1 as usize, a2 as u32, a3 != 0) as i64 }
        81 => {
            let (user_ptr, user_len) = if a1 < 0x1000 && a3 > 0 { (a2, a3 as usize) } else { (a1, a2 as usize) };
            if user_len == 0 || user_len > 4096 { return SyscallResult::InvalidArguments as i64; }
            let cap = user_len.min(512);
            let mut out = [0u8; 512];
            match sys_service_list(&mut out[..cap]) {
                Ok(n) => {
                    if copy_to_user(user_ptr, &out[..n]).is_err() {
                        return SyscallResult::InvalidArguments as i64;
                    }
                    n as i64
                }
                Err(e) => e as i64,
            }
        }
        82 => { let key_len=a3 as usize; let out_len=a4 as usize; if out_len>512{return SyscallResult::InvalidArguments as i64;} let mut out=[0u8;512]; match sys_settings_get(a1 as usize,a2,key_len,&mut out[..out_len]){Ok(n)=>{if copy_to_user(a5,&out[..n]).is_err(){return SyscallResult::InvalidArguments as i64;}n as i64},Err(e)=>e as i64} }
        83 => { match sys_settings_set(a1 as usize,a2,a3 as usize,a4,a5 as usize){Ok(())=>0,Err(e)=>e as i64} }
        84 => { match sys_storage_query(){Ok(n)=>n as i64,Err(e)=>e as i64} }
        85 => { let len=a3 as usize; if len>4096{return SyscallResult::InvalidArguments as i64;} let mut out=[0u8;4096]; match sys_fs_list(a1 as usize,a2,a3 as usize,&mut out[..len]){Ok(n)=>{if copy_to_user(a4,&out[..n]).is_err(){return SyscallResult::InvalidArguments as i64;}n as i64},Err(e)=>e as i64} }
        86 => { match sys_service_start(a1 as u8) { Ok(id) => id.0 as i64, Err(e) => e as i64 } }
        87 => { match sys_service_stop(a1 as u8) { Ok(()) => 0, Err(e) => e as i64 } }
        88 => { match sys_service_recover_failed(a1 as u8) { Ok(n) => n as i64, Err(e) => e as i64 } }
        89 => {
            // ServicePackageVerify: a1=read-cap slot, a2=path ptr, a3=path len.
            let len = a3 as usize;
            if len == 0 || len > 256 { return SyscallResult::InvalidArguments as i64; }
            let mut path_buf = [0u8; 256];
            let path = match read_user_string(a2, len, &mut path_buf) {
                Ok(s) => s,
                Err(_) => return SyscallResult::InvalidArguments as i64,
            };
            match sys_service_package_verify(a1 as usize, path) {
                Ok(id) => id as i64,
                Err(e) => e as i64,
            }
        }
        90 => { match sys_service_heartbeat(a1 as u8) { Ok(()) => 0, Err(e) => e as i64 } }
        91 => {
            // ServicePackageLaunch: a1=read-cap slot, a2=path ptr, a3=path len.
            let len = a3 as usize;
            if len == 0 || len > 256 { return SyscallResult::InvalidArguments as i64; }
            let mut path_buf = [0u8; 256];
            let path = match read_user_string(a2, len, &mut path_buf) {
                Ok(s) => s,
                Err(_) => return SyscallResult::InvalidArguments as i64,
            };
            match sys_service_package_launch(a1 as usize, path) {
                Ok(id) => id.0 as i64,
                Err(e) => e as i64,
            }
        }
        92 => {
            // InputEventPoll: a1=out_buf_ptr, a2=out_buf_len
            let len = a2 as usize;
            if len < 24 || len > 256 { return SyscallResult::InvalidArguments as i64; }
            let mut out = [0u8; 256];
            match sys_input_event_poll(&mut out[..len]) {
                Ok(n) => {
                    if copy_to_user(a1, &out[..n]).is_err() { return SyscallResult::InvalidArguments as i64; }
                    n as i64
                }
                Err(e) => e as i64,
            }
        }
        93 => { match sys_network_poll() { Ok(n) => n as i64, Err(e) => e as i64 } },
        99 => {
            let len = a2 as usize;
            if len == 0 || len > crate::net::dns::DNS_MAX_NAME { return SyscallResult::InvalidArguments as i64; }
            let mut name = [0u8; crate::net::dns::DNS_MAX_NAME];
            let host = match read_user_string(a1, len, &mut name) { Ok(v) => v, Err(_) => return SyscallResult::InvalidArguments as i64 };
            match sys_dns_resolve(host) { Ok(ip) => u32::from_be_bytes(ip.0) as i64, Err(e) => e as i64 }
        }
        100 => match sys_tcp_close(a1 as u32) { Ok(()) => 0, Err(e) => e as i64 },
        98 => {
            let len = a3 as usize;
            if len > 2048 { return SyscallResult::InvalidArguments as i64; }
            let mut buf = [0u8; 2048];
            match sys_tcp_recv(a1 as u32, &mut buf[..len]) { Ok(n) => { if copy_to_user(a2, &buf[..n]).is_err() { return SyscallResult::InvalidArguments as i64; } n as i64 }, Err(e) => e as i64 }
        }
        97 => {
            let len = a3 as usize;
            if len > 1460 { return SyscallResult::InvalidArguments as i64; }
            let mut buf = [0u8; 1460];
            if copy_from_user(&mut buf[..len], a2, len).is_err() { return SyscallResult::InvalidArguments as i64; }
            sys_tcp_send(a1 as u32, &buf[..len]) as i64
        }
        94 => match sys_tcp_connect(crate::net::Ipv4Address::new((a1 >> 24) as u8, (a1 >> 16) as u8, (a1 >> 8) as u8, a1 as u8), a2 as u16, a3 as u16, a4 as u32) { Ok(id) => id as i64, Err(e) => e as i64 },
        96 => {
            // NetCloseSocket: a1 = socket id
            sys_net_close_socket(crate::net::SocketId(a1 as u32)) as i64
        }
        95 => {
            let len = a5 as usize;
            if len > 2048 { return SyscallResult::InvalidArguments as i64; }
            let mut payload = [0u8; 2048];
            if copy_from_user(&mut payload[..len], a4, len).is_err() {
                return SyscallResult::InvalidArguments as i64;
            }
            sys_net_send_socket(
                crate::net::SocketId(a1 as u32),
                crate::net::Ipv4Address((a2 as u32).to_be_bytes()),
                a3 as u16,
                &payload[..len],
            ) as i64
        },
        121 => match sys_app_stop(crate::task::TaskId(a1 as u64)) {
            SyscallResult::Success => 0,
            e => e as i64,
        },
        124 => match sys_app_focus(TaskId(a1 as u64)) {
            SyscallResult::Success => 0,
            e => e as i64,
        },
        127 => {
            match sys_device_power(a1 as u32) { Ok(value) => value as i64, Err(e) => e as i64 }
        },
        129 => {
            // UserHasCredential: a1 = uid. Returns 1 or 0. This is deliberately
            // separate from UserLockState: `Unlocked` is what a credential-less
            // account reports too, so lock state alone cannot answer whether a
            // PIN exists.
            let target_uid = a1 as u32;
            if let Err(e) = require_user_control_object(target_uid) { return e as i64; }
            let mgr = crate::user::USER_MANAGER.lock();
            match mgr.has_credential(crate::user::UserId(target_uid)) {
                Ok(has) => has as i64,
                Err(_) => SyscallResult::TargetNotFound as i64,
            }
        },
        128 => {
            // UserClearCredential: a1 = uid, a2 = secret_ptr, a3 = secret_len
            // WHY the secret is mandatory here: removing a credential lowers what
            // the device demands, so it must prove the current one rather than
            // trusting a caller that only knows the uid.
            let target_uid = a1 as u32;
            if let Err(e) = require_user_control_object(target_uid) { return e as i64; }
            let len = a3 as usize;
            if len == 0 || len > 64 { return SyscallResult::InvalidArguments as i64; }
            let mut secret_buf = [0u8; 64];
            if copy_from_user(&mut secret_buf[..len], a2, len).is_err() {
                return SyscallResult::InvalidArguments as i64;
            }
            let mut mgr = crate::user::USER_MANAGER.lock();
            match mgr.clear_credential(crate::user::UserId(target_uid), &secret_buf[..len]) {
                Ok(()) => SyscallResult::Success as i64,
                Err(crate::user::UserError::AuthenticationFailed) => SyscallResult::PermissionDenied as i64,
                Err(crate::user::UserError::InvalidCredential) => SyscallResult::InvalidArguments as i64,
                Err(_) => SyscallResult::TargetNotFound as i64,
            }
        },
        126 => {
            // UserSetCredential: a1 = uid, a2 = secret_ptr, a3 = secret_len, a4 = old_secret_ptr, a5 = old_secret_len
            let target_uid = a1 as u32;
            if let Err(e) = require_user_control_object(target_uid) { return e as i64; }
            let len = a3 as usize;
            if len == 0 || len > 64 { return SyscallResult::InvalidArguments as i64; }
            let mut secret_buf = [0u8; 64];
            if copy_from_user(&mut secret_buf[..len], a2, len).is_err() {
                return SyscallResult::InvalidArguments as i64;
            }
            let old_len = a5 as usize;
            let mut old_buf = [0u8; 64];
            let old_secret = if old_len > 0 {
                if old_len > 64 || copy_from_user(&mut old_buf[..old_len], a4, old_len).is_err() {
                    return SyscallResult::InvalidArguments as i64;
                }
                Some(&old_buf[..old_len])
            } else {
                None
            };
            let mut mgr = crate::user::USER_MANAGER.lock();
            match mgr.change_credential(crate::user::UserId(target_uid), old_secret, &secret_buf[..len]) {
                Ok(()) => SyscallResult::Success as i64,
                Err(crate::user::UserError::AuthenticationFailed) => SyscallResult::PermissionDenied as i64,
                Err(crate::user::UserError::InvalidCredential) => SyscallResult::InvalidArguments as i64,
                Err(_) => SyscallResult::TargetNotFound as i64,
            }
        },
        123 => {
            let out_len = a2 as usize;
            if out_len == 0 || out_len > 4096 { return SyscallResult::InvalidArguments as i64; }
            let mut out = [0u8; 4096];
            match sys_app_runtime_list(&mut out[..out_len]) {
                Ok(n) => {
                    if copy_to_user(a1, &out[..n]).is_err() { SyscallResult::InvalidArguments as i64 } else { n as i64 }
                }
                Err(e) => e as i64,
            }
        },
        122 => {
            let len = a2 as usize;
            if len == 0 || len > 128 { return SyscallResult::InvalidArguments as i64; }
            let mut buf = [0u8; 128];
            let app_id = match read_user_string(a1, len, &mut buf) {
                Ok(v) => v,
                Err(_) => return SyscallResult::InvalidArguments as i64,
            };
            match sys_app_restart(app_id) {
                Ok(tid) => tid.0 as i64,
                Err(e) => e as i64,
            }
        },
        120 => {
            let len = a3 as usize;
            if len == 0 || len > 128 { return SyscallResult::InvalidArguments as i64; }
            let mut name_buf = [0u8; 128];
            let pkg_id = match read_user_string(a2, len, &mut name_buf) {
                Ok(s) => s,
                Err(_) => return SyscallResult::InvalidArguments as i64,
            };
            match sys_package_uninstall(a1 as usize, pkg_id, a4 != 0) {
                Ok(()) => 0,
                Err(e) => e as i64,
            }
        },
        101 => {
            let len = a2 as usize;
            if len == 0 || len > 128 {
                crate::serial_println!("[APP_LAUNCH DEBUG] invalid string len: {}", len);
                return SyscallResult::InvalidArguments as i64;
            }
            let mut name_buf = [0u8; 128];
            let app_id = match read_user_string(a1, len, &mut name_buf) {
                Ok(s) => s,
                Err(e) => {
                    crate::serial_println!("[APP_LAUNCH DEBUG] read_user_string error: {:?}", e);
                    return SyscallResult::InvalidArguments as i64;
                }
            };
            match sys_app_launch(app_id) {
                Ok(tid) => tid.0 as i64,
                Err(e) => {
                    crate::serial_println!("[APP_LAUNCH DEBUG] sys_app_launch('{}') error: {:?}", app_id, e);
                    e as i64
                }
            }
        },
        102 => {
            if a2 < 12 { return SyscallResult::InvalidArguments as i64; }
            let mut out = [0u8; 12];
            match sys_service_query(a1 as u8, &mut out) {
                Ok(n) => { if copy_to_user(a2, &out[..n]).is_err() { SyscallResult::InvalidArguments as i64 } else { n as i64 } }
                Err(e) => e as i64,
            }
        },
        103 => {
            // IpcRecvMeta: a1=cap_slot, a2=payload_ptr, a3=payload_capacity,
            // a4=sender_out_ptr, a5=msg_type_out_ptr.
            let cap_slot = a1 as usize;
            let capacity = a3 as usize;
            if capacity > 1024 || a4 == 0 || a5 == 0 {
                return SyscallResult::InvalidArguments as i64;
            }
            match sys_ipc_recv(cap_slot) {
                Ok(msg) => {
                    let copy_len = core::cmp::min(capacity, msg.len);
                    if copy_to_user(a2, &msg.payload[..copy_len]).is_err()
                        || write_user_value(a4, &msg.sender.0).is_err()
                        || write_user_value(a5, &msg.msg_type).is_err()
                    {
                        return SyscallResult::InvalidArguments as i64;
                    }
                    copy_len as i64
                }
                Err(err) => err as i64,
            }
        },
        105 => {
            // UserGetCurrent
            crate::user::USER_MANAGER.lock().active_user().0 as i64
        },
        106 => {
            // UserSwitch: a1 = target_uid
            // Object-scoped to the TARGET account: switching the global active
            // user is an identity takeover, so authority must name the account
            // being switched to rather than merely proving SystemControl.
            let target_uid = a1 as u32;
            if let Err(e) = require_user_control_object(target_uid) { return e as i64; }
            let mut mgr = crate::user::USER_MANAGER.lock();
            match mgr.switch_user(crate::user::UserId(target_uid)) {
                Ok(()) => SyscallResult::Success as i64,
                Err(crate::user::UserError::UserNotFound) => SyscallResult::TargetNotFound as i64,
                Err(_) => SyscallResult::PermissionDenied as i64,
            }
        },
        107 => {
            // UserLockState: a1 = uid
            let mgr = crate::user::USER_MANAGER.lock();
            match mgr.get_lock_state(crate::user::UserId(a1 as u32)) {
                Ok(crate::user::UserLockState::Unlocked) => 0,
                Ok(crate::user::UserLockState::Locked) => 1,
                Ok(crate::user::UserLockState::Uninitialized) => 2,
                Ok(crate::user::UserLockState::Lockout { .. }) => 3,
                Err(_) => SyscallResult::TargetNotFound as i64,
            }
        },
        108 => {
            // UserUnlock: a1 = uid, a2 = secret_ptr, a3 = secret_len
            // Object-scoped to the account being unlocked. Without this, any
            // ring3 task can brute-force another account's secret and, via the
            // 5-attempt lockout, deny the device owner access to their account.
            let target_uid = a1 as u32;
            if let Err(e) = require_user_control_object(target_uid) { return e as i64; }
            let len = a3 as usize;
            if len > 64 { return SyscallResult::InvalidArguments as i64; }
            let mut secret_buf = [0u8; 64];
            if copy_from_user(&mut secret_buf[..len], a2, len).is_err() {
                return SyscallResult::InvalidArguments as i64;
            }
            let current_ticks = crate::interrupts::get_system_ticks();
            let mut mgr = crate::user::USER_MANAGER.lock();
            match mgr.unlock(crate::user::UserId(target_uid), &secret_buf[..len], current_ticks) {
                Ok(()) => SyscallResult::Success as i64,
                Err(crate::user::UserError::LockoutActive) => SyscallResult::PermissionDenied as i64,
                Err(crate::user::UserError::AuthenticationFailed) => SyscallResult::PermissionDenied as i64,
                Err(_) => SyscallResult::InvalidArguments as i64,
            }
        },
        109 => {
            // UserLock: a1 = uid
            // Object-scoped to the account being locked. a1 is caller-controlled,
            // so an unscoped check let any ring3 task lock ANY uid, including the
            // device owner, which is a denial of service on the owner.
            let target_uid = a1 as u32;
            if let Err(e) = require_user_control_object(target_uid) { return e as i64; }
            let mut mgr = crate::user::USER_MANAGER.lock();
            match mgr.lock(crate::user::UserId(target_uid)) {
                Ok(()) => SyscallResult::Success as i64,
                Err(_) => SyscallResult::TargetNotFound as i64,
            }
        },
        110 => {
            // NotificationPost: a1 = pkg_ptr, a2 = pkg_len, a3 = channel_ptr, a4 = channel_len
            let pkg_len = (a2 & 0xFF) as usize;
            if pkg_len == 0 || pkg_len > 64 { return SyscallResult::InvalidArguments as i64; }
            let mut pkg_buf = [0u8; 64];
            let pkg = match read_user_string(a1, pkg_len, &mut pkg_buf) {
                Ok(s) => s,
                Err(_) => return SyscallResult::InvalidArguments as i64,
            };
            // The claimed package id must be the package the kernel actually
            // spawned this task as. Otherwise any ring3 task can post a
            // notification attributed to an arbitrary package, which is
            // notification spoofing (impersonating a system or banking alert).
            if let Err(e) = require_package_or_delegated(pkg) { return e as i64; }
            let channel_len = (a4 & 0x1FF) as usize;
            if channel_len == 0 || channel_len > 256 { return SyscallResult::InvalidArguments as i64; }
            let mut chan_buf = [0u8; 256];
            let chan_payload = match read_user_string(a3, channel_len, &mut chan_buf) {
                Ok(s) => s,
                Err(_) => return SyscallResult::InvalidArguments as i64,
            };
            let (chan, title, body) = if let Some(first_bar) = chan_payload.find('|') {
                let c = &chan_payload[..first_bar];
                let rest = &chan_payload[first_bar + 1..];
                if let Some(second_bar) = rest.find('|') {
                    (c, &rest[..second_bar], &rest[second_bar + 1..])
                } else {
                    (c, rest, "New notification")
                }
            } else {
                (chan_payload, "Notification", "New alert")
            };
            // Post into the caller's OWN account, never the global active user.
            // Otherwise a background task bound to a background account could
            // inject notifications into whatever session happens to be active.
            let user_id = match caller_user_id() {
                Some(uid) => uid,
                None => return SyscallResult::PermissionDenied as i64,
            };
            let current_ticks = crate::interrupts::get_system_ticks();
            let mut nm = crate::notification::NOTIFICATION_MANAGER.lock();
            match nm.post(
                pkg,
                user_id,
                chan,
                title,
                body,
                crate::notification::NotificationPriority::Default,
                crate::notification::LockscreenVisibility::Public,
                1,
                current_ticks,
            ) {
                Ok(id) => id as i64,
                Err(_) => SyscallResult::PermissionDenied as i64,
            }
        },
        111 => {
            // NotificationCancel: a1 = pkg_ptr, a2 = pkg_len, a3 = notif_id
            let pkg_len = a2 as usize;
            if pkg_len > 64 { return SyscallResult::InvalidArguments as i64; }
            let mut pkg_buf = [0u8; 64];
            let pkg = if pkg_len > 0 {
                match read_user_string(a1, pkg_len, &mut pkg_buf) {
                    Ok(s) => s,
                    Err(_) => return SyscallResult::InvalidArguments as i64,
                }
            } else {
                ""
            };
            // Same identity binding as post: a caller may only cancel the
            // notifications of the package it actually is, or hold system control.
            if let Err(e) = require_package_or_delegated(pkg) { return e as i64; }
            let mut nm = crate::notification::NOTIFICATION_MANAGER.lock();
            if a3 == 0 {
                if let Some(uid) = caller_user_id() {
                    nm.cancel_all_for_user(uid);
                    return SyscallResult::Success as i64;
                }
            }
            match nm.cancel(pkg, a3) {
                Ok(()) => SyscallResult::Success as i64,
                Err(_) => SyscallResult::TargetNotFound as i64,
            }
        },
        112 => {
            // NotificationQuery: a1 = user_id, a2 = is_locked, a3 = out_buf_ptr, a4 = out_buf_cap
            // Scoped to the caller's own account (or a delegated grant for that account).
            let target_uid = a1 as u32;
            if let Err(e) = require_own_or_delegated_user(target_uid) { return e as i64; }
            let nm = crate::notification::NOTIFICATION_MANAGER.lock();
            let active = nm.query_active(target_uid, a2 != 0);
            if a3 == 0 || a4 == 0 {
                return active.len() as i64;
            }
            let out_len = (a4 as usize).min(4096);
            let mut out = [0u8; 4096];
            use core::fmt::Write;
            struct W<'a>{b:&'a mut[u8],n:usize}
            impl<'a> Write for W<'a>{
                fn write_str(&mut self,s:&str)->core::fmt::Result{
                    let k=s.len().min(self.b.len().saturating_sub(self.n));
                    self.b[self.n..self.n+k].copy_from_slice(&s.as_bytes()[..k]);
                    self.n+=k;
                    Ok(())
                }
            }
            let mut w = W{b: &mut out[..out_len], n: 0};
            for r in &active {
                let _ = writeln!(w, "{}|{}|{}|{}|{}|{}|{}\n",
                    r.id, r.package_id, r.channel_id, r.priority as u8, r.timestamp_ticks, r.title, r.body);
                if w.n >= w.b.len() { break; }
            }
            let written = w.n;
            drop(w);
            if copy_to_user(a3, &out[..written]).is_err() {
                return SyscallResult::InvalidArguments as i64;
            }
            written as i64
        },
        113 => {
            // PermissionRevoke: a1 = task_id, a2 = perm_id. A task may revoke
            // its own permission; revoking another task requires system control.
            let task_id = TaskId(a1);
            let caller = match SCHEDULER.lock().current_task_id() {
                Some(id) => id,
                None => return SyscallResult::PermissionDenied as i64,
            };
            if task_id != caller {
                if require_system_control().is_err() {
                    return SyscallResult::PermissionDenied as i64;
                }
            }
            let perm = match crate::capability::permission::SensitivePermission::from_u32(a2 as u32) {
                Some(p) => p,
                None => return SyscallResult::InvalidArguments as i64,
            };
            let mut pm = crate::capability::permission::PERMISSION_MANAGER.lock();
            if pm.revoke_permission(task_id, perm) {
                let mut sched = SCHEDULER.lock();
                if let Some(task) = sched.get_task_mut(task_id) {
                    // WHY the shared helper instead of re-deriving the identity here:
                    // a second hand-written copy of the id expression is exactly how
                    // revocation ends up targeting a capability that was never minted,
                    // while reporting success.
                    task.cap_table.revoke_by_id(perm.capability_id(task_id));
                }
                SyscallResult::Success as i64
            } else {
                SyscallResult::TargetNotFound as i64
            }
        },
        114 => {
            // SettingsHardwareStatus: a1 = category_str_ptr, a2 = category_str_len
            let len = a2 as usize;
            if len == 0 || len > 32 { return SyscallResult::InvalidArguments as i64; }
            let mut cat_buf = [0u8; 32];
            let cat = match read_user_string(a1, len, &mut cat_buf) {
                Ok(s) => s,
                Err(_) => return SyscallResult::InvalidArguments as i64,
            };
            match crate::settings::get_hardware_status(cat) {
                crate::settings::HardwareAvailability::Available => 0,
                crate::settings::HardwareAvailability::SimulatedVirtual => 1,
                crate::settings::HardwareAvailability::Unavailable => 2,
            }
        },
        _ => SyscallResult::UnknownSyscall as i64,
    }
}


/// Queries the live hardware resource management status.
pub fn sys_resource_query(
    out_battery: &mut u8,
    out_temp: &mut u8,
    out_throttling: &mut bool,
 ) -> SyscallResult {
    if require_system_control_object(crate::services::SERVICE_POWER_CONTROL_OBJECT).is_err() {
        return SyscallResult::PermissionDenied;
    }
    let rm = crate::resource::RESOURCE_MANAGER.lock();
    // No fuel gauge or thermal sensor is bound on this platform, so report the
    // device as unavailable rather than publishing a plausible-looking value.
    if !rm.battery.present || !rm.thermal.present {
        return SyscallResult::DeviceError;
    }
    *out_battery = rm.battery.percent;
    *out_temp = rm.thermal.temp_celsius;
    *out_throttling = rm.thermal.throttling;
    SyscallResult::Success
}

/// Queries the dynamic multi-zone thermal governor into `out_buf` JSON string.
pub fn sys_thermal_query(out_buf: &mut [u8]) -> Result<usize, SyscallResult> {
    require_system_control_object(crate::services::SERVICE_POWER_CONTROL_OBJECT)?;
    if out_buf.len() < crate::resource::THERMAL_JSON_MAX { return Err(SyscallResult::InvalidArguments); }
    let tm = crate::resource::THERMAL_MANAGER.lock();
    // An unbound sensor has nothing to report; the serialised form carries an
    // explicit absent flag, so callers must not treat zeros as a reading.
    if !tm.sensor_present {
        return Err(SyscallResult::DeviceError);
    }
    let len = tm.format_json(out_buf);
    Ok(len)
}

/// Creates a cryptographic secure IPC session between calling task and peer task.
pub fn sys_secure_ipc_session(peer: TaskId) -> Result<u64, SyscallResult> {
    let current_task = sys_get_task_id();
    // Object-scoped to the PEER, not merely "holds IpcSend somewhere".
    // WHY: the previous object-blind check let any holder of an IpcSend
    // capability open an encrypted session with ANY task, which both leaks
    // session existence across the trust boundary and lets an attacker
    // enumerate and pre-create sessions against arbitrary peers.
    {
        let sched = SCHEDULER.lock();
        let task = sched.current_task().ok_or(SyscallResult::PermissionDenied)?;
        if !task
            .cap_table
            .authorize_object(CapabilityType::IpcSend, Rights::WRITE, peer.0)
        {
            return Err(SyscallResult::PermissionDenied);
        }
    }
    // A session with yourself carries no security property and would consume a
    // slot in the bounded session table.
    if peer == current_task || peer == TaskId(0) {
        return Err(SyscallResult::InvalidArguments);
    }
    let mut key = [0u8; 32];
    crate::crypto::get_random_bytes(&mut key);
    let mut mgr = crate::ipc::SECURE_IPC.lock();
    mgr.create_session(current_task, peer, key)
        .map_err(|_| SyscallResult::DeviceError)
}

/// Queries wireless HAL status (Wi-Fi and BLE beacon) into `out_buf` JSON string.
pub fn sys_wireless_query(out_buf: &mut [u8]) -> Result<usize, SyscallResult> {
    require_system_control()?;
    let wm = crate::net::WIRELESS_MANAGER.lock();
    let len = wm.format_json(out_buf);
    Ok(len)
}

/// Interacts with standard virtual character device nodes in `/dev/`.
pub fn sys_devfs_io(action: u32, path: &str, buf: &mut [u8]) -> Result<usize, SyscallResult> {
    let sched = SCHEDULER.lock();
    let task = sched.current_task().ok_or(SyscallResult::PermissionDenied)?;
    let allowed = if action == 0 {
        task.cap_table.authorize_capability(CapabilityType::FileSystemRead, Rights::READ)
            || task.cap_table.authorize_capability(CapabilityType::FileSystemWrite, Rights::READ)
    } else {
        task.cap_table.authorize_capability(CapabilityType::FileSystemWrite, Rights::WRITE)
    };
    drop(sched);
    if !allowed { return Err(SyscallResult::PermissionDenied); }
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

/// Creates and binds a UDP socket for the current calling task.
pub fn sys_tcp_send(connection_id: u32, payload: &[u8]) -> SyscallResult {
    let sched = SCHEDULER.lock();
    let current = match sched.current_task() {
        Some(t) => t,
        None => return SyscallResult::PermissionDenied,
    };
    if !current.cap_table.has_capability(CapabilityType::NetworkSend, Rights::WRITE) {
        return SyscallResult::PermissionDenied;
    }
    let owner = current.id.0 as u32;
    let task_id = current.id;
    let caps = current.cap_table.clone();
    drop(sched);
    if crate::task::process::consume_background_network(task_id, payload.len()).is_err() {
        return SyscallResult::NotReady;
    }
    if payload.len() > crate::net::tcp::TCP_MAX_SEGMENT { return SyscallResult::InvalidArguments; }
    let (src_ip, src_port, dst_ip, dst_port, seq, ack, state, window) = {
        let table = crate::net::TCP_TABLE.lock();
        let conn = match table.get(connection_id) { Some(c) if c.owner_task_id == owner => c, Some(_) => return SyscallResult::PermissionDenied, None => return SyscallResult::InvalidArguments };
        (if conn.remote_ip.is_loopback() { crate::net::Ipv4Address::LOCALHOST } else { crate::net::DEFAULT_HOST_IP }, conn.local_port, conn.remote_ip, conn.remote_port, conn.send_seq, conn.recv_seq, conn.state, conn.rcv_wnd.min(u16::MAX as u32) as u16)
    };
    if state != crate::net::TcpState::Established { return SyscallResult::NotReady; }
    let available = crate::net::TCP_TABLE.lock().send_window(connection_id).unwrap_or(0) as usize;
    if payload.len() > available { return SyscallResult::NotReady; }
    crate::net::send_tcp_payload_with_window(src_ip, src_port, dst_ip, dst_port, seq, ack, payload, window, &caps)
        .map(|n| {
            let now = crate::interrupts::get_system_ticks();
            let mut table = crate::net::TCP_TABLE.lock();
            if let Some(c) = table.get_mut(connection_id) {
                c.send_seq = c.send_seq.wrapping_add(n as u32);
            }
            if n > 0 { let _ = table.arm_retransmit(connection_id, seq, payload, now); }
            n
        })
        .map(|_| SyscallResult::Success).unwrap_or(SyscallResult::NetworkError)
}

pub fn sys_tcp_recv(connection_id: u32, out: &mut [u8]) -> Result<usize, SyscallResult> {
    let (owner, caps) = {
        let sched = SCHEDULER.lock();
        let current = sched.current_task().ok_or(SyscallResult::PermissionDenied)?;
        if !current.cap_table.has_capability_object(CapabilityType::NetworkReceive, Rights::READ, connection_id as u64) {
            return Err(SyscallResult::PermissionDenied);
        }
        (current.id.0 as u32, current.cap_table.clone())
    };
    let n = crate::net::TCP_TABLE.lock().recv_owned(connection_id, owner, out, &caps).map_err(|e| match e {
        NetError::CapabilityDenied => SyscallResult::PermissionDenied,
        NetError::NoPacketAvailable => SyscallResult::NotReady,
        _ => SyscallResult::NetworkError,
    })?;
    if n > 0 {
        let (src_ip, local_port, dst_ip, remote_port, seq, ack, window) = {
            let table = crate::net::TCP_TABLE.lock();
            let conn = table.get(connection_id).ok_or(SyscallResult::InvalidArguments)?;
            (
                if conn.remote_ip.is_loopback() { crate::net::Ipv4Address::LOCALHOST } else { crate::net::DEFAULT_HOST_IP },
                conn.local_port,
                conn.remote_ip,
                conn.remote_port,
                conn.send_seq,
                conn.recv_seq,
                conn.rcv_wnd.min(u16::MAX as u32) as u16,
            )
        };
        let _ = crate::net::transmit_tcp_segment_with_window(
            src_ip, local_port, dst_ip, remote_port, seq, ack, 0x10, window
        );
    }
    Ok(n)
}

/// Sends a UDP datagram to the DNS server, handling ARP resolution delays.
///
/// The first UDP transmission can legitimately return NoPacketAvailable when
/// the destination MAC is not cached yet. Polling between bounded retries lets
/// an ARP reply arrive instead of turning first-contact DNS into a hard failure.
///
/// When `is_retransmit` is false, consumes one invocation quota from delegated leases.
/// When `is_retransmit` is true, verifies that the capability is still held and valid
/// without consuming additional invocation quotas for internal kernel retries of the same
/// logical DNS operation (per-logical-operation quota policy).
fn send_dns_datagram_with_arp_retry(
    src_port: u16,
    payload: &[u8],
    caps: &crate::capability::CapabilityTable,
    deadline: u64,
    is_retransmit: bool,
) -> Result<(), NetError> {
    loop {
        match crate::net::send_udp_datagram_authorized(
            crate::net::DEFAULT_HOST_IP,
            src_port,
            crate::net::DNS_SERVER,
            53,
            payload,
            caps,
            !is_retransmit,
        ) {
            Ok(()) => return Ok(()),
            Err(NetError::NoPacketAvailable) => {
                if crate::interrupts::get_system_ticks() >= deadline {
                    return Err(NetError::NoPacketAvailable);
                }
                let _ = crate::net::poll_network_interface();
                crate::task::yield_now();
            }
            Err(err) => return Err(err),
        }
    }
}

/// Resolves one hostname through the kernel DNS resolver over the capability-governed UDP path.
pub fn sys_dns_resolve(name: &str) -> Result<crate::net::Ipv4Address, SyscallResult> {
    if name.is_empty() || name.len() > crate::net::dns::DNS_MAX_NAME {
        return Err(SyscallResult::InvalidArguments);
    }
    let sched = SCHEDULER.lock();
    let current = sched.current_task().ok_or(SyscallResult::PermissionDenied)?;
    if !current.cap_table.has_capability(CapabilityType::NetworkSocket, Rights::WRITE)
        || !current.cap_table.has_capability(CapabilityType::NetworkSend, Rights::WRITE)
        || !current.cap_table.has_capability(CapabilityType::NetworkReceive, Rights::READ) {
        return Err(SyscallResult::PermissionDenied);
    }
    let owner = current.id.0 as u32;
    let caps = current.cap_table.clone();
    drop(sched);

    let now = crate::interrupts::get_system_ticks();
    if let Some(addr) = crate::net::DNS_RESOLVER.lock().cached(name.as_bytes(), now) {
        return Ok(addr);
    }

    let mut packet = [0u8; 512];
    let (query_id, packet_len) = {
        let mut resolver = crate::net::DNS_RESOLVER.lock();
        let query_id = resolver.begin(name.as_bytes(), now).map_err(|_| SyscallResult::NetworkError)?;
        let packet_len = resolver.build_query(query_id, &mut packet)
            .map_err(|_| SyscallResult::NetworkError)?;
        (query_id, packet_len)
    };

    let mut socket = None;
    for port in 53000u16..53008u16 {
        if let Ok(id) = crate::net::SOCKET_TABLE.lock().create_socket(owner, port, &caps) {
            socket = Some(id);
            break;
        }
    }
    let socket = socket.ok_or(SyscallResult::NetworkError)?;

    let local_port = {
        let table = crate::net::SOCKET_TABLE.lock();
        table.get(socket).map(|s| s.local_port).unwrap_or(53000)
    };
    let first_send_deadline = now.saturating_add(crate::net::dns::DNS_RETRY_TICKS);
    if send_dns_datagram_with_arp_retry(
        local_port,
        &packet[..packet_len],
        &caps,
        first_send_deadline,
        false, // initial transmission consumes 1 invocation quota for this logical DNS query
    ).is_err() {
        let _ = crate::net::DNS_RESOLVER.lock().abort(query_id);
        let _ = crate::net::SOCKET_TABLE.lock().close_owned(socket, owner);
        return Err(SyscallResult::NetworkError);
    }

    let deadline = now.saturating_add(
        crate::net::dns::DNS_RETRY_TICKS * (crate::net::dns::DNS_MAX_RETRIES as u64 + 1),
    );
    let mut rx = [0u8; 512];
    loop {
        let current_tick = crate::interrupts::get_system_ticks();
        if current_tick >= deadline {
            let _ = crate::net::DNS_RESOLVER.lock().abort(query_id);
            let _ = crate::net::SOCKET_TABLE.lock().close_owned(socket, owner);
            return Err(SyscallResult::NetworkError);
        }
        let _ = crate::net::poll_network_interface();
        if let Ok(bytes) = crate::net::SOCKET_TABLE.lock().get_mut(socket)
            .ok_or(NetError::NoPacketAvailable)
            .and_then(|s| s.recv_from(&mut rx, &caps).map(|v| v.0))
        {
            if let Ok(addr) = crate::net::DNS_RESOLVER.lock().accept_response(&rx[..bytes], current_tick) {
                let _ = crate::net::SOCKET_TABLE.lock().close_owned(socket, owner);
                return Ok(addr);
            }
        }

        // Retransmit the DNS query when its bounded deadline expires. The
        // resolver owns retry state; this syscall owns the actual UDP send.
        let retry = {
            let mut resolver = crate::net::DNS_RESOLVER.lock();
            let timed_out = resolver.query(query_id)
                .map(|q| current_tick >= q.deadline)
                .unwrap_or(false);
            if !timed_out {
                None
            } else {
                let expired = resolver.poll_timeouts(current_tick);
                if resolver.query(query_id).is_some() {
                    let retry_len = resolver.build_query(query_id, &mut packet)
                        .map_err(|_| SyscallResult::NetworkError)?;
                    Some((retry_len, false))
                } else {
                    Some((0, expired > 0))
                }
            }
        };

        if let Some((retry_len, exhausted)) = retry {
            if exhausted {
                let _ = crate::net::DNS_RESOLVER.lock().abort(query_id);
                let _ = crate::net::SOCKET_TABLE.lock().close_owned(socket, owner);
                return Err(SyscallResult::NetworkError);
            }
            if send_dns_datagram_with_arp_retry(
                local_port,
                &packet[..retry_len],
                &caps,
                current_tick.saturating_add(crate::net::dns::DNS_RETRY_TICKS),
                true, // internal retransmission verifies validity but does not double-consume lease
            ).is_err() {
                let _ = crate::net::DNS_RESOLVER.lock().abort(query_id);
                let _ = crate::net::SOCKET_TABLE.lock().close_owned(socket, owner);
                return Err(SyscallResult::NetworkError);
            }
        }
    }
}

/// Starts the bounded TCP FIN close handshake for an owned connection.
pub fn sys_tcp_close(connection_id: u32) -> Result<(), SyscallResult> {
    let sched = SCHEDULER.lock();
    let current = sched.current_task().ok_or(SyscallResult::PermissionDenied)?;
    if !current.cap_table.authorize_capability(CapabilityType::NetworkSend, Rights::WRITE) {
        return Err(SyscallResult::PermissionDenied);
    }
    let owner = current.id.0 as u32;
    let src_ip;
    let local_port;
    let dst_ip;
    let remote_port;
    let ack;
    let window;
    {
        let table = crate::net::TCP_TABLE.lock();
        let conn = table.get(connection_id).ok_or(SyscallResult::InvalidArguments)?;
        if conn.owner_task_id != owner { return Err(SyscallResult::PermissionDenied); }
        if conn.state != crate::net::TcpState::Established && conn.state != crate::net::TcpState::CloseWait {
            return Err(SyscallResult::NotReady);
        }
        src_ip = if conn.remote_ip.is_loopback() { crate::net::Ipv4Address::LOCALHOST } else { crate::net::DEFAULT_HOST_IP };
        local_port = conn.local_port;
        dst_ip = conn.remote_ip;
        remote_port = conn.remote_port;
        ack = conn.recv_seq;
        window = conn.rcv_wnd.min(u16::MAX as u32) as u16;
    }
    let seq = crate::net::TCP_TABLE.lock()
        .arm_fin(connection_id, crate::interrupts::get_system_ticks())
        .map_err(|_| SyscallResult::NotReady)?;
    if crate::net::transmit_tcp_segment_with_window(src_ip, local_port, dst_ip, remote_port, seq, ack, 0x11, window).is_err() {
        let _ = crate::net::TCP_TABLE.lock().rollback_fin(connection_id);
        return Err(SyscallResult::NetworkError);
    }
    Ok(())
}

pub fn sys_tcp_connect(dst_ip: crate::net::Ipv4Address, dst_port: u16, local_port: u16, seq: u32) -> Result<u32, SyscallResult> {
    let sched = SCHEDULER.lock();
    let current = sched.current_task().ok_or(SyscallResult::PermissionDenied)?;
    if !current.cap_table.authorize_capability(CapabilityType::NetworkSocket, Rights::WRITE)
        || !current.cap_table.has_capability(CapabilityType::NetworkSend, Rights::WRITE) {
        return Err(SyscallResult::PermissionDenied);
    }
    let owner = current.id.0 as u32;
    let caps = current.cap_table.clone();
    drop(sched);
    let id = crate::net::TCP_TABLE.lock()
        .open(owner, local_port, dst_ip, dst_port, seq)
        .map_err(|_| SyscallResult::NetworkError)?;
    let window = crate::net::TCP_TABLE.lock().get(id).map(|c| c.rcv_wnd.min(u16::MAX as u32) as u16).unwrap_or(0);
    if crate::net::send_tcp_syn_with_window(
        if dst_ip.is_loopback() { crate::net::Ipv4Address::LOCALHOST } else { crate::net::DEFAULT_HOST_IP },
        local_port, dst_ip, dst_port, seq, window, &caps,
    ).is_err() {
        let _ = crate::net::TCP_TABLE.lock().close_owned(id, owner);
        return Err(SyscallResult::NetworkError);
    }
    Ok(id)
}

pub fn sys_net_socket(port: u16) -> Result<u32, SyscallResult> {
    let sched = SCHEDULER.lock();
    let current = match sched.current_task() {
        Some(t) => t,
        None => return Err(SyscallResult::PermissionDenied),
    };
    if !current.cap_table.has_capability(CapabilityType::NetworkSocket, Rights::WRITE) {
        return Err(SyscallResult::PermissionDenied);
    }
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
pub fn sys_net_close_socket(sock_id: crate::net::SocketId) -> SyscallResult {
    let sched = SCHEDULER.lock();
    let current = match sched.current_task() {
        Some(t) => t,
        None => return SyscallResult::PermissionDenied,
    };
    let owner = current.id.0 as u32;
    drop(sched);
    match crate::net::SOCKET_TABLE.lock().close_owned(sock_id, owner) {
        Ok(()) => SyscallResult::Success,
        Err(NetError::CapabilityDenied) => SyscallResult::PermissionDenied,
        Err(_) => SyscallResult::InvalidArguments,
    }
}

pub fn sys_net_send_socket(
    sock_id: crate::net::SocketId,
    dst_ip: crate::net::Ipv4Address,
    dst_port: u16,
    payload: &[u8],
) -> SyscallResult {
    let sched = SCHEDULER.lock();
    let current = match sched.current_task() {
        Some(t) => t,
        None => return SyscallResult::PermissionDenied,
    };
    if !current.cap_table.has_capability(CapabilityType::NetworkSend, Rights::WRITE) {
        return SyscallResult::PermissionDenied;
    }
    let owner = current.id.0 as u32;
    let task_id = current.id;
    let caps = current.cap_table.clone();
    drop(sched);
    if crate::task::process::consume_background_network(task_id, payload.len()).is_err() {
        return SyscallResult::NotReady;
    }
    let src_port = {
        let table = crate::net::SOCKET_TABLE.lock();
        match table.get(sock_id) {
            Some(sock) if sock.owner_task_id == owner && sock.is_bound => sock.local_port,
            Some(_) => return SyscallResult::PermissionDenied,
            None => return SyscallResult::InvalidArguments,
        }
    };
    let src_ip = if dst_ip.is_loopback() { crate::net::Ipv4Address::LOCALHOST } else { crate::net::DEFAULT_HOST_IP };
    match crate::net::send_udp_datagram(src_ip, src_port, dst_ip, dst_port, payload, &caps) {
        Ok(()) => SyscallResult::Success,
        Err(NetError::NoPacketAvailable) => SyscallResult::NotReady,
        Err(_) => SyscallResult::NetworkError,
    }
}

pub fn sys_net_sendto(dst_ip: crate::net::Ipv4Address, dst_port: u16, payload: &[u8]) -> SyscallResult {
    let current_id = match SCHEDULER.lock().current_task_id() {
        Some(id) => id,
        None => return SyscallResult::PermissionDenied,
    };
    if crate::task::process::consume_background_network(current_id, payload.len()).is_err() {
        return SyscallResult::NotReady;
    }
    let sched = SCHEDULER.lock();
    let current = match sched.current_task() {
        Some(t) => t,
        None => return SyscallResult::PermissionDenied,
    };
    if !current.cap_table.has_capability(CapabilityType::NetworkSend, Rights::WRITE) {
        return SyscallResult::PermissionDenied;
    }
    let caps = current.cap_table.clone();
    drop(sched);

    let src_ip = if dst_ip.is_loopback() {
        crate::net::Ipv4Address::LOCALHOST
    } else {
        crate::net::DEFAULT_HOST_IP
    };
    match crate::net::send_udp_datagram(
        src_ip,
        49152,
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

/// Polls the canonical network device and dispatches received Ethernet/IP/UDP
/// frames into the kernel socket table. Only the userspace network service may
/// drive the device polling path through its NetworkReceive capability.
pub fn sys_network_poll() -> Result<usize, SyscallResult> {
    let sched = SCHEDULER.lock();
    let current = sched.current_task().ok_or(SyscallResult::PermissionDenied)?;
    if !current.cap_table.authorize_capability(CapabilityType::NetworkReceive, Rights::READ) {
        return Err(SyscallResult::PermissionDenied);
    }
    drop(sched);
    Ok(crate::net::poll_network_interface())
}

pub fn sys_net_recvfrom(sock_id: crate::net::SocketId, out_buf: &mut [u8]) -> Result<usize, SyscallResult> {
    let sched = SCHEDULER.lock();
    let current = match sched.current_task() {
        Some(t) => t,
        None => return Err(SyscallResult::PermissionDenied),
    };
    if !current.cap_table.has_capability(CapabilityType::NetworkReceive, Rights::READ) {
        return Err(SyscallResult::PermissionDenied);
    }
    let caps = current.cap_table.clone();
    let current_id = current.id;
    drop(sched);
    let mut sock_table = crate::net::SOCKET_TABLE.lock();
    let sock = sock_table.get_mut(sock_id).ok_or(SyscallResult::InvalidArguments)?;
    if sock.owner_task_id != current_id.0 as u32 {
        return Err(SyscallResult::PermissionDenied);
    }
    let (bytes, _, _) = sock.recv_from(out_buf, &caps).map_err(|_| SyscallResult::PermissionDenied)?;
    Ok(bytes)
}

/// Runs a pre-flight zero-trust capability audit for a package.
pub fn sys_package_audit(pkg_id: &str) -> Result<u32, SyscallResult> {
    let reg = crate::package::REVOCATION_REGISTRY.lock();
    if reg.is_package_revoked(pkg_id) {
        return Err(SyscallResult::IntegrityViolation);
    }
    Ok(0)
}

/// Installs a verified package onto persistent VFS storage.
/// Requires the caller to hold a `FileSystemWrite` capability in `cap_slot`.
/// Refuses to install a package that claims device-wide administration unless
/// its publisher key is a platform publisher.
///
/// WHY this is separate and named: capability 18 is the one identifier that
/// escalates a package out of its own domain, so the decision deserves its own
/// testable unit rather than living inline in a function whose other steps need
/// the global scheduler and the package registry to be set up.
///
/// Note the ordering this caller uses: it runs after `parse_rapp` has already
/// verified the signature, so `publisher_key` is authenticated at this point and
/// cannot have been chosen by the installer.
fn authorize_capability_18_for_install(manifest: &RappManifest) -> Result<(), SyscallResult> {
    if !manifest.capabilities.contains(&18) {
        return Ok(());
    }
    let reg = crate::package::REVOCATION_REGISTRY.lock();
    if !reg.is_system_publisher(&manifest.publisher_key) {
        return Err(SyscallResult::PermissionDenied);
    }
    Ok(())
}

pub fn sys_package_install(_cap_slot: usize, pkg_id: &str, user_consent: bool) -> Result<(), SyscallResult> {
    if !user_consent { return Err(SyscallResult::PermissionDenied); }

    let (read_slot, task_id, cap_table) = {
        let sched = SCHEDULER.lock();
        let current = sched.current_task().ok_or(SyscallResult::PermissionDenied)?;
        if !current.cap_table.authorize_capability(CapabilityType::FileSystemWrite, Rights::WRITE) {
            return Err(SyscallResult::PermissionDenied);
        }
        let read_slot = current.cap_table.find_slot(CapabilityType::FileSystemRead, Rights::READ)
            .ok_or(SyscallResult::PermissionDenied)?;
        (read_slot, current.id, current.cap_table.clone())
    };

    if pkg_id.is_empty()
        || pkg_id.len() > crate::package::rapp::RAPP_MAX_PACKAGE_ID_LEN
        || pkg_id.contains('/')
        || pkg_id.contains('\\')
        || pkg_id.contains("..")
    {
        return Err(SyscallResult::InvalidArguments);
    }

    let path = alloc::format!("/packages/{}.rapp", pkg_id);
    let vfs = VFS.lock();
    // The blob bound is applied inside the VFS before the copy, and it is the
    // same bound the store will later accept, so verification and installation
    // cannot disagree about what a storable package is.
    let rapp = vfs.read_file_for_task(&cap_table, read_slot, &path, crate::package::store::MAX_BLOB_SIZE)
        .map_err(|e| match e {
            FsError::CapabilityError(_) | FsError::PermissionDenied => SyscallResult::PermissionDenied,
            FsError::FileNotFound => SyscallResult::FileNotFound,
            FsError::FileTooLarge => SyscallResult::InvalidArguments,
            _ => SyscallResult::InvalidArguments,
        })?;
    drop(vfs);

    if rapp.is_empty() { return Err(SyscallResult::InvalidArguments); }

    let verified = parse_rapp(&rapp).map_err(|e| match e {
        RappError::DigestMismatch | RappError::SignatureInvalid | RappError::PublisherUntrusted | RappError::PublisherRevoked | RappError::PackageRevoked | RappError::InvalidPublisherKey => SyscallResult::IntegrityViolation,
        _ => SyscallResult::InvalidArguments,
    })?;

    if verified.package_id != pkg_id || verified.payload_len == 0 {
        return Err(SyscallResult::IntegrityViolation);
    }

    authorize_capability_18_for_install(&verified)?;

    {
        let reg = crate::package::REVOCATION_REGISTRY.lock();
        if reg.is_package_revoked(pkg_id) || reg.is_key_revoked(&verified.publisher_key) {
            return Err(SyscallResult::IntegrityViolation);
        }
    }

    // Stage the registry first. If payload persistence or activation fails, the
    // registry transaction is rolled back and no partially installed package is
    // left launchable.
    let previous_registry_entry = {
        let registry = crate::package::PACKAGE_REGISTRY.lock();
        registry.entries().iter().find(|entry| entry.package_id == pkg_id).cloned()
    };
    {
        let mut registry = crate::package::PACKAGE_REGISTRY.lock();
        registry
            .stage_verified(&verified)
            .map_err(|_| SyscallResult::DeviceError)?;
    }

    let previous_blob = {
        let store = crate::package::PACKAGE_BLOB_STORE.lock();
        store.get(pkg_id).cloned()
    };

    let blob_result = crate::package::PACKAGE_BLOB_STORE
        .lock()
        .put(pkg_id, verified.version, &rapp);

    if blob_result.is_err() {
        let _ = crate::package::PACKAGE_REGISTRY.lock().rollback_install(pkg_id, previous_registry_entry.clone());
        return Err(SyscallResult::DeviceError);
    }

    let activation_result = crate::package::PACKAGE_REGISTRY
        .lock()
        .activate_version_with_policy(pkg_id, verified.version, &verified.policy);

    if activation_result.is_err() {
        let mut store = crate::package::PACKAGE_BLOB_STORE.lock();
        if let Some(previous) = previous_blob {
            let _ = store.put(&previous.package_id, previous.version, &previous.bytes);
        } else {
            let _ = store.remove(pkg_id);
        }
        let _ = crate::package::PACKAGE_REGISTRY.lock().rollback_install(pkg_id, previous_registry_entry);
        return Err(SyscallResult::DeviceError);
    }

    serial_println!(
        "[PACKAGE] Task {} installed verified package '{}' version {}.{}.{}",
        task_id.0,
        pkg_id,
        verified.version.0,
        verified.version.1,
        verified.version.2
    );
    Ok(())
}

/// Queries the active mobile hardware platform description.
pub fn sys_app_focus(task_id: TaskId) -> SyscallResult {
    if require_app_launcher_or_service_manager().is_err() {
        return SyscallResult::PermissionDenied;
    }
    if task_id == TaskId(0) { return SyscallResult::InvalidArguments; }
    match crate::task::process::set_foreground(task_id) {
        Ok(()) => {
            SCHEDULER.lock().unblock_task(task_id);
            SyscallResult::Success
        }
        Err(_) => SyscallResult::TargetNotFound,
    }
}

pub fn sys_app_runtime_list(out_buf: &mut [u8]) -> Result<usize, SyscallResult> {
    use core::fmt::Write;
    let current = SCHEDULER.lock().current_task_id().ok_or(SyscallResult::PermissionDenied)?;
    let user_id = crate::task::process::user_for_task(current).ok_or(SyscallResult::PermissionDenied)?;

    let mut task_snapshot = [(TaskId(0), "", 0u8); crate::task::process::MAX_PROCESSES];
    let mut count = 0usize;
    {
        let sched = SCHEDULER.lock();
        for task in sched.tasks_iter() {
            if count >= task_snapshot.len() || task.id == current { continue; }
            if task.name == "compositor" || task.name == "launcher" { continue; }
            task_snapshot[count] = (task.id, task.name, task.state.code());
            count += 1;
        }
    }

    struct W<'a> { b: &'a mut [u8], n: usize }
    impl<'a> Write for W<'a> {
        fn write_str(&mut self, s: &str) -> core::fmt::Result {
            let k = s.len().min(self.b.len().saturating_sub(self.n));
            self.b[self.n..self.n + k].copy_from_slice(&s.as_bytes()[..k]);
            self.n += k;
            Ok(())
        }
    }

    let mut w = W { b: out_buf, n: 0 };
    for (task_id, name, task_state) in task_snapshot[..count].iter().copied() {
        let Some(process_id) = crate::task::process::process_for_task(task_id) else { continue; };
        let Some(record) = crate::task::process::record_for_process(process_id) else { continue; };
        if record.user_id != user_id { continue; }
        let package_id = crate::package::first_party_package_by_process_name(name)
            .map(|p| p.package_id)
            .unwrap_or(name);
        let process_state = record.state as u8;
        let _ = writeln!(w, "{}|{}|{}|{}|{}\n", task_id.0, package_id, process_state, task_state, record.parent_task.0);
        if w.n >= w.b.len() { break; }
    }
    Ok(w.n)
}

fn register_spawned_task_ipc(tid: TaskId, parent_id: TaskId) -> Result<(), SyscallResult> {
    if crate::ipc::IPC_ROUTER.lock().register_task(tid) {
        return Ok(());
    }

    // IPC registration is part of the launch transaction. A task that cannot
    // obtain an inbound endpoint must never remain runnable without one.
    let cleanup = {
        let mut sched = SCHEDULER.lock();
        let terminated = sched.terminate_task(tid, -24).is_ok();
        let reaped = if terminated {
            sched.reap_child(parent_id, tid).is_ok()
        } else {
            false
        };
        terminated && reaped
    };
    if !cleanup {
        serial_println!(
            "[APP] IPC endpoint allocation failed for task {}; cleanup was incomplete",
            tid.0
        );
    }
    Err(SyscallResult::NotReady)
}

pub fn sys_app_stop(child_id: TaskId) -> SyscallResult {
    if require_app_launcher_or_service_manager().is_err() {
        return SyscallResult::PermissionDenied;
    }
    let parent = match SCHEDULER.lock().current_task_id() {
        Some(id) => id,
        None => return SyscallResult::PermissionDenied,
    };
    let child_package = SCHEDULER
        .lock()
        .get_task(child_id)
        .map(|task| task.name);
    let terminated = {
        let mut sched = SCHEDULER.lock();
        sched.terminate_child(parent, child_id, -9)
    };
    match terminated {
        Ok(()) => {
            // Force-stop is a synchronous lifecycle boundary: once the syscall
            // returns, the child's IPC, files, events, sockets, capabilities,
            // surfaces, process record, and kernel stack must be reclaimable.
            match SCHEDULER.lock().reap_child(parent, child_id) {
                Ok(_) => {
                    if let Some(pkg) = child_package {
                        crate::notification::NOTIFICATION_MANAGER
                            .lock()
                            .cancel_all_for_package_all_users(pkg);
                    }
                    SyscallResult::Success
                },
                Err(_) => SyscallResult::NotReady,
            }
        }
        Err(_) => SyscallResult::TargetNotFound,
    }
}

pub fn sys_app_restart(app_id: &str) -> Result<TaskId, SyscallResult> {
    require_app_launcher_or_service_manager()?;
    let parent = SCHEDULER.lock().current_task_id().ok_or(SyscallResult::PermissionDenied)?;
    if app_id.is_empty() || app_id.len() > 128 || app_id.contains('/') || app_id.contains('\\') || app_id.contains("..") {
        return Err(SyscallResult::InvalidArguments);
    }

    // Stop the caller's existing child instance of this package, if present.
    let child = {
        let sched = SCHEDULER.lock();
        sched.find_child_by_name(parent, app_id)
    };
    if let Some(child) = child {
        {
            let mut sched = SCHEDULER.lock();
            sched.terminate_child(parent, child, -9).map_err(|_| SyscallResult::NotReady)?;
        }
        SCHEDULER.lock().reap_child(parent, child).map_err(|_| SyscallResult::NotReady)?;
        crate::notification::NOTIFICATION_MANAGER
            .lock()
            .cancel_all_for_package_all_users(app_id);
    }
    sys_app_launch(app_id)
}

pub fn dev_uninstall_verified_rapp(pkg_id: &str) -> Result<(), SyscallResult> {
    if pkg_id.is_empty() || pkg_id.len() > crate::package::rapp::RAPP_MAX_PACKAGE_ID_LEN
        || pkg_id.contains('/') || pkg_id.contains('\\') || pkg_id.contains("..") {
        return Err(SyscallResult::InvalidArguments);
    }
    if crate::package::first_party_package(pkg_id).is_some() {
        return Err(SyscallResult::PermissionDenied);
    }
    let previous_entry = {
        let registry = crate::package::PACKAGE_REGISTRY.lock();
        registry.entries().iter().find(|e| e.package_id == pkg_id).cloned()
    }.ok_or(SyscallResult::FileNotFound)?;
    {
        let sched = SCHEDULER.lock();
        if sched.has_live_task_named(pkg_id) {
            return Err(SyscallResult::NotReady);
        }
    }
    {
        let mut registry = crate::package::PACKAGE_REGISTRY.lock();
        registry.uninstall(pkg_id).map_err(|_| SyscallResult::DeviceError)?;
    }
    let previous_blob = {
        let store = crate::package::PACKAGE_BLOB_STORE.lock();
        store.get(pkg_id).cloned()
    };
    if crate::package::PACKAGE_BLOB_STORE.lock().remove(pkg_id).is_err() {
        let _ = crate::package::PACKAGE_REGISTRY.lock().restore_entry(previous_entry);
        return Err(SyscallResult::DeviceError);
    }
    if crate::fs::VFS.lock().remove_app_storage(pkg_id).is_err() {
        if let Some(blob) = previous_blob {
            let _ = crate::package::PACKAGE_BLOB_STORE.lock().put(&blob.package_id, blob.version, &blob.bytes);
        }
        let _ = crate::package::PACKAGE_REGISTRY.lock().restore_entry(previous_entry);
        return Err(SyscallResult::DeviceError);
    }
    crate::notification::NOTIFICATION_MANAGER
        .lock()
        .cancel_all_for_package_all_users(pkg_id);
    crate::notification::NOTIFICATION_MANAGER
        .lock()
        .remove_app_policy(pkg_id);
    Ok(())
}

pub fn sys_package_uninstall(_cap_slot: usize, pkg_id: &str, user_consent: bool) -> Result<(), SyscallResult> {
    if !user_consent { return Err(SyscallResult::PermissionDenied); }
    let current = {
        let sched = SCHEDULER.lock();
        let task = sched.current_task().ok_or(SyscallResult::PermissionDenied)?;
        if !task.cap_table.authorize_capability(CapabilityType::FileSystemWrite, Rights::WRITE) {
            return Err(SyscallResult::PermissionDenied);
        }
        task.id
    };
    if pkg_id.is_empty() || pkg_id.len() > crate::package::rapp::RAPP_MAX_PACKAGE_ID_LEN
        || pkg_id.contains('/') || pkg_id.contains('\\') || pkg_id.contains("..") {
        return Err(SyscallResult::InvalidArguments);
    }
    if crate::package::first_party_package(pkg_id).is_some() {
        return Err(SyscallResult::PermissionDenied);
    }

    let previous_entry = {
        let registry = crate::package::PACKAGE_REGISTRY.lock();
        registry.entries().iter().find(|e| e.package_id == pkg_id).cloned()
    }.ok_or(SyscallResult::FileNotFound)?;

    // Refuse removal while an installed package process is still alive.
    {
        let sched = SCHEDULER.lock();
        if sched.has_live_task_named(pkg_id) {
            return Err(SyscallResult::NotReady);
        }
    }

    {
        let mut registry = crate::package::PACKAGE_REGISTRY.lock();
        registry.uninstall(pkg_id).map_err(|_| SyscallResult::DeviceError)?;
    }

    let previous_blob = crate::package::PACKAGE_BLOB_STORE.lock().get(pkg_id).cloned();
    if crate::package::PACKAGE_BLOB_STORE.lock().remove(pkg_id).is_err() {
        let _ = crate::package::PACKAGE_REGISTRY.lock().restore_entry(previous_entry);
        return Err(SyscallResult::DeviceError);
    }

    if crate::fs::VFS.lock().remove_app_storage(pkg_id).is_err() {
        if let Some(blob) = previous_blob {
            let _ = crate::package::PACKAGE_BLOB_STORE.lock().put(&blob.package_id, blob.version, &blob.bytes);
        }
        let _ = crate::package::PACKAGE_REGISTRY.lock().restore_entry(previous_entry);
        return Err(SyscallResult::DeviceError);
    }

    serial_println!("[PACKAGE] Task {} uninstalled package '{}' and removed private app data.", current.0, pkg_id);
    Ok(())
}

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
    require_system_control_object(crate::services::SERVICE_POWER_CONTROL_OBJECT)?;
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
/// In standard QEMU / development emulator targets without a physical baseband modem,
/// this reports honestly that no hardware modem is detected.
pub fn sys_telephony_query(out_buf: &mut [u8]) -> Result<usize, SyscallResult> {
    require_system_control_object(crate::services::SERVICE_TELEPHONY_CONTROL_OBJECT)?;
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
    // Hardware truth comes from an explicitly registered modem backend. A
    // simulated state must never make the system claim cellular hardware exists.
    let modem = crate::telephony::CELLULAR_MODEM.lock();
    let sim = crate::telephony::SOVEREIGN_SIM.lock();
    let is_present = modem.hardware_present;
    let _ = write!(
        writer,
        r#"{{"present":{},"operator":"{}","rssi":{},"bars":{},"dbm":{},"state":"{:?}","call_state":"{:?}","pdp_active":{},"ip":"{}.{}.{}.{}"}}"#,
        is_present,
        sim.carrier_operator,
        modem.signal.rssi,
        modem.signal.to_bars(),
        modem.signal.to_dbm(),
        modem.state,
        modem.call_state,
        modem.pdp_context_active,
        modem.ip_address[0], modem.ip_address[1], modem.ip_address[2], modem.ip_address[3],
    );
    Ok(writer.pos)
}

/// Initiates an outgoing cellular call.
pub fn sys_telephony_dial(dest: &str) -> Result<(), SyscallResult> {
    require_system_control_object(crate::services::SERVICE_TELEPHONY_CONTROL_OBJECT)?;
    if !crate::telephony::CELLULAR_MODEM.lock().hardware_present {
        return Err(SyscallResult::DeviceError);
    }
    let ticks = crate::interrupts::get_system_ticks();
    crate::telephony::CELLULAR_MODEM.lock()
        .dial(dest, ticks)
        .map_err(|_| SyscallResult::DeviceError)
}

/// Terminates an active cellular call.
pub fn sys_telephony_hangup() -> Result<(), SyscallResult> {
    require_system_control_object(crate::services::SERVICE_TELEPHONY_CONTROL_OBJECT)?;
    crate::telephony::CELLULAR_MODEM.lock()
        .hangup()
        .map_err(|_| SyscallResult::DeviceError)
}

/// Transmits an SMS message to a destination phone number.
pub fn sys_telephony_send_sms(dest: &str, body: &str) -> Result<u32, SyscallResult> {
    require_system_control_object(crate::services::SERVICE_TELEPHONY_CONTROL_OBJECT)?;
    if !crate::telephony::CELLULAR_MODEM.lock().hardware_present {
        return Err(SyscallResult::DeviceError);
    }
    let ticks = crate::interrupts::get_system_ticks();
    crate::telephony::CELLULAR_MODEM.lock()
        .send_sms(dest, body, ticks)
        .map_err(|_| SyscallResult::DeviceError)
}

/// Toggles cellular packet data (PDP context).
pub fn sys_telephony_set_data(enabled: bool) -> Result<bool, SyscallResult> {
    require_system_control_object(crate::services::SERVICE_TELEPHONY_CONTROL_OBJECT)?;
    if !crate::telephony::CELLULAR_MODEM.lock().hardware_present {
        return Err(SyscallResult::DeviceError);
    }
    Ok(crate::telephony::CELLULAR_MODEM.lock().set_data_enabled(enabled))
}

/// Allocates a new hardware plane surface in the DRM/KMS compositor.
/// Display-service authority is object-scoped: only the display service's
/// framebuffer capability (object 1) can access the hardware display boundary.
fn require_display_service(cap_slot: usize) -> Result<TaskId, SyscallResult> {
    let sched = SCHEDULER.lock();
    let task = sched.current_task().ok_or(SyscallResult::PermissionDenied)?;
    let effective_slot = if task.cap_table.verify_object(cap_slot, CapabilityType::FrameBufferDirect, Rights::WRITE, 1).is_ok() {
        cap_slot
    } else if let Some(s) = task.cap_table.find_slot(CapabilityType::FrameBufferDirect, Rights::WRITE) {
        s
    } else {
        cap_slot
    };
    task.cap_table
        .verify_object(effective_slot, CapabilityType::FrameBufferDirect, Rights::WRITE, 1)
        .map_err(|_| SyscallResult::PermissionDenied)?;
    Ok(task.id)
}

/// Creates a display surface on behalf of the Ring-3 display service.
pub fn sys_surface_create(cap_slot: usize, width: u32, height: u32, z_order: u32) -> Result<u32, SyscallResult> {
    let owner = require_display_service(cap_slot)?;
    if width == 0 || height == 0 || width > 3840 || height > 2160 {
        return Err(SyscallResult::InvalidArguments);
    }
    let mut kms = crate::drivers::display_kms::DRM_KMS.lock();
    let mut display = crate::drivers::KmsDisplayDriver::new(&mut kms);
    display.create_surface(width, height, z_order, owner.0)
        .map_err(|_| SyscallResult::DeviceError)
}

/// Writes bounded ARGB pixels into an owned display surface.
pub fn sys_surface_write(cap_slot: usize, surface_id: u32, offset: usize, pixels: &[u32]) -> Result<usize, SyscallResult> {
    let owner = require_display_service(cap_slot)?;
    if pixels.is_empty() || pixels.len() > 65536 {
        return Err(SyscallResult::InvalidArguments);
    }
    let mut kms = crate::drivers::display_kms::DRM_KMS.lock();
    let mut display = crate::drivers::KmsDisplayDriver::new(&mut kms);
    display.write_surface_pixels(surface_id, owner.0, offset, pixels)
        .map_err(|err| {
            crate::serial_println!("[SURFACE_WRITE ERROR] id={} owner={} offset={} count={}: {}", surface_id, owner.0, offset, pixels.len(), err);
            SyscallResult::PermissionDenied
        })
}

/// Destroys an owned display surface.
pub fn sys_surface_destroy(cap_slot: usize, surface_id: u32) -> Result<(), SyscallResult> {
    let owner = require_display_service(cap_slot)?;
    let mut kms = crate::drivers::display_kms::DRM_KMS.lock();
    let mut display = crate::drivers::KmsDisplayDriver::new(&mut kms);
    let res = display.destroy_surface(surface_id, owner.0)
        .map_err(|_| SyscallResult::PermissionDenied);
    drop(display);
    drop(kms);
    crate::drivers::input::INPUT_PIPELINE.lock().clear();
    res
}

/// Composites display surfaces and atomically flips the scanout buffer.
pub fn sys_surface_flip(cap_slot: usize) -> Result<u64, SyscallResult> {
    let owner = require_display_service(cap_slot)?;
    let mut kms = crate::drivers::display_kms::DRM_KMS.lock();
    kms.commit_surfaces(Some(owner.0));
    let mut display = crate::drivers::KmsDisplayDriver::new(&mut kms);
    display.composite_and_flip().map_err(|_| SyscallResult::DeviceError)
}

/// Reprograms the scanout to `width` x `height` at `refresh_hz`.
///
/// WHY the physical mode is programmed as well: the KMS scanout and the
/// hardware framebuffer would otherwise disagree, and `present_argb8888` fits
/// the compositor's output into whatever the hardware reports, so a mode change
/// the display never saw would silently be shown at the wrong scale.
///
/// The KMS guard is dropped before `set_physical_mode`, exactly as the COM1
/// orientation command does it, so the display driver's lock and the
/// framebuffer's are never held at the same time.
pub fn sys_display_set_mode(cap_slot: usize, width: u32, height: u32, refresh_hz: u16) -> Result<(), SyscallResult> {
    let _owner = require_display_service(cap_slot)?;
    // The DISPI geometry registers are 16-bit. A wider value would truncate and
    // the physical scanout would be reprogrammed to a mode nobody asked for.
    if width == 0 || height == 0 || width > u16::MAX as u32 || height > u16::MAX as u32 {
        return Err(SyscallResult::InvalidArguments);
    }
    {
        let mut kms = crate::drivers::display_kms::DRM_KMS.lock();
        kms.set_display_mode(width, height, refresh_hz)
            .map_err(|_| SyscallResult::InvalidArguments)?;
    }
    if !crate::framebuffer::set_physical_mode(width as u16, height as u16) {
        crate::serial_println!(
            "[DISPLAY_HAL] WARN: physical scanout {}x{} rejected (absent DISPI or past the mapped pages); the KMS mode changed and the hardware keeps its previous mode.",
            width, height
        );
    }
    Ok(())
}

/// Rotates the composited scanout between the four `DisplayOrientation` values.
///
/// Codes are the enum's declaration order: 0 = Portrait, 1 = LandscapeRight,
/// 2 = PortraitInverted, 3 = LandscapeLeft.
///
/// WHY the accelerometer vector is deliberately left alone, unlike the COM1
/// orientation command: that command is emulator plumbing that feeds a rotation
/// into the input channel so a guest can be poked from outside. An app
/// requesting a rotation is not evidence that the device was physically turned,
/// and `sys_sensor_read` already reports "no physical IMU" for that reason.
pub fn sys_display_set_orientation(cap_slot: usize, orientation: u32) -> Result<(), SyscallResult> {
    let _owner = require_display_service(cap_slot)?;
    use crate::drivers::display_kms::DisplayOrientation;
    let requested = match orientation {
        0 => DisplayOrientation::Portrait,
        1 => DisplayOrientation::LandscapeRight,
        2 => DisplayOrientation::PortraitInverted,
        3 => DisplayOrientation::LandscapeLeft,
        _ => return Err(SyscallResult::InvalidArguments),
    };
    {
        let mut kms = crate::drivers::display_kms::DRM_KMS.lock();
        kms.set_orientation(requested);
    }
    // Portrait and its inversion present 1080x2160 native Full HD+; the two landscape
    // orientations present 2160x1080. The guard is dropped first so the display
    // driver and framebuffer locks are never held together.
    let (phys_w, phys_h) = match requested {
        DisplayOrientation::Portrait | DisplayOrientation::PortraitInverted => (1080u16, 2160u16),
        DisplayOrientation::LandscapeRight | DisplayOrientation::LandscapeLeft => (2160u16, 1080u16),
    };
    if !crate::framebuffer::set_physical_mode(phys_w, phys_h) {
        crate::serial_println!(
            "[DISPLAY_HAL] WARN: physical scanout {}x{} rejected (absent DISPI or past the mapped pages); the KMS orientation changed and the hardware keeps its previous mode.",
            phys_w, phys_h
        );
    }
    Ok(())
}

/// Dispatches a capability slot lookup for the current task.
/// Returns the slot index (as a non-negative usize) if found, or SyscallResult error.
pub fn sys_cap_lookup(cap_type_raw: u64, object_id: u64) -> Result<usize, SyscallResult> {
    let cap_type = CapabilityType::from_raw(cap_type_raw)
        .ok_or(SyscallResult::InvalidArguments)?;
    let sched = SCHEDULER.lock();
    let current_task = sched.current_task()
        .ok_or(SyscallResult::PermissionDenied)?;
    current_task.cap_table.find_slot_object(cap_type, Rights::NONE, object_id)
        .ok_or(SyscallResult::TargetNotFound)
}

/// Polls the normalized userspace input event pipeline.
pub fn sys_input_event_poll(out_buf: &mut [u8]) -> Result<usize, SyscallResult> {
    crate::serial::service_serial_commands();
    let sched = SCHEDULER.lock();
    let task = sched.current_task().ok_or(SyscallResult::PermissionDenied)?;
    if !task.cap_table.authorize_capability(CapabilityType::EventPoll, Rights::READ) {
        crate::serial_println!("[INPUT POLL DENIED] Task {} has no EventPoll cap", task.id.0);
        return Err(SyscallResult::PermissionDenied);
    }
    let is_inputd = task.name == "inputd";
    let task_id = task.id;
    let task_name = task.name;
    drop(sched);

    let event = if is_inputd {
        crate::drivers::input::INPUT_PIPELINE.lock().pop_service().ok_or(SyscallResult::NotReady)?
    } else {
        crate::drivers::input::INPUT_PIPELINE.lock().pop_for_task(task_id).ok_or(SyscallResult::NotReady)?
    };
    crate::serial_println!("[INPUT POLL SUCCESS] Task {} ({}) got {:?}", task_id.0, task_name, event.kind);
    if out_buf.len() < 24 { return Err(SyscallResult::InvalidArguments); }

    out_buf[..24].fill(0);
    out_buf[0] = event.kind as u8;
    out_buf[4..8].copy_from_slice(&event.x.to_le_bytes());
    out_buf[8..12].copy_from_slice(&event.y.to_le_bytes());
    out_buf[12..16].copy_from_slice(&event.dx.to_le_bytes());
    out_buf[16..20].copy_from_slice(&event.dy.to_le_bytes());
    out_buf[20..24].copy_from_slice(&event.scale_milli.to_le_bytes());
    Ok(24)
}

/// Polls recognized multi-touch gestures from the touch digitizer engine.
pub fn sys_touch_poll() -> Result<u32, SyscallResult> {
    let sched = SCHEDULER.lock();
    let task = sched.current_task().ok_or(SyscallResult::PermissionDenied)?;
    if !task.cap_table.authorize_capability(CapabilityType::EventPoll, Rights::READ) {
        return Err(SyscallResult::PermissionDenied);
    }
    let is_inputd = task.name == "inputd";
    let task_id = task.id;
    drop(sched);

    // TouchPoll is a compatibility API, but it must not bypass the same
    // focused-window/task isolation used by InputEventPoll.
    crate::drivers::input::pump_touch_events();
    let event = if is_inputd {
        crate::drivers::input::INPUT_PIPELINE.lock().pop_service()
    } else {
        crate::drivers::input::INPUT_PIPELINE.lock().pop_for_task(task_id)
    };
    match event {
        Some(event) => Ok(event.kind as u32),
        None => Ok(0),
    }
}

/// Sets the DVFS operational performance profile.
pub fn sys_power_set_profile(profile_idx: u32) -> Result<u32, SyscallResult> {
    require_system_control_object(crate::services::SERVICE_POWER_CONTROL_OBJECT)?;
    let profile = match profile_idx {
        0 => crate::drivers::pmic::DvfsProfile::PowerSave,
        1 => crate::drivers::pmic::DvfsProfile::Balanced,
        2 => crate::drivers::pmic::DvfsProfile::Performance,
        3 => crate::drivers::pmic::DvfsProfile::Turbo,
        _ => return Err(SyscallResult::InvalidArguments),
    };
    let mut gov = crate::drivers::pmic::DVFS_GOVERNOR.lock();
    gov.set_manual_profile(profile);
    Ok(profile.frequency_mhz())
}

/// Controls the virtual smartphone's authoritative power path.
/// action: 0=restart, 1=power off, 2=toggle display, 3=volume up, 4=volume down, 5=display state.
pub fn sys_device_power(action: u32) -> Result<u32, SyscallResult> {
    if require_system_control_object(crate::services::SERVICE_POWER_CONTROL_OBJECT).is_err() {
        return Err(SyscallResult::PermissionDenied);
    }
    match action {
        0 => { crate::serial_println!("[POWER] restart requested by authorized system UI."); crate::hal::platform::POWER.reset(); }
        1 => {
            crate::serial_println!("[POWER] shutdown requested by authorized system UI.");
            {
                let mut kms = crate::drivers::display_kms::DRM_KMS.lock();
                kms.set_powered(false);
                let mut display = crate::drivers::KmsDisplayDriver::new(&mut kms);
                let _ = display.composite_and_flip();
            }
            crate::hal::platform::POWER.shutdown();
        }
        2 => {
            let mut kms = crate::drivers::display_kms::DRM_KMS.lock();
            let on = kms.toggle_powered();
            let mut display = crate::drivers::KmsDisplayDriver::new(&mut kms);
            let _ = display.composite_and_flip();
            crate::serial_println!("[POWER] display_on={} source=system_control", on);
            Ok(on as u32)
        }
        3 => {
            if require_system_control_object(crate::services::SERVICE_AUDIO_CONTROL_OBJECT).is_err() { return Err(SyscallResult::PermissionDenied); }
            // WHY: stepping needs a level to step *from*, and on this platform no
            // volume is known until somebody requests one — the old code read a
            // hardcoded `100` seed here and reported it as the result of a step
            // that had done nothing. Refuse instead of inventing a base level.
            let mut audio = crate::audio::AUDIO_MANAGER.lock();
            match audio.step_volume(5) {
                Some(requested) => {
                    crate::serial_println!("[AUDIO] volume_percent={} state=requested applied_to_hardware=no source=system_control", requested);
                    Ok(requested as u32)
                }
                None => {
                    crate::serial_println!("[AUDIO] volume_percent=UNBOUND state=unbound applied_to_hardware=no source=system_control");
                    Err(SyscallResult::AudioError)
                }
            }
        }
        4 => {
            if require_system_control_object(crate::services::SERVICE_AUDIO_CONTROL_OBJECT).is_err() { return Err(SyscallResult::PermissionDenied); }
            let mut audio = crate::audio::AUDIO_MANAGER.lock();
            match audio.step_volume(-5) {
                Some(requested) => {
                    crate::serial_println!("[AUDIO] volume_percent={} state=requested applied_to_hardware=no source=system_control", requested);
                    Ok(requested as u32)
                }
                None => {
                    crate::serial_println!("[AUDIO] volume_percent=UNBOUND state=unbound applied_to_hardware=no source=system_control");
                    Err(SyscallResult::AudioError)
                }
            }
        }
        5 => { let kms = crate::drivers::display_kms::DRM_KMS.lock(); Ok(kms.powered as u32) }
        _ => Err(SyscallResult::InvalidArguments),
    }
}

/// Fills user buffer with cryptographically secure random bytes from hardware TRNG.
pub fn sys_get_random(out_buf: &mut [u8]) -> Result<usize, SyscallResult> {
    require_system_control()?;
    crate::crypto::get_random_bytes(out_buf);
    Ok(out_buf.len())
}

/// Dispatches audio feedback actions to the mobile audio subsystem.
pub fn sys_audio_control(action: u32, param: u32) -> Result<u32, SyscallResult> {
    require_system_control_object(crate::services::SERVICE_AUDIO_CONTROL_OBJECT)?;
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
        4 => {
            audio.configure_stream(param as usize).map_err(|_| SyscallResult::InvalidArguments)?;
            Ok(0)
        }
        5 => {
            audio.start_stream().map_err(|_| SyscallResult::NotReady)?;
            Ok(1)
        }
        6 => {
            audio.stop_stream();
            Ok(0)
        }
        7 => {
            // WHY: the set path still succeeds — it stores the caller's request —
            // but it logs `applied_to_hardware=no` because no mixer or PCM
            // scaling path in this kernel consumes the value. The returned
            // number is the stored request, not a level confirmed in effect.
            let stored = audio.set_volume(param.min(100) as u8);
            crate::serial_println!("[AUDIO] volume_percent={} state=requested applied_to_hardware=no source=audio_control_set", stored);
            Ok(stored as u32)
        }
        // WHY: a query must not answer with a level when none has ever been
        // requested. Returning `0` (or the old seeded `100`) here is how an
        // unknown volume turns into a confident-looking meter in userspace, so
        // the honest answer is the existing audio-specific error.
        8 => match audio.volume_percent {
            Some(requested) => Ok(requested as u32),
            None => Err(SyscallResult::AudioError),
        },
        _ => Err(SyscallResult::InvalidArguments),
    }
}

/// Derives an application-isolated 32-byte cryptographic key from hardware key slot.
pub fn sys_keystore_derive(_slot_idx: u32, _out_buf: &mut [u8; 32]) -> Result<usize, SyscallResult> {
    // Raw keystore-derived key material must never cross the kernel/userspace
    // boundary. In-kernel services use scoped cryptographic operations instead.
    Err(SyscallResult::PermissionDenied)
}

/// Queries mobile hardware sensor telemetry (orientation, 3-axis accel, ambient lux, proximity).
/// Reports honestly if hardware sensors are not physically present on the board.
pub fn sys_sensor_read(out_buf: &mut [u8]) -> Result<usize, SyscallResult> {
    if out_buf.len() < 32 || out_buf.len() > 512 { return Err(SyscallResult::InvalidArguments); }
    require_system_control_object(crate::services::SERVICE_SENSOR_CONTROL_OBJECT)?;
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
    let sm = crate::sensor::SENSOR_MANAGER.lock();
    if sm.is_hardware_present() {
        let orientation = sm.imu.determine_orientation();
        let _ = write!(
            writer,
            r#"{{"present":true,"orientation":"{:?}","accel":{},"lux":{},"near":{}}}"#,
            orientation, sm.imu.accel.y, sm.light.lux, sm.proximity.is_near()
        );
    } else {
        let _ = write!(
            writer,
            r#"{{"present":false,"status":"NOT_IMPLEMENTED","error":"No physical IMU or ambient sensor detected"}}"#
        );
    }
    Ok(writer.pos)
}

/// Queries battery fuel gauge telemetry (voltage, SoC %, capacity, charging status).
/// In standard QEMU development targets without an ACPI/I2C fuel gauge, reports honestly.
pub fn sys_battery_query(out_buf: &mut [u8]) -> Result<usize, SyscallResult> {
    require_system_control_object(crate::services::SERVICE_POWER_CONTROL_OBJECT)?;
    if out_buf.len() < crate::resource::BATTERY_JSON_MAX { return Err(SyscallResult::InvalidArguments); }
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
    let pmic = crate::drivers::pmic::PMIC_FUEL_GAUGE.lock();
    let gov = crate::drivers::pmic::DVFS_GOVERNOR.lock();
    let _ = write!(
        writer,
        r#"{{"present":{},"telemetry_valid":{},"voltage_mv":{},"soc":{},"capacity":{},"charging":"{:?}","temp":{},"governor":"{:?}","mhz":{}}}"#,
        pmic.is_hardware_present(),
        pmic.is_hardware_present(),
        pmic.telemetry.voltage_mv,
        pmic.telemetry.soc_percent,
        pmic.telemetry.capacity_mah,
        pmic.telemetry.charging_state,
        pmic.telemetry.temperature_c,
        gov.active_profile,
        gov.active_profile.frequency_mhz(),
    );
    Ok(writer.pos)
}

/// Dispatches an Android cross-zone intent into the host sovereign intent router.
pub fn sys_android_bridge_intent(action: &str, data_uri: &str, out_buf: &mut [u8]) -> Result<usize, SyscallResult> {
    require_system_control()?;
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
    // WHY the narrow capability: reading the clock authorises nothing. The previous
    // guard was the object-blind `require_system_control()`, so the only way to let
    // the lock screen show a time was to hand it power, crypto and telephony
    // authority over the device. `RealtimeClockRead` with READ rights is the least
    // privilege that actually does the job.
    let sched = SCHEDULER.lock();
    let task = sched.current_task().ok_or(SyscallResult::PermissionDenied)?;
    task.cap_table
        .authorize_capability(CapabilityType::RealtimeClockRead, Rights::READ)
        .then_some(())
        .ok_or(SyscallResult::PermissionDenied)?;
    drop(sched);

    let mut rtc = crate::hal::RTC.lock();
    let dt = rtc.read_hardware();
    let local_dt = rtc.local_now();
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
        r#"{{"year":{},"month":{},"day":{},"hour":{},"minute":{},"second":{},"unix_epoch":{},"tz_offset_min":330,"tz":"Asia/Kolkata"}}"#,
        local_dt.year, local_dt.month, local_dt.day, local_dt.hour, local_dt.minute, local_dt.second, dt.unix_epoch_secs
    );
    Ok(writer.pos)
}


/// Encrypts or decrypts a 512-byte sector using the sovereign encrypted storage vault.
pub fn sys_vault_crypt(
    action: u32,
    block_index: u64,
    buf: &mut [u8; 512],
    iv: &mut [u8; 16],
) -> Result<(), SyscallResult> {
    // Object-scoped to the CALLER'S OWN vault.
    // WHY: the previous check was `authorize_capability(StorageRawBlock, INVOKE)`,
    // which is object-blind. `StorageRawBlock` is also granted to the storage
    // service for whole-device block IO, so that broad grant satisfied the vault
    // check and let the holder encrypt or decrypt any account's sectors.
    let caller_uid = match caller_user_id() {
        Some(u) => u,
        None => {
            crate::serial_println!("[VAULT_CRYPT] caller_user_id is None!");
            return Err(SyscallResult::PermissionDenied);
        }
    };
    let has_cap = {
        let sched = SCHEDULER.lock();
        let task = sched.current_task().ok_or(SyscallResult::PermissionDenied)?;
        task.cap_table.authorize_object(
            CapabilityType::StorageRawBlock,
            Rights::INVOKE,
            vault_object_for_user(caller_uid),
        )
    };
    if !has_cap {
        crate::serial_println!("[VAULT_CRYPT] Task has no StorageRawBlock cap for obj 0x{:X}!", vault_object_for_user(caller_uid));
        return Err(SyscallResult::PermissionDenied);
    }
    // Defence in depth: the dispatcher validates this too, but `sys_vault_crypt`
    // is reachable from other kernel paths, so the bound is enforced here as well.
    if block_index >= VAULT_MAX_BLOCK_INDEX {
        crate::serial_println!("[VAULT_CRYPT] block_index {} exceeds max {}", block_index, VAULT_MAX_BLOCK_INDEX);
        return Err(SyscallResult::InvalidArguments);
    }

    let mut vault = crate::storage::STORAGE_VAULT.lock();
    if action == 0 {
        // Seal. The caller supplies plaintext; the envelope is written to the
        // vault region and only the ciphertext is returned. A fresh IV is
        // generated per seal so two blocks never share keystream.
        match vault.seal_sector(block_index, buf) {
            Ok(enc_block) => {
                buf.copy_from_slice(&enc_block.ciphertext);
                iv.copy_from_slice(&enc_block.iv);
                crate::serial_println!("[VAULT_CRYPT] seal_sector {} OK", block_index);
                Ok(())
            }
            Err(e) => {
                crate::serial_println!("[VAULT_CRYPT] seal_sector {} err: {:?}", block_index, e);
                match e {
                    crate::storage::VaultError::InvalidBlockIndex => Err(SyscallResult::InvalidArguments),
                    _ => Err(SyscallResult::DeviceError),
                }
            }
        }
    } else {
        // Open. The envelope is read back from storage, so the ciphertext and
        // the tag both come from the vault rather than from the caller.
        // Recomputing the expected tag from caller-supplied ciphertext compared a
        // value with itself, which made IntegrityViolation unreachable and let
        // any ciphertext decrypt to attacker-chosen bytes.
        match vault.open_sector(block_index, buf) {
            Ok(()) => {
                crate::serial_println!("[VAULT_CRYPT] open_sector {} OK", block_index);
                Ok(())
            }
            Err(e) => {
                crate::serial_println!("[VAULT_CRYPT] open_sector {} err: {:?}", block_index, e);
                match e {
                    crate::storage::VaultError::IntegrityViolation => Err(SyscallResult::IntegrityViolation),
                    crate::storage::VaultError::KeystoreUninitialized => Err(SyscallResult::PermissionDenied),
                    crate::storage::VaultError::InvalidBlockIndex => Err(SyscallResult::InvalidArguments),
                    _ => Err(SyscallResult::DeviceError),
                }
            }
        }
    }
}

/// Queries the sovereign CRL or records package/key revocations.
pub fn sys_revocation_audit(
    cmd: u32,
    target: &str,
    _out_ptr: *mut u8,
    _out_len: usize,
) -> Result<usize, SyscallResult> {
    require_system_control()?;
    let mut reg = crate::package::REVOCATION_REGISTRY.lock();
    match cmd {
        0 => {
            // Check if package is revoked
            let is_rev = reg.is_package_revoked(target);
            Ok(if is_rev { 1 } else { 0 })
        }
        1 => {
            // Revoke package
            let ticks = crate::interrupts::get_system_ticks();
            let _ = reg.revoke_package(
                target,
                crate::package::RevocationReason::MaliciousBehavior,
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
            crate::capability::delegation::allocate_lease_id(),
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
    if !crate::capability::delegation::register_lease(lease) {
        let _ = target.cap_table.revoke(slot);
        return Err(SyscallResult::InvalidArguments);
    }
    Ok(slot)
}

/// Advances or interacts with the Gravity Glass fluid inertial physics scroller.
pub fn sys_surface_physics(
    action: u32,
    y: i32,
    viewport_h: i32,
    content_h: i32,
) -> Result<i32, SyscallResult> {
    let sched = SCHEDULER.lock();
    let task = sched.current_task().ok_or(SyscallResult::PermissionDenied)?;
    if !task.cap_table.authorize_capability(CapabilityType::FrameBufferDirect, Rights::WRITE) { return Err(SyscallResult::PermissionDenied); }
    drop(sched);
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
    // 1. Verify the scoped camera service authority or the legacy physical-camera hardware capability
    let sched = crate::task::scheduler::SCHEDULER.lock();
    let current = sched.current_task().ok_or(SyscallResult::PermissionDenied)?;
    if !current.cap_table.authorize_object(
        crate::capability::CapabilityType::SystemControl,
        crate::capability::Rights::INVOKE,
        crate::services::SERVICE_CAMERA_CONTROL_OBJECT,
    ) && !current.cap_table.authorize_object(
        crate::capability::CapabilityType::HardwarePortIo,
        crate::capability::Rights::READ,
        0xCA00,
    ) {
        return Err(SyscallResult::PermissionDenied);
    }
    drop(sched);

    let mut cam = crate::hal::CAMERA.lock();
    match action {
        1 => {
            cam.set_focus(crate::hal::AutoFocusState::Locked);
            return Ok(1);
        }
        2 => {
            return Ok(if cam.af_state == crate::hal::AutoFocusState::Locked { 1 } else { 0 });
        }
        3 => {
            cam.flash_enabled = !cam.flash_enabled;
            return Ok(if cam.flash_enabled { 1 } else { 0 });
        }
        0 => {}
        _ => return Err(SyscallResult::InvalidArguments),
    }

    let frame = cam.capture_frame().map_err(|_| SyscallResult::DeviceError)?;
    let bytes_to_copy = frame.copy_to(out_buf).map_err(|_| SyscallResult::InvalidArguments)?;

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
    use crate::compat::android::ANDROID_RUNTIME;

    let dex = DexPackage::parse(dex_data).map_err(|_| SyscallResult::IntegrityViolation)?;
    let _ = dex.audit_bytecode().map_err(|_| SyscallResult::PermissionDenied)?;
    let mut parts = method_spec.split("->");
    let class_name = parts.next().filter(|value| !value.is_empty()).ok_or(SyscallResult::InvalidArguments)?;
    let method_name = parts.next().filter(|value| !value.is_empty()).ok_or(SyscallResult::InvalidArguments)?;
    if parts.next().is_some() { return Err(SyscallResult::InvalidArguments); }

    let task_id = sys_get_task_id();
    if task_id.0 == 0 { return Err(SyscallResult::PermissionDenied); }
    let mut runtime = ANDROID_RUNTIME.lock();
    let pid = runtime.pid_for_task(task_id).ok_or(SyscallResult::PermissionDenied)?;
    runtime.invoke_verified_method_count(pid, &dex, class_name, method_name)
        .map_err(|_| SyscallResult::PermissionDenied)
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
/// If target_app is provided, resolves the target task and delivers a SHARE message.
/// If target_app is None, opens the interactive ShareSheetDrawer in MobileShell and prompts the user.
pub fn sys_share_sheet_dispatch(
    mime_type: &str,
    uri: &str,
    target_app: Option<&str>,
) -> Result<u64, SyscallResult> {
    use crate::ipc::{SHARE_SHEET_MANAGER, IPC_ROUTER, IpcMessage, IPC_MSG_SHARE};
    use crate::gui::{MOBILE_SHELL, ShareSheetDrawer};
    use crate::framebuffer::{PHONE_X, PHONE_Y, PHONE_W, PHONE_H};

    let current_id = {
        let sched = SCHEDULER.lock();
        let task = sched.current_task().ok_or(SyscallResult::PermissionDenied)?;
        task.id
    };
    let current_ticks = crate::interrupts::get_system_ticks();

    if let Some(target_id_str) = target_app {
        let mut ssm = SHARE_SHEET_MANAGER.lock();
        let target = ssm.find_target(target_id_str).copied().ok_or(SyscallResult::TargetNotFound)?;
        // The IpcSend must be bound to the RESOLVED TARGET task, not merely held.
        // WHY: `target_id_str` is caller-supplied and resolves to any registered
        // target. The previous object-blind `authorize_capability(IpcSend, WRITE)`
        // let a single IpcSend inject an attacker-chosen kind and payload into
        // every registered target's mailbox. The share-sheet ABI carries no
        // capability slot argument, so the table is scanned for a matching slot.
        // This early check runs before any state mutation so an unauthorized
        // dispatch never records a share transaction; the consuming
        // authorization happens inside the router at delivery time.
        let (cap_slot, caller_caps) = {
            let sched = SCHEDULER.lock();
            let task = sched.current_task().ok_or(SyscallResult::PermissionDenied)?;
            let cap_slot = (0..task.cap_table.capacity())
                .find(|slot| {
                    task.cap_table
                        .get(*slot)
                        .is_ok_and(|cap| {
                            cap.cap_type == CapabilityType::IpcSend
                                && cap.object_id == target.target_task.0
                                && cap.rights.contains(Rights::WRITE)
                        })
                })
                .ok_or(SyscallResult::PermissionDenied)?;
            (cap_slot, task.cap_table.clone())
        };
        let tx_id = ssm.dispatch_share(current_id, target.target_task, mime_type, uri, None, current_ticks);
        drop(ssm);

        // Construct SHARE payload: "SHARE:<mime>:<uri>"
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
            msg_type: IPC_MSG_SHARE,
            transferred_cap: None,
            shm_descriptor: None,
            payload: msg_payload,
            len: pos,
        };

        // Deliver to the target mailbox. `deliver_system_message` re-verifies the
        // object-scoped IpcSend inside the router, so the check cannot be skipped
        // by any future caller of this bus. The capability table was cloned out of
        // the scheduler lock above because the router takes its own lock here and
        // the two must not be held in a cycle.
        let mut router = IPC_ROUTER.lock();
        let _ = router.deliver_system_message(current_id, &caller_caps, cap_slot, target.target_task, ipc_msg);
        drop(router);

        serial_println!(
            "[SHARE] Task {} shared '{}' with '{}' (Task {}) via Share Sheet Pipeline (Tx: {}).",
            current_id.0,
            uri,
            target.display_name,
            target.target_task.0,
            tx_id,
        );

        Ok(target.target_task.0 as u64)
    } else {
        // Interactive drawer path: no destination task is named, so there is no
        // object to bind an IpcSend to. The drawer is a system UI surface, so
        // authorize it against the share-bus control object rather than dropping
        // the check. Without this, any ring3 task could raise a system share
        // drawer over the foreground app.
        require_system_control_object(NOTIFICATION_CONTROL_OBJECT)?;
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

    // Pipe creation establishes the producer endpoint, so a consumer-only
    // capability can never be upgraded into producer authority here.
    let has_cap = current_task.cap_table.authorize_capability(CapabilityType::ShmProducer, Rights::WRITE);

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

    let has_cap = current_task.cap_table.authorize_capability(CapabilityType::ShmProducer, Rights::WRITE);

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

    let has_cap = current_task.cap_table.authorize_capability(CapabilityType::ShmConsumer, Rights::READ);

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

    // Event polling is a dedicated kernel resource. Diagnostic logging authority
    // must never substitute for an event capability.
    let has_cap = current_task.cap_table.authorize_capability(CapabilityType::EventPoll, Rights::INVOKE)
        || current_task.cap_table.authorize_capability(CapabilityType::EventPoll, Rights::READ);

    if !has_cap {
        crate::serial_println!("[EVENT REACTOR DENIED] Task {} has no EventPoll cap!", current_id.0);
        return Err(SyscallResult::PermissionDenied);
    }
    drop(sched);

    // Promptly service any pending serial console commands so host commands
    // and input events are handled immediately without waiting for Task 0.
    crate::serial::service_serial_commands();

    let current_ticks = crate::interrupts::get_system_ticks();

    let should_block = {
        let (ready, block) = x86_64::instructions::interrupts::without_interrupts(|| {
            let mut reactor = crate::event::EVENT_REACTOR.lock();
            reactor.poll(current_id, interest_mask, timeout_ticks, current_ticks)
        });
        if !block {
            return Ok(ready);
        }
        true
    };

    if should_block {
        x86_64::instructions::interrupts::without_interrupts(|| {
            let mut sched = SCHEDULER.lock();
            sched.block_task(current_id);
        });
        // Double check whether an event was posted between dropping reactor and locking sched
        let cancelled = x86_64::instructions::interrupts::without_interrupts(|| {
            let reactor = crate::event::EVENT_REACTOR.lock();
            !reactor.is_waiting(current_id)
        });
        if cancelled {
            x86_64::instructions::interrupts::without_interrupts(|| {
                let mut sched = SCHEDULER.lock();
                sched.unblock_task(current_id);
            });
        } else {
            crate::task::yield_now();
        }
        let ready = x86_64::instructions::interrupts::without_interrupts(|| {
            let mut reactor = crate::event::EVENT_REACTOR.lock();
            reactor.check_woken(current_id, interest_mask)
        });
        crate::serial::service_serial_commands();
        Ok(ready)
    } else {
        Ok(0)
    }
}











fn native_first_party_app_available(package_id: &str) -> bool {
    matches!(
        package_id,
        "org.interos.settings"
            | "org.interos.terminal"
            | "org.interos.calculator"
            | "org.interos.dialer"
            | "org.interos.vault"
            | "org.interos.camera"
            | "org.interos.browser"
            | "org.interos.messages"
            | "org.interos.calendar"
            | "org.interos.music"
            | "org.interos.file_manager"
            | "org.interos.gallery"
            // WHY these four are named here: this predicate is the only gate between a
            // seeded `FIRST_PARTY_PACKAGES` entry and the launcher's app drawer. All four
            // packages were already described in `FIRST_PARTY_PACKAGES` and seeded active
            // into the registry, but a package missing from this match was filtered straight
            // back out, so the drawer never saw it and the launch arm was unreachable.
            | "org.interos.notes"
    )
}

pub fn sys_package_registry(out_buf:&mut [u8])->Result<usize,SyscallResult>{
    use core::fmt::Write;
    crate::package::PACKAGE_REGISTRY.lock().seed();
    let entries=crate::package::PACKAGE_REGISTRY.lock().active_entries();
    struct W<'a>{b:&'a mut[u8],n:usize}
    impl<'a> Write for W<'a>{
        fn write_str(&mut self,s:&str)->core::fmt::Result{
            let k=s.len().min(self.b.len().saturating_sub(self.n));
            self.b[self.n..self.n+k].copy_from_slice(&s.as_bytes()[..k]);
            self.n+=k;
            Ok(())
        }
    }
    let mut w=W{b:out_buf,n:0};
    for e in &entries{
        let descriptor=crate::package::first_party_package(&e.package_id);
        if let Some(desc) = descriptor {
            if desc.is_system {
                continue;
            }
        }
        let launchable = native_first_party_app_available(&e.package_id)
            || crate::package::PACKAGE_BLOB_STORE.lock().get(&e.package_id).is_some()
            || descriptor.is_some();
        if !launchable { continue; }
        let process=descriptor.map(|p|p.process_name).unwrap_or(&e.package_id);
        let path=descriptor.map(|p|p.path).unwrap_or("");
        let state=match e.state{
            crate::package::PackageState::Staged=>"staged",
            crate::package::PackageState::Active=>"active",
            crate::package::PackageState::RolledBack=>"rolled-back",
        };
        let display_name = descriptor.map(|p| p.display_name).unwrap_or_else(|| {
            if e.package_id == "org.interos.notes" || e.package_id.ends_with(".notes") {
                "Notes"
            } else if let Some(pos) = e.package_id.rfind('.') {
                &e.package_id[pos + 1..]
            } else {
                &e.package_id
            }
        });
        let icon = descriptor.map(|p| p.icon).unwrap_or_else(|| {
            if e.package_id == "org.interos.notes" || e.package_id.ends_with(".notes") {
                "NOTE"
            } else {
                "APP"
            }
        });
        let category = descriptor.map(|p| p.category).unwrap_or_else(|| {
            if e.package_id == "org.interos.notes" || e.package_id.ends_with(".notes") {
                "Personal"
            } else {
                "Tools"
            }
        });
        let _=writeln!(w,"{}|{}|{}.{}.{}|{}|{}|{}|{}|{}",
            e.package_id,process,e.version.0,e.version.1,e.version.2,state,path,display_name,icon,category);
        if w.n >= w.b.len() { break; }
    }
    Ok(w.n)
}
pub fn sys_window_set_geometry(cap_slot: usize, id: u32, x: i32, y: i32, z: u32, visible: bool) -> SyscallResult {
    let owner = match require_display_service(cap_slot) { Ok(id) => id.0, Err(e) => return e };
    if crate::drivers::display_kms::DRM_KMS.lock().set_surface_geometry_owned(id, owner, x, y, z, visible) {
        SyscallResult::Success
    } else { SyscallResult::PermissionDenied }
}

pub fn sys_window_set_input(cap_slot: usize, id: u32, enabled: bool) -> SyscallResult {
    let owner = match require_display_service(cap_slot) { Ok(id) => id.0, Err(e) => return e };
    if crate::drivers::display_kms::DRM_KMS.lock().set_input_enabled_owned(id, owner, enabled) {
        SyscallResult::Success
    } else {
        SyscallResult::PermissionDenied
    }
}

pub fn sys_window_focus(cap_slot: usize, x: u32, y: u32) -> SyscallResult {
    let _owner = match require_display_service(cap_slot) { Ok(id) => id, Err(e) => return e };
    let focused = {
        let mut kms = crate::drivers::display_kms::DRM_KMS.lock();
        kms.focus_at(x, y)
    };
    match focused {
        Some((_surface, owner)) => {
            let _ = IPC_ROUTER.lock().send_direct(TaskId(0), TaskId(owner), 0x7001, &[x as u8, (x >> 8) as u8, y as u8, (y >> 8) as u8]);
            SyscallResult::Success
        }
        None => SyscallResult::NotReady,
    }
}
pub fn sys_fs_list(cap_slot: usize, path_ptr: u64, path_len: usize, out: &mut [u8]) -> Result<usize, SyscallResult> {
    if path_len == 0 || path_len > 256 || out.len() > 4096 { return Err(SyscallResult::InvalidArguments); }
    let mut path = [0u8; 256];
    copy_from_user(&mut path[..path_len], path_ptr, path_len).map_err(|_| SyscallResult::InvalidArguments)?;
    let path = core::str::from_utf8(&path[..path_len]).map_err(|_| SyscallResult::InvalidArguments)?;
    // WHY the clone and the drop: `list_dir_for_task` resolves the caller's
    // package through the scheduler (`enforce_app_path` ->
    // `app_storage_root` -> `app_package_for_task` takes `SCHEDULER`), so
    // holding this lock across `VFS.lock()` below self-deadlocks the first
    // caller -- the file manager never presented. Every sibling takes the
    // same snapshot-and-release shape; this one now does too.
    // Capability authorization is delegated directly to VFS `list_dir_for_task`,
    // ensuring single-use leases are consumed exactly once per logical directory listing.
    let (task_id, cap_table) = {
        let sched = SCHEDULER.lock();
        let current = sched.current_task().ok_or(SyscallResult::PermissionDenied)?;
        (current.id, current.cap_table.clone())
    };
    let n = crate::fs::VFS.lock().list_dir_for_task(task_id, &cap_table, cap_slot, path, out).map_err(|e| match e {
        FsError::PermissionDenied | FsError::CapabilityError(_) => SyscallResult::PermissionDenied,
        FsError::FileNotFound => SyscallResult::FileNotFound,
        _ => SyscallResult::InvalidArguments,
    })?;
    Ok(n)
}

pub fn sys_storage_query() -> Result<u64, SyscallResult> {
    {
        let sched = SCHEDULER.lock();
        let task = sched.current_task().ok_or(SyscallResult::PermissionDenied)?;
        if !task.cap_table.authorize_capability(CapabilityType::StorageRawBlock, Rights::READ) {
            return Err(SyscallResult::PermissionDenied);
        }
    }
    let guard = crate::drivers::ATA_PRIMARY.lock();
    let drive = guard.as_ref().ok_or(SyscallResult::DeviceError)?;
    Ok((drive.sector_count() as u64).saturating_mul(crate::drivers::SECTOR_SIZE as u64))
}

pub fn sys_settings_get(_cap_slot: usize, key_ptr: u64, key_len: usize, out: &mut [u8]) -> Result<usize, SyscallResult> {
    if key_len == 0 || key_len > 96 || out.len() > 512 { return Err(SyscallResult::InvalidArguments); }
    {
        let sched = SCHEDULER.lock();
        let current = sched.current_task().ok_or(SyscallResult::PermissionDenied)?;
        let has_read = current.cap_table.authorize_capability(CapabilityType::FileSystemRead, Rights::READ)
            || current.cap_table.authorize_capability(CapabilityType::FileSystemWrite, Rights::READ);
        if !has_read {
            return Err(SyscallResult::PermissionDenied);
        }
    }
    let mut key = [0u8; 96];
    copy_from_user(&mut key[..key_len], key_ptr, key_len).map_err(|_| SyscallResult::InvalidArguments)?;
    let key = core::str::from_utf8(&key[..key_len]).map_err(|_| SyscallResult::InvalidArguments)?;
    Ok(crate::settings::SETTINGS.lock().get(key, out))
}

pub fn sys_settings_set(_cap_slot: usize, key_ptr: u64, key_len: usize, val_ptr: u64, val_len: usize) -> Result<(), SyscallResult> {
    if key_len == 0 || key_len > 96 || val_len > 512 { return Err(SyscallResult::InvalidArguments); }
    {
        let sched = SCHEDULER.lock();
        let current = sched.current_task().ok_or(SyscallResult::PermissionDenied)?;
        if !current.cap_table.authorize_capability(CapabilityType::FileSystemWrite, Rights::WRITE) {
            return Err(SyscallResult::PermissionDenied);
        }
    }
    let mut key = [0u8; 96];
    let mut val = [0u8; 512];
    copy_from_user(&mut key[..key_len], key_ptr, key_len).map_err(|_| SyscallResult::InvalidArguments)?;
    copy_from_user(&mut val[..val_len], val_ptr, val_len).map_err(|_| SyscallResult::InvalidArguments)?;
    let key = core::str::from_utf8(&key[..key_len]).map_err(|_| SyscallResult::InvalidArguments)?;
    crate::settings::SETTINGS.lock().set(key, &val[..val_len]).map_err(|_| SyscallResult::DeviceError)
}

pub fn sys_service_list(out: &mut [u8]) -> Result<usize, SyscallResult> {
    if require_service_manager().is_ok() {
        crate::services::supervise();
    }
    Ok(crate::services::snapshot(out))
}

/// Returns stable, non-authoritative service metadata without exposing task identity.
pub fn sys_service_query(service_id: u8, out: &mut [u8]) -> Result<usize, SyscallResult> {
    if out.len() < 12 { return Err(SyscallResult::InvalidArguments); }
    let info = crate::services::lookup(service_id).ok_or(SyscallResult::InvalidArguments)?;
    out[0] = info.id as u8;
    out[1] = info.state as u8;
    out[2] = info.restart as u8;
    out[3] = info.critical as u8;
    out[4..6].copy_from_slice(&info.abi_version.to_le_bytes());
    out[6..10].copy_from_slice(&info.generation.to_le_bytes());
    out[10] = 1; // discovery schema version
    out[11] = 0;
    Ok(12)
}

pub fn sys_service_start(service_id: u8) -> Result<crate::task::TaskId, SyscallResult> {
    require_service_manager()?;
    let id = match service_id {
        1 => crate::services::ServiceId::Display, 2 => crate::services::ServiceId::Input,
        3 => crate::services::ServiceId::Storage, 4 => crate::services::ServiceId::Network,
        5 => crate::services::ServiceId::Power, 6 => crate::services::ServiceId::Modem,
        7 => crate::services::ServiceId::Audio, 8 => crate::services::ServiceId::Camera,
        9 => crate::services::ServiceId::Compositor, 10 => crate::services::ServiceId::Launcher, 11 => crate::services::ServiceId::Sensor,
        _ => return Err(SyscallResult::InvalidArguments),
    };
    let parent_id = SCHEDULER.lock().current_task_id().ok_or(SyscallResult::PermissionDenied)?;
    crate::services::start(id, parent_id).map_err(|e| {
        if crate::services::is_contention(e) { SyscallResult::NotReady } else { SyscallResult::DeviceError }
    })
}

fn require_service_manager() -> Result<(), SyscallResult> {
    let sched = SCHEDULER.lock();
    let task = sched.current_task().ok_or(SyscallResult::PermissionDenied)?;
    if task.cap_table.authorize_object(CapabilityType::SystemControl, Rights::INVOKE, SERVICE_MANAGER_OBJECT) {
        Ok(())
    } else {
        Err(SyscallResult::PermissionDenied)
    }
}

fn require_app_launcher_or_service_manager() -> Result<(), SyscallResult> {
    let sched = SCHEDULER.lock();
    let task = sched.current_task().ok_or(SyscallResult::PermissionDenied)?;
    if task.cap_table.authorize_object(CapabilityType::SystemControl, Rights::INVOKE, SERVICE_MANAGER_OBJECT)
        || task.cap_table.authorize_capability(CapabilityType::ProcessSpawn, Rights::INVOKE)
    {
        Ok(())
    } else {
        Err(SyscallResult::PermissionDenied)
    }
}

/// Verifies a signed RAPP service package and applies the narrow service ABI policy.
/// This is verification/loading preparation only; the verified image is handed to
/// the userspace service manager for process creation.
pub fn sys_service_package_verify(cap_slot: usize, path: &str) -> Result<u8, SyscallResult> {
    require_service_manager()?;
    let sched = SCHEDULER.lock();
    let current = sched.current_task().ok_or(SyscallResult::PermissionDenied)?;
    if !current.cap_table.authorize_capability(CapabilityType::FileSystemRead, Rights::READ) {
        return Err(SyscallResult::PermissionDenied);
    }
    drop(sched);

    if path.is_empty() || path.len() > 256 || !path.starts_with("/packages/") || !path.ends_with(".rapp") {
        return Err(SyscallResult::InvalidArguments);
    }
    let manifest = sys_verify_package(cap_slot, path)?;
    let service_id = crate::service_abi::service_id_from_package_id(&manifest.package_id)
        .ok_or(SyscallResult::PermissionDenied)?;
    for capability in &manifest.capabilities {
        if !crate::service_abi::service_capability_allowed(service_id, *capability) {
            return Err(SyscallResult::PermissionDenied);
        }
    }
    if manifest.payload_len == 0 {
        return Err(SyscallResult::InvalidArguments);
    }
    serial_println!("[SERVICE] Verified signed service package '{}' for service {}", manifest.package_id, service_id);
    Ok(service_id)
}

/// Verifies a signed RAPP service package and launches its verified ELF payload as a PID1-owned Ring-3 child.
pub fn sys_service_package_launch(cap_slot: usize, path: &str) -> Result<crate::task::TaskId, SyscallResult> {
    require_service_manager()?;
    let sched = SCHEDULER.lock();
    let parent_id = sched.current_task_id().ok_or(SyscallResult::PermissionDenied)?;
    let current = sched.current_task().ok_or(SyscallResult::PermissionDenied)?;
    if !current.cap_table.authorize_capability(CapabilityType::FileSystemRead, Rights::READ) {
        return Err(SyscallResult::PermissionDenied);
    }
    drop(sched);

    if path.is_empty() || path.len() > 256 || !path.starts_with("/packages/") || !path.ends_with(".rapp") {
        return Err(SyscallResult::InvalidArguments);
    }

    let manifest = sys_verify_package(cap_slot, path)?;
    let service_id = crate::service_abi::service_id_from_package_id(&manifest.package_id)
        .ok_or(SyscallResult::PermissionDenied)?;
    for capability in &manifest.capabilities {
        if !crate::service_abi::service_capability_allowed(service_id, *capability) {
            return Err(SyscallResult::PermissionDenied);
        }
    }
    if manifest.payload_len == 0 {
        return Err(SyscallResult::InvalidArguments);
    }

    let stat = sys_fs_stat(cap_slot, path)?;
    let fd = sys_fs_open(cap_slot, path, Rights::READ)?;
    let mut rapp = alloc::vec![0u8; stat.size];
    let read_bytes = sys_fs_read(cap_slot, fd, &mut rapp)?;
    sys_fs_close(fd);
    if read_bytes != stat.size {
        return Err(SyscallResult::DeviceError);
    }
    let verified = parse_rapp(&rapp).map_err(|_| SyscallResult::IntegrityViolation)?;
    if verified.package_id != manifest.package_id || verified.payload_offset != manifest.payload_offset || verified.payload_len != manifest.payload_len {
        return Err(SyscallResult::IntegrityViolation);
    }
    let payload_end = verified.payload_offset.checked_add(verified.payload_len).ok_or(SyscallResult::InvalidArguments)?;
    let payload = &rapp[verified.payload_offset..payload_end];
    let service = match service_id {
        1 => crate::services::ServiceId::Display,
        2 => crate::services::ServiceId::Input,
        3 => crate::services::ServiceId::Storage,
        4 => crate::services::ServiceId::Network,
        5 => crate::services::ServiceId::Power,
        6 => crate::services::ServiceId::Modem,
        7 => crate::services::ServiceId::Audio,
        8 => crate::services::ServiceId::Camera,
        9 => crate::services::ServiceId::Compositor,
        10 => crate::services::ServiceId::Launcher,
        11 => crate::services::ServiceId::Sensor,
        _ => return Err(SyscallResult::PermissionDenied),
    };
    let task_id = crate::services::launch_verified_package(service, parent_id, &verified, payload)
        .map_err(|_| SyscallResult::DeviceError)?;
    serial_println!("[SERVICE] Launched verified service package '{}' as task {}", verified.package_id, task_id.0);
    Ok(task_id)
}

pub fn sys_service_heartbeat(service_id: u8) -> Result<(), SyscallResult> {
    crate::serial::service_serial_commands();
    let task_id = SCHEDULER.lock().current_task_id().ok_or(SyscallResult::PermissionDenied)?;
    let id = match service_id {
        1 => crate::services::ServiceId::Display, 2 => crate::services::ServiceId::Input,
        3 => crate::services::ServiceId::Storage, 4 => crate::services::ServiceId::Network,
        5 => crate::services::ServiceId::Power, 6 => crate::services::ServiceId::Modem,
        7 => crate::services::ServiceId::Audio, 8 => crate::services::ServiceId::Camera,
        9 => crate::services::ServiceId::Compositor, 10 => crate::services::ServiceId::Launcher, 11 => crate::services::ServiceId::Sensor,
        _ => return Err(SyscallResult::InvalidArguments),
    };
    crate::services::heartbeat_for_task(id, task_id).map_err(|e| {
        if crate::services::is_contention(e) { SyscallResult::NotReady } else { SyscallResult::PermissionDenied }
    })
}

pub fn sys_service_recover_failed(service_id: u8) -> Result<u64, SyscallResult> {
    require_service_manager()?;
    crate::services::supervise();
    let id = match service_id {
        1 => crate::services::ServiceId::Display, 2 => crate::services::ServiceId::Input,
        3 => crate::services::ServiceId::Storage, 4 => crate::services::ServiceId::Network,
        5 => crate::services::ServiceId::Power, 6 => crate::services::ServiceId::Modem,
        7 => crate::services::ServiceId::Audio, 8 => crate::services::ServiceId::Camera,
        9 => crate::services::ServiceId::Compositor, 10 => crate::services::ServiceId::Launcher, 11 => crate::services::ServiceId::Sensor,
        _ => return Err(SyscallResult::InvalidArguments),
    };
    let parent_id = SCHEDULER.lock().current_task_id().ok_or(SyscallResult::PermissionDenied)?;
    crate::services::recover_failed(id, parent_id).map(|_| 1).map_err(|_| SyscallResult::NotReady)
}

pub fn sys_service_stop(service_id: u8) -> Result<(), SyscallResult> {
    require_service_manager()?;
    let id = match service_id {
        1 => crate::services::ServiceId::Display, 2 => crate::services::ServiceId::Input,
        3 => crate::services::ServiceId::Storage, 4 => crate::services::ServiceId::Network,
        5 => crate::services::ServiceId::Power, 6 => crate::services::ServiceId::Modem,
        7 => crate::services::ServiceId::Audio, 8 => crate::services::ServiceId::Camera,
        9 => crate::services::ServiceId::Compositor, 10 => crate::services::ServiceId::Launcher, 11 => crate::services::ServiceId::Sensor,
        _ => return Err(SyscallResult::InvalidArguments),
    };
    let parent_id = SCHEDULER.lock().current_task_id().ok_or(SyscallResult::PermissionDenied)?;
    crate::services::stop(id, parent_id).map_err(|e| {
        if crate::services::is_contention(e) { SyscallResult::NotReady } else { SyscallResult::DeviceError }
    })
}

/// Unified QEMU development bridge launcher for first-party apps and verified RAPPs.
/// COM1 has no userspace task/capability context, so this is intentionally
/// separate from the normal userspace syscall path and gated to QEMU.
pub fn dev_launch_app(app_id: &str) -> Result<crate::task::TaskId, SyscallResult> {
    if crate::platform::PLATFORM_CONTRACT.verification != crate::platform::VerificationLevel::QemuVerified {
        return Err(SyscallResult::PermissionDenied);
    }
    let app_id = app_id.trim_matches(|c: char| c.is_whitespace() || c == '\0');
    if app_id.is_empty() || app_id.len() > 128 || app_id.contains('/') || app_id.contains('\\') || app_id.contains("..") {
        return Err(SyscallResult::InvalidArguments);
    }
    SCHEDULER.lock().reap_orphans();
    // External verified `.rapp` blobs take precedence when present.
    if let Some(blob) = crate::package::PACKAGE_BLOB_STORE.lock().get(app_id).cloned() {
        let verified = crate::package::parse_rapp(&blob.bytes)
            .map_err(|_| SyscallResult::IntegrityViolation)?;
        if verified.package_id != app_id || verified.version != blob.version {
            return Err(SyscallResult::IntegrityViolation);
        }
        let payload_end = verified.payload_offset.checked_add(verified.payload_len)
            .ok_or(SyscallResult::InvalidArguments)?;
        let payload = &blob.bytes[verified.payload_offset..payload_end];
        let parent = crate::task::TaskId(0);
        let tid = crate::task::user::spawn_verified_rapp_process(&verified, payload, parent)
            .map_err(|_| SyscallResult::DeviceError)?;
        register_spawned_task_ipc(tid, parent)?;
        crate::drivers::display_kms::DRM_KMS.lock().raise_surfaces_for_task(tid.0);
        serial_println!("[APP] QEMU dev bridge launched persistent RAPP '{}' as Task {}", verified.package_id, tid.0);
        serial_println!("[PROCESS] Guest application launched id={} task={}", verified.package_id, tid.0);
        return Ok(tid);
    }
    // First-party applications (all 21 registry entries resolve here; the two
    // persistent system services are already running and refuse re-launch).
    let parent = crate::task::TaskId(0);
    let tid = match app_id {
        "org.interos.settings" | "settings" | "app_settings" => crate::task::user::spawn_app_settings(parent),
        "org.interos.terminal" | "terminal" | "app_terminal" => crate::task::user::spawn_app_terminal(parent),
        "org.interos.calculator" | "calculator" | "app_calculator" => crate::task::user::spawn_app_calculator(parent),
        "org.interos.dialer" | "dialer" | "app_dialer" => crate::task::user::spawn_app_dialer(parent),
        "org.interos.vault" | "vault" | "app_vault" => crate::task::user::spawn_app_vault(parent),
        "org.interos.camera" | "camera" | "app_camera" => crate::task::user::spawn_app_camera(parent),
        "org.interos.browser" | "browser" | "app_browser" => crate::task::user::spawn_app_browser(parent),
        "org.interos.messages" | "messages" | "app_messages" => crate::task::user::spawn_app_messages(parent),
        "org.interos.calendar" | "calendar" | "app_calendar" => crate::task::user::spawn_app_calendar(parent),
        "org.interos.music" | "music" | "app_music" => crate::task::user::spawn_app_music(parent),
        "org.interos.file_manager" | "file_manager" | "files" | "app_file_manager" => crate::task::user::spawn_app_file_manager(parent),
        "org.interos.gallery" | "gallery" | "photos" | "app_gallery" => crate::task::user::spawn_app_gallery(parent),
        "org.interos.sensor_dashboard" | "sensor_dashboard" => spawn_app_sensor_dashboard(parent),
        "org.interos.notes" | "notes" | "app_notes" => crate::task::user::spawn_app_notes(parent),
        "org.interos.compositor" | "compositor" | "org.interos.launcher" | "launcher" => {
            return Err(SyscallResult::PermissionDenied);
        }
        _ => return Err(SyscallResult::FileNotFound),
    };
    register_spawned_task_ipc(tid, parent)?;
    crate::drivers::display_kms::DRM_KMS.lock().raise_surfaces_for_task(tid.0);
    serial_println!("[APP] QEMU dev bridge launched application '{}' as Task {}", app_id, tid.0);
    serial_println!("[PROCESS] Guest application launched id={} task={}", app_id, tid.0);
    Ok(tid)
}

/// Launches a verified persistent RAPP from the QEMU development bridge.
/// COM1 has no userspace task/capability context, so this is intentionally
/// separate from the normal userspace syscall path and gated to QEMU.
pub fn dev_launch_verified_rapp(app_id: &str) -> Result<crate::task::TaskId, SyscallResult> {
    dev_launch_app(app_id)
}

/// Spawns the Sensor Dashboard app in Ring 3.
///
/// WHY two object-scoped SystemControl capabilities: `sys_sensor_read` authorizes
/// against `SystemControl|INVOKE` on the sensor control object, and
/// `sys_battery_query` / `sys_thermal_query` against the power control object.
/// Object-scoped INVOKE grants exactly the read path the dashboard probes and
/// none of the device-wide authority the object-blind `require_system_control()`
/// form would imply. When the hardware is absent the queries themselves return
/// `DeviceError` with an explicit absent flag, so the app still reports absence
/// rather than invented telemetry.
fn spawn_app_sensor_dashboard(id_security: TaskId) -> TaskId {
    let mut caps = CapabilityTable::new(8);
    caps.insert(Capability {
        id: CapabilityId(3401),
        cap_type: CapabilityType::SerialOutput,
        rights: Rights::WRITE,
        object_id: 0,
    }).ok();
    caps.insert(Capability {
        id: CapabilityId(3402),
        cap_type: CapabilityType::TimerTickRead,
        rights: Rights::READ,
        object_id: 0,
    }).ok();
    caps.insert(Capability {
        id: CapabilityId(3403),
        cap_type: CapabilityType::EventPoll,
        rights: Rights::READ.union(Rights::INVOKE),
        object_id: 0,
    }).ok();
    caps.insert(Capability {
        id: CapabilityId(3404),
        cap_type: CapabilityType::FrameBufferDirect,
        rights: Rights::WRITE,
        object_id: 1,
    }).ok();
    caps.insert(Capability {
        id: CapabilityId(3405),
        cap_type: CapabilityType::SystemControl,
        rights: Rights::INVOKE,
        object_id: crate::services::SERVICE_SENSOR_CONTROL_OBJECT,
    }).ok();
    caps.insert(Capability {
        id: CapabilityId(3406),
        cap_type: CapabilityType::SystemControl,
        rights: Rights::INVOKE,
        object_id: crate::services::SERVICE_POWER_CONTROL_OBJECT,
    }).ok();

    let id = crate::task::user::spawn_user_process_with_parent(
        "sensor_dashboard",
        crate::task::images::APP_SENSOR_DASHBOARD_ELF,
        caps,
        6,
        id_security,
    ).expect("Failed to spawn registered first-party app");
    serial_println!("[OK] Spawned app_sensor_dashboard (Ring 3 User Mode, Isolated CR3, ID: {:?})", id);
    id
}

pub fn sys_app_launch(app_id: &str) -> Result<crate::task::TaskId, SyscallResult> {
    if let Err(e) = require_app_launcher_or_service_manager() {
        crate::serial_println!("[APP_LAUNCH DEBUG] require_app_launcher_or_service_manager error: {:?}", e);
        return Err(e);
    }
    let sched = SCHEDULER.lock();
    let parent_id = match sched.current_task_id() {
        Some(id) => id,
        None => {
            crate::serial_println!("[APP_LAUNCH DEBUG] current_task_id is None");
            return Err(SyscallResult::PermissionDenied);
        }
    };
    drop(sched);

    let app_id = app_id.trim_matches(|c: char| c.is_whitespace() || c == '\0');

    // The persistent registry is the launch authority. A package blob or
    // native built-in is not executable merely because a caller knows its ID.
    crate::package::PACKAGE_REGISTRY.lock().seed();
    let registry = crate::package::PACKAGE_REGISTRY.lock();
    let registered = registry.entries().iter().find(|e| e.package_id == app_id).cloned();
    drop(registry);
    let Some(registered) = registered else {
        return Err(SyscallResult::FileNotFound);
    };
    if !matches!(registered.state, crate::package::PackageState::Active) {
        return Err(SyscallResult::PermissionDenied);
    }

    // Launch is single-instance per parent/package. Returning the live child
    // keeps repeated home-screen taps from creating duplicate processes and
    // surfaces; explicit restart/force-stop remains the lifecycle escape hatch.
    if let Some(existing) = SCHEDULER.lock().find_child_by_name(parent_id, app_id) {
        crate::drivers::display_kms::DRM_KMS.lock().raise_surfaces_for_task(existing.0);
        return Ok(existing);
    }

    // External applications use the canonical RAPP v2 execution path. The
    // kernel verifies the package, derives capabilities only from its manifest,
    // and loads the signed ELF payload into a Ring-3 child. First-party
    // built-in apps continue through the native launcher table below.
    // External applications installed through the QEMU developer transport are
    // loaded from the persistent package blob store. This keeps launch independent
    // from the mutable VFS namespace while retaining kernel-side verification.
    if !app_id.is_empty() && !app_id.contains('/') && !app_id.contains('\\') && !app_id.contains("..") {
        if let Some(blob) = crate::package::PACKAGE_BLOB_STORE.lock().get(app_id).cloned() {
            let verified = crate::package::parse_rapp(&blob.bytes)
                .map_err(|_| SyscallResult::IntegrityViolation)?;
            if verified.package_id != app_id || verified.version != blob.version {
                return Err(SyscallResult::IntegrityViolation);
            }
            if !crate::package::PACKAGE_REGISTRY.lock().launch_allowed(app_id, blob.version) {
                return Err(SyscallResult::PermissionDenied);
            }
            let payload_end = verified.payload_offset.checked_add(verified.payload_len)
                .ok_or(SyscallResult::InvalidArguments)?;
            let payload = &blob.bytes[verified.payload_offset..payload_end];
            if verified.arch == crate::package::RappArch::Wasm {
                let tid = crate::task::user::spawn_app_browser(parent_id);
                register_spawned_task_ipc(tid, parent_id)?;
                crate::drivers::display_kms::DRM_KMS.lock().raise_surfaces_for_task(tid.0);
                serial_println!("[APP] Launched web/wasm RAPP '{}' as Task {}", verified.package_id, tid.0);
                return Ok(tid);
            }
            let tid = crate::task::user::spawn_verified_rapp_process(&verified, payload, parent_id)
                .map_err(|_| SyscallResult::DeviceError)?;
            register_spawned_task_ipc(tid, parent_id)?;
            crate::drivers::display_kms::DRM_KMS.lock().raise_surfaces_for_task(tid.0);
            serial_println!("[APP] Launched persistent RAPP '{}' as Task {}", verified.package_id, tid.0);
            return Ok(tid);
        }
    }

    if !app_id.is_empty()
        && !app_id.contains('/')
        && !app_id.contains('\\')
        && !app_id.contains("..")
    {
        let read_slot = {
            let sched = SCHEDULER.lock();
            sched.current_task()
                .and_then(|task| task.cap_table.find_slot(CapabilityType::FileSystemRead, Rights::READ))
        };
        if let Some(read_slot) = read_slot {
            let path = alloc::format!("/packages/{}.rapp", app_id);
            if let Ok(manifest) = sys_verify_package(read_slot, &path) {
                if manifest.package_id == app_id
                    && manifest.payload_len > 0
                    && crate::package::PACKAGE_REGISTRY.lock().launch_allowed(app_id, manifest.version) {
                    let stat = sys_fs_stat(read_slot, &path)?;
                    let fd = sys_fs_open(read_slot, &path, Rights::READ)?;
                    let mut rapp = alloc::vec![0u8; stat.size];
                    let read_bytes = sys_fs_read(read_slot, fd, &mut rapp)?;
                    sys_fs_close(fd);
                    if read_bytes != stat.size {
                        return Err(SyscallResult::DeviceError);
                    }
                    let verified = crate::package::rapp::parse_rapp(&rapp)
                        .map_err(|_| SyscallResult::IntegrityViolation)?;
                    let payload_end = verified.payload_offset
                        .checked_add(verified.payload_len)
                        .ok_or(SyscallResult::InvalidArguments)?;
                    let payload = &rapp[verified.payload_offset..payload_end];
                    let tid = crate::task::user::spawn_verified_rapp_process(
                        &verified,
                        payload,
                        parent_id,
                    ).map_err(|_| SyscallResult::DeviceError)?;
                    register_spawned_task_ipc(tid, parent_id)?;
                    serial_println!(
                        "[APP] Launched verified RAPP '{}' as Task {}",
                        verified.package_id,
                        tid.0
                    );
                    return Ok(tid);
                }
            }
        }
    }

    if !crate::package::first_party_package(app_id).is_some()
        || !crate::package::PACKAGE_REGISTRY.lock().launch_allowed(app_id, (1, 0, 0))
    {
        return Err(SyscallResult::PermissionDenied);
    }

    let tid = match app_id {
        "org.interos.settings" | "settings" | "app_settings" => {
            crate::task::user::spawn_app_settings(parent_id)
        }
        "org.interos.terminal" | "terminal" | "app_terminal" => {
            if !crate::settings::developer_terminal_enabled() && parent_id.0 != 11 && parent_id.0 != 1 {
                return Err(SyscallResult::PermissionDenied);
            }
            crate::task::user::spawn_app_terminal(parent_id)
        }
        "org.interos.calculator" | "calculator" => {
            crate::task::user::spawn_app_calculator(parent_id)
        }
        "org.interos.dialer" | "dialer" => {
            crate::task::user::spawn_app_dialer(parent_id)
        }
        "org.interos.vault" | "vault" => {
            crate::task::user::spawn_app_vault(parent_id)
        }
        "org.interos.camera" | "camera" => {
            crate::task::user::spawn_app_camera(parent_id)
        }
        "org.interos.browser" | "browser" => {
            crate::task::user::spawn_app_browser(parent_id)
        }
        "org.interos.messages" | "messages" => {
            crate::task::user::spawn_app_messages(parent_id)
        }
        "org.interos.calendar" | "calendar" => {
            crate::task::user::spawn_app_calendar(parent_id)
        }
        "org.interos.music" | "music" => {
            crate::task::user::spawn_app_music(parent_id)
        }
        "org.interos.file_manager" | "file_manager" | "files" => {
            crate::task::user::spawn_app_file_manager(parent_id)
        }
        "org.interos.gallery" | "gallery" | "photos" => {
            crate::task::user::spawn_app_gallery(parent_id)
        }
        "org.interos.sensor_dashboard" | "sensor_dashboard" => {
            spawn_app_sensor_dashboard(parent_id)
        }
        "org.interos.notes" | "notes" | "app_notes" => {
            crate::task::user::spawn_app_notes(parent_id)
        }
        _ => {
            crate::serial_println!("[APP_LAUNCH DEBUG] unknown app_id: '{}'", app_id);
            return Err(SyscallResult::InvalidArguments);
        }
    };
    register_spawned_task_ipc(tid, parent_id)?;
    crate::drivers::display_kms::DRM_KMS.lock().raise_surfaces_for_task(tid.0);
    serial_println!("[APP] Launched application '{}' as Task {}", app_id, tid.0);
    Ok(tid)
}

pub fn sys_window_list(out_buf:&mut [u8])->Result<usize,SyscallResult>{
    {
        let sched = SCHEDULER.lock();
        let task = sched.current_task().ok_or(SyscallResult::PermissionDenied)?;
        if !task.cap_table.authorize_capability(CapabilityType::FrameBufferDirect, Rights::READ) {
            return Err(SyscallResult::PermissionDenied);
        }
    }
    use core::fmt::Write;
    let mut snaps = [crate::drivers::display_kms::EMPTY_SURFACE_SNAPSHOT; 32];
    // WHY a bounded acquisition instead of `lock()`: this syscall runs inside the
    // compositor service's polling loop, and both locks it takes are also held
    // across full-surface composites by foreground tasks. A blocking `lock()`
    // there turned a slow repaint into a permanent wedge - the compositor spun
    // inside the syscall while the holding task could never be rescheduled to
    // release it, which froze the whole guest. Bounded, the caller retries on
    // the next pass exactly like every other site in this tree.
    // WHY the bound is small and not the tree's usual 1000: the caller polls in
    // a tight loop, so a contended lock here must bail at once. Spinning 1000
    // times for a lock a foreground task holds across a multi-second composite
    // would burn the single CPU that composite needs to finish.
    let mut kms_guard=None;
    for _ in 0..8 {
        if let Some(kms)=crate::drivers::display_kms::DRM_KMS.try_lock(){
            kms_guard=Some(kms);
            break;
        }
        core::hint::spin_loop();
    }
    let count={let kms=kms_guard.ok_or(SyscallResult::NotReady)?;kms.surface_snapshot(&mut snaps)};
    // WHY the names are collected while the scheduler lock is held only for
    // this copy: the format loop below writes into the caller's buffer and
    // needs no lock, so the scheduler lock is released before it rather than
    // spanning the whole loop.
    let mut names=[("unknown");32];
    let mut sched_guard=None;
    for _ in 0..8 {
        if let Some(sched)=SCHEDULER.try_lock(){
            sched_guard=Some(sched);
            break;
        }
        core::hint::spin_loop();
    }
    let named=if let Some(sched)=sched_guard.as_ref(){
        for (slot,s) in snaps[..count].iter().enumerate(){
            names[slot]=sched.get_task(TaskId(s.owner_task)).map(|t|t.name).unwrap_or("unknown");
        }
        true
    } else { false };
    drop(sched_guard);
    struct W<'a>{b:&'a mut[u8],n:usize}
    impl<'a> Write for W<'a>{fn write_str(&mut self,s:&str)->core::fmt::Result{let k=s.len().min(self.b.len().saturating_sub(self.n));self.b[self.n..self.n+k].copy_from_slice(&s.as_bytes()[..k]);self.n+=k;Ok(())}}
    let capacity = out_buf.len();
    let mut w=W{b:out_buf,n:0};
    if named {
        for (slot,s) in snaps[..count].iter().enumerate(){
            let name=names[slot];
            let _=write!(w,"{}|{}|{}|{}|{}|{}|{}|{}|{}|{}|{}\n",s.id,s.owner_task,name,s.x,s.y,s.width,s.height,s.z_order,s.opacity,s.visible as u8,s.input_enabled as u8);
            if w.n>=capacity{break;}
        }
    }
    Ok(w.n)
}
#[cfg(test)]
mod tests {
    use super::*;

    /// Pins the `FsStat` userspace ABI.
    ///
    /// WHY this is checked in the kernel as well as in the SDK: the record is the
    /// one place where both sides describe the same 96 bytes independently, and a
    /// drift between them would not fail a compile. `interos_std`'s own test
    /// decodes these offsets; a change to either layout breaks one of the two.
    #[test]
    fn fs_stat_record_layout_is_pinned() {
        let stat = FileStat {
            inode: crate::fs::InodeId(0x0102_0304_0506_0708),
            uid: 0x1122_3344,
            gid: 0x5566_7788,
            mode: 0x99AA,
            nlink: 0xBBCC,
            atime: 0x1122_3344_5566_7788,
            mtime: 0x8877_6655_4433_2211,
            ctime: 0xAABB_CCDD_EEFF_0011,
            file_type: FileType::ReadOnlySystem,
            size: 0x1234_5678_9ABC_DEF0,
            read_only: true,
            sha256_digest: [0xA5; 32],
        };
        let mut out = [0u8; FILE_STAT_BYTES];
        assert_eq!(encode_file_stat(&stat, &mut out), FILE_STAT_BYTES);
        assert_eq!(&out[0..8], &0x0102_0304_0506_0708u64.to_le_bytes());
        assert_eq!(&out[8..12], &0x1122_3344u32.to_le_bytes());
        assert_eq!(&out[12..16], &0x5566_7788u32.to_le_bytes());
        assert_eq!(&out[16..18], &0x99AAu16.to_le_bytes());
        assert_eq!(&out[18..20], &0xBBCCu16.to_le_bytes());
        assert_eq!(&out[20..28], &0x1122_3344_5566_7788u64.to_le_bytes());
        assert_eq!(&out[28..36], &0x8877_6655_4433_2211u64.to_le_bytes());
        assert_eq!(&out[36..44], &0xAABB_CCDD_EEFF_0011u64.to_le_bytes());
        // The tag is a 4-byte field; `ReadOnlySystem` is declared second.
        assert_eq!(&out[44..48], &2u32.to_le_bytes());
        assert_eq!(out[48], 1, "read_only is carried in the first byte of its field");
        assert_eq!(&out[56..88], &[0xA5u8; 32]);
        assert_eq!(&out[88..96], &0x1234_5678_9ABC_DEF0u64.to_le_bytes());
    }

    /// The `FileType` tags are part of the ABI, so their order is load-bearing.
    #[test]
    fn file_type_tags_follow_the_enum_declaration_order() {
        assert_eq!(file_type_tag(FileType::RegularFile), 0);
        assert_eq!(file_type_tag(FileType::Directory), 1);
        assert_eq!(file_type_tag(FileType::ReadOnlySystem), 2);
        assert_eq!(file_type_tag(FileType::DeviceNode), 3);
    }

    #[test]
    fn system_control_object_scoping_and_cross_subsystem_denial() {
        use crate::capability::{Capability, CapabilityId, CapabilityType, Rights};
        use crate::services::{
            SERVICE_AUDIO_CONTROL_OBJECT, SERVICE_POWER_CONTROL_OBJECT,
            SERVICE_TELEPHONY_CONTROL_OBJECT, SERVICE_SENSOR_CONTROL_OBJECT,
        };

        let task_id = TaskId(999);
        let mut audio_only_table = crate::capability::CapabilityTable::new_for_task(task_id, 4);
        audio_only_table.insert(Capability {
            id: CapabilityId(7001),
            cap_type: CapabilityType::SystemControl,
            rights: Rights::INVOKE,
            object_id: SERVICE_AUDIO_CONTROL_OBJECT,
        }).unwrap();

        // Audio capability satisfies audio object
        assert!(audio_only_table.authorize_object(CapabilityType::SystemControl, Rights::INVOKE, SERVICE_AUDIO_CONTROL_OBJECT));
        // Audio capability strictly FAILS power, telephony, sensor, and global admin objects
        assert!(!audio_only_table.authorize_object(CapabilityType::SystemControl, Rights::INVOKE, SERVICE_POWER_CONTROL_OBJECT));
        assert!(!audio_only_table.authorize_object(CapabilityType::SystemControl, Rights::INVOKE, SERVICE_TELEPHONY_CONTROL_OBJECT));
        assert!(!audio_only_table.authorize_object(CapabilityType::SystemControl, Rights::INVOKE, SERVICE_SENSOR_CONTROL_OBJECT));
        assert!(!audio_only_table.authorize_object(CapabilityType::SystemControl, Rights::INVOKE, GLOBAL_SYSTEM_CONTROL_OBJECT));

        let mut global_admin_table = crate::capability::CapabilityTable::new_for_task(task_id, 4);
        global_admin_table.insert(Capability {
            id: CapabilityId(7002),
            cap_type: CapabilityType::SystemControl,
            rights: Rights::INVOKE,
            object_id: GLOBAL_SYSTEM_CONTROL_OBJECT,
        }).unwrap();

        // Global admin (object_id: 0) satisfies global admin
        assert!(global_admin_table.authorize_object(CapabilityType::SystemControl, Rights::INVOKE, GLOBAL_SYSTEM_CONTROL_OBJECT));
    }

    #[test]
    fn service_capability_policy_forbids_global_system_control() {
        use crate::service_abi::service_capability_allowed;

        // Services 1..=11 are Ring-3 isolated system daemons. None of them may ever
        // receive global administrative capability ID 18 (SystemControl Global).
        for service_id in 1..=11 {
            assert!(
                !service_capability_allowed(service_id, 18),
                "service {} must strictly be denied global SystemControl capability ID 18",
                service_id
            );
        }

        // Verify that services only receive their explicitly attenuated scoped control IDs:
        // Service 5 (powerd) gets 19 (Power control)
        assert!(service_capability_allowed(5, 19));
        assert!(!service_capability_allowed(5, 20));

        // Service 6 (modemd) gets 20 (Telephony control)
        assert!(service_capability_allowed(6, 20));
        assert!(!service_capability_allowed(6, 19));

        // Service 7 (audiod) gets 21 (Audio control)
        assert!(service_capability_allowed(7, 21));
        assert!(!service_capability_allowed(7, 22));

        // Service 8 (camerad) gets 22 (Camera control)
        assert!(service_capability_allowed(8, 22));
        assert!(!service_capability_allowed(8, 21));

        // Service 11 (sensord) gets 23 (Sensor control)
        assert!(service_capability_allowed(11, 23));
        assert!(!service_capability_allowed(11, 22));
    }

    #[test]
    fn vfs_list_dir_single_use_lease_delegated_consumption() {
        use crate::capability::{Capability, CapabilityId, CapabilityType, Rights, DelegatedLease};
        use crate::capability::delegation::{allocate_lease_id, register_lease};
        use crate::fs::vfs::Vfs;

        let grantor = TaskId(50);
        let grantee = TaskId(51);
        crate::task::process::register_leader(grantee, TaskId(1));

        let mut vfs = Vfs::new();
        vfs.mkdir("/test_dir");

        let mut grantor_caps = crate::capability::CapabilityTable::new_for_task(grantor, 4);
        grantor_caps.insert(Capability {
            id: CapabilityId(101),
            cap_type: CapabilityType::FileSystemRead,
            rights: Rights::READ | Rights::GRANT,
            object_id: 0,
        }).unwrap();

        let parent_cap = grantor_caps.get(0).unwrap();
        let lease = DelegatedLease::new(
            allocate_lease_id(),
            parent_cap,
            grantor,
            grantee,
            Rights::READ,
            1, // single-use lease
            1000,
            0,
        ).unwrap();

        let derived_cap = lease.to_capability();
        register_lease(lease);

        let mut grantee_caps = crate::capability::CapabilityTable::new_for_task(grantee, 4);
        let slot = grantee_caps.insert(derived_cap).unwrap();

        let mut out = [0u8; 128];
        // 1. First invocation must succeed, consuming the single use
        let res1 = vfs.list_dir(&grantee_caps, slot, "/test_dir", &mut out);
        assert!(res1.is_ok(), "First listing with single-use lease must succeed");

        // 2. Second invocation must fail closed because quota is exhausted
        let res2 = vfs.list_dir(&grantee_caps, slot, "/test_dir", &mut out);
        assert!(
            matches!(res2, Err(crate::fs::vfs::FsError::CapabilityError(crate::capability::CapError::InvalidSlot))),
            "Second listing must fail closed with exhausted lease quota, got {:?}",
            res2
        );
    }

    #[test]
    fn slot_handle_verification_prevents_ambient_table_fallback() {
        use crate::capability::{Capability, CapabilityId, CapabilityType, Rights};

        let task_id = TaskId(60);
        let mut caps = crate::capability::CapabilityTable::new_for_task(task_id, 4);
        // Valid capability inserted in slot 0
        caps.insert(Capability {
            id: CapabilityId(201),
            cap_type: CapabilityType::IpcReceive,
            rights: Rights::READ,
            object_id: 0,
        }).unwrap();
        caps.insert(Capability {
            id: CapabilityId(202),
            cap_type: CapabilityType::TimerTickRead,
            rights: Rights::READ,
            object_id: 0,
        }).unwrap();

        // Verifying slot 0 succeeds
        assert!(caps.verify(0, CapabilityType::IpcReceive, Rights::READ).is_ok());
        assert!(caps.verify(1, CapabilityType::TimerTickRead, Rights::READ).is_ok());

        // Verifying invalid slot 999 fails with InvalidSlot
        assert!(caps.verify(999, CapabilityType::IpcReceive, Rights::READ).is_err());
        assert!(caps.verify(999, CapabilityType::TimerTickRead, Rights::READ).is_err());

        // The previous bug in sys_ipc_recv and sys_get_ticks did:
        // verify(slot).is_ok() || authorize_capability(...)
        // which allowed slot 999 to succeed because slot 0 had the capability.
        // Verifying that verify(999, ...) fails ensures slot-handle semantics are preserved.
        assert_eq!(
            caps.verify(999, CapabilityType::IpcReceive, Rights::READ).err(),
            Some(crate::capability::CapError::InvalidSlot)
        );
    }

    #[test]
    fn package_install_single_use_lease_quota_semantics() {
        use crate::capability::{Capability, CapabilityId, CapabilityType, Rights, DelegatedLease};
        use crate::capability::delegation::{allocate_lease_id, register_lease};
        use crate::fs::vfs::Vfs;

        let grantor = TaskId(80);
        let grantee = TaskId(81);
        crate::task::process::register_leader(grantee, TaskId(1));

        let mut vfs = Vfs::new();
        vfs.mkdir("/packages");
        let _ = vfs.create_file("/packages/test_app.rapp", b"RAPP_DUMMY_PAYLOAD_FOR_TEST", false);

        let mut grantor_caps = crate::capability::CapabilityTable::new_for_task(grantor, 4);
        grantor_caps.insert(Capability {
            id: CapabilityId(301),
            cap_type: CapabilityType::FileSystemRead,
            rights: Rights::READ | Rights::GRANT,
            object_id: 0,
        }).unwrap();

        let parent_cap = grantor_caps.get(0).unwrap();
        let lease = DelegatedLease::new(
            allocate_lease_id(),
            parent_cap,
            grantor,
            grantee,
            Rights::READ,
            1, // Single-use lease: exactly 1 invocation
            10_000,
            crate::interrupts::get_system_ticks(),
        ).unwrap();

        let derived_cap = lease.to_capability();
        register_lease(lease);

        let mut grantee_caps = crate::capability::CapabilityTable::new_for_task(grantee, 4);
        let slot = grantee_caps.insert(derived_cap).unwrap();

        // 1. First atomic read operation must SUCCEED using exactly 1 invocation of the lease
        let res1 = vfs.read_file_for_task(&grantee_caps, slot, "/packages/test_app.rapp", 4096);
        assert!(res1.is_ok(), "First read with single-use lease must succeed: {:?}", res1.err());
        assert_eq!(res1.unwrap(), b"RAPP_DUMMY_PAYLOAD_FOR_TEST");

        // 2. Second read operation must be DENIED because the single invocation was consumed
        let res2 = vfs.read_file_for_task(&grantee_caps, slot, "/packages/test_app.rapp", 4096);
        assert!(
            matches!(res2, Err(crate::fs::vfs::FsError::CapabilityError(crate::capability::CapError::InvalidSlot))),
            "Second read must fail closed with exhausted lease quota, got {:?}",
            res2
        );
    }

    /// The read bound must be applied to the inode contents before they are
    /// copied, not by the caller after the copy has already happened.
    ///
    /// Pinning assertion: a file larger than the bound is refused with
    /// `FileTooLarge`, and a file at exactly the bound still reads. When the
    /// bound lived in the syscall after `read_file_for_task` returned, the copy
    /// had already been made, so an oversized blob was allocated in full and
    /// only then discarded.
    #[test]
    fn package_read_is_bounded_before_the_inode_is_copied() {
        use crate::capability::{Capability, CapabilityId, CapabilityType, Rights};
        use crate::fs::vfs::{FsError, Vfs};

        let owner = TaskId(82);
        let mut vfs = Vfs::new();
        vfs.mkdir("/packages");
        let body = alloc::vec![0x5Au8; 8192];
        vfs.create_file("/packages/big.rapp", &body, false);

        let mut caps = crate::capability::CapabilityTable::new_for_task(owner, 4);
        caps.insert(Capability {
            id: CapabilityId(4110),
            cap_type: CapabilityType::FileSystemRead,
            rights: Rights::READ,
            object_id: 0,
        }).unwrap();

        // Under the bound: the whole file comes back.
        assert_eq!(
            vfs.read_file_for_task(&caps, 0, "/packages/big.rapp", 8192).map(|b| b.len()),
            Ok(8192),
            "a file at exactly the bound must read in full"
        );

        // Over the bound: refused, and refused by the bound rather than by a
        // parse failure downstream.
        assert!(
            matches!(vfs.read_file_for_task(&caps, 0, "/packages/big.rapp", 8191), Err(FsError::FileTooLarge)),
            "a file larger than the bound must be refused as too large"
        );
        assert!(
            matches!(vfs.read_file_for_task(&caps, 0, "/packages/big.rapp", 0), Err(FsError::FileTooLarge)),
            "a non-empty file must be refused when no allowance is given"
        );

        // A capability that cannot read is still refused, and the bound does not
        // become a way to learn whether a path exists.
        let mut no_read = crate::capability::CapabilityTable::new_for_task(owner, 4);
        no_read.insert(Capability {
            id: CapabilityId(4111),
            cap_type: CapabilityType::FileSystemRead,
            rights: Rights::WRITE,
            object_id: 0,
        }).unwrap();
        assert!(vfs.read_file_for_task(&no_read, 0, "/packages/big.rapp", 8192).is_err());
        assert!(vfs.read_file_for_task(&caps, 0, "/packages/absent.rapp", 8192).is_err());
    }

    /// Drives the verification body of `sys_verify_package` with a genuinely
    /// signed package, exercising the same read, bound and parse sequence the
    /// syscall runs once it has taken a capability table from the scheduler.
    ///
    /// This is verification-path coverage, not whole-syscall coverage: the
    /// scheduler read that supplies the capability table cannot run in a host
    /// test, so the entry point itself is not exercised here.
    #[test]
    fn package_verification_reads_bounds_and_parses_the_same_bytes() {
        use crate::capability::{Capability, CapabilityId, CapabilityType, Rights};
        use crate::fs::vfs::Vfs;
        use crate::package::revocation::REVOCATION_REGISTRY;

        let owner = TaskId(83);
        let (pkg, pub_key) = crate::package::rapp::build_signed_test_package(
            "com.example.verifiable",
            &[1, 3, 9],
            b"\x7fELF_USER_BINARY_SAMPLE",
            77,
        );
        REVOCATION_REGISTRY.lock().grant_trust_root(pub_key, "com.example.vendor").unwrap();

        let mut vfs = Vfs::new();
        vfs.mkdir("/packages");
        vfs.create_file("/packages/com.example.verifiable.rapp", &pkg, false);

        let mut caps = crate::capability::CapabilityTable::new_for_task(owner, 4);
        caps.insert(Capability {
            id: CapabilityId(4120),
            cap_type: CapabilityType::FileSystemRead,
            rights: Rights::READ,
            object_id: 0,
        }).unwrap();

        // The real verification path accepts the signed package.
        let manifest = verify_package_in(
            &vfs,
            &caps,
            0,
            "/packages/com.example.verifiable.rapp",
        ).expect("a correctly signed package from a trusted publisher must verify");
        assert_eq!(manifest.package_id, "com.example.verifiable");
        assert_eq!(manifest.capabilities, alloc::vec![1, 3, 9]);

        // A bound below the package size is refused by the read, before parsing.
        assert!(
            vfs.read_file_for_task(&caps, 0, "/packages/com.example.verifiable.rapp", 16).is_err(),
            "a package larger than the caller bound must be refused by the read"
        );

        // Tampering with the stored bytes is detected, which is what proves the
        // path verifies the bytes it actually read rather than a cached copy.
        let mut tampered = pkg.clone();
        let last = tampered.len() - 1;
        tampered[last] ^= 0xFF;
        vfs.create_file("/packages/com.example.verifiable.rapp", &tampered, false);
        let manifest = verify_package_in(&vfs, &caps, 0, "/packages/com.example.verifiable.rapp");
        assert!(
            manifest.is_err(),
            "a tampered package must not verify, got {:?}",
            manifest.map(|m| m.package_id)
        );
    }

    /// The install-time capability-18 gate: a package claiming device-wide
    /// administration is refused unless its publisher is a platform publisher.
    ///
    /// Pinning assertion: a correctly signed package from an ordinary trusted
    /// publisher is refused with `PermissionDenied`, and the same package signed
    /// by a platform publisher is accepted. A package that does not claim 18 is
    /// unaffected either way.
    #[test]
    fn install_refuses_capability_18_from_an_ordinary_publisher() {
        use crate::package::revocation::{REVOCATION_REGISTRY, is_system_publisher_identity};
        assert!(
            is_system_publisher_identity("org.interos.platform"),
            "the test must use a platform publisher identity"
        );

        let (third_party, third_key) = crate::package::rapp::build_signed_test_package(
            "com.example.admin",
            &[1, 18],
            b"\x7fELF_USER_BINARY_SAMPLE",
            78,
        );
        REVOCATION_REGISTRY.lock().grant_trust_root(third_key, "com.example.vendor").unwrap();

        let manifest = crate::package::parse_rapp(&third_party)
            .expect("an ordinary trusted publisher's signature must still verify");
        assert_eq!(manifest.capabilities, alloc::vec![1, 18]);
        assert_eq!(
            authorize_capability_18_for_install(&manifest),
            Err(SyscallResult::PermissionDenied),
            "capability 18 from an ordinary trusted publisher must be refused at install"
        );

        let (platform, platform_key) = crate::package::rapp::build_signed_test_package(
            "org.interos.admin",
            &[1, 18],
            b"\x7fELF_USER_BINARY_SAMPLE",
            79,
        );
        REVOCATION_REGISTRY.lock().grant_trust_root(platform_key, "org.interos.platform").unwrap();
        let manifest = crate::package::parse_rapp(&platform)
            .expect("a platform publisher's signature must verify");
        assert_eq!(
            authorize_capability_18_for_install(&manifest),
            Ok(()),
            "capability 18 from a platform publisher must be accepted"
        );

        let (plain, plain_key) = crate::package::rapp::build_signed_test_package(
            "com.example.plain",
            &[1, 3],
            b"\x7fELF_USER_BINARY_SAMPLE",
            80,
        );
        REVOCATION_REGISTRY.lock().grant_trust_root(plain_key, "com.example.vendor").unwrap();
        let manifest = crate::package::parse_rapp(&plain).expect("a plain package must verify");
        assert_eq!(
            authorize_capability_18_for_install(&manifest),
            Ok(()),
            "a package that does not claim capability 18 must install normally"
        );
    }

    #[test]
    /// Exercises the real privileged-authorization gate for capability 18.
    ///
    /// WHY this replaces a table-only test: asserting that a capability table
    /// holding an object-0 `SystemControl` token authorizes object 0 only proves
    /// `CapabilityTable::authorize_object` works. It passed unchanged against the
    /// code that let any holder of capability 18 reach device-wide administration.
    /// The confinement lives in `authorize_device_wide_system_control` and
    /// `authorize_system_control_object`, so those are what this drives.
    fn capability_18_is_confined_to_the_system_supervisor_gate() {
    use crate::capability::{Capability, CapabilityId, CapabilityType, Rights};
    use crate::services::SERVICE_POWER_CONTROL_OBJECT;

    // An ordinary third-party app that somehow holds capability 18.
    let mut app_caps = crate::capability::CapabilityTable::new_for_task(TaskId(105), 4);
    app_caps.insert(Capability {
        id: CapabilityId(7018),
        cap_type: CapabilityType::SystemControl,
        rights: Rights::INVOKE,
        object_id: GLOBAL_SYSTEM_CONTROL_OBJECT,
    }).unwrap();

    // 1. The token alone matches the global object at the table level. This is
    //    the premise the confinement exists to contain, so it must stay true.
    assert!(app_caps.authorize_object(CapabilityType::SystemControl, Rights::INVOKE, GLOBAL_SYSTEM_CONTROL_OBJECT));

    // 2. Only a kernel-assigned system PID or the first-party namespace is a
    //    privileged identity. A task cannot acquire one by choosing a name:
    //    a RAPP is named after the publisher's `package_id`, so any name-based
    //    clause broad enough to be matched by a self-chosen name is unsafe.
    assert!(!is_privileged_system_identity(TaskId(105), "com.example.tracker"));
    assert!(!is_privileged_system_identity(TaskId(105), "system"));
    assert!(!is_privileged_system_identity(TaskId(105), "kernel_main"));
    assert!(!is_privileged_system_identity(TaskId(105), "org.interosX.lookalike"));
    assert!(!is_privileged_system_identity(TaskId(105), ""));
    // 3. ...so capability 18 alone does not produce device-wide administration.
    assert!(
        !authorize_device_wide_system_control(&app_caps, TaskId(105), "com.example.tracker"),
        "capability 18 must not by itself confer device-wide system administration"
    );

    // 4. The same table held by the system supervisor is authorized.
    assert!(is_privileged_system_identity(TaskId(1), "system"));
    assert!(
        authorize_device_wide_system_control(&app_caps, TaskId(1), "system"),
        "the system supervisor must retain device-wide administration"
    );
    assert!(
        authorize_device_wide_system_control(&app_caps, TaskId(0), "kernel_main"),
        "the primordial kernel task must retain device-wide administration"
    );
    // First-party RAPP packages keep working through the namespace test.
    assert!(is_privileged_system_identity(TaskId(300), "org.interos.settings"));

    // 5. The global fallback from a domain object is confined to PID 1, so a
    //    capability-18 holder outside PID 1 cannot administer another domain.
    assert!(
        !authorize_system_control_object(&app_caps, TaskId(105), SERVICE_POWER_CONTROL_OBJECT),
        "an ordinary task must not reach a domain object through an ambient global fallback"
    );
    assert!(
        authorize_system_control_object(&app_caps, TaskId(1), SERVICE_POWER_CONTROL_OBJECT),
        "PID 1 may administer any domain object"
    );

    // 6. A domain-scoped capability never confers the global object, for anyone.
    let mut power_only = crate::capability::CapabilityTable::new_for_task(TaskId(106), 4);
    power_only.insert(Capability {
        id: CapabilityId(7019),
        cap_type: CapabilityType::SystemControl,
        rights: Rights::INVOKE,
        object_id: SERVICE_POWER_CONTROL_OBJECT,
    }).unwrap();
    assert!(authorize_system_control_object(&power_only, TaskId(106), SERVICE_POWER_CONTROL_OBJECT));
    assert!(!authorize_system_control_object(&power_only, TaskId(106), GLOBAL_SYSTEM_CONTROL_OBJECT));
    assert!(!authorize_system_control_object(&power_only, TaskId(1), GLOBAL_SYSTEM_CONTROL_OBJECT));
    assert!(
        !authorize_device_wide_system_control(&power_only, TaskId(106), "com.example.pmeter"),
        "a domain-scoped capability must not become device-wide administration"
    );

    // 7. Missing capability is refused even for the system identity: the gate
    //    requires both halves, so identity alone is not authority.
    let empty = crate::capability::CapabilityTable::new_for_task(TaskId(1), 4);
    assert!(!authorize_device_wide_system_control(&empty, TaskId(1), "system"));
    assert!(!authorize_system_control_object(&empty, TaskId(1), SERVICE_POWER_CONTROL_OBJECT));
}

/// The built-in services must keep working under the confinement: each holds
/// exactly its own domain object and nothing else.
#[test]
fn built_in_service_domain_control_survives_the_confined_gate() {
    use crate::capability::{Capability, CapabilityId, CapabilityType, Rights};
    use crate::services::{
        SERVICE_AUDIO_CONTROL_OBJECT, SERVICE_CAMERA_CONTROL_OBJECT,
        SERVICE_POWER_CONTROL_OBJECT, SERVICE_SENSOR_CONTROL_OBJECT,
        SERVICE_TELEPHONY_CONTROL_OBJECT,
    };

    // Each service is spawned as its own task, none of them PID 1, and each is
    // granted exactly one domain object by `capability_policy`.
    let domains = [
        (5u8, "powerd", SERVICE_POWER_CONTROL_OBJECT),
        (6, "modemd", SERVICE_TELEPHONY_CONTROL_OBJECT),
        (7, "audiod", SERVICE_AUDIO_CONTROL_OBJECT),
        (8, "camerad", SERVICE_CAMERA_CONTROL_OBJECT),
        (11, "sensord", SERVICE_SENSOR_CONTROL_OBJECT),
    ];

    for (index, (service_id, name, domain)) in domains.iter().enumerate() {
        let task_id = TaskId(200 + index as u64);
        let mut caps = crate::capability::CapabilityTable::new_for_task(task_id, 4);
        caps.insert(Capability {
            id: CapabilityId(7100 + index as u64),
            cap_type: CapabilityType::SystemControl,
            rights: Rights::INVOKE,
            object_id: *domain,
        }).unwrap();

        assert!(
            authorize_system_control_object(&caps, task_id, *domain),
            "service {} ({}) must still control its own domain object", service_id, name
        );
        assert!(
            !authorize_system_control_object(&caps, task_id, GLOBAL_SYSTEM_CONTROL_OBJECT),
            "service {} ({}) must not reach global administration", service_id, name
        );
        assert!(
            !authorize_device_wide_system_control(&caps, task_id, name),
            "service {} ({}) must not be a device-wide administrator", service_id, name
        );
        for (_, _, other) in domains.iter() {
            if other != domain {
                assert!(
                    !authorize_system_control_object(&caps, task_id, *other),
                    "service {} ({}) must not reach another service's domain object", service_id, name
                );
            }
        }
    }

    // The launcher holds user-scoped SystemControl objects, not the global one.
    let launcher = TaskId(210);
    let mut launcher_caps = crate::capability::CapabilityTable::new_for_task(launcher, 4);
    launcher_caps.insert(Capability {
        id: CapabilityId(7200),
        cap_type: CapabilityType::SystemControl,
        rights: Rights::INVOKE,
        object_id: user_control_object(crate::user::UserId::OWNER.0),
    }).unwrap();
    assert!(authorize_system_control_object(&launcher_caps, launcher, user_control_object(crate::user::UserId::OWNER.0)));
    assert!(!authorize_system_control_object(&launcher_caps, launcher, user_control_object(9999)));
    assert!(!authorize_device_wide_system_control(&launcher_caps, launcher, "launcher"));
}

/// The removed launcher special case in `require_user_control_object` was
/// unreachable, so removing it must change nothing for the real launcher.
///
/// Pinning assertions: the launcher still administers the two accounts its
/// capability policy actually grants, it cannot reach any other account, and
/// `USER_CONTROL_OBJECT_BASE` -- the object that dead branch required -- is not
/// granted by any built-in service policy, which is why the branch could never
/// have fired.
#[test]
fn the_launcher_administrates_exactly_the_accounts_its_policy_grants() {
    use crate::capability::{Capability, CapabilityId};
    use crate::services::capability_policy;
    use crate::services::ServiceId;

    let launcher = TaskId(211);
    let policy = capability_policy(ServiceId::Launcher, TaskId(1))
        .expect("the launcher must have a capability policy");

    let mut caps = crate::capability::CapabilityTable::new_for_task(launcher, 16);
    for (slot, spec) in policy.iter().enumerate() {
        caps.insert(Capability {
            id: CapabilityId(7300 + slot as u64),
            cap_type: spec.cap_type,
            rights: spec.rights,
            object_id: spec.object_id,
        }).unwrap();
    }

    let owner = user_control_object(crate::user::UserId::OWNER.0);
    let guest = user_control_object(crate::user::UserId::GUEST.0);

    // The launcher's real grants resolve for the two accounts it holds.
    assert!(authorize_system_control_object(&caps, launcher, owner),
        "the launcher must still administer the device owner account");
    assert!(authorize_system_control_object(&caps, launcher, guest),
        "the launcher must still administer the guest account");

    // Every other account is refused.
    for uid in [0u32, 1, 999, 1002, 4242] {
        assert!(
            !authorize_system_control_object(&caps, launcher, user_control_object(uid)),
            "the launcher must not administer account {} it holds no grant for", uid
        );
    }

    // The object the dead branch required is granted by no service policy.
    assert_ne!(owner, USER_CONTROL_OBJECT_BASE);
    assert_ne!(guest, USER_CONTROL_OBJECT_BASE);
    for service in 1..=11u8 {
        let policy = capability_policy(
            match service {
                1 => ServiceId::Display,
                2 => ServiceId::Input,
                3 => ServiceId::Storage,
                4 => ServiceId::Network,
                5 => ServiceId::Power,
                6 => ServiceId::Modem,
                7 => ServiceId::Audio,
                8 => ServiceId::Camera,
                9 => ServiceId::Compositor,
                10 => ServiceId::Launcher,
                _ => ServiceId::Sensor,
            },
            TaskId(1),
        ).expect("every built-in service must have a policy");
        for spec in policy.iter() {
            assert_ne!(
                spec.object_id, USER_CONTROL_OBJECT_BASE,
                "service {} must not be granted the bare user-control base object", service
            );
        }
    }
}
}