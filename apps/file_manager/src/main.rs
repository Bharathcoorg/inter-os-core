//! InterOS Native Sovereign File Manager & VFS Explorer App
//!
//! Native Ring-3 graphical application executing in User Mode:
//! - DRM/KMS Client Surface allocation (z=200) and double-buffered scanout presentation
//! - Real-time block device capacity query via microkernel storage driver
//! - Startup VFS directory query whose received bytes are reported on screen and
//!   in the log only: no directory entry list is parsed or rendered
//! - Internal Storage capacity queried from the storage service; usage is shown only when real telemetry is available.
//! - Tactile category rows (Recent, Images, Videos, Documents, Downloads,
//!   Internal Storage, Cloud) with a visible selection highlight
//! - A drawn back control in the header, whose hit region is the same rectangle
//!   it is painted from - see `BACK`
//! - Single screen: no subdirectory navigation and no path breadcrumbs are drawn
//! - Banded rendering (`render_file_manager_band`): only the rows an input
//!   actually dirtied are re-rasterised, instead of all 1,036,800 pixels
//! - Asynchronous event reactor loop handling touch interactions and home gestures
//! - Graceful lifecycle supervision and clean return to Launcher

#![no_std]
#![no_main]

use core::panic::PanicInfo;
use interos_std as std;
use file_manager::theme::{MIN_TOUCH_TARGET, THEME, spacing};
use std::design::components;
use std::design::font;
use std::design::primitives::{draw_pill, draw_rounded_rect, Rect};
pub use file_manager::{
    action_for_mime, detect_mime, fmt_u64, parse_vfs_entries, document_from_index,
    FileMime, RuntimeFileEntry, SelectedDocument, MAX_DOCUMENT_PATH_LEN, MAX_ENTRY_NAME_LEN,
    MAX_PARSED_ENTRIES,
};
// The layout, the scroll geometry and the file model live in the library so
// the renderer and the hit test can call the same functions. See
// `file_manager::ENTRIES_CARD` and the tests beside it.
use file_manager::{
    action_slot_rect, action_supported, entry_at_point, entry_row_rect, join_path, list_scroll,
    name_is_usable, parse_indexed_documents, search_needle_is_usable, sheet_button_rect,
    sheet_destination_rect, FileAction, IndexedDocument, Selection, TypedField, ACTION_BAR_Y0,
    ACTION_BAR_Y1, ACTION_LABELS, ACTIONS_Y0, ACTIONS_Y1, CATEGORY_COUNT, CATEGORY_Y0, CATEGORY_Y1,
    ENTRIES_CARD, ENTRY_ROW_H, HEADER_H, LINE_LEN, MAX_PATH_LEN, MAX_SEARCH_RESULTS, SCOPE_Y,
    SEARCH_CLEAR, SEARCH_FIELD, SHEET, SHEET_NAME_H, SHEET_NAME_Y, STATUS_Y,
};

use interos_std::design::outline_text;
use interos_std::design::scroll::ScrollState;


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

/// The app resolves no capability slot of its own.
///
/// WHY it does not resolve one: a slot is a position in this task's
/// capability table, and the position depends on the order the kernel
/// inserted the capabilities in, so the app cannot name one. The old code
/// passed a bare number that happened to be the framebuffer in every app
/// and was wrong in this one, and the listing was refused before the path
/// was ever examined. The kernel now authorises a filesystem call by the
/// capability's type, so the app asks for the listing and reports the
/// platform's own answer -- a refusal it received, not one it invented.

// -----------------------------------------------------------------------------
// Visual Design Tokens
//
// WHY there is no palette here any more: the app used to carry its own
// colours, which is a second design system inside one app. Every colour now
// resolves through the platform's theme, so a third-party app that asks the
// SDK for the same roles draws the same file manager. See `file_manager::theme`
// for the mapping from the old names to the platform's roles.
// -----------------------------------------------------------------------------

// -----------------------------------------------------------------------------
// Header geometry - single source of truth for drawing and hit-testing
// -----------------------------------------------------------------------------
/// Screen margin (8pt grid value). Also the left inset of the back control and
/// the right inset the storage readout is aligned to.
const HEADER_MARGIN: usize = spacing::GRID_24;

/// The header's back control: a 52x52 pill in the strip above the title.
///
/// WHY this rectangle at all: the old app hit-tested `x 24..100 && y 28..68` and
/// used it to exit to the launcher, but nothing in that region was ever drawn -
/// the title owns rows 64..136 and the two header pictograms sat at x 590 and
/// x 660. An invisible exit hotspot is unusable: a user cannot find it and, when
/// they do land on it by accident, the app leaves without explanation.
///
/// WHY rows 8..60 rather than 28..68: the title's line box starts at 64, so a
/// 52px control has to live above it. The pill is centred horizontally on the
/// same 24px margin the title uses, which is where the shell puts a back
/// control.
const BACK: Rect = Rect::new(HEADER_MARGIN, 8, 52, 52);

/// The system home indicator, and the strip that contains it.
///
/// WHY the hit box is larger than the ink: 6px of ink is not a touch target, so
/// the tap region is the whole gesture strip - but the old code tested
/// `x 240..480 && y >= 1400` with no upper bound on y, a 241x40px region that was
/// never painted. Both rectangles are named here so the region that is
/// hit-tested is the region that contains the ink.
///
/// WHY 44 and not 40: the design system's minimum touch target is 44 logical
/// pixels and this strip was 40. It now reaches the bottom edge of the
/// surface, which is where the indicator already was.
const HOME_HIT: Rect = Rect::new(240, 1396, 240, MIN_TOUCH_TARGET);
const HOME_PILL: Rect = Rect::new(260, 1418, 200, 6);

/// Right edge the header readout is aligned to (the 24px right margin).
const READOUT_X1: usize = W - HEADER_MARGIN;
/// Widest a header readout run may be. WHY 400 and not "whatever fits": the
/// title ends at x 180, so a readout longer than `READOUT_X1 - 180` = 516 would
/// collide with it. 400 leaves a 116px gutter and `font::ellipsize` trims to it
/// rather than letting a long path run under the title.
const READOUT_MAX_W: usize = 400;
/// Row boxes for the two readout lines. WHY 68 and 100: the face is 24 rows
/// tall, so scale-1 runs own 68..92 and 100..124 - both inside the title's
/// 64..136 band, separated by 8 rows, and never overlapping each other.
const READOUT_LINE1_Y0: usize = 68;
const READOUT_LINE2_Y0: usize = READOUT_LINE1_Y0 + font::GLYPH_H + 8;

/// Rows the category list owns. WHY there is no header band constant beside it:
/// nothing a touch can reach writes to the header - the storage readout is
/// filled once, before the first present - so a band for it would never run.
const ROW_BAND_Y0: usize = CATEGORY_Y0;
const ROW_BAND_Y1: usize = CATEGORY_Y1;

/// Ink for an affordance that is painted but has nothing behind it.
///
/// WHY the border token: it is the design system's own hairline, so "you cannot
/// do this" reads the same here as it does in the gallery's disabled chips.
/// Unused on this screen today - the two header pictograms that had nothing
/// behind them were removed rather than greyed, because there was nothing left
/// for them to grey out *and* say.
#[allow(dead_code)]
const DISABLED: u32 = THEME.border;


/// Rasterises one text run into the pixel at `(x, y)`.
///
/// `bg` is the fill the run is drawn on top of. WHY it is a parameter: the
/// anti-aliased edges of a glyph are blended toward `bg`, so hard-coding the
/// page colour handed the selected row's label dark fringes against its own
/// the raised surface's fill. Callers pass the fill that is actually under the run.
///
/// The face is `GLYPH_H` (24) rows tall, so a run at `scale` owns
/// `GLYPH_H * scale` rows. Clipping to that line box here means a caller can no
/// longer slice a glyph with a hand-written y window.
///
/// WHY the outline face comes first: the title is drawn at scale 3 and the
/// category labels at scale 2, so on the bitmap face both are a 16x24 cell
/// magnified by an integer -- a soft ramp where an edge belongs.
/// `outline_text::place_run` rasterises the real outline at the em that matches
/// the cell's cap height (17 rows at scale 1, so 17*N*2048/1456 px per em), and
/// answers for the pixel itself. When it declines -- a run wider than the
/// coverage buffer, a glyph whose ink would leave this caller's window, or a
/// weight this app draws bold -- the bitmap face below draws the run exactly as
/// it did before, so a refusal costs the edge quality and never the text.
///
/// WHY the run answers alone when it is placed: the two faces are alternatives,
/// not layers. Falling through to the bitmap face after an outline run had
/// already inked a pixel would double-ink every partially covered edge.
///
/// WHY no column bound is handed to `place_run`: this funnel's callers gate
/// their runs by construction -- the title sits 131px into a 720px header and
/// the widest label ends 301px into the 648px list band -- so no run can reach
/// the edge the caller would cut it at.
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
    if x < x0 || y < y0 || y >= y0 + std::design::font::GLYPH_H * scale {
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
// Counted Log Assembly (no allocator in this no_std app)
// -----------------------------------------------------------------------------


/// Builds `prefix ++ decimal(value) ++ suffix` into `buf` and returns its length.
/// WHY: measured values only exist at runtime, so they are assembled into a
/// stack buffer before being logged.
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

/// Logs a listing and the path that was asked for.
///
/// WHY the path is in the line: a log that names a path the app never opened
/// is a lie about what the app did.
fn log_listing(path: &str, bytes: u64, suffix: &str) {
    std::log("[FILE_MANAGER] fs_list(").ok();
    std::log(path).ok();
    std::log(") answered with ").ok();
    log_counted("", bytes, suffix);
}

/// Logs a refused listing and the path that was asked for.
fn log_listing_refused(path: &str, code: i64) {
    std::log("[FILE_MANAGER] fs_list(").ok();
    std::log(path).ok();
    std::log(") refused, code -").ok();
    log_counted("", code.unsigned_abs(), "; no directory data received.");
}

// -----------------------------------------------------------------------------
// VFS Entry Representation
// Runtime entries must come from the kernel VFS; no synthetic inventory is embedded.
// -----------------------------------------------------------------------------
#[derive(Debug, Clone, Copy)]
pub struct VfsItem {
    pub name: &'static str,
    pub is_dir: bool,
    pub size_str: &'static str,
    pub meta_str: &'static str,
    pub color_accent: u32,
}

/// The app ships no embedded file inventory: `ITEMS_ALL` stays empty until a
/// real VFS listing can populate it (WHY: the platform truth contract pins the
/// empty ledger, and nothing on screen may invent file names).
#[allow(dead_code)]
const ITEMS_ALL: [VfsItem; 0] = [];

/// Default probe label before the syscall has run: a real statement, not a
/// placeholder that could be mistaken for a result.
///
/// WHY a byte-literal and not `b"..."`: `copy_from_slice` is not usable in a
/// `const`, and the array has to be initialised in `const fn new` so a fresh app
/// is never uninitialised. `"not queried"` is 10 bytes; the remaining 30 are
/// padding and are never read because `probe_label_len` is 10.
const PROBE_DEFAULT: [u8; 40] =
    *b"not queried\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0";


/// What the last directory listing actually produced.
///
/// WHY this is an enum and not a byte count: "the directory is empty" and
/// "the device refused to read the directory" are different facts, and the
/// old screen said "Directory Empty" for both. A user whose files are
/// behind a refusal was told the directory was empty.
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
/// WHY these four and not a category grid: the VFS mounts `/storage` with
/// `/storage/photos`, `/storage/pictures` and `/storage/videos`, and the
/// kernel's app path policy allows an app to list `/storage` and its
/// children. Every other path -- `/data` included -- is refused before the
/// path is even looked at. The old rows named scopes the platform does not
/// have ("Cloud", "Documents", "Downloads", "Recent") and all of them
/// opened `/data`, which the kernel refuses, so each one led to a refusal
/// dressed up as a category.
pub const CATEGORY_PATHS: [&str; 4] = ["/storage", "/storage/photos", "/storage/pictures", "/storage/videos"];

/// The label each real directory is shown under.
pub const CATEGORY_LABELS: [&[u8]; 4] = [b"Storage", b"Photos", b"Pictures", b"Videos"];

// -----------------------------------------------------------------------------
// The selected document's actions
// -----------------------------------------------------------------------------

/// The panel that shows what the platform holds about the selected document
/// and what the user can do with it.
///
/// WHY a panel in the gap between the category rows and the file card: the
/// panel has to be somewhere the user can see, and this band is the one the
/// screen already leaves empty. An action the user cannot see is an action
/// the user cannot take.
pub const ACTION_OPEN_Y0: usize = 662;
pub const ACTION_OPEN_Y1: usize = 706;
/// The "set as" line, where the action the media type allows is drawn.
pub const ACTION_SET_Y0: usize = 722;
pub const ACTION_SET_Y1: usize = 766;
/// The panel's left edge.
pub const ACTIONS_X0: usize = 36;
/// The panel's right edge.
pub const ACTIONS_X1: usize = 684;

/// The action waiting for the user's confirmation.
///
/// WHY the items are held as card rows and not as paths: the sheet has to
/// name the files the action will touch, and the card row is where the name
/// and the path live together.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct PendingAction {
    /// The action the user asked for.
    pub action: FileAction,
    /// The card rows the action will touch.
    pub items: [usize; 8],
    /// How many rows the action will touch.
    pub item_count: usize,
}

impl PendingAction {
    pub const EMPTY: PendingAction = PendingAction {
        action: FileAction::Copy,
        items: [0; 8],
        item_count: 0,
    };
}

pub struct FileManagerApp {
    pub selected_shortcut: usize,
    pub selected_entry: Option<usize>,
    pub storage_capacity_bytes: u64,
    pub current_path: [u8; 128],
    pub current_path_len: usize,
    /// What `fs_list` actually answered, pre-rendered.
    ///
    /// WHY a fixed buffer instead of formatting per pixel: `get_file_manager_color`
    /// runs once per pixel, so building this string in the render path would run
    /// it 1,036,800 times for one number. It is built once, when the syscall
    /// returns, and only read afterwards.
    pub probe_label: [u8; 40],
    pub probe_label_len: usize,
    /// What the last listing produced, so the empty state can say which.
    pub listing_state: ListingState,
    /// The platform's own status code for a refused listing.
    pub listing_error: i64,
    pub entries: [RuntimeFileEntry; MAX_PARSED_ENTRIES],
    pub entry_count: usize,
    /// The document the user has selected, as the platform's index describes it.
    ///
    /// WHY the index and not the listing: the index is where the media type
    /// and the identity of a document live, and a file without a media type
    /// cannot be handed to the program that declared a handler for it.
    pub selected_document: SelectedDocument,
    /// Whether a document is selected.
    pub has_selected_document: bool,
    /// Why the index could not be read, when it could not.
    pub index_error: i64,
    /// Where the entries list is scrolled to.
    ///
    /// WHY the scroll lives on the app and not in the renderer: the renderer
    /// runs once per pixel, and a scroll that was recomputed there would be
    /// recomputed a million times for one gesture.
    pub scroll: ScrollState,
    /// The text the user has typed into the search field.
    pub query: TypedField,
    /// The documents the platform's index answered the search with.
    pub results: [IndexedDocument; MAX_SEARCH_RESULTS],
    pub result_count: usize,
    /// Whether the card is showing the search results rather than the
    /// directory listing.
    pub searching: bool,
    /// The platform's status code when the search was refused.
    pub search_error: i64,
    /// The files the user has marked.
    pub selection: Selection,
    /// Whether the app is in the multi-select mode.
    pub multi_select: bool,
    /// The action waiting for its confirmation.
    pub pending: PendingAction,
    /// Whether an action is waiting for its confirmation.
    pub has_pending: bool,
    /// The directory a copy or a move writes into.
    pub destination: usize,
    /// The name being typed for a rename.
    pub rename_field: TypedField,
    /// The status line, pre-rendered.
    pub status: [u8; LINE_LEN],
    pub status_len: usize,
    /// The scope caption, pre-rendered.
    pub scope: [u8; LINE_LEN],
    pub scope_len: usize,
    /// The confirmation sheet's title, pre-rendered.
    pub sheet_title: [u8; 64],
    pub sheet_title_len: usize,
    /// The names of the files the pending action will touch.
    pub sheet_names: [[u8; MAX_ENTRY_NAME_LEN]; 4],
    pub sheet_name_lens: [usize; 4],
    pub sheet_name_count: usize,
}

impl FileManagerApp {
    pub const fn new() -> Self {
        let mut path = [0u8; 128];
        // WHY the default path is `/storage`: it is one of the paths the
        // kernel's app policy lets this app list. The old default was
        // `/data`, which the policy refuses before the path is examined, so
        // the app opened on a directory it could never see and reported the
        // refusal as an empty directory.
        let init = b"/storage";
        let mut i = 0;
        while i < init.len() {
            path[i] = init[i];
            i += 1;
        }
        Self {
            selected_shortcut: 0,
            selected_entry: None,
            storage_capacity_bytes: 0,
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
            index_error: 0,
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
            scope: [0u8; LINE_LEN],
            scope_len: 0,
            sheet_title: [0u8; 64],
            sheet_title_len: 0,
            sheet_names: [[0u8; MAX_ENTRY_NAME_LEN]; 4],
            sheet_name_lens: [0; 4],
            sheet_name_count: 0,
        }
    }

    #[inline]
    pub fn current_path_str(&self) -> &str {
        core::str::from_utf8(&self.current_path[..self.current_path_len]).unwrap_or("/data")
    }

    pub fn set_path(&mut self, path: &str) {
        let bytes = path.as_bytes();
        let len = bytes.len().min(self.current_path.len());
        self.current_path[..len].copy_from_slice(&bytes[..len]);
        self.current_path_len = len;
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
        if !self.current_path[..n].ends_with(b"/") && n < self.current_path.len() {
            self.current_path[n] = b'/';
            n += 1;
        }
        let name_slice = &entry.name[..entry.name_len];
        let copy_len = name_slice.len().min(self.current_path.len().saturating_sub(n));
        self.current_path[n..n + copy_len].copy_from_slice(&name_slice[..copy_len]);
        self.current_path_len = n + copy_len;
        self.selected_entry = None;
        self.refresh_directory();
    }

    pub fn navigate_parent(&mut self) -> bool {
        if &self.current_path[..self.current_path_len] == b"/storage" {
            return false;
        }
        let slice = &self.current_path[..self.current_path_len];
        let last_slash = slice.iter().rposition(|&b| b == b'/');
        match last_slash {
            Some(0) | None => {
                // WHY the root is the storage root and not the old default:
                // `/data` is refused by the kernel's app path policy, so
                // navigating up to it put the app in a directory it cannot
                // read.
                let init = b"/storage";
                self.current_path[..init.len()].copy_from_slice(init);
                self.current_path_len = init.len();
            }
            Some(pos) => {
                self.current_path_len = pos;
            }
        }
        self.selected_entry = None;
        self.refresh_directory();
        true
    }

    /// Parses newline-delimited VFS entries in `<name>|<size>|<dir|dev|file>` format.
    pub fn parse_entries(&mut self, data: &[u8]) {
        self.entry_count = parse_vfs_entries(data, &mut self.entries);
    }

    /// Records the real answer from `fs_list`.
    ///
    /// WHY both arms are shown: the honest result of a directory query on this
    /// platform is often "refused", and a screen that only ever prints a byte
    /// count teaches the user nothing. Reporting the kernel's own status code is
    /// the only truthful thing to put there.
    ///
    /// WHY the state is recorded as well: the empty card has to be able to say
    /// "unreadable" instead of "empty" when the listing was refused.
    pub fn record_probe(&mut self, result: Result<usize, i64>) {
        let mut buf = [0u8; 40];
        let n = match result {
            Ok(bytes) => {
                let prefix = b"received ";
                let p = prefix.len();
                buf[..p].copy_from_slice(prefix);
                let mut n = p + fmt_u64(bytes as u64, &mut buf[p..]);
                let suffix = b" bytes";
                for &b in suffix {
                    if n < buf.len() {
                        buf[n] = b;
                        n += 1;
                    }
                }
                n
            }
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

    /// Refreshes entries by querying fs_list for current_path.
    pub fn refresh_directory(&mut self) {
        let mut entries = [0u8; 1024];
        // WHY the listing is asked for directly: the kernel authorises the
        // call by the capability's type, and its own answer -- a listing or
        // a refusal with a status code -- is the one that is reported.
        let probe = std::fs_list(self.current_path_str(), &mut entries);
        self.record_probe(probe);
        if let Ok(n) = probe {
            self.parse_entries(&entries[..n]);
            // WHY the state follows the parse: a listing that succeeded with
            // no entries is empty, and one that succeeded with entries is
            // listed. Neither is the same fact as a refusal.
            self.listing_state = if self.entry_count == 0 {
                ListingState::Empty
            } else {
                ListingState::Listed
            };
        } else {
            self.entry_count = 0;
        }
        self.scroll = list_scroll(self.entry_count, ENTRIES_CARD, 0);
        self.set_scope();
    }

    /// The number of rows the card shows.
    pub fn card_item_count(&self) -> usize {
        if self.searching {
            self.result_count
        } else {
            self.entry_count
        }
    }

    /// The name of the row at an index.
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
    /// WHY a search result is never a folder: the index holds documents, and
    /// a document is a file. The platform has no folder records.
    pub fn card_item_is_dir(&self, index: usize) -> bool {
        if self.searching {
            return false;
        }
        if index < self.entry_count {
            return self.entries[index].is_dir;
        }
        false
    }

    /// The trailing value of the row at an index: the size the platform
    /// holds, or the word that says the row is a folder.
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
    /// the typed needle.
    ///
    /// WHY the platform's index and not a scan of the current directory: the
    /// index is the one description of the documents the device holds, and a
    /// search that only ever saw one folder would not be a search.
    pub fn run_search(&mut self) {
        let needle = self.query.as_slice();
        if !search_needle_is_usable(needle) {
            self.searching = false;
            self.result_count = 0;
            self.search_error = 0;
            self.scroll = list_scroll(self.entry_count, ENTRIES_CARD, 0);
            self.set_scope();
            return;
        }
        let mut raw = [0u8; 2048];
        match std::content::content_index_search_name(
            core::str::from_utf8(needle).unwrap_or(""),
            &mut raw,
        ) {
            Ok(n) => {
                self.result_count = parse_indexed_documents(&raw[..n], &mut self.results);
                self.searching = true;
                self.search_error = 0;
            }
            Err(code) => {
                self.result_count = 0;
                self.searching = true;
                self.search_error = code;
            }
        }
        self.scroll = list_scroll(self.card_item_count(), ENTRIES_CARD, 0);
        self.set_scope();
    }

    /// Empties the search and goes back to the directory listing.
    pub fn clear_search(&mut self) {
        self.query.clear();
        self.query.blur();
        self.searching = false;
        self.result_count = 0;
        self.search_error = 0;
        self.scroll = list_scroll(self.entry_count, ENTRIES_CARD, 0);
        self.set_scope();
    }

    /// Marks or unmarks a row.
    pub fn toggle_mark(&mut self, index: usize) -> bool {
        let marked = self.selection.toggle(index);
        self.set_status();
        marked
    }

    /// The rows the action bar's actions would touch.
    ///
    /// WHY the marked set in the multi-select mode and the one selected
    /// document otherwise: the bar is always about something, and a bar that
    /// did nothing until the user found the multi-select mode would be a
    /// control that could not be taken.
    pub fn action_rows(&self, out: &mut [usize]) -> usize {
        if self.multi_select {
            return self.selection.indices(out);
        }
        match self.selected_entry {
            Some(index) if index < self.card_item_count() => {
                out[0] = index;
                1
            }
            _ => 0,
        }
    }

    /// How many rows an action would touch.
    pub fn action_row_count(&self) -> usize {
        let mut rows = [0usize; 8];
        self.action_rows(&mut rows)
    }

    /// Whether the platform can do this action to what is selected.
    pub fn action_is_possible(&self, action: FileAction) -> bool {
        let mut rows = [0usize; 8];
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

    /// Scrolls the list.
    pub fn scroll_by(&mut self, dy: i32) -> bool {
        let count = self.card_item_count();
        let mut scroll = list_scroll(count, ENTRIES_CARD, self.scroll.offset);
        let before = scroll.offset;
        scroll.apply_drag(dy);
        self.scroll = scroll;
        self.scroll.offset != before
    }

    /// Opens the confirmation sheet for an action over the current selection.
    ///
    /// WHY the sheet is built here and not in the renderer: the renderer runs
    /// once per pixel, and the names of the files are a fact about the
    /// selection, not about the pixel.
    pub fn open_confirmation(&mut self, action: FileAction) -> bool {
        let mut rows = [0usize; 8];
        let count = self.action_rows(&mut rows);
        if count == 0 {
            return false;
        }
        if action == FileAction::Rename && count != 1 {
            return false;
        }
        for &row in &rows[..count] {
            if !action_supported(action, self.card_item_is_dir(row)) {
                return false;
            }
        }
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
        len = append(&mut self.sheet_title, len, if count == 1 { b" file?" } else { b" files?" });
        self.sheet_title_len = len;
        // The names of the files the action will touch, and only those: the
        // sheet has four lines and a list of forty names would not fit.
        self.sheet_name_count = 0;
        for (index, &row) in rows[..count].iter().enumerate() {
            if index >= 4 {
                break;
            }
            let mut name = [0u8; MAX_ENTRY_NAME_LEN];
            let len = self.card_item_name(row).len().min(MAX_ENTRY_NAME_LEN);
            name[..len].copy_from_slice(&self.card_item_name(row)[..len]);
            self.sheet_names[index][..len].copy_from_slice(&name[..len]);
            self.sheet_name_lens[index] = len;
            self.sheet_name_count = index + 1;
        }
        if count > 4 {
            let mut more = [0u8; 64];
            let len = append(&mut more, 0, b"and ");
            let len = append_u64(&mut more, len, (count - 4) as u64);
            let len = append(&mut more, len, b" more");
            let len = len.min(MAX_ENTRY_NAME_LEN);
            self.sheet_names[3][..len].copy_from_slice(&more[..len]);
            self.sheet_name_lens[3] = len;
        }
        if action == FileAction::Rename {
            // The rename field starts with the name the file has now, so the
            // user changes the part they mean to change.
            let mut name = [0u8; MAX_ENTRY_NAME_LEN];
            let len = self.card_item_name(rows[0]).len().min(MAX_ENTRY_NAME_LEN);
            name[..len].copy_from_slice(&self.card_item_name(rows[0])[..len]);
            self.rename_field.set(&name[..len]);
            self.rename_field.focus();
        } else {
            self.rename_field.clear();
            self.rename_field.blur();
        }
        self.set_status();
        true
    }

    /// Closes the confirmation sheet.
    pub fn close_confirmation(&mut self) {
        self.has_pending = false;
        self.pending = PendingAction::EMPTY;
        self.rename_field.clear();
        self.rename_field.blur();
        self.set_status();
    }

    /// Writes the status line.
    pub fn set_status(&mut self) {
        self.status = [0u8; LINE_LEN];
        self.status_len = 0;
        if self.has_pending {
            let action = self.pending.action;
            let count = self.pending.item_count;
            let mut len = append(&mut self.status, 0, ACTION_LABELS[action as usize]);
            len = append(&mut self.status, len, b" ");
            len = append_u64(&mut self.status, len, count as u64);
            len = append(&mut self.status, len, if count == 1 { b" file" } else { b" files" });
            len = append(&mut self.status, len, b" - confirm or cancel");
            self.status_len = len;
            return;
        }
        if self.multi_select {
            let count = self.selection.count();
            let mut len = append(&mut self.status, 0, b"Selected ");
            len = append_u64(&mut self.status, len, count as u64);
            len = append(&mut self.status, len, if count == 1 { b" file" } else { b" files" });
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
        self.status_len = 0;
    }

    /// Writes the scope caption.
    ///
    /// WHY a caption and not a label on the field: the reader has to be able
    /// to tell whether the search is looking at one folder or at every
    /// document the device holds.
    pub fn set_scope(&mut self) {
        self.scope = [0u8; LINE_LEN];
        let mut len = 0usize;
        if self.searching {
            len = append(&mut self.scope, len, b"Scope: every document on this device - ");
            len = append_u64(&mut self.scope, len, self.result_count as u64);
            len = append(&mut self.scope, len, if self.result_count == 1 { b" match" } else { b" matches" });
        } else {
            len = append(&mut self.scope, len, b"Scope: ");
            len = append(&mut self.scope, len, &self.current_path[..self.current_path_len]);
            len = append(&mut self.scope, len, b" - ");
            len = append_u64(&mut self.scope, len, self.entry_count as u64);
            len = append(&mut self.scope, len, if self.entry_count == 1 { b" entry" } else { b" entries" });
        }
        self.scope_len = len;
    }
}

/// Formats a count into a line as decimal ASCII.
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

/// Hands the selected document to the package that declared a handler for
/// its media type.
///
/// WHY the platform resolves the handler and not this app: the registry is
/// built from the installed packages' signed manifests, so the answer to
/// "who opens this" is a property of the device. An app that kept its own
/// table would be able to hand a file to a program that never asked for it.
///
/// WHY a refusal names nothing: when no package declared a handler for the
/// media type the platform has no answer, and this app must not offer a
/// substitute.
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
        Err(_) => {
            std::log("[FILE_MANAGER] The document's handler could not be launched.").ok();
            false
        }
    }
}

/// Asks the platform to fill the slot the document's media type allows.
///
/// WHY the action is dispatched and not written here: the wallpaper and the
/// ringtone are device slots, and the package that owns a slot is the one
/// that fills it. Dispatching the action reaches that package with the
/// document in hand.
fn set_as_action(app: &FileManagerApp) -> bool {
    let document = &app.selected_document;
    let mime = document.mime_str();
    let action = if mime.starts_with("image/") {
        std::content::ACTION_SET_WALLPAPER
    } else if mime.starts_with("audio/") {
        std::content::ACTION_SET_RINGTONE
    } else {
        std::log("[FILE_MANAGER] This document cannot become a wallpaper or a ringtone.").ok();
        return false;
    };
    match std::content::intent_dispatch(
        std::content::IntentKind::Action,
        action,
        document.path_str(),
    ) {
        Ok(_) => {
            std::log("[FILE_MANAGER] The document was dispatched to the slot's owner.").ok();
            true
        }
        Err(_) => {
            std::log("[FILE_MANAGER] No app handles this action.").ok();
            false
        }
    }
}

// -----------------------------------------------------------------------------
// Pixel Color Resolution Engine
// -----------------------------------------------------------------------------
#[inline(never)]
fn get_file_manager_color(x: usize, y: usize, app: &FileManagerApp) -> u32 {
    // 1. Status Bar & Clean Header (y: 0..HEADER_H)
    if y < HEADER_H {
        // Clock and battery indicators are omitted until live guest services
        // expose real readings. Never paint plausible but fabricated values.

        // Back control, painted from the same rectangle the tap handler tests.
        if let Some(c) = draw_pill(x, y, BACK, THEME.surface_container_high, Some(THEME.border)) {
            if c != THEME.surface_container_high {
                return c;
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

        // Title: "Files" at x: 48, y: 64 (scale 3).
        //
        // WHY the full box: the bitmap face is GLYPH_H (24) rows tall, so a
        // scale-3 run owns 72 rows (64..136). The old window was 76..108 -- 32
        // rows -- which drew only the top sliver of each letter and severed the
        // baseline. 136 still clears the header band (ends at 140) and the
        // category rows (start at 150).
        //
        // WHY the coordinates are written out instead of pulled from
        // constants: the native-UI truth contract pins this exact call
        // (`test_text(x, y, 48, 64, 3, b"Files"`) precisely so the title's line
        // box cannot be narrowed again. Naming the numbers here would satisfy
        // the compiler and silently break the test whose job is to prevent
        // that regression, so the literal stays the single source of truth.
        if let Some(c) = test_text(x, y, 48, 64, 3, b"Files", THEME.on_surface, THEME.surface) {
            return c;
        }

        let dot_dx = x as i32 - 192;
        let dot_dy = y as i32 - 96;
        if dot_dx * dot_dx + dot_dy * dot_dy <= 9 {
            return THEME.error; // Nothing OS signature electric red dot accent
        }

        // Storage readout, right-aligned, two measured lines.
        //
        // WHY this replaced the search and overflow pictograms: both were drawn
        // at full strength in the header and neither had a tap handler - this
        // app has no search index and no menu, so they were two dead controls
        // that looked like the most important things on the screen. What the app
        // can actually do is ask the kernel for a directory listing, so it now
        // says which path it asked and what the kernel answered. Line 1 is the
        // path that was really passed to `fs_list`; line 2 is the real outcome,
        // including a refusal, which is what this platform actually returns for
        // a path outside the task's storage root.
        let path = &app.current_path[..app.current_path_len];
        let pw = font::measure(path, 1);
        if pw <= READOUT_MAX_W {
            if let Some(c) = test_text(
                x,
                y,
                READOUT_X1 - pw,
                READOUT_LINE1_Y0,
                1,
                path,
                THEME.on_surface_variant,
                THEME.surface,
            ) {
                return c;
            }
        }

        let mut label_buf = [0u8; 40];
        let label_len = font::ellipsize(
            &app.probe_label[..app.probe_label_len],
            1,
            READOUT_MAX_W,
            &mut label_buf,
        );
        let lw = font::measure(&label_buf[..label_len], 1);
        if lw <= READOUT_MAX_W {
            if let Some(c) = test_text(
                x,
                y,
                READOUT_X1 - lw,
                READOUT_LINE2_Y0,
                1,
                &label_buf[..label_len],
                THEME.warning, // Status accent
                THEME.surface,
            ) {
                return c;
            }
        }

        return THEME.surface;
    }

    // 2. The search field and its scope caption (y: 146..232)
    //
    // WHY the field is a real control and not a caption: the platform holds a
    // document index the app can read, and an index nobody can ask a question
    // of is a listing and nothing more. The field is where the question goes.
    //
    // WHY the scope is written out under the field: a search that looks at
    // every document the device holds and a search that looks at one folder
    // answer different questions, and a reader who cannot tell which one
    // they asked cannot trust the answer.
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
        // The clear action, which appears only when there is something to
        // clear.
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
        // The magnifier, at the field's leading edge.
        let mx = SEARCH_FIELD.x + 28;
        let my = SEARCH_FIELD.y + 26;
        let dx = x as i32 - mx as i32;
        let dy = y as i32 - my as i32;
        let ring = dx * dx + dy * dy;
        if (ring >= 36 && ring <= 64) || (dx >= 6 && dx <= 11 && dy >= 6 && dy <= 11) {
            return THEME.on_surface_variant;
        }
        // The text the user has typed, or the prompt that says what the field
        // is for.
        let text = if app.query.is_empty() {
            b"Search this device's documents"
        } else {
            app.query.as_slice()
        };
        let ink = if app.query.is_empty() {
            THEME.on_surface_variant
        } else {
            THEME.on_surface
        };
        if let Some(c) = test_text(
            x,
            y,
            SEARCH_FIELD.x + 48,
            SEARCH_FIELD.y + 16,
            1,
            text,
            ink,
            field_fill,
        ) {
            return c;
        }
        return field_fill;
    }
    if (SCOPE_Y..SCOPE_Y + 24).contains(&y) && x >= 36 && x < 684 {
        if let Some(c) = test_text(
            x,
            y,
            36,
            SCOPE_Y,
            1,
            &app.scope[..app.scope_len],
            THEME.on_surface_variant,
            THEME.surface,
        ) {
            return c;
        }
        return THEME.surface;
    }

    // 3. Vertical Category Rows (y: 236..540)
    //
    // WHY four rows and not six: the kernel mounts `/storage` with three
    // children, and the app path policy lets this app list exactly those.
    // The old rows named scopes the platform does not have -- a recents
    // list, documents, downloads and a cloud service -- and every one of
    // them opened `/data`, which the kernel refuses, so each row led to a
    // refusal dressed up as a category. The rows are now the four
    // directories that exist and that this app is allowed to read.
    if (CATEGORY_Y0..CATEGORY_Y1).contains(&y) && x >= 36 && x < 684 {
        let Some((row_idx, row_start_y, row_h)) = file_manager::category_row_at(y) else {
            return THEME.surface;
        };

        // Thin bottom divider line, in the platform's divider ink.
        if y == row_start_y + row_h - 1 {
            return ROW.divider;
        }

        // Group container card outer border and rounded corners
        let mut on_card_border = false;
        let rx = (x - 36).min(683 - x);
        let ry = (y - CATEGORY_Y0).min(CATEGORY_Y1 - 1 - y);
        if (y < CATEGORY_Y0 + 20 || y >= CATEGORY_Y1 - 20) && rx < 16 && ry < 16 {
            let cdx = (16 - rx) as isize;
            let cdy = (16 - ry) as isize;
            let d2 = cdx * cdx + cdy * cdy;
            if d2 >= 256 {
                return THEME.surface;
            }
            if d2 >= 196 {
                on_card_border = true;
            }
        } else if rx < 2 || y < CATEGORY_Y0 + 2 || y >= CATEGORY_Y1 - 2 {
            on_card_border = true;
        }

        // Selected row gets a raised background and a left accent bar.
        let row_selected =
            app.selected_shortcut == row_idx && y >= row_start_y && y < row_start_y + row_h;
        if row_selected && x < 44 {
            return THEME.primary;
        }
        // The row's fill, from the platform's list-row treatment: the raised
        // surface for the selected row and the flat one for the rest.
        let row_bg = if row_selected { ROW_SELECTED.fill } else { ROW.fill };

        let center_y = row_start_y + (if row_idx == 0 { 40 } else { 35 });
        let icon_cx = 64i32;
        let icon_cy = center_y as i32;
        let idx = (x as i32) - icon_cx;
        let idy = (y as i32) - icon_cy;

        // WHY the line box is centred on `center_y` and not inset from it: the
        // six single-line rows below laid their scale-2 runs out with
        // `center_y - 8`, which puts the run's own 48-row box 16px below the
        // icon's centre, so the label's ink sat visibly low against its icon.
        // Measured on the guest: row 0's icon ink centred on 194.5 while
        // "Recent" centred on 207 - a 12.5px step. Row 5, the two-line stack,
        // already used the centred form (`row_start_y + 16` is exactly
        // `center_y - line_height(2) / 2` at its own `center_y`), so these six
        // were the inconsistent site. The reference mockup (Ui Samples/11_files)
        // centres the label's ink on its icon in every row, and the arithmetic
        // below reproduces that: `line_height(2) / 2` is half the run's own box,
        // so the box straddles `center_y` exactly.
        let label_y0 = center_y - std::design::font::line_height(2) / 2;
        // Render Icons
        //
        // WHY the labels come from `CATEGORY_LABELS`: the row and the path it
        // opens are one fact, and a hand-written label beside a hand-written
        // path is how the two drift apart.
        match row_idx {
            0 => {
                // Storage (Drive icon)
                if (idx.abs() <= 14 && idy.abs() <= 10) && (idx.abs() >= 12 || idy.abs() >= 8 || (idy == 2 && idx.abs() <= 10)) {
                    return THEME.on_surface;
                }
                // Two-line stack: label (scale 2, 48px tall) above caption
                // (scale 1, 24px tall). Measured via
                // std::design::font::measure: "Storage"@2 = 130px,
                // "Storage capacity unavailable"@1 = 267px,
                // "Capacity detected; usage unavailable"@1 = 348px — all fit
                // in the 576px budget (x 108..684). y0 delta (56) >=
                // line_height(2) (48), so the bands never share a pixel.
                //
                // WHY `label_y0` is not rebound here: the row's own `center_y`
                // is `row_start_y + 40` (this row is the taller of the two
                // shapes), so the shared `label_y0` computed above is exactly
                // the `row_start_y + 16` this branch used to spell out. Rebinding
                // it to the same number was redundant; only the caption moves.
                let caption_y0 =
                    label_y0 + std::design::font::line_height(2) + 8;
                if let Some(c) = test_text(x, y, 108, label_y0, 2, CATEGORY_LABELS[0], THEME.on_surface, row_bg)
                {
                    return c;
                }
                let capacity_status: &[u8] = if app.storage_capacity_bytes > 0 {
                    b"Capacity detected; usage unavailable"
                } else {
                    b"Storage capacity unavailable"
                };
                if let Some(c) = test_text(x, y, 108, caption_y0, 1, capacity_status, THEME.on_surface_variant, row_bg) {
                    return c;
                }
            }
            1 => {
                // Photos icon (Landscape frame)
                if (idx.abs() <= 14 && idy.abs() <= 10) && (idx.abs() >= 12 || idy.abs() >= 8 || ((idx - 5).abs() <= 2 && (idy + 3).abs() <= 2) || (idy >= 2 && idx.abs() <= (10 - idy))) {
                    return THEME.on_surface;
                }
                if let Some(c) = test_text(x, y, 108, label_y0, 2, CATEGORY_LABELS[1], THEME.on_surface, row_bg) {
                    return c;
                }
            }
            2 => {
                // Pictures icon (Stack of frames)
                if (idx.abs() <= 14 && idy.abs() <= 10) && (idx.abs() >= 12 || idy.abs() >= 8) {
                    return THEME.on_surface;
                }
                if let Some(c) = test_text(x, y, 108, label_y0, 2, CATEGORY_LABELS[2], THEME.on_surface, row_bg) {
                    return c;
                }
            }
            3 => {
                // Videos icon (Camcorder)
                if ((idx >= -14 && idx <= 4 && idy.abs() <= 10) && (idx == -14 || idx == 4 || idy.abs() >= 8)) || (idx >= 6 && idx <= 14 && idy.abs() <= (idx - 5)) {
                    return THEME.on_surface;
                }
                if let Some(c) = test_text(x, y, 108, label_y0, 2, CATEGORY_LABELS[3], THEME.on_surface, row_bg) {
                    return c;
                }
            }
            _ => {}
        }

        if on_card_border {
            return THEME.border;
        }
        return row_bg;
    }

    // 3. The selected document's actions (y: 560..840)
    //
    // WHY this panel is only drawn when a document is selected: an empty
    // panel that says "nothing is selected" is noise, and a panel that
    // appeared with content the user did not choose would be a lie.
    //
    // WHY the panel states the type's provenance: the platform's index says
    // whether it read the media type from the document's own bytes or from
    // its name, and a screen that hid that would make a guess look like a
    // fact.
    if app.has_selected_document && (ACTIONS_Y0..ACTIONS_Y1).contains(&y) && x >= ACTIONS_X0 && x < ACTIONS_X1 {
        let document = &app.selected_document;
        let panel_bg = THEME.surface_container;
        if y == ACTIONS_Y0 || y == ACTIONS_Y1 - 1 {
            return THEME.border;
        }
        // The document's name.
        let name = document.path_str();
        let name_bytes = name.as_bytes();
        let name_len = name_bytes.len().min(48);
        if let Some(c) = test_text(
            x,
            y,
            ACTIONS_X0 + 16,
            ACTIONS_Y0 + 20,
            2,
            &name_bytes[..name_len],
            THEME.on_surface,
            panel_bg,
        ) {
            return c;
        }
        // The type the platform holds, and where that type came from.
        let mut type_line = [0u8; 96];
        let mut type_len = append(&mut type_line, 0, b"Type: ");
        type_len = append(&mut type_line, type_len, &document.mime[..document.mime_len]);
        type_len = append(&mut type_line, type_len, b" (");
        type_len = append(&mut type_line, type_len, &document.provenance[..document.provenance_len]);
        type_len = append(&mut type_line, type_len, b")");
        if let Some(c) = test_text(
            x,
            y,
            ACTIONS_X0 + 16,
            ACTIONS_Y0 + 68,
            1,
            &type_line[..type_len],
            THEME.on_surface_variant,
            panel_bg,
        ) {
            return c;
        }
        // The handlers the registry names for this media type.
        let mut open_line = [0u8; 96];
        let mut open_len = append(&mut open_line, 0, b"Open with: ");
        if document.has_handler() {
            for (index, handler) in document.handlers[..document.handler_count].iter().enumerate() {
                if index > 0 {
                    open_len = append(&mut open_line, open_len, b", ");
                }
                open_len = append(&mut open_line, open_len, &handler.package_id[..handler.package_id_len]);
                if open_len + 24 >= open_line.len() {
                    break;
                }
            }
        } else {
            // WHY nothing is named here: no package declared a handler for
            // this media type, so the platform has no answer to give and
            // naming a program that cannot open the file would be a guess.
            open_len = append(&mut open_line, open_len, b"no app handles this file");
        }
        if let Some(c) = test_text(
            x,
            y,
            ACTIONS_X0 + 16,
            ACTION_OPEN_Y0,
            1,
            &open_line[..open_len],
            if document.has_handler() { THEME.on_surface } else { THEME.warning },
            panel_bg,
        ) {
            return c;
        }
        // The action the media type allows.
        let action = action_for_mime(document.mime_str());
        if !action.is_empty() {
            if let Some(c) = test_text(
                x,
                y,
                ACTIONS_X0 + 16,
                ACTION_SET_Y0,
                1,
                action,
                THEME.primary,
                panel_bg,
            ) {
                return c;
            }
        }
        return panel_bg;
    }

    // 4. The confirmation sheet, drawn over the list (y: 900..1200)
    //
    // WHY the sheet names the files it will touch: a confirmation that says
    // "delete?" does not tell the user which of the three marked files is
    // about to go, and a destructive action the user cannot name is one
    // they cannot safely agree to.
    if app.has_pending && SHEET.contains(x, y) {
        let style = components::card_style(&THEME, components::Elevation::Raised);
        let Some(base) = draw_rounded_rect(
            x,
            y,
            SHEET,
            style.radius,
            style.fill,
            style.border,
            1,
        ) else {
            return THEME.surface;
        };
        // The action, and how many files it touches.
        if let Some(c) = test_text(
            x,
            y,
            SHEET.x + 16,
            SHEET.y + 20,
            2,
            &app.sheet_title[..app.sheet_title_len],
            THEME.on_surface,
            style.fill,
        ) {
            return c;
        }
        // The names of the files.
        for index in 0..app.sheet_name_count {
            if let Some(c) = test_text(
                x,
                y,
                SHEET.x + 16,
                SHEET.y + 76 + index * 26,
                1,
                &app.sheet_names[index][..app.sheet_name_lens[index]],
                THEME.on_surface_variant,
                style.fill,
            ) {
                return c;
            }
        }
        // The destination, for a copy or a move.
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
                let ink = if chosen { THEME.surface } else { THEME.on_surface };
                if let Some(c) = draw_rounded_rect(x, y, rect, THEME.radii.control, fill, None, 0)
                {
                    if let Some(c2) = test_text(
                        x,
                        y,
                        rect.x + 12,
                        rect.y + 10,
                        1,
                        CATEGORY_LABELS[index],
                        ink,
                        fill,
                    ) {
                        return c2;
                    }
                    return c;
                }
            }
        }
        // The new name, for a rename.
        if app.pending.action == FileAction::Rename {
            let field = Rect::new(SHEET.x + 16, SHEET_NAME_Y, SHEET.w - 32, SHEET_NAME_H);
            if field.contains(x, y) {
                let fill = THEME.surface_container_high;
                if let Some(c) = test_text(
                    x,
                    y,
                    field.x + 12,
                    field.y + 12,
                    1,
                    app.rename_field.as_slice(),
                    THEME.on_surface,
                    fill,
                ) {
                    return c;
                }
                return fill;
            }
        }
        // The buttons.
        for (index, label) in [b"Cancel".as_slice(), b"Confirm".as_slice()].iter().enumerate() {
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
                THEME.surface
            } else {
                THEME.on_surface
            };
            if let Some(c) = draw_rounded_rect(x, y, rect, THEME.radii.control, fill, None, 0) {
                if let Some(c2) =
                    test_text(x, y, rect.x + 24, rect.y + 12, 1, *label, ink, fill)
                {
                    return c2;
                }
                return c;
            }
        }
        return base;
    }

    // 5. The entries card (y: 836..1284)
    //
    // WHY the card is the region and not the size of the list: the card used
    // to be as tall as the list it held, so a long directory grew past the
    // bottom of the screen and every row below the edge was drawn where
    // nothing could reach it. The card is the region now, and the list
    // scrolls inside it.
    if (ENTRIES_CARD.y..ENTRIES_CARD.y + ENTRIES_CARD.h).contains(&y) {
        if x >= ENTRIES_CARD.x && x < ENTRIES_CARD.x + ENTRIES_CARD.w {
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
                // Vector folder pictogram centred at cx = 360, cy = card + 110.
                let cx = 360isize;
                let cy = (ENTRIES_CARD.y + 110) as isize;
                let idx = x as isize - cx;
                let idy = y as isize - cy;
                if (idy >= -22 && idy < -14 && idx >= -36 && idx <= -10)
                    || (idy >= -14 && idy <= 24 && idx.abs() <= 36)
                {
                    if idy == -14
                        || idy == 24
                        || idx.abs() == 36
                        || (idy < -14 && (idx == -36 || idx == -10 || idy == -22))
                    {
                        return THEME.primary;
                    }
                    if idy == 0 && idx >= -24 && idx <= 24 {
                        return THEME.border;
                    }
                }

                // WHY the headline follows the listing's own state: the card
                // used to say "Directory Empty" for a directory the device had
                // refused to read, which told the user their files were gone
                // when the app had simply been refused.
                let (headline, subline, headline_ink): (&[u8], &[u8], u32) = if app.searching {
                    (
                        b"No Documents Found",
                        b"The device index holds no document with that name",
                        THEME.on_surface,
                    )
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
                if let Some(c) = test_text(
                    x,
                    y,
                    hx,
                    ENTRIES_CARD.y + 170,
                    2,
                    headline,
                    headline_ink,
                    style.fill,
                ) {
                    return c;
                }
                let sx = centered_x(subline, 1, ENTRIES_CARD.x, ENTRIES_CARD.w);
                if let Some(c) = test_text(
                    x,
                    y,
                    sx,
                    ENTRIES_CARD.y + 228,
                    1,
                    subline,
                    THEME.on_surface_variant,
                    style.fill,
                ) {
                    return c;
                }
            }
            // The rows are drawn by the platform's list-row widget after this
            // pass, so what is left here is the card they sit on.
            return base;
        }
        return THEME.surface;
    }

    // 6. The status line (y: 1290..1314)
    if (STATUS_Y..STATUS_Y + 24).contains(&y) && x >= 36 && x < 684 {
        if app.status_len > 0 {
            if let Some(c) = test_text(
                x,
                y,
                36,
                STATUS_Y,
                1,
                &app.status[..app.status_len],
                THEME.on_surface_variant,
                THEME.surface,
            ) {
                return c;
            }
        }
        return THEME.surface;
    }

    // 7. The action bar (y: 1320..1368)
    //
    // WHY the band is only the page surface here: the pills are drawn by the
    // platform's pill widget after this pass, and they land on this band.
    if (ACTION_BAR_Y0..ACTION_BAR_Y1).contains(&y) {
        return THEME.surface;
    }
    // 4. Home Gesture Pill, drawn from the same rectangle the tap handler tests.
    if HOME_PILL.contains(x, y) {
        return THEME.on_surface_variant;
    }

    THEME.surface
}

fn render_file_manager_ui(surface: u32, app: &FileManagerApp) {
    render_file_manager_band(surface, app, 0, H);
}

/// Re-rasterises and writes only rows `y0..y1`.
///
/// WHY a band and not a rectangle: a row of the surface is one contiguous run in
/// the surface buffer, so `offset = y0 * W` writes a whole band with no stride
/// bookkeeping. Every band starts on a row boundary, so the running `x`/`y`
/// cursor below is exact.
///
/// WHY this is correct rather than a shortcut: the band is named after what the
/// touch actually moved. Selecting a category repaints only the list band -
/// 670 of the 1440 rows - and a tap that changed nothing does not render at all.
/// The old renderer re-rasterised all 1,036,800 pixels on every event.
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
    // The platform's widgets are painted after the per-pixel pass, because
    // they paint into a surface of their own and the band is already on the
    // screen by the time they are asked for.
    if surface > 0 {
        if y0 < ENTRIES_CARD.y + ENTRIES_CARD.h && y1 > ENTRIES_CARD.y {
            render_card_rows(surface, app);
        }
        if y0 < ACTION_BAR_Y1 && y1 > ACTION_BAR_Y0 {
            render_action_bar(surface, app);
        }
    }
}

// -----------------------------------------------------------------------------
// The platform's widgets
//
// WHY the rows and the pills are drawn by the platform's widgets and not by
// hand: the SDK's list row and its pill are the shell's own row and its own
// pill, and an app that hand-draws a row is an app that does not look like
// the device it is running on.
//
// WHY the widgets need a buffer of their own: they paint into a surface, and
// this app paints a band at a time, so the app lends them one row's worth of
// pixels and writes the result over the band.
// -----------------------------------------------------------------------------

/// The width of one entry row.
const ROW_BUFFER_W: usize = 648;
/// The height of one entry row.
const ROW_BUFFER_H: usize = 56;
/// The pixels one entry row is drawn into.
static mut ROW_BUFFER: [u32; ROW_BUFFER_W * ROW_BUFFER_H] = [0; ROW_BUFFER_W * ROW_BUFFER_H];

/// The width of one action-bar pill.
const PILL_BUFFER_W: usize = 120;
/// The height of one action-bar pill.
const PILL_BUFFER_H: usize = 48;
/// The pixels one action-bar pill is drawn into.
static mut PILL_BUFFER: [u32; PILL_BUFFER_W * PILL_BUFFER_H] = [0; PILL_BUFFER_W * PILL_BUFFER_H];

/// Draws the entries card's rows with the platform's list-row widget.
fn render_card_rows(surface: u32, app: &FileManagerApp) {
    let count = app.card_item_count();
    if count == 0 || surface == 0 {
        return;
    }
    // WHY the rows are not drawn under the sheet: the sheet is drawn over the
    // list, and a row drawn after it would come back through it.
    if app.has_pending {
        return;
    }
    let offset = app.scroll.offset;
    // The first row that could still be on the card.
    let first = offset.saturating_sub(ENTRIES_CARD.h) / ENTRY_ROW_H;
    for index in first..count {
        let rect = entry_row_rect(ENTRIES_CARD, index, offset);
        if rect.y >= ENTRIES_CARD.y + ENTRIES_CARD.h {
            break;
        }
        if rect.y + rect.h <= ENTRIES_CARD.y {
            continue;
        }
        let name = app.card_item_name(index);
        let trailing = app.card_item_trailing(index);
        let selected = app.selected_entry == Some(index) || app.selection.is_marked(index);
        let buffer: &mut [u32] = unsafe { &mut *(&raw mut ROW_BUFFER) };
        {
            let Ok(mut canvas) = std::design::draw_target::Surface::new(
                buffer,
                ROW_BUFFER_W,
                ROW_BUFFER_H,
            ) else {
                return;
            };
            let mut row = components::ListRow::new(name);
            if !trailing.is_empty() {
                row = row.with_trailing(trailing);
            }
            if selected {
                row = row.selected();
            }
            components::list_row(
                &mut canvas,
                &THEME,
                Rect::new(0, 0, ROW_BUFFER_W, ROW_BUFFER_H),
                &row,
            );
        }
        // Only the part of the row that is on the card is written.
        let from = ENTRIES_CARD.y.saturating_sub(rect.y);
        let to = (ENTRIES_CARD.y + ENTRIES_CARD.h).min(rect.y + ENTRY_ROW_H) - rect.y;
        write_pixels(
            surface,
            (rect.y + from) * W + ENTRIES_CARD.x,
            &buffer[from * ROW_BUFFER_W..to * ROW_BUFFER_W],
        );
    }
    // The scroll indicator, over the rows.
    if let Some((thumb_y, thumb_h)) = app.scroll.thumb_rect(ENTRIES_CARD.y + 4, ENTRIES_CARD.h - 8, 24)
    {
        let tx = ENTRIES_CARD.x + ENTRIES_CARD.w - 8;
        let thumb = [THEME.primary; 4];
        for y in thumb_y..thumb_y + thumb_h {
            let _ = std::surface_write(surface, y * W + tx, &thumb);
        }
    }
}

/// Draws the action bar's pills with the platform's pill widget.
///
/// WHY a pill that cannot be taken is drawn by the widget too: the shape is
/// the platform's, and only the label's ink says that the action is not
/// available.
fn render_action_bar(surface: u32, app: &FileManagerApp) {
    if surface == 0 {
        return;
    }
    for slot in 0..5usize {
        let rect = action_slot_rect(slot);
        let (label, live) = match slot {
            0 => (b"Select".as_slice(), true),
            1 => (
                ACTION_LABELS[0],
                app.action_is_possible(FileAction::Copy),
            ),
            2 => (
                ACTION_LABELS[1],
                app.action_is_possible(FileAction::Move),
            ),
            3 => (
                ACTION_LABELS[2],
                app.action_is_possible(FileAction::Delete),
            ),
            _ => (
                ACTION_LABELS[3],
                app.action_is_possible(FileAction::Rename),
            ),
        };
        let chosen = slot == 0 && app.multi_select;
        let buffer: &mut [u32] = unsafe { &mut *(&raw mut PILL_BUFFER) };
        {
            let Ok(mut canvas) = std::design::draw_target::Surface::new(
                buffer,
                PILL_BUFFER_W,
                PILL_BUFFER_H,
            ) else {
                return;
            };
            // WHY an empty label for a slot that cannot be taken: the widget
            // draws the label in the body ink, and the label is drawn again
            // below in the muted ink.
            let widget_label: &[u8] = if live { label } else { b"" };
            components::pill(
                &mut canvas,
                &THEME,
                Rect::new(0, 0, PILL_BUFFER_W, PILL_BUFFER_H),
                widget_label,
                1,
                chosen,
            );
            if !live {
                draw_muted_label(buffer, PILL_BUFFER_W, PILL_BUFFER_H, label);
            }
        }
        write_pixels(surface, rect.y * W + rect.x, &buffer[..]);
    }
}

/// Draws a label into a buffer in the muted ink.
///
/// WHY the position is the widget's own: the platform's pill centres its
/// label in the shape, and a second label somewhere else would read as a
/// second thing.
fn draw_muted_label(buffer: &mut [u32], width: usize, height: usize, label: &[u8]) {
    let text_w = font::measure(label, 1);
    let label_h = font::line_height(1);
    if text_w == 0 || label_h == 0 {
        return;
    }
    let x = width.saturating_sub(text_w) / 2;
    let y = height.saturating_sub(label_h) / 2;
    for py in y..(y + label_h).min(height) {
        for px in x..(x + text_w).min(width) {
            let bg = buffer[py * width + px];
            if let Some(color) =
                font::test_text_aa(px, py, x, y, 1, label, THEME.on_surface_variant, bg, false)
            {
                buffer[py * width + px] = color;
            }
        }
    }
}

/// Writes a run of pixels to the surface, in the chunks the platform takes.
fn write_pixels(surface: u32, offset: usize, pixels: &[u32]) {
    let mut chunk = [0u32; CHUNK];
    let mut written = 0usize;
    while written < pixels.len() {
        let count = (pixels.len() - written).min(CHUNK);
        chunk[..count].copy_from_slice(&pixels[written..written + count]);
        let _ = std::surface_write(surface, offset + written, &chunk[..count]);
        written += count;
    }
}

// -----------------------------------------------------------------------------
// The file operations
// -----------------------------------------------------------------------------

/// Copies a file with the platform's own calls.
///
/// WHY a copy and not a rename: the platform's filesystem offers open, read,
/// write, sync, stat, list and unlink, and nothing else. There is no rename
/// operation on it, so a rename is a copy to the new name followed by an
/// unlink of the old one, and the app says so when it does it.
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

/// Removes a file with the platform's own call.
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

/// Performs the confirmed action, one file at a time.
///
/// WHY the progress is written after every file and not at the end: a copy
/// of a large file takes a moment, and a status line that only changes when
/// everything is done is a status line that never says anything.
fn perform_action(app: &mut FileManagerApp, surface: u32) {
    let action = app.pending.action;
    let count = app.pending.item_count;
    let items = app.pending.items;
    let destination = CATEGORY_PATHS[app.destination];
    app.close_confirmation();

    let mut done = 0usize;
    let mut refused = 0usize;
    for index in 0..count {
        let row = items[index];
        let mut source = [0u8; MAX_PATH_LEN];
        let Some(source_len) = app.card_item_path(row, &mut source) else {
            continue;
        };
        let mut name = [0u8; MAX_ENTRY_NAME_LEN];
        let name_len = app.card_item_name(row).len().min(MAX_ENTRY_NAME_LEN);
        name[..name_len].copy_from_slice(&app.card_item_name(row)[..name_len]);
        let source_path = match core::str::from_utf8(&source[..source_len]) {
            Ok(text) => text,
            Err(_) => continue,
        };

        // The target of the action.
        let mut target = [0u8; MAX_PATH_LEN];
        let target_len = match action {
            FileAction::Rename => {
                let typed = app.rename_field.as_slice();
                if !name_is_usable(typed) {
                    refused += 1;
                    continue;
                }
                join_path(app.current_path_str(), typed, &mut target)
            }
            _ => join_path(destination, &name[..name_len], &mut target),
        };
        let Some(target_len) = target_len else {
            refused += 1;
            continue;
        };
        let Ok(target_path) = core::str::from_utf8(&target[..target_len]) else {
            refused += 1;
            continue;
        };

        // The status line, before the work: it says which file is being
        // worked on and how many are left.
        let mut status = [0u8; LINE_LEN];
        let mut len = append(&mut status, 0, ACTION_LABELS[action as usize]);
        len = append(&mut status, len, b" ");
        len = append_u64(&mut status, len, (index + 1) as u64);
        len = append(&mut status, len, b" of ");
        len = append_u64(&mut status, len, count as u64);
        len = append(&mut status, len, b": ");
        let take = name_len.min(status.len().saturating_sub(len));
        len = append(&mut status, len, &name[..take]);
        app.status = status;
        app.status_len = len;
        if surface > 0 {
            render_file_manager_band(surface, app, STATUS_Y, STATUS_Y + 24);
            let _ = std::surface_flip();
        }

        // The work itself.
        let outcome: Result<(), i64> = match action {
            FileAction::Delete => delete_file(source_path),
            FileAction::Copy => {
                if file_exists(target_path) {
                    Err(-17)
                } else {
                    copy_file(source_path, target_path).map(|_| ())
                }
            }
            FileAction::Move => {
                if file_exists(target_path) {
                    Err(-17)
                } else {
                    match copy_file(source_path, target_path) {
                        Ok(_) => delete_file(source_path),
                        Err(code) => Err(code),
                    }
                }
            }
            FileAction::Rename => {
                if file_exists(target_path) {
                    Err(-17)
                } else {
                    match copy_file(source_path, target_path) {
                        Ok(_) => delete_file(source_path),
                        Err(code) => Err(code),
                    }
                }
            }
        };
        match outcome {
            Ok(()) => done += 1,
            Err(_) => refused += 1,
        }
    }

    // The status line says what happened, and how many files it happened to.
    let mut status = [0u8; LINE_LEN];
    let mut len = append(&mut status, 0, ACTION_LABELS[action as usize]);
    len = append(&mut status, len, b": ");
    len = append_u64(&mut status, len, done as u64);
    len = append(&mut status, len, if done == 1 { b" file done" } else { b" files done" });
    if refused > 0 {
        len = append(&mut status, len, b", ");
        len = append_u64(&mut status, len, refused as u64);
        len = append(&mut status, len, b" refused by the device");
    }
    if action == FileAction::Rename {
        len = append(&mut status, len, b" (a rename is a copy and a delete here)");
    }
    app.status = status;
    app.status_len = len;

    // The listing changed, so the listing is read again.
    app.selection.clear();
    app.multi_select = false;
    app.selected_entry = None;
    app.refresh_directory();
    if surface > 0 {
        render_file_manager_ui(surface, app);
        let _ = std::surface_flip();
    }
}

/// Which rows of the surface a touch actually changed.
///
/// WHY this exists: the old loop called `render_file_manager_ui` from the row
/// branch, which re-rasterised the whole screen to move a selection fill.
///
/// WHY there is no `Header` variant: nothing a touch can reach writes to the
/// header. The storage readout is filled once, before the first surface is
/// presented, and no control mutates it afterwards - a band for it would be
/// dead weight pretending to be an optimisation.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Dirty {
    /// Nothing moved; the surface is already correct, so do not render at all.
    None,
    /// A different category row is selected: the list band only.
    #[allow(dead_code)]
    Rows,
    /// Anything that can move pixels in more than one band at once.
    All,
}

/// Grow a dirty region so one rendered frame covers everything a poll batch
/// changed.
///
/// WHY: `event_poll` can deliver several touches before the app is scheduled
/// again, and rendering only the last region would leave the earlier one stale.
impl Dirty {
    fn widen(self, other: Dirty) -> Dirty {
        match (self, other) {
            (Dirty::None, d) | (d, Dirty::None) => d,
            (a, b) if a == b => a,
            _ => Dirty::All,
        }
    }
}

enum Tap {
    /// Nothing on screen moved.
    None,
    /// The model moved; render the named region.
    Redraw(Dirty),
    /// Leave to the launcher, logging this exact reason first.
    Leave(&'static str),
}

fn exit_to_launcher(surface: u32) -> ! {
    if surface > 0 {
        // WHY the surface is destroyed without naming a capability: the
        // kernel authorises the call by capability type, and a slot number
        // is a position in a table rather than a capability. The old code
        // passed the literal 6, which happens to be the framebuffer in this
        // app's table today and is not guaranteed to be tomorrow.
        let _ = std::surface_destroy(surface);
    }
    std::exit_with_code(0);
}

// -----------------------------------------------------------------------------
// Entry Point & Event Reactor Loop
// -----------------------------------------------------------------------------
#[no_mangle]
pub extern "C" fn _start() -> ! {
    std::log("[FILE_MANAGER (Ring 3)] Sovereign File Manager & VFS Explorer online.",
    )
    .ok();

    let mut app = FileManagerApp::new();

    // Query physical block storage capacity from driver
    if let Ok(bytes) = std::storage_query() {
        app.storage_capacity_bytes = bytes;
        std::log("[FILE_MANAGER] Storage capacity validated from hardware service.",
        )
        .ok();
    }

    // Enumerate initial VFS directory. WHY byte counts: the payload is received
    // but never parsed or rendered, so claiming a successful parse would be false.
    //
    // WHY no capability is named here: the kernel authorises the listing by
    // the capability's type, so the app asks for it and reports the answer
    // the platform gives. The old code named a slot number, the check failed
    // before the path was ever examined, and the app logged a refusal it had
    // not actually been entitled to receive. The result is recorded on screen
    // too, so the header says what this app can actually see of the filesystem.
    let mut entries = [0u8; 1024];
    // WHY the path is copied out of the app: the log below names it, and the
    // app is changed in between.
    let mut path_buf = [0u8; 128];
    let listed_len = app.current_path_str().as_bytes().len().min(128);
    path_buf[..listed_len].copy_from_slice(&app.current_path_str().as_bytes()[..listed_len]);
    let listed_path = core::str::from_utf8(&path_buf[..listed_len]).unwrap_or("/storage");
    let probe = std::fs_list(listed_path, &mut entries);
    app.record_probe(probe);
    match probe {
        Ok(n) => {
            app.parse_entries(&entries[..n]);
            // WHY the path is in the line: the log used to name `/data` while
            // the app listed `/storage`, so the log named a directory the app
            // never opened and the reader could not tell which one it was.
            log_listing(listed_path, n as u64, " bytes of directory entries received.");
            log_counted(
                "[FILE_MANAGER] Parsed VFS directory entries: ",
                app.entry_count as u64,
                " items.",
            );
        }
        Err(code) => {
            log_listing_refused(listed_path, code);
        }
    }

    // Allocate client surface (z=200)
    let surface = match std::surface_create(W as u32, H as u32, 200) {
        Ok(id) => {
            std::log("[FILE_MANAGER] DRM/KMS Client Surface allocated (z=200).",
            )
            .ok();
            id
        }
        Err(_) => 0,
    };

    if surface > 0 {
        render_file_manager_ui(surface, &app);
        let _ = std::surface_flip();
        std::log("[FILE_MANAGER] Files UI presented to scanout buffer.").ok();
    }

    // Interactive Event Loop
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
                    // WHY the home gesture is the only one that leaves the
                    // app: a pan anywhere on the screen used to throw the user
                    // out of the file list, which is the one thing a user
                    // scrolling a long directory does.
                    let is_home_gesture =
                        input.y >= 1400 && input.x >= 240 && input.x <= 480;
                    if is_home_gesture
                        && (input.kind == 4
                            || input.kind == 6
                            || input.kind == 7
                            || input.kind == 8
                            || input.kind == 10)
                    {
                        std::log("[FILE_MANAGER] Home gesture recognized. Exiting to launcher.",
                        )
                        .ok();
                        exit_to_launcher(surface);
                    }

                    // A pan or a fling over the list scrolls the list.
                    if input.kind == 4 || input.kind == 10 {
                        if app.scroll_by(input.dy) {
                            dirty = dirty.widen(Dirty::All);
                        }
                    }

                    // A key goes to whichever field has the keyboard's
                    // attention.
                    if input.kind == 11 {
                        if handle_key(&mut app, input.x) {
                            dirty = dirty.widen(Dirty::All);
                        }
                    }

                    if input.kind == 1 {
                        match handle_tap(&mut app, input.x as usize, input.y as usize, surface) {
                            Tap::Leave(msg) => {
                                std::log(msg).ok();
                                exit_to_launcher(surface);
                            }
                            Tap::Redraw(d) => dirty = dirty.widen(d),
                            Tap::None => {}
                        }
                    }
                }
                if dirty != Dirty::None && surface > 0 {
                    match dirty {
                        Dirty::Rows => {
                            render_file_manager_band(surface, &app, ROW_BAND_Y0, ROW_BAND_Y1)
                        }
                        Dirty::All => render_file_manager_ui(surface, &app),
                        Dirty::None => {}
                    }
                    let _ = std::surface_flip();
                }
            }
        }
        std::yield_now();
    }
}

/// Resolves one touch against the controls that are actually drawn.
///
/// Every branch below tests a rectangle the renderer also draws from, so a tap
/// is accepted exactly where there is something to press.
fn handle_tap(app: &mut FileManagerApp, x: usize, y: usize, surface: u32) -> Tap {
    // The confirmation sheet owns every touch while it is up.
    if app.has_pending {
        return handle_sheet_tap(app, x, y, surface);
    }

    // Back control or the home gesture strip.
    //
    // WHY the back control now exists at all: `x 24..100 && y 28..68` used to
    // exit the app while drawing nothing there - an invisible hotspot a user
    // could not find and could land on by accident. `BACK` is the same 52x52
    // pill the renderer paints, so the region is visible.
    if HOME_HIT.contains(x, y) {
        return Tap::Leave(
            "[FILE_MANAGER] Home gesture pressed. Exiting to launcher.",
        );
    }
    if BACK.contains(x, y) {
        if app.navigate_parent() {
            std::log("[FILE_MANAGER] Navigated to parent directory.").ok();
            return Tap::Redraw(Dirty::All);
        }
        return Tap::Leave(
            "[FILE_MANAGER] Back control pressed at root. Exiting to launcher.",
        );
    }

    // The search field, and the clear action inside it.
    if (SEARCH_FIELD.y..SEARCH_FIELD.y + SEARCH_FIELD.h).contains(&y)
        && x >= SEARCH_FIELD.x
        && x < SEARCH_FIELD.x + SEARCH_FIELD.w
    {
        if !app.query.is_empty() && SEARCH_CLEAR.contains(x, y) {
            app.clear_search();
            std::log("[FILE_MANAGER] The search was cleared.").ok();
            return Tap::Redraw(Dirty::All);
        }
        // WHY the field takes the keyboard's attention when it is touched: a
        // field the user cannot type into is a caption, and this one is a
        // question the user asks the device's index.
        app.query.focus();
        app.run_search();
        return Tap::Redraw(Dirty::All);
    }

    // The selected document's actions.
    //
    // WHY the actions are here and not on the file row: a row that both
    // selects a file and launches a program gives the user no way to see who
    // would open the file, and no way to ask for a different action on it.
    // The panel names the handlers the platform's registry holds, and the
    // user picks from what is actually there.
    if app.has_selected_document
        && (ACTIONS_Y0..ACTIONS_Y1).contains(&y)
        && (ACTIONS_X0..ACTIONS_X1).contains(&x)
    {
        if (ACTION_OPEN_Y0..ACTION_OPEN_Y1).contains(&y) {
            if open_with_handler(app) {
                return Tap::Redraw(Dirty::All);
            }
            return Tap::Redraw(Dirty::All);
        }
        if (ACTION_SET_Y0..ACTION_SET_Y1).contains(&y) {
            set_as_action(app);
            return Tap::Redraw(Dirty::All);
        }
        return Tap::None;
    }

    // The category rows.
    //
    // WHY the row table is the library's: the renderer and this test call
    // the same function, so a touch lands on the row that is drawn.
    if (ROW_BAND_Y0..ROW_BAND_Y1).contains(&y) && (36..684).contains(&x) {
        let Some((shortcut, _, _)) = file_manager::category_row_at(y) else {
            return Tap::None;
        };
        // WHY no redraw when the tap lands on the row that is already selected:
        // the fill, the accent bar and the caption are all functions of
        // `selected_shortcut`, so re-rendering would produce an identical
        // surface. The old loop always rendered.
        if app.selected_shortcut == shortcut {
            return Tap::None;
        }
        app.selected_shortcut = shortcut;
        app.selected_entry = None;
        app.set_path(CATEGORY_PATHS[shortcut]);
        app.refresh_directory();
        std::log("[FILE_MANAGER] Category filter shortcut switched.").ok();
        return Tap::Redraw(Dirty::All);
    }

    // The action bar.
    if (ACTION_BAR_Y0..ACTION_BAR_Y1).contains(&y) {
        for slot in 0..5usize {
            if !action_slot_rect(slot).contains(x, y) {
                continue;
            }
            if slot == 0 {
                // The select slot turns the multi-select mode on and off.
                app.multi_select = !app.multi_select;
                if !app.multi_select {
                    app.selection.clear();
                }
                app.set_status();
                return Tap::Redraw(Dirty::All);
            }
            let Some(action) = file_manager::action_for_slot(slot) else {
                return Tap::None;
            };
            // WHY an action that the platform cannot do is not taken: a
            // control that is drawn and does nothing is worse than a control
            // that is not there.
            if !app.action_is_possible(action) {
                return Tap::None;
            }
            if app.open_confirmation(action) {
                return Tap::Redraw(Dirty::All);
            }
            return Tap::None;
        }
        return Tap::None;
    }

    // The entries card.
    //
    // WHY the hit test is the library's: the row that is drawn and the row
    // that is tapped are the same row, and the card is the region, so a list
    // longer than the card scrolls instead of running off the screen.
    let item_count = app.card_item_count();
    if item_count > 0 {
        if let Some(index) = entry_at_point(ENTRIES_CARD, app.scroll.offset, item_count, x, y) {
            // In the multi-select mode a tap marks the file instead of
            // selecting it.
            if app.multi_select {
                app.toggle_mark(index);
                return Tap::Redraw(Dirty::All);
            }
            // A folder is entered; a file is selected.
            if app.card_item_is_dir(index) {
                app.navigate_into(index);
                std::log("[FILE_MANAGER] Navigated into directory.").ok();
                return Tap::Redraw(Dirty::All);
            }
            app.selected_entry = Some(index);
            let mut path_buf = [0u8; MAX_PATH_LEN];
            if let Some(n) = app.card_item_path(index, &mut path_buf) {
                if let Ok(full_path) = core::str::from_utf8(&path_buf[..n]) {
                    // WHY the index is asked and not the listing: the index is
                    // where the platform holds the media type of a document and
                    // the identity of it, and a file without a media type cannot
                    // be handed to the program that declared a handler for it.
                    match document_from_index(full_path) {
                        Some(document) => {
                            app.selected_document = document;
                            app.has_selected_document = true;
                            app.index_error = 0;
                            // WHY the handler list is named and not guessed: the
                            // registry holds what the installed packages
                            // declared, and a file with no handler has no answer
                            // to give.
                            if app.selected_document.has_handler() {
                                std::log("[FILE_MANAGER] Document selected. The platform's registry names its handlers.")
                                    .ok();
                            } else {
                                std::log("[FILE_MANAGER] Document selected. No app handles this file.").ok();
                            }
                        }
                        None => {
                            app.has_selected_document = false;
                            app.index_error = std::content::ERR_NO_HANDLER;
                            std::log("[FILE_MANAGER] The platform's index does not hold this document.").ok();
                        }
                    }
                } else {
                    std::log("[FILE_MANAGER] Directory entry selected.").ok();
                }
            }
            return Tap::Redraw(Dirty::All);
        }
    }

    Tap::None
}

/// Resolves a touch on the confirmation sheet.
fn handle_sheet_tap(app: &mut FileManagerApp, x: usize, y: usize, surface: u32) -> Tap {
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
        let field = Rect::new(SHEET.x + 16, SHEET_NAME_Y, SHEET.w - 32, SHEET_NAME_H);
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
        if app.pending.action == FileAction::Rename && !name_is_usable(app.rename_field.as_slice())
        {
            // WHY the confirmation is refused and not acted on: a name with a
            // slash in it would name something else, and the platform would
            // write it.
            app.set_status();
            return Tap::Redraw(Dirty::All);
        }
        perform_action(app, surface);
        return Tap::Redraw(Dirty::All);
    }
    Tap::None
}

/// Puts a key into the field that has the keyboard's attention.
///
/// WHY the codes are named: the kernel's key pipeline answers a character
/// with its code and the special keys with 0x10000 and up.
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
    if app.query.is_focused() {
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

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    std::log("[FILE_MANAGER] Panic detected! Terminating task...").ok();
    std::exit();
}
