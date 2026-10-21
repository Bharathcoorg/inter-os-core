#![no_std]
//! InterOS File Manager VFS parser and data models.

extern crate interos_std;

/// The app's view of the platform's design system.
pub mod theme;

pub const MAX_PARSED_ENTRIES: usize = 8;
pub const MAX_ENTRY_NAME_LEN: usize = 32;

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

pub const VIDEO_SUFFIXES: [&[u8]; 5] = [
    b".mp4", b".webm", b".mkv", b".avi", b".mov",
];

pub const AUDIO_SUFFIXES: [&[u8]; 6] = [
    b".mp3", b".wav", b".ogg", b".flac", b".aac", b".m4a",
];

pub const DOC_SUFFIXES: [&[u8]; 7] = [
    b".txt", b".pdf", b".md", b".doc", b".json", b".log", b".csv",
];

pub fn has_suffix(name: &[u8], suffix: &[u8]) -> bool {
    name.len() > suffix.len() && name[name.len() - suffix.len()..].eq_ignore_ascii_case(suffix)
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
    /// Where the media type answer came from: `bytes`, `extension` or
    /// `unknown`.
    pub provenance: [u8; 16],
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
        provenance: [0u8; 16],
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
        let mime_len = record.mime.len().min(MAX_DOCUMENT_MIME_LEN);
        selected.mime[..mime_len].copy_from_slice(&record.mime.as_bytes()[..mime_len]);
        selected.mime_len = mime_len;
        let provenance_len = record.provenance.len().min(16);
        selected.provenance[..provenance_len]
            .copy_from_slice(&record.provenance.as_bytes()[..provenance_len]);
        selected.provenance_len = provenance_len;
        selected.size = record.length;
        let mut mime_buf = [0u8; MAX_DOCUMENT_MIME_LEN];
        mime_buf[..mime_len].copy_from_slice(&record.mime.as_bytes()[..mime_len]);
        let mime = core::str::from_utf8(&mime_buf[..mime_len]).unwrap_or("");
        selected.handler_count = handlers_for_mime(mime, &mut handler_raw, &mut selected.handlers);
        return Some(selected);
    }
    None
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

    #[test]
    fn test_detect_mime() {
        assert_eq!(detect_mime(b"folder", true), FileMime::Directory);
        assert_eq!(detect_mime(b"photo.jpg", false), FileMime::Image);
        assert_eq!(detect_mime(b"screenshot.ppm", false), FileMime::Image);
        assert_eq!(detect_mime(b"clip.mp4", false), FileMime::Video);
        assert_eq!(detect_mime(b"song.mp3", false), FileMime::Audio);
        assert_eq!(detect_mime(b"notes.txt", false), FileMime::Document);
        assert_eq!(detect_mime(b"archive.tar", false), FileMime::Other);
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
}