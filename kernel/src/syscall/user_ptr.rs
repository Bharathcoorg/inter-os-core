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
}

/// Validates that the memory range `[addr, addr + len)` resides entirely within
/// the user-accessible address space and does not overflow.
#[inline]
pub fn validate_user_buffer(addr: u64, len: usize) -> Result<(), UserAccessError> {
    if addr < USER_SPACE_START {
        return Err(UserAccessError::NullOrGuardPage);
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
        validate_user_buffer(addr, size)?;
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
    pub unsafe fn read(&self) -> Result<T, UserAccessError> {
        let size = core::mem::size_of::<T>();
        validate_user_buffer(self.addr, size)?;
        let ptr = self.addr as *const T;
        Ok(core::ptr::read_unaligned(ptr))
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
        validate_user_buffer(addr, len)?;
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
        validate_user_buffer(self.addr, self.len)?;
        unsafe {
            core::ptr::copy_nonoverlapping(
                self.addr as *const u8,
                dst.as_mut_ptr(),
                self.len,
            );
        }
        Ok(self.len)
    }

    /// Validates that the userspace slice represents a valid UTF-8 string and executes
    /// the provided closure with the kernel-borrowed string slice.
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
        validate_user_buffer(addr, len)?;
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
        validate_user_buffer(self.addr, to_copy)?;
        unsafe {
            core::ptr::copy_nonoverlapping(
                src.as_ptr(),
                self.addr as *mut u8,
                to_copy,
            );
        }
        Ok(to_copy)
    }
}
