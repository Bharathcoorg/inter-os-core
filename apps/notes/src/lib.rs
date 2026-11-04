// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

//! InterOS Sovereign Native Notes Library
//!
//! Provides core data structures, storage serialization, and text editing
//! buffer engine for the native Ring-3 Notes application.
//!
//! Enforces:
//! - `#![no_std]` compliance with zero heap allocations
//! - Fletcher-32 binary serialization with checksum integrity
//! - Deterministic word-wrapping and multiline layout calculation
//! - Sovereign storage lifecycle and platform-truth guarantees

#![no_std]

pub use interos_std as std;

/// The app's view of the platform's design system.
pub mod theme;

/// The soft keyboard's keys.
pub mod keyboard;

/// The notes' documents in the shared storage area.
pub mod library;

/// The header row's back control, and the label beside it.
pub mod chrome;

use crate::theme::{THEME, THEME_DARK};

pub const MAX_NOTES: usize = 16;
pub const MAX_TITLE_LEN: usize = 48;
pub const MAX_BODY_LEN: usize = 1024;

// =============================================================================
// The containers the renderer paints into
//
// WHY these live in the library and not beside the per-pixel branch that draws
// them: this app's binary is built with `test = false`, so a fit gate can only
// be a `#[test]` in this library -- and a `#[test]` can only ask a container it
// can reach. The renderer used to hold each of these widths as a literal inside
// its own branch, which meant the gate was measuring a number the painter had
// invented somewhere the test could not see. That is the defect class where a
// real clip passes the suite: raise a rung, the run grows, the remembered lane
// stays, and nothing answers.
//
// Every lane below is derived forward from the box the painter actually draws
// into, and every gap of air is its own named constant, so moving the box moves
// the lane and the gate with it. The rungs are not restated here: the painter
// asks `theme::type_scale`, and the gates measure at the same rung the painter
// uses.
// =============================================================================

/// The note card's left edge in the list.
pub const LIST_CARD_X0: usize = 28;

/// The note card's width.
pub const LIST_CARD_W: usize = 664;

/// The note card's right edge.
pub const LIST_CARD_X1: usize = LIST_CARD_X0 + LIST_CARD_W;

/// The card's hairline: the painter draws the frame, then cuts one step inside
/// it to get the fill. Same treatment for the list cards and the empty state.
pub const CARD_HAIRLINE: usize = 2;

/// The list card's fill: the surface a run is actually painted on.
pub const LIST_CARD_INNER_X0: usize = LIST_CARD_X0 + CARD_HAIRLINE;
pub const LIST_CARD_INNER_X1: usize = LIST_CARD_X1 - CARD_HAIRLINE;

/// The tone rail: the first mark inside the card.
pub const LIST_CARD_RAIL_X: usize = LIST_CARD_X0 + CARD_HAIRLINE + 2;
pub const LIST_CARD_RAIL_W: usize = 4;

/// A card's runs start one grid step clear of the rail.
pub const LIST_CARD_TEXT_X: usize =
    LIST_CARD_RAIL_X + LIST_CARD_RAIL_W + crate::theme::spacing::GRID_20;

/// The pin glyph's centre column, and the half-width of its widest arm (the
/// rim, which `draw_pin_glyph` paints at `dx.abs() <= 7`).
pub const PIN_GLYPH_CX: usize = 656;
pub const PIN_GLYPH_HALF_W: usize = 7;

/// The columns the pin glyph can answer for.
pub const PIN_GLYPH_X0: usize = PIN_GLYPH_CX - PIN_GLYPH_HALF_W;
pub const PIN_GLYPH_X1: usize = PIN_GLYPH_CX + PIN_GLYPH_HALF_W + 1;

/// Air between an unpinned title's run and the card's fill edge.
pub const TITLE_RIGHT_AIR: usize = crate::theme::spacing::GRID_24;

/// The lane the painter ellipsizes an unpinned title to.
pub const LIST_CARD_TITLE_LANE: usize =
    LIST_CARD_INNER_X1 - TITLE_RIGHT_AIR - LIST_CARD_TEXT_X;

/// Air between a pinned title's run and the pin glyph's left arm.
///
/// WHY it is not `TITLE_RIGHT_AIR`: the pin is ink, not a card edge. A title
/// that reaches it reads as one smear, so this row reserves its gap against the
/// glyph that shares the row.
pub const TITLE_PIN_CLEARANCE: usize = 13;

/// The lane the painter ellipsizes a pinned title to.
pub const LIST_CARD_TITLE_LANE_PINNED: usize =
    PIN_GLYPH_X0 - TITLE_PIN_CLEARANCE - LIST_CARD_TEXT_X;

/// The checklist box `draw_checkbox_glyph` paints, and the gap before the text
/// that belongs to it.
pub const CHECKBOX_SIZE: usize = 22;
pub const CHECKBOX_X: usize = LIST_CARD_TEXT_X;
pub const CHECKBOX_TEXT_GAP: usize = 10;
pub const CHECKBOX_TEXT_X: usize = CHECKBOX_X + CHECKBOX_SIZE + CHECKBOX_TEXT_GAP;

/// Air between the end of a snippet run and the card's fill edge. Both snippet
/// variants -- the one that starts on the checkbox and the one that starts on
/// the card's text column -- end on this same wall, which is why the checked
/// lane is not the plain lane minus the box.
pub const SNIPPET_RIGHT_AIR: usize = 14;
pub const SNIPPET_RUN_X1: usize = LIST_CARD_INNER_X1 - SNIPPET_RIGHT_AIR;

/// The lane the painter ellipsizes a plain snippet line to.
pub const LIST_CARD_SNIPPET_LANE: usize = SNIPPET_RUN_X1 - LIST_CARD_TEXT_X;

/// The lane the painter ellipsizes a checklist snippet line to.
pub const LIST_CARD_CHECK_SNIPPET_LANE: usize = SNIPPET_RUN_X1 - CHECKBOX_TEXT_X;

/// The empty-state card: the box the renderer draws when the list has nothing
/// to show, and the lane its subtitle is ellipsized to.
pub const EMPTY_CARD_X: usize = 60;
pub const EMPTY_CARD_Y: usize = 440;
pub const EMPTY_CARD_W: usize = 600;
pub const EMPTY_CARD_H: usize = 240;
pub const EMPTY_CARD_X1: usize = EMPTY_CARD_X + EMPTY_CARD_W;
pub const EMPTY_CARD_INNER_X0: usize = EMPTY_CARD_X + CARD_HAIRLINE;
pub const EMPTY_CARD_INNER_X1: usize = EMPTY_CARD_X1 - CARD_HAIRLINE;

/// Air the empty-state subtitle keeps on either side of the card.
pub const EMPTY_SUBTITLE_INSET: usize = 20;
pub const EMPTY_SUBTITLE_LANE: usize = EMPTY_CARD_W - 2 * EMPTY_SUBTITLE_INSET;

/// The ruled page in the editor: the column a body line wraps to and the column
/// it is drawn in are the distance between the rule's two edges.
pub const EDITOR_RULE_INSET: usize = 44;
pub const EDITOR_RULE_X0: usize = EDITOR_RULE_INSET;
pub const EDITOR_RULE_X1: usize =
    interos_std::design::viewport::LOGICAL_WIDTH - EDITOR_RULE_INSET;
pub const EDITOR_TEXT_COL_W: usize = EDITOR_RULE_X1 - EDITOR_RULE_X0;

/// The editor's circular tools: the radius the painter fills, and the centre
/// columns it draws them at. `EDITOR_FIRST_TOOL_CX` is the one a title can run
/// into, so the plate's left edge is the container the title answers to.
pub const EDITOR_TOOL_R: usize = 20;
pub const EDITOR_TOOL_CX: [usize; 3] = [520, 580, 640];
pub const EDITOR_FIRST_TOOL_CX: usize = 520;
pub const EDITOR_FIRST_TOOL_X0: usize = EDITOR_FIRST_TOOL_CX - EDITOR_TOOL_R;

/// Air between the editor title's run and the first tool's plate.
pub const EDITOR_TITLE_TOOL_CLEARANCE: usize = crate::theme::spacing::GRID_8;

/// The lane the painter ellipsizes the editor title to.
pub const EDITOR_TITLE_LANE: usize =
    EDITOR_FIRST_TOOL_X0 - EDITOR_TITLE_TOOL_CLEARANCE - EDITOR_RULE_X0;

// The containers have to describe a card that can actually hold its runs. A
// lane that reaches past the fill it is drawn on is the bug these refuse: the
// painter would ellipsize to a width the card does not own, and the tail of the
// run would answer nothing.
const _: () = assert!(LIST_CARD_TEXT_X > LIST_CARD_INNER_X0);
const _: () = assert!(LIST_CARD_TEXT_X + LIST_CARD_TITLE_LANE <= LIST_CARD_INNER_X1);
const _: () = assert!(LIST_CARD_TEXT_X + LIST_CARD_SNIPPET_LANE <= LIST_CARD_INNER_X1);
const _: () = assert!(CHECKBOX_TEXT_X <= SNIPPET_RUN_X1);
const _: () = assert!(CHECKBOX_TEXT_X + LIST_CARD_CHECK_SNIPPET_LANE == SNIPPET_RUN_X1);
const _: () = assert!(PIN_GLYPH_X1 <= LIST_CARD_INNER_X1);
const _: () = assert!(LIST_CARD_TEXT_X + LIST_CARD_TITLE_LANE_PINNED <= PIN_GLYPH_X0);
const _: () = assert!(LIST_CARD_TITLE_LANE_PINNED < LIST_CARD_TITLE_LANE);
const _: () = assert!(LIST_CARD_CHECK_SNIPPET_LANE < LIST_CARD_SNIPPET_LANE);
const _: () = assert!(EMPTY_CARD_X1 < interos_std::design::viewport::LOGICAL_WIDTH);
const _: () = assert!(EMPTY_SUBTITLE_LANE <= EMPTY_CARD_INNER_X1 - EMPTY_CARD_INNER_X0);
const _: () = assert!(EDITOR_RULE_X0 + EDITOR_TEXT_COL_W == EDITOR_RULE_X1);
const _: () = assert!(EDITOR_RULE_X1 <= interos_std::design::viewport::LOGICAL_WIDTH);
const _: () = assert!(EDITOR_RULE_X0 + EDITOR_TITLE_LANE <= EDITOR_FIRST_TOOL_X0);

/// On-disk header. The version byte is `3`.
///
/// WHY version 3: versions 1 and 2 stored the two timestamp slots as Unix
/// epoch seconds, and the value the app wrote into them was a constant that
/// was never read from a clock. A blob written by those versions therefore
/// carries a time the device never observed, and a reader that treated the
/// slot as a packed civil time would print nonsense such as "17293:30:0000".
/// Version 3 stores the civil fields the real-time clock reported. Blobs from
/// version 1 and 2 are still accepted -- a note is a user's content and must
/// not be lost to a format change -- but their timestamp slots are read as
/// "no timestamp observed" rather than as a time.
pub const MAGIC_HEADER: [u8; 6] = *b"NOTE\x03\x00";

/// The oldest on-disk version this reader accepts.
pub const LEGACY_MAGIC_HEADER: [u8; 6] = *b"NOTE\x02\x00";

/// What the app knows about the note store on the device.
///
/// WHY this is an enum and not a bool: "the platform refused to read the
/// store", "the store is corrupt" and "the store is empty" are three
/// different facts. A single flag made all three look like an empty list,
/// which told the user their notes were gone when the app had merely failed
/// to read them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StorageState {
    /// The store was read and every note in it validated.
    Ready,
    /// There is no store on the device yet.
    Empty,
    /// The platform refused to read the store.
    Refused,
    /// The store was read and failed its checksum or its structure.
    Corrupt,
    /// A staged copy was restored after the live store failed to validate.
    Recovered,
}

impl StorageState {
    /// Whether a write to the store can be expected to land.
    pub fn writable(self) -> bool {
        !matches!(self, StorageState::Refused)
    }
}

// =============================================================================
// Note Tones & Categories
// =============================================================================
//
// WHY a note's palette is a neutral ladder and not a set of hues: a note used
// to carry one of eight saturated themes -- coral, amber, mint, teal, sky,
// lavender, rose, slate -- and those eight were the last coloured chrome in
// the suite. The platform reserves colour for its logo, its model identity and
// a status that means something, so a note's "colour" is now the tone of the
// paper it is written on: four steps of the light theme's surface ladder and
// four of the dark theme's, from white paper to black ink. Every value below
// is a theme role; none of them is a hand-picked hex value.
//
// WHY the dark tones come from THEME_DARK: the dark half of the platform's
// theme already pairs a surface with the ink that is legible on it. A tone
// that invented its own greys would be a second dark theme inside the app,
// and the ink on it would be a guess.
//
// WHY the ladder is still a choice: the feature is that a note carries a
// tone, and the user picks it. Collapsing the eight tones to one surface
// would leave the picker with eight identical swatches, which is not a
// choice; it would also be a silent removal of a feature.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NoteTone {
    /// The card's fill.
    pub fill: u32,
    /// The card's hairline.
    pub border: u32,
    /// The fill of the category chip on this card.
    pub chip: u32,
    /// The note's accent: its rail, its checkbox fill and its cursor.
    ///
    /// WHY the accent is an ink and not a hue: on a monochrome surface the
    /// accent is the strongest ink the surface carries. A dark tone takes the
    /// dark theme's primary, which is the light ink that is legible on it.
    pub accent: u32,
    /// The ink of the note's title.
    pub title_ink: u32,
    /// The ink of the note's body.
    pub body_ink: u32,
    /// The ink of the note's metadata: its timestamp, its category label and
    /// a completed checklist line.
    pub muted_ink: u32,
}

/// The number of tones a note may carry.
pub const NOTE_TONE_COUNT: usize = 8;

/// A tone cut from the light half of the platform's theme.
///
/// WHY the helper and not seven literals per entry: a tone's hairline and ink
/// are decisions the platform already made, and writing them once means a
/// palette entry cannot accidentally pair a light fill with dark-theme ink.
const fn paper(fill: u32, chip: u32) -> NoteTone {
    NoteTone {
        fill,
        border: THEME.border,
        chip,
        accent: THEME.primary,
        title_ink: THEME.on_surface,
        body_ink: THEME.on_surface,
        muted_ink: THEME.on_surface_variant,
    }
}

/// A tone cut from the dark half of the platform's theme.
const fn ink(fill: u32, chip: u32) -> NoteTone {
    NoteTone {
        fill,
        border: THEME_DARK.border,
        chip,
        accent: THEME_DARK.primary,
        title_ink: THEME_DARK.on_surface,
        body_ink: THEME_DARK.on_surface,
        muted_ink: THEME_DARK.on_surface_variant,
    }
}

/// The eight tones a note may be written on, from paper to ink.
pub const NOTE_TONES: [NoteTone; NOTE_TONE_COUNT] = [
    // 0: Paper -- the platform's resting card.
    paper(THEME.surface_container, THEME.surface_container_low),
    // 1: Canvas -- the page itself, a card that does not lift off it.
    paper(THEME.surface, THEME.surface_container_high),
    // 2: Mist -- the well below a card.
    paper(THEME.surface_container_low, THEME.surface_container_high),
    // 3: Stone -- the raised step.
    paper(THEME.surface_container_high, THEME.surface_container_low),
    // 4: Graphite -- the highest step of the dark theme's ladder.
    ink(THEME_DARK.surface_container_high, THEME_DARK.surface_container_low),
    // 5: Charcoal -- a card one step above the dark page.
    ink(THEME_DARK.surface_container_low, THEME_DARK.surface_container_high),
    // 6: Ink -- the dark theme's resting card.
    ink(THEME_DARK.surface_container, THEME_DARK.surface_container_high),
    // 7: Midnight -- the dark page itself.
    ink(THEME_DARK.surface, THEME_DARK.surface_container_low),
];

pub const CATEGORY_NAMES: [&[u8]; 5] = [
    b"All",
    b"Work",
    b"Personal",
    b"Ideas",
    b"System",
];

// =============================================================================
// Civil Time: the only clock a note may ever carry
// =============================================================================
//
// WHY a packed civil time rather than a Unix epoch: the kernel's RTC reports
// the *local* civil fields ("year", "month", "day", "hour", "minute") and
// applies the device time zone itself. Storing the civil fields the kernel
// actually returned means a note's timestamp is exactly what the clock said
// at the moment it was written, with no epoch-to-civil conversion that could
// disagree with the device's time zone.
//
// A note that could not be stamped carries `TIMESTAMP_UNKNOWN` and the UI
// says so. It never carries a made-up time.

/// Sentinel for "this note has no observed timestamp".
pub const TIMESTAMP_UNKNOWN: u64 = 0;

/// A civil date and time as reported by the device's real-time clock.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CivilTime {
    pub year: u32,
    pub month: u32,
    pub day: u32,
    pub hour: u32,
    pub minute: u32,
}

/// Three-letter month abbreviations, indexed by `month - 1`.
pub const MONTH_ABBREV: [&[u8]; 12] = [
    b"Jan", b"Feb", b"Mar", b"Apr", b"May", b"Jun", b"Jul", b"Aug", b"Sep", b"Oct", b"Nov", b"Dec",
];

/// Packs a civil time into the decimal form `YYYYMMDDHHMM`.
///
/// WHY this encoding: it is monotonic in the civil fields, it round-trips
/// exactly, and it sorts the same way the calendar does. The decimal layout
/// is `YYYY | MM | DD | HH | MM`, so the year occupies the eight highest
/// decimal places. A value of zero is reserved for "no timestamp observed"
/// and is rejected by `unpack_civil`.
pub fn pack_civil(t: CivilTime) -> u64 {
    (t.year as u64) * 100_000_000
        + (t.month as u64) * 1_000_000
        + (t.day as u64) * 10_000
        + (t.hour as u64) * 100
        + (t.minute as u64)
}

/// Unpacks a `YYYYMMDDHHMM` value, rejecting anything that is not a real civil
/// time.
///
/// WHY the validation matters: the packed form is also what a stale or
/// foreign blob would contain. A year of zero, a month of thirteen or a
/// minute of ninety-nine is not a time the clock ever reported, so it is
/// reported as "no timestamp" rather than drawn as one.
pub fn unpack_civil(packed: u64) -> Option<CivilTime> {
    if packed == TIMESTAMP_UNKNOWN {
        return None;
    }
    let year = (packed / 100_000_000) as u32;
    let month = ((packed / 1_000_000) % 100) as u32;
    let day = ((packed / 10_000) % 100) as u32;
    let hour = ((packed / 100) % 100) as u32;
    let minute = (packed % 100) as u32;

    if !(1970..=9999).contains(&year)
        || !(1..=12).contains(&month)
        || !(1..=31).contains(&day)
        || hour > 23
        || minute > 59
    {
        return None;
    }
    Some(CivilTime {
        year,
        month,
        day,
        hour,
        minute,
    })
}

/// Days since the Unix epoch for a civil date (Howard Hinnant's algorithm).
///
/// WHY this is here and not an epoch conversion: the app compares a note's
/// calendar day against the current calendar day to decide whether to print
/// "Today" or "Yesterday". That comparison needs a day number, and it needs
/// one that is derived from the same civil fields the clock reported.
pub fn days_from_civil(year: u32, month: u32, day: u32) -> i64 {
    let y = year as i64;
    let m = month as i64;
    let d = day as i64;

    // Shift the year so that March is the first month of the year.
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400; // [0, 399]
    let mp = (m + 9) % 12; // March = 0
    let doy = (153 * mp + 2) / 5 + d - 1; // [0, 365]
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy; // [0, 146096]
    era * 146_097 + doe - 719_468
}

/// Formats a clock time as `H:MM AM` / `H:MM PM` into `buf`, returning the
/// number of bytes written.
///
/// WHY 12-hour: the previous UI showed "10:30 AM", which is what a notes
/// list on a phone reads as. The digits come from the clock either way.
pub fn format_clock(t: CivilTime, buf: &mut [u8]) -> usize {
    let hour12 = match t.hour % 12 {
        0 => 12u32,
        h => h,
    };
    let mut n = 0usize;
    // WHY the leading zero is dropped: a caption that reads "04:45 PM" is a
    // spreadsheet, not a phone. Only the two-digit case carries a tens
    // digit.
    if hour12 >= 10 && n < buf.len() {
        buf[n] = b'0' + (hour12 / 10) as u8;
        n += 1;
    }
    if n < buf.len() {
        buf[n] = b'0' + (hour12 % 10) as u8;
        n += 1;
    }
    if n < buf.len() {
        buf[n] = b':';
        n += 1;
    }
    if n < buf.len() {
        buf[n] = b'0' + (t.minute / 10) as u8;
        n += 1;
    }
    if n < buf.len() {
        buf[n] = b'0' + (t.minute % 10) as u8;
        n += 1;
    }
    if n < buf.len() {
        buf[n] = b' ';
        n += 1;
    }
    let suffix: &[u8] = if t.hour < 12 { b"AM" } else { b"PM" };
    let take = suffix.len().min(buf.len().saturating_sub(n));
    buf[n..n + take].copy_from_slice(&suffix[..take]);
    n + take
}

/// Formats a note's timestamp for the card caption.
///
/// `now` is the current civil time, or `None` when the clock is unreadable.
/// The result is one of:
///   * `Today 10:30 AM` / `Yesterday 4:15 PM` -- when the note falls on the
///     current or previous calendar day, and the current day is known;
///   * `18 Oct 2026` -- any other day;
///   * `No timestamp` -- the clock was never read for this note.
///
/// WHY "No timestamp" and never a substitute time: the note's edit time is a
/// fact about the device, and the device either knows it or does not.
pub fn format_timestamp(
    packed: u64,
    now: Option<CivilTime>,
    buf: &mut [u8],
) -> usize {
    let Some(note_time) = unpack_civil(packed) else {
        let s = b"No timestamp";
        let take = s.len().min(buf.len());
        buf[..take].copy_from_slice(&s[..take]);
        return take;
    };

    let mut n = 0usize;
    // A relative label is only honest when the current day is known.
    if let Some(now_time) = now {
        let now_days = days_from_civil(now_time.year, now_time.month, now_time.day);
        let note_days = days_from_civil(note_time.year, note_time.month, note_time.day);
        let prefix: &[u8] = if note_days == now_days {
            b"Today "
        } else if note_days == now_days - 1 {
            b"Yesterday "
        } else {
            b""
        };
        if !prefix.is_empty() {
            let take = prefix.len().min(buf.len());
            buf[..take].copy_from_slice(&prefix[..take]);
            n = take;
        } else {
            // Fall through to the absolute date below.
        }
    }

    if n > 0 {
        // A relative label was written; append the clock time.
        let time_len = format_clock(note_time, &mut buf[n..]);
        return n + time_len;
    }

    // Absolute date: "18 Oct 2026".
    let day = note_time.day;
    let mut digit_buf = [0u8; 2];
    let day_len = decimal_u16(day, &mut digit_buf);
    let take = day_len.min(buf.len());
    buf[..take].copy_from_slice(&digit_buf[..take]);
    n = take;

    if n < buf.len() {
        buf[n] = b' ';
        n += 1;
    }
    let month = MONTH_ABBREV[(note_time.month - 1) as usize % 12];
    let take = month.len().min(buf.len().saturating_sub(n));
    buf[n..n + take].copy_from_slice(&month[..take]);
    n += take;

    if n < buf.len() {
        buf[n] = b' ';
        n += 1;
    }
    let mut year_buf = [0u8; 4];
    let year_len = decimal_u16(note_time.year, &mut year_buf);
    let take = year_len.min(buf.len().saturating_sub(n));
    buf[n..n + take].copy_from_slice(&year_buf[..take]);
    n + take
}

/// Formats the note-count badge as `[N]` and returns its length.
///
/// WHY the count is written out in full: the store holds up to `MAX_NOTES`
/// notes, and the badge used to be built as `b'0' + count.min(9)`, so a
/// device holding twelve notes reported nine. A count is a measurement and
/// has to survive the second digit.
pub fn format_count_badge(count: usize, buf: &mut [u8]) -> usize {
    if buf.is_empty() {
        return 0;
    }
    buf[0] = b'[';
    let mut n = 1;
    n += decimal_u16(count as u32, &mut buf[n..]);
    if n < buf.len() {
        buf[n] = b']';
        n += 1;
    }
    n
}

/// Writes the decimal form of `value` into `buf` and returns its length.
pub fn decimal_u16(value: u32, buf: &mut [u8]) -> usize {
    if value == 0 {
        if buf.is_empty() {
            return 0;
        }
        buf[0] = b'0';
        return 1;
    }
    let mut digits = [0u8; 10];
    let mut len = 0usize;
    let mut v = value;
    while v > 0 && len < digits.len() {
        digits[len] = b'0' + (v % 10) as u8;
        v /= 10;
        len += 1;
    }
    let mut n = 0usize;
    while len > 0 {
        len -= 1;
        if n < buf.len() {
            buf[n] = digits[len];
            n += 1;
        }
    }
    n
}

// =============================================================================
// Note Data Structure
// =============================================================================

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Note {
    pub id: u32,
    pub title: [u8; MAX_TITLE_LEN],
    pub title_len: usize,
    pub body: [u8; MAX_BODY_LEN],
    pub body_len: usize,
    pub created_at: u64,
    pub updated_at: u64,
    pub is_dirty: bool,
    /// The tone the note is written on, as a slot in [`NOTE_TONES`].
    ///
    /// WHY the field still says "color": it is a slot in the on-disk record,
    /// and renaming it would change the format for a cosmetic reason. The
    /// value is a tone index; the name is the format's history.
    pub color_idx: u8,
    pub is_pinned: bool,
    pub category: u8,
}

impl Note {
    pub const fn empty() -> Self {
        Self {
            id: 0,
            title: [0; MAX_TITLE_LEN],
            title_len: 0,
            body: [0; MAX_BODY_LEN],
            body_len: 0,
            created_at: 0,
            updated_at: 0,
            is_dirty: false,
            color_idx: 0,
            is_pinned: false,
            category: 0,
        }
    }

    pub fn new(id: u32, title: &str, body: &str) -> Self {
        let mut note = Self::empty();
        note.id = id;
        note.set_title(title);
        note.set_body(body);
        note.is_dirty = false;
        note
    }

    /// Chooses the tone the note is written on.
    ///
    /// WHY the value is taken modulo the ladder: `color_idx` is a byte read
    /// from the store, and an old blob may carry any value. A slot outside
    /// the ladder must wrap to a real tone rather than panic the renderer.
    pub fn set_color(&mut self, idx: u8) {
        self.color_idx = idx % NOTE_TONE_COUNT as u8;
        self.is_dirty = true;
    }

    pub fn toggle_pinned(&mut self) {
        self.is_pinned = !self.is_pinned;
        self.is_dirty = true;
    }

    pub fn set_category(&mut self, cat: u8) {
        self.category = cat % 5;
        self.is_dirty = true;
    }

    pub fn set_title(&mut self, title: &str) {
        let bytes = title.as_bytes();
        let len = bytes.len().min(MAX_TITLE_LEN);
        self.title[..len].copy_from_slice(&bytes[..len]);
        for b in &mut self.title[len..] {
            *b = 0;
        }
        self.title_len = len;
        self.is_dirty = true;
    }

    pub fn set_body(&mut self, body: &str) {
        let bytes = body.as_bytes();
        let len = bytes.len().min(MAX_BODY_LEN);
        self.body[..len].copy_from_slice(&bytes[..len]);
        for b in &mut self.body[len..] {
            *b = 0;
        }
        self.body_len = len;
        self.is_dirty = true;
    }

    pub fn as_title_str(&self) -> &str {
        core::str::from_utf8(&self.title[..self.title_len]).unwrap_or("")
    }

    pub fn as_body_str(&self) -> &str {
        core::str::from_utf8(&self.body[..self.body_len]).unwrap_or("")
    }

    pub fn title_bytes(&self) -> &[u8] {
        &self.title[..self.title_len]
    }

    pub fn body_bytes(&self) -> &[u8] {
        &self.body[..self.body_len]
    }
}

// =============================================================================
// Notes Store
// =============================================================================

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NotesStore {
    pub notes: [Note; MAX_NOTES],
    pub count: usize,
    pub next_id: u32,
}

impl NotesStore {
    pub const fn new() -> Self {
        Self {
            notes: [Note::empty(); MAX_NOTES],
            count: 0,
            next_id: 1,
        }
    }

    pub fn count(&self) -> usize {
        self.count
    }

    pub fn create_note(&mut self, title: &str, body: &str) -> Option<u32> {
        if self.count >= MAX_NOTES {
            return None;
        }
        let id = self.next_id;
        self.next_id = self.next_id.wrapping_add(1);
        if self.next_id == 0 {
            self.next_id = 1;
        }
        let mut note = Note::new(id, title, body);
        note.is_dirty = true;
        self.notes[self.count] = note;
        self.count += 1;
        Some(id)
    }

    pub fn get_note(&self, id: u32) -> Option<&Note> {
        for i in 0..self.count {
            if self.notes[i].id == id {
                return Some(&self.notes[i]);
            }
        }
        None
    }

    pub fn get_note_mut(&mut self, id: u32) -> Option<&mut Note> {
        for i in 0..self.count {
            if self.notes[i].id == id {
                return Some(&mut self.notes[i]);
            }
        }
        None
    }

    pub fn delete_note(&mut self, id: u32) -> bool {
        let mut found_idx = None;
        for i in 0..self.count {
            if self.notes[i].id == id {
                found_idx = Some(i);
                break;
            }
        }
        if let Some(idx) = found_idx {
            for i in idx..(self.count - 1) {
                self.notes[i] = self.notes[i + 1];
            }
            self.count -= 1;
            self.notes[self.count] = Note::empty();
            true
        } else {
            false
        }
    }

    pub fn get_by_index(&self, index: usize) -> Option<&Note> {
        if index < self.count {
            Some(&self.notes[index])
        } else {
            None
        }
    }

    pub fn get_by_index_mut(&mut self, index: usize) -> Option<&mut Note> {
        if index < self.count {
            Some(&mut self.notes[index])
        } else {
            None
        }
    }

    /// A store starts empty.
    ///
    /// WHY there is no `seed_defaults` any more: the store used to be seeded
    /// with four notes the app invented -- including one titled "Hardware
    /// Specs" that asserted a scanout resolution, an upscaler and a font
    /// rasteriser the app had never measured. Those notes were then written
    /// to the user's persistent storage, so from the second launch onward
    /// the device carried four documents the user never wrote, rendered in
    /// the same card style as the user's own notes and indistinguishable
    /// from real content. A notes app whose contents are the user's is empty
    /// when the user has written nothing, and the UI already says so.
    pub fn new_empty(&mut self) {
        self.count = 0;
        self.next_id = 1;
        for i in 0..MAX_NOTES {
            self.notes[i] = Note::empty();
        }
    }

    /// Stamps a note with an observed civil time, or clears the stamp when the
    /// clock could not be read.
    ///
    /// WHY this is the only way a note's timestamp changes: the value comes
    /// from the device's real-time clock, and `TIMESTAMP_UNKNOWN` is the
    /// honest answer when that clock refuses.
    pub fn stamp(&mut self, id: u32, packed: u64) -> bool {
        let Some(note) = self.get_note_mut(id) else {
            return false;
        };
        note.updated_at = packed;
        if note.created_at == TIMESTAMP_UNKNOWN {
            note.created_at = packed;
        }
        true
    }

    pub fn serialize(&self, buf: &mut [u8]) -> Result<usize, StoreError> {
        if buf.len() < 12 {
            return Err(StoreError::BufferTooSmall);
        }

        buf[0..6].copy_from_slice(&MAGIC_HEADER);
        buf[6..8].copy_from_slice(&(self.count as u16).to_le_bytes());

        let mut offset = 12;
        for i in 0..self.count {
            let note = &self.notes[i];
            let rec_size = 4 + 2 + note.title_len + 2 + note.body_len + 8 + 8 + 1 + 1 + 1 + 1;
            if offset + rec_size > buf.len() {
                return Err(StoreError::BufferTooSmall);
            }

            // id: u32
            buf[offset..offset + 4].copy_from_slice(&note.id.to_le_bytes());
            offset += 4;

            // title_len: u16 & title
            buf[offset..offset + 2].copy_from_slice(&(note.title_len as u16).to_le_bytes());
            offset += 2;
            buf[offset..offset + note.title_len].copy_from_slice(&note.title[..note.title_len]);
            offset += note.title_len;

            // body_len: u16 & body
            buf[offset..offset + 2].copy_from_slice(&(note.body_len as u16).to_le_bytes());
            offset += 2;
            buf[offset..offset + note.body_len].copy_from_slice(&note.body[..note.body_len]);
            offset += note.body_len;

            // created_at: u64
            buf[offset..offset + 8].copy_from_slice(&note.created_at.to_le_bytes());
            offset += 8;

            // updated_at: u64
            buf[offset..offset + 8].copy_from_slice(&note.updated_at.to_le_bytes());
            offset += 8;

            // is_dirty: u8
            buf[offset] = if note.is_dirty { 1 } else { 0 };
            offset += 1;

            // color_idx: u8
            buf[offset] = note.color_idx;
            offset += 1;

            // is_pinned: u8
            buf[offset] = if note.is_pinned { 1 } else { 0 };
            offset += 1;

            // category: u8
            buf[offset] = note.category;
            offset += 1;
        }

        let checksum = fletcher32(&buf[12..offset]);
        buf[8..12].copy_from_slice(&checksum.to_le_bytes());

        Ok(offset)
    }

    pub fn deserialize(&mut self, buf: &[u8]) -> Result<(), StoreError> {
        if buf.len() < 12 {
            return Err(StoreError::UnexpectedEof);
        }

        if &buf[0..4] != b"NOTE" {
            return Err(StoreError::InvalidMagic);
        }
        let version = buf[4];
        // Versions 1 and 2 are still read, so a user's notes are never lost
        // to a format change -- but their timestamp slots are discarded,
        // because those held epoch seconds written from a constant the app
        // never read from a clock. Version 3 holds the civil fields the
        // real-time clock actually reported.
        if (version != 1 && version != 2 && version != 3) || buf[5] != 0 {
            return Err(StoreError::UnsupportedVersion);
        }
        let legacy_timestamps = version < 3;

        let count = u16::from_le_bytes([buf[6], buf[7]]) as usize;
        if count > MAX_NOTES {
            return Err(StoreError::CapacityExceeded);
        }

        let expected_checksum = u32::from_le_bytes([buf[8], buf[9], buf[10], buf[11]]);

        let mut temp_notes = [Note::empty(); MAX_NOTES];
        let mut offset = 12;

        for i in 0..count {
            if offset + 4 > buf.len() {
                return Err(StoreError::UnexpectedEof);
            }
            let id = u32::from_le_bytes([buf[offset], buf[offset + 1], buf[offset + 2], buf[offset + 3]]);
            offset += 4;

            if offset + 2 > buf.len() {
                return Err(StoreError::UnexpectedEof);
            }
            let title_len = u16::from_le_bytes([buf[offset], buf[offset + 1]]) as usize;
            offset += 2;
            if title_len > MAX_TITLE_LEN {
                return Err(StoreError::CorruptedData);
            }
            if offset + title_len > buf.len() {
                return Err(StoreError::UnexpectedEof);
            }
            let mut title = [0u8; MAX_TITLE_LEN];
            title[..title_len].copy_from_slice(&buf[offset..offset + title_len]);
            offset += title_len;

            if offset + 2 > buf.len() {
                return Err(StoreError::UnexpectedEof);
            }
            let body_len = u16::from_le_bytes([buf[offset], buf[offset + 1]]) as usize;
            offset += 2;
            if body_len > MAX_BODY_LEN {
                return Err(StoreError::CorruptedData);
            }
            if offset + body_len > buf.len() {
                return Err(StoreError::UnexpectedEof);
            }
            let mut body = [0u8; MAX_BODY_LEN];
            body[..body_len].copy_from_slice(&buf[offset..offset + body_len]);
            offset += body_len;

            let extra_bytes = if version >= 2 { 20 } else { 17 };
            if offset + extra_bytes > buf.len() {
                return Err(StoreError::UnexpectedEof);
            }
            let created_at = if legacy_timestamps {
                // A version 1 or 2 slot holds a value the device never
                // observed. It is read as "no timestamp" rather than shown.
                TIMESTAMP_UNKNOWN
            } else {
                u64::from_le_bytes([
                    buf[offset], buf[offset + 1], buf[offset + 2], buf[offset + 3],
                    buf[offset + 4], buf[offset + 5], buf[offset + 6], buf[offset + 7],
                ])
            };
            offset += 8;

            let updated_at = if legacy_timestamps {
                TIMESTAMP_UNKNOWN
            } else {
                u64::from_le_bytes([
                    buf[offset], buf[offset + 1], buf[offset + 2], buf[offset + 3],
                    buf[offset + 4], buf[offset + 5], buf[offset + 6], buf[offset + 7],
                ])
            };
            offset += 8;

            let is_dirty = buf[offset] != 0;
            offset += 1;

            let (color_idx, is_pinned, category) = if version >= 2 {
                let c = buf[offset];
                let p = buf[offset + 1] != 0;
                let cat = buf[offset + 2];
                offset += 3;
                (c, p, cat)
            } else {
                (0, false, 0)
            };

            temp_notes[i] = Note {
                id,
                title,
                title_len,
                body,
                body_len,
                created_at,
                updated_at,
                is_dirty,
                color_idx,
                is_pinned,
                category,
            };
        }

        let actual_checksum = fletcher32(&buf[12..offset]);
        if actual_checksum != expected_checksum {
            return Err(StoreError::ChecksumMismatch);
        }

        self.notes = temp_notes;
        self.count = count;

        let mut max_id = 0;
        for i in 0..count {
            if self.notes[i].id > max_id {
                max_id = self.notes[i].id;
            }
        }
        self.next_id = max_id.wrapping_add(1);
        if self.next_id == 0 {
            self.next_id = 1;
        }

        Ok(())
    }
}

// =============================================================================
// Text Editing Buffer Engine
// =============================================================================

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LineSpan {
    pub start: usize,
    pub end: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TextEditorBuffer {
    pub buf: [u8; MAX_BODY_LEN],
    pub len: usize,
    pub cursor: usize,
}

impl TextEditorBuffer {
    pub const fn new() -> Self {
        Self {
            buf: [0; MAX_BODY_LEN],
            len: 0,
            cursor: 0,
        }
    }

    pub fn from_str(s: &str) -> Self {
        let mut buf = Self::new();
        buf.insert_str(s);
        buf
    }

    pub fn load_from_slice(&mut self, slice: &[u8]) {
        let l = slice.len().min(MAX_BODY_LEN);
        self.buf[..l].copy_from_slice(&slice[..l]);
        for b in &mut self.buf[l..] {
            *b = 0;
        }
        self.len = l;
        self.cursor = l;
    }

    pub fn clear(&mut self) {
        self.len = 0;
        self.cursor = 0;
        for b in &mut self.buf {
            *b = 0;
        }
    }

    pub fn as_str(&self) -> &str {
        core::str::from_utf8(&self.buf[..self.len]).unwrap_or("")
    }

    pub fn as_bytes(&self) -> &[u8] {
        &self.buf[..self.len]
    }

    pub fn insert_char(&mut self, c: char) -> bool {
        let mut enc = [0u8; 4];
        let enc_str = c.encode_utf8(&mut enc);
        let char_len = enc_str.len();

        if self.len + char_len > MAX_BODY_LEN {
            return false;
        }

        if self.cursor < self.len {
            self.buf.copy_within(self.cursor..self.len, self.cursor + char_len);
        }
        self.buf[self.cursor..self.cursor + char_len].copy_from_slice(enc_str.as_bytes());
        self.cursor += char_len;
        self.len += char_len;
        true
    }

    pub fn insert_str(&mut self, s: &str) -> bool {
        let bytes = s.as_bytes();
        let bytes_len = bytes.len();

        if self.len + bytes_len > MAX_BODY_LEN {
            return false;
        }

        if self.cursor < self.len {
            self.buf.copy_within(self.cursor..self.len, self.cursor + bytes_len);
        }
        self.buf[self.cursor..self.cursor + bytes_len].copy_from_slice(bytes);
        self.cursor += bytes_len;
        self.len += bytes_len;
        true
    }

    pub fn toggle_checklist_at(&mut self, line_start: usize) -> bool {
        if line_start + 3 <= self.len {
            if self.buf[line_start] == b'[' && self.buf[line_start + 2] == b']' {
                if self.buf[line_start + 1] == b' ' {
                    self.buf[line_start + 1] = b'x';
                    return true;
                } else if self.buf[line_start + 1] == b'x' || self.buf[line_start + 1] == b'X' {
                    self.buf[line_start + 1] = b' ';
                    return true;
                }
            }
        }
        false
    }

    pub fn insert_checklist_item(&mut self) -> bool {
        if self.len == 0 {
            self.insert_str("[ ] ")
        } else {
            self.insert_str("\n[ ] ")
        }
    }

    pub fn backspace(&mut self) -> bool {
        if self.cursor == 0 {
            return false;
        }

        let mut prev = self.cursor - 1;
        while prev > 0 && (self.buf[prev] & 0xC0) == 0x80 {
            prev -= 1;
        }
        let char_len = self.cursor - prev;

        if self.cursor < self.len {
            self.buf.copy_within(self.cursor..self.len, prev);
        }
        self.cursor = prev;
        self.len -= char_len;
        self.buf[self.len..self.len + char_len].fill(0);
        true
    }

    pub fn delete(&mut self) -> bool {
        if self.cursor >= self.len {
            return false;
        }

        let mut next = self.cursor + 1;
        while next < self.len && (self.buf[next] & 0xC0) == 0x80 {
            next += 1;
        }
        let char_len = next - self.cursor;

        if next < self.len {
            self.buf.copy_within(next..self.len, self.cursor);
        }
        self.len -= char_len;
        self.buf[self.len..self.len + char_len].fill(0);
        true
    }

    pub fn move_cursor_left(&mut self) {
        if self.cursor == 0 {
            return;
        }
        let mut prev = self.cursor - 1;
        while prev > 0 && (self.buf[prev] & 0xC0) == 0x80 {
            prev -= 1;
        }
        self.cursor = prev;
    }

    pub fn move_cursor_right(&mut self) {
        if self.cursor >= self.len {
            return;
        }
        let mut next = self.cursor + 1;
        while next < self.len && (self.buf[next] & 0xC0) == 0x80 {
            next += 1;
        }
        self.cursor = next;
    }

    pub fn move_cursor_home(&mut self) {
        let mut pos = self.cursor;
        while pos > 0 && self.buf[pos - 1] != b'\n' {
            pos -= 1;
        }
        self.cursor = pos;
    }

    pub fn move_cursor_end(&mut self) {
        let mut pos = self.cursor;
        while pos < self.len && self.buf[pos] != b'\n' {
            pos += 1;
        }
        self.cursor = pos;
    }

    pub fn move_cursor_to_start(&mut self) {
        self.cursor = 0;
    }

    pub fn move_cursor_to_end(&mut self) {
        self.cursor = self.len;
    }

    /// Breaks the body into drawn line spans, wrapping on the run's measured
    /// advance rather than a character count.
    ///
    /// WHY pixels and not characters: the editor draws each span with the
    /// outline face at `scale`, whose glyphs are not one width. A count of
    /// characters is a count of a bitmap cell that no longer draws this text,
    /// so a "line" of a fixed count could be wider than the column it is drawn
    /// in and run off the page and past the rule. Here a line ends where the
    /// ladder's own advances no longer fit `max_w`, the column the line is drawn
    /// in, so the break is the break the screen shows.
    pub fn compute_lines(&self, scale: usize, max_w: usize, out: &mut [LineSpan]) -> usize {
        if out.is_empty() {
            return 0;
        }
        if self.len == 0 {
            out[0] = LineSpan { start: 0, end: 0 };
            return 1;
        }
        let em = std::design::font::em_for_scale(scale);
        let budget = max_w.max(1);
        let mut line_count = 0;
        let mut line_start = 0;

        while line_start < self.len && line_count < out.len() {
            let mut idx = line_start;
            let mut line_end = line_start;
            let mut cur_w = 0usize;
            let mut last_space: Option<usize> = None;

            while idx < self.len {
                let b = self.buf[idx];
                if b == b'\n' {
                    line_end = idx;
                    idx += 1;
                    break;
                }
                let adv = std::design::typeface::advance_px(b, em) as usize;
                if cur_w + adv > budget {
                    // The run no longer fits the column. Break at the last
                    // space so a word is not split where it can be avoided;
                    // with no space in the line the over-long word is broken
                    // here so the span still makes forward progress.
                    if let Some(space_idx) = last_space {
                        line_end = space_idx;
                        idx = space_idx + 1;
                    } else {
                        line_end = if idx == line_start { line_start + 1 } else { idx };
                        idx = line_end;
                    }
                    break;
                }
                cur_w += adv;
                if b == b' ' {
                    last_space = Some(idx);
                }
                idx += 1;
                line_end = idx;
            }

            out[line_count] = LineSpan {
                start: line_start,
                end: line_end,
            };
            line_count += 1;
            line_start = idx;
        }

        if self.len > 0 && self.buf[self.len - 1] == b'\n' && line_count < out.len() {
            out[line_count] = LineSpan {
                start: self.len,
                end: self.len,
            };
            line_count += 1;
        }

        line_count
    }

    pub fn line_count(&self, scale: usize, max_w: usize) -> usize {
        let mut spans = [LineSpan { start: 0, end: 0 }; 64];
        self.compute_lines(scale, max_w, &mut spans)
    }

    pub fn cursor_line_col(&self, scale: usize, max_w: usize) -> (usize, usize) {
        let mut spans = [LineSpan { start: 0, end: 0 }; 64];
        let total = self.compute_lines(scale, max_w, &mut spans);
        if total == 0 {
            return (0, 0);
        }
        for (i, span) in spans[..total].iter().enumerate() {
            if self.cursor >= span.start && (self.cursor <= span.end || i == total - 1) {
                let col = self.cursor.saturating_sub(span.start);
                return (i, col);
            }
        }
        (total.saturating_sub(1), 0)
    }

    pub fn move_cursor_up(&mut self, scale: usize, max_w: usize) {
        let mut spans = [LineSpan { start: 0, end: 0 }; 64];
        let total = self.compute_lines(scale, max_w, &mut spans);
        if total == 0 {
            return;
        }
        let (line, col) = self.cursor_line_col(scale, max_w);
        if line > 0 {
            let target_span = spans[line - 1];
            self.cursor = (target_span.start + col).min(target_span.end);
        }
    }

    pub fn move_cursor_down(&mut self, scale: usize, max_w: usize) {
        let mut spans = [LineSpan { start: 0, end: 0 }; 64];
        let total = self.compute_lines(scale, max_w, &mut spans);
        if total == 0 {
            return;
        }
        let (line, col) = self.cursor_line_col(scale, max_w);
        if line + 1 < total {
            let target_span = spans[line + 1];
            self.cursor = (target_span.start + col).min(target_span.end);
        }
    }
}

// =============================================================================
// Fletcher-32 Checksum & Serialization Errors
// =============================================================================

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StoreError {
    BufferTooSmall,
    InvalidMagic,
    UnsupportedVersion,
    ChecksumMismatch,
    CorruptedData,
    CapacityExceeded,
    UnexpectedEof,
}

pub fn fletcher32(data: &[u8]) -> u32 {
    let mut sum1: u32 = 0;
    let mut sum2: u32 = 0;
    let mut i = 0;
    let len = data.len();

    while i < len {
        let word = if i + 1 < len {
            u16::from_le_bytes([data[i], data[i + 1]]) as u32
        } else {
            data[i] as u32
        };
        sum1 = (sum1 + word) % 65535;
        sum2 = (sum2 + sum1) % 65535;
        i += 2;
    }

    (sum2 << 16) | sum1
}

// =============================================================================
// Unit Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_the_count_badge_carries_every_digit() {
        // WHY this test exists: the badge used to be `b'0' + count.min(9)`,
        // so a device holding twelve notes reported nine. A count is a
        // measurement and it has to survive the second digit.
        let mut buf = [0u8; 8];
        let n = format_count_badge(0, &mut buf);
        assert_eq!(&buf[..n], b"[0]");
        let n = format_count_badge(9, &mut buf);
        assert_eq!(&buf[..n], b"[9]");
        let n = format_count_badge(10, &mut buf);
        assert_eq!(&buf[..n], b"[10]");
        let n = format_count_badge(12, &mut buf);
        assert_eq!(&buf[..n], b"[12]");
        let n = format_count_badge(16, &mut buf);
        assert_eq!(&buf[..n], b"[16]");
    }

    #[test]
    fn test_the_count_badge_stays_inside_its_buffer() {
        let mut buf = [0u8; 3];
        // Three bytes hold "[9]" and nothing more; a wider count must not
        // write past the end of the buffer.
        let n = format_count_badge(16, &mut buf);
        assert!(n <= buf.len());
    }

    #[test]
    fn test_the_storage_states_are_distinguishable() {
        // WHY this test exists: a single boolean collapsed "refused",
        // "corrupt" and "empty" into one outcome, and the list then told the
        // user their notes were gone when the app had only failed to read
        // them.
        assert_ne!(StorageState::Ready, StorageState::Empty);
        assert_ne!(StorageState::Refused, StorageState::Corrupt);
        assert_ne!(StorageState::Recovered, StorageState::Corrupt);
        assert!(StorageState::Ready.writable());
        assert!(StorageState::Empty.writable());
        assert!(StorageState::Corrupt.writable());
        assert!(!StorageState::Refused.writable());
    }

    #[test]
    fn test_note_creation_and_fields() {
        let mut note = Note::new(42, "Architecture", "Microkernel security");
        assert_eq!(note.id, 42);
        assert_eq!(note.as_title_str(), "Architecture");
        assert_eq!(note.as_body_str(), "Microkernel security");
        assert_eq!(note.title_bytes(), b"Architecture");
        assert_eq!(note.body_bytes(), b"Microkernel security");
        assert!(!note.is_dirty);

        note.set_title("New Title");
        assert_eq!(note.as_title_str(), "New Title");
        assert!(note.is_dirty);

        note.set_body("Updated body text");
        assert_eq!(note.as_body_str(), "Updated body text");
        assert!(note.is_dirty);
    }

    #[test]
    fn test_notes_store_operations_and_ordering() {
        let mut store = NotesStore::new();
        assert_eq!(store.count(), 0);

        let id1 = store.create_note("First", "Body 1").expect("id1");
        let id2 = store.create_note("Second", "Body 2").expect("id2");
        let id3 = store.create_note("Third", "Body 3").expect("id3");

        assert_eq!(store.count(), 3);
        assert_eq!(store.get_note(id1).unwrap().as_title_str(), "First");
        assert_eq!(store.get_note(id2).unwrap().as_title_str(), "Second");
        assert_eq!(store.get_note(id3).unwrap().as_title_str(), "Third");

        // Index ordering
        assert_eq!(store.get_by_index(0).unwrap().id, id1);
        assert_eq!(store.get_by_index(1).unwrap().id, id2);
        assert_eq!(store.get_by_index(2).unwrap().id, id3);

        // Delete middle note and verify compact ordering
        let deleted = store.delete_note(id2);
        assert!(deleted);
        assert_eq!(store.count(), 2);
        assert_eq!(store.get_by_index(0).unwrap().id, id1);
        assert_eq!(store.get_by_index(1).unwrap().id, id3);
        assert!(store.get_note(id2).is_none());

        // Modify note via get_note_mut
        if let Some(n) = store.get_note_mut(id3) {
            n.set_title("Third Modified");
        }
        assert_eq!(store.get_note(id3).unwrap().as_title_str(), "Third Modified");
    }

    #[test]
    fn test_text_editor_operations() {
        let mut ed = TextEditorBuffer::new();
        assert_eq!(ed.len, 0);
        assert_eq!(ed.cursor, 0);

        // Insert characters
        assert!(ed.insert_char('H'));
        assert!(ed.insert_char('e'));
        assert!(ed.insert_char('l'));
        assert!(ed.insert_char('l'));
        assert!(ed.insert_char('o'));
        assert_eq!(ed.as_str(), "Hello");
        assert_eq!(ed.cursor, 5);

        // Cursor movement
        ed.move_cursor_left();
        assert_eq!(ed.cursor, 4);
        ed.move_cursor_left();
        assert_eq!(ed.cursor, 3);
        ed.move_cursor_right();
        assert_eq!(ed.cursor, 4);

        // Backspace inside text
        assert!(ed.backspace());
        assert_eq!(ed.as_str(), "Helo");
        assert_eq!(ed.cursor, 3);

        // Delete
        assert!(ed.delete());
        assert_eq!(ed.as_str(), "Hel");
        assert_eq!(ed.cursor, 3);

        // Home and End
        ed.move_cursor_home();
        assert_eq!(ed.cursor, 0);
        ed.move_cursor_end();
        assert_eq!(ed.cursor, 3);

        // Multiline insertion and navigation
        ed.insert_char('\n');
        ed.insert_str("World");
        assert_eq!(ed.as_str(), "Hel\nWorld");
        assert_eq!(ed.cursor, 9);

        ed.move_cursor_home();
        assert_eq!(ed.cursor, 4); // Start of "World" line
        ed.move_cursor_end();
        assert_eq!(ed.cursor, 9);
    }

    #[test]
    fn test_text_editor_wraps_by_measured_width() {
        use interos_std::design::font;
        let scale = crate::theme::type_scale::BODY;
        let mut ed = TextEditorBuffer::new();
        ed.insert_str("The quick brown fox jumps over the lazy dog");

        // A narrow column forces several wrapped lines.
        let budget = 200;
        let mut spans = [LineSpan { start: 0, end: 0 }; 16];
        let count = ed.compute_lines(scale, budget, &mut spans);

        assert!(count >= 3);
        // Every wrapped line fits the column it is drawn in.
        for s in &spans[..count] {
            assert!(font::measure(&ed.buf[s.start..s.end], scale) <= budget);
        }
        // The spans tile the buffer: the whole body stays reachable.
        assert_eq!(spans[0].start, 0);
        assert_eq!(spans[count - 1].end, ed.len);

        // Cursor line and col land on a real span boundary.
        ed.cursor = 0;
        assert_eq!(ed.cursor_line_col(scale, budget), (0, 0));
    }

    #[test]
    fn test_binary_serialization_roundtrip() {
        let mut store = NotesStore::new();
        // WHY this creates its own notes: the store used to be seeded with
        // four notes the app invented, which is exactly the fabrication this
        // round-trip test then asserted was "correct".
        store.create_note("First", "Body one");
        store.create_note("Second", "Body two");
        store.create_note("Third", "Body three");
        store.create_note("Fourth", "Body four");
        assert_eq!(store.count(), 4);

        let mut buf = [0u8; 4096];
        let bytes_written = store.serialize(&mut buf).expect("serialize should succeed");
        assert!(bytes_written > 12);

        // Verify header
        assert_eq!(&buf[0..6], &MAGIC_HEADER);
        let count = u16::from_le_bytes([buf[6], buf[7]]);
        assert_eq!(count, 4);

        // Deserialize into new store
        let mut restored = NotesStore::new();
        restored.deserialize(&buf[..bytes_written]).expect("deserialize should succeed");

        assert_eq!(restored.count(), 4);
        for i in 0..4 {
            let orig = store.get_by_index(i).unwrap();
            let rest = restored.get_by_index(i).unwrap();
            assert_eq!(orig.id, rest.id);
            assert_eq!(orig.as_title_str(), rest.as_title_str());
            assert_eq!(orig.as_body_str(), rest.as_body_str());
            assert_eq!(orig.created_at, rest.created_at);
            assert_eq!(orig.updated_at, rest.updated_at);
            assert_eq!(orig.color_idx, rest.color_idx);
            assert_eq!(orig.is_pinned, rest.is_pinned);
            assert_eq!(orig.category, rest.category);
        }
    }

    #[test]
    fn test_a_new_store_is_empty_and_never_seeded() {
        // WHY this matters: the app used to write four invented notes into the
        // user's persistent storage. A fresh store must hold nothing.
        let mut store = NotesStore::new();
        assert_eq!(store.count(), 0);
        store.new_empty();
        assert_eq!(store.count(), 0);
        for i in 0..MAX_NOTES {
            assert_eq!(store.get_by_index(i).map(|n| n.title_len), None);
        }
    }

    #[test]
    fn test_civil_time_packs_and_unpacks() {
        let t = CivilTime {
            year: 2026,
            month: 10,
            day: 18,
            hour: 16,
            minute: 45,
        };
        let packed = pack_civil(t);
        assert_eq!(unpack_civil(packed), Some(t));

        // Zero is the "never observed" sentinel, and must not unpack.
        assert_eq!(unpack_civil(TIMESTAMP_UNKNOWN), None);

        // A value that is not a real civil time must be rejected rather than
        // rendered.
        assert_eq!(unpack_civil(pack_civil(CivilTime { year: 2026, month: 13, day: 1, hour: 0, minute: 0 })), None);
        assert_eq!(unpack_civil(pack_civil(CivilTime { year: 2026, month: 10, day: 32, hour: 0, minute: 0 })), None);
        assert_eq!(unpack_civil(pack_civil(CivilTime { year: 2026, month: 10, day: 1, hour: 24, minute: 0 })), None);
        assert_eq!(unpack_civil(pack_civil(CivilTime { year: 2026, month: 10, day: 1, hour: 0, minute: 60 })), None);
        assert_eq!(unpack_civil(pack_civil(CivilTime { year: 1969, month: 10, day: 1, hour: 0, minute: 0 })), None);
    }

    #[test]
    fn test_days_from_civil_matches_known_epochs() {
        // 1970-01-01 is day zero.
        assert_eq!(days_from_civil(1970, 1, 1), 0);
        // 2000-01-01 is day 10957.
        assert_eq!(days_from_civil(2000, 1, 1), 10957);
        // A leap day is one day after the 28th of February.
        assert_eq!(
            days_from_civil(2024, 3, 1) - days_from_civil(2024, 2, 28),
            2
        );
    }

    #[test]
    fn test_timestamp_caption_reflects_the_real_clock_only() {
        let mut buf = [0u8; 32];

        // A note with no observed timestamp says so.
        let n = format_timestamp(TIMESTAMP_UNKNOWN, None, &mut buf);
        assert_eq!(&buf[..n], b"No timestamp");

        let t = CivilTime { year: 2026, month: 10, day: 18, hour: 16, minute: 45 };
        let packed = pack_civil(t);

        // Same calendar day as "now" -> a relative label with the clock time.
        let now = CivilTime { year: 2026, month: 10, day: 18, hour: 9, minute: 5 };
        let n = format_timestamp(packed, Some(now), &mut buf);
        assert_eq!(&buf[..n], b"Today 4:45 PM");

        // The calendar day before "now" -> "Yesterday".
        let tomorrow = CivilTime { year: 2026, month: 10, day: 19, hour: 9, minute: 5 };
        let n = format_timestamp(packed, Some(tomorrow), &mut buf);
        assert_eq!(&buf[..n], b"Yesterday 4:45 PM");

        // A day that is neither today nor yesterday -> the absolute civil date.
        let older = CivilTime { year: 2026, month: 10, day: 1, hour: 9, minute: 5 };
        let n = format_timestamp(packed, Some(older), &mut buf);
        assert_eq!(&buf[..n], b"18 Oct 2026");

        // A note stamped in the future is still an absolute date, never a
        // relative label.
        let n = format_timestamp(packed, Some(now), &mut buf);
        assert_eq!(&buf[..n], b"Today 4:45 PM");

        // Midnight and noon must not be confused with each other.
        let midnight = CivilTime { year: 2026, month: 10, day: 18, hour: 0, minute: 30 };
        let n = format_timestamp(pack_civil(midnight), Some(now), &mut buf);
        assert_eq!(&buf[..n], b"Today 12:30 AM");
        let noon = CivilTime { year: 2026, month: 10, day: 18, hour: 12, minute: 5 };
        let n = format_timestamp(pack_civil(noon), Some(now), &mut buf);
        assert_eq!(&buf[..n], b"Today 12:05 PM");
    }

    #[test]
    fn test_a_legacy_blob_loses_its_fabricated_timestamp() {
        // A version 2 blob carried epoch seconds written from a constant. The
        // reader must not present that value as an observed time.
        let mut store = NotesStore::new();
        store.create_note("Legacy", "Body");
        let mut buf = [0u8; 512];
        let n = store.serialize(&mut buf).expect("serialize");
        // Rewrite the header to the legacy version and stamp the slots with
        // the old fabricated constant.
        buf[0..6].copy_from_slice(&LEGACY_MAGIC_HEADER);
        let mut offset = 12usize;
        let id = 4u32.to_le_bytes();
        buf[offset..offset + 4].copy_from_slice(&id);
        offset += 4;
        let title_len = 6u16.to_le_bytes();
        buf[offset..offset + 2].copy_from_slice(&title_len);
        offset += 2;
        buf[offset..offset + 6].copy_from_slice(b"Legacy");
        offset += 6;
        let body_len = 4u16.to_le_bytes();
        buf[offset..offset + 2].copy_from_slice(&body_len);
        offset += 2;
        buf[offset..offset + 4].copy_from_slice(b"Body");
        offset += 4;
        buf[offset..offset + 8].copy_from_slice(&1729330000u64.to_le_bytes());
        offset += 8;
        buf[offset..offset + 8].copy_from_slice(&1729330000u64.to_le_bytes());
        offset += 8;
        // The trailing flag bytes are part of the checksummed region, so the
        // legacy record has to be written out in full before it is hashed.
        buf[offset] = 0; // is_dirty
        buf[offset + 1] = 0; // color_idx
        buf[offset + 2] = 0; // is_pinned
        buf[offset + 3] = 0; // category
        offset += 4;
        let checksum = fletcher32(&buf[12..offset]);
        buf[8..12].copy_from_slice(&checksum.to_le_bytes());

        let mut target = NotesStore::new();
        target.deserialize(&buf[..n]).expect("legacy blob should still load");
        assert_eq!(target.count(), 1);
        let note = target.get_by_index(0).expect("note");
        assert_eq!(note.as_title_str(), "Legacy");
        assert_eq!(note.created_at, TIMESTAMP_UNKNOWN);
        assert_eq!(note.updated_at, TIMESTAMP_UNKNOWN);
    }

    #[test]
    fn test_corrupted_data_rejection() {
        let mut store = NotesStore::new();
        store.create_note("Valid", "Data payload");

        let mut buf = [0u8; 2048];
        let bytes_written = store.serialize(&mut buf).expect("serialize");

        // 1. Bit flip in payload triggers ChecksumMismatch
        let mut corrupted_payload = buf;
        corrupted_payload[20] ^= 0xFF;
        let mut target = NotesStore::new();
        let res = target.deserialize(&corrupted_payload[..bytes_written]);
        assert_eq!(res, Err(StoreError::ChecksumMismatch));
        assert_eq!(target.count(), 0); // Unaltered

        // 2. Corrupt magic triggers InvalidMagic
        let mut corrupted_magic = buf;
        corrupted_magic[0] = b'X';
        let res = target.deserialize(&corrupted_magic[..bytes_written]);
        assert_eq!(res, Err(StoreError::InvalidMagic));

        // 3. Truncated buffer triggers UnexpectedEof
        let res = target.deserialize(&buf[..10]);
        assert_eq!(res, Err(StoreError::UnexpectedEof));

        // 4. Unsupported version
        let mut bad_ver = buf;
        bad_ver[4] = 99; // unsupported version
        let res = target.deserialize(&bad_ver[..bytes_written]);
        assert_eq!(res, Err(StoreError::UnsupportedVersion));
    }

    #[test]
    fn test_store_capacity_limits() {
        let mut store = NotesStore::new();
        for i in 0..MAX_NOTES {
            let res = store.create_note("Test Note", "Body content");
            assert!(res.is_some(), "Should allow creating note {}", i);
        }
        assert_eq!(store.count(), MAX_NOTES);

        // 17th note should fail
        let extra = store.create_note("Overflow", "Cannot fit");
        assert!(extra.is_none());
        assert_eq!(store.count(), MAX_NOTES);

        // Capacity exceeded on deserialization
        let mut fake_buf = [0u8; 32];
        fake_buf[0..6].copy_from_slice(&MAGIC_HEADER);
        fake_buf[6..8].copy_from_slice(&((MAX_NOTES as u16 + 1).to_le_bytes()));
        let mut target = NotesStore::new();
        assert_eq!(target.deserialize(&fake_buf), Err(StoreError::CapacityExceeded));
    }

    #[test]
    fn test_text_editor_up_down_and_bounds() {
        let scale = crate::theme::type_scale::BODY;
        // Wide enough that the three explicit lines never re-wrap.
        let budget = 632;
        let mut ed = TextEditorBuffer::new();
        ed.insert_str("Line 1\nLine 2 is longer\nLine 3");

        // Move to start of line 2
        ed.cursor = 7;
        assert_eq!(ed.cursor_line_col(scale, budget), (1, 0));

        // Move up to line 1
        ed.move_cursor_up(scale, budget);
        assert_eq!(ed.cursor_line_col(scale, budget), (0, 0));
        assert_eq!(ed.cursor, 0);

        // Move down to line 2
        ed.move_cursor_down(scale, budget);
        assert_eq!(ed.cursor_line_col(scale, budget), (1, 0));
        assert_eq!(ed.cursor, 7);

        // Move down to line 3
        ed.move_cursor_down(scale, budget);
        assert_eq!(ed.cursor_line_col(scale, budget), (2, 0));
        assert_eq!(ed.cursor, 24);

        // Cannot move down past end
        ed.move_cursor_down(scale, budget);
        assert_eq!(ed.cursor_line_col(scale, budget), (2, 0));

        // Start / End
        ed.move_cursor_to_start();
        assert_eq!(ed.cursor, 0);
        ed.move_cursor_to_end();
        assert_eq!(ed.cursor, ed.len);
    }

    #[test]
    fn test_checklist_support() {
        let mut ed = TextEditorBuffer::new();
        ed.insert_checklist_item();
        ed.insert_str("Buy milk");
        assert_eq!(ed.as_str(), "[ ] Buy milk");

        // Toggle to checked
        assert!(ed.toggle_checklist_at(0));
        assert_eq!(ed.as_str(), "[x] Buy milk");

        // Toggle back to unchecked
        assert!(ed.toggle_checklist_at(0));
        assert_eq!(ed.as_str(), "[ ] Buy milk");
    }

    #[test]
    fn test_typography_scale_2_body_line_metrics() {
        // WHY this matters: a body line that reserves less than the run drawn
        // inside it overlaps the next line, and on a note editor that is every
        // line of the document reading as one grey band.
        //
        // WHY the ladder's pixel bands are not repeated here: the rung sizes
        // are the SDK's, and they moved. An application that restates them
        // fails whenever the ladder is re-measured while its own layout, which
        // follows the ladder, was correct all along. What this screen needs is
        // that the box is larger than the cap and larger than the caption's
        // box, so the roles stay in order whatever the ladder is drawn at.
        let body_scale = crate::theme::type_scale::BODY;
        assert_eq!(body_scale, 2);
        let cap = std::design::font::cap_for_scale(body_scale);
        let lh = std::design::font::line_height(body_scale);
        assert!(
            lh > cap as usize,
            "a {cap}px body cap needs a line box taller than itself, got {lh}"
        );
        let caption_box = std::design::font::line_height(crate::theme::type_scale::CAPTION);
        assert!(
            lh > caption_box,
            "body lines sit {lh}px apart and captions {caption_box}px, so the roles read as one"
        );
    }

    #[test]
    fn test_scale_2_line_wrapping_at_column_limit() {
        use interos_std::design::font;
        let scale = crate::theme::type_scale::BODY;
        let mut ed = TextEditorBuffer::new();
        // A wide run of text wrapped at the real body column.
        ed.insert_str("The quick brown fox jumps over the lazy dog and runs across the sovereign field");
        let budget = 200;
        let mut spans = [LineSpan { start: 0, end: 0 }; 16];
        let count = ed.compute_lines(scale, budget, &mut spans);
        assert!(count >= 3);
        for span in &spans[..count] {
            assert!(font::measure(&ed.buf[span.start..span.end], scale) <= budget);
        }
    }

    #[test]
    fn test_cursor_navigation_left_right() {
        let mut ed = TextEditorBuffer::new();
        ed.insert_str("InterOS");
        assert_eq!(ed.cursor, 7);

        ed.move_cursor_left();
        assert_eq!(ed.cursor, 6);
        ed.move_cursor_left();
        assert_eq!(ed.cursor, 5);

        ed.move_cursor_right();
        assert_eq!(ed.cursor, 6);
        ed.move_cursor_right();
        assert_eq!(ed.cursor, 7);

        // Cannot move past end
        ed.move_cursor_right();
        assert_eq!(ed.cursor, 7);
    }

    #[test]
    fn test_notes_ui_title_and_preview_truncation() {
        use interos_std::design::font;
        let mut buf = [0u8; 96];
        let long_title = b"Very Long Sovereign Research Note Title That Exceeds Normal Card Width Limit";

        // WHY the lane is read from the library and not remembered here: the
        // painter ellipsizes to `LIST_CARD_TITLE_LANE[_PINNED]`, so a width
        // typed at this call site stops describing the card the moment the pin
        // glyph, the rail or a rung moves -- and the gate keeps passing while the
        // title on screen loses its tail.
        let pinned_w = LIST_CARD_TITLE_LANE_PINNED;
        let unpinned_w = LIST_CARD_TITLE_LANE;

        let len_pinned = font::ellipsize(long_title, 2, pinned_w, &mut buf);
        let disp_pinned = if len_pinned > 0 { &buf[..len_pinned] } else { long_title.as_slice() };
        assert!(font::measure(disp_pinned, 2) <= pinned_w);
        assert!(disp_pinned.ends_with(b"...") || disp_pinned == long_title.as_slice());

        let len_unpinned = font::ellipsize(long_title, 2, unpinned_w, &mut buf);
        let disp_unpinned = if len_unpinned > 0 { &buf[..len_unpinned] } else { long_title.as_slice() };
        assert!(font::measure(disp_unpinned, 2) <= unpinned_w);
        assert!(
            unpinned_w > pinned_w,
            "an unpinned title may not have less room than one that shares its row with the pin"
        );

        // WHY the bound is the key and not a remembered width: the action labels
        // sit on keys of four different widths -- shift, symbols, backspace and
        // the space bar -- and the painter centres each run inside its own key,
        // whose plate carves three columns off either side. One remembered 50
        // covered every label only while the caption rung drew narrow; when the
        // ladder raised it, "Space" passed 50 while still sitting comfortably in
        // the bar it is actually drawn on. So ask the table, not the number: a
        // label that would run off its own key fails here whatever the rung.
        use crate::keyboard::{KeyAction, LETTER_ROWS, SYMBOL_ROWS};
        // The columns the painted plate gives back on either side of the key.
        const KEY_PLATE_INSET: usize = 3;
        let widest_label = |action: KeyAction| -> &'static [&'static [u8]] {
            match action {
                // Both states, because either can be on screen when the key is
                // drawn and the key's width cannot change between them.
                KeyAction::Shift => &[b"^", b"ABC"],
                KeyAction::Symbols => &[b"?123", b"ABC"],
                KeyAction::Backspace => &[b"<X>"],
                KeyAction::Space => &[b"Space"],
                _ => &[],
            }
        };
        for rows in [LETTER_ROWS, SYMBOL_ROWS] {
            for row in rows {
                for key in row.keys {
                    for label in widest_label(key.action) {
                        let needed = font::measure(label, 1);
                        let plate = key.w.saturating_sub(2 * KEY_PLATE_INSET);
                        assert!(
                            needed <= plate,
                            "{label:?} needs {needed} px on a {} px key that paints {plate} px",
                            key.w
                        );
                    }
                }
            }
        }
    }

    /// The defect this covers was measured on the device: Notes painted no
    /// control at the platform's rect, so the header row it did paint answered
    /// the tap at (48, 80) as a search field and the person stayed on a screen
    /// with no way back.
    #[test]
    fn test_the_back_control_paints_the_canonical_rect() {
        use crate::chrome;
        use crate::theme::THEME;
        use interos_std::design::back_button;

        let cx = back_button::X + back_button::SIZE / 2;
        let cy = back_button::Y + back_button::SIZE / 2;

        // The chip's body and its ring are the theme's own roles, so a theme
        // that moves moves the control with it.
        assert_eq!(chrome::back_pixel(cx, cy), Some(THEME.surface_container_high));
        assert_eq!(chrome::back_pixel(back_button::X, cy), Some(THEME.outline));

        // The chevron is ink laid inside the chip, not the ring around it:
        // on the monochrome theme the ring and the ink are the same role, so
        // only a pixel away from the border can prove the mark is drawn.
        let mut chevron = 0usize;
        let mut body = 0usize;
        for y in back_button::Y + 1..back_button::Y + back_button::SIZE - 1 {
            for x in back_button::X + 1..back_button::X + back_button::SIZE - 1 {
                match chrome::back_pixel(x, y) {
                    Some(c) if c == THEME.on_surface => chevron += 1,
                    Some(c) if c == THEME.surface_container_high => body += 1,
                    _ => {}
                }
            }
        }
        assert!(chevron > 0, "the back control paints no chevron in the notes header");
        assert!(body > 0, "the back control paints a mark with no chip behind it");

        // Nothing is painted outside the square the control owns.
        assert!(chrome::back_pixel(back_button::X - 1, cy).is_none());
        assert!(chrome::back_pixel(back_button::X + back_button::SIZE, cy).is_none());
        assert!(chrome::back_pixel(cx, back_button::Y - 1).is_none());
        assert!(chrome::back_pixel(cx, back_button::Y + back_button::SIZE).is_none());
    }

    /// The painter and the tap handler answer one square, and the label stands
    /// outside it.
    #[test]
    fn test_the_back_control_claims_every_pixel_it_paints() {
        use crate::chrome;
        use interos_std::design::back_button;

        // (48, 80) is the guest pixel the device reported the tap at.
        assert!(chrome::back_claims(48, 80));

        for y in 0..back_button::Y + back_button::SIZE + 8 {
            for x in 0..back_button::X + back_button::SIZE + 8 {
                if chrome::back_pixel(x, y).is_some() {
                    assert!(
                        chrome::back_claims(x, y),
                        "the header paints ({x}, {y}) with no control to press there"
                    );
                }
            }
        }

        // Outside the square, the row underneath keeps its taps: the search
        // pill and the toolbar run under and beside the control.
        assert!(!chrome::back_claims(back_button::X - 1, back_button::Y));
        assert!(!chrome::back_claims(
            back_button::X + back_button::SIZE,
            back_button::Y + back_button::SIZE - 1
        ));
        assert!(!chrome::back_claims(48, back_button::Y + back_button::SIZE));

        // The header's run starts clear of the chip, or the app would draw a
        // label under the control and hit-test it first.
        assert!(chrome::LABEL_X >= back_button::X + back_button::SIZE);
    }

    /// Every circular tool this app draws stands at the platform's floor, and
    /// the press answers the same disc.
    ///
    /// WHY this is pinned: the renderer and the press each carried their own
    /// radius -- the editor's toolbar was drawn at twenty and answered at
    /// twenty-two, the reading dock's tools at twenty against twenty-four, the
    /// floating action button at twenty-eight against thirty-four, and the
    /// swatch picker at twenty-two against twenty-six. A thumb lands on the
    /// ink, so the ink is what has to clear the floor.
    #[test]
    fn test_every_circular_tool_is_inked_at_the_disc_its_press_answers() {
        use interos_std::design::touch::MIN_TARGET;
        assert_eq!(chrome::TOOL_R * 2, MIN_TARGET);
        assert!(chrome::FAB_R * 2 >= MIN_TARGET);
        for cx in chrome::EDITOR_TOOL_CXS {
            assert!(cx + chrome::TOOL_R <= 720);
            assert!(chrome::EDITOR_TOOL_CY >= chrome::TOOL_R);
        }
        for cx in chrome::READING_TOOL_CXS {
            let disc_top = chrome::READING_TOOL_CY - chrome::TOOL_R;
            let disc_bottom = chrome::READING_TOOL_CY + chrome::TOOL_R;
            assert!(
                disc_top >= chrome::READING_DOCK.y && disc_bottom <= chrome::READING_DOCK.y + chrome::READING_DOCK.h,
                "the reading dock's tool at x={cx} is inked outside the plate it stands on"
            );
        }
    }

    /// The home strip answers the rows the pill inks, and never reaches into
    /// the soft keyboard.
    #[test]
    fn test_home_strip_covers_its_pill_and_stops_below_the_keyboard() {
        assert!(
            chrome::HOME_HIT.y >= crate::keyboard::KB_Y1,
            "the home strip starts at row {}, inside the keyboard's {}..{}",
            chrome::HOME_HIT.y,
            crate::keyboard::KB_Y0,
            crate::keyboard::KB_Y1
        );
        for y in chrome::HOME_PILL.y..chrome::HOME_PILL.y + chrome::HOME_PILL.h {
            for x in chrome::HOME_PILL.x..chrome::HOME_PILL.x + chrome::HOME_PILL.w {
                assert!(chrome::home_claims(x, y), "the pill inked ({x}, {y}) that no press answers");
            }
        }
        assert!(chrome::home_claims(360, 1420));
        assert!(!chrome::home_claims(360, crate::keyboard::KB_Y1 - 1));
        assert!(!chrome::home_claims(200, 1439));
    }

    /// The dock's three tool bands are each centred on the glyph they name,
    /// and run to one another with no dead column between.
    #[test]
    fn test_dock_tool_bands_are_centred_on_the_glyphs_they_answer() {
        use interos_std::design::touch::MIN_TARGET;
        let mid_y = chrome::DOCK_PILL.y + chrome::DOCK_PILL.h / 2;
        assert!(chrome::DOCK_TOOL_W >= MIN_TARGET);
        for (i, cx) in chrome::DOCK_TOOL_CXS.iter().enumerate() {
            assert_eq!(
                chrome::dock_tool_at(*cx, mid_y),
                Some(i),
                "the glyph at x={cx} answers as tool {:?}",
                chrome::dock_tool_at(*cx, mid_y)
            );
            assert_eq!(chrome::dock_tool_at(cx - chrome::DOCK_TOOL_W / 2, mid_y), Some(i));
            assert_eq!(chrome::dock_tool_at(cx + chrome::DOCK_TOOL_W / 2 - 1, mid_y), Some(i));
            assert_ne!(chrome::dock_tool_at(cx + chrome::DOCK_TOOL_W / 2, mid_y), Some(i));
        }
        // The bands touch: no column of the pill's tool run answers nothing.
        for pair in chrome::DOCK_TOOL_CXS.windows(2) {
            let seam = pair[0] + chrome::DOCK_TOOL_W / 2;
            assert_eq!(seam, pair[1] - chrome::DOCK_TOOL_W / 2);
            assert!(chrome::dock_tool_at(seam, mid_y).is_some());
        }
        // The pill's own head still reads as the note it advertises.
        assert_eq!(chrome::dock_tool_at(chrome::DOCK_PILL.x + 4, mid_y), None);
    }

    /// The search pill's clear control is a plate the press answers whole,
    /// inside the pill that carries it.
    #[test]
    fn test_search_clear_answers_the_plate_the_painter_inks() {
        use interos_std::design::touch::MIN_TARGET;
        assert!(chrome::SEARCH_CLEAR.w >= MIN_TARGET && chrome::SEARCH_CLEAR.h >= MIN_TARGET);
        assert!(chrome::SEARCH_CLEAR.x >= chrome::SEARCH_PILL.x);
        assert!(chrome::SEARCH_CLEAR.y >= chrome::SEARCH_PILL.y);
        assert!(chrome::SEARCH_CLEAR.x + chrome::SEARCH_CLEAR.w <= chrome::SEARCH_PILL.x + chrome::SEARCH_PILL.w);
        assert!(chrome::SEARCH_CLEAR.y + chrome::SEARCH_CLEAR.h <= chrome::SEARCH_PILL.y + chrome::SEARCH_PILL.h);
        let mid_y = chrome::SEARCH_CLEAR.y + chrome::SEARCH_CLEAR.h / 2;
        for x in 0..720usize {
            assert_eq!(
                chrome::search_clear_claims(x, mid_y),
                chrome::SEARCH_CLEAR.contains(x, mid_y),
                "the clear press disagreed with the clear plate at x={x}"
            );
        }
    }

    /// The reading dock's category tag is drawn on the plate the dock owns,
    /// at the platform's floor.
    #[test]
    fn test_reading_tag_is_a_plate_the_dock_draws() {
        use interos_std::design::touch::MIN_TARGET;
        assert!(chrome::READING_TAG.w >= MIN_TARGET && chrome::READING_TAG.h >= MIN_TARGET);
        assert!(chrome::READING_TAG.x >= chrome::READING_DOCK.x);
        assert!(chrome::READING_TAG.y >= chrome::READING_DOCK.y);
        assert!(chrome::READING_TAG.x + chrome::READING_TAG.w <= chrome::READING_DOCK.x + chrome::READING_DOCK.w);
        assert!(chrome::READING_TAG.y + chrome::READING_TAG.h <= chrome::READING_DOCK.y + chrome::READING_DOCK.h);
    }

    /// The keyboard's accessory strip answers exactly the plates it inks, and
    /// every plate clears the platform's floor on both of its edges.
    ///
    /// WHY this is pinned: the strip painted each button as a rounded rect of
    /// `x, 920, w, 48` and pressed it as `ux >= x && ux <= x + w`, so every
    /// plate's press reached past its own ink -- one column for the checklist,
    /// three for each cursor step -- and the strip's band ran to row 974 while
    /// its container ended its ink at 973. A press in the last column of a
    /// button answered a key the person was not looking at, and a press on the
    /// page below the strip pressed a key at all.
    #[test]
    fn the_accessory_strip_answers_exactly_the_plates_it_inks() {
        use interos_std::design::touch::MIN_TARGET;
        for (i, plate) in chrome::STRIP_PLATES.iter().enumerate() {
            assert!(
                plate.w >= MIN_TARGET && plate.h >= MIN_TARGET,
                "strip plate {i} is {}x{}, under the {} floor",
                plate.w,
                plate.h,
                MIN_TARGET
            );
            // Every pixel of the plate answers, and it answers as itself.
            for y in plate.y..plate.y + plate.h {
                for x in plate.x..plate.x + plate.w {
                    assert_eq!(
                        chrome::strip_plate_at(x, y),
                        Some(i),
                        "plate {i} inked ({x},{y}) that answers {:?}",
                        chrome::strip_plate_at(x, y)
                    );
                }
            }
            // The plate stands inside the container the strip inks.
            assert!(plate.x >= chrome::STRIP.x && plate.y >= chrome::STRIP.y);
            assert!(plate.x + plate.w <= chrome::STRIP.x + chrome::STRIP.w);
            assert!(plate.y + plate.h <= chrome::STRIP.y + chrome::STRIP.h);
        }
        // One column outside a plate answers nothing: the gap between two
        // buttons is the strip's own surface, not a third key.
        for pair in chrome::STRIP_PLATES.windows(2) {
            let seam = (pair[0].x + pair[0].w + pair[1].x) / 2;
            let mid_y = chrome::STRIP_BTN_Y + chrome::STRIP_BTN_H / 2;
            assert_eq!(chrome::strip_plate_at(seam, mid_y), None);
            assert_eq!(chrome::strip_plate_at(pair[0].x + pair[0].w, mid_y), None);
            assert_eq!(chrome::strip_plate_at(pair[0].x - 1, mid_y), None);
        }
        // The band the strip owns stops at the keyboard's first row.
        assert!(chrome::STRIP.y + chrome::STRIP.h <= crate::keyboard::KB_Y0);
        assert_eq!(chrome::STRIP.h, 58);
    }

    /// The tone picker answers a floor-sized disc on the band the popover
    /// paints, in both of the rows the keyboard can leave it in.
    ///
    /// WHY this is pinned: the popover's band and its swatch pitch were written
    /// out twice, once in the renderer and once in the press, as the same `if
    /// keyboard_visible { 852 } else { 1248 }` expression. Two copies agreed
    /// today and nothing held them together.
    #[test]
    fn the_tone_picker_answers_the_band_the_popover_paints() {
        use interos_std::design::touch::MIN_TARGET;
        for keyboard_visible in [false, true] {
            let band = chrome::swatch_band_at(keyboard_visible);
            assert!(
                band.h >= MIN_TARGET,
                "the picker's band is {} rows, under the {} floor",
                band.h,
                MIN_TARGET
            );
            assert!(chrome::SWATCH_R * 2 >= MIN_TARGET);
            let cy = chrome::swatch_cy(band);
            for i in 0..NOTE_TONE_COUNT {
                let cx = chrome::SWATCH_X0 + i * chrome::SWATCH_PITCH;
                // The swatch's own plate answers, edge to edge of the disc.
                assert_eq!(chrome::swatch_at(cx, cy, keyboard_visible), Some(i));
                assert_eq!(chrome::swatch_at(cx - chrome::SWATCH_R, cy, keyboard_visible), Some(i));
                assert_eq!(chrome::swatch_at(cx + chrome::SWATCH_R - 1, cy, keyboard_visible), Some(i));
                assert_eq!(chrome::swatch_at(cx + chrome::SWATCH_R + 1, cy, keyboard_visible), None);
                // Every swatch stands inside the band the popover inks.
                assert!(cx - chrome::SWATCH_R >= band.x);
                assert!(cx + chrome::SWATCH_R <= band.x + band.w);
                assert!(cy - chrome::SWATCH_R >= band.y);
                assert!(cy + chrome::SWATCH_R <= band.y + band.h);
            }
            // The row between two swatches is the popover's surface, not a key.
            let seam = chrome::SWATCH_X0 + chrome::SWATCH_PITCH / 2;
            assert_eq!(chrome::swatch_at(seam, cy, keyboard_visible), None);
        }
        // Neither band reaches into the chrome it sits above.
        assert!(chrome::swatch_band_at(true).y + chrome::swatch_band_at(true).h <= chrome::STRIP.y);
        assert!(
            chrome::swatch_band_at(false).y + chrome::swatch_band_at(false).h
                <= chrome::READING_DOCK.y
        );
    }
}

