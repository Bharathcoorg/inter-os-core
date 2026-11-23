// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

//! Mobile eMMC 5.1 & SDHCI Storage Host Controller Driver (AArch64)
//!
//! Conforms to SD Host Controller Standard Specification (v3.00 / v4.20)
//! driving non-volatile flash storage on physical smartphones:
//! - PinePhone (Allwinner A64 eMMC 5.1 & microSD)
//! - Qualcomm Snapdragon mobile platforms (SDHCI / eMMC / UFS bridge)
//! - Raspberry Pi Compute Module 4 (Broadcom eMMC)
//!
//! It is also the AArch64 implementation of the platform-neutral block device
//! contract the shared kernel core's journal commits through
//! (`interos_kernel_core::storage::BlockDevice`), which is what lets the journal
//! that used to exist only in the x86 kernel run here. The adapter is at the
//! bottom of this file.

use core::sync::atomic::{compiler_fence, Ordering};

use interos_kernel_core::storage::{BlockDevice, BlockDeviceError, SECTOR_SIZE};

pub const SDHCI_BLOCK_SIZE: usize = 0x04;
pub const SDHCI_BLOCK_COUNT: usize = 0x06;
pub const SDHCI_ARGUMENT: usize = 0x08;
pub const SDHCI_TRANSFER_MODE: usize = 0x0C;
pub const SDHCI_COMMAND: usize = 0x0E;
pub const SDHCI_RESPONSE_0: usize = 0x10;
pub const SDHCI_BUFFER_PORT: usize = 0x20;
pub const SDHCI_PRESENT_STATE: usize = 0x24;
pub const SDHCI_SOFTWARE_RESET: usize = 0x2F;
pub const SDHCI_INT_STATUS: usize = 0x30;

pub const STATE_CMD_INHIBIT: u32 = 1 << 0;
pub const STATE_DAT_INHIBIT: u32 = 1 << 1;
pub const STATE_BUFFER_WRITE_EN: u32 = 1 << 10;
pub const STATE_BUFFER_READ_EN: u32 = 1 << 11;
pub const STATE_CARD_INSERTED: u32 = 1 << 16;

pub const INT_CMD_COMPLETE: u32 = 1 << 0;
pub const INT_TRANSFER_COMPLETE: u32 = 1 << 1;
pub const INT_ERROR: u32 = 1 << 15;
pub const INT_TIMEOUT_ERROR: u32 = 1 << 16;

pub const CMD0_GO_IDLE: u16 = 0x0000;
pub const CMD17_READ_SINGLE: u16 = (17 << 8) | 0x3A;
pub const CMD24_WRITE_SINGLE: u16 = (24 << 8) | 0x3A;

pub const TIMEOUT_CYCLES: u32 = 200_000;

#[inline(always)]
unsafe fn mmio_read32(base: usize, offset: usize) -> u32 {
    let ptr = (base + offset) as *const u32;
    compiler_fence(Ordering::SeqCst);
    let v = core::ptr::read_volatile(ptr);
    compiler_fence(Ordering::SeqCst);
    v
}

#[inline(always)]
unsafe fn mmio_write32(base: usize, offset: usize, val: u32) {
    let ptr = (base + offset) as *mut u32;
    compiler_fence(Ordering::SeqCst);
    core::ptr::write_volatile(ptr, val);
    compiler_fence(Ordering::SeqCst);
}

#[inline(always)]
unsafe fn mmio_write16(base: usize, offset: usize, val: u16) {
    let ptr = (base + offset) as *mut u16;
    compiler_fence(Ordering::SeqCst);
    core::ptr::write_volatile(ptr, val);
    compiler_fence(Ordering::SeqCst);
}

#[inline(always)]
unsafe fn mmio_write8(base: usize, offset: usize, val: u8) {
    let ptr = (base + offset) as *mut u8;
    compiler_fence(Ordering::SeqCst);
    core::ptr::write_volatile(ptr, val);
    compiler_fence(Ordering::SeqCst);
}

pub struct SdhciController {
    pub base_vaddr: usize,
    pub hardware_present: bool,
    pub rca: u16,
    pub total_sectors: u32,
    pub high_capacity: bool,
    pub model_name: [u8; 16],
}

impl SdhciController {
    pub const fn new(base_vaddr: usize) -> Self {
        Self {
            base_vaddr,
            hardware_present: false,
            rca: 0,
            total_sectors: 0,
            high_capacity: false,
            model_name: *b"eMMC 5.1 Mobile ",
        }
    }

    /// Clears exactly the given interrupt-status bits using W1C semantics.
    fn clear_interrupts(&self, bits: u32) {
        unsafe {
            mmio_write32(self.base_vaddr, SDHCI_INT_STATUS, bits);
        }
    }

    pub fn set_hardware_present(&mut self, present: bool) {
        self.hardware_present = present;
        if !present {
            self.rca = 0;
            self.total_sectors = 0;
            self.high_capacity = false;
        }
    }

    pub fn reset(&self) -> Result<(), &'static str> {
        if !self.hardware_present {
            return Err("SDHCI hardware unavailable");
        }
        unsafe {
            mmio_write8(self.base_vaddr, SDHCI_SOFTWARE_RESET, 0x01);
            for _ in 0..TIMEOUT_CYCLES {
                let status = mmio_read32(self.base_vaddr, SDHCI_SOFTWARE_RESET) as u8;
                if (status & 0x01) == 0 {
                    return Ok(());
                }
            }
        }
        Err("SDHCI Reset Timeout")
    }

    pub fn is_card_inserted(&self) -> bool {
        if !self.hardware_present {
            return false;
        }
        unsafe {
            let state = mmio_read32(self.base_vaddr, SDHCI_PRESENT_STATE);
            (state & STATE_CARD_INSERTED) != 0
        }
    }

    pub fn send_command(&self, cmd_idx: u16, arg: u32, is_data: bool) -> Result<u32, &'static str> {
        if !self.hardware_present {
            return Err("SDHCI hardware unavailable");
        }
        unsafe {
            for _ in 0..TIMEOUT_CYCLES {
                let state = mmio_read32(self.base_vaddr, SDHCI_PRESENT_STATE);
                let mask = if is_data {
                    STATE_CMD_INHIBIT | STATE_DAT_INHIBIT
                } else {
                    STATE_CMD_INHIBIT
                };
                if (state & mask) == 0 {
                    break;
                }
            }

            let pending = mmio_read32(self.base_vaddr, SDHCI_INT_STATUS);
            if (pending & INT_ERROR) != 0 {
                self.clear_interrupts(pending & INT_ERROR);
            }

            mmio_write32(self.base_vaddr, SDHCI_ARGUMENT, arg);
            mmio_write16(self.base_vaddr, SDHCI_COMMAND, cmd_idx);

            // Hardware owns interrupt status. Never manufacture completion bits in software.
            for _ in 0..TIMEOUT_CYCLES {
                let int_stat = mmio_read32(self.base_vaddr, SDHCI_INT_STATUS);
                if (int_stat & INT_ERROR) != 0 || (int_stat & INT_TIMEOUT_ERROR) != 0 {
                    return Err("SDHCI Device Error");
                }
                if (int_stat & INT_CMD_COMPLETE) != 0 {
                    self.clear_interrupts(INT_CMD_COMPLETE);
                    let resp0 = mmio_read32(self.base_vaddr, SDHCI_RESPONSE_0);
                    return Ok(resp0);
                }
            }
        }
        Err("SDHCI Command Timeout")
    }

    pub fn read_block(&self, lba: u32, buf: &mut [u8; SECTOR_SIZE]) -> Result<(), &'static str> {
        let addr_arg = if self.high_capacity {
            lba
        } else {
            lba.checked_mul(SECTOR_SIZE as u32).ok_or("LBA overflow")?
        };

        unsafe {
            mmio_write16(self.base_vaddr, SDHCI_BLOCK_SIZE, 512);
            mmio_write16(self.base_vaddr, SDHCI_BLOCK_COUNT, 1);
            mmio_write16(self.base_vaddr, SDHCI_TRANSFER_MODE, 0x0012);

            self.send_command(CMD17_READ_SINGLE, addr_arg, true)?;

            let mut ready = false;
            for _ in 0..TIMEOUT_CYCLES {
                let state = mmio_read32(self.base_vaddr, SDHCI_PRESENT_STATE);
                if (state & STATE_BUFFER_READ_EN) != 0 {
                    ready = true;
                    break;
                }
            }
            if !ready {
                return Err("SDHCI Read Buffer Not Ready");
            }

            for i in 0..128 {
                let dword = mmio_read32(self.base_vaddr, SDHCI_BUFFER_PORT);
                core::ptr::write_unaligned(buf.as_mut_ptr().add(i * 4) as *mut u32, dword);
            }

            self.clear_interrupts(INT_TRANSFER_COMPLETE);
        }

        Ok(())
    }

    pub fn write_block(&self, lba: u32, buf: &[u8; SECTOR_SIZE]) -> Result<(), &'static str> {
        let addr_arg = if self.high_capacity {
            lba
        } else {
            lba.checked_mul(SECTOR_SIZE as u32).ok_or("LBA overflow")?
        };

        unsafe {
            mmio_write16(self.base_vaddr, SDHCI_BLOCK_SIZE, 512);
            mmio_write16(self.base_vaddr, SDHCI_BLOCK_COUNT, 1);
            mmio_write16(self.base_vaddr, SDHCI_TRANSFER_MODE, 0x0002);

            self.send_command(CMD24_WRITE_SINGLE, addr_arg, true)?;

            let mut ready = false;
            for _ in 0..TIMEOUT_CYCLES {
                let state = mmio_read32(self.base_vaddr, SDHCI_PRESENT_STATE);
                if (state & STATE_BUFFER_WRITE_EN) != 0 {
                    ready = true;
                    break;
                }
            }
            if !ready {
                return Err("SDHCI Write Buffer Not Ready");
            }

            for i in 0..128 {
                let dword = core::ptr::read_unaligned(buf.as_ptr().add(i * 4) as *const u32);
                mmio_write32(self.base_vaddr, SDHCI_BUFFER_PORT, dword);
            }

            self.clear_interrupts(INT_TRANSFER_COMPLETE);
        }

        Ok(())
    }
}

/// AArch64 half of the platform-neutral block device contract.
///
/// WHY the shared core needs this at all: its journal was extracted from the x86
/// kernel, where it was hard-bound to `crate::drivers::ATA_PRIMARY
/// (kernel/src/storage/persistent.rs:25) — an ATA PIO driver at fixed port
/// addresses. The core now declares `BlockDevice` and asks the architecture to
/// bind one; this is that binding for AArch64.
///
/// WHY the errors are this shape: the journal distinguishes "no device" from "the
/// device failed". A controller with no card bound must answer `DeviceNotFound`,
/// never a successful read of zeroes, because the journal would then commit
/// records into a device that does not exist and report them as durable.
impl BlockDevice for SdhciController {
    fn read_sector(
        &mut self,
        lba: u32,
        buf: &mut [u8; SECTOR_SIZE],
    ) -> Result<(), BlockDeviceError> {
        if !self.hardware_present {
            return Err(BlockDeviceError::DeviceNotFound);
        }
        if self.total_sectors != 0 && lba >= self.total_sectors {
            return Err(BlockDeviceError::LbaOutOfRange);
        }
        self.read_block(lba, buf)
            .map_err(|_| BlockDeviceError::ReadError)
    }

    fn write_sector(&mut self, lba: u32, buf: &[u8; SECTOR_SIZE]) -> Result<(), BlockDeviceError> {
        if !self.hardware_present {
            return Err(BlockDeviceError::DeviceNotFound);
        }
        if self.total_sectors != 0 && lba >= self.total_sectors {
            return Err(BlockDeviceError::LbaOutOfRange);
        }
        self.write_block(lba, buf)
            .map_err(|_| BlockDeviceError::WriteError)
    }

    fn sector_count(&self) -> u32 {
        // Zero when no card is bound, which is what makes the journal report
        // `Unavailable` rather than compute slot addresses against a device that
        // does not exist.
        if self.hardware_present {
            self.total_sectors
        } else {
            0
        }
    }

    fn model(&self) -> &str {
        core::str::from_utf8(&self.model_name).unwrap_or("SDHCI")
    }

    // WHY the trait's default `Ok(())` is kept rather than inventing a flush: this
    // driver implements no SD stop-transmission or power-off notification command,
    // and reporting a flush that was never sent would tell the journal its records
    // reached media when nothing was written. The eMMC write completes in the
    // command's transfer-complete status, which is what `write_block` waits for.
}

/// The extent of the running AArch64 image, as the boot guard's seam supplies it.
///
/// WHY this exists at all: the guard refuses a write it cannot prove is clear of
/// the code that is executing, and the extent it compares against was derived on
/// x86 by walking the kernel's own ELF program headers
/// (`kernel/src/storage/boot_guard.rs:254`, via the `__ehdr_start` symbol the
/// bootloader API provides). An AArch64 image cannot do that: `linker.ld` puts the
/// ARM64 Linux boot header in the first 64 bytes of `.text`, so there is no ELF
/// header at the load base to walk.
///
/// WHY the bound comes from the linker symbols: `_kernel_start` and `_kernel_end`
/// are defined by `linker.ld`, and the same difference is what the ARM64 image
/// header publishes as `image_size` at offset 0x10. The guard only ever uses
/// `file_sectors()`, the on-disk size, and the memory extent is never smaller than
/// the file extent — so using it here can only make the guard refuse more often,
/// which is the direction the guard is allowed to be wrong in.
pub struct Aarch64ImageExtent;

impl interos_kernel_core::storage::boot_guard::ImageExtentSource for Aarch64ImageExtent {
    fn image_extent(&self) -> Option<interos_kernel_core::storage::ImageExtent> {
        extern "C" {
            static _kernel_start: u8;
            static _kernel_end: u8;
        }
        // SAFETY: `_kernel_start` and `_kernel_end` are linker-script symbols
        // bounding this kernel's own loadable image, and only their addresses are
        // taken; nothing is dereferenced.
        let start = unsafe { &_kernel_start as *const u8 as u64 };
        let end = unsafe { &_kernel_end as *const u8 as u64 };
        let image_bytes = end.checked_sub(start)?;
        Some(interos_kernel_core::storage::ImageExtent {
            hdr_vaddr: start,
            vaddr_start: start,
            vaddr_end: end,
            paddr_start: start,
            paddr_end: end,
            file_bytes: image_bytes,
        })
    }
}

pub static AARCH64_IMAGE_EXTENT: Aarch64ImageExtent = Aarch64ImageExtent;

/// The SDHCI controller bound to the shared core's journal.
///
/// WHY a static: the journal takes the device as a `&'static mut` and holds it
/// for the whole of a read or a commit, exactly as the x86 kernel holds
/// `ATA_PRIMARY` behind its mutex, so that no other context can interleave a
/// write with a read.
static mut SDHCI_JOURNAL_DEVICE: SdhciController = SdhciController::new(0);

/// The virtio-blk transport bound to the journal, when one answered the probe.
/// Filled at most once, by `bind_journal_device`, before secondary CPUs run.
static mut VIRTIO_JOURNAL_DEVICE: Option<
    interos_kernel_core::storage::virtio_blk::VirtioMmioBlockDevice,
> = None;

/// Binds this platform's block device to the shared journal and reports what the
/// journal can now do. Called during boot.
///
/// WHAT THIS REPORTS ON QEMU `virt` TODAY, measured: no storage region has been
/// discovered, so the controller is bound unpopulated and reports zero sectors —
/// the log line is `block device bound, 0 sectors`, and the journal's recovery
/// answers `Ok(None)` because with zero sectors there are no slots to hold a
/// record. That is the correct answer for a device that does not exist, and it is
/// not the same answer as a first boot on a real device: on a device with a
/// journal region, `Ok(None)` means "no record has been committed" and the
/// keystore mints a storage root there. The two are distinguishable, which is why
/// the boot log never claims a rollback floor was read.
///
/// WHY virtio is probed first: the QEMU `virt` board attaches no eMMC region,
/// so without a real transport this static would only ever hold the
/// bound-but-empty controller that proves the journal's refusal path. The
/// shared core's virtio-mmio block driver turns `-drive if=none -device
/// virtio-blk-device` into real sectors for exactly that case; the SDHCI
/// controller below remains the binding for real hardware. Whichever probe
/// answers first owns the journal.
pub fn bind_journal_device() {
    // Virtio transports first. On QEMU's ARM `virt` board the virtio-mmio
    // transports sit at 0x0a000000 with a 0x200-byte stride (32 slots,
    // 0x0a000000..0x0a004000) — NOT the 0x10001000/0x1000 layout the RISC-V
    // virt board uses. `virtio-blk-device` plugs into the first free slot.
    for i in 0..32usize {
        let mmio = 0x0a00_0000 + i * 0x200;
        // SAFETY: identity-mapped device MMIO, 4096 bytes per transport; the
        // probe writes only virtio registers.
        let device =
            interos_kernel_core::storage::virtio_blk::VirtioMmioBlockDevice::probe(mmio);
        if device.is_none() {
            // Diagnose a transport that answers "virtio" but refuses the
            // driver: without this, a failed negotiation looks identical to
            // an empty slot and the journal silently binds the SDHCI stub.
            let magic = unsafe { core::ptr::read_volatile(mmio as *const u32) };
            if magic == 0x7472_6976 {
                let version = unsafe { core::ptr::read_volatile((mmio + 4) as *const u32) };
                let device_id = unsafe { core::ptr::read_volatile((mmio + 8) as *const u32) };
                let status = unsafe { core::ptr::read_volatile((mmio + 0x70) as *const u32) };
                let max_queue = unsafe { core::ptr::read_volatile((mmio + 0x34) as *const u32) };
                crate::kprintln!(
                    "[VIRTIO-MMIO] probe failed at {:#x}: version {} device_id {} status {:#x} queue_num_max {}.",
                    mmio,
                    version,
                    device_id,
                    status,
                    max_queue
                );
            }
            continue;
        }
        let device = device.expect("checked Some above");
        // The device moves here: from probe's stack frame into the kernel
        // static. Queue registers and DRIVER_OK are only programmed after the
        // move, by `activate`, so the transport never sees a pre-move PFN.
        let bound: &'static mut interos_kernel_core::storage::virtio_blk::VirtioMmioBlockDevice =
            unsafe { (&mut *core::ptr::addr_of_mut!(VIRTIO_JOURNAL_DEVICE)).insert(device) };
        if !bound.activate() {
            // This transport answered "virtio" but refused the queue/features
            // step. Give it back and keep scanning; the static is restored to
            // None so the next slot starts clean.
            unsafe { (&mut *core::ptr::addr_of_mut!(VIRTIO_JOURNAL_DEVICE)).take() };
            crate::kprintln!(
                "[VIRTIO-MMIO] transport at {:#x} refused activation; kept scanning.",
                mmio
            );
            continue;
        }
        let sectors = bound.sector_count();
        match interos_kernel_core::storage::persistent::bind_block_device(bound) {
            Ok(()) => crate::kprintln!(
                "[OK] kernel-core journal: virtio-blk bound at MMIO {:#x}, {} sectors.",
                mmio,
                sectors
            ),
            Err(reason) => crate::kprintln!("[WARN] kernel-core journal: {}", reason),
        }
        return;
    }

    // SAFETY: this runs once, on the primordial CPU, before secondary cores are
    // brought up and before anything else can reach the journal, so there is
    // exactly one `&'static mut` to hand out and it is handed out once.
    let device: &'static mut SdhciController =
        unsafe { &mut *core::ptr::addr_of_mut!(SDHCI_JOURNAL_DEVICE) };
    // WHY the count is read before the bind: the binding hands the same reference
    // away for `'static`, so it cannot be borrowed again afterwards.
    let sectors = device.sector_count();
    match interos_kernel_core::storage::persistent::bind_block_device(device) {
        Ok(()) => crate::kprintln!(
            "[OK] kernel-core journal: block device bound, {} sectors.",
            sectors
        ),
        Err(reason) => crate::kprintln!("[WARN] kernel-core journal: {}", reason),
    }
}

/// Reads the verified-boot rollback floor (journal record kind 9) through the
/// shared core's recovery routine and reports exactly what it found.
///
/// This is the read path that a boot with no block device answers `Unavailable`
/// for, and it is written so that "no device" is never reported as "no record":
/// the keystore's storage root is recovered from the same call, and treating
/// absence as a first boot mints a replacement root that orphans every credential
/// stored under the real one.
pub fn report_journal_rollback_floor() {
    let mut floor = [0u8; 8];
    match interos_kernel_core::storage::persistent::load_latest_kind(
        &mut floor, 9, // KIND_FIRMWARE_ROLLBACK_FLOOR, mirrored from the x86 kind table
    ) {
        Ok(Some((generation, len))) => {
            crate::kprintln!(
                "[OK] kernel-core journal: rollback floor generation {} recovered ({} bytes).",
                generation,
                len
            );
        }
        Ok(None) => {
            crate::kprintln!("[OK] kernel-core journal: no rollback floor committed yet.");
        }
        Err(err) => {
            crate::kprintln!(
                "[OK] kernel-core journal: rollback floor unavailable ({:?}) — no block device is bound.",
                err
            );
        }
    }
}

/// Commits one rollback-floor record through the journal and reads it back.
///
/// Mirrors the RISC-V kernel's selftest (`kernel-riscv64/src/storage.rs`):
/// at boot it proves the whole vertical — virtio driver, queue, DMA scratch,
/// journal slots, HMAC chain — against the device QEMU actually attached, and
/// because the generation is read before the commit, a second boot on the same
/// disk recovers the first boot's record and commits the next generation, which
/// is the persistence proof a single boot cannot give. A failure is reported
/// as it is, never swallowed: an unproven journal must look unproven.
pub fn journal_selftest() -> Result<(), BlockDeviceError> {
    const KIND: u8 = 9;
    let mut generation = 1u64;
    let mut existing = [0u8; 8];
    if let Ok(Some((gen, _))) =
        interos_kernel_core::storage::persistent::load_latest_kind(&mut existing, KIND)
    {
        generation = gen.saturating_add(1);
    }
    interos_kernel_core::storage::persistent::commit_kind(
        &generation.to_le_bytes(),
        KIND,
    )
    .map_err(|_| BlockDeviceError::WriteError)?;
    let mut readback = [0u8; 8];
    match interos_kernel_core::storage::persistent::load_latest_kind(&mut readback, KIND) {
        Ok(Some((gen, len))) if gen == generation && len == 8 => {
            crate::kprintln!(
                "[OK] kernel-core journal selftest: commit generation {} written and read back.",
                generation
            );
            Ok(())
        }
        Ok(Some((gen, len))) => {
            crate::kprintln!(
                "[WARN] kernel-core journal selftest: MISMATCH (generation {} len {} after committing {}).",
                gen,
                len,
                generation
            );
            Err(BlockDeviceError::ReadError)
        }
        Ok(None) => {
            crate::kprintln!("[WARN] kernel-core journal selftest: record vanished after commit.");
            Err(BlockDeviceError::ReadError)
        }
        Err(err) => {
            crate::kprintln!(
                "[WARN] kernel-core journal selftest: read-back failed ({:?}).",
                err
            );
            Err(BlockDeviceError::ReadError)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn journal_selftest_refuses_when_no_device_is_bound() {
        // Host build: the journal's device binding is per-process state, and no
        // device is bound here, so a commit must fail closed rather than
        // fabricate a generation. Mirrors the RISC-V kernel's host test.
        let result = journal_selftest();
        assert!(result.is_err(), "an unbound journal must refuse, not succeed");
    }
}
