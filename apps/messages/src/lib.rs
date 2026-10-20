#![no_std]
//! InterOS Messages presentation, GSM-7 encoding, threat heuristic engine,
//! and circular bounded thread ledger store.

extern crate interos_std;

pub mod gsm;

pub const MAX_PHONE_LEN: usize = 32;
pub const MAX_PEER_HANDLE_LEN: usize = 32;
pub const MAX_SNIPPET_LEN: usize = 64;
pub const MAX_THREADS: usize = 8;
pub const MAX_MESSAGES_PER_THREAD: usize = 16;
pub const MAX_DRAFT_LEN: usize = 160;
pub const MAX_MESSAGE_BODY_LEN: usize = 160;

/// Message transmission direction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MessageDirection {
    Incoming,
    Outgoing,
}

/// Truthful delivery status for SMS and P2P messaging.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeliveryStatus {
    Pending,
    Sent,
    Failed,
    RefusedNoTransport,
}

impl DeliveryStatus {
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Pending => "Pending",
            Self::Sent => "Sent",
            Self::Failed => "Failed",
            Self::RefusedNoTransport => "Refused - No Transport",
        }
    }
}

/// Bounded individual message record.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MessageRecord {
    pub id: u32,
    pub thread_id: u32,
    pub timestamp: u64,
    pub direction: MessageDirection,
    pub status: DeliveryStatus,
    pub body: [u8; MAX_MESSAGE_BODY_LEN],
    pub body_len: usize,
}

impl MessageRecord {
    pub const EMPTY: Self = Self {
        id: 0,
        thread_id: 0,
        timestamp: 0,
        direction: MessageDirection::Incoming,
        status: DeliveryStatus::Pending,
        body: [0; MAX_MESSAGE_BODY_LEN],
        body_len: 0,
    };

    pub fn text(&self) -> &[u8] {
        &self.body[..self.body_len]
    }
}

/// Bounded conversation thread record.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ThreadRecord {
    pub id: u32,
    pub peer_handle: [u8; MAX_PEER_HANDLE_LEN],
    pub handle_len: usize,
    pub last_timestamp: u64,
    pub unread_count: u8,
    pub message_count: u8,
}

impl ThreadRecord {
    pub const EMPTY: Self = Self {
        id: 0,
        peer_handle: [0; MAX_PEER_HANDLE_LEN],
        handle_len: 0,
        last_timestamp: 0,
        unread_count: 0,
        message_count: 0,
    };

    pub fn peer(&self) -> &[u8] {
        &self.peer_handle[..self.handle_len]
    }
}

/// Circular message ring buffer for a single thread, holding up to 16 messages.
#[derive(Debug, Clone, Copy)]
pub struct ThreadMessageRing {
    pub messages: [MessageRecord; MAX_MESSAGES_PER_THREAD],
    pub head: usize,
    pub count: usize,
}

impl ThreadMessageRing {
    pub const EMPTY: Self = Self {
        messages: [MessageRecord::EMPTY; MAX_MESSAGES_PER_THREAD],
        head: 0,
        count: 0,
    };

    /// Pushes a message into the circular ring. When full, overwrites the oldest message.
    pub fn push(&mut self, record: MessageRecord) {
        self.messages[self.head] = record;
        self.head = (self.head + 1) % MAX_MESSAGES_PER_THREAD;
        if self.count < MAX_MESSAGES_PER_THREAD {
            self.count += 1;
        }
    }

    /// Retrieves messages in chronological order (oldest to newest).
    /// Returns the number of messages copied into `out`.
    pub fn get_messages(&self, out: &mut [MessageRecord]) -> usize {
        let n = self.count.min(out.len());
        let oldest = if self.count < MAX_MESSAGES_PER_THREAD {
            0
        } else {
            self.head
        };
        for i in 0..n {
            let idx = (oldest + i) % MAX_MESSAGES_PER_THREAD;
            out[i] = self.messages[idx];
        }
        n
    }

    /// Returns the most recent message posted to this ring, if any.
    pub fn latest(&self) -> Option<&MessageRecord> {
        if self.count == 0 {
            None
        } else {
            let idx = (self.head + MAX_MESSAGES_PER_THREAD - 1) % MAX_MESSAGES_PER_THREAD;
            Some(&self.messages[idx])
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

/// Production ThreadLedger managing up to 8 conversation threads,
/// each backed by a 16-message circular buffer.
#[derive(Debug, Clone, Copy)]
pub struct ThreadLedger {
    pub threads: [ThreadRecord; MAX_THREADS],
    pub rings: [ThreadMessageRing; MAX_THREADS],
    pub thread_count: usize,
    pub next_thread_id: u32,
    pub next_msg_id: u32,
    pub virtual_clock: u64,
}

impl ThreadLedger {
    pub const EMPTY: Self = Self {
        threads: [ThreadRecord::EMPTY; MAX_THREADS],
        rings: [ThreadMessageRing::EMPTY; MAX_THREADS],
        thread_count: 0,
        next_thread_id: 1,
        next_msg_id: 1,
        virtual_clock: 100,
    };

    pub const fn new() -> Self {
        Self::EMPTY
    }

    /// Finds an existing thread by peer handle or creates a new thread record.
    /// Returns the thread ID on success, or None if the ledger is full (8 threads max).
    pub fn create_or_get_thread(&mut self, peer: &[u8]) -> Option<u32> {
        let plen = peer.len().min(MAX_PEER_HANDLE_LEN);
        for i in 0..self.thread_count {
            if self.threads[i].handle_len == plen && &self.threads[i].peer_handle[..plen] == &peer[..plen] {
                return Some(self.threads[i].id);
            }
        }
        if self.thread_count < MAX_THREADS {
            let idx = self.thread_count;
            let tid = self.next_thread_id;
            self.next_thread_id = self.next_thread_id.wrapping_add(1);
            if self.next_thread_id == 0 {
                self.next_thread_id = 1;
            }

            let mut tr = ThreadRecord::EMPTY;
            tr.id = tid;
            tr.peer_handle[..plen].copy_from_slice(&peer[..plen]);
            tr.handle_len = plen;
            tr.last_timestamp = self.virtual_clock;
            tr.unread_count = 0;
            tr.message_count = 0;

            self.threads[idx] = tr;
            self.rings[idx] = ThreadMessageRing::EMPTY;
            self.thread_count += 1;
            Some(tid)
        } else {
            None
        }
    }

    /// Posts a message to a thread with default delivery status (RefusedNoTransport for outgoing, Sent for incoming).
    pub fn post_message(&mut self, thread_id: u32, dir: MessageDirection, body: &[u8]) -> Option<u32> {
        let status = match dir {
            MessageDirection::Incoming => DeliveryStatus::Sent,
            MessageDirection::Outgoing => DeliveryStatus::RefusedNoTransport,
        };
        self.post_message_with_status(thread_id, dir, body, status)
    }

    /// Posts a message to a thread with explicit delivery status.
    pub fn post_message_with_status(
        &mut self,
        thread_id: u32,
        dir: MessageDirection,
        body: &[u8],
        status: DeliveryStatus,
    ) -> Option<u32> {
        let idx = self.threads[..self.thread_count].iter().position(|t| t.id == thread_id)?;
        let msg_id = self.next_msg_id;
        self.next_msg_id = self.next_msg_id.wrapping_add(1);
        if self.next_msg_id == 0 {
            self.next_msg_id = 1;
        }

        self.virtual_clock = self.virtual_clock.saturating_add(1);
        let blen = body.len().min(MAX_MESSAGE_BODY_LEN);
        let mut rec = MessageRecord::EMPTY;
        rec.id = msg_id;
        rec.thread_id = thread_id;
        rec.timestamp = self.virtual_clock;
        rec.direction = dir;
        rec.status = status;
        rec.body[..blen].copy_from_slice(&body[..blen]);
        rec.body_len = blen;

        self.rings[idx].push(rec);
        self.threads[idx].last_timestamp = rec.timestamp;
        self.threads[idx].message_count = self.threads[idx].message_count.saturating_add(1);
        if dir == MessageDirection::Incoming {
            self.threads[idx].unread_count = self.threads[idx].unread_count.saturating_add(1);
        }
        Some(msg_id)
    }

    /// Fills `out` with the thread's messages ordered chronologically (oldest to newest).
    /// Returns the count of messages retrieved.
    pub fn get_thread_messages(&self, thread_id: u32, out: &mut [MessageRecord]) -> usize {
        if let Some(idx) = self.threads[..self.thread_count].iter().position(|t| t.id == thread_id) {
            self.rings[idx].get_messages(out)
        } else {
            0
        }
    }

    /// Marks a thread as read by resetting its unread counter to 0.
    pub fn mark_thread_read(&mut self, thread_id: u32) -> bool {
        if let Some(idx) = self.threads[..self.thread_count].iter().position(|t| t.id == thread_id) {
            self.threads[idx].unread_count = 0;
            true
        } else {
            false
        }
    }

    /// Returns a reference to the ThreadRecord for the given thread_id.
    pub fn get_thread(&self, thread_id: u32) -> Option<&ThreadRecord> {
        self.threads[..self.thread_count].iter().find(|t| t.id == thread_id)
    }

    /// Returns a reference to the ThreadRecord by index.
    pub fn get_thread_by_idx(&self, idx: usize) -> Option<&ThreadRecord> {
        if idx < self.thread_count {
            Some(&self.threads[idx])
        } else {
            None
        }
    }

    /// Returns the total unread messages count across all active threads.
    pub fn unread_total(&self) -> u32 {
        let mut sum = 0u32;
        for i in 0..self.thread_count {
            sum += self.threads[i].unread_count as u32;
        }
        sum
    }

    /// Removes a thread and its message ring from the ledger.
    pub fn remove_thread(&mut self, thread_id: u32) -> bool {
        if let Some(idx) = self.threads[..self.thread_count].iter().position(|t| t.id == thread_id) {
            for i in idx..self.thread_count - 1 {
                self.threads[i] = self.threads[i + 1];
                self.rings[i] = self.rings[i + 1];
            }
            self.threads[self.thread_count - 1] = ThreadRecord::EMPTY;
            self.rings[self.thread_count - 1] = ThreadMessageRing::EMPTY;
            self.thread_count -= 1;
            true
        } else {
            false
        }
    }

    /// Returns the latest message for a thread.
    pub fn latest_message(&self, thread_id: u32) -> Option<&MessageRecord> {
        let idx = self.threads[..self.thread_count].iter().position(|t| t.id == thread_id)?;
        self.rings[idx].latest()
    }
}

/// Legacy conversation thread record retained for compatibility.
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
        assert_eq!(ledger.thread_count, 0);
        assert_eq!(ledger.unread_total(), 0);

        let t1_id = ledger.create_or_get_thread(b"+15550001").unwrap();
        ledger.post_message(t1_id, MessageDirection::Incoming, b"Hello there!");
        assert_eq!(ledger.unread_total(), 1);
        assert_eq!(ledger.get_thread(t1_id).unwrap().unread_count, 1);

        let t2_id = ledger.create_or_get_thread(b"+15550002").unwrap();
        ledger.post_message(t2_id, MessageDirection::Incoming, b"Meeting at 3pm");
        ledger.post_message(t2_id, MessageDirection::Incoming, b"See you soon");
        assert_eq!(ledger.get_thread(t2_id).unwrap().unread_count, 2);

        assert_eq!(ledger.thread_count, 2);
        assert_eq!(ledger.unread_total(), 3);

        // Re-finding existing returns same record id
        let t1_refound_id = ledger.create_or_get_thread(b"+15550001").unwrap();
        assert_eq!(t1_refound_id, t1_id);

        // Mark thread 2 read
        assert!(ledger.mark_thread_read(t2_id));
        assert_eq!(ledger.get_thread(t2_id).unwrap().unread_count, 0);
        assert_eq!(ledger.unread_total(), 1);
    }

    #[test]
    fn test_thread_ledger_circular_overflow() {
        let mut ledger = ThreadLedger::new();
        let tid = ledger.create_or_get_thread(b"+19998887777").unwrap();

        // Post 20 messages (ring buffer capacity is 16)
        for i in 0..20u8 {
            let body = [b'A' + i; 10];
            ledger.post_message(tid, MessageDirection::Outgoing, &body);
        }

        let mut retrieved = [MessageRecord::EMPTY; 20];
        let count = ledger.get_thread_messages(tid, &mut retrieved);
        assert_eq!(count, 16);

        // The oldest 4 messages (0..3, 'A'..'D') must have been dropped.
        // First returned message must be 'E' (i=4)
        assert_eq!(retrieved[0].body[0], b'A' + 4);
        // Last returned message must be 'T' (i=19)
        assert_eq!(retrieved[15].body[0], b'A' + 19);

        // Thread message_count tracks total posted (20)
        assert_eq!(ledger.get_thread(tid).unwrap().message_count, 20);
    }

    #[test]
    fn test_thread_ledger_max_threads_capacity() {
        let mut ledger = ThreadLedger::new();
        for i in 0..8 {
            let handle = [b'0' + i as u8; 4];
            assert!(ledger.create_or_get_thread(&handle).is_some());
        }
        assert_eq!(ledger.thread_count, 8);
        // 9th thread must be refused
        assert!(ledger.create_or_get_thread(b"9999").is_none());
    }

    #[test]
    fn test_delivery_status_truthful_refused_no_transport() {
        let mut ledger = ThreadLedger::new();
        let tid = ledger.create_or_get_thread(b"+15550099").unwrap();
        ledger.post_message(tid, MessageDirection::Outgoing, b"Hello from isolated sandbox");

        let mut msgs = [MessageRecord::EMPTY; 4];
        let n = ledger.get_thread_messages(tid, &mut msgs);
        assert_eq!(n, 1);
        assert_eq!(msgs[0].direction, MessageDirection::Outgoing);
        assert_eq!(msgs[0].status, DeliveryStatus::RefusedNoTransport);
        assert_eq!(msgs[0].status.as_str(), "Refused - No Transport");
    }
}
