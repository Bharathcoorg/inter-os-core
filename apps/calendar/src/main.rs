#![no_std]
#![no_main]

//! Inter OS Native Sovereign Calendar & Chrono-Planner
//!
//! Native personal timeline & autonomous schedule planning engine executing in Ring 3 (User Mode):
//! - Gregorian Civil Calendar date math:
//!   * Astronomical leap year calculation
//!   * Month duration tables
//!   * Zeller's Congruence for day-of-week calculation (Sunday = 0 .. Saturday = 6)
//! - Hardware RTC real wall-clock synchronization via fast syscall
//! - Natural language scheduling intent parser ("schedule dev sync tomorrow at 3pm")
//! - Sovereign Privacy Shield scrubbing to prevent calendar metadata leakage
//! - Sovereign Vault sector encryption for agenda events (Sector 203)
//! - Persistent agenda state is populated only from real stored calendar data.
//! - Ascending A-major chord acoustic reminder chime (440Hz -> 554Hz -> 659Hz) and capacitive haptics
//! - Asynchronous event reactor loop handling reminder alarms, user intent schedules, and tick updates

use interos_std as std;
use core::panic::PanicInfo;

mod outline_text;

/// Calendar event status.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EventStatus {
    Confirmed = 0,
    Tentative = 1,
    Completed = 2,
}

/// Calendar event entry.
///
/// WHY this type is retained even though the agenda below is no longer drawn
/// from a static table: `tests/app_truth_contract.sh` pins the *declaration* of
/// the empty agenda (`AGENDA: [CalendarEvent; 0] = []`) so a static built-in
/// table can never be reintroduced. Real events live in `EventStore`, so this
/// type and the constant are now only a contract anchor and are not referenced
/// by the renderer.
#[allow(dead_code)]
pub struct CalendarEvent {
    id: u32,
    title: &'static str,
    time_str: &'static str,
    category: &'static str,
    status: EventStatus,
}

/// Static Built-in Agenda.
///
/// WHY zero-length: no event is fabricated. The list the app actually draws is
/// read back out of the settings store (`EventStore`), so this array stays empty
/// and the agenda branch is reached only through real stored records.
#[allow(dead_code)]
pub static AGENDA: [CalendarEvent; 0] = [];

// -----------------------------------------------------------------------------
// Event Store: bounded fixed-size records in the platform's own settings store
// -----------------------------------------------------------------------------

/// Longest event title the record format holds.
const EVENT_TITLE_MAX: usize = 32;
/// Bounded number of events the calendar can hold.
///
/// WHY 12: a record is `RECORD_BYTES` wide and the kernel refuses a settings
/// value longer than 512 bytes (`kernel/src/syscall/mod.rs:4796`), so
/// `2 + MAX_EVENTS * RECORD_BYTES <= 512` caps the store at 12 events. The store
/// says so when it is full instead of dropping anything.
const MAX_EVENTS: usize = 12;

/// Record width, pinned: 2 year (u16 LE) + 1 month + 1 day + 1 hour + 1 minute
/// + 1 title length + `EVENT_TITLE_MAX` title bytes + 1 reserved byte = 40.
///
/// WHY the width is a constant: `#![no_std]` with no allocator means the store
/// is a fixed byte array, so this encoding is the store's on-disk contract.
/// Changing any field width changes `STORE_BLOB_BYTES` and orphans every event
/// already saved, so the widths are written out here rather than derived.
const RECORD_BYTES: usize = 2 + 1 + 1 + 1 + 1 + 1 + EVENT_TITLE_MAX + 1;

/// Blob width: 1 version byte + 1 count byte + `MAX_EVENTS` records = 482 bytes,
/// inside the kernel's 512-byte settings value cap.
const STORE_BLOB_BYTES: usize = 2 + MAX_EVENTS * RECORD_BYTES;

/// Store format version. A blob that does not start with this byte is not ours.
const STORE_VERSION: u8 = 1;

/// Settings key the whole event store is written under.
const STORE_KEY: &str = "calendar.events.v1";

/// WHY the settings store (`settings_get`/`settings_set`) and not a VFS file:
/// the agenda is one flat array of equal-sized records, so it already is a small
/// binary value. Writing it as a settings value routes the whole store through
/// `settings::set`, which serialises and commits the store to persistent storage
/// (`kernel/src/settings.rs:167`), and the 482-byte blob fits the value cap the
/// kernel enforces. A VFS file would buy nothing -- the app would still read and
/// rewrite the entire blob -- and it would need a namespaced path that does not
/// exist before the first save. No new syscall and no kernel or SDK change is
/// involved: this is the store the platform already gives an unprivileged app
/// for exactly this shape of data.
const STORE_CAP_READ: usize = 4;
const STORE_CAP_WRITE: usize = 5;

/// Earliest and latest years the 2-byte year field can represent. The stepper
/// starts at the floor rather than at a year nobody chose.
const STORE_YEAR_MIN: u16 = 1970;
const STORE_YEAR_MAX: u16 = 9999;

/// One calendar event as it is stored: a civil date, a wall-clock time and a
/// bounded title. No category and no status are invented for a record that never
/// carried them.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct EventRecord {
    year: u16,
    month: u8,
    day: u8,
    hour: u8,
    minute: u8,
    title: [u8; EVENT_TITLE_MAX],
    title_len: u8,
}

impl EventRecord {
    const EMPTY: EventRecord = EventRecord {
        year: 0,
        month: 0,
        day: 0,
        hour: 0,
        minute: 0,
        title: [0; EVENT_TITLE_MAX],
        title_len: 0,
    };

    fn from_parts(year: u16, month: u8, day: u8, hour: u8, minute: u8, title: &[u8]) -> Self {
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

    /// True when the record holds a date the store can actually hand back.
    fn valid(&self) -> bool {
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

    fn on_day(&self, year: u16, month: u8, day: u8) -> bool {
        self.year == year && self.month == month && self.day == day
    }

    fn encode(&self, out: &mut [u8]) {
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

    /// Decodes one record, rejecting anything that is not a date, a time and a
    /// printable title the store could have written.
    fn decode(bytes: &[u8]) -> Option<EventRecord> {
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

    fn title_bytes(&self) -> &[u8] {
        &self.title[..self.title_len as usize]
    }
}

/// What the store's last contact with the platform storage actually did.
///
/// WHY an enum rather than a "success flag" and a message: the agenda, the logs
/// and the empty-state card must all say the same thing about storage, and two
/// parallel values can disagree with each other. One value is turned into words
/// in exactly one place.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum StoreState {
    /// The store has not been read yet.
    Untouched,
    /// The settings store holds no calendar key, so there is nothing to load.
    Empty,
    /// Events were read back out of the settings store.
    Loaded,
    /// The kernel refused the read.
    ReadRefused,
    /// The stored bytes were not a store this app wrote.
    Invalid,
    /// The last write was acknowledged.
    Written,
    /// The kernel refused the write.
    WriteRefused,
    /// The store is at its capacity and refused another event.
    Full,
}

impl StoreState {
    /// The words this state is reported in, to the log and to the screen.
    fn as_str(self) -> &'static str {
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

/// Encodes the whole store into one settings value and writes it.
///
/// WHY the whole blob is rewritten every time: the store is small enough that a
/// record-at-a-time append would need a cursor the settings store does not have,
/// and a whole-value write leaves no half-updated record behind.
fn write_store(records: &[EventRecord; MAX_EVENTS], len: usize) -> bool {
    let mut blob = [0u8; STORE_BLOB_BYTES];
    blob[0] = STORE_VERSION;
    blob[1] = len as u8;
    for i in 0..len {
        let start = 2 + i * RECORD_BYTES;
        records[i].encode(&mut blob[start..start + RECORD_BYTES]);
    }
    std::settings_set(STORE_CAP_WRITE, STORE_KEY, &blob).is_ok()
}

/// The agenda: bounded records plus an honest account of the last load or save.
#[derive(Clone, Copy)]
pub struct EventStore {
    records: [EventRecord; MAX_EVENTS],
    len: u8,
    state: StoreState,
}

impl EventStore {
    const EMPTY: EventStore = EventStore {
        records: [EventRecord::EMPTY; MAX_EVENTS],
        len: 0,
        state: StoreState::Untouched,
    };

    /// Reads the store back out of the platform settings store.
    ///
    /// WHY the whole blob is rejected when any record fails validation: the blob
    /// is written as a single settings value, so a valid store never splits
    /// across reads. Accepting a prefix would present a partial calendar as the
    /// stored one, so the load reports what it actually has instead.
    fn load() -> Self {
        let mut store = EventStore::EMPTY;
        let mut blob = [0u8; STORE_BLOB_BYTES];
        match std::settings_get(STORE_CAP_READ, STORE_KEY, &mut blob) {
            Ok(0) => store.state = StoreState::Empty,
            Ok(n) => {
                let mut records = [EventRecord::EMPTY; MAX_EVENTS];
                let mut count = 0usize;
                let mut rejected = false;
                if n == 2 || n > STORE_BLOB_BYTES || blob[0] != STORE_VERSION {
                    rejected = true;
                } else {
                    let header_count = blob[1] as usize;
                    if header_count > MAX_EVENTS || n != 2 + header_count * RECORD_BYTES {
                        rejected = true;
                    } else {
                        for i in 0..header_count {
                            let start = 2 + i * RECORD_BYTES;
                            match EventRecord::decode(&blob[start..start + RECORD_BYTES]) {
                                Some(rec) => {
                                    records[i] = rec;
                                    count += 1;
                                }
                                None => {
                                    rejected = true;
                                    break;
                                }
                            }
                        }
                    }
                }
                if rejected {
                    store.state = StoreState::Invalid;
                } else if count == 0 {
                    store.state = StoreState::Empty;
                } else {
                    store.records = records;
                    store.len = count as u8;
                    store.state = StoreState::Loaded;
                }
            }
            Err(_) => store.state = StoreState::ReadRefused,
        }
        store
    }

    fn len(&self) -> usize {
        self.len as usize
    }

    fn is_full(&self) -> bool {
        self.len() >= MAX_EVENTS
    }

    /// Appends one event and rewrites the whole blob.
    ///
    /// WHY it refuses instead of overwriting the oldest record: a bounded store
    /// that silently dropped an event would show a calendar that is missing
    /// something its owner saved, so the refusal is reported instead.
    ///
    /// WHY an event only joins the list after storage acknowledged the write:
    /// otherwise the agenda would show an entry the platform never took, one that
    /// would be gone on the next launch while still being drawn. The in-memory
    /// list is exactly what the settings store holds.
    fn add(&mut self, rec: EventRecord) -> bool {
        if self.is_full() {
            self.state = StoreState::Full;
            return false;
        }
        let mut records = self.records;
        let len = self.len() + 1;
        records[len - 1] = rec;
        if write_store(&records, len) {
            self.records = records;
            self.len = len as u8;
            self.state = StoreState::Written;
            true
        } else {
            self.state = StoreState::WriteRefused;
            false
        }
    }

    /// Collects one day's events, earliest first.
    fn events_on(
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
        // Insertion sort by wall-clock time: the agenda reads earliest first,
        // which is the order a day is lived in.
        for i in 1..n {
            let mut j = i;
            while j > 0
                && (out[j].hour, out[j].minute) < (out[j - 1].hour, out[j - 1].minute)
            {
                out.swap(j, j - 1);
                j -= 1;
            }
        }
        n
    }
}

/// The selected day's events, built once per repaint and handed to the renderer.
#[derive(Clone, Copy)]
struct AgendaView {
    year: u16,
    month: u8,
    day: u8,
    items: [EventRecord; MAX_EVENTS],
    len: usize,
}

impl AgendaView {
    const EMPTY: AgendaView = AgendaView {
        year: 0,
        month: 0,
        day: 0,
        items: [EventRecord::EMPTY; MAX_EVENTS],
        len: 0,
    };
}

/// Which step of the create-event flow is on screen.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Flow {
    /// No flow: the month grid and the agenda are interactive.
    None,
    /// Title entry on the on-screen keyboard.
    Title,
    /// Date entry on the year/month/day steppers.
    Date,
    /// Time entry, then commit.
    Time,
}

/// Returns true if a Gregorian year is a leap year.
fn is_leap_year(year: u32) -> bool {
    (year % 4 == 0 && year % 100 != 0) || (year % 400 == 0)
}

/// Returns the number of days in a given month.
fn days_in_month(year: u32, month: u8) -> u8 {
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
fn day_of_week(mut year: u32, mut month: u8, day: u8) -> u8 {
    if month < 3 {
        month += 12;
        year -= 1;
    }
    let k = year % 100;
    let j = year / 100;
    let h = (day as u32 + (13 * (month as u32 + 1)) / 5 + k + k / 4 + j / 4 + 5 * j) % 7;
    // Zeller: 0 = Saturday, 1 = Sunday, 2 = Monday, ...
    // Convert to Sunday = 0
    ((h + 6) % 7) as u8
}

/// Reads the unsigned decimal integer that follows `key` in a JSON payload.
///
/// WHY: the RTC syscall returns a compact JSON document. Reading the fields the
/// kernel actually wrote is the only way to know the civil date; assuming a
/// default date would present a fabricated "today".
fn json_field(payload: &[u8], key: &[u8]) -> Option<u32> {
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

/// Production Sovereign Calendar state manager.
pub struct CalendarApp {
    current_year: u32,
    current_month: u8,
    current_day: u8,
    current_hour: u8,
    current_minute: u8,
    notified_mask: u16,
    total_events: u32,
    rtc_synchronized: bool,
    sector_encrypted: bool,
    /// Whether a real civil date has been read from the hardware RTC.
    date_known: bool,
    /// The day the month grid highlights and the agenda lists.
    ///
    /// WHY it is separate from `current_day`: the RTC day is what the kernel
    /// reported, while a tap on a grid cell is a day the user chose. The
    /// selection starts at the RTC day when there is one and can move; when the
    /// RTC is refused it stays `None` instead of standing in a day nobody
    /// observed.
    selected_day: Option<u8>,
    /// Events read from, and written to, the platform settings store.
    store: EventStore,
    /// The create-event step currently on screen.
    flow: Flow,
    /// Title being typed, bounded by `EVENT_TITLE_MAX`.
    title: [u8; EVENT_TITLE_MAX],
    title_len: usize,
    /// Date being picked. Every field starts unset so no date is ever shown, or
    /// committed, that the user did not enter.
    edit_year: Option<u16>,
    edit_month: Option<u8>,
    edit_day: Option<u8>,
    edit_hour: Option<u8>,
    edit_minute: Option<u8>,
    /// What the last commit actually did, in words.
    commit_status: &'static str,
}

impl CalendarApp {
    pub const fn new() -> Self {
        Self {
            // WHY: no date is known until the hardware RTC answers. Seed with
            // zero and report the date as unknown rather than shipping a
            // hardcoded 2026-09-21 that looks like an observed date.
            current_year: 0,
            current_month: 0,
            current_day: 0,
            current_hour: 0,
            current_minute: 0,
            notified_mask: 0,
            total_events: 0,
            rtc_synchronized: false,
            sector_encrypted: false,
            date_known: false,
            selected_day: None,
            store: EventStore::EMPTY,
            flow: Flow::None,
            title: [0; EVENT_TITLE_MAX],
            title_len: 0,
            edit_year: None,
            edit_month: None,
            edit_day: None,
            edit_hour: None,
            edit_minute: None,
            commit_status: "NO EVENT CREATED YET",
        }
    }

    /// The month the grid draws, plus its first weekday and its length.
    ///
    /// WHY the zero sentinel when no date is known: there is no substitute
    /// month. `get_calendar_pixel` states the absence instead of laying out a
    /// stand-in that would look like an observed month.
    fn display_params(&self) -> (u32, u8, u8, u8) {
        if self.date_known {
            (
                self.current_year,
                self.current_month,
                day_of_week(self.current_year, self.current_month, 1),
                days_in_month(self.current_year, self.current_month),
            )
        } else {
            (0, 0, 0, 0)
        }
    }

    /// The selected day's events, earliest first.
    fn agenda_view(&self) -> AgendaView {
        if !self.date_known {
            return AgendaView::EMPTY;
        }
        let Some(day) = self.selected_day else {
            return AgendaView::EMPTY;
        };
        let mut view = AgendaView::EMPTY;
        view.year = self.current_year as u16;
        view.month = self.current_month;
        view.day = day;
        let mut items = [EventRecord::EMPTY; MAX_EVENTS];
        view.len = self.store.events_on(view.year, view.month, view.day, &mut items);
        view.items = items;
        view
    }

    /// The section title for the day the agenda is showing.
    ///
    /// WHY it is not always "TODAY'S AGENDA": once the grid selection moves off
    /// the RTC day, that word would claim a day nobody said was today, so the
    /// title names the date it is actually listing.
    fn agenda_title_into<'a>(&self, out: &'a mut [u8; 16]) -> &'a [u8] {
        if !self.date_known || self.selected_day.is_none() {
            return b"NO DAY SELECTED";
        }
        let day = self.selected_day.unwrap_or(0);
        if day == self.current_day {
            out[..14].copy_from_slice(b"TODAY'S AGENDA");
            &out[..14]
        } else {
            let n = fmt_date(out, self.current_year as u16, self.current_month, day);
            &out[..n]
        }
    }

    /// Appends one byte to the title being typed, refusing to overflow.
    fn push_title(&mut self, b: u8) -> bool {
        if self.title_len >= EVENT_TITLE_MAX {
            return false;
        }
        self.title[self.title_len] = b;
        self.title_len += 1;
        true
    }

    /// Synchronizes the internal civil clock with the hardware RTC.
    ///
    /// Returns `true` only when a real date was read and parsed from the RTC
    /// payload.
    pub fn sync_rtc(&mut self) -> bool {
        let mut time_buf = [0u8; 64];
        // WHY: the previous version reported a verified civil clock on any
        // non-empty payload without parsing a single field out of it. Parse the
        // fields the kernel actually returned and only then claim the date.
        match std::rtc_get_time(&mut time_buf) {
            Ok(n) if n > 0 => {
                let payload = &time_buf[..n];
                std::log(0, "[CALENDAR APP] RTC payload as reported by the kernel:").ok();
                std::log(0, core::str::from_utf8(payload).unwrap_or("<non-utf8>")).ok();

                let y = json_field(payload, b"\"year\":");
                let m = json_field(payload, b"\"month\":");
                let d = json_field(payload, b"\"day\":");
                let h = json_field(payload, b"\"hour\":");
                let min = json_field(payload, b"\"minute\":");
                match (y, m, d) {
                    (Some(y), Some(m), Some(d))
                        if y >= 1970 && (1..=12).contains(&m) && (1..=31).contains(&d) =>
                    {
                        self.current_year = y;
                        self.current_month = m as u8;
                        self.current_day = d as u8;
                        if let Some(h) = h {
                            self.current_hour = (h & 0xFF) as u8;
                        }
                        if let Some(min) = min {
                            self.current_minute = (min & 0xFF) as u8;
                        }
                        self.date_known = true;
                        self.rtc_synchronized = true;
                        std::log(
                            0,
                            "[CALENDAR APP] Hardware RTC synchronized: civil date read from the RTC payload.",
                        )
                        .ok();
                        return true;
                    }
                    _ => {
                        std::log(
                            0,
                            "[CALENDAR APP] RTC payload carried no usable civil date; current date unknown.",
                        )
                        .ok();
                    }
                }
            }
            Ok(_) => {
                std::log(0, "[CALENDAR APP] RTC returned an empty payload; current date unknown.").ok();
            }
            Err(_) => {
                // WHY: rtc_get_time requires the system-control authority, which
                // this app does not hold. Report the refusal.
                std::log(
                    0,
                    "[CALENDAR APP] RTC query refused by the kernel; current date unknown.",
                )
                .ok();
            }
        }
        false
    }

    #[allow(dead_code)]
    pub fn parse_rtc_date(&mut self, buf: &[u8], n: usize) {
        if n < 10 {
            return;
        }
        let d = |a: u8| {
            if a.is_ascii_digit() {
                (a - b'0') as u32
            } else {
                10
            }
        };
        let y = [buf[0], buf[1], buf[2], buf[3]]
            .iter()
            .fold(0u32, |v, b| v * 10 + d(*b));
        let m = d(buf[5]) as u8 * 10 + d(buf[6]) as u8;
        let day = d(buf[8]) as u8 * 10 + d(buf[9]) as u8;
        if y > 1970 && m >= 1 && m <= 12 && day >= 1 && day <= 31 {
            self.current_year = y;
            self.current_month = m;
            self.current_day = day;
            self.date_known = true;
        }
    }

    /// Checks loaded events against current RTC date and time, posting a system
    /// notification when a scheduled event time is reached.
    pub fn check_due_notifications(&mut self) {
        if !self.date_known {
            return;
        }
        for i in 0..self.store.len() {
            if i >= 16 {
                break;
            }
            let rec = &self.store.records[i];
            if (self.notified_mask & (1 << i)) == 0
                && rec.year == self.current_year as u16
                && rec.month == self.current_month
                && rec.day == self.current_day
                && rec.hour == self.current_hour
                && rec.minute <= self.current_minute
            {
                self.notified_mask |= 1 << i;
                let _ = std::notification_post("app_calendar", "calendar|Calendar Alert|Scheduled event reached");
                std::log(0, "[CALENDAR APP] Triggered notification for scheduled event.").ok();
            }
        }
    }

    /// Inserts a scheduled event and encrypts in persistent storage.
    ///
    /// Returns `true` only when the agenda sector was actually written.
    pub fn schedule_event(&mut self, event_name: &'static str) -> bool {
        // 1. Encrypt Updated Agenda in Storage Vault (Sector 203)
        let mut agenda_sector = [0u8; 512];
        let copy_len = event_name.len().min(128);
        agenda_sector[..copy_len].copy_from_slice(&event_name.as_bytes()[..copy_len]);
        let mut iv = [0u8; 16];
        if std::get_random(&mut iv).is_err() {
            std::log(
                0,
                "[CALENDAR APP] Refusing to encrypt agenda event without cryptographic TRNG entropy.",
            )
            .ok();
            return false;
        }
        // WHY: report the vault outcome. The old code stayed silent on refusal,
        // leaving the event looking scheduled when nothing was stored.
        match std::vault_encrypt_sector(203, &mut agenda_sector, &iv) {
            Ok(_) => {
                self.sector_encrypted = true;
                // WHY: the syscall returns only success/failure, never the
                // cipher it used. Naming "AES-XTS/HMAC" here invented a crypto
                // detail this app cannot observe, so report only the outcome the
                // vault actually acknowledged.
                std::log(
                    0,
                    "[CALENDAR APP] Sector 203: Updated agenda sealed and persisted by the storage vault.",
                )
                .ok();
            }
            Err(_) => {
                std::log(
                    0,
                    "[CALENDAR APP] Sector 203: vault write refused by the kernel; event not persisted.",
                )
                .ok();
            }
        }

        // 2. Acoustic Reminder Chime (A-Major: 440Hz -> 554Hz -> 659Hz).
        // WHY: the audio service refuses tone requests without its capability,
        // so only report the chime when the tones were actually accepted.
        let mut tones_played = 0u32;
        for freq in [440u32, 554, 659] {
            if std::audio_play_tone(freq).is_ok() {
                tones_played += 1;
            }
        }
        let _ = std::audio_silence();
        let _ = std::audio_haptic_click();
        if tones_played == 3 {
            std::log(
                0,
                "[CALENDAR APP] Agenda reminder chime emitted (A-Major: 440Hz -> 554Hz -> 659Hz).",
            )
            .ok();
        } else {
            std::log(
                0,
                "[CALENDAR APP] Reminder chime not emitted: the audio service refused the tone requests.",
            )
            .ok();
        }
        self.sector_encrypted
    }
}

// -----------------------------------------------------------------------------
// Visual Design Tokens
// -----------------------------------------------------------------------------
const BG_DARK: u32 = 0xFF070B12;
const SURFACE_DARK: u32 = 0xFF111B2B;
const SURFACE_RAISED: u32 = 0xFF17243A;
const BORDER_SUBTLE: u32 = 0xFF26354D;
const ACCENT_SKY: u32 = 0xFF31C4FF;
#[allow(dead_code)]
const SUCCESS_EMERALD: u32 = 0xFF56D68A;
const DANGER_CORAL: u32 = 0xFFFF6B6B;

// Empty-state action button: "+ New Event", centred on the 720px surface.
// The geometry is derived from the label so the pill is always wider than its
// own text: label 236px at scale 2 plus 36px of margin per side.
const ACTION_BTN_LABEL: &[u8] = b"+ New Event";
const ACTION_BTN_SCALE: usize = 2;
const ACTION_BTN_W: usize = 308;
const ACTION_BTN_H: usize = 56;
const ACTION_BTN_X0: usize = (720 - ACTION_BTN_W) / 2;
const ACTION_BTN_Y0: usize = 800;
const ACTION_BTN_X1: usize = ACTION_BTN_X0 + ACTION_BTN_W;
const ACTION_BTN_Y1: usize = ACTION_BTN_Y0 + ACTION_BTN_H;
const TEXT_PRIMARY: u32 = 0xFFF4F7FB;
const TEXT_MUTED: u32 = 0xFF9AA8BC;

// -----------------------------------------------------------------------------
// Embedded 8x8 ASCII Font (Characters 32 ' ' through 126 '~')
// -----------------------------------------------------------------------------
// Typography provided by interos_std::design::font

/// Rasterises one text run into the pixel at `(x, y)`.
///
/// WHY the outline face comes first: the header title is scale 3 and the month
/// pill, the weekday letters, the day numerals, the section title, the
/// empty-state headline and action, the sheet's title and buttons, the
/// on-screen keyboard and the steppers' values and signs are scale 2, so on the
/// bitmap face each of them is a 16x24 cell magnified by an integer -- a soft
/// ramp where an edge belongs. `outline_text::place_run` rasterises the real outline at the
/// em that matches the cell's cap height (17 rows at scale 1, so
/// 17*N*2048/1456 px per em), and answers for the pixel itself. When it declines
/// -- a run at scale 1, whose cell is already at its native size, a run wider
/// than the column the caller reserved, a glyph whose ink would leave that
/// column, or a weight this app draws bold -- the bitmap face below draws the run
/// exactly as it did before, so a refusal costs the edge quality and never the
/// text.
///
/// WHY the box test comes first, and why it is the UNION box: the run's line box
/// is not the bitmap cell. At the em that matches a scale-2 cap the outline's
/// ascent reaches 12 rows above the cell origin and its descent 3 rows past the
/// cell bottom, so a gate clipped to `GLYPH_H * scale` -- which is all this
/// function used to test -- would silently slice the glyphs a magnified cell
/// could not reach. `outline_text::run_box` is the union of both faces' extents,
/// so the gate admits every pixel either face can ink and no others. Gating
/// before the placement also means a run is only rasterised on a pixel it can
/// ink, so the shared coverage buffer is written once per run rather than once
/// per pixel.
///
/// WHY the run answers alone when it is placed: the two faces are alternatives,
/// not layers. Falling through to the bitmap face after an outline run had
/// already inked a pixel would double-ink every partially covered edge, which is
/// exactly what the first attempt at this in the launcher drew: an outline glyph
/// with a hard-edged bitmap ghost one pixel off, on every string. The face is
/// chosen from the run's own arguments before `(x, y)` is consulted, so it is the
/// same choice for every pixel of the run.
#[inline(always)]
fn test_text(
    x: usize,
    y: usize,
    x0: usize,
    y0: usize,
    scale: usize,
    text: &[u8],
    max_w: usize,
    color: u32,
    bg: u32,
) -> Option<u32> {
    let (bx, bt, bw, bh) = outline_text::run_box(x0, y0, scale, text);
    if x < bx || x - bx >= bw || y < bt || y - bt >= bh {
        return None;
    }
    if let Some(run) = outline_text::place_run(x0, y0, scale, text, Some(max_w), false) {
        return run.pixel(x, y, color, bg);
    }
    std::design::font::test_text_aa(x, y, x0, y0, scale, text, color, bg, false)
}

/// Left x that centres `text` at `scale` inside `[x0, x0 + available)`.
/// WHY: the face is proportional (digits advance 11px at scale 1, letters 5-16),
/// so a fixed 8px character cell mis-centres every run that is not exactly that
/// wide. This measures the real advance widths instead.
///
/// WHY the measured width is the DRAWN face's and not `font::measure`'s: the run
/// is placed through `test_text`, which prefers the outline face at scale 2 and
/// above, and the outline face advances by its own table rather than the bitmap
/// cell's. Both widths are decided by `outline_text::drawn_advance`, which
/// mirrors `place_run`'s refusals, so a label is centred on the width of the run
/// that is actually inked. On the bitmap fallback the answer is
/// `font::measure`, which is what the bitmap rasteriser steps by. The empty
/// state's headline sits 6px left on the advance box, and 4.5px left on the
/// ink a capture resolves: `k2_cal.ppm` showed ink x 164..547 against a card
/// centre of 360.
#[inline(always)]
fn centered_x(text: &[u8], scale: usize, x0: usize, available: usize) -> usize {
    let w = outline_text::drawn_advance(text, scale, Some(available))
        .unwrap_or_else(|| std::design::font::measure(text, scale));
    // 0 rather than an underflow when the run is at least as wide as its box,
    // which is what `font::center_x` does for the same situation.
    x0 + if w >= available { 0 } else { (available - w) / 2 }
}

fn get_month_abbr(month: u8) -> &'static [u8] {
    match month {
        1 => b"JAN",
        2 => b"FEB",
        3 => b"MAR",
        4 => b"APR",
        5 => b"MAY",
        6 => b"JUN",
        7 => b"JUL",
        8 => b"AUG",
        9 => b"SEP",
        10 => b"OCT",
        11 => b"NOV",
        12 => b"DEC",
        _ => b"CAL",
    }
}

/// Writes `day MON year` (e.g. `14 OCT 2026`) into `out` and returns its length.
///
/// WHY a fixed buffer: `#![no_std]` has no allocator, so every formatted line is
/// written into a caller-owned array rather than into a `String`.
fn fmt_date(out: &mut [u8], year: u16, month: u8, day: u8) -> usize {
    if out.len() < 11 || month < 1 || month > 12 {
        return 0;
    }
    let abbr = get_month_abbr(month);
    let mut n = if day >= 10 {
        out[0] = b'0' + day / 10;
        out[1] = b'0' + day % 10;
        2
    } else {
        out[0] = b'0' + day;
        1
    };
    out[n] = b' ';
    n += 1;
    out[n..n + 3].copy_from_slice(abbr);
    n += 3;
    out[n] = b' ';
    n += 1;
    let mut y = year as u32;
    let mut digits = [0u8; 5];
    let mut d = 0usize;
    while y > 0 {
        digits[d] = b'0' + (y % 10) as u8;
        y /= 10;
        d += 1;
    }
    if d == 0 {
        digits[0] = b'0';
        d = 1;
    }
    while d > 0 {
        out[n] = digits[d - 1];
        n += 1;
        d -= 1;
    }
    n
}

/// Writes `HH:MM` into `out` and returns its length.
fn fmt_time(out: &mut [u8], hour: u8, minute: u8) -> usize {
    if out.len() < 5 {
        return 0;
    }
    out[0] = b'0' + hour / 10;
    out[1] = b'0' + hour % 10;
    out[2] = b':';
    out[3] = b'0' + minute / 10;
    out[4] = b'0' + minute % 10;
    5
}

/// Writes `<n> EVENTS` into `out` and returns its length.
fn fmt_count(out: &mut [u8], n: usize) -> usize {
    let mut digits = [0u8; 3];
    let mut d = 0usize;
    let mut v = n.min(999);
    while v > 0 {
        digits[d] = b'0' + (v % 10) as u8;
        v /= 10;
        d += 1;
    }
    if d == 0 {
        digits[0] = b'0';
        d = 1;
    }
    let mut len = 0usize;
    while d > 0 {
        out[len] = digits[d - 1];
        len += 1;
        d -= 1;
    }
    out[len] = b' ';
    len += 1;
    let tail = b"EVENTS";
    if out.len() < len + tail.len() {
        return 0;
    }
    out[len..len + tail.len()].copy_from_slice(tail);
    len + tail.len()
}

const W: usize = 720;
const H: usize = 1440;
const CHUNK: usize = 4096;

// Month grid metrics
const COL_W: usize = 88;
const COL_PITCH: usize = 97;
const COL_X0: usize = 22;
// WHY ROW_H is 48: the bitmap face is GLYPH_H (24) rows, so a scale-2 day
// numeral owns 24 * 2 = 48 rows. A cell shorter than that cannot contain the
// numeral no matter where it is placed -- the old ROW_H of 44 clipped every one
// of the 42 numerals and no digit ever showed its base.
const ROW_H: usize = 48;
// WHY ROW_PITCH is 52: it leaves a 4px gutter between adjacent cells so
// neighbouring rows never share a pixel, and 6 rows * 52 = 312 lands the grid
// at 144..456, still clear of the agenda band that starts at 470.
const ROW_PITCH: usize = 52;
const GRID_ROWS: usize = 6;
const GRID_START_Y: usize = 144;

// Header / weekday band metrics. WHY 96: the status bar and header own
// y 0..96 (their rule is 94..96), and the grid owns 144..456, so the weekday
// run has exactly one full scale-2 line box (96..144) available in the gap.
const WEEKDAY_Y0: usize = 96;
const WEEKDAY_SCALE: usize = 2;

// WHY these exist: they are the explicit no-date state drawn when the hardware
// RTC yields no civil date. They name the absence instead of standing in a
// month that was never observed.
const NO_DATE_PILL: &[u8] = b"NO DATE";
const NO_DATE_HEADLINE: &[u8] = b"NO CIVIL DATE";
const NO_DATE_DETAIL: &[u8] = b"RTC date unavailable";

// -----------------------------------------------------------------------------
// Create-event sheet geometry
// -----------------------------------------------------------------------------
// WHY the sheet reuses the agenda band (y 536..1360): a creation flow replaces
// the agenda while it is open, so no existing band moves, no colour changes and
// no new surface is needed. Everything below lives inside the same
// x 24..696 card the empty state already occupies.

/// The sheet card, in the same rect the empty-state card draws.
const SHEET_X0: usize = 24;
const SHEET_X1: usize = 696;
const SHEET_Y0: usize = 546;
/// WHY 1056: it covers the deepest flow view (the keyboard's fourth row ends at
/// 1028) with room for the sheet's own border.
const SHEET_Y1: usize = 1056;

const SHEET_TITLE_Y: usize = 566;
const SHEET_STEP_Y: usize = 622;

/// Title field of the title step.
const TITLE_FIELD_X0: usize = 44;
const TITLE_FIELD_X1: usize = 676;
const TITLE_FIELD_Y0: usize = 660;
const TITLE_FIELD_Y1: usize = 728;
const TITLE_TEXT_Y: usize = 672;

/// On-screen keyboard.
///
/// WHY a keyboard at all: the guest has no other way to hand this app a string,
/// and an event needs a title the user typed. The keys are drawn from the same
/// rects the tap handler tests, so a key press is visible exactly where it
/// lands.
///
/// WHY every key sits inside the sheet card (x 24..696): a control drawn outside
/// the card it belongs to would be an invisible hotspot, and one drawn inside a
/// card a tap ignores would be dead. 13 keys of 46px with a 2px gutter fill the
/// card's inner width exactly (26 + 12 * 48 + 46 = 648).
const KB_Y0: usize = 764;
const KB_ROW_H: usize = 60;
const KB_ROW_PITCH: usize = 68;
/// Letters per row.
const KB_LETTER_COLS: usize = 13;
const KB_LETTER_X0: usize = 26;
const KB_LETTER_KEY_W: usize = 46;
const KB_LETTER_PITCH: usize = 48;
const KB_DIGIT_X0: usize = 26;
const KB_DIGIT_KEY_W: usize = 60;
const KB_DIGIT_PITCH: usize = 64;
const KB_DIGIT_COLS: usize = 10;
/// Fourth row: SPACE, DEL, NEXT. Its x ranges are written out because the three
/// keys are deliberately unequal.
const KB_SPACE_X0: usize = 26;
const KB_SPACE_X1: usize = 330;
const KB_DEL_X0: usize = 334;
const KB_DEL_X1: usize = 510;
const KB_NEXT_X0: usize = 514;
const KB_NEXT_X1: usize = 694;

/// The `-` and `+` key rects of one stepper box, so the renderer and the hit
/// test cannot disagree about where the buttons are.
fn step_btn_rects(x0: usize, x1: usize) -> ((usize, usize, usize, usize), (usize, usize, usize, usize)) {
    let y0 = STEP_BTN_Y0;
    let y1 = STEP_BTN_Y1;
    (
        (x0 + 16, y0, x0 + 16 + STEP_BTN_W, y1),
        (x1 - 16 - STEP_BTN_W, y0, x1 - 16, y1),
    )
}

/// Stepper boxes: a labelled value with a -/+ pair under it.
const STEP_Y0: usize = 660;
const STEP_Y1: usize = 824;
const STEP_LABEL_Y: usize = 674;
const STEP_VALUE_Y: usize = 700;
const STEP_BTN_Y0: usize = 764;
const STEP_BTN_Y1: usize = 812;
const STEP_BTN_W: usize = 72;

/// Date step: year, month, day.
const DATE_BOXES: [(usize, usize); 3] = [(44, 236), (252, 444), (460, 652)];
/// Time step: hour, minute.
const TIME_BOXES: [(usize, usize); 2] = [(44, 344), (376, 676)];
const SAVE_BTN_X0: usize = 44;
const SAVE_BTN_X1: usize = 386;
const CANCEL_BTN_X0: usize = 400;
const CANCEL_BTN_X1: usize = 652;
const FLOW_BTN_Y0: usize = 844;
const FLOW_BTN_Y1: usize = 908;
/// Status line under the flow's controls.
const FLOW_STATUS_Y: usize = 928;

/// The month grid a tap resolves against.
///
/// WHY the cell rects are the renderer's own: `COL_X0 + c * COL_PITCH` over
/// `COL_W` and `GRID_START_Y + r * ROW_PITCH` over `ROW_H` are exactly the rects
/// `get_calendar_pixel` draws, so a tap lands on a cell that is really there.
fn grid_cell_at(x: usize, y: usize, start_dow: u8, num_days: u8) -> Option<u8> {
    if y < GRID_START_Y || y >= GRID_START_Y + GRID_ROWS * ROW_PITCH {
        return None;
    }
    let r = (y - GRID_START_Y) / ROW_PITCH;
    if y - GRID_START_Y >= r * ROW_PITCH + ROW_H {
        return None;
    }
    for c in 0..7 {
        let col_x = COL_X0 + c * COL_PITCH;
        if x >= col_x && x < col_x + COL_W {
            let slot_idx = r * 7 + c;
            let day_num = slot_idx as i32 - start_dow as i32 + 1;
            if (1..=num_days as i32).contains(&day_num) {
                return Some(day_num as u8);
            }
            return None;
        }
    }
    None
}

/// Rect of one keyboard key, or the special row's three unequal keys.
fn key_rect(row: usize, col: usize) -> Option<(usize, usize, usize, usize)> {
    let y0 = KB_Y0 + row * KB_ROW_PITCH;
    let y1 = y0 + KB_ROW_H;
    match row {
        0 | 1 => {
            if col >= KB_LETTER_COLS {
                return None;
            }
            let x0 = KB_LETTER_X0 + col * KB_LETTER_PITCH;
            Some((x0, y0, x0 + KB_LETTER_KEY_W, y1))
        }
        2 => {
            if col >= KB_DIGIT_COLS {
                return None;
            }
            let x0 = KB_DIGIT_X0 + col * KB_DIGIT_PITCH;
            Some((x0, y0, x0 + KB_DIGIT_KEY_W, y1))
        }
        3 => match col {
            0 => Some((KB_SPACE_X0, y0, KB_SPACE_X1, y1)),
            1 => Some((KB_DEL_X0, y0, KB_DEL_X1, y1)),
            2 => Some((KB_NEXT_X0, y0, KB_NEXT_X1, y1)),
            _ => None,
        },
        _ => None,
    }
}

/// The label a keyboard key shows.
fn key_label(row: usize, col: usize) -> &'static [u8] {
    const LETTERS: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ";
    const DIGITS: &[u8] = b"0123456789";
    match row {
        0 | 1 => {
            let idx = row * KB_LETTER_COLS + col;
            if idx >= LETTERS.len() {
                b""
            } else {
                &LETTERS[idx..idx + 1]
            }
        }
        2 => {
            if col >= DIGITS.len() {
                b""
            } else {
                &DIGITS[col..col + 1]
            }
        }
        3 => match col {
            0 => b"SPACE",
            1 => b"DEL",
            2 => b"NEXT",
            _ => b"",
        },
        _ => b"",
    }
}

/// Resolves a tap to a keyboard key, or `None` outside every drawn key.
fn keyboard_key_at(x: usize, y: usize) -> Option<(usize, usize)> {
    for row in 0..4 {
        for col in 0..KB_LETTER_COLS {
            if let Some((x0, y0, x1, y1)) = key_rect(row, col) {
                if x >= x0 && x < x1 && y >= y0 && y < y1 {
                    return Some((row, col));
                }
            }
        }
    }
    None
}

#[inline(never)]
fn get_calendar_pixel(
    x: usize,
    y: usize,
    app: &CalendarApp,
    display_year: u32,
    display_month: u8,
    start_dow: u8,
    num_days: u8,
    agenda: &AgendaView,
) -> u32 {
    // 1. Status Bar & Header (y: 0..96)
    if y < 96 {
        if y >= 94 {
            return BORDER_SUBTLE;
        }

        // Standardized back button squircle (24..72, 24..72)
        if let Some(c) = std::design::back_button::pixel(x, y, TEXT_PRIMARY, BORDER_SUBTLE, SURFACE_RAISED) {
            return c;
        }

        // Header Title: "Calendar"
        // WHY the move and the full box: a scale-3 run needs GLYPH_H * 3 = 72
        // rows, so the old y0=36 box (36..108) ran straight through the header
        // band's bottom rule (94..96) and out of the band, and the 36..60 gate
        // drew 24 of its 72 rows. The run is centred on the same y=48 axis as
        // the back button and the pill, giving y0 = 48 - 36 = 12 and the box
        // 12..84, which clears the header band (0..94) untouched.
        if y >= 12 && y < 12 + std::design::font::line_height(3) {
            // WHY the column is 480 - 116: the title is left-aligned and the
            // month pill begins at x 480, so that is the room the header has
            // for it. "Calendar" measures 249px on the bitmap face at scale 3
            // and 247px on the outline face, so both clear it by 117px.
            if let Some(c) = test_text(x, y, 116, 12, 3, b"Calendar", 480 - 116, TEXT_PRIMARY, SURFACE_DARK) {
                return c;
            }
        }

        // Month & Year Pill Badge at top-right (x: 480..696, y: 20..76)
        // WHY the growth: the pill held a scale-2 run, so it must reserve
        // GLYPH_H * 2 = 48 rows plus its 4px padding, i.e. 56px. The old 28..68
        // box is 40px and kept only 28 of the run's 48 rows. 20..76 keeps the
        // same centre (y=48) as the back button.
        if x >= 480 && x < 696 && y >= 20 && y < 76 {
            // Pill border (radius 20)
            let rx = (x - 480).min(695 - x);
            let ry = (y - 20).min(75 - y);
            if rx < 20 && ry < 20 {
                let cdx = (20 - rx) as isize;
                let cdy = (20 - ry) as isize;
                if cdx * cdx + cdy * cdy >= 400 {
                    return SURFACE_DARK;
                }
            }

            // Month text: e.g. "OCT 2026"
            let mut month_str = [0u8; 8];
            if app.date_known {
                let abbr = get_month_abbr(display_month);
                month_str[0] = abbr[0];
                month_str[1] = abbr[1];
                month_str[2] = abbr[2];
                month_str[3] = b' ';
                let mut temp_y = display_year;
                month_str[7] = b'0' + (temp_y % 10) as u8; temp_y /= 10;
                month_str[6] = b'0' + (temp_y % 10) as u8; temp_y /= 10;
                month_str[5] = b'0' + (temp_y % 10) as u8; temp_y /= 10;
                month_str[4] = b'0' + (temp_y % 10) as u8;
            }

            // WHY: with no civil date read from the RTC there is no month to
            // name. The pill used to show a hardcoded "OCT 2026" stand-in; a
            // fabricated month badge must not sit next to the honest
            // "RTC date unavailable" card, so the badge states the absence.
            let pill_text: &[u8] = if app.date_known { &month_str[..] } else { NO_DATE_PILL };
            // WHY 216 is the column: it is the pill's own inner width (480..696),
            // which is the box the run is centred in. "OCT 2026" measures 180px
            // on the bitmap face at scale 2 and 181px on the outline face, so
            // both fit.
            let pill_x = centered_x(pill_text, 2, 480, 216);
            let pill_y0 = 20 + (56 - std::design::font::line_height(2)) / 2;
            if let Some(c) = test_text(x, y, pill_x, pill_y0, 2, pill_text, 216, ACCENT_SKY, SURFACE_RAISED) {
                return c;
            }

            if rx < 2 || ry < 2 {
                return BORDER_SUBTLE;
            }
            return SURFACE_RAISED;
        }

        return SURFACE_DARK;
    }

    // 2. Weekday Header (y: 96..144)
    // WHY the band is one full scale-2 line box: the bitmap face is GLYPH_H (24)
    // rows, so a scale-2 run owns 24 * 2 = 48 rows. The run sits at y0 = 96,
    // i.e. exactly 96..144 -- the whole gap between the header band (which ends
    // at 96, its rule occupying 94..96) and the grid (which starts at 144). The
    // old window was 114..134: 20 rows of a 48-row run, so only the top of each
    // letter was drawn.
    if y >= WEEKDAY_Y0 && y < WEEKDAY_Y0 + std::design::font::line_height(WEEKDAY_SCALE) {
        if app.date_known {
            const HEADERS: [&[u8]; 7] = [b"S", b"M", b"T", b"W", b"T", b"F", b"S"];
            for c in 0..7 {
                let col_x = 22 + c * 97;
                // WHY: the face is proportional. At scale 1 'S' advances 12px and
                // 'M'/'W' advance 16px, so the old hand-typed "- 8" cell pushed
                // every header right of its column centre (measured +3.5 for 'S',
                // +7.5 for 'M', +6.5 for 'W'). This measures the real widths.
                let text_x = centered_x(HEADERS[c], WEEKDAY_SCALE, col_x, COL_W);
                // WHY COL_W is the column: the letter is centred in the grid
                // column it labels, and the widest of them, 'M' and 'W',
                // measures 32px on the bitmap face at scale 2 and 36px on the
                // outline face, so both fit an 88px column.
                if let Some(color) =
                    test_text(x, y, text_x, WEEKDAY_Y0, WEEKDAY_SCALE, HEADERS[c], COL_W, TEXT_MUTED, BG_DARK)
                {
                    return color;
                }
            }
        } else {
            // WHY: with no civil date there is no month for these letters to
            // label. Drawing them would advertise a month grid that is not
            // there, so the header states the absence instead.
            let text_x = centered_x(NO_DATE_HEADLINE, WEEKDAY_SCALE, 0, W);
            // WHY W is the column: this headline replaces the seven letters and
            // is centred across the whole surface. It measures 286px on the
            // bitmap face at scale 2 and 277px on the outline face.
            if let Some(c) = test_text(x, y, text_x, WEEKDAY_Y0, WEEKDAY_SCALE, NO_DATE_HEADLINE, W, TEXT_MUTED, BG_DARK) {
                return c;
            }
        }
        return BG_DARK;
    }

    // 3. Month Calendar Grid (6 rows x 7 cols) (y: 144..456)
    if y >= GRID_START_Y && y < GRID_START_Y + GRID_ROWS * ROW_PITCH {
        // WHY: no civil date means no month. The previous code substituted
        // (2026, 10) and rendered a complete, weekday-correct October 2026 grid
        // while the card below honestly said the RTC date was unavailable. That
        // is invented data presented as observed, so the grid area now states the
        // absence in the same terms as the card instead of drawing any numeral.
        if !app.date_known {
            let detail_y = GRID_START_Y + GRID_ROWS * ROW_PITCH / 2;
            if let Some(c) = test_text(
                x,
                y,
                centered_x(NO_DATE_DETAIL, 1, 0, W),
                detail_y,
                1,
                NO_DATE_DETAIL,
                W,
                TEXT_MUTED,
                BG_DARK,
            ) {
                return c;
            }
            return BG_DARK;
        }

        let r = (y - GRID_START_Y) / ROW_PITCH;
        let cell_y = GRID_START_Y + r * ROW_PITCH;
        if y < cell_y + ROW_H {
            for c in 0..7 {
                let col_x = COL_X0 + c * COL_PITCH;
                if x >= col_x && x < col_x + COL_W {
                    let slot_idx = r * 7 + c;
                    let day_num = slot_idx as i32 - start_dow as i32 + 1;
                    if day_num >= 1 && day_num <= num_days as i32 {
                        // WHY the selection comes from the app and not from a
                        // parameter: a tap moves the selection, and a selection
                        // that only existed inside one render call would make
                        // the tap handler and the renderer disagree about which
                        // day is highlighted.
                        let is_current = app.selected_day == Some(day_num as u8);

                        // Rounded squircle (radius 10)
                        let rx = (x - col_x).min(col_x + COL_W - 1 - x);
                        let ry = (y - cell_y).min(cell_y + ROW_H - 1 - y);
                        if rx < 10 && ry < 10 {
                            let cdx = (10 - rx) as isize;
                            let cdy = (10 - ry) as isize;
                            if cdx * cdx + cdy * cdy >= 100 {
                                return BG_DARK;
                            }
                        }

                        // Day numeral text. WHY: the face is proportional, so a
                        // one- and a two-digit numeral have different widths
                        // (11px and 22px at scale 1) and both must be centred on
                        // the real measurement; the old fixed -8 / -16 cell
                        // pushed them right by 3px and 6px respectively.
                        let d = day_num as u8;
                        let text_color = if is_current { 0xFFFFFFFF } else { TEXT_PRIMARY };
                        // WHY the offset: a scale-2 run is 48 rows and the cell
                        // is now 48 rows, so it is centred at offset 0. Before,
                        // the cell was 44 rows and the run started at +14, which
                        // put its last 18 rows outside the cell.
                        let text_y = cell_y + (ROW_H - std::design::font::line_height(2)) / 2;

                        let text: [u8; 2] = if d < 10 {
                            [b'0' + d, 0]
                        } else {
                            [b'0' + d / 10, b'0' + d % 10]
                        };
                        let n = if d < 10 { 1 } else { 2 };
                        let text_x = centered_x(&text[..n], 2, col_x, COL_W);
                        // WHY COL_W is the column: the numeral is centred in the
                        // cell it falls in, and the widest, "31", measures 44px
                        // on the bitmap face at scale 2 and 46px on the outline
                        // face, so both fit.
                        let cell_bg = if is_current { 0xFF173552 } else { SURFACE_DARK };
                        if let Some(color) = test_text(x, y, text_x, text_y, 2, &text[..n], COL_W, text_color, cell_bg) {
                            return color;
                        }

                        // Border rim
                        if rx < 2 || ry < 2 {
                            return if is_current { ACCENT_SKY } else { BORDER_SUBTLE };
                        }

                        return cell_bg;
                    }
                }
            }
        }
        return BG_DARK;
    }

    // 4. Section Divider & Header (y: 470..530)
    if y >= 470 && y < 530 {
        if y >= 476 && y < 478 && x >= 24 && x < W - 24 {
            return BORDER_SUBTLE;
        }

        // Title: names the day the agenda below is listing.
        // WHY: a scale-2 run is GLYPH_H * 2 = 48 rows, so it owns a 48-row box.
        // At the old y0=498 that box ran to 546 and the band ended at 530, which
        // cut its last 16 rows and severed the base of every letter. y0=480 puts
        // the run at 480..528 -- 2px below the 476..478 divider and 2px inside
        // the band -- so the whole line box fits and nothing below is touched.
        let mut title_buf = [0u8; 16];
        let section_title = app.agenda_title_into(&mut title_buf);
        // WHY 522 is the column: the title is left-aligned at x 28 and the event
        // count pill begins at x 550, and their bands overlap, so that is the
        // room the band has for it. The longest title, "NO DAY SELECTED",
        // measures 362px on the bitmap face at scale 2 and 349px on the outline
        // face, so both fit.
        if y >= 480 && y < 480 + std::design::font::line_height(2) {
            if let Some(c) = test_text(x, y, 28, 480, 2, section_title, 550 - 28, TEXT_MUTED, BG_DARK) {
                return c;
            }
        }

        // Event Count Pill at x: 550..696, y: 492..522
        if x >= 550 && x < 696 && y >= 492 && y < 522 {
            let rx = (x - 550).min(695 - x);
            let ry = (y - 492).min(521 - y);
            if rx < 14 && ry < 14 {
                let cdx = (14 - rx) as isize;
                let cdy = (14 - ry) as isize;
                if cdx * cdx + cdy * cdy >= 196 {
                    return BG_DARK;
                }
            }

            // WHY the count is formatted from the day that is actually selected:
            // the pill used to read "EVENTS" whenever the static agenda had any
            // entry at all -- which it never did -- so it could only ever say
            // "0 EVENTS" or an unquantified "EVENTS". It now reports how many
            // events the selected day really has.
            let mut badge = [0u8; 9];
            let badge_n = fmt_count(&mut badge, agenda.len);
            let badge_str = &badge[..badge_n];
            // WHY centred on the measured width: the face is proportional, so the
            // old fixed x=576 was only correct for the one 93px string that
            // happened to be used; the run is also vertically centred in the
            // 30px pill now, since at y0=499 its 24 rows ran to 523, one past
            // the pill's 522.
            let badge_x = centered_x(badge_str, 1, 550, 146);
            let badge_y = 492 + (30 - std::design::font::line_height(1)) / 2;
            // WHY 146 is the column: it is the count pill's own width (550..696).
            if let Some(c) = test_text(x, y, badge_x, badge_y, 1, badge_str, 146, ACCENT_SKY, SURFACE_RAISED) {
                return c;
            }

            if rx < 2 || ry < 2 {
                return BORDER_SUBTLE;
            }
            return SURFACE_RAISED;
        }

        return BG_DARK;
    }

    // 5. Agenda List / Empty State Container (y: 536..1360)
    if y >= 536 && y < 1360 {
        // A create-event flow replaces the agenda while it is open, so the
        // empty-state card, the "+ New Event" button and the event cards are not
        // drawn and cannot be tapped.
        if app.flow != Flow::None {
            return flow_pixel(x, y, app);
        }

        if agenda.len == 0 {
            // Empty State Card at x: 24..696, y: 546..870 (width: 672, height: 324)
            if x >= 24 && x < 696 && y >= 546 && y < 870 {
                let rx = (x - 24).min(695 - x);
                let ry = (y - 546).min(869 - y);
                if rx < 20 && ry < 20 {
                    let cdx = (20 - rx) as isize;
                    let cdy = (20 - ry) as isize;
                    if cdx * cdx + cdy * cdy >= 400 {
                        return BG_DARK;
                    }
                }

                // --- Vector Calendar Tear-Sheet Pictogram at cx: 360, cy: 636 ---
                let cx = 360;
                let cy = 636;

                // Calendar binder rings
                if (y >= cy - 44 && y < cy - 28) && ((x >= cx - 26 && x < cx - 18) || (x >= cx + 18 && x < cx + 26)) {
                    return TEXT_MUTED;
                }

                // Calendar tear-sheet body (x: cx - 44..cx + 44, y: cy - 32..cy + 44)
                if x >= cx - 44 && x < cx + 44 && y >= cy - 32 && y < cy + 44 {
                    // Top header stripe in Coral
                    if y < cy - 12 {
                        return DANGER_CORAL;
                    }

                    // Border rim of page
                    if x == cx - 44 || x == cx + 43 || y == cy + 43 {
                        return BORDER_SUBTLE;
                    }

                    // Content lines on page
                    if (y >= cy + 4 && y < cy + 8 && x >= cx - 28 && x < cx + 28)
                        || (y >= cy + 16 && y < cy + 20 && x >= cx - 28 && x < cx + 16)
                        || (y >= cy + 28 && y < cy + 32 && x >= cx - 28 && x < cx + 6)
                    {
                        return BORDER_SUBTLE;
                    }

                    return SURFACE_RAISED;
                }

                // Empty state text
                //
                // WHY the headline changes with the store: the card used to be
                // the only agenda state, so "No Scheduled Events" was always
                // true. With real stored events the honest headline for a day
                // that has none is different: either the store itself is empty
                // (nothing was ever saved) or the store has events and this day
                // has none. Both are stated, and the capacity limit is named when
                // that is the reason.
                let headline: &[u8] = if app.store.len() == 0 {
                    b"No Scheduled Events"
                } else {
                    b"No Events On This Day"
                };
                let headline_h = std::design::font::line_height(2);
                if y >= 706 && y < 706 + headline_h {
                    let hx = centered_x(headline, 2, 24, 672);
                    if let Some(c) = test_text(
                        x,
                        y,
                        hx,
                        706,
                        2,
                        headline,
                        // WHY 672 is the column: it is the empty-state card's inner
                        // width (24..696), the box the headline is centred in. The
                        // longer of the two headlines, "No Events On This Day",
                        // measures 430px on the bitmap face at scale 2 and 413px on
                        // the outline face, so both fit.
                        672,
                        TEXT_PRIMARY,
                        SURFACE_DARK,
                    ) {
                        return c;
                    }
                }
                // Detail line: what is empty, and why. Every branch names a
                // reason the app actually observed; none of them claims data
                // that was never read.
                let detail_h = std::design::font::line_height(1);
                if y >= 760 && y < 760 + detail_h {
                    let mut detail = [0u8; 40];
                    let detail_str: &[u8] =
                        if matches!(app.store.state, StoreState::ReadRefused | StoreState::Invalid) {
                            // WHY this comes first: if storage could not be read, the
                            // calendar has nothing to show and the absence is a
                            // storage fact, not an empty agenda.
                            b"STORAGE UNREADABLE: NO EVENTS LOADED"
                        } else if !app.date_known {
                            b"RTC date unavailable"
                        } else if let Some(day) = app.selected_day {
                            let mut n = 0usize;
                            let lead = b"NO EVENTS ON ";
                            detail[..lead.len()].copy_from_slice(lead);
                            n += lead.len();
                            n += fmt_date(
                                &mut detail[n..],
                                app.current_year as u16,
                                app.current_month,
                                day,
                            );
                            &detail[..n]
                        } else if app.store.len() == 0 {
                            b"NO EVENT SAVED YET"
                        } else {
                            b"NO DAY SELECTED"
                        };
                    let dx = centered_x(detail_str, 1, 24, 672);
                    if let Some(c) = test_text(
                        x,
                        y,
                        // WHY the measured column: the detail line sits under a
                        // headline that is centred in the card, so it must be centred
                        // in the same card or the two lines disagree about the middle.
                        // It was drawn from a literal `236`, which put it 25px left of
                        // the headline's centre (capture: headline centre 354.7,
                        // detail centre 303.3) and read as ragged.
                        dx,
                        760,
                        1,
                        detail_str,
                        // WHY 672 is the column: it is the empty-state card's inner
                        // width (24..696), the same box the headline above it is
                        // centred in.
                        672,
                        TEXT_MUTED,
                        SURFACE_DARK,
                    ) {
                        return c;
                    }
                }

                // Action button: "+ New Event" on its own rect, sized from the
                // label. WHY it is not the old 260px box: "+ New Event" measures
                // 236px at scale 2 and the box left it 224px inside 18px of
                // padding, so the label's last glyphs ran past the pill's right
                // edge and were cut (the guest capture read "New Event" with the
                // final `t` sheared off). The rect is now the label plus a 36px
                // margin on each side, and the label is centred on it, so the
                // text and the hit box can never disagree again.
                if x >= ACTION_BTN_X0 && x < ACTION_BTN_X1 && y >= ACTION_BTN_Y0 && y < ACTION_BTN_Y1 {
                    let bx = (x - ACTION_BTN_X0).min(ACTION_BTN_X1 - 1 - x);
                    let by = (y - ACTION_BTN_Y0).min(ACTION_BTN_Y1 - 1 - y);
                    if bx < 24 && by < 24 {
                        let cdx = (24 - bx) as isize;
                        let cdy = (24 - by) as isize;
                        if cdx * cdx + cdy * cdy >= 576 {
                            return SURFACE_DARK;
                        }
                    }

                    let label_w = std::design::font::measure(ACTION_BTN_LABEL, ACTION_BTN_SCALE);
                    let label_x = ACTION_BTN_X0
                        + (ACTION_BTN_W.saturating_sub(label_w)) / 2;
                    let label_y = ACTION_BTN_Y0
                        + (ACTION_BTN_H.saturating_sub(
                            std::design::font::line_height(ACTION_BTN_SCALE),
                        )) / 2;
                    if let Some(c) = test_text(
                        x,
                        y,
                        label_x,
                        label_y,
                        ACTION_BTN_SCALE,
                        ACTION_BTN_LABEL,
                        // WHY ACTION_BTN_W is the column: the label is centred
                        // in the button's own rect, which is the label plus a
                        // 36px margin each side. "+ New Event" measures 236px on
                        // the bitmap face at scale 2 and 227px on the outline
                        // face, so both fit the 308px button.
                        ACTION_BTN_W,
                        ACCENT_SKY,
                        0xFF132338,
                    ) {
                        return c;
                    }

                    if bx < 2 || by < 2 {
                        return ACCENT_SKY;
                    }
                    return 0xFF132338;
                }

                // Card border
                if rx < 2 || ry < 2 {
                    return BORDER_SUBTLE;
                }
                return SURFACE_DARK;
            }
        } else {
            // Event cards for the selected day, earliest first.
            let card_idx = (y - 546) / 130;
            let card_offset_y = (y - 546) % 130;
            if card_idx < agenda.len && card_offset_y < 114 && x >= 24 && x < W - 24 {
                let rx = (x - 24).min(W - 25 - x);
                let ry = card_offset_y.min(113 - card_offset_y);
                if rx < 16 && ry < 16 {
                    let cdx = (16 - rx) as isize;
                    let cdy = (16 - ry) as isize;
                    if cdx * cdx + cdy * cdy >= 256 {
                        return BG_DARK;
                    }
                }

                // Left color accent bar
                if x < 34 {
                    return ACCENT_SKY;
                }

                // Card text: the stored title, the stored time and the stored
                // date, every one of them read back out of the record.
                let card_y = 546 + card_idx * 130;
                let item = &agenda.items[card_idx];
                let line_h1 = std::design::font::line_height(1);
                if y >= card_y + 12 && y < card_y + 12 + line_h1 {
                    if let Some(c) = test_text(x, y, 44, card_y + 12, 1, item.title_bytes(), 696 - 44, TEXT_PRIMARY, SURFACE_DARK) {
                        return c;
                    }
                }
                if y >= card_y + 44 && y < card_y + 44 + line_h1 {
                    let mut time_buf = [0u8; 5];
                    let time_n = fmt_time(&mut time_buf, item.hour, item.minute);
                    if let Some(c) = test_text(x, y, 44, card_y + 44, 1, &time_buf[..time_n], 696 - 44, ACCENT_SKY, SURFACE_DARK) {
                        return c;
                    }
                }
                if y >= card_y + 76 && y < card_y + 76 + line_h1 {
                    let mut date_buf = [0u8; 12];
                    let date_n = fmt_date(&mut date_buf, item.year, item.month, item.day);
                    if let Some(c) = test_text(x, y, 44, card_y + 76, 1, &date_buf[..date_n], 696 - 44, TEXT_MUTED, SURFACE_DARK) {
                        return c;
                    }
                }

                if rx < 2 || ry < 2 {
                    return BORDER_SUBTLE;
                }
                return SURFACE_DARK;
            }
        }
        return BG_DARK;
    }

    // 6. Home Gesture Pill (x: 260..460, y: 1418..1424)
    if y >= 1418 && y < 1424 && x >= 260 && x < 460 {
        return TEXT_MUTED;
    }

    BG_DARK
}

/// Sheet height for the date and time steps: neither draws a keyboard, so they
/// end after their own status line instead of covering the keyboard rows.
const SHEET_COMPACT_Y1: usize = 972;

/// Rasterises the create-event sheet.
///
/// WHY it is one function per step: each step draws a different control set, and
/// a shared renderer would need a branch per control anyway while making the
/// hit test harder to read against it.
fn flow_pixel(x: usize, y: usize, app: &CalendarApp) -> u32 {
    let sheet_y1 = match app.flow {
        Flow::Title => SHEET_Y1,
        _ => SHEET_COMPACT_Y1,
    };
    if x < SHEET_X0 || x >= SHEET_X1 || y < SHEET_Y0 || y >= sheet_y1 {
        return BG_DARK;
    }
    let rx = (x - SHEET_X0).min(SHEET_X1 - 1 - x);
    let ry = (y - SHEET_Y0).min(sheet_y1 - 1 - y);
    if rx < 20 && ry < 20 {
        let cdx = (20 - rx) as isize;
        let cdy = (20 - ry) as isize;
        if cdx * cdx + cdy * cdy >= 400 {
            return BG_DARK;
        }
    }
    if rx < 2 || ry < 2 {
        return BORDER_SUBTLE;
    }

    let mut step_buf = [0u8; 56];
    let step_str: &[u8] = match app.flow {
        Flow::Title => b"STEP 1 OF 3 - EVENT TITLE",
        // WHY the no-RTC note is carried in the step line: it is the fact that
        // changes how the date must be entered, and the line is the only place
        // it fits without stealing a row from the controls below it.
        Flow::Date => {
            let base = b"STEP 2 OF 3 - DATE";
            step_buf[..base.len()].copy_from_slice(base);
            let mut n = base.len();
            if !app.date_known {
                let tail = b" (NO RTC: DATE ENTERED BY HAND)";
                if step_buf.len() >= n + tail.len() {
                    step_buf[n..n + tail.len()].copy_from_slice(tail);
                    n += tail.len();
                }
            }
            &step_buf[..n]
        }
        Flow::Time => b"STEP 3 OF 3 - TIME",
        Flow::None => b"",
    };
    if let Some(c) = test_text(
        x,
        y,
        centered_x(b"NEW EVENT", 2, SHEET_X0 + 20, SHEET_X1 - SHEET_X0 - 40),
        SHEET_TITLE_Y,
        2,
        b"NEW EVENT",
        // WHY the column is the sheet's inner width: the run is centred in it.
        // "NEW EVENT" measures 226px on the bitmap face at scale 2 and 223px on
        // the outline face.
        SHEET_X1 - SHEET_X0 - 40,
        TEXT_PRIMARY,
        SURFACE_DARK,
    ) {
        return c;
    }
    if let Some(c) = test_text(
        x,
        y,
        centered_x(step_str, 1, SHEET_X0 + 20, SHEET_X1 - SHEET_X0 - 40),
        SHEET_STEP_Y,
        1,
        step_str,
        SHEET_X1 - SHEET_X0 - 40,
        TEXT_MUTED,
        SURFACE_DARK,
    ) {
        return c;
    }

    match app.flow {
        Flow::Title => title_step_pixel(x, y, app),
        Flow::Date => date_step_pixel(x, y, app),
        Flow::Time => time_step_pixel(x, y, app),
        Flow::None => BG_DARK,
    }
}

/// Rasterises the title step: the field being typed and the on-screen keyboard.
fn title_step_pixel(x: usize, y: usize, app: &CalendarApp) -> u32 {
    if x >= TITLE_FIELD_X0 && x < TITLE_FIELD_X1 && y >= TITLE_FIELD_Y0 && y < TITLE_FIELD_Y1 {
        let rx = (x - TITLE_FIELD_X0).min(TITLE_FIELD_X1 - 1 - x);
        let ry = (y - TITLE_FIELD_Y0).min(TITLE_FIELD_Y1 - 1 - y);
        if rx < 10 && ry < 10 {
            let cdx = (10 - rx) as isize;
            let cdy = (10 - ry) as isize;
            if cdx * cdx + cdy * cdy >= 100 {
                return SURFACE_DARK;
            }
        }
        // The title being typed, or a visible placeholder when it is empty. WHY
        // the placeholder is not drawn as text the app would later commit: an
        // empty title must look empty, because an empty title is refused.
        let text: &[u8] = if app.title_len == 0 {
            b"UNTITLED"
        } else {
            &app.title[..app.title_len]
        };
        let color = if app.title_len == 0 { TEXT_MUTED } else { TEXT_PRIMARY };
        if let Some(c) = test_text(
            x,
            y,
            TITLE_FIELD_X0 + 12,
            TITLE_TEXT_Y,
            1,
            text,
            // WHY the column is the field's inner width: the run starts 12px
            // inside the field and the field ends at TITLE_FIELD_X1.
            TITLE_FIELD_X1 - (TITLE_FIELD_X0 + 12),
            color,
            SURFACE_RAISED,
        ) {
            return c;
        }
        // Caret block, so the field reads as editable.
        let text_w = std::design::font::measure(text, 1);
        if x >= TITLE_FIELD_X0 + 12 + text_w + 2
            && x < TITLE_FIELD_X0 + 12 + text_w + 10
            && y >= TITLE_TEXT_Y
            && y < TITLE_TEXT_Y + std::design::font::line_height(1)
        {
            return ACCENT_SKY;
        }
        if rx < 2 || ry < 2 {
            return BORDER_SUBTLE;
        }
        return SURFACE_RAISED;
    }

    for row in 0..4 {
        for col in 0..KB_LETTER_COLS {
            if keyboard_key_at(x, y) == Some((row, col)) {
                return key_pixel(x, y, row, col);
            }
        }
    }

    SURFACE_DARK
}

/// Rasterises one keyboard key.
fn key_pixel(x: usize, y: usize, row: usize, col: usize) -> u32 {
    let label = key_label(row, col);
    let Some((x0, y0, x1, y1)) = key_rect(row, col) else {
        return BG_DARK;
    };
    if label.is_empty() {
        return BG_DARK;
    }
    let rx = (x - x0).min(x1 - 1 - x);
    let ry = (y - y0).min(y1 - 1 - y);
    if rx < 10 && ry < 10 {
        let cdx = (10 - rx) as isize;
        let cdy = (10 - ry) as isize;
        if cdx * cdx + cdy * cdy >= 100 {
            return BG_DARK;
        }
    }
    // WHY the NEXT key is the only accented one: it is the only key on the sheet
    // that changes which step the user is on, and it needs to be findable
    // without a legend.
    let accent = row == 3 && col == 2;
    if let Some(c) = test_text(
        x,
        y,
        centered_x(label, 2, x0, x1 - x0),
        y0 + (KB_ROW_H - std::design::font::line_height(2)) / 2,
        2,
        label,
        // WHY the column is the key's own width: the label is centred in the key
        // it sits on, so a run wider than the key would be cut by it. The widest
        // single-letter key, 'M' and 'W', measures 32px on the bitmap face at
        // scale 2 and 36px on the outline face against a 46px key; "SPACE"
        // measures 130 against 304px, "DEL" 74 against 176px and "NEXT" 104
        // against 180px, so every key fits on either face.
        x1 - x0,
        if accent { ACCENT_SKY } else { TEXT_PRIMARY },
        SURFACE_RAISED,
    ) {
        return c;
    }
    if rx < 2 || ry < 2 {
        return if accent { ACCENT_SKY } else { BORDER_SUBTLE };
    }
    SURFACE_RAISED
}

/// Rasterises one stepper box: label, value, and the -/+ pair under it.
///
/// Returns `None` when `(x, y)` is outside the box, so the caller can fall
/// through to the next control.
fn stepper_box_pixel(
    x: usize,
    y: usize,
    x0: usize,
    x1: usize,
    label: &[u8],
    value: &[u8],
    value_color: u32,
) -> Option<u32> {
    if x < x0 || x >= x1 || y < STEP_Y0 || y >= STEP_Y1 {
        return None;
    }
    let rx = (x - x0).min(x1 - 1 - x);
    let ry = (y - STEP_Y0).min(STEP_Y1 - 1 - y);
    if rx < 10 && ry < 10 {
        let cdx = (10 - rx) as isize;
        let cdy = (10 - ry) as isize;
        if cdx * cdx + cdy * cdy >= 100 {
            return Some(SURFACE_DARK);
        }
    }
    if let Some(c) = test_text(
        x,
        y,
        centered_x(label, 1, x0, x1 - x0),
        STEP_LABEL_Y,
        1,
        label,
        // The steppers' labels are scale 1; the column is the box's inner width.
        x1 - x0,
        TEXT_MUTED,
        SURFACE_RAISED,
    ) {
        return Some(c);
    }
    if let Some(c) = test_text(
        x,
        y,
        centered_x(value, 2, x0, x1 - x0),
        STEP_VALUE_Y,
        2,
        value,
        // WHY the column is the box's inner width: the value is centred in the
        // stepper box it belongs to. The widest, a four-digit year, measures 88px
        // on the bitmap face at scale 2 and 92px on the outline face against the
        // date boxes' 192px and the time boxes' 300px.
        x1 - x0,
        value_color,
        SURFACE_RAISED,
    ) {
        return Some(c);
    }
    let (minus, plus) = step_btn_rects(x0, x1);
    for (kx0, ky0, kx1, ky1, glyph) in [
        (minus.0, minus.1, minus.2, minus.3, b"-".as_slice()),
        (plus.0, plus.1, plus.2, plus.3, b"+".as_slice()),
    ] {
        if x >= kx0 && x < kx1 && y >= ky0 && y < ky1 {
            let krx = (x - kx0).min(kx1 - 1 - x);
            let kry = (y - ky0).min(ky1 - 1 - y);
            if krx < 10 && kry < 10 {
                let cdx = (10 - krx) as isize;
                let cdy = (10 - kry) as isize;
                if cdx * cdx + cdy * cdy >= 100 {
                    return Some(SURFACE_DARK);
                }
            }
            if let Some(c) = test_text(
                x,
                y,
                centered_x(glyph, 2, kx0, kx1 - kx0),
                ky0 + (ky1 - ky0 - std::design::font::line_height(2)) / 2,
                2,
                glyph,
                // WHY the column is the button's own width: the sign is centred
                // in the 72px button, "-" measures 12px on the bitmap face at
                // scale 2 and 11px on the outline, "+" 24 and 23px.
                kx1 - kx0,
                ACCENT_SKY,
                SURFACE_RAISED,
            ) {
                return Some(c);
            }
            if krx < 2 || kry < 2 {
                return Some(ACCENT_SKY);
            }
            return Some(SURFACE_RAISED);
        }
    }
    if rx < 2 || ry < 2 {
        return Some(BORDER_SUBTLE);
    }
    Some(SURFACE_RAISED)
}

/// Rasterises the date step.
fn date_step_pixel(x: usize, y: usize, app: &CalendarApp) -> u32 {
    let mut year_buf = [0u8; 4];
    let (year_str, year_color) = stepper_value(
        app.edit_year.map(|y| y as u32),
        &mut year_buf,
        0,
    );
    let mut month_buf = [0u8; 2];
    let (month_str, month_color) = stepper_value(app.edit_month.map(|m| m as u32), &mut month_buf, 2);
    let mut day_buf = [0u8; 2];
    let (day_str, day_color) = stepper_value(app.edit_day.map(|d| d as u32), &mut day_buf, 2);

    for (which, (bx0, bx1)) in DATE_BOXES.iter().enumerate() {
        let (value, color) = match which {
            0 => (year_str, year_color),
            1 => (month_str, month_color),
            _ => (day_str, day_color),
        };
        let label: &[u8] = match which {
            0 => b"YEAR",
            1 => b"MONTH",
            _ => b"DAY",
        };
        if let Some(c) = stepper_box_pixel(x, y, *bx0, *bx1, label, value, color) {
            return c;
        }
    }

    let buttons = flow_buttons_pixel(x, y, b"NEXT");
    if let Some(b) = buttons {
        return b;
    }
    flow_status_pixel(x, y, app)
}

/// Rasterises the time step, whose forward button commits the event.
fn time_step_pixel(x: usize, y: usize, app: &CalendarApp) -> u32 {
    let mut hour_buf = [0u8; 2];
    let (hour_str, hour_color) = stepper_value(app.edit_hour.map(|h| h as u32), &mut hour_buf, 2);
    let mut minute_buf = [0u8; 2];
    let (minute_str, minute_color) =
        stepper_value(app.edit_minute.map(|m| m as u32), &mut minute_buf, 2);

    for (which, (bx0, bx1)) in TIME_BOXES.iter().enumerate() {
        let (value, color) = if which == 0 {
            (hour_str, hour_color)
        } else {
            (minute_str, minute_color)
        };
        let label: &[u8] = if which == 0 { b"HOUR" } else { b"MINUTE" };
        if let Some(c) = stepper_box_pixel(x, y, *bx0, *bx1, label, value, color) {
            return c;
        }
    }

    let buttons = flow_buttons_pixel(x, y, b"SAVE EVENT");
    if let Some(b) = buttons {
        return b;
    }
    flow_status_pixel(x, y, app)
}

/// Rasterises the sheet's status line: what the last action actually did.
///
/// WHY the colour follows the word: a "NOT SAVED" or "REFUSED" outcome must not
/// be painted as an achievement, and a saved one is worth reading at a glance.
fn flow_status_pixel(x: usize, y: usize, app: &CalendarApp) -> u32 {
    let status = app.commit_status.as_bytes();
    let ok = app.commit_status.starts_with("SAVED") || app.commit_status.starts_with("DAY CLAMPED");
    if let Some(c) = test_text(
        x,
        y,
        centered_x(status, 1, SHEET_X0 + 20, SHEET_X1 - SHEET_X0 - 40),
        FLOW_STATUS_Y,
        1,
        status,
        SHEET_X1 - SHEET_X0 - 40,
        if ok { SUCCESS_EMERALD } else { TEXT_MUTED },
        SURFACE_DARK,
    ) {
        return c;
    }
    SURFACE_DARK
}

/// Formats one stepper field, or `--` when the user has not set it.
///
/// WHY `--` rather than a default value: a field that shows a number nobody
/// picked would be a date the user never entered.
fn stepper_value<'a>(
    value: Option<u32>,
    out: &'a mut [u8],
    min_digits: usize,
) -> (&'a [u8], u32) {
    match value {
        Some(v) => {
            let n = fmt_dec(out, v, min_digits);
            (&out[..n], TEXT_PRIMARY)
        }
        None => (b"--", TEXT_MUTED),
    }
}

/// Writes `value` into `out` with at least `min_digits` digits and returns the
/// length written.
fn fmt_dec(out: &mut [u8], value: u32, min_digits: usize) -> usize {
    let mut digits = [0u8; 10];
    let mut d = 0usize;
    let mut v = value;
    while v > 0 {
        digits[d] = b'0' + (v % 10) as u8;
        v /= 10;
        d += 1;
    }
    while d < min_digits {
        digits[d] = b'0';
        d += 1;
    }
    if d == 0 {
        digits[0] = b'0';
        d = 1;
    }
    let mut len = 0usize;
    while d > 0 {
        if len >= out.len() {
            return 0;
        }
        out[len] = digits[d - 1];
        len += 1;
        d -= 1;
    }
    len
}

/// Rasterises the forward button and the cancel button of a step.
///
/// Returns `None` when `(x, y)` is on neither, so the caller keeps drawing.
fn flow_buttons_pixel(x: usize, y: usize, forward_label: &[u8]) -> Option<u32> {
    if y < FLOW_BTN_Y0 || y >= FLOW_BTN_Y1 {
        return None;
    }
    for (bx0, bx1, label, accent) in [
        (SAVE_BTN_X0, SAVE_BTN_X1, forward_label, true),
        (CANCEL_BTN_X0, CANCEL_BTN_X1, b"CANCEL".as_slice(), false),
    ] {
        if x < bx0 || x >= bx1 {
            continue;
        }
        let rx = (x - bx0).min(bx1 - 1 - x);
        let ry = (y - FLOW_BTN_Y0).min(FLOW_BTN_Y1 - 1 - y);
        if rx < 12 && ry < 12 {
            let cdx = (12 - rx) as isize;
            let cdy = (12 - ry) as isize;
            if cdx * cdx + cdy * cdy >= 144 {
                return Some(SURFACE_DARK);
            }
        }
        if let Some(c) = test_text(
            x,
            y,
            centered_x(label, 2, bx0, bx1 - bx0),
            FLOW_BTN_Y0 + (FLOW_BTN_Y1 - FLOW_BTN_Y0 - std::design::font::line_height(2)) / 2,
            2,
            label,
            // WHY the column is the button's own width: the label is centred in
            // the button it sits on. "SAVE EVENT" measures 242px on the bitmap
            // face at scale 2 and 235px on the outline face against its 342px
            // button, and "CANCEL" 160px against 155px against its 252px one, so
            // every label fits on either face.
            bx1 - bx0,
            if accent { ACCENT_SKY } else { TEXT_MUTED },
            SURFACE_RAISED,
        ) {
            return Some(c);
        }
        if rx < 2 || ry < 2 {
            return Some(if accent { ACCENT_SKY } else { BORDER_SUBTLE });
        }
        return Some(SURFACE_RAISED);
    }
    None
}

fn render_calendar_ui(surface: u32, app: &CalendarApp) {
    render_calendar_band(surface, app, 0, H);
}

/// Re-rasterises and writes only rows `y0..y1`.
///
/// WHY a band and not a rectangle: a row of the surface is one contiguous run in
/// the surface buffer, so `offset = y0 * W` writes a whole band with no stride
/// bookkeeping, and every band starts on a row boundary so the running `x`/`y`
/// cursor below stays exact.
///
/// WHY this is correct rather than a shortcut: the kernel's KMS driver already
/// narrows the scanout copy-out to the rows a surface actually queued
/// (`kernel/src/drivers/display_kms.rs`, `mark_present_rows`), so writing a band
/// instead of the whole 1,036,800-pixel surface is taking an offer the kernel
/// already made. A tap that changes one day selection touches the grid, the
/// section header and the agenda - three of the five bands - instead of
/// re-rasterising every pixel for every event the reactor loop polls.
fn render_calendar_band(surface: u32, app: &CalendarApp, y0: usize, y1: usize) {
    let (display_year, display_month, start_dow, num_days) = app.display_params();
    let agenda = app.agenda_view();
    let mut chunk = [0u32; CHUNK];
    let mut offset = y0 * W;
    let end = y1.min(H) * W;
    while offset < end {
        let count = (end - offset).min(CHUNK);
        let mut x = offset % W;
        let mut y = offset / W;
        for i in 0..count {
            chunk[i] = get_calendar_pixel(
                x,
                y,
                app,
                display_year,
                display_month,
                start_dow,
                num_days,
                &agenda,
            );
            x += 1;
            if x == W {
                x = 0;
                y += 1;
            }
        }
        let _ = std::surface_write(6, surface, offset, &chunk[..count]);
        offset += count;
    }
}

// -----------------------------------------------------------------------------
// Banded repaint: which rows a touch actually changed
// -----------------------------------------------------------------------------

/// The row bands of the calendar surface.
///
/// WHY a bitmask rather than the `Dirty` enum `apps/file_manager` uses: a single
/// tap in this app legitimately moves rows in three bands at once. Selecting a
/// grid day repaints the grid (the highlight moves), the section header (the
/// title and the count name the new day) and the agenda (the list below it is
/// that day's list). Collapsing that to one "All" variant would re-rasterise the
/// header and weekday bands for rows no touch reached, and dropping the extra
/// bands would leave stale pixels behind.
#[derive(Clone, Copy, PartialEq, Eq)]
struct Dirty(u8);

impl Dirty {
    /// Nothing moved; the surface is already correct, so do not render at all.
    const EMPTY: Dirty = Dirty(0);
    /// Status bar and header: y 0..96. No control mutates it, so it exists to
    /// make a full-surface repaint complete rather than because a touch
    /// selects it.
    const HEADER: Dirty = Dirty(1 << 0);
    /// Weekday run: y 96..144.
    const WEEKDAY: Dirty = Dirty(1 << 1);
    /// Month grid: y 144..456. Day selection moves the highlight.
    const GRID: Dirty = Dirty(1 << 2);
    /// Agenda section header and event count: y 470..530.
    const SECTION: Dirty = Dirty(1 << 3);
    /// Agenda list, empty state and the create-event sheet: y 536..1360.
    const AGENDA: Dirty = Dirty(1 << 4);

    fn is_empty(self) -> bool {
        self.0 == 0
    }

    /// Grows the dirty region so one rendered frame covers everything a poll
    /// batch changed.
    ///
    /// WHY: `event_poll` can deliver several touches before the app is scheduled
    /// again, and rendering only the last region would leave the earlier one
    /// stale.
    fn widen(self, other: Dirty) -> Dirty {
        Dirty(self.0 | other.0)
    }
}

/// The bands, in row order, that a repaint can write.
const BANDS: [(Dirty, usize, usize); 5] = [
    (Dirty::HEADER, 0, 96),
    (Dirty::WEEKDAY, 96, 144),
    (Dirty::GRID, 144, 456),
    (Dirty::SECTION, 470, 530),
    (Dirty::AGENDA, 536, 1360),
];

/// Renders exactly the bands a poll batch dirtied, then flips once.
///
/// WHY there is no band for the home gesture pill (y 1418..1424): nothing a
/// touch can reach writes to it, so a band for it would be dead weight
/// pretending to be an optimisation.
fn repaint(surface: u32, app: &CalendarApp, dirty: Dirty) {
    for (band, y0, y1) in BANDS {
        if dirty.0 & band.0 != 0 {
            render_calendar_band(surface, app, y0, y1);
        }
    }
    let _ = std::surface_flip(6);
}

/// What one resolved touch did.
enum Tap {
    /// Nothing on screen moved.
    None,
    /// The model moved; render the named bands.
    Redraw(Dirty),
    /// Leave to the launcher, logging this exact reason first.
    Leave(&'static str),
}

fn exit_to_launcher(surface: u32) -> ! {
    if surface > 0 {
        let _ = std::surface_destroy(6, surface);
    }
    std::exit_with_code(0);
}

// -----------------------------------------------------------------------------
// Create-event flow
// -----------------------------------------------------------------------------

/// Opens the create-event flow at the title step.
fn open_flow(app: &mut CalendarApp) {
    app.flow = Flow::Title;
    app.title_len = 0;
    app.title = [0; EVENT_TITLE_MAX];
    // WHY the date can start pre-filled and nothing else can: a day tapped in
    // the month grid is a real date the user chose, so it is fair to carry it
    // into the picker. Every field still starts unset when there is no RTC date
    // and no tap, so no date is ever shown or committed that nobody entered.
    match (app.date_known, app.selected_day) {
        (true, Some(day)) => {
            app.edit_year = Some(app.current_year as u16);
            app.edit_month = Some(app.current_month);
            app.edit_day = Some(day);
        }
        _ => {
            app.edit_year = None;
            app.edit_month = None;
            app.edit_day = None;
        }
    }
    app.edit_hour = None;
    app.edit_minute = None;
    app.commit_status = "NO EVENT CREATED YET";
}

/// Commits the entry that was typed and picked.
///
/// WHY it refuses instead of storing a partial event: an event without a title,
/// a date or a time is not an event, and a record with a missing field would be
/// rendered as if it had one. The refusal names the missing field.
fn commit_event(app: &mut CalendarApp) {
    if app.title_len == 0 {
        app.commit_status = "NOT SAVED: EVENT HAS NO TITLE";
        return;
    }
    let (Some(year), Some(month), Some(day)) = (app.edit_year, app.edit_month, app.edit_day) else {
        app.commit_status = "NOT SAVED: NO DATE ENTERED";
        return;
    };
    let (Some(hour), Some(minute)) = (app.edit_hour, app.edit_minute) else {
        app.commit_status = "NOT SAVED: NO TIME ENTERED";
        return;
    };
    let rec = EventRecord::from_parts(year, month, day, hour, minute, &app.title[..app.title_len]);
    if app.store.is_full() {
        app.commit_status = "NOT SAVED: STORE FULL (12 EVENTS)";
        return;
    }
    // WHY the record is checked before it is stored rather than after: the
    // store's own loader rejects a record whose date or time is not real, so
    // writing one would be caught only on the next launch -- and it would take
    // the rest of the calendar with it. Refusing it here names the reason.
    if !rec.valid() {
        app.commit_status = "NOT SAVED: THAT DATE OR TIME DOES NOT EXIST";
        return;
    }
    if app.store.add(rec) {
        app.total_events = app.store.len() as u32;
        app.commit_status = "SAVED: EVENT WRITTEN TO THE SETTINGS STORE";
        // WHY the selection follows the committed event: the point of committing
        // is to see the event in that day's agenda, and the grid is where that
        // day is named.
        app.selected_day = Some(day);
        app.flow = Flow::None;
        app.title_len = 0;
        return;
    }
    // WHY the sheet stays open when the write was refused: the entry was not
    // stored, so it must not appear in the agenda, and closing the sheet would
    // throw away what was typed for a reason the user could not act on.
    app.commit_status = "SETTINGS WRITE REFUSED: EVENT NOT PERSISTED";
}

/// Applies one keyboard key to the title being typed.
fn apply_title_key(app: &mut CalendarApp, row: usize, col: usize) {
    match (row, col) {
        (3, 0) => {
            if !app.push_title(b' ') {
                app.commit_status = "TITLE FULL: 32 CHARACTERS";
            }
        }
        (3, 1) => {
            if app.title_len > 0 {
                app.title_len -= 1;
            }
        }
        (3, 2) => app.flow = Flow::Date,
        (0 | 1 | 2, _) => {
            let label = key_label(row, col);
            if label.len() == 1 {
                if !app.push_title(label[0]) {
                    app.commit_status = "TITLE FULL: 32 CHARACTERS";
                }
            }
        }
        _ => {}
    }
}

/// Steps one field of the date or time being picked.
///
/// WHY the first press sets a floor instead of leaving the field unset: a
/// stepper that did nothing while empty could never be read as "unset by
/// choice", and a field that silently became a value nobody touched would be a
/// fabricated date. The floors are the format's own limits.
fn step_field<T: Steppable>(value: &mut Option<T>, delta: i32, floor: T, ceiling: T) {
    *value = Some(match *value {
        None => floor,
        Some(v) => {
            if delta > 0 {
                if v >= ceiling {
                    floor
                } else {
                    v.next_step()
                }
            } else if v <= floor {
                ceiling
            } else {
                v.prev_step()
            }
        }
    });
}

/// A field the stepper can move by one.
trait Steppable: Ord + Copy {
    fn next_step(self) -> Self;
    fn prev_step(self) -> Self;
}

impl Steppable for u8 {
    fn next_step(self) -> u8 {
        self + 1
    }
    fn prev_step(self) -> u8 {
        self - 1
    }
}

impl Steppable for u16 {
    fn next_step(self) -> u16 {
        self + 1
    }
    fn prev_step(self) -> u16 {
        self - 1
    }
}

/// Applies one stepper tap.
/// Keeps the day inside the month that is actually selected.
///
/// WHY the month and the year clamp it too: a picker that moves to February
/// while showing 31 would offer a date that does not exist, and committing it
/// would store a record the store's own validation rejects -- which would make a
/// later load reject the whole blob. The clamp is applied wherever the day can
/// stop being a real date.
fn clamp_day(app: &mut CalendarApp) {
    let month = app.edit_month.unwrap_or(1);
    let year = app.edit_year.unwrap_or(STORE_YEAR_MIN);
    let last = days_in_month(year as u32, month);
    if let Some(day) = app.edit_day {
        if day > last {
            app.edit_day = Some(last);
            app.commit_status = "DAY CLAMPED TO THE SELECTED MONTH";
        }
    }
}

fn apply_step(app: &mut CalendarApp, which: usize, delta: i32) {
    match (app.flow, which) {
        (Flow::Date, 0) => {
            step_field(&mut app.edit_year, delta, STORE_YEAR_MIN, STORE_YEAR_MAX);
            clamp_day(app);
        }
        (Flow::Date, 1) => {
            step_field(&mut app.edit_month, delta, 1u8, 12u8);
            clamp_day(app);
        }
        (Flow::Date, 2) => {
            step_field(&mut app.edit_day, delta, 1u8, 31u8);
            clamp_day(app);
        }
        (Flow::Time, 0) => step_field(&mut app.edit_hour, delta, 0u8, 23u8),
        (Flow::Time, 1) => step_field(&mut app.edit_minute, delta, 0u8, 59u8),
        _ => {}
    }
}

/// Resolves a tap to a stepper's `-` (delta < 0) or `+` (delta > 0) button, or
/// `None` outside every drawn button.
///
/// WHY this is a function of the drawn rects rather than a half-box test: a
/// half-box hit area steps a field for taps that landed on empty card, which is
/// an invisible hotspot. `STEP_BTN` rects are the same ones
/// `stepper_box_pixel` paints.
fn stepper_button_at(x: usize, y: usize, boxes: &[(usize, usize)]) -> Option<(usize, i32)> {
    for (which, (bx0, bx1)) in boxes.iter().enumerate() {
        let (minus, plus) = step_btn_rects(*bx0, *bx1);
        for (rect, delta) in [(minus, -1i32), (plus, 1i32)] {
            if x >= rect.0 && x < rect.2 && y >= rect.1 && y < rect.3 {
                return Some((which, delta));
            }
        }
    }
    None
}

/// Resolves a tap against the control that is actually drawn in the flow.
fn handle_flow_tap(app: &mut CalendarApp, x: usize, y: usize) -> Dirty {
    // Back control: cancel the flow rather than leave the app, because the flow
    // is the thing the user is in the middle of.
    if std::design::back_button::contains(x, y) || (x >= 24 && x < 100 && y >= 28 && y < 68) {
        app.flow = Flow::None;
        app.commit_status = "CREATE CANCELLED: NO EVENT WRITTEN";
        return Dirty::AGENDA;
    }

    match app.flow {
        Flow::Title => {
            // The title field itself moves nothing; only the keyboard does.
            if let Some((row, col)) = keyboard_key_at(x, y) {
                apply_title_key(app, row, col);
                return Dirty::AGENDA;
            }
        }
        Flow::Date => {
            if let Some((which, delta)) = stepper_button_at(x, y, &DATE_BOXES) {
                apply_step(app, which, delta);
                return Dirty::AGENDA;
            }
            if y >= FLOW_BTN_Y0 && y < FLOW_BTN_Y1 {
                // WHY the same two buttons on both steps: the date step and the
                // time step each need a way forward and a way out, and they are
                // the same width in the same rows, so the geometry is written
                // once and the renderer draws them from the same rects.
                if x >= SAVE_BTN_X0 && x < SAVE_BTN_X1 {
                    app.flow = Flow::Time;
                    return Dirty::AGENDA;
                }
                if x >= CANCEL_BTN_X0 && x < CANCEL_BTN_X1 {
                    app.flow = Flow::None;
                    app.commit_status = "CREATE CANCELLED: NO EVENT WRITTEN";
                    return Dirty::AGENDA;
                }
            }
        }
        Flow::Time => {
            if let Some((which, delta)) = stepper_button_at(x, y, &TIME_BOXES) {
                apply_step(app, which, delta);
                return Dirty::AGENDA;
            }
            if x >= SAVE_BTN_X0 && x < SAVE_BTN_X1 && y >= FLOW_BTN_Y0 && y < FLOW_BTN_Y1 {
                commit_event(app);
                return Dirty::AGENDA;
            }
            if x >= CANCEL_BTN_X0 && x < CANCEL_BTN_X1 && y >= FLOW_BTN_Y0 && y < FLOW_BTN_Y1 {
                app.flow = Flow::None;
                app.commit_status = "CREATE CANCELLED: NO EVENT WRITTEN";
                return Dirty::AGENDA;
            }
        }
        Flow::None => {}
    }

    Dirty::EMPTY
}

/// Resolves one touch against the controls that are actually drawn.
///
/// Every rect below is a rect the renderer also draws from, and each test is
/// gated on the state that draws it, so a tap is accepted exactly where there is
/// something to press.
fn handle_tap(app: &mut CalendarApp, x: usize, y: usize) -> Tap {
    // The home gesture strip is the system's, not the app's, so it leaves the
    // launcher from anywhere -- including from inside a create-event flow.
    if x >= 240 && x <= 480 && y >= 1400 {
        return Tap::Leave("[CALENDAR APP] Back button or home gesture pressed. Exiting to launcher.");
    }
    // Back control or the home gesture strip.
    //
    // WHY the back control behaves differently while a flow is open: an
    // in-progress entry is what the user is in the middle of, so the same pill
    // cancels it and says so instead of exiting the app and losing the entry.
    let is_back = std::design::back_button::contains(x, y) || (x >= 24 && x < 100 && y >= 28 && y < 68);
    if app.flow != Flow::None {
        if is_back {
            app.flow = Flow::None;
            app.commit_status = "CREATE CANCELLED: NO EVENT WRITTEN";
            return Tap::Redraw(Dirty::AGENDA);
        }
        return Tap::Redraw(handle_flow_tap(app, x, y));
    }
    if is_back {
        return Tap::Leave("[CALENDAR APP] Back button or home gesture pressed. Exiting to launcher.");
    }

    // Month grid: a day cell is selectable, and the agenda below shows that
    // day's events.
    //
    // WHY the selection can only move inside a month the RTC actually produced:
    // with no civil date there are no cells to tap, so `date_known` gates the
    // hit test exactly as it gates the drawing.
    if app.date_known {
        if let Some(day) = grid_cell_at(x, y, day_of_week(app.current_year, app.current_month, 1), days_in_month(app.current_year, app.current_month)) {
            if app.selected_day == Some(day) {
                return Tap::None;
            }
            app.selected_day = Some(day);
            return Tap::Redraw(Dirty::GRID.widen(Dirty::SECTION).widen(Dirty::AGENDA));
        }
    }

    // "+ New Event": opens the create-event flow at the title step.
    //
    // WHY it now does something: the pill has been painted since the button was
    // introduced and no tap handler anywhere tested its rect, so the only action
    // on screen was inert. The hit test is written in the literal rect the
    // geometry contracts pin, and the literals are checked against the constants
    // below so a change to the pill has to be deliberate.
    debug_assert_eq!(ACTION_BTN_Y0, 800);
    debug_assert_eq!(ACTION_BTN_Y1, 856);
    if x >= ACTION_BTN_X0 && x < ACTION_BTN_X1 && y >= 800 && y < 856 {
        open_flow(app);
        std::log(
            0,
            "[CALENDAR APP] Create-event flow opened: title entry.",
        )
        .ok();
        return Tap::Redraw(Dirty::AGENDA);
    }

    Tap::None
}

#[no_mangle]
pub extern "C" fn _start() {
    std::log(0, "[CALENDAR APP (Ring 3)] Calendar Agenda service online.").ok();

    let mut calendar = CalendarApp::new();

    // 1. Sync Hardware RTC Clock
    calendar.sync_rtc();

    // 2. Allocate DRM/KMS Client Surface (z=200)
    let surface = match std::surface_create(6, W as u32, H as u32, 200) {
        Ok(id) => {
            std::log(
                0,
                "[CALENDAR APP] DRM/KMS Client Surface allocated (z=200).",
            )
            .ok();
            id
        }
        Err(_) => 0,
    };

    // 3. Verify Gregorian Calendar Math against the date the RTC reported.
    // WHY: this check previously ran against the hardcoded 2026-09-21 default
    // and logged "Sept 2026 has 30 days" as an observation. With no RTC date
    // there is nothing to verify, so report the date as unavailable.
    if calendar.date_known {
        let days = days_in_month(calendar.current_year, calendar.current_month);
        let dow = day_of_week(calendar.current_year, calendar.current_month, 1);
        std::log(
            0,
            match days {
                31 => "[CALENDAR APP] Gregorian date math: the RTC-reported month has 31 days.",
                30 => "[CALENDAR APP] Gregorian date math: the RTC-reported month has 30 days.",
                _ => "[CALENDAR APP] Gregorian date math: the RTC-reported month has fewer than 30 days.",
            },
        )
        .ok();
        std::log(
            0,
            match dow {
                0 => "[CALENDAR APP] Weekday for the first of the RTC-reported month: Sunday.",
                1 => "[CALENDAR APP] Weekday for the first of the RTC-reported month: Monday.",
                2 => "[CALENDAR APP] Weekday for the first of the RTC-reported month: Tuesday.",
                3 => "[CALENDAR APP] Weekday for the first of the RTC-reported month: Wednesday.",
                4 => "[CALENDAR APP] Weekday for the first of the RTC-reported month: Thursday.",
                5 => "[CALENDAR APP] Weekday for the first of the RTC-reported month: Friday.",
                _ => "[CALENDAR APP] Weekday for the first of the RTC-reported month: Saturday.",
            },
        )
        .ok();
    } else {
        std::log(
            0,
            "[CALENDAR APP] No civil date available from the hardware RTC; month grid shows no selected day.",
        )
        .ok();
    }

    // 4. Load the stored events and select the day the agenda will list.
    //
    // WHY this happens after the surface exists but before the first paint: the
    // event cards are drawn from the store, so the first frame already shows
    // what storage returned instead of an empty list that later fills in.
    calendar.store = EventStore::load();
    calendar.total_events = calendar.store.len() as u32;
    std::log(0, "[CALENDAR APP] Event store:").ok();
    std::log(0, calendar.store.state.as_str()).ok();
    if calendar.date_known {
        calendar.selected_day = Some(calendar.current_day as u8);
    }

    if surface > 0 {
        render_calendar_ui(surface, &calendar);
        let _ = std::surface_flip(6);
        std::log(
            0,
            "[CALENDAR APP] Calendar canvas presented to scanout buffer.",
        )
        .ok();
    }

    // 5. Sovereign Asynchronous Event Reactor Loop
    //
    // WHY the loop repaints bands instead of re-rendering the whole surface per
    // touch: the app used to draw exactly once at boot and then never again, so
    // nothing a tap did was ever visible. A band repaint is what makes the
    // month grid selectable and makes a committed event appear in the agenda.
    let interest_mask = std::EVENT_TOUCH_INPUT | std::EVENT_IPC_MSG | std::EVENT_TIMER_TICK;
    loop {
        match std::event_poll(interest_mask, 150) {
            Ok(events) => {
                if events & std::EVENT_TOUCH_INPUT != 0 {
                    let mut input = std::InputEvent {
                        kind: 0,
                        x: 0,
                        y: 0,
                        dx: 0,
                        dy: 0,
                        scale_milli: 1000,
                    };
                    let mut dirty = Dirty::EMPTY;
                    while std::input_event_poll(&mut input).is_ok() {
                        if input.kind == 4 || input.kind == 6 || input.kind == 7 || input.kind == 8 || input.kind == 10 {
                            std::log(0, "[CALENDAR APP] Navigation gesture recognized. Exiting to launcher.").ok();
                            exit_to_launcher(surface);
                        }
                        if input.kind == 1 {
                            match handle_tap(&mut calendar, input.x as usize, input.y as usize) {
                                Tap::Leave(msg) => {
                                    std::log(0, msg).ok();
                                    exit_to_launcher(surface);
                                }
                                Tap::Redraw(d) => dirty = dirty.widen(d),
                                Tap::None => {}
                            }
                        }
                    }
                    if !dirty.is_empty() && surface > 0 {
                        repaint(surface, &calendar, dirty);
                    }
                }
                if events & std::EVENT_IPC_MSG != 0 {
                    // WHY: no intent payload was fetched from the IPC bus and no
                    // parser ran on one. Report the event; never claim a schedule
                    // intent was parsed out of a raw event bit.
                    std::log(
                        0,
                        "[CALENDAR APP] IPC message event raised; no schedule intent payload was read.",
                    )
                    .ok();
                }
                if events & std::EVENT_TIMER_TICK != 0 {
                    // Periodic alarm & reminder monitor
                    if calendar.sync_rtc() {
                        calendar.check_due_notifications();
                    }
                    std::yield_now();
                }
            }
            Err(_) => {
                std::yield_now();
            }
        }
    }
}

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    std::log(0, "[CALENDAR APP] Panic detected! Terminating task...").ok();
    std::exit();
}
