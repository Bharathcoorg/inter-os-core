//! Hardware Drivers for Inter OS
//!
//! Exposes modular hardware controller drivers:
//! - Direct Rendering Manager & KMS multi-surface compositor (`display_kms`)
//! - Flattened Device Tree hardware discovery parser (`fdt`)
//! - PMIC Fuel Gauge & Interactive DVFS Power Governor (`pmic`)
//! - Capacitive Multi-Touch Digitizer & Gesture Engine (`touch_i2c`, `touch_gestures`)
//! - ATA PIO / SDHCI Storage Controllers (`ata`, `sdhci`)
//! - Audio & PCI Controllers (`audio`, `pci`)

pub mod ata;
pub mod audio;
pub mod display;
pub mod display_kms;
pub mod fdt;
pub mod i2c_mmio;
pub mod input;
pub mod pci;
pub mod pmic;
pub mod ps2_mouse;
pub mod sdhci;
pub mod touch_gestures;
pub mod touch_i2c;
pub mod virtio_gpu;

pub use ata::{AtaDrive, AtaError, BlockDevice, ATA_BOOT, ATA_PRIMARY, SECTOR_SIZE};
pub use audio::{PciAudioController, PciAudioDevice};
pub use display::{DisplayDriver, KmsDisplayDriver};
pub use display_kms::{DrmKmsController, DrmPixelFormat, DrmSurface, DRM_KMS, test_drm_kms};
pub use fdt::{DiscoveredDevice, FdtParser, test_fdt_parser};
pub use i2c_mmio::{MmioI2cController, test_mmio_i2c_host};
pub use input::{InputEvent, InputEventKind, INPUT_PIPELINE};
pub use pci::{scan_pci_bus, PciAddress, PciDevice};
pub use pmic::{BatteryTelemetry, ChargingState, DvfsGovernor, DvfsProfile, PmicFuelGauge, DVFS_GOVERNOR, PMIC_FUEL_GAUGE, test_pmic_dvfs};
pub use sdhci::{SdhciController, test_sdhci_controller};
pub use touch_gestures::{ContactSlot, RecognizedGesture, TouchGestureEngine, TOUCH_ENGINE, test_touch_gestures};
pub use touch_i2c::{GoodixGt911Driver, I2cBusAdapter, MemoryI2cAdapter, test_gt911_pipeline};
pub use virtio_gpu::{VirtioGpuCtrlHdr, VirtioGpuDriver, VirtioGpuRect, VIRTIO_GPU};

/// Runs comprehensive self-tests on all modular hardware drivers.
pub fn init_and_test_drivers() {
    crate::serial_println!("--- [Testing Modular Hardware Drivers & HAL Subsystems] ---");
    test_drm_kms();
    test_fdt_parser();
    test_pmic_dvfs();
    test_touch_gestures();
    test_gt911_pipeline();
    test_sdhci_controller();
    virtio_gpu::test_virtio_gpu_structures();
    let _ = virtio_gpu::VIRTIO_GPU.lock().probe();
    crate::serial_println!(
        "--- [Hardware driver register models exercised; no physical controller is bound] ---"
    );
}
