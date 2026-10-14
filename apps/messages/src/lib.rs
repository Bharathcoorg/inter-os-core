#![no_std]
//! InterOS Messages presentation, GSM-7 encoding, and threat heuristic analysis.

extern crate interos_std;

pub mod gsm;

pub const MAX_PHONE_LEN: usize = 16;
pub const MAX_SNIPPET_LEN: usize = 64;
pub const MAX_THREADS: usize = 8;
pub const MAX_DRAFT_LEN: usize = 160;

/// Bounded conversation thread record.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ConversationThread {
    pub sender: [u8; MAX_PHONE_LEN],
    pub sender_len: usize,
    pub last_snippet: [u8; MAX_SNIPPET_LEN],
    pub snippet_len: usize,
    pub unread_count: u8,
    pub last_timestamp_secs: u64,
}

impl ConversationThread {
    pub const EMPTY: Self = Self {
        sender: [0; MAX_PHONE_LEN],
        sender_len: 0,
        last_snippet: [0; MAX_SNIPPET_LEN],
        snippet_len: 0,
        unread_count: 0,
        last_timestamp_secs: 0,
    };

    pub fn new(sender_bytes: &[u8], snippet: &[u8], timestamp: u64) -> Self {
        let mut t = Self::EMPTY;
        let s = sender_bytes.len().min(MAX_PHONE_LEN);
        t.sender[..s].copy_from_slice(&sender_bytes[..s]);
        t.sender_len = s;
        let sn = snippet.len().min(MAX_SNIPPET_LEN);
        t.last_snippet[..sn].copy_from_slice(&snippet[..sn]);
        t.snippet_len = sn;
        t.last_timestamp_secs = timestamp;
        t
    }

    pub fn update_snippet(&mut self, snippet: &[u8], timestamp: u64, is_incoming: bool) {
        let sn = snippet.len().min(MAX_SNIPPET_LEN);
        self.last_snippet = [0; MAX_SNIPPET_LEN];
        self.last_snippet[..sn].copy_from_slice(&snippet[..sn]);
        self.snippet_len = sn;
        self.last_timestamp_secs = timestamp;
        if is_incoming {
            self.unread_count = self.unread_count.saturating_add(1);
        }
    }
}

/// Zero-heap SMS message composition draft buffer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MessageDraft {
    pub recipient: [u8; MAX_PHONE_LEN],
    pub recipient_len: usize,
    pub text: [u8; MAX_DRAFT_LEN],
    pub len: usize,
}

impl MessageDraft {
    pub const fn new() -> Self {
        Self {
            recipient: [0; MAX_PHONE_LEN],
            recipient_len: 0,
            text: [0; MAX_DRAFT_LEN],
            len: 0,
        }
    }

    pub fn set_recipient(&mut self, rec: &[u8]) {
        let n = rec.len().min(MAX_PHONE_LEN);
        self.recipient = [0; MAX_PHONE_LEN];
        self.recipient[..n].copy_from_slice(&rec[..n]);
        self.recipient_len = n;
    }

    pub fn push_char(&mut self, c: u8) -> bool {
        if self.len < MAX_DRAFT_LEN {
            self.text[self.len] = c;
            self.len += 1;
            true
        } else {
            false
        }
    }

    pub fn pop_char(&mut self) -> Option<u8> {
        if self.len > 0 {
            self.len -= 1;
            Some(self.text[self.len])
        } else {
            None
        }
    }

    pub fn clear(&mut self) {
        self.len = 0;
    }

    pub const fn remaining_chars(&self) -> usize {
        MAX_DRAFT_LEN - self.len
    }
}

/// Bounded conversation thread ledger.
#[derive(Debug, Clone, Copy)]
pub struct ThreadLedger {
    pub threads: [ConversationThread; MAX_THREADS],
    pub count: usize,
}

impl ThreadLedger {
    pub const fn new() -> Self {
        Self {
            threads: [ConversationThread::EMPTY; MAX_THREADS],
            count: 0,
        }
    }

    pub fn find_or_create(&mut self, sender: &[u8]) -> Option<&mut ConversationThread> {
        let s_len = sender.len().min(MAX_PHONE_LEN);
        for i in 0..self.count {
            if self.threads[i].sender_len == s_len && &self.threads[i].sender[..s_len] == &sender[..s_len] {
                return Some(&mut self.threads[i]);
            }
        }
        if self.count < MAX_THREADS {
            let idx = self.count;
            self.threads[idx] = ConversationThread::new(sender, b"", 0);
            self.count += 1;
            Some(&mut self.threads[idx])
        } else {
            None
        }
    }

    pub fn unread_total(&self) -> u32 {
        let mut sum = 0u32;
        for i in 0..self.count {
            sum += self.threads[i].unread_count as u32;
        }
        sum
    }
}

#[cfg(test)]
extern crate std;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_message_draft_limits() {
        let mut draft = MessageDraft::new();
        draft.set_recipient(b"+1234567890");
        assert_eq!(&draft.recipient[..draft.recipient_len], b"+1234567890");
        assert_eq!(draft.remaining_chars(), MAX_DRAFT_LEN);

        for _ in 0..MAX_DRAFT_LEN {
            assert!(draft.push_char(b'A'));
        }
        assert_eq!(draft.remaining_chars(), 0);
        assert!(!draft.push_char(b'B'));

        assert_eq!(draft.pop_char(), Some(b'A'));
        assert_eq!(draft.remaining_chars(), 1);
        draft.clear();
        assert_eq!(draft.len, 0);
    }

    #[test]
    fn test_thread_ledger_management() {
        let mut ledger = ThreadLedger::new();
        assert_eq!(ledger.count, 0);
        assert_eq!(ledger.unread_total(), 0);

        let t1 = ledger.find_or_create(b"+15550001").unwrap();
        t1.update_snippet(b"Hello there!", 100, true);
        assert_eq!(t1.unread_count, 1);

        let t2 = ledger.find_or_create(b"+15550002").unwrap();
        t2.update_snippet(b"Meeting at 3pm", 200, true);
        t2.update_snippet(b"See you soon", 210, true);
        assert_eq!(t2.unread_count, 2);

        assert_eq!(ledger.count, 2);
        assert_eq!(ledger.unread_total(), 3);

        // Re-finding existing returns same record
        let t1_refound = ledger.find_or_create(b"+15550001").unwrap();
        assert_eq!(&t1_refound.last_snippet[..t1_refound.snippet_len], b"Hello there!");
    }
}
