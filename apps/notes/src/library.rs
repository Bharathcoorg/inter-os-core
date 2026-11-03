// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

//! The notes' documents in the shared storage area.
//!
//! WHY this exists: the note store lived in the app's own sandbox at
//! `data/notes.bin`, and a sandbox is private. No other app could list a
//! note, the platform's document index did not know that notes existed and
//! the launcher's search could not find one. Publishing each note as a
//! document in the shared storage area makes the notes visible to the rest
//! of the device, which is what the design document asks for.

use interos_std as std;

use crate::Note;

/// The shared storage area the platform's index walks.
pub const STORAGE_ROOT: &str = "/storage";

/// The prefix every published note's document carries.
pub const DOC_PREFIX: &[u8] = b"note-";

/// The suffix every published note's document carries.
pub const DOC_SUFFIX: &[u8] = b".note";

/// The media type the platform's index gives a published note.
///
/// WHY the notes have a media type of their own: the intent registry
/// answers "which app takes this document", and a note is not a plain text
/// document. The type is declared in the app's manifest, so the registry
/// and the app agree about it.
pub const NOTE_MIME: &str = "text/x-interos-note";

/// The size of a published document.
///
/// WHY a fixed size: the platform's write replaces the bytes at the file's
/// offset and does not truncate, so a document that changed length would
/// keep the tail of its previous contents. Every write covers the whole
/// file, so the document never carries a stale tail.
pub const DOC_BYTES: usize = 1024;

/// The longest path a published document may have.
pub const PATH_CAP: usize = 64;

/// What publishing a note's document did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PublishOutcome {
    /// The document was written.
    Written,
    /// The platform refused the write of the document.
    WriteRefused,
}

/// Writes the path a note's document is published at into `out` and returns
/// its length.
///
/// WHY the title is in the name: the launcher's search matches a person's
/// query against the name of the file, so a document called "note-Groceries.note"
/// is found by the word "groceries" and a document called "note-7.note" is
/// found by nothing.
pub fn document_path(title: &[u8], out: &mut [u8; PATH_CAP]) -> usize {
    let mut at = 0usize;
    for &byte in STORAGE_ROOT.as_bytes() {
        if at >= out.len() {
            return at;
        }
        out[at] = byte;
        at += 1;
    }
    if at < out.len() {
        out[at] = b'/';
        at += 1;
    }
    for &byte in DOC_PREFIX {
        if at >= out.len() {
            return at;
        }
        out[at] = byte;
        at += 1;
    }
    // WHY the title is sanitised: a title may hold a slash or a space, and
    // a path with either in it is a path that names a file nobody can open.
    // A title that sanitises to nothing falls back to the note's own id.
    let mut wrote_title = false;
    for &byte in title {
        if at >= out.len() {
            return at;
        }
        let safe = match byte {
            b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'-' | b'_' => byte,
            _ => b'-',
        };
        out[at] = safe;
        at += 1;
        wrote_title = true;
    }
    if !wrote_title {
        for &byte in b"untitled" {
            if at >= out.len() {
                return at;
            }
            out[at] = byte;
            at += 1;
        }
    }
    for &byte in DOC_SUFFIX {
        if at >= out.len() {
            return at;
        }
        out[at] = byte;
        at += 1;
    }
    at
}

/// Writes a note's document into `out` and returns its length.
pub fn render_document(note: &Note, out: &mut [u8; DOC_BYTES]) -> usize {
    let mut at = 0usize;
    let mut push = |bytes: &[u8], at: &mut usize| {
        for &byte in bytes {
            if *at >= out.len() {
                return;
            }
            out[*at] = byte;
            *at += 1;
        }
    };
    push(b"Title: ", &mut at);
    push(note.title_bytes(), &mut at);
    push(b"\n\n", &mut at);
    push(note.body_bytes(), &mut at);
    push(b"\n", &mut at);
    while at < out.len() {
        out[at] = b' ';
        at += 1;
    }
    at
}

/// Writes one note's document into the shared storage area.
///
/// Returns what the platform did, never what was hoped for.
pub fn publish_note(note: &Note) -> PublishOutcome {
    let mut path = [0u8; PATH_CAP];
    let path_len = document_path(note.title_bytes(), &mut path);
    let Ok(path_str) = core::str::from_utf8(&path[..path_len]) else {
        return PublishOutcome::WriteRefused;
    };
    let mut document = [0u8; DOC_BYTES];
    render_document(note, &mut document);
    let fd = match std::fs_open(path_str, std::FS_WRITE) {
        Ok(fd) => fd,
        Err(_) => return PublishOutcome::WriteRefused,
    };
    let written = std::fs_write(fd, &document[..]);
    let _ = std::fs_fsync(fd);
    let _ = std::fs_close(fd);
    if written.is_err() {
        return PublishOutcome::WriteRefused;
    }
    PublishOutcome::Written
}

/// Asks the platform to describe the shared storage area again.
///
/// WHY the refresh is asked for explicitly: the platform's index is rebuilt
/// when the storage generation moves, and the query that reads it does
/// that on its own. The refresh is asked for so the notes are described
/// before the app reports that they were published.
pub fn refresh_index() -> bool {
    std::content::content_index_refresh().is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::NotesStore;

    fn note_with(title: &str, body: &str) -> Note {
        let mut store = NotesStore::new();
        let id = store.create_note(title, body).expect("the store holds a note");
        store.get_note(id).expect("the note is there").clone()
    }

    #[test]
    fn a_published_note_is_named_after_its_title() {
        // WHY this matters: the launcher's search matches the person's
        // query against the name of the file, so a document named after the
        // note is findable and one named after its index is not.
        let mut path = [0u8; PATH_CAP];
        let n = document_path(b"Groceries", &mut path);
        let text = core::str::from_utf8(&path[..n]).expect("the path is text");
        assert_eq!(text, "/storage/note-Groceries.note");
    }

    #[test]
    fn a_published_path_holds_no_character_that_would_break_it() {
        // WHY this matters: a title may hold a slash or a space, and a path
        // with either in it names a file nobody can open.
        let mut path = [0u8; PATH_CAP];
        let n = document_path(b"a b/c", &mut path);
        let text = core::str::from_utf8(&path[..n]).expect("the path is text");
        assert!(!text.contains(' '));
        assert!(!text[9..].contains('/'));
        assert_eq!(text, "/storage/note-a-b-c.note");
    }

    #[test]
    fn a_note_without_a_title_still_gets_a_document_name() {
        // WHY this matters: a note with no title is a note the user wrote
        // and did not name, and it still has to be findable.
        let mut path = [0u8; PATH_CAP];
        let n = document_path(b"", &mut path);
        let text = core::str::from_utf8(&path[..n]).expect("the path is text");
        assert_eq!(text, "/storage/note-untitled.note");
    }

    #[test]
    fn a_document_carries_the_title_and_the_body() {
        let note = note_with("Groceries", "milk\nbread");
        let mut out = [0u8; DOC_BYTES];
        let n = render_document(&note, &mut out);
        assert_eq!(n, DOC_BYTES);
        let text = core::str::from_utf8(&out).expect("the document is text");
        assert!(text.contains("Title: Groceries"));
        assert!(text.contains("milk"));
        assert!(text.contains("bread"));
    }

    #[test]
    fn a_document_is_always_the_same_length() {
        // WHY: the platform's write does not truncate, so a document that
        // changed length would keep the tail of its previous contents.
        let short = note_with("A", "b");
        let long = note_with("A much longer title", "a much longer body");
        let mut a = [0u8; DOC_BYTES];
        let mut b = [0u8; DOC_BYTES];
        assert_eq!(render_document(&short, &mut a), render_document(&long, &mut b));
    }
}
