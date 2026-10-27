//! Mobile Application Permission Manager & Interactive Consent Gating Subsystem
//!
//! Enforces zero ambient authority for sensitive mobile capabilities:
//! Unprivileged Ring 3 applications must explicitly request capability delegation.
//! The kernel tracks permission requests, presents interactive consent dialogs in
//! the Mobile Shell, and dynamically mints capability tokens into the calling task's
//! CapabilityTable upon approval.

use alloc::string::String;
use alloc::vec::Vec;
use spin::Mutex;
use crate::capability::{Capability, CapabilityId, CapabilityType, Rights};
use crate::task::TaskId;
use crate::serial_println;

/// Bit 62 tag for the permission-minted capability id space.
///
/// WHY a dedicated constant: `delegation::lease_capability_id` tags delegated
/// leases with bit 63, and `CapabilityTable::verify_object` treats any capability
/// id with bit 63 set as a lease. Permission ids previously used a `0x9000_...`
/// base that also set bit 63, so the two spaces overlapped and every
/// permission-minted capability was verified as a nonexistent lease, failing with
/// `InvalidSlot`. Keeping the tag in one named place is what stops the two spaces
/// from silently overlapping again.
///
/// Bit 62 is used because nothing else claims it: lease ids use bit 63, user slot
/// handles are `0x5000_0000 + slot`, and the boot self-test ids (`0x00C0_FFEE_*`,
/// `0x5A11_*`) live in the low 32 bits.
pub const PERMISSION_ID_BASE: u64 = 0x4000_0000_0000_0000;

/// Sensitive hardware and system resource categories requiring explicit consent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum SensitivePermission {
    /// Permission to access 4G/5G cellular baseband modem, dial calls, and transmit SMS.
    TelephonyModem = 1,
    /// Permission to encrypt, decrypt, and manage sectors in the Sovereign Encrypted Vault.
    StorageSectorCrypt = 2,
    /// Permission to create zero-trust network sockets and transmit packets across the mesh.
    NetworkSend = 3,
    /// Permission to access MIPI-CSI2 camera hardware and capture live video/stills.
    CameraCapture = 4,
    /// Permission to generate audio tones and synthesize acoustic melodies via hardware timers.
    AudioSynthesis = 5,
    /// Permission to read real hardware RTC civil wall-clock time and calendar metrics.
    RtcClock = 6,
    /// Permission to capture live microphone audio.
    Microphone = 7,
    /// Permission to query precise GNSS / GPS geographical location.
    Location = 8,
    /// Permission to read and write user contacts and address book.
    Contacts = 9,
    /// Permission to access user files and private document storage.
    Files = 10,
    /// Permission to send and receive cellular SMS messages.
    Sms = 11,
    /// Permission to poll hardware motion, environmental, and biometric sensors.
    Sensors = 12,
    /// Permission to communicate over Bluetooth and BLE controllers.
    Bluetooth = 13,
    /// Permission to post system notifications and badges.
    Notifications = 14,
    /// Permission to run bounded agent tool executions on the caller's behalf.
    ///
    /// WHY this is its own permission and not the agent service's own
    /// capability: the agent service (service 14) holds `AgentTask` so it can
    /// do agent work of its own. An app that wants to run a tool under a
    /// person's consent needs the same capability type, but it has to be
    /// minted for that app by the consent broker rather than handed to it at
    /// spawn time, or every app would hold tool authority it never asked for.
    AgentTask = 15,
}

impl SensitivePermission {
    /// The identity a grant of this permission mints for `task`.
    ///
    /// WHY this is a function of the module rather than an expression repeated at
    /// each use: the identifier is what a revocation is published against, so a
    /// second, hand-written copy of this expression is a way to revoke the wrong
    /// capability while believing it succeeded.
    pub fn capability_id(&self, task_id: TaskId) -> CapabilityId {
        // WHY bit 62 and not bit 63: `CapabilityTable::verify_object` identifies a
        // delegated lease by testing `cap.id.0 & 0x8000_0000_0000_0000`, because
        // `delegation::lease_capability_id` tags leases with bit 63. This id space
        // originally used the `0x9000_...` base, which also sets bit 63, so every
        // permission-minted capability was misread as a lease: verification called
        // `consume_invocation` for a lease that never existed and failed with
        // `InvalidSlot`. The kernel panicked in its own boot self-test on this.
        //
        // The two id spaces must stay disjoint, so permission ids take bit 62,
        // which no other producer uses (leases use bit 63; user slot handles and
        // the self-test ids live in the low 32 bits).
        CapabilityId(PERMISSION_ID_BASE | ((task_id.0 as u64) << 16) | (*self as u64))
    }

    /// Parses a raw u32 syscall argument into a `SensitivePermission`.
    pub fn from_u32(val: u32) -> Option<Self> {
        match val {
            1 => Some(Self::TelephonyModem),
            2 => Some(Self::StorageSectorCrypt),
            3 => Some(Self::NetworkSend),
            4 => Some(Self::CameraCapture),
            5 => Some(Self::AudioSynthesis),
            6 => Some(Self::RtcClock),
            7 => Some(Self::Microphone),
            8 => Some(Self::Location),
            9 => Some(Self::Contacts),
            10 => Some(Self::Files),
            11 => Some(Self::Sms),
            12 => Some(Self::Sensors),
            13 => Some(Self::Bluetooth),
            14 => Some(Self::Notifications),
            15 => Some(Self::AgentTask),
            _ => None,
        }
    }

    /// Returns human-readable permission name for UI prompt.
    pub const fn name(&self) -> &'static str {
        match self {
            Self::TelephonyModem => "Cellular Modem & Telephony",
            Self::StorageSectorCrypt => "Encrypted Storage Vault",
            Self::NetworkSend => "Network Sockets & Data Transmission",
            Self::CameraCapture => "Camera Sensor & Computer Vision",
            Self::AudioSynthesis => "Hardware Audio Synthesizer",
            Self::RtcClock => "Hardware Real-Time Clock",
            Self::Microphone => "Microphone & Audio Recording",
            Self::Location => "Precise GNSS Location",
            Self::Contacts => "Contacts & Address Book",
            Self::Files => "User Storage & Files",
            Self::Sms => "Cellular SMS Messaging",
            Self::Sensors => "Sensors & Biometrics",
            Self::Bluetooth => "Bluetooth & BLE Controller",
            Self::Notifications => "System Notifications",
            Self::AgentTask => "Agent Tool Execution",
        }
    }

    /// Returns human-readable description of the sensitive resource.
    pub const fn description(&self) -> &'static str {
        match self {
            Self::TelephonyModem => "Allows placing cellular phone calls and raw modem AT commands.",
            Self::StorageSectorCrypt => "Allows reading and writing encrypted sectors in Sovereign Vault.",
            Self::NetworkSend => "Allows transmitting network datagrams and establishing P2P sockets.",
            Self::CameraCapture => "Allows capturing live video frames from the camera sensor.",
            Self::AudioSynthesis => "Allows synthesizing audio frequencies on the hardware speaker.",
            Self::RtcClock => "Allows reading real hardware calendar time and wall-clock RTC.",
            Self::Microphone => "Allows capturing audio from the built-in microphone.",
            Self::Location => "Allows querying precise GPS/GNSS device coordinates.",
            Self::Contacts => "Allows reading and modifying stored contact information.",
            Self::Files => "Allows reading and writing files in user storage.",
            Self::Sms => "Allows composing and sending cellular text messages.",
            Self::Sensors => "Allows reading accelerometer, gyroscope, and biometric sensors.",
            Self::Bluetooth => "Allows pairing and transferring data over Bluetooth/BLE.",
            Self::Notifications => "Allows dispatching status and alert notifications to the user.",
            Self::AgentTask => "Allows running bounded agent tools that ask the platform for hardware, storage and service access.",
        }
    }

    /// Converts this permission into a concrete microkernel capability token.
    pub fn to_capability(&self, task_id: TaskId) -> Capability {
        let (cap_type, rights, object_id) = match self {
            Self::TelephonyModem => (
                CapabilityType::HardwarePortIo,
                Rights::READ.union(Rights::INVOKE),
                0x3F8, // Modem COM port / baseband object
            ),
            Self::StorageSectorCrypt => (
                CapabilityType::StorageRawBlock,
                Rights::READ.union(Rights::WRITE).union(Rights::INVOKE),
                task_id.0 as u64, // Vault sector partition ID
            ),
            Self::NetworkSend => (
                CapabilityType::NetworkSend,
                Rights::WRITE.union(Rights::INVOKE),
                0, // Network adapter object
            ),
            Self::CameraCapture => (
                CapabilityType::HardwarePortIo,
                Rights::READ.union(Rights::INVOKE),
                0xCA00, // Dedicated camera hardware object
            ),
            Self::AudioSynthesis => (
                CapabilityType::HardwarePortIo,
                Rights::WRITE.union(Rights::INVOKE),
                0x61, // PIT Channel 2 audio port
            ),
            Self::RtcClock => (
                CapabilityType::TimerTickRead,
                Rights::READ.union(Rights::INVOKE),
                0x70, // CMOS RTC port
            ),
            Self::Microphone => (
                CapabilityType::HardwarePortIo,
                Rights::READ.union(Rights::INVOKE),
                0xCA01, // Microphone audio input object
            ),
            Self::Location => (
                CapabilityType::HardwarePortIo,
                Rights::READ.union(Rights::INVOKE),
                0xCA02, // GNSS location controller object
            ),
            Self::Contacts => (
                CapabilityType::FileSystemRead,
                Rights::READ.union(Rights::WRITE),
                0xC001, // Contacts storage partition
            ),
            Self::Files => (
                CapabilityType::FileSystemRead,
                Rights::READ.union(Rights::WRITE),
                0xF11E, // User files storage partition
            ),
            Self::Sms => (
                CapabilityType::HardwarePortIo,
                Rights::WRITE.union(Rights::INVOKE),
                0x3F9, // Cellular SMS modem interface
            ),
            Self::Sensors => (
                CapabilityType::HardwarePortIo,
                Rights::READ.union(Rights::INVOKE),
                0x5E45, // Sensor bus controller object
            ),
            Self::Bluetooth => (
                CapabilityType::HardwarePortIo,
                Rights::READ.union(Rights::WRITE).union(Rights::INVOKE),
                0xB700, // Bluetooth HCI controller object
            ),
            Self::Notifications => (
                CapabilityType::IpcSend,
                Rights::WRITE.union(Rights::INVOKE),
                0x9071, // Notification manager IPC object
            ),
            Self::AgentTask => (
                // WHY `INVOKE` and nothing else: this grant says "this app may
                // ask the platform to run a bounded tool". It is not authority
                // over the clock, the disk or the services; every one of those
                // is a separate capability the kernel checks on its own, and
                // the tool's own syscall still refuses without it.
                CapabilityType::AgentTask,
                Rights::INVOKE,
                0, // The agent task domain
            ),
        };

        Capability {
            id: self.capability_id(task_id),
            cap_type,
            rights,
            object_id,
        }
    }
}

/// Scope of a granted permission.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PermissionScope {
    OneTime,
    WhileInUse,
    Always,
}

/// Status of user consent for a permission request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum ConsentDecision {
    Pending = 0,
    Granted = 1,
    Denied = 2,
    GrantedOnce = 3,
    Revoked = 4,
}

/// Audit record for a requested capability.
#[derive(Debug, Clone)]
pub struct PermissionRecord {
    pub request_id: u64,
    pub task_id: TaskId,
    pub app_name: String,
    pub permission: SensitivePermission,
    pub decision: ConsentDecision,
    pub timestamp_ticks: u64,
}

/// Mobile Application Permission Manager state engine.
pub struct PermissionManager {
    pub records: Vec<PermissionRecord>,
    pub next_request_id: u64,
    pub active_prompt: Option<PermissionRecord>,
}

impl PermissionManager {
    pub const fn new() -> Self {
        Self {
            records: Vec::new(),
            next_request_id: 1,
            active_prompt: None,
        }
    }

    /// Registers a new permission request.
    pub fn request_consent(
        &mut self,
        task_id: TaskId,
        app_name: &str,
        permission: SensitivePermission,
        auto_approve: bool,
        current_ticks: u64,
    ) -> (u64, ConsentDecision) {
        let request_id = self.next_request_id;
        self.next_request_id = self.next_request_id.saturating_add(1);

        // The syscall caller is never trusted to self-approve a new sensitive
        // permission. An existing granted record is the only non-interactive path.
        // auto_approve is retained for ABI compatibility and test callers, but
        // cannot bypass the permission state.
        let decision = if self.has_permission(task_id, permission) {
            ConsentDecision::Granted
        } else {
            let _ = auto_approve;
            ConsentDecision::Pending
        };

        let record = PermissionRecord {
            request_id,
            task_id,
            app_name: String::from(app_name),
            permission,
            decision,
            timestamp_ticks: current_ticks,
        };

        if decision == ConsentDecision::Pending {
            self.active_prompt = Some(record.clone());
        }

        self.records.push(record);
        (request_id, decision)
    }

    /// Resolves an active permission prompt with the user's interactive decision.
    pub fn resolve_consent(
        &mut self,
        request_id: u64,
        decision: ConsentDecision,
    ) -> Option<PermissionRecord> {
        if let Some(ref prompt) = self.active_prompt {
            if prompt.request_id == request_id {
                self.active_prompt = None;
            }
        }

        for record in self.records.iter_mut() {
            if record.request_id == request_id {
                // WHY only a request that was actually outstanding can lift a
                // revocation: otherwise an old, already-resolved request id could
                // be replayed to un-revoke a capability that was taken back
                // afterwards.
                let was_outstanding = record.decision == ConsentDecision::Pending;
                record.decision = decision;
                if was_outstanding && decision == ConsentDecision::Granted {
                    // A fresh consent resolution is the one event that
                    // re-establishes authority for this capability identity.
                    crate::capability::revocation::reinstate(
                        record.permission.capability_id(record.task_id),
                    );
                }
                return Some(record.clone());
            }
        }
        None
    }

    /// Checks if a task already holds a granted permission.
    pub fn has_permission(&self, task_id: TaskId, permission: SensitivePermission) -> bool {
        self.records.iter().any(|r| {
            r.task_id == task_id
                && r.permission == permission
                && (r.decision == ConsentDecision::Granted || r.decision == ConsentDecision::GrantedOnce)
        })
    }

    /// Dynamically revokes a granted permission from a task.
    ///
    /// WHY the revocation is published unconditionally, before the record scan is
    /// even reported: the consent record is only the policy half of the grant. The
    /// capability itself is a separate object that may sit in the task's table, in
    /// another table, or in an in-flight IPC message, so the record must not be
    /// able to decide whether the authority ends. Publishing it here also closes
    /// the case where the record was already marked revoked - by a one-time grant
    /// being consumed, for instance - while the capability was still installed.
    pub fn revoke_permission(&mut self, task_id: TaskId, permission: SensitivePermission) -> bool {
        crate::capability::revocation::revoke(permission.capability_id(task_id));

        let mut revoked = false;
        for record in self.records.iter_mut() {
            if record.task_id == task_id && record.permission == permission {
                if record.decision == ConsentDecision::Granted || record.decision == ConsentDecision::GrantedOnce {
                    record.decision = ConsentDecision::Revoked;
                    revoked = true;
                }
            }
        }
        if revoked {
            crate::serial_println!(
                "[PERMISSION] Revoked permission {:?} from Task {}",
                permission,
                task_id.0
            );
        }
        revoked
    }

    /// Consumes a one-time permission grant after single use.
    ///
    /// WHY this also publishes a revocation: consuming the consent record is not
    /// the same as ending the authority. Without this the minted capability stayed
    /// installed and usable for as long as the task lived, which is exactly the
    /// single-use promise the grant was made under.
    pub fn consume_one_time_permission(&mut self, task_id: TaskId, permission: SensitivePermission) -> bool {
        for record in self.records.iter_mut() {
            if record.task_id == task_id && record.permission == permission && record.decision == ConsentDecision::GrantedOnce {
                record.decision = ConsentDecision::Revoked;
                crate::capability::revocation::revoke(permission.capability_id(task_id));
                crate::serial_println!(
                    "[PERMISSION] Consumed one-time permission {:?} for Task {}",
                    permission,
                    task_id.0
                );
                return true;
            }
        }
        false
    }

    /// Drops every permission record belonging to an exiting task.
    ///
    /// WHY this matters for authority and not just tidiness: the records are the
    /// input to `has_permission`, so leaving them behind would let a later request
    /// from a re-used identity be answered from a grant made to a dead task.
    /// Returns the number of records dropped.
    pub fn cleanup_task(&mut self, task_id: TaskId) -> usize {
        let before = self.records.len();
        self.records.retain(|record| record.task_id != task_id);
        let dropped = before - self.records.len();
        if self
            .active_prompt
            .as_ref()
            .is_some_and(|prompt| prompt.task_id == task_id)
        {
            self.active_prompt = None;
        }
        dropped
    }
}

pub static PERMISSION_MANAGER: Mutex<PermissionManager> = Mutex::new(PermissionManager::new());

/// Drops every permission record belonging to an exiting task. Returns the
/// number of records dropped.
pub fn cleanup_task(task: TaskId) -> usize {
    PERMISSION_MANAGER.lock().cleanup_task(task)
}

/// Initializes the permission subsystem and executes automated self-tests.
pub fn init() {
    let mut pm = PERMISSION_MANAGER.lock();

    // 1. A caller cannot self-approve a new sensitive permission.
    let (req1_id, decision1) = pm.request_consent(
        TaskId(21),
        "interos.camera",
        SensitivePermission::CameraCapture,
        true,
        100,
    );
    assert_eq!(decision1, ConsentDecision::Pending);
    assert!(!pm.has_permission(TaskId(21), SensitivePermission::CameraCapture));

    // 2. Only an explicit consent resolution creates the grant.
    let resolved1 = pm.resolve_consent(req1_id, ConsentDecision::Granted);
    assert!(resolved1.is_some());
    assert!(pm.has_permission(TaskId(21), SensitivePermission::CameraCapture));

    // 3. Test interactive pending request
    let (req2_id, decision2) = pm.request_consent(
        TaskId(23),
        "interos.browser",
        SensitivePermission::NetworkSend,
        false,
        105,
    );
    assert_eq!(decision2, ConsentDecision::Pending);
    assert!(pm.active_prompt.is_some());
    assert_eq!(pm.active_prompt.as_ref().unwrap().request_id, req2_id);

    // 4. Resolve interactive pending request
    let resolved = pm.resolve_consent(req2_id, ConsentDecision::Granted);
    assert!(resolved.is_some());
    assert_eq!(resolved.unwrap().decision, ConsentDecision::Granted);
    assert!(pm.active_prompt.is_none());
    assert!(pm.has_permission(TaskId(23), SensitivePermission::NetworkSend));

    // 5. Test capability minting
    let cap = SensitivePermission::TelephonyModem.to_capability(TaskId(19));
    assert_eq!(cap.cap_type, CapabilityType::HardwarePortIo);
    assert!(cap.rights.contains(Rights::READ));
    assert!(cap.rights.contains(Rights::INVOKE));

    serial_println!("[TEST] Capability Consent Engine: Dynamic permission gating & verification PASSED.");
}
