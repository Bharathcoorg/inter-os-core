#![no_std]

pub use interos_std as std;

/// The app's view of the platform's design system.
pub mod theme;

pub const CONTACT_NAME_MAX: usize = 18;
pub const CONTACT_NUM_MAX: usize = 16;
pub const CONTACT_RECORD_BYTES: usize = 1 + 1 + CONTACT_NAME_MAX + CONTACT_NUM_MAX; // 36 bytes

/// Maximum durable contacts. Previously capped at 8 due to unescaped 0x0A ('\n')
/// values in the length/id fields failing settings storage checks.
pub const MAX_CONTACTS: usize = 12;

pub const STORE_BLOB_BYTES: usize = 2 + MAX_CONTACTS * CONTACT_RECORD_BYTES;
pub const STORE_VERSION: u8 = 1;

pub const ESCAPE_BYTE: u8 = 0xFE;
pub const ESCAPED_NEWLINE: u8 = 0x8A;
pub const ESCAPED_ESCAPE: u8 = 0xEE;

/// What the modem reported about the call.
///
/// WHY the modem's own answer and not the app's guess: `telephony_dial`
/// returns as soon as the modem has accepted the request, and the modem
/// enters its own `Dialing` state. The app used to turn that acceptance
/// into a connected call, so the screen claimed a call that had never
/// connected.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModemCallState {
    /// The modem reported no call.
    Idle,
    /// The modem is placing the call.
    Dialing,
    /// The modem reports the call as connected.
    Connected,
    /// The modem reports the call as ended.
    Ended,
    /// The modem reported a state this build does not know.
    Other,
}

/// Reads the modem's own call state out of the telemetry payload.
///
/// WHY a parser and not a guess: the kernel writes the modem's call state
/// into the payload as the modem's own value, and the modem is the only
/// thing that knows whether a call connected. The token is the variant
/// name, so the word before the `{` is the state.
pub fn parse_modem_call_state(payload: &[u8]) -> ModemCallState {
    const KEY: &[u8] = b"\"call_state\":\"";
    let mut i = 0usize;
    while i + KEY.len() <= payload.len() {
        if &payload[i..i + KEY.len()] == KEY {
            let rest = &payload[i + KEY.len()..];
            let end = rest.iter().position(|&b| b == b'"').unwrap_or(rest.len());
            let token = &rest[..end];
            if token.starts_with(b"Idle") {
                return ModemCallState::Idle;
            }
            if token.starts_with(b"Dialing") {
                return ModemCallState::Dialing;
            }
            if token.starts_with(b"Active") {
                return ModemCallState::Connected;
            }
            if token.starts_with(b"Disconnected") {
                return ModemCallState::Ended;
            }
            return ModemCallState::Other;
        }
        i += 1;
    }
    ModemCallState::Other
}

/// Escapes any 0x0A ('\n') or 0xFE bytes in a raw store blob so the serialized
/// settings value never contains a newline.
pub fn escape_store_blob(raw: &[u8], out: &mut [u8]) -> Result<usize, ()> {
    let mut pos = 0usize;
    for &b in raw {
        if b == b'\n' {
            if pos + 2 > out.len() {
                return Err(());
            }
            out[pos] = ESCAPE_BYTE;
            out[pos + 1] = ESCAPED_NEWLINE;
            pos += 2;
        } else if b == ESCAPE_BYTE {
            if pos + 2 > out.len() {
                return Err(());
            }
            out[pos] = ESCAPE_BYTE;
            out[pos + 1] = ESCAPED_ESCAPE;
            pos += 2;
        } else {
            if pos + 1 > out.len() {
                return Err(());
            }
            out[pos] = b;
            pos += 1;
        }
    }
    Ok(pos)
}

/// Unescapes a store blob read from the settings store. Handles both escaped
/// modern payloads and legacy unescaped blobs transparently.
pub fn unescape_store_blob(encoded: &[u8], out: &mut [u8]) -> Result<usize, ()> {
    let mut in_pos = 0usize;
    let mut out_pos = 0usize;
    while in_pos < encoded.len() {
        let b = encoded[in_pos];
        if b == ESCAPE_BYTE {
            in_pos += 1;
            if in_pos >= encoded.len() {
                return Err(());
            }
            match encoded[in_pos] {
                ESCAPED_NEWLINE => {
                    if out_pos >= out.len() {
                        return Err(());
                    }
                    out[out_pos] = b'\n';
                    out_pos += 1;
                }
                ESCAPED_ESCAPE => {
                    if out_pos >= out.len() {
                        return Err(());
                    }
                    out[out_pos] = ESCAPE_BYTE;
                    out_pos += 1;
                }
                _ => return Err(()),
            }
            in_pos += 1;
        } else {
            if out_pos >= out.len() {
                return Err(());
            }
            out[out_pos] = b;
            out_pos += 1;
            in_pos += 1;
        }
    }
    Ok(out_pos)
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct ContactRecord {
    pub id: u8,
    pub flags: u8,
    pub name: [u8; CONTACT_NAME_MAX],
    pub number: [u8; CONTACT_NUM_MAX],
}

impl ContactRecord {
    pub const EMPTY: ContactRecord = ContactRecord {
        id: 0,
        flags: 0,
        name: [0; CONTACT_NAME_MAX],
        number: [0; CONTACT_NUM_MAX],
    };

    pub const FLAG_FAVOURITE: u8 = 1 << 0;

    pub fn from_parts(id: u8, name: &[u8], number: &[u8], favourite: bool) -> Self {
        let mut rec = ContactRecord::EMPTY;
        rec.id = id;
        rec.set_favourite(favourite);
        let n = name.len().min(CONTACT_NAME_MAX);
        rec.name[..n].copy_from_slice(&name[..n]);
        let m = number.len().min(CONTACT_NUM_MAX);
        rec.number[..m].copy_from_slice(&number[..m]);
        rec
    }

    pub fn valid(&self) -> bool {
        (1..=MAX_CONTACTS as u8).contains(&self.id)
            && self.flags & !Self::FLAG_FAVOURITE == 0
            && self.name_bytes().iter().all(|b| (0x20..=0x7E).contains(b))
            && self
                .number
                .iter()
                .take(self.number_len())
                .all(|b| b.is_ascii_digit() || *b == b'+' || *b == b'*' || *b == b'#')
    }

    pub fn name_len(&self) -> usize {
        self.name.iter().position(|&b| b == 0).unwrap_or(CONTACT_NAME_MAX)
    }

    pub fn number_len(&self) -> usize {
        self.number
            .iter()
            .position(|&b| b == 0)
            .unwrap_or(CONTACT_NUM_MAX)
    }

    pub fn name_bytes(&self) -> &[u8] {
        &self.name[..self.name_len()]
    }

    pub fn number_bytes(&self) -> &[u8] {
        &self.number[..self.number_len()]
    }

    pub fn is_favourite(&self) -> bool {
        self.flags & Self::FLAG_FAVOURITE != 0
    }

    pub fn set_favourite(&mut self, on: bool) {
        self.flags = (self.flags & !Self::FLAG_FAVOURITE) | (on as u8) * Self::FLAG_FAVOURITE;
    }

    pub fn encode(&self, out: &mut [u8]) {
        out[0] = self.id;
        out[1] = self.flags;
        out[2..2 + CONTACT_NAME_MAX].copy_from_slice(&self.name);
        out[2 + CONTACT_NAME_MAX..2 + CONTACT_NAME_MAX + CONTACT_NUM_MAX].copy_from_slice(&self.number);
    }

    pub fn decode(bytes: &[u8]) -> Option<ContactRecord> {
        if bytes.len() < CONTACT_RECORD_BYTES {
            return None;
        }
        let mut rec = ContactRecord::EMPTY;
        rec.id = bytes[0];
        rec.flags = bytes[1];
        rec.name.copy_from_slice(&bytes[2..2 + CONTACT_NAME_MAX]);
        let num_at = 2 + CONTACT_NAME_MAX;
        rec.number
            .copy_from_slice(&bytes[num_at..num_at + CONTACT_NUM_MAX]);
        if rec.valid() {
            Some(rec)
        } else {
            None
        }
    }
}

/// Standard ITU-T Q.23 Dual-Tone Multi-Frequency (DTMF) acoustic frequency pairs (Hz).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DtmfPair {
    pub low_freq_hz: u16,
    pub high_freq_hz: u16,
}

impl DtmfPair {
    pub const fn from_key(key: char) -> Option<Self> {
        match key {
            '1' => Some(Self { low_freq_hz: 697, high_freq_hz: 1209 }),
            '2' => Some(Self { low_freq_hz: 697, high_freq_hz: 1336 }),
            '3' => Some(Self { low_freq_hz: 697, high_freq_hz: 1477 }),
            '4' => Some(Self { low_freq_hz: 770, high_freq_hz: 1209 }),
            '5' => Some(Self { low_freq_hz: 770, high_freq_hz: 1336 }),
            '6' => Some(Self { low_freq_hz: 770, high_freq_hz: 1477 }),
            '7' => Some(Self { low_freq_hz: 852, high_freq_hz: 1209 }),
            '8' => Some(Self { low_freq_hz: 852, high_freq_hz: 1336 }),
            '9' => Some(Self { low_freq_hz: 852, high_freq_hz: 1477 }),
            '*' => Some(Self { low_freq_hz: 941, high_freq_hz: 1209 }),
            '0' => Some(Self { low_freq_hz: 941, high_freq_hz: 1336 }),
            '#' => Some(Self { low_freq_hz: 941, high_freq_hz: 1477 }),
            _ => None,
        }
    }
}

/// Call type classification for call records.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CallType {
    Outgoing = 0,
    Incoming = 1,
    Missed = 2,
}

pub const MAX_CALL_LOGS: usize = 16;

/// Bounded call history record.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CallLogEntry {
    pub number: [u8; CONTACT_NUM_MAX],
    pub number_len: usize,
    pub call_type: CallType,
    pub duration_secs: u32,
    pub timestamp_epoch_secs: u64,
}

impl CallLogEntry {
    pub const EMPTY: Self = Self {
        number: [0; CONTACT_NUM_MAX],
        number_len: 0,
        call_type: CallType::Outgoing,
        duration_secs: 0,
        timestamp_epoch_secs: 0,
    };

    pub fn new(number_bytes: &[u8], call_type: CallType, duration_secs: u32, timestamp: u64) -> Self {
        let mut entry = Self::EMPTY;
        let n = number_bytes.len().min(CONTACT_NUM_MAX);
        entry.number[..n].copy_from_slice(&number_bytes[..n]);
        entry.number_len = n;
        entry.call_type = call_type;
        entry.duration_secs = duration_secs;
        entry.timestamp_epoch_secs = timestamp;
        entry
    }
}

/// Bounded, zero-heap call history circular ledger.
#[derive(Debug, Clone, Copy)]
pub struct CallHistoryLedger {
    pub entries: [CallLogEntry; MAX_CALL_LOGS],
    pub count: usize,
    pub head: usize,
}

impl CallHistoryLedger {
    pub const fn new() -> Self {
        Self {
            entries: [CallLogEntry::EMPTY; MAX_CALL_LOGS],
            count: 0,
            head: 0,
        }
    }

    pub fn push(&mut self, entry: CallLogEntry) {
        self.entries[self.head] = entry;
        self.head = (self.head + 1) % MAX_CALL_LOGS;
        if self.count < MAX_CALL_LOGS {
            self.count += 1;
        }
    }

    pub fn get(&self, idx: usize) -> Option<&CallLogEntry> {
        if idx >= self.count {
            return None;
        }
        // Most recent first: index 0 is (head - 1 + MAX_CALL_LOGS) % MAX_CALL_LOGS
        let slot = (self.head + MAX_CALL_LOGS - 1 - idx) % MAX_CALL_LOGS;
        Some(&self.entries[slot])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escape_and_unescape_roundtrip_with_0x0a() {
        let raw = [STORE_VERSION, 10, 0x0A, 42, 0xFE, 99];
        let mut enc = [0u8; 32];
        let enc_len = escape_store_blob(&raw, &mut enc).expect("escape");
        // Must contain NO 0x0A bytes
        assert!(!enc[..enc_len].iter().any(|&b| b == b'\n'));

        let mut dec = [0u8; 32];
        let dec_len = unescape_store_blob(&enc[..enc_len], &mut dec).expect("unescape");
        assert_eq!(&dec[..dec_len], &raw[..]);
    }

    #[test]
    fn contact_store_10_contacts_ceiling_lifted() {
        // Create 10 valid contact records
        let mut raw = [0u8; STORE_BLOB_BYTES];
        raw[0] = STORE_VERSION;
        raw[1] = 10; // count = 10 is 0x0A!

        for i in 1..=10 {
            let name = b"Contact";
            let num = b"+123456789";
            let rec = ContactRecord::from_parts(i as u8, name, num, i % 2 == 0);
            assert!(rec.valid());
            let offset = 2 + (i - 1) * CONTACT_RECORD_BYTES;
            rec.encode(&mut raw[offset..offset + CONTACT_RECORD_BYTES]);
        }
        let raw_len = 2 + 10 * CONTACT_RECORD_BYTES;

        // Escaped output must have no 0x0A
        let mut enc = [0u8; STORE_BLOB_BYTES + 32];
        let enc_len = escape_store_blob(&raw[..raw_len], &mut enc).expect("escape");
        assert!(!enc[..enc_len].iter().any(|&b| b == b'\n'));

        // Unescaped matches raw
        let mut dec = [0u8; STORE_BLOB_BYTES];
        let dec_len = unescape_store_blob(&enc[..enc_len], &mut dec).expect("unescape");
        assert_eq!(dec_len, raw_len);
        assert_eq!(&dec[..dec_len], &raw[..raw_len]);

        // Verify the 10th record (id = 10, which was 0x0A) decodes properly
        let offset_10 = 2 + 9 * CONTACT_RECORD_BYTES;
        let rec10 = ContactRecord::decode(&dec[offset_10..offset_10 + CONTACT_RECORD_BYTES]).expect("rec 10 decode");
        assert_eq!(rec10.id, 10);
    }

    #[test]
    fn legacy_unescaped_blob_decodes_identically() {
        let legacy = [STORE_VERSION, 2, 1, 0, b'A', 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, b'1', 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0];
        let mut dec = [0u8; 64];
        let dec_len = unescape_store_blob(&legacy, &mut dec).expect("unescape legacy");
        assert_eq!(&dec[..dec_len], &legacy[..]);
    }

    #[test]
    fn test_dtmf_frequencies() {
        assert_eq!(DtmfPair::from_key('1'), Some(DtmfPair { low_freq_hz: 697, high_freq_hz: 1209 }));
        assert_eq!(DtmfPair::from_key('5'), Some(DtmfPair { low_freq_hz: 770, high_freq_hz: 1336 }));
        assert_eq!(DtmfPair::from_key('#'), Some(DtmfPair { low_freq_hz: 941, high_freq_hz: 1477 }));
        assert_eq!(DtmfPair::from_key('X'), None);
    }

    #[test]
    fn the_modem_call_state_comes_from_the_modem_and_not_from_the_acceptance() {
        // WHY this test exists: `telephony_dial` returns as soon as the modem
        // has accepted the request, and the modem enters its own `Dialing`
        // state. The app used to report that acceptance as a connected call,
        // so the screen claimed a call that had never connected.
        let dialing = br#"{"present":true,"operator":"none","rssi":99,"bars":0,"dbm":-110,"state":"PowerDown","call_state":"Dialing { destination: \"555\", start_ticks: 42 }","pdp_active":false,"ip":0.0.0.0}"#;
        assert_eq!(parse_modem_call_state(dialing), ModemCallState::Dialing);

        let active = br#"{"present":true,"call_state":"Active { number: \"555\", start_ticks: 42 }"}"#;
        assert_eq!(parse_modem_call_state(active), ModemCallState::Connected);

        let idle = br#"{"present":true,"call_state":"Idle"}"#;
        assert_eq!(parse_modem_call_state(idle), ModemCallState::Idle);

        let ended = br#"{"present":true,"call_state":"Disconnected"}"#;
        assert_eq!(parse_modem_call_state(ended), ModemCallState::Ended);

        // "Alerting" is the modem ringing out, which is not a connection.
        let alerting = br#"{"present":true,"call_state":"Alerting { destination: \"555\", start_ticks: 42 }"}"#;
        assert_eq!(parse_modem_call_state(alerting), ModemCallState::Other);

        // A payload with no call state at all is not a call.
        assert_eq!(
            parse_modem_call_state(br#"{"present":false,"bars":0}"#),
            ModemCallState::Other
        );
    }

    #[test]
    fn test_call_history_ledger() {        let mut ledger = CallHistoryLedger::new();
        assert_eq!(ledger.count, 0);

        ledger.push(CallLogEntry::new(b"+15551234", CallType::Outgoing, 45, 1000));
        assert_eq!(ledger.count, 1);
        let first = ledger.get(0).unwrap();
        assert_eq!(&first.number[..first.number_len], b"+15551234");
        assert_eq!(first.call_type, CallType::Outgoing);
        assert_eq!(first.duration_secs, 45);

        // Push 20 calls to verify circular overwrite at capacity 16
        for i in 2..=20 {
            ledger.push(CallLogEntry::new(b"+15550000", CallType::Incoming, i as u32, 1000 + i as u64));
        }
        assert_eq!(ledger.count, MAX_CALL_LOGS);
        // Latest pushed call has duration_secs = 20
        assert_eq!(ledger.get(0).unwrap().duration_secs, 20);
        assert_eq!(ledger.get(15).unwrap().duration_secs, 5);
        assert_eq!(ledger.get(16), None);
    }
}

