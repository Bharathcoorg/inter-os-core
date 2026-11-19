// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

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
use crate::capability::{Capability, CapabilityId, CapabilityTable, CapabilityType, Rights};
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
    /// Permission to run the bounded Calculator agent tool.
    AgentToolCalculator = 16,
    /// Permission to run the bounded CalendarClock agent tool.
    AgentToolClock = 17,
    /// Permission to run the bounded StorageStats agent tool.
    AgentToolStorage = 18,
    /// Permission to run the bounded SettingsQuery agent tool.
    AgentToolSettings = 19,
    /// Permission to run the bounded CapabilityCheck agent tool.
    AgentToolCapCheck = 20,
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
            16 => Some(Self::AgentToolCalculator),
            17 => Some(Self::AgentToolClock),
            18 => Some(Self::AgentToolStorage),
            19 => Some(Self::AgentToolSettings),
            20 => Some(Self::AgentToolCapCheck),
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
            Self::AgentToolCalculator => "Agent Tool: Calculator",
            Self::AgentToolClock => "Agent Tool: Calendar & Clock",
            Self::AgentToolStorage => "Agent Tool: Storage Stats",
            Self::AgentToolSettings => "Agent Tool: Settings Query",
            Self::AgentToolCapCheck => "Agent Tool: Capability Check",
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
            Self::AgentToolCalculator => "Allows executing the bounded integer math calculator tool.",
            Self::AgentToolClock => "Allows querying real-time clock and calendar metrics via RTC.",
            Self::AgentToolStorage => "Allows querying storage volume capacity and usage stats.",
            Self::AgentToolSettings => "Allows querying safe non-sensitive configuration settings.",
            Self::AgentToolCapCheck => "Allows inspecting capability delegation states.",
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
                // WHY `READ | EXECUTE | INVOKE`: broad agent task domain
                // covering all 5 bounded tools (mask 0x1F).
                CapabilityType::AgentTask,
                Rights::READ.union(Rights::EXECUTE).union(Rights::INVOKE),
                0x1F,
            ),
            Self::AgentToolCalculator => (
                CapabilityType::AgentTask,
                Rights::EXECUTE.union(Rights::INVOKE),
                1 << 0,
            ),
            Self::AgentToolClock => (
                CapabilityType::AgentTask,
                Rights::READ.union(Rights::INVOKE),
                1 << 1,
            ),
            Self::AgentToolStorage => (
                CapabilityType::AgentTask,
                Rights::READ.union(Rights::INVOKE),
                1 << 2,
            ),
            Self::AgentToolSettings => (
                CapabilityType::AgentTask,
                Rights::READ.union(Rights::INVOKE),
                1 << 3,
            ),
            Self::AgentToolCapCheck => (
                CapabilityType::AgentTask,
                Rights::READ.union(Rights::INVOKE),
                1 << 4,
            ),
        };

        Capability {
            id: self.capability_id(task_id),
            cap_type,
            rights,
            object_id,
        }
    }

    /// The service-control object a manifest SystemControl grant for this
    /// resource is bound to (RAPP v2 ids 20-23), if any.
    ///
    /// WHY this exists: a trusted publisher's manifest grants `SystemControl`
    /// INVOKE on a service control object, while a consent grant mints a different
    /// capability shape (e.g. `HardwarePortIo` on a dedicated port) for the same
    /// resource. The two live in different id spaces -- the manifest grant is
    /// re-keyed into the per-launch instance space at spawn -- so revoking the
    /// consent identity alone cannot reach the manifest grant. This mapping lets
    /// the revocation boundary find the manifest grant by its resource object.
    ///
    /// RAPP v2 id 19 (POWER) has no consent permission, so it is not revocable
    /// through this boundary; it is revoked only when the task exits.
    pub fn manifest_control_object(&self) -> Option<u64> {
        match self {
            Self::TelephonyModem => Some(crate::services::SERVICE_TELEPHONY_CONTROL_OBJECT),
            Self::Microphone | Self::AudioSynthesis => Some(crate::services::SERVICE_AUDIO_CONTROL_OBJECT),
            Self::CameraCapture => Some(crate::services::SERVICE_CAMERA_CONTROL_OBJECT),
            Self::Sensors => Some(crate::services::SERVICE_SENSOR_CONTROL_OBJECT),
            _ => None,
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
    pub package_id: String,
    pub code_digest: [u8; 32],
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

/// Matches application names and common canonical aliases.
pub fn app_names_match(a: &str, b: &str) -> bool {
    if a == b {
        return true;
    }
    let a_norm = a.strip_prefix("org.interos.").unwrap_or(a);
    let b_norm = b.strip_prefix("org.interos.").unwrap_or(b);
    if a_norm == b_norm {
        return true;
    }
    let is_agent_a = a == "Agent Manager" || a == "agentmanager" || a == "app_agent_manager" || a == "org.interos.agentmanager";
    let is_agent_b = b == "Agent Manager" || b == "agentmanager" || b == "app_agent_manager" || b == "org.interos.agentmanager";
    is_agent_a && is_agent_b
}

impl PermissionManager {
    pub const fn new() -> Self {
        Self {
            records: Vec::new(),
            next_request_id: 1,
            active_prompt: None,
        }
    }

    /// Checks if a verified application identity already holds a granted permission.
    pub fn has_identity_permission(&self, identity: &crate::task::SecurityIdentity, permission: SensitivePermission) -> bool {
        self.records.iter().any(|r| {
            r.package_id == identity.package_id
                && r.code_digest == identity.code_digest
                && r.permission == permission
                && r.decision == ConsentDecision::Granted
        })
    }

    /// Registers a new permission request.
    pub fn request_consent(
        &mut self,
        task_id: TaskId,
        identity: Option<crate::task::SecurityIdentity>,
        app_name: &str,
        permission: SensitivePermission,
        auto_approve: bool,
        current_ticks: u64,
    ) -> (u64, ConsentDecision) {
        let _ = auto_approve;
        if identity.as_ref().is_some_and(|id| self.has_identity_permission(id, permission)) {
            return (0, ConsentDecision::Granted);
        }

        let pkg_id_str = identity.map(|id| id.package_id).unwrap_or(app_name);
        let code_digest = identity.map(|id| id.code_digest).unwrap_or([0u8; 32]);

        for record in self.records.iter_mut() {
            if record.task_id == task_id
                && record.package_id == pkg_id_str
                && record.code_digest == code_digest
                && record.permission == permission
                && record.decision == ConsentDecision::Pending
            {
                record.timestamp_ticks = current_ticks;
                let req_id = record.request_id;
                self.active_prompt = Some(record.clone());
                return (req_id, ConsentDecision::Pending);
            }
        }

        let request_id = self.next_request_id;
        self.next_request_id = self.next_request_id.saturating_add(1);

        let record = PermissionRecord {
            request_id,
            task_id,
            app_name: String::from(app_name),
            package_id: String::from(pkg_id_str),
            code_digest,
            permission,
            decision: ConsentDecision::Pending,
            timestamp_ticks: current_ticks,
        };

        self.active_prompt = Some(record.clone());
        self.records.push(record);
        (request_id, ConsentDecision::Pending)
    }

    pub(crate) fn pending_request(&self, request_id: u64) -> Option<&PermissionRecord> {
        self.records.iter().find(|record| {
            record.request_id == request_id && record.decision == ConsentDecision::Pending
        })
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
                // Replay prevention: only a request that was actually pending can be resolved.
                if record.decision != ConsentDecision::Pending {
                    return None;
                }
                record.decision = decision;
                if decision == ConsentDecision::Granted || decision == ConsentDecision::GrantedOnce {
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

    /// Dynamically revokes a granted permission from a task's consent records.
    ///
    /// WHY the revocation is published unconditionally, before the record scan is
    /// even reported: the consent record is only the policy half of the grant. The
    /// capability itself is a separate object that may sit in the task's table, in
    /// another table, or in an in-flight IPC message, so the record must not be
    /// able to decide whether the authority ends. Publishing it here also closes
    /// the case where the record was already marked revoked - by a one-time grant
    /// being consumed, for instance - while the capability was still installed.
    ///
    /// WHY this is the policy half only: the live-authority half has to read the
    /// target task's capability table, which lives under `SCHEDULER`. Taking that
    /// lock from here would nest `PERMISSION_MANAGER` -> `SCHEDULER`, the reverse
    /// of the order `sys_cap_request_consent` already uses. Callers that must end
    /// the live authority too use [`revoke_permission_live`], which owns the whole
    /// boundary in one lock order. The bit-62 identity published here still ends
    /// the consent-minted capability wherever a copy of it survives.
    pub fn revoke_permission(&mut self, task_id: TaskId, permission: SensitivePermission) -> bool {
        if task_id != TaskId(0) {
            crate::capability::revocation::revoke(permission.capability_id(task_id));
        }

        let mut revoked = false;
        for record in self.records.iter_mut() {
            if (task_id == TaskId(0) || record.task_id == task_id) && record.permission == permission {
                if record.decision == ConsentDecision::Granted || record.decision == ConsentDecision::GrantedOnce {
                    record.decision = ConsentDecision::Revoked;
                    crate::capability::revocation::revoke(record.permission.capability_id(record.task_id));
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

    /// Drops records belonging to an exiting task while preserving pending prompts
    /// and persistent application grants.
    pub fn cleanup_task(&mut self, task_id: TaskId) -> usize {
        let mut dropped = 0;
        self.records.retain(|record| {
            if record.task_id == task_id {
                if record.decision == ConsentDecision::GrantedOnce {
                    crate::capability::revocation::revoke(record.permission.capability_id(task_id));
                    dropped += 1;
                    false
                } else if record.decision == ConsentDecision::Pending || record.decision == ConsentDecision::Granted {
                    true
                } else {
                    dropped += 1;
                    false
                }
            } else {
                true
            }
        });
        if self
            .active_prompt
            .as_ref()
            .is_some_and(|prompt| prompt.task_id == task_id && prompt.decision == ConsentDecision::GrantedOnce)
        {
            self.active_prompt = None;
        }
        dropped
    }

    /// Formats active pending consent requests and/or records into `out`.
    /// If `filter_task` is Some(id), only records for that task are returned.
    /// Format per line: `request_id|task_id|app_name|perm_val|decision\n`
    pub fn format_records(&self, filter_task: Option<TaskId>, out: &mut [u8]) -> usize {
        let mut written = 0;
        for record in &self.records {
            if let Some(id) = filter_task {
                if record.task_id != id {
                    continue;
                }
            }
            let mut line_buf = [0u8; 160];
            let mut pos = 0;
            pos += write_u64_ascii(record.request_id, &mut line_buf[pos..]);
            if pos < line_buf.len() { line_buf[pos] = b'|'; pos += 1; }
            pos += write_u64_ascii(record.task_id.0 as u64, &mut line_buf[pos..]);
            if pos < line_buf.len() { line_buf[pos] = b'|'; pos += 1; }
            let name_bytes = record.app_name.as_bytes();
            let n = name_bytes.len().min(line_buf.len().saturating_sub(pos + 10));
            line_buf[pos..pos + n].copy_from_slice(&name_bytes[..n]);
            pos += n;
            if pos < line_buf.len() { line_buf[pos] = b'|'; pos += 1; }
            pos += write_u64_ascii(record.permission as u64, &mut line_buf[pos..]);
            if pos < line_buf.len() { line_buf[pos] = b'|'; pos += 1; }
            pos += write_u64_ascii(record.decision as u64, &mut line_buf[pos..]);
            if pos < line_buf.len() { line_buf[pos] = b'\n'; pos += 1; }

            if written + pos <= out.len() {
                out[written..written + pos].copy_from_slice(&line_buf[..pos]);
                written += pos;
            } else {
                break;
            }
        }
        written
    }
}

#[test]
fn consent_reuse_requires_the_same_verified_executable() {
    let mut pm = PermissionManager::new();
    let permission = SensitivePermission::AgentTask;
    let original = crate::task::SecurityIdentity {
        package_id: "org.interos.agentmanager",
        code_digest: [1; 32],
        is_system: false,
    };
    let replacement = crate::task::SecurityIdentity { code_digest: [2; 32], ..original };
    let (request, _) = pm.request_consent(TaskId(8900), Some(original), "Agent Manager", permission, false, 0);
    let (other_request, _) = pm.request_consent(TaskId(8901), Some(original), "Agent Manager", permission, false, 1);
    assert_ne!(request, other_request);
    assert_eq!(pm.pending_request(request).unwrap().task_id, TaskId(8900));
    pm.resolve_consent(request, ConsentDecision::Granted).unwrap();
    assert_eq!(pm.request_consent(TaskId(8902), Some(original), "Agent Manager", permission, false, 2).1, ConsentDecision::Granted);
    assert_eq!(pm.request_consent(TaskId(8903), Some(replacement), "Agent Manager", permission, false, 3).1, ConsentDecision::Pending);
    assert_eq!(pm.request_consent(TaskId(8904), None, "Agent Manager", permission, false, 4).1, ConsentDecision::Pending);
    assert_eq!(pm.request_consent(TaskId(8900), Some(replacement), "Agent Manager", permission, false, 5).1, ConsentDecision::Pending);
    let (once, _) = pm.request_consent(TaskId(8905), Some(replacement), "Agent Manager", permission, false, 6);
    pm.resolve_consent(once, ConsentDecision::GrantedOnce).unwrap();
    assert!(!pm.has_identity_permission(&replacement, permission));
}

fn write_u64_ascii(mut val: u64, buf: &mut [u8]) -> usize {
    if val == 0 {
        if !buf.is_empty() {
            buf[0] = b'0';
            return 1;
        }
        return 0;
    }
    let mut digits = [0u8; 20];
    let mut len = 0;
    while val > 0 && len < digits.len() {
        digits[len] = b'0' + (val % 10) as u8;
        val /= 10;
        len += 1;
    }
    let take = len.min(buf.len());
    for i in 0..take {
        buf[i] = digits[len - 1 - i];
    }
    take
}

/// Collects the live capability ids in `table` that grant `permission`'s
/// resource, in both the consent-minted shape and the manifest SystemControl
/// shape.
///
/// WHY both shapes: revoking a permission must end the authority regardless of
/// how it was granted. A consent grant carries the bit-62 permission identity; a
/// manifest grant carries a per-launch instance identity (the spawn funnel
/// re-keys `0x5000_0000 + slot`). Matching the resource shape -- the capability
/// type and object, not the id -- is what lets one revocation reach both.
///
/// WHY `Option`: a revocation that cannot finish collecting the ids it must
/// withdraw has not ended the authority. Reporting that as an empty list would
/// turn a heap failure into a silent skip, so the failure is returned to the
/// caller instead of being folded into "nothing matched".
fn live_resource_cap_ids(
    table: &CapabilityTable,
    task_id: TaskId,
    permission: SensitivePermission,
) -> Option<Vec<CapabilityId>> {
    let consent = permission.to_capability(task_id);
    let control_object = permission.manifest_control_object();
    let mut ids = Vec::new();
    for slot in 0..table.capacity() {
        let Some(cap) = table.raw_slot(slot) else { continue };
        let matches_consent = cap.cap_type == consent.cap_type && cap.object_id == consent.object_id;
        let matches_manifest = cap.cap_type == CapabilityType::SystemControl
            && control_object.is_some_and(|obj| obj == cap.object_id);
        if (matches_consent || matches_manifest) && !ids.contains(&cap.id) {
            ids.try_reserve(1).ok()?;
            ids.push(cap.id);
        }
    }
    Some(ids)
}

/// Outcome of the whole permission-revocation boundary: consent records plus the
/// live capabilities that actually carry the authority.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RevocationOutcome {
    /// Neither a granted consent record nor a live capability for this resource
    /// existed. Nothing was withdrawn because there was nothing to withdraw.
    NotFound,
    /// Every matching consent record was marked revoked and every matching live
    /// capability was cleared from its table and published as revoked.
    Revoked,
    /// The live-table scan could not be completed, so a manifest grant may still
    /// authorize. WHY this is distinct from `NotFound`: "nothing matched" and
    /// "we never got to look" are different facts, and only the first one may be
    /// reported to userspace as anything other than a failure.
    Incomplete,
}

/// Revokes a permission's consent records AND every live capability that grants
/// the same resource, in both the consent-minted and manifest SystemControl
/// shapes. This is the entry syscall 113 (PermissionRevoke) uses.
///
/// WHY the lock order is SCHEDULER then PERMISSION_MANAGER, and why the record
/// half runs in an inner scope: the kernel already acquires these two in that
/// order -- `sys_cap_request_consent` holds `SCHEDULER` across its
/// `PERMISSION_MANAGER` acquire, and the force-stop path holds a `SCHEDULER`
/// guard across `run_deferred_cleanup`, which reaches `permission::cleanup_task`.
/// The previous shape of this boundary did the opposite (PERMISSION_MANAGER held
/// across a SCHEDULER acquire), which closed a cycle with those callers. Dropping
/// the record guard before the live scan keeps every acquisition on this path in
/// the one established order.
///
/// WHY the scan uses a blocking lock and has no retry budget: `SCHEDULER` is the
/// first lock this boundary takes and the record guard is already gone by the
/// time the scan runs, so waiting for it cannot close a cycle. A bounded
/// `try_lock` spin here used to expire while the scheduler was busy and return
/// without touching the manifest grant -- while the caller still reported
/// success. There is no arm on this path that can skip the scan: it is reached
/// unconditionally after the record half, and the only way it reports less than
/// success is `Incomplete`, which the syscall turns into a failure code.
pub fn revoke_permission_live(
    task_id: TaskId,
    permission: SensitivePermission,
) -> RevocationOutcome {
    let mut sched = crate::task::SCHEDULER.lock();
    let records_revoked = {
        let mut pm = PERMISSION_MANAGER.lock();
        pm.revoke_permission(task_id, permission)
    };
    match revoke_live_resource_caps(&mut sched, task_id, permission) {
        Some(live) if records_revoked || live > 0 => RevocationOutcome::Revoked,
        Some(_) => RevocationOutcome::NotFound,
        None => {
            crate::serial_println!(
                "[PERMISSION] Revocation of {:?} for Task {} INCOMPLETE: live capability scan could not finish; authority may survive.",
                permission,
                task_id.0
            );
            RevocationOutcome::Incomplete
        }
    }
}

/// Clears and publishes every live capability in `sched` that grants
/// `permission`'s resource, for one task or for every task when `task_id` is
/// `TaskId(0)`. Returns the number of live capabilities withdrawn, or `None` if
/// the scan could not be completed.
///
/// WHY the caller passes its own guard instead of this function locking: the
/// caller already owns `SCHEDULER` for the whole boundary, and acquiring it here
/// would be a second, non-reentrant acquire of a lock this thread holds. Taking
/// the guard as an argument makes "the scan always runs" a property of the type
/// rather than of a retry budget.
///
/// WHY the task list is walked by index rather than collected: collecting it
/// allocates, and an allocation failure on the all-tasks path would otherwise
/// have to be reported as an incomplete revocation for a reason that has nothing
/// to do with the target tasks. Revocation never adds or removes a task, so the
/// indices stay valid for the whole walk.
pub fn revoke_live_resource_caps(
    sched: &mut crate::task::Scheduler,
    task_id: TaskId,
    permission: SensitivePermission,
) -> Option<usize> {
    let mut withdrawn = 0usize;
    if task_id == TaskId(0) {
        let count = sched.tasks_iter().count();
        for index in 0..count {
            let Some(tid) = sched.tasks_iter().nth(index).map(|t| t.id) else { continue };
            withdrawn += revoke_task_resource_caps(sched, tid, permission)?;
        }
    } else {
        withdrawn += revoke_task_resource_caps(sched, task_id, permission)?;
    }
    Some(withdrawn)
}

/// Withdraws one task's live caps for this resource. `None` propagates a scan
/// that could not finish; a task that is not in the table contributes nothing.
fn revoke_task_resource_caps(
    sched: &mut crate::task::Scheduler,
    task_id: TaskId,
    permission: SensitivePermission,
) -> Option<usize> {
    let Some(task) = sched.get_task(task_id) else { return Some(0) };
    let ids = live_resource_cap_ids(&task.cap_table, task_id, permission)?;
    let count = ids.len();
    let Some(task) = sched.get_task_mut(task_id) else { return Some(0) };
    for id in ids {
        // `revoke_by_id` clears every slot holding the id and publishes the
        // revocation, so a copy that escaped into another table or an in-flight
        // IPC message dies with it.
        task.cap_table.revoke_by_id(id);
    }
    Some(count)
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
        None,
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

    // Replay prevention: re-resolving an already-resolved request must be rejected.
    let replayed = pm.resolve_consent(req1_id, ConsentDecision::Granted);
    assert!(replayed.is_none(), "Consent replay must be rejected");

    // 3. Test interactive pending request
    let (req2_id, decision2) = pm.request_consent(
        TaskId(23),
        None,
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

    // 5. Test verified security identity and binary replacement defense
    let id_v1 = crate::task::SecurityIdentity {
        package_id: "org.interos.sample",
        code_digest: [0x11; 32],
        is_system: false,
    };
    let (req3_id, _) = pm.request_consent(
        TaskId(30),
        Some(id_v1),
        "Sample App",
        SensitivePermission::Files,
        false,
        110,
    );
    pm.resolve_consent(req3_id, ConsentDecision::Granted);
    assert!(pm.has_identity_permission(&id_v1, SensitivePermission::Files));

    // An attacker replacing the binary (different digest) must NOT inherit the grant:
    let id_replaced = crate::task::SecurityIdentity {
        package_id: "org.interos.sample",
        code_digest: [0x22; 32],
        is_system: false,
    };
    assert!(!pm.has_identity_permission(&id_replaced, SensitivePermission::Files), "Replaced binary must not inherit grant");

    // 6. Test capability minting & granular tool masks
    let cap = SensitivePermission::TelephonyModem.to_capability(TaskId(19));
    assert_eq!(cap.cap_type, CapabilityType::HardwarePortIo);
    assert!(cap.rights.contains(Rights::READ));
    assert!(cap.rights.contains(Rights::INVOKE));

    let calc_cap = SensitivePermission::AgentToolCalculator.to_capability(TaskId(19));
    assert_eq!(calc_cap.cap_type, CapabilityType::AgentTask);
    assert_eq!(calc_cap.object_id, 1 << 0);
    assert!(calc_cap.rights.contains(Rights::EXECUTE));

    let clock_cap = SensitivePermission::AgentToolClock.to_capability(TaskId(19));
    assert_eq!(clock_cap.cap_type, CapabilityType::AgentTask);
    assert_eq!(clock_cap.object_id, 1 << 1);
    assert!(clock_cap.rights.contains(Rights::READ));

    serial_println!("[TEST] Capability Consent Engine: Dynamic permission gating & verification PASSED.");
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capability::{CapabilityTable, CapabilityType, Rights};
    use crate::task::process::PROCESS_TEST_LOCK;

    extern "C" fn a504_dummy_entry() -> ! {
        loop { core::hint::spin_loop(); }
    }

    /// The resource-shape matcher must find a manifest SystemControl grant by its
    /// service control object even though its live id is a per-launch instance id,
    /// and must also find the consent-minted cap for the same resource.
    #[test]
    fn live_resource_matcher_reaches_manifest_and_consent_shapes() {
        let task = TaskId::new();
        // Manifest grants 20-23 (telephony/audio/camera/sensor SystemControl).
        let mut manifest = crate::task::user::capabilities_from_rapp_with_trust(
            &[20, 21, 22, 23], 0x1234, true,
        ).expect("trusted manifest grants");
        // The spawn funnel re-keys every manifest handle into the instance space.
        assert_eq!(manifest.rekey_all(), 4);

        // Camera: the manifest SystemControl cap (instance id) must be matched.
        let cam_ids = live_resource_cap_ids(&manifest, task, SensitivePermission::CameraCapture)
            .expect("matcher must complete");
        assert_eq!(cam_ids.len(), 1, "exactly the camera SystemControl grant matches");
        let cam_cap = manifest.raw_slot(2).expect("camera manifest slot");
        assert_eq!(cam_cap.cap_type, CapabilityType::SystemControl);
        assert_eq!(cam_cap.object_id, crate::services::SERVICE_CAMERA_CONTROL_OBJECT);
        assert!(cam_ids.contains(&cam_cap.id));
        // The instance id is not the consent id -- this is the whole defect.
        assert_ne!(cam_cap.id, SensitivePermission::CameraCapture.capability_id(task));

        // A consent-minted camera cap is matched too.
        let mut consent_table = CapabilityTable::new_for_task(task, 2);
        let consent_cap = SensitivePermission::CameraCapture.to_capability(task);
        let consent_id = consent_cap.id;
        consent_table.insert(consent_cap).expect("insert consent cap");
        let consent_ids = live_resource_cap_ids(&consent_table, task, SensitivePermission::CameraCapture)
            .expect("matcher must complete");
        assert!(consent_ids.contains(&consent_id));

        // An unrelated resource (power, manifest 19) is not matched by camera.
        let mut power = crate::task::user::capabilities_from_rapp_with_trust(&[19], 0, true)
            .expect("power grant");
        power.rekey_all();
        assert!(live_resource_cap_ids(&power, task, SensitivePermission::CameraCapture)
            .expect("matcher must complete")
            .is_empty());
    }

    /// A5-04 end to end, driven through the production revocation boundary that
    /// syscall 113 calls: grant manifest SystemControl caps to a live task, revoke
    /// the corresponding consent permissions, and prove the manifest grants no
    /// longer authorize AND are no longer installed. Before the fix the revocation
    /// published only the bit-62 consent id and the re-keyed manifest cap kept
    /// authorizing.
    #[test]
    fn revoking_a_permission_kills_manifest_system_control_grants() {
        let _lock = PROCESS_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let mut caps = crate::task::user::capabilities_from_rapp_with_trust(
            &[20, 21, 22, 23], 0x1234, true,
        ).expect("trusted manifest grants");
        assert_eq!(caps.rekey_all(), 4);
        let tid = {
            let mut sched = crate::task::SCHEDULER.lock();
            sched.spawn("a504.manifest.app", a504_dummy_entry, caps, 6)
        };
        assert_ne!(tid, TaskId(0), "spawn must register the task");
        let _leader = crate::task::process::OwnedLeader::adopted(tid);

        // (permission, service control object, manifest slot, live instance id)
        let cases = [
            (SensitivePermission::TelephonyModem, crate::services::SERVICE_TELEPHONY_CONTROL_OBJECT, 0usize),
            (SensitivePermission::Microphone, crate::services::SERVICE_AUDIO_CONTROL_OBJECT, 1),
            (SensitivePermission::CameraCapture, crate::services::SERVICE_CAMERA_CONTROL_OBJECT, 2),
            (SensitivePermission::Sensors, crate::services::SERVICE_SENSOR_CONTROL_OBJECT, 3),
        ];

        // Before: every manifest grant authorizes its control object, and its live
        // id is a per-launch instance id, never the bit-62 consent identity.
        let instance_ids: Vec<CapabilityId> = {
            let sched = crate::task::SCHEDULER.lock();
            let task = sched.get_task(tid).expect("task");
            for (perm, obj, slot) in cases {
                assert!(task.cap_table.authorize_object(CapabilityType::SystemControl, Rights::INVOKE, obj));
                let cap = task.cap_table.raw_slot(slot).expect("manifest slot occupied");
                assert_ne!(cap.id, perm.capability_id(tid), "manifest id must not be the consent id");
            }
            (0..4).map(|s| task.cap_table.raw_slot(s).expect("slot").id).collect()
        };

        // Revoke each consent permission through the entry syscall 113 uses. No
        // consent records exist; the manifest grant is the only authority, which is
        // exactly the A5-04 case, so a policy-only revocation would report NotFound.
        for (perm, _, _) in cases {
            assert_eq!(
                revoke_permission_live(tid, perm),
                RevocationOutcome::Revoked,
                "{:?}: the manifest grant is live authority, so the boundary must report it withdrawn",
                perm,
            );
        }

        // After: none of the manifest grants may authorize, none may remain
        // installed, and every instance id must be published as revoked.
        let sched = crate::task::SCHEDULER.lock();
        let task = sched.get_task(tid).expect("task");
        for (perm, obj, slot) in cases {
            assert!(
                !task.cap_table.authorize_object(CapabilityType::SystemControl, Rights::INVOKE, obj),
                "{:?}: manifest SystemControl grant must not survive revocation of the consent permission",
                perm,
            );
            assert!(
                task.cap_table.raw_slot(slot).is_none(),
                "{:?}: the manifest slot must be cleared, not merely published against",
                perm,
            );
            assert_eq!(
                task.cap_table.verify_object(slot, CapabilityType::SystemControl, Rights::INVOKE, obj),
                Err(crate::capability::CapError::InvalidSlot),
                "{:?}: the slot must no longer verify",
                perm,
            );
        }
        for id in instance_ids {
            assert!(
                crate::capability::revocation::is_revoked(id),
                "the live instance id {:?} must be published as revoked",
                id,
            );
        }
    }

    /// The record half on its own must not be mistaken for the whole boundary:
    /// with no consent record it reports nothing revoked, and the manifest grant
    /// it never looked at stays live. This pins WHY syscall 113 cannot use
    /// `revoke_permission` alone.
    #[test]
    fn record_only_revocation_does_not_reach_a_manifest_grant() {
        let _lock = PROCESS_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let mut caps = crate::task::user::capabilities_from_rapp_with_trust(&[22], 0, true)
            .expect("trusted camera manifest grant");
        assert_eq!(caps.rekey_all(), 1);
        let tid = {
            let mut sched = crate::task::SCHEDULER.lock();
            sched.spawn("a504.policy.only", a504_dummy_entry, caps, 6)
        };
        assert_ne!(tid, TaskId(0));
        let _leader = crate::task::process::OwnedLeader::adopted(tid);

        assert!(
            !PERMISSION_MANAGER.lock().revoke_permission(tid, SensitivePermission::CameraCapture),
            "no consent record exists, so the policy half has nothing to revoke",
        );
        {
            let sched = crate::task::SCHEDULER.lock();
            let task = sched.get_task(tid).expect("task");
            assert!(
                task.cap_table.authorize_object(
                    CapabilityType::SystemControl, Rights::INVOKE,
                    crate::services::SERVICE_CAMERA_CONTROL_OBJECT,
                ),
                "the policy half never reads the live table, so the manifest grant is still there",
            );
        }

        // The full boundary reaches it.
        assert_eq!(
            revoke_permission_live(tid, SensitivePermission::CameraCapture),
            RevocationOutcome::Revoked,
        );
        let sched = crate::task::SCHEDULER.lock();
        let task = sched.get_task(tid).expect("task");
        assert!(!task.cap_table.authorize_object(
            CapabilityType::SystemControl, Rights::INVOKE,
            crate::services::SERVICE_CAMERA_CONTROL_OBJECT,
        ));
    }

    /// Positive control: the consent-minted shape is still revoked, and a task
    /// that never held the resource is reported as NotFound rather than Success.
    #[test]
    fn consent_shape_revocation_and_absent_resource_are_both_honest() {
        let _lock = PROCESS_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let tid = {
            let mut sched = crate::task::SCHEDULER.lock();
            sched.spawn("a504.consent.app", a504_dummy_entry, CapabilityTable::new_for_task(TaskId(0), 4), 6)
        };
        assert_ne!(tid, TaskId(0));
        let _leader = crate::task::process::OwnedLeader::adopted(tid);

        let consent = SensitivePermission::Microphone.to_capability(tid);
        let consent_id = consent.id;
        let slot = {
            let mut sched = crate::task::SCHEDULER.lock();
            let task = sched.get_task_mut(tid).expect("task");
            task.cap_table.insert(consent).expect("insert consent cap")
        };
        {
            let mut pm = PERMISSION_MANAGER.lock();
            let (req, _) = pm.request_consent(tid, None, "a504.consent.app", SensitivePermission::Microphone, false, 0);
            assert!(pm.resolve_consent(req, ConsentDecision::Granted).is_some());
        }

        // A resource this task never held: nothing to withdraw, so not Success.
        assert_eq!(
            revoke_permission_live(tid, SensitivePermission::CameraCapture),
            RevocationOutcome::NotFound,
            "a task without the resource must not be reported as revoked",
        );
        {
            let sched = crate::task::SCHEDULER.lock();
            let task = sched.get_task(tid).expect("task");
            assert!(task.cap_table.raw_slot(slot).is_some(), "an unrelated revoke must not touch the microphone cap");
        }

        assert_eq!(
            revoke_permission_live(tid, SensitivePermission::Microphone),
            RevocationOutcome::Revoked,
        );
        let sched = crate::task::SCHEDULER.lock();
        let task = sched.get_task(tid).expect("task");
        assert!(task.cap_table.raw_slot(slot).is_none(), "the consent slot must be cleared");
        assert!(crate::capability::revocation::is_revoked(consent_id));
        assert!(!PERMISSION_MANAGER.lock().has_permission(tid, SensitivePermission::Microphone));
    }

    /// `TaskId(0)` must reach every live task, in both shapes, and must leave a
    /// task holding a different resource alone.
    #[test]
    fn all_tasks_revocation_reaches_every_live_table() {
        let _lock = PROCESS_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let spawn_with = |name: &'static str, ids: &[u16]| -> TaskId {
            let mut caps = crate::task::user::capabilities_from_rapp_with_trust(ids, 0, true)
                .expect("trusted manifest grants");
            caps.rekey_all();
            let tid = {
                let mut sched = crate::task::SCHEDULER.lock();
                sched.spawn(name, a504_dummy_entry, caps, 6)
            };
            assert_ne!(tid, TaskId(0), "spawn must register the task");
            tid
        };
        let manifest_task = spawn_with("a504.all.manifest", &[22]);
        let consent_task = spawn_with("a504.all.consent", &[]);
        let power_task = spawn_with("a504.all.power", &[19]);
        let _l1 = crate::task::process::OwnedLeader::adopted(manifest_task);
        let _l2 = crate::task::process::OwnedLeader::adopted(consent_task);
        let _l3 = crate::task::process::OwnedLeader::adopted(power_task);

        let consent_slot = {
            let mut sched = crate::task::SCHEDULER.lock();
            let task = sched.get_task_mut(consent_task).expect("task");
            task.cap_table.insert(SensitivePermission::CameraCapture.to_capability(consent_task))
                .expect("insert consent cap")
        };
        {
            let sched = crate::task::SCHEDULER.lock();
            let manifest = sched.get_task(manifest_task).expect("task");
            assert!(manifest.cap_table.authorize_object(
                CapabilityType::SystemControl, Rights::INVOKE,
                crate::services::SERVICE_CAMERA_CONTROL_OBJECT,
            ), "the manifest camera grant must authorize before the revoke");
            let consent = sched.get_task(consent_task).expect("task");
            assert!(consent.cap_table.verify_object(
                consent_slot, CapabilityType::HardwarePortIo, Rights::READ, 0xCA00,
            ).is_ok(), "the consent camera grant must verify before the revoke");
            let power = sched.get_task(power_task).expect("task");
            assert!(power.cap_table.authorize_object(
                CapabilityType::SystemControl, Rights::INVOKE,
                crate::services::SERVICE_POWER_CONTROL_OBJECT,
            ));
        }

        assert_eq!(
            revoke_permission_live(TaskId(0), SensitivePermission::CameraCapture),
            RevocationOutcome::Revoked,
        );

        let sched = crate::task::SCHEDULER.lock();
        let manifest = sched.get_task(manifest_task).expect("task");
        assert!(manifest.cap_table.raw_slot(0).is_none(), "manifest camera grant must be gone");
        assert!(!manifest.cap_table.authorize_object(
            CapabilityType::SystemControl, Rights::INVOKE,
            crate::services::SERVICE_CAMERA_CONTROL_OBJECT,
        ));
        let consent = sched.get_task(consent_task).expect("task");
        assert!(consent.cap_table.raw_slot(consent_slot).is_none(), "consent camera grant must be gone");
        let power = sched.get_task(power_task).expect("task");
        assert!(
            power.cap_table.authorize_object(
                CapabilityType::SystemControl, Rights::INVOKE,
                crate::services::SERVICE_POWER_CONTROL_OBJECT,
            ),
            "a camera revoke must not strip an unrelated power grant",
        );
    }

    /// The fail-open is gone: when SCHEDULER is held by another context the
    /// boundary WAITS for it and then completes the live scan. The old shape spun
    /// `try_lock` 10_000 times, expired in microseconds, returned without touching
    /// the manifest grant, and let the caller report success.
    #[test]
    fn a_busy_scheduler_makes_revocation_wait_not_skip_the_live_scan() {
        let _lock = PROCESS_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let mut caps = crate::task::user::capabilities_from_rapp_with_trust(&[22], 0, true)
            .expect("trusted camera manifest grant");
        assert_eq!(caps.rekey_all(), 1);
        let tid = {
            let mut sched = crate::task::SCHEDULER.lock();
            sched.spawn("a504.contended.app", a504_dummy_entry, caps, 6)
        };
        assert_ne!(tid, TaskId(0));
        let _leader = crate::task::process::OwnedLeader::adopted(tid);

        let (held_tx, held_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel::<()>();
        let holder = std::thread::spawn(move || {
            let guard = crate::task::SCHEDULER.lock();
            held_tx.send(()).expect("signal held");
            release_rx.recv().expect("release signal");
            drop(guard);
        });
        held_rx.recv().expect("holder must own SCHEDULER");

        let worker = std::thread::spawn(move || {
            revoke_permission_live(tid, SensitivePermission::CameraCapture)
        });
        // Far longer than the old 10_000-iteration try-spin could ever survive.
        std::thread::sleep(std::time::Duration::from_millis(150));
        assert!(
            !worker.is_finished(),
            "the boundary must still be waiting for SCHEDULER; finishing here means it skipped the live scan",
        );
        release_tx.send(()).expect("release holder");
        let outcome = worker.join().expect("revocation worker");
        holder.join().expect("holder thread");

        assert_eq!(
            outcome,
            RevocationOutcome::Revoked,
            "a contended scheduler must not turn a live manifest grant into a reported success-without-revocation",
        );
        let sched = crate::task::SCHEDULER.lock();
        let task = sched.get_task(tid).expect("task");
        assert!(task.cap_table.raw_slot(0).is_none(), "the manifest slot must actually be cleared");
        assert!(!task.cap_table.authorize_object(
            CapabilityType::SystemControl, Rights::INVOKE,
            crate::services::SERVICE_CAMERA_CONTROL_OBJECT,
        ));
    }
}
