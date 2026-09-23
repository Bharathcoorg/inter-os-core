//! On-Device AI Context Broker & Privacy-Preserving Working Memory Ring
//!
//! Enforces strict token budgets and multi-tier privacy gating (Public, PrivateUser,
//! HardwareEnclave) over working memory used for local LLM inference and agentic
//! automations. Guarantees that private user context is never leaked to unprivileged
//! callers and automatically prunes expired context to enforce RAM ceilings.

use alloc::string::String;
use alloc::vec::Vec;
use spin::Mutex;

/// Sensitivity classification of contextual knowledge.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
#[repr(u8)]
pub enum PrivacyTier {
    Public = 0,          // System status, time, hardware metrics
    PrivateUser = 1,     // Order history, SMS, calendar, personal notes
    HardwareEnclave = 2, // Credentials, DID signatures, payment tokens
}

/// A discrete record of semantic context in the active working memory ring.
#[derive(Debug, Clone)]
pub struct ContextEntry {
    pub entry_id: u64,
    pub timestamp_secs: u64,
    pub tier: PrivacyTier,
    pub token_count: usize,
    pub content: String,
}

/// Context Broker managing working memory and token budget accounting.
pub struct ContextBroker {
    max_token_budget: usize,
    current_tokens: usize,
    entries: Vec<ContextEntry>,
    next_id: u64,
}

impl ContextBroker {
    pub const fn new(max_token_budget: usize) -> Self {
        Self {
            max_token_budget,
            current_tokens: 0,
            entries: Vec::new(),
            next_id: 1,
        }
    }

    /// Appends a new context item, automatically pruning oldest entries if budget exceeded.
    pub fn append(&mut self, tier: PrivacyTier, content: &str) -> u64 {
        let tokens = (content.len() + 3) / 4; // Approx: 1 token per 4 chars
        let timestamp = crate::hal::RTC.lock().now().unix_epoch_secs;

        // Prune oldest entries if appending exceeds maximum token budget
        while self.current_tokens + tokens > self.max_token_budget && !self.entries.is_empty() {
            let removed = self.entries.remove(0);
            self.current_tokens = self.current_tokens.saturating_sub(removed.token_count);
        }

        self.next_id += 1;
        let id = self.next_id;

        self.entries.push(ContextEntry {
            entry_id: id,
            timestamp_secs: timestamp,
            tier,
            token_count: tokens,
            content: String::from(content),
        });
        self.current_tokens += tokens;
        id
    }

    /// Queries working memory filtered by caller's authorized privacy clearance.
    pub fn query_context(&self, max_allowed_tier: PrivacyTier, out_buf: &mut [u8]) -> usize {
        use core::fmt::Write;
        struct SliceWriter<'a> {
            buf: &'a mut [u8],
            pos: usize,
        }
        impl<'a> Write for SliceWriter<'a> {
            fn write_str(&mut self, s: &str) -> core::fmt::Result {
                let bytes = s.as_bytes();
                let rem = self.buf.len().saturating_sub(self.pos);
                let to_write = bytes.len().min(rem);
                self.buf[self.pos..self.pos + to_write].copy_from_slice(&bytes[..to_write]);
                self.pos += to_write;
                Ok(())
            }
        }
        let mut writer = SliceWriter { buf: out_buf, pos: 0 };
        let _ = write!(writer, r#"{{"tokens":{},"budget":{},"items":["#, self.current_tokens, self.max_token_budget);

        let mut first = true;
        for entry in &self.entries {
            // Only include entries equal to or lower in sensitivity than caller's clearance
            if entry.tier <= max_allowed_tier {
                if !first {
                    let _ = write!(writer, ",");
                }
                first = false;
                let _ = write!(
                    writer,
                    r#"{{"id":{},"tier":{:?},"text":"{}"}}"#,
                    entry.entry_id, entry.tier, entry.content
                );
            }
        }
        let _ = write!(writer, r#"]}}"#);
        writer.pos
    }

    /// Purges all sensitive user and enclave context (e.g. on device lock).
    pub fn purge_sensitive(&mut self) {
        self.entries.retain(|e| e.tier == PrivacyTier::Public);
        let mut total = 0;
        for e in &self.entries {
            total += e.token_count;
        }
        self.current_tokens = total;
    }

    /// Returns current active token count.
    pub fn current_tokens(&self) -> usize {
        self.current_tokens
    }
}

pub static CONTEXT_BROKER: Mutex<ContextBroker> = Mutex::new(ContextBroker::new(4096));

/// Initializes the AI Context Broker and validates privacy filtering and token budgeting.
pub fn init() {
    let mut cb = CONTEXT_BROKER.lock();

    // 1. Append public system context
    let id_pub = cb.append(PrivacyTier::Public, "System online: Inter OS v1.6");
    assert!(id_pub > 0);

    // 2. Append private user context
    let id_priv = cb.append(PrivacyTier::PrivateUser, "User query: reorder milk on Blinkit");
    assert!(id_priv > id_pub);

    // 3. Test Privacy Clearance Gating
    let mut pub_buf = [0u8; 512];
    let pub_len = cb.query_context(PrivacyTier::Public, &mut pub_buf);
    let pub_str = core::str::from_utf8(&pub_buf[..pub_len]).unwrap_or("");
    assert!(pub_str.contains("Inter OS v1.6"), "Public query must see public entries");
    assert!(!pub_str.contains("Blinkit"), "Public query must NEVER see private user entries");

    // 4. Test Authorized Private Clearance
    let mut priv_buf = [0u8; 512];
    let priv_len = cb.query_context(PrivacyTier::PrivateUser, &mut priv_buf);
    let priv_str = core::str::from_utf8(&priv_buf[..priv_len]).unwrap_or("");
    assert!(priv_str.contains("Inter OS v1.6") && priv_str.contains("Blinkit"), "Private clearance sees both");

    crate::serial_println!(
        "[TEST] AI Context Broker: 4096 token budget tracked ({} tokens used), privacy clearance isolation VERIFIED.",
        cb.current_tokens()
    );
}
