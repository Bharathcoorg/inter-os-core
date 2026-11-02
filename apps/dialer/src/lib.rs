#![no_std]

pub use interos_std as std;

use interos_std::design::scroll::ScrollState;

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
///
/// WHY there is no duration field: the platform's telephony telemetry carries
/// the modem's call state and its signal readings, and no duration. A record
/// that carried one would carry a number this app invented, and a call
/// history that shows an invented duration is a history that cannot be
/// believed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CallLogEntry {
    pub number: [u8; CONTACT_NUM_MAX],
    pub number_len: usize,
    pub call_type: CallType,
    pub timestamp_epoch_secs: u64,
}

impl CallLogEntry {
    pub const EMPTY: Self = Self {
        number: [0; CONTACT_NUM_MAX],
        number_len: 0,
        call_type: CallType::Outgoing,
        timestamp_epoch_secs: 0,
    };

    pub fn new(number_bytes: &[u8], call_type: CallType, timestamp: u64) -> Self {
        let mut entry = Self::EMPTY;
        let n = number_bytes.len().min(CONTACT_NUM_MAX);
        entry.number[..n].copy_from_slice(&number_bytes[..n]);
        entry.number_len = n;
        entry.call_type = call_type;
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

// ---------------------------------------------------------------------------
// The lists the dialer scrolls.
//
// WHY the extents live here and not in the renderer: a list that is taller
// than its window has to be scrollable, and the height of a list and the
// height of the window it is shown in are one fact. Putting them in the
// model means the test can state the fact without drawing a pixel.
// ---------------------------------------------------------------------------

/// The height of one row in the recents ledger.
pub const RECENTS_ROW_H: usize = 96;

/// The top of the recents list.
pub const RECENTS_Y0: usize = 424;

/// The bottom of the recents list's window.
pub const RECENTS_VIEWPORT_Y1: usize = 1140;

/// The height of one row in the contact list.
pub const CONTACT_ROW_H: usize = 66;

/// The top of the contact list.
pub const CONTACTS_LIST_Y0: usize = 410;

/// The bottom of the contact list's window; the notice band starts here.
pub const CONTACTS_VIEWPORT_Y1: usize = 938;

// ---------------------------------------------------------------------------
// The tab row the dialer draws across the top of every section.
//
// WHY the geometry lives here and not only in the renderer: the row's width is
// the sum of the columns the painter seats, and the only honest way to know a
// tab's label fits its column -- and the row fits the surface -- is to add the
// real columns back. If those numbers live in the painter (`src/main.rs`, a
// `test = false` bin no host test can reach), the gate re-states them as
// literals and the gate and the row can disagree without either failing to
// compile. The painter reads these back, so the row and the gate are one fact.
// ---------------------------------------------------------------------------
pub mod tab_row {
    use crate::theme::type_scale;
    use interos_std::design::font;
    use interos_std::design::outline_text;
    use interos_std::design::viewport;

    /// The rung the tab labels are drawn at: the body rung. WHY not lowered: a
    /// tab that fits only by shrinking its type is a tab the gate passed by
    /// lying about the label, not by the label being short.
    pub const SCALE: usize = type_scale::BODY;

    /// How many tabs the row lays out -- all four sections are live.
    pub const COUNT: usize = 4;

    /// The tab labels, in section order, exactly as the painter draws them.
    pub const LABELS: [&[u8]; COUNT] = [b"Dial", b"Recents", b"Contacts", b"Settings"];

    /// The top of the band the tabs sit in.
    pub const BAND_Y0: usize = 120;

    /// The height of the band the tabs sit in.
    pub const BAND_H: usize = 44;

    /// The width of one tab column.
    pub const COL_W: usize = 147;

    /// The x the first column starts on: the display's margin.
    pub const START_X: usize = 48;

    /// The gap between two tab columns.
    pub const GAP: usize = 12;

    /// The width of the surface the row is laid out on. The painter draws into a
    /// surface of this width, so a row whose last edge passes it is off-screen.
    pub const SURFACE_W: usize = viewport::LOGICAL_WIDTH;

    /// The left edge of the column at `index`, derived forward from the margin,
    /// the column width and the gap -- never a literal that can drift.
    pub const fn col_x0(index: usize) -> usize {
        START_X + index * (COL_W + GAP)
    }

    /// The right edge (one-past) of the column at `index`.
    pub const fn col_x1(index: usize) -> usize {
        col_x0(index) + COL_W
    }

    /// The right edge (one-past the last column) the whole row reaches.
    pub const fn row_right_edge() -> usize {
        START_X + COUNT * COL_W + (COUNT - 1) * GAP
    }

    /// The ascent-plus-descent the rung occupies, i.e. the vertical band a tab's
    /// line box claims. WHY not the line box the painter centres with: the gate
    /// is asked whether the rung fits the band the app draws it into, and that
    /// extent is the face's own ascent plus descent at the rung's em.
    pub const fn rung_band_h() -> usize {
        // `row_band(0, scale).1` is the band's lower row for a run based at 0,
        // which is exactly the ascent-plus-descent height the rung occupies.
        outline_text::row_band(0, SCALE).1
    }

    /// Whether one tab label fits a column `col_w` wide at the rung it is drawn at.
    pub fn label_fits_column(label: &[u8], col_w: usize) -> bool {
        font::measure(label, SCALE) <= col_w
    }

    /// Whether every tab label fits the column the painter gives it.
    pub fn each_label_fits_column() -> bool {
        let mut i = 0;
        while i < COUNT {
            if !label_fits_column(LABELS[i], COL_W) {
                return false;
            }
            i += 1;
        }
        true
    }

    /// Whether every tab label fits a column `col_w` wide -- the gate asked with
    /// the container narrowed, to prove it tracks the column and not a literal.
    pub fn each_label_fits_column_at(col_w: usize) -> bool {
        let mut i = 0;
        while i < COUNT {
            if !label_fits_column(LABELS[i], col_w) {
                return false;
            }
            i += 1;
        }
        true
    }

    /// Whether the row ends on or before a surface `surface_w` wide.
    pub const fn row_fits(surface_w: usize) -> bool {
        row_right_edge() <= surface_w
    }
}

/// How tall a scrollable list is, and how much of it is on screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScrollExtents {
    /// The height of the whole list.
    pub content_h: usize,
    /// The height of the part of the list that is on screen.
    pub viewport_h: usize,
}

impl ScrollExtents {
    /// Whether the list has anything below the fold.
    ///
    /// WHY this is asked of the extents and not of the scroll state: a list
    /// that fits does not need an indicator, and an indicator on a list that
    /// fits is a control that says there is more when there is not.
    pub const fn scrolls(self) -> bool {
        self.content_h > self.viewport_h
    }

    /// The scroll state these extents describe.
    pub const fn state(self) -> ScrollState {
        ScrollState::new(self.content_h, self.viewport_h, 0)
    }
}

/// The extents of the recents ledger.
///
/// WHY the ledger's window is the whole content band: the recents section
/// owns that band, and a ledger that could only be scrolled through a
/// window smaller than its own would leave the rows below the fold
/// unreachable in the one section that has nothing else to show.
pub const fn recents_scroll_extents(entries: usize) -> ScrollExtents {
    ScrollExtents {
        content_h: entries * RECENTS_ROW_H,
        viewport_h: RECENTS_VIEWPORT_Y1 - RECENTS_Y0,
    }
}

/// The extents of the contact list.
pub const fn contacts_scroll_extents(rows: usize) -> ScrollExtents {
    ScrollExtents {
        content_h: rows * CONTACT_ROW_H,
        viewport_h: CONTACTS_VIEWPORT_Y1 - CONTACTS_LIST_Y0,
    }
}

// ---------------------------------------------------------------------------
// The ringtone picker.
// ---------------------------------------------------------------------------

/// One audio document the picker can offer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RingtoneChoice {
    /// The document's file name, which is what the row shows.
    pub name: [u8; 40],
    /// How many bytes of the name are used.
    pub name_len: usize,
    /// The document's path, which is what the ringtone slot holds.
    pub path: [u8; 96],
    /// How many bytes of the path are used.
    pub path_len: usize,
}

impl RingtoneChoice {
    /// An empty choice.
    pub const EMPTY: Self = Self {
        name: [0; 40],
        name_len: 0,
        path: [0; 96],
        path_len: 0,
    };
}

/// Reads the audio documents out of the platform's index answer.
///
/// WHY the picker is built from the index and not from a list this app keeps:
/// the ringtone slot names a document on the device, and the documents on the
/// device are the platform's. A picker over the app's own list would offer
/// files the platform does not hold.
///
/// WHY only audio documents: the slot is a ringtone, and a ringtone that is
/// a photograph is not a ringtone.
pub fn parse_audio_documents(payload: &[u8], out: &mut [RingtoneChoice]) -> usize {
    let mut count = 0usize;
    for line in payload.split(|byte| *byte == b'\n') {
        if count >= out.len() {
            break;
        }
        if line.is_empty() {
            continue;
        }
        let mut fields = line.split(|byte| *byte == b'|');
        let _id = fields.next();
        let Some(path) = fields.next() else { continue };
        let Some(mime) = fields.next() else { continue };
        if !mime.starts_with(b"audio/") {
            continue;
        }
        let n = path.len().min(out[count].path.len());
        out[count].path[..n].copy_from_slice(&path[..n]);
        out[count].path_len = n;
        let name = match path.iter().rposition(|byte| *byte == b'/') {
            Some(index) => &path[index + 1..],
            None => path,
        };
        let m = name.len().min(out[count].name.len());
        out[count].name[..m].copy_from_slice(&name[..m]);
        out[count].name_len = m;
        count += 1;
    }
    count
}

// ---------------------------------------------------------------------------
// The keypad-tone preference.
// ---------------------------------------------------------------------------

/// The settings key the keypad-tone preference is stored under.
pub const DTMF_ENABLED_KEY: &str = "dialer.dtmf.enabled";

/// Whether the keypad sounds while a digit is entered.
///
/// WHY the default is on: a phone whose keypad is silent because the
/// preference was never written is a phone the user cannot use without
/// looking at the screen, and the platform's own answer to "the user has not
/// chosen" is the ordinary behaviour.
pub fn dtmf_enabled() -> bool {
    let mut buf = [0u8; 16];
    match std::settings_get(DTMF_ENABLED_KEY, &mut buf) {
        Ok(n) if n > 0 => buf[0] != b'0',
        _ => true,
    }
}

/// Stores the keypad-tone preference.
pub fn set_dtmf_enabled(enabled: bool) -> Result<(), i64> {
    std::settings_set(DTMF_ENABLED_KEY, if enabled { b"1" } else { b"0" })
}

/// Writes a number into `out` as decimal and returns how many bytes it wrote.
///
/// WHY this is here and not only in the binary: the app has to put a
/// platform's error code into a log line, and a log line that says "code -"
/// without the number is not a report of what the platform answered.
pub fn put_decimal(mut value: u32, out: &mut [u8]) -> usize {
    if value == 0 {
        if out.is_empty() {
            return 0;
        }
        out[0] = b'0';
        return 1;
    }
    let mut digits = [0u8; 10];
    let mut count = 0usize;
    while value > 0 && count < digits.len() {
        digits[count] = b'0' + (value % 10) as u8;
        value /= 10;
        count += 1;
    }
    let mut written = 0usize;
    let mut index = count;
    while index > 0 {
        index -= 1;
        if written >= out.len() {
            break;
        }
        out[written] = digits[index];
        written += 1;
    }
    written
}

// ---------------------------------------------------------------------------
// The settings surface.
// ---------------------------------------------------------------------------

/// The settings the dialer's settings surface offers.
///
/// WHY the surface names the settings it cannot offer: the dialer's design
/// document asks for a default-SIM choice, and the platform exposes one SIM
/// profile and no subscription-selection call. A row that says so is an
/// answer; a row that offers a choice which cannot be made is a lie.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettingsRow {
    /// The document the device plays for an incoming call.
    Ringtone,
    /// Whether a pressed key sounds.
    KeypadTone,
    /// Which SIM the phone calls on.
    DefaultSim,
    /// What the platform reports about the length of a call.
    CallDuration,
}

impl SettingsRow {
    /// The rows, in the order the surface shows them.
    pub const ORDER: [SettingsRow; 4] = [
        SettingsRow::Ringtone,
        SettingsRow::KeypadTone,
        SettingsRow::DefaultSim,
        SettingsRow::CallDuration,
    ];

    /// The row's title.
    pub const fn title(self) -> &'static [u8] {
        match self {
            SettingsRow::Ringtone => b"Ringtone",
            SettingsRow::KeypadTone => b"Keypad tone",
            SettingsRow::DefaultSim => b"Default SIM",
            SettingsRow::CallDuration => b"Call duration",
        }
    }

    /// The row's note: what the platform actually supports.
    pub const fn note(self) -> &'static [u8] {
        match self {
            SettingsRow::Ringtone => b"The document the device plays for an incoming call",
            SettingsRow::KeypadTone => b"Whether a pressed key sounds",
            SettingsRow::DefaultSim => {
                b"Not supported: the platform exposes one SIM and no way to choose"
            }
            SettingsRow::CallDuration => b"The platform's telephony telemetry carries no duration",
        }
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

        ledger.push(CallLogEntry::new(b"+15551234", CallType::Outgoing, 1000));
        assert_eq!(ledger.count, 1);
        let first = ledger.get(0).unwrap();
        assert_eq!(&first.number[..first.number_len], b"+15551234");
        assert_eq!(first.call_type, CallType::Outgoing);

        // Push 20 calls to verify circular overwrite at capacity 16
        for i in 2..=20 {
            ledger.push(CallLogEntry::new(b"+15550000", CallType::Incoming, 1000 + i as u64));
        }
        assert_eq!(ledger.count, MAX_CALL_LOGS);
        // The newest call is first and the oldest is still the sixteenth.
        assert_eq!(ledger.get(0).unwrap().timestamp_epoch_secs, 1020);
        assert_eq!(ledger.get(15).unwrap().timestamp_epoch_secs, 1005);
        assert_eq!(ledger.get(16), None);
    }

    #[test]
    fn the_recents_list_scrolls_when_there_is_more_than_it_can_show() {
        // A ledger that fits its window needs no indicator, and one that does
        // not fit has to be scrollable, or the rows below the fold are
        // unreachable.
        let fits = recents_scroll_extents(3);
        assert!(!fits.scrolls());
        assert_eq!(fits.content_h, 3 * RECENTS_ROW_H);
        assert_eq!(fits.viewport_h, RECENTS_VIEWPORT_Y1 - RECENTS_Y0);

        let overflows = recents_scroll_extents(16);
        assert!(overflows.scrolls());
        assert!(overflows.state().max_offset() > 0);
    }

    #[test]
    fn the_contact_list_scrolls_when_there_is_more_than_it_can_show() {
        let fits = contacts_scroll_extents(4);
        assert!(!fits.scrolls());
        assert_eq!(fits.content_h, 4 * CONTACT_ROW_H);
        assert_eq!(fits.viewport_h, CONTACTS_VIEWPORT_Y1 - CONTACTS_LIST_Y0);

        // Twelve saved and twelve unsaved contacts are more than the window
        // holds, and every one of them has to be reachable.
        let overflows = contacts_scroll_extents(24);
        assert!(overflows.scrolls());
        assert!(overflows.state().max_offset() > 0);
    }

    #[test]
    fn the_ringtone_picker_offers_only_the_audio_documents_the_platform_holds() {
        let payload = b"1|/storage/audio/chime.pcm|audio/x-wav|bytes|4096|7\n2|/storage/photos/shot-1.ppm|image/x-portable-pixmap|bytes|2048|9\n3|/storage/audio/alarm.pcm|audio/x-wav|bytes|1024|11\n";
        let mut choices = [RingtoneChoice::EMPTY; 4];
        let n = parse_audio_documents(payload, &mut choices);
        // The photograph is not a ringtone.
        assert_eq!(n, 2);
        assert_eq!(&choices[0].path[..choices[0].path_len], b"/storage/audio/chime.pcm");
        assert_eq!(&choices[0].name[..choices[0].name_len], b"chime.pcm");
        assert_eq!(&choices[1].name[..choices[1].name_len], b"alarm.pcm");
    }

    #[test]
    fn the_ringtone_picker_survives_an_index_answer_with_nothing_in_it() {
        let mut choices = [RingtoneChoice::EMPTY; 2];
        assert_eq!(parse_audio_documents(b"", &mut choices), 0);
        assert_eq!(parse_audio_documents(b"garbage\n", &mut choices), 0);
    }

    #[test]
    fn the_settings_surface_says_which_choices_the_platform_does_not_offer() {
        // The default-SIM row has to name the limit rather than offer a
        // choice the platform cannot make.
        let note = core::str::from_utf8(SettingsRow::DefaultSim.note()).unwrap();
        assert!(note.contains("Not supported"));
        assert!(note.contains("one SIM"));
        // The duration row has to say the platform reports none.
        let note = core::str::from_utf8(SettingsRow::CallDuration.note()).unwrap();
        assert!(note.contains("no duration"));
        // Every row has a title and a note.
        for row in SettingsRow::ORDER {
            assert!(!row.title().is_empty());
            assert!(!row.note().is_empty());
        }
    }

    #[test]
    fn test_dialer_ui_strings_fit_horizontal_bands() {
        use interos_std::design::font;

        const DISPLAY_W: usize = 672;

        // 1. The tab row is measured against what the app actually draws: the
        //    real labels, at the real column, on the real rung, and the whole row
        //    laid end to end against the real surface -- not a stale 3-label set
        //    held to a 208px pin that the painter never uses.
        for t in tab_row::LABELS.iter() {
            let w = font::measure(t, tab_row::SCALE);
            assert!(
                w <= tab_row::COL_W,
                "Tab label {:?} width {} exceeds its {}px column",
                core::str::from_utf8(t).unwrap(),
                w,
                tab_row::COL_W
            );
        }
        assert!(
            tab_row::each_label_fits_column(),
            "a tab label overruns the column the painter gives it"
        );
        assert!(
            tab_row::row_fits(tab_row::SURFACE_W),
            "tab row right edge {} overruns the {}-wide surface",
            tab_row::row_right_edge(),
            tab_row::SURFACE_W
        );
        let (_top, band_h) = interos_std::design::outline_text::row_band(0, tab_row::SCALE);
        assert!(
            band_h <= tab_row::BAND_H,
            "tab rung band {} exceeds the {}px tab band",
            band_h,
            tab_row::BAND_H
        );

        // 2. Settings row titles and notes must fit within DISPLAY_W - 48 at scale 1
        for row in SettingsRow::ORDER {
            let tw = font::measure(row.title(), 1);
            assert!(tw <= DISPLAY_W - 48, "Settings title {:?} exceeds band", core::str::from_utf8(row.title()).unwrap());
            let nw = font::measure(row.note(), 1);
            assert!(nw <= DISPLAY_W - 48, "Settings note {:?} exceeds band", core::str::from_utf8(row.note()).unwrap());
        }

        // 3. Contact store capacity line: ensure correct byte formatting without null bytes
        let mut line = [0u8; 96];
        let mut n = 0;
        let head = b"Saved ";
        line[n..n + head.len()].copy_from_slice(head);
        n += head.len();
        line[n] = b'3';
        n += 1;
        let mid = b" of ";
        line[n..n + mid.len()].copy_from_slice(mid);
        n += mid.len();
        line[n..n + 2].copy_from_slice(b"12");
        n += 2;
        assert_eq!(&line[..n], b"Saved 3 of 12");
        assert!(!line[..n].contains(&0), "Formatted string must not contain null bytes");
        assert!(font::measure(&line[..n], 1) <= DISPLAY_W - 48);
    }

    /// The re-pointed tab gate must actually bite: it refuses a row whose columns
    /// overrun the surface, and a label that no longer fits the column the painter
    /// gives it -- proving the gate tracks the geometry the app draws, not the
    /// stale 3-label / 208px pin it replaced.
    #[test]
    fn test_dialer_tab_gate_refuses_a_clipped_row() {
        use interos_std::design::{font, outline_text};

        // The real row fits the real surface, but the same arithmetic on a
        // surface narrowed to just under the row's own right edge is refused.
        let right = tab_row::row_right_edge();
        assert!(tab_row::row_fits(tab_row::SURFACE_W));
        assert!(!tab_row::row_fits(right - 1));

        // The real labels fit the column the painter gives them, but a column
        // narrowed under the widest label's own advance is refused -- the gate
        // tracks the column, not the 208px pin it replaced.
        let mut widest = 0usize;
        let mut widest_label: &[u8] = b"";
        for t in tab_row::LABELS.iter() {
            let w = font::measure(t, tab_row::SCALE);
            if w > widest {
                widest = w;
                widest_label = t;
            }
        }
        assert!(tab_row::each_label_fits_column());
        assert!(tab_row::label_fits_column(widest_label, widest));
        assert!(!tab_row::label_fits_column(widest_label, widest - 1));
        assert!(!tab_row::each_label_fits_column_at(widest - 1));

        // The rung's own ascent+descent fits the band the painter centres it in.
        let band_h = outline_text::row_band(0, tab_row::SCALE).1;
        assert!(band_h <= tab_row::BAND_H);
    }
}
