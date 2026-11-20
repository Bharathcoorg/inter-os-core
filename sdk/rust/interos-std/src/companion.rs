// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

//! Native InterOS Companion State, Memory, and Interactive Plan Management.
//!
//! Provides structured personal context, transparent user control, encrypted
//! persistence at rest, lock-state data sanitization, privacy modes, and
//! interactive execution plan state machines.

use crate::appmcp::{ToolRiskClass, ToolConsentClass};
use crate::crypto::chacha20_poly1305::ChaCha20Poly1305;

pub const COMPANION_MAGIC: u32 = 0x434F_4D50; // "COMP"
pub const COMPANION_VERSION: u32 = 1;
pub const PATH_COMPANION_MEMORY: &str = "/data/companion/memory.bin";
pub const PATH_MODEL_DEFAULTS: &str = "/data/ai/model_defaults.bin";

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
    /// Active: observes user corrections, preferences, and frequent routines.
    Active = 0,
    /// ReadOnly: uses previously learned memory but does not record new items.
    ReadOnly = 1,
    /// Disabled: completely ignores personal memory and disables personalization.
    Disabled = 2,
}

impl Default for LearningMode {
    fn default() -> Self {
        Self::Active
    }
}

impl LearningMode {
    pub const fn from_u8(val: u8) -> Self {
        match val {
            1 => Self::ReadOnly,
            2 => Self::Disabled,
            _ => Self::Active,
        }
    }

    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Active => "Active",
            Self::ReadOnly => "Read Only",
            Self::Disabled => "Disabled",
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
            learning_mode: LearningMode::Active,
            is_private_session: false,
            is_locked: false,
        }
    }

    /// Sets the lock state. When locked, in-memory plaintexts are wiped to prevent leak.
    pub fn set_locked(&mut self, locked: bool) {
        self.is_locked = locked;
        if locked {
            self.wipe_in_memory_plaintext();
        }
    }

    /// Wipes in-memory plaintexts, setting bytes to zero.
    pub fn wipe_in_memory_plaintext(&mut self) {
        for pref in self.preferences.iter_mut() {
            pref.key.fill(0);
            pref.value.fill(0);
            pref.key_len = 0;
            pref.value_len = 0;
        }
        self.pref_count = 0;

        for corr in self.corrections.iter_mut() {
            corr.query.fill(0);
            corr.system_action.fill(0);
            corr.user_fix.fill(0);
            corr.query_len = 0;
            corr.action_len = 0;
            corr.fix_len = 0;
        }
        self.corr_count = 0;

        for routine in self.routines.iter_mut() {
            routine.trigger_name.fill(0);
            routine.action.fill(0);
            routine.trigger_len = 0;
            routine.action_len = 0;
        }
        self.routine_count = 0;

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
        self.patterns.fill(IntentPattern::default());
        self.pattern_count = 0;
        self.tool_stats.fill(ToolSuccessRecord::default());
        self.tool_stat_count = 0;
    }

    pub fn add_preference(
        &mut self,
        category: PreferenceCategory,
        key: &[u8],
        val: &[u8],
        confidence: u8,
        now: u64,
    ) -> bool {
        if self.is_locked || self.is_private_session || self.learning_mode == LearningMode::Disabled {
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
        if self.is_locked || self.learning_mode == LearningMode::Disabled {
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
        if self.is_locked || self.is_private_session || self.learning_mode != LearningMode::Active {
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
        if self.is_locked || self.is_private_session || self.learning_mode != LearningMode::Active {
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
        if self.is_locked || self.is_private_session {
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

    /// Serializes and encrypts state with ChaCha20Poly1305.
    /// Wire format:
    /// [0..4]: COMPANION_MAGIC
    /// [4..8]: COMPANION_VERSION
    /// [8]: LearningMode
    /// [9]: reserved
    /// [10..12]: pref_count (u16)
    /// [12..14]: corr_count (u16)
    /// [14..16]: routine_count (u16)
    /// [16..28]: 12-byte Nonce
    /// [28..28+N]: Encrypted records payload
    /// [28+N..44+N]: 16-byte Poly1305 Tag
    pub fn serialize_encrypted(
        &self,
        key: &[u8; 32],
        nonce: &[u8; 12],
        out_buf: &mut [u8],
    ) -> Result<usize, ()> {
        let hdr_len = 28;
        if out_buf.len() < hdr_len + 16 {
            return Err(());
        }

        out_buf[0..4].copy_from_slice(&COMPANION_MAGIC.to_le_bytes());
        out_buf[4..8].copy_from_slice(&COMPANION_VERSION.to_le_bytes());
        out_buf[8] = self.learning_mode as u8;
        out_buf[9] = 0;
        out_buf[10..12].copy_from_slice(&(self.pref_count as u16).to_le_bytes());
        out_buf[12..14].copy_from_slice(&(self.corr_count as u16).to_le_bytes());
        out_buf[14..16].copy_from_slice(&(self.routine_count as u16).to_le_bytes());
        out_buf[16..28].copy_from_slice(nonce);

        // Pack plaintext records into contiguous scratch area
        let mut pt_cursor = hdr_len;
        for i in 0..self.pref_count {
            let p = &self.preferences[i];
            let rec_size = 1 + 1 + 32 + 1 + 64 + 1 + 8;
            if pt_cursor + rec_size + 16 > out_buf.len() {
                return Err(());
            }
            out_buf[pt_cursor] = p.category as u8;
            out_buf[pt_cursor + 1] = p.key_len;
            out_buf[pt_cursor + 2..pt_cursor + 34].copy_from_slice(&p.key);
            out_buf[pt_cursor + 34] = p.value_len;
            out_buf[pt_cursor + 35..pt_cursor + 99].copy_from_slice(&p.value);
            out_buf[pt_cursor + 99] = p.confidence;
            out_buf[pt_cursor + 100..pt_cursor + 108].copy_from_slice(&p.updated_at.to_le_bytes());
            pt_cursor += rec_size;
        }

        let pt_len = pt_cursor - hdr_len;
        let mut tag = [0u8; 16];
        let mut scratch = [0u8; 4096];
        if pt_len > scratch.len() {
            return Err(());
        }
        scratch[..pt_len].copy_from_slice(&out_buf[hdr_len..pt_cursor]);
        let (header_part, rest) = out_buf.split_at_mut(hdr_len);
        let (ct_part, tag_part) = rest.split_at_mut(pt_len);
        ChaCha20Poly1305::encrypt(
            key,
            nonce,
            header_part,
            &scratch[..pt_len],
            ct_part,
            &mut tag,
        );
        tag_part[..16].copy_from_slice(&tag);

        Ok(pt_cursor + 16)
    }

    /// Decrypts and deserializes state using ChaCha20Poly1305.
    pub fn deserialize_encrypted(
        &mut self,
        key: &[u8; 32],
        in_buf: &[u8],
    ) -> Result<(), ()> {
        let hdr_len = 28;
        if in_buf.len() < hdr_len + 16 {
            return Err(());
        }

        let magic = u32::from_le_bytes([in_buf[0], in_buf[1], in_buf[2], in_buf[3]]);
        let ver = u32::from_le_bytes([in_buf[4], in_buf[5], in_buf[6], in_buf[7]]);
        if magic != COMPANION_MAGIC || ver != COMPANION_VERSION {
            return Err(());
        }

        self.learning_mode = LearningMode::from_u8(in_buf[8]);
        let pref_count = u16::from_le_bytes([in_buf[10], in_buf[11]]) as usize;
        let mut nonce = [0u8; 12];
        nonce.copy_from_slice(&in_buf[16..28]);

        let ct_len = in_buf.len() - hdr_len - 16;
        let mut tag = [0u8; 16];
        tag.copy_from_slice(&in_buf[in_buf.len() - 16..]);

        // Decrypt payload in place into a scratch slice
        let mut scratch = [0u8; 4096];
        if ct_len > scratch.len() {
            return Err(());
        }
        scratch[..ct_len].copy_from_slice(&in_buf[hdr_len..hdr_len + ct_len]);

        if ChaCha20Poly1305::open_in_place(key, &nonce, &in_buf[..hdr_len], &mut scratch[..ct_len], &tag).is_err() {
            return Err(());
        }

        // Unpack preferences
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
    pub fn new() -> Self {
        let mut reg = Self {
            bindings: [
                ModelRoleBinding::new(ModelRole::TextGeneration, b"gemma-2-2b-it", 0, false),
                ModelRoleBinding::new(ModelRole::TextEmbedding, b"embeddinggemma-256", 0, false),
                ModelRoleBinding::new(ModelRole::SpeechToText, b"whisper-tiny-en", 1, false),
                ModelRoleBinding::new(ModelRole::TextToSpeech, b"piper-en-low", 1, false),
                ModelRoleBinding::new(ModelRole::VisionUnderstanding, b"paligemma-3b", 0, false),
            ],
        };
        reg.bindings[0].role = ModelRole::TextGeneration;
        reg.bindings[1].role = ModelRole::TextEmbedding;
        reg.bindings[2].role = ModelRole::SpeechToText;
        reg.bindings[3].role = ModelRole::TextToSpeech;
        reg.bindings[4].role = ModelRole::VisionUnderstanding;
        reg
    }

    pub fn get_binding(&self, role: ModelRole) -> Option<&ModelRoleBinding> {
        self.bindings.iter().find(|b| b.role == role && b.enabled)
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
}

#[cfg(test)]
mod tests {
    use super::*;

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
        assert_eq!(state.pref_count, 1);

        // Lock the device
        state.set_locked(true);
        assert!(state.is_locked);
        assert_eq!(state.pref_count, 0);
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

        state.set_private_session(false);
        state.set_learning_mode(LearningMode::Disabled);
        assert!(!state.add_preference(PreferenceCategory::Search, b"recent", b"query", 50, 100));
        assert!(!state.add_correction(b"turn off", b"settings", b"power", 100));
    }

    #[test]
    fn test_companion_encryption_at_rest_roundtrip() {
        let mut state = CompanionState::new();
        state.add_preference(PreferenceCategory::Display, b"brightness", b"80", 90, 500);
        state.add_preference(PreferenceCategory::Audio, b"volume", b"60", 85, 600);

        let key = [0x42u8; 32];
        let nonce = [0x11u8; 12];
        let mut buf = [0u8; 1024];

        let len = state.serialize_encrypted(&key, &nonce, &mut buf).unwrap();
        assert!(len > 28 + 16);

        let mut loaded = CompanionState::new();
        loaded.deserialize_encrypted(&key, &buf[..len]).unwrap();

        assert_eq!(loaded.pref_count, 2);
        let p1 = loaded.get_preference(PreferenceCategory::Display, b"brightness").unwrap();
        assert_eq!(p1.value_str(), "80");
        let p2 = loaded.get_preference(PreferenceCategory::Audio, b"volume").unwrap();
        assert_eq!(p2.value_str(), "60");
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
    fn test_model_registry_defaults() {
        let mut defaults = ModelRegistryDefaults::new();
        let binding = defaults.get_binding(ModelRole::TextGeneration).unwrap();
        assert_eq!(binding.model_id_str(), "gemma-2-2b-it");
        assert!(!binding.is_custom);

        defaults.set_binding(ModelRole::TextGeneration, b"custom-local-llama", 2, true);
        let custom = defaults.get_binding(ModelRole::TextGeneration).unwrap();
        assert_eq!(custom.model_id_str(), "custom-local-llama");
        assert!(custom.is_custom);
    }
}
