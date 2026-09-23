//! ARM Power State Coordination Interface (PSCI v1.1)
//!
//! Provides bare-metal CPU power management and multi-core SMP bring-up for mobile ARM SoCs
//! (Qualcomm Snapdragon, MediaTek Dimensity, Google Tensor, PinePhone Allwinner A64).
//!
//! Executes standard EL1 -> EL3 firmware calls (`smc` / `hvc`) conforming to the
//! ARM PSCI v1.1 specification (ARM DEN 0022D).

pub const PSCI_0_2_FN_BASE: u32 = 0x84000000;
pub const PSCI_0_2_FN64_BASE: u32 = 0xC4000000;

pub const PSCI_FN_VERSION: u32 = PSCI_0_2_FN_BASE + 0x00;
pub const PSCI_FN_CPU_SUSPEND: u32 = PSCI_0_2_FN64_BASE + 0x01;
pub const PSCI_FN_CPU_OFF: u32 = PSCI_0_2_FN_BASE + 0x02;
pub const PSCI_FN_CPU_ON: u32 = PSCI_0_2_FN64_BASE + 0x03;
pub const PSCI_FN_AFFINITY_INFO: u32 = PSCI_0_2_FN64_BASE + 0x04;
pub const PSCI_FN_SYSTEM_OFF: u32 = PSCI_0_2_FN_BASE + 0x08;
pub const PSCI_FN_SYSTEM_RESET: u32 = PSCI_0_2_FN_BASE + 0x09;

pub const PSCI_SUCCESS: i32 = 0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PsciConduit {
    Hvc,
    Smc,
    Emulated,
}

pub struct PsciController {
    pub conduit: PsciConduit,
    pub major_version: u16,
    pub minor_version: u16,
}

impl PsciController {
    pub const fn new(conduit: PsciConduit) -> Self {
        Self {
            conduit,
            major_version: 1,
            minor_version: 1,
        }
    }

    pub fn invoke_psci(&self, func_id: u32, arg0: u64, arg1: u64, arg2: u64) -> i64 {
        match self.conduit {
            PsciConduit::Emulated => match func_id {
                PSCI_FN_VERSION => 0x00010001,
                _ => PSCI_SUCCESS as i64,
            },
            #[cfg(target_arch = "aarch64")]
            PsciConduit::Smc => {
                let res: i64;
                unsafe {
                    core::arch::asm!(
                        "smc #0",
                        inout("x0") func_id as u64 => res,
                        in("x1") arg0,
                        in("x2") arg1,
                        in("x3") arg2,
                        options(nostack)
                    );
                }
                res
            }
            #[cfg(target_arch = "aarch64")]
            PsciConduit::Hvc => {
                let res: i64;
                unsafe {
                    core::arch::asm!(
                        "hvc #0",
                        inout("x0") func_id as u64 => res,
                        in("x1") arg0,
                        in("x2") arg1,
                        in("x3") arg2,
                        options(nostack)
                    );
                }
                res
            }
            #[cfg(not(target_arch = "aarch64"))]
            _ => {
                let _ = (arg0, arg1, arg2);
                PSCI_SUCCESS as i64
            }
        }
    }

    pub fn cpu_on(&self, target_cpu_mpidr: u64, entry_point_phys: u64, context_id: u64) -> Result<(), i32> {
        let ret = self.invoke_psci(PSCI_FN_CPU_ON, target_cpu_mpidr, entry_point_phys, context_id);
        if ret == 0 {
            Ok(())
        } else {
            Err(ret as i32)
        }
    }

    pub fn system_reset(&self) -> ! {
        self.invoke_psci(PSCI_FN_SYSTEM_RESET, 0, 0, 0);
        loop {
            #[cfg(target_arch = "aarch64")]
            unsafe { core::arch::asm!("wfe"); }
        }
    }

    pub fn system_off(&self) -> ! {
        self.invoke_psci(PSCI_FN_SYSTEM_OFF, 0, 0, 0);
        loop {
            #[cfg(target_arch = "aarch64")]
            unsafe { core::arch::asm!("wfe"); }
        }
    }
}
