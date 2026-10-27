#![no_std]

pub use interos_std as std;

/// The app's view of the platform's design system.
pub mod theme;

/// The bounded CSS subset the reader applies to block elements.
pub mod css;

/// The bounded inline-image policy: size caps, format sniffing, markers
/// and same-origin URL resolution.
pub mod images;

/// The bounded HTTP protocol layer: redirects, cookies, forms, conditional
/// requests, charset conversion and the compressed-body decoder.
pub mod http;

/// Bounded DEFLATE, zlib and gzip decoding for response bodies.
pub mod inflate;

/// The bounded DOM and box layout engine used by the browser view.
pub mod layout;

/// The bounded native JavaScript AST lexer, parser and interpreter.
pub mod js;

/// Stable browser engine abstraction (Native Flow & Servo Embed).
pub mod engine;
pub use engine::{BrowserEngine, EngineAction, EngineCapabilities, EngineStatus, NativeFlowEngine};

/// The platform's scroll model.
use interos_std::design::scroll::ScrollState;

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

    pub fn default_port(&self) -> u16 {
        match self {
            UriScheme::Http => 80,
            UriScheme::Https => 443,
            UriScheme::Gemini => 1965,
            _ => 0,
        }
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
    pub query: Option<&'a [u8]>,
}

impl<'a> ParsedUri<'a> {
    pub fn query_str(&self) -> Option<&'a str> {
        self.query.and_then(|q| core::str::from_utf8(q).ok())
    }
}

fn strip_prefix_ignore_case<'a>(raw: &'a [u8], prefix: &[u8]) -> Option<&'a [u8]> {
    if raw.len() >= prefix.len() && raw[..prefix.len()].eq_ignore_ascii_case(prefix) {
        Some(&raw[prefix.len()..])
    } else {
        None
    }
}

/// Parses a URI byte string into its protocol scheme, host, path, port, and query string.
pub fn parse_uri(raw: &[u8]) -> ParsedUri<'_> {
    let (scheme, rest) = if let Some(r) = strip_prefix_ignore_case(raw, b"interos://") {
        (UriScheme::InterLayer, r)
    } else if let Some(r) = strip_prefix_ignore_case(raw, b"interlayer://") {
        (UriScheme::InterLayer, r)
    } else if let Some(r) = strip_prefix_ignore_case(raw, b"file://") {
        (UriScheme::File, r)
    } else if let Some(r) = strip_prefix_ignore_case(raw, b"https://") {
        (UriScheme::Https, r)
    } else if let Some(r) = strip_prefix_ignore_case(raw, b"http://") {
        (UriScheme::Http, r)
    } else if let Some(r) = strip_prefix_ignore_case(raw, b"gemini://") {
        (UriScheme::Gemini, r)
    } else {
        return ParsedUri {
            scheme: UriScheme::Unknown,
            host: b"",
            path: b"",
            port: 0,
            query: None,
        };
    };

    if scheme.is_local() {
        let (path, query) = match rest.iter().position(|&b| b == b'?') {
            Some(q_idx) => (&rest[..q_idx], Some(&rest[q_idx + 1..])),
            None => (rest, None),
        };
        return ParsedUri {
            scheme,
            host: b"localhost",
            path,
            port: 0,
            query,
        };
    }

    let delim_pos = rest.iter().position(|&b| b == b'/' || b == b'?');
    let (raw_authority, remainder) = match delim_pos {
        Some(idx) => (&rest[..idx], &rest[idx..]),
        None => (rest, &b""[..]),
    };

    // Strip userinfo if present to prevent credential leakage into host
    let authority = match raw_authority.iter().position(|&b| b == b'@') {
        Some(at_idx) => &raw_authority[at_idx + 1..],
        None => raw_authority,
    };

    let (host, port) = if authority.starts_with(b"[") {
        // Bracketed IPv6 literal: [::1] or [::1]:8080
        match authority.iter().position(|&b| b == b']') {
            Some(close_idx) => {
                let ipv6_host = &authority[..=close_idx];
                let remainder_after_bracket = &authority[close_idx + 1..];
                if let Some(colon_pos) = remainder_after_bracket.iter().position(|&b| b == b':') {
                    let digits = &remainder_after_bracket[colon_pos + 1..];
                    let mut value: u32 = 0;
                    let mut all_digits = !digits.is_empty();
                    for &b in digits {
                        if b.is_ascii_digit() {
                            value = value.saturating_mul(10).saturating_add((b - b'0') as u32);
                        } else {
                            all_digits = false;
                            break;
                        }
                    }
                    if all_digits && value <= 65535 {
                        (ipv6_host, value as u16)
                    } else {
                        (ipv6_host, scheme.default_port())
                    }
                } else {
                    (ipv6_host, scheme.default_port())
                }
            }
            None => (authority, scheme.default_port()),
        }
    } else {
        match authority.iter().rposition(|&b| b == b':') {
            Some(i) => {
                let digits = &authority[i + 1..];
                let mut value: u32 = 0;
                let mut all_digits = !digits.is_empty();
                for &b in digits {
                    if b.is_ascii_digit() {
                        value = value.saturating_mul(10).saturating_add((b - b'0') as u32);
                    } else {
                        all_digits = false;
                        break;
                    }
                }
                if all_digits && value > 0 && value <= 65535 {
                    (&authority[..i], value as u16)
                } else if all_digits && value == 0 {
                    (&authority[..i], 0)
                } else {
                    (&authority[..i], scheme.default_port())
                }
            }
            None => (authority, scheme.default_port()),
        }
    };

    let (path, query) = if remainder.starts_with(b"?") {
        (&b"/"[..], Some(&remainder[1..]))
    } else if remainder.is_empty() {
        (&b""[..], None)
    } else {
        match remainder.iter().position(|&b| b == b'?') {
            Some(q_idx) => (&remainder[..q_idx], Some(&remainder[q_idx + 1..])),
            None => (remainder, None),
        }
    };

    ParsedUri {
        scheme,
        host,
        path,
        port,
        query,
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
    CodeBlock,
    GemtextLink,
    /// An inline image element recorded by the markup pipeline.
    Image,
    Paragraph,
    Empty,
}

/// Classifies a single line of text for document rendering without dynamic allocation.
pub fn classify_line(line: &[u8]) -> LineKind {
    if images::is_image_marker(line) {
        return LineKind::Image;
    }
    let mut i = 0;
    while i < line.len() && (line[i] == b' ' || line[i] == b'\t' || line[i] == b'\r') {
        i += 1;
    }
    let trimmed = &line[i..];
    if trimmed.is_empty() {
        return LineKind::Empty;
    }
    if trimmed.starts_with(b"```") {
        LineKind::CodeBlock
    } else if trimmed.starts_with(b"### ") {
        LineKind::Heading3
    } else if trimmed.starts_with(b"## ") {
        LineKind::Heading2
    } else if trimmed.starts_with(b"# ") {
        LineKind::Heading1
    } else if trimmed.starts_with(b"* ") || trimmed.starts_with(b"- ") || trimmed.starts_with(b"+ ") {
        LineKind::ListItem
    } else if trimmed.starts_with(b"> ") || trimmed.starts_with(b">") {
        LineKind::Blockquote
    } else if trimmed.starts_with(b"=> ") || trimmed.starts_with(b"=>\t") || trimmed.starts_with(b"=>") {
        LineKind::GemtextLink
    } else {
        LineKind::Paragraph
    }
}

/// Formats a standards-compliant HTTP/1.1 GET request into `out`.
pub fn format_http_get(host: &[u8], path: &[u8], out: &mut [u8]) -> usize {
    let clean_path = if path.is_empty() { b"/" } else { path };
    let mut cursor = 0;

    let write_slice = |cur: &mut usize, src: &[u8], dst: &mut [u8]| {
        let n = src.len().min(dst.len().saturating_sub(*cur));
        dst[*cur..*cur + n].copy_from_slice(&src[..n]);
        *cur += n;
    };

    write_slice(&mut cursor, b"GET ", out);
    write_slice(&mut cursor, clean_path, out);
    write_slice(&mut cursor, b" HTTP/1.1\r\nHost: ", out);
    write_slice(&mut cursor, host, out);
    write_slice(&mut cursor, b"\r\nUser-Agent: InterOS-Browser/1.0 (Mobile; Sovereign)\r\nAccept: text/html,text/gemini,text/plain,*/*\r\nConnection: close\r\n\r\n", out);

    cursor
}

/// Formats an HTTP request to the Device Studio proxy, reached through the
/// slirp gateway at 10.0.2.2:4300.
pub fn format_proxy_request(target_url: &[u8], out: &mut [u8]) -> usize {
    let mut cursor = 0;
    let write_slice = |cur: &mut usize, src: &[u8], dst: &mut [u8]| {
        let n = src.len().min(dst.len().saturating_sub(*cur));
        dst[*cur..*cur + n].copy_from_slice(&src[..n]);
        *cur += n;
    };

    write_slice(&mut cursor, b"GET /api/browser/proxy?url=", out);
    for &b in target_url {
        if b.is_ascii_alphanumeric() || b == b'-' || b == b'_' || b == b'.' || b == b'~' {
            if cursor < out.len() {
                out[cursor] = b;
                cursor += 1;
            }
        } else {
            if cursor + 3 <= out.len() {
                out[cursor] = b'%';
                let hi = b >> 4;
                let lo = b & 0x0F;
                out[cursor + 1] = if hi < 10 { b'0' + hi } else { b'A' + (hi - 10) };
                out[cursor + 2] = if lo < 10 { b'0' + lo } else { b'A' + (lo - 10) };
                cursor += 3;
            }
        }
    }
    // The Studio's local-only guard accepts a loopback authority only, so the
    // Host names the service on its own loopback even though the wire
    // destination is the slirp gateway at 10.0.2.2.
    write_slice(&mut cursor, b" HTTP/1.1\r\nHost: 127.0.0.1:4300\r\nUser-Agent: InterOS-Browser/1.0 (Mobile; Sovereign)\r\nConnection: close\r\n\r\n", out);
    cursor
}

/// The most a single browser upload will send.
///
/// WHY this matches the gateway's own cap: the gateway refuses a body it
/// cannot hold, and an upload the browser knows will be refused should be
/// refused before a byte goes on the wire.
pub const MAX_UPLOAD_BYTES: usize = 4 * 1024 * 1024;

/// Appends a decimal number to a buffer.
fn append_decimal(out: &mut [u8], mut n: usize, mut value: u64) -> usize {
    let mut digits = [0u8; 20];
    let mut len = 0usize;
    loop {
        digits[len] = b'0' + (value % 10) as u8;
        len += 1;
        value /= 10;
        if value == 0 {
            break;
        }
    }
    while len > 0 {
        len -= 1;
        if n >= out.len() {
            break;
        }
        out[n] = digits[len];
        n += 1;
    }
    n
}

/// Appends a filename that is safe to put in a request header.
///
/// WHY only the last path component and a small alphabet: the name is
/// copied into an HTTP header, and a newline or a colon from the file
/// system would end the header or start another one. The gateway turns
/// this name back into a stored name, so the two ends agree on one
/// spelling.
fn append_header_filename(out: &mut [u8], mut n: usize, raw: &[u8]) -> usize {
    if raw.is_empty() {
        return append_bytes(out, n, b"upload.bin");
    }
    let name = match raw.iter().rposition(|&b| b == b'/' || b == b'\\') {
        Some(slash) => &raw[slash + 1..],
        None => raw,
    };
    let start = n;
    for &b in name {
        if n >= out.len() {
            break;
        }
        if b.is_ascii_alphanumeric() || b == b'.' || b == b'-' || b == b'_' {
            out[n] = b;
            n += 1;
        } else if b == b' ' {
            out[n] = b'_';
            n += 1;
        }
    }
    if n == start {
        append_bytes(out, n, b"upload.bin")
    } else {
        n
    }
}

/// Appends a media type that is safe to put in a request header.
fn append_header_mime(out: &mut [u8], mut n: usize, raw: &[u8]) -> usize {
    let semi = raw.iter().position(|&b| b == b';').unwrap_or(raw.len());
    let value = trim_ascii(&raw[..semi]);
    let start = n;
    for &b in value {
        if n >= out.len() {
            break;
        }
        if b.is_ascii_alphanumeric() || b == b'/' || b == b'+' || b == b'.' || b == b'-' {
            out[n] = b;
            n += 1;
        }
    }
    if n == start {
        append_bytes(out, n, b"application/octet-stream")
    } else {
        n
    }
}

/// Copies a content type as the bare media type, without its parameters.
///
/// WHY the parameters are dropped: the platform's handler registry keys a
/// handler by media type, and `image/png; charset=binary` is not the key
/// `image/png`. A type that carries nothing usable becomes
/// `application/octet-stream`, which is what the registry will also call an
/// unknown file.
pub fn bare_mime(raw: &[u8], out: &mut [u8]) -> usize {
    append_header_mime(out, 0, raw)
}

/// Formats the upload the browser sends to the Studio gateway.
///
/// WHY the file is the request body and the name travels in a header:
/// the gateway's upload route stores the bytes it receives and reports
/// the count back, so there is no form syntax to get wrong and the body
/// is exactly the file.
pub fn format_upload_request(
    filename: &[u8],
    content_type: &[u8],
    content_length: usize,
    out: &mut [u8],
) -> usize {
    let mut n = append_bytes(out, 0, b"POST /api/browser/upload HTTP/1.1\r\nHost: 127.0.0.1:4300\r\nContent-Type: ");
    n = append_header_mime(out, n, content_type);
    n = append_bytes(out, n, b"\r\nContent-Length: ");
    n = append_decimal(out, n, content_length as u64);
    n = append_bytes(out, n, b"\r\nX-Interos-Filename: ");
    n = append_header_filename(out, n, filename);
    append_bytes(out, n, b"\r\nConnection: close\r\n\r\n")
}

/// Formats a speed-test download request for a bounded payload.
pub fn format_speedtest_download_request(bytes: usize, out: &mut [u8]) -> usize {
    let mut n = append_bytes(out, 0, b"GET /api/speedtest/download?bytes=");
    n = append_decimal(out, n, bytes as u64);
    append_bytes(
        out,
        n,
        b" HTTP/1.1\r\nHost: 127.0.0.1:4300\r\nAccept: application/octet-stream\r\nConnection: close\r\n\r\n",
    )
}

/// Formats a speed-test upload request for a body of `content_length` bytes.
pub fn format_speedtest_upload_request(content_length: usize, out: &mut [u8]) -> usize {
    let mut n = append_bytes(
        out,
        0,
        b"POST /api/speedtest/upload HTTP/1.1\r\nHost: 127.0.0.1:4300\r\nContent-Type: application/octet-stream\r\nContent-Length: ",
    );
    n = append_decimal(out, n, content_length as u64);
    append_bytes(out, n, b"\r\nConnection: close\r\n\r\n")
}

/// Reads an unsigned integer field out of the gateway's JSON reply.
///
/// WHY the local parser and not a document model: the replies the browser
/// asks for are flat objects of scalars, and the one number that matters
/// here is the byte count the gateway says it received.
pub fn json_u64_field(json: &[u8], key: &[u8]) -> Option<u64> {
    let mut pattern = [0u8; 48];
    if key.len() + 2 > pattern.len() {
        return None;
    }
    pattern[0] = b'"';
    pattern[1..1 + key.len()].copy_from_slice(key);
    pattern[1 + key.len()] = b'"';
    let plen = key.len() + 2;
    let mut pos = json.windows(plen).position(|w| w == &pattern[..plen])? + plen;
    while pos < json.len() && json[pos].is_ascii_whitespace() {
        pos += 1;
    }
    if pos >= json.len() || json[pos] != b':' {
        return None;
    }
    pos += 1;
    while pos < json.len() && json[pos].is_ascii_whitespace() {
        pos += 1;
    }
    let mut value: u64 = 0;
    let mut digits = 0usize;
    while pos < json.len() && json[pos].is_ascii_digit() {
        value = value.checked_mul(10)?.checked_add((json[pos] - b'0') as u64)?;
        pos += 1;
        digits += 1;
    }
    if digits == 0 {
        None
    } else {
        Some(value)
    }
}

/// Extracts the HTML body from a proxy JSON response `{"success": true, ..., "html": "..."}`
/// without heap allocation, unescaping `\"`, `\n`, `\r`, `\t`, and `\/`.
pub fn extract_proxy_html(json: &[u8], out: &mut [u8]) -> usize {
    let key = b"\"html\": \"";
    let key_pos = match json.windows(key.len()).position(|w| w == key) {
        Some(pos) => pos + key.len(),
        None => {
            let alt_key = b"\"html\":\"";
            match json.windows(alt_key.len()).position(|w| w == alt_key) {
                Some(pos) => pos + alt_key.len(),
                None => return 0,
            }
        }
    };

    let content = &json[key_pos..];
    let mut r = 0;
    let mut w = 0;
    while r < content.len() && w < out.len() {
        if content[r] == b'\\' && r + 1 < content.len() {
            match content[r + 1] {
                b'"' => { out[w] = b'"'; w += 1; r += 2; }
                b'\\' => { out[w] = b'\\'; w += 1; r += 2; }
                b'/' => { out[w] = b'/'; w += 1; r += 2; }
                b'n' => { out[w] = b'\n'; w += 1; r += 2; }
                b'r' => { out[w] = b'\r'; w += 1; r += 2; }
                b't' => { out[w] = b'\t'; w += 1; r += 2; }
                b'b' => { out[w] = 0x08; w += 1; r += 2; }
                b'f' => { out[w] = 0x0C; w += 1; r += 2; }
                // WHY the \uXXXX case matters: the gateway's JSON encoder
                // escapes every non-ASCII character, so an em dash arrives
                // as the six characters "\u2014". Without decoding it the
                // reader printed the escape sequence on the page -- the
                // live article's title read "InterOS \u2014 A security-first
                // mobile operating system".
                b'u' => {
                    let (cp, next) = read_hex4(content, r + 2);
                    match cp {
                        Some(high @ 0xD800..=0xDBFF) => {
                            if next + 2 <= content.len() && content[next] == b'\\' && content[next + 1] == b'u' {
                                let (low, low_next) = read_hex4(content, next + 2);
                                if let Some(low @ 0xDC00..=0xDFFF) = low {
                                    let scalar = 0x10000 + (((high - 0xD800) << 10) | (low - 0xDC00));
                                    let len = utf8_len(scalar);
                                    if len > 0 && w + len <= out.len() {
                                        w = write_utf8(out, w, scalar);
                                        r = low_next;
                                    } else {
                                        out[w] = b'?';
                                        w += 1;
                                        r = low_next;
                                    }
                                } else {
                                    out[w] = b'?';
                                    w += 1;
                                    r = next;
                                }
                            } else {
                                out[w] = b'?';
                                w += 1;
                                r = next;
                            }
                        }
                        Some(cp) => {
                            let len = utf8_len(cp);
                            if len > 0 && w + len <= out.len() {
                                w = write_utf8(out, w, cp);
                                r = next;
                            } else {
                                out[w] = b'?';
                                w += 1;
                                r = next;
                            }
                        }
                        None => {
                            out[w] = b'?';
                            w += 1;
                            r += 2;
                        }
                    }
                }
                _ => { out[w] = content[r]; w += 1; r += 1; }
            }
        } else if content[r] == b'"' {
            break;
        } else {
            out[w] = content[r];
            w += 1;
            r += 1;
        }
    }
    w
}

/// Reads four hexadecimal digits at `pos` as a Unicode code point.
fn read_hex4(buf: &[u8], pos: usize) -> (Option<u32>, usize) {
    if pos + 4 > buf.len() {
        return (None, pos);
    }
    let mut value = 0u32;
    for i in 0..4 {
        let digit = match buf[pos + i] {
            b'0'..=b'9' => buf[pos + i] - b'0',
            b'a'..=b'f' => buf[pos + i] - b'a' + 10,
            b'A'..=b'F' => buf[pos + i] - b'A' + 10,
            _ => return (None, pos),
        };
        value = value * 16 + digit as u32;
    }
    (Some(value), pos + 4)
}

/// The number of UTF-8 bytes a code point occupies, or zero if it is not a
/// scalar value.
pub fn utf8_len(cp: u32) -> usize {
    match cp {
        0x0000..=0x007F => 1,
        0x0080..=0x07FF => 2,
        0x0800..=0xD7FF => 3,
        0xE000..=0xFFFF => 3,
        0x10000..=0x10FFFF => 4,
        _ => 0,
    }
}

/// Writes a code point as UTF-8 into `out` at `w`, returning the new cursor.
pub fn write_utf8(out: &mut [u8], w: usize, cp: u32) -> usize {
    let len = utf8_len(cp);
    if len == 0 || w + len > out.len() {
        return w;
    }
    match len {
        1 => out[w] = cp as u8,
        2 => {
            out[w] = 0xC0 | (cp >> 6) as u8;
            out[w + 1] = 0x80 | (cp & 0x3F) as u8;
        }
        3 => {
            out[w] = 0xE0 | (cp >> 12) as u8;
            out[w + 1] = 0x80 | ((cp >> 6) & 0x3F) as u8;
            out[w + 2] = 0x80 | (cp & 0x3F) as u8;
        }
        _ => {
            out[w] = 0xF0 | (cp >> 18) as u8;
            out[w + 1] = 0x80 | ((cp >> 12) & 0x3F) as u8;
            out[w + 2] = 0x80 | ((cp >> 6) & 0x3F) as u8;
            out[w + 3] = 0x80 | (cp & 0x3F) as u8;
        }
    }
    w + len
}

/// Parsed HTTP/1.1 Response Header and Body.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HttpResponse<'a> {
    pub status_code: u16,
    pub content_type: &'a [u8],
    pub body: &'a [u8],
}

/// Parses an HTTP/1.1 response stream into status code, Content-Type, and body slice.
pub fn parse_http_response(raw: &[u8]) -> Option<HttpResponse<'_>> {
    if !raw.starts_with(b"HTTP/1.") {
        return None;
    }
    let space1 = raw.iter().position(|&b| b == b' ')?;
    let status_slice = &raw[space1 + 1..];
    if status_slice.len() < 3 {
        return None;
    }
    let code_b = &status_slice[..3];
    if !code_b[0].is_ascii_digit() || !code_b[1].is_ascii_digit() || !code_b[2].is_ascii_digit() {
        return None;
    }
    let status_code = ((code_b[0] - b'0') as u16) * 100
        + ((code_b[1] - b'0') as u16) * 10
        + ((code_b[2] - b'0') as u16);

    let (header_bytes, body) = if let Some(pos) = raw.windows(4).position(|w| w == b"\r\n\r\n") {
        (&raw[..pos], &raw[pos + 4..])
    } else if let Some(pos) = raw.windows(2).position(|w| w == b"\n\n") {
        (&raw[..pos], &raw[pos + 2..])
    } else {
        (raw, &b""[..])
    };

    let mut content_type = &b"text/html"[..];
    for line in header_bytes.split(|&b| b == b'\n') {
        let trimmed = if !line.is_empty() && line[line.len() - 1] == b'\r' {
            &line[..line.len() - 1]
        } else {
            line
        };
        if trimmed.len() >= 13 {
            let prefix = &trimmed[..13];
            let is_ct = prefix.iter().zip(b"content-type:".iter()).all(|(a, b)| a.to_ascii_lowercase() == *b);
            if is_ct {
                let mut ct_val = &trimmed[13..];
                while !ct_val.is_empty() && (ct_val[0] == b' ' || ct_val[0] == b'\t') {
                    ct_val = &ct_val[1..];
                }
                if let Some(semi) = ct_val.iter().position(|&b| b == b';') {
                    ct_val = &ct_val[..semi];
                }
                content_type = ct_val;
                break;
            }
        }
    }

    Some(HttpResponse {
        status_code,
        content_type,
        body,
    })
}

// =============================================================================
// Reader pipeline: HTML -> editorial text
// =============================================================================
//
// WHY this layer exists at all: a web page is a layout, and a phone
// reader wants the article. The pipeline has one job -- turn real markup
// into a clean, linear, readable document that fits the platform font.
//
// Three defects in the previous implementation made it useless against a
// real page, and all three were measured against the live site:
//
//  1. Whitespace was not collapsed. The source markup's indentation
//     between tags became reader lines, so 688 of the 958 lines the live
//     article produced were blank or a single space. With a 32-line
//     document budget, the reader showed ONE line of content.
//  2. Entities were decoded into UTF-8. The platform font covers ASCII
//     32..126 and nothing else, so an em dash became three blank
//     six-pixel boxes. The reader transliterates instead.
//  3. Page chrome was kept. The site's own navigation was seven of the
//     first twenty reader lines, each rendered as a link card.
//
// The rules below are the whole contract.

/// Elements whose content is page furniture rather than article text.
///
/// WHY the list: a reader that keeps a site's navigation, header and
/// footer turns an article into a list of link cards. The live article's
/// navigation was seven of its first twenty reader lines.
const CHROME_TAGS: [&[u8]; 16] = [
    b"nav", b"header", b"footer", b"aside", b"script", b"style", b"noscript", b"svg",
    b"form", b"button", b"select", b"template", b"iframe", b"canvas", b"video", b"object",
];

/// How many nested page-chrome elements the reader can be inside at once.
///
/// WHY this is bounded: the stack is a fixed array, and a page that nests
/// chrome deeper than this simply stops tracking the extra levels. The
/// content of a chrome element is dropped either way.
pub const CHROME_STACK_DEPTH: usize = 16;

/// Elements that end a paragraph and are therefore separated by a blank
/// line in the rendered document.
const PARAGRAPH_TAGS: [&[u8]; 20] = [
    b"p", b"div", b"h1", b"h2", b"h3", b"h4", b"h5", b"h6", b"li", b"section", b"article",
    b"ul", b"ol", b"table", b"tr", b"blockquote", b"pre", b"figure", b"figcaption", b"dl",
];

/// Elements that flow inside a line and never break it.
const INLINE_TAGS: [&[u8]; 20] = [
    b"span", b"b", b"i", b"em", b"strong", b"small", b"sup", b"sub", b"u", b"mark",
    b"abbr", b"time", b"label", b"q", b"cite", b"var", b"kbd", b"samp", b"wbr", b"dfn",
];

/// The transliteration of a Unicode code point into the ASCII subset the
/// platform font can draw.
///
/// WHY the SDK owns this table and not the reader: the platform font covers
/// ASCII 32..126 and nothing else, so a byte outside that range has no
/// glyph and draws as a blank six-pixel gap. Every app that ingests text
/// has to answer the same question -- what does this code point become? --
/// and a per-app table is a per-app answer that will drift. The SDK
/// carries one table, and the reader defers to it.
pub fn transliterate(cp: u32) -> &'static [u8] {
    interos_std::font::transliterate(cp)
}

/// Writes a code point as the ASCII the platform font can draw.
fn push_transliterated(out: &mut [u8], w: &mut usize, cp: u32) {
    if (0x20..=0x7E).contains(&cp) {
        if *w < out.len() {
            out[*w] = cp as u8;
            *w += 1;
        }
        return;
    }
    for &b in transliterate(cp) {
        if *w < out.len() {
            out[*w] = b;
            *w += 1;
        } else {
            break;
        }
    }
}

/// Reads a run of decimal or hexadecimal digits at `pos`.
fn read_digits(buf: &[u8], mut pos: usize, base: u32) -> (Option<u32>, usize) {
    let mut value = 0u32;
    let mut digits = 0usize;
    while pos < buf.len() {
        let digit = match buf[pos] {
            b'0'..=b'9' => (buf[pos] - b'0') as u32,
            b'a'..=b'f' if base == 16 => (buf[pos] - b'a' + 10) as u32,
            b'A'..=b'F' if base == 16 => (buf[pos] - b'A' + 10) as u32,
            _ => break,
        };
        value = value.saturating_mul(base).saturating_add(digit);
        digits += 1;
        pos += 1;
    }
    if digits == 0 {
        (None, pos)
    } else {
        (Some(value), pos)
    }
}

/// Decodes the HTML entity at `pos` into drawable ASCII.
///
/// Returns the position after the entity, or `None` when the text at
/// `pos` is not an entity the reader understands.
fn decode_html_entity(html: &[u8], pos: usize, out: &mut [u8], w: &mut usize) -> Option<usize> {
    if html[pos..].starts_with(b"&#") {
        let (cp, next) = if pos + 2 < html.len() && (html[pos + 2] == b'x' || html[pos + 2] == b'X') {
            read_digits(html, pos + 3, 16)
        } else {
            read_digits(html, pos + 2, 10)
        };
        if let Some(cp) = cp {
            if next < html.len() && html[next] == b';' {
                push_transliterated(out, w, cp);
                return Some(next + 1);
            }
        }
        return None;
    }

    let semi = html[pos..].iter().position(|&b| b == b';').map(|p| pos + p)?;
    if semi <= pos + 1 || semi - pos > 12 {
        return None;
    }
    let name = &html[pos + 1..semi];
    let cp = match name {
        b"amp" => Some(b'&' as u32),
        b"lt" => Some(b'<' as u32),
        b"gt" => Some(b'>' as u32),
        b"quot" => Some(b'"' as u32),
        b"apos" => Some(b'\'' as u32),
        b"nbsp" => Some(0x00A0),
        b"mdash" => Some(0x2014),
        b"ndash" => Some(0x2013),
        b"hellip" => Some(0x2026),
        b"lsquo" => Some(0x2018),
        b"rsquo" => Some(0x2019),
        b"ldquo" => Some(0x201C),
        b"rdquo" => Some(0x201D),
        b"bull" => Some(0x2022),
        b"middot" => Some(0x00B7),
        b"rarr" => Some(0x2192),
        b"larr" => Some(0x2190),
        b"uarr" => Some(0x2191),
        b"darr" => Some(0x2193),
        b"copy" => Some(0x00A9),
        b"reg" => Some(0x00AE),
        b"trade" => Some(0x2122),
        b"deg" => Some(0x00B0),
        b"times" => Some(0x00D7),
        b"divide" => Some(0x00F7),
        b"plusmn" => Some(0x00B1),
        b"frac12" => Some(0x00BD),
        b"frac14" => Some(0x00BC),
        b"frac34" => Some(0x00BE),
        b"eacute" => Some(0x00E9),
        b"egrave" => Some(0x00E8),
        b"agrave" => Some(0x00E0),
        b"uuml" => Some(0x00FC),
        b"ouml" => Some(0x00F6),
        b"auml" => Some(0x00E4),
        b"szlig" => Some(0x00DF),
        b"ccedil" => Some(0x00E7),
        b"ntilde" => Some(0x00F1),
        b"aring" => Some(0x00E5),
        b"aelig" => Some(0x00E6),
        b"oslash" => Some(0x00F8),
        b"laquo" => Some(0x00AB),
        b"raquo" => Some(0x00BB),
        b"euro" => Some(0x20AC),
        b"pound" => Some(0x00A3),
        b"rupee" => Some(0x20B9),
        b"micro" => Some(0x00B5),
        b"sect" => Some(0x00A7),
        b"para" => Some(0x00B6),
        b"dagger" => Some(0x2020),
        b"permil" => Some(0x2030),
        b"prime" => Some(0x2032),
        b"sbquo" => Some(0x201A),
        b"bdquo" => Some(0x201E),
        b"lsaquo" => Some(0x2039),
        b"rsaquo" => Some(0x203A),
        b"le" => Some(0x2264),
        b"ge" => Some(0x2265),
        b"ne" => Some(0x2260),
        b"minus" => Some(0x2212),
        b"shy" => Some(0x00AD),
        b"lowast" => Some(0x2217),
        _ => None,
    };
    let cp = cp?;
    push_transliterated(out, w, cp);
    Some(semi + 1)
}

/// Copies `src` into the output, decoding entities and transliterating.
fn push_decoded(out: &mut [u8], w: &mut usize, src: &[u8]) {
    let mut i = 0usize;
    while i < src.len() {
        if src[i] == b'&' {
            if let Some(next) = decode_html_entity(src, i, out, w) {
                i = next;
                continue;
            }
        }
        if *w < out.len() {
            out[*w] = src[i];
            *w += 1;
        } else {
            break;
        }
        i += 1;
    }
}

/// Finds the value of `name` in a tag's attribute text.
fn attr_value<'a>(tag: &'a [u8], name: &[u8]) -> Option<&'a [u8]> {
    let mut i = 0usize;
    while i < tag.len() {
        if i > 0 && tag[i - 1] != b' ' && tag[i - 1] != b'\t' && tag[i - 1] != b'\n' && tag[i - 1] != b'\r' {
            i += 1;
            continue;
        }
        if tag.len() - i >= name.len() && tag[i..i + name.len()].eq_ignore_ascii_case(name) {
            let mut j = i + name.len();
            while j < tag.len() && (tag[j] == b' ' || tag[j] == b'\t' || tag[j] == b'\n' || tag[j] == b'\r') {
                j += 1;
            }
            if j < tag.len() && tag[j] == b'=' {
                j += 1;
                while j < tag.len() && (tag[j] == b' ' || tag[j] == b'\t' || tag[j] == b'\n' || tag[j] == b'\r') {
                    j += 1;
                }
                if j < tag.len() {
                    if tag[j] == b'"' || tag[j] == b'\'' {
                        let quote = tag[j];
                        let start = j + 1;
                        let mut end = start;
                        while end < tag.len() && tag[end] != quote {
                            end += 1;
                        }
                        return Some(&tag[start..end.min(tag.len())]);
                    } else {
                        let start = j;
                        let mut end = start;
                        while end < tag.len()
                            && tag[end] != b' '
                            && tag[end] != b'\t'
                            && tag[end] != b'\n'
                            && tag[end] != b'\r'
                            && tag[end] != b'>'
                        {
                            end += 1;
                        }
                        if end > start {
                            return Some(&tag[start..end]);
                        }
                    }
                }
            }
        }
        i += 1;
    }
    None
}

/// Whether a payload contains real markup rather than prose that happens
/// to use angle brackets.
///
/// WHY this cannot be a test for the presence of '<': the GNU General
/// Public Licence writes its own preamble as "<one line to give the
/// program's name...>", and a standards document may use '<' as a
/// comparison operator. Both are text. A tag, by contrast, is a '<' that
/// opens a name and is closed by a '>' within a short run of tag-legal
/// bytes.
pub fn looks_like_markup(html: &[u8]) -> bool {
    let mut i = 0usize;
    while i < html.len() {
        if html[i] != b'<' {
            i += 1;
            continue;
        }
        let mut j = i + 1;
        if j < html.len() && html[j] == b'/' {
            j += 1;
        }
        if j < html.len() && html[j].is_ascii_alphabetic() {
            let mut k = j;
            while k < html.len() && k - i < 64 {
                let c = html[k];
                if c == b'>' {
                    return true;
                }
                if !(c.is_ascii_alphanumeric()
                    || c == b' '
                    || c == b'\t'
                    || c == b'\n'
                    || c == b'\r'
                    || c == b'='
                    || c == b'"'
                    || c == b'\''
                    || c == b'-'
                    || c == b'/'
                    || c == b'#'
                    || c == b'.'
                    || c == b':')
                {
                    break;
                }
                k += 1;
            }
        }
        i += 1;
    }
    false
}

/// Whether a tag name is in a fixed list.
fn tag_in(list: &[&[u8]], name: &[u8]) -> bool {
    list.iter().any(|candidate| candidate.eq_ignore_ascii_case(name))
}

/// The index of a tag in the chrome list, when it is one of them.
///
/// WHY an index and not the name: the chrome stack is a fixed array of
/// byte values, so it has to be able to hold a name without borrowing
/// from the page.
fn chrome_tag_index(name: &[u8]) -> Option<usize> {
    CHROME_TAGS.iter().position(|c| c.eq_ignore_ascii_case(name))
}

/// Pushes a newline, unless the output already ends with one.
fn push_newline(out: &mut [u8], w: &mut usize) {
    if *w > 0 && out[*w - 1] == b'\n' {
        return;
    }
    if *w < out.len() {
        out[*w] = b'\n';
        *w += 1;
    }
}

/// Pushes a blank line, unless the output already ends with one.
fn push_blank_line(out: &mut [u8], w: &mut usize) {
    if *w >= 2 && out[*w - 1] == b'\n' && out[*w - 2] == b'\n' {
        return;
    }
    push_newline(out, w);
    if *w < out.len() {
        out[*w] = b'\n';
        *w += 1;
    }
}

/// Separates the next text from what precedes it with a single space.
fn push_separator(out: &mut [u8], w: &mut usize) {
    if *w == 0 {
        return;
    }
    let last = out[*w - 1];
    if last == b' ' || last == b'\n' {
        return;
    }
    if *w < out.len() {
        out[*w] = b' ';
        *w += 1;
    }
}

/// What kind of payload the reader was handed.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PayloadKind {
    /// Markup. Tags are structure, and a source newline is a space.
    Markup,
    /// A plain document. There are no tags, and a source newline is a
    /// line break.
    ///
    /// WHY this has to be told rather than guessed: the GNU General Public
    /// Licence writes its own preamble as "<one line to give the program's
    /// name...>", a standards document uses '<' as a comparison operator,
    /// and a URL in angle brackets is not a tag. No heuristic separates
    /// those from markup, but the content type does -- a server says
    /// `text/plain` and the reader believes it.
    PlainText,
}

/// Whether a content type names a plain-text payload.
///
/// WHY the app needs this: the Device Studio gateway wraps a page in a
/// JSON envelope, so the HTTP content type the browser sees is
/// `application/json` and the page's own type is inside the envelope.
pub fn content_type_is_plain(content_type: &[u8]) -> bool {
    const NEEDLE: &[u8] = b"text/plain";
    let mut start = 0usize;
    while start + NEEDLE.len() <= content_type.len() {
        let mut matched = true;
        for (i, &n) in NEEDLE.iter().enumerate() {
            if content_type[start + i].to_ascii_lowercase() != n {
                matched = false;
                break;
            }
        }
        if matched {
            // The match must be a whole media type, not a prefix of
            // another one.
            let before_ok = start == 0
                || content_type[start - 1] == b' '
                || content_type[start - 1] == b';';
            let after = start + NEEDLE.len();
            let after_ok = after >= content_type.len()
                || content_type[after] == b';'
                || content_type[after] == b' '
                || content_type[after] == b',';
            if before_ok && after_ok {
                return true;
            }
        }
        start += 1;
    }
    false
}

/// Finds the `content_type` the gateway reported for a page.
///
/// WHY the gateway's own type and not the HTTP header: the gateway wraps
/// the page in a JSON envelope, so the response's HTTP content type is
/// `application/json` and the page's real type is inside the envelope.
pub fn proxy_content_type(json: &[u8]) -> Option<&[u8]> {
    let key = b"\"content_type\": \"";
    let key_pos = json.windows(key.len()).position(|w| w == key)?;
    let start = key_pos + key.len();
    let end = json[start..].iter().position(|&b| b == b'"')? + start;
    Some(&json[start..end])
}

/// Returns true when the gateway's JSON envelope reports a failed fetch.
///
/// WHY the envelope is inspected instead of the HTTP status: the gateway
/// answers a failed fetch with HTTP 200 and a `{"success": false, ...}`
/// body. Rendering that body as a page showed the envelope's own JSON as
/// if it were the site's text.
pub fn proxy_reports_failure(json: &[u8]) -> bool {
    const SPACED: &[u8] = b"\"success\": false";
    const COMPACT: &[u8] = b"\"success\":false";
    json.windows(SPACED.len()).any(|w| w == SPACED)
        || json.windows(COMPACT.len()).any(|w| w == COMPACT)
}

/// Renders a plain-text document into the reader's line format.
///
/// WHY this is not the markup path: a plain document has no tags, and
/// collapsing its newlines into spaces turned a 35 KB licence into ONE
/// line. A document that is not markup is text, and text has lines.
pub fn reader_text_from_plain(text: &[u8], out: &mut [u8]) -> usize {
    let mut w = 0usize;
    let mut i = 0usize;
    let mut pending_space = false;

    while i < text.len() {
        let b = text[i];

        if b == b'\n' {
            push_newline(out, &mut w);
            pending_space = false;
            i += 1;
            continue;
        }
        if b == b'\r' {
            i += 1;
            continue;
        }
        if b == b' ' || b == b'\t' || b == 0x0C {
            pending_space = true;
            i += 1;
            continue;
        }
        if b == b'&' {
            if let Some(next) = decode_html_entity(text, i, out, &mut w) {
                pending_space = false;
                i = next;
                continue;
            }
        }
        if pending_space {
            pending_space = false;
            push_separator(out, &mut w);
        }
        if b < 0x80 {
            push_byte(out, &mut w, b);
            i += 1;
            continue;
        }
        let (cp, next) = decode_utf8_at(text, i);
        if cp >= 0x80 {
            push_transliterated(out, &mut w, cp);
        }
        i = next;
    }
    w
}

/// Extracts readable editorial content from raw HTML into clean Markdown / Gemtext
/// lines without heap allocation.
///
/// The result is a sequence of `\n`-separated lines using a small
/// vocabulary: `# ` for a heading, `* ` for a list item, `> ` for a quote,
/// "```" for a code fence, `=> URL label` for a link, and plain text
/// otherwise. Every byte of the result is in the range the platform font
/// can draw.
pub fn html_to_reader_document(html: &[u8], out: &mut [u8]) -> usize {
    // A payload with no markup at all is text, and its newlines are lines.
    let kind = if looks_like_markup(html) {
        PayloadKind::Markup
    } else {
        PayloadKind::PlainText
    };
    html_to_reader_document_kind(html, kind, out)
}

/// How many nested styled blocks the converter tracks.
///
/// WHY this is bounded: the stack is a fixed array, and a document that
/// nests blocks deeper than this keeps the style of the deepest frame it
/// could push. It never allocates and never panics.
pub const STYLE_STACK_DEPTH: usize = 12;

/// One open block element while the reader document is being built.
#[derive(Clone, Copy)]
struct StyleFrame<'a> {
    tag: &'a [u8],
    class: &'a [u8],
    id: &'a [u8],
    style: css::ComputedStyle,
}

impl<'a> StyleFrame<'a> {
    const EMPTY: Self = Self {
        tag: b"",
        class: b"",
        id: b"",
        style: css::ComputedStyle::DEFAULT,
    };
}

fn emit_style_marker(out: &mut [u8], w: &mut usize, style: &css::ComputedStyle) {
    // A marker lives on its own line so the plain-text reader contract
    // (one line is one run of visible text) stays true for every line
    // that is not a marker.
    push_newline(out, w);
    let mut marker = [0u8; 32];
    let n = css::encode_style_marker(style, &mut marker);
    push_slice(out, w, &marker[..n]);
    push_newline(out, w);
}

/// Opens a block: computes its style from the stylesheet, the inline
/// `style` attribute and the parent's inherited colour, then records a
/// style marker only when the effective style actually changed.
fn open_block<'a>(
    out: &mut [u8],
    w: &mut usize,
    sheet: &css::Stylesheet<'a>,
    frames: &mut [StyleFrame<'a>; STYLE_STACK_DEPTH],
    depth: &mut usize,
    tag: &'a [u8],
    tag_content: &'a [u8],
    active: &mut css::ComputedStyle,
) {
    if *depth >= STYLE_STACK_DEPTH {
        return;
    }
    let class = attr_value(tag_content, b"class").unwrap_or(b"");
    let id = attr_value(tag_content, b"id").unwrap_or(b"");
    let inline = attr_value(tag_content, b"style");
    let mut ancestors = [css::Compound::EMPTY; STYLE_STACK_DEPTH];
    for (slot, frame) in ancestors.iter_mut().zip(frames.iter()).take(*depth) {
        *slot = css::Compound {
            tag: frame.tag,
            class: frame.class,
            id: frame.id,
        };
    }
    let element = css::Compound { tag, class, id };
    let mut style = css::cascade(sheet, element, &ancestors[..*depth], inline);
    // Colour and font size inherit in CSS. Background, border, margin
    // and padding do not, and the reader does not invent that they do.
    if !style.has_color && active.has_color {
        style.color = active.color;
        style.has_color = true;
    }
    if style.font_scale == 0 {
        style.font_scale = active.font_scale;
    }
    frames[*depth] = StyleFrame {
        tag,
        class,
        id,
        style,
    };
    *depth += 1;
    if style != *active {
        emit_style_marker(out, w, &style);
        *active = style;
    }
}

/// Closes a block, restoring the enclosing style.
fn close_block(
    out: &mut [u8],
    w: &mut usize,
    frames: &[StyleFrame<'_>; STYLE_STACK_DEPTH],
    depth: &mut usize,
    tag: &[u8],
    active: &mut css::ComputedStyle,
) {
    if *depth == 0 || !frames[*depth - 1].tag.eq_ignore_ascii_case(tag) {
        // A close tag that does not match the open block is ignored,
        // exactly as the chrome stack ignores one: the two ends of an
        // element only agree when they name each other.
        return;
    }
    *depth -= 1;
    let restored = if *depth > 0 {
        frames[*depth - 1].style
    } else {
        css::ComputedStyle::DEFAULT
    };
    if restored != *active {
        emit_style_marker(out, w, &restored);
        *active = restored;
    }
}

/// Reads a `width` or `height` attribute as a pixel count.
fn dimension_attr(tag: &[u8], name: &[u8]) -> Option<u16> {
    let raw = attr_value(tag, name)?;
    let mut value: u32 = 0;
    let mut any = false;
    for &b in raw {
        if b.is_ascii_digit() {
            value = value.saturating_mul(10).saturating_add((b - b'0') as u32);
            any = true;
        } else {
            break;
        }
    }
    if !any {
        return None;
    }
    Some(value.min(u16::MAX as u32) as u16)
}

/// Finds the matching close tag for a raw-text container element (`script`, `style`, `textarea`).
fn find_raw_text_close(html: &[u8], start: usize, tag: &[u8]) -> Option<(usize, usize)> {
    let mut scan = start;
    let n = html.len();
    while scan + 2 < n {
        if html[scan] == b'<' && html[scan + 1] == b'/' {
            let rest = &html[scan + 2..];
            if rest.len() >= tag.len() && rest[..tag.len()].eq_ignore_ascii_case(tag) {
                let mut after = scan + 2 + tag.len();
                while after < n && (html[after] == b' ' || html[after] == b'\t' || html[after] == b'\r' || html[after] == b'\n') {
                    after += 1;
                }
                if after < n && html[after] == b'>' {
                    return Some((scan, after + 1));
                }
            }
        }
        scan += 1;
    }
    None
}

/// Extracts readable editorial content from a payload of a known kind.
pub fn html_to_reader_document_kind(html: &[u8], kind: PayloadKind, out: &mut [u8]) -> usize {
    if kind == PayloadKind::PlainText {
        return reader_text_from_plain(html, out);
    }
    let mut r = 0usize;
    let mut w = 0usize;
    let n = html.len();

    // The stylesheet a `<style>` element carried, and the block-style
    // stack that gives descendant selectors their ancestors.
    let mut sheet = css::Stylesheet::empty();
    let mut sheet_parsed = false;
    let mut style_frames: [StyleFrame<'_>; STYLE_STACK_DEPTH] =
        [StyleFrame::EMPTY; STYLE_STACK_DEPTH];
    let mut style_depth = 0usize;
    let mut active_style = css::ComputedStyle::DEFAULT;
    // How many `<img>` elements have been recorded. The rest keep the
    // reader's textual alt fallback rather than growing the document.
    let mut images_recorded = 0usize;

    // The open page-chrome elements. Everything inside them is furniture,
    // not article text. The stack is what makes an open and a close agree.
    let mut chrome_stack: [u8; CHROME_STACK_DEPTH] = [0u8; CHROME_STACK_DEPTH];
    let mut chrome_depth = 0usize;
    // Whether whitespace in the source has been seen and is owed to the
    // next piece of text as a single separating space.
    let mut pending_space = false;
    // Whether the current anchor is a same-page fragment, which is not a
    // navigation target and must not become a link card.
    let mut in_fragment_link = false;
    // Whether the current anchor is an accessibility skip link, whose text
    // is dropped entirely.
    let mut in_skip_link = false;
    // The depth of open heading elements, so a line break inside a title
    // is a space rather than a new document line.
    let mut heading_depth = 0usize;
    // Whether the payload is a document rather than markup.
    //
    // WHY this is decided by the absence of markup: a standards document
    // or a licence is plain text, and its line structure is the content.
    // Treating it as markup -- collapsing every newline into a space --
    // produced a single line the length of the whole file.
    let plain_text = false;
    // Whether the current anchor is a real link being emitted.
    let mut in_link = false;

    while r < n {
        // 1. Comments.
        if html[r..].starts_with(b"<!--") {
            r += 4;
            while r + 2 < n && !html[r..].starts_with(b"-->") {
                r += 1;
            }
            if r + 2 < n {
                r += 3;
            } else {
                r = n;
            }
            continue;
        }

        // 2. Tags.
        if html[r] == b'<' {
            let tag_start = r + 1;
            let mut tag_end = tag_start;
            while tag_end < n && html[tag_end] != b'>' {
                tag_end += 1;
            }
            let tag_content = &html[tag_start..tag_end.min(n)];
            let is_closing = tag_content.starts_with(b"/");
            let tag_name = if is_closing { &tag_content[1..] } else { tag_content };
            let tag_word = tag_name
                .split(|&b| b == b' ' || b == b'\t' || b == b'\n' || b == b'\r' || b == b'/')
                .next()
                .unwrap_or(b"");
            let next = if tag_end < n { tag_end + 1 } else { n };

            // 2a. Raw-text elements. Their content is not markup, so it
            // is skipped to the matching close tag rather than parsed.
            //
            // WHY this matters: the live page carries a 7 KB
            // application/ld+json block whose text mentions "UserPtr<T>".
            // A parser that treats that "<" as a tag reads the next ">"
            // as the end of a tag, swallows a chunk of the document, and
            // dumps the JSON into the article.
            if tag_word.eq_ignore_ascii_case(b"script")
                || tag_word.eq_ignore_ascii_case(b"style")
                || tag_word.eq_ignore_ascii_case(b"textarea")
            {
                // WHY an external script is not a raw-text container: a
                // `<script src="...">` tag has no closing tag in HTML. The
                // raw-text skip searched for the next "</script>" and
                // therefore swallowed everything between the module
                // bundle and the next script -- the site's whole header,
                // navigation and hero with it. An external or
                // self-closing script is a void element: the tag is the
                // whole thing.
                let is_external = attr_value(tag_content, b"src").is_some()
                    || tag_content.ends_with(b"/");
                if is_external {
                    r = next;
                    continue;
                }
                if !is_closing {
                    let close_tag: &[u8] = if tag_word.eq_ignore_ascii_case(b"script") {
                        b"script"
                    } else if tag_word.eq_ignore_ascii_case(b"style") {
                        b"style"
                    } else {
                        b"textarea"
                    };
                    let (scan, next_r) = match find_raw_text_close(html, next, close_tag) {
                        Some((s, e)) => (s, e),
                        None => (n, n),
                    };
                    // An inline stylesheet is parsed before it is skipped
                    // as raw text. Only the first one is read, and only
                    // its first MAX_CSS_BYTES: a page that carries a
                    // framework bundle in a second <style> does not get
                    // to grow this app's work without bound.
                    if !sheet_parsed && tag_word.eq_ignore_ascii_case(b"style") {
                        let css_end = scan.min(n).min(next.saturating_add(css::MAX_CSS_BYTES));
                        sheet = css::parse_stylesheet(&html[next..css_end]);
                        sheet_parsed = true;
                    }
                    r = next_r;
                    continue;
                }
                r = next;
                continue;
            }

            // 2b. Page chrome. The content is dropped as well as the tags
            // -- a reader that keeps the text of a navigation menu is not
            // a reader.
            //
            // WHY the open tags are stacked rather than counted: a
            // counting depth is asymmetric. A `<nav>` inside a `<header>`
            // never incremented the count -- its open tag was skipped
            // because the count was already positive -- but its `</nav>`
            // still decremented it. The count reached zero in the middle
            // of the header, and the site's GitHub link and call-to-action
            // leaked into the article.
            //
            // The stack makes the two ends agree: an open tag pushes, and
            // a close tag only pops when it matches what is on top.
            if let Some(index) = chrome_tag_index(tag_word) {
                if !is_closing {
                    if chrome_depth < CHROME_STACK_DEPTH {
                        chrome_stack[chrome_depth] = index as u8;
                        chrome_depth += 1;
                    }
                } else if chrome_depth > 0
                    && chrome_stack[chrome_depth - 1] as usize == index
                {
                    chrome_depth -= 1;
                }
                r = next;
                continue;
            }
            if chrome_depth > 0 {
                r = next;
                continue;
            }

            // 2b. Void elements.
            if tag_word.eq_ignore_ascii_case(b"br") {
                // WHY a space inside a heading: a heading is one title
                // however the source wraps it. The live article's <h1>
                // contains two <br> elements, and treating them as line
                // breaks split one title into three document lines, so
                // the reader showed a fragment of the heading and then
                // two lines that looked like body copy.
                if heading_depth > 0 {
                    // A space inside a title, owed to the next text.
                    pending_space = true;
                } else {
                    push_newline(out, &mut w);
                    pending_space = false;
                }
                r = next;
                continue;
            }
            if tag_word.eq_ignore_ascii_case(b"hr") {
                push_blank_line(out, &mut w);
                pending_space = false;
                r = next;
                continue;
            }
            if tag_word.eq_ignore_ascii_case(b"img") {
                let src = attr_value(tag_content, b"src").unwrap_or(b"");
                let alt = attr_value(tag_content, b"alt").unwrap_or(b"");
                // Up to MAX_INLINE_IMAGES elements become placeable
                // markers. The rest keep the alt text, so a gallery does
                // not consume the reader's whole document budget. A source
                // the marker cannot carry keeps its alt text too: a cut
                // URL would name a different resource.
                let mut marker = [0u8; 160];
                let n = if !src.is_empty()
                    && src.len() <= 64
                    && images_recorded < images::MAX_INLINE_IMAGES
                {
                    images::encode_image_marker(
                        src,
                        alt,
                        dimension_attr(tag_content, b"width"),
                        dimension_attr(tag_content, b"height"),
                        &mut marker,
                    )
                } else {
                    0
                };
                if n > 0 {
                    push_newline(out, &mut w);
                    push_slice(out, &mut w, &marker[..n]);
                    push_newline(out, &mut w);
                    images_recorded += 1;
                    pending_space = false;
                } else if !alt.is_empty() {
                    push_separator(out, &mut w);
                    push_byte(out, &mut w, b'[');
                    push_decoded(out, &mut w, alt);
                    push_byte(out, &mut w, b']');
                    pending_space = true;
                }
                r = next;
                continue;
            }

            // 2c. Headings.
            if tag_word.eq_ignore_ascii_case(b"h1")
                || tag_word.eq_ignore_ascii_case(b"h2")
                || tag_word.eq_ignore_ascii_case(b"h3")
                || tag_word.eq_ignore_ascii_case(b"h4")
                || tag_word.eq_ignore_ascii_case(b"h5")
                || tag_word.eq_ignore_ascii_case(b"h6")
                || tag_word.eq_ignore_ascii_case(b"title")
            {
                if !is_closing {
                    open_block(
                        out,
                        &mut w,
                        &sheet,
                        &mut style_frames,
                        &mut style_depth,
                        tag_word,
                        tag_content,
                        &mut active_style,
                    );
                    heading_depth += 1;
                    push_newline(out, &mut w);
                    let hashes = match tag_word {
                        b"h1" | b"title" => 1,
                        b"h2" => 2,
                        b"h3" => 3,
                        b"h4" => 4,
                        b"h5" => 5,
                        _ => 6,
                    };
                    for _ in 0..hashes {
                        push_byte(out, &mut w, b'#');
                    }
                    push_byte(out, &mut w, b' ');
                    pending_space = false;
                } else {
                    if heading_depth > 0 {
                        heading_depth -= 1;
                    }
                    push_blank_line(out, &mut w);
                    close_block(
                        out,
                        &mut w,
                        &style_frames,
                        &mut style_depth,
                        tag_word,
                        &mut active_style,
                    );
                    pending_space = false;
                }
                r = next;
                continue;
            }

            // 2d. Links.
            if tag_word.eq_ignore_ascii_case(b"a") {
                if !is_closing {
                    let href = attr_value(tag_content, b"href").unwrap_or(b"");
                    let is_fragment = href.starts_with(b"#");
                    // WHY the skip-link case: a "skip to content" link is
                    // an accessibility affordance that is visually hidden
                    // on the site and must not become the reader's first
                    // line.
                    let is_skip_link = attr_value(tag_content, b"class")
                        .map(|c| c.windows(4).any(|w| w.eq_ignore_ascii_case(b"skip")))
                        .unwrap_or(false);
                    if is_skip_link {
                        // WHY the text is dropped, not just the link: a
                        // "skip to content" link is an accessibility
                        // affordance that the site hides visually, and
                        // leaving its label in the article put "Skip to
                        // content" at the top of every page.
                        in_skip_link = true;
                    } else if is_fragment || href.is_empty() {
                        in_fragment_link = true;
                    } else {
                        push_newline(out, &mut w);
                        push_slice(out, &mut w, b"=> ");
                        push_decoded(out, &mut w, href);
                        push_byte(out, &mut w, b' ');
                        in_link = true;
                    }
                    pending_space = false;
                } else {
                    if in_link {
                        push_newline(out, &mut w);
                        in_link = false;
                    } else if in_fragment_link {
                        push_separator(out, &mut w);
                    }
                    in_fragment_link = false;
                    in_skip_link = false;
                    pending_space = false;
                }
                r = next;
                continue;
            }

            // 2e. Lists.
            if tag_word.eq_ignore_ascii_case(b"li") {
                if !is_closing {
                    open_block(
                        out,
                        &mut w,
                        &sheet,
                        &mut style_frames,
                        &mut style_depth,
                        tag_word,
                        tag_content,
                        &mut active_style,
                    );
                    push_newline(out, &mut w);
                    push_slice(out, &mut w, b"* ");
                    pending_space = false;
                } else {
                    push_newline(out, &mut w);
                    close_block(
                        out,
                        &mut w,
                        &style_frames,
                        &mut style_depth,
                        tag_word,
                        &mut active_style,
                    );
                    pending_space = false;
                }
                r = next;
                continue;
            }

            // 2f. Code blocks.
            if tag_word.eq_ignore_ascii_case(b"pre") || tag_word.eq_ignore_ascii_case(b"code") {
                if !is_closing {
                    push_newline(out, &mut w);
                    push_slice(out, &mut w, b"```\n");
                } else {
                    push_slice(out, &mut w, b"\n```\n");
                }
                pending_space = false;
                r = next;
                continue;
            }

            // 2g. Quotes.
            if tag_word.eq_ignore_ascii_case(b"blockquote") {
                if !is_closing {
                    open_block(
                        out,
                        &mut w,
                        &sheet,
                        &mut style_frames,
                        &mut style_depth,
                        tag_word,
                        tag_content,
                        &mut active_style,
                    );
                    push_newline(out, &mut w);
                    push_slice(out, &mut w, b"> ");
                    pending_space = false;
                } else {
                    push_blank_line(out, &mut w);
                    close_block(
                        out,
                        &mut w,
                        &style_frames,
                        &mut style_depth,
                        tag_word,
                        &mut active_style,
                    );
                    pending_space = false;
                }
                r = next;
                continue;
            }

            // 2h. Blocks and inline elements.
            if tag_in(&PARAGRAPH_TAGS, tag_word) {
                if !is_closing {
                    open_block(
                        out,
                        &mut w,
                        &sheet,
                        &mut style_frames,
                        &mut style_depth,
                        tag_word,
                        tag_content,
                        &mut active_style,
                    );
                    push_newline(out, &mut w);
                } else {
                    push_blank_line(out, &mut w);
                    close_block(
                        out,
                        &mut w,
                        &style_frames,
                        &mut style_depth,
                        tag_word,
                        &mut active_style,
                    );
                }
                pending_space = false;
                r = next;
                continue;
            }
            if tag_in(&INLINE_TAGS, tag_word) {
                // WHY a separator is owed on BOTH ends: the source markup
                // puts adjacent inline elements together with no whitespace
                // between them. Inserting the space only when the element
                // closed fixed "<span>InterOS</span><span>A first</span>"
                // but not "<span>InterOS<span>A first</span>", where the
                // text sits inside the element. The live article's own
                // title came out as "InterOSA security-first" until this
                // was made symmetric.
                //
                // The separator is owed rather than written, so that the
                // next piece of text decides whether one is needed -- an
                // element that ends a line, or is empty, adds nothing.
                pending_space = true;
                r = next;
                continue;
            }

            r = next;
            continue;
        }

        // 3. Entities.
        if html[r] == b'&' {
            if let Some(next) = decode_html_entity(html, r, out, &mut w) {
                pending_space = false;
                r = next;
                continue;
            }
        }

        // 4. Text.
        //
        // WHY chrome content is dropped here and not only at the tag: the
        // tag branch skips the markup of a chrome element, but its text
        // was still reaching the document. The site's own navigation --
        // seven links and a wordmark -- was the reader's first content.
        if chrome_depth > 0 {
            r += 1;
            continue;
        }
        let b = html[r];
        if b == b'\n' && plain_text {
            // WHY a plain document keeps its line breaks: a standards
            // document, a licence or an RFC has no markup at all, and
            // collapsing its newlines turned the whole file into ONE
            // 32 KB line. A document that is not markup is text, and text
            // has lines.
            push_newline(out, &mut w);
            pending_space = false;
            r += 1;
            continue;
        }
        if b == b' ' || b == b'\t' || b == b'\n' || b == b'\r' || b == 0x0C {
            // WHY whitespace is collapsed rather than written: the source
            // markup's indentation between tags became reader lines, and
            // 688 of the 958 lines the live article produced were blank.
            pending_space = true;
            r += 1;
            continue;
        }

        if pending_space {
            pending_space = false;
            push_separator(out, &mut w);
        }
        if in_skip_link {
            r += 1;
            continue;
        }

        // 5. Non-ASCII text is transliterated into what the font can draw.
        if b < 0x80 {
            push_byte(out, &mut w, b);
            r += 1;
            continue;
        }

        let (cp, next) = decode_utf8_at(html, r);
        if cp >= 0x80 {
            push_transliterated(out, &mut w, cp);
        } else {
            // A byte the font cannot draw at all: drop it rather than
            // leave a blank gap on the page.
        }
        r = next;
    }

    w
}

/// Decodes the UTF-8 sequence at `pos` into a code point.
///
/// Returns the code point and the position after it. An invalid sequence
/// yields the byte value itself and advances by one, so the reader never
/// loops.
pub fn decode_utf8_at(buf: &[u8], pos: usize) -> (u32, usize) {
    if pos >= buf.len() {
        return (0, pos);
    }
    let b0 = buf[pos];
    if b0 < 0x80 {
        return (b0 as u32, pos + 1);
    }
    let len = if b0 & 0xE0 == 0xC0 {
        2
    } else if b0 & 0xF0 == 0xE0 {
        3
    } else if b0 & 0xF8 == 0xF0 {
        4
    } else {
        return (0, pos + 1);
    };
    if pos + len > buf.len() {
        return (0, pos + 1);
    }
    let mut cp = match len {
        2 => (b0 & 0x1F) as u32,
        3 => (b0 & 0x0F) as u32,
        _ => (b0 & 0x07) as u32,
    };
    for i in 1..len {
        let cont = buf[pos + i];
        if cont & 0xC0 != 0x80 {
            return (0, pos + 1);
        }
        cp = (cp << 6) | (cont & 0x3F) as u32;
    }
    let valid = match len {
        2 => cp >= 0x80,
        3 => cp >= 0x0800 && !(0xD800..=0xDFFF).contains(&cp),
        4 => (0x10000..=0x10FFFF).contains(&cp),
        _ => false,
    };
    if !valid {
        return (0, pos + 1);
    }
    (cp, pos + len)
}

/// Pushes one byte into the output.
fn push_byte(out: &mut [u8], w: &mut usize, b: u8) {
    if *w < out.len() {
        out[*w] = b;
        *w += 1;
    }
}

/// Pushes a slice into the output.
fn push_slice(out: &mut [u8], w: &mut usize, slice: &[u8]) {
    for &b in slice {
        if *w < out.len() {
            out[*w] = b;
            *w += 1;
        } else {
            break;
        }
    }
}

/// Parses a Gemtext link line `=> URL [optional label]` into (target_url, label).
pub fn parse_gemtext_link(line: &[u8]) -> Option<(&[u8], &[u8])> {
    let mut i = 0;
    while i < line.len() && (line[i] == b' ' || line[i] == b'\t') {
        i += 1;
    }
    let trimmed = &line[i..];
    if !trimmed.starts_with(b"=>") {
        return None;
    }
    let rest = &trimmed[2..];
    let mut r = 0;
    while r < rest.len() && (rest[r] == b' ' || rest[r] == b'\t') {
        r += 1;
    }
    let target_and_label = &rest[r..];
    if target_and_label.is_empty() {
        return None;
    }
    match target_and_label.iter().position(|&b| b == b' ' || b == b'\t') {
        Some(idx) => {
            let tgt = &target_and_label[..idx];
            let mut lbl_start = idx;
            while lbl_start < target_and_label.len()
                && (target_and_label[lbl_start] == b' ' || target_and_label[lbl_start] == b'\t')
            {
                lbl_start += 1;
            }
            Some((tgt, &target_and_label[lbl_start..]))
        }
        None => Some((target_and_label, &b""[..])),
    }
}

/// Fast line iterator for structured documents without dynamic allocation.
pub struct DocumentTokenizer<'a> {
    remaining: &'a [u8],
}

impl<'a> DocumentTokenizer<'a> {
    pub const fn new(content: &'a [u8]) -> Self {
        Self { remaining: content }
    }

    pub fn next_line(&mut self) -> Option<(&'a [u8], LineKind)> {
        if self.remaining.is_empty() {
            return None;
        }
        let (line, rest) = match self.remaining.iter().position(|&b| b == b'\n') {
            Some(idx) => (&self.remaining[..idx], &self.remaining[idx + 1..]),
            None => (self.remaining, &b""[..]),
        };
        self.remaining = rest;
        // WHY the trailing whitespace is trimmed here: the pipeline inserts
        // a separating space after an inline element, so a line could end
        // with a space that the reader would draw as a trailing gap.
        let mut end = line.len();
        while end > 0 && (line[end - 1] == b' ' || line[end - 1] == b'\r' || line[end - 1] == b'\t') {
            end -= 1;
        }
        let clean_line = &line[..end];
        let kind = classify_line(clean_line);
        Some((clean_line, kind))
    }
}

/// Width of the URL the start page's page list shows one row at.
pub const URL_FIELD: usize = 64;

/// Copies a URL into the start page's fixed-width field, dropping one trailing
/// `/` so a bare origin does not render as `host./`.
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

pub const MAX_TABS: usize = 4;
pub const MAX_URL_LEN: usize = 128;
pub const MAX_TITLE_LEN: usize = 64;
pub const TAB_HISTORY_CAP: usize = 8;
pub const MAX_BOOKMARKS: usize = 16;
pub const MAX_HISTORY_ENTRIES: usize = 8;
/// The most document lines the reader holds.
///
/// WHY it is a thousand and not sixty-four: the reader used to stop after
/// sixty-four lines and tell the reader that it was showing the first
/// sixty-four of the rest. A page that is longer than that was therefore
/// a page nobody could finish. A thousand lines hold the whole of a
/// documentation page, and the reader scrolls through them.
pub const MAX_DOC_LINES: usize = 1024;
pub const MAX_LINE_BYTES: usize = 96;

/// The largest raw HTML slice the browser keeps for the reader.
///
/// WHY this is a published constant rather than a local array size: the
/// network path, the reader pipeline and the contract test all have to
/// agree on one capacity. When the sizes were separate locals, the app
/// truncated a 57 KB page at 2 KB and nothing in the build said so.
pub const MAX_HTML_BYTES: usize = 128 * 1024;

/// The largest reader document the browser keeps.
///
/// WHY 64 KB: the live article produces 10.6 KB of reader text, and a
/// real documentation page produces more than 32 KB. The earlier 8 KB
/// budget silently dropped the last third of an article -- the part after
/// the fold, which is the part people read -- and a 32 KB budget still
/// truncated the Rust book.
pub const MAX_READER_BYTES: usize = 64 * 1024;

/// The reader's content column, in pixels.
///
/// WHY 24 pixels of padding on each side: a reading surface with text
/// running to the screen edge is unreadable on a phone, and the mission
/// specification calls for 24px horizontal padding. The value is
/// published so the wrapper and the renderer cannot drift apart.
pub const CONTENT_PADDING: usize = 24;

/// The width available for reader text, in pixels.
///
/// WHY this is stated rather than derived: the reader's text column is the
/// 720-pixel surface less a 24-pixel margin on each side. The app asserts
/// that its own width agrees, so the two cannot drift apart.
pub const CONTENT_WIDTH: usize = 672;

/// How many pieces a single document line may be broken into.
pub const MAX_WRAP_PIECES: usize = 6;

/// Wraps a line of text to a pixel width.
///
/// WHY the reader wraps rather than trusting the source: a line of prose
/// has no line breaks in it, so without wrapping a long sentence runs off
/// the right edge of the screen and is clipped mid-glyph.
///
/// The break is at a word boundary where one is available, and inside a
/// word only when a single word is longer than the column. A break never
/// lands inside a multi-byte character.
pub fn wrap_line(
    text: &[u8],
    scale: usize,
    max_w: usize,
    out: &mut [[u8; MAX_LINE_BYTES]; MAX_WRAP_PIECES],
    lens: &mut [usize; MAX_WRAP_PIECES],
) -> usize {
    if text.is_empty() {
        return 0;
    }
    let mut pieces = 0usize;
    let mut start = 0usize;

    while start < text.len() && pieces < MAX_WRAP_PIECES {
        // Measure the rest of the line from `start`.
        let rest = &text[start..];
        if std::design::font::measure(rest, scale) <= max_w && rest.len() <= MAX_LINE_BYTES {
            let n = rest.len().min(MAX_LINE_BYTES);
            out[pieces][..n].copy_from_slice(&rest[..n]);
            lens[pieces] = n;
            pieces += 1;
            break;
        }

        // Find the last word boundary that fits.
        let mut width = 0usize;
        let mut last_space: Option<usize> = None;
        let mut j = 0usize;
        while j < rest.len() {
            let mut k = j;
            while k < rest.len() && rest[k] != b' ' {
                k += 1;
            }
            let word = &rest[j..k];
            let word_w = std::design::font::measure(word, scale);
            let space_w = if j == 0 { 0 } else { std::design::font::measure(b" ", scale) };
            if (width + space_w + word_w > max_w || k > MAX_LINE_BYTES) && j > 0 {
                // This word does not fit. Break before it.
                break;
            }
            width += space_w + word_w;
            if k < rest.len() {
                if k <= MAX_LINE_BYTES {
                    last_space = Some(k);
                }
                j = k + 1;
            } else {
                break;
            }
        }

        let cut = match last_space {
            Some(space) => space,
            None => {
                // A single word longer than the column: break it inside the
                // word, at a character boundary.
                let mut w = 0usize;
                let mut k = 0usize;
                while k < rest.len() {
                    let step = utf8_char_len(rest[k]);
                    if k + step > MAX_LINE_BYTES && k > 0 {
                        break;
                    }
                    let chunk = &rest[k..(k + step).min(rest.len())];
                    let cw = std::design::font::measure(chunk, scale);
                    if w + cw > max_w && k > 0 {
                        break;
                    }
                    w += cw;
                    k += step;
                }
                k.max(1).min(MAX_LINE_BYTES)
            }
        };

        let piece = &rest[..cut.min(MAX_LINE_BYTES)];
        let n = piece.len().min(MAX_LINE_BYTES);
        out[pieces][..n].copy_from_slice(&piece[..n]);
        lens[pieces] = n;
        pieces += 1;

        // Skip the space that was broken at.
        let mut next = start + cut;
        while next < text.len() && text[next] == b' ' {
            next += 1;
        }
        if next == start {
            next += 1;
        }
        start = next;
    }
    pieces
}

/// The length in bytes of the UTF-8 character that starts at `b`.
fn utf8_char_len(b: u8) -> usize {
    if b < 0x80 {
        1
    } else if b & 0xE0 == 0xC0 {
        2
    } else if b & 0xF0 == 0xE0 {
        3
    } else if b & 0xF8 == 0xF0 {
        4
    } else {
        1
    }
}


/// Parsed and classified line of content for document rendering.
#[derive(Debug, Clone, Copy)]
pub struct DocLine {
    pub bytes: [u8; MAX_LINE_BYTES],
    pub len: usize,
    pub kind: LineKind,
    pub link_target: [u8; 64],
    pub link_target_len: usize,
    pub link_label: [u8; 64],
    pub link_label_len: usize,
    /// Whether this line is the continuation of the line above it.
    ///
    /// WHY this is carried rather than inferred: a wrapped heading is one
    /// heading, and the reader draws a rule under a heading and a bullet
    /// beside a list item once. A renderer that cannot tell a continuation
    /// from a new item draws a second rule in the middle of a title.
    pub continued: bool,
    /// The CSS subset's computed style for the block this line belongs to.
    pub style: css::ComputedStyle,
    /// Whether this line opens or closes a styled block.
    ///
    /// WHY they are computed after the whole document is read: a top
    /// margin belongs above the first line of a block, and a border's
    /// bottom edge below the last, and neither end is known from one
    /// line alone.
    pub block_start: bool,
    pub block_end: bool,
    /// The placed image this line refers to, or 255 for none.
    pub image_index: u8,
    /// The style epoch, used to find block boundaries. Zero means the
    /// document carried no style markers at all.
    pub style_epoch: u16,
    /// The height this line reserves when it is an image line.
    pub layout_h: u16,
}

impl DocLine {
    pub const EMPTY: Self = Self {
        bytes: [0; MAX_LINE_BYTES],
        len: 0,
        kind: LineKind::Empty,
        link_target: [0; 64],
        link_target_len: 0,
        link_label: [0; 64],
        link_label_len: 0,
        continued: false,
        style: css::ComputedStyle::DEFAULT,
        block_start: false,
        block_end: false,
        image_index: 255,
        style_epoch: 0,
        layout_h: 0,
    };
}


/// Document loading state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoadingState {
    Idle,
    Connecting,
    Receiving,
    Completed,
    Failed,
}

/// Individual browser tab state.
#[derive(Debug, Clone, Copy)]
pub struct Tab {
    pub tab_id: u32,
    pub active_url: [u8; MAX_URL_LEN],
    pub active_url_len: usize,
    pub title: [u8; MAX_TITLE_LEN],
    pub title_len: usize,
    pub scroll_offset: usize,
    pub loading_state: LoadingState,
    pub error_code: i32,
    pub is_open: bool,
    pub history: [[u8; MAX_URL_LEN]; TAB_HISTORY_CAP],
    pub history_lens: [usize; TAB_HISTORY_CAP],
    pub history_len: usize,
    pub history_index: usize,
}

impl Tab {
    pub const EMPTY: Self = Self {
        tab_id: 0,
        active_url: [0; MAX_URL_LEN],
        active_url_len: 0,
        title: [0; MAX_TITLE_LEN],
        title_len: 0,
        scroll_offset: 0,
        loading_state: LoadingState::Idle,
        error_code: 0,
        is_open: false,
        history: [[0; MAX_URL_LEN]; TAB_HISTORY_CAP],
        history_lens: [0; TAB_HISTORY_CAP],
        history_len: 0,
        history_index: 0,
    };

    pub fn new(id: u32, url: &[u8], title: &[u8]) -> Self {
        let mut tab = Self::EMPTY;
        tab.tab_id = id;
        tab.is_open = true;
        tab.set_url(url);
        tab.set_title(title);
        tab.push_history(url);
        tab
    }

    pub fn set_url(&mut self, url: &[u8]) {
        let n = url.len().min(MAX_URL_LEN);
        self.active_url[..n].copy_from_slice(&url[..n]);
        self.active_url_len = n;
    }

    pub fn set_title(&mut self, title: &[u8]) {
        let n = title.len().min(MAX_TITLE_LEN);
        self.title[..n].copy_from_slice(&title[..n]);
        self.title_len = n;
    }

    pub fn url(&self) -> &[u8] {
        &self.active_url[..self.active_url_len]
    }

    pub fn title_bytes(&self) -> &[u8] {
        &self.title[..self.title_len]
    }

    pub fn push_history(&mut self, url: &[u8]) {
        if url.is_empty() {
            return;
        }
        // Truncate forward history if we navigated from earlier in the stack
        if self.history_len > 0 && self.history_index + 1 < self.history_len {
            self.history_len = self.history_index + 1;
        }
        if self.history_len >= TAB_HISTORY_CAP {
            for i in 1..TAB_HISTORY_CAP {
                self.history[i - 1] = self.history[i];
                self.history_lens[i - 1] = self.history_lens[i];
            }
            self.history_len = TAB_HISTORY_CAP - 1;
            self.history_index = self.history_index.saturating_sub(1);
        }
        let idx = self.history_len;
        let u = url.len().min(MAX_URL_LEN);
        self.history[idx][..u].copy_from_slice(&url[..u]);
        self.history_lens[idx] = u;
        self.history_len += 1;
        self.history_index = self.history_len - 1;
    }

    pub fn can_go_back(&self) -> bool {
        self.history_index > 0
    }

    pub fn can_go_forward(&self) -> bool {
        self.history_len > 0 && self.history_index + 1 < self.history_len
    }

    pub fn go_back(&mut self) -> Option<&[u8]> {
        if !self.can_go_back() {
            return None;
        }
        self.history_index -= 1;
        let idx = self.history_index;
        let len = self.history_lens[idx];
        self.active_url[..len].copy_from_slice(&self.history[idx][..len]);
        self.active_url_len = len;
        self.scroll_offset = 0;
        Some(&self.active_url[..self.active_url_len])
    }

    pub fn go_forward(&mut self) -> Option<&[u8]> {
        if !self.can_go_forward() {
            return None;
        }
        self.history_index += 1;
        let idx = self.history_index;
        let len = self.history_lens[idx];
        self.active_url[..len].copy_from_slice(&self.history[idx][..len]);
        self.active_url_len = len;
        self.scroll_offset = 0;
        Some(&self.active_url[..self.active_url_len])
    }
}

/// Multi-Tab State & Session Management with fixed capacity up to 4 concurrent tabs.
#[derive(Debug, Clone, Copy)]
pub struct TabSessionManager {
    pub tabs: [Tab; MAX_TABS],
    pub active_index: usize,
    pub next_id: u32,
}

impl TabSessionManager {
    pub const fn new() -> Self {
        Self {
            tabs: [Tab::EMPTY; MAX_TABS],
            active_index: 0,
            next_id: 1,
        }
    }

    pub fn init_default(&mut self) {
        if !self.tabs[0].is_open {
            self.tabs[0] = Tab::new(self.next_id, b"about:blank", b"New Tab");
            self.next_id += 1;
            self.active_index = 0;
        }
    }

    pub fn tab_count(&self) -> usize {
        let mut count = 0;
        for tab in &self.tabs {
            if tab.is_open {
                count += 1;
            }
        }
        count
    }

    pub fn new_tab(&mut self, url: &[u8]) -> Option<usize> {
        for i in 0..MAX_TABS {
            if !self.tabs[i].is_open {
                let id = self.next_id;
                self.next_id += 1;
                let title = if url.is_empty() || url == b"about:blank" {
                    &b"New Tab"[..]
                } else {
                    &url[..url.len().min(MAX_TITLE_LEN)]
                };
                self.tabs[i] = Tab::new(id, url, title);
                self.active_index = i;
                return Some(i);
            }
        }
        None
    }

    pub fn close_tab(&mut self, tab_index: usize) -> bool {
        if tab_index >= MAX_TABS || !self.tabs[tab_index].is_open {
            return false;
        }
        if self.tab_count() <= 1 {
            self.tabs[tab_index] = Tab::new(self.tabs[tab_index].tab_id, b"about:blank", b"New Tab");
            return true;
        }

        self.tabs[tab_index].is_open = false;
        self.tabs[tab_index] = Tab::EMPTY;

        if self.active_index == tab_index {
            let mut found = None;
            for i in (0..tab_index).rev() {
                if self.tabs[i].is_open {
                    found = Some(i);
                    break;
                }
            }
            if found.is_none() {
                for i in (tab_index + 1)..MAX_TABS {
                    if self.tabs[i].is_open {
                        found = Some(i);
                        break;
                    }
                }
            }
            if let Some(new_idx) = found {
                self.active_index = new_idx;
            }
        }
        true
    }

    pub fn switch_tab(&mut self, tab_index: usize) -> bool {
        if tab_index < MAX_TABS && self.tabs[tab_index].is_open {
            self.active_index = tab_index;
            true
        } else {
            false
        }
    }

    pub fn active_tab(&self) -> &Tab {
        &self.tabs[self.active_index]
    }

    pub fn active_tab_mut(&mut self) -> &mut Tab {
        &mut self.tabs[self.active_index]
    }

    pub fn active_tab_index(&self) -> usize {
        self.active_index
    }

    pub fn tab(&self, index: usize) -> Option<&Tab> {
        if index < MAX_TABS && self.tabs[index].is_open {
            Some(&self.tabs[index])
        } else {
            None
        }
    }

    pub fn navigate_to(&mut self, url: &[u8]) -> bool {
        let active = &mut self.tabs[self.active_index];
        if !active.is_open {
            return false;
        }
        active.set_url(url);
        active.set_title(url);
        active.push_history(url);
        active.scroll_offset = 0;
        active.loading_state = LoadingState::Connecting;
        active.error_code = 0;
        true
    }

    pub fn can_go_back(&self) -> bool {
        let active = &self.tabs[self.active_index];
        active.is_open && active.can_go_back()
    }

    pub fn can_go_forward(&self) -> bool {
        let active = &self.tabs[self.active_index];
        active.is_open && active.can_go_forward()
    }

    pub fn go_back(&mut self) -> Option<&[u8]> {
        let active = &mut self.tabs[self.active_index];
        if active.is_open {
            active.go_back()
        } else {
            None
        }
    }

    pub fn go_forward(&mut self) -> Option<&[u8]> {
        let active = &mut self.tabs[self.active_index];
        if active.is_open {
            active.go_forward()
        } else {
            None
        }
    }
}

/// Bounded bookmark entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BookmarkItem {
    pub title: [u8; MAX_TITLE_LEN],
    pub title_len: usize,
    pub url: [u8; MAX_URL_LEN],
    pub url_len: usize,
}

impl BookmarkItem {
    pub const EMPTY: Self = Self {
        title: [0; MAX_TITLE_LEN],
        title_len: 0,
        url: [0; MAX_URL_LEN],
        url_len: 0,
    };

    pub fn new(title_bytes: &[u8], url_bytes: &[u8]) -> Self {
        let mut item = Self::EMPTY;
        let t = title_bytes.len().min(MAX_TITLE_LEN);
        item.title[..t].copy_from_slice(&title_bytes[..t]);
        item.title_len = t;
        let u = url_bytes.len().min(MAX_URL_LEN);
        item.url[..u].copy_from_slice(&url_bytes[..u]);
        item.url_len = u;
        item
    }

    pub fn title(&self) -> &[u8] {
        &self.title[..self.title_len]
    }

    pub fn url(&self) -> &[u8] {
        &self.url[..self.url_len]
    }
}

/// Bookmarks store supporting up to 16 bookmarks with duplicate prevention.
#[derive(Debug, Clone, Copy)]
pub struct BookmarkStore {
    pub items: [BookmarkItem; MAX_BOOKMARKS],
    pub count: usize,
}

impl BookmarkStore {
    pub const fn new() -> Self {
        Self {
            items: [BookmarkItem::EMPTY; MAX_BOOKMARKS],
            count: 0,
        }
    }

    pub fn add_bookmark(&mut self, title: &[u8], url: &[u8]) -> bool {
        if url.is_empty() || self.count >= MAX_BOOKMARKS {
            return false;
        }
        if self.is_bookmarked(url) {
            return false;
        }
        self.items[self.count] = BookmarkItem::new(title, url);
        self.count += 1;
        true
    }

    pub fn remove_bookmark(&mut self, url: &[u8]) -> bool {
        let mut found = None;
        for i in 0..self.count {
            if self.items[i].url() == url {
                found = Some(i);
                break;
            }
        }
        let Some(idx) = found else {
            return false;
        };
        for i in idx..self.count - 1 {
            self.items[i] = self.items[i + 1];
        }
        self.items[self.count - 1] = BookmarkItem::EMPTY;
        self.count -= 1;
        true
    }

    pub fn is_bookmarked(&self, url: &[u8]) -> bool {
        for i in 0..self.count {
            if self.items[i].url() == url {
                return true;
            }
        }
        false
    }

    pub fn list_bookmarks(&self) -> &[BookmarkItem] {
        &self.items[..self.count]
    }

    pub fn len(&self) -> usize {
        self.count
    }

    pub fn is_empty(&self) -> bool {
        self.count == 0
    }
}

// -----------------------------------------------------------------------------
// Legacy structures preserved for backward compatibility
// -----------------------------------------------------------------------------

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

/// Bounded bookmark record for settings blob.
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
    pub bookmarks: [BookmarkRecord; 8],
    pub bookmark_count: usize,
}

impl BrowserDataStore {
    pub const fn new() -> Self {
        Self {
            history: [HistoryRecord::EMPTY; MAX_HISTORY_ENTRIES],
            history_count: 0,
            bookmarks: [BookmarkRecord::EMPTY; 8],
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
        if self.bookmark_count >= 8 {
            return false;
        }
        self.bookmarks[self.bookmark_count] = b;
        self.bookmark_count += 1;
        true
    }
}

/// The host a search query is sent to.
///
/// WHY the host is named rather than implied: the browser has no search
/// engine of its own. What it does is send the query to one host, and a
/// screen that says "Search Web" for a request that goes to a single site
/// is a claim the browser cannot keep.
pub const SEARCH_HOST: &str = "interos.tech";

/// The address a search query is sent to.
pub const SEARCH_ENDPOINT: &[u8] = b"https://interos.tech/search?q=";

/// The label the search suggestion carries.
///
/// WHY it names the host: the suggestion has to say where the query goes,
/// and the only place the query goes is that one host.
pub const SEARCH_LABEL: &[u8] = b"Search interos.tech";

/// Builds the address a search query is sent to.
///
/// WHY the query is copied rather than escaped: the browser sends the
/// bytes the user typed. A query that needs escaping is a query the
/// search host will reject, and the screen says which host was asked.
pub fn search_url(query: &[u8], out: &mut [u8]) -> usize {
    let prefix = SEARCH_ENDPOINT;
    let mut n = prefix.len().min(out.len());
    out[..n].copy_from_slice(&prefix[..n]);
    let take = query.len().min(out.len().saturating_sub(n));
    out[n..n + take].copy_from_slice(&query[..take]);
    n += take;
    n
}

/// What the browser made of the address the user typed.
///
/// WHY this is a value and not a branch in the renderer: the address bar
/// used to substitute `interos.tech` for an address it could not parse,
/// so a page that named no host was shown as though the browser had
/// fetched that site. The screen has to be able to say that the address
/// was not recognised, and it can only say that if the decision is
/// recorded somewhere the screen can read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AddressOutcome {
    /// The address names a host the browser can ask for.
    Fetchable,
    /// The address carries no scheme the browser knows.
    NoScheme,
    /// The address carries a scheme but names no host.
    NoHost,
    /// The address names a scheme the browser cannot fetch.
    UnsupportedScheme,
}

/// Decides what the browser can do with the address the user typed.
pub fn classify_address(raw: &[u8]) -> AddressOutcome {
    let parsed = parse_uri(raw);
    match parsed.scheme {
        UriScheme::Unknown => return AddressOutcome::NoScheme,
        // WHY the gemini scheme is refused: the browser reads pages over
        // HTTP and the gateway in front of it speaks HTTP. It cannot speak
        // gemini, so an address in that scheme names a host the browser has
        // no way to ask.
        UriScheme::Gemini => return AddressOutcome::UnsupportedScheme,
        _ => {}
    }
    if parsed.host.is_empty() {
        return AddressOutcome::NoHost;
    }
    AddressOutcome::Fetchable
}

/// Prepends the scheme a bare hostname is fetched with.
///
/// WHY: a hostname typed without a scheme (`example.com`) names a host as
/// plainly as `https://example.com` does, but the parser saw no scheme and
/// the browser reported the address as unrecognised. Only input that reads
/// as a hostname is given a scheme; a bare word (`hello`) still names
/// nothing the browser can fetch.
pub fn normalize_address(raw: &[u8], out: &mut [u8]) -> usize {
    let trimmed = trim_ascii(raw);
    if !matches!(parse_uri(trimmed).scheme, UriScheme::Unknown) || !looks_like_hostname(trimmed) {
        let n = trimmed.len().min(out.len());
        out[..n].copy_from_slice(&trimmed[..n]);
        return n;
    }

    const PREFIX: &[u8] = b"https://";
    let n = PREFIX.len().min(out.len());
    out[..n].copy_from_slice(&PREFIX[..n]);
    let take = trimmed.len().min(out.len().saturating_sub(n));
    out[n..n + take].copy_from_slice(&trimmed[..take]);
    n + take
}

fn trim_ascii(raw: &[u8]) -> &[u8] {
    let start = raw
        .iter()
        .position(|b| !b.is_ascii_whitespace())
        .unwrap_or(raw.len());
    let end = raw
        .iter()
        .rposition(|b| !b.is_ascii_whitespace())
        .map(|i| i + 1)
        .unwrap_or(start);
    &raw[start..end]
}

fn looks_like_hostname(raw: &[u8]) -> bool {
    if raw.is_empty() || raw.iter().any(|b| b.is_ascii_whitespace()) || raw[0] == b'/' {
        return false;
    }
    let authority_end = raw
        .iter()
        .position(|&b| b == b'/' || b == b'?')
        .unwrap_or(raw.len());
    let authority = &raw[..authority_end];
    let host = match authority.iter().rposition(|&b| b == b'@') {
        Some(at) => &authority[at + 1..],
        None => authority,
    };
    let host = match host.iter().position(|&b| b == b':') {
        Some(colon) => &host[..colon],
        None => host,
    };
    host.contains(&b'.')
        && !host.starts_with(b".")
        && !host.ends_with(b".")
        && host
            .iter()
            .all(|&b| b.is_ascii_alphanumeric() || b == b'.' || b == b'-')
}

/// The directory the browser saves pages into.
pub const DOWNLOAD_DIR: &str = "/storage/downloads";

/// The prefix the path of a page the browser saved carries.
pub const DOWNLOAD_PREFIX: &str = "/storage/downloads/page_";

/// The name the browser gives the pages it saves.
///
/// WHY one name: the file manager and the launcher find these documents
/// through the platform's content index, and a name is what a person
/// recognises in a list of files.
pub const DOWNLOAD_NAME_PREFIX: &str = "page_";

/// Builds the path a saved page is written to.
pub fn download_path(index: u32, out: &mut [u8]) -> usize {
    let prefix = DOWNLOAD_PREFIX.as_bytes();
    let mut n = prefix.len().min(out.len());
    out[..n].copy_from_slice(&prefix[..n]);

    let mut digits = [0u8; 10];
    let mut len = 0usize;
    let mut value = index;
    loop {
        digits[len] = b'0' + (value % 10) as u8;
        len += 1;
        value /= 10;
        if value == 0 {
            break;
        }
    }
    while len > 0 {
        len -= 1;
        if n >= out.len() {
            break;
        }
        out[n] = digits[len];
        n += 1;
    }
    for suffix in b".html" {
        if n >= out.len() {
            break;
        }
        out[n] = *suffix;
        n += 1;
    }
    n
}

/// Whether a path in the shared document index is a page the browser saved.
///
/// WHY the name is matched as well as the directory: the shared storage
/// area holds every app's documents, and the browser's list is its own
/// pages and nobody else's.
pub fn is_download_path(path: &str) -> bool {
    if !path.starts_with("/storage/") {
        return false;
    }
    let Some(name) = path.rsplit('/').next() else {
        return false;
    };
    name.starts_with(DOWNLOAD_NAME_PREFIX) && name.ends_with(".html")
}

/// The prefix the path of a downloaded file carries.
///
/// WHY downloads share the directory but not the prefix: the browser's
/// saved-pages list is its own pages, while a downloaded file belongs to
/// the whole device and is opened by whichever app handles its type.
pub const MEDIA_PREFIX: &str = "/storage/downloads/dl_";

/// Whether a path in the shared document index is a file the browser downloaded.
///
/// WHY the `.origin` sidecar is excluded: the browser records the address a
/// file came from in a neighbouring document, and that record is the app's
/// own bookkeeping rather than something the person downloaded.
pub fn is_media_download_path(path: &str) -> bool {
    path.starts_with(MEDIA_PREFIX) && !path.ends_with(".origin")
}

/// Whether a content type carries a file rather than a document.
///
/// WHY the decision is made here: the gateway hands media over as raw
/// bytes, and the browser has to know before it decodes the body as text.
/// A type it does not recognise as text is a file.
pub fn content_type_is_binary(content_type: &[u8]) -> bool {
    let semi = content_type
        .iter()
        .position(|&b| b == b';')
        .unwrap_or(content_type.len());
    let ct = trim_ascii(&content_type[..semi]);
    if ct.is_empty() {
        return false;
    }
    const TEXTUAL: [&[u8]; 5] = [
        b"text/",
        b"application/xhtml+xml",
        b"application/xml",
        b"application/json",
        b"application/javascript",
    ];
    for needle in TEXTUAL {
        if starts_with_ignore_case(ct, needle) {
            return false;
        }
    }
    !(ends_with_ignore_case(ct, b"+xml") || ends_with_ignore_case(ct, b"+json"))
}

/// The file extension the browser gives a payload of `content_type`.
pub fn media_extension_for(content_type: &[u8]) -> Option<&'static str> {
    let semi = content_type
        .iter()
        .position(|&b| b == b';')
        .unwrap_or(content_type.len());
    let ct = trim_ascii(&content_type[..semi]);
    const TYPES: [(&[u8], &str); 17] = [
        (b"image/png", ".png"),
        (b"image/jpeg", ".jpg"),
        (b"image/gif", ".gif"),
        (b"image/webp", ".webp"),
        (b"image/bmp", ".bmp"),
        (b"image/x-ms-bmp", ".bmp"),
        (b"image/avif", ".avif"),
        (b"audio/mpeg", ".mp3"),
        (b"audio/mp3", ".mp3"),
        (b"audio/ogg", ".ogg"),
        (b"audio/wav", ".wav"),
        (b"audio/x-wav", ".wav"),
        (b"audio/flac", ".flac"),
        (b"audio/mp4", ".m4a"),
        (b"video/mp4", ".mp4"),
        (b"video/webm", ".webm"),
        (b"application/pdf", ".pdf"),
    ];
    for (name, extension) in TYPES {
        if eq_ignore_case(ct, name) {
            return Some(extension);
        }
    }
    None
}

/// The value of an HTTP header in a response, matched without case.
///
/// WHY the header section is bounded: a body can contain a line that looks
/// like a header, and it is not one.
pub fn http_header_value<'a>(raw: &'a [u8], name: &[u8]) -> Option<&'a [u8]> {
    let header_end = raw
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .unwrap_or(raw.len());
    for line in raw[..header_end].split(|&b| b == b'\n') {
        let line = match line.last() {
            Some(&b'\r') => &line[..line.len() - 1],
            _ => line,
        };
        if line.len() <= name.len() || line[name.len()] != b':' {
            continue;
        }
        if !eq_ignore_case(&line[..name.len()], name) {
            continue;
        }
        let value = trim_ascii(&line[name.len() + 1..]);
        return Some(value);
    }
    None
}

/// Builds the name a downloaded file is saved under.
///
/// WHY the server's name wins over the URL's: the origin may name the file
/// in Content-Disposition, and a URL can be a query rather than a path. The
/// extension is the content type's, so a file named `.php` that carries an
/// image is stored as the image it is and opens in the image viewer.
pub fn download_filename(
    url: &[u8],
    content_type: &[u8],
    given: Option<&[u8]>,
    out: &mut [u8],
) -> usize {
    // Leave room for an extension the name may have to be given.
    let name_cap = out.len().saturating_sub(8);
    let mut len = match given {
        Some(value) if !value.is_empty() => sanitize_download_name(value, &mut out[..name_cap]),
        _ => url_download_name(url, &mut out[..name_cap]),
    };
    if len == 0 {
        let fallback = b"download";
        len = fallback.len().min(name_cap);
        out[..len].copy_from_slice(&fallback[..len]);
    }
    match media_extension_for(content_type) {
        Some(extension) => {
            let expected = extension.as_bytes();
            let existing = out[..len]
                .iter()
                .rposition(|&b| b == b'.')
                .filter(|&dot| dot > 0);
            match existing {
                Some(dot) if eq_ignore_case(&out[dot + 1..len], &expected[1..]) && dot + 1 < len => {}
                Some(dot) => {
                    len = dot;
                    len = append_bytes(out, len, expected);
                }
                None => len = append_bytes(out, len, expected),
            }
        }
        None => {
            let has_extension = out[..len].iter().rposition(|&b| b == b'.').is_some_and(|dot| dot > 0);
            if !has_extension {
                len = append_bytes(out, len, b".bin");
            }
        }
    }
    len
}

/// Builds the path a downloaded file is written to.
///
/// WHY an index prefix: two downloads can share a name, and a second file
/// that silently overwrote the first would not be a download. The index is
/// the device's count of saved downloads, so the name is unique without
/// reading the directory first.
pub fn media_download_path(index: u32, name: &[u8], out: &mut [u8]) -> usize {
    let mut n = append_bytes(out, 0, MEDIA_PREFIX.as_bytes());
    let mut digits = [0u8; 10];
    let mut len = 0usize;
    let mut value = index;
    loop {
        digits[len] = b'0' + (value % 10) as u8;
        len += 1;
        value /= 10;
        if value == 0 {
            break;
        }
    }
    while len > 0 {
        len -= 1;
        n = append_bytes(out, n, &digits[len..len + 1]);
    }
    n = append_bytes(out, n, b"_");
    append_bytes(out, n, name)
}

/// Copies `bytes` into `out` at `n`, stopping at the buffer's end.
fn append_bytes(out: &mut [u8], mut n: usize, bytes: &[u8]) -> usize {
    for &b in bytes {
        if n >= out.len() {
            break;
        }
        out[n] = b;
        n += 1;
    }
    n
}

fn eq_ignore_case(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len()
        && a.iter()
            .zip(b.iter())
            .all(|(&x, &y)| x.to_ascii_lowercase() == y.to_ascii_lowercase())
}

fn starts_with_ignore_case(haystack: &[u8], needle: &[u8]) -> bool {
    haystack.len() >= needle.len() && eq_ignore_case(&haystack[..needle.len()], needle)
}

fn ends_with_ignore_case(haystack: &[u8], needle: &[u8]) -> bool {
    haystack.len() >= needle.len()
        && eq_ignore_case(&haystack[haystack.len() - needle.len()..], needle)
}

/// A pending download awaiting user confirmation before write to storage.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PendingDownload {
    pub active: bool,
    pub url: [u8; 128],
    pub url_len: usize,
    pub content_type: [u8; 48],
    pub content_type_len: usize,
    pub filename: [u8; 72],
    pub filename_len: usize,
    pub body_len: usize,
    pub body_offset: usize,
    pub truncated: bool,
}

impl PendingDownload {
    pub const EMPTY: PendingDownload = PendingDownload {
        active: false,
        url: [0; 128],
        url_len: 0,
        content_type: [0; 48],
        content_type_len: 0,
        filename: [0; 72],
        filename_len: 0,
        body_len: 0,
        body_offset: 0,
        truncated: false,
    };

    pub fn new(
        url: &[u8],
        content_type: &[u8],
        given_name: Option<&[u8]>,
        body_len: usize,
        body_offset: usize,
        truncated: bool,
    ) -> Self {
        let mut download = Self::EMPTY;
        download.active = true;
        let u_len = url.len().min(download.url.len());
        download.url[..u_len].copy_from_slice(&url[..u_len]);
        download.url_len = u_len;

        let c_len = content_type.len().min(download.content_type.len());
        download.content_type[..c_len].copy_from_slice(&content_type[..c_len]);
        download.content_type_len = c_len;

        let name_len = download_filename(url, content_type, given_name, &mut download.filename);
        download.filename_len = name_len;

        download.body_len = body_len;
        download.body_offset = body_offset;
        download.truncated = truncated;
        download
    }

    pub fn cancel(&mut self) {
        self.active = false;
    }
}

/// Keeps the characters a file name may hold, replacing the rest.
fn sanitize_download_name(name: &[u8], out: &mut [u8]) -> usize {
    let mut n = 0usize;
    for &b in name {
        if n >= out.len() {
            break;
        }
        let keep = b.is_ascii_alphanumeric() || b == b'.' || b == b'-' || b == b'_';
        out[n] = if keep { b } else { b'_' };
        n += 1;
    }
    while n > 0 && (out[n - 1] == b'.' || out[n - 1] == b'_') {
        n -= 1;
    }
    while n > 0 && out[0] == b'.' {
        out.copy_within(1..n, 0);
        n -= 1;
    }
    n
}

/// The last path segment of a URL, without its query or fragment.
fn url_download_name(url: &[u8], out: &mut [u8]) -> usize {
    let scheme_end = url
        .windows(3)
        .position(|w| w == b"://")
        .map(|i| i + 3)
        .unwrap_or(0);
    let path_start = url[scheme_end..]
        .iter()
        .position(|&b| b == b'/')
        .map(|i| scheme_end + i + 1)
        .unwrap_or(url.len());
    let path = &url[path_start..];
    let end = path
        .iter()
        .position(|&b| b == b'?' || b == b'#')
        .unwrap_or(path.len());
    let segment = match path[..end].iter().rposition(|&b| b == b'/') {
        Some(slash) => &path[slash + 1..end],
        None => &path[..end],
    };
    sanitize_download_name(segment, out)
}

/// The height one document line occupies, in logical pixels.
///
/// WHY this lives here and not in the renderer: the renderer walked the
/// lines to draw them and the tap handler walked them again to find the
/// link under the finger, and the two walks disagreed about how tall a
/// code block is. One table of heights is the only way the finger and
/// the eye agree.
pub fn doc_line_height(kind: LineKind, bytes: &[u8]) -> usize {
    match kind {
        LineKind::Heading1 => {
            std::design::font::line_height(crate::theme::type_scale::BODY) + 26
        }
        LineKind::Heading2 => 40,
        LineKind::Heading3 => 32,
        LineKind::ListItem => 28,
        LineKind::Blockquote => 34,
        LineKind::CodeBlock => {
            if bytes.starts_with(b"```") {
                // The fence itself is not drawn, but it still occupies the
                // space the block needs.
                8
            } else {
                34
            }
        }
        LineKind::GemtextLink => 66,
        LineKind::Image => images::PLACEHOLDER_HEIGHT,
        LineKind::Paragraph => 28,
        LineKind::Empty => 14,
    }
}

/// The height one document line reserves, including the style the reader
/// applied to it.
///
/// WHY this is the single source of truth: the scroll extent and the
/// renderer both walk the same list of lines, and if one of them counted
/// a top margin or a larger font rung that the other did not, the page
/// would end somewhere other than where it was drawn.
pub fn line_box_height(line: &DocLine) -> usize {
    let base = doc_line_height(line.kind, &line.bytes[..line.len.min(line.bytes.len())]);
    let mut height = base;
    if line.kind == LineKind::Image {
        height = (line.layout_h as usize).max(images::PLACEHOLDER_HEIGHT);
    } else if line.style.font_scale >= 2
        && matches!(line.kind, LineKind::Paragraph | LineKind::ListItem | LineKind::Blockquote)
    {
        height = height.max(std::design::font::line_height(2) + 4);
    }
    if line.block_start {
        height = height.saturating_add(line.style.margin_top as usize);
    }
    height
}

/// The height of a whole document, in logical pixels.
pub fn document_height_px(lines: &[DocLine]) -> usize {
    let mut total = 0usize;
    for line in lines {
        total = total.saturating_add(line_box_height(line));
    }
    total
}

/// The scroll state the reader uses for a document of a given height.
///
/// WHY the viewport is a parameter: the reader's viewport is the band
/// between the toolbar and the navigation bar, and the scroll state has
/// to know how tall that band is to know how far the document can move.
pub fn reader_scroll(content_h: usize, viewport_h: usize) -> ScrollState {
    ScrollState::new(content_h, viewport_h, 0)
}

/// Whether `haystack` contains `needle`, ignoring case.
///
/// WHY the match is loose: a person looking for a bookmark types the
/// first letters of what they remember, and a filter that matched only
/// the exact bytes would find nothing.
pub fn contains_ignore_case(haystack: &[u8], needle: &[u8]) -> bool {
    if needle.is_empty() {
        return true;
    }
    if needle.len() > haystack.len() {
        return false;
    }
    for start in 0..=(haystack.len() - needle.len()) {
        let mut matched = true;
        for (offset, &wanted) in needle.iter().enumerate() {
            if haystack[start + offset].to_ascii_lowercase() != wanted.to_ascii_lowercase() {
                matched = false;
                break;
            }
        }
        if matched {
            return true;
        }
    }
    false
}

// -----------------------------------------------------------------------------
// The text the screen shows for a candidate address and for a failure
// -----------------------------------------------------------------------------

/// The width the address bar's suggestion row gives its candidate string.
///
/// WHY a constant and not the screen's width: the suggestion card is 48 px
/// from each edge of the screen and its text starts 92 px in, so the string
/// owns 604 px. A string that does not fit is cut inside that budget and
/// marked, never past the edge of the row.
pub const SUGGESTION_TEXT_MAX: usize = 604;

/// Fits a candidate string into the suggestion row's width.
///
/// WHY the string is cut and not dropped: a candidate that stops mid-word
/// with no mark reads as a whole address, and a reader who cannot tell
/// that the address they are about to open is not the address they were
/// shown will open the wrong thing. The platform's ellipsis says the text
/// continues.
///
/// WHY the platform's helper and not a hand-rolled cut: the other
/// applications in this repository mark a string that does not fit with
/// `font::ellipsize`, and a second way of marking a cut string would be a
/// second way of getting it wrong.
///
/// WHY the caller's buffer must hold the whole string: the helper copies a
/// string that fits the width byte for byte, so a buffer smaller than the
/// string would cut it with no mark. A buffer of 256 bytes holds the
/// longest address the address bar accepts.
pub fn fit_suggestion(text: &[u8], out: &mut [u8]) -> usize {
    std::design::font::ellipsize(text, 1, SUGGESTION_TEXT_MAX, out)
}

/// The largest gateway reason the screen keeps.
pub const GATEWAY_REASON_MAX: usize = 256;

/// The label the screen puts in front of the gateway's own reason.
///
/// WHY the reason is named and not paraphrased: the reason is the host's
/// account of the failure, and a paraphrase would be a claim the browser
/// cannot back.
pub const GATEWAY_REASON_LABEL: &[u8] = b"Gateway said: ";

/// The width the gateway's reason line may occupy.
///
/// WHY it is the body's width: the reason is a line of the same block the
/// explanation is, so it is cut to the same measure the body is wrapped
/// to.
pub const GATEWAY_REASON_WIDTH: usize = 560;

/// The bytes the gateway's reason line may hold.
///
/// WHY it is larger than the reason itself: the line is the label and the
/// reason, and the helper that cuts it needs room for the mark it adds.
pub const GATEWAY_REASON_LINE_MAX: usize = GATEWAY_REASON_LABEL.len() + GATEWAY_REASON_MAX + 4;

/// Reads the reason the gateway gave for a failed fetch.
///
/// WHY the reason is read and not invented: the gateway answers a failed
/// fetch with `{"success": false, "error": "<reason>"}`, and that reason is
/// the host's own account of what happened. The screen used to print a
/// generic failure and throw the reason away, which made the browser look
/// broken instead of telling the person what happened.
///
/// WHY an envelope with no reason gives no reason: the screen then shows
/// the generic failure, which is true. Nothing is put in the gateway's
/// mouth.
///
/// WHY the caller's buffer must hold the reason and three more bytes: the
/// helper copies a reason that fits the width byte for byte, and a buffer
/// smaller than the reason would cut it with no mark.
pub fn gateway_failure_reason(json: &[u8], out: &mut [u8]) -> usize {
    let Some(raw) = http::proxy_string_field(json, b"error") else {
        return 0;
    };
    // The reason is cut at a byte that does not split a JSON escape, so the
    // text that is decoded is text the gateway wrote.
    let mut cut = raw.len().min(GATEWAY_REASON_MAX);
    while cut > 0 && raw[cut - 1] == b'\\' {
        cut -= 1;
    }
    let mut decoded = [0u8; GATEWAY_REASON_MAX];
    let Some(n) = http::unescape_json_string(&raw[..cut], &mut decoded) else {
        return 0;
    };
    std::design::font::ellipsize(&decoded[..n], 1, GATEWAY_REASON_WIDTH, out)
}

/// The line the screen shows for the reason the gateway gave.
///
/// WHY the reason is on a line of its own and not inside the explanation:
/// the explanation is what the browser knows and the reason is what the
/// server said, and a reader has to be able to tell the two apart.
///
/// WHY the line is cut with the platform's mark: a reason that stops
/// mid-word with no mark reads as the whole reason, which is the same lie
/// as showing no reason at all.
pub fn gateway_reason_line(reason: &[u8], out: &mut [u8]) -> usize {
    let mut composed = [0u8; GATEWAY_REASON_LINE_MAX];
    let label = GATEWAY_REASON_LABEL;
    let take = label.len().min(composed.len());
    composed[..take].copy_from_slice(&label[..take]);
    let mut n = take;
    let take = reason.len().min(composed.len() - n);
    composed[n..n + take].copy_from_slice(&reason[..take]);
    n += take;
    std::design::font::ellipsize(&composed[..n], 1, GATEWAY_REASON_WIDTH, out)
}

#[cfg(test)]
mod tests {
    // WHY this is declared here: the crate is `#![no_std]` and aliases the
    // SDK as `std`, but the host test that reads a captured page needs the
    // real standard library for the filesystem and the environment.
    extern crate std;

    use super::*;

    #[test]
    fn test_uri_parser_schemes() {
        assert_eq!(
            parse_uri(b"https://example.com/index.html"),
            ParsedUri {
                scheme: UriScheme::Https,
                host: b"example.com",
                path: b"/index.html",
                port: 443,
                query: None,
            }
        );

        assert_eq!(
            parse_uri(b"gemini://geminispace.info:1965/capsule"),
            ParsedUri {
                scheme: UriScheme::Gemini,
                host: b"geminispace.info",
                path: b"/capsule",
                port: 1965,
                query: None,
            }
        );

        assert_eq!(
            parse_uri(b"file:///data/docs/readme.txt"),
            ParsedUri {
                scheme: UriScheme::File,
                host: b"localhost",
                path: b"/data/docs/readme.txt",
                port: 0,
                query: None,
            }
        );

        assert_eq!(
            parse_uri(b"https://user:secret@example.com/dashboard"),
            ParsedUri {
                scheme: UriScheme::Https,
                host: b"example.com",
                path: b"/dashboard",
                port: 443,
                query: None,
            }
        );

        assert_eq!(
            parse_uri(b"http://[::1]:8080/api"),
            ParsedUri {
                scheme: UriScheme::Http,
                host: b"[::1]",
                path: b"/api",
                port: 8080,
                query: None,
            }
        );

        assert_eq!(
            parse_uri(b"interos://manifest.json"),
            ParsedUri {
                scheme: UriScheme::InterLayer,
                host: b"localhost",
                path: b"manifest.json",
                port: 0,
                query: None,
            }
        );

        assert_eq!(
            parse_uri(b"ftp://invalid.com"),
            ParsedUri {
                scheme: UriScheme::Unknown,
                host: b"",
                path: b"",
                port: 0,
                query: None,
            }
        );
    }

    #[test]
    fn test_uri_default_ports_and_queries() {
        let u1 = parse_uri(b"http://sovereign.local/docs?topic=microkernel&lang=rs");
        assert_eq!(u1.scheme, UriScheme::Http);
        assert_eq!(u1.host, b"sovereign.local");
        assert_eq!(u1.port, 80);
        assert_eq!(u1.path, b"/docs");
        assert_eq!(u1.query, Some(&b"topic=microkernel&lang=rs"[..]));
        assert_eq!(u1.query_str(), Some("topic=microkernel&lang=rs"));

        let u2 = parse_uri(b"https://secure.site:8443/login");
        assert_eq!(u2.scheme, UriScheme::Https);
        assert_eq!(u2.host, b"secure.site");
        assert_eq!(u2.port, 8443);
        assert_eq!(u2.path, b"/login");
        assert_eq!(u2.query, None);

        let u3 = parse_uri(b"gemini://geminispace.info");
        assert_eq!(u3.scheme, UriScheme::Gemini);
        assert_eq!(u3.host, b"geminispace.info");
        assert_eq!(u3.port, 1965);
        assert_eq!(u3.path, b"");
        assert_eq!(u3.query, None);

        let u4 = parse_uri(b"file:///etc/hosts?debug=true");
        assert_eq!(u4.scheme, UriScheme::File);
        assert_eq!(u4.host, b"localhost");
        assert_eq!(u4.path, b"/etc/hosts");
        assert_eq!(u4.port, 0);
        assert_eq!(u4.query, Some(&b"debug=true"[..]));
    }

    #[test]
    fn test_uri_parser_comprehensive_matrix() {
        // 1. Malformed URLs
        let empty = parse_uri(b"");
        assert_eq!(empty.scheme, UriScheme::Unknown);
        assert_eq!(empty.host, b"");

        let no_scheme = parse_uri(b"just-a-bare-word");
        assert_eq!(no_scheme.scheme, UriScheme::Unknown);

        let colon_only = parse_uri(b"://missing-scheme");
        assert_eq!(colon_only.scheme, UriScheme::Unknown);

        // 2. Unusual ports
        let port_zero = parse_uri(b"http://example.com:0/path");
        assert_eq!(port_zero.host, b"example.com");
        assert_eq!(port_zero.port, 0);

        let port_max = parse_uri(b"http://example.com:65535/path");
        assert_eq!(port_max.host, b"example.com");
        assert_eq!(port_max.port, 65535);

        let port_overflow = parse_uri(b"http://example.com:70000/path");
        assert_eq!(port_overflow.host, b"example.com");
        assert_eq!(port_overflow.port, 80); // Falls back to default port

        let port_non_digit = parse_uri(b"http://example.com:abc/path");
        assert_eq!(port_non_digit.host, b"example.com");
        assert_eq!(port_non_digit.port, 80);

        let port_empty = parse_uri(b"http://example.com:/path");
        assert_eq!(port_empty.host, b"example.com");
        assert_eq!(port_empty.port, 80);

        // 3. Bracketed IPv6 literals
        let ipv6_std = parse_uri(b"http://[::1]/");
        assert_eq!(ipv6_std.scheme, UriScheme::Http);
        assert_eq!(ipv6_std.host, b"[::1]");
        assert_eq!(ipv6_std.port, 80);
        assert_eq!(ipv6_std.path, b"/");

        let ipv6_port = parse_uri(b"https://[2001:db8::1]:8443/status?q=1");
        assert_eq!(ipv6_port.scheme, UriScheme::Https);
        assert_eq!(ipv6_port.host, b"[2001:db8::1]");
        assert_eq!(ipv6_port.port, 8443);
        assert_eq!(ipv6_port.path, b"/status");
        assert_eq!(ipv6_port.query, Some(&b"q=1"[..]));

        // 4. Userinfo stripping (prevents credential leakage in host component)
        let userinfo1 = parse_uri(b"http://user:password@example.com/login");
        assert_eq!(userinfo1.host, b"example.com");
        assert_eq!(userinfo1.port, 80);
        assert_eq!(userinfo1.path, b"/login");

        let userinfo2 = parse_uri(b"https://admin@sub.domain.org:9000/");
        assert_eq!(userinfo2.host, b"sub.domain.org");
        assert_eq!(userinfo2.port, 9000);

        let userinfo_ipv6 = parse_uri(b"http://user:pass@[::1]:8080/");
        assert_eq!(userinfo_ipv6.host, b"[::1]");
        assert_eq!(userinfo_ipv6.port, 8080);

        // 5. Unsupported schemes
        let ftp = parse_uri(b"ftp://ftp.is.co.za/rfc/rfc1808.txt");
        assert_eq!(ftp.scheme, UriScheme::Unknown);

        let gopher = parse_uri(b"gopher://gopher.floodgap.com/");
        assert_eq!(gopher.scheme, UriScheme::Unknown);

        let javascript = parse_uri(b"javascript:alert(1)");
        assert_eq!(javascript.scheme, UriScheme::Unknown);

        let mailto = parse_uri(b"mailto:admin@example.com");
        assert_eq!(mailto.scheme, UriScheme::Unknown);
    }

    #[test]
    fn test_classify_markdown_and_gemtext_lines() {
        assert_eq!(classify_line(b"# Main Title"), LineKind::Heading1);
        assert_eq!(classify_line(b"## Section Heading"), LineKind::Heading2);
        assert_eq!(classify_line(b"### Subheading"), LineKind::Heading3);
        assert_eq!(classify_line(b"* Bullet point item"), LineKind::ListItem);
        assert_eq!(classify_line(b"- Hyphen list item"), LineKind::ListItem);
        assert_eq!(classify_line(b"+ Plus list item"), LineKind::ListItem);
        assert_eq!(classify_line(b"> Blockquote text"), LineKind::Blockquote);
        assert_eq!(classify_line(b"```rust"), LineKind::CodeBlock);
        assert_eq!(classify_line(b"```"), LineKind::CodeBlock);
        assert_eq!(classify_line(b"=> https://example.com Link Text"), LineKind::GemtextLink);
        assert_eq!(classify_line(b"Regular paragraph prose."), LineKind::Paragraph);
        assert_eq!(classify_line(b""), LineKind::Empty);
        assert_eq!(classify_line(b"   \t\r"), LineKind::Empty);
    }

    #[test]
    fn test_gemtext_link_parser() {
        let (tgt, lbl) = parse_gemtext_link(b"=> https://interos.tech Official Site").unwrap();
        assert_eq!(tgt, b"https://interos.tech");
        assert_eq!(lbl, b"Official Site");

        let (tgt2, lbl2) = parse_gemtext_link(b"=>   gemini://capsule/page\tMy Capsule").unwrap();
        assert_eq!(tgt2, b"gemini://capsule/page");
        assert_eq!(lbl2, b"My Capsule");

        let (tgt3, lbl3) = parse_gemtext_link(b"=> /index.gmi").unwrap();
        assert_eq!(tgt3, b"/index.gmi");
        assert_eq!(lbl3, b"");

        assert!(parse_gemtext_link(b"# Not a link").is_none());
    }

    #[test]
    fn test_document_tokenizer() {
        let doc = b"# Header\n\n* Item 1\n```\nlet x = 42;\n```\n=> https://site.org Link\nParagraph";
        let mut tok = DocumentTokenizer::new(doc);

        assert_eq!(tok.next_line(), Some((&b"# Header"[..], LineKind::Heading1)));
        assert_eq!(tok.next_line(), Some((&b""[..], LineKind::Empty)));
        assert_eq!(tok.next_line(), Some((&b"* Item 1"[..], LineKind::ListItem)));
        assert_eq!(tok.next_line(), Some((&b"```"[..], LineKind::CodeBlock)));
        assert_eq!(tok.next_line(), Some((&b"let x = 42;"[..], LineKind::Paragraph)));
        assert_eq!(tok.next_line(), Some((&b"```"[..], LineKind::CodeBlock)));
        assert_eq!(tok.next_line(), Some((&b"=> https://site.org Link"[..], LineKind::GemtextLink)));
        assert_eq!(tok.next_line(), Some((&b"Paragraph"[..], LineKind::Paragraph)));
        assert_eq!(tok.next_line(), None);
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
        let (_, len) = into_url_field(b"/");
        assert_eq!(len, 1);
    }

    #[test]
    fn a_url_field_is_truncated_to_the_row_width() {
        let long = b"https://example.org/a/very/long/path/that/keeps/going/and/going/beyond/the/field";
        assert!(long.len() > URL_FIELD);
        let (field, len) = into_url_field(long);
        assert_eq!(len, URL_FIELD);
        assert_eq!(&field[..len], &long[..URL_FIELD]);
    }

    #[test]
    fn test_tab_session_manager_lifecycle() {
        let mut mgr = TabSessionManager::new();
        assert_eq!(mgr.tab_count(), 0);

        mgr.init_default();
        assert_eq!(mgr.tab_count(), 1);
        assert_eq!(mgr.active_tab().url(), b"about:blank");
        assert_eq!(mgr.active_tab_index(), 0);

        // Add 3 more tabs -> capacity 4 reached
        let t1 = mgr.new_tab(b"https://interos.tech").unwrap();
        assert_eq!(t1, 1);
        assert_eq!(mgr.active_tab_index(), 1);

        let t2 = mgr.new_tab(b"gemini://geminispace.info").unwrap();
        assert_eq!(t2, 2);

        let t3 = mgr.new_tab(b"file:///docs/readme.txt").unwrap();
        assert_eq!(t3, 3);
        assert_eq!(mgr.tab_count(), 4);

        // 5th tab must fail
        assert!(mgr.new_tab(b"https://extra.com").is_none());

        // Switching tabs
        assert!(mgr.switch_tab(1));
        assert_eq!(mgr.active_tab().url(), b"https://interos.tech");
        assert!(!mgr.switch_tab(5)); // Out of bounds

        // Close tab 1
        assert!(mgr.close_tab(1));
        assert_eq!(mgr.tab_count(), 3);
        assert!(mgr.tab(1).is_none());
        // Active index adjusted to a valid remaining tab
        assert!(mgr.active_tab().is_open);

        // Close all down to 1
        mgr.close_tab(2);
        mgr.close_tab(3);
        assert_eq!(mgr.tab_count(), 1);

        // Closing the last tab resets it to about:blank
        let last_idx = mgr.active_tab_index();
        assert!(mgr.close_tab(last_idx));
        assert_eq!(mgr.tab_count(), 1);
        assert_eq!(mgr.active_tab().url(), b"about:blank");
    }

    #[test]
    fn test_tab_history_navigation_and_truncation() {
        let mut mgr = TabSessionManager::new();
        mgr.init_default();

        assert!(!mgr.can_go_back());
        assert!(!mgr.can_go_forward());

        mgr.navigate_to(b"https://page1.org");
        mgr.navigate_to(b"https://page2.org");
        mgr.navigate_to(b"https://page3.org");

        assert!(mgr.can_go_back());
        assert!(!mgr.can_go_forward());
        assert_eq!(mgr.active_tab().url(), b"https://page3.org");

        // Back to page 2
        assert_eq!(mgr.go_back(), Some(&b"https://page2.org"[..]));
        assert!(mgr.can_go_back());
        assert!(mgr.can_go_forward());

        // Back to page 1
        assert_eq!(mgr.go_back(), Some(&b"https://page1.org"[..]));
        // Back to about:blank
        assert_eq!(mgr.go_back(), Some(&b"about:blank"[..]));
        assert!(!mgr.can_go_back());

        // Forward to page 1
        assert_eq!(mgr.go_forward(), Some(&b"https://page1.org"[..]));

        // New navigation from page 1 truncates forward history (page 2 and 3)
        mgr.navigate_to(b"https://new-branch.org");
        assert!(!mgr.can_go_forward());
        assert_eq!(mgr.active_tab().url(), b"https://new-branch.org");

        // Back should lead directly to page 1
        assert_eq!(mgr.go_back(), Some(&b"https://page1.org"[..]));
    }

    #[test]
    fn test_bookmark_store() {
        let mut store = BookmarkStore::new();
        assert_eq!(store.len(), 0);
        assert!(store.is_empty());

        // Add bookmarks
        assert!(store.add_bookmark(b"InterOS", b"https://interos.tech"));
        assert!(store.add_bookmark(b"Gemini", b"gemini://geminispace.info"));
        assert_eq!(store.len(), 2);
        assert!(store.is_bookmarked(b"https://interos.tech"));
        assert!(store.is_bookmarked(b"gemini://geminispace.info"));
        assert!(!store.is_bookmarked(b"https://unknown.com"));

        // Duplicate prevention
        assert!(!store.add_bookmark(b"Duplicate InterOS", b"https://interos.tech"));
        assert_eq!(store.len(), 2);

        // List bookmarks
        let list = store.list_bookmarks();
        assert_eq!(list.len(), 2);
        assert_eq!(list[0].title(), b"InterOS");
        assert_eq!(list[0].url(), b"https://interos.tech");

        // Remove bookmark
        assert!(store.remove_bookmark(b"https://interos.tech"));
        assert_eq!(store.len(), 1);
        assert!(!store.is_bookmarked(b"https://interos.tech"));
        assert!(store.is_bookmarked(b"gemini://geminispace.info"));

        // Removing non-existent returns false
        assert!(!store.remove_bookmark(b"https://nonexistent.org"));

        // Capacity test (up to 16)
        for i in 2..=16 {
            let mut url = [0u8; 32];
            url[0..7].copy_from_slice(b"http://");
            url[7] = b'a' + (i as u8);
            assert!(store.add_bookmark(b"Site", &url[..8]));
        }
        assert_eq!(store.len(), 16);
        // 17th bookmark rejected
        assert!(!store.add_bookmark(b"Overflow", b"http://overflow.org"));
    }

    #[test]
    fn test_browser_data_store() {
        let mut store = BrowserDataStore::new();
        assert_eq!(store.history_count, 0);
        assert_eq!(store.bookmark_count, 0);

        store.push_history(HistoryRecord::new(b"https://interos.tech", b"InterOS Home", 100));
        assert_eq!(store.history_count, 1);
        assert_eq!(&store.history[0].url[..store.history[0].url_len], b"https://interos.tech");

        for i in 2..=10 {
            store.push_history(HistoryRecord::new(b"https://site.org", b"Site", i as u64));
        }
        assert_eq!(store.history_count, MAX_HISTORY_ENTRIES);
        assert_eq!(store.history[MAX_HISTORY_ENTRIES - 1].visited_at_epoch_secs, 10);
        assert_eq!(store.history[0].visited_at_epoch_secs, 3);

        assert!(store.add_bookmark(BookmarkRecord::new(b"gemini://docs", b"Documentation", true)));
        assert_eq!(store.bookmark_count, 1);
        assert!(store.bookmarks[0].is_pinned);
    }

    #[test]
    fn test_format_http_get_request() {
        let mut buf = [0u8; 256];
        let n = format_http_get(b"interos.tech", b"/docs/kernel", &mut buf);
        let req = core::str::from_utf8(&buf[..n]).unwrap();
        assert!(req.starts_with("GET /docs/kernel HTTP/1.1\r\nHost: interos.tech\r\n"));
        assert!(req.contains("User-Agent: InterOS-Browser/1.0"));
        assert!(req.ends_with("\r\n\r\n"));
    }

    #[test]
    fn test_parse_http_response_success() {
        let raw = b"HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: 42\r\n\r\n<h1>Hello</h1><p>Test</p>";
        let res = parse_http_response(raw).expect("valid http response");
        assert_eq!(res.status_code, 200);
        assert_eq!(res.content_type, b"text/html");
        assert_eq!(res.body, b"<h1>Hello</h1><p>Test</p>");
    }

    #[test]
    fn test_parse_http_response_not_found() {
        let raw = b"HTTP/1.0 404 Not Found\r\nServer: Custom\r\n\r\nMissing";
        let res = parse_http_response(raw).expect("valid http 404");
        assert_eq!(res.status_code, 404);
        assert_eq!(res.body, b"Missing");
    }

    #[test]
    fn test_html_to_reader_document_conversion() {
        let html = b"<html><head><title>InterOS Site</title></head><body><h1>Welcome</h1><p>This is a <b>sovereign</b> system.</p><p>Visit <a href=\"https://interos.tech\">Home</a> now.</p><ul><li>Feature 1</li></ul><pre><code>code sample</code></pre></body></html>";
        let mut out = [0u8; 1024];
        let n = html_to_reader_document(html, &mut out);
        let doc = core::str::from_utf8(&out[..n]).unwrap();
        assert!(doc.contains("# InterOS Site"));
        assert!(doc.contains("# Welcome"));
        assert!(doc.contains("This is a sovereign system."));
        assert!(doc.contains("=> https://interos.tech Home"));
        assert!(doc.contains("* Feature 1"));
        assert!(doc.contains("```\ncode sample\n```"));
    }

    /// The reader carries styles and images through its text pipeline as
    /// marker lines rather than inventing a second document type.
    ///
    /// WHY this test exists: the CSS and image modules can each be correct
    /// while the HTML walk never emits their markers, and then the page
    /// renders unstyled with no picture and no explanation.
    #[test]
    fn test_styles_and_images_become_reader_markers() {
        let html = b"<style>p { color: #ff0000; }</style>\
<p>Red</p><img src=\"x.png\" alt=\"X\" width=\"40\" height=\"30\">";
        let mut out = [0u8; 1024];
        let n = html_to_reader_document(html, &mut out);
        let mut style_markers = 0usize;
        let mut image_markers = 0usize;
        let mut saw_red = false;
        for line in out[..n].split(|&b| b == b'\n') {
            if css::is_style_marker(line) {
                style_markers += 1;
            } else if images::is_image_marker(line) {
                image_markers += 1;
                assert_eq!(classify_line(line), LineKind::Image);
            } else if line == b"Red" {
                saw_red = true;
            }
        }
        assert!(style_markers >= 2, "a style marker before and after the block");
        assert_eq!(image_markers, 1);
        assert!(saw_red, "the paragraph text survives the markers");
    }

    #[test]
    fn test_format_proxy_request() {
        let mut buf = [0u8; 256];
        let n = format_proxy_request(b"https://interos.tech", &mut buf);
        let req = core::str::from_utf8(&buf[..n]).unwrap();
        assert!(req.starts_with("GET /api/browser/proxy?url=https%3A%2F%2Finteros.tech HTTP/1.1\r\n"));
        assert!(req.contains("Host: 127.0.0.1:4300\r\n"));
    }

    #[test]
    fn test_extract_proxy_html() {
        let json = br#"{"success": true, "url": "https://interos.tech", "html": "<h1>InterOS</h1><p>Test</p>"}"#;
        let mut out = [0u8; 128];
        let n = extract_proxy_html(json, &mut out);
        let html = core::str::from_utf8(&out[..n]).unwrap();
        assert_eq!(html, "<h1>InterOS</h1><p>Test</p>");
    }

    /// The reader must not keep a page's own navigation.
    ///
    /// WHY this test exists: the reader used to keep every element, so the
    /// live site's navigation was seven of the first twenty lines it
    /// produced, each rendered as a link card.
    #[test]
    fn test_page_chrome_is_dropped_and_the_article_is_kept() {
        let html = b"<html><head><title>Doc</title></head><body>\
<header class=\"site-header\"><nav id=\"primary-nav\"><a href=\"#top\">Home</a></nav>\
<div class=\"header-actions\"><a class=\"icon-link\" href=\"https://example.com/x\">GH</a></div></header>\
<main><p>The actual article.</p></main></body></html>";
        let mut out = [0u8; 512];
        let n = html_to_reader_document(html, &mut out);
        let text = core::str::from_utf8(&out[..n]).expect("utf8");

        assert!(text.contains("The actual article."), "the article was lost: {text:?}");
        assert!(!text.contains("Home"), "the navigation text leaked: {text:?}");
        assert!(!text.contains("example.com"), "a header link leaked: {text:?}");
    }

    /// Whitespace between tags must not become reader lines.
    ///
    /// WHY this test exists: the source markup's indentation became reader
    /// lines, so 688 of the 958 lines the live article produced were blank
    /// or a single space, and the reader showed one line of content.
    #[test]
    fn test_source_indentation_does_not_become_blank_lines() {
        let html = b"<html>\n   <body>\n     <div>\n       <p>One.</p>\n     </div>\n   </body>\n</html>";
        let mut out = [0u8; 512];
        let n = html_to_reader_document(html, &mut out);
        let text = core::str::from_utf8(&out[..n]).expect("utf8");

        let blanks = text.lines().filter(|l| l.trim().is_empty()).count();
        assert!(blanks <= 2, "the document is mostly blank lines: {text:?}");
        assert!(text.contains("One."), "the text was lost: {text:?}");
    }

    /// A script's content is not markup and must not reach the article.
    ///
    /// WHY this test exists: the live page carries a 7 KB
    /// application/ld+json block whose text mentions "UserPtr<T>". A parser
    /// that treats that "<" as a tag dumps the JSON into the article.
    #[test]
    fn test_script_content_is_not_markup() {
        let html = b"<html><body><script type=\"application/ld+json\">{\"a\":\"UserPtr<T> bounds\"}</script>\
<p>Article.</p></body></html>";
        let mut out = [0u8; 512];
        let n = html_to_reader_document(html, &mut out);
        let text = core::str::from_utf8(&out[..n]).expect("utf8");

        assert!(text.contains("Article."), "the article was lost: {text:?}");
        assert!(!text.contains("UserPtr"), "script content leaked: {text:?}");
    }

    /// An external script has no closing tag and must not swallow the page.
    ///
    /// WHY this test exists: the raw-text skip searched for the next
    /// "</script>", so a `<script src="...">` tag consumed everything up to
    /// the next script -- the site's whole header and hero with it.
    #[test]
    fn test_an_external_script_does_not_swallow_the_page() {
        let html = b"<html><head><script type=\"module\" src=\"/a.js\"></script></head>\
<body><p>Kept.</p></body></html>";
        let mut out = [0u8; 512];
        let n = html_to_reader_document(html, &mut out);
        let text = core::str::from_utf8(&out[..n]).expect("utf8");
        assert!(text.contains("Kept."), "the article was swallowed: {text:?}");
    }

    /// A line break inside a heading is a space, not a new document line.
    ///
    /// WHY this test exists: the live article's <h1> contains two <br>
    /// elements, and treating them as line breaks split one title into
    /// three document lines.
    #[test]
    fn test_a_heading_stays_one_line() {
        let html = b"<h1>InterOS<span>A first</span><br />mobile operating system</h1>";
        let mut out = [0u8; 512];
        let n = html_to_reader_document(html, &mut out);
        let text = core::str::from_utf8(&out[..n]).expect("utf8");

        assert_eq!(text.lines().filter(|l| l.starts_with('#')).count(), 1,
            "the title was split across lines: {text:?}");
        assert!(text.contains("InterOS A first mobile operating system"),
            "the title text was not joined: {text:?}");
    }

    /// Adjacent inline elements are separated, so words do not run together.
    ///
    /// WHY this test exists: the live article's title is
    /// "<span>InterOS</span><span class=\"hero-subtitle\">A security-first</span>",
    /// and concatenating them produced "InterOSA security-first".
    #[test]
    fn test_adjacent_inline_elements_are_separated() {
        let html = b"<h1><span>InterOS</span><span>A security-first</span></h1>";
        let mut out = [0u8; 512];
        let n = html_to_reader_document(html, &mut out);
        let text = core::str::from_utf8(&out[..n]).expect("utf8");
        assert!(text.contains("InterOS A security-first"),
            "the inline elements ran together: {text:?}");
    }

    /// Every byte the reader emits must be drawable by the platform font.
    ///
    /// WHY this test exists: the platform font covers ASCII 32..126 and
    /// nothing else, so a byte outside that range draws as a blank
    /// six-pixel gap. An em dash decoded into UTF-8 would put three blank
    /// boxes on the page.
    #[test]
    fn test_the_reader_document_is_drawable() {
        // The em dash, the north-east arrow and the ellipsis, as UTF-8.
        let html = b"<p>InterOS \xe2\x80\x94 a security-first OS \xe2\x86\x97 done\xe2\x80\xa6</p>";
        let mut out = [0u8; 512];
        let n = html_to_reader_document(html, &mut out);
        let text = core::str::from_utf8(&out[..n]).expect("utf8");

        // The newlines are the line separators the tokenizer splits on;
        // they are never drawn. Every byte of an actual line has to be
        // drawable.
        for line in text.lines() {
            for &b in line.as_bytes() {
                assert!((32..=126).contains(&b),
                    "the reader emitted byte {b:#04x}, which the font cannot draw: {text:?}");
            }
        }
        assert!(
            text.contains("InterOS") && text.contains("a security-first OS"),
            "the em dash was not transliterated: {text:?}"
        );
        assert!(
            !text.as_bytes().iter().any(|&b| b >= 0x80),
            "the reader emitted a byte the font cannot draw: {text:?}"
        );
    }

    /// A plain document keeps its lines.
    ///
    /// WHY this test exists: a standards document or a licence has no
    /// markup, and collapsing its newlines into spaces turned a 35 KB file
    /// into ONE line. The GNU licence also writes its own preamble as
    /// "<one line to give the program's name...>", so no heuristic can
    /// tell it from markup -- the content type can.
    #[test]
    fn test_a_plain_document_keeps_its_lines() {
        let text = b"Title\n\nFirst paragraph line one.\nFirst paragraph line two.\n\nSecond paragraph.";
        let mut out = [0u8; 512];
        let n = reader_text_from_plain(text, &mut out);
        let doc = core::str::from_utf8(&out[..n]).expect("utf8");

        let mut non_empty = 0usize;
        for line in doc.lines() {
            if !line.trim().is_empty() {
                non_empty += 1;
            }
        }
        assert_eq!(non_empty, 4, "the document lost its lines: {doc:?}");
        assert!(doc.contains("First paragraph line one."));
    }

    /// The content type decides how a payload is read.
    #[test]
    fn test_the_content_type_decides_the_mode() {
        assert!(content_type_is_plain(b"text/plain"));
        assert!(content_type_is_plain(b"text/plain; charset=utf-8"));
        assert!(content_type_is_plain(b"TEXT/PLAIN"));
        assert!(!content_type_is_plain(b"text/html; charset=utf-8"));
        assert!(!content_type_is_plain(b"application/json"));
        // A prefix of another media type is not a match.
        assert!(!content_type_is_plain(b"text/plaintext"));
        assert!(!content_type_is_plain(b"application/x-text/plain"));
    }

    /// The gateway's reported content type is found inside its envelope.
    #[test]
    fn test_the_gateway_content_type_is_read_from_the_envelope() {
        let json = br#"{"success": true, "url": "https://example.com", "content_type": "text/plain; charset=utf-8", "html": "hi"}"#;
        let ct = proxy_content_type(json).expect("no content type");
        assert_eq!(ct, b"text/plain; charset=utf-8");
        assert!(content_type_is_plain(ct));
    }

    /// A failed gateway fetch is recognised by its envelope, not by its HTTP
    /// status, and is never mistaken for a page.
    #[test]
    fn test_a_failed_gateway_fetch_is_recognised() {
        let spaced = br#"{"success": false, "url": "http://neverssl.com", "error": "<urlopen error timed out>"}"#;
        let compact = br#"{"success":false,"url":"http://example.com","error":"boom"}"#;
        let ok = br#"{"success": true, "url": "https://example.com", "content_type": "text/html", "html": "hi"}"#;
        assert!(proxy_reports_failure(spaced));
        assert!(proxy_reports_failure(compact));
        assert!(!proxy_reports_failure(ok));
    }

    /// A local document's address yields the path to read.
    ///
    /// WHY this matters: the browser reads a local document from the guest
    /// filesystem, and the path it reads has to be the one the address
    /// named rather than a fixed one.
    #[test]
    fn test_a_local_address_yields_its_path() {
        let parsed = parse_uri(b"file:///data/reader_sample.html");
        assert_eq!(parsed.scheme, UriScheme::File);
        assert!(parsed.scheme.is_local());
        assert_eq!(parsed.path, b"/data/reader_sample.html");
    }

    /// A long line is wrapped to the reading column.
    ///
    /// WHY this matters: prose has no line breaks in it, so without
    /// wrapping a long sentence ran off the right edge of the screen and
    /// was clipped mid-glyph.
    #[test]
    fn test_a_long_line_is_wrapped_to_the_column() {
        let text = b"InterOS is an open-source smartphone operating system with a capability-based microkernel and hardware-enforced memory isolation.";
        let mut out = [[0u8; MAX_LINE_BYTES]; MAX_WRAP_PIECES];
        let mut lens = [0usize; MAX_WRAP_PIECES];
        let n = wrap_line(text, 1, CONTENT_WIDTH, &mut out, &mut lens);

        assert!(n >= 2, "the line was not wrapped: {n} piece(s)");
        for i in 0..n {
            let width = interos_std::design::font::measure(&out[i][..lens[i]], 1);
            assert!(
                width <= CONTENT_WIDTH,
                "piece {i} is {width} pixels wide, over the {CONTENT_WIDTH} column"
            );
            assert!(lens[i] > 0, "piece {i} is empty");
        }

        // The words must all survive the split.
        let mut rejoined = [0u8; 256];
        let mut len = 0usize;
        for i in 0..n {
            for j in 0..lens[i] {
                if len < rejoined.len() {
                    rejoined[len] = out[i][j];
                    len += 1;
                }
            }
            if i + 1 < n && len < rejoined.len() {
                rejoined[len] = b' ';
                len += 1;
            }
        }
        let rejoined = core::str::from_utf8(&rejoined[..len]).expect("utf8");
        assert!(rejoined.contains("InterOS is an open-source"));
        assert!(rejoined.contains("memory isolation."));
    }

    /// A line that already fits is left alone.
    #[test]
    fn test_a_short_line_is_not_wrapped() {
        let mut out = [[0u8; MAX_LINE_BYTES]; MAX_WRAP_PIECES];
        let mut lens = [0usize; MAX_WRAP_PIECES];
        let n = wrap_line(b"Short enough.", 1, CONTENT_WIDTH, &mut out, &mut lens);
        assert_eq!(n, 1);
        assert_eq!(&out[0][..lens[0]], b"Short enough.");
    }

    /// The reader pipeline, replayed against a matrix of real sites.
    ///
    /// WHY a matrix and not one page: real markup differs every time. A
    /// marketing page, a wiki article, a documentation page, a forum and
    /// a plain-text RFC each nest their content differently, and a reader
    /// that handles one of them is not a reader.
    ///
    /// The fixtures are captured by `tools/capture_site_matrix.py`, which
    /// asks the Device Studio gateway for each page -- the same route the
    /// device itself uses.
    ///
    /// The test is skipped when no matrix has been captured, so the suite
    /// still runs on a machine with no Studio.
    #[test]
    fn test_the_reader_survives_a_matrix_of_real_sites() {
        let Ok(dir) = std::env::var("INTEROS_BROWSER_SITE_MATRIX") else {
            std::eprintln!(
                "skipping: INTEROS_BROWSER_SITE_MATRIX is not set. Capture one with:\n  python tools/capture_site_matrix.py"
            );
            return;
        };
        let Ok(entries) = std::fs::read_dir(&dir) else {
            std::eprintln!("skipping: {dir} could not be read");
            return;
        };

        // WHY fixed buffers: the crate is `#![no_std]`, so a test cannot
        // use Vec or String either. The failure list is bounded, and a
        // bounded list is enough to name the sites that broke.
        const MAX_FAILURES: usize = 16;
        let mut failures: [[u8; 96]; MAX_FAILURES] = [[0; 96]; MAX_FAILURES];
        let mut failure_lens: [usize; MAX_FAILURES] = [0; MAX_FAILURES];
        let mut failure_count = 0usize;

        let mut checked = 0usize;
        let mut entry = entries;
        while let Some(Ok(dir_entry)) = entry.next() {
            let path = dir_entry.path();
            let name = path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("")
                .as_bytes();
            if name.len() < 5 || !name.ends_with(b".json") || name == b"index.json" {
                continue;
            }
            let Ok(raw) = std::fs::read(&path) else {
                continue;
            };

            // 1. The HTML is extracted from the proxy's JSON envelope.
            let mut html = [0u8; MAX_HTML_BYTES];
            let html_len = extract_proxy_html(&raw, &mut html);
            if html_len == 0 {
                if failure_count < MAX_FAILURES {
                    let l = copy_fail(&mut failures[failure_count], &name);
                    failure_lens[failure_count] = l;
                    failure_count += 1;
                }
                continue;
            }

            // 2. The document is parsed from it, in the mode the gateway
            //    reported for the page. A `text/plain` page is text, and
            //    its newlines are lines.
            let kind = match proxy_content_type(&raw) {
                Some(ct) if content_type_is_plain(ct) => PayloadKind::PlainText,
                _ => PayloadKind::Markup,
            };
            let mut doc = [0u8; MAX_READER_BYTES];
            let doc_len = html_to_reader_document_kind(&html[..html_len], kind, &mut doc);
            if doc_len == 0 {
                if failure_count < MAX_FAILURES {
                    let l = copy_fail(&mut failures[failure_count], &name);
                    failure_lens[failure_count] = l;
                    failure_count += 1;
                }
                continue;
            }
            let reader_text =
                core::str::from_utf8(&doc[..doc_len]).expect("reader text is not UTF-8");

            // 3. Every byte of every line has to be drawable by the
            //    platform font.
            let mut undrawable = false;
            for line in reader_text.lines() {
                for &b in line.as_bytes() {
                    if !(32..=126).contains(&b) {
                        undrawable = true;
                        break;
                    }
                }
                if undrawable {
                    break;
                }
            }

            // 4. The document must be content, not padding. The live
            //    article's markup indentation produced 688 blank lines out
            //    of 958 before the whitespace rule was fixed.
            let mut total = 0usize;
            let mut blank = 0usize;
            let mut non_blank = 0usize;
            let mut last_line: &[u8] = b"";
            for line in reader_text.lines() {
                total += 1;
                if line.trim().is_empty() {
                    blank += 1;
                } else {
                    non_blank += 1;
                    last_line = line.as_bytes();
                }
            }
            let blank_ratio = if total == 0 { 0usize } else { (blank * 100) / total };

            // 5. A truncated document ends mid-tag.
            let mut cut = false;
            if let Some(&last) = last_line.last() {
                cut = last == b'<' || last == b'=';
            }

            // WHY the blank test needs a floor as well as a ratio: a
            // five-line page with three paragraph gaps is 60% blank and
            // is perfectly normal. What is wrong is MANY blank lines
            // relative to the content, which is what the markup's
            // indentation produced.
            let padding_heavy = blank > 8 && blank_ratio > 50;
            if undrawable || non_blank == 0 || padding_heavy || cut {
                if failure_count < MAX_FAILURES {
                    let l = copy_fail(&mut failures[failure_count], &name);
                    failure_lens[failure_count] = l;
                    failure_count += 1;
                }
            }

            std::eprintln!(
                "{:24} html={:>7} reader={:>6} lines={:>4} non-blank={:>4} blank={:>2}%{}",
                core::str::from_utf8(name).unwrap_or("?"),
                html_len,
                doc_len,
                total,
                non_blank,
                blank_ratio,
                if undrawable { " UNDRAWABLE" } else { "" }
            );
            checked += 1;
        }

        assert!(checked > 0, "no site fixtures were found");
        assert!(
            failure_count == 0,
            "the reader failed on {failure_count} of {checked} real sites"
        );
    }

    /// Copies a fixture name into a fixed failure slot.
    fn copy_fail(slot: &mut [u8; 96], name: &[u8]) -> usize {
        let take = name.len().min(slot.len());
        slot[..take].copy_from_slice(&name[..take]);
        take
    }

    /// The reader pipeline, exercised against a real page captured from the
    /// Device Studio proxy.
    ///
    /// WHY this reads a file rather than an inline string: a hand-written
    /// fixture proves only that the parser handles the markup its author
    /// imagined. The live page is 57 KB of real markup, and the pipeline
    /// has to survive it.
    ///
    /// The test is skipped when the fixture is absent, so the suite still
    /// runs on a machine with no Studio. Capture one with:
    ///   python tools/capture_browser_fixture.py
    #[test]
    fn test_the_reader_pipeline_survives_a_live_page() {
        let Ok(path) = std::env::var("INTEROS_BROWSER_FIXTURE") else {
            std::eprintln!("skipping: INTEROS_BROWSER_FIXTURE is not set");
            return;
        };
        let Ok(raw) = std::fs::read(&path) else {
            panic!("INTEROS_BROWSER_FIXTURE points at {path}, which could not be read");
        };
        assert!(raw.len() > 1024, "the captured proxy response is implausibly small");

        // 1. The HTML is extracted from the proxy's JSON envelope.
        let mut html = [0u8; MAX_HTML_BYTES];
        let html_len = extract_proxy_html(&raw, &mut html);
        assert!(html_len > 0, "no HTML was extracted from the proxy response");
        assert!(html_len <= MAX_HTML_BYTES);

        let html_text = core::str::from_utf8(&html[..html_len])
            .expect("the extracted HTML is not valid UTF-8");
        assert!(
            html_text.contains("<html") || html_text.contains("<!doctype"),
            "the extracted HTML does not look like a document"
        );

        // 2. The document is parsed from it.
        let mut doc = [0u8; MAX_READER_BYTES];
        let doc_len = html_to_reader_document(&html_text.as_bytes(), &mut doc);
        assert!(doc_len > 0, "the reader produced an empty document");
        assert!(doc_len <= MAX_READER_BYTES);

        let reader_text = core::str::from_utf8(&doc[..doc_len]).expect("reader text is not UTF-8");

        // 3. The document carries the page's real structure, not just the
        //    first few hundred bytes of its <head>.
        let lower = reader_text.to_ascii_lowercase();
        assert!(
            lower.contains("interos"),
            "the reader text does not mention the page it came from"
        );
        assert!(
            reader_text.contains('#') || reader_text.contains("InterOS"),
            "the reader text carries no heading at all"
        );

        // 4. The pipeline did not simply stop at the first buffer boundary.
        //    A truncated document ends mid-attribute or mid-sentence, so the
        //    last line must still be a complete one.
        let last_line = reader_text
            .split('\n')
            .filter(|line| !line.trim().is_empty())
            .last()
            .expect("the reader text has no lines");
        assert!(
            !last_line.ends_with('<') && !last_line.ends_with("="),
            "the reader text was cut mid-tag: {last_line:?}"
        );
    }

    /// The search suggestion has to name the host the query really goes to.
    ///
    /// WHY: the card used to read "Search Web" over a request that was
    /// sent to one host, so the screen promised a web search the browser
    /// does not perform.
    #[test]
    fn test_the_search_label_names_the_host_the_query_goes_to() {
        assert!(
            core::str::from_utf8(SEARCH_LABEL)
                .unwrap()
                .contains(SEARCH_HOST),
            "the search label does not name the host the query is sent to"
        );
        assert_ne!(
            SEARCH_LABEL,
            &b"Search Web"[..],
            "the search label still promises a web search"
        );
        // The endpoint and the label have to agree about the host.
        let endpoint = core::str::from_utf8(SEARCH_ENDPOINT).unwrap();
        assert!(
            endpoint.contains(SEARCH_HOST),
            "the search endpoint goes somewhere the label does not name"
        );
    }

    /// The query the user typed has to reach the endpoint the label names.
    #[test]
    fn test_a_search_query_is_sent_to_the_named_endpoint() {
        let mut out = [0u8; 128];
        let n = search_url(b"interos kernel", &mut out);
        let built = &out[..n];
        assert_eq!(&built[..SEARCH_ENDPOINT.len()], SEARCH_ENDPOINT);
        assert_eq!(&built[SEARCH_ENDPOINT.len()..], b"interos kernel");
        assert!(core::str::from_utf8(built)
            .unwrap()
            .starts_with("https://interos.tech/search?q="));
    }

    /// An address the browser cannot parse must not be shown as a site.
    ///
    /// WHY: the address bar used to substitute `interos.tech` for an
    /// address that named no host, so the page that came back was
    /// presented as though the user had asked for that site.
    #[test]
    fn test_an_address_that_names_no_host_is_not_recognised() {
        assert_eq!(classify_address(b"hello"), AddressOutcome::NoScheme);
        assert_eq!(classify_address(b"ftp://example.org"), AddressOutcome::NoScheme);
        assert_eq!(classify_address(b"http://"), AddressOutcome::NoHost);
        assert_eq!(
            classify_address(b"gemini://geminispace.info"),
            AddressOutcome::UnsupportedScheme
        );
        assert_eq!(
            classify_address(b"https://interos.tech"),
            AddressOutcome::Fetchable
        );
        assert_eq!(
            classify_address(b"http://example.org/page"),
            AddressOutcome::Fetchable
        );
        assert_eq!(
            classify_address(b"file:///storage/downloads/page_0.html"),
            AddressOutcome::Fetchable
        );
    }

    /// A hostname typed without a scheme is fetched, and a bare word is not.
    ///
    /// WHY: the omnibox has to accept `example.com` the way a person writes
    /// it, while `hello` still names nothing the browser can fetch.
    #[test]
    fn test_a_bare_hostname_is_given_the_https_scheme() {
        let mut out = [0u8; 128];

        let n = normalize_address(b"example.com", &mut out);
        assert_eq!(&out[..n], b"https://example.com");
        assert_eq!(classify_address(&out[..n]), AddressOutcome::Fetchable);

        let n = normalize_address(b"  example.com/path?q=1  ", &mut out);
        assert_eq!(&out[..n], b"https://example.com/path?q=1");
        assert_eq!(parse_uri(&out[..n]).path, b"/path");

        let n = normalize_address(b"http://example.com", &mut out);
        assert_eq!(&out[..n], b"http://example.com");

        let n = normalize_address(b"hello", &mut out);
        assert_eq!(&out[..n], b"hello");
        assert_eq!(classify_address(&out[..n]), AddressOutcome::NoScheme);
    }

    /// The reader's scroll extent has to cover the whole document.
    ///
    /// WHY: the page body used to stop after a fixed number of lines and
    /// say so, and there was no way to reach the rest of the page.
    #[test]
    fn test_the_reader_scroll_extent_covers_the_whole_document() {
        let lines = [
            DocLine {
                len: 3,
                kind: LineKind::Paragraph,
                ..DocLine::EMPTY
            },
            DocLine::EMPTY,
        ];
        let height = document_height_px(&lines);
        assert_eq!(height, doc_line_height(LineKind::Paragraph, b"abc") + 14);

        // A document taller than the viewport can be scrolled through, and
        // the scroll stops at the end of the document.
        let viewport = 20usize;
        let mut scroll = reader_scroll(height, viewport);
        assert_eq!(scroll.max_offset(), height.saturating_sub(viewport));
        assert_eq!(scroll.offset, 0);
        scroll.offset = scroll.max_offset();
        assert_eq!(scroll.offset + viewport, height);

        // A document that fits does not scroll at all.
        let short = reader_scroll(10, viewport);
        assert_eq!(short.max_offset(), 0);
        assert!(short.thumb_rect(0, 100, 20).is_none());

        // A drag past the end of the document stops at the end of it.
        let mut dragged = reader_scroll(height, viewport);
        dragged.apply_drag(-1000);
        assert_eq!(dragged.offset, dragged.max_offset());
        dragged.apply_drag(1000);
        assert_eq!(dragged.offset, 0);
    }

    /// The path a page is saved to has to be the one the platform's index
    /// can be asked about.
    #[test]
    fn test_a_saved_page_has_a_path_the_index_can_find() {
        let mut out = [0u8; 64];
        let n = download_path(7, &mut out);
        let path = core::str::from_utf8(&out[..n]).unwrap();
        assert_eq!(path, "/storage/downloads/page_7.html");
        assert!(is_download_path(path));
        assert!(!is_download_path("/storage/photos/holiday.ppm"));
        assert!(!is_download_path("/storage/downloads/notes.txt"));
    }

    /// A filter that matched only whole words would find nothing.
    #[test]
    fn test_the_panel_filter_ignores_case() {
        assert!(contains_ignore_case(b"https://InterOS.tech/docs", b"interos"));
        assert!(contains_ignore_case(b"https://interos.tech", b"INTEROS.TECH"));
        assert!(!contains_ignore_case(b"https://example.org", b"interos"));
        assert!(contains_ignore_case(b"anything", b""));
    }

    /// A page is a document and an image is not; the browser has to know
    /// which bytes to decode as text and which to keep as a file.
    #[test]
    fn test_binary_content_types_are_not_documents() {
        assert!(content_type_is_binary(b"image/png"));
        assert!(content_type_is_binary(b"image/jpeg; charset=binary"));
        assert!(content_type_is_binary(b"audio/mpeg"));
        assert!(content_type_is_binary(b"video/mp4"));
        assert!(content_type_is_binary(b"application/octet-stream"));
        assert!(content_type_is_binary(b"application/pdf"));
        assert!(!content_type_is_binary(b"text/html; charset=utf-8"));
        assert!(!content_type_is_binary(b"text/plain"));
        assert!(!content_type_is_binary(b"application/json"));
        assert!(!content_type_is_binary(b"application/xhtml+xml"));
        assert!(!content_type_is_binary(b"image/svg+xml"));
        assert!(!content_type_is_binary(b""));
    }

    /// The file keeps the origin's name and the type's extension, so the
    /// app that opens it is chosen by what the file is.
    #[test]
    fn test_a_download_keeps_its_name_and_gets_the_types_extension() {
        let mut out = [0u8; 96];

        let n = download_filename(
            b"https://upload.wikimedia.org/wikipedia/commons/4/47/demo.png",
            b"image/png",
            None,
            &mut out,
        );
        assert_eq!(core::str::from_utf8(&out[..n]).unwrap(), "demo.png");

        // No extension in the URL: the content type supplies it.
        let n = download_filename(b"https://example.com/media/photo", b"image/jpeg", None, &mut out);
        assert_eq!(core::str::from_utf8(&out[..n]).unwrap(), "photo.jpg");

        // The server's own name wins, and a mismatched extension is replaced.
        let n = download_filename(
            b"https://example.com/image.php",
            b"image/png",
            Some(b"holiday.php"),
            &mut out,
        );
        assert_eq!(core::str::from_utf8(&out[..n]).unwrap(), "holiday.png");

        // A query is not part of the name, and unsafe characters are replaced.
        let n = download_filename(
            b"https://example.com/get?file=a%20b.mp3",
            b"audio/mpeg",
            None,
            &mut out,
        );
        assert_eq!(core::str::from_utf8(&out[..n]).unwrap(), "get.mp3");

        // An unknown type still produces a file with a usable name.
        let n = download_filename(b"https://example.com/archive", b"application/x-thing", None, &mut out);
        assert_eq!(core::str::from_utf8(&out[..n]).unwrap(), "archive.bin");
    }

    /// The header the gateway adds names the file; the body must not be
    /// mistaken for a header.
    #[test]
    fn test_the_download_name_header_is_read_from_the_header_section() {
        let raw = b"HTTP/1.0 200 OK\r\nContent-Type: image/png\r\nX-Interos-Filename: demo.png\r\n\r\nname: in-the-body.png";
        assert_eq!(http_header_value(raw, b"x-interos-filename"), Some(&b"demo.png"[..]));
        assert_eq!(http_header_value(raw, b"content-type"), Some(&b"image/png"[..]));
        assert_eq!(http_header_value(raw, b"missing"), None);
    }

    /// A downloaded file lives beside the saved pages but is addressed as
    /// its own file, never as a page.
    #[test]
    fn test_a_download_path_names_the_file_under_the_downloads_directory() {
        let mut out = [0u8; 96];
        let n = media_download_path(4, b"demo.png", &mut out);
        assert_eq!(
            core::str::from_utf8(&out[..n]).unwrap(),
            "/storage/downloads/dl_4_demo.png"
        );
        assert!(!is_download_path(core::str::from_utf8(&out[..n]).unwrap()));
    }

    /// A server-supplied name is a file name, not a path. Whatever the
    /// origin sends, the saved file stays directly under /storage/downloads.
    #[test]
    fn test_a_download_name_cannot_escape_the_downloads_directory() {
        let hostile: [&[u8]; 7] = [
            b"../../etc/passwd",
            b"..\\..\\windows\\system32\\cfg",
            b"....//....//secret",
            b"name%2F..%2Fescape",
            b"/absolute/path.png",
            b"..",
            b".",
        ];
        for raw in hostile {
            let mut out = [0u8; 96];
            let n = download_filename(
                b"https://example.com/get",
                b"application/octet-stream",
                Some(raw),
                &mut out,
            );
            let name = core::str::from_utf8(&out[..n]).unwrap();
            assert!(!name.contains('/'), "name {name:?} kept a slash");
            assert!(!name.contains('\\'), "name {name:?} kept a backslash");
            assert!(!name.is_empty(), "name from {raw:?} was empty");

            let mut path = [0u8; 128];
            let pn = media_download_path(1, name.as_bytes(), &mut path);
            let path = core::str::from_utf8(&path[..pn]).unwrap();
            assert!(path.starts_with("/storage/downloads/dl_1_"), "path {path:?}");
            assert_eq!(
                path.matches('/').count(),
                3,
                "path {path:?} left the downloads directory"
            );
        }
    }

    /// The upload request carries the file as its body, and the name
    /// cannot break out of the header it travels in.
    #[test]
    fn test_the_upload_request_names_the_file_without_header_injection() {
        let mut out = [0u8; 512];
        let n = format_upload_request(
            b"/storage/downloads/evil\r\nX-Injected: 1.png",
            b"image/png; charset=binary",
            12345,
            &mut out,
        );
        let text = core::str::from_utf8(&out[..n]).unwrap();
        assert!(text.starts_with("POST /api/browser/upload HTTP/1.1\r\n"));
        assert!(text.contains("Content-Length: 12345\r\n"));
        assert!(text.contains("Content-Type: image/png\r\n"));
        assert!(text.contains("X-Interos-Filename: evilX-Injected_1.png\r\n"));
        // The name introduced no second header line.
        assert_eq!(text.matches("\r\nX-Injected:").count(), 0);
        assert_eq!(text.matches("X-Interos-Filename:").count(), 1);
        assert!(text.ends_with("\r\n\r\n"));
    }

    /// A name that sanitises to nothing still produces a usable file name.
    #[test]
    fn test_an_upload_with_no_usable_name_still_names_a_file() {
        let mut out = [0u8; 256];
        let n = format_upload_request(b"///", b"", 7, &mut out);
        let text = core::str::from_utf8(&out[..n]).unwrap();
        assert!(text.contains("X-Interos-Filename: upload.bin\r\n"));
        assert!(text.contains("Content-Type: application/octet-stream\r\n"));
    }

    /// The speed-test requests name the exact payload the gateway will
    /// send or expect.
    #[test]
    fn test_speedtest_requests_name_their_byte_count() {
        let mut out = [0u8; 256];
        let n = format_speedtest_download_request(131072, &mut out);
        let text = core::str::from_utf8(&out[..n]).unwrap();
        assert!(text.starts_with("GET /api/speedtest/download?bytes=131072 "));
        assert!(text.contains("Host: 127.0.0.1:4300\r\n"));

        let n = format_speedtest_upload_request(65536, &mut out);
        let text = core::str::from_utf8(&out[..n]).unwrap();
        assert!(text.starts_with("POST /api/speedtest/upload HTTP/1.1\r\n"));
        assert!(text.contains("Content-Length: 65536\r\n"));
    }

    /// The gateway's byte count is read only when the field is a number.
    #[test]
    fn test_the_gateway_byte_count_is_read_from_its_reply() {
        let json = br#"{"success": true, "received_bytes": 102400, "sha256": "ab"}"#;
        assert_eq!(json_u64_field(json, b"received_bytes"), Some(102400));
        assert_eq!(json_u64_field(json, b"missing"), None);
        assert_eq!(json_u64_field(br#"{"received_bytes": "nope"}"#, b"received_bytes"), None);
        assert_eq!(MAX_UPLOAD_BYTES, 4 * 1024 * 1024);
    }

    /// The media type handed to the handler registry carries no parameters,
    /// because the registry keys a handler by the type itself.
    #[test]
    fn test_the_bare_media_type_drops_parameters() {
        let mut out = [0u8; 64];
        let n = bare_mime(b"image/png; charset=binary", &mut out);
        assert_eq!(core::str::from_utf8(&out[..n]).unwrap(), "image/png");

        let n = bare_mime(b"", &mut out);
        assert_eq!(
            core::str::from_utf8(&out[..n]).unwrap(),
            "application/octet-stream"
        );
    }

    #[test]
    fn test_uri_parser_case_and_port_overflow() {
        let uri = parse_uri(b"HTTPS://EXAMPLE.COM:8080/path");
        assert_eq!(uri.scheme, UriScheme::Https);
        assert_eq!(uri.host, b"EXAMPLE.COM");
        assert_eq!(uri.port, 8080);
        assert_eq!(uri.path, b"/path");

        // Port > 65535 should fall back to scheme default port
        let overflow = parse_uri(b"http://example.com:99999/test");
        assert_eq!(overflow.port, 80);
    }

    #[test]
    fn test_http_response_rejects_non_digit_status() {
        assert_eq!(parse_http_response(b"HTTP/1.1 20a OK\r\n\r\nbody"), None);
        assert_eq!(parse_http_response(b"HTTP/1.1 ABC OK\r\n\r\nbody"), None);
        assert_eq!(parse_http_response(b"HTTP/1.1 200 OK\r\n\r\nbody").unwrap().status_code, 200);
    }

    #[test]
    fn test_utf8_decode_security() {
        // Overlong 2-byte NUL: 0xC0 0x80
        let (cp, next) = decode_utf8_at(&[0xC0, 0x80], 0);
        assert_eq!(cp, 0);
        assert_eq!(next, 1);

        // Surrogate 0xD800: 0xED 0xA0 0x80
        let (cp, next) = decode_utf8_at(&[0xED, 0xA0, 0x80], 0);
        assert_eq!(cp, 0);
        assert_eq!(next, 1);

        // Out of Unicode range > 0x10FFFF: 0xF4 0x90 0x80 0x80
        let (cp, next) = decode_utf8_at(&[0xF4, 0x90, 0x80, 0x80], 0);
        assert_eq!(cp, 0);
        assert_eq!(next, 1);

        // Valid 3-byte character
        let (cp, next) = decode_utf8_at("€".as_bytes(), 0);
        assert_eq!(cp, 0x20AC);
        assert_eq!(next, 3);
    }

    #[test]
    fn test_proxy_html_surrogate_pairs() {
        let mut out = [0u8; 128];
        let json = br#"{"html": "Hello \uD83D\uDE00 World!"}"#;
        let n = extract_proxy_html(json, &mut out);
        assert_eq!(core::str::from_utf8(&out[..n]).unwrap(), "Hello 😀 World!");

        let unpaired = br#"{"html": "Bad \uD83D alone"}"#;
        let n = extract_proxy_html(unpaired, &mut out);
        assert_eq!(core::str::from_utf8(&out[..n]).unwrap(), "Bad ? alone");
    }

    #[test]
    fn test_raw_text_case_insensitive_close_and_unclosed_comment() {
        let mut out = [0u8; 256];
        let html = b"<SCRIPT>var x = 1;</SCRIPT><p>Article content</p>";
        let n = html_to_reader_document(html, &mut out);
        let text = core::str::from_utf8(&out[..n]).unwrap();
        assert!(text.contains("Article content"));

        // Unclosed comment at EOF should not leak trailing bytes
        let mut out2 = [0u8; 256];
        let html2 = b"<p>Start</p><!-- Unclosed comment at EOF";
        let n2 = html_to_reader_document(html2, &mut out2);
        let text2 = core::str::from_utf8(&out2[..n2]).unwrap();
        assert_eq!(text2.trim(), "Start");
    }

    #[test]
    fn test_attr_value_boundary_and_case() {
        let tag = b"img datasrc=\"fake.png\" SRC='real.png' width=100";
        assert_eq!(attr_value(tag, b"src"), Some(&b"real.png"[..]));
        assert_eq!(attr_value(tag, b"width"), Some(&b"100"[..]));
        assert_eq!(attr_value(tag, b"datasrc"), Some(&b"fake.png"[..]));
        assert_eq!(attr_value(tag, b"missing"), None);
    }

    #[test]
    fn test_wrap_line_narrow_chars_no_loss() {
        // A line with 150 'i' characters (narrow). Even if measure <= max_w,
        // it must wrap without dropping any characters.
        let text = [b'i'; 150];
        let mut out = [[0u8; MAX_LINE_BYTES]; MAX_WRAP_PIECES];
        let mut lens = [0usize; MAX_WRAP_PIECES];
        let pieces = wrap_line(&text, 1, 1000, &mut out, &mut lens);
        assert!(pieces >= 2);
        let mut total_bytes = 0usize;
        for i in 0..pieces {
            total_bytes += lens[i];
        }
        assert_eq!(total_bytes, 150);
    }

    #[test]
    fn test_form_urlencode_get_query() {
        let base = b"https://example.com/search";
        let pairs: &[(&[u8], &[u8])] = &[
            (b"q", b"hello world"),
            (b"page", b"1"),
        ];
        let mut out = [0u8; 256];
        let n = http::form_urlencode_url(base, pairs, &mut out).expect("encoding failed");
        assert_eq!(&out[..n], b"https://example.com/search?q=hello+world&page=1");
    }

    #[test]
    fn test_form_urlencode_post_body() {
        let pairs: &[(&[u8], &[u8])] = &[
            (b"user", b"john doe"),
            (b"secret", b"a&b=c"),
        ];
        let mut out = [0u8; 256];
        let n = http::form_urlencode(pairs, &mut out).expect("encoding failed");
        assert_eq!(&out[..n], b"user=john+doe&secret=a%26b%3Dc");
    }

    #[test]
    fn test_html_document_load_and_layout() {
        let markup = b"<html><body>\
            <h1>Test Document</h1>\
            <p>Here is a paragraph with a <a href='/page2'>link</a>.</p>\
            <form action='/login' method='post'>\
                <input type='text' name='user' value='admin'>\
                <input type='submit' value='Log In'>\
            </form>\
            </body></html>";

        let mut dom = layout::Dom::new();
        dom.parse(markup);
        assert!(dom.node_count() > 1);

        let sheet = css::parse_stylesheet(dom.css());
        let mut layout = layout::TextLayout::new();
        layout::build(&dom, &sheet, 672, &mut layout);

        assert!(!layout.is_empty());
        assert!(layout.height > 0);
        assert!(!layout.truncated);
        assert_eq!(layout.link_count, 1);
        assert_eq!(layout.field_count, 2);

        // Verify link resolves to /page2
        let link_box = layout.links[0];
        assert_eq!(dom.link_href(link_box.link), Some(&b"/page2"[..]));
        assert!(layout.link_at(link_box.x as usize + 1, link_box.y as usize + 1).is_some());

        // Verify form fields
        let user_field = layout.fields[0];
        assert_eq!(user_field.name.get(&dom), b"user");
        assert_eq!(user_field.value.get(&dom), b"admin");
        assert!(layout.field_at(user_field.x as usize + 1, user_field.y as usize + 1).is_some());
    }

    #[test]
    fn test_pending_download_stage_and_cancel() {
        let mut pd = PendingDownload::new(
            b"https://example.com/assets/report.pdf",
            b"application/pdf",
            Some(b"financial_report.pdf"),
            1024,
            64,
            false,
        );
        assert!(pd.active);
        assert_eq!(&pd.url[..pd.url_len], b"https://example.com/assets/report.pdf");
        assert_eq!(&pd.content_type[..pd.content_type_len], b"application/pdf");
        assert_eq!(&pd.filename[..pd.filename_len], b"financial_report.pdf");
        assert_eq!(pd.body_len, 1024);
        assert_eq!(pd.body_offset, 64);
        assert!(!pd.truncated);

        pd.cancel();
        assert!(!pd.active);
    }

    // -------------------------------------------------------------------------
    // The candidate string and the failure detail
    // -------------------------------------------------------------------------
    #[test]
    fn test_a_candidate_that_fits_is_shown_whole() {
        let mut buf = [0u8; 256];
        let n = fit_suggestion(b"https://interos.tech", &mut buf);
        assert_eq!(&buf[..n], b"https://interos.tech");
    }

    #[test]
    fn test_a_candidate_that_does_not_fit_is_marked_as_continuing() {
        let mut buf = [0u8; 256];
        let long = b"https://example.com/a/path/that/keeps/going/and/going/until/it/no/longer/\
fits/in/the/row/and/the/row/is/still/not/wide/enough/to/be/cut/at/all/by/the/row/it/owns";
        let n = fit_suggestion(long, &mut buf);
        assert!(n < long.len(), "the whole candidate was drawn past the row");
        assert_eq!(
            &buf[n - 3..n],
            b"...",
            "a cut candidate carries no mark, so it reads as a whole address"
        );
        assert!(
            crate::std::design::font::measure(&buf[..n], 1) <= SUGGESTION_TEXT_MAX,
            "the marked candidate is still wider than the row"
        );
    }

    #[test]
    fn test_the_gateway_reason_is_read_out_of_its_envelope() {
        let mut buf = [0u8; GATEWAY_REASON_MAX + 3];
        let envelope =
            br#"{"success": false, "url": "https://example.com", "error": "SSRF Blocked: host is not public"}"#;
        let n = gateway_failure_reason(envelope, &mut buf);
        assert_eq!(&buf[..n], b"SSRF Blocked: host is not public");
    }

    #[test]
    fn test_an_envelope_that_names_no_reason_gives_no_reason() {
        let mut buf = [0u8; GATEWAY_REASON_MAX + 3];
        let envelope = br#"{"success": false, "url": "https://example.com"}"#;
        assert_eq!(gateway_failure_reason(envelope, &mut buf), 0);
    }

    #[test]
    fn test_a_reason_that_does_not_fit_is_marked_as_continuing() {
        let mut envelope = [0u8; 512];
        let mut n = 0usize;
        for &byte in br#"{"success": false, "error": ""#.iter() {
            envelope[n] = byte;
            n += 1;
        }
        for _ in 0..400 {
            envelope[n] = b'x';
            n += 1;
        }
        envelope[n] = b'"';
        n += 1;

        let mut buf = [0u8; GATEWAY_REASON_MAX + 3];
        let written = gateway_failure_reason(&envelope[..n], &mut buf);
        assert!(written > 0, "a reason the gateway gave was thrown away");
        assert_eq!(
            &buf[written - 3..written],
            b"...",
            "a cut reason carries no mark, so it reads as the whole reason"
        );
    }

    #[test]
    fn test_the_reason_line_names_the_gateway_and_marks_a_cut_reason() {
        let mut buf = [0u8; GATEWAY_REASON_LINE_MAX + 3];
        let n = gateway_reason_line(b"SSRF Blocked: host is not public", &mut buf);
        assert_eq!(&buf[..n], b"Gateway said: SSRF Blocked: host is not public");

        let long = b"a reason that is far longer than the line the screen gives it can ever hold";
        let n = gateway_reason_line(long, &mut buf);
        assert!(n < long.len() + GATEWAY_REASON_LABEL.len());
        assert_eq!(&buf[n - 3..n], b"...", "a cut reason line carries no mark");
        assert!(
            crate::std::design::font::measure(&buf[..n], 1) <= GATEWAY_REASON_WIDTH,
            "the reason line is wider than the block it belongs to"
        );
    }
}
