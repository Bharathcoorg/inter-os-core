// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

#![no_std]

pub use interos_std as std;

/// The app's view of the platform's design system.
pub mod theme;

/// The image file extensions the platform recognises.
pub use interos_std::image::IMAGE_SUFFIXES;

pub const VIDEO_SUFFIXES: [&[u8]; 8] = [
    b".mp4", b".webm", b".mkv", b".avi", b".mov", b".mjpeg", b".mjpg", b".h264",
];

/// Whether an entry name carries a recognised image extension (case-insensitive).
pub use interos_std::image::has_image_suffix;
pub fn has_video_suffix(name: &[u8]) -> bool {
    VIDEO_SUFFIXES.iter().any(|suffix| {
        name.len() > suffix.len()
            && name[name.len() - suffix.len()..].eq_ignore_ascii_case(suffix)
    })
}

/// Whether an entry name carries any media extension (image or video).
pub fn is_media_file(name: &[u8]) -> bool {
    has_image_suffix(name) || has_video_suffix(name)
}

/// Counts total entries and image files in a VFS `fs_list` payload formatted
/// as newline-separated `name|size|type` lines.
pub fn count_listing(payload: &[u8]) -> (usize, usize) {
    let mut entries = 0usize;
    let mut images = 0usize;
    for line in payload.split(|&b| b == b'\n') {
        if line.is_empty() {
            continue;
        }
        entries += 1;
        let end = line.iter().position(|&b| b == b'|').unwrap_or(line.len());
        if has_image_suffix(&line[..end]) {
            images += 1;
        }
    }
    (entries, images)
}

/// Appends `bytes` to `buf` at `n` and returns the new length, truncating
/// rather than overrunning.
pub fn append(buf: &mut [u8], n: usize, bytes: &[u8]) -> usize {
    let take = bytes.len().min(buf.len().saturating_sub(n));
    buf[n..n + take].copy_from_slice(&bytes[..take]);
    n + take
}

/// Appends the decimal form of `value` to `buf` at `n`.
pub fn append_u64(buf: &mut [u8], mut n: usize, mut value: u64) -> usize {
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

/// Width of a decoded thumbnail, in pixels.
pub const THUMB_W: usize = 54;

/// Height of a decoded thumbnail, in pixels.
pub const THUMB_H: usize = 42;

/// Number of bytes a decoded thumbnail occupies (RGB triples).
pub const THUMB_BYTES: usize = THUMB_W * THUMB_H * 3;

/// A decoded thumbnail of a stored image.
///
/// WHY a thumbnail exists at all: the grid used to paint each stored file
/// with a gradient whose colours were hashed from the file name. That is a
/// picture of nothing -- it looks like a preview of the photograph and is
/// not one. A thumbnail here is either the real image, decoded from the
/// file, or it is absent and the tile says so.
#[derive(Clone, Copy)]
pub struct Thumbnail {
    /// RGB triples, row-major.
    pub rgb: [u8; THUMB_BYTES],
    /// The decoded image's own width and height.
    ///
    /// WHY the source size is kept: the grid's image area is wider than it is
    /// tall, so a portrait capture drawn across the whole area was stretched
    /// sideways. The tile draws the picture at its own aspect and leaves the
    /// tile's background on the two sides.
    pub src_w: u32,
    pub src_h: u32,
    /// Whether the thumbnail holds a decoded image.
    pub valid: bool,
}

impl Thumbnail {
    /// The empty thumbnail -- no image was decoded.
    pub const EMPTY: Self = Self {
        rgb: [0u8; THUMB_BYTES],
        src_w: 0,
        src_h: 0,
        valid: false,
    };

    /// Scales a decoded RGBA image down into a thumbnail.
    ///
    /// Returns `None` when the source is empty. A source that is smaller
    /// than the thumbnail is sampled by nearest neighbour, which is enough
    /// for a preview and never invents a pixel that was not in the file.
    pub fn from_rgba(rgba: &[u8], src_w: u32, src_h: u32) -> Option<Self> {
        if src_w == 0 || src_h == 0 {
            return None;
        }
        let mut thumb = Self {
            rgb: [0u8; THUMB_BYTES],
            src_w,
            src_h,
            valid: true,
        };
        for ty in 0..THUMB_H {
            // Nearest-neighbour sample: pick the source row that this
            // thumbnail row maps to.
            let sy = (ty as u64 * src_h as u64 / THUMB_H as u64) as usize;
            let sy = if sy >= src_h as usize { src_h as usize - 1 } else { sy };
            for tx in 0..THUMB_W {
                let sx = (tx as u64 * src_w as u64 / THUMB_W as u64) as usize;
                let sx = if sx >= src_w as usize { src_w as usize - 1 } else { sx };
                let src = (sy * src_w as usize + sx) * 4;
                if src + 3 >= rgba.len() {
                    continue;
                }
                let dst = (ty * THUMB_W + tx) * 3;
                thumb.rgb[dst] = rgba[src];
                thumb.rgb[dst + 1] = rgba[src + 1];
                thumb.rgb[dst + 2] = rgba[src + 2];
            }
        }
        Some(thumb)
    }

    /// The colour of the thumbnail at a point inside the tile.
    ///
    /// Returns `None` when the thumbnail holds no image, so the caller can
    /// draw a placeholder rather than a colour.
    pub fn sample(&self, tx: usize, ty: usize) -> Option<u32> {
        if !self.valid {
            return None;
        }
        if tx >= THUMB_W || ty >= THUMB_H {
            return None;
        }
        let i = (ty * THUMB_W + tx) * 3;
        Some(
            0xFF000000
                | ((self.rgb[i] as u32) << 16)
                | ((self.rgb[i + 1] as u32) << 8)
                | (self.rgb[i + 2] as u32),
        )
    }

    /// The size the picture occupies inside a tile area of `area_w` by `area_h`.
    ///
    /// The picture keeps its own aspect and is centred; the caller paints the
    /// tile's background around it. An empty thumbnail fills the whole area,
    /// because its placeholder belongs to the area rather than to a picture.
    pub fn fitted_size(&self, area_w: usize, area_h: usize) -> (usize, usize) {
        if !self.valid || self.src_w == 0 || self.src_h == 0 {
            return (area_w, area_h);
        }
        let area_w = area_w as u64;
        let area_h = area_h as u64;
        let src_w = self.src_w as u64;
        let src_h = self.src_h as u64;
        if src_w * area_h <= src_h * area_w {
            // Height-bound: a portrait picture is as tall as the area.
            let w = ((src_w * area_h / src_h) as usize).max(1);
            (w, area_h as usize)
        } else {
            let h = ((src_h * area_w / src_w) as usize).max(1);
            (area_w as usize, h)
        }
    }
}

/// Extracts the first Motion JPEG frame payload from a RIFF AVI container.
///
/// Returns `None` when the file is not a RIFF AVI or contains no decodable
/// Motion JPEG frame chunks.
pub fn find_avi_first_mjpeg_frame(data: &[u8]) -> Option<&[u8]> {
    if data.len() < 12 || &data[0..4] != b"RIFF" || &data[8..12] != b"AVI " {
        return None;
    }
    let mut pos = 12;
    while pos + 8 <= data.len() {
        let chunk_id = &data[pos..pos + 4];
        let chunk_len = u32::from_le_bytes([
            data[pos + 4],
            data[pos + 5],
            data[pos + 6],
            data[pos + 7],
        ]) as usize;
        let payload_start = pos + 8;
        let payload_end = payload_start.saturating_add(chunk_len);
        if payload_end > data.len() {
            break;
        }
        if (chunk_id == b"00dc" || chunk_id == b"01dc" || chunk_id == b"00db" || chunk_id == b"01db")
            && chunk_len >= 4
            && data[payload_start] == 0xFF
            && data[payload_start + 1] == 0xD8
        {
            return Some(&data[payload_start..payload_end]);
        }
        if chunk_id == b"LIST" && chunk_len >= 4 {
            pos += 12;
            continue;
        }
        pos = payload_end + (chunk_len & 1);
    }
    // Fallback: search for first JPEG SOI (0xFF, 0xD8, 0xFF) inside the file
    let mut i = 12;
    while i + 3 < data.len() {
        if data[i] == 0xFF && data[i + 1] == 0xD8 && data[i + 2] == 0xFF {
            return Some(&data[i..]);
        }
        i += 1;
    }
    None
}

/// Genuine thumbnail extraction for decodable video files (MJPEG, PPM) into a
/// caller-supplied static scratch buffer (e.g. 64x64 or 96x96 pixels).
///
/// Returns `None` for unsupported video codecs (H.264, MP4, WebM, VP9, AV1),
/// strictly upholding the Platform Truth Contract by never synthesizing fake pictures.
pub fn extract_video_thumbnail(
    data: &[u8],
    name: &[u8],
    frame_scratch: &mut [u8],
) -> Option<Thumbnail> {
    if name.len() >= 4 && name[name.len() - 4..].eq_ignore_ascii_case(b".ppm")
        || (data.len() >= 2 && data[0] == b'P' && data[1] == b'6')
    {
        let (w, h, _header_len) = interos_std::image::decode_ppm_header(data).ok()?;
        let required = (w as usize).checked_mul(h as usize)?.checked_mul(4)?;
        if required <= frame_scratch.len() {
            interos_std::image::decode_ppm(data, frame_scratch).ok()?;
            return Thumbnail::from_rgba(&frame_scratch[..required], w, h);
        }
        return None;
    }

    if (name.len() >= 5 && name[name.len() - 5..].eq_ignore_ascii_case(b".mjpg"))
        || (name.len() >= 6 && name[name.len() - 6..].eq_ignore_ascii_case(b".mjpeg"))
        || (data.len() >= 2 && data[0] == 0xFF && data[1] == 0xD8)
    {
        if let Ok((w, h, bytes)) = interos_std::image::decode_jpeg(data, frame_scratch) {
            return Thumbnail::from_rgba(&frame_scratch[..bytes], w, h);
        }
        return None;
    }

    if (name.len() >= 4 && name[name.len() - 4..].eq_ignore_ascii_case(b".avi"))
        || (data.len() >= 12 && &data[0..4] == b"RIFF" && &data[8..12] == b"AVI ")
    {
        if let Some(payload) = find_avi_first_mjpeg_frame(data) {
            if let Ok((w, h, bytes)) = interos_std::image::decode_jpeg(payload, frame_scratch) {
                return Thumbnail::from_rgba(&frame_scratch[..bytes], w, h);
            }
        }
        return None;
    }

    // Unsupported video formats (MP4, H.264, WebM, etc.): return None truthfully
    None
}

/// One media item the gallery can show or play.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Media {
    Wallpaper(usize),
    Photo(usize),
    StorageItem(usize),
}

/// Which set the filter row selects.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Filter {
    All,
    Wallpapers,
    Photos,
    Videos,
}

impl Filter {
    pub fn from_index(index: usize) -> Filter {
        match index {
            1 => Filter::Wallpapers,
            2 => Filter::Photos,
            3 => Filter::Videos,
            _ => Filter::All,
        }
    }

    pub const fn index(self) -> usize {
        match self {
            Filter::All => 0,
            Filter::Wallpapers => 1,
            Filter::Photos => 2,
            Filter::Videos => 3,
        }
    }

    /// The label the filter row draws, without a count.
    pub const fn label(self) -> &'static [u8] {
        match self {
            Filter::All => b"All",
            Filter::Wallpapers => b"Wallpapers",
            Filter::Photos => b"Photos",
            Filter::Videos => b"Videos",
        }
    }

    pub fn shows_wallpapers(self) -> bool {
        matches!(self, Filter::All | Filter::Wallpapers)
    }

    pub fn shows_ledger(self) -> bool {
        matches!(self, Filter::All | Filter::Photos)
    }

    pub fn shows_videos(self) -> bool {
        matches!(self, Filter::All | Filter::Videos)
    }

    /// The number of media items this filter selects.
    pub fn count(self, counts: FilterCounts) -> usize {
        match self {
            Filter::All => counts.all,
            Filter::Wallpapers => counts.wallpapers,
            Filter::Photos => counts.photos,
            Filter::Videos => counts.videos,
        }
    }

    /// The value the filter is persisted as.
    pub const fn setting_byte(self) -> u8 {
        b'0' + self.index() as u8
    }
}

/// How many media items each filter selects.
///
/// WHY this is a measured struct and not four separate counts: every filter
/// chip shows its own count, and a chip whose count came from a different
/// measurement than the grid behind it would be a filter that lies about what
/// tapping it shows.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct FilterCounts {
    pub all: usize,
    pub wallpapers: usize,
    pub photos: usize,
    pub videos: usize,
}

/// The counts behind the four filter chips.
pub const fn filter_counts(
    wallpapers: usize,
    photos: usize,
    videos: usize,
) -> FilterCounts {
    FilterCounts {
        all: wallpapers + photos + videos,
        wallpapers,
        photos,
        videos,
    }
}

/// Parses the stored filter. An absent or unrecognised value selects `All`,
/// which is the only safe default: it cannot hide media the user has.
pub fn filter_from_setting(raw: &[u8]) -> Filter {
    for &byte in raw {
        match byte {
            b'0'..=b'3' => return Filter::from_index((byte - b'0') as usize),
            b' ' | b'\t' | b'\r' | b'\n' => continue,
            _ => return Filter::All,
        }
    }
    Filter::All
}

/// Builds a chip label such as `Photos 3` into `out` and returns its length.
pub fn write_filter_label(filter: Filter, counts: FilterCounts, out: &mut [u8]) -> usize {
    let mut n = append(out, 0, filter.label());
    n = append(out, n, b" ");
    append_u64(out, n, filter.count(counts) as u64)
}

// -----------------------------------------------------------------------------
// The filter-chip row's layout
// -----------------------------------------------------------------------------
// WHY the geometry lives in the model and not only in the painter: the chip row
// is a horizontal row of a fixed set of chips, and the only honest way to know
// it fits the surface it is laid out on is to add the widths the painter gives
// each chip and read the row's own right edge back. If the numbers that describe
// the row live in the painter (`src/main.rs`, a `test = false` bin no host test
// can reach) the gate has to re-state them as literals, and then the gate and
// the row can disagree without either failing to compile. Putting them here, and
// having the painter read them back, makes the row and the gate the same fact.
pub mod chip_row {
    use crate::{write_filter_label, Filter, FilterCounts};
    use interos_std::design::font;
    use interos_std::design::spacing;

    /// The rung the chip labels are drawn at: the body rung. WHY not lowered: a
    /// chip that fits only by shrinking its type is a chip the gate passed by
    /// lying about the label, not by the label being short.
    pub const SCALE: usize = interos_std::design::typography::BODY_SCALE;

    /// How many filter chips the row lays out.
    pub const COUNT: usize = 4;

    /// The bare label of each chip, in the order `Filter::from_index` returns.
    ///
    /// WHY the filter's own labels: a chip is a filter over one grid, so these
    /// are the labels the row is drawn from; the count the chip appends is the
    /// only extra text, and `counted_label_slices` adds it back.
    pub const BARE_LABELS: [&[u8]; COUNT] =
        [b"All".as_slice(), b"Wallpapers".as_slice(), b"Photos".as_slice(), b"Videos".as_slice()];

    /// The x the row starts on: the grid's margin, so the first chip is flush
    /// with the grid it filters.
    pub const START_X: usize = spacing::GRID_24;

    /// The gap between two chips.
    pub const GAP: usize = 10;

    /// The padding inside a chip, left and right of its label.
    pub const PAD: usize = spacing::GRID_16;

    /// The narrowest a chip may be, however short its label.
    pub const MIN_W: usize = 64;

    /// The width of the surface the row is laid out on. The painter draws into a
    /// surface of this width, so a row whose last edge passes it is off-screen.
    pub const SURFACE_W: usize = interos_std::design::viewport::LOGICAL_WIDTH;

    /// The width the painter gives a chip holding `label`: the label's own
    /// advance, plus the padding on both sides, clamped up to the chip's floor.
    ///
    /// WHY not `const`: it calls `font::measure`, which walks the glyph table.
    pub fn chip_width(label: &[u8]) -> usize {
        (font::measure(label, SCALE) + PAD * 2).max(MIN_W)
    }

    /// Whether every chip in `labels` seats the text it draws: the label's own
    /// advance never exceeds the width the chip is given.
    pub fn each_chip_holds_text(labels: &[&[u8]]) -> bool {
        let mut i = 0;
        while i < labels.len() {
            if font::measure(labels[i], SCALE) > chip_width(labels[i]) {
                return false;
            }
            i += 1;
        }
        true
    }

    /// The right edge (one-past the last column) the row reaches when it lays
    /// out `labels` from `START_X`, chip by chip, with `GAP` between them.
    ///
    /// WHY a slice of counted labels: the count is part of the text the chip
    /// draws, so it is part of the width the chip occupies; measuring the bare
    /// label alone leaves the count's digits outside the row the gate checked.
    pub fn row_right_edge(labels: &[&[u8]]) -> usize {
        let mut x = START_X;
        let mut i = 0;
        while i < labels.len() {
            x += chip_width(labels[i]);
            if i + 1 < labels.len() {
                x += GAP;
            }
            i += 1;
        }
        x
    }

    /// Whether the row of `labels` ends on or before a surface `surface_w` wide.
    pub fn row_fits(labels: &[&[u8]], surface_w: usize) -> bool {
        row_right_edge(labels) <= surface_w
    }

    /// The counted label slices for `counts`, in chip order, written into
    /// `scratch` and their lengths into `lens`. WHY it takes scratch: a chip's
    /// rectangle is measured from the bytes it actually draws, and the label is
    /// `"<Filter> <count>"`, so the gate builds the label into a fixed buffer the
    /// same way the painter does and views it at the true length.
    pub fn counted_label_slices(
        counts: FilterCounts,
        scratch: &mut [[u8; 28]; COUNT],
        lens: &mut [usize; COUNT],
    ) {
        let mut i = 0;
        while i < COUNT {
            lens[i] = write_filter_label(Filter::from_index(i), counts, &mut scratch[i]);
            i += 1;
        }
    }
}

// -----------------------------------------------------------------------------
// Timeline grouping and the grid's row plan
// -----------------------------------------------------------------------------
/// The width of the grid, in cells.
pub const GRID_COLUMNS: usize = 3;

/// The height of one square media cell.
pub const GRID_CELL: usize = 216;

/// The height of a timeline section heading row.
pub const SECTION_ROW_H: usize = 40;

/// The most media entries a plan can hold.
pub const MAX_GRID_ENTRIES: usize = 48;

/// The most rows a plan can hold.
pub const MAX_GRID_ROWS: usize = 64;

/// The heading a run of media is filed under.
///
/// WHY the only dated group is "Newest first": the platform's document index
/// reports a modification *tick*, not a calendar date, and this app has no
/// wall-clock capability. A heading such as "Today" or "March" would be a
/// date the platform never supplied. `Undated` is what the index says when it
/// holds no modification time at all -- which is the current filesystem's
/// answer for every file it creates.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum TimelineGroup {
    BuiltIn,
    Newest,
    Earlier,
    Undated,
}

impl TimelineGroup {
    pub const fn label(self) -> &'static [u8] {
        match self {
            TimelineGroup::BuiltIn => b"Wallpapers",
            TimelineGroup::Newest => b"Newest first",
            TimelineGroup::Earlier => b"Earlier",
            TimelineGroup::Undated => b"Time not recorded",
        }
    }
}

/// The group a document's modification tick belongs to.
pub fn timeline_group(mtime: u64, newest: u64) -> TimelineGroup {
    if mtime == 0 {
        TimelineGroup::Undated
    } else if mtime == newest {
        TimelineGroup::Newest
    } else {
        TimelineGroup::Earlier
    }
}

/// One row of the grid: a section heading or a run of media cells.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum GridRow {
    Section { group: TimelineGroup, count: usize },
    Media { first: usize, count: usize },
}

/// The immutable layout of the Photos grid for one filter.
///
/// WHY the plan is built once per list change: the renderer runs once per
/// pixel, and a plan assembled there would be reassembled a million times for
/// one frame. It holds indices, not copies of the records, so the thumbnails
/// and the tapped media still resolve to the same storage slot.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct GridPlan {
    pub entries: [Media; MAX_GRID_ENTRIES],
    pub entry_count: usize,
    pub rows: [GridRow; MAX_GRID_ROWS],
    pub row_count: usize,
    pub tops: [usize; MAX_GRID_ROWS],
    pub content_h: usize,
}

impl GridPlan {
    pub const EMPTY: Self = Self {
        entries: [Media::Photo(0); MAX_GRID_ENTRIES],
        entry_count: 0,
        rows: [GridRow::Media { first: 0, count: 0 }; MAX_GRID_ROWS],
        row_count: 0,
        tops: [0; MAX_GRID_ROWS],
        content_h: 0,
    };

    /// Lays out the media the filter selects.
    ///
    /// The wallpapers are the device's built-in artwork, so they are their own
    /// section. The stored documents are ordered by the modification tick the
    /// index reports, newest first; documents with no recorded time follow
    /// under a heading that says exactly that.
    pub fn build(
        wallpaper_count: usize,
        photo_count: usize,
        items: &[StorageMediaItem],
        filter: Filter,
    ) -> Self {
        let mut plan = Self::EMPTY;

        if filter.shows_wallpapers() && wallpaper_count > 0 {
            plan.begin_section(TimelineGroup::BuiltIn);
            for index in 0..wallpaper_count.min(MAX_GRID_ENTRIES) {
                plan.push_media(Media::Wallpaper(index));
            }
        }

        let mut order = [0usize; MAX_STORAGE_ITEMS];
        let mut order_len = 0usize;
        let mut newest = 0u64;
        let limit = items.len().min(MAX_STORAGE_ITEMS);
        for index in 0..limit {
            let item = &items[index];
            let visible = if item.is_video {
                filter.shows_videos()
            } else {
                filter.shows_ledger()
            };
            if !visible {
                continue;
            }
            order[order_len] = index;
            order_len += 1;
            if item.mtime > newest {
                newest = item.mtime;
            }
        }

        // Insertion sort: at most sixteen stored items, and no heap to sort
        // them with. Equal times keep their index order, so the grid is stable
        // between scans.
        for slot in 1..order_len {
            let value = order[slot];
            let key = items[value].mtime;
            let mut at = slot;
            while at > 0 && newer_than(key, items[order[at - 1]].mtime) {
                order[at] = order[at - 1];
                at -= 1;
            }
            order[at] = value;
        }

        let mut current: Option<TimelineGroup> = None;
        for slot in 0..order_len {
            let index = order[slot];
            let group = timeline_group(items[index].mtime, newest);
            if current != Some(group) {
                plan.begin_section(group);
                current = Some(group);
            }
            plan.push_media(Media::StorageItem(index));
        }

        if filter.shows_ledger() && photo_count > 0 {
            if current != Some(TimelineGroup::Undated) {
                plan.begin_section(TimelineGroup::Undated);
            }
            for index in 0..photo_count.min(MAX_GRID_ENTRIES) {
                plan.push_media(Media::Photo(index));
            }
        }

        plan.finish();
        plan
    }

    fn begin_section(&mut self, group: TimelineGroup) {
        if self.row_count > 0 {
            if let GridRow::Section { .. } = self.rows[self.row_count - 1] {
                self.rows[self.row_count - 1] = GridRow::Section { group, count: 0 };
                return;
            }
        }
        if self.row_count < MAX_GRID_ROWS {
            self.rows[self.row_count] = GridRow::Section { group, count: 0 };
            self.row_count += 1;
        }
    }

    fn push_media(&mut self, media: Media) {
        if self.entry_count >= MAX_GRID_ENTRIES {
            return;
        }
        let entry = self.entry_count;
        self.entries[entry] = media;
        self.entry_count += 1;
        if self.row_count > 0 {
            if let GridRow::Media { first, count } = self.rows[self.row_count - 1] {
                if count < GRID_COLUMNS {
                    self.rows[self.row_count - 1] = GridRow::Media {
                        first,
                        count: count + 1,
                    };
                    return;
                }
            }
        }
        if self.row_count < MAX_GRID_ROWS {
            self.rows[self.row_count] = GridRow::Media {
                first: entry,
                count: 1,
            };
            self.row_count += 1;
        }
    }

    fn finish(&mut self) {
        let mut top = 0usize;
        let mut row = 0usize;
        while row < self.row_count {
            self.tops[row] = top;
            match self.rows[row] {
                GridRow::Section { group, .. } => {
                    let mut count = 0usize;
                    let mut next = row + 1;
                    while next < self.row_count {
                        match self.rows[next] {
                            GridRow::Media { count: c, .. } => {
                                count += c;
                                next += 1;
                            }
                            GridRow::Section { .. } => break,
                        }
                    }
                    self.rows[row] = GridRow::Section { group, count };
                    top += SECTION_ROW_H;
                }
                GridRow::Media { .. } => top += GRID_CELL,
            }
            row += 1;
        }
        self.content_h = top;
    }

    pub fn row_height(&self, row: usize) -> usize {
        match self.rows.get(row) {
            Some(GridRow::Section { .. }) => SECTION_ROW_H,
            Some(GridRow::Media { .. }) => GRID_CELL,
            None => 0,
        }
    }

    /// The row a content-space offset falls in, with the offset inside it.
    pub fn row_at(&self, offset: usize) -> Option<(usize, usize)> {
        if self.row_count == 0 || offset >= self.content_h {
            return None;
        }
        let mut low = 0usize;
        let mut high = self.row_count;
        while low + 1 < high {
            let mid = (low + high) / 2;
            if self.tops[mid] <= offset {
                low = mid;
            } else {
                high = mid;
            }
        }
        let inside = offset - self.tops[low];
        if inside < self.row_height(low) {
            Some((low, inside))
        } else {
            None
        }
    }

    /// The media at a cell of a media row.
    pub fn media_at(&self, row: usize, column: usize) -> Option<Media> {
        match self.rows.get(row) {
            Some(GridRow::Media { first, count }) if column < *count => {
                self.entries.get(first + column).copied()
            }
            _ => None,
        }
    }

    /// Where a media item sits in the plan, for the viewer's position line.
    pub fn media_position(&self, media: Media) -> Option<usize> {
        self.entries[..self.entry_count]
            .iter()
            .position(|entry| *entry == media)
    }
}

/// Whether `a` should sort before `b`: newest first, and an unrecorded time
/// (`0`) after every recorded one.
fn newer_than(a: u64, b: u64) -> bool {
    if a == 0 {
        return false;
    }
    if b == 0 {
        return true;
    }
    a > b
}

/// The storage slot a delete action may remove, or `None` for built-in media.
pub fn deletable_storage_index(media: Media, storage_count: usize) -> Option<usize> {
    match media {
        Media::StorageItem(index) if index < storage_count => Some(index),
        _ => None,
    }
}

// -----------------------------------------------------------------------------
// Full-screen viewer geometry
// -----------------------------------------------------------------------------
pub const VIEWER_MIN_ZOOM: u32 = 1000;
pub const VIEWER_MAX_ZOOM: u32 = 3000;
pub const VIEWER_MAX_ITEMS: usize = 32;

pub fn clamp_viewer_zoom(zoom: u32) -> u32 {
    zoom.clamp(VIEWER_MIN_ZOOM, VIEWER_MAX_ZOOM)
}

/// The item a swipe lands on. Stops at the ends rather than wrapping, because
/// an index that jumps from the last item to the first is a jump the user did
/// not ask for.
pub fn viewer_step_index(current: usize, len: usize, forward: bool) -> Option<usize> {
    if len == 0 || current >= len {
        return None;
    }
    if forward {
        if current + 1 < len {
            Some(current + 1)
        } else {
            None
        }
    } else if current > 0 {
        Some(current - 1)
    } else {
        None
    }
}

/// The item that should be shown after the viewer's current item is removed.
pub fn viewer_index_after_removal(
    current: usize,
    removed: usize,
    remaining: usize,
) -> Option<usize> {
    if remaining == 0 {
        return None;
    }
    if current > removed {
        Some((current - 1).min(remaining - 1))
    } else {
        Some(current.min(remaining - 1))
    }
}

/// A second double-tap at a zoomed scale returns to fit; otherwise it zooms in.
pub fn double_tap_zoom(zoom: u32) -> u32 {
    if zoom > VIEWER_MIN_ZOOM {
        VIEWER_MIN_ZOOM
    } else {
        2000
    }
}

/// The size an image occupies when fitted into a view, preserving its aspect.
pub fn fit_size(view_w: usize, view_h: usize, src_w: usize, src_h: usize) -> (usize, usize) {
    if view_w == 0 || view_h == 0 || src_w == 0 || src_h == 0 {
        return (0, 0);
    }
    if src_w * view_h <= src_h * view_w {
        (((src_w * view_h / src_h) as usize).max(1), view_h)
    } else {
        (view_w, ((src_h * view_w / src_w) as usize).max(1))
    }
}

/// The on-screen size of a fitted image at a zoom.
pub fn viewer_display_size(fit_w: usize, fit_h: usize, zoom: u32) -> (usize, usize) {
    let zoom = clamp_viewer_zoom(zoom) as usize;
    let disp_w = (fit_w * zoom / 1000).max(1);
    let disp_h = (fit_h * zoom / 1000).max(1);
    (disp_w, disp_h)
}

/// How far the zoomed image may be panned before its edge would leave a gap.
pub fn viewer_pan_limit(
    view_w: usize,
    view_h: usize,
    fit_w: usize,
    fit_h: usize,
    zoom: u32,
) -> (i32, i32) {
    let zoom = clamp_viewer_zoom(zoom) as usize;
    let disp_w = fit_w * zoom / 1000;
    let disp_h = fit_h * zoom / 1000;
    let x = if disp_w > view_w { (disp_w - view_w) / 2 } else { 0 };
    let y = if disp_h > view_h { (disp_h - view_h) / 2 } else { 0 };
    (x as i32, y as i32)
}

pub fn clamp_pan_value(pan: i32, limit: i32) -> i32 {
    pan.clamp(-limit, limit)
}

/// The source pixel shown at a view pixel under a zoom and pan.
///
/// Returns `None` when the point is outside the picture (the letterbox), so
/// the viewer can draw its own background rather than stretching an edge
/// pixel across the gap.
pub fn viewer_source_pixel(
    x: usize,
    y: usize,
    view_w: usize,
    view_h: usize,
    src_w: usize,
    src_h: usize,
    zoom: u32,
    pan_x: i32,
    pan_y: i32,
) -> Option<(usize, usize)> {
    let (fit_w, fit_h) = fit_size(view_w, view_h, src_w, src_h);
    if fit_w == 0 || fit_h == 0 {
        return None;
    }
    let zoom = clamp_viewer_zoom(zoom) as i64;
    let disp_w = (fit_w as i64 * zoom / 1000).max(1);
    let disp_h = (fit_h as i64 * zoom / 1000).max(1);
    let left = (view_w as i64 - disp_w) / 2 + pan_x as i64;
    let top = (view_h as i64 - disp_h) / 2 + pan_y as i64;
    let dx = x as i64 - left;
    let dy = y as i64 - top;
    if dx < 0 || dy < 0 || dx >= disp_w || dy >= disp_h {
        return None;
    }
    let src_x = (dx * src_w as i64 / disp_w).clamp(0, src_w as i64 - 1) as usize;
    let src_y = (dy * src_h as i64 / disp_h).clamp(0, src_h as i64 - 1) as usize;
    Some((src_x, src_y))
}

/// Builds a zoom label such as `1.5x` and returns its length.
pub fn write_zoom_label(zoom: u32, out: &mut [u8; 8]) -> usize {
    let zoom = clamp_viewer_zoom(zoom) as usize;
    let whole = zoom / 1000;
    let tenths = (zoom % 1000) / 100;
    let mut n = append_u64(out, 0, whole as u64);
    n = append(out, n, b".");
    n = append(out, n, &[b'0' + tenths as u8]);
    append(out, n, b"x")
}

pub const MAX_STORAGE_ITEMS: usize = 16;

/// A media file discovered on guest storage (/storage/photos, /storage/pictures, etc.)
#[derive(Clone, Copy, Debug)]
pub struct StorageMediaItem {
    pub name: [u8; 48],
    pub name_len: usize,
    pub path: [u8; 64],
    pub path_len: usize,
    /// The byte length the platform's document index holds for this document.
    pub size: u64,
    /// The modification time the platform's document index holds.
    ///
    /// WHY the index and not the file: the index is where the platform keeps
    /// the fact, and the sheet says which source every number came from.
    pub mtime: u64,
    pub is_video: bool,
    pub is_favourite: bool,
    /// The document's own width, when the app has read its header.
    pub width: u32,
    /// The document's own height, when the app has read its header.
    pub height: u32,
    /// Whether the width and the height are the document's own.
    ///
    /// WHY it is a flag and not a zero width: a zero width is also what a
    /// document the app could not read looks like, and the sheet has to say
    /// which of the two it is showing.
    pub has_dimensions: bool,
    pub primary_color: u32,
    pub secondary_color: u32,
}

impl StorageMediaItem {
    /// The placeholder for a media item the gallery has no pixels for.
    ///
    /// WHY these two colours are not the platform's roles: they are the
    /// *media's* own colours -- the two ends of the gradient a tile is filled
    /// with so a thumbnail-less item still has a depth -- and not the app's
    /// chrome. The app's chrome around it comes from the platform's theme;
    /// see `gallery::theme` for that mapping. A media item that has real
    /// pixels carries the colours sampled from them, which is the same
    /// argument.
    ///
    /// WHY the dimensions are zero and not the screen's: the old placeholder
    /// carried 720 x 1440, the size of the panel this app draws on, and the
    /// sheet printed that as the document's resolution.
    pub const EMPTY: Self = Self {
        name: [0u8; 48],
        name_len: 0,
        path: [0u8; 64],
        path_len: 0,
        size: 0,
        mtime: 0,
        is_video: false,
        is_favourite: false,
        width: 0,
        height: 0,
        has_dimensions: false,
        primary_color: 0xFF1E293B,
        secondary_color: 0xFF0F172A,
    };
}

/// Deletes the storage media item at `idx` by shifting all following items and
/// their cached thumbnails down by one slot, resetting the trailing slot to
/// empty, and decrementing the total count.
///
/// WHY the shift: items in the grid are addressed by their index in the array,
/// and `STORAGE_THUMBS` mirrors `storage_items` one-to-one. Simply decrementing
/// the count and clearing the last slot leaves the tapped item on screen and
/// destroys an unrelated photo at the tail of the gallery.
pub fn delete_storage_item_at(
    items: &mut [StorageMediaItem],
    thumbs: &mut [Thumbnail],
    count: &mut usize,
    idx: usize,
) -> bool {
    if idx >= *count || *count == 0 {
        return false;
    }
    let n = *count;
    for i in idx..n.saturating_sub(1) {
        items[i] = items[i + 1];
        thumbs[i] = thumbs[i + 1];
    }
    items[n - 1] = StorageMediaItem::EMPTY;
    thumbs[n - 1] = Thumbnail::EMPTY;
    *count -= 1;
    true
}

/// The dimensions a document's own header carries, if the header is one the
/// platform can read.
///
/// WHY the header and not the pixels: a header is a few dozen bytes and the
/// dimensions are in it, so the app can measure a document it cannot
/// afford to decode - and can say where the number came from.
pub fn measure_image_header(data: &[u8], name: &[u8]) -> Option<(u32, u32)> {
    if name.ends_with(b".ppm") || (data.len() >= 2 && &data[0..2] == b"P6") {
        return decode_ppm_header(data).ok().map(|(w, h, _)| (w, h));
    }
    if name.ends_with(b".bmp") || (data.len() >= 2 && &data[0..2] == b"BM") {
        return decode_bmp_header(data).ok().map(|(w, h, _, _)| (w, h));
    }
    if name.ends_with(b".qoi") || (data.len() >= 4 && &data[0..4] == b"qoif") {
        return decode_qoi_header(data).ok().map(|(w, h, _, _)| (w, h));
    }
    // WHY these two are measured before they are decoded: the sheet says how
    // large a document is before the user opens it, and a PNG or GIF header
    // carries that number in a few dozen bytes. The decode itself may still
    // refuse an image beyond the app's buffer, and then the sheet keeps the
    // measured size rather than claiming the picture was rendered.
    if name.ends_with(b".png") || (data.len() >= 8 && &data[0..8] == b"\x89PNG\r\n\x1a\n") {
        return decode_png_header(data).ok();
    }
    if name.ends_with(b".gif") || (data.len() >= 6 && (&data[0..6] == b"GIF87a" || &data[0..6] == b"GIF89a")) {
        return decode_gif_header(data).ok();
    }
    None
}

/// What a media line under a title describes.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum MetaSubject {
    /// A still picture.
    Photo,
    /// An animated picture.
    AnimatedGif,
    /// A video document.
    Video,
}

/// The room a media line needs for the widest answer it can build.
///
/// WHY it is a constant rather than a size the call site guesses: every part
/// of the line has a bounded width, and the worst of them together is what the
/// buffer has to hold. A line clipped mid-word is the failure this guards.
pub const META_LINE_MAX: usize = 88;

/// Builds the line under a media title.
///
/// WHY words rather than zeros: the line used to print the picture's
/// dimensions as "0x0" when its header could not be read, and to print a
/// bare "0 bytes" when the file was empty. Both read as measurements, and
/// neither was one. A number is printed only where it was measured, and
/// everything else is said in words.
///
/// WHY the size is optional: a file the platform's index does not hold a
/// length for has no measured size, and saying so is truer than printing a
/// zero that reads like a weight.
pub fn document_meta_line(
    subject: MetaSubject,
    dimensions: Option<(u32, u32)>,
    size: Option<u64>,
    frames: Option<u32>,
    out: &mut [u8],
) -> usize {
    let mut n = match subject {
        MetaSubject::Photo => append(out, 0, b"Photo, "),
        MetaSubject::AnimatedGif => append(out, 0, b"Animated GIF, "),
        MetaSubject::Video => append(out, 0, b"Video, "),
    };

    // WHY a video has no dimensions: this build decodes no video frame, so
    // there is no picture whose size could have been measured.
    if subject != MetaSubject::Video {
        match dimensions {
            Some((w, h)) if w > 0 && h > 0 => {
                n = append_u64(out, n, w as u64);
                n = append(out, n, b"x");
                n = append_u64(out, n, h as u64);
            }
            _ => {
                n = append(out, n, b"dimensions not measured");
            }
        }
        n = append(out, n, b", ");
    }

    if let Some(frames) = frames {
        n = append_u64(out, n, frames as u64);
        n = append(out, n, b" frames, ");
    }

    match size {
        // WHY an empty file is named rather than measured: "0 bytes" reads
        // as a weight, and the useful fact about an empty document is that
        // it holds nothing at all.
        Some(0) => {
            n = append(out, n, b"the file is empty");
        }
        Some(size) => {
            n = append_u64(out, n, size);
            n = append(out, n, b" bytes");
        }
        None => {
            n = append(out, n, b"size not measured");
        }
    }

    if subject == MetaSubject::Video {
        n = append(out, n, b", no frame decoded");
    }

    n
}

/// The number of grid rows a library of `item_count` items takes.
///
/// WHY it is a function: the grid scrolls when the library is taller than the
/// band it is drawn in, and the scroll's extent has to be the same number
/// the rows are laid out with.
pub fn grid_row_count(item_count: usize, columns: usize) -> usize {
    if columns == 0 {
        return 0;
    }
    item_count.div_ceil(columns)
}

/// Supported mobile video codecs.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum VideoCodec {
    /// The container was not parsed, so the codec is not known.
    Undetermined,
}

impl VideoCodec {
    /// A description of what is actually known about the codec.
    ///
    /// WHY this says almost nothing: a file extension names a *container*,
    /// not a codec. An `.mp4` may carry H.264, HEVC or AV1; a `.webm` may
    /// carry VP8, VP9 or AV1; an `.mkv` may carry anything at all. The old
    /// code mapped extensions to codecs and then printed claims such as
    /// "H.264 / AVC 1080p", "VP9 - 60 FPS" and "AV1 - HDR10" -- a codec, a
    /// resolution, a frame rate and a colour profile, none of which had
    /// been read from a file. Until the container is parsed, the codec is
    /// undetermined, and that is what the screen says.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Undetermined => "Codec undetermined (container not parsed)",
        }
    }
}

/// Modern media gallery tabs
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum GalleryTab {
    Photos = 0,
    Videos = 1,
    Albums = 2,
    Screenshots = 3,
}

impl GalleryTab {
    pub fn from_index(index: usize) -> Self {
        match index {
            1 => GalleryTab::Videos,
            2 => GalleryTab::Albums,
            3 => GalleryTab::Screenshots,
            _ => GalleryTab::Photos,
        }
    }
}

/// Video player runtime state with playback controls.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct VideoPlayerState {
    pub is_playing: bool,
    pub position_ms: u32,
    pub duration_ms: u32,
    pub is_fullscreen: bool,
    pub is_fill: bool,
    pub codec: VideoCodec,
    pub width: u32,
    pub height: u32,
}

impl VideoPlayerState {
    /// Creates a player state for a file whose duration is not known.
    ///
    /// WHY the duration is not an argument: the gallery used to open every
    /// video with a hardcoded 30-second duration, which the scrubber, the
    /// elapsed clock and the progress bar then displayed as though it had
    /// been measured. A container's duration is only known once the
    /// container is parsed, so until then the duration is zero and the
    /// screen shows that it is unknown.
    pub const fn new_undetermined() -> Self {
        Self {
            is_playing: false,
            position_ms: 0,
            duration_ms: 0,
            is_fullscreen: false,
            is_fill: false,
            codec: VideoCodec::Undetermined,
            width: 0,
            height: 0,
        }
    }

    /// Whether the media's duration is known.
    pub const fn has_known_duration(&self) -> bool {
        self.duration_ms > 0
    }

    pub fn toggle_play(&mut self) {
        self.is_playing = !self.is_playing;
    }

    pub fn toggle_aspect(&mut self) {
        self.is_fill = !self.is_fill;
    }

    pub fn advance_ticks(&mut self, ticks: u32) {
        if self.is_playing && self.duration_ms > 0 {
            let next = self.position_ms.saturating_add(ticks.saturating_mul(16));
            if next >= self.duration_ms {
                self.position_ms = 0; // loop
            } else {
                self.position_ms = next;
            }
        }
    }

    pub fn seek_to_ratio(&mut self, ratio_milli: u32) {
        let clamped = ratio_milli.min(1000);
        self.position_ms = (self.duration_ms as u64 * clamped as u64 / 1000) as u32;
    }
}

/// Resolves the media item at a flat grid index.
pub fn media_at(index: usize, wallpaper_count: usize, photo_count: usize) -> Option<Media> {
    media_at_with_storage(index, wallpaper_count, photo_count, 0)
}

/// Resolves the media item at a flat grid index including storage items.
pub fn media_at_with_storage(
    index: usize,
    wallpaper_count: usize,
    photo_count: usize,
    storage_count: usize,
) -> Option<Media> {
    if index < wallpaper_count {
        return Some(Media::Wallpaper(index));
    }
    let photo = index - wallpaper_count;
    if photo < photo_count {
        return Some(Media::Photo(photo));
    }
    let storage = photo - photo_count;
    if storage < storage_count {
        return Some(Media::StorageItem(storage));
    }
    None
}

// The image decoders are the platform's, not the gallery's: the launcher paints a
// document the user chose as a wallpaper and the file manager shows what a document
// is, and three programs that each carried their own decoder would be three programs
// that could each be wrong about what a QOI file is. See `interos_std::image`.
pub use interos_std::image::{
    decode_bmp, decode_bmp_header, decode_gif, decode_gif_header, decode_image, decode_png,
    decode_png_header, decode_ppm, decode_ppm_header, decode_qoi, decode_qoi_header, decode_raw,
    encode_ppm, qoi_color_hash,
};

// =============================================================================
// The person's text size, and the geometry that follows it
//
// WHY this block exists: every box in this app was authored as the platform's
// type ladder read at one hundred per cent, so a reader who raised "Text size"
// in Settings got the same plates around *larger* words and the same reading
// surface as before -- the ink moves through the SDK's runtime `*_user_scale`
// faces while the boxes under it do not move at all. Settings writes the level,
// `DynamicType::publish` hands it to the renderer, and the faces under
// [`rows_at`] answer at it. A layout that moves its ink but keeps its box is the
// clipped-text defect this repository gates against; a layout that moves its box
// to a rung the renderer never got is the same defect mirrored, so the rung is
// read from the one cell the renderer draws from.
//
// The authored numbers are kept as the anchor of each derivation rather than
// replaced by it: every `_at` face below answers the number the screen was drawn
// with at [`PLATFORM_LEVEL`], and the sweeps beside them refuse the day it stops
// being true. That is what lets this app adopt the setting without moving a
// pixel for the reader who never raised it.
//
// WHAT MEASURES A RUN cannot be a `const fn`: the SDK's glyph advance tables are
// ordinary functions, so an advance, a trim or a centred x asked for at compile
// time fails to evaluate. Every box below whose *width* comes from a measured
// run is therefore a runtime face, and only the ladder's own vertical metrics --
// cap, em, ascent, line box -- are derived as `const fn` of a level, which is
// what lets the compile-time gates sweep all four selectable rungs.
//
// SCOPE, stated rather than hidden: the faces below drive the album cards the
// Albums destination lays out -- the card's plate, the row stride between two
// cards, the two runs' boxes and the lanes they are trimmed to. The painter and
// the press both read [`album_card_h_at`] and [`album_stride_at`], which is the
// point of doing it here: the tap band moves with the ink instead of being left
// under a card that has grown out from under it. The header, the filter chips,
// the tab row, the media cells and their captions, the timeline section rows, the
// empty states, the inspection bar and the viewer's caption scrim are still
// authored -- adopting those means moving each painter and its hit region in the
// same change, and several of their plates are pinned between two neighbouring
// rows that do not move with them.
// =============================================================================

/// The text-size level every box in this app was authored against.
///
/// WHY it is named: each box is derived twice -- once as a `const fn` of a level,
/// so the compile-time gates can sweep all four selectable levels, and once at
/// run time through the published rung, so the pixels a person gets follow the
/// setting they chose.
pub const PLATFORM_LEVEL: u8 = interos_std::design::font::TEXT_SCALE_NORMAL;

/// The highest level the ladder offers, and therefore the level the gates sweep
/// to. The SDK clamps anything above it on the way in, so a sweep past this rung
/// would be a sweep over a level no renderer can be handed.
pub const MAX_LEVEL: u8 = 3;

/// The rung an album card's name is painted at, and the rung its count line is.
///
/// WHY named here: the vertical derivations below are functions of a rung as well
/// as a level, and a stride grown for the wrong rung is a stride that clips the
/// run it was cut for.
pub const ALBUM_TITLE_RUNG: usize = crate::theme::type_scale::BODY;
pub const ALBUM_COUNT_RUNG: usize = crate::theme::type_scale::CAPTION;

/// The later of two edges or boxes: the max the union bands are walked with.
pub const fn later(a: usize, b: usize) -> usize {
    if a > b {
        a
    } else {
        b
    }
}

/// The cap height a rung inks at on a level.
pub const fn cap_at(scale: usize, level: u8) -> u32 {
    interos_std::design::font::cap_for_user_scale(scale, level)
}

/// The line box a rung reserves on a level.
pub const fn line_at(scale: usize, level: u8) -> usize {
    interos_std::design::font::line_height_for_user_scale(scale, level)
}

/// The ascent a rung's baseline sits above its box's top on a level.
pub const fn ascent_at(scale: usize, level: u8) -> usize {
    interos_std::design::font::ascent_for_user_scale(scale, level)
}

/// The pixels-per-em a rung is rasterised at on a level.
pub const fn em_at(scale: usize, level: u8) -> u32 {
    interos_std::design::font::em_for_user_scale(scale, level)
}

/// The rows a run at `scale` can ink on `level`:
/// [`interos_std::design::outline_text::row_band`]'s union of the outline line
/// box and the bitmap cell, asked at a level rather than at the ladder.
///
/// WHY a restatement and not a call: `row_band` is the platform's own answer and
/// it is a `const fn` of the ladder, so a plate sized from it can never see the
/// setting. The arithmetic below is the same walk -- the lower of the two faces'
/// baselines, the deeper of the two faces' bottoms -- with every metric read
/// through the `*_user_scale` faces, and the gate beside it refuses the day the
/// two answers disagree at the level they were authored at.
pub const fn rows_at(scale: usize, level: u8) -> usize {
    if scale == 0 {
        return 0;
    }
    let em = em_at(scale, level);
    let (outline_ascent, outline_descent) = interos_std::design::typeface::line_metrics(em);
    let outline_ascent = outline_ascent as usize;
    let outline_descent = outline_descent as usize;
    let line_ascent = ascent_at(scale, level);
    let baseline = later(outline_ascent, line_ascent);
    later(baseline + outline_descent, line_at(scale, level))
}

/// An authored vertical stride between two stacked runs, grown by the rows the
/// rung's box gained above the level the stride was authored at.
///
/// WHY this form: a stride is the space one run's box plus its air, and the only
/// honest way to keep the authored pixels at the platform level *and* carry a
/// raised rung is to add exactly the growth the box took. It never shrinks a
/// stride, and at the authored level it is the stride itself.
pub const fn grown(stride: usize, scale: usize, level: u8) -> usize {
    stride + rows_at(scale, level).saturating_sub(rows_at(scale, PLATFORM_LEVEL))
}

/// A plate that reserves the run it carries, at that run's rung, plus its
/// authored air -- and never less than the height the screen was drawn with.
///
/// WHY the floor: the card's height is a number its own fit gates and the press
/// path both read, so a plate may only ever grow. At the authored level this is
/// `authored` exactly, and the air is the authored distance from the run's box to
/// the plate's foot rather than an invented number.
pub const fn plate_at(authored: usize, inset: usize, scale: usize, level: u8, air: usize) -> usize {
    later(authored, inset + rows_at(scale, level) + air)
}

/// The advance a run takes at `scale` on the level `lvl` names.
///
/// WHY not `const`: the SDK's glyph advance tables are ordinary functions, so a
/// measured width can only be asked at run time -- which is why every box whose
/// width depends on a measured run is a runtime face rather than a `const fn`.
pub fn run_w_at(text: &[u8], scale: usize, lvl: u8) -> usize {
    if scale == 0 || text.is_empty() {
        return 0;
    }
    if lvl == PLATFORM_LEVEL {
        return interos_std::design::font::measure(text, scale);
    }
    let w = interos_std::design::font::measure_user_scale(text, scale, lvl);
    if w == 0 {
        interos_std::design::font::measure(text, scale)
    } else {
        w
    }
}

/// `font::ellipsize` at a rung.
///
/// WHY a restatement and not a call: `font::ellipsize` spends its trim budget in
/// the ladder's em, so a run cut to a lane at one rung and inked at another is an
/// ellipsis past the edge of the plate it was trimmed for. At the authored level
/// this *is* `font::ellipsize`, byte for byte.
pub fn ellipsize_run(text: &[u8], scale: usize, max_w: usize, out_buf: &mut [u8]) -> usize {
    let lvl = draw_level();
    if lvl == PLATFORM_LEVEL {
        return interos_std::design::font::ellipsize(text, scale, max_w, out_buf);
    }
    if text.is_empty() || scale == 0 {
        return 0;
    }
    if run_w_at(text, scale, lvl) <= max_w {
        let len = text.len().min(out_buf.len());
        out_buf[..len].copy_from_slice(&text[..len]);
        return len;
    }
    let dots = b"...";
    let dots_w = run_w_at(dots, scale, lvl);
    if dots_w > max_w || out_buf.len() < 3 {
        return 0;
    }
    let em = em_at(scale, lvl);
    let fit_len = longest_fit(text, em, max_w - dots_w, out_buf.len() - 3);
    out_buf[..fit_len].copy_from_slice(&text[..fit_len]);
    out_buf[fit_len..fit_len + 3].copy_from_slice(dots);
    fit_len + 3
}

/// How many leading bytes of `text` a painter can ink inside `max_w` at `em`,
/// given `room` bytes of the caller's buffer for them.
///
/// WHY one decision, not one per byte: the width the painter then answers with
/// is `typeface::text_advance_px`, which validates the whole run once and
/// charges one advance per codepoint when the slice decodes. A byte walk would
/// charge a multi-byte run's continuation bytes as glyphs the painter never
/// inks, so it trims a run wider than the run it is measuring.
fn longest_fit(text: &[u8], em: u32, max_w: usize, room: usize) -> usize {
    let decodes = core::str::from_utf8(text).is_ok();
    let mut current_w = 0usize;
    let mut fit_len = 0usize;
    let mut next = 0usize;
    while next < text.len() {
        let (adv, step) = if decodes {
            match core::str::from_utf8(&text[next..]).ok().and_then(|s| s.chars().next()) {
                Some(ch) => (
                    interos_std::design::typeface::advance_codepoint(ch as u32, em) as usize,
                    ch.len_utf8(),
                ),
                None => break,
            }
        } else {
            (interos_std::design::typeface::advance_px(text[next], em) as usize, 1)
        };
        if current_w + adv > max_w || fit_len + step > room {
            break;
        }
        current_w += adv;
        fit_len = next + step;
        next = fit_len;
    }
    fit_len
}

/// The run a plate actually gets: the trimmed prefix when the lane took the mark,
/// the label whole when it never needed cutting, and a hard cut when it refused.
///
/// WHY a cut and not the label: `ellipsize_run` answers `0` when even the
/// ellipsis will not sit in the lane, and handing the caller the untouched label
/// then inks a run past the plate that framed it -- the words a reader was
/// promised in a card that cannot hold them. The cut is the same answer the lane
/// gives when it does have room, minus the mark.
pub fn fitted_run<'a>(label: &'a [u8], scale: usize, lane: usize, buf: &'a mut [u8]) -> &'a [u8] {
    let len = ellipsize_run(label, scale, lane, buf);
    if len > 0 {
        return &buf[..len];
    }
    if label.is_empty() || scale == 0 {
        return label;
    }
    // WHY the bytes are copied here: a refusal is the one answer `ellipsize_run`
    // gives without writing, so a cut returned straight from the buffer would be
    // the stale zeros the caller zeroed it to, not the label's own front.
    let fit = longest_fit(label, em_at(scale, draw_level()), lane, buf.len());
    buf[..fit].copy_from_slice(&label[..fit]);
    &buf[..fit]
}

// -----------------------------------------------------------------------------
// The album cards: the plate, the stride and the two runs, all at the rung
// -----------------------------------------------------------------------------

/// The album card's authored height: the box the screen has always drawn.
pub const AUTHORED_ALBUM_CARD_H: usize = 180;
/// The authored distance from one album row's top to the next.
pub const AUTHORED_ALBUM_STRIDE: usize = 200;
/// The banner every album card carries. It is a picture, not type, so no rung of
/// the ladder ever moves it and the card's growth is all below it.
pub const ALBUM_BANNER_H: usize = 100;
/// The title's line-box top inside a card, measured from the card's own top.
pub const ALBUM_TITLE_INSET: usize = ALBUM_BANNER_H + 12;
/// The count line's line-box top inside a card, from the card's own top.
pub const ALBUM_COUNT_INSET: usize = ALBUM_BANNER_H + 46;
/// The authored stride from the title's box to the count line's.
pub const ALBUM_COUNT_STRIDE: usize = ALBUM_COUNT_INSET - ALBUM_TITLE_INSET;
/// Air between the count line's box and the card's bottom edge, solved from the
/// authored pair so the authored plate is exactly [`AUTHORED_ALBUM_CARD_H`] at
/// the authored level rather than a number remembered beside it.
pub const ALBUM_BOTTOM_AIR: usize =
    AUTHORED_ALBUM_CARD_H - (ALBUM_COUNT_INSET + rows_at(ALBUM_COUNT_RUNG, PLATFORM_LEVEL));
/// Air between one card's foot and the next card's head.
pub const ALBUM_ROW_AIR: usize = AUTHORED_ALBUM_STRIDE - AUTHORED_ALBUM_CARD_H;

/// The title's line-box top inside a card, at a level.
///
/// WHY the level is unused here: the title sits directly under the banner, and
/// the banner is a picture the ladder never sizes, so the title's top is the
/// card's top plus a fixed inset and never moves with the rung. The level is kept
/// in the signature so the three album faces read as one family.
pub const fn album_title_inset_at(_level: u8) -> usize {
    ALBUM_TITLE_INSET
}

/// The count line's line-box top inside a card, at a level: the title's box grown
/// by the rows the rung took, and nothing else.
pub const fn album_count_inset_at(level: u8) -> usize {
    album_title_inset_at(level) + grown(ALBUM_COUNT_STRIDE, ALBUM_TITLE_RUNG, level)
}

/// The card's height at a level: never below the plate it was drawn with, and tall
/// enough for the rung's count line and its authored air.
pub const fn album_card_h_at(level: u8) -> usize {
    plate_at(
        AUTHORED_ALBUM_CARD_H,
        album_count_inset_at(level),
        ALBUM_COUNT_RUNG,
        level,
        ALBUM_BOTTOM_AIR,
    )
}

/// The distance from one album row's top to the next, at a level: the card the
/// rung drew plus the authored air between two of them.
pub const fn album_stride_at(level: u8) -> usize {
    album_card_h_at(level) + ALBUM_ROW_AIR
}

/// The rung this process draws at: the level the renderer was handed, clamped to
/// the ladder's largest setting so a gate can never sweep a level no face answers.
#[inline]
pub fn draw_level() -> u8 {
    let want = interos_std::design::font::user_text_scale();
    if want > MAX_LEVEL {
        MAX_LEVEL
    } else {
        want
    }
}

// WHY the derivations above are pinned to the ladder at the level they were
// authored at: they are only a faithful restatement of it if every rung answers
// the platform's own number there.
const _: () = assert!(rows_at(ALBUM_TITLE_RUNG, PLATFORM_LEVEL)
    == interos_std::design::outline_text::row_band(0, ALBUM_TITLE_RUNG).1);
const _: () = assert!(rows_at(ALBUM_COUNT_RUNG, PLATFORM_LEVEL)
    == interos_std::design::outline_text::row_band(0, ALBUM_COUNT_RUNG).1);
const _: () = assert!(line_at(ALBUM_TITLE_RUNG, PLATFORM_LEVEL)
    == interos_std::design::font::line_height(ALBUM_TITLE_RUNG));
const _: () = assert!(line_at(ALBUM_COUNT_RUNG, PLATFORM_LEVEL)
    == interos_std::design::font::line_height(ALBUM_COUNT_RUNG));
const _: () = assert!(cap_at(ALBUM_TITLE_RUNG, PLATFORM_LEVEL)
    == interos_std::design::font::cap_for_scale(ALBUM_TITLE_RUNG));
const _: () = assert!(ascent_at(ALBUM_COUNT_RUNG, PLATFORM_LEVEL)
    == interos_std::design::font::ascent_for_scale(ALBUM_COUNT_RUNG));
const _: () = assert!(em_at(ALBUM_TITLE_RUNG, PLATFORM_LEVEL)
    == interos_std::design::font::em_for_scale(ALBUM_TITLE_RUNG));

// WHY these anchors hold to the numbers on the screen: the album card has always
// been 180 rows with its name at 112 and its count at 146, so adopting the
// setting must move no pixel for a reader who left it alone.
const _: () = assert!(album_title_inset_at(PLATFORM_LEVEL) == 112);
const _: () = assert!(album_count_inset_at(PLATFORM_LEVEL) == 146);
const _: () = assert!(album_card_h_at(PLATFORM_LEVEL) == AUTHORED_ALBUM_CARD_H);
const _: () = assert!(album_stride_at(PLATFORM_LEVEL) == AUTHORED_ALBUM_STRIDE);

/// Whether a level's album card holds both of its runs and both fit under the
/// plate the press path cuts.
const fn album_card_holds_its_runs(level: u8) -> bool {
    album_title_inset_at(level) + rows_at(ALBUM_TITLE_RUNG, level) <= album_card_h_at(level)
        && album_count_inset_at(level) + rows_at(ALBUM_COUNT_RUNG, level)
            <= album_card_h_at(level)
        && album_card_h_at(level) <= album_stride_at(level)
}

/// WHY a sweep and not the authored check alone: the whole point of the
/// derivations is the rungs above the authored one, and each claim below is the
/// gate the screen already needs, asked at every level the ladder can be handed
/// -- the card holds its two runs, the stride is never shallower than the card it
/// separates, the plate never falls below the box it was drawn with, and the
/// card's short edge stays above the platform's touch floor at every rung.
const fn album_grid_holds_at_every_level() -> bool {
    if album_card_h_at(PLATFORM_LEVEL) != AUTHORED_ALBUM_CARD_H {
        return false;
    }
    if album_stride_at(PLATFORM_LEVEL) != AUTHORED_ALBUM_STRIDE {
        return false;
    }
    let mut level = 0usize;
    while level <= MAX_LEVEL as usize {
        let lvl = level as u8;
        if !album_card_holds_its_runs(lvl) {
            return false;
        }
        if album_card_h_at(lvl) < AUTHORED_ALBUM_CARD_H {
            return false;
        }
        if level > 0 && album_card_h_at(lvl) < album_card_h_at((level - 1) as u8) {
            return false;
        }
        if album_card_h_at(lvl) < interos_std::design::touch::MIN_TARGET {
            return false;
        }
        level += 1;
    }
    true
}
const _: () = assert!(album_grid_holds_at_every_level());

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_image_suffix_detection() {
        assert!(has_image_suffix(b"photo.jpg"));
        assert!(has_image_suffix(b"IMAGE.PNG"));
        assert!(has_image_suffix(b"graphic.bmp"));
        assert!(has_image_suffix(b"anim.gif"));
        assert!(has_image_suffix(b"shot.webp"));
        assert!(has_image_suffix(b"picture.jpeg"));
        assert!(has_image_suffix(b"screenshot.ppm"));
        assert!(has_image_suffix(b"frame.raw"));

        assert!(!has_image_suffix(b"document.txt"));
        assert!(!has_image_suffix(b"binary.exe"));
        assert!(!has_image_suffix(b"manifest.json"));
        assert!(!has_image_suffix(b".png")); // suffix without base name
        assert!(!has_image_suffix(b""));
    }

    #[test]
    fn test_video_suffix_detection() {
        assert!(has_video_suffix(b"clip.mp4"));
        assert!(has_video_suffix(b"MOVIE.WEBM"));
        assert!(has_video_suffix(b"teaser.mkv"));
        assert!(has_video_suffix(b"stream.mov"));
        assert!(has_video_suffix(b"old.avi"));

        assert!(!has_video_suffix(b"photo.jpg"));
        assert!(!has_video_suffix(b"audio.mp3"));
        assert!(!has_video_suffix(b""));
    }

    #[test]
    fn test_is_media_file() {
        assert!(is_media_file(b"pic.jpg"));
        assert!(is_media_file(b"vid.mp4"));
        assert!(!is_media_file(b"data.bin"));
    }

    #[test]
    fn test_count_listing() {
        let payload = b"wallpaper.jpg|10240|file\nreadme.txt|512|file\npic.PNG|2048|file\nempty_dir|0|dir\n";
        let (entries, images) = count_listing(payload);
        assert_eq!(entries, 4);
        assert_eq!(images, 2);
    }

    #[test]
    fn test_append_u64_formatting() {
        let mut buf = [0u8; 32];
        let n = append_u64(&mut buf, 0, 0);
        assert_eq!(&buf[..n], b"0");

        let n2 = append_u64(&mut buf, 0, 1440);
        assert_eq!(&buf[..n2], b"1440");

        let n3 = append_u64(&mut buf, 0, 9876543210);
        assert_eq!(&buf[..n3], b"9876543210");
    }

    #[test]
    fn test_filter_and_media_at() {
        let f_all = Filter::from_index(0);
        assert!(f_all.shows_wallpapers());
        assert!(f_all.shows_ledger());
        assert!(f_all.shows_videos());

        let f_wall = Filter::from_index(1);
        assert!(f_wall.shows_wallpapers());
        assert!(!f_wall.shows_ledger());
        assert!(!f_wall.shows_videos());

        let f_photo = Filter::from_index(2);
        assert!(!f_photo.shows_wallpapers());
        assert!(f_photo.shows_ledger());
        assert!(!f_photo.shows_videos());

        let f_video = Filter::from_index(3);
        assert!(!f_video.shows_wallpapers());
        assert!(!f_video.shows_ledger());
        assert!(f_video.shows_videos());

        assert_eq!(media_at(0, 3, 2), Some(Media::Wallpaper(0)));
        assert_eq!(media_at(2, 3, 2), Some(Media::Wallpaper(2)));
        assert_eq!(media_at(3, 3, 2), Some(Media::Photo(0)));
        assert_eq!(media_at(4, 3, 2), Some(Media::Photo(1)));
        assert_eq!(media_at(5, 3, 2), None);

        assert_eq!(media_at_with_storage(5, 3, 2, 2), Some(Media::StorageItem(0)));
        assert_eq!(media_at_with_storage(6, 3, 2, 2), Some(Media::StorageItem(1)));
        assert_eq!(media_at_with_storage(7, 3, 2, 2), None);
    }

    #[test]
    fn test_storage_media_and_video_state() {
        let mut item = StorageMediaItem::EMPTY;
        assert_eq!(item.name_len, 0);
        assert!(!item.is_video);
        item.is_video = true;
        item.is_favourite = true;
        assert!(item.is_video);
        assert!(item.is_favourite);

        let mut player = VideoPlayerState::new_undetermined();
        // WHY the duration starts unknown: the gallery cannot parse a
        // container, so it must not claim a length it never read.
        assert!(!player.is_playing);
        assert_eq!(player.position_ms, 0);
        assert_eq!(player.duration_ms, 0);
        assert!(!player.has_known_duration());
        assert_eq!(player.codec.as_str(), "Codec undetermined (container not parsed)");

        // A player with no known duration never advances its position.
        player.advance_ticks(10);
        assert_eq!(player.position_ms, 0);

        // A player with a known duration advances and seeks.
        player.duration_ms = 60_000;
        player.is_playing = true;
        assert!(player.has_known_duration());
        player.advance_ticks(10);
        assert_eq!(player.position_ms, 160);

        player.seek_to_ratio(500); // 50%
        assert_eq!(player.position_ms, 30_000);

        player.toggle_play();
        assert!(!player.is_playing);
        player.advance_ticks(10);
        assert_eq!(player.position_ms, 30_000); // does not advance when paused
    }

    #[test]
    fn test_a_thumbnail_holds_only_decoded_pixels() {
        // WHY this matters: the grid used to fill each stored file with a
        // gradient hashed from its name. A thumbnail is either the real
        // image or it is absent.
        let empty = Thumbnail::EMPTY;
        assert!(!empty.valid);
        assert_eq!(empty.sample(0, 0), None);

        // A two-by-two red image, scaled into the thumbnail.
        let mut rgba = [0u8; 4 * 4];
        for px in rgba.chunks_exact_mut(4) {
            px[0] = 255;
            px[1] = 0;
            px[2] = 0;
            px[3] = 255;
        }
        let thumb = Thumbnail::from_rgba(&rgba, 2, 2).expect("thumbnail");
        assert!(thumb.valid);
        let colour = thumb.sample(0, 0).expect("a pixel");
        assert_eq!(colour & 0x00FF_FFFF, 0x00FF_0000);

        // A zero-sized source is not an image.
        assert!(Thumbnail::from_rgba(&rgba, 0, 2).is_none());
    }

    #[test]
    fn test_a_portrait_thumbnail_is_fitted_not_stretched() {
        // WHY this matters: a capture is 36x72, and the grid's image area is
        // wider than it is tall. Drawing the thumbnail across the whole area
        // squashed the phone screen sideways, so the tile must keep the
        // picture's own aspect and centre it in the area.
        let rgba = [0xFFu8; 36 * 72 * 4];
        let thumb = Thumbnail::from_rgba(&rgba, 36, 72).expect("thumbnail");
        assert_eq!(thumb.fitted_size(214, 174), (87, 174));

        let landscape = Thumbnail::from_rgba(&rgba, 72, 36).expect("thumbnail");
        assert_eq!(landscape.fitted_size(214, 174), (214, 107));

        // An empty thumbnail's placeholder belongs to the whole area.
        assert_eq!(Thumbnail::EMPTY.fitted_size(214, 174), (214, 174));
    }

    #[test]
    fn test_ppm_encode_decode() {
        let rgb_data = [
            255, 0, 0,   // Red
            0, 255, 0,   // Green
            0, 0, 255,   // Blue
            255, 255, 0, // Yellow
        ];
        let mut ppm_buf = [0u8; 64];
        let n = encode_ppm(2, 2, &rgb_data, false, &mut ppm_buf).expect("ppm encode");
        assert!(n > 12);

        let (w, h, offset) = decode_ppm_header(&ppm_buf[..n]).expect("ppm header");
        assert_eq!(w, 2);
        assert_eq!(h, 2);
        assert!(offset > 0);

        let mut out_rgba = [0u8; 16];
        let decoded_len = decode_ppm(&ppm_buf[..n], &mut out_rgba).expect("ppm decode");
        assert_eq!(decoded_len, 16);
        assert_eq!(&out_rgba[0..4], &[255, 0, 0, 255]);
        assert_eq!(&out_rgba[4..8], &[0, 255, 0, 255]);
        assert_eq!(&out_rgba[8..12], &[0, 0, 255, 255]);
        assert_eq!(&out_rgba[12..16], &[255, 255, 0, 255]);
    }

    #[test]
    fn test_bmp_decode() {
        // Minimal 2x2 24bpp BMP
        // File header: 14 bytes
        // DIB header: 40 bytes
        // Pixel data: 2 rows of 2 pixels (6 bytes + 2 bytes padding = 8 bytes per row -> 16 bytes)
        // Total = 70 bytes
        let mut bmp = [0u8; 70];
        // File header
        bmp[0] = b'B';
        bmp[1] = b'M';
        bmp[2..6].copy_from_slice(&(70u32.to_le_bytes()));
        bmp[10..14].copy_from_slice(&(54u32.to_le_bytes())); // data offset
        // DIB header
        bmp[14..18].copy_from_slice(&(40u32.to_le_bytes())); // header size
        bmp[18..22].copy_from_slice(&(2i32.to_le_bytes()));  // width = 2
        bmp[22..26].copy_from_slice(&(2i32.to_le_bytes()));  // height = 2 (bottom-up)
        bmp[26..28].copy_from_slice(&(1u16.to_le_bytes()));  // planes = 1
        bmp[28..30].copy_from_slice(&(24u16.to_le_bytes())); // bpp = 24
        bmp[30..34].copy_from_slice(&(0u32.to_le_bytes()));  // BI_RGB = 0

        // Row 0 (bottom row): Pixel (0, 1): Blue (255, 0, 0 in BGR), Pixel (1, 1): White (255, 255, 255)
        let row0_start = 54;
        bmp[row0_start..row0_start + 3].copy_from_slice(&[255, 0, 0]); // BGR Blue
        bmp[row0_start + 3..row0_start + 6].copy_from_slice(&[255, 255, 255]); // BGR White
        // 2 bytes padding
        bmp[row0_start + 6] = 0;
        bmp[row0_start + 7] = 0;

        // Row 1 (top row): Pixel (0, 0): Red (0, 0, 255 in BGR), Pixel (1, 0): Green (0, 255, 0 in BGR)
        let row1_start = 62;
        bmp[row1_start..row1_start + 3].copy_from_slice(&[0, 0, 255]); // BGR Red
        bmp[row1_start + 3..row1_start + 6].copy_from_slice(&[0, 255, 0]); // BGR Green
        bmp[row1_start + 6] = 0;
        bmp[row1_start + 7] = 0;

        let (w, h, bpp, offset) = decode_bmp_header(&bmp).expect("bmp header");
        assert_eq!(w, 2);
        assert_eq!(h, 2);
        assert_eq!(bpp, 24);
        assert_eq!(offset, 54);

        let mut out_rgba = [0u8; 16];
        let n = decode_bmp(&bmp, &mut out_rgba).expect("bmp decode");
        assert_eq!(n, 16);
        // Top row first in decoded RGBA:
        // (0,0) Red
        assert_eq!(&out_rgba[0..4], &[255, 0, 0, 255]);
        // (1,0) Green
        assert_eq!(&out_rgba[4..8], &[0, 255, 0, 255]);
        // (0,1) Blue
        assert_eq!(&out_rgba[8..12], &[0, 0, 255, 255]);
        // (1,1) White
        assert_eq!(&out_rgba[12..16], &[255, 255, 255, 255]);
    }

    #[test]
    fn test_raw_decode() {
        let raw = [10, 20, 30, 40, 50, 60];
        let mut rgba = [0u8; 8];
        let n = decode_raw(&raw, 2, 1, 24, &mut rgba).expect("raw decode");
        assert_eq!(n, 8);
        assert_eq!(&rgba[0..4], &[10, 20, 30, 255]);
        assert_eq!(&rgba[4..8], &[40, 50, 60, 255]);
    }

    #[test]
    fn test_qoi_suffix_and_decode() {
        assert!(has_image_suffix(b"photo.qoi"));
        assert!(has_image_suffix(b"CAPTURE.QOI"));

        // Minimal valid 1x1 red QOI image (RGBA: 255, 0, 0, 255)
        let mut qoi_data = [
            b'q', b'o', b'i', b'f',
            0, 0, 0, 1, // width = 1
            0, 0, 0, 1, // height = 1
            4, 0,       // channels = 4, colorspace = 0
            0xFE, 255, 0, 0, // QOI_OP_RGB
            0, 0, 0, 0, 0, 0, 0, 1, // end marker
        ];

        let (w, h, ch, _) = decode_qoi_header(&qoi_data).expect("header decode");
        assert_eq!(w, 1);
        assert_eq!(h, 1);
        assert_eq!(ch, 4);

        let mut out_rgba = [0u8; 4];
        let bytes_written = decode_qoi(&qoi_data, &mut out_rgba).expect("pixel decode");
        assert_eq!(bytes_written, 4);
        assert_eq!(out_rgba, [255, 0, 0, 255]);

        // Corrupt magic
        qoi_data[0] = b'x';
        assert_eq!(decode_qoi_header(&qoi_data), Err(()));
    }

    /// The sheet's resolution is the document's own, read out of the
    /// document's own header.
    #[test]
    fn test_a_documents_dimensions_are_measured_from_its_header() {
        // A 2x1 PPM: "P6\n2 1\n255\n" and two pixels.
        let ppm = b"P6\n2 1\n255\n\x01\x02\x03\x04\x05\x06";
        assert_eq!(measure_image_header(ppm, b"a.ppm"), Some((2, 1)));

        // A 1x1 QOI.
        let qoi = [
            b'q', b'o', b'i', b'f', 0, 0, 0, 1, 0, 0, 0, 1, 4, 0, 0xFE, 255, 0, 0, 0, 0, 0,
            0, 1,
        ];
        assert_eq!(measure_image_header(&qoi, b"a.qoi"), Some((1, 1)));

        // A document the platform cannot read the header of has no
        // dimensions to report, and the app says so rather than guessing.
        assert_eq!(measure_image_header(b"not an image", b"a.txt"), None);
        assert_eq!(measure_image_header(b"", b"a.ppm"), None);
    }

    /// The grid's scroll extent is the number of rows the library takes.
    #[test]
    fn test_the_grid_scrolls_when_the_library_is_taller_than_the_band() {
        assert_eq!(grid_row_count(0, 3), 0);
        assert_eq!(grid_row_count(1, 3), 1);
        assert_eq!(grid_row_count(3, 3), 1);
        assert_eq!(grid_row_count(4, 3), 2);
        assert_eq!(grid_row_count(12, 3), 4);
        assert_eq!(grid_row_count(13, 3), 5);
        // A grid of no columns holds nothing.
        assert_eq!(grid_row_count(9, 0), 0);
    }

    /// A media line says in words what it could not measure.
    #[test]
    fn an_unmeasured_fact_is_said_rather_than_zeroed() {
        let mut line = [0u8; META_LINE_MAX];

        // The failure this guards: a download that arrived with no bytes read
        // as "Photo, 0x0, 0 bytes" -- two numbers, neither of them a
        // measurement.
        let n = document_meta_line(MetaSubject::Photo, None, Some(0), None, &mut line);
        assert_eq!(
            &line[..n],
            b"Photo, dimensions not measured, the file is empty"
        );

        let n = document_meta_line(MetaSubject::Photo, Some((640, 480)), Some(20_480), None, &mut line);
        assert_eq!(&line[..n], b"Photo, 640x480, 20480 bytes");

        // A video is not measured by a decoder this build does not have, so
        // its line says what is absent instead of printing a zero frame.
        let n = document_meta_line(MetaSubject::Video, None, Some(4096), None, &mut line);
        assert_eq!(&line[..n], b"Video, 4096 bytes, no frame decoded");

        let n = document_meta_line(
            MetaSubject::AnimatedGif,
            Some((32, 32)),
            Some(900),
            Some(4),
            &mut line,
        );
        assert_eq!(&line[..n], b"Animated GIF, 32x32, 4 frames, 900 bytes");

        // A zero the header reported is not a measurement either.
        let n = document_meta_line(MetaSubject::Photo, Some((0, 0)), Some(9), None, &mut line);
        assert_eq!(&line[..n], b"Photo, dimensions not measured, 9 bytes");
    }

    /// The widest line the builder can form still fits its buffer.
    #[test]
    fn the_widest_media_line_fits_the_buffer() {
        let mut line = [0u8; META_LINE_MAX];
        let n = document_meta_line(
            MetaSubject::AnimatedGif,
            Some((u32::MAX, u32::MAX)),
            Some(u64::MAX),
            Some(u32::MAX),
            &mut line,
        );
        assert!(
            n < META_LINE_MAX,
            "the widest line is {} bytes and the buffer holds {}",
            n,
            META_LINE_MAX
        );
    }

    /// The placeholder carries no dimensions, so nothing can mistake the
    /// screen's size for a document's.
    #[test]
    fn test_an_unreadable_document_carries_no_dimensions() {
        let item = StorageMediaItem::EMPTY;
        assert_eq!(item.width, 0);
        assert_eq!(item.height, 0);
        assert!(!item.has_dimensions);
        assert_eq!(item.size, 0);
        assert_eq!(item.mtime, 0);
    }

    /// Deleting an item at a specific slot shifts all subsequent items and
    /// thumbnails down, clears the vacated end slot, and decrements the count.
    #[test]
    fn test_delete_storage_item_shifts_and_clears_slot() {
        let mut items = [StorageMediaItem::EMPTY; 4];
        let mut thumbs = [Thumbnail::EMPTY; 4];

        // Seed 3 items
        for i in 0..3 {
            items[i].name[0] = b'0' + i as u8;
            items[i].name_len = 1;
            thumbs[i].rgb[0] = (i + 1) as u8 * 10;
            thumbs[i].valid = true;
        }
        let mut count = 3usize;

        // Out-of-bounds deletion is rejected
        assert!(!delete_storage_item_at(&mut items, &mut thumbs, &mut count, 3));
        assert_eq!(count, 3);

        // Delete middle item (index 1)
        assert!(delete_storage_item_at(&mut items, &mut thumbs, &mut count, 1));
        assert_eq!(count, 2);

        // Item 0 is untouched
        assert_eq!(items[0].name[0], b'0');
        assert_eq!(thumbs[0].rgb[0], 10);
        assert!(thumbs[0].valid);

        // Item 1 is now former Item 2
        assert_eq!(items[1].name[0], b'2');
        assert_eq!(thumbs[1].rgb[0], 30);
        assert!(thumbs[1].valid);

        // Trailing slots are cleared to EMPTY
        assert_eq!(items[2].name_len, 0);
        assert!(!thumbs[2].valid);
        assert_eq!(items[3].name_len, 0);
        assert!(!thumbs[3].valid);

        // Delete first item (index 0)
        assert!(delete_storage_item_at(&mut items, &mut thumbs, &mut count, 0));
        assert_eq!(count, 1);
        assert_eq!(items[0].name[0], b'2');
        assert_eq!(thumbs[0].rgb[0], 30);
        assert_eq!(items[1].name_len, 0);
        assert!(!thumbs[1].valid);

        // Delete remaining item
        assert!(delete_storage_item_at(&mut items, &mut thumbs, &mut count, 0));
        assert_eq!(count, 0);
        assert_eq!(items[0].name_len, 0);
        assert!(!thumbs[0].valid);
    }

    #[test]
    fn test_media_at_with_storage_resolution() {
        // 3 wallpapers, 0 photos, 2 storage items
        assert_eq!(media_at_with_storage(0, 3, 0, 2), Some(Media::Wallpaper(0)));
        assert_eq!(media_at_with_storage(1, 3, 0, 2), Some(Media::Wallpaper(1)));
        assert_eq!(media_at_with_storage(2, 3, 0, 2), Some(Media::Wallpaper(2)));
        assert_eq!(media_at_with_storage(3, 3, 0, 2), Some(Media::StorageItem(0)));
        assert_eq!(media_at_with_storage(4, 3, 0, 2), Some(Media::StorageItem(1)));
        assert_eq!(media_at_with_storage(5, 3, 0, 2), None);
    }

    #[test]
    fn test_filter_coverage() {
        assert!(Filter::All.shows_wallpapers());
        assert!(Filter::All.shows_ledger());
        assert!(Filter::All.shows_videos());

        assert!(Filter::Wallpapers.shows_wallpapers());
        assert!(!Filter::Wallpapers.shows_ledger());
        assert!(!Filter::Wallpapers.shows_videos());

        assert!(!Filter::Photos.shows_wallpapers());
        assert!(Filter::Photos.shows_ledger());
        assert!(!Filter::Photos.shows_videos());

        assert!(!Filter::Videos.shows_wallpapers());
        assert!(!Filter::Videos.shows_ledger());
        assert!(Filter::Videos.shows_videos());
    }

    #[test]
    fn test_gallery_ui_text_bounds_and_ellipsize() {
        use interos_std::design::font;
        let mut buf = [0u8; 96];
        let long_name = b"Very_Long_Photograph_Filename_That_Would_Overflow_The_Caption_Band_Or_Centered_Text_Bounds.ppm";
        let cell_w = 216; // typical cell width
        let len = font::ellipsize(long_name, 1, cell_w - 16, &mut buf);
        let disp = if len > 0 { &buf[..len] } else { long_name.as_slice() };
        assert!(font::measure(disp, 1) <= cell_w - 16);
        assert!(disp.ends_with(b"...") || disp == long_name.as_slice());

        // The filter-chip row is gated against the surface it is laid out on,
        // using the geometry the painter itself reads (see `chip_row`). Two facts
        // must hold and neither is a bare-label-vs-literal check any more:
        //   1. every chip seats the counted label it draws, and
        //   2. the chips, laid end to end with their gaps from the grid margin,
        //      end on or before the right edge of the surface.
        // The counted labels are built at the widest counts the app can hold, so
        // the digits the chips carry are inside the row the gate measures.
        let counts = filter_counts(MAX_STORAGE_ITEMS, MAX_STORAGE_ITEMS, MAX_STORAGE_ITEMS);
        let mut scratch = [[0u8; 28]; chip_row::COUNT];
        let mut lens = [0usize; chip_row::COUNT];
        chip_row::counted_label_slices(counts, &mut scratch, &mut lens);
        let labels: [&[u8]; chip_row::COUNT] = [
            &scratch[0][..lens[0]],
            &scratch[1][..lens[1]],
            &scratch[2][..lens[2]],
            &scratch[3][..lens[3]],
        ];
        assert!(
            chip_row::each_chip_holds_text(&labels),
            "a filter chip is narrower than the counted label it draws"
        );
        assert!(
            chip_row::row_fits(&labels, chip_row::SURFACE_W),
            "filter-chip row right edge {} overruns the {}-wide surface",
            chip_row::row_right_edge(&labels),
            chip_row::SURFACE_W
        );
    }

    /// The re-pointed chip gate must actually bite: it refuses a row that
    /// overruns its surface and a chip narrower than the text it draws, rather
    /// than quietly measuring a different (bare) string against a literal.
    #[test]
    fn test_gallery_chip_gate_refuses_an_overrun_row() {
        // The real counted row fits the real surface...
        let counts = filter_counts(MAX_STORAGE_ITEMS, MAX_STORAGE_ITEMS, MAX_STORAGE_ITEMS);
        let mut scratch = [[0u8; 28]; chip_row::COUNT];
        let mut lens = [0usize; chip_row::COUNT];
        chip_row::counted_label_slices(counts, &mut scratch, &mut lens);
        let labels: [&[u8]; chip_row::COUNT] = [
            &scratch[0][..lens[0]],
            &scratch[1][..lens[1]],
            &scratch[2][..lens[2]],
            &scratch[3][..lens[3]],
        ];
        let right = chip_row::row_right_edge(&labels);
        assert!(chip_row::row_fits(&labels, chip_row::SURFACE_W));
        // ...but the same row, on a surface narrowed to just under its own right
        // edge, is refused: the gate tracks the surface, not a fixed 180px pin.
        assert!(!chip_row::row_fits(&labels, right - 1));

        // A longer label than the app draws pushes the row off the real surface.
        let long: [&[u8]; chip_row::COUNT] = [
            b"All",
            b"Wallpapers 16777216 items here",
            b"Photos",
            b"Videos",
        ];
        assert!(!chip_row::row_fits(&long, chip_row::SURFACE_W));

        // And the per-chip hold check is a real gate, not a tautology: it reads
        // the same rung and formula the painter does. The long label's advance is
        // wider than the chip floor on its own, so a chip that was ever clamped
        // to the floor (rather than grown to the text) would refuse to seat it.
        assert!(interos_std::design::font::measure(long[1], chip_row::SCALE) > chip_row::MIN_W);
        assert!(chip_row::each_chip_holds_text(&labels));
    }

    // -------------------------------------------------------------------------
    // The redesigned grid: timeline, filter counts and viewer geometry.
    // -------------------------------------------------------------------------

    fn storage(name: &[u8], is_video: bool, mtime: u64) -> StorageMediaItem {
        let mut item = StorageMediaItem::EMPTY;
        let len = name.len().min(item.name.len());
        item.name[..len].copy_from_slice(&name[..len]);
        item.name_len = len;
        item.path[0] = b'/';
        item.path_len = 1;
        item.is_video = is_video;
        item.mtime = mtime;
        item
    }

    #[test]
    fn test_timeline_groups_are_measured_not_invented() {
        assert_eq!(timeline_group(0, 500), TimelineGroup::Undated);
        assert_eq!(timeline_group(500, 500), TimelineGroup::Newest);
        assert_eq!(timeline_group(400, 500), TimelineGroup::Earlier);

        // The index reports a tick, not a date, so the headings only claim an
        // ordering and an absence; no calendar label exists here.
        assert_eq!(TimelineGroup::Newest.label(), b"Newest first");
        assert_eq!(TimelineGroup::Undated.label(), b"Time not recorded");

        let items = [
            storage(b"undated.ppm", false, 0),
            storage(b"newest.ppm", false, 500),
            storage(b"older.ppm", false, 300),
        ];
        let plan = GridPlan::build(0, 0, &items, Filter::All);
        assert_eq!(plan.entry_count, 3);
        assert_eq!(plan.entries[0], Media::StorageItem(1));
        assert_eq!(plan.entries[1], Media::StorageItem(2));
        assert_eq!(plan.entries[2], Media::StorageItem(0));

        // Six rows: a heading and one media row for each of the three runs.
        assert_eq!(plan.row_count, 6);
        assert_eq!(plan.rows[0], GridRow::Section { group: TimelineGroup::Newest, count: 1 });
        assert_eq!(plan.rows[2], GridRow::Section { group: TimelineGroup::Earlier, count: 1 });
        assert_eq!(plan.rows[4], GridRow::Section { group: TimelineGroup::Undated, count: 1 });
        assert_eq!(plan.content_h, 3 * SECTION_ROW_H + 3 * GRID_CELL);
    }

    #[test]
    fn test_a_library_with_no_times_is_honestly_undated() {
        let items = [
            storage(b"a.ppm", false, 0),
            storage(b"b.ppm", false, 0),
        ];
        let plan = GridPlan::build(0, 0, &items, Filter::All);
        // One section heading, one row holding both cells.
        assert_eq!(plan.row_count, 2);
        assert_eq!(plan.rows[0], GridRow::Section { group: TimelineGroup::Undated, count: 2 });
        assert!(plan.rows[1] == GridRow::Media { first: 0, count: 2 });
        assert_eq!(plan.row_at(0), Some((0, 0)));
        assert_eq!(plan.row_at(SECTION_ROW_H), Some((1, 0)));
        assert_eq!(plan.row_at(plan.content_h), None);
        assert_eq!(plan.media_at(1, 1), Some(Media::StorageItem(1)));
        assert_eq!(plan.media_at(1, 2), None);
        assert_eq!(plan.media_position(Media::StorageItem(1)), Some(1));
        assert_eq!(plan.media_position(Media::Wallpaper(0)), None);
    }

    #[test]
    fn test_filter_counts_label_every_chip() {
        let counts = filter_counts(6, 2, 1);
        assert_eq!(counts.all, 9);
        assert_eq!(Filter::All.count(counts), 9);
        assert_eq!(Filter::Wallpapers.count(counts), 6);
        assert_eq!(Filter::Photos.count(counts), 2);
        assert_eq!(Filter::Videos.count(counts), 1);

        let mut buf = [0u8; 24];
        let n = write_filter_label(Filter::Photos, counts, &mut buf);
        assert_eq!(&buf[..n], b"Photos 2");
        let n = write_filter_label(Filter::Videos, filter_counts(0, 0, 0), &mut buf);
        assert_eq!(&buf[..n], b"Videos 0");
    }

    #[test]
    fn test_filter_state_survives_the_settings_store() {
        assert_eq!(Filter::All.setting_byte(), b'0');
        assert_eq!(Filter::Videos.setting_byte(), b'3');
        assert_eq!(filter_from_setting(b"0"), Filter::All);
        assert_eq!(filter_from_setting(b"1"), Filter::Wallpapers);
        assert_eq!(filter_from_setting(b"2"), Filter::Photos);
        assert_eq!(filter_from_setting(b"3\n"), Filter::Videos);
        assert_eq!(filter_from_setting(b" 2 "), Filter::Photos);
        // A value this build does not understand cannot hide media.
        assert_eq!(filter_from_setting(b"9"), Filter::All);
        assert_eq!(filter_from_setting(b""), Filter::All);
    }

    #[test]
    fn test_plan_selects_exactly_the_filtered_sets() {
        let items = [
            storage(b"photo.ppm", false, 10),
            storage(b"clip.avi", true, 20),
            storage(b"photo2.png", false, 30),
        ];
        let photos = GridPlan::build(2, 0, &items, Filter::Photos);
        assert_eq!(photos.entry_count, 2);
        assert_eq!(photos.entries[0], Media::StorageItem(2));
        assert_eq!(photos.entries[1], Media::StorageItem(0));

        let videos = GridPlan::build(2, 0, &items, Filter::Videos);
        assert_eq!(videos.entry_count, 1);
        assert_eq!(videos.entries[0], Media::StorageItem(1));

        let walls = GridPlan::build(2, 0, &items, Filter::Wallpapers);
        assert_eq!(walls.entry_count, 2);
        assert_eq!(walls.entries[0], Media::Wallpaper(0));
        assert_eq!(walls.entries[1], Media::Wallpaper(1));
        assert_eq!(walls.rows[0], GridRow::Section { group: TimelineGroup::BuiltIn, count: 2 });

        let all = GridPlan::build(2, 0, &items, Filter::All);
        assert_eq!(all.entry_count, 5);
        assert_eq!(all.media_at(1, 0), Some(Media::Wallpaper(0)));
    }

    #[test]
    fn test_viewer_index_bounds_do_not_wrap() {
        assert_eq!(viewer_step_index(0, 3, true), Some(1));
        assert_eq!(viewer_step_index(2, 3, true), None);
        assert_eq!(viewer_step_index(0, 3, false), None);
        assert_eq!(viewer_step_index(2, 3, false), Some(1));
        assert_eq!(viewer_step_index(0, 0, true), None);

        // Deleting the shown item selects the one that took its place; the
        // last item's removal selects the new last item.
        assert_eq!(viewer_index_after_removal(1, 1, 2), Some(1));
        assert_eq!(viewer_index_after_removal(2, 1, 2), Some(1));
        assert_eq!(viewer_index_after_removal(0, 1, 2), Some(0));
        assert_eq!(viewer_index_after_removal(0, 0, 0), None);
    }

    #[test]
    fn test_viewer_zoom_pan_and_source_mapping_stay_in_bounds() {
        assert_eq!(clamp_viewer_zoom(500), VIEWER_MIN_ZOOM);
        assert_eq!(clamp_viewer_zoom(9000), VIEWER_MAX_ZOOM);
        assert_eq!(double_tap_zoom(1000), 2000);
        assert_eq!(double_tap_zoom(2000), 1000);

        // A 100x50 picture fitted into a 200x200 view: 200x100, centred.
        assert_eq!(fit_size(200, 200, 100, 50), (200, 100));
        assert_eq!(fit_size(200, 200, 50, 100), (100, 200));
        assert_eq!(viewer_display_size(200, 100, 2000), (400, 200));
        assert_eq!(viewer_pan_limit(200, 200, 200, 100, 1000), (0, 0));
        assert_eq!(viewer_pan_limit(200, 200, 200, 100, 2000), (100, 0));
        assert_eq!(clamp_pan_value(500, 100), 100);
        assert_eq!(clamp_pan_value(-500, 100), -100);

        // At fit, the centre of the view is the centre of the picture.
        assert_eq!(
            viewer_source_pixel(100, 100, 200, 200, 100, 50, 1000, 0, 0),
            Some((50, 25))
        );
        // Above and below the letterbox is outside the picture, not its edge.
        assert_eq!(viewer_source_pixel(100, 0, 200, 200, 100, 50, 1000, 0, 0), None);
        assert_eq!(viewer_source_pixel(100, 199, 200, 200, 100, 50, 1000, 0, 0), None);
        // At 2x the picture is 400 wide with its left edge at -100.
        assert_eq!(
            viewer_source_pixel(0, 100, 200, 200, 100, 50, 2000, 0, 0),
            Some((25, 25))
        );

        let mut label = [0u8; 8];
        let n = write_zoom_label(1500, &mut label);
        assert_eq!(&label[..n], b"1.5x");
        let n = write_zoom_label(3000, &mut label);
        assert_eq!(&label[..n], b"3.0x");
        let n = write_zoom_label(1000, &mut label);
        assert_eq!(&label[..n], b"1.0x");
    }

    #[test]
    fn test_delete_selection_names_only_stored_media() {
        assert_eq!(deletable_storage_index(Media::StorageItem(2), 3), Some(2));
        assert_eq!(deletable_storage_index(Media::StorageItem(3), 3), None);
        // A built-in wallpaper has no file to unlink.
        assert_eq!(deletable_storage_index(Media::Wallpaper(0), 3), None);
        assert_eq!(deletable_storage_index(Media::Photo(0), 3), None);
    }

    #[test]
    fn test_extract_video_thumbnail_ppm() {
        let mut ppm_data = [0u8; 64];
        let header = b"P6
2 2
255
";
        ppm_data[..header.len()].copy_from_slice(header);
        let pixels = [
            255, 0, 0,
            0, 255, 0,
            0, 0, 255,
            255, 255, 0,
        ];
        ppm_data[header.len()..header.len() + 12].copy_from_slice(&pixels);

        let mut scratch = [0u8; 96 * 96 * 4];
        let thumb = extract_video_thumbnail(&ppm_data[..header.len() + 12], b"test.ppm", &mut scratch);
        assert!(thumb.is_some());
        let t = thumb.unwrap();
        assert!(t.valid);
        assert_eq!(t.src_w, 2);
        assert_eq!(t.src_h, 2);
    }

    #[test]
    fn test_extract_video_thumbnail_unsupported_h264_mp4() {
        let mut scratch = [0u8; 96 * 96 * 4];
        let fake_mp4 = b"....ftypmp42";
        let thumb = extract_video_thumbnail(fake_mp4, b"movie.mp4", &mut scratch);
        assert!(thumb.is_none());

        let thumb_h264 = extract_video_thumbnail(b"   g", b"video.h264", &mut scratch);
        assert!(thumb_h264.is_none());
    }

    #[test]
    fn test_find_avi_first_mjpeg_frame() {
        assert_eq!(find_avi_first_mjpeg_frame(b"not an avi"), None);
    }

    #[test]
    fn test_extract_video_thumbnail_avi_mjpeg() {
        const AVI_FIXTURE: &[u8] = include_bytes!("../../video_player/tests/fixtures/mjpeg_5fps_audio.avi");
        let mut scratch = [0u8; 96 * 96 * 4];
        let thumb = extract_video_thumbnail(AVI_FIXTURE, b"clip.avi", &mut scratch);
        assert!(thumb.is_some());
        let t = thumb.unwrap();
        assert!(t.valid);
        assert_eq!(t.src_w, 64);
        assert_eq!(t.src_h, 48);
    }

    /// The album card stacks its name and its count 34 rows apart while the name's
    /// own line box is 39 rows deep at the authored rung, so the count line's box
    /// opens five rows inside the name's. That is the layout the screen has always
    /// been drawn with, and this change is about the rung rather than about it, so
    /// what is pinned here is what the ladder does to it: every rung above the
    /// authored one grows the name's box and the stride below it by the same rows,
    /// which leaves the overlap exactly as it was found, and the rung below the
    /// authored one keeps the authored stride rather than shrinking it, which
    /// separates the two boxes instead of pushing the count line up into a name.
    /// The five-row overlap itself is unfinished work, not a contract.
    #[test]
    fn the_album_runs_keep_their_gap_at_every_rung() {
        let gap_at = |lvl: u8| {
            album_count_inset_at(lvl) as isize
                - (album_title_inset_at(lvl) + rows_at(ALBUM_TITLE_RUNG, lvl)) as isize
        };
        let authored = gap_at(PLATFORM_LEVEL);
        assert_eq!(
            authored, -5,
            "the album name's box and its count line moved against each other at the \
             authored rung, where the card was drawn 34 rows apart behind a 39-row band"
        );
        for lvl in PLATFORM_LEVEL..=MAX_LEVEL {
            assert_eq!(
                gap_at(lvl),
                authored,
                "rung {} changed the overlap between the album name's box and its count line",
                lvl
            );
        }
        assert!(
            gap_at(0) >= 0,
            "the smallest rung put the count line {} rows inside the album name's box",
            -gap_at(0)
        );
    }

    /// A plate too small to hold the three-byte mark is the one refusal
    /// `ellipsize_run` answers without writing a byte, and the card still has to be
    /// handed bytes its own name says: the cut copied into the caller's buffer,
    /// never whatever that buffer was holding before.
    ///
    /// WHY the refusal is reached through the plate and not the lane: no album card
    /// is narrower than the mark -- the gate beside `ALBUM_TEXT_LANE` refuses the
    /// day one is -- so a lane cut is the app's own case, while a one- or two-byte
    /// plate is the only way to make the trim say no inside a test.
    #[test]
    fn a_refused_trim_copies_its_cut_into_the_plate_it_was_given() {
        let long = [b'e'; 240];
        // Wide enough for a glyph or two at every rung, far short of the whole run,
        // so the plate alone is what makes the trim refuse.
        let lane = 200usize;
        for room in [1usize, 2] {
            let mut backing = [0u8; 2];
            let cut = fitted_run(&long, ALBUM_TITLE_RUNG, lane, &mut backing[..room]);
            assert!(
                !cut.is_empty() && cut.len() <= room,
                "a {}-byte plate was answered with {} bytes",
                room,
                cut.len()
            );
            assert!(
                long.starts_with(cut),
                "a {}-byte plate was handed bytes its label never had",
                room
            );
        }
    }
}

