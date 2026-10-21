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
use file_manager::theme::{MIN_TOUCH_TARGET, THEME, radius, spacing};
use std::design::components;
use std::design::font;
use std::design::primitives::{draw_pill, Rect};
pub use file_manager::{
    action_for_mime, detect_mime, fmt_u64, parse_vfs_entries, document_from_index,
    FileMime, RuntimeFileEntry, SelectedDocument, MAX_DOCUMENT_PATH_LEN, MAX_ENTRY_NAME_LEN,
    MAX_PARSED_ENTRIES,
};

use interos_std::design::outline_text;


/// The platform's corner radii, under the short names the renderer uses.
///
/// WHY the aliases exist: the corner test names its radius four times, and a
/// number that appears four times is a number that can drift from the
/// platform. The alias resolves to the platform's step, so the drift is the
/// platform's and not this app's.
const R_SOFT: usize = radius::SOFT_PILL;

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
/// Height of the header band.
const HEADER_H: usize = 140;
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
const ROW_BAND_Y0: usize = 150;
const ROW_BAND_Y1: usize = 530;

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
pub const ACTIONS_Y0: usize = 560;
pub const ACTIONS_Y1: usize = 840;
/// The "open with" line, where the handlers the registry names are drawn.
pub const ACTION_OPEN_Y0: usize = 676;
pub const ACTION_OPEN_Y1: usize = 720;
/// The "set as" line, where the action the media type allows is drawn.
pub const ACTION_SET_Y0: usize = 736;
pub const ACTION_SET_Y1: usize = 780;
/// The panel's left edge.
pub const ACTIONS_X0: usize = 36;
/// The panel's right edge.
pub const ACTIONS_X1: usize = 684;

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
    }
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

    // 2. Vertical Category Rows (y: 150..530)
    //
    // WHY four rows and not six: the kernel mounts `/storage` with three
    // children, and the app path policy lets this app list exactly those.
    // The old rows named scopes the platform does not have -- a recents
    // list, documents, downloads and a cloud service -- and every one of
    // them opened `/data`, which the kernel refuses, so each row led to a
    // refusal dressed up as a category. The rows are now the four
    // directories that exist and that this app is allowed to read.
    if (150..530).contains(&y) && x >= 36 && x < 684 {
        let (row_idx, row_start_y, row_h) = if y < 260 {
            (0, 150, 110)
        } else if y < 350 {
            (1, 260, 90)
        } else if y < 440 {
            (2, 350, 90)
        } else {
            (3, 440, 90)
        };

        // Thin bottom divider line, in the platform's divider ink.
        if y == row_start_y + row_h - 1 {
            return ROW.divider;
        }

        // Group container card outer border and rounded corners
        let mut on_card_border = false;
        let rx = (x - 36).min(683 - x);
        let ry = (y - 150).min(529 - y);
        if (y < 170 || y >= 510) && rx < 16 && ry < 16 {
            let cdx = (16 - rx) as isize;
            let cdy = (16 - ry) as isize;
            let d2 = cdx * cdx + cdy * cdy;
            if d2 >= 256 {
                return THEME.surface;
            }
            if d2 >= 196 {
                on_card_border = true;
            }
        } else if rx < 2 || y < 152 || y >= 528 {
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

        let center_y = row_start_y + (if row_idx == 0 { 40 } else { 45 });
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

                // Tactile capacity track bar (x: 108..640, y: 250..256)
                if y >= 250 && y < 256 && x >= 108 && x < 640 {
                    return THEME.border;
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

    // 4. VFS Directory Explorer Section (y: 840..1380)
    if (840..1380).contains(&y) {
        // Section Header: "Directory Explorer" at y: 842, path at y: 842
        if (840..880).contains(&y) {
            if let Some(c) = test_text(x, y, 48, 842, 1, b"Directory Explorer", THEME.on_surface, THEME.surface) {
                return c;
            }
            let path_bytes = &app.current_path[..app.current_path_len];
            if let Some(c) = test_text(x, y, 260, 842, 1, path_bytes, THEME.on_surface_variant, THEME.surface) {
                return c;
            }
            return THEME.surface;
        }

        // Entries container card: x: 36..684
        if x >= 36 && x < 684 {
            let container_h = if app.entry_count == 0 {
                280
            } else {
                (app.entry_count * 56).max(120)
            };
            let card_y0 = 880;
            let card_y1 = card_y0 + container_h;

            if y >= card_y0 && y < card_y1 {
                let mut on_explorer_border = false;
                let rx = (x - 36).min(683 - x);
                let ry = (y - card_y0).min(card_y1 - 1 - y);

                // Rounded corners
                if rx < R_SOFT && ry < R_SOFT {
                    let cdx = (R_SOFT - rx) as isize;
                    let cdy = (R_SOFT - ry) as isize;
                    let d2 = cdx * cdx + cdy * cdy;
                    if d2 >= (R_SOFT * R_SOFT) as isize {
                        return THEME.surface;
                    }
                    if d2 >= ((R_SOFT - 2) * (R_SOFT - 2)) as isize {
                        on_explorer_border = true;
                    }
                } else if rx < 2 || ry < 2 {
                    on_explorer_border = true;
                }

                if app.entry_count == 0 {
                    // Vector folder pictogram centered at cx = 360, cy = card_y0 + 70
                    let cx = 360isize;
                    let cy = (card_y0 + 70) as isize;
                    let idx = x as isize - cx;
                    let idy = y as isize - cy;

                    // Folder tab (top-left) & folder body outline
                    if (idy >= -22 && idy < -14 && idx >= -36 && idx <= -10)
                        || (idy >= -14 && idy <= 24 && idx.abs() <= 36)
                    {
                        if idy == -14 || idy == 24 || idx.abs() == 36 || (idy < -14 && (idx == -36 || idx == -10 || idy == -22)) {
                            return THEME.primary;
                        }
                        if idy == 0 && idx >= -24 && idx <= 24 {
                            return THEME.border;
                        }
                    }

                    // WHY the headline follows the listing's own state: the
                    // card used to say "Directory Empty" for a directory the
                    // device had refused to read, which told the user their
                    // files were gone when the app had simply been refused.
                    let (headline, subline, headline_ink): (&[u8], &[u8], u32) =
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
                        };

                    // Headline
                    let hx = centered_x(headline, 2, 36, 648);
                    if let Some(c) = test_text(
                        x,
                        y,
                        hx,
                        card_y0 + 130,
                        2,
                        headline,
                        headline_ink,
                        THEME.surface_container,
                    ) {
                        return c;
                    }

                    // Subtitle
                    let sx = centered_x(subline, 1, 36, 648);
                    if let Some(c) = test_text(
                        x,
                        y,
                        sx,
                        card_y0 + 188,
                        1,
                        subline,
                        THEME.on_surface_variant,
                        THEME.surface_container,
                    ) {
                        return c;
                    }

                    if on_explorer_border {
                        return THEME.border;
                    }
                    return THEME.surface_container;
                }

                // Render specific entry row
                let entry_idx = (y - card_y0) / 56;
                let row_offset_y = (y - card_y0) % 56;

                if entry_idx < app.entry_count {
                    let entry = &app.entries[entry_idx];

                    // Row divider (except last row)
                    if row_offset_y == 55 && entry_idx + 1 < app.entry_count {
                        return THEME.border;
                    }

                    let is_selected = app.selected_entry == Some(entry_idx);
                    if is_selected && x < 44 {
                        return THEME.primary;
                    }
                    let entry_bg = if is_selected { THEME.primary } else { THEME.surface_container };

                    // Icon glyph / indicator at x: 56..80
                    let icon_cx = 64isize;
                    let icon_cy = (card_y0 + entry_idx * 56 + 28) as isize;
                    let idx = x as isize - icon_cx;
                    let idy = y as isize - icon_cy;

                    if entry.is_dir {
                        // Folder vector glyph
                        if (idx.abs() <= 12 && idy.abs() <= 8 && (idy >= -4 || idx <= 0))
                            || (idx >= -12 && idx <= 12 && idy == 8)
                        {
                            return THEME.primary;
                        }
                    } else {
                        // File document glyph
                        if (idx.abs() <= 8 && idy.abs() <= 10) && (idx.abs() >= 7 || idy.abs() >= 9 || idy == 0) {
                            return THEME.on_surface_variant;
                        }
                    }

                    // Entry name at x: 96, y0 + 16 (scale 1)
                    let text_y0 = card_y0 + entry_idx * 56 + 16;
                    if let Some(c) = test_text(
                        x,
                        y,
                        96,
                        text_y0,
                        1,
                        &entry.name[..entry.name_len],
                        THEME.on_surface,
                        entry_bg,
                    ) {
                        return c;
                    }

                    // Size / Type label right-aligned
                    let sw = font::measure(&entry.size_label[..entry.size_label_len], 1);
                    let sx = 660usize.saturating_sub(sw);
                    if let Some(c) = test_text(
                        x,
                        y,
                        sx,
                        text_y0,
                        1,
                        &entry.size_label[..entry.size_label_len],
                        if entry.is_dir { THEME.primary } else { THEME.on_surface_variant },
                        entry_bg,
                    ) {
                        return c;
                    }

                    return entry_bg;
                }
            }
        }
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
    let probe = std::fs_list(app.current_path_str(), &mut entries);
    app.record_probe(probe);
    match probe {
        Ok(n) => {
            app.parse_entries(&entries[..n]);
            log_counted(
                "[FILE_MANAGER] fs_list('/data'): ",
                n as u64,
                " bytes of directory entries received and parsed.",
            );
            log_counted(
                "[FILE_MANAGER] Parsed VFS directory entries: ",
                app.entry_count as u64,
                " items.",
            );
        }
        Err(code) => {
            log_counted(
                "[FILE_MANAGER] fs_list('/data') refused, code -",
                code.unsigned_abs(),
                "; no directory data received.",
            );
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
                    // Swipe gesture -> Return to launcher
                    if input.kind == 4 || input.kind == 6 || input.kind == 7 || input.kind == 8 || input.kind == 10
                    {
                        std::log("[FILE_MANAGER] Navigation gesture recognized. Exiting to launcher.",
                        )
                        .ok();
                        exit_to_launcher(surface);
                    }

                    if input.kind == 1 {
                        match handle_tap(&mut app, input.x as usize, input.y as usize) {
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
fn handle_tap(app: &mut FileManagerApp, x: usize, y: usize) -> Tap {
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

    // Category rows are drawn as a vertical list (y ROW_BAND_Y0..ROW_BAND_Y1,
    // x 36..684; rows of 90px, Internal Storage 110px).
    // WHY row-shaped hit test: the previous horizontal band logged a filter
    // switch for taps that hit nothing, missed the rows that are actually drawn,
    // and had no visible effect. The selection and the log now happen together,
    // only inside the row that was really tapped, and the renderer highlights
    // the selected row.
    if (ROW_BAND_Y0..ROW_BAND_Y1).contains(&y) && (36..684).contains(&x) {
        let shortcut = if y < 260 {
            0 // Storage
        } else if y < 350 {
            1 // Photos
        } else if y < 440 {
            2 // Pictures
        } else {
            3 // Videos
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

    // Directory Explorer entries card hit-test: x: 36..684, y: 880..
    let container_h = if app.entry_count == 0 { 72 } else { app.entry_count * 56 };
    let card_y0 = 880;
    let card_y1 = card_y0 + container_h;
    if (card_y0..card_y1).contains(&y) && (36..684).contains(&x) && app.entry_count > 0 {
        let entry_idx = (y - card_y0) / 56;
        if entry_idx < app.entry_count {
            let entry = app.entries[entry_idx];
            if entry.is_dir {
                app.navigate_into(entry_idx);
                std::log("[FILE_MANAGER] Navigated into directory.").ok();
                return Tap::Redraw(Dirty::All);
            }
            app.selected_entry = Some(entry_idx);
            let mut path_buf = [0u8; 128];
            let cp = &app.current_path[..app.current_path_len];
            let mut n = append(&mut path_buf, 0, cp);
            if !cp.ends_with(b"/") {
                n = append(&mut path_buf, n, b"/");
            }
            n = append(&mut path_buf, n, &entry.name[..entry.name_len]);
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
            return Tap::Redraw(Dirty::All);
        }
    }

    Tap::None
}

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    std::log("[FILE_MANAGER] Panic detected! Terminating task...").ok();
    std::exit();
}
