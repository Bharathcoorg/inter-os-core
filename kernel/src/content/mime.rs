//! Media type detection for the platform's document index.
//!
//! WHY the bytes come first and the name only second: a file's name is a claim
//! somebody made and the file's contents are the fact. A `cat.jpg` that holds a
//! PNG is a PNG, and an index that reports `image/jpeg` for it would be
//! reporting the claim. The provenance is carried on the record so a consumer
//! can tell "the platform read the bytes" from "the platform believed the
//! name", and the contract test pins that the two are never conflated.

use alloc::string::String;

/// Where a media type answer came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MimeProvenance {
    /// The answer came from the file's own bytes.
    Bytes,
    /// The answer came from the file's name, because the bytes were not
    /// recognised.
    Extension,
    /// The platform could not determine a media type at all.
    Unknown,
}

impl MimeProvenance {
    /// The wire spelling of the provenance, as it appears in an index record.
    pub const fn as_str(self) -> &'static str {
        match self {
            MimeProvenance::Bytes => "bytes",
            MimeProvenance::Extension => "extension",
            MimeProvenance::Unknown => "unknown",
        }
    }
}

/// A detected media type together with the evidence behind it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MimeType {
    /// The media type, e.g. `image/png`.
    pub mime: &'static str,
    /// Where the answer came from.
    pub provenance: MimeProvenance,
}

impl MimeType {
    /// The answer for a file the platform could not classify.
    pub const UNKNOWN: MimeType = MimeType {
        mime: "application/octet-stream",
        provenance: MimeProvenance::Unknown,
    };
}

/// Magic-number table: the bytes that identify a format.
///
/// WHY a table and not a chain of `if`s: the set of formats the platform
/// recognises is a property of the platform, and a table makes "which formats
/// does the platform know" a question with one answer.
const MAGIC: &[(&[u8], &str)] = &[
    (b"\x89PNG\r\n\x1a\n", "image/png"),
    (b"\xff\xd8\xff", "image/jpeg"),
    (b"GIF87a", "image/gif"),
    (b"GIF89a", "image/gif"),
    (b"BM", "image/bmp"),
    (b"qoif", "image/qoi"),
    (b"OggS", "audio/ogg"),
    (b"ID3", "audio/mpeg"),
    (b"%PDF-", "application/pdf"),
    (b"PK\x03\x04", "application/zip"),
    (b"\x7fELF", "application/x-executable"),
    (b"fLaC", "audio/flac"),
];

/// Extension table: the fallback when the bytes are not recognised.
///
/// WHY the fallback exists at all: a document whose format the platform cannot
/// decode is still a document the user has, and refusing to describe it would
/// make the index lie by omission. The record says `extension` in that case,
/// so a consumer knows the answer is a name and not a fact.
const EXTENSIONS: &[(&str, &str)] = &[
    ("jpg", "image/jpeg"),
    ("jpeg", "image/jpeg"),
    ("png", "image/png"),
    ("gif", "image/gif"),
    ("bmp", "image/bmp"),
    ("webp", "image/webp"),
    ("qoi", "image/qoi"),
    ("ppm", "image/x-portable-pixmap"),
    ("raw", "application/octet-stream"),
    ("svg", "image/svg+xml"),
    ("mp4", "video/mp4"),
    ("m4v", "video/mp4"),
    ("mov", "video/quicktime"),
    ("webm", "video/webm"),
    ("mkv", "video/x-matroska"),
    ("avi", "video/x-msvideo"),
    ("mjpg", "video/x-motion-jpeg"),
    ("mjpeg", "video/x-motion-jpeg"),
    ("mp3", "audio/mpeg"),
    ("wav", "audio/wav"),
    ("pcm", "audio/pcm"),
    ("ogg", "audio/ogg"),
    ("flac", "audio/flac"),
    ("aac", "audio/aac"),
    ("m4a", "audio/mp4"),
    ("txt", "text/plain"),
    ("log", "text/plain"),
    ("md", "text/markdown"),
    ("csv", "text/csv"),
    ("html", "text/html"),
    ("htm", "text/html"),
    ("json", "application/json"),
    ("xml", "application/xml"),
    ("pdf", "application/pdf"),
    ("zip", "application/zip"),
    ("doc", "application/msword"),
    ("rapp", "application/vnd.interos.rapp"),
    // A published note is a note, not a plain text document: the notes app
    // declares this type in its manifest, so the intent registry and the
    // notes app agree about which app opens one.
    ("note", "text/x-interos-note"),
];

/// Detects a media type from a file's own bytes.
///
/// Returns `None` when the bytes match no known format. A `RIFF` container is
/// resolved further by its form type, because `RIFF` alone is a container and
/// not a format.
pub fn sniff_bytes(head: &[u8]) -> Option<&'static str> {
    for (magic, mime) in MAGIC {
        if head.len() >= magic.len() && &head[..magic.len()] == *magic {
            return Some(mime);
        }
    }
    if head.len() >= 12 && &head[..4] == b"RIFF" {
        match &head[8..12] {
            b"WEBP" => return Some("image/webp"),
            b"WAVE" => return Some("audio/wav"),
            b"AVI " => return Some("video/x-msvideo"),
            _ => {}
        }
    }
    if head.len() >= 12 && &head[4..8] == b"ftyp" {
        return Some("video/mp4");
    }
    // WHY the PPM header is checked with its whitespace and not on its own:
    // a netpbm file's first two bytes are its magic, but a text file that
    // happens to begin "P6" is not a picture, so the third byte has to be the
    // whitespace that follows the magic in a real header.
    if head.len() >= 3
        && (&head[..2] == b"P6" || &head[..2] == b"P3")
        && matches!(head[2], b' ' | b'\n' | b'\r' | b'\t')
    {
        return Some("image/x-portable-pixmap");
    }
    None
}

/// Detects a media type from a file's name alone.
pub fn from_extension(name: &str) -> Option<&'static str> {
    let dot = name.rfind('.')?;
    let ext = &name[dot + 1..];
    if ext.is_empty() {
        return None;
    }
    let lowered = ext.to_ascii_lowercase();
    EXTENSIONS
        .iter()
        .find(|(candidate, _)| *candidate == lowered)
        .map(|(_, mime)| *mime)
}

/// Classifies a document from its name and the first bytes of its contents.
///
/// WHY the bytes are consulted first: the name is the fallback for a format
/// the platform cannot read, and the provenance on the answer says which of
/// the two produced it.
pub fn classify(name: &str, head: &[u8]) -> MimeType {
    if let Some(mime) = sniff_bytes(head) {
        return MimeType {
            mime,
            provenance: MimeProvenance::Bytes,
        };
    }
    if let Some(mime) = from_extension(name) {
        return MimeType {
            mime,
            provenance: MimeProvenance::Extension,
        };
    }
    MimeType::UNKNOWN
}

/// The name of a document's file, without its directory part.
pub fn file_name(path: &str) -> &str {
    match path.rsplit_once('/') {
        Some((_, name)) if !name.is_empty() => name,
        _ => path,
    }
}

/// The lowercased extension of a name, without the dot.
pub fn extension(name: &str) -> String {
    match name.rsplit_once('.') {
        Some((_, ext)) if !ext.is_empty() => ext.to_ascii_lowercase(),
        _ => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_png_is_recognised_from_its_own_bytes() {
        let head = b"\x89PNG\r\n\x1a\n\x00\x00\x00\rIHDR";
        let answer = classify("holiday.jpeg", head);
        assert_eq!(answer.mime, "image/png");
        assert_eq!(answer.provenance, MimeProvenance::Bytes);
    }

    #[test]
    fn a_name_that_contradicts_the_bytes_loses_to_the_bytes() {
        // A PNG named `.jpg` is a PNG, and the record says where the answer
        // came from so the contradiction is visible rather than hidden.
        let head = b"\x89PNG\r\n\x1a\n";
        let answer = classify("holiday.jpg", head);
        assert_eq!(answer.mime, "image/png");
        assert_eq!(answer.provenance, MimeProvenance::Bytes);
    }

    #[test]
    fn an_unreadable_format_falls_back_to_the_name_and_says_so() {
        let answer = classify("notes.txt", b"plain text bytes");
        assert_eq!(answer.mime, "text/plain");
        assert_eq!(answer.provenance, MimeProvenance::Extension);
    }

    #[test]
    fn an_unknown_document_is_reported_as_unknown() {
        let answer = classify("payload", b"\x01\x02\x03\x04");
        assert_eq!(answer.mime, "application/octet-stream");
        assert_eq!(answer.provenance, MimeProvenance::Unknown);
    }

    #[test]
    fn riff_containers_are_resolved_by_their_form_type() {
        let mut wave = [0u8; 12];
        wave[..4].copy_from_slice(b"RIFF");
        wave[8..].copy_from_slice(b"WAVE");
        assert_eq!(sniff_bytes(&wave), Some("audio/wav"));
    }

    #[test]
    fn the_extension_fallback_is_case_insensitive() {
        assert_eq!(from_extension("PHOTO.JPG"), Some("image/jpeg"));
        assert_eq!(from_extension("clip.MP4"), Some("video/mp4"));
        assert_eq!(from_extension("noextension"), None);
    }
}
