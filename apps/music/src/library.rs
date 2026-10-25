//! The library's documents in the shared storage area, and the import that
//! reads the audio files the device holds.
//!
//! WHY this exists: the library was a record in the app's own settings
//! store, so the file manager could not list it, the launcher's search could
//! not find it and the platform's document index did not know it existed.
//! Publishing the library as documents in the shared storage area makes it
//! visible to the rest of the device, and importing from the same area lets
//! the person bring the audio files they already have into the player.

use interos_std as std;

use crate::{StoreState, TrackStore, TITLE_MAX};

/// The shared storage area the platform's index walks.
pub const STORAGE_ROOT: &str = "/storage";

/// The prefix every published track document carries.
pub const DOC_PREFIX: &[u8] = b"music-";

/// The suffix every published track document carries.
pub const DOC_SUFFIX: &[u8] = b".txt";

/// The size of a published document.
///
/// WHY a fixed size: the platform's write replaces the bytes at the file's
/// offset and does not truncate, so a document that changed length would
/// keep the tail of its previous contents. Every write covers the whole
/// file, so the document never carries a stale tail.
pub const DOC_BYTES: usize = 128;

/// The longest path a published document may have.
pub const PATH_CAP: usize = 64;

/// The file extensions the importer treats as audio.
pub const AUDIO_EXTENSIONS: [&[u8]; 7] = [
    b"mp3", b"wav", b"flac", b"ogg", b"m4a", b"aac", b"opus",
];

/// What publishing the library did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PublishOutcome {
    /// Every track was written and the platform's index was refreshed.
    Published(usize),
    /// Some tracks were written and the platform refused the index refresh.
    IndexRefused(usize),
    /// The platform refused the write of a document.
    WriteRefused,
}

/// What the import did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImportOutcome {
    /// `n` tracks were imported from the shared storage.
    Imported(usize),
    /// `n` tracks were imported and `refused` audio files were found but not
    /// persisted.
    PartiallyImported(usize, usize),
    /// `found` audio files were listed but the device refused to persist any
    /// of them.
    WriteRefused(usize),
    /// The device refused to list the shared storage.
    ListRefused,
    /// The listing held no audio files the library did not already hold.
    NoneFound,
}

/// Whether a file name is one the importer treats as audio.
pub fn is_audio_name(name: &[u8]) -> bool {
    for ext in AUDIO_EXTENSIONS {
        if name.len() > ext.len() && name[name.len() - ext.len()..].eq_ignore_ascii_case(ext) {
            // WHY the dot is checked: a file called "flac" with no extension
            // is a file called flac, and a file called "myflac" is not an
            // audio file with an extension.
            if name[name.len() - ext.len() - 1] == b'.' {
                return true;
            }
        }
    }
    false
}

/// Writes the track title a file name carries into `out` and returns its
/// length.
///
/// WHY the extension is dropped: the name of the file is the only title the
/// device has, and "song.mp3" is a file name, not a title.
pub fn title_from_name(name: &[u8], out: &mut [u8; TITLE_MAX]) -> usize {
    let mut stem = name;
    for ext in AUDIO_EXTENSIONS {
        if stem.len() > ext.len()
            && stem[stem.len() - ext.len()..].eq_ignore_ascii_case(ext)
            && stem[stem.len() - ext.len() - 1] == b'.'
        {
            stem = &stem[..stem.len() - ext.len() - 1];
            break;
        }
    }
    let n = stem.len().min(TITLE_MAX);
    out[..n].copy_from_slice(&stem[..n]);
    n
}

/// Writes the path a track's document is published at into `out` and returns
/// its length.
///
/// WHY the title is in the name: the launcher's search matches a person's
/// query against the name of the file, so a document called
/// "music-Nocturne.txt" is found by the word "nocturne" and a document
/// called "track-3.txt" is found by nothing.
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
    for &byte in title {
        if at >= out.len() {
            return at;
        }
        // WHY the character is replaced rather than kept: a title may hold
        // a slash or a space, and a path with either in it is a path that
        // names a file nobody can open.
        let safe = match byte {
            b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'-' | b'_' => byte,
            _ => b'-',
        };
        out[at] = safe;
        at += 1;
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

/// Writes a track's document into `out` and returns its length.
pub fn render_document(
    title: &[u8],
    duration_known: bool,
    duration_sec: u32,
    out: &mut [u8; DOC_BYTES],
) -> usize {
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
    push(b"Track: ", &mut at);
    push(title, &mut at);
    push(b"\n", &mut at);
    if duration_known {
        push(b"Duration: ", &mut at);
        let mut digits = [0u8; 10];
        let mut n = 0usize;
        let mut value = duration_sec;
        loop {
            digits[n] = b'0' + (value % 10) as u8;
            value /= 10;
            n += 1;
            if value == 0 {
                break;
            }
        }
        for i in 0..n {
            push(&[digits[n - 1 - i]], &mut at);
        }
        push(b" seconds\n", &mut at);
    } else {
        push(b"Duration: not read\n", &mut at);
    }
    push(b"Published by the InterOS music player.\n", &mut at);
    while at < out.len() {
        out[at] = b' ';
        at += 1;
    }
    at
}

/// Publishes every track in the library and refreshes the platform's index.
///
/// Returns how many documents were written and whether the index took the
/// refresh.
pub fn publish_library(store: &TrackStore) -> PublishOutcome {
    let mut written = 0usize;
    for i in 0..store.len() {
        let Some(track) = store.get(i) else {
            continue;
        };
        let title = track.title();
        let mut path = [0u8; PATH_CAP];
        let path_len = document_path(title, &mut path);
        let Ok(path_str) = core::str::from_utf8(&path[..path_len]) else {
            return PublishOutcome::WriteRefused;
        };
        let mut document = [0u8; DOC_BYTES];
        render_document(
            title,
            track.duration_known(),
            track.duration_sec(),
            &mut document,
        );
        let fd = match std::fs_open(path_str, std::FS_WRITE) {
            Ok(fd) => fd,
            Err(_) => return PublishOutcome::WriteRefused,
        };
        let result = std::fs_write(fd, &document[..]);
        let _ = std::fs_fsync(fd);
        let _ = std::fs_close(fd);
        if result.is_err() {
            return PublishOutcome::WriteRefused;
        }
        written += 1;
    }
    if written == 0 {
        return PublishOutcome::WriteRefused;
    }
    // WHY the refresh: the platform's index is rebuilt when the storage
    // generation moves, and the query that reads it does that on its own.
    // The refresh is asked for explicitly so the documents are described
    // before the app reports that they were published.
    match std::content::content_index_refresh() {
        Ok(_) => PublishOutcome::Published(written),
        Err(_) => PublishOutcome::IndexRefused(written),
    }
}

/// Imports the audio files the shared storage area holds into the library.
///
/// WHY the duration is left unread: the device's filesystem reports a file's
/// length in bytes, not the length of the sound in it, and a length that was
/// never read is stored as unknown rather than guessed.
pub fn import_from_storage(store: &mut TrackStore) -> ImportOutcome {
    let mut buf = [0u8; 4096];
    let listed = match std::fs_list(STORAGE_ROOT, &mut buf) {
        Ok(n) => n,
        Err(_) => return ImportOutcome::ListRefused,
    };
    let mut imported = 0usize;
    let mut refused = 0usize;
    imported += import_listing(store, &buf[..listed], &mut refused);

    // One level into each folder the storage holds, so the import finds the
    // files a person put in a folder and not only the ones at the top.
    let mut directories: [([u8; 48], usize); 8] = [([0; 48], 0); 8];
    let mut directory_count = 0usize;
    for line in buf[..listed].split(|b| *b == b'\n') {
        if line.is_empty() {
            continue;
        }
        let mut fields = line.split(|b| *b == b'|');
        let Some(name) = fields.next() else {
            continue;
        };
        let Some(_size) = fields.next() else {
            continue;
        };
        let kind = fields.next().unwrap_or(b"file");
        if kind != b"dir" || name.is_empty() || directory_count >= directories.len() {
            continue;
        }
        let mut path = [0u8; 48];
        let mut at = 0usize;
        for &byte in STORAGE_ROOT.as_bytes() {
            if at >= path.len() {
                break;
            }
            path[at] = byte;
            at += 1;
        }
        if at < path.len() {
            path[at] = b'/';
            at += 1;
        }
        let n = name.len().min(path.len().saturating_sub(at));
        path[at..at + n].copy_from_slice(&name[..n]);
        at += n;
        directories[directory_count] = (path, at);
        directory_count += 1;
    }
    for index in 0..directory_count {
        let (path, len) = directories[index];
        let Ok(path_str) = core::str::from_utf8(&path[..len]) else {
            continue;
        };
        let mut inner = [0u8; 4096];
        let Ok(listed) = std::fs_list(path_str, &mut inner) else {
            continue;
        };
        imported += import_listing(store, &inner[..listed], &mut refused);
    }

    import_outcome(imported, refused)
}

/// Maps what the import found to what the screen may claim.
///
/// WHY the refusal count is separate from the search: a person who put audio
/// in the storage and got a refused write must not be told that no audio was
/// found.
fn import_outcome(imported: usize, refused: usize) -> ImportOutcome {
    match (imported, refused) {
        (0, 0) => ImportOutcome::NoneFound,
        (0, refused) => ImportOutcome::WriteRefused(refused),
        (imported, 0) => ImportOutcome::Imported(imported),
        (imported, refused) => ImportOutcome::PartiallyImported(imported, refused),
    }
}

/// Imports the audio files in one directory listing.
///
/// WHY a track the library already holds is skipped: the import runs on a
/// button, and a person who presses it twice should not find their library
/// full of copies.
fn import_listing(store: &mut TrackStore, listing: &[u8], refused: &mut usize) -> usize {
    let mut imported = 0usize;
    for line in listing.split(|b| *b == b'\n') {
        if line.is_empty() {
            continue;
        }
        let mut fields = line.split(|b| *b == b'|');
        let Some(name) = fields.next() else {
            continue;
        };
        let Some(_size) = fields.next() else {
            continue;
        };
        let kind = fields.next().unwrap_or(b"file");
        if kind == b"dir" || !is_audio_name(name) {
            continue;
        }
        let mut title = [0u8; TITLE_MAX];
        let n = title_from_name(name, &mut title);
        if n == 0 {
            continue;
        }
        if already_held(store, &title[..n]) {
            continue;
        }
        // WHY a duration of zero: the device reports a file's length in
        // bytes, not the length of the sound in it, and a length nobody read
        // is stored as unknown rather than guessed.
        if store.add(&title[..n], 0) {
            imported += 1;
        } else if matches!(store.state, StoreState::WriteRefused | StoreState::Full) {
            // WHY a found file is counted separately: the person did put audio
            // in the storage, so reporting "nothing was found" would describe
            // a search that never happened. The store refused to keep it.
            *refused += 1;
        }
    }
    imported
}

/// Whether the library already holds a track with this title.
fn already_held(store: &TrackStore, title: &[u8]) -> bool {
    for i in 0..store.len() {
        if let Some(track) = store.get(i) {
            if track.title() == title {
                return true;
            }
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_file_with_an_audio_extension_is_audio() {
        // WHY this matters: the import reads a directory listing, and a
        // listing holds every file the device has, not only the ones that
        // are sound.
        assert!(is_audio_name(b"nocturne.mp3"));
        assert!(is_audio_name(b"NOCTURNE.MP3"));
        assert!(is_audio_name(b"nocturne.flac"));
        assert!(!is_audio_name(b"notes.txt"));
        assert!(!is_audio_name(b"nocturne"));
        assert!(!is_audio_name(b"myflac"));
        assert!(!is_audio_name(b"mp3"));
    }

    #[test]
    fn a_file_name_becomes_a_title_without_its_extension() {
        let mut out = [0u8; TITLE_MAX];
        let n = title_from_name(b"nocturne.mp3", &mut out);
        assert_eq!(&out[..n], b"nocturne");
        let n = title_from_name(b"a-long-name.wav", &mut out);
        assert_eq!(&out[..n], b"a-long-name");
    }

    #[test]
    fn a_published_document_is_named_after_the_track() {
        // WHY this matters: the launcher's search matches the person's
        // query against the name of the file, so a document named after the
        // track is findable and one named after its index is not.
        let mut path = [0u8; PATH_CAP];
        let n = document_path(b"Nocturne", &mut path);
        let text = core::str::from_utf8(&path[..n]).expect("the path is text");
        assert_eq!(text, "/storage/music-Nocturne.txt");
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
        assert_eq!(text, "/storage/music-a-b-c.txt");
    }

    #[test]
    fn a_document_says_a_length_it_read_and_an_unknown_it_did_not() {
        let mut out = [0u8; DOC_BYTES];
        let n = render_document(b"Nocturne", true, 225, &mut out);
        assert_eq!(n, DOC_BYTES);
        let text = core::str::from_utf8(&out).expect("the document is text");
        assert!(text.contains("Track: Nocturne"));
        assert!(text.contains("Duration: 225 seconds"));

        let n = render_document(b"Nocturne", false, 0, &mut out);
        assert_eq!(n, DOC_BYTES);
        let text = core::str::from_utf8(&out).expect("the document is text");
        assert!(text.contains("Duration: not read"));
        assert!(!text.contains("Duration: 0 seconds"));
    }

    #[test]
    fn a_document_is_always_the_same_length() {
        // WHY: the platform's write does not truncate, so a document that
        // changed length would keep the tail of its previous contents.
        let mut short = [0u8; DOC_BYTES];
        let mut long = [0u8; DOC_BYTES];
        let a = render_document(b"a", true, 1, &mut short);
        let b = render_document(b"a very much longer title", true, 100_000, &mut long);
        assert_eq!(a, b);
        assert_eq!(a, DOC_BYTES);
    }

    #[test]
    fn a_refused_write_is_not_reported_as_an_empty_search() {
        // WHY: the old import reported both an empty listing and a refused
        // persistence write as "nothing found", which told a person their
        // audio was missing when the device had refused to store it.
        assert_eq!(import_outcome(0, 0), ImportOutcome::NoneFound);
        assert_eq!(import_outcome(0, 3), ImportOutcome::WriteRefused(3));
        assert_eq!(import_outcome(2, 0), ImportOutcome::Imported(2));
        assert_eq!(import_outcome(2, 1), ImportOutcome::PartiallyImported(2, 1));
    }
}
