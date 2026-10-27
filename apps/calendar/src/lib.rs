#![no_std]

pub use interos_std as std;

/// The app's view of the platform's design system.
pub mod theme;

pub const EVENT_TITLE_MAX: usize = 32;
pub const MAX_EVENTS: usize = 12;
pub const RECORD_BYTES: usize = 2 + 1 + 1 + 1 + 1 + 1 + EVENT_TITLE_MAX + 1;
pub const STORE_BLOB_BYTES: usize = 2 + MAX_EVENTS * RECORD_BYTES;
/// Version 2 seals every numeric field with bit 7 set so the settings store's
/// line format never sees a 0x0A (newline) byte: month, day, hour and minute
/// can legitimately be 10, which version 1 stored raw.
pub const STORE_VERSION: u8 = 2;
pub const STORE_VERSION_LEGACY: u8 = 1;
pub const STORE_KEY: &str = "calendar.events.v1";
pub const STORE_YEAR_MIN: u16 = 1970;
pub const STORE_YEAR_MAX: u16 = 9999;

/// Returns true if a Gregorian year is a leap year.
pub fn is_leap_year(year: u32) -> bool {
    (year % 4 == 0 && year % 100 != 0) || (year % 400 == 0)
}

/// Returns the number of days in a given month.
pub fn days_in_month(year: u32, month: u8) -> u8 {
    match month {
        1 => 31,
        2 => {
            if is_leap_year(year) {
                29
            } else {
                28
            }
        }
        3 => 31,
        4 => 30,
        5 => 31,
        6 => 30,
        7 => 31,
        8 => 31,
        9 => 30,
        10 => 31,
        11 => 30,
        12 => 31,
        _ => 30,
    }
}

/// Computes the day of the week using Zeller's Congruence.
/// Returns 0 for Sunday, 1 for Monday, ..., 6 for Saturday.
pub fn day_of_week(mut year: u32, mut month: u8, day: u8) -> u8 {
    if month < 3 {
        month += 12;
        year -= 1;
    }
    let k = year % 100;
    let j = year / 100;
    let h = (day as u32 + (13 * (month as u32 + 1)) / 5 + k + k / 4 + j / 4 + 5 * j) % 7;
    ((h + 6) % 7) as u8
}

/// Reads the unsigned decimal integer that follows `key` in a JSON payload.
pub fn json_field(payload: &[u8], key: &[u8]) -> Option<u32> {
    if key.is_empty() || key.len() > payload.len() {
        return None;
    }
    for i in 0..=(payload.len() - key.len()) {
        if &payload[i..i + key.len()] != key {
            continue;
        }
        let mut j = i + key.len();
        let mut value: u32 = 0;
        let mut digits = 0;
        while j < payload.len() && payload[j].is_ascii_digit() {
            value = value.saturating_mul(10).saturating_add((payload[j] - b'0') as u32);
            digits += 1;
            j += 1;
        }
        if digits > 0 {
            return Some(value);
        }
        return None;
    }
    None
}

// -----------------------------------------------------------------------------
// Day-view header navigation arrows
// -----------------------------------------------------------------------------

/// How far a chevron's arms reach away from its apex, in pixels.
pub const CHEVRON_ARM: isize = 8;

/// The direction a chevron glyph points.
///
/// WHY this is a value and not a convention: the day-view header once drew
/// both of its chevrons mirrored, so the arrow on the left pointed forward
/// and the arrow on the right pointed back. The direction is part of the
/// glyph contract now, and a unit test pins it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ChevronDir {
    /// The apex is the leftmost point of the glyph: it reads "back".
    Left,
    /// The apex is the rightmost point of the glyph: it reads "forward".
    Right,
}

/// Tests whether the pixel at (x, y) lies on a chevron glyph whose apex sits
/// at (apex_x, apex_y).
///
/// WHY the arms are diagonal: the horizontal distance from the apex equals the
/// vertical distance from the centre line, so the glyph is a thin "<" or ">"
/// that is drawn from its apex outwards.
#[inline]
pub fn chevron_pixel(x: usize, y: usize, apex_x: isize, apex_y: isize, dir: ChevronDir) -> bool {
    let dx = match dir {
        ChevronDir::Left => x as isize - apex_x,
        ChevronDir::Right => apex_x - x as isize,
    };
    let dy = ((y as isize) - apex_y).abs();
    dx >= 0 && dx <= CHEVRON_ARM && (dx - dy).abs() <= 1
}

// The day-view header band and the two arrow buttons inside it.
pub const DAY_ARROW_BAND_Y0: usize = 160;
pub const DAY_ARROW_BAND_Y1: usize = 208;
pub const DAY_PREV_ARROW_X0: usize = 24;
pub const DAY_PREV_ARROW_X1: usize = 72;
pub const DAY_NEXT_ARROW_X0: usize = 648;
pub const DAY_NEXT_ARROW_X1: usize = 696;
/// The previous-day chevron's apex: the leftmost point of its glyph.
pub const DAY_PREV_ARROW_APEX_X: isize = 44;
pub const DAY_PREV_ARROW_APEX_Y: isize = 184;
/// The next-day chevron's apex: the rightmost point of its glyph.
pub const DAY_NEXT_ARROW_APEX_X: isize = 676;
pub const DAY_NEXT_ARROW_APEX_Y: isize = 184;

/// Which way a day-view header arrow moves the selected day.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum DayArrow {
    /// The left chevron: back to the previous day.
    Previous,
    /// The right chevron: forward to the next day.
    Next,
}

/// The day the arrow selects, or `None` when the arrow would leave the month.
///
/// WHY this lives beside the glyphs: the chevron that points back and the
/// chevron that steps back are the same control, so the direction is decided
/// once and the renderer and the tap handler read the same answer.
pub fn step_selected_day(day: u8, arrow: DayArrow, days_in_month: u8) -> Option<u8> {
    match arrow {
        DayArrow::Previous if day > 1 => Some(day - 1),
        DayArrow::Next if day < days_in_month => Some(day + 1),
        _ => None,
    }
}

/// One calendar event as it is stored: a civil date, a wall-clock time and a
/// bounded title.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct EventRecord {
    pub year: u16,
    pub month: u8,
    pub day: u8,
    pub hour: u8,
    pub minute: u8,
    pub title: [u8; EVENT_TITLE_MAX],
    pub title_len: u8,
}

impl EventRecord {
    pub const EMPTY: EventRecord = EventRecord {
        year: 0,
        month: 0,
        day: 0,
        hour: 0,
        minute: 0,
        title: [0; EVENT_TITLE_MAX],
        title_len: 0,
    };

    pub fn from_parts(year: u16, month: u8, day: u8, hour: u8, minute: u8, title: &[u8]) -> Self {
        let mut rec = EventRecord::EMPTY;
        rec.year = year;
        rec.month = month;
        rec.day = day;
        rec.hour = hour;
        rec.minute = minute;
        let n = title.len().min(EVENT_TITLE_MAX);
        rec.title[..n].copy_from_slice(&title[..n]);
        rec.title_len = n as u8;
        rec
    }

    pub fn valid(&self) -> bool {
        self.year >= STORE_YEAR_MIN
            && self.year <= STORE_YEAR_MAX
            && (1..=12).contains(&self.month)
            && (1..=days_in_month(self.year as u32, self.month)).contains(&self.day)
            && self.hour <= 23
            && self.minute <= 59
            && self.title_len as usize <= EVENT_TITLE_MAX
            && self
                .title
                .iter()
                .take(self.title_len as usize)
                .all(|b| (0x20..=0x7E).contains(b))
    }

    pub fn on_day(&self, year: u16, month: u8, day: u8) -> bool {
        self.year == year && self.month == month && self.day == day
    }

    pub fn title_bytes(&self) -> &[u8] {
        &self.title[..self.title_len as usize]
    }

    /// Stores a value in the low 7 bits with bit 7 set, so no field byte can
    /// collide with the settings store's 0x0A line terminator.
    fn seal(value: u8) -> u8 {
        (value & 0x7F) | 0x80
    }

    fn unseal(byte: u8) -> u8 {
        byte & 0x7F
    }

    fn encode_fields(
        year: u16,
        month: u8,
        day: u8,
        hour: u8,
        minute: u8,
        title_len: u8,
        title: &[u8],
        out: &mut [u8],
    ) {
        out[0] = Self::seal((year & 0x7F) as u8);
        out[1] = Self::seal(((year >> 7) & 0x7F) as u8);
        out[2] = Self::seal(month);
        out[3] = Self::seal(day);
        out[4] = Self::seal(hour);
        out[5] = Self::seal(minute);
        out[6] = Self::seal(title_len);
        out[7..7 + EVENT_TITLE_MAX].copy_from_slice(&title[..EVENT_TITLE_MAX]);
        out[7 + EVENT_TITLE_MAX] = 0;
    }

    pub fn encode(&self, out: &mut [u8]) {
        Self::encode_fields(
            self.year,
            self.month,
            self.day,
            self.hour,
            self.minute,
            self.title_len,
            &self.title,
            out,
        );
    }

    fn decode_fields(bytes: &[u8], sealed: bool) -> Option<EventRecord> {
        if bytes.len() < RECORD_BYTES {
            return None;
        }
        let mut rec = EventRecord::EMPTY;
        if sealed {
            rec.year = Self::unseal(bytes[0]) as u16 | ((Self::unseal(bytes[1]) as u16) << 7);
            rec.month = Self::unseal(bytes[2]);
            rec.day = Self::unseal(bytes[3]);
            rec.hour = Self::unseal(bytes[4]);
            rec.minute = Self::unseal(bytes[5]);
            rec.title_len = Self::unseal(bytes[6]);
        } else {
            rec.year = bytes[0] as u16 | ((bytes[1] as u16) << 8);
            rec.month = bytes[2];
            rec.day = bytes[3];
            rec.hour = bytes[4];
            rec.minute = bytes[5];
            rec.title_len = bytes[6];
        }
        if rec.title_len as usize > EVENT_TITLE_MAX {
            return None;
        }
        rec.title[..EVENT_TITLE_MAX].copy_from_slice(&bytes[7..7 + EVENT_TITLE_MAX]);
        if rec.valid() {
            Some(rec)
        } else {
            None
        }
    }

    pub fn decode(bytes: &[u8]) -> Option<EventRecord> {
        Self::decode_fields(bytes, true)
    }

    fn decode_legacy(bytes: &[u8]) -> Option<EventRecord> {
        Self::decode_fields(bytes, false)
    }
}

/// The location line an event card draws under the event title.
///
/// WHY the record is an argument and the answer is the same for every
/// record: `EventRecord` stores a civil date, a wall-clock time and a title.
/// It has no location, section or calendar field, so the platform holds no
/// place for the event and the app must not name one. The card reports that
/// the location is absent instead of showing a venue that was never stored.
pub fn event_location(_rec: &EventRecord) -> &'static [u8] {
    b"No location"
}

/// What the store's last contact with the platform storage actually did.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum StoreState {
    Untouched,
    Empty,
    Loaded,
    ReadRefused,
    Invalid,
    Written,
    WriteRefused,
    Full,
}

impl StoreState {
    pub fn as_str(self) -> &'static str {
        match self {
            StoreState::Untouched => "EVENT STORE NOT READ YET",
            StoreState::Empty => "NO STORED EVENTS: SETTINGS STORE HOLDS NO CALENDAR KEY",
            StoreState::Loaded => "STORED EVENTS LOADED FROM THE SETTINGS STORE",
            StoreState::ReadRefused => "SETTINGS READ REFUSED: NO STORED EVENTS LOADED",
            StoreState::Invalid => "STORED EVENT DATA FAILED VALIDATION: NO EVENTS LOADED",
            StoreState::Written => "EVENT WRITTEN TO THE SETTINGS STORE",
            StoreState::WriteRefused => "SETTINGS WRITE REFUSED: EVENT NOT PERSISTED",
            StoreState::Full => "STORE FULL: NO EVENT WRITTEN",
        }
    }
}

/// The agenda: bounded records plus an honest account of the last load or save.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct EventStore {
    pub records: [EventRecord; MAX_EVENTS],
    pub len: u8,
    pub state: StoreState,
}

impl EventStore {
    pub const EMPTY: EventStore = EventStore {
        records: [EventRecord::EMPTY; MAX_EVENTS],
        len: 0,
        state: StoreState::Untouched,
    };

    pub fn len(&self) -> usize {
        self.len as usize
    }

    pub fn is_full(&self) -> bool {
        self.len() >= MAX_EVENTS
    }

    pub fn has_event_on(&self, year: u16, month: u8, day: u8) -> bool {
        for i in 0..self.len() {
            let r = &self.records[i];
            if r.on_day(year, month, day) {
                return true;
            }
        }
        false
    }

    pub fn add_in_memory(&mut self, rec: EventRecord) -> bool {
        if self.is_full() {
            self.state = StoreState::Full;
            return false;
        }
        if !rec.valid() {
            return false;
        }
        let len = self.len();
        self.records[len] = rec;
        self.len = (len + 1) as u8;
        self.state = StoreState::Written;
        true
    }

    pub fn delete_at_in_memory(&mut self, index: usize) -> bool {
        if index >= self.len() {
            return false;
        }
        let count = self.len();
        for i in index..count - 1 {
            self.records[i] = self.records[i + 1];
        }
        self.records[count - 1] = EventRecord::EMPTY;
        self.len = (count - 1) as u8;
        self.state = StoreState::Written;
        true
    }

    pub fn delete_event_in_memory(&mut self, rec: &EventRecord) -> bool {
        for i in 0..self.len() {
            if &self.records[i] == rec {
                return self.delete_at_in_memory(i);
            }
        }
        false
    }

    /// Collects one day's events, earliest first.
    pub fn events_on(
        &self,
        year: u16,
        month: u8,
        day: u8,
        out: &mut [EventRecord; MAX_EVENTS],
    ) -> usize {
        let mut n = 0usize;
        for i in 0..self.len() {
            if self.records[i].on_day(year, month, day) {
                out[n] = self.records[i];
                n += 1;
            }
        }
        // Insertion sort by wall-clock time
        for i in 1..n {
            let mut j = i;
            while j > 0 && (out[j].hour, out[j].minute) < (out[j - 1].hour, out[j - 1].minute) {
                out.swap(j, j - 1);
                j -= 1;
            }
        }
        n
    }

    /// Collects all events across all dates, sorted chronologically (year, month, day, hour, minute).
    pub fn all_events_sorted(&self, out: &mut [EventRecord; MAX_EVENTS]) -> usize {
        let n = self.len();
        for i in 0..n {
            out[i] = self.records[i];
        }
        for i in 1..n {
            let mut j = i;
            while j > 0 {
                let prev = (out[j - 1].year, out[j - 1].month, out[j - 1].day, out[j - 1].hour, out[j - 1].minute);
                let curr = (out[j].year, out[j].month, out[j].day, out[j].hour, out[j].minute);
                if curr < prev {
                    out.swap(j, j - 1);
                    j -= 1;
                } else {
                    break;
                }
            }
        }
        n
    }

    /// Encodes the entire store into a binary blob.
    pub fn encode_to_blob(&self, out: &mut [u8; STORE_BLOB_BYTES]) -> usize {
        out[0] = STORE_VERSION;
        let count = self.len();
        out[1] = count as u8;
        for i in 0..count {
            let start = 2 + i * RECORD_BYTES;
            self.records[i].encode(&mut out[start..start + RECORD_BYTES]);
        }
        2 + count * RECORD_BYTES
    }

    /// Decodes a store from a binary blob.
    pub fn decode_from_blob(blob: &[u8]) -> Result<EventStore, StoreState> {
        let n = blob.len();
        if n < 2 || n > STORE_BLOB_BYTES {
            return Err(StoreState::Invalid);
        }
        let version = blob[0];
        if version != STORE_VERSION && version != STORE_VERSION_LEGACY {
            return Err(StoreState::Invalid);
        }
        let header_count = blob[1] as usize;
        if header_count > MAX_EVENTS {
            return Err(StoreState::Invalid);
        }
        // Version 1 writers always sent the full fixed-width array; version 2
        // sends only the used records. Accept both shapes for legacy blobs.
        let used_len = 2 + header_count * RECORD_BYTES;
        let length_ok = n == used_len || (version == STORE_VERSION_LEGACY && n == STORE_BLOB_BYTES);
        if !length_ok {
            return Err(StoreState::Invalid);
        }
        let mut store = EventStore::EMPTY;
        for i in 0..header_count {
            let start = 2 + i * RECORD_BYTES;
            let decoded = if version == STORE_VERSION_LEGACY {
                EventRecord::decode_legacy(&blob[start..start + RECORD_BYTES])
            } else {
                EventRecord::decode(&blob[start..start + RECORD_BYTES])
            };
            match decoded {
                Some(rec) => {
                    store.records[i] = rec;
                }
                None => {
                    return Err(StoreState::Invalid);
                }
            }
        }
        store.len = header_count as u8;
        store.state = if header_count == 0 { StoreState::Empty } else { StoreState::Loaded };
        Ok(store)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_gregorian_leap_years_and_month_durations() {
        assert!(is_leap_year(2000));
        assert!(!is_leap_year(1900));
        assert!(is_leap_year(2024));
        assert!(!is_leap_year(2025));
        assert!(!is_leap_year(2026));
        assert!(is_leap_year(2028));

        assert_eq!(days_in_month(2024, 2), 29);
        assert_eq!(days_in_month(2026, 2), 28);
        assert_eq!(days_in_month(2026, 1), 31);
        assert_eq!(days_in_month(2026, 4), 30);
        assert_eq!(days_in_month(2026, 12), 31);
    }

    #[test]
    fn test_zeller_congruence_day_of_week() {
        // 2026-10-14 is Wednesday (3)
        assert_eq!(day_of_week(2026, 10, 14), 3);
        // 2000-01-01 was Saturday (6)
        assert_eq!(day_of_week(2000, 1, 1), 6);
        // 2024-02-29 was Thursday (4)
        assert_eq!(day_of_week(2024, 2, 29), 4);
        // 2026-01-01 is Thursday (4)
        assert_eq!(day_of_week(2026, 1, 1), 4);
    }

    #[test]
    fn test_json_field_parser() {
        let payload = b"{\"year\":2026,\"month\":10,\"day\":14,\"hour\":9,\"minute\":30}";
        assert_eq!(json_field(payload, b"\"year\":"), Some(2026));
        assert_eq!(json_field(payload, b"\"month\":"), Some(10));
        assert_eq!(json_field(payload, b"\"day\":"), Some(14));
        assert_eq!(json_field(payload, b"\"hour\":"), Some(9));
        assert_eq!(json_field(payload, b"\"minute\":"), Some(30));
        assert_eq!(json_field(payload, b"\"second\":"), None);
    }

    #[test]
    fn test_event_record_encode_decode() {
        let rec = EventRecord::from_parts(2026, 10, 14, 15, 0, b"Team Sync");
        assert!(rec.valid());

        let mut buf = [0u8; RECORD_BYTES];
        rec.encode(&mut buf);

        let decoded = EventRecord::decode(&buf).expect("decode valid event");
        assert_eq!(decoded.year, 2026);
        assert_eq!(decoded.month, 10);
        assert_eq!(decoded.day, 14);
        assert_eq!(decoded.hour, 15);
        assert_eq!(decoded.minute, 0);
        assert_eq!(&decoded.title[..decoded.title_len as usize], b"Team Sync");

        // Invalid month
        let mut bad_buf = buf;
        bad_buf[2] = 13;
        assert!(EventRecord::decode(&bad_buf).is_none());

        // Invalid day (Feb 30)
        let mut bad_feb = buf;
        bad_feb[2] = 2;
        bad_feb[3] = 30;
        assert!(EventRecord::decode(&bad_feb).is_none());
    }

    #[test]
    fn test_event_store_add_and_delete() {
        let mut store = EventStore::EMPTY;
        assert_eq!(store.len(), 0);

        let ev1 = EventRecord::from_parts(2026, 10, 14, 9, 30, b"Morning Standup");
        let ev2 = EventRecord::from_parts(2026, 10, 14, 15, 0, b"Architecture Review");
        let ev3 = EventRecord::from_parts(2026, 10, 15, 11, 0, b"Product Demo");

        assert!(store.add_in_memory(ev1));
        assert!(store.add_in_memory(ev2));
        assert!(store.add_in_memory(ev3));
        assert_eq!(store.len(), 3);
        assert!(store.has_event_on(2026, 10, 14));
        assert!(store.has_event_on(2026, 10, 15));
        assert!(!store.has_event_on(2026, 10, 16));

        // Delete second event
        assert!(store.delete_at_in_memory(1));
        assert_eq!(store.len(), 2);
        assert_eq!(store.records[0], ev1);
        assert_eq!(store.records[1], ev3);

        // Delete by record
        assert!(store.delete_event_in_memory(&ev1));
        assert_eq!(store.len(), 1);
        assert_eq!(store.records[0], ev3);
    }

    #[test]
    fn test_event_store_persistence_encode_decode() {
        let mut store = EventStore::EMPTY;
        let ev1 = EventRecord::from_parts(2026, 10, 14, 10, 0, b"Sprint Planning");
        let ev2 = EventRecord::from_parts(2026, 10, 20, 14, 0, b"Release Cut");
        assert!(store.add_in_memory(ev1));
        assert!(store.add_in_memory(ev2));

        let mut blob = [0u8; STORE_BLOB_BYTES];
        let bytes_written = store.encode_to_blob(&mut blob);
        assert_eq!(bytes_written, 2 + 2 * RECORD_BYTES);

        let decoded = EventStore::decode_from_blob(&blob[..bytes_written]).expect("decode store");
        assert_eq!(decoded.len(), 2);
        assert_eq!(decoded.records[0], ev1);
        assert_eq!(decoded.records[1], ev2);
        assert_eq!(decoded.state, StoreState::Loaded);

        // Corrupted blob version
        let mut bad_blob = blob;
        bad_blob[0] = 99;
        assert_eq!(EventStore::decode_from_blob(&bad_blob[..bytes_written]), Err(StoreState::Invalid));
    }

    #[test]
    fn test_store_blob_is_newline_free() {
        // The settings store is newline-delimited, so a store blob must never
        // contain 0x0A. October (month 10) and any field set to 10 used to.
        let mut store = EventStore::EMPTY;
        for i in 0..MAX_EVENTS {
            let ev = EventRecord::from_parts(
                2058,
                10,
                10,
                10,
                10,
                b"Ten O'clock Sync",
            );
            assert!(store.add_in_memory(ev), "record {} should fit", i);
        }
        let mut blob = [0u8; STORE_BLOB_BYTES];
        let used = store.encode_to_blob(&mut blob);
        assert_eq!(used, STORE_BLOB_BYTES);
        assert!(
            !blob[..used].contains(&b'\n'),
            "encoded calendar blob contains a newline byte"
        );
        assert!(used <= 512, "settings values are capped at 512 bytes");
    }

    #[test]
    fn test_legacy_store_version_still_decodes() {
        let ev = EventRecord::from_parts(2026, 10, 14, 10, 0, b"Legacy Sync");
        let mut blob = [0u8; STORE_BLOB_BYTES];
        blob[0] = STORE_VERSION_LEGACY;
        blob[1] = 1;
        blob[2] = (ev.year & 0xFF) as u8;
        blob[3] = (ev.year >> 8) as u8;
        blob[4] = ev.month;
        blob[5] = ev.day;
        blob[6] = ev.hour;
        blob[7] = ev.minute;
        blob[8] = ev.title_len;
        blob[9..9 + EVENT_TITLE_MAX].copy_from_slice(&ev.title);

        let decoded = EventStore::decode_from_blob(&blob).expect("full-width legacy blob");
        assert_eq!(decoded.len(), 1);
        assert_eq!(decoded.records[0], ev);

        let decoded_used = EventStore::decode_from_blob(&blob[..2 + RECORD_BYTES])
            .expect("trimmed legacy blob");
        assert_eq!(decoded_used.len(), 1);
        assert_eq!(decoded_used.records[0], ev);
    }

    #[test]
    fn test_event_store_capacity_limit() {
        let mut store = EventStore::EMPTY;
        for i in 0..MAX_EVENTS {
            let ev = EventRecord::from_parts(2026, 10, (i + 1) as u8, 10, 0, b"Sync");
            assert!(store.add_in_memory(ev));
        }
        assert!(store.is_full());
        assert_eq!(store.len(), MAX_EVENTS);

        // Adding 13th event should be refused
        let overflow_ev = EventRecord::from_parts(2026, 10, 25, 10, 0, b"Too Many");
        assert!(!store.add_in_memory(overflow_ev));
        assert_eq!(store.state, StoreState::Full);
    }

    #[test]
    fn test_event_sorting_and_chronology() {
        let mut store = EventStore::EMPTY;
        let ev_late = EventRecord::from_parts(2026, 10, 14, 18, 0, b"Dinner");
        let ev_early = EventRecord::from_parts(2026, 10, 14, 8, 30, b"Breakfast");
        let ev_noon = EventRecord::from_parts(2026, 10, 14, 12, 0, b"Lunch");
        let ev_tomorrow = EventRecord::from_parts(2026, 10, 15, 9, 0, b"Tomorrow");

        store.add_in_memory(ev_late);
        store.add_in_memory(ev_early);
        store.add_in_memory(ev_tomorrow);
        store.add_in_memory(ev_noon);

        let mut day_items = [EventRecord::EMPTY; MAX_EVENTS];
        let day_count = store.events_on(2026, 10, 14, &mut day_items);
        assert_eq!(day_count, 3);
        assert_eq!(day_items[0], ev_early);
        assert_eq!(day_items[1], ev_noon);
        assert_eq!(day_items[2], ev_late);

        let mut all_items = [EventRecord::EMPTY; MAX_EVENTS];
        let all_count = store.all_events_sorted(&mut all_items);
        assert_eq!(all_count, 4);
        assert_eq!(all_items[0], ev_early);
        assert_eq!(all_items[1], ev_noon);
        assert_eq!(all_items[2], ev_late);
        assert_eq!(all_items[3], ev_tomorrow);
    }

    #[test]
    fn test_all_calendar_strings_fit_horizontal_bands() {
        use interos_std::design::font;

        // 1. Commit status strings must fit the flow sheet width (632px) at scale 2
        let statuses: [&[u8]; 8] = [
            b"NO EVENT CREATED YET",
            b"NOT SAVED: EVENT HAS NO TITLE",
            b"NOT SAVED: NO DATE ENTERED",
            b"NOT SAVED: NO TIME ENTERED",
            b"NOT SAVED: STORE FULL (12)",
            b"NOT SAVED: INVALID DATE OR TIME",
            b"SAVED: EVENT STORED",
            b"CANCELLED: NO EVENT WRITTEN",
        ];
        for s in statuses.iter() {
            let w = font::measure(s, 2);
            assert!(w <= 632, "Status {:?} width {} exceeds 632px", core::str::from_utf8(s).unwrap(), w);
        }

        // 2. Wizard step headers must fit sheet width (632px) at scale 2
        let steps: [&[u8]; 3] = [
            b"STEP 1 OF 3 - EVENT TITLE",
            b"STEP 2 OF 3 - DATE (MANUAL)",
            b"STEP 3 OF 3 - TIME",
        ];
        for s in steps.iter() {
            let w = font::measure(s, 2);
            assert!(w <= 632, "Step header {:?} width {} exceeds 632px", core::str::from_utf8(s).unwrap(), w);
        }

        // 3. Tab labels must fit TAB_W (147px) at scale 2
        let tabs: [&[u8]; 4] = [b"Month", b"Week", b"Day", b"Agenda"];
        for t in tabs.iter() {
            let w = font::measure(t, 2);
            assert!(w <= 147, "Tab {:?} width {} exceeds 147px", core::str::from_utf8(t).unwrap(), w);
        }

        // 4. Details storage line must fit within 600px at scale 2
        let storage_line = b"Settings Store / Sector 203";
        let w = font::measure(storage_line, 2);
        assert!(w <= 600, "Storage line width {} exceeds 600px", w);
    }

    /// The horizontal extent of the ink a chevron glyph lays down.
    fn chevron_ink_bounds(
        apex_x: isize,
        apex_y: isize,
        dir: ChevronDir,
        x0: usize,
        x1: usize,
        y0: usize,
        y1: usize,
    ) -> (usize, usize) {
        let mut min_x = usize::MAX;
        let mut max_x = 0usize;
        for y in y0..y1 {
            for x in x0..x1 {
                if chevron_pixel(x, y, apex_x, apex_y, dir) {
                    if x < min_x {
                        min_x = x;
                    }
                    if x > max_x {
                        max_x = x;
                    }
                }
            }
        }
        (min_x, max_x)
    }

    #[test]
    fn test_day_header_chevrons_point_the_way_they_move() {
        // WHY this is pinned: the day-view header drew both chevrons mirrored,
        // so the arrow on the left pointed forward and the arrow on the right
        // pointed back. The glyph and the control it belongs to have to agree.

        // The previous-day chevron is a "<": its apex is the leftmost point of
        // the ink and the arms run up and down to the right of it.
        let prev_apex_x = DAY_PREV_ARROW_APEX_X as usize;
        let prev_apex_y = DAY_PREV_ARROW_APEX_Y as usize;
        assert!(chevron_pixel(
            prev_apex_x,
            prev_apex_y,
            DAY_PREV_ARROW_APEX_X,
            DAY_PREV_ARROW_APEX_Y,
            ChevronDir::Left
        ));
        assert!(chevron_pixel(
            prev_apex_x + CHEVRON_ARM as usize,
            prev_apex_y - CHEVRON_ARM as usize,
            DAY_PREV_ARROW_APEX_X,
            DAY_PREV_ARROW_APEX_Y,
            ChevronDir::Left
        ));
        assert!(chevron_pixel(
            prev_apex_x + CHEVRON_ARM as usize,
            prev_apex_y + CHEVRON_ARM as usize,
            DAY_PREV_ARROW_APEX_X,
            DAY_PREV_ARROW_APEX_Y,
            ChevronDir::Left
        ));
        // Nothing is inked to the left of the apex: the glyph points back.
        assert!(!chevron_pixel(
            prev_apex_x - 1,
            prev_apex_y,
            DAY_PREV_ARROW_APEX_X,
            DAY_PREV_ARROW_APEX_Y,
            ChevronDir::Left
        ));

        // The next-day chevron is the mirror image: its apex is the rightmost
        // point of the ink and the arms run up and down to the left of it.
        let next_apex_x = DAY_NEXT_ARROW_APEX_X as usize;
        let next_apex_y = DAY_NEXT_ARROW_APEX_Y as usize;
        assert!(chevron_pixel(
            next_apex_x,
            next_apex_y,
            DAY_NEXT_ARROW_APEX_X,
            DAY_NEXT_ARROW_APEX_Y,
            ChevronDir::Right
        ));
        assert!(chevron_pixel(
            next_apex_x - CHEVRON_ARM as usize,
            next_apex_y - CHEVRON_ARM as usize,
            DAY_NEXT_ARROW_APEX_X,
            DAY_NEXT_ARROW_APEX_Y,
            ChevronDir::Right
        ));
        assert!(chevron_pixel(
            next_apex_x - CHEVRON_ARM as usize,
            next_apex_y + CHEVRON_ARM as usize,
            DAY_NEXT_ARROW_APEX_X,
            DAY_NEXT_ARROW_APEX_Y,
            ChevronDir::Right
        ));
        // Nothing is inked to the right of the apex: the glyph points forward.
        assert!(!chevron_pixel(
            next_apex_x + 1,
            next_apex_y,
            DAY_NEXT_ARROW_APEX_X,
            DAY_NEXT_ARROW_APEX_Y,
            ChevronDir::Right
        ));

        // Each glyph stays inside its own button, and the previous glyph ends
        // where the next one begins: the two arrows are mirror images.
        let (prev_min, prev_max) = chevron_ink_bounds(
            DAY_PREV_ARROW_APEX_X,
            DAY_PREV_ARROW_APEX_Y,
            ChevronDir::Left,
            DAY_PREV_ARROW_X0,
            DAY_PREV_ARROW_X1,
            DAY_ARROW_BAND_Y0,
            DAY_ARROW_BAND_Y1,
        );
        let (next_min, next_max) = chevron_ink_bounds(
            DAY_NEXT_ARROW_APEX_X,
            DAY_NEXT_ARROW_APEX_Y,
            ChevronDir::Right,
            DAY_NEXT_ARROW_X0,
            DAY_NEXT_ARROW_X1,
            DAY_ARROW_BAND_Y0,
            DAY_ARROW_BAND_Y1,
        );
        assert_eq!(prev_min, DAY_PREV_ARROW_APEX_X as usize);
        assert_eq!(prev_max, DAY_PREV_ARROW_APEX_X as usize + CHEVRON_ARM as usize);
        assert_eq!(next_min, DAY_NEXT_ARROW_APEX_X as usize - CHEVRON_ARM as usize);
        assert_eq!(next_max, DAY_NEXT_ARROW_APEX_X as usize);
        assert!(prev_max < next_min, "the two header chevrons overlap");
    }

    #[test]
    fn test_day_arrows_move_the_selected_day_in_their_own_direction() {
        // The left chevron goes back to the previous day and the right chevron
        // goes forward to the next day.
        assert_eq!(step_selected_day(14, DayArrow::Previous, 31), Some(13));
        assert_eq!(step_selected_day(14, DayArrow::Next, 31), Some(15));

        // The first and the last day of the month do not wrap around.
        assert_eq!(step_selected_day(1, DayArrow::Previous, 31), None);
        assert_eq!(step_selected_day(31, DayArrow::Next, 31), None);

        // Every day in between moves the way its chevron points.
        for day in 2..=30u8 {
            let back = step_selected_day(day, DayArrow::Previous, 31).expect("a day before the last");
            let forward = step_selected_day(day, DayArrow::Next, 31).expect("a day after the first");
            assert!(back < day, "the previous-day arrow moved forward");
            assert!(forward > day, "the next-day arrow moved backward");
        }

        // A short month clamps on its own last day.
        assert_eq!(step_selected_day(28, DayArrow::Next, 28), None);
        assert_eq!(step_selected_day(28, DayArrow::Previous, 28), Some(27));
    }

    #[test]
    fn test_event_card_location_reports_the_absent_field_instead_of_a_venue() {
        // The stored record holds a civil date, a wall-clock time and a title.
        // It has no location, section or calendar field, so an event card has
        // no real place to show and must not name one.
        let rec = EventRecord::from_parts(2026, 10, 14, 9, 30, b"Team Sync");
        let label = event_location(&rec);
        assert_eq!(label, b"No location");

        // The card used to name a sealed vault sector as the place of every
        // event. The label must not name a place again, so the whole app
        // source stays free of the invented venue.
        let text = core::str::from_utf8(label).expect("ascii label");
        assert!(!text.contains("Sector"), "the location names a vault sector");
        assert!(!text.contains("Encrypted"), "the location names an encrypted venue");

        // A record with no title is reported the same way: the location comes
        // from the record's fields, and none of them is a place.
        assert_eq!(event_location(&EventRecord::EMPTY), b"No location");
    }
}
