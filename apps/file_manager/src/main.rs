// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

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
use std::design::accessibility::{DynamicType, UserTextScale};
use std::design::components;
use std::design::font;
use std::design::outline_text;
use std::design::primitives::{draw_pill, draw_rounded_rect, Rect};
use std::design::scroll::ScrollState;
pub use file_manager::{
    action_for_mime, detect_mime, fmt_u64, glyph_ink, has_suffix, parse_vfs_entries,
    document_from_index, document_from_path, FileMime, RuntimeFileEntry, SelectedDocument,
    MAX_DOCUMENT_PATH_LEN, MAX_ENTRY_NAME_LEN, MAX_PARSED_ENTRIES,
};
// The layout, the scroll geometry and the file model live in the library so
// the renderer and the hit test can call the same functions.
use file_manager::{
    action_for_slot, action_slot_rect, action_supported, apply_search_scope, breadcrumb_crumbs,
    build_details_with_provenance, category_row_at, control_label_lane, crumb_step,
    entry_at_point, entry_row_rect, fit_details, format_mtime,
    format_size_human, grid_at_point, grid_cell_rect, grid_scroll, join_path, list_scroll,
    parse_indexed_documents, problem_text, row_is_visible, search_needle_is_usable,
    sheet_button_rect, sheet_destination_rect, sort_documents, sort_entries, sort_recents,
    validate_operation, ActionTapOutcome, Crumb, DetailsView, evaluate_action_tap,
    FileAction,
    IndexedDocument, LayoutMode, MAX_DETAILS_LINES, MAX_PATH_LEN, MAX_SEARCH_RESULTS, OpProblem,
    SearchScope, Selection, SortMode, TypedField, ViewMode, ACTION_BAR_Y0, ACTION_BAR_Y1,
    ACTION_LABELS, BREADCRUMB_SCALE,
    CATEGORY_CAPTION_SCALE, CATEGORY_COUNT, CATEGORY_FIRST_LABEL_DY, CATEGORY_LABEL_LANE,
    CATEGORY_LABEL_SCALE, CATEGORY_LABEL_X, CATEGORY_OTHER_LABEL_DY, CATEGORY_ROW_X1,
    CRUMB_ROW_X0, CRUMB_ROW_X1,
    CRUMB_SEP, DETAILS_INSET,
    DETAILS_LINE_SCALE, DETAILS_TITLE_SCALE, EMPTY_CARD_LANE, EMPTY_HEADLINE_SCALE,
    EMPTY_SUBLINE_SCALE, ENTRY_GLYPH_X, ENTRY_NAME_LANE, ENTRY_NAME_SCALE,
    ENTRY_RIGHT_AIR, ENTRY_SUB_SCALE, ENTRY_TEXT_X,
    fitted_run, GRID_CELL_W, GRID_NAME_LANE, GRID_NAME_SCALE,
    GRID_TEXT_INSET, HEADER_BOTTOM_AIR, HEADER_H, HEADER_READOUT_LINE2_Y,
    HEADER_READOUT_MAX_W, HEADER_READOUT_ROWS, HEADER_READOUT_SCALE, HEADER_TITLE_MAX_W,
    HEADER_TITLE_ROWS, HEADER_TITLE_SCALE, HEADER_TITLE_Y,
    HOME_CARD_LINE_SCALE, HOME_CARD_TITLE_SCALE,
    HOME_RECENT_NAME_DY, HOME_RECENT_NAME_SCALE,
    HOME_RECENTS_VISIBLE,
    HOME_ROW_X, HOME_ROW_W, LABEL_RUNG, LINE_LEN,
    PANEL_CONTENT_X1,
    SCOPE_CAPTION_MAX_W, SCOPE_CAPTION_SCALE, SCOPE_TEXT_X,
    SEARCH_TEXT_LANE, SEARCH_TEXT_SCALE, SEARCH_TEXT_X,
    SHEET, SHEET_NAME_LIST_SCALE, SHEET_TITLE_SCALE,
    STATUS_LANE, STATUS_SCALE, STATUS_TEXT_X,
    // The same boxes, derived at the rung the person chose. Every paint site and
    // every hit test below asks one of these, so the rectangle the app inks is
    // literally the expression the app answers a press with. The authored names
    // above are the ones a press is never measured against: the flat bands
    // (`ACTION_BAR_Y0..Y1`) that the ladder does not move, the insets and lanes a
    // run is trimmed to, and the scales themselves.
    cap_at, category_caption_dy, category_y0,
    category_y1, crumb_hit, crumb_rows, crumb_y, drawn_w, draw_level, empty_headline_y,
    empty_subline_y, ellipsize_run, entry_glyph_dy, entry_row_h_max, entry_search_name_dy,
    entry_search_path_dy, entries_card,
    entries_card_at, grid_cell_h_max, grid_glyph_dy, grown,
    GLYPH_COLS, GLYPH_ROWS, GRID_GLYPH_X,
    header_h, header_readout_line1_y, header_readout_line2_y,
    header_title_y, home_browse_btn, home_card_line1_y,
    home_card_line2_y, home_card_title_y, home_category_title_y, home_note_y, home_recent_row_h,
    home_recent_time_dy, home_recents_caption_y, home_recents_note_dy, home_recents_title_y,
    home_recents_y0,
    home_storage_card, PLATFORM_LEVEL, rows_at, run_line, run_rows, run_w, search_clear,
    search_field, search_glyph_dy,
    scope_all_pill, scope_this_pill, scope_row_y1, scope_y, sheet,
    sheet_name_h, sheet_name_line_y, sheet_name_y, sheet_title_y, status_rows,
    status_y, details_card, details_close, details_line_y, details_open,
    details_title_y, toolbar_pill,
};

/// The platform's list-row treatment, at the two states the file list uses.
///
/// WHY the file list is the platform's list row: a row that invents its own
/// fill, its own divider and its own padding is a row a third-party app
/// cannot match, and the platform already says what a row looks like.
const ROW: components::ListRowStyle = components::list_row_style(&THEME, false);
const ROW_SELECTED: components::ListRowStyle = components::list_row_style(&THEME, true);

/// The hairline a secondary control is outlined with.
///
/// WHY a control needs one at all: `draw_rounded_rect` with `border: None`
/// paints fill only, so a control whose fill is within a hair of the surface
/// behind it has no visible edge and the label reads as text floating on the
/// panel. The tap target exists -- it is the same rectangle the hit test
/// uses -- the person just cannot see it.
///
/// WHY `outline` and not `border`: `border` / `outline_variant` is the
/// `#E5E5E5` hairline that separates two surfaces, 1.21:1 against the page and
/// 1.08:1 against the raised sheet -- it separates, it does not say "press me".
/// `outline` is the theme's strong hairline (`#0A0A0A`, 18.97:1 on the page,
/// 17.04:1 on the sheet), and a boundary a person can see is exactly what a
/// control's edge is for. It is a role the theme already names, so no colour
/// and no new value enters the app.
const CONTROL_OUTLINE: u32 = THEME.outline;

/// The fill of a secondary control that sits on the raised `#EFEEE9` card or
/// sheet -- the platform's own resting white.
///
/// WHY not the app's usual chip fill: every other chip here stands on the
/// `#FAFAFA` page, where `surface_container_high` is a darker step and reads.
/// Inside a `Raised` card the same token *is* the card's fill, so the control
/// paints itself out completely. `surface_container` is the one role that
/// differs from the sheet in the direction the platform already uses for a
/// surface resting on a surface.
const RAISED_CONTROL_FILL: u32 = THEME.surface_container;

const W: usize = 720;
const H: usize = 1440;
const CHUNK: usize = 4096;

/// The platform's own inset at the top of every screen.
///
/// WHY the app asks the platform for it rather than writing 48: the number is
/// the shell's, and a bar that retypes it keeps the old number the day the
/// inset moves.
const STATUS_BAR_HEIGHT: usize = std::design::viewport::STATUS_BAR_HEIGHT;

/// The clock lane's text: the RTC's `HH:MM`, or blanks when it did not answer.
const CLOCK_LEN: usize = 5;

/// The details card's handler line, and the notice shown when the registry
/// answers that no installed package declared the document's type.
const OPEN_WITH_LABEL: &[u8] = b"Open with: ";
const NO_HANDLER_NOTICE: &[u8] = b"no app handles this file";
/// Shown when the registry named a handler but the platform refused to launch
/// it. Saying "no app handles this file" there would be false.
const HANDLER_REFUSED_NOTICE: &[u8] = b"the app for this file could not launch";

// -----------------------------------------------------------------------------
// Header geometry - single source of truth for drawing and hit-testing
//
// WHY the band's rows live in the library: the header's height comes off the
// type ladder, and the browse stack, the home card and the tap handler all start
// on its foot. A band derived in one place and re-typed in another is a band
// that clips the day a rung moves. What stays here is only what this file's
// pixel function owns: the title's lane and the readout's lane.
// -----------------------------------------------------------------------------
/// Screen margin (8pt grid value): the right inset the storage readout is
/// aligned to.
///
/// WHY it no longer also places the back control: the control's inset is the
/// platform's, not the app's, and an app that re-declares it is an app that
/// drifts the day the SDK moves the control. The control now comes from
/// [`std::design::back_button`] and answers for itself.
const HEADER_MARGIN: usize = spacing::GRID_24;

/// The x the title starts on: the control's right edge plus the platform's
/// 16px gap, the same expression every app that puts a title beside this
/// control uses.
const TITLE_X: usize =
    std::design::back_button::X + std::design::back_button::SIZE + spacing::GRID_16;

// The title's lane width is [`HEADER_TITLE_MAX_W`] in the library: this branch
// clips a run only at its left edge and the panel's right edge, so that number
// is the only thing between a long title and the readout's lane, and the host
// test beside it is what measures `b"Files"` at [`HEADER_TITLE_SCALE`] against
// it. `font::measure` is not a `const fn`, so the width cannot be checked here.

/// WHY checked at compile time: the title and the back control are one row, and
/// the row has two owners -- this gate is the claim that they never claim the
/// same pixel, which is the whole reason the title's lane can then be measured
/// against the readout's lane beside it. The rows the band itself answers are
/// checked in the library, because the browse stack and the home card start on
/// its foot and only the model sees both.
const _: () = assert!(
    TITLE_X >= std::design::back_button::X + std::design::back_button::SIZE,
    "the title must start clear of the back control's right edge"
);

/// The system home indicator, and the strip that contains it.
///
/// Both come from the platform's own declaration: the mark this app paints and
/// the box that answers a press are then one fact, not two, and the press box is
/// the touch floor rather than the six rows of ink.
const HOME_HIT: Rect = std::design::home_band::HIT;
const HOME_PILL: Rect = std::design::home_band::PILL;

/// Right edge the header readout is aligned to (the 24px right margin).
const READOUT_X1: usize = W - HEADER_MARGIN;

/// WHY also checked here: the readout's two lines are painted by this file's
/// pixel function and the branch that answers them stops at `HEADER_H`, so the
/// lowest row this branch reaches must be a row the band answers. The band is
/// built from the same union boxes beside [`HEADER_H`]; this gate is the copy
/// that fails here, at the draw site, if the two ever disagree.
const _: () = assert!(
    HEADER_READOUT_LINE2_Y + HEADER_READOUT_ROWS <= HEADER_H,
    "the header branch must answer every row the readout's second line reserves"
);

/// WHY also checked: the title's row and the band's own air are the two figures
/// a rung move changes first. Both come off the same rungs the band is built
/// from, so a rung that outgrew this header fails here, at the draw site, rather
/// than painting over the browse stack that starts on `HEADER_H`.
const _: () = assert!(
    HEADER_TITLE_Y + HEADER_TITLE_ROWS <= HEADER_H,
    "the title's union box must stand entirely inside the rows the header answers"
);
/// WHY this one repeats the band's arithmetic instead of trusting it: the branch
/// that paints the readout stops at `HEADER_H`, so the last row it may ink plus
/// the air under it has to be inside the band by this file's own numbers too.
const _: () = assert!(
    HEADER_H >= HEADER_READOUT_LINE2_Y + HEADER_READOUT_ROWS + HEADER_BOTTOM_AIR,
    "the header must keep its full air below the last row its readout reserves"
);

/// The readout's lane width is [`HEADER_READOUT_MAX_W`] in the library, and the
/// title's is [`HEADER_TITLE_MAX_W`]: `ellipsize_run` trims to the first here
/// while the host test measures the trimmed run against it there, and two copies
/// of one lane is how a header comes to reserve room at one rung and measure its
/// runs at another. This gate is what keeps the trim honest about *why* it
/// trims -- the run's left edge stays right of the title's reserved box, so the
/// path line and the item-count line clear both the back control and the title at
/// whatever position the SDK gives them.
const _: () = assert!(
    READOUT_X1 - HEADER_READOUT_MAX_W >= TITLE_X + HEADER_TITLE_MAX_W,
    "the header readout's lane must start right of the title's reserved box"
);

/// Ink for an affordance that is painted but has nothing behind it.
#[allow(dead_code)]
const DISABLED: u32 = THEME.border;

// -----------------------------------------------------------------------------
// Rasterisers
// -----------------------------------------------------------------------------

#[inline(always)]
fn centered_x(text: &[u8], scale: usize, x0: usize, available: usize) -> usize {
    let w = drawn_w(text, scale, available);
    x0 + if w >= available { 0 } else { (available - w) / 2 }
}

// -----------------------------------------------------------------------------
// The person's text size, and the rung this app draws at
//
// WHY this block is in the binary and not the library: publishing is the app's
// job -- the library only answers the cell the renderer was handed. Half of
// adopting Settings' "Text size" is reading `display.text_scale` and handing the
// level to the renderer; the other half is drawing through the helpers that read
// it back. Both halves are this app's own, and a surface that did only the first
// would resize nothing, while one that did only the second would be stuck at one
// hundred per cent forever.
// -----------------------------------------------------------------------------

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
    /// The status band's clock lane, as the RTC answered it.
    ///
    /// WHY it is cached on the app: the read is a syscall and the band is
    /// rasterised per pixel, so a band that asked per pixel would ask 34,560
    /// times a frame. The loop reads it once per tick and hands the bytes down.
    pub clock: [u8; CLOCK_LEN],
    /// Whether the RTC answered at all. When it did not, the lane stays blank.
    pub clock_ok: bool,
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
            scroll: ScrollState::new(0, entries_card_at(PLATFORM_LEVEL).h, 0),
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
            clock: [b' '; CLOCK_LEN],
            clock_ok: false,
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
            grid_scroll(count, entries_card(), offset)
        } else {
            list_scroll(count, entries_card(), offset)
        }
    }

    /// Re-solve the listing's scroll state after the platform's rung moved.
    ///
    /// WHY it is a call of its own: the card's height and every row inside it are
    /// derived from the rung this process draws at, so an extent solved before the
    /// rung moved bounds a screen that no longer exists -- a thumb sized against
    /// the card the person left, and an offset past the new bottom no tap can
    /// reach back.
    pub fn relayout_for_rung(&mut self) {
        self.scroll = self.scroll_for(self.scroll.offset);
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
        let count = if action == FileAction::NewFolder {
            0
        } else {
            self.action_rows(&mut rows)
        };
        if action != FileAction::NewFolder {
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
        }
        self.notice_len = 0;
        self.pending = PendingAction {
            action,
            items: rows,
            item_count: count,
        };
        self.has_pending = true;

        self.sheet_title = [0u8; 64];
        if action == FileAction::NewFolder {
            let title = b"New folder?";
            let len = title.len().min(self.sheet_title.len());
            self.sheet_title[..len].copy_from_slice(&title[..len]);
            self.sheet_title_len = len;
        } else {
            let mut len = append(&mut self.sheet_title, 0, ACTION_LABELS[action as usize]);
            len = append(&mut self.sheet_title, len, b" ");
            len = append_u64(&mut self.sheet_title, len, count as u64);
            len = append(
                &mut self.sheet_title,
                len,
                if count == 1 { b" file?" } else { b" files?" },
            );
            self.sheet_title_len = len;
        }

        // The names of the files the action will touch, and only those: the
        // sheet holds three lines, and a list of forty names would not fit.
        self.sheet_name_count = 0;
        if action != FileAction::NewFolder {
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
        } else if action == FileAction::NewFolder {
            self.rename_field.clear();
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
// The ink itself is `file_manager::glyph_ink`, beside the box it fills, so the
// shapes a person reads in a list row, a grid cell and a recents row are one
// testable answer rather than four copies of a predicate.
// -----------------------------------------------------------------------------

// -----------------------------------------------------------------------------
// The frame: what this pass paints, answered once
//
// WHY the answers are hoisted: the renderer is asked for every pixel of the
// surface, and the question it is asked -- "what colour is (x, y)" -- has only a
// couple of dozen distinct answers per pass, all of them fixed the moment the
// pass starts. Asked per pixel, each row of air around a label pays for the
// ladder derivation that places it and the trim that cuts it. Measured on the
// guest, one `header_h()` costs 2.6 µs, one storage-card rect 5.6 µs and one
// `ellipsize_run` 6.4 µs, because every one of them walks the rung through
// several integer divisions and QEMU's TCG turns each division into a helper
// call. Once per pass, the same work is paid 20-odd times instead of a million,
// and a pixel is a handful of integer compares.
//
// WHY the placement travels with the run: [`outline_text::OutlineRun`] carries
// its own key and re-places its coverage if the pool has evicted it, so a handle
// held across the paint loses speed and never loses ink. The trimmed bytes
// travel beside it for the one case the outline face refuses -- a run longer
// than the pool's key, or wider than its box -- which then still has to reach the
// bitmap face.
// -----------------------------------------------------------------------------

/// The longest run a single site can hold: the header's path readout, which is
/// never longer than the model's own path buffer.
const RUN_BUF_MAX: usize = MAX_PATH_LEN;

#[derive(Clone, Copy)]
struct PlacedRun {
    /// The bytes the run draws, after the lane trim.
    buf: [u8; RUN_BUF_MAX],
    len: usize,
    x0: usize,
    y0: usize,
    scale: usize,
    /// One past the last row this run can ink: `y0 + rows_at(scale, level)`, held
    /// so a pixel standing on a row the run clears costs one compare and no
    /// lookups.
    ///
    /// WHY the band is the ladder's `rows_at` and not [`outline_text::row_band`]:
    /// the library's identity gates say the two are one number at the rung the
    /// pass draws on, and the band is the union of the outline line box and the
    /// bitmap cell, so neither face can ink a row outside it.
    y_end: usize,
    /// The cap the bitmap face is asked for, above the authored rung.
    cap: u32,
    /// The outline face's placement, when the face took this run.
    run: Option<outline_text::OutlineRun>,
    ink: u32,
    bg: u32,
}

impl PlacedRun {
    /// A site the pass has nothing to draw: no bytes, so no ink.
    const EMPTY: PlacedRun = PlacedRun {
        buf: [0u8; RUN_BUF_MAX],
        len: 0,
        x0: 0,
        y0: 0,
        scale: 0,
        y_end: 0,
        cap: 0,
        run: None,
        ink: 0,
        bg: 0,
    };
}

/// Resolves one line of type for the pass: its box, its colours and its place
/// in the outline pool.
fn place(x0: usize, y0: usize, scale: usize, text: &[u8], ink: u32, bg: u32) -> PlacedRun {
    let lvl = draw_level();
    let cap = cap_at(scale, lvl);
    let mut buf = [0u8; RUN_BUF_MAX];
    let len = text.len().min(RUN_BUF_MAX);
    buf[..len].copy_from_slice(&text[..len]);
    let run = if len == 0 || scale == 0 {
        None
    } else if lvl == PLATFORM_LEVEL {
        outline_text::place_run(x0, y0, scale, &buf[..len], None, false)
    } else {
        outline_text::place_run_cap(x0, y0, cap, &buf[..len], None, false)
    };
    PlacedRun {
        buf,
        len,
        x0,
        y0,
        scale,
        y_end: y0 + rows_at(scale, lvl),
        cap,
        run,
        ink,
        bg,
    }
}

impl PlacedRun {
    /// The colour of (`x`, `y`) if this run inks it.
    ///
    /// WHY the band is the first thing asked: a pixel that stands clear of the
    /// run's rows is refused by one compare, and the placement pool is never
    /// touched for it.
    #[inline(always)]
    fn pixel(&self, x: usize, y: usize) -> Option<u32> {
        if self.len == 0 || x < self.x0 || y < self.y0 || y >= self.y_end {
            return None;
        }
        if let Some(run) = self.run {
            let (rx, rtop, rw, rh) = run.bounds();
            if x >= rx && x - rx < rw && y >= rtop && y - rtop < rh {
                return run.pixel(x, y, self.ink, self.bg);
            }
            return None;
        }
        self.bitmap_pixel(x, y)
    }

    /// The same question, asked of the bitmap face the outline pool refused.
    ///
    /// WHY it is kept out of line: the pool takes every run this screen holds,
    /// so this is the branch a frame reaches never or once, and inlining it
    /// would grow the hot guard for no pixel's benefit. The band was tested by
    /// the caller and is not tested again here.
    #[inline(never)]
    fn bitmap_pixel(&self, x: usize, y: usize) -> Option<u32> {
        let text = &self.buf[..self.len];
        if draw_level() == PLATFORM_LEVEL {
            return font::test_text_aa(
                x,
                y,
                self.x0,
                self.y0,
                self.scale,
                text,
                self.ink,
                self.bg,
                false,
            );
        }
        std::design::monochrome::text_px(
            x, y, self.x0, self.y0, self.cap, text, self.ink, self.bg, false,
        )
    }
}

/// The header: its height, the title and the two readout lines.
#[derive(Clone, Copy)]
struct HeaderFrame {
    h: usize,
    title: PlacedRun,
    line1: PlacedRun,
    line2: PlacedRun,
}

/// One recent row, as the pass draws it.
#[derive(Clone, Copy)]
struct RecentRow {
    top: usize,
    bottom: usize,
    divider_y: usize,
    /// The first of the 26 rows the file-type glyph's plate stands on.
    glyph_top: usize,
    mime: FileMime,
    name: PlacedRun,
    time: PlacedRun,
}

impl RecentRow {
    const EMPTY: RecentRow = RecentRow {
        top: 0,
        bottom: 0,
        divider_y: 0,
        glyph_top: 0,
        mime: FileMime::Directory,
        name: PlacedRun::EMPTY,
        time: PlacedRun::EMPTY,
    };
}

/// One directory row, as the pass draws it.
#[derive(Clone, Copy)]
struct CategoryRow {
    top: usize,
    height: usize,
    rect: Rect,
    fill: u32,
    selected: bool,
    /// The first row of the 26 the glyph's plate stands on.
    glyph_top: usize,
    mime: FileMime,
    label: PlacedRun,
    caption: PlacedRun,
}

impl CategoryRow {
    const EMPTY: CategoryRow = CategoryRow {
        top: 0,
        height: 0,
        rect: Rect::new(0, 0, 0, 0),
        fill: 0,
        selected: false,
        glyph_top: 0,
        mime: FileMime::Directory,
        label: PlacedRun::EMPTY,
        caption: PlacedRun::EMPTY,
    };
}

/// The home screen's layout, resolved once for the pass.
#[derive(Clone, Copy)]
struct HomeFrame {
    card: Rect,
    card_radius: usize,
    card_fill: u32,
    card_border: Option<u32>,
    card_title: PlacedRun,
    card_line1: PlacedRun,
    card_line2: PlacedRun,
    recents_title: PlacedRun,
    recents_caption: PlacedRun,
    /// The notice that stands in for the list when the index refused or is empty.
    recents_note: PlacedRun,
    recent_rows: usize,
    recents: [RecentRow; HOME_RECENTS_VISIBLE],
    category_title: PlacedRun,
    category_y0: usize,
    category_y1: usize,
    category_rows: usize,
    categories: [CategoryRow; CATEGORY_COUNT],
    browse: Rect,
    browse_label: PlacedRun,
    note1: PlacedRun,
    note2: PlacedRun,
}

/// A control the pass paints with `draw_pill`: its plate, its state's colours,
/// and the run centred on the plate.
#[derive(Clone, Copy)]
struct Pill {
    rect: Rect,
    fill: u32,
    ring: Option<u32>,
    label: PlacedRun,
}

impl Pill {
    /// This control's pixel, or `None` for a pixel it does not answer.
    ///
    /// The three answers are the ones the per-pixel painter gave: the label's ink
    /// where it reaches, the plate across the rest of the box, and nothing at all
    /// for a pixel inside the box but outside its corner arc, which falls through
    /// to whatever stands behind the control.
    #[inline(always)]
    fn pixel(&self, x: usize, y: usize) -> Option<u32> {
        if !self.rect.contains(x, y) {
            return None;
        }
        let base = draw_pill(x, y, self.rect, self.fill, self.ring)?;
        Some(self.label.pixel(x, y).unwrap_or(base))
    }

    /// A scope or toolbar chip.
    ///
    /// WHY the ring is the strong outline and not `THEME.border`: `border` is
    /// #E5E5E5, 1.08:1 against this chip's own #EFEEE9 fill and 1.21:1 against the
    /// #FAFAFA page it stands on, so the edge said nothing and the chip itself was
    /// only 1.11:1 off the page -- an invisible control with a label on it.
    /// `outline` is #0A0A0A: 17.04:1 on the chip and 18.97:1 on the page, the same
    /// role the platform's own back chip takes for the same reason. A selected pill
    /// is the page's opposite (#0A0A0A, 18.97:1 on the page), so it needs no edge.
    fn chip(rect: Rect, label: &[u8], selected: bool) -> Pill {
        let (fill, ink, ring) = if selected {
            (THEME.inverted_surface(), THEME.on_inverted_surface(), None)
        } else {
            (
                THEME.surface_container_high,
                THEME.on_surface,
                Some(CONTROL_OUTLINE),
            )
        };
        Pill {
            rect,
            fill,
            ring,
            label: plate_label(label, rect, ink, fill),
        }
    }

    /// An action bar slot, whose third state -- the one a person cannot take --
    /// keeps a boundary that still reads as quiet: the muted ink, 5.11:1 against
    /// the page it sits on, because a disabled slot's fill *is* the page.
    fn action(app: &FileManagerApp, slot: usize) -> Pill {
        let rect = action_slot_rect(slot);
        let (label, enabled, selected) = action_slot_state(app, slot);
        let (fill, ink, ring) = if selected {
            (THEME.inverted_surface(), THEME.on_inverted_surface(), None)
        } else if enabled {
            (
                THEME.surface_container_high,
                THEME.on_surface,
                Some(CONTROL_OUTLINE),
            )
        } else {
            (THEME.surface, THEME.muted, Some(THEME.muted))
        };
        Pill {
            rect,
            fill,
            ring,
            label: plate_label(label, rect, ink, fill),
        }
    }
}

/// A control the pass paints with `draw_rounded_rect` on top of a card: its plate,
/// its colours, and the run on the plate.
#[derive(Clone, Copy)]
struct Button {
    rect: Rect,
    radius: usize,
    fill: u32,
    border: Option<u32>,
    border_w: usize,
    label: PlacedRun,
}

impl Button {
    const EMPTY: Button = Button {
        rect: Rect::new(0, 0, 0, 0),
        radius: 0,
        fill: 0,
        border: None,
        border_w: 0,
        label: PlacedRun::EMPTY,
    };

    /// This control's pixel over the card's own `base`, and `None` outside its box.
    ///
    /// A control the state refuses is `EMPTY`, so its box answers no pixel and the
    /// card shows where it would have stood; a pixel inside the box but outside its
    /// corner arc answers the card, which is what the painter answered once the
    /// control's own plate refused.
    #[inline(always)]
    fn pixel(&self, x: usize, y: usize, base: u32) -> Option<u32> {
        if !self.rect.contains(x, y) {
            return None;
        }
        let Some(plate) =
            draw_rounded_rect(x, y, self.rect, self.radius, self.fill, self.border, self.border_w)
        else {
            return Some(base);
        };
        Some(self.label.pixel(x, y).unwrap_or(plate))
    }

    /// A button standing on a `Raised` card.
    ///
    /// WHY the primary one has no edge: near-black on the raised card is 17.04:1 by
    /// itself. WHY the secondary one moved off `surface_container_high`: the card is
    /// painted with that very token, so the button was #EFEEE9 on #EFEEE9 -- 1.00:1,
    /// borderless, and only its word on screen. It takes the platform's resting white
    /// (1.16:1 against the card) and the theme's strong outline (17.04:1).
    fn on_card(rect: Rect, label: &[u8], primary: bool) -> Button {
        let (fill, border, ink) = if primary {
            (THEME.primary, None, THEME.on_primary)
        } else {
            (
                RAISED_CONTROL_FILL,
                Some(CONTROL_OUTLINE),
                THEME.on_surface,
            )
        };
        Button {
            rect,
            radius: THEME.radii.control,
            fill,
            border,
            border_w: usize::from(border.is_some()),
            label: plate_label(label, rect, ink, fill),
        }
    }
}

/// A control's label, cut to the plate it stands on and centred inside it.
///
/// WHY once for the pass rather than once per pixel: `fitted_run` measures a run one
/// advance at a time and `centered_x` asks the ladder what it just measured, so every
/// pixel standing anywhere on the control re-measured the whole word it could not
/// even be on.
fn plate_label(label: &[u8], rect: Rect, ink: u32, bg: u32) -> PlacedRun {
    let mut buf = [0u8; LINE_LEN];
    let shown = fitted_run(label, LABEL_RUNG, control_label_lane(rect.w), &mut buf);
    let x = centered_x(shown, LABEL_RUNG, rect.x, rect.w);
    let y = rect.y + rect.h.saturating_sub(run_rows(LABEL_RUNG)) / 2;
    place(x, y, LABEL_RUNG, shown, ink, bg)
}

/// One breadcrumb: its run, and the separator that follows it in its own ink.
#[derive(Clone, Copy)]
struct CrumbInk {
    run: PlacedRun,
    sep: PlacedRun,
}

impl CrumbInk {
    const EMPTY: CrumbInk = CrumbInk {
        run: PlacedRun::EMPTY,
        sep: PlacedRun::EMPTY,
    };
}

/// The details card, as the pass draws it.
#[derive(Clone, Copy)]
struct DetailsFrame {
    rect: Rect,
    radius: usize,
    fill: u32,
    border: Option<u32>,
    title: PlacedRun,
    line_count: usize,
    lines: [PlacedRun; MAX_DETAILS_LINES],
    open: Button,
    close: Button,
}

/// The confirmation sheet, as the pass draws it.
#[derive(Clone, Copy)]
struct SheetFrame {
    rect: Rect,
    radius: usize,
    fill: u32,
    border: Option<u32>,
    title: PlacedRun,
    name_count: usize,
    names: [PlacedRun; 3],
    destinations: [Button; CATEGORY_COUNT],
    name_field: Button,
    cancel: Button,
    confirm: Button,
}

/// The browse screen, as the pass draws it.
#[derive(Clone, Copy)]
struct BrowseFrame {
    field: Rect,
    field_radius: usize,
    field_fill: u32,
    magnifier_x: usize,
    magnifier_y: usize,
    clear: Rect,
    clear_x: usize,
    clear_y: usize,
    show_clear: bool,
    query: PlacedRun,
    scope_y0: usize,
    scope_y1: usize,
    /// Where the caption's own column ends: the first scope pill's left edge.
    scope_x1: usize,
    scope_caption: PlacedRun,
    scope_pills: [Pill; 2],
    crumb_y0: usize,
    crumb_y1: usize,
    crumb_count: usize,
    crumbs: [CrumbInk; 6],
    toolbar: [Pill; 3],
    card: Rect,
    card_radius: usize,
    card_fill: u32,
    card_border: Option<u32>,
    empty: bool,
    /// The row the folder pictogram above the empty-card headline stands on.
    glyph_y: isize,
    headline: PlacedRun,
    subline: PlacedRun,
    status_y0: usize,
    status_y1: usize,
    status: PlacedRun,
    slots: [Pill; 7],
    details: Option<DetailsFrame>,
    sheet: Option<SheetFrame>,
}

/// The screen below the header, with the layout it draws already resolved.
#[derive(Clone, Copy)]
enum Screen {
    Home(HomeFrame),
    Browse(BrowseFrame),
}

/// The pass's answers, from the header down.
#[derive(Clone, Copy)]
struct Frame {
    header: HeaderFrame,
    screen: Screen,
}

// WHY a budget and not a shrug: the frame carries every run the screen inks, and a
// run carries its own text, so this is the largest object an app stack ever holds.
// The task's user stack is 256 KiB; the pass getting near this line is the pass that
// needs its runs cut down, not the stack raised.
const _: () = assert!(core::mem::size_of::<Frame>() < 48 * 1024);

fn build_frame(app: &FileManagerApp) -> Frame {
    let title = place(
        TITLE_X,
        header_title_y(),
        HEADER_TITLE_SCALE,
        b"Files",
        THEME.on_surface,
        THEME.surface,
    );

    // The path the app asked, and the platform's own answer. Line 1 is the path
    // really passed to `fs_list`; line 2 is the real outcome, including a refusal.
    let mut path_buf = [0u8; MAX_PATH_LEN];
    let path_len = ellipsize_run(
        &app.current_path[..app.current_path_len],
        HEADER_READOUT_SCALE,
        HEADER_READOUT_MAX_W,
        &mut path_buf,
    );
    let path_w = run_w(&path_buf[..path_len], HEADER_READOUT_SCALE);
    let line1 = if path_len > 0 && path_w <= HEADER_READOUT_MAX_W {
        place(
            READOUT_X1 - path_w,
            header_readout_line1_y(),
            HEADER_READOUT_SCALE,
            &path_buf[..path_len],
            THEME.on_surface_variant,
            THEME.surface,
        )
    } else {
        PlacedRun::EMPTY
    };

    let mut label_buf = [0u8; 40];
    let label_len = ellipsize_run(
        &app.probe_label[..app.probe_label_len],
        HEADER_READOUT_SCALE,
        HEADER_READOUT_MAX_W,
        &mut label_buf,
    );
    let label_w = run_w(&label_buf[..label_len], HEADER_READOUT_SCALE);
    let line2 = if label_len > 0 && label_w <= HEADER_READOUT_MAX_W {
        place(
            READOUT_X1 - label_w,
            header_readout_line2_y(),
            HEADER_READOUT_SCALE,
            &label_buf[..label_len],
            THEME.warning,
            THEME.surface,
        )
    } else {
        PlacedRun::EMPTY
    };

    Frame {
        header: HeaderFrame {
            h: header_h(),
            title,
            line1,
            line2,
        },
        screen: match app.view {
            ViewMode::Home => Screen::Home(build_home_frame(app)),
            ViewMode::Browse => Screen::Browse(build_browse_frame(app)),
        },
    }
}

fn build_home_frame(app: &FileManagerApp) -> HomeFrame {
    // 1. The storage card.
    let style = components::card_style(&THEME, components::Elevation::Resting);
    let card_title = place(
        60,
        home_card_title_y(),
        HOME_CARD_TITLE_SCALE,
        b"Storage",
        THEME.on_surface,
        style.fill,
    );
    let card_line1 = place(
        60,
        home_card_line1_y(),
        HOME_CARD_LINE_SCALE,
        &app.storage_line[..app.storage_line_len],
        THEME.on_surface_variant,
        style.fill,
    );
    let card_line2 = place(
        60,
        home_card_line2_y(),
        HOME_CARD_LINE_SCALE,
        app.storage_status(),
        THEME.warning,
        style.fill,
    );

    // 2. The recents section.
    let recents_title = place(
        36,
        home_recents_title_y(),
        HOME_CARD_TITLE_SCALE,
        b"Recently modified",
        THEME.on_surface,
        THEME.surface,
    );
    let recents_caption = place(
        36,
        home_recents_caption_y(),
        HOME_CARD_LINE_SCALE,
        b"Newest first, from the platform's document index",
        THEME.on_surface_variant,
        THEME.surface,
    );
    let mut recents_note = PlacedRun::EMPTY;
    let mut recents = [RecentRow::EMPTY; HOME_RECENTS_VISIBLE];
    let mut recent_rows = 0usize;
    if app.recents_error != 0 || app.recents_count == 0 {
        let (line, ink): (&[u8], u32) = if app.recents_error != 0 {
            (
                b"The document index was refused; recents are unavailable",
                THEME.warning,
            )
        } else {
            (b"The document index holds no documents", THEME.on_surface_variant)
        };
        recents_note = place(
            HOME_ROW_X,
            home_recents_y0() + home_recents_note_dy(),
            LABEL_RUNG,
            line,
            ink,
            THEME.surface,
        );
    } else {
        let recent_row_h = home_recent_row_h();
        let stack_top = home_recents_y0();
        let visible = app.recents_count.min(HOME_RECENTS_VISIBLE);
        // The name column ends at the row's right edge; the 24px inset matches
        // the browse rows so a long name cannot reach the border.
        let max_w = 684 - 100 - 24;
        for index in 0..visible {
            let top = stack_top + index * recent_row_h;
            let name = &app.recents[index].name[..app.recents[index].name_len];
            let mut name_buf = [0u8; 48];
            let name_len = ellipsize_run(name, HOME_RECENT_NAME_SCALE, max_w, &mut name_buf);
            recents[index] = RecentRow {
                top,
                bottom: top + recent_row_h,
                divider_y: top + recent_row_h - 1,
                glyph_top: top + (recent_row_h - 25) / 2,
                mime: detect_mime(name, false),
                name: place(
                    100,
                    top + HOME_RECENT_NAME_DY,
                    HOME_RECENT_NAME_SCALE,
                    &name_buf[..name_len],
                    THEME.on_surface,
                    THEME.surface,
                ),
                time: place(
                    100,
                    top + home_recent_time_dy(),
                    HOME_CARD_LINE_SCALE,
                    &app.recent_time[index][..app.recent_time_len[index]],
                    THEME.on_surface_variant,
                    THEME.surface,
                ),
            };
        }
        recent_rows = visible;
    }

    // 3. The category section.
    let category_title = place(
        36,
        home_category_title_y(),
        HOME_CARD_TITLE_SCALE,
        b"Browse directories",
        THEME.on_surface,
        THEME.surface,
    );
    let mut categories = [CategoryRow::EMPTY; CATEGORY_COUNT];
    let mut category_rows = 0usize;
    {
        let mut y = category_y0();
        while category_rows < CATEGORY_COUNT {
            let Some((index, row_top, row_h)) = category_row_at(y) else {
                break;
            };
            let selected = app.selected_shortcut == index;
            let fill = if selected { ROW_SELECTED.fill } else { ROW.fill };
            let icon_center = if index == 0 { row_top + 44 } else { row_top + 35 };
            let mime = match index {
                0 => FileMime::Directory,
                1 | 2 => FileMime::Image,
                _ => FileMime::Video,
            };
            let label_y = if index == 0 {
                row_top + CATEGORY_FIRST_LABEL_DY
            } else {
                row_top + CATEGORY_OTHER_LABEL_DY
            };
            let mut label_buf = [0u8; LINE_LEN];
            let label = place(
                CATEGORY_LABEL_X,
                label_y,
                CATEGORY_LABEL_SCALE,
                fitted_run(
                    CATEGORY_LABELS[index],
                    CATEGORY_LABEL_SCALE,
                    CATEGORY_LABEL_LANE,
                    &mut label_buf,
                ),
                THEME.on_surface,
                fill,
            );
            let caption = if index == 0 {
                let mut status_buf = [0u8; LINE_LEN];
                place(
                    CATEGORY_LABEL_X,
                    row_top + category_caption_dy(),
                    CATEGORY_CAPTION_SCALE,
                    fitted_run(
                        app.storage_status(),
                        CATEGORY_CAPTION_SCALE,
                        CATEGORY_LABEL_LANE,
                        &mut status_buf,
                    ),
                    THEME.on_surface_variant,
                    fill,
                )
            } else {
                PlacedRun::EMPTY
            };
            categories[index] = CategoryRow {
                top: row_top,
                height: row_h,
                rect: Rect::new(HOME_ROW_X, row_top, HOME_ROW_W, row_h.saturating_sub(1)),
                fill,
                selected,
                glyph_top: icon_center - 13,
                mime,
                label,
                caption,
            };
            category_rows += 1;
            y = row_top + row_h;
        }
    }

    // 4. The browse-everything control.
    let browse = home_browse_btn();
    let ly = browse.y + browse.h.saturating_sub(run_rows(LABEL_RUNG)) / 2;
    let mut browse_buf = [0u8; LINE_LEN];
    let shown = fitted_run(
        b"Browse all files",
        LABEL_RUNG,
        control_label_lane(browse.w),
        &mut browse_buf,
    );
    let lx = centered_x(shown, LABEL_RUNG, browse.x, browse.w);
    let browse_label = place(
        lx,
        ly,
        LABEL_RUNG,
        shown,
        THEME.primary,
        THEME.surface_container_high,
    );

    // 5. The honest note.
    let note_y = home_note_y();
    let note1 = place(
        36,
        note_y,
        HOME_CARD_LINE_SCALE,
        b"Recents come from the platform's document index.",
        THEME.on_surface_variant,
        THEME.surface,
    );
    let note2 = place(
        36,
        note_y + grown(24, HOME_CARD_LINE_SCALE, draw_level()),
        HOME_CARD_LINE_SCALE,
        b"Files the index has not walked are not listed here.",
        THEME.on_surface_variant,
        THEME.surface,
    );

    HomeFrame {
        card: home_storage_card(),
        card_radius: style.radius,
        card_fill: style.fill,
        card_border: style.border,
        card_title,
        card_line1,
        card_line2,
        recents_title,
        recents_caption,
        recents_note,
        recent_rows,
        recents,
        category_title,
        category_y0: category_y0(),
        category_y1: category_y1(),
        category_rows,
        categories,
        browse,
        browse_label,
        note1,
        note2,
    }
}

/// The browse screen's layout, resolved once for the pass.
///
/// WHY the whole screen is resolved here and not in the pixel function: this chrome
/// is a score of boxes derived from the ladder -- the field, the scope row, the
/// breadcrumb strip, three toolbar chips, the card, the status line, seven action
/// slots -- and each of the ~20 trims and centring calls the painter used to make
/// *per pixel*. Under the emulator's interpreter a division is a helper call, so the
/// pass cost 54,692 guest ticks, 61 times the home screen's 889, and a tap on the
/// home screen's browse control took the better part of a minute to reach scanout.
/// What is left below is a compare per pixel and, on the pixels that hold ink, one
/// read of a run the pass already rasterised.
fn build_browse_frame(app: &FileManagerApp) -> BrowseFrame {
    // 1. The search field, its magnifier, its clear action and its needle.
    let field = search_field();
    let clear = search_clear();
    let has_query = !app.query.is_empty();
    let text = if has_query {
        app.query.as_slice()
    } else {
        b"Search this device's documents".as_slice()
    };
    let ink = if has_query {
        THEME.on_surface
    } else {
        THEME.on_surface_variant
    };
    // The needle is trimmed to the field's own lane -- the column it is typed in,
    // up to the air before the clear action -- so a long query stays inside the box
    // the tap handler answers instead of running under the control that clears it.
    let mut needle = [0u8; LINE_LEN];
    let shown = fitted_run(text, SEARCH_TEXT_SCALE, SEARCH_TEXT_LANE, &mut needle);
    let query = place(
        SEARCH_TEXT_X,
        field.y + grown(16, SEARCH_TEXT_SCALE, draw_level()),
        SEARCH_TEXT_SCALE,
        shown,
        ink,
        THEME.surface_container_high,
    );

    // 2. The search scope caption and its two pills.
    let scope_this = scope_this_pill();
    let mut scope_buf = [0u8; LINE_LEN];
    let scope_shown = fitted_run(
        &app.scope[..app.scope_len],
        SCOPE_CAPTION_SCALE,
        SCOPE_CAPTION_MAX_W,
        &mut scope_buf,
    );
    let scope_caption = place(
        SCOPE_TEXT_X,
        scope_y(),
        SCOPE_CAPTION_SCALE,
        scope_shown,
        THEME.on_surface_variant,
        THEME.surface,
    );

    // 3. The breadcrumbs. The painted rows are the whole union box the rungs
    // reserve, centred inside the touch strip beside them, so a crumb is answered
    // wherever its ink reaches -- and the strip is laid out by `crumb_step`, the
    // same function the tap handler walks, so a crumb is only inked where it can be
    // tapped and a run never straddles the strip's edge.
    let crumb_y0 = crumb_y();
    let crumb_rows = crumb_rows();
    let mut crumbs = [CrumbInk::EMPTY; 6];
    let mut crumb_count = 0usize;
    {
        let mut cursor = CRUMB_ROW_X0;
        for index in 0..app.crumb_count {
            let crumb = app.crumbs[index];
            let label =
                &app.current_path[crumb.label_start..crumb.label_start + crumb.label_len];
            let last = index + 1 == app.crumb_count;
            let mut buf = [0u8; MAX_PATH_LEN];
            let Some(step) = crumb_step(label, cursor, last, &mut buf) else {
                break;
            };
            let ink = if last {
                THEME.on_surface
            } else {
                THEME.primary
            };
            let run = place(
                step.x,
                crumb_y0,
                BREADCRUMB_SCALE,
                &buf[..step.len],
                ink,
                THEME.surface,
            );
            let sep = step
                .sep_x
                .map(|x| {
                    place(
                        x,
                        crumb_y0,
                        BREADCRUMB_SCALE,
                        CRUMB_SEP,
                        THEME.on_surface_variant,
                        THEME.surface,
                    )
                })
                .unwrap_or(PlacedRun::EMPTY);
            crumbs[index] = CrumbInk { run, sep };
            crumb_count += 1;
            cursor = step.next_x;
        }
    }

    // 5. The details card and 6. the confirmation sheet, each over the entries card.
    let details = app.has_details.then(|| build_details_frame(app));
    let sheet = app.has_pending.then(|| build_sheet_frame(app));

    // 7. The entries card, and the two runs that explain it when it holds nothing.
    let card = entries_card();
    let style = components::card_style(&THEME, components::Elevation::Resting);
    let empty = app.card_item_count() == 0;
    let (headline, subline, headline_ink): (&[u8], &[u8], u32) = if empty {
        if app.searching {
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
        }
    } else {
        (b"", b"", THEME.on_surface)
    };
    // The card's two runs are cut to the card's own lane before they are centred: an
    // explanation of why nothing is listed that leaves the card is worse than no
    // explanation, and the gate beside `EMPTY_CARD_LANE` is what says the platform's
    // own refusals fit.
    let empty_headline_y = empty_headline_y();
    let mut headline_buf = [0u8; LINE_LEN];
    let headline_shown =
        fitted_run(headline, EMPTY_HEADLINE_SCALE, EMPTY_CARD_LANE, &mut headline_buf);
    let headline = place(
        centered_x(headline_shown, EMPTY_HEADLINE_SCALE, card.x, card.w),
        empty_headline_y,
        EMPTY_HEADLINE_SCALE,
        headline_shown,
        headline_ink,
        style.fill,
    );
    let mut subline_buf = [0u8; LINE_LEN];
    let subline_shown =
        fitted_run(subline, EMPTY_SUBLINE_SCALE, EMPTY_CARD_LANE, &mut subline_buf);
    let subline = place(
        centered_x(subline_shown, EMPTY_SUBLINE_SCALE, card.x, card.w),
        empty_subline_y(),
        EMPTY_SUBLINE_SCALE,
        subline_shown,
        THEME.on_surface_variant,
        style.fill,
    );

    // 8. The status line.
    let mut status_buf = [0u8; LINE_LEN];
    let status_shown = if app.status_len > 0 {
        fitted_run(
            &app.status[..app.status_len],
            STATUS_SCALE,
            STATUS_LANE,
            &mut status_buf,
        )
    } else {
        b"".as_slice()
    };
    let status = place(
        STATUS_TEXT_X,
        status_y(),
        STATUS_SCALE,
        status_shown,
        THEME.on_surface_variant,
        THEME.surface,
    );

    BrowseFrame {
        field,
        field_radius: THEME.radii.control,
        field_fill: THEME.surface_container_high,
        magnifier_x: field.x + 28,
        magnifier_y: field.y + search_glyph_dy(),
        clear,
        clear_x: clear.x + clear.w / 2,
        clear_y: clear.y + clear.h / 2,
        show_clear: has_query,
        query,
        scope_y0: scope_y(),
        scope_y1: scope_row_y1(),
        scope_x1: scope_this.x,
        scope_caption,
        scope_pills: [
            Pill::chip(scope_this, SearchScope::ThisFolder.label(), app.search_scope == SearchScope::ThisFolder),
            Pill::chip(scope_all_pill(), SearchScope::AllStorage.label(), app.search_scope == SearchScope::AllStorage),
        ],
        crumb_y0,
        crumb_y1: crumb_y0 + crumb_rows,
        crumb_count,
        crumbs,
        toolbar: [
            Pill::chip(toolbar_pill(0), &app.sort_label[..app.sort_label_len], false),
            Pill::chip(toolbar_pill(1), LayoutMode::List.label(), app.layout == LayoutMode::List),
            Pill::chip(toolbar_pill(2), LayoutMode::Grid.label(), app.layout == LayoutMode::Grid),
        ],
        card,
        card_radius: style.radius,
        card_fill: style.fill,
        card_border: style.border,
        empty,
        glyph_y: empty_headline_y as isize - 53,
        headline,
        subline,
        status_y0: status_y(),
        status_y1: status_y() + status_rows(),
        status,
        slots: [
            Pill::action(app, 0),
            Pill::action(app, 1),
            Pill::action(app, 2),
            Pill::action(app, 3),
            Pill::action(app, 4),
            Pill::action(app, 5),
            Pill::action(app, 6),
        ],
        details,
        sheet,
    }
}

/// The details card, with the handler it names already decided.
fn build_details_frame(app: &FileManagerApp) -> DetailsFrame {
    let dcard = details_card();
    let style = components::card_style(&THEME, components::Elevation::Raised);
    let title = place(
        dcard.x + DETAILS_INSET,
        details_title_y(),
        DETAILS_TITLE_SCALE,
        app.details.title_str().as_bytes(),
        THEME.on_surface,
        style.fill,
    );
    let mut lines = [PlacedRun::EMPTY; MAX_DETAILS_LINES];
    for index in 0..app.details.line_count {
        lines[index] = place(
            dcard.x + DETAILS_INSET,
            details_line_y(index),
            DETAILS_LINE_SCALE,
            &app.details.lines[index][..app.details.line_lens[index]],
            if index == 3 && !app.details.has_handler {
                THEME.warning
            } else {
                THEME.on_surface_variant
            },
            style.fill,
        );
    }
    // WHY the button is absent rather than greyed out: a button that opens nothing
    // is a promise the device cannot keep. The card names the missing handler in its
    // own line, so the reason is still on the screen.
    let open_offered = app.details.is_dir || app.details.has_handler;
    DetailsFrame {
        rect: dcard,
        radius: style.radius,
        fill: style.fill,
        border: style.border,
        title,
        line_count: app.details.line_count,
        lines,
        open: open_offered.then(|| Button::on_card(details_open(), b"Open", true)).unwrap_or(Button::EMPTY),
        close: Button::on_card(details_close(), b"Close", false),
    }
}

/// The confirmation sheet, in the state the pending action puts it in.
fn build_sheet_frame(app: &FileManagerApp) -> SheetFrame {
    let sheet = sheet();
    let style = components::card_style(&THEME, components::Elevation::Raised);
    let title = place(
        sheet.x + 20,
        sheet_title_y(),
        SHEET_TITLE_SCALE,
        &app.sheet_title[..app.sheet_title_len],
        THEME.on_surface,
        style.fill,
    );
    let mut names = [PlacedRun::EMPTY; 3];
    for index in 0..app.sheet_name_count {
        names[index] = place(
            sheet.x + 20,
            sheet_name_line_y(index),
            SHEET_NAME_LIST_SCALE,
            &app.sheet_names[index][..app.sheet_name_lens[index]],
            THEME.on_surface_variant,
            style.fill,
        );
    }
    let choosing = app.pending.action == FileAction::Copy || app.pending.action == FileAction::Move;
    let destinations = if choosing {
        [
            Button::on_card(sheet_destination_rect(0), CATEGORY_LABELS[0], app.destination == 0),
            Button::on_card(sheet_destination_rect(1), CATEGORY_LABELS[1], app.destination == 1),
            Button::on_card(sheet_destination_rect(2), CATEGORY_LABELS[2], app.destination == 2),
            Button::on_card(sheet_destination_rect(3), CATEGORY_LABELS[3], app.destination == 3),
        ]
    } else {
        [Button::EMPTY; CATEGORY_COUNT]
    };
    // WHY the plate has an edge: its old fill was `surface_container_high`, which is
    // what the `Raised` sheet is painted with -- #EFEEE9 on #EFEEE9, 1.00:1, and no
    // border at all. The place to type a name had no bounds, and one of the two
    // states of this sheet (the state that asks you to edit text) was a bare word on
    // the panel. It takes the same treatment as the sheet's secondary buttons: the
    // platform's resting white (1.16:1 against the sheet) ringed with the theme's
    // strong outline (17.04:1 against the sheet), so the typed run reads at 18.97:1
    // instead of 17.04:1. The radius stays 0 and the run keeps its own inset: the
    // rectangle, the label's position and the tap target are the ones the sheet
    // already answers, and only the colours move.
    let renaming =
        app.pending.action == FileAction::Rename || app.pending.action == FileAction::NewFolder;
    let name_field = if renaming {
        let rect = Rect::new(SHEET.x + 20, sheet_name_y(), SHEET.w - 40, sheet_name_h());
        Button {
            rect,
            radius: 0,
            fill: RAISED_CONTROL_FILL,
            border: Some(CONTROL_OUTLINE),
            border_w: 1,
            label: place(
                rect.x + 12,
                rect.y + grown(12, SEARCH_TEXT_SCALE, draw_level()),
                SEARCH_TEXT_SCALE,
                app.rename_field.as_slice(),
                THEME.on_surface,
                RAISED_CONTROL_FILL,
            ),
        }
    } else {
        Button::EMPTY
    };
    SheetFrame {
        rect: sheet,
        radius: style.radius,
        fill: style.fill,
        border: style.border,
        title,
        name_count: app.sheet_name_count,
        names,
        destinations,
        name_field,
        cancel: Button::on_card(sheet_button_rect(false), b"Cancel", false),
        confirm: Button::on_card(sheet_button_rect(true), b"Confirm", true),
    }
}

// -----------------------------------------------------------------------------
// Pixel colour resolution
// -----------------------------------------------------------------------------

#[inline(never)]
fn get_file_manager_color(x: usize, y: usize, app: &FileManagerApp, frame: &Frame) -> u32 {
    // The platform's band owns the top inset; the app's own chrome starts below it.
    //
    // WHY the instruments are unmeasured: this app holds no radio, no fuel
    // gauge and no Wi-Fi association -- what it measures is the filesystem, and
    // that reading is on the storage card, not in the band. A filled bar here
    // would be a level no instrument of this app took.
    if y < STATUS_BAR_HEIGHT {
        let band = components::StatusBand::new(W);
        let clock = if app.clock_ok { Some(&app.clock[..]) } else { None };
        return band
            .pixel(x, y, clock, &components::Instruments::unmeasured(), &THEME)
            .unwrap_or(THEME.surface);
    }
    if y < frame.header.h {
        return header_color(x, y, &frame.header);
    }
    match &frame.screen {
        Screen::Home(home) => home_color(x, y, home),
        Screen::Browse(browse) => browse_color(x, y, browse),
    }
}

/// Reads the wall clock once and says whether the band's lane changed.
///
/// WHY it returns the change rather than redrawing: the loop repaints only what
/// actually moved, and a clock that answers the same minute costs no frame.
/// WHY an unanswered RTC leaves the lane blank: the read needs the
/// `RealtimeClockRead` capability, and a placeholder time is a measurement the
/// device never took.
fn refresh_clock(app: &mut FileManagerApp) -> bool {
    let mut clock = [b' '; CLOCK_LEN];
    let mut clock_ok = false;
    if let Some(now) = std::rtc::Reading::now() {
        clock = now.clock_text();
        clock_ok = true;
    }
    let changed = clock_ok != app.clock_ok || (clock_ok && clock != app.clock);
    app.clock = clock;
    app.clock_ok = clock_ok;
    changed
}

/// The header: the platform band above, then the back control, the title and the measured readout.
fn header_color(x: usize, y: usize, frame: &HeaderFrame) -> u32 {
    // The platform's back control, asked first because it is the one occupant of
    // this row the app does not own: its rect, radius, colours and chevron all
    // come from [`std::design::back_button`], which is what Settings, Calculator,
    // Dialer, Music, Vault, Camera and Gallery paint, so the control the user
    // finds in the top-left corner of every other screen is the control here.
    //
    // WHY the app's own pill rasteriser is gone: the chip it drew stood on the
    // row *under* the title, at the app's own inset and size, and the tap handler
    // tested that same hand-authored rectangle. It worked wherever it painted --
    // and nowhere in the corner the rest of the device answers.
    if let Some(color) = std::design::back_button::pixel_for_theme(x, y, &THEME) {
        return color;
    }

    // Title: "Files" on the control's row, starting clear of its right edge.
    //
    // WHY the coordinates arrive already placed: `TITLE_X` is derived from the
    // control's own rect and the title's row from the rung the title draws at, so
    // the row cannot be re-invented here, and the gates beside them are where the
    // header's claim -- that the control and the title both fit the rows this
    // branch answers and never claim one another's pixel -- is checked at compile
    // time.
    if let Some(color) = frame.title.pixel(x, y) {
        return color;
    }

    // The path the app asked, and the platform's own answer. Line 1 is the
    // path really passed to `fs_list`; line 2 is the real outcome, including
    // a refusal. Each is trimmed to its lane once, when the frame is built, and
    // a lane that the trim cannot fit leaves the site with nothing to ink.
    if let Some(color) = frame.line1.pixel(x, y) {
        return color;
    }
    if let Some(color) = frame.line2.pixel(x, y) {
        return color;
    }

    THEME.surface
}

/// The home screen: the storage insight, the index's recents and the four
/// directory entry points.
fn home_color(x: usize, y: usize, frame: &HomeFrame) -> u32 {
    // 1. The storage card.
    if frame.card.contains(x, y) {
        let Some(base) = draw_rounded_rect(
            x,
            y,
            frame.card,
            frame.card_radius,
            frame.card_fill,
            frame.card_border,
            1,
        ) else {
            return THEME.surface;
        };
        if let Some(color) = frame.card_title.pixel(x, y) {
            return color;
        }
        if let Some(color) = frame.card_line1.pixel(x, y) {
            return color;
        }
        if let Some(color) = frame.card_line2.pixel(x, y) {
            return color;
        }
        return base;
    }

    // 2. The recents section.
    if let Some(color) = frame.recents_title.pixel(x, y) {
        return color;
    }
    if let Some(color) = frame.recents_caption.pixel(x, y) {
        return color;
    }
    if let Some(color) = frame.recents_note.pixel(x, y) {
        return color;
    }

    for row in &frame.recents[..frame.recent_rows] {
        if y < row.top || y >= row.bottom || x < HOME_ROW_X || x >= PANEL_CONTENT_X1 {
            continue;
        }
        if y == row.divider_y {
            return ROW.divider;
        }
        // The glyph plate is 26 rows tall and starts where the row centres it, so
        // the mime walk is only asked on the rows its answer can reach -- and the
        // answer itself is taken once, when the row was laid out.
        if y >= row.glyph_top && y - row.glyph_top <= 25 {
            if let Some(ink) =
                glyph_ink(row.mime, x as isize - 47, y as isize - row.glyph_top as isize, THEME.on_surface)
            {
                return ink;
            }
        }
        if let Some(color) = row.name.pixel(x, y) {
            return color;
        }
        if let Some(color) = row.time.pixel(x, y) {
            return color;
        }
        return THEME.surface;
    }

    // 3. The category section.
    if let Some(color) = frame.category_title.pixel(x, y) {
        return color;
    }

    if (frame.category_y0..frame.category_y1).contains(&y)
        && (HOME_ROW_X..CATEGORY_ROW_X1).contains(&x)
    {
        for row in &frame.categories[..frame.category_rows] {
            if y < row.top || y >= row.top + row.height {
                continue;
            }
            let Some(base) = draw_rounded_rect(
                x,
                y,
                row.rect,
                ROW.radius,
                row.fill,
                Some(ROW.divider),
                1,
            ) else {
                return THEME.surface;
            };
            if row.selected && x < 44 {
                return THEME.primary;
            }
            if let Some(ink) = glyph_ink(
                row.mime,
                x as isize - 47,
                y as isize - row.glyph_top as isize,
                THEME.on_surface,
            ) {
                return ink;
            }
            // The row's own label lane: the row's right edge, less the column the
            // label stands in and the air it keeps there. The four directories are
            // the app's own words and they fit whole -- the gate beside
            // `CATEGORY_LABEL_LANE` measures them at `CATEGORY_LABEL_SCALE` and
            // refuses the day the row narrows under them -- while the storage
            // caption under the first row is the platform's sentence, so it is cut
            // with an ellipsis rather than run past the row it belongs to.
            if let Some(color) = row.label.pixel(x, y) {
                return color;
            }
            if let Some(color) = row.caption.pixel(x, y) {
                return color;
            }
            return base;
        }
        // The rows the stack reserves that no plate answered: the same surface
        // the stack's own height promised them.
        return THEME.surface;
    }

    // 4. The browse-everything control.
    if frame.browse.contains(x, y) {
        // WHY the outline: `border` (#E5E5E5) was 1.08:1 on this pill's own
        // #EFEEE9 fill and 1.21:1 on the #FAFAFA page, and the fill is only
        // 1.11:1 off the page, so the app's one entry-to-everything control was
        // a word floating on the page. `outline` (#0A0A0A) is 17.04:1 on the
        // pill and 18.97:1 on the page; the label stays 17.04:1.
        if let Some(base) = draw_pill(
            x,
            y,
            frame.browse,
            THEME.surface_container_high,
            Some(CONTROL_OUTLINE),
        ) {
            if let Some(color) = frame.browse_label.pixel(x, y) {
                return color;
            }
            return base;
        }
        return THEME.surface;
    }

    // 5. The honest note.
    if let Some(color) = frame.note1.pixel(x, y) {
        return color;
    }
    if let Some(color) = frame.note2.pixel(x, y) {
        return color;
    }

    // The home gesture pill, drawn from the rectangle the tap handler tests.
    if HOME_PILL.contains(x, y) {
        return THEME.on_surface_variant;
    }

    THEME.surface
}

/// The browse screen.
fn browse_color(x: usize, y: usize, frame: &BrowseFrame) -> u32 {
    // 1. The search field and its clear action.
    if frame.field.contains(x, y) {
        // WHY the outline: `border` (#E5E5E5) is 1.08:1 against this field's own
        // #EFEEE9 fill and 1.21:1 against the page, and the fill is 1.11:1 off the
        // page, so the search field had no visible bounds. `outline` (#0A0A0A) is
        // 17.04:1 on the field and 18.97:1 on the page; the magnifier and clear
        // glyphs keep `on_surface_variant` at 6.53:1 on the fill, above the 3:1 this
        // repo asks of an icon.
        let Some(base) = draw_rounded_rect(
            x,
            y,
            frame.field,
            frame.field_radius,
            frame.field_fill,
            Some(CONTROL_OUTLINE),
            1,
        ) else {
            return THEME.surface;
        };
        if frame.show_clear && frame.clear.contains(x, y) {
            let dx = x as i32 - frame.clear_x as i32;
            let dy = y as i32 - frame.clear_y as i32;
            if dx.abs() <= 6 && dy.abs() <= 6 && (dx.abs() - dy.abs()).abs() <= 2 {
                return THEME.on_surface_variant;
            }
            return base;
        }
        let dx = x as i32 - frame.magnifier_x as i32;
        let dy = y as i32 - frame.magnifier_y as i32;
        let ring = dx * dx + dy * dy;
        if (ring >= 36 && ring <= 64) || (dx >= 6 && dx <= 11 && dy >= 6 && dy <= 11) {
            return THEME.on_surface_variant;
        }
        if let Some(color) = frame.query.pixel(x, y) {
            return color;
        }
        return frame.field_fill;
    }

    // 2. The search scope caption and its two pills.
    if (frame.scope_y0..frame.scope_y1).contains(&y)
        && x >= SCOPE_TEXT_X
        && x < frame.scope_x1
    {
        if let Some(color) = frame.scope_caption.pixel(x, y) {
            return color;
        }
        return THEME.surface;
    }
    for pill in &frame.scope_pills {
        if let Some(color) = pill.pixel(x, y) {
            return color;
        }
    }

    // 3. The breadcrumbs. The painted rows are the whole union box the rungs
    // reserve, centred inside the touch strip beside them, so a crumb is
    // answered wherever its ink reaches -- and the strip the frame holds was laid
    // out by `crumb_step`, the same function the tap handler walks, so a crumb is
    // only inked where it can be tapped and a run never straddles the strip's edge.
    if (frame.crumb_y0..frame.crumb_y1).contains(&y)
        && (CRUMB_ROW_X0..CRUMB_ROW_X1).contains(&x)
    {
        for crumb in &frame.crumbs[..frame.crumb_count] {
            if let Some(color) = crumb.run.pixel(x, y) {
                return color;
            }
            if let Some(color) = crumb.sep.pixel(x, y) {
                return color;
            }
        }
        return THEME.surface;
    }

    // 4. The sort and layout toolbar.
    for pill in &frame.toolbar {
        if let Some(color) = pill.pixel(x, y) {
            return color;
        }
    }

    // 5. The details card, drawn over the entries card.
    if let Some(details) = &frame.details {
        if let Some(color) = details_color(x, y, details) {
            return color;
        }
    }

    // 6. The confirmation sheet, drawn over the entries card.
    if let Some(sheet) = &frame.sheet {
        if let Some(color) = sheet_color(x, y, sheet) {
            return color;
        }
    }

    // 7. The entries card.
    if (frame.card.y..frame.card.y + frame.card.h).contains(&y) {
        if x < frame.card.x || x >= frame.card.x + frame.card.w {
            return THEME.surface;
        }
        let Some(base) = draw_rounded_rect(
            x,
            y,
            frame.card,
            frame.card_radius,
            frame.card_fill,
            frame.card_border,
            1,
        ) else {
            return THEME.surface;
        };
        if frame.empty {
            // Folder pictogram above the headline.
            if let Some(ink) = glyph_ink(
                FileMime::Directory,
                x as isize - 343,
                y as isize - frame.glyph_y,
                THEME.primary,
            ) {
                return ink;
            }
            if let Some(color) = frame.headline.pixel(x, y) {
                return color;
            }
            if let Some(color) = frame.subline.pixel(x, y) {
                return color;
            }
        }
        return base;
    }

    // 8. The status line.
    if (frame.status_y0..frame.status_y1).contains(&y)
        && (STATUS_TEXT_X..CRUMB_ROW_X1).contains(&x)
    {
        if let Some(color) = frame.status.pixel(x, y) {
            return color;
        }
        return THEME.surface;
    }

    // 9. The action bar.
    if (ACTION_BAR_Y0..ACTION_BAR_Y1).contains(&y) {
        for pill in &frame.slots {
            if let Some(color) = pill.pixel(x, y) {
                return color;
            }
        }
        return THEME.surface;
    }

    // The home gesture pill, drawn from the rectangle the tap handler tests.
    if HOME_PILL.contains(x, y) {
        return THEME.on_surface_variant;
    }

    THEME.surface
}

/// The label, whether it can be taken, and whether it is the current mode.
fn action_slot_state(app: &FileManagerApp, slot: usize) -> (&'static [u8], bool, bool) {
    match slot {
        0 => (b"Select", true, app.multi_select),
        1 => (b"All", app.card_item_count() > 0, false),
        _ => match action_for_slot(slot) {
            Some(action) => {
                let enabled = if action == FileAction::NewFolder {
                    // The control is enabled when the platform supports new folder creation.
                    file_manager::NEW_FOLDER_SUPPORTED
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
///
/// WHY it reads a resolved frame: the card's radius, fill, border and every run on
/// it were re-derived at the pixel, so the one screen that names a single file cost
/// more than the whole list it covers.
fn details_color(x: usize, y: usize, frame: &DetailsFrame) -> Option<u32> {
    if !frame.rect.contains(x, y) {
        return None;
    }
    let Some(base) = draw_rounded_rect(
        x,
        y,
        frame.rect,
        frame.radius,
        frame.fill,
        frame.border,
        1,
    ) else {
        return Some(THEME.surface);
    };
    if let Some(color) = frame.title.pixel(x, y) {
        return Some(color);
    }
    for line in &frame.lines[..frame.line_count] {
        if let Some(color) = line.pixel(x, y) {
            return Some(color);
        }
    }
    if let Some(color) = frame.open.pixel(x, y, base) {
        return Some(color);
    }
    // Inside a button's rectangle but outside its corner arc: the card shows
    // through, and `base` is that card's own pixel. It used to say
    // `THEME.surface` -- the page's `#FAFAFA` -- which punched a pale quarter-disc
    // of a colour that is nowhere on this card into each corner. The outline made
    // that artefact worth naming: a control with a visible edge shows the blobs
    // sitting right against its arc.
    frame.close.pixel(x, y, base).or(Some(base))
}

/// The confirmation sheet, drawn over the entries card.
///
/// WHY it reads a resolved frame: the sheet's state -- whether it offers
/// destinations or a name field -- was re-derived from the pending action at every
/// one of its pixels, along with its ladder and each of its runs.
fn sheet_color(x: usize, y: usize, frame: &SheetFrame) -> Option<u32> {
    if !frame.rect.contains(x, y) {
        return None;
    }
    let Some(base) =
        draw_rounded_rect(x, y, frame.rect, frame.radius, frame.fill, frame.border, 1)
    else {
        return Some(THEME.surface);
    };
    if let Some(color) = frame.title.pixel(x, y) {
        return Some(color);
    }
    for name in &frame.names[..frame.name_count] {
        if let Some(color) = name.pixel(x, y) {
            return Some(color);
        }
    }
    // A destination that the pending action does not offer arrives as an empty
    // button, so it answers nothing and the sheet's own pixels stand.
    for dest in &frame.destinations {
        if let Some(color) = dest.pixel(x, y, base) {
            return Some(color);
        }
    }
    if let Some(color) = frame.name_field.pixel(x, y, base) {
        return Some(color);
    }
    if let Some(color) = frame.cancel.pixel(x, y, base) {
        return Some(color);
    }
    frame.confirm.pixel(x, y, base).or(Some(base))
}

// -----------------------------------------------------------------------------
// Widget rendering
// -----------------------------------------------------------------------------

const ROW_BUFFER_W: usize = 648;
/// WHY the largest selectable rung rather than the one drawn with: a row is painted
/// into this buffer and blitted out of it, so a buffer cut to the rung the layout
/// happened to answer at would clip the taller row the person's Text size asks for
/// -- and the clip is invisible to the pixel function, which never sees the row.
const ROW_BUFFER_H: usize = entry_row_h_max();
static mut ROW_BUFFER: [u32; ROW_BUFFER_W * ROW_BUFFER_H] = [0; ROW_BUFFER_W * ROW_BUFFER_H];

const CELL_BUFFER_W: usize = GRID_CELL_W;
const CELL_BUFFER_H: usize = grid_cell_h_max();
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
    let card = entries_card();
    if let Some((thumb_y, thumb_h)) = app.scroll.thumb_rect(card.y + 4, card.h - 8, 24) {
        let tx = card.x + card.w - 10;
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
    let card = entries_card();
    let is_video_ctx = app.current_path_str().starts_with("/storage/videos");
    for index in 0..count {
        if !row_is_visible(card, index, offset) {
            continue;
        }
        let rect = entry_row_rect(card, index, offset);
        // WHY the buffer is walked to the row's own height and not to the buffer's:
        // the buffer is sized at the largest rung a person can choose, while the row
        // painted into it is the rung this frame draws at. Filling the whole buffer
        // would paint the divider and the selection bar past the row's foot, into the
        // rows the next entry owns.
        let row_h = rect.h;
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
        for ry in 0..row_h {
            for rx in 0..ROW_BUFFER_W {
                let mut color = fill;
                if ry == row_h - 1 {
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
            row_h,
            ENTRY_GLYPH_X,
            entry_glyph_dy(),
            mime,
            ink,
        );

        if app.searching {
            // A search result shows its name and the path it was found at:
            // two files with the same name in different folders must not look
            // like the same result. Both runs answer the row's own lane --
            // `ENTRY_NAME_LANE`, the row's width less the text column it starts
            // in and the air before its right edge -- at the caption rung, so
            // the trim the row applies is the trim the gate measures. The two
            // runs stand on the row's own offsets, whose lower band is the bound
            // `entry_row_h_at` sizes this plate by.
            let mut name_buf = [0u8; 48];
            let name_len = ellipsize_run(name, ENTRY_SUB_SCALE, ENTRY_NAME_LANE, &mut name_buf);
            if name_len > 0 {
                draw_buffer_text(
                    buffer,
                    ROW_BUFFER_W,
                    row_h,
                    ENTRY_TEXT_X,
                    entry_search_name_dy(),
                    ENTRY_SUB_SCALE,
                    &name_buf[..name_len],
                    THEME.on_surface,
                );
            }
            let mut path = [0u8; MAX_PATH_LEN];
            if let Some(path_len) = app.card_item_path(index, &mut path) {
                let mut path_buf = [0u8; MAX_PATH_LEN];
                let shown = ellipsize_run(
                    &path[..path_len],
                    ENTRY_SUB_SCALE,
                    ENTRY_NAME_LANE,
                    &mut path_buf,
                );
                if shown > 0 {
                    draw_buffer_text(
                        buffer,
                        ROW_BUFFER_W,
                        row_h,
                        ENTRY_TEXT_X,
                        entry_search_path_dy(),
                        ENTRY_SUB_SCALE,
                        &path_buf[..shown],
                        THEME.on_surface_variant,
                    );
                }
            }
        } else {
            // The name gives up exactly the room its trailing fact occupies, so
            // the two runs never claim the same column of the row, and the lane
            // they split is still the row's own.
            let trailing = app.card_item_trailing(index);
            let trailing_w = run_w(trailing, ENTRY_NAME_SCALE);
            let max_w = ENTRY_NAME_LANE.saturating_sub(trailing_w);
            let mut name_buf = [0u8; 48];
            let name_len = ellipsize_run(name, ENTRY_NAME_SCALE, max_w, &mut name_buf);
            if name_len > 0 {
                draw_buffer_text(
                    buffer,
                    ROW_BUFFER_W,
                    row_h,
                    ENTRY_TEXT_X,
                    (row_h.saturating_sub(run_line(ENTRY_NAME_SCALE))) / 2,
                    ENTRY_NAME_SCALE,
                    &name_buf[..name_len],
                    THEME.on_surface,
                );
            }
            if trailing_w > 0 {
                draw_buffer_text(
                    buffer,
                    ROW_BUFFER_W,
                    row_h,
                    ROW_BUFFER_W - ENTRY_RIGHT_AIR - trailing_w,
                    (row_h.saturating_sub(run_line(ENTRY_NAME_SCALE))) / 2,
                    ENTRY_NAME_SCALE,
                    trailing,
                    THEME.on_surface_variant,
                );
            }
        }

        // Only the part of the row that is on the card is written.
        let top = rect.y.max(card.y);
        let bottom = (rect.y + row_h).min(card.y + card.h);
        if bottom <= top {
            continue;
        }
        let from = top - rect.y;
        let rows = bottom - top;
        write_pixels_rows(
            surface,
            top * W + card.x,
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
    let card = entries_card();
    let lvl = draw_level();
    let is_video_ctx = app.current_path_str().starts_with("/storage/videos");
    for index in 0..count {
        let rect = grid_cell_rect(card, index, offset);
        if rect.y >= card.y + card.h || rect.y + rect.h <= card.y {
            continue;
        }
        // WHY the buffer is walked to the cell's own height: as in `render_rows`,
        // the buffer is sized at the largest selectable rung and the cell painted
        // into it at the rung this frame draws at.
        let cell_h = rect.h;
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
        for py in 0..cell_h {
            for px in 0..CELL_BUFFER_W {
                let mut color = fill;
                if selected && px < 4 {
                    color = THEME.primary;
                }
                if py == cell_h - 1 {
                    color = ROW.divider;
                }
                buffer[py * CELL_BUFFER_W + px] = color;
            }
        }
        paint_glyph(buffer, CELL_BUFFER_W, cell_h, GRID_GLYPH_X, grid_glyph_dy(), mime, ink);

        // The cell's own lane: `GRID_CELL_W` less the inset it stands on each
        // side. The name and the fact under it both answer that lane, so a long
        // name is trimmed with an ellipsis the user can see rather than cut off
        // at the cell's edge, where the next cell's ink begins.
        let mut name_buf = [0u8; 48];
        let name_len = ellipsize_run(name, GRID_NAME_SCALE, GRID_NAME_LANE, &mut name_buf);
        if name_len > 0 {
            draw_buffer_text(
                buffer,
                CELL_BUFFER_W,
                cell_h,
                GRID_TEXT_INSET,
                grown(66, GRID_NAME_SCALE, lvl),
                GRID_NAME_SCALE,
                &name_buf[..name_len],
                ink,
            );
        }
        if app.searching {
            let mut path = [0u8; MAX_PATH_LEN];
            if let Some(path_len) = app.card_item_path(index, &mut path) {
                let mut path_buf = [0u8; MAX_PATH_LEN];
                let shown = ellipsize_run(
                    &path[..path_len],
                    GRID_NAME_SCALE,
                    GRID_NAME_LANE,
                    &mut path_buf,
                );
                if shown > 0 {
                    draw_buffer_text(
                        buffer,
                        CELL_BUFFER_W,
                        cell_h,
                        GRID_TEXT_INSET,
                        grown(96, GRID_NAME_SCALE, lvl),
                        GRID_NAME_SCALE,
                        &path_buf[..shown],
                        THEME.on_surface_variant,
                    );
                }
            }
        } else {
            let trailing = app.card_item_trailing(index);
            let mut trailing_buf = [0u8; 48];
            let shown =
                ellipsize_run(trailing, GRID_NAME_SCALE, GRID_NAME_LANE, &mut trailing_buf);
            if shown > 0 {
                draw_buffer_text(
                    buffer,
                    CELL_BUFFER_W,
                    cell_h,
                    GRID_TEXT_INSET,
                    grown(96, GRID_NAME_SCALE, lvl),
                    GRID_NAME_SCALE,
                    &trailing_buf[..shown],
                    THEME.on_surface_variant,
                );
            }
        }

        let top = rect.y.max(card.y);
        let bottom = (rect.y + cell_h).min(card.y + card.h);
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
    for dy in 0..GLYPH_ROWS {
        let py = icon_y + dy;
        if py >= height {
            break;
        }
        for dx in 0..GLYPH_COLS {
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
///
/// WHY the band and the width come from the rung: the plate the row or cell is
/// filled to is `plate_at`'s, i.e. the platform's `line_height` *at the level*,
/// and a blit loop that walked the ladder's box inside a rung-sized plate
/// would leave the bottom of every enlarged row unpainted -- pixels the hit test
/// still answers. At the level the layout was authored at these are the ladder's
/// own two numbers, so no row changes.
///
/// WHY the ladder's anti-aliased face is kept at that level: a widget buffer is
/// blitted whole, and swapping its face for the outline one would repaint rows
/// the platform's truth contract already samples. Above it the bitmap face is
/// asked for the rung's cap height, which is the same face grown.
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
    if scale == 0 || text.is_empty() {
        return;
    }
    let lvl = draw_level();
    let line_h = run_line(scale);
    let text_w = run_w(text, scale);
    let x1 = (x0 + text_w).min(width);
    let y1 = (y0 + line_h).min(height);
    let cap = cap_at(scale, lvl);
    for py in y0..y1 {
        for px in x0..x1 {
            let bg = buffer[py * width + px];
            let inked = if lvl == PLATFORM_LEVEL {
                font::test_text_aa(px, py, x0, y0, scale, text, color, bg, false)
            } else {
                std::design::monochrome::text_px(px, py, x0, y0, cap, text, color, bg, false)
            };
            if let Some(inked) = inked {
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

/// Guest ticks, when the platform will answer.
#[inline]
fn tick_opt() -> Option<u64> {
    std::get_ticks().ok()
}

/// Guest ticks between two sample points.
#[inline]
fn tick_delta(a: Option<u64>, b: Option<u64>) -> u64 {
    match (a, b) {
        (Some(a), Some(b)) => b.wrapping_sub(a),
        _ => 0,
    }
}

/// Stack buffer for a log line, because this binary holds no heap.
struct LineBuf<const N: usize> {
    buf: [u8; N],
    len: usize,
}

impl<const N: usize> LineBuf<N> {
    const fn new() -> Self {
        LineBuf { buf: [0u8; N], len: 0 }
    }
    fn as_str(&self) -> &str {
        core::str::from_utf8(&self.buf[..self.len]).unwrap_or("")
    }
}

impl<const N: usize> core::fmt::Write for LineBuf<N> {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        for ch in s.bytes() {
            if self.len >= N {
                return Err(core::fmt::Error);
            }
            self.buf[self.len] = ch;
            self.len += 1;
        }
        Ok(())
    }
}

/// The split of one full-surface pass, in guest ticks.
///
/// WHY the pass is split at all: "the screen came late" and "the screen came
/// never" look the same from outside, and the two costs they name -- resolving
/// the frame once, then answering a million pixels from it -- are bounded by
/// different code. A regression in either is visible in one number.
fn log_frame_breakdown(prep: u64, draw: u64, write: u64, widgets: u64, chunks: usize) {
    let mut line = LineBuf::<128>::new();
    {
        use core::fmt::Write;
        let _ = write!(line, "[FILE_MANAGER] FRAMEBREAK prep={} draw={} write={} widgets={} chunks={}",
            prep, draw, write, widgets, chunks);
    }
    std::log(line.as_str()).ok();
}

/// Re-rasterises and writes only rows `y0..y1`.
fn render_file_manager_band(surface: u32, app: &FileManagerApp, y0: usize, y1: usize) {
    // The pass's answers, resolved before the first pixel is asked.
    let t_prep = tick_opt();
    let frame = build_frame(app);
    let prep_ticks = tick_delta(t_prep, tick_opt());
    let mut chunk = [0u32; CHUNK];
    let mut offset = y0 * W;
    let end = y1.min(H) * W;
    let mut draw_ticks = 0u64;
    let mut write_ticks = 0u64;
    let mut chunks = 0usize;
    while offset < end {
        let count = (end - offset).min(CHUNK);
        let mut x = offset % W;
        let mut y = offset / W;
        let t_draw = tick_opt();
        for i in 0..count {
            chunk[i] = get_file_manager_color(x, y, app, &frame);
            x += 1;
            if x == W {
                x = 0;
                y += 1;
            }
        }
        let t_write = tick_opt();
        draw_ticks += tick_delta(t_draw, t_write);
        std::surface_write(surface, offset, &chunk[..count]).ok();
        let t_done = tick_opt();
        write_ticks += tick_delta(t_write, t_done);
        chunks += 1;
        offset += count;
    }
    let t_widgets = tick_opt();
    if surface > 0
        && y0 < entries_card().y + entries_card().h
        && y1 > entries_card().y
    {
        render_widgets(surface, app);
    }
    let widgets_ticks = tick_delta(t_widgets, tick_opt());
    if y0 == 0 && y1 >= H {
        log_frame_breakdown(prep_ticks, draw_ticks, write_ticks, widgets_ticks, chunks);
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
    let mut stat_buf = [0u8; std::FILE_STAT_BYTES];
    if std::fs_stat(path, &mut stat_buf).is_ok() {
        return true;
    }
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
        let mut target = [0u8; MAX_PATH_LEN];
        let Some(target_len) = join_path(app.current_path_str(), &typed[..typed_len], &mut target) else {
            record_failure(app, &typed[..typed_len], b"the target path is too long");
            let mut line = [0u8; LINE_LEN];
            let len = append(&mut line, 0, b"New: 0 done, 1 failed - first: the target path is too long");
            app.notice = line;
            app.notice_len = len;
            return;
        };
        let Ok(target_str) = core::str::from_utf8(&target[..target_len]) else {
            record_failure(app, &typed[..typed_len], b"the target path is not text");
            let mut line = [0u8; LINE_LEN];
            let len = append(&mut line, 0, b"New: 0 done, 1 failed - first: the target path is not text");
            app.notice = line;
            app.notice_len = len;
            return;
        };
        let target_exists = file_exists(target_str);
        if let Err(problem) = validate_operation(
            action,
            0,
            false,
            &typed[..typed_len],
            b"",
            target_exists,
        ) {
            record_failure(app, &typed[..typed_len], problem_text(problem));
            let mut line = [0u8; LINE_LEN];
            let mut len = append(&mut line, 0, b"New: 0 done, 1 failed - first: ");
            let take = typed_len.min(line.len().saturating_sub(len + 32));
            len = append(&mut line, len, &typed[..take]);
            len = append(&mut line, len, b" (");
            let reason = problem_text(problem);
            let rtake = reason.len().min(line.len().saturating_sub(len + 1));
            len = append(&mut line, len, &reason[..rtake]);
            len = append(&mut line, len, b")");
            app.notice = line;
            app.notice_len = len;
            return;
        }

        let outcome = std::fs_mkdir(target_str);
        match outcome {
            Ok(()) => {
                let mut line = [0u8; LINE_LEN];
                let len = append(&mut line, 0, b"New: 1 done, 0 failed");
                app.notice = line;
                app.notice_len = len;
            }
            Err(code) => {
                let mut reason = [0u8; 32];
                let mut rlen = append(&mut reason, 0, b"platform code -");
                rlen = append_u64(&mut reason, rlen, code.unsigned_abs());
                record_failure(app, &typed[..typed_len], &reason[..rlen]);

                let mut line = [0u8; LINE_LEN];
                let mut len = append(&mut line, 0, b"New: 0 done, 1 failed - first: ");
                let take = typed_len.min(line.len().saturating_sub(len + 32));
                len = append(&mut line, len, &typed[..take]);
                len = append(&mut line, len, b" (");
                let rtake = rlen.min(line.len().saturating_sub(len + 1));
                len = append(&mut line, len, &reason[..rtake]);
                len = append(&mut line, len, b")");
                app.notice = line;
                app.notice_len = len;
            }
        }

        app.selection.clear();
        app.multi_select = false;
        app.selected_entry = None;
        app.refresh_directory();
        app.load_recents();
        if surface > 0 {
            render_file_manager_ui(surface, app);
            let _ = std::surface_flip();
        }
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
            // The band the painter treats as the status line, asked at the rung
            // it inks at: `STATUS_Y` plus a one-em line is the authored strip, and
            // above the authored rung the row grows past it -- a progress tick
            // would repaint the top of the line and leave its tail on screen.
            let status_top = status_y();
            render_file_manager_band(surface, app, status_top, status_top + status_rows());
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
            FileAction::NewFolder => std::fs_mkdir(target_str),
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
    let mut cursor = CRUMB_ROW_X0;
    for index in 0..app.crumb_count {
        let crumb = app.crumbs[index];
        let label = &app.current_path[crumb.label_start..crumb.label_start + crumb.label_len];
        // WHY the model's layout function and not a measurement of its own: the
        // painter lays this row out through `crumb_step`, which trims a run to the
        // room the strip has left and stops when there is none. A hit test that
        // measured the raw label answered letters that were never inked, and kept
        // answering crumbs the strip had already dropped -- and a rung or a
        // separator move changed one site and not the other.
        let mut buf = [0u8; MAX_PATH_LEN];
        let Some(step) = crumb_step(label, cursor, index + 1 == app.crumb_count, &mut buf) else {
            break;
        };
        let width = run_w(&buf[..step.len], BREADCRUMB_SCALE);
        if x >= step.x && x < step.x + width {
            return Some(index);
        }
        cursor = step.next_x;
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
    // The drawn back control, hit where every other app's control is hit: the
    // SDK's rect is both the paint and the target, so the two cannot disagree.
    if std::design::back_button::contains(x, y) {
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
        let row_h = home_recent_row_h();
        for index in 0..visible {
            let top = home_recents_y0() + index * row_h;
            if y >= top && y < top + row_h
                && (HOME_ROW_X..PANEL_CONTENT_X1).contains(&x)
            {
                app.open_recent(index);
                std::log("[FILE_MANAGER] Opened a recent document from the index.").ok();
                return Tap::Redraw(Dirty::All);
            }
        }
    }

    // The category rows.
    if (category_y0()..category_y1()).contains(&y)
        && (HOME_ROW_X..PANEL_CONTENT_X1).contains(&x)
    {
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
    if home_browse_btn().contains(x, y) {
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
    let field = search_field();
    if (field.y..field.y + field.h).contains(&y)
        && x >= field.x
        && x < field.x + field.w
    {
        if !app.query.is_empty() && search_clear().contains(x, y) {
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
    if scope_this_pill().contains(x, y) || scope_all_pill().contains(x, y) {
        app.search_scope = app.search_scope.toggle();
        if app.searching && !app.query.is_empty() {
            app.run_search();
        } else {
            app.set_scope();
            app.set_status();
        }
        return Tap::Redraw(Dirty::All);
    }

    // The breadcrumbs. The strip is derived as the gap between the scope row's
    // foot and the toolbar's top, so its foot is the toolbar's top by
    // construction -- a clamp against it here would be arithmetic on one number,
    // and the gate beside the derivation refuses the day it stops being one.
    let (crumb_y0, crumb_y1) = crumb_hit();
    if y >= crumb_y0 && y < crumb_y1 {
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
    if toolbar_pill(0).contains(x, y) {
        app.apply_sort();
        return Tap::Redraw(Dirty::All);
    }
    if toolbar_pill(1).contains(x, y) {
        if app.layout != LayoutMode::List {
            app.toggle_layout();
        }
        return Tap::Redraw(Dirty::All);
    }
    if toolbar_pill(2).contains(x, y) {
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
    if count > 0 && entries_card().contains(x, y) {
        let hit = match app.layout {
            LayoutMode::List => entry_at_point(entries_card(), app.scroll.offset, count, x, y),
            LayoutMode::Grid => grid_at_point(entries_card(), app.scroll.offset, count, x, y),
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
            let mut rows = [0usize; MAX_PARSED_ENTRIES];
            let count = if action == FileAction::NewFolder {
                0
            } else {
                app.action_rows(&mut rows)
            };
            let mut any_dir = false;
            for &row in &rows[..count] {
                if app.card_item_is_dir(row) {
                    any_dir = true;
                    break;
                }
            }
            match evaluate_action_tap(action, count, any_dir) {
                ActionTapOutcome::Refused(problem) => {
                    app.set_notice(problem_text(problem));
                    return Tap::Redraw(Dirty::All);
                }
                ActionTapOutcome::OpenConfirmation(act) => {
                    if set_as_action(app, act) {
                        return Tap::Redraw(Dirty::All);
                    }
                }
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
    if app.pending.action == FileAction::Rename || app.pending.action == FileAction::NewFolder {
        let field = Rect::new(SHEET.x + 20, sheet_name_y(), SHEET.w - 40, sheet_name_h());
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
        let target_exists = if action == FileAction::NewFolder {
            let mut target = [0u8; MAX_PATH_LEN];
            if let Some(target_len) = join_path(app.current_path_str(), app.rename_field.as_slice(), &mut target) {
                if let Ok(target_str) = core::str::from_utf8(&target[..target_len]) {
                    file_exists(target_str)
                } else {
                    false
                }
            } else {
                false
            }
        } else {
            false
        };
        if let Err(problem) = validate_operation(
            action,
            count,
            any_dir,
            app.rename_field.as_slice(),
            &current_name.0[..current_name.1],
            target_exists,
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
    if open_offered && details_open().contains(x, y) {
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
    if details_close().contains(x, y) || !details_card().contains(x, y) {
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
    if count == 0 || !entries_card().contains(x, y) {
        return None;
    }
    match app.layout {
        LayoutMode::List => entry_at_point(entries_card(), app.scroll.offset, count, x, y),
        LayoutMode::Grid => grid_at_point(entries_card(), app.scroll.offset, count, x, y),
    }
}

// -----------------------------------------------------------------------------
// Entry point and event reactor loop
// -----------------------------------------------------------------------------

#[no_mangle]
pub extern "C" fn _start() -> ! {
    std::log("[FILE_MANAGER (Ring 3)] Sovereign File Manager & VFS Explorer online.").ok();

    // WHY before the app is built and the surface exists: every box in this
    // screen is derived from the rung the renderer holds, so a rung handed over
    // after the first frame is painted is a first frame laid out at one hundred
    // per cent and never re-laid.
    publish_text_scale();

    let mut app = FileManagerApp::new();

    // The band's clock lane, read before the first frame so the first paint
    // shows the time the platform answered with and not a blank lane.
    refresh_clock(&mut app);

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
        // WHY before the poll: the poll blocks up to its timeout, so this is one
        // clock read per wake -- never one per pixel, and never inside the
        // rasteriser.
        let clock_moved = refresh_clock(&mut app);
        // WHY on every wake rather than once at launch: the person changes the
        // size in Settings while this screen is alive, and a rung read only at
        // startup is a screen that resizes on the next cold open -- if ever.
        let rung_moved = publish_text_scale();
        // WHY before the poll rather than beside the repaint below: a tap in this
        // same wake would paint the new rung off the frame, and the listing's
        // scroll extent is solved from the card the *old* rung left behind. The
        // extent has to move with the rung, or the thumb sizes a card that is no
        // longer there and an offset past the new bottom no tap can reach back.
        if rung_moved {
            app.relayout_for_rung();
        }
        let interest = std::EVENT_TOUCH_INPUT | std::EVENT_TIMER_TICK;
        let mut repainted_wholly = false;
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
                    let is_home_gesture = HOME_HIT.contains(input.x as usize, input.y as usize);
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
                        && entries_card().contains(input.x as usize, input.y as usize)
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
                    repainted_wholly = true;
                }
            }
            // The band alone is re-rasterised when the minute turned, so a
            // clock tick costs 48 rows instead of the whole screen.
            if clock_moved && !repainted_wholly && surface > 0 {
                render_file_manager_band(surface, &app, 0, STATUS_BAR_HEIGHT);
                let _ = std::surface_flip();
            }
        }
        // The whole screen, not a band: the rung moves the header, the card's
        // rows, the toolbar's pills and the status strip, so a band repaint would
        // leave every region the previous frame sized at the size the person just
        // left behind. WHY outside the poll guard above: `publish_text_scale` has
        // already moved the rung the library answers from, so a poll that errored
        // would leave one frame painted across two rungs.
        if rung_moved && !repainted_wholly && surface > 0 {
            render_file_manager_ui(surface, &app);
            let _ = std::surface_flip();
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
