// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

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
//! - Dedicated Month, Week, Day, and Agenda views with comfortable >= 48px touch targets
//! - Event Details card with deletion and touch Clear [X] affordances
//! - Upgraded typography using Scale 2 Body for comfortable readability on 360x720 / 720x1440 screens

use interos_std as std;
use calendar::theme::{radius, type_scale, THEME};
use core::panic::PanicInfo;

use interos_std::design::components;
use interos_std::design::accessibility::{DynamicType, UserTextScale};
use interos_std::design::outline_text;

pub use calendar::{
    chevron_pixel, days_in_month, day_of_week, event_location, is_leap_year,
    rtc_report_is_new, step_selected_day, ChevronDir, DayArrow,
    EventRecord, StoreState,
    DAY_ARROW_BAND_Y0, DAY_ARROW_BAND_Y1,
    DAY_NEXT_ARROW_APEX_X, DAY_NEXT_ARROW_APEX_Y, DAY_NEXT_ARROW_X0, DAY_NEXT_ARROW_X1,
    DAY_PREV_ARROW_APEX_X, DAY_PREV_ARROW_APEX_Y, DAY_PREV_ARROW_X0, DAY_PREV_ARROW_X1,
    EVENT_TITLE_MAX, MAX_EVENTS, RECORD_BYTES, STORE_BLOB_BYTES, STORE_KEY, STORE_VERSION,
    STORE_YEAR_MAX, STORE_YEAR_MIN,
};

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

/// Encodes the used records into one settings value and writes it.
fn write_store(records: &[EventRecord; MAX_EVENTS], len: usize) -> bool {
    let mut blob = [0u8; STORE_BLOB_BYTES];
    blob[0] = STORE_VERSION;
    blob[1] = len as u8;
    for i in 0..len {
        let start = 2 + i * RECORD_BYTES;
        records[i].encode(&mut blob[start..start + RECORD_BYTES]);
    }
    std::settings_set(STORE_KEY, &blob[..2 + len * RECORD_BYTES]).is_ok()
}

/// The agenda: bounded records plus an honest account of the last load or save.
#[derive(Clone, Copy, PartialEq, Eq)]
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

    pub fn has_event_on(&self, year: u16, month: u8, day: u8) -> bool {
        for i in 0..self.len as usize {
            let r = &self.records[i];
            if r.on_day(year, month, day) {
                return true;
            }
        }
        false
    }

    fn load() -> Self {
        let mut blob = [0u8; STORE_BLOB_BYTES];
        match std::settings_get(STORE_KEY, &mut blob) {
            Ok(0) => EventStore { state: StoreState::Empty, ..EventStore::EMPTY },
            Ok(n) => match calendar::EventStore::decode_from_blob(&blob[..n]) {
                Ok(decoded) => EventStore {
                    records: decoded.records,
                    len: decoded.len,
                    state: decoded.state,
                },
                Err(state) => EventStore { state, ..EventStore::EMPTY },
            },
            Err(_) => EventStore { state: StoreState::ReadRefused, ..EventStore::EMPTY },
        }
    }

    fn len(&self) -> usize {
        self.len as usize
    }

    fn is_full(&self) -> bool {
        self.len() >= MAX_EVENTS
    }

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

    fn delete_event(&mut self, rec: &EventRecord) -> bool {
        let count = self.len();
        for i in 0..count {
            if &self.records[i] == rec {
                let mut records = self.records;
                for j in i..count - 1 {
                    records[j] = records[j + 1];
                }
                records[count - 1] = EventRecord::EMPTY;
                let len = count - 1;
                if write_store(&records, len) {
                    self.records = records;
                    self.len = len as u8;
                    self.state = StoreState::Written;
                    return true;
                } else {
                    self.state = StoreState::WriteRefused;
                    return false;
                }
            }
        }
        false
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

    /// Collects all events in chronological order across the calendar.
    fn all_events_sorted(&self, out: &mut [EventRecord; MAX_EVENTS]) -> usize {
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

/// Which view mode is active.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ViewMode {
    Month = 0,
    Week = 1,
    Day = 2,
    Agenda = 3,
}

/// Which step of the create-event flow is on screen.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Flow {
    /// No flow: the calendar view is interactive.
    None,
    /// Title entry on the on-screen keyboard.
    Title,
    /// Date entry on the year/month/day steppers.
    Date,
    /// Time entry, then commit.
    Time,
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
    /// The civil date this app last put on the console.
    rtc_reported_date: Option<(u32, u8, u8)>,
    /// Whether the current "the clock did not answer" state has been reported.
    rtc_unknown_reported: bool,
    /// The day the month grid highlights and the agenda lists.
    selected_day: Option<u8>,
    /// Events read from, and written to, the platform settings store.
    store: EventStore,
    /// The active view mode: Month, Week, Day, Agenda.
    view_mode: ViewMode,
    /// Selected event opened in the Event Details modal.
    details_event: Option<EventRecord>,
    /// The create-event step currently on screen.
    flow: Flow,
    /// Title being typed, bounded by `EVENT_TITLE_MAX`.
    title: [u8; EVENT_TITLE_MAX],
    title_len: usize,
    /// Date being picked.
    edit_year: Option<u16>,
    edit_month: Option<u8>,
    edit_day: Option<u8>,
    edit_hour: Option<u8>,
    edit_minute: Option<u8>,
    /// What the last commit actually did, in words.
    commit_status: &'static str,
    clock: [u8; 5],
    clock_ok: bool,
}

impl CalendarApp {
    pub const fn new() -> Self {
        Self {
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
            rtc_reported_date: None,
            rtc_unknown_reported: false,
            selected_day: None,
            store: EventStore::EMPTY,
            view_mode: ViewMode::Month,
            details_event: None,
            flow: Flow::None,
            title: [0; EVENT_TITLE_MAX],
            title_len: 0,
            edit_year: None,
            edit_month: None,
            edit_day: None,
            edit_hour: None,
            edit_minute: None,
            commit_status: "NO EVENT CREATED YET",
            clock: [b' '; 5],
            clock_ok: false,
        }
    }

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

    fn push_title(&mut self, b: u8) -> bool {
        if self.title_len >= EVENT_TITLE_MAX {
            return false;
        }
        self.title[self.title_len] = b;
        self.title_len += 1;
        true
    }

    pub fn sync_rtc(&mut self) -> bool {
        if let Some(now) = std::rtc::Reading::now() {
            self.current_year = now.year;
            self.current_month = now.month as u8;
            self.current_day = now.day as u8;
            self.current_hour = now.hour as u8;
            self.current_minute = now.minute as u8;
            self.clock = now.clock_text();
            self.clock_ok = true;
            self.date_known = true;
            self.rtc_synchronized = true;
        } else {
            self.clock_ok = false;
        }
        let mut time_buf = [0u8; 64];
        match std::rtc_get_time(&mut time_buf) {
            Ok(n) if n > 0 => {
                let payload = &time_buf[..n];

                // WHY the SDK reads the fields and not this app: the calendar
                // used to scan the payload itself and then mask each time field
                // into a byte with `& 0xFF`, so an hour the clock never
                // reported -- 25, or 300 -- became 01 or 44 and was printed as
                // a time. Worse, a reminder stored at 08:30 could then never
                // match a `current_minute` of 286. The shared reader bounds
                // every field, and refuses the whole reading if any of them is
                // out of range, so a partial time is no longer shown as one.
                match std::rtc::from_payload(payload) {
                    Some(now) => {
                        self.current_year = now.year;
                        self.current_month = now.month as u8;
                        self.current_day = now.day as u8;
                        self.current_hour = now.hour as u8;
                        self.current_minute = now.minute as u8;
                        self.date_known = true;
                        self.rtc_synchronized = true;
                        // WHY this is gated: `sync_rtc` runs from the timer
                        // branch about twice a second, and used to print the
                        // whole payload plus a success line every time, so one
                        // backgrounded instance buried the console in tens of
                        // thousands of identical lines and pushed real input
                        // answers seconds late. A civil date is news once.
                        let today = (now.year, now.month as u8, now.day as u8);
                        if rtc_report_is_new(self.rtc_reported_date, today) {
                            self.rtc_reported_date = Some(today);
                            self.rtc_unknown_reported = false;
                            std::log("[CALENDAR APP] RTC payload as reported by the kernel:").ok();
                            std::log(core::str::from_utf8(payload).unwrap_or("<non-utf8>")).ok();
                            std::log("[CALENDAR APP] Hardware RTC synchronized: civil date read from the RTC payload.",
                            )
                            .ok();
                        }
                        return true;
                    }
                    None => {
                        self.report_unknown_state("[CALENDAR APP] RTC payload carried no usable civil date; current date unknown.");
                    }
                }
            }
            Ok(_) => {
                self.report_unknown_state("[CALENDAR APP] RTC returned an empty payload; current date unknown.");
            }
            Err(_) => {
                self.report_unknown_state("[CALENDAR APP] RTC query refused by the kernel; current date unknown.");
            }
        }
        false
    }

    /// Puts a clock failure on the console once, not once per timer tick.
    fn report_unknown_state(&mut self, message: &str) {
        if !self.rtc_unknown_reported {
            self.rtc_unknown_reported = true;
            self.rtc_reported_date = None;
            std::log(message).ok();
        }
    }

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
                std::log("[CALENDAR APP] Triggered notification for scheduled event.").ok();
            }
        }
    }

    pub fn schedule_event(&mut self, event_name: &'static str) -> bool {
        let mut agenda_sector = [0u8; 512];
        let copy_len = event_name.len().min(128);
        agenda_sector[..copy_len].copy_from_slice(&event_name.as_bytes()[..copy_len]);
        let mut iv = [0u8; 16];
        if std::get_random(&mut iv).is_err() {
            std::log("[CALENDAR APP] Refusing to encrypt agenda event without cryptographic TRNG entropy.",
            )
            .ok();
            return false;
        }
        match std::vault_encrypt_sector(203, &mut agenda_sector, &iv) {
            Ok(_) => {
                self.sector_encrypted = true;
                std::log("[CALENDAR APP] Sector 203: Updated agenda sealed and persisted by the storage vault.",
                )
                .ok();
            }
            Err(_) => {
                std::log("[CALENDAR APP] Sector 203: vault write refused by the kernel; event not persisted.",
                )
                .ok();
            }
        }

        let mut tones_played = 0u32;
        for freq in [440u32, 554, 659] {
            if std::audio_play_tone(freq).is_ok() {
                tones_played += 1;
            }
        }
        let _ = std::audio_silence();
        let _ = std::audio_haptic_click();
        if tones_played == 3 {
            std::log("[CALENDAR APP] Agenda reminder chime emitted (A-Major: 440Hz -> 554Hz -> 659Hz).",
            )
            .ok();
        } else {
            std::log("[CALENDAR APP] Reminder chime not emitted: the audio service refused the tone requests.",
            )
            .ok();
        }
        self.sector_encrypted
    }
}

// -----------------------------------------------------------------------------
// Visual Design Tokens & Geometry
// -----------------------------------------------------------------------------

const ACTION_BTN_LABEL: &[u8] = b"+ New Event";
const ACTION_BTN_SCALE: usize = type_scale::BODY;
const ACTION_BTN_W: usize = 308;
const ACTION_BTN_H: usize = 56;
const ACTION_BTN_X0: usize = (720 - ACTION_BTN_W) / 2;
const ACTION_BTN_Y0: usize = 800;
const ACTION_BTN_X1: usize = ACTION_BTN_X0 + ACTION_BTN_W;
const ACTION_BTN_Y1: usize = ACTION_BTN_Y0 + ACTION_BTN_H;

/// WHY the button is one box for every view: Week, Day and Agenda painted and
/// hit-tested it from the literals `816` and `872` while Month painted it from
/// these constants at `800..856`, so the same control sat sixteen rows lower in
/// three of the four views, and the cards stacked above it -- which are capped
/// by this band's top edge -- were measured against a box the button no longer
/// occupied. Every view now reads the same two numbers.
const _: () = assert!(ACTION_BTN_Y1 <= H);

// -----------------------------------------------------------------------------
// The card chains the four views stack above the action button
//
// WHY each chain is capped: a per-pixel painter has no clip, so a card whose
// box runs past the top of the button does not stop at the button -- the first
// branch to claim the pixel wins, and the card branch is written before the
// button branch. Day view drew a fifth card at `810..870`, six rows of it above
// the button and the rest of it untappable, and the tap handler measured the
// same chain with no band at all, so the visible "+ New Event" opened the event
// instead. Agenda view did the same with its sixth card. A chain now reserves
// exactly the cards that end at or above the button, and the gates beside each
// count fail the build if a card is ever made to overlap the control.
// -----------------------------------------------------------------------------

/// The top of the Day view's timeline cards.
const DAY_CARDS_Y0: usize = 266;

/// The height of a Day view card.
const DAY_CARD_H: usize = 120;

/// The stride between two Day view cards.
const DAY_CARD_PITCH: usize = 136;

/// How many Day view cards fit above the action button.
const DAY_CARD_MAX: usize =
    (ACTION_BTN_Y0 - DAY_CARDS_Y0 - DAY_CARD_H) / DAY_CARD_PITCH + 1;

const _: () = assert!(DAY_CARD_H <= ACTION_BTN_Y0 - DAY_CARDS_Y0);
const _: () = assert!((DAY_CARD_MAX - 1) * DAY_CARD_PITCH + DAY_CARD_H <= ACTION_BTN_Y0 - DAY_CARDS_Y0);

/// The Day view's "nothing scheduled" card.
///
/// WHY the card's corners, hairline and fill all read this one box: the window
/// that let the painter claim the card's pixels ran from `276` to `536` while
/// the maths inside it measured the card from `260` to `519`, so the card's top
/// sixteen rows were never painted, sixteen rows below its bottom border were
/// painted as fill, and `519 - y` subtracted past zero for every row of that
/// tail -- a border that closed the card on three sides only, with a slab of
/// its own background hanging off the fourth.
const DAY_EMPTY_X0: usize = 24;
const DAY_EMPTY_W: usize = W - 2 * DAY_EMPTY_X0;
const DAY_EMPTY_Y0: usize = 276;
const DAY_EMPTY_H: usize = 260;
const DAY_EMPTY_Y1: usize = DAY_EMPTY_Y0 + DAY_EMPTY_H;
const DAY_EMPTY_HEAD_Y: usize = 356;
const DAY_EMPTY_BODY_Y: usize = DAY_EMPTY_HEAD_Y + std::design::font::line_height(2);

const _: () = assert!(DAY_EMPTY_BODY_Y + std::design::font::line_height(2) < DAY_EMPTY_Y1);
const _: () = assert!(DAY_EMPTY_Y1 <= ACTION_BTN_Y0);

// -----------------------------------------------------------------------------
// The Month view's section band and its event-count pill
//
// WHY the pill's radius comes from the band's height: the corner test asks
// whether a pixel lies inside a circle of the radius, centred one radius in
// from each edge. The band used to be a remembered twenty-eight rows carrying
// the platform's soft pill radius of sixteen, so the two corner circles of one
// end overlapped and their union covered the whole end of the box: at the first
// column the test dropped every row, at the second all but six of them, and the
// count pill read as torn rather than round. The platform's own primitive
// clamps a radius to half the box it is given; this box is drawn by hand, so it
// now asks the same question the clamp asks. Because the height it asks about
// comes from the rung the caption is set in, a ladder that raises the caption
// widens the pill with it instead of underflowing the centring below.
// -----------------------------------------------------------------------------

/// The top of the Month view's "Events on <date>" band.
const SECTION_BAND_Y0: usize = 516;

/// The rung the band's title and its event count are set in.
const SECTION_TITLE_SCALE: usize = type_scale::CAPTION;

/// The rows the band's caption inks.
///
/// WHY read from the renderer and not from the rung's line box alone: a caption
/// is drawn by whichever face fits, and the bitmap cell's baseline can sit below
/// the outline ascent, which makes the inked band taller than `line_height`.
/// [`outline_text::row_band`] is the union of the two, so this is the number the
/// band has to hold rather than the number one of the two faces needs.
const SECTION_TITLE_ROWS: usize = outline_text::row_band(0, SECTION_TITLE_SCALE).1;

/// The air between the caption's box and each edge of the band.
///
/// WHY two rows: the hairline that opens the band is painted in its first two
/// rows and the count pill's own border in its last two, so a box set at the
/// band's top would have both of them inked across the run's own leading.
const SECTION_BAND_PAD: usize = 2;

/// The height of that band: the caption's box, plus the air it needs.
///
/// WHY derived: the band was a remembered twenty-eight rows for a caption whose
/// box grew to thirty with the ladder, and the centring underflowed the moment
/// the content outgrew the frame. The container follows the text.
const SECTION_BAND_H: usize = SECTION_TITLE_ROWS + SECTION_BAND_PAD * 2;

/// The row the band ends on -- exclusive.
const SECTION_BAND_Y1: usize = SECTION_BAND_Y0 + SECTION_BAND_H;

/// Where the band's title sits inside it.
///
/// WHY both of the band's runs use it: the section title and the count beside
/// it are one line of text, and the count was centred from the band's line box
/// while the title was set six rows down, so the two sat on baselines three rows
/// apart and the line read as two unrelated labels.
const SECTION_TEXT_Y: usize = (SECTION_BAND_H - SECTION_TITLE_ROWS) / 2;

/// The left edge of the event-count pill.
const BADGE_X0: usize = 540;

/// The width of the event-count pill, to the content column's right edge.
const BADGE_W: usize = W - 24 - BADGE_X0;

/// The pill's corner radius: half the band that holds it.
const BADGE_R: usize = SECTION_BAND_H / 2;

const _: () = assert!(2 * BADGE_R <= SECTION_BAND_H);
const _: () = assert!(SECTION_TEXT_Y + std::design::font::line_height(1) <= SECTION_BAND_H);
const _: () = assert!(SECTION_TEXT_Y + SECTION_TITLE_ROWS <= SECTION_BAND_H);
const _: () = assert!(BADGE_X0 + BADGE_W <= W);
/// WHY the band is gated against both of its neighbours: it starts on the row the
/// month grid ends on, and the list starts below it, so a band that grew
/// unnoticed would have climbed into the grid's last row -- which the grid's
/// branch claims first, so the band simply lost its top -- or painted over the
/// first card's border, which its own branch claims first. The list's top is now
/// read from the band's bottom, so the second gate says out loud what that
/// derivation promises: if the list is ever pinned back to a remembered row, the
/// band cannot silently overlap it.
const _: () = assert!(GRID_START_Y + GRID_ROWS * ROW_PITCH <= SECTION_BAND_Y0);
const _: () = assert!(SECTION_BAND_Y1 <= MONTH_LIST_Y0);

/// The Agenda view's chronological cards are laid out from the person's text rung
/// by the faces in `calendar` (`agenda_card_h_at`, `agenda_line_y_at`,
/// `agenda_card_pitch_at`, `agenda_card_max_at`), with the pixels this screen was
/// drawn with held beside them as compile-time anchors.
///
/// WHY they live in the library and not here: the painter and `handle_tap` answer
/// the same cards, and a rung-aware plate inked from one copy of the arithmetic
/// and tapped from another is the drift this app's own gates keep refusing.
///
/// WHY the band's floor is pinned here as well: the chain is measured against the
/// row the action button starts on, and the two statements of that row have to be
/// one number.
const _: () = assert!(ACTION_BTN_Y0 == calendar::AGENDA_LIST_Y1);

/// The air between the section band's bottom edge and the list under it.
///
/// WHY it is named: the list's top was a remembered row of its own, two under a
/// remembered band, so the band had nowhere to grow -- a band that takes the rows
/// its caption needs would have spent the surplus over the first card's top
/// border, and the band's branch claims those rows first, so the card would only
/// lose its edge.
const MONTH_LIST_GAP: usize = 2;

/// The band the Month view's saved-day list is drawn and tapped in.
///
/// WHY its top is derived from the band above it: the list sits on that band, so
/// the band decides where the list starts and a ladder that raises the caption
/// moves the list down instead of painting over it.
const MONTH_LIST_Y0: usize = SECTION_BAND_Y1 + MONTH_LIST_GAP;
const MONTH_LIST_Y1: usize = 1360;
/// The height and stride of one card in that list.
const MONTH_CARD_H: usize = 114;
const MONTH_CARD_PITCH: usize = 130;

/// How many whole cards the band holds.
const MONTH_CARD_MAX: usize =
    (MONTH_LIST_Y1 - MONTH_LIST_Y0 - MONTH_CARD_H) / MONTH_CARD_PITCH + 1;

const _: () = assert!(MONTH_CARD_H <= MONTH_LIST_Y1 - MONTH_LIST_Y0);
const _: () = assert!(
    (MONTH_CARD_MAX - 1) * MONTH_CARD_PITCH + MONTH_CARD_H <= MONTH_LIST_Y1 - MONTH_LIST_Y0
);

/// The Month view's "nothing listed" card: one box over the list band.
///
/// WHY its top is the list band's own: the card stands in for the cards, so it
/// starts where they start and a band that moves down carries the card with it.
/// Left behind, the card's first rows would fall in the section band above, whose
/// branch claims those pixels first, and the box would lose its top border and
/// both rounded caps.
const MONTH_EMPTY_CARD_H: usize = 324;
const MONTH_EMPTY_CARD_Y0: usize = MONTH_LIST_Y0;
const MONTH_EMPTY_CARD_Y1: usize = MONTH_EMPTY_CARD_Y0 + MONTH_EMPTY_CARD_H;

/// The rows from the empty card's top to the tear-sheet mark's centre, to the
/// headline's box and to the detail line's box.
///
/// WHY measured from the card and not from the surface: they are the card's own
/// composition, so a card that moves has to move what it holds with it.
const MONTH_EMPTY_MARK_DY: usize = 90;
const MONTH_EMPTY_HEADLINE_DY: usize = 160;
const MONTH_EMPTY_DETAIL_DY: usize = 214;

const _: () = assert!(MONTH_EMPTY_CARD_Y1 <= MONTH_LIST_Y1);
const _: () = assert!(
    MONTH_EMPTY_CARD_Y0 + MONTH_EMPTY_DETAIL_DY + std::design::font::line_height(1)
        <= ACTION_BTN_Y0
);
const _: () = assert!(ACTION_BTN_Y1 <= MONTH_EMPTY_CARD_Y1);

// Segmented Navigation Tabs (y: 100..152) - comfortable >= 48px touch targets
const TAB_SCALE: usize = type_scale::BODY;
const TAB_BAND_Y0: usize = 116;
const TAB_BAND_H: usize = 50;
const TAB_W: usize = 147;
const TAB1_X0: usize = 48;
const TAB1_X1: usize = TAB1_X0 + TAB_W; // 195
const TAB2_X0: usize = 207;
const TAB2_X1: usize = TAB2_X0 + TAB_W; // 354
const TAB3_X0: usize = 366;
const TAB3_X1: usize = TAB3_X0 + TAB_W; // 513
const TAB4_X0: usize = 525;
const TAB4_X1: usize = TAB4_X0 + TAB_W; // 672
const TAB_LABELS: [&[u8]; 4] = [b"Month", b"Week", b"Day", b"Agenda"];

/// The tab band's four pills: the columns each is drawn in, and the view it names.
///
/// WHY one table for the painter and the press: the band is drawn from a strip
/// of columns and answered from another, and the two have to agree everywhere
/// the band is reachable -- including while a modal covers the rest of it.
const TABS: [(usize, usize, ViewMode); 4] = [
    (TAB1_X0, TAB1_X1, ViewMode::Month),
    (TAB2_X0, TAB2_X1, ViewMode::Week),
    (TAB3_X0, TAB3_X1, ViewMode::Day),
    (TAB4_X0, TAB4_X1, ViewMode::Agenda),
];

/// The view a press in the tab band asks for, named by the pill it lands on.
fn tab_mode_at(x: usize) -> Option<ViewMode> {
    TABS.iter()
        .find(|(x0, x1, _)| x >= *x0 && x < *x1)
        .map(|(_, _, mode)| *mode)
}

// Text rendering helpers
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
    let line_h = std::design::font::line_height(scale);
    if y + 24 < y0 || y >= y0 + line_h.max(scale * 24) {
        return None;
    }
    if x + 16 < x0 || x >= x0 + max_w {
        return None;
    }
    let mut buf = [0u8; 96];
    let n = std::design::font::ellipsize(text, scale, max_w, &mut buf);
    let fitted = if n > 0 { &buf[..n] } else { text };
    let (bx, bt, bw, bh) = outline_text::run_box(x0, y0, scale, fitted);
    if x < bx || x - bx >= bw || y < bt || y - bt >= bh {
        return None;
    }
    if let Some(run) = outline_text::place_run(x0, y0, scale, fitted, Some(max_w), false) {
        return run.pixel(x, y, color, bg);
    }
    std::design::font::test_text_aa(x, y, x0, y0, scale, fitted, color, bg, false)
}

#[inline(always)]
fn test_text_bold(
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
    let line_h = std::design::font::line_height(scale);
    if y + 24 < y0 || y >= y0 + line_h.max(scale * 24) {
        return None;
    }
    if x + 16 < x0 || x >= x0 + max_w {
        return None;
    }
    let mut buf = [0u8; 96];
    let n = std::design::font::ellipsize(text, scale, max_w, &mut buf);
    let fitted = if n > 0 { &buf[..n] } else { text };
    let (bx, bt, bw, bh) = outline_text::run_box(x0, y0, scale, fitted);
    if x < bx || x - bx >= bw || y < bt || y - bt >= bh {
        return None;
    }
    if let Some(run) = outline_text::place_run(x0, y0, scale, fitted, Some(max_w), true) {
        return run.pixel(x, y, color, bg);
    }
    std::design::font::test_text_aa(x, y, x0, y0, scale, fitted, color, bg, true)
}

#[inline(always)]
fn centered_x(text: &[u8], scale: usize, x0: usize, available: usize) -> usize {
    let w = outline_text::drawn_advance(text, scale, Some(available))
        .unwrap_or_else(|| std::design::font::measure(text, scale));
    x0 + if w >= available { 0 } else { (available - w) / 2 }
}

/// Reads the platform's text-size setting and hands the rung to the renderer.
///
/// Returns whether the rung the last frame was laid out with actually moved,
/// which is the caller's signal that the whole screen has to be repainted and not
/// just the band something else dirtied.
///
/// WHY the shared parser and not this app's own chain of compares: the store
/// accepts both the digit and the word for a level, and a reader that understood
/// fewer spellings than the writer produces would read an existing preference as
/// "unset" and quietly reset the rung.
///
/// WHY the rung the process already holds when the store does not answer: a frame
/// that fell back to one hundred per cent because the store was busy would resize
/// the screen under a person who never touched the setting.
fn publish_text_scale() -> bool {
    let mut buf = [0u8; 32];
    let mut want = None;
    if let Ok(n) = std::settings_get("display.text_scale", &mut buf) {
        want = UserTextScale::from_settings_value(&buf[..n]);
    }
    let before = DynamicType::current().scale;
    let next = want.unwrap_or(before);
    // WHY the answer is the rung actually stored and not the one asked for: a
    // level above the ladder's largest setting is clamped on the way in, and a
    // caller that repaints because "the level changed" should be wrong about that
    // only when the pixels really did move.
    DynamicType::new(next).publish() != before
}

/// One line of an Agenda card, inked at the rung this process draws at.
///
/// WHY a separate face and not the shared [`test_text`]: only the Agenda cards'
/// plates are derived from the rung so far, so only their runs may grow. The
/// header, the tabs, the month grid's lattice, the section band, the month list,
/// the week and day views and both modals still lay out at the authored ladder,
/// and growing their ink before their boxes would clip them. At the authored level
/// this delegates straight to [`test_text`]/[`test_text_bold`] and moves no pixel.
///
/// WHY the rung arrives as an argument: the pixel face reads the ladder once and
/// hands that answer to every run in the card, so no glyph costs a read of the
/// setting. The read the pixel does pay is a relaxed load and a downward walk over
/// the crate's own const arithmetic -- the syscall that would not belong in the
/// paint path is `settings_get`, and that one lives only in the wake path.
#[inline(always)]
fn agenda_line_text(
    x: usize,
    y: usize,
    lvl: u8,
    x0: usize,
    y0: usize,
    scale: usize,
    text: &[u8],
    max_w: usize,
    color: u32,
    bg: u32,
    bold: bool,
) -> Option<u32> {
    if lvl == calendar::PLATFORM_LEVEL {
        return if bold {
            test_text_bold(x, y, x0, y0, scale, text, max_w, color, bg)
        } else {
            test_text(x, y, x0, y0, scale, text, max_w, color, bg)
        };
    }
    if scale == 0 || text.is_empty() || x < x0 || x >= x0 + max_w {
        return None;
    }
    // The run is cut to the lane at the rung's own advances, so a wider face
    // ellipsises inside the box it was trimmed for instead of walking past it.
    let mut buf = [0u8; 96];
    let n = calendar::ellipsize_run(text, scale, max_w, lvl, &mut buf);
    // WHY the refusal is answered with a cut and not with the run itself:
    // `ellipsize_run` answers 0 when even the three-byte mark will not sit in the
    // lane, and painting the untouched run then puts words past the plate that
    // framed them -- the same answer the lane gives when it does have room, minus
    // the mark.
    let fitted = if n > 0 {
        &buf[..n]
    } else {
        // WHY the run's own bytes are put into the buffer first: a refusal is the
        // one answer `ellipsize_run` gives without writing, so a cut read straight
        // back would be the zeroed buffer, not the front of the run.
        let fit = calendar::longest_fit(text, calendar::em_at(scale, lvl), max_w, buf.len());
        buf[..fit].copy_from_slice(&text[..fit]);
        &buf[..fit]
    };
    let cap = calendar::cap_at(scale, lvl);
    if let Some(run) = outline_text::place_run_cap(x0, y0, cap, fitted, None, bold) {
        let (rx, rtop, rw, rh) = run.bounds();
        if x >= rx && x - rx < rw && y >= rtop && y - rtop < rh {
            return run.pixel(x, y, color, bg);
        }
        return None;
    }
    // The bitmap face, at the rung it is asked for. The band grows with the level:
    // a run fitted to a scaled em walks taller than the unscaled cell, so the
    // ladder's band would cut the descenders of the very run it enlarged.
    if y >= y0 && y < y0 + calendar::rows_at(scale, lvl) {
        return std::design::monochrome::text_px(x, y, x0, y0, cap, fitted, color, bg, bold);
    }
    None
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
    let tail: &[u8] = if n == 1 { b"EVENT" } else { b"EVENTS" };
    if out.len() < len + tail.len() {
        return 0;
    }
    out[len..len + tail.len()].copy_from_slice(tail);
    len + tail.len()
}

const CARD: components::CardStyle =
    components::card_style(&THEME, components::Elevation::Resting);

const R_CARD: usize = CARD.radius;
const R_CHIP: usize = radius::CHIP;
const R_CONTROL: usize = radius::CONTROL;
const R_SHEET: usize = radius::SHEET;
const R_SOFT: usize = radius::SOFT_PILL;

const W: usize = 720;
const H: usize = 1440;
const CHUNK: usize = 4096;

// Month grid metrics
const COL_W: usize = 88;
const COL_PITCH: usize = 97;
const COL_X0: usize = 22;
const ROW_H: usize = 46;
const ROW_PITCH: usize = 52;
const GRID_ROWS: usize = 6;
const GRID_START_Y: usize = 204;

const WEEKDAY_Y0: usize = 172;
const WEEKDAY_SCALE: usize = type_scale::BODY;

const NO_DATE_PILL: &[u8] = b"NO DATE";
const NO_DATE_HEADLINE: &[u8] = b"NO CIVIL DATE";
const NO_DATE_DETAIL: &[u8] = b"RTC date unavailable";

// -----------------------------------------------------------------------------
// Sheet & Modal Geometries (Creation & Details)
// -----------------------------------------------------------------------------

const SHEET_X0: usize = 24;
const SHEET_X1: usize = 696;
const SHEET_Y0: usize = 530;
const SHEET_Y1: usize = 1060;
const SHEET_COMPACT_Y1: usize = 980;

const SHEET_TITLE_Y: usize = 546;
const SHEET_STEP_Y: usize = 594;

const TITLE_FIELD_X0: usize = 44;
const TITLE_FIELD_X1: usize = 676;
const TITLE_FIELD_Y0: usize = 644;
const TITLE_FIELD_Y1: usize = 712;
const TITLE_TEXT_Y: usize = 654;

// Touch Clear [X] affordance on title input field
const CLEAR_BTN_X0: usize = 616;
const CLEAR_BTN_X1: usize = 668;
const CLEAR_BTN_Y0: usize = 648;
const CLEAR_BTN_Y1: usize = 708;

/// The column the title run is drawn from, and how far it may reach.
///
/// WHY the run and its caret share one budget: the clear control is painted
/// over the right end of the field, so both the text and the mark that follows
/// it have to stop at it.
const TITLE_TEXT_X0: usize = TITLE_FIELD_X0 + 16;
const TITLE_TEXT_MAX_W: usize = CLEAR_BTN_X0 - TITLE_TEXT_X0 - 4;

/// The width of the caret block that trails the run.
const TITLE_CARET_W: usize = 8;

/// The widest the run may measure and still have its caret land inside the field.
const TITLE_CARET_TEXT_MAX: usize = TITLE_TEXT_MAX_W - TITLE_CARET_W - 2;

const _: () = assert!(TITLE_TEXT_X0 + TITLE_TEXT_MAX_W <= CLEAR_BTN_X0);
const _: () = assert!(TITLE_TEXT_X0 + TITLE_CARET_TEXT_MAX + 2 + TITLE_CARET_W <= CLEAR_BTN_X0);

// Keyboard metrics
const KB_Y0: usize = 730;
const KB_ROW_H: usize = 60;
const KB_ROW_PITCH: usize = 68;
const KB_LETTER_COLS: usize = 13;
const KB_LETTER_X0: usize = 26;
const KB_LETTER_KEY_W: usize = 46;
const KB_LETTER_PITCH: usize = 48;
const KB_DIGIT_X0: usize = 26;
const KB_DIGIT_KEY_W: usize = 60;
const KB_DIGIT_PITCH: usize = 64;
const KB_DIGIT_COLS: usize = 10;

// Keyboard bottom row with Space, Backspace [BKSP], and Next
const KB_SPACE_X0: usize = 26;
const KB_SPACE_X1: usize = 240;
const KB_DEL_X0: usize = 248;
const KB_DEL_X1: usize = 466;
const KB_NEXT_X0: usize = 474;
const KB_NEXT_X1: usize = 694;

// Steppers
fn step_btn_rects(x0: usize, x1: usize) -> ((usize, usize, usize, usize), (usize, usize, usize, usize)) {
    let y0 = STEP_BTN_Y0;
    let y1 = STEP_BTN_Y1;
    (
        (x0 + 16, y0, x0 + 16 + STEP_BTN_W, y1),
        (x1 - 16 - STEP_BTN_W, y0, x1 - 16, y1),
    )
}

const STEP_Y0: usize = 644;
const STEP_Y1: usize = 816;
const STEP_LABEL_Y: usize = 654;
const STEP_VALUE_Y: usize = 696;
const STEP_BTN_Y0: usize = 752;
const STEP_BTN_Y1: usize = 808;
const STEP_BTN_W: usize = 72;

const DATE_BOXES: [(usize, usize); 3] = [(44, 236), (252, 444), (460, 652)];
const TIME_BOXES: [(usize, usize); 2] = [(44, 344), (376, 676)];

const SAVE_BTN_X0: usize = 44;
const SAVE_BTN_X1: usize = 386;
const CANCEL_BTN_X0: usize = 400;
const CANCEL_BTN_X1: usize = 652;
const FLOW_BTN_Y0: usize = 836;
const FLOW_BTN_Y1: usize = 896;
const FLOW_STATUS_Y: usize = 918;

// Event Details Modal Geometry
const DETAILS_X0: usize = 24;
const DETAILS_X1: usize = 696;
const DETAILS_Y0: usize = 430;
const DETAILS_Y1: usize = 1010;
const DETAILS_TITLE_Y: usize = 446;
const DETAILS_CLOSE_X0: usize = 624;
const DETAILS_CLOSE_X1: usize = 676;
const DETAILS_CLOSE_Y0: usize = 438;
const DETAILS_CLOSE_Y1: usize = 490;
const DETAILS_BTN_Y0: usize = 820;
const DETAILS_BTN_Y1: usize = 880;

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
            1 => b"BKSP",
            2 => b"NEXT",
            _ => b"",
        },
        _ => b"",
    }
}

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

// -----------------------------------------------------------------------------
// Pixel Rendering Engine
// -----------------------------------------------------------------------------

const STATUS_BAR_HEIGHT: usize = interos_std::design::viewport::STATUS_BAR_HEIGHT;
const HEADER_TOP: usize = interos_std::design::viewport::HEADER_TOP;

/// The rows the header's back control and month pill stand in.
///
/// WHY read from the control and not from the band: the header's ink line is the
/// control band's floor, and a band authored from a remembered height has its
/// floor move whenever the platform's control does.
const HEADER_CONTROL_ROWS: usize = std::design::back_button::SIZE;

/// The rung the header's title run is set in.
///
/// WHY named and read by the painter as well as the band: the band is derived
/// from this rung's box, so a painter hard-coding a different rung is exactly how
/// the run and its container came to disagree.
const TITLE_SCALE: usize = type_scale::SECTION;

/// The rows a run at the title's rung inks, whatever its bytes are.
///
/// WHY read from the renderer and not from the rung's line box alone: a title is
/// drawn by whichever face fits, and the bitmap cell's baseline can sit below the
/// outline ascent, which makes the inked band taller than `line_height`.
/// [`outline_text::row_band`] is the union of the two, so this is the number the
/// header has to hold rather than the number one of the two faces needs.
const TITLE_ROWS: usize = outline_text::row_band(0, TITLE_SCALE).1;

/// The air the header band keeps between the title's box and its own floor.
///
/// WHY two rows: it is the air this app already keeps between a run's box and its
/// band's edge (`SECTION_BAND_PAD`), so a title gets the same floor as the Month
/// view's section caption.
const HEADER_BOTTOM_AIR: usize = SECTION_BAND_PAD;

/// The row the title run starts on: the control band's floor less the run's own
/// ascent, so the title's baseline lands on the back control's last row.
///
/// WHY derived from the ladder: it was a remembered nine rows, which is the same
/// number the ascent leaves over at this rung, and a rung the ladder raises would
/// have kept the old offset and slid its baseline off the header's ink line.
const TITLE_Y0: usize = HEADER_TOP + HEADER_CONTROL_ROWS
    - std::design::font::ascent_for_scale(TITLE_SCALE);

/// The header row's own box, one past its last row.
///
/// WHY derived and not 112: the back control and the month badge are both
/// `HEADER_CONTROL_ROWS` tall and start at `HEADER_TOP`, but the title's box
/// reaches `TITLE_Y0 + TITLE_ROWS`, which is ten rows below the controls' floor
/// and two below the remembered 112 the band carried once the ladder grew this
/// rung's box. The view under the header claimed those two rows before the header
/// branch was reached, so the run's last descent rows were painted over. The
/// container follows the text.
const HEADER_H: usize = TITLE_Y0 + TITLE_ROWS + HEADER_BOTTOM_AIR;

/// WHY strict: the band's floor has to stay below the run's box by real air rather
/// than have the box land exactly on the row the band names, which is the
/// off-by-one this gate replaces.
const _: () = assert!(TITLE_Y0 + TITLE_ROWS < HEADER_H);
/// WHY also against the control band: the header holds both the controls and the
/// title, and a container derived from the text alone could one day stop short of
/// the back control's last row and hand it to the view below.
const _: () = assert!(HEADER_TOP + HEADER_CONTROL_ROWS <= HEADER_H);
/// WHY gated against the tab band and the weekday row: the header branch answers
/// every row up to `HEADER_H`, the segmented tabs claim theirs from
/// `TAB_BAND_Y0`, and the weekday row claims the view's top from `WEEKDAY_Y0` --
/// a band that grew past either would silently lose its last rows to the
/// neighbour that claims them first, which is the same loss it just suffered from
/// below.
const _: () = assert!(HEADER_H <= TAB_BAND_Y0);
const _: () = assert!(TAB_BAND_Y0 + TAB_BAND_H <= WEEKDAY_Y0);

pub fn status_band() -> (usize, usize) {
    (0, STATUS_BAR_HEIGHT)
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
    // 0. Status Band (y < STATUS_BAR_HEIGHT)
    if y < STATUS_BAR_HEIGHT {
        let band = interos_std::design::components::StatusBand::new(W);
        let clock = if app.clock_ok { Some(&app.clock[..]) } else { None };
        return band
            .pixel(x, y, clock, &interos_std::design::components::Instruments::unmeasured(), &THEME)
            .unwrap_or(THEME.surface);
    }

    // 1. Header row: back control, title and month badge (y: HEADER_TOP..HEADER_H)
    if y < HEADER_H {
        // Standardized back button squircle (24..72, HEADER_TOP..HEADER_TOP+48)
        if let Some(c) = std::design::back_button::pixel_for_theme(x, y, &THEME) {
            return c;
        }

        // Header Title: "Calendar"
        if y >= TITLE_Y0 && y < TITLE_Y0 + TITLE_ROWS {
            if let Some(c) = test_text(x, y, 116, TITLE_Y0, TITLE_SCALE, b"Calendar", 480 - 116, THEME.on_surface, THEME.surface_container) {
                return c;
            }
        }

        // Month & Year Pill Badge at top-right (x: 480..696, HEADER_TOP..HEADER_TOP+48)
        if x >= 480 && x < 696 && y >= HEADER_TOP && y < HEADER_TOP + 48 {
            let mut on_pill_border = false;
            let rx = (x - 480).min(695 - x);
            let ry = (y - HEADER_TOP).min(HEADER_TOP + 47 - y);
            if rx < R_CARD && ry < R_CARD {
                let cdx = (R_CARD - rx) as isize;
                let cdy = (R_CARD - ry) as isize;
                let d2 = cdx * cdx + cdy * cdy;
                if d2 >= (R_CARD * R_CARD) as isize {
                    return CARD.fill;
                }
                if d2 >= ((R_CARD - 2) * (R_CARD - 2)) as isize {
                    on_pill_border = true;
                }
            } else if rx < 2 || ry < 2 {
                on_pill_border = true;
            }

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

            let pill_text: &[u8] = if app.date_known { &month_str[..] } else { NO_DATE_PILL };
            let pill_x = centered_x(pill_text, 2, 480, 216);
            let pill_y0 = HEADER_TOP + (48 - std::design::font::line_height(2)) / 2;
            if let Some(c) = test_text(x, y, pill_x, pill_y0, 2, pill_text, 216, THEME.primary, THEME.surface_container_high) {
                return c;
            }

            if on_pill_border {
                return THEME.border;
            }
            return THEME.surface_container_high;
        }

        return THEME.surface_container;
    }

    // 2. Segmented Navigation Tabs (y: 116..166)
    if y >= TAB_BAND_Y0 && y < TAB_BAND_Y0 + TAB_BAND_H {
        return tabs_pixel(x, y, app);
    }

    // 3. Event Details Card Modal (if open)
    if let Some(event) = app.details_event {
        if y >= DETAILS_Y0 && y < DETAILS_Y1 {
            return details_pixel(x, y, app, &event);
        }
        return THEME.surface;
    }

    // 4. Create-Event Sheet Modal (if open)
    if app.flow != Flow::None {
        let sheet_y1 = match app.flow {
            Flow::Title => SHEET_Y1,
            _ => SHEET_COMPACT_Y1,
        };
        let over_sheet =
            x >= SHEET_X0 && x < SHEET_X1 && y >= SHEET_Y0 && y < sheet_y1;
        if over_sheet {
            return flow_pixel(x, y, app);
        }
        // Everything outside the sheet keeps the calendar visible under a
        // muted scrim; the old branch returned a blank page, so opening the
        // wizard looked like a screen change rather than a modal.
        let under = match app.view_mode {
            ViewMode::Month => month_view_pixel(x, y, app, display_year, display_month, start_dow, num_days, agenda),
            ViewMode::Week => week_view_pixel(x, y, app, display_year, display_month),
            ViewMode::Day => day_view_pixel(x, y, app, display_year, display_month, agenda),
            ViewMode::Agenda => agenda_view_pixel(x, y, app),
        };
        return modal_scrim(under);
    }

    // 5. Active View Mode Content (y: 156..1360)
    match app.view_mode {
        ViewMode::Month => month_view_pixel(x, y, app, display_year, display_month, start_dow, num_days, agenda),
        ViewMode::Week => week_view_pixel(x, y, app, display_year, display_month),
        ViewMode::Day => day_view_pixel(x, y, app, display_year, display_month, agenda),
        ViewMode::Agenda => agenda_view_pixel(x, y, app),
    }
}

/// Navigation tabs pixel renderer.
fn tabs_pixel(x: usize, y: usize, app: &CalendarApp) -> u32 {
    let label_y0 = TAB_BAND_Y0 + (TAB_BAND_H.saturating_sub(std::design::font::line_height(TAB_SCALE))) / 2;

    for (i, (x0, x1, mode)) in TABS.iter().enumerate() {
        if x >= *x0 && x < *x1 {
            let is_active = app.view_mode == *mode;
            let rx = (x - *x0).min(*x1 - 1 - x);
            let ry = (y - TAB_BAND_Y0).min(TAB_BAND_Y0 + TAB_BAND_H - 1 - y);

            if rx < R_CHIP && ry < R_CHIP {
                let cdx = (R_CHIP - rx) as isize;
                let cdy = (R_CHIP - ry) as isize;
                if cdx * cdx + cdy * cdy >= (R_CHIP * R_CHIP) as isize {
                    return THEME.surface;
                }
            }

            let fill = if is_active { THEME.primary } else { THEME.surface_container_high };
            let ink = if is_active { THEME.on_primary } else { THEME.on_surface_variant };
            let label = TAB_LABELS[i];
            let label_x0 = centered_x(label, TAB_SCALE, *x0, TAB_W);

            if is_active {
                if let Some(c) = test_text_bold(x, y, label_x0, label_y0, TAB_SCALE, label, TAB_W, ink, fill) {
                    return c;
                }
            } else {
                if let Some(c) = test_text(x, y, label_x0, label_y0, TAB_SCALE, label, TAB_W, ink, fill) {
                    return c;
                }
            }

            if rx < 2 || ry < 2 {
                return if is_active { THEME.primary } else { THEME.border };
            }
            return fill;
        }
    }

    THEME.surface
}

// -----------------------------------------------------------------------------
// View 1: Month View
// -----------------------------------------------------------------------------

fn month_view_pixel(
    x: usize,
    y: usize,
    app: &CalendarApp,
    _display_year: u32,
    _display_month: u8,
    start_dow: u8,
    num_days: u8,
    agenda: &AgendaView,
) -> u32 {
    // Weekday Header (y: 160..196)
    if y >= WEEKDAY_Y0 && y < WEEKDAY_Y0 + std::design::font::line_height(WEEKDAY_SCALE) {
        if app.date_known {
            const HEADERS: [&[u8]; 7] = [b"Sun", b"Mon", b"Tue", b"Wed", b"Thu", b"Fri", b"Sat"];
            for c in 0..7 {
                let col_x = 22 + c * 97;
                let text_x = centered_x(HEADERS[c], WEEKDAY_SCALE, col_x, COL_W);
                if let Some(color) = test_text_bold(x, y, text_x, WEEKDAY_Y0, WEEKDAY_SCALE, HEADERS[c], COL_W, THEME.on_surface, THEME.surface) {
                    return color;
                }
            }
        } else {
            let text_x = centered_x(NO_DATE_HEADLINE, WEEKDAY_SCALE, 0, W);
            if let Some(c) = test_text(x, y, text_x, WEEKDAY_Y0, WEEKDAY_SCALE, NO_DATE_HEADLINE, W, THEME.on_surface_variant, THEME.surface) {
                return c;
            }
        }
        return THEME.surface;
    }

    // Month Grid (y: 204..516)
    if y >= GRID_START_Y && y < GRID_START_Y + GRID_ROWS * ROW_PITCH {
        if !app.date_known {
            let detail_y = GRID_START_Y + GRID_ROWS * ROW_PITCH / 2;
            if let Some(c) = test_text(
                x,
                y,
                centered_x(NO_DATE_DETAIL, 2, 0, W),
                detail_y,
                2,
                NO_DATE_DETAIL,
                W,
                THEME.on_surface_variant,
                THEME.surface,
            ) {
                return c;
            }
            return THEME.surface;
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
                        let is_current = app.selected_day == Some(day_num as u8);
                        let d = day_num as u8;
                        let text_y = cell_y + (ROW_H - std::design::font::line_height(2)) / 2;
                        let text: [u8; 2] = if d < 10 {
                            [b'0' + d, 0]
                        } else {
                            [b'0' + d / 10, b'0' + d % 10]
                        };
                        let n = if d < 10 { 1 } else { 2 };
                        let text_x = centered_x(&text[..n], 2, col_x, COL_W);

                        let cx = (col_x + COL_W / 2) as isize;
                        let cy = (cell_y + ROW_H / 2) as isize;
                        let has_event = app.store.has_event_on(app.current_year as u16, app.current_month, d);

                        if is_current {
                            let dx = (x as isize) - cx;
                            let dy = (y as isize) - cy;
                            let rad = 22isize;
                            if dx * dx + dy * dy <= rad * rad {
                                if let Some(color) = test_text_bold(x, y, text_x, text_y, 2, &text[..n], COL_W, THEME.on_primary, THEME.primary) {
                                    return color;
                                }
                                return THEME.primary;
                            }
                            return THEME.surface;
                        } else {
                            if let Some(color) = test_text_bold(x, y, text_x, text_y, 2, &text[..n], COL_W, THEME.on_surface, THEME.surface) {
                                return color;
                            }
                            if has_event {
                                let dot_dx = (x as isize) - cx;
                                let dot_dy = (y as isize) - (cy + 17);
                                if dot_dx * dot_dx + dot_dy * dot_dy <= 5 {
                                    return THEME.primary;
                                }
                            }
                            return THEME.surface;
                        }
                    }
                }
            }
        }
        return THEME.surface;
    }

    // Section Divider & Header (SECTION_BAND_Y0..SECTION_BAND_Y1)
    if y >= SECTION_BAND_Y0 && y < SECTION_BAND_Y1 {
        // The hairline that opens this band.
        //
        // WHY it keeps to the band's first two rows and stops at the badge: it
        // used to run the panel's whole width at offsets 2..3, and the count
        // pill's own border is drawn at offsets 0..1 -- so the rule thickened
        // that pill to a four-row top, filled the two columns beside each
        // rounded cap that the pill had already decided were surface, and
        // stepped one row into the header's line box, which the caption's box
        // of 22 rows then placed at offset 3.
        if y >= SECTION_BAND_Y0 && y < SECTION_BAND_Y0 + 2 && x >= 24 && x < BADGE_X0 {
            return THEME.border;
        }

        let mut title_buf = [0u8; 16];
        let section_title = app.agenda_title_into(&mut title_buf);
        // WHY the header is caption scale: the band is the caption's own box
        // plus the air named beside it, and a body-scale run reserves a taller
        // box than that, so a larger run could only ever be clipped by the band
        // it is drawn in.
        if y >= SECTION_BAND_Y0 + SECTION_TEXT_Y
            && y < SECTION_BAND_Y0 + SECTION_TEXT_Y + std::design::font::line_height(1)
        {
            if let Some(c) = test_text(
                x,
                y,
                28,
                SECTION_BAND_Y0 + SECTION_TEXT_Y,
                1,
                section_title,
                BADGE_X0 - 28,
                THEME.on_surface_variant,
                THEME.surface,
            ) {
                return c;
            }
        }

        // Event Count Pill
        if x >= BADGE_X0 && x < BADGE_X0 + BADGE_W && y >= SECTION_BAND_Y0 && y < SECTION_BAND_Y1 {
            let mut on_badge_border = false;
            let rx = (x - BADGE_X0).min(BADGE_X0 + BADGE_W - 1 - x);
            let ry = (y - SECTION_BAND_Y0).min(SECTION_BAND_Y1 - 1 - y);
            if rx < BADGE_R && ry < BADGE_R {
                let cdx = (BADGE_R - rx) as isize;
                let cdy = (BADGE_R - ry) as isize;
                let d2 = cdx * cdx + cdy * cdy;
                if d2 >= (BADGE_R * BADGE_R) as isize {
                    return THEME.surface;
                }
                if d2 >= ((BADGE_R - 2) * (BADGE_R - 2)) as isize {
                    on_badge_border = true;
                }
            } else if rx < 2 || ry < 2 {
                on_badge_border = true;
            }

            let mut badge = [0u8; 12];
            let badge_n = fmt_count(&mut badge, agenda.len);
            let badge_str = &badge[..badge_n];
            let badge_x = centered_x(badge_str, 1, BADGE_X0, BADGE_W);
            let badge_y = SECTION_BAND_Y0 + SECTION_TEXT_Y;
            if let Some(c) = test_text(
                x,
                y,
                badge_x,
                badge_y,
                1,
                badge_str,
                BADGE_W,
                THEME.primary,
                THEME.surface_container_high,
            ) {
                return c;
            }

            if on_badge_border {
                return THEME.border;
            }
            return THEME.surface_container_high;
        }

        return THEME.surface;
    }

    // Agenda List / Empty State Container (MONTH_LIST_Y0..MONTH_LIST_Y1)
    if y >= MONTH_LIST_Y0 && y < MONTH_LIST_Y1 {
        if agenda.len == 0 {
            return empty_state_pixel(x, y, app);
        } else {
            return event_list_pixel(x, y, agenda.items.as_slice(), agenda.len);
        }
    }

    // Home Gesture Pill -- the SDK's own mark, from the same declaration
    // `handle_tap` answers below.
    if let Some(c) = std::design::home_band::pixel(x, y, THEME.on_surface_variant) {
        return c;
    }

    THEME.surface
}

// -----------------------------------------------------------------------------
// View 2: Week View
// -----------------------------------------------------------------------------

fn week_view_pixel(
    x: usize,
    y: usize,
    app: &CalendarApp,
    display_year: u32,
    display_month: u8,
) -> u32 {
    // Header (y: 176..212)
    if y >= 176 && y < 212 {
        if let Some(c) = test_text_bold(x, y, 28, 180, 2, b"WEEKLY SCHEDULE", 400, THEME.on_surface, THEME.surface) {
            return c;
        }
        if let Some(c) = test_text(x, y, 560, 180, 2, b"7 DAYS", 120, THEME.primary, THEME.surface) {
            return c;
        }
        return THEME.surface;
    }

    // 7 Day Cards (y: 220..808)
    let selected_d = app.selected_day.unwrap_or(app.current_day.max(1));
    let dow = if app.date_known { day_of_week(display_year, display_month, selected_d) } else { 0 };
    let sunday = if selected_d > dow { selected_d - dow } else { 1 };
    let num_days = if app.date_known { days_in_month(display_year, display_month) } else { 31 };

    const WEEK_LABELS: [&[u8]; 7] = [b"SUN", b"MON", b"TUE", b"WED", b"THU", b"FRI", b"SAT"];

    for i in 0..7 {
        let card_y = 220 + i * 84;
        if y >= card_y && y < card_y + 74 && x >= 24 && x < W - 24 {
            let day_num = (sunday + i as u8).min(num_days);
            let is_sel = app.selected_day == Some(day_num);

            let rx = (x - 24).min(W - 25 - x);
            let ry = (y - card_y).min(card_y + 73 - y);
            if rx < R_CHIP && ry < R_CHIP {
                let cdx = (R_CHIP - rx) as isize;
                let cdy = (R_CHIP - ry) as isize;
                if cdx * cdx + cdy * cdy >= (R_CHIP * R_CHIP) as isize {
                    return THEME.surface;
                }
            }

            // Left day badge pill (x: 36..136, y: card_y + 12..card_y + 62)
            if x >= 36 && x < 136 && y >= card_y + 12 && y < card_y + 62 {
                let bg = if is_sel { THEME.primary } else { THEME.surface_container };
                let text_c = if is_sel { THEME.on_primary } else { THEME.on_surface };
                let mut d_buf = [0u8; 8];
                let w_lbl = WEEK_LABELS[i];
                d_buf[..3].copy_from_slice(w_lbl);
                d_buf[3] = b' ';
                let dn = if day_num < 10 {
                    d_buf[4] = b'0' + day_num;
                    5
                } else {
                    d_buf[4] = b'0' + day_num / 10;
                    d_buf[5] = b'0' + day_num % 10;
                    6
                };
                let lx = centered_x(&d_buf[..dn], 2, 36, 100);
                if let Some(c) = test_text_bold(x, y, lx, card_y + 18, 2, &d_buf[..dn], 100, text_c, bg) {
                    return c;
                }
                return bg;
            }

            // Middle: Event summary
            let mut day_events = [EventRecord::EMPTY; MAX_EVENTS];
            let ev_count = app.store.events_on(display_year as u16, display_month, day_num, &mut day_events);
            if ev_count > 0 {
                let first = &day_events[0];
                if let Some(c) = test_text_bold(x, y, 150, card_y + 10, 2, first.title_bytes(), 460, THEME.on_surface, THEME.surface_container_high) {
                    return c;
                }
                let mut time_buf = [0u8; 5];
                let tn = fmt_time(&mut time_buf, first.hour, first.minute);
                if let Some(c) = test_text(x, y, 150, card_y + 42, 2, &time_buf[..tn], 200, THEME.primary, THEME.surface_container_high) {
                    return c;
                }
            } else {
                if let Some(c) = test_text(x, y, 150, card_y + 26, 2, b"No events", 300, THEME.on_surface_variant, THEME.surface_container_high) {
                    return c;
                }
            }

            // Right chevron '>'
            //
            // WHY it goes through chevron_pixel: the hand-rolled arm test here
            // measured dx to the right of a left-hand apex, which draws a '<'
            // on a row that opens forward into the event. The shared helper
            // puts the apex on the pointing side.
            if x >= 644 && x < 664 && y >= card_y + 26 && y < card_y + 48 {
                if chevron_pixel(x, y, 658, (card_y + 37) as isize, ChevronDir::Right) {
                    return THEME.on_surface_variant;
                }
            }

            if rx < 2 || ry < 2 {
                return if is_sel { THEME.primary } else { THEME.border };
            }
            return THEME.surface_container_high;
        }
    }

    // Action button "+ New Event": the one box every view paints and taps.
    if x >= ACTION_BTN_X0 && x < ACTION_BTN_X1 && y >= ACTION_BTN_Y0 && y < ACTION_BTN_Y1 {
        return action_button_pixel(x, y, ACTION_BTN_Y0, ACTION_BTN_Y1);
    }

    THEME.surface
}

// -----------------------------------------------------------------------------
// View 3: Day View
// -----------------------------------------------------------------------------

fn day_view_pixel(
    x: usize,
    y: usize,
    app: &CalendarApp,
    display_year: u32,
    display_month: u8,
    agenda: &AgendaView,
) -> u32 {
    // Day hero header (y: 176..256)
    if y >= 176 && y < 256 {
        // Prev button '<' (x: 24..72, y: 176..224)
        //
        // WHY the direction is named: the glyph used to be drawn mirrored, so
        // the arrow on the left pointed forward and the arrow on the right
        // pointed back. The apex now sits on the pointing side, and the
        // direction is the same value the tap handler steps the day with.
        if x >= DAY_PREV_ARROW_X0 && x < DAY_PREV_ARROW_X1 && y >= DAY_ARROW_BAND_Y0 && y < DAY_ARROW_BAND_Y1 {
            if chevron_pixel(x, y, DAY_PREV_ARROW_APEX_X, DAY_PREV_ARROW_APEX_Y, ChevronDir::Left) {
                return THEME.primary;
            }
            return THEME.surface_container_high;
        }

        // Next button '>' (x: 648..696, y: 176..224)
        if x >= DAY_NEXT_ARROW_X0 && x < DAY_NEXT_ARROW_X1 && y >= DAY_ARROW_BAND_Y0 && y < DAY_ARROW_BAND_Y1 {
            if chevron_pixel(x, y, DAY_NEXT_ARROW_APEX_X, DAY_NEXT_ARROW_APEX_Y, ChevronDir::Right) {
                return THEME.primary;
            }
            return THEME.surface_container_high;
        }

        // Date Title (Scale 2 Bold)
        let mut date_buf = [0u8; 16];
        let day = app.selected_day.unwrap_or(app.current_day.max(1));
        let dn = fmt_date(&mut date_buf, display_year as u16, display_month, day);
        let date_str = &date_buf[..dn];
        let tx = centered_x(date_str, 2, 80, 560);
        if let Some(c) = test_text_bold(x, y, tx, 184, 2, date_str, 560, THEME.on_surface, THEME.surface) {
            return c;
        }

        // Subtitle event count
        let mut count_buf = [0u8; 16];
        let cn = fmt_count(&mut count_buf, agenda.len);
        let count_str = &count_buf[..cn];
        let cx0 = centered_x(count_str, 2, 80, 560);
        if let Some(c) = test_text(x, y, cx0, 222, 2, count_str, 560, THEME.primary, THEME.surface) {
            return c;
        }

        return THEME.surface;
    }

    // Day Schedule / Timeline Cards: the band ends where the action button
    // begins, so no card can be drawn over it.
    if y >= DAY_CARDS_Y0 && y < ACTION_BTN_Y0 {
        if agenda.len == 0 {
            // Day empty card
            if x >= DAY_EMPTY_X0
                && x < DAY_EMPTY_X0 + DAY_EMPTY_W
                && y >= DAY_EMPTY_Y0
                && y < DAY_EMPTY_Y1
            {
                let rx = (x - DAY_EMPTY_X0).min(DAY_EMPTY_W - 1 - (x - DAY_EMPTY_X0));
                let ry = (y - DAY_EMPTY_Y0).min(DAY_EMPTY_H - 1 - (y - DAY_EMPTY_Y0));
                if rx < R_CARD && ry < R_CARD {
                    let cdx = (R_CARD - rx) as isize;
                    let cdy = (R_CARD - ry) as isize;
                    if cdx * cdx + cdy * cdy >= (R_CARD * R_CARD) as isize {
                        return THEME.surface;
                    }
                }
                let hx = centered_x(b"No Events on this Day", 2, DAY_EMPTY_X0, DAY_EMPTY_W);
                if let Some(c) = test_text_bold(
                    x,
                    y,
                    hx,
                    DAY_EMPTY_HEAD_Y,
                    2,
                    b"No Events on this Day",
                    DAY_EMPTY_W,
                    THEME.on_surface,
                    THEME.surface_container,
                ) {
                    return c;
                }
                let dx0 = centered_x(b"Your schedule is clear", 2, DAY_EMPTY_X0, DAY_EMPTY_W);
                if let Some(c) = test_text(
                    x,
                    y,
                    dx0,
                    DAY_EMPTY_BODY_Y,
                    2,
                    b"Your schedule is clear",
                    DAY_EMPTY_W,
                    THEME.on_surface_variant,
                    THEME.surface_container,
                ) {
                    return c;
                }
                if rx < 2 || ry < 2 {
                    return THEME.border;
                }
                return THEME.surface_container;
            }
        } else {
            // Day event timeline cards: only the cards the band above the
            // action button can hold.
            for (i, item) in agenda.items[..agenda.len.min(DAY_CARD_MAX)].iter().enumerate() {
                let card_y = DAY_CARDS_Y0 + i * DAY_CARD_PITCH;
                if y >= card_y && y < card_y + DAY_CARD_H && x >= 24 && x < W - 24 {
                    let rx = (x - 24).min(W - 25 - x);
                    let ry = (y - card_y).min(card_y + DAY_CARD_H - 1 - y);
                    if rx < R_SOFT && ry < R_SOFT {
                        let cdx = (R_SOFT - rx) as isize;
                        let cdy = (R_SOFT - ry) as isize;
                        if cdx * cdx + cdy * cdy >= (R_SOFT * R_SOFT) as isize {
                            return THEME.surface;
                        }
                    }

                    // Left accent bar
                    if x < 34 {
                        return THEME.primary;
                    }

                    // Time pill (x: 44..150, y: card_y + 16..card_y + 60)
                    if x >= 44 && x < 150 && y >= card_y + 16 && y < card_y + 60 {
                        let mut t_buf = [0u8; 5];
                        let tn = fmt_time(&mut t_buf, item.hour, item.minute);
                        let tx0 = centered_x(&t_buf[..tn], 2, 44, 106);
                        if let Some(c) = test_text_bold(x, y, tx0, card_y + 22, 2, &t_buf[..tn], 106, THEME.on_primary, THEME.primary) {
                            return c;
                        }
                        return THEME.primary;
                    }

                    // Event Title (Scale 2 Bold)
                    if let Some(c) = test_text_bold(x, y, 168, card_y + 18, 2, item.title_bytes(), 450, THEME.on_surface, THEME.surface_container_high) {
                        return c;
                    }

                    // Subtitle (Scale 2 Body)
                    //
                    // WHY the line is derived from the record: the card used
                    // to print an invented venue for every event. The stored
                    // record carries no location, so the card says so.
                    if let Some(c) = test_text(x, y, 168, card_y + 54, 2, event_location(item), 450, THEME.on_surface_variant, THEME.surface_container_high) {
                        return c;
                    }

                    // Chevron '>'
                    //
                    // WHY it goes through chevron_pixel: the arm test here
                    // measured dx to the right of a left-hand apex, which drew
                    // a '<' on a row that opens forward.
                    if x >= 644 && x < 664 && y >= card_y + 46 && y < card_y + 70 {
                        if chevron_pixel(x, y, 658, (card_y + 58) as isize, ChevronDir::Right) {
                            return THEME.on_surface_variant;
                        }
                    }

                    if rx < 2 || ry < 2 {
                        return THEME.border;
                    }
                    return THEME.surface_container_high;
                }
            }
        }
    }

    // Action button "+ New Event": the one box every view paints and taps.
    if x >= ACTION_BTN_X0 && x < ACTION_BTN_X1 && y >= ACTION_BTN_Y0 && y < ACTION_BTN_Y1 {
        return action_button_pixel(x, y, ACTION_BTN_Y0, ACTION_BTN_Y1);
    }

    THEME.surface
}

// -----------------------------------------------------------------------------
// View 4: Agenda View (Chronological Feed)
// -----------------------------------------------------------------------------

fn agenda_view_pixel(
    x: usize,
    y: usize,
    app: &CalendarApp,
) -> u32 {
    // Header (y: 176..220)
    if y >= 176 && y < 220 {
        if let Some(c) = test_text_bold(x, y, 28, 184, 2, b"AGENDA TIMELINE", 350, THEME.on_surface, THEME.surface) {
            return c;
        }
        let mut count_buf = [0u8; 16];
        let cn = fmt_count(&mut count_buf, app.store.len());
        let count_str = &count_buf[..cn];
        if let Some(c) = test_text(x, y, 540, 184, 2, count_str, 156, THEME.primary, THEME.surface) {
            return c;
        }
        return THEME.surface;
    }

    let mut all_items = [EventRecord::EMPTY; MAX_EVENTS];
    let total = app.store.all_events_sorted(&mut all_items);

    if total == 0 {
        // Empty state card (y: 220..520)
        if x >= 24 && x < W - 24 && y >= 220 && y < 520 {
            let rx = (x - 24).min(W - 25 - x);
            let ry = (y - 220).min(519 - y);
            if rx < R_CARD && ry < R_CARD {
                let cdx = (R_CARD - rx) as isize;
                let cdy = (R_CARD - ry) as isize;
                if cdx * cdx + cdy * cdy >= (R_CARD * R_CARD) as isize {
                    return THEME.surface;
                }
            }
            let hx = centered_x(b"No Scheduled Events", 2, 24, 672);
            if let Some(c) = test_text_bold(x, y, hx, 310, 2, b"No Scheduled Events", 672, THEME.on_surface, THEME.surface_container) {
                return c;
            }
            let dx0 = centered_x(b"Create your first event below", 2, 24, 672);
            if let Some(c) = test_text(x, y, dx0, 370, 2, b"Create your first event below", 672, THEME.on_surface_variant, THEME.surface_container) {
                return c;
            }
            if rx < 2 || ry < 2 {
                return THEME.border;
            }
            return THEME.surface_container;
        }
    } else {
        // Chronological cards: only the ones that end above the action button.
        //
        // WHY the rung is read here and handed down: the plate, the three run
        // origins, the stride and the count the band holds all come from the same
        // `*_at(lvl)` faces `handle_tap` answers below, so a rung that grew a card
        // moved the row it opens. The read is a relaxed load and a downward walk
        // over this crate's const arithmetic; the platform call that would not
        // belong in a pixel -- `settings_get` -- is answered once per wake.
        let lvl = calendar::draw_level();
        let card_h = calendar::agenda_card_h_at(lvl);
        let rows = calendar::agenda_rows_at(lvl);
        let dt_y = calendar::agenda_line_y_at(0, lvl);
        let title_y = calendar::agenda_line_y_at(1, lvl);
        let label_y = calendar::agenda_line_y_at(2, lvl);
        for (i, item) in all_items[..total.min(calendar::agenda_card_max_at(lvl))]
            .iter()
            .enumerate()
        {
            let card_y = calendar::AGENDA_CARDS_Y0 + i * calendar::agenda_card_pitch_at(lvl);
            if y >= card_y && y < card_y + card_h && x >= 24 && x < W - 24 {
                let rx = (x - 24).min(W - 25 - x);
                let ry = (y - card_y).min(card_y + card_h - 1 - y);
                if rx < R_SOFT && ry < R_SOFT {
                    let cdx = (R_SOFT - rx) as isize;
                    let cdy = (R_SOFT - ry) as isize;
                    if cdx * cdx + cdy * cdy >= (R_SOFT * R_SOFT) as isize {
                        return THEME.surface;
                    }
                }

                if x < 34 {
                    return THEME.primary;
                }

                // Date & Time line
                let mut dt_buf = [0u8; 24];
                let dn = fmt_date(&mut dt_buf, item.year, item.month, item.day);
                dt_buf[dn] = b' ';
                dt_buf[dn + 1] = b' ';
                let tn = fmt_time(&mut dt_buf[dn + 2..], item.hour, item.minute);
                let full_dt = &dt_buf[..dn + 2 + tn];
                if let Some(c) = agenda_line_text(x, y, lvl, 48, card_y + dt_y, calendar::AGENDA_LINE_RUNG, full_dt, 550, THEME.primary, THEME.surface_container_high, true) {
                    return c;
                }

                // Title line
                if let Some(c) = agenda_line_text(x, y, lvl, 48, card_y + title_y, calendar::AGENDA_LINE_RUNG, item.title_bytes(), 550, THEME.on_surface, THEME.surface_container_high, true) {
                    return c;
                }

                // Storage label
                if let Some(c) = agenda_line_text(x, y, lvl, 48, card_y + label_y, calendar::AGENDA_LINE_RUNG, b"Sovereign Settings Store", 550, THEME.on_surface_variant, THEME.surface_container_high, false) {
                    return c;
                }

                // Chevron '>'
                //
                // WHY it goes through chevron_pixel: the arm test here measured
                // dx to the right of a left-hand apex, which drew a '<' on a
                // row that opens forward.
                //
                // WHY its band follows the plate: the arrow sits on the title
                // line's box, so a card that grew at a raised rung moves the mark
                // that says the row opens, instead of leaving it over the line.
                if x >= 644 && x < 664 && y >= card_y + title_y && y < card_y + title_y + rows {
                    if chevron_pixel(x, y, 658, (card_y + title_y + rows / 2) as isize, ChevronDir::Right) {
                        return THEME.on_surface_variant;
                    }
                }

                if rx < 2 || ry < 2 {
                    return THEME.border;
                }
                return THEME.surface_container_high;
            }
        }
    }

    // Action button "+ New Event": the one box every view paints and taps.
    if x >= ACTION_BTN_X0 && x < ACTION_BTN_X1 && y >= ACTION_BTN_Y0 && y < ACTION_BTN_Y1 {
        return action_button_pixel(x, y, ACTION_BTN_Y0, ACTION_BTN_Y1);
    }

    THEME.surface
}

// -----------------------------------------------------------------------------
// Empty State & Event List Renderers
// -----------------------------------------------------------------------------

fn empty_state_pixel(x: usize, y: usize, app: &CalendarApp) -> u32 {
    if x >= 24 && x < 696 && y >= MONTH_EMPTY_CARD_Y0 && y < MONTH_EMPTY_CARD_Y1 {
        let mut on_card_border = false;
        let rx = (x - 24).min(695 - x);
        let ry = (y - MONTH_EMPTY_CARD_Y0).min(MONTH_EMPTY_CARD_Y1 - 1 - y);
        if rx < R_CARD && ry < R_CARD {
            let cdx = (R_CARD - rx) as isize;
            let cdy = (R_CARD - ry) as isize;
            let d2 = cdx * cdx + cdy * cdy;
            if d2 >= (R_CARD * R_CARD) as isize {
                return THEME.surface;
            }
            if d2 >= ((R_CARD - 2) * (R_CARD - 2)) as isize {
                on_card_border = true;
            }
        } else if rx < 2 || ry < 2 {
            on_card_border = true;
        }

        // Calendar Tear-Sheet Pictogram, on the card's own centre column and
        // hung from the card's top rather than the surface's row.
        let cx = 360;
        let cy = MONTH_EMPTY_CARD_Y0 + MONTH_EMPTY_MARK_DY;
        if (y >= cy - 44 && y < cy - 28) && ((x >= cx - 26 && x < cx - 18) || (x >= cx + 18 && x < cx + 26)) {
            return THEME.on_surface_variant;
        }
        if x >= cx - 44 && x < cx + 44 && y >= cy - 32 && y < cy + 44 {
            if y < cy - 12 {
                return THEME.error;
            }
            if x == cx - 44 || x == cx + 43 || y == cy + 43 {
                return THEME.border;
            }
            if (y >= cy + 4 && y < cy + 8 && x >= cx - 28 && x < cx + 28)
                || (y >= cy + 16 && y < cy + 20 && x >= cx - 28 && x < cx + 16)
                || (y >= cy + 28 && y < cy + 32 && x >= cx - 28 && x < cx + 6)
            {
                return THEME.border;
            }
            return THEME.surface_container_high;
        }

        let headline: &[u8] = if app.store.len() == 0 {
            b"No Scheduled Events"
        } else {
            b"No Events On This Day"
        };
        let headline_h = std::design::font::line_height(2);
        let headline_y = MONTH_EMPTY_CARD_Y0 + MONTH_EMPTY_HEADLINE_DY;
        if y >= headline_y && y < headline_y + headline_h {
            let hx = centered_x(headline, 2, 24, 672);
            if let Some(c) = test_text(
                x,
                y,
                hx,
                headline_y,
                2,
                headline,
                672,
                THEME.on_surface,
                THEME.surface_container,
            ) {
                return c;
            }
        }

        let detail_h = std::design::font::line_height(1);
        let detail_y = MONTH_EMPTY_CARD_Y0 + MONTH_EMPTY_DETAIL_DY;
        if y >= detail_y && y < detail_y + detail_h {
            let mut detail = [0u8; 40];
            let detail_str: &[u8] =
                if matches!(app.store.state, StoreState::ReadRefused | StoreState::Invalid) {
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
                dx,
                detail_y,
                1,
                detail_str,
                672,
                THEME.on_surface_variant,
                THEME.surface_container,
            ) {
                return c;
            }
        }

        // Action button: "+ New Event"
        if x >= ACTION_BTN_X0 && x < ACTION_BTN_X1 && y >= ACTION_BTN_Y0 && y < ACTION_BTN_Y1 {
            return action_button_pixel(x, y, ACTION_BTN_Y0, ACTION_BTN_Y1);
        }

        if on_card_border {
            return THEME.border;
        }
        return THEME.surface_container;
    }

    THEME.surface
}

fn event_list_pixel(x: usize, y: usize, items: &[EventRecord], len: usize) -> u32 {
    let card_idx = (y - MONTH_LIST_Y0) / MONTH_CARD_PITCH;
    let card_offset_y = (y - MONTH_LIST_Y0) % MONTH_CARD_PITCH;
    if card_idx < len.min(MONTH_CARD_MAX)
        && card_offset_y < MONTH_CARD_H
        && x >= 24
        && x < W - 24
    {
        let rx = (x - 24).min(W - 25 - x);
        let ry = card_offset_y.min(MONTH_CARD_H - 1 - card_offset_y);
        if rx < R_SOFT && ry < R_SOFT {
            let cdx = (R_SOFT - rx) as isize;
            let cdy = (R_SOFT - ry) as isize;
            if cdx * cdx + cdy * cdy >= (R_SOFT * R_SOFT) as isize {
                return THEME.surface;
            }
        }

        if x < 34 {
            return THEME.primary;
        }

        let card_y = MONTH_LIST_Y0 + card_idx * MONTH_CARD_PITCH;
        let item = &items[card_idx];
        let line_h2 = std::design::font::line_height(2);
        if y >= card_y + 14 && y < card_y + 14 + line_h2 {
            if let Some(c) = test_text_bold(x, y, 48, card_y + 14, 2, item.title_bytes(), 580 - 48, THEME.on_surface, THEME.surface_container) {
                return c;
            }
        }
        if y >= card_y + 48 && y < card_y + 48 + line_h2 {
            let mut time_buf = [0u8; 5];
            let time_n = fmt_time(&mut time_buf, item.hour, item.minute);
            if let Some(c) = test_text_bold(x, y, 48, card_y + 48, 2, &time_buf[..time_n], 580 - 48, THEME.primary, THEME.surface_container) {
                return c;
            }
        }
        if y >= card_y + 80 && y < card_y + 80 + line_h2 {
            let mut date_buf = [0u8; 12];
            let date_n = fmt_date(&mut date_buf, item.year, item.month, item.day);
            if let Some(c) = test_text(x, y, 48, card_y + 80, 2, &date_buf[..date_n], 580 - 48, THEME.on_surface_variant, THEME.surface_container) {
                return c;
            }
        }

        // Right chevron '>' (indicator to open event details)
        //
        // WHY it goes through chevron_pixel: the arm test here measured dx to
        // the right of a left-hand apex, which draws a '<' on a row that opens
        // forward into the event's details.
        if x >= 644 && x < 660 && y >= card_y + 44 && y < card_y + 64 {
            if chevron_pixel(x, y, 658, (card_y + 54) as isize, ChevronDir::Right) {
                return THEME.on_surface_variant;
            }
        }

        if rx < 2 || ry < 2 {
            return THEME.border;
        }
        return THEME.surface_container;
    }

    THEME.surface
}

fn action_button_pixel(x: usize, y: usize, y0: usize, y1: usize) -> u32 {
    let mut on_btn_border = false;
    let bx = (x - ACTION_BTN_X0).min(ACTION_BTN_X1 - 1 - x);
    let by = (y - y0).min(y1 - 1 - y);
    if bx < R_SHEET && by < R_SHEET {
        let cdx = (R_SHEET - bx) as isize;
        let cdy = (R_SHEET - by) as isize;
        let d2 = cdx * cdx + cdy * cdy;
        if d2 >= (R_SHEET * R_SHEET) as isize {
            return THEME.surface_container;
        }
        if d2 >= ((R_SHEET - 2) * (R_SHEET - 2)) as isize {
            on_btn_border = true;
        }
    } else if bx < 2 || by < 2 {
        on_btn_border = true;
    }

    let label_w = std::design::font::measure(ACTION_BTN_LABEL, ACTION_BTN_SCALE);
    let label_x = ACTION_BTN_X0 + (ACTION_BTN_W.saturating_sub(label_w)) / 2;
    let label_y = y0 + ((y1 - y0).saturating_sub(std::design::font::line_height(ACTION_BTN_SCALE))) / 2;
    if let Some(c) = test_text(
        x,
        y,
        label_x,
        label_y,
        ACTION_BTN_SCALE,
        ACTION_BTN_LABEL,
        ACTION_BTN_W,
        THEME.primary,
        THEME.surface_container,
    ) {
        return c;
    }

    if on_btn_border {
        return THEME.primary;
    }
    THEME.surface_container
}

// -----------------------------------------------------------------------------
// Event Details Modal Renderer
// -----------------------------------------------------------------------------

fn details_pixel(x: usize, y: usize, app: &CalendarApp, event: &EventRecord) -> u32 {
    if x < DETAILS_X0 || x >= DETAILS_X1 || y < DETAILS_Y0 || y >= DETAILS_Y1 {
        return THEME.surface;
    }
    let rx = (x - DETAILS_X0).min(DETAILS_X1 - 1 - x);
    let ry = (y - DETAILS_Y0).min(DETAILS_Y1 - 1 - y);
    if rx < R_CARD && ry < R_CARD {
        let cdx = (R_CARD - rx) as isize;
        let cdy = (R_CARD - ry) as isize;
        if cdx * cdx + cdy * cdy >= (R_CARD * R_CARD) as isize {
            return THEME.surface;
        }
    }
    if rx < 2 || ry < 2 {
        return THEME.border;
    }

    // Modal Header Title: "EVENT DETAILS" (Scale 2 Bold)
    if let Some(c) = test_text_bold(x, y, DETAILS_X0 + 24, DETAILS_TITLE_Y, 2, b"EVENT DETAILS", 300, THEME.on_surface, THEME.surface_container) {
        return c;
    }

    // Touch Clear [X] affordance at top-right
    if x >= DETAILS_CLOSE_X0 && x < DETAILS_CLOSE_X1 && y >= DETAILS_CLOSE_Y0 && y < DETAILS_CLOSE_Y1 {
        let cx = (DETAILS_CLOSE_X0 + (DETAILS_CLOSE_X1 - DETAILS_CLOSE_X0) / 2) as isize;
        let cy = (DETAILS_CLOSE_Y0 + (DETAILS_CLOSE_Y1 - DETAILS_CLOSE_Y0) / 2) as isize;
        let dx = (x as isize) - cx;
        let dy = (y as isize) - cy;
        let r2 = dx * dx + dy * dy;
        if r2 <= 20 * 20 {
            let adx = dx.abs();
            let ady = dy.abs();
            if (adx - ady).abs() <= 1 && adx <= 8 && ady <= 8 {
                return THEME.on_surface;
            }
            if r2 >= 18 * 18 {
                return THEME.border;
            }
            return THEME.surface_container_high;
        }
    }

    // Header divider line (y: 498..500)
    if y >= 498 && y < 500 && x >= DETAILS_X0 + 24 && x < DETAILS_X1 - 24 {
        return THEME.border;
    }

    // Section 1: Title
    if let Some(c) = test_text(x, y, DETAILS_X0 + 24, 512, 2, b"EVENT TITLE", 300, THEME.on_surface_variant, THEME.surface_container) {
        return c;
    }
    if let Some(c) = test_text_bold(x, y, DETAILS_X0 + 24, 550, 2, event.title_bytes(), 600, THEME.on_surface, THEME.surface_container) {
        return c;
    }

    // Section 2: Date & Time
    if let Some(c) = test_text(x, y, DETAILS_X0 + 24, 612, 2, b"SCHEDULED FOR", 300, THEME.on_surface_variant, THEME.surface_container) {
        return c;
    }
    let mut dt_buf = [0u8; 32];
    let dn = fmt_date(&mut dt_buf, event.year, event.month, event.day);
    let at_str = b" at ";
    dt_buf[dn..dn + at_str.len()].copy_from_slice(at_str);
    let tn = fmt_time(&mut dt_buf[dn + at_str.len()..], event.hour, event.minute);
    let full_dt = &dt_buf[..dn + at_str.len() + tn];
    if let Some(c) = test_text_bold(x, y, DETAILS_X0 + 24, 650, 2, full_dt, 600, THEME.primary, THEME.surface_container) {
        return c;
    }

    // Section 3: Security & Storage
    if let Some(c) = test_text(x, y, DETAILS_X0 + 24, 712, 2, b"SECURITY & STORAGE", 300, THEME.on_surface_variant, THEME.surface_container) {
        return c;
    }
    // WHY the line follows the flag: this row used to name the sealed sector
    // whatever the vault had answered. The app only knows whether its own
    // write of this sector was accepted, and that is all the row may say.
    let storage_note: &[u8] = if app.sector_encrypted {
        b"Vault accepted the sector 203 write"
    } else {
        b"No sector 203 write accepted by the vault"
    };
    if let Some(c) = test_text(x, y, DETAILS_X0 + 24, 750, 2, storage_note, 600, THEME.on_surface, THEME.surface_container) {
        return c;
    }

    // Action Buttons: DELETE EVENT (x: 44..344) and CLOSE (x: 376..676)
    if y >= DETAILS_BTN_Y0 && y < DETAILS_BTN_Y1 {
        // DELETE button
        if x >= 44 && x < 344 {
            let rx = (x - 44).min(343 - x);
            let ry = (y - DETAILS_BTN_Y0).min(DETAILS_BTN_Y1 - 1 - y);
            if rx < R_CONTROL && ry < R_CONTROL {
                let cdx = (R_CONTROL - rx) as isize;
                let cdy = (R_CONTROL - ry) as isize;
                if cdx * cdx + cdy * cdy >= (R_CONTROL * R_CONTROL) as isize {
                    return THEME.surface_container;
                }
            }
            let lx = centered_x(b"DELETE EVENT", 2, 44, 300);
            let ly = DETAILS_BTN_Y0 + (60 - std::design::font::line_height(2)) / 2;
            if let Some(c) = test_text_bold(x, y, lx, ly, 2, b"DELETE EVENT", 300, THEME.error, THEME.surface_container_high) {
                return c;
            }
            if rx < 2 || ry < 2 {
                return THEME.error;
            }
            return THEME.surface_container_high;
        }

        // CLOSE button
        if x >= 376 && x < 676 {
            let rx = (x - 376).min(675 - x);
            let ry = (y - DETAILS_BTN_Y0).min(DETAILS_BTN_Y1 - 1 - y);
            if rx < R_CONTROL && ry < R_CONTROL {
                let cdx = (R_CONTROL - rx) as isize;
                let cdy = (R_CONTROL - ry) as isize;
                if cdx * cdx + cdy * cdy >= (R_CONTROL * R_CONTROL) as isize {
                    return THEME.surface_container;
                }
            }
            let lx = centered_x(b"CLOSE", 2, 376, 300);
            let ly = DETAILS_BTN_Y0 + (60 - std::design::font::line_height(2)) / 2;
            if let Some(c) = test_text_bold(x, y, lx, ly, 2, b"CLOSE", 300, THEME.on_surface, THEME.surface_container_high) {
                return c;
            }
            if rx < 2 || ry < 2 {
                return THEME.border;
            }
            return THEME.surface_container_high;
        }
    }

    // Status message
    if y >= 900 && y < 932 {
        // WHY this line counts the store instead of repeating the create flow's
        // status: the panel opens on an event that is already stored, and the
        // create flow's string stays "NO EVENT CREATED YET" until that flow
        // runs — so the panel denied the existence of the event it was showing.
        let mut buf = [0u8; 32];
        buf[..8].copy_from_slice(b"STORED: ");
        let n = 8 + fmt_count(&mut buf[8..], app.store.len());
        let status = &buf[..n];
        let sx = centered_x(status, 2, DETAILS_X0 + 20, DETAILS_X1 - DETAILS_X0 - 40);
        if let Some(c) = test_text(x, y, sx, 900, 2, status, DETAILS_X1 - DETAILS_X0 - 40, THEME.on_surface_variant, THEME.surface_container) {
            return c;
        }
    }

    THEME.surface_container
}

// -----------------------------------------------------------------------------
// Event Creation Sheet Renderer
// -----------------------------------------------------------------------------

/// Blends the page behind a modal 60% toward the page surface, so the sheet
/// stays legible against a light or dark theme without a second palette.
fn modal_scrim(pixel: u32) -> u32 {
    let surface = THEME.surface;
    let channel = |shift: u32| {
        let p = (pixel >> shift) & 0xFF;
        let s = (surface >> shift) & 0xFF;
        (((p * 2 + s * 3) / 5) & 0xFF) << shift
    };
    0xFF00_0000 | channel(16) | channel(8) | channel(0)
}

fn flow_pixel(x: usize, y: usize, app: &CalendarApp) -> u32 {
    let sheet_y1 = match app.flow {
        Flow::Title => SHEET_Y1,
        _ => SHEET_COMPACT_Y1,
    };
    if x < SHEET_X0 || x >= SHEET_X1 || y < SHEET_Y0 || y >= sheet_y1 {
        return THEME.surface;
    }
    let rx = (x - SHEET_X0).min(SHEET_X1 - 1 - x);
    let ry = (y - SHEET_Y0).min(sheet_y1 - 1 - y);
    if rx < R_CARD && ry < R_CARD {
        let cdx = (R_CARD - rx) as isize;
        let cdy = (R_CARD - ry) as isize;
        if cdx * cdx + cdy * cdy >= (R_CARD * R_CARD) as isize {
            return THEME.surface;
        }
    }
    if rx < 2 || ry < 2 {
        return THEME.border;
    }

    let mut step_buf = [0u8; 56];
    let step_str: &[u8] = match app.flow {
        Flow::Title => b"STEP 1 OF 3 - EVENT TITLE",
        Flow::Date => {
            let base = b"STEP 2 OF 3 - DATE";
            step_buf[..base.len()].copy_from_slice(base);
            let mut n = base.len();
            if !app.date_known {
                let tail = b" (MANUAL)";
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

    // Sheet Header: "NEW EVENT" (Scale 2 Bold)
    if let Some(c) = test_text_bold(
        x,
        y,
        centered_x(b"NEW EVENT", 2, SHEET_X0 + 20, SHEET_X1 - SHEET_X0 - 40),
        SHEET_TITLE_Y,
        2,
        b"NEW EVENT",
        SHEET_X1 - SHEET_X0 - 40,
        THEME.on_surface,
        THEME.surface_container,
    ) {
        return c;
    }

    // Touch Close [X] affordance at top-right
    if x >= 624 && x < 676 && y >= 538 && y < 590 {
        let cx = 650isize;
        let cy = 564isize;
        let dx = (x as isize) - cx;
        let dy = (y as isize) - cy;
        let r2 = dx * dx + dy * dy;
        if r2 <= 20 * 20 {
            let adx = dx.abs();
            let ady = dy.abs();
            if (adx - ady).abs() <= 1 && adx <= 8 && ady <= 8 {
                return THEME.on_surface;
            }
            if r2 >= 18 * 18 {
                return THEME.border;
            }
            return THEME.surface_container_high;
        }
    }

    // Step string in Scale 2 Body
    if let Some(c) = test_text(
        x,
        y,
        centered_x(step_str, 2, SHEET_X0 + 20, SHEET_X1 - SHEET_X0 - 40),
        SHEET_STEP_Y,
        2,
        step_str,
        SHEET_X1 - SHEET_X0 - 40,
        THEME.on_surface_variant,
        THEME.surface_container,
    ) {
        return c;
    }

    match app.flow {
        Flow::Title => title_step_pixel(x, y, app),
        Flow::Date => date_step_pixel(x, y, app),
        Flow::Time => time_step_pixel(x, y, app),
        Flow::None => THEME.surface,
    }
}

fn title_step_pixel(x: usize, y: usize, app: &CalendarApp) -> u32 {
    if x >= TITLE_FIELD_X0 && x < TITLE_FIELD_X1 && y >= TITLE_FIELD_Y0 && y < TITLE_FIELD_Y1 {
        let rx = (x - TITLE_FIELD_X0).min(TITLE_FIELD_X1 - 1 - x);
        let ry = (y - TITLE_FIELD_Y0).min(TITLE_FIELD_Y1 - 1 - y);
        if rx < R_CHIP && ry < R_CHIP {
            let cdx = (R_CHIP - rx) as isize;
            let cdy = (R_CHIP - ry) as isize;
            if cdx * cdx + cdy * cdy >= (R_CHIP * R_CHIP) as isize {
                return THEME.surface_container;
            }
        }

        // Touch Clear [X] affordance button on the title field (x: 616..668, y: 648..708)
        if x >= CLEAR_BTN_X0 && x < CLEAR_BTN_X1 && y >= CLEAR_BTN_Y0 && y < CLEAR_BTN_Y1 {
            let cx = (CLEAR_BTN_X0 + (CLEAR_BTN_X1 - CLEAR_BTN_X0) / 2) as isize;
            let cy = (CLEAR_BTN_Y0 + (CLEAR_BTN_Y1 - CLEAR_BTN_Y0) / 2) as isize;
            let dx = (x as isize) - cx;
            let dy = (y as isize) - cy;
            let r2 = dx * dx + dy * dy;
            if r2 <= 18 * 18 {
                let adx = dx.abs();
                let ady = dy.abs();
                if (adx - ady).abs() <= 1 && adx <= 7 && ady <= 7 {
                    return THEME.on_surface;
                }
                if r2 >= 16 * 16 {
                    return THEME.border;
                }
                return THEME.surface_container;
            }
        }

        let text: &[u8] = if app.title_len == 0 {
            b"Enter Title..."
        } else {
            &app.title[..app.title_len]
        };
        let color = if app.title_len == 0 { THEME.on_surface_variant } else { THEME.on_surface };

        // Title text in Scale 2 Body
        if let Some(c) = test_text(
            x,
            y,
            TITLE_TEXT_X0,
            TITLE_TEXT_Y,
            2,
            text,
            TITLE_TEXT_MAX_W,
            color,
            THEME.surface_container_high,
        ) {
            return c;
        }

        // Caret block
        //
        // WHY it is clamped to the field: the mark used to sit at the full
        // measure of the run, but the run above it is ellipsized to this
        // field's budget -- so as soon as the title was long enough to shorten,
        // the caret landed 58 columns right of the last glyph it was meant to
        // follow, on top of the clear control and cut off by the field's edge.
        if app.title_len > 0 {
            let caret_x = TITLE_TEXT_X0
                + std::design::font::measure(text, 2).min(TITLE_CARET_TEXT_MAX)
                + 2;
            if x >= caret_x
                && x < caret_x + TITLE_CARET_W
                && y >= TITLE_TEXT_Y
                && y < TITLE_TEXT_Y + std::design::font::line_height(2)
            {
                return THEME.primary;
            }
        }

        if rx < 2 || ry < 2 {
            return THEME.border;
        }
        return THEME.surface_container_high;
    }

    for row in 0..4 {
        for col in 0..KB_LETTER_COLS {
            if keyboard_key_at(x, y) == Some((row, col)) {
                return key_pixel(x, y, row, col);
            }
        }
    }

    THEME.surface_container
}

fn key_pixel(x: usize, y: usize, row: usize, col: usize) -> u32 {
    let label = key_label(row, col);
    let Some((x0, y0, x1, y1)) = key_rect(row, col) else {
        return THEME.surface;
    };
    if label.is_empty() {
        return THEME.surface;
    }
    let rx = (x - x0).min(x1 - 1 - x);
    let ry = (y - y0).min(y1 - 1 - y);
    if rx < R_CHIP && ry < R_CHIP {
        let cdx = (R_CHIP - rx) as isize;
        let cdy = (R_CHIP - ry) as isize;
        if cdx * cdx + cdy * cdy >= (R_CHIP * R_CHIP) as isize {
            return THEME.surface;
        }
    }

    let accent = row == 3 && col == 2; // NEXT button
    let del_key = row == 3 && col == 1; // BKSP button
    let text_c = if accent {
        THEME.primary
    } else if del_key {
        THEME.error
    } else {
        THEME.on_surface
    };

    if let Some(c) = test_text(
        x,
        y,
        centered_x(label, 2, x0, x1 - x0),
        y0 + (KB_ROW_H - std::design::font::line_height(2)) / 2,
        2,
        label,
        x1 - x0,
        text_c,
        THEME.surface_container_high,
    ) {
        return c;
    }
    if rx < 2 || ry < 2 {
        return if accent { THEME.primary } else if del_key { THEME.error } else { THEME.border };
    }
    THEME.surface_container_high
}

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
    if rx < R_CHIP && ry < R_CHIP {
        let cdx = (R_CHIP - rx) as isize;
        let cdy = (R_CHIP - ry) as isize;
        if cdx * cdx + cdy * cdy >= (R_CHIP * R_CHIP) as isize {
            return Some(THEME.surface_container);
        }
    }
    // Label in Scale 2 Body
    if let Some(c) = test_text(
        x,
        y,
        centered_x(label, 2, x0, x1 - x0),
        STEP_LABEL_Y,
        2,
        label,
        x1 - x0,
        THEME.on_surface_variant,
        THEME.surface_container_high,
    ) {
        return Some(c);
    }
    // Value in Scale 2 Bold
    if let Some(c) = test_text_bold(
        x,
        y,
        centered_x(value, 2, x0, x1 - x0),
        STEP_VALUE_Y,
        2,
        value,
        x1 - x0,
        value_color,
        THEME.surface_container_high,
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
            if krx < R_CHIP && kry < R_CHIP {
                let cdx = (R_CHIP - krx) as isize;
                let cdy = (R_CHIP - kry) as isize;
                if cdx * cdx + cdy * cdy >= (R_CHIP * R_CHIP) as isize {
                    return Some(THEME.surface_container);
                }
            }
            if let Some(c) = test_text_bold(
                x,
                y,
                centered_x(glyph, 2, kx0, kx1 - kx0),
                ky0 + (ky1 - ky0 - std::design::font::line_height(2)) / 2,
                2,
                glyph,
                kx1 - kx0,
                THEME.primary,
                THEME.surface_container_high,
            ) {
                return Some(c);
            }
            if krx < 2 || kry < 2 {
                return Some(THEME.primary);
            }
            return Some(THEME.surface_container_high);
        }
    }
    if rx < 2 || ry < 2 {
        return Some(THEME.border);
    }
    Some(THEME.surface_container_high)
}

fn date_step_pixel(x: usize, y: usize, app: &CalendarApp) -> u32 {
    let mut year_buf = [0u8; 4];
    let (year_str, year_color) = stepper_value(app.edit_year.map(|y| y as u32), &mut year_buf, 0);
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

fn time_step_pixel(x: usize, y: usize, app: &CalendarApp) -> u32 {
    let mut hour_buf = [0u8; 2];
    let (hour_str, hour_color) = stepper_value(app.edit_hour.map(|h| h as u32), &mut hour_buf, 2);
    let mut minute_buf = [0u8; 2];
    let (minute_str, minute_color) = stepper_value(app.edit_minute.map(|m| m as u32), &mut minute_buf, 2);

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

fn flow_status_pixel(x: usize, y: usize, app: &CalendarApp) -> u32 {
    let status = app.commit_status.as_bytes();
    let ok = app.commit_status.starts_with("SAVED") || app.commit_status.starts_with("DAY CLAMPED");
    if let Some(c) = test_text(
        x,
        y,
        centered_x(status, 2, SHEET_X0 + 20, SHEET_X1 - SHEET_X0 - 40),
        FLOW_STATUS_Y,
        2,
        status,
        SHEET_X1 - SHEET_X0 - 40,
        if ok { THEME.success } else { THEME.on_surface_variant },
        THEME.surface_container,
    ) {
        return c;
    }
    THEME.surface_container
}

fn stepper_value<'a>(
    value: Option<u32>,
    out: &'a mut [u8],
    min_digits: usize,
) -> (&'a [u8], u32) {
    match value {
        Some(v) => {
            let n = fmt_dec(out, v, min_digits);
            (&out[..n], THEME.on_surface)
        }
        None => (b"--", THEME.on_surface_variant),
    }
}

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
        let mut on_btn_border = false;
        let rx = (x - bx0).min(bx1 - 1 - x);
        let ry = (y - FLOW_BTN_Y0).min(FLOW_BTN_Y1 - 1 - y);
        if rx < R_CONTROL && ry < R_CONTROL {
            let cdx = (R_CONTROL - rx) as isize;
            let cdy = (R_CONTROL - ry) as isize;
            let d2 = cdx * cdx + cdy * cdy;
            if d2 >= (R_CONTROL * R_CONTROL) as isize {
                return Some(THEME.surface_container);
            }
            if d2 >= ((R_CONTROL - 2) * (R_CONTROL - 2)) as isize {
                on_btn_border = true;
            }
        } else if rx < 2 || ry < 2 {
            on_btn_border = true;
        }
        if let Some(c) = test_text_bold(
            x,
            y,
            centered_x(label, 2, bx0, bx1 - bx0),
            FLOW_BTN_Y0 + (FLOW_BTN_Y1 - FLOW_BTN_Y0 - std::design::font::line_height(2)) / 2,
            2,
            label,
            bx1 - bx0,
            if accent { THEME.primary } else { THEME.on_surface_variant },
            THEME.surface_container_high,
        ) {
            return Some(c);
        }
        if on_btn_border {
            return Some(if accent { THEME.primary } else { THEME.border });
        }
        return Some(THEME.surface_container_high);
    }
    None
}

// -----------------------------------------------------------------------------
// Surface Presentation & Banded Repainting
// -----------------------------------------------------------------------------

fn render_calendar_ui(surface: u32, app: &CalendarApp) {
    render_calendar_band(surface, app, 0, H);
}

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
        let _ = std::surface_write(surface, offset, &chunk[..count]);
        offset += count;
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct Dirty(u8);

impl Dirty {
    const EMPTY: Dirty = Dirty(0);
    const HEADER: Dirty = Dirty(1 << 0);
    const TABS: Dirty = Dirty(1 << 1);
    const CONTENT: Dirty = Dirty(1 << 2);

    fn is_empty(self) -> bool {
        self.0 == 0
    }

    fn widen(self, other: Dirty) -> Dirty {
        Dirty(self.0 | other.0)
    }
}

const BANDS: [(Dirty, usize, usize); 3] = [
    (Dirty::HEADER, 0, HEADER_H),
    (Dirty::TABS, HEADER_H, 172),
    (Dirty::CONTENT, 172, 1360),
];

fn repaint(surface: u32, app: &CalendarApp, dirty: Dirty) {
    for (band, y0, y1) in BANDS {
        if dirty.0 & band.0 != 0 {
            render_calendar_band(surface, app, y0, y1);
        }
    }
    let _ = std::surface_flip();
}

enum Tap {
    None,
    Redraw(Dirty),
    Leave(&'static str),
}

fn exit_to_launcher(surface: u32) -> ! {
    if surface > 0 {
        let _ = std::surface_destroy(surface);
    }
    std::exit_with_code(0);
}

// -----------------------------------------------------------------------------
// Interactive Flow & Tap Handling
// -----------------------------------------------------------------------------

fn open_flow(app: &mut CalendarApp) {
    app.flow = Flow::Title;
    app.title_len = 0;
    app.title = [0; EVENT_TITLE_MAX];
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
        app.commit_status = "NOT SAVED: STORE FULL (12)";
        return;
    }
    if !rec.valid() {
        app.commit_status = "NOT SAVED: INVALID DATE OR TIME";
        return;
    }
    if app.store.add(rec) {
        app.total_events = app.store.len() as u32;
        app.commit_status = "SAVED: EVENT STORED";
        app.selected_day = Some(day);
        app.flow = Flow::None;
        app.title_len = 0;
        return;
    }
    app.commit_status = "SETTINGS REFUSED: NOT PERSISTED";
}

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

fn handle_flow_tap(app: &mut CalendarApp, x: usize, y: usize) -> Dirty {
    // Back button or touch Close [X] at top-right of sheet
    //
    // WHY the band's own test is gone: the header paints the platform chip
    // through `back_button::pixel_for_theme`, which answers 24..72, while this
    // press answered 24..100. The extra 28 columns were bare `THEME.surface` --
    // the title run does not start until x=116 -- so they were a tap area that
    // painted nothing. The band that closes the sheet is now the box the chip
    // draws, one rect, read from the SDK by both the painter and this test.
    if std::design::back_button::contains(x, y) || (x >= 624 && x < 676 && y >= 538 && y < 590) {
        app.flow = Flow::None;
        app.commit_status = "CANCELLED: NO EVENT WRITTEN";
        return Dirty::CONTENT;
    }

    match app.flow {
        Flow::Title => {
            // Touch Clear [X] on title field
            if x >= CLEAR_BTN_X0 && x < CLEAR_BTN_X1 && y >= CLEAR_BTN_Y0 && y < CLEAR_BTN_Y1 {
                app.title_len = 0;
                app.commit_status = "TITLE CLEARED";
                return Dirty::CONTENT;
            }
            if let Some((row, col)) = keyboard_key_at(x, y) {
                apply_title_key(app, row, col);
                return Dirty::CONTENT;
            }
        }
        Flow::Date => {
            if let Some((which, delta)) = stepper_button_at(x, y, &DATE_BOXES) {
                apply_step(app, which, delta);
                return Dirty::CONTENT;
            }
            if y >= FLOW_BTN_Y0 && y < FLOW_BTN_Y1 {
                if x >= SAVE_BTN_X0 && x < SAVE_BTN_X1 {
                    app.flow = Flow::Time;
                    return Dirty::CONTENT;
                }
                if x >= CANCEL_BTN_X0 && x < CANCEL_BTN_X1 {
                    app.flow = Flow::None;
                    app.commit_status = "CANCELLED: NO EVENT WRITTEN";
                    return Dirty::CONTENT;
                }
            }
        }
        Flow::Time => {
            if let Some((which, delta)) = stepper_button_at(x, y, &TIME_BOXES) {
                apply_step(app, which, delta);
                return Dirty::CONTENT;
            }
            if x >= SAVE_BTN_X0 && x < SAVE_BTN_X1 && y >= FLOW_BTN_Y0 && y < FLOW_BTN_Y1 {
                commit_event(app);
                return Dirty::CONTENT;
            }
            if x >= CANCEL_BTN_X0 && x < CANCEL_BTN_X1 && y >= FLOW_BTN_Y0 && y < FLOW_BTN_Y1 {
                app.flow = Flow::None;
                app.commit_status = "CANCELLED: NO EVENT WRITTEN";
                return Dirty::CONTENT;
            }
        }
        Flow::None => {}
    }

    Dirty::EMPTY
}

fn handle_tap(app: &mut CalendarApp, x: usize, y: usize) -> Tap {
    // 1. Home gesture -- the SDK's answering box, the same declaration the
    // month view inks above.
    //
    // WHY it comes from `home_band`: the press used to be a hand-written
    // `x >= 240 && x <= 480 && y >= 1400` -- 241 columns by 40 rows,
    // off-centre about column 360.5 and under the platform's 44-row touch
    // floor -- while the pill it answers was painted from independent
    // literals at `260..460, 1418..1424`, which is how the two drifted apart.
    // `home_band::HIT` is the pill's neighbourhood at the 48-row preferred
    // target, centred and reaching the panel's last row, and the app's own
    // content stops at 1360, so the box claims only rows this app paints
    // empty.
    if std::design::home_band::contains(x, y) {
        return Tap::Leave("[CALENDAR APP] Back button or home gesture pressed. Exiting to launcher.");
    }

    // WHY only the chip's own box: see `handle_flow_tap` -- the wider
    // 24..100 band answered rows the header never inks as a control, so the
    // press and the picture disagreed. `back_button::contains` is the box the
    // painter draws, and at 48x48 it is still this app's own 48px touch-target
    // floor, so shrinking to the picture costs no reachability.
    let is_back = std::design::back_button::contains(x, y);

    // Navigation tabs -- answered before either modal gets to the row.
    //
    // WHY the band comes first: both surfaces are drawn under it, never over it,
    // so its pills stay at full strength while they are on screen. Each of them
    // used to test its own geometry first and count a press on a bright tab as a
    // press outside itself: the details panel closed and the view stayed put,
    // and the create-event sheet swallowed the press outright. A press here now
    // means what it looks like -- leave whatever the band is sitting over, show
    // that view -- and a gutter between the pills answers nothing.
    if y >= TAB_BAND_Y0 && y < TAB_BAND_Y0 + TAB_BAND_H {
        if let Some(mode) = tab_mode_at(x) {
            let mut dirty = Dirty::TABS;
            if app.details_event.take().is_some() {
                dirty = dirty.widen(Dirty::CONTENT);
            }
            if app.flow != Flow::None {
                app.flow = Flow::None;
                app.commit_status = "CANCELLED: NO EVENT WRITTEN";
                dirty = dirty.widen(Dirty::CONTENT);
            }
            if app.view_mode != mode {
                app.view_mode = mode;
                dirty = dirty.widen(Dirty::CONTENT);
            }
            return Tap::Redraw(dirty);
        }
        return Tap::None;
    }

    // 2. If Event Details modal is open
    if let Some(event) = app.details_event {
        if is_back || (x >= DETAILS_CLOSE_X0 && x < DETAILS_CLOSE_X1 && y >= DETAILS_CLOSE_Y0 && y < DETAILS_CLOSE_Y1) {
            app.details_event = None;
            return Tap::Redraw(Dirty::CONTENT);
        }
        // Action buttons inside details: DELETE (44..344) and CLOSE (376..676)
        if y >= DETAILS_BTN_Y0 && y < DETAILS_BTN_Y1 {
            if x >= 44 && x < 344 {
                // Delete event
                if app.store.delete_event(&event) {
                    app.total_events = app.store.len() as u32;
                    app.commit_status = "EVENT DELETED FROM STORAGE";
                    std::log("[CALENDAR APP] Event deleted from settings store.").ok();
                } else {
                    app.commit_status = "SETTINGS REFUSED: NOT DELETED";
                }
                app.details_event = None;
                return Tap::Redraw(Dirty::CONTENT);
            }
            if x >= 376 && x < 676 {
                app.details_event = None;
                return Tap::Redraw(Dirty::CONTENT);
            }
        }
        // Outside modal tap closes details
        if x < DETAILS_X0 || x >= DETAILS_X1 || y < DETAILS_Y0 || y >= DETAILS_Y1 {
            app.details_event = None;
            return Tap::Redraw(Dirty::CONTENT);
        }
        return Tap::None;
    }

    // 3. If Event Creation flow is open
    if app.flow != Flow::None {
        if is_back {
            app.flow = Flow::None;
            app.commit_status = "CANCELLED: NO EVENT WRITTEN";
            return Tap::Redraw(Dirty::CONTENT);
        }
        return Tap::Redraw(handle_flow_tap(app, x, y));
    }

    // Back button exits when no modal is open
    if is_back {
        return Tap::Leave("[CALENDAR APP] Back button or home gesture pressed. Exiting to launcher.");
    }

    // 4. Active View Mode Tap Handling
    match app.view_mode {
        ViewMode::Month => {
            // Month grid day selection
            if app.date_known {
                if let Some(day) = grid_cell_at(x, y, day_of_week(app.current_year, app.current_month, 1), days_in_month(app.current_year, app.current_month)) {
                    if app.selected_day != Some(day) {
                        app.selected_day = Some(day);
                        return Tap::Redraw(Dirty::CONTENT);
                    }
                }
            }

            // The list the surface is showing for this selection. One call
            // decides both branches below, so a press can never answer a
            // control the painter did not draw.
            let agenda = app.agenda_view();

            // Empty state action button "+ New Event"
            //
            // WHY it is gated on an empty list: the month view reaches this
            // button only through `empty_state_pixel`, so once one event is
            // stored the control has no pixels on the surface -- and its band,
            // ACTION_BTN_Y0..ACTION_BTN_Y1, lands squarely on the third card,
            // which starts at `MONTH_LIST_Y0 + 2 * MONTH_CARD_PITCH`, across the
            // row that card draws its title on. A press on a visible event
            // opened the create flow instead of showing that event.
            if agenda.len == 0 && x >= ACTION_BTN_X0 && x < ACTION_BTN_X1 && y >= ACTION_BTN_Y0 && y < ACTION_BTN_Y1 {
                open_flow(app);
                std::log("[CALENDAR APP] Create-event flow opened: title entry.").ok();
                return Tap::Redraw(Dirty::CONTENT);
            }

            // Tapping an event card to open details
            if y >= MONTH_LIST_Y0 && y < MONTH_LIST_Y1 {
                let card_idx = (y - MONTH_LIST_Y0) / MONTH_CARD_PITCH;
                let card_offset_y = (y - MONTH_LIST_Y0) % MONTH_CARD_PITCH;
                if card_idx < agenda.len.min(MONTH_CARD_MAX)
                    && card_offset_y < MONTH_CARD_H
                    && x >= 24
                    && x < W - 24
                {
                    app.details_event = Some(agenda.items[card_idx]);
                    return Tap::Redraw(Dirty::CONTENT);
                }
            }
        }
        ViewMode::Week => {
            // Tapping week day card
            let display_year = app.current_year;
            let display_month = app.current_month;
            let selected_d = app.selected_day.unwrap_or(app.current_day.max(1));
            let dow = if app.date_known { day_of_week(display_year, display_month, selected_d) } else { 0 };
            let sunday = if selected_d > dow { selected_d - dow } else { 1 };
            let num_days = if app.date_known { days_in_month(display_year, display_month) } else { 31 };

            for i in 0..7 {
                let card_y = 220 + i * 84;
                if y >= card_y && y < card_y + 74 && x >= 24 && x < W - 24 {
                    let day_num = (sunday + i as u8).min(num_days);
                    app.selected_day = Some(day_num);
                    let mut day_events = [EventRecord::EMPTY; MAX_EVENTS];
                    let ev_count = app.store.events_on(display_year as u16, display_month, day_num, &mut day_events);
                    if ev_count > 0 {
                        app.details_event = Some(day_events[0]);
                    }
                    return Tap::Redraw(Dirty::CONTENT);
                }
            }

            // "+ New Event" button
            if x >= ACTION_BTN_X0 && x < ACTION_BTN_X1 && y >= ACTION_BTN_Y0 && y < ACTION_BTN_Y1 {
                open_flow(app);
                return Tap::Redraw(Dirty::CONTENT);
            }
        }
        ViewMode::Day => {
            // Prev day '<' (the left chevron points back to the previous day)
            if x >= DAY_PREV_ARROW_X0 && x < DAY_PREV_ARROW_X1 && y >= DAY_ARROW_BAND_Y0 && y < DAY_ARROW_BAND_Y1 {
                if let Some(d) = app.selected_day {
                    let max_d = days_in_month(app.current_year, app.current_month);
                    if let Some(day) = step_selected_day(d, DayArrow::Previous, max_d) {
                        app.selected_day = Some(day);
                        return Tap::Redraw(Dirty::CONTENT);
                    }
                }
            }
            // Next day '>' (the right chevron points forward to the next day)
            if x >= DAY_NEXT_ARROW_X0 && x < DAY_NEXT_ARROW_X1 && y >= DAY_ARROW_BAND_Y0 && y < DAY_ARROW_BAND_Y1 {
                if let Some(d) = app.selected_day {
                    let max_d = days_in_month(app.current_year, app.current_month);
                    if let Some(day) = step_selected_day(d, DayArrow::Next, max_d) {
                        app.selected_day = Some(day);
                        return Tap::Redraw(Dirty::CONTENT);
                    }
                }
            }

            // Event cards in Day view: the same capped chain the painter draws.
            let agenda = app.agenda_view();
            for (i, item) in agenda.items[..agenda.len.min(DAY_CARD_MAX)]
                .iter()
                .enumerate()
            {
                let card_y = DAY_CARDS_Y0 + i * DAY_CARD_PITCH;
                if y >= card_y && y < card_y + DAY_CARD_H && x >= 24 && x < W - 24 {
                    app.details_event = Some(*item);
                    return Tap::Redraw(Dirty::CONTENT);
                }
            }

            // "+ New Event" button
            if x >= ACTION_BTN_X0 && x < ACTION_BTN_X1 && y >= ACTION_BTN_Y0 && y < ACTION_BTN_Y1 {
                open_flow(app);
                return Tap::Redraw(Dirty::CONTENT);
            }
        }
        ViewMode::Agenda => {
            let mut all_items = [EventRecord::EMPTY; MAX_EVENTS];
            let total = app.store.all_events_sorted(&mut all_items);

            // WHY the chain is answered from the same faces the painter inked: the
            // rung sizes the plate and its stride, so a card that grew took its
            // press band with it. Answering the authored stride under a rung-aware
            // plate would open the event two rows above the one the finger landed
            // on, and a band that shrank would cost the row its reachability.
            let lvl = calendar::draw_level();
            let card_h = calendar::agenda_card_h_at(lvl);
            for (i, item) in all_items[..total.min(calendar::agenda_card_max_at(lvl))]
                .iter()
                .enumerate()
            {
                let card_y = calendar::AGENDA_CARDS_Y0 + i * calendar::agenda_card_pitch_at(lvl);
                if y >= card_y && y < card_y + card_h && x >= 24 && x < W - 24 {
                    app.details_event = Some(*item);
                    return Tap::Redraw(Dirty::CONTENT);
                }
            }

            // "+ New Event" button
            if x >= ACTION_BTN_X0 && x < ACTION_BTN_X1 && y >= ACTION_BTN_Y0 && y < ACTION_BTN_Y1 {
                open_flow(app);
                return Tap::Redraw(Dirty::CONTENT);
            }
        }
    }

    Tap::None
}

// -----------------------------------------------------------------------------
// Entry Point & Event Reactor Loop
// -----------------------------------------------------------------------------

#[no_mangle]
pub extern "C" fn _start() {
    std::log("[CALENDAR APP (Ring 3)] Calendar Agenda service online.").ok();

    let mut calendar = CalendarApp::new();

    // 1. Sync Hardware RTC Clock
    calendar.sync_rtc();

    // 2. Allocate DRM/KMS Client Surface (z=200)
    let surface = match std::surface_create(W as u32, H as u32, 200) {
        Ok(id) => {
            std::log("[CALENDAR APP] DRM/KMS Client Surface allocated (z=200).").ok();
            id
        }
        Err(_) => 0,
    };

    // 3. Verify Gregorian Calendar Math against observed RTC date
    if calendar.date_known {
        let days = days_in_month(calendar.current_year, calendar.current_month);
        let dow = day_of_week(calendar.current_year, calendar.current_month, 1);
        std::log(match days {
            31 => "[CALENDAR APP] Gregorian date math: the RTC-reported month has 31 days.",
            30 => "[CALENDAR APP] Gregorian date math: the RTC-reported month has 30 days.",
            _ => "[CALENDAR APP] Gregorian date math: the RTC-reported month has fewer than 30 days.",
        }).ok();
        std::log(match dow {
            0 => "[CALENDAR APP] Weekday for the first of the RTC-reported month: Sunday.",
            1 => "[CALENDAR APP] Weekday for the first of the RTC-reported month: Monday.",
            2 => "[CALENDAR APP] Weekday for the first of the RTC-reported month: Tuesday.",
            3 => "[CALENDAR APP] Weekday for the first of the RTC-reported month: Wednesday.",
            4 => "[CALENDAR APP] Weekday for the first of the RTC-reported month: Thursday.",
            5 => "[CALENDAR APP] Weekday for the first of the RTC-reported month: Friday.",
            _ => "[CALENDAR APP] Weekday for the first of the RTC-reported month: Saturday.",
        }).ok();
    } else {
        std::log("[CALENDAR APP] No civil date available from the hardware RTC; month grid shows no selected day.").ok();
    }

    // 4. Load the stored events and select the day the agenda will list
    calendar.store = EventStore::load();
    calendar.total_events = calendar.store.len() as u32;
    std::log("[CALENDAR APP] Event store:").ok();
    std::log(calendar.store.state.as_str()).ok();
    if calendar.date_known {
        calendar.selected_day = Some(calendar.current_day as u8);
    }

    if surface > 0 {
        // WHY the rung is published before the first frame is laid out: Settings'
        // "Text size" is already chosen when this app opens, so the opening screen
        // must not be drawn at the ladder and resized a frame later.
        publish_text_scale();
        render_calendar_ui(surface, &calendar);
        let _ = std::surface_flip();
        std::log("[CALENDAR APP] Calendar canvas presented to scanout buffer.").ok();
    }

    // 5. Sovereign Asynchronous Event Reactor Loop
    let interest_mask = std::EVENT_TOUCH_INPUT | std::EVENT_IPC_MSG | std::EVENT_TIMER_TICK;
    loop {
        // WHY re-read at every wake: the size is changed in Settings while this
        // screen is alive, so a rung that moved has to repaint the whole surface --
        // the Agenda cards' plates, their stride, the count the band holds and the
        // rows they answer all move with it, and a band repaint would leave part of
        // the chain at the rung the person has already left. There is no scroll
        // extent to re-solve here: every view recomputes its list per frame.
        if publish_text_scale() && surface > 0 {
            render_calendar_ui(surface, &calendar);
            let _ = std::surface_flip();
        }

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
                        // WHY the pan and fling answer the SDK's home box:
                        // this used to exit for a navigation gesture from
                        // anywhere on the screen, so a scroll inside the month
                        // list threw the user out of the calendar. It now
                        // answers `home_band::contains` -- the same 240x48 box
                        // the tap above answers and the pill painter inks.
                        if (input.kind == 4
                            || input.kind == 6
                            || input.kind == 7
                            || input.kind == 8
                            || input.kind == 10)
                            && std::design::home_band::contains(
                                input.x as usize,
                                input.y as usize,
                            )
                        {
                            std::log("[CALENDAR APP] Navigation gesture recognized. Exiting to launcher.").ok();
                            exit_to_launcher(surface);
                        }
                        if input.kind == 1 {
                            match handle_tap(&mut calendar, input.x as usize, input.y as usize) {
                                Tap::Leave(msg) => {
                                    std::log(msg).ok();
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
                    std::log("[CALENDAR APP] IPC message event raised; no schedule intent payload was read.").ok();
                }
                if events & std::EVENT_TIMER_TICK != 0 {
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
    std::log("[CALENDAR APP] Panic detected! Terminating task...").ok();
    std::exit();
}
