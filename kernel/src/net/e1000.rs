//! Intel E1000-compatible MMIO Ethernet controller.
//!
//! The driver owns descriptor-ring programming and packet movement behind the
//! NetworkDevice boundary. DMA addresses are supplied explicitly by the
//! platform mapping layer; the driver never assumes a virtual address is a
//! physical DMA address.

use core::sync::atomic::{compiler_fence, Ordering};
use super::device::{NetError, NetworkDevice, MAX_PACKET_SIZE};
use super::ethernet::MacAddress;

const CTRL: usize = 0x0000;
const STATUS: usize = 0x0008;
const EERD: usize = 0x0014;
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
const RX_DESC_COUNT: usize = 16;
const TX_DESC_COUNT: usize = 16;

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

/// Minimal descriptor-ring E1000 driver.
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
        Self { base_vaddr, mac, rx_desc, tx_desc, rx_desc_phys, tx_desc_phys,
            rx_buffer_phys, tx_buffer_phys, rx_index: 0, tx_index: 0, link_up: false }
    }

    pub fn reset(&mut self) {
        unsafe {
            let ctrl = read32(self.base_vaddr, CTRL);
            write32(self.base_vaddr, CTRL, ctrl | (1 << 26));
            for _ in 0..100_000 {
                if read32(self.base_vaddr, CTRL) & (1 << 26) == 0 { break; }
            }
        }
        self.link_up = false;
    }

    pub fn initialize(&mut self) -> Result<(), NetError> {
        unsafe {
            for i in 0..RX_DESC_COUNT {
                let desc = &mut *self.rx_desc.add(i);
                desc.addr = self.rx_buffer_phys + (i * MAX_PACKET_SIZE) as u64;
                desc.length = 0;
                desc.status = 0;
                desc.errors = 0;
            }
            for i in 0..TX_DESC_COUNT {
                let desc = &mut *self.tx_desc.add(i);
                desc.addr = self.tx_buffer_phys + (i * MAX_PACKET_SIZE) as u64;
                desc.length = 0;
                desc.cmd = 0;
                desc.status = 1;
            }
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
            write32(self.base_vaddr, RCTL, (1 << 1) | (1 << 15) | (1 << 26) | (1 << 30));
            write32(self.base_vaddr, TCTL, (1 << 1) | (1 << 3) | (1 << 4) | (0x40 << 4) | (0x10 << 12));
            write32(self.base_vaddr, CTRL, read32(self.base_vaddr, CTRL) | (1 << 1));
            self.link_up = true;
        }
        Ok(())
    }

    pub fn read_link_status(&self) -> bool {
        unsafe { read32(self.base_vaddr, STATUS) & (1 << 1) != 0 }
    }

    pub fn eeprom_word(&self, address: u8) -> u16 {
        unsafe {
            write32(self.base_vaddr, EERD, ((address as u32) << 8) | 1);
            for _ in 0..100_000 {
                let value = read32(self.base_vaddr, EERD);
                if value & 1 != 0 { return (value >> 16) as u16; }
            }
        }
        0
    }

    pub fn mac_address(&self) -> MacAddress { self.mac }
}

impl NetworkDevice for E1000Device {
    fn mac(&self) -> MacAddress { self.mac }

    fn transmit(&mut self, frame: &[u8]) -> Result<(), NetError> {
        if !self.link_up { return Err(NetError::DeviceDown); }
        if frame.len() > MAX_PACKET_SIZE { return Err(NetError::BufferTooSmall); }
        let idx = self.tx_index as usize;
        unsafe {
            let desc = &mut *self.tx_desc.add(idx);
            if desc.status & 1 == 0 && desc.cmd != 0 { return Err(NetError::QueueFull); }
            desc.length = frame.len() as u16;
            desc.cmd = TX_CMD_EOP | TX_CMD_IFCS | TX_CMD_RS;
            desc.status = 0;
            let buffer = self.tx_buffer_phys + (idx * MAX_PACKET_SIZE) as u64;
            core::ptr::copy_nonoverlapping(frame.as_ptr(), buffer as *mut u8, frame.len());
            desc.addr = buffer;
            compiler_fence(Ordering::Release);
            write32(self.base_vaddr, TDT, ((idx + 1) % TX_DESC_COUNT) as u32);
        }
        self.tx_index = ((idx + 1) % TX_DESC_COUNT) as u16;
        Ok(())
    }

    fn receive(&mut self, out: &mut [u8]) -> Result<usize, NetError> {
        if !self.link_up { return Err(NetError::DeviceDown); }
        let idx = self.rx_index as usize;
        unsafe {
            let desc = &mut *self.rx_desc.add(idx);
            if desc.status & RX_EOP == 0 { return Err(NetError::NoPacketAvailable); }
            let len = desc.length as usize;
            if len > out.len() { return Err(NetError::BufferTooSmall); }
            let buffer = self.rx_buffer_phys + (idx * MAX_PACKET_SIZE) as u64;
            core::ptr::copy_nonoverlapping(buffer as *const u8, out.as_mut_ptr(), len);
            desc.status = 0;
            compiler_fence(Ordering::Release);
            write32(self.base_vaddr, RDT, idx as u32);
            self.rx_index = ((idx + 1) % RX_DESC_COUNT) as u16;
            Ok(len)
        }
    }

    fn has_packets(&self) -> bool {
        unsafe { (*self.rx_desc.add(self.rx_index as usize)).status & RX_EOP != 0 }
    }
}

pub const fn rx_descriptor_count() -> usize { RX_DESC_COUNT }
pub const fn tx_descriptor_count() -> usize { TX_DESC_COUNT }
