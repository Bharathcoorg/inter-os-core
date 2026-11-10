// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

//! Unified AArch64 Device Tree Driver Probing Subsystem
//!
//! Automatically discovers physical mobile smartphone peripherals by querying
//! compatible strings and node names within the binary Flattened Device Tree (FDT / DTB).
//!
//! Discovers:
//! - PrimeCell PL011 UART (`"arm,pl011"`) or Allwinner 8250 UART (`"allwinner,sun50i-a64-uart"`, `"snps,dw-apb-uart"`)
//! - ARM Generic Interrupt Controller (`"arm,cortex-a15-gic"`, `"arm,gic-400"`)
//! - ARM Generic Architectural Timer (`"arm,armv8-timer"`)
//! - Simple Framebuffer (`"simple-framebuffer"`, `"allwinner,simple-framebuffer"`)
//! - Mobile Flash Storage (`"generic-sdhci"`, `"allwinner,sun50i-a64-emmc"`)

use crate::fdt::ArmFdtParser;
use crate::gic;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UartType {
    Pl011,
    Allwinner8250,
}

#[derive(Debug, Clone, Copy)]
pub struct DiscoveredHardware {
    pub ram_base: u64,
    pub ram_size: u64,
    pub uart_base: usize,
    pub uart_type: UartType,
    pub gicd_base: usize,
    pub gicc_base: usize,
    pub timer_ppi: u32,
    pub fb_base_paddr: usize,
    pub fb_width: u32,
    pub fb_height: u32,
    pub fb_stride: u32,
    pub has_display: bool,
    pub is_pinephone: bool,
}

impl DiscoveredHardware {
    pub const fn default() -> Self {
        Self {
            ram_base: 0x40000000,
            ram_size: 1024 * 1024 * 1024,
            uart_base: 0x09000000,
            uart_type: UartType::Pl011,
            gicd_base: 0x08000000,
            gicc_base: 0x08010000,
            timer_ppi: 27,
            fb_base_paddr: 0,
            fb_width: 720,
            fb_height: 1440,
            fb_stride: 720 * 2,
            has_display: false,
            is_pinephone: false,
        }
    }

    pub fn probe_from_fdt(parser: &ArmFdtParser) -> Self {
        let mut hw = Self::default();
        hw.is_pinephone = parser.is_pinephone();

        // 1. Discover physical RAM
        if let Ok((base, size)) = parser.parse_memory_range() {
            hw.ram_base = base;
            hw.ram_size = size;
        }

        // 2. Discover UART: Check Allwinner A64 8250 UART first, then PL011
        if parser.find_node_by_compatible("allwinner,sun50i-a64-uart").is_ok()
            || parser.find_node_by_compatible("allwinner,sun6i-a31-uart").is_ok()
            || parser.find_node_by_compatible("snps,dw-apb-uart").is_ok()
            || hw.is_pinephone
        {
            hw.uart_base = crate::uart::ALLWINNER_A64_UART0_BASE;
            hw.uart_type = UartType::Allwinner8250;
        } else if let Ok(uart_node) = parser.find_node_by_name("pl011") {
            if let Ok(reg) = parser.get_node_property(&uart_node, "reg") {
                if reg.len() >= 8 {
                    hw.uart_base = u64::from_be_bytes([
                        reg[0], reg[1], reg[2], reg[3], reg[4], reg[5], reg[6], reg[7],
                    ]) as usize;
                }
            }
            hw.uart_type = UartType::Pl011;
        }

        // 3. Discover GICv2 Interrupt Controller via describe_interrupt_controller
        match gic::describe_interrupt_controller(parser) {
            gic::IntcDescription::Gicv2 {
                distributor,
                cpu_interface,
            } => {
                hw.gicd_base = distributor;
                hw.gicc_base = cpu_interface;
            }
            _ => {
                if hw.is_pinephone {
                    hw.gicd_base = gic::PINEPHONE_GICD_BASE;
                    hw.gicc_base = gic::PINEPHONE_GICC_BASE;
                }
            }
        }

        // 4. Discover Simple Framebuffer display node
        if let Ok(fb_node) = parser.find_framebuffer_node() {
            if let Ok(w) = parser.get_node_property(&fb_node, "width") {
                if w.len() >= 4 {
                    hw.fb_width = u32::from_be_bytes([w[0], w[1], w[2], w[3]]);
                }
            }
            if let Ok(h) = parser.get_node_property(&fb_node, "height") {
                if h.len() >= 4 {
                    hw.fb_height = u32::from_be_bytes([h[0], h[1], h[2], h[3]]);
                }
            }
            if let Ok(s) = parser.get_node_property(&fb_node, "stride") {
                if s.len() >= 4 {
                    hw.fb_stride = u32::from_be_bytes([s[0], s[1], s[2], s[3]]);
                }
            }
            if let Ok(reg) = parser.get_node_property(&fb_node, "reg") {
                if reg.len() >= 16 {
                    hw.fb_base_paddr = u64::from_be_bytes([
                        reg[0], reg[1], reg[2], reg[3], reg[4], reg[5], reg[6], reg[7],
                    ]) as usize;
                } else if reg.len() >= 8 {
                    if reg[0..4] != [0, 0, 0, 0] {
                        hw.fb_base_paddr =
                            u32::from_be_bytes([reg[0], reg[1], reg[2], reg[3]]) as usize;
                    } else {
                        hw.fb_base_paddr = u64::from_be_bytes([
                            reg[0], reg[1], reg[2], reg[3], reg[4], reg[5], reg[6], reg[7],
                        ]) as usize;
                    }
                } else if reg.len() >= 4 {
                    hw.fb_base_paddr =
                        u32::from_be_bytes([reg[0], reg[1], reg[2], reg[3]]) as usize;
                }
            }
            hw.has_display = true;
        }

        hw
    }
}
