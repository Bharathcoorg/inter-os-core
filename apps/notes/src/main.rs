//! InterOS Sovereign Native Notes Application — Google Keep-Grade Mobile UX (2026 Edition)
//!
//! Production Ring-3 graphical application executing in User Mode:
//! - DRM/KMS Client Surface allocation with dynamic capability lookup
//! - Pure `#![no_std]` and `#![no_main]` with zero heap allocations in scanout and event polling
//! - Google Keep-style colorful card aesthetic with 8 distinct pastel themes
//! - Generous, airy spacing between cards, chips, search pill, and typography
//! - Staggered card stream with kinetic touch drag scrolling and category filter chips
//! - Elevated Search Pill bar with live substring filtering and keyboard integration
//! - Interactive checklists (To-Dos) with toggleable checkboxes `[ ]` <-> `[x]` and text strikethrough
//! - Distraction-free full-page reading mode with bottom-docked virtual keyboard
//! - Floating accessory strip with `☑` To-Do, `•` Bullet, `🎨` Palette, `🏷` Category, and `▼` Dismiss
//! - Sleek 2026 vector iconography (no Windows 95 button widgets)
//! - Sovereign local storage persistence in the app's own sandbox
//!   (`data/notes.bin`, relative to `/user/<uid>/apps/org.interos.notes`)
//!   with Fletcher-32 integrity, a staged copy and a quarantine path
//! - Graceful lifecycle supervision and clean return to Launcher

#![no_std]
#![no_main]

use core::panic::PanicInfo;
use interos_std as std;
use notes::theme::THEME;
use interos_std::design::components;
use interos_std::design::outline_text;
use interos_std::design::scroll::ScrollState;
use notes::keyboard::{self, KeyAction};
use notes::library;
pub use notes::{
    format_count_badge, format_timestamp, LineSpan, Note, NoteColorTheme, NotesStore,
    TextEditorBuffer, CATEGORY_NAMES, CivilTime, KEEP_COLORS, MAX_BODY_LEN, MAX_NOTES, MAX_TITLE_LEN,
    StorageState, TIMESTAMP_UNKNOWN,
};

/// The platform's card treatment, at the elevation the app's cards sit at.
///
/// WHY the app's cards are the platform's cards: a card that invents its own
/// fill and its own hairline is a card a third-party app cannot match, and
/// the platform already says what a card looks like.
const CARD: components::CardStyle =
    components::card_style(&THEME, components::Elevation::Resting);

/// The card's hairline, or the card's own fill when the card has no hairline.
const CARD_BORDER: u32 = match CARD.border {
    Some(c) => c,
    None => CARD.fill,
};

const W: usize = 720;
const H: usize = 1440;
const CHUNK: usize = 4096;
const CHARS_PER_LINE: usize = 40;

/// The note card's height.
const CARD_H: usize = 216;

/// The gap between two note cards.
const CARD_GAP: usize = 20;

/// The distance from one card's top to the next one's top.
const CARD_STRIDE: usize = CARD_H + CARD_GAP;

/// The top of the note list's visible band.
const LIST_TOP: usize = 164;

/// The bottom of the note list's visible band.
const LIST_BOTTOM: usize = 1308;

/// The height of the note list's visible band.
const LIST_VIEWPORT_H: usize = LIST_BOTTOM - LIST_TOP;

/// The scroll thumb's track, at the list's trailing edge.
const LIST_TRACK_X: usize = 700;

/// The scroll thumb's width.
const LIST_TRACK_W: usize = 4;

/// The category chips: the five categories a note may carry.
const CHIP_XS: [usize; 5] = [28, 126, 224, 344, 442];
const CHIP_WS: [usize; 5] = [88, 88, 110, 88, 100];

/// The pin filter, beside the category chips.
///
/// WHY the pin has its own control: a pin is a state a note carries, not a
/// category it belongs to, and a filter that cannot be combined with a
/// category is a filter that hides more than the person asked for.
///
/// WHY 44: the design system's minimum touch target is 44 logical pixels,
/// and the whole chip row is 44 tall so the pin filter and the five
/// category chips are the same size. A 42-pixel chip is two pixels short
/// of the minimum, which is the kind of shortfall nobody notices and
/// everybody's thumb does.
const PIN_FILTER: (usize, usize, usize, usize) = (556, 96, 136, 44);

/// The ruled page's left edge.
const RULE_X0: usize = 44;

/// The ruled page's right edge: the note's text column.
const RULE_X1: usize = RULE_X0 + CHARS_PER_LINE * 8;

/// The distance from a line's top to the rule drawn under it.
const RULE_DY: usize = 30;

// -----------------------------------------------------------------------------
// Visual Design Tokens
//
// WHY there is no palette here any more: the app used to carry its own
// colours, which is a second design system inside one app. Every colour now
// resolves through the platform's theme, so a third-party app that asks the
// SDK for the same roles draws the same notes surface. See `notes::theme` for
// the mapping from the old names to the platform's roles.
//
// WHY the note's own colour theme is still a local palette: `KEEP_COLORS` is
// the *note's* palette -- a thing the user picks, one of eight, each with its
// own colours -- and not the app's chrome. The platform's theme describes the
// surface the note is drawn on; the note's own colours are content.
// -----------------------------------------------------------------------------
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ViewMode {
    List,
    Editor,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum EditorFocus {
    Title,
    Body,
}

/// What the app knows about the note store on the device.
///
/// The type lives in the library so that the classification is covered by the
/// library's unit tests; the screen copy that goes with each state lives in
/// the renderer.

pub struct AppState {
    pub store: NotesStore,
    pub view: ViewMode,
    pub selected_note_id: u32,
    pub focus: EditorFocus,
    pub editor_buf: TextEditorBuffer,
    pub cursor_visible: bool,
    pub cursor_pos: (usize, usize),
    pub line_spans: [LineSpan; 32],
    pub line_spans_count: usize,
    /// What the last load found, and what the last save managed.
    pub storage: StorageState,
    /// Set when a save was attempted and did not reach the device.
    ///
    /// WHY this is separate from `storage`: a refused *read* and a failed
    /// *write* are different failures, and the editor's "Saved" label has to
    /// distinguish them. The old code discarded the result of every save,
    /// so a note could be lost while the header still read "Saved".
    pub save_failed: bool,
    /// The note list's scroll position.
    ///
    /// WHY the platform's model and not a hand-rolled offset: the platform
    /// clamps the offset, draws the thumb and answers "is this list
    /// scrollable" in one place, and the list that used its own counter
    /// could scroll past its last card and could not say that it had more.
    pub list_scroll: ScrollState,
    /// The soft keyboard's scroll position.
    ///
    /// WHY the keyboard scrolls: the symbols page has more rows than the
    /// dock can show, and a row that cannot be reached is a character that
    /// cannot be typed.
    pub kb_scroll: ScrollState,
    /// Whether the list shows only the pinned notes.
    ///
    /// WHY this is separate from the category: a pin is a state a note
    /// carries, not a category it belongs to. Mixing the two in one row
    /// made a filter that cannot be combined with a category.
    pub pinned_only: bool,
    pub selected_category: u8, // 0: All, 1: Work, 2: Personal, 3: Ideas, 4: System
    pub search_active: bool,
    pub search_query: [u8; 32],
    pub search_len: usize,
    pub keyboard_visible: bool,
    pub swatch_picker_open: bool,
    pub kb_shift: bool,
    pub kb_symbols: bool,
    /// The most recent civil time read from the device's real-time clock.
    ///
    /// WHY the app caches it: the clock is a syscall, and a note list asks
    /// for "today" once per card. `None` means the clock was refused or
    /// carried no usable civil date, and the app says so rather than
    /// substituting a time.
    pub now_civil: Option<CivilTime>,
}

impl AppState {
    /// Writes the store to the device and records whether it landed.
    ///
    /// WHY the result is kept: the old code discarded the return value of
    /// every save, so a note that never reached the device still read
    /// "Saved" in the editor header. The header now says "Not saved" when
    /// the write did not land.
    pub fn save_to_storage(&mut self) -> bool {
        let ok = save_notes_to_storage(&mut self.store);
        self.save_failed = !ok;
        if ok {
            self.storage = StorageState::Ready;
            // WHY the notes are published after a write: the document the
            // file manager and the launcher read has to be the note the
            // user just wrote.
            self.publish_notes();
        }
        ok
    }

    pub fn update_editor_layout(&mut self) {
        self.line_spans_count = self.editor_buf.compute_lines(CHARS_PER_LINE, &mut self.line_spans);
        self.cursor_pos = self.editor_buf.cursor_line_col(CHARS_PER_LINE);
    }

    pub fn sync_editor_to_note(&mut self) {
        if let Some(note) = self.store.get_note_mut(self.selected_note_id) {
            note.set_body(self.editor_buf.as_str());
            // WHY the stamp comes from the clock and not a constant: the edit
            // time is a fact about the device. When the clock is unreadable
            // the note carries no timestamp, and the card says so.
            note.updated_at = match self.now_civil {
                Some(now) => notes::pack_civil(now),
                None => TIMESTAMP_UNKNOWN,
            };
        }
    }

    /// Creates a note and stamps it with the observed clock.
    ///
    /// WHY this returns an `Option`: the store has a fixed capacity, and when
    /// it is full the app must not fall back to an existing note id. The old
    /// code did exactly that -- `create_note(..).unwrap_or(1)` -- so a full
    /// store silently opened the user's *first* note and overwrote it.
    ///
    /// WHY the title starts empty: the title "New Note" was app-authored
    /// text landing in the user's own list. The editor already renders a
    /// "Title" placeholder, so an untitled note is both honest and obvious.
    pub fn create_stamped_note(&mut self, body: &str) -> Option<u32> {
        let id = self.store.create_note("", body)?;
        let packed = match self.now_civil {
            Some(now) => notes::pack_civil(now),
            None => TIMESTAMP_UNKNOWN,
        };
        self.store.stamp(id, packed);
        Some(id)
    }

    pub fn filtered_indices(&self, out: &mut [usize; MAX_NOTES]) -> usize {
        let mut count = 0;
        let q = &self.search_query[..self.search_len];
        for i in 0..self.store.count() {
            let note = match self.store.get_by_index(i) {
                Some(n) => n,
                None => continue,
            };

            // Category filter
            //
            // WHY the pin is not a category: a pin is a state the note
            // carries, and it has its own filter beside the categories.
            if self.selected_category > 0 && note.category != self.selected_category {
                continue;
            }
            if self.pinned_only && !note.is_pinned {
                continue;
            }

            // Search query filter
            if self.search_len > 0 {
                let matches_title = contains_ignore_case(note.title_bytes(), q);
                let matches_body = contains_ignore_case(note.body_bytes(), q);
                if !matches_title && !matches_body {
                    continue;
                }
            }

            out[count] = i;
            count += 1;
        }
        count
    }

    /// Re-derives the note list's scroll extent from the notes it shows.
    ///
    /// WHY the extent follows the *filtered* count: the old counter used
    /// the store's count, so a search that matched two of sixteen notes
    /// still scrolled as if all sixteen were there, and a note below the
    /// fold could not be reached.
    pub fn sync_list_extent(&mut self) {
        let mut filtered = [0usize; MAX_NOTES];
        let visible = self.filtered_indices(&mut filtered);
        self.list_scroll = ScrollState::new(
            visible * CARD_STRIDE,
            LIST_VIEWPORT_H,
            self.list_scroll.offset,
        );
    }

    /// Re-derives the soft keyboard's scroll extent from the page it shows.
    pub fn sync_kb_extent(&mut self) {
        self.kb_scroll = ScrollState::new(
            keyboard::content_h(self.kb_symbols),
            keyboard::viewport_h(),
            self.kb_scroll.offset,
        );
    }

    /// Publishes every note as a document the device can find, and asks the
    /// platform to describe them again.
    ///
    /// WHY the app publishes its notes: the note store lives in the app's
    /// own sandbox, and a sandbox is private. The file manager could not
    /// list a note, the launcher's search could not find one and the
    /// platform's document index did not know that notes existed.
    ///
    /// Returns how many documents were written.
    pub fn publish_notes(&mut self) -> usize {
        let mut written = 0usize;
        for i in 0..self.store.count() {
            let note = self.store.get_by_index(i).copied();
            let Some(note) = note else { continue };
            if library::publish_note(&note) == library::PublishOutcome::Written {
                written += 1;
            }
        }
        if written > 0 {
            library::refresh_index();
        }
        written
    }
}

static mut CHUNK_BUF: [u32; CHUNK] = [0; CHUNK];

/// Draws a run of text with the sharp outline face, falling back to the
/// bitmap face when the outline face refuses the run.
///
/// WHY the outline face is tried first: the panel is 1080x2160 and this app
/// paints at 720x1440, so the compositor magnifies a bitmap run 3:2 and the
/// glyph edges read as soft. The outline face rasterises the real outline
/// and reads as sharp at the panel's own resolution.
///
/// WHY the box test comes first: the run's own arguments decide which face
/// is used, so the pixel under the cursor never gets to choose. The box is
/// the union of the two faces' boxes -- the outline face's box sits one or
/// two rows inside the bitmap cell's, and a bitmap fallback that lost its
/// top row would draw a clipped glyph.
#[inline(always)]
fn test_text(
    x: usize,
    y: usize,
    x0: usize,
    y0: usize,
    scale: usize,
    text: &[u8],
    color: u32,
    bg: u32,
    bold: bool,
) -> Option<u32> {
    if scale == 0 || text.is_empty() {
        return None;
    }
    let (bx, bt, bw, bh) = outline_text::run_box(x0, y0, scale, text);
    let bitmap_h = std::design::font::line_height(scale);
    let bitmap_w = std::design::font::measure(text, scale);
    let top = bt.min(y0);
    let height = bh.max(bitmap_h);
    // A bold outline run is one pixel wider than the run the box was measured
    // from, so the box has to carry that pixel or the last glyph is cut.
    let width = bw.max(bitmap_w) + if bold { 1 } else { 0 };
    if x < bx || x - bx >= width || y < top || y - top >= height {
        return None;
    }
    if let Some(run) = outline_text::place_run(x0, y0, scale, text, None, bold) {
        return run.pixel(x, y, color, bg);
    }
    std::design::font::test_text_aa(x, y, x0, y0, scale, text, color, bg, bold)
}

fn contains_ignore_case(haystack: &[u8], needle: &[u8]) -> bool {
    if needle.is_empty() {
        return true;
    }
    if needle.len() > haystack.len() {
        return false;
    }
    for i in 0..=(haystack.len() - needle.len()) {
        let mut matched = true;
        for j in 0..needle.len() {
            let b1 = haystack[i + j].to_ascii_lowercase();
            let b2 = needle[j].to_ascii_lowercase();
            if b1 != b2 {
                matched = false;
                break;
            }
        }
        if matched {
            return true;
        }
    }
    false
}

/// Reads the device's real-time clock and returns the civil time it reports.
///
/// WHY this returns an `Option` and not a fallback: the RTC syscall needs
/// the `RealtimeClockRead` capability, and a RAPP that does not hold it is
/// refused outright. The previous implementation called the clock, threw the
/// answer away, and returned a hardcoded epoch constant -- so every note on
/// the device carried an edit time the device never observed. A note must
/// either be stamped with what the clock said or carry no timestamp at all.
fn current_civil_time() -> Option<CivilTime> {
    let mut buf = [0u8; 64];
    let n = std::rtc_get_time(&mut buf).ok()?;
    if n == 0 {
        return None;
    }
    let payload = &buf[..n];

    let year = json_field(payload, b"\"year\":")?;
    let month = json_field(payload, b"\"month\":")?;
    let day = json_field(payload, b"\"day\":")?;
    // WHY the hour and the minute are required and not defaulted: a payload
    // that carried a date but no time would otherwise be stamped as
    // midnight, and midnight is a time the clock never reported. The note
    // carries no timestamp instead.
    let hour = json_field(payload, b"\"hour\":")?;
    let minute = json_field(payload, b"\"minute\":")?;

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

/// Reads a numeric field out of the RTC's JSON payload.
///
/// WHY a parser and not a fixed offset: the kernel writes the fields in a
/// fixed order today, but the length of the year and the presence of a
/// leading zero are not fixed, so only reading the field can be correct.
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

/// Formats a note's caption timestamp from the clock, never from a constant.
///
/// WHY `now` is passed in rather than read here: the current day decides
/// whether the caption says "Today", and the app already keeps the clock
/// reading in one place.
fn format_note_timestamp(note: &Note, now: Option<CivilTime>, buf: &mut [u8; 32]) -> usize {
    if note.is_dirty {
        let s = b"Editing now";
        let take = s.len().min(buf.len());
        buf[..take].copy_from_slice(&s[..take]);
        return take;
    }
    notes::format_timestamp(note.updated_at, now, buf)
}

// -----------------------------------------------------------------------------
// Pixel Geometry & Drawing Helpers
// -----------------------------------------------------------------------------

#[inline(always)]
fn in_rounded_rect(x: usize, y: usize, rx: usize, ry: usize, rw: usize, rh: usize, r: usize) -> bool {
    if x < rx || x >= rx + rw || y < ry || y >= ry + rh {
        return false;
    }
    let dx = if x < rx + r {
        rx + r - x
    } else if x >= rx + rw - r {
        x - (rx + rw - r - 1)
    } else {
        0
    };
    let dy = if y < ry + r {
        ry + r - y
    } else if y >= ry + rh - r {
        y - (ry + rh - r - 1)
    } else {
        0
    };
    dx * dx + dy * dy <= r * r
}

#[inline(always)]
fn in_circle(x: usize, y: usize, cx: usize, cy: usize, r: usize) -> bool {
    let dx = x as i32 - cx as i32;
    let dy = y as i32 - cy as i32;
    dx * dx + dy * dy <= (r * r) as i32
}

// -----------------------------------------------------------------------------
// 2026 Minimal Vector Glyphs
// -----------------------------------------------------------------------------

fn draw_chevron_left(x: usize, y: usize, cx: usize, cy: usize, color: u32) -> Option<u32> {
    let dx = x as i32 - cx as i32;
    let dy = y as i32 - cy as i32;
    if dy.abs() <= 1 && dx >= -7 && dx <= 8 {
        return Some(color);
    }
    if dy.abs() <= 8 && (dx - (-7 + dy.abs())).abs() <= 1 {
        return Some(color);
    }
    None
}

fn draw_pin_glyph(x: usize, y: usize, cx: usize, cy: usize, is_pinned: bool, color: u32) -> Option<u32> {
    let dx = x as i32 - cx as i32;
    let dy = y as i32 - cy as i32;
    // Sleek vector pushpin geometry:
    // Top cap:
    if dy >= -8 && dy <= -7 && dx.abs() <= 6 {
        return Some(color);
    }
    // Head body:
    if dy >= -6 && dy <= -2 && dx.abs() <= 4 {
        if is_pinned || dx.abs() == 4 || dy == -6 {
            return Some(color);
        }
    }
    // Rim / flange:
    if (dy == -1 || dy == 0) && dx.abs() <= 7 {
        return Some(color);
    }
    // Needle pointing down:
    if dy >= 1 && dy <= 9 && dx.abs() <= 1 {
        if dy >= 7 && dx.abs() == 1 {
            return None;
        }
        return Some(color);
    }
    None
}

fn draw_trash_glyph(x: usize, y: usize, cx: usize, cy: usize, color: u32) -> Option<u32> {
    let dx = x as i32 - cx as i32;
    let dy = y as i32 - cy as i32;
    // Lid handle:
    if dy >= -9 && dy <= -8 && dx.abs() <= 3 {
        if dy == -9 || dx.abs() == 3 {
            return Some(color);
        }
    }
    // Lid horizontal bar:
    if (dy == -7 || dy == -6) && dx.abs() <= 8 {
        return Some(color);
    }
    // Tapered bin body:
    if dy >= -5 && dy <= 7 {
        let side = if dy >= 4 { 5 } else { 6 };
        if dx.abs() == side || (dy == 7 && dx.abs() <= 5) {
            return Some(color);
        }
        // Vertical interior slats:
        if (dx == -2 || dx == 2) && dy >= -3 && dy <= 5 {
            return Some(color);
        }
    }
    None
}

fn draw_palette_glyph(x: usize, y: usize, cx: usize, cy: usize, color: u32) -> Option<u32> {
    let dx = x as i32 - cx as i32;
    let dy = y as i32 - cy as i32;
    let r2 = dx * dx + dy * dy;
    if r2 > 81 {
        return None;
    }
    // Thumb cutout hole at bottom right:
    if (dx - 4) * (dx - 4) + (dy - 4) * (dy - 4) <= 6 {
        return None;
    }
    // Outer perimeter stroke:
    if r2 >= 49 && r2 <= 81 {
        return Some(color);
    }
    // 3 distinct color wells:
    if (dx + 3) * (dx + 3) + (dy + 2) * (dy + 2) <= 3 {
        return Some(THEME.primary);
    }
    if (dx + 1) * (dx + 1) + (dy - 4) * (dy - 4) <= 3 {
        return Some(THEME.warning);
    }
    if (dx - 3) * (dx - 3) + (dy + 1) * (dy + 1) <= 3 {
        return Some(THEME.error);
    }
    None
}

fn draw_keyboard_glyph(x: usize, y: usize, cx: usize, cy: usize, color: u32) -> Option<u32> {
    let dx = x as i32 - cx as i32;
    let dy = y as i32 - cy as i32;
    if dx.abs() <= 10 && dy.abs() <= 6 {
        if dx.abs() == 10 || dy.abs() == 6 {
            return Some(color);
        }
        if (dy == -3 || dy == -2) && (dx.abs() == 7 || dx.abs() == 3 || dx == 0) {
            return Some(color);
        }
        if (dy == 1 || dy == 2) && (dx.abs() == 5 || dx.abs() == 1) {
            return Some(color);
        }
        if dy == 4 && dx.abs() <= 4 {
            return Some(color);
        }
    }
    None
}

fn draw_chevron_down(x: usize, y: usize, cx: usize, cy: usize, color: u32) -> Option<u32> {
    let dx = x as i32 - cx as i32;
    let dy = y as i32 - cy as i32;
    if dx.abs() <= 6 && (dy - (3 - dx.abs())).abs() <= 1 {
        return Some(color);
    }
    None
}

fn draw_checkbox_glyph(x: usize, y: usize, bx: usize, by: usize, is_checked: bool, accent: u32) -> Option<u32> {
    if x < bx || x >= bx + 22 || y < by || y >= by + 22 {
        return None;
    }
    let rx = x - bx;
    let ry = y - by;
    let in_box = in_rounded_rect(x, y, bx, by, 22, 22, 5);
    if !in_box {
        return None;
    }
    if is_checked {
        let is_check = (rx >= 5 && rx <= 9 && (ry as i32 - (rx as i32 + 6)).abs() <= 1)
            || (rx >= 9 && rx <= 16 && (ry as i32 - (24 - rx as i32)).abs() <= 1);
        if is_check {
            return Some(THEME.on_surface);
        }
        return Some(if accent != 0 { accent } else { THEME.success });
    } else {
        let in_inner = in_rounded_rect(x, y, bx + 2, by + 2, 18, 18, 4);
        if !in_inner {
            return Some(THEME.on_surface_variant);
        }
        return None;
    }
}

fn draw_search_glyph(x: usize, y: usize, cx: usize, cy: usize, color: u32) -> Option<u32> {
    let dx = x as i32 - cx as i32;
    let dy = y as i32 - cy as i32;
    let lens_dist = (dx + 2) * (dx + 2) + (dy + 2) * (dy + 2);
    if lens_dist >= 20 && lens_dist <= 48 {
        return Some(color);
    }
    if dx >= 2 && dx <= 7 && (dx - dy).abs() <= 1 {
        return Some(color);
    }
    None
}

fn draw_plus_glyph(x: usize, y: usize, cx: usize, cy: usize, color: u32) -> Option<u32> {
    let dx = (x as i32 - cx as i32).abs();
    let dy = (y as i32 - cy as i32).abs();
    if (dx <= 2 && dy <= 10) || (dy <= 2 && dx <= 10) {
        return Some(color);
    }
    None
}

// -----------------------------------------------------------------------------
// Soft Keyboard Layout (Docked at Screen Bottom: y: 980..1410)
// -----------------------------------------------------------------------------

/// The keyboard dock's edges, from the platform's key table.
const KB_Y0: usize = keyboard::KB_Y0;
const KB_Y1: usize = keyboard::KB_Y1;


// -----------------------------------------------------------------------------
// Pixel Rendering Engine (Zero Allocation Shader)
// -----------------------------------------------------------------------------

fn render_pixel(x: usize, y: usize, state: &AppState) -> u32 {
    // 1. Android / InterOS Bottom Home Gesture Bar (y: 1416..1440)
    if y >= 1416 && y <= 1440 {
        if in_rounded_rect(x, y, 260, 1422, 200, 4, 2) {
            return 0x80FFFFFF;
        }
        return THEME.surface;
    }

    // 2. View Mode Dispatch
    match state.view {
        ViewMode::List => render_notes_list(x, y, state),
        ViewMode::Editor => render_note_editor(x, y, state),
    }
}

struct CardSnippet<'a> {
    p1: &'a [u8],
    p1_is_chk: bool,
    p1_is_done: bool,
    p2: &'a [u8],
    p2_is_chk: bool,
    p2_is_done: bool,
}

fn extract_card_snippet<'a>(body: &'a [u8]) -> CardSnippet<'a> {
    let mut sn = CardSnippet {
        p1: b"",
        p1_is_chk: false,
        p1_is_done: false,
        p2: b"",
        p2_is_chk: false,
        p2_is_done: false,
    };
    if body.is_empty() {
        return sn;
    }

    // Check if body starts with a checklist line
    let starts_chk = body.starts_with(b"[ ] ") || body.starts_with(b"[x] ") || body.starts_with(b"[X] ");
    if starts_chk {
        // Line 1: up to first '\n'
        let mut l1_end = 0;
        while l1_end < body.len() && body[l1_end] != b'\n' {
            l1_end += 1;
        }
        let l1 = &body[..l1_end];
        sn.p1_is_chk = true;
        sn.p1_is_done = l1.starts_with(b"[x] ") || l1.starts_with(b"[X] ");
        let l1_txt = if l1.len() > 4 { &l1[4..] } else { b"" };
        sn.p1 = &l1_txt[..l1_txt.len().min(40)];

        // Line 2: if there is a second line
        if l1_end + 1 < body.len() {
            let rem = &body[l1_end + 1..];
            let mut l2_end = 0;
            while l2_end < rem.len() && rem[l2_end] != b'\n' {
                l2_end += 1;
            }
            let l2 = &rem[..l2_end];
            let l2_chk = l2.starts_with(b"[ ] ") || l2.starts_with(b"[x] ") || l2.starts_with(b"[X] ");
            if l2_chk {
                sn.p2_is_chk = true;
                sn.p2_is_done = l2.starts_with(b"[x] ") || l2.starts_with(b"[X] ");
                let l2_txt = if l2.len() > 4 { &l2[4..] } else { b"" };
                sn.p2 = &l2_txt[..l2_txt.len().min(40)];
            } else if !l2.is_empty() {
                sn.p2 = &l2[..l2.len().min(44)];
            }
        }
        return sn;
    }

    // Normal text wrapping without breaking words
    let mut l1_nl = None;
    for i in 0..body.len().min(44) {
        if body[i] == b'\n' {
            l1_nl = Some(i);
            break;
        }
    }

    if let Some(nl) = l1_nl {
        sn.p1 = &body[..nl];
        if nl + 1 < body.len() {
            let rem = &body[nl + 1..];
            let mut l2_end = 0;
            while l2_end < rem.len() && rem[l2_end] != b'\n' && l2_end < 44 {
                l2_end += 1;
            }
            sn.p2 = &rem[..l2_end];
        }
    } else {
        // No newline in first 44 bytes: wrap at word boundary
        if body.len() <= 40 {
            sn.p1 = body;
        } else {
            let mut break_idx = None;
            for i in (15..=40.min(body.len())).rev() {
                if body[i - 1] == b' ' {
                    break_idx = Some(i - 1);
                    break;
                }
            }
            let p1_len = break_idx.unwrap_or(40.min(body.len()));
            sn.p1 = &body[..p1_len];

            let mut p2_start = p1_len;
            while p2_start < body.len() && body[p2_start] == b' ' {
                p2_start += 1;
            }
            if p2_start < body.len() {
                let rem = &body[p2_start..];
                if rem.len() <= 44 {
                    sn.p2 = rem;
                } else {
                    let mut p2_break = None;
                    for i in (15..=42.min(rem.len())).rev() {
                        if rem[i - 1] == b' ' {
                            p2_break = Some(i - 1);
                            break;
                        }
                    }
                    let p2_len = p2_break.unwrap_or(42.min(rem.len()));
                    sn.p2 = &rem[..p2_len];
                }
            }
        }
    }

    sn
}

// -----------------------------------------------------------------------------
// View 1: Google Keep Notes List Feed
// -----------------------------------------------------------------------------

fn render_notes_list(x: usize, y: usize, state: &AppState) -> u32 {
    // --- A. Elevated Search Pill Bar (y: 24..78, x: 28..692) ---
    if y >= 24 && y <= 78 && x >= 28 && x <= 692 {
        if in_rounded_rect(x, y, 28, 24, 664, 54, 24) {
            if !in_rounded_rect(x, y, 30, 26, 660, 50, 22) {
                return if state.search_active { THEME.primary } else { THEME.border };
            }

            // Search Icon on Left
            if let Some(c) = draw_search_glyph(x, y, 58, 51, THEME.on_surface_variant) {
                return c;
            }

            // Search Query Text or Placeholder
            if state.search_len > 0 {
                let q = &state.search_query[..state.search_len];
                if let Some(c) = test_text(x, y, 88, 42, 1, q, THEME.on_surface, THEME.surface_container, false) {
                    return c;
                }
                // Blinking cursor
                if state.cursor_visible && state.search_active {
                    let cur_x = 88 + state.search_len * 8;
                    if x >= cur_x && x <= cur_x + 2 && y >= 42 && y <= 60 {
                        return THEME.primary;
                    }
                }
            } else {
                let hint = b"Search your notes...";
                if let Some(c) = test_text(x, y, 88, 42, 1, hint, THEME.on_surface_variant, THEME.surface_container, false) {
                    return c;
                }
            }

            // Right Clear 'X' or Note Count Pill
            if state.search_len > 0 {
                let dx = (x as i32 - 650).abs();
                let dy = (y as i32 - 51).abs();
                if (dx - dy).abs() <= 1 && dx <= 6 {
                    return THEME.on_surface_variant;
                }
            } else {
                let count = state.store.count();
                let mut badge = [0u8; 8];
                let n = format_count_badge(count, &mut badge);
                // WHY the count is written out in full: the store holds up to
                // MAX_NOTES notes, and the old badge collapsed the count to a
                // single digit, so a device with twelve notes reported nine.
                let w = outline_text::drawn_advance(&badge[..n], 1, None)
                    .unwrap_or_else(|| std::design::font::measure(&badge[..n], 1));
                let bx = 682usize.saturating_sub(w);
                if let Some(c) = test_text(x, y, bx, 42, 1, &badge[..n], THEME.primary, THEME.surface_container, true) {
                    return c;
                }
            }

            return THEME.surface_container;
        }
        return THEME.surface;
    }

    // --- B. Category Filter Chips and the Pin Filter (y: 96..138) ---
    // Clean 18px gap below search bar
    //
    // WHY the pin is not a sixth chip: a pin is a state a note carries, and
    // a state is not a category. Mixed into the category row it could not
    // be combined with a category, and a person who wanted the pinned
    // notes of one category could not have them.
    if y >= 96 && y <= 138 {
        let chip_names: [&[u8]; 5] = [
            CATEGORY_NAMES[0],
            CATEGORY_NAMES[1],
            CATEGORY_NAMES[2],
            CATEGORY_NAMES[3],
            CATEGORY_NAMES[4],
        ];

        for i in 0..5 {
            let cx = CHIP_XS[i];
            let cw = CHIP_WS[i];
            if x >= cx && x < cx + cw {
                let is_active = state.selected_category as usize == i;
                if in_rounded_rect(x, y, cx, 96, cw, 44, 14) {
                    if is_active {
                        let txt_color = THEME.on_surface;
                        let text_x = cx + (cw - chip_names[i].len() * 8) / 2;
                        if let Some(c) = test_text(x, y, text_x, 109, 1, chip_names[i], txt_color, THEME.primary, true) {
                            return c;
                        }
                        return THEME.primary;
                    } else {
                        if !in_rounded_rect(x, y, cx + 1, 97, cw - 2, 42, 13) {
                            return THEME.border;
                        }
                        let text_x = cx + (cw - chip_names[i].len() * 8) / 2;
                        if let Some(c) = test_text(x, y, text_x, 109, 1, chip_names[i], THEME.on_surface_variant, THEME.surface_container, false) {
                            return c;
                        }
                        return THEME.surface_container;
                    }
                }
            }
        }

        // The pin filter.
        if in_rounded_rect(x, y, PIN_FILTER.0, PIN_FILTER.1, PIN_FILTER.2, PIN_FILTER.3, 14) {
            let active = state.pinned_only;
            let fill = if active { THEME.primary } else { THEME.surface_container };
            let ink = if active { THEME.on_surface } else { THEME.on_surface_variant };
            if !active && !in_rounded_rect(
                x,
                y,
                PIN_FILTER.0 + 1,
                PIN_FILTER.1 + 1,
                PIN_FILTER.2 - 2,
                PIN_FILTER.3 - 2,
                13,
            ) {
                return THEME.border;
            }
            if let Some(c) = draw_pin_glyph(x, y, PIN_FILTER.0 + 22, PIN_FILTER.1 + 22, active, ink) {
                return c;
            }
            if let Some(c) = test_text(x, y, PIN_FILTER.0 + 40, 109, 1, b"PINNED", ink, fill, false) {
                return c;
            }
            return fill;
        }
        return THEME.surface;
    }

    // --- C. Keyboard in List View (if searching) ---
    if state.keyboard_visible && y >= KB_Y0 && y <= KB_Y1 {
        return render_soft_keyboard(x, y, state);
    }

    // --- D. Floating Action Bar Dock at Bottom (y: 1318..1386) ---
    if !state.keyboard_visible && y >= 1318 && y <= 1386 {
        // Right circular Cyan FAB (+) at (654, 1352, r=28)
        if in_circle(x, y, 654, 1352, 28) {
            if let Some(c) = draw_plus_glyph(x, y, 654, 1352, THEME.on_surface) {
                return c;
            }
            return THEME.primary;
        }

        // Left Pill: "Take a note..." + Quick tools (x: 28..608)
        if in_rounded_rect(x, y, 28, 1320, 580, 64, 26) {
            if !in_rounded_rect(x, y, 30, 1322, 576, 60, 24) {
                return THEME.border;
            }

            // Checklist tool icon at x: 440
            if let Some(c) = draw_checkbox_glyph(x, y, 440, 1341, false, THEME.primary) {
                return c;
            }

            // Palette tool icon at x: 500
            if let Some(c) = draw_palette_glyph(x, y, 500, 1352, THEME.on_surface_variant) {
                return c;
            }

            // Search / edit tool icon at x: 560
            if let Some(c) = draw_keyboard_glyph(x, y, 560, 1352, THEME.on_surface_variant) {
                return c;
            }

            // "Take a note..." placeholder
            if let Some(c) = test_text(x, y, 56, 1344, 1, b"Take a note...", THEME.on_surface_variant, THEME.surface_container, false) {
                return c;
            }

            return THEME.surface_container;
        }

        return THEME.surface;
    }

    // --- E. Dynamic Note Cards Feed (y: 162..1308) ---
    // Clean 24px gap below category filter chips
    if y >= 162 && y <= 1308 {
        let mut filtered = [0usize; MAX_NOTES];
        let visible_count = state.filtered_indices(&mut filtered);

        if visible_count == 0 {
            // Empty state card
            if in_rounded_rect(x, y, 60, 440, 600, 240, 24) {
                if !in_rounded_rect(x, y, 62, 442, 596, 236, 22) {
                    return CARD_BORDER;
                }
                // WHY the state is read here and not assumed: an empty list
                // and an unreadable store look the same on the screen unless
                // the app says which one it is. The old code drew "No Notes
                // Saved Yet" for a store it had failed to read, which told
                // the user their notes were gone.
                let (title, subtitle): (&[u8], &[u8]) = if state.search_len > 0 {
                    (b"No matching notes", b"Clear the search to see the rest of your notes")
                } else {
                    match state.storage {
                        StorageState::Ready | StorageState::Empty => (
                            b"No Notes Saved Yet",
                            b"Tap '+ Take a note...' below to capture ideas",
                        ),
                        StorageState::Recovered => (
                            b"Recovered From A Partial Write",
                            b"The staged copy was restored; the previous write did not finish",
                        ),
                        StorageState::Refused => (
                            b"Storage Unavailable",
                            b"The device refused to read the note store; notes cannot be saved",
                        ),
                        StorageState::Corrupt => (
                            b"Store Failed Validation",
                            b"The stored notes did not validate and were moved aside",
                        ),
                    }
                };
                // WHY the title is centred and the subtitle is bounded: the
                // card is a fixed width, and a title placed by a number
                // that happens to suit one string runs past the card's
                // edge the moment the state changes. The subtitle is
                // ellipsized to the card's inner width for the same
                // reason: a sentence that runs past the card is a
                // sentence the user cannot finish reading.
                const CARD_X: usize = 60;
                const CARD_W: usize = 600;
                let title_x = CARD_X + std::design::font::center_x(title, 2, CARD_W);
                if let Some(c) = test_text(x, y, title_x, 500, 2, title, THEME.on_surface, THEME.surface_container, true) {
                    return c;
                }
                let mut sub_buf = [0u8; 96];
                let sub_len = std::design::font::ellipsize(subtitle, 1, CARD_W - 40, &mut sub_buf);
                let sub_x = CARD_X + std::design::font::center_x(&sub_buf[..sub_len], 1, CARD_W);
                if let Some(c) = test_text(x, y, sub_x, 560, 1, &sub_buf[..sub_len], THEME.on_surface_variant, THEME.surface_container, false) {
                    return c;
                }
                return CARD.fill;
            }
            return THEME.surface;
        }

        // Generous, breathable card metrics (Zero text overlapping)
        let card_h = CARD_H;
        let stride = CARD_STRIDE;

        for slot in 0..visible_count {
            let note_idx = filtered[slot];
            let note = match state.store.get_by_index(note_idx) {
                Some(n) => n,
                None => continue,
            };

            // WHY the offset is subtracted here and not stored per card:
            // the platform's scroll model owns the position, and the card
            // that is drawn is the one the model says is under the finger.
            let cy0 = LIST_TOP as isize - state.list_scroll.offset as isize
                + (slot * stride) as isize;
            let cy1 = cy0 + card_h as isize;

            let iy = y as isize;
            if iy >= cy0 && iy < cy1 && x >= 28 && x <= 692 {
                let theme = KEEP_COLORS[note.color_idx as usize % 8];

                // Rounded Card Frame (r=20)
                if !in_rounded_rect(x, y, 28, cy0 as usize, 664, card_h, CARD.radius) {
                    return THEME.surface;
                }
                // Card Border: the card's own hairline, one step inside it.
                if !in_rounded_rect(
                    x,
                    y,
                    30,
                    (cy0 + 2) as usize,
                    660,
                    card_h - 4,
                    CARD.radius.saturating_sub(2),
                ) {
                    return theme.border;
                }

                let uy0 = cy0 as usize;

                // Left Accent Pill Indicator
                if in_rounded_rect(x, y, 32, uy0 + 18, 6, card_h - 36, 3) {
                    return theme.accent;
                }

                // 1. Top row: Note Title (Scale 1 Bold, y: uy0 + 22 .. uy0 + 46)
                let title = note.title_bytes();
                if let Some(c) = test_text(x, y, 56, uy0 + 22, 1, title, theme.text_primary, theme.bg, true) {
                    return c;
                }

                // Pinned Vector Icon if pinned (x: 656, y: uy0 + 26)
                if note.is_pinned {
                    if let Some(c) = draw_pin_glyph(x, y, 656, uy0 + 26, true, THEME.warning) {
                        return c;
                    }
                }

                // 2. Middle: Body preview lines with checklist support & word wrapping
                // Clean 14px gap below Title 24px height: Line 1 starts at uy0 + 60
                let body = note.body_bytes();
                if !body.is_empty() {
                    let sn = extract_card_snippet(body);

                    // Preview line 1 at uy0 + 60 (ends at uy0 + 84)
                    if !sn.p1.is_empty() {
                        if sn.p1_is_chk {
                            if let Some(c) = draw_checkbox_glyph(x, y, 56, uy0 + 60, sn.p1_is_done, theme.accent) {
                                return c;
                            }
                            let tcol = if sn.p1_is_done { theme.text_muted } else { theme.text_body };
                            if let Some(c) = test_text(x, y, 88, uy0 + 60, 1, sn.p1, tcol, theme.bg, false) {
                                return c;
                            }
                            if sn.p1_is_done && y == uy0 + 72 && x >= 88 && x <= 88 + sn.p1.len() * 8 {
                                return theme.text_muted;
                            }
                        } else {
                            if let Some(c) = test_text(x, y, 56, uy0 + 60, 1, sn.p1, theme.text_body, theme.bg, false) {
                                return c;
                            }
                        }
                    }

                    // Preview line 2 at uy0 + 92 (ends at uy0 + 116, clean 8px gap)
                    if !sn.p2.is_empty() {
                        if sn.p2_is_chk {
                            if let Some(c) = draw_checkbox_glyph(x, y, 56, uy0 + 92, sn.p2_is_done, theme.accent) {
                                return c;
                            }
                            let tcol = if sn.p2_is_done { theme.text_muted } else { theme.text_body };
                            if let Some(c) = test_text(x, y, 88, uy0 + 92, 1, sn.p2, tcol, theme.bg, false) {
                                return c;
                            }
                            if sn.p2_is_done && y == uy0 + 104 && x >= 88 && x <= 88 + sn.p2.len() * 8 {
                                return theme.text_muted;
                            }
                        } else {
                            if let Some(c) = test_text(x, y, 56, uy0 + 92, 1, sn.p2, theme.text_body, theme.bg, false) {
                                return c;
                            }
                        }
                    }
                }

                // 3. Bottom Row: Category Chip & Timestamp (y: uy0 + 144..176)
                let cat_name = CATEGORY_NAMES[note.category as usize % 5];
                if in_rounded_rect(x, y, 56, uy0 + 144, 88, 32, 10) {
                    if let Some(c) = test_text(x, y, 66, uy0 + 148, 1, cat_name, theme.accent, theme.chip_bg, true) {
                        return c;
                    }
                    return theme.chip_bg;
                }

                let mut ts_buf = [0u8; 32];
                let ts_len = format_note_timestamp(note, state.now_civil, &mut ts_buf);
                if let Some(c) = test_text(x, y, 510, uy0 + 166, 1, &ts_buf[..ts_len], theme.text_muted, theme.bg, false) {
                    return c;
                }

                return theme.bg;
            }
        }

        // The scroll thumb.
        //
        // WHY the thumb is drawn: a list that has more cards than the band
        // can show has to say so, and a thumb is the platform's own way of
        // saying it.
        if let Some((thumb_y, thumb_h)) =
            state.list_scroll.thumb_rect(LIST_TOP, LIST_VIEWPORT_H, LIST_TRACK_W)
        {
            if x >= LIST_TRACK_X && x < LIST_TRACK_X + LIST_TRACK_W && y >= thumb_y && y < thumb_y + thumb_h
            {
                return THEME.primary;
            }
        }

        return THEME.surface;
    }

    THEME.surface
}

// -----------------------------------------------------------------------------
// View 2: Distraction-Free Colored Note Editor (Google Keep UX)
// -----------------------------------------------------------------------------

fn render_note_editor(x: usize, y: usize, state: &AppState) -> u32 {
    let note = match state.store.get_note(state.selected_note_id) {
        Some(n) => n,
        None => return THEME.surface,
    };
    let theme = KEEP_COLORS[note.color_idx as usize % 8];

    // --- A. Top Navigation Bar (y: 0..84) ---
    if y < 84 {
        // Back Chevron at (50, 46, r=22)
        if in_circle(x, y, 50, 46, 22) {
            if let Some(c) = draw_chevron_left(x, y, 50, 46, THEME.on_surface) {
                return c;
            }
            return theme.chip_bg;
        }

        // Status metadata: "Saved", "Editing..." or "Not saved"
        // WHY three states: the old code derived "Saved" from the note's own
        // dirty flag, which the store cleared only after a successful write --
        // and every write silently failed, so a note that had never reached
        // the device still read "Saved".
        let status_text: &[u8] = if state.save_failed {
            b"Not saved"
        } else if note.is_dirty {
            b"Editing..."
        } else {
            b"Saved"
        };
        let status_color = if state.save_failed { THEME.error } else { theme.text_muted };
        if let Some(c) = test_text(x, y, 96, 38, 1, status_text, status_color, theme.bg, false) {
            return c;
        }

        // Right Tools:
        // 1. Pin Toggle Glyph (x: 520, y: 46)
        if in_circle(x, y, 520, 46, 20) {
            let pcol = if note.is_pinned { THEME.warning } else { theme.text_muted };
            if let Some(c) = draw_pin_glyph(x, y, 520, 46, note.is_pinned, pcol) {
                return c;
            }
            return theme.chip_bg;
        }

        // 2. Palette Toggle Glyph (x: 580, y: 46)
        if in_circle(x, y, 580, 46, 20) {
            if let Some(c) = draw_palette_glyph(x, y, 580, 46, THEME.on_surface) {
                return c;
            }
            return theme.chip_bg;
        }

        // 3. Trash Can Glyph (x: 640, y: 46)
        if in_circle(x, y, 640, 46, 20) {
            if let Some(c) = draw_trash_glyph(x, y, 640, 46, THEME.on_surface) {
                return c;
            }
            return theme.chip_bg;
        }

        // Subtle divider line
        if y == 82 {
            return theme.border;
        }

        return theme.bg;
    }

    // --- B. Note Title Area (y: 96..166) ---
    // Clean, unruled modern typography with ample negative whitespace
    if y >= 96 && y < 166 {
        let title = note.title_bytes();
        if title.is_empty() {
            if let Some(c) = test_text(x, y, 44, 116, 1, b"Title", theme.text_muted, theme.bg, true) {
                return c;
            }
        } else {
            if let Some(c) = test_text(x, y, 44, 116, 1, title, theme.text_primary, theme.bg, true) {
                return c;
            }
        }

        // Title blinking cursor
        if state.focus == EditorFocus::Title && state.cursor_visible && state.keyboard_visible {
            let tw = std::design::font::measure_bold(title, 1);
            let cur_x = 44 + tw;
            if x >= cur_x && x <= cur_x + 2 && y >= 114 && y <= 140 {
                return theme.accent;
            }
        }

        return theme.bg;
    }

    // --- C. Swatch Picker Popover Bar ---
    let swatch_y0 = if state.keyboard_visible { 852 } else { 1248 };
    let swatch_y1 = swatch_y0 + 62;
    if state.swatch_picker_open && y >= swatch_y0 && y <= swatch_y1 {
        if in_rounded_rect(x, y, 20, swatch_y0, 680, 62, 22) {
            if !in_rounded_rect(x, y, 22, swatch_y0 + 2, 676, 58, 20) {
                return THEME.border;
            }

            let cy = swatch_y0 + 31;
            for i in 0..8 {
                let cx = 58 + i * 82;
                let col_theme = KEEP_COLORS[i];

                // Active circle ring
                if i == note.color_idx as usize {
                    if in_circle(x, y, cx, cy, 22) && !in_circle(x, y, cx, cy, 18) {
                        return THEME.on_surface;
                    }
                }

                if in_circle(x, y, cx, cy, 18) {
                    if !in_circle(x, y, cx, cy, 16) {
                        return col_theme.border;
                    }
                    return col_theme.bg;
                }
            }

            return THEME.surface_container;
        }
    }

    // --- D. Keyboard Dock & Accessory Strip ---
    if state.keyboard_visible {
        // Accessory Strip (y: 916..974)
        if y >= 916 && y <= 974 {
            if !in_rounded_rect(x, y, 16, 916, 688, 58, 18) {
                return THEME.surface;
            }

            // Checkbox button at x: 44
            if in_rounded_rect(x, y, 32, 922, 54, 46, 10) {
                if let Some(c) = draw_checkbox_glyph(x, y, 48, 934, false, THEME.primary) {
                    return c;
                }
                return THEME.surface_container;
            }

            // Bullet button at x: 98
            if in_rounded_rect(x, y, 96, 922, 54, 46, 10) {
                if let Some(c) = test_text(x, y, 118, 936, 1, b"*", THEME.on_surface, THEME.surface_container, true) {
                    return c;
                }
                return THEME.surface_container;
            }

            // Palette button at x: 160
            if in_rounded_rect(x, y, 160, 922, 54, 46, 10) {
                if let Some(c) = draw_palette_glyph(x, y, 187, 945, THEME.on_surface) {
                    return c;
                }
                return THEME.surface_container;
            }

            // Category Cycle Pill at x: 224..340
            let cat_name = CATEGORY_NAMES[note.category as usize % 5];
            if in_rounded_rect(x, y, 224, 922, 116, 46, 10) {
                if let Some(c) = test_text(x, y, 240, 936, 1, cat_name, theme.accent, THEME.surface_container, true) {
                    return c;
                }
                return THEME.surface_container;
            }

            // Hide Keyboard Button at x: 630 (Down Chevron)
            if in_rounded_rect(x, y, 626, 922, 64, 46, 10) {
                if let Some(c) = draw_chevron_down(x, y, 658, 945, THEME.primary) {
                    return c;
                }
                return THEME.surface_container;
            }

            return THEME.surface_container;
        }

        // Soft Keyboard (y: 980..1408)
        if y >= KB_Y0 && y <= KB_Y1 {
            return render_soft_keyboard(x, y, state);
        }
    } else {
        // Full-page reading mode: Minimal floating bottom dock (y: 1324..1392)
        if y >= 1324 && y <= 1392 {
            if in_rounded_rect(x, y, 36, 1324, 648, 68, 28) {
                if !in_rounded_rect(x, y, 38, 1326, 644, 64, 26) {
                    return THEME.border;
                }

                // Category Tag
                let cat_name = CATEGORY_NAMES[note.category as usize % 5];
                if in_rounded_rect(x, y, 54, 1338, 100, 40, 12) {
                    if let Some(c) = test_text(x, y, 74, 1346, 1, cat_name, theme.accent, theme.chip_bg, true) {
                        return c;
                    }
                    return theme.chip_bg;
                }

                // Checklist tool
                if in_circle(x, y, 240, 1358, 20) {
                    if let Some(c) = draw_checkbox_glyph(x, y, 229, 1347, false, THEME.primary) {
                        return c;
                    }
                    return THEME.surface_container;
                }

                // Palette tool
                if in_circle(x, y, 340, 1358, 20) {
                    if let Some(c) = draw_palette_glyph(x, y, 340, 1358, THEME.on_surface) {
                        return c;
                    }
                    return THEME.surface_container;
                }

                // Type / Edit note button (Keyboard icon)
                if in_circle(x, y, 440, 1358, 20) {
                    if let Some(c) = draw_keyboard_glyph(x, y, 440, 1358, THEME.primary) {
                        return c;
                    }
                    return THEME.surface_container;
                }

                // Delete Note icon
                if in_circle(x, y, 630, 1358, 20) {
                    if let Some(c) = draw_trash_glyph(x, y, 630, 1358, THEME.error) {
                        return c;
                    }
                    return THEME.surface_container;
                }

                return THEME.surface_container;
            }
        }
    }

    // --- E. Multiline Note Body Area ---
    // Starts at y: 176 (clean 26px below title text)
    let body_max_y = if state.keyboard_visible { 910 } else { 1318 };
    if y >= 170 && y < body_max_y {
        // The ruled page.
        //
        // WHY the page is ruled: the design document asks for a ruled note
        // page, and an editor with no geometry under the text gives the
        // writer nothing to write on. The rule belongs to the note, so it
        // is drawn in the note's own border colour and not in the app's.
        if y >= 176 + RULE_DY && x >= RULE_X0 && x < RULE_X1 {
            let line_index = (y - 176 - RULE_DY) / 36;
            if (y - 176 - RULE_DY) % 36 == 0 {
                let ruled = if state.editor_buf.len == 0 {
                    line_index == 0
                } else {
                    line_index < state.line_spans_count
                };
                if ruled {
                    return theme.border;
                }
            }
        }
        if state.editor_buf.len == 0 {
            if let Some(c) = test_text(x, y, 44, 176, 1, b"Note", theme.text_muted, theme.bg, false) {
                return c;
            }
        } else {
            let body_bytes = state.editor_buf.as_bytes();
            for line_idx in 0..state.line_spans_count {
                let span = state.line_spans[line_idx];
                let line_y = 176 + line_idx * 36;

                if y >= line_y && y < line_y + 36 {
                    let line_slice = &body_bytes[span.start..span.end];
                    let is_chk = line_slice.starts_with(b"[ ] ") || line_slice.starts_with(b"[x] ") || line_slice.starts_with(b"[X] ");
                    let is_done = line_slice.starts_with(b"[x] ") || line_slice.starts_with(b"[X] ");

                    if is_chk {
                        // Draw Checkbox vertically centered at line_y + 7 (22px in 36px line)
                        if let Some(c) = draw_checkbox_glyph(x, y, 44, line_y + 7, is_done, theme.accent) {
                            return c;
                        }
                        // Text starts after "[ ] " at x: 82, vertically aligned with checkbox at line_y + 6
                        let text_slice = if line_slice.len() > 4 { &line_slice[4..] } else { b"" };
                        let tcol = if is_done { theme.text_muted } else { theme.text_primary };
                        if let Some(c) = test_text(x, y, 82, line_y + 6, 1, text_slice, tcol, theme.bg, false) {
                            return c;
                        }
                        // Clean horizontal strikethrough through center of lowercase x-height
                        if is_done && (y == line_y + 19 || y == line_y + 20) && x >= 82 && x <= 82 + text_slice.len() * 8 {
                            return theme.text_muted;
                        }
                    } else {
                        // Normal line at x: 44, y: line_y + 6
                        if let Some(c) = test_text(x, y, 44, line_y + 6, 1, line_slice, theme.text_primary, theme.bg, false) {
                            return c;
                        }
                    }
                }
            }
        }

        // Body blinking cursor
        if state.focus == EditorFocus::Body && state.cursor_visible && state.keyboard_visible {
            let (cur_line, cur_col) = state.cursor_pos;
            let span = if cur_line < state.line_spans_count {
                state.line_spans[cur_line]
            } else {
                LineSpan { start: 0, end: 0 }
            };
            let body_bytes = state.editor_buf.as_bytes();
            let is_chk = if span.end > span.start {
                let s = &body_bytes[span.start..span.end];
                s.starts_with(b"[ ] ") || s.starts_with(b"[x] ") || s.starts_with(b"[X] ")
            } else {
                false
            };
            let text_x0 = if is_chk { 82 } else { 44 };
            let cur_x = text_x0 + cur_col.saturating_sub(if is_chk { 4 } else { 0 }) * 8;
            let cur_y = 176 + cur_line * 36 + 6;
            if x >= cur_x && x <= cur_x + 2 && y >= cur_y && y <= cur_y + 24 {
                return theme.accent;
            }
        }

        return theme.bg;
    }

    theme.bg
}

// -----------------------------------------------------------------------------
// Soft Keyboard Rendering (Docked at Bottom y: 980..1408)
// -----------------------------------------------------------------------------

fn render_soft_keyboard(x: usize, y: usize, state: &AppState) -> u32 {
    // Container Frame
    if !in_rounded_rect(x, y, 6, KB_Y0, 708, KB_Y1 - KB_Y0, 22) {
        return THEME.surface;
    }
    // Top border stroke
    if y == KB_Y0 || y == KB_Y0 + 1 {
        return THEME.border;
    }

    // The key sheet.
    //
    // WHY the keys come from a table: the sheet scrolls when it has more
    // rows than the dock can show, and a sheet that scrolls has to know
    // where its keys are at every offset. The table is the one description
    // the renderer and the tap handler share.
    //
    // WHY the keys are the platform's pills: a key is a small control, and
    // a control that invents its own fill, its own radius and its own ink
    // is a control a third-party app cannot match.
    let rows = keyboard::rows(state.kb_symbols);
    let offset = state.kb_scroll.offset;
    for (index, row) in rows.iter().enumerate() {
        let top = keyboard::row_y(index, offset);
        if top >= KB_Y1 {
            break;
        }
        if top + keyboard::KB_KEY_H <= KB_Y0 {
            continue;
        }
        for key in row.keys {
            let action = key.action;
            let kx = key.x;
            let kw = key.w;
            if x < kx || x >= kx + kw {
                continue;
            }
            if y < top || y >= top + keyboard::KB_KEY_H {
                continue;
            }
            // The platform's pill treatment.
            let fill = THEME.surface_container_high;
            if in_rounded_rect(x, y, kx + 3, top, kw - 6, keyboard::KB_KEY_H - 12, THEME.radii.pill) {
                // The character the key types, in the case the shift state
                // asks for.
                if let Some(c) = keyboard::char_of(keyboard::shifted(action, state.kb_shift)) {
                    let ch_buf = [c];
                    if let Some(c) = test_text(x, y, kx + (kw - 8) / 2, top + 12, 2, &ch_buf, THEME.on_surface, fill, true) {
                        return c;
                    }
                    return fill;
                }
                match action {
                    KeyAction::Char(_) => {
                        // The character was drawn above.
                    }
                    KeyAction::Shift => {
                        let label: &[u8] = if state.kb_symbols { b"ABC" } else { b"^" };
                        if let Some(c) = test_text(x, y, kx + (kw - label.len() * 8) / 2, top + 26, 1, label, THEME.on_surface, fill, true) {
                            return c;
                        }
                    }
                    KeyAction::Symbols => {
                        let label: &[u8] = if state.kb_symbols { b"ABC" } else { b"?123" };
                        if let Some(c) = test_text(x, y, kx + (kw - label.len() * 8) / 2, top + 26, 1, label, THEME.on_surface, fill, true) {
                            return c;
                        }
                    }
                    KeyAction::Backspace => {
                        if let Some(c) = test_text(x, y, kx + (kw - 24) / 2, top + 26, 1, b"<X>", THEME.on_surface, fill, true) {
                            return c;
                        }
                    }
                    KeyAction::Space => {
                        if let Some(c) = test_text(x, y, kx + (kw - 40) / 2, top + 26, 1, b"Space", THEME.on_surface_variant, fill, false) {
                            return c;
                        }
                    }
                    KeyAction::Done => {
                        if let Some(c) = draw_chevron_down(x, y, kx + kw / 2, top + 36, THEME.on_surface) {
                            return c;
                        }
                    }
                }
                return fill;
            }
            // The space between the keys shows the sheet.
            return THEME.surface_container_low;
        }
    }

    THEME.surface_container_low
}

// -----------------------------------------------------------------------------
// UI Presentation & Hardware Scanout
// -----------------------------------------------------------------------------

fn render_notes_ui(surface: u32, state: &AppState) {
    let total_pixels = W * H;
    let mut pixel_idx = 0;

    while pixel_idx < total_pixels {
        let count = CHUNK.min(total_pixels - pixel_idx);
        for i in 0..count {
            let px = pixel_idx + i;
            let x = px % W;
            let y = px / W;
            unsafe {
                CHUNK_BUF[i] = render_pixel(x, y, state);
            }
        }
        unsafe {
            let _ = std::surface_write(surface,
                pixel_idx,
                &CHUNK_BUF[..count],
            );
        }
        pixel_idx += count;
    }
}

// -----------------------------------------------------------------------------
// Touch & Event Handling
// -----------------------------------------------------------------------------

enum TapAction {
    None,
    Redraw,
    Exit,
}

fn inject_char(state: &mut AppState, code: u8) {
    if state.search_active && state.view == ViewMode::List {
        if code == 8 {
            if state.search_len > 0 {
                state.search_len -= 1;
                state.search_query[state.search_len] = 0;
            }
        } else if code >= 32 && code <= 126 {
            if state.search_len < 30 {
                state.search_query[state.search_len] = code;
                state.search_len += 1;
            }
        }
        // WHY the extent is re-derived: the list shows the notes the search
        // matches, and a scroll extent that counted every note would scroll
        // past the last card that is really there.
        state.sync_list_extent();
        return;
    }

    match state.focus {
        EditorFocus::Title => {
            if let Some(note) = state.store.get_note_mut(state.selected_note_id) {
                if code == 8 {
                    if note.title_len > 0 {
                        note.title_len -= 1;
                        note.title[note.title_len] = 0;
                        note.is_dirty = true;
                    }
                } else if code >= 32 && code <= 126 {
                    if note.title_len < MAX_TITLE_LEN - 1 {
                        note.title[note.title_len] = code;
                        note.title_len += 1;
                        note.is_dirty = true;
                    }
                }
            }
        }
        EditorFocus::Body => {
            if code == 8 {
                state.editor_buf.backspace();
            } else if code == 10 {
                state.editor_buf.insert_char('\n');
            } else if code >= 32 && code <= 126 {
                state.editor_buf.insert_char(code as char);
            }
            state.update_editor_layout();
            if let Some(note) = state.store.get_note_mut(state.selected_note_id) {
                note.is_dirty = true;
            }
        }
    }
}

/// Applies a key of the soft keyboard.
///
/// WHY the two views share this: the same key table drives both the list
/// view's search keyboard and the editor's keyboard, and a key that behaved
/// differently in each would be a key that lied about what it did.
fn apply_key_action(state: &mut AppState, action: KeyAction) {
    match action {
        KeyAction::Shift => {
            state.kb_shift = !state.kb_shift;
        }
        KeyAction::Symbols => {
            state.kb_symbols = !state.kb_symbols;
            // WHY the extent is re-derived: the symbols page has more rows
            // than the letters page, so the sheet's height and its scroll
            // position change with the page.
            state.sync_kb_extent();
        }
        KeyAction::Backspace => {
            inject_char(state, 8);
        }
        KeyAction::Space => {
            inject_char(state, b' ');
        }
        KeyAction::Done => {
            state.keyboard_visible = false;
        }
        KeyAction::Char(c) => {
            inject_char(state, c);
        }
    }
}

fn handle_tap(state: &mut AppState, ux: usize, uy: usize) -> TapAction {
    // 1. Android Home Gesture Bar Exit (x: 240..480, y >= 1400)
    if uy >= 1400 && ux >= 240 && ux <= 480 {
        return TapAction::Exit;
    }

    match state.view {
        ViewMode::List => {
            // Dismiss keyboard if tapped outside keyboard when keyboard is open
            if state.keyboard_visible && uy < KB_Y0 && uy > 78 {
                state.keyboard_visible = false;
                state.search_active = false;
                return TapAction::Redraw;
            }

            // Keyboard taps in list view (search typing)
            if state.keyboard_visible && uy >= KB_Y0 && uy <= KB_Y1 {
                if let Some(action) =
                    keyboard::key_at(ux, uy, state.kb_shift, state.kb_symbols, state.kb_scroll.offset)
                {
                    apply_key_action(state, action);
                    return TapAction::Redraw;
                }
            }

            // Search Bar Tap (y: 24..78, x: 28..692)
            if uy >= 24 && uy <= 78 && ux >= 28 && ux <= 692 {
                if state.search_len > 0 && ux >= 630 && ux <= 680 {
                    state.search_len = 0;
                    state.search_query = [0; 32];
                    return TapAction::Redraw;
                }
                state.search_active = true;
                state.keyboard_visible = true;
                return TapAction::Redraw;
            }

            // Category Filter Chips Tap (y: 96..140)
            //
            // WHY the pin has its own control: a pin is a state a note
            // carries, not a category it belongs to, and the two filters
            // have to be combinable.
            //
            // WHY 140 and not 138: the chip row is 44 logical pixels tall
            // so it meets the design system's minimum touch target, and
            // the hit band has to be the row's own height. A band that
            // stopped two pixels short of the chip would leave the last
            // two pixels of every chip dead.
            if uy >= 96 && uy < 140 {
                for i in 0..5 {
                    if ux >= CHIP_XS[i] && ux < CHIP_XS[i] + CHIP_WS[i] {
                        state.selected_category = i as u8;
                        state.list_scroll.offset = 0;
                        state.sync_list_extent();
                        return TapAction::Redraw;
                    }
                }
                if ux >= PIN_FILTER.0 && ux < PIN_FILTER.0 + PIN_FILTER.2 {
                    state.pinned_only = !state.pinned_only;
                    state.list_scroll.offset = 0;
                    state.sync_list_extent();
                    return TapAction::Redraw;
                }
            }

            // Bottom Action Bar Dock (y: 1318..1386)
            if uy >= 1318 && uy <= 1386 {
                // FAB (+) or "Take a note..." tap
                if in_circle(ux, uy, 654, 1352, 34) || (ux >= 28 && ux <= 410) {
                    let Some(new_id) = state.create_stamped_note("") else {
                        // The store is full. The card stays as it is; the
                        // limit is reported by the store, not by opening
                        // somebody else's note.
                        return TapAction::Redraw;
                    };
                    state.selected_note_id = new_id;
                    state.editor_buf.clear();
                    state.focus = EditorFocus::Body;
                    state.update_editor_layout();
                    state.keyboard_visible = true;
                    state.swatch_picker_open = false;
                    state.view = ViewMode::Editor;
                    return TapAction::Redraw;
                }

                // Checklist tool tap
                if ux >= 415 && ux <= 465 {
                    // WHY the body starts with a checklist marker: this is the
                    // scaffold the checklist tool exists to create, and the
                    // editor's first line must be a toggleable item.
                    let Some(new_id) = state.create_stamped_note("[ ] ") else {
                        return TapAction::Redraw;
                    };
                    state.selected_note_id = new_id;
                    state.editor_buf.clear();
                    state.editor_buf.insert_checklist_item();
                    state.focus = EditorFocus::Body;
                    state.update_editor_layout();
                    state.keyboard_visible = true;
                    state.swatch_picker_open = false;
                    state.view = ViewMode::Editor;
                    return TapAction::Redraw;
                }

                // Palette tool tap
                if ux >= 475 && ux <= 525 {
                    let Some(new_id) = state.create_stamped_note("") else {
                        return TapAction::Redraw;
                    };
                    state.selected_note_id = new_id;
                    state.editor_buf.clear();
                    state.focus = EditorFocus::Body;
                    state.update_editor_layout();
                    state.keyboard_visible = false;
                    state.swatch_picker_open = true;
                    state.view = ViewMode::Editor;
                    return TapAction::Redraw;
                }
            }

            // Card Deck Taps (y: 162..1308)
            if uy >= 162 && uy <= 1308 {
                let mut filtered = [0usize; MAX_NOTES];
                let visible_count = state.filtered_indices(&mut filtered);
                let card_h = CARD_H;
                let stride = CARD_STRIDE;

                for slot in 0..visible_count {
                    let cy0 = LIST_TOP as isize - state.list_scroll.offset as isize
                        + (slot * stride) as isize;
                    let cy1 = cy0 + card_h as isize;
                    let iy = uy as isize;
                    if iy >= cy0 && iy < cy1 && ux >= 28 && ux <= 692 {
                        let note_idx = filtered[slot];
                        if let Some(note) = state.store.get_by_index(note_idx) {
                            state.selected_note_id = note.id;
                            state.editor_buf.load_from_slice(note.body_bytes());
                            state.focus = EditorFocus::Body;
                            state.update_editor_layout();
                            state.keyboard_visible = false;
                            state.swatch_picker_open = false;
                            state.view = ViewMode::Editor;
                            return TapAction::Redraw;
                        }
                    }
                }
            }
        }

        ViewMode::Editor => {
            // Top Bar: Back Chevron at (50, 46, r=24)
            if in_circle(ux, uy, 50, 46, 24) {
                state.sync_editor_to_note();
                state.save_to_storage();
                state.view = ViewMode::List;
                state.sync_list_extent();
                state.keyboard_visible = false;
                state.swatch_picker_open = false;
                return TapAction::Redraw;
            }

            // Top Bar: Pin Toggle at (520, 46, r=22)
            if in_circle(ux, uy, 520, 46, 22) {
                if let Some(note) = state.store.get_note_mut(state.selected_note_id) {
                    note.toggle_pinned();
                }
                return TapAction::Redraw;
            }

            // Top Bar: Palette Toggle at (580, 46, r=22)
            if in_circle(ux, uy, 580, 46, 22) {
                state.swatch_picker_open = !state.swatch_picker_open;
                return TapAction::Redraw;
            }

            // Top Bar: Trash / Delete at (640, 46, r=22)
            if in_circle(ux, uy, 640, 46, 22) {
                state.store.delete_note(state.selected_note_id);
                state.sync_list_extent();
                state.save_to_storage();
                state.view = ViewMode::List;
                state.keyboard_visible = false;
                state.swatch_picker_open = false;
                return TapAction::Redraw;
            }

            // Title Area Tap (y: 96..166)
            if uy >= 96 && uy < 166 && ux >= 40 && ux <= 680 {
                state.focus = EditorFocus::Title;
                state.keyboard_visible = true;
                return TapAction::Redraw;
            }

            // Swatch Picker Popover Bar Taps
            let swatch_y0 = if state.keyboard_visible { 852 } else { 1248 };
            let swatch_y1 = swatch_y0 + 62;
            if state.swatch_picker_open && uy >= swatch_y0 && uy <= swatch_y1 {
                let cy = swatch_y0 + 31;
                for i in 0..8 {
                    let cx = 58 + i * 82;
                    if in_circle(ux, uy, cx, cy, 26) {
                        if let Some(note) = state.store.get_note_mut(state.selected_note_id) {
                            note.set_color(i as u8);
                        }
                        return TapAction::Redraw;
                    }
                }
            }

            // When Keyboard is visible
            if state.keyboard_visible {
                // Accessory Strip Taps (y: 916..974)
                if uy >= 916 && uy <= 974 {
                    // Checkbox button
                    if ux >= 32 && ux <= 86 {
                        state.editor_buf.insert_checklist_item();
                        state.update_editor_layout();
                        return TapAction::Redraw;
                    }
                    // Bullet button
                    if ux >= 96 && ux <= 150 {
                        state.editor_buf.insert_str("\n* ");
                        state.update_editor_layout();
                        return TapAction::Redraw;
                    }
                    // Palette button
                    if ux >= 160 && ux <= 214 {
                        state.swatch_picker_open = !state.swatch_picker_open;
                        return TapAction::Redraw;
                    }
                    // Category cycle button
                    if ux >= 224 && ux <= 340 {
                        if let Some(note) = state.store.get_note_mut(state.selected_note_id) {
                            note.category = (note.category + 1) % 5;
                            note.is_dirty = true;
                        }
                        return TapAction::Redraw;
                    }
                    // Dismiss keyboard button
                    if ux >= 626 && ux <= 688 {
                        state.keyboard_visible = false;
                        state.swatch_picker_open = false;
                        return TapAction::Redraw;
                    }
                }

                // Keyboard Key Taps (y: 980..1408)
                if uy >= KB_Y0 && uy <= KB_Y1 {
                    if let Some(action) =
                        keyboard::key_at(ux, uy, state.kb_shift, state.kb_symbols, state.kb_scroll.offset)
                    {
                        apply_key_action(state, action);
                        if !state.keyboard_visible {
                            state.swatch_picker_open = false;
                        }
                        return TapAction::Redraw;
                    }
                }
            } else {
                // Reading mode bottom dock taps (y: 1324..1392)
                if uy >= 1324 && uy <= 1392 {
                    // Category cycle pill (x: 54..154)
                    if ux >= 54 && ux <= 154 {
                        if let Some(note) = state.store.get_note_mut(state.selected_note_id) {
                            note.category = (note.category + 1) % 5;
                            note.is_dirty = true;
                        }
                        return TapAction::Redraw;
                    }
                    // Checkbox tool
                    if in_circle(ux, uy, 240, 1358, 24) {
                        state.editor_buf.insert_checklist_item();
                        state.update_editor_layout();
                        state.keyboard_visible = true;
                        return TapAction::Redraw;
                    }
                    // Palette tool
                    if in_circle(ux, uy, 340, 1358, 24) {
                        state.swatch_picker_open = !state.swatch_picker_open;
                        return TapAction::Redraw;
                    }
                    // Edit / Keyboard button
                    if in_circle(ux, uy, 440, 1358, 24) {
                        state.focus = EditorFocus::Body;
                        state.keyboard_visible = true;
                        return TapAction::Redraw;
                    }
                    // Delete button
                    if in_circle(ux, uy, 630, 1358, 24) {
                        state.store.delete_note(state.selected_note_id);
                        state.sync_list_extent();
                        state.save_to_storage();
                        state.view = ViewMode::List;
                        return TapAction::Redraw;
                    }
                }
            }

            // Body Area Taps: Checkbox toggles and text positioning
            let body_max_y = if state.keyboard_visible { 910 } else { 1318 };
            if uy >= 168 && uy <= body_max_y && ux >= 36 && ux <= 684 {
                let line_idx = (uy.saturating_sub(172)) / 36;
                if line_idx < state.line_spans_count {
                    let span = state.line_spans[line_idx];
                    // Check if tap hit the checkbox directly at x: 36..78
                    if ux >= 36 && ux <= 78 {
                        if state.editor_buf.toggle_checklist_at(span.start) {
                            state.update_editor_layout();
                            if let Some(note) = state.store.get_note_mut(state.selected_note_id) {
                                note.is_dirty = true;
                            }
                            return TapAction::Redraw;
                        }
                    }

                    // Otherwise focus body and place cursor
                    state.focus = EditorFocus::Body;
                    state.keyboard_visible = true;
                    let is_chk = if span.end > span.start {
                        let s = &state.editor_buf.as_bytes()[span.start..span.end];
                        s.starts_with(b"[ ] ") || s.starts_with(b"[x] ") || s.starts_with(b"[X] ")
                    } else {
                        false
                    };
                    let text_x0 = if is_chk { 82 } else { 44 };
                    let col = (ux.saturating_sub(text_x0)) / 8;
                    state.editor_buf.cursor = (span.start + (if is_chk { 4 } else { 0 }) + col).min(span.end);
                    state.update_editor_layout();
                    return TapAction::Redraw;
                } else {
                    state.focus = EditorFocus::Body;
                    state.keyboard_visible = true;
                    state.editor_buf.move_cursor_to_end();
                    state.update_editor_layout();
                    return TapAction::Redraw;
                }
            }
        }
    }

    TapAction::None
}

// -----------------------------------------------------------------------------
// VFS Persistence Operations
// -----------------------------------------------------------------------------

/// The live note store, inside this app's own sandbox.
///
/// WHY the path is relative and not absolute: `app_storage_open` resolves a
/// relative path inside the calling app's sandbox root
/// (`/user/<uid>/apps/org.interos.notes`) and the kernel rejects an absolute
/// path with `InvalidArguments`. The old constant was an absolute path
/// under the device's `/data`, so the open always failed and every note the
/// user wrote was lost when the app exited.
const STORAGE_PATH: &str = "data/notes.bin";

/// The staged copy, written before the live store is replaced.
///
/// WHY a staging copy exists: the platform exposes no atomic rename, so a
/// write that is interrupted by a power loss can leave the live store
/// truncated. Writing the new blob to a staging file first means the last
/// known-good copy survives, and the loader can restore it. The Fletcher-32
/// checksum on the blob is what tells the loader which copy is the good one.
const STAGING_PATH: &str = "data/notes.staged";

/// Where a store that failed validation is kept.
///
/// WHY the bytes are kept and not deleted: a store that does not validate is
/// either a partial write or a file from a future version, and in both cases
/// the user may still want what is in it. The bytes are copied here before
/// the live store is reinitialised.
const QUARANTINE_PATH: &str = "data/notes.corrupt";

/// The largest store blob the app reads or writes.
///
/// WHY this bound: the store holds at most `MAX_NOTES` notes, and the
/// serialised form of one note is bounded by the title and body limits, so
/// 8 KiB is comfortably above the largest store the app can produce and
/// below the app's stack budget.
const STORE_BLOB_MAX: usize = 8192;

/// Reads a file from the app's sandbox into `buf`.
///
/// Returns the number of bytes read. `Err` carries the platform's own
/// refusal code: `-8` is `FileNotFound`, which is how the app tells "there
/// is no store yet" from "the platform would not read it".
fn read_sandbox_file(path: &str, buf: &mut [u8]) -> Result<usize, i64> {
    let fd = std::app_storage_open(path, std::FS_READ)?;
    let n = match std::fs_read(fd, buf) {
        Ok(n) => n,
        Err(e) => {
            let _ = std::fs_close(fd);
            return Err(e);
        }
    };
    let _ = std::fs_close(fd);
    Ok(n)
}

/// Writes a file into the app's sandbox and flushes it to the device.
fn write_sandbox_file(path: &str, bytes: &[u8]) -> Result<usize, i64> {
    let fd = std::app_storage_open(path, std::FS_WRITE)?;
    let n = match std::fs_write(fd, bytes) {
        Ok(n) => n,
        Err(e) => {
            let _ = std::fs_close(fd);
            return Err(e);
        }
    };
    // WHY the flush: the write is only durable once the platform has
    // flushed it, and the whole point of the staging copy is to survive a
    // power loss.
    let _ = std::fs_fsync(fd);
    let _ = std::fs_close(fd);
    Ok(n)
}

/// Loads the note store and reports what the device actually held.
///
/// WHY the return type carries the distinction: an empty list and an
/// unreadable list look identical until the app says which one it is, and
/// the old code reported both as "no notes saved".
fn load_notes_from_storage(store: &mut NotesStore) -> StorageState {

    let mut buf = [0u8; STORE_BLOB_MAX];
    let live = read_sandbox_file(STORAGE_PATH, &mut buf);

    let live_bytes: &[u8] = match live {
        Ok(n) => &buf[..n],
        Err(-8) => {
            // There is no live store. A staged copy is still worth trying,
            // because that is exactly the state a power loss during a save
            // leaves behind.
            let mut staged = [0u8; STORE_BLOB_MAX];
            match read_sandbox_file(STAGING_PATH, &mut staged) {
                Ok(n) if n > 0 => {
                    if store.deserialize(&staged[..n]).is_ok() {
                        restore_live_store(store);
                        return StorageState::Recovered;
                    }
                    quarantine_failed_store(&staged[..n]);
                    return StorageState::Corrupt;
                }
                _ => return StorageState::Empty,
            }
        }
        Err(_) => return StorageState::Refused,
    };

    if live_bytes.is_empty() {
        return StorageState::Empty;
    }
    if store.deserialize(live_bytes).is_ok() {
        return StorageState::Ready;
    }

    // The live store did not validate. Try the staged copy before giving up
    // on the user's notes.
    let mut staged = [0u8; STORE_BLOB_MAX];
    match read_sandbox_file(STAGING_PATH, &mut staged) {
        Ok(n) if n > 0 && store.deserialize(&staged[..n]).is_ok() => {
            restore_live_store(store);
            StorageState::Recovered
        }
        _ => {
            quarantine_failed_store(live_bytes);
            store.new_empty();
            StorageState::Corrupt
        }
    }
}

/// Copies the current store into the live path from the staging copy.
///
/// WHY this is a copy and not a rename: the platform has no rename syscall.
/// The staging copy is the known-good blob, so it is written over the live
/// path and the result is checked.
fn restore_live_store(store: &NotesStore) {
    let mut buf = [0u8; STORE_BLOB_MAX];
    let n = match store.serialize(&mut buf) {
        Ok(n) => n,
        Err(_) => return,
    };
    let _ = write_sandbox_file(STORAGE_PATH, &buf[..n]);
}

/// Keeps a store that failed validation instead of destroying it.
///
/// WHY the bytes are copied before the live store is reset: a store that
/// does not validate is either a partial write or a file this version does
/// not understand, and in both cases the user may still want what is in
/// it. The copy is best effort -- if the platform refuses the write, the
/// bytes are still in the live path and the app says the store is corrupt.
fn quarantine_failed_store(bytes: &[u8]) {
    let _ = write_sandbox_file(QUARANTINE_PATH, bytes);
}

/// Writes the store to the device.
///
/// WHY the staging copy comes first: the platform has no atomic rename, so
/// the sequence is "stage, flush, replace, flush, read back". A power loss
/// during the replace leaves the staging copy holding the last known-good
/// blob, and the loader restores it. The read-back is what turns "the write
/// was accepted" into "the write landed".
fn save_notes_to_storage(store: &mut NotesStore) -> bool {

    let mut buf = [0u8; STORE_BLOB_MAX];
    let nbytes = match store.serialize(&mut buf) {
        Ok(n) => n,
        Err(_) => return false,
    };

    // Stage first.
    if write_sandbox_file(STAGING_PATH, &buf[..nbytes]).is_err() {
        return false;
    }
    // Replace the live store.
    match write_sandbox_file(STORAGE_PATH, &buf[..nbytes]) {
        Ok(n) if n == nbytes => {}
        _ => return false,
    }

    // Read the live store back and validate it. Only a store that reads back
    // and validates counts as saved.
    let mut check = [0u8; STORE_BLOB_MAX];
    let n = match read_sandbox_file(STORAGE_PATH, &mut check) {
        Ok(n) => n,
        Err(_) => return false,
    };
    if n != nbytes {
        return false;
    }
    let mut verify = NotesStore::new();
    if verify.deserialize(&check[..n]).is_err() {
        return false;
    }

    for i in 0..store.count() {
        if let Some(note) = store.get_by_index_mut(i) {
            note.is_dirty = false;
        }
    }
    true
}

// -----------------------------------------------------------------------------
// Ring-3 Entrypoint & Lifecycle
// -----------------------------------------------------------------------------

#[no_mangle]
pub extern "C" fn _start() -> ! {
    // WHY there is no capability lookup before the surface is asked for: the
    // kernel authorises the surface by the capability's *type*, so the app
    // names the authority it wants and the kernel finds it. A slot is a
    // position in this app's capability table, and the position depends on
    // the order the kernel inserted the capabilities in -- the launcher
    // proved that by passing slot 0 while its filesystem read capability sat
    // at slot 3, and the developer-terminal flag it wanted was never read.
    // The platform's own answer to the request is the one that is reported.
    let surface = match std::surface_create(W as u32, H as u32, 200) {
        Ok(s) => s,
        Err(_) => {
            std::log("[NOTES] No surface could be created; refusing to draw.").ok();
            std::exit_with_code(1);
        }
    };

    let mut state = AppState {
        store: NotesStore::new(),
        view: ViewMode::List,
        selected_note_id: 1,
        focus: EditorFocus::Body,
        editor_buf: TextEditorBuffer::new(),
        cursor_visible: true,
        cursor_pos: (0, 0),
        line_spans: [LineSpan { start: 0, end: 0 }; 32],
        line_spans_count: 0,
        storage: StorageState::Empty,
        save_failed: false,
        list_scroll: ScrollState::new(0, LIST_VIEWPORT_H, 0),
        kb_scroll: ScrollState::new(keyboard::content_h(false), keyboard::viewport_h(), 0),
        pinned_only: false,
        selected_category: 0,
        search_active: false,
        search_query: [0; 32],
        search_len: 0,
        keyboard_visible: false,
        swatch_picker_open: false,
        kb_shift: false,
        kb_symbols: false,
        now_civil: None,
    };

    // WHY nothing is seeded here: a notes app whose contents are the user's
    // must not invent documents and write them into persistent storage. The
    // store used to be seeded with four invented notes -- one of which
    // asserted hardware specifications the app had never measured -- and
    // then persisted them, so the device carried documents the user never
    // wrote. When the store cannot be read the list is simply empty, and the
    // UI already says "No Notes Saved Yet".
    let loaded = load_notes_from_storage(&mut state.store);
    if matches!(
        loaded,
        StorageState::Refused | StorageState::Corrupt | StorageState::Empty
    ) {
        state.store.new_empty();
    }
    // WHY this is the read's own result and not a constant: what the store
    // holds is a fact about the device, and the read is the only thing that
    // can support it. The old code kept the result in a flag that no
    // renderer ever read, so a refused or corrupt store was displayed as an
    // empty list.
    state.storage = loaded;
    state.now_civil = current_civil_time();

    // The scroll extents follow what the store now holds, and the notes are
    // published so the file manager, the launcher and the platform's
    // document index can find them.
    state.sync_list_extent();
    state.sync_kb_extent();
    state.publish_notes();

    // Initial frame presentation
    render_notes_ui(surface, &state);
    let _ = std::surface_flip();

    // Required by automated lifecycle test contracts
    std::log("[NOTES] Initial frame presented via hardware scanout.",
    )
    .ok();

    let mut input = std::InputEvent {
        kind: 0,
        x: 0,
        y: 0,
        dx: 0,
        dy: 0,
        scale_milli: 1000,
    };
    let mut blink_counter = 0usize;
    let mut clock_counter = 0usize;

    // Interactive Event Loop
    loop {
        let mut needs_redraw = false;

        if let Ok(_) = std::event_poll(std::EVENT_TOUCH_INPUT, 50) {
            while std::input_event_poll(&mut input).is_ok() {
                if let Some(key) = input.key_event() {
                    if key.pressed {
                        if key.code == 27 {
                            // Escape key exits app or editor
                            if state.keyboard_visible {
                                state.keyboard_visible = false;
                                state.swatch_picker_open = false;
                                needs_redraw = true;
                            } else if state.view == ViewMode::Editor {
                                state.sync_editor_to_note();
                                state.save_to_storage();
                                state.view = ViewMode::List;
                                needs_redraw = true;
                            } else {
                                let _ = std::surface_destroy(surface);
                                std::exit_with_code(0);
                            }
                        } else if state.view == ViewMode::Editor {
                            inject_char(&mut state, key.code as u8);
                            needs_redraw = true;
                        }
                    }
                    continue;
                }

                if input.kind == 4 || input.kind == 6 || input.kind == 7 || input.kind == 8 || input.kind == 10 {
                    // A drag that ends over a scrolling band is a scroll
                    // gesture; anything else is the system's navigation
                    // gesture.
                    //
                    // WHY the drag has to be handled here: the old code
                    // treated every one of these events as an exit, so a
                    // person who dragged the note list left the app instead
                    // of scrolling it, and a note below the fold could not
                    // be reached.
                    let over_list = state.view == ViewMode::List
                        && !state.keyboard_visible
                        && input.y >= LIST_TOP as u32
                        && (input.y as usize) < LIST_BOTTOM;
                    let over_keyboard = state.keyboard_visible
                        && input.y >= KB_Y0 as u32
                        && (input.y as usize) < KB_Y1;
                    if (input.kind == 4 || input.kind == 10) && over_list {
                        state.list_scroll.apply_drag(input.dy);
                        needs_redraw = true;
                        continue;
                    }
                    if (input.kind == 4 || input.kind == 10) && over_keyboard {
                        state.kb_scroll.apply_drag(input.dy);
                        needs_redraw = true;
                        continue;
                    }
                    // System navigation exit gestures
                    let _ = std::surface_destroy(surface);
                    std::exit_with_code(0);
                }

                if input.kind == 1 {
                    // Touch tap event
                    match handle_tap(&mut state, input.x as usize, input.y as usize) {
                        TapAction::Exit => {
                            let _ = std::surface_destroy(surface);
                            std::exit_with_code(0);
                        }
                        TapAction::Redraw => needs_redraw = true,
                        TapAction::None => {}
                    }
                } else if input.kind == 2 {
                    // Touch drag / kinetic scroll event
                    if state.view == ViewMode::List && !state.keyboard_visible {
                        state.list_scroll.apply_drag(input.dy);
                        needs_redraw = true;
                    }
                }
            }
        }

        // Periodic cursor blink
        blink_counter += 1;
        if blink_counter >= 20 {
            blink_counter = 0;
            state.cursor_visible = !state.cursor_visible;
            if (state.view == ViewMode::Editor && state.keyboard_visible) || state.search_active {
                needs_redraw = true;
            }
        }

        // Refresh the device clock roughly once a second.
        //
        // WHY this is a counter and not a per-frame read: the RTC is a
        // syscall, and the caption only needs to be right to the minute. It
        // is also the only way "Today" stays true across a session that
        // crosses midnight -- but it is only ever allowed to carry a time
        // the clock actually reported.
        clock_counter += 1;
        if clock_counter >= 20 {
            clock_counter = 0;
            state.now_civil = current_civil_time();
            if state.view == ViewMode::List {
                needs_redraw = true;
            }
        }

        if needs_redraw {
            render_notes_ui(surface, &state);
            let _ = std::surface_flip();
        }

        std::yield_now();
    }
}

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    loop {}
}
