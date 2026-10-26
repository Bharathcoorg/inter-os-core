//! The browser's bounded HTTP/1.1 protocol layer.
//!
//! This module owns the parts of HTTP that sit between the transport and the
//! reader: redirect resolution, the cookie jar, form bodies, conditional
//! requests, charset conversion and the gateway request/envelope format. It
//! is `no_std`, allocates nothing and puts every buffer size in a constant so
//! a hostile header cannot grow the app's memory.
//!
//! What is deliberately *not* here: HTML parsing and CSS layout. The reader
//! pipeline in `lib.rs` owns that, and this module hands it bytes in UTF-8.

use crate::{parse_uri, UriScheme};

// =============================================================================
// Header access
// =============================================================================

/// The largest URL a redirect or cache key will be stored as.
pub const MAX_URL_BYTES: usize = 512;

/// Returns the header section of a raw HTTP response (without the status
/// line and without the terminating blank line).
pub fn header_section(raw: &[u8]) -> &[u8] {
    if let Some(position) = raw.windows(4).position(|w| w == b"\r\n\r\n") {
        return &raw[..position];
    }
    if let Some(position) = raw.windows(2).position(|w| w == b"\n\n") {
        return &raw[..position];
    }
    raw
}

/// Calls `visit(name, value)` for each header field in a raw response.
///
/// The status line is skipped, obs-fold continuation lines are not joined
/// (the response is treated as untrusted input), and every field's name is
/// returned exactly as it arrived. Values are trimmed of surrounding spaces.
pub fn for_each_header<'a, F: FnMut(&'a [u8], &'a [u8])>(raw: &'a [u8], mut visit: F) {
    let section = header_section(raw);
    let mut first = true;
    for line in section.split(|&b| b == b'\n') {
        let line = if !line.is_empty() && line[line.len() - 1] == b'\r' {
            &line[..line.len() - 1]
        } else {
            line
        };
        if first {
            first = false;
            continue;
        }
        let Some(colon) = line.iter().position(|&b| b == b':') else {
            continue;
        };
        let name = trim_ascii(&line[..colon]);
        let mut value = &line[colon + 1..];
        while !value.is_empty() && (value[0] == b' ' || value[0] == b'\t') {
            value = &value[1..];
        }
        while !value.is_empty()
            && (value[value.len() - 1] == b' ' || value[value.len() - 1] == b'\t')
        {
            value = &value[..value.len() - 1];
        }
        visit(name, value);
    }
}

/// Finds the first value of a header, case-insensitively.
pub fn find_header<'a>(raw: &'a [u8], wanted: &[u8]) -> Option<&'a [u8]> {
    let mut found = None;
    for_each_header(raw, |name, value| {
        if found.is_none() && name.eq_ignore_ascii_case(wanted) {
            found = Some(value);
        }
    });
    found
}

/// Finds every value of a repeated header, calling `visit` in wire order.
pub fn for_each_header_value<'a, F: FnMut(&'a [u8])>(
    raw: &'a [u8],
    wanted: &[u8],
    mut visit: F,
) {
    for_each_header(raw, |name, value| {
        if name.eq_ignore_ascii_case(wanted) {
            visit(value);
        }
    });
}

fn trim_ascii(raw: &[u8]) -> &[u8] {
    let mut start = 0;
    let mut end = raw.len();
    while start < end && (raw[start] == b' ' || raw[start] == b'\t' || raw[start] == b'\r') {
        start += 1;
    }
    while end > start && (raw[end - 1] == b' ' || raw[end - 1] == b'\t' || raw[end - 1] == b'\r') {
        end -= 1;
    }
    &raw[start..end]
}

// =============================================================================
// Redirects
// =============================================================================

/// How many redirects a navigation follows before it stops.
///
/// WHY a small fixed number: a chain longer than this is either a loop the
/// URL comparison did not catch or a server that never settles. Both are
/// reported to the reader instead of being chased forever.
pub const MAX_REDIRECTS: u8 = 8;

/// True for the status codes that carry a `Location`.
pub fn status_is_redirect(status: u16) -> bool {
    matches!(status, 301 | 302 | 303 | 307 | 308)
}

/// The method the browser uses for a redirect hop.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HttpMethod {
    Get,
    Post,
}

impl HttpMethod {
    pub fn as_str(self) -> &'static [u8] {
        match self {
            HttpMethod::Get => b"GET",
            HttpMethod::Post => b"POST",
        }
    }
}

/// The method a redirect status requires.
///
/// 303 always becomes GET. 301 and 302 are treated the way browsers treat
/// them: a POST becomes a GET and loses its body, while a GET stays a GET.
/// 307 and 308 preserve both the method and the body. This is a deliberate
/// browser-compatible reading of RFC 7231, not a byte-for-byte copy of it.
pub fn method_for_redirect(status: u16, original: HttpMethod) -> HttpMethod {
    match status {
        303 => HttpMethod::Get,
        301 | 302 => match original {
            HttpMethod::Post => HttpMethod::Get,
            HttpMethod::Get => HttpMethod::Get,
        },
        307 | 308 => original,
        _ => original,
    }
}

fn origin_end(url: &[u8]) -> Option<usize> {
    let scheme = url.windows(3).position(|w| w == b"://")?;
    let mut end = scheme + 3;
    while end < url.len() && url[end] != b'/' && url[end] != b'?' && url[end] != b'#' {
        end += 1;
    }
    Some(end)
}

/// Normalizes a path by removing `.` and `..` segments (RFC 3986 5.2.4).
fn normalize_path(path: &[u8], out: &mut [u8]) -> Option<usize> {
    if out.is_empty() {
        return None;
    }
    out[0] = b'/';
    let mut written = 1usize;
    let mut index = 0usize;
    while index < path.len() {
        while index < path.len() && path[index] == b'/' {
            index += 1;
        }
        let start = index;
        while index < path.len() && path[index] != b'/' {
            index += 1;
        }
        let segment = &path[start..index];
        if segment.is_empty() || segment == b"." {
            continue;
        }
        if segment == b".." {
            if written > 1 {
                written -= 1;
                while written > 1 && out[written - 1] != b'/' {
                    written -= 1;
                }
                if written > 1 {
                    written -= 1;
                }
            }
            continue;
        }
        if written > 1 {
            if written >= out.len() {
                return None;
            }
            out[written] = b'/';
            written += 1;
        }
        if written + segment.len() > out.len() {
            return None;
        }
        out[written..written + segment.len()].copy_from_slice(segment);
        written += segment.len();
    }
    // A path that ended in a slash (or in `.`/`..`) names a directory and
    // keeps one trailing slash (RFC 3986 5.2.4).
    let directory_ending = path.ends_with(b"/")
        || path.ends_with(b"/.")
        || path.ends_with(b"/..")
        || path == b"."
        || path == b"..";
    if directory_ending && out[written - 1] != b'/' {
        if written >= out.len() {
            return None;
        }
        out[written] = b'/';
        written += 1;
    }
    Some(written)
}

fn normalize_absolute(url: &[u8], out: &mut [u8]) -> Option<usize> {
    let end_of_origin = origin_end(url)?;
    let rest = &url[end_of_origin..];
    let query_at = rest.iter().position(|&b| b == b'?' || b == b'#');
    let (path, query) = match query_at {
        Some(position) => {
            let candidate = &rest[position..];
            if candidate.starts_with(b"?") {
                (&rest[..position], Some(candidate))
            } else {
                (&rest[..position], None)
            }
        }
        None => (rest, None),
    };
    let path = if path.is_empty() { b"/" as &[u8] } else { path };
    let mut normalized = [0u8; MAX_URL_BYTES];
    let path_len = normalize_path(path, &mut normalized)?;
    let query_len = query.map(|value| value.len()).unwrap_or(0);
    let total = end_of_origin + path_len + query_len;
    if total > out.len() {
        return None;
    }
    out[..end_of_origin].copy_from_slice(&url[..end_of_origin]);
    out[end_of_origin..end_of_origin + path_len].copy_from_slice(&normalized[..path_len]);
    if let Some(query) = query {
        out[end_of_origin + path_len..total].copy_from_slice(query);
    }
    Some(total)
}

/// Resolves a `Location` value against the URL that produced it.
///
/// Absolute URLs, scheme-relative `//host/path`, absolute paths, query-only
/// references and relative paths are all handled. Fragments are dropped
/// because they are never sent to a server and would make a redirect loop
/// look different on every hop.
pub fn resolve_url(base: &[u8], reference: &[u8], out: &mut [u8]) -> Option<usize> {
    let reference = trim_ascii(reference);
    if reference.is_empty() {
        return None;
    }
    if reference.starts_with(b"http://") || reference.starts_with(b"https://") {
        return normalize_absolute(reference, out);
    }
    let end_of_origin = origin_end(base)?;
    let mut candidate = [0u8; MAX_URL_BYTES];
    if reference.starts_with(b"//") {
        let scheme_end = base.windows(3).position(|w| w == b"://")?;
        let prefix = &base[..scheme_end + 1]; // includes the ':'
        if prefix.len() + reference.len() > candidate.len() {
            return None;
        }
        candidate[..prefix.len()].copy_from_slice(prefix);
        candidate[prefix.len()..prefix.len() + reference.len()].copy_from_slice(reference);
        return normalize_absolute(&candidate[..prefix.len() + reference.len()], out);
    }

    let base_rest = &base[end_of_origin..];
    let base_path_end = base_rest
        .iter()
        .position(|&b| b == b'?' || b == b'#')
        .unwrap_or(base_rest.len());
    let base_path = &base_rest[..base_path_end];

    let mut length;
    match reference[0] {
        b'/' => {
            candidate[..end_of_origin].copy_from_slice(&base[..end_of_origin]);
            length = end_of_origin;
        }
        b'?' => {
            candidate[..end_of_origin].copy_from_slice(&base[..end_of_origin]);
            length = end_of_origin;
            let path: &[u8] = if base_path.is_empty() { b"/" } else { base_path };
            candidate[length..length + path.len()].copy_from_slice(path);
            length += path.len();
        }
        b'#' => {
            // A fragment-only reference names the same resource.
            return normalize_absolute(base, out);
        }
        _ => {
            candidate[..end_of_origin].copy_from_slice(&base[..end_of_origin]);
            length = end_of_origin;
            let directory_end = base_path
                .iter()
                .rposition(|&b| b == b'/')
                .map(|position| position + 1)
                .unwrap_or(0);
            let directory: &[u8] = if directory_end == 0 {
                b"/"
            } else {
                &base_path[..directory_end]
            };
            candidate[length..length + directory.len()].copy_from_slice(directory);
            length += directory.len();
        }
    }
    if length + reference.len() > candidate.len() {
        return None;
    }
    candidate[length..length + reference.len()].copy_from_slice(reference);
    normalize_absolute(&candidate[..length + reference.len()], out)
}

/// Which redirect rule stopped a navigation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RedirectFault {
    Loop,
    Limit,
}

/// Tracks visited URLs across one navigation.
#[derive(Clone, Copy)]
pub struct RedirectTracker {
    seen: [[u8; 192]; MAX_REDIRECTS as usize + 1],
    seen_len: [u16; MAX_REDIRECTS as usize + 1],
    count: usize,
    hops: u8,
}

impl RedirectTracker {
    pub const fn new() -> Self {
        Self {
            seen: [[0; 192]; MAX_REDIRECTS as usize + 1],
            seen_len: [0; MAX_REDIRECTS as usize + 1],
            count: 0,
            hops: 0,
        }
    }

    /// Records the first URL. A URL too long to track is still navigable;
    /// it simply cannot participate in loop detection, so the hop limit is
    /// the only bound for it.
    pub fn start(&mut self, url: &[u8]) {
        self.seen = [[0; 192]; MAX_REDIRECTS as usize + 1];
        self.seen_len = [0; MAX_REDIRECTS as usize + 1];
        self.count = 0;
        self.hops = 0;
        self.remember(url);
    }

    fn remember(&mut self, url: &[u8]) {
        if url.len() > 192 || self.count >= self.seen.len() {
            return;
        }
        self.seen[self.count][..url.len()].copy_from_slice(url);
        self.seen_len[self.count] = url.len() as u16;
        self.count += 1;
    }

    /// Classifies the next `Location` target.
    pub fn next(&mut self, url: &[u8]) -> Result<(), RedirectFault> {
        for index in 0..self.count {
            let len = self.seen_len[index] as usize;
            if self.seen[index][..len] == *url {
                return Err(RedirectFault::Loop);
            }
        }
        if self.hops >= MAX_REDIRECTS {
            return Err(RedirectFault::Limit);
        }
        self.hops += 1;
        self.remember(url);
        Ok(())
    }

    pub fn hops(&self) -> u8 {
        self.hops
    }
}

/// Builds the truthful reader page shown when redirects stop.
///
/// The page names the limit and the last address. It contains no invented
/// status code and no browser-branded error copy.
pub fn redirect_error_document(fault: RedirectFault, last_url: &[u8], out: &mut [u8]) -> usize {
    let mut writer = 0usize;
    let push = |out: &mut [u8], writer: &mut usize, text: &[u8]| {
        let take = text.len().min(out.len().saturating_sub(*writer));
        out[*writer..*writer + take].copy_from_slice(&text[..take]);
        *writer += take;
    };
    match fault {
        RedirectFault::Loop => {
            push(out, &mut writer, b"Redirect loop\n\n");
            push(
                out,
                &mut writer,
                b"The browser stopped because this address redirected back to one it had already visited.\n\n",
            );
        }
        RedirectFault::Limit => {
            push(out, &mut writer, b"Too many redirects\n\n");
            push(
                out,
                &mut writer,
                b"The browser follows at most 8 redirects. This page did not settle.\n\n",
            );
        }
    }
    push(out, &mut writer, b"Last address: ");
    push(out, &mut writer, last_url);
    push(out, &mut writer, b"\n");
    writer
}

// =============================================================================
// Cookies (RFC 6265 subset)
// =============================================================================
//
// LIMITATION: there is no public-suffix list. A server at
// `example.co.uk` can therefore set a cookie for `co.uk`. The jar enforces
// the RFC's domain-match rule, path-match rule, Secure and HttpOnly flags,
// expiry and a fixed capacity, but it cannot know which registrable domains
// a suffix list would contain. The device has no PSL data, and shipping a
// stale copy inside the app would be a different lie.

pub const MAX_COOKIES: usize = 24;
const COOKIE_NAME_MAX: usize = 64;
const COOKIE_VALUE_MAX: usize = 256;
const COOKIE_DOMAIN_MAX: usize = 128;
const COOKIE_PATH_MAX: usize = 128;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SameSitePolicy {
    None,
    Lax,
    Strict,
}

pub fn is_public_suffix(domain: &[u8]) -> bool {
    let lower = lower_host(domain);
    if !lower.contains(&b'.') {
        return true;
    }
    matches!(
        lower,
        b"co.uk" | b"org.uk" | b"gov.uk" | b"ac.uk"
            | b"com.au" | b"net.au" | b"org.au"
            | b"co.jp" | b"ne.jp"
            | b"com.br"
    )
}

#[derive(Clone, Copy)]
pub struct Cookie {
    name: [u8; COOKIE_NAME_MAX],
    name_len: usize,
    value: [u8; COOKIE_VALUE_MAX],
    value_len: usize,
    domain: [u8; COOKIE_DOMAIN_MAX],
    domain_len: usize,
    path: [u8; COOKIE_PATH_MAX],
    path_len: usize,
    host_only: bool,
    secure: bool,
    http_only: bool,
    same_site: SameSitePolicy,
    /// Unix seconds; `u64::MAX` means a session cookie.
    expires: u64,
    created: u64,
}

impl Cookie {
    const EMPTY: Self = Self {
        name: [0; COOKIE_NAME_MAX],
        name_len: 0,
        value: [0; COOKIE_VALUE_MAX],
        value_len: 0,
        domain: [0; COOKIE_DOMAIN_MAX],
        domain_len: 0,
        path: [0; COOKIE_PATH_MAX],
        path_len: 0,
        host_only: true,
        secure: false,
        http_only: false,
        same_site: SameSitePolicy::Lax,
        expires: u64::MAX,
        created: 0,
    };

    pub fn name(&self) -> &[u8] {
        &self.name[..self.name_len]
    }
    pub fn value(&self) -> &[u8] {
        &self.value[..self.value_len]
    }
    pub fn domain(&self) -> &[u8] {
        &self.domain[..self.domain_len]
    }
    pub fn path(&self) -> &[u8] {
        &self.path[..self.path_len]
    }
    pub fn is_secure(&self) -> bool {
        self.secure
    }
    pub fn is_http_only(&self) -> bool {
        self.http_only
    }
    pub fn same_site(&self) -> SameSitePolicy {
        self.same_site
    }
    pub fn expires_at(&self) -> u64 {
        self.expires
    }
}

/// The result of processing one `Set-Cookie` field.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CookieOutcome {
    Stored,
    Deleted,
    Rejected,
}

pub struct CookieJar {
    cookies: [Cookie; MAX_COOKIES],
    count: usize,
    next_created: u64,
}

impl CookieJar {
    pub const fn new() -> Self {
        Self {
            cookies: [Cookie::EMPTY; MAX_COOKIES],
            count: 0,
            next_created: 1,
        }
    }

    pub fn len(&self) -> usize {
        self.count
    }

    pub fn is_empty(&self) -> bool {
        self.count == 0
    }

    pub fn clear(&mut self) {
        self.cookies = [Cookie::EMPTY; MAX_COOKIES];
        self.count = 0;
    }

    pub fn cookie(&self, index: usize) -> Option<&Cookie> {
        if index < self.count {
            Some(&self.cookies[index])
        } else {
            None
        }
    }

    fn remove(&mut self, index: usize) {
        if index >= self.count {
            return;
        }
        for position in index..self.count - 1 {
            self.cookies[position] = self.cookies[position + 1];
        }
        self.count -= 1;
    }

    /// Parses and stores one `Set-Cookie` value for `url`.
    ///
    /// `now` is Unix seconds. A device with no wall clock passes zero; in
    /// that case a date-based expiry cannot honestly be evaluated, but
    /// `Max-Age=0` (the universal delete instruction) still takes effect.
    pub fn store(&mut self, url: &[u8], set_cookie: &[u8], now: u64) -> CookieOutcome {
        let parsed = parse_uri(url);
        let host = lower_host(parsed.host);
        if host.is_empty() {
            return CookieOutcome::Rejected;
        }
        let request_path = if parsed.path.is_empty() {
            b"/" as &[u8]
        } else {
            parsed.path
        };

        let (pair, attributes) = match set_cookie.iter().position(|&b| b == b';') {
            Some(position) => (&set_cookie[..position], &set_cookie[position + 1..]),
            None => (set_cookie, &b""[..]),
        };
        let Some(equals) = pair.iter().position(|&b| b == b'=') else {
            return CookieOutcome::Rejected;
        };
        let name = trim_ascii(&pair[..equals]);
        let value = trim_ascii(&pair[equals + 1..]);
        if !is_token(name) || !is_cookie_value(value) {
            return CookieOutcome::Rejected;
        }
        if name.len() > COOKIE_NAME_MAX || value.len() > COOKIE_VALUE_MAX {
            return CookieOutcome::Rejected;
        }

        let mut domain = host;
        let mut host_only = true;
        let mut path: &[u8] = default_cookie_path(request_path);
        let mut secure = false;
        let mut http_only = false;
        let mut same_site = SameSitePolicy::Lax;
        let mut expires = u64::MAX;
        let mut expired_now = false;

        for attribute in attributes.split(|&b| b == b';') {
            let attribute = trim_ascii(attribute);
            if attribute.is_empty() {
                continue;
            }
            let (key, value) = match attribute.iter().position(|&b| b == b'=') {
                Some(position) => (
                    trim_ascii(&attribute[..position]),
                    trim_ascii(&attribute[position + 1..]),
                ),
                None => (attribute, &b""[..]),
            };
            if key.eq_ignore_ascii_case(b"secure") {
                secure = true;
            } else if key.eq_ignore_ascii_case(b"httponly") {
                http_only = true;
            } else if key.eq_ignore_ascii_case(b"samesite") {
                if value.eq_ignore_ascii_case(b"strict") {
                    same_site = SameSitePolicy::Strict;
                } else if value.eq_ignore_ascii_case(b"lax") {
                    same_site = SameSitePolicy::Lax;
                } else {
                    same_site = SameSitePolicy::None;
                }
            } else if key.eq_ignore_ascii_case(b"domain") {
                let mut candidate = value;
                if candidate.starts_with(b".") {
                    candidate = &candidate[1..];
                }
                let candidate = lower_host(candidate);
                if candidate.is_empty() || is_public_suffix(candidate) || !domain_matches(host, candidate) {
                    return CookieOutcome::Rejected;
                }
                domain = candidate;
                host_only = false;
            } else if key.eq_ignore_ascii_case(b"path") {
                if value.starts_with(b"/") && value.len() <= COOKIE_PATH_MAX {
                    path = value;
                }
            } else if key.eq_ignore_ascii_case(b"max-age") {
                if let Some(seconds) = parse_i64(value) {
                    if seconds <= 0 {
                        expires = 0;
                        expired_now = true;
                    } else if now > 0 {
                        expires = now.saturating_add(seconds as u64);
                    }
                }
            } else if key.eq_ignore_ascii_case(b"expires") {
                if let Some(stamp) = parse_http_date(value) {
                    expires = stamp;
                    if now > 0 && stamp <= now {
                        expired_now = true;
                    }
                }
            }
        }

        let existing = self.find(name, domain, path);
        if expired_now || (now > 0 && expires != u64::MAX && expires <= now) {
            if let Some(index) = existing {
                self.remove(index);
            }
            return CookieOutcome::Deleted;
        }

        let mut cookie = Cookie::EMPTY;
        cookie.name[..name.len()].copy_from_slice(name);
        cookie.name_len = name.len();
        cookie.value[..value.len()].copy_from_slice(value);
        cookie.value_len = value.len();
        cookie.domain[..domain.len()].copy_from_slice(domain);
        cookie.domain_len = domain.len();
        cookie.path[..path.len()].copy_from_slice(path);
        cookie.path_len = path.len();
        cookie.host_only = host_only;
        cookie.secure = secure;
        cookie.http_only = http_only;
        cookie.same_site = same_site;
        cookie.expires = expires;
        cookie.created = self.next_created;
        self.next_created = self.next_created.saturating_add(1);

        if let Some(index) = existing {
            self.cookies[index] = cookie;
            return CookieOutcome::Stored;
        }
        if self.count < MAX_COOKIES {
            self.cookies[self.count] = cookie;
            self.count += 1;
            return CookieOutcome::Stored;
        }
        // The jar is bounded. The oldest cookie is evicted, not the new one
        // silently dropped, so a server's newest state survives.
        let mut oldest = 0usize;
        for index in 1..self.count {
            if self.cookies[index].created < self.cookies[oldest].created {
                oldest = index;
            }
        }
        self.cookies[oldest] = cookie;
        CookieOutcome::Stored
    }

    fn find(&self, name: &[u8], domain: &[u8], path: &[u8]) -> Option<usize> {
        for index in 0..self.count {
            let cookie = &self.cookies[index];
            if cookie.name() == name && cookie.domain() == domain && cookie.path() == path {
                return Some(index);
            }
        }
        None
    }

    /// Writes the `Cookie` header value for `url`, longest path first.
    ///
    /// Returns zero when no cookie applies. The caller decides whether to
    /// send the header at all.
    pub fn header_for(&mut self, url: &[u8], now: u64, out: &mut [u8]) -> usize {
        if now > 0 {
            let mut index = 0;
            while index < self.count {
                let expires = self.cookies[index].expires;
                if expires != u64::MAX && expires <= now {
                    self.remove(index);
                } else {
                    index += 1;
                }
            }
        }
        let parsed = parse_uri(url);
        let host = lower_host(parsed.host);
        if host.is_empty() {
            return 0;
        }
        let request_path = if parsed.path.is_empty() {
            b"/" as &[u8]
        } else {
            parsed.path
        };
        let secure_request = parsed.scheme == UriScheme::Https;

        let mut emitted = [false; MAX_COOKIES];
        let mut written = 0usize;
        let mut emitted_any = false;
        for _ in 0..self.count {
            let mut best: Option<usize> = None;
            for index in 0..self.count {
                if emitted[index] {
                    continue;
                }
                let cookie = &self.cookies[index];
                if cookie.host_only {
                    if !cookie.domain().eq_ignore_ascii_case(host) {
                        continue;
                    }
                } else if !domain_matches(host, cookie.domain()) {
                    continue;
                }
                if !path_matches(request_path, cookie.path()) {
                    continue;
                }
                if cookie.secure && !secure_request {
                    continue;
                }
                match best {
                    Some(current) if self.cookies[current].path_len >= cookie.path_len => {}
                    _ => best = Some(index),
                }
            }
            let Some(index) = best else {
                break;
            };
            emitted[index] = true;
            let cookie = &self.cookies[index];
            // The '=' is part of the pair; the separator belongs only to a
            // pair that follows another. Counting both wrong skipped a
            // cookie one byte too late and wrote past the buffer.
            let needed = cookie.name_len
                + cookie.value_len
                + 1
                + if emitted_any { 2 } else { 0 };
            if written + needed > out.len() {
                continue;
            }
            if emitted_any {
                out[written] = b';';
                out[written + 1] = b' ';
                written += 2;
            }
            out[written..written + cookie.name_len].copy_from_slice(cookie.name());
            written += cookie.name_len;
            out[written] = b'=';
            written += 1;
            out[written..written + cookie.value_len].copy_from_slice(cookie.value());
            written += cookie.value_len;
            emitted_any = true;
        }
        written
    }
}

fn is_token(name: &[u8]) -> bool {
    !name.is_empty()
        && name.iter().all(|&byte| {
            byte.is_ascii_alphanumeric()
                || matches!(
                    byte,
                    b'!' | b'#'
                        | b'$'
                        | b'%'
                        | b'&'
                        | b'\''
                        | b'*'
                        | b'+'
                        | b'-'
                        | b'.'
                        | b'^'
                        | b'_'
                        | b'`'
                        | b'|'
                        | b'~'
                )
        })
}

fn is_cookie_value(value: &[u8]) -> bool {
    value.iter().all(|&byte| {
        byte == 0x21
            || (0x23..=0x2B).contains(&byte)
            || (0x2D..=0x3A).contains(&byte)
            || (0x3C..=0x5B).contains(&byte)
            || (0x5D..=0x7E).contains(&byte)
    })
}

fn lower_host(raw: &[u8]) -> &[u8] {
    // The caller compares and stores a lower-case host. Trimming happens in
    // place at the call sites; the jar only needs case-insensitive compares
    // for the wire host, so this returns the original slice.
    raw
}

/// RFC 6265 5.1.3 domain matching. The stored domain is compared
/// case-insensitively because host names are.
pub fn domain_matches(host: &[u8], domain: &[u8]) -> bool {
    if host.eq_ignore_ascii_case(domain) {
        return true;
    }
    host.len() > domain.len()
        && host[host.len() - domain.len() - 1] == b'.'
        && host[host.len() - domain.len()..].eq_ignore_ascii_case(domain)
}

/// RFC 6265 5.1.4 path matching.
pub fn path_matches(request_path: &[u8], cookie_path: &[u8]) -> bool {
    if request_path == cookie_path {
        return true;
    }
    if !request_path.starts_with(cookie_path) {
        return false;
    }
    cookie_path.ends_with(b"/") || request_path.get(cookie_path.len()) == Some(&b'/')
}

fn default_cookie_path(request_path: &[u8]) -> &[u8] {
    if !request_path.starts_with(b"/") {
        return b"/";
    }
    match request_path.iter().rposition(|&b| b == b'/') {
        Some(0) | None => b"/",
        Some(position) => &request_path[..position],
    }
}

fn parse_i64(raw: &[u8]) -> Option<i64> {
    let raw = trim_ascii(raw);
    if raw.is_empty() {
        return None;
    }
    let (negative, digits) = match raw[0] {
        b'-' => (true, &raw[1..]),
        b'+' => (false, &raw[1..]),
        _ => (false, raw),
    };
    if digits.is_empty() {
        return None;
    }
    let mut value = 0i64;
    for &byte in digits {
        if !byte.is_ascii_digit() {
            return None;
        }
        value = value
            .checked_mul(10)?
            .checked_add((byte - b'0') as i64)?;
    }
    Some(if negative { -value } else { value })
}

fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = year - if month <= 2 { 1 } else { 0 };
    let era = if year >= 0 { year } else { year - 399 } / 400;
    let year_of_era = year - era * 400;
    let adjusted_month = month + if month > 2 { -3 } else { 9 };
    let day_of_year = (153 * adjusted_month + 2) / 5 + day - 1;
    let day_of_era =
        year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

/// Parses the IMF-fixdate form (`Sun, 06 Nov 1994 08:49:37 GMT`).
///
/// The older RFC 850 and asctime forms are not accepted. A cookie whose
/// expiry the browser cannot read is kept as a session cookie rather than
/// deleted or given an invented date.
pub fn parse_http_date(raw: &[u8]) -> Option<u64> {
    let mut fields = [&b""[..]; 5];
    let mut count = 0usize;
    for token in raw.split(|&b| b == b' ') {
        let token = trim_ascii(token);
        if token.is_empty() || token.eq_ignore_ascii_case(b"gmt") {
            continue;
        }
        if count < fields.len() {
            fields[count] = token;
            count += 1;
        }
    }
    // Either "Wdy, DD Mon YYYY HH:MM:SS" or "DD Mon YYYY HH:MM:SS".
    let mut index = 0usize;
    if fields[0].ends_with(b",") || (fields[0].len() == 3 && !fields[0][0].is_ascii_digit()) {
        index = 1;
    }
    if count < index + 4 {
        return None;
    }
    let day = parse_i64(fields[index])?;
    let month = match fields[index + 1] {
        b"Jan" => 1,
        b"Feb" => 2,
        b"Mar" => 3,
        b"Apr" => 4,
        b"May" => 5,
        b"Jun" => 6,
        b"Jul" => 7,
        b"Aug" => 8,
        b"Sep" => 9,
        b"Oct" => 10,
        b"Nov" => 11,
        b"Dec" => 12,
        _ => return None,
    };
    let year = parse_i64(fields[index + 2])?;
    let mut time = fields[index + 3].split(|&b| b == b':');
    let hour = parse_i64(time.next()?)?;
    let minute = parse_i64(time.next()?)?;
    let second = parse_i64(time.next()?)?;
    if !(1..=31).contains(&day)
        || !(0..=23).contains(&hour)
        || !(0..=59).contains(&minute)
        || !(0..=60).contains(&second)
    {
        return None;
    }
    let days = days_from_civil(year, month, day);
    let seconds = days * 86_400 + hour * 3_600 + minute * 60 + second;
    if seconds < 0 {
        None
    } else {
        Some(seconds as u64)
    }
}

// =============================================================================
// Forms
// =============================================================================

fn hex_digit(value: u8) -> u8 {
    if value < 10 {
        b'0' + value
    } else {
        b'A' + (value - 10)
    }
}

fn append(out: &mut [u8], at: &mut usize, bytes: &[u8]) -> bool {
    if *at + bytes.len() > out.len() {
        return false;
    }
    out[*at..*at + bytes.len()].copy_from_slice(bytes);
    *at += bytes.len();
    true
}

/// Percent-encodes one form component into `out`, appending at `at`.
pub fn urlencode_component(raw: &[u8], out: &mut [u8], at: &mut usize) -> bool {
    for &byte in raw {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'*' | b'-' | b'.' | b'_' => {
                if !append(out, at, &[byte]) {
                    return false;
                }
            }
            b' ' => {
                if !append(out, at, b"+") {
                    return false;
                }
            }
            _ => {
                if !append(out, at, &[b'%', hex_digit(byte >> 4), hex_digit(byte & 0x0F)]) {
                    return false;
                }
            }
        }
    }
    true
}

/// Encodes `application/x-www-form-urlencoded` pairs.
pub fn form_urlencode(pairs: &[(&[u8], &[u8])], out: &mut [u8]) -> Option<usize> {
    let mut written = 0usize;
    for (index, (name, value)) in pairs.iter().enumerate() {
        if index > 0 && !append(out, &mut written, b"&") {
            return None;
        }
        if !urlencode_component(name, out, &mut written)
            || !append(out, &mut written, b"=")
            || !urlencode_component(value, out, &mut written)
        {
            return None;
        }
    }
    Some(written)
}

/// Builds a GET URL from an action and form fields.
pub fn form_urlencode_url(action: &[u8], pairs: &[(&[u8], &[u8])], out: &mut [u8]) -> Option<usize> {
    let mut written = 0usize;
    if !append(out, &mut written, action) {
        return None;
    }
    let separator = if action.contains(&b'?') {
        b"&" as &[u8]
    } else {
        b"?" as &[u8]
    };
    if !append(out, &mut written, separator) {
        return None;
    }
    for (index, (name, value)) in pairs.iter().enumerate() {
        if index > 0 && !append(out, &mut written, b"&") {
            return None;
        }
        if !urlencode_component(name, out, &mut written)
            || !append(out, &mut written, b"=")
            || !urlencode_component(value, out, &mut written)
        {
            return None;
        }
    }
    Some(written)
}

/// The boundary every multipart body from this browser uses.
pub const MULTIPART_BOUNDARY: &[u8] = b"----InterOSFormBoundary8d2b";

/// One file part of a multipart form.
pub struct MultipartFile<'a> {
    pub field: &'a [u8],
    pub filename: &'a [u8],
    pub content_type: &'a [u8],
    pub data: &'a [u8],
}

fn safe_part_name(raw: &[u8], fallback: &[u8], out: &mut [u8]) -> Option<usize> {
    let mut written = 0usize;
    for &byte in raw {
        let byte = if byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'_') {
            byte
        } else if byte == b' ' {
            b'_'
        } else {
            continue;
        };
        if written >= out.len() {
            return None;
        }
        out[written] = byte;
        written += 1;
    }
    if written == 0 {
        if fallback.len() > out.len() {
            return None;
        }
        out[..fallback.len()].copy_from_slice(fallback);
        written = fallback.len();
    }
    Some(written)
}

fn file_basename(raw: &[u8]) -> &[u8] {
    match raw.iter().rposition(|&b| b == b'/' || b == b'\\') {
        Some(position) => &raw[position + 1..],
        None => raw,
    }
}

/// Encodes a bounded `multipart/form-data` body.
///
/// Names and file names are reduced to a safe header alphabet and a file
/// name is reduced to its last path component, so a form field cannot
/// inject a new header into the part.
pub fn multipart_body(
    fields: &[(&[u8], &[u8])],
    files: &[MultipartFile<'_>],
    out: &mut [u8],
) -> Option<usize> {
    let mut written = 0usize;
    let mut name = [0u8; 96];
    let mut file_name = [0u8; 128];
    let mut mime = [0u8; 96];
    for (field, value) in fields {
        let name_len = safe_part_name(field, b"field", &mut name)?;
        if !append(out, &mut written, b"--")
            || !append(out, &mut written, MULTIPART_BOUNDARY)
            || !append(out, &mut written, b"\r\nContent-Disposition: form-data; name=\"")
            || !append(out, &mut written, &name[..name_len])
            || !append(out, &mut written, b"\"\r\n\r\n")
            || !append(out, &mut written, value)
            || !append(out, &mut written, b"\r\n")
        {
            return None;
        }
    }
    for file in files {
        let name_len = safe_part_name(file.field, b"file", &mut name)?;
        let filename_len = safe_part_name(file_basename(file.filename), b"upload.bin", &mut file_name)?;
        let mime_len = safe_part_name(file.content_type, b"application/octet-stream", &mut mime)?;
        if !append(out, &mut written, b"--")
            || !append(out, &mut written, MULTIPART_BOUNDARY)
            || !append(out, &mut written, b"\r\nContent-Disposition: form-data; name=\"")
            || !append(out, &mut written, &name[..name_len])
            || !append(out, &mut written, b"\"; filename=\"")
            || !append(out, &mut written, &file_name[..filename_len])
            || !append(out, &mut written, b"\"\r\nContent-Type: ")
            || !append(out, &mut written, &mime[..mime_len])
            || !append(out, &mut written, b"\r\n\r\n")
            || !append(out, &mut written, file.data)
            || !append(out, &mut written, b"\r\n")
        {
            return None;
        }
    }
    if !append(out, &mut written, b"--")
        || !append(out, &mut written, MULTIPART_BOUNDARY)
        || !append(out, &mut written, b"--\r\n")
    {
        return None;
    }
    Some(written)
}

// =============================================================================
// Gateway request formatting
// =============================================================================

fn append_component_encoded(target: &[u8], out: &mut [u8], at: &mut usize) {
    for &byte in target {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
            if *at < out.len() {
                out[*at] = byte;
                *at += 1;
            }
        } else if *at + 3 <= out.len() {
            out[*at] = b'%';
            out[*at + 1] = hex_digit(byte >> 4);
            out[*at + 2] = hex_digit(byte & 0x0F);
            *at += 3;
        }
    }
}

/// Copies a header value, dropping control characters.
fn append_header_safe(out: &mut [u8], at: &mut usize, value: &[u8]) -> bool {
    for &byte in value {
        if byte == b'\r' || byte == b'\n' || byte == 0 {
            continue;
        }
        if *at >= out.len() {
            return false;
        }
        out[*at] = byte;
        *at += 1;
    }
    true
}

/// A request the browser sends to the Studio gateway.
pub struct ProxyRequest<'a> {
    pub method: HttpMethod,
    pub url: &'a [u8],
    pub body: &'a [u8],
    pub content_type: &'a [u8],
    pub cookie: &'a [u8],
    pub if_none_match: &'a [u8],
    pub if_modified_since: &'a [u8],
    pub accept_gzip: bool,
}

impl<'a> ProxyRequest<'a> {
    pub const fn get(url: &'a [u8]) -> Self {
        Self {
            method: HttpMethod::Get,
            url,
            body: b"",
            content_type: b"",
            cookie: b"",
            if_none_match: b"",
            if_modified_since: b"",
            accept_gzip: true,
        }
    }
}

/// Formats a request to `/api/browser/proxy` carrying a method and body.
///
/// The target URL is percent-encoded in the query; cookies and validators
/// travel as ordinary request headers, which the gateway forwards.
pub fn format_proxy_request(request: &ProxyRequest<'_>, out: &mut [u8]) -> usize {
    let mut written = 0usize;
    if !append(out, &mut written, request.method.as_str())
        || !append(out, &mut written, b" /api/browser/proxy?url=")
    {
        return written;
    }
    append_component_encoded(request.url, out, &mut written);
    if !append(
        out,
        &mut written,
        b" HTTP/1.1\r\nHost: 127.0.0.1:4300\r\nUser-Agent: InterOS-Browser/1.0 (Mobile; Sovereign)\r\nAccept: text/html,text/gemini,text/plain,*/*\r\n",
    ) {
        return written;
    }
    if request.accept_gzip && !append(out, &mut written, b"Accept-Encoding: gzip, deflate\r\n") {
        return written;
    }
    if !request.cookie.is_empty() {
        if !append(out, &mut written, b"Cookie: ")
            || !append_header_safe(out, &mut written, request.cookie)
            || !append(out, &mut written, b"\r\n")
        {
            return written;
        }
    }
    if !request.if_none_match.is_empty() {
        if !append(out, &mut written, b"If-None-Match: ")
            || !append_header_safe(out, &mut written, request.if_none_match)
            || !append(out, &mut written, b"\r\n")
        {
            return written;
        }
    }
    if !request.if_modified_since.is_empty() {
        if !append(out, &mut written, b"If-Modified-Since: ")
            || !append_header_safe(out, &mut written, request.if_modified_since)
            || !append(out, &mut written, b"\r\n")
        {
            return written;
        }
    }
    if request.method == HttpMethod::Post {
        if !request.content_type.is_empty() {
            if !append(out, &mut written, b"Content-Type: ")
                || !append_header_safe(out, &mut written, request.content_type)
                || !append(out, &mut written, b"\r\n")
            {
                return written;
            }
        }
        if !append(out, &mut written, b"Content-Length: ") {
            return written;
        }
        let mut digits = [0u8; 20];
        let mut value = request.body.len();
        let mut count = 0usize;
        loop {
            digits[count] = b'0' + (value % 10) as u8;
            count += 1;
            value /= 10;
            if value == 0 {
                break;
            }
        }
        while count > 0 {
            count -= 1;
            if written >= out.len() {
                return written;
            }
            out[written] = digits[count];
            written += 1;
        }
        if !append(out, &mut written, b"\r\n") {
            return written;
        }
    }
    if !append(out, &mut written, b"Connection: close\r\n\r\n") {
        return written;
    }
    if request.method == HttpMethod::Post && !request.body.is_empty() {
        append(out, &mut written, request.body);
    }
    written
}

/// Reads a string field from the gateway's JSON envelope.
///
/// The gateway only emits ASCII header values in these fields. The returned
/// slice is the raw text between the quotes; callers that need JSON unescape
/// (`html`) use the reader's existing extractor.
pub fn proxy_string_field<'a>(json: &'a [u8], key: &[u8]) -> Option<&'a [u8]> {
    if key.len() > 40 {
        return None;
    }
    let mut pattern = [0u8; 48];
    pattern[0] = b'"';
    pattern[1..1 + key.len()].copy_from_slice(key);
    pattern[1 + key.len()] = b'"';
    let pattern_len = key.len() + 2;
    let position = json.windows(pattern_len).position(|w| w == &pattern[..pattern_len])?;
    let mut cursor = position + pattern_len;
    while cursor < json.len() && json[cursor].is_ascii_whitespace() {
        cursor += 1;
    }
    if cursor >= json.len() || json[cursor] != b':' {
        return None;
    }
    cursor += 1;
    while cursor < json.len() && json[cursor].is_ascii_whitespace() {
        cursor += 1;
    }
    if cursor >= json.len() || json[cursor] != b'"' {
        return None;
    }
    cursor += 1;
    let start = cursor;
    while cursor < json.len() {
        if json[cursor] == b'\\' {
            cursor += 2;
            continue;
        }
        if json[cursor] == b'"' {
            return Some(&json[start..cursor]);
        }
        cursor += 1;
    }
    None
}

fn hex4(input: &[u8], at: usize) -> Option<u32> {
    if at + 4 > input.len() {
        return None;
    }
    let mut value = 0u32;
    for &byte in &input[at..at + 4] {
        let digit = match byte {
            b'0'..=b'9' => byte - b'0',
            b'a'..=b'f' => byte - b'a' + 10,
            b'A'..=b'F' => byte - b'A' + 10,
            _ => return None,
        };
        value = value * 16 + digit as u32;
    }
    Some(value)
}

/// Decodes the JSON escapes in a string returned by [`proxy_string_field`].
///
/// Header values from the gateway are normally plain ASCII. This exists for
/// the one field that can legitimately contain a newline: the gateway joins
/// multiple `Set-Cookie` values with `\n`, which JSON escapes as `\n`.
pub fn unescape_json_string(input: &[u8], out: &mut [u8]) -> Option<usize> {
    let mut read = 0usize;
    let mut written = 0usize;
    while read < input.len() {
        let byte = input[read];
        if byte != b'\\' {
            if written >= out.len() {
                return None;
            }
            out[written] = byte;
            written += 1;
            read += 1;
            continue;
        }
        if read + 1 >= input.len() {
            return None;
        }
        let escaped = input[read + 1];
        read += 2;
        match escaped {
            b'"' | b'\\' | b'/' => {
                if written >= out.len() {
                    return None;
                }
                out[written] = escaped;
                written += 1;
            }
            b'n' => {
                if written >= out.len() {
                    return None;
                }
                out[written] = b'\n';
                written += 1;
            }
            b'r' => {
                if written >= out.len() {
                    return None;
                }
                out[written] = b'\r';
                written += 1;
            }
            b't' => {
                if written >= out.len() {
                    return None;
                }
                out[written] = b'\t';
                written += 1;
            }
            b'b' => {
                if written >= out.len() {
                    return None;
                }
                out[written] = 0x08;
                written += 1;
            }
            b'f' => {
                if written >= out.len() {
                    return None;
                }
                out[written] = 0x0C;
                written += 1;
            }
            b'u' => {
                let high = hex4(input, read)?;
                read += 4;
                let scalar = if (0xD800..=0xDBFF).contains(&high) {
                    if read + 6 > input.len() || input[read] != b'\\' || input[read + 1] != b'u' {
                        return None;
                    }
                    let low = hex4(input, read + 2)?;
                    read += 6;
                    if !(0xDC00..=0xDFFF).contains(&low) {
                        return None;
                    }
                    0x10000 + (((high - 0xD800) << 10) | (low - 0xDC00))
                } else if (0xDC00..=0xDFFF).contains(&high) {
                    return None;
                } else {
                    high
                };
                let length = crate::utf8_len(scalar);
                if length == 0 || written + length > out.len() {
                    return None;
                }
                written = crate::write_utf8(out, written, scalar);
            }
            _ => return None,
        }
    }
    Some(written)
}

/// Reads the upstream status the gateway recorded in its envelope.
pub fn proxy_status(json: &[u8]) -> Option<u16> {
    crate::json_u64_field(json, b"status").map(|value| value.min(999) as u16)
}

/// Decodes standard base64 with padding, ignoring ASCII whitespace.
pub fn base64_decode(input: &[u8], out: &mut [u8]) -> Option<usize> {
    let mut written = 0usize;
    let mut accumulator = 0u32;
    let mut bits = 0u8;
    let mut padding = 0u8;
    for &byte in input {
        if byte.is_ascii_whitespace() {
            continue;
        }
        if byte == b'=' {
            padding = padding.saturating_add(1);
            if padding > 2 {
                return None;
            }
            continue;
        }
        if padding > 0 {
            return None;
        }
        let value = match byte {
            b'A'..=b'Z' => byte - b'A',
            b'a'..=b'z' => byte - b'a' + 26,
            b'0'..=b'9' => byte - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            _ => return None,
        };
        accumulator = (accumulator << 6) | value as u32;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            let decoded = (accumulator >> bits) as u8;
            if written >= out.len() {
                return None;
            }
            out[written] = decoded;
            written += 1;
            accumulator &= (1 << bits) - 1;
        }
    }
    // Leftover bits must be zero padding, not a partial fourth symbol.
    if bits >= 6 || (accumulator != 0 && bits > 0) {
        return None;
    }
    Some(written)
}

// =============================================================================
// Charset conversion
// =============================================================================

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Charset {
    Utf8,
    Latin1,
    Windows1252,
}

/// Reads the `charset` parameter from a Content-Type value.
///
/// UTF-8 is the default for an absent or unknown label. US-ASCII is a
/// subset of UTF-8, so it needs no separate decoder.
pub fn charset_from_content_type(content_type: &[u8]) -> Charset {
    let mut position = 0usize;
    while position + 8 <= content_type.len() {
        if content_type[position..position + 8].eq_ignore_ascii_case(b"charset=") {
            let mut value = &content_type[position + 8..];
            if value.starts_with(b"\"") {
                value = &value[1..];
                if let Some(end) = value.iter().position(|&b| b == b'"') {
                    value = &value[..end];
                }
            } else if let Some(end) = value.iter().position(|&b| b == b';') {
                value = &value[..end];
            }
            let value = trim_ascii(value);
            if value.eq_ignore_ascii_case(b"iso-8859-1")
                || value.eq_ignore_ascii_case(b"latin1")
                || value.eq_ignore_ascii_case(b"latin-1")
            {
                return Charset::Latin1;
            }
            if value.eq_ignore_ascii_case(b"windows-1252")
                || value.eq_ignore_ascii_case(b"cp1252")
            {
                return Charset::Windows1252;
            }
            return Charset::Utf8;
        }
        position += 1;
    }
    Charset::Utf8
}

const WINDOWS_1252_HIGH: [u16; 32] = [
    0x20AC, 0xFFFD, 0x201A, 0x0192, 0x201E, 0x2026, 0x2020, 0x2021, 0x02C6, 0x2030,
    0x0160, 0x2039, 0x0152, 0xFFFD, 0x017D, 0xFFFD, 0xFFFD, 0x2018, 0x2019, 0x201C,
    0x201D, 0x2022, 0x2013, 0x2014, 0x02DC, 0x2122, 0x0161, 0x203A, 0x0153, 0xFFFD,
    0x017E, 0x0178,
];

/// The result of converting a body to UTF-8.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DecodedText {
    pub written: usize,
    /// Code points that were not valid in the declared charset. They were
    /// replaced with U+FFFD, never silently dropped.
    pub invalid: usize,
    /// The output buffer filled before the input was consumed.
    pub truncated: bool,
}

fn push_utf8(out: &mut [u8], written: &mut usize, code: u32) -> bool {
    let length = crate::utf8_len(code);
    if length == 0 || *written + length > out.len() {
        return false;
    }
    *written = crate::write_utf8(out, *written, code);
    true
}

/// Converts a body into UTF-8 for the reader pipeline.
///
/// UTF-8 input is validated; a bad sequence becomes U+FFFD and increments
/// `invalid`. Latin-1 and Windows-1252 bytes are mapped to their code
/// points, and the five undefined Windows-1252 bytes also become U+FFFD.
/// The platform font is ASCII-only, and the reader transliterates the
/// resulting UTF-8 into that drawable path.
pub fn decode_text(input: &[u8], charset: Charset, out: &mut [u8]) -> DecodedText {
    let mut result = DecodedText {
        written: 0,
        invalid: 0,
        truncated: false,
    };
    let mut index = 0usize;
    while index < input.len() {
        match charset {
            Charset::Utf8 => {
                let byte = input[index];
                if byte < 0x80 {
                    if result.written >= out.len() {
                        result.truncated = true;
                        break;
                    }
                    out[result.written] = byte;
                    result.written += 1;
                    index += 1;
                    continue;
                }
                let (code, length) = decode_utf8_sequence(&input[index..]);
                match code {
                    Some(code) => {
                        if !push_utf8(out, &mut result.written, code) {
                            result.truncated = true;
                            break;
                        }
                        index += length;
                    }
                    None => {
                        if !push_utf8(out, &mut result.written, 0xFFFD) {
                            result.truncated = true;
                            break;
                        }
                        result.invalid += 1;
                        index += 1;
                    }
                }
            }
            Charset::Latin1 => {
                let byte = input[index];
                if !push_utf8(out, &mut result.written, byte as u32) {
                    result.truncated = true;
                    break;
                }
                index += 1;
            }
            Charset::Windows1252 => {
                let byte = input[index];
                let code = if (0x80..=0x9F).contains(&byte) {
                    let mapped = WINDOWS_1252_HIGH[(byte - 0x80) as usize];
                    if mapped == 0xFFFD {
                        result.invalid += 1;
                    }
                    mapped as u32
                } else {
                    byte as u32
                };
                if !push_utf8(out, &mut result.written, code) {
                    result.truncated = true;
                    break;
                }
                index += 1;
            }
        }
    }
    result
}

fn decode_utf8_sequence(input: &[u8]) -> (Option<u32>, usize) {
    let first = input[0];
    let (length, mut code) = match first {
        0xC2..=0xDF => (2usize, (first & 0x1F) as u32),
        0xE0..=0xEF => (3usize, (first & 0x0F) as u32),
        0xF0..=0xF4 => (4usize, (first & 0x07) as u32),
        _ => return (None, 1),
    };
    if input.len() < length {
        return (None, 1);
    }
    for &byte in &input[1..length] {
        if byte & 0xC0 != 0x80 {
            return (None, 1);
        }
        code = (code << 6) | (byte & 0x3F) as u32;
    }
    let minimum = match length {
        2 => 0x80,
        3 => 0x800,
        _ => 0x10000,
    };
    if code < minimum || code > 0x10FFFF || (0xD800..=0xDFFF).contains(&code) {
        return (None, 1);
    }
    (Some(code), length)
}

// =============================================================================
// Conditional requests and a small cache
// =============================================================================

pub const MAX_CACHE_ENTRIES: usize = 2;
pub const CACHE_BODY_BYTES: usize = 12 * 1024;
const CACHE_URL_BYTES: usize = 192;
const CACHE_ETAG_BYTES: usize = 96;
const CACHE_DATE_BYTES: usize = 40;

#[derive(Clone, Copy)]
pub struct CacheEntry {
    url: [u8; CACHE_URL_BYTES],
    url_len: usize,
    etag: [u8; CACHE_ETAG_BYTES],
    etag_len: usize,
    last_modified: [u8; CACHE_DATE_BYTES],
    last_modified_len: usize,
    body: [u8; CACHE_BODY_BYTES],
    body_len: usize,
    valid: bool,
    used: u64,
}

impl CacheEntry {
    const EMPTY: Self = Self {
        url: [0; CACHE_URL_BYTES],
        url_len: 0,
        etag: [0; CACHE_ETAG_BYTES],
        etag_len: 0,
        last_modified: [0; CACHE_DATE_BYTES],
        last_modified_len: 0,
        body: [0; CACHE_BODY_BYTES],
        body_len: 0,
        valid: false,
        used: 0,
    };

    pub fn url(&self) -> &[u8] {
        &self.url[..self.url_len]
    }
    pub fn etag(&self) -> &[u8] {
        &self.etag[..self.etag_len]
    }
    pub fn last_modified(&self) -> &[u8] {
        &self.last_modified[..self.last_modified_len]
    }
    pub fn body(&self) -> &[u8] {
        &self.body[..self.body_len]
    }
}

/// A two-entry cache. Every hit is revalidated with a conditional request;
/// there is no heuristic freshness because the browser has no wall clock
/// and should not pretend a page is fresh.
pub struct HttpCache {
    entries: [CacheEntry; MAX_CACHE_ENTRIES],
    clock: u64,
}

impl HttpCache {
    pub const fn new() -> Self {
        Self {
            entries: [CacheEntry::EMPTY; MAX_CACHE_ENTRIES],
            clock: 0,
        }
    }

    pub fn lookup(&self, url: &[u8]) -> Option<&CacheEntry> {
        self.entries
            .iter()
            .find(|entry| entry.valid && entry.url() == url)
    }

    /// Returns the validator headers to send for `url`, or zero lengths.
    pub fn conditional_headers<'a>(
        &self,
        url: &[u8],
        etag: &'a mut [u8],
        last_modified: &'a mut [u8],
    ) -> (usize, usize) {
        let Some(entry) = self.lookup(url) else {
            return (0, 0);
        };
        let etag_len = entry.etag().len().min(etag.len());
        etag[..etag_len].copy_from_slice(&entry.etag()[..etag_len]);
        let modified_len = entry.last_modified().len().min(last_modified.len());
        last_modified[..modified_len].copy_from_slice(&entry.last_modified()[..modified_len]);
        (etag_len, modified_len)
    }

    /// Stores a body unless the response forbade it.
    ///
    /// `Cache-Control: no-store` removes any older entry. A body larger
    /// than the cache slot is not stored; the browser keeps the page in
    /// memory for the current navigation but does not claim a cache hit
    /// later.
    pub fn store(
        &mut self,
        url: &[u8],
        etag: &[u8],
        last_modified: &[u8],
        cache_control: &[u8],
        body: &[u8],
    ) -> bool {
        if contains_token(cache_control, b"no-store") {
            self.invalidate(url);
            return false;
        }
        if url.len() > CACHE_URL_BYTES || body.len() > CACHE_BODY_BYTES {
            return false;
        }
        self.clock = self.clock.saturating_add(1);
        let slot = self
            .entries
            .iter()
            .position(|entry| !entry.valid || entry.url() == url)
            .unwrap_or_else(|| {
                let mut oldest = 0usize;
                for index in 1..MAX_CACHE_ENTRIES {
                    if self.entries[index].used < self.entries[oldest].used {
                        oldest = index;
                    }
                }
                oldest
            });
        let entry = &mut self.entries[slot];
        *entry = CacheEntry::EMPTY;
        entry.url[..url.len()].copy_from_slice(url);
        entry.url_len = url.len();
        let etag_len = etag.len().min(CACHE_ETAG_BYTES);
        entry.etag[..etag_len].copy_from_slice(&etag[..etag_len]);
        entry.etag_len = etag_len;
        let modified_len = last_modified.len().min(CACHE_DATE_BYTES);
        entry.last_modified[..modified_len].copy_from_slice(&last_modified[..modified_len]);
        entry.last_modified_len = modified_len;
        entry.body[..body.len()].copy_from_slice(body);
        entry.body_len = body.len();
        entry.valid = true;
        entry.used = self.clock;
        true
    }

    pub fn invalidate(&mut self, url: &[u8]) {
        for entry in &mut self.entries {
            if entry.valid && entry.url() == url {
                *entry = CacheEntry::EMPTY;
            }
        }
    }

    pub fn clear(&mut self) {
        self.entries = [CacheEntry::EMPTY; MAX_CACHE_ENTRIES];
    }
}

fn contains_token(value: &[u8], token: &[u8]) -> bool {
    for part in value.split(|&b| b == b',' || b == b';') {
        let part = trim_ascii(part);
        let directive = match part.iter().position(|&b| b == b'=') {
            Some(position) => trim_ascii(&part[..position]),
            None => part,
        };
        if directive.eq_ignore_ascii_case(token) {
            return true;
        }
    }
    false
}

/// True when a response may be kept by the cache.
pub fn cache_control_allows_store(value: &[u8]) -> bool {
    !contains_token(value, b"no-store")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redirect_statuses_are_recognised() {
        for status in [301u16, 302, 303, 307, 308] {
            assert!(status_is_redirect(status));
        }
        for status in [200u16, 204, 304, 400, 500] {
            assert!(!status_is_redirect(status));
        }
    }

    #[test]
    fn a_redirect_method_follows_the_status() {
        assert_eq!(method_for_redirect(303, HttpMethod::Post), HttpMethod::Get);
        assert_eq!(method_for_redirect(307, HttpMethod::Post), HttpMethod::Post);
        assert_eq!(method_for_redirect(308, HttpMethod::Post), HttpMethod::Post);
        assert_eq!(method_for_redirect(301, HttpMethod::Post), HttpMethod::Get);
        assert_eq!(method_for_redirect(302, HttpMethod::Post), HttpMethod::Get);
        assert_eq!(method_for_redirect(302, HttpMethod::Get), HttpMethod::Get);
    }

    #[test]
    fn a_relative_location_resolves_against_its_base() {
        let mut out = [0u8; 256];
        let n = resolve_url(b"https://example.com/a/b/page.html", b"../c?x=1", &mut out).unwrap();
        assert_eq!(&out[..n], b"https://example.com/a/c?x=1");

        let n = resolve_url(b"https://example.com/a/b/page.html", b"/root", &mut out).unwrap();
        assert_eq!(&out[..n], b"https://example.com/root");

        let n = resolve_url(b"https://example.com/a/b/page.html", b"?q=2", &mut out).unwrap();
        assert_eq!(&out[..n], b"https://example.com/a/b/page.html?q=2");

        let n = resolve_url(b"https://example.com/a/b/page.html", b"child", &mut out).unwrap();
        assert_eq!(&out[..n], b"https://example.com/a/b/child");

        let n = resolve_url(b"https://example.com", b"//other.test/x", &mut out).unwrap();
        assert_eq!(&out[..n], b"https://other.test/x");

        let n = resolve_url(b"https://example.com/a", b"https://absolute.test/z/../q", &mut out)
            .unwrap();
        assert_eq!(&out[..n], b"https://absolute.test/q");
    }

    #[test]
    fn dot_segments_are_removed_and_trailing_slashes_kept() {
        let mut out = [0u8; 128];
        let n = resolve_url(b"http://h/a/b/c", b"./../../d/", &mut out).unwrap();
        assert_eq!(&out[..n], b"http://h/d/");
        let n = resolve_url(b"http://h/a/b/", b"..", &mut out).unwrap();
        assert_eq!(&out[..n], b"http://h/a/");
    }

    #[test]
    fn the_tracker_detects_a_loop_and_the_limit() {
        let mut tracker = RedirectTracker::new();
        tracker.start(b"http://a/1");
        assert_eq!(tracker.next(b"http://a/1"), Err(RedirectFault::Loop));
        for index in 0..MAX_REDIRECTS {
            let mut url = [0u8; 32];
            let text = b"http://a/hop";
            url[..text.len()].copy_from_slice(text);
            url[text.len()] = b'0' + index;
            assert_eq!(tracker.next(&url[..text.len() + 1]), Ok(()));
        }
        assert_eq!(tracker.next(b"http://a/limit"), Err(RedirectFault::Limit));
    }

    #[test]
    fn the_redirect_error_page_names_the_limit_and_the_address() {
        let mut out = [0u8; 512];
        let n = redirect_error_document(RedirectFault::Limit, b"https://a/b", &mut out);
        let text = core::str::from_utf8(&out[..n]).unwrap();
        assert!(text.contains("Too many redirects"));
        assert!(text.contains("at most 8 redirects"));
        assert!(text.contains("https://a/b"));
        let n = redirect_error_document(RedirectFault::Loop, b"https://a/b", &mut out);
        let text = core::str::from_utf8(&out[..n]).unwrap();
        assert!(text.contains("Redirect loop"));
    }

    #[test]
    fn a_set_cookie_is_stored_and_emitted_for_its_path() {
        let mut jar = CookieJar::new();
        assert_eq!(
            jar.store(
                b"https://example.com/a/b",
                b"sid=abc123; Path=/a; Max-Age=3600; HttpOnly; Secure",
                1_700_000_000
            ),
            CookieOutcome::Stored
        );
        let mut out = [0u8; 128];
        let n = jar.header_for(b"https://example.com/a/c", 1_700_000_100, &mut out);
        assert_eq!(&out[..n], b"sid=abc123");
        let n = jar.header_for(b"https://example.com/other", 1_700_000_100, &mut out);
        assert_eq!(n, 0);
        let n = jar.header_for(b"http://example.com/a/c", 1_700_000_100, &mut out);
        assert_eq!(n, 0, "a Secure cookie must not travel over http");
    }

    #[test]
    fn a_host_only_cookie_is_not_sent_to_a_subdomain() {
        let mut jar = CookieJar::new();
        jar.store(b"https://example.com/", b"a=1; Path=/", 1_700_000_000);
        let mut out = [0u8; 64];
        assert_eq!(jar.header_for(b"https://sub.example.com/", 1_700_000_100, &mut out), 0);
        assert!(jar.header_for(b"https://example.com/x", 1_700_000_100, &mut out) > 0);
    }

    #[test]
    fn a_domain_cookie_is_sent_to_a_subdomain() {
        let mut jar = CookieJar::new();
        assert_eq!(
            jar.store(b"https://example.com/", b"a=1; Domain=.example.com; Path=/", 1_700_000_000),
            CookieOutcome::Stored
        );
        let mut out = [0u8; 64];
        let n = jar.header_for(b"https://deep.sub.example.com/", 1_700_000_100, &mut out);
        assert_eq!(&out[..n], b"a=1");
    }

    #[test]
    fn a_cookie_for_an_unrelated_domain_is_rejected() {
        let mut jar = CookieJar::new();
        assert_eq!(
            jar.store(b"https://example.com/", b"a=1; Domain=evil.test", 1_700_000_000),
            CookieOutcome::Rejected
        );
    }

    #[test]
    fn max_age_zero_and_expiry_delete_a_cookie() {
        let mut jar = CookieJar::new();
        jar.store(b"https://e.test/", b"a=1; Path=/", 1_700_000_000);
        assert_eq!(
            jar.store(b"https://e.test/", b"a=2; Path=/; Max-Age=0", 1_700_000_100),
            CookieOutcome::Deleted
        );
        assert_eq!(jar.len(), 0);
        jar.store(b"https://e.test/", b"b=1; Path=/; Expires=Wed, 21 Oct 2015 07:28:00 GMT", 1_700_000_000);
        assert_eq!(
            jar.store(b"https://e.test/", b"b=2; Path=/; Expires=Wed, 21 Oct 2015 07:28:00 GMT", 1_700_000_100),
            CookieOutcome::Deleted
        );
    }

    #[test]
    fn http_dates_parse_to_unix_seconds() {
        assert_eq!(parse_http_date(b"Thu, 01 Jan 1970 00:00:00 GMT"), Some(0));
        assert_eq!(
            parse_http_date(b"Wed, 21 Oct 2015 07:28:00 GMT"),
            Some(1_445_412_480)
        );
        assert_eq!(parse_http_date(b"not a date"), None);
    }

    #[test]
    fn the_jar_overwrites_the_same_identity_and_evicts_the_oldest() {
        let mut jar = CookieJar::new();
        jar.store(b"https://e.test/", b"a=1; Path=/", 1);
        jar.store(b"https://e.test/", b"a=2; Path=/", 1);
        assert_eq!(jar.len(), 1);
        let mut out = [0u8; 64];
        let n = jar.header_for(b"https://e.test/", 0, &mut out);
        assert_eq!(&out[..n], b"a=2");
        for index in 0..MAX_COOKIES {
            let mut name = [b'c'; 8];
            name[1] = b'0' + (index % 10) as u8;
            name[2] = b'0' + (index / 10) as u8;
            let mut field = [0u8; 32];
            field[..8].copy_from_slice(&name);
            field[8..10].copy_from_slice(b"=1");
            jar.store(b"https://e.test/", &field[..10], 1);
        }
        assert!(jar.len() <= MAX_COOKIES);
    }

    #[test]
    fn a_cookie_that_does_not_fit_is_skipped_not_truncated() {
        let mut jar = CookieJar::new();
        assert_eq!(
            jar.store(b"https://e.test/", b"a=1", 0),
            CookieOutcome::Stored
        );
        assert_eq!(
            jar.store(b"https://e.test/", b"b=2", 0),
            CookieOutcome::Stored
        );

        let mut exact = [0u8; 8];
        let n = jar.header_for(b"https://e.test/", 0, &mut exact);
        assert_eq!(&exact[..n], b"a=1; b=2");

        // One byte short: the second pair is left out whole rather than
        // written past the end of the buffer.
        let mut short = [0u8; 7];
        let n = jar.header_for(b"https://e.test/", 0, &mut short);
        assert_eq!(&short[..n], b"a=1");
    }

    #[test]
    fn form_urlencoding_escapes_reserved_bytes() {
        let mut out = [0u8; 128];
        let n = form_urlencode(&[(b"q", b"a b&c"), (b"n", b"\xC3\xA9")], &mut out).unwrap();
        assert_eq!(&out[..n], b"q=a+b%26c&n=%C3%A9");
    }

    #[test]
    fn a_get_form_appends_to_an_existing_query() {
        let mut out = [0u8; 128];
        let n = form_urlencode_url(b"https://e.test/s?x=1", &[(b"q", b"a")], &mut out).unwrap();
        assert_eq!(&out[..n], b"https://e.test/s?x=1&q=a");
    }

    #[test]
    fn a_multipart_body_names_its_parts_and_ends_with_the_boundary() {
        let mut out = [0u8; 1024];
        let file = MultipartFile {
            field: b"upload",
            filename: b"../evil\r\nname.txt",
            content_type: b"text/plain",
            data: b"hello",
        };
        let n = multipart_body(&[(b"title", b"Report")], &[file], &mut out).unwrap();
        let text = core::str::from_utf8(&out[..n]).unwrap();
        assert!(text.contains("name=\"title\"\r\n\r\nReport\r\n"));
        assert!(text.contains("name=\"upload\"; filename=\"evilname.txt\""));
        assert!(!text.contains("\r\nname.txt"));
        assert!(text.ends_with("--\r\n"));
    }

    #[test]
    fn the_gateway_request_carries_method_body_cookie_and_validators() {
        let mut out = [0u8; 1024];
        let request = ProxyRequest {
            method: HttpMethod::Post,
            url: b"https://e.test/f?q=1",
            body: b"a=1",
            content_type: b"application/x-www-form-urlencoded",
            cookie: b"sid=abc",
            if_none_match: b"\"v1\"",
            if_modified_since: b"Wed, 21 Oct 2015 07:28:00 GMT",
            accept_gzip: true,
        };
        let n = format_proxy_request(&request, &mut out);
        let text = core::str::from_utf8(&out[..n]).unwrap();
        assert!(text.starts_with("POST /api/browser/proxy?url=https%3A%2F%2Fe.test%2Ff%3Fq%3D1"));
        assert!(text.contains("Content-Length: 3"));
        assert!(text.contains("Cookie: sid=abc"));
        assert!(text.contains("If-None-Match: \"v1\""));
        assert!(text.contains("Accept-Encoding: gzip, deflate"));
        assert!(text.ends_with("\r\n\r\na=1"));
    }

    #[test]
    fn a_header_value_cannot_end_the_header_block() {
        let mut out = [0u8; 512];
        let request = ProxyRequest {
            method: HttpMethod::Get,
            url: b"https://e.test/",
            body: b"",
            content_type: b"",
            cookie: b"a=1\r\nX-Injected: yes",
            if_none_match: b"",
            if_modified_since: b"",
            accept_gzip: false,
        };
        let n = format_proxy_request(&request, &mut out);
        let text = core::str::from_utf8(&out[..n]).unwrap();
        assert!(text.contains("Cookie: a=1X-Injected: yes\r\n"));
        assert!(!text.contains("\r\nX-Injected"));
    }

    #[test]
    fn the_envelope_fields_are_read_without_a_json_dependency() {
        let json = br#"{"success": true, "status": 302, "location": "https://e.test/next", "set_cookie": "a=1\nb=2", "content_encoding": "gzip", "body_b64": "aGVsbG8=", "html": "hi"}"#;
        assert_eq!(proxy_status(json), Some(302));
        assert_eq!(proxy_string_field(json, b"location"), Some(&b"https://e.test/next"[..]));
        assert_eq!(proxy_string_field(json, b"content_encoding"), Some(&b"gzip"[..]));
        assert_eq!(proxy_string_field(json, b"missing"), None);
        let raw = proxy_string_field(json, b"set_cookie").unwrap();
        let mut cookies = [0u8; 32];
        let len = unescape_json_string(raw, &mut cookies).unwrap();
        assert_eq!(&cookies[..len], b"a=1\nb=2");
    }

    #[test]
    fn base64_decodes_padded_and_ignores_whitespace() {
        let mut out = [0u8; 16];
        assert_eq!(base64_decode(b"aGVsbG8=", &mut out), Some(5));
        assert_eq!(&out[..5], b"hello");
        assert_eq!(base64_decode(b"aGVs\r\nbG8=", &mut out), Some(5));
        assert_eq!(base64_decode(b"aGVsbG8", &mut out), Some(5));
        assert_eq!(base64_decode(b"!!!!", &mut out), None);
        let mut tiny = [0u8; 2];
        assert_eq!(base64_decode(b"aGVsbG8=", &mut tiny), None);
    }

    #[test]
    fn latin1_and_windows_1252_convert_to_utf8() {
        let mut out = [0u8; 32];
        let result = decode_text(b"caf\xE9", Charset::Latin1, &mut out);
        assert_eq!(&out[..result.written], b"caf\xC3\xA9");
        assert_eq!(result.invalid, 0);

        let result = decode_text(b"\x93quoted\x94 \x80", Charset::Windows1252, &mut out);
        assert_eq!(&out[..result.written], b"\xE2\x80\x9Cquoted\xE2\x80\x9D \xE2\x82\xAC");
        let result = decode_text(b"\x81", Charset::Windows1252, &mut out);
        assert_eq!(&out[..result.written], b"\xEF\xBF\xBD");
        assert_eq!(result.invalid, 1);
    }

    #[test]
    fn invalid_utf8_becomes_an_explicit_replacement() {
        let mut out = [0u8; 32];
        let result = decode_text(b"ok\xFF\xC3\x28", Charset::Utf8, &mut out);
        assert_eq!(&out[..result.written], b"ok\xEF\xBF\xBD\xEF\xBF\xBD(");
        assert_eq!(result.invalid, 2);
    }

    #[test]
    fn a_charset_label_is_read_from_the_content_type() {
        assert_eq!(
            charset_from_content_type(b"text/html; charset=iso-8859-1"),
            Charset::Latin1
        );
        assert_eq!(
            charset_from_content_type(b"text/html; charset=\"windows-1252\""),
            Charset::Windows1252
        );
        assert_eq!(
            charset_from_content_type(b"text/html; charset=utf-8"),
            Charset::Utf8
        );
        assert_eq!(charset_from_content_type(b"text/html"), Charset::Utf8);
    }

    #[test]
    fn a_conditional_request_reuses_a_cached_body_and_honours_no_store() {
        let mut cache = HttpCache::new();
        assert!(cache.store(
            b"https://e.test/a",
            b"\"v1\"",
            b"Wed, 21 Oct 2015 07:28:00 GMT",
            b"max-age=60",
            b"body"
        ));
        let mut etag = [0u8; 64];
        let mut date = [0u8; 64];
        let (etag_len, date_len) =
            cache.conditional_headers(b"https://e.test/a", &mut etag, &mut date);
        assert_eq!(&etag[..etag_len], b"\"v1\"");
        assert!(date_len > 0);
        assert_eq!(cache.lookup(b"https://e.test/a").unwrap().body(), b"body");

        assert!(!cache.store(
            b"https://e.test/a",
            b"\"v2\"",
            b"",
            b"no-store",
            b"new"
        ));
        assert!(cache.lookup(b"https://e.test/a").is_none());
    }

    #[test]
    fn the_cache_keeps_a_bounded_number_of_entries() {
        let mut cache = HttpCache::new();
        for index in 0..4u8 {
            let mut url = [0u8; 24];
            url[..15].copy_from_slice(b"https://e.test/");
            url[15] = b'0' + index;
            assert!(cache.store(&url[..16], b"\"x\"", b"", b"", b"body"));
        }
        let live = (0..4u8)
            .filter(|index| {
                let mut url = [0u8; 24];
                url[..15].copy_from_slice(b"https://e.test/");
                url[15] = b'0' + index;
                cache.lookup(&url[..16]).is_some()
            })
            .count();
        assert_eq!(live, MAX_CACHE_ENTRIES);
    }

    /// The exact shape the gateway returns for a compressed, non-UTF-8 page:
    /// a base64 body, a gzip coding and a Latin-1 charset. Decoding it is the
    /// browser's job, and every step is bounded.
    #[test]
    fn a_gateway_envelope_is_walked_to_drawable_text() {
        // gzip('<html><body><h1>Caf\xe9</h1></body></html>')
        const B64: &[u8] =
            b"H4sIAAAAAAAC/7PJKMnNsbNJyk+ptLPJMLRzTkx7aaMPZNjoQ8T0wQoAxEYgnicAAAA=";
        let mut envelope = [0u8; 512];
        let mut length = 0usize;
        for part in [
            b"{\"success\": true, \"url\": \"http://example.test/page\", \"status\": 200, "
                as &[u8],
            b"\"content_type\": \"text/html; charset=iso-8859-1\", ",
            b"\"content_encoding\": \"gzip\", \"body_b64\": \"",
            B64,
            b"\"}",
        ] {
            envelope[length..length + part.len()].copy_from_slice(part);
            length += part.len();
        }
        let envelope = &envelope[..length];

        assert_eq!(proxy_status(envelope), Some(200));
        let mut content_type = [0u8; 64];
        let content_type_len = unescape_json_string(
            proxy_string_field(envelope, b"content_type").unwrap(),
            &mut content_type,
        )
        .unwrap();
        let encoding = proxy_string_field(envelope, b"content_encoding").unwrap();

        let mut wire = [0u8; 128];
        let wire_len = base64_decode(
            proxy_string_field(envelope, b"body_b64").unwrap(),
            &mut wire,
        )
        .unwrap();
        let mut inflated = [0u8; 128];
        let inflated_len =
            crate::inflate::decode_content_encoding(encoding, &wire[..wire_len], &mut inflated)
                .unwrap();
        assert_eq!(
            &inflated[..inflated_len],
            b"<html><body><h1>Caf\xe9</h1></body></html>"
        );

        let mut text = [0u8; 256];
        let decoded = decode_text(
            &inflated[..inflated_len],
            charset_from_content_type(&content_type[..content_type_len]),
            &mut text,
        );
        assert_eq!(decoded.invalid, 0);
        assert_eq!(
            &text[..decoded.written],
            b"<html><body><h1>Caf\xc3\xa9</h1></body></html>"
        );
    }

    #[test]
    fn test_post_body_encoding_and_redirect_method_preservation() {
        let mut out = [0u8; 512];
        let post_body = b"user=alice&action=login";
        let req = ProxyRequest {
            method: HttpMethod::Post,
            url: b"https://api.interos.org/v1/auth",
            body: post_body,
            content_type: b"application/x-www-form-urlencoded",
            cookie: b"session=xyz",
            if_none_match: b"",
            if_modified_since: b"",
            accept_gzip: false,
        };
        let len = format_proxy_request(&req, &mut out);
        let s = core::str::from_utf8(&out[..len]).expect("valid request bytes");
        assert!(s.starts_with("POST /api/browser/proxy?url=https%3A%2F%2Fapi.interos.org%2Fv1%2Fauth"));
        assert!(s.contains("Content-Length: 23\r\n"));
        assert!(s.contains("Content-Type: application/x-www-form-urlencoded\r\n"));
        assert!(s.ends_with("\r\n\r\nuser=alice&action=login"));

        // 303 must switch to GET and drop POST body
        let m303 = method_for_redirect(303, HttpMethod::Post);
        assert_eq!(m303, HttpMethod::Get);

        // 307 must preserve POST method and body
        let m307 = method_for_redirect(307, HttpMethod::Post);
        assert_eq!(m307, HttpMethod::Post);

        // 308 must also preserve POST method and body
        let m308 = method_for_redirect(308, HttpMethod::Post);
        assert_eq!(m308, HttpMethod::Post);
    }

    #[test]
    fn test_cookie_jar_scoping_rules() {
        let mut jar = CookieJar::new();
        // Domain cookie: scoped to .example.com
        assert_eq!(
            jar.store(b"https://api.example.com/", b"global=1; Domain=example.com; Path=/", 1000),
            CookieOutcome::Stored
        );
        // Host-only cookie: scoped strictly to auth.example.com
        assert_eq!(
            jar.store(b"https://auth.example.com/login", b"auth_token=secret; Path=/login; Secure", 1000),
            CookieOutcome::Stored
        );
        // Path-scoped cookie
        assert_eq!(
            jar.store(b"https://example.com/app", b"app_state=active; Path=/app; SameSite=Strict", 1000),
            CookieOutcome::Stored
        );
        // Public suffix cookie must be rejected
        assert_eq!(
            jar.store(b"https://evil.co.uk/", b"bad=1; Domain=co.uk", 1000),
            CookieOutcome::Rejected
        );

        let mut out = [0u8; 256];

        // Root request to example.com should receive global cookie, but NOT path or host-only
        let n = jar.header_for(b"https://example.com/", 1001, &mut out);
        let header = core::str::from_utf8(&out[..n]).unwrap();
        assert!(header.contains("global=1"));
        assert!(!header.contains("auth_token"));
        assert!(!header.contains("app_state"));

        // Request to auth.example.com/login (HTTPS) should receive both global and auth_token
        let n = jar.header_for(b"https://auth.example.com/login", 1001, &mut out);
        let header = core::str::from_utf8(&out[..n]).unwrap();
        assert!(header.contains("global=1"));
        assert!(header.contains("auth_token=secret"));

        // Insecure HTTP request to auth.example.com/login must NOT receive Secure auth_token
        let n = jar.header_for(b"http://auth.example.com/login", 1001, &mut out);
        let header = core::str::from_utf8(&out[..n]).unwrap();
        assert!(!header.contains("auth_token"));

        // Request to other.org must receive NO cookies
        let n = jar.header_for(b"https://other.org/", 1001, &mut out);
        assert_eq!(n, 0);
    }
}
