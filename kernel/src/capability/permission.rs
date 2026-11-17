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
                && (r.decision == ConsentDecision::Granted || r.decision == ConsentDecision::GrantedOnce)
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
        if self.has_permission(task_id, permission)
            || identity.as_ref().is_some_and(|id| self.has_identity_permission(id, permission))
            || self.has_app_permission(app_name, permission)
        {
            return (0, ConsentDecision::Granted);
        }

        let pkg_id_str = identity.map(|id| id.package_id).unwrap_or(app_name);
        let code_digest = identity.map(|id| id.code_digest).unwrap_or([0u8; 32]);

        for record in self.records.iter_mut() {
            let identity_matches = if let Some(ref id) = identity {
                record.package_id == id.package_id && record.code_digest == id.code_digest
            } else {
                app_names_match(&record.app_name, app_name)
            };
            if identity_matches
                && record.permission == permission
                && record.decision == ConsentDecision::Pending
            {
                record.task_id = task_id;
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

    /// Checks if an application identity already holds a granted permission.
    pub fn has_app_permission(&self, app_name: &str, permission: SensitivePermission) -> bool {
        self.records.iter().any(|r| {
            app_names_match(&r.app_name, app_name)
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
