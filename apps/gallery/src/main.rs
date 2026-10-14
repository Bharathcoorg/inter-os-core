//! InterOS Native Gallery App (light Photos surface)
//!
//! Native Ring-3 graphical application executing in User Mode:
//! - DRM/KMS Client Surface allocation (z=200) and double-buffered scanout presentation
//! - Every cell renders a REAL image: the device's own wallpaper catalogue,
//!   `interos_std::design::walls::SCENES`, rasterised per pixel through the same
//!   `walls::scene_pixel` the compositor draws the lock and home surfaces with.
//!   The scene tables are traced silhouette and ramp data generated from the
//!   reference artwork (`tools/gen_walls.py`), so the gallery shows the device's
//!   real artwork rather than anything invented here
//! - The photo ledger stays a real ledger and stays empty: `PHOTOS` is only ever
//!   filled from image files found in the guest filesystem, and this build
//!   MEASURES that at boot (`scan_media`) instead of assuming it. The empty
//!   state quotes the measured counts and names the paths it could not read
//! - Full-screen photo inspection view (light themed) driven by a media record,
//!   reaching its previously unreachable state over the wallpaper catalogue
//! - Segmented filter chips (All / Wallpapers / Photos) and a bottom section bar,
//!   both bound to real sets - see `filters_available`
//! - Every drawn affordance and its hit region come from the same `Rect`, so a
//!   tap is accepted exactly where something is painted
//! - Every string is measured with `interos_std::design::font::measure` /
//!   `ellipsize` at the coordinates it is drawn at, so text never clips or overlaps
//! - Banded rendering (`render_gallery_band`): only the rows an input actually
//!   dirtied are re-rasterised, instead of all 1,036,800 pixels on every event
//! - Asynchronous event reactor loop, graceful lifecycle supervision and clean
//!   return to the Launcher

#![no_std]
#![no_main]

mod outline_text;

use core::panic::PanicInfo;
use interos_std as std;
use std::design::color;
use std::design::font;
use std::design::primitives::{draw_pill, draw_rounded_rect, Rect};
use std::design::radius;
use std::design::spacing;
use std::design::touch;
use std::design::walls;

const W: usize = 720;
const H: usize = 1440;
const CHUNK: usize = 4096;

// -----------------------------------------------------------------------------
// 32-bit ARGB8888 Design Color Tokens
//
// Every token is the design system's own (`sdk/rust/interos-std/src/design/
// mod.rs`), so a retuned palette moves this surface with the rest of the shell
// instead of leaving it behind. The names this app used before are kept as
// aliases so the geometry below still reads the same way, but no colour value
// is written in this file.
// -----------------------------------------------------------------------------
const BG: u32 = color::BACKGROUND;
const SURFACE: u32 = color::SURFACE;
const SOFT: u32 = color::SURFACE_SOFT;
const BORDER: u32 = color::BORDER;
const TEXT: u32 = color::TEXT;
const MUTED: u32 = color::TEXT_MUTED;
/// Ink for a filled (selected) control. The design system's surface tone is the
/// light end of its own scale, which is exactly the ink a dark fill takes.
const INVERSE: u32 = color::SURFACE;

/// Capability slot that holds `FileSystemRead` for the `app_gallery` task.
///
/// WHY a named constant and not a bare number: `spawn_app_gallery`
/// (`kernel/src/task/user.rs`) inserts its capabilities in a fixed order -
/// SerialOutput, IpcSend, TimerTickRead, HardwarePortIo, FileSystemRead,
/// FileSystemWrite, FrameBufferDirect, EventPoll - and `CapabilityTable::insert`
/// always takes the first free slot, so the filesystem read capability is index
/// 4. Passing slot 3 instead verifies `HardwarePortIo`, which every
/// filesystem-read syscall refuses before it looks at the path. The same slot
/// serves `fs_list` and `settings_get`, because the settings gate
/// (`sys_settings_get`) accepts `FileSystemRead` with READ rights exactly as
/// `fs_list` does.
const FS_READ_CAP_SLOT: usize = 4;

#[inline]
fn fs_read_slot() -> usize {
    std::cap_lookup(std::cap_type::FILE_SYSTEM_READ, 0).unwrap_or(FS_READ_CAP_SLOT)
}

// -----------------------------------------------------------------------------
// Layout constants - single source of truth for rendering *and* hit-testing
// -----------------------------------------------------------------------------
/// Screen margin (8pt grid value).
const MARGIN: usize = 24;
/// Grid gutter between columns/rows. 8px gutters with a 24px margin cannot
/// produce an integer cell on a 720px canvas (720 - 48 - 16 = 656, 656/3 is not
/// integral), so the gutter is the next 8pt-grid value that divides exactly:
/// 3 x 216 + 2 x 12 + 2 x 24 = 648 + 24 + 48 = 720.
const GUTTER: usize = 12;
const CELL: usize = 216;
const COL_X: [usize; 3] = [MARGIN, MARGIN + CELL + GUTTER, MARGIN + 2 * (CELL + GUTTER)];

const HEADER_H: usize = 144;
const TITLE_Y: usize = 44;
const SUBTITLE_Y: usize = 116;

const PILL_Y: usize = 152;
const PILL_H: usize = 48;
const PILL_GAP: usize = 12;
const PILL_PAD: usize = 20;
const PILL_MIN_W: usize = 72;

const GRID_TOP: usize = 216;
const ROW_STRIDE: usize = CELL + GUTTER;
const ROWS: usize = 4;
/// 216 + 4 x 216 + 3 x 12 = 1116: the grid stops clear of the bottom bar.
const GRID_BOTTOM: usize = GRID_TOP + ROWS * CELL + (ROWS - 1) * GUTTER;

/// Caption strip under a thumbnail's image.
///
/// WHY 46 and not the cell's full height: a wallpaper's name is one measured
/// scale-1 line (24 rows), and 46 leaves 11 rows of breathing room above and
/// below it inside a 216px cell. The image takes the rest.
const CAPTION_H: usize = 46;

const NAV_Y0: usize = 1290;
const NAV_Y1: usize = 1400;
const NAV_ICON_CY: usize = 1330;
const NAV_LABEL_Y: usize = 1356;

/// The system home indicator: a 200x6 pill drawn inside the 240x40 gesture
/// strip that the tap handler accepts.
///
/// WHY two rectangles and not one: the drawn indicator is 6px of ink, which is
/// not a touch target, so the hit box has to be the whole strip - but the strip
/// itself has to be a rectangle that is actually painted, otherwise the tap
/// region drifts. The old code tested `x 240..480 && y >= 1400` with no upper
/// bound on y, a 241x40px region that nothing ever drew, so a tap 18 rows above
/// the indicator exited the app. Both rectangles live here, so the strip that is
/// hit-tested is the strip that contains the ink.
const HOME_HIT: Rect = Rect::new(240, 1400, 240, 40);
const HOME_PILL: Rect = Rect::new(260, 1418, 200, 6);

/// Rows the filter-chip band owns.
///
/// WHY exactly the drawn pill band: `pill_pixel` returns `None` outside
/// `PILL_Y..PILL_Y + PILL_H`, and every run of text inside it is gated on its
/// own measured line box, which is inside the pill. No other row can hold a
/// pixel of this control, so widening the band would re-rasterise rows that
/// cannot have changed.
const FILTER_BAND_Y0: usize = PILL_Y;
const FILTER_BAND_Y1: usize = PILL_Y + PILL_H;

// Inspection view geometry.
const DETAIL_BAR_H: usize = 112;
const DETAIL_VIEW_Y0: usize = 128;
const DETAIL_VIEW_Y1: usize = 800;
/// The inspection viewport's height, and the frame's own ratio.
const DETAIL_VIEW_H: usize = DETAIL_VIEW_Y1 - DETAIL_VIEW_Y0;
/// The inspection viewport's width: half the content width, because a wallpaper
/// is 720x1440 and the only rectangle that shows every pixel of it at its own
/// proportions is one with the same 1:2 ratio. Fitting the frame into a square
/// viewport would squash the sky and double the foreground; cropping instead
/// would hide half the artwork. What is left of the content width is the page
/// fill, which is the surface tone rather than stretched pixels.
const DETAIL_VIEW_W: usize = (W - 2 * MARGIN) / 2;
/// The viewport's left edge: centred in the content width.
const DETAIL_VIEW_X: usize = (W - DETAIL_VIEW_W) / 2;
/// The inspection view's close control, named so the renderer and the tap
/// handler test the same rectangle rather than two copies of its numbers.
const CLOSE: Rect = Rect::new(232, 856, 256, 56);

const PILL_LABELS: [&[u8]; 3] = [b"All", b"Wallpapers", b"Photos"];
const NAV_LABELS: [&[u8]; 3] = [b"Photos", b"Albums", b"Places"];

// -----------------------------------------------------------------------------
// Text rendering helpers - the gate always matches the glyph line box, so
// glyphs are never clipped mid-row.
// -----------------------------------------------------------------------------
#[inline(always)]
fn text(
    x: usize,
    y: usize,
    x0: usize,
    y0: usize,
    scale: usize,
    s: &[u8],
    color: u32,
    bg: u32,
) -> Option<u32> {
    if y < y0 || y >= y0 + font::line_height(scale) {
        return None;
    }
    // The outline face, chosen ONCE for the run rather than once per pixel.
    //
    // WHY the order matters here specifically: this funnel is called for every
    // pixel of every label, and a per-pixel choice would draw the glyph from
    // both faces at once -- the outline's edge with a hard bitmap ghost one
    // pixel off, on every string on the surface. The launcher's first version of
    // this change did exactly that and it was only visible in the capture.
    if let Some(run) =
        outline_text::OutlineRun::place_run(x0, y0 + font::BASELINE * scale, scale, s, false)
    {
        // WHY the window is the run's and not the cell's: at the em that matches
        // this scale the outline's ascent reaches 18px above the cell origin and
        // its descent 4px past the bottom, so the cell-shaped test above would
        // clip tall glyphs off the top without anything looking wrong. The run's
        // own bounds admit exactly its ink and nothing more, so widening to them
        // cannot put a pixel where a glyph is not.
        let (rx, rtop, rw, rh) = run.bounds();
        if x >= rx && x - rx < rw && y >= rtop && y - rtop < rh {
            return run.pixel(x, y, color, bg);
        }
        // Inside the cell window but outside the run's ink: the caller's
        // background shows through, exactly as an uninked bitmap pixel would.
        return None;
    }
    // The bitmap face is the fallback, not the baseline: a refusal here -- a
    // bold run, a glyph whose ink leaves the window, a run wider than the
    // buffer, a baseline too near the top of the surface -- must leave the text
    // drawn rather than dropped.
    font::test_text_aa(x, y, x0, y0, scale, s, color, bg, false)
}

#[inline(always)]
fn text_bold(
    x: usize,
    y: usize,
    x0: usize,
    y0: usize,
    scale: usize,
    s: &[u8],
    color: u32,
    bg: u32,
) -> Option<u32> {
    if y < y0 || y >= y0 + font::line_height(scale) {
        return None;
    }
    font::test_text_aa(x, y, x0, y0, scale, s, color, bg, true)
}

/// Horizontally centred text inside `[x0, x0 + max_w)`, measured with the real
/// font metrics so it can never run past its container.
///
/// WHY the measured width is the DRAWN face's and not `font::measure`'s: the run
/// is drawn through `text`, which prefers the outline face at scale 2 and above,
/// and that face advances by its own table rather than the bitmap cell's. Both
/// widths are decided by `outline_text::drawn_advance`, which mirrors
/// `place_run`'s refusals, so a label is centred on the width of the run that is
/// actually inked. On the bitmap fallback the answer is `font::measure`, which
/// is what the bitmap rasteriser steps by. The ledger's title was 3px left of
/// the surface's centre on the bitmap width (measured in capture
/// `g2_gal_wall.ppm`: ink x 264..451 for a centre of 360), and every line under
/// it is scale 1, so it was the only one on this screen that moved.
#[inline(always)]
fn text_centered(
    x: usize,
    y: usize,
    x0: usize,
    y0: usize,
    max_w: usize,
    scale: usize,
    s: &[u8],
    color: u32,
    bg: u32,
) -> Option<u32> {
    let w = font::measure(s, scale);
    if w > max_w {
        return None;
    }
    let drawn = outline_text::drawn_advance(s, scale).unwrap_or(w);
    text(x, y, x0 + (max_w.saturating_sub(drawn)) / 2, y0, scale, s, color, bg)
}

// -----------------------------------------------------------------------------
// Runtime number formatting.
//
// WHY hand-rolled: this is a `no_std` binary with no allocator, and every count
// it shows (entries listed, image files found, paths not readable) only exists
// after a syscall answers. These two helpers build those measured strings into
// stack buffers at boot, so the render path never formats anything.
// -----------------------------------------------------------------------------
/// Appends `bytes` to `buf` at `n` and returns the new length, truncating
/// rather than overrunning.
fn append(buf: &mut [u8], n: usize, bytes: &[u8]) -> usize {
    let take = bytes.len().min(buf.len().saturating_sub(n));
    buf[n..n + take].copy_from_slice(&bytes[..take]);
    n + take
}

/// Appends the decimal form of `value` to `buf` at `n`.
fn append_u64(buf: &mut [u8], mut n: usize, mut value: u64) -> usize {
    if value == 0 {
        return append(buf, n, b"0");
    }
    let mut digits = [0u8; 20];
    let mut len = 0usize;
    while value > 0 && len < 20 {
        digits[len] = b'0' + (value % 10) as u8;
        value /= 10;
        len += 1;
    }
    while len > 0 {
        len -= 1;
        if n < buf.len() {
            buf[n] = digits[len];
            n += 1;
        }
    }
    n
}

/// Builds `prefix ++ decimal(value) ++ suffix` and logs it.
fn log_counted(prefix: &str, value: u64, suffix: &str) {
    let mut buf = [0u8; 192];
    let mut n = append(&mut buf, 0, prefix.as_bytes());
    n = append_u64(&mut buf, n, value);
    n = append(&mut buf, n, suffix.as_bytes());
    log_line(&buf[..n]);
}

/// Logs one assembled ASCII line, refusing a non-UTF-8 one rather than passing
/// `from_utf8_unchecked` to the kernel's logger.
fn log_line(bytes: &[u8]) {
    if let Ok(line) = core::str::from_utf8(bytes) {
        let _ = std::log(0, line);
    }
}

// -----------------------------------------------------------------------------
// Photo Record Definitions
// -----------------------------------------------------------------------------
#[derive(Debug, Clone, Copy)]
pub struct PhotoItem {
    pub title: &'static str,
    pub date: &'static str,
    pub meta: &'static str,
    pub primary_color: u32,
    pub secondary_color: u32,
}

/// The device photo ledger. It is populated only from real media storage, which
/// currently exposes no images, so the ledger is honestly empty.
///
/// WHY this stays an empty array rather than being filled from the wallpaper
/// catalogue: a wallpaper is the device's own artwork, not a photograph the user
/// took or stored, and listing one here would inflate the photo count the
/// header reports. The wallpapers are shown through `Media::Wallpaper` and
/// `walls::SCENES`, which is what they actually are.
pub static PHOTOS: [PhotoItem; 0] = [];

/// Whether the photo ledger holds no record.
#[inline]
fn ledger_is_empty() -> bool {
    PHOTOS.is_empty()
}

/// Whether the device offers any wallpaper at all.
#[inline]
fn wallpapers_available() -> bool {
    !walls::SCENES.is_empty()
}

/// Whether the segmented filter chips can select anything at all.
///
/// WHY derived from the collections and not a hand-set flag: the row filters
/// real sets -- the wallpaper catalogue and the photo ledger -- so it is live
/// while either of them holds something. Both empty would mean there is nothing
/// on this device to filter, and the chips would only select the same empty set
/// three ways; in that state they are drawn recessed and are not hit-tested,
/// exactly as they were when the ledger was the only source.
#[inline]
fn filters_available() -> bool {
    wallpapers_available() || !ledger_is_empty()
}

/// How many bottom-bar destinations this build can actually reach.
///
/// WHY 1 and not 3: nothing bound to this app returns an album list or a place
/// list - the VFS exposes no such index and the platform binds no service that
/// does - so `Albums` and `Places` had no screen behind them. The wallpaper
/// catalogue does not change this: it is a set on THIS screen, selected by the
/// filter chips above, not a destination of its own. Entries at or past this
/// index are drawn recessed and are not hit-tested.
const REAL_SECTIONS: usize = 1;

/// Ink for an affordance that is painted but has no capability behind it.
///
/// WHY the border tone and not a grey invented here: it is the design system's
/// own hairline, so an unavailable control reads as recessed chrome against both
/// the page fill and a card, and it stays in step with the rest of the shell if
/// the hairline token ever moves.
const DISABLED: u32 = BORDER;

// -----------------------------------------------------------------------------
// Media model
// -----------------------------------------------------------------------------
/// One image this gallery can show.
///
/// WHY public: `GalleryApp` carries one in a public field, so the variant has to
/// be at least as visible as the field that names it.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Media {
    /// An entry of the device's wallpaper catalogue: real scene data, rendered
    /// by the guest itself.
    Wallpaper(usize),
    /// A record of the photo ledger. No value of this variant exists today
    /// because `PHOTOS` is empty; the arm is kept so the ledger's path through
    /// the viewer is not deleted while it is unreachable.
    Photo(usize),
}

/// Which set the filter row selects. Public for the same reason as `Media`.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Filter {
    All,
    Wallpapers,
    Photos,
}

impl Filter {
    fn from_index(index: usize) -> Filter {
        match index {
            1 => Filter::Wallpapers,
            2 => Filter::Photos,
            _ => Filter::All,
        }
    }

    /// Whether this filter admits the wallpaper catalogue.
    fn shows_wallpapers(self) -> bool {
        matches!(self, Filter::All | Filter::Wallpapers)
    }

    /// Whether this filter admits the photo ledger.
    fn shows_ledger(self) -> bool {
        matches!(self, Filter::All | Filter::Photos)
    }
}

/// The media item at a flat grid index, wallpapers first.
///
/// WHY a flat index: the grid is one list rendered in three columns, so a cell
/// and a media record must resolve through the same numbering or the tile and
/// the thing it opens disagree.
fn media_at(index: usize) -> Option<Media> {
    if index < walls::SCENES.len() {
        return Some(Media::Wallpaper(index));
    }
    let photo = index - walls::SCENES.len();
    if photo < PHOTOS.len() {
        return Some(Media::Photo(photo));
    }
    None
}

/// Rows the wallpaper set occupies in the grid.
///
/// WHY a ceiling: the catalogue is not obliged to be a multiple of the column
/// count, so the honest ledger note has to start below whatever row the last
/// wallpaper actually lands on.
fn wallpaper_rows() -> usize {
    (walls::SCENES.len() + 2) / 3
}

/// The media a grid cell holds under the current filter, if any.
fn cell_media(app: &GalleryApp, row: usize, col: usize) -> Option<Media> {
    let media = media_at(row * 3 + col)?;
    match media {
        Media::Wallpaper(_) if app.filter.shows_wallpapers() => Some(media),
        Media::Photo(_) if app.filter.shows_ledger() => Some(media),
        _ => None,
    }
}

// -----------------------------------------------------------------------------
// The measured media scan.
//
// The gallery's photo ledger is only ever filled from image files that actually
// exist on this device, so the app asks the guest filesystem what it holds
// rather than assuming. `fs_list` is read-only and gated on the same
// `FileSystemRead` capability the Files app uses, so the scan changes nothing
// and needs no authority this app does not already have.
// -----------------------------------------------------------------------------
/// Directory paths the scan asks about.
///
/// WHY this list and no other: `Vfs::list_dir_for_task` -> `enforce_app_path`
/// admits a read of `/system`, `/system/...`, `/packages`, `/packages/...` and
/// the calling task's own storage root, and refuses every other path. These are
/// therefore the whole set of directories this app is entitled to look at.
/// `/data` and `/user` are asked anyway, because their refusal is a real
/// measured answer about this device's sandbox and is reported as one.
const SCAN_PATHS: [&str; 6] = [
    "/system",
    "/system/etc",
    "/system/bin",
    "/packages",
    "/data",
    "/user",
];

/// This task's package id in the kernel's first-party table.
///
/// WHY the literal: `Vfs::app_storage_root` builds
/// `/user/{uid}/apps/{package_id}` from `kernel/src/package/mod.rs`, which maps
/// process name `app_gallery` to `org.interos.gallery`. If that mapping ever
/// moved, this path would come back "not found" and the scan would report that
/// rather than silently listing somewhere else.
const APP_STORAGE_PACKAGE: &str = "org.interos.gallery";

/// File extensions the scan counts as images.
///
/// WHY a suffix match on the name: the guest has no image decoder at all
/// (`sdk/rust/interos-std/src/design/walls.rs` opens with exactly that fact), so
/// there is nothing to sniff a magic number with. A name is the honest
/// available signal, and the count is reported as "image files by name", which
/// is what it is.
const IMAGE_SUFFIXES: [&[u8]; 6] = [
    b".jpg", b".jpeg", b".png", b".bmp", b".gif", b".webp",
];

/// What the guest filesystem actually holds, measured at boot.
#[derive(Clone, Copy)]
struct MediaScan {
    /// Directories that answered with a listing.
    listed: usize,
    /// Entries named across those directories.
    entries: usize,
    /// Entries whose name carries an image extension.
    images: usize,
    /// Paths that did not answer, comma separated, as they were asked.
    unreadable: [u8; 128],
    unreadable_len: usize,
}

/// Whether a `fs_list` entry name carries an image extension.
fn has_image_suffix(name: &[u8]) -> bool {
    IMAGE_SUFFIXES.iter().any(|suffix| {
        name.len() > suffix.len()
            && name[name.len() - suffix.len()..].eq_ignore_ascii_case(suffix)
    })
}

/// Counts the entries and the image files in one `fs_list` payload.
///
/// WHY only the first field of each line is read: the kernel writes
/// `name|size|type` per line (`Vfs::list_dir`), so the name runs up to the first
/// `|` and nothing after it is trusted for the extension test.
fn count_listing(payload: &[u8]) -> (usize, usize) {
    let mut entries = 0usize;
    let mut images = 0usize;
    for line in payload.split(|&b| b == b'\n') {
        if line.is_empty() {
            continue;
        }
        entries += 1;
        let end = line
            .iter()
            .position(|&b| b == b'|')
            .unwrap_or(line.len());
        if has_image_suffix(&line[..end]) {
            images += 1;
        }
    }
    (entries, images)
}

/// Builds this task's own storage root path for the live foreground user.
fn build_own_storage_path(uid: u32, out: &mut [u8; 128]) -> usize {
    let mut n = append(out, 0, b"/user/");
    n = append_u64(out, n, uid as u64);
    n = append(out, n, b"/apps/");
    append(out, n, APP_STORAGE_PACKAGE.as_bytes())
}

/// Asks the filesystem about one directory and folds the answer into `scan`.
///
/// Each path's own answer is logged as one assembled line, because the serial
/// log is where a per-path outcome belongs; the screen carries the totals.
fn probe_media_path(path: &str, scan: &mut MediaScan, out: &mut [u8; 4096]) {
    let mut line = [0u8; 256];
    let mut n = append(&mut line, 0, b"[GALLERY] media scan '");
    n = append(&mut line, n, path.as_bytes());
    match std::fs_list(fs_read_slot(), path, out) {
        Ok(bytes) => {
            scan.listed += 1;
            let (entries, images) = count_listing(&out[..bytes]);
            scan.entries += entries;
            scan.images += images;
            n = append(&mut line, n, b"': ");
            n = append_u64(&mut line, n, entries as u64);
            n = append(&mut line, n, b" entries, ");
            n = append_u64(&mut line, n, images as u64);
            n = append(&mut line, n, b" image files.");
            log_line(&line[..n]);
        }
        Err(code) => {
            // A path that does not answer is recorded by name so the on-screen
            // empty state can say exactly what it could not read.
            if scan.unreadable_len > 0 {
                scan.unreadable_len = append(&mut scan.unreadable, scan.unreadable_len, b", ");
            }
            scan.unreadable_len =
                append(&mut scan.unreadable, scan.unreadable_len, path.as_bytes());
            n = append(&mut line, n, b"' not readable, code -");
            n = append_u64(&mut line, n, code.unsigned_abs());
            n = append(&mut line, n, b".");
            log_line(&line[..n]);
        }
    }
}

/// Measures what the guest filesystem holds, and logs each path's own answer.
fn scan_media() -> MediaScan {
    let mut scan = MediaScan {
        listed: 0,
        entries: 0,
        images: 0,
        unreadable: [0u8; 128],
        unreadable_len: 0,
    };
    let mut out = [0u8; 4096];

    for path in SCAN_PATHS.iter().copied() {
        probe_media_path(path, &mut scan, &mut out);
    }

    // The app's own storage root is where images would legitimately arrive if
    // anything ever wrote one, so it is asked last, built from the live user.
    let mut root = [0u8; 128];
    let root_len = build_own_storage_path(std::user_get_current(), &mut root);
    if let Ok(root) = core::str::from_utf8(&root[..root_len]) {
        probe_media_path(root, &mut scan, &mut out);
    }

    scan
}

/// Renders the measured totals into `buf` and returns its length.
fn build_scan_line(scan: &MediaScan, buf: &mut [u8; 96]) -> usize {
    let mut n = append(buf, 0, b"Scanned ");
    n = append_u64(buf, n, scan.listed as u64);
    n = append(buf, n, b" paths: ");
    n = append_u64(buf, n, scan.entries as u64);
    n = append(buf, n, b" entries, ");
    n = append_u64(buf, n, scan.images as u64);
    append(buf, n, b" image files")
}

/// Renders the paths that did not answer into `buf` and returns its length.
fn build_unreadable_line(scan: &MediaScan, buf: &mut [u8; 128]) -> usize {
    if scan.unreadable_len == 0 {
        return append(buf, 0, b"Every scanned path answered");
    }
    let n = append(buf, 0, b"Not readable: ");
    append(buf, n, &scan.unreadable[..scan.unreadable_len])
}

/// Renders what the scan actually found, so the empty state cannot state a
/// count it did not measure.
///
/// WHY both arms: a zero-image scan is the common case, but a build that ever
/// booted with a stray image file present must not keep printing "no image
/// files" over a non-zero count. The second arm is also the honest ceiling of
/// what this app could do with one - the guest ships no image decoder at all,
/// so a named file would be a file it cannot read, not one it could show.
fn build_image_note(scan: &MediaScan, buf: &mut [u8; 72]) -> usize {
    if scan.images == 0 {
        return append(buf, 0, b"No image files in the readable paths.");
    }
    let mut n = append_u64(buf, 0, scan.images as u64);
    n = append(buf, n, b" image files by name; none decodable here.");
    n
}

/// Renders the device's real media summary into `buf`.
fn build_summary(buf: &mut [u8; 48]) -> usize {
    let mut n = append_u64(buf, 0, walls::SCENES.len() as u64);
    n = append(buf, n, b" wallpapers, ");
    n = append_u64(buf, n, PHOTOS.len() as u64);
    append(buf, n, b" photos on device")
}

/// Renders the wallpaper catalogue's real provenance into `buf`.
///
/// WHY the resolution is quoted from the catalogue itself: `SCENE_REF_W` and
/// `SCENE_REF_H` are the panels `tools/gen_walls.py` traced the reference
/// artwork at, so the number on screen is the catalogue's own measurement rather
/// than a claim this app makes about it.
fn build_catalogue_note(buf: &mut [u8; 72]) -> usize {
    let mut n = append(buf, 0, b"Each frame traced at ");
    n = append_u64(buf, n, walls::SCENE_REF_W as u64);
    n = append(buf, n, b"x");
    n = append_u64(buf, n, walls::SCENE_REF_H as u64);
    append(buf, n, b" reference columns")
}

// -----------------------------------------------------------------------------
// The wallpaper's applied state, read out of the persistent settings store.
// -----------------------------------------------------------------------------
/// Resolves one stored wallpaper key to a catalogue scene.
///
/// WHY the value is trimmed: the store is a newline-delimited text file, so a
/// value written by an older build can carry trailing whitespace, and the
/// catalogue resolves keys byte-exactly. A key the catalogue does not hold is
/// reported as "not chosen" rather than defaulted silently.
fn stored_scene_key(key: &str) -> Option<&'static walls::Scene> {
    let mut buf = [0u8; 16];
    match std::settings_get(fs_read_slot(), key, &mut buf) {
        Ok(n) if n > 0 && n <= buf.len() => {
            let raw = &buf[..n];
            let start = raw.iter().position(|b| !b.is_ascii_whitespace())?;
            let end = raw[start..]
                .iter()
                .position(|b| b.is_ascii_whitespace())
                .map(|e| start + e)
                .unwrap_or(raw.len());
            if end <= start {
                return None;
            }
            walls::Scene::by_key(&raw[start..end])
        }
        _ => None,
    }
}

/// Builds the inspection meta line for a wallpaper from the real store.
fn build_applied_label(
    scene: &walls::Scene,
    home: Option<&'static walls::Scene>,
    lock: Option<&'static walls::Scene>,
    buf: &mut [u8; 64],
) -> usize {
    let n = append(buf, 0, b"Wallpaper");
    match (home == Some(scene), lock == Some(scene)) {
        (true, true) => append(buf, n, b", applied on home and lock"),
        (true, false) => append(buf, n, b", applied on home"),
        (false, true) => append(buf, n, b", applied on lock"),
        (false, false) => append(buf, n, b", not chosen in Settings"),
    }
}

pub struct GalleryApp {
    pub filter: Filter,
    /// The media record open in the inspection view.
    pub open: Option<Media>,
    /// Title of the open record, taken from the record itself.
    pub detail_title: &'static [u8],
    /// Meta line of the open record, built when it was opened.
    pub detail_meta: [u8; 64],
    pub detail_meta_len: usize,
    /// Device media summary, measured once at boot.
    pub summary: [u8; 48],
    pub summary_len: usize,
    /// The measured media scan, pre-rendered.
    ///
    /// WHY buffers and not a recomputed string: the render funnel runs once per
    /// pixel, so a scan summary assembled inside it would be rebuilt over a
    /// million times a frame.
    pub scan_line: [u8; 96],
    pub scan_line_len: usize,
    pub image_note: [u8; 72],
    pub image_note_len: usize,
    pub unreadable_line: [u8; 128],
    pub unreadable_line_len: usize,
    /// Provenance of the wallpaper catalogue, pre-rendered.
    pub catalogue_note: [u8; 72],
    pub catalogue_note_len: usize,
}

impl GalleryApp {
    pub const fn new() -> Self {
        Self {
            filter: Filter::All,
            open: None,
            detail_title: b"",
            detail_meta: [0u8; 64],
            detail_meta_len: 0,
            summary: [0u8; 48],
            summary_len: 0,
            scan_line: [0u8; 96],
            scan_line_len: 0,
            image_note: [0u8; 72],
            image_note_len: 0,
            unreadable_line: [0u8; 128],
            unreadable_line_len: 0,
            catalogue_note: [0u8; 72],
            catalogue_note_len: 0,
        }
    }
}

// -----------------------------------------------------------------------------
// Vector Pictogram Engines (one rounded line family, 24-grid geometry)
// -----------------------------------------------------------------------------

/// Empty-state picture frame pictogram (frame + sun + ridge).
fn empty_frame(x: usize, y: usize, cx: usize, cy: usize) -> bool {
    let dx = x as isize - cx as isize;
    let dy = y as isize - cy as isize;
    if dx.abs() <= 60 && dy.abs() <= 48 {
        if dx.abs() >= 57 || dy.abs() >= 45 {
            return true;
        }
        let sun_dx = dx + 22;
        let sun_dy = dy + 16;
        if sun_dx * sun_dx + sun_dy * sun_dy <= 36 {
            return true;
        }
        if dy >= -6 && dy <= 40 {
            let m1 = (dx + 12).abs() * 2 <= (dy + 10) * 3;
            let m2 = (dx - 26).abs() * 2 <= (dy - 4) * 3;
            if m1 || m2 {
                return true;
            }
        }
    }
    false
}

/// Vector icon for the Photos tab (small picture frame).
fn tab_photos_icon(x: usize, y: usize, cx: usize, cy: usize) -> bool {
    let dx = x as isize - cx as isize;
    let dy = y as isize - cy as isize;
    if dx.abs() <= 12 && dy.abs() <= 10 {
        if dx.abs() >= 10 || dy.abs() >= 8 {
            return true;
        }
        if (dx + 5).abs() <= 2 && (dy + 3).abs() <= 2 {
            return true;
        }
        if dy >= 0 && dy <= 6 && (dx - 2).abs() <= dy {
            return true;
        }
    }
    false
}

/// Vector icon for the Albums tab (stacked album cards).
fn tab_albums_icon(x: usize, y: usize, cx: usize, cy: usize) -> bool {
    let dx = x as isize - cx as isize;
    let dy = y as isize - cy as isize;
    let front = (dx >= -8 && dx <= 10 && (dy == -6 || dy == 10))
        || (dy >= -6 && dy <= 10 && (dx == -8 || dx == 10));
    let back = (dx >= -12 && dx <= 6 && dy == -10)
        || (dy >= -10 && dy <= 6 && dx == -12);
    front || back
}

/// Vector icon for the Places tab (map pin marker).
fn tab_places_icon(x: usize, y: usize, cx: usize, cy: usize) -> bool {
    let dx = x as isize - cx as isize;
    let dy = y as isize - cy as isize;
    if dy <= 2 {
        let r2 = dx * dx + (dy + 3) * (dy + 3);
        if r2 <= 81 && r2 >= 25 {
            return true;
        }
    } else if dy <= 11 && dx.abs() <= (11 - dy) {
        return true;
    }
    false
}

/// Back chevron stroke (`half` px tall), apex pointing left, centred on (cx, cy).
fn chevron_left(x: usize, y: usize, cx: usize, cy: usize, half: isize) -> bool {
    let dx = (x as isize) - (cx as isize);
    let dy = (y as isize) - (cy as isize);
    let d = dy.abs();
    dx <= 0 && d <= half && (dx + d).abs() <= 2
}

// -----------------------------------------------------------------------------
// Wallpaper rendering.
//
// The catalogue is the one image source this guest genuinely owns, so the whole
// viewer is built over it: `walls::scene_pixel` is the same function the
// compositor uses for the lock and home surfaces, which means a pixel shown
// here is the pixel the shell draws, computed from the traced scene data and
// not from anything this app invented.
// -----------------------------------------------------------------------------
/// The row of the 720x1440 wallpaper frame a square crop starts on.
///
/// WHY derived from the scene's own band stops rather than the frame's middle:
/// the artwork's interest runs from the far ridge to the foreground, so a square
/// window centred on the midpoint between them shows the most of it. A fixed
/// window would cut the ridge off one scene and the foreground off another.
fn crop_origin_y(scene: &walls::Scene) -> usize {
    let mid = ((scene.horizon_y as usize + scene.rock_y as usize) * H) / (2 * walls::SCENE_REF_H);
    mid.saturating_sub(W / 2).min(H - W)
}

/// The wallpaper's own pixel for surface pixel `(x, y)` inside `rect`.
///
/// WHY the coordinates are projected onto the 720x1440 frame: that is the frame
/// the scene tables were traced for and the one the compositor draws, so
/// `scene_pixel` resolves exactly the artwork's pixel and the thumbnail, the
/// inspector and the lock screen agree. The `.min` clamps are belt-and-braces:
/// `rect` is inside the surface, so the projection cannot leave it, but a
/// rounding edge must not index past the frame either.
fn scene_pixel_in(scene: &walls::Scene, x: usize, y: usize, rect: Rect, crop_y: usize) -> u32 {
    let sx = (x - rect.x) * W / rect.w;
    let sy = crop_y + (y - rect.y) * W / rect.h;
    walls::scene_pixel(
        scene,
        sx.min(W - 1),
        sy.min(H - 1),
        W,
        H,
    )
}

/// The whole wallpaper frame, fitted into `rect`.
fn scene_fit(scene: &walls::Scene, x: usize, y: usize, rect: Rect) -> u32 {
    let sx = (x - rect.x) * W / rect.w;
    let sy = (y - rect.y) * H / rect.h;
    walls::scene_pixel(scene, sx.min(W - 1), sy.min(H - 1), W, H)
}

// -----------------------------------------------------------------------------
// Grid cell: photo records carry their own colours
// -----------------------------------------------------------------------------
#[inline(always)]
fn blend_argb(top: u32, bottom: u32, t: usize) -> u32 {
    let tr = ((top >> 16) & 0xFF) as isize;
    let tg = ((top >> 8) & 0xFF) as isize;
    let tb = (top & 0xFF) as isize;
    let br = ((bottom >> 16) & 0xFF) as isize;
    let bg = ((bottom >> 8) & 0xFF) as isize;
    let bb = (bottom & 0xFF) as isize;
    let ri = tr + (br - tr) * t as isize / 255;
    let gi = tg + (bg - tg) * t as isize / 255;
    let bi = tb + (bb - tb) * t as isize / 255;
    0xFF000000
        | ((ri.clamp(0, 255) as u32) << 16)
        | ((gi.clamp(0, 255) as u32) << 8)
        | (bi.clamp(0, 255) as u32)
}

/// One grid cell holding a wallpaper.
fn wallpaper_cell_pixel(x: usize, y: usize, col: usize) -> u32 {
    let scene = &walls::SCENES[col];
    let rect = Rect::new(COL_X[col], GRID_TOP, CELL, CELL);
    let Some(shape) = draw_rounded_rect(x, y, rect, radius::SMALL, SURFACE, Some(BORDER), 1) else {
        return BG;
    };
    if shape == BORDER {
        return BORDER;
    }
    // The image area is the cell minus its caption strip, and it is tested with
    // the same rounded-rectangle rasteriser so the artwork is clipped by the
    // card's corners rather than painting over them.
    let image = Rect::new(rect.x, rect.y, rect.w, rect.h - CAPTION_H);
    if draw_rounded_rect(x, y, image, radius::SMALL, SURFACE, None, 0).is_some() {
        return scene_pixel_in(scene, x, y, image, crop_origin_y(scene));
    }
    let caption = Rect::new(rect.x, rect.y + rect.h - CAPTION_H, rect.w, CAPTION_H);
    if caption.contains(x, y) {
        if let Some(c) = text_centered(
            x,
            y,
            caption.x,
            caption.y + (CAPTION_H - font::line_height(1)) / 2,
            caption.w,
            1,
            scene.name,
            TEXT,
            SURFACE,
        ) {
            return c;
        }
        return SURFACE;
    }
    SURFACE
}

/// One grid cell holding a photo-ledger record.
fn photo_cell_pixel(x: usize, y: usize, idx: usize) -> u32 {
    let Some(photo) = PHOTOS.get(idx) else {
        return BG;
    };
    let col = idx % 3;
    let row = idx / 3;
    let x0 = COL_X[col];
    let y0 = GRID_TOP + row * ROW_STRIDE;
    let Some(shape) = draw_rounded_rect(
        x,
        y,
        Rect::new(x0, y0, CELL, CELL),
        radius::SMALL,
        SURFACE,
        Some(BORDER),
        1,
    ) else {
        return BG;
    };
    if shape == BORDER {
        return BORDER;
    }
    let t = ((y - y0) * 255 / CELL.max(1)).min(255);
    blend_argb(photo.primary_color, photo.secondary_color, t)
}

fn item_pixel(x: usize, y: usize, media: Media) -> u32 {
    match media {
        Media::Wallpaper(col) => wallpaper_cell_pixel(x, y, col),
        Media::Photo(idx) => photo_cell_pixel(x, y, idx),
    }
}

// -----------------------------------------------------------------------------
// Segmented filter tabs - geometry derived from the real font metrics so the
// rendered pill and the hit-test are always the same rectangle.
// -----------------------------------------------------------------------------
fn pill_rect(index: usize) -> Rect {
    let mut x = MARGIN;
    for (i, label) in PILL_LABELS.iter().enumerate() {
        let w = (font::measure(label, 2) + PILL_PAD * 2).max(PILL_MIN_W);
        let rect = Rect::new(x, PILL_Y, w, PILL_H);
        if i == index {
            return rect;
        }
        x += w + PILL_GAP;
    }
    Rect::new(0, 0, 0, 0)
}

fn pill_pixel(x: usize, y: usize, app: &GalleryApp) -> Option<u32> {
    if y < PILL_Y || y >= PILL_Y + PILL_H {
        return None;
    }
    let available = filters_available();
    for (i, label) in PILL_LABELS.iter().enumerate() {
        let rect = pill_rect(i);
        if !rect.contains(x, y) {
            continue;
        }
        // WHY the three-way treatment: an available row keeps the live
        // segmented-control grammar (filled = selected, soft = at rest), and an
        // unavailable row drops onto the page fill with a hairline and muted ink,
        // which is how the rest of the shell paints something it cannot do.
        // It is not hit-tested in that state, so the two agree.
        let (fill, ink, border) = if !available {
            (BG, MUTED, Some(BORDER))
        } else if app.filter == Filter::from_index(i) {
            (TEXT, INVERSE, None)
        } else {
            (SOFT, TEXT, Some(BORDER))
        };
        let shape = draw_pill(x, y, rect, fill, border)?;
        if shape != fill {
            return Some(shape);
        }
        // WHY `drawn_advance` here too: the label is drawn by the same
        // `text` funnel the ledger title uses, so at scale 2 it lands on the
        // outline face and centring on the bitmap measure would leave it off
        // the pill's centre by the same 1-4% of its width. On the bitmap
        // fallback the answer is the measure the rasteriser steps by.
        let lw = outline_text::drawn_advance(label, 2).unwrap_or_else(|| font::measure(label, 2));
        let lx = rect.x + rect.w.saturating_sub(lw) / 2;
        if let Some(c) = text(x, y, lx, PILL_Y, 2, label, ink, fill) {
            return Some(c);
        }
        return Some(fill);
    }
    None
}

// -----------------------------------------------------------------------------
// Grid band
// -----------------------------------------------------------------------------
fn col_of(x: usize) -> Option<usize> {
    for (i, x0) in COL_X.iter().enumerate() {
        if x >= *x0 && x < *x0 + CELL {
            return Some(i);
        }
    }
    None
}

fn row_of(y: usize) -> Option<usize> {
    if y < GRID_TOP || y >= GRID_BOTTOM {
        return None;
    }
    let rel = y - GRID_TOP;
    let row = rel / ROW_STRIDE;
    if row >= ROWS || rel % ROW_STRIDE >= CELL {
        return None;
    }
    Some(row)
}

/// Height of the honest ledger note, from the pictogram's top row to the last
/// caption's bottom row.
const LEDGER_BLOCK_H: usize = 400;

/// Height of the catalogue provenance note: two measured scale-1 lines.
///
/// WHY `GLYPH_H` and not `line_height(1)`: the font module's line height is not a
/// `const fn`, and this constant is needed in a `const`. `line_height(scale)` is
/// exactly `GLYPH_H * scale`, so writing the cell height directly is the same
/// number with no second source of truth.
const CATALOGUE_BLOCK_H: usize = 2 * font::GLYPH_H;

/// Where the honest ledger note is drawn.
///
/// WHY it moves with the filter: the wallpaper set and the ledger are two
/// different sets, and the filter decides which of them are on screen. Under
/// `All` the wallpapers take the rows they occupy and the note is centred in
/// what is left; under `Photos` there is no wallpaper row to clear, so the note
/// takes the whole band. Both are the same note in the same place relative to
/// whatever it sits below.
fn ledger_block_y0(app: &GalleryApp) -> usize {
    let top = if app.filter.shows_wallpapers() {
        GRID_TOP + wallpaper_rows() * ROW_STRIDE
    } else {
        GRID_TOP
    };
    let available = GRID_BOTTOM.saturating_sub(top);
    top + available.saturating_sub(LEDGER_BLOCK_H) / 2
}

/// The honest photo-ledger state: nothing is tiled, and the screen says why.
///
/// WHY every line is here: this is the one surface a user meets when the device
/// holds no photographs, and each line is a measured fact about this device
/// rather than a plausible-sounding placeholder. The camera line is the HAL's
/// own answer - `kernel/src/hal/camera.rs` fixes `hardware_present: false` in a
/// `const fn` that nothing mutates, and its self-test logs "offline ...; failing
/// closed without synthetic frames" - and the file counts are the ones this
/// app's boot-time scan actually returned.
fn ledger_pixel(x: usize, y: usize, app: &GalleryApp) -> u32 {
    let y0 = ledger_block_y0(app);
    if empty_frame(x, y, 360, y0 + 120) {
        return MUTED;
    }
    let title = b"No photos";
    if let Some(c) = text_centered(x, y, MARGIN, y0 + 200, W - 2 * MARGIN, 2, title, TEXT, BG) {
        return c;
    }
    let camera_note = b"No camera hardware on this device.";
    if let Some(c) = text_centered(x, y, MARGIN, y0 + 280, W - 2 * MARGIN, 1, camera_note, MUTED, BG)
    {
        return c;
    }
    if let Some(c) = text_centered(
        x,
        y,
        MARGIN,
        y0 + 304,
        W - 2 * MARGIN,
        1,
        &app.image_note[..app.image_note_len],
        MUTED,
        BG,
    ) {
        return c;
    }
    if let Some(c) = text_centered(
        x,
        y,
        MARGIN,
        y0 + 328,
        W - 2 * MARGIN,
        1,
        &app.scan_line[..app.scan_line_len],
        TEXT,
        BG,
    ) {
        return c;
    }
    if let Some(c) = text_centered(
        x,
        y,
        MARGIN,
        y0 + 352,
        W - 2 * MARGIN,
        1,
        &app.unreadable_line[..app.unreadable_line_len],
        MUTED,
        BG,
    ) {
        return c;
    }
    // WHY this line is kept: it is the on-screen reason the two extra bottom-bar
    // entries are painted recessed. The filter chips above it are live, so the
    // note they used to carry is gone; this one still describes something the
    // app cannot do.
    let sections_note = b"Albums and Places need their own index.";
    if let Some(c) = text_centered(x, y, MARGIN, y0 + 376, W - 2 * MARGIN, 1, sections_note, MUTED, BG)
    {
        return c;
    }
    BG
}

/// Where the wallpaper catalogue's provenance note is drawn.
fn catalogue_block_y0() -> usize {
    let top = GRID_TOP + wallpaper_rows() * ROW_STRIDE;
    let available = GRID_BOTTOM.saturating_sub(top);
    top + available.saturating_sub(CATALOGUE_BLOCK_H) / 2
}

/// The wallpaper catalogue's own note, shown when the ledger is filtered out.
///
/// WHY a note rather than empty rows: under the `Wallpapers` filter there is
/// nothing else on this screen, and a band of blank rows would read as a
/// rendering fault. These two lines are the measured provenance of what IS on
/// screen - the device's summary and the resolution the scene tables were
/// traced at - so the empty space says something true instead of nothing.
fn catalogue_pixel(x: usize, y: usize, app: &GalleryApp) -> u32 {
    let y0 = catalogue_block_y0();
    if let Some(c) = text_centered(
        x,
        y,
        MARGIN,
        y0,
        W - 2 * MARGIN,
        1,
        &app.summary[..app.summary_len],
        TEXT,
        BG,
    ) {
        return c;
    }
    if let Some(c) = text_centered(
        x,
        y,
        MARGIN,
        y0 + font::line_height(1),
        W - 2 * MARGIN,
        1,
        &app.catalogue_note[..app.catalogue_note_len],
        MUTED,
        BG,
    ) {
        return c;
    }
    BG
}

fn grid_pixel(x: usize, y: usize, app: &GalleryApp) -> Option<u32> {
    if y < GRID_TOP || y >= GRID_BOTTOM {
        return None;
    }

    // 1. A cell the current filter fills holds a real image.
    if let (Some(col), Some(row)) = (col_of(x), row_of(y)) {
        if let Some(media) = cell_media(app, row, col) {
            return Some(item_pixel(x, y, media));
        }
    }

    // 2. What fills the rest of the band depends on which sets the filter
    //    admits: the honest ledger note when the ledger is on screen and holds
    //    no record, and the catalogue's provenance note when only the wallpapers
    //    are.
    if app.filter.shows_ledger() {
        if ledger_is_empty() {
            return Some(ledger_pixel(x, y, app));
        }
    } else {
        return Some(catalogue_pixel(x, y, app));
    }

    Some(BG)
}

// -----------------------------------------------------------------------------
// Bottom section bar
// -----------------------------------------------------------------------------
/// WHY the bar takes no `&GalleryApp`: with `REAL_SECTIONS == 1` there is
/// exactly one reachable destination, so a stored "selected tab" would always be
/// 0 and would only pretend to be state. The bar marks where this screen is, and
/// marks the destinations it cannot reach as recessed.
fn nav_pixel(x: usize, y: usize) -> Option<u32> {
    if y < NAV_Y0 || y >= NAV_Y1 {
        return None;
    }
    if y == NAV_Y0 {
        return Some(BORDER);
    }
    for (i, label) in NAV_LABELS.iter().enumerate() {
        let box_x0 = i * (W / 3);
        if x < box_x0 || x >= box_x0 + W / 3 {
            continue;
        }
        let cx = box_x0 + W / 6;
        // WHY the hairline ink past the real destinations: it is the same token
        // the unavailable filter chips use, so "painted but not actionable" looks
        // identical wherever the user meets it.
        let color = if i < REAL_SECTIONS { TEXT } else { DISABLED };
        let is_icon = match i {
            0 => tab_photos_icon(x, y, cx, NAV_ICON_CY),
            1 => tab_albums_icon(x, y, cx, NAV_ICON_CY),
            _ => tab_places_icon(x, y, cx, NAV_ICON_CY),
        };
        if is_icon {
            return Some(color);
        }
        if let Some(c) = text_centered(
            x,
            y,
            box_x0,
            NAV_LABEL_Y,
            W / 3,
            1,
            label,
            color,
            SURFACE,
        ) {
            return Some(c);
        }
        return Some(SURFACE);
    }
    Some(SURFACE)
}

// -----------------------------------------------------------------------------
// Media inspection view (light)
// -----------------------------------------------------------------------------
/// The inspection app bar's back control.
///
/// WHY a named constant: the old code wrote `Rect::new(MARGIN, 28, 56, 56)` in
/// the renderer and again, literally, in the tap handler. Two copies of a
/// geometry is two chances for the drawn pill and the hit box to disagree, and
/// 56 is the shell's primary action target, so it is taken from the scale rather
/// than retyped.
const DETAIL_BACK: Rect = Rect::new(MARGIN, 28, touch::PRIMARY_ACTION, touch::PRIMARY_ACTION);

fn detail_pixel(x: usize, y: usize, app: &GalleryApp, media: Media) -> u32 {
    // App bar: back control + record title/meta, each measured to fit.
    if y < DETAIL_BAR_H {
        if y == DETAIL_BAR_H - 1 {
            return BORDER;
        }
        if let Some(c) = draw_pill(x, y, DETAIL_BACK, SOFT, Some(BORDER)) {
            if c != SOFT {
                return c;
            }
            // WHY the centre is derived, not typed: the chevron apex sits at the
            // pill's left third and its vertical middle, both taken from
            // `DETAIL_BACK`, so moving the pill cannot orphan the glyph.
            let cx = DETAIL_BACK.x + DETAIL_BACK.w / 4;
            let cy = DETAIL_BACK.y + DETAIL_BACK.h / 2;
            if chevron_left(x, y, cx, cy, 12) {
                return TEXT;
            }
            return SOFT;
        }

        // WHY `DETAIL_BACK.x + DETAIL_BACK.w + spacing::GRID_16` rather than the
        // literal 96: the run starts a fixed gap past the right edge of the
        // control it sits beside, so the two cannot collide if the control moves.
        let text_x = DETAIL_BACK.x + DETAIL_BACK.w + spacing::GRID_16;
        let text_w = W - text_x - MARGIN;
        if y >= 28 && y < 28 + font::line_height(2) {
            let mut buf = [0u8; 48];
            let n = font::ellipsize(app.detail_title, 2, text_w, &mut buf);
            if let Some(c) = text(x, y, text_x, 28, 2, &buf[..n], TEXT, SURFACE) {
                return c;
            }
        }
        if y >= 80 && y < 80 + font::line_height(1) {
            let mut buf = [0u8; 64];
            let n = font::ellipsize(&app.detail_meta[..app.detail_meta_len], 1, text_w, &mut buf);
            if let Some(c) = text(x, y, text_x, 80, 1, &buf[..n], MUTED, SURFACE) {
                return c;
            }
        }
        return SURFACE;
    }

    // Image viewport.
    if y >= DETAIL_VIEW_Y0 && y < DETAIL_VIEW_Y1 && x >= MARGIN && x < W - MARGIN {
        let view = Rect::new(DETAIL_VIEW_X, DETAIL_VIEW_Y0, DETAIL_VIEW_W, DETAIL_VIEW_H);
        let shape = match draw_rounded_rect(x, y, view, radius::SMALL, BG, Some(BORDER), 1) {
            Some(c) => c,
            None => return BG,
        };
        if shape == BORDER {
            return BORDER;
        }
        match media {
            // The whole wallpaper frame, every pixel of it, through the same
            // rasteriser the compositor draws it with.
            Media::Wallpaper(idx) => {
                let scene = &walls::SCENES[idx];
                return scene_fit(scene, x, y, view);
            }
            // A ledger record's own colours, nothing fabricated.
            Media::Photo(idx) => {
                let Some(photo) = PHOTOS.get(idx) else {
                    return BG;
                };
                let t = ((y - DETAIL_VIEW_Y0) * 255 / (DETAIL_VIEW_Y1 - DETAIL_VIEW_Y0)).min(255);
                return blend_argb(photo.primary_color, photo.secondary_color, t);
            }
        }
    }

    // Close control: 256 x 56 pill, label centred on the real metrics.
    if let Some(c) = draw_pill(x, y, CLOSE, SOFT, Some(BORDER)) {
        if c != SOFT {
            return c;
        }
        let label = b"Close";
        // WHY `drawn_advance` and not the measure: the label is a scale-2 run on
        // the outline face, whose advance table is not the bitmap cell's, so the
        // bitmap measure put "Close" 1px left of this pill's centre. On the
        // bitmap fallback the measure is what the rasteriser steps by.
        let lw = outline_text::drawn_advance(label, 2).unwrap_or_else(|| font::measure(label, 2));
        let lx = CLOSE.x + (CLOSE.w.saturating_sub(lw)) / 2;
        // WHY the measured centring, kept: a 48px glyph cell inside a 56px pill
        // is 4px of slack top and bottom; anything less and the descenders of
        // the face clip against the pill edge.
        let ly = CLOSE.y + (CLOSE.h.saturating_sub(font::line_height(2))) / 2;
        if let Some(c) = text(x, y, lx, ly, 2, label, TEXT, SOFT) {
            return c;
        }
        return SOFT;
    }

    // Home gesture indicator.
    if HOME_PILL.contains(x, y) {
        return MUTED;
    }

    BG
}

// -----------------------------------------------------------------------------
// Main screen
// -----------------------------------------------------------------------------
#[inline(never)]
fn gallery_pixel(x: usize, y: usize, app: &GalleryApp) -> u32 {
    if let Some(media) = app.open {
        return detail_pixel(x, y, app, media);
    }

    // 1. Header: screen title + a real ledger summary.
    if y < HEADER_H {
        if let Some(c) = text_bold(x, y, MARGIN, TITLE_Y, 3, b"Photos", TEXT, BG) {
            return c;
        }
        if let Some(c) = text(
            x,
            y,
            MARGIN,
            SUBTITLE_Y,
            1,
            &app.summary[..app.summary_len],
            MUTED,
            BG,
        ) {
            return c;
        }
        return BG;
    }

    // 2. Segmented filter chips.
    if let Some(c) = pill_pixel(x, y, app) {
        return c;
    }

    // 3. Thumbnail grid band (or honest empty state).
    if let Some(c) = grid_pixel(x, y, app) {
        return c;
    }

    // 4. Bottom section bar.
    if let Some(c) = nav_pixel(x, y) {
        return c;
    }

    // 5. Home gesture indicator.
    if HOME_PILL.contains(x, y) {
        return MUTED;
    }

    BG
}

fn render_gallery_ui(surface: u32, app: &GalleryApp) {
    render_gallery_band(surface, app, 0, H);
}

/// Re-rasterises and writes only rows `y0..y1`.
///
/// WHY a band and not a rectangle: a row of the surface is one contiguous run in
/// the surface buffer, so `offset = y0 * W` writes a whole band with no stride
/// bookkeeping. Every band starts on a row boundary, so the running `x`/`y`
/// cursor below is exact.
///
/// WHY this is correct rather than a shortcut: the band is derived from what the
/// input actually changed, and the pixels outside it are correct because nothing
/// about them changed. The old renderer re-rasterised all 1,036,800 pixels on
/// every event, so a tap that changed nothing cost a full-screen pass.
fn render_gallery_band(surface: u32, app: &GalleryApp, y0: usize, y1: usize) {
    let mut chunk = [0u32; CHUNK];
    let mut offset = y0 * W;
    let end = y1.min(H) * W;
    while offset < end {
        let count = (end - offset).min(CHUNK);
        let mut x = offset % W;
        let mut y = offset / W;
        for i in 0..count {
            chunk[i] = gallery_pixel(x, y, app);
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
// Input handling. Every region below is hit-tested against the same `Rect` the
// renderer drew, so a control is accepted exactly where it is painted.
// -----------------------------------------------------------------------------

/// Which rows of the surface an input actually changed.
///
/// WHY this exists: the old loop called `render_gallery_ui` from every branch
/// that touched the model, which re-rasterised all 1,036,800 pixels - so a tap
/// on an empty grid, or on one of the two destinations with nothing behind it,
/// cost a full-screen pass to redraw an identical screen.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Dirty {
    /// Nothing changed; do not render at all.
    None,
    /// A filter chip was selected: the chip row and the grid band both moved.
    FilterRow,
    /// A media item was opened or closed: the whole screen is replaced.
    All,
}

/// Grow a dirty region so one rendered frame covers everything a poll batch
/// changed.
///
/// WHY: `event_poll` can deliver several taps before the app is scheduled again,
/// and each of them reports its own band. Rendering only the last one would
/// leave the earlier tap's pixels stale.
impl Dirty {
    fn widen(self, other: Dirty) -> Dirty {
        match (self, other) {
            (Dirty::None, d) | (d, Dirty::None) => d,
            (Dirty::FilterRow, Dirty::FilterRow) => Dirty::FilterRow,
            // Every other pair involves the whole screen, so it stays the whole
            // screen.
            _ => Dirty::All,
        }
    }
}

enum Tap {
    /// Nothing on screen moved; the surface is already correct.
    None,
    /// The model moved; render the named region.
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

/// Opens a media record in the inspection view, reading the real state its meta
/// line describes.
fn open_media(app: &mut GalleryApp, media: Media) -> Tap {
    app.open = Some(media);
    match media {
        Media::Wallpaper(idx) => {
            let scene = &walls::SCENES[idx];
            app.detail_title = scene.name;
            // WHY the store is read here and not cached at boot: the Settings app
            // writes these keys from another process, and the kernel has no
            // channel that would tell this app about a change. Reading at the
            // moment the item is opened is the only point at which the answer is
            // guaranteed fresh, and it costs one read of a 16-byte buffer.
            let home = stored_scene_key("ui.wallpaper.home");
            let lock = stored_scene_key("ui.wallpaper.lock");
            app.detail_meta_len = build_applied_label(scene, home, lock, &mut app.detail_meta);
            std::log(0, "[GALLERY] Wallpaper opened in inspection view.").ok();
        }
        Media::Photo(idx) => {
            // Unreachable while `PHOTOS` is empty, and kept so the ledger's path
            // through this viewer is not deleted.
            if let Some(photo) = PHOTOS.get(idx) {
                app.detail_title = photo.title.as_bytes();
                let mut n = append(&mut app.detail_meta, 0, b"Photo, ");
                n = append(&mut app.detail_meta, n, photo.date.as_bytes());
                n = append(&mut app.detail_meta, n, b", ");
                app.detail_meta_len = append(&mut app.detail_meta, n, photo.meta.as_bytes());
                std::log(0, "[GALLERY] Photo tapped: inspection view opened.").ok();
            }
        }
    }
    Tap::Redraw(Dirty::All)
}

fn handle_tap(app: &mut GalleryApp, x: usize, y: usize) -> Tap {
    // Bottom home gesture: closes the inspection view, else leaves the app.
    // WHY `HOME_HIT` and not the raw numbers: the strip that is hit-tested is the
    // same strip the indicator is drawn in, so the tap region can no longer be a
    // region nothing was ever painted on.
    if HOME_HIT.contains(x, y) {
        if app.open.is_some() {
            app.open = None;
            std::log(0, "[GALLERY] Exited photo inspection view.").ok();
            return Tap::Redraw(Dirty::All);
        }
        return Tap::Leave("[GALLERY] Home gesture pressed. Exiting to launcher.");
    }

    // Inspection view controls.
    if app.open.is_some() {
        if DETAIL_BACK.contains(x, y) || CLOSE.contains(x, y) {
            app.open = None;
            std::log(0, "[GALLERY] Closed photo inspection view.").ok();
            return Tap::Redraw(Dirty::All);
        }
        return Tap::None;
    }

    // Segmented filter chips - hit-tested only when they are drawn live.
    //
    // WHY the availability gate: `pill_pixel` draws the row recessed when
    // `filters_available()` is false, and a recessed control that still accepted
    // a tap would be exactly the dead affordance this replaces. The two states
    // are decided by the same function, so they cannot disagree.
    if filters_available() && y >= FILTER_BAND_Y0 && y < FILTER_BAND_Y1 {
        for i in 0..PILL_LABELS.len() {
            if pill_rect(i).contains(x, y) {
                let filter = Filter::from_index(i);
                if filter != app.filter {
                    app.filter = filter;
                    std::log(0, "[GALLERY] Gallery filter chip switched.").ok();
                    return Tap::Redraw(Dirty::FilterRow);
                }
                // Re-tapping the selected chip changes nothing, so nothing is
                // re-rasterised.
                return Tap::None;
            }
        }
        return Tap::None;
    }

    // Thumbnail grid taps.
    if y >= GRID_TOP && y < GRID_BOTTOM {
        if let (Some(col), Some(row)) = (col_of(x), row_of(y)) {
            if let Some(media) = cell_media(app, row, col) {
                return open_media(app, media);
            }
        }
        if ledger_is_empty() {
            std::log(
                0,
                "[GALLERY] Photo grid tapped: no media items in storage.",
            )
            .ok();
        }
        return Tap::None;
    }

    // The section bar marks where this screen is. `Albums` and `Places` have no
    // index behind them, so they are not controls and are not hit-tested - the
    // old code took the tap, stored a new index and repainted a label colour,
    // which is the same "looks live, goes nowhere" affordance.
    Tap::None
}

// -----------------------------------------------------------------------------
// Entry Point & Event Reactor Loop
// -----------------------------------------------------------------------------
#[no_mangle]
pub extern "C" fn _start() -> ! {
    std::log(0, "[GALLERY (Ring 3)] Gallery online.").ok();

    let mut app = GalleryApp::new();
    app.summary_len = build_summary(&mut app.summary);
    app.catalogue_note_len = build_catalogue_note(&mut app.catalogue_note);

    // Measure what this device actually holds before anything is drawn, so the
    // first frame already carries the real answer rather than a guess.
    let scan = scan_media();
    app.scan_line_len = build_scan_line(&scan, &mut app.scan_line);
    app.image_note_len = build_image_note(&scan, &mut app.image_note);
    app.unreadable_line_len = build_unreadable_line(&scan, &mut app.unreadable_line);
    log_counted(
        "[GALLERY] media scan: ",
        scan.listed as u64,
        " paths listed.",
    );
    log_counted(
        "[GALLERY] media scan: ",
        scan.entries as u64,
        " entries named across them.",
    );
    log_counted(
        "[GALLERY] media scan: ",
        scan.images as u64,
        " image files by name.",
    );

    // Allocate client surface (slot 6 = FrameBufferDirect, z=200)
    let surface = match std::surface_create(6, W as u32, H as u32, 200) {
        Ok(id) => {
            std::log(0, "[GALLERY] DRM/KMS Client Surface allocated (z=200).").ok();
            id
        }
        Err(_) => 0,
    };

    if surface > 0 {
        render_gallery_ui(surface, &app);
        let _ = std::surface_flip(6);
        std::log(0, "[GALLERY] Gallery UI presented to scanout buffer.").ok();
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
                    // Swipe up home gesture -> Return to launcher
                    if input.kind == 4
                        || input.kind == 6
                        || input.kind == 7
                        || input.kind == 8
                        || input.kind == 10
                    {
                        std::log(
                            0,
                            "[GALLERY] Navigation gesture recognized. Exiting to launcher.",
                        )
                        .ok();
                        exit_to_launcher(surface);
                    }

                    if input.kind == 1 {
                        match handle_tap(&mut app, input.x as usize, input.y as usize) {
                            Tap::Leave(msg) => {
                                std::log(0, msg).ok();
                                exit_to_launcher(surface);
                            }
                            Tap::Redraw(d) => dirty = dirty.widen(d),
                            Tap::None => {}
                        }
                    }
                }
                if dirty != Dirty::None && surface > 0 {
                    match dirty {
                        // A chip tap moves the chip row and, because the grid
                        // band is filtered, the grid rows as well. Both are
                        // re-rasterised and nothing else is: the header, the
                        // section bar and the home strip are all unchanged by a
                        // filter, so they keep the pixels they already have.
                        Dirty::FilterRow => {
                            render_gallery_band(surface, &app, FILTER_BAND_Y0, FILTER_BAND_Y1);
                            render_gallery_band(surface, &app, GRID_TOP, GRID_BOTTOM);
                        }
                        Dirty::All => render_gallery_ui(surface, &app),
                        Dirty::None => {}
                    }
                    let _ = std::surface_flip(6);
                }
            }
        }
        std::yield_now();
    }
}

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    std::log(0, "[GALLERY] Panic detected! Terminating task...").ok();
    std::exit();
}
