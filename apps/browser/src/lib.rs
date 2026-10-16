#![no_std]

pub use interos_std as std;

/// Supported URI schemes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UriScheme {
    Http = 0,
    Https = 1,
    InterLayer = 2,
    Gemini = 3,
    File = 4,
    Unknown = 5,
}

impl UriScheme {
    pub fn is_local(&self) -> bool {
        matches!(self, UriScheme::File | UriScheme::InterLayer)
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            UriScheme::Http => "http",
            UriScheme::Https => "https",
            UriScheme::InterLayer => "interos",
            UriScheme::Gemini => "gemini",
            UriScheme::File => "file",
            UriScheme::Unknown => "unknown",
        }
    }
}

/// Parsed uniform resource identifier components without allocation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ParsedUri<'a> {
    pub scheme: UriScheme,
    pub host: &'a [u8],
    pub path: &'a [u8],
    pub port: u16,
}

/// Parses a URI byte string into its protocol scheme, host, and path components.
pub fn parse_uri(raw: &[u8]) -> ParsedUri<'_> {
    let (scheme, rest) = if let Some(r) = raw.strip_prefix(b"interos://") {
        (UriScheme::InterLayer, r)
    } else if let Some(r) = raw.strip_prefix(b"interlayer://") {
        (UriScheme::InterLayer, r)
    } else if let Some(r) = raw.strip_prefix(b"file://") {
        (UriScheme::File, r)
    } else if let Some(r) = raw.strip_prefix(b"https://") {
        (UriScheme::Https, r)
    } else if let Some(r) = raw.strip_prefix(b"http://") {
        (UriScheme::Http, r)
    } else if let Some(r) = raw.strip_prefix(b"gemini://") {
        (UriScheme::Gemini, r)
    } else {
        return ParsedUri {
            scheme: UriScheme::Unknown,
            host: b"",
            path: b"",
            port: 0,
        };
    };

    if scheme.is_local() {
        return ParsedUri {
            scheme,
            host: b"localhost",
            path: rest,
            port: 0,
        };
    }

    let slash_pos = rest.iter().position(|&b| b == b'/');
    let (authority, path) = match slash_pos {
        Some(idx) => (&rest[..idx], &rest[idx..]),
        None => (rest, &b""[..]),
    };

    let (host, port) = match authority.iter().rposition(|&b| b == b':') {
        Some(i) => {
            let digits = &authority[i + 1..];
            let mut value: u16 = 0;
            let mut all_digits = !digits.is_empty();
            for &b in digits {
                if b.is_ascii_digit() {
                    value = value.saturating_mul(10).saturating_add((b - b'0') as u16);
                } else {
                    all_digits = false;
                    break;
                }
            }
            if all_digits {
                (&authority[..i], value)
            } else {
                (authority, 0)
            }
        }
        None => (authority, 0),
    };

    ParsedUri {
        scheme,
        host,
        path,
        port,
    }
}

/// Line classification for Markdown and Gemtext documents.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineKind {
    Heading1,
    Heading2,
    Heading3,
    ListItem,
    Blockquote,
    GemtextLink,
    Paragraph,
}

/// Classifies a single line of text for document rendering.
pub fn classify_line(line: &[u8]) -> LineKind {
    if line.starts_with(b"### ") {
        LineKind::Heading3
    } else if line.starts_with(b"## ") {
        LineKind::Heading2
    } else if line.starts_with(b"# ") {
        LineKind::Heading1
    } else if line.starts_with(b"* ") || line.starts_with(b"- ") {
        LineKind::ListItem
    } else if line.starts_with(b"> ") {
        LineKind::Blockquote
    } else if line.starts_with(b"=> ") || line.starts_with(b"=>\t") {
        LineKind::GemtextLink
    } else {
        LineKind::Paragraph
    }
}

/// Width of the URL the start page's page list shows one row at.
///
/// WHY 64 when a stored bookmark may be 128 bytes: the row is a caption, and a
/// longer address is truncated rather than wrapped onto a second line. What
/// matters is that the field is a fixed width, so a row is `Copy` and the
/// per-pixel painter can read it without borrowing the app state through every
/// `if let` between the loop and the draw call.
pub const URL_FIELD: usize = 64;

/// Copies a URL into the start page's fixed-width field, dropping one trailing
/// `/` so a bare origin does not render as `host./`.
///
/// Returns the field and the number of bytes written into it.
pub fn into_url_field(url: &[u8]) -> ([u8; URL_FIELD], usize) {
    let mut end = url.len();
    if end > 1 && url[end - 1] == b'/' {
        end -= 1;
    }
    let end = end.min(URL_FIELD);
    let mut out = [0u8; URL_FIELD];
    out[..end].copy_from_slice(&url[..end]);
    (out, end)
}

/// A one-character mark for the scheme a page was opened under.
///
/// WHY this exists: the start page's row used to carry a decorative tile
/// labelled "WEB", standing in for a favicon. No favicon service is bound to
/// this shell, so it stood for an image the browser had never loaded. This
/// stands for something that can be read off the URL itself.
pub fn scheme_mark(url: &[u8]) -> &'static [u8] {
    if url.starts_with(b"https://") {
        b"S"
    } else if url.starts_with(b"http://") {
        b"H"
    } else if url.starts_with(b"gemini://") {
        b"G"
    } else if url.starts_with(b"interos://") || url.starts_with(b"interlayer://") {
        b"I"
    } else if url.starts_with(b"file://") {
        b"F"
    } else {
        b"?"
    }
}

pub const MAX_URL_LEN: usize = 128;
pub const MAX_TITLE_LEN: usize = 64;
pub const MAX_HISTORY_ENTRIES: usize = 8;
pub const MAX_BOOKMARKS: usize = 8;

/// Bounded browser navigation history record.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HistoryRecord {
    pub url: [u8; MAX_URL_LEN],
    pub url_len: usize,
    pub title: [u8; MAX_TITLE_LEN],
    pub title_len: usize,
    pub visited_at_epoch_secs: u64,
}

impl HistoryRecord {
    pub const EMPTY: Self = Self {
        url: [0; MAX_URL_LEN],
        url_len: 0,
        title: [0; MAX_TITLE_LEN],
        title_len: 0,
        visited_at_epoch_secs: 0,
    };

    pub fn new(url_bytes: &[u8], title_bytes: &[u8], timestamp: u64) -> Self {
        let mut rec = Self::EMPTY;
        let u = url_bytes.len().min(MAX_URL_LEN);
        rec.url[..u].copy_from_slice(&url_bytes[..u]);
        rec.url_len = u;
        let t = title_bytes.len().min(MAX_TITLE_LEN);
        rec.title[..t].copy_from_slice(&title_bytes[..t]);
        rec.title_len = t;
        rec.visited_at_epoch_secs = timestamp;
        rec
    }
}

/// Bounded bookmark record.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BookmarkRecord {
    pub url: [u8; MAX_URL_LEN],
    pub url_len: usize,
    pub title: [u8; MAX_TITLE_LEN],
    pub title_len: usize,
    pub is_pinned: bool,
}

impl BookmarkRecord {
    pub const EMPTY: Self = Self {
        url: [0; MAX_URL_LEN],
        url_len: 0,
        title: [0; MAX_TITLE_LEN],
        title_len: 0,
        is_pinned: false,
    };

    pub fn new(url_bytes: &[u8], title_bytes: &[u8], is_pinned: bool) -> Self {
        let mut rec = Self::EMPTY;
        let u = url_bytes.len().min(MAX_URL_LEN);
        rec.url[..u].copy_from_slice(&url_bytes[..u]);
        rec.url_len = u;
        let t = title_bytes.len().min(MAX_TITLE_LEN);
        rec.title[..t].copy_from_slice(&title_bytes[..t]);
        rec.title_len = t;
        rec.is_pinned = is_pinned;
        rec
    }
}

/// Zero-heap bounded browser history and bookmark store.
#[derive(Debug, Clone, Copy)]
pub struct BrowserDataStore {
    pub history: [HistoryRecord; MAX_HISTORY_ENTRIES],
    pub history_count: usize,
    pub bookmarks: [BookmarkRecord; MAX_BOOKMARKS],
    pub bookmark_count: usize,
}

impl BrowserDataStore {
    pub const fn new() -> Self {
        Self {
            history: [HistoryRecord::EMPTY; MAX_HISTORY_ENTRIES],
            history_count: 0,
            bookmarks: [BookmarkRecord::EMPTY; MAX_BOOKMARKS],
            bookmark_count: 0,
        }
    }

    pub fn push_history(&mut self, rec: HistoryRecord) {
        if self.history_count < MAX_HISTORY_ENTRIES {
            self.history[self.history_count] = rec;
            self.history_count += 1;
        } else {
            for i in 1..MAX_HISTORY_ENTRIES {
                self.history[i - 1] = self.history[i];
            }
            self.history[MAX_HISTORY_ENTRIES - 1] = rec;
        }
    }

    pub fn add_bookmark(&mut self, b: BookmarkRecord) -> bool {
        if self.bookmark_count >= MAX_BOOKMARKS {
            return false;
        }
        self.bookmarks[self.bookmark_count] = b;
        self.bookmark_count += 1;
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_uri_parser_schemes() {
        assert_eq!(parse_uri(b"https://example.com/index.html"), ParsedUri {
            scheme: UriScheme::Https,
            host: b"example.com",
            path: b"/index.html",
            port: 0,
        });

        assert_eq!(parse_uri(b"gemini://geminispace.info:1965/capsule"), ParsedUri {
            scheme: UriScheme::Gemini,
            host: b"geminispace.info",
            path: b"/capsule",
            port: 1965,
        });

        assert_eq!(parse_uri(b"file:///data/docs/readme.txt"), ParsedUri {
            scheme: UriScheme::File,
            host: b"localhost",
            path: b"/data/docs/readme.txt",
            port: 0,
        });

        assert_eq!(parse_uri(b"interos://manifest.json"), ParsedUri {
            scheme: UriScheme::InterLayer,
            host: b"localhost",
            path: b"manifest.json",
            port: 0,
        });

        assert_eq!(parse_uri(b"ftp://invalid.com"), ParsedUri {
            scheme: UriScheme::Unknown,
            host: b"",
            path: b"",
            port: 0,
        });
    }

    #[test]
    fn test_classify_markdown_and_gemtext_lines() {
        assert_eq!(classify_line(b"# Main Title"), LineKind::Heading1);
        assert_eq!(classify_line(b"## Section Heading"), LineKind::Heading2);
        assert_eq!(classify_line(b"### Subheading"), LineKind::Heading3);
        assert_eq!(classify_line(b"* Bullet point item"), LineKind::ListItem);
        assert_eq!(classify_line(b"- Hyphen list item"), LineKind::ListItem);
        assert_eq!(classify_line(b"> Blockquote text"), LineKind::Blockquote);
        assert_eq!(classify_line(b"=> https://example.com Link Text"), LineKind::GemtextLink);
        assert_eq!(classify_line(b"Regular paragraph prose."), LineKind::Paragraph);
    }

    #[test]
    fn a_scheme_mark_reflects_the_url_it_was_given() {
        assert_eq!(scheme_mark(b"https://example.org/x"), b"S");
        assert_eq!(scheme_mark(b"http://example.org"), b"H");
        assert_eq!(scheme_mark(b"gemini://capsule"), b"G");
        assert_eq!(scheme_mark(b"interos://manifest.json"), b"I");
        assert_eq!(scheme_mark(b"file:///data/notes.txt"), b"F");
        assert_eq!(scheme_mark(b"about:blank"), b"?");
    }

    #[test]
    fn a_url_field_drops_a_trailing_slash_from_a_bare_origin() {
        let (field, len) = into_url_field(b"https://example.org/");
        assert_eq!(&field[..len], b"https://example.org");
    }

    #[test]
    fn a_url_field_keeps_a_lone_slash_as_a_path() {
        // WHY: trimming must not be "drop a trailing slash" applied blindly. A
        // single `/` is a path, and collapsing it to an empty field would render
        // as no address at all.
        let (_, len) = into_url_field(b"/");
        assert_eq!(len, 1);
    }

    #[test]
    fn a_url_field_is_truncated_to_the_row_width() {
        let long = b"https://example.org/a/very/long/path/that/keeps/going/and/going/beyond/the/field";
        assert!(
            long.len() > URL_FIELD,
            "the fixture must exceed the field or the truncation is untested"
        );
        let (field, len) = into_url_field(long);
        assert_eq!(len, URL_FIELD);
        assert_eq!(&field[..len], &long[..URL_FIELD]);
    }

    #[test]
    fn test_browser_data_store() {
        let mut store = BrowserDataStore::new();
        assert_eq!(store.history_count, 0);
        assert_eq!(store.bookmark_count, 0);

        store.push_history(HistoryRecord::new(b"https://interos.org", b"InterOS Home", 100));
        assert_eq!(store.history_count, 1);
        assert_eq!(&store.history[0].url[..store.history[0].url_len], b"https://interos.org");

        // Push 10 entries to verify sliding window of 8 entries
        for i in 2..=10 {
            store.push_history(HistoryRecord::new(b"https://site.org", b"Site", i as u64));
        }
        assert_eq!(store.history_count, MAX_HISTORY_ENTRIES);
        assert_eq!(store.history[MAX_HISTORY_ENTRIES - 1].visited_at_epoch_secs, 10);
        assert_eq!(store.history[0].visited_at_epoch_secs, 3);

        // Bookmarks
        assert!(store.add_bookmark(BookmarkRecord::new(b"gemini://docs", b"Documentation", true)));
        assert_eq!(store.bookmark_count, 1);
        assert!(store.bookmarks[0].is_pinned);
    }
}

