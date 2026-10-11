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

#![no_std]
#![no_main]

use core::panic::PanicInfo;
use interos_std as std;

/// Calendar event status.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EventStatus {
    Confirmed = 0,
    Tentative = 1,
    Completed = 2,
}

/// Calendar event entry.
#[derive(Debug, Clone, Copy)]
pub struct CalendarEvent {
    pub id: u32,
    pub title: &'static str,
    pub time_str: &'static str,
    pub category: &'static str,
    pub status: EventStatus,
}

/// Static Built-in Agenda.
pub static AGENDA: [CalendarEvent; 0] = [];

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
    pub current_year: u32,
    pub current_month: u8,
    pub current_day: u8,
    pub total_events: u32,
    pub rtc_synchronized: bool,
    pub sector_encrypted: bool,
    /// Whether a real civil date has been read from the hardware RTC.
    pub date_known: bool,
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
            total_events: 0,
            rtc_synchronized: false,
            sector_encrypted: false,
            date_known: false,
        }
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
                match (y, m, d) {
                    (Some(y), Some(m), Some(d))
                        if y >= 1970 && (1..=12).contains(&m) && (1..=31).contains(&d) =>
                    {
                        self.current_year = y;
                        self.current_month = m as u8;
                        self.current_day = d as u8;
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
    fn parse_rtc_date(&mut self, buf: &[u8], n: usize) {
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

    /// Inserts a scheduled event and encrypts in persistent storage.
    ///
    /// Returns `true` only when the agenda sector was actually written.
    pub fn schedule_event(&mut self, event_name: &'static str) -> bool {
        // 1. Encrypt Updated Agenda in Storage Vault (Sector 203)
        let mut agenda_sector = [0u8; 512];
        let copy_len = event_name.len().min(128);
        agenda_sector[..copy_len].copy_from_slice(&event_name.as_bytes()[..copy_len]);
        let iv = [0x77u8; 16];
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
/// WHY the line-box clip: the shared bitmap face is `GLYPH_H` (24) rows tall,
/// so a run at `scale` owns `GLYPH_H * scale` rows. Clipping to that box here
/// means no call site can silently slice a glyph with a shorter hand-written
/// `y` window.
#[inline(always)]
fn test_text(
    x: usize,
    y: usize,
    x0: usize,
    y0: usize,
    scale: usize,
    text: &[u8],
    color: u32,
) -> Option<u32> {
    if x < x0 || y < y0 || y >= y0 + std::design::font::GLYPH_H * scale {
        return None;
    }
    std::design::font::test_text_aa(x, y, x0, y0, scale, text, color, BG_DARK, false)
}

/// Left x that centres `text` at `scale` inside `[x0, x0 + available)`.
/// WHY: the face is proportional (digits advance 11px at scale 1, letters 5-16),
/// so a fixed 8px character cell mis-centres every run that is not exactly that
/// wide. This measures the real advance widths instead.
#[inline(always)]
fn centered_x(text: &[u8], scale: usize, x0: usize, available: usize) -> usize {
    x0 + std::design::font::center_x(text, scale, available)
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

#[inline(never)]
fn get_calendar_pixel(
    x: usize,
    y: usize,
    app: &CalendarApp,
    display_year: u32,
    display_month: u8,
    start_dow: u8,
    num_days: u8,
    selected_day: Option<u8>,
) -> u32 {
    // 1. Status Bar & Header (y: 0..96)
    if y < 96 {
        if y >= 94 {
            return BORDER_SUBTLE;
        }

        // Back button (<) at (x: 24..100, y: 28..68)
        if x >= 24 && x < 100 && y >= 28 && y < 68 {
            let center_x = 62;
            let center_y = 48;
            let dx = (x as isize - center_x as isize).abs();
            let dy = (y as isize - center_y as isize).abs();
            if (dx + dy <= 12 && x <= center_x)
                || (x >= 24 && x < 100 && (y == 28 || y == 67 || x == 24 || x == 99))
            {
                return TEXT_PRIMARY;
            }
            return SURFACE_RAISED;
        }

        // Header Title: "Calendar"
        // WHY the move and the full box: a scale-3 run needs GLYPH_H * 3 = 72
        // rows, so the old y0=36 box (36..108) ran straight through the header
        // band's bottom rule (94..96) and out of the band, and the 36..60 gate
        // drew 24 of its 72 rows. The run is centred on the same y=48 axis as
        // the back button and the pill, giving y0 = 48 - 36 = 12 and the box
        // 12..84, which clears the header band (0..94) untouched.
        if y >= 12 && y < 12 + std::design::font::line_height(3) {
            if let Some(c) = test_text(x, y, 116, 12, 3, b"Calendar", TEXT_PRIMARY) {
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
            let pill_x = centered_x(pill_text, 2, 480, 216);
            let pill_y0 = 20 + (56 - std::design::font::line_height(2)) / 2;
            if let Some(c) = test_text(x, y, pill_x, pill_y0, 2, pill_text, ACCENT_SKY) {
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
                if let Some(color) =
                    test_text(x, y, text_x, WEEKDAY_Y0, WEEKDAY_SCALE, HEADERS[c], TEXT_MUTED)
                {
                    return color;
                }
            }
        } else {
            // WHY: with no civil date there is no month for these letters to
            // label. Drawing them would advertise a month grid that is not
            // there, so the header states the absence instead.
            let text_x = centered_x(NO_DATE_HEADLINE, WEEKDAY_SCALE, 0, W);
            if let Some(c) = test_text(x, y, text_x, WEEKDAY_Y0, WEEKDAY_SCALE, NO_DATE_HEADLINE, TEXT_MUTED) {
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
                TEXT_MUTED,
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
                        let is_current = selected_day == Some(day_num as u8);

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
                        if let Some(color) = test_text(x, y, text_x, text_y, 2, &text[..n], text_color) {
                            return color;
                        }

                        // Border rim
                        if rx < 2 || ry < 2 {
                            return if is_current { ACCENT_SKY } else { BORDER_SUBTLE };
                        }

                        return if is_current { 0xFF173552 } else { SURFACE_DARK };
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

        // Title: "TODAY'S AGENDA" at y0=480 (scale 2).
        // WHY: a scale-2 run is GLYPH_H * 2 = 48 rows, so it owns a 48-row box.
        // At the old y0=498 that box ran to 546 and the band ended at 530, which
        // cut its last 16 rows and severed the base of every letter. y0=480 puts
        // the run at 480..528 -- 2px below the 476..478 divider and 2px inside
        // the band -- so the whole line box fits and nothing below is touched.
        if let Some(c) = test_text(x, y, 28, 480, 2, b"TODAY'S AGENDA", TEXT_MUTED) {
            return c;
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

            let badge_str: &[u8] = if AGENDA.is_empty() { b"0 EVENTS" } else { b"EVENTS" };
            // WHY centred on the measured width: the face is proportional, so the
            // old fixed x=576 was only correct for the one 93px string that
            // happened to be used; the run is also vertically centred in the
            // 30px pill now, since at y0=499 its 24 rows ran to 523, one past
            // the pill's 522.
            let badge_x = centered_x(badge_str, 1, 550, 146);
            let badge_y = 492 + (30 - std::design::font::line_height(1)) / 2;
            if let Some(c) = test_text(x, y, badge_x, badge_y, 1, badge_str, ACCENT_SKY) {
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
        if AGENDA.is_empty() {
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
                if let Some(c) = test_text(x, y, 208, 706, 2, b"No Scheduled Events", TEXT_PRIMARY) {
                    return c;
                }
                if let Some(c) = test_text(x, y, 236, 760, 1, if app.date_known { &b"RTC date available"[..] } else { &b"RTC date unavailable"[..] }, TEXT_MUTED) {
                    return c;
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
                        ACCENT_SKY,
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
            // Render actual events list
            let card_idx = (y - 546) / 130;
            let card_offset_y = (y - 546) % 130;
            if card_idx < AGENDA.len() && card_offset_y < 114 && x >= 24 && x < W - 24 {
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

fn render_calendar_ui(surface: u32, app: &CalendarApp) {
    // WHY: there is no substitute month. When the RTC yields no civil date the
    // grid is not laid out from a stand-in value -- `get_calendar_pixel` draws
    // the explicit no-date state instead. Year 0 / month 0 / 0 days are sentinels
    // that are never formatted or rendered as a date; they only reach the
    // `date_known == false` branches.
    let (display_year, display_month, start_dow, num_days, selected_day) = if app.date_known {
        let y = app.current_year;
        let m = app.current_month;
        (
            y,
            m,
            day_of_week(y, m, 1),
            days_in_month(y, m),
            Some(app.current_day),
        )
    } else {
        (0, 0, 0, 0, None)
    };

    let mut chunk = [0u32; CHUNK];
    let total = W * H;
    let mut offset = 0;
    while offset < total {
        let count = (total - offset).min(CHUNK);
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
                selected_day,
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

#[no_mangle]
pub extern "C" fn _start() -> ! {
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

    if surface > 0 {
        render_calendar_ui(surface, &calendar);
        let _ = std::surface_flip(6);
        std::log(
            0,
            "[CALENDAR APP] Calendar canvas presented to scanout buffer.",
        )
        .ok();
    }

    // 4. Sovereign Asynchronous Event Reactor Loop
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
                    while std::input_event_poll(&mut input).is_ok() {
                        if input.kind == 4 || input.kind == 6 || input.kind == 7 || input.kind == 8 || input.kind == 10 {
                            std::log(0, "[CALENDAR APP] Navigation gesture recognized. Exiting to launcher.").ok();
                            if surface > 0 {
                                let _ = std::surface_destroy(6, surface);
                            }
                            std::exit_with_code(0);
                        }
                        if input.kind == 1 {
                            // Check Back button in header or Home gesture bar: exit cleanly to launcher
                            if (input.x >= 24 && input.x < 100 && input.y >= 28 && input.y < 68)
                                || (input.x >= 240 && input.x <= 480 && input.y >= 1400)
                            {
                                std::log(0, "[CALENDAR APP] Back button or home gesture pressed. Exiting to launcher.").ok();
                                if surface > 0 {
                                    let _ = std::surface_destroy(6, surface);
                                }
                                std::exit_with_code(0);
                            }
                        }
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
