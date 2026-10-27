#![no_std]
//! InterOS File Manager VFS parser and data models.

extern crate interos_std;

use interos_std::design::primitives::Rect;
use interos_std::design::scroll::ScrollState;

/// The app's view of the platform's design system.
pub mod theme;

/// The most entries one listing may hold.
///
/// WHY 32 and not 8: the card is a fixed region with a scroll inside it, so
/// the number of entries the app can hold is what decides whether a long
/// directory is reachable. Eight entries fit the region exactly, which means
/// the scroll never had anything to scroll.
pub const MAX_PARSED_ENTRIES: usize = 32;
pub const MAX_ENTRY_NAME_LEN: usize = 32;

// -----------------------------------------------------------------------------
// The two screens
//
// WHY the app has a home screen and a browse screen and not one screen with
// everything on it: the storage card, the index's recents and the four
// directory entry points answer "where are my files"; the browse screen answers
// "what is in this folder". One screen holding both would have to scroll a
// control the other one owns.
// -----------------------------------------------------------------------------

/// Which of the app's screens is on the surface.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ViewMode {
    /// The storage insight card, the index's recents and the category entries.
    Home,
    /// One directory: breadcrumbs, the search scopes, the list or grid and the
    /// selection actions.
    Browse,
}

/// How the browse screen lays its items out.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum LayoutMode {
    List,
    Grid,
}

impl LayoutMode {
    /// The other layout.
    ///
    /// WHY toggling the layout touches no selection: the items keep their
    /// order, so a marked row is the same file in either layout. The app calls
    /// this instead of resetting anything.
    pub const fn toggle(self) -> Self {
        match self {
            LayoutMode::List => LayoutMode::Grid,
            LayoutMode::Grid => LayoutMode::List,
        }
    }

    pub const fn label(self) -> &'static [u8] {
        match self {
            LayoutMode::List => b"List",
            LayoutMode::Grid => b"Grid",
        }
    }
}

/// The order the browse screen lists a directory in.
///
/// WHY these two keys and not "modified": a directory listing carries a name
/// and a byte length, and the platform's index is the only place a modification
/// time lives. Sorting a listing by a time it does not hold would order the
/// rows by a number the app made up.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SortMode {
    Name,
    Size,
}

impl SortMode {
    pub const fn toggle(self) -> Self {
        match self {
            SortMode::Name => SortMode::Size,
            SortMode::Size => SortMode::Name,
        }
    }

    pub const fn label(self) -> &'static [u8] {
        match self {
            SortMode::Name => b"Name",
            SortMode::Size => b"Size",
        }
    }
}

/// What a search looks at.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SearchScope {
    /// Only documents whose path is inside the folder on screen.
    ThisFolder,
    /// Every document the platform's index holds.
    AllStorage,
}

impl SearchScope {
    pub const fn toggle(self) -> Self {
        match self {
            SearchScope::ThisFolder => SearchScope::AllStorage,
            SearchScope::AllStorage => SearchScope::ThisFolder,
        }
    }

    pub const fn label(self) -> &'static [u8] {
        match self {
            SearchScope::ThisFolder => b"This folder",
            SearchScope::AllStorage => b"All storage",
        }
    }
}

/// A bounded, zero-heap directory entry parsed from kernel VFS `fs_list`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct RuntimeFileEntry {
    pub name: [u8; MAX_ENTRY_NAME_LEN],
    pub name_len: usize,
    pub size: u64,
    pub size_label: [u8; 16],
    pub size_label_len: usize,
    pub is_dir: bool,
}

impl RuntimeFileEntry {
    pub const EMPTY: Self = Self {
        name: [0u8; MAX_ENTRY_NAME_LEN],
        name_len: 0,
        size: 0,
        size_label: [0u8; 16],
        size_label_len: 0,
        is_dir: false,
    };
}

/// Formats `v` as decimal ASCII into `out`; returns the bytes written.
pub fn fmt_u64(mut v: u64, out: &mut [u8]) -> usize {
    if out.is_empty() {
        return 0;
    }
    if v == 0 {
        out[0] = b'0';
        return 1;
    }
    let mut digits = [0u8; 20];
    let mut n = 0usize;
    while v > 0 && n < 20 {
        digits[n] = b'0' + (v % 10) as u8;
        v /= 10;
        n += 1;
    }
    let n = n.min(out.len());
    for i in 0..n {
        out[i] = digits[n - 1 - i];
    }
    n
}

/// Parses newline-delimited VFS entries in `<name>|<size>|<dir|dev|file>` format.
pub fn parse_vfs_entries(data: &[u8], entries: &mut [RuntimeFileEntry; MAX_PARSED_ENTRIES]) -> usize {
    let mut entry_count = 0;
    let mut start = 0;
    while start < data.len() && entry_count < MAX_PARSED_ENTRIES {
        let end = data[start..]
            .iter()
            .position(|&b| b == b'\n')
            .map(|p| start + p)
            .unwrap_or(data.len());
        let line = &data[start..end];
        if !line.is_empty() {
            let mut parts = line.split(|&b| b == b'|');
            if let (Some(name_bytes), Some(size_bytes), Some(type_bytes)) =
                (parts.next(), parts.next(), parts.next())
            {
                // The browser writes the origin URL of a download into a
                // `.origin` companion file. That file is metadata, not
                // something the person downloaded, so it is not listed.
                if name_bytes.ends_with(b".origin") {
                    start = end.saturating_add(1);
                    continue;
                }
                let mut entry = RuntimeFileEntry::EMPTY;
                let n = name_bytes.len().min(MAX_ENTRY_NAME_LEN);
                entry.name[..n].copy_from_slice(&name_bytes[..n]);
                entry.name_len = n;
                let mut size: u64 = 0;
                for &b in size_bytes {
                    if b.is_ascii_digit() {
                        size = size.saturating_mul(10).saturating_add((b - b'0') as u64);
                    }
                }
                entry.size = size;
                entry.is_dir = type_bytes == b"dir";
                if entry.is_dir {
                    entry.size_label[..3].copy_from_slice(b"DIR");
                    entry.size_label_len = 3;
                } else {
                    let sn = fmt_u64(entry.size, &mut entry.size_label);
                    if sn + 2 <= 16 {
                        entry.size_label[sn..sn + 2].copy_from_slice(b" B");
                        entry.size_label_len = sn + 2;
                    } else {
                        entry.size_label_len = sn;
                    }
                }
                entries[entry_count] = entry;
                entry_count += 1;
            }
        }
        start = end.saturating_add(1);
    }
    entry_count
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum FileMime {
    Directory,
    Image,
    Video,
    Audio,
    Document,
    Other,
}

pub const IMAGE_SUFFIXES: [&[u8]; 9] = [
    b".jpg", b".jpeg", b".png", b".bmp", b".gif", b".webp", b".qoi", b".ppm", b".raw",
];

pub const VIDEO_SUFFIXES: [&[u8]; 8] = [
    b".mp4", b".m4v", b".webm", b".mkv", b".avi", b".mov", b".mjpg", b".mjpeg",
];

pub const AUDIO_SUFFIXES: [&[u8]; 7] = [
    b".mp3", b".wav", b".ogg", b".flac", b".aac", b".m4a", b".pcm",
];

pub const DOC_SUFFIXES: [&[u8]; 10] = [
    b".txt", b".pdf", b".md", b".doc", b".json", b".log", b".csv", b".htm", b".html", b".xml",
];

pub fn has_suffix(name: &[u8], suffix: &[u8]) -> bool {
    name.len() > suffix.len() && name[name.len() - suffix.len()..].eq_ignore_ascii_case(suffix)
}

/// The media type of a file the document index has not described yet.
///
/// WHY this mirrors the kernel's extension table: the index classifies a file
/// from its own bytes, and this answers for a path the index has not reached.
/// If the two tables disagreed, one file would be described two ways depending
/// on when it was looked at. A type the platform cannot open is still named
/// accurately here; the intent registry, not this table, decides who opens it.
pub fn mime_for_name(name: &[u8], is_video_dir: bool) -> &'static str {
    // A frame sequence recorded into the video area belongs to a video even
    // though its bytes look like a still picture.
    if is_video_dir {
        if has_suffix(name, b".ppm") {
            return "video/x-ppm";
        }
        if has_suffix(name, b".raw") {
            return "video/x-raw";
        }
    }
    if has_suffix(name, b".jpg") || has_suffix(name, b".jpeg") {
        return "image/jpeg";
    }
    if has_suffix(name, b".png") {
        return "image/png";
    }
    if has_suffix(name, b".gif") {
        return "image/gif";
    }
    if has_suffix(name, b".bmp") {
        return "image/bmp";
    }
    if has_suffix(name, b".webp") {
        return "image/webp";
    }
    if has_suffix(name, b".qoi") {
        return "image/qoi";
    }
    if has_suffix(name, b".ppm") {
        return "image/x-portable-pixmap";
    }
    if has_suffix(name, b".svg") {
        return "image/svg+xml";
    }
    if has_suffix(name, b".mp4") || has_suffix(name, b".m4v") {
        return "video/mp4";
    }
    if has_suffix(name, b".mov") {
        return "video/quicktime";
    }
    if has_suffix(name, b".webm") {
        return "video/webm";
    }
    if has_suffix(name, b".mkv") {
        return "video/x-matroska";
    }
    if has_suffix(name, b".avi") {
        return "video/x-msvideo";
    }
    if has_suffix(name, b".mjpg") || has_suffix(name, b".mjpeg") {
        return "video/x-motion-jpeg";
    }
    if has_suffix(name, b".mp3") {
        return "audio/mpeg";
    }
    if has_suffix(name, b".wav") {
        return "audio/wav";
    }
    if has_suffix(name, b".pcm") {
        return "audio/pcm";
    }
    if has_suffix(name, b".ogg") {
        return "audio/ogg";
    }
    if has_suffix(name, b".flac") {
        return "audio/flac";
    }
    if has_suffix(name, b".aac") {
        return "audio/aac";
    }
    if has_suffix(name, b".m4a") {
        return "audio/mp4";
    }
    if has_suffix(name, b".json") {
        return "application/json";
    }
    if has_suffix(name, b".pdf") {
        return "application/pdf";
    }
    if has_suffix(name, b".md") {
        return "text/markdown";
    }
    if has_suffix(name, b".csv") {
        return "text/csv";
    }
    if has_suffix(name, b".htm") || has_suffix(name, b".html") {
        return "text/html";
    }
    if has_suffix(name, b".xml") {
        return "application/xml";
    }
    if has_suffix(name, b".zip") {
        return "application/zip";
    }
    if has_suffix(name, b".doc") {
        return "application/msword";
    }
    if has_suffix(name, b".rapp") {
        return "application/vnd.interos.rapp";
    }
    if has_suffix(name, b".note") {
        return "text/x-interos-note";
    }
    if has_suffix(name, b".txt") || has_suffix(name, b".log") {
        return "text/plain";
    }
    "application/octet-stream"
}

pub fn detect_mime(name: &[u8], is_dir: bool) -> FileMime {
    if is_dir {
        return FileMime::Directory;
    }
    if IMAGE_SUFFIXES.iter().any(|s| has_suffix(name, s)) {
        return FileMime::Image;
    }
    if VIDEO_SUFFIXES.iter().any(|s| has_suffix(name, s)) {
        return FileMime::Video;
    }
    if AUDIO_SUFFIXES.iter().any(|s| has_suffix(name, s)) {
        return FileMime::Audio;
    }
    if DOC_SUFFIXES.iter().any(|s| has_suffix(name, s)) {
        return FileMime::Document;
    }
    FileMime::Other
}


// -----------------------------------------------------------------------------
// The document index and the intent registry
// -----------------------------------------------------------------------------

/// The longest path a selected document's name is held at.
pub const MAX_DOCUMENT_PATH_LEN: usize = 128;
/// The longest media type a document's record is held at.
pub const MAX_DOCUMENT_MIME_LEN: usize = 48;
/// The most handlers one media type may name.
pub const MAX_DOCUMENT_HANDLERS: usize = 4;
/// The longest package id a handler's name is held at.
pub const MAX_HANDLER_ID_LEN: usize = 40;

/// One package that declared it handles a media type.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct HandlerEntry {
    /// The package id, as the registry spells it.
    pub package_id: [u8; MAX_HANDLER_ID_LEN],
    /// How many bytes of the package id are meaningful.
    pub package_id_len: usize,
}

impl HandlerEntry {
    /// The empty handler.
    pub const EMPTY: Self = Self {
        package_id: [0u8; MAX_HANDLER_ID_LEN],
        package_id_len: 0,
    };
}

/// A document the platform's index holds, together with the handlers that
/// declared its media type.
///
/// WHY this is one type: a file and the programs that can open it are one
/// fact about the device, and a screen that showed the file without the
/// handlers would have to look them up again.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct SelectedDocument {
    /// The document's absolute path.
    pub path: [u8; MAX_DOCUMENT_PATH_LEN],
    /// How many bytes of the path are meaningful.
    pub path_len: usize,
    /// The document's media type, as the platform's index reports it.
    pub mime: [u8; MAX_DOCUMENT_MIME_LEN],
    /// How many bytes of the media type are meaningful.
    pub mime_len: usize,
    /// Where the media type answer came from: `bytes`, `extension`,
    /// `unknown`, or an explicit download origin URL.
    pub provenance: [u8; 128],
    /// How many bytes of the provenance are meaningful.
    pub provenance_len: usize,
    /// The document's length in bytes.
    pub size: u64,
    /// The packages that declared a handler for the document's media type.
    pub handlers: [HandlerEntry; MAX_DOCUMENT_HANDLERS],
    /// How many handlers the document has.
    pub handler_count: usize,
}

impl SelectedDocument {
    /// The empty selection.
    pub const EMPTY: Self = Self {
        path: [0u8; MAX_DOCUMENT_PATH_LEN],
        path_len: 0,
        mime: [0u8; MAX_DOCUMENT_MIME_LEN],
        mime_len: 0,
        provenance: [0u8; 128],
        provenance_len: 0,
        size: 0,
        handlers: [HandlerEntry::EMPTY; MAX_DOCUMENT_HANDLERS],
        handler_count: 0,
    };

    /// The document's path as text.
    pub fn path_str(&self) -> &str {
        core::str::from_utf8(&self.path[..self.path_len]).unwrap_or("")
    }

    /// The document's media type as text.
    pub fn mime_str(&self) -> &str {
        core::str::from_utf8(&self.mime[..self.mime_len]).unwrap_or("")
    }

    /// Where the media type answer came from.
    pub fn provenance_str(&self) -> &str {
        core::str::from_utf8(&self.provenance[..self.provenance_len]).unwrap_or("")
    }

    /// Whether the media type was read from the document's own bytes.
    pub fn mime_from_bytes(&self) -> bool {
        self.provenance_str() == "bytes"
    }

    /// Whether any package declared a handler for the document's media type.
    pub fn has_handler(&self) -> bool {
        self.handler_count > 0
    }

    /// The first package that declared a handler for the document's media
    /// type.
    pub fn first_handler(&self) -> Option<&str> {
        if self.handler_count == 0 {
            return None;
        }
        let handler = &self.handlers[0];
        core::str::from_utf8(&handler.package_id[..handler.package_id_len]).ok()
    }
}

/// Reads the device's document index and finds the record for a path.
///
/// WHY the index and not a directory listing: the index is the platform's one
/// description of the documents the device holds, and it carries the media
/// type and where that media type came from. A listing by name would give
/// this app a second notion of what a file is.
pub fn document_from_index(path: &str) -> Option<SelectedDocument> {
    let mut raw = [0u8; 2048];
    let n = interos_std::content::content_index_list(&mut raw).ok()?;
    let mut handler_raw = [0u8; 2048];
    let text = core::str::from_utf8(&raw[..n]).ok()?;
    for line in text.split('\n') {
        if line.is_empty() {
            continue;
        }
        let record = interos_std::content::DocumentRecord::parse(line)?;
        if record.path != path {
            continue;
        }
        let mut selected = SelectedDocument::EMPTY;
        let path_len = record.path.len().min(MAX_DOCUMENT_PATH_LEN);
        selected.path[..path_len].copy_from_slice(&record.path.as_bytes()[..path_len]);
        selected.path_len = path_len;
        let is_video_dir = record.path.starts_with("/storage/videos") || record.path.contains("/videos/");
        let mime_override = if is_video_dir && (record.path.ends_with(".ppm") || record.path.ends_with(".PPM")) {
            "video/x-ppm"
        } else if is_video_dir && (record.path.ends_with(".raw") || record.path.ends_with(".RAW")) {
            "video/x-raw"
        } else if is_video_dir && (record.path.ends_with(".mjpg") || record.path.ends_with(".MJPG") || record.path.ends_with(".mjpeg") || record.path.ends_with(".MJPEG")) {
            "video/x-motion-jpeg"
        } else {
            record.mime
        };
        let mime_len = mime_override.len().min(MAX_DOCUMENT_MIME_LEN);
        selected.mime[..mime_len].copy_from_slice(&mime_override.as_bytes()[..mime_len]);
        selected.mime_len = mime_len;
        let provenance_len = record.provenance.len().min(selected.provenance.len());
        selected.provenance[..provenance_len]
            .copy_from_slice(&record.provenance.as_bytes()[..provenance_len]);
        selected.provenance_len = provenance_len;
        selected.size = record.length;
        selected.handler_count = handlers_for_mime(mime_override, &mut handler_raw, &mut selected.handlers);
        return Some(selected);
    }
    None
}

/// Resolves a selected document and its handlers from a path, falling back to
/// extension MIME detection and the system intent registry when unindexed.
pub fn document_from_path(path: &str) -> Option<SelectedDocument> {
    if let Some(doc) = document_from_index(path) {
        return Some(doc);
    }
    let bytes = path.as_bytes();
    let is_video_dir = path.starts_with("/storage/videos") || path.contains("/videos/");
    let mime_str = if path.ends_with('/') {
        "inode/directory"
    } else {
        mime_for_name(bytes, is_video_dir)
    };
    let mut selected = SelectedDocument::EMPTY;
    let plen = bytes.len().min(MAX_DOCUMENT_PATH_LEN);
    selected.path[..plen].copy_from_slice(&bytes[..plen]);
    selected.path_len = plen;
    let mbytes = mime_str.as_bytes();
    let mlen = mbytes.len().min(MAX_DOCUMENT_MIME_LEN);
    selected.mime[..mlen].copy_from_slice(&mbytes[..mlen]);
    selected.mime_len = mlen;
    selected.provenance[..9].copy_from_slice(b"extension");
    selected.provenance_len = 9;
    let mut handler_raw = [0u8; 2048];
    selected.handler_count = handlers_for_mime(mime_str, &mut handler_raw, &mut selected.handlers);
    Some(selected)
}

/// The packages that declared a handler for a media type.
///
/// WHY the registry and not a table in this app: "who opens this file" is a
/// question about the whole device, and an answer that lives in one app is an
/// answer that app can edit. The registry is built from the packages' signed
/// manifests, so the answer is a property of what is installed.
pub fn handlers_for_mime(
    mime: &str,
    raw: &mut [u8],
    out: &mut [HandlerEntry; MAX_DOCUMENT_HANDLERS],
) -> usize {
    let n = match interos_std::content::intent_list(raw) {
        Ok(n) => n,
        Err(_) => return 0,
    };
    let text = match core::str::from_utf8(&raw[..n]) {
        Ok(text) => text,
        Err(_) => return 0,
    };
    let mut count = 0usize;
    for line in text.split('\n') {
        if line.is_empty() || count >= out.len() {
            break;
        }
        let mut parts = line.split('|');
        let kind = parts.next().unwrap_or("");
        let value = parts.next().unwrap_or("");
        let package_id = parts.next().unwrap_or("");
        if kind != "mime" || value != mime {
            continue;
        }
        let id_len = package_id.len().min(MAX_HANDLER_ID_LEN);
        out[count].package_id[..id_len].copy_from_slice(&package_id.as_bytes()[..id_len]);
        out[count].package_id_len = id_len;
        count += 1;
    }
    count
}

/// The action the platform offers for a media type, if any.
///
/// WHY this is derived from the media type and not from a table of
/// applications: what a document may become is a property of the kind of
/// document it is, and the packages that declared those actions are the ones
/// the registry knows about. An empty answer means the platform offers no
/// such action for this media type, and the app must then offer none.
pub fn action_for_mime(mime: &str) -> &'static [u8] {
    if mime.starts_with("image/") {
        return b"Set as wallpaper";
    }
    if mime.starts_with("audio/") {
        return b"Set as ringtone";
    }
    b""
}

// -----------------------------------------------------------------------------
// The layout
//
// WHY the layout lives here and not in the renderer: the renderer and the hit
// test have to agree on where every control is, and the only way to keep them
// honest is for both of them to call the same functions. The tests at the
// bottom of this file check that they do.
// -----------------------------------------------------------------------------

/// The header band.
pub const HEADER_H: usize = 140;

// -----------------------------------------------------------------------------
// The home screen
// -----------------------------------------------------------------------------

/// The storage insight card: capacity and index counts, and nothing the
/// platform did not answer.
pub const HOME_STORAGE_CARD: Rect = Rect::new(36, 152, 648, 148);
/// The recents section's heading and caption.
pub const HOME_RECENTS_TITLE_Y: usize = 316;
pub const HOME_RECENTS_CAPTION_Y: usize = 364;
/// The recents rows. The card holds the most recent documents the index
/// reported, in the index's own modification-time order.
pub const HOME_RECENTS_Y0: usize = 392;
pub const HOME_RECENT_ROW_H: usize = 56;
/// How many recents the home screen shows.
pub const HOME_RECENTS_VISIBLE: usize = 4;
/// The category section's heading, and the four rows under it.
///
/// WHY 616 and not 608: the four recents rows start at 392 and are 56 rows
/// each, so the last one ends exactly at 616. A heading at 608 would paint
/// its first eight rows over the fourth recent.
pub const HOME_CATEGORY_TITLE_Y: usize = 616;
pub const CATEGORY_Y0: usize = 668;
/// The first category row is the storage row, which carries a caption under
/// its label, so it is taller than the three that follow it.
pub const CATEGORY_FIRST_ROW_H: usize = 100;
pub const CATEGORY_OTHER_ROW_H: usize = 70;
pub const CATEGORY_COUNT: usize = 4;
pub const CATEGORY_Y1: usize =
    CATEGORY_Y0 + CATEGORY_FIRST_ROW_H + (CATEGORY_COUNT - 1) * CATEGORY_OTHER_ROW_H;
/// The home screen's "browse everything" control.
pub const HOME_BROWSE_BTN: Rect = Rect::new(36, 996, 648, 56);
/// The honest note under it.
pub const HOME_NOTE_Y: usize = 1072;

/// The category row at a y, as `(index, top, height)`.
///
/// WHY this is a function and not a table in the renderer: the renderer and
/// the hit test have to agree on which row a touch landed on, and two copies
/// of one table is how they stop agreeing.
pub fn category_row_at(y: usize) -> Option<(usize, usize, usize)> {
    if y < CATEGORY_Y0 {
        return None;
    }
    let mut top = CATEGORY_Y0;
    for index in 0..CATEGORY_COUNT {
        let height = if index == 0 {
            CATEGORY_FIRST_ROW_H
        } else {
            CATEGORY_OTHER_ROW_H
        };
        if y < top + height {
            return Some((index, top, height));
        }
        top += height;
    }
    None
}

// -----------------------------------------------------------------------------
// The browse screen
// -----------------------------------------------------------------------------

/// The search field: the app's one text input.
pub const SEARCH_FIELD: Rect = Rect::new(36, 146, 648, 56);
/// The clear action inside the search field.
pub const SEARCH_CLEAR: Rect = Rect::new(612, 150, 48, 48);
/// The scope caption beside the two scope pills.
pub const SCOPE_Y: usize = 208;
/// The two search scopes, as pills the user can press.
pub const SCOPE_THIS_PILL: Rect = Rect::new(432, 204, 120, 48);
pub const SCOPE_ALL_PILL: Rect = Rect::new(560, 204, 124, 48);
/// The widest the scope caption may run before it meets the first pill.
pub const SCOPE_CAPTION_MAX_W: usize = 380;

/// The breadcrumb band. The crumbs are text at scale 1, so the band is one
/// 24-row line box with a touch strip around it.
pub const BREADCRUMB_Y: usize = 262;
pub const BREADCRUMB_HIT_Y0: usize = 256;
pub const BREADCRUMB_HIT_Y1: usize = 304;

/// The sort and layout toolbar.
pub const TOOLBAR_Y: usize = 300;
pub const TOOLBAR_H: usize = 48;
pub const SORT_PILL: Rect = Rect::new(36, TOOLBAR_Y, 160, TOOLBAR_H);
pub const LIST_PILL: Rect = Rect::new(512, TOOLBAR_Y, 84, TOOLBAR_H);
pub const GRID_PILL: Rect = Rect::new(600, TOOLBAR_Y, 84, TOOLBAR_H);

/// The entries card.
///
/// WHY the card is the region and not the size of the list: a long directory
/// scrolls inside the card instead of growing past the bottom of the screen.
pub const ENTRIES_CARD: Rect = Rect::new(36, 364, 648, 920);
/// The height of one entry row.
pub const ENTRY_ROW_H: usize = 56;

/// The grid's cell, its gap and the padding inside the card.
///
/// WHY three columns: three 200px cells and two 12px gaps are 624px, which
/// leaves the 12px inset on both sides of the 648px card. A fourth column
/// would be 150px, too narrow for a legible file name.
pub const GRID_COLS: usize = 3;
pub const GRID_CELL_W: usize = 200;
pub const GRID_CELL_H: usize = 150;
pub const GRID_GAP: usize = 12;
pub const GRID_PAD: usize = 12;

/// The status line under the card.
pub const STATUS_Y: usize = 1290;

/// The action bar.
pub const ACTION_BAR_Y0: usize = 1320;
pub const ACTION_BAR_Y1: usize = 1368;
/// The width of one action slot, and the gap between two slots. Seven slots
/// of 86px and six 6px gaps span 638px of the 648px bar.
pub const ACTION_SLOT_W: usize = 86;
pub const ACTION_SLOT_GAP: usize = 6;

/// The details card, drawn over the entries card.
pub const DETAILS_CARD: Rect = Rect::new(36, 364, 648, 620);
pub const DETAILS_TITLE_Y: usize = 388;
pub const DETAILS_LINE_Y0: usize = 448;
pub const DETAILS_LINE_H: usize = 40;
/// The details card's two buttons.
pub const DETAILS_OPEN: Rect = Rect::new(60, 884, 280, 48);
pub const DETAILS_CLOSE: Rect = Rect::new(380, 884, 280, 48);

/// The confirmation sheet, drawn over the list.
pub const SHEET: Rect = Rect::new(36, 900, 648, 300);
/// The destination slots in the copy and move sheet.
pub const SHEET_DEST_Y: usize = 1060;
pub const SHEET_DEST_H: usize = 48;
/// The rename and new-folder sheet's name field.
pub const SHEET_NAME_Y: usize = 1060;
pub const SHEET_NAME_H: usize = 48;
/// The sheet's confirm and cancel slots.
pub const SHEET_BUTTON_Y: usize = 1120;
pub const SHEET_BUTTON_H: usize = 48;

/// The longest a status or scope line may be.
pub const LINE_LEN: usize = 128;

/// The height of the list's content for a given number of items.
pub fn list_content_height(item_count: usize) -> usize {
    item_count * ENTRY_ROW_H
}

/// The rectangle one item's row is drawn in.
///
/// The renderer calls this, and the hit test calls [`entry_at_point`], so a
/// row that is drawn and a row that is tapped are the same row.
pub fn entry_row_rect(card: Rect, index: usize, scroll_offset: usize) -> Rect {
    let top = (card.y + index * ENTRY_ROW_H).saturating_sub(scroll_offset);
    Rect::new(card.x, top, card.w, ENTRY_ROW_H)
}

/// The item under a point, or `None` when the point is not on a row.
pub fn entry_at_point(
    card: Rect,
    scroll_offset: usize,
    item_count: usize,
    x: usize,
    y: usize,
) -> Option<usize> {
    if item_count == 0 {
        return None;
    }
    if x < card.x || x >= card.x + card.w {
        return None;
    }
    if y < card.y || y >= card.y + card.h {
        return None;
    }
    let index = (y - card.y + scroll_offset) / ENTRY_ROW_H;
    if index >= item_count {
        return None;
    }
    Some(index)
}

/// Whether a row is inside the card and therefore drawn.
pub fn row_is_visible(card: Rect, index: usize, scroll_offset: usize) -> bool {
    let rect = entry_row_rect(card, index, scroll_offset);
    rect.y < card.y + card.h && rect.y + rect.h > card.y
}

/// The scroll state for a list of `item_count` items.
///
/// WHY this is a function and not a field: the card's height and the
/// content's height have to change together, and a scroll state that was
/// built for one list and then reused for another is how a list ends up
/// scrolled past its own end.
pub fn list_scroll(item_count: usize, card: Rect, offset: usize) -> ScrollState {
    ScrollState::new(list_content_height(item_count), card.h, offset)
}

// -----------------------------------------------------------------------------
// The grid
// -----------------------------------------------------------------------------

/// The height of the grid's content for a given number of items.
pub fn grid_content_height(item_count: usize) -> usize {
    if item_count == 0 {
        return 0;
    }
    let rows = item_count.div_ceil(GRID_COLS);
    rows * GRID_CELL_H + rows.saturating_sub(1) * GRID_GAP
}

/// The rectangle one grid cell is drawn in.
pub fn grid_cell_rect(card: Rect, index: usize, scroll_offset: usize) -> Rect {
    let row = index / GRID_COLS;
    let col = index % GRID_COLS;
    let x = card.x + GRID_PAD + col * (GRID_CELL_W + GRID_GAP);
    let top = (card.y + row * (GRID_CELL_H + GRID_GAP)).saturating_sub(scroll_offset);
    Rect::new(x, top, GRID_CELL_W, GRID_CELL_H)
}

/// The grid item under a point, or `None` in the padding or a gap.
///
/// WHY this is arithmetic and not a loop: the renderer asks this for the pixel
/// it is painting, and a loop over every cell for every pixel would be a
/// million times the work. The row and column are division on the distance
/// from the card's inset, and a point in a gap falls between two cells.
pub fn grid_at_point(
    card: Rect,
    scroll_offset: usize,
    item_count: usize,
    x: usize,
    y: usize,
) -> Option<usize> {
    if item_count == 0 {
        return None;
    }
    if x < card.x + GRID_PAD || x >= card.x + card.w - GRID_PAD {
        return None;
    }
    if y < card.y || y >= card.y + card.h {
        return None;
    }
    let rel_x = x - card.x - GRID_PAD;
    let col = rel_x / (GRID_CELL_W + GRID_GAP);
    if col >= GRID_COLS || rel_x % (GRID_CELL_W + GRID_GAP) >= GRID_CELL_W {
        return None;
    }
    let rel_y = y - card.y + scroll_offset;
    let row = rel_y / (GRID_CELL_H + GRID_GAP);
    if rel_y % (GRID_CELL_H + GRID_GAP) >= GRID_CELL_H {
        return None;
    }
    let index = row * GRID_COLS + col;
    if index >= item_count {
        return None;
    }
    Some(index)
}

/// The scroll state for a grid of `item_count` items.
pub fn grid_scroll(item_count: usize, card: Rect, offset: usize) -> ScrollState {
    ScrollState::new(grid_content_height(item_count), card.h, offset)
}

/// The action bar's slot rectangle.
pub fn action_slot_rect(index: usize) -> Rect {
    let x = 36 + index * (ACTION_SLOT_W + ACTION_SLOT_GAP);
    Rect::new(x, ACTION_BAR_Y0, ACTION_SLOT_W, ACTION_BAR_Y1 - ACTION_BAR_Y0)
}

/// The confirmation sheet's destination slot.
///
/// WHY 140 and not 150: four slots of 150 with three 16px gaps span 648px
/// starting 24px inside a 648px sheet, so the last slot hung 24px past the
/// sheet's own edge. 140 and 12 put all four inside it.
pub fn sheet_destination_rect(index: usize) -> Rect {
    let w = 140usize;
    let gap = 12usize;
    let x = 60 + index * (w + gap);
    Rect::new(x, SHEET_DEST_Y, w, SHEET_DEST_H)
}

/// The confirmation sheet's confirm and cancel slots.
pub fn sheet_button_rect(confirm: bool) -> Rect {
    let x = if confirm { 380 } else { 80 };
    Rect::new(x, SHEET_BUTTON_Y, 260, SHEET_BUTTON_H)
}

// -----------------------------------------------------------------------------
// The document index, as the app holds it
// -----------------------------------------------------------------------------

/// The longest path an indexed document is held at.
pub const MAX_INDEXED_PATH_LEN: usize = 160;
/// The longest name an indexed document is held at.
pub const MAX_INDEXED_NAME_LEN: usize = 48;
/// The most search results the app holds at once.
pub const MAX_SEARCH_RESULTS: usize = 32;
/// The most documents the home screen's recents list holds.
pub const MAX_RECENTS: usize = 8;

/// One document the platform's document index holds.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct IndexedDocument {
    pub path: [u8; MAX_INDEXED_PATH_LEN],
    pub path_len: usize,
    pub name: [u8; MAX_INDEXED_NAME_LEN],
    pub name_len: usize,
    pub mime: [u8; 48],
    pub mime_len: usize,
    pub size: u64,
    /// The size, pre-rendered for the row that shows it.
    ///
    /// WHY it is pre-rendered: the row is drawn once per frame and the
    /// number is formatted once when the answer arrives, not once per frame.
    pub size_label: [u8; 20],
    pub size_label_len: usize,
    pub mtime: u64,
    pub provenance: [u8; 64],
    pub provenance_len: usize,
}

impl IndexedDocument {
    pub const EMPTY: IndexedDocument = IndexedDocument {
        path: [0; MAX_INDEXED_PATH_LEN],
        path_len: 0,
        name: [0; MAX_INDEXED_NAME_LEN],
        name_len: 0,
        mime: [0; 48],
        mime_len: 0,
        size: 0,
        size_label: [0; 20],
        size_label_len: 0,
        mtime: 0,
        provenance: [0; 64],
        provenance_len: 0,
    };

    /// The document's name as text.
    pub fn name_str(&self) -> &str {
        core::str::from_utf8(&self.name[..self.name_len]).unwrap_or("?")
    }

    /// The document's path as text.
    pub fn path_str(&self) -> &str {
        core::str::from_utf8(&self.path[..self.path_len]).unwrap_or("?")
    }
}

/// Parses the index's answer into the app's own records.
///
/// The index answers with one document per line: `path|mime|length|mtime`.
/// The app keeps the byte length and the modification time the index
/// reported, because those are the two facts about a document the index
/// actually holds.
pub fn parse_indexed_documents(buf: &[u8], out: &mut [IndexedDocument]) -> usize {
    let mut count = 0usize;
    for line in buf.split(|&b| b == b'\n') {
        if count >= out.len() {
            break;
        }
        if line.is_empty() {
            continue;
        }
        let Ok(text) = core::str::from_utf8(line) else {
            continue;
        };
        let Some(record) = interos_std::content::DocumentRecord::parse(text) else {
            continue;
        };
        let mut doc = IndexedDocument::EMPTY;
        let path_len = record.path.len().min(MAX_INDEXED_PATH_LEN);
        doc.path[..path_len].copy_from_slice(&record.path.as_bytes()[..path_len]);
        doc.path_len = path_len;
        let name = record.path.rsplit('/').next().unwrap_or(record.path);
        let name_len = name.len().min(MAX_INDEXED_NAME_LEN);
        doc.name[..name_len].copy_from_slice(&name.as_bytes()[..name_len]);
        doc.name_len = name_len;
        let mime_len = record.mime.len().min(48);
        doc.mime[..mime_len].copy_from_slice(&record.mime.as_bytes()[..mime_len]);
        doc.mime_len = mime_len;
        doc.size = record.length;
        doc.mtime = record.mtime;
        let prov_len = record.provenance.len().min(64);
        doc.provenance[..prov_len].copy_from_slice(&record.provenance.as_bytes()[..prov_len]);
        doc.provenance_len = prov_len;
        let label_len = fmt_u64(record.length, &mut doc.size_label);
        doc.size_label_len = label_len;
        out[count] = doc;
        count += 1;
    }
    count
}

/// Whether a typed search needle is worth sending to the index.
pub fn search_needle_is_usable(needle: &[u8]) -> bool {
    !needle.is_empty() && needle.len() <= 64
}

/// Puts the index's documents in recents order: newest first.
///
/// WHY the index's `mtime` and not the order the index answered in: the index
/// is a walk of the storage tree, not a history, so its answer arrives in
/// path order. The modification time is the only recency fact the platform
/// reports. A document whose time is zero (the platform never stamped it)
/// sorts after every stamped document, because a missing time is not an old
/// time.
pub fn sort_recents(docs: &mut [IndexedDocument], count: usize) -> usize {
    let count = count.min(docs.len());
    for i in 1..count {
        let mut j = i;
        while j > 0 && recents_after(&docs[j], &docs[j - 1]) {
            docs.swap(j, j - 1);
            j -= 1;
        }
    }
    count
}

/// Whether `a` belongs before `b` in recents order.
fn recents_after(a: &IndexedDocument, b: &IndexedDocument) -> bool {
    let a_known = a.mtime > 0;
    let b_known = b.mtime > 0;
    if a_known != b_known {
        return a_known;
    }
    if a_known && a.mtime != b.mtime {
        return a.mtime > b.mtime;
    }
    compare_names(a.name_str().as_bytes(), b.name_str().as_bytes()) == core::cmp::Ordering::Less
}

/// Keeps only documents inside `folder`, compacting the array.
///
/// WHY the boundary is a whole path component: `/storage/photos2` starts with
/// `/storage/photos`, and a prefix test that stopped there would answer with a
/// file from a different folder. The folder scope is the folder, not every
/// path whose text begins with its name.
pub fn filter_to_folder(docs: &mut [IndexedDocument], count: usize, folder: &str) -> usize {
    let folder = folder.trim_end_matches('/');
    let mut kept = 0usize;
    for read in 0..count.min(docs.len()) {
        if path_is_in_folder(docs[read].path_str(), folder) {
            if kept != read {
                docs[kept] = docs[read];
            }
            kept += 1;
        }
    }
    kept
}

/// Whether a document's path is inside a folder (or is the folder itself).
pub fn path_is_in_folder(path: &str, folder: &str) -> bool {
    if folder.is_empty() {
        return false;
    }
    if path == folder {
        return true;
    }
    if path.len() <= folder.len() || !path.starts_with(folder) {
        return false;
    }
    path.as_bytes()[folder.len()] == b'/'
}

/// Applies a search scope to the index's answer.
pub fn apply_search_scope(
    docs: &mut [IndexedDocument],
    count: usize,
    scope: SearchScope,
    folder: &str,
) -> usize {
    match scope {
        SearchScope::ThisFolder => filter_to_folder(docs, count, folder),
        SearchScope::AllStorage => count.min(docs.len()),
    }
}

/// Orders index records by the browse screen's sort key.
pub fn sort_documents(docs: &mut [IndexedDocument], count: usize, sort: SortMode) {
    let count = count.min(docs.len());
    for i in 1..count {
        let mut j = i;
        while j > 0 && document_after(&docs[j], &docs[j - 1], sort) {
            docs.swap(j, j - 1);
            j -= 1;
        }
    }
}

fn document_after(a: &IndexedDocument, b: &IndexedDocument, sort: SortMode) -> bool {
    let order = compare_names(a.name_str().as_bytes(), b.name_str().as_bytes());
    match sort {
        SortMode::Name => order == core::cmp::Ordering::Greater,
        SortMode::Size => {
            if a.size != b.size {
                a.size < b.size
            } else {
                order == core::cmp::Ordering::Greater
            }
        }
    }
}

// -----------------------------------------------------------------------------
// The details card
// -----------------------------------------------------------------------------

/// One line of the details card.
pub const MAX_DETAILS_LINE: usize = 96;
/// The most lines the card holds: type, size, modified, handler, path, and source provenance.
pub const MAX_DETAILS_LINES: usize = 6;

/// The facts the details card shows, already formatted.
///
/// WHY the card is built once and not per pixel: every line is a string
/// assembled from platform values, and the renderer runs once per pixel.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct DetailsView {
    pub title: [u8; 64],
    pub title_len: usize,
    pub lines: [[u8; MAX_DETAILS_LINE]; MAX_DETAILS_LINES],
    pub line_lens: [usize; MAX_DETAILS_LINES],
    pub line_count: usize,
    /// Whether the platform named a package that opens this file.
    pub has_handler: bool,
    /// Whether the path is a folder (the Open button does not apply).
    pub is_dir: bool,
}

impl DetailsView {
    pub const EMPTY: Self = Self {
        title: [0; 64],
        title_len: 0,
        lines: [[0; MAX_DETAILS_LINE]; MAX_DETAILS_LINES],
        line_lens: [0; MAX_DETAILS_LINES],
        line_count: 0,
        has_handler: false,
        is_dir: false,
    };

    pub fn title_str(&self) -> &str {
        core::str::from_utf8(&self.title[..self.title_len]).unwrap_or("")
    }
}

/// Formats a byte length the way the details card shows it.
///
/// WHY a second size format: a listing shows the exact byte count, and the
/// details card also says what that count is in units a reader can hold in
/// their head. Both are the same measured number; neither is an estimate.
pub fn format_size_human(mut size: u64, out: &mut [u8]) -> usize {
    if out.is_empty() {
        return 0;
    }
    if size < 1024 {
        let n = fmt_u64(size, out);
        return append_bytes(out, n, b" B");
    }
    let (divisor, unit): (u64, &[u8]) = if size < 1024 * 1024 {
        (1024, b" KiB")
    } else if size < 1024 * 1024 * 1024 {
        (1024 * 1024, b" MiB")
    } else {
        (1024 * 1024 * 1024, b" GiB")
    };
    let whole = size / divisor;
    size %= divisor;
    let tenth = (size * 10 + divisor / 2) / divisor;
    let tenth = if tenth == 10 { 9 } else { tenth };
    let mut n = fmt_u64(whole, out);
    if n + 2 <= out.len() {
        out[n] = b'.';
        out[n + 1] = b'0' + tenth as u8;
        n += 2;
    }
    append_bytes(out, n, unit)
}

fn append_bytes(out: &mut [u8], n: usize, bytes: &[u8]) -> usize {
    let take = bytes.len().min(out.len().saturating_sub(n));
    out[n..n + take].copy_from_slice(&bytes[..take]);
    n + take
}

/// Formats the modification time the platform reports.
///
/// WHY ticks and not a date: the platform's stat record carries a tick
/// counter, and it exposes no wall clock and no tick rate to an app. Printing
/// a calendar time from it would be an invention; the tick is the fact.
pub fn format_mtime(mtime: Option<u64>, out: &mut [u8]) -> usize {
    match mtime {
        None | Some(0) => {
            let text = b"Modified: not reported by the platform";
            let take = text.len().min(out.len());
            out[..take].copy_from_slice(&text[..take]);
            take
        }
        Some(ticks) => {
            let mut n = append_bytes(out, 0, b"Modified: ");
            n = fmt_u64(ticks, &mut out[n..]) + n;
            append_bytes(out, n, b" platform ticks")
        }
    }
}

/// Builds the details card from the facts the platform actually answered.
///
/// WHY `mtime` is an `Option` and `handler` an `Option`: a refused stat and a
/// file with no handler are different from a time of zero and from a handler
/// named "none". The card says which one it has; it never fills a gap with a
/// plausible value.
pub fn build_details(
    name: &[u8],
    path: &[u8],
    mime: &[u8],
    size: u64,
    mtime: Option<u64>,
    handler: Option<&[u8]>,
    is_dir: bool,
    handler_label: &[u8],
) -> DetailsView {
    build_details_with_provenance(name, path, mime, size, mtime, handler, is_dir, handler_label, None)
}

/// Builds the details card with explicit source URL / provenance tracking.
pub fn build_details_with_provenance(
    name: &[u8],
    path: &[u8],
    mime: &[u8],
    size: u64,
    mtime: Option<u64>,
    handler: Option<&[u8]>,
    is_dir: bool,
    handler_label: &[u8],
    provenance: Option<&[u8]>,
) -> DetailsView {
    let mut view = DetailsView::EMPTY;
    let n = name.len().min(view.title.len());
    view.title[..n].copy_from_slice(&name[..n]);
    view.title_len = n;
    view.is_dir = is_dir;
    view.has_handler = handler.is_some() && !is_dir;

    let mut line = 0usize;
    // Type.
    let mut buf = [0u8; MAX_DETAILS_LINE];
    let mut len = append_bytes(&mut buf, 0, b"Type: ");
    if is_dir {
        len = append_bytes(&mut buf, len, b"Folder");
    } else if mime.is_empty() {
        len = append_bytes(&mut buf, len, b"not reported");
    } else {
        len = append_bytes(&mut buf, len, mime);
    }
    push_line(&mut view, line, &buf, len);
    line += 1;

    // Size.
    buf = [0u8; MAX_DETAILS_LINE];
    if is_dir {
        len = append_bytes(&mut buf, 0, b"Size: not reported for a folder");
    } else {
        len = append_bytes(&mut buf, 0, b"Size: ");
        len = format_size_human(size, &mut buf[len..]) + len;
        len = append_bytes(&mut buf, len, b" (");
        len = fmt_u64(size, &mut buf[len..]) + len;
        len = append_bytes(&mut buf, len, b" bytes)");
    }
    push_line(&mut view, line, &buf, len);
    line += 1;

    // Modified.
    buf = [0u8; MAX_DETAILS_LINE];
    len = format_mtime(mtime, &mut buf);
    push_line(&mut view, line, &buf, len);
    line += 1;

    // Handler, or the platform's answer that there is none.
    buf = [0u8; MAX_DETAILS_LINE];
    if is_dir {
        len = append_bytes(&mut buf, 0, handler_label);
        len = append_bytes(&mut buf, len, b"folders open in this browser");
    } else if let Some(handler) = handler {
        len = append_bytes(&mut buf, 0, handler_label);
        len = append_bytes(&mut buf, len, handler);
    } else {
        len = append_bytes(&mut buf, 0, handler_label);
        len = append_bytes(&mut buf, len, b"no app declared this type");
    }
    push_line(&mut view, line, &buf, len);
    line += 1;

    // Path.
    buf = [0u8; MAX_DETAILS_LINE];
    len = append_bytes(&mut buf, 0, b"Path: ");
    len = append_bytes(&mut buf, len, path);
    push_line(&mut view, line, &buf, len);
    line += 1;

    // Source provenance.
    if let Some(prov) = provenance {
        buf = [0u8; MAX_DETAILS_LINE];
        len = append_bytes(&mut buf, 0, b"Source: ");
        if is_dir {
            len = append_bytes(&mut buf, len, b"local folder");
        } else if prov.is_empty() || prov == b"unknown" {
            len = append_bytes(&mut buf, len, b"unknown / not reported");
        } else if prov == b"bytes" {
            len = append_bytes(&mut buf, len, b"on-device (classified from content bytes)");
        } else if prov == b"extension" {
            len = append_bytes(&mut buf, len, b"on-device (classified from file extension)");
        } else {
            len = append_bytes(&mut buf, len, prov);
        }
        push_line(&mut view, line, &buf, len);
        line += 1;
    }

    view.line_count = line;
    view
}

fn push_line(view: &mut DetailsView, index: usize, buf: &[u8], len: usize) {
    if index >= MAX_DETAILS_LINES {
        return;
    }
    let take = len.min(MAX_DETAILS_LINE);
    view.lines[index][..take].copy_from_slice(&buf[..take]);
    view.line_lens[index] = take;
    view.line_count = index + 1;
}

// -----------------------------------------------------------------------------
// Breadcrumbs
// -----------------------------------------------------------------------------

/// One crumb: the path prefix it navigates to, and the label it draws.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Crumb {
    /// How many bytes of the path the crumb navigates to.
    pub prefix_len: usize,
    /// Where the label starts in the path.
    pub label_start: usize,
    /// How many bytes of the path the label holds.
    pub label_len: usize,
}

impl Crumb {
    pub const EMPTY: Self = Self {
        prefix_len: 0,
        label_start: 0,
        label_len: 0,
    };
}

/// Splits an absolute path into breadcrumbs.
///
/// WHY the prefix and the label are separate: the label is the component, and
/// the prefix is the path to navigate to, which is longer than the label for
/// every crumb after the first.
pub fn breadcrumb_crumbs(path: &[u8], out: &mut [Crumb]) -> usize {
    let mut count = 0usize;
    let mut end = path.len();
    while end > 0 && path[end - 1] == b'/' {
        end -= 1;
    }
    if end == 0 || path[0] != b'/' {
        return 0;
    }
    let mut component_end = end;
    while component_end > 0 {
        let start = match path[..component_end].iter().rposition(|&b| b == b'/') {
            Some(index) if index < component_end => index,
            _ => break,
        };
        let label_start = start + 1;
        if label_start < component_end {
            if count < out.len() {
                out[count] = Crumb {
                    prefix_len: component_end,
                    label_start,
                    label_len: component_end - label_start,
                };
                count += 1;
            }
            component_end = start;
        } else {
            break;
        }
    }
    // The crumbs were collected from the leaf to the root; the caller reads
    // them root first.
    out[..count].reverse();
    count
}

// -----------------------------------------------------------------------------
// Sorting a directory listing
// -----------------------------------------------------------------------------

/// Orders a directory listing: folders first, then by the chosen key.
///
/// WHY the selection is remapped by name: a selection is a set of files, not
/// a set of row numbers. Sorting the rows and leaving the marks where they
/// were would move every mark to a different file, which is the one thing a
/// sort must not do.
pub fn sort_entries(
    entries: &mut [RuntimeFileEntry],
    count: usize,
    sort: SortMode,
    selection: &mut Selection,
) {
    let count = count.min(entries.len());
    let mut marked = [false; MAX_PARSED_ENTRIES];
    let mut names = [[0u8; MAX_ENTRY_NAME_LEN]; MAX_PARSED_ENTRIES];
    let mut lens = [0usize; MAX_PARSED_ENTRIES];
    let mut dirs = [false; MAX_PARSED_ENTRIES];
    for index in 0..count {
        marked[index] = selection.is_marked(index);
        let src = &entries[index];
        let len = src.name_len.min(MAX_ENTRY_NAME_LEN);
        names[index][..len].copy_from_slice(&src.name[..len]);
        lens[index] = len;
        dirs[index] = src.is_dir;
    }
    for i in 1..count {
        let mut j = i;
        while j > 0 && entry_after(&entries[j - 1], &entries[j], sort) {
            entries.swap(j, j - 1);
            j -= 1;
        }
    }
    selection.clear();
    for new_index in 0..count {
        for old_index in 0..count {
            if lens[old_index] == entries[new_index].name_len
                && names[old_index][..lens[old_index]] == entries[new_index].name[..lens[old_index]]
                && dirs[old_index] == entries[new_index].is_dir
            {
                if marked[old_index] {
                    selection.set(new_index, true);
                }
                break;
            }
        }
    }
}

/// Whether `a` belongs after `b` under a sort key.
fn entry_after(a: &RuntimeFileEntry, b: &RuntimeFileEntry, sort: SortMode) -> bool {
    if a.is_dir != b.is_dir {
        return !a.is_dir;
    }
    let name_order = compare_names(
        &a.name[..a.name_len.min(MAX_ENTRY_NAME_LEN)],
        &b.name[..b.name_len.min(MAX_ENTRY_NAME_LEN)],
    );
    match sort {
        SortMode::Name => name_order == core::cmp::Ordering::Greater,
        SortMode::Size => {
            if a.size != b.size {
                a.size < b.size
            } else {
                name_order == core::cmp::Ordering::Greater
            }
        }
    }
}

/// Compares two names case-insensitively, then exactly, so the order is total.
pub fn compare_names(a: &[u8], b: &[u8]) -> core::cmp::Ordering {
    let fold = |byte: u8| {
        if byte.is_ascii_uppercase() {
            byte + 32
        } else {
            byte
        }
    };
    for (left, right) in a.iter().zip(b.iter()) {
        let (left, right) = (fold(*left), fold(*right));
        if left != right {
            return left.cmp(&right);
        }
    }
    let order = a.len().cmp(&b.len());
    if order != core::cmp::Ordering::Equal {
        return order;
    }
    a.cmp(b)
}

// -----------------------------------------------------------------------------
// Operation validation
// -----------------------------------------------------------------------------

/// Why an action cannot be taken on the current selection.
///
/// WHY this is a value and not a bool: a control that is offered and then does
/// nothing tells the user less than a control that says why. The caller turns
/// each of these into the line the user reads.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum OpProblem {
    /// Nothing is selected.
    NoSelection,
    /// A folder cannot be copied, moved, renamed or deleted by this platform.
    FolderNotSupported,
    /// A rename names one file.
    RenameNeedsOne,
    /// The typed name is not a name this platform can store.
    NameUnusable,
    /// The new name is the one the file already has.
    NameUnchanged,
    /// Something already holds the target name.
    NameTaken,
    /// The platform has no call that performs this operation.
    UnsupportedByPlatform,
}

/// Whether the platform exposes a directory-creation call.
///
/// WHY false: the SDK's filesystem surface is open, read, write, sync, stat,
/// list and unlink. There is no mkdir. A New Folder control therefore reports
/// NOT IMPLEMENTED instead of pretending to create a directory.
pub const NEW_FOLDER_SUPPORTED: bool = false;

/// Validates an action before the confirmation sheet is shown, and again
/// before it runs.
///
/// WHY one function for both: a sheet can sit open while the user types, and
/// the rules it was opened under must be the rules it runs under.
pub fn validate_operation(
    action: FileAction,
    target_count: usize,
    any_dir: bool,
    name: &[u8],
    current_name: &[u8],
    target_exists: bool,
) -> Result<(), OpProblem> {
    if action == FileAction::NewFolder {
        if !NEW_FOLDER_SUPPORTED {
            return Err(OpProblem::UnsupportedByPlatform);
        }
        if !name_is_usable(name) {
            return Err(OpProblem::NameUnusable);
        }
        if target_exists {
            return Err(OpProblem::NameTaken);
        }
        return Ok(());
    }
    if target_count == 0 {
        return Err(OpProblem::NoSelection);
    }
    if any_dir {
        return Err(OpProblem::FolderNotSupported);
    }
    if action == FileAction::Rename {
        if target_count != 1 {
            return Err(OpProblem::RenameNeedsOne);
        }
        if !name_is_usable(name) {
            return Err(OpProblem::NameUnusable);
        }
        if name == current_name {
            return Err(OpProblem::NameUnchanged);
        }
    }
    if target_exists {
        return Err(OpProblem::NameTaken);
    }
    Ok(())
}

/// The line the user reads for a refused action.
pub fn problem_text(problem: OpProblem) -> &'static [u8] {
    match problem {
        OpProblem::NoSelection => b"nothing is selected",
        OpProblem::FolderNotSupported => b"the platform cannot change a folder",
        OpProblem::RenameNeedsOne => b"a rename names one file",
        OpProblem::NameUnusable => b"that name cannot be stored",
        OpProblem::NameUnchanged => b"the name is unchanged",
        OpProblem::NameTaken => b"that name is already taken",
        OpProblem::UnsupportedByPlatform => {
            b"NOT IMPLEMENTED: the platform has no directory-creation call"
        }
    }
}

// -----------------------------------------------------------------------------
// The file actions
// -----------------------------------------------------------------------------

/// The actions the app can perform on the files it has selected.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum FileAction {
    Copy = 0,
    Move = 1,
    Delete = 2,
    Rename = 3,
    /// A new folder. The platform has no directory-creation call, so this
    /// action always ends in a reported NOT IMPLEMENTED outcome; see
    /// [`NEW_FOLDER_SUPPORTED`].
    NewFolder = 4,
}

/// The labels the action bar draws, indexed by [`FileAction`].
pub const ACTION_LABELS: [&[u8]; 5] = [b"Copy", b"Move", b"Delete", b"Rename", b"New"];

/// The action a slot of the action bar runs, or `None` for the slots that
/// are not file actions (Select and All).
///
/// WHY the slot numbers changed: the bar now carries seven controls -- Select,
/// All, Copy, Move, Delete, Rename and New -- so the file actions sit at
/// slots 2..=6.
pub fn action_for_slot(slot: usize) -> Option<FileAction> {
    match slot {
        2 => Some(FileAction::Copy),
        3 => Some(FileAction::Move),
        4 => Some(FileAction::Delete),
        5 => Some(FileAction::Rename),
        6 => Some(FileAction::NewFolder),
        _ => None,
    }
}

/// Whether the platform's filesystem can do this to this item.
///
/// WHY a folder is excluded: the platform can unlink a regular file and it
/// can write a regular file. It has no directory creation, so a folder is
/// not a thing any of these actions can change, and offering them on a
/// folder would be offering a control that cannot do what it says.
pub fn action_supported(action: FileAction, is_dir: bool) -> bool {
    match action {
        FileAction::Copy | FileAction::Move | FileAction::Delete | FileAction::Rename => !is_dir,
        FileAction::NewFolder => NEW_FOLDER_SUPPORTED,
    }
}

/// Whether a typed name can be a file's name on this platform.
///
/// WHY the rules: the name becomes a path component, so a name with a slash
/// in it would name something else, and the platform's paths are UTF-8 text.
pub fn name_is_usable(name: &[u8]) -> bool {
    if name.is_empty() || name.len() > MAX_ENTRY_NAME_LEN {
        return false;
    }
    if name == b"." || name == b".." {
        return false;
    }
    name.iter()
        .all(|&b| b >= 0x20 && b < 0x7f && b != b'/')
}

/// The longest path the app builds.
pub const MAX_PATH_LEN: usize = 192;

/// Joins a directory and a name into a path.
pub fn join_path(dir: &str, name: &[u8], out: &mut [u8; MAX_PATH_LEN]) -> Option<usize> {
    let mut n = 0usize;
    for &b in dir.as_bytes() {
        if n >= out.len() {
            return None;
        }
        out[n] = b;
        n += 1;
    }
    if !dir.ends_with('/') {
        if n >= out.len() {
            return None;
        }
        out[n] = b'/';
        n += 1;
    }
    for &b in name {
        if n >= out.len() {
            return None;
        }
        out[n] = b;
        n += 1;
    }
    Some(n)
}

/// The last path component of a path.
pub fn path_file_name(path: &[u8]) -> &[u8] {
    match path.iter().rposition(|&b| b == b'/') {
        Some(index) => &path[index + 1..],
        None => path,
    }
}

// -----------------------------------------------------------------------------
// The selection
// -----------------------------------------------------------------------------

/// The set of files the user has marked.
///
/// WHY a bitmap and not a list: the app has no heap, the selection has to
/// survive a redraw, and a row's place in the list is the only identity a
/// row has.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Selection {
    marked: [bool; MAX_PARSED_ENTRIES],
    count: usize,
}

impl Selection {
    pub const EMPTY: Selection = Selection {
        marked: [false; MAX_PARSED_ENTRIES],
        count: 0,
    };

    /// Whether an index is marked.
    pub fn is_marked(&self, index: usize) -> bool {
        self.marked.get(index).copied().unwrap_or(false)
    }

    /// How many items are marked.
    pub fn count(&self) -> usize {
        self.count
    }

    /// Marks or unmarks an index and reports whether it is now marked.
    pub fn toggle(&mut self, index: usize) -> bool {
        if index >= MAX_PARSED_ENTRIES {
            return false;
        }
        self.marked[index] = !self.marked[index];
        if self.marked[index] {
            self.count += 1;
        } else {
            self.count = self.count.saturating_sub(1);
        }
        self.marked[index]
    }

    /// Marks or unmarks an index without toggling it.
    ///
    /// WHY this is separate from [`Selection::toggle`]: a sort rebuilds the
    /// marked set from the names it carried, and it has to be able to say
    /// "this new row is marked" without depending on what was there before.
    pub fn set(&mut self, index: usize, marked: bool) {
        if index >= MAX_PARSED_ENTRIES || self.marked[index] == marked {
            return;
        }
        self.marked[index] = marked;
        if marked {
            self.count += 1;
        } else {
            self.count = self.count.saturating_sub(1);
        }
    }

    /// Marks every row of a listing of `count` items.
    pub fn select_all(&mut self, count: usize) {
        self.clear();
        let count = count.min(MAX_PARSED_ENTRIES);
        for index in 0..count {
            self.marked[index] = true;
        }
        self.count = count;
    }

    /// Clears every mark.
    pub fn clear(&mut self) {
        self.marked = [false; MAX_PARSED_ENTRIES];
        self.count = 0;
    }

    /// The marked indices, in order.
    pub fn indices(&self, out: &mut [usize]) -> usize {
        let mut n = 0usize;
        for (index, marked) in self.marked.iter().enumerate() {
            if *marked {
                if n >= out.len() {
                    break;
                }
                out[n] = index;
                n += 1;
            }
        }
        n
    }
}

// -----------------------------------------------------------------------------
// The typed text fields
// -----------------------------------------------------------------------------

/// The longest a typed field may be.
pub const MAX_TYPED_LEN: usize = 32;

/// One text field the user can type into.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct TypedField {
    bytes: [u8; MAX_TYPED_LEN],
    len: usize,
    focused: bool,
}

impl TypedField {
    pub const EMPTY: TypedField = TypedField {
        bytes: [0; MAX_TYPED_LEN],
        len: 0,
        focused: false,
    };

    /// The field's text.
    pub fn as_slice(&self) -> &[u8] {
        &self.bytes[..self.len]
    }

    /// How much the field holds.
    pub fn len(&self) -> usize {
        self.len
    }

    /// Whether the field is empty.
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Whether the field has the keyboard's attention.
    pub fn is_focused(&self) -> bool {
        self.focused
    }

    /// Gives the field the keyboard's attention.
    pub fn focus(&mut self) {
        self.focused = true;
    }

    /// Takes the keyboard's attention away.
    pub fn blur(&mut self) {
        self.focused = false;
    }

    /// Empties the field.
    pub fn clear(&mut self) {
        self.len = 0;
    }

    /// Puts text in the field, replacing what was there.
    pub fn set(&mut self, text: &[u8]) {
        self.len = text.len().min(MAX_TYPED_LEN);
        self.bytes[..self.len].copy_from_slice(&text[..self.len]);
    }

    /// Appends a character, if the field has room and the character is one
    /// the platform's typeface can draw.
    pub fn push(&mut self, byte: u8) -> bool {
        if self.len >= MAX_TYPED_LEN {
            return false;
        }
        if byte < 0x20 || byte > 0x7e {
            return false;
        }
        self.bytes[self.len] = byte;
        self.len += 1;
        true
    }

    /// Takes the last character off.
    pub fn pop(&mut self) {
        self.len = self.len.saturating_sub(1);
    }
}

#[cfg(test)]
extern crate std;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_selected_document_reports_its_media_type_and_its_provenance() {
        let mut document = SelectedDocument::EMPTY;
        document.mime[..9].copy_from_slice(b"image/png");
        document.mime_len = 9;
        document.provenance[..5].copy_from_slice(b"bytes");
        document.provenance_len = 5;
        assert_eq!(document.mime_str(), "image/png");
        assert_eq!(document.provenance_str(), "bytes");
        assert!(document.mime_from_bytes());
    }

    #[test]
    fn a_document_with_no_handler_says_so() {
        let document = SelectedDocument::EMPTY;
        assert!(!document.has_handler());
        assert!(document.first_handler().is_none());
    }

    #[test]
    fn a_handler_carries_the_package_that_declared_it() {
        let mut document = SelectedDocument::EMPTY;
        document.handlers[0].package_id[..19].copy_from_slice(b"org.interos.gallery");
        document.handlers[0].package_id_len = 19;
        document.handler_count = 1;
        assert!(document.has_handler());
        assert_eq!(document.first_handler(), Some("org.interos.gallery"));
    }

    #[test]
    fn test_parse_vfs_entries() {
        let raw = b"media|0|dir\nlogs|4096|dir\nconfig.json|512|file\nsystem.bin|1048576|file\n";
        let mut entries = [RuntimeFileEntry::EMPTY; MAX_PARSED_ENTRIES];
        let count = parse_vfs_entries(raw, &mut entries);
        assert_eq!(count, 4);
        assert_eq!(&entries[0].name[..entries[0].name_len], b"media");
        assert!(entries[0].is_dir);
        assert_eq!(&entries[0].size_label[..entries[0].size_label_len], b"DIR");
        assert_eq!(&entries[1].name[..entries[1].name_len], b"logs");
        assert!(entries[1].is_dir);
        assert_eq!(&entries[2].name[..entries[2].name_len], b"config.json");
        assert!(!entries[2].is_dir);
        assert_eq!(entries[2].size, 512);
        assert_eq!(&entries[2].size_label[..entries[2].size_label_len], b"512 B");
    }

    /// A download's origin sidecar is metadata, not a file the person saved.
    #[test]
    fn test_download_origin_sidecars_are_not_listed() {
        let raw = b"dl_1_photo.png|4096|file\ndl_1_photo.png.origin|52|file\ndl_2_song.wav|8192|file\n";
        let mut entries = [RuntimeFileEntry::EMPTY; MAX_PARSED_ENTRIES];
        let count = parse_vfs_entries(raw, &mut entries);
        assert_eq!(count, 2);
        assert_eq!(&entries[0].name[..entries[0].name_len], b"dl_1_photo.png");
        assert_eq!(&entries[1].name[..entries[1].name_len], b"dl_2_song.wav");
    }

    #[test]
    fn test_detect_mime() {
        assert_eq!(detect_mime(b"folder", true), FileMime::Directory);
        assert_eq!(detect_mime(b"photo.jpg", false), FileMime::Image);
        assert_eq!(detect_mime(b"screenshot.ppm", false), FileMime::Image);
        assert_eq!(detect_mime(b"clip.mp4", false), FileMime::Video);
        assert_eq!(detect_mime(b"stream.mjpg", false), FileMime::Video);
        assert_eq!(detect_mime(b"movie.mjpeg", false), FileMime::Video);
        assert_eq!(detect_mime(b"song.mp3", false), FileMime::Audio);
        assert_eq!(detect_mime(b"sound.wav", false), FileMime::Audio);
        assert_eq!(detect_mime(b"voice.pcm", false), FileMime::Audio);
        assert_eq!(detect_mime(b"notes.txt", false), FileMime::Document);
        assert_eq!(detect_mime(b"archive.tar", false), FileMime::Other);
    }

    #[test]
    fn an_unindexed_file_is_named_by_the_format_its_extension_names() {
        // WHY: the fallback describes files the index has not walked yet. It
        // has to give the same answer the index would, or a document would be
        // described one way before the walk and another way after it.
        assert_eq!(mime_for_name(b"photo.jpg", false), "image/jpeg");
        assert_eq!(mime_for_name(b"photo.jpeg", false), "image/jpeg");
        assert_eq!(mime_for_name(b"photo.png", false), "image/png");
        assert_eq!(mime_for_name(b"anim.gif", false), "image/gif");
        assert_eq!(mime_for_name(b"still.webp", false), "image/webp");
        assert_eq!(mime_for_name(b"icon.bmp", false), "image/bmp");
        assert_eq!(mime_for_name(b"frame.qoi", false), "image/qoi");
        assert_eq!(mime_for_name(b"shot.ppm", false), "image/x-portable-pixmap");
        assert_eq!(mime_for_name(b"vector.svg", false), "image/svg+xml");
        assert_eq!(mime_for_name(b"clip.mp4", false), "video/mp4");
        assert_eq!(mime_for_name(b"clip.mov", false), "video/quicktime");
        assert_eq!(mime_for_name(b"clip.webm", false), "video/webm");
        assert_eq!(mime_for_name(b"clip.mkv", false), "video/x-matroska");
        assert_eq!(mime_for_name(b"clip.avi", false), "video/x-msvideo");
        assert_eq!(mime_for_name(b"capture.mjpg", false), "video/x-motion-jpeg");
        assert_eq!(mime_for_name(b"song.mp3", false), "audio/mpeg");
        assert_eq!(mime_for_name(b"song.wav", false), "audio/wav");
        assert_eq!(mime_for_name(b"song.ogg", false), "audio/ogg");
        assert_eq!(mime_for_name(b"song.flac", false), "audio/flac");
        assert_eq!(mime_for_name(b"song.aac", false), "audio/aac");
        assert_eq!(mime_for_name(b"song.m4a", false), "audio/mp4");
        assert_eq!(mime_for_name(b"page.html", false), "text/html");
        assert_eq!(mime_for_name(b"table.csv", false), "text/csv");
        assert_eq!(mime_for_name(b"readme.md", false), "text/markdown");
        assert_eq!(mime_for_name(b"log.txt", false), "text/plain");
        assert_eq!(mime_for_name(b"payload.bin", false), "application/octet-stream");
    }

    #[test]
    fn a_frame_in_the_video_area_is_a_video_and_not_a_picture() {
        // WHY: a raw frame's bytes look like a still picture to a byte
        // sniffer, but the file is one frame of a recording, and the video
        // player is the package that can show it.
        assert_eq!(mime_for_name(b"/storage/videos/clip.ppm", true), "video/x-ppm");
        assert_eq!(mime_for_name(b"/storage/videos/clip.raw", true), "video/x-raw");
        assert_eq!(mime_for_name(b"/storage/photos/clip.ppm", false), "image/x-portable-pixmap");
        assert_eq!(mime_for_name(b"/storage/photos/clip.raw", false), "application/octet-stream");
    }

    #[test]
    fn the_action_offered_for_a_media_type_comes_from_the_media_type() {
        // WHY: what a document may become is a property of the kind of
        // document it is. The package that performs the action is the one the
        // platform's registry knows about, so the app offers the action and
        // lets the platform find its owner.
        //
        // WHY a video is not offered: the platform's decoders read still
        // images, and a wallpaper is a still picture. Offering the action for
        // a video would be a promise the platform cannot keep.
        assert_eq!(action_for_mime("image/png"), &b"Set as wallpaper"[..]);
        assert_eq!(action_for_mime("audio/mpeg"), &b"Set as ringtone"[..]);
        assert_eq!(action_for_mime("video/mp4"), &b""[..]);
        assert_eq!(action_for_mime("application/pdf"), &b""[..]);
    }

    /// The renderer draws a row with [`entry_row_rect`] and the hit test
    /// answers with [`entry_at_point`]. This holds the two together.
    #[test]
    fn the_renderer_and_the_hit_test_agree_on_the_entries_card() {
        for count in [0usize, 1, 2, 7, 8, 9, 20, 32] {
            for offset in [0usize, 56, 112, 448, 900, 1344] {
                let scroll = list_scroll(count, ENTRIES_CARD, offset);
                let off = scroll.offset;
                let mid_x = ENTRIES_CARD.x + ENTRIES_CARD.w / 2;
                for y in ENTRIES_CARD.y..ENTRIES_CARD.y + ENTRIES_CARD.h {
                    let hit = entry_at_point(ENTRIES_CARD, off, count, mid_x, y);
                    let mut drawn = None;
                    for index in 0..count {
                        if !row_is_visible(ENTRIES_CARD, index, off) {
                            continue;
                        }
                        let rect = entry_row_rect(ENTRIES_CARD, index, off);
                        if y >= rect.y && y < rect.y + rect.h {
                            drawn = Some(index);
                            break;
                        }
                    }
                    assert_eq!(
                        hit, drawn,
                        "count={} offset={} y={}: the tap target and the drawn row disagree",
                        count, off, y
                    );
                }
            }
        }
    }

    #[test]
    fn a_tap_outside_the_card_hits_no_row() {
        let scroll = list_scroll(10, ENTRIES_CARD, 0);
        let off = scroll.offset;
        // Left of the card, right of the card, above it, below it.
        assert_eq!(entry_at_point(ENTRIES_CARD, off, 10, 4, 900), None);
        assert_eq!(entry_at_point(ENTRIES_CARD, off, 10, 700, 900), None);
        assert_eq!(entry_at_point(ENTRIES_CARD, off, 10, 360, 340), None);
        assert_eq!(entry_at_point(ENTRIES_CARD, off, 10, 360, 1300), None);
        // An empty list has no rows at all.
        assert_eq!(entry_at_point(ENTRIES_CARD, off, 0, 360, 900), None);
    }

    #[test]
    fn the_card_is_the_region_and_a_long_list_scrolls_inside_it() {
        // The card is the region: it does not grow with the list.
        assert_eq!(ENTRIES_CARD.y, 364);
        assert_eq!(ENTRIES_CARD.h, 920);
        // A list that fits the card does not scroll.
        let short = list_scroll(7, ENTRIES_CARD, 0);
        assert_eq!(short.max_offset(), 0);
        // A list taller than the card has to scroll.
        let long = list_scroll(20, ENTRIES_CARD, 0);
        assert_eq!(long.max_offset(), list_content_height(20) - ENTRIES_CARD.h);
        assert!(long.max_offset() > 0);
        // The scroll never shows past the end of the list.
        let clamped = list_scroll(20, ENTRIES_CARD, 100_000);
        assert_eq!(clamped.offset, long.max_offset());
        // The last row is reachable at the end of the scroll.
        let last = 19usize;
        let rect = entry_row_rect(ENTRIES_CARD, last, clamped.offset);
        assert!(rect.y + rect.h > ENTRIES_CARD.y);
        assert!(rect.y + rect.h <= ENTRIES_CARD.y + ENTRIES_CARD.h);
        let hit = entry_at_point(
            ENTRIES_CARD,
            clamped.offset,
            20,
            ENTRIES_CARD.x + 10,
            rect.y + rect.h - 1,
        );
        assert_eq!(hit, Some(last));
    }

    #[test]
    fn every_row_of_a_long_list_is_reachable() {
        let count = MAX_PARSED_ENTRIES;
        for index in 0..count {
            // Scroll until the row is on screen, then tap it.
            let offset = (index * ENTRY_ROW_H).saturating_sub(ENTRY_ROW_H);
            let state = list_scroll(count, ENTRIES_CARD, offset);
            let rect = entry_row_rect(ENTRIES_CARD, index, state.offset);
            let y = rect.y + rect.h / 2;
            if y < ENTRIES_CARD.y || y >= ENTRIES_CARD.y + ENTRIES_CARD.h {
                continue;
            }
            let hit = entry_at_point(ENTRIES_CARD, state.offset, count, 360, y);
            assert_eq!(hit, Some(index));
        }
        assert!(list_scroll(count, ENTRIES_CARD, 0).max_offset() > 0);
    }

    #[test]
    fn a_search_answer_carries_the_size_and_the_modification_time() {
        // The index answers with `id|path|mime|provenance|length|mtime`.
        let answer = b"1|/storage/notes.txt|text/plain|bytes|2048|12345\n\
                      2|/storage/pictures/a.ppm|image/x-portable-pixmap|extension|65536|999\n";
        let mut docs = [IndexedDocument::EMPTY; 4];
        let count = parse_indexed_documents(answer, &mut docs);
        assert_eq!(count, 2);
        assert_eq!(docs[0].name_str(), "notes.txt");
        assert_eq!(docs[0].path_str(), "/storage/notes.txt");
        assert_eq!(docs[0].size, 2048);
        assert_eq!(docs[0].mtime, 12345);
        assert_eq!(docs[1].name_str(), "a.ppm");
        assert_eq!(docs[1].size, 65536);
        assert_eq!(docs[1].mtime, 999);
    }

    #[test]
    fn an_empty_search_answer_holds_no_documents() {
        let mut docs = [IndexedDocument::EMPTY; 4];
        assert_eq!(parse_indexed_documents(b"", &mut docs), 0);
        assert_eq!(parse_indexed_documents(b"\n\n", &mut docs), 0);
    }

    #[test]
    fn a_search_needs_a_needle() {
        assert!(!search_needle_is_usable(b""));
        assert!(search_needle_is_usable(b"report"));
        assert!(!search_needle_is_usable(&[b'x'; 65]));
    }

    #[test]
    fn the_platform_can_only_change_files() {
        assert!(action_supported(FileAction::Copy, false));
        assert!(action_supported(FileAction::Move, false));
        assert!(action_supported(FileAction::Delete, false));
        assert!(action_supported(FileAction::Rename, false));
        // The platform has no directory creation, so a folder is not a thing
        // any of these actions can change.
        assert!(!action_supported(FileAction::Copy, true));
        assert!(!action_supported(FileAction::Move, true));
        assert!(!action_supported(FileAction::Delete, true));
        assert!(!action_supported(FileAction::Rename, true));
    }

    #[test]
    fn a_rename_needs_a_usable_name() {
        assert!(name_is_usable(b"report.txt"));
        assert!(!name_is_usable(b""));
        assert!(!name_is_usable(b".."));
        assert!(!name_is_usable(b"a/b"));
        assert!(!name_is_usable(&[b'x'; MAX_ENTRY_NAME_LEN + 1]));
    }

    #[test]
    fn a_target_path_is_built_from_the_destination_and_the_name() {
        let mut out = [0u8; MAX_PATH_LEN];
        let len = join_path("/storage/photos", b"a.ppm", &mut out).unwrap();
        assert_eq!(&out[..len], b"/storage/photos/a.ppm");
        let len = join_path("/storage/", b"a.ppm", &mut out).unwrap();
        assert_eq!(&out[..len], b"/storage/a.ppm");
        assert_eq!(path_file_name(b"/storage/photos/a.ppm"), b"a.ppm");
    }

    #[test]
    fn a_selection_marks_clears_and_counts() {
        let mut selection = Selection::EMPTY;
        assert_eq!(selection.count(), 0);
        assert!(selection.toggle(0));
        assert!(selection.toggle(3));
        assert_eq!(selection.count(), 2);
        assert!(selection.is_marked(3));
        assert!(!selection.is_marked(1));
        let mut indices = [0usize; 8];
        assert_eq!(selection.indices(&mut indices), 2);
        assert_eq!(indices[0], 0);
        assert_eq!(indices[1], 3);
        selection.toggle(0);
        assert_eq!(selection.count(), 1);
        selection.clear();
        assert_eq!(selection.count(), 0);
        assert!(!selection.is_marked(3));
    }

    #[test]
    fn a_typed_field_takes_prints_backspaces_and_clears() {
        let mut field = TypedField::EMPTY;
        assert!(field.is_empty());
        assert!(field.push(b'r'));
        assert!(field.push(b'e'));
        assert_eq!(field.as_slice(), b"re");
        field.pop();
        assert_eq!(field.as_slice(), b"r");
        assert!(!field.push(0x01));
        field.set(b"report.txt");
        assert_eq!(field.as_slice(), b"report.txt");
        field.clear();
        assert!(field.is_empty());
    }

    #[test]
    fn touch_targets_meet_48_unit_minimum() {
        assert!(SEARCH_CLEAR.w >= 48);
        assert!(SEARCH_CLEAR.h >= 48);
        assert!(SHEET_DEST_H >= 48);
    }

    #[test]
    fn test_file_manager_ui_text_bounds_and_ellipsize() {
        use interos_std::design::font;
        let mut buf = [0u8; 64];
        let long_name = b"A_very_long_directory_name_that_exceeds_normal_breadcrumb_display_limits";
        let len = font::ellipsize(long_name, 2, 510, &mut buf);
        let disp = if len > 0 { &buf[..len] } else { long_name.as_slice() };
        assert!(font::measure(disp, 2) <= 510);
        assert!(disp.ends_with(b"...") || disp == long_name.as_slice());

        // Test category labels and action buttons fit inside their sheet bounds
        for label in [b"Storage".as_slice(), b"Photos".as_slice(), b"Pictures".as_slice(), b"Videos".as_slice()] {
            assert!(font::measure(label, 1) <= 150);
        }
        for btn in [b"Cancel".as_slice(), b"Confirm".as_slice()] {
            assert!(font::measure(btn, 1) <= 260);
        }
    }

    /// Builds one indexed document for the ordering and scope tests.
    fn indexed(path: &str, mtime: u64) -> IndexedDocument {
        let mut doc = IndexedDocument::EMPTY;
        let plen = path.len().min(MAX_INDEXED_PATH_LEN);
        doc.path[..plen].copy_from_slice(&path.as_bytes()[..plen]);
        doc.path_len = plen;
        let name = path.rsplit('/').next().unwrap_or(path);
        let nlen = name.len().min(MAX_INDEXED_NAME_LEN);
        doc.name[..nlen].copy_from_slice(&name.as_bytes()[..nlen]);
        doc.name_len = nlen;
        doc.mtime = mtime;
        doc.size = 1;
        doc
    }

    #[test]
    fn recents_are_newest_first_and_undated_documents_sort_last() {
        // WHY: recents order is the index's modification time. A document the
        // platform never stamped has no time, and a missing time must not be
        // treated as the oldest time (which would bury new files) or as the
        // newest (which would put them at the top of a list they did not earn).
        let mut docs = [
            indexed("/storage/a.txt", 10),
            indexed("/storage/b.txt", 40),
            indexed("/storage/c.txt", 0),
            indexed("/storage/d.txt", 40),
        ];
        assert_eq!(sort_recents(&mut docs, 4), 4);
        assert_eq!(docs[0].name_str(), "b.txt");
        assert_eq!(docs[1].name_str(), "d.txt");
        assert_eq!(docs[2].name_str(), "a.txt");
        assert_eq!(docs[3].name_str(), "c.txt");
        // Ties break by name, so two runs over the same index agree.
        let mut again = [
            indexed("/storage/d.txt", 40),
            indexed("/storage/b.txt", 40),
            indexed("/storage/c.txt", 0),
            indexed("/storage/a.txt", 10),
        ];
        sort_recents(&mut again, 4);
        assert_eq!(again[0].name_str(), "b.txt");
        assert_eq!(again[1].name_str(), "d.txt");
    }

    #[test]
    fn a_folder_scope_does_not_keep_a_sibling_that_shares_a_prefix() {
        // WHY: `/storage/photos2` is not inside `/storage/photos`. A string
        // prefix test would answer with a document from another folder.
        let mut docs = [
            indexed("/storage/photos/a.ppm", 3),
            indexed("/storage/photos2/b.ppm", 2),
            indexed("/storage/photos/deep/c.ppm", 1),
            indexed("/storage/videos/d.ppm", 4),
        ];
        let kept = apply_search_scope(&mut docs, 4, SearchScope::ThisFolder, "/storage/photos");
        assert_eq!(kept, 2);
        assert_eq!(docs[0].path_str(), "/storage/photos/a.ppm");
        assert_eq!(docs[1].path_str(), "/storage/photos/deep/c.ppm");
        // All storage keeps the index's whole answer.
        let all = apply_search_scope(&mut docs, 4, SearchScope::AllStorage, "/storage/photos");
        assert_eq!(all, 4);
    }

    #[test]
    fn scope_labels_and_toggles_are_their_own_opposites() {
        assert_eq!(SearchScope::ThisFolder.toggle(), SearchScope::AllStorage);
        assert_eq!(SearchScope::AllStorage.toggle(), SearchScope::ThisFolder);
        assert_eq!(SearchScope::ThisFolder.label(), b"This folder");
        assert_eq!(SearchScope::AllStorage.label(), b"All storage");
        assert_eq!(LayoutMode::List.toggle(), LayoutMode::Grid);
        assert_eq!(LayoutMode::Grid.toggle(), LayoutMode::List);
        assert_eq!(SortMode::Name.toggle(), SortMode::Size);
    }

    #[test]
    fn every_grid_cell_is_drawn_and_tapped_in_the_same_place() {
        for count in [0usize, 1, 2, 3, 4, 9, 32] {
            let scroll = grid_scroll(count, ENTRIES_CARD, 0);
            for index in 0..count {
                let rect = grid_cell_rect(ENTRIES_CARD, index, scroll.offset);
                if rect.y + rect.h <= ENTRIES_CARD.y || rect.y >= ENTRIES_CARD.y + ENTRIES_CARD.h {
                    continue;
                }
                let hit = grid_at_point(
                    ENTRIES_CARD,
                    scroll.offset,
                    count,
                    rect.x + rect.w / 2,
                    rect.y + rect.h / 2,
                );
                assert_eq!(hit, Some(index), "count={} index={}", count, index);
            }
        }
        // The padding and the gaps are not cells.
        assert_eq!(grid_at_point(ENTRIES_CARD, 0, 9, ENTRIES_CARD.x + 2, 400), None);
        assert_eq!(
            grid_at_point(ENTRIES_CARD, 0, 9, ENTRIES_CARD.x + GRID_PAD + GRID_CELL_W + 4, 400),
            None
        );
    }

    #[test]
    fn toggling_the_layout_keeps_the_selection_on_the_same_files() {
        // WHY: a layout is a view of one listing. The rows keep their order,
        // so every marked index still names the file it named in the other
        // layout; the app must not clear the marks when the view changes.
        let mut selection = Selection::EMPTY;
        selection.toggle(1);
        selection.toggle(4);
        // The same item is reachable through the grid hit test and the list
        // hit test, and it is still marked.
        let list = entry_at_point(ENTRIES_CARD, 0, 9, ENTRIES_CARD.x + 100, ENTRIES_CARD.y + 1 * ENTRY_ROW_H + 8);
        assert_eq!(list, Some(1));
        let cell = grid_cell_rect(ENTRIES_CARD, 4, 0);
        let grid = grid_at_point(ENTRIES_CARD, 0, 9, cell.x + 4, cell.y + 4);
        assert_eq!(grid, Some(4));
        assert!(selection.is_marked(1));
        assert!(selection.is_marked(4));
        assert_eq!(selection.count(), 2);
    }

    #[test]
    fn select_all_marks_every_row_it_is_given() {
        let mut selection = Selection::EMPTY;
        selection.select_all(5);
        assert_eq!(selection.count(), 5);
        assert!(selection.is_marked(4));
        assert!(!selection.is_marked(5));
        // Select-all over a smaller listing replaces the old set rather than
        // leaving marks on rows that no longer exist.
        selection.select_all(2);
        assert_eq!(selection.count(), 2);
        assert!(!selection.is_marked(4));
        let mut selection = Selection::EMPTY;
        selection.select_all(MAX_PARSED_ENTRIES + 10);
        assert_eq!(selection.count(), MAX_PARSED_ENTRIES);
    }

    fn entry(name: &[u8], size: u64, is_dir: bool) -> RuntimeFileEntry {
        let mut e = RuntimeFileEntry::EMPTY;
        let n = name.len().min(MAX_ENTRY_NAME_LEN);
        e.name[..n].copy_from_slice(&name[..n]);
        e.name_len = n;
        e.size = size;
        e.is_dir = is_dir;
        e
    }

    #[test]
    fn a_listing_is_folders_first_then_sorted_by_the_chosen_key() {
        let mut entries = [
            entry(b"zeta.txt", 10, false),
            entry(b"photos", 0, true),
            entry(b"alpha.txt", 30, false),
            entry(b"videos", 0, true),
        ];
        let mut selection = Selection::EMPTY;
        sort_entries(&mut entries, 4, SortMode::Name, &mut selection);
        assert_eq!(&entries[0].name[..entries[0].name_len], b"photos");
        assert_eq!(&entries[1].name[..entries[1].name_len], b"videos");
        assert_eq!(&entries[2].name[..entries[2].name_len], b"alpha.txt");
        assert_eq!(&entries[3].name[..entries[3].name_len], b"zeta.txt");
        sort_entries(&mut entries, 4, SortMode::Size, &mut selection);
        // Folders stay first; files go largest first.
        assert!(entries[0].is_dir && entries[1].is_dir);
        assert_eq!(&entries[2].name[..entries[2].name_len], b"alpha.txt");
        assert_eq!(&entries[3].name[..entries[3].name_len], b"zeta.txt");
    }

    #[test]
    fn sorting_a_listing_keeps_the_marks_on_their_files() {
        // WHY: the marks are a set of files. Sorting rows must move the marks
        // with the files or a sort silently retargets every action.
        let mut entries = [
            entry(b"zeta.txt", 10, false),
            entry(b"alpha.txt", 30, false),
            entry(b"middle.txt", 20, false),
        ];
        let mut selection = Selection::EMPTY;
        selection.toggle(1); // alpha.txt
        selection.toggle(2); // middle.txt
        sort_entries(&mut entries, 3, SortMode::Size, &mut selection);
        assert_eq!(&entries[0].name[..entries[0].name_len], b"alpha.txt");
        assert_eq!(&entries[2].name[..entries[2].name_len], b"zeta.txt");
        assert!(selection.is_marked(0));
        assert!(selection.is_marked(1));
        assert!(!selection.is_marked(2));
        assert_eq!(selection.count(), 2);
    }

    #[test]
    fn details_show_only_the_facts_the_platform_answered() {
        let view = build_details(
            b"a.ppm",
            b"/storage/photos/a.ppm",
            b"image/x-portable-pixmap",
            2048,
            Some(12345),
            Some(b"org.interos.gallery"),
            false,
            b"Handler: ",
        );
        assert_eq!(view.title_str(), "a.ppm");
        assert_eq!(view.line_count, 5);
        assert_eq!(&view.lines[0][..view.line_lens[0]], b"Type: image/x-portable-pixmap");
        assert_eq!(&view.lines[1][..view.line_lens[1]], b"Size: 2.0 KiB (2048 bytes)");
        assert_eq!(&view.lines[2][..view.line_lens[2]], b"Modified: 12345 platform ticks");
        assert_eq!(&view.lines[3][..view.line_lens[3]], b"Handler: org.interos.gallery");
        assert_eq!(
            &view.lines[4][..view.line_lens[4]],
            b"Path: /storage/photos/a.ppm"
        );
        assert!(view.has_handler);
        assert!(!view.is_dir);
    }

    #[test]
    fn details_do_not_invent_a_time_or_a_handler() {
        let view = build_details(
            b"b.bin",
            b"/storage/b.bin",
            b"application/octet-stream",
            512,
            None,
            None,
            false,
            b"Handler: ",
        );
        assert_eq!(&view.lines[1][..view.line_lens[1]], b"Size: 512 B (512 bytes)");
        assert_eq!(
            &view.lines[2][..view.line_lens[2]],
            b"Modified: not reported by the platform"
        );
        assert_eq!(
            &view.lines[3][..view.line_lens[3]],
            b"Handler: no app declared this type"
        );
        assert!(!view.has_handler);
        // A zero stamp is the platform saying "no time", not 1970.
        let mut buf = [0u8; MAX_DETAILS_LINE];
        let n = format_mtime(Some(0), &mut buf);
        assert_eq!(&buf[..n], b"Modified: not reported by the platform");
    }

    #[test]
    fn details_with_provenance_reports_download_source_or_unknown() {
        let view_url = build_details_with_provenance(
            b"song.flac",
            b"/storage/downloads/song.flac",
            b"audio/flac",
            1048576,
            Some(500),
            Some(b"org.interos.music"),
            false,
            b"Handler: ",
            Some(b"https://music.interos.org/dl/song.flac"),
        );
        assert_eq!(view_url.line_count, 6);
        assert_eq!(
            &view_url.lines[5][..view_url.line_lens[5]],
            b"Source: https://music.interos.org/dl/song.flac"
        );

        let view_unknown = build_details_with_provenance(
            b"song.flac",
            b"/storage/downloads/song.flac",
            b"audio/flac",
            1048576,
            Some(500),
            Some(b"org.interos.music"),
            false,
            b"Handler: ",
            Some(b"unknown"),
        );
        assert_eq!(view_unknown.line_count, 6);
        assert_eq!(
            &view_unknown.lines[5][..view_unknown.line_lens[5]],
            b"Source: unknown / not reported"
        );
    }

    #[test]
    fn a_folder_details_card_says_what_it_does_not_know() {
        let view = build_details(
            b"photos",
            b"/storage/photos",
            b"inode/directory",
            0,
            None,
            None,
            true,
            b"Handler: ",
        );
        assert!(view.is_dir);
        assert!(!view.has_handler);
        assert_eq!(&view.lines[0][..view.line_lens[0]], b"Type: Folder");
        assert_eq!(
            &view.lines[1][..view.line_lens[1]],
            b"Size: not reported for a folder"
        );
        assert_eq!(
            &view.lines[3][..view.line_lens[3]],
            b"Handler: folders open in this browser"
        );
    }

    #[test]
    fn human_sizes_are_the_same_measured_number_in_readable_units() {
        let mut buf = [0u8; 24];
        let n = format_size_human(512, &mut buf);
        assert_eq!(&buf[..n], b"512 B");
        let n = format_size_human(2048, &mut buf);
        assert_eq!(&buf[..n], b"2.0 KiB");
        let n = format_size_human(1536, &mut buf);
        assert_eq!(&buf[..n], b"1.5 KiB");
        let n = format_size_human(3 * 1024 * 1024, &mut buf);
        assert_eq!(&buf[..n], b"3.0 MiB");
        let n = format_size_human(2 * 1024 * 1024 * 1024, &mut buf);
        assert_eq!(&buf[..n], b"2.0 GiB");
    }

    #[test]
    fn the_breadcrumbs_run_from_the_mount_to_the_leaf() {
        let mut crumbs = [Crumb::EMPTY; 8];
        let count = breadcrumb_crumbs(b"/storage/photos/a.ppm", &mut crumbs);
        assert_eq!(count, 3);
        assert_eq!(&b"/storage/photos/a.ppm"[..crumbs[2].prefix_len], &b"/storage/photos/a.ppm"[..]);
        assert_eq!(&b"/storage/photos/a.ppm"[crumbs[2].label_start..crumbs[2].label_start + crumbs[2].label_len], b"a.ppm");
        assert_eq!(&b"/storage/photos/a.ppm"[..crumbs[1].prefix_len], &b"/storage/photos"[..]);
        assert_eq!(&b"/storage/photos/a.ppm"[crumbs[1].label_start..crumbs[1].label_start + crumbs[1].label_len], b"photos");
        assert_eq!(crumbs[0].prefix_len, 8);
        assert_eq!(&b"/storage/photos/a.ppm"[crumbs[0].label_start..crumbs[0].label_start + crumbs[0].label_len], b"storage");
        // A trailing slash names the same folder.
        let count = breadcrumb_crumbs(b"/storage/photos/", &mut crumbs);
        assert_eq!(count, 2);
        assert_eq!(crumbs[1].prefix_len, 15);
        // A path at the mount is one crumb.
        let count = breadcrumb_crumbs(b"/storage", &mut crumbs);
        assert_eq!(count, 1);
    }

    #[test]
    fn operations_are_validated_before_anything_is_written() {
        // A copy needs a file, not a folder, and the name must be free.
        assert_eq!(
            validate_operation(FileAction::Copy, 0, false, b"", b"", false),
            Err(OpProblem::NoSelection)
        );
        assert_eq!(
            validate_operation(FileAction::Copy, 1, true, b"", b"", false),
            Err(OpProblem::FolderNotSupported)
        );
        assert_eq!(
            validate_operation(FileAction::Copy, 1, false, b"", b"", true),
            Err(OpProblem::NameTaken)
        );
        assert_eq!(
            validate_operation(FileAction::Copy, 2, false, b"", b"", false),
            Ok(())
        );
        // A rename names one file and must actually rename it.
        assert_eq!(
            validate_operation(FileAction::Rename, 2, false, b"x", b"a", false),
            Err(OpProblem::RenameNeedsOne)
        );
        assert_eq!(
            validate_operation(FileAction::Rename, 1, false, b"", b"a", false),
            Err(OpProblem::NameUnusable)
        );
        assert_eq!(
            validate_operation(FileAction::Rename, 1, false, b"a", b"a", false),
            Err(OpProblem::NameUnchanged)
        );
        assert_eq!(
            validate_operation(FileAction::Rename, 1, false, b"b", b"a", true),
            Err(OpProblem::NameTaken)
        );
        assert_eq!(
            validate_operation(FileAction::Rename, 1, false, b"b", b"a", false),
            Ok(())
        );
    }

    #[test]
    fn a_new_folder_is_refused_because_the_platform_has_no_call_for_it() {
        // WHY: the SDK exposes open, read, write, sync, stat, list and unlink.
        // There is no mkdir, so the honest answer is NOT IMPLEMENTED, not a
        // directory that appears in a listing because the app drew one.
        assert!(!NEW_FOLDER_SUPPORTED);
        assert_eq!(
            validate_operation(FileAction::NewFolder, 0, false, b"notes", b"", false),
            Err(OpProblem::UnsupportedByPlatform)
        );
        assert_eq!(
            problem_text(OpProblem::UnsupportedByPlatform),
            b"NOT IMPLEMENTED: the platform has no directory-creation call"
        );
        assert!(!action_supported(FileAction::NewFolder, false));
    }

    #[test]
    fn the_action_slots_cover_select_all_and_the_five_file_actions() {
        assert_eq!(action_for_slot(0), None);
        assert_eq!(action_for_slot(1), None);
        assert_eq!(action_for_slot(2), Some(FileAction::Copy));
        assert_eq!(action_for_slot(6), Some(FileAction::NewFolder));
        assert_eq!(action_for_slot(7), None);
        // The seven slots fit inside the 648px bar.
        assert!(action_slot_rect(6).x + action_slot_rect(6).w <= 684);
        assert!(ACTION_LABELS[4] == b"New");
    }
}
