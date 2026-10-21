#![no_std]

pub use interos_std as std;

/// The app's view of the platform's design system.
pub mod theme;

pub const EVENT_TITLE_MAX: usize = 32;
pub const MAX_EVENTS: usize = 12;
pub const RECORD_BYTES: usize = 2 + 1 + 1 + 1 + 1 + 1 + EVENT_TITLE_MAX + 1;
pub const STORE_BLOB_BYTES: usize = 2 + MAX_EVENTS * RECORD_BYTES;
pub const STORE_VERSION: u8 = 1;
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

    pub fn encode(&self, out: &mut [u8]) {
        out[0] = (self.year & 0xFF) as u8;
        out[1] = (self.year >> 8) as u8;
        out[2] = self.month;
        out[3] = self.day;
        out[4] = self.hour;
        out[5] = self.minute;
        out[6] = self.title_len;
        out[7..7 + EVENT_TITLE_MAX].copy_from_slice(&self.title);
        out[7 + EVENT_TITLE_MAX] = 0;
    }

    pub fn decode(bytes: &[u8]) -> Option<EventRecord> {
        if bytes.len() < RECORD_BYTES {
            return None;
        }
        let mut rec = EventRecord::EMPTY;
        rec.year = bytes[0] as u16 | ((bytes[1] as u16) << 8);
        rec.month = bytes[2];
        rec.day = bytes[3];
        rec.hour = bytes[4];
        rec.minute = bytes[5];
        rec.title_len = bytes[6];
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
}
