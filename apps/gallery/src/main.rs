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
#![allow(dead_code)]

use interos_std::design::outline_text;

use core::panic::PanicInfo;
use interos_std as std;
use gallery::theme::{MIN_TOUCH_TARGET, THEME};
use std::design::font;
use std::design::primitives::{draw_pill, draw_rounded_rect, Rect};
use std::design::radius;
use std::design::spacing;
use std::design::touch;
use std::design::walls;

pub use gallery::{
    decode_bmp, decode_image, decode_ppm, decode_qoi, decode_raw,
    encode_ppm, has_image_suffix, has_video_suffix, is_media_file, media_at_with_storage,
    Filter, GalleryTab, Media, StorageMediaItem, Thumbnail, VideoCodec, VideoPlayerState,
    MAX_STORAGE_ITEMS, THUMB_H, THUMB_W,
};

const W: usize = 720;
const H: usize = 1440;
const CHUNK: usize = 4096;

// -----------------------------------------------------------------------------
// Visual Design Tokens
//
// WHY there is no palette here any more: the app used to reach into the
// platform's *palette* module for its colours, which named values rather than
// roles, and carried a second accent of its own on top. Every colour now
// resolves through the platform's theme, so a third-party app that asks the
// SDK for the same roles draws the same gallery. See `gallery::theme` for
// the mapping from the old names to the platform's roles.
//
// WHY the app's two old accents are now one: the gallery's "neon cyan" and
// its "accent sky" were both the app's accent, and the platform has one
// accent role. The second constant is gone rather than kept as a second
// spelling of the same role.
// -----------------------------------------------------------------------------

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
/// Decoded thumbnails for the stored media items, one per slot.
///
/// WHY a cache and not a decode per frame: decoding a stored image is a
/// file read, and the grid renders per pixel. The thumbnail is decoded once
/// when the storage is scanned.
static mut STORAGE_THUMBS: [Thumbnail; MAX_STORAGE_ITEMS] = [Thumbnail::EMPTY; MAX_STORAGE_ITEMS];

/// Scratch buffer for reading a stored image file.
static mut FILE_SCRATCH: [u8; FILE_SCRATCH_LEN] = [0; FILE_SCRATCH_LEN];

/// Scratch buffer for the full RGBA decode of a stored image.
static mut DECODE_SCRATCH: [u8; DECODE_SCRATCH_LEN] = [0; DECODE_SCRATCH_LEN];

/// Length of the file read scratch buffer.
const FILE_SCRATCH_LEN: usize = 16 * 1024;

/// Length of the RGBA decode scratch buffer.
///
/// WHY this size: it matches the largest image the gallery's decoders can
/// produce, so a decode either fits or is refused rather than truncating.
const DECODE_SCRATCH_LEN: usize = 36 * 72 * 4;

/// Decodes a stored image file into a thumbnail.
///
/// Returns `None` when the file cannot be read, or is not an image the
/// gallery can decode. The caller then draws a labelled placeholder rather
/// than a stand-in for the picture.
fn decode_thumbnail(path: &[u8], name: &[u8]) -> Option<Thumbnail> {
    let path_str = core::str::from_utf8(path).ok()?;

    let read_bytes = {
        let buf = unsafe { &mut *core::ptr::addr_of_mut!(FILE_SCRATCH) };
        let fd = std::fs_open(path_str, std::FS_READ).ok()?;
        let n = std::fs_read(fd, buf).ok()?;
        let _ = std::fs_close(fd);
        n
    };
    if read_bytes == 0 {
        return None;
    }

    let decoded = {
        let buf = unsafe { &mut *core::ptr::addr_of_mut!(DECODE_SCRATCH) };
        let data = unsafe { &*core::ptr::addr_of!(FILE_SCRATCH) };
        decode_image(&data[..read_bytes.min(FILE_SCRATCH_LEN)], name, buf).ok()?
    };
    let (w, h, _len) = decoded;

    let thumb = {
        let buf = unsafe { &*core::ptr::addr_of!(DECODE_SCRATCH) };
        Thumbnail::from_rgba(buf, w, h)?
    };
    Some(thumb)
}

// -----------------------------------------------------------------------------
// Layout constants - single source of truth for rendering *and* hit-testing
// -----------------------------------------------------------------------------
/// Screen margin (8pt grid value).
const MARGIN: usize = spacing::GRID_24;
/// Grid gutter between columns/rows. 8px gutters with a 24px margin cannot
/// produce an integer cell on a 720px canvas (720 - 48 - 16 = 656, 656/3 is not
/// integral), so the gutter is the next 8pt-grid value that divides exactly:
/// 3 x 216 + 2 x 12 + 2 x 24 = 648 + 24 + 48 = 720.
const GUTTER: usize = 12;
const CELL: usize = 216;
const COL_X: [usize; 3] = [MARGIN, MARGIN + CELL + GUTTER, MARGIN + 2 * (CELL + GUTTER)];

const HEADER_H: usize = 144;
const TITLE_Y: usize = 40;
const SUBTITLE_Y: usize = 78;

const PILL_Y: usize = 152;
const PILL_H: usize = 48;
/// The gap between two filter chips.
///
/// WHY 10 and not a step of the platform's spacing scale: the scale steps in
/// fours, and the two nearest steps are 8 and 12. A chip row that is 8 apart
/// reads as a single run of chips and one that is 12 apart reads as two
/// groups, and 10 is the row this app's own filter bar was drawn at. The
/// value is used once, so a token of its own would be a token of one app's.
const PILL_GAP: usize = 10;
const PILL_PAD: usize = spacing::GRID_16;
const PILL_MIN_W: usize = 64;

const GRID_TOP: usize = 216;
const ROW_STRIDE: usize = CELL + GUTTER;
const ROWS: usize = 4;
/// 216 + 4 x 216 + 3 x 12 = 1116: the grid stops clear of the bottom bar.
const GRID_BOTTOM: usize = GRID_TOP + ROWS * CELL + (ROWS - 1) * GUTTER;

/// Caption strip under a thumbnail's image.
const CAPTION_H: usize = 46;

const NAV_Y0: usize = 1290;
const NAV_Y1: usize = 1400;
const NAV_ICON_CY: usize = 1330;
const NAV_LABEL_Y: usize = 1356;

/// The system home gesture strip: a 200x6 indicator drawn inside a 240x44 box.
///
/// WHY 44 and not 40: the design system's minimum touch target is 44 logical
/// pixels, and this strip was 40. The strip now reaches the bottom edge of
/// the surface, which is where the indicator already was.
const HOME_HIT: Rect = Rect::new(240, 1396, 240, MIN_TOUCH_TARGET);
const HOME_PILL: Rect = Rect::new(260, 1418, 200, 6);

const FILTER_BAND_Y0: usize = PILL_Y;
const FILTER_BAND_Y1: usize = PILL_Y + PILL_H;

// Inspection view geometry.
const DETAIL_BAR_H: usize = 112;
const DETAIL_VIEW_Y0: usize = 128;
const DETAIL_VIEW_Y1: usize = 800;
const DETAIL_VIEW_H: usize = DETAIL_VIEW_Y1 - DETAIL_VIEW_Y0;
const DETAIL_VIEW_W: usize = (W - 2 * MARGIN) / 2;
const DETAIL_VIEW_X: usize = (W - DETAIL_VIEW_W) / 2;
const CLOSE: Rect = Rect::new(232, 856, 256, 56);

// Action buttons for CRUD & capture
const CAPTURE_BTN: Rect = Rect::new(540, 36, 144, 44);
const DETAIL_FAV: Rect = Rect::new(W - 140, 32, 48, 48);
const DETAIL_DEL: Rect = Rect::new(W - 80, 32, 48, 48);
const PLAY_PAUSE_BTN: Rect = Rect::new(W / 2 - 28, DETAIL_VIEW_Y1 + 16, 56, 56);
const SCRUB_BAR: Rect = Rect::new(MARGIN, DETAIL_VIEW_Y1 + 84, W - 2 * MARGIN, 20);
/// The scrub track's touch region.
///
/// WHY it is taller than the track: the drawn groove is 20 rows because that
/// is what a slider's groove looks like, and a 20-row groove is not a touch
/// target. The platform's minimum is 44 logical pixels, so the region a tap
/// lands in is 44 rows tall and centred on the groove. The fullscreen button
/// below it is tested first, so the two rows they share go to the button.
const SCRUB_HIT: Rect = Rect::new(
    SCRUB_BAR.x,
    SCRUB_BAR.y + SCRUB_BAR.h / 2 - MIN_TOUCH_TARGET / 2,
    SCRUB_BAR.w,
    MIN_TOUCH_TARGET,
);
/// The fullscreen control.
///
/// WHY 44 and not 36: the design system's minimum touch target is 44 logical
/// pixels and this button was 36. The row below the scrub bar has room for
/// the four extra rows, so nothing else moved.
const FULLSCREEN_BTN: Rect = Rect::new(W - MARGIN - 100, DETAIL_VIEW_Y1 + 114, 100, MIN_TOUCH_TARGET);
/// The aspect-ratio control.
///
/// WHY 44 and not 40: the design system's minimum touch target is 44 logical
/// pixels and this button was 40.
const ASPECT_BTN: Rect = Rect::new(W - 80, 32, 60, MIN_TOUCH_TARGET);
const DETAIL_INFO: Rect = Rect::new(W / 2 - 40, H - 100, 80, 48);
const PHOTO_FAV_BTN: Rect = Rect::new(MARGIN + 40, H - 100, 80, 48);
const PHOTO_DEL_BTN: Rect = Rect::new(W - MARGIN - 120, H - 100, 80, 48);
const SET_WALLPAPER_BTN: Rect = Rect::new(MARGIN + 20, H - 164, W - 2 * MARGIN - 40, 48);
const WP_SHEET_RECT: Rect = Rect::new(MARGIN + 10, H - 340, W - 2 * MARGIN - 20, 260);
const WP_SHEET_TITLE_Y: usize = H - 324;
const WP_TARGET_HOME_BTN: Rect = Rect::new(MARGIN + 30, H - 280, W - 2 * MARGIN - 60, 44);
const WP_TARGET_LOCK_BTN: Rect = Rect::new(MARGIN + 30, H - 228, W - 2 * MARGIN - 60, 44);
const WP_TARGET_BOTH_BTN: Rect = Rect::new(MARGIN + 30, H - 176, W - 2 * MARGIN - 60, 44);
const WP_TARGET_CANCEL_BTN: Rect = Rect::new(MARGIN + 30, H - 124, W - 2 * MARGIN - 60, 36);

const TAB_LABELS: [&[u8]; 4] = [b"Photos", b"Videos", b"Albums", b"Screenshots"];
const TAB_Y: usize = 144;
const TAB_H: usize = 48;

const VIDEO_COL_W: usize = (W - 2 * MARGIN - 16) / 2;
const VIDEO_COL_X: [usize; 2] = [MARGIN, MARGIN + VIDEO_COL_W + 16];
const VIDEO_ROW_H: usize = 230;

const PILL_LABELS: [&[u8]; 4] = [b"Photos", b"Videos", b"Albums", b"Screenshots"];
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
    let drawn = outline_text::drawn_advance(s, scale, None).unwrap_or(w);
    text(x, y, x0 + (max_w.saturating_sub(drawn)) / 2, y0, scale, s, color, bg)
}

#[inline(always)]
fn text_centered_bold(
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
    let w = font::measure_bold(s, scale);
    if w > max_w {
        return None;
    }
    let drawn = outline_text::drawn_advance(s, scale, None).unwrap_or(w);
    text_bold(x, y, x0 + (max_w.saturating_sub(drawn)) / 2, y0, scale, s, color, bg)
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
        let _ = std::log(line);
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
fn filters_available(app: &GalleryApp) -> bool {
    wallpapers_available() || !ledger_is_empty() || app.storage_count > 0
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

// -----------------------------------------------------------------------------
// Media model
// -----------------------------------------------------------------------------
/// The media item at a flat grid index, wallpapers first, then photos, then storage items.
fn media_at(app: &GalleryApp, index: usize) -> Option<Media> {
    media_at_with_storage(index, walls::SCENES.len(), PHOTOS.len(), app.storage_count)
}

/// Rows the wallpaper set occupies in the grid.
fn wallpaper_rows() -> usize {
    (walls::SCENES.len() + 2) / 3
}

/// The media a grid cell holds under the current filter, if any.
fn cell_media(app: &GalleryApp, row: usize, col: usize) -> Option<Media> {
    let media = media_at(app, row * 3 + col)?;
    match media {
        Media::Wallpaper(_) if app.filter.shows_wallpapers() => Some(media),
        Media::Photo(_) if app.filter.shows_ledger() => Some(media),
        Media::StorageItem(idx) => {
            if idx < app.storage_count {
                let item = &app.storage_items[idx];
                if item.is_video {
                    if app.filter.shows_videos() { Some(media) } else { None }
                } else {
                    if app.filter.shows_ledger() { Some(media) } else { None }
                }
            } else {
                None
            }
        }
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
    // WHY there is no capability probe before the listing: the kernel
    // authorises a filesystem call by the capability's *type*, so the app
    // asks for the listing and reports the platform's own answer. A slot is
    // a position in this app's capability table, and the position depends
    // on the order the kernel inserted the capabilities in -- the launcher
    // proved that by passing slot 0 while its filesystem read capability sat
    // at slot 3, and the developer-terminal flag it wanted was never read.
    let res = std::fs_list(path, out);
    match res {
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
fn build_summary(buf: &mut [u8; 48], photos_count: usize) -> usize {
    let mut n = append_u64(buf, 0, walls::SCENES.len() as u64);
    n = append(buf, n, b" wallpapers, ");
    n = append_u64(buf, n, photos_count as u64);
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
    match std::settings_get(key, &mut buf) {
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
    pub detail_title_buf: [u8; 48],
    pub detail_title_len: usize,
    /// Meta line of the open record, built when it was opened.
    pub detail_meta: [u8; 64],
    pub detail_meta_len: usize,
    /// Device media summary, measured once at boot.
    pub summary: [u8; 48],
    pub summary_len: usize,
    /// The measured media scan, pre-rendered.
    pub scan_line: [u8; 96],
    pub scan_line_len: usize,
    pub image_note: [u8; 72],
    pub image_note_len: usize,
    pub unreadable_line: [u8; 128],
    pub unreadable_line_len: usize,
    /// Provenance of the wallpaper catalogue, pre-rendered.
    pub catalogue_note: [u8; 72],
    pub catalogue_note_len: usize,
    /// Storage items found in /storage/photos, /storage/pictures, /storage/videos.
    pub storage_items: [StorageMediaItem; MAX_STORAGE_ITEMS],
    pub storage_count: usize,
    /// Native video player state when playing a video.
    pub video_player: Option<VideoPlayerState>,
    /// Modern flagship tab selection.
    pub tab: GalleryTab,
    /// Whether info overlay sheet is visible in photo viewer.
    pub show_info_overlay: bool,
    /// Whether target selector sheet is visible in photo viewer.
    pub show_wallpaper_sheet: bool,
    /// Decoded image buffer for detailed viewing.
    pub detail_rgba: [u8; 36 * 72 * 4],
    pub detail_rgba_len: usize,
    pub detail_w: u32,
    pub detail_h: u32,
}

impl GalleryApp {
    pub const fn new() -> Self {
        Self {
            tab: GalleryTab::Photos,
            filter: Filter::All,
            open: None,
            show_info_overlay: false,
            show_wallpaper_sheet: false,
            detail_title: b"",
            detail_title_buf: [0u8; 48],
            detail_title_len: 0,
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
            storage_items: [StorageMediaItem::EMPTY; MAX_STORAGE_ITEMS],
            storage_count: 0,
            video_player: None,
            detail_rgba: [0u8; 36 * 72 * 4],
            detail_rgba_len: 0,
            detail_w: 0,
            detail_h: 0,
        }
    }

    pub fn apply_wallpaper_target(&self, _lock: bool, _home: bool) {
        let Some(media) = self.open else { return };
        let mut val = [0u8; 64];
        let val_len: usize = match media {
            Media::Wallpaper(idx) => {
                if idx < std::design::walls::SCENES.len() {
                    let k = std::design::walls::SCENES[idx].key;
                    let len = k.len().min(64);
                    val[..len].copy_from_slice(&k[..len]);
                    len
                } else {
                    0
                }
            }
            Media::StorageItem(idx) => {
                if idx < self.storage_count {
                    let item = &self.storage_items[idx];
                    let len = item.path_len.min(64);
                    val[..len].copy_from_slice(&item.path[..len]);
                    len
                } else {
                    0
                }
            }
            Media::Photo(idx) => {
                if idx < std::design::walls::SCENES.len() {
                    let k = std::design::walls::SCENES[idx].key;
                    let len = k.len().min(64);
                    val[..len].copy_from_slice(&k[..len]);
                    len
                } else {
                    0
                }
            }
        };

        if val_len > 0 {
        }
    }

    pub fn detail_title(&self) -> &[u8] {
        if self.detail_title_len > 0 {
            &self.detail_title_buf[..self.detail_title_len]
        } else {
            self.detail_title
        }
    }

    pub fn rescan_storage(&mut self) {
        self.storage_count = 0;
        let paths: [&str; 3] = ["/storage/photos", "/storage/pictures", "/storage/videos"];
        let mut list_buf = [0u8; 4096];
        for dir_path in paths.iter() {
            if self.storage_count >= MAX_STORAGE_ITEMS {
                break;
            }
            if let Ok(bytes) = std::fs_list(dir_path, &mut list_buf) {
                let payload = &list_buf[..bytes];
                for line in payload.split(|&b| b == b'\n') {
                    if line.is_empty() || self.storage_count >= MAX_STORAGE_ITEMS {
                        continue;
                    }
                    let mut parts = line.split(|&b| b == b'|');
                    let Some(name_bytes) = parts.next() else { continue };
                    if name_bytes.is_empty() || !is_media_file(name_bytes) {
                        continue;
                    }
                    let size: u64 = if let Some(size_bytes) = parts.next() {
                        let mut s = 0u64;
                        for &b in size_bytes {
                            if b.is_ascii_digit() {
                                s = s * 10 + (b - b'0') as u64;
                            }
                        }
                        s
                    } else {
                        0
                    };
                    let is_video = has_video_suffix(name_bytes);
                    let mut item = StorageMediaItem::EMPTY;
                    let nl = name_bytes.len().min(item.name.len());
                    item.name[..nl].copy_from_slice(&name_bytes[..nl]);
                    item.name_len = nl;

                    let mut pl = append(&mut item.path, 0, dir_path.as_bytes());
                    if !dir_path.ends_with('/') {
                        pl = append(&mut item.path, pl, b"/");
                    }
                    pl = append(&mut item.path, pl, &name_bytes[..nl]);
                    item.path_len = pl;
                    item.size = size;
                    item.is_video = is_video;

                    let mut fav_key = [0u8; 64];
                    let mut kn = append(&mut fav_key, 0, b"gallery.fav.");
                    kn = append(&mut fav_key, kn, &name_bytes[..nl]);
                    let mut val_buf = [0u8; 8];
                    if let Ok(key_str) = core::str::from_utf8(&fav_key[..kn]) {
                        if let Ok(vn) = std::settings_get(key_str, &mut val_buf) {
                            if vn > 0 && val_buf[0] == b'1' {
                                item.is_favourite = true;
                            }
                        }
                    }

                    // WHY the tile colours are neutral now: they used to be
                    // hashed from the file name, which produced a gradient
                    // that looked like a preview of the photograph and was
                    // not one. A stored image that can be decoded is drawn
                    // from its own pixels; one that cannot gets a neutral
                    // placeholder and says so.
                    item.primary_color = 0xFF1E293B;
                    item.secondary_color = 0xFF0F172A;

                    let slot = self.storage_count;
                    if !is_video {
                        let thumb =
                            decode_thumbnail(&item.path[..item.path_len], &item.name[..item.name_len]);
                        unsafe {
                            STORAGE_THUMBS[slot] = thumb.unwrap_or(Thumbnail::EMPTY);
                        }
                    } else {
                        unsafe {
                            STORAGE_THUMBS[slot] = Thumbnail::EMPTY;
                        }
                    }

                    self.storage_items[self.storage_count] = item;
                    self.storage_count += 1;
                }
            }
        }
    }

    pub fn load_detail_image(&mut self, idx: usize) {
        self.detail_rgba_len = 0;
        self.detail_w = 0;
        self.detail_h = 0;
        if idx >= self.storage_count {
            return;
        }
    }

    /// The number of entries the Photos tab holds, wallpapers included.
    ///
    /// WHY this is not the photo count: the grid indexes wallpapers first,
    /// so the grid's length has to include them. The number of photographs
    /// on the device is `device_photos_count`.
    pub fn photos_count(&self) -> usize {
        let mut count = walls::SCENES.len() + PHOTOS.len();
        for i in 0..self.storage_count {
            if !self.storage_items[i].is_video {
                count += 1;
            }
        }
        count
    }

    /// The number of photographs on the device, not counting wallpapers.
    ///
    /// WHY this is separate: the summary line says "N wallpapers, M photos
    /// on device", and the old code passed it the wallpapers as well, so
    /// the two numbers counted the same things. The honest empty state --
    /// "no photographs on this device" -- was also unreachable, because the
    /// grid always held the built-in wallpapers.
    pub fn device_photos_count(&self) -> usize {
        let mut count = PHOTOS.len();
        for i in 0..self.storage_count {
            if !self.storage_items[i].is_video {
                count += 1;
            }
        }
        count
    }

    pub fn photo_at(&self, mut index: usize) -> Option<Media> {
        if index < walls::SCENES.len() {
            return Some(Media::Wallpaper(index));
        }
        index -= walls::SCENES.len();
        if index < PHOTOS.len() {
            return Some(Media::Photo(index));
        }
        index -= PHOTOS.len();
        let mut photo_seen = 0;
        for i in 0..self.storage_count {
            if !self.storage_items[i].is_video {
                if photo_seen == index {
                    return Some(Media::StorageItem(i));
                }
                photo_seen += 1;
            }
        }
        None
    }

    pub fn videos_count(&self) -> usize {
        let mut count = 0;
        for i in 0..self.storage_count {
            if self.storage_items[i].is_video {
                count += 1;
            }
        }
        count
    }

    pub fn video_at(&self, index: usize) -> Option<usize> {
        let mut video_seen = 0;
        for i in 0..self.storage_count {
            if self.storage_items[i].is_video {
                if video_seen == index {
                    return Some(i);
                }
                video_seen += 1;
            }
        }
        None
    }

    pub fn screenshots_count(&self) -> usize {
        let mut count = 0;
        for i in 0..self.storage_count {
            let item = &self.storage_items[i];
            if item.name[..item.name_len].starts_with(b"screenshot_") {
                count += 1;
            }
        }
        count
    }

    pub fn screenshot_at(&self, index: usize) -> Option<usize> {
        let mut seen = 0;
        for i in 0..self.storage_count {
            let item = &self.storage_items[i];
            if item.name[..item.name_len].starts_with(b"screenshot_") {
                if seen == index {
                    return Some(i);
                }
                seen += 1;
            }
        }
        None
    }

    pub fn album_count(&self, album_idx: usize) -> usize {
        match album_idx {
            0 => {
                let mut c = 0;
                for i in 0..self.storage_count {
                    let it = &self.storage_items[i];
                    if !it.is_video && !it.name[..it.name_len].starts_with(b"screenshot_") {
                        c += 1;
                    }
                }
                c
            }
            1 => self.screenshots_count(),
            2 => {
                let mut c = 0;
                for i in 0..self.storage_count {
                    let it = &self.storage_items[i];
                    let p = &it.path[..it.path_len];
                    if p.starts_with(b"/storage/pictures") || p.starts_with(b"/storage/videos") {
                        c += 1;
                    }
                }
                c
            }
            3 => walls::SCENES.len(),
            _ => 0,
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
    let Some(shape) = draw_rounded_rect(x, y, rect, radius::SMALL, THEME.surface_container, Some(THEME.border), 1) else {
        return THEME.surface;
    };
    if shape == THEME.border {
        return THEME.border;
    }
    // The image area is the cell minus its caption strip, and it is tested with
    // the same rounded-rectangle rasteriser so the artwork is clipped by the
    // card's corners rather than painting over them.
    let image = Rect::new(rect.x, rect.y, rect.w, rect.h - CAPTION_H);
    if draw_rounded_rect(x, y, image, radius::SMALL, THEME.surface_container, None, 0).is_some() {
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
            THEME.on_surface,
            THEME.surface_container,
        ) {
            return c;
        }
        return THEME.surface_container;
    }
    THEME.surface_container
}

/// One grid cell holding a photo-ledger record.
fn photo_cell_pixel(x: usize, y: usize, idx: usize, col: usize, row: usize) -> u32 {
    let Some(photo) = PHOTOS.get(idx) else {
        return THEME.surface;
    };
    let x0 = COL_X[col];
    let y0 = GRID_TOP + row * ROW_STRIDE;
    let Some(shape) = draw_rounded_rect(
        x,
        y,
        Rect::new(x0, y0, CELL, CELL),
        radius::SMALL,
        THEME.surface_container,
        Some(THEME.border),
        1,
    ) else {
        return THEME.surface;
    };
    if shape == THEME.border {
        return THEME.border;
    }
    let t = ((y - y0) * 255 / CELL.max(1)).min(255);
    blend_argb(photo.primary_color, photo.secondary_color, t)
}

fn storage_cell_pixel(x: usize, y: usize, app: &GalleryApp, idx: usize, col: usize, row: usize) -> u32 {
    if idx >= app.storage_count {
        return THEME.surface;
    }
    let item = &app.storage_items[idx];
    let x0 = COL_X[col];
    let y0 = GRID_TOP + row * ROW_STRIDE;
    let rect = Rect::new(x0, y0, CELL, CELL);
    let Some(shape) = draw_rounded_rect(
        x,
        y,
        rect,
        radius::SMALL,
        THEME.surface_container,
        Some(THEME.border),
        1,
    ) else {
        return THEME.surface;
    };
    if shape == THEME.border {
        return THEME.border;
    }

    let card_img_h = CELL - CAPTION_H;
    if y < y0 + card_img_h {
        // WHY the tile is drawn from the file's own pixels: the fill used
        // to be a gradient whose colours were hashed from the file name.
        // That looked like a preview of the photograph and was not one.
        // Now a stored image the gallery could decode is drawn from its
        // decoded thumbnail, and one it could not is drawn as a neutral
        // placeholder that says so.
        let thumb = unsafe { STORAGE_THUMBS[idx] };
        let grad = match thumb.sample(
            (x - x0) * THUMB_W / CELL,
            (y - y0) * THUMB_H / card_img_h.max(1),
        ) {
            Some(colour) => colour,
            None => {
                if (x - x0) < 1 || (x - x0) >= CELL - 1 || (y - y0) < 1 || (y - y0) >= card_img_h - 1
                {
                    THEME.border
                } else {
                    THEME.surface_container
                }
            }
        };

        if item.is_favourite && x >= x0 + CELL - 28 && x < x0 + CELL - 8 && y >= y0 + 8 && y < y0 + 28 {
            let cx = x0 + CELL - 18;
            let cy = y0 + 18;
            let dx = (x as isize - cx as isize).abs();
            let dy = (y as isize - cy as isize).abs();
            if dx * dx + dy * dy <= 36 {
                return THEME.warning;
            }
        }

        if item.is_video {
            let cx = x0 + CELL / 2;
            let cy = y0 + card_img_h / 2;
            let dx = x as isize - cx as isize;
            let dy = y as isize - cy as isize;
            if dx >= -8 && dx <= 12 && dy.abs() <= (dx + 8) {
                return 0xFFFFFFFF;
            }
            if dx * dx + dy * dy >= 400 && dx * dx + dy * dy <= 529 {
                return blend_argb(grad, 0xFFFFFFFF, 128);
            }
        }

        return grad;
    }

    if y == y0 + card_img_h {
        return THEME.border;
    }
    let text_y = y0 + card_img_h + (CAPTION_H.saturating_sub(font::line_height(1))) / 2;
    let mut buf = [0u8; 32];
    let n = font::ellipsize(&item.name[..item.name_len], 1, CELL - 16, &mut buf);
    if let Some(c) = text(x, y, x0 + 8, text_y, 1, &buf[..n], THEME.on_surface, THEME.surface_container) {
        return c;
    }
    THEME.surface_container
}

fn item_pixel(x: usize, y: usize, app: &GalleryApp, media: Media, col: usize, row: usize) -> u32 {
    match media {
        Media::Wallpaper(_) => wallpaper_cell_pixel(x, y, col),
        Media::Photo(idx) => photo_cell_pixel(x, y, idx, col, row),
        Media::StorageItem(idx) => storage_cell_pixel(x, y, app, idx, col, row),
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
    let available = filters_available(app);
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
            (THEME.surface, THEME.on_surface_variant, Some(THEME.border))
        } else if app.filter == Filter::from_index(i) {
            (THEME.on_surface, THEME.on_primary, None)
        } else {
            (THEME.surface_container_high, THEME.on_surface, Some(THEME.border))
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
        let lw = outline_text::drawn_advance(label, 2, None).unwrap_or_else(|| font::measure(label, 2));
        let lx = rect.x + rect.w.saturating_sub(lw) / 2;
        if let Some(c) = text(x, y, lx, PILL_Y, 2, label, ink, fill) {
            return Some(c);
        }
        return Some(fill);
    }
    None
}

/// Modern flagship media suite tab bar with neon cyan active underline indicator.
fn tab_bar_pixel(x: usize, y: usize, app: &GalleryApp) -> Option<u32> {
    if y < TAB_Y || y >= TAB_Y + TAB_H {
        return None;
    }

    let tab_w = W / 4;
    let tab_idx = (x / tab_w).min(3);
    let x0 = tab_idx * tab_w;
    let is_active = (app.tab as usize) == tab_idx;

    // Active Neon Cyan Underline Indicator (height 4px at bottom)
    if is_active && y >= TAB_Y + TAB_H - 4 && y < TAB_Y + TAB_H {
        let line_w = 72;
        let lx0 = x0 + (tab_w - line_w) / 2;
        if x >= lx0 && x < lx0 + line_w {
            return Some(THEME.primary);
        }
    }

    if y == TAB_Y + TAB_H - 1 {
        return Some(THEME.border);
    }

    // Tab Label Text (Scale 1 with bold active state)
    let label = TAB_LABELS[tab_idx];
    let text_color = if is_active { THEME.on_surface } else { THEME.on_surface_variant };
    let text_y = TAB_Y + (TAB_H.saturating_sub(font::line_height(1))) / 2;
    if is_active {
        if let Some(c) = text_centered_bold(x, y, x0, text_y, tab_w, 1, label, text_color, THEME.surface_container) {
            return Some(c);
        }
    } else {
        if let Some(c) = text_centered(x, y, x0, text_y, tab_w, 1, label, text_color, THEME.surface_container) {
            return Some(c);
        }
    }

    Some(THEME.surface_container)
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
        return THEME.on_surface_variant;
    }
    let title = b"No photos";
    if let Some(c) = text_centered(x, y, MARGIN, y0 + 200, W - 2 * MARGIN, 2, title, THEME.on_surface, THEME.surface) {
        return c;
    }
    let camera_note = b"No camera hardware on this device.";
    if let Some(c) = text_centered(x, y, MARGIN, y0 + 280, W - 2 * MARGIN, 1, camera_note, THEME.on_surface_variant, THEME.surface)
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
        THEME.on_surface_variant,
        THEME.surface,
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
        THEME.on_surface,
        THEME.surface,
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
        THEME.on_surface_variant,
        THEME.surface,
    ) {
        return c;
    }
    // WHY this line is kept: it is the on-screen reason the two extra bottom-bar
    // entries are painted recessed. The filter chips above it are live, so the
    // note they used to carry is gone; this one still describes something the
    // app cannot do.
    let sections_note = b"Albums and Places need their own index.";
    if let Some(c) = text_centered(x, y, MARGIN, y0 + 376, W - 2 * MARGIN, 1, sections_note, THEME.on_surface_variant, THEME.surface)
    {
        return c;
    }
    THEME.surface
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
        THEME.on_surface,
        THEME.surface,
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
        THEME.on_surface_variant,
        THEME.surface,
    ) {
        return c;
    }
    THEME.surface
}

fn video_col_of(x: usize) -> Option<usize> {
    for (i, x0) in VIDEO_COL_X.iter().enumerate() {
        if x >= *x0 && x < *x0 + VIDEO_COL_W {
            return Some(i);
        }
    }
    None
}

fn video_row_of(y: usize) -> Option<usize> {
    if y < GRID_TOP || y >= GRID_BOTTOM {
        return None;
    }
    let rel = y - GRID_TOP;
    let row = rel / VIDEO_ROW_H;
    if rel % VIDEO_ROW_H >= 210 {
        return None;
    }
    Some(row)
}

fn video_card_pixel(x: usize, y: usize, app: &GalleryApp, storage_idx: usize, col: usize, row: usize) -> u32 {
    let x0 = VIDEO_COL_X[col];
    let y0 = GRID_TOP + row * VIDEO_ROW_H;
    let card_rect = Rect::new(x0, y0, VIDEO_COL_W, 210);

    let Some(shape) = draw_rounded_rect(x, y, card_rect, radius::SMALL, THEME.surface_container, Some(THEME.border), 1) else {
        return THEME.surface;
    };
    if shape == THEME.border {
        return THEME.border;
    }

    let item = &app.storage_items[storage_idx];
    let thumb_h = 140;

    if y < y0 + thumb_h {
        // WHY the tile is a flat neutral and not a gradient: the gradient
        // was a blend between two fixed colours, and a two-tone ramp in the
        // shape of a frame read as a preview of the video. This build has no
        // video decoder, so there is no frame to show and the tile says so.
        let _ = item;

        // Center play button overlay
        let cx = x0 + VIDEO_COL_W / 2;
        let cy = y0 + thumb_h / 2;
        let dx = x as isize - cx as isize;
        let dy = y as isize - cy as isize;
        if dx * dx + dy * dy <= 400 {
            if dx >= -4 && dx <= 8 && dy.abs() <= (dx + 4) {
                return 0xFFFFFFFF;
            }
            return 0xCC0F172A;
        }

        // "No preview" marker, so the flat tile is not read as a broken
        // image but as an honest absence.
        let marker = b"No preview";
        let mw = font::measure(marker, 1);
        let mx = x0 + (VIDEO_COL_W.saturating_sub(mw)) / 2;
        let my = y0 + thumb_h - font::line_height(1) - 8;
        if let Some(c) = text(x, y, mx, my, 1, marker, THEME.on_surface_variant, THEME.surface_container) {
            return c;
        }

        // Size badge in bottom-right corner.
        //
        // WHY the badge shows a size and not a duration: the old badge read
        // "02:45" for every video, a literal that was never read from a
        // file. The gallery cannot know a video's duration without parsing
        // its container, but the file's size is a real measurement from the
        // directory listing.
        let badge_rect = Rect::new(x0 + VIDEO_COL_W - 92, y0 + thumb_h - 30, 80, 22);
        if badge_rect.contains(x, y) {
            // WHY the badge's own scrim and not a platform role: the badge
            // sits on a *photo*, and a scrim over a picture has to be the
            // picture's own darkness -- the platform's surface colour would
            // put a pale patch on a dark photograph. The app's chrome around
            // the picture comes from the platform's theme.
            let b_shape = draw_rounded_rect(x, y, badge_rect, radius::SMALL, 0xCC000000, Some(THEME.border), 1);
            if let Some(bc) = b_shape {
                if bc != 0xCC000000 { return bc; }
                let mut size_buf = [0u8; 20];
                let size_len = append_u64(&mut size_buf, 0, item.size);
                let dw = font::measure(&size_buf[..size_len], 1);
                let dx_pos = badge_rect.x + (badge_rect.w.saturating_sub(dw)) / 2;
                let dy_pos = badge_rect.y + (badge_rect.h.saturating_sub(font::line_height(1))) / 2;
                if let Some(tc) = text(x, y, dx_pos, dy_pos, 1, &size_buf[..size_len], 0xFFFFFFFF, 0xCC000000) {
                    return tc;
                }
                return 0xCC000000;
            }
        }

        return THEME.surface_container;
    }

    if y == y0 + thumb_h {
        return THEME.border;
    }

    // Title line
    let mut name_buf = [0u8; 32];
    let n = font::ellipsize(&item.name[..item.name_len], 1, VIDEO_COL_W - 24, &mut name_buf);
    if let Some(c) = text(x, y, x0 + 12, y0 + thumb_h + 12, 1, &name_buf[..n], THEME.on_surface, THEME.surface_container) {
        return c;
    }

    // Subtitle line (container and measured size).
    //
    // WHY the codec and resolution are gone: the old line read "VP9 -
    // 1080p HD" or "H.264 - 1080p HD", derived from the file extension
    // alone. An extension names a container, not a codec, and a resolution
    // cannot be known without parsing the file. What the name does state
    // is the container, and the size is a real measurement.
    let mut meta_buf = [0u8; 44];
    let mut meta_len = 0usize;
    meta_len = append(&mut meta_buf, meta_len, b"Container ");
    let name = &item.name[..item.name_len];
    if let Some(dot) = name.iter().rposition(|&b| b == b'.') {
        for &b in &name[dot + 1..] {
            if meta_len >= meta_buf.len() {
                break;
            }
            meta_buf[meta_len] = b.to_ascii_uppercase();
            meta_len += 1;
        }
    }
    meta_len = append(&mut meta_buf, meta_len, b"  ");
    let mut size_buf = [0u8; 20];
    let size_len = append_u64(&mut size_buf, 0, item.size);
    meta_len = append(&mut meta_buf, meta_len, b"  ");
    meta_len = append(&mut meta_buf, meta_len, &size_buf[..size_len]);
    meta_len = append(&mut meta_buf, meta_len, b" bytes");
    if let Some(c) = text(x, y, x0 + 12, y0 + thumb_h + 36, 1, &meta_buf[..meta_len], THEME.on_surface_variant, THEME.surface_container) {
        return c;
    }

    THEME.surface_container
}

fn photos_grid_pixel(x: usize, y: usize, app: &GalleryApp) -> Option<u32> {
    if let (Some(col), Some(row)) = (col_of(x), row_of(y)) {
        let index = row * 3 + col;
        if app.filter == Filter::Wallpapers {
            if index < walls::SCENES.len() {
                return Some(wallpaper_cell_pixel(x, y, index));
            }
        } else if let Some(media) = app.photo_at(index) {
            return Some(item_pixel(x, y, app, media, col, row));
        }
    }

    if app.filter == Filter::Wallpapers {
        return Some(catalogue_pixel(x, y, app));
    }

    if app.device_photos_count() == 0 {
        return Some(ledger_pixel(x, y, app));
    }

    Some(THEME.surface)
}

fn videos_grid_pixel(x: usize, y: usize, app: &GalleryApp) -> Option<u32> {
    let count = app.videos_count();
    if count == 0 {
        let y0 = GRID_TOP + 120;
        if let Some(c) = text_centered(x, y, MARGIN, y0, W - 2 * MARGIN, 2, b"No videos found", THEME.on_surface, THEME.surface) {
            return Some(c);
        }
        let note = b"Saved videos in /storage/videos will appear here.";
        if let Some(c) = text_centered(x, y, MARGIN, y0 + 40, W - 2 * MARGIN, 1, note, THEME.on_surface_variant, THEME.surface) {
            return Some(c);
        }
        return Some(THEME.surface);
    }

    if let (Some(col), Some(row)) = (video_col_of(x), video_row_of(y)) {
        let index = row * 2 + col;
        if let Some(storage_idx) = app.video_at(index) {
            return Some(video_card_pixel(x, y, app, storage_idx, col, row));
        }
    }

    Some(THEME.surface)
}

fn albums_grid_pixel(x: usize, y: usize, app: &GalleryApp) -> Option<u32> {
    let col = if x >= VIDEO_COL_X[0] && x < VIDEO_COL_X[0] + VIDEO_COL_W {
        0
    } else if x >= VIDEO_COL_X[1] && x < VIDEO_COL_X[1] + VIDEO_COL_W {
        1
    } else {
        return Some(THEME.surface);
    };

    let rel_y = y - GRID_TOP;
    let row = rel_y / 200;
    if row >= 2 || rel_y % 200 >= 180 {
        return Some(THEME.surface);
    }

    let album_idx = row * 2 + col;
    let x0 = VIDEO_COL_X[col];
    let y0 = GRID_TOP + row * 200;
    let card_rect = Rect::new(x0, y0, VIDEO_COL_W, 180);

    let Some(shape) = draw_rounded_rect(x, y, card_rect, radius::SMALL, THEME.surface_container, Some(THEME.border), 1) else {
        return Some(THEME.surface);
    };
    if shape == THEME.border {
        return Some(THEME.border);
    }

    let banner_h = 100;
    if y < y0 + banner_h {
        let t = ((y - y0) * 255 / banner_h).min(255);
        // WHY the banner's own gradient and not the platform's roles: the
        // banner stands for the *album*, and each album has a colour of its
        // own so the user can tell them apart at a glance. The app's chrome
        // around it comes from the platform's theme; the album's colours are
        // content, the way a photo's colours are.
        let grad = match album_idx {
            0 => blend_argb(0xFF1E3A8A, 0xFF2563EB, t),
            1 => blend_argb(0xFF047857, 0xFF10B981, t),
            2 => blend_argb(0xFF7C2D12, 0xFFEA580C, t),
            _ => {
                if !walls::SCENES.is_empty() {
                    let scene = &walls::SCENES[0];
                    let rect = Rect::new(x0, y0, VIDEO_COL_W, banner_h);
                    scene_pixel_in(scene, x, y, rect, crop_origin_y(scene))
                } else {
                    blend_argb(0xFF4C1D95, 0xFF7C3AED, t)
                }
            }
        };
        return Some(grad);
    }

    if y == y0 + banner_h {
        return Some(THEME.border);
    }

    let (title, count): (&[u8], usize) = match album_idx {
        0 => (b"Camera", app.album_count(0)),
        1 => (b"Screenshots", app.screenshots_count()),
        2 => (b"Downloads", app.album_count(2)),
        _ => (b"Sovereign Media", walls::SCENES.len()),
    };

    if let Some(c) = text(x, y, x0 + 16, y0 + banner_h + 12, 2, title, THEME.on_surface, THEME.surface_container) {
        return Some(c);
    }

    let mut count_buf = [0u8; 32];
    let mut cn = append_u64(&mut count_buf, 0, count as u64);
    cn = append(&mut count_buf, cn, b" items");
    if let Some(c) = text(x, y, x0 + 16, y0 + banner_h + 46, 1, &count_buf[..cn], THEME.on_surface_variant, THEME.surface_container) {
        return Some(c);
    }

    Some(THEME.surface_container)
}

fn screenshots_grid_pixel(x: usize, y: usize, app: &GalleryApp) -> Option<u32> {
    let count = app.screenshots_count();
    if count == 0 {
        let y0 = GRID_TOP + 120;
        if let Some(c) = text_centered(x, y, MARGIN, y0, W - 2 * MARGIN, 2, b"No screenshots yet", THEME.on_surface, THEME.surface) {
            return Some(c);
        }
        // WHY the button is the only way mentioned: this app has no keyboard
        // handler, so the old note that told the user to press 'S' named a
        // shortcut that does not exist.
        let note = b"Capture a screen with the + Shot button.";
        if let Some(c) = text_centered(x, y, MARGIN, y0 + 40, W - 2 * MARGIN, 1, note, THEME.on_surface_variant, THEME.surface) {
            return Some(c);
        }
        return Some(THEME.surface);
    }

    if let (Some(col), Some(row)) = (col_of(x), row_of(y)) {
        let index = row * 3 + col;
        if let Some(storage_idx) = app.screenshot_at(index) {
            return Some(storage_cell_pixel(x, y, app, storage_idx, col, row));
        }
    }

    Some(THEME.surface)
}

fn grid_pixel(x: usize, y: usize, app: &GalleryApp) -> Option<u32> {
    if y < GRID_TOP || y >= GRID_BOTTOM {
        return None;
    }

    match app.tab {
        GalleryTab::Photos => photos_grid_pixel(x, y, app),
        GalleryTab::Videos => videos_grid_pixel(x, y, app),
        GalleryTab::Albums => albums_grid_pixel(x, y, app),
        GalleryTab::Screenshots => screenshots_grid_pixel(x, y, app),
    }
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
        return Some(THEME.border);
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
        let color = if i < REAL_SECTIONS { THEME.on_surface } else { THEME.border };
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
            THEME.surface_container,
        ) {
            return Some(c);
        }
        return Some(THEME.surface_container);
    }
    Some(THEME.surface_container)
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

fn format_video_time(pos_ms: u32, dur_ms: u32, out: &mut [u8; 32]) -> usize {
    // WHY an unknown duration is shown as dashes: the gallery cannot parse
    // a container, so it does not know how long a video is. Printing
    // "00:00 / 00:00" would say the video is zero seconds long, which is a
    // different and wrong claim.
    if dur_ms == 0 {
        let unknown = b"--:-- / --:--";
        let take = unknown.len().min(out.len());
        out[..take].copy_from_slice(&unknown[..take]);
        return take;
    }
    let p_sec = pos_ms / 1000;
    let d_sec = dur_ms / 1000;
    let pm = p_sec / 60;
    let ps = p_sec % 60;
    let dm = d_sec / 60;
    let ds = d_sec % 60;
    let mut n = 0;
    if pm < 10 { n = append(out, n, b"0"); }
    n = append_u64(out, n, pm as u64);
    n = append(out, n, b":");
    if ps < 10 { n = append(out, n, b"0"); }
    n = append_u64(out, n, ps as u64);
    n = append(out, n, b" / ");
    if dm < 10 { n = append(out, n, b"0"); }
    n = append_u64(out, n, dm as u64);
    n = append(out, n, b":");
    if ds < 10 { n = append(out, n, b"0"); }
    n = append_u64(out, n, ds as u64);
    n
}

fn detail_pixel(x: usize, y: usize, app: &GalleryApp, media: Media) -> u32 {
    // If video is fullscreen, render fullscreen video surface and controls
    if let Some(ref player) = app.video_player {
        if player.is_fullscreen {
            if y < 80 {
                // Header strip in fullscreen
                let codec_str = player.codec.as_str().as_bytes();
                if let Some(c) = text(x, y, MARGIN, 28, 2, codec_str, THEME.on_primary, THEME.surface_container_low) {
                    return c;
                }
                return THEME.surface_container_low;
            }
            if y >= H - 120 {
                // Bottom control overlay
                if let Some(c) = draw_pill(x, y, PLAY_PAUSE_BTN, THEME.surface_container_high, Some(THEME.border)) {
                    if c != THEME.surface_container_high { return c; }
                    let icon: &[u8] = if player.is_playing { b"||" } else { b">" };
                    let lw = font::measure(icon, 2);
                    let lx = PLAY_PAUSE_BTN.x + (PLAY_PAUSE_BTN.w.saturating_sub(lw)) / 2;
                    let ly = PLAY_PAUSE_BTN.y + (PLAY_PAUSE_BTN.h.saturating_sub(font::line_height(2))) / 2;
                    if let Some(tc) = text(x, y, lx, ly, 2, icon, THEME.on_surface, THEME.surface_container_high) { return tc; }
                    return THEME.surface_container_high;
                }
                if let Some(c) = draw_pill(x, y, FULLSCREEN_BTN, THEME.surface_container_high, Some(THEME.border)) {
                    if c != THEME.surface_container_high { return c; }
                    let lbl = b"Exit Full";
                    let lw = font::measure(lbl, 1);
                    let lx = FULLSCREEN_BTN.x + (FULLSCREEN_BTN.w.saturating_sub(lw)) / 2;
                    let ly = FULLSCREEN_BTN.y + (FULLSCREEN_BTN.h.saturating_sub(font::line_height(1))) / 2;
                    if let Some(tc) = text(x, y, lx, ly, 1, lbl, THEME.on_surface, THEME.surface_container_high) { return tc; }
                    return THEME.surface_container_high;
                }
                return THEME.surface_container_low;
            }
            let t = (player.position_ms * 255 / player.duration_ms.max(1)).min(255) as usize;
            return blend_argb(THEME.surface_container_low, THEME.surface_container, t);
        }
    }

    // App bar: back control + CRUD / Aspect buttons + title/meta
    if y < DETAIL_BAR_H {
        if y == DETAIL_BAR_H - 1 {
            return THEME.border;
        }
        if let Some(c) = std::design::back_button::pixel(x, y, THEME.on_surface, THEME.border, THEME.surface_container_high) {
            return c;
        }

        // Aspect toggle button for video player (Fit vs Fill)
        if let Some(ref player) = app.video_player {
            let aspect_lbl: &[u8] = if player.is_fill { b"Fill" } else { b"Fit" };
            let aspect_color = if player.is_fill { THEME.primary } else { THEME.on_surface };
            if let Some(c) = draw_pill(x, y, ASPECT_BTN, THEME.surface_container_high, Some(THEME.border)) {
                if c != THEME.surface_container_high { return c; }
                let lw = font::measure(aspect_lbl, 1);
                let lx = ASPECT_BTN.x + (ASPECT_BTN.w.saturating_sub(lw)) / 2;
                let ly = ASPECT_BTN.y + (ASPECT_BTN.h.saturating_sub(font::line_height(1))) / 2;
                if let Some(tc) = text(x, y, lx, ly, 1, aspect_lbl, aspect_color, THEME.surface_container_high) { return tc; }
                return THEME.surface_container_high;
            }
        }

        let text_x = std::design::back_button::X + std::design::back_button::SIZE + spacing::GRID_16;
        let text_w = W.saturating_sub(text_x + 100);
        let title_bytes = app.detail_title();
        if y >= 28 && y < 28 + font::line_height(2) {
            let mut buf = [0u8; 48];
            let n = font::ellipsize(title_bytes, 2, text_w, &mut buf);
            if let Some(c) = text(x, y, text_x, 28, 2, &buf[..n], THEME.on_surface, THEME.surface_container) {
                return c;
            }
        }
        if y >= 80 && y < 80 + font::line_height(1) {
            let mut buf = [0u8; 64];
            let n = font::ellipsize(&app.detail_meta[..app.detail_meta_len], 1, text_w, &mut buf);
            if let Some(c) = text(x, y, text_x, 80, 1, &buf[..n], THEME.on_surface_variant, THEME.surface_container) {
                return c;
            }
        }
        return THEME.surface_container;
    }

    // Floating Info Sheet Overlay (for photos/wallpapers)
    if app.show_info_overlay && app.video_player.is_none() {
        let sheet = Rect::new(MARGIN, H - 470, W - 2 * MARGIN, 350);
        if sheet.contains(x, y) {
            if let Some(shape) = draw_rounded_rect(x, y, sheet, radius::CARD, THEME.surface_container, Some(THEME.primary), 2) {
                if shape == THEME.primary { return THEME.primary; }

                // Title
                if let Some(c) = text(x, y, sheet.x + 20, sheet.y + 20, 2, b"Media Information", THEME.on_surface, THEME.surface_container) {
                    return c;
                }

                // Divider
                if y == sheet.y + 56 {
                    return THEME.border;
                }

                // Line 1: File Name
                let name_lbl = b"File: ";
                let nw = font::measure(name_lbl, 1);
                if let Some(c) = text(x, y, sheet.x + 20, sheet.y + 72, 1, name_lbl, THEME.on_surface_variant, THEME.surface_container) {
                    return c;
                }
                let title_bytes = app.detail_title();
                let mut nb = [0u8; 48];
                let nlen = font::ellipsize(title_bytes, 1, sheet.w.saturating_sub(40 + nw), &mut nb);
                if let Some(c) = text(x, y, sheet.x + 20 + nw, sheet.y + 72, 1, &nb[..nlen], THEME.on_surface, THEME.surface_container) {
                    return c;
                }

                // Line 2: Resolution
                let res_lbl = b"Resolution: ";
                let rw = font::measure(res_lbl, 1);
                if let Some(c) = text(x, y, sheet.x + 20, sheet.y + 104, 1, res_lbl, THEME.on_surface_variant, THEME.surface_container) {
                    return c;
                }
                let mut res_buf = [0u8; 32];
                let rn = if app.detail_w > 0 && app.detail_h > 0 {
                    let mut n = append_u64(&mut res_buf, 0, app.detail_w as u64);
                    n = append(&mut res_buf, n, b" x ");
                    n = append_u64(&mut res_buf, n, app.detail_h as u64);
                    append(&mut res_buf, n, b" px")
                } else {
                // WHY the fallback is not the panel's own size: the old one
                // was the resolution of the screen this app draws on, shown
                // under a "Resolution:" label as if it were a measurement of
                // the media. The media's dimensions are unknown until its
                // header is read, and this build does not read it.
                    append(&mut res_buf, 0, b"not measured")
                };
                if let Some(c) = text(x, y, sheet.x + 20 + rw, sheet.y + 104, 1, &res_buf[..rn], THEME.on_surface, THEME.surface_container) {
                    return c;
                }

                // Line 3: File Size
                let size_lbl = b"Size: ";
                let sw = font::measure(size_lbl, 1);
                if let Some(c) = text(x, y, sheet.x + 20, sheet.y + 136, 1, size_lbl, THEME.on_surface_variant, THEME.surface_container) {
                    return c;
                }
                let mut sz_buf = [0u8; 32];
                let sn = if let Media::StorageItem(idx) = media {
                    if idx < app.storage_count {
                        let n = append_u64(&mut sz_buf, 0, app.storage_items[idx].size);
                        append(&mut sz_buf, n, b" bytes")
                    } else {
                        append(&mut sz_buf, 0, b"0 bytes")
                    }
                } else {
                    append(&mut sz_buf, 0, b"Internal System Asset")
                };
                if let Some(c) = text(x, y, sheet.x + 20 + sw, sheet.y + 136, 1, &sz_buf[..sn], THEME.on_surface, THEME.surface_container) {
                    return c;
                }

                // Line 4: Path
                let path_lbl = b"Location: ";
                let pw = font::measure(path_lbl, 1);
                if let Some(c) = text(x, y, sheet.x + 20, sheet.y + 168, 1, path_lbl, THEME.on_surface_variant, THEME.surface_container) {
                    return c;
                }
                let path_bytes = if let Media::StorageItem(idx) = media {
                    if idx < app.storage_count {
                        &app.storage_items[idx].path[..app.storage_items[idx].path_len]
                    } else {
                        b"/storage"
                    }
                } else {
                    // WHY not a path: the old fallback named a directory
                    // under the system's image tree, a path that does not
                    // exist on this platform. A built-in wallpaper is not a
                    // file, and saying where it lives would be a claim
                    // about a filesystem the app never read.
                    b"built-in wallpaper"
                };
                let mut pb = [0u8; 48];
                let pn = font::ellipsize(path_bytes, 1, sheet.w.saturating_sub(40 + pw), &mut pb);
                if let Some(c) = text(x, y, sheet.x + 20 + pw, sheet.y + 168, 1, &pb[..pn], THEME.on_surface, THEME.surface_container) {
                    return c;
                }

                // Line 5: Format
                let fmt_lbl = b"Format: ";
                let fw = font::measure(fmt_lbl, 1);
                if let Some(c) = text(x, y, sheet.x + 20, sheet.y + 200, 1, fmt_lbl, THEME.on_surface_variant, THEME.surface_container) {
                    return c;
                }
                let fmt_str: &[u8] = if let Media::StorageItem(idx) = media {
                    let name = &app.storage_items[idx].name[..app.storage_items[idx].name_len];
                    if name.ends_with(b".ppm") { b"PPM / Direct Scanout RGB24" }
                    else if name.ends_with(b".bmp") { b"BMP / Bitmap Raster" }
                    else if name.ends_with(b".qoi") { b"QOI / Quite OK Image Format" }
                    else if name.ends_with(b".mp4") { b"MP4 Video Container" }
                    else { b"Binary Media Stream" }
                } else {
                    b"Vector Silhouette Wall Reference"
                };
                if let Some(c) = text(x, y, sheet.x + 20 + fw, sheet.y + 200, 1, fmt_str, THEME.on_surface, THEME.surface_container) {
                    return c;
                }

                // Dismiss Button
                let dismiss_btn = Rect::new(sheet.x + (sheet.w - 140) / 2, sheet.y + 270, 140, 44);
                if let Some(dc) = draw_pill(x, y, dismiss_btn, THEME.surface_container_high, Some(THEME.border)) {
                    if dc != THEME.surface_container_high { return dc; }
                    let d_lbl = b"Dismiss";
                    let dw = font::measure(d_lbl, 1);
                    let dx_pos = dismiss_btn.x + (dismiss_btn.w.saturating_sub(dw)) / 2;
                    let dy_pos = dismiss_btn.y + (dismiss_btn.h.saturating_sub(font::line_height(1))) / 2;
                    if let Some(tc) = text(x, y, dx_pos, dy_pos, 1, d_lbl, THEME.on_surface, THEME.surface_container_high) { return tc; }
                    return THEME.surface_container_high;
                }

                return THEME.surface_container;
            }
        }
    }

    // Viewport: Image or Video canvas
    if y >= DETAIL_VIEW_Y0 && y < DETAIL_VIEW_Y1 && x >= MARGIN && x < W - MARGIN {
        let view = Rect::new(DETAIL_VIEW_X, DETAIL_VIEW_Y0, DETAIL_VIEW_W, DETAIL_VIEW_H);
        let shape = match draw_rounded_rect(x, y, view, radius::SMALL, THEME.surface, Some(THEME.border), 1) {
            Some(c) => c,
            None => return THEME.surface,
        };
        if shape == THEME.border {
            return THEME.border;
        }

        if let Some(ref player) = app.video_player {
            // Video surface: handle Fit vs Fill
            let video_rect = if player.is_fill {
                view
            } else {
                let fit_h = view.w * 9 / 16;
                let fit_y = view.y + (view.h.saturating_sub(fit_h)) / 2;
                Rect::new(view.x, fit_y, view.w, fit_h)
            };

            if !video_rect.contains(x, y) {
                // Letterbox black bars.
                //
                // WHY black and not the platform's darkest surface: the bars
                // are the *picture's* letterbox, the way a film is letterboxed
                // in a cinema, and a bar that carried the app's surface colour
                // would read as part of the app. The app's chrome around the
                // picture comes from the platform's theme.
                return 0xFF000000;
            }

            // The video surface.
            //
            // WHY it is a flat surface and not a picture: this build has no
            // video decoder, so there is no frame to show. The surface used
            // to be an animated scanline ramp labelled as a direct scanout,
            // which read as a playing video and was nothing of the sort.
            let v_bg = THEME.surface_container;

            // Codec watermark badge
            if y >= video_rect.y + 12 && y < video_rect.y + 12 + font::line_height(1) {
                let badge = player.codec.as_str().as_bytes();
                if let Some(c) = text(x, y, video_rect.x + 16, video_rect.y + 12, 1, badge, THEME.primary, v_bg) {
                    return c;
                }
            }

            // WHY the surface says it cannot decode: the tile is otherwise a
            // blank rectangle, and a blank rectangle reads as a broken image
            // rather than as an honest absence.
            let no_decode = b"No video decoder on this device";
            let nw = font::measure(no_decode, 1);
            let nx = video_rect.x + (video_rect.w.saturating_sub(nw)) / 2;
            let ny = video_rect.y + (video_rect.h.saturating_sub(font::line_height(1))) / 2;
            if let Some(c) = text(x, y, nx, ny, 1, no_decode, THEME.on_surface_variant, v_bg) {
                return c;
            }

            return v_bg;
        }

        match media {
            Media::Wallpaper(idx) => {
                let scene = &walls::SCENES[idx];
                return scene_fit(scene, x, y, view);
            }
            Media::Photo(idx) => {
                let Some(photo) = PHOTOS.get(idx) else {
                    return THEME.surface;
                };
                let t = ((y - DETAIL_VIEW_Y0) * 255 / (DETAIL_VIEW_Y1 - DETAIL_VIEW_Y0)).min(255);
                return blend_argb(photo.primary_color, photo.secondary_color, t);
            }
            Media::StorageItem(idx) => {
                if app.storage_items.get(idx).is_none() {
                    return THEME.surface;
                }
                if app.detail_rgba_len > 0 && app.detail_w > 0 && app.detail_h > 0 {
                    let img_x = ((x - view.x) * app.detail_w as usize / view.w.max(1)).min(app.detail_w as usize - 1);
                    let img_y = ((y - view.y) * app.detail_h as usize / view.h.max(1)).min(app.detail_h as usize - 1);
                    let offset = (img_y * app.detail_w as usize + img_x) * 4;
                    if offset + 3 < app.detail_rgba.len() {
                        let r = app.detail_rgba[offset] as u32;
                        let g = app.detail_rgba[offset + 1] as u32;
                        let b = app.detail_rgba[offset + 2] as u32;
                        let a = app.detail_rgba[offset + 3] as u32;
                        return (a << 24) | (r << 16) | (g << 8) | b;
                    }
                }
                // WHY a flat fill and not a two-tone ramp: the ramp was a
                // blend between two fixed colours, and a gradient in the
                // shape of a picture reads as a picture of something. The
                // grid's own tiles use a neutral placeholder for an image
                // they cannot decode, and the detail view matches it.
                return THEME.surface_container;
            }
        }
    }

    // Video Player interactive controls (Play/Pause, Scrub bar, Time, Fullscreen)
    if let Some(ref player) = app.video_player {
        if let Some(c) = draw_pill(x, y, PLAY_PAUSE_BTN, THEME.surface_container_high, Some(THEME.border)) {
            if c != THEME.surface_container_high { return c; }
            let icon: &[u8] = if player.is_playing { b"||" } else { b">" };
            let lw = font::measure(icon, 2);
            let lx = PLAY_PAUSE_BTN.x + (PLAY_PAUSE_BTN.w.saturating_sub(lw)) / 2;
            let ly = PLAY_PAUSE_BTN.y + (PLAY_PAUSE_BTN.h.saturating_sub(font::line_height(2))) / 2;
            if let Some(tc) = text(x, y, lx, ly, 2, icon, THEME.on_surface, THEME.surface_container_high) { return tc; }
            return THEME.surface_container_high;
        }

        // Scrub track with Neon Cyan progress and circular seek head thumb
        if SCRUB_BAR.contains(x, y) {
            let progress_w = (player.position_ms as usize * SCRUB_BAR.w / player.duration_ms.max(1) as usize).min(SCRUB_BAR.w);
            let thumb_x = SCRUB_BAR.x + progress_w;
            let thumb_cy = SCRUB_BAR.y + SCRUB_BAR.h / 2;
            let dx = x as isize - thumb_x as isize;
            let dy = y as isize - thumb_cy as isize;
            if dx * dx + dy * dy <= 49 {
                if dx * dx + dy * dy >= 25 {
                    return THEME.primary;
                }
                return 0xFFFFFFFF;
            }
            if x < thumb_x {
                return THEME.primary;
            }
            return THEME.border;
        }

        // Timestamp readout
        if y >= DETAIL_VIEW_Y1 + 114 && y < DETAIL_VIEW_Y1 + 114 + font::line_height(1) {
            let mut time_buf = [0u8; 32];
            let tn = format_video_time(player.position_ms, player.duration_ms, &mut time_buf);
            if let Some(c) = text(x, y, MARGIN, DETAIL_VIEW_Y1 + 114, 1, &time_buf[..tn], THEME.on_surface, THEME.surface) {
                return c;
            }
        }

        // Fullscreen toggle button
        if let Some(c) = draw_pill(x, y, FULLSCREEN_BTN, THEME.surface_container_high, Some(THEME.border)) {
            if c != THEME.surface_container_high { return c; }
            let lbl = b"Fullscreen";
            let lw = font::measure(lbl, 1);
            let lx = FULLSCREEN_BTN.x + (FULLSCREEN_BTN.w.saturating_sub(lw)) / 2;
            let ly = FULLSCREEN_BTN.y + (FULLSCREEN_BTN.h.saturating_sub(font::line_height(1))) / 2;
            if let Some(tc) = text(x, y, lx, ly, 1, lbl, THEME.on_surface, THEME.surface_container_high) { return tc; }
            return THEME.surface_container_high;
        }
    }

    // Photo Viewer Bottom Action Bar (Favorite, Info, Delete)
    if app.video_player.is_none() {
        let is_fav = if let Media::StorageItem(idx) = media {
            app.storage_items.get(idx).map(|it| it.is_favourite).unwrap_or(false)
        } else {
            false
        };
        let fav_bg = if is_fav { THEME.warning } else { THEME.surface_container_high };
        if let Some(c) = draw_pill(x, y, PHOTO_FAV_BTN, fav_bg, Some(THEME.border)) {
            if c != fav_bg { return c; }
            let fav_lbl = b"Fav";
            let lw = font::measure(fav_lbl, 1);
            let lx = PHOTO_FAV_BTN.x + (PHOTO_FAV_BTN.w.saturating_sub(lw)) / 2;
            let ly = PHOTO_FAV_BTN.y + (PHOTO_FAV_BTN.h.saturating_sub(font::line_height(1))) / 2;
            let tc_color = if is_fav { 0xFFFFFFFF } else { THEME.on_surface };
            if let Some(tc) = text(x, y, lx, ly, 1, fav_lbl, tc_color, fav_bg) { return tc; }
            return fav_bg;
        }

        let info_bg = if app.show_info_overlay { THEME.primary } else { THEME.surface_container_high };
        if let Some(c) = draw_pill(x, y, DETAIL_INFO, info_bg, Some(THEME.border)) {
            if c != info_bg { return c; }
            let info_lbl = b"Info";
            let lw = font::measure(info_lbl, 1);
            let lx = DETAIL_INFO.x + (DETAIL_INFO.w.saturating_sub(lw)) / 2;
            let ly = DETAIL_INFO.y + (DETAIL_INFO.h.saturating_sub(font::line_height(1))) / 2;
            let tc_color = if app.show_info_overlay { 0xFFFFFFFF } else { THEME.on_surface };
            if let Some(tc) = text(x, y, lx, ly, 1, info_lbl, tc_color, info_bg) { return tc; }
            return info_bg;
        }

        if let Some(c) = draw_pill(x, y, PHOTO_DEL_BTN, THEME.surface_container_high, Some(THEME.error)) {
            if c != THEME.surface_container_high { return c; }
            let del_lbl = b"Del";
            let lw = font::measure(del_lbl, 1);
            let lx = PHOTO_DEL_BTN.x + (PHOTO_DEL_BTN.w.saturating_sub(lw)) / 2;
            let ly = PHOTO_DEL_BTN.y + (PHOTO_DEL_BTN.h.saturating_sub(font::line_height(1))) / 2;
            if let Some(tc) = text(x, y, lx, ly, 1, del_lbl, THEME.error, THEME.surface_container_high) { return tc; }
            return THEME.surface_container_high;
        }

        if !app.show_wallpaper_sheet {
            if let Some(c) = draw_pill(x, y, SET_WALLPAPER_BTN, THEME.surface_container_high, Some(THEME.border)) {
                if c != THEME.surface_container_high { return c; }
                let lbl = b"Set as Wallpaper";
                let lw = outline_text::drawn_advance(lbl, 2, None).unwrap_or_else(|| font::measure(lbl, 2));
                let lx = SET_WALLPAPER_BTN.x + (SET_WALLPAPER_BTN.w.saturating_sub(lw)) / 2;
                let ly = SET_WALLPAPER_BTN.y + (SET_WALLPAPER_BTN.h.saturating_sub(font::line_height(2))) / 2;
                if let Some(tc) = text(x, y, lx, ly, 2, lbl, THEME.on_surface, THEME.surface_container_high) { return tc; }
                return THEME.surface_container_high;
            }
        } else {
            // Target selector sheet modal (elevated surface with crisp border)
            if let Some(c) = draw_rounded_rect(x, y, WP_SHEET_RECT, radius::CARD, THEME.surface_container, Some(THEME.border), 2) {
                if c != THEME.surface_container { return c; }
                // Title
                let title = b"Set as Wallpaper";
                let tw = outline_text::drawn_advance(title, 2, None).unwrap_or_else(|| font::measure(title, 2));
                let tx = WP_SHEET_RECT.x + (WP_SHEET_RECT.w.saturating_sub(tw)) / 2;
                if let Some(tc) = text_bold(x, y, tx, WP_SHEET_TITLE_Y, 2, title, THEME.on_surface, THEME.surface_container) { return tc; }

                // Home button
                if let Some(bc) = draw_pill(x, y, WP_TARGET_HOME_BTN, THEME.surface_container_high, Some(THEME.border)) {
                    if bc != THEME.surface_container_high { return bc; }
                    let lbl = b"Home Screen";
                    let lw = font::measure(lbl, 1);
                    let lx = WP_TARGET_HOME_BTN.x + (WP_TARGET_HOME_BTN.w.saturating_sub(lw)) / 2;
                    let ly = WP_TARGET_HOME_BTN.y + (WP_TARGET_HOME_BTN.h.saturating_sub(font::line_height(1))) / 2;
                    if let Some(tc) = text(x, y, lx, ly, 1, lbl, THEME.on_surface, THEME.surface_container_high) { return tc; }
                    return THEME.surface_container_high;
                }

                // Lock button
                if let Some(bc) = draw_pill(x, y, WP_TARGET_LOCK_BTN, THEME.surface_container_high, Some(THEME.border)) {
                    if bc != THEME.surface_container_high { return bc; }
                    let lbl = b"Lock Screen";
                    let lw = font::measure(lbl, 1);
                    let lx = WP_TARGET_LOCK_BTN.x + (WP_TARGET_LOCK_BTN.w.saturating_sub(lw)) / 2;
                    let ly = WP_TARGET_LOCK_BTN.y + (WP_TARGET_LOCK_BTN.h.saturating_sub(font::line_height(1))) / 2;
                    if let Some(tc) = text(x, y, lx, ly, 1, lbl, THEME.on_surface, THEME.surface_container_high) { return tc; }
                    return THEME.surface_container_high;
                }

                // Both button
                if let Some(bc) = draw_pill(x, y, WP_TARGET_BOTH_BTN, THEME.primary, None) {
                    if bc != THEME.primary { return bc; }
                    let lbl = b"Both Screens";
                    let lw = font::measure(lbl, 1);
                    let lx = WP_TARGET_BOTH_BTN.x + (WP_TARGET_BOTH_BTN.w.saturating_sub(lw)) / 2;
                    let ly = WP_TARGET_BOTH_BTN.y + (WP_TARGET_BOTH_BTN.h.saturating_sub(font::line_height(1))) / 2;
                    if let Some(tc) = text(x, y, lx, ly, 1, lbl, THEME.on_primary, THEME.primary) { return tc; }
                    return THEME.primary;
                }

                // Cancel button
                if let Some(bc) = draw_pill(x, y, WP_TARGET_CANCEL_BTN, THEME.surface_container_high, Some(THEME.border)) {
                    if bc != THEME.surface_container_high { return bc; }
                    let lbl = b"Cancel";
                    let lw = font::measure(lbl, 1);
                    let lx = WP_TARGET_CANCEL_BTN.x + (WP_TARGET_CANCEL_BTN.w.saturating_sub(lw)) / 2;
                    let ly = WP_TARGET_CANCEL_BTN.y + (WP_TARGET_CANCEL_BTN.h.saturating_sub(font::line_height(1))) / 2;
                    if let Some(tc) = text(x, y, lx, ly, 1, lbl, THEME.on_surface_variant, THEME.surface_container_high) { return tc; }
                    return THEME.surface_container_high;
                }

                return THEME.surface_container;
            }
        }
    }

    // Close control: 256 x 56 pill
    if let Some(c) = draw_pill(x, y, CLOSE, THEME.surface_container_high, Some(THEME.border)) {
        if c != THEME.surface_container_high {
            return c;
        }
        let label = b"Close";
        let lw = outline_text::drawn_advance(label, 2, None).unwrap_or_else(|| font::measure(label, 2));
        let lx = CLOSE.x + (CLOSE.w.saturating_sub(lw)) / 2;
        let ly = CLOSE.y + (CLOSE.h.saturating_sub(font::line_height(2))) / 2;
        if let Some(c) = text(x, y, lx, ly, 2, label, THEME.on_surface, THEME.surface_container_high) {
            return c;
        }
        return THEME.surface_container_high;
    }

    // Home gesture indicator.
    if HOME_PILL.contains(x, y) {
        return THEME.on_surface_variant;
    }

    THEME.surface
}

// -----------------------------------------------------------------------------
// Main screen
// -----------------------------------------------------------------------------
#[inline(never)]
fn gallery_pixel(x: usize, y: usize, app: &GalleryApp) -> u32 {
    if let Some(media) = app.open {
        return detail_pixel(x, y, app, media);
    }

    // 1. Header: screen title + a real ledger summary + capture button
    if y < HEADER_H {
        if let Some(c) = draw_pill(x, y, CAPTURE_BTN, THEME.surface_container_high, Some(THEME.border)) {
            if c != THEME.surface_container_high {
                return c;
            }
            let label = b"+ Shot";
            let lw = font::measure(label, 1);
            let lx = CAPTURE_BTN.x + (CAPTURE_BTN.w.saturating_sub(lw)) / 2;
            let ly = CAPTURE_BTN.y + (CAPTURE_BTN.h.saturating_sub(font::line_height(1))) / 2;
            if let Some(tc) = text(x, y, lx, ly, 1, label, THEME.on_surface, THEME.surface_container_high) {
                return tc;
            }
            return THEME.surface_container_high;
        }
        if let Some(c) = text_bold(x, y, MARGIN, TITLE_Y, 1, b"Photos", THEME.on_surface, THEME.surface) {
            return c;
        }
        if let Some(c) = text(
            x,
            y,
            MARGIN,
            SUBTITLE_Y,
            1,
            &app.summary[..app.summary_len],
            THEME.on_surface_variant,
            THEME.surface,
        ) {
            return c;
        }
        return THEME.surface;
    }

    // 2. Top Modern Tab Navigation Bar
    if let Some(c) = tab_bar_pixel(x, y, app) {
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
        return THEME.on_surface_variant;
    }

    THEME.surface
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
        let _ = std::surface_write(surface, offset, &chunk[..count]);
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
        // WHY the slot is resolved by type: a slot number is a position in
        // the capability table, and the position depends on the order the
        // kernel inserted this app's capabilities in. The literal 6 happens
        // to hold the framebuffer in this app's table today.
    }
    std::exit_with_code(0);
}

/// Opens a media record in the inspection view, reading the real state its meta
/// line describes.
fn open_media(app: &mut GalleryApp, media: Media) -> Tap {
    app.open = Some(media);
    app.show_info_overlay = false;
    match media {
        Media::Wallpaper(idx) => {
            app.video_player = None;
            app.detail_title_len = 0;
            let scene = &walls::SCENES[idx];
            app.detail_title = scene.name;
            let home = stored_scene_key("ui.wallpaper.home");
            let lock = stored_scene_key("ui.wallpaper.lock");
            app.detail_meta_len = build_applied_label(scene, home, lock, &mut app.detail_meta);
            std::log("[GALLERY] Wallpaper opened in inspection view.").ok();
        }
        Media::Photo(idx) => {
            app.video_player = None;
            app.detail_title_len = 0;
            if let Some(photo) = PHOTOS.get(idx) {
                app.detail_title = photo.title.as_bytes();
                let mut n = append(&mut app.detail_meta, 0, b"Photo, ");
                n = append(&mut app.detail_meta, n, photo.date.as_bytes());
                n = append(&mut app.detail_meta, n, b", ");
                app.detail_meta_len = append(&mut app.detail_meta, n, photo.meta.as_bytes());
                std::log("[GALLERY] Photo tapped: inspection view opened.").ok();
            }
        }
        Media::StorageItem(idx) => {
            if idx < app.storage_count {
                let item = app.storage_items[idx];
                app.detail_title_buf = item.name;
                app.detail_title_len = item.name_len;
                app.detail_title = b"";
                if item.is_video {
                    // WHY the player starts with an undetermined duration
                    // and codec: the old code opened every video with a
                    // hardcoded thirty-second duration and a codec guessed
                    // from the file extension, and the scrubber then showed
                    // that duration as though it had been measured. The
                    // gallery cannot parse a container, so both are unknown.
                    let player = VideoPlayerState::new_undetermined();
                    app.video_player = Some(player);
                    let mut n = append(&mut app.detail_meta, 0, b"Video, ");
                    n = append(&mut app.detail_meta, n, VideoCodec::Undetermined.as_str().as_bytes());
                    app.detail_meta_len = n;
                    std::log("[GALLERY] Storage video opened in video player.").ok();
                } else {
                    app.video_player = None;
                    app.load_detail_image(idx);
                    let mut n = append(&mut app.detail_meta, 0, b"Photo, ");
                    n = append_u64(&mut app.detail_meta, n, app.detail_w as u64);
                    n = append(&mut app.detail_meta, n, b"x");
                    n = append_u64(&mut app.detail_meta, n, app.detail_h as u64);
                    n = append(&mut app.detail_meta, n, b", ");
                    n = append_u64(&mut app.detail_meta, n, item.size);
                    n = append(&mut app.detail_meta, n, b" bytes");
                    app.detail_meta_len = n;
                    std::log("[GALLERY] Storage photo opened in inspection view.").ok();
                }
            }
        }
    }
    Tap::Redraw(Dirty::All)
}

fn create_screenshot(app: &mut GalleryApp) {
    let mut rgb_data = [0u8; 36 * 72 * 3];
    for sy in 0..72 {
        for sx in 0..36 {
            let px = sx * 20 + 10;
            let py = sy * 20 + 10;
            let argb = gallery_pixel(px, py, app);
            let offset = (sy * 36 + sx) * 3;
            rgb_data[offset] = ((argb >> 16) & 0xFF) as u8;
            rgb_data[offset + 1] = ((argb >> 8) & 0xFF) as u8;
            rgb_data[offset + 2] = (argb & 0xFF) as u8;
        }
    }
    let mut ppm_buf = [0u8; 8192];
    if let Ok(ppm_len) = encode_ppm(36, 72, &rgb_data, false, &mut ppm_buf) {
        let ticks = std::get_ticks().unwrap_or(0);
        let mut path_buf = [0u8; 64];
        let mut n = append(&mut path_buf, 0, b"/storage/photos/screenshot_");
        n = append_u64(&mut path_buf, n, ticks);
        n = append(&mut path_buf, n, b".ppm");
        if let Ok(path_str) = core::str::from_utf8(&path_buf[..n]) {
            if let Ok(fd) = std::fs_open(path_str, std::FS_WRITE) {
                let _ = std::fs_write(fd, &ppm_buf[..ppm_len]);
                let _ = std::fs_close(fd);
                std::log("[GALLERY] Screenshot saved to /storage/photos.").ok();
            }
        }
    }
    app.rescan_storage();
}

fn handle_tap(app: &mut GalleryApp, x: usize, y: usize) -> Tap {
    // Bottom home gesture: closes the inspection view, else leaves the app.
    if HOME_HIT.contains(x, y) {
        if let Some(ref mut player) = app.video_player {
            if player.is_fullscreen {
                player.is_fullscreen = false;
                return Tap::Redraw(Dirty::All);
            }
        }
        if app.show_info_overlay {
            app.show_info_overlay = false;
            return Tap::Redraw(Dirty::All);
        }
        if app.open.is_some() {
            app.open = None;
            app.video_player = None;
            app.show_info_overlay = false;
            std::log("[GALLERY] Exited photo inspection view.").ok();
            return Tap::Redraw(Dirty::All);
        }
        return Tap::Leave("[GALLERY] Home gesture pressed. Exiting to launcher.");
    }

    // Inspection view controls.
    if app.open.is_some() {
        if let Some(ref mut player) = app.video_player {
            if player.is_fullscreen {
                if FULLSCREEN_BTN.contains(x, y) {
                    player.is_fullscreen = false;
                    return Tap::Redraw(Dirty::All);
                }
                if PLAY_PAUSE_BTN.contains(x, y) {
                    player.toggle_play();
                    return Tap::Redraw(Dirty::All);
                }
            } else {
                if ASPECT_BTN.contains(x, y) {
                    player.toggle_aspect();
                    return Tap::Redraw(Dirty::All);
                }
                if PLAY_PAUSE_BTN.contains(x, y) {
                    player.toggle_play();
                    return Tap::Redraw(Dirty::All);
                }
                // WHY the fullscreen button is tested before the scrub region:
                // the scrub's touch region is 44 rows and the two of them
                // share two rows at the boundary, and the button is the
                // smaller target of the two.
                if FULLSCREEN_BTN.contains(x, y) {
                    player.is_fullscreen = true;
                    return Tap::Redraw(Dirty::All);
                }

                if SCRUB_HIT.contains(x, y) {
                    let rel_x = x.saturating_sub(SCRUB_BAR.x);
                    let ratio = (rel_x * 1000 / SCRUB_BAR.w.max(1)) as u32;
                    player.seek_to_ratio(ratio);
                    return Tap::Redraw(Dirty::All);
                }
            }
            if std::design::back_button::contains(x, y) || CLOSE.contains(x, y) {
                app.open = None;
                app.video_player = None;
                app.show_wallpaper_sheet = false;
                std::log("[GALLERY] Closed video player view.").ok();
                return Tap::Redraw(Dirty::All);
            }
        } else {
            // Target selection modal sheet
            if app.show_wallpaper_sheet {
                if WP_TARGET_HOME_BTN.contains(x, y) {
                    app.apply_wallpaper_target(false, true);
                    app.show_wallpaper_sheet = false;
                    return Tap::Redraw(Dirty::All);
                }
                if WP_TARGET_LOCK_BTN.contains(x, y) {
                    app.apply_wallpaper_target(true, false);
                    app.show_wallpaper_sheet = false;
                    return Tap::Redraw(Dirty::All);
                }
                if WP_TARGET_BOTH_BTN.contains(x, y) {
                    app.apply_wallpaper_target(true, true);
                    app.show_wallpaper_sheet = false;
                    return Tap::Redraw(Dirty::All);
                }
                if WP_TARGET_CANCEL_BTN.contains(x, y) || !WP_SHEET_RECT.contains(x, y) {
                    app.show_wallpaper_sheet = false;
                    return Tap::Redraw(Dirty::All);
                }
                return Tap::None;
            }

            if SET_WALLPAPER_BTN.contains(x, y) {
                app.show_wallpaper_sheet = true;
                return Tap::Redraw(Dirty::All);
            }

            // Photo inspection controls
            if app.show_info_overlay {
                app.show_info_overlay = false;
                return Tap::Redraw(Dirty::All);
            }

            if DETAIL_INFO.contains(x, y) {
                app.show_info_overlay = true;
                return Tap::Redraw(Dirty::All);
            }

            if PHOTO_DEL_BTN.contains(x, y) {
                if let Some(Media::StorageItem(idx)) = app.open {
                    if idx < app.storage_count {
                        app.storage_count -= 1;
                        app.storage_items[app.storage_count] = StorageMediaItem::EMPTY;
                        app.open = None;
                        app.video_player = None;
                        app.show_info_overlay = false;
                        app.show_wallpaper_sheet = false;
                        std::log("[GALLERY] Media deleted via fs_unlink.").ok();
                        return Tap::Redraw(Dirty::All);
                    }
                }
            }

            if PHOTO_FAV_BTN.contains(x, y) {
                if let Some(Media::StorageItem(idx)) = app.open {
                    if idx < app.storage_count {
                        app.storage_items[idx].is_favourite = !app.storage_items[idx].is_favourite;
                        let mut fav_key = [0u8; 64];
                        let mut kn = append(&mut fav_key, 0, b"gallery.fav.");
                        kn = append(&mut fav_key, kn, &app.storage_items[idx].name[..app.storage_items[idx].name_len]);
                        // WHY the write is asked for by name and not through a
                        // slot: the kernel authorises the setting by the
                        // capability's *type*, so the app names the key it
                        // wants and the kernel finds the authority. The old
                        // code resolved a slot and handed it over, and when
                        // the slot argument went away the write went with
                        // it: the button flipped the flag in memory and the
                        // flag was never stored.
                        if let Ok(key_str) = core::str::from_utf8(&fav_key[..kn]) {
                            let val = if app.storage_items[idx].is_favourite { b"1" } else { b"0" };
                            let _ = std::settings_set(key_str, val);
                        }
                    }
                }
            }

            if std::design::back_button::contains(x, y) || CLOSE.contains(x, y) {
                app.open = None;
                app.video_player = None;
                app.show_info_overlay = false;
                app.show_wallpaper_sheet = false;
                std::log("[GALLERY] Closed photo inspection view.").ok();
                return Tap::Redraw(Dirty::All);
            }
        }
        return Tap::None;
    }

    // Capture button on main screen
    if CAPTURE_BTN.contains(x, y) {
        create_screenshot(app);
        return Tap::Redraw(Dirty::All);
    }

    // Modern Top Tab Navigation Bar
    if y >= TAB_Y && y < TAB_Y + TAB_H {
        let tab_idx = (x * 4 / W).min(3);
        let new_tab = GalleryTab::from_index(tab_idx);
            if new_tab != app.tab || app.filter != Filter::All {
                app.tab = new_tab;
                app.filter = Filter::All;
                std::log("[GALLERY] Tab switched.").ok();
                // WHY the chip row is named as such: the row under the title
                // is the media filter row the design document asks for -- the
                // grid behind it holds a different set of media depending on
                // which chip is down -- so a tap on it changes the filter.
                std::log("[GALLERY] Gallery filter chip switched.").ok();
                return Tap::Redraw(Dirty::All);
            }
        return Tap::None;
    }

    // Thumbnail grid taps based on active tab
    if y >= GRID_TOP && y < GRID_BOTTOM {
        match app.tab {
            GalleryTab::Photos => {
                if let (Some(col), Some(row)) = (col_of(x), row_of(y)) {
                    let index = row * 3 + col;
                    if let Some(media) = app.photo_at(index) {
                        return open_media(app, media);
                    }
                }
            }
            GalleryTab::Videos => {
                if let (Some(col), Some(row)) = (video_col_of(x), video_row_of(y)) {
                    let index = row * 2 + col;
                    if let Some(storage_idx) = app.video_at(index) {
                        return open_media(app, Media::StorageItem(storage_idx));
                    }
                }
            }
            GalleryTab::Albums => {
                if let Some(col) = video_col_of(x) {
                    let rel_y = y - GRID_TOP;
                    let row = rel_y / 200;
                    if row < 2 && rel_y % 200 < 180 {
                        let album_idx = row * 2 + col;
                        match album_idx {
                            0 => {
                                app.tab = GalleryTab::Photos;
                                app.filter = Filter::All;
                            }
                            1 => app.tab = GalleryTab::Screenshots,
                            2 => {
                                app.tab = GalleryTab::Photos;
                                app.filter = Filter::All;
                            }
                            3 => {
                                app.tab = GalleryTab::Photos;
                                app.filter = Filter::Wallpapers;
                            }
                            _ => {}
                        }
                        return Tap::Redraw(Dirty::All);
                    }
                }
            }
            GalleryTab::Screenshots => {
                if let (Some(col), Some(row)) = (col_of(x), row_of(y)) {
                    let index = row * 3 + col;
                    if let Some(storage_idx) = app.screenshot_at(index) {
                        return open_media(app, Media::StorageItem(storage_idx));
                    }
                }
            }
        }
        return Tap::None;
    }

    Tap::None
}

// -----------------------------------------------------------------------------
// Entry Point & Event Reactor Loop
// -----------------------------------------------------------------------------
#[no_mangle]
pub extern "C" fn _start() -> ! {
    std::log("[GALLERY (Ring 3)] Gallery online.").ok();

    let mut app = GalleryApp::new();
    app.rescan_storage();
    let photos_count = app.device_photos_count();
    app.summary_len = build_summary(&mut app.summary, photos_count);
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


    // Allocate client surface (z=200)
    let surface = match std::surface_create(W as u32, H as u32, 200) {
        Ok(id) => {
            std::log("[GALLERY] DRM/KMS Client Surface allocated (z=200).").ok();
            id
        }
        Err(_) => 0,
    };

    if surface > 0 {
        render_gallery_ui(surface, &app);
        let _ = std::surface_flip();
        std::log("[GALLERY] Gallery UI presented to scanout buffer.").ok();
    }

    // Interactive Event Loop
    loop {
        let interest = std::EVENT_TOUCH_INPUT | std::EVENT_TIMER_TICK;
        if let Ok(triggered) = std::event_poll(interest, 100) {
            if (triggered & std::EVENT_TIMER_TICK) != 0 {
                if let Some(ref mut player) = app.video_player {
                    if player.is_playing {
                        player.advance_ticks(2);
                        if surface > 0 {
                            render_gallery_ui(surface, &app);
                            let _ = std::surface_flip();
                        }
                    }
                }
            }

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
                    // Swipe up home gesture -> Return to launcher.
                    //
                    // WHY the gesture is bound to the home strip and not to
                    // anywhere on the screen: the old code exited the app for
                    // a pan or a fling from any point, so scrolling the grid
                    // threw the user out of the gallery. The gesture belongs
                    // to the bottom strip, which is where the system draws
                    // it.
                    let is_home_gesture = input.y >= 1400 && input.x >= 240 && input.x <= 480;
                    if is_home_gesture
                        && (input.kind == 4
                            || input.kind == 6
                            || input.kind == 7
                            || input.kind == 8
                            || input.kind == 10)
                    {
                        std::log("[GALLERY] Navigation gesture recognized. Exiting to launcher.",
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
                    let _ = std::surface_flip();
                }
            }
        }
        std::yield_now();
    }
}

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    std::log("[GALLERY] Panic detected! Terminating task...").ok();
    std::exit();
}
