//! Hardware Drivers for Inter OS
//!
//! Exposes modular hardware controller drivers (Serial UART, Framebuffer,
//! ATA Storage, Keyboard, PIC, PIT).

pub mod ata;
pub mod audio;
pub mod i2c_mmio;
pub mod pci;
pub mod sdhci;
pub mod touch_i2c;

pub use ata::{AtaDrive, AtaError, BlockDevice, ATA_PRIMARY, SECTOR_SIZE};
pub use audio::{PciAudioController, PciAudioDevice};
pub use i2c_mmio::{MmioI2cController, test_mmio_i2c_host};
pub use pci::{scan_pci_bus, PciAddress, PciDevice};
pub use sdhci::{SdhciController, test_sdhci_controller};
pub use touch_i2c::{GoodixGt911Driver, I2cBusAdapter, MemoryI2cAdapter, test_gt911_pipeline};

