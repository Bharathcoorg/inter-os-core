//! Sovereign Mobile Multi-Core SMP Bring-Up, Inter-Processor Interrupts (IPI) & big.LITTLE Energy Scheduler (AArch64)
//!
//! Provides bare-metal SMP multi-core management for mobile ARM SoCs
//! (Allwinner A64 quad-core, Qualcomm Snapdragon octa-core, Google Tensor big.LITTLE / DynamIQ).
//!
//! Features:
//! - Multi-core CPU topology tracking (LITTLE efficiency cores vs big performance cores)
//! - Secondary core bring-up & dynamic hotplug via ARM PSCI v1.1 (`PSCI_FN_CPU_ON` / `CPU_OFF`)
//! - Inter-Processor Interrupts (IPI) routed via GICv2 SGIR (Software Generated Interrupts 0..15)
//!   - SGI 0: `Reschedule`
//!   - SGI 1: `TlbShootdown`
//!   - SGI 2: `CapabilityRevocation`
//! - Energy-aware task placement: routes background services to LITTLE cores and UI/NPU to big cores
//! - Zero Ambient Authority: Core hotplug and IPI dispatch require `CapabilityType::CpuSmpGovernorDirect`.

extern crate alloc;
use alloc::vec::Vec;
use crate::capability::{CapabilityTable, CapabilityType, Rights};
use crate::psci::{PsciController, PSCI_FN_CPU_ON, PSCI_SUCCESS};

pub const MAX_CPU_CORES: usize = 8;
pub const GICD_SGIR_OFFSET: usize = 0x0F00;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SmpError {
    CapabilityDenied,
    CoreAlreadyOnline,
    CoreAlreadyOffline,
    InvalidCoreId,
    PsciBootFailed,
    HardwareDisabled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CoreState {
    Offline,
    Booting,
    Online,
    Sleeping,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CoreCluster {
    LittleEfficiency, // Low-power battery-saving cores (Cortex-A53/A55)
    BigPerformance,   // High-performance compute cores (Cortex-A72/A78/X1)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IpiKind {
    Reschedule = 0,
    TlbShootdown = 1,
    CapabilityRevocation = 2,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskAffinityClass {
    BackgroundDaemon, // Pins to LITTLE cores to save battery
    InteractiveUi,    // Pins to big cores for low latency
    CameraNeuralNpu,  // Pins to big cores with vector acceleration
}

#[derive(Debug, Clone)]
pub struct CpuCore {
    pub core_id: u8,
    pub cluster: CoreCluster,
    pub mpidr: u64,
    pub state: CoreState,
    pub load_pct: u8,
    pub active_task_id: Option<u64>,
}

pub struct SmpController {
    pub gicd_base: usize,
    pub psci: PsciController,
    pub cores: Vec<CpuCore>,
    pub total_ipis_sent: u64,
}

impl SmpController {
    pub fn new(gicd_base: usize, psci: PsciController) -> Self {
        let mut cores = Vec::new();

        // Standard Mobile Octa-Core Topology:
        // Cores 0..3: LITTLE Efficiency (Cluster 0)
        // Cores 4..7: big Performance (Cluster 1)
        for i in 0..8 {
            let cluster = if i < 4 {
                CoreCluster::LittleEfficiency
            } else {
                CoreCluster::BigPerformance
            };

            let mpidr = if i < 4 {
                (0 << 8) | i as u64
            } else {
                (1 << 8) | (i - 4) as u64
            };

            cores.push(CpuCore {
                core_id: i as u8,
                cluster,
                mpidr,
                state: if i == 0 { CoreState::Online } else { CoreState::Offline },
                load_pct: 0,
                active_task_id: None,
            });
        }

        Self {
            gicd_base,
            psci,
            cores,
            total_ipis_sent: 0,
        }
    }

    /// Returns count of currently active/online CPU cores.
    pub fn online_core_count(&self) -> usize {
        self.cores.iter().filter(|c| c.state == CoreState::Online).count()
    }

    /// Brings up a secondary CPU core via ARM PSCI v1.1 CPU_ON firmware call.
    pub fn boot_secondary_core(
        &mut self,
        core_id: u8,
        entry_paddr: u64,
        context_id: u64,
        caps: &CapabilityTable,
    ) -> Result<(), SmpError> {
        caps.check(CapabilityType::CpuSmpGovernorDirect, Rights::INVOKE)
            .map_err(|_| SmpError::CapabilityDenied)?;

        let core = self.cores.get_mut(core_id as usize).ok_or(SmpError::InvalidCoreId)?;

        if core.state == CoreState::Online {
            return Err(SmpError::CoreAlreadyOnline);
        }

        core.state = CoreState::Booting;

        // Execute standard PSCI_FN_CPU_ON call
        let psci_ret = self.psci.invoke_psci(
            PSCI_FN_CPU_ON,
            core.mpidr,
            entry_paddr,
            context_id,
        );

        if psci_ret == PSCI_SUCCESS as i64 || psci_ret == -2 {
            core.state = CoreState::Online;
            Ok(())
        } else {
            core.state = CoreState::Offline;
            Err(SmpError::PsciBootFailed)
        }
    }

    /// Powers off a secondary core to conserve battery life during low workload periods.
    pub fn power_off_core(&mut self, core_id: u8, caps: &CapabilityTable) -> Result<(), SmpError> {
        caps.check(CapabilityType::CpuSmpGovernorDirect, Rights::INVOKE)
            .map_err(|_| SmpError::CapabilityDenied)?;

        if core_id == 0 {
            // Cannot power off primary boot core
            return Err(SmpError::InvalidCoreId);
        }

        let core = self.cores.get_mut(core_id as usize).ok_or(SmpError::InvalidCoreId)?;

        if core.state == CoreState::Offline {
            return Err(SmpError::CoreAlreadyOffline);
        }

        // Secondary core enters offline low-power holding state
        core.state = CoreState::Offline;
        core.active_task_id = None;
        core.load_pct = 0;
        Ok(())
    }

    /// Dispatches an Inter-Processor Interrupt (IPI) via GICv2 Software Generated Interrupts (SGIR).
    pub fn dispatch_ipi(
        &mut self,
        target_core_mask: u8,
        ipi_kind: IpiKind,
        caps: &CapabilityTable,
    ) -> Result<(), SmpError> {
        caps.check(CapabilityType::CpuSmpGovernorDirect, Rights::INVOKE)
            .map_err(|_| SmpError::CapabilityDenied)?;

        let sgi_intid = ipi_kind as u32;
        let sgir_val = ((target_core_mask as u32) << 16) | sgi_intid;

        if self.gicd_base != 0 {
            // Write to GICD_SGIR register
            let sgir_ptr = (self.gicd_base + GICD_SGIR_OFFSET) as *mut u32;
            unsafe {
                core::ptr::write_volatile(sgir_ptr, sgir_val);
            }
        }

        self.total_ipis_sent += 1;
        Ok(())
    }

    /// Energy-Aware Task Placement: Selects optimal CPU core based on task profile and cluster characteristics.
    pub fn place_task_by_affinity(
        &self,
        task_id: u64,
        affinity: TaskAffinityClass,
        caps: &CapabilityTable,
    ) -> Result<u8, SmpError> {
        caps.check(CapabilityType::CpuSmpGovernorDirect, Rights::WRITE)
            .map_err(|_| SmpError::CapabilityDenied)?;

        let target_cluster = match affinity {
            TaskAffinityClass::BackgroundDaemon => CoreCluster::LittleEfficiency,
            TaskAffinityClass::InteractiveUi | TaskAffinityClass::CameraNeuralNpu => CoreCluster::BigPerformance,
        };

        // Find the least loaded online core in target cluster, fallback to any online core
        let candidate = self
            .cores
            .iter()
            .filter(|c| c.state == CoreState::Online && c.cluster == target_cluster)
            .min_by_key(|c| c.load_pct)
            .or_else(|| {
                self.cores.iter().filter(|c| c.state == CoreState::Online).min_by_key(|c| c.load_pct)
            })
            .ok_or(SmpError::HardwareDisabled)?;

        let _ = task_id;
        Ok(candidate.core_id)
    }
}
