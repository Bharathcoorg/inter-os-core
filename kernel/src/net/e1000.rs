// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

//! Intel E1000-compatible MMIO Ethernet controller.
//!
//! The driver owns descriptor-ring programming and packet movement behind the
//! NetworkDevice boundary. DMA addresses are supplied explicitly by the
//! platform mapping layer; the driver never assumes a virtual address is a
//! physical DMA address.

use core::sync::atomic::{compiler_fence, AtomicU64, Ordering};

pub static E1000_RX_COUNT: AtomicU64 = AtomicU64::new(0);

pub fn rx_packet_count() -> u64 {
    E1000_RX_COUNT.load(Ordering::Relaxed)
}
use super::device::{NetError, NetworkDevice, MAX_PACKET_SIZE};
use super::ethernet::MacAddress;
use crate::memory::{allocate_dma_frames, physical_to_virtual};
use x86_64::PhysAddr;
use x86_64::structures::paging::{PhysFrame, Size4KiB};

const CTRL: usize = 0x0000;
const STATUS: usize = 0x0008;
const EERD: usize = 0x0014;
/// EERD bit 0 starts a read and bit 4 reports that the word is ready;
/// bits 15:8 carry the word address and bits 31:16 the word itself.
///
/// WHY the start bit is not bit 31: 31:16 is the data field. A write that
/// sets bit 31 instead of bit 0 never starts a transfer, the done poll never
/// observes a completed read, and every word comes back zero.
const EERD_START: u32 = 1 << 0;
const EERD_DONE: u32 = 1 << 4;
const EERD_ADDR_SHIFT: u32 = 8;
const EERD_DATA_SHIFT: u32 = 16;
const RCTL: usize = 0x0100;
const TCTL: usize = 0x0400;
const RDBAL: usize = 0x2800;
const RDBAH: usize = 0x2804;
const RDLEN: usize = 0x2808;
const RDH: usize = 0x2810;
const RDT: usize = 0x2818;
const TDBAL: usize = 0x3800;
const TDBAH: usize = 0x3804;
const TDLEN: usize = 0x3808;
const TDH: usize = 0x3810;
const TDT: usize = 0x3818;
const RAL0: usize = 0x5400;
const RAH0: usize = 0x5404;
const RAH_AV: u32 = 1 << 31;
const IMC: usize = 0x00D8;
const RX_DESC_COUNT: usize = 16;
const TX_DESC_COUNT: usize = 16;
pub const RX_BUFFER_SIZE: usize = 2048;
pub const TX_BUFFER_SIZE: usize = 2048;

#[repr(C)]
#[derive(Copy, Clone)]
pub struct RxDescriptor {
    addr: u64,
    length: u16,
    checksum: u16,
    status: u8,
    errors: u8,
    special: u16,
}

#[repr(C)]
#[derive(Copy, Clone)]
pub struct TxDescriptor {
    addr: u64,
    length: u16,
    cso: u8,
    cmd: u8,
    status: u8,
    css: u8,
    special: u16,
}

const RX_EOP: u8 = 2;
const TX_CMD_EOP: u8 = 1;
const TX_CMD_IFCS: u8 = 2;
const TX_CMD_RS: u8 = 8;

#[inline(always)]
unsafe fn read32(base: usize, offset: usize) -> u32 {
    compiler_fence(Ordering::SeqCst);
    let value = core::ptr::read_volatile((base + offset) as *const u32);
    compiler_fence(Ordering::SeqCst);
    value
}

#[inline(always)]
unsafe fn write32(base: usize, offset: usize, value: u32) {
    compiler_fence(Ordering::SeqCst);
    core::ptr::write_volatile((base + offset) as *mut u32, value);
    compiler_fence(Ordering::SeqCst);
}

/// Reads one EEPROM word, or returns zero if the controller never completes
/// the transfer. The caller distinguishes "zero" from a valid word by the
/// address it asked for: words 3..5 hold the station address.
unsafe fn read_eeprom_word(base: usize, address: u8) -> u16 {
    write32(
        base,
        EERD,
        EERD_START | ((address as u32) << EERD_ADDR_SHIFT),
    );
    for _ in 0..100_000 {
        let value = read32(base, EERD);
        if value & EERD_DONE != 0 {
            return (value >> EERD_DATA_SHIFT) as u16;
        }
    }
    0
}

/// Minimal descriptor-ring E1000 driver.
/// Descriptor storage must be backed by physically contiguous DMA memory.
/// `rx_buffer_phys` and `tx_buffer_phys` point to arrays of RX/TX slots.
struct E1000DmaResources {
    rx_desc_frames: alloc::vec::Vec<PhysFrame<Size4KiB>>,
    tx_desc_frames: alloc::vec::Vec<PhysFrame<Size4KiB>>,
    rx_buffer_frames: alloc::vec::Vec<PhysFrame<Size4KiB>>,
    tx_buffer_frames: alloc::vec::Vec<PhysFrame<Size4KiB>>,
}

impl E1000DmaResources {
    fn allocate() -> Option<Self> {
        Some(Self {
            rx_desc_frames: allocate_dma_frames(1)?,
            tx_desc_frames: allocate_dma_frames(1)?,
            rx_buffer_frames: allocate_dma_frames(8)?,
            tx_buffer_frames: allocate_dma_frames(8)?,
        })
    }
    fn rx_desc_phys(&self) -> u64 { self.rx_desc_frames[0].start_address().as_u64() }
    fn tx_desc_phys(&self) -> u64 { self.tx_desc_frames[0].start_address().as_u64() }
    fn rx_buffer_phys(&self) -> u64 { self.rx_buffer_frames[0].start_address().as_u64() }
    fn tx_buffer_phys(&self) -> u64 { self.tx_buffer_frames[0].start_address().as_u64() }
    fn rx_desc_ptr(&self) -> *mut RxDescriptor { physical_to_virtual(self.rx_desc_frames[0].start_address()).as_mut_ptr() }
    fn tx_desc_ptr(&self) -> *mut TxDescriptor { physical_to_virtual(self.tx_desc_frames[0].start_address()).as_mut_ptr() }
}

/// Descriptor-ring E1000 controller with owned physically contiguous DMA storage.
pub struct E1000Device {
    base_vaddr: usize,
    mac: MacAddress,
    rx_desc: *mut RxDescriptor,
    tx_desc: *mut TxDescriptor,
    rx_desc_phys: u64,
    tx_desc_phys: u64,
    rx_buffer_phys: u64,
    tx_buffer_phys: u64,
    rx_index: u16,
    tx_index: u16,
    link_up: bool,
    dma: Option<E1000DmaResources>,
    rx_assembling: bool,
    rx_discard: bool,
    rx_accum_len: usize,
    rx_desc_chain_len: usize,
    rx_assembly_buf: [u8; MAX_PACKET_SIZE],
}

unsafe impl Send for E1000Device {}
unsafe impl Sync for E1000Device {}

impl E1000Device {
    pub unsafe fn new(
        base_vaddr: usize,
        rx_desc: *mut RxDescriptor,
        tx_desc: *mut TxDescriptor,
        rx_desc_phys: u64,
        tx_desc_phys: u64,
        rx_buffer_phys: u64,
        tx_buffer_phys: u64,
        mac: MacAddress,
    ) -> Self {
        Self {
            base_vaddr,
            mac,
            rx_desc,
            tx_desc,
            rx_desc_phys,
            tx_desc_phys,
            rx_buffer_phys,
            tx_buffer_phys,
            rx_index: 0,
            tx_index: 0,
            link_up: false,
            dma: None,
            rx_assembling: false,
            rx_discard: false,
            rx_accum_len: 0,
            rx_desc_chain_len: 0,
            rx_assembly_buf: [0u8; MAX_PACKET_SIZE],
        }
    }

    /// Allocates and owns all DMA memory required by the controller.
    pub unsafe fn allocate_with_dma(base_vaddr: usize, mac: MacAddress) -> Option<Self> {
        let dma = E1000DmaResources::allocate()?;
        let rx_desc = dma.rx_desc_ptr();
        let tx_desc = dma.tx_desc_ptr();
        let mut device = Self::new(
            base_vaddr, rx_desc, tx_desc,
            dma.rx_desc_phys(), dma.tx_desc_phys(),
            dma.rx_buffer_phys(), dma.tx_buffer_phys(), mac,
        );
        device.dma = Some(dma);
        Some(device)
    }

    /// Returns the physical address currently programmed for the RX ring.
    pub fn rx_ring_physical_address(&self) -> u64 { self.rx_desc_phys }

    /// Returns the physical address currently programmed for the TX ring.
    pub fn tx_ring_physical_address(&self) -> u64 { self.tx_desc_phys }

    /// Returns the physical address of the first RX buffer slot.
    pub fn rx_buffer_physical_address(&self) -> u64 { self.rx_buffer_phys }

    /// Returns the physical address of the first TX buffer slot.
    pub fn tx_buffer_physical_address(&self) -> u64 { self.tx_buffer_phys }

    pub fn reset(&mut self) {
        unsafe {
            let ctrl = read32(self.base_vaddr, CTRL);
            write32(self.base_vaddr, CTRL, ctrl | (1 << 26));
            for _ in 0..100_000 {
                if read32(self.base_vaddr, CTRL) & (1 << 26) == 0 { break; }
            }
        }
        self.rx_index = 0;
        self.tx_index = 0;
        self.link_up = false;
        self.rx_assembling = false;
        self.rx_discard = false;
        self.rx_accum_len = 0;
        self.rx_desc_chain_len = 0;
    }

    pub fn initialize(&mut self) -> Result<(), NetError> {
        self.rx_index = 0;
        self.tx_index = 0;
        self.rx_assembling = false;
        self.rx_discard = false;
        self.rx_accum_len = 0;
        self.rx_desc_chain_len = 0;
        unsafe {
            for i in 0..RX_DESC_COUNT {
                let desc = &mut *self.rx_desc.add(i);
                desc.addr = self.rx_buffer_phys + (i * RX_BUFFER_SIZE) as u64;
                desc.length = 0;
                desc.status = 0;
                desc.errors = 0;
            }
            for i in 0..TX_DESC_COUNT {
                let desc = &mut *self.tx_desc.add(i);
                desc.addr = self.tx_buffer_phys + (i * TX_BUFFER_SIZE) as u64;
                desc.length = 0;
                desc.cmd = 0;
                desc.status = 1;
            }
            // Mask all hardware interrupts since driver operates in polling mode
            write32(self.base_vaddr, IMC, 0xFFFF_FFFF);

            write32(self.base_vaddr, RDBAL, self.rx_desc_phys as u32);
            write32(self.base_vaddr, RDBAH, (self.rx_desc_phys >> 32) as u32);
            write32(self.base_vaddr, RDLEN, (RX_DESC_COUNT * 16) as u32);
            write32(self.base_vaddr, RDH, 0);
            write32(self.base_vaddr, RDT, (RX_DESC_COUNT - 1) as u32);
            write32(self.base_vaddr, TDBAL, self.tx_desc_phys as u32);
            write32(self.base_vaddr, TDBAH, (self.tx_desc_phys >> 32) as u32);
            write32(self.base_vaddr, TDLEN, (TX_DESC_COUNT * 16) as u32);
            write32(self.base_vaddr, TDH, 0);
            write32(self.base_vaddr, TDT, 0);
            // Program station address into Receive Address 0 (RAL0/RAH0) with Address Valid (AV) bit
            let ral = (self.mac.0[0] as u32)
                | ((self.mac.0[1] as u32) << 8)
                | ((self.mac.0[2] as u32) << 16)
                | ((self.mac.0[3] as u32) << 24);
            let rah = (self.mac.0[4] as u32)
                | ((self.mac.0[5] as u32) << 8)
                | RAH_AV;
            write32(self.base_vaddr, RAL0, ral);
            write32(self.base_vaddr, RAH0, rah);
            // RCTL: EN(1<<1) | MPE(1<<4) | BAM(1<<15) | SECRC(1<<26)
            write32(self.base_vaddr, RCTL, (1 << 1) | (1 << 4) | (1 << 15) | (1 << 26));
            write32(self.base_vaddr, TCTL, (1 << 1) | (1 << 3) | (0x10 << 4) | (0x40 << 12));
            write32(self.base_vaddr, CTRL, read32(self.base_vaddr, CTRL) | (1 << 1) | (1 << 6));
            self.link_up = true;
        }
        Ok(())
    }

    pub fn read_link_status(&self) -> bool {
        unsafe { read32(self.base_vaddr, STATUS) & (1 << 1) != 0 }
    }

    pub fn eeprom_word(&self, address: u8) -> u16 {
        unsafe { read_eeprom_word(self.base_vaddr, address) }
    }

    pub fn mac_address(&self) -> MacAddress { self.mac }

    /// Reads the permanent station address from the E1000 EEPROM.
    pub unsafe fn read_mac_from_eeprom(base_vaddr: usize) -> MacAddress {
        // Intel 8254x parts store the station address in EEPROM words 0..2,
        // low byte first. Words 3..5 are not part of the address: on the QEMU
        // 82540 profile they read 0x0000, 0xFFFF, 0x0000, which would attach
        // the controller under a bogus 00:00:ff:ff:00:00 address.
        let w0 = read_eeprom_word(base_vaddr, 0);
        let w1 = read_eeprom_word(base_vaddr, 1);
        let w2 = read_eeprom_word(base_vaddr, 2);
        MacAddress([(w0 & 0xff) as u8, (w0 >> 8) as u8,
                    (w1 & 0xff) as u8, (w1 >> 8) as u8,
                    (w2 & 0xff) as u8, (w2 >> 8) as u8])
    }
}

impl NetworkDevice for E1000Device {
    fn mac(&self) -> MacAddress { self.mac }

    fn transmit(&mut self, frame: &[u8]) -> Result<(), NetError> {
        if !self.link_up { return Err(NetError::DeviceDown); }
        if frame.is_empty() || frame.len() > MAX_PACKET_SIZE { return Err(NetError::BufferTooSmall); }
        let idx = self.tx_index as usize;
        unsafe {
            let desc = &mut *self.tx_desc.add(idx);
            let status = core::ptr::read_volatile(&desc.status);
            if (status & 1) == 0 { return Err(NetError::QueueFull); }
            core::sync::atomic::fence(Ordering::Acquire);

            let buffer = self.tx_buffer_phys + (idx * TX_BUFFER_SIZE) as u64;
            let buffer_virt = physical_to_virtual(PhysAddr::new(buffer)).as_mut_ptr();
            core::ptr::copy_nonoverlapping(frame.as_ptr(), buffer_virt, frame.len());

            // Ensure buffer payload is visible to DMA before descriptor is posted
            core::sync::atomic::fence(Ordering::Release);

            desc.addr = buffer;
            desc.length = frame.len() as u16;
            desc.cso = 0;
            desc.css = 0;
            desc.special = 0;
            core::ptr::write_volatile(&mut desc.status, 0);
            desc.cmd = TX_CMD_EOP | TX_CMD_IFCS | TX_CMD_RS;
            core::sync::atomic::fence(Ordering::SeqCst);
            write32(self.base_vaddr, TDT, ((idx + 1) % TX_DESC_COUNT) as u32);
        }
        self.tx_index = ((idx + 1) % TX_DESC_COUNT) as u16;
        Ok(())
    }

    fn receive(&mut self, out: &mut [u8]) -> Result<usize, NetError> {
        if !self.link_up { return Err(NetError::DeviceDown); }

        for _ in 0..RX_DESC_COUNT {
            let idx = self.rx_index as usize;
            unsafe {
                let desc = &mut *self.rx_desc.add(idx);
                let status = core::ptr::read_volatile(&desc.status);
                if (status & 1) == 0 {
                    return Err(NetError::NoPacketAvailable);
                }
                core::sync::atomic::fence(Ordering::Acquire);

                let expected = self.rx_buffer_phys + (idx * RX_BUFFER_SIZE) as u64;

                // Defend against descriptor pointer desynchronization or device memory corruption
                if desc.addr != expected {
                    self.rx_assembling = false;
                    self.rx_discard = false;
                    self.rx_accum_len = 0;
                    self.rx_desc_chain_len = 0;

                    desc.addr = expected;
                    core::ptr::write_volatile(&mut desc.status, 0);
                    core::sync::atomic::fence(Ordering::Release);
                    write32(self.base_vaddr, RDT, idx as u32);
                    self.rx_index = ((idx + 1) % RX_DESC_COUNT) as u16;
                    return Err(NetError::InvalidPacket);
                }

                let is_eop = (status & RX_EOP) != 0;
                let seg_len = desc.length as usize;
                let has_error = desc.errors != 0;

                self.rx_desc_chain_len += 1;
                if self.rx_desc_chain_len > RX_DESC_COUNT {
                    self.rx_discard = true;
                    self.rx_assembling = false;
                    self.rx_accum_len = 0;
                }

                if self.rx_discard {
                    desc.addr = expected;
                    core::ptr::write_volatile(&mut desc.status, 0);
                    core::sync::atomic::fence(Ordering::Release);
                    write32(self.base_vaddr, RDT, idx as u32);
                    self.rx_index = ((idx + 1) % RX_DESC_COUNT) as u16;

                    if is_eop {
                        self.rx_discard = false;
                        self.rx_desc_chain_len = 0;
                        return Err(NetError::InvalidPacket);
                    }
                    continue;
                }

                if has_error {
                    desc.addr = expected;
                    core::ptr::write_volatile(&mut desc.status, 0);
                    core::sync::atomic::fence(Ordering::Release);
                    write32(self.base_vaddr, RDT, idx as u32);
                    self.rx_index = ((idx + 1) % RX_DESC_COUNT) as u16;

                    if is_eop {
                        self.rx_assembling = false;
                        self.rx_accum_len = 0;
                        self.rx_desc_chain_len = 0;
                        return Err(NetError::InvalidPacket);
                    } else {
                        self.rx_discard = true;
                        self.rx_assembling = false;
                        self.rx_accum_len = 0;
                        continue;
                    }
                }

                if !is_eop {
                    if self.rx_accum_len.saturating_add(seg_len) > MAX_PACKET_SIZE {
                        self.rx_discard = true;
                        self.rx_assembling = false;
                        self.rx_accum_len = 0;
                    } else {
                        let buffer_virt = physical_to_virtual(PhysAddr::new(expected)).as_ptr();
                        core::ptr::copy_nonoverlapping(
                            buffer_virt,
                            self.rx_assembly_buf.as_mut_ptr().add(self.rx_accum_len),
                            seg_len,
                        );
                        self.rx_accum_len += seg_len;
                        self.rx_assembling = true;
                    }

                    desc.addr = expected;
                    core::ptr::write_volatile(&mut desc.status, 0);
                    core::sync::atomic::fence(Ordering::Release);
                    write32(self.base_vaddr, RDT, idx as u32);
                    self.rx_index = ((idx + 1) % RX_DESC_COUNT) as u16;
                    continue;
                } else {
                    if self.rx_assembling {
                        let total_len = self.rx_accum_len.saturating_add(seg_len);
                        let buffer_virt = physical_to_virtual(PhysAddr::new(expected)).as_ptr();

                        let result = if total_len > MAX_PACKET_SIZE {
                            Err(NetError::InvalidPacket)
                        } else if total_len > out.len() {
                            Err(NetError::BufferTooSmall)
                        } else {
                            core::ptr::copy_nonoverlapping(
                                buffer_virt,
                                self.rx_assembly_buf.as_mut_ptr().add(self.rx_accum_len),
                                seg_len,
                            );
                            core::ptr::copy_nonoverlapping(
                                self.rx_assembly_buf.as_ptr(),
                                out.as_mut_ptr(),
                                total_len,
                            );
                            Ok(total_len)
                        };

                        desc.addr = expected;
                        core::ptr::write_volatile(&mut desc.status, 0);
                        core::sync::atomic::fence(Ordering::Release);
                        write32(self.base_vaddr, RDT, idx as u32);
                        self.rx_index = ((idx + 1) % RX_DESC_COUNT) as u16;

                        self.rx_assembling = false;
                        self.rx_accum_len = 0;
                        self.rx_desc_chain_len = 0;

                        if result.is_ok() {
                            E1000_RX_COUNT.fetch_add(1, Ordering::Relaxed);
                        }
                        return result;
                    } else {
                        let result = if seg_len > out.len() {
                            Err(NetError::BufferTooSmall)
                        } else if seg_len > MAX_PACKET_SIZE {
                            Err(NetError::InvalidPacket)
                        } else {
                            let buffer_virt = physical_to_virtual(PhysAddr::new(expected)).as_ptr();
                            core::ptr::copy_nonoverlapping(buffer_virt, out.as_mut_ptr(), seg_len);
                            Ok(seg_len)
                        };

                        desc.addr = expected;
                        core::ptr::write_volatile(&mut desc.status, 0);
                        core::sync::atomic::fence(Ordering::Release);
                        write32(self.base_vaddr, RDT, idx as u32);
                        self.rx_index = ((idx + 1) % RX_DESC_COUNT) as u16;
                        self.rx_desc_chain_len = 0;

                        if result.is_ok() {
                            E1000_RX_COUNT.fetch_add(1, Ordering::Relaxed);
                        }
                        return result;
                    }
                }
            }
        }

        Err(NetError::NoPacketAvailable)
    }

    fn has_packets(&self) -> bool {
        unsafe { (core::ptr::read_volatile(&(*self.rx_desc.add(self.rx_index as usize)).status) & 1) != 0 }
    }
}

impl E1000Device {
    pub fn transmit_probe(&mut self, frame: &[u8]) -> Result<(), NetError> {
        if !self.link_up { return Err(NetError::DeviceDown); }
        let idx = self.tx_index as usize;
        self.transmit(frame)?;
        unsafe {
            let desc = &*self.tx_desc.add(idx);
            for _ in 0..100_000 {
                if core::ptr::read_volatile(&desc.status) & 1 != 0 { return Ok(()); }
                core::hint::spin_loop();
            }
        }
        Err(NetError::TransmitFailed)
    }

    /// Bounded hardware RX-ring readiness probe used during NIC bring-up.
    pub fn receive_probe(&mut self, out: &mut [u8]) -> Result<usize, NetError> {
        if !self.link_up { return Err(NetError::DeviceDown); }
        for _ in 0..100_000 {
            if self.has_packets() {
                return self.receive(out);
            }
            core::hint::spin_loop();
        }
        Err(NetError::NoPacketAvailable)
    }
}

pub const fn rx_descriptor_count() -> usize { RX_DESC_COUNT }
pub const fn tx_descriptor_count() -> usize { TX_DESC_COUNT }

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::boxed::Box;

    struct MockE1000Hardware {
        mmio: [u32; 0x4000],
        rx_descriptors: [RxDescriptor; RX_DESC_COUNT],
        tx_descriptors: [TxDescriptor; TX_DESC_COUNT],
        rx_buffers: [u8; RX_DESC_COUNT * RX_BUFFER_SIZE],
        tx_buffers: [u8; TX_DESC_COUNT * TX_BUFFER_SIZE],
    }

    impl MockE1000Hardware {
        fn new() -> Box<Self> {
            let mut hw = Box::new(Self {
                mmio: [0u32; 0x4000],
                rx_descriptors: [RxDescriptor { addr: 0, length: 0, checksum: 0, status: 0, errors: 0, special: 0 }; RX_DESC_COUNT],
                tx_descriptors: [TxDescriptor { addr: 0, length: 0, cso: 0, cmd: 0, status: 1, css: 0, special: 0 }; TX_DESC_COUNT],
                rx_buffers: [0u8; RX_DESC_COUNT * RX_BUFFER_SIZE],
                tx_buffers: [0u8; TX_DESC_COUNT * TX_BUFFER_SIZE],
            });

            for i in 0..RX_DESC_COUNT {
                hw.rx_descriptors[i].addr = hw.rx_buffers.as_mut_ptr() as u64 + (i * RX_BUFFER_SIZE) as u64;
            }
            for i in 0..TX_DESC_COUNT {
                hw.tx_descriptors[i].addr = hw.tx_buffers.as_mut_ptr() as u64 + (i * TX_BUFFER_SIZE) as u64;
                hw.tx_descriptors[i].status = 1;
            }
            hw
        }

        fn create_device(&mut self) -> E1000Device {
            let base_vaddr = self.mmio.as_mut_ptr() as usize;
            let rx_desc = self.rx_descriptors.as_mut_ptr();
            let tx_desc = self.tx_descriptors.as_mut_ptr();
            let rx_desc_phys = rx_desc as u64;
            let tx_desc_phys = tx_desc as u64;
            let rx_buffer_phys = self.rx_buffers.as_mut_ptr() as u64;
            let tx_buffer_phys = self.tx_buffers.as_mut_ptr() as u64;
            let mac = MacAddress([0x52, 0x54, 0x00, 0x12, 0x34, 0x56]);

            let mut dev = unsafe {
                E1000Device::new(
                    base_vaddr,
                    rx_desc,
                    tx_desc,
                    rx_desc_phys,
                    tx_desc_phys,
                    rx_buffer_phys,
                    tx_buffer_phys,
                    mac,
                )
            };
            dev.link_up = true;
            dev
        }
    }

    #[test]
    fn test_single_descriptor_rx_success() {
        let mut hw = MockE1000Hardware::new();
        let mut dev = hw.create_device();

        // Hardware deposits 64-byte frame at slot 0
        let payload = [0xABu8; 64];
        hw.rx_buffers[0..64].copy_from_slice(&payload);
        hw.rx_descriptors[0].length = 64;
        hw.rx_descriptors[0].status = 1 | RX_EOP; // DD=1, EOP=1

        let mut out = [0u8; 1514];
        let res = dev.receive(&mut out);
        assert_eq!(res, Ok(64));
        assert_eq!(&out[0..64], &payload);
        assert_eq!(dev.rx_index, 1);
        assert_eq!(hw.rx_descriptors[0].status, 0); // Recycled
    }

    #[test]
    fn test_multi_descriptor_rx_assembly_success() {
        let mut hw = MockE1000Hardware::new();
        let mut dev = hw.create_device();

        // Descriptor 0: 300 bytes, EOP=0
        let part1 = [0x11u8; 300];
        hw.rx_buffers[0..300].copy_from_slice(&part1);
        hw.rx_descriptors[0].length = 300;
        hw.rx_descriptors[0].status = 1; // DD=1, EOP=0

        // Descriptor 1: 200 bytes, EOP=1
        let part2 = [0x22u8; 200];
        let offset1 = RX_BUFFER_SIZE;
        hw.rx_buffers[offset1..offset1 + 200].copy_from_slice(&part2);
        hw.rx_descriptors[1].length = 200;
        hw.rx_descriptors[1].status = 1 | RX_EOP; // DD=1, EOP=1

        let mut out = [0u8; 1514];
        let res = dev.receive(&mut out);
        assert_eq!(res, Ok(500));
        assert_eq!(&out[0..300], &part1);
        assert_eq!(&out[300..500], &part2);
        assert_eq!(dev.rx_index, 2);
        assert_eq!(hw.rx_descriptors[0].status, 0);
        assert_eq!(hw.rx_descriptors[1].status, 0);
    }

    #[test]
    fn test_multi_descriptor_partial_never_delivered() {
        let mut hw = MockE1000Hardware::new();
        let mut dev = hw.create_device();

        // Descriptor 0 arrives with DD=1, EOP=0
        let part1 = [0x33u8; 256];
        hw.rx_buffers[0..256].copy_from_slice(&part1);
        hw.rx_descriptors[0].length = 256;
        hw.rx_descriptors[0].status = 1; // DD=1, EOP=0

        // Descriptor 1 has DD=0 (not arrived yet)
        hw.rx_descriptors[1].status = 0;

        let mut out = [0u8; 1514];
        let res = dev.receive(&mut out);
        // Partial frame must NEVER be delivered!
        assert_eq!(res, Err(NetError::NoPacketAvailable));
        assert!(dev.rx_assembling);
        assert_eq!(dev.rx_accum_len, 256);
        assert_eq!(dev.rx_index, 1);
        assert_eq!(hw.rx_descriptors[0].status, 0); // Recycled

        // Now descriptor 1 arrives
        let part2 = [0x44u8; 128];
        let offset1 = RX_BUFFER_SIZE;
        hw.rx_buffers[offset1..offset1 + 128].copy_from_slice(&part2);
        hw.rx_descriptors[1].length = 128;
        hw.rx_descriptors[1].status = 1 | RX_EOP; // DD=1, EOP=1

        let res2 = dev.receive(&mut out);
        assert_eq!(res2, Ok(384));
        assert_eq!(&out[0..256], &part1);
        assert_eq!(&out[256..384], &part2);
        assert_eq!(dev.rx_index, 2);
        assert!(!dev.rx_assembling);
    }

    #[test]
    fn test_multi_descriptor_oversized_discard_mode() {
        let mut hw = MockE1000Hardware::new();
        let mut dev = hw.create_device();

        // 2 segments exceeding MAX_PACKET_SIZE 1514
        hw.rx_descriptors[0].length = 1000;
        hw.rx_descriptors[0].status = 1; // DD=1, EOP=0

        hw.rx_descriptors[1].length = 800; // 1000 + 800 = 1800 > 1514
        hw.rx_descriptors[1].status = 1; // DD=1, EOP=0

        hw.rx_descriptors[2].length = 100;
        hw.rx_descriptors[2].status = 1 | RX_EOP; // DD=1, EOP=1

        let mut out = [0u8; 1514];
        let res = dev.receive(&mut out);
        assert_eq!(res, Err(NetError::InvalidPacket));
        assert_eq!(dev.rx_index, 3);
        assert!(!dev.rx_assembling);
        assert!(!dev.rx_discard);
        // All 3 descriptors recycled
        assert_eq!(hw.rx_descriptors[0].status, 0);
        assert_eq!(hw.rx_descriptors[1].status, 0);
        assert_eq!(hw.rx_descriptors[2].status, 0);
    }

    #[test]
    fn test_tx_slot_busy_queue_full() {
        let mut hw = MockE1000Hardware::new();
        let mut dev = hw.create_device();

        // Slot 0 has DD=0 (hardware still processing previous transmission)
        hw.tx_descriptors[0].status = 0;

        let frame = [0x55u8; 100];
        let res = dev.transmit(&frame);
        assert_eq!(res, Err(NetError::QueueFull));
        assert_eq!(dev.tx_index, 0); // Must not advance
        assert_eq!(hw.tx_buffers[0], 0); // Buffer must not be touched
    }

    #[test]
    fn test_tx_success_and_buffer_coherency() {
        let mut hw = MockE1000Hardware::new();
        let mut dev = hw.create_device();

        hw.tx_descriptors[0].status = 1; // DD=1 (ready)

        let frame = [0x77u8; 120];
        let res = dev.transmit(&frame);
        assert_eq!(res, Ok(()));
        assert_eq!(dev.tx_index, 1);
        assert_eq!(&hw.tx_buffers[0..120], &frame);
        assert_eq!(hw.tx_descriptors[0].length, 120);
        assert_eq!(hw.tx_descriptors[0].status, 0); // Status cleared for hardware
        assert_eq!(hw.tx_descriptors[0].cmd, TX_CMD_EOP | TX_CMD_IFCS | TX_CMD_RS);
    }

    #[test]
    fn test_rx_address_corruption_recovery() {
        let mut hw = MockE1000Hardware::new();
        let mut dev = hw.create_device();

        // Corrupt descriptor address written by buggy device/DMA
        hw.rx_descriptors[0].addr = 0xDEAD_BEEF_0000;
        hw.rx_descriptors[0].status = 1 | RX_EOP;

        let mut out = [0u8; 1514];
        let res = dev.receive(&mut out);
        assert_eq!(res, Err(NetError::InvalidPacket));
        assert_eq!(dev.rx_index, 1);
        // Desynchronized address must be restored to expected physical buffer address
        assert_eq!(hw.rx_descriptors[0].addr, hw.rx_buffers.as_mut_ptr() as u64);
        assert_eq!(hw.rx_descriptors[0].status, 0);
    }
}
