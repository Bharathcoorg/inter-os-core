//! InterOS Native Sovereign File Manager & VFS Explorer App
//!
//! Native Ring-3 graphical application executing in User Mode:
//! - DRM/KMS Client Surface allocation (z=200) and double-buffered scanout presentation
//! - Two screens: a home screen (storage insight, the index's recents and the
//!   four directory entry points) and a browse screen (breadcrumbs, scoped
//!   search, a list or a grid, multi-select and the file actions)
//! - Real directory listings through `fs_list`, real document facts through the
//!   platform's content index and `fs_stat`
//! - Banded rendering: only the rows an input actually dirtied are re-rasterised
//! - Asynchronous event reactor loop handling touches, long-press selection,
//!   pans and home gestures
//! - Graceful lifecycle supervision and clean return to Launcher
//!
//! WHY every value on screen is a platform answer: this app used to carry a
//! fabricated inventory, a fabricated clock and a fabricated storage bar. A
//! file browser that invents a file is worse than one that says "not reported",
//! so every line drawn here comes from a VFS call, the document index, the
//! intent registry or a `storage_query` answer.

#![no_std]
#![no_main]

use core::panic::PanicInfo;
use interos_std as std;
use file_manager::theme::{THEME, spacing};
use std::design::components;
use std::design::font;
use std::design::outline_text;
use std::design::primitives::{draw_pill, draw_rounded_rect, Rect};
use std::design::scroll::ScrollState;
pub use file_manager::{
    action_for_mime, detect_mime, fmt_u64, has_suffix, parse_vfs_entries, document_from_index,
    document_from_path, FileMime, RuntimeFileEntry, SelectedDocument, MAX_DOCUMENT_PATH_LEN,
    MAX_ENTRY_NAME_LEN, MAX_PARSED_ENTRIES,
};
// The layout, the scroll geometry and the file model live in the library so
// the renderer and the hit test can call the same functions.
use file_manager::{
    action_for_slot, action_slot_rect, action_supported, apply_search_scope, breadcrumb_crumbs,
    build_details_with_provenance, category_row_at, entry_at_point, entry_row_rect, format_mtime,
    format_size_human, grid_at_point, grid_cell_rect, grid_scroll, join_path, list_scroll,
    parse_indexed_documents, problem_text, row_is_visible, search_needle_is_usable,
    sheet_button_rect, sheet_destination_rect, sort_documents, sort_entries, sort_recents,
    validate_operation, Crumb, DetailsView, FileAction, IndexedDocument, LayoutMode,
    MAX_DETAILS_LINE, MAX_PATH_LEN, MAX_SEARCH_RESULTS, OpProblem, SearchScope, Selection,
    SortMode, TypedField, ViewMode, ACTION_BAR_Y0, ACTION_BAR_Y1, ACTION_LABELS,
    BREADCRUMB_HIT_Y0, BREADCRUMB_HIT_Y1, BREADCRUMB_Y, CATEGORY_COUNT, CATEGORY_Y0, CATEGORY_Y1,
    DETAILS_CARD, DETAILS_CLOSE, DETAILS_LINE_H, DETAILS_LINE_Y0, DETAILS_OPEN, DETAILS_TITLE_Y,
    ENTRIES_CARD, GRID_CELL_H, GRID_CELL_W, HEADER_H, HOME_BROWSE_BTN,
    HOME_CATEGORY_TITLE_Y, HOME_NOTE_Y, HOME_RECENTS_CAPTION_Y, HOME_RECENTS_TITLE_Y,
    HOME_RECENT_ROW_H, HOME_RECENTS_VISIBLE, HOME_RECENTS_Y0, HOME_STORAGE_CARD, LINE_LEN,
    LIST_PILL, GRID_PILL, SCOPE_ALL_PILL, SCOPE_CAPTION_MAX_W, SCOPE_THIS_PILL, SCOPE_Y,
    SEARCH_CLEAR, SEARCH_FIELD, SHEET, SHEET_NAME_H, SHEET_NAME_Y, SORT_PILL,
    STATUS_Y, TOOLBAR_Y,
};

/// The platform's list-row treatment, at the two states the file list uses.
///
/// WHY the file list is the platform's list row: a row that invents its own
/// fill, its own divider and its own padding is a row a third-party app
/// cannot match, and the platform already says what a row looks like.
const ROW: components::ListRowStyle = components::list_row_style(&THEME, false);
const ROW_SELECTED: components::ListRowStyle = components::list_row_style(&THEME, true);

const W: usize = 720;
const H: usize = 1440;
const CHUNK: usize = 4096;

/// The details card's handler line, and the notice shown when the registry
/// answers that no installed package declared the document's type.
const OPEN_WITH_LABEL: &[u8] = b"Open with: ";
const NO_HANDLER_NOTICE: &[u8] = b"no app handles this file";
/// Shown when the registry named a handler but the platform refused to launch
/// it. Saying "no app handles this file" there would be false.
const HANDLER_REFUSED_NOTICE: &[u8] = b"the app for this file could not launch";

// -----------------------------------------------------------------------------
// Header geometry - single source of truth for drawing and hit-testing
// -----------------------------------------------------------------------------
/// Screen margin (8pt grid value). Also the left inset of the back control and
/// the right inset the storage readout is aligned to.
const HEADER_MARGIN: usize = spacing::GRID_24;

/// The header's back control: a 52x52 pill in the strip above the title.
///
/// WHY the back control exists at all: it is painted from the same rectangle
/// the tap handler tests, so the region that leaves the screen is visible.
const BACK: Rect = Rect::new(HEADER_MARGIN, 8, 52, 52);

/// The system home indicator, and the strip that contains it.
const HOME_HIT: Rect = Rect::new(240, 1392, 240, 48);
const HOME_PILL: Rect = Rect::new(260, 1418, 200, 6);

/// Right edge the header readout is aligned to (the 24px right margin).
const READOUT_X1: usize = W - HEADER_MARGIN;
/// Widest a header readout run may be. The title ends at x 180, so a readout
/// longer than `READOUT_X1 - 180` would collide with it; `font::ellipsize`
/// trims to this instead of letting a long path run under the title.
const READOUT_MAX_W: usize = 400;
const READOUT_LINE1_Y0: usize = 68;
const READOUT_LINE2_Y0: usize = READOUT_LINE1_Y0 + font::line_height(1) + 8;

/// Ink for an affordance that is painted but has nothing behind it.
#[allow(dead_code)]
const DISABLED: u32 = THEME.border;

// -----------------------------------------------------------------------------
// Rasterisers
// -----------------------------------------------------------------------------

/// Rasterises one text run into the pixel at `(x, y)`.
///
/// `bg` is the fill the run is drawn on top of, because the anti-aliased glyph
/// edges are blended toward what is actually under them.
///
/// WHY the outline face comes first: the title is drawn at scale 3 and section
/// headings at scale 2, so on the bitmap face both are a 16x24 cell magnified
/// by an integer -- a soft ramp where an edge belongs.
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
) -> Option<u32> {
    if x < x0 || x >= W || y < y0 || y >= y0 + std::design::font::line_height(scale) {
        return None;
    }
    if let Some(run) = outline_text::place_run(x0, y0, scale, text, None, false) {
        return run.pixel(x, y, color, bg);
    }
    std::design::font::test_text_aa(x, y, x0, y0, scale, text, color, bg, false)
}

#[inline(always)]
fn centered_x(text: &[u8], scale: usize, x0: usize, available: usize) -> usize {
    let w = outline_text::drawn_advance(text, scale, Some(available))
        .unwrap_or_else(|| std::design::font::measure(text, scale));
    x0 + if w >= available { 0 } else { (available - w) / 2 }
}

// -----------------------------------------------------------------------------
// Counted log assembly (no allocator in this no_std app)
// -----------------------------------------------------------------------------

fn log_counted(prefix: &str, value: u64, suffix: &str) {
    let mut buf = [0u8; 192];
    let p = prefix.len().min(buf.len());
    buf[..p].copy_from_slice(&prefix.as_bytes()[..p]);
    let v = fmt_u64(value, &mut buf[p..]);
    let n = p + v;
    let s = suffix.len().min(buf.len().saturating_sub(n));
    buf[n..n + s].copy_from_slice(&suffix.as_bytes()[..s]);
    if let Ok(msg) = core::str::from_utf8(&buf[..n + s]) {
        std::log(msg).ok();
    }
}

fn log_listing(path: &str, bytes: u64, suffix: &str) {
    std::log("[FILE_MANAGER] fs_list(").ok();
    std::log(path).ok();
    std::log(") answered with ").ok();
    log_counted("", bytes, suffix);
}

fn log_listing_refused(path: &str, code: i64) {
    std::log("[FILE_MANAGER] fs_list(").ok();
    std::log(path).ok();
    std::log(") refused, code -").ok();
    log_counted("", code.unsigned_abs(), "; no directory data received.");
}

// -----------------------------------------------------------------------------
// VFS entry representation
// -----------------------------------------------------------------------------

/// The app ships no embedded file inventory: `ITEMS_ALL` stays empty until a
/// real VFS listing can populate it (WHY: the platform truth contract pins the
/// empty ledger, and nothing on screen may invent file names).
#[derive(Debug, Clone, Copy)]
pub struct VfsItem {
    pub name: &'static str,
    pub is_dir: bool,
    pub size_str: &'static str,
    pub meta_str: &'static str,
    pub color_accent: u32,
}

#[allow(dead_code)]
const ITEMS_ALL: [VfsItem; 0] = [];

/// Default probe label before the syscall has run: a real statement, not a
/// placeholder that could be mistaken for a result.
const PROBE_DEFAULT: [u8; 40] =
    *b"not queried\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0";

/// What the last directory listing actually produced.
///
/// WHY this is an enum and not a byte count: "the directory is empty" and
/// "the device refused to read the directory" are different facts, and a
/// screen that showed one headline for both told the user their files were
/// gone when the app had only been refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ListingState {
    /// No listing has been attempted yet.
    NotListed,
    /// The listing succeeded and the directory holds no entries.
    Empty,
    /// The listing succeeded and the directory holds entries.
    Listed,
    /// The platform refused the listing.
    Refused,
}

/// The directories this app can actually read.
///
/// WHY these four: the VFS mounts `/storage` with `/storage/photos`,
/// `/storage/pictures` and `/storage/videos`, and the kernel's app path
/// policy allows this app to list exactly those. Every other path -- `/data`
/// included -- is refused before the path is examined.
pub const CATEGORY_PATHS: [&str; 4] = ["/storage", "/storage/photos", "/storage/pictures", "/storage/videos"];

/// The label each real directory is shown under.
pub const CATEGORY_LABELS: [&[u8]; 4] = [b"Storage", b"Photos", b"Pictures", b"Videos"];

/// The action waiting for the user's confirmation.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct PendingAction {
    pub action: FileAction,
    /// The card rows the action will touch. WHY as many rows as a listing may
    /// hold: a selection of the whole directory must act on the whole
    /// directory, and a shorter array would silently drop the rest.
    pub items: [usize; MAX_PARSED_ENTRIES],
    pub item_count: usize,
}

impl PendingAction {
    pub const EMPTY: PendingAction = PendingAction {
        action: FileAction::Copy,
        items: [0; MAX_PARSED_ENTRIES],
        item_count: 0,
    };
}

// -----------------------------------------------------------------------------
// The app
// -----------------------------------------------------------------------------

pub struct FileManagerApp {
    /// Which screen is on the surface.
    pub view: ViewMode,
    /// How the browse screen lays its items out.
    pub layout: LayoutMode,
    /// The browse screen's sort key.
    pub sort: SortMode,
    /// What a search looks at.
    pub search_scope: SearchScope,
    /// The category row the current folder was opened from.
    pub selected_shortcut: usize,
    pub selected_entry: Option<usize>,
    pub storage_capacity_bytes: u64,
    /// The storage card's capacity line, pre-rendered from the measured answer.
    pub storage_line: [u8; 64],
    pub storage_line_len: usize,
    pub current_path: [u8; 128],
    pub current_path_len: usize,
    /// What `fs_list` actually answered, pre-rendered.
    pub probe_label: [u8; 40],
    pub probe_label_len: usize,
    pub listing_state: ListingState,
    pub listing_error: i64,
    pub entries: [RuntimeFileEntry; MAX_PARSED_ENTRIES],
    pub entry_count: usize,
    pub selected_document: SelectedDocument,
    pub has_selected_document: bool,
    /// Where the entries list or the grid is scrolled to.
    pub scroll: ScrollState,
    /// The text the user has typed into the search field.
    pub query: TypedField,
    /// The documents the platform's index answered the search with.
    pub results: [IndexedDocument; MAX_SEARCH_RESULTS],
    pub result_count: usize,
    pub searching: bool,
    pub search_error: i64,
    pub selection: Selection,
    pub multi_select: bool,
    pub pending: PendingAction,
    pub has_pending: bool,
    /// The directory a copy or a move writes into.
    pub destination: usize,
    pub rename_field: TypedField,
    /// The status line, pre-rendered.
    pub status: [u8; LINE_LEN],
    pub status_len: usize,
    /// The outcome of the last operation, when it has something to say. It
    /// takes precedence over the status line until the user does something
    /// else, because "3 done, 1 failed" is a result the user must be able to
    /// read after the listing has been refreshed.
    pub notice: [u8; LINE_LEN],
    pub notice_len: usize,
    /// The search scope caption, pre-rendered.
    pub scope: [u8; LINE_LEN],
    pub scope_len: usize,
    pub sheet_title: [u8; 64],
    pub sheet_title_len: usize,
    pub sheet_names: [[u8; MAX_ENTRY_NAME_LEN]; 3],
    pub sheet_name_lens: [usize; 3],
    pub sheet_name_count: usize,
    /// The documents the index reports, newest first.
    pub recents: [IndexedDocument; MAX_SEARCH_RESULTS],
    pub recents_count: usize,
    /// The platform's code when the index was refused.
    pub recents_error: i64,
    /// Each visible recent's modification line, pre-rendered.
    pub recent_time: [[u8; 56]; HOME_RECENTS_VISIBLE],
    pub recent_time_len: [usize; HOME_RECENTS_VISIBLE],
    /// The details card's facts, as the platform answered them.
    pub details: DetailsView,
    pub has_details: bool,
    /// The first per-item failure, so a partial operation can name it.
    pub fail_name: [u8; MAX_ENTRY_NAME_LEN],
    pub fail_name_len: usize,
    pub fail_reason: [u8; 64],
    pub fail_reason_len: usize,
    /// The breadcrumbs of the current path.
    pub crumbs: [Crumb; 6],
    pub crumb_count: usize,
    /// The sort pill's label, pre-rendered.
    pub sort_label: [u8; 24],
    pub sort_label_len: usize,
    /// Whether the next tap is the one a long press already handled.
    pub suppress_tap: bool,
}

impl FileManagerApp {
    pub const fn new() -> Self {
        let mut path = [0u8; 128];
        // WHY the default path is `/storage`: it is one of the paths the
        // kernel's app policy lets this app list. The old default was
        // `/data`, which the policy refuses before the path is examined, so
        // the app opened on a directory it could never see.
        let init = b"/storage";
        let mut i = 0;
        while i < init.len() {
            path[i] = init[i];
            i += 1;
        }
        Self {
            view: ViewMode::Home,
            layout: LayoutMode::List,
            sort: SortMode::Name,
            search_scope: SearchScope::AllStorage,
            selected_shortcut: 0,
            selected_entry: None,
            storage_capacity_bytes: 0,
            storage_line: [0u8; 64],
            storage_line_len: 0,
            current_path: path,
            current_path_len: init.len(),
            probe_label: PROBE_DEFAULT,
            probe_label_len: 10,
            listing_state: ListingState::NotListed,
            listing_error: 0,
            entries: [RuntimeFileEntry::EMPTY; MAX_PARSED_ENTRIES],
            entry_count: 0,
            selected_document: SelectedDocument::EMPTY,
            has_selected_document: false,
            scroll: ScrollState::new(0, ENTRIES_CARD.h, 0),
            query: TypedField::EMPTY,
            results: [IndexedDocument::EMPTY; MAX_SEARCH_RESULTS],
            result_count: 0,
            searching: false,
            search_error: 0,
            selection: Selection::EMPTY,
            multi_select: false,
            pending: PendingAction::EMPTY,
            has_pending: false,
            destination: 0,
            rename_field: TypedField::EMPTY,
            status: [0u8; LINE_LEN],
            status_len: 0,
            notice: [0u8; LINE_LEN],
            notice_len: 0,
            scope: [0u8; LINE_LEN],
            scope_len: 0,
            sheet_title: [0u8; 64],
            sheet_title_len: 0,
            sheet_names: [[0u8; MAX_ENTRY_NAME_LEN]; 3],
            sheet_name_lens: [0; 3],
            sheet_name_count: 0,
            recents: [IndexedDocument::EMPTY; MAX_SEARCH_RESULTS],
            recents_count: 0,
            recents_error: 0,
            recent_time: [[0u8; 56]; HOME_RECENTS_VISIBLE],
            recent_time_len: [0; HOME_RECENTS_VISIBLE],
            details: DetailsView::EMPTY,
            has_details: false,
            fail_name: [0u8; MAX_ENTRY_NAME_LEN],
            fail_name_len: 0,
            fail_reason: [0u8; 64],
            fail_reason_len: 0,
            crumbs: [Crumb::EMPTY; 6],
            crumb_count: 0,
            sort_label: [0u8; 24],
            sort_label_len: 0,
            suppress_tap: false,
        }
    }

    #[inline]
    pub fn current_path_str(&self) -> &str {
        core::str::from_utf8(&self.current_path[..self.current_path_len]).unwrap_or("/storage")
    }

    pub fn set_path(&mut self, path: &str) {
        let bytes = path.as_bytes();
        let len = bytes.len().min(self.current_path.len());
        self.current_path[..len].copy_from_slice(&bytes[..len]);
        self.current_path_len = len;
    }

    /// Keeps `selected_shortcut` pointing at the category the path belongs to.
    fn sync_shortcut(&mut self) {
        let mut found = None;
        for index in 0..CATEGORY_COUNT {
            if CATEGORY_PATHS[index] == self.current_path_str() {
                found = Some(index);
                break;
            }
        }
        if let Some(index) = found {
            self.selected_shortcut = index;
        }
    }

    /// The storage card's one measured line, built once.
    pub fn refresh_storage_line(&mut self) {
        self.storage_line = [0u8; 64];
        let mut len = append(&mut self.storage_line, 0, b"Capacity: ");
        if self.storage_capacity_bytes > 0 {
            len = format_size_human(self.storage_capacity_bytes, &mut self.storage_line[len..]) + len;
            len = append(&mut self.storage_line, len, b" reported by the storage service");
        } else {
            len = append(
                &mut self.storage_line,
                len,
                b"the storage service reported no size",
            );
        }
        self.storage_line_len = len;
    }

    /// The storage card's honest status, from the platform's own answer.
    ///
    /// WHY the same two sentences as before: a capacity without a usage is
    /// exactly what the storage service reports, so the card says the capacity
    /// is real and the usage is not available. It never guesses a percentage.
    pub fn storage_status(&self) -> &'static [u8] {
        if self.storage_capacity_bytes > 0 {
            b"Capacity detected; usage unavailable"
        } else {
            b"Storage capacity unavailable"
        }
    }

    /// Reads the platform's document index and keeps the newest documents.
    ///
    /// WHY the index and not a walk of the storage tree: the index is the one
    /// description of the documents the device holds, and it carries the
    /// modification time. The list is ordered by that time, newest first.
    pub fn load_recents(&mut self) {
        self.recents_error = 0;
        self.recents_count = 0;
        let mut raw = [0u8; 4096];
        match std::content::content_index_list(&mut raw) {
            Ok(n) => {
                let parsed = parse_indexed_documents(&raw[..n], &mut self.recents);
                let sorted = sort_recents(&mut self.recents, parsed);
                self.recents_count = sorted.min(MAX_SEARCH_RESULTS);
                self.refresh_recent_times();
                log_counted(
                    "[FILE_MANAGER] Document index recents: ",
                    self.recents_count as u64,
                    " documents.",
                );
            }
            Err(code) => {
                self.recents_error = code;
                self.refresh_recent_times();
                log_counted(
                    "[FILE_MANAGER] The document index was refused, code -",
                    code.unsigned_abs(),
                    "",
                );
            }
        }
    }

    /// Pre-renders each visible recent's modification line.
    ///
    /// WHY pre-rendered: the renderer runs once per pixel and the line holds a
    /// measured number, so it is formatted once here.
    fn refresh_recent_times(&mut self) {
        for index in 0..HOME_RECENTS_VISIBLE {
            self.recent_time[index] = [0u8; 56];
            self.recent_time_len[index] = 0;
            if index >= self.recents_count {
                continue;
            }
            let mtime = self.recents[index].mtime;
            let value = if mtime > 0 { Some(mtime) } else { None };
            self.recent_time_len[index] = format_mtime(value, &mut self.recent_time[index]);
        }
    }

    pub fn navigate_into(&mut self, entry_idx: usize) {
        if entry_idx >= self.entry_count {
            return;
        }
        let entry = self.entries[entry_idx];
        if !entry.is_dir {
            return;
        }
        let mut n = self.current_path_len;
        if n > 0 && !self.current_path[..n].ends_with(b"/") && n < self.current_path.len() {
            self.current_path[n] = b'/';
            n += 1;
        }
        let name_slice = &entry.name[..entry.name_len];
        let copy_len = name_slice.len().min(self.current_path.len().saturating_sub(n));
        self.current_path[n..n + copy_len].copy_from_slice(&name_slice[..copy_len]);
        self.current_path_len = n + copy_len;
        self.reset_browse_state();
        self.refresh_directory();
    }

    pub fn navigate_parent(&mut self) -> bool {
        let slice = &self.current_path[..self.current_path_len];
        if slice == b"/storage" {
            return false;
        }
        let last_slash = slice.iter().rposition(|&b| b == b'/');
        match last_slash {
            Some(0) | None => {
                // WHY the root is the storage root and not `/data`: `/data` is
                // refused by the kernel's app path policy, so navigating up to
                // it put the app in a directory it cannot read.
                let init = b"/storage";
                self.current_path[..init.len()].copy_from_slice(init);
                self.current_path_len = init.len();
            }
            Some(pos) => {
                self.current_path_len = pos;
            }
        }
        self.reset_browse_state();
        self.refresh_directory();
        true
    }

    /// Clears the per-directory view state when the folder changes.
    fn reset_browse_state(&mut self) {
        self.selected_entry = None;
        self.multi_select = false;
        self.selection.clear();
        self.searching = false;
        self.result_count = 0;
        self.search_error = 0;
        self.query.clear();
        self.query.blur();
        self.has_details = false;
        self.has_selected_document = false;
    }

    /// Records the real answer from `fs_list`, in both its forms.
    pub fn record_probe(&mut self, result: Result<usize, i64>) {
        let mut buf = [0u8; 40];
        let n = match result {
            // The header summarizes what the folder holds, not how many
            // bytes the listing answer happened to occupy. The entry count
            // is written once the answer has been parsed.
            Ok(_) => 0,
            Err(code) => {
                self.listing_state = ListingState::Refused;
                self.listing_error = code;
                let prefix = b"Refused (-";
                let p = prefix.len();
                buf[..p].copy_from_slice(prefix);
                let mut n = p + fmt_u64(code.unsigned_abs(), &mut buf[p..]);
                if n < buf.len() {
                    buf[n] = b')';
                    n += 1;
                }
                n
            }
        };
        self.probe_label = [0u8; 40];
        self.probe_label[..n].copy_from_slice(&buf[..n]);
        self.probe_label_len = n;
    }

    /// Writes the folder's item count into the header.
    fn set_probe_count(&mut self, count: usize) {
        let mut buf = [0u8; 40];
        let n = if count == 0 {
            let message = b"Empty folder";
            buf[..message.len()].copy_from_slice(message);
            message.len()
        } else {
            let mut n = fmt_u64(count as u64, &mut buf);
            let suffix: &[u8] = if count == 1 { b" item" } else { b" items" };
            for &b in suffix {
                if n < buf.len() {
                    buf[n] = b;
                    n += 1;
                }
            }
            n
        };
        self.probe_label = [0u8; 40];
        self.probe_label[..n].copy_from_slice(&buf[..n]);
        self.probe_label_len = n;
    }

    /// Reads the current directory, sorts it, and rebuilds what depends on it.
    pub fn refresh_directory(&mut self) {
        let mut raw = [0u8; 4096];
        let probe = std::fs_list(self.current_path_str(), &mut raw);
        self.record_probe(probe);
        if let Ok(n) = probe {
            self.entry_count = parse_vfs_entries(&raw[..n], &mut self.entries);
            // WHY the state follows the parse: a listing that succeeded with
            // no entries is empty, and one that succeeded with entries is
            // listed. Neither is the same fact as a refusal.
            self.listing_state = if self.entry_count == 0 {
                ListingState::Empty
            } else {
                ListingState::Listed
            };
            self.set_probe_count(self.entry_count);
        } else {
            self.entry_count = 0;
        }

        // A listing is folders first, then the chosen key. The selection is
        // remapped by name so a re-sort never retargets a marked file.
        sort_entries(
            &mut self.entries,
            self.entry_count,
            self.sort,
            &mut self.selection,
        );

        let mut crumbs = [Crumb::EMPTY; 6];
        let crumb_count = breadcrumb_crumbs(
            &self.current_path[..self.current_path_len],
            &mut crumbs,
        );
        self.crumbs = crumbs;
        self.crumb_count = crumb_count;

        self.sync_shortcut();
        self.scroll = self.scroll_for(self.scroll.offset);
        self.refresh_sort_label();
        self.set_scope();
        self.set_status();

        // The log names the path that was really asked for.
        let mut path_buf = [0u8; 128];
        let path_len = self.current_path_len.min(128);
        path_buf[..path_len].copy_from_slice(&self.current_path[..path_len]);
        if let Ok(path) = core::str::from_utf8(&path_buf[..path_len]) {
            match probe {
                Ok(bytes) => {
                    log_listing(path, bytes as u64, " bytes of directory entries received.");
                    log_counted(
                        "[FILE_MANAGER] Parsed VFS directory entries: ",
                        self.entry_count as u64,
                        " items.",
                    );
                }
                Err(code) => log_listing_refused(path, code),
            }
        }
    }

    /// The scroll state for the current listing and layout.
    fn scroll_for(&self, offset: usize) -> ScrollState {
        let count = self.card_item_count();
        if self.layout == LayoutMode::Grid {
            grid_scroll(count, ENTRIES_CARD, offset)
        } else {
            list_scroll(count, ENTRIES_CARD, offset)
        }
    }

    /// The number of rows the card shows.
    pub fn card_item_count(&self) -> usize {
        if self.searching {
            self.result_count
        } else {
            self.entry_count
        }
    }

    pub fn card_item_name(&self, index: usize) -> &[u8] {
        if self.searching {
            if index < self.result_count {
                return &self.results[index].name[..self.results[index].name_len];
            }
            return b"";
        }
        if index < self.entry_count {
            return &self.entries[index].name[..self.entries[index].name_len];
        }
        b""
    }

    /// Whether the row at an index is a folder.
    ///
    /// WHY a search result is never a folder: the index holds documents, and a
    /// document is a file. The platform has no folder records.
    pub fn card_item_is_dir(&self, index: usize) -> bool {
        if self.searching {
            return false;
        }
        if index < self.entry_count {
            return self.entries[index].is_dir;
        }
        false
    }

    pub fn card_item_trailing(&self, index: usize) -> &[u8] {
        if self.searching {
            if index < self.result_count {
                return &self.results[index].size_label[..self.results[index].size_label_len];
            }
            return b"";
        }
        if index < self.entry_count {
            return &self.entries[index].size_label[..self.entries[index].size_label_len];
        }
        b""
    }

    /// The absolute path of the row at an index.
    pub fn card_item_path(&self, index: usize, out: &mut [u8; MAX_PATH_LEN]) -> Option<usize> {
        if self.searching {
            if index >= self.result_count {
                return None;
            }
            let len = self.results[index].path_len.min(MAX_PATH_LEN);
            out[..len].copy_from_slice(&self.results[index].path[..len]);
            return Some(len);
        }
        if index >= self.entry_count {
            return None;
        }
        let name = &self.entries[index].name[..self.entries[index].name_len];
        join_path(self.current_path_str(), name, out)
    }

    /// Asks the platform's document index for the documents whose name holds
    /// the typed needle, then applies the search scope.
    ///
    /// WHY the scope is applied after the index answers: the index answers
    /// about the whole device, and "this folder" is a filter over that answer.
    /// A folder scope uses whole path components, so `/storage/photos2` is not
    /// inside `/storage/photos`.
    pub fn run_search(&mut self) {
        let mut needle = [0u8; 32];
        let needle_len = self.query.as_slice().len().min(32);
        needle[..needle_len].copy_from_slice(&self.query.as_slice()[..needle_len]);
        if !search_needle_is_usable(&needle[..needle_len]) {
            self.searching = false;
            self.result_count = 0;
            self.search_error = 0;
            self.selection.clear();
            self.scroll = self.scroll_for(0);
            self.set_scope();
            self.set_status();
            return;
        }
        let needle_str = core::str::from_utf8(&needle[..needle_len]).unwrap_or("");

        // The folder is copied out first: the index call and the filter both
        // need it, and the results array is borrowed mutably in between.
        let mut folder = [0u8; 128];
        let folder_len = self.current_path_len.min(128);
        folder[..folder_len].copy_from_slice(&self.current_path[..folder_len]);
        let folder_str = core::str::from_utf8(&folder[..folder_len]).unwrap_or("/storage");

        let mut raw = [0u8; 4096];
        match std::content::content_index_search_name(needle_str, &mut raw) {
            Ok(n) => {
                let parsed = parse_indexed_documents(&raw[..n], &mut self.results);
                let count = apply_search_scope(
                    &mut self.results,
                    parsed,
                    self.search_scope,
                    folder_str,
                );
                sort_documents(&mut self.results, count, self.sort);
                self.result_count = count;
                self.searching = true;
                self.search_error = 0;
                self.selection.clear();
                self.multi_select = false;
                self.selected_entry = None;
            }
            Err(code) => {
                self.result_count = 0;
                self.searching = true;
                self.search_error = code;
                self.selection.clear();
            }
        }
        self.scroll = self.scroll_for(0);
        self.set_scope();
        self.set_status();
    }

    /// Empties the search and goes back to the directory listing.
    pub fn clear_search(&mut self) {
        self.query.clear();
        self.query.blur();
        self.searching = false;
        self.result_count = 0;
        self.search_error = 0;
        self.multi_select = false;
        self.selection.clear();
        self.scroll = self.scroll_for(0);
        self.set_scope();
        self.set_status();
    }

    /// Toggles the list/grid layout without touching the selection.
    ///
    /// WHY nothing is cleared: the rows keep their order, so a marked row is
    /// the same file in either layout.
    pub fn toggle_layout(&mut self) {
        self.layout = self.layout.toggle();
        self.scroll = self.scroll_for(self.scroll.offset);
        self.set_status();
    }

    /// Toggles the sort key and re-orders what is on screen.
    pub fn apply_sort(&mut self) {
        self.sort = self.sort.toggle();
        self.refresh_sort_label();
        if self.searching {
            // WHY the marks are cleared here: an index record has no listing
            // name mapping to re-apply, and leaving the row numbers behind
            // would point every action at a different document.
            let had_marks = self.selection.count() > 0;
            sort_documents(&mut self.results, self.result_count, self.sort);
            self.selection.clear();
            self.multi_select = false;
            if had_marks {
                self.set_notice(b"Search results re-sorted; the selection was cleared");
            }
        } else {
            sort_entries(
                &mut self.entries,
                self.entry_count,
                self.sort,
                &mut self.selection,
            );
        }
        self.scroll = self.scroll_for(0);
        self.set_status();
    }

    fn refresh_sort_label(&mut self) {
        self.sort_label = [0u8; 24];
        let mut len = append(&mut self.sort_label, 0, b"Sort: ");
        len = append(&mut self.sort_label, len, self.sort.label());
        self.sort_label_len = len;
    }

    /// Marks or unmarks a row.
    pub fn toggle_mark(&mut self, index: usize) -> bool {
        let marked = self.selection.toggle(index);
        self.set_status();
        marked
    }

    /// Marks every row of what is on screen.
    pub fn select_all_visible(&mut self) {
        self.selection.select_all(self.card_item_count());
        self.multi_select = true;
        self.set_status();
    }

    /// The rows the action bar's actions would touch.
    pub fn action_rows(&self, out: &mut [usize]) -> usize {
        if self.multi_select {
            return self.selection.indices(out);
        }
        match self.selected_entry {
            Some(index) if index < self.card_item_count() => {
                if out.is_empty() {
                    return 0;
                }
                out[0] = index;
                1
            }
            _ => 0,
        }
    }

    /// How many rows an action would touch.
    pub fn action_row_count(&self) -> usize {
        let mut rows = [0usize; MAX_PARSED_ENTRIES];
        self.action_rows(&mut rows)
    }

    /// Whether the platform can do this action to what is selected.
    pub fn action_is_possible(&self, action: FileAction) -> bool {
        let mut rows = [0usize; MAX_PARSED_ENTRIES];
        let count = self.action_rows(&mut rows);
        if count == 0 {
            return false;
        }
        if action == FileAction::Rename && count != 1 {
            // A rename names one file. Offering it for a set would be
            // offering a control that cannot do what it says.
            return false;
        }
        for &row in &rows[..count] {
            if !action_supported(action, self.card_item_is_dir(row)) {
                return false;
            }
        }
        true
    }

    /// Scrolls the list or the grid.
    pub fn scroll_by(&mut self, dy: i32) -> bool {
        let mut scroll = self.scroll_for(self.scroll.offset);
        let before = scroll.offset;
        scroll.apply_drag(dy);
        self.scroll = scroll;
        self.scroll.offset != before
    }

    /// Opens the confirmation sheet for an action over the current selection.
    pub fn open_confirmation(&mut self, action: FileAction) -> bool {
        let mut rows = [0usize; MAX_PARSED_ENTRIES];
        let count = self.action_rows(&mut rows);
        if count == 0 {
            self.set_notice(problem_text(OpProblem::NoSelection));
            return false;
        }
        let mut any_dir = false;
        for &row in &rows[..count] {
            if self.card_item_is_dir(row) {
                any_dir = true;
                break;
            }
        }
        if any_dir {
            self.set_notice(problem_text(OpProblem::FolderNotSupported));
            return false;
        }
        if action == FileAction::Rename && count != 1 {
            self.set_notice(problem_text(OpProblem::RenameNeedsOne));
            return false;
        }
        self.notice_len = 0;
        self.pending = PendingAction {
            action,
            items: rows,
            item_count: count,
        };
        self.has_pending = true;

        self.sheet_title = [0u8; 64];
        let mut len = append(&mut self.sheet_title, 0, ACTION_LABELS[action as usize]);
        len = append(&mut self.sheet_title, len, b" ");
        len = append_u64(&mut self.sheet_title, len, count as u64);
        len = append(
            &mut self.sheet_title,
            len,
            if count == 1 { b" file?" } else { b" files?" },
        );
        self.sheet_title_len = len;

        // The names of the files the action will touch, and only those: the
        // sheet holds three lines, and a list of forty names would not fit.
        self.sheet_name_count = 0;
        let shown = count.min(3);
        for index in 0..shown {
            let mut copy = [0u8; MAX_ENTRY_NAME_LEN];
            let name = self.card_item_name(rows[index]);
            let take = name.len().min(MAX_ENTRY_NAME_LEN);
            copy[..take].copy_from_slice(&name[..take]);
            self.sheet_names[index] = copy;
            self.sheet_name_lens[index] = take;
            self.sheet_name_count = index + 1;
        }
        if count > 3 {
            let mut more = [0u8; 64];
            let mut len = append(&mut more, 0, b"and ");
            len = append_u64(&mut more, len, (count - 3) as u64);
            len = append(&mut more, len, b" more");
            let take = len.min(MAX_ENTRY_NAME_LEN);
            self.sheet_names[2] = [0u8; MAX_ENTRY_NAME_LEN];
            self.sheet_names[2][..take].copy_from_slice(&more[..take]);
            self.sheet_name_lens[2] = take;
            self.sheet_name_count = 3;
        }

        if action == FileAction::Rename {
            // The rename field starts with the name the file has now, so the
            // user changes the part they mean to change.
            let mut copy = [0u8; MAX_ENTRY_NAME_LEN];
            let name = self.card_item_name(rows[0]);
            let take = name.len().min(MAX_ENTRY_NAME_LEN);
            copy[..take].copy_from_slice(&name[..take]);
            self.rename_field.set(&copy[..take]);
            self.rename_field.focus();
        } else {
            self.rename_field.clear();
            self.rename_field.blur();
        }

        // A copy or a move starts on a destination that is not the folder the
        // files are already in, because copying a file onto itself would only
        // ever be refused.
        self.destination = 0;
        if CATEGORY_PATHS[0] == self.current_path_str() {
            self.destination = 1;
        }
        self.set_status();
        true
    }

    pub fn close_confirmation(&mut self) {
        self.has_pending = false;
        self.pending = PendingAction::EMPTY;
        self.rename_field.clear();
        self.rename_field.blur();
        self.set_status();
    }

    /// Writes a result line that outlives the status line's usual content.
    pub fn set_notice(&mut self, text: &[u8]) {
        self.notice = [0u8; LINE_LEN];
        let take = text.len().min(LINE_LEN);
        self.notice[..take].copy_from_slice(&text[..take]);
        self.notice_len = take;
        self.set_status();
    }

    pub fn clear_notice(&mut self) {
        if self.notice_len > 0 {
            self.notice_len = 0;
            self.set_status();
        }
    }

    /// Writes the status line.
    pub fn set_status(&mut self) {
        self.status = [0u8; LINE_LEN];
        self.status_len = 0;
        if self.notice_len > 0 {
            let take = self.notice_len.min(LINE_LEN);
            self.status[..take].copy_from_slice(&self.notice[..take]);
            self.status_len = take;
            return;
        }
        if self.has_pending {
            let action = self.pending.action;
            let count = self.pending.item_count;
            let mut len = append(&mut self.status, 0, ACTION_LABELS[action as usize]);
            len = append(&mut self.status, len, b" ");
            len = append_u64(&mut self.status, len, count as u64);
            len = append(
                &mut self.status,
                len,
                if count == 1 { b" file" } else { b" files" },
            );
            len = append(&mut self.status, len, b" - confirm or cancel");
            self.status_len = len;
            return;
        }
        if self.multi_select {
            let count = self.selection.count();
            let total = self.card_item_count();
            let mut len = append(&mut self.status, 0, b"Selected ");
            len = append_u64(&mut self.status, len, count as u64);
            len = append(&mut self.status, len, b" of ");
            len = append_u64(&mut self.status, len, total as u64);
            len = append(
                &mut self.status,
                len,
                if count == 1 { b" item" } else { b" items" },
            );
            self.status_len = len;
            return;
        }
        if self.searching && self.search_error != 0 {
            let mut len = append(&mut self.status, 0, b"The search was refused (");
            len = append_u64(&mut self.status, len, self.search_error.unsigned_abs());
            len = append(&mut self.status, len, b")");
            self.status_len = len;
            return;
        }
        if self.searching {
            let mut len = append(&mut self.status, 0, b"");
            len = append_u64(&mut self.status, len, self.result_count as u64);
            len = append(
                &mut self.status,
                len,
                if self.result_count == 1 {
                    b" match - tap a result for its details"
                } else {
                    b" matches - tap a result for its details"
                },
            );
            self.status_len = len;
            return;
        }
        let mut len = append(&mut self.status, 0, b"");
        len = append_u64(&mut self.status, len, self.entry_count as u64);
        len = append(
            &mut self.status,
            len,
            if self.entry_count == 1 {
                b" entry"
            } else {
                b" entries"
            },
        );
        self.status_len = len;
    }

    /// Writes the search scope caption.
    ///
    /// WHY a caption beside the pills: a search over one folder and a search
    /// over every document answer different questions, and a reader who cannot
    /// tell which one was asked cannot trust the answer.
    pub fn set_scope(&mut self) {
        self.scope = [0u8; LINE_LEN];
        let mut len = 0usize;
        if self.searching {
            len = append(&mut self.scope, len, b"Scope: ");
            len = append(&mut self.scope, len, self.search_scope.label());
            len = append(&mut self.scope, len, b" - ");
            len = append_u64(&mut self.scope, len, self.result_count as u64);
            len = append(
                &mut self.scope,
                len,
                if self.result_count == 1 {
                    b" match"
                } else {
                    b" matches"
                },
            );
        } else {
            len = append(&mut self.scope, len, b"Search in ");
            len = append(&mut self.scope, len, self.search_scope.label());
            len = append(&mut self.scope, len, b" - ");
            len = append_u64(&mut self.scope, len, self.entry_count as u64);
            len = append(
                &mut self.scope,
                len,
                if self.entry_count == 1 {
                    b" entry"
                } else {
                    b" entries"
                },
            );
        }
        self.scope_len = len;
    }

    /// Opens the details card for a row of the current listing or search.
    pub fn show_details(&mut self, index: usize) {
        if index >= self.card_item_count() {
            return;
        }
        let mut path = [0u8; MAX_PATH_LEN];
        let Some(path_len) = self.card_item_path(index, &mut path) else {
            return;
        };
        let mut name = [0u8; MAX_ENTRY_NAME_LEN];
        let name_len = self.card_item_name(index).len().min(MAX_ENTRY_NAME_LEN);
        name[..name_len].copy_from_slice(&self.card_item_name(index)[..name_len]);
        let is_dir = self.card_item_is_dir(index);

        let mut size = if self.searching {
            self.results[index].size
        } else {
            self.entries[index].size
        };
        let mut mtime = if self.searching && self.results[index].mtime > 0 {
            Some(self.results[index].mtime)
        } else {
            None
        };

        // A listing carries a name and a byte length; the modification time
        // and the exact size come from the platform's stat call.
        if !self.searching && !is_dir {
            if let Ok(path_str) = core::str::from_utf8(&path[..path_len]) {
                let mut stat_raw = [0u8; 96];
                if let Ok(written) = std::fs_stat(path_str, &mut stat_raw) {
                    if let Some(record) = std::FileStatRecord::parse(&stat_raw[..written]) {
                        if record.mtime > 0 {
                            mtime = Some(record.mtime);
                        }
                        if record.size > 0 {
                            size = record.size;
                        }
                    }
                }
            }
        }

        let (mut view, document) = details_for(
            &name[..name_len],
            &path[..path_len],
            is_dir,
            size,
            mtime,
        );
        fit_details(&mut view);
        self.details = view;
        self.has_details = true;
        self.selected_entry = Some(index);
        if let Some(document) = document {
            self.has_selected_document = document.has_handler();
            self.selected_document = document;
        } else {
            self.has_selected_document = false;
        }
    }

    pub fn close_details(&mut self) {
        self.has_details = false;
        // WHY the selected row is kept: the details card is how a single file
        // is selected, and clearing the selection here muted every single-file
        // action. Copy, Move, Delete and Rename are enabled by the selection,
        // and the card's action bar is the row's own Open control.
        self.set_status();
    }

    /// Opens a document the index lists as recent.
    ///
    /// WHY this navigates to the document's folder first: the details card
    /// names a path, and the browser behind it should be at that path rather
    /// than wherever the user last was.
    pub fn open_recent(&mut self, index: usize) {
        if index >= self.recents_count {
            return;
        }
        let doc = self.recents[index];
        let name_len = doc.name_len.min(MAX_ENTRY_NAME_LEN);
        let mut name = [0u8; MAX_ENTRY_NAME_LEN];
        name[..name_len].copy_from_slice(&doc.name[..name_len]);
        let path_len = doc.path_len.min(MAX_PATH_LEN);
        let mut path = [0u8; MAX_PATH_LEN];
        path[..path_len].copy_from_slice(&doc.path[..path_len]);

        let mut parent = [0u8; 128];
        let parent_len = parent_path(&path[..path_len], &mut parent);
        self.current_path = [0u8; 128];
        self.current_path[..parent_len].copy_from_slice(&parent[..parent_len]);
        self.current_path_len = parent_len;

        self.view = ViewMode::Browse;
        self.searching = false;
        self.result_count = 0;
        self.search_error = 0;
        self.query.clear();
        self.query.blur();
        self.multi_select = false;
        self.selection.clear();
        self.selected_entry = None;
        self.refresh_directory();

        // The row is selected when the folder still holds it.
        for row in 0..self.entry_count {
            let entry_name = &self.entries[row].name[..self.entries[row].name_len];
            if entry_name == &name[..name_len] {
                self.selected_entry = Some(row);
                break;
            }
        }

        let mtime = if doc.mtime > 0 { Some(doc.mtime) } else { None };
        let (mut view, document) =
            details_for(&name[..name_len], &path[..path_len], false, doc.size, mtime);
        fit_details(&mut view);
        self.details = view;
        self.has_details = true;
        if let Some(document) = document {
            self.has_selected_document = document.has_handler();
            self.selected_document = document;
        } else {
            self.has_selected_document = false;
        }
        self.set_notice(b"Opened from the platform's document index");
    }
}

fn append_u64(buf: &mut [u8], n: usize, value: u64) -> usize {
    let mut tmp = [0u8; 20];
    let len = fmt_u64(value, &mut tmp);
    append(buf, n, &tmp[..len])
}

fn append(buf: &mut [u8], n: usize, bytes: &[u8]) -> usize {
    let take = bytes.len().min(buf.len().saturating_sub(n));
    buf[n..n + take].copy_from_slice(&bytes[..take]);
    n + take
}

/// The parent folder of a path, or the storage root when there is none.
fn parent_path(path: &[u8], out: &mut [u8; 128]) -> usize {
    let mut end = path.len();
    while end > 0 && path[end - 1] == b'/' {
        end -= 1;
    }
    let mut pos = end;
    while pos > 0 {
        pos -= 1;
        if path[pos] == b'/' {
            break;
        }
    }
    let len = if path.is_empty() || path[pos] != b'/' {
        0
    } else if pos == 0 {
        1
    } else {
        pos
    };
    if len == 0 {
        let init = b"/storage";
        out[..init.len()].copy_from_slice(init);
        init.len()
    } else {
        out[..len].copy_from_slice(&path[..len]);
        len
    }
}

/// Builds the details card from the facts the platform answered.
///
/// WHY the index first and the registry second: the index is where the
/// platform records what a document is; the registry is where it records who
/// can open that kind of document. Both are platform answers, and the card
/// shows neither as a guess.
fn details_for(
    name: &[u8],
    path: &[u8],
    is_dir: bool,
    size: u64,
    mtime: Option<u64>,
) -> (DetailsView, Option<SelectedDocument>) {
    let path_str = core::str::from_utf8(path).unwrap_or("");
    let indexed = if path_str.is_empty() {
        None
    } else {
        document_from_index(path_str)
    };
    let mut selected = if path_str.is_empty() {
        None
    } else {
        document_from_path(path_str)
    };

    let mut mime = [0u8; 48];
    let mut mime_len = 0usize;
    let mut handler = [0u8; 40];
    let mut handler_len = 0usize;
    let mut has_handler = false;

    if let Some(document) = indexed.as_ref() {
        mime_len = document.mime_len.min(48);
        mime[..mime_len].copy_from_slice(&document.mime[..mime_len]);
        for entry in document.handlers[..document.handler_count].iter() {
            let bytes = core::str::from_utf8(&entry.package_id[..entry.package_id_len]).unwrap_or("");
            handler_len = bytes.len().min(40);
            handler[..handler_len].copy_from_slice(&bytes.as_bytes()[..handler_len]);
            has_handler = true;
            break;
        }
    } else if let Some(document) = selected.as_ref() {
        // The index has not walked this path yet; the extension table and the
        // intent registry are still platform facts, so they answer instead.
        mime_len = document.mime_len.min(48);
        mime[..mime_len].copy_from_slice(&document.mime[..mime_len]);
        for entry in document.handlers[..document.handler_count].iter() {
            let bytes = core::str::from_utf8(&entry.package_id[..entry.package_id_len]).unwrap_or("");
            handler_len = bytes.len().min(40);
            handler[..handler_len].copy_from_slice(&bytes.as_bytes()[..handler_len]);
            has_handler = true;
            break;
        }
    }

    let mut origin_buf = [0u8; 128];
    let origin_len = if !path_str.is_empty() {
        read_origin_url(path_str, &mut origin_buf)
    } else {
        None
    };
    let prov: Option<&[u8]> = if let Some(len) = origin_len {
        if let Some(doc) = selected.as_mut() {
            let take = len.min(doc.provenance.len());
            doc.provenance[..take].copy_from_slice(&origin_buf[..take]);
            doc.provenance_len = take;
        }
        Some(&origin_buf[..len])
    } else {
        selected.as_ref().map(|d| &d.provenance[..d.provenance_len])
    };
    let effective_mtime = mtime.filter(|value| *value > 0);
    let view = build_details_with_provenance(
        name,
        path,
        &mime[..mime_len],
        size,
        effective_mtime,
        if has_handler {
            Some(&handler[..handler_len])
        } else {
            None
        },
        is_dir,
        OPEN_WITH_LABEL,
        prov,
    );
    (view, selected)
}

/// Reads the download origin URL from a companion `.origin` sidecar file, if present.
fn read_origin_url(path: &str, out: &mut [u8]) -> Option<usize> {
    let mut meta_path = [0u8; 128];
    if path.len() + 7 > meta_path.len() {
        return None;
    }
    meta_path[..path.len()].copy_from_slice(path.as_bytes());
    meta_path[path.len()..path.len() + 7].copy_from_slice(b".origin");
    let meta_str = core::str::from_utf8(&meta_path[..path.len() + 7]).ok()?;
    let fd = std::fs_open(meta_str, std::FS_READ).ok()?;
    let mut buf = [0u8; 128];
    let n = std::fs_read(fd, &mut buf).ok()?;
    let _ = std::fs_close(fd);
    if n == 0 {
        return None;
    }
    let take = n.min(out.len());
    out[..take].copy_from_slice(&buf[..take]);
    Some(take)
}

/// Trims a details card's title and lines to what the card can draw.
fn fit_details(view: &mut DetailsView) {
    let mut title = [0u8; 64];
    let title_len = font::ellipsize(&view.title[..view.title_len], 2, 600, &mut title);
    if title_len > 0 {
        view.title = [0u8; 64];
        view.title[..title_len].copy_from_slice(&title[..title_len]);
        view.title_len = title_len;
    }
    for index in 0..view.line_count {
        let mut line = [0u8; MAX_DETAILS_LINE];
        let line_len = font::ellipsize(
            &view.lines[index][..view.line_lens[index]],
            1,
            600,
            &mut line,
        );
        if line_len > 0 {
            view.lines[index] = line;
            view.line_lens[index] = line_len;
        }
    }
}

/// Hands the selected document to the package that declared a handler for
/// its media type.
///
/// WHY the platform resolves the handler and not this app: the registry is
/// built from the installed packages' signed manifests, so the answer to
/// "who opens this" is a property of the device. An app that kept its own
/// table would be able to hand a file to a program that never asked for it.
///
/// The answer is a yes or a no. Which of the two refusals it was is read
/// from the document's own handler list, which the card already shows.
fn open_with_handler(app: &FileManagerApp) -> bool {
    let document = &app.selected_document;
    if !document.has_handler() {
        std::log("[FILE_MANAGER] No app handles this file.").ok();
        return false;
    }
    match std::content::intent_dispatch(
        std::content::IntentKind::Mime,
        document.mime_str(),
        document.path_str(),
    ) {
        Ok(_) => {
            std::log("[FILE_MANAGER] The document was dispatched to its handler.").ok();
            true
        }
        Err(code) => {
            std::log("[FILE_MANAGER] The document's handler could not be launched.").ok();
            log_counted("[FILE_MANAGER] Platform launch refused, code: -", code.unsigned_abs(), "");
            false
        }
    }
}

/// Opens the confirmation sheet for one of the file actions.
///
/// WHY the confirmation is a separate step: a copy, move, rename or delete
/// changes storage, and the app asks before it writes.
fn set_as_action(app: &mut FileManagerApp, action: FileAction) -> bool {
    app.open_confirmation(action)
}

// -----------------------------------------------------------------------------
// Vector pictograms
//
// WHY the icons are drawn here and not in the SDK: they are this app's own
// vocabulary for the media types it lists. The shapes are the same ones the
// list rows, the grid cells and the recents rows all use, so one file reads
// the same wherever it appears.
// -----------------------------------------------------------------------------

/// The ink for one pixel of a media-type pictogram, in a 34x26 box.
fn glyph_ink(mime: FileMime, dx: isize, dy: isize, ink: u32) -> Option<u32> {
    if dx < 0 || dy < 0 || dx > 33 || dy > 25 {
        return None;
    }
    let hit = match mime {
        FileMime::Directory => {
            let in_tab = dx <= 12 && dy >= 1 && dy <= 5;
            let in_body = dx <= 32 && dy >= 5 && dy <= 23;
            if in_tab || in_body {
                dy == 1
                    || dy == 23
                    || dx == 0
                    || dx == 32
                    || (dy == 5 && dx > 12)
                    || (dx == 12 && dy <= 5)
            } else {
                false
            }
        }
        FileMime::Image => {
            if dx >= 2 && dx <= 30 && dy <= 24 {
                let frame = dx == 2 || dx == 30 || dy == 0 || dy == 24;
                let sun_dx = dx - 22;
                let sun_dy = dy - 6;
                let sun = sun_dx * sun_dx + sun_dy * sun_dy <= 4;
                let mountain = dy >= 14 && (dx - 12).abs() <= 24 - dy;
                frame || sun || mountain
            } else {
                false
            }
        }
        FileMime::Video => {
            if dx >= 2 && dx <= 30 && dy <= 24 {
                let frame = dx == 2 || dx == 30 || dy == 0 || dy == 24;
                let play = dx >= 12 && dx <= 22 && (dy - 12).abs() <= dx - 12;
                frame || play
            } else {
                false
            }
        }
        FileMime::Audio => {
            let head_one = dx >= 4 && dx <= 10 && dy >= 17 && dy <= 22;
            let head_two = dx >= 16 && dx <= 22 && dy >= 14 && dy <= 19;
            let stem_one = dx == 10 && dy >= 2 && dy <= 19;
            let stem_two = dx == 22 && dy >= 2 && dy <= 16;
            let beam = dx >= 10 && dx <= 22 && dy >= 2 && dy <= 5;
            head_one || head_two || stem_one || stem_two || beam
        }
        FileMime::Document | FileMime::Other => {
            if dx >= 4 && dx <= 28 && dy <= 24 {
                let in_fold = dx > 20 && dy < 8 && (dx - 20) + dy > 8;
                if in_fold {
                    false
                } else {
                    let outline = dx == 4
                        || dx == 28
                        || dy == 0
                        || dy == 24
                        || ((dx >= 20 && dy <= 8) && ((dx - 20) + dy == 8));
                    let lines = (dy == 11 || dy == 16) && dx >= 9 && dx <= 23;
                    outline || lines
                }
            } else {
                false
            }
        }
    };
    if hit {
        Some(ink)
    } else {
        None
    }
}

// -----------------------------------------------------------------------------
// Pixel colour resolution
// -----------------------------------------------------------------------------

#[inline(never)]
fn get_file_manager_color(x: usize, y: usize, app: &FileManagerApp) -> u32 {
    if y < HEADER_H {
        return header_color(x, y, app);
    }
    match app.view {
        ViewMode::Home => home_color(x, y, app),
        ViewMode::Browse => browse_color(x, y, app),
    }
}

/// The header: the back control, the title and the measured readout.
fn header_color(x: usize, y: usize, app: &FileManagerApp) -> u32 {
    // Back control, painted from the same rectangle the tap handler tests.
    if BACK.contains(x, y) {
        if let Some(color) =
            draw_pill(x, y, BACK, THEME.surface_container_high, Some(THEME.border))
        {
            if color != THEME.surface_container_high {
                return color;
            }
            if let Some(ink) = std::design::back_button::chevron_ink(
                x,
                y,
                BACK.x + BACK.w / 2,
                BACK.y + BACK.h / 2,
                THEME.on_surface,
            ) {
                return ink;
            }
            return THEME.surface_container_high;
        }
        return THEME.surface;
    }

    // Title: "Files" at x: 48, y: 64 (scale 3).
    //
    // WHY the coordinates are written out instead of pulled from constants:
    // the native-UI truth contract pins this exact call so the title's line
    // box cannot be narrowed again.
    if let Some(color) = test_text(x, y, 48, 64, 3, b"Files", THEME.on_surface, THEME.surface) {
        return color;
    }

    // The path the app asked, and the platform's own answer. Line 1 is the
    // path really passed to `fs_list`; line 2 is the real outcome, including
    // a refusal.
    let path = &app.current_path[..app.current_path_len];
    let mut path_buf = [0u8; MAX_PATH_LEN];
    let path_len = font::ellipsize(path, 1, READOUT_MAX_W, &mut path_buf);
    let path_w = font::measure(&path_buf[..path_len], 1);
    if path_w <= READOUT_MAX_W {
        if let Some(color) = test_text(
            x,
            y,
            READOUT_X1 - path_w,
            READOUT_LINE1_Y0,
            1,
            &path_buf[..path_len],
            THEME.on_surface_variant,
            THEME.surface,
        ) {
            return color;
        }
    }

    let mut label_buf = [0u8; 40];
    let label_len = font::ellipsize(
        &app.probe_label[..app.probe_label_len],
        1,
        READOUT_MAX_W,
        &mut label_buf,
    );
    let label_w = font::measure(&label_buf[..label_len], 1);
    if label_w <= READOUT_MAX_W {
        if let Some(color) = test_text(
            x,
            y,
            READOUT_X1 - label_w,
            READOUT_LINE2_Y0,
            1,
            &label_buf[..label_len],
            THEME.warning,
            THEME.surface,
        ) {
            return color;
        }
    }

    THEME.surface
}

/// The home screen: the storage insight, the index's recents and the four
/// directory entry points.
fn home_color(x: usize, y: usize, app: &FileManagerApp) -> u32 {
    // 1. The storage card.
    if HOME_STORAGE_CARD.contains(x, y) {
        let style = components::card_style(&THEME, components::Elevation::Resting);
        let Some(base) = draw_rounded_rect(
            x,
            y,
            HOME_STORAGE_CARD,
            style.radius,
            style.fill,
            style.border,
            1,
        ) else {
            return THEME.surface;
        };
        if let Some(color) = test_text(x, y, 60, 172, 2, b"Storage", THEME.on_surface, style.fill)
        {
            return color;
        }
        if let Some(color) = test_text(
            x,
            y,
            60,
            228,
            1,
            &app.storage_line[..app.storage_line_len],
            THEME.on_surface_variant,
            style.fill,
        ) {
            return color;
        }
        if let Some(color) = test_text(
            x,
            y,
            60,
            256,
            1,
            app.storage_status(),
            THEME.warning,
            style.fill,
        ) {
            return color;
        }
        return base;
    }

    // 2. The recents section.
    if let Some(color) = test_text(
        x,
        y,
        36,
        HOME_RECENTS_TITLE_Y,
        2,
        b"Recently modified",
        THEME.on_surface,
        THEME.surface,
    ) {
        return color;
    }
    if let Some(color) = test_text(
        x,
        y,
        36,
        HOME_RECENTS_CAPTION_Y,
        1,
        b"Newest first, from the platform's document index",
        THEME.on_surface_variant,
        THEME.surface,
    ) {
        return color;
    }

    if app.recents_error != 0 || app.recents_count == 0 {
        let (line, ink): (&[u8], u32) = if app.recents_error != 0 {
            (
                b"The document index was refused; recents are unavailable",
                THEME.warning,
            )
        } else {
            (b"The document index holds no documents", THEME.on_surface_variant)
        };
        if let Some(color) = test_text(x, y, 36, HOME_RECENTS_Y0 + 8, 1, line, ink, THEME.surface)
        {
            return color;
        }
    } else {
        let visible = app.recents_count.min(HOME_RECENTS_VISIBLE);
        for index in 0..visible {
            let top = HOME_RECENTS_Y0 + index * HOME_RECENT_ROW_H;
            if y < top || y >= top + HOME_RECENT_ROW_H || x < 36 || x >= 684 {
                continue;
            }
            if y == top + HOME_RECENT_ROW_H - 1 {
                return ROW.divider;
            }
            let name = &app.recents[index].name[..app.recents[index].name_len];
            let mime = detect_mime(name, false);
            if let Some(ink) = glyph_ink(
                mime,
                x as isize - 47,
                y as isize - (top as isize + 15),
                THEME.on_surface,
            ) {
                return ink;
            }
            // The name column ends at the row's right edge; the 24px inset
            // matches the browse rows so a long name cannot reach the border.
            let max_w = 684 - 100 - 24;
            let mut name_buf = [0u8; 48];
            let name_len = font::ellipsize(name, 1, max_w, &mut name_buf);
            if name_len > 0 {
                if let Some(color) = test_text(
                    x,
                    y,
                    100,
                    top + 8,
                    1,
                    &name_buf[..name_len],
                    THEME.on_surface,
                    THEME.surface,
                ) {
                    return color;
                }
            }
            if let Some(color) = test_text(
                x,
                y,
                100,
                top + 30,
                1,
                &app.recent_time[index][..app.recent_time_len[index]],
                THEME.on_surface_variant,
                THEME.surface,
            ) {
                return color;
            }
            return THEME.surface;
        }
    }

    // 3. The category section.
    if let Some(color) = test_text(
        x,
        y,
        36,
        HOME_CATEGORY_TITLE_Y,
        2,
        b"Browse directories",
        THEME.on_surface,
        THEME.surface,
    ) {
        return color;
    }

    if (CATEGORY_Y0..CATEGORY_Y1).contains(&y) && (36..684).contains(&x) {
        let Some((index, row_top, row_h)) = category_row_at(y) else {
            return THEME.surface;
        };
        let selected = app.selected_shortcut == index;
        let fill = if selected { ROW_SELECTED.fill } else { ROW.fill };
        let rect = Rect::new(36, row_top, 648, row_h.saturating_sub(1));
        let Some(base) = draw_rounded_rect(x, y, rect, ROW.radius, fill, Some(ROW.divider), 1)
        else {
            return THEME.surface;
        };
        let icon_center = if index == 0 {
            row_top + 44
        } else {
            row_top + 35
        };
        let mime = match index {
            0 => FileMime::Directory,
            1 | 2 => FileMime::Image,
            _ => FileMime::Video,
        };
        if selected && x < 44 {
            return THEME.primary;
        }
        if let Some(ink) = glyph_ink(
            mime,
            x as isize - 47,
            y as isize - (icon_center as isize - 13),
            THEME.on_surface,
        ) {
            return ink;
        }
        let label_y = if index == 0 {
            row_top + 22
        } else {
            row_top + 11
        };
        if let Some(color) = test_text(
            x,
            y,
            108,
            label_y,
            2,
            CATEGORY_LABELS[index],
            THEME.on_surface,
            fill,
        ) {
            return color;
        }
        if index == 0 {
            if let Some(color) = test_text(
                x,
                y,
                108,
                row_top + 72,
                1,
                app.storage_status(),
                THEME.on_surface_variant,
                fill,
            ) {
                return color;
            }
        }
        return base;
    }

    // 4. The browse-everything control.
    if HOME_BROWSE_BTN.contains(x, y) {
        if let Some(base) = draw_pill(
            x,
            y,
            HOME_BROWSE_BTN,
            THEME.surface_container_high,
            Some(THEME.border),
        ) {
            let lx = centered_x(b"Browse all files", 1, HOME_BROWSE_BTN.x, HOME_BROWSE_BTN.w);
            let ly = HOME_BROWSE_BTN.y
                + HOME_BROWSE_BTN.h.saturating_sub(font::line_height(1)) / 2;
            if let Some(color) = test_text(
                x,
                y,
                lx,
                ly,
                1,
                b"Browse all files",
                THEME.primary,
                THEME.surface_container_high,
            ) {
                return color;
            }
            return base;
        }
        return THEME.surface;
    }

    // 5. The honest note.
    if let Some(color) = test_text(
        x,
        y,
        36,
        HOME_NOTE_Y,
        1,
        b"Recents come from the platform's document index.",
        THEME.on_surface_variant,
        THEME.surface,
    ) {
        return color;
    }
    if let Some(color) = test_text(
        x,
        y,
        36,
        HOME_NOTE_Y + 24,
        1,
        b"Files the index has not walked are not listed here.",
        THEME.on_surface_variant,
        THEME.surface,
    ) {
        return color;
    }

    // The home gesture pill, drawn from the rectangle the tap handler tests.
    if HOME_PILL.contains(x, y) {
        return THEME.on_surface_variant;
    }

    THEME.surface
}

/// The browse screen.
fn browse_color(x: usize, y: usize, app: &FileManagerApp) -> u32 {
    // 1. The search field and its clear action.
    if (SEARCH_FIELD.y..SEARCH_FIELD.y + SEARCH_FIELD.h).contains(&y)
        && x >= SEARCH_FIELD.x
        && x < SEARCH_FIELD.x + SEARCH_FIELD.w
    {
        let field_fill = THEME.surface_container_high;
        let Some(base) = draw_rounded_rect(
            x,
            y,
            SEARCH_FIELD,
            THEME.radii.control,
            field_fill,
            Some(THEME.border),
            1,
        ) else {
            return THEME.surface;
        };
        if !app.query.is_empty() && SEARCH_CLEAR.contains(x, y) {
            let cx = SEARCH_CLEAR.x + SEARCH_CLEAR.w / 2;
            let cy = SEARCH_CLEAR.y + SEARCH_CLEAR.h / 2;
            let dx = x as i32 - cx as i32;
            let dy = y as i32 - cy as i32;
            if dx.abs() <= 6 && dy.abs() <= 6 && (dx.abs() - dy.abs()).abs() <= 2 {
                return THEME.on_surface_variant;
            }
            return base;
        }
        let mx = SEARCH_FIELD.x + 28;
        let my = SEARCH_FIELD.y + 26;
        let dx = x as i32 - mx as i32;
        let dy = y as i32 - my as i32;
        let ring = dx * dx + dy * dy;
        if (ring >= 36 && ring <= 64) || (dx >= 6 && dx <= 11 && dy >= 6 && dy <= 11) {
            return THEME.on_surface_variant;
        }
        let text = if app.query.is_empty() {
            b"Search this device's documents".as_slice()
        } else {
            app.query.as_slice()
        };
        let ink = if app.query.is_empty() {
            THEME.on_surface_variant
        } else {
            THEME.on_surface
        };
        if let Some(color) = test_text(
            x,
            y,
            SEARCH_FIELD.x + 48,
            SEARCH_FIELD.y + 16,
            1,
            text,
            ink,
            field_fill,
        ) {
            return color;
        }
        return field_fill;
    }

    // 2. The search scope caption and its two pills.
    if (SCOPE_Y..SCOPE_Y + font::line_height(1)).contains(&y)
        && x >= 36
        && x < SCOPE_THIS_PILL.x
    {
        let mut buf = [0u8; LINE_LEN];
        let len = font::ellipsize(
            &app.scope[..app.scope_len],
            1,
            SCOPE_CAPTION_MAX_W,
            &mut buf,
        );
        if len > 0 {
            if let Some(color) = test_text(
                x,
                y,
                36,
                SCOPE_Y,
                1,
                &buf[..len],
                THEME.on_surface_variant,
                THEME.surface,
            ) {
                return color;
            }
        }
        return THEME.surface;
    }
    if let Some(color) = pill_color(
        x,
        y,
        SCOPE_THIS_PILL,
        SearchScope::ThisFolder.label(),
        app.search_scope == SearchScope::ThisFolder,
    ) {
        return color;
    }
    if let Some(color) = pill_color(
        x,
        y,
        SCOPE_ALL_PILL,
        SearchScope::AllStorage.label(),
        app.search_scope == SearchScope::AllStorage,
    ) {
        return color;
    }

    // 3. The breadcrumbs.
    if (BREADCRUMB_Y..BREADCRUMB_Y + font::line_height(1)).contains(&y)
        && (36..684).contains(&x)
    {
        let mut cursor = 36usize;
        for index in 0..app.crumb_count {
            let crumb = app.crumbs[index];
            let label = &app.current_path[crumb.label_start..crumb.label_start + crumb.label_len];
            let ink = if index + 1 == app.crumb_count {
                THEME.on_surface
            } else {
                THEME.primary
            };
            if let Some(color) =
                test_text(x, y, cursor, BREADCRUMB_Y, 1, label, ink, THEME.surface)
            {
                return color;
            }
            cursor += font::measure(label, 1) + 6;
            if index + 1 < app.crumb_count {
                if let Some(color) = test_text(
                    x,
                    y,
                    cursor,
                    BREADCRUMB_Y,
                    1,
                    b">",
                    THEME.on_surface_variant,
                    THEME.surface,
                ) {
                    return color;
                }
                cursor += font::measure(b">", 1) + 6;
            }
        }
        return THEME.surface;
    }

    // 4. The sort and layout toolbar.
    if let Some(color) = pill_color(
        x,
        y,
        SORT_PILL,
        &app.sort_label[..app.sort_label_len],
        false,
    ) {
        return color;
    }
    if let Some(color) = pill_color(
        x,
        y,
        LIST_PILL,
        LayoutMode::List.label(),
        app.layout == LayoutMode::List,
    ) {
        return color;
    }
    if let Some(color) = pill_color(
        x,
        y,
        GRID_PILL,
        LayoutMode::Grid.label(),
        app.layout == LayoutMode::Grid,
    ) {
        return color;
    }

    // 5. The details card, drawn over the entries card.
    if app.has_details {
        if let Some(color) = details_color(x, y, app) {
            return color;
        }
    }

    // 6. The confirmation sheet, drawn over the entries card.
    if app.has_pending {
        if let Some(color) = sheet_color(x, y, app) {
            return color;
        }
    }

    // 7. The entries card.
    if (ENTRIES_CARD.y..ENTRIES_CARD.y + ENTRIES_CARD.h).contains(&y) {
        if x < ENTRIES_CARD.x || x >= ENTRIES_CARD.x + ENTRIES_CARD.w {
            return THEME.surface;
        }
        let style = components::card_style(&THEME, components::Elevation::Resting);
        let Some(base) = draw_rounded_rect(
            x,
            y,
            ENTRIES_CARD,
            style.radius,
            style.fill,
            style.border,
            1,
        ) else {
            return THEME.surface;
        };
        if app.card_item_count() == 0 {
            // Folder pictogram above the headline.
            if let Some(ink) = glyph_ink(
                FileMime::Directory,
                x as isize - 343,
                y as isize - (ENTRIES_CARD.y as isize + 97),
                THEME.primary,
            ) {
                return ink;
            }
            let (headline, subline, headline_ink): (&[u8], &[u8], u32) = if app.searching {
                if app.search_error != 0 {
                    (
                        b"Search Unavailable",
                        b"The device refused the search; see the status line",
                        THEME.warning,
                    )
                } else {
                    (
                        b"No Matches",
                        b"The index holds no document with that name",
                        THEME.on_surface,
                    )
                }
            } else {
                match app.listing_state {
                    ListingState::Refused => (
                        b"Directory Unreadable",
                        b"The device refused to list this directory",
                        THEME.warning,
                    ),
                    ListingState::NotListed => (
                        b"Not Listed Yet",
                        b"This directory has not been listed",
                        THEME.on_surface_variant,
                    ),
                    _ => (
                        b"Directory Empty",
                        b"This directory holds no entries",
                        THEME.on_surface,
                    ),
                }
            };
            let hx = centered_x(headline, 2, ENTRIES_CARD.x, ENTRIES_CARD.w);
            if let Some(color) = test_text(
                x,
                y,
                hx,
                ENTRIES_CARD.y + 150,
                2,
                headline,
                headline_ink,
                style.fill,
            ) {
                return color;
            }
            let sx = centered_x(subline, 1, ENTRIES_CARD.x, ENTRIES_CARD.w);
            if let Some(color) = test_text(
                x,
                y,
                sx,
                ENTRIES_CARD.y + 214,
                1,
                subline,
                THEME.on_surface_variant,
                style.fill,
            ) {
                return color;
            }
        }
        return base;
    }

    // 8. The status line.
    if (STATUS_Y..STATUS_Y + font::line_height(1)).contains(&y) && (36..684).contains(&x) {
        if app.status_len > 0 {
            if let Some(color) = test_text(
                x,
                y,
                36,
                STATUS_Y,
                1,
                &app.status[..app.status_len],
                THEME.on_surface_variant,
                THEME.surface,
            ) {
                return color;
            }
        }
        return THEME.surface;
    }

    // 9. The action bar.
    if (ACTION_BAR_Y0..ACTION_BAR_Y1).contains(&y) {
        for slot in 0..7usize {
            let rect = action_slot_rect(slot);
            if !rect.contains(x, y) {
                continue;
            }
            let (label, enabled, selected) = action_slot_state(app, slot);
            let fill = if selected {
                THEME.inverted_surface()
            } else if enabled {
                THEME.surface_container_high
            } else {
                THEME.surface
            };
            let ink = if selected {
                THEME.on_inverted_surface()
            } else if enabled {
                THEME.on_surface
            } else {
                THEME.muted
            };
            if let Some(base) = draw_pill(x, y, rect, fill, Some(THEME.border)) {
                let lx = centered_x(label, 1, rect.x, rect.w);
                let ly = rect.y + rect.h.saturating_sub(font::line_height(1)) / 2;
                if let Some(color) = test_text(x, y, lx, ly, 1, label, ink, fill) {
                    return color;
                }
                return base;
            }
            return THEME.surface;
        }
        return THEME.surface;
    }

    // The home gesture pill, drawn from the rectangle the tap handler tests.
    if HOME_PILL.contains(x, y) {
        return THEME.on_surface_variant;
    }

    THEME.surface
}

/// One of the rounded toolbar or scope pills.
fn pill_color(
    x: usize,
    y: usize,
    rect: Rect,
    label: &[u8],
    selected: bool,
) -> Option<u32> {
    if !rect.contains(x, y) {
        return None;
    }
    let fill = if selected {
        THEME.inverted_surface()
    } else {
        THEME.surface_container_high
    };
    let ink = if selected {
        THEME.on_inverted_surface()
    } else {
        THEME.on_surface
    };
    let base = draw_pill(x, y, rect, fill, Some(THEME.border))?;
    let lx = centered_x(label, 1, rect.x, rect.w);
    let ly = rect.y + rect.h.saturating_sub(font::line_height(1)) / 2;
    if let Some(color) = test_text(x, y, lx, ly, 1, label, ink, fill) {
        return Some(color);
    }
    Some(base)
}

/// The label, whether it can be taken, and whether it is the current mode.
fn action_slot_state(app: &FileManagerApp, slot: usize) -> (&'static [u8], bool, bool) {
    match slot {
        0 => (b"Select", true, app.multi_select),
        1 => (b"All", app.card_item_count() > 0, false),
        _ => match action_for_slot(slot) {
            Some(action) => {
                let enabled = if action == FileAction::NewFolder {
                    // The control exists so the refusal can be reported; the
                    // platform has no directory-creation call.
                    true
                } else {
                    app.action_is_possible(action)
                };
                (ACTION_LABELS[action as usize], enabled, false)
            }
            None => (b"", false, false),
        },
    }
}

/// The details card, drawn over the entries card.
fn details_color(x: usize, y: usize, app: &FileManagerApp) -> Option<u32> {
    if !DETAILS_CARD.contains(x, y) {
        return None;
    }
    let style = components::card_style(&THEME, components::Elevation::Raised);
    let Some(base) = draw_rounded_rect(
        x,
        y,
        DETAILS_CARD,
        style.radius,
        style.fill,
        style.border,
        1,
    ) else {
        return Some(THEME.surface);
    };
    if let Some(color) = test_text(
        x,
        y,
        DETAILS_CARD.x + 24,
        DETAILS_TITLE_Y,
        2,
        app.details.title_str().as_bytes(),
        THEME.on_surface,
        style.fill,
    ) {
        return Some(color);
    }
    for index in 0..app.details.line_count {
        let line = &app.details.lines[index][..app.details.line_lens[index]];
        if let Some(color) = test_text(
            x,
            y,
            DETAILS_CARD.x + 24,
            DETAILS_LINE_Y0 + index * DETAILS_LINE_H,
            1,
            line,
            if index == 3 && !app.details.has_handler {
                THEME.warning
            } else {
                THEME.on_surface_variant
            },
            style.fill,
        ) {
            return Some(color);
        }
    }
    for (rect, label, primary) in [
        (DETAILS_OPEN, b"Open".as_slice(), true),
        (DETAILS_CLOSE, b"Close".as_slice(), false),
    ] {
        if primary && !app.details.is_dir && !app.details.has_handler {
            // WHY the button is absent rather than greyed out: a button that
            // opens nothing is a promise the device cannot keep. The card
            // names the missing handler in its own line, so the reason is
            // still on the screen.
            continue;
        }
        if !rect.contains(x, y) {
            continue;
        }
        let fill = if primary {
            THEME.primary
        } else {
            THEME.surface_container_high
        };
        let ink = if primary {
            THEME.on_primary
        } else {
            THEME.on_surface
        };
        if let Some(button) = draw_rounded_rect(x, y, rect, THEME.radii.control, fill, None, 0) {
            let lx = centered_x(label, 1, rect.x, rect.w);
            let ly = rect.y + rect.h.saturating_sub(font::line_height(1)) / 2;
            if let Some(color) = test_text(x, y, lx, ly, 1, label, ink, fill) {
                return Some(color);
            }
            return Some(button);
        }
        return Some(THEME.surface);
    }
    Some(base)
}

/// The confirmation sheet, drawn over the entries card.
fn sheet_color(x: usize, y: usize, app: &FileManagerApp) -> Option<u32> {
    if !SHEET.contains(x, y) {
        return None;
    }
    let style = components::card_style(&THEME, components::Elevation::Raised);
    let Some(base) = draw_rounded_rect(x, y, SHEET, style.radius, style.fill, style.border, 1)
    else {
        return Some(THEME.surface);
    };
    if let Some(color) = test_text(
        x,
        y,
        SHEET.x + 20,
        SHEET.y + 20,
        2,
        &app.sheet_title[..app.sheet_title_len],
        THEME.on_surface,
        style.fill,
    ) {
        return Some(color);
    }
    for index in 0..app.sheet_name_count {
        if let Some(color) = test_text(
            x,
            y,
            SHEET.x + 20,
            SHEET.y + 76 + index * 24,
            1,
            &app.sheet_names[index][..app.sheet_name_lens[index]],
            THEME.on_surface_variant,
            style.fill,
        ) {
            return Some(color);
        }
    }
    if app.pending.action == FileAction::Copy || app.pending.action == FileAction::Move {
        for index in 0..CATEGORY_COUNT {
            let rect = sheet_destination_rect(index);
            if !rect.contains(x, y) {
                continue;
            }
            let chosen = index == app.destination;
            let fill = if chosen {
                THEME.primary
            } else {
                THEME.surface_container_high
            };
            let ink = if chosen {
                THEME.on_primary
            } else {
                THEME.on_surface
            };
            let Some(button) = draw_rounded_rect(x, y, rect, THEME.radii.control, fill, None, 0)
            else {
                return Some(THEME.surface);
            };
            let lx = centered_x(CATEGORY_LABELS[index], 1, rect.x, rect.w);
            let ly = rect.y + rect.h.saturating_sub(font::line_height(1)) / 2;
            if let Some(color) = test_text(
                x,
                y,
                lx,
                ly,
                1,
                CATEGORY_LABELS[index],
                ink,
                fill,
            ) {
                return Some(color);
            }
            return Some(button);
        }
    }
    if app.pending.action == FileAction::Rename {
        let field = Rect::new(SHEET.x + 20, SHEET_NAME_Y, SHEET.w - 40, SHEET_NAME_H);
        if field.contains(x, y) {
            let fill = THEME.surface_container_high;
            if let Some(color) = test_text(
                x,
                y,
                field.x + 12,
                field.y + 12,
                1,
                app.rename_field.as_slice(),
                THEME.on_surface,
                fill,
            ) {
                return Some(color);
            }
            return Some(fill);
        }
    }
    for (index, label) in [b"Cancel".as_slice(), b"Confirm".as_slice()]
        .iter()
        .enumerate()
    {
        let rect = sheet_button_rect(index == 1);
        if !rect.contains(x, y) {
            continue;
        }
        let fill = if index == 1 {
            THEME.primary
        } else {
            THEME.surface_container_high
        };
        let ink = if index == 1 {
            THEME.on_primary
        } else {
            THEME.on_surface
        };
        let Some(button) = draw_rounded_rect(x, y, rect, THEME.radii.control, fill, None, 0)
        else {
            return Some(THEME.surface);
        };
        let lx = centered_x(label, 1, rect.x, rect.w);
        let ly = rect.y + rect.h.saturating_sub(font::line_height(1)) / 2;
        if let Some(color) = test_text(x, y, lx, ly, 1, label, ink, fill) {
            return Some(color);
        }
        return Some(button);
    }
    Some(base)
}

// -----------------------------------------------------------------------------
// Widget rendering
// -----------------------------------------------------------------------------

const ROW_BUFFER_W: usize = 648;
const ROW_BUFFER_H: usize = 56;
static mut ROW_BUFFER: [u32; ROW_BUFFER_W * ROW_BUFFER_H] = [0; ROW_BUFFER_W * ROW_BUFFER_H];

const CELL_BUFFER_W: usize = GRID_CELL_W;
const CELL_BUFFER_H: usize = GRID_CELL_H;
static mut CELL_BUFFER: [u32; CELL_BUFFER_W * CELL_BUFFER_H] =
    [0; CELL_BUFFER_W * CELL_BUFFER_H];

/// Draws the platform's list rows, the grid cells and the scroll thumb.
fn render_widgets(surface: u32, app: &FileManagerApp) {
    if surface == 0 || app.view != ViewMode::Browse || app.has_pending || app.has_details {
        return;
    }
    match app.layout {
        LayoutMode::List => render_rows(surface, app),
        LayoutMode::Grid => render_grid(surface, app),
    }
    if let Some((thumb_y, thumb_h)) =
        app.scroll
            .thumb_rect(ENTRIES_CARD.y + 4, ENTRIES_CARD.h - 8, 24)
    {
        let tx = ENTRIES_CARD.x + ENTRIES_CARD.w - 10;
        let thumb = [THEME.primary; 4];
        for y in thumb_y..thumb_y + thumb_h {
            let _ = std::surface_write(surface, y * W + tx, &thumb);
        }
    }
}

fn render_rows(surface: u32, app: &FileManagerApp) {
    let count = app.card_item_count();
    if count == 0 {
        return;
    }
    let offset = app.scroll.offset;
    let is_video_ctx = app.current_path_str().starts_with("/storage/videos");
    for index in 0..count {
        if !row_is_visible(ENTRIES_CARD, index, offset) {
            continue;
        }
        let rect = entry_row_rect(ENTRIES_CARD, index, offset);
        let selected = app.selected_entry == Some(index) || app.selection.is_marked(index);
        let fill = if selected { ROW_SELECTED.fill } else { ROW.fill };
        let name = app.card_item_name(index);
        let is_dir = app.card_item_is_dir(index);
        let mime = if !is_dir
            && is_video_ctx
            && (has_suffix(name, b".ppm") || has_suffix(name, b".raw"))
        {
            FileMime::Video
        } else {
            detect_mime(name, is_dir)
        };
        let ink = if selected { THEME.primary } else { THEME.on_surface };
        let buffer: &mut [u32] = unsafe { &mut *(&raw mut ROW_BUFFER) };
        for ry in 0..ROW_BUFFER_H {
            for rx in 0..ROW_BUFFER_W {
                let mut color = fill;
                if ry == ROW_BUFFER_H - 1 {
                    color = ROW.divider;
                }
                if selected && rx < 6 {
                    color = THEME.primary;
                }
                buffer[ry * ROW_BUFFER_W + rx] = color;
            }
        }
        paint_glyph(
            buffer,
            ROW_BUFFER_W,
            ROW_BUFFER_H,
            20,
            16,
            mime,
            ink,
        );

        if app.searching {
            // A search result shows its name and the path it was found at:
            // two files with the same name in different folders must not look
            // like the same result.
            let max_w = ROW_BUFFER_W - 64 - 20;
            let mut name_buf = [0u8; 48];
            let name_len = font::ellipsize(name, 1, max_w, &mut name_buf);
            if name_len > 0 {
                draw_buffer_text(
                    buffer,
                    ROW_BUFFER_W,
                    ROW_BUFFER_H,
                    64,
                    6,
                    1,
                    &name_buf[..name_len],
                    THEME.on_surface,
                );
            }
            let mut path = [0u8; MAX_PATH_LEN];
            if let Some(path_len) = app.card_item_path(index, &mut path) {
                let mut path_buf = [0u8; MAX_PATH_LEN];
                let shown = font::ellipsize(&path[..path_len], 1, max_w, &mut path_buf);
                if shown > 0 {
                    draw_buffer_text(
                        buffer,
                        ROW_BUFFER_W,
                        ROW_BUFFER_H,
                        64,
                        28,
                        1,
                        &path_buf[..shown],
                        THEME.on_surface_variant,
                    );
                }
            }
        } else {
            let trailing = app.card_item_trailing(index);
            let trailing_w = font::measure(trailing, 2);
            let max_w = ROW_BUFFER_W.saturating_sub(64 + trailing_w + 20);
            let mut name_buf = [0u8; 48];
            let name_len = font::ellipsize(name, 2, max_w, &mut name_buf);
            if name_len > 0 {
                draw_buffer_text(
                    buffer,
                    ROW_BUFFER_W,
                    ROW_BUFFER_H,
                    64,
                    (ROW_BUFFER_H - font::line_height(2)) / 2,
                    2,
                    &name_buf[..name_len],
                    THEME.on_surface,
                );
            }
            if trailing_w > 0 {
                draw_buffer_text(
                    buffer,
                    ROW_BUFFER_W,
                    ROW_BUFFER_H,
                    ROW_BUFFER_W - 20 - trailing_w,
                    (ROW_BUFFER_H - font::line_height(2)) / 2,
                    2,
                    trailing,
                    THEME.on_surface_variant,
                );
            }
        }

        // Only the part of the row that is on the card is written.
        let top = rect.y.max(ENTRIES_CARD.y);
        let bottom = (rect.y + ROW_BUFFER_H).min(ENTRIES_CARD.y + ENTRIES_CARD.h);
        if bottom <= top {
            continue;
        }
        let from = top - rect.y;
        let rows = bottom - top;
        write_pixels_rows(
            surface,
            top * W + ENTRIES_CARD.x,
            &buffer[from * ROW_BUFFER_W..(from + rows) * ROW_BUFFER_W],
            ROW_BUFFER_W,
        );
    }
}

fn render_grid(surface: u32, app: &FileManagerApp) {
    let count = app.card_item_count();
    if count == 0 {
        return;
    }
    let offset = app.scroll.offset;
    let is_video_ctx = app.current_path_str().starts_with("/storage/videos");
    for index in 0..count {
        let rect = grid_cell_rect(ENTRIES_CARD, index, offset);
        if rect.y >= ENTRIES_CARD.y + ENTRIES_CARD.h
            || rect.y + rect.h <= ENTRIES_CARD.y
        {
            continue;
        }
        let selected = app.selected_entry == Some(index) || app.selection.is_marked(index);
        let fill = if selected {
            THEME.surface_container_high
        } else {
            THEME.surface
        };
        let name = app.card_item_name(index);
        let is_dir = app.card_item_is_dir(index);
        let mime = if !is_dir
            && is_video_ctx
            && (has_suffix(name, b".ppm") || has_suffix(name, b".raw"))
        {
            FileMime::Video
        } else {
            detect_mime(name, is_dir)
        };
        let ink = if selected { THEME.primary } else { THEME.on_surface };
        let buffer: &mut [u32] = unsafe { &mut *(&raw mut CELL_BUFFER) };
        for py in 0..CELL_BUFFER_H {
            for px in 0..CELL_BUFFER_W {
                let mut color = fill;
                if selected && px < 4 {
                    color = THEME.primary;
                }
                if py == CELL_BUFFER_H - 1 {
                    color = ROW.divider;
                }
                buffer[py * CELL_BUFFER_W + px] = color;
            }
        }
        paint_glyph(buffer, CELL_BUFFER_W, CELL_BUFFER_H, 83, 30, mime, ink);

        let mut name_buf = [0u8; 48];
        let name_len = font::ellipsize(name, 1, CELL_BUFFER_W - 24, &mut name_buf);
        if name_len > 0 {
            draw_buffer_text(
                buffer,
                CELL_BUFFER_W,
                CELL_BUFFER_H,
                12,
                66,
                1,
                &name_buf[..name_len],
                ink,
            );
        }
        if app.searching {
            let mut path = [0u8; MAX_PATH_LEN];
            if let Some(path_len) = app.card_item_path(index, &mut path) {
                let mut path_buf = [0u8; MAX_PATH_LEN];
                let shown = font::ellipsize(&path[..path_len], 1, CELL_BUFFER_W - 24, &mut path_buf);
                if shown > 0 {
                    draw_buffer_text(
                        buffer,
                        CELL_BUFFER_W,
                        CELL_BUFFER_H,
                        12,
                        96,
                        1,
                        &path_buf[..shown],
                        THEME.on_surface_variant,
                    );
                }
            }
        } else {
            let trailing = app.card_item_trailing(index);
            if !trailing.is_empty() {
                draw_buffer_text(
                    buffer,
                    CELL_BUFFER_W,
                    CELL_BUFFER_H,
                    12,
                    96,
                    1,
                    trailing,
                    THEME.on_surface_variant,
                );
            }
        }

        let top = rect.y.max(ENTRIES_CARD.y);
        let bottom = (rect.y + GRID_CELL_H).min(ENTRIES_CARD.y + ENTRIES_CARD.h);
        if bottom <= top {
            continue;
        }
        let from = top - rect.y;
        let rows = bottom - top;
        write_pixels_rows(
            surface,
            top * W + rect.x,
            &buffer[from * CELL_BUFFER_W..(from + rows) * CELL_BUFFER_W],
            CELL_BUFFER_W,
        );
    }
}

/// Paints one pictogram into a buffer at `(icon_x, icon_y)`.
fn paint_glyph(
    buffer: &mut [u32],
    width: usize,
    height: usize,
    icon_x: usize,
    icon_y: usize,
    mime: FileMime,
    ink: u32,
) {
    for dy in 0..26usize {
        let py = icon_y + dy;
        if py >= height {
            break;
        }
        for dx in 0..34usize {
            let px = icon_x + dx;
            if px >= width {
                break;
            }
            if let Some(color) = glyph_ink(mime, dx as isize, dy as isize, ink) {
                buffer[py * width + px] = color;
            }
        }
    }
}

/// Draws a text run into a widget buffer, blending toward the pixels under it.
fn draw_buffer_text(
    buffer: &mut [u32],
    width: usize,
    height: usize,
    x0: usize,
    y0: usize,
    scale: usize,
    text: &[u8],
    color: u32,
) {
    let line_h = font::line_height(scale);
    let text_w = font::measure(text, scale);
    let x1 = (x0 + text_w).min(width);
    let y1 = (y0 + line_h).min(height);
    for py in y0..y1 {
        for px in x0..x1 {
            let bg = buffer[py * width + px];
            if let Some(inked) =
                font::test_text_aa(px, py, x0, y0, scale, text, color, bg, false)
            {
                buffer[py * width + px] = inked;
            }
        }
    }
}

/// Writes a widget buffer into the surface, one row at a time.
fn write_pixels_rows(surface: u32, offset: usize, pixels: &[u32], row_w: usize) {
    let mut chunk = [0u32; CHUNK];
    let mut src = 0usize;
    let mut row = 0usize;
    let mut col = 0usize;
    while src < pixels.len() {
        let count = (pixels.len() - src).min(row_w - col).min(CHUNK);
        chunk[..count].copy_from_slice(&pixels[src..src + count]);
        let _ = std::surface_write(surface, offset + row * W + col, &chunk[..count]);
        src += count;
        col += count;
        if col == row_w {
            col = 0;
            row += 1;
        }
    }
}

fn render_file_manager_ui(surface: u32, app: &FileManagerApp) {
    render_file_manager_band(surface, app, 0, H);
}

/// Re-rasterises and writes only rows `y0..y1`.
fn render_file_manager_band(surface: u32, app: &FileManagerApp, y0: usize, y1: usize) {
    let mut chunk = [0u32; CHUNK];
    let mut offset = y0 * W;
    let end = y1.min(H) * W;
    while offset < end {
        let count = (end - offset).min(CHUNK);
        let mut x = offset % W;
        let mut y = offset / W;
        for i in 0..count {
            chunk[i] = get_file_manager_color(x, y, app);
            x += 1;
            if x == W {
                x = 0;
                y += 1;
            }
        }
        let _ = std::surface_write(surface, offset, &chunk[..count]);
        offset += count;
    }
    if surface > 0
        && y0 < ENTRIES_CARD.y + ENTRIES_CARD.h
        && y1 > ENTRIES_CARD.y
    {
        render_widgets(surface, app);
    }
}

// -----------------------------------------------------------------------------
// The file operations
// -----------------------------------------------------------------------------

/// Copies a file with the platform's own calls.
///
/// WHY a copy and not a rename: the platform's filesystem offers open, read,
/// write, sync, stat, list and unlink, and nothing else. A rename is a copy to
/// the new name followed by an unlink of the old one, and the progress line
/// says so.
fn copy_file(source: &str, target: &str) -> Result<u64, i64> {
    let src = std::fs_open(source, std::FS_READ)?;
    let dst = match std::fs_open(target, std::FS_WRITE) {
        Ok(handle) => handle,
        Err(code) => {
            let _ = std::fs_close(src);
            return Err(code);
        }
    };
    let mut buf = [0u8; 4096];
    let mut total = 0u64;
    loop {
        let read = match std::fs_read(src, &mut buf) {
            Ok(0) => break,
            Ok(n) => n,
            Err(code) => {
                let _ = std::fs_close(src);
                let _ = std::fs_close(dst);
                return Err(code);
            }
        };
        let mut written = 0usize;
        while written < read {
            match std::fs_write(dst, &buf[written..read]) {
                Ok(0) => {
                    let _ = std::fs_close(src);
                    let _ = std::fs_close(dst);
                    return Err(-5);
                }
                Ok(n) => written += n,
                Err(code) => {
                    let _ = std::fs_close(src);
                    let _ = std::fs_close(dst);
                    return Err(code);
                }
            }
        }
        total += read as u64;
    }
    let _ = std::fs_fsync(dst);
    let _ = std::fs_close(dst);
    let _ = std::fs_close(src);
    Ok(total)
}

fn delete_file(path: &str) -> Result<(), i64> {
    std::fs_unlink(path)
}

/// Whether something already holds a file at a path.
fn file_exists(path: &str) -> bool {
    match std::fs_open(path, std::FS_READ) {
        Ok(handle) => {
            let _ = std::fs_close(handle);
            true
        }
        Err(_) => false,
    }
}

/// Remembers the first per-item failure so the result line can name it.
fn record_failure(app: &mut FileManagerApp, name: &[u8], reason: &[u8]) {
    if app.fail_name_len > 0 {
        return;
    }
    let take = name.len().min(MAX_ENTRY_NAME_LEN);
    app.fail_name = [0u8; MAX_ENTRY_NAME_LEN];
    app.fail_name[..take].copy_from_slice(&name[..take]);
    app.fail_name_len = take;
    let take = reason.len().min(64);
    app.fail_reason = [0u8; 64];
    app.fail_reason[..take].copy_from_slice(&reason[..take]);
    app.fail_reason_len = take;
}

/// Performs the confirmed action, one file at a time.
///
/// WHY the progress is written after every file: a copy of a large file takes
/// a moment, and a status line that only changes when everything is done is a
/// status line that never says anything.
///
/// WHY every file is validated and its failure recorded: a batch that reports
/// "2 failed" without naming one leaves the user unable to find out which.
fn perform_action(app: &mut FileManagerApp, surface: u32) {
    let action = app.pending.action;
    let count = app.pending.item_count.min(MAX_PARSED_ENTRIES);
    let items = app.pending.items;
    let destination = CATEGORY_PATHS[app.destination.min(CATEGORY_COUNT - 1)];
    let mut typed = [0u8; MAX_ENTRY_NAME_LEN];
    let typed_len = app.rename_field.as_slice().len().min(MAX_ENTRY_NAME_LEN);
    typed[..typed_len].copy_from_slice(&app.rename_field.as_slice()[..typed_len]);
    app.close_confirmation();
    app.fail_name_len = 0;
    app.fail_reason_len = 0;

    if action == FileAction::NewFolder {
        app.set_notice(problem_text(OpProblem::UnsupportedByPlatform));
        return;
    }

    let mut done = 0usize;
    let mut failed = 0usize;
    for position in 0..count {
        let row = items[position];
        let mut source = [0u8; MAX_PATH_LEN];
        let Some(source_len) = app.card_item_path(row, &mut source) else {
            failed += 1;
            record_failure(app, b"?", b"the row is no longer in this listing");
            continue;
        };
        let mut name = [0u8; MAX_ENTRY_NAME_LEN];
        let name_len = app.card_item_name(row).len().min(MAX_ENTRY_NAME_LEN);
        name[..name_len].copy_from_slice(&app.card_item_name(row)[..name_len]);
        let is_dir = app.card_item_is_dir(row);
        let Ok(source_str) = core::str::from_utf8(&source[..source_len]) else {
            failed += 1;
            record_failure(app, &name[..name_len], b"the path is not text");
            continue;
        };

        let mut target = [0u8; MAX_PATH_LEN];
        let target_len = if action == FileAction::Rename {
            join_path(app.current_path_str(), &typed[..typed_len], &mut target)
        } else {
            join_path(destination, &name[..name_len], &mut target)
        };
        let Some(target_len) = target_len else {
            failed += 1;
            record_failure(app, &name[..name_len], b"the target path is too long");
            continue;
        };
        let Ok(target_str) = core::str::from_utf8(&target[..target_len]) else {
            failed += 1;
            record_failure(app, &name[..name_len], b"the target path is not text");
            continue;
        };

        let target_exists = file_exists(target_str);
        if let Err(problem) = validate_operation(
            action,
            1,
            is_dir,
            &typed[..typed_len],
            &name[..name_len],
            target_exists,
        ) {
            failed += 1;
            record_failure(app, &name[..name_len], problem_text(problem));
            continue;
        }

        // The progress line, before the work: it says which file is being
        // worked on and how many are left.
        let mut status = [0u8; LINE_LEN];
        let mut len = append(&mut status, 0, ACTION_LABELS[action as usize]);
        len = append(&mut status, len, b" ");
        len = append_u64(&mut status, len, (position + 1) as u64);
        len = append(&mut status, len, b" of ");
        len = append_u64(&mut status, len, count as u64);
        len = append(&mut status, len, b": ");
        len = append(&mut status, len, &name[..name_len]);
        app.status = status;
        app.status_len = len;
        app.notice_len = 0;
        if surface > 0 {
            render_file_manager_band(surface, app, STATUS_Y, STATUS_Y + font::line_height(1));
            let _ = std::surface_flip();
        }

        let outcome: Result<(), i64> = match action {
            FileAction::Delete => delete_file(source_str),
            FileAction::Copy => {
                if target_exists {
                    Err(-17)
                } else {
                    copy_file(source_str, target_str).map(|_| ())
                }
            }
            FileAction::Move | FileAction::Rename => {
                if target_exists {
                    Err(-17)
                } else {
                    match copy_file(source_str, target_str) {
                        Ok(_) => delete_file(source_str),
                        Err(code) => Err(code),
                    }
                }
            }
            FileAction::NewFolder => Err(-38),
        };
        match outcome {
            Ok(()) => done += 1,
            Err(code) => {
                failed += 1;
                let mut reason = [0u8; 32];
                let mut len = append(&mut reason, 0, b"platform code -");
                len = append_u64(&mut reason, len, code.unsigned_abs());
                record_failure(app, &name[..name_len], &reason[..len]);
            }
        }
    }

    // The result line says what happened, how many files it happened to, and
    // which file failed first if any did.
    let mut line = [0u8; LINE_LEN];
    let mut len = append(&mut line, 0, ACTION_LABELS[action as usize]);
    len = append(&mut line, len, b": ");
    len = append_u64(&mut line, len, done as u64);
    len = append(&mut line, len, b" done, ");
    len = append_u64(&mut line, len, failed as u64);
    len = append(&mut line, len, b" failed");
    if failed > 0 && app.fail_name_len > 0 {
        len = append(&mut line, len, b" - first: ");
        let take = app.fail_name_len.min(line.len().saturating_sub(len + 32));
        len = append(&mut line, len, &app.fail_name[..take]);
        len = append(&mut line, len, b" (");
        let take = app.fail_reason_len.min(line.len().saturating_sub(len + 1));
        len = append(&mut line, len, &app.fail_reason[..take]);
        len = append(&mut line, len, b")");
    }
    app.notice = line;
    app.notice_len = len;

    // The listing changed, so the listing is read again. The notice survives
    // the refresh because it speaks about what just happened.
    app.selection.clear();
    app.multi_select = false;
    app.selected_entry = None;
    app.refresh_directory();
    app.load_recents();
    if surface > 0 {
        render_file_manager_ui(surface, app);
        let _ = std::surface_flip();
    }
}

// -----------------------------------------------------------------------------
// Input
// -----------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq)]
enum Dirty {
    /// Nothing moved; the surface is already correct, so do not render at all.
    None,
    /// Anything that can move pixels in more than one band at once.
    All,
}

enum Tap {
    None,
    Redraw(Dirty),
    Leave(&'static str),
}

fn exit_to_launcher(surface: u32) -> ! {
    if surface > 0 {
        // WHY the surface is destroyed without naming a capability: the kernel
        // authorises the call by capability type, and a slot number is a
        // position in a table rather than a capability.
        let _ = std::surface_destroy(surface);
    }
    std::exit_with_code(0);
}

/// The breadcrumb whose label contains `x`, and the path prefix it opens.
fn crumb_at(app: &FileManagerApp, x: usize) -> Option<usize> {
    let mut cursor = 36usize;
    for index in 0..app.crumb_count {
        let crumb = app.crumbs[index];
        let label = &app.current_path[crumb.label_start..crumb.label_start + crumb.label_len];
        let width = font::measure(label, 1);
        if x >= cursor && x < cursor + width {
            return Some(index);
        }
        cursor += width + 6;
        if index + 1 < app.crumb_count {
            cursor += font::measure(b">", 1) + 6;
        }
    }
    None
}

fn handle_tap(app: &mut FileManagerApp, x: usize, y: usize, surface: u32) -> Tap {
    // The confirmation sheet owns every touch while it is up.
    if app.has_pending {
        return handle_sheet_tap(app, x, y, surface);
    }
    // The details card owns every touch while it is up, except the action bar
    // beneath it: that bar stays visible, and a single tap there both closes
    // the card and runs the action. `close_details` keeps the row selected,
    // so Copy, Move, Delete and Rename still know which file they mean.
    if app.has_details {
        if (ACTION_BAR_Y0..ACTION_BAR_Y1).contains(&y) {
            for slot in 0..7usize {
                if action_slot_rect(slot).contains(x, y) {
                    app.close_details();
                    return handle_action_slot(app, slot);
                }
            }
        }
        return handle_details_tap(app, x, y);
    }
    app.clear_notice();

    // The home indicator strip.
    if HOME_HIT.contains(x, y) {
        return Tap::Leave("[FILE_MANAGER] Home gesture pressed. Exiting to launcher.");
    }
    // The drawn back control.
    if BACK.contains(x, y) {
        return handle_back(app);
    }

    match app.view {
        ViewMode::Home => handle_home_tap(app, x, y),
        ViewMode::Browse => handle_browse_tap(app, x, y, surface),
    }
}

/// The back control: close what is open, go up a folder, then go home.
fn handle_back(app: &mut FileManagerApp) -> Tap {
    if app.has_pending {
        app.close_confirmation();
        std::log("[FILE_MANAGER] The action was cancelled.").ok();
        return Tap::Redraw(Dirty::All);
    }
    if app.has_details {
        app.close_details();
        return Tap::Redraw(Dirty::All);
    }
    if app.view == ViewMode::Browse {
        if app.current_path_str() == "/storage" {
            app.view = ViewMode::Home;
            app.has_details = false;
            app.selected_entry = None;
            app.multi_select = false;
            app.selection.clear();
            app.searching = false;
            app.query.clear();
            app.query.blur();
            app.set_status();
            std::log("[FILE_MANAGER] Returned to the home screen.").ok();
        } else {
            app.navigate_parent();
            std::log("[FILE_MANAGER] Navigated to parent directory.").ok();
        }
        return Tap::Redraw(Dirty::All);
    }
    Tap::Leave("[FILE_MANAGER] Back control pressed on Home. Exiting to launcher.")
}

fn handle_home_tap(app: &mut FileManagerApp, x: usize, y: usize) -> Tap {
    // A recent document opens its folder and its details card.
    if app.recents_error == 0 && app.recents_count > 0 {
        let visible = app.recents_count.min(HOME_RECENTS_VISIBLE);
        for index in 0..visible {
            let top = HOME_RECENTS_Y0 + index * HOME_RECENT_ROW_H;
            if y >= top && y < top + HOME_RECENT_ROW_H && (36..684).contains(&x) {
                app.open_recent(index);
                std::log("[FILE_MANAGER] Opened a recent document from the index.").ok();
                return Tap::Redraw(Dirty::All);
            }
        }
    }

    // The category rows.
    if (CATEGORY_Y0..CATEGORY_Y1).contains(&y) && (36..684).contains(&x) {
        let Some((index, _, _)) = category_row_at(y) else {
            return Tap::None;
        };
        app.view = ViewMode::Browse;
        app.set_path(CATEGORY_PATHS[index]);
        app.selected_shortcut = index;
        app.reset_browse_state();
        app.refresh_directory();
        std::log("[FILE_MANAGER] Category filter shortcut switched.").ok();
        return Tap::Redraw(Dirty::All);
    }

    // Browse everything.
    if HOME_BROWSE_BTN.contains(x, y) {
        app.view = ViewMode::Browse;
        app.set_path("/storage");
        app.reset_browse_state();
        app.refresh_directory();
        std::log("[FILE_MANAGER] Browsing the storage root.").ok();
        return Tap::Redraw(Dirty::All);
    }

    Tap::None
}

fn handle_browse_tap(app: &mut FileManagerApp, x: usize, y: usize, surface: u32) -> Tap {
    // The search field and its clear action.
    if (SEARCH_FIELD.y..SEARCH_FIELD.y + SEARCH_FIELD.h).contains(&y)
        && x >= SEARCH_FIELD.x
        && x < SEARCH_FIELD.x + SEARCH_FIELD.w
    {
        if !app.query.is_empty() && SEARCH_CLEAR.contains(x, y) {
            app.clear_search();
            std::log("[FILE_MANAGER] The search was cleared.").ok();
            return Tap::Redraw(Dirty::All);
        }
        app.query.focus();
        if !app.query.is_empty() {
            app.run_search();
        }
        return Tap::Redraw(Dirty::All);
    }

    // The two search scopes.
    if SCOPE_THIS_PILL.contains(x, y) || SCOPE_ALL_PILL.contains(x, y) {
        app.search_scope = app.search_scope.toggle();
        if app.searching && !app.query.is_empty() {
            app.run_search();
        } else {
            app.set_scope();
            app.set_status();
        }
        return Tap::Redraw(Dirty::All);
    }

    // The breadcrumbs. The band stops at the toolbar so the two never share
    // a row of pixels.
    if y >= BREADCRUMB_HIT_Y0 && y < BREADCRUMB_HIT_Y1.min(TOOLBAR_Y) {
        if let Some(index) = crumb_at(app, x) {
            let prefix_len = app.crumbs[index].prefix_len;
            let mut prefix = [0u8; 128];
            let take = prefix_len.min(128);
            prefix[..take].copy_from_slice(&app.current_path[..take]);
            if let Ok(path) = core::str::from_utf8(&prefix[..take]) {
                app.set_path(path);
                app.reset_browse_state();
                app.refresh_directory();
                std::log("[FILE_MANAGER] Breadcrumb navigated to a parent folder.").ok();
                return Tap::Redraw(Dirty::All);
            }
        }
    }

    // The sort and layout toolbar.
    if SORT_PILL.contains(x, y) {
        app.apply_sort();
        return Tap::Redraw(Dirty::All);
    }
    if LIST_PILL.contains(x, y) {
        if app.layout != LayoutMode::List {
            app.toggle_layout();
        }
        return Tap::Redraw(Dirty::All);
    }
    if GRID_PILL.contains(x, y) {
        if app.layout != LayoutMode::Grid {
            app.toggle_layout();
        }
        return Tap::Redraw(Dirty::All);
    }

    // The action bar.
    if (ACTION_BAR_Y0..ACTION_BAR_Y1).contains(&y) {
        for slot in 0..7usize {
            if action_slot_rect(slot).contains(x, y) {
                return handle_action_slot(app, slot);
            }
        }
        return Tap::None;
    }

    // The entries card: a row or a cell.
    let count = app.card_item_count();
    if count > 0 && ENTRIES_CARD.contains(x, y) {
        let hit = match app.layout {
            LayoutMode::List => entry_at_point(ENTRIES_CARD, app.scroll.offset, count, x, y),
            LayoutMode::Grid => grid_at_point(ENTRIES_CARD, app.scroll.offset, count, x, y),
        };
        if let Some(index) = hit {
            if app.multi_select {
                app.toggle_mark(index);
                return Tap::Redraw(Dirty::All);
            }
            if app.card_item_is_dir(index) {
                app.navigate_into(index);
                std::log("[FILE_MANAGER] Navigated into directory.").ok();
                return Tap::Redraw(Dirty::All);
            }
            app.show_details(index);
            std::log("[FILE_MANAGER] Details opened for the selected document.").ok();
            return Tap::Redraw(Dirty::All);
        }
    }

    let _ = surface;
    Tap::None
}

fn handle_action_slot(app: &mut FileManagerApp, slot: usize) -> Tap {
    match slot {
        0 => {
            // The select slot turns multi-select on and off.
            app.multi_select = !app.multi_select;
            if !app.multi_select {
                app.selection.clear();
            }
            app.set_status();
            std::log("[FILE_MANAGER] Multi-select mode toggled.").ok();
            Tap::Redraw(Dirty::All)
        }
        1 => {
            app.select_all_visible();
            std::log("[FILE_MANAGER] Every visible item was selected.").ok();
            Tap::Redraw(Dirty::All)
        }
        _ => {
            let Some(action) = action_for_slot(slot) else {
                return Tap::None;
            };
            if action == FileAction::NewFolder {
                // The platform exposes no directory-creation call, so the
                // control reports the refusal instead of pretending.
                app.set_notice(problem_text(OpProblem::UnsupportedByPlatform));
                std::log("[FILE_MANAGER] New folder refused: the platform has no directory-creation call.").ok();
                return Tap::Redraw(Dirty::All);
            }
            let mut rows = [0usize; MAX_PARSED_ENTRIES];
            let count = app.action_rows(&mut rows);
            if count == 0 {
                app.set_notice(problem_text(OpProblem::NoSelection));
                return Tap::Redraw(Dirty::All);
            }
            let mut any_dir = false;
            for &row in &rows[..count] {
                if app.card_item_is_dir(row) {
                    any_dir = true;
                    break;
                }
            }
            if any_dir {
                app.set_notice(problem_text(OpProblem::FolderNotSupported));
                return Tap::Redraw(Dirty::All);
            }
            if action == FileAction::Rename && count != 1 {
                app.set_notice(problem_text(OpProblem::RenameNeedsOne));
                return Tap::Redraw(Dirty::All);
            }
            if set_as_action(app, action) {
                return Tap::Redraw(Dirty::All);
            }
            Tap::Redraw(Dirty::All)
        }
    }
}

fn handle_sheet_tap(
    app: &mut FileManagerApp,
    x: usize,
    y: usize,
    surface: u32,
) -> Tap {
    // The destination, for a copy or a move.
    if app.pending.action == FileAction::Copy || app.pending.action == FileAction::Move {
        for index in 0..CATEGORY_COUNT {
            if sheet_destination_rect(index).contains(x, y) {
                app.destination = index;
                return Tap::Redraw(Dirty::All);
            }
        }
    }
    // The name field, for a rename.
    if app.pending.action == FileAction::Rename {
        let field = Rect::new(SHEET.x + 20, SHEET_NAME_Y, SHEET.w - 40, SHEET_NAME_H);
        if field.contains(x, y) {
            app.rename_field.focus();
            return Tap::Redraw(Dirty::All);
        }
    }
    // Cancel.
    if sheet_button_rect(false).contains(x, y) {
        app.close_confirmation();
        std::log("[FILE_MANAGER] The action was cancelled.").ok();
        return Tap::Redraw(Dirty::All);
    }
    // Confirm.
    if sheet_button_rect(true).contains(x, y) {
        let action = app.pending.action;
        let count = app.pending.item_count;
        let mut rows = [0usize; MAX_PARSED_ENTRIES];
        let row_count = app.action_rows(&mut rows);
        let mut any_dir = false;
        for &row in &rows[..row_count] {
            if app.card_item_is_dir(row) {
                any_dir = true;
                break;
            }
        }
        let current_name = if action == FileAction::Rename && row_count == 1 {
            let name = app.card_item_name(rows[0]);
            let mut buf = [0u8; MAX_ENTRY_NAME_LEN];
            let take = name.len().min(MAX_ENTRY_NAME_LEN);
            buf[..take].copy_from_slice(&name[..take]);
            (buf, take)
        } else {
            ([0u8; MAX_ENTRY_NAME_LEN], 0)
        };
        if let Err(problem) = validate_operation(
            action,
            count,
            any_dir,
            app.rename_field.as_slice(),
            &current_name.0[..current_name.1],
            false,
        ) {
            // The sheet stays open so the user can correct the name or
            // cancel; the reason is written where the status line goes.
            app.set_notice(problem_text(problem));
            return Tap::Redraw(Dirty::All);
        }
        perform_action(app, surface);
        return Tap::Redraw(Dirty::All);
    }
    Tap::None
}

fn handle_details_tap(app: &mut FileManagerApp, x: usize, y: usize) -> Tap {
    // WHY the Open action waits for the registry: the card offers it only
    // when the device named a handler, and the launch asks the platform
    // again before it hands anything over.
    let open_offered = app.details.is_dir || app.details.has_handler;
    if open_offered && DETAILS_OPEN.contains(x, y) {
        if app.details.is_dir {
            let row = app.selected_entry;
            app.close_details();
            if let Some(row) = row {
                app.navigate_into(row);
                std::log("[FILE_MANAGER] Navigated into directory.").ok();
            }
        } else if open_with_handler(app) {
            app.set_notice(b"Opened with the platform's handler");
        } else if app.details.has_handler {
            app.set_notice(HANDLER_REFUSED_NOTICE);
        } else {
            app.set_notice(NO_HANDLER_NOTICE);
        }
        return Tap::Redraw(Dirty::All);
    }
    if DETAILS_CLOSE.contains(x, y) || !DETAILS_CARD.contains(x, y) {
        app.close_details();
        return Tap::Redraw(Dirty::All);
    }
    Tap::None
}

/// Puts a key into the field that has the keyboard's attention.
fn handle_key(app: &mut FileManagerApp, code: u32) -> bool {
    if app.has_pending && app.rename_field.is_focused() {
        if code < 0x80 {
            return app.rename_field.push(code as u8);
        }
        if code == 0x10002 {
            app.rename_field.pop();
            return true;
        }
        if code == 0x10001 {
            app.rename_field.clear();
            return true;
        }
        return false;
    }
    if !app.has_pending && !app.has_details && app.query.is_focused() {
        if code < 0x80 {
            if !app.query.push(code as u8) {
                return false;
            }
            app.run_search();
            return true;
        }
        if code == 0x10002 {
            app.query.pop();
            app.run_search();
            return true;
        }
        if code == 0x10001 {
            app.clear_search();
            return true;
        }
    }
    false
}

/// The row or cell under a long press.
fn long_press_hit(app: &FileManagerApp, x: usize, y: usize) -> Option<usize> {
    if app.view != ViewMode::Browse || app.has_pending || app.has_details {
        return None;
    }
    let count = app.card_item_count();
    if count == 0 || !ENTRIES_CARD.contains(x, y) {
        return None;
    }
    match app.layout {
        LayoutMode::List => entry_at_point(ENTRIES_CARD, app.scroll.offset, count, x, y),
        LayoutMode::Grid => grid_at_point(ENTRIES_CARD, app.scroll.offset, count, x, y),
    }
}

// -----------------------------------------------------------------------------
// Entry point and event reactor loop
// -----------------------------------------------------------------------------

#[no_mangle]
pub extern "C" fn _start() -> ! {
    std::log("[FILE_MANAGER (Ring 3)] Sovereign File Manager & VFS Explorer online.").ok();

    let mut app = FileManagerApp::new();

    // Query physical block storage capacity from the driver.
    if let Ok(bytes) = std::storage_query() {
        app.storage_capacity_bytes = bytes;
        std::log("[FILE_MANAGER] Storage capacity validated from hardware service.").ok();
    }
    app.refresh_storage_line();

    // The home screen's recents and the first listing. WHY both at startup:
    // the home screen is what the user sees first, and the browse screen is
    // one press away.
    app.refresh_directory();
    app.load_recents();

    let surface = match std::surface_create(W as u32, H as u32, 200) {
        Ok(id) => {
            std::log("[FILE_MANAGER] DRM/KMS Client Surface allocated (z=200).").ok();
            id
        }
        Err(_) => 0,
    };

    if surface > 0 {
        render_file_manager_ui(surface, &app);
        let _ = std::surface_flip();
        std::log("[FILE_MANAGER] Files UI presented to scanout buffer.").ok();
    }

    loop {
        let interest = std::EVENT_TOUCH_INPUT | std::EVENT_TIMER_TICK;
        if let Ok(triggered) = std::event_poll(interest, 100) {
            if (triggered & std::EVENT_TOUCH_INPUT) != 0 {
                let mut input = std::InputEvent {
                    kind: 0,
                    x: 0,
                    y: 0,
                    dx: 0,
                    dy: 0,
                    scale_milli: 1000,
                };
                let mut dirty = Dirty::None;
                while std::input_event_poll(&mut input).is_ok() {
                    // WHY the home gesture is the only one that leaves from
                    // outside a control: a pan anywhere on the screen used to
                    // throw the user out of the file list.
                    let is_home_gesture = input.y >= 1400 && input.x >= 240 && input.x <= 480;
                    if is_home_gesture
                        && (input.kind == 4
                            || input.kind == 6
                            || input.kind == 7
                            || input.kind == 8
                            || input.kind == 10)
                    {
                        std::log("[FILE_MANAGER] Home gesture recognized. Exiting to launcher.").ok();
                        exit_to_launcher(surface);
                    }

                    // A pan or a fling over the card scrolls it.
                    if (input.kind == 4 || input.kind == 10)
                        && app.view == ViewMode::Browse
                        && !app.has_pending
                        && !app.has_details
                        && ENTRIES_CARD.contains(input.x as usize, input.y as usize)
                    {
                        if app.scroll_by(input.dy) {
                            dirty = Dirty::All;
                        }
                    }

                    // A key goes to whichever field has the attention.
                    if input.kind == 11 {
                        if handle_key(&mut app, input.x) {
                            dirty = Dirty::All;
                        }
                    }

                    // A long press starts multi-select on the row it lands on.
                    if input.kind == 3 {
                        if let Some(index) =
                            long_press_hit(&app, input.x as usize, input.y as usize)
                        {
                            app.multi_select = true;
                            app.selection.set(index, true);
                            app.set_status();
                            app.suppress_tap = true;
                            dirty = Dirty::All;
                            std::log("[FILE_MANAGER] Long press started multi-select.").ok();
                        }
                    }

                    // Back closes what is open, then goes up, then home.
                    if input.kind == 6 {
                        match handle_back(&mut app) {
                            Tap::Leave(message) => {
                                std::log(message).ok();
                                exit_to_launcher(surface);
                            }
                            Tap::Redraw(d) => dirty = widen(dirty, d),
                            Tap::None => {}
                        }
                    }

                    if input.kind == 1 {
                        if app.suppress_tap {
                            // The long press that just started the selection
                            // already consumed this gesture.
                            app.suppress_tap = false;
                        } else {
                            match handle_tap(
                                &mut app,
                                input.x as usize,
                                input.y as usize,
                                surface,
                            ) {
                                Tap::Leave(message) => {
                                    std::log(message).ok();
                                    exit_to_launcher(surface);
                                }
                                Tap::Redraw(d) => dirty = widen(dirty, d),
                                Tap::None => {}
                            }
                        }
                    }
                }
                if dirty != Dirty::None && surface > 0 {
                    render_file_manager_ui(surface, &app);
                    let _ = std::surface_flip();
                }
            }
        }
        std::yield_now();
    }
}

fn widen(a: Dirty, b: Dirty) -> Dirty {
    match (a, b) {
        (Dirty::None, d) | (d, Dirty::None) => d,
        _ => Dirty::All,
    }
}

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    std::log("[FILE_MANAGER] Panic detected! Terminating task...").ok();
    std::exit();
}
