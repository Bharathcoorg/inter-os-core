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

/// The search field: the app's one text input.
pub const SEARCH_FIELD: Rect = Rect::new(36, 146, 648, 56);
/// The clear action inside the search field.
pub const SEARCH_CLEAR: Rect = Rect::new(612, 154, 48, 40);
/// The scope caption under the search field. It says what the search looks
/// at, so the reader never has to guess whether it is one folder or the
/// whole device.
pub const SCOPE_Y: usize = 208;

/// The category rows.
pub const CATEGORY_Y0: usize = 236;
/// The first category row is the storage row, which carries a caption under
/// its label, so it is taller than the three that follow it.
pub const CATEGORY_FIRST_ROW_H: usize = 100;
pub const CATEGORY_OTHER_ROW_H: usize = 70;
pub const CATEGORY_COUNT: usize = 4;
pub const CATEGORY_Y1: usize =
    CATEGORY_Y0 + CATEGORY_FIRST_ROW_H + (CATEGORY_COUNT - 1) * CATEGORY_OTHER_ROW_H;

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

/// The panel that holds the selected document's actions.
pub const ACTIONS_Y0: usize = 546;
pub const ACTIONS_Y1: usize = 830;

/// The entries card.
///
/// WHY the card is the region and not the size of the list: the card used to
/// be as tall as the list it held, so a long directory grew past the bottom
/// of the screen and the rows below the edge could never be reached. The card
/// is now the region it lives in, and the list scrolls inside it.
pub const ENTRIES_CARD: Rect = Rect::new(36, 836, 648, 448);
/// The height of one entry row.
pub const ENTRY_ROW_H: usize = 56;

/// The status line under the card.
pub const STATUS_Y: usize = 1290;

/// The action bar.
pub const ACTION_BAR_Y0: usize = 1320;
pub const ACTION_BAR_Y1: usize = 1368;
/// The width of one action slot, and the gap between two slots.
pub const ACTION_SLOT_W: usize = 120;
pub const ACTION_SLOT_GAP: usize = 12;

/// The confirmation sheet, drawn over the list.
pub const SHEET: Rect = Rect::new(36, 900, 648, 300);
/// The destination slots in the copy and move sheet.
pub const SHEET_DEST_Y: usize = 1060;
pub const SHEET_DEST_H: usize = 44;
/// The rename sheet's name field.
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

/// The action bar's slot rectangle.
pub fn action_slot_rect(index: usize) -> Rect {
    let x = 36 + index * (ACTION_SLOT_W + ACTION_SLOT_GAP);
    Rect::new(x, ACTION_BAR_Y0, ACTION_SLOT_W, ACTION_BAR_Y1 - ACTION_BAR_Y0)
}

/// The confirmation sheet's destination slot.
pub fn sheet_destination_rect(index: usize) -> Rect {
    let w = 150usize;
    let gap = 16usize;
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
}

/// The labels the action bar draws.
pub const ACTION_LABELS: [&[u8]; 4] = [b"Copy", b"Move", b"Delete", b"Rename"];

/// The action a slot of the action bar runs, or `None` for the select slot.
pub fn action_for_slot(slot: usize) -> Option<FileAction> {
    match slot {
        1 => Some(FileAction::Copy),
        2 => Some(FileAction::Move),
        3 => Some(FileAction::Delete),
        4 => Some(FileAction::Rename),
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
        assert_eq!(entry_at_point(ENTRIES_CARD, off, 10, 360, 800), None);
        assert_eq!(entry_at_point(ENTRIES_CARD, off, 10, 360, 1300), None);
        // An empty list has no rows at all.
        assert_eq!(entry_at_point(ENTRIES_CARD, off, 0, 360, 900), None);
    }

    #[test]
    fn the_card_is_the_region_and_a_long_list_scrolls_inside_it() {
        // The card is the region: it does not grow with the list.
        assert_eq!(ENTRIES_CARD.y, 836);
        assert_eq!(ENTRIES_CARD.h, 448);
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
}