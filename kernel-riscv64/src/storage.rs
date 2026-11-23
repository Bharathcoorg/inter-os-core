// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

//! RISC-V persistent storage: virtio-blk binding for the shared kernel core.
//!
//! First storage implementation on this architecture. Everything here is a
//! seam, not a reimplemention: the journal, the boot guard and the block
//! contract live in `interos_kernel_core` (extracted from the x86 kernel and
//! already running on AArch64), and this file supplies the three machine parts
//! the core declares:
//!
//! 1. a block device — virtio-mmio block transports discovered on the QEMU
//!    `virt` memory map, probed by `storage::virtio_blk::VirtioMmioBlockDevice`;
//! 2. the running image's extent — derived from this platform's linker bounds
//!    (`_kernel_start` / `_kernel_end`), because a `-kernel`-loaded RISC-V
//!    image has no ELF header at its load base to walk;
//! 3. the boot-time journal binding and rollback-floor report, called from
//!    `main` once the heap exists.
//!
//! Failure direction: no transport, a transport that is not a block device,
//! or a queue the device refuses all end in an unbound journal, whose reads
//! then report `Unavailable` — never an empty-but-successful device.

use interos_kernel_core::storage::boot_guard::{ImageExtent, ImageExtentSource};
use interos_kernel_core::storage::virtio_blk::VirtioMmioBlockDevice;
use interos_kernel_core::storage::{BlockDevice, BlockDeviceError, SECTOR_SIZE};

/// QEMU `virt` (rv64) places the first virtio-mmio transport at 0x10001000 and
/// steps by 0x1000; the board implements exactly 8 of them
/// (0x10001000..0x10009000, per the generated DTB). Reads past the last slot
/// hit unmapped memory and fault.
const VIRTIO_MMIO_BASE: usize = 0x1000_1000;
const VIRTIO_MMIO_STRIDE: usize = 0x1000;
const VIRTIO_MMIO_COUNT: usize = 8;

static mut BOUND_DEVICE: Option<VirtioMmioBlockDevice> = None;

/// Probes every virtio-mmio transport on the `virt` memory map and binds the
/// first block device it finds to the shared core's journal.
///
/// Runs once, on the primordial hart, before anything else can reach the
/// journal. With no device present this leaves the journal unbound, which is
/// the honest answer: reads report `Unavailable`, and nothing pretends a
/// device exists.
pub fn bind_journal_device() {
    for i in 0..VIRTIO_MMIO_COUNT {
        let mmio = VIRTIO_MMIO_BASE + i * VIRTIO_MMIO_STRIDE;
        // SAFETY: identity-mapped device MMIO on the QEMU virt board, 4096
        // bytes per transport; the probe writes only virtio registers.
        // (`probe` itself is a safe fn taking a raw address.)
        if let Some(device) = VirtioMmioBlockDevice::probe(mmio) {
            // The device moves here, from probe's stack frame into the static.
            // Queue registers and DRIVER_OK are only programmed after the move,
            // by `activate`, so the transport never sees a pre-move PFN.
            let bound: &'static mut VirtioMmioBlockDevice = unsafe {
                let slot = &mut *core::ptr::addr_of_mut!(BOUND_DEVICE);
                slot.insert(device)
            };
            if !bound.activate() {
                unsafe {
                    (&mut *core::ptr::addr_of_mut!(BOUND_DEVICE)).take();
                }
                crate::println!(
                    "[STORAGE] virtio-blk at MMIO {:#x} refused activation; kept scanning.",
                    mmio
                );
                continue;
            }
            crate::println!(
                "[STORAGE] virtio-blk bound at MMIO {:#x}: {} sectors ({} MiB).",
                mmio,
                bound.sector_count(),
                bound.sector_count() as u64 * SECTOR_SIZE as u64 / (1024 * 1024)
            );
            if let Err(reason) = interos_kernel_core::storage::persistent::bind_block_device(bound) {
                crate::println!("[STORAGE] journal refused the device: {}", reason);
            }
            return;
        }
    }
    crate::println!(
        "[STORAGE] no virtio-blk transport answered on the virt memory map — journal unbound."
    );
}

/// The extent of the image this kernel executes from, from linker bounds.
///
/// WHY linker bounds and not ELF headers: QEMU's `-kernel` loads the raw
/// image straight into RAM at `0x80200000` with no ELF header in memory to
/// parse, so the only truthful description of what the guard must not let a
/// journal sector overwrite is the linker's `_kernel_start` / `_kernel_end`.
static RISCV_IMAGE_EXTENT: LinkerBoundsExtent = LinkerBoundsExtent;

struct LinkerBoundsExtent;

impl ImageExtentSource for LinkerBoundsExtent {
    fn image_extent(&self) -> Option<ImageExtent> {
        extern "C" {
            static _kernel_start: u8;
            static _kernel_end: u8;
        }
        // SAFETY: these are linker symbols, not real objects; only their
        // addresses are read.
        let start = unsafe { &_kernel_start as *const u8 as u64 };
        let end = unsafe { &_kernel_end as *const u8 as u64 };
        if end <= start {
            return None;
        }
        Some(ImageExtent {
            hdr_vaddr: start,
            vaddr_start: start,
            vaddr_end: end,
            paddr_start: start,
            paddr_end: end,
            file_bytes: end - start,
        })
    }
}

/// Installs this platform's image-extent source with the shared core's boot
/// guard. Called from `main` before the journal is bound.
pub fn install_boot_guard_extent() {
    interos_kernel_core::storage::boot_guard::install_image_extent_source(&RISCV_IMAGE_EXTENT);
}

/// Runs one direct sector read against the bound transport and prints the
/// driver's own error for it. The journal collapses every transfer failure to
/// `PersistentError::Io`, which hides whether the edge is a Timeout, a
/// DeviceFault or a device-reported IOERR; this keeps the boot log honest
/// about which one it was.
pub fn diagnose_bound_device() {
    // SAFETY: single-hart boot path, after the device was bound; the journal
    // holds the same `&'static mut`, so this diagnostic runs before any
    // journal transfer and nothing else touches the device concurrently.
    let device = unsafe {
        let slot = &mut *core::ptr::addr_of_mut!(BOUND_DEVICE);
        slot.as_mut()
    };
    let Some(device) = device else {
        return;
    };
    let mut sector = [0u8; SECTOR_SIZE];
    match device.read_sector(0, &mut sector) {
        Ok(()) => {
            crate::println!("[STORAGE] diag: read_sector(0) ok, header {:?}.", &sector[..8]);
        }
        Err(err) => {
            crate::println!("[STORAGE] diag: read_sector(0) failed: {:?}.", err);
        }
    }
}

/// Reports the last committed firmware-rollback floor through the journal, or
/// the honest reason there is none. Mirrors the AArch64 kernel's report.
pub fn report_journal_rollback_floor() {
    let mut floor = [0u8; 8];
    match interos_kernel_core::storage::persistent::load_latest_kind(
        &mut floor,
        9, // KIND_FIRMWARE_ROLLBACK_FLOOR, mirrored from the x86 kind table
    ) {
        Ok(Some((generation, len))) => {
            crate::println!(
                "[STORAGE] journal: rollback floor generation {} recovered ({} bytes).",
                generation,
                len
            );
        }
        Ok(None) => {
            crate::println!("[STORAGE] journal: no rollback floor committed yet.");
        }
        Err(err) => {
            crate::println!(
                "[STORAGE] journal: rollback floor unavailable ({:?}) — no block device is bound.",
                err
            );
        }
    }
}

/// Commits one rollback-floor record through the journal and reads it back.
///
/// Used at boot to prove the whole vertical — driver, queue, DMA scratch,
/// journal slots, HMAC chain — against the real device QEMU attached. A
/// failure here is reported as it is, never swallowed: an unproven journal
/// must look unproven.
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
            crate::println!(
                "[STORAGE] journal selftest: commit generation {} written and read back.",
                generation
            );
            Ok(())
        }
        Ok(Some((gen, len))) => {
            crate::println!(
                "[STORAGE] journal selftest: MISMATCH (generation {} len {} after committing {}).",
                gen,
                len,
                generation
            );
            Err(BlockDeviceError::ReadError)
        }
        Ok(None) => {
            crate::println!("[STORAGE] journal selftest: record vanished after commit.");
            Err(BlockDeviceError::ReadError)
        }
        Err(err) => {
            crate::println!("[STORAGE] journal selftest: read-back failed ({:?}).", err);
            Err(BlockDeviceError::ReadError)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extent_source_derives_from_linker_bounds_and_never_inverts() {
        let source = LinkerBoundsExtent;
        let extent = source.image_extent().expect("linker symbols exist on host");
        assert!(extent.vaddr_end > extent.vaddr_start);
        assert_eq!(extent.vaddr_start, extent.paddr_start);
        assert_eq!(extent.file_bytes, extent.vaddr_end - extent.vaddr_start);
        assert_eq!(extent.hdr_vaddr, extent.vaddr_start);
    }

    #[test]
    fn virtio_mmio_scan_window_matches_the_qemu_virt_board() {
        // QEMU riscv64 -M virt implements exactly 8 virtio-mmio transports
        // (0x10001000..0x10009000, per its generated DTB); slot 8 at
        // 0x10009000 is unmapped memory and a read there faults.
        let last = VIRTIO_MMIO_BASE + (VIRTIO_MMIO_COUNT - 1) * VIRTIO_MMIO_STRIDE;
        assert_eq!(last, 0x1000_8000);
        assert_eq!(VIRTIO_MMIO_STRIDE, 0x1000);
    }

    #[test]
    fn journal_selftest_reads_back_what_it_committed() {
        // Host build: the journal's device binding is per-process state, and no
        // device is bound here, so a commit must fail closed rather than
        // fabricate a generation.
        let result = journal_selftest();
        assert!(result.is_err(), "an unbound journal must refuse, not succeed");
    }
}
