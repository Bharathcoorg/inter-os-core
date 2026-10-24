#![no_std]

pub use interos_std as std;

/// The app's view of the platform's design system.
pub mod theme;

/// The image file extensions the platform recognises.
pub use interos_std::image::IMAGE_SUFFIXES;

pub const VIDEO_SUFFIXES: [&[u8]; 5] = [
    b".mp4", b".webm", b".mkv", b".avi", b".mov",
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

    pub fn shows_wallpapers(self) -> bool {
        matches!(self, Filter::All | Filter::Wallpapers)
    }

    pub fn shows_ledger(self) -> bool {
        matches!(self, Filter::All | Filter::Photos)
    }

    pub fn shows_videos(self) -> bool {
        matches!(self, Filter::All | Filter::Videos)
    }
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
    None
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
    decode_bmp, decode_bmp_header, decode_image, decode_ppm, decode_ppm_header,
    decode_qoi, decode_qoi_header, decode_raw, encode_ppm, qoi_color_hash,
};

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
}
