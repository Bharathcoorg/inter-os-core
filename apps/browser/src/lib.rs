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

/// Parses a URI byte string into its protocol scheme, host, path, port, and query string.
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
    let (authority, remainder) = match delim_pos {
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
            if all_digits && value > 0 {
                (&authority[..i], value)
            } else if all_digits {
                (&authority[..i], 0)
            } else {
                (authority, scheme.default_port())
            }
        }
        None => (authority, scheme.default_port()),
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
    Paragraph,
    Empty,
}

/// Classifies a single line of text for document rendering without dynamic allocation.
pub fn classify_line(line: &[u8]) -> LineKind {
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
        let clean_line = if !line.is_empty() && line[line.len() - 1] == b'\r' {
            &line[..line.len() - 1]
        } else {
            line
        };
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
pub const MAX_DOC_LINES: usize = 32;
pub const MAX_LINE_BYTES: usize = 96;

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

#[cfg(test)]
mod tests {
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
}
