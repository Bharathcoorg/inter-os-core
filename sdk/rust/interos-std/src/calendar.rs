//! Calendar shared types, date math, and event persistence codec for `calendar.events.v1`.

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

    pub fn decode_legacy(bytes: &[u8]) -> Option<EventRecord> {
        Self::decode_fields(bytes, false)
    }
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

    pub fn is_empty(&self) -> bool {
        self.len == 0
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
                let prev = (
                    out[j - 1].year,
                    out[j - 1].month,
                    out[j - 1].day,
                    out[j - 1].hour,
                    out[j - 1].minute,
                );
                let curr = (
                    out[j].year,
                    out[j].month,
                    out[j].day,
                    out[j].hour,
                    out[j].minute,
                );
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
        store.state = if header_count == 0 {
            StoreState::Empty
        } else {
            StoreState::Loaded
        };
        Ok(store)
    }

    /// Loads the store using [`load`].
    pub fn load() -> Self {
        load()
    }
}

/// Helper that reads `crate::settings_get(STORE_KEY, ...)` and distinguishes
/// `Empty`, `ReadRefused`, `Loaded`, and `Invalid`.
pub fn load() -> EventStore {
    let mut blob = [0u8; STORE_BLOB_BYTES];
    match crate::settings_get(STORE_KEY, &mut blob) {
        Ok(0) => EventStore {
            state: StoreState::Empty,
            ..EventStore::EMPTY
        },
        Ok(n) => match EventStore::decode_from_blob(&blob[..n]) {
            Ok(decoded) => decoded,
            Err(state) => EventStore {
                state,
                ..EventStore::EMPTY
            },
        },
        Err(_) => EventStore {
            state: StoreState::ReadRefused,
            ..EventStore::EMPTY
        },
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
        assert_eq!(day_of_week(2026, 10, 14), 3);
        assert_eq!(day_of_week(2000, 1, 1), 6);
        assert_eq!(day_of_week(2024, 2, 29), 4);
        assert_eq!(day_of_week(2026, 1, 1), 4);
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

        assert!(store.delete_at_in_memory(1));
        assert_eq!(store.len(), 2);
        assert_eq!(store.records[0], ev1);
        assert_eq!(store.records[1], ev3);

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
    }
}
