//! AArch64 Zero-Trust Capability Security Subsystem
//!
//! Enforces zero ambient authority: all privileged operations (UART logging,
//! framebuffer scanout, timer reading, storage blocks, IPC message channels)
//! require an explicit, unforgeable capability token.

extern crate alloc;
use alloc::vec::Vec;
use core::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct CapabilityId(pub u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CapabilityType {
    SerialOutput,
    FrameBufferDirect,
    TimerTickRead,
    IpcSend,
    IpcReceive,
    StorageBlockRead,
    StorageBlockWrite,
    TelephonyModem,
    MeshNetworkTransmit,
    MeshNetworkReceive,
    AiInferenceExecution,
    AndroidContainerGuest,
    WasmExecute,
    LiteVerseStorage,
    SocialRecoveryEnclave,
    OtaUpdateManagement,
    AudioStreamDirect,
    IntentOrchestrator,
    CameraCaptureDirect,
    SmsDispatchDirect,
    NfcSecureElement,
    SensorTelemetryRead,
    BluetoothDirect,
    WifiNetworkDirect,
    GpsTelemetryRead,
    HapticsActuatorDirect,
    BiometricAuthEnclave,
    UsbPowerDeliveryDirect,
    NeuralProcessingUnitDirect,
    CpuSmpGovernorDirect,
    SimCardSecureElement,
    DisplayControllerDirect,
    StrongBoxSecureEnclave,
    RemoteKillSwitchDirect,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Rights(pub u32);

impl Rights {
    pub const NONE: Rights = Rights(0);
    pub const READ: Rights = Rights(1 << 0);
    pub const WRITE: Rights = Rights(1 << 1);
    pub const EXECUTE: Rights = Rights(1 << 2);
    pub const GRANT: Rights = Rights(1 << 3);
    pub const INVOKE: Rights = Rights(1 << 4);

    pub fn contains(self, required: Rights) -> bool {
        (self.0 & required.0) == required.0
    }

    pub fn union(self, other: Rights) -> Rights {
        Rights(self.0 | other.0)
    }
}

impl core::ops::BitOr for Rights {
    type Output = Self;
    #[inline]
    fn bitor(self, rhs: Self) -> Self {
        Rights(self.0 | rhs.0)
    }
}

impl fmt::Debug for Rights {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Rights({:#x})", self.0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CapError {
    CapabilityNotFound,
    PermissionDenied,
    TypeMismatch,
    TargetMismatch,
    TableFull,
}

#[derive(Debug, Clone)]
pub struct Capability {
    pub id: CapabilityId,
    pub cap_type: CapabilityType,
    pub rights: Rights,
    pub target_id: Option<u32>,
}

#[derive(Debug, Clone)]
pub struct CapabilityTable {
    pub entries: Vec<Capability>,
    pub next_id: u64,
}

impl CapabilityTable {
    pub const fn new() -> Self {
        Self {
            entries: Vec::new(),
            next_id: 1,
        }
    }

    pub fn mint(&mut self, cap_type: CapabilityType, rights: Rights, target_id: Option<u32>) -> CapabilityId {
        let id = CapabilityId(self.next_id);
        self.next_id += 1;
        self.entries.push(Capability {
            id,
            cap_type,
            rights,
            target_id,
        });
        id
    }

    pub fn check(&self, cap_type: CapabilityType, required_rights: Rights) -> Result<&Capability, CapError> {
        for cap in &self.entries {
            if cap.cap_type == cap_type {
                if cap.rights.contains(required_rights) {
                    return Ok(cap);
                } else {
                    return Err(CapError::PermissionDenied);
                }
            }
        }
        Err(CapError::CapabilityNotFound)
    }

    pub fn check_targeted(&self, cap_type: CapabilityType, required_rights: Rights, target: u32) -> Result<&Capability, CapError> {
        for cap in &self.entries {
            if cap.cap_type == cap_type {
                if cap.target_id == Some(target) || cap.target_id.is_none() {
                    if cap.rights.contains(required_rights) {
                        return Ok(cap);
                    } else {
                        return Err(CapError::PermissionDenied);
                    }
                }
            }
        }
        Err(CapError::TargetMismatch)
    }
}
