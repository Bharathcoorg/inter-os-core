// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

//! RISC-V 64 Zero-Trust Capability Security Subsystem
//!
//! Enforces zero ambient authority: all privileged operations (UART logging,
//! framebuffer scanout, timer reading, storage blocks, IPC message channels)
//! require an explicit, unforgeable capability token.

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
    AndroidContainerGuest,
    WasmExecute,
    SocialRecoveryEnclave,
    OtaUpdateManagement,
    AudioStreamDirect,
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
    InputDevice,
    DisplayServer,
    FileSystemRead,
    FileSystemWrite,
    EventPoll,
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
    pub const ADMIN: Rights = Rights(1 << 5);

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

pub const MAX_CAPS: usize = 32;

#[derive(Debug, Clone, Copy)]
pub struct Capability {
    pub id: CapabilityId,
    pub cap_type: CapabilityType,
    pub rights: Rights,
    pub target_id: Option<u32>,
}

#[derive(Debug, Clone, Copy)]
pub struct CapabilityTable {
    pub entries: [Option<Capability>; MAX_CAPS],
    pub next_id: u64,
}

impl CapabilityTable {
    pub const fn new() -> Self {
        Self {
            entries: [None; MAX_CAPS],
            next_id: 1,
        }
    }

    pub fn mint(&mut self, cap_type: CapabilityType, rights: Rights, target_id: Option<u32>) -> Result<CapabilityId, CapError> {
        for slot in self.entries.iter_mut() {
            if slot.is_none() {
                let id = CapabilityId(self.next_id);
                self.next_id += 1;
                *slot = Some(Capability {
                    id,
                    cap_type,
                    rights,
                    target_id,
                });
                return Ok(id);
            }
        }
        Err(CapError::TableFull)
    }

    pub fn check(&self, cap_type: CapabilityType, required_rights: Rights) -> Result<&Capability, CapError> {
        for slot in self.entries.iter() {
            if let Some(cap) = slot {
                if cap.cap_type == cap_type {
                    if cap.rights.contains(required_rights) {
                        return Ok(cap);
                    } else {
                        return Err(CapError::PermissionDenied);
                    }
                }
            }
        }
        Err(CapError::CapabilityNotFound)
    }

    pub fn check_targeted(&self, cap_type: CapabilityType, required_rights: Rights, target: u32) -> Result<&Capability, CapError> {
        for slot in self.entries.iter() {
            if let Some(cap) = slot {
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
        }
        Err(CapError::TargetMismatch)
    }
}
