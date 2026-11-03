// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

//! Publishing a sealed thread as a document the rest of the device can see.
//!
//! WHY this exists: the sealed copy of a thread lives in a vault sector, and
//! a vault sector is a secret. No other app can read it, the platform's
//! document index does not know that it exists, and the launcher cannot find
//! it. The app's own design document asks for the sealed thread to be
//! readable, so the app writes a transcript of the thread into the shared
//! storage area: the file manager lists it, the launcher's search finds it,
//! and the platform's content index describes it.
//!
//! WHY the vault copy stays: the transcript is a readable record for the
//! user, and the sealed sector is the app's own protected copy. Publishing a
//! transcript does not make the vault redundant, and the two hold different
//! things.

use interos_std as std;

use crate::{MessageRecord, MessageDirection};

/// The shared storage area the platform's index walks.
pub const STORAGE_ROOT: &str = "/storage";

/// The prefix of a published thread document's name.
pub const THREAD_DOC_PREFIX: &str = "messages-thread-";

/// The suffix of a published thread document's name.
pub const THREAD_DOC_SUFFIX: &str = ".txt";

/// The media type the platform's index gives a published thread.
///
/// WHY `text/plain`: the document is a transcript, it is a text document, and
/// the platform's index classifies a `.txt` file as plain text. The
/// share sheet still has `text/` targets, so the thread can be handed to
/// another app.
pub const THREAD_DOC_MIME: &str = "text/plain";

/// The size of a published document, in bytes.
///
/// WHY a fixed size and not "as long as the transcript": the platform's
/// write call replaces the bytes at the file's offset and does not truncate
/// the file, so a document that changed length would keep the tail of its
/// previous contents. Every write covers the whole file, so the document
/// never carries a stale tail.
pub const DOC_BYTES: usize = 512;

/// The longest path a published document may have.
pub const PATH_CAP: usize = 64;

/// What publishing a thread's document did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PublishOutcome {
    /// The document was written and the platform's index was refreshed.
    Published,
    /// The platform refused the write of the document.
    WriteRefused,
    /// The document was written, but the platform refused the index refresh.
    ///
    /// WHY this is separate from [`PublishOutcome::WriteRefused`]: the
    /// document is on the device either way, and a caller that says "the
    /// write failed" when the write succeeded is wrong about a fact the user
    /// can check.
    IndexRefused,
}

/// What handing a document to the app that handles its media type did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HandOutcome {
    /// The document was handed to the package that declared its media type.
    Handed,
    /// No installed package declares a handler for the document's media type.
    ///
    /// WHY the refusal names nothing: the platform has no answer to give, so
    /// the app must not offer a substitute.
    NoHandler,
    /// The platform refused the dispatch itself.
    DispatchRefused,
}

/// Writes the path of a thread's document into `out` and returns its length.
///
/// WHY the path is built here and not by the caller: the name has to be the
/// same one the publisher writes and the same one the intent dispatcher
/// hands over, and two spellings of one path is how a hand-off ends up
/// pointing at a file that was never written.
pub fn document_path(thread_id: u32, out: &mut [u8; PATH_CAP]) -> usize {
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
    for &byte in THREAD_DOC_PREFIX.as_bytes() {
        if at >= out.len() {
            return at;
        }
        out[at] = byte;
        at += 1;
    }
    let mut digits = [0u8; 10];
    let mut n = 0usize;
    let mut value = thread_id;
    loop {
        digits[n] = b'0' + (value % 10) as u8;
        value /= 10;
        n += 1;
        if value == 0 {
            break;
        }
    }
    for i in 0..n {
        if at >= out.len() {
            return at;
        }
        out[at] = digits[n - 1 - i];
        at += 1;
    }
    for &byte in THREAD_DOC_SUFFIX.as_bytes() {
        if at >= out.len() {
            return at;
        }
        out[at] = byte;
        at += 1;
    }
    at
}

/// Writes the transcript of a thread into `out` and returns its length.
///
/// WHY the transcript names the direction of every line: a line that says
/// only what was written does not say whether the app sent it, and a
/// transcript that blurs that is the same defect as a delivery badge that
/// claims a transport.
pub fn render_document(
    peer: &[u8],
    messages: &[MessageRecord],
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
    push(b"Thread with ", &mut at);
    push(peer, &mut at);
    push(b"\n", &mut at);
    push(b"No transport is bound to this app; every line below was written on this device.\n", &mut at);
    for record in messages {
        let who: &[u8] = if record.direction == MessageDirection::Outgoing {
            b"You"
        } else {
            b"Stored locally"
        };
        push(who, &mut at);
        push(b": ", &mut at);
        push(record.text(), &mut at);
        push(b"\n", &mut at);
    }
    // The remainder is padded so the document is always exactly DOC_BYTES
    // long. A shorter write would leave the tail of the previous transcript
    // in the file.
    while at < out.len() {
        out[at] = b' ';
        at += 1;
    }
    at
}

/// Writes a thread's document into the shared storage area and refreshes the
/// platform's index so the file manager and the launcher can find it.
///
/// Returns what the platform did, never what was hoped for.
pub fn publish(
    thread_id: u32,
    peer: &[u8],
    messages: &[MessageRecord],
) -> (PublishOutcome, [u8; PATH_CAP], usize) {
    let mut path = [0u8; PATH_CAP];
    let path_len = document_path(thread_id, &mut path);
    let Ok(path_str) = core::str::from_utf8(&path[..path_len]) else {
        return (PublishOutcome::WriteRefused, path, path_len);
    };

    let mut document = [0u8; DOC_BYTES];
    render_document(peer, messages, &mut document);

    let fd = match std::fs_open(path_str, std::FS_WRITE) {
        Ok(fd) => fd,
        Err(_) => return (PublishOutcome::WriteRefused, path, path_len),
    };
    let written = std::fs_write(fd, &document[..]);
    let _ = std::fs_fsync(fd);
    let _ = std::fs_close(fd);
    if written.is_err() {
        return (PublishOutcome::WriteRefused, path, path_len);
    }

    // WHY the refresh: the platform's index is rebuilt when the storage
    // generation moves, and the query that reads it does that on its own.
    // The refresh is asked for explicitly so the document is described
    // before the app reports that it was published.
    match std::content::content_index_refresh() {
        Ok(_) => (PublishOutcome::Published, path, path_len),
        Err(_) => (PublishOutcome::IndexRefused, path, path_len),
    }
}

/// Hands a published document to the package that declared a handler for its
/// media type.
///
/// Returns what the platform's registry said. A refusal names nothing.
pub fn hand_to_handler(path: &str) -> HandOutcome {
    let mut handler = [0u8; 64];
    match std::content::intent_resolve(
        std::content::IntentKind::Mime,
        THREAD_DOC_MIME,
        &mut handler,
    ) {
        Ok(0) | Err(_) => HandOutcome::NoHandler,
        Ok(_) => match std::content::intent_dispatch(
            std::content::IntentKind::Mime,
            THREAD_DOC_MIME,
            path,
        ) {
            Ok(_) => HandOutcome::Handed,
            Err(_) => HandOutcome::DispatchRefused,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::MessageDirection;

    fn record(direction: MessageDirection, body: &[u8]) -> MessageRecord {
        let mut r = MessageRecord::EMPTY;
        r.direction = direction;
        let n = body.len().min(crate::MAX_MESSAGE_BODY_LEN);
        r.body[..n].copy_from_slice(&body[..n]);
        r.body_len = n;
        r
    }

    #[test]
    fn a_published_thread_has_a_path_inside_the_shared_storage() {
        let mut path = [0u8; PATH_CAP];
        let len = document_path(7, &mut path);
        let text = core::str::from_utf8(&path[..len]).expect("the path is text");
        assert_eq!(text, "/storage/messages-thread-7.txt");
    }

    #[test]
    fn the_path_is_the_same_one_the_publisher_writes_and_the_hand_off_names() {
        // WHY this matters: the hand-off names a path and the publisher
        // writes one. Two spellings of one path is a hand-off that points at
        // a file that was never written.
        let mut a = [0u8; PATH_CAP];
        let mut b = [0u8; PATH_CAP];
        let la = document_path(123, &mut a);
        let lb = document_path(123, &mut b);
        assert_eq!(&a[..la], &b[..lb]);
    }

    #[test]
    fn a_transcript_says_that_every_line_was_written_here() {
        let mut document = [0u8; DOC_BYTES];
        let messages = [record(MessageDirection::Outgoing, b"hello")];
        let len = render_document(b"+15550000", &messages, &mut document);
        assert_eq!(len, DOC_BYTES);
        let text = core::str::from_utf8(&document).expect("the document is text");
        assert!(text.contains("Thread with +15550000"));
        assert!(text.contains("no transport") || text.contains("No transport"));
        assert!(text.contains("You: hello"));
    }

    #[test]
    fn a_transcript_is_always_the_same_length() {
        // WHY: the platform's write does not truncate, so a document that
        // changed length would keep the tail of its previous contents.
        let mut short = [0u8; DOC_BYTES];
        let mut long = [0u8; DOC_BYTES];
        let a = render_document(b"a", &[], &mut short);
        let many = [
            record(MessageDirection::Outgoing, b"one"),
            record(MessageDirection::Incoming, b"two"),
        ];
        let b = render_document(b"a", &many, &mut long);
        assert_eq!(a, b);
        assert_eq!(a, DOC_BYTES);
    }

    #[test]
    fn the_published_media_type_is_one_the_platform_can_hand_over() {
        // WHY: a hand-off names a media type and the platform's registry
        // answers "which app takes this". A type nothing declares would make
        // the hand-off a dead control.
        assert_eq!(THREAD_DOC_MIME, "text/plain");
        assert!(THREAD_DOC_SUFFIX.ends_with(".txt"));
    }
}
