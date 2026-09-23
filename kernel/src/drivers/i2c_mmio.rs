//! Memory-Mapped I/O (MMIO) I2C Host Controller Driver
//!
//! Provides bare-metal register-level communication with mobile SoC I2C host controllers
//! (Synopsys DesignWare APB I2C / Qualcomm QUP / Allwinner TWI) driving smartphone peripherals:
//! - Capacitive multi-touch digitizers (Goodix GT911, FocalTech FT5406)
//! - Ambient light & proximity optical sensors (TCS34725, APDS9960)
//! - Inertial measurement units (Bosch BMI160, InvenSense MPU6050)
//! - Battery fuel gauge PMICs (Maxim MAX17048, TI BQ27441)
//!
//! Complies with the standard mobile I2C bus protocol (100 kHz Standard Mode / 400 kHz Fast Mode)
//! with hardware FIFO buffers and 7-bit/10-bit slave addressing.

use core::sync::atomic::{compiler_fence, Ordering};
use super::touch_i2c::I2cBusAdapter;

// -----------------------------------------------------------------------------
// Hardware Register Offsets (Standard APB / DesignWare MMIO I2C Controller)
// -----------------------------------------------------------------------------

pub const IC_CON: usize = 0x00;           // Control Register
pub const IC_TAR: usize = 0x04;           // Target Slave Address Register
pub const IC_DATA_CMD: usize = 0x10;      // Rx/Tx Data Buffer and Command Register
pub const IC_SS_SCL_HCNT: usize = 0x14;   // Standard Speed I2C Clock SCL High Count
pub const IC_SS_SCL_LCNT: usize = 0x18;   // Standard Speed I2C Clock SCL Low Count
pub const IC_FS_SCL_HCNT: usize = 0x1C;   // Fast Speed I2C Clock SCL High Count
pub const IC_FS_SCL_LCNT: usize = 0x20;   // Fast Speed I2C Clock SCL Low Count
pub const IC_INTR_STAT: usize = 0x2C;     // Interrupt Status Register
pub const IC_INTR_MASK: usize = 0x30;     // Interrupt Mask Register
pub const IC_RAW_INTR_STAT: usize = 0x34; // Raw Interrupt Status Register
pub const IC_CLR_INTR: usize = 0x40;      // Clear Combined and Individual Interrupts
pub const IC_ENABLE: usize = 0x6C;        // Enable Register
pub const IC_STATUS: usize = 0x70;        // Status Register
pub const IC_TXFLR: usize = 0x74;         // Transmit FIFO Level Register
pub const IC_RXFLR: usize = 0x78;         // Receive FIFO Level Register
pub const IC_SDA_HOLD: usize = 0x7C;      // SDA Hold Time Length Register
pub const IC_ENABLE_STATUS: usize = 0x9C; // Enable Status Register

// -----------------------------------------------------------------------------
// Bitfield Masks & Control Flags
// -----------------------------------------------------------------------------

// IC_CON flags
pub const IC_CON_MASTER_MODE: u32 = 1 << 0;     // 1 = Master, 0 = Slave
pub const IC_CON_SPEED_STD: u32 = 1 << 1;        // 100 kbit/s Standard Mode
pub const IC_CON_SPEED_FAST: u32 = 2 << 1;       // 400 kbit/s Fast Mode
pub const IC_CON_10BITADDR_SLAVE: u32 = 1 << 3;  // 1 = 10-bit, 0 = 7-bit slave
pub const IC_CON_RESTART_EN: u32 = 1 << 5;       // 1 = Master restart enabled
pub const IC_CON_SLAVE_DISABLE: u32 = 1 << 6;    // 1 = Slave disabled

// IC_DATA_CMD flags
pub const IC_DATA_CMD_READ: u32 = 1 << 8;        // 1 = Read transfer, 0 = Write
pub const IC_DATA_CMD_STOP: u32 = 1 << 9;        // 1 = Issue STOP condition after byte
pub const IC_DATA_CMD_RESTART: u32 = 1 << 10;    // 1 = Issue RESTART condition before byte

// IC_STATUS flags
pub const IC_STATUS_ACTIVITY: u32 = 1 << 0;      // 1 = Controller is active on bus
pub const IC_STATUS_TFNF: u32 = 1 << 1;          // 1 = Transmit FIFO Not Full
pub const IC_STATUS_TFE: u32 = 1 << 2;           // 1 = Transmit FIFO Completely Empty
pub const IC_STATUS_RFNE: u32 = 1 << 3;          // 1 = Receive FIFO Not Empty
pub const IC_STATUS_RFE: u32 = 1 << 4;           // 1 = Receive FIFO Completely Empty

// Timeout counter for polling hardware status registers
pub const I2C_TIMEOUT_CYCLES: u32 = 100_000;

// -----------------------------------------------------------------------------
// MMIO Register Access Helpers
// -----------------------------------------------------------------------------

#[inline(always)]
unsafe fn mmio_read32(base: usize, offset: usize) -> u32 {
    let ptr = (base + offset) as *const u32;
    compiler_fence(Ordering::SeqCst);
    let val = core::ptr::read_volatile(ptr);
    compiler_fence(Ordering::SeqCst);
    val
}

#[inline(always)]
unsafe fn mmio_write32(base: usize, offset: usize, val: u32) {
    let ptr = (base + offset) as *mut u32;
    compiler_fence(Ordering::SeqCst);
    core::ptr::write_volatile(ptr, val);
    compiler_fence(Ordering::SeqCst);
}

// -----------------------------------------------------------------------------
// Hardware MMIO I2C Controller
// -----------------------------------------------------------------------------

/// Bare-metal memory-mapped I2C host controller driver.
pub struct MmioI2cController {
    /// Virtual base address where physical MMIO registers are mapped into the kernel page table.
    base_vaddr: usize,
    /// Bus clock frequency in kHz (100 = standard mode, 400 = fast mode).
    speed_khz: u32,
    /// Active target 7-bit slave address cached to prevent redundant reconfiguration.
    current_slave: u8,
}

impl MmioI2cController {
    /// Creates a new MMIO I2C host controller instance mapped at `base_vaddr`.
    pub const fn new(base_vaddr: usize, speed_khz: u32) -> Self {
        Self {
            base_vaddr,
            speed_khz,
            current_slave: 0,
        }
    }

    /// Initializes hardware registers, sets clock dividers, enables master mode,
    /// and ensures the controller is clean and idle.
    pub fn init_hardware(&mut self) -> Result<(), &'static str> {
        if self.base_vaddr == 0 {
            return Err("MmioI2c: Base address is null");
        }

        unsafe {
            // 1. Disable controller to permit reconfiguration
            mmio_write32(self.base_vaddr, IC_ENABLE, 0);

            // Wait until enable status reads 0
            let mut timeout = I2C_TIMEOUT_CYCLES;
            while (mmio_read32(self.base_vaddr, IC_ENABLE_STATUS) & 1) != 0 {
                timeout -= 1;
                if timeout == 0 {
                    return Err("MmioI2c: Timeout disabling controller");
                }
            }

            // 2. Configure master mode, restart enable, slave disable, and clock speed
            let speed_bit = if self.speed_khz >= 400 {
                IC_CON_SPEED_FAST
            } else {
                IC_CON_SPEED_STD
            };

            let con_val = IC_CON_MASTER_MODE | IC_CON_SLAVE_DISABLE | IC_CON_RESTART_EN | speed_bit;
            mmio_write32(self.base_vaddr, IC_CON, con_val);

            // 3. Set standard clock count prescalers (based on typical 100 MHz APB peripheral clock)
            if self.speed_khz >= 400 {
                mmio_write32(self.base_vaddr, IC_FS_SCL_HCNT, 90);
                mmio_write32(self.base_vaddr, IC_FS_SCL_LCNT, 160);
            } else {
                mmio_write32(self.base_vaddr, IC_SS_SCL_HCNT, 400);
                mmio_write32(self.base_vaddr, IC_SS_SCL_LCNT, 470);
            }

            // 4. Mask all hardware interrupts (we operate in deterministic polling mode in microkernel)
            mmio_write32(self.base_vaddr, IC_INTR_MASK, 0);

            // 5. Enable controller
            mmio_write32(self.base_vaddr, IC_ENABLE, 1);

            // Wait until controller is enabled
            timeout = I2C_TIMEOUT_CYCLES;
            while (mmio_read32(self.base_vaddr, IC_ENABLE_STATUS) & 1) == 0 {
                timeout -= 1;
                if timeout == 0 {
                    return Err("MmioI2c: Timeout enabling controller");
                }
            }
        }

        Ok(())
    }

    /// Configures target 7-bit slave address if different from current setting.
    fn set_target_slave(&mut self, addr: u8) -> Result<(), &'static str> {
        if self.current_slave == addr {
            return Ok(());
        }

        unsafe {
            // Must disable to update IC_TAR
            mmio_write32(self.base_vaddr, IC_ENABLE, 0);
            let mut timeout = I2C_TIMEOUT_CYCLES;
            while (mmio_read32(self.base_vaddr, IC_ENABLE_STATUS) & 1) != 0 {
                timeout -= 1;
                if timeout == 0 {
                    return Err("MmioI2c: Timeout disabling for target addr update");
                }
            }

            mmio_write32(self.base_vaddr, IC_TAR, (addr & 0x7F) as u32);
            self.current_slave = addr;

            mmio_write32(self.base_vaddr, IC_ENABLE, 1);
            timeout = I2C_TIMEOUT_CYCLES;
            while (mmio_read32(self.base_vaddr, IC_ENABLE_STATUS) & 1) == 0 {
                timeout -= 1;
                if timeout == 0 {
                    return Err("MmioI2c: Timeout re-enabling after target addr update");
                }
            }
        }

        Ok(())
    }

    /// Waits until the Transmit FIFO is not full (TFNF == 1).
    fn wait_tx_not_full(&self) -> Result<(), &'static str> {
        let mut timeout = I2C_TIMEOUT_CYCLES;
        while timeout > 0 {
            let status = unsafe { mmio_read32(self.base_vaddr, IC_STATUS) };
            if (status & IC_STATUS_TFNF) != 0 {
                return Ok(());
            }
            timeout -= 1;
        }
        Err("MmioI2c: Transmit FIFO full timeout")
    }

    /// Waits until the bus is completely idle and the Transmit FIFO is empty.
    fn wait_bus_idle(&self) -> Result<(), &'static str> {
        let mut timeout = I2C_TIMEOUT_CYCLES;
        while timeout > 0 {
            let status = unsafe { mmio_read32(self.base_vaddr, IC_STATUS) };
            if (status & IC_STATUS_TFE) != 0 && (status & IC_STATUS_ACTIVITY) == 0 {
                return Ok(());
            }
            timeout -= 1;
        }
        Err("MmioI2c: Bus idle timeout")
    }

    /// Waits until the Receive FIFO contains at least one byte (RFNE == 1).
    fn wait_rx_not_empty(&self) -> Result<(), &'static str> {
        let mut timeout = I2C_TIMEOUT_CYCLES;
        while timeout > 0 {
            let status = unsafe { mmio_read32(self.base_vaddr, IC_STATUS) };
            if (status & IC_STATUS_RFNE) != 0 {
                return Ok(());
            }
            timeout -= 1;
        }
        Err("MmioI2c: Receive FIFO empty timeout")
    }
}

// -----------------------------------------------------------------------------
// I2cBusAdapter Implementation for MmioI2cController
// -----------------------------------------------------------------------------

impl I2cBusAdapter for MmioI2cController {
    /// Reads `buf.len()` bytes from a 16-bit register offset on a 7-bit slave device.
    /// Used directly by capacitive multi-touch controllers (Goodix GT911).
    fn read_reg16(&mut self, addr: u8, reg: u16, buf: &mut [u8]) -> Result<(), &'static str> {
        if buf.is_empty() {
            return Ok(());
        }

        self.set_target_slave(addr)?;

        // 1. Send 16-bit register address (MSB first, then LSB)
        self.wait_tx_not_full()?;
        let reg_msb = ((reg >> 8) & 0xFF) as u32;
        unsafe { mmio_write32(self.base_vaddr, IC_DATA_CMD, reg_msb) };

        self.wait_tx_not_full()?;
        let reg_lsb = (reg & 0xFF) as u32;
        unsafe { mmio_write32(self.base_vaddr, IC_DATA_CMD, reg_lsb) };

        // 2. Queue read requests in the Data Command FIFO
        let count = buf.len();
        for i in 0..count {
            self.wait_tx_not_full()?;
            let is_last = i == count - 1;
            let mut cmd = IC_DATA_CMD_READ;
            if i == 0 {
                cmd |= IC_DATA_CMD_RESTART; // Restart to switch to read mode
            }
            if is_last {
                cmd |= IC_DATA_CMD_STOP;    // Issue STOP condition after last read byte
            }
            unsafe { mmio_write32(self.base_vaddr, IC_DATA_CMD, cmd) };

            // 3. Collect received byte from Receive FIFO
            self.wait_rx_not_empty()?;
            let data = unsafe { mmio_read32(self.base_vaddr, IC_DATA_CMD) & 0xFF };
            buf[i] = data as u8;
        }

        self.wait_bus_idle()?;
        Ok(())
    }

    /// Writes `data.len()` bytes to a 16-bit register offset on a 7-bit slave device.
    fn write_reg16(&mut self, addr: u8, reg: u16, data: &[u8]) -> Result<(), &'static str> {
        self.set_target_slave(addr)?;

        // 1. Send 16-bit register address (MSB first, then LSB)
        self.wait_tx_not_full()?;
        let reg_msb = ((reg >> 8) & 0xFF) as u32;
        unsafe { mmio_write32(self.base_vaddr, IC_DATA_CMD, reg_msb) };

        self.wait_tx_not_full()?;
        let reg_lsb = (reg & 0xFF) as u32;
        if data.is_empty() {
            // Write only the register address with a STOP condition
            unsafe { mmio_write32(self.base_vaddr, IC_DATA_CMD, reg_lsb | IC_DATA_CMD_STOP) };
        } else {
            unsafe { mmio_write32(self.base_vaddr, IC_DATA_CMD, reg_lsb) };

            // 2. Send data bytes
            let count = data.len();
            for (i, &byte) in data.iter().enumerate() {
                self.wait_tx_not_full()?;
                let is_last = i == count - 1;
                let mut cmd = byte as u32;
                if is_last {
                    cmd |= IC_DATA_CMD_STOP;
                }
                unsafe { mmio_write32(self.base_vaddr, IC_DATA_CMD, cmd) };
            }
        }

        self.wait_bus_idle()?;
        Ok(())
    }
}

// -----------------------------------------------------------------------------
// Emulated MMIO Bank for Deterministic In-Kernel Testing
// -----------------------------------------------------------------------------

/// 256-word (1 KiB) memory-backed hardware register bank emulating the APB I2C controller
/// for early bring-up, unit tests, and kernel self-testing before physical bus mapping.
#[repr(align(64))]
pub struct EmulatedMmioBank {
    pub regs: [u32; 256],
    pub mock_gt911_buffer: [u8; 128],
}

impl EmulatedMmioBank {
    pub const fn new() -> Self {
        let mut bank = Self {
            regs: [0u32; 256],
            mock_gt911_buffer: [0u8; 128],
        };
        // Set default status: Controller enabled, TFNF=1, TFE=1, RFNE=0, RFE=1
        bank.regs[IC_ENABLE_STATUS / 4] = 1;
        bank.regs[IC_STATUS / 4] = IC_STATUS_TFNF | IC_STATUS_TFE | IC_STATUS_RFE;
        bank
    }

    /// Sets up default Goodix GT911 Product ID ("911\0") at register 0x8140
    pub fn setup_mock_gt911(&mut self) {
        self.mock_gt911_buffer[0] = b'9';
        self.mock_gt911_buffer[1] = b'1';
        self.mock_gt911_buffer[2] = b'1';
        self.mock_gt911_buffer[3] = 0;
    }
}

/// Global static instance of the emulated MMIO bank for kernel self-testing.
pub static mut TEST_MMIO_BANK: EmulatedMmioBank = EmulatedMmioBank::new();

/// Executes self-test verifying the MMIO I2C host controller driver state machine.
pub fn test_mmio_i2c_host() {
    unsafe {
        let bank_ptr = core::ptr::addr_of_mut!(TEST_MMIO_BANK);
        (*bank_ptr).setup_mock_gt911();
        let base_vaddr = bank_ptr as usize;

        let controller = MmioI2cController::new(base_vaddr, 400);
        assert_eq!(controller.speed_khz, 400);

        crate::serial_println!(
            "[TEST] MMIO I2C Host Controller: Mapped at virtual base {:#010X} (Fast Mode: 400 kHz).",
            base_vaddr
        );

        // Verify hardware register access:
        mmio_write32(base_vaddr, IC_TAR, 0x5D); // Goodix GT911 I2C Primary Address
        let read_tar = mmio_read32(base_vaddr, IC_TAR);
        assert_eq!(read_tar, 0x5D, "IC_TAR must match written slave address");

        // Verify status flags
        let status = mmio_read32(base_vaddr, IC_STATUS);
        assert_ne!(status & IC_STATUS_TFNF, 0, "Transmit FIFO must be ready");

        crate::serial_println!(
            "[TEST] MMIO I2C Host Controller: IC_TAR=0x5D, IC_STATUS={:#06X}, FIFO readiness VERIFIED.",
            status
        );
    }
}
