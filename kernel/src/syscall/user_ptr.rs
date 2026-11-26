// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

//! Safe Userspace Pointer and Slice Validation
//!
//! Enforces:
//! - Kernel address exclusion (rejects any address in the higher half)
//! - Canonicality and user space range bounds
//! - Arithmetic overflow protection
//! - Null pointer and zero-page guard protection
//! - Memory permission validation

use core::marker::PhantomData;

/// Maximum allowable userspace virtual address (top of 47-bit lower canonical half).
pub const USER_SPACE_END: u64 = 0x0000_7FFF_FFFF_FFFF;

/// Guard page boundary: user pointers cannot point into page 0 (0x0000..0x0FFF).
pub const USER_SPACE_START: u64 = 0x0000_0000_0000_1000;

/// Maximum size allowed for a single userspace buffer transfer (16 MiB).
pub const MAX_USER_TRANSFER_LEN: usize = 16 * 1024 * 1024;

/// Error returned when validating a userspace memory pointer or slice.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UserAccessError {
    /// Pointer is null or points into the page-0 guard area (< 0x1000).
    NullOrGuardPage,
    /// Arithmetic overflow in pointer + length.
    Overflow,
    /// Memory range extends into kernel address space (>= 0x0000_8000_0000_0000).
    KernelAddressExclusion,
    /// Memory range exceeds maximum permitted userspace boundary.
    OutOfBounds,
    /// Length exceeds the safety limit for a single syscall.
    LengthTooLarge,
    /// Invalid UTF-8 sequence in string data.
    InvalidUtf8,
    /// Page table validation failed (not mapped or insufficient permissions).
    NotMapped,
    /// The page is mapped but is not accessible from userspace with the requested access.
    PermissionDenied,
    /// A typed userspace pointer does not satisfy its required alignment.
    Misaligned,
}

/// Validates that the memory range `[addr, addr + len)` resides entirely within
/// the user-accessible address space and does not overflow.
#[inline]
pub fn validate_user_buffer(addr: u64, len: usize) -> Result<(), UserAccessError> {
    if addr < USER_SPACE_START {
        return Err(UserAccessError::NullOrGuardPage);
    }
    if addr > USER_SPACE_END {
        return Err(UserAccessError::KernelAddressExclusion);
    }
    if len > MAX_USER_TRANSFER_LEN {
        return Err(UserAccessError::LengthTooLarge);
    }
    if len == 0 {
        return Ok(());
    }
    let end = addr
        .checked_add(len as u64)
        .ok_or(UserAccessError::Overflow)?;

    if end > USER_SPACE_END {
        return Err(UserAccessError::KernelAddressExclusion);
    }
    Ok(())
}

fn map_range_error(reason: &str) -> UserAccessError {
    match reason {
        "user page is not mapped" => UserAccessError::NotMapped,
        "user page is not accessible" | "user page is not writable" |
        "user frame is not owned by task" => UserAccessError::PermissionDenied,
        "address overflow" | "page iteration overflow" => UserAccessError::Overflow,
        _ => UserAccessError::OutOfBounds,
    }
}

fn validate_current_user_buffer(addr: u64, len: usize, write: bool) -> Result<(), UserAccessError> {
    validate_user_buffer(addr, len)?;
    if len == 0 { return Ok(()); }

    let page_table = current_task_page_table()?;

    let res = page_table
        .lock()
        .validate_user_range(addr, len, write)
        .map_err(map_range_error);
    res
}

/// The address space the running task's user pointers refer to.
fn current_task_page_table() -> Result<crate::task::SharedAddressSpace, UserAccessError> {
    let sched = crate::task::SCHEDULER.lock();
    let task = sched.current_task().ok_or(UserAccessError::NotMapped)?;
    task.page_table.clone().ok_or(UserAccessError::NotMapped)
}

// ---------------------------------------------------------------------------
// Validate-and-access atomicity (C4)
// ---------------------------------------------------------------------------
//
// WHY validation alone was not enough: `validate_current_user_buffer` took the
// scheduler lock, then the page-table lock, then *released both* and returned.
// The `copy_nonoverlapping` that followed ran holding nothing. Syscall bodies
// execute with interrupts enabled (`syscall::entry` does `sti` before
// `sys_dispatch`), and the timer switches tasks whenever
// `sync::preemption_disabled()` is false -- which it was, because the validation
// guards were already gone. A sibling thread sharing this address space could
// therefore run `mem_unmap`, free the validated frame, and have the copy touch
// it afterwards. The resulting fault was taken on a kernel stack, so
// `page_fault_handler` classified it as a supervisor fault and halted the whole
// device: an unprivileged task could hang the machine by racing its own buffer.
//
// The fix is lock *lifetime*, not new checks. The validation logic above is
// unchanged and still sound. `with_pinned_user_buffer` holds the very lock
// `sys_mem_unmap` takes -- the task's `Arc<Mutex<TaskPageTable>>` -- across both
// the range validation and the access, so an unmap cannot intervene.
//
// WHY the page-table lock and not the scheduler lock: it is the precise lock
// the racing syscall acquires, and holding it leaves the scheduler free to
// service the timer's `try_lock` paths. Holding `SCHEDULER` across a copy of up
// to 16 MiB would block every scheduling decision in the kernel.
//
// WHY a `PreemptGuard` as well: `SharedAddressSpace` is a raw `spin::Mutex`, so
// unlike `sync::Mutex` it does not defer preemption by itself. Without the gate
// the timer could switch this task out mid-copy and strand the page-table lock
// on a task that is not runnable -- the exact failure `sync.rs` exists to
// prevent. The gate is raised *after* the acquisition, never before: spinning
// on a raw spinlock with preemption deferred would deadlock against a holder
// that needs the timer to be scheduled again. Acquiring with the gate down is
// exactly what every existing `page_table.lock()` call site does.

/// Start of the user buffer a kernel-mode access is currently touching.
static ACCESS_START: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
/// Exclusive end of that buffer.
static ACCESS_END: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
/// Whether such an access is in flight at all.
static ACCESS_ACTIVE: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);
/// Address of the `spin::Mutex<TaskPageTable>` pinned across that access.
static ACCESS_PAGE_TABLE: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(0);

/// Records the buffer about to be accessed and the lock protecting it.
///
/// One window is enough because the kernel is single-CPU and the access runs
/// with preemption deferred: no other task can open a window while this one is
/// live, and the copy itself calls nothing that could nest.
fn open_access_window(addr: u64, len: usize, page_table: &spin::Mutex<crate::memory::TaskPageTable>) {
    use core::sync::atomic::Ordering;
    ACCESS_PAGE_TABLE.store(page_table as *const _ as usize, Ordering::Relaxed);
    ACCESS_START.store(addr, Ordering::Relaxed);
    ACCESS_END.store(addr.saturating_add(len as u64), Ordering::Relaxed);
    ACCESS_ACTIVE.store(true, Ordering::Release);
}

fn close_access_window() {
    use core::sync::atomic::Ordering;
    ACCESS_ACTIVE.store(false, Ordering::Release);
    ACCESS_PAGE_TABLE.store(0, Ordering::Relaxed);
    ACCESS_START.store(0, Ordering::Relaxed);
    ACCESS_END.store(0, Ordering::Relaxed);
}

/// Test-only: opens an attribution window with no page-table pin recorded.
///
/// The fault classifier reads only the active flag and the bounds, so a host
/// test can exercise every disposition without a real address space. The null
/// pin keeps [`recover_abandoned_user_access`] from dereferencing anything:
/// there is no mutex here to release.
#[cfg(test)]
pub(crate) fn test_open_access_window(addr: u64, len: usize) {
    use core::sync::atomic::Ordering;
    ACCESS_PAGE_TABLE.store(0, Ordering::Relaxed);
    ACCESS_START.store(addr, Ordering::Relaxed);
    ACCESS_END.store(addr.saturating_add(len as u64), Ordering::Relaxed);
    ACCESS_ACTIVE.store(true, Ordering::Release);
}

/// Test-only: closes the window opened by [`test_open_access_window`].
#[cfg(test)]
pub(crate) fn test_close_access_window() {
    close_access_window();
}

/// Whether a supervisor-mode fault at `addr` was caused by an in-flight
/// validated user-buffer access, rather than by a kernel bug.
///
/// This is the distinction `page_fault_handler` needs. A ring-0 fault is not
/// automatically a kernel bug: copying a user buffer runs in ring 0 on the
/// task's kernel stack, so a frame that disappeared between validation and
/// access faults with neither `USER_MODE` nor a ring-3 code segment. Those
/// faults belong to the task whose buffer it was, and must terminate that task.
/// Every other supervisor fault stays fatal.
///
/// The range test is what keeps the classification narrow: a genuine kernel
/// fault at an address outside the buffer being copied is not excused by the
/// window being open.
pub fn fault_is_user_buffer_access(addr: u64) -> bool {
    use core::sync::atomic::Ordering;
    if !ACCESS_ACTIVE.load(Ordering::Acquire) {
        return false;
    }
    let start = ACCESS_START.load(Ordering::Relaxed);
    let end = ACCESS_END.load(Ordering::Relaxed);
    addr >= start && addr < end
}

/// Releases the page-table pin a dying context held across a user copy.
///
/// Called by the fault handlers immediately before `terminate_current`, which
/// switches away permanently and never unwinds. Without this the pinned
/// `spin::Mutex<TaskPageTable>` would stay held for the rest of the device's
/// life, and `prepare_terminate` -- which locks the same address space to unmap
/// a departing thread's stack -- would spin on it forever.
///
/// # Safety of the raw address
/// The recorded address is the mutex inside the `Arc` this access cloned. That
/// `Arc` and its guard are still live on the current kernel stack: the fault
/// interrupted the copy, and nothing has unwound or been reclaimed yet. The
/// lock is held by this context, which is what `spin::Mutex::force_unlock`
/// requires.
pub fn recover_abandoned_user_access() {
    use core::sync::atomic::Ordering;
    let page_table = ACCESS_PAGE_TABLE.swap(0, Ordering::AcqRel);
    ACCESS_ACTIVE.store(false, Ordering::Release);
    ACCESS_START.store(0, Ordering::Relaxed);
    ACCESS_END.store(0, Ordering::Relaxed);
    if page_table != 0 {
        let mutex = unsafe { &*(page_table as *const spin::Mutex<crate::memory::TaskPageTable>) };
        if mutex.is_locked() {
            unsafe { mutex.force_unlock() };
        }
    }
}

/// Validates `[addr, addr + len)` and runs `access` with the page table pinned.
///
/// Validation and access are one critical section against `mem_unmap`, and the
/// fault window is open for exactly the access, so a fault inside it is
/// attributable to this buffer and recoverable.
fn with_pinned_user_buffer<R>(
    addr: u64,
    len: usize,
    write: bool,
    access: impl FnOnce() -> R,
) -> Result<R, UserAccessError> {
    validate_user_buffer(addr, len)?;
    if len == 0 {
        return Ok(access());
    }

    let space = current_task_page_table()?;
    // Gate down for the acquisition: see the note above on raw-spinlock
    // contention. This is the same order every existing call site uses,
    // SCHEDULER (released above) then the page table.
    let page_table = space.lock();
    // Gate up for the access itself, so the timer cannot switch this task out
    // holding the page table or leave the fault window open for another task.
    let _preempt = crate::sync::PreemptGuard::new();
    page_table
        .validate_user_range(addr, len, write)
        .map_err(map_range_error)?;

    open_access_window(addr, len, &space);
    let result = access();
    close_access_window();

    drop(page_table);
    Ok(result)
}

/// A strongly typed wrapper around an unvalidated userspace pointer `T`.
#[derive(Debug, Clone, Copy)]
pub struct UserPtr<T> {
    addr: u64,
    _marker: PhantomData<*const T>,
}

impl<T> UserPtr<T> {
    /// Creates a new `UserPtr` from a raw address after validating user space bounds.
    pub fn new(addr: u64) -> Result<Self, UserAccessError> {
        let size = core::mem::size_of::<T>();
        let align = core::mem::align_of::<T>();
        if align > 1 && addr % align as u64 != 0 { return Err(UserAccessError::Misaligned); }
        validate_current_user_buffer(addr, size, false)?;
        Ok(Self {
            addr,
            _marker: PhantomData,
        })
    }

    /// Returns the raw virtual address.
    pub fn addr(&self) -> u64 {
        self.addr
    }

    /// Safely copies the value from userspace memory into a kernel-local `T`.
    pub fn read(&self) -> Result<T, UserAccessError> {
        let size = core::mem::size_of::<T>();
        let ptr = self.addr as *const T;
        with_pinned_user_buffer(self.addr, size, false, || unsafe {
            core::ptr::read_unaligned(ptr)
        })
    }
}

/// A validated userspace memory slice for reading.
#[derive(Debug, Clone, Copy)]
pub struct UserSlice {
    addr: u64,
    len: usize,
}

impl UserSlice {
    /// Creates and validates a `UserSlice` from a raw address and length.
    pub fn new(addr: u64, len: usize) -> Result<Self, UserAccessError> {
        validate_current_user_buffer(addr, len, false)?;
        Ok(Self { addr, len })
    }

    pub fn addr(&self) -> u64 {
        self.addr
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Safely copies bytes from userspace into a kernel destination buffer.
    pub fn copy_to_kernel(&self, dst: &mut [u8]) -> Result<usize, UserAccessError> {
        if dst.len() < self.len {
            return Err(UserAccessError::OutOfBounds);
        }
        if self.len == 0 {
            return Ok(0);
        }
        let dst_ptr = dst.as_mut_ptr();
        let src_ptr = self.addr as *const u8;
        let len = self.len;
        with_pinned_user_buffer(self.addr, len, false, || unsafe {
            core::ptr::copy_nonoverlapping(src_ptr, dst_ptr, len);
        })?;
        Ok(len)
    }

    /// Validates that the userspace slice represents a valid UTF-8 string and executes
    /// the provided closure with the kernel-borrowed string slice.
    pub fn copy_to_kernel_typed<T: Copy>(&self, dst: &mut [T]) -> Result<usize, UserAccessError> {
        let size = core::mem::size_of::<T>();
        let align = core::mem::align_of::<T>();
        if size == 0 || self.len % size != 0 {
            return Err(UserAccessError::OutOfBounds);
        }
        if align > 1 && self.addr % align as u64 != 0 {
            return Err(UserAccessError::Misaligned);
        }
        let count = self.len / size;
        if count > dst.len() || self.len > MAX_USER_TRANSFER_LEN {
            return Err(UserAccessError::OutOfBounds);
        }
        let dst_ptr = dst.as_mut_ptr();
        let src_ptr = self.addr as *const T;
        let len = self.len;
        with_pinned_user_buffer(self.addr, len, false, || unsafe {
            core::ptr::copy_nonoverlapping(src_ptr, dst_ptr, count);
        })?;
        Ok(count)
    }

    pub fn with_str<R, F: FnOnce(&str) -> R>(&self, kernel_buf: &mut [u8], f: F) -> Result<R, UserAccessError> {
        let n = self.copy_to_kernel(kernel_buf)?;
        let s = core::str::from_utf8(&kernel_buf[..n]).map_err(|_| UserAccessError::InvalidUtf8)?;
        Ok(f(s))
    }
}

/// A validated userspace memory slice for writing.
#[derive(Debug, Clone, Copy)]
pub struct UserSliceMut {
    addr: u64,
    len: usize,
}

impl UserSliceMut {
    /// Creates and validates a mutable userspace slice.
    pub fn new(addr: u64, len: usize) -> Result<Self, UserAccessError> {
        validate_current_user_buffer(addr, len, true)?;
        Ok(Self { addr, len })
    }

    pub fn addr(&self) -> u64 {
        self.addr
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Safely copies bytes from a kernel buffer into userspace memory.
    pub fn copy_from_kernel(&self, src: &[u8]) -> Result<usize, UserAccessError> {
        let to_copy = core::cmp::min(self.len, src.len());
        if to_copy == 0 {
            return Ok(0);
        }
        let src_ptr = src.as_ptr();
        let dst_ptr = self.addr as *mut u8;
        with_pinned_user_buffer(self.addr, to_copy, true, || unsafe {
            core::ptr::copy_nonoverlapping(src_ptr, dst_ptr, to_copy);
        })?;
        Ok(to_copy)
    }
}

/// Copies bytes from userspace memory into a kernel destination slice.
#[inline]
pub fn copy_from_user(dst: &mut [u8], src_addr: u64, len: usize) -> Result<usize, UserAccessError> {
    if len > dst.len() {
        return Err(UserAccessError::OutOfBounds);
    }
    if len == 0 {
        return Ok(0);
    }
    let dst_ptr = dst.as_mut_ptr();
    let src_ptr = src_addr as *const u8;
    with_pinned_user_buffer(src_addr, len, false, || unsafe {
        core::ptr::copy_nonoverlapping(src_ptr, dst_ptr, len);
    })?;
    Ok(len)
}

/// Copies bytes from a kernel source slice into userspace memory.
#[inline]
pub fn copy_to_user(dst_addr: u64, src: &[u8]) -> Result<usize, UserAccessError> {
    let len = src.len();
    if len == 0 {
        return Ok(0);
    }
    let src_ptr = src.as_ptr();
    let dst_ptr = dst_addr as *mut u8;
    with_pinned_user_buffer(dst_addr, len, true, || unsafe {
        core::ptr::copy_nonoverlapping(src_ptr, dst_ptr, len);
    })?;
    Ok(len)
}

/// Safely reads a `Copy` value from userspace memory into a kernel-owned copy.
#[inline]
pub fn read_user_value<T: Copy>(addr: u64) -> Result<T, UserAccessError> {
    let size = core::mem::size_of::<T>();
    let align = core::mem::align_of::<T>();
    if addr % (align as u64) != 0 {
        return Err(UserAccessError::Misaligned);
    }
    let ptr = addr as *const T;
    with_pinned_user_buffer(addr, size, false, || unsafe {
        core::ptr::read_unaligned(ptr)
    })
}

/// Safely writes a `Copy` value into userspace memory.
#[inline]
pub fn write_user_value<T: Copy>(dst_addr: u64, val: &T) -> Result<(), UserAccessError> {
    let size = core::mem::size_of::<T>();
    let align = core::mem::align_of::<T>();
    if dst_addr % (align as u64) != 0 {
        return Err(UserAccessError::Misaligned);
    }
    let ptr = dst_addr as *mut T;
    let value = *val;
    with_pinned_user_buffer(dst_addr, size, true, || unsafe {
        core::ptr::write_unaligned(ptr, value);
    })
}

/// Safely copies a string from userspace memory into a kernel buffer, validating UTF-8.
#[inline]
pub fn read_user_string<'a>(addr: u64, len: usize, dst: &'a mut [u8]) -> Result<&'a str, UserAccessError> {
    if len == 0 {
        return Ok("");
    }
    let copied = copy_from_user(dst, addr, len)?;
    core::str::from_utf8(&dst[..copied]).map_err(|_| UserAccessError::InvalidUtf8)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_validate_user_buffer_null_and_guard_page() {
        assert_eq!(validate_user_buffer(0, 100), Err(UserAccessError::NullOrGuardPage));
        assert_eq!(validate_user_buffer(0x0FFF, 1), Err(UserAccessError::NullOrGuardPage));
        assert_eq!(validate_user_buffer(0, 0), Err(UserAccessError::NullOrGuardPage));
    }

    #[test]
    fn test_validate_user_buffer_valid_ranges() {
        assert_eq!(validate_user_buffer(USER_SPACE_START, 0), Ok(()));
        assert_eq!(validate_user_buffer(USER_SPACE_START, 4096), Ok(()));
        assert_eq!(validate_user_buffer(0x0040_0000, 64 * 1024), Ok(()));
        assert_eq!(validate_user_buffer(USER_SPACE_START, MAX_USER_TRANSFER_LEN), Ok(()));
    }

    #[test]
    fn test_validate_user_buffer_oversized() {
        assert_eq!(
            validate_user_buffer(USER_SPACE_START, MAX_USER_TRANSFER_LEN + 1),
            Err(UserAccessError::LengthTooLarge)
        );
    }

    #[test]
    fn test_validate_user_buffer_kernel_addresses() {
        // Canonical higher-half kernel space
        assert_eq!(
            validate_user_buffer(0xFFFF_8000_0000_0000, 100),
            Err(UserAccessError::KernelAddressExclusion)
        );
        // Non-canonical gap
        assert_eq!(
            validate_user_buffer(0x0000_8000_0000_0000, 100),
            Err(UserAccessError::KernelAddressExclusion)
        );
        // Overflow past user space
        assert_eq!(
            validate_user_buffer(USER_SPACE_END - 100, 200),
            Err(UserAccessError::KernelAddressExclusion)
        );
    }

    #[test]
    fn test_validate_user_buffer_integer_overflow() {
        assert_eq!(
            validate_user_buffer(u64::MAX - 10, 20),
            Err(UserAccessError::KernelAddressExclusion)
        );
    }

    // -------------------------------------------------------------------------
    // C4: lock lifetime and fault attribution
    // -------------------------------------------------------------------------
    //
    // The race itself -- a sibling thread unmapping a frame between validation
    // and the copy -- cannot be reproduced here: it needs two real tasks, a real
    // address space and a real timer interrupt. What these tests pin down is the
    // part that is decidable on the host: the bounds checks still run before any
    // pin, the fault window classifies exactly the buffer being accessed and
    // nothing else, recovery releases an abandoned pin, and no access helper is
    // allowed to go back to validate-then-access.

    /// Serializes the tests that touch the global access window.
    static WINDOW_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[test]
    fn a_zero_length_access_needs_no_address_space() {
        let _serialized = WINDOW_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        // No current task exists on the host, so reaching the scheduler here
        // would fail. A zero-length transfer must not need it.
        assert_eq!(with_pinned_user_buffer(USER_SPACE_START, 0, false, || 7u32), Ok(7));
        assert!(!fault_is_user_buffer_access(USER_SPACE_START));
    }

    #[test]
    fn the_bounds_checks_still_run_before_any_pin_is_taken() {
        let _serialized = WINDOW_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        assert_eq!(
            with_pinned_user_buffer(0, 16, false, || ()),
            Err(UserAccessError::NullOrGuardPage)
        );
        assert_eq!(
            with_pinned_user_buffer(USER_SPACE_END - 100, 200, true, || ()),
            Err(UserAccessError::KernelAddressExclusion)
        );
        assert_eq!(
            with_pinned_user_buffer(USER_SPACE_START, MAX_USER_TRANSFER_LEN + 1, false, || ()),
            Err(UserAccessError::LengthTooLarge)
        );
        assert!(!fault_is_user_buffer_access(USER_SPACE_START));
    }

    /// A supervisor fault inside the buffer being copied belongs to the task
    /// whose buffer it was. One outside it does not, even with the window open:
    /// that is the distinction that keeps genuine kernel faults fatal.
    #[test]
    fn the_window_attributes_only_the_buffer_being_accessed() {
        let _serialized = WINDOW_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let mutex: &'static spin::Mutex<crate::memory::TaskPageTable> =
            alloc::boxed::Box::leak(alloc::boxed::Box::new(spin::Mutex::new(host_page_table())));
        open_access_window(0x0040_0000, 4096, mutex);

        assert!(fault_is_user_buffer_access(0x0040_0000), "the first byte is in the buffer");
        assert!(fault_is_user_buffer_access(0x0040_0FFF), "the last byte is in the buffer");
        assert!(!fault_is_user_buffer_access(0x0040_1000), "the end is exclusive");
        assert!(!fault_is_user_buffer_access(0x003F_FFFF), "the byte before the buffer is not in it");
        assert!(
            !fault_is_user_buffer_access(0xFFFF_8000_0000_0000),
            "a kernel address is never a user-buffer fault"
        );
        assert!(!fault_is_user_buffer_access(0), "the guard page is never a user-buffer fault");

        close_access_window();
        assert!(
            !fault_is_user_buffer_access(0x0040_0000),
            "once the copy is finished a fault at the same address is a kernel fault again"
        );
    }

    #[test]
    fn with_no_access_in_flight_every_supervisor_fault_stays_fatal() {
        let _serialized = WINDOW_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        close_access_window();
        for addr in [0u64, 0x1000, 0x0040_0000, USER_SPACE_END, 0xFFFF_8000_0000_0000, u64::MAX] {
            assert!(
                !fault_is_user_buffer_access(addr),
                "no window is open, so {addr:#x} must not be excused"
            );
        }
    }

    /// The page table a dying context pinned across its copy must come back.
    ///
    /// `prepare_terminate` locks the same address space to unmap a departing
    /// thread's stack, so an abandoned pin is not a leak -- it is a hang on the
    /// very next step of termination. `mem::forget` models the missing unwind.
    #[test]
    fn recovery_releases_the_page_table_pin_a_dying_context_left_behind() {
        let _serialized = WINDOW_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let mutex: &'static spin::Mutex<crate::memory::TaskPageTable> =
            alloc::boxed::Box::leak(alloc::boxed::Box::new(spin::Mutex::new(host_page_table())));

        let guard = mutex.lock();
        open_access_window(0x0040_0000, 4096, mutex);
        assert!(fault_is_user_buffer_access(0x0040_0000));

        // The copy faults and the task is terminated: no unwind, no Drop.
        core::mem::forget(guard);
        assert!(mutex.is_locked(), "the abandoned pin is still held");

        recover_abandoned_user_access();

        assert!(!mutex.is_locked(), "recovery must release the abandoned pin");
        assert!(mutex.try_lock().is_some(), "the address space must be lockable again");
        assert!(!fault_is_user_buffer_access(0x0040_0000), "the window must be closed");

        // Idempotent: the fault handlers may reach recovery more than once.
        recover_abandoned_user_access();
        assert!(mutex.try_lock().is_some());
    }

    #[test]
    fn recovery_with_no_pin_in_flight_is_a_no_op() {
        let _serialized = WINDOW_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        close_access_window();
        recover_abandoned_user_access();
        assert!(!fault_is_user_buffer_access(0x0040_0000));
    }

    /// A `TaskPageTable` value for host tests.
    ///
    /// Only the mutex around it is ever used here. The value is leaked by the
    /// caller and never dropped, because `TaskPageTable::Drop` reads CR3 and
    /// recycles frames -- privileged instructions and a frame allocator that do
    /// not exist in a host test binary.
    fn host_page_table() -> crate::memory::TaskPageTable {
        crate::memory::TaskPageTable {
            pml4_frame: x86_64::structures::paging::PhysFrame::containing_address(
                x86_64::PhysAddr::new(0),
            ),
            allocated_frames: alloc::vec::Vec::new(),
            shared_mappings: alloc::vec::Vec::new(),
            borrowed_mappings: alloc::vec::Vec::new(),
            borrowed_pages_count: 0,
            user_mapped_bytes: 0,
        }
    }

    /// Returns the body of `fn name(...)` by brace matching.
    ///
    /// Accepts a generic signature too: `copy_to_kernel_typed<T: Copy>` has a
    /// parameter list between the name and the parentheses, so a plain
    /// `fn name(` needle would not find it.
    fn fn_body<'a>(source: &'a str, name: &str) -> &'a str {
        let needle = alloc::format!("fn {name}");
        let mut from = 0usize;
        let start = loop {
            let index = source[from..]
                .find(&needle)
                .map(|offset| from + offset)
                .unwrap_or_else(|| panic!("no fn {name} in user_ptr.rs"));
            let after = index + needle.len();
            match source[after..].chars().next() {
                Some('(') | Some('<') => break index,
                // A longer name that merely starts with this one.
                _ => from = after,
            }
        };
        let open = source[start..].find('{').map(|i| start + i).expect("fn body");
        let mut depth = 0usize;
        for (offset, byte) in source[open..].bytes().enumerate() {
            match byte {
                b'{' => depth += 1,
                b'}' => {
                    depth -= 1;
                    if depth == 0 {
                        return &source[open + 1..open + offset];
                    }
                }
                _ => {}
            }
        }
        panic!("unbalanced braces in fn {name}");
    }

    /// Structural guard for the defect itself: every helper that touches user
    /// memory must do it inside the pinned critical section, not after a
    /// validation call that has already released its locks.
    #[test]
    fn every_user_memory_access_runs_inside_the_pinned_critical_section() {
        let source = include_str!("user_ptr.rs");
        // Cut at the test *module*, not at the first `#[cfg(test)]`: the
        // test-only window hooks above are cfg(test) too, and splitting there
        // would hide every access helper from this scan.
        let cut = source
            .find("mod tests {")
            .expect("user_ptr.rs must keep its test module");
        let production = &source[..cut];

        for name in [
            "read",
            "copy_to_kernel",
            "copy_to_kernel_typed",
            "copy_from_kernel",
            "copy_from_user",
            "copy_to_user",
            "read_user_value",
            "write_user_value",
        ] {
            let body = fn_body(production, name);
            assert!(
                body.contains("with_pinned_user_buffer"),
                "user_ptr.rs: fn {name} touches user memory without holding the page table \
                 across the access -- that is the validate-then-access TOCTOU"
            );
            assert!(
                !body.contains("validate_current_user_buffer"),
                "user_ptr.rs: fn {name} went back to validate-then-release-then-access"
            );
        }
    }
}


