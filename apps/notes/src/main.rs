// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

//! InterOS Sovereign Native Notes Application — Google Keep-Grade Mobile UX (2026 Edition)
//!
//! Production Ring-3 graphical application executing in User Mode:
//! - DRM/KMS Client Surface allocation with dynamic capability lookup
//! - Pure `#![no_std]` and `#![no_main]` with zero heap allocations in scanout and event polling
//! - Monochrome note tones: four paper steps and four ink steps, all platform roles
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
use notes::chrome;
use notes::keyboard::{self, KeyAction};
use notes::library;
pub use notes::{
    format_count_badge, format_timestamp, LineSpan, Note, NoteTone, NotesStore,
    TextEditorBuffer, CATEGORY_NAMES, CivilTime, MAX_BODY_LEN, MAX_NOTES, MAX_TITLE_LEN,
    NOTE_TONE_COUNT, NOTE_TONES, StorageState, TIMESTAMP_UNKNOWN,
};

// The boxes the painter draws into, and the lanes it ellipsizes its runs to.
//
// WHY they are the library's and not literals in the branch that paints them:
// the binary is built without a test harness, so a fit gate can only reach a
// container this library exports. A lane authored here is a number no test can
// check; a lane derived there is the same number plus a refusal.
use notes::{
    CARD_HAIRLINE, CHECKBOX_TEXT_X, CHECKBOX_X, EDITOR_TITLE_LANE,
    LIST_CARD_CHECK_SNIPPET_LANE, LIST_CARD_INNER_X0, LIST_CARD_INNER_X1, LIST_CARD_RAIL_X,
    LIST_CARD_SNIPPET_LANE, LIST_CARD_TEXT_X, LIST_CARD_TITLE_LANE,
    LIST_CARD_TITLE_LANE_PINNED, LIST_CARD_W, LIST_CARD_X0, LIST_CARD_X1, PIN_GLYPH_CX,
    EMPTY_CARD_H, EMPTY_CARD_W, EMPTY_CARD_X, EMPTY_CARD_Y, EMPTY_SUBTITLE_LANE,
};

const RULE_X0: usize = notes::EDITOR_RULE_X0;
const RULE_X1: usize = notes::EDITOR_RULE_X1;

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
/// The scale the editor draws its body text at, and the column it wraps to.
const EDITOR_BODY_SCALE: usize = 2;

/// The note card's height.
const CARD_H: usize = 216;

/// The gap between two note cards.
const CARD_GAP: usize = 20;

/// The distance from one card's top to the next one's top.
const CARD_STRIDE: usize = CARD_H + CARD_GAP;

/// The row this app's own chrome starts at.
///
/// WHY not the top of the panel: the status band owns `0..STATUS_BAR_HEIGHT` and
/// answers the pixel function before any of this app's rows are reached, so the
/// search pill authored at y=24 lost the whole upper arc of its round ends to a
/// surface painted over it -- and the editor's circular tools lost their top half.
const HEADER_TOP: usize = interos_std::design::viewport::HEADER_TOP;

/// The list's search pill.
///
/// WHY it reads the chrome module: the pill, the clear control inside it and
/// the press that answers it are three sites over two functions, and they had
/// three sets of numbers.
const SEARCH_PILL: notes::chrome::Rect = notes::chrome::SEARCH_PILL;
const SEARCH_CLEAR: notes::chrome::Rect = notes::chrome::SEARCH_CLEAR;
const SEARCH_Y0: usize = SEARCH_PILL.y;
const SEARCH_H: usize = SEARCH_PILL.h;
const SEARCH_Y1: usize = SEARCH_PILL.y + SEARCH_PILL.h;

/// The pill's own search icon, and the run beside it.
///
/// WHY they are pushed clear of the chip: the back control is painted over the
/// pill's left end at the platform's rect, and a glyph authored inside that
/// square is a picture nobody can see and a target nobody can hit. The pill
/// itself keeps its width, so the row under it is untouched.
const SEARCH_GLYPH_CX: usize = chrome::LABEL_X + 12;
const SEARCH_TEXT_X: usize = chrome::LABEL_X + 30;

/// The category chip row, 16 rows under the pill.
const CHIP_Y0: usize = SEARCH_Y1 + 16;
const CHIP_H: usize = 48;
const CHIP_Y1: usize = CHIP_Y0 + CHIP_H;

/// The editor's toolbar box, and the centre row its circular tools stand on.
///
/// WHY the centre is 24 rows into the box rather than at its middle: the tools are
/// circles of radius 20-22 and the box has to carry the divider below them.
const EDITOR_BAR_H: usize = HEADER_TOP + 62;
const EDITOR_TOOL_CY: usize = notes::chrome::EDITOR_TOOL_CY;

/// The editor's title row, the body band under it, and the first line's top.
const EDITOR_TITLE_Y0: usize = EDITOR_BAR_H + 12;
const EDITOR_TITLE_Y1: usize = EDITOR_TITLE_Y0 + 70;
const EDITOR_TITLE_TEXT_Y: usize = EDITOR_TITLE_Y0 + 20;
const EDITOR_BODY_Y0: usize = EDITOR_TITLE_Y1 + 4;
const EDITOR_TEXT_Y0: usize = EDITOR_BODY_Y0 + 6;

/// The top of the note list's visible band.
const LIST_TOP: usize = CHIP_Y1 + 22;

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
const PIN_FILTER: (usize, usize, usize, usize) = (556, CHIP_Y0, 136, CHIP_H);

// WHY these gates sit beside the numbers: the chip row and the pin filter are
// painted and pressed from the arrays below, and both bands once ran one row
// past the plate they inked. A floor that is only written in a comment is a
// floor the next edit steps over.
const _: () = assert!(CHIP_H >= interos_std::design::touch::MIN_TARGET);
const _: () = {
    let mut i = 0;
    while i < CHIP_XS.len() {
        assert!(CHIP_WS[i] >= interos_std::design::touch::MIN_TARGET);
        assert!(CHIP_XS[i] + CHIP_WS[i] <= 720);
        if i + 1 < CHIP_XS.len() {
            assert!(CHIP_XS[i] + CHIP_WS[i] <= CHIP_XS[i + 1]);
        }
        i += 1;
    }
    assert!(PIN_FILTER.2 >= interos_std::design::touch::MIN_TARGET);
    assert!(PIN_FILTER.3 >= interos_std::design::touch::MIN_TARGET);
    assert!(CHIP_XS[CHIP_XS.len() - 1] + CHIP_WS[CHIP_WS.len() - 1] <= PIN_FILTER.0);
    assert!(PIN_FILTER.0 + PIN_FILTER.2 <= 720);
};

/// The width the editor wraps a body line to: the distance between the rule's
/// two edges, the column every line is drawn in.
///
/// WHY the column and not a character count: the body is drawn with the outline
/// face at `EDITOR_BODY_SCALE`, whose glyphs vary in width, so a line has to
/// break where its measured advance reaches this edge rather than after a fixed
/// number of characters that never measures anything.
const EDITOR_TEXT_COL_W: usize = notes::EDITOR_TEXT_COL_W;

/// The distance from one line to the next in the editor.
#[allow(dead_code)]
const EDITOR_LINE_STRIDE: usize = 40;
/// The distance from a line's top to the rule drawn under it in the editor.
#[allow(dead_code)]
const EDITOR_RULE_DY: usize = 36;

// -----------------------------------------------------------------------------
// Visual Design Tokens
//
// WHY there is no palette here any more: the app used to carry its own
// colours, which is a second design system inside one app. Every colour now
// resolves through the platform's theme, so a third-party app that asks the
// SDK for the same roles draws the same notes surface. See `notes::theme` for
// the mapping from the old names to the platform's roles.
//
// WHY the note's own tone is still a ladder: `NOTE_TONES` is the tone the
// user picks for a note -- one of eight, from paper to ink -- and not the
// app's chrome. The platform's theme describes the surface the note is drawn
// on; the tone is what the user chose to write it on, and every step of the
// ladder is itself a platform role.
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
    pub line_spans: [LineSpan; 48],
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
    pub clock: [u8; 5],
    pub clock_ok: bool,
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
        self.line_spans_count =
            self.editor_buf.compute_lines(EDITOR_BODY_SCALE, EDITOR_TEXT_COL_W, &mut self.line_spans);
        self.cursor_pos = self
            .editor_buf
            .cursor_line_col(EDITOR_BODY_SCALE, EDITOR_TEXT_COL_W);
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
/// WHY the outline face is tried first: the platform scanout is 720x1440,
/// the same as the authoring space this app paints, so composition is 1:1
/// and the compositor magnifies nothing. The outline face is still preferred
/// because it rasterises the real outline at the requested cap height
/// instead of magnifying the 16x24 bitmap fallback face, so it stays sharp
/// at every size.
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
    if x < bx || x >= W || x - bx >= width || y < top || y - top >= height {
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
///
/// WHY the reading comes from the SDK: the note used to scan the kernel's JSON
/// itself, with its own bounds and its own idea of when a payload was whole, so
/// a note stamp and the lock-screen clock disagreed about the same second.
fn current_civil_time() -> Option<CivilTime> {
    let now = std::rtc::Reading::now()?;
    // WHY the hour and the minute are required and not defaulted: a payload
    // that carried a date but no time would otherwise be stamped as
    // midnight, and midnight is a time the clock never reported. The note
    // carries no timestamp instead. `Reading::now` asks for all five fields
    // and refuses the whole reading if any is missing or out of range.
    Some(CivilTime {
        year: now.year,
        month: now.month,
        day: now.day,
        hour: now.hour,
        minute: now.minute,
    })
}

/// Formats a note's caption timestamp from the clock, never from a constant.
///
/// WHY `now` is passed in rather than read here: the current day decides
/// whether the caption says "Today", and the app already keeps the clock
/// reading in one place.
fn format_note_timestamp(note: &Note, now: Option<CivilTime>, buf: &mut [u8; 32]) -> usize {
    if note.is_dirty {
        // WHY the list says "Unsaved changes" and not "Editing now": this
        // caption is drawn on the card in the list, where the user is not
        // editing, so it may only report the note's state.
        let s = b"Unsaved changes";
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
    // A stadium radius (the platform's pill token) is 999 and must be clamped
    // to half the shape, otherwise the corner-distance test fails at every
    // pixel and the shape disappears.
    let r = r.min(rw / 2).min(rh / 2);
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

fn draw_chevron_right(x: usize, y: usize, cx: usize, cy: usize, color: u32) -> Option<u32> {
    let dx = x as i32 - cx as i32;
    let dy = y as i32 - cy as i32;
    if dy.abs() <= 1 && dx >= -8 && dx <= 7 {
        return Some(color);
    }
    if dy.abs() <= 8 && (dx - (7 - dy.abs())).abs() <= 1 {
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
    // 3 tone wells, all in the ink the caller asked for.
    //
    // WHY they are not green, amber and red: a palette glyph is a shape, not
    // a status, and the platform's three status colours are reserved for a
    // reading that means something. Drawing them here spent the status
    // palette on decoration.
    if (dx + 3) * (dx + 3) + (dy + 2) * (dy + 2) <= 3 {
        return Some(color);
    }
    if (dx + 1) * (dx + 1) + (dy - 4) * (dy - 4) <= 3 {
        return Some(color);
    }
    if (dx - 3) * (dx - 3) + (dy + 1) * (dy + 1) <= 3 {
        return Some(color);
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

/// Draws a checklist box: an ink rim when empty, a filled ink box with the
/// tick punched out of it when checked.
///
/// WHY the tick is `bg` and not a fixed colour: the box sits on a note whose
/// fill the user chose, and a tick painted in a fixed ink disappears the
/// moment the fill is that ink's own tone. The surface under the box is the
/// one colour the tick is guaranteed to contrast with.
#[allow(clippy::too_many_arguments)]
fn draw_checkbox_glyph(
    x: usize,
    y: usize,
    bx: usize,
    by: usize,
    is_checked: bool,
    ink: u32,
    rim: u32,
    bg: u32,
) -> Option<u32> {
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
            return Some(bg);
        }
        return Some(ink);
    } else {
        let in_inner = in_rounded_rect(x, y, bx + 2, by + 2, 18, 18, 4);
        if !in_inner {
            return Some(rim);
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

const STATUS_BAR_HEIGHT: usize = interos_std::design::viewport::STATUS_BAR_HEIGHT;

pub fn status_band() -> (usize, usize) {
    (0, STATUS_BAR_HEIGHT)
}

fn refresh_clock(state: &mut AppState) {
    if let Some(now) = std::rtc::Reading::now() {
        state.clock = now.clock_text();
        state.clock_ok = true;
    } else {
        state.clock_ok = false;
    }
}

fn render_pixel(x: usize, y: usize, state: &AppState) -> u32 {
    // 0. Status Band (y < STATUS_BAR_HEIGHT)
    if y < STATUS_BAR_HEIGHT {
        let band = interos_std::design::components::StatusBand::new(W);
        let clock = if state.clock_ok { Some(&state.clock[..]) } else { None };
        return band
            .pixel(x, y, clock, &interos_std::design::components::Instruments::unmeasured(), &THEME)
            .unwrap_or(THEME.surface);
    }

    // 1. The platform's home band: the pill's row down to the panel's last row
    //    is the shell's, and this app paints nothing into it.
    //
    // WHY the indicator is ink and not a translucent white: the bar is drawn
    // on the theme's page, and a white pill on a near-white page is a pill
    // nobody can see. The ink role is the strong mark a home indicator is.
    if y >= chrome::HOME_PILL.y {
        if let Some(c) = interos_std::design::home_band::pixel(x, y, THEME.on_surface) {
            return c;
        }
        return THEME.surface;
    }

    // 2. The platform's back control, on the header row of both views.
    //
    // WHY here and not inside each view: the chip owns one rectangle on every
    // screen this app draws, and the tap handler answers that same rectangle
    // once. Two painters of one control is how the list came to have one and
    // the editor came to have a different circle in its place.
    if let Some(c) = chrome::back_pixel(x, y) {
        return c;
    }

    // 3. View Mode Dispatch
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
        sn.p1 = &l1_txt[..l1_txt.len().min(38)];

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
                sn.p2 = &l2_txt[..l2_txt.len().min(38)];
            } else if !l2.is_empty() {
                sn.p2 = &l2[..l2.len().min(40)];
            }
        }
        return sn;
    }

    // Normal text wrapping without breaking words (Scale 2 Body: ~38 chars)
    let mut l1_nl = None;
    for i in 0..body.len().min(40) {
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
            while l2_end < rem.len() && rem[l2_end] != b'\n' && l2_end < 40 {
                l2_end += 1;
            }
            sn.p2 = &rem[..l2_end];
        }
    } else {
        // No newline in first 40 bytes: wrap at word boundary
        if body.len() <= 38 {
            sn.p1 = body;
        } else {
            let mut break_idx = None;
            for i in (14..=38.min(body.len())).rev() {
                if body[i - 1] == b' ' {
                    break_idx = Some(i - 1);
                    break;
                }
            }
            let p1_len = break_idx.unwrap_or(38.min(body.len()));
            sn.p1 = &body[..p1_len];

            let mut p2_start = p1_len;
            while p2_start < body.len() && body[p2_start] == b' ' {
                p2_start += 1;
            }
            if p2_start < body.len() {
                let rem = &body[p2_start..];
                if rem.len() <= 40 {
                    sn.p2 = rem;
                } else {
                    let mut p2_break = None;
                    for i in (14..=38.min(rem.len())).rev() {
                        if rem[i - 1] == b' ' {
                            p2_break = Some(i - 1);
                            break;
                        }
                    }
                    let p2_len = p2_break.unwrap_or(38.min(rem.len()));
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
    // --- A. Elevated Search Pill Bar (y: SEARCH_Y0..SEARCH_Y1, x: 28..692) ---
    if SEARCH_PILL.contains(x, y) {
        if in_rounded_rect(x, y, SEARCH_PILL.x, SEARCH_PILL.y, SEARCH_PILL.w, SEARCH_PILL.h, 24) {
            if !in_rounded_rect(x, y, SEARCH_PILL.x + 2, SEARCH_PILL.y + 2, SEARCH_PILL.w - 4, SEARCH_PILL.h - 4, 22) {
                return if state.search_active { THEME.primary } else { THEME.border };
            }

            // Search Icon, right of the back control
            if let Some(c) = draw_search_glyph(x, y, SEARCH_GLYPH_CX, SEARCH_Y0 + SEARCH_H / 2, THEME.on_surface_variant) {
                return c;
            }

            // Search Query Text or Placeholder (Scale 2 Body)
            if state.search_len > 0 {
                let q = &state.search_query[..state.search_len];
                // WHY the query is ellipsized: the buffer holds up to 30
                // bytes, which at scale 2 is wider than the pill's inner
                // width, so the run has to be measured before it is drawn.
                let mut q_buf = [0u8; 40];
                let q_len = std::design::font::ellipsize(q, 2, 696 - 20 - SEARCH_TEXT_X, &mut q_buf);
                let q = &q_buf[..q_len];
                if let Some(c) = test_text(x, y, SEARCH_TEXT_X, SEARCH_Y0 + 14, 2, q, THEME.on_surface, THEME.surface_container, false) {
                    return c;
                }
                // Blinking cursor
                if state.cursor_visible && state.search_active {
                    let cur_w = outline_text::drawn_advance(q, 2, None).unwrap_or_else(|| std::design::font::measure(q, 2));
                    let cur_x = SEARCH_TEXT_X + cur_w;
                    if x >= cur_x && x <= cur_x + 2 && y >= SEARCH_Y0 + 14 && y <= SEARCH_Y0 + 40 {
                        return THEME.primary;
                    }
                }
            } else {
                let hint = b"Search your notes...";
                if let Some(c) = test_text(x, y, SEARCH_TEXT_X, SEARCH_Y0 + 14, 2, hint, THEME.on_surface_variant, THEME.surface_container, false) {
                    return c;
                }
            }

            // Right Clear 'X' or Note Count Pill
            if state.search_len > 0 {
                let dx = (x as i32 - (SEARCH_CLEAR.x + SEARCH_CLEAR.w / 2) as i32).abs();
                let dy = (y as i32 - (SEARCH_CLEAR.y + SEARCH_CLEAR.h / 2) as i32).abs();
                if (dx - dy).abs() <= 1 && dx <= 7 {
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
                if let Some(c) = test_text(x, y, bx, SEARCH_Y0 + 18, 1, &badge[..n], THEME.primary, THEME.surface_container, true) {
                    return c;
                }
            }

            return THEME.surface_container;
        }
        return THEME.surface;
    }

    // --- B. Category Filter Chips and the Pin Filter (y: CHIP_Y0..CHIP_Y1) ---
    // Clean 16px gap below search bar, >= 48px touch targets
    if y >= CHIP_Y0 && y < CHIP_Y1 {
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
                if in_rounded_rect(x, y, cx, CHIP_Y0, cw, CHIP_H, 14) {
                    if is_active {
                        let txt_color = THEME.on_primary;
                        let text_w = std::design::font::measure(chip_names[i], 1);
                        let text_x = cx + (cw.saturating_sub(text_w)) / 2;
                        if let Some(c) = test_text(x, y, text_x, CHIP_Y0 + 15, 1, chip_names[i], txt_color, THEME.primary, true) {
                            return c;
                        }
                        return THEME.primary;
                    } else {
                        if !in_rounded_rect(x, y, cx + 1, CHIP_Y0 + 1, cw - 2, CHIP_H - 2, 13) {
                            return THEME.border;
                        }
                        let text_w = std::design::font::measure(chip_names[i], 1);
                        let text_x = cx + (cw.saturating_sub(text_w)) / 2;
                        if let Some(c) = test_text(x, y, text_x, CHIP_Y0 + 15, 1, chip_names[i], THEME.on_surface_variant, THEME.surface_container, false) {
                            return c;
                        }
                        return THEME.surface_container;
                    }
                }
            }
        }

        // The pin filter (>= 48px touch target).
        if in_rounded_rect(x, y, PIN_FILTER.0, PIN_FILTER.1, PIN_FILTER.2, PIN_FILTER.3, 14) {
            let active = state.pinned_only;
            let fill = if active { THEME.primary } else { THEME.surface_container };
            let ink = if active { THEME.on_primary } else { THEME.on_surface_variant };
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
            if let Some(c) = draw_pin_glyph(x, y, PIN_FILTER.0 + 22, PIN_FILTER.1 + 24, active, ink) {
                return c;
            }
            if let Some(c) = test_text(x, y, PIN_FILTER.0 + 40, CHIP_Y0 + 15, 1, b"PINNED", ink, fill, false) {
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
    if !state.keyboard_visible && y >= chrome::DOCK_PILL.y && y < chrome::DOCK_PILL.y + chrome::DOCK_PILL.h {
        // Right circular primary FAB (+) at (654, 1352, r=28)
        if in_circle(x, y, chrome::DOCK_FAB_CX, chrome::DOCK_FAB_CY, chrome::FAB_R) {
            // The glyph is drawn on the filled primary control, so its ink is
            // the control's on-colour; on_surface would be the same black as
            // the fill in the monochrome theme.
            if let Some(c) = draw_plus_glyph(x, y, chrome::DOCK_FAB_CX, chrome::DOCK_FAB_CY, THEME.on_primary) {
                return c;
            }
            return THEME.primary;
        }

        // Left Pill: "Take a note..." + Quick tools (x: 28..608)
        if in_rounded_rect(x, y, chrome::DOCK_PILL.x, chrome::DOCK_PILL.y, chrome::DOCK_PILL.w, chrome::DOCK_PILL.h, 26) {
            if !in_rounded_rect(x, y, chrome::DOCK_PILL.x + 2, chrome::DOCK_PILL.y + 2, chrome::DOCK_PILL.w - 4, chrome::DOCK_PILL.h - 4, 24) {
                return THEME.border;
            }

            // Checklist tool icon at x: 440
            if let Some(c) = draw_checkbox_glyph(
                x,
                y,
                chrome::DOCK_TOOL_CXS[0],
                chrome::DOCK_FAB_CY - 11,
                false,
                THEME.primary,
                THEME.on_surface_variant,
                THEME.surface_container,
            ) {
                return c;
            }

            // Palette tool icon at x: 500
            if let Some(c) = draw_palette_glyph(x, y, chrome::DOCK_TOOL_CXS[1], chrome::DOCK_FAB_CY, THEME.on_surface_variant) {
                return c;
            }

            // Search / edit tool icon at x: 560
            if let Some(c) = draw_keyboard_glyph(x, y, chrome::DOCK_TOOL_CXS[2], chrome::DOCK_FAB_CY, THEME.on_surface_variant) {
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

    // --- E. Dynamic Note Cards Feed (y: LIST_TOP-2..LIST_BOTTOM) ---
    // Clean 24px gap below category filter chips
    if y >= LIST_TOP - 2 && y <= LIST_BOTTOM {
        let mut filtered = [0usize; MAX_NOTES];
        let visible_count = state.filtered_indices(&mut filtered);

        if visible_count == 0 {
            // Empty state card
            if in_rounded_rect(x, y, EMPTY_CARD_X, EMPTY_CARD_Y, EMPTY_CARD_W, EMPTY_CARD_H, 24) {
                if !in_rounded_rect(
                    x,
                    y,
                    EMPTY_CARD_X + CARD_HAIRLINE,
                    EMPTY_CARD_Y + CARD_HAIRLINE,
                    EMPTY_CARD_W - 2 * CARD_HAIRLINE,
                    EMPTY_CARD_H - 2 * CARD_HAIRLINE,
                    22,
                ) {
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
                const CARD_X: usize = EMPTY_CARD_X;
                const CARD_W: usize = EMPTY_CARD_W;
                let title_x = CARD_X + std::design::font::center_x(title, 2, CARD_W);
                if let Some(c) = test_text(x, y, title_x, 500, 2, title, THEME.on_surface, THEME.surface_container, true) {
                    return c;
                }
                let mut sub_buf = [0u8; 96];
                let sub_len = std::design::font::ellipsize(subtitle, 1, EMPTY_SUBTITLE_LANE, &mut sub_buf);
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
            if iy >= cy0 && iy < cy1 && x >= LIST_CARD_X0 && x <= LIST_CARD_X1 {
                let theme = NOTE_TONES[note.color_idx as usize % NOTE_TONE_COUNT];

                // Rounded Card Frame (r=20)
                if !in_rounded_rect(
                    x,
                    y,
                    LIST_CARD_X0,
                    cy0 as usize,
                    LIST_CARD_W,
                    card_h,
                    CARD.radius,
                ) {
                    return THEME.surface;
                }
                // Card Border: the card's own hairline, one step inside it.
                if !in_rounded_rect(
                    x,
                    y,
                    LIST_CARD_INNER_X0,
                    (cy0 + CARD_HAIRLINE as isize) as usize,
                    LIST_CARD_INNER_X1 - LIST_CARD_INNER_X0,
                    card_h - 2 * CARD_HAIRLINE,
                    CARD.radius.saturating_sub(CARD_HAIRLINE),
                ) {
                    return theme.border;
                }

                let uy0 = cy0 as usize;

                // Left Accent Rail
                //
                // WHY it is inset and muted: a full-height pure-black bar on
                // a white card reads as a rendering stripe, not as the note's
                // mark, so only a pinned note may carry the tone's accent.
                if in_rounded_rect(
                    x,
                    y,
                    LIST_CARD_RAIL_X,
                    uy0 + 26,
                    notes::LIST_CARD_RAIL_W,
                    card_h - 52,
                    2,
                ) {
                    return if note.is_pinned { theme.accent } else { theme.muted_ink };
                }

                // 1. Top row: Note Title (Scale 2 Bold, y: uy0 + 20 .. uy0 + 44)
                let title = note.title_bytes();
                let max_title_w = if note.is_pinned {
                    LIST_CARD_TITLE_LANE_PINNED
                } else {
                    LIST_CARD_TITLE_LANE
                };
                let mut title_buf = [0u8; 96];
                let title_len = std::design::font::ellipsize(title, 2, max_title_w, &mut title_buf);
                let title_disp = if title_len > 0 { &title_buf[..title_len] } else { title };
                if let Some(c) = test_text(x, y, LIST_CARD_TEXT_X, uy0 + 20, 2, title_disp, theme.title_ink, theme.fill, true) {
                    return c;
                }

                // Pinned Vector Icon if pinned (x: PIN_GLYPH_CX, y: uy0 + 26)
                if note.is_pinned {
                    if let Some(c) = draw_pin_glyph(x, y, PIN_GLYPH_CX, uy0 + 26, true, theme.accent) {
                        return c;
                    }
                }

                // 2. Middle: Body preview lines with checklist support & word wrapping (Scale 2 Body)
                let body = note.body_bytes();
                if !body.is_empty() {
                    let sn = extract_card_snippet(body);

                    // Preview line 1 at uy0 + 56 (ends at uy0 + 82)
                    if !sn.p1.is_empty() {
                        // WHY the snippet is ellipsized at draw time: the
                        // extractor cuts by byte count, and a wide run at
                        // scale 2 runs past the card's inner edge, so the
                        // measured run is the one that gets painted.
                        let mut p1_buf = [0u8; 48];
                        let p1_max = if sn.p1_is_chk {
                            LIST_CARD_CHECK_SNIPPET_LANE
                        } else {
                            LIST_CARD_SNIPPET_LANE
                        };
                        let p1_len = std::design::font::ellipsize(sn.p1, 2, p1_max, &mut p1_buf);
                        let p1 = &p1_buf[..p1_len];
                        if sn.p1_is_chk {
                            if let Some(c) = draw_checkbox_glyph(
                                x,
                                y,
                                CHECKBOX_X,
                                uy0 + 58,
                                sn.p1_is_done,
                                theme.accent,
                                theme.muted_ink,
                                theme.fill,
                            ) {
                                return c;
                            }
                            let tcol = if sn.p1_is_done { theme.muted_ink } else { theme.body_ink };
                            if let Some(c) = test_text(x, y, CHECKBOX_TEXT_X, uy0 + 56, 2, p1, tcol, theme.fill, false) {
                                return c;
                            }
                            let p1_w = std::design::font::measure(p1, 2);
                            if sn.p1_is_done
                                && (y == uy0 + 69 || y == uy0 + 70)
                                && x >= CHECKBOX_TEXT_X
                                && x <= CHECKBOX_TEXT_X + p1_w
                            {
                                return theme.muted_ink;
                            }
                        } else {
                            if let Some(c) = test_text(x, y, LIST_CARD_TEXT_X, uy0 + 56, 2, p1, theme.body_ink, theme.fill, false) {
                                return c;
                            }
                        }
                    }

                    // Preview line 2 at uy0 + 90 (ends at uy0 + 116, clean 8px gap)
                    if !sn.p2.is_empty() {
                        let mut p2_buf = [0u8; 48];
                        let p2_max = if sn.p2_is_chk {
                            LIST_CARD_CHECK_SNIPPET_LANE
                        } else {
                            LIST_CARD_SNIPPET_LANE
                        };
                        let p2_len = std::design::font::ellipsize(sn.p2, 2, p2_max, &mut p2_buf);
                        let p2 = &p2_buf[..p2_len];
                        if sn.p2_is_chk {
                            if let Some(c) = draw_checkbox_glyph(
                                x,
                                y,
                                CHECKBOX_X,
                                uy0 + 92,
                                sn.p2_is_done,
                                theme.accent,
                                theme.muted_ink,
                                theme.fill,
                            ) {
                                return c;
                            }
                            let tcol = if sn.p2_is_done { theme.muted_ink } else { theme.body_ink };
                            if let Some(c) = test_text(x, y, CHECKBOX_TEXT_X, uy0 + 90, 2, p2, tcol, theme.fill, false) {
                                return c;
                            }
                            let p2_w = std::design::font::measure(p2, 2);
                            if sn.p2_is_done
                                && (y == uy0 + 103 || y == uy0 + 104)
                                && x >= CHECKBOX_TEXT_X
                                && x <= CHECKBOX_TEXT_X + p2_w
                            {
                                return theme.muted_ink;
                            }
                        } else {
                            if let Some(c) = test_text(x, y, LIST_CARD_TEXT_X, uy0 + 90, 2, p2, theme.body_ink, theme.fill, false) {
                                return c;
                            }
                        }
                    }
                }

                // 3. Bottom Row: Category Chip & Timestamp (y: uy0 + 144..176)
                let cat_name = CATEGORY_NAMES[note.category as usize % 5];
                if in_rounded_rect(x, y, 56, uy0 + 144, 88, 32, 10) {
                    if let Some(c) = test_text(x, y, 66, uy0 + 150, 1, cat_name, theme.accent, theme.chip, true) {
                        return c;
                    }
                    return theme.chip;
                }

                let mut ts_buf = [0u8; 32];
                let ts_len = format_note_timestamp(note, state.now_civil, &mut ts_buf);
                let ts_w = std::design::font::measure(&ts_buf[..ts_len], 1);
                let ts_x = 672usize.saturating_sub(ts_w);
                if let Some(c) = test_text(x, y, ts_x, uy0 + 152, 1, &ts_buf[..ts_len], theme.muted_ink, theme.fill, false) {
                    return c;
                }

                return theme.fill;
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
// View 2: Distraction-Free Note Editor (Google Keep UX)
// -----------------------------------------------------------------------------

fn render_note_editor(x: usize, y: usize, state: &AppState) -> u32 {
    let note = match state.store.get_note(state.selected_note_id) {
        Some(n) => n,
        None => return THEME.surface,
    };
    let theme = NOTE_TONES[note.color_idx as usize % NOTE_TONE_COUNT];

    // --- A. Top Navigation Bar (y: 0..EDITOR_BAR_H) ---
    if y < EDITOR_BAR_H {
        // The back control is painted by `render_pixel`, ahead of this bar, so
        // the toolbar carries only the note's own tools and its status run.

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
        let status_color = if state.save_failed { THEME.error } else { theme.muted_ink };
        if let Some(c) = test_text(x, y, 96, EDITOR_TOOL_CY - 8, 1, status_text, status_color, theme.fill, false) {
            return c;
        }

        // Right Tools:
        // 1. Pin Toggle Glyph (x: 520, on the toolbar's centre row)
        if in_circle(x, y, chrome::EDITOR_TOOL_CXS[0], chrome::EDITOR_TOOL_CY, chrome::TOOL_R) {
            // A pin is the note's own state, so it takes the tone's accent;
            // `warning` would report a caution the user never set.
            let pcol = if note.is_pinned { theme.accent } else { theme.muted_ink };
            if let Some(c) = draw_pin_glyph(x, y, chrome::EDITOR_TOOL_CXS[0], chrome::EDITOR_TOOL_CY, note.is_pinned, pcol) {
                return c;
            }
            return theme.chip;
        }

        // 2. Palette Toggle Glyph (x: 580, on the toolbar's centre row)
        if in_circle(x, y, chrome::EDITOR_TOOL_CXS[1], chrome::EDITOR_TOOL_CY, chrome::TOOL_R) {
            if let Some(c) = draw_palette_glyph(x, y, chrome::EDITOR_TOOL_CXS[1], chrome::EDITOR_TOOL_CY, theme.title_ink) {
                return c;
            }
            return theme.chip;
        }

        // 3. Trash Can Glyph (x: 640, on the toolbar's centre row)
        if in_circle(x, y, chrome::EDITOR_TOOL_CXS[2], chrome::EDITOR_TOOL_CY, chrome::TOOL_R) {
            if let Some(c) = draw_trash_glyph(x, y, chrome::EDITOR_TOOL_CXS[2], chrome::EDITOR_TOOL_CY, THEME.error) {
                return c;
            }
            return theme.chip;
        }

        // Subtle divider line
        if y == EDITOR_BAR_H - 2 {
            return theme.border;
        }

        return theme.fill;
    }

    // --- B. Note Title Area (y: EDITOR_TITLE_Y0..EDITOR_TITLE_Y1) ---
    // Clean, unruled modern typography with ample negative whitespace
    if y >= EDITOR_TITLE_Y0 && y < EDITOR_TITLE_Y1 {
        let title = note.title_bytes();
        // WHY the title is ellipsized to the lane the library derives: the pin,
        // palette and trash tools are circles whose plates begin at
        // `notes::EDITOR_FIRST_TOOL_X0`, so an unbounded title runs under them
        // and off-screen -- and the caret, placed from the same run, with it.
        let mut title_buf = [0u8; 64];
        let title_len = std::design::font::ellipsize(title, 2, EDITOR_TITLE_LANE, &mut title_buf);
        let title_disp = &title_buf[..title_len];
        if title.is_empty() {
            if let Some(c) = test_text(x, y, 44, EDITOR_TITLE_TEXT_Y, 2, b"Title", theme.muted_ink, theme.fill, true) {
                return c;
            }
        } else {
            if let Some(c) = test_text(x, y, 44, EDITOR_TITLE_TEXT_Y, 2, title_disp, theme.title_ink, theme.fill, true) {
                return c;
            }
        }

        // Title blinking cursor
        if state.focus == EditorFocus::Title && state.cursor_visible && state.keyboard_visible {
            let tw = outline_text::drawn_advance(title_disp, 2, None).unwrap_or_else(|| std::design::font::measure_bold(title_disp, 2));
            let cur_x = 44 + tw;
            if x >= cur_x && x <= cur_x + 2 && y >= EDITOR_TITLE_TEXT_Y - 2 && y <= EDITOR_TITLE_TEXT_Y + 28 {
                return theme.accent;
            }
        }

        return theme.fill;
    }

    // --- C. Tone Picker Popover Bar ---
    let swatch_band = notes::chrome::swatch_band_at(state.keyboard_visible);
    if state.swatch_picker_open && swatch_band.contains(x, y) {
        if in_rounded_rect(x, y, swatch_band.x, swatch_band.y, swatch_band.w, swatch_band.h, 22) {
            if !in_rounded_rect(x, y, swatch_band.x + 2, swatch_band.y + 2, swatch_band.w - 4, swatch_band.h - 4, 20) {
                return THEME.border;
            }

            let cy = notes::chrome::swatch_cy(swatch_band);
            for i in 0..NOTE_TONE_COUNT {
                let cx = notes::chrome::SWATCH_X0 + i * notes::chrome::SWATCH_PITCH;
                let tone = NOTE_TONES[i];

                // Active circle ring
                if i == note.color_idx as usize {
                    if in_circle(x, y, cx, cy, notes::chrome::SWATCH_R)
                        && !in_circle(x, y, cx, cy, notes::chrome::SWATCH_R - 4)
                    {
                        return THEME.on_surface;
                    }
                }

                if in_circle(x, y, cx, cy, notes::chrome::SWATCH_R - 4) {
                    if !in_circle(x, y, cx, cy, notes::chrome::SWATCH_R - 6) {
                        // WHY the picker's ring is the platform's secondary
                        // ink and not the tone's own hairline: the four paper
                        // tones are all near-white, and a near-white swatch
                        // edged with the card hairline on a white bar is a
                        // swatch with no edge. The ring is picker chrome, so
                        // it is drawn in picker ink.
                        return THEME.on_surface_variant;
                    }
                    return tone.fill;
                }
            }

            // One step below the popover's card, so the white and canvas
            // swatches have something to be white against.
            return THEME.surface_container_low;
        }
    }

    // --- D. Keyboard Dock & Accessory Strip ---
    if state.keyboard_visible {
        // The accessory strip's container, and the seven plates it carries.
        //
        // WHY the band is `STRIP` and not `y >= 916 && y <= 974`: the container
        // ends its ink at row 973 and the band ran one row past it, so a press
        // on the page below the strip answered a button the strip never drew.
        if notes::chrome::STRIP.contains(x, y) {
            let strip = notes::chrome::STRIP;
            if !in_rounded_rect(x, y, strip.x, strip.y, strip.w, strip.h, 18) {
                return THEME.surface;
            }

            // Checklist.
            let p = notes::chrome::STRIP_PLATES[0];
            if in_rounded_rect(x, y, p.x, p.y, p.w, p.h, 10) {
                if let Some(c) = draw_checkbox_glyph(
                    x,
                    y,
                    p.x + 16,
                    p.y + 13,
                    false,
                    THEME.primary,
                    THEME.on_surface_variant,
                    THEME.surface_container,
                ) {
                    return c;
                }
                return THEME.surface_container;
            }

            // Bullet.
            let p = notes::chrome::STRIP_PLATES[1];
            if in_rounded_rect(x, y, p.x, p.y, p.w, p.h, 10) {
                if let Some(c) = test_text(x, y, p.x + 22, p.y + 15, 1, b"*", THEME.on_surface, THEME.surface_container, true) {
                    return c;
                }
                return THEME.surface_container;
            }

            // Palette.
            let p = notes::chrome::STRIP_PLATES[2];
            if in_rounded_rect(x, y, p.x, p.y, p.w, p.h, 10) {
                if let Some(c) = draw_palette_glyph(x, y, p.x + 27, p.y + 24, THEME.on_surface) {
                    return c;
                }
                return THEME.surface_container;
            }

            // Category cycle pill.
            let cat_name = CATEGORY_NAMES[note.category as usize % 5];
            let p = notes::chrome::STRIP_PLATES[3];
            if in_rounded_rect(x, y, p.x, p.y, p.w, p.h, 10) {
                let text_w = std::design::font::measure(cat_name, 1);
                let text_x = p.x + (p.w.saturating_sub(text_w)) / 2;
                if let Some(c) = test_text(x, y, text_x, p.y + 15, 1, cat_name, THEME.primary, THEME.surface_container, true) {
                    return c;
                }
                return THEME.surface_container;
            }

            // Cursor left.
            let p = notes::chrome::STRIP_PLATES[4];
            if in_rounded_rect(x, y, p.x, p.y, p.w, p.h, 10) {
                if let Some(c) = draw_chevron_left(x, y, p.x + 28, p.y + 24, THEME.on_surface) {
                    return c;
                }
                return THEME.surface_container;
            }

            // Cursor right.
            let p = notes::chrome::STRIP_PLATES[5];
            if in_rounded_rect(x, y, p.x, p.y, p.w, p.h, 10) {
                if let Some(c) = draw_chevron_right(x, y, p.x + 28, p.y + 24, THEME.on_surface) {
                    return c;
                }
                return THEME.surface_container;
            }

            // Hide keyboard.
            let p = notes::chrome::STRIP_PLATES[6];
            if in_rounded_rect(x, y, p.x, p.y, p.w, p.h, 10) {
                if let Some(c) = draw_chevron_down(x, y, p.x + 31, p.y + 24, THEME.primary) {
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
        if y >= chrome::READING_DOCK.y && y < chrome::READING_DOCK.y + chrome::READING_DOCK.h {
            if in_rounded_rect(x, y, chrome::READING_DOCK.x, chrome::READING_DOCK.y, chrome::READING_DOCK.w, chrome::READING_DOCK.h, 28) {
                if !in_rounded_rect(x, y, chrome::READING_DOCK.x + 2, chrome::READING_DOCK.y + 2, chrome::READING_DOCK.w - 4, chrome::READING_DOCK.h - 4, 26) {
                    return THEME.border;
                }

                // Category Tag
                let cat_name = CATEGORY_NAMES[note.category as usize % 5];
                if in_rounded_rect(x, y, chrome::READING_TAG.x, chrome::READING_TAG.y, chrome::READING_TAG.w, chrome::READING_TAG.h, 12) {
                    if let Some(c) = test_text(x, y, chrome::READING_TAG.x + 20, chrome::READING_TAG.y + 8, 1, cat_name, theme.accent, theme.chip, true) {
                        return c;
                    }
                    return theme.chip;
                }

                // Checklist tool
                if in_circle(x, y, chrome::READING_TOOL_CXS[0], chrome::READING_TOOL_CY, chrome::TOOL_R) {
                    if let Some(c) = draw_checkbox_glyph(
                        x,
                        y,
                        229,
                        1347,
                        false,
                        THEME.primary,
                        THEME.on_surface_variant,
                        THEME.surface_container,
                    ) {
                        return c;
                    }
                    return THEME.surface_container;
                }

                // Palette tool
                if in_circle(x, y, chrome::READING_TOOL_CXS[1], chrome::READING_TOOL_CY, chrome::TOOL_R) {
                    if let Some(c) = draw_palette_glyph(x, y, chrome::READING_TOOL_CXS[1], chrome::READING_TOOL_CY, THEME.on_surface) {
                        return c;
                    }
                    return THEME.surface_container;
                }

                // Type / Edit note button (Keyboard icon)
                if in_circle(x, y, chrome::READING_TOOL_CXS[2], chrome::READING_TOOL_CY, chrome::TOOL_R) {
                    if let Some(c) = draw_keyboard_glyph(x, y, chrome::READING_TOOL_CXS[2], chrome::READING_TOOL_CY, THEME.primary) {
                        return c;
                    }
                    return THEME.surface_container;
                }

                // Delete Note icon
                if in_circle(x, y, chrome::READING_TOOL_CXS[3], chrome::READING_TOOL_CY, chrome::TOOL_R) {
                    if let Some(c) = draw_trash_glyph(x, y, chrome::READING_TOOL_CXS[3], chrome::READING_TOOL_CY, THEME.error) {
                        return c;
                    }
                    return THEME.surface_container;
                }

                return THEME.surface_container;
            }
        }
    }

    // --- E. Multiline Note Body Area (Scale 2 Body) ---
    // Starts at EDITOR_TEXT_Y0 (clean 26px below the title's text)
    let body_max_y = if state.keyboard_visible { 910 } else { 1318 };
    if y >= EDITOR_BODY_Y0 && y < body_max_y {
        // The ruled page.
        //
        // WHY the page is ruled: the design document asks for a ruled note
        // page, and an editor with no geometry under the text gives the
        // writer nothing to write on. The rule belongs to the note, so it
        // is drawn in the note's own border colour and not in the app's.
        if y >= EDITOR_TEXT_Y0 + EDITOR_RULE_DY && x >= RULE_X0 && x < RULE_X1 {
            let line_index = (y - EDITOR_TEXT_Y0 - EDITOR_RULE_DY) / EDITOR_LINE_STRIDE;
            if (y - EDITOR_TEXT_Y0 - EDITOR_RULE_DY) % EDITOR_LINE_STRIDE == 0 {
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
            if let Some(c) = test_text(x, y, 44, EDITOR_TEXT_Y0 + 4, 2, b"Note", theme.muted_ink, theme.fill, false) {
                return c;
            }
        } else {
            let body_bytes = state.editor_buf.as_bytes();
            for line_idx in 0..state.line_spans_count {
                let span = state.line_spans[line_idx];
                let line_y = EDITOR_TEXT_Y0 + line_idx * EDITOR_LINE_STRIDE;

                if y >= line_y && y < line_y + EDITOR_LINE_STRIDE {
                    let line_slice = &body_bytes[span.start..span.end];
                    let is_chk = line_slice.starts_with(b"[ ] ") || line_slice.starts_with(b"[x] ") || line_slice.starts_with(b"[X] ");
                    let is_done = line_slice.starts_with(b"[x] ") || line_slice.starts_with(b"[X] ");

                    if is_chk {
                        // Draw Checkbox vertically centered at line_y + 6 (22px in 40px line)
                        if let Some(c) = draw_checkbox_glyph(
                            x,
                            y,
                            44,
                            line_y + 6,
                            is_done,
                            theme.accent,
                            theme.muted_ink,
                            theme.fill,
                        ) {
                            return c;
                        }
                        // Text starts after "[ ] " at x: 82, vertically aligned with checkbox at line_y + 4
                        let text_slice = if line_slice.len() > 4 { &line_slice[4..] } else { b"" };
                        let tcol = if is_done { theme.muted_ink } else { theme.body_ink };
                        if let Some(c) = test_text(x, y, 82, line_y + 4, 2, text_slice, tcol, theme.fill, false) {
                            return c;
                        }
                        // Clean horizontal strikethrough through center of lowercase x-height
                        let text_w = std::design::font::measure(text_slice, 2);
                        if is_done && (y == line_y + 17 || y == line_y + 18) && x >= 82 && x <= 82 + text_w {
                            return theme.muted_ink;
                        }
                    } else {
                        // Normal line at x: 44, y: line_y + 4
                        if let Some(c) = test_text(x, y, 44, line_y + 4, 2, line_slice, theme.body_ink, theme.fill, false) {
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
            let line_slice = if span.end > span.start { &body_bytes[span.start..span.end] } else { b"" };
            let text_slice = if is_chk && line_slice.len() >= 4 { &line_slice[4..] } else { line_slice };
            let prefix_col = cur_col.saturating_sub(if is_chk { 4 } else { 0 }).min(text_slice.len());
            let cur_adv = std::design::font::measure(&text_slice[..prefix_col], 2);
            let cur_x = text_x0 + cur_adv;
            let cur_y = EDITOR_TEXT_Y0 + cur_line * EDITOR_LINE_STRIDE + 4;
            if x >= cur_x && x <= cur_x + 2 && y >= cur_y && y <= cur_y + 24 {
                return theme.accent;
            }
        }

        return theme.fill;
    }

    theme.fill
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
                        let lw = std::design::font::measure(label, 1);
                        if let Some(c) = test_text(x, y, kx + (kw.saturating_sub(lw)) / 2, top + 26, 1, label, THEME.on_surface, fill, true) {
                            return c;
                        }
                    }
                    KeyAction::Symbols => {
                        let label: &[u8] = if state.kb_symbols { b"ABC" } else { b"?123" };
                        let lw = std::design::font::measure(label, 1);
                        if let Some(c) = test_text(x, y, kx + (kw.saturating_sub(lw)) / 2, top + 26, 1, label, THEME.on_surface, fill, true) {
                            return c;
                        }
                    }
                    KeyAction::Backspace => {
                        let label = b"<X>";
                        let lw = std::design::font::measure(label, 1);
                        if let Some(c) = test_text(x, y, kx + (kw.saturating_sub(lw)) / 2, top + 26, 1, label, THEME.on_surface, fill, true) {
                            return c;
                        }
                    }
                    KeyAction::Space => {
                        let label = b"Space";
                        let lw = std::design::font::measure(label, 1);
                        if let Some(c) = test_text(x, y, kx + (kw.saturating_sub(lw)) / 2, top + 26, 1, label, THEME.on_surface_variant, fill, false) {
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
    // 1. The home strip, at the rows below the keyboard's last row.
    //
    // WHY the keyboard's bottom edge is the strip's top: the band used to
    // begin at row 1400, inside the soft keyboard's own `980..1410`, so a
    // finger that pressed a key in the keyboard's bottom row between x=240
    // and x=480 quit the app instead of typing -- and the strip it quit from
    // was two hundred rows wider than the pill the app inks there.
    if chrome::home_claims(ux, uy) {
        return TapAction::Exit;
    }

    // 2. The platform's back control, before any row that shares its rect.
    //
    // WHY first: the list's search pill and the editor's toolbar both run
    // under the chip's square, and whichever of them tested the point first
    // took the tap -- which is how the control that is painted on the header
    // row answered nothing at all.
    if chrome::back_claims(ux, uy) {
        if state.view == ViewMode::Editor {
            state.sync_editor_to_note();
            state.save_to_storage();
            state.view = ViewMode::List;
            state.sync_list_extent();
            state.keyboard_visible = false;
            state.swatch_picker_open = false;
            return TapAction::Redraw;
        }
        std::log("[NOTES APP] Back control pressed at root. Exiting to launcher.").ok();
        return TapAction::Exit;
    }

    match state.view {
        ViewMode::List => {
            // Dismiss keyboard if tapped outside keyboard when keyboard is open
            if state.keyboard_visible && uy < KB_Y0 && uy > SEARCH_Y1 {
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

            // Search Bar Tap (y: SEARCH_Y0..SEARCH_Y1, x: 28..692)
            if SEARCH_PILL.contains(ux, uy) {
                if state.search_len > 0 && chrome::search_clear_claims(ux, uy) {
                    state.search_len = 0;
                    state.search_query = [0; 32];
                    return TapAction::Redraw;
                }
                state.search_active = true;
                state.keyboard_visible = true;
                return TapAction::Redraw;
            }

            // Category Filter Chips Tap (y: CHIP_Y0..CHIP_Y1, touch target >= 48px)
            if uy >= CHIP_Y0 && uy < CHIP_Y1 {
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

            // Bottom Action Bar Dock: the pill and the button beside it.
            //
            // WHY the press is asked through `chrome::dock_tool_at`: the dock
            // drew its three glyphs at 440, 500 and 560 and answered them at
            // 415..465, 475..525 and 535..608, so every band stood a few rows
            // right of the mark it was named for and the ten rows between the
            // palette and the keyboard answered nothing at all.
            if chrome::DOCK_PILL.contains(ux, uy)
                || in_circle(ux, uy, chrome::DOCK_FAB_CX, chrome::DOCK_FAB_CY, chrome::FAB_R)
            {
                let dock_tool = chrome::dock_tool_at(ux, uy);
                // FAB (+) or "Take a note..." tap
                if dock_tool.is_none() {
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
                if dock_tool == Some(0) {
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
                if dock_tool == Some(1) {
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

                // Keyboard tool tap (x: 535..608, centred on the glyph at 560)
                //
                // WHY this band exists: the dock drew a keyboard glyph and its
                // three bands stopped at x=525, so the one icon that promises a
                // keyboard answered nothing and the person pressed an inert
                // picture. It now does what the icon says -- it focuses the
                // list's search field and raises the keyboard over it, the same
                // pair the search bar itself sets. It runs to the pill's own
                // right edge at 608 so no part of the pill is left inert.
                if dock_tool == Some(2) {
                    state.search_active = true;
                    state.keyboard_visible = true;
                    return TapAction::Redraw;
                }
            }

            // Card Deck Taps (y: LIST_TOP-2..LIST_BOTTOM)
            if uy >= LIST_TOP - 2 && uy <= LIST_BOTTOM {
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
            // Top Bar: Pin Toggle on the toolbar's centre row
            if in_circle(ux, uy, chrome::EDITOR_TOOL_CXS[0], chrome::EDITOR_TOOL_CY, chrome::TOOL_R) {
                if let Some(note) = state.store.get_note_mut(state.selected_note_id) {
                    note.toggle_pinned();
                }
                return TapAction::Redraw;
            }

            // Top Bar: Palette Toggle on the toolbar's centre row
            if in_circle(ux, uy, chrome::EDITOR_TOOL_CXS[1], chrome::EDITOR_TOOL_CY, chrome::TOOL_R) {
                state.swatch_picker_open = !state.swatch_picker_open;
                return TapAction::Redraw;
            }

            // Top Bar: Trash / Delete on the toolbar's centre row
            if in_circle(ux, uy, chrome::EDITOR_TOOL_CXS[2], chrome::EDITOR_TOOL_CY, chrome::TOOL_R) {
                state.store.delete_note(state.selected_note_id);
                state.sync_list_extent();
                state.save_to_storage();
                state.view = ViewMode::List;
                state.keyboard_visible = false;
                state.swatch_picker_open = false;
                return TapAction::Redraw;
            }

            // Title Area Tap (y: EDITOR_TITLE_Y0..EDITOR_TITLE_Y1)
            if uy >= EDITOR_TITLE_Y0 && uy < EDITOR_TITLE_Y1 && ux >= 40 && ux <= 680 {
                state.focus = EditorFocus::Title;
                state.keyboard_visible = true;
                return TapAction::Redraw;
            }

            // The tone picker's swatches, answered through the same band and
            // the same pitch the popover paints.
            if state.swatch_picker_open {
                if let Some(i) = chrome::swatch_at(ux, uy, state.keyboard_visible) {
                    if let Some(note) = state.store.get_note_mut(state.selected_note_id) {
                        note.set_color(i as u8);
                    }
                    return TapAction::Redraw;
                }
            }

            // When Keyboard is visible
            if state.keyboard_visible {
                // The accessory strip's seven plates.
                //
                // WHY the press is asked through `chrome::strip_plate_at`: the
                // strip drew each plate as `x, 920, w, 48` and answered it as
                // `ux >= x && ux <= x + w`, so every button's press reached one
                // to three columns past its own ink -- the cursor steps, at
                // 350..406 and 418..474 of plate, answered 350..408 and
                // 418..476. The band itself ran to row 974 while the container
                // ended its ink at 973, so a press on the page below the strip
                // pressed a key. Both sides now read the same array.
                if let Some(plate) = chrome::strip_plate_at(ux, uy) {
                    match plate {
                        0 => {
                            state.editor_buf.insert_checklist_item();
                            state.update_editor_layout();
                            return TapAction::Redraw;
                        }
                        1 => {
                            state.editor_buf.insert_str("\n* ");
                            state.update_editor_layout();
                            return TapAction::Redraw;
                        }
                        2 => {
                            state.swatch_picker_open = !state.swatch_picker_open;
                            return TapAction::Redraw;
                        }
                        3 => {
                            if let Some(note) = state.store.get_note_mut(state.selected_note_id) {
                                note.category = (note.category + 1) % 5;
                                note.is_dirty = true;
                            }
                            return TapAction::Redraw;
                        }
                        4 => {
                            state.editor_buf.move_cursor_left();
                            state.update_editor_layout();
                            return TapAction::Redraw;
                        }
                        5 => {
                            state.editor_buf.move_cursor_right();
                            state.update_editor_layout();
                            return TapAction::Redraw;
                        }
                        _ => {
                            state.keyboard_visible = false;
                            state.swatch_picker_open = false;
                            return TapAction::Redraw;
                        }
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
                if uy >= chrome::READING_DOCK.y && uy < chrome::READING_DOCK.y + chrome::READING_DOCK.h {
                    // Category cycle pill: the plate the dock draws.
                    if chrome::READING_TAG.contains(ux, uy) {
                        if let Some(note) = state.store.get_note_mut(state.selected_note_id) {
                            note.category = (note.category + 1) % 5;
                            note.is_dirty = true;
                        }
                        return TapAction::Redraw;
                    }
                    // Checkbox tool
                    if in_circle(ux, uy, chrome::READING_TOOL_CXS[0], chrome::READING_TOOL_CY, chrome::TOOL_R) {
                        state.editor_buf.insert_checklist_item();
                        state.update_editor_layout();
                        state.keyboard_visible = true;
                        return TapAction::Redraw;
                    }
                    // Palette tool
                    if in_circle(ux, uy, chrome::READING_TOOL_CXS[1], chrome::READING_TOOL_CY, chrome::TOOL_R) {
                        state.swatch_picker_open = !state.swatch_picker_open;
                        return TapAction::Redraw;
                    }
                    // Edit / Keyboard button
                    if in_circle(ux, uy, chrome::READING_TOOL_CXS[2], chrome::READING_TOOL_CY, chrome::TOOL_R) {
                        state.focus = EditorFocus::Body;
                        state.keyboard_visible = true;
                        return TapAction::Redraw;
                    }
                    // Delete button
                    if in_circle(ux, uy, chrome::READING_TOOL_CXS[3], chrome::READING_TOOL_CY, chrome::TOOL_R) {
                        state.store.delete_note(state.selected_note_id);
                        state.sync_list_extent();
                        state.save_to_storage();
                        state.view = ViewMode::List;
                        return TapAction::Redraw;
                    }
                }
            }

            // Body Area Taps: Checkbox toggles and text positioning (Scale 2 Body)
            let body_max_y = if state.keyboard_visible { 910 } else { 1318 };
            if uy >= EDITOR_BODY_Y0 && uy <= body_max_y && ux >= 28 && ux <= 684 {
                let line_idx = (uy.saturating_sub(EDITOR_TEXT_Y0)) / EDITOR_LINE_STRIDE;
                if line_idx < state.line_spans_count {
                    let span = state.line_spans[line_idx];
                    // Check if tap hit the checkbox directly at x: 28..80 (>= 48px touch target)
                    if ux >= 28 && ux <= 80 {
                        if state.editor_buf.toggle_checklist_at(span.start) {
                            state.update_editor_layout();
                            if let Some(note) = state.store.get_note_mut(state.selected_note_id) {
                                note.is_dirty = true;
                            }
                            return TapAction::Redraw;
                        }
                    }

                    // Otherwise focus body and place cursor with tactile precision
                    state.focus = EditorFocus::Body;
                    state.keyboard_visible = true;
                    let line_slice = &state.editor_buf.as_bytes()[span.start..span.end];
                    let is_chk = line_slice.starts_with(b"[ ] ") || line_slice.starts_with(b"[x] ") || line_slice.starts_with(b"[X] ");
                    let text_x0 = if is_chk { 82 } else { 44 };
                    let text_slice = if is_chk && line_slice.len() >= 4 { &line_slice[4..] } else { line_slice };
                    let target_dx = ux.saturating_sub(text_x0);
                    let mut best_col = 0;
                    let mut best_diff = target_dx;
                    for char_idx in 1..=text_slice.len() {
                        let w = std::design::font::measure(&text_slice[..char_idx], 2);
                        let diff = if w >= target_dx { w - target_dx } else { target_dx - w };
                        if diff < best_diff {
                            best_diff = diff;
                            best_col = char_idx;
                        }
                    }
                    let base = span.start + if is_chk { 4 } else { 0 };
                    state.editor_buf.cursor = (base + best_col).min(span.end);
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
    // WHY the flags are cleared before the blob is built: `is_dirty` is part
    // of the serialized record, so a blob written while the flags were still
    // set would persist "unsaved" for the very edit this call is saving, and
    // the next launch would misreport a saved note. A failed write restores
    // the flags so the in-memory state still tells the truth.
    let mut was_dirty = [false; MAX_NOTES];
    for i in 0..store.count() {
        if let Some(note) = store.get_by_index_mut(i) {
            was_dirty[i] = note.is_dirty;
            note.is_dirty = false;
        }
    }

    if persist_store(store) {
        return true;
    }

    for i in 0..store.count() {
        if was_dirty[i] {
            if let Some(note) = store.get_by_index_mut(i) {
                note.is_dirty = true;
            }
        }
    }
    false
}

fn persist_store(store: &NotesStore) -> bool {
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
        line_spans: [LineSpan { start: 0, end: 0 }; 48],
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
        clock: [b' '; 5],
        clock_ok: false,
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
    refresh_clock(&mut state);
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
        //
        // WHY the redraw follows the reading rather than the counter: the
        // list's timestamps are minute-granular, so a per-second redraw
        // painted the same page sixty times a minute. Measured on the guest,
        // that made a backgrounded Notes the busiest app on the device --
        // 129 timeslices in twelve seconds against the shell's two -- and the
        // render it paid for was a full 720x1440 surface nobody was looking
        // at.
        clock_counter += 1;
        if clock_counter >= 20 {
            clock_counter = 0;
            let read = current_civil_time();
            if read != state.now_civil {
                state.now_civil = read;
                if state.view == ViewMode::List {
                    needs_redraw = true;
                }
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
