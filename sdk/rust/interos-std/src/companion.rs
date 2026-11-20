// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

//! Native InterOS Companion State, Memory, and Interactive Plan Management.
//!
//! Provides structured personal context, transparent user control, encrypted
//! persistence at rest, lock-state data sanitization, privacy modes, and
//! interactive execution plan state machines.

use crate::appmcp::{ToolRiskClass, ToolConsentClass};
use crate::crypto::chacha20_poly1305::ChaCha20Poly1305;
use crate::crypto::hkdf;

pub const COMPANION_MAGIC: u32 = 0x434F_4D50; // "COMP"
pub const COMPANION_VERSION: u32 = 1;
pub const COMPANION_FORMAT_MAGIC: [u8; 4] = *b"IOCM"; // "InterOS Companion Memory"
pub const COMPANION_FORMAT_VERSION: u16 = 1;
pub const COMPANION_HEADER_SIZE: usize = 64;
pub const CIPHER_CHACHA20_POLY1305: u32 = 0x0001;

pub const PATH_COMPANION_MEMORY: &str = "/data/companion/memory.bin";
pub const PATH_MODEL_DEFAULTS: &str = "/data/ai/model_defaults.bin";

/// Bounded storage budgets (in bytes)
pub const COMPANION_STORAGE_BUDGET_MAX: u32 = 64 * 1024; // 64 KiB
pub const SEMANTIC_INDEX_BUDGET_MAX: u32 = 512 * 1024;   // 512 KiB

/// Domain salt for software-backed companion key derivation.
pub const COMPANION_KEY_SALT: [u8; 16] = [
    0x49, 0x4F, 0x43, 0x4D, 0x2D, 0x55, 0x53, 0x45, // "IOCM-USE"
    0x52, 0x2D, 0x4B, 0x45, 0x59, 0x2D, 0x53, 0x31, // "R-KEY-S1"
];

/// Volatile zeroization helper to prevent compiler dead-code elimination.
#[inline(never)]
pub fn secure_zero(slice: &mut [u8]) {
    for byte in slice.iter_mut() {
        unsafe {
            core::ptr::write_volatile(byte, 0);
        }
    }
    core::sync::atomic::compiler_fence(core::sync::atomic::Ordering::SeqCst);
}

/// Derives a 32-byte cryptographic key bound to the authenticated user ID.
///
/// SECURITY & PLATFORM TRUTH:
/// In QEMU environments without hardware-backed root of trust, this is a
/// software-backed development key provider using RFC 5869 HKDF-SHA256.
/// It is isolated inside the companion persistence abstraction.
/// The model, aid, Agent Manager, and third-party apps NEVER receive this key.
pub fn derive_companion_user_key(uid: u32, salt: &[u8; 16]) -> [u8; 32] {
    let mut ikm = [0u8; 40];
    ikm[0..4].copy_from_slice(&uid.to_le_bytes());
    ikm[4..40].copy_from_slice(b"InterOS-Companion-Memory-Context-v1!");
    let prk = hkdf::hkdf_extract(salt, &ikm);
    let mut okm = [0u8; 32];
    let _ = hkdf::hkdf_expand(&prk, b"interos.companion.storage.key", &mut okm);
    okm
}

/// Monotonic Nonce Generator ensuring unique ChaCha20-Poly1305 nonces across
/// consecutive saves, reboots, crashes, and restores.
///
/// Nonce structure (12 bytes):
/// [0..4]: user_id (u32 LE)
/// [4..8]: generation (u32 LE low 32 of persistent generation)
/// [8..12]: monotonic save counter (u32 LE)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NonceGenerator {
    pub user_id: u32,
    pub generation: u64,
    pub counter: u32,
}

impl NonceGenerator {
    pub const fn new(user_id: u32, generation: u64) -> Self {
        Self {
            user_id,
            generation,
            counter: 0,
        }
    }

    /// Generates the next unique 12-byte nonce and increments the monotonic counter.
    pub fn next_nonce(&mut self) -> [u8; 12] {
        self.counter = self.counter.wrapping_add(1);
        let mut nonce = [0u8; 12];
        nonce[0..4].copy_from_slice(&self.user_id.to_le_bytes());
        nonce[4..8].copy_from_slice(&(self.generation as u32).to_le_bytes());
        nonce[8..12].copy_from_slice(&self.counter.to_le_bytes());
        nonce
    }
}

/// Builds the authoritative per-user companion memory path: `/user/{uid}/companion/state.bin`.
pub fn companion_user_storage_path<'a>(uid: u32, buf: &'a mut [u8; 64]) -> Result<&'a str, ()> {
    let prefix = b"/user/";
    let suffix = b"/companion/state.bin";
    let mut pos = 0;
    buf[pos..pos + prefix.len()].copy_from_slice(prefix);
    pos += prefix.len();

    let mut num_buf = [0u8; 10];
    let mut num = uid;
    let mut num_len = 0;
    if num == 0 {
        num_buf[0] = b'0';
        num_len = 1;
    } else {
        while num > 0 {
            num_buf[num_len] = b'0' + (num % 10) as u8;
            num /= 10;
            num_len += 1;
        }
        num_buf[..num_len].reverse();
    }

    if pos + num_len + suffix.len() > buf.len() {
        return Err(());
    }
    buf[pos..pos + num_len].copy_from_slice(&num_buf[..num_len]);
    pos += num_len;
    buf[pos..pos + suffix.len()].copy_from_slice(suffix);
    pos += suffix.len();

    core::str::from_utf8(&buf[..pos]).map_err(|_| ())
}

/// Maximum capacity of structured companion memory tables.
pub const MAX_PREFERENCES: usize = 32;
pub const MAX_CORRECTIONS: usize = 16;
pub const MAX_ROUTINES: usize = 16;
pub const MAX_INTENT_PATTERNS: usize = 16;
pub const MAX_TOOL_STATS: usize = 32;
pub const MAX_PLAN_STEPS: usize = 8;

/// Learning mode governing how the companion records user interactions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum LearningMode {
    /// Adaptive (Active): observes user corrections, preferences, and frequent routines.
    Adaptive = 0,
    /// ExplicitOnly (ReadOnly): uses permitted state; only records explicitly saved preferences/routines.
    ExplicitOnly = 1,
    /// Off (Disabled): completely ignores personal memory and disables personalization.
    Off = 2,
    /// Custom: fine-grained per-source learning controls.
    Custom = 3,
}

impl Default for LearningMode {
    fn default() -> Self {
        Self::Adaptive
    }
}

impl LearningMode {
    #[allow(non_upper_case_globals)]
    pub const Active: Self = Self::Adaptive;
    #[allow(non_upper_case_globals)]
    pub const ReadOnly: Self = Self::ExplicitOnly;
    #[allow(non_upper_case_globals)]
    pub const Disabled: Self = Self::Off;

    pub const fn from_u8(val: u8) -> Self {
        match val {
            1 => Self::ExplicitOnly,
            2 => Self::Off,
            3 => Self::Custom,
            _ => Self::Adaptive,
        }
    }

    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Adaptive => "Active",
            Self::ExplicitOnly => "Read Only",
            Self::Off => "Disabled",
            Self::Custom => "Custom",
        }
    }

    pub const fn consumer_label(&self) -> &'static str {
        match self {
            Self::Adaptive => "Adaptive",
            Self::ExplicitOnly => "Explicit only",
            Self::Off => "Off",
            Self::Custom => "Custom",
        }
    }
}

/// Category of personal preference.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum PreferenceCategory {
    Theme = 0,
    Audio = 1,
    Display = 2,
    Model = 3,
    Search = 4,
    Apps = 5,
    Custom = 6,
}

impl PreferenceCategory {
    pub const fn from_u8(val: u8) -> Self {
        match val {
            0 => Self::Theme,
            1 => Self::Audio,
            2 => Self::Display,
            3 => Self::Model,
            4 => Self::Search,
            5 => Self::Apps,
            _ => Self::Custom,
        }
    }
}

/// A structured preference record.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PreferenceRecord {
    pub category: PreferenceCategory,
    pub key: [u8; 32],
    pub key_len: u8,
    pub value: [u8; 64],
    pub value_len: u8,
    pub confidence: u8,
    pub updated_at: u64,
}

impl Default for PreferenceRecord {
    fn default() -> Self {
        Self {
            category: PreferenceCategory::Custom,
            key: [0u8; 32],
            key_len: 0,
            value: [0u8; 64],
            value_len: 0,
            confidence: 0,
            updated_at: 0,
        }
    }
}

impl PreferenceRecord {
    pub fn new(cat: PreferenceCategory, key: &[u8], val: &[u8], confidence: u8, now: u64) -> Self {
        let mut rec = Self {
            category: cat,
            key: [0u8; 32],
            key_len: key.len().min(32) as u8,
            value: [0u8; 64],
            value_len: val.len().min(64) as u8,
            confidence: confidence.min(100),
            updated_at: now,
        };
        rec.key[..rec.key_len as usize].copy_from_slice(&key[..rec.key_len as usize]);
        rec.value[..rec.value_len as usize].copy_from_slice(&val[..rec.value_len as usize]);
        rec
    }

    pub fn key_str(&self) -> &str {
        core::str::from_utf8(&self.key[..self.key_len as usize]).unwrap_or("")
    }

    pub fn value_str(&self) -> &str {
        core::str::from_utf8(&self.value[..self.value_len as usize]).unwrap_or("")
    }
}

/// A user correction record capturing when the user overturned or amended a suggestion.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UserCorrection {
    pub query: [u8; 64],
    pub query_len: u8,
    pub system_action: [u8; 64],
    pub action_len: u8,
    pub user_fix: [u8; 64],
    pub fix_len: u8,
    pub timestamp: u64,
}

impl Default for UserCorrection {
    fn default() -> Self {
        Self {
            query: [0u8; 64],
            query_len: 0,
            system_action: [0u8; 64],
            action_len: 0,
            user_fix: [0u8; 64],
            fix_len: 0,
            timestamp: 0,
        }
    }
}

impl UserCorrection {
    pub fn new(query: &[u8], action: &[u8], fix: &[u8], now: u64) -> Self {
        let mut rec = Self {
            query: [0u8; 64],
            query_len: query.len().min(64) as u8,
            system_action: [0u8; 64],
            action_len: action.len().min(64) as u8,
            user_fix: [0u8; 64],
            fix_len: fix.len().min(64) as u8,
            timestamp: now,
        };
        rec.query[..rec.query_len as usize].copy_from_slice(&query[..rec.query_len as usize]);
        rec.system_action[..rec.action_len as usize].copy_from_slice(&action[..rec.action_len as usize]);
        rec.user_fix[..rec.fix_len as usize].copy_from_slice(&fix[..rec.fix_len as usize]);
        rec
    }
}

/// A recurring routine pattern.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RoutineRecord {
    pub trigger_name: [u8; 32],
    pub trigger_len: u8,
    pub action: [u8; 64],
    pub action_len: u8,
    pub count: u32,
    pub last_executed: u64,
}

impl Default for RoutineRecord {
    fn default() -> Self {
        Self {
            trigger_name: [0u8; 32],
            trigger_len: 0,
            action: [0u8; 64],
            action_len: 0,
            count: 0,
            last_executed: 0,
        }
    }
}

/// An intent match pattern mapped to an AppMCP tool.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IntentPattern {
    pub pattern: [u8; 48],
    pub pattern_len: u8,
    pub tool_id: u32,
    pub confidence: u8,
}

impl Default for IntentPattern {
    fn default() -> Self {
        Self {
            pattern: [0u8; 48],
            pattern_len: 0,
            tool_id: 0,
            confidence: 0,
        }
    }
}

/// Statistics on tool execution success and failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ToolSuccessRecord {
    pub tool_id: u32,
    pub invocations: u32,
    pub successes: u32,
    pub failures: u32,
    pub last_failure_code: i32,
}

impl Default for ToolSuccessRecord {
    fn default() -> Self {
        Self {
            tool_id: 0,
            invocations: 0,
            successes: 0,
            failures: 0,
            last_failure_code: 0,
        }
    }
}

/// Execution status of an interactive plan.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum PlanStatus {
    Proposed = 0,
    AwaitingConsent = 1,
    Running = 2,
    Completed = 3,
    Failed = 4,
    Cancelled = 5,
}

impl PlanStatus {
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Proposed => "Proposed",
            Self::AwaitingConsent => "Awaiting Consent",
            Self::Running => "Running",
            Self::Completed => "Completed",
            Self::Failed => "Failed",
            Self::Cancelled => "Cancelled",
        }
    }
}

/// Execution status of a single step within a plan.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum StepStatus {
    Pending = 0,
    AwaitingConsent = 1,
    Executing = 2,
    Succeeded = 3,
    Failed = 4,
    Skipped = 5,
}

/// A single discrete step in an execution plan.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PlanStep {
    pub tool_id: u32,
    pub risk_class: ToolRiskClass,
    pub consent_class: ToolConsentClass,
    pub status: StepStatus,
    pub args: [u8; 64],
    pub args_len: u8,
}

impl Default for PlanStep {
    fn default() -> Self {
        Self {
            tool_id: 0,
            risk_class: ToolRiskClass::Observe,
            consent_class: ToolConsentClass::AlwaysAsk,
            status: StepStatus::Pending,
            args: [0u8; 64],
            args_len: 0,
        }
    }
}

impl PlanStep {
    pub fn args_bytes(&self) -> &[u8] {
        &self.args[..self.args_len as usize]
    }

    pub fn args_str(&self) -> &str {
        core::str::from_utf8(self.args_bytes()).unwrap_or("")
    }
}

/// An interactive multi-step execution plan subject to user consent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExecutionPlan {
    pub id: u64,
    pub status: PlanStatus,
    pub steps: [PlanStep; MAX_PLAN_STEPS],
    pub step_count: usize,
    pub current_step: usize,
    pub user_consent_granted: bool,
}

impl Default for ExecutionPlan {
    fn default() -> Self {
        Self {
            id: 0,
            status: PlanStatus::Proposed,
            steps: [PlanStep::default(); MAX_PLAN_STEPS],
            step_count: 0,
            current_step: 0,
            user_consent_granted: false,
        }
    }
}

impl ExecutionPlan {
    pub fn new(id: u64) -> Self {
        Self {
            id,
            status: PlanStatus::Proposed,
            steps: [PlanStep::default(); MAX_PLAN_STEPS],
            step_count: 0,
            current_step: 0,
            user_consent_granted: false,
        }
    }

    pub fn add_step(
        &mut self,
        tool_id: u32,
        risk: ToolRiskClass,
        consent: ToolConsentClass,
        args: &[u8],
    ) -> bool {
        if self.step_count >= MAX_PLAN_STEPS {
            return false;
        }
        let mut step = PlanStep {
            tool_id,
            risk_class: risk,
            consent_class: consent,
            status: StepStatus::Pending,
            args: [0u8; 64],
            args_len: args.len().min(64) as u8,
        };
        step.args[..step.args_len as usize].copy_from_slice(&args[..step.args_len as usize]);
        self.steps[self.step_count] = step;
        self.step_count += 1;
        true
    }

    pub fn requires_user_consent(&self) -> bool {
        for i in 0..self.step_count {
            match self.steps[i].consent_class {
                ToolConsentClass::AlwaysAsk | ToolConsentClass::AllowOnce => return true,
                ToolConsentClass::NeverAllow => return true,
                _ => {}
            }
            if matches!(self.steps[i].risk_class, ToolRiskClass::SecuritySensitive | ToolRiskClass::PersonalMutation) {
                return true;
            }
        }
        false
    }

    pub fn advance_step(&mut self, success: bool) -> PlanStatus {
        if self.current_step >= self.step_count {
            self.status = PlanStatus::Completed;
            return self.status;
        }

        if !success {
            self.steps[self.current_step].status = StepStatus::Failed;
            self.status = PlanStatus::Failed;
            return self.status;
        }

        self.steps[self.current_step].status = StepStatus::Succeeded;
        self.current_step += 1;

        if self.current_step >= self.step_count {
            self.status = PlanStatus::Completed;
        } else {
            self.status = PlanStatus::Running;
        }
        self.status
    }

    pub fn cancel(&mut self) {
        self.status = PlanStatus::Cancelled;
        if self.current_step < self.step_count {
            self.steps[self.current_step].status = StepStatus::Skipped;
        }
    }
}

pub const REC_TAG_PREFERENCE: u8 = 0x01;
pub const REC_TAG_CORRECTION: u8 = 0x02;
pub const REC_TAG_ROUTINE: u8    = 0x03;
pub const REC_TAG_PATTERN: u8    = 0x04;
pub const REC_TAG_TOOL_STAT: u8  = 0x05;

/// Standard IEEE 802.3 CRC-32 for binary header validation.
pub fn crc32(data: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for &b in data {
        crc ^= b as u32;
        for _ in 0..8 {
            if crc & 1 != 0 {
                crc = (crc >> 1) ^ 0xEDB8_8320;
            } else {
                crc >>= 1;
            }
        }
    }
    !crc
}

/// Primary companion state managing memory, privacy settings, lock sanitation, and plans.
pub struct CompanionState {
    pub preferences: [PreferenceRecord; MAX_PREFERENCES],
    pub pref_count: usize,
    pub corrections: [UserCorrection; MAX_CORRECTIONS],
    pub corr_count: usize,
    pub routines: [RoutineRecord; MAX_ROUTINES],
    pub routine_count: usize,
    pub patterns: [IntentPattern; MAX_INTENT_PATTERNS],
    pub pattern_count: usize,
    pub tool_stats: [ToolSuccessRecord; MAX_TOOL_STATS],
    pub tool_stat_count: usize,
    pub active_plan: Option<ExecutionPlan>,
    pub learning_mode: LearningMode,
    pub is_private_session: bool,
    pub is_locked: bool,
    pub user_id: u32,
    pub generation: u64,
}

impl Default for CompanionState {
    fn default() -> Self {
        Self::new()
    }
}

impl CompanionState {
    pub const fn new() -> Self {
        Self {
            preferences: [PreferenceRecord {
                category: PreferenceCategory::Custom,
                key: [0u8; 32],
                key_len: 0,
                value: [0u8; 64],
                value_len: 0,
                confidence: 0,
                updated_at: 0,
            }; MAX_PREFERENCES],
            pref_count: 0,
            corrections: [UserCorrection {
                query: [0u8; 64],
                query_len: 0,
                system_action: [0u8; 64],
                action_len: 0,
                user_fix: [0u8; 64],
                fix_len: 0,
                timestamp: 0,
            }; MAX_CORRECTIONS],
            corr_count: 0,
            routines: [RoutineRecord {
                trigger_name: [0u8; 32],
                trigger_len: 0,
                action: [0u8; 64],
                action_len: 0,
                count: 0,
                last_executed: 0,
            }; MAX_ROUTINES],
            routine_count: 0,
            patterns: [IntentPattern {
                pattern: [0u8; 48],
                pattern_len: 0,
                tool_id: 0,
                confidence: 0,
            }; MAX_INTENT_PATTERNS],
            pattern_count: 0,
            tool_stats: [ToolSuccessRecord {
                tool_id: 0,
                invocations: 0,
                successes: 0,
                failures: 0,
                last_failure_code: 0,
            }; MAX_TOOL_STATS],
            tool_stat_count: 0,
            active_plan: None,
            learning_mode: LearningMode::Adaptive,
            is_private_session: false,
            is_locked: false,
            user_id: 1000,
            generation: 1,
        }
    }

    /// Sets the lock state. When locked, in-memory plaintexts are wiped to prevent leak.
    pub fn set_locked(&mut self, locked: bool) {
        self.is_locked = locked;
        if locked {
            self.wipe_in_memory_plaintext();
        }
    }

    /// Wipes in-memory plaintexts using volatile writes to prevent compiler dead-code elimination.
    pub fn wipe_in_memory_plaintext(&mut self) {
        for pref in self.preferences.iter_mut() {
            secure_zero(&mut pref.key);
            secure_zero(&mut pref.value);
            pref.key_len = 0;
            pref.value_len = 0;
            pref.confidence = 0;
            pref.updated_at = 0;
        }
        self.pref_count = 0;

        for corr in self.corrections.iter_mut() {
            secure_zero(&mut corr.query);
            secure_zero(&mut corr.system_action);
            secure_zero(&mut corr.user_fix);
            corr.query_len = 0;
            corr.action_len = 0;
            corr.fix_len = 0;
            corr.timestamp = 0;
        }
        self.corr_count = 0;

        for routine in self.routines.iter_mut() {
            secure_zero(&mut routine.trigger_name);
            secure_zero(&mut routine.action);
            routine.trigger_len = 0;
            routine.action_len = 0;
            routine.count = 0;
            routine.last_executed = 0;
        }
        self.routine_count = 0;

        for pat in self.patterns.iter_mut() {
            secure_zero(&mut pat.pattern);
            pat.pattern_len = 0;
            pat.tool_id = 0;
            pat.confidence = 0;
        }
        self.pattern_count = 0;

        for stat in self.tool_stats.iter_mut() {
            stat.tool_id = 0;
            stat.invocations = 0;
            stat.successes = 0;
            stat.failures = 0;
            stat.last_failure_code = 0;
        }
        self.tool_stat_count = 0;

        if let Some(ref mut plan) = self.active_plan {
            for step in plan.steps.iter_mut() {
                secure_zero(&mut step.args);
                step.args_len = 0;
            }
        }
        self.active_plan = None;
    }

    pub fn set_learning_mode(&mut self, mode: LearningMode) {
        self.learning_mode = mode;
    }

    pub fn set_private_session(&mut self, private: bool) {
        self.is_private_session = private;
    }

    pub fn clear_memory(&mut self) {
        self.wipe_in_memory_plaintext();
    }

    pub fn add_preference(
        &mut self,
        category: PreferenceCategory,
        key: &[u8],
        val: &[u8],
        confidence: u8,
        now: u64,
    ) -> bool {
        if self.is_locked || self.is_private_session || self.learning_mode == LearningMode::Off {
            return false;
        }

        // Update if existing key matches
        for i in 0..self.pref_count {
            if self.preferences[i].category == category
                && &self.preferences[i].key[..self.preferences[i].key_len as usize] == key
            {
                let vlen = val.len().min(64);
                self.preferences[i].value[..vlen].copy_from_slice(&val[..vlen]);
                self.preferences[i].value_len = vlen as u8;
                self.preferences[i].confidence = confidence.min(100);
                self.preferences[i].updated_at = now;
                return true;
            }
        }

        if self.pref_count < MAX_PREFERENCES {
            self.preferences[self.pref_count] =
                PreferenceRecord::new(category, key, val, confidence, now);
            self.pref_count += 1;
            true
        } else {
            false
        }
    }

    pub fn get_preference(&self, category: PreferenceCategory, key: &[u8]) -> Option<&PreferenceRecord> {
        if self.is_locked || self.learning_mode == LearningMode::Off {
            return None;
        }
        for i in 0..self.pref_count {
            if self.preferences[i].category == category
                && &self.preferences[i].key[..self.preferences[i].key_len as usize] == key
            {
                return Some(&self.preferences[i]);
            }
        }
        None
    }

    pub fn add_correction(&mut self, query: &[u8], action: &[u8], fix: &[u8], now: u64) -> bool {
        if self.is_locked || self.is_private_session || self.learning_mode != LearningMode::Adaptive {
            return false;
        }
        if self.corr_count < MAX_CORRECTIONS {
            self.corrections[self.corr_count] = UserCorrection::new(query, action, fix, now);
            self.corr_count += 1;
            true
        } else {
            // Overwrite oldest
            self.corrections[0] = UserCorrection::new(query, action, fix, now);
            true
        }
    }

    pub fn record_routine(&mut self, trigger: &[u8], action: &[u8], now: u64) -> bool {
        if self.is_locked || self.is_private_session || self.learning_mode != LearningMode::Adaptive {
            return false;
        }
        for i in 0..self.routine_count {
            if &self.routines[i].trigger_name[..self.routines[i].trigger_len as usize] == trigger
                && &self.routines[i].action[..self.routines[i].action_len as usize] == action
            {
                self.routines[i].count = self.routines[i].count.saturating_add(1);
                self.routines[i].last_executed = now;
                return true;
            }
        }
        if self.routine_count < MAX_ROUTINES {
            let mut r = RoutineRecord {
                trigger_name: [0u8; 32],
                trigger_len: trigger.len().min(32) as u8,
                action: [0u8; 64],
                action_len: action.len().min(64) as u8,
                count: 1,
                last_executed: now,
            };
            r.trigger_name[..r.trigger_len as usize].copy_from_slice(&trigger[..r.trigger_len as usize]);
            r.action[..r.action_len as usize].copy_from_slice(&action[..r.action_len as usize]);
            self.routines[self.routine_count] = r;
            self.routine_count += 1;
            true
        } else {
            false
        }
    }

    pub fn record_tool_result(&mut self, tool_id: u32, success: bool, err_code: i32) {
        if self.is_locked || self.is_private_session || self.learning_mode != LearningMode::Adaptive {
            return;
        }
        for i in 0..self.tool_stat_count {
            if self.tool_stats[i].tool_id == tool_id {
                self.tool_stats[i].invocations = self.tool_stats[i].invocations.saturating_add(1);
                if success {
                    self.tool_stats[i].successes = self.tool_stats[i].successes.saturating_add(1);
                } else {
                    self.tool_stats[i].failures = self.tool_stats[i].failures.saturating_add(1);
                    self.tool_stats[i].last_failure_code = err_code;
                }
                return;
            }
        }
        if self.tool_stat_count < MAX_TOOL_STATS {
            self.tool_stats[self.tool_stat_count] = ToolSuccessRecord {
                tool_id,
                invocations: 1,
                successes: if success { 1 } else { 0 },
                failures: if success { 0 } else { 1 },
                last_failure_code: err_code,
            };
            self.tool_stat_count += 1;
        }
    }

    pub fn record_intent_pattern(&mut self, pattern: &[u8], tool_id: u32, confidence: u8) -> bool {
        if self.is_locked || self.is_private_session || self.learning_mode != LearningMode::Adaptive {
            return false;
        }
        if self.pattern_count < MAX_INTENT_PATTERNS {
            let mut pat = [0u8; 48];
            let plen = pattern.len().min(48);
            pat[..plen].copy_from_slice(&pattern[..plen]);
            self.patterns[self.pattern_count] = IntentPattern {
                pattern: pat,
                pattern_len: plen as u8,
                tool_id,
                confidence: confidence.min(100),
            };
            self.pattern_count += 1;
            true
        } else {
            false
        }
    }

    /// Serializes and encrypts state with ChaCha20Poly1305.
    /// Wire format:
    /// [0..4]: COMPANION_FORMAT_MAGIC ("IOCM")
    /// [4..6]: COMPANION_FORMAT_VERSION (1)
    /// [6..14]: generation (u64 LE)
    /// [14..18]: user_binding (u32 LE)
    /// [18]: LearningMode
    /// [19]: privacy_flags
    /// [20..22]: record_count (u16 LE)
    /// [22..24]: record_type_mask (u16 LE)
    /// [24..28]: record_length (u32 LE)
    /// [28..32]: cipher_metadata (0x0001 = ChaCha20Poly1305)
    /// [32..44]: 12-byte Nonce
    /// [44..60]: 16-byte Poly1305 Tag
    /// [60..64]: CRC32 of bytes 0..60
    /// [64..64+N]: Encrypted records payload
    pub fn serialize_encrypted(
        &self,
        key: &[u8; 32],
        nonce: &[u8; 12],
        out_buf: &mut [u8],
    ) -> Result<usize, ()> {
        if self.is_private_session {
            // Private session policy: NO records persisted to disk
            return Err(());
        }
        if self.learning_mode == LearningMode::Off {
            // Companion learning disabled: no learned state persisted
            return Ok(0);
        }

        let hdr_len = COMPANION_HEADER_SIZE;
        if out_buf.len() < hdr_len + 16 {
            return Err(());
        }

        // Pack plaintext records into scratch buffer
        let mut scratch = [0u8; 4096];
        let mut pt_cursor = 0usize;
        let mut total_records = 0u16;
        let mut record_type_mask = 0u16;

        // 1. Preferences (allowed in Adaptive, ExplicitOnly, Custom)
        for i in 0..self.pref_count {
            let p = &self.preferences[i];
            let rec_len = 1 + 1 + 32 + 1 + 64 + 1 + 8; // 108
            if pt_cursor + 2 + rec_len > scratch.len() {
                break;
            }
            scratch[pt_cursor] = REC_TAG_PREFERENCE;
            scratch[pt_cursor + 1] = rec_len as u8;
            scratch[pt_cursor + 2] = p.category as u8;
            scratch[pt_cursor + 3] = p.key_len;
            scratch[pt_cursor + 4..pt_cursor + 36].copy_from_slice(&p.key);
            scratch[pt_cursor + 36] = p.value_len;
            scratch[pt_cursor + 37..pt_cursor + 101].copy_from_slice(&p.value);
            scratch[pt_cursor + 101] = p.confidence;
            scratch[pt_cursor + 102..pt_cursor + 110].copy_from_slice(&p.updated_at.to_le_bytes());
            pt_cursor += 2 + rec_len;
            total_records += 1;
            record_type_mask |= 1 << 0;
        }

        // 2. Corrections (Adaptive only)
        if self.learning_mode == LearningMode::Adaptive {
            for i in 0..self.corr_count {
                let c = &self.corrections[i];
                let rec_len = 1 + 64 + 1 + 64 + 1 + 64 + 8; // 203
                if pt_cursor + 2 + rec_len > scratch.len() {
                    break;
                }
                scratch[pt_cursor] = REC_TAG_CORRECTION;
                scratch[pt_cursor + 1] = rec_len as u8;
                scratch[pt_cursor + 2] = c.query_len;
                scratch[pt_cursor + 3..pt_cursor + 67].copy_from_slice(&c.query);
                scratch[pt_cursor + 67] = c.action_len;
                scratch[pt_cursor + 68..pt_cursor + 132].copy_from_slice(&c.system_action);
                scratch[pt_cursor + 132] = c.fix_len;
                scratch[pt_cursor + 133..pt_cursor + 197].copy_from_slice(&c.user_fix);
                scratch[pt_cursor + 197..pt_cursor + 205].copy_from_slice(&c.timestamp.to_le_bytes());
                pt_cursor += 2 + rec_len;
                total_records += 1;
                record_type_mask |= 1 << 1;
            }
        }

        // 3. Routines (Adaptive and ExplicitOnly)
        for i in 0..self.routine_count {
            let r = &self.routines[i];
            let rec_len = 1 + 32 + 1 + 64 + 4 + 8; // 110
            if pt_cursor + 2 + rec_len > scratch.len() {
                break;
            }
            scratch[pt_cursor] = REC_TAG_ROUTINE;
            scratch[pt_cursor + 1] = rec_len as u8;
            scratch[pt_cursor + 2] = r.trigger_len;
            scratch[pt_cursor + 3..pt_cursor + 35].copy_from_slice(&r.trigger_name);
            scratch[pt_cursor + 35] = r.action_len;
            scratch[pt_cursor + 36..pt_cursor + 100].copy_from_slice(&r.action);
            scratch[pt_cursor + 100..pt_cursor + 104].copy_from_slice(&r.count.to_le_bytes());
            scratch[pt_cursor + 104..pt_cursor + 112].copy_from_slice(&r.last_executed.to_le_bytes());
            pt_cursor += 2 + rec_len;
            total_records += 1;
            record_type_mask |= 1 << 2;
        }

        // 4. Intent Patterns (Adaptive only)
        if self.learning_mode == LearningMode::Adaptive {
            for i in 0..self.pattern_count {
                let pat = &self.patterns[i];
                let rec_len = 1 + 48 + 4 + 1; // 54
                if pt_cursor + 2 + rec_len > scratch.len() {
                    break;
                }
                scratch[pt_cursor] = REC_TAG_PATTERN;
                scratch[pt_cursor + 1] = rec_len as u8;
                scratch[pt_cursor + 2] = pat.pattern_len;
                scratch[pt_cursor + 3..pt_cursor + 51].copy_from_slice(&pat.pattern);
                scratch[pt_cursor + 51..pt_cursor + 55].copy_from_slice(&pat.tool_id.to_le_bytes());
                scratch[pt_cursor + 55] = pat.confidence;
                pt_cursor += 2 + rec_len;
                total_records += 1;
                record_type_mask |= 1 << 3;
            }
        }

        // 5. Tool Success Records (Adaptive only)
        if self.learning_mode == LearningMode::Adaptive {
            for i in 0..self.tool_stat_count {
                let ts = &self.tool_stats[i];
                let rec_len = 4 + 4 + 4 + 4 + 4; // 20
                if pt_cursor + 2 + rec_len > scratch.len() {
                    break;
                }
                scratch[pt_cursor] = REC_TAG_TOOL_STAT;
                scratch[pt_cursor + 1] = rec_len as u8;
                scratch[pt_cursor + 2..pt_cursor + 6].copy_from_slice(&ts.tool_id.to_le_bytes());
                scratch[pt_cursor + 6..pt_cursor + 10].copy_from_slice(&ts.invocations.to_le_bytes());
                scratch[pt_cursor + 10..pt_cursor + 14].copy_from_slice(&ts.successes.to_le_bytes());
                scratch[pt_cursor + 14..pt_cursor + 18].copy_from_slice(&ts.failures.to_le_bytes());
                scratch[pt_cursor + 18..pt_cursor + 22].copy_from_slice(&ts.last_failure_code.to_le_bytes());
                pt_cursor += 2 + rec_len;
                total_records += 1;
                record_type_mask |= 1 << 4;
            }
        }

        let pt_len = pt_cursor;
        if hdr_len + pt_len + 16 > out_buf.len() {
            return Err(());
        }

        // Header construction (64 bytes)
        out_buf[0..4].copy_from_slice(&COMPANION_FORMAT_MAGIC);
        out_buf[4..6].copy_from_slice(&COMPANION_FORMAT_VERSION.to_le_bytes());
        out_buf[6..14].copy_from_slice(&self.generation.to_le_bytes());
        out_buf[14..18].copy_from_slice(&self.user_id.to_le_bytes());
        out_buf[18] = self.learning_mode as u8;
        let mut priv_flags = 0u8;
        if self.is_private_session { priv_flags |= 1 << 0; }
        if self.is_locked { priv_flags |= 1 << 1; }
        out_buf[19] = priv_flags;
        out_buf[20..22].copy_from_slice(&total_records.to_le_bytes());
        out_buf[22..24].copy_from_slice(&record_type_mask.to_le_bytes());
        out_buf[24..28].copy_from_slice(&(pt_len as u32).to_le_bytes());
        out_buf[28..32].copy_from_slice(&CIPHER_CHACHA20_POLY1305.to_le_bytes());
        out_buf[32..44].copy_from_slice(nonce);

        // Encrypt scratch plaintext into out_buf[64..64+pt_len]
        let mut tag = [0u8; 16];
        let (header_part, rest) = out_buf.split_at_mut(hdr_len);
        let (ct_part, _tag_part) = rest.split_at_mut(pt_len);
        ChaCha20Poly1305::encrypt(
            key,
            nonce,
            &header_part[..44], // authenticated additional data: header bytes 0..44
            &scratch[..pt_len],
            ct_part,
            &mut tag,
        );
        header_part[44..60].copy_from_slice(&tag);

        // Calculate CRC32 of header bytes 0..60
        let crc = crc32(&header_part[..60]);
        header_part[60..64].copy_from_slice(&crc.to_le_bytes());

        // Zeroize scratch buffer
        secure_zero(&mut scratch[..pt_len]);

        Ok(hdr_len + pt_len)
    }

    /// Decrypts and deserializes state using ChaCha20Poly1305.
    pub fn deserialize_encrypted(
        &mut self,
        key: &[u8; 32],
        in_buf: &[u8],
    ) -> Result<(), ()> {
        self.deserialize_encrypted_with_min_generation(key, in_buf, 0)
    }

    /// Decrypts and deserializes state, enforcing a minimum monotonic generation to defeat rollback.
    pub fn deserialize_encrypted_with_min_generation(
        &mut self,
        key: &[u8; 32],
        in_buf: &[u8],
        min_expected_generation: u64,
    ) -> Result<(), ()> {
        let hdr_len = COMPANION_HEADER_SIZE;
        if in_buf.len() < hdr_len {
            return Err(());
        }

        // Check magic
        let magic = [in_buf[0], in_buf[1], in_buf[2], in_buf[3]];
        if magic != COMPANION_FORMAT_MAGIC {
            // Check legacy COMP magic for backwards compatibility
            if u32::from_le_bytes(magic) == COMPANION_MAGIC {
                return self.deserialize_legacy(key, in_buf);
            }
            return Err(());
        }

        // Verify CRC32 of header bytes 0..60
        let want_crc = u32::from_le_bytes([in_buf[60], in_buf[61], in_buf[62], in_buf[63]]);
        if crc32(&in_buf[..60]) != want_crc {
            return Err(());
        }

        let version = u16::from_le_bytes([in_buf[4], in_buf[5]]);
        if version != COMPANION_FORMAT_VERSION {
            return Err(());
        }

        let generation = u64::from_le_bytes([
            in_buf[6], in_buf[7], in_buf[8], in_buf[9],
            in_buf[10], in_buf[11], in_buf[12], in_buf[13],
        ]);
        if min_expected_generation > 0 && generation < min_expected_generation {
            // Replay/rollback detected: reject stale state
            return Err(());
        }

        let user_binding = u32::from_le_bytes([in_buf[14], in_buf[15], in_buf[16], in_buf[17]]);
        let learning_mode = LearningMode::from_u8(in_buf[18]);
        let privacy_flags = in_buf[19];
        let total_records = u16::from_le_bytes([in_buf[20], in_buf[21]]);
        let record_length = u32::from_le_bytes([in_buf[24], in_buf[25], in_buf[26], in_buf[27]]) as usize;
        let cipher_id = u32::from_le_bytes([in_buf[28], in_buf[29], in_buf[30], in_buf[31]]);

        if cipher_id != CIPHER_CHACHA20_POLY1305 {
            return Err(());
        }

        let mut nonce = [0u8; 12];
        nonce.copy_from_slice(&in_buf[32..44]);

        let mut tag = [0u8; 16];
        tag.copy_from_slice(&in_buf[44..60]);

        if in_buf.len() < hdr_len + record_length {
            return Err(());
        }

        let ct = &in_buf[hdr_len..hdr_len + record_length];
        let mut scratch = [0u8; 4096];
        if record_length > scratch.len() {
            return Err(());
        }
        scratch[..record_length].copy_from_slice(ct);

        // Open in place using header bytes 0..44 as AAD
        if ChaCha20Poly1305::open_in_place(key, &nonce, &in_buf[..44], &mut scratch[..record_length], &tag).is_err() {
            secure_zero(&mut scratch[..record_length]);
            return Err(());
        }

        // Clear existing state before applying restored records
        self.clear_memory();
        self.generation = generation;
        self.user_id = user_binding;
        self.learning_mode = learning_mode;
        self.is_private_session = (privacy_flags & (1 << 0)) != 0;
        self.is_locked = (privacy_flags & (1 << 1)) != 0;

        // Unpack plaintext records
        let mut cursor = 0usize;
        let mut records_read = 0u16;

        while cursor + 2 <= record_length && records_read < total_records {
            let rtag = scratch[cursor];
            let rec_len = scratch[cursor + 1] as usize;
            cursor += 2;
            if cursor + rec_len > record_length {
                break;
            }

            match rtag {
                REC_TAG_PREFERENCE => {
                    if rec_len >= 108 && self.pref_count < MAX_PREFERENCES {
                        let cat = PreferenceCategory::from_u8(scratch[cursor]);
                        let klen = scratch[cursor + 1].min(32);
                        let mut k = [0u8; 32];
                        k.copy_from_slice(&scratch[cursor + 2..cursor + 34]);
                        let vlen = scratch[cursor + 34].min(64);
                        let mut v = [0u8; 64];
                        v.copy_from_slice(&scratch[cursor + 35..cursor + 99]);
                        let conf = scratch[cursor + 99];
                        let upd = u64::from_le_bytes([
                            scratch[cursor + 100], scratch[cursor + 101], scratch[cursor + 102], scratch[cursor + 103],
                            scratch[cursor + 104], scratch[cursor + 105], scratch[cursor + 106], scratch[cursor + 107],
                        ]);
                        self.preferences[self.pref_count] = PreferenceRecord {
                            category: cat,
                            key: k,
                            key_len: klen,
                            value: v,
                            value_len: vlen,
                            confidence: conf,
                            updated_at: upd,
                        };
                        self.pref_count += 1;
                    }
                }
                REC_TAG_CORRECTION => {
                    if rec_len >= 203 && self.corr_count < MAX_CORRECTIONS {
                        let qlen = scratch[cursor].min(64);
                        let mut q = [0u8; 64];
                        q.copy_from_slice(&scratch[cursor + 1..cursor + 65]);
                        let actlen = scratch[cursor + 65].min(64);
                        let mut act = [0u8; 64];
                        act.copy_from_slice(&scratch[cursor + 66..cursor + 130]);
                        let fixlen = scratch[cursor + 130].min(64);
                        let mut fix = [0u8; 64];
                        fix.copy_from_slice(&scratch[cursor + 131..cursor + 195]);
                        let ts = u64::from_le_bytes([
                            scratch[cursor + 195], scratch[cursor + 196], scratch[cursor + 197], scratch[cursor + 198],
                            scratch[cursor + 199], scratch[cursor + 200], scratch[cursor + 201], scratch[cursor + 202],
                        ]);
                        self.corrections[self.corr_count] = UserCorrection {
                            query: q,
                            query_len: qlen,
                            system_action: act,
                            action_len: actlen,
                            user_fix: fix,
                            fix_len: fixlen,
                            timestamp: ts,
                        };
                        self.corr_count += 1;
                    }
                }
                REC_TAG_ROUTINE => {
                    if rec_len >= 110 && self.routine_count < MAX_ROUTINES {
                        let tlen = scratch[cursor].min(32);
                        let mut trig = [0u8; 32];
                        trig.copy_from_slice(&scratch[cursor + 1..cursor + 33]);
                        let actlen = scratch[cursor + 33].min(64);
                        let mut act = [0u8; 64];
                        act.copy_from_slice(&scratch[cursor + 34..cursor + 98]);
                        let count = u32::from_le_bytes([
                            scratch[cursor + 98], scratch[cursor + 99], scratch[cursor + 100], scratch[cursor + 101],
                        ]);
                        let last_exec = u64::from_le_bytes([
                            scratch[cursor + 102], scratch[cursor + 103], scratch[cursor + 104], scratch[cursor + 105],
                            scratch[cursor + 106], scratch[cursor + 107], scratch[cursor + 108], scratch[cursor + 109],
                        ]);
                        self.routines[self.routine_count] = RoutineRecord {
                            trigger_name: trig,
                            trigger_len: tlen,
                            action: act,
                            action_len: actlen,
                            count,
                            last_executed: last_exec,
                        };
                        self.routine_count += 1;
                    }
                }
                REC_TAG_PATTERN => {
                    if rec_len >= 54 && self.pattern_count < MAX_INTENT_PATTERNS {
                        let patlen = scratch[cursor].min(48);
                        let mut pat = [0u8; 48];
                        pat.copy_from_slice(&scratch[cursor + 1..cursor + 49]);
                        let tid = u32::from_le_bytes([
                            scratch[cursor + 49], scratch[cursor + 50], scratch[cursor + 51], scratch[cursor + 52],
                        ]);
                        let conf = scratch[cursor + 53];
                        self.patterns[self.pattern_count] = IntentPattern {
                            pattern: pat,
                            pattern_len: patlen,
                            tool_id: tid,
                            confidence: conf,
                        };
                        self.pattern_count += 1;
                    }
                }
                REC_TAG_TOOL_STAT => {
                    if rec_len >= 20 && self.tool_stat_count < MAX_TOOL_STATS {
                        let tid = u32::from_le_bytes([
                            scratch[cursor], scratch[cursor + 1], scratch[cursor + 2], scratch[cursor + 3],
                        ]);
                        let inv = u32::from_le_bytes([
                            scratch[cursor + 4], scratch[cursor + 5], scratch[cursor + 6], scratch[cursor + 7],
                        ]);
                        let succ = u32::from_le_bytes([
                            scratch[cursor + 8], scratch[cursor + 9], scratch[cursor + 10], scratch[cursor + 11],
                        ]);
                        let fail = u32::from_le_bytes([
                            scratch[cursor + 12], scratch[cursor + 13], scratch[cursor + 14], scratch[cursor + 15],
                        ]);
                        let err = i32::from_le_bytes([
                            scratch[cursor + 16], scratch[cursor + 17], scratch[cursor + 18], scratch[cursor + 19],
                        ]);
                        self.tool_stats[self.tool_stat_count] = ToolSuccessRecord {
                            tool_id: tid,
                            invocations: inv,
                            successes: succ,
                            failures: fail,
                            last_failure_code: err,
                        };
                        self.tool_stat_count += 1;
                    }
                }
                _ => {}
            }

            cursor += rec_len;
            records_read += 1;
        }

        secure_zero(&mut scratch[..record_length]);
        Ok(())
    }

    fn deserialize_legacy(&mut self, key: &[u8; 32], in_buf: &[u8]) -> Result<(), ()> {
        let hdr_len = 28;
        if in_buf.len() < hdr_len + 16 {
            return Err(());
        }
        let pref_count = u16::from_le_bytes([in_buf[10], in_buf[11]]) as usize;
        let mut nonce = [0u8; 12];
        nonce.copy_from_slice(&in_buf[16..28]);

        let ct_len = in_buf.len() - hdr_len - 16;
        let mut tag = [0u8; 16];
        tag.copy_from_slice(&in_buf[in_buf.len() - 16..]);

        let mut scratch = [0u8; 4096];
        if ct_len > scratch.len() {
            return Err(());
        }
        scratch[..ct_len].copy_from_slice(&in_buf[hdr_len..hdr_len + ct_len]);

        if ChaCha20Poly1305::open_in_place(key, &nonce, &in_buf[..hdr_len], &mut scratch[..ct_len], &tag).is_err() {
            return Err(());
        }

        self.pref_count = 0;
        let mut cursor = 0usize;
        let rec_size = 1 + 1 + 32 + 1 + 64 + 1 + 8;
        for _ in 0..pref_count.min(MAX_PREFERENCES) {
            if cursor + rec_size > ct_len {
                break;
            }
            let cat = PreferenceCategory::from_u8(scratch[cursor]);
            let klen = scratch[cursor + 1].min(32);
            let mut key_buf = [0u8; 32];
            key_buf.copy_from_slice(&scratch[cursor + 2..cursor + 34]);
            let vlen = scratch[cursor + 34].min(64);
            let mut val_buf = [0u8; 64];
            val_buf.copy_from_slice(&scratch[cursor + 35..cursor + 99]);
            let conf = scratch[cursor + 99];
            let updated = u64::from_le_bytes([
                scratch[cursor + 100], scratch[cursor + 101], scratch[cursor + 102], scratch[cursor + 103],
                scratch[cursor + 104], scratch[cursor + 105], scratch[cursor + 106], scratch[cursor + 107],
            ]);

            self.preferences[self.pref_count] = PreferenceRecord {
                category: cat,
                key: key_buf,
                key_len: klen,
                value: val_buf,
                value_len: vlen,
                confidence: conf,
                updated_at: updated,
            };
            self.pref_count += 1;
            cursor += rec_size;
        }

        Ok(())
    }
}

/// Generic model role defaults and replacement routing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum ModelRole {
    TextGeneration = 0,
    TextEmbedding = 1,
    SpeechToText = 2,
    TextToSpeech = 3,
    VisionUnderstanding = 4,
}

impl ModelRole {
    pub const ALL: [Self; 5] = [
        Self::TextGeneration,
        Self::TextEmbedding,
        Self::SpeechToText,
        Self::TextToSpeech,
        Self::VisionUnderstanding,
    ];

    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::TextGeneration => "Text Generation",
            Self::TextEmbedding => "Text Embedding",
            Self::SpeechToText => "Speech to Text",
            Self::TextToSpeech => "Text to Speech",
            Self::VisionUnderstanding => "Vision Understanding",
        }
    }
}

/// A binding between a ModelRole and a configured model identity/provider.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ModelRoleBinding {
    pub role: ModelRole,
    pub model_id: [u8; 32],
    pub model_id_len: u8,
    pub provider_type: u8, // 0 = native gguf, 1 = litert, 2 = custom
    pub is_custom: bool,
    pub enabled: bool,
}

impl Default for ModelRoleBinding {
    fn default() -> Self {
        Self {
            role: ModelRole::TextGeneration,
            model_id: [0u8; 32],
            model_id_len: 0,
            provider_type: 0,
            is_custom: false,
            enabled: true,
        }
    }
}

impl ModelRoleBinding {
    pub fn new(role: ModelRole, id: &[u8], provider_type: u8, is_custom: bool) -> Self {
        let mut b = Self {
            role,
            model_id: [0u8; 32],
            model_id_len: id.len().min(32) as u8,
            provider_type,
            is_custom,
            enabled: true,
        };
        b.model_id[..b.model_id_len as usize].copy_from_slice(&id[..b.model_id_len as usize]);
        b
    }

    pub fn model_id_str(&self) -> &str {
        core::str::from_utf8(&self.model_id[..self.model_id_len as usize]).unwrap_or("")
    }
}

/// Registry of default model bindings for all system AI roles.
pub struct ModelRegistryDefaults {
    pub bindings: [ModelRoleBinding; 5],
}

impl Default for ModelRegistryDefaults {
    fn default() -> Self {
        Self::new()
    }
}

impl ModelRegistryDefaults {
    /// Constructs an unselected default configuration.
    /// An empty or new installation has no hardcoded model names;
    /// every role starts as 'Not selected' unless provisioned.
    pub const fn new() -> Self {
        Self {
            bindings: [
                ModelRoleBinding {
                    role: ModelRole::TextGeneration,
                    model_id: [0u8; 32],
                    model_id_len: 0,
                    provider_type: 0,
                    is_custom: false,
                    enabled: false,
                },
                ModelRoleBinding {
                    role: ModelRole::TextEmbedding,
                    model_id: [0u8; 32],
                    model_id_len: 0,
                    provider_type: 0,
                    is_custom: false,
                    enabled: false,
                },
                ModelRoleBinding {
                    role: ModelRole::SpeechToText,
                    model_id: [0u8; 32],
                    model_id_len: 0,
                    provider_type: 0,
                    is_custom: false,
                    enabled: false,
                },
                ModelRoleBinding {
                    role: ModelRole::TextToSpeech,
                    model_id: [0u8; 32],
                    model_id_len: 0,
                    provider_type: 0,
                    is_custom: false,
                    enabled: false,
                },
                ModelRoleBinding {
                    role: ModelRole::VisionUnderstanding,
                    model_id: [0u8; 32],
                    model_id_len: 0,
                    provider_type: 0,
                    is_custom: false,
                    enabled: false,
                },
            ],
        }
    }

    /// Explicitly loads provisioned model weights when present on reference images.
    pub fn with_provisioned() -> Self {
        let mut reg = Self::new();
        reg.set_binding(ModelRole::TextGeneration, b"gemma-2-2b-it", 0, false);
        reg.set_binding(ModelRole::TextEmbedding, b"embeddinggemma-256", 0, false);
        reg.set_binding(ModelRole::SpeechToText, b"whisper-tiny-en", 1, false);
        reg.set_binding(ModelRole::TextToSpeech, b"piper-en-low", 1, false);
        reg.set_binding(ModelRole::VisionUnderstanding, b"paligemma-3b", 0, false);
        reg
    }

    pub fn get_binding(&self, role: ModelRole) -> Option<&ModelRoleBinding> {
        self.bindings.iter().find(|b| b.role == role && b.enabled && b.model_id_len > 0)
    }

    pub fn set_binding(&mut self, role: ModelRole, model_id: &[u8], provider: u8, is_custom: bool) {
        for b in self.bindings.iter_mut() {
            if b.role == role {
                *b = ModelRoleBinding::new(role, model_id, provider, is_custom);
                return;
            }
        }
    }

    pub fn toggle_enabled(&mut self, role: ModelRole, enabled: bool) {
        for b in self.bindings.iter_mut() {
            if b.role == role {
                b.enabled = enabled;
                return;
            }
        }
    }

    /// Serializes model role bindings to a compact binary format for persistence.
    pub fn serialize(&self, out: &mut [u8]) -> Result<usize, ()> {
        if out.len() < 4 + 5 * 36 {
            return Err(());
        }
        out[0..4].copy_from_slice(b"MRDF");
        let mut cursor = 4;
        for b in &self.bindings {
            out[cursor] = b.role as u8;
            out[cursor + 1] = b.model_id_len;
            out[cursor + 2..cursor + 34].copy_from_slice(&b.model_id);
            out[cursor + 34] = b.provider_type;
            let flags = (if b.is_custom { 1 } else { 0 }) | (if b.enabled { 2 } else { 0 });
            out[cursor + 35] = flags;
            cursor += 36;
        }
        Ok(cursor)
    }

    /// Deserializes model role bindings from binary storage.
    pub fn deserialize(&mut self, in_buf: &[u8]) -> Result<(), ()> {
        if in_buf.len() < 4 + 5 * 36 || &in_buf[0..4] != b"MRDF" {
            return Err(());
        }
        let mut cursor = 4;
        for i in 0..5 {
            let role_u8 = in_buf[cursor];
            let id_len = in_buf[cursor + 1].min(32);
            let mut id = [0u8; 32];
            id.copy_from_slice(&in_buf[cursor + 2..cursor + 34]);
            let provider = in_buf[cursor + 34];
            let flags = in_buf[cursor + 35];
            cursor += 36;

            let role = match role_u8 {
                0 => ModelRole::TextGeneration,
                1 => ModelRole::TextEmbedding,
                2 => ModelRole::SpeechToText,
                3 => ModelRole::TextToSpeech,
                _ => ModelRole::VisionUnderstanding,
            };

            self.bindings[i] = ModelRoleBinding {
                role,
                model_id: id,
                model_id_len: id_len,
                provider_type: provider,
                is_custom: (flags & 1) != 0,
                enabled: (flags & 2) != 0,
            };
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_companion_key_derivation_user_isolation() {
        let key_user_a = derive_companion_user_key(1000, &COMPANION_KEY_SALT);
        let key_user_b = derive_companion_user_key(1001, &COMPANION_KEY_SALT);
        assert_ne!(key_user_a, key_user_b, "different users must receive distinct cryptographic keys");

        let key_user_a_again = derive_companion_user_key(1000, &COMPANION_KEY_SALT);
        assert_eq!(key_user_a, key_user_a_again, "key derivation must be deterministic for the same user");
    }

    #[test]
    fn test_consecutive_saves_use_unique_nonces() {
        let mut gen = NonceGenerator::new(1000, 42);
        let n1 = gen.next_nonce();
        let n2 = gen.next_nonce();
        let n3 = gen.next_nonce();

        assert_ne!(n1, n2);
        assert_ne!(n2, n3);
        assert_ne!(n1, n3);

        // Generation difference ensures nonces do not collide across reboot
        let mut gen_reboot = NonceGenerator::new(1000, 43);
        let n_after_reboot = gen_reboot.next_nonce();
        assert_ne!(n1, n_after_reboot);
    }

    #[test]
    fn test_companion_state_learning_and_preferences() {
        let mut state = CompanionState::new();
        assert_eq!(state.learning_mode, LearningMode::Active);

        let ok = state.add_preference(PreferenceCategory::Theme, b"dark_mode", b"true", 95, 1000);
        assert!(ok);
        assert_eq!(state.pref_count, 1);

        let pref = state.get_preference(PreferenceCategory::Theme, b"dark_mode").unwrap();
        assert_eq!(pref.key_str(), "dark_mode");
        assert_eq!(pref.value_str(), "true");
        assert_eq!(pref.confidence, 95);

        // Update preference
        state.add_preference(PreferenceCategory::Theme, b"dark_mode", b"false", 99, 2000);
        assert_eq!(state.pref_count, 1);
        let pref = state.get_preference(PreferenceCategory::Theme, b"dark_mode").unwrap();
        assert_eq!(pref.value_str(), "false");
        assert_eq!(pref.updated_at, 2000);
    }

    #[test]
    fn test_companion_state_lock_sanitization() {
        let mut state = CompanionState::new();
        state.add_preference(PreferenceCategory::Apps, b"favorite", b"calculator", 80, 100);
        state.add_correction(b"calc", b"eval", b"launch", 150);
        state.record_routine(b"morning", b"weather", 200);
        state.record_tool_result(1, true, 0);

        assert_eq!(state.pref_count, 1);
        assert_eq!(state.corr_count, 1);
        assert_eq!(state.routine_count, 1);
        assert_eq!(state.tool_stat_count, 1);

        // Lock the device
        state.set_locked(true);
        assert!(state.is_locked);
        assert_eq!(state.pref_count, 0);
        assert_eq!(state.corr_count, 0);
        assert_eq!(state.routine_count, 0);
        assert_eq!(state.tool_stat_count, 0);

        // Plaintexts are completely wiped with volatile zeros
        assert!(state.get_preference(PreferenceCategory::Apps, b"favorite").is_none());

        // Cannot add while locked
        let ok = state.add_preference(PreferenceCategory::Apps, b"favorite", b"camera", 90, 200);
        assert!(!ok);
    }

    #[test]
    fn test_companion_private_session_and_disabled_mode() {
        let mut state = CompanionState::new();
        state.set_private_session(true);

        assert!(!state.add_preference(PreferenceCategory::Search, b"recent", b"query", 50, 100));
        assert!(!state.add_correction(b"turn off", b"settings", b"power", 100));
        assert!(!state.record_routine(b"alarm", b"sound", 100));

        let key = derive_companion_user_key(1000, &COMPANION_KEY_SALT);
        let mut buf = [0u8; 1024];
        let mut gen = NonceGenerator::new(1000, 1);
        // Private session refuses to serialize to disk
        assert!(state.serialize_encrypted(&key, &gen.next_nonce(), &mut buf).is_err());

        state.set_private_session(false);
        state.set_learning_mode(LearningMode::Disabled);
        assert!(!state.add_preference(PreferenceCategory::Search, b"recent", b"query", 50, 100));
        assert!(!state.add_correction(b"turn off", b"settings", b"power", 100));

        // In Disabled mode, zero bytes are serialized
        assert_eq!(state.serialize_encrypted(&key, &gen.next_nonce(), &mut buf).unwrap(), 0);
    }

    #[test]
    fn test_companion_readonly_mode_prevents_passive_mutation() {
        let mut state = CompanionState::new();
        state.set_learning_mode(LearningMode::ReadOnly);

        // Passive corrections and routine observations must be refused in ReadOnly
        assert!(!state.add_correction(b"type", b"typo", b"fix", 100));
        assert!(!state.record_routine(b"lunch", b"open_music", 200));
        state.record_tool_result(5, true, 0);
        assert_eq!(state.tool_stat_count, 0, "ReadOnly mode must not record tool stats");
    }

    #[test]
    fn test_companion_encryption_at_rest_roundtrip() {
        let mut state = CompanionState::new();
        state.user_id = 1000;
        state.generation = 15;
        state.add_preference(PreferenceCategory::Display, b"brightness", b"80", 90, 500);
        state.add_preference(PreferenceCategory::Audio, b"volume", b"60", 85, 600);
        state.add_correction(b"open song", b"search", b"music_play", 700);
        state.record_routine(b"evening", b"night_mode", 800);
        state.record_intent_pattern(b"check weather", 42, 90);
        state.record_tool_result(42, true, 0);

        let key = derive_companion_user_key(1000, &COMPANION_KEY_SALT);
        let mut gen = NonceGenerator::new(1000, state.generation);
        let nonce = gen.next_nonce();
        let mut buf = [0u8; 2048];

        let len = state.serialize_encrypted(&key, &nonce, &mut buf).unwrap();
        assert!(len >= COMPANION_HEADER_SIZE + 16);

        // Verify header magic and fields
        assert_eq!(&buf[0..4], b"IOCM");

        let mut loaded = CompanionState::new();
        loaded.deserialize_encrypted(&key, &buf[..len]).unwrap();

        assert_eq!(loaded.user_id, 1000);
        assert_eq!(loaded.generation, 15);
        assert_eq!(loaded.pref_count, 2);
        let p1 = loaded.get_preference(PreferenceCategory::Display, b"brightness").unwrap();
        assert_eq!(p1.value_str(), "80");
        let p2 = loaded.get_preference(PreferenceCategory::Audio, b"volume").unwrap();
        assert_eq!(p2.value_str(), "60");
        assert_eq!(loaded.corr_count, 1);
        assert_eq!(loaded.routine_count, 1);
        assert_eq!(loaded.pattern_count, 1);
        assert_eq!(loaded.tool_stat_count, 1);
    }

    #[test]
    fn test_companion_tamper_and_corruption_fails_closed() {
        let mut state = CompanionState::new();
        state.add_preference(PreferenceCategory::Display, b"brightness", b"80", 90, 500);

        let key = derive_companion_user_key(1000, &COMPANION_KEY_SALT);
        let mut gen = NonceGenerator::new(1000, 1);
        let mut buf = [0u8; 1024];
        let len = state.serialize_encrypted(&key, &gen.next_nonce(), &mut buf).unwrap();

        // 1. Tamper with ciphertext
        let mut corrupted = buf;
        corrupted[COMPANION_HEADER_SIZE + 2] ^= 0xFF;
        let mut loaded = CompanionState::new();
        assert!(loaded.deserialize_encrypted(&key, &corrupted[..len]).is_err(), "corrupted ciphertext must fail closed");

        // 2. Tamper with header CRC
        let mut bad_crc = buf;
        bad_crc[60] ^= 0xFF;
        assert!(loaded.deserialize_encrypted(&key, &bad_crc[..len]).is_err(), "header CRC mismatch must fail closed");

        // 3. Tamper with magic
        let mut bad_magic = buf;
        bad_magic[0] = b'X';
        assert!(loaded.deserialize_encrypted(&key, &bad_magic[..len]).is_err(), "bad magic must fail closed");

        // 4. Rollback detection
        assert!(loaded.deserialize_encrypted_with_min_generation(&key, &buf[..len], 100).is_err(), "replay of generation 1 when generation 100 expected must be rejected");
    }

    #[test]
    fn test_execution_plan_lifecycle() {
        let mut plan = ExecutionPlan::new(101);
        plan.add_step(
            1,
            ToolRiskClass::Observe,
            ToolConsentClass::AlwaysAllow,
            b"calc: 1+1",
        );
        plan.add_step(
            2,
            ToolRiskClass::PersonalMutation,
            ToolConsentClass::AlwaysAsk,
            b"set_alarm: 07:00",
        );

        assert_eq!(plan.step_count, 2);
        assert!(plan.requires_user_consent());
        assert_eq!(plan.status, PlanStatus::Proposed);

        let s1 = plan.advance_step(true);
        assert_eq!(s1, PlanStatus::Running);
        assert_eq!(plan.current_step, 1);

        let s2 = plan.advance_step(true);
        assert_eq!(s2, PlanStatus::Completed);
    }

    #[test]
    fn test_model_registry_defaults_unselected_by_default() {
        let defaults = ModelRegistryDefaults::new();
        // Zero models selected by default:
        for role in ModelRole::ALL {
            assert!(defaults.get_binding(role).is_none(), "no hardcoded models in new unprovisioned install");
        }

        // Test persistence roundtrip
        let provisioned = ModelRegistryDefaults::with_provisioned();
        let mut ser_buf = [0u8; 256];
        let bytes = provisioned.serialize(&mut ser_buf).unwrap();

        let mut restored = ModelRegistryDefaults::new();
        restored.deserialize(&ser_buf[..bytes]).unwrap();
        let gen = restored.get_binding(ModelRole::TextGeneration).unwrap();
        assert_eq!(gen.model_id_str(), "gemma-2-2b-it");
    }
}
