//! PCI Audio Hardware Controller Driver
//!
//! Enumerates PCI audio controllers (Intel HDA, AC'97, Ensoniq) and implements a
//! real AC'97 PCM-output path: PCI discovery, NAM (Native Audio Mixer) codec
//! programming, NABM (Native Audio Bus Master) buffer-descriptor-list playback,
//! and start/stop/status control for 16-bit stereo 48 kHz PCM.
//!
//! ## What is verified, and what is not
//!
//! The register layout below comes from the AC'97 2.3 specification as
//! implemented by QEMU's `AC97` device (`hw/audio/ac97.c`): the NABM PCM-out
//! block starts at 0x10, its control register is at 0x1B, and a buffer
//! descriptor is an 8-byte `{ u32 address; u32 control; }` record whose low 16
//! control bits are a length in 16-bit samples. Unit tests exercise that
//! encoding and the register state machine against an in-memory register file.
//! Nothing here is hardware-verified: no physical AC'97 codec has been bound,
//! and no test claims audible output.
//!
//! ## Honesty when the card is absent
//!
//! [`Ac97Controller::probe`] returns [`Ac97Error::Unavailable`] when no
//! controller answers. Every constructor and programming entry point then
//! returns a typed error; there is no "pretend running" state, and the kernel
//! logs the absence rather than reporting playback.

use alloc::vec::Vec;
use core::sync::atomic::Ordering;
use x86_64::instructions::port::Port;
use x86_64::structures::paging::{PhysFrame, Size4KiB};

use crate::audio::pcm::{DmaBufferDescriptor, PcmStream};
use crate::drivers::pci::{self, PciAddress, PciDevice};

/// Intel AC'97 vendor / device pair used by QEMU's `-device AC97`.
pub const AC97_INTEL_VENDOR: u16 = 0x8086;
pub const AC97_INTEL_DEVICE: u16 = 0x2415;

/// The only PCM format this driver programs: 16-bit signed stereo at 48 kHz.
pub const AC97_SAMPLE_RATE_HZ: u32 = 48_000;
pub const AC97_CHANNELS: usize = 2;
pub const AC97_BITS_PER_SAMPLE: u8 = 16;
pub const AC97_FRAME_BYTES: usize = AC97_CHANNELS * 2;

/// Number of buffer descriptors in one playback batch.
pub const AC97_BDL_ENTRIES: usize = 16;
/// Frames held by one descriptor's buffer (2048 bytes per descriptor).
pub const AC97_CHUNK_FRAMES: usize = 512;
pub const AC97_CHUNK_BYTES: usize = AC97_CHUNK_FRAMES * AC97_FRAME_BYTES;

/// Native Audio Mixer (BAR0) registers.
const NAM_RESET: u16 = 0x00;
const NAM_MASTER_VOLUME: u16 = 0x02;
const NAM_PCM_OUT_VOLUME: u16 = 0x18;
const NAM_PCM_FRONT_DAC_RATE: u16 = 0x2C;
const NAM_VENDOR_ID1: u16 = 0x7C;
const NAM_VENDOR_ID2: u16 = 0x7E;

/// Native Audio Bus Master (BAR1) PCM-out registers.
const NABM_PO_BDBAR: u16 = 0x10;
const NABM_PO_LVI: u16 = 0x15;
const NABM_PO_SR: u16 = 0x16;
const NABM_PO_CR: u16 = 0x1B;
const NABM_GLOB_CNT: u16 = 0x2C;

/// PCM-out control/status bits (AC'97 2.3, as implemented by QEMU).
const PO_CR_RPBM: u8 = 0x01;
const PO_CR_RR: u8 = 0x02;
const PO_SR_DCH: u16 = 0x01;
const PO_SR_LVBCI: u16 = 0x04;
const PO_SR_BCIS: u16 = 0x08;
const PO_SR_FIFOE: u16 = 0x10;
const PO_SR_WCLEAR: u16 = PO_SR_LVBCI | PO_SR_BCIS | PO_SR_FIFOE;

/// Errors returned by the AC'97 path.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Ac97Error {
    /// No controller answered, or the codec did not identify itself.
    Unavailable,
    /// A register read-back disagreed with the value programmed.
    RegisterMismatch,
    /// Physically contiguous DMA memory could not be allocated.
    DmaUnavailable,
    /// A descriptor length or index was outside the hardware bounds.
    InvalidDescriptor,
}

/// Snapshot of the controller's real register state and counters.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct Ac97Status {
    /// The controller was discovered and its codec initialized.
    pub present: bool,
    /// A batch is currently playing (hardware run bit set and not yet idle).
    pub running: bool,
    /// Frames handed to the controller since it was initialized.
    pub frames_submitted: u64,
    /// Frames the controller reported finishing, at descriptor granularity.
    pub frames_completed: u64,
    /// The device went idle during *this* poll. The caller decides whether that
    /// was an underrun (no queued frames) or a normal batch boundary.
    pub just_idle: bool,
}

/// Byte-level port I/O used by the AC'97 register layer.
///
/// WHY a trait: it lets the register state machine be tested against an
/// in-memory register file on the host. The real implementation below is the
/// only one that reaches hardware.
pub trait Ac97Io {
    fn read8(&mut self, port: u16) -> u8;
    fn read16(&mut self, port: u16) -> u16;
    fn read32(&mut self, port: u16) -> u32;
    fn write8(&mut self, port: u16, value: u8);
    fn write16(&mut self, port: u16, value: u16);
    fn write32(&mut self, port: u16, value: u32);
}

/// x86 port-I/O implementation for a real AC'97 controller.
pub struct PortAc97Io;

impl Ac97Io for PortAc97Io {
    fn read8(&mut self, port: u16) -> u8 {
        let mut p: Port<u8> = Port::new(port);
        unsafe { p.read() }
    }

    fn read16(&mut self, port: u16) -> u16 {
        let mut p: Port<u16> = Port::new(port);
        unsafe { p.read() }
    }

    fn read32(&mut self, port: u16) -> u32 {
        let mut p: Port<u32> = Port::new(port);
        unsafe { p.read() }
    }

    fn write8(&mut self, port: u16, value: u8) {
        let mut p: Port<u8> = Port::new(port);
        unsafe { p.write(value) }
    }

    fn write16(&mut self, port: u16, value: u16) {
        let mut p: Port<u16> = Port::new(port);
        unsafe { p.write(value) }
    }

    fn write32(&mut self, port: u16, value: u32) {
        let mut p: Port<u32> = Port::new(port);
        unsafe { p.write(value) }
    }
}

/// The AC'97 mixer and bus-master registers, independent of DMA storage.
///
/// This type owns no buffers, so it can be driven by an in-memory `Ac97Io` in
/// tests while the real controller supplies [`PortAc97Io`].
pub struct Ac97Registers<I: Ac97Io> {
    io: I,
    nam_base: u16,
    nabm_base: u16,
    ready: bool,
    running: bool,
    batch_frames: u32,
    frames_submitted: u64,
    frames_completed: u64,
}

impl<I: Ac97Io> Ac97Registers<I> {
    pub fn new(io: I, nam_base: u16, nabm_base: u16) -> Self {
        Self {
            io,
            nam_base,
            nabm_base,
            ready: false,
            running: false,
            batch_frames: 0,
            frames_submitted: 0,
            frames_completed: 0,
        }
    }

    /// Initializes the codec and leaves the PCM-out bus master idle.
    pub fn initialize(&mut self) -> Result<(), Ac97Error> {
        let vendor1 = self.io.read16(self.nam_base + NAM_VENDOR_ID1);
        let vendor2 = self.io.read16(self.nam_base + NAM_VENDOR_ID2);
        // WHY 0xFFFF/0: an absent or unresponsive codec floats the mixer bus
        // high (or reads back zero). Both mean "no codec answered", not "a
        // codec with an unknown vendor".
        if vendor1 == 0xFFFF || vendor1 == 0x0000 || vendor2 == 0xFFFF {
            return Err(Ac97Error::Unavailable);
        }

        // Cold-reset the codec, then program the one format this driver emits.
        self.io.write16(self.nam_base + NAM_RESET, 0x0000);
        self.io.write16(
            self.nam_base + NAM_PCM_FRONT_DAC_RATE,
            AC97_SAMPLE_RATE_HZ as u16,
        );
        // WHY read back: a codec without variable-rate audio silently keeps its
        // default rate. Checking the value is the difference between "48 kHz
        // programmed" and "48 kHz assumed".
        if self.io.read16(self.nam_base + NAM_PCM_FRONT_DAC_RATE) != AC97_SAMPLE_RATE_HZ as u16 {
            return Err(Ac97Error::RegisterMismatch);
        }
        // QEMU's mixer reset leaves the master output muted; unmute both stages
        // at 0 dB attenuation so programmed PCM is not discarded by the mixer.
        self.io.write16(self.nam_base + NAM_MASTER_VOLUME, 0x0000);
        self.io.write16(self.nam_base + NAM_PCM_OUT_VOLUME, 0x0000);

        // Polling driver: leave the global interrupt enable clear.
        self.io.write32(self.nabm_base + NABM_GLOB_CNT, 0);

        // Clear stale completion bits, then reset the PCM-out bus master.
        self.io.write16(self.nabm_base + NABM_PO_SR, PO_SR_WCLEAR);
        self.io.write8(self.nabm_base + NABM_PO_CR, PO_CR_RR);
        // The reset leaves the run bit clear and DCH set, which is the only
        // state in which a batch may be programmed.
        if self.io.read16(self.nabm_base + NABM_PO_SR) & PO_SR_DCH == 0 {
            return Err(Ac97Error::RegisterMismatch);
        }

        self.ready = true;
        Ok(())
    }

    pub fn is_ready(&self) -> bool {
        self.ready
    }

    pub fn is_running(&self) -> bool {
        self.running
    }

    /// Programs a full batch: BDBAR, last valid index, then the run bit.
    ///
    /// `bdl_phys` must be 4-byte aligned (the controller ignores the low two
    /// address bits) and `total_frames` is the exact number of frames described
    /// by the entries `0..=last_index`.
    pub fn program_batch(
        &mut self,
        bdl_phys: u32,
        last_index: u8,
        total_frames: u32,
    ) -> Result<(), Ac97Error> {
        if !self.ready {
            return Err(Ac97Error::Unavailable);
        }
        if last_index as usize >= AC97_BDL_ENTRIES || total_frames == 0 {
            return Err(Ac97Error::InvalidDescriptor);
        }

        // A finished batch leaves RPBM set in hardware. Force idle first so the
        // new BDBAR/LVI pair cannot be observed halfway through a descriptor.
        self.io.write8(self.nabm_base + NABM_PO_CR, 0);
        self.io.write8(self.nabm_base + NABM_PO_CR, PO_CR_RR);
        self.io.write32(self.nabm_base + NABM_PO_BDBAR, bdl_phys & !0x3);
        self.io.write8(self.nabm_base + NABM_PO_LVI, last_index);
        self.io.write16(self.nabm_base + NABM_PO_SR, PO_SR_WCLEAR);
        self.io.write8(self.nabm_base + NABM_PO_CR, PO_CR_RPBM);

        if self.io.read8(self.nabm_base + NABM_PO_CR) & PO_CR_RPBM == 0 {
            return Err(Ac97Error::RegisterMismatch);
        }

        self.running = true;
        self.batch_frames = total_frames;
        self.frames_submitted = self.frames_submitted.saturating_add(total_frames as u64);
        Ok(())
    }

    /// Stops playback. Frames in the current batch are not counted as consumed.
    pub fn stop(&mut self) {
        if self.ready {
            self.io.write8(self.nabm_base + NABM_PO_CR, 0);
            self.io.write16(self.nabm_base + NABM_PO_SR, PO_SR_WCLEAR);
        }
        self.running = false;
        self.batch_frames = 0;
    }

    /// Reads the status register and accounts a finished batch exactly once.
    pub fn poll(&mut self) -> Ac97Status {
        let mut just_idle = false;
        if self.ready {
            let status = self.io.read16(self.nabm_base + NABM_PO_SR);
            if self.running && status & PO_SR_DCH != 0 {
                self.frames_completed = self
                    .frames_completed
                    .saturating_add(self.batch_frames as u64);
                self.batch_frames = 0;
                self.running = false;
                just_idle = true;
            }
        }
        Ac97Status {
            present: self.ready,
            running: self.running,
            frames_submitted: self.frames_submitted,
            frames_completed: self.frames_completed,
            just_idle,
        }
    }

    pub fn status(&self) -> Ac97Status {
        Ac97Status {
            present: self.ready,
            running: self.running,
            frames_submitted: self.frames_submitted,
            frames_completed: self.frames_completed,
            just_idle: false,
        }
    }
}

/// Physically contiguous DMA storage owned by the controller.
struct Ac97Dma {
    bdl_frames: Vec<PhysFrame<Size4KiB>>,
    buffer_frames: Vec<PhysFrame<Size4KiB>>,
}

/// A discovered AC'97 controller with its descriptor list and PCM buffers.
pub struct Ac97Controller<I: Ac97Io> {
    regs: Ac97Registers<I>,
    dma: Ac97Dma,
    pub pci_address: PciAddress,
    pub vendor_id: u16,
    pub device_id: u16,
}

// WHY these impls: the controller owns raw DMA pointers behind `Vec<PhysFrame>`
// and is stored in a kernel-wide `Mutex`. The pointers are only dereferenced
// while the mutex is held, so moving the value between threads is sound.
unsafe impl<I: Ac97Io + Send> Send for Ac97Controller<I> {}
unsafe impl<I: Ac97Io + Send> Sync for Ac97Controller<I> {}

impl Ac97Controller<PortAc97Io> {
    /// Discovers an AC'97 controller on the PCI bus and initializes it.
    ///
    /// Prefers the Intel 8086:2415 pair QEMU exposes, then accepts any device
    /// the PCI class scan identifies as AC'97. Absence is `Unavailable`.
    pub fn probe() -> Result<Self, Ac97Error> {
        let devices = pci::scan_pci_bus();
        let device = devices
            .iter()
            .find(|d| d.vendor_id == AC97_INTEL_VENDOR && d.device_id == AC97_INTEL_DEVICE)
            .or_else(|| devices.iter().find(|d| d.is_ac97()))
            .cloned()
            .ok_or(Ac97Error::Unavailable)?;
        Self::from_pci(device, PortAc97Io)
    }
}

impl<I: Ac97Io> Ac97Controller<I> {
    /// Binds a controller described by `device`, allocating its DMA storage.
    pub fn from_pci(device: PciDevice, io: I) -> Result<Self, Ac97Error> {
        // WHY bit 0 and bit 2: both AC'97 BARs are I/O-space BARs, and the
        // buffer descriptor list is read by bus-master DMA. Memory decoding
        // alone would leave the codec visible but unable to fetch PCM.
        let mut command = pci::pci_read_u16(device.addr, 0x04);
        command |= 0x0001 | 0x0004;
        pci::pci_write_u16(device.addr, 0x04, command);
        if pci::pci_read_u16(device.addr, 0x04) & 0x0005 != 0x0005 {
            return Err(Ac97Error::Unavailable);
        }

        let nam_bar = pci::pci_read_u32(device.addr, 0x10);
        let nabm_bar = pci::pci_read_u32(device.addr, 0x14);
        if nam_bar & 0x1 == 0 || nabm_bar & 0x1 == 0 {
            return Err(Ac97Error::Unavailable);
        }
        let nam_base = (nam_bar & !0x3) as u16;
        let nabm_base = (nabm_bar & !0x3) as u16;
        if nam_base == 0 || nabm_base == 0 {
            return Err(Ac97Error::Unavailable);
        }

        let bdl_frames =
            crate::memory::allocate_dma_frames(1).ok_or(Ac97Error::DmaUnavailable)?;
        let buffer_pages = (AC97_BDL_ENTRIES * AC97_CHUNK_BYTES + 0xFFF) / 0x1000;
        let buffer_frames = crate::memory::allocate_dma_frames(buffer_pages)
            .ok_or(Ac97Error::DmaUnavailable)?;

        // AC'97 buffer descriptor addresses are 32-bit. A machine with more
        // than 4 GiB can hand out a frame the controller cannot reach, so
        // refuse the binding instead of truncating the address and having the
        // DMA engine read unrelated memory.
        let bdl_phys = bdl_frames[0].start_address().as_u64();
        let buffer_phys = buffer_frames[0].start_address().as_u64();
        let buffer_span = (AC97_BDL_ENTRIES * AC97_CHUNK_BYTES) as u64;
        let buffer_end = buffer_phys
            .checked_add(buffer_span)
            .ok_or(Ac97Error::DmaUnavailable)?;
        if bdl_phys > u32::MAX as u64 || buffer_end > u32::MAX as u64 + 1 {
            return Err(Ac97Error::DmaUnavailable);
        }

        let mut regs = Ac97Registers::new(io, nam_base, nabm_base);
        regs.initialize()?;

        Ok(Self {
            regs,
            dma: Ac97Dma {
                bdl_frames,
                buffer_frames,
            },
            pci_address: device.addr,
            vendor_id: device.vendor_id,
            device_id: device.device_id,
        })
    }

    fn bdl_physical(&self) -> u32 {
        self.dma.bdl_frames[0].start_address().as_u64() as u32
    }

    fn buffer_physical(&self) -> u32 {
        self.dma.buffer_frames[0].start_address().as_u64() as u32
    }

    fn bdl_ptr(&self) -> *mut DmaBufferDescriptor {
        crate::memory::physical_to_virtual(self.dma.bdl_frames[0].start_address())
            .as_mut_ptr::<DmaBufferDescriptor>()
    }

    fn buffer_ptr(&self) -> *mut u8 {
        crate::memory::physical_to_virtual(self.dma.buffer_frames[0].start_address()).as_mut_ptr()
    }

    /// Moves queued frames from `stream` into a new batch and starts it.
    ///
    /// Returns the number of frames handed to the controller. Returns `Ok(0)`
    /// when a batch is still running or the stream has nothing queued; it never
    /// starts the device on an empty buffer.
    pub fn pump(&mut self, stream: &mut PcmStream) -> Result<usize, Ac97Error> {
        let status = self.regs.poll();
        if status.running || stream.queued_frames() == 0 {
            return Ok(0);
        }

        let mut total_frames = 0usize;
        let mut last_index = None;
        for index in 0..AC97_BDL_ENTRIES {
            let offset = index * AC97_CHUNK_BYTES;
            // SAFETY: `buffer_ptr` covers exactly AC97_BDL_ENTRIES * CHUNK_BYTES
            // contiguous frames allocated for this controller, and `index` is
            // bounded by that count.
            let buffer = unsafe {
                core::slice::from_raw_parts_mut(self.buffer_ptr().add(offset), AC97_CHUNK_BYTES)
            };
            let frames = stream.read_into(buffer, AC97_CHUNK_FRAMES);
            if frames == 0 {
                break;
            }
            let descriptor = DmaBufferDescriptor::ac97(
                self.buffer_physical() + offset as u32,
                frames,
                AC97_CHANNELS,
            )
            .map_err(|_| Ac97Error::InvalidDescriptor)?;
            // SAFETY: the descriptor list is one allocated frame, which holds
            // AC97_BDL_ENTRIES 8-byte entries, and `index` is below that.
            unsafe { *self.bdl_ptr().add(index) = descriptor; }
            total_frames += frames;
            last_index = Some(index);
        }

        let Some(last_index) = last_index else {
            return Ok(0);
        };
        // Publish the descriptors and PCM bytes before the controller's DMA
        // engine is allowed to read them.
        core::sync::atomic::fence(Ordering::SeqCst);
        self.regs
            .program_batch(self.bdl_physical(), last_index as u8, total_frames as u32)?;
        Ok(total_frames)
    }

    /// Stops the bus master and abandons the current batch.
    pub fn stop(&mut self) {
        self.regs.stop();
    }

    /// Reads live controller state and retires a completed batch.
    pub fn poll(&mut self) -> Ac97Status {
        self.regs.poll()
    }

    /// Reads controller state without retiring anything.
    pub fn status(&self) -> Ac97Status {
        self.regs.status()
    }

    pub fn is_ready(&self) -> bool {
        self.regs.is_ready()
    }
}

/// Legacy PCI audio enumeration retained for the audio subsystem's hardware
/// inventory and for the existing HDA/AC'97 class scan.
///
/// This type does not program a codec or claim playback; [`Ac97Controller`] is
/// the PCM path.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PciAudioDevice {
    pub addr: PciAddress,
    pub vendor_id: u16,
    pub device_id: u16,
    pub is_hda: bool,
    pub bar0: u32,
    pub interrupt_line: u8,
    pub bus_master_enabled: bool,
}

/// Hardware PCI Audio Controller Manager
pub struct PciAudioController;

impl PciAudioController {
    /// Scans the PCI bus for audio and multimedia controllers.
    pub fn probe_devices() -> Vec<PciAudioDevice> {
        let all_devices = pci::scan_pci_bus();
        let mut audio_devices = Vec::new();

        for dev in &all_devices {
            if dev.is_audio_controller() {
                let bar0 = pci::pci_read_u32(dev.addr, 0x10);
                let is_hda = dev.is_intel_hda();

                // Enable Bus Mastering (Bit 2) and Memory/IO Space (Bits 0 & 1)
                let mut cmd = pci::pci_read_u16(dev.addr, 0x04);
                cmd |= 0x07;
                pci::pci_write_u16(dev.addr, 0x04, cmd);
                let cmd_verified = pci::pci_read_u16(dev.addr, 0x04);
                let bus_master_enabled = (cmd_verified & 0x04) != 0;

                audio_devices.push(PciAudioDevice {
                    addr: dev.addr,
                    vendor_id: dev.vendor_id,
                    device_id: dev.device_id,
                    is_hda,
                    bar0,
                    interrupt_line: dev.interrupt_line,
                    bus_master_enabled,
                });
            }
        }

        audio_devices
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::collections::BTreeMap;
    use alloc::vec::Vec;

    const NAM: u16 = 0x1000;
    const NABM: u16 = 0x2000;

    /// In-memory AC'97 register file. It models only the bits the driver reads
    /// back: the codec IDs/rate, the read-only DCH bit, and SR write-one-to-
    /// clear semantics.
    #[derive(Default)]
    struct MockAc97Io {
        bytes: BTreeMap<u16, u8>,
        writes: Vec<(u16, u32)>,
        cr_port: Option<u16>,
        sr_port: Option<u16>,
    }

    impl MockAc97Io {
        fn codec() -> Self {
            let mut io = Self {
                cr_port: Some(NABM + NABM_PO_CR),
                sr_port: Some(NABM + NABM_PO_SR),
                ..Self::default()
            };
            io.set16_raw(NAM + NAM_VENDOR_ID1, 0x8384);
            io.set16_raw(NAM + NAM_VENDOR_ID2, 0x7600);
            io
        }

        fn get16(&self, port: u16) -> u16 {
            let lo = *self.bytes.get(&port).unwrap_or(&0) as u16;
            let hi = *self.bytes.get(&(port + 1)).unwrap_or(&0) as u16;
            lo | (hi << 8)
        }

        fn set16_raw(&mut self, port: u16, value: u16) {
            self.bytes.insert(port, value as u8);
            self.bytes.insert(port + 1, (value >> 8) as u8);
        }

        /// Simulates the controller finishing a batch (DCH set by hardware).
        fn set_dch(&mut self) {
            let sr_port = self.sr_port.expect("SR port");
            let value = self.get16(sr_port);
            self.set16_raw(sr_port, value | PO_SR_DCH);
        }

        fn last_write(&self, port: u16) -> Option<u32> {
            self.writes.iter().rev().find(|(p, _)| *p == port).map(|(_, v)| *v)
        }
    }

    impl Ac97Io for MockAc97Io {
        fn read8(&mut self, port: u16) -> u8 {
            *self.bytes.get(&port).unwrap_or(&0)
        }

        fn read16(&mut self, port: u16) -> u16 {
            self.get16(port)
        }

        fn read32(&mut self, port: u16) -> u32 {
            self.get16(port) as u32 | ((self.get16(port + 2) as u32) << 16)
        }

        fn write8(&mut self, port: u16, value: u8) {
            self.writes.push((port, value as u32));
            self.bytes.insert(port, value);
            if Some(port) == self.cr_port {
                let sr_port = self.sr_port.expect("SR port");
                let status = self.get16(sr_port);
                if value & PO_CR_RR != 0 || value & PO_CR_RPBM == 0 {
                    self.set16_raw(sr_port, status | PO_SR_DCH);
                } else {
                    self.set16_raw(sr_port, status & !PO_SR_DCH);
                }
            }
        }

        fn write16(&mut self, port: u16, value: u16) {
            self.writes.push((port, value as u32));
            if Some(port) == self.sr_port {
                // DCH is read-only; the remaining status bits are write-one-to-clear.
                let status = self.get16(port);
                let next = (status & !(value & PO_SR_WCLEAR)) | (value & !PO_SR_DCH);
                self.set16_raw(port, next);
            } else {
                self.set16_raw(port, value);
            }
        }

        fn write32(&mut self, port: u16, value: u32) {
            self.writes.push((port, value));
            self.set16_raw(port, value as u16);
            self.set16_raw(port + 2, (value >> 16) as u16);
        }
    }

    fn initialized_registers() -> Ac97Registers<MockAc97Io> {
        let mut regs = Ac97Registers::new(MockAc97Io::codec(), NAM, NABM);
        regs.initialize().expect("mock codec must initialize");
        regs
    }

    #[test]
    fn initialize_programs_rate_and_unmutes_outputs() {
        let regs = initialized_registers();
        assert!(regs.is_ready());
        assert_eq!(regs.io.get16(NAM + NAM_PCM_FRONT_DAC_RATE), 48_000);
        // QEMU's AC'97 reset leaves master muted; both stages must be unmuted.
        assert_eq!(regs.io.get16(NAM + NAM_MASTER_VOLUME), 0x0000);
        assert_eq!(regs.io.get16(NAM + NAM_PCM_OUT_VOLUME), 0x0000);
        assert_eq!(regs.io.last_write(NABM + NABM_GLOB_CNT), Some(0));
        assert!(!regs.is_running());
    }

    #[test]
    fn absent_codec_is_unavailable() {
        let mut io = MockAc97Io::codec();
        io.set16_raw(NAM + NAM_VENDOR_ID1, 0xFFFF);
        let mut regs = Ac97Registers::new(io, NAM, NABM);
        assert_eq!(regs.initialize(), Err(Ac97Error::Unavailable));
        assert!(!regs.is_ready());
    }

    #[test]
    fn codec_that_ignores_the_rate_register_is_a_mismatch() {
        struct FixedRateIo(MockAc97Io);
        impl Ac97Io for FixedRateIo {
            fn read8(&mut self, port: u16) -> u8 { self.0.read8(port) }
            fn read16(&mut self, port: u16) -> u16 {
                if port == NAM + NAM_PCM_FRONT_DAC_RATE { 44_100 } else { self.0.read16(port) }
            }
            fn read32(&mut self, port: u16) -> u32 { self.0.read32(port) }
            fn write8(&mut self, port: u16, value: u8) { self.0.write8(port, value) }
            fn write16(&mut self, port: u16, value: u16) { self.0.write16(port, value) }
            fn write32(&mut self, port: u16, value: u32) { self.0.write32(port, value) }
        }
        let mut regs = Ac97Registers::new(FixedRateIo(MockAc97Io::codec()), NAM, NABM);
        assert_eq!(regs.initialize(), Err(Ac97Error::RegisterMismatch));
    }

    #[test]
    fn program_batch_sets_run_bit_and_publishes_bdbar() {
        let mut regs = initialized_registers();
        regs.program_batch(0x30_000, 3, 2048).expect("program batch");
        assert!(regs.is_running());
        assert_eq!(regs.io.last_write(NABM + NABM_PO_BDBAR), Some(0x30_000));
        assert_eq!(regs.io.last_write(NABM + NABM_PO_LVI), Some(3));
        assert_eq!(regs.io.last_write(NABM + NABM_PO_CR), Some(PO_CR_RPBM as u32));
        let status = regs.status();
        assert_eq!(status.frames_submitted, 2048);
        assert_eq!(status.frames_completed, 0);
    }

    #[test]
    fn completed_batch_is_accounted_exactly_once() {
        let mut regs = initialized_registers();
        regs.program_batch(0x30_000, 3, 2048).expect("program batch");
        regs.io.set_dch();

        let first = regs.poll();
        assert!(first.just_idle);
        assert!(!first.running);
        assert_eq!(first.frames_completed, 2048);

        let second = regs.poll();
        assert!(!second.just_idle, "an idle device must not be counted twice");
        assert_eq!(second.frames_completed, 2048);
    }

    #[test]
    fn stop_abandons_the_batch_without_claiming_completion() {
        let mut regs = initialized_registers();
        regs.program_batch(0x30_000, 1, 1024).expect("program batch");
        regs.stop();
        let status = regs.status();
        assert!(!status.running);
        assert_eq!(status.frames_completed, 0);
        assert_eq!(status.frames_submitted, 1024);
    }

    #[test]
    fn programming_before_initialize_or_with_bad_index_is_refused() {
        let mut cold = Ac97Registers::new(MockAc97Io::codec(), NAM, NABM);
        assert_eq!(cold.program_batch(0x30_000, 0, 512), Err(Ac97Error::Unavailable));

        let mut regs = initialized_registers();
        assert_eq!(
            regs.program_batch(0x30_000, AC97_BDL_ENTRIES as u8, 512),
            Err(Ac97Error::InvalidDescriptor)
        );
        assert_eq!(regs.program_batch(0x30_000, 0, 0), Err(Ac97Error::InvalidDescriptor));
        assert!(!regs.is_running());
    }
}
