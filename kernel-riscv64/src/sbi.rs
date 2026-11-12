// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

//! RISC-V Supervisor Binary Interface (SBI) Wrappers

use core::arch::asm;

#[inline(always)]
pub fn console_putchar(c: usize) {
    unsafe {
        asm!(
            "ecall",
            in("a7") 1usize, // SBI_CONSOLE_PUTCHAR
            in("a0") c,
            lateout("a0") _,
            lateout("a1") _,
        );
    }
}

#[inline(always)]
pub fn console_getchar() -> Option<u8> {
    let uart_lsr = 0x1000_0005 as *const u8;
    let lsr = unsafe { core::ptr::read_volatile(uart_lsr) };
    if (lsr & 0x01) != 0 {
        let uart_rbr = 0x1000_0000 as *const u8;
        return Some(unsafe { core::ptr::read_volatile(uart_rbr) });
    }

    let ret: isize;
    unsafe {
        asm!(
            "ecall",
            in("a7") 2usize, // SBI_CONSOLE_GETCHAR
            lateout("a0") ret,
            lateout("a1") _,
        );
    }
    if ret >= 0 {
        Some(ret as u8)
    } else {
        None
    }
}

#[inline(always)]
pub fn shutdown() -> ! {
    unsafe {
        asm!(
            "ecall",
            in("a7") 8usize, // SBI_SHUTDOWN
            lateout("a0") _,
            lateout("a1") _,
        );
    }
    loop {}
}

pub const SBI_EXT_BASE: usize = 0x10;
pub const SBI_EXT_BASE_PROBE_EXT: usize = 3;

pub const SBI_EXT_RFNC: usize = 0x52464E43;
pub const SBI_EXT_RFNC_REMOTE_FENCE_I: usize = 0;
pub const SBI_EXT_RFNC_REMOTE_SFENCE_VMA: usize = 1;
pub const SBI_EXT_RFNC_REMOTE_SFENCE_VMA_ASID: usize = 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SbiError {
    Failed,
    NotSupported,
    InvalidParam,
    Denied,
    InvalidAddress,
    AlreadyAvailable,
    AlreadyStarted,
    AlreadyStopped,
    NoShmem,
    Unknown(isize),
}

impl SbiError {
    pub fn from_isize(val: isize) -> Option<Self> {
        match val {
            0 => None, // SBI_SUCCESS
            -1 => Some(SbiError::Failed),
            -2 => Some(SbiError::NotSupported),
            -3 => Some(SbiError::InvalidParam),
            -4 => Some(SbiError::Denied),
            -5 => Some(SbiError::InvalidAddress),
            -6 => Some(SbiError::AlreadyAvailable),
            -7 => Some(SbiError::AlreadyStarted),
            -8 => Some(SbiError::AlreadyStopped),
            -9 => Some(SbiError::NoShmem),
            other => Some(SbiError::Unknown(other)),
        }
    }
}

#[inline(always)]
pub fn sbi_call(eid: usize, fid: usize, arg0: usize, arg1: usize, arg2: usize, arg3: usize) -> (isize, usize) {
    let err: isize;
    let val: usize;
    unsafe {
        asm!(
            "ecall",
            in("a7") eid,
            in("a6") fid,
            in("a0") arg0,
            in("a1") arg1,
            in("a2") arg2,
            in("a3") arg3,
            lateout("a0") err,
            lateout("a1") val,
        );
    }
    (err, val)
}

/// Probes whether a given SBI extension ID is supported by firmware.
pub fn probe_extension(extension_id: usize) -> bool {
    let (err, val) = sbi_call(SBI_EXT_BASE, SBI_EXT_BASE_PROBE_EXT, extension_id, 0, 0, 0);
    err == 0 && val != 0
}

/// Broadcasts an sfence.vma instruction to remote harts via SBI RFNC extension (EID 0x52464E43, FID 0x1).
/// Returns Ok(()) on SBI_SUCCESS, or Err(SbiError) if firmware rejected or failed the request.
pub fn remote_sfence_vma(hart_mask: usize, hart_mask_base: usize, start_addr: usize, size: usize) -> Result<(), SbiError> {
    let (err, _) = sbi_call(SBI_EXT_RFNC, SBI_EXT_RFNC_REMOTE_SFENCE_VMA, hart_mask, hart_mask_base, start_addr, size);
    if let Some(e) = SbiError::from_isize(err) {
        Err(e)
    } else {
        Ok(())
    }
}

pub struct SbiConsole;

impl core::fmt::Write for SbiConsole {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        for b in s.bytes() {
            if b == b'\n' {
                console_putchar(b'\r' as usize);
            }
            console_putchar(b as usize);
        }
        Ok(())
    }
}
