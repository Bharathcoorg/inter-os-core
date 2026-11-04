// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

#![no_std]
//! InterOS File Manager VFS parser and data models.

extern crate interos_std;

use interos_std::design::back_button;
use interos_std::design::font;
use interos_std::design::outline_text;
use interos_std::design::primitives::Rect;
use interos_std::design::scroll::ScrollState;
use interos_std::design::spacing;
use interos_std::design::viewport;

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

/// The later of two layout edges.
///
/// WHY a helper rather than `Ord::max`: `max` is not callable in a `const`
/// context on this toolchain, and every edge of this band has to be a
/// compile-time constant, because the pixel function compares against it once
/// per pixel.
pub const fn later(a: usize, b: usize) -> usize {
    if a > b {
        a
    } else {
        b
    }
}

// -----------------------------------------------------------------------------
// The person's text size, and the geometry that follows it
//
// WHY this block exists: every box below was authored as the platform's type
// ladder read at one hundred per cent, so a person who raised "Text size" in
// Settings got a *smaller* reading surface here -- the ink moves through the
// runtime helpers, the plate does not move at all. Settings writes the level,
// `DynamicType::publish` hands it to the renderer, and the faces under
// [`rows_at`] answer at it. A layout that moves its ink but keeps its box is the
// defect this app had; a layout that moves its box to a rung the renderer never
// got is the same defect mirrored, so the rung is read from the one cell the
// renderer draws from.
// -----------------------------------------------------------------------------

/// The text-size level every box in this file was authored against.
///
/// WHY it is named: each box is derived twice -- once as a `const fn` of a level,
/// so the compile-time gates can still sweep all four selectable levels, and once
/// at run time through the published rung, so the pixels a person gets follow the
/// setting they chose. The identity checks beside each derivation say the
/// level-aware form answers the authored number at *this* level, which is what
/// keeps the app's Normal-size pixels exactly the pixels it drew before it read
/// the setting at all.
pub const PLATFORM_LEVEL: u8 = font::TEXT_SCALE_NORMAL;

/// The highest level the ladder offers, and therefore the level the gates sweep
/// to. [`UserTextScale::from_u8`] clamps anything above it, so a sweep past this
/// rung would be a sweep over a level no renderer can be handed.
pub const MAX_LEVEL: u8 = 3;

/// The cap height a rung inks at on a level.
pub const fn cap_at(scale: usize, level: u8) -> u32 {
    font::cap_for_user_scale(scale, level)
}

/// The line box a rung reserves on a level.
pub const fn line_at(scale: usize, level: u8) -> usize {
    font::line_height_for_user_scale(scale, level)
}

/// The ascent a rung's baseline sits above its box's top on a level.
pub const fn ascent_at(scale: usize, level: u8) -> usize {
    font::ascent_for_user_scale(scale, level)
}

/// The pixels-per-em a rung is rasterised at on a level.
pub const fn em_at(scale: usize, level: u8) -> u32 {
    font::em_for_user_scale(scale, level)
}

/// The rows a run at `scale` can ink on `level`: [`outline_text::row_band`]'s
/// union of the outline line box and the bitmap cell, asked at a level rather
/// than at the ladder.
///
/// WHY a restatement and not a call: `row_band` is the platform's own answer and
/// it is a `const fn` of the ladder, so a plate sized from it can never see the
/// setting. The arithmetic below is the same walk -- the lower of the two
/// faces' baselines, the deeper of the two faces' bottoms -- with every metric
/// read through the `*_user_scale` faces, and the gate beside it refuses the day
/// the two answers disagree at the level they were authored at.
///
/// At a raised rung the union collapses, and the sweep below pins that it does:
/// the line box the ladder rasterises the outline face at *is* the box the
/// platform's own `line_height` returns for that rung (`ascent_at` and
/// `line_at` read the same `line_metrics` at the same em), so the deeper of the
/// two faces' bottoms is the outline's and the band is the line box. The bitmap
/// cell cannot out-grow it: the rung the person chose is never below the
/// authored one, `rows_at` is swept to run forwards with the level, and at the
/// authored level this equals [`outline_text::row_band`] by the gate above.
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
/// honest way to keep the authored pixels at Normal *and* carry a raised rung is
/// to add exactly the growth the box took. It never shrinks a stride, and at the
/// authored level it is the stride itself.
pub const fn grown(stride: usize, scale: usize, level: u8) -> usize {
    stride + rows_at(scale, level).saturating_sub(rows_at(scale, PLATFORM_LEVEL))
}

/// A plate that reserves a rung's box plus its authored air, never below the
/// height it was drawn with.
///
/// WHY the floor: several of this app's band heights are numbers the platform's
/// layout contract reads, so a plate may only ever grow -- at the authored level
/// this is `authored` exactly, and the air is solved from the authored pair
/// rather than invented here.
///
/// This is the one plate form, and it serves both ways this app paints: a run
/// through the surface's pixel test and a run blitted out of a widget buffer.
/// They used to be sized from two derivations, [`rows_at`] and the platform's
/// `line_height`, on the theory that the union band and the line box are
/// different numbers a plate had to pick between. They are not -- the sweep
/// beside the identity gates above proves the band equals the line box at every
/// rung this app offers and every level the ladder carries -- so the second form
/// was a second name for the same arithmetic, and a plate that disagreed with the
/// ink it bounded could have been written either way and checked neither.
pub const fn plate_at(authored: usize, scale: usize, level: u8) -> usize {
    let air = authored.saturating_sub(rows_at(scale, PLATFORM_LEVEL));
    later(authored, rows_at(scale, level) + air)
}

// -----------------------------------------------------------------------------
// The rung the painter and the hit test both ask for
//
// WHY these exist beside the `*_at` derivations: a plate that moves to the
// person's rung while the run inside it is still rasterised at the ladder's em
// is the mirror image of the defect the plates were written to fix -- the box
// grows and the ink does not. Every one of these answers the *same* rung
// [`draw_level`] hands every plate in this file, and at the authored level each
// delegates straight to the platform's ladder, so adopting the setting moves no
// pixel for a reader who never raised it.
// -----------------------------------------------------------------------------

/// The rows a run at `scale` can ink at the rung this process draws at.
#[inline]
pub fn run_rows(scale: usize) -> usize {
    rows_at(scale, draw_level())
}

/// The cap height a run at `scale` is rasterised at, at this app's rung.
#[inline]
pub fn run_cap(scale: usize) -> u32 {
    cap_at(scale, draw_level())
}

/// The line box a run at `scale` reserves at this app's rung.
#[inline]
pub fn run_line(scale: usize) -> usize {
    line_at(scale, draw_level())
}

/// The advance a run takes at `scale` on the level `lvl` names.
pub fn run_w_at(text: &[u8], scale: usize, lvl: u8) -> usize {
    if scale == 0 || text.is_empty() {
        return 0;
    }
    if lvl == PLATFORM_LEVEL {
        return font::measure(text, scale);
    }
    let w = font::measure_user_scale(text, scale, lvl);
    if w == 0 {
        font::measure(text, scale)
    } else {
        w
    }
}

/// The advance a run takes at the rung this process draws at.
#[inline]
pub fn run_w(text: &[u8], scale: usize) -> usize {
    run_w_at(text, scale, draw_level())
}

/// The advance a run the surface can actually take, in the column `available`.
///
/// WHY it restates [`outline_text::drawn_advance`] rather than calling it: that
/// function folds `scale` through the platform's em, so a run centred with it at a
/// raised rung is centred on the width the painter no longer inks. Above the
/// authored level the answer is [`run_w_at`], which measures the run at the em the
/// ladder rasterises it at and decodes it the way `typeface::text_advance_px` does
/// -- one advance per codepoint, never one per byte, because a byte walk charges a
/// multi-byte name's continuation bytes as Latin-1 glyphs the painter never inks.
/// A run wider than the column is not the painter's to draw, and the platform's
/// own branch answers that with a plain `font::measure`, so this does too.
pub fn drawn_w(text: &[u8], scale: usize, available: usize) -> usize {
    let lvl = draw_level();
    if lvl == PLATFORM_LEVEL {
        return outline_text::drawn_advance(text, scale, Some(available))
            .unwrap_or_else(|| font::measure(text, scale));
    }
    if text.is_empty() || scale == 0 {
        return 0;
    }
    let w = run_w_at(text, scale, lvl);
    if w > available {
        font::measure(text, scale)
    } else {
        w
    }
}

/// `font::ellipsize` at the rung this process draws at.
///
/// WHY a restatement and not a call: `font::ellipsize` spends its trim budget in
/// the ladder's em, so a run cut to a lane at one rung and inked at another is
/// the ellipsis-past-the-edge this repo keeps being asked to kill. At the
/// authored level this *is* `font::ellipsize`, byte for byte.
pub fn ellipsize_run(text: &[u8], scale: usize, max_w: usize, out_buf: &mut [u8]) -> usize {
    let lvl = draw_level();
    if lvl == PLATFORM_LEVEL {
        return font::ellipsize(text, scale, max_w, out_buf);
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
/// is `typeface::text_advance_px`, and that function validates the *whole* run
/// once -- decoding it and charging one advance per codepoint, or, when the
/// slice is not UTF-8 at all, charging every byte the bitmap face's own. A walk
/// that decided per suffix would charge a valid word sitting behind one
/// undecodable byte as a codepoint while the painter charged its bytes, which
/// is a run trimmed narrower than the run inked. A byte walk over valid UTF-8
/// is the same mistake sideways, and a walk that fell back to `font::measure`
/// charged the *platform* em for a glyph the raised rung inks larger, which is
/// the mismatch this helper exists to kill.
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
            (
                interos_std::design::typeface::advance_px(text[next], em) as usize,
                1,
            )
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

/// Whether a level's home stack is answered by the panel it is painted on: every
/// row a touch is answered on is a touch target, every plate that carries a label
/// is a control a finger can hit, and the note that closes the screen ends above
/// the gesture band.
const fn home_stack_fits(level: u8) -> bool {
    if home_recent_row_h_at(level) < theme::MIN_TOUCH_TARGET {
        return false;
    }
    if category_first_row_h_at(level) < theme::MIN_TOUCH_TARGET {
        return false;
    }
    if category_other_row_h_at(level) < theme::MIN_TOUCH_TARGET {
        return false;
    }
    if home_browse_btn_at(level).h < theme::MIN_TOUCH_TARGET {
        return false;
    }
    home_note_y_at(level) + rows_at(LABEL_RUNG, level) < 1440 - 40
}

/// Whether the rung this process draws at is one this app can paint.
///
/// WHY a clamp and not the published rung itself: the home screen has no scroll,
/// so its stack -- header, storage card, four recents, four category rows, button
/// and note -- has a height the 1440-row panel either answers or does not. Growing
/// every box to the person's rung is the right half of this change; the other half
/// is refusing a rung the panel cannot carry and drawing at the largest one it can,
/// rather than letting the note and the last category row walk off the bottom of
/// the screen. This is the same discipline the platform's 44-row touch floor is: a
/// bound the app cannot move, so the rung gives way rather than the ink.
///
/// WHY a downward walk: the stack's height grows with the rung and never shrinks,
/// so the largest rung that fits is found by stepping down from the one the person
/// asked for -- and at the authored level the walk never moves.
pub const fn draw_level_at(want: u8) -> u8 {
    let mut level = want;
    while level > 0 && !home_stack_fits(level) {
        level -= 1;
    }
    level
}
#[inline]
pub fn draw_level() -> u8 {
    draw_level_at(font::user_text_scale())
}

// WHY swept, and why against the clamp rather than the raw rung: the clamp only
// earns its place if every selectable level answers the panel once the clamp has
// had it, and the authored level must be untouched by it -- otherwise the app would
// resize the screen for a reader who asked for nothing.
const fn draw_level_holds_at_every_level() -> bool {
    if draw_level_at(PLATFORM_LEVEL) != PLATFORM_LEVEL {
        return false;
    }
    let mut want = 0;
    while want <= MAX_LEVEL {
        let chosen = draw_level_at(want);
        if chosen > want {
            return false;
        }
        if !home_stack_fits(chosen) {
            return false;
        }
        want += 1;
    }
    true
}
const _: () = assert!(draw_level_holds_at_every_level());

// WHY checked here: the derivations above are only a faithful restatement of the
// ladder if every rung answers the platform's own number at the platform's level.
const _: () = assert!(rows_at(theme::type_scale::CAPTION, PLATFORM_LEVEL)
    == outline_text::row_band(0, theme::type_scale::CAPTION).1);
const _: () = assert!(rows_at(theme::type_scale::BODY, PLATFORM_LEVEL)
    == outline_text::row_band(0, theme::type_scale::BODY).1);
const _: () = assert!(rows_at(theme::type_scale::SECTION, PLATFORM_LEVEL)
    == outline_text::row_band(0, theme::type_scale::SECTION).1);
const _: () = assert!(line_at(theme::type_scale::BODY, PLATFORM_LEVEL) == font::line_height(theme::type_scale::BODY));
const _: () = assert!(cap_at(theme::type_scale::SECTION, PLATFORM_LEVEL) == font::cap_for_scale(theme::type_scale::SECTION));
const _: () = assert!(ascent_at(theme::type_scale::SECTION, PLATFORM_LEVEL) == font::ascent_for_scale(theme::type_scale::SECTION));

/// WHY the union is pinned to be a collapse: `rows_at` is the walk `outline_text::
/// row_band` does, and `plate_at` is sized from it, so a plate that reserved more
/// or less than the box the painter walks is a plate that clips ink or leaves a
/// gap in it. The two faces that walk are one face at a raised rung -- `ascent_at`
/// and `line_at` both read `line_metrics` at the em the ladder rasterises at -- so
/// the band and the line box are the same number, and this is what lets the file
/// size a blitted row's plate and a pixel-tested row's plate from one derivation.
/// Written as a sweep because the claim is about the raised rungs, and an
/// assertion at the authored level alone is true of any two faces that happen to
/// agree there.
const _: () = assert!(rows_are_line_boxes_at_every_rung());
const fn rows_are_line_boxes_at_every_rung() -> bool {
    let rungs = [
        0,
        theme::type_scale::CAPTION,
        theme::type_scale::BODY,
        theme::type_scale::SECTION,
        theme::type_scale::TITLE,
    ];
    let mut rung = 0;
    while rung < rungs.len() {
        let scale = rungs[rung];
        let mut level = PLATFORM_LEVEL as usize;
        while level <= MAX_LEVEL as usize {
            if rows_at(scale, level as u8) != line_at(scale, level as u8) {
                return false;
            }
            level += 1;
        }
        rung += 1;
    }
    true
}
/// WHY a sweep and not one check: a plate that shrinks at Compact or a box that
/// runs backwards at Extra Large is invisible to an assertion written at the
/// authored level alone, and this file's gates are all written against the pair.
const _: () = assert!(rows_monotone_up_to_max_level());
const fn rows_monotone_up_to_max_level() -> bool {
    let rungs = [
        theme::type_scale::CAPTION,
        theme::type_scale::BODY,
        theme::type_scale::SECTION,
        theme::type_scale::TITLE,
    ];
    let mut rung = 0;
    while rung < rungs.len() {
        let scale = rungs[rung];
        if rows_at(scale, PLATFORM_LEVEL) != outline_text::row_band(0, scale).1 {
            return false;
        }
        let mut level = 1;
        while level <= MAX_LEVEL {
            if rows_at(scale, level) < rows_at(scale, level - 1) {
                return false;
            }
            level += 1;
        }
        let mut compact = 1;
        while compact <= MAX_LEVEL {
            if rows_at(scale, PLATFORM_LEVEL.saturating_sub(compact)) > rows_at(scale, PLATFORM_LEVEL) {
                return false;
            }
            compact += 1;
        }
        rung += 1;
    }
    true
}

/// The rung the header's title draws at: the platform's section rung, named
/// rather than written as a multiplier so a ladder move cannot silently lower
/// it here.
///
/// WHY the model owns the rung and not the painter: the header band reserves this
/// rung's whole box, so a draw site that hard-codes a different multiplier is
/// exactly how a run and its container came to disagree. The host test
/// `every_header_run_fits_the_lane_the_painter_hands_it` measures the title at
/// this number, and the painter reads it back from here.
pub const HEADER_TITLE_SCALE: usize = theme::type_scale::SECTION;

/// The rows the header's title row must reserve.
///
/// WHY the union band and not the outline box alone: the run is drawn with
/// whichever face fits it, so the space the band answers is the wider of the
/// outline line box and the bitmap cell -- [`outline_text::row_band`]'s answer
/// at the rung, which is what a container has to reserve for a run at `scale`.
pub const HEADER_TITLE_ROWS: usize = outline_text::row_band(0, HEADER_TITLE_SCALE).1;

/// The rows the title row reserves at a text-size level.
///
/// WHY it follows the rung: the title is inked at the published level, and a row
/// that kept the authored box while the ink moved to Large is ink escaping its
/// plate -- the class of defect the gates beside [`HEADER_H`] were written to
/// refuse. The authored `const` above stays as the level it was drawn at, because
/// the platform's layout contract reads it.
const fn header_title_rows_at(level: u8) -> usize {
    rows_at(HEADER_TITLE_SCALE, level)
}
#[inline]
pub fn header_title_rows() -> usize {
    header_title_rows_at(draw_level())
}

/// The row the title and the platform's back control share: the middle of the
/// control's own band.
pub const HEADER_ROW: usize = back_button::Y + back_button::SIZE / 2;

/// The row the title's whole box stands on: the box centred on [`HEADER_ROW`],
/// clamped to the first row the app's chrome may claim.
///
/// WHY the clamp: centring lands the box's top on `viewport::HEADER_TOP` exactly
/// today -- this rung's 49-row box is one row taller than the 48-row control band
/// it is centred in, and integer centring spends that row under the control. Raise
/// the rung again and the box overruns the centring's slack outright, so the clamp
/// is what keeps its top off the rows the platform's band paints over, and the
/// gate below is where the band is checked to answer the whole box that leaves.
pub const HEADER_TITLE_Y: usize = later(
    viewport::HEADER_TOP,
    HEADER_ROW.saturating_sub(HEADER_TITLE_ROWS / 2),
);

/// The row the title's whole box stands on at a level: the box centred on
/// [`HEADER_ROW`], clamped to the first row the app's chrome may claim.
///
/// WHY the same clamp applies at every level: the platform's status band paints
/// over `0..HEADER_TOP` whatever rung the person chose, so a raised box that
/// wants to climb past the inset is held at the inset and grows *downward* -- and
/// the band below answers the whole of it, which is what [`header_h_at`] is for.
const fn header_title_y_at(level: u8) -> usize {
    later(
        viewport::HEADER_TOP,
        HEADER_ROW.saturating_sub(header_title_rows_at(level) / 2),
    )
}
#[inline]
pub fn header_title_y() -> usize {
    header_title_y_at(draw_level())
}
// WHY checked here: the centring's clamp only means something while it is
// answering the authored box too.
const _: () = assert!(header_title_y_at(PLATFORM_LEVEL) == HEADER_TITLE_Y);

/// The rung the header's two readout lines draw at: the caption rung.
///
/// WHY named in the model: the painter used to pass a bare `1` at both draw
/// sites, so nothing said which rung the band was sized for and nothing could
/// measure the drawn run against the lane it is handed.
pub const HEADER_READOUT_SCALE: usize = theme::type_scale::CAPTION;

/// The rows one readout line reserves at that rung.
pub const HEADER_READOUT_ROWS: usize = outline_text::row_band(0, HEADER_READOUT_SCALE).1;

/// The rows one readout line reserves at a level.
const fn header_readout_rows_at(level: u8) -> usize {
    rows_at(HEADER_READOUT_SCALE, level)
}
#[inline]
pub fn header_readout_rows() -> usize {
    header_readout_rows_at(draw_level())
}

/// Air between the header's two readout lines.
///
/// WHY eight rows: it is the gap the pair has always kept, named now so the next
/// move changes the boxes (`HEADER_READOUT_ROWS`) and leaves this alone, rather
/// than a reader having to work out which of the two an `+ 8` meant.
pub const HEADER_READOUT_GAP: usize = spacing::GRID_8;

/// Air the header keeps below the lowest row it answers.
///
/// WHY sixteen rows: it is the same `spacing::GRID_16` this header spends
/// between the back control and its title, so the air under the band matches the
/// air inside it. The remembered 152 kept none: it was authored while the back
/// control stood on the title's foot row and ended on the control's own last row.
pub const HEADER_BOTTOM_AIR: usize = spacing::GRID_16;

/// The row the readout's first line stands on: the row the title's caps start
/// on, so the two runs share an optical top instead of a remembered offset.
///
/// WHY derived from the title's rung: row 68 was written as a bare literal with
/// no expression tying it to the title it stands under, so the day the section
/// rung moved the caption stayed where the number remembered instead of where the
/// caps now start.
pub const HEADER_READOUT_LINE1_Y: usize = HEADER_TITLE_Y
    + font::ascent_for_scale(HEADER_TITLE_SCALE)
    - font::cap_for_scale(HEADER_TITLE_SCALE) as usize;

/// The row the readout's first line stands on at a level: the row the title's
/// caps start on, so the two runs keep sharing an optical top at any rung.
///
/// WHY derived from the same level as the title above: the authored pair is
/// "the title box's top, less what its caps do not occupy". Solve it from the
/// ladder and the caption stays put while the caps move, which is the drift this
/// band was fixed for once already.
const fn header_readout_line1_y_at(level: u8) -> usize {
    header_title_y_at(level)
        + ascent_at(HEADER_TITLE_SCALE, level)
        - cap_at(HEADER_TITLE_SCALE, level) as usize
}
#[inline]
pub fn header_readout_line1_y() -> usize {
    header_readout_line1_y_at(draw_level())
}

/// The row the readout's second line stands on: one whole line box plus the gap
/// under the first, so a first line with a descender cannot paint into it.
pub const HEADER_READOUT_LINE2_Y: usize =
    HEADER_READOUT_LINE1_Y + HEADER_READOUT_ROWS + HEADER_READOUT_GAP;

/// The foot of the readout's two-line stack at a level: the first line's row,
/// two whole boxes and the gap between them.
const fn header_readout_line2_y_at(level: u8) -> usize {
    header_readout_line1_y_at(level)
        + header_readout_rows_at(level)
        + HEADER_READOUT_GAP
}
#[inline]
pub fn header_readout_line2_y() -> usize {
    header_readout_line2_y_at(draw_level())
}

/// The foot of the readout's two-line stack.
pub const HEADER_READOUT_Y1: usize = HEADER_READOUT_LINE2_Y + HEADER_READOUT_ROWS;

/// The foot of the readout's stack at a level.
const fn header_readout_y1_at(level: u8) -> usize {
    header_readout_line2_y_at(level) + header_readout_rows_at(level)
}
#[inline]
pub fn header_readout_y1() -> usize {
    header_readout_y1_at(draw_level())
}

/// The header's title lane: the widest the title run may stand.
///
/// WHY the bound lives in the model: the pixel test clips a run at its left edge
/// and at the panel's right edge only, so nothing at the draw site stops a long
/// title reaching the readout's lane. This number is the only thing that does,
/// and the host test that measures `b"Files"` at [`HEADER_TITLE_SCALE`] against
/// it is what makes the painter's claim that the title fits a measured one rather
/// than a remembered one.
pub const HEADER_TITLE_MAX_W: usize = 160;

/// The widest a header readout run may be: `font::ellipsize` is handed this at
/// both readout draw sites instead of letting a long path run under the title.
///
/// WHY the bound lives in the model: the lane is what the fit test measures
/// against, and a width authored only in the painter is a width no host test can
/// see -- which is how a header can come to measure its runs at one rung and
/// reserve room for another.
pub const HEADER_READOUT_MAX_W: usize = 400;

/// The header band: the lowest row any of its occupants occupies, plus its air.
///
/// WHY derived from the ladder and not written out: the 152 this replaces was
/// authored while the back control stood on the title's foot row, so the band
/// ended on the control's own last row -- 44 rows of title box plus a 44px
/// control, and no air of its own. The control has since moved into the
/// platform's chip band and the ladder has grown both rungs, so the figure that
/// decides this band is now the readout's two-line stack. Every row the band
/// answers comes off the rungs and the control, so raising one grows this band
/// and the browse stack that starts on it (`BROWSE_STACK_Y`) instead of clipping
/// the run that moved.
pub const HEADER_H: usize = later(
    later(
        HEADER_READOUT_Y1,
        HEADER_TITLE_Y + HEADER_TITLE_ROWS,
    ),
    back_button::Y + back_button::SIZE,
) + HEADER_BOTTOM_AIR;

/// The header band at a level: the lowest row any of its occupants occupies at
/// that level, plus its air.
///
/// WHY a function of the rung and not the authored figure: every row of this
/// app's two screens starts on this band's foot, and a band that stayed at 152
/// while the readout's stack grew to the person's rung would paint the surplus
/// over the first thing below it -- this platform paints no clip. At the authored
/// level the two are the same number, so nothing moves for a reader who never
/// raised the setting.
const fn header_h_at(level: u8) -> usize {
    later(
        later(
            header_readout_y1_at(level),
            header_title_y_at(level) + header_title_rows_at(level),
        ),
        back_button::Y + back_button::SIZE,
    ) + HEADER_BOTTOM_AIR
}
#[inline]
pub fn header_h() -> usize {
    header_h_at(draw_level())
}
// WHY checked here: the band's number is the layout the platform's contract
// reads, and the derivation must answer it at the level it was read at.
const _: () = assert!(header_h_at(PLATFORM_LEVEL) == HEADER_H);

/// The last row the header's title ink takes at a level: its box's top plus the
/// union box the painter's own band test uses.
const fn header_title_ink_y1_at(level: u8) -> usize {
    header_title_y_at(level) + header_title_rows_at(level)
}

/// Truth gate, swept over every selectable level: the band answers the whole box
/// of each of its own runs, keeps its air inside itself, and never shrinks below
/// the band it was drawn with.
///
/// WHY a sweep rather than the four single-level asserts above it: those say the
/// authored pixels are unchanged. The only claim that matters for a person on
/// Large or Extra Large is that the *same* containment holds at the rung they
/// chose, and no number written at build time for one rung can say that.
const fn header_holds_at_every_level() -> bool {
    let mut level = 0;
    while level <= MAX_LEVEL {
        if header_title_ink_y1_at(level) > header_h_at(level) {
            return false;
        }
        if header_readout_y1_at(level) > header_h_at(level) {
            return false;
        }
        if back_button::Y + back_button::SIZE > header_h_at(level) {
            return false;
        }
        if header_title_y_at(level) < viewport::HEADER_TOP {
            return false;
        }
        if header_readout_line1_y_at(level) < header_title_y_at(level) {
            return false;
        }
        level += 1;
    }
    true
}
const _: () = assert!(header_holds_at_every_level());

/// Truth gates: the band answers every row each of its own runs occupies, no run
/// starts inside the rows the platform's status band owns, and the band's air is
/// paid for out of the band itself rather than borrowed from the surface under
/// it. These are the checks that fire if the band and the runs inside it ever
/// disagree -- e.g. a title rung raised past the point where the readout stack is
/// still the header's lowest occupant.
const _: () = assert!(
    HEADER_TITLE_Y >= viewport::HEADER_TOP,
    "the header's title box must start below the rows the platform's status band owns"
);
const _: () = assert!(
    HEADER_READOUT_LINE1_Y >= viewport::HEADER_TOP,
    "the header's readout must start below the rows the platform's status band owns"
);
const _: () = assert!(
    HEADER_TITLE_Y + HEADER_TITLE_ROWS <= HEADER_H,
    "the header must answer the whole box its title run reserves"
);
const _: () = assert!(
    back_button::Y + back_button::SIZE <= HEADER_H,
    "the back control must stand entirely inside the rows the header answers"
);
const _: () = assert!(
    HEADER_READOUT_Y1 <= HEADER_H,
    "the header must answer the whole stack of its readout lines"
);
const _: () = assert!(
    HEADER_READOUT_LINE1_Y >= HEADER_TITLE_Y
        && HEADER_READOUT_LINE1_Y <= HEADER_TITLE_Y + HEADER_TITLE_ROWS,
    "the readout's first line must start inside the title row it stands beside"
);
/// WHY the air is checked against the row the next band claims rather than
/// against `HEADER_H`: `HEADER_H` *is* `BROWSE_STACK_Y`, so a band that ate its
/// own air would still pass a test written against itself. Checked against the
/// surface below, the last inked row of either occupant plus the band's air has
/// to land on or before the row the search field starts on -- which is the whole
/// claim that the header crowds nothing.
const _: () = assert!(
    later(
        HEADER_TITLE_Y + HEADER_TITLE_ROWS,
        HEADER_READOUT_Y1,
    ) + HEADER_BOTTOM_AIR
        <= BROWSE_STACK_Y,
    "the header's ink and its air must clear the first row the browse stack claims"
);
/// WHY also against the home screen's first card: the header band is the foot of
/// both screens, and a card authored from a remembered row loses its top rows to
/// the header the moment a rung grows the band above it.
const _: () = assert!(
    later(
        HEADER_TITLE_Y + HEADER_TITLE_ROWS,
        HEADER_READOUT_Y1,
    ) + HEADER_BOTTOM_AIR
        <= HOME_STORAGE_CARD.y,
    "the header's ink and its air must clear the home screen's first card"
);

// -----------------------------------------------------------------------------
// The home screen
// -----------------------------------------------------------------------------

/// The storage insight card: capacity and index counts, and nothing the
/// platform did not answer.
///
/// WHY it starts on the header's foot row: the card is the first thing the home
/// screen draws, and a card authored from a remembered row loses its top rows to
/// the header the moment a rung grows the band above it.
pub const HOME_STORAGE_CARD: Rect = Rect::new(36, HEADER_H, 648, 148);

/// The card's three runs, as the band it was drawn with placed them: its title
/// row, the two strides under it and the air below the last box.
///
/// WHY solved from the authored card rather than written as new numbers: the
/// card's height is a figure the home screen's own layout contract reads, and the
/// only way to grow it for a raised rung without moving a pixel at Normal is to
/// re-state the distances the painter already used and add each run's own growth.
const HOME_CARD_TITLE_DY: usize = 20;
const HOME_CARD_LINE1_STRIDE: usize = 56;
const HOME_CARD_LINE2_STRIDE: usize = 28;
const HOME_CARD_FOOT_AIR: usize = 148
    - (HOME_CARD_TITLE_DY
        + HOME_CARD_LINE1_STRIDE
        + HOME_CARD_LINE2_STRIDE
        + outline_text::row_band(0, theme::type_scale::CAPTION).1);

/// The row the card's first line stands on at a level, below the card's top.
pub const fn home_card_line1_dy_at(level: u8) -> usize {
    HOME_CARD_TITLE_DY + grown(HOME_CARD_LINE1_STRIDE, theme::type_scale::BODY, level)
}
/// The row the card's status line stands on at a level, below the card's top.
pub const fn home_card_line2_dy_at(level: u8) -> usize {
    home_card_line1_dy_at(level)
        + grown(HOME_CARD_LINE2_STRIDE, theme::type_scale::CAPTION, level)
}
/// The card's height at a level: its three boxes and the air it kept under the
/// last of them.
pub const fn home_storage_card_h_at(level: u8) -> usize {
    home_card_line2_dy_at(level)
        + rows_at(theme::type_scale::CAPTION, level)
        + HOME_CARD_FOOT_AIR
}
/// The card at a level: it starts on the header's foot row and answers its own
/// stack, so a raised rung moves it down and grows it -- it never loses its top
/// rows to the band above it.
pub const fn home_storage_card_at(level: u8) -> Rect {
    Rect::new(
        HOME_STORAGE_CARD.x,
        header_h_at(level),
        HOME_STORAGE_CARD.w,
        home_storage_card_h_at(level),
    )
}
#[inline]
pub fn home_storage_card() -> Rect {
    home_storage_card_at(draw_level())
}
/// The row the storage card's title stands on, at this app's rung.
#[inline]
pub fn home_card_title_y() -> usize {
    home_storage_card().y + HOME_CARD_TITLE_DY
}
/// The row the storage card's first line stands on, at this app's rung.
#[inline]
pub fn home_card_line1_y() -> usize {
    home_storage_card().y + home_card_line1_dy_at(draw_level())
}
/// The row the storage card's status line stands on, at this app's rung.
#[inline]
pub fn home_card_line2_y() -> usize {
    home_storage_card().y + home_card_line2_dy_at(draw_level())
}
/// The rung the storage card's title is inked at: the Body rung its box reserves.
pub const HOME_CARD_TITLE_SCALE: usize = theme::type_scale::BODY;
/// The rung the storage card's two lines are inked at: the caption rung the card's
/// foot reserves below them.
pub const HOME_CARD_LINE_SCALE: usize = theme::type_scale::CAPTION;
const _: () = assert!(
    home_storage_card_at(PLATFORM_LEVEL).y == HOME_STORAGE_CARD.y
        && home_storage_card_at(PLATFORM_LEVEL).h == HOME_STORAGE_CARD.h
);

/// The rung the recents section's heading and caption draw at.
const HOME_RECENTS_TITLE_SCALE: usize = theme::type_scale::BODY;
const HOME_RECENTS_CAPTION_SCALE: usize = theme::type_scale::CAPTION;
/// The strides the home screen authored between the card, the heading, the
/// caption and the first row.
const HOME_RECENTS_TITLE_STRIDE: usize = 16;
const HOME_RECENTS_CAPTION_STRIDE: usize = 48;
const HOME_RECENTS_FIRST_ROW_STRIDE: usize = 28;

/// The recents section's heading row at a level.
pub const fn home_recents_title_y_at(level: u8) -> usize {
    let card = home_storage_card_at(level);
    card.y + card.h + HOME_RECENTS_TITLE_STRIDE
}
/// The recents caption row at a level.
pub const fn home_recents_caption_y_at(level: u8) -> usize {
    home_recents_title_y_at(level) + grown(HOME_RECENTS_CAPTION_STRIDE, HOME_RECENTS_TITLE_SCALE, level)
}
/// The first recents row's top at a level.
pub const fn home_recents_y0_at(level: u8) -> usize {
    home_recents_caption_y_at(level)
        + grown(HOME_RECENTS_FIRST_ROW_STRIDE, HOME_RECENTS_CAPTION_SCALE, level)
}
#[inline]
pub fn home_recents_title_y() -> usize {
    home_recents_title_y_at(draw_level())
}
#[inline]
pub fn home_recents_caption_y() -> usize {
    home_recents_caption_y_at(draw_level())
}
#[inline]
pub fn home_recents_y0() -> usize {
    home_recents_y0_at(draw_level())
}
/// The recents section's heading and caption.
pub const HOME_RECENTS_TITLE_Y: usize = 316;
pub const HOME_RECENTS_CAPTION_Y: usize = 364;
/// The recents rows. The card holds the most recent documents the index
/// reported, in the index's own modification-time order.
pub const HOME_RECENTS_Y0: usize = 392;

// WHY checked here: the three rows above are the authored layout the platform's
// contract reads, and the level-aware forms beside them are only an honest
// restatement while they answer those same numbers at the authored level.
const _: () = assert!(home_recents_title_y_at(PLATFORM_LEVEL) == HOME_RECENTS_TITLE_Y);
const _: () = assert!(home_recents_caption_y_at(PLATFORM_LEVEL) == HOME_RECENTS_CAPTION_Y);
const _: () = assert!(home_recents_y0_at(PLATFORM_LEVEL) == HOME_RECENTS_Y0);
/// The rung a recent's name draws at: the Body rung, so a file's name reads as
/// the row's title and its "Modified:" line reads as the caption under it.
///
/// WHY it was a caption: both lines drew at rung 1, which put a 13px cap under
/// a 13px cap and made four rows of the list read as one grey texture.
pub const HOME_RECENT_NAME_SCALE: usize = 2;
/// The air between a row's top edge and the box its name draws in.
pub const HOME_RECENT_NAME_DY: usize = 4;
/// The "Modified:" line's box top: one whole name box under the name's, so a
/// name with a descender cannot paint into it.
pub const HOME_RECENT_TIME_DY: usize =
    HOME_RECENT_NAME_DY + interos_std::font::line_height(HOME_RECENT_NAME_SCALE);

/// A recents row's two boxes at a level: the name's line box, then the caption
/// under it, then the divider's row.
///
/// WHY a function of the rung: the row stacks a body-rung name over a caption,
/// and both are inked at the published level. A row authored from the ladder's
/// two boxes is the luck the comment beside [`HOME_RECENT_ROW_H`] already
/// describes, one rung further down.
const HOME_RECENT_DIVIDER_AIR: usize = 6;
pub const fn home_recent_time_dy_at(level: u8) -> usize {
    HOME_RECENT_NAME_DY + line_at(HOME_RECENT_NAME_SCALE, level)
}
pub const fn home_recent_row_h_at(level: u8) -> usize {
    home_recent_time_dy_at(level)
        + line_at(HOME_RECENT_CAPTION_SCALE, level)
        + HOME_RECENT_DIVIDER_AIR
}
/// The rung a recent's "Modified:" line draws at.
const HOME_RECENT_CAPTION_SCALE: usize = theme::type_scale::CAPTION;
#[inline]
pub fn home_recent_time_dy() -> usize {
    home_recent_time_dy_at(draw_level())
}
#[inline]
pub fn home_recent_row_h() -> usize {
    home_recent_row_h_at(draw_level())
}
const _: () = assert!(home_recent_time_dy_at(PLATFORM_LEVEL) == HOME_RECENT_TIME_DY);
const _: () = assert!(home_recent_row_h_at(PLATFORM_LEVEL) == HOME_RECENT_ROW_H);
/// A recents row: the two boxes it draws plus the air its divider needs below
/// them.
///
/// WHY derived: the row was a remembered 56 rows, which is what the two caption
/// boxes were when this list was written. The name moved to the Body rung and
/// the ladder's boxes grew with it, so the only figure that stays true is the
/// one that adds the boxes the row actually paints.
pub const HOME_RECENT_ROW_H: usize =
    HOME_RECENT_TIME_DY + interos_std::font::line_height(1) + 6;
/// How many recents the home screen shows.
pub const HOME_RECENTS_VISIBLE: usize = 4;
/// Where the recents block ends, and so where the section under it begins.
pub const HOME_RECENTS_Y1: usize = HOME_RECENTS_Y0 + HOME_RECENTS_VISIBLE * HOME_RECENT_ROW_H;

/// The recents block's foot at a level, and the category section's heading,
/// first row and last row with it.
///
/// WHY the whole chain is a function of the rung: the heading is painted at the
/// row the block above it ends on, so a block that grew without the heading
/// moving would have the last recent's caption painted through the section title
/// that answers taps below it. Deriving the chain from the row height is what lets
/// a raised rung push the sections under it down instead of running through them.
pub const fn home_recents_y1_at(level: u8) -> usize {
    home_recents_y0_at(level) + HOME_RECENTS_VISIBLE * home_recent_row_h_at(level)
}

/// The air the recents block keeps above the notice it paints when the index gave
/// it nothing to list.
///
/// WHY derived and not added at the branch that inks it: the block's own top came
/// from [`home_recents_y0_at`], which moves with the rung, while the `8` the painter
/// added to it did not. The notice replaces rows that each grew their own box, so a
/// line left at the authored air floats to the top of a block the person just made
/// taller -- the same half-moved layout the plates above it were written to fix.
const HOME_RECENTS_NOTE_DY: usize = 8;
pub const fn home_recents_note_dy_at(level: u8) -> usize {
    grown(HOME_RECENTS_NOTE_DY, LABEL_RUNG, level)
}
#[inline]
pub fn home_recents_note_dy() -> usize {
    home_recents_note_dy_at(draw_level())
}
const _: () = assert!(home_recents_note_dy_at(PLATFORM_LEVEL) == HOME_RECENTS_NOTE_DY);
pub const fn category_title_y_at(level: u8) -> usize {
    home_recents_y1_at(level)
}
/// The air the home screen authored between a section heading and its first row.
const CATEGORY_HEAD_STRIDE: usize = 52;
pub const fn category_y0_at(level: u8) -> usize {
    category_title_y_at(level) + grown(CATEGORY_HEAD_STRIDE, CATEGORY_LABEL_SCALE, level)
}
#[inline]
pub fn home_recents_y1() -> usize {
    home_recents_y1_at(draw_level())
}
#[inline]
pub fn category_y0() -> usize {
    category_y0_at(draw_level())
}
const _: () = assert!(home_recents_y1_at(PLATFORM_LEVEL) == HOME_RECENTS_Y1);
const _: () = assert!(category_y0_at(PLATFORM_LEVEL) == CATEGORY_Y0);
/// The category section's heading, and the four rows under it.
///
/// WHY it starts where the recents block ends: a heading that began inside the
/// last recent's box would paint over the rows that box reserves, and the row
/// that lost them still answers taps there.
pub const HOME_CATEGORY_TITLE_Y: usize = HOME_RECENTS_Y1;
pub const CATEGORY_Y0: usize = HOME_CATEGORY_TITLE_Y + 52;
/// The first category row is the storage row, which carries a caption under
/// its label, so it is taller than the three that follow it.
///
/// WHY this is the plate the ink needs and not the height first drawn: the row
/// authored 100 rows while its own caption stands on row 72 and its box reaches
/// 30 rows past that, so the second line of the storage sentence inked two rows
/// onto the divider and into the next card -- at every rung, because the plate
/// and the caption grew by the same boxes and only the air between them was
/// authored. 111 is that plate with its own air restored: the label's 22 rows on
/// top, the caption's box, and the nine rows the 100 was meant to leave below.
pub const CATEGORY_FIRST_ROW_H: usize = 111;
pub const CATEGORY_OTHER_ROW_H: usize = 70;

/// A category row's plate at a level: the boxes its own runs ink, plus the air
/// the authored rows left around them, never below the row it was drawn with.
///
/// WHY derived the same way for both: the first row carries a caption under its
/// label and the other three carry only the label, and both authored heights leave
/// the same thirty-one rows of air. Solving the air from the authored pair keeps
/// that proportion at every rung instead of pinning the plate to Normal.
const CATEGORY_ROW_AIR: usize = 31;
pub const fn category_first_row_h_at(level: u8) -> usize {
    // The caption's own foot, plus the air the row was authored to leave under
    // its last box. Solving the plate from where the ink ends is the only form
    // that can hold it: the authored sum of the two boxes and 31 rows of air was
    // two rows short of the caption the same row paints, at every rung.
    let content = category_caption_dy_at(level)
        + line_at(CATEGORY_CAPTION_SCALE, level)
        + (CATEGORY_ROW_AIR - CATEGORY_FIRST_LABEL_DY);
    later(CATEGORY_FIRST_ROW_H, content)
}
pub const fn category_other_row_h_at(level: u8) -> usize {
    let content = line_at(CATEGORY_LABEL_SCALE, level) + CATEGORY_ROW_AIR;
    later(CATEGORY_OTHER_ROW_H, content)
}
#[inline]
pub fn category_first_row_h() -> usize {
    category_first_row_h_at(draw_level())
}
#[inline]
pub fn category_other_row_h() -> usize {
    category_other_row_h_at(draw_level())
}
pub const CATEGORY_COUNT: usize = 4;
pub const CATEGORY_Y1: usize =
    CATEGORY_Y0 + CATEGORY_FIRST_ROW_H + (CATEGORY_COUNT - 1) * CATEGORY_OTHER_ROW_H;

/// The category block's foot at a level, its "browse everything" control and the
/// honest note under it.
pub const fn category_y1_at(level: u8) -> usize {
    category_y0_at(level)
        + category_first_row_h_at(level)
        + (CATEGORY_COUNT - 1) * category_other_row_h_at(level)
}
/// The gap the home screen authored between the last category row and its button,
/// and between the button and the note that closes the screen.
const CATEGORY_BUTTON_STRIDE: usize = 18;
const CATEGORY_NOTE_STRIDE: usize = 20;
pub const fn home_browse_btn_at(level: u8) -> Rect {
    Rect::new(
        HOME_BROWSE_BTN.x,
        category_y1_at(level) + CATEGORY_BUTTON_STRIDE,
        HOME_BROWSE_BTN.w,
        plate_at(HOME_BROWSE_BTN.h, LABEL_RUNG, level),
    )
}
pub const fn home_note_y_at(level: u8) -> usize {
    let btn = home_browse_btn_at(level);
    btn.y + btn.h + CATEGORY_NOTE_STRIDE
}
#[inline]
pub fn category_y1() -> usize {
    category_y1_at(draw_level())
}
/// The category section's heading row, at this app's rung.
#[inline]
pub fn home_category_title_y() -> usize {
    category_title_y_at(draw_level())
}
/// The air a category row keeps above its label: the row's own top air, which is
/// air and not a box, so it does not move with the rung.
pub const CATEGORY_FIRST_LABEL_DY: usize = 22;
/// The air the three rows without a caption keep above their label.
pub const CATEGORY_OTHER_LABEL_DY: usize = 11;
/// The gap the first row authored between its label's box and its caption's box:
/// `72 - 22 - line(BODY)`, solved from the pair rather than written anew.
const CATEGORY_CAPTION_GAP: usize =
    72 - CATEGORY_FIRST_LABEL_DY - interos_std::font::line_height(theme::type_scale::BODY);
/// The row the first category row's caption stands on, at this app's rung: one
/// whole label box under the label, so a label with a descender cannot ink into it.
#[inline]
pub fn category_caption_dy() -> usize {
    category_caption_dy_at(draw_level())
}
/// The same lane solved from the ladder rather than from the rung this process
/// holds, so the plate that has to carry the caption can be checked against it at
/// every rung.
pub const fn category_caption_dy_at(level: u8) -> usize {
    CATEGORY_FIRST_LABEL_DY + line_at(CATEGORY_LABEL_SCALE, level) + CATEGORY_CAPTION_GAP
}
#[inline]
pub fn home_browse_btn() -> Rect {
    home_browse_btn_at(draw_level())
}
#[inline]
pub fn home_note_y() -> usize {
    home_note_y_at(draw_level())
}
const _: () = assert!(category_y1_at(PLATFORM_LEVEL) == CATEGORY_Y1);
const _: () = assert!(
    home_browse_btn_at(PLATFORM_LEVEL).y == HOME_BROWSE_BTN.y
        && home_browse_btn_at(PLATFORM_LEVEL).h == HOME_BROWSE_BTN.h
);
const _: () = assert!(home_note_y_at(PLATFORM_LEVEL) == HOME_NOTE_Y);
/// The home screen's "browse everything" control: the first thing below the
/// last category row, with the gap it has always had.
pub const HOME_BROWSE_BTN: Rect = Rect::new(36, CATEGORY_Y1 + 18, 648, 56);
/// The honest note under it.
pub const HOME_NOTE_Y: usize = HOME_BROWSE_BTN.y + HOME_BROWSE_BTN.h + 20;

/// Truth gates: a recents row holds the two boxes it paints, every touch band
/// on the home screen stays a target, and the browse control stays pressable.
///
/// WHY swept over every level: the first claim is about a stack of ink, and at
/// the authored level it is true by 1 row -- a rung the person can choose eats
/// that margin outright, so the gate has to be asked at the largest rung it,
/// not the one the layout was drawn at.
///
/// WHY the panel fit is NOT claimed here: the home screen has no scroll, so a
/// raw sweep cannot promise the note clears the gesture bar at every level.
/// That promise is made where it is kept -- `draw_level_holds_at_every_level`
/// asserts it of the level the screen actually draws at, which is the wanted
/// level clamped down until the stack fits.
const fn home_stack_holds_at_every_level() -> bool {
    let mut level = 0;
    while level <= MAX_LEVEL {
        let row = home_recent_row_h_at(level);
        if home_recent_time_dy_at(level) + line_at(HOME_RECENT_CAPTION_SCALE, level) + 1 > row {
            return false;
        }
        // The category plate is named for the rows it carries, so the caption is
        // checked against it here: the storage row's second line ends inside the
        // box that row answers taps on, and not on the divider that closes it or
        // in the card of the row beneath.
        if category_caption_dy_at(level) + line_at(CATEGORY_CAPTION_SCALE, level)
            > category_first_row_h_at(level)
        {
            return false;
        }
        if row < theme::MIN_TOUCH_TARGET {
            return false;
        }
        if category_first_row_h_at(level) < theme::MIN_TOUCH_TARGET {
            return false;
        }
        if category_other_row_h_at(level) < theme::MIN_TOUCH_TARGET {
            return false;
        }
        if home_browse_btn_at(level).h < theme::MIN_TOUCH_TARGET {
            return false;
        }
        level += 1;
    }
    true
}
const _: () = assert!(home_stack_holds_at_every_level());

/// The category row at a y, as `(index, top, height)`.
///
/// WHY this is a function and not a table in the renderer: the renderer and
/// the hit test have to agree on which row a touch landed on, and two copies
/// of one table is how they stop agreeing.
pub fn category_row_at(y: usize) -> Option<(usize, usize, usize)> {
    let lvl = draw_level();
    let first = category_first_row_h_at(lvl);
    let other = category_other_row_h_at(lvl);
    let stack_top = category_y0_at(lvl);
    if y < stack_top {
        return None;
    }
    let mut top = stack_top;
    for index in 0..CATEGORY_COUNT {
        let height = if index == 0 { first } else { other };
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

/// The row the browse screen's stack starts on: the first row the header no
/// longer answers.
///
/// WHY derived from the header and not written out: the header grew to carry the
/// back control on the row under the title's line box, and every row of the
/// stack below it kept the gap it already had from the search field. A stack
/// starting inside the rows the header answers loses its top rows to a surface
/// that never asks the stack what it is drawing.
pub const BROWSE_STACK_Y: usize = HEADER_H;

/// The row the browse stack starts on at a level: the header's foot row at that
/// level, which is the same claim the authored `const` makes at Normal.
pub const fn browse_stack_y_at(level: u8) -> usize {
    header_h_at(level)
}
#[inline]
pub fn browse_stack_y() -> usize {
    browse_stack_y_at(draw_level())
}
const _: () = assert!(browse_stack_y_at(PLATFORM_LEVEL) == BROWSE_STACK_Y);

/// The search field: the app's one text input.
pub const SEARCH_FIELD: Rect = Rect::new(36, BROWSE_STACK_Y, 648, 56);
/// The clear action inside the search field.
pub const SEARCH_CLEAR: Rect = Rect::new(612, BROWSE_STACK_Y + 4, 48, 48);
/// The scope caption beside the two scope pills.
pub const SCOPE_Y: usize = BROWSE_STACK_Y + 62;
/// The two search scopes, as pills the user can press.
pub const SCOPE_THIS_PILL: Rect = Rect::new(432, BROWSE_STACK_Y + 58, 120, 48);
pub const SCOPE_ALL_PILL: Rect = Rect::new(560, BROWSE_STACK_Y + 58, 124, 48);

/// The field, its clear control, the caption beside the pills and the two pills
/// themselves, all placed at a level.
///
/// WHY the whole row moves together: the field holds the query at the rung the
/// person chose, the caption and the pills' labels hold theirs, and every one of
/// them is answered by a tap -- so a raised field that left its clear control and
/// its scope row where they were would put the control's rows on the field's ink.
/// The authored air each element keeps is solved from the authored stack, so the
/// row's proportions are the layout's and not new numbers.
const SEARCH_CLEAR_DY: usize = 4;
const SCOPE_ROW_DY: usize = 58;
const SCOPE_CAPTION_DY: usize = 62;
/// The row the field's magnifier stands on, inside the field's own plate.
///
/// WHY derived and not written at the branch that inks it: the field's height is
/// [`plate_at`]'s, so above the authored rung the box grew and the magnifier did
/// not -- the lens and the needle beside it left one another, one moving with the
/// rung and one pinned to the authored drop from the field's top.
const SEARCH_GLYPH_DY: usize = 26;

pub const fn search_glyph_dy_at(level: u8) -> usize {
    grown(SEARCH_GLYPH_DY, SEARCH_TEXT_SCALE, level)
}
#[inline]
pub fn search_glyph_dy() -> usize {
    search_glyph_dy_at(draw_level())
}
const _: () = assert!(search_glyph_dy_at(PLATFORM_LEVEL) == SEARCH_GLYPH_DY);

pub const fn search_field_at(level: u8) -> Rect {
    Rect::new(
        SEARCH_FIELD.x,
        browse_stack_y_at(level),
        SEARCH_FIELD.w,
        plate_at(SEARCH_FIELD.h, SEARCH_TEXT_SCALE, level),
    )
}
pub const fn search_clear_at(level: u8) -> Rect {
    Rect::new(
        SEARCH_CLEAR.x,
        search_field_at(level).y + SEARCH_CLEAR_DY,
        later(SEARCH_CLEAR.w, cap_at(SEARCH_TEXT_SCALE, level) as usize),
        later(SEARCH_CLEAR.h, rows_at(SEARCH_TEXT_SCALE, level)),
    )
}
pub const fn scope_pill_at(level: u8, first: bool) -> Rect {
    let authored = if first { &SCOPE_THIS_PILL } else { &SCOPE_ALL_PILL };
    Rect::new(
        authored.x,
        browse_stack_y_at(level) + grown(SCOPE_ROW_DY, SEARCH_TEXT_SCALE, level),
        authored.w,
        plate_at(authored.h, LABEL_RUNG, level),
    )
}
pub const fn scope_y_at(level: u8) -> usize {
    browse_stack_y_at(level) + grown(SCOPE_CAPTION_DY, SEARCH_TEXT_SCALE, level)
}
pub const fn scope_row_y1_at(level: u8) -> usize {
    let pill = scope_pill_at(level, true);
    pill.y + pill.h
}
#[inline]
pub fn search_field() -> Rect {
    search_field_at(draw_level())
}
#[inline]
pub fn search_clear() -> Rect {
    search_clear_at(draw_level())
}
#[inline]
pub fn scope_this_pill() -> Rect {
    scope_pill_at(draw_level(), true)
}
#[inline]
pub fn scope_all_pill() -> Rect {
    scope_pill_at(draw_level(), false)
}
#[inline]
pub fn scope_y() -> usize {
    scope_y_at(draw_level())
}
#[inline]
pub fn scope_row_y1() -> usize {
    scope_row_y1_at(draw_level())
}
const _: () = assert!(
    search_field_at(PLATFORM_LEVEL).y == SEARCH_FIELD.y
        && search_field_at(PLATFORM_LEVEL).h == SEARCH_FIELD.h
);
const _: () = assert!(search_clear_at(PLATFORM_LEVEL).y == SEARCH_CLEAR.y);
const _: () = assert!(scope_pill_at(PLATFORM_LEVEL, true).y == SCOPE_THIS_PILL.y);
const _: () = assert!(scope_pill_at(PLATFORM_LEVEL, false).y == SCOPE_ALL_PILL.y);
const _: () = assert!(scope_y_at(PLATFORM_LEVEL) == SCOPE_Y);
const _: () = assert!(scope_row_y1_at(PLATFORM_LEVEL) == SCOPE_ROW_Y1);

// The scope caption's own lane is derived from the first pill's column in
// "The lanes the painter draws into" below, as `SCOPE_CAPTION_MAX_W`.

/// The sort and layout toolbar.
pub const TOOLBAR_Y: usize = BROWSE_STACK_Y + 154;
pub const TOOLBAR_H: usize = 48;
pub const SORT_PILL: Rect = Rect::new(36, TOOLBAR_Y, 160, TOOLBAR_H);
pub const LIST_PILL: Rect = Rect::new(512, TOOLBAR_Y, 84, TOOLBAR_H);
pub const GRID_PILL: Rect = Rect::new(600, TOOLBAR_Y, 84, TOOLBAR_H);

/// The toolbar's row at a level: it keeps the gap the authored stack left under
/// the scope row, and its pills answer the label rung they carry.
const TOOLBAR_GAP: usize = TOOLBAR_Y - SCOPE_ROW_Y1;
pub const fn toolbar_y_at(level: u8) -> usize {
    scope_row_y1_at(level) + TOOLBAR_GAP
}
pub const fn toolbar_h_at(level: u8) -> usize {
    plate_at(TOOLBAR_H, LABEL_RUNG, level)
}
pub const fn toolbar_pill_at(level: u8, index: usize) -> Rect {
    let authored = match index {
        0 => &SORT_PILL,
        1 => &LIST_PILL,
        _ => &GRID_PILL,
    };
    Rect::new(
        authored.x,
        toolbar_y_at(level),
        authored.w,
        toolbar_h_at(level),
    )
}
#[inline]
pub fn toolbar_y() -> usize {
    toolbar_y_at(draw_level())
}
#[inline]
pub fn toolbar_h() -> usize {
    toolbar_h_at(draw_level())
}
#[inline]
pub fn toolbar_pill(index: usize) -> Rect {
    toolbar_pill_at(draw_level(), index)
}
const _: () = assert!(toolbar_y_at(PLATFORM_LEVEL) == TOOLBAR_Y);
const _: () = assert!(toolbar_h_at(PLATFORM_LEVEL) == TOOLBAR_H);

/// The foot of the scope row: the two pills stand taller than the caption
/// beside them, so the pills answer the row's last row.
pub const SCOPE_ROW_Y1: usize = SCOPE_THIS_PILL.y + SCOPE_THIS_PILL.h;

/// The rung the crumbs draw at: the caption rung, named in the model so the
/// plate below reserves the box the painter actually inks.
pub const BREADCRUMB_SCALE: usize = theme::type_scale::CAPTION;
/// The rows one crumb run reserves at that rung.
pub const BREADCRUMB_ROWS: usize = outline_text::row_band(0, BREADCRUMB_SCALE).1;

/// The crumb's box at a level.
const fn breadcrumb_rows_at(level: u8) -> usize {
    rows_at(BREADCRUMB_SCALE, level)
}

/// The breadcrumb's touch strip: the whole gap between the scope row's foot and
/// the toolbar's top, and nothing more.
///
/// WHY derived from the two rows either side and not authored: the strip ran from
/// `BROWSE_STACK_Y + 110` to `+ 158` while the toolbar's rows start at `+ 154`,
/// so the crumb strip and the toolbar both claimed the same four rows and the tap
/// handler had to clamp one against the other to keep the pair honest. The gap the
/// two bands leave between them is 48 rows at the rung this stack was authored at,
/// four rows more than the platform's minimum target, so the strip is that whole
/// gap: no dead row above it, no borrowed row below it. Raised rungs grow the two
/// bands around it at different rates, so the gap is not 48 there and is not
/// claimed to be -- what holds at every rung is that the strip stays a minimum
/// target tall and that its foot *is* the toolbar's top, and the sweep beside
/// [`crumb_hit_at`] is what says so. The tap handler reads that second claim as
/// permission to stop a press at the strip's foot without clamping it again.
pub const BREADCRUMB_HIT_Y0: usize = SCOPE_ROW_Y1;
pub const BREADCRUMB_HIT_Y1: usize = TOOLBAR_Y;

/// The row the crumbs' boxes stand on: the whole band centred in the strip that
/// answers it, so the painted run and the hit strip agree at any rung.
pub const BREADCRUMB_Y: usize = BREADCRUMB_HIT_Y0
    + (BREADCRUMB_HIT_Y1 - BREADCRUMB_HIT_Y0 - BREADCRUMB_ROWS) / 2;

/// The crumb strip and the row it centres its box on, at a level.
pub const fn crumb_hit_at(level: u8) -> (usize, usize) {
    (scope_row_y1_at(level), toolbar_y_at(level))
}
pub const fn crumb_y_at(level: u8) -> usize {
    let (y0, y1) = crumb_hit_at(level);
    y0 + (y1.saturating_sub(y0).saturating_sub(breadcrumb_rows_at(level)) / 2)
}
#[inline]
pub fn crumb_hit() -> (usize, usize) {
    crumb_hit_at(draw_level())
}
#[inline]
pub fn crumb_y() -> usize {
    crumb_y_at(draw_level())
}
#[inline]
pub fn crumb_rows() -> usize {
    breadcrumb_rows_at(draw_level())
}
const _: () = assert!(crumb_hit_at(PLATFORM_LEVEL).0 == BREADCRUMB_HIT_Y0);
const _: () = assert!(crumb_hit_at(PLATFORM_LEVEL).1 == BREADCRUMB_HIT_Y1);
const _: () = assert!(crumb_y_at(PLATFORM_LEVEL) == BREADCRUMB_Y);

/// Truth gates: the crumb strip is a touch target, it answers its own run, and
/// it never claims a row the bands on either side of it own.
const _: () = assert!(
    SCOPE_Y + BREADCRUMB_ROWS <= BREADCRUMB_HIT_Y0,
    "the scope caption's box must end before the crumb strip's rows start"
);
const _: () = assert!(
    BREADCRUMB_HIT_Y1 - BREADCRUMB_HIT_Y0 >= theme::MIN_TOUCH_TARGET,
    "the breadcrumb strip must stay a minimum touch target tall"
);
const _: () = assert!(
    BREADCRUMB_Y >= BREADCRUMB_HIT_Y0
        && BREADCRUMB_Y + BREADCRUMB_ROWS <= BREADCRUMB_HIT_Y1,
    "the crumbs must stand entirely inside the strip that answers them"
);

/// WHY swept over every level: the three gates above are the authored layout's
/// own claims, and the same three claims have to hold at the rung the person
/// chose -- where the field, the pills and the crumb box have all grown and the
/// strip between the scope row and the toolbar is what has to carry it.
const fn browse_stack_holds_at_every_level() -> bool {
    let mut level = 0;
    while level <= MAX_LEVEL {
        let field = search_field_at(level);
        if field.h < theme::MIN_TOUCH_TARGET {
            return false;
        }
        if field.y + rows_at(SEARCH_TEXT_SCALE, level) > field.y + field.h {
            return false;
        }
        let clear = search_clear_at(level);
        if clear.h < theme::MIN_TOUCH_TARGET || clear.w < theme::MIN_TOUCH_TARGET {
            return false;
        }
        if clear.y < field.y || clear.y + clear.h > field.y + field.h {
            return false;
        }
        let scope_y = scope_y_at(level);
        if scope_y + rows_at(SCOPE_CAPTION_SCALE, level) > scope_row_y1_at(level) {
            return false;
        }
        let pill = scope_pill_at(level, true);
        if pill.h < theme::MIN_TOUCH_TARGET {
            return false;
        }
        let bar = toolbar_y_at(level);
        if bar < scope_row_y1_at(level) {
            return false;
        }
        let (hit0, hit1) = crumb_hit_at(level);
        if hit1 - hit0 < theme::MIN_TOUCH_TARGET {
            return false;
        }
        if hit1 != bar {
            return false;
        }
        let rows = breadcrumb_rows_at(level);
        let y = crumb_y_at(level);
        if y < hit0 || y + rows > hit1 {
            return false;
        }
        if toolbar_h_at(level) < theme::MIN_TOUCH_TARGET {
            return false;
        }
        level += 1;
    }
    true
}
const _: () = assert!(browse_stack_holds_at_every_level());

/// The entries card.
///
/// WHY the card is the region and not the size of the list: a long directory
/// scrolls inside the card instead of growing past the bottom of the screen.
pub const ENTRIES_CARD: Rect = Rect::new(36, 364, 648, 920);
/// The height of one entry row.
///
/// WHY 58 and not the 56 the row was drawn with: a search row stacks two caption
/// lines inside the plate, and the lower one's box reached row 58 at the size the
/// layout was authored for. The row was 56, so the painter was handed a buffer
/// two rows shorter than the ink it was asked to carry, and clipped the
/// found-in path's own bottom away. The buffer the row is blitted out of is
/// sized from the same derivation, so it grew with it.
pub const ENTRY_ROW_H: usize = 58;

/// The list viewport at a level: it starts the row the toolbar's foot clears, and
/// it ends where the status line starts, so a stack of taller rows narrows the
/// window instead of walking off the screen -- and the scroll inside it carries the
/// rows the window no longer shows.
///
/// WHY the card's height is a subtraction and not a plate: `BODY_H` in the
/// reference is the same idea. A viewport that kept the authored 920 rows after the
/// stack above it had grown would report content below the panel, which is a scroll
/// range with rows in it that no pixel of the screen shows.
const ENTRIES_TOOLBAR_STRIDE: usize = 364 - (TOOLBAR_Y + TOOLBAR_H);
const STATUS_CARD_STRIDE: usize = STATUS_Y - (364 + 920);

/// The status line's row at a level: where it was authored while its box still
/// clears the action bar, and lifted onto the bar's own foot row when the rung has
/// grown the box past the room between them.
pub const fn status_y_at(level: u8) -> usize {
    let rows = rows_at(STATUS_SCALE, level);
    if STATUS_Y + rows <= ACTION_BAR_Y0 {
        STATUS_Y
    } else {
        ACTION_BAR_Y0.saturating_sub(rows)
    }
}
pub const fn entries_card_at(level: u8) -> Rect {
    let y = toolbar_y_at(level) + TOOLBAR_H + ENTRIES_TOOLBAR_STRIDE;
    let bottom = status_y_at(level).saturating_sub(STATUS_CARD_STRIDE);
    Rect::new(
        ENTRIES_CARD.x,
        y,
        ENTRIES_CARD.w,
        bottom.saturating_sub(y),
    )
}
/// A list row's plate at a level.
///
/// WHY sized from the line box: the row is blitted out of a widget buffer whose
/// paint loop walks the platform's `line_height`, and the sweep beside the
/// identity gates proves that box and [`rows_at`] are the same number at every
/// rung on offer -- and the buffer beside it is sized at the largest rung the
/// ladder offers, so no raised row is ever cut by the buffer that carries it.
///
/// WHY the search stack is a second bound: the plate above is the name's own box
/// plus the air it was drawn with, but a search row inks two caption lines inside
/// the same plate, and at every rung they grow by *two* caption boxes while the
/// name's plate grows by one body box. The taller of the two is the only plate
/// that both the name and the found-in line stand on -- and the row's rect, the
/// buffer it is blitted through and the hit test that divides by it all read this
/// one number, so a plate that is short is ink that never reaches the screen
/// rather than a row that is merely taller.
pub const fn entry_row_h_at(level: u8) -> usize {
    later(
        plate_at(ENTRY_ROW_H, ENTRY_NAME_SCALE, level),
        entry_search_ink_bottom_at(level),
    )
}
/// A grid cell's plate at a level: the cell carries its name at the grid's own
/// rung, and the glyph above it keeps the room it was drawn with.
pub const fn grid_cell_h_at(level: u8) -> usize {
    plate_at(GRID_CELL_H, GRID_NAME_SCALE, level)
}
#[inline]
pub fn entries_card() -> Rect {
    entries_card_at(draw_level())
}
#[inline]
pub fn entry_row_h() -> usize {
    entry_row_h_at(draw_level())
}
#[inline]
pub fn grid_cell_h() -> usize {
    grid_cell_h_at(draw_level())
}
#[inline]
pub fn status_y() -> usize {
    status_y_at(draw_level())
}
/// The rows the status line's own box reserves at this app's rung: the foot the
/// painter's band and the tap handler's strip both stop on.
#[inline]
pub fn status_rows() -> usize {
    rows_at(STATUS_SCALE, draw_level())
}
/// The empty-card pair: the headline's own drop from the card's top and the stride
/// to the subline under it, both restated so each moves by the box above it.
const EMPTY_HEADLINE_DY: usize = 150;
const EMPTY_SUBLINE_STRIDE: usize = 214 - 150;
/// The row the empty card's headline stands on, at this app's rung.
#[inline]
pub fn empty_headline_y() -> usize {
    entries_card().y + grown(EMPTY_HEADLINE_DY, EMPTY_HEADLINE_SCALE, draw_level())
}
/// The row the empty card's explanation stands on, at this app's rung: one whole
/// headline stride under the headline, so a headline with a descender cannot ink
/// into it.
#[inline]
pub fn empty_subline_y() -> usize {
    empty_headline_y() + grown(EMPTY_SUBLINE_STRIDE, EMPTY_SUBLINE_SCALE, draw_level())
}
const _: () = assert!(
    entries_card_at(PLATFORM_LEVEL).y == ENTRIES_CARD.y
        && entries_card_at(PLATFORM_LEVEL).h == ENTRIES_CARD.h
);
const _: () = assert!(entry_row_h_at(PLATFORM_LEVEL) == ENTRY_ROW_H);
const _: () = assert!(grid_cell_h_at(PLATFORM_LEVEL) == GRID_CELL_H);
const _: () = assert!(status_y_at(PLATFORM_LEVEL) == STATUS_Y);

/// The largest row plate any selectable level asks for, so the widget buffers the
/// renderer blits through are big enough for the rung a person can choose rather
/// than the one the layout was drawn at.
pub const fn entry_row_h_max() -> usize {
    let mut level = 0;
    let mut tallest = ENTRY_ROW_H;
    while level <= MAX_LEVEL {
        let h = entry_row_h_at(level);
        if h > tallest {
            tallest = h;
        }
        level += 1;
    }
    tallest
}

/// The largest grid cell any selectable level asks for, for the same reason
/// [`entry_row_h_max`] sizes the row buffer: the cell is blitted through a widget
/// buffer, and a buffer cut to the rung the layout happened to be drawn at cuts
/// the rung the person chose.
pub const fn grid_cell_h_max() -> usize {
    let mut level = 0;
    let mut tallest = GRID_CELL_H;
    while level <= MAX_LEVEL {
        let h = grid_cell_h_at(level);
        if h > tallest {
            tallest = h;
        }
        level += 1;
    }
    tallest
}

/// Truth gate, swept: every list row and grid cell is still a touch target at the
/// largest rung, and the viewport the rows scroll inside keeps a row's worth of
/// height rather than closing to nothing when the stack above it grows.
const fn entry_plates_hold_at_every_level() -> bool {
    let mut level = 0;
    while level <= MAX_LEVEL {
        if entry_row_h_at(level) < theme::MIN_TOUCH_TARGET {
            return false;
        }
        if grid_cell_h_at(level) < theme::MIN_TOUCH_TARGET {
            return false;
        }
        let card = entries_card_at(level);
        if card.h < entry_row_h_at(level) {
            return false;
        }
        if card.y + card.h > status_y_at(level) {
            return false;
        }
        if status_y_at(level) + rows_at(STATUS_SCALE, level) > ACTION_BAR_Y0 {
            return false;
        }
        if entry_row_h_at(level) > entry_row_h_max() {
            return false;
        }
        level += 1;
    }
    true
}
const _: () = assert!(entry_plates_hold_at_every_level());

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

/// The details card at a level: it stands where the entries card stands, its title
/// keeps the air above it, its lines keep the stride the ladder gave them plus
/// their own growth, and its two buttons stay inside it.
///
/// WHY the card grows rather than the rows being cut: it lists a fixed number of
/// lines and has no scroll of its own, so a rung that grew the title and every line
/// without growing the card would run the last line through the buttons that close
/// it -- and the closing buttons are the only way out of the view.
const DETAILS_TITLE_DY: usize = DETAILS_TITLE_Y - DETAILS_CARD.y;
const DETAILS_LINES_DY: usize = DETAILS_LINE_Y0 - DETAILS_CARD.y;
const DETAILS_BUTTON_DY: usize = DETAILS_OPEN.y - DETAILS_CARD.y;
/// The air the authored card keeps below its two buttons.
const DETAILS_FOOT_AIR: usize =
    DETAILS_CARD.h - (DETAILS_BUTTON_DY + DETAILS_OPEN.h);

pub const fn details_title_dy_at(level: u8) -> usize {
    grown(DETAILS_TITLE_DY, DETAILS_TITLE_SCALE, level)
}
pub const fn details_lines_dy_at(level: u8) -> usize {
    grown(DETAILS_LINES_DY, DETAILS_TITLE_SCALE, level)
}
pub const fn details_line_h_at(level: u8) -> usize {
    plate_at(DETAILS_LINE_H, DETAILS_LINE_SCALE, level)
}
pub const fn details_button_h_at(level: u8) -> usize {
    plate_at(DETAILS_OPEN.h, LABEL_RUNG, level)
}
pub const fn details_button_dy_at(level: u8) -> usize {
    later(
        DETAILS_BUTTON_DY,
        details_lines_dy_at(level) + MAX_DETAILS_LINES * details_line_h_at(level) + 12,
    )
}
pub const fn details_card_at(level: u8) -> Rect {
    Rect::new(
        DETAILS_CARD.x,
        entries_card_at(level).y,
        DETAILS_CARD.w,
        details_button_dy_at(level) + details_button_h_at(level) + DETAILS_FOOT_AIR,
    )
}
pub const fn details_title_y_at(level: u8) -> usize {
    entries_card_at(level).y + details_title_dy_at(level)
}
pub const fn details_line_y_at(level: u8, index: usize) -> usize {
    entries_card_at(level).y + details_lines_dy_at(level) + index * details_line_h_at(level)
}
pub const fn details_open_at(level: u8) -> Rect {
    Rect::new(
        DETAILS_OPEN.x,
        entries_card_at(level).y + details_button_dy_at(level),
        DETAILS_OPEN.w,
        details_button_h_at(level),
    )
}
pub const fn details_close_at(level: u8) -> Rect {
    let open = details_open_at(level);
    Rect::new(DETAILS_CLOSE.x, open.y, DETAILS_CLOSE.w, open.h)
}
#[inline]
pub fn details_card() -> Rect {
    details_card_at(draw_level())
}
#[inline]
pub fn details_title_y() -> usize {
    details_title_y_at(draw_level())
}
#[inline]
pub fn details_line_y(index: usize) -> usize {
    details_line_y_at(draw_level(), index)
}
#[inline]
pub fn details_line_h() -> usize {
    details_line_h_at(draw_level())
}
#[inline]
pub fn details_open() -> Rect {
    details_open_at(draw_level())
}
#[inline]
pub fn details_close() -> Rect {
    details_close_at(draw_level())
}
const _: () = assert!(
    details_card_at(PLATFORM_LEVEL).y == DETAILS_CARD.y
        && details_card_at(PLATFORM_LEVEL).h == DETAILS_CARD.h
);
const _: () = assert!(details_title_y_at(PLATFORM_LEVEL) == DETAILS_TITLE_Y);
const _: () = assert!(details_line_y_at(PLATFORM_LEVEL, 0) == DETAILS_LINE_Y0);
const _: () = assert!(details_line_h_at(PLATFORM_LEVEL) == DETAILS_LINE_H);
const _: () = assert!(details_open_at(PLATFORM_LEVEL).y == DETAILS_OPEN.y);
const _: () = assert!(details_close_at(PLATFORM_LEVEL).y == DETAILS_CLOSE.y);

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

/// The sheet at a level: its heading, its field or destination slots and its two
/// buttons each answer the rung they carry, and the sheet grows to hold them rather
/// than painting the buttons off its own plate.
const SHEET_DEST_DY: usize = SHEET_DEST_Y - SHEET.y;
const SHEET_BUTTON_DY: usize = SHEET_BUTTON_Y - SHEET.y;
/// The air the authored sheet keeps below its two buttons.
const SHEET_FOOT_AIR: usize = SHEET.h - (SHEET_BUTTON_DY + SHEET_BUTTON_H);

pub const fn sheet_slot_h_at(level: u8) -> usize {
    plate_at(SHEET_DEST_H, LABEL_RUNG, level)
}
pub const fn sheet_dest_y_at(level: u8) -> usize {
    SHEET.y + grown(SHEET_DEST_DY, LABEL_RUNG, level)
}
pub const fn sheet_button_y_at(level: u8) -> usize {
    let stack = sheet_dest_y_at(level) + sheet_slot_h_at(level) + 12;
    later(SHEET.y + SHEET_BUTTON_DY, stack)
}
pub const fn sheet_at(level: u8) -> Rect {
    Rect::new(
        SHEET.x,
        SHEET.y,
        SHEET.w,
        sheet_button_y_at(level) + sheet_slot_h_at(level) + SHEET_FOOT_AIR - SHEET.y,
    )
}
pub const fn sheet_name_h_at(level: u8) -> usize {
    plate_at(SHEET_NAME_H, SEARCH_TEXT_SCALE, level)
}
#[inline]
pub fn sheet() -> Rect {
    sheet_at(draw_level())
}
#[inline]
pub fn sheet_dest_y() -> usize {
    sheet_dest_y_at(draw_level())
}
#[inline]
pub fn sheet_slot_h() -> usize {
    sheet_slot_h_at(draw_level())
}
#[inline]
pub fn sheet_button_y() -> usize {
    sheet_button_y_at(draw_level())
}
#[inline]
pub fn sheet_name_y() -> usize {
    sheet_dest_y_at(draw_level())
}
#[inline]
pub fn sheet_name_h() -> usize {
    sheet_name_h_at(draw_level())
}
/// The air the sheet keeps above its heading, and the stride between the name
/// lines under it -- both as the sheet was drawn with, restated so a raised rung
/// moves each line by the box above it rather than running the lines together.
pub const SHEET_TITLE_DY: usize = 20;
pub const SHEET_TITLE_SCALE: usize = theme::type_scale::BODY;
const SHEET_NAME_FIRST_DY: usize = 76;
const SHEET_NAME_STRIDE: usize = 24;
pub const SHEET_NAME_LIST_SCALE: usize = theme::type_scale::CAPTION;
/// The sheet's heading row, at this app's rung.
#[inline]
pub fn sheet_title_y() -> usize {
    sheet().y + SHEET_TITLE_DY
}
/// The row the sheet's `index`-th name line stands on, at this app's rung.
#[inline]
pub fn sheet_name_line_y(index: usize) -> usize {
    let lvl = draw_level();
    sheet().y
        + grown(SHEET_NAME_FIRST_DY, SHEET_TITLE_SCALE, lvl)
        + index * grown(SHEET_NAME_STRIDE, SHEET_NAME_LIST_SCALE, lvl)
}
const _: () = assert!(sheet_at(PLATFORM_LEVEL).h == SHEET.h);
const _: () = assert!(sheet_dest_y_at(PLATFORM_LEVEL) == SHEET_DEST_Y);
const _: () = assert!(sheet_slot_h_at(PLATFORM_LEVEL) == SHEET_DEST_H);
const _: () = assert!(sheet_button_y_at(PLATFORM_LEVEL) == SHEET_BUTTON_Y);
const _: () = assert!(sheet_name_h_at(PLATFORM_LEVEL) == SHEET_NAME_H);

/// Truth gate, swept: the two overlay plates hold every run they paint and stay on
/// the panel at the largest rung.
///
/// WHY these two are checked together: both are drawn over the list and both close
/// with a button, so a card whose last line reaches its own buttons and a sheet
/// whose button walks off the panel are the same defect seen from the two surfaces
/// that have no scroll.
const fn overlays_hold_at_every_level() -> bool {
    let mut level = 0;
    while level <= MAX_LEVEL {
        let card = details_card_at(level);
        let last_line = details_line_y_at(level, MAX_DETAILS_LINES - 1)
            + rows_at(DETAILS_LINE_SCALE, level);
        if last_line > details_open_at(level).y {
            return false;
        }
        if details_open_at(level).y + details_button_h_at(level) > card.y + card.h {
            return false;
        }
        if card.y + card.h > 1440 {
            return false;
        }
        if details_title_y_at(level) + rows_at(DETAILS_TITLE_SCALE, level)
            > details_lines_dy_at(level) + card.y
        {
            return false;
        }
        let sheet = sheet_at(level);
        if sheet.y + sheet.h > 1440 {
            return false;
        }
        if sheet_button_y_at(level) < sheet_dest_y_at(level) + sheet_slot_h_at(level) {
            return false;
        }
        if sheet_slot_h_at(level) < theme::MIN_TOUCH_TARGET {
            return false;
        }
        if details_button_h_at(level) < theme::MIN_TOUCH_TARGET {
            return false;
        }
        level += 1;
    }
    true
}
const _: () = assert!(overlays_hold_at_every_level());

/// The longest a status or scope line may be.
pub const LINE_LEN: usize = 128;

// -----------------------------------------------------------------------------
// The lanes the painter draws into
//
// WHY these live in the library and not beside the per-pixel branch that draws
// them: this app's binary is built with `test = false`, so a fit gate can only
// be a `#[test]` in this library -- and a `#[test]` can only ask a container it
// can reach. Every width below used to be a literal inside the branch that
// painted it (`600` for the details card, `64`/`20` for a list row, `24` for a
// grid cell, `1` and `2` for the rungs), and the one host gate that claimed to
// check the app's text measured `510`, `150` and `260` -- numbers no container
// on this screen owns. That is the defect class where a real clip passes the
// suite: raise a rung, the run grows, the remembered lane stays, and nothing
// answers.
//
// Every lane here is derived from the box the painter actually draws into, and
// every gap of air is its own named constant, so moving the box moves the lane
// and the gate with it. The rungs are named on the platform's ladder rather than
// written as multipliers, because a draw site that types its own multiplier can
// measure a run at one rung and ink it at another.
// -----------------------------------------------------------------------------

/// The rung this app's control labels read at: the caption rung.
///
/// WHY named: the pills, the sheet's destination slots and the sheet's two
/// buttons each passed a bare `1` to the rasteriser, so a rung move could not be
/// checked against the label it drew.
pub const LABEL_RUNG: usize = theme::type_scale::CAPTION;

/// The air a control's label keeps inside the control that carries it.
///
/// WHY one grid step of air in total rather than one per side: the label is
/// centred, so its air is split across its two sides by `centered_x` -- a
/// measure per side is a bound this app's own words ("Rename" in an 86-row slot)
/// do not keep, and inventing a stricter bound than the control owns is how a
/// gate starts refusing runs the screen shows honestly.
pub const CONTROL_LABEL_PAD: usize = 8;

/// The lane a control's label owns inside a control `w` rows wide.
///
/// WHY a function and not a table: the same claim covers the scope pills, the
/// toolbar pills, the action slots, the sheet's four destination slots and its
/// two buttons, and one copy of the arithmetic is what lets the painter and the
/// gate disagree about nothing.
pub const fn control_label_lane(w: usize) -> usize {
    if w > CONTROL_LABEL_PAD {
        w - CONTROL_LABEL_PAD
    } else {
        w
    }
}

/// The list row's glyph column, the text column beside it, and the air the name
/// keeps from the row's right edge.
pub const ENTRY_GLYPH_X: usize = 20;
pub const ENTRY_TEXT_X: usize = 64;
pub const ENTRY_RIGHT_AIR: usize = 20;

/// The rungs a list row inks: its name at the body rung, its found-in path at
/// the caption rung.
pub const ENTRY_NAME_SCALE: usize = theme::type_scale::BODY;
pub const ENTRY_SUB_SCALE: usize = theme::type_scale::CAPTION;

/// The lane a list row's name is ellipsized to.
///
/// Derived from the row's own box: the card's width, less the text column the
/// run starts in and the air before the row's right edge. The painter used to
/// write `ROW_BUFFER_W - 64 - 20` inside the branch that drew it.
pub const ENTRY_NAME_LANE: usize = ENTRIES_CARD.w - ENTRY_TEXT_X - ENTRY_RIGHT_AIR;

/// The grid cell's text inset and the lane its name is ellipsized to.
pub const GRID_TEXT_INSET: usize = 12;
pub const GRID_NAME_SCALE: usize = theme::type_scale::CAPTION;
pub const GRID_NAME_LANE: usize = GRID_CELL_W - 2 * GRID_TEXT_INSET;

/// The pictogram box every entry glyph is drawn in.
///
/// WHY named at all: `paint_glyph` walked `0..26` and `0..34` inline, so the box
/// was two numbers no gate could read and the sweep below could not ask whether
/// the glyph still stands inside the plate that carries it. It is the artwork's
/// own extent, not a layout figure, so it stays authored at every rung -- what
/// must move is the row it stands on, which is the pair of faces under this.
pub const GLYPH_COLS: usize = 34;
pub const GLYPH_ROWS: usize = 26;

/// The air a list row keeps above its glyph, and the column the grid's glyph
/// stands in with the cell's own width taken from both sides.
///
/// WHY derived and not written at the branch that inks it: the row's and the
/// cell's plates are [`plate_at`]'s, so above the authored rung the box grew
/// while `16` and the hand-centred `83` did not -- the glyph stayed glued to the
/// top of a plate the person just made taller, and the caption under it, which
/// does move, closed on it. Centring is derived from the cell's width, so a cell
/// that ever narrows keeps its glyph in the middle instead of off-centre.
const ENTRY_GLYPH_DY: usize = 16;
const GRID_GLYPH_DY: usize = 30;

pub const fn entry_glyph_dy_at(level: u8) -> usize {
    grown(ENTRY_GLYPH_DY, ENTRY_NAME_SCALE, level)
}
pub const fn grid_glyph_dy_at(level: u8) -> usize {
    grown(GRID_GLYPH_DY, GRID_NAME_SCALE, level)
}
pub const GRID_GLYPH_X: usize = (GRID_CELL_W - GLYPH_COLS) / 2;

#[inline]
pub fn entry_glyph_dy() -> usize {
    entry_glyph_dy_at(draw_level())
}
#[inline]
pub fn grid_glyph_dy() -> usize {
    grid_glyph_dy_at(draw_level())
}
const _: () = assert!(entry_glyph_dy_at(PLATFORM_LEVEL) == ENTRY_GLYPH_DY);
const _: () = assert!(grid_glyph_dy_at(PLATFORM_LEVEL) == GRID_GLYPH_DY);
const _: () = assert!(GRID_GLYPH_X == 83);

/// The two rows a search result's runs stand on: the document's name, and the
/// folder it was found in directly under it.
///
/// WHY faces and not the numbers added inside the branch: the painter wrote
/// `grown(6, ..)` and `grown(28, ..)` beside the `draw_buffer_text` call, so the
/// offset moved with the rung while nothing else in the app knew those two runs
/// were the tallest ink the row carries. [`entry_row_h_at`] bounds its plate by
/// the lower of them, and a plate that bounded an offset no gate could read would
/// have been checking a stack the painter was free to move at any time.
const ENTRY_SEARCH_NAME_DY: usize = 6;
const ENTRY_SEARCH_PATH_DY: usize = 28;

pub const fn entry_search_name_dy_at(level: u8) -> usize {
    grown(ENTRY_SEARCH_NAME_DY, ENTRY_SUB_SCALE, level)
}
pub const fn entry_search_path_dy_at(level: u8) -> usize {
    grown(ENTRY_SEARCH_PATH_DY, ENTRY_SUB_SCALE, level)
}
/// The row a search result's lower run reaches, measured from its plate's top.
pub const fn entry_search_ink_bottom_at(level: u8) -> usize {
    entry_search_path_dy_at(level) + rows_at(ENTRY_SUB_SCALE, level)
}

#[inline]
pub fn entry_search_name_dy() -> usize {
    entry_search_name_dy_at(draw_level())
}
#[inline]
pub fn entry_search_path_dy() -> usize {
    entry_search_path_dy_at(draw_level())
}
const _: () = assert!(entry_search_name_dy_at(PLATFORM_LEVEL) == ENTRY_SEARCH_NAME_DY);
const _: () = assert!(entry_search_path_dy_at(PLATFORM_LEVEL) == ENTRY_SEARCH_PATH_DY);

/// Truth gate, swept: each of the offsets this app used to add inside the branch
/// that inked it still puts its mark inside the plate that carries it, and runs
/// forwards with the rung rather than drifting up toward the plate's top.
const fn authored_paint_pixels_hold_at_every_level() -> bool {
    let mut level = 0;
    while level <= MAX_LEVEL {
        if entry_glyph_dy_at(level) + GLYPH_ROWS > entry_row_h_at(level) {
            return false;
        }
        if grid_glyph_dy_at(level) + GLYPH_ROWS > grid_cell_h_at(level) {
            return false;
        }
        if GRID_GLYPH_X + GLYPH_COLS > GRID_CELL_W {
            return false;
        }
        if search_glyph_dy_at(level) + GLYPH_ROWS > search_field_at(level).h {
            return false;
        }
        if entry_search_ink_bottom_at(level) > entry_row_h_at(level) {
            return false;
        }
        if entry_search_path_dy_at(level) <= entry_search_name_dy_at(level) {
            return false;
        }
        let block = HOME_RECENTS_VISIBLE * home_recent_row_h_at(level);
        if home_recents_note_dy_at(level) + rows_at(LABEL_RUNG, level) > block {
            return false;
        }
        if level > PLATFORM_LEVEL {
            if entry_glyph_dy_at(level) <= entry_glyph_dy_at(PLATFORM_LEVEL) {
                return false;
            }
            if grid_glyph_dy_at(level) <= grid_glyph_dy_at(PLATFORM_LEVEL) {
                return false;
            }
            if search_glyph_dy_at(level) <= search_glyph_dy_at(PLATFORM_LEVEL) {
                return false;
            }
            if entry_search_path_dy_at(level) <= entry_search_path_dy_at(PLATFORM_LEVEL) {
                return false;
            }
            if home_recents_note_dy_at(level) <= home_recents_note_dy_at(PLATFORM_LEVEL) {
                return false;
            }
        }
        level += 1;
    }
    true
}
const _: () = assert!(authored_paint_pixels_hold_at_every_level());

/// The details card's inset, its two rungs and the two lanes they answer.
///
/// WHY one lane for both: the title and the lines stand on the same two edges of
/// the same card, so a line that outgrows the card and a title that outgrows it
/// are the same defect; the rungs differ, so the two lanes are checked at their
/// own rung by the gate beside this.
pub const DETAILS_INSET: usize = 24;
pub const DETAILS_TITLE_SCALE: usize = theme::type_scale::BODY;
pub const DETAILS_LINE_SCALE: usize = theme::type_scale::CAPTION;
pub const DETAILS_TITLE_LANE: usize = DETAILS_CARD.w - 2 * DETAILS_INSET;
pub const DETAILS_LINE_LANE: usize = DETAILS_TITLE_LANE;

/// The home screen's category rows: the box each row is drawn in, the column its
/// label starts in and the air it keeps from the row's right edge.
pub const HOME_ROW_X: usize = 36;
pub const HOME_ROW_W: usize = 648;
pub const CATEGORY_LABEL_X: usize = 108;
pub const CATEGORY_RIGHT_AIR: usize = 24;
pub const CATEGORY_LABEL_SCALE: usize = theme::type_scale::BODY;
pub const CATEGORY_CAPTION_SCALE: usize = theme::type_scale::CAPTION;
pub const CATEGORY_ROW_X1: usize = HOME_ROW_X + HOME_ROW_W;
pub const CATEGORY_LABEL_LANE: usize = CATEGORY_ROW_X1 - CATEGORY_LABEL_X - CATEGORY_RIGHT_AIR;

/// The search scope's caption: its column, its rung, the air it keeps from the
/// first pill, and the lane that answers both.
///
/// WHY derived rather than written: the remembered `380` was this same distance,
/// measured by hand off the first pill; the day a pill moves, the remembered
/// number is what lets the caption run under it.
pub const SCOPE_TEXT_X: usize = HOME_ROW_X;
pub const SCOPE_CAPTION_SCALE: usize = theme::type_scale::CAPTION;
pub const SCOPE_PILL_CLEARANCE: usize = 16;
pub const SCOPE_CAPTION_MAX_W: usize = SCOPE_THIS_PILL.x - SCOPE_TEXT_X - SCOPE_PILL_CLEARANCE;

/// The crumb strip: the two columns it answers, the rung it inks, the gap it
/// spends between a run and its separator, and the least room a run must have
/// before drawing it says anything.
///
/// WHY the strip's edges come from the entries card: the painter's branch gated
/// itself on `(36..684)`, a remembered pair of columns, while the hit test walked
/// the whole crumb list with no edge at all. Both now answer the same strip, so a
/// crumb that is drawn is a crumb that can be tapped, and a run never straddles
/// the edge of the band that owns it.
pub const CRUMB_ROW_X0: usize = ENTRIES_CARD.x;
pub const CRUMB_ROW_X1: usize = ENTRIES_CARD.x + ENTRIES_CARD.w;
/// The panel's content column: the right edge every full-width row of this app
/// answers, taken from the card those rows are drawn on rather than written as
/// the remembered `684` at each hit band.
pub const PANEL_CONTENT_X1: usize = ENTRIES_CARD.x + ENTRIES_CARD.w;
pub const CRUMB_STRIP_W: usize = CRUMB_ROW_X1 - CRUMB_ROW_X0;
pub const CRUMB_SEP: &[u8] = b">";
pub const CRUMB_SEP_GAP: usize = 6;
/// The least lane worth drawing: the separator's own advance plus its two gaps,
/// so a crumb that would land in less than that is not drawn at all rather than
/// drawn as a sliver the hit test cannot answer.
pub const CRUMB_MIN_ROOM: usize = 14;

/// One crumb's place in the strip: where its run stands, how many bytes of the
/// caller's buffer it wrote, where the separator stands when the row keeps going,
/// and where the next crumb's run starts.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct CrumbStep {
    pub x: usize,
    pub len: usize,
    pub sep_x: Option<usize>,
    pub next_x: usize,
}

/// Lays one crumb out inside the strip, trimming its run to the room the strip
/// has left at `x`.
///
/// WHY the painter and the hit test both call this: it is the only way the two
/// can be kept answering the same letters. `crumb_at` used to measure the raw
/// label at a typed rung while the painter inked the same label through the
/// ladder's caption rung, and `ellipsize` spends its trim budget in the outline
/// face's advances -- so a run that was trimmed for the screen was still walked
/// at its full width by the tap handler, and a tap landed on the wrong directory.
///
/// `None` means the strip has no room left for this crumb; the caller stops, and
/// the path the strip cannot show is already shown whole in the header's readout.
pub fn crumb_step(label: &[u8], x: usize, is_last: bool, buf: &mut [u8]) -> Option<CrumbStep> {
    // The gap the run spends before the next mark is paid for out of the strip
    // too, so a crumb that fills the strip's last rows leaves its own cursor
    // inside the edge instead of one gap past it.
    let room = CRUMB_ROW_X1
        .saturating_sub(x)
        .saturating_sub(CRUMB_SEP_GAP);
    if room < CRUMB_MIN_ROOM {
        return None;
    }
    let len = ellipsize_run(label, BREADCRUMB_SCALE, room, buf);
    if len == 0 {
        return None;
    }
    let w = run_w(&buf[..len], BREADCRUMB_SCALE);
    let after_run = x + w + CRUMB_SEP_GAP;
    let sep_w = run_w(CRUMB_SEP, BREADCRUMB_SCALE);
    let sep_x = if !is_last && after_run + sep_w <= CRUMB_ROW_X1 {
        Some(after_run)
    } else {
        None
    };
    let next_x = match sep_x {
        Some(sx) => sx + sep_w + CRUMB_SEP_GAP,
        None => after_run,
    };
    Some(CrumbStep {
        x,
        len,
        sep_x,
        next_x,
    })
}

/// The search field's text column, its rung, the air it keeps from the clear
/// action inside the same field, and the lane that answers both.
///
/// WHY a lane at all: the field inked whatever the user had typed from a fixed
/// column with no bound, so a long needle ran under the clear control and past
/// the field's own edge, and the pixels that disagreed with the field's box were
/// the pixels the tap handler still answered.
pub const SEARCH_TEXT_X: usize = SEARCH_FIELD.x + 48;
pub const SEARCH_TEXT_SCALE: usize = theme::type_scale::CAPTION;
pub const SEARCH_CLEAR_CLEARANCE: usize = 8;
pub const SEARCH_TEXT_LANE: usize = SEARCH_CLEAR.x - SEARCH_TEXT_X - SEARCH_CLEAR_CLEARANCE;

/// The card's empty state: the two centred runs it draws, their rungs, and the
/// lane they share inside the card.
///
/// WHY a lane here too: the headline and the subline are the only words on the
/// screen that explain why nothing is listed, and they were centred against the
/// card's width with no bound of their own -- a refusal the platform phrased at
/// length simply left the card.
pub const EMPTY_HEADLINE_SCALE: usize = theme::type_scale::BODY;
pub const EMPTY_SUBLINE_SCALE: usize = theme::type_scale::CAPTION;
pub const EMPTY_CARD_LANE: usize = control_label_lane(ENTRIES_CARD.w);

/// The status line: its column, its rung and the lane the panel gives it.
pub const STATUS_TEXT_X: usize = ENTRIES_CARD.x;
pub const STATUS_SCALE: usize = theme::type_scale::CAPTION;
pub const STATUS_LANE: usize = ENTRIES_CARD.w;

/// The run a container hands its painter: `label` whole when the container holds
/// it, otherwise the same label cut to `lane` with an ellipsis on it -- and when
/// the lane cannot even carry the ellipsis, the leading glyphs the lane does
/// hold, cut hard rather than spilled past the container.
///
/// WHY it lives in the model: every call site is a per-pixel branch this app's
/// binary cannot test, and the claim a fit gate is worth making -- a run this app
/// draws is either whole or visibly cut, never silently missing -- has to be
/// checked against the same function the painter calls. A lane narrower than the
/// ellipsis itself answers `font::ellipsize` with nothing, and nothing is the one
/// answer that used to be worse than the run: the helper handed back `label`
/// whole, so a run the lane could not hold was inked past the lane's edge anyway
/// and the pixels outside it belonged to whatever was drawn next. The hard cut
/// keeps the bound the container asked for; the gate beside this is what refuses
/// a lane so narrow that the cut loses its mark.
/// `label` and `buf` share one lifetime because the answer is either of them: the
/// caller's borrow ends where the painted run does, which is the block that draws
/// it.
pub fn fitted_run<'a>(
    label: &'a [u8],
    scale: usize,
    lane: usize,
    buf: &'a mut [u8],
) -> &'a [u8] {
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

/// The confirmation sheet's destination slots: the first slot's column, the slot
/// width, and the gap between two slots.
///
/// WHY 140 and not 150: four slots of 150 with three 16px gaps span 648px
/// starting 24px inside a 648px sheet, so the last slot hung 24px past the
/// sheet's own edge. 140 and 12 put all four inside it.
pub const SHEET_DEST_X0: usize = 60;
pub const SHEET_DEST_SLOT_W: usize = 140;
pub const SHEET_DEST_SLOT_GAP: usize = 12;
/// The sheet's two buttons: their columns and their shared width.
pub const SHEET_CANCEL_X: usize = 80;
pub const SHEET_CONFIRM_X: usize = 380;
pub const SHEET_BUTTON_W: usize = 260;

/// Truth gates: every lane is a lane -- it owns room of its own and it never
/// reaches past the box it is drawn in. These are the checks that fire if a
/// container moves and its lane is left where the remembered number had it.
const _: () = assert!(ENTRIES_CARD.x == HOME_ROW_X && ENTRIES_CARD.w == HOME_ROW_W);
const _: () = assert!(ENTRY_TEXT_X > ENTRY_GLYPH_X);
const _: () = assert!(ENTRY_NAME_LANE > 0);
const _: () = assert!(
    ENTRY_TEXT_X + ENTRY_NAME_LANE <= ENTRIES_CARD.x + ENTRIES_CARD.w,
    "a list row's name lane must end inside the row it is drawn on"
);
const _: () = assert!(GRID_NAME_LANE > 0);
const _: () = assert!(
    GRID_TEXT_INSET + GRID_NAME_LANE <= GRID_CELL_W,
    "a grid cell's name lane must end inside the cell it is drawn on"
);
const _: () = assert!(DETAILS_TITLE_LANE > 0);
const _: () = assert!(
    DETAILS_CARD.x + DETAILS_INSET + DETAILS_TITLE_LANE <= DETAILS_CARD.x + DETAILS_CARD.w,
    "the details card's title lane must end inside the card"
);
const _: () = assert!(CATEGORY_LABEL_X > 36 + 44);
const _: () = assert!(
    CATEGORY_LABEL_X + CATEGORY_LABEL_LANE <= CATEGORY_ROW_X1,
    "a category row's label lane must end inside the row"
);
const _: () = assert!(
    SCOPE_TEXT_X + SCOPE_CAPTION_MAX_W <= SCOPE_THIS_PILL.x,
    "the scope caption must stop before the first pill it sits beside"
);
const _: () = assert!(SEARCH_TEXT_LANE > 0);
const _: () = assert!(
    SEARCH_TEXT_X + SEARCH_TEXT_LANE <= SEARCH_CLEAR.x,
    "the search field's text lane must stop before the clear action it is typed beside"
);
const _: () = assert!(control_label_lane(SCOPE_THIS_PILL.w) > 0);
const _: () = assert!(control_label_lane(SHEET_DEST_SLOT_W) > 0);
const _: () = assert!(control_label_lane(SHEET_BUTTON_W) > 0);
const _: () = assert!(CRUMB_STRIP_W > 0);
const _: () = assert!(PANEL_CONTENT_X1 == CATEGORY_ROW_X1);
const _: () = assert!(PANEL_CONTENT_X1 == CRUMB_ROW_X1);
const _: () = assert!(EMPTY_CARD_LANE > 0);
const _: () = assert!(
    STATUS_TEXT_X + STATUS_LANE <= CRUMB_ROW_X1,
    "the status line must stay inside the panel's own column"
);
const _: () = assert!(CRUMB_ROW_X1 <= ENTRIES_CARD.x + ENTRIES_CARD.w);
const _: () = assert!(CRUMB_MIN_ROOM > CRUMB_SEP_GAP);

/// The height of the list's content for a given number of items.
pub fn list_content_height(item_count: usize) -> usize {
    item_count * entry_row_h()
}

/// The rectangle one item's row is drawn in.
///
/// The renderer calls this, and the hit test calls [`entry_at_point`], so a
/// row that is drawn and a row that is tapped are the same row.
///
/// WHY the row's height comes from the published rung: the card scrolls its rows,
/// so a taller stack reaches the person instead of being cut -- which is only true
/// while the row the painter fills, the index the tap resolves and the scroll
/// range that carries them all answer the same one number.
pub fn entry_row_rect(card: Rect, index: usize, scroll_offset: usize) -> Rect {
    let row_h = entry_row_h();
    let top = (card.y + index * row_h).saturating_sub(scroll_offset);
    Rect::new(card.x, top, card.w, row_h)
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
    let index = (y - card.y + scroll_offset) / entry_row_h();
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
    let cell_h = grid_cell_h();
    rows * cell_h + rows.saturating_sub(1) * GRID_GAP
}

/// The rectangle one grid cell is drawn in.
pub fn grid_cell_rect(card: Rect, index: usize, scroll_offset: usize) -> Rect {
    let row = index / GRID_COLS;
    let col = index % GRID_COLS;
    let cell_h = grid_cell_h();
    let x = card.x + GRID_PAD + col * (GRID_CELL_W + GRID_GAP);
    let top = (card.y + row * (cell_h + GRID_GAP)).saturating_sub(scroll_offset);
    Rect::new(x, top, GRID_CELL_W, cell_h)
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
    let cell_h = grid_cell_h();
    let row = rel_y / (cell_h + GRID_GAP);
    if rel_y % (cell_h + GRID_GAP) >= cell_h {
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
/// WHY the slot's width and gap are the library's constants above: the label the
/// sheet centres in this slot is ellipsized to `control_label_lane` of this very
/// width, and a slot authored twice is a slot whose lane the gate cannot trust.
pub fn sheet_destination_rect(index: usize) -> Rect {
    let x = SHEET_DEST_X0 + index * (SHEET_DEST_SLOT_W + SHEET_DEST_SLOT_GAP);
    Rect::new(
        x,
        sheet_dest_y_at(draw_level()),
        SHEET_DEST_SLOT_W,
        sheet_slot_h_at(draw_level()),
    )
}

/// The confirmation sheet's confirm and cancel slots.
pub fn sheet_button_rect(confirm: bool) -> Rect {
    let x = if confirm { SHEET_CONFIRM_X } else { SHEET_CANCEL_X };
    Rect::new(
        x,
        sheet_button_y_at(draw_level()),
        SHEET_BUTTON_W,
        sheet_slot_h_at(draw_level()),
    )
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

/// Fits one details slot to the lane its card owns, writing the answer back even
/// when the trim refused to answer at all.
///
/// WHY it always writes: a refusal is the one answer `ellipsize_run` gives with a
/// length of `0` and without writing a byte, and a caller that read `> 0` as
/// "adopt this" left the slot's whole label in place -- the card then inked a run
/// its own lane cannot hold, past the inset on both sides, over whatever is drawn
/// under it. [`fitted_run`] is the only answer this slot may hold: the label whole
/// when the lane carries it, the label with a visible mark when it does not, and
/// the leading glyphs the lane does hold when even the mark will not fit. A run
/// that needs no cutting is handed back untouched, because that is what
/// [`fitted_run`] returns for it.
///
/// WHY the plate is the caller's and not a fixed array: the plate's own length is
/// what makes `ellipsize_run` refuse (`out_buf.len() < 3`), and a slot whose test
/// could only ever hand it a card-sized plate is a slot whose refusal no test can
/// reach. Every real call site passes a plate as wide as the slot.
///
/// `next` is filled inside the block and assigned after it: the answer can be
/// borrowed from the very slot being replaced -- a label the lane holds whole
/// comes back as the label, not as plate bytes -- so the write-back has to wait
/// for the read to end.
fn fit_details_slot<const N: usize>(
    slot: &mut [u8; N],
    len: &mut usize,
    scale: usize,
    lane: usize,
    plate: &mut [u8],
) {
    let mut next = [0u8; N];
    let kept = {
        let run = fitted_run(&slot[..*len], scale, lane, plate);
        let take = run.len().min(N);
        next[..take].copy_from_slice(&run[..take]);
        take
    };
    *slot = next;
    *len = kept;
}

/// Trims a details card's title and lines to what the card can draw.
///
/// WHY the two bounds are the library's: `fit_details` is the card's own text-fit
/// gate, and the card's title rung and line rung live beside the card's box here
/// in the model -- so the trim is checked against the lane the card actually owns
/// rather than a width a painter remembered, and against the refusal the same
/// lane answers with. That refusal is the reason this moved out of the binary:
/// the card kept the whole label whenever the trim said nothing, and only the
/// model can be reached by a test that hands a slot a plate too small for the
/// mark it was going to cut with.
pub fn fit_details(view: &mut DetailsView) {
    let mut plate = [0u8; MAX_DETAILS_LINE];
    fit_details_slot(
        &mut view.title,
        &mut view.title_len,
        DETAILS_TITLE_SCALE,
        DETAILS_TITLE_LANE,
        &mut plate,
    );
    for index in 0..view.line_count {
        let mut plate = [0u8; MAX_DETAILS_LINE];
        fit_details_slot(
            &mut view.lines[index],
            &mut view.line_lens[index],
            DETAILS_LINE_SCALE,
            DETAILS_LINE_LANE,
            &mut plate,
        );
    }
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
    /// The platform does not support this operation.
    UnsupportedByPlatform,
}

/// Whether the platform exposes a directory-creation call.
///
/// WHY true: the SDK's filesystem surface exposes fs_mkdir via syscall 161.
pub const NEW_FOLDER_SUPPORTED: bool = true;

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
            b"NOT IMPLEMENTED: the platform does not support this operation"
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
    /// A new folder. The platform supports directory creation via `fs_mkdir`;
    /// see [`NEW_FOLDER_SUPPORTED`].
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

/// The outcome of tapping an action control in the action bar.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ActionTapOutcome {
    /// Opens the confirmation sheet for the action.
    OpenConfirmation(FileAction),
    /// Refused with a specific reason.
    Refused(OpProblem),
}

/// Evaluates what tapping an action control in the action bar does.
///
/// WHY this is here: the toolbar's visual state and its tap handler must agree.
/// If slot 6 is enabled because `NEW_FOLDER_SUPPORTED` is true, tapping slot 6
/// must open the confirmation sheet rather than refusing.
pub fn evaluate_action_tap(
    action: FileAction,
    selection_count: usize,
    any_dir: bool,
) -> ActionTapOutcome {
    if action == FileAction::NewFolder {
        if !NEW_FOLDER_SUPPORTED {
            return ActionTapOutcome::Refused(OpProblem::UnsupportedByPlatform);
        }
        return ActionTapOutcome::OpenConfirmation(FileAction::NewFolder);
    }
    if selection_count == 0 {
        return ActionTapOutcome::Refused(OpProblem::NoSelection);
    }
    if any_dir {
        return ActionTapOutcome::Refused(OpProblem::FolderNotSupported);
    }
    if action == FileAction::Rename && selection_count != 1 {
        return ActionTapOutcome::Refused(OpProblem::RenameNeedsOne);
    }
    ActionTapOutcome::OpenConfirmation(action)
}

/// Error returned when attempting to create a new folder.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum NewFolderError {
    Problem(OpProblem),
    PlatformErrno(i64),
}

/// Appends a byte slice into an output buffer, returning the new length.
pub fn append(buf: &mut [u8], n: usize, bytes: &[u8]) -> usize {
    let take = bytes.len().min(buf.len().saturating_sub(n));
    buf[n..n + take].copy_from_slice(&bytes[..take]);
    n + take
}

/// Formats a u64 decimal and appends it to an output buffer.
pub fn append_u64(buf: &mut [u8], n: usize, value: u64) -> usize {
    let mut tmp = [0u8; 20];
    let len = fmt_u64(value, &mut tmp);
    append(buf, n, &tmp[..len])
}

/// Checks whether `haystack` contains `needle`.
pub fn contains_subslice(haystack: &[u8], needle: &[u8]) -> bool {
    if needle.is_empty() {
        return true;
    }
    if haystack.len() < needle.len() {
        return false;
    }
    haystack.windows(needle.len()).any(|w| w == needle)
}

/// Performs a new folder creation given a parent directory, typed name, existence check,
/// and mkdir implementation.
pub fn execute_new_folder_op<F>(
    parent: &str,
    name: &[u8],
    target_exists: bool,
    mut mkdir: F,
) -> Result<(), NewFolderError>
where
    F: FnMut(&str) -> Result<(), i64>,
{
    validate_operation(FileAction::NewFolder, 0, false, name, b"", target_exists)
        .map_err(NewFolderError::Problem)?;
    let mut target = [0u8; MAX_PATH_LEN];
    let target_len = join_path(parent, name, &mut target)
        .ok_or(NewFolderError::Problem(OpProblem::NameUnusable))?;
    let target_str = core::str::from_utf8(&target[..target_len])
        .map_err(|_| NewFolderError::Problem(OpProblem::NameUnusable))?;
    mkdir(target_str).map_err(NewFolderError::PlatformErrno)
}

/// Formats the result notice for a new folder operation.
pub fn format_new_folder_notice(
    name: &[u8],
    outcome: Result<(), NewFolderError>,
    out: &mut [u8; LINE_LEN],
) -> usize {
    match outcome {
        Ok(()) => {
            let text = b"New: 1 done, 0 failed";
            let len = text.len().min(LINE_LEN);
            out[..len].copy_from_slice(&text[..len]);
            len
        }
        Err(NewFolderError::Problem(prob)) => {
            let mut len = append(out, 0, b"New: 0 done, 1 failed - first: ");
            let take = name.len().min(out.len().saturating_sub(len + 32));
            len = append(out, len, &name[..take]);
            len = append(out, len, b" (");
            let reason = problem_text(prob);
            let rtake = reason.len().min(out.len().saturating_sub(len + 1));
            len = append(out, len, &reason[..rtake]);
            len = append(out, len, b")");
            len
        }
        Err(NewFolderError::PlatformErrno(code)) => {
            let mut len = append(out, 0, b"New: 0 done, 1 failed - first: ");
            let take = name.len().min(out.len().saturating_sub(len + 32));
            len = append(out, len, &name[..take]);
            len = append(out, len, b" (platform code -");
            len = append_u64(out, len, code.unsigned_abs());
            len = append(out, len, b")");
            len
        }
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

    /// The header band and the browse stack under it are the ladder's answer.
    ///
    /// WHY a test beside the `const` gates: the gates prove the band fits the
    /// rungs it was built on today; this says *what it is built on* -- the band
    /// is the lowest of its occupants' union boxes plus its air, each occupant
    /// reserves the box `row_band` names at its own rung, and the first thing
    /// below the header starts on its foot. Raising a rung moves every one of
    /// these together, and a layout that stopped following the ladder fails here
    /// with the two numbers that disagreed.
    #[test]
    fn the_header_band_follows_the_ladder_it_was_built_on() {
        assert_eq!(
            HEADER_TITLE_ROWS,
            outline_text::row_band(0, theme::type_scale::SECTION).1,
            "the title row must reserve the section rung's whole union box"
        );
        assert_eq!(
            HEADER_READOUT_ROWS,
            outline_text::row_band(0, theme::type_scale::CAPTION).1,
            "a readout line must reserve the caption rung's whole union box"
        );
        assert_eq!(HEADER_TITLE_SCALE, theme::type_scale::SECTION);
        assert_eq!(HEADER_READOUT_SCALE, theme::type_scale::CAPTION);

        let title_foot = HEADER_TITLE_Y + HEADER_TITLE_ROWS;
        let control_foot = back_button::Y + back_button::SIZE;
        let lowest = title_foot.max(control_foot).max(HEADER_READOUT_Y1);
        assert_eq!(
            HEADER_H,
            lowest + HEADER_BOTTOM_AIR,
            "the header band is the lowest row it answers plus its air"
        );

        // Nothing may be clipped by the platform's status band, and nothing may
        // crowd the surface under the header: the band's ink starts at or below
        // the first row the app's chrome may claim and ends, with its whole air,
        // on or before the first row the next band claims.
        assert!(HEADER_TITLE_Y >= viewport::HEADER_TOP);
        assert!(HEADER_READOUT_LINE1_Y >= viewport::HEADER_TOP);
        assert!(lowest + HEADER_BOTTOM_AIR <= BROWSE_STACK_Y);
        assert!(lowest + HEADER_BOTTOM_AIR <= HOME_STORAGE_CARD.y);

        assert!(HEADER_READOUT_LINE1_Y >= HEADER_TITLE_Y);
        assert!(
            HEADER_READOUT_LINE2_Y
                == HEADER_READOUT_LINE1_Y + HEADER_READOUT_ROWS + HEADER_READOUT_GAP
        );
        assert!(HOME_STORAGE_CARD.y == HEADER_H);
        assert!(BROWSE_STACK_Y == HEADER_H);

        assert_eq!(
            BREADCRUMB_ROWS,
            outline_text::row_band(0, theme::type_scale::CAPTION).1,
            "the crumbs must reserve their rung's whole union box"
        );
        assert_eq!(
            BREADCRUMB_HIT_Y1, TOOLBAR_Y,
            "the crumb strip must stop where the toolbar's rows start"
        );
        assert!(BREADCRUMB_HIT_Y1 - BREADCRUMB_HIT_Y0 >= theme::MIN_TOUCH_TARGET);
        assert!(BREADCRUMB_Y >= BREADCRUMB_HIT_Y0);
        assert!(BREADCRUMB_Y + BREADCRUMB_ROWS <= BREADCRUMB_HIT_Y1);
        assert!(
            SEARCH_FIELD.h >= BREADCRUMB_ROWS + spacing::GRID_8,
            "the search field must close over the caption it holds"
        );
    }

    /// Every run the header draws is measured at the rung the header sets it to,
    /// against the lane the painter actually hands it.
    ///
    /// WHY this test exists at all: the header's title is clipped only at its own
    /// left edge and the panel's right edge, so the single thing standing between
    /// a long title and the readout's lane is [`HEADER_TITLE_MAX_W`] -- a bound
    /// `font::measure` cannot check at compile time. The readout's two lines are
    /// trimmed to [`HEADER_READOUT_MAX_W`] by `font::ellipsize`, and the painter
    /// then asks whether the trimmed run fits before drawing it at all, so an
    /// ellipsize that overshot its bound would not overflow the lane, it would
    /// blank the line. Both claims are measurements, and a rung move that breaks
    /// either one has to fail here rather than quietly shrink or drop a run.
    #[test]
    fn every_header_run_fits_the_lane_the_painter_hands_it() {
        // The title, at the rung `header_color` passes to the pixel test.
        assert!(
            font::measure(b"Files", HEADER_TITLE_SCALE) <= HEADER_TITLE_MAX_W,
            "the header's title no longer fits its lane at the section rung"
        );

        // The path line. These are the directories the app opens and offers, at
        // the rung `HEADER_READOUT_SCALE` names -- not the caption rung the painter
        // used to pass and no test could see.
        let paths: [&[u8]; 5] = [
            b"/storage",
            b"/storage/photos",
            b"/storage/pictures",
            b"/storage/videos",
            b"/storage/downloads",
        ];
        for path in paths {
            assert!(
                font::measure(path, HEADER_READOUT_SCALE) <= HEADER_READOUT_MAX_W,
                "a directory the app opens had to be shortened to fit the readout lane"
            );
        }

        // Both readout lines go through `font::ellipsize` at the lane's own width,
        // so no run the painter can be handed -- here the widest each source field
        // can hold, 128 bytes for `current_path` and 40 for `probe_label` -- may
        // come back wider than the lane, or come back empty.
        let widest_path = [b'w'; 128];
        let widest_label = [b'W'; 40];
        let fixtures: [&[u8]; 4] = [
            b"/storage/pictures/vacation-2026/IMG_0442.heic",
            b"the app for this file could not launch",
            &widest_path,
            &widest_label,
        ];
        for run in fixtures {
            let mut buf = [0u8; MAX_PATH_LEN];
            let len = font::ellipsize(run, HEADER_READOUT_SCALE, HEADER_READOUT_MAX_W, &mut buf);
            assert!(
                len > 0,
                "ellipsize returned nothing for a run the readout lane is meant to hold"
            );
            assert!(
                font::measure(&buf[..len], HEADER_READOUT_SCALE) <= HEADER_READOUT_MAX_W,
                "a run trimmed to the readout lane still overflowed it"
            );
        }
    }

    /// Every run this app's text-fit gate speaks of is measured against the
    /// container the painter draws it into, at the rung that draw site inks.
    ///
    /// WHY this replaces the remembered numbers: the gate used to ellipsize a
    /// 72-byte name at a typed rung `2` into `510`, then measure the category
    /// labels against `150` and the sheet's two buttons against `260` at a typed
    /// rung `1`. No row, cell, card or chip on this screen is 510 or 150 wide --
    /// the destination slots the app actually draws those labels into are
    /// `SHEET_DEST_SLOT_W` wide, and the lane a name owns is
    /// [`ENTRY_NAME_LANE`] -- so the measurement checked nothing the user sees:
    /// narrow a slot or raise a rung and the gate kept passing while the word on
    /// screen lost its tail. Each lane below is read from the derived container,
    /// and each run is measured at the rung the matching draw site passes.
    #[test]
    fn test_file_manager_ui_text_bounds_and_ellipsize() {
        use interos_std::design::font;
        let mut buf = [0u8; MAX_PATH_LEN];

        // 1. A file's name on a list row: the row's own lane, the row's own rung.
        let long_name =
            b"A_very_long_directory_name_that_exceeds_normal_breadcrumb_display_limits.bin";
        let len = font::ellipsize(long_name, ENTRY_NAME_SCALE, ENTRY_NAME_LANE, &mut buf);
        assert!(
            len > 0,
            "a name trimmed to the row's lane came back empty -- the row would draw nothing"
        );
        let disp = &buf[..len];
        assert!(font::measure(disp, ENTRY_NAME_SCALE) <= ENTRY_NAME_LANE);
        assert!(
            disp.ends_with(b"...") || disp == long_name.as_slice(),
            "the row's name was cut without an ellipsis -- a silent clip"
        );

        // The longest name the model can hold at all -- `sort_entries` and the
        // parser both cap a name at `MAX_ENTRY_NAME_LEN` -- has to reach the row
        // whole, or be trimmed visibly. A name the app is allowed to hold is not
        // the app's business to shorten behind the user's back.
        let held_name = [b'n'; MAX_ENTRY_NAME_LEN];
        let len = font::ellipsize(&held_name, ENTRY_NAME_SCALE, ENTRY_NAME_LANE, &mut buf);
        assert!(len > 0, "a name the model holds does not fit the row that shows it");
        assert!(font::measure(&buf[..len], ENTRY_NAME_SCALE) <= ENTRY_NAME_LANE);
        assert!(
            buf[..len].ends_with(b"...") || len == held_name.len(),
            "a held name lost bytes on the row without an ellipsis"
        );

        // 2. A file's name in a grid cell: the cell's inset lane at the cell's rung.
        let len = font::ellipsize(&held_name, GRID_NAME_SCALE, GRID_NAME_LANE, &mut buf);
        assert!(
            len > 0,
            "a name trimmed to the cell's lane came back empty -- the cell would draw nothing"
        );
        assert!(font::measure(&buf[..len], GRID_NAME_SCALE) <= GRID_NAME_LANE);
        assert!(
            buf[..len].ends_with(b"...") || len == held_name.len(),
            "a cell's name lost bytes without an ellipsis"
        );

        // 3. The details card: its title at the card's title rung, its longest
        // line at the card's line rung. `fit_details` hands both of these to
        // `font::ellipsize` before the card is drawn, so a refusal here is the
        // card blanking a fact, not a tail going missing.
        let len = font::ellipsize(&held_name, DETAILS_TITLE_SCALE, DETAILS_TITLE_LANE, &mut buf);
        assert!(len > 0, "a document's name does not fit the details card that names it");
        assert!(font::measure(&buf[..len], DETAILS_TITLE_SCALE) <= DETAILS_TITLE_LANE);
        let long_line = [b's'; MAX_DETAILS_LINE];
        let len = font::ellipsize(&long_line, DETAILS_LINE_SCALE, DETAILS_LINE_LANE, &mut buf);
        assert!(
            len > 0,
            "a details line trimmed to the card's lane came back empty -- the card would drop a fact"
        );
        assert!(font::measure(&buf[..len], DETAILS_LINE_SCALE) <= DETAILS_LINE_LANE);

        // 4. The app's own words, which are never trimmed: the four destination
        // slots the copy/move sheet draws the category labels into, and the
        // sheet's two buttons. Measured at [`LABEL_RUNG`] because that is the
        // rung `pill_color` and the sheet's painter ink them at, and against
        // `control_label_lane` of the slot's real width.
        for label in [
            b"Storage".as_slice(),
            b"Photos".as_slice(),
            b"Pictures".as_slice(),
            b"Videos".as_slice(),
        ] {
            assert!(
                font::measure(label, LABEL_RUNG) <= control_label_lane(SHEET_DEST_SLOT_W),
                "a directory the sheet offers no longer fits the slot that offers it"
            );
        }
        for btn in [b"Cancel".as_slice(), b"Confirm".as_slice()] {
            assert!(
                font::measure(btn, LABEL_RUNG) <= control_label_lane(SHEET_BUTTON_W),
                "one of the sheet's two buttons no longer fits its own slot"
            );
        }
        for label in [
            b"Select".as_slice(),
            b"All".as_slice(),
            b"Open".as_slice(),
            b"Copy".as_slice(),
            b"Move".as_slice(),
            b"Rename".as_slice(),
            b"Delete".as_slice(),
            b"New".as_slice(),
        ] {
            assert!(
                font::measure(label, LABEL_RUNG) <= control_label_lane(ACTION_SLOT_W),
                "an action bar label no longer fits the slot the bar gives it"
            );
        }

        // 5. A run the container cannot hold says so. `font::ellipsize` answers
        // zero when the ellipsis itself will not fit, and a painter that ignored
        // that answer would draw nothing at all -- which is how a card ends up
        // blank instead of trimmed. Every lane this app draws into has to be wide
        // enough for the mark that says the run was cut.
        for lane in [
            ENTRY_NAME_LANE,
            GRID_NAME_LANE,
            DETAILS_TITLE_LANE,
            DETAILS_LINE_LANE,
            SCOPE_CAPTION_MAX_W,
            CATEGORY_LABEL_LANE,
            CRUMB_STRIP_W,
        ] {
            assert!(
                font::measure(b"...", ENTRY_SUB_SCALE) <= lane,
                "a lane of {} rows cannot even hold the ellipsis that would mark a cut run",
                lane
            );
        }
    }

    /// The crumb row: the strip the painter lays the crumbs in is the strip the
    /// tap handler answers, and no crumb is drawn past either of its edges.
    ///
    /// WHY a layout function rather than a draw-site width: the painter walked
    /// every crumb at a typed rung while the hit test measured the raw label at
    /// a typed `1`, so the two could only agree by luck. Both now ask
    /// [`crumb_step`], which trims a crumb to the room the strip has left, so a
    /// crumb is tappable exactly where it is inked and a component longer than
    /// the strip ends in an ellipsis instead of running off the band.
    #[test]
    fn the_crumb_row_is_laid_out_inside_the_strip_that_answers_it() {
        use interos_std::design::font;
        let mut crumbs = [Crumb::EMPTY; 6];
        let mut buf = [0u8; MAX_PATH_LEN];

        // The deepest path the app can be pointed at by its own category rows.
        let path = b"/storage/photos/pictures/videos";
        let count = breadcrumb_crumbs(path, &mut crumbs);
        assert!(count > 1, "the fixture path no longer splits into crumbs");

        let mut cursor = CRUMB_ROW_X0;
        let mut whole_row = 0usize;
        for index in 0..count {
            let crumb = crumbs[index];
            let label = &path[crumb.label_start..crumb.label_start + crumb.label_len];
            let step = crumb_step(label, cursor, index + 1 == count, &mut buf)
                .expect("a category crumb the app can navigate to does not fit its own strip");
            let run = &buf[..step.len];
            assert_eq!(step.x, cursor);
            assert!(
                font::measure(run, BREADCRUMB_SCALE) <= CRUMB_ROW_X1 - step.x,
                "a crumb run paints past the right edge of the strip that answers it"
            );
            assert!(
                run == label,
                "a crumb the platform itself names was trimmed -- the strip is too narrow for its own paths"
            );
            assert!(step.len <= MAX_PATH_LEN);
            cursor = step.next_x;
            whole_row = cursor;
        }
        assert!(
            whole_row <= CRUMB_ROW_X1,
            "the crumb row ran past the strip: the last crumb had nowhere to be tapped"
        );

        // A component longer than the whole strip is trimmed, and the row stops
        // rather than laying ink past the edge.
        let deep = b"/storage/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
        let count = breadcrumb_crumbs(deep, &mut crumbs);
        let mut cursor = CRUMB_ROW_X0;
        let mut drawn = 0usize;
        for index in 0..count {
            let crumb = crumbs[index];
            let label = &deep[crumb.label_start..crumb.label_start + crumb.label_len];
            match crumb_step(label, cursor, index + 1 == count, &mut buf) {
                Some(step) => {
                    let run = &buf[..step.len];
                    assert!(
                        font::measure(run, BREADCRUMB_SCALE) <= CRUMB_ROW_X1 - step.x
                    );
                    assert!(
                        run.ends_with(b"...") || run == label,
                        "a crumb was cut without an ellipsis -- a silent clip"
                    );
                    drawn += 1;
                    cursor = step.next_x;
                    assert!(cursor <= CRUMB_ROW_X1 + CRUMB_SEP_GAP);
                }
                None => {
                    assert!(cursor >= CRUMB_ROW_X1 - CRUMB_MIN_ROOM);
                    break;
                }
            }
        }
        assert!(drawn >= 1, "a crumb row with a deep path drew nothing at all");
        assert!(cursor <= CRUMB_ROW_X1);
    }

    /// A run handed to [`fitted_run`] -- the helper every centred label in this
    /// app now goes through -- is either whole or visibly cut, and never wider
    /// than the container that asked for it.
    ///
    /// WHY this is a gate and not a convenience: the painter used to hand
    /// `centered_x` a run of any length. `centered_x` left-aligns what it cannot
    /// centre, so the label simply ran over the control's right edge and the
    /// pixels outside it belonged to whatever was drawn next.
    #[test]
    fn a_fitted_run_is_whole_or_visibly_cut_never_missing() {
        use interos_std::design::font;
        let mut buf = [0u8; MAX_PATH_LEN];

        // A word the container holds is handed back byte for byte.
        let whole = b"Directory Empty";
        assert_eq!(
            fitted_run(whole, EMPTY_HEADLINE_SCALE, EMPTY_CARD_LANE, &mut buf),
            whole.as_slice(),
            "a headline the card holds was altered on its way to the screen"
        );

        // A run the container cannot hold is cut, and says so.
        let long = [b'e'; MAX_PATH_LEN - 1];
        let cut = fitted_run(&long, ENTRY_NAME_SCALE, ENTRY_NAME_LANE, &mut buf);
        assert!(
            font::measure(cut, ENTRY_NAME_SCALE) <= ENTRY_NAME_LANE,
            "a run cut to the row's lane still overflowed it"
        );
        assert!(
            cut.ends_with(b"..."),
            "a run cut to a lane lost its ellipsis -- the silent clip this helper exists to refuse"
        );

        // The one case the helper hands back less than the whole run without a
        // mark is the case where the ellipsis itself would not fit, so no
        // container this app draws into may be narrower than the mark that says a
        // run was cut -- and the mark is weighed in the rung its own label inks
        // at, at every rung the person can ask for, not at the caption's.
        for (lane, scale) in [
            (ENTRY_NAME_LANE, ENTRY_NAME_SCALE),
            (GRID_NAME_LANE, GRID_NAME_SCALE),
            (EMPTY_CARD_LANE, EMPTY_HEADLINE_SCALE),
            (STATUS_LANE, STATUS_SCALE),
            (CATEGORY_LABEL_LANE, CATEGORY_LABEL_SCALE),
            (SEARCH_TEXT_LANE, SEARCH_TEXT_SCALE),
            (SCOPE_CAPTION_MAX_W, SCOPE_CAPTION_SCALE),
            (control_label_lane(ACTION_SLOT_W), LABEL_RUNG),
            (control_label_lane(SHEET_DEST_SLOT_W), LABEL_RUNG),
            (control_label_lane(SHEET_BUTTON_W), LABEL_RUNG),
        ] {
            let mut level = 0;
            while level <= MAX_LEVEL {
                assert!(
                    run_w_at(b"...", scale, level) <= lane,
                    "a lane of {} px cannot hold the ellipsis its own label inks at rung {}",
                    lane,
                    level
                );
                level += 1;
            }
        }
        // The browse control's lane is the one fitted label whose box the ladder
        // resizes, so it is swept with the rung rather than at the authored width.
        let mut level = 0;
        while level <= MAX_LEVEL {
            let lane = control_label_lane(home_browse_btn_at(level).w);
            assert!(
                run_w_at(b"...", LABEL_RUNG, level) <= lane,
                "the browse control's lane of {} px cannot hold its own ellipsis at rung {}",
                lane,
                level
            );
            level += 1;
        }
    }

    /// A lane narrower than the ellipsis is cut hard: what the container gets is
    /// the leading glyphs it can hold, never the whole run it cannot.
    ///
    /// WHY this is a test of the helper rather than of a lane: no lane this app
    /// draws into is that narrow -- the sweep above refuses the day one is -- but
    /// the fallback is the part of `fitted_run` that used to answer a refusal by
    /// handing the painter the unbounded run, and a contract that only holds while
    /// its precondition happens to be true is not the contract the centred labels
    /// are drawn under.
    #[test]
    fn a_lane_narrower_than_the_mark_is_cut_to_it_rather_than_spilled() {
        let long = [b'e'; MAX_PATH_LEN - 1];
        let lvl = draw_level();
        for lane in [0usize, 1, 4, 8, 12] {
            let mut buf = [0u8; MAX_PATH_LEN];
            let cut = fitted_run(&long, ENTRY_NAME_SCALE, lane, &mut buf);
            assert!(
                run_w_at(cut, ENTRY_NAME_SCALE, lvl) <= lane,
                "a run cut to a {}px lane still overflowed it by {}px",
                lane,
                run_w_at(cut, ENTRY_NAME_SCALE, lvl).saturating_sub(lane)
            );
        }
    }

    /// The caller's buffer is a bound the cut run keeps to, whatever the lane: a
    /// run trimmed into four bytes is four bytes of run, not the mark's three
    /// written past the end of what the painter was handed.
    ///
    /// WHY the buffer is swept as a slice rather than left to the arrays this file
    /// declares: a lane here is 596 rows wide while the run beside it may hold 191
    /// bytes, and a narrow run is out of buffer long before it is out of lane. That
    /// is the branch the platform's own trim took one glyph past, and the mark's
    /// copy then wrote past the slice's end -- a panic in the painter, not a pixel
    /// in the wrong place.
    ///
    /// WHY the bytes themselves are pinned beside the length: a refusal is the one
    /// answer `ellipsize_run` gives without writing, so a cut read back from a
    /// buffer measured only by its length can be all the zeros it was zeroed to --
    /// the label gone, and no width or length assertion the wiser.
    #[test]
    fn a_short_buffer_is_filled_to_its_end_and_never_past_it() {
        let long = [b'e'; MAX_PATH_LEN - 1];
        for room in [0usize, 1, 2, 3, 4, 5, 8, 16] {
            let mut backing = [0u8; MAX_PATH_LEN];
            let cut = fitted_run(
                &long,
                ENTRY_NAME_SCALE,
                ENTRY_NAME_LANE,
                &mut backing[..room],
            );
            assert!(
                cut.len() <= room,
                "a run cut for a {}-byte buffer came back {} bytes long",
                room,
                cut.len()
            );
            let body = cut.strip_suffix(b"...").unwrap_or(cut);
            assert!(
                long.starts_with(body),
                "a {}-byte buffer was handed bytes its label never had",
                room
            );
            assert!(
                room < 1 || !cut.is_empty(),
                "a lane this wide lost a glyph into a {}-byte buffer",
                room
            );
        }
    }

    /// The details card's own fit, reached through a plate too small for the mark:
    /// the slot comes back with the leading glyphs its lane holds, never with the
    /// label its card refused.
    ///
    /// WHY the plate and not the lane carries the refusal here: the card's lanes
    /// are 600px and no line the model builds can overflow the mark's own three
    /// bytes, so the only honest way to make the details path answer a refusal is
    /// the way it really happens in the trim -- `out_buf.len() < 3`. A 1- and a
    /// 2-byte plate are that branch, and the slot is what the painter then inks.
    ///
    /// WHY this is a test of the model rather than of `fitted_run` again: the
    /// defect was never the helper's fallback, it was a caller that read the
    /// trim's `0` as "keep what you had" and left the whole label in the slot.
    /// That caller cannot be reached from the binary at all -- the app is built
    /// with `test = false` -- so the rule lives or dies on these two lines.
    #[test]
    fn a_refused_details_slot_keeps_a_cut_and_never_its_label() {
        let lvl = draw_level();
        let label = [b'W'; 64];
        assert!(
            run_w_at(&label, DETAILS_TITLE_SCALE, lvl) > DETAILS_TITLE_LANE,
            "this fixture's label fits the card's lane, so its plate would never be refused"
        );
        for room in [1usize, 2] {
            let mut slot = [0u8; 64];
            slot.copy_from_slice(&label);
            let mut len = label.len();
            let mut backing = [0u8; 2];
            fit_details_slot(
                &mut slot,
                &mut len,
                DETAILS_TITLE_SCALE,
                DETAILS_TITLE_LANE,
                &mut backing[..room],
            );
            let kept = &slot[..len];
            assert!(!kept.is_empty(), "a {}-byte plate lost the run whole", room);
            assert!(
                kept.len() < label.len(),
                "a {}-byte plate left the details slot its whole label",
                room
            );
            assert!(
                label.starts_with(kept),
                "the details slot came back with bytes its label never had"
            );
            assert!(
                run_w_at(kept, DETAILS_TITLE_SCALE, lvl) <= DETAILS_TITLE_LANE,
                "the run the card kept overflows its own lane by {}px",
                run_w_at(kept, DETAILS_TITLE_SCALE, lvl).saturating_sub(DETAILS_TITLE_LANE)
            );
        }
    }

    /// The card's fit adopts an answer for every slot it owns -- title and lines
    /// alike -- and a run the lane holds is handed on untouched, so the only thing
    /// a slot may lose is the ink the card could not draw.
    #[test]
    fn the_details_fit_holds_a_short_run_and_cuts_an_over_wide_one() {
        let lvl = draw_level();
        let mut view = DetailsView::EMPTY;
        let short = b"notes.txt";
        view.title[..short.len()].copy_from_slice(short);
        view.title_len = short.len();
        let long = [b'W'; MAX_DETAILS_LINE];
        assert!(
            run_w_at(&long, DETAILS_LINE_SCALE, lvl) > DETAILS_LINE_LANE,
            "this fixture's line fits the card's lane, so the fit would have nothing to cut"
        );
        view.lines[0].copy_from_slice(&long);
        view.line_lens[0] = long.len();
        view.line_count = 1;

        fit_details(&mut view);

        assert_eq!(
            &view.title[..view.title_len],
            short.as_slice(),
            "a title the card holds was cut on its way to the screen"
        );
        let kept = &view.lines[0][..view.line_lens[0]];
        assert!(
            kept.len() < long.len(),
            "the card kept a line whole that overflows its own lane by {}px",
            run_w_at(kept, DETAILS_LINE_SCALE, lvl).saturating_sub(DETAILS_LINE_LANE)
        );
        assert!(
            long.starts_with(kept.strip_suffix(b"...").unwrap_or(kept)),
            "a cut details line came back with bytes its label never had"
        );
        assert!(
            run_w_at(kept, DETAILS_LINE_SCALE, lvl) <= DETAILS_LINE_LANE,
            "a cut details line still overflowed the card's lane"
        );
    }

    /// A search row's two caption lines both stand inside the plate the row is
    /// drawn, scrolled and tapped at -- at every rung the ladder offers.
    ///
    /// WHY measured from the plate and not the buffer: `draw_buffer_text` clips a
    /// run to the buffer it is handed, and the buffer is `entry_row_h_max()` rows
    /// deep while the row's *rect* is `entry_row_h()` tall. A plate shorter than
    /// the ink stacked inside it is therefore not a taller row with a trimmed
    /// bottom -- it is a row whose lower lines are quietly never painted, and the
    /// hit test still says the row is only that tall.
    #[test]
    fn the_two_caption_lines_of_a_search_row_stand_inside_its_plate() {
        for level in 0..=MAX_LEVEL {
            let path_bottom = entry_search_ink_bottom_at(level);
            assert!(
                path_bottom <= entry_row_h_at(level),
                "rung {} stacks its found-in line to row {}, but the row's plate ends at {}",
                level,
                path_bottom,
                entry_row_h_at(level)
            );
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
    fn a_new_folder_is_supported_by_the_platform() {
        assert!(NEW_FOLDER_SUPPORTED);
        assert_eq!(
            validate_operation(FileAction::NewFolder, 0, false, b"notes", b"", false),
            Ok(())
        );
        assert!(action_supported(FileAction::NewFolder, false));
        // The refusal text for an unsupported operation does not claim a missing syscall.
        assert_eq!(
            problem_text(OpProblem::UnsupportedByPlatform),
            b"NOT IMPLEMENTED: the platform does not support this operation"
        );
        // Genuine refusals: unusable name (e.g. empty or containing slash) or taken name.
        assert_eq!(
            validate_operation(FileAction::NewFolder, 0, false, b"", b"", false),
            Err(OpProblem::NameUnusable)
        );
        assert_eq!(
            validate_operation(FileAction::NewFolder, 0, false, b"invalid/slash", b"", false),
            Err(OpProblem::NameUnusable)
        );
        assert_eq!(
            problem_text(OpProblem::NameUnusable),
            b"that name cannot be stored"
        );
        assert_eq!(
            validate_operation(FileAction::NewFolder, 0, false, b"notes", b"", true),
            Err(OpProblem::NameTaken)
        );
        assert_eq!(
            problem_text(OpProblem::NameTaken),
            b"that name is already taken"
        );
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

    #[test]
    fn new_folder_enabled_flag_and_tap_handler_agree() {
        let enabled = action_supported(FileAction::NewFolder, false);
        assert!(enabled, "toolbar control must be enabled when NEW_FOLDER_SUPPORTED is true");

        // The tap handler must agree with the enabled flag:
        // When enabled is true, tapping slot 6 must open the confirmation sheet,
        // rather than refusing with UnsupportedByPlatform.
        let tap = evaluate_action_tap(FileAction::NewFolder, 0, false);
        assert_eq!(
            tap,
            ActionTapOutcome::OpenConfirmation(FileAction::NewFolder),
            "tap handler must agree with enabled flag and open confirmation"
        );
    }

    #[test]
    fn new_folder_success_produces_real_directory_in_listing() {
        let mut entries = [RuntimeFileEntry::EMPTY; MAX_PARSED_ENTRIES];
        let initial_raw = b"photos|0|dir\nreport.txt|1024|file\n";
        let count = parse_vfs_entries(initial_raw, &mut entries);
        assert_eq!(count, 2);

        // When a new folder is created successfully:
        let mut created_path = [0u8; MAX_PATH_LEN];
        let mut created_len = 0usize;
        let outcome = execute_new_folder_op(
            "/storage",
            b"documents",
            false,
            |target| {
                let bytes = target.as_bytes();
                created_path[..bytes.len()].copy_from_slice(bytes);
                created_len = bytes.len();
                Ok(())
            },
        );
        assert_eq!(outcome, Ok(()));
        assert_eq!(&created_path[..created_len], b"/storage/documents");

        // Success produces a real directory visible in the VFS listing
        let updated_raw = b"documents|0|dir\nphotos|0|dir\nreport.txt|1024|file\n";
        let mut updated_entries = [RuntimeFileEntry::EMPTY; MAX_PARSED_ENTRIES];
        let updated_count = parse_vfs_entries(updated_raw, &mut updated_entries);
        assert_eq!(updated_count, 3);
        assert_eq!(&updated_entries[0].name[..updated_entries[0].name_len], b"documents");
        assert!(updated_entries[0].is_dir);
        assert_eq!(&updated_entries[0].size_label[..updated_entries[0].size_label_len], b"DIR");
    }

    #[test]
    fn new_folder_genuine_failures_surface_specific_reasons_and_never_unsupported() {
        let mut notice_buf = [0u8; LINE_LEN];

        // 1. Existing name (target_exists = true)
        let err_existing = execute_new_folder_op(
            "/storage",
            b"photos",
            true,
            |_| Ok(()),
        );
        assert_eq!(err_existing, Err(NewFolderError::Problem(OpProblem::NameTaken)));
        let len = format_new_folder_notice(b"photos", err_existing, &mut notice_buf);
        let notice = &notice_buf[..len];
        assert_eq!(notice, b"New: 0 done, 1 failed - first: photos (that name is already taken)");
        assert!(!contains_subslice(notice, b"platform does not support"));

        // 2. Bad name (empty or invalid character)
        let err_bad_empty = execute_new_folder_op(
            "/storage",
            b"",
            false,
            |_| Ok(()),
        );
        assert_eq!(err_bad_empty, Err(NewFolderError::Problem(OpProblem::NameUnusable)));
        let len = format_new_folder_notice(b"", err_bad_empty, &mut notice_buf);
        let notice = &notice_buf[..len];
        assert_eq!(notice, b"New: 0 done, 1 failed - first:  (that name cannot be stored)");
        assert!(!contains_subslice(notice, b"platform does not support"));

        let err_bad_slash = execute_new_folder_op(
            "/storage",
            b"sub/dir",
            false,
            |_| Ok(()),
        );
        assert_eq!(err_bad_slash, Err(NewFolderError::Problem(OpProblem::NameUnusable)));
        let len = format_new_folder_notice(b"sub/dir", err_bad_slash, &mut notice_buf);
        let notice = &notice_buf[..len];
        assert_eq!(notice, b"New: 0 done, 1 failed - first: sub/dir (that name cannot be stored)");
        assert!(!contains_subslice(notice, b"platform does not support"));

        // 3. Denied capability (kernel returns permission denied errno -13)
        let err_denied = execute_new_folder_op(
            "/storage",
            b"restricted",
            false,
            |_| Err(-13),
        );
        assert_eq!(err_denied, Err(NewFolderError::PlatformErrno(-13)));
        let len = format_new_folder_notice(b"restricted", err_denied, &mut notice_buf);
        let notice = &notice_buf[..len];
        assert_eq!(notice, b"New: 0 done, 1 failed - first: restricted (platform code -13)");
        assert!(!contains_subslice(notice, b"platform does not support"));
    }
}
