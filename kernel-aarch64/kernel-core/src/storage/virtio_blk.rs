// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

//! Platform-neutral virtio-mmio block device driver.
//!
//! WHY this lives in the shared core: the journal commits through the
//! platform-neutral `BlockDevice` contract (`super::BlockDevice`), and QEMU's
//! `virt` machines expose the same virtio-mmio transport on every architecture
//! this core is linked into — the register interface is identical; only the
//! transport discovery addresses differ, and those are supplied by the
//! architecture kernel that owns its memory map. One driver, three kernels.
//!
//! Device model: virtio-blk over virtio-mmio, negotiated modern (v2) when the
//! device reports version 2 and legacy (v1, queue-pfn) when it reports 1.
//! QEMU's `virtio-blk-device` (used with `-drive if=none -device
//! virtio-blk-device`) is transitional and answers either way.
//!
//! Failure direction: a transport that fails any probe or activation step
//! reports `DeviceNotFound`; a sector outside the device's capacity reports
//! `LbaOutOfRange`; a completion that never arrives reports `Timeout`; a
//! device-reported failure status reports `ReadError`/`WriteError`. No path in
//! this driver manufactures a successful read, because the journal would then
//! commit records into a device that does not exist and report them durable.

use super::{BlockDevice, BlockDeviceError, SECTOR_SIZE};
use core::sync::atomic::{compiler_fence, Ordering};

// virtio-mmio register offsets, shared by version 1 and version 2 devices
// (version 2 adds the 64-bit queue-address and queue-ready registers).
const MAGIC: usize = 0x000;
const VERSION: usize = 0x004;
const DEVICE_ID: usize = 0x008;
const DEVICE_FEATURES: usize = 0x010;
const DEVICE_FEATURES_SEL: usize = 0x014;
const DRIVER_FEATURES: usize = 0x020;
const DRIVER_FEATURES_SEL: usize = 0x024;
const GUEST_PAGE_SIZE: usize = 0x028; // legacy (v1) only
const QUEUE_SEL: usize = 0x030;
const QUEUE_NUM_MAX: usize = 0x034;
const QUEUE_NUM: usize = 0x038;
const QUEUE_ALIGN: usize = 0x03c; // legacy (v1) only
const QUEUE_PFN: usize = 0x040; // legacy (v1) only
const QUEUE_READY: usize = 0x044; // modern (v2) only
const QUEUE_NOTIFY: usize = 0x050;
const INTERRUPT_STATUS: usize = 0x060;
const INTERRUPT_ACK: usize = 0x064;
const STATUS: usize = 0x070;
const QUEUE_DESC_LOW: usize = 0x080; // modern (v2) only
const QUEUE_DESC_HIGH: usize = 0x084;
const QUEUE_AVAIL_LOW: usize = 0x090;
const QUEUE_AVAIL_HIGH: usize = 0x094;
const QUEUE_USED_LOW: usize = 0x0a0;
const QUEUE_USED_HIGH: usize = 0x0a4;
const CONFIG_BASE: usize = 0x100;

const VIRTIO_MMIO_MAGIC: u32 = 0x7472_6976; // "virt", little-endian
pub const VIRTIO_DEV_BLK: u32 = 2;
const VERSION_LEGACY: u32 = 1;
const VERSION_MODERN: u32 = 2;

// Virtio 1.0 feature bit 32; a transitional device requires the driver to
// acknowledge it before the modern queue registers may be used.
const F_VERSION_1: u64 = 1 << 32;

const S_ACKNOWLEDGE: u32 = 1;
const S_DRIVER: u32 = 2;
const S_DRIVER_OK: u32 = 4;
const S_FEATURES_OK: u32 = 8;
const S_FAILED: u32 = 128;

pub const QUEUE_LEN: u32 = 16;
const DESC_HDR: u16 = 0;
const DESC_DATA: u16 = 1;
const DESC_STATUS: u16 = 2;
const F_NEXT: u16 = 1;
const F_WRITE: u16 = 2; // device-writable
/// Legacy devices place the used ring at `align_up(avail_end, QueueAlign)`;
/// announcing QueueAlign = 512 to the device keeps its computed used-ring
/// base exactly at the driver's `QueueRegion::used_offset()`, inside one
/// region. Legacy page size: the PFN register shifts the queue base right by
/// this many bits (GuestPageSize = 4096, a power of two >= the queue size).
pub const LEGACY_USED_ALIGN: u32 = 512;
pub const LEGACY_PAGE_SHIFT: u32 = 12;
pub const LEGACY_PAGE_SIZE: u32 = 1 << LEGACY_PAGE_SHIFT;

const VIRTIO_BLK_T_IN: u32 = 0; // device -> driver (sector read)
const VIRTIO_BLK_T_OUT: u32 = 1; // driver -> device (sector write)
const VIRTIO_BLK_S_OK: u8 = 0;
const VIRTIO_BLK_S_IOERR: u8 = 1;
const VIRTIO_BLK_S_UNSUPP: u8 = 2;

/// Upper bound on completion polling, so a dead transport answers `Timeout`
/// instead of hanging the boot.
pub const POLL_LIMIT: u32 = 4_000_000;

/// Legacy devices lay the used ring at `align_up(avail_end, QUEUE_ALIGN)`.
/// The available ring ends at 292 or 294 bytes depending on which side of the
/// `used_event` boundary the device counts; aligning either to 512 (the
/// natural alignment between the two rings) gives the same offset on every
/// reader, which is what makes one fixed region layout serve both protocol
/// versions.
/// Legacy page shift: the PFN register shifts the queue base right by this
/// many bits, so GuestPageSize must be announced as 1 << 12 = 4096 — a power
/// of two >= the queue size and >= QueueAlign.
/// One virtqueue, laid out the way the legacy PFN mode requires — descriptor
/// table, then available ring, then used ring in a single 4096-byte-aligned
/// region — which the modern address registers accept unchanged.
///
/// Layout math for `QUEUE_LEN = 16`: descriptors 16 x 16 = 256 bytes at 0;
/// available ring `flags` + `idx` + 16 x u16 = 36 bytes at 256 (ends at 292);
/// used ring at `align_up(292, LEGACY_USED_ALIGN)` = 512, `flags` + `idx` +
/// 16 x 8 = 132 bytes; total 644, comfortably inside the 1024-byte region
/// below.
#[repr(C, align(4096))]
pub struct QueueRegion {
    pub bytes: [u8; 1024],
}

impl QueueRegion {
    pub const fn new() -> Self {
        Self {
            bytes: [0u8; 1024],
        }
    }

    /// Offset of the descriptor table inside the region.
    pub const fn desc_offset() -> usize {
        0
    }

    /// Offset of the available ring. 16 descriptors x 16 bytes.
    pub const fn avail_offset() -> usize {
        256
    }

    /// Offset of the used ring. The available ring ends at 292 (without the
    /// trailing `used_event` field, as QEMU's legacy path computes it) or 294
    /// (with it, as the spec layout does); `align_up` of either to
    /// `LEGACY_USED_ALIGN` is 512, so every reading lands here.
    pub const fn used_offset() -> usize {
        512
    }

    /// Total bytes of queue structure actually used.
    pub const fn used_bytes() -> usize {
        Self::used_offset() + 4 + 8 * QUEUE_LEN as usize
    }
}

/// The DMA scratch a request needs: 16-byte request header, 512-byte sector,
/// 1-byte device-written status.
#[repr(C, align(16))]
pub struct SectorScratch {
    pub bytes: [u8; 16 + SECTOR_SIZE + 1],
}

impl SectorScratch {
    pub const fn new() -> Self {
        Self {
            bytes: [0u8; 16 + SECTOR_SIZE + 1],
        }
    }

    pub const HDR_LEN: usize = 16;
    pub const DATA_OFF: usize = 16;
    pub const STATUS_OFF: usize = 16 + SECTOR_SIZE;

    /// Encodes a virtio-blk request header into `out`: little-endian
    /// `{ u32 request_type, u32 reserved, u64 sector }`.
    pub fn encode_header(out: &mut [u8; Self::HDR_LEN], request_type: u32, sector: u64) {
        out[0..4].copy_from_slice(&request_type.to_le_bytes());
        out[4..8].copy_from_slice(&[0; 4]);
        out[8..16].copy_from_slice(&sector.to_le_bytes());
    }

    /// The device's final status byte for a request.
    pub fn decode_status(scratch: &Self) -> u8 {
        scratch.bytes[Self::STATUS_OFF]
    }
}

/// Maps a device-reported virtio-blk status byte to the journal's error
/// vocabulary. `is_read` only chooses which operation a failure is reported
/// against; both directions fail closed.
pub fn map_blk_error(status: u8, is_read: bool) -> BlockDeviceError {
    match status {
        VIRTIO_BLK_S_IOERR => {
            if is_read {
                BlockDeviceError::ReadError
            } else {
                BlockDeviceError::WriteError
            }
        }
        _ => BlockDeviceError::DeviceFault,
    }
}

#[inline(always)]
fn mmio_read32(base: usize, offset: usize) -> u32 {
    compiler_fence(Ordering::SeqCst);
    let v = unsafe { core::ptr::read_volatile((base + offset) as *const u32) };
    compiler_fence(Ordering::SeqCst);
    v
}

#[inline(always)]
fn mmio_write32(base: usize, offset: usize, val: u32) {
    compiler_fence(Ordering::SeqCst);
    unsafe { core::ptr::write_volatile((base + offset) as *mut u32, val) };
    compiler_fence(Ordering::SeqCst);
}

/// Pure probe discriminator: the checks every transport must pass before this
/// driver will touch its queues. Kept free of MMIO so the host test suite can
/// exercise the fail-closed edges directly.
pub fn probe_registers_ok(magic: u32, version: u32, device_id: u32) -> bool {
    magic == VIRTIO_MMIO_MAGIC
        && (version == VERSION_LEGACY || version == VERSION_MODERN)
        && device_id == VIRTIO_DEV_BLK
}

/// The driver-visible feature set this driver acknowledges, as a
/// (selector, value) pair list. Modern devices must see `F_VERSION_1`;
/// legacy devices must not.
pub fn driver_features_for(version: u32) -> [(u32, u32); 2] {
    if version == VERSION_MODERN {
        // F_VERSION_1 lives in the upper word.
        [(1, (F_VERSION_1 >> 32) as u32), (0, 0)]
    } else {
        [(0, 0), (1, 0)]
    }
}

/// Physical page-frame number the legacy queue register expects: the queue
/// base shifted right by the announced GuestPageSize (4096 = 2^12).
pub fn legacy_pfn(region_addr: u64) -> u32 {
    (region_addr >> LEGACY_PAGE_SHIFT) as u32
}

#[repr(C)]
struct Desc {
    addr: u64,
    len: u32,
    flags: u16,
    next: u16,
}

fn write_desc(region: &mut QueueRegion, index: u16, addr: u64, len: u32, flags: u16, next: u16) {
    let off = QueueRegion::desc_offset() + index as usize * core::mem::size_of::<Desc>();
    // SAFETY: `off` is in-bounds for the 1024-byte region for index < QUEUE_LEN,
    // and the region's alignment satisfies `Desc`'s 8-byte alignment.
    let d = unsafe { &mut *(region.bytes.as_mut_ptr().add(off) as *mut Desc) };
    d.addr = addr;
    d.len = len;
    d.flags = flags;
    d.next = next;
}

fn set_avail(region: &mut QueueRegion, avail_idx: u16) {
    let off = QueueRegion::avail_offset();
    region.bytes[off] = 0; // avail.flags: no interrupt suppression
    region.bytes[off + 1] = 0;
    let slot = off + 4 + (avail_idx as usize % QUEUE_LEN as usize) * 2;
    region.bytes[slot..slot + 2].copy_from_slice(&DESC_HDR.to_le_bytes());
    region.bytes[off + 2..off + 4].copy_from_slice(&(avail_idx.wrapping_add(1)).to_le_bytes());
}

/// Reads `used.idx` out of the region after the device has published a
/// completion.
fn used_idx(region: &QueueRegion) -> u16 {
    let off = QueueRegion::used_offset() + 2;
    // Volatile, because the completion poll below re-reads this field against
    // device-written memory: a plain load can be hoisted out of the poll loop,
    // after which a healthy device is reported as a Timeout. Offsets here are
    // even, so the u16 read is aligned.
    u16::from_le(unsafe { core::ptr::read_volatile(region.bytes.as_ptr().add(off) as *const u16) })
}

/// Reads the `id` (head descriptor index) of the used entry at `slot`.
///
/// The used entry's `id` field is 32 bits wide; reading only its low half
/// would report descriptor 0x1_0000 as id 0 and fail every completion check.
fn used_entry_id(region: &QueueRegion, slot: u16) -> u32 {
    let off = QueueRegion::used_offset() + 4 + (slot as usize % QUEUE_LEN as usize) * 8;
    // Volatile: the used ring is device-written memory.
    u32::from_le(unsafe { core::ptr::read_volatile(region.bytes.as_ptr().add(off) as *const u32) })
}

/// A virtio-mmio block device, self-contained: it owns its queue region and
/// DMA scratch so a kernel can hold it in one static and hand the journal a
/// `'static mut`.
pub struct VirtioMmioBlockDevice {
    mmio: usize,
    version: u32,
    total_sectors: u64,
    last_seen_used_idx: u16,
    queue: QueueRegion,
    scratch: SectorScratch,
    probed: bool,
}

// The device owns nothing aliased: its MMIO region is addressable from any hart
// and the journal serializes access through its own mutex.
unsafe impl Send for VirtioMmioBlockDevice {}
unsafe impl Sync for VirtioMmioBlockDevice {}

impl VirtioMmioBlockDevice {
    /// Probes one virtio-mmio transport and, if it is a block device,
    /// negotiates features. The device is not started yet: queue registers
    /// are programmed and DRIVER_OK is set only by `activate`, which the
    /// caller must run once the struct has reached its final address.
    ///
    /// WHY two steps: this struct is typically built in the caller's frame and
    /// then moved into its permanent static. Programming the queue here would
    /// announce the pre-move address as the queue PFN and the device would
    /// DMA into a stale stack page while the real ring sat elsewhere — the
    /// first request would never complete. `activate` re-runs the setup from
    /// the address the struct finally occupies.
    ///
    /// SAFETY: `mmio` must be the physical (identity-mapped) base of a
    /// virtio-mmio transport, 4096 bytes wide.
    pub fn probe(mmio: usize) -> Option<Self> {
        let magic = mmio_read32(mmio, MAGIC);
        let version = mmio_read32(mmio, VERSION);
        let device_id = mmio_read32(mmio, DEVICE_ID);
        if !probe_registers_ok(magic, version, device_id) {
            return None;
        }

        // Reset any state a previous driver left, then acknowledge.
        mmio_write32(mmio, STATUS, 0);
        mmio_write32(mmio, STATUS, S_ACKNOWLEDGE);
        mmio_write32(mmio, STATUS, S_ACKNOWLEDGE | S_DRIVER);

        for (sel, value) in driver_features_for(version) {
            mmio_write32(mmio, DEVICE_FEATURES_SEL, sel);
            let _ = mmio_read32(mmio, DEVICE_FEATURES);
            mmio_write32(mmio, DRIVER_FEATURES_SEL, sel);
            mmio_write32(mmio, DRIVER_FEATURES, value);
        }

        let device = Self {
            mmio,
            version,
            total_sectors: 0,
            last_seen_used_idx: 0,
            queue: QueueRegion::new(),
            scratch: SectorScratch::new(),
            probed: false,
        };
        Some(device)
    }

    /// Brings queue 0 up and starts the device, from the address the struct
    /// occupies NOW. Must be called after the device has been placed at its
    /// final location and before the first request; a device whose caller
    /// never calls it stays fail-closed (every request answers
    /// `DeviceNotFound`). Returns false if the transport refused — queue too
    /// small, or feature negotiation rejected — in which case the caller must
    /// not use the device and may scan the next transport.
    pub fn activate(&mut self) -> bool {
        let mmio = self.mmio;
        if !self.setup_queue() {
            mmio_write32(mmio, STATUS, S_FAILED);
            return false;
        }

        // Capacity is the first field of the virtio-blk configuration:
        // the size in 512-byte sectors, as an aligned u64 at 0x100.
        self.total_sectors = unsafe {
            core::ptr::read_volatile((mmio + CONFIG_BASE) as *const u64)
        };

        // Spec order: FEATURES_OK goes first and must be read back accepted
        // before DRIVER_OK may be set — a combined write would let a device
        // that rejected the features run anyway.
        if self.version == VERSION_MODERN {
            mmio_write32(mmio, STATUS, S_ACKNOWLEDGE | S_DRIVER | S_FEATURES_OK);
            if (mmio_read32(mmio, STATUS) & S_FEATURES_OK) == 0 {
                mmio_write32(mmio, STATUS, S_FAILED);
                return false;
            }
        }
        mmio_write32(mmio, STATUS, S_ACKNOWLEDGE | S_DRIVER | S_DRIVER_OK);

        self.probed = true;
        true
    }

    fn setup_queue(&mut self) -> bool {
        let mmio = self.mmio;
        mmio_write32(mmio, QUEUE_SEL, 0);
        let max = mmio_read32(mmio, QUEUE_NUM_MAX);
        if max == 0 || max < QUEUE_LEN {
            return false;
        }
        mmio_write32(mmio, QUEUE_NUM, QUEUE_LEN);

        // Final address of the queue region: activate() runs only after the
        // caller has placed the struct at its permanent address (see the
        // probe/activate contract in activate's docs), so the device DMAs
        // into live memory.
        let region_addr = &self.queue as *const QueueRegion as u64;
        if self.version == VERSION_MODERN {
            mmio_write32(mmio, QUEUE_DESC_LOW, region_addr as u32);
            mmio_write32(mmio, QUEUE_DESC_HIGH, (region_addr >> 32) as u32);
            let avail = region_addr + QueueRegion::avail_offset() as u64;
            mmio_write32(mmio, QUEUE_AVAIL_LOW, avail as u32);
            mmio_write32(mmio, QUEUE_AVAIL_HIGH, (avail >> 32) as u32);
            let used = region_addr + QueueRegion::used_offset() as u64;
            mmio_write32(mmio, QUEUE_USED_LOW, used as u32);
            mmio_write32(mmio, QUEUE_USED_HIGH, (used >> 32) as u32);
            mmio_write32(mmio, QUEUE_READY, 1);
        } else {
            // Legacy (v1) devices derive every ring address from one PFN:
            // base = PFN << GuestPageSize, avail = base + 256, used =
            // align_up(base + avail_end, QueueAlign). Announce GuestPageSize
            // 4096 and QueueAlign 512 so the device's computed layout matches
            // `QueueRegion` exactly (desc 0, avail 256, used 512).
            mmio_write32(mmio, GUEST_PAGE_SIZE, LEGACY_PAGE_SIZE);
            mmio_write32(mmio, QUEUE_ALIGN, LEGACY_USED_ALIGN);
            mmio_write32(mmio, QUEUE_PFN, legacy_pfn(region_addr));
        }
        true
    }

    /// Submits one single-sector request and polls for its completion.
    fn request(&mut self, request_type: u32, lba: u32) -> Result<(), BlockDeviceError> {
        if !self.probed {
            return Err(BlockDeviceError::DeviceNotFound);
        }
        let is_read = request_type == VIRTIO_BLK_T_IN;
        SectorScratch::encode_header(
            (&mut self.scratch.bytes[0..SectorScratch::HDR_LEN])
                .try_into()
                .expect("header is 16 bytes"),
            request_type,
            lba as u64,
        );
        self.scratch.bytes[SectorScratch::STATUS_OFF] = VIRTIO_BLK_S_UNSUPP;

        let scratch_addr = &self.scratch as *const SectorScratch as u64;
        let data_flags = if is_read { F_WRITE } else { 0 };
        let status_flags = F_WRITE;

        // The device follows the chain from DESC_HDR through `next`.
        write_desc(
            &mut self.queue,
            DESC_HDR,
            scratch_addr,
            SectorScratch::HDR_LEN as u32,
            F_NEXT,
            DESC_DATA,
        );
        write_desc(
            &mut self.queue,
            DESC_DATA,
            scratch_addr + SectorScratch::DATA_OFF as u64,
            SECTOR_SIZE as u32,
            F_NEXT | data_flags,
            DESC_STATUS,
        );
        write_desc(
            &mut self.queue,
            DESC_STATUS,
            scratch_addr + SectorScratch::STATUS_OFF as u64,
            1,
            status_flags,
            0,
        );

        let avail_idx_off = QueueRegion::avail_offset() + 2;
        let avail_idx = u16::from_le_bytes([
            self.queue.bytes[avail_idx_off],
            self.queue.bytes[avail_idx_off + 1],
        ]);
        set_avail(&mut self.queue, avail_idx);
        compiler_fence(Ordering::SeqCst);
        mmio_write32(self.mmio, QUEUE_NOTIFY, 0);

        // Poll the used ring for this request's completion.
        let mut polls = 0u32;
        while used_idx(&self.queue) == self.last_seen_used_idx {
            polls += 1;
            if polls >= POLL_LIMIT {
                // Acknowledge whatever the device raised, and report honestly.
                let pending = mmio_read32(self.mmio, INTERRUPT_STATUS);
                if pending != 0 {
                    mmio_write32(self.mmio, INTERRUPT_ACK, pending);
                }
                // A completion that never arrived may still be in flight: the
                // scratch and descriptors are now ambiguous, so fail this
                // device closed for good rather than let a late completion
                // answer a later request with stale data.
                self.probed = false;
                return Err(BlockDeviceError::Timeout);
            }
        }
        self.last_seen_used_idx = used_idx(&self.queue);
        let pending = mmio_read32(self.mmio, INTERRUPT_STATUS);
        if pending != 0 {
            mmio_write32(self.mmio, INTERRUPT_ACK, pending);
        }

        let slot = self.last_seen_used_idx.wrapping_sub(1);
        if used_entry_id(&self.queue, slot) != u32::from(DESC_HDR) {
            return Err(BlockDeviceError::DeviceFault);
        }
        let status = SectorScratch::decode_status(&self.scratch);
        if status == VIRTIO_BLK_S_OK {
            Ok(())
        } else {
            Err(map_blk_error(status, is_read))
        }
    }
}

impl BlockDevice for VirtioMmioBlockDevice {
    fn read_sector(
        &mut self,
        lba: u32,
        buf: &mut [u8; SECTOR_SIZE],
    ) -> Result<(), BlockDeviceError> {
        if !self.probed {
            return Err(BlockDeviceError::DeviceNotFound);
        }
        if self.total_sectors == 0 || lba as u64 >= self.total_sectors {
            return Err(BlockDeviceError::LbaOutOfRange);
        }
        self.request(VIRTIO_BLK_T_IN, lba)?;
        buf.copy_from_slice(
            &self.scratch.bytes
                [SectorScratch::DATA_OFF..SectorScratch::DATA_OFF + SECTOR_SIZE],
        );
        Ok(())
    }

    fn write_sector(
        &mut self,
        lba: u32,
        buf: &[u8; SECTOR_SIZE],
    ) -> Result<(), BlockDeviceError> {
        if !self.probed {
            return Err(BlockDeviceError::DeviceNotFound);
        }
        if self.total_sectors == 0 || lba as u64 >= self.total_sectors {
            return Err(BlockDeviceError::LbaOutOfRange);
        }
        self.scratch.bytes
            [SectorScratch::DATA_OFF..SectorScratch::DATA_OFF + SECTOR_SIZE]
            .copy_from_slice(buf);
        self.request(VIRTIO_BLK_T_OUT, lba)
    }

    fn sector_count(&self) -> u32 {
        // Zero when no device answered the probe, which is what makes the
        // journal report `Unavailable` rather than compute slot addresses
        // against a device that does not exist.
        if self.probed {
            u32::try_from(self.total_sectors).unwrap_or(0)
        } else {
            0
        }
    }

    fn model(&self) -> &str {
        "virtio-blk"
    }

    // The default `flush` (Ok) is honest here: every `write_sector` above only
    // returns after the device has copied the sector and written the status
    // byte, and QEMU's virtio-blk device has no volatile write cache to flush.
    // A driver for caching hardware must override this.
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn regression_transport_offsets_match_virtio_mmio() {
        assert_eq!(QUEUE_READY, 0x044);
        assert_eq!(QUEUE_NOTIFY, 0x050);
    }

    #[test]
    fn regression_used_id_is_not_truncated() {
        let mut region = QueueRegion::new();
        let entry = QueueRegion::used_offset() + 4;
        region.bytes[entry..entry + 4].copy_from_slice(&0x0001_0000u32.to_le_bytes());
        assert_ne!(used_entry_id(&region, 0) as u32, 0);
    }

    #[test]
    fn probe_discriminator_fails_closed_on_every_bad_edge() {
        assert!(probe_registers_ok(VIRTIO_MMIO_MAGIC, VERSION_LEGACY, VIRTIO_DEV_BLK));
        assert!(probe_registers_ok(VIRTIO_MMIO_MAGIC, VERSION_MODERN, VIRTIO_DEV_BLK));
        // Wrong magic bytes.
        assert!(!probe_registers_ok(0x1234_5678, VERSION_MODERN, VIRTIO_DEV_BLK));
        // Unknown protocol version.
        assert!(!probe_registers_ok(VIRTIO_MMIO_MAGIC, 3, VIRTIO_DEV_BLK));
        // Present transport, wrong device type (console is id 3, rng is 4).
        assert!(!probe_registers_ok(VIRTIO_MMIO_MAGIC, VERSION_MODERN, 0));
        assert!(!probe_registers_ok(VIRTIO_MMIO_MAGIC, VERSION_MODERN, 3));
        assert!(!probe_registers_ok(VIRTIO_MMIO_MAGIC, VERSION_MODERN, 4));
    }

    #[test]
    fn queue_region_layout_matches_the_documented_offsets() {
        assert_eq!(QueueRegion::desc_offset(), 0);
        assert_eq!(QueueRegion::avail_offset(), 16 * 16);
        let used = QueueRegion::used_offset();
        // align_up(avail_end = 292, LEGACY_USED_ALIGN = 512).
        assert_eq!(used, 512);
        assert_eq!(used % 4, 0, "used ring must be 4-byte aligned");
        assert!(QueueRegion::used_bytes() <= 1024, "region overflows");
        assert_eq!(QueueRegion::used_bytes(), 512 + 4 + 8 * 16);
    }

    #[test]
    fn header_encoding_is_little_endian_with_zero_reserved() {
        let mut hdr = [0u8; 16];
        SectorScratch::encode_header(&mut hdr, VIRTIO_BLK_T_OUT, 0x1234_5678_9abc);
        assert_eq!(&hdr[0..4], &1u32.to_le_bytes());
        assert_eq!(&hdr[4..8], &[0; 4], "reserved must stay zero");
        assert_eq!(&hdr[8..16], &0x1234_5678_9abcu64.to_le_bytes());

        SectorScratch::encode_header(&mut hdr, VIRTIO_BLK_T_IN, 0);
        assert_eq!(&hdr[0..4], &0u32.to_le_bytes());
        assert_eq!(&hdr[8..16], &0u64.to_le_bytes());
    }

    #[test]
    fn scratch_offsets_sum_to_one_sector_plus_framing() {
        assert_eq!(SectorScratch::DATA_OFF, 16);
        assert_eq!(SectorScratch::STATUS_OFF, 16 + 512);
        assert_eq!(SectorScratch::STATUS_OFF + 1, SectorScratch::new().bytes.len());
    }

    #[test]
    fn a_timed_out_device_fails_closed_and_reports_device_not_found() {
        // A never-activated device (probed=false, the post-Timeout state) must
        // refuse every request with DeviceNotFound — a stale in-flight
        // completion must never answer a later request with ambiguous data.
        let mut device = VirtioMmioBlockDevice {
            mmio: 0,
            version: VERSION_LEGACY,
            total_sectors: 131_072,
            last_seen_used_idx: 0,
            queue: QueueRegion::new(),
            scratch: SectorScratch::new(),
            probed: false,
        };
        assert!(matches!(
            device.request(VIRTIO_BLK_T_IN, 0),
            Err(BlockDeviceError::DeviceNotFound)
        ));
        assert!(matches!(
            device.request(VIRTIO_BLK_T_OUT, 0),
            Err(BlockDeviceError::DeviceNotFound)
        ));
    }

    #[test]
    fn device_errors_map_by_direction_and_never_to_success() {
        assert!(matches!(map_blk_error(VIRTIO_BLK_S_IOERR, true), BlockDeviceError::ReadError));
        assert!(matches!(
            map_blk_error(VIRTIO_BLK_S_IOERR, false),
            BlockDeviceError::WriteError
        ));
        assert!(matches!(map_blk_error(VIRTIO_BLK_S_UNSUPP, true), BlockDeviceError::DeviceFault));
        assert!(matches!(map_blk_error(0xFF, false), BlockDeviceError::DeviceFault));
        // An OK status must never come back from the error mapper.
        assert!(!matches!(map_blk_error(VIRTIO_BLK_S_OK, true), BlockDeviceError::ReadError));
    }

    #[test]
    fn feature_acknowledgement_matches_the_protocol_version() {
        // Modern: acknowledge F_VERSION_1 (upper word), nothing in the lower.
        let modern = driver_features_for(VERSION_MODERN);
        assert_eq!(modern[0], (1, (F_VERSION_1 >> 32) as u32));
        assert_eq!(modern[1], (0, 0));
        // Legacy: no features acknowledged at all.
        let legacy = driver_features_for(VERSION_LEGACY);
        assert_eq!(legacy, [(0, 0), (1, 0)]);
    }

    #[test]
    fn legacy_pfn_is_a_twelve_bit_right_shift() {
        assert_eq!(legacy_pfn(0x8020_1000), 0x0802_01);
        assert_eq!(legacy_pfn(4096), 1);
        assert_eq!(legacy_pfn(0), 0);
    }

    #[test]
    fn avail_ring_publishes_head_and_advances_index_wrapping_at_queue_len() {
        let off = QueueRegion::avail_offset();

        let mut region = QueueRegion::new();
        set_avail(&mut region, 0);
        // Ring slot 0 carries the head descriptor index.
        assert_eq!(&region.bytes[off + 4..off + 6], &0u16.to_le_bytes());
        // The free-running index advanced to 1.
        assert_eq!(&region.bytes[off + 2..off + 4], &1u16.to_le_bytes());

        // Index 16 wraps back to ring slot 0.
        let mut region = QueueRegion::new();
        set_avail(&mut region, QUEUE_LEN as u16);
        assert_eq!(&region.bytes[off + 4..off + 6], &0u16.to_le_bytes());
        assert_eq!(&region.bytes[off + 2..off + 4], &17u16.to_le_bytes());
    }

    #[test]
    fn used_ring_reader_reads_the_published_index_and_entry_id() {
        let mut region = QueueRegion::new();
        let used = QueueRegion::used_offset();
        region.bytes[used + 2..used + 4].copy_from_slice(&7u16.to_le_bytes());
        assert_eq!(used_idx(&region), 7);

        let entry = used + 4 + 3 * 8;
        region.bytes[entry..entry + 2].copy_from_slice(&DESC_HDR.to_le_bytes());
        assert_eq!(used_entry_id(&region, 3), u32::from(DESC_HDR));
    }

    #[test]
    fn sector_count_reports_zero_only_when_unprobed() {
        let mut device = VirtioMmioBlockDevice {
            mmio: 0,
            version: VERSION_MODERN,
            total_sectors: 0,
            last_seen_used_idx: 0,
            queue: QueueRegion::new(),
            scratch: SectorScratch::new(),
            probed: false,
        };
        assert_eq!(device.sector_count(), 0);
        assert!(matches!(
            device.read_sector(0, &mut [0u8; SECTOR_SIZE]),
            Err(BlockDeviceError::DeviceNotFound)
        ));
        assert!(matches!(
            device.write_sector(0, &[0u8; SECTOR_SIZE]),
            Err(BlockDeviceError::DeviceNotFound)
        ));

        device.probed = true;
        device.total_sectors = u64::from(u32::MAX) + 10;
        // Capacities beyond u32 clamp to zero, which reports Unavailable
        // rather than a silently truncated bound.
        assert_eq!(device.sector_count(), 0);

        device.total_sectors = 1000;
        assert_eq!(device.sector_count(), 1000);
        // The capacity bound is enforced before any MMIO is touched, so an
        // out-of-range LBA is refused even on a transport that never answers.
        assert!(matches!(
            device.read_sector(1000, &mut [0u8; SECTOR_SIZE]),
            Err(BlockDeviceError::LbaOutOfRange)
        ));
        assert!(matches!(
            device.write_sector(u32::MAX, &[0u8; SECTOR_SIZE]),
            Err(BlockDeviceError::LbaOutOfRange)
        ));
    }
}
