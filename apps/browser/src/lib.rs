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
}
