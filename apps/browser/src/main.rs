//! Inter OS Native Sovereign Browser & Web Client
//!
//! High-performance Ring-3 (User Mode) smartphone browser laid out for the
//! 720x1440 sovereign smartphone viewport:
//! - Full Mobile UI: Omnibox with SSL security padlock, domain highlighting,
//!   tab counter badge, overflow kebab menu, and reload
//! - Start Page / New Tab:
//!   - Sovereign browser emblem
//!   - Quick Search Pill
//!   - "Your pages": saved bookmarks and the saved visit history
//! - Web Viewport / Content Area:
//!   - Classified document lines (H1, H2, H3, bullet lists, code blocks, gemtext links)
//!   - Smooth vertical scrolling with scroll position indicator
//!   - Tapping links navigates directly to target
//! - Tab Switcher Sheet overlay (up to 4 tabs, switch, close, new tab)
//! - Bookmarks & History sheet overlays
//! - Kernel socket integration: calls non-blocking socket syscalls
//!   (`std::net_socket`, `net_connect`, `net_send`, `net_recv`) with truthful error reporting
//! - Strict adherence to Platform Truth & Native UI contracts

#![no_std]
#![no_main]

use core::panic::PanicInfo;
use interos_std as std;
use browser::theme::{MIN_TOUCH_TARGET, THEME, radius, spacing, type_scale};
use interos_std::design::back_button;
use interos_std::design::components;
use interos_std::design::font;
use interos_std::design::primitives::{draw_rounded_rect, Rect};

use browser::{
    format_proxy_request, is_browser_envelope, link_arrow_ink, parse_gemtext_link, parse_ipv4,
    parse_uri, perform_tls_handshake, scheme_mark, url_field, ConnectionFailure, DocLine,
    DocumentTokenizer, FetchErrorKind, LineKind, LoadingState, PageStatus, ProtocolFaultKind,
    TabSessionManager, TcpSocketStream, TransportFailure, UriScheme, MAX_DOC_LINES,
    MAX_LINE_BYTES, MAX_TABS,
};
pub use browser::tls;
use browser::css;
use browser::images;
use browser::layout;
use interos_std::design::outline_text;
use interos_std::design::scroll::ScrollState;

// -----------------------------------------------------------------------------
// Sovereign InterOS Browser Design Tokens
// -----------------------------------------------------------------------------

// -----------------------------------------------------------------------------
// Viewport & Layout Geometry (720x1440, 8pt Grid)
// -----------------------------------------------------------------------------
// -----------------------------------------------------------------------------
// HTTP over TCP
// -----------------------------------------------------------------------------

/// The kernel's "not ready" result code.
///
/// WHY it is named here: a TCP connection is not established the instant
/// `tcp_connect` returns -- the handshake completes asynchronously -- so
/// the transport has to tell "not yet" apart from "failed". The kernel
/// reports that as `SyscallResult::NotReady`.
const NOT_READY: i64 = -30;

/// The app's own marker for "the entropy read was refused, so no connection was
/// opened".
///
/// WHY it is not a code from the transport: the transport was never reached --
/// the sequence number is chosen before `tcp_connect` -- and every other number
/// this field carries came back from a connection. A marker that did not is
/// labelled as the app's own rather than dressed up as a peer's answer.
const ENTROPY_REFUSED: i64 = -1100;

/// The largest HTTP response the browser will hold.
///
/// WHY this is large: the Device Studio gateway serves a real page, and a
/// documentation page is more than 64 KB of HTML. The gateway itself caps a
/// response at 1 MB, so a response that reaches this limit has been
/// truncated by the browser and the screen says so.
const MAX_RESPONSE_BYTES: usize = 256 * 1024;

/// The buffer an HTTP response is accumulated into.
///
/// WHY this is a static and not a local array: it is a quarter of a
/// megabyte, and a local of that size does not belong on a 128 KB task
/// stack.
static mut RESPONSE_BUF: [u8; MAX_RESPONSE_BYTES] = [0; MAX_RESPONSE_BYTES];

/// The buffer the page's HTML is extracted into.
static mut HTML_BUF: [u8; browser::MAX_HTML_BYTES] = [0; browser::MAX_HTML_BYTES];

/// The scratch buffer a decoded body is written into before charset
/// conversion.
///
/// WHY it is separate from `HTML_BUF`: a compressed body has to be inflated
/// before its charset is known, and the inflated bytes and the converted
/// document cannot share one buffer. It is also why the envelope's JSON
/// survives: `RESPONSE_BUF` still holds it while this buffer is written.
static mut DECODE_BUF: [u8; browser::MAX_HTML_BYTES] = [0; browser::MAX_HTML_BYTES];

/// The buffer a linked stylesheet's text is copied out of the gateway's
/// envelope into.
///
/// WHY it is as large as the document's own style arena: the two hold the
/// same kind of rules, and reading a link into anything smaller would cut a
/// stylesheet short before the arena that would have held it ever saw it.
static mut STYLE_BUF: [u8; css::MAX_CSS_BYTES] = [0; css::MAX_CSS_BYTES];

/// The reply bytes one linked stylesheet may ask the wire for.
///
/// WHY not the whole response buffer: a stylesheet is read as far as the
/// document's style window, and the text kept from it is then cut to the rules
/// this engine applies, so anything past that window is paid for in wire time
/// and thrown away where it lands. A page that chains a dozen colour-token
/// files, each twice the size of the window, pays that dozen times over
/// before the file holding its own type is reached.
///
/// WHY sixteen kilobytes above the window: the reply is the stylesheet inside
/// an HTTP head and a JSON envelope, and the envelope escapes quotes and
/// newlines, so the bytes on the wire are more than the text they carry.
const STYLESHEET_WINDOW_BYTES: usize = css::MAX_CSS_BYTES + 16 * 1024;

/// The cookies the browser has been given, across navigations.
///
/// WHY a static and not a field of `BrowserApp`: the jar is bounded but
/// larger than the task stack should carry, and `BrowserApp::new()` is
/// built on that stack. Cookie state outlives a single navigation, which
/// is exactly what the static's lifetime expresses.
static mut COOKIE_JAR: browser::http::CookieJar = browser::http::CookieJar::new();

/// The two-entry revalidating cache, also across navigations.
static mut HTTP_CACHE: browser::http::HttpCache = browser::http::HttpCache::new();

/// The request line and headers for one navigation hop.
static mut REQUEST_BUF: [u8; 2048] = [0; 2048];

/// The `Cookie` header emitted for one hop.
static mut COOKIE_HEADER_BUF: [u8; 1536] = [0; 1536];

/// The address one hop is being fetched for.
static mut HOP_BUF: [u8; browser::http::MAX_URL_BYTES] = [0; browser::http::MAX_URL_BYTES];

/// Copies a string field out of the gateway's envelope.
fn envelope_field(json: &[u8], key: &[u8], out: &mut [u8]) -> usize {
    match browser::http::proxy_string_field(json, key) {
        Some(raw) => match browser::http::unescape_json_string(raw, out) {
            Some(length) => length,
            None => 0,
        },
        None => 0,
    }
}

/// The buffer a local document is read into.
static mut FILE_BUF: [u8; MAX_FILE_BYTES] = [0; MAX_FILE_BYTES];

/// The largest local document the browser will read.
const MAX_FILE_BYTES: usize = 64 * 1024;

/// The payloads the built-in speed test moves in each direction.
///
/// WHY these sizes: the download stays inside the response buffer beside
/// its headers, and the upload is large enough to outlast the TCP
/// handshake and the gateway's first read. Both are the gateway's own
/// fixed payloads, so the byte count is known before the request is sent.
const SPEEDTEST_DOWNLOAD_BYTES: usize = 128 * 1024;
const SPEEDTEST_UPLOAD_BYTES: usize = 64 * 1024;
const SPEEDTEST_LATENCY_BYTES: usize = 1024;

/// The largest slice one request-body send carries.
///
/// WHY this is below the kernel's segment cap: `tcp_send` refuses a payload
/// above one segment, and a body longer than that is sent in steps.
const TCP_SEND_CHUNK: usize = 1200;

/// How long a request body may take to go on the wire.
const TCP_BODY_TIMEOUT_TICKS: u64 = 60_000;

/// How many files the upload picker lists.
const MAX_PICK_FILES: usize = 8;

/// The body the upload test sends.
///
/// WHY a static: the payload is 64 KiB, and a local of that size does not
/// belong on the task stack. It is filled before each run.
static mut UPLOAD_TEST_BUF: [u8; SPEEDTEST_UPLOAD_BYTES] = [0; SPEEDTEST_UPLOAD_BYTES];

/// The buffer the reader document is parsed into.
static mut READER_BUF: [u8; browser::MAX_READER_BYTES] = [0; browser::MAX_READER_BYTES];

/// The decoded pixels of the page's inline images.
///
/// WHY one arena: the reader has no heap. A page may place at most
/// `images::MAX_INLINE_IMAGES` pictures, and the arena bounds their
/// total to two megabytes no matter how many pixels the headers claim.
/// An image that does not fit is refused with a placeholder, never
/// silently dropped and never truncated into a partial bitmap.
static mut IMAGE_ARENA: [u8; images::IMAGE_ARENA_BYTES] = [0; images::IMAGE_ARENA_BYTES];

/// The DOM of the page the HTML view is showing.
///
/// WHY a static and not a field of the app state: the arena is a few
/// hundred kilobytes, and the app state is built on the task's stack.
static mut PAGE_DOM: layout::Dom = layout::Dom::new();

/// The display list the HTML view paints.
static mut PAGE_LAYOUT: layout::TextLayout = layout::TextLayout::new();

/// The longest value one form control's editor holds.
const FIELD_TEXT_MAX: usize = 96;

/// The value each form control would submit.
///
/// WHY these are statics: the display list is static, and the value a
/// person typed into a control has to outlive the focus moving to the
/// next one. A value copied only while its field is focused would be
/// lost the moment another field took the keyboard.
static mut FIELD_TEXT: [[u8; FIELD_TEXT_MAX]; layout::MAX_FIELDS] =
    [[0; FIELD_TEXT_MAX]; layout::MAX_FIELDS];
static mut FIELD_TEXT_LEN: [u8; layout::MAX_FIELDS] = [0; layout::MAX_FIELDS];

/// The encoded body of a form the browser is submitting.
///
/// WHY one buffer: a submission is one request, and the request builder
/// reads the body while it is still the current navigation.
static mut FORM_BODY: [u8; 8192] = [0; 8192];

/// The encoded query of a GET submission, before it becomes an address.
static mut FORM_ENCODED: [u8; 1024] = [0; 1024];

/// The address a form submission is sent to, after resolution.
static mut FORM_URL: [u8; browser::http::MAX_URL_BYTES] = [0; browser::http::MAX_URL_BYTES];

/// No form control has the keyboard.
const NO_FIELD: u8 = u8::MAX;

/// The lines the reader holds for the document on screen.
///
/// WHY a static and not a field of the app state: a thousand lines is a
/// quarter of a megabyte, and the app state is built on the task's stack.
/// The lines are the reader's buffer, and a buffer of that size does not
/// belong on a 128 KB stack.
static mut DOC_LINES: [DocLine; MAX_DOC_LINES] = [DocLine::EMPTY; MAX_DOC_LINES];

/// The reader's line at an index.
///
/// WHY a function and not a field of the app state: the lines live in a
/// static because a thousand of them do not belong on the task's stack,
/// and both the renderer and the tap handler read them.
///
/// WHY the pointer form: the app is single threaded, and taking a
/// reference to the whole static would claim a borrow the compiler
/// cannot check.
static EMPTY_DOC_LINE: DocLine = DocLine::EMPTY;

fn doc_line(index: usize) -> &'static DocLine {
    if index < MAX_DOC_LINES {
        unsafe { &(*core::ptr::addr_of!(DOC_LINES))[index] }
    } else {
        &EMPTY_DOC_LINE
    }
}

/// The nearest `<form>` ancestor of a node.
fn nearest_form(dom: &layout::Dom, node: layout::NodeId) -> Option<layout::NodeId> {
    let mut cursor = dom.parent(node);
    while let Some(id) = cursor {
        if dom.tag(id) == b"form" {
            return Some(id);
        }
        cursor = dom.parent(id);
    }
    None
}

/// The value an option submits: its `value`, or the text it shows.
fn option_value<'a>(dom: &'a layout::Dom, node: layout::NodeId) -> &'a [u8] {
    if let Some(value) = dom.attr(node, b"value") {
        return value;
    }
    let mut child = dom.first_child(node);
    while let Some(id) = child {
        if dom.node(id).is_text() {
            return dom.text_of(id);
        }
        child = dom.next_sibling(id);
    }
    b""
}

/// How many sends are attempted if the tick counter is unavailable.
///
/// WHY a retry loop and not a single send: `tcp_send` refuses with
/// `NotReady` until the connection reaches the established state, and the
/// handshake completes asynchronously -- the SYN-ACK arrives on the
/// network and is processed by the kernel's poll path. The previous
/// implementation issued one datagram and read once, so it read nothing
/// and reported a transport failure for every page.
const TCP_SEND_ATTEMPTS: usize = 20_000;

/// How many receives are attempted if the tick counter is unavailable.
const TCP_RECV_ATTEMPTS: usize = 20_000;

/// How long a fetch waits for the handshake to complete.
///
/// WHY a time budget and not a fixed attempt count: attempts are cheap and
/// the loop can spend them all in a few milliseconds, which is far less than
/// a real round trip through the gateway to a site takes. The device gave up
/// while the response was still in flight and reported zero bytes.
const TCP_HANDSHAKE_TIMEOUT_TICKS: u64 = 5_000;

/// How long a fetch waits for the gateway's response, in total.
///
/// WHY the budget is not one round trip: the gateway fetches the origin on
/// the host and then relays the body over SLIRP, whose receive window
/// advances in 2 KiB steps. A body of a few dozen kilobytes can take longer
/// than the old 20 s budget while bytes are still arriving, so a real
/// download was saved as a partial file.
const TCP_RESPONSE_TIMEOUT_TICKS: u64 = 240_000;

/// How long the reader tolerates silence before the transfer is treated as over.
///
/// WHY a stall budget exists beside the total: a connection that has stopped
/// delivering must not hold the app for the whole four-minute budget, while a
/// slow but healthy transfer must not be cut off.
const TCP_RESPONSE_STALL_TICKS: u64 = 10_000;

/// The largest slice one receive may ask the kernel for.
///
/// WHY this is not the response buffer's length: the receive syscall carries
/// its payload through a fixed 2 KiB kernel buffer and refuses a longer
/// request outright. Asking for the whole 256 KiB response in one call was
/// rejected before the kernel ever looked at the connection, and the browser
/// reported the refusal as an empty transport. Reads advance in chunks.
const TCP_READ_CHUNK: usize = 2048;

/// BSS-allocated static session storage for TLS 1.3 client connections (allocation-free).
static mut TLS_SESSION_STORAGE: Option<tls::TlsClientSession<'static>> = None;

/// BSS-allocated static verifier for TLS 1.3 certificate trust validation.
static mut TLS_VERIFIER: tls::DefaultCertVerifier<'static> = tls::DefaultCertVerifier::new();

/// BSS-allocated server name buffer for TLS SNI and certificate hostname matching.
static mut TLS_HOSTNAME_BUF: [u8; 128] = [0; 128];
static mut TLS_HOSTNAME_LEN: usize = 0;

/// How many bytes of server name the buffer holds.
///
/// WHY this is a constant rather than the buffer's own length: reading the
/// length of a mutable static takes a shared reference to it, which the
/// compiler rightly refuses to do.
const TLS_HOSTNAME_CAPACITY: usize = 128;

/// Whether the last server name was refused rather than stored.
///
/// WHY this is remembered instead of being reported once: the name is set on
/// the navigation path and read on the fetch path, and a name that was
/// refused must not turn into a different name on the way to the handshake.
static mut TLS_HOSTNAME_REFUSED: bool = false;

/// Sets the active server name for TLS 1.3 connections.
///
/// WHY a name that does not fit is refused instead of truncated: the name the
/// client advertises in SNI and the identity the verifier pins have to be the
/// whole name. A truncated name is a prefix, and a certificate issued for the
/// prefix is not a certificate for the host that was dialled.
///
/// WHY a name that is not text is refused: the name is a host label, and a
/// byte string that is not text cannot be compared against a certificate's
/// name honestly.
pub fn set_current_server_name(name: &[u8]) {
    unsafe {
        let buf = &mut *core::ptr::addr_of_mut!(TLS_HOSTNAME_BUF);
        if name.len() > TLS_HOSTNAME_CAPACITY || core::str::from_utf8(name).is_err() {
            TLS_HOSTNAME_REFUSED = true;
            TLS_HOSTNAME_LEN = 0;
            return;
        }
        buf[..name.len()].copy_from_slice(name);
        TLS_HOSTNAME_LEN = name.len();
        TLS_HOSTNAME_REFUSED = false;
    }
}

/// Whether the verifier this connection was built with holds no trust anchor.
///
/// WHY the browser asks its own verifier: the anchor list comes from the device,
/// and a device whose operator never installed one has none -- the OS ships no
/// root. That is a fact about this device, and a screen that says "the certificate
/// was rejected" when nothing could ever have been accepted here is describing a
/// fault the browser did not observe.
///
/// WHY it reads the verifier rather than the kernel again: the verifier was built
/// from the store this connection was allowed to read, so asking it is asking what
/// the handshake actually judged with.
fn tls_trust_store_is_empty() -> bool {
    unsafe { (&(*core::ptr::addr_of!(TLS_VERIFIER)).trusted_roots).is_empty() }
}

/// The outcome of a fetch over TCP.
struct FetchOutcome {
    /// Whether the bytes arrived over a TLS session whose handshake completed.
    ///
    /// WHY it is kept: the secure mark in the omnibox is a claim about the
    /// transport that delivered the document, so it has to be read from what
    /// the transport actually did rather than inferred from the scheme the
    /// user typed.
    tls_verified: bool,
    /// The number of bytes received.
    bytes: usize,
    /// Whether the response filled the buffer, and so was truncated.
    truncated: bool,
    /// The last error the transport reported, if any.
    error: i64,
    /// The tick at which the first response byte arrived.
    ///
    /// WHY it is kept: the speed test reports a round trip only from a
    /// reading the clock actually gave, so the first byte's time is
    /// measured rather than inferred from the whole transfer.
    first_byte_tick: Option<u64>,
    /// The specific stage or nature of transport/connection failure.
    kind: FetchErrorKind,
}

/// The initial sequence number for one connection.
///
/// WHY a refused read ends the connection instead of standing in for it: the
/// value used to fall back to was written at the desk and is the same number on
/// every connection this device opens, so anyone who has read one connection has
/// read the next. That is not a fallback, it is a guessable value presented as a
/// choice. The kernel grants this app `CryptoEntropyRead` in its own spawn table
/// -- the same narrow read that seeds the TLS handshake's key material -- so a
/// refusal here means the entropy service said no, and the honest answer to that
/// is a connection that was never opened and a screen that says so.
///
/// WHY no strength is claimed: these are the bytes `sys_get_random` handed back.
/// What they are worth is the kernel's answer, not this function's, and nothing
/// here says they are anything more than that.
fn connection_sequence_number() -> Option<u32> {
    let mut seq_bytes = [0u8; 4];
    match std::get_random(&mut seq_bytes) {
        Ok(4) => Some(u32::from_le_bytes(seq_bytes)),
        _ => None,
    }
}

/// Fetches an HTTP response over a TLS 1.3 encrypted transport.
///
/// An `https` hop wraps its request in a TLS 1.3 ClientHello record and never
/// sends raw plaintext HTTP GET, whatever port that hop resolves to. On TLS
/// failure (handshake failure, cert rejected, unsupported cipher) the
/// connection fails closed with explicit FetchErrorKind::TlsHandshakeFailed and
/// no plaintext request is emitted.
fn fetch_over_tls(
    dst_ip: [u8; 4],
    dst_port: u16,
    local_port: u16,
    server_name: &'static str,
    request: &[u8],
    window: usize,
) -> FetchOutcome {
    let mut outcome = FetchOutcome {
        tls_verified: false,
        bytes: 0,
        truncated: false,
        error: 0,
        first_byte_tick: None,
        kind: FetchErrorKind::None,
    };

    let seq = match connection_sequence_number() {
        Some(seq) => seq,
        None => {
            outcome.kind = FetchErrorKind::TlsEntropyRefused;
            return outcome;
        }
    };

    let connection = match std::tcp_connect(dst_ip, dst_port, local_port, seq) {
        Ok(id) => id,
        Err(code) => {
            outcome.error = code;
            outcome.kind = FetchErrorKind::NoTransport;
            return outcome;
        }
    };

    // Asynchronous TCP handshake: wait until connection is established
    let handshake_start = std::get_ticks().ok();
    let mut connected = false;
    let mut attempts = 0usize;
    loop {
        // Under InterOS non-blocking socket syscalls, a zero-length send verifies
        // connection establishment without emitting network payload.
        match std::tcp_send(connection, &[]) {
            Ok(()) => {
                connected = true;
                break;
            }
            Err(code) if code == NOT_READY => {}
            Err(code) => {
                outcome.error = code;
                outcome.kind = FetchErrorKind::ConnectionRefused;
                let _ = std::tcp_close(connection);
                return outcome;
            }
        }
        attempts += 1;
        let expired = match (std::get_ticks(), handshake_start) {
            (Ok(now), Some(start)) => now.saturating_sub(start) >= TCP_HANDSHAKE_TIMEOUT_TICKS,
            _ => attempts >= TCP_SEND_ATTEMPTS,
        };
        if expired {
            break;
        }
        std::yield_now();
    }
    if !connected {
        outcome.error = NOT_READY;
        outcome.kind = FetchErrorKind::ConnectionTimeout;
        let _ = std::tcp_close(connection);
        return outcome;
    }

    // The name was staged once, on the navigation path, by
    // `set_current_server_name`, and the caller hands back exactly those bytes.
    //
    // WHY this path no longer copies them into the buffer again: the argument it
    // was given *is* a borrow of that buffer, so the copy wrote a block of bytes
    // onto itself, and `copy_nonoverlapping`'s overlap check panicked before a
    // single TLS byte was read. The buffer has one writer so that can never
    // happen again, and a name that did not fit or was not text was already
    // refused by that writer before this function was reached.
    let host_str: &'static str = server_name;

    // WHY the clock is part of the verifier: a certificate's expiry is an
    // absolute instant, and a verifier that is not given one can only confirm
    // that two timestamps are present -- so a key that ran out years ago would
    // verify. The RTC is the only source of that instant the guest has, so a
    // reading that does not arrive ends the connection here rather than
    // continuing with a date the browser invented.
    let now_unix = match std::rtc::Reading::now().and_then(|reading| reading.unix_utc()) {
        Some(instant) => instant,
        None => {
            outcome.kind = FetchErrorKind::TlsClockUnavailable;
            let _ = std::tcp_close(connection);
            return outcome;
        }
    };

    // WHY the anchors are read again for every connection: a chain verifies only
    // against an anchor the operator installed on this device, and that happens
    // from the console at a moment of the operator's choosing. A store read once at
    // startup would keep refusing a chain the device could verify an instant later.
    // An answer of "none" is not a fault to report here -- it is the device holding
    // no anchor, which the page says in those words below.
    let device_anchors = match tls::load_device_trust_anchors() {
        Ok(roots) => roots,
        Err(_) => {
            outcome.kind = FetchErrorKind::TlsTrustStoreRefused;
            let _ = std::tcp_close(connection);
            return outcome;
        }
    };

    unsafe {
        *core::ptr::addr_of_mut!(TLS_VERIFIER) =
            tls::DefaultCertVerifier::for_host(host_str)
                .with_roots(device_anchors)
                .with_clock(now_unix);
    }

    let mut random = [0u8; 32];
    if std::get_random(&mut random).is_err() || random == [0u8; 32] || random == [0x42u8; 32] {
        outcome.kind = FetchErrorKind::TlsEntropyRefused;
        let _ = std::tcp_close(connection);
        return outcome;
    }
    let mut private_key = [0u8; 32];
    if std::get_random(&mut private_key).is_err() || private_key == [0u8; 32] || private_key == [0x24u8; 32] {
        outcome.kind = FetchErrorKind::TlsEntropyRefused;
        let _ = std::tcp_close(connection);
        return outcome;
    }

    let config = match tls::TlsConfig::new(
        host_str,
        random,
        private_key,
        unsafe { &*core::ptr::addr_of!(TLS_VERIFIER) },
    ) {
        Ok(c) => c,
        Err(_) => {
            outcome.kind = FetchErrorKind::TlsHandshakeFailed;
            let _ = std::tcp_close(connection);
            return outcome;
        }
    };

    let config_session = tls::TlsConfig {
        server_name: Some(host_str),
        random,
        private_key,
        verifier: unsafe { &*core::ptr::addr_of!(TLS_VERIFIER) },
    };
    unsafe {
        *core::ptr::addr_of_mut!(TLS_SESSION_STORAGE) = Some(tls::TlsClientSession::new(config_session));
    }

    let stream = TcpSocketStream::new(connection, TCP_RESPONSE_STALL_TICKS);
    let mut tls_client = tls::TlsClient::new(stream, config);

    // Perform TLS 1.3 handshake: ClientHello -> ServerHello -> EncryptedExtensions -> Cert -> CertVerify -> Finished
    if let Err(kind) = perform_tls_handshake(&mut tls_client) {
        outcome.kind = kind;
        let _ = std::tcp_close(connection);
        return outcome;
    }
    // The only place that says a channel was authenticated, and it is set after
    // the handshake completed and the verifier rejected nothing. Every failure
    // path above returns before this line, so nothing unverified can be marked.
    outcome.tls_verified = true;

    // Send HTTP request encrypted via tls_client.write
    if let Err(_) = tls_client.write(request) {
        outcome.kind = FetchErrorKind::TlsHandshakeFailed;
        let _ = std::tcp_close(connection);
        return outcome;
    }

    // Receive decrypted HTTP response via tls_client.read
    let cap = window.min(MAX_RESPONSE_BYTES);
    let mut total = 0usize;
    let mut record_refused = false;
    loop {
        if total >= cap {
            outcome.truncated = true;
            break;
        }
        let buffer = unsafe { &mut *core::ptr::addr_of_mut!(RESPONSE_BUF) };
        let want = (cap - total).min(TCP_READ_CHUNK);
        match tls_client.read(&mut buffer[total..total + want]) {
            Ok(0) => break,
            Ok(n) => {
                total += n;
                if outcome.first_byte_tick.is_none() {
                    outcome.first_byte_tick = std::get_ticks().ok();
                }
            }
            Err(_) => {
                record_refused = true;
                break;
            }
        }
    }

    let _ = std::tcp_close(connection);
    outcome.bytes = total;
    if total == 0 {
        // WHY the two zero-byte cases are separate: an empty read is the peer
        // closing after a request it answered with nothing, while an error here
        // is this client refusing a record it did receive. Only the first is a
        // fact about the gateway.
        outcome.kind = if record_refused {
            FetchErrorKind::TlsRecordRefused
        } else {
            FetchErrorKind::ZeroBytes
        };
    }
    outcome
}

/// Fetches an HTTP response over TCP.
///
/// WHY TCP and not UDP: the Device Studio gateway speaks HTTP over TCP.
/// The browser used to open a UDP socket and send the request as a single
/// datagram, which no TCP server answers, and then read once -- before a
/// response could have arrived. The device therefore reported a transport
/// failure for every page, including the ones the gateway would have
/// served.
///
/// WHY the caller names the window: a subresource is often bigger than the
/// room the reader has for it, and bytes past that room are dropped where
/// they land. Stopping the transfer at the room saves the wire and the time
/// for the fetches behind it -- a page that chains twenty stylesheets pays
/// for every one of them.
/// WHY the caller names the security, not the port: `https` on any port means
/// the channel has to be TLS, and an `http` URL on 443 means it does not.
/// Deciding here from `dst_port == 443` let `https://host:4300` reach the wire
/// as a plaintext GET while the omnibox still drew the mark the scheme implied.
fn fetch_over_tcp(
    dst_ip: [u8; 4],
    dst_port: u16,
    local_port: u16,
    request: &[u8],
    window: usize,
    use_tls: bool,
) -> FetchOutcome {
    if use_tls {
        // WHY a refused name ends the fetch here: the name is what the
        // peer's certificate is checked against, and a name the browser would
        // not store is a name it cannot honestly pin. Falling back to another
        // name would check the certificate against something the user never
        // asked for.
        let host: Option<&'static str> = unsafe {
            if TLS_HOSTNAME_REFUSED {
                None
            } else {
                let buf = &*core::ptr::addr_of!(TLS_HOSTNAME_BUF);
                core::str::from_utf8(&buf[..TLS_HOSTNAME_LEN]).ok()
            }
        };
        let Some(host_str) = host else {
            let mut outcome = FetchOutcome {
                tls_verified: false,
                bytes: 0,
                truncated: false,
                error: 0,
                first_byte_tick: None,
                kind: FetchErrorKind::None,
            };
            outcome.kind = FetchErrorKind::TlsNameRefused;
            return outcome;
        };
        return fetch_over_tls(dst_ip, dst_port, local_port, host_str, request, window);
    }
    let mut outcome = FetchOutcome {
        tls_verified: false,
        bytes: 0,
        truncated: false,
        error: 0,
        first_byte_tick: None,
        kind: FetchErrorKind::None,
    };

    // The initial sequence number is the device's choice, and a fresh one
    // per connection is what the protocol asks for.
    let seq = match connection_sequence_number() {
        Some(seq) => seq,
        None => {
            // Nothing has been put on the wire: the refusal is what stops the
            // connection, not an answer the gateway gave.
            outcome.kind = FetchErrorKind::EntropyRefused;
            return outcome;
        }
    };

    let connection = match std::tcp_connect(dst_ip, dst_port, local_port, seq) {
        Ok(id) => id,
        Err(code) => {
            outcome.error = code;
            outcome.kind = FetchErrorKind::NoTransport;
            return outcome;
        }
    };

    // The handshake is asynchronous, so the first sends are refused. The
    // refusal is `NotReady`, which is not a failure -- it means the
    // connection is not established yet.
    // An absent clock is not tick zero: the loop falls back to a send
    // budget rather than measuring against a start time it never read.
    let handshake_start = std::get_ticks().ok();
    let mut sent = false;
    let mut attempts = 0usize;
    loop {
        match std::tcp_send(connection, request) {
            Ok(()) => {
                sent = true;
                break;
            }
            Err(code) => {
                if code != NOT_READY {
                    outcome.error = code;
                    outcome.kind = FetchErrorKind::ConnectionRefused;
                    let _ = std::tcp_close(connection);
                    return outcome;
                }
            }
        }
        attempts += 1;
        let expired = match (std::get_ticks(), handshake_start) {
            (Ok(now), Some(start)) => now.saturating_sub(start) >= TCP_HANDSHAKE_TIMEOUT_TICKS,
            _ => attempts >= TCP_SEND_ATTEMPTS,
        };
        if expired {
            break;
        }
        std::yield_now();
    }
    if !sent {
        outcome.error = NOT_READY;
        outcome.kind = FetchErrorKind::ConnectionTimeout;
        let _ = std::tcp_close(connection);
        return outcome;
    }

    // The response is read until the server closes the connection. The
    // gateway speaks HTTP/1.0 and sends no content length, so the end of
    // the body is the end of the stream.
    let response_start = std::get_ticks().ok();
    let mut last_data_tick = response_start;
    let mut total = 0usize;
    let mut reads = 0usize;
    let mut recv_stalled = false;
    let cap = window.min(MAX_RESPONSE_BYTES);
    loop {
        reads += 1;
        let buffer = unsafe { &mut *core::ptr::addr_of_mut!(RESPONSE_BUF) };
        if total >= cap {
            outcome.truncated = true;
            break;
        }
        let want = (cap - total).min(TCP_READ_CHUNK);
        match std::tcp_recv(connection, &mut buffer[total..total + want]) {
            Ok(0) => break,
            Ok(n) => {
                total += n;
                last_data_tick = std::get_ticks().ok();
                if outcome.first_byte_tick.is_none() {
                    outcome.first_byte_tick = std::get_ticks().ok();
                }
            }
            Err(code) => {
                if code != NOT_READY {
                    // A closed connection ends the response. The bytes
                    // already read are still the response.
                    if total == 0 {
                        outcome.error = code;
                        outcome.kind = FetchErrorKind::ConnectionRefused;
                    }
                    break;
                }
            }
        }
        let now = std::get_ticks().ok();
        let stalled = match (now, last_data_tick) {
            (Some(now), Some(mark)) => now.saturating_sub(mark) >= TCP_RESPONSE_STALL_TICKS,
            _ => false,
        };
        let expired = match (now, response_start) {
            (Some(now), Some(start)) => now.saturating_sub(start) >= TCP_RESPONSE_TIMEOUT_TICKS,
            _ => reads >= TCP_RECV_ATTEMPTS,
        };
        if stalled || expired {
            outcome.truncated = true;
            recv_stalled = true;
            break;
        }
        std::yield_now();
    }

    let _ = std::tcp_close(connection);
    outcome.bytes = total;
    if total == 0 {
        if recv_stalled {
            outcome.kind = FetchErrorKind::ConnectionStalled;
        } else if outcome.kind == FetchErrorKind::None {
            outcome.kind = FetchErrorKind::ZeroBytes;
        }
    }
    outcome
}

/// The bytes an upload sends after its request head.
enum UploadSource<'a> {
    /// A body already held in memory, used by the speed test.
    Bytes(&'a [u8]),
    /// A file read in steps, so an upload is not bounded by one buffer.
    File { path: &'a str, length: usize },
}

/// The outcome of an upload over TCP.
struct UploadOutcome {
    /// The body bytes the transport accepted.
    sent: usize,
    /// The number of response bytes received.
    response: usize,
    /// The last error the transport reported, if any.
    error: i64,
}

/// Sends `bytes` in segment-sized steps.
///
/// WHY the retry loop: `tcp_send` refuses with `NotReady` until the
/// handshake completes and while the send window is full. A body longer
/// than one segment therefore needs the same waiting the request head
/// needs, not a single call.
fn tcp_send_all(connection: u32, bytes: &[u8], timeout_ticks: u64) -> Result<(), i64> {
    let start = std::get_ticks().ok();
    let mut offset = 0usize;
    while offset < bytes.len() {
        let end = (offset + TCP_SEND_CHUNK).min(bytes.len());
        let mut attempts = 0usize;
        loop {
            match std::tcp_send(connection, &bytes[offset..end]) {
                Ok(()) => break,
                Err(code) if code == NOT_READY => {}
                Err(code) => return Err(code),
            }
            attempts += 1;
            let expired = match (std::get_ticks(), start) {
                (Ok(now), Some(start)) => now.saturating_sub(start) >= timeout_ticks,
                _ => attempts >= TCP_SEND_ATTEMPTS,
            };
            if expired {
                return Err(NOT_READY);
            }
            std::yield_now();
        }
        offset = end;
    }
    Ok(())
}

/// Posts a request head and a body to the gateway and reads its reply.
///
/// WHY a file is streamed: the upload picker sends the document the index
/// named, and a document larger than one buffer still has to reach the
/// gateway as the same bytes. The declared length is what the gateway
/// expects, so a file that ends early aborts the request rather than
/// letting the connection wait for bytes that will never come.
fn upload_over_tcp(
    dst_ip: [u8; 4],
    dst_port: u16,
    local_port: u16,
    head: &[u8],
    source: UploadSource<'_>,
) -> UploadOutcome {
    let mut outcome = UploadOutcome {
        sent: 0,
        response: 0,
        error: 0,
    };

    let seq = match connection_sequence_number() {
        Some(seq) => seq,
        None => {
            // The number is the app's own marker for the refusal, not a code
            // the transport answered with: no transport was asked. The upload
            // screen shows the code it holds, and holds no byte count.
            outcome.error = ENTROPY_REFUSED;
            return outcome;
        }
    };

    let connection = match std::tcp_connect(dst_ip, dst_port, local_port, seq) {
        Ok(id) => id,
        Err(code) => {
            outcome.error = code;
            return outcome;
        }
    };

    if let Err(code) = tcp_send_all(connection, head, TCP_HANDSHAKE_TIMEOUT_TICKS) {
        outcome.error = code;
        let _ = std::tcp_close(connection);
        return outcome;
    }

    match source {
        UploadSource::Bytes(body) => {
            if let Err(code) = tcp_send_all(connection, body, TCP_BODY_TIMEOUT_TICKS) {
                outcome.error = code;
                let _ = std::tcp_close(connection);
                return outcome;
            }
            outcome.sent = body.len();
        }
        UploadSource::File { path, length } => {
            let fd = match std::fs_open(path, std::FS_READ) {
                Ok(fd) => fd,
                Err(code) => {
                    outcome.error = code;
                    let _ = std::tcp_close(connection);
                    return outcome;
                }
            };
            let mut sent = 0usize;
            let mut failure = None;
            while sent < length {
                let want = (length - sent).min(MAX_FILE_BYTES);
                let buffer = unsafe { &mut *core::ptr::addr_of_mut!(FILE_BUF) };
                let read = match std::fs_read(fd, &mut buffer[..want]) {
                    Ok(0) => {
                        failure = Some(-5i64); // EIO: shorter than the index said
                        break;
                    }
                    Ok(n) => n,
                    Err(code) => {
                        failure = Some(code);
                        break;
                    }
                };
                if let Err(code) = tcp_send_all(connection, &buffer[..read], TCP_BODY_TIMEOUT_TICKS) {
                    failure = Some(code);
                    break;
                }
                sent += read;
            }
            let _ = std::fs_close(fd);
            outcome.sent = sent;
            if let Some(code) = failure {
                outcome.error = code;
                let _ = std::tcp_close(connection);
                return outcome;
            }
            if sent != length {
                outcome.error = -5;
                let _ = std::tcp_close(connection);
                return outcome;
            }
        }
    }

    let response_start = std::get_ticks().ok();
    let mut total = 0usize;
    let mut reads = 0usize;
    loop {
        reads += 1;
        let buffer = unsafe { &mut *core::ptr::addr_of_mut!(RESPONSE_BUF) };
        if total >= buffer.len() {
            break;
        }
        let want = (buffer.len() - total).min(TCP_READ_CHUNK);
        match std::tcp_recv(connection, &mut buffer[total..total + want]) {
            Ok(0) => break,
            Ok(n) => total += n,
            Err(code) if code == NOT_READY => {}
            Err(code) => {
                if total == 0 {
                    outcome.error = code;
                }
                break;
            }
        }
        let expired = match (std::get_ticks(), response_start) {
            (Ok(now), Some(start)) => now.saturating_sub(start) >= TCP_BODY_TIMEOUT_TICKS,
            _ => reads >= TCP_RECV_ATTEMPTS,
        };
        if expired {
            break;
        }
        std::yield_now();
    }

    let _ = std::tcp_close(connection);
    outcome.response = total;
    outcome
}


/// The platform's corner radii, under the short names the renderer uses.
///
/// WHY the aliases exist: the corner test names its radius four times, and a
/// number that appears four times is a number that can drift from the
/// platform. The alias resolves to the platform's step, so the drift is the
/// platform's and not this app's.
const R_CARD: usize = radius::CARD;
const R_CHIP: usize = radius::CHIP;
const R_CONTROL: usize = radius::CONTROL;
const R_SMALL: usize = radius::SMALL;
const R_SOFT: usize = radius::SOFT_PILL;

/// The platform's card treatment, at the elevation the browser's cards sit at.
///
/// WHY the browser's cards are the platform's cards: a card that invents its
/// own fill and its own hairline is a card a third-party app cannot match, and
/// the platform already says what a card looks like. The raised elevation is
/// the one the browser's cards have always been -- a fill one step above the
/// page, with a hairline around it.
const CARD: components::CardStyle =
    components::card_style(&THEME, components::Elevation::Raised);

const W: usize = 720;
const H: usize = 1440;
const CHUNK: usize = 4096;

/// y 0..48: System Status Bar
const STATUS_H: usize = 48;

/// y 48..116: Sovereign Top Omnibox App Bar
#[allow(dead_code)]
const TOOLBAR_Y0: usize = STATUS_H;
const TOOLBAR_Y1: usize = 116;

/// Sovereign Omnibox URL Capsule (x 88..476)
/// Appends bytes to a small buffer, truncating rather than overrunning.
fn append_text(buf: &mut [u8], n: usize, bytes: &[u8]) -> usize {
    let take = bytes.len().min(buf.len().saturating_sub(n));
    buf[n..n + take].copy_from_slice(&bytes[..take]);
    n + take
}

/// Appends the decimal form of a value to a small buffer.
fn append_u64_text(buf: &mut [u8], mut n: usize, mut value: u64) -> usize {
    if value == 0 {
        return append_text(buf, n, b"0");
    }
    let mut digits = [0u8; 20];
    let mut len = 0usize;
    while value > 0 && len < digits.len() {
        digits[len] = b'0' + (value % 10) as u8;
        value /= 10;
        len += 1;
    }
    while len > 0 {
        len -= 1;
        if n < buf.len() {
            buf[n] = digits[len];
            n += 1;
        }
    }
    n
}

/// A byte rate over a measured elapsed time.
///
/// WHY zero elapsed reports zero: the tick counter has millisecond
/// resolution, and a transfer that began and ended inside one tick was not
/// measured. Dividing by a zero duration would invent a rate.
fn rate_per_second(bytes: usize, ms: u64) -> u64 {
    if ms == 0 {
        return 0;
    }
    (bytes as u64).saturating_mul(1000) / ms
}

/// Appends a byte rate in the unit its size asks for.
fn append_rate_text(buf: &mut [u8], n: usize, bps: u64) -> usize {
    if bps >= 1024 * 1024 {
        let n = append_u64_text(buf, n, bps / (1024 * 1024));
        append_text(buf, n, b" MiB/s")
    } else if bps >= 1024 {
        let n = append_u64_text(buf, n, bps / 1024);
        append_text(buf, n, b" KiB/s")
    } else {
        let n = append_u64_text(buf, n, bps);
        append_text(buf, n, b" B/s")
    }
}

/// Appends a signed decimal number to a small buffer.
fn append_signed_text(buf: &mut [u8], n: usize, value: i64) -> usize {
    if value < 0 {
        let n = append_text(buf, n, b"-");
        append_u64_text(buf, n, value.unsigned_abs())
    } else {
        append_u64_text(buf, n, value as u64)
    }
}

/// The type scale of a document title.
///
/// WHY there is a scale and not a size: the platform font offers integer
/// scales, and the reader's three heading levels were all set at scale 1 --
/// so a page title and a section heading were the same size on the screen
/// and differed only in colour.
const TYPE_H1_SCALE: usize = type_scale::BODY;

/// The address capsule, clear of the platform's Back control at x 24..72.
///
/// WHY its left edge is the chip's right edge plus one grid step: the capsule
/// used to start at x 16 and covered the canonical 48x48 rect, so the corner a
/// person learns once in Settings, Dialer, Music, Camera and Vault answered
/// here by focusing the keyboard instead of giving a way back. Its right edge
/// stays at 476, where the reload control has always had its gutter, so only
/// the field moves and no row of the bar does.
const PILL: Rect = Rect::new(
    back_button::X + back_button::SIZE + spacing::GRID_16,
    56,
    476 - (back_button::X + back_button::SIZE + spacing::GRID_16),
    52,
);
const PILL_R: usize = 26;
const PILL_TEXT_X: usize = PILL.x + 40;
const PILL_TEXT_Y: usize = 70;
const PILL_TEXT_MAX: usize = PILL.w - 80;

/// WHY these gates: the chip and the field are one row, and a field that creeps
/// back under the chip, or a chip that grows past the band that paints it, is
/// the defect this row was rewritten for.
const _: () = assert!(PILL.x >= back_button::X + back_button::SIZE);
const _: () = assert!(back_button::Y + back_button::SIZE <= TOOLBAR_Y1);

/// The tab switcher's "+ New Tab" control.
///
/// WHY 44 and not 38: the design system's minimum touch target is 44 logical
/// pixels and this button was 38. The tab switcher header band is 120..170, so
/// the button now fills the band from its top to its bottom and the tab cards
/// below, which start at 180, are untouched.
const NEW_TAB_BTN: Rect = Rect::new(530, 126, 150, MIN_TOUCH_TARGET);

/// Omnibox Right Controls
const RELOAD_BTN: Rect = Rect::new(486, 60, 44, 44);

/// The square at the head of the address field that wears the page's own
/// picture, where the padlock used to sit alone.
const FAVICON_BOX: Rect = Rect::new(PILL.x + 14, 74, 16, 16);

/// The centre the page's picture and the TLS mark share at the head of the
/// field. WHY named: both are one mark about one address, drawn in one box, and
/// the box now moves with the field rather than sitting at a memorised pixel.
const MARK_CX: usize = FAVICON_BOX.x + FAVICON_BOX.w / 2;
const MARK_CY: usize = FAVICON_BOX.y + FAVICON_BOX.h / 2;

/// The pixels a page's face is drawn with: one RGBA quadruple for each pixel
/// of [`FAVICON_BOX`].
const FAVICON_FACE_PIXELS: usize = FAVICON_BOX.w * FAVICON_BOX.h;
const TABS_TOP_BTN: Rect = Rect::new(540, 60, 46, 44);
const KEBAB_BTN: Rect = Rect::new(596, 60, 44, 44);
const CLOSE_BTN: Rect = Rect::new(650, 60, 44, 44);
const BTN_R: usize = 12;

/// The pixel of a chrome control's box: the tile it is painted on and the
/// edge that holds it.
///
/// WHY this exists: every control in the top toolbar and the bottom
/// navigation bar used to be painted with a fill equal to the bar behind it
/// and no border at all, so reload, the tab count, the overflow kebab, the
/// close-to-launcher box and the back and forward boxes were `#FAFAFA` on
/// `#FAFAFA` -- the navigation was invisible, and a person could not see
/// that it existed let alone where its edges were.
///
/// WHY these two tokens: the tile is the platform's raised surface
/// (`#EFEEE9`), the step the app already paints every chip and field with, so
/// the control reads as an object resting on the bar rather than as the bar.
/// The edge is the platform's secondary ink (`#525458`), because
/// `Theme::border` is `#E5E5E5` -- 1.21:1 against a `#FAFAFA` bar and 1.08:1
/// against the tile it would ring, which is a hairline nobody can see. At
/// 6.53:1 on the tile, 7.27:1 on the bar and 7.59:1 on a card the edge clears
/// the 3:1 a non-text control needs with room to spare while staying inside
/// the black-white-grey ladder: no new colour, and no geometry, hit band or
/// position moves.
#[inline]
fn chrome_control_px(x: usize, y: usize, rect: Rect, radius: usize) -> Option<u32> {
    draw_rounded_rect(
        x,
        y,
        rect,
        radius,
        THEME.surface_container_high,
        Some(THEME.on_surface_variant),
        1,
    )
}

/// The ink a disabled navigation glyph is painted with.
///
/// WHY the muted role and not the hairline role: a disabled control is still
/// a control, and the app used to ink it with `Theme::border` -- `#E5E5E5`,
/// which is 1.21:1 against the `#FAFAFA` bar and 1.08:1 against the tile it
/// now sits on, so the disabled state was indistinguishable from nothing being
/// painted there at all. `Theme::muted` is the platform's disabled ink:
/// `#6B6B6B` is 4.59:1 on the tile, and the enabled ink (`Theme::on_surface`)
/// is 17.04:1 on it, so the two states are three-and-a-half stops apart from
/// each other and cannot be confused. WCAG exempts an inactive control from
/// the 3:1 a live one needs; what matters here is that "greyed out" is
/// readable as greyed out -- and at 4.59:1 this disabled ink clears the 4.5:1
/// the repo's contrast contract demands of body text anyway.
#[inline]
fn nav_glyph_ink(active: bool) -> u32 {
    if active {
        THEME.on_surface
    } else {
        THEME.muted
    }
}

/// The kebab menu's rows: where the first row's ink sits and how tall a row is.
///
/// WHY one pair of constants: the painter counted its rows from y=130 and the hit
/// test counted the same rows from y=116, so the top fourteen pixels of every row
/// belonged to the item above it -- a tap on "New Tab" opened Bookmarks, and a tap
/// on "Save Page" reloaded the page.
const MENU_ROW_Y0: usize = 130;
const MENU_ROW_H: usize = 44;

/// The kebab menu's card.
///
/// WHY one named box for the card and its rows: the card is painted as a
/// rectangle whose far edge is exclusive, and the hit test used to compare the
/// same edges with `<=`, so the column at x 696 and the row at y 532 answered
/// an item although no pixel of the card is drawn there. Both now read this
/// one box, and the gate beside it fails the build if the rows ever outgrow
/// the card that holds them.
const MENU_CARD: Rect = Rect::new(440, 116, 256, 416);

/// The menu's items, and so the number of rows the card must hold.
const MENU_ITEMS: [&[u8]; 9] = [
    b"New Tab",
    b"Bookmarks",
    b"History",
    b"Downloads",
    b"Settings",
    b"Tools",
    b"Reload Page",
    b"Save Page",
    b"Close Menu",
];

const _: () = assert!(MENU_ROW_Y0 + MENU_ITEMS.len() * MENU_ROW_H <= MENU_CARD.y + MENU_CARD.h);
const _: () = assert!(MENU_CARD.x + MENU_CARD.w <= W);
const _: () = assert!(MENU_CARD.y + MENU_CARD.h <= NAV_BAR_Y0);

/// Start Page Favorites / Quick Access Geometry (4 columns)
#[allow(dead_code)]
const QA_ROW_Y: usize = 282;
#[allow(dead_code)]
const QA_TILE_SIZE: usize = 64;

/// Sovereign Bottom Navigation Bar (y 1350..1410)
const NAV_BAR_Y0: usize = 1350;
const NAV_BAR_Y1: usize = 1410;
/// The height of every tile in the bar: the platform chip's own plate.
const NAV_BTN_H: usize = back_button::SIZE;
/// The row the tiles stand on, centred inside the bar's band.
///
/// WHY derived and not a remembered 1356: the band is 60 rows and the plate is
/// `NAV_BTN_H`, so the air above and below is what is left over -- an authored
/// 1356 is a number that goes stale the moment the platform's control changes,
/// and the control is what this row is sized against.
const NAV_ROW_Y: usize = NAV_BAR_Y0 + (NAV_BAR_Y1 - NAV_BAR_Y0 - NAV_BTN_H) / 2;
/// The Back tile. It stays the app's own rect, and for a real layout reason: the
/// browser puts its navigation on the bar under the page, not in the header the
/// SDK chip owns -- and the header chip is adopted here too, painted by
/// `back_button::pixel_for_theme` and answered by `back_button::contains`.
///
/// WHY painted == hit is structural: `bottom_bar_pixel` inks the whole plate
/// through `chrome_control_px(x, y, BACK_BTN, BTN_R)` and the press tests the
/// same `BACK_BTN`, so neither can claim a column the other has not; the width
/// the bar gives this tile (110) is fill the person sees, not dead tap area.
const BACK_BTN: Rect = Rect::new(back_button::X, NAV_ROW_Y, 110, NAV_BTN_H);
// WHY this gate: the tile has to stay inside the band the bar paints. A plate
// that left the band would answer presses on rows the bar never inks -- the
// mismatch this task is about, in the other direction.
const _: () = assert!(BACK_BTN.y >= NAV_BAR_Y0 && BACK_BTN.y + BACK_BTN.h <= NAV_BAR_Y1);
const FWD_BTN: Rect = Rect::new(160, 1356, 110, 48);
const HOME_TAB_BTN: Rect = Rect::new(296, 1356, 128, 48);
const ADD_BOOK_BTN: Rect = Rect::new(448, 1356, 110, 48);
const TABS_BTN: Rect = Rect::new(582, 1356, 114, 48);

/// System Gesture Home Indicator (y 1410..1440)
const GESTURE_PILL: Rect = Rect::new(260, 1422, 200, 6);
const HOME_Y0: usize = 1410;

/// The top of the band the reader draws the document in.
const READER_TOP_Y: usize = 126;

/// The height of the band the reader draws the document in.
///
/// WHY it is named: the reader's viewport is the space between the
/// toolbar and the navigation bar, and the scroll state has to know how
/// tall that band is to know how far a document can move in it.
const READER_VIEWPORT_H: usize = NAV_BAR_Y0 - READER_TOP_Y;

// -----------------------------------------------------------------------------
// The quick-link row
//
// WHY the row is a row of pills and not a stack of cards: the platform
// offers the pill as its treatment for a tappable run of text, and the
// browser had drawn its own rounded rectangle with its own radius. The
// row is now painted by the platform, so it is the row the shell draws.
// -----------------------------------------------------------------------------

/// The left edge of the quick-link row.
const QUICK_LINK_X: usize = 24;

/// The top of the quick-link row.
const QUICK_LINK_Y: usize = 156;

/// The height of a quick-link pill.
const QUICK_LINK_H: usize = 52;

/// The width of the quick-link row.
const QUICK_LINK_W: usize = W - 48;

/// The gap between the two pills.
const QUICK_LINK_GAP: usize = 16;

/// The top of the row of addresses under the pills.
const QUICK_LINK_URL_Y: usize = QUICK_LINK_Y + QUICK_LINK_H + 4;

/// The height of the row of addresses under the pills.
const QUICK_LINK_URL_H: usize = 24;

/// The quick links the address bar offers, and the addresses they open.
///
/// WHY there are two and not three: the third was a gemini:// link, and
/// the browser reads pages over HTTP. It cannot fetch that scheme, so the
/// link led to an error screen.
const QUICK_LINKS: [(&[u8], &[u8]); 2] = [
    (b"InterOS", b"https://interos.tech"),
    (b"GitHub", b"https://github.com"),
];

/// The quick-link row, painted by the platform's pill.
///
/// WHY a buffer and not a drawing call: the platform's pill paints into
/// a surface, and this app's renderer answers one pixel at a time. The
/// row is painted once per frame and the renderer reads the pixel it
/// needs out of the buffer.
static mut QUICK_LINK_ROW: [u32; QUICK_LINK_W * QUICK_LINK_H] = [0; QUICK_LINK_W * QUICK_LINK_H];

/// How many saved pages the downloads list holds.
const MAX_DOWNLOADS: usize = 12;

/// How many characters a list filter holds.
const FILTER_MAX: usize = 32;

/// The top of the filter row in the bookmark and history sheets.
const FILTER_Y: usize = 176;

/// The height of the filter row in the bookmark and history sheets.
const FILTER_H: usize = 44;

/// The first row below the filter row.
const LIST_Y: usize = 236;

/// The stride between two bookmark rows.
const BOOKMARK_ROW_PITCH: usize = 80;

/// The gap left between one bookmark card and the next.
const BOOKMARK_ROW_GAP: usize = 10;

/// The height of a bookmark card.
///
/// WHY the hit test uses this and not the pitch: the card is drawn inside the
/// pitch, so the ten rows of gap at its foot belonged to the card above -- eight
/// rows of nothing between two history cards answered the visit they were not
/// drawn for. Both sheets now test the box they paint.
const BOOKMARK_CARD_H: usize = BOOKMARK_ROW_PITCH - BOOKMARK_ROW_GAP;

// -----------------------------------------------------------------------------
// The history sheet's rows
//
// WHY the card is derived from the rows of text it owns: the card used to be
// `row_h - 8` -- fifty-two rows -- while its second run started at row thirty-
// two of it and reserved a whole line box, fifty-four rows in all. A per-pixel
// painter has no clip: the run's last two rows fell outside the card the
// label was drawn in, and the card's own fill answered them, so the descenders
// of `g`, `j`, `p`, `q` and `y` were erased at the card's bottom border. Every
// number below now comes from the type ladder, and the gates beside them fail
// the build if the ladder and the card ever disagree.
// -----------------------------------------------------------------------------

/// The stride between two history rows.
const HISTORY_ROW_PITCH: usize = HISTORY_CARD_H + HISTORY_ROW_GAP;

/// The gap left between one history card and the next.
const HISTORY_ROW_GAP: usize = 8;

/// The height of a history card: the block it carries, plus the air above and
/// below it. The stride now follows the card rather than leading it.
const HISTORY_CARD_H: usize = HISTORY_BLOCK_H + 2 * HISTORY_BLOCK_PAD_Y;

/// How many history rows the sheet walks.
const HISTORY_ROWS: usize = 8;

/// The rung both of a history row's runs are set at.
const HISTORY_RUN_SCALE: usize = type_scale::CAPTION;

/// One run's line box at [`HISTORY_RUN_SCALE`].
const HISTORY_LINE_BOX: usize = font::line_height(HISTORY_RUN_SCALE);

/// The left edge of a history card.
const HISTORY_CARD_X: usize = 24;

/// The width of a history card.
const HISTORY_CARD_W: usize = W - 2 * HISTORY_CARD_X;

/// Where a history row's text starts.
const HISTORY_TEXT_X: usize = 40;

/// The inset between the card's edge and its text, on each side.
const HISTORY_TEXT_INSET: usize = HISTORY_TEXT_X - HISTORY_CARD_X;

/// The width a history row's run is cut to.
const HISTORY_TEXT_MAX: usize = HISTORY_CARD_W - 2 * HISTORY_TEXT_INSET;

/// The two runs a named visit reserves, as one block.
const HISTORY_BLOCK_H: usize = 2 * HISTORY_LINE_BOX;

/// Air above and below that block inside the card, on each side.
const HISTORY_BLOCK_PAD_Y: usize = 4;

/// WHY the block is inset as a block: a row's title and address read as one
/// paragraph, so the space above the first line equals the space below the
/// second, and neither line's descenders can fall past the card.
const HISTORY_BLOCK_Y: usize = (HISTORY_CARD_H - HISTORY_BLOCK_H) / 2;

/// The top of a named visit's address line.
const HISTORY_SECOND_Y: usize = HISTORY_BLOCK_Y + HISTORY_LINE_BOX;

/// The top of an unnamed visit's single line.
const HISTORY_SINGLE_Y: usize = (HISTORY_CARD_H - HISTORY_LINE_BOX) / 2;

const _: () = assert!(HISTORY_BLOCK_Y + HISTORY_BLOCK_H + HISTORY_BLOCK_Y <= HISTORY_CARD_H);
const _: () = assert!(HISTORY_SINGLE_Y + HISTORY_LINE_BOX <= HISTORY_CARD_H);
const _: () = assert!(HISTORY_TEXT_X + HISTORY_TEXT_MAX <= HISTORY_CARD_X + HISTORY_CARD_W);
const _: () = assert!(HISTORY_ROWS * HISTORY_ROW_PITCH <= NAV_BAR_Y0 - LIST_Y);

const PLACEHOLDER: &[u8] = b"Search or type URL";

// Contract strings pinned for test suites
//
// WHY the body of the start page says what the screen does and not what the
// app is: the line used to read "Sovereign Decentralized Web Client on
// InterOS Microkernel.", which is a claim about the platform presented as
// the page the person was looking at. The page now says what is on it.
const BODY_BLANK: &[u8] = b"Type an address to open a page, or pick one of the quick links below.";
const BODY_LOADED: &[u8] = b"The page's text is shown here. Tap a link to open it.";
const BODY_NO_TRANSPORT: &[u8] = b"The address was parsed and a navigation was attempted, \
but no network transport is bound to this app: no request was sent and no page was loaded.";
const BODY_NETWORK_DOWN: &[u8] = b"The network interface is down or unreachable. \
The device could not route packets to the destination.";
const BODY_DNS_FAILED: &[u8] = b"The host name could not be resolved by the system DNS \
service. No address was dialled and no request was sent.";
const BODY_NO_ENTROPY: &[u8] = b"The device's entropy service refused the read that chooses a \
connection's starting number, so the browser had no number to open with and opened nothing. \
The value it used to fall back to was the same on every connection, so one the person could \
have seen would have disclosed the next; a fixed number was dropped rather than used.";
const BODY_CONNECTION_REFUSED: &[u8] = b"The connection to the gateway at 10.0.2.2:4300 \
was refused. The proxy service may not be running on the host or the port is closed.";
const BODY_CONNECTION_TIMEOUT: &[u8] = b"The connection to the gateway at 10.0.2.2:4300 \
timed out during the TCP handshake. The gateway did not respond in time.";
const BODY_CONNECTION_STALLED: &[u8] = b"The connection to the gateway was established, \
but the transfer stalled before completion. No further bytes were received.";
const BODY_TLS_HANDSHAKE_FAILED: &[u8] = b"The TLS 1.3 secure connection could not be established. \
The peer's certificate was rejected or the cryptographic handshake failed. \
The browser failed closed to protect your data.";
const BODY_TLS_NO_TRUST_ANCHOR: &[u8] = b"The TLS 1.3 secure connection could not be established. \
This browser holds no trust anchor for a certificate chain, so it cannot accept \
the peer's certificate. The browser failed closed: no plaintext was sent.";
const BODY_TLS_NAME_REFUSED: &[u8] = b"The host name in the address was refused before the secure \
connection was opened, so no certificate could be checked against it. The browser \
failed closed: no plaintext was sent.";
const BODY_TLS_ENTROPY_REFUSED: &[u8] = b"The device's random-number service refused the reads \
that seed a secure connection, so the browser had no starting number and no key to offer. The \
connection was closed before a single handshake byte was sent; a key folded out of the boot \
clock was refused rather than used, because both peers could have derived it.";
const BODY_TLS_CLOCK_UNAVAILABLE: &[u8] = b"The device's clock answered no instant, so the browser \
had nothing to measure a certificate's expiry against. The connection was closed before the \
handshake: accepting a key whose validity window cannot be judged would carry the padlock past \
a check the browser never made.";
const BODY_TLS_TRUST_STORE_REFUSED: &[u8] = b"The kernel refused to hand this browser the trust \
anchors installed on the device, so the browser cannot tell what it is allowed to trust. The \
connection was closed before the handshake: verifying against a store the browser was not \
allowed to read would report a decision it never had the information to make.";
const BODY_ZERO_BYTES: &[u8] = b"The gateway connection succeeded and the request was sent, \
but the gateway closed the connection without returning any response data.";
const BODY_TLS_RECORD_REFUSED: &[u8] = b"The secure channel was opened, the request was sent, \
and an answer arrived, but this device's record reader refused the answer and accepted none of \
it. The gateway replied; nothing of the page was read here.";
const BODY_PROTOCOL_MALFORMED: &[u8] = b"The gateway returned a response that could not be \
parsed as valid HTTP or gateway envelope data.";
const BODY_PROTOCOL_ENCODING: &[u8] = b"The response payload had invalid encoding or \
compression that could not be decoded by the browser.";
const BODY_PROTOCOL_UNPARSEABLE: &[u8] = b"The gateway returned a document, but its format \
could not be parsed into a displayable page.";
const BODY_PROTOCOL_EMPTY: &[u8] = b"The page was received and decoded, but contained no \
readable text or displayable elements.";
const BODY_INVALID: &[u8] = b"Supported schemes are http, https, interlayer and file \
followed by a host. Even a valid address loads nothing while no transport is bound.";
const BODY_UNRECOGNISED: &[u8] = b"The address did not name a host this browser can open, \
so nothing was requested and no page was loaded.";
const BODY_UNSUPPORTED_SCHEME: &[u8] = b"This browser reads pages over HTTP. The address \
names a scheme it cannot fetch, so nothing was requested.";
const BODY_GATEWAY_ERROR: &[u8] = b"The Device Studio gateway received the request but could \
not fetch the page. The failure is the origin's or this host's network, not a page the browser \
can show.";
const BODY_LOCAL_NO_CAPABILITY: &[u8] = b"The address names a local document, but this app holds \
no filesystem read capability, so it cannot open a guest file at all.";
const BODY_LOCAL_REFUSED: &[u8] = b"The address names a local document, and this app holds the \
capability to read one, but the guest filesystem refused to open it. Nothing was read.";
const BODY_LOCAL_EMPTY: &[u8] = b"The address names a local document and the guest filesystem \
opened it, but the file holds no bytes. There is nothing to show.";
const BODY_LOCAL_READ_REFUSED: &[u8] = b"The address names a local document and the guest \
filesystem opened it, but the read itself was refused. Nothing was shown.";
const BODY_LOCAL_NO_PATH: &[u8] = b"The address names a local document but carries no path, so \
there is nothing for the browser to open.";
const BODY_DOWNLOADED: &[u8] = b"The response was a file, so it was saved to the shared storage \
area unchanged. The File Manager lists it under Downloads and opens it with the app registered \
for its type.";
const BODY_DOWNLOADED_PARTIAL: &[u8] = b"The response was a file, but only part of it arrived, so \
the saved copy is incomplete. A partial file may not open. The transfer ended early because the \
file is larger than this browser's response buffer, or because the origin stopped sending.";
const BODY_DOWNLOAD_FAILED: &[u8] = b"The response was a file, but the shared storage area did \
not accept it. Nothing was saved.";
const BODY_DOWNLOAD_NO_BYTES: &[u8] = b"The origin sent no bytes, so there was no file to save. \
Nothing was written to the shared storage area, and the File Manager lists no new document.";
const BODY_DOWNLOAD_NO_PATH: &[u8] = b"The response was a file, but the shared storage area has \
no free name for it. Nothing was saved.";
const BODY_DOWNLOAD_LOST: &[u8] = b"The bytes of the response could not be found again when the \
download was confirmed. Nothing was saved.";
const BODY_DOWNLOAD_REFUSED: &[u8] = b"The origin did not return the file. Its answer was an \
error rather than the document that was asked for, so the error text was not saved as a file. \
Nothing was written to the shared storage area.";
const BODY_DOWNLOAD_PROMPT: &[u8] = b"This website wants to download a file to your device. InterOS \
requires your explicit permission before any file can be stored.";
const BODY_DOWNLOAD_PROMPT_PARTIAL: &[u8] = b"This website wants to download a file, but the \
transfer already ended early: only part of it arrived. If you allow it, the incomplete part is \
saved.";
const BODY_DOWNLOAD_DECLINED: &[u8] = b"The download request was declined. No data was written to storage, \
and temporary network buffers were discarded.";

/// Why a local document could not be shown.
///
/// WHY this is carried rather than inferred: the error screen used to
/// print one paragraph for every local failure, so a file that opened and
/// held no bytes was reported as an app that held no filesystem
/// capability. The screen has to state the failure that happened.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum LocalFailure {
    /// The address carried no path.
    NoPath,
    /// The app holds no filesystem read capability.
    NoCapability,
    /// The filesystem refused to open the path.
    Refused,
    /// The document opened and held no bytes.
    Empty,
    /// The document opened and the read itself was refused.
    ReadRefused,
}

/// Why a download was not saved.
///
/// WHY this is carried rather than inferred: the download screen used to
/// print one paragraph for every failed download, so a response that
/// carried no bytes at all was reported as a storage area that refused
/// the write. The screen has to state the failure that happened.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum DownloadFailure {
    /// The download was saved.
    None,
    /// The origin sent no bytes at all.
    NoBytes,
    /// The shared storage area refused the write.
    StorageRefused,
    /// No free name could be found for the file.
    NoPath,
    /// The response's bytes could not be found again.
    Lost,
    /// The origin's answer was an error rather than the file.
    OriginRefused,
}

// -----------------------------------------------------------------------------
// Network Socket Syscall Wrappers
// -----------------------------------------------------------------------------
pub fn net_socket(port: u16) -> Result<u32, i64> {
    std::net_socket(port)
}

pub fn net_connect(dst_ip: [u8; 4], dst_port: u16) -> Result<u32, i64> {
    std::tcp_connect(dst_ip, dst_port, 49152, 1)
}

pub fn net_send(sock_or_conn: u32, payload: &[u8]) -> Result<(), i64> {
    std::tcp_send(sock_or_conn, payload)
}

pub fn net_recv(sock_or_conn: u32, buf: &mut [u8]) -> Result<usize, i64> {
    std::tcp_recv(sock_or_conn, buf)
}

// -----------------------------------------------------------------------------
// Bookmark Data Models & Settings Persistence
// -----------------------------------------------------------------------------
const BM_TITLE_MAX: usize = 24;
const BM_URL_MAX: usize = 48;
const MAX_BOOKMARKS: usize = 6;
const BM_RECORD_BYTES: usize = 1 + BM_TITLE_MAX + BM_URL_MAX;
const STORE_BLOB_BYTES: usize = 2 + MAX_BOOKMARKS * BM_RECORD_BYTES;
const STORE_VERSION: u8 = 1;
const STORE_KEY: &str = "browser.bookmarks.v1";

#[derive(Clone, Copy)]
pub struct Bookmark {
    pub name: &'static str,
    pub url: &'static str,
    pub is_sovereign: bool,
}
pub static BOOKMARKS: [Bookmark; 0] = [];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BookmarkStoreState {
    Untouched,
    Empty,
    Loaded,
    ReadRefused,
    Invalid,
    Written,
    WriteRefused,
    Full,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BookmarkRecord {
    id: u8,
    title: [u8; BM_TITLE_MAX],
    url: [u8; BM_URL_MAX],
}

impl BookmarkRecord {
    const EMPTY: BookmarkRecord = BookmarkRecord {
        id: 0,
        title: [0; BM_TITLE_MAX],
        url: [0; BM_URL_MAX],
    };

    fn from_parts(id: u8, title: &[u8], url: &[u8]) -> Self {
        let mut rec = BookmarkRecord::EMPTY;
        rec.id = id;
        let n = title.len().min(BM_TITLE_MAX);
        rec.title[..n].copy_from_slice(&title[..n]);
        let m = url.len().min(BM_URL_MAX);
        rec.url[..m].copy_from_slice(&url[..m]);
        rec
    }

    fn valid(&self) -> bool {
        (1..=MAX_BOOKMARKS as u8).contains(&self.id)
            && self.title_bytes().iter().all(|b| (0x20..=0x7E).contains(b))
            && self.url_bytes().iter().all(|b| (0x20..=0x7E).contains(b))
    }

    fn title_len(&self) -> usize {
        self.title.iter().position(|&b| b == 0).unwrap_or(BM_TITLE_MAX)
    }

    fn url_len(&self) -> usize {
        self.url.iter().position(|&b| b == 0).unwrap_or(BM_URL_MAX)
    }

    fn title_bytes(&self) -> &[u8] {
        &self.title[..self.title_len()]
    }

    pub fn url_bytes(&self) -> &[u8] {
        &self.url[..self.url_len()]
    }

    fn encode(&self, out: &mut [u8]) {
        out[0] = self.id;
        out[1..1 + BM_TITLE_MAX].copy_from_slice(&self.title);
        out[1 + BM_TITLE_MAX..1 + BM_TITLE_MAX + BM_URL_MAX].copy_from_slice(&self.url);
    }

    fn decode(bytes: &[u8]) -> Option<BookmarkRecord> {
        if bytes.len() < BM_RECORD_BYTES {
            return None;
        }
        let mut rec = BookmarkRecord::EMPTY;
        rec.id = bytes[0];
        rec.title.copy_from_slice(&bytes[1..1 + BM_TITLE_MAX]);
        let url_at = 1 + BM_TITLE_MAX;
        rec.url.copy_from_slice(&bytes[url_at..url_at + BM_URL_MAX]);
        if rec.valid() {
            Some(rec)
        } else {
            None
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct BookmarkStore {
    records: [BookmarkRecord; MAX_BOOKMARKS],
    len: u8,
    unsaved: [BookmarkRecord; MAX_BOOKMARKS],
    unsaved_len: u8,
    state: BookmarkStoreState,
    next_id: u8,
}

impl BookmarkStore {
    pub const EMPTY: BookmarkStore = BookmarkStore {
        records: [BookmarkRecord::EMPTY; MAX_BOOKMARKS],
        len: 0,
        unsaved: [BookmarkRecord::EMPTY; MAX_BOOKMARKS],
        unsaved_len: 0,
        state: BookmarkStoreState::Untouched,
        next_id: 1,
    };

    pub fn load() -> Self {
        let mut store = BookmarkStore::EMPTY;
        let mut blob = [0u8; STORE_BLOB_BYTES];
        match std::settings_get(STORE_KEY, &mut blob) {
            Ok(0) => store.state = BookmarkStoreState::Empty,
            Ok(n) => {
                let mut records = [BookmarkRecord::EMPTY; MAX_BOOKMARKS];
                let mut count = 0usize;
                let mut rejected = false;
                if n > STORE_BLOB_BYTES || blob[0] != STORE_VERSION {
                    rejected = true;
                } else {
                    let header_count = blob[1] as usize;
                    if header_count > MAX_BOOKMARKS || n != 2 + header_count * BM_RECORD_BYTES {
                        rejected = true;
                    } else {
                        for i in 0..header_count {
                            let start = 2 + i * BM_RECORD_BYTES;
                            match BookmarkRecord::decode(&blob[start..start + BM_RECORD_BYTES]) {
                                Some(rec) => {
                                    records[i] = rec;
                                    count += 1;
                                }
                                None => {
                                    rejected = true;
                                    break;
                                }
                            }
                        }
                    }
                }
                if rejected {
                    store.state = BookmarkStoreState::Invalid;
                } else {
                    store.records = records;
                    store.len = count as u8;
                    store.state = BookmarkStoreState::Loaded;
                    store.next_id = (count as u8 + 1).min(MAX_BOOKMARKS as u8);
                }
            }
            Err(_) => {
                store.state = BookmarkStoreState::ReadRefused;
            }
        }
        store
    }

    pub fn persist(&mut self) -> Result<(), i64> {
        let mut blob = [0u8; STORE_BLOB_BYTES];
        blob[0] = STORE_VERSION;
        blob[1] = self.len;
        for i in 0..self.len as usize {
            let offset = 2 + i * BM_RECORD_BYTES;
            self.records[i].encode(&mut blob[offset..offset + BM_RECORD_BYTES]);
        }
        let used = 2 + self.len as usize * BM_RECORD_BYTES;
        if blob[..used].iter().any(|&b| b == b'\n') {
            self.state = BookmarkStoreState::WriteRefused;
            return Err(-22);
        }
        match std::settings_set(STORE_KEY, &blob[..used]) {
            Ok(()) => {
                self.state = BookmarkStoreState::Written;
                Ok(())
            }
            Err(code) => {
                self.state = BookmarkStoreState::WriteRefused;
                Err(code)
            }
        }
    }

    pub fn add(&mut self, title: &[u8], url: &[u8]) -> Result<(), i64> {
        if title.is_empty() || url.is_empty() {
            return Err(-22);
        }
        if self.next_id as usize > MAX_BOOKMARKS || self.next_id == 0 {
            return Err(-22);
        }
        let id = self.next_id;
        let rec = BookmarkRecord::from_parts(id, title, url);
        if !rec.valid() {
            return Err(-22);
        }
        if self.len as usize >= MAX_BOOKMARKS {
            self.state = BookmarkStoreState::Full;
            return Err(-28);
        }
        self.records[self.len as usize] = rec;
        self.len += 1;
        self.next_id += 1;
        match self.persist() {
            Ok(()) => Ok(()),
            Err(code) => {
                self.len -= 1;
                self.records[self.len as usize] = BookmarkRecord::EMPTY;
                if (self.unsaved_len as usize) < MAX_BOOKMARKS {
                    let mut held = rec;
                    held.id = id;
                    self.unsaved[self.unsaved_len as usize] = held;
                    self.unsaved_len += 1;
                }
                Err(code)
            }
        }
    }

    pub fn len(&self) -> usize {
        self.len as usize
    }

    pub fn unsaved_len(&self) -> usize {
        self.unsaved_len as usize
    }

    pub fn record(&self, index: usize) -> Option<&BookmarkRecord> {
        if index < self.len as usize {
            Some(&self.records[index])
        } else {
            None
        }
    }

    pub fn state(&self) -> BookmarkStoreState {
        self.state
    }
}

// -----------------------------------------------------------------------------
// History
// -----------------------------------------------------------------------------
const HISTORY_CAP: usize = 16;

/// The largest address a visit holds.
///
/// WHY this is not 64 bytes: real page and media addresses are longer than
/// that, and a truncated entry navigated to a different address than the one
/// that was visited.
const HISTORY_ENTRY_BYTES: usize = 256;

/// The largest name a visit holds.
///
/// WHY it is the tab's own width: the same page names itself one title, and a
/// log that kept more of it than the tab shows would list a name the person
/// never saw anywhere else.
const HISTORY_TITLE_BYTES: usize = browser::MAX_TITLE_LEN;

/// The largest summary a visit holds, from the page's own description.
///
/// WHY a hundred and twenty-eight bytes: a `<meta name="description">` is
/// written to be read in the line under a search result, and the line a card
/// gives it is one line. Longer is a paragraph the card cannot draw and every
/// visit in the file pays for.
const HISTORY_SUMMARY_BYTES: usize = 128;

/// The app-private file the visit log is kept in.
///
/// WHY a file and not a setting: a setting value is capped at 512 bytes, and
/// a log that held two addresses would not be a history.
const HISTORY_FILE: &str = "history.bin";

/// The first byte of the file names the format.
///
/// WHY two and not one: version one recorded only the address, and a record
/// from it read as version two would hand the address's bytes to the name. A
/// log in the older format is discarded whole rather than half-understood.
const HISTORY_VERSION: u8 = 2;

/// The longest field one length byte can describe.
const HISTORY_FIELD_BYTES: usize = u8::MAX as usize;

/// One version byte, one count byte, then `HISTORY_CAP` records of one length
/// byte plus that many bytes, once each for the address, the name and the
/// summary.
const HISTORY_BLOB_BYTES: usize = 2
    + HISTORY_CAP * (HISTORY_ENTRY_BYTES + HISTORY_TITLE_BYTES + HISTORY_SUMMARY_BYTES + 3);

/// The whole log, as one file read.
///
/// WHY a static and not a local: the log is larger than the task stack should
/// carry, and `BrowserApp::new()` — which reads it — is built on that stack.
static mut HISTORY_BLOB: [u8; HISTORY_BLOB_BYTES] = [0; HISTORY_BLOB_BYTES];

/// The text a fixed slot holds: everything up to its first zero.
fn slot_text(slot: &[u8]) -> &[u8] {
    let n = slot.iter().position(|&b| b == 0).unwrap_or(slot.len());
    &slot[..n]
}

/// Puts text into a fixed slot, clearing whatever the slot held before.
fn slot_put(slot: &mut [u8], text: &[u8]) {
    let n = text.len().min(slot.len()).min(HISTORY_FIELD_BYTES);
    for byte in slot.iter_mut() {
        *byte = 0;
    }
    slot[..n].copy_from_slice(&text[..n]);
}

/// Reads one length-prefixed field of a history record into a fixed slot and
/// moves the cursor past it. `None` is a field the file does not hold.
fn read_record_field(
    blob: &[u8],
    filled: usize,
    cursor: &mut usize,
    out: &mut [u8],
) -> Option<usize> {
    if *cursor >= filled {
        return None;
    }
    let n = blob[*cursor] as usize;
    *cursor += 1;
    if n > out.len() || *cursor + n > filled {
        return None;
    }
    for byte in out.iter_mut() {
        *byte = 0;
    }
    out[..n].copy_from_slice(&blob[*cursor..*cursor + n]);
    *cursor += n;
    Some(n)
}

/// Writes one length-prefixed field of a history record, in bytes used.
fn write_record_field(out: &mut [u8], text: &[u8]) -> usize {
    let n = text.len().min(out.len() - 1).min(HISTORY_FIELD_BYTES);
    out[0] = n as u8;
    out[1..1 + n].copy_from_slice(&text[..n]);
    n + 1
}

/// One recorded visit: where it was, and what the page called itself.
#[derive(Debug, Clone, Copy)]
pub struct Visit<'a> {
    pub url: &'a [u8],
    pub title: &'a [u8],
    pub description: &'a [u8],
}

#[derive(Debug, Clone, Copy)]
pub struct History {
    entries: [[u8; HISTORY_ENTRY_BYTES]; HISTORY_CAP],
    names: [[u8; HISTORY_TITLE_BYTES]; HISTORY_CAP],
    summaries: [[u8; HISTORY_SUMMARY_BYTES]; HISTORY_CAP],
    len: usize,
}

impl History {
    const EMPTY: History = History {
        entries: [[0; HISTORY_ENTRY_BYTES]; HISTORY_CAP],
        names: [[0; HISTORY_TITLE_BYTES]; HISTORY_CAP],
        summaries: [[0; HISTORY_SUMMARY_BYTES]; HISTORY_CAP],
        len: 0,
    };

    pub fn len(&self) -> usize {
        self.len
    }

    /// Appends one visit. A full log drops its oldest entry.
    pub fn push(&mut self, target: &[u8]) {
        if target.is_empty() {
            return;
        }
        // WHY a repeated address is not appended twice: reloading the page a
        // person is already on is not a new visit, and the list would fill
        // with copies of one address.
        if self.len > 0 && slot_text(&self.entries[self.len - 1]) == target {
            return;
        }
        if self.len >= HISTORY_CAP {
            for i in 0..self.len - 1 {
                self.entries[i] = self.entries[i + 1];
                self.names[i] = self.names[i + 1];
                self.summaries[i] = self.summaries[i + 1];
            }
            self.len -= 1;
        }
        slot_put(&mut self.entries[self.len], target);
        // WHY the name is cleared with the address: the slot this visit lands
        // in held an earlier page, and a visit wearing another page's name
        // records a visit nobody made.
        self.names[self.len] = [0; HISTORY_TITLE_BYTES];
        self.summaries[self.len] = [0; HISTORY_SUMMARY_BYTES];
        self.len += 1;
    }

    /// Gives the newest visit the name and summary the page stated for itself.
    ///
    /// WHY the newest entry and not a search by address: the page being read
    /// is the visit just recorded, and a page reached through a redirect or a
    /// canonical address names itself over bytes stored under a different
    /// address. Searching by address would leave those pages unnamed.
    ///
    /// Returns whether the log changed, so the caller rewrites the file only
    /// when a name actually arrived.
    pub fn name_newest(&mut self, title: &[u8], description: &[u8]) -> bool {
        if self.len == 0 {
            return false;
        }
        let last = self.len - 1;
        if slot_text(&self.names[last]) == title
            && slot_text(&self.summaries[last]) == description
        {
            return false;
        }
        slot_put(&mut self.names[last], title);
        slot_put(&mut self.summaries[last], description);
        true
    }

    pub fn visit(&self, index: usize) -> Option<Visit<'_>> {
        if index >= self.len {
            return None;
        }
        let url = slot_text(&self.entries[index]);
        if url.is_empty() {
            return None;
        }
        Some(Visit {
            url,
            title: slot_text(&self.names[index]),
            description: slot_text(&self.summaries[index]),
        })
    }

    /// Reads the log the previous session saved.
    ///
    /// An absent file is a first run, not a failure. A file that does not
    /// match the format is discarded whole, because a log whose count and
    /// entries disagree cannot be read honestly in part.
    pub fn load() -> Self {
        let mut history = Self::EMPTY;
        let Ok(fd) = std::app_storage_open(HISTORY_FILE, 1) else {
            return history;
        };
        let blob = unsafe { &mut *core::ptr::addr_of_mut!(HISTORY_BLOB) };
        let mut filled = 0;
        loop {
            match std::fs_read(fd, &mut blob[filled..]) {
                Ok(0) => break,
                Ok(n) => {
                    filled += n;
                    if filled == blob.len() {
                        break;
                    }
                }
                Err(_) => {
                    let _ = std::fs_close(fd);
                    return Self::EMPTY;
                }
            }
        }
        let _ = std::fs_close(fd);
        if filled < 2 || blob[0] != HISTORY_VERSION {
            return Self::EMPTY;
        }
        let count = blob[1] as usize;
        if count > HISTORY_CAP {
            return Self::EMPTY;
        }
        let mut offset = 2;
        for i in 0..count {
            // WHY a name of none is allowed and an address of none is not: a
            // page that stated no title is an ordinary page, and the card
            // gives it its address. A visit with no address says nothing
            // happened, and a file that says that is not a log.
            let address = read_record_field(blob, filled, &mut offset, &mut history.entries[i]);
            let name = read_record_field(blob, filled, &mut offset, &mut history.names[i]);
            let summary = read_record_field(blob, filled, &mut offset, &mut history.summaries[i]);
            if !matches!(address, Some(length) if length > 0)
                || name.is_none()
                || summary.is_none()
            {
                return Self::EMPTY;
            }
        }
        history.len = count;
        history
    }

    /// Saves the log to app-private storage.
    ///
    /// Returns whether the platform accepted the write, so the caller can
    /// tell a saved history from a session-only one.
    pub fn persist(&self) -> bool {
        let blob = unsafe { &mut *core::ptr::addr_of_mut!(HISTORY_BLOB) };
        blob[0] = HISTORY_VERSION;
        blob[1] = self.len as u8;
        let mut offset = 2;
        for i in 0..self.len {
            offset += write_record_field(&mut blob[offset..], slot_text(&self.entries[i]));
            offset += write_record_field(&mut blob[offset..], slot_text(&self.names[i]));
            offset += write_record_field(&mut blob[offset..], slot_text(&self.summaries[i]));
        }
        let Ok(fd) = std::app_storage_open(HISTORY_FILE, 2) else {
            return false;
        };
        let mut written = 0;
        while written < offset {
            match std::fs_write(fd, &blob[written..offset]) {
                Ok(0) => {
                    let _ = std::fs_close(fd);
                    return false;
                }
                Ok(n) => written += n,
                Err(_) => {
                    let _ = std::fs_close(fd);
                    return false;
                }
            }
        }
        let _ = std::fs_close(fd);
        true
    }
}

// -----------------------------------------------------------------------------
// Sheet Overlays
// -----------------------------------------------------------------------------
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SheetOverlay {
    None,
    TabSwitcher,
    Bookmarks,
    History,
    Downloads,
    Menu,
    Tools,
    Settings,
    UploadPicker,
    SpeedTest,
    CompatibilityLab,
    TestFiles,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BookmarkStep {
    Idle,
    Title,
    Address,
}

/// What happened when the downloaded file was handed to the platform.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpenState {
    NotAttempted,
    Dispatched,
    NoHandler,
    Refused,
}

/// The work the next event-loop pass performs.
///
/// WHY the work is deferred by one pass: a sheet can show that an upload
/// or a test is running only if the frame is presented before the
/// transfer begins. The tap schedules the work, the loop draws the
/// running state, and then the transfer runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PendingTask {
    None,
    Upload(usize),
    SpeedTest,
    Lab,
}

/// How a transfer ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TransferState {
    Idle,
    Running,
    Complete,
    Refused,
    Failed,
}

/// A document the upload picker offers.
#[derive(Clone, Copy)]
struct PickEntry {
    path: [u8; 96],
    path_len: usize,
    mime: [u8; 40],
    mime_len: usize,
    length: u64,
}

impl PickEntry {
    const EMPTY: PickEntry = PickEntry {
        path: [0; 96],
        path_len: 0,
        mime: [0; 40],
        mime_len: 0,
        length: 0,
    };
}

/// The numbers one speed-test run measured.
#[derive(Clone, Copy)]
struct SpeedResults {
    download: TransferState,
    download_bytes: usize,
    download_ms: u64,
    download_bps: u64,
    upload: TransferState,
    upload_bytes: usize,
    upload_confirmed: u64,
    upload_ms: u64,
    upload_bps: u64,
    latency: TransferState,
    latency_ms: u64,
    error: i64,
}

impl SpeedResults {
    const EMPTY: SpeedResults = SpeedResults {
        download: TransferState::Idle,
        download_bytes: 0,
        download_ms: 0,
        download_bps: 0,
        upload: TransferState::Idle,
        upload_bytes: 0,
        upload_confirmed: 0,
        upload_ms: 0,
        upload_bps: 0,
        latency: TransferState::Idle,
        latency_ms: 0,
        error: 0,
    };
}

/// One address the compatibility lab checked.
#[derive(Clone, Copy)]
struct LabResult {
    state: TransferState,
    status: u16,
    bytes: usize,
    content_type: [u8; 40],
    content_type_len: usize,
    reader_lines: usize,
    error: i64,
}

impl LabResult {
    const EMPTY: LabResult = LabResult {
        state: TransferState::Idle,
        status: 0,
        bytes: 0,
        content_type: [0; 40],
        content_type_len: 0,
        reader_lines: 0,
        error: 0,
    };
}

/// The addresses the compatibility lab fetches.
///
/// WHY this fixed list: the lab reports what this browser actually did
/// with the sites the device is asked to reach, and a list the person can
/// read is more useful than an address bar that silently replaced their
/// last page. The lab runs no JavaScript and saves nothing.
const LAB_URLS: [&[u8]; 4] = [
    b"https://www.google.com/",
    b"https://m.youtube.com/",
    b"https://interos.tech/",
    b"https://www.speedtest.net/",
];

/// The gateway's built-in media samples, by name and declared type.
///
/// WHY this fixed list: the proxy refuses loopback addresses, and the one
/// local URL it serves is the gateway's own allowlisted fixture route.
/// Naming the fixtures here keeps the app from offering an address the
/// proxy would refuse.
const TEST_FIXTURES: [(&[u8], &[u8]); 11] = [
    (b"sample.png", b"image/png"),
    (b"sample.jpg", b"image/jpeg"),
    (b"sample.gif", b"image/gif"),
    (b"sample.bmp", b"image/bmp"),
    (b"sample.ppm", b"image/x-portable-pixmap"),
    (b"sample.qoi", b"image/qoi"),
    (b"sample.wav", b"audio/wav"),
    (b"sample.mp3", b"audio/mpeg"),
    (b"sample.mjpeg", b"video/x-motion-jpeg"),
    (b"sample.mp4", b"video/mp4"),
    (b"sample.raw", b"video/x-raw"),
];

const FIXTURE_URL_PREFIX: &[u8] = b"http://127.0.0.1:4300/api/media/fixture/";

// -----------------------------------------------------------------------------
// Browser Application State
// -----------------------------------------------------------------------------
/// One inline image the page placed.
///
/// The bitmap borrows the static arena; a refused image keeps only the
/// typed reason and the alt text, and the renderer draws a placeholder
/// that says why rather than a broken picture.
#[derive(Clone, Copy)]
pub struct PlacedImage {
    pub bitmap: Option<std::image::Bitmap<'static>>,
    pub status: images::ImageStatus,
    pub display_w: usize,
    pub display_h: usize,
    pub alt: [u8; 64],
    pub alt_len: usize,
    pub margin_top: u8,
    pub padding_left: u8,
}

impl PlacedImage {
    const EMPTY: PlacedImage = PlacedImage {
        bitmap: None,
        status: images::ImageStatus::Refused(images::ImageRefusal::NoSource),
        display_w: browser::CONTENT_WIDTH,
        display_h: images::PLACEHOLDER_HEIGHT,
        alt: [0; 64],
        alt_len: 0,
        margin_top: 0,
        padding_left: 0,
    };
}

pub struct BrowserApp {
    /// The address of the document on screen.
    ///
    /// WHY this is longer than 64 bytes: real addresses are, and a truncated
    /// address made Reload, Back and Forward fetch a different resource than
    /// the one on screen.
    pub current_url: [u8; 256],
    pub current_url_len: usize,
    /// How far the reader has been scrolled, and how far it can be.
    ///
    /// WHY the platform's scroll model and not a bare offset: the reader
    /// used to carry a pixel offset that nothing clamped, so a long page
    /// could be dragged past its end and the scrollbar thumb could leave
    /// its track. The scroll state knows the document's height and the
    /// height of the band it is drawn in, so the offset is always inside
    /// the document.
    pub scroll: ScrollState,
    pub pages_loaded: usize,
    pub trackers_blocked: usize,
    pub active_socket_id: Option<u32>,
    pub address: [u8; 256],
    pub address_len: usize,
    pub address_focused: bool,
    pub select_all: bool,
    pub history: History,
    pub bookmarks: BookmarkStore,
    pub bm_step: BookmarkStep,
    pub bm_title: [u8; BM_TITLE_MAX],
    pub bm_title_len: usize,
    pub status: PageStatus,
    pub loading_progress: u8,
    pub error_code: i32,
    pub tabs: TabSessionManager,
    pub overlay: SheetOverlay,
    pub doc_line_count: usize,
    /// Whether the page is drawn by the HTML layout engine rather than the
    /// reader.
    ///
    /// WHY the app keeps this: the two views scroll in the same band but
    /// have different coordinate systems, so the renderer and the tap
    /// handler both have to know which one owns the content.
    pub html_view: bool,
    /// The form control the keyboard is editing, or [`NO_FIELD`].
    pub field_focus: u8,
    /// How many bytes of [`FORM_BODY`] the next navigation sends.
    ///
    /// WHY it is cleared when read: a POST belongs to one navigation, and
    /// a later navigation that carried the previous body would be sending
    /// a form the person did not submit.
    form_post_len: usize,
    /// The inline images this page placed, in document order.
    pub images: [PlacedImage; images::MAX_INLINE_IMAGES],
    pub image_count: usize,
    /// How much of the static image arena the current page owns.
    image_arena_used: usize,
    /// The picture the current page asked to be known by, already reduced to
    /// the size the address field draws it at.
    ///
    /// WHY it is held beside the page rather than re-fetched per frame: a
    /// navigation is the only moment its address is known, and the pixels
    /// belong to that page. WHY it can be `None` on a page that names an
    /// icon: the picture a page prefers to be known by is often drawn from
    /// paths rather than pixels, and this platform has no vector rasteriser;
    /// the honest answer to a picture that cannot be drawn is the mark drawn
    /// before this one existed.
    favicon: Option<[[u8; 4]; FAVICON_FACE_PIXELS]>,
    /// How many lines the document has, before the reader's bound.
    ///
    /// WHY it is counted: the reader holds a bounded number of lines, and
    /// a page that is longer than that is an excerpt. The screen says
    /// which, rather than presenting a fragment as the whole article.
    ///
    /// WHY it counts the wrapped lines: the reader draws the wrapped
    /// lines, so the total has to be counted in the same unit.
    pub doc_total_lines: usize,
    pub kb_shift: bool,
    pub kb_symbols: bool,
    /// Whether the last response was larger than the browser's buffer.
    ///
    /// WHY this is remembered: a response that overflows the buffer is the
    /// beginning of a page, not the whole of it, and the screen has to say
    /// so rather than present a fragment as the article.
    pub response_truncated: bool,
    /// Why the last local document could not be shown.
    pub local_failure: LocalFailure,
    /// Why the last download was not saved.
    ///
    /// WHY it is kept: a response that carried no bytes and a storage area
    /// that refused the bytes are different failures, and the screen used
    /// to print the storage refusal for both.
    pub download_failure: DownloadFailure,
    /// Why network transport was unavailable for navigation.
    pub transport_failure: TransportFailure,
    /// Why connection to the gateway failed.
    pub connection_failure: ConnectionFailure,
    /// Whether the document on screen arrived over a TLS session whose
    /// handshake completed.
    ///
    /// WHY it is kept: the secure mark in the omnibox is a claim about this
    /// page's transport. Deriving it from the scheme alone would also mark a
    /// page whose request went out as plaintext, and that is the one thing the
    /// mark must never do.
    pub document_tls_verified: bool,
    /// Why protocol handling or document decoding failed.
    pub protocol_fault: ProtocolFaultKind,
    /// The reason the gateway gave for the last failed fetch.
    ///
    /// WHY it is kept: the gateway answers a failed fetch with an envelope
    /// that names the reason, and the screen used to throw that reason
    /// away and print a generic failure instead. The reason is the host's
    /// own account of what happened, and it is both more useful and more
    /// honest than a message the browser made up.
    ///
    /// WHY it is empty when the gateway named no reason: the screen then
    /// shows the generic failure, which is true. Nothing is put in the
    /// gateway's mouth.
    pub gateway_reason: [u8; browser::GATEWAY_REASON_MAX],
    /// How many bytes of the gateway's reason are held.
    pub gateway_reason_len: usize,
    /// How many bytes of the last response the transport delivered.
    ///
    /// WHY it is kept: a download is the bytes the transport delivered, and
    /// the app has to know how many there were to write them.
    pub response_len: usize,
    /// How many bytes of the page's own document the last response held.
    ///
    /// WHY this is not `response_len`: the gateway wraps a page in a JSON
    /// envelope, so the response is the envelope and the document inside it
    /// is what a saved page has to be. Saving the envelope wrote a file that
    /// reopened as JSON rather than as the page.
    pub page_source_len: usize,
    /// How many pages this app has saved into the shared storage area.
    ///
    /// WHY it is counted: a saved page is named by a counter rather than by a
    /// clock reading, so two saves of the same page do not overwrite one
    /// another and the name does not depend on a reading the device may not
    /// have.
    pub downloads_saved: u32,
    /// The pages the browser saved, as the platform's index holds them.
    ///
    /// WHY they come from the index: the index is the platform's record of
    /// what is in the shared storage area, so the list survives the app
    /// being closed and agrees with what the file manager shows.
    pub downloads: [DownloadEntry; MAX_DOWNLOADS],
    /// How many of the saved pages and downloaded files the list holds.
    pub download_count: usize,
    /// Whether the last tap on a downloaded file found no handler.
    ///
    /// WHY the sheet keeps its own answer: a file the platform cannot open
    /// has to say so where the person tapped it, not in a log.
    pub downloads_error: bool,
    /// The text the open list is filtered by.
    ///
    /// WHY the panel has its own filter and not the address bar: the
    /// address bar is the place an address is typed, and a list that
    /// borrowed it would put a page's address and a search term in the
    /// same field.
    pub filter: [u8; FILTER_MAX],
    pub filter_len: usize,
    /// The file the last download wrote, and the type it was saved as.
    ///
    /// WHY the pair is kept: opening a download is a question about the
    /// file that was just written, and the platform resolves the handler
    /// from the file's own media type. A path without its type could only
    /// be opened by guessing.
    last_download_path: [u8; 96],
    last_download_path_len: usize,
    last_download_mime: [u8; 48],
    last_download_mime_len: usize,
    /// Whether the file the last download wrote is only part of the origin's file.
    ///
    /// WHY the flag is kept: the completion screen used to report every saved
    /// file as complete, so a transfer that ended early was presented as the
    /// whole file and the person only found out when it failed to open.
    last_download_truncated: bool,
    open_state: OpenState,
    /// The documents the upload picker offers, as the index holds them.
    uploads: [PickEntry; MAX_PICK_FILES],
    upload_count: usize,
    upload_state: TransferState,
    upload_sent: usize,
    upload_confirmed: u64,
    upload_error: i64,
    upload_name: [u8; 128],
    upload_name_len: usize,
    speed: SpeedResults,
    speed_running: bool,
    lab: [LabResult; LAB_URLS.len()],
    lab_running: bool,
    pending: PendingTask,
    /// A download awaiting explicit user confirmation.
    pub pending_download: browser::PendingDownload,
    /// The local port the next outbound connection asks for.
    ///
    /// WHY a counter: two connections from the same port in quick
    /// succession can collide with the previous connection's state, and
    /// the browser opens a connection for every upload and every test.
    next_local_port: u16,
}

/// A page the browser saved or a file it downloaded.
#[derive(Clone, Copy)]
pub struct DownloadEntry {
    /// The path of the file in the shared storage area.
    pub path: [u8; 96],
    pub path_len: usize,
    /// The size of the file in bytes.
    pub length: u64,
    /// The media type the platform's index gives the file.
    ///
    /// WHY the type travels with the entry: opening a downloaded file asks
    /// the platform for the handler registered for its type, and a list
    /// that held only paths could only open a page.
    pub mime: [u8; 48],
    pub mime_len: usize,
    /// Whether the entry is a page the browser saved rather than a file it downloaded.
    pub is_page: bool,
}

impl DownloadEntry {
    const EMPTY: DownloadEntry = DownloadEntry {
        path: [0; 96],
        path_len: 0,
        length: 0,
        mime: [0; 48],
        mime_len: 0,
        is_page: false,
    };
}

/// The largest slice one filesystem write carries.
///
/// WHY this is smaller than the syscall's limit: the payload is copied
/// through a fixed kernel buffer, so steps below the limit leave room for
/// the copy and keep the last step from depending on the remainder.
const FILE_WRITE_CHUNK: usize = 32 * 1024;

/// Writes `bytes` to `path` in buffered steps.
///
/// WHY a partial write is retried rather than reported: a short write is
/// not a failure, and the next step continues where the last one stopped.
/// A file that cannot be finished is removed, so a damaged download never
/// looks like a complete one.
///
/// WHY an empty body is refused before the file is opened: opening a file
/// creates it, so a write of no bytes would leave an empty document in the
/// shared storage area that the File Manager and the Gallery list as a
/// real file. A download that wrote nothing is not a download.
fn write_file(path: &str, bytes: &[u8]) -> Result<usize, ()> {
    if bytes.is_empty() {
        return Err(());
    }
    let fd = std::fs_open(path, std::FS_WRITE).map_err(|_| ())?;
    let mut written = 0usize;
    while written < bytes.len() {
        let end = (written + FILE_WRITE_CHUNK).min(bytes.len());
        match std::fs_write(fd, &bytes[written..end]) {
            Ok(0) => {
                let _ = std::fs_close(fd);
                let _ = std::fs_unlink(path);
                return Err(());
            }
            Ok(n) => written += n,
            Err(_) => {
                let _ = std::fs_close(fd);
                let _ = std::fs_unlink(path);
                return Err(());
            }
        }
    }
    let _ = std::fs_close(fd);
    Ok(written)
}

impl BrowserApp {
    pub fn new() -> Self {
        let mut app = Self {
            current_url: [0; 256],
            current_url_len: 0,
            scroll: ScrollState::new(0, READER_VIEWPORT_H, 0),
            pages_loaded: 0,
            trackers_blocked: 0,
            active_socket_id: None,
            address: [0; 256],
            address_len: 0,
            address_focused: false,
            select_all: false,
            history: History::load(),
            bookmarks: BookmarkStore::load(),
            bm_step: BookmarkStep::Idle,
            bm_title: [0; BM_TITLE_MAX],
            bm_title_len: 0,
            status: PageStatus::Blank,
            loading_progress: 0,
            error_code: 0,
            tabs: TabSessionManager::new(),
            overlay: SheetOverlay::None,
            doc_line_count: 0,
            html_view: false,
            field_focus: NO_FIELD,
            form_post_len: 0,
            images: [PlacedImage::EMPTY; images::MAX_INLINE_IMAGES],
            image_count: 0,
            image_arena_used: 0,
            favicon: None,
            doc_total_lines: 0,
            kb_shift: false,
            kb_symbols: false,
            response_truncated: false,
            local_failure: LocalFailure::NoPath,
            download_failure: DownloadFailure::None,
            transport_failure: TransportFailure::None,
            connection_failure: ConnectionFailure::None,
            document_tls_verified: false,
            protocol_fault: ProtocolFaultKind::None,
            gateway_reason: [0; browser::GATEWAY_REASON_MAX],
            gateway_reason_len: 0,
            response_len: 0,
            page_source_len: 0,
            downloads_saved: 0,
            downloads: [DownloadEntry::EMPTY; MAX_DOWNLOADS],
            download_count: 0,
            downloads_error: false,
            filter: [0; FILTER_MAX],
            filter_len: 0,
            last_download_path: [0; 96],
            last_download_path_len: 0,
            last_download_mime: [0; 48],
            last_download_mime_len: 0,
            last_download_truncated: false,
            open_state: OpenState::NotAttempted,
            uploads: [PickEntry::EMPTY; MAX_PICK_FILES],
            upload_count: 0,
            upload_state: TransferState::Idle,
            upload_sent: 0,
            upload_confirmed: 0,
            upload_error: 0,
            upload_name: [0; 128],
            upload_name_len: 0,
            speed: SpeedResults::EMPTY,
            speed_running: false,
            lab: [LabResult::EMPTY; LAB_URLS.len()],
            lab_running: false,
            pending: PendingTask::None,
            pending_download: browser::PendingDownload::EMPTY,
            next_local_port: 45000,
        };
        app.tabs.init_default();
        let start = b"about:blank";
        app.current_url[..start.len()].copy_from_slice(start);
        app.current_url_len = start.len();
        // WHY the list is read at start-up: the pages the browser saved
        // are on the shared storage area whether or not this app is the
        // one that saved them, and the index is the platform's record of
        // them.
        app.refresh_downloads();
        app.scan_downloads();
        app
    }

    fn current_url(&self) -> &[u8] {
        &self.current_url[..self.current_url_len]
    }

    /// The text the open list is filtered by.
    pub fn filter_bytes(&self) -> &[u8] {
        &self.filter[..self.filter_len]
    }

    /// Adds a character to the list filter.
    ///
    /// WHY the filter is bounded: the panel is a fixed column of text, and
    /// a filter longer than the column would be a filter the person
    /// cannot read.
    pub fn filter_push(&mut self, c: u8) {
        if self.filter_len < FILTER_MAX {
            self.filter[self.filter_len] = c;
            self.filter_len += 1;
        }
    }

    /// Removes the last character from the list filter.
    pub fn filter_backspace(&mut self) {
        self.filter_len = self.filter_len.saturating_sub(1);
    }

    /// Clears the list filter.
    pub fn filter_clear(&mut self) {
        self.filter_len = 0;
    }

    /// Whether the open sheet is a list the filter applies to.
    ///
    /// WHY only the lists: a tab strip and a menu are not lists of things
    /// a person would look for, and a filter over them would be a field
    /// that could not narrow anything.
    pub fn filterable(&self) -> bool {
        matches!(self.overlay, SheetOverlay::Bookmarks | SheetOverlay::History)
    }

    /// Reads a local document from the guest filesystem and renders it.
    ///
    /// WHY the same pipeline as a fetched page: a local file and a
    /// downloaded page are both documents, and a reader that treated them
    /// differently would be two readers.
    fn load_local_document(&mut self, path: &[u8]) -> bool {
        if path.is_empty() {
            self.status = PageStatus::LocalRefused;
            self.local_failure = LocalFailure::NoPath;
            self.tabs.active_tab_mut().loading_state = LoadingState::Failed;
            return false;
        }
        let Ok(path_str) = core::str::from_utf8(path) else {
            self.status = PageStatus::LocalRefused;
            self.local_failure = LocalFailure::NoPath;
            self.tabs.active_tab_mut().loading_state = LoadingState::Failed;
            self.tabs.active_tab_mut().error_code = -3; // EINVAL
            return false;
        };

        // WHY the capability is asked for by its type and not by a slot: a
        // slot is a position in this task's capability table, and the
        // position depends on the order the kernel inserted the
        // capabilities in. The kernel authorises the open itself by
        // capability type, so all the browser has to know is whether it
        // holds a filesystem read authority at all.
        //
        // WHY the platform is asked rather than the answer assumed: the
        // local branch used to report that the app held no filesystem
        // capability without asking, so a refusal the browser had invented
        // was shown as a refusal the device had given. The question is
        // asked here, and only this answer is reported.
        if std::cap_lookup(std::cap_type::FILE_SYSTEM_READ, 0).is_err() {
            std::log("[BROWSER APP] This app holds no filesystem read capability, so a local document cannot be opened.")
                .ok();
            self.status = PageStatus::LocalRefused;
            self.local_failure = LocalFailure::NoCapability;
            self.tabs.active_tab_mut().loading_state = LoadingState::Failed;
            self.tabs.active_tab_mut().error_code = -1; // EPERM
            let _ = std::audio_haptic_click();
            return false;
        }

        let fd = match std::fs_open(path_str, std::FS_READ) {
            Ok(fd) => fd,
            Err(code) => {
                std::log("[BROWSER APP] The guest filesystem refused to open the local document.")
                    .ok();
                self.status = PageStatus::LocalRefused;
                self.local_failure = LocalFailure::Refused;
                self.tabs.active_tab_mut().loading_state = LoadingState::Failed;
                self.tabs.active_tab_mut().error_code = code as i32;
                let _ = std::audio_haptic_click();
                return false;
            }
        };

        // WHY the read's own result is kept: a refused read and an empty
        // file are different failures, and discarding the error reported
        // every one of them as an empty file.
        let read_result = {
            let buffer = unsafe { &mut *core::ptr::addr_of_mut!(FILE_BUF) };
            let result = std::fs_read(fd, buffer);
            let _ = std::fs_close(fd);
            result
        };
        let read_bytes = match read_result {
            Ok(n) => n,
            Err(code) => {
                std::log("[BROWSER APP] The guest filesystem refused to read the local document.")
                    .ok();
                self.status = PageStatus::LocalRefused;
                self.local_failure = LocalFailure::ReadRefused;
                self.tabs.active_tab_mut().loading_state = LoadingState::Failed;
                self.tabs.active_tab_mut().error_code = code as i32;
                let _ = std::audio_haptic_click();
                return false;
            }
        };
        if read_bytes == 0 {
            std::log("[BROWSER APP] The local document is empty.").ok();
            self.status = PageStatus::LocalRefused;
            self.local_failure = LocalFailure::Empty;
            self.tabs.active_tab_mut().loading_state = LoadingState::Failed;
            self.tabs.active_tab_mut().error_code = 0;
            let _ = std::audio_haptic_click();
            return false;
        }

        // The file's own bytes are what a saved copy holds, and they are
        // kept where the save path reads from, so a local document can be
        // saved like a page.
        {
            let source = unsafe { &*core::ptr::addr_of!(FILE_BUF) };
            let page = unsafe { &mut *core::ptr::addr_of_mut!(HTML_BUF) };
            let take = read_bytes.min(page.len());
            page[..take].copy_from_slice(&source[..take]);
            self.page_source_len = take;
        }

        // The kind of document decides how it is read. A file whose name
        // says markup is read as markup; anything else is text, and a text
        // file keeps its lines.
        let is_markup = path_str.ends_with(".html") || path_str.ends_with(".htm");
        let kind = if is_markup {
            browser::PayloadKind::Markup
        } else {
            browser::PayloadKind::PlainText
        };

        // Markup goes through the layout engine so its blocks, tables,
        // forms and inline images are placed; a document with nothing to
        // place falls back to the reader, which always holds its text.
        let mut loaded = false;
        if kind == browser::PayloadKind::Markup {
            let source = unsafe { &*core::ptr::addr_of!(FILE_BUF) };
            loaded = self.load_html_document(&source[..read_bytes]);
        }
        if !loaded {
            let doc_len = {
                let source = unsafe { &*core::ptr::addr_of!(FILE_BUF) };
                let destination = unsafe { &mut *core::ptr::addr_of_mut!(READER_BUF) };
                browser::html_to_reader_document_kind(&source[..read_bytes], kind, destination)
            };
            if doc_len == 0 {
                self.status = PageStatus::LocalRefused;
                self.local_failure = LocalFailure::Empty;
                self.tabs.active_tab_mut().loading_state = LoadingState::Failed;
                return false;
            }
            {
                let document = unsafe { &*core::ptr::addr_of!(READER_BUF) };
                self.load_document_content(&document[..doc_len]);
            }
        }

        self.status = PageStatus::Loaded;
        self.pages_loaded += 1;
        self.loading_progress = 100;
        self.response_truncated = false;
        self.tabs.active_tab_mut().loading_state = LoadingState::Completed;
        let _ = std::audio_haptic_click();
        true
    }

    /// Fetches the stylesheets the page links and adds their rules to the
    /// ones the page carried between its own `<style>` tags.
    ///
    /// WHY before the layout: a page's colours, its type sizes and which of
    /// its elements are hidden usually live in those files, so a page laid out
    /// without them is the same words in a different book. This is the one
    /// subresource that changes where every other box goes, so it is fetched
    /// before the column is measured rather than corrected after.
    ///
    /// WHY the chain and not just the first link: a bundled site keeps its
    /// design tokens in one file and its rules in another, and the one that
    /// makes the page look like itself is often behind a dozen files that
    /// carry colour names for a script to read. The walk therefore runs to
    /// the end of what the page links, and stops at the two budgets that
    /// actually bind: the rules the reader can hold, and the style text
    /// those rules need.
    fn fetch_linked_stylesheets(&mut self, dom: &mut layout::Dom) {
        // What the document is made of is settled before a single file is
        // asked for: each one is cut down to the rules that can reach an
        // element of this page, and a rule dropped on the way in is gone.
        dom.collect_names();
        // The page's own `<style>` blocks are already in the arena, so the
        // room left for the files it links is what is left of the rule budget.
        let mut held = css::parse_stylesheet(dom.css(), dom.token_store()).len;
        for index in 0..dom.stylesheets().len() {
            if held >= css::MAX_RULES || dom.css().len() == layout::dom::MAX_CSS_BYTES {
                break;
            }
            let declared = dom.stylesheets()[index];
            held += self.fetch_stylesheet(dom, declared, css::MAX_RULES - held);
        }
    }

    /// Fetches one linked stylesheet and keeps the rules of it this engine can
    /// apply, in as few bytes as those rules need.
    ///
    /// WHY the file is cut down on the way into the arena: a page's colour
    /// tokens, its icon paths and its animation curves are ordinary CSS that
    /// this reader paints nothing from, and a site that ships them as separate
    /// files fills the arena with text before the file holding the page's own
    /// type is reached. What cannot change a pixel is not kept, so that what
    /// can has somewhere to live.
    ///
    /// WHY a reply that stops short is still read: the gateway holds a body as
    /// far as its own buffer, and a stylesheet longer than that has its front
    /// on the wire. The front is where a bundled file keeps the page's own
    /// colours, so it is unescaped and parsed rather than refused.
    ///
    /// Returns how many rules the file contributed.
    fn fetch_stylesheet(
        &mut self,
        dom: &mut layout::Dom,
        declared: layout::StrRef,
        room: usize,
    ) -> usize {
        if declared.is_empty() || room == 0 {
            return 0;
        }
        // The same rule the pictures are held to, and for the same reason: a
        // stylesheet carried in plaintext under a verified document is bytes a
        // middlebox may choose, and the page is being shown as though it was
        // not. This transport passes `false` for TLS, so the file would go out
        // in the open whatever its address says.
        if images::mixed_content_block(self.document_tls_verified, true) {
            self.log_stylesheet_blocked_as_insecure();
            return 0;
        }
        // Copied out of the document before anything is fetched: the reply
        // lands in the same transport buffer the page's own bytes came
        // through, and the address is read out of the text arena that buffer
        // feeds.
        let raw = declared.get(&*dom);
        let mut href = [0u8; images::MAX_SRC_LEN + 1];
        let len = raw.len().min(href.len());
        href[..len].copy_from_slice(&raw[..len]);
        let mut url = [0u8; images::MAX_SRC_LEN];
        let Ok(len) = images::resolve_subresource(self.current_url(), &href[..len], &mut url)
        else {
            return 0;
        };
        let mut request = [0u8; 1024];
        let request_len = browser::format_proxy_request(&url[..len], &mut request);
        if request_len + 128 > request.len() {
            return 0;
        }
        let local_port = self.take_local_port();
        let outcome = fetch_over_tcp(
            [10, 0, 2, 2],
            4300,
            local_port,
            &request[..request_len],
            STYLESHEET_WINDOW_BYTES,
            false,
        );
        let buffer = unsafe { &*core::ptr::addr_of!(RESPONSE_BUF) };
        let reply = &buffer[..outcome.bytes];
        if outcome.bytes == 0 {
            self.log_stylesheet_failed(-(outcome.error as i32));
            return 0;
        }
        let Some(response) = browser::parse_http_response(reply) else {
            self.log_stylesheet_failed(-1);
            return 0;
        };
        // The gateway answers a failed upstream fetch with an HTTP 200 whose
        // body is its own JSON envelope. That is a refusal, not a stylesheet.
        if response.status_code != 200 || browser::proxy_reports_failure(reply) {
            self.log_stylesheet_failed(response.status_code as i32);
            return 0;
        }
        // A stylesheet that came through the gateway arrives as the text
        // inside that envelope, with its newlines and quotes still escaped
        // for JSON. Unescaped is what the rule parser reads. The style buffer
        // is the window this browser can read, so a file longer than it is
        // taken as far as the window goes.
        let style = unsafe { &mut *core::ptr::addr_of_mut!(STYLE_BUF) };
        let text = if browser::is_browser_envelope(reply) {
            let Some(escaped) = browser::http::proxy_string_field_prefix(reply, b"html") else {
                self.log_stylesheet_failed(-2);
                return 0;
            };
            let (kept, _) = browser::http::unescape_json_string_prefix(escaped, style);
            if kept == 0 {
                self.log_stylesheet_failed(-2);
                return 0;
            }
            kept
        } else {
            let len = response.body.len().min(style.len());
            style[..len].copy_from_slice(&response.body[..len]);
            len
        };
        // The file's `--token` definitions are read before anything is cut
        // away: a rules file that asks for `var(--color-accent-fg)` is only
        // legible once the theme file that names it has been heard, and the
        // theme file's own rules are exactly the kind this compaction drops.
        dom.stow_tokens_from(&style[..text]);
        let (carries_bytes, carries_rules) = css::keep_readable_rules(
            &mut style[..text],
            room,
            dom.token_store(),
            dom.name_set(),
        );
        if carries_bytes == 0 {
            // Every selector in the file is one this engine skips. Its text is
            // not kept, because keeping it would leave the page looking the
            // way it looked before the file was fetched at all.
            std::log("[BROWSER APP] Linked stylesheet carries no rule this browser reads; its bytes are not held.")
                .ok();
            return 0;
        }
        let kept = dom.append_css(&style[..carries_bytes]);
        let mut log_buf = [0u8; 96];
        let prefix = b"[BROWSER APP] Linked stylesheet applied bytes=";
        let plen = prefix.len();
        log_buf[..plen].copy_from_slice(prefix);
        let mut lp = browser::append_decimal(&mut log_buf, plen, kept as u64);
        let suffix = b" rules=";
        log_buf[lp..lp + suffix.len()].copy_from_slice(suffix);
        lp += suffix.len();
        lp = browser::append_decimal(&mut log_buf, lp, carries_rules as u64);
        if let Ok(line) = core::str::from_utf8(&log_buf[..lp]) {
            std::log(line).ok();
        }
        carries_rules
    }

    /// Fetches the picture a page asks to be known by, keeping the first of
    /// its offers this browser can draw.
    ///
    /// WHY the offers in the page's own order: it knows which of its pictures
    /// it likes best, and the reader only steps over the ones it was built
    /// without a decoder for. WHY the walk can end with nothing: an icon is
    /// the one subresource a page is allowed to fail to have, and the mark
    /// drawn here before favicons existed is what the address bar shows then.
    fn fetch_favicon(&mut self, dom: &layout::Dom) {
        self.favicon = None;
        let offers = dom.icons();
        let mut tried = 0usize;
        let mut last_refusal = None;
        for href in offers {
            // Copied out before anything is fetched, for the same reason the
            // stylesheets are: the reply lands in the buffer the page's own
            // bytes came through.
            let raw = href.get(dom);
            let mut declared = [0u8; images::MAX_SRC_LEN + 1];
            let len = raw.len().min(declared.len());
            declared[..len].copy_from_slice(&raw[..len]);
            let mut url = [0u8; images::MAX_SRC_LEN];
            let Ok(len) =
                images::resolve_subresource(self.current_url(), &declared[..len], &mut url)
            else {
                continue;
            };
            if undrawable_picture(&url[..len]) {
                continue;
            }
            tried += 1;
            let (status, bitmap, _, _) = self.fetch_inline_image(&url[..len]);
            if let Some(picture) = bitmap {
                let mut face = [[0u8; 4]; FAVICON_FACE_PIXELS];
                reduce_to_face(picture, &mut face);
                self.favicon = Some(face);
                break;
            }
            if let images::ImageStatus::Refused(refusal) = status {
                last_refusal = Some(refusal);
            }
        }
        self.log_favicon(tried, self.favicon.is_some(), offers.len(), last_refusal);
    }

    /// Says whether the page's own picture arrived, how many of its offers
    /// were fetched, and — when none did — what the reader ran into.
    ///
    /// WHY the reason is said at all: an icon that does not draw looks from
    /// outside like a feature that was never built, and the difference between
    /// "the page offered only formats this reader cannot draw", "the origin
    /// refused the file" and "the reply stopped short" is only visible here.
    fn log_favicon(
        &self,
        tried: usize,
        drawn: bool,
        offers: usize,
        reason: Option<images::ImageRefusal>,
    ) {
        if offers == 0 {
            return;
        }
        let mut log_buf = [0u8; 128];
        let prefix = if drawn {
            b"[BROWSER APP] Page icon drawn tries=".as_slice()
        } else {
            b"[BROWSER APP] Page icon not drawn tries=".as_slice()
        };
        let plen = prefix.len();
        log_buf[..plen].copy_from_slice(prefix);
        let mut lp = browser::append_decimal(&mut log_buf, plen, tried as u64);
        log_buf[lp] = b'/';
        lp += 1;
        lp = browser::append_decimal(&mut log_buf, lp, offers as u64);
        if !drawn {
            let text = match reason {
                Some(refusal) => refusal_reason(refusal),
                None => b"no offer in a drawable format",
            };
            let p = b" reason=";
            log_buf[lp..lp + p.len()].copy_from_slice(p);
            lp += p.len();
            let take = text.len().min(log_buf.len() - lp);
            log_buf[lp..lp + take].copy_from_slice(&text[..take]);
            lp += take;
            // The words name the failure; only the number the transport
            // answered with says who refused it, so the line carries both.
            if let Some(images::ImageRefusal::FetchFailed { code }) = reason {
                let p = b" err=";
                if lp + p.len() + 12 <= log_buf.len() {
                    log_buf[lp..lp + p.len()].copy_from_slice(p);
                    lp += p.len();
                    if code < 0 {
                        log_buf[lp] = b'-';
                        lp += 1;
                    }
                    lp = browser::append_decimal(
                        &mut log_buf,
                        lp,
                        i64::from(code).unsigned_abs() as u64,
                    );
                }
            }
        }
        if let Ok(line) = core::str::from_utf8(&log_buf[..lp]) {
            std::log(line).ok();
        }
    }

    /// Says that a linked stylesheet was blocked as mixed content.
    ///
    /// WHY it is said separately and not as another code on the line above:
    /// every other code on that line is something that happened to a request
    /// this reader sent. No request was sent here, and a reader scanning the
    /// log for a gateway fault would chase one that never occurred.
    fn log_stylesheet_blocked_as_insecure(&self) {
        std::log(
            "[BROWSER APP] Linked stylesheet blocked: this reader carries no \
             stylesheet over TLS, and the document on screen arrived over a \
             verified channel, so the file was not fetched in plaintext.",
        )
        .ok();
    }

    /// Says that a page linked a stylesheet that did not become rules.
    ///
    /// WHY this is logged at all: an unstyled page looks like a broken
    /// renderer, and the difference between "the page links nothing", "the
    /// file arrived" and "the origin refused it" is only visible from here.
    /// A negative code is the reader's own fault — no reply (-err), a reply
    /// that is not HTTP at all (-1), or a reply with no stylesheet text in
    /// it (-2); a positive one is the status the origin answered with.
    fn log_stylesheet_failed(&self, code: i32) {
        let mut log_buf = [0u8; 96];
        let prefix = b"[BROWSER APP] Linked stylesheet not applied code=";
        let plen = prefix.len();
        log_buf[..plen].copy_from_slice(prefix);
        let mut lp = plen;
        if code < 0 {
            log_buf[lp] = b'-';
            lp += 1;
        }
        lp = browser::append_decimal(&mut log_buf, lp, code.unsigned_abs() as u64);
        if let Ok(line) = core::str::from_utf8(&log_buf[..lp]) {
            std::log(line).ok();
        }
    }

    /// Loads a reader document, wrapping every line to the reading column.
    ///
    /// WHY the wrapping happens here and not in the renderer: a renderer
    /// that wrapped per pixel would have to break the same line over and
    /// over, and the break would not be stable while the reader scrolls.
    /// Wrapping once, when the document is loaded, gives the renderer a
    /// list of lines that are each already the right length.
    /// Resolves, fetches and decodes one inline image.
    ///
    /// WHY this is synchronous: the browser already loads a page
    /// synchronously over the same TCP path. An image is fetched only
    /// after the page's own bytes are held, through the same gateway and
    /// with the same transport, and its result is a bitmap or a typed
    /// refusal. The response buffer is reused, so an image body is bounded
    /// by the same transport buffer as a page.
    fn fetch_inline_image(
        &mut self,
        url: &[u8],
    ) -> (images::ImageStatus, Option<std::image::Bitmap<'static>>, u32, u32) {
        // A document whose own bytes came off a channel whose TLS handshake
        // completed is shown with the mark that says so. This transport is
        // plaintext -- the `false` below is the whole of it -- so fetching the
        // picture here would put it on an unencrypted hop under a padlock, and
        // anything on that path could hand back different bytes. Refuse the
        // fetch, and let the placeholder card on the page say why.
        if images::mixed_content_block(self.document_tls_verified, true) {
            return (
                images::ImageStatus::Refused(images::ImageRefusal::InsecureSubresource),
                None,
                0,
                0,
            );
        }
        // WHY this is wide: the address goes on the wire percent-encoded,
        // and a query string of separators alone triples it. A request
        // buffer that ran out would send half an address and be answered
        // with half a picture, so an address that cannot be sent whole is
        // refused for its own reason before a byte is written. The rest of
        // the request — method line, Host, User-Agent, Close — is 108
        // bytes, which is what the margin below is sized against.
        let mut request = [0u8; 1024];
        let request_len = browser::format_proxy_request(url, &mut request);
        if request_len + 128 > request.len() {
            return (
                images::ImageStatus::Refused(images::ImageRefusal::SourceTooLong),
                None,
                0,
                0,
            );
        }
        let local_port = self.take_local_port();
        let outcome = fetch_over_tcp(
            [10, 0, 2, 2],
            4300,
            local_port,
            &request[..request_len],
            MAX_RESPONSE_BYTES,
            false,
        );
        if outcome.bytes == 0 {
            return (
                images::ImageStatus::Refused(images::ImageRefusal::FetchFailed {
                    code: outcome.error as i32,
                }),
                None,
                0,
                0,
            );
        }
        let buffer = unsafe { &*core::ptr::addr_of!(RESPONSE_BUF) };
        let raw = &buffer[..outcome.bytes];
        let Some(response) = browser::parse_http_response(raw) else {
            return (
                images::ImageStatus::Refused(images::ImageRefusal::BadHeader),
                None,
                0,
                0,
            );
        };
        if response.status_code != 200 {
            return (
                images::ImageStatus::Refused(images::ImageRefusal::FetchFailed {
                    code: response.status_code as i32,
                }),
                None,
                0,
                0,
            );
        }
        // A body the gateway or the transport had to cut is refused, not
        // decoded: half a PNG is not a picture.
        //
        // WHY this is not reported as "too large": the file was never
        // measured against this browser's own cap here. What happened is
        // that the reply stopped short of the length the origin promised,
        // and the placeholder has to say which of the two it ran into.
        let truncated = outcome.truncated
            || browser::http_header_value(raw, b"x-interos-truncated").is_some();
        let body = response.body;
        if truncated {
            return (
                images::ImageStatus::Refused(images::ImageRefusal::CutShort { bytes: body.len() }),
                None,
                0,
                0,
            );
        }
        if body.len() > images::MAX_IMAGE_BODY_BYTES {
            return (
                images::ImageStatus::Refused(images::ImageRefusal::TooLarge {
                    bytes: body.len(),
                    limit: images::MAX_IMAGE_BODY_BYTES,
                }),
                None,
                0,
                0,
            );
        }
        let admission = images::admit_image(body);
        if matches!(admission, images::ImageAdmission::Refused(images::ImageRefusal::UnsupportedType))
            && browser::proxy_reports_failure(raw)
        {
            // The gateway reports a failed upstream fetch as an HTTP 200
            // JSON envelope. That is a fetch failure, not a file of an
            // unknown type, and the placeholder should say so.
            return (
                images::ImageStatus::Refused(images::ImageRefusal::FetchFailed { code: -5 }),
                None,
                0,
                0,
            );
        }
        let (width, height) = match admission {
            images::ImageAdmission::Accepted {
                width, height, ..
            } => (width, height),
            images::ImageAdmission::Refused(refusal) => {
                return (images::ImageStatus::Refused(refusal), None, 0, 0);
            }
        };
        let Some(required) = images::required_rgba_bytes(width, height) else {
            return (
                images::ImageStatus::Refused(images::ImageRefusal::DimensionsTooLarge {
                    width,
                    height,
                    limit: images::MAX_IMAGE_DIMENSION,
                }),
                None,
                0,
                0,
            );
        };
        if self.image_arena_used.saturating_add(required) > images::IMAGE_ARENA_BYTES {
            return (
                images::ImageStatus::Refused(images::ImageRefusal::MemoryBudget),
                None,
                0,
                0,
            );
        }
        // The arena is a static, and each image owns a disjoint range of
        // it for the lifetime of the page. The bitmap borrows that range;
        // the publisher's single-threaded event loop is what makes this
        // sound, and `image_arena_used` never hands the same bytes out
        // twice.
        let arena_ptr = core::ptr::addr_of_mut!(IMAGE_ARENA) as *mut u8;
        let raster = unsafe {
            core::slice::from_raw_parts_mut(arena_ptr.add(self.image_arena_used), required)
        };
        match std::image::decode_image(body, b"", raster) {
            Ok((decoded_w, decoded_h, len)) if len <= required => {
                let start = self.image_arena_used;
                self.image_arena_used += len;
                let pixels = unsafe { core::slice::from_raw_parts(arena_ptr.add(start), len) };
                let bitmap = std::image::Bitmap {
                    w: decoded_w,
                    h: decoded_h,
                    rgba: pixels,
                };
                (images::ImageStatus::Decoded, Some(bitmap), decoded_w, decoded_h)
            }
            _ => (
                images::ImageStatus::Refused(images::ImageRefusal::DecodeFailed),
                None,
                0,
                0,
            ),
        }
    }

    /// Places one `<img>` element in the page's image list.
    fn place_image(&mut self, marker: images::ImageMarker<'_>, column_w: usize) -> usize {
        if self.image_count >= images::MAX_INLINE_IMAGES {
            return usize::MAX;
        }
        // Every branch below assigns the status exactly once, so the value is
        // defined on all paths rather than defaulted to a refusal that could
        // be mistaken for a real outcome.
        let status: images::ImageStatus;
        let mut bitmap = None;
        let mut display_w = browser::CONTENT_WIDTH;
        let mut display_h = images::PLACEHOLDER_HEIGHT;

        if marker.src.is_empty() {
            status = images::ImageStatus::Refused(images::ImageRefusal::NoSource);
        } else if marker.src.len() > images::MAX_SRC_LEN {
            // The marker cannot carry this address without cutting it,
            // and fetching a cut address would be fetching something
            // other than what the page named.
            status = images::ImageStatus::Refused(images::ImageRefusal::SourceTooLong);
        } else if matches!(
            parse_uri(self.current_url()).scheme,
            UriScheme::File | UriScheme::InterLayer
        ) {
            status = images::ImageStatus::Refused(images::ImageRefusal::LocalDocument);
        } else {
            let mut url = [0u8; images::MAX_SRC_LEN];
            match images::resolve_subresource(self.current_url(), marker.src, &mut url) {
                Ok(len) => {
                    let (result, decoded, width, height) =
                        self.fetch_inline_image(&url[..len]);
                    status = result;
                    bitmap = decoded;
                    if let Some((w, h)) =
                        bitmap.map(|bitmap| (bitmap.w, bitmap.h)).or(if width > 0 {
                            Some((width, height))
                        } else {
                            None
                        })
                    {
                        let (dw, dh) = images::fit_display(
                            w.max(1),
                            h.max(1),
                            marker.declared_w,
                            marker.declared_h,
                        );
                        // The image sits inside the block's left inset, so
                        // the column it may use is narrower than the page's.
                        let available = column_w.max(1);
                        if dw > available {
                            display_w = available;
                            display_h = (dh.saturating_mul(available) / dw).max(1);
                        } else {
                            display_w = dw;
                            display_h = dh;
                        }
                    }
                }
                Err(error) => {
                    status = images::ImageStatus::Refused(match error {
                        images::ResolveError::NoSource => images::ImageRefusal::NoSource,
                        images::ResolveError::UnsupportedScheme => {
                            images::ImageRefusal::UnsupportedSource
                        }
                        images::ResolveError::InvalidBase => {
                            images::ImageRefusal::FetchFailed { code: -22 }
                        }
                        images::ResolveError::TooLong => images::ImageRefusal::SourceTooLong,
                    });
                }
            }
        }

        let mut slot = PlacedImage::EMPTY;
        slot.bitmap = bitmap;
        slot.status = status;
        slot.display_w = display_w;
        slot.display_h = display_h;
        let n = marker.alt.len().min(slot.alt.len());
        slot.alt[..n].copy_from_slice(&marker.alt[..n]);
        slot.alt_len = n;
        self.images[self.image_count] = slot;
        self.image_count += 1;
        self.image_count - 1
    }

    /// The box one placed image needs: its own size when it decoded, and
    /// the notice's strip when it did not.
    ///
    /// WHY the notice is measured and not given the markup's box: a page
    /// whose hero is too large for the reader would otherwise open a hole
    /// the height of the screen around the one line that says so.
    fn placed_image_box(slot: &PlacedImage) -> (usize, usize) {
        match slot.status {
            images::ImageStatus::Decoded => (
                slot.display_w.max(1).min(browser::CONTENT_WIDTH),
                slot.display_h.max(1),
            ),
            images::ImageStatus::Refused(refusal) => {
                let notice = font::measure(refusal.placeholder(), 1) as usize + 24;
                (
                    notice.min(browser::CONTENT_WIDTH).max(64),
                    images::PLACEHOLDER_HEIGHT,
                )
            }
        }
    }

    pub fn load_document_content(&mut self, content: &[u8]) {
        self.doc_line_count = 0;
        self.doc_total_lines = 0;
        self.image_count = 0;
        self.image_arena_used = 0;
        self.favicon = None;
        let mut tokenizer = DocumentTokenizer::new(content);
        let mut pieces: [[u8; MAX_LINE_BYTES]; browser::MAX_WRAP_PIECES] =
            [[0; MAX_LINE_BYTES]; browser::MAX_WRAP_PIECES];
        let mut piece_lens: [usize; browser::MAX_WRAP_PIECES] = [0; browser::MAX_WRAP_PIECES];
        let lines = unsafe { &mut *core::ptr::addr_of_mut!(DOC_LINES) };
        let mut current_style = css::ComputedStyle::DEFAULT;
        let mut style_epoch: u16 = 0;

        while let Some((line, kind)) = tokenizer.next_line() {
            // A style marker is metadata, not reader text: it changes the
            // style of the lines that follow and occupies no line itself.
            if css::is_style_marker(line) {
                if let Some(style) = css::parse_style_marker(line) {
                    current_style = style;
                    style_epoch = style_epoch.wrapping_add(1);
                }
                continue;
            }
            if kind == LineKind::Image {
                let Some(marker) = images::parse_image_marker(line) else {
                    continue;
                };
                let index = self.place_image(
                    marker,
                    browser::CONTENT_WIDTH
                        .saturating_sub(current_style.padding_left as usize)
                        .max(64),
                );
                if self.doc_line_count < MAX_DOC_LINES {
                    let idx = self.doc_line_count;
                    let mut dl = DocLine::EMPTY;
                    dl.kind = LineKind::Image;
                    dl.style = current_style;
                    dl.style_epoch = style_epoch;
                    dl.image_index = if index < 255 { index as u8 } else { 255 };
                    if index < self.image_count {
                        let slot = &self.images[index];
                        let n = slot.alt_len.min(dl.bytes.len());
                        dl.bytes[..n].copy_from_slice(&slot.alt[..n]);
                        dl.len = n;
                        dl.layout_h =
                            (slot.display_h + 12).min(u16::MAX as usize) as u16;
                    } else {
                        dl.layout_h = images::PLACEHOLDER_HEIGHT as u16;
                    }
                    lines[idx] = dl;
                    self.doc_line_count += 1;
                }
                self.doc_total_lines += 1;
                continue;
            }

            let scale = match kind {
                LineKind::Heading1 => TYPE_H1_SCALE,
                // Paragraph text is the one run the styled renderer can set
                // on the larger rung, so its wrapping has to use that rung.
                // Wrapping at scale 1 and drawing at scale 2 is how a styled
                // paragraph ran off the column.
                LineKind::Paragraph if current_style.font_scale >= 2 => 2,
                _ => 1,
            };
            // The text is wrapped inside the block's left inset, not the
            // full column, or a padded paragraph would draw past the
            // column it was measured against.
            let wrap_width = browser::CONTENT_WIDTH
                .saturating_sub(current_style.padding_left as usize)
                .max(64);
            let count = browser::wrap_line(
                line,
                scale,
                wrap_width,
                &mut pieces,
                &mut piece_lens,
            );

            // WHY the total counts the pieces and not the source lines: the
            // reader shows the wrapped lines, so the total has to be
            // counted in the same unit. Counting source lines and pieces
            // in one number made the disclosure read "the first 150 of
            // 100 lines".
            self.doc_total_lines += count;

            if self.doc_line_count >= MAX_DOC_LINES {
                // The document is longer than the reader holds. The count
                // is remembered so the screen can say so.
                continue;
            }

            // A line that is not a link has no target to carry; a link
            // carries its target and its label.
            let (tgt, lbl) = if kind == LineKind::GemtextLink {
                match parse_gemtext_link(line) {
                    Some((tgt, lbl)) => (tgt, lbl),
                    None => (&b""[..], &b""[..]),
                }
            } else {
                (&b""[..], &b""[..])
            };

            for piece in 0..count {
                if self.doc_line_count >= MAX_DOC_LINES {
                    break;
                }
                let idx = self.doc_line_count;
                let mut dl = DocLine::EMPTY;
                let n = piece_lens[piece];
                dl.bytes[..n].copy_from_slice(&pieces[piece][..n]);
                dl.len = n;
                dl.kind = kind;
                dl.continued = piece > 0;
                dl.style = current_style;
                dl.style_epoch = style_epoch;
                if kind == LineKind::GemtextLink {
                    let u = tgt.len().min(64);
                    dl.link_target[..u].copy_from_slice(&tgt[..u]);
                    dl.link_target_len = u;
                    let l = lbl.len().min(64);
                    dl.link_label[..l].copy_from_slice(&lbl[..l]);
                    dl.link_label_len = l;
                }
                lines[idx] = dl;
                self.doc_line_count += 1;
            }
        }

        // A block's first line takes the top margin and its last line
        // takes the bottom border. Both ends are only knowable once the
        // whole document is in.
        //
        // WHY a blank line also ends a block: sibling paragraphs with the
        // same computed style emit no new style marker, so their epochs
        // match. The converter separates block elements with a blank line,
        // and that separator is what keeps two paragraphs from becoming
        // one block that only the first of them spaces.
        let count = self.doc_line_count;
        for i in 0..count {
            let epoch = lines[i].style_epoch;
            lines[i].block_start = i == 0
                || lines[i - 1].style_epoch != epoch
                || lines[i - 1].kind == LineKind::Empty;
            lines[i].block_end = i + 1 >= count
                || lines[i + 1].style_epoch != epoch
                || lines[i + 1].kind == LineKind::Empty;
        }

        // WHY the scroll extent is measured here: the reader can only be
        // scrolled as far as the document is long, and the document's
        // length is known only once its lines are in. A long page scrolls
        // through all of it instead of stopping at the first screenful.
        let content_h = browser::document_height_px(&lines[..self.doc_line_count]);
        self.scroll = browser::reader_scroll(content_h, READER_VIEWPORT_H);
        // A reader document replaced an HTML page; the content band now
        // belongs to the reader.
        self.html_view = false;
        self.field_focus = NO_FIELD;
    }

    /// Parses markup into the HTML view.
    ///
    /// WHY this is a second path and not a rewrite of the reader: a page
    /// that is plain text or Gemtext has no elements to lay out, and the
    /// reader already draws those with wrapped lines. Markup is the case
    /// the reader could not express — tables, forms, images in the flow,
    /// and blocks whose width depends on their parent.
    ///
    /// Returns false when the markup holds nothing the layout engine can
    /// place, so the caller can fall back to the reader.
    pub fn load_html_document(&mut self, source: &[u8]) -> bool {
        self.doc_line_count = 0;
        self.doc_total_lines = 0;
        self.image_count = 0;
        self.image_arena_used = 0;
        self.field_focus = NO_FIELD;

        let dom = unsafe { &mut *core::ptr::addr_of_mut!(PAGE_DOM) };
        dom.parse(source);
        // The tab is renamed to what the page calls itself.
        //
        // WHY here and not when the address was entered: before the bytes
        // arrive the URL is the only name there is, so the tab starts out
        // wearing it, and the document replaces that as soon as it is read.
        // A page that states no title keeps the URL, which is the truth:
        // nothing named it.
        let stated_title = dom.meta().title.get(dom);
        if !stated_title.is_empty() {
            self.tabs.active_tab_mut().set_title(stated_title);
        }
        // WHY the visit is named here: the log records the address the moment
        // the navigation is asked for, because a page that never arrived was
        // still a visit. What the page calls itself, and the line it wrote
        // under that name, are only known once its bytes have been read — so
        // the entry is dressed when the document arrives, and stays bare when
        // it does not.
        let stated_summary = dom.meta().description.get(dom);
        if self.history.name_newest(stated_title, stated_summary) {
            let _ = self.history.persist();
        }
        if dom.node_count() <= 1 {
            self.html_view = false;
            return false;
        }
        // The page's own rules are read before its bytes are laid out, so
        // the column is measured once, against the styling the author asked
        // for, rather than measured twice and corrected on screen.
        self.fetch_linked_stylesheets(dom);
        self.execute_page_scripts();
        let out = unsafe { &mut *core::ptr::addr_of_mut!(PAGE_LAYOUT) };
        {
            let sheet = css::parse_stylesheet(dom.css(), dom.token_store());
            layout::build(dom, &sheet, browser::CONTENT_WIDTH, out);
            for id in 0..dom.node_count() as layout::NodeId {
                let bs = layout::style::compute(dom, id, &sheet, None);
                if bs.has_unsupported() {
                    for i in 0..bs.unsupported_count as usize {
                        let prop = bs.unsupported[i].as_str();
                        let mut log_buf = [0u8; 96];
                        let prefix = b"[BROWSER APP] Unsupported CSS property in page: ";
                        let plen = prefix.len();
                        log_buf[..plen].copy_from_slice(prefix);
                        let pbytes = prop.as_bytes();
                        let take = pbytes.len().min(log_buf.len() - plen);
                        log_buf[plen..plen + take].copy_from_slice(&pbytes[..take]);
                        if let Ok(msg) = core::str::from_utf8(&log_buf[..plen + take]) {
                            std::log(msg).ok();
                        }
                    }
                    break;
                }
            }
        }
        if out.is_empty() {
            self.html_view = false;
            return false;
        }

        // Each control starts with the value the markup gave it; from then
        // on the value is what the person has typed.
        let text = unsafe { &mut *core::ptr::addr_of_mut!(FIELD_TEXT) };
        let lens = unsafe { &mut *core::ptr::addr_of_mut!(FIELD_TEXT_LEN) };
        for index in 0..out.field_count {
            let value = out.fields[index].value.get(dom);
            let n = value.len().min(FIELD_TEXT_MAX);
            text[index][..n].copy_from_slice(&value[..n]);
            lens[index] = n as u8;
        }

        // The layout reserves the image slots in document order, so the
        // fetches run in that same order and each result lands in the
        // slot the display list already names.
        let image_count = out.image_count;
        for slot in 0..image_count {
            let item = out.images[slot];
            // One byte wider than the limit, so an address the browser
            // cannot carry still arrives wider than the limit and is
            // refused instead of quietly fetched cut short.
            let mut src = [0u8; images::MAX_SRC_LEN + 1];
            let mut alt = [0u8; 64];
            let src_bytes = item.src.get(dom);
            let alt_bytes = item.alt.get(dom);
            let src_len = src_bytes.len().min(src.len());
            let alt_len = alt_bytes.len().min(alt.len());
            src[..src_len].copy_from_slice(&src_bytes[..src_len]);
            alt[..alt_len].copy_from_slice(&alt_bytes[..alt_len]);
            let marker = images::ImageMarker {
                src: &src[..src_len],
                alt: &alt[..alt_len],
                declared_w: if item.declared_w > 0 {
                    Some(item.declared_w)
                } else {
                    None
                },
                declared_h: if item.declared_h > 0 {
                    Some(item.declared_h)
                } else {
                    None
                },
            };
            self.place_image(marker, (item.w as usize).max(1));
        }

        // The bytes are in, and a picture does not always land in the box
        // the markup drew. Re-flow so the text sits under the image that
        // actually arrived, and so an image that could not be shown keeps
        // the notice's strip instead of the whole hole.
        //
        // WHY this cannot fetch again: layout hands out slots in document
        // order under a fixed cap, so the second pass names the same slots
        // the fetches already filled.
        let mut outcomes = [layout::ImageOutcome::Unknown; layout::MAX_IMAGES];
        let mut reflow = false;
        for slot in 0..image_count {
            let (w, h) = Self::placed_image_box(&self.images[slot]);
            let item = out.images[slot];
            if w != item.w as usize || h != item.h as usize {
                outcomes[slot] = layout::ImageOutcome::Placed { w, h };
                reflow = true;
            }
        }
        if reflow {
            let sheet = css::parse_stylesheet(dom.css(), dom.token_store());
            layout::build_with_outcomes(&*dom, &sheet, browser::CONTENT_WIDTH, &outcomes, out);
        }

        // The page is laid out and its pictures are in, so the one picture it
        // asked to be known by is fetched last: nothing about it moves a box,
        // and holding up the first sight of the page for it would trade the
        // article for a sixteen-pixel square.
        self.fetch_favicon(&*dom);

        let height = out.height as usize;
        let note = if out.truncated { out.note_reserve() } else { 0 };
        self.scroll = browser::reader_scroll(
            height + browser::CONTENT_PADDING + note,
            READER_VIEWPORT_H,
        );
        self.html_view = true;
        if out.truncated {
            // WHY one line per budget: a page can run out of more than one
            // thing at once, and the reader only needs the worst while whoever
            // raises a budget needs all of them.
            for (bit, reason) in layout::OVERFLOW_BUDGETS {
                if out.overflow & bit == 0 {
                    continue;
                }
                let mut log_buf = [0u8; 112];
                let prefix = b"[BROWSER APP] ";
                let plen = prefix.len();
                let msg = reason.as_bytes();
                let take = msg.len().min(log_buf.len() - plen);
                log_buf[..plen].copy_from_slice(prefix);
                log_buf[plen..plen + take].copy_from_slice(&msg[..take]);
                if let Ok(line) = core::str::from_utf8(&log_buf[..plen + take]) {
                    std::log(line).ok();
                }
            }
        }
        true
    }

    /// Executes an inline script or event handler with deterministic step and memory budgets,
    /// and logs clear diagnostic messages for unsupported syntax and budget overruns.
    pub fn run_script(code: &[u8], dom: &mut layout::Dom, context: &str) {
        if code.is_empty() {
            return;
        }
        let _ = context;
        match browser::js::execute_script(code, dom, browser::js::DEFAULT_STEP_BUDGET) {
            Ok(_) => {
                std::log("[BROWSER APP] JS script executed successfully.").ok();
            }
            Err(browser::js::JsError::StepBudgetExhausted) => {
                std::log("[BROWSER APP] JS execution halted: step budget exhausted (infinite loop prevention).").ok();
            }
            Err(browser::js::JsError::CapacityExceeded) => {
                std::log("[BROWSER APP] JS execution halted: memory budget exceeded (AST/variable capacity limit).").ok();
            }
            Err(browser::js::JsError::UnsupportedSyntax(feature)) => {
                let mut log_buf = [0u8; 128];
                let prefix = b"[BROWSER APP] JS unsupported syntax: ";
                let plen = prefix.len();
                log_buf[..plen].copy_from_slice(prefix);
                let fbytes = feature.as_bytes();
                let flen = fbytes.len().min(log_buf.len() - plen);
                log_buf[plen..plen + flen].copy_from_slice(&fbytes[..flen]);
                if let Ok(msg) = core::str::from_utf8(&log_buf[..plen + flen]) {
                    std::log(msg).ok();
                }
            }
            Err(browser::js::JsError::UnexpectedToken) => {
                std::log("[BROWSER APP] JS syntax error: unexpected token.").ok();
            }
            Err(browser::js::JsError::ReferenceError) => {
                std::log("[BROWSER APP] JS runtime error: reference error (undeclared variable).").ok();
            }
            Err(browser::js::JsError::TypeError) => {
                std::log("[BROWSER APP] JS runtime error: type error.").ok();
            }
        }
    }

    /// Finds captured script tags in the DOM and executes them in order.
    pub fn execute_page_scripts(&mut self) {
        let dom = unsafe { &mut *core::ptr::addr_of_mut!(PAGE_DOM) };
        let node_count = dom.node_count();
        let mut script_nodes: [layout::NodeId; 16] = [layout::NO_NODE; 16];
        let mut script_count = 0usize;

        for id in 0..node_count as layout::NodeId {
            if dom.node(id).is_element() && dom.tag(id).eq_ignore_ascii_case(b"script") {
                if let Some(t) = dom.attr(id, b"type") {
                    let is_js = t.eq_ignore_ascii_case(b"text/javascript")
                        || t.eq_ignore_ascii_case(b"application/javascript")
                        || t.eq_ignore_ascii_case(b"text/ecmascript")
                        || t.eq_ignore_ascii_case(b"application/ecmascript")
                        || t.eq_ignore_ascii_case(b"module");
                    if !is_js {
                        continue;
                    }
                }
                if script_count < script_nodes.len() {
                    script_nodes[script_count] = id;
                    script_count += 1;
                }
            }
        }

        for i in 0..script_count {
            let nid = script_nodes[i];
            let mut buf = [0u8; 2048];
            let body = dom.script_body(nid);
            let n = body.len().min(buf.len());
            if n == 0 {
                continue;
            }
            buf[..n].copy_from_slice(&body[..n]);
            Self::run_script(&buf[..n], dom, "inline script");
        }
    }

    /// Whether a keyboard is on screen.
    fn keyboard_open(&self) -> bool {
        self.address_focused || self.field_focus != NO_FIELD
    }

    /// Gives the keyboard to a form control and scrolls it clear of it.
    fn focus_field(&mut self, index: usize) {
        let out = unsafe { &*core::ptr::addr_of!(PAGE_LAYOUT) };
        if index >= out.field_count {
            return;
        }
        self.address_focused = false;
        self.select_all = false;
        self.field_focus = index as u8;
        // A field under the keyboard cannot be seen while it is typed
        // into, so the document moves until the control's bottom edge is
        // above the accessory bar.
        let field = out.fields[index];
        let bottom = field.y as isize + field.h as isize;
        let screen_bottom =
            READER_TOP_Y as isize + bottom - self.scroll.offset as isize;
        let limit = KB_ACC_Y0 as isize - 16;
        if screen_bottom > limit {
            let moved = self.scroll.offset as isize + (screen_bottom - limit);
            self.scroll.offset =
                moved.clamp(0, self.scroll.max_offset() as isize) as usize;
        }
        let _ = std::audio_haptic_click();
        std::log("[BROWSER APP] A form control took the keyboard.").ok();
    }

    fn blur_field(&mut self) {
        if self.field_focus != NO_FIELD {
            self.field_focus = NO_FIELD;
            let _ = std::audio_haptic_click();
        }
    }

    fn field_push(&mut self, byte: u8) {
        let index = self.field_focus as usize;
        if index >= layout::MAX_FIELDS {
            return;
        }
        let lens = unsafe { &mut *core::ptr::addr_of_mut!(FIELD_TEXT_LEN) };
        let len = lens[index] as usize;
        if len < FIELD_TEXT_MAX {
            unsafe {
                (*core::ptr::addr_of_mut!(FIELD_TEXT))[index][len] = byte;
            }
            lens[index] = (len + 1) as u8;
        }
    }

    fn field_backspace(&mut self) {
        let index = self.field_focus as usize;
        if index >= layout::MAX_FIELDS {
            return;
        }
        let lens = unsafe { &mut *core::ptr::addr_of_mut!(FIELD_TEXT_LEN) };
        lens[index] = lens[index].saturating_sub(1);
    }

    fn field_clear(&mut self) {
        let index = self.field_focus as usize;
        if index < layout::MAX_FIELDS {
            unsafe {
                (*core::ptr::addr_of_mut!(FIELD_TEXT_LEN))[index] = 0;
            }
        }
    }

    fn toggle_check(&mut self, index: usize) {
        let out = unsafe { &mut *core::ptr::addr_of_mut!(PAGE_LAYOUT) };
        if index >= out.field_count {
            return;
        }
        let checked = !out.fields[index].checked;
        out.fields[index].checked = checked;
        // A radio group holds one answer: choosing one clears the others
        // that share its name.
        if out.fields[index].input == layout::InputType::Radio && checked {
            let name = out.fields[index].name;
            for other in 0..out.field_count {
                if other != index
                    && out.fields[other].input == layout::InputType::Radio
                    && out.fields[other].name == name
                {
                    out.fields[other].checked = false;
                }
            }
        }
    }

    /// Replaces a select's value with its next option.
    fn cycle_select(&mut self, index: usize) {
        let dom = unsafe { &*core::ptr::addr_of!(PAGE_DOM) };
        let out = unsafe { &*core::ptr::addr_of!(PAGE_LAYOUT) };
        if index >= out.field_count {
            return;
        }
        let field = out.fields[index];
        if field.node == layout::NO_NODE {
            return;
        }
        let text = unsafe { &*core::ptr::addr_of!(FIELD_TEXT) };
        let lens = unsafe { &*core::ptr::addr_of!(FIELD_TEXT_LEN) };
        let selected = &text[index][..lens[index] as usize];
        let mut first: Option<&[u8]> = None;
        let mut next: Option<&[u8]> = None;
        let mut seen = false;
        let mut child = dom.first_child(field.node);
        while let Some(id) = child {
            if dom.node(id).is_element() && dom.tag(id) == b"option" {
                let value = option_value(dom, id);
                if first.is_none() {
                    first = Some(value);
                }
                if seen && next.is_none() {
                    next = Some(value);
                }
                if value == selected {
                    seen = true;
                }
            }
            child = dom.next_sibling(id);
        }
        let choice = match (seen, next) {
            (true, Some(value)) => value,
            _ => match first {
                Some(value) => value,
                None => return,
            },
        };
        let n = choice.len().min(FIELD_TEXT_MAX);
        unsafe {
            let text = &mut *core::ptr::addr_of_mut!(FIELD_TEXT);
            text[index][..n].copy_from_slice(&choice[..n]);
            (*core::ptr::addr_of_mut!(FIELD_TEXT_LEN))[index] = n as u8;
        }
        let _ = std::audio_haptic_click();
    }

    /// Submits the form the control at `trigger` belongs to.
    ///
    /// The body and the address are encoded into statics first, so the
    /// navigation that sends them does not borrow the display list.
    pub fn submit_form(&mut self, trigger: usize) {
        let dom = unsafe { &*core::ptr::addr_of!(PAGE_DOM) };
        let out = unsafe { &*core::ptr::addr_of!(PAGE_LAYOUT) };
        if trigger >= out.field_count {
            return;
        }
        let field = out.fields[trigger];
        {
            let dom_mut = unsafe { &mut *core::ptr::addr_of_mut!(PAGE_DOM) };
            if let Some(onclick) = dom_mut.attr(field.node, b"onclick") {
                let mut buf = [0u8; 512];
                let n = onclick.len().min(buf.len());
                buf[..n].copy_from_slice(&onclick[..n]);
                Self::run_script(&buf[..n], dom_mut, "button onclick");
            }
        }
        if field.input == layout::InputType::Reset {
            // Reset restores the values the markup gave the controls.
            for index in 0..out.field_count {
                if nearest_form(dom, out.fields[index].node) != nearest_form(dom, field.node) {
                    continue;
                }
                let value = out.fields[index].value.get(dom);
                let n = value.len().min(FIELD_TEXT_MAX);
                unsafe {
                    let text = &mut *core::ptr::addr_of_mut!(FIELD_TEXT);
                    text[index][..n].copy_from_slice(&value[..n]);
                    (*core::ptr::addr_of_mut!(FIELD_TEXT_LEN))[index] = n as u8;
                }
            }
            let _ = std::audio_haptic_click();
            return;
        }
        let Some(form) = nearest_form(dom, field.node) else {
            std::log("[BROWSER APP] The control is not inside a form; nothing was submitted.").ok();
            return;
        };
        {
            let dom_mut = unsafe { &mut *core::ptr::addr_of_mut!(PAGE_DOM) };
            if let Some(onsubmit) = dom_mut.attr(form, b"onsubmit") {
                let mut buf = [0u8; 512];
                let n = onsubmit.len().min(buf.len());
                buf[..n].copy_from_slice(&onsubmit[..n]);
                Self::run_script(&buf[..n], dom_mut, "form onsubmit");
            }
        }
        let action = dom.attr(form, b"action").unwrap_or(b"");
        let method = if dom
            .attr(form, b"method")
            .map_or(false, |m| m.eq_ignore_ascii_case(b"post"))
        {
            layout::FormMethod::Post
        } else {
            layout::FormMethod::Get
        };

        let url_out = unsafe { &mut *core::ptr::addr_of_mut!(FORM_URL) };
        let target_len = if action.is_empty() {
            let n = self.current_url_len.min(url_out.len());
            url_out[..n].copy_from_slice(&self.current_url[..n]);
            n
        } else {
            let base = self.current_url();
            match browser::http::resolve_url(base, action, url_out) {
                Some(n) => n,
                None => {
                    std::log("[BROWSER APP] The form's action could not be resolved; nothing was submitted.").ok();
                    return;
                }
            }
        };

        // A submission carries the controls of its own form, not every
        // control on the page.
        let mut pairs: [(&[u8], &[u8]); layout::MAX_FIELDS] =
            [(&b""[..], &b""[..]); layout::MAX_FIELDS];
        let mut count = 0usize;
        let field_text = unsafe { &*core::ptr::addr_of!(FIELD_TEXT) };
        let field_lens = unsafe { &*core::ptr::addr_of!(FIELD_TEXT_LEN) };
        for index in 0..out.field_count {
            if count >= pairs.len() {
                break;
            }
            let control = out.fields[index];
            if control.name.is_empty() || nearest_form(dom, control.node) != Some(form) {
                continue;
            }
            if control.kind == layout::FieldKind::Button && index != trigger {
                continue;
            }
            match control.input {
                layout::InputType::Checkbox | layout::InputType::Radio => {
                    if !control.checked {
                        continue;
                    }
                }
                layout::InputType::Submit | layout::InputType::Reset => {
                    if index != trigger {
                        continue;
                    }
                }
                _ => {}
            }
            let mut value = &field_text[index][..field_lens[index] as usize];
            if value.is_empty()
                && (control.input == layout::InputType::Checkbox
                    || control.input == layout::InputType::Radio)
            {
                value = b"on";
            }
            pairs[count] = (control.name.get(dom), value);
            count += 1;
        }

        // A hidden control is part of the form but not of the display
        // list, so it is collected from the tree.
        for node in 0..dom.node_count() {
            if count >= pairs.len() {
                break;
            }
            let node = node as layout::NodeId;
            if !dom.node(node).is_element() || dom.tag(node) != b"input" {
                continue;
            }
            if !dom
                .attr(node, b"type")
                .map_or(false, |t| t.eq_ignore_ascii_case(b"hidden"))
            {
                continue;
            }
            let name = dom.attr(node, b"name").unwrap_or(b"");
            if name.is_empty() || nearest_form(dom, node) != Some(form) {
                continue;
            }
            pairs[count] = (name, dom.attr(node, b"value").unwrap_or(b""));
            count += 1;
        }

        match method {
            layout::FormMethod::Get => {
                if target_len > 220 {
                    std::log("[BROWSER APP] The form's action is too long for the browser's address bound; nothing was submitted.").ok();
                    return;
                }
                let encoded = unsafe { &mut *core::ptr::addr_of_mut!(FORM_ENCODED) };
                let Some(n) = browser::http::form_urlencode_url(
                    &url_out[..target_len],
                    &pairs[..count],
                    encoded,
                ) else {
                    std::log("[BROWSER APP] The form's query is larger than the browser's bound; nothing was submitted.").ok();
                    return;
                };
                if n > 256 {
                    std::log("[BROWSER APP] The form's query is longer than the browser's address bound; nothing was submitted.").ok();
                    return;
                }
                let mut target = [0u8; 256];
                target[..n].copy_from_slice(&encoded[..n]);
                let _ = self.navigate(&target[..n]);
            }
            layout::FormMethod::Post => {
                if target_len > 220 {
                    std::log("[BROWSER APP] The form's action is too long for the browser's address bound; nothing was submitted.").ok();
                    return;
                }
                let encoded = unsafe { &mut *core::ptr::addr_of_mut!(FORM_ENCODED) };
                let Some(n) = browser::http::form_urlencode(&pairs[..count], encoded) else {
                    std::log("[BROWSER APP] The form's data is larger than one request; nothing was submitted.").ok();
                    return;
                };
                let mut target = [0u8; 256];
                target[..target_len].copy_from_slice(&url_out[..target_len]);
                let _ = self.navigate_post(&target[..target_len], &encoded[..n]);
            }
        }
    }

    /// Navigates with a form body.
    ///
    /// WHY the body is copied into a static: the navigation reads it while
    /// building the request, after this call has returned, and a request
    /// that outlived its body buffer is not a request the form asked for.
    fn navigate_post(&mut self, target: &[u8], body: &[u8]) -> bool {
        let body_buf = unsafe { &mut *core::ptr::addr_of_mut!(FORM_BODY) };
        if body.len() > body_buf.len() {
            return false;
        }
        body_buf[..body.len()].copy_from_slice(body);
        self.form_post_len = body.len();
        self.history.push(target);
        let _ = self.history.persist();
        self.tabs.navigate_to(target);
        self.navigate_again(target)
    }

    /// Navigates to a link target, resolving relative references against
    /// the current page address.
    pub fn navigate_href(&mut self, href: &[u8]) -> bool {
        let _ = std::audio_haptic_click();
        let target_uri = browser::parse_uri(href);
        if target_uri.scheme != browser::UriScheme::Unknown {
            return self.navigate(href);
        }
        let base = self.current_url();
        let mut resolved = [0u8; browser::http::MAX_URL_BYTES];
        if let Some(n) = browser::http::resolve_url(base, href, &mut resolved) {
            self.navigate(&resolved[..n])
        } else {
            self.navigate(href)
        }
    }


    /// Saves the page the browser is showing into the shared storage area.
    ///
    /// WHY the shared storage area: it is the one place every app may look,
    /// so a saved page becomes a document the platform's index holds and the
    /// file manager lists beside everything else on the device. A download the
    /// rest of the device cannot see is not a download.
    ///
    /// WHY the bytes are the ones the transport delivered: the file is the
    /// response, not a summary of it.
    pub fn save_page_to_storage(&mut self) {
        if self.status != PageStatus::Loaded {
            std::log("[BROWSER APP] There is no loaded page to save.").ok();
            return;
        }
        let response = unsafe { &*core::ptr::addr_of!(HTML_BUF) };
        if self.page_source_len == 0 || self.page_source_len > response.len() {
            std::log("[BROWSER APP] The page's own document is not held; nothing was saved.").ok();
            return;
        }
        self.scan_downloads();
        let mut path_buf = [0u8; 96];
        let n = browser::download_path(self.downloads_saved, &mut path_buf);
        let Ok(path) = core::str::from_utf8(&path_buf[..n]) else {
            std::log("[BROWSER APP] The download path could not be built; nothing was saved.").ok();
            return;
        };
        match write_file(path, &response[..self.page_source_len]) {
            Ok(n) => {
                self.downloads_saved = self.downloads_saved.saturating_add(1);
                std::log("[BROWSER APP] The page was saved into the shared storage area.").ok();
                let _ = n;
                // WHY the index is refreshed here: the platform's
                // index is what the file manager and the launcher
                // read, and it is built by walking the shared
                // storage area. A saved page that is not in the
                // index is a file the rest of the device cannot
                // find.
                self.publish_download(path);
            }
            Err(_) => {
                std::log("[BROWSER APP] The page could not be written; nothing was saved.").ok();
            }
        }
    }

    /// Stages a downloaded file, requiring user confirmation before saving.
    pub fn stage_pending_download(
        &mut self,
        url: &[u8],
        content_type: &[u8],
        given_name: Option<&[u8]>,
        body: &[u8],
        truncated: bool,
    ) {
        // WHY an empty body is refused before the prompt: the response
        // carried headers and no bytes, so there is no file to offer. A
        // prompt for a file that does not exist asks the person to confirm
        // a download that can only produce an empty document.
        if body.is_empty() {
            self.fail_download(
                DownloadFailure::NoBytes,
                "[BROWSER APP] The origin sent no bytes; there was no file to save and nothing was saved.",
            );
            return;
        }

        let base_ptr = core::ptr::addr_of!(RESPONSE_BUF) as *const u8 as usize;
        let body_ptr = body.as_ptr() as usize;
        let body_offset = if body_ptr >= base_ptr && body_ptr < base_ptr + MAX_RESPONSE_BYTES {
            body_ptr - base_ptr
        } else {
            // WHY this is refused rather than defaulted to zero: an offset
            // of zero points at the response's own headers, so the file that
            // was written would begin "HTTP/1.0 200 OK" instead of holding
            // the picture the origin sent.
            self.fail_download(
                DownloadFailure::Lost,
                "[BROWSER APP] The bytes of the response could not be located in the response buffer; nothing was saved.",
            );
            return;
        };

        self.pending_download = browser::PendingDownload::new(
            url,
            content_type,
            given_name,
            body.len(),
            body_offset,
            truncated,
        );
        self.download_failure = DownloadFailure::None;
        self.status = PageStatus::DownloadPrompt;
        self.loading_progress = 100;
        self.tabs.active_tab_mut().loading_state = LoadingState::Completed;
        self.tabs.active_tab_mut().error_code = 0;
        std::log("[BROWSER APP] The origin sent a file download; staged awaiting user confirmation.").ok();
    }

    /// Records a download that could not be saved, and says why.
    ///
    /// WHY the reason is carried rather than implied: the screen prints one
    /// paragraph per failure, and a response that carried no bytes is a
    /// different failure from a storage area that refused the bytes.
    fn fail_download(&mut self, reason: DownloadFailure, message: &str) {
        self.download_failure = reason;
        self.status = PageStatus::DownloadFailed;
        self.loading_progress = 0;
        self.tabs.active_tab_mut().loading_state = LoadingState::Failed;
        self.tabs.active_tab_mut().error_code = -5; // EIO
        std::log(message).ok();
        let _ = std::audio_haptic_click();
    }

    /// User confirmed the pending download; saves it to shared storage.
    pub fn accept_pending_download(&mut self) -> bool {
        if !self.pending_download.active {
            return false;
        }
        let pending = self.pending_download;
        self.pending_download.active = false;

        let url = &pending.url[..pending.url_len];
        let content_type = &pending.content_type[..pending.content_type_len];
        let filename = &pending.filename[..pending.filename_len];
        let offset = pending.body_offset;
        let len = pending.body_len;
        let truncated = pending.truncated;

        let response = unsafe { &*core::ptr::addr_of!(RESPONSE_BUF) };
        // WHY the staged bytes are looked for again: the response buffer is
        // shared, and a body that is no longer where it was left would
        // otherwise be written as the headers at the start of the buffer.
        if offset + len > response.len() {
            self.fail_download(
                DownloadFailure::Lost,
                "[BROWSER APP] The bytes of the response were no longer in the response buffer; nothing was saved.",
            );
            return false;
        }
        let body = &response[offset..offset + len];

        self.save_media_download(url, content_type, Some(filename), body, truncated)
    }

    /// User refused the pending download; discards staged state.
    pub fn refuse_pending_download(&mut self) {
        self.pending_download.cancel();
        self.status = PageStatus::DownloadRefused;
        self.loading_progress = 100;
        self.tabs.active_tab_mut().loading_state = LoadingState::Completed;
        self.tabs.active_tab_mut().error_code = 0;
        std::log("[BROWSER APP] Download request refused by user.").ok();
        let _ = std::audio_haptic_click();
    }

    /// Writes a fetched file into the shared storage area.
    ///
    /// WHY the body is written in steps: the filesystem syscall carries at
    /// most 64 KiB per call, so a file larger than that is written in parts
    /// rather than refused. A refused write removes the partial file, so a
    /// damaged download is not left where the File Manager lists it.
    ///
    /// WHY an empty body is refused before the file is opened: opening a
    /// file creates it, so writing nothing would leave an empty document in
    /// the shared storage area under a download's name. The person would
    /// find a file that is not the file the origin sent, and the File
    /// Manager and the Gallery would list it as a real document.
    fn save_media_download(
        &mut self,
        url: &[u8],
        content_type: &[u8],
        given_name: Option<&[u8]>,
        body: &[u8],
        truncated: bool,
    ) -> bool {
        if body.is_empty() {
            self.fail_download(
                DownloadFailure::NoBytes,
                "[BROWSER APP] The origin sent no bytes; there was no file to save and nothing was saved.",
            );
            return false;
        }

        let mut name = [0u8; 72];
        let name_len = browser::download_filename(url, content_type, given_name, &mut name);
        let mut path_buf = [0u8; 96];
        let Some(path_len) = self.unused_download_path(&name[..name_len], &mut path_buf) else {
            self.download_failure = DownloadFailure::NoPath;
            self.status = PageStatus::DownloadFailed;
            self.loading_progress = 0;
            self.tabs.active_tab_mut().loading_state = LoadingState::Failed;
            self.tabs.active_tab_mut().error_code = -17; // EEXIST
            std::log("[BROWSER APP] No unused path could be found for the download; nothing was saved.").ok();
            let _ = std::audio_haptic_click();
            return false;
        };
        let Ok(path) = core::str::from_utf8(&path_buf[..path_len]) else {
            self.download_failure = DownloadFailure::NoPath;
            self.status = PageStatus::DownloadFailed;
            self.loading_progress = 0;
            self.tabs.active_tab_mut().loading_state = LoadingState::Failed;
            std::log("[BROWSER APP] The download path could not be built; nothing was saved.").ok();
            let _ = std::audio_haptic_click();
            return false;
        };
        match write_file(path, body) {
            Ok(written) => {
                std::log("[BROWSER APP] The response was a file and was saved into the shared storage area.").ok();
                if truncated {
                    std::log("[BROWSER APP] The transfer ended before the whole file arrived; the saved copy is partial.").ok();
                }
                self.last_download_truncated = truncated;
                self.download_failure = DownloadFailure::None;
                let _ = written;

                // Persist the download origin URL alongside the downloaded file metadata/record.
                let mut origin_path_buf = [0u8; 128];
                let pbytes = path.as_bytes();
                let otake = pbytes.len().min(origin_path_buf.len() - 8);
                origin_path_buf[..otake].copy_from_slice(&pbytes[..otake]);
                origin_path_buf[otake..otake + 7].copy_from_slice(b".origin");
                if let Ok(opath) = core::str::from_utf8(&origin_path_buf[..otake + 7]) {
                    let _ = write_file(opath, url);
                }

                self.publish_download(path);
                self.remember_download(path, content_type);
                self.status = PageStatus::Downloaded;
                self.loading_progress = 100;
                self.tabs.active_tab_mut().loading_state = LoadingState::Completed;
                self.tabs.active_tab_mut().error_code = 0;
                let _ = std::audio_haptic_click();
                true
            }
            Err(_) => {
                self.download_failure = DownloadFailure::StorageRefused;
                std::log("[BROWSER APP] The shared storage area refused the download; nothing was saved.").ok();
                self.status = PageStatus::DownloadFailed;
                self.loading_progress = 0;
                self.tabs.active_tab_mut().loading_state = LoadingState::Failed;
                self.tabs.active_tab_mut().error_code = -5; // EIO
                let _ = std::audio_haptic_click();
                false
            }
        }
    }

    /// Finds a download path that is not already taken.
    ///
    /// WHY the directory is probed: the app's own counter restarts at zero
    /// when it is closed, and a name that overwrote yesterday's download
    /// would lose a file the person kept. An open for reading that succeeds
    /// is an existing file.
    fn unused_download_path(&self, name: &[u8], out: &mut [u8]) -> Option<usize> {
        for index in 0..1000u32 {
            let n = browser::media_download_path(index, name, out);
            let path = core::str::from_utf8(&out[..n]).ok()?;
            match std::fs_open(path, std::FS_READ) {
                Ok(fd) => {
                    let _ = std::fs_close(fd);
                }
                Err(_) => return Some(n),
            }
        }
        None
    }

    /// Publishes a saved page into the platform's content index.
    ///
    /// WHY the index is refreshed and not written to: the index is the
    /// platform's record of what is in the shared storage area, and it is
    /// built by walking that area. Refreshing it is the platform's own
    /// way of publishing a document, and it is the only way an app can
    /// publish one.
    ///
    /// WHY the result is checked: a refresh that fails leaves the page
    /// invisible to the file manager and the launcher, and the app has to
    /// say so rather than let the person believe it was published.
    pub fn publish_download(&mut self, path: &str) {
        match std::content::content_index_refresh() {
            Ok(_) => {
                let mut buffer = [0u8; 4096];
                let found = std::content::content_index_find_path(path, &mut buffer);
                if found.is_some() {
                    std::log("[BROWSER APP] The saved page is in the platform's content index; the file manager and the launcher can find it.")
                        .ok();
                } else {
                    std::log("[BROWSER APP] The saved page was written, but the content index does not hold it yet; it may not appear in the file manager until the next index refresh.")
                        .ok();
                }
            }
            Err(_) => {
                std::log("[BROWSER APP] The content index could not be refreshed; the saved page is on the storage area but the file manager may not list it yet.")
                    .ok();
            }
        }
        self.refresh_downloads();
    }

    /// Reads the pages the browser saved and the files it downloaded from
    /// the platform's content index.
    ///
    /// WHY the index and not a list the app keeps: the index is the
    /// platform's record of what is in the shared storage area, so the
    /// list survives the app being closed and closed again, and it agrees
    /// with what the file manager shows.
    pub fn refresh_downloads(&mut self) {
        self.download_count = 0;
        self.downloads_error = false;
        let mut buffer = [0u8; 4096];
        let len = match std::content::content_index_list(&mut buffer) {
            Ok(len) => len,
            Err(_) => {
                std::log("[BROWSER APP] The content index could not be read; the downloads list is empty.").ok();
                return;
            }
        };
        let mut records: [std::content::DocumentRecord; MAX_DOWNLOADS] =
            [std::content::DocumentRecord {
                id: 0,
                path: "",
                mime: "",
                provenance: "",
                length: 0,
                mtime: 0,
            }; MAX_DOWNLOADS];
        let count = std::content::parse_index(&buffer[..len], &mut records);
        for record in records.iter().take(count) {
            if !browser::is_download_path(record.path)
                && !browser::is_media_download_path(record.path)
            {
                continue;
            }
            if self.download_count >= MAX_DOWNLOADS {
                break;
            }
            let entry = &mut self.downloads[self.download_count];
            let bytes = record.path.as_bytes();
            let n = bytes.len().min(entry.path.len());
            entry.path[..n].copy_from_slice(&bytes[..n]);
            entry.path_len = n;
            entry.length = record.length;
            let m = record.mime.len().min(entry.mime.len());
            entry.mime[..m].copy_from_slice(&record.mime.as_bytes()[..m]);
            entry.mime_len = m;
            entry.is_page = browser::is_download_path(record.path);
            self.download_count += 1;
        }
        self.scan_downloads();
    }

    /// Scans `/storage/downloads` to update `downloads_saved` to avoid collisions
    /// or overwriting existing downloaded files.
    pub fn scan_downloads(&mut self) {
        let mut list_buf = [0u8; 4096];
        if let Ok(n) = std::fs_list("/storage/downloads", &mut list_buf) {
            let text = core::str::from_utf8(&list_buf[..n]).unwrap_or("");
            for line in text.lines() {
                let name = line.split('|').next().unwrap_or("");
                if let Some(rest) = name.strip_prefix(browser::DOWNLOAD_NAME_PREFIX) {
                    if let Some(num_str) = rest.strip_suffix(".html") {
                        if let Ok(num) = num_str.parse::<u32>() {
                            if num >= self.downloads_saved {
                                self.downloads_saved = num.saturating_add(1);
                            }
                        }
                    }
                }
            }
        }
        for entry in self.downloads.iter().take(self.download_count) {
            if let Ok(path_str) = core::str::from_utf8(&entry.path[..entry.path_len]) {
                if let Some(name) = path_str.rsplit('/').next() {
                    if let Some(rest) = name.strip_prefix(browser::DOWNLOAD_NAME_PREFIX) {
                        if let Some(num_str) = rest.strip_suffix(".html") {
                            if let Ok(num) = num_str.parse::<u32>() {
                                if num >= self.downloads_saved {
                                    self.downloads_saved = num.saturating_add(1);
                                }
                            }
                        }
                    }
                }
            }
        }
        while self.downloads_saved < 100_000 {
            let mut probe_buf = [0u8; 96];
            let n = browser::download_path(self.downloads_saved, &mut probe_buf);
            if let Ok(p) = core::str::from_utf8(&probe_buf[..n]) {
                if let Ok(fd) = std::fs_open(p, std::FS_READ) {
                    let _ = std::fs_close(fd);
                    self.downloads_saved = self.downloads_saved.saturating_add(1);
                    continue;
                }
            }
            break;
        }
    }

    /// Asks the platform to open the file the last download wrote.
    ///
    /// WHY the platform resolves the handler: the registry is built from
    /// the installed packages' manifests, so the answer is a property of
    /// the device rather than a table this app keeps. A refusal names the
    /// refusal: no handler and a refused launch are different answers.
    pub fn open_last_download(&mut self) -> bool {
        if self.last_download_path_len == 0 || self.last_download_mime_len == 0 {
            self.open_state = OpenState::NoHandler;
            std::log("[BROWSER APP] The download's path or type is not held; nothing was opened.").ok();
            return false;
        }
        let Ok(path) = core::str::from_utf8(&self.last_download_path[..self.last_download_path_len])
        else {
            self.open_state = OpenState::Refused;
            return false;
        };
        let Ok(mime) = core::str::from_utf8(&self.last_download_mime[..self.last_download_mime_len])
        else {
            self.open_state = OpenState::NoHandler;
            return false;
        };
        match std::content::intent_dispatch(std::content::IntentKind::Mime, mime, path) {
            Ok(_) => {
                self.open_state = OpenState::Dispatched;
                std::log("[BROWSER APP] The platform dispatched the downloaded file to its handler.").ok();
                true
            }
            Err(code) if code == std::content::ERR_NO_HANDLER => {
                self.open_state = OpenState::NoHandler;
                std::log("[BROWSER APP] No installed app declared a handler for the download's media type.").ok();
                false
            }
            Err(_) => {
                self.open_state = OpenState::Refused;
                std::log("[BROWSER APP] The platform refused to launch the download's handler.").ok();
                false
            }
        }
    }

    /// Records the file a download wrote and the type the platform gives it.
    ///
    /// WHY the index's answer wins over the response header: the index
    /// derives a document's media type from its bytes and its extension,
    /// and that is the answer the file manager and the handler registry
    /// use. The header is the origin's claim; the index is what this
    /// device will actually hand to a handler.
    fn remember_download(&mut self, path: &str, content_type: &[u8]) {
        self.open_state = OpenState::NotAttempted;
        let n = path.len().min(self.last_download_path.len());
        self.last_download_path[..n].copy_from_slice(&path.as_bytes()[..n]);
        self.last_download_path_len = n;
        let mut buffer = [0u8; 4096];
        self.last_download_mime_len = match std::content::content_index_find_path(path, &mut buffer) {
            Some(record) if !record.mime.is_empty() => {
                let m = record.mime.len().min(self.last_download_mime.len());
                self.last_download_mime[..m].copy_from_slice(&record.mime.as_bytes()[..m]);
                m
            }
            _ => browser::bare_mime(content_type, &mut self.last_download_mime),
        };
    }

    /// Reads the documents the platform's index holds into the picker.
    ///
    /// WHY the whole index and not only browser downloads: an upload is a
    /// test of this device's path and a media file in the shared area is
    /// exactly what has to travel. The list is bounded, so a large index
    /// offers the first entries the index holds.
    fn refresh_uploads(&mut self) {
        self.upload_count = 0;
        let mut buffer = [0u8; 4096];
        let len = match std::content::content_index_list(&mut buffer) {
            Ok(len) => len,
            Err(_) => {
                std::log("[BROWSER APP] The content index could not be read; the upload picker is empty.").ok();
                return;
            }
        };
        let mut records: [std::content::DocumentRecord; MAX_PICK_FILES] =
            [std::content::DocumentRecord {
                id: 0,
                path: "",
                mime: "",
                provenance: "",
                length: 0,
                mtime: 0,
            }; MAX_PICK_FILES];
        let count = std::content::parse_index(&buffer[..len], &mut records);
        for record in records.iter().take(count) {
            if self.upload_count >= MAX_PICK_FILES {
                break;
            }
            if record.length == 0 {
                continue;
            }
            let index = self.upload_count;
            let entry = &mut self.uploads[index];
            let path = record.path.as_bytes();
            let path_len = path.len().min(entry.path.len());
            entry.path[..path_len].copy_from_slice(&path[..path_len]);
            entry.path_len = path_len;
            let mime = record.mime.as_bytes();
            let mime_len = mime.len().min(entry.mime.len());
            entry.mime[..mime_len].copy_from_slice(&mime[..mime_len]);
            entry.mime_len = mime_len;
            entry.length = record.length;
            self.upload_count += 1;
        }
        // WHY the two counts are both stated: an empty picker and a picker that
        // refused every record for length zero look identical on screen, and a
        // person cannot tell whether the index holds nothing or the platform
        // handed out records with no bytes in them. Saying which it saw is the
        // difference between a working device and a silent one.
        {
            let mut line = [0u8; 128];
            let mut lp = 0;
            let p1 = b"[BROWSER APP] The upload picker lists ";
            line[lp..lp + p1.len()].copy_from_slice(p1);
            lp += p1.len();
            lp = browser::append_decimal(&mut line, lp, self.upload_count as u64);
            let p2 = b" of ";
            line[lp..lp + p2.len()].copy_from_slice(p2);
            lp += p2.len();
            lp = browser::append_decimal(&mut line, lp, count as u64);
            let p3 = b" indexed documents.";
            if lp + p3.len() <= line.len() {
                line[lp..lp + p3.len()].copy_from_slice(p3);
                lp += p3.len();
            }
            if let Ok(text) = core::str::from_utf8(&line[..lp]) {
                let _ = std::log(text);
            }
        }
    }

    /// The local port the next outbound connection asks for.
    fn take_local_port(&mut self) -> u16 {
        let port = self.next_local_port;
        self.next_local_port = if self.next_local_port >= 45_900 {
            45_000
        } else {
            self.next_local_port + 1
        };
        port
    }

    /// Sends the picked file to the gateway and records its answer.
    ///
    /// WHY success is the gateway's byte count and not the write count:
    /// the device can say how many bytes it handed the transport, but only
    /// the gateway can say how many it received. The screen prints both,
    /// and calls the upload complete only when they agree.
    fn perform_upload(&mut self, index: usize) {
        self.upload_sent = 0;
        self.upload_confirmed = 0;
        self.upload_error = 0;
        if index >= self.upload_count {
            self.upload_state = TransferState::Failed;
            self.upload_error = -3;
            return;
        }
        let entry = self.uploads[index];
        let shown = entry.path_len.min(self.upload_name.len());
        self.upload_name[..shown].copy_from_slice(&entry.path[..shown]);
        self.upload_name_len = shown;
        if entry.length == 0 || entry.length > browser::MAX_UPLOAD_BYTES as u64 {
            self.upload_state = TransferState::Refused;
            std::log("[BROWSER APP] The picked file is larger than the 4 MiB upload limit; nothing was sent.").ok();
            return;
        }
        let Ok(path) = core::str::from_utf8(&entry.path[..entry.path_len]) else {
            self.upload_state = TransferState::Failed;
            self.upload_error = -3;
            return;
        };
        let length = entry.length as usize;
        let mut head = [0u8; 512];
        let head_len = browser::format_upload_request(
            &entry.path[..entry.path_len],
            &entry.mime[..entry.mime_len],
            length,
            &mut head,
        );
        let local_port = self.take_local_port();
        let outcome = upload_over_tcp(
            [10, 0, 2, 2],
            4300,
            local_port,
            &head[..head_len],
            UploadSource::File { path, length },
        );
        self.upload_sent = outcome.sent;
        if outcome.error != 0 || outcome.response == 0 {
            self.upload_state = TransferState::Failed;
            self.upload_error = if outcome.error != 0 { outcome.error } else { -5 };
            std::log("[BROWSER APP] The upload did not complete; no byte count was confirmed.").ok();
            return;
        }
        let response = unsafe { &*core::ptr::addr_of!(RESPONSE_BUF) };
        let Some(parsed) = browser::parse_http_response(&response[..outcome.response]) else {
            self.upload_state = TransferState::Failed;
            self.upload_error = -5;
            return;
        };
        if parsed.status_code != 200 {
            self.upload_state = TransferState::Failed;
            self.upload_error = -5;
            return;
        }
        match browser::json_u64_field(parsed.body, b"received_bytes") {
            Some(confirmed) if confirmed == entry.length => {
                self.upload_confirmed = confirmed;
                self.upload_state = TransferState::Complete;
                std::log("[BROWSER APP] The gateway confirmed the uploaded byte count.").ok();
            }
            Some(confirmed) => {
                self.upload_confirmed = confirmed;
                self.upload_state = TransferState::Failed;
                self.upload_error = -5;
                std::log("[BROWSER APP] The gateway received fewer bytes than were sent; the upload is not complete.").ok();
            }
            None => {
                self.upload_state = TransferState::Failed;
                self.upload_error = -5;
            }
        }
    }

    /// Measures this device's path to the Studio gateway.
    ///
    /// WHY every number is measured: the test exists to tell the person
    /// what the device did, not what a profile says it should do. A test
    /// that cannot read the clock or does not get the gateway's own byte
    /// count reports that instead of a rate.
    fn perform_speed_test(&mut self) {
        self.speed = SpeedResults::EMPTY;
        self.speed_running = true;

        // The download.
        let mut request = [0u8; 256];
        let request_len =
            browser::format_speedtest_download_request(SPEEDTEST_DOWNLOAD_BYTES, &mut request);
        let local_port = self.take_local_port();
        let start = std::get_ticks().ok();
        let outcome = fetch_over_tcp(
            [10, 0, 2, 2],
            4300,
            local_port,
            &request[..request_len],
            MAX_RESPONSE_BYTES,
            false,
        );
        let end = std::get_ticks().ok();
        match self.download_rate(start, end, outcome, SPEEDTEST_DOWNLOAD_BYTES) {
            Ok((bytes, ms, bps)) => {
                self.speed.download = TransferState::Complete;
                self.speed.download_bytes = bytes;
                self.speed.download_ms = ms;
                self.speed.download_bps = bps;
            }
            Err(code) => {
                self.speed.download = TransferState::Failed;
                self.speed.error = code;
            }
        }

        // The round trip to the gateway's first byte.
        let request_len =
            browser::format_speedtest_download_request(SPEEDTEST_LATENCY_BYTES, &mut request);
        let local_port = self.take_local_port();
        let start = std::get_ticks().ok();
        let outcome = fetch_over_tcp(
            [10, 0, 2, 2],
            4300,
            local_port,
            &request[..request_len],
            MAX_RESPONSE_BYTES,
            false,
        );
        match (start, outcome.first_byte_tick) {
            (Some(start), Some(first)) if outcome.bytes > 0 && outcome.error == 0 => {
                let body_ok = {
                    let response = unsafe { &*core::ptr::addr_of!(RESPONSE_BUF) };
                    match browser::parse_http_response(&response[..outcome.bytes]) {
                        Some(parsed) => {
                            parsed.status_code == 200
                                && parsed.body.len() == SPEEDTEST_LATENCY_BYTES
                        }
                        None => false,
                    }
                };
                if body_ok {
                    self.speed.latency = TransferState::Complete;
                    self.speed.latency_ms = first.saturating_sub(start);
                } else {
                    self.speed.latency = TransferState::Failed;
                    self.speed.error = -5;
                }
            }
            _ => {
                self.speed.latency = TransferState::Failed;
                self.speed.error = if start.is_none() { -1000 } else { outcome.error };
            }
        }

        // The upload.
        let buffer = unsafe { &mut *core::ptr::addr_of_mut!(UPLOAD_TEST_BUF) };
        for (index, byte) in buffer.iter_mut().enumerate() {
            *byte = (index.wrapping_mul(31).wrapping_add(7) & 0xFF) as u8;
        }
        let request_len =
            browser::format_speedtest_upload_request(SPEEDTEST_UPLOAD_BYTES, &mut request);
        let local_port = self.take_local_port();
        let start = std::get_ticks().ok();
        let outcome = upload_over_tcp(
            [10, 0, 2, 2],
            4300,
            local_port,
            &request[..request_len],
            UploadSource::Bytes(&buffer[..SPEEDTEST_UPLOAD_BYTES]),
        );
        let end = std::get_ticks().ok();
        let confirmed = if outcome.error == 0 && outcome.response > 0 {
            let response = unsafe { &*core::ptr::addr_of!(RESPONSE_BUF) };
            browser::parse_http_response(&response[..outcome.response])
                .filter(|parsed| parsed.status_code == 200)
                .and_then(|parsed| browser::json_u64_field(parsed.body, b"received_bytes"))
        } else {
            None
        };
        match (start, end, confirmed) {
            (Some(start), Some(end), Some(confirmed))
                if confirmed == SPEEDTEST_UPLOAD_BYTES as u64 && end >= start =>
            {
                let ms = end - start;
                self.speed.upload = TransferState::Complete;
                self.speed.upload_bytes = SPEEDTEST_UPLOAD_BYTES;
                self.speed.upload_confirmed = confirmed;
                self.speed.upload_ms = ms;
                self.speed.upload_bps = rate_per_second(SPEEDTEST_UPLOAD_BYTES, ms);
            }
            (None, _, _) => {
                self.speed.upload = TransferState::Failed;
                self.speed.error = -1000;
            }
            _ => {
                self.speed.upload = TransferState::Failed;
                if self.speed.error == 0 {
                    self.speed.error = if outcome.error != 0 { outcome.error } else { -5 };
                }
            }
        }

        self.speed_running = false;
        std::log("[BROWSER APP] Speed test finished. Completed rows were measured on this device; failed rows are marked not measured.").ok();
    }

    /// Validates a measured download against the gateway's declared size.
    fn download_rate(
        &self,
        start: Option<u64>,
        end: Option<u64>,
        outcome: FetchOutcome,
        expected: usize,
    ) -> Result<(usize, u64, u64), i64> {
        let (Some(start), Some(end)) = (start, end) else {
            return Err(-1000);
        };
        if outcome.error != 0 || outcome.bytes == 0 || outcome.truncated {
            return Err(if outcome.error != 0 { outcome.error } else { -5 });
        }
        let response = unsafe { &*core::ptr::addr_of!(RESPONSE_BUF) };
        let Some(parsed) = browser::parse_http_response(&response[..outcome.bytes]) else {
            return Err(-5);
        };
        if parsed.status_code != 200 || parsed.body.len() != expected {
            return Err(-5);
        }
        let ms = end.saturating_sub(start);
        Ok((expected, ms, rate_per_second(expected, ms)))
    }

    /// Fetches the fixed lab addresses and records what happened to each.
    ///
    /// WHY the lab does not navigate: the person's page stays where it is,
    /// and the lab's answers are a separate record. The reader buffers it
    /// uses are scratch; the document on screen is not touched.
    fn perform_lab(&mut self) {
        self.lab = [LabResult::EMPTY; LAB_URLS.len()];
        self.lab_running = true;
        for index in 0..LAB_URLS.len() {
            let result = self.check_lab_url(index);
            self.lab[index] = result;
        }
        self.lab_running = false;
        std::log("[BROWSER APP] Compatibility lab finished. Each row reports the gateway's answer or its refusal.").ok();
    }

    fn check_lab_url(&mut self, index: usize) -> LabResult {
        let mut request = [0u8; 512];
        let request_len = format_proxy_request(LAB_URLS[index], &mut request);
        let local_port = self.take_local_port();
        let outcome = fetch_over_tcp(
            [10, 0, 2, 2],
            4300,
            local_port,
            &request[..request_len],
            MAX_RESPONSE_BYTES,
            false,
        );
        let mut result = LabResult::EMPTY;
        if outcome.bytes == 0 || outcome.error != 0 {
            result.state = TransferState::Failed;
            result.error = outcome.error;
            return result;
        }
        result.bytes = outcome.bytes;
        let response = unsafe { &*core::ptr::addr_of!(RESPONSE_BUF) };
        let slice = &response[..outcome.bytes];
        let Some(parsed) = browser::parse_http_response(slice) else {
            result.state = TransferState::Failed;
            result.error = -5;
            return result;
        };
        result.status = parsed.status_code;
        let page_type = browser::proxy_content_type(slice).unwrap_or(parsed.content_type);
        let len = page_type.len().min(result.content_type.len());
        result.content_type[..len].copy_from_slice(&page_type[..len]);
        result.content_type_len = len;
        if parsed.status_code != 200 {
            result.state = TransferState::Failed;
            return result;
        }
        if browser::proxy_reports_failure(slice) {
            result.state = TransferState::Refused;
            result.error = -1;
            return result;
        }
        // A file is a truthful answer to the lab too: the gateway
        // delivered it, and the reader has no document to extract from it.
        if browser::content_type_is_binary(page_type) {
            result.state = TransferState::Refused;
            result.error = -2;
            return result;
        }
        let html = unsafe { &mut *core::ptr::addr_of_mut!(FILE_BUF) };
        let html_len = browser::extract_proxy_html(slice, html);
        if html_len == 0 {
            result.state = TransferState::Failed;
            result.error = -5;
            return result;
        }
        let document = unsafe { &mut *core::ptr::addr_of_mut!(READER_BUF) };
        let doc_len = browser::html_to_reader_document_kind(
            &html[..html_len],
            browser::PayloadKind::Markup,
            document,
        );
        if doc_len == 0 {
            result.state = TransferState::Failed;
            return result;
        }
        result.reader_lines = document[..doc_len]
            .split(|&b| b == b'\n')
            .filter(|line| !line.is_empty())
            .count();
        result.state = TransferState::Complete;
        result
    }

    pub fn navigate(&mut self, target: &[u8]) -> bool {
        self.history.push(target);
        let _ = self.history.persist();
        self.tabs.navigate_to(target);
        self.navigate_again(target)
    }

    /// Keeps the reason the gateway gave for a failed fetch.
    ///
    /// WHY the reason is kept and not summarised: it is the host's own
    /// account of what happened, and a summary would be a claim the browser
    /// cannot back. A gateway that named no reason leaves the screen with
    /// the generic failure, which is true.
    fn record_gateway_reason(&mut self, envelope: &[u8]) {
        let mut buffer = [0u8; browser::GATEWAY_REASON_MAX + 3];
        let n = browser::gateway_failure_reason(envelope, &mut buffer);
        if n == 0 {
            self.gateway_reason_len = 0;
            return;
        }
        // A control character would break the line the reason is drawn on,
        // so it is written as a space rather than as a break.
        let mut written = 0usize;
        for &byte in &buffer[..n] {
            let byte = if byte < 0x20 { b' ' } else { byte };
            self.gateway_reason[written] = byte;
            written += 1;
        }
        self.gateway_reason_len = written;
    }

    pub fn navigate_again(&mut self, target: &[u8]) -> bool {
        // A hostname typed without a scheme is fetched over HTTPS, so the
        // address bar records the address the request actually names.
        let mut normalized = [0u8; 256];
        let target = {
            let n = browser::normalize_address(target, &mut normalized);
            &normalized[..n]
        };

        let n = target.len().min(self.current_url.len());
        self.current_url[..n].copy_from_slice(&target[..n]);
        self.current_url_len = n;
        self.scroll = ScrollState::new(0, READER_VIEWPORT_H, 0);
        // The gateway's reason belongs to the fetch that failed. A new
        // navigation has not failed yet, so the last reason is forgotten
        // rather than shown against a page it says nothing about.
        self.gateway_reason_len = 0;
        self.transport_failure = TransportFailure::None;
        self.connection_failure = ConnectionFailure::None;
        self.protocol_fault = ProtocolFaultKind::None;
        // The security mark is a claim about the document that arrives. This
        // navigation has delivered nothing yet, so the page it replaces keeps
        // its mark in its own history entry and cannot lend it to a URL whose
        // transport has not been tried.
        self.document_tls_verified = false;
        // A navigation is a new document; the previous document's bytes must
        // not be savable under the new address, and the previous page's
        // keyboard must not stay attached to controls that no longer exist.
        self.page_source_len = 0;
        self.html_view = false;
        self.field_focus = NO_FIELD;
        self.address_focused = false;
        self.select_all = false;
        // A submitted body belongs to this navigation only. Reading and
        // clearing it here keeps a later navigation from resending it.
        let mut post_len = self.form_post_len;
        self.form_post_len = 0;

        let parsed = parse_uri(target);
        std::log("[BROWSER APP] Navigation target URL:").ok();
        if let Ok(url) = core::str::from_utf8(target) {
            std::log(url).ok();
        }

        // WHY the address is judged before anything is requested: the
        // browser used to stand in `interos.tech` for an address that named
        // no host, so a page that named nothing was fetched from that site
        // and shown as though the person had asked for it. An address the
        // browser cannot make sense of is reported as such, and nothing is
        // requested.
        match browser::classify_address(target) {
            browser::AddressOutcome::Fetchable => {}
            browser::AddressOutcome::NoScheme | browser::AddressOutcome::NoHost => {
                std::log("[BROWSER APP] The address did not name a host this browser can open; nothing was requested.")
                    .ok();
                self.status = PageStatus::UnrecognisedAddress;
                self.loading_progress = 0;
                self.tabs.active_tab_mut().loading_state = LoadingState::Failed;
                self.tabs.active_tab_mut().error_code = -22; // EINVAL
                let _ = std::audio_haptic_click();
                return false;
            }
            browser::AddressOutcome::UnsupportedScheme => {
                std::log("[BROWSER APP] The address names a scheme this browser cannot fetch; nothing was requested.")
                    .ok();
                self.status = PageStatus::UnsupportedScheme;
                self.loading_progress = 0;
                self.tabs.active_tab_mut().loading_state = LoadingState::Failed;
                self.tabs.active_tab_mut().error_code = -22; // EINVAL
                let _ = std::audio_haptic_click();
                return false;
            }
        }

        // A local document is read from the guest filesystem and read
        // through the same reader pipeline as a fetched page.
        //
        // WHY this is not a stub: the branch used to log that the
        // filesystem refused the document and that the app holds no
        // filesystem capability, without ever asking the filesystem and
        // without checking the capability. The app does hold
        // CAP_FILESYSTEM_READ, and the refusal it reported was one it had
        // assumed.
        if parsed.scheme.is_local() {
            std::log("[BROWSER APP] Address names a local document; reading it from the guest filesystem.",
            )
            .ok();
            return self.load_local_document(&parsed.path);
        }

        // Real Network Socket dispatch with ephemeral client binding
        // The request is built first, so the transport only has to carry
        // bytes.
        let jar = unsafe { &mut *core::ptr::addr_of_mut!(COOKIE_JAR) };
        let cache = unsafe { &mut *core::ptr::addr_of_mut!(HTTP_CACHE) };
        let request_buf = unsafe { &mut *core::ptr::addr_of_mut!(REQUEST_BUF) };
        let hop_buf = unsafe { &mut *core::ptr::addr_of_mut!(HOP_BUF) };
        let start_len = target.len().min(hop_buf.len());
        hop_buf[..start_len].copy_from_slice(&target[..start_len]);
        let mut hop_len = start_len;

        // One navigation owns one method and one hop history. The tracker is
        // what makes "too many redirects" and "this is a loop" different
        // answers rather than one silent truncation.
        let mut method = if post_len > 0 {
            browser::http::HttpMethod::Post
        } else {
            browser::http::HttpMethod::Get
        };
        let mut tracker = browser::http::RedirectTracker::new();
        tracker.start(&hop_buf[..hop_len]);

        let mut document_len = 0usize;
        let mut document_kind = browser::PayloadKind::Markup;
        let mut final_truncated = false;
        let mut invalid_sequences = 0usize;
        let mut redirect_fault: Option<browser::http::RedirectFault> = None;
        let mut protocol_fault = false;

        loop {
            let hop_parsed = parse_uri(&hop_buf[..hop_len]);
            // The scheme alone decides both where this hop dials and whether it
            // wraps in TLS. `|| port == 443` used to be here, which sent an
            // `http://host:443` request into a TLS client hello and, together
            // with the port test inside the transport, let an `https` URL on
            // any other port travel as plaintext.
            let hop_is_https = hop_parsed.scheme == UriScheme::Https;
            let (dst_ip, dst_port) = if hop_is_https {
                if let Ok(h) = core::str::from_utf8(hop_parsed.host) {
                    set_current_server_name(h.as_bytes());
                }
                // A name this device cannot resolve is a destination the person
                // did not reach, and the fallback that used to sit here answered
                // a failed lookup with the QEMU gateway's own address. That is an
                // invented answer: it opens a connection to a peer nobody named
                // while the address field still shows the name they typed, and it
                // makes a resolution failure indistinguishable from a resolution.
                // The refusal is the honest one, and the screen already has a
                // name for it.
                //
                // WHY an IPv4 literal needs no lookup: the address is the answer,
                // so there is nothing to resolve and nothing to refuse.
                let ip = if let Some(ip) = parse_ipv4(hop_parsed.host) {
                    ip
                } else {
                    let name = core::str::from_utf8(hop_parsed.host);
                    match name.ok().and_then(|h| std::dns_resolve(h).ok()) {
                        Some(ip) => ip,
                        None => {
                            self.loading_progress = 0;
                            self.tabs.active_tab_mut().loading_state = LoadingState::Failed;
                            self.status = PageStatus::NoTransport;
                            self.transport_failure = TransportFailure::DnsFailed;
                            std::log("[BROWSER APP] The host name was not resolved; no address was dialled and no request was sent.").ok();
                            let _ = std::audio_play_tone(220);
                            let _ = std::audio_silence();
                            let _ = std::audio_haptic_click();
                            return false;
                        }
                    }
                };
                (ip, if hop_parsed.port != 0 { hop_parsed.port } else { 443 })
            } else {
                // The gateway is a configured proxy, not a resolved host: this
                // address is where the plaintext hop goes by design and the
                // origin it fetches is named inside the request.
                ([10, 0, 2, 2], 4300)
            };

            // Cookies are matched to the address whose turn this is, so a
            // cookie set by a redirect is sent on the next hop and not to
            // the address that set it.
            let cookie_header = unsafe { &mut *core::ptr::addr_of_mut!(COOKIE_HEADER_BUF) };
            let cookie_len = jar.header_for(&hop_buf[..hop_len], 0, cookie_header);

            // A cached document is never assumed fresh: its validator is
            // sent and the origin decides. Without a wall clock this is the
            // only honest conditional request the browser can make.
            let mut conditional_etag = [0u8; 96];
            let mut conditional_modified = [0u8; 40];
            let (conditional_etag_len, conditional_modified_len) = cache.conditional_headers(
                &hop_buf[..hop_len],
                &mut conditional_etag,
                &mut conditional_modified,
            );

            // A form body is carried only while the method that sent it is
            // still POST. A redirect that turns the request into a GET
            // drops the body, which is what the method change means.
            let body: &[u8] = if method == browser::http::HttpMethod::Post && post_len > 0 {
                let posted = unsafe { &*core::ptr::addr_of!(FORM_BODY) };
                &posted[..post_len]
            } else {
                b""
            };
            let content_type: &[u8] = if body.is_empty() {
                b""
            } else {
                b"application/x-www-form-urlencoded"
            };
            let request_len = if hop_is_https {
                browser::format_http_get(hop_parsed.host, hop_parsed.path, request_buf)
            } else {
                browser::http::format_proxy_request(
                    &browser::http::ProxyRequest {
                        method,
                        url: &hop_buf[..hop_len],
                        body,
                        content_type,
                        cookie: &cookie_header[..cookie_len],
                        if_none_match: &conditional_etag[..conditional_etag_len],
                        if_modified_since: &conditional_modified[..conditional_modified_len],
                        accept_gzip: true,
                    },
                    request_buf,
                )
            };

            // The local port has to be non-zero and unique for the connection.
            let local_port = self.take_local_port();

            std::log("[BROWSER APP] Request dispatched over TCP/TLS.").ok();

            let outcome = fetch_over_tcp(
                dst_ip,
                dst_port,
                local_port,
                &request_buf[..request_len],
                MAX_RESPONSE_BYTES,
                hop_is_https,
            );

            // The secure mark follows this reading rather than the scheme the
            // address was typed with. Every hop overwrites it, so a redirect
            // that leaves a secure channel cannot keep the mark behind.
            self.document_tls_verified = outcome.tls_verified;

            // PLATFORM TRUTH: a page is loaded only when the transport
            // actually delivered bytes. The specific failure is reported
            // truthfully according to what stage failed.
            if outcome.bytes == 0 {
                self.loading_progress = 0;
                self.tabs.active_tab_mut().loading_state = LoadingState::Failed;
                self.tabs.active_tab_mut().error_code = outcome.error as i32;

                match outcome.kind {
                    FetchErrorKind::NoTransport => {
                        self.status = PageStatus::NoTransport;
                        self.transport_failure = if outcome.error == -1 {
                            TransportFailure::NoCapability
                        } else {
                            TransportFailure::NetworkDown
                        };
                        std::log("[BROWSER APP] Network transport unavailable; no connection could be initiated.").ok();
                    }
                    FetchErrorKind::EntropyRefused => {
                        self.status = PageStatus::NoTransport;
                        self.transport_failure = TransportFailure::NoEntropy;
                        std::log("[BROWSER APP] The entropy read that picks the connection's sequence number was refused; no socket was opened.").ok();
                    }
                    FetchErrorKind::ConnectionRefused => {
                        self.status = PageStatus::ConnectionFailed;
                        self.connection_failure = ConnectionFailure::Refused;
                        std::log("[BROWSER APP] Connection to the gateway was refused.").ok();
                    }
                    FetchErrorKind::ConnectionTimeout => {
                        self.status = PageStatus::ConnectionFailed;
                        self.connection_failure = ConnectionFailure::Timeout;
                        std::log("[BROWSER APP] Connection to the gateway timed out during TCP handshake.").ok();
                    }
                    FetchErrorKind::ConnectionStalled => {
                        self.status = PageStatus::ConnectionFailed;
                        self.connection_failure = ConnectionFailure::Stall;
                        std::log("[BROWSER APP] Connection to the gateway stalled before response bytes arrived.").ok();
                    }
                    FetchErrorKind::TlsHandshakeFailed => {
                        self.status = PageStatus::ConnectionFailed;
                        // WHY the browser distinguishes the two cases: it can
                        // observe that its own trust store is empty, and "no
                        // trust anchor" is a different fact from "the peer's
                        // certificate was refused". Reporting the one it can
                        // prove keeps the screen from describing a fault it
                        // did not observe.
                        self.connection_failure = if tls_trust_store_is_empty() {
                            ConnectionFailure::TlsNoTrustAnchor
                        } else {
                            ConnectionFailure::TlsFailed
                        };
                        std::log("[BROWSER APP] TLS 1.3 handshake failed; connection terminated securely without sending plaintext.").ok();
                    }
                    FetchErrorKind::TlsNameRefused => {
                        self.status = PageStatus::ConnectionFailed;
                        self.connection_failure = ConnectionFailure::TlsNameRefused;
                        std::log("[BROWSER APP] The server name in the address was refused before the TLS handshake; no connection was opened.").ok();
                    }
                    FetchErrorKind::TlsEntropyRefused => {
                        self.status = PageStatus::ConnectionFailed;
                        self.connection_failure = ConnectionFailure::TlsEntropyRefused;
                        std::log("[BROWSER APP] The TRNG refused the reads that seed the TLS handshake; no ClientHello was sent.").ok();
                    }
                    FetchErrorKind::TlsClockUnavailable => {
                        self.status = PageStatus::ConnectionFailed;
                        self.connection_failure = ConnectionFailure::TlsClockUnavailable;
                        std::log("[BROWSER APP] The RTC answered no instant, so no certificate's expiry could be judged; the connection was closed before the handshake.").ok();
                    }
                    FetchErrorKind::TlsTrustStoreRefused => {
                        self.status = PageStatus::ConnectionFailed;
                        self.connection_failure = ConnectionFailure::TlsTrustStoreRefused;
                        std::log("[BROWSER APP] The kernel refused the trust-anchor read, so the device's store is unknown; the connection was closed before the handshake.").ok();
                    }
                    FetchErrorKind::TlsRecordRefused => {
                        self.status = PageStatus::ConnectionFailed;
                        self.connection_failure = ConnectionFailure::TlsRecordRefused;
                        std::log("[BROWSER APP] The secure channel answered with a record this client refuses; no response data was accepted.").ok();
                    }
                    FetchErrorKind::ZeroBytes | FetchErrorKind::None => {
                        self.status = PageStatus::ZeroBytes;
                        std::log("[BROWSER APP] The gateway returned zero bytes. No document data was received.").ok();
                    }
                }
                let _ = std::audio_play_tone(220);
                let _ = std::audio_silence();
                let _ = std::audio_haptic_click();
                return false;
            }

            let response = unsafe { &*core::ptr::addr_of!(RESPONSE_BUF) };
            let rx_slice = &response[..outcome.bytes];
            self.response_len = outcome.bytes;

            {
                let mut log_buf = [0u8; 128];
                let mut lp = 0;
                let p1 = b"[BROWSER APP] fetch_over_tcp bytes=";
                log_buf[lp..lp+p1.len()].copy_from_slice(p1);
                lp += p1.len();
                lp = browser::append_decimal(&mut log_buf, lp, outcome.bytes as u64);
                let p2 = b" trunc=";
                log_buf[lp..lp+p2.len()].copy_from_slice(p2);
                lp += p2.len();
                log_buf[lp] = if outcome.truncated { b'1' } else { b'0' };
                lp += 1;
                let p3 = b" err=";
                log_buf[lp..lp+p3.len()].copy_from_slice(p3);
                lp += p3.len();
                lp = browser::append_decimal(&mut log_buf, lp, outcome.error.unsigned_abs());
                if let Ok(s) = core::str::from_utf8(&log_buf[..lp]) {
                    let _ = std::log(s);
                }
            }

            // A payload that is a file rather than a document is kept as it
            // arrived. The gateway hands media over as raw bytes; decoding it
            // as text first would replace every non-ASCII byte, so the saved
            // file would no longer be the file the origin sent.
            if let Some(resp) = browser::parse_http_response(rx_slice) {
                if browser::content_type_is_binary(resp.content_type) {
                    // WHY the status is checked before the bytes are weighed:
                    // an error answer arrives with a body of its own, and a
                    // browser that saves whatever arrived stores the error
                    // text under the name of the file that was asked for.
                    if !browser::status_carries_file(resp.status_code) {
                        self.fail_download(
                            DownloadFailure::OriginRefused,
                            "[BROWSER APP] The origin answered with an error instead of the file; nothing was saved.",
                        );
                        return true;
                    }
                    // WHY the body is weighed against the origin's own
                    // number: the headers can arrive without the bytes they
                    // announce, and a file written from what arrived would
                    // then be an empty document that the File Manager and
                    // the Gallery list as a real file. The origin's declared
                    // length is the only statement about how long the file
                    // should have been.
                    let body_state = browser::classify_download_body(rx_slice, resp.body.len());
                    let truncated = outcome.truncated
                        || browser::http_header_value(rx_slice, b"x-interos-truncated").is_some()
                        || matches!(body_state, browser::DownloadBody::Short { .. });
                    match body_state {
                        // WHY the empty body is refused here and not offered:
                        // the response carried headers and no bytes, so
                        // there is no file to download. Nothing is written,
                        // and the screen says the origin sent no bytes
                        // rather than reporting a storage refusal.
                        browser::DownloadBody::Empty { .. } => {
                            self.fail_download(
                                DownloadFailure::NoBytes,
                                "[BROWSER APP] The origin sent a file with no bytes in it; there was no file to save and nothing was saved.",
                            );
                            return true;
                        }
                        _ => {
                            self.stage_pending_download(
                                &hop_buf[..hop_len],
                                resp.content_type,
                                browser::http_header_value(rx_slice, b"x-interos-filename"),
                                resp.body,
                                truncated,
                            );
                            return true;
                        }
                    }
                }
            }

            if is_browser_envelope(rx_slice) {
                final_truncated = outcome.truncated
                    || rx_slice
                        .windows(b"\"truncated\": true".len())
                        .any(|w| w == b"\"truncated\": true")
                    || rx_slice
                        .windows(b"\"truncated\":true".len())
                        .any(|w| w == b"\"truncated\":true");

                let status = match browser::http::proxy_status(rx_slice) {
                    Some(status) => status,
                    None => 0,
                };

                // Every Set-Cookie on every hop belongs to the address that
                // sent it, including the redirect responses themselves.
                let mut cookie_text = [0u8; 1536];
                let cookie_text_len = envelope_field(rx_slice, b"set_cookie", &mut cookie_text);
                if cookie_text_len > 0 {
                    for line in cookie_text[..cookie_text_len].split(|&b| b == b'\n') {
                        if !line.is_empty() {
                            let _ = jar.store(&hop_buf[..hop_len], line, 0);
                        }
                    }
                }

                if status == 304 {
                    // The origin confirmed the copy the browser holds. The
                    // body is served from the cache, not invented.
                    if let Some(entry) = cache.lookup(&hop_buf[..hop_len]) {
                        let html = unsafe { &mut *core::ptr::addr_of_mut!(HTML_BUF) };
                        let body = entry.body();
                        document_len = body.len().min(html.len());
                        html[..document_len].copy_from_slice(&body[..document_len]);
                        std::log("[BROWSER APP] The origin confirmed the cached copy; the revalidated document is shown.").ok();
                        break;
                    }
                    protocol_fault = true;
                    break;
                }

                if browser::http::status_is_redirect(status) {
                    let mut location = [0u8; browser::http::MAX_URL_BYTES];
                    let location_len = envelope_field(rx_slice, b"location", &mut location);
                    if location_len == 0 {
                        protocol_fault = true;
                        break;
                    }
                    let mut next = [0u8; browser::http::MAX_URL_BYTES];
                    let Some(next_len) = browser::http::resolve_url(
                        &hop_buf[..hop_len],
                        &location[..location_len],
                        &mut next,
                    ) else {
                        protocol_fault = true;
                        break;
                    };
                    match tracker.next(&next[..next_len]) {
                        Ok(()) => {
                            method = browser::http::method_for_redirect(status, method);
                            if method != browser::http::HttpMethod::Post {
                                post_len = 0;
                            }
                            hop_buf[..next_len].copy_from_slice(&next[..next_len]);
                            hop_len = next_len;
                            std::log("[BROWSER APP] The origin redirected; the browser follows the Location under its own hop bound.").ok();
                            continue;
                        }
                        Err(fault) => {
                            redirect_fault = Some(fault);
                            break;
                        }
                    }
                }

                // The gateway's envelope names the page's own type, which is
                // what decides markup versus plain text.
                let mut content_type = [0u8; 160];
                let mut content_type_len =
                    envelope_field(rx_slice, b"content_type", &mut content_type);
                if content_type_len == 0 {
                    if let Some(ct) = browser::proxy_content_type(rx_slice) {
                        content_type_len = ct.len().min(content_type.len());
                        content_type[..content_type_len].copy_from_slice(&ct[..content_type_len]);
                    }
                }
                document_kind = if browser::content_type_is_plain(
                    &content_type[..content_type_len],
                ) {
                    browser::PayloadKind::PlainText
                } else {
                    browser::PayloadKind::Markup
                };

                let mut encoding = [0u8; 32];
                let encoding_len = envelope_field(rx_slice, b"content_encoding", &mut encoding);
                let charset =
                    browser::http::charset_from_content_type(&content_type[..content_type_len]);

                let html = unsafe { &mut *core::ptr::addr_of_mut!(HTML_BUF) };
                let decoded_buf = unsafe { &mut *core::ptr::addr_of_mut!(DECODE_BUF) };
                if let Some(encoded) = browser::http::proxy_string_field(rx_slice, b"body_b64") {
                    // The gateway base64-encodes a body whose bytes must not
                    // be re-encoded as JSON text: a compressed body, or one
                    // in a charset other than UTF-8.
                    let Some(wire_len) = browser::http::base64_decode(encoded, html) else {
                        protocol_fault = true;
                        break;
                    };
                    if encoding_len > 0 {
                        // A body that outgrows the buffer is a body this
                        // browser cannot hold, not a body that failed to
                        // decode. The decoder fills the buffer before it
                        // reports the limit, so the beginning of the page is
                        // real markup and is shown with the truncation the
                        // reader is already told about; refusing it lost the
                        // whole site over its size.
                        let inflated = match browser::inflate::decode_content_encoding(
                            &encoding[..encoding_len],
                            &html[..wire_len],
                            decoded_buf,
                        ) {
                            Ok(inflated) => inflated,
                            Err(browser::inflate::InflateError::OutputLimit) => {
                                final_truncated = true;
                                decoded_buf.len()
                            }
                            Err(_) => {
                                protocol_fault = true;
                                break;
                            }
                        };
                        let decoded = browser::http::decode_text(
                            &decoded_buf[..inflated],
                            charset,
                            html,
                        );
                        document_len = decoded.written;
                        final_truncated |= decoded.truncated;
                        invalid_sequences += decoded.invalid;
                    } else {
                        let decoded =
                            browser::http::decode_text(&html[..wire_len], charset, decoded_buf);
                        document_len = decoded.written;
                        final_truncated |= decoded.truncated;
                        invalid_sequences += decoded.invalid;
                        html[..document_len].copy_from_slice(&decoded_buf[..document_len]);
                    }
                } else {
                    document_len = browser::extract_proxy_html(rx_slice, html);
                    let decoded =
                        browser::http::decode_text(&html[..document_len], charset, decoded_buf);
                    document_len = decoded.written;
                    final_truncated |= decoded.truncated;
                    invalid_sequences += decoded.invalid;
                    html[..document_len].copy_from_slice(&decoded_buf[..document_len]);
                }

                // A truncated body is held in memory for this navigation and
                // not put in the cache: the validators the origin sent describe
                // the whole document, so revalidating against a part of it
                // would turn a 304 into a page shown as complete.
                if document_len > 0
                    && status == 200
                    && !final_truncated
                    && document_kind == browser::PayloadKind::Markup
                {
                    let mut etag = [0u8; 96];
                    let etag_len = envelope_field(rx_slice, b"etag", &mut etag);
                    let mut modified = [0u8; 40];
                    let modified_len = envelope_field(rx_slice, b"last-modified", &mut modified);
                    let mut cache_control = [0u8; 64];
                    let cache_control_len =
                        envelope_field(rx_slice, b"cache-control", &mut cache_control);
                    let _ = cache.store(
                        &hop_buf[..hop_len],
                        &etag[..etag_len],
                        &modified[..modified_len],
                        &cache_control[..cache_control_len],
                        &html[..document_len],
                    );
                }
                break;
            }

            // The gateway reports a failed fetch as HTTP 200 with
            // `success: false` and no page. That body is not the site's
            // content, so it is reported as a failure instead of being
            // rendered as text.
            if browser::proxy_reports_failure(rx_slice) {
                self.status = PageStatus::GatewayError;
                self.loading_progress = 0;
                self.tabs.active_tab_mut().loading_state = LoadingState::Failed;
                self.tabs.active_tab_mut().error_code = -5; // EIO
                // The envelope names the reason the fetch failed. It is the
                // host's own account of what happened, and the screen shows
                // it instead of a message the browser made up.
                self.record_gateway_reason(rx_slice);
                std::log("[BROWSER APP] The gateway received the request but could not fetch the page; its error envelope is reported, not rendered.").ok();
                let _ = std::audio_haptic_click();
                return false;
            }

            // A response outside the envelope is read as an HTTP response of
            // its own. The binary case returned above; what is left is text.
            let Some(resp) = browser::parse_http_response(rx_slice) else {
                protocol_fault = true;
                break;
            };
            if resp.body.is_empty() {
                protocol_fault = true;
                break;
            }
            document_kind = if browser::content_type_is_plain(resp.content_type) {
                browser::PayloadKind::PlainText
            } else {
                browser::PayloadKind::Markup
            };
            // `parse_http_response` strips the parameters, and the charset
            // lives there, so the raw header value is used for the decoder.
            let full_content_type =
                browser::http::find_header(rx_slice, b"content-type").unwrap_or(resp.content_type);
            let charset = browser::http::charset_from_content_type(full_content_type);
            let html = unsafe { &mut *core::ptr::addr_of_mut!(HTML_BUF) };
            let decoded = browser::http::decode_text(resp.body, charset, html);
            document_len = decoded.written;
            final_truncated = outcome.truncated || decoded.truncated;
            invalid_sequences += decoded.invalid;
            break;
        }

        if let Some(fault) = redirect_fault {
            // The fault is shown as a page of its own: it names the limit or
            // the loop and the last address, and invents no status code.
            let text = unsafe { &mut *core::ptr::addr_of_mut!(RESPONSE_BUF) };
            let text_len = browser::http::redirect_error_document(
                fault,
                &hop_buf[..hop_len],
                text,
            );
            let document = unsafe { &mut *core::ptr::addr_of_mut!(READER_BUF) };
            let doc_len = browser::reader_text_from_plain(&text[..text_len], document);
            if doc_len == 0 {
                protocol_fault = true;
            } else {
                self.page_source_len = 0;
                self.response_truncated = false;
                self.load_document_content(&document[..doc_len]);
                self.status = PageStatus::Loaded;
                self.loading_progress = 100;
                self.tabs.active_tab_mut().loading_state = LoadingState::Completed;
                std::log("[BROWSER APP] The redirect bound stopped this navigation; the page states the bound and the last address.").ok();
                let _ = std::audio_haptic_click();
                return true;
            }
        }

        {
            let response = unsafe { &*core::ptr::addr_of!(RESPONSE_BUF) };
            let rx_slice = &response[..self.response_len];
            let mut log_buf2 = [0u8; 128];
            let mut lp2 = 0;
            let d1 = b"[BROWSER APP] decode: doc_len=";
            log_buf2[lp2..lp2+d1.len()].copy_from_slice(d1);
            lp2 += d1.len();
            lp2 = browser::append_decimal(&mut log_buf2, lp2, document_len as u64);
            let d2 = b" proto_fault=";
            log_buf2[lp2..lp2+d2.len()].copy_from_slice(d2);
            lp2 += d2.len();
            log_buf2[lp2] = if protocol_fault { b'1' } else { b'0' };
            lp2 += 1;
            let d3 = b" is_envelope=";
            log_buf2[lp2..lp2+d3.len()].copy_from_slice(d3);
            lp2 += d3.len();
            log_buf2[lp2] = if is_browser_envelope(rx_slice) { b'1' } else { b'0' };
            lp2 += 1;
            if let Ok(s) = core::str::from_utf8(&log_buf2[..lp2]) {
                let _ = std::log(s);
            }
        }

        if protocol_fault || document_len == 0 {
            self.status = PageStatus::ProtocolFault;
            if self.protocol_fault == ProtocolFaultKind::None {
                self.protocol_fault = if protocol_fault {
                    ProtocolFaultKind::MalformedResponse
                } else {
                    ProtocolFaultKind::UnparseableDocument
                };
            }
            self.loading_progress = 0;
            self.tabs.active_tab_mut().loading_state = LoadingState::Failed;
            self.tabs.active_tab_mut().error_code = -5; // EIO
            std::log("[BROWSER APP] The response was not a document this browser could decode; no page was synthesised.").ok();
            let _ = std::audio_haptic_click();
            return false;
        }

        if invalid_sequences > 0 {
            // Explicit, not silent: the bytes were neither valid UTF-8 nor
            // valid for the declared charset, and each bad sequence became
            // U+FFFD rather than disappearing.
            std::log("[BROWSER APP] The document held byte sequences invalid in its declared charset; they were replaced, not dropped.").ok();
        }

        // The page's own document, not the envelope that carried it, is what
        // a saved page is.
        self.page_source_len = document_len;
        // The HTML is read from one buffer and the reader document is written
        // to another: the reader rewrites the markup in place, so source and
        // destination cannot be the same memory.
        let source = unsafe { &*core::ptr::addr_of!(HTML_BUF) };
        let mut loaded = false;
        if document_kind == browser::PayloadKind::Markup {
            loaded = self.load_html_document(&source[..document_len]);
        }
        if !loaded {
            let destination = unsafe { &mut *core::ptr::addr_of_mut!(READER_BUF) };
            let doc_len = browser::html_to_reader_document_kind(
                &source[..document_len],
                document_kind,
                destination,
            );
            if doc_len == 0 {
                self.status = PageStatus::ProtocolFault;
                self.protocol_fault = ProtocolFaultKind::EmptyDocument;
                self.loading_progress = 0;
                self.tabs.active_tab_mut().loading_state = LoadingState::Failed;
                self.tabs.active_tab_mut().error_code = -5; // EIO
                std::log("[BROWSER APP] The page carried no readable text after reader conversion; no page was synthesised.").ok();
                return false;
            }
            self.load_document_content(&destination[..doc_len]);
        }

        if final_truncated {
            // WHY the screen says so: the response was larger than the
            // browser's buffer, so what is on screen is the beginning of a
            // page rather than the whole of it.
            std::log("[BROWSER APP] The response was larger than the browser's buffer; the page is shown truncated.",
            )
            .ok();
            self.response_truncated = true;
        } else {
            self.response_truncated = false;
        }

        self.status = PageStatus::Loaded;
        self.pages_loaded += 1;
        self.loading_progress = 100;
        self.tabs.active_tab_mut().loading_state = LoadingState::Completed;

        let _ = std::audio_play_tone(784);
        let _ = std::audio_silence();
        let _ = std::audio_haptic_click();
        true
    }

    pub fn is_loading(&self) -> bool {
        matches!(
            self.tabs.active_tab().loading_state,
            LoadingState::Connecting | LoadingState::Receiving
        ) || self.speed_running
            || self.lab_running
    }

    pub fn stop(&mut self) {
        if matches!(
            self.tabs.active_tab().loading_state,
            LoadingState::Connecting | LoadingState::Receiving
        ) {
            self.tabs.active_tab_mut().loading_state = LoadingState::Completed;
        }
        if self.speed_running {
            self.speed_running = false;
            self.pending = PendingTask::None;
        }
        if self.lab_running {
            self.lab_running = false;
            self.pending = PendingTask::None;
        }
        if self.status == PageStatus::DownloadPrompt {
            self.refuse_pending_download();
        }
        std::log("[BROWSER APP] Stop: navigation / loading stopped.").ok();
    }

    pub fn reload(&mut self) {
        let mut target = [0u8; 256];
        let n = self.current_url_len.min(target.len());
        target[..n].copy_from_slice(&self.current_url[..n]);
        self.navigate_again(&target[..n]);
    }

    /// Returns the browser to its start page and forgets the document.
    ///
    /// WHY this is a state of its own rather than a navigation: the tab
    /// stack begins at `about:blank`, so Back from the first page reaches
    /// it. Sending `about:blank` through the address classifier made the
    /// browser report "Address not recognised" for the start page and kept
    /// the previous document's address in the bar.
    fn show_home(&mut self) {
        let start = b"about:blank";
        self.current_url[..start.len()].copy_from_slice(start);
        self.current_url_len = start.len();
        self.address_len = 0;
        self.address_focused = false;
        self.select_all = false;
        self.status = PageStatus::Blank;
        self.scroll = ScrollState::new(0, READER_VIEWPORT_H, 0);
        self.page_source_len = 0;
        self.html_view = false;
        self.field_focus = NO_FIELD;
        self.form_post_len = 0;
        self.doc_line_count = 0;
        self.doc_total_lines = 0;
        self.image_count = 0;
        self.image_arena_used = 0;
        self.favicon = None;
        self.response_truncated = false;
        self.local_failure = LocalFailure::NoPath;
        self.download_failure = DownloadFailure::None;
        self.gateway_reason_len = 0;
        self.transport_failure = TransportFailure::None;
        self.connection_failure = ConnectionFailure::None;
        self.protocol_fault = ProtocolFaultKind::None;
        self.response_len = 0;
        self.loading_progress = 0;
        self.error_code = 0;
    }

    pub fn go_back(&mut self) {
        if let Some(url) = self.tabs.go_back() {
            let mut target = [0u8; 256];
            let n = url.len().min(target.len());
            target[..n].copy_from_slice(&url[..n]);
            if n == 0 || &target[..n] == b"about:blank" {
                self.show_home();
                return;
            }
            std::log("[BROWSER APP] Back: re-issuing navigation attempt.").ok();
            self.navigate_again(&target[..n]);
            return;
        }
        std::log("[BROWSER APP] Back is unavailable: the history holds no earlier address.").ok();
    }

    pub fn go_forward(&mut self) {
        if let Some(url) = self.tabs.go_forward() {
            let mut target = [0u8; 256];
            let n = url.len().min(target.len());
            target[..n].copy_from_slice(&url[..n]);
            if n == 0 || &target[..n] == b"about:blank" {
                self.show_home();
                return;
            }
            std::log("[BROWSER APP] Forward: re-issuing navigation attempt.").ok();
            self.navigate_again(&target[..n]);
            return;
        }
        std::log("[BROWSER APP] Forward is unavailable: the history holds no later address.").ok();
    }

    pub fn start_add_bookmark(&mut self) {
        self.bm_step = BookmarkStep::Title;
        self.address_focused = true;
        self.address_len = 0;
        self.select_all = false;
        std::log("[BROWSER APP] Adding a bookmark: type the title, then the address.").ok();
    }

    pub fn commit_bookmark_step(&mut self) -> Result<(), i64> {
        match self.bm_step {
            BookmarkStep::Idle => Ok(()),
            BookmarkStep::Title => {
                if self.address_len == 0 {
                    std::log("[BROWSER APP] Bookmark not added: a bookmark needs a title.").ok();
                    return Err(-22);
                }
                let n = self.address_len.min(self.bm_title.len());
                self.bm_title[..n].copy_from_slice(&self.address[..n]);
                self.bm_title_len = n;
                self.bm_step = BookmarkStep::Address;
                self.address_len = 0;
                self.select_all = false;
                std::log("[BROWSER APP] Bookmark title set; type the address.").ok();
                Ok(())
            }
            BookmarkStep::Address => {
                if self.address_len == 0 {
                    std::log("[BROWSER APP] Bookmark not added: a bookmark needs an address.").ok();
                    return Err(-22);
                }
                if self.address_len > BM_URL_MAX {
                    self.address_len = BM_URL_MAX;
                }
                let title_len = self.bm_title_len.min(BM_TITLE_MAX);
                let outcome = self.bookmarks.add(&self.bm_title[..title_len], &self.address[..self.address_len]);
                self.bm_step = BookmarkStep::Idle;
                self.address_len = 0;
                self.address_focused = false;
                self.select_all = false;
                self.bm_title_len = 0;
                match outcome {
                    Ok(()) => {
                        std::log("[BROWSER APP] Bookmark written to the settings store and acknowledged.").ok();
                    }
                    Err(_) => {
                        std::log("[BROWSER APP] Bookmark was NOT saved: the kernel refused the settings write. It is held in the unsaved group for this session only.").ok();
                    }
                }
                outcome
            }
        }
    }

    pub fn cancel_bookmark_step(&mut self) {
        self.bm_step = BookmarkStep::Idle;
        self.address_len = 0;
        self.address_focused = false;
        self.select_all = false;
        self.bm_title_len = 0;
    }
}

// -----------------------------------------------------------------------------
// Per-Frame State Cache
// -----------------------------------------------------------------------------
struct Frame {
    pill: [u8; 96],
    pill_len: usize,
    pill_color: u32,
    show_caret: bool,
    caret_x: usize,
    select_w: usize,
    head: [u8; 96],
    head_len: usize,
    head_scale: usize,
    head_x: usize,
    head_y: usize,
    body: [&'static [u8]; 6],
    body_n: usize,
    body_x: [usize; 6],
    body_y: [usize; 6],
    /// The typed address as the suggestion cards show it.
    ///
    /// WHY it is fitted here and not where it is drawn: the renderer
    /// answers one pixel at a time, and a string that is cut to a row's
    /// width needs that width, which is a constant of the row and not a
    /// per-pixel decision.
    suggest: [u8; 256],
    suggest_len: usize,
    /// The gateway's own reason for the failure, as one marked line.
    detail: [u8; browser::GATEWAY_REASON_LINE_MAX],
    detail_len: usize,
    detail_y: usize,
    /// Where the failure badge is drawn.
    ///
    /// WHY it is in the frame: the badge sits below the body text, and a
    /// gateway failure puts the server's own reason between the two, so the
    /// badge's place depends on the frame rather than on a formula each
    /// place repeats.
    err_y: usize,
    icon_cy: usize,
    is_home: bool,
    /// The band's clock, as the platform's RTC answered it for this frame.
    clock: [u8; 5],
    /// Whether that answer came at all, and so whether the lane is inked.
    clock_ok: bool,
}

impl Frame {
    fn build(app: &BrowserApp) -> Self {
        let mut f = Self {
            pill: [0; 96],
            pill_len: 0,
            pill_color: THEME.on_surface_variant,
            show_caret: false,
            caret_x: PILL_TEXT_X,
            select_w: 0,
            head: [0; 96],
            head_len: 0,
            head_scale: 1,
            head_x: 0,
            head_y: 0,
            body: [&b""[..]; 6],
            body_n: 0,
            body_x: [0; 6],
            body_y: [0; 6],
            suggest: [0; 256],
            suggest_len: 0,
            detail: [0; browser::GATEWAY_REASON_LINE_MAX],
            detail_len: 0,
            detail_y: 0,
            err_y: 0,
            icon_cy: 0,
            is_home: app.status == PageStatus::Blank,
            clock: [b' '; 5],
            clock_ok: false,
        };

        // WHY the clock is read once for the frame and carried in it: the
        // renderer answers one pixel at a time, and a band that asked the RTC
        // for each of the thirty-four thousand pixels it covers would spend the
        // whole frame in syscalls. WHY the read is not hoisted further: the
        // frame is built once per repaint, so the clock the band prints is the
        // one the platform gave for this frame.
        if let Some(now) = std::rtc::Reading::now() {
            f.clock = now.clock_text();
            f.clock_ok = true;
        }

        let src: &[u8] = if app.address_len > 0 {
            &app.address[..app.address_len]
        } else if app.address_focused {
            &app.address[..0]
        } else if app.current_url_len > 0 && &app.current_url[..app.current_url_len] != b"about:blank" {
            app.current_url()
        } else {
            PLACEHOLDER
        };

        f.pill_color = if app.address_len > 0
            || (app.current_url_len > 0 && &app.current_url[..app.current_url_len] != b"about:blank")
        {
            THEME.on_surface
        } else {
            THEME.on_surface_variant
        };
        f.pill_len = font::ellipsize(src, 1, PILL_TEXT_MAX, &mut f.pill);
        f.show_caret = app.address_focused;
        f.caret_x = PILL_TEXT_X + text_width(&f.pill[..f.pill_len], 1);
        f.select_w = if app.select_all {
            text_width(&f.pill[..f.pill_len], 1)
        } else {
            0
        };

        let (head_src, body_src): (&[u8], &[u8]) = match app.status {
            PageStatus::Blank => (b"InterOS Browser", BODY_BLANK),
            PageStatus::Loaded => (b"Web Documents", BODY_LOADED),
            PageStatus::NoTransport => match app.transport_failure {
                TransportFailure::NetworkDown => (b"Network unavailable", BODY_NETWORK_DOWN),
                TransportFailure::DnsFailed => (b"Address cannot be resolved", BODY_DNS_FAILED),
                TransportFailure::NoEntropy => (b"No starting number", BODY_NO_ENTROPY),
                _ => (b"Navigation unavailable", BODY_NO_TRANSPORT),
            },
            PageStatus::ConnectionFailed => match app.connection_failure {
                ConnectionFailure::Refused => (b"Connection refused", BODY_CONNECTION_REFUSED),
                ConnectionFailure::Timeout => (b"Connection timed out", BODY_CONNECTION_TIMEOUT),
                ConnectionFailure::Stall => (b"Connection stalled", BODY_CONNECTION_STALLED),
                ConnectionFailure::TlsFailed => (b"Security check failed", BODY_TLS_HANDSHAKE_FAILED),
                ConnectionFailure::TlsNoTrustAnchor => (b"No trust anchor", BODY_TLS_NO_TRUST_ANCHOR),
                ConnectionFailure::TlsNameRefused => (b"Address refused", BODY_TLS_NAME_REFUSED),
                ConnectionFailure::TlsEntropyRefused => (b"No key entropy", BODY_TLS_ENTROPY_REFUSED),
                ConnectionFailure::TlsClockUnavailable => (b"No clock reading", BODY_TLS_CLOCK_UNAVAILABLE),
                ConnectionFailure::TlsTrustStoreRefused => (b"Trust store unreadable", BODY_TLS_TRUST_STORE_REFUSED),
                ConnectionFailure::TlsRecordRefused => (b"Answer refused here", BODY_TLS_RECORD_REFUSED),
                ConnectionFailure::None => (b"Connection failed", BODY_CONNECTION_TIMEOUT),
            },
            PageStatus::ZeroBytes => (b"No data received", BODY_ZERO_BYTES),
            PageStatus::ProtocolFault => match app.protocol_fault {
                ProtocolFaultKind::MalformedResponse => (b"Protocol fault", BODY_PROTOCOL_MALFORMED),
                ProtocolFaultKind::InvalidEncoding => (b"Decoding failed", BODY_PROTOCOL_ENCODING),
                ProtocolFaultKind::UnparseableDocument => (b"Unparseable document", BODY_PROTOCOL_UNPARSEABLE),
                ProtocolFaultKind::EmptyDocument => (b"Document empty", BODY_PROTOCOL_EMPTY),
                ProtocolFaultKind::None => (b"Protocol fault", BODY_PROTOCOL_MALFORMED),
            },
            PageStatus::InvalidAddress => (b"Address cannot be opened", BODY_INVALID),
            PageStatus::UnrecognisedAddress => (b"Address not recognised", BODY_UNRECOGNISED),
            PageStatus::UnsupportedScheme => (b"Scheme not supported", BODY_UNSUPPORTED_SCHEME),
            PageStatus::GatewayError => (b"Gateway could not fetch", BODY_GATEWAY_ERROR),
            PageStatus::Downloaded => {
                if app.last_download_truncated {
                    (b"Saved to Downloads (partial)", BODY_DOWNLOADED_PARTIAL)
                } else {
                    (b"Saved to Downloads", BODY_DOWNLOADED)
                }
            }
            PageStatus::DownloadFailed => match app.download_failure {
                DownloadFailure::NoBytes => (b"Download Not Saved", BODY_DOWNLOAD_NO_BYTES),
                DownloadFailure::NoPath => (b"Download could not be saved", BODY_DOWNLOAD_NO_PATH),
                DownloadFailure::Lost => (b"Download could not be saved", BODY_DOWNLOAD_LOST),
                DownloadFailure::OriginRefused => (b"Download Not Saved", BODY_DOWNLOAD_REFUSED),
                DownloadFailure::StorageRefused | DownloadFailure::None => {
                    (b"Download could not be saved", BODY_DOWNLOAD_FAILED)
                }
            },
            PageStatus::DownloadPrompt => {
                if app.pending_download.truncated {
                    (b"Download Permission", BODY_DOWNLOAD_PROMPT_PARTIAL)
                } else {
                    (b"Download Permission", BODY_DOWNLOAD_PROMPT)
                }
            }
            PageStatus::DownloadRefused => (b"Download Refused", BODY_DOWNLOAD_DECLINED),
            PageStatus::LocalRefused => (b"Local document unavailable", match app.local_failure {
                LocalFailure::NoPath => BODY_LOCAL_NO_PATH,
                LocalFailure::NoCapability => BODY_LOCAL_NO_CAPABILITY,
                LocalFailure::Refused => BODY_LOCAL_REFUSED,
                LocalFailure::Empty => BODY_LOCAL_EMPTY,
                LocalFailure::ReadRefused => BODY_LOCAL_READ_REFUSED,
            }),
        };

        f.head_scale = 1;
        let n = head_src.len().min(f.head.len());
        f.head[..n].copy_from_slice(&head_src[..n]);
        f.head_len = n;
        let head_w = text_width(&f.head[..f.head_len], 1);
        f.head_x = (W.saturating_sub(head_w)) / 2;

        let mut lines: [&'static [u8]; 6] = [&b""[..]; 6];
        f.body_n = font::wrap_text(body_src, 1, 560, 6, &mut lines);
        for i in 0..f.body_n {
            f.body[i] = lines[i];
            f.body_x[i] = (W.saturating_sub(text_width(lines[i], 1))) / 2;
        }

        f.icon_cy = 380;
        f.head_y = 440;
        for i in 0..f.body_n {
            f.body_y[i] = f.head_y + 40 + i * 28;
        }

        // The candidate the suggestion cards offer.
        //
        // WHY it is fitted and not truncated: a candidate that stops at the
        // edge of the row with no mark reads as a whole address, and the
        // address a person is about to open would not be the address they
        // were shown.
        f.suggest_len = browser::fit_suggestion(&app.address[..app.address_len], &mut f.suggest);

        // The gateway's own reason for the failure, when it gave one.
        //
        // WHY it is a line of its own: the body says what the browser
        // knows and this says what the server said, and a reader has to be
        // able to tell the two apart.
        f.detail_len = if app.status == PageStatus::GatewayError && app.gateway_reason_len > 0 {
            browser::gateway_reason_line(
                &app.gateway_reason[..app.gateway_reason_len],
                &mut f.detail,
            )
        } else {
            0
        };
        f.detail_y = f.head_y + 40 + f.body_n * 28 + 16;
        // The badge sits below the body, and below the reason when there
        // is one.
        f.err_y = f.head_y + 40 + f.body_n * 28 + 24;
        if f.detail_len > 0 {
            f.err_y = f.detail_y + 28 + 20;
        }

        f
    }
}

// -----------------------------------------------------------------------------
// Glyphs & Vector Drawing
// -----------------------------------------------------------------------------
/// The colour of (`x`, `y`) if the text inks it, blended over `bg`.
///
/// WHY the outline face comes first: the compositor magnifies the frame
/// three to two, and the bitmap face is a twenty-four row cell blown up
/// to thirty-six, which reads soft. The outline face is drawn at the size
/// the screen uses, so the magnified text keeps its edges.
///
/// WHY the baseline is the bitmap's baseline: the outline face's ascent
/// is taller than the bitmap cell's, so placing the run the way the
/// bitmap's baseline is placed keeps every label where the layout put
/// it. This is the shell's own convention.
#[inline(always)]
fn text_px(
    x: usize,
    y: usize,
    x0: usize,
    y0: usize,
    scale: usize,
    text: &[u8],
    color: u32,
    bg: u32,
    bold: bool,
) -> Option<u32> {
    if scale == 0 || text.is_empty() {
        return None;
    }
    let em = outline_text::em_for_scale(scale);
    let (ascent, _) = interos_std::design::typeface::line_metrics(em);
    let baseline = (y0 + font::ascent_for_scale(scale)).max(ascent as usize);
    if let Some(run) = outline_text::OutlineRun::place_styled(x0, baseline, em, text, bold) {
        let (bx, by, bw, bh) = run.bounds();
        if x >= bx && x < bx + bw && y >= by && y < by + bh {
            return run.pixel(x, y, color, bg);
        }
        return None;
    }
    if x < x0 || y < y0 || y >= y0 + font::line_height(scale) {
        return None;
    }
    font::test_text_aa(x, y, x0, y0, scale, text, color, bg, bold)
}

/// The width of a string as the reader draws it.
///
/// WHY the outline face's advance and not the bitmap's: the text is
/// drawn with the outline face, so a caret or a centred label measured
/// with the bitmap's width sits in the wrong place.
fn text_width(text: &[u8], scale: usize) -> usize {
    outline_text::drawn_advance(text, scale, None).unwrap_or_else(|| font::measure(text, scale))
}

/// Blends a colour toward another by a fixed amount in 0..=255.
///
/// WHY this exists: a scrim has to darken whatever is behind it without
/// erasing it, and the surface is a straight pixel buffer, so the blend
/// has to be written here.
fn blend_toward(colour: u32, toward: u32, amount: u32) -> u32 {
    let amount = amount.min(255) as i64;
    let mix = |c: u32, t: u32| -> u32 {
        let c = c as i64;
        let t = t as i64;
        (((c * (255 - amount)) + (t * amount)) / 255) as u32
    };
    (colour & 0xFF00_0000)
        | (mix((colour >> 16) & 0xFF, (toward >> 16) & 0xFF) << 16)
        | (mix((colour >> 8) & 0xFF, (toward >> 8) & 0xFF) << 8)
        | mix(colour & 0xFF, toward & 0xFF)
}

/// SSL Padlock Icon, centred on the box the page's picture would occupy.
fn padlock_glyph(x: usize, y: usize, cx: usize, cy: usize) -> bool {
    let dx = x as isize - cx as isize;
    let dy = y as isize - cy as isize;
    if dy >= -6 && dy <= -2 {
        let arc = dx.abs() >= 2 && dx.abs() <= 4;
        let top = dy == -6 && dx.abs() <= 3;
        if arc || top {
            return true;
        }
    }
    if dy >= -1 && dy <= 5 && dx.abs() <= 5 {
        if dy == 1 && dx.abs() <= 1 {
            return false;
        }
        if dy == 2 && dx == 0 {
            return false;
        }
        return true;
    }
    false
}

/// Whether an address names a picture this browser was built without a
/// decoder for.
///
/// WHY by the name rather than by asking the origin: the format a page most
/// often picks for its own picture that this platform cannot rasterise is a
/// vector one, and fetching a file whose answer is already known costs a
/// round trip on the way to a refusal. A name that lies about its type still
/// reaches the decoder and is refused there, so the check only ever saves a
/// fetch; it never hides a picture that could have been drawn.
fn undrawable_picture(href: &[u8]) -> bool {
    let Some(dot) = href.iter().rposition(|&b| b == b'.') else {
        return false;
    };
    href[dot + 1..].eq_ignore_ascii_case(b"svg")
}

/// The words a refusal carries, without the "Image not shown:" opening that
/// only reads correctly inside the placeholder card on a page.
///
/// WHY reuse the placeholder rather than name a code: the two have to say the
/// same thing about the same file, and a reader of the serial log who can see
/// the card should not need a second vocabulary for what is behind it.
fn refusal_reason(refusal: images::ImageRefusal) -> &'static [u8] {
    let text = refusal.placeholder();
    match text.iter().position(|&b| b == b':') {
        Some(colon) if colon + 2 < text.len() => &text[colon + 2..],
        _ => text,
    }
}

/// The page's picture, averaged down to the sixteen-by-sixteen the address
/// field has room for.
///
/// WHY averaged and not sampled: a page offers its face a hundred pixels
/// across and the field shows sixteen, so taking one source pixel per drawn
/// one keeps a hundredth of the picture and the mark arrives as a speckle.
/// Each drawn pixel is the mean of the block it stands for, weighted by
/// opacity so a transparent corner does not grey out its neighbours.
///
/// WHY once here rather than per pixel while painting: the same hundred reads
/// would otherwise be made again on every repaint of the bar.
fn reduce_to_face(icon: std::image::Bitmap<'_>, out: &mut [[u8; 4]]) {
    for row in 0..FAVICON_BOX.h {
        for column in 0..FAVICON_BOX.w {
            let top = row * icon.h as usize / FAVICON_BOX.h;
            let bottom = ((row + 1) * icon.h as usize / FAVICON_BOX.h).max(top + 1);
            let left = column * icon.w as usize / FAVICON_BOX.w;
            let right = ((column + 1) * icon.w as usize / FAVICON_BOX.w).max(left + 1);
            let (mut red, mut green, mut blue, mut opacity) = (0u64, 0u64, 0u64, 0u64);
            for source_y in top..bottom {
                for source_x in left..right {
                    let pixel = (source_y * icon.w as usize + source_x) * 4;
                    let Some(rgba) = icon.rgba.get(pixel..pixel + 4) else {
                        continue;
                    };
                    let alpha = rgba[3] as u64;
                    opacity += alpha;
                    red += rgba[0] as u64 * alpha;
                    green += rgba[1] as u64 * alpha;
                    blue += rgba[2] as u64 * alpha;
                }
            }
            let cell = &mut out[row * FAVICON_BOX.w + column];
            if opacity == 0 {
                *cell = [0, 0, 0, 0];
                continue;
            }
            let seen = ((bottom - top) * (right - left)) as u64;
            cell[0] = (red / opacity) as u8;
            cell[1] = (green / opacity) as u8;
            cell[2] = (blue / opacity) as u8;
            cell[3] = (opacity / seen).min(255) as u8;
        }
    }
}

/// One pixel of the page's face, over the address field's own colour.
fn favicon_pixel(x: usize, y: usize, face: &[[u8; 4]; FAVICON_FACE_PIXELS], bg: u32) -> Option<u32> {
    if x < FAVICON_BOX.x || y < FAVICON_BOX.y {
        return None;
    }
    let px = x - FAVICON_BOX.x;
    let py = y - FAVICON_BOX.y;
    if px >= FAVICON_BOX.w || py >= FAVICON_BOX.h {
        return None;
    }
    let [red, green, blue, alpha] = face[py * FAVICON_BOX.w + px];
    if alpha == 0 {
        return None;
    }
    let argb =
        (alpha as u32) << 24 | (red as u32) << 16 | (green as u32) << 8 | blue as u32;
    if alpha == 255 {
        return Some(argb);
    }
    Some(blend_toward(argb, bg, 255 - alpha as u32) | 0xFF00_0000)
}

/// Close 'X' Glyph (centered at cx, cy)
fn close_glyph(x: usize, y: usize, cx: usize, cy: usize) -> bool {
    let dx = x as isize - cx as isize;
    let dy = y as isize - cy as isize;
    if dx.abs() > 8 || dy.abs() > 8 {
        return false;
    }
    (dx + dy).abs() <= 1 || (dx - dy).abs() <= 1
}

/// The close target inside the tab card at `card_y`.
///
/// WHY a named box instead of `x >= 630`: the glyph is seventeen pixels across
/// and the hit test took everything from 630 to the card's right edge and the
/// full height of the card, so a tap on empty padding beside the title closed
/// the tab rather than switching to it. This is the floor the platform sets for
/// a touch target, and the paint fills the same box the tap reads.
const TAB_CLOSE_CENTER_X: usize = 646;
const TAB_CLOSE_CENTER_Y: usize = 32;

fn tab_close_rect(card_y: usize) -> Rect {
    Rect::new(
        TAB_CLOSE_CENTER_X - MIN_TOUCH_TARGET / 2,
        card_y + TAB_CLOSE_CENTER_Y - MIN_TOUCH_TARGET / 2,
        MIN_TOUCH_TARGET,
        MIN_TOUCH_TARGET,
    )
}

/// Star Glyph for Bookmarks
fn star_glyph(x: usize, y: usize, cx: usize, cy: usize, filled: bool) -> bool {
    let dx = x as isize - cx as isize;
    let dy = y as isize - cy as isize;
    if dx.abs() > 9 || dy.abs() > 9 {
        return false;
    }
    let on_outline = {
        let top = dy <= 0 && dx.abs() as isize <= 7 + (-dy) / 3;
        let bottom = dy > 0 && dx.abs() as isize <= 8 - dy;
        top || bottom
    };
    if filled {
        on_outline || (dx.abs() <= 4 && dy.abs() <= 5)
    } else {
        on_outline
    }
}

/// Reload Ring Glyph with Arrow
fn reload_glyph(x: usize, y: usize, cx: usize, cy: usize) -> bool {
    let dx = x as isize - cx as isize;
    let dy = y as isize - cy as isize;
    let d2 = dx * dx + dy * dy;
    if d2 >= 49 && d2 <= 81 && !(dx > 3 && dy < -3) {
        return true;
    }
    let t = (cx + 5) as isize - x as isize;
    if t >= 0 && t <= 6 {
        let half = 3 - (t * 3) / 6;
        if (y as isize - (cy as isize - 8)).abs() <= half {
            return true;
        }
    }
    false
}

/// Kebab Overflow Menu Glyph (3 vertical dots)
fn kebab_glyph(x: usize, y: usize, cx: usize, cy: usize) -> bool {
    let dx = (x as isize - cx as isize).abs();
    let dy1 = (y as isize - (cy as isize - 8)).abs();
    let dy2 = (y as isize - cy as isize).abs();
    let dy3 = (y as isize - (cy as isize + 8)).abs();
    dx <= 2 && (dy1 <= 2 || dy2 <= 2 || dy3 <= 2)
}

/// Tab Count Badge Box `[N]`
fn tab_badge_pixel(x: usize, y: usize, cx: usize, cy: usize, count: usize) -> Option<u32> {
    let dx = (x as isize - cx as isize).abs();
    let dy = (y as isize - cy as isize).abs();
    if dx > 13 || dy > 13 {
        return None;
    }
    if (dx >= 11 && dx <= 13) || (dy >= 11 && dy <= 13) {
        return Some(THEME.on_surface_variant);
    }
    let digit = [b'0' + (count as u8).min(9).max(1)];
    if let Some(px) = text_px(x, y, cx - 4, cy - 10, 1, &digit, THEME.on_surface, THEME.surface_container, true) {
        return Some(px);
    }
    Some(THEME.surface_container)
}

/// Link Arrow Glyph (`→`)
fn link_arrow_glyph(x: usize, y: usize, cx: usize, cy: usize) -> bool {
    link_arrow_ink(x as isize - cx as isize, y as isize - cy as isize)
}

/// Offline Beacon Glyph
fn offline_glyph(x: usize, y: usize, cx: usize, cy: usize) -> bool {
    let dx = (x as isize - cx as isize).abs();
    let dy = (y as isize - cy as isize).abs();
    if dx > 24 || dy > 24 {
        return false;
    }
    let d2 = dx * dx + dy * dy;
    if ((d2 >= 320 && d2 <= 440) || (d2 >= 120 && d2 <= 200)) && (y as isize) <= (cy as isize + 2) {
        return true;
    }
    if d2 <= 16 {
        return true;
    }
    let slash = (x as isize - cx as isize) - (y as isize - cy as isize);
    if slash.abs() <= 2 && dx <= 20 && dy <= 20 {
        return true;
    }
    false
}

/// Download Glyph: an arrow into a tray.
fn download_glyph(x: usize, y: usize, cx: usize, cy: usize) -> bool {
    let dx = x as isize - cx as isize;
    let dy = y as isize - cy as isize;
    if dx.abs() > 22 || dy < -20 || dy > 20 {
        return false;
    }
    if dx.abs() <= 2 && dy >= -18 && dy <= 4 {
        return true;
    }
    if dy >= 2 && dy <= 15 && dx.abs() <= 15 - dy {
        return true;
    }
    if dy >= 16 && dy <= 19 && dx.abs() <= 20 {
        return true;
    }
    if dy >= 8 && dy < 16 && (dx.abs() == 20) {
        return true;
    }
    false
}

/// Down Chevron Glyph (for dismiss)
fn draw_chevron_down(x: usize, y: usize, cx: usize, cy: usize, color: u32) -> Option<u32> {
    let dx = x as isize - cx as isize;
    let dy = y as isize - cy as isize;
    if dx.abs() <= 6 && (dy - (3 - dx.abs())).abs() <= 1 {
        return Some(color);
    }
    None
}

// -----------------------------------------------------------------------------
// Soft Keyboard Layout & Geometry (Docked above the navigation bar)
// -----------------------------------------------------------------------------
const KB_Y0: usize = 980;
/// Bottom edge of the docked keyboard.
///
/// WHY it is the navigation bar's own top and not a remembered 1408: the bar is
/// tested before the keyboard when the surface is painted, so its buttons were
/// drawn -- but the press path tested them in the other order, and the keyboard
/// branch answered any row inside its own band with a redraw whether or not it
/// held a key. The bar's rows 1350..1408 were inside that band and held no key,
/// so with a field focused Back / Forward / Home / Bookmarks / Tabs were visible
/// and completely dead. The keyboard now stops where the bar starts.
const KB_Y1: usize = NAV_BAR_Y0;
const KB_ACC_Y0: usize = 918;
const KB_ACC_Y1: usize = 974;

/// The top inset that centres a line at rung `rung` inside a box `h` rows tall.
const fn centred_text_top(h: usize, rung: usize) -> usize {
    (h - font::line_height(rung)) / 2
}

/// The column that centres a run inside a key, by the run's own measured width.
///
/// WHY measured and not remembered: every key placed its label at a fixed column
/// of a box whose centre the glyph cannot know about, so a narrow cap sat four
/// rows left of its key's middle while a wide one sat three right of it, and the
/// "Space" label sat eleven rows left of the widest key on the board.
#[inline(always)]
fn centred_text_left(rect: Rect, text: &[u8], rung: usize) -> usize {
    rect.x + rect.w.saturating_sub(text_width(text, rung)) / 2
}

const _: () = assert!(
    1312 <= KB_Y1,
    "the keyboard's last key row must sit inside the panel it is drawn on",
);

/// The keyboard's column rhythm: rows 0, 1 and 2 are runs of caps `KB_COL_PITCH`
/// apart, starting at each row's own left edge and `KB_KEY_W` wide.
///
/// WHY one rhythm for the painter and the tap handler: the two symbol rows used
/// to count their columns from a different edge and a different stride than the
/// caps they answer. Row 1 painted nine caps from x 46 and hit-tested ten
/// columns from x 15, and row 2 painted seven caps on a stride of 70 and
/// hit-tested eight columns on a stride of 64. The left edge of a drawn cap and
/// the left edge of the column that answers it are forty-odd rows apart, so the
/// outer half of every symbol key inserted its neighbour's glyph -- tapping the
/// drawn "@" typed `"`, and tapping the drawn "+" typed "=" -- and the extra
/// column past the last cap was tappable where no key is drawn.
const KB_COL_PITCH: usize = 70;
const KB_KEY_W: usize = 60;

/// The space left inside a column before its cap begins.
const KB_KEY_INSET: usize = 3;

/// The left edge of the keyboard's three run-of-caps rows.
///
/// WHY the tap handler tests a column against it before dividing: `x` minus the
/// row's edge, saturated at zero, folds the blank panel to the left of every row
/// into that row's first column -- so a tap well outside the drawn caps, in the
/// margin between the keyboard card and its own edge, inserted `q` or `a`.
const KB_ROW_X0: [usize; 3] = [15, 46, 105];

/// How many caps each of those rows draws, and so how many columns answer it.
const KB_ROW_KEYS: [usize; 3] = [10, 9, 7];

/// The left edge of the backspace cap that ends row 2.
const KB_BACKSPACE_X0: usize = 620;

/// Row 0's letters and digits, and row 1's and row 2's symbols: one table per
/// row per layout, each exactly as long as the row draws caps, so no glyph is
/// reachable without a key drawn for it and no drawn key is inert.
/// Row 1's symbols put `=` where a `$` used to sit: no key on either layout
/// produced an `=`, so a query-string address -- the shape most of today's links
/// actually are -- could not be typed at all, and a form field could not submit a
/// key=value pair.
const KB_ROW0_LETTERS: &[u8] = b"QWERTYUIOP";
const KB_ROW0_SYMBOLS: &[u8] = b"1234567890";
const KB_ROW1_LETTERS: &[u8] = b"ASDFGHJKL";
const KB_ROW1_SYMBOLS: &[u8] = b"-/:;()=&@";
const KB_ROW2_LETTERS: &[u8] = b"ZXCVBNM";
const KB_ROW2_SYMBOLS: &[u8] = b"?!'#%*+";

const _: () = assert!(KB_ROW0_LETTERS.len() == KB_ROW_KEYS[0]);
const _: () = assert!(KB_ROW0_SYMBOLS.len() == KB_ROW_KEYS[0]);
const _: () = assert!(KB_ROW1_LETTERS.len() == KB_ROW_KEYS[1]);
const _: () = assert!(KB_ROW1_SYMBOLS.len() == KB_ROW_KEYS[1]);
const _: () = assert!(KB_ROW2_LETTERS.len() == KB_ROW_KEYS[2]);
const _: () = assert!(KB_ROW2_SYMBOLS.len() == KB_ROW_KEYS[2]);
const _: () = assert!(KB_ROW_X0[0] + KB_COL_PITCH * KB_ROW_KEYS[0] <= W);
const _: () = assert!(KB_ROW_X0[1] + KB_COL_PITCH * KB_ROW_KEYS[1] <= W);
const _: () = assert!(KB_ROW_X0[2] + KB_COL_PITCH * KB_ROW_KEYS[2] <= KB_BACKSPACE_X0);

fn get_soft_keyboard_char(x: usize, y: usize, shift: bool, symbols: bool) -> Option<u8> {
    if y < KB_Y0 || y >= KB_Y1 {
        return None;
    }

    if symbols {
        // Row 0: 1 2 3 4 5 6 7 8 9 0 (y: 994..1066)
        if y >= 994 && y <= 1066 {
            let col = (x.saturating_sub(KB_ROW_X0[0])) / KB_COL_PITCH;
            if x >= KB_ROW_X0[0] && col < KB_ROW_KEYS[0] {
                return Some(KB_ROW0_SYMBOLS[col]);
            }
        }
        // Row 1: - / : ; ( ) = & @ (y: 1076..1148)
        if y >= 1076 && y <= 1148 {
            let col = (x.saturating_sub(KB_ROW_X0[1])) / KB_COL_PITCH;
            if x >= KB_ROW_X0[1] && col < KB_ROW_KEYS[1] {
                return Some(KB_ROW1_SYMBOLS[col]);
            }
        }
        // Row 2: ABC, ? ! ' # % * +, Backspace (y: 1158..1230)
        if y >= 1158 && y <= 1230 {
            if x >= 15 && x <= 95 {
                return Some(0xFE); // toggle ABC
            }
            if x >= KB_BACKSPACE_X0 && x <= 705 {
                return Some(8); // Backspace
            }
            let col = (x.saturating_sub(KB_ROW_X0[2])) / KB_COL_PITCH;
            if x >= KB_ROW_X0[2] && col < KB_ROW_KEYS[2] {
                return Some(KB_ROW2_SYMBOLS[col]);
            }
        }
        // Row 3: 123, comma, space, period, Go (y: 1240..1312)
        if y >= 1240 && y <= 1312 {
            if x >= 15 && x <= 115 {
                return Some(0xFE); // Back to ABC
            }
            if x >= 125 && x <= 195 {
                return Some(b',');
            }
            if x >= 205 && x <= 515 {
                return Some(b' ');
            }
            if x >= 525 && x <= 595 {
                return Some(b'.');
            }
            if x >= 605 && x <= 705 {
                return Some(b'\n'); // Go / Navigate
            }
        }
        return None;
    }

    // Standard QWERTY
    // Row 0: Q W E R T Y U I O P (y: 994..1066)
    if y >= 994 && y <= 1066 {
        let col = (x.saturating_sub(KB_ROW_X0[0])) / KB_COL_PITCH;
        if x >= KB_ROW_X0[0] && col < KB_ROW_KEYS[0] {
            let ch = KB_ROW0_LETTERS[col];
            return Some(if shift { ch } else { ch.to_ascii_lowercase() });
        }
    }
    // Row 1: A S D F G H J K L (y: 1076..1148)
    if y >= 1076 && y <= 1148 {
        let col = (x.saturating_sub(KB_ROW_X0[1])) / KB_COL_PITCH;
        if x >= KB_ROW_X0[1] && col < KB_ROW_KEYS[1] {
            let ch = KB_ROW1_LETTERS[col];
            return Some(if shift { ch } else { ch.to_ascii_lowercase() });
        }
    }
    // Row 2: Shift, Z X C V B N M, Backspace (y: 1158..1230)
    if y >= 1158 && y <= 1230 {
        if x >= 15 && x <= 95 {
            return Some(0xFF); // Shift toggle
        }
        if x >= KB_BACKSPACE_X0 && x <= 705 {
            return Some(8); // Backspace
        }
        let col = (x.saturating_sub(KB_ROW_X0[2])) / KB_COL_PITCH;
        if x >= KB_ROW_X0[2] && col < KB_ROW_KEYS[2] {
            let ch = KB_ROW2_LETTERS[col];
            return Some(if shift { ch } else { ch.to_ascii_lowercase() });
        }
    }
    // Row 3: ?123, comma, space, period, Go (y: 1240..1312)
    if y >= 1240 && y <= 1312 {
        if x >= 15 && x <= 115 {
            return Some(0xFE); // toggle symbols
        }
        if x >= 125 && x <= 195 {
            return Some(b',');
        }
        if x >= 205 && x <= 515 {
            return Some(b' ');
        }
        if x >= 525 && x <= 595 {
            return Some(b'.');
        }
        if x >= 605 && x <= 705 {
            return Some(b'\n'); // Go / Navigate
        }
    }

    None
}

fn render_browser_soft_keyboard(x: usize, y: usize, app: &BrowserApp) -> u32 {
    let krect = Rect::new(6, KB_Y0, 708, KB_Y1 - KB_Y0);
    if let Some(_) = draw_rounded_rect(x, y, krect, R_CARD, THEME.surface_container_low, Some(THEME.border), 1) {
        // Row 0: Q W E R T Y U I O P (y: 994..1066)
        if y >= 994 && y <= 1066 {
            let col = (x.saturating_sub(KB_ROW_X0[0])) / KB_COL_PITCH;
            let kx = KB_ROW_X0[0] + col * KB_COL_PITCH;
            if col < KB_ROW_KEYS[0] && x >= kx + KB_KEY_INSET && x < kx + KB_KEY_INSET + KB_KEY_W {
                let rect = Rect::new(kx + KB_KEY_INSET, 994, KB_KEY_W, 72);
                if let Some(c) = draw_rounded_rect(x, y, rect, R_CHIP, THEME.surface_container_high, None, 0) {
                    let ch = if app.kb_symbols {
                        KB_ROW0_SYMBOLS[col]
                    } else {
                        let ch = KB_ROW0_LETTERS[col];
                        if app.kb_shift { ch } else { ch.to_ascii_lowercase() }
                    };
                    let ch_buf = [ch];
                    if let Some(px) = text_px(x, y, centred_text_left(rect, &ch_buf, 2), rect.y + centred_text_top(rect.h, 2), 2, &ch_buf, THEME.on_surface, c, true) {
                        return px;
                    }
                    return c;
                }
            }
        }

        // Row 1: A S D F G H J K L (y: 1076..1148)
        if y >= 1076 && y <= 1148 {
            let col = (x.saturating_sub(KB_ROW_X0[1])) / KB_COL_PITCH;
            let kx = KB_ROW_X0[1] + col * KB_COL_PITCH;
            if col < KB_ROW_KEYS[1] && x >= kx + KB_KEY_INSET && x < kx + KB_KEY_INSET + KB_KEY_W {
                let rect = Rect::new(kx + KB_KEY_INSET, 1076, KB_KEY_W, 72);
                if let Some(c) = draw_rounded_rect(x, y, rect, R_CHIP, THEME.surface_container_high, None, 0) {
                    let ch = if app.kb_symbols {
                        KB_ROW1_SYMBOLS[col]
                    } else {
                        let ch = KB_ROW1_LETTERS[col];
                        if app.kb_shift { ch } else { ch.to_ascii_lowercase() }
                    };
                    let ch_buf = [ch];
                    if let Some(px) = text_px(x, y, centred_text_left(rect, &ch_buf, 2), rect.y + centred_text_top(rect.h, 2), 2, &ch_buf, THEME.on_surface, c, true) {
                        return px;
                    }
                    return c;
                }
            }
        }

        // Row 2: Shift, Letters, Backspace (y: 1158..1230)
        if y >= 1158 && y <= 1230 {
            // Shift key (15..95)
            if x >= 15 && x <= 95 {
                let rect = Rect::new(15, 1158, 80, 72);
                let armed = app.kb_shift;
                let fill = if armed { THEME.primary } else { THEME.surface_container };
                if let Some(c) = draw_rounded_rect(x, y, rect, R_CHIP, fill, None, 0) {
                    // WHY the ink trades places with the fill: when shift is
                    // armed the key inverts to the filled primary so the state
                    // a person is in is readable without reading the field, and
                    // `Theme::on_surface` is the ink for a *page*, which on this
                    // theme is `#0A0A0A` -- the same black as the fill it now
                    // sits on (1.00:1). The "^" was therefore invisible exactly
                    // in the one state the key exists to announce. Every other
                    // filled control here already asks the theme for its on-primary
                    // ink; this one now does too.
                    let ink = if armed { THEME.on_primary } else { THEME.on_surface };
                    let txt = if app.kb_symbols { b"ABC" as &[u8] } else { b"^" as &[u8] };
                    if let Some(px) = text_px(x, y, centred_text_left(rect, txt, 1), rect.y + centred_text_top(rect.h, 1), 1, txt, ink, c, true) {
                        return px;
                    }
                    return c;
                }
            }

            // Center 7 keys: Z X C V B N M, or the seven symbols
            let col = (x.saturating_sub(KB_ROW_X0[2])) / KB_COL_PITCH;
            let kx = KB_ROW_X0[2] + col * KB_COL_PITCH;
            if col < KB_ROW_KEYS[2] && x >= kx + KB_KEY_INSET && x < kx + KB_KEY_INSET + KB_KEY_W {
                let rect = Rect::new(kx + KB_KEY_INSET, 1158, KB_KEY_W, 72);
                if let Some(c) = draw_rounded_rect(x, y, rect, R_CHIP, THEME.surface_container_high, None, 0) {
                    let ch = if app.kb_symbols {
                        KB_ROW2_SYMBOLS[col]
                    } else {
                        let ch = KB_ROW2_LETTERS[col];
                        if app.kb_shift { ch } else { ch.to_ascii_lowercase() }
                    };
                    let ch_buf = [ch];
                    if let Some(px) = text_px(x, y, centred_text_left(rect, &ch_buf, 2), rect.y + centred_text_top(rect.h, 2), 2, &ch_buf, THEME.on_surface, c, true) {
                        return px;
                    }
                    return c;
                }
            }

            // Backspace key (620..705)
            if x >= KB_BACKSPACE_X0 && x <= 705 {
                let rect = Rect::new(KB_BACKSPACE_X0, 1158, 85, 72);
                if let Some(c) = draw_rounded_rect(x, y, rect, R_CHIP, THEME.surface_container, None, 0) {
                    if let Some(px) = text_px(x, y, centred_text_left(rect, b"<X]", 1), rect.y + centred_text_top(rect.h, 1), 1, b"<X]", THEME.on_surface, c, true) {
                        return px;
                    }
                    return c;
                }
            }
        }

        // Row 3: ?123, Comma, Space, Period, Go (y: 1240..1312)
        if y >= 1240 && y <= 1312 {
            // ?123 / ABC toggle key (15..115)
            if x >= 15 && x <= 115 {
                let rect = Rect::new(15, 1240, 100, 72);
                if let Some(c) = draw_rounded_rect(x, y, rect, R_CHIP, THEME.surface_container, None, 0) {
                    let txt = if app.kb_symbols { b"ABC" as &[u8] } else { b"?123" as &[u8] };
                    if let Some(px) = text_px(x, y, centred_text_left(rect, txt, 1), rect.y + centred_text_top(rect.h, 1), 1, txt, THEME.on_surface, c, true) {
                        return px;
                    }
                    return c;
                }
            }

            // Comma key (125..195)
            if x >= 125 && x <= 195 {
                let rect = Rect::new(125, 1240, 70, 72);
                if let Some(c) = draw_rounded_rect(x, y, rect, R_CHIP, THEME.surface_container_high, None, 0) {
                    if let Some(px) = text_px(x, y, centred_text_left(rect, b",", 2), rect.y + centred_text_top(rect.h, 2), 2, b",", THEME.on_surface, c, true) {
                        return px;
                    }
                    return c;
                }
            }

            // Spacebar (205..515)
            if x >= 205 && x <= 515 {
                let rect = Rect::new(205, 1240, 310, 72);
                if let Some(c) = draw_rounded_rect(x, y, rect, R_CHIP, THEME.surface_container_high, None, 0) {
                    if let Some(px) = text_px(x, y, centred_text_left(rect, b"Space", 1), rect.y + centred_text_top(rect.h, 1), 1, b"Space", THEME.on_surface_variant, c, false) {
                        return px;
                    }
                    return c;
                }
            }

            // Period key (525..595)
            if x >= 525 && x <= 595 {
                let rect = Rect::new(525, 1240, 70, 72);
                if let Some(c) = draw_rounded_rect(x, y, rect, R_CHIP, THEME.surface_container_high, None, 0) {
                    if let Some(px) = text_px(x, y, centred_text_left(rect, b".", 2), rect.y + centred_text_top(rect.h, 2), 2, b".", THEME.on_surface, c, true) {
                        return px;
                    }
                    return c;
                }
            }

            // Go / Navigate key (605..705) - Vibrant Blue Accent
            if x >= 605 && x <= 705 {
                let rect = Rect::new(605, 1240, 100, 72);
                if let Some(c) = draw_rounded_rect(x, y, rect, R_CHIP, THEME.primary, None, 0) {
                    if let Some(px) = text_px(x, y, centred_text_left(rect, b"Go", 1), rect.y + centred_text_top(rect.h, 1), 1, b"Go", THEME.on_primary, c, true) {
                        return px;
                    }
                    return c;
                }
            }
        }
        return THEME.surface_container_low;
    }
    THEME.surface
}

fn render_accessory_bar(x: usize, y: usize, app: &BrowserApp) -> u32 {
    let bar_rect = Rect::new(16, KB_ACC_Y0, 688, 56);
    if let Some(_) = draw_rounded_rect(x, y, bar_rect, R_SOFT, THEME.surface_container, Some(THEME.border), 1) {
        if app.field_focus != NO_FIELD {
            // Clear button (x: 24..104)
            let c_clear = Rect::new(24, KB_ACC_Y0 + 6, 80, 44);
            if let Some(c) = chrome_control_px(x, y, c_clear, R_CHIP) {
                if let Some(px) = text_px(x, y, centred_text_left(c_clear, b"Clear", 1), KB_ACC_Y0 + 16, 1, b"Clear", THEME.on_surface_variant, c, false) {
                    return px;
                }
                return c;
            }

            // Done button (x: 600..694)
            let c_done = Rect::new(600, KB_ACC_Y0 + 6, 94, 44);
            if let Some(c) = draw_rounded_rect(x, y, c_done, R_CHIP, THEME.primary, None, 0) {
                if let Some(px) = text_px(x, y, centred_text_left(c_done, b"Done", 1), KB_ACC_Y0 + 16, 1, b"Done", THEME.on_primary, c, true) {
                    return px;
                }
                return c;
            }

            return THEME.surface_container;
        }

        // Chip 1: "https://" (x 24..134)
        let c1 = Rect::new(24, KB_ACC_Y0 + 6, 110, 44);
        if let Some(c) = chrome_control_px(x, y, c1, R_CHIP) {
            if let Some(px) = text_px(x, y, centred_text_left(c1, b"https://", 1), KB_ACC_Y0 + 16, 1, b"https://", THEME.on_surface, c, false) {
                return px;
            }
            return c;
        }

        // Chip 2: ".com" (x 144..224)
        let c2 = Rect::new(144, KB_ACC_Y0 + 6, 80, 44);
        if let Some(c) = chrome_control_px(x, y, c2, R_CHIP) {
            if let Some(px) = text_px(x, y, centred_text_left(c2, b".com", 1), KB_ACC_Y0 + 16, 1, b".com", THEME.on_surface, c, false) {
                return px;
            }
            return c;
        }

        // Chip 3: ".org" (x 234..314)
        let c3 = Rect::new(234, KB_ACC_Y0 + 6, 80, 44);
        if let Some(c) = chrome_control_px(x, y, c3, R_CHIP) {
            if let Some(px) = text_px(x, y, centred_text_left(c3, b".org", 1), KB_ACC_Y0 + 16, 1, b".org", THEME.on_surface, c, false) {
                return px;
            }
            return c;
        }

        // Chip 4: "Clear" (x 324..404)
        let c4 = Rect::new(324, KB_ACC_Y0 + 6, 80, 44);
        if let Some(c) = chrome_control_px(x, y, c4, R_CHIP) {
            if let Some(px) = text_px(x, y, centred_text_left(c4, b"Clear", 1), KB_ACC_Y0 + 16, 1, b"Clear", THEME.on_surface_variant, c, false) {
                return px;
            }
            return c;
        }

        // Dismiss Down Chevron (x 630..694)
        let c_down = Rect::new(630, KB_ACC_Y0 + 6, 64, 44);
        if let Some(c) = chrome_control_px(x, y, c_down, R_CHIP) {
            if let Some(col) = draw_chevron_down(x, y, 662, KB_ACC_Y0 + 28, THEME.primary) {
                return col;
            }
            return c;
        }

        return THEME.surface_container;
    }
    THEME.surface
}

// -----------------------------------------------------------------------------
// Top Omnibox & Toolbar Renderer
// -----------------------------------------------------------------------------
fn toolbar_pixel(x: usize, y: usize, app: &BrowserApp, f: &Frame) -> u32 {
    let parsed = parse_uri(app.current_url());
    // Secure only when both halves agree: the address says https, and the
    // document on screen came off a channel whose handshake completed.
    let is_https = parsed.scheme == UriScheme::Https && app.document_tls_verified;

    // Back control, painted from the SDK so the chip is the same one every
    // other app puts in the same rect.
    if let Some(c) = back_button::pixel_for_theme(x, y, &THEME) {
        return c;
    }

    // Omnibox Capsule
    if let Some(c) = draw_rounded_rect(
        x,
        y,
        PILL,
        PILL_R,
        if app.address_focused {
            THEME.surface
        } else {
            THEME.surface_container
        },
        Some(if app.address_focused {
            THEME.primary
        } else {
            THEME.border
        }),
        if app.address_focused { 2 } else { 1 },
    ) {
        if let Some(face) = &app.favicon {
            if let Some(px) = favicon_pixel(x, y, face, c) {
                return px;
            }
        }
        // WHY the lock yields to the page's own picture: both are marks about
        // the address in the field, and a face says more about what is loaded
        // here than a lock this app never verified does.
        if app.favicon.is_none() && padlock_glyph(x, y, MARK_CX, MARK_CY) {
            // WHY the mark reflects an observed TLS channel: it is read from the
            // transport that delivered this document, so it appears only for a
            // page whose TLS 1.3 handshake completed and whose peer certificate
            // passed the browser's verifier -- which checks the certificate's
            // structure, its validity dates, the name match against the address
            // dialled, and a signature chain to the roots the browser ships.
            // It says nothing beyond that, and in particular nothing about a
            // public trust root. A plain HTTP channel stays muted in
            // THEME.on_surface_variant.
            //
            // The mark is not drawn at all when nothing has loaded, so a
            // failed navigation shows no lock beside a URL that was never
            // contacted.
            if app.status != PageStatus::Loaded {
                return THEME.surface_container;
            }
            return if is_https { THEME.primary } else { THEME.on_surface_variant };
        }
        let in_sel = f.select_w > 0
            && x >= PILL_TEXT_X
            && x < PILL_TEXT_X + f.select_w
            && y >= PILL_TEXT_Y
            && y < PILL_TEXT_Y + font::line_height(1);
        let t = &f.pill[..f.pill_len];
        if !t.is_empty() {
            let bg = if in_sel { THEME.primary_container } else { c };
            if let Some(px) = text_px(x, y, PILL_TEXT_X, PILL_TEXT_Y, 1, t, f.pill_color, bg, false) {
                return px;
            }
        }
        if in_sel {
            return THEME.primary_container;
        }
        if f.show_caret
            && x >= f.caret_x
            && x < f.caret_x + 2
            && y >= PILL_TEXT_Y
            && y < PILL_TEXT_Y + font::line_height(1)
        {
            return THEME.primary;
        }
        return c;
    }

    // Reload / Stop Button
    if let Some(c) = chrome_control_px(x, y, RELOAD_BTN, BTN_R) {
        let cx = RELOAD_BTN.x + RELOAD_BTN.w / 2;
        let cy = RELOAD_BTN.y + RELOAD_BTN.h / 2;
        let active = if app.is_loading() {
            close_glyph(x, y, cx, cy)
        } else {
            reload_glyph(x, y, cx, cy)
        };
        return if active {
            THEME.on_surface_variant
        } else {
            c
        };
    }

    // Tab Counter Badge `[N]` Button
    //
    // WHY the box is returned and not discarded: the badge paints its own
    // 27-pixel square, and every pixel of the target outside it used to fall
    // through to the bar's colour, so the rest of the 46x44 control a tap
    // answers was invisible.
    if let Some(c) = chrome_control_px(x, y, TABS_TOP_BTN, BTN_R) {
        let cx = TABS_TOP_BTN.x + TABS_TOP_BTN.w / 2;
        let cy = TABS_TOP_BTN.y + TABS_TOP_BTN.h / 2;
        if let Some(badge) = tab_badge_pixel(x, y, cx, cy, app.tabs.tab_count()) {
            return badge;
        }
        return c;
    }

    // Overflow Kebab Menu Button (`⋮`)
    if let Some(c) = chrome_control_px(x, y, KEBAB_BTN, BTN_R) {
        let cx = KEBAB_BTN.x + KEBAB_BTN.w / 2;
        let cy = KEBAB_BTN.y + KEBAB_BTN.h / 2;
        return if kebab_glyph(x, y, cx, cy) {
            THEME.on_surface_variant
        } else {
            c
        };
    }

    // Close Button (dismisses app to launcher)
    if let Some(c) = chrome_control_px(x, y, CLOSE_BTN, BTN_R) {
        let cx = CLOSE_BTN.x + CLOSE_BTN.w / 2;
        let cy = CLOSE_BTN.y + CLOSE_BTN.h / 2;
        return if close_glyph(x, y, cx, cy) {
            THEME.on_surface_variant
        } else {
            c
        };
    }

    // Blue Loading Progress Bar (y 114..116)
    if y >= 114 && y < 116 {
        return if app.loading_progress > 0 {
            THEME.primary
        } else {
            THEME.outline_variant
        };
    }

    THEME.surface
}

// -----------------------------------------------------------------------------
// Start Page (New Tab)
// -----------------------------------------------------------------------------
fn home_page_pixel(x: usize, y: usize, app: &BrowserApp) -> u32 {
    // 1. Sovereign Brand Hero Section (y 130..244)
    if y >= 130 && y < 244 {
        // App icon squircle (52x52) centered at (334, 134)
        let icon_rect = Rect::new(334, 134, 52, 52);
        if let Some(c) = draw_rounded_rect(x, y, icon_rect, R_CARD, THEME.primary, Some(THEME.border), 1) {
            let cx = 360isize;
            let cy = 160isize;
            let dx = (x as isize) - cx;
            let dy = (y as isize) - cy;
            let r2 = dx * dx + dy * dy;
            if r2 >= 196 && r2 <= 289 {
                return THEME.on_primary;
            }
            if (dx.abs() <= 1 && dy.abs() <= 17) || (dy.abs() <= 1 && dx.abs() <= 17) {
                return THEME.on_primary;
            }
            return c;
        }

        // Brand Title
        let brand_text = b"InterOS Browser";
        let bw = text_width(brand_text, 1);
        let bx0 = (W.saturating_sub(bw)) / 2;
        if let Some(px) = text_px(x, y, bx0, 196, 1, brand_text, THEME.on_surface, THEME.surface, true) {
            return px;
        }

        // Subtitle
        let sub_text = b"Sovereign Web";
        let sw = text_width(sub_text, 1);
        let sx0 = (W.saturating_sub(sw)) / 2;
        if let Some(px) = text_px(x, y, sx0, 220, 1, sub_text, THEME.on_surface_variant, THEME.surface, false) {
            return px;
        }
        return THEME.surface;
    }

    // 2. Favorites / Quick Access Grid (y 252..376)
    if y >= 252 && y < 376 {
        // Section label
        if y >= 252 && y < 252 + font::line_height(1) {
            // WHY the heading is "Quick links" and not "Favorites": the tiles
        // are a list the app shipped with, not the user's favourites. The
        // user's own bookmarks are the list below this one, and it says so
        // when it is empty.
        if let Some(px) = text_px(x, y, 28, 254, 1, b"Quick links", THEME.on_surface_variant, THEME.surface, true) {
                return px;
            }
            return THEME.surface;
        }

        // Col 0: cx = 98 (InterOS Platform)
        let c0_rect = Rect::new(66, 278, 64, 64);
        if let Some(c) = draw_rounded_rect(x, y, c0_rect, R_SOFT, THEME.surface_container_high, Some(THEME.border), 1) {
            if let Some(px) = text_px(x, y, 86, 300, 1, b"IO", THEME.on_surface, c, true) {
                return px;
            }
            return c;
        }
        if let Some(px) = text_px(x, y, 70, 350, 1, b"InterOS", THEME.on_surface, THEME.surface, false) {
            return px;
        }

        // Col 1: cx = 266 (YouTube Mobile)
        let c1_rect = Rect::new(234, 278, 64, 64);
        if let Some(c) = draw_rounded_rect(x, y, c1_rect, R_SOFT, THEME.surface_container_high, Some(THEME.border), 1) {
            let px = (x as isize) - 260;
            let py = ((y as isize) - 310).abs();
            if px >= 0 && px <= 14 && py * 14 <= (14 - px) * 10 {
                return THEME.on_surface_variant;
            }
            return c;
        }
        if let Some(px) = text_px(x, y, 238, 350, 1, b"YouTube", THEME.on_surface, THEME.surface, false) {
            return px;
        }

        // Col 2: cx = 434 (GitHub Source)
        let c2_rect = Rect::new(402, 278, 64, 64);
        if let Some(c) = draw_rounded_rect(x, y, c2_rect, R_SOFT, THEME.surface_container_high, Some(THEME.border), 1) {
            if let Some(px) = text_px(x, y, 422, 300, 1, b"GH", THEME.on_surface, c, true) {
                return px;
            }
            return c;
        }
        if let Some(px) = text_px(x, y, 410, 350, 1, b"GitHub", THEME.on_surface, THEME.surface, false) {
            return px;
        }

        // Col 3: cx = 602 (Wikipedia)
        let c3_rect = Rect::new(570, 278, 64, 64);
        if let Some(c) = draw_rounded_rect(x, y, c3_rect, R_SOFT, THEME.surface_container_high, Some(THEME.border), 1) {
            if let Some(px) = text_px(x, y, 594, 300, 1, b"W", THEME.on_surface, c, true) {
                return px;
            }
            return c;
        }
        if let Some(px) = text_px(x, y, 566, 350, 1, b"Wikipedia", THEME.on_surface, THEME.surface, false) {
            return px;
        }

        return THEME.surface;
    }

    // 3. "Your pages": saved bookmarks & session visits (y 386..1340)
    if y >= 386 && y < 418 {
        if let Some(px) = text_px(x, y, 28, 392, 1, b"Your pages", THEME.on_surface, THEME.surface, true) {
            return px;
        }
        return THEME.surface;
    }

    // 4. Saved pages & recent visits cards
    if home_row(app, 0).is_none() {
        let empty_rect = Rect::new(24, 422, W - 48, 96);
        if let Some(c) = draw_rounded_rect(x, y, empty_rect, CARD.radius, CARD.fill, CARD.border, 1) {
            let star_box = Rect::new(44, 438, 46, 46);
            if let Some(sc) = draw_rounded_rect(x, y, star_box, R_CHIP, THEME.surface_container_low, None, 0) {
                if star_glyph(x, y, 67, 461, false) {
                    return THEME.on_surface_variant;
                }
                return sc;
            }
            if let Some(px) = text_px(x, y, 104, 442, 1, b"No Bookmarks Yet", THEME.on_surface, c, true) {
                return px;
            }
            if let Some(px) = text_px(x, y, 104, 468, 1, b"Bookmarked and visited pages appear here", THEME.on_surface_variant, c, false) {
                return px;
            }
            return c;
        }

        return THEME.surface;
    }

    let card_h = 100;
    let card_gap = 12;
    for i in 0..4 {
        let card_y = 422 + i * (card_h + card_gap);
        let card_rect = Rect::new(24, card_y, W - 48, card_h);
        let Some(row) = home_row(app, i) else {
            continue;
        };

        if let Some(c) = draw_rounded_rect(x, y, card_rect, CARD.radius, CARD.fill, CARD.border, 1) {
            let thumb = Rect::new(W - 48 - 84, card_y + card_h - 88, 68, 68);
            // WHY the card's three lines are trimmed to the picture's edge: a
            // page names itself as long as it likes, and the last words of a
            // name that outgrew the card would land under its icon.
            let line_w = thumb.x - 56;
            let mut line = [0u8; 128];
            let n = font::ellipsize(row.title, 1, line_w, &mut line);
            if let Some(px) = text_px(x, y, 44, card_y + 20, 1, &line[..n], THEME.on_surface, THEME.surface_container_high, true) {
                return px;
            }
            let n = font::ellipsize(row.url, 1, line_w, &mut line);
            if let Some(px) = text_px(x, y, 44, card_y + 48, 1, &line[..n], THEME.on_surface_variant, THEME.surface_container_high, false) {
                return px;
            }
            // WHY the category is muted and not accented: the accent is the
            // colour this browser uses for the thing a tap acts on, and the
            // whole card is already that one target. An accent word inside a
            // tappable card reads as a second, narrower link that is not one.
            let n = font::ellipsize(row.meta, 1, line_w, &mut line);
            if let Some(px) = text_px(x, y, 44, card_y + card_h - 28, 1, &line[..n], THEME.on_surface_variant, THEME.surface_container_high, false) {
                return px;
            }
            if let Some(tc) = draw_rounded_rect(x, y, thumb, R_CHIP, THEME.surface_container, None, 0) {
                let tw = text_width(row.scheme_mark, 1);
                if let Some(px) = text_px(
                    x,
                    y,
                    thumb.x + 34 - tw / 2,
                    thumb.y + 24,
                    1,
                    row.scheme_mark,
                    THEME.on_surface_variant,
                    tc,
                    true,
                ) {
                    return px;
                }
                return tc;
            }
            return c;
        }
    }

    THEME.surface
}

/// One card of the start page's "Your pages" list.
///
/// WHY every field borrows: the start page is painted one pixel at a time and
/// this is built for each of those pixels. A row that owned its name and
/// address would copy both once per pixel of every card.
struct HomeRow<'a> {
    title: &'a [u8],
    url: &'a [u8],
    /// The address a tap on this card opens, which is the recorded one rather
    /// than the shortened text the card shows.
    target: &'a [u8],
    meta: &'a [u8],
    scheme_mark: &'a [u8],
}

fn home_row(app: &BrowserApp, index: usize) -> Option<HomeRow<'_>> {
    let saved = app.bookmarks.len();
    if index < saved {
        let rec = app.bookmarks.record(index)?;
        let url = rec.url_bytes();
        let title = rec.title_bytes();
        return Some(HomeRow {
            title: if title.is_empty() { b"Saved" } else { title },
            url: url_field(url),
            target: url,
            meta: b"Bookmark",
            scheme_mark: scheme_mark(url),
        });
    }

    let visited = app.history.len();
    let from_history = index - saved;
    if from_history >= visited {
        return None;
    }
    let visit = app.history.visit(visited - 1 - from_history)?;
    if visit.url == b"about:blank" {
        return None;
    }
    // WHY the page's own name and line, with the labels behind them: a card
    // that says "Visited" over an address tells the person the browser kept a
    // record, which they already knew. The page named itself, and that name is
    // what they scanned the log for. A page that stated no name keeps the
    // label, because nothing else is true of it.
    Some(HomeRow {
        title: if visit.title.is_empty() { b"Visited" } else { visit.title },
        url: url_field(visit.url),
        target: visit.url,
        meta: if visit.description.is_empty() {
            b"History"
        } else {
            visit.description
        },
        scheme_mark: scheme_mark(visit.url),
    })
}

/// The visit the history sheet shows in its `row`-th slot.
///
/// WHY the painter and the tap handler share one walk: the row a person sees
/// is the row their tap opens, and two walks of the same log that ever
/// disagree — about the filter, or about how far back the sheet looks — send a
/// tap to a page other than the one under the finger.
///
/// WHY eight: the sheet is a screen of rows, and a walk that went further back
/// drew rows no pixel reached.
fn history_row(app: &BrowserApp, row: usize) -> Option<Visit<'_>> {
    let count = app.history.len();
    let filter = app.filter_bytes();
    let mut skipped = row;
    for i in 0..count.min(HISTORY_ROWS) {
        let Some(visit) = app.history.visit(count - 1 - i) else {
            continue;
        };
        // WHY the name is searched as well as the address: the sheet now shows
        // both, and a page found by the words in its own title is a page the
        // person was looking for.
        if !browser::contains_ignore_case(visit.url, filter)
            && !browser::contains_ignore_case(visit.title, filter)
        {
            continue;
        }
        if skipped == 0 {
            return Some(visit);
        }
        skipped -= 1;
    }
    None
}

// -----------------------------------------------------------------------------
// Web Viewport / Content Area Renderer
// -----------------------------------------------------------------------------
/// The box the downloaded file's Open action is drawn in.
///
/// WHY it is derived from the frame: the badge sits below the wrapped body
/// text, and the body's line count depends on the headline. The renderer and
/// the tap handler both ask this function, so the control a person sees is
/// the control they can press.
fn open_action_rect(f: &Frame) -> Rect {
    Rect::new(W / 2 - 90, f.err_y + 54, 180, 48)
}

/// The box the download confirmation action is drawn in.
fn download_confirm_rect(f: &Frame) -> Rect {
    Rect::new(W / 2 - 190, f.err_y + 54, 180, 48)
}

/// The box the download cancellation action is drawn in.
fn download_cancel_rect(f: &Frame) -> Rect {
    Rect::new(W / 2 + 10, f.err_y + 54, 180, 48)
}

/// The box the dismissed state's action is drawn in.
fn dismiss_action_rect(f: &Frame) -> Rect {
    Rect::new(W / 2 - 90, f.err_y + 54, 180, 48)
}

/// Paints one pixel of an HTML-formatted page using the computed display list.
fn html_page_pixel(x: usize, y: usize, app: &BrowserApp, _f: &Frame) -> u32 {
    // 1. Scroll Position Indicator (on right edge x 712..716)
    if x >= 712 && x < 716 && y >= READER_TOP_Y && y < NAV_BAR_Y0 {
        let track_h = READER_VIEWPORT_H;
        let min_thumb_h = 60;
        if let Some((thumb_y, thumb_h)) = app.scroll.thumb_rect(READER_TOP_Y, track_h, min_thumb_h) {
            if y >= thumb_y && y < thumb_y + thumb_h {
                return THEME.on_surface_variant;
            }
        }
        return THEME.outline_variant;
    }

    // 2. Clamp x to content column and y to reader viewport
    if x < browser::CONTENT_PADDING
        || x >= browser::CONTENT_PADDING + browser::CONTENT_WIDTH
        || y < READER_TOP_Y
        || y >= NAV_BAR_Y0
    {
        return THEME.surface;
    }

    let doc_x = (x as isize) - (browser::CONTENT_PADDING as isize);
    let doc_y = (y as isize) - (READER_TOP_Y as isize) + (app.scroll.offset as isize);

    let layout = unsafe { &*core::ptr::addr_of!(PAGE_LAYOUT) };
    let dom = unsafe { &*core::ptr::addr_of!(PAGE_DOM) };
    let field_text = unsafe { &*core::ptr::addr_of!(FIELD_TEXT) };
    let field_lens = unsafe { &*core::ptr::addr_of!(FIELD_TEXT_LEN) };

    // 3. Outlines: 1 px stroke (reverse order of PAGE_LAYOUT.rects where kind == RectKind::Outline)
    for i in (0..layout.rect_count).rev() {
        let r = layout.rects[i];
        if r.kind == layout::RectKind::Outline {
            let rx = r.x as isize;
            let ry = r.y as isize;
            let rw = r.w as isize;
            let rh = r.h as isize;
            if rw > 0 && rh > 0 && doc_x >= rx && doc_x < rx + rw && doc_y >= ry && doc_y < ry + rh {
                if doc_x == rx || doc_x == rx + rw - 1 || doc_y == ry || doc_y == ry + rh - 1 {
                    return r.color;
                }
            }
        }
    }

    // 4. Form controls: PAGE_LAYOUT.fields
    for index in 0..layout.field_count {
        let field = layout.fields[index];
        let fx = field.x as isize;
        let fy = field.y as isize;
        let fw = field.w as isize;
        let fh = field.h as isize;
        if fw > 0 && fh > 0 && doc_x >= fx && doc_x < fx + fw && doc_y >= fy && doc_y < fy + fh {
            let is_focused = app.field_focus == index as u8;
            let border_col = if is_focused {
                THEME.primary
            } else {
                THEME.border
            };
            let on_1px_border = doc_x == fx || doc_x == fx + fw - 1 || doc_y == fy || doc_y == fy + fh - 1;

            match field.kind {
                layout::FieldKind::Input if field.input == layout::InputType::Checkbox => {
                    if on_1px_border {
                        return border_col;
                    }
                    if field.checked {
                        if doc_x >= fx + 3 && doc_x < fx + fw - 3 && doc_y >= fy + 3 && doc_y < fy + fh - 3 {
                            return THEME.primary;
                        }
                    }
                    return THEME.surface_container_low;
                }
                layout::FieldKind::Input if field.input == layout::InputType::Radio => {
                    if on_1px_border {
                        return border_col;
                    }
                    if field.checked {
                        if doc_x >= fx + 4 && doc_x < fx + fw - 4 && doc_y >= fy + 4 && doc_y < fy + fh - 4 {
                            return THEME.primary;
                        }
                    }
                    return THEME.surface_container_low;
                }
                layout::FieldKind::Button
                | layout::FieldKind::Input
                    if field.input == layout::InputType::Submit
                        || field.input == layout::InputType::Reset =>
                {
                    let is_submit = field.input == layout::InputType::Submit;
                    let bg_col = if is_submit {
                        THEME.primary
                    } else {
                        THEME.surface_container_high
                    };
                    let fg_col = if is_submit {
                        THEME.on_primary
                    } else {
                        THEME.on_surface
                    };
                    let btn_border = if is_submit {
                        THEME.primary
                    } else {
                        THEME.border
                    };

                    let label_bytes: &[u8] = if field_lens[index] > 0 {
                        &field_text[index][..field_lens[index] as usize]
                    } else {
                        let v = field.value.get(dom);
                        if !v.is_empty() {
                            v
                        } else if is_submit {
                            b"Submit"
                        } else if field.input == layout::InputType::Reset {
                            b"Reset"
                        } else {
                            b"Button"
                        }
                    };

                    let tw = text_width(label_bytes, 1);
                    let text_doc_x = fx + (fw - tw as isize).max(0) / 2;
                    let text_doc_y = fy + (fh - font::line_height(1) as isize).max(0) / 2;
                    let screen_tx = browser::CONTENT_PADDING as isize + text_doc_x;
                    let screen_ty = READER_TOP_Y as isize + text_doc_y - app.scroll.offset as isize;

                    if screen_ty >= 0 && screen_tx >= 0 {
                        if let Some(px) = text_px(
                            x,
                            y,
                            screen_tx as usize,
                            screen_ty as usize,
                            1,
                            label_bytes,
                            fg_col,
                            bg_col,
                            true,
                        ) {
                            return px;
                        }
                    }
                    if on_1px_border {
                        return btn_border;
                    }
                    return bg_col;
                }
                layout::FieldKind::Select => {
                    let bg_col = THEME.surface_container_low;
                    let screen_cx = browser::CONTENT_PADDING as isize + fx + fw - 16;
                    let screen_cy = READER_TOP_Y as isize + fy + fh / 2 - app.scroll.offset as isize;
                    if screen_cx >= 0 && screen_cy >= 0 {
                        if let Some(c) = draw_chevron_down(
                            x,
                            y,
                            screen_cx as usize,
                            screen_cy as usize,
                            THEME.on_surface_variant,
                        ) {
                            return c;
                        }
                    }

                    let val = if field_lens[index] > 0 {
                        &field_text[index][..field_lens[index] as usize]
                    } else {
                        field.value.get(dom)
                    };
                    if !val.is_empty() {
                        let screen_tx = browser::CONTENT_PADDING as isize + fx + 8;
                        let screen_ty = READER_TOP_Y as isize + fy + (fh - font::line_height(1) as isize).max(0) / 2 - app.scroll.offset as isize;
                        if screen_tx >= 0 && screen_ty >= 0 {
                            if let Some(px) = text_px(
                                x,
                                y,
                                screen_tx as usize,
                                screen_ty as usize,
                                1,
                                val,
                                THEME.on_surface,
                                bg_col,
                                false,
                            ) {
                                return px;
                            }
                        }
                    }

                    if on_1px_border {
                        return border_col;
                    }
                    return bg_col;
                }
                _ => {
                    let bg_col = THEME.surface_container_low;
                    let is_textarea = field.kind == layout::FieldKind::Textarea;

                    let len = field_lens[index] as usize;
                    let (val, fg_col) = if len > 0 {
                        let bytes = &field_text[index][..len];
                        (bytes, THEME.on_surface)
                    } else {
                        let ph = field.placeholder.get(dom);
                        (ph, THEME.on_surface_variant)
                    };

                    let text_doc_y = if is_textarea {
                        fy + 8
                    } else {
                        fy + (fh - font::line_height(1) as isize).max(0) / 2
                    };
                    let screen_tx = browser::CONTENT_PADDING as isize + fx + 8;
                    let screen_ty = READER_TOP_Y as isize + text_doc_y - app.scroll.offset as isize;

                    if !val.is_empty() && screen_tx >= 0 && screen_ty >= 0 {
                        let masked = [b'*'; FIELD_TEXT_MAX];
                        let render_val = if field.input == layout::InputType::Password && len > 0 {
                            let mlen = len.min(FIELD_TEXT_MAX);
                            &masked[..mlen]
                        } else {
                            val
                        };
                        if let Some(px) = text_px(
                            x,
                            y,
                            screen_tx as usize,
                            screen_ty as usize,
                            1,
                            render_val,
                            fg_col,
                            bg_col,
                            false,
                        ) {
                            return px;
                        }
                    }

                    if is_focused {
                        let val_for_caret = if len > 0 {
                            &field_text[index][..len]
                        } else {
                            b""
                        };
                        let tw = text_width(val_for_caret, 1);
                        let caret_doc_x = fx + 8 + tw as isize;
                        if doc_x >= caret_doc_x && doc_x < caret_doc_x + 2 && doc_y >= text_doc_y && doc_y < text_doc_y + font::line_height(1) as isize {
                            return THEME.primary;
                        }
                    }

                    if on_1px_border {
                        return border_col;
                    }
                    return bg_col;
                }
            }
        }
    }

    // 5. Text runs: PAGE_LAYOUT.runs (only runs in viewport range)
    for i in 0..layout.run_count {
        let run = layout.runs[i];
        let rx = run.x as isize;
        let ry = run.y as isize;
        let rw = run.w as isize;
        let rh = (run.h as isize).max(font::line_height(run.scale as usize) as isize);
        let screen_sy = READER_TOP_Y as isize + ry - app.scroll.offset as isize;

        if (y as isize) >= screen_sy && (y as isize) < screen_sy + rh {
            let screen_sx = browser::CONTENT_PADDING as isize + rx;
            if screen_sy >= 0 && screen_sx >= 0 {
                let bytes = run.bytes(dom);
                if !bytes.is_empty() {
                    let color = if run.link != layout::NO_LINK {
                        THEME.primary
                    } else if run.has_color {
                        run.color
                    } else {
                        THEME.on_surface
                    };
                    if let Some(px) = text_px(
                        x,
                        y,
                        screen_sx as usize,
                        screen_sy as usize,
                        run.scale as usize,
                        bytes,
                        color,
                        THEME.surface,
                        run.bold,
                    ) {
                        return px;
                    }
                    if run.link != layout::NO_LINK {
                        let underline_y = screen_sy + font::line_height(run.scale as usize) as isize - 2;
                        if (y as isize) == underline_y && (x as isize) >= screen_sx && (x as isize) < screen_sx + rw {
                            return THEME.primary;
                        }
                    }
                }
            }
        }
    }

    // 6. Images: PAGE_LAYOUT.images
    for i in 0..layout.image_count {
        let img = layout.images[i];
        let ix = img.x as isize;
        let iy = img.y as isize;
        let iw = img.w as usize;
        let ih = img.h as usize;
        if iw > 0 && ih > 0 && doc_x >= ix && doc_x < ix + iw as isize && doc_y >= iy && doc_y < iy + ih as isize {
            let slot_idx = img.slot as usize;
            let reason = if img.slot == 255 || slot_idx >= app.image_count {
                Some(images::ImageRefusal::LimitReached)
            } else {
                match app.images[slot_idx].status {
                    images::ImageStatus::Decoded => None,
                    images::ImageStatus::Refused(refusal) => Some(refusal),
                }
            };
            let placed = if img.slot != 255 && slot_idx < app.image_count {
                Some(&app.images[slot_idx])
            } else {
                None
            };

            if let (Some(slot), None) = (placed, reason) {
                if let Some(bitmap) = slot.bitmap {
                    let px_x = (doc_x - ix) as usize;
                    let px_y = (doc_y - iy) as usize;
                    if px_x < slot.display_w && px_y < slot.display_h {
                        let argb = bitmap.pixel(px_x, px_y, slot.display_w, slot.display_h);
                        let alpha = (argb >> 24) & 0xFF;
                        if alpha == 0xFF {
                            return argb;
                        }
                        if alpha > 0 {
                            return blend_toward(argb, THEME.surface, 255 - alpha) | 0xFF00_0000;
                        }
                    }
                }
            } else {
                let card_sx = browser::CONTENT_PADDING as isize + ix;
                let card_sy = READER_TOP_Y as isize + iy - app.scroll.offset as isize;
                if card_sy >= 0 && card_sx >= 0 {
                    let card = Rect::new(card_sx as usize, card_sy as usize, iw, ih);
                    if let Some(card_bg) = draw_rounded_rect(
                        x,
                        y,
                        card,
                        R_SMALL,
                        THEME.surface_container_low,
                        Some(THEME.border),
                        1,
                    ) {
                        if let Some(slot) = placed {
                            if slot.alt_len > 0 {
                                let mut alt_buf = [0u8; 64];
                                let alt_len = font::ellipsize(
                                    &slot.alt[..slot.alt_len.min(slot.alt.len())],
                                    1,
                                    iw.saturating_sub(24),
                                    &mut alt_buf,
                                );
                                if let Some(px) = text_px(
                                    x,
                                    y,
                                    card_sx as usize + 12,
                                    card_sy as usize + 8,
                                    1,
                                    &alt_buf[..alt_len],
                                    THEME.on_surface,
                                    card_bg,
                                    true,
                                ) {
                                    return px;
                                }
                            }
                        }
                        let reason_y = card_sy as usize + if placed.map_or(0, |s| s.alt_len) > 0 { 32 } else { 18 };
                        let refusal = reason.unwrap_or(images::ImageRefusal::LimitReached);
                        if let Some(px) = text_px(
                            x,
                            y,
                            card_sx as usize + 12,
                            reason_y,
                            1,
                            refusal.placeholder(),
                            THEME.on_surface_variant,
                            card_bg,
                            false,
                        ) {
                            return px;
                        }
                        return card_bg;
                    }
                }
                return THEME.surface_container_low;
            }
        }
    }

    // 7. Rule rects: PAGE_LAYOUT.rects where kind == RectKind::Rule
    for i in 0..layout.rect_count {
        let r = layout.rects[i];
        if r.kind == layout::RectKind::Rule {
            let rx = r.x as isize;
            let ry = r.y as isize;
            let rw = r.w as isize;
            let rh = r.h as isize;
            if rw > 0 && rh > 0 && doc_x >= rx && doc_x < rx + rw && doc_y >= ry && doc_y < ry + rh {
                return r.color;
            }
        }
    }

    // 8. Fills: PAGE_LAYOUT.rects where kind == RectKind::Fill (reverse order so ancestors paint first)
    for i in (0..layout.rect_count).rev() {
        let r = layout.rects[i];
        if r.kind == layout::RectKind::Fill {
            let rx = r.x as isize;
            let ry = r.y as isize;
            let rw = r.w as isize;
            let rh = r.h as isize;
            if rw > 0 && rh > 0 && doc_x >= rx && doc_x < rx + rw && doc_y >= ry && doc_y < ry + rh {
                return r.color;
            }
        }
    }

    // 9. Truncation note: if PAGE_LAYOUT.truncated
    if layout.truncated {
        // WHY the band is tested before the sentence is read: the break is
        // already on the display list, so the only work here is picking the
        // line this pixel falls in. A whole sentence measured per pixel is
        // the cost the reader cannot pay.
        let line_h = font::line_height(1) as isize;
        let first_sy = READER_TOP_Y as isize + layout.note_line_y(0) as isize
            - app.scroll.offset as isize;
        let line = if (y as isize) >= first_sy && (y as isize) < first_sy + line_h {
            Some((first_sy, layout.note_lines().0))
        } else {
            let second_sy = first_sy + line_h;
            if (y as isize) >= second_sy && (y as isize) < second_sy + line_h {
                Some((second_sy, layout.note_lines().1))
            } else {
                None
            }
        };
        if let Some((line_sy, line)) = line {
            if line_sy >= 0 && !line.is_empty() {
                if let Some(px) = text_px(
                    x,
                    y,
                    browser::CONTENT_PADDING,
                    line_sy as usize,
                    1,
                    line.as_bytes(),
                    THEME.on_surface_variant,
                    THEME.surface,
                    false,
                ) {
                    return px;
                }
            }
        }
    }

    // 10. Page background
    THEME.surface
}

fn web_page_pixel(x: usize, y: usize, app: &BrowserApp, f: &Frame) -> u32 {
    // Error state / No transport screen
    let download_status = app.status == PageStatus::Downloaded
        || app.status == PageStatus::DownloadFailed
        || app.status == PageStatus::DownloadPrompt
        || app.status == PageStatus::DownloadRefused;
    if app.status.is_error()
        || download_status
    {
        let cx = W / 2;
        let cy = f.icon_cy;
        let glyph = if download_status {
            download_glyph(x, y, cx, cy)
        } else {
            offline_glyph(x, y, cx, cy)
        };
        if glyph {
            return match app.status {
                PageStatus::Downloaded if app.last_download_truncated => THEME.warning,
                PageStatus::DownloadPrompt if app.pending_download.truncated => THEME.warning,
                PageStatus::Downloaded | PageStatus::DownloadPrompt => THEME.primary,
                _ => THEME.on_surface_variant,
            };
        }
        if f.head_len > 0 {
            if let Some(px) = text_px(
                x,
                y,
                f.head_x,
                f.head_y,
                f.head_scale,
                &f.head[..f.head_len.min(f.head.len())],
                THEME.on_surface,
                THEME.surface,
                true,
            ) {
                return px;
            }
        }
        for i in 0..f.body_n.min(f.body.len()).min(f.body_x.len()).min(f.body_y.len()) {
            if let Some(px) = text_px(x, y, f.body_x[i], f.body_y[i], 1, f.body[i], THEME.on_surface_variant, THEME.surface, false) {
                return px;
            }
        }

        // The gateway's own reason for the failure, when it gave one.
        //
        // WHY it is shown: the body says what the browser knows and the
        // badge says that the fetch failed, but neither says why. The
        // reason is the host's own account of the failure, and the screen
        // used to throw it away, which made the browser look broken rather
        // than telling the person what happened.
        //
        // WHY it is a line of its own: a reader has to be able to tell what
        // the server said from what the browser knows.
        if f.detail_len > 0 {
            let detail = &f.detail[..f.detail_len];
            let dw = text_width(detail, 1);
            if let Some(px) = text_px(
                x,
                y,
                (W.saturating_sub(dw)) / 2,
                f.detail_y,
                1,
                detail,
                THEME.on_surface,
                THEME.surface,
                false,
            ) {
                return px;
            }
        }

        // The failure is named, not stamped with one code.
        //
        // WHY: the screen used to print "ERR_NETWORK_TRANSPORT_REFUSED"
        // for every failure -- an invalid address, a refused local
        // connection and a gateway that answered nothing all produced the
        // same badge, so the badge was decoration rather than a report.
        let err_y = f.err_y;
        let err_rect = Rect::new(cx - 180, err_y, 360, 36);
        if let Some(c) = draw_rounded_rect(x, y, err_rect, R_SMALL, THEME.surface_container_high, Some(THEME.border), 1) {
            let label: &[u8] = match app.status {
                PageStatus::NoTransport => match app.transport_failure {
                    TransportFailure::NoCapability => b"NO TRANSPORT CAPABILITY",
                    TransportFailure::NetworkDown => b"NETWORK DOWN",
                    TransportFailure::DnsFailed => b"DNS RESOLUTION FAILED",
                    TransportFailure::NoEntropy => b"ENTROPY REFUSED",
                    TransportFailure::None => b"NO TRANSPORT",
                },
                PageStatus::ConnectionFailed => match app.connection_failure {
                    ConnectionFailure::Refused => b"CONNECTION REFUSED",
                    ConnectionFailure::Timeout => b"CONNECTION TIMED OUT",
                    ConnectionFailure::Stall => b"CONNECTION STALLED",
                    ConnectionFailure::TlsFailed => b"TLS HANDSHAKE FAILED",
                    ConnectionFailure::TlsNoTrustAnchor => b"NO TRUST ANCHOR",
                    ConnectionFailure::TlsNameRefused => b"ADDRESS REFUSED",
                    ConnectionFailure::TlsEntropyRefused => b"NO KEY ENTROPY",
                    ConnectionFailure::TlsClockUnavailable => b"NO CLOCK READING",
                    ConnectionFailure::TlsTrustStoreRefused => b"TRUST STORE REFUSED",
                    ConnectionFailure::TlsRecordRefused => b"ANSWER REFUSED HERE",
                    ConnectionFailure::None => b"CONNECTION FAILED",
                },
                PageStatus::ZeroBytes => b"NO RESPONSE FROM THE GATEWAY",
                PageStatus::ProtocolFault => match app.protocol_fault {
                    ProtocolFaultKind::MalformedResponse => b"PROTOCOL FAULT",
                    ProtocolFaultKind::InvalidEncoding => b"INVALID ENCODING",
                    ProtocolFaultKind::UnparseableDocument => b"UNPARSEABLE DOCUMENT",
                    ProtocolFaultKind::EmptyDocument => b"EMPTY DOCUMENT",
                    ProtocolFaultKind::None => b"PROTOCOL FAULT",
                },
                PageStatus::InvalidAddress => b"INVALID ADDRESS",
                PageStatus::UnrecognisedAddress => b"ADDRESS NOT RECOGNISED",
                PageStatus::UnsupportedScheme => b"SCHEME NOT SUPPORTED",
                PageStatus::GatewayError => b"GATEWAY COULD NOT FETCH",
                PageStatus::Downloaded => {
                    if app.last_download_truncated {
                        b"DOWNLOAD PARTIAL"
                    } else {
                        b"DOWNLOAD COMPLETE"
                    }
                }
                PageStatus::DownloadFailed => match app.download_failure {
                    DownloadFailure::NoBytes => b"NO BYTES RECEIVED",
                    DownloadFailure::NoPath => b"NO FREE NAME",
                    DownloadFailure::Lost => b"BYTES LOST",
                    DownloadFailure::OriginRefused => b"ORIGIN REFUSED",
                    DownloadFailure::StorageRefused | DownloadFailure::None => b"DOWNLOAD FAILED",
                },
                PageStatus::DownloadPrompt => b"PERMISSION REQUIRED",
                PageStatus::DownloadRefused => b"DOWNLOAD DECLINED",
                PageStatus::LocalRefused => match app.local_failure {
                    LocalFailure::NoPath => b"NO PATH GIVEN",
                    LocalFailure::NoCapability => b"NO FILESYSTEM CAPABILITY",
                    LocalFailure::Refused => b"REFUSED BY THE DEVICE",
                    LocalFailure::Empty => b"THE FILE IS EMPTY",
                    LocalFailure::ReadRefused => b"THE READ WAS REFUSED",
                },
                _ => b"NO RESPONSE FROM THE GATEWAY",
            };
            let label_color = match app.status {
                PageStatus::Downloaded if app.last_download_truncated => THEME.warning,
                PageStatus::DownloadPrompt if app.pending_download.truncated => THEME.warning,
                PageStatus::Downloaded | PageStatus::DownloadPrompt => THEME.primary,
                _ => THEME.error,
            };
            let lw = text_width(label, 1);
            if let Some(px) = text_px(x, y, cx - lw / 2, err_y + 8, 1, label, label_color, c, false) {
                return px;
            }
            return c;
        }

        // A saved file or download prompt is not a network failure, so there is nothing to reload.
        if download_status {
            if app.status == PageStatus::Downloaded {
                let btn = open_action_rect(f);
                if let Some(c) = draw_rounded_rect(x, y, btn, R_CONTROL, THEME.primary, None, 0) {
                    let label = b"Open";
                    let lw = text_width(label, 1);
                    if let Some(px) = text_px(x, y, W / 2 - lw / 2, btn.y + 14, 1, label, THEME.on_primary, c, true) {
                        return px;
                    }
                    return c;
                }
                // The platform's answer is the only claim the screen makes:
                // a file with no handler says so instead of offering a
                // launch that would go nowhere.
                let caption: &[u8] = match app.open_state {
                    OpenState::NotAttempted => b"Opens with the app registered for this file's type.",
                    OpenState::Dispatched => b"The platform launched the registered handler.",
                    OpenState::NoHandler => b"No installed app handles this file's type.",
                    OpenState::Refused => b"The platform refused to launch the handler.",
                };
                let cw = text_width(caption, 1);
                if let Some(px) = text_px(
                    x,
                    y,
                    (W.saturating_sub(cw)) / 2,
                    btn.y + 62,
                    1,
                    caption,
                    THEME.on_surface_variant,
                    THEME.surface,
                    false,
                ) {
                    return px;
                }
            } else if app.status == PageStatus::DownloadPrompt {
                let confirm_btn = download_confirm_rect(f);
                if let Some(c) = draw_rounded_rect(x, y, confirm_btn, R_CONTROL, THEME.primary, None, 0) {
                    let label = b"Download";
                    let lw = text_width(label, 1);
                    if let Some(px) = text_px(x, y, confirm_btn.x + (confirm_btn.w.saturating_sub(lw)) / 2, confirm_btn.y + 14, 1, label, THEME.on_primary, c, true) {
                        return px;
                    }
                    return c;
                }
                let cancel_btn = download_cancel_rect(f);
                if let Some(c) = draw_rounded_rect(x, y, cancel_btn, R_CONTROL, THEME.surface_container_high, Some(THEME.border), 1) {
                    let label = b"Cancel";
                    let lw = text_width(label, 1);
                    if let Some(px) = text_px(x, y, cancel_btn.x + (cancel_btn.w.saturating_sub(lw)) / 2, cancel_btn.y + 14, 1, label, THEME.on_surface, c, true) {
                        return px;
                    }
                    return c;
                }
                let info_y = confirm_btn.y + 62;
                let filename = &app.pending_download.filename[..app.pending_download.filename_len];
                if !filename.is_empty() {
                    let fw = text_width(filename, 1);
                    if let Some(px) = text_px(
                        x,
                        y,
                        (W.saturating_sub(fw)) / 2,
                        info_y,
                        1,
                        filename,
                        THEME.on_surface_variant,
                        THEME.surface,
                        false,
                    ) {
                        return px;
                    }
                }
                if app.pending_download.truncated {
                    let warning: [&[u8]; 2] = [
                        b"Only part of the file arrived; the transfer ended early.",
                        b"If you allow it, the incomplete part is saved.",
                    ];
                    for (i, line) in warning.iter().enumerate() {
                        let lw = text_width(line, 1);
                        if let Some(px) = text_px(
                            x,
                            y,
                            (W.saturating_sub(lw)) / 2,
                            info_y + 30 + i * 24,
                            1,
                            line,
                            THEME.warning,
                            THEME.surface,
                            false,
                        ) {
                            return px;
                        }
                    }
                }
            } else if app.status == PageStatus::DownloadRefused {
                let btn = dismiss_action_rect(f);
                if let Some(c) = draw_rounded_rect(x, y, btn, R_CONTROL, THEME.surface_container_high, Some(THEME.border), 1) {
                    let label = b"Dismiss";
                    let lw = text_width(label, 1);
                    if let Some(px) = text_px(x, y, W / 2 - lw / 2, btn.y + 14, 1, label, THEME.on_surface, c, true) {
                        return px;
                    }
                    return c;
                }
            }
            return THEME.surface;
        }

        // Reload Button Card
        let btn_y = err_y + 54;
        let btn_rect = Rect::new(cx - 90, btn_y, 180, 48);
        if let Some(c) = draw_rounded_rect(x, y, btn_rect, R_CONTROL, THEME.primary, None, 0) {
            let rel = b"Reload Page";
            let rw = text_width(rel, 1);
            if let Some(px) = text_px(x, y, cx - rw / 2, btn_y + 14, 1, rel, THEME.on_primary, c, true) {
                return px;
            }
            return c;
        }

        return THEME.surface;
    }

    if app.html_view {
        return html_page_pixel(x, y, app, f);
    }

    // Render classified document lines with smooth vertical scrolling.
    //
    // WHY the layout is rebuilt for every pixel: the reader draws from a
    // list of positioned lines, and the position of a line depends on the
    // lines above it. The platform's rendering work owns a display list
    // that would hoist this; until then the walk is bounded by the
    // document.
    let content_start_y = READER_TOP_Y;
    let base_y = (content_start_y as isize) - (app.scroll.offset as isize);
    let pad = browser::CONTENT_PADDING;
    let column = browser::CONTENT_WIDTH;

    let mut current_y = base_y;
    for i in 0..app.doc_line_count {
        let line = doc_line(i);
        let bytes = &line.bytes[..line.len.min(line.bytes.len())];
        let line_h = browser::line_box_height(line);
        let band_y = current_y
            + if line.block_start {
                line.style.margin_top as isize
            } else {
                0
            };
        let text_x = pad + line.style.padding_left as usize;
        let text_color = if line.style.has_color {
            line.style.color
        } else {
            THEME.on_surface
        };
        let muted_color = if line.style.has_color {
            line.style.color
        } else {
            THEME.on_surface_variant
        };
        let band_bg = if line.style.has_background {
            line.style.background
        } else {
            THEME.surface
        };

        match line.kind {
            LineKind::Heading1 => {
                // WHY the title is set at scale 2: the three heading levels
                // were all set at scale 1, so a page title and a section
                // heading were the same size and differed only in colour.
                // The type scale is the hierarchy.
                let title = if bytes.starts_with(b"# ") {
                    &bytes[2..]
                } else if bytes.starts_with(b"#") {
                    &bytes[1..]
                } else {
                    bytes
                };
                let h_y = band_y + 8;
                let band = font::line_height(TYPE_H1_SCALE);
                if y as isize >= h_y && (y as isize) < h_y + band as isize {
                    if let Some(px) = text_px(
                        x, y, text_x, h_y as usize, TYPE_H1_SCALE, title, text_color, band_bg, true,
                    ) {
                        return px;
                    }
                }
                // A rule under the title, drawn once for the heading rather
                // than once for each line it was wrapped onto.
                if !line.continued {
                    let d_y = h_y + band as isize + 6;
                    if y as isize == d_y && x >= pad && x <= W - pad {
                        return THEME.outline_variant;
                    }
                }
            }
            LineKind::Heading2 => {
                // WHY the second level carries a rule above it: the type
                // scale has two steps, so a second level that differed only
                // in weight was still the same size as the third.
                let title = if bytes.starts_with(b"## ") {
                    &bytes[3..]
                } else if bytes.starts_with(b"##") {
                    &bytes[2..]
                } else {
                    bytes
                };
                let h_y = band_y + 14;
                if !line.continued {
                    let r_y = band_y + 4;
                    if y as isize == r_y && x >= pad && x < pad + 40 {
                        return THEME.primary;
                    }
                }
                if y as isize >= h_y && (y as isize) < h_y + font::line_height(1) as isize {
                    if let Some(px) = text_px(x, y, text_x, h_y as usize, 1, title, text_color, band_bg, true) {
                        return px;
                    }
                }
            }
            LineKind::Heading3 => {
                let title = if bytes.starts_with(b"### ") {
                    &bytes[4..]
                } else if bytes.starts_with(b"###") {
                    &bytes[3..]
                } else {
                    bytes
                };
                let h_y = band_y + 8;
                if y as isize >= h_y && (y as isize) < h_y + font::line_height(1) as isize {
                    let color = if line.style.has_color { text_color } else { THEME.primary };
                    if let Some(px) = text_px(x, y, text_x, h_y as usize, 1, title, color, band_bg, true) {
                        return px;
                    }
                }
            }
            LineKind::ListItem => {
                let l_y = band_y + 4;
                if y as isize >= l_y && (y as isize) < l_y + 24 {
                    // The bullet sits in the margin, and the text starts
                    // after it.
                    let dx = x as isize - (text_x as isize + 10);
                    let dy = (y as isize) - (l_y + 8);
                    if dx * dx + dy * dy <= 8 {
                        return THEME.primary;
                    }
                    let item_text = if bytes.starts_with(b"* ") || bytes.starts_with(b"- ") || bytes.starts_with(b"+ ") {
                        &bytes[2..]
                    } else {
                        bytes
                    };
                    if let Some(px) = text_px(x, y, text_x + 22, l_y as usize, 1, item_text, text_color, band_bg, false) {
                        return px;
                    }
                }
            }
            LineKind::CodeBlock => {
                if bytes.starts_with(b"```") {
                    // The fence itself is not drawn, but it still occupies
                    // the space the block needs.
                } else {
                    // WHY the box spans the whole run: each line of a code
                    // block used to get its own bordered box, so a ten-line
                    // snippet rendered as ten separate cards. The box now
                    // starts at the first line of the run and ends at the
                    // last.
                    let c_y = band_y + 2;
                    let c_h = 30;
                    if y as isize >= c_y && (y as isize) < c_y + c_h as isize {
                        let code_bg = if line.style.has_background {
                            band_bg
                        } else {
                            THEME.surface_container_low
                        };
                        if let Some(c) = draw_rounded_rect(x, y, Rect::new(pad, c_y as usize, column, c_h), 8, code_bg, Some(THEME.border), 1) {
                            if x >= pad && x <= pad + 3 {
                                return THEME.primary;
                            }
                            let color = if line.style.has_color { text_color } else { THEME.success };
                            if let Some(px) = text_px(x, y, text_x + 14, (c_y + 5) as usize, 1, bytes, color, c, false) {
                                return px;
                            }
                            return c;
                        }
                    }
                }
            }
            LineKind::GemtextLink => {
                let g_y = band_y + 4;
                let g_h = 56;
                let link_rect = Rect::new(pad, g_y as usize, column, g_h);
                if y as isize >= g_y && (y as isize) < g_y + g_h as isize {
                    if let Some(c) = draw_rounded_rect(x, y, link_rect, R_CONTROL, THEME.surface_container_high, Some(THEME.border), 1) {
                        // The link's mark sits in the margin.
                        let icon_box = Rect::new(pad + 12, (g_y + 10) as usize, 36, 36);
                        if let Some(ic) = draw_rounded_rect(x, y, icon_box, R_SMALL, THEME.surface_container, None, 0) {
                            if link_arrow_glyph(x, y, pad + 30, (g_y + 28) as usize) {
                                return THEME.primary;
                            }
                            return ic;
                        }

                        let label_bytes = if line.link_label_len > 0 {
                            &line.link_label[..line.link_label_len.min(line.link_label.len())]
                        } else if line.link_target_len > 0 {
                            &line.link_target[..line.link_target_len.min(line.link_target.len())]
                        } else {
                            bytes
                        };
                        // WHY the label is ellipsized: a link's text is
                        // unbounded, and an unellipsized label runs off the
                        // card and off the screen.
                        let mut label_buf = [0u8; 64];
                        let label_len = font::ellipsize(
                            label_bytes,
                            1,
                            column - 72,
                            &mut label_buf,
                        );
                        if let Some(px) = text_px(x, y, pad + 60, (g_y + 10) as usize, 1, &label_buf[..label_len], text_color, c, true) {
                            return px;
                        }

                        if line.link_target_len > 0 {
                            let target_bytes = &line.link_target[..line.link_target_len.min(line.link_target.len())];
                            let mut target_buf = [0u8; 64];
                            let target_len = font::ellipsize(
                                target_bytes,
                                1,
                                column - 72,
                                &mut target_buf,
                            );
                            if let Some(px) = text_px(x, y, pad + 60, (g_y + 30) as usize, 1, &target_buf[..target_len], THEME.on_surface_variant, c, false) {
                                return px;
                            }
                        }

                        // The chevron marks the card as tappable.
                        let ch_x = (W - pad) as isize;
                        let ch_y = (g_y + 28) as isize;
                        let dist = ch_x - (x as isize);
                        if dist >= 0 && dist <= 7 {
                            let dy = ((y as isize) - ch_y).abs();
                            if (dy - dist).abs() <= 1 {
                                return THEME.on_surface_variant;
                            }
                        }

                        return c;
                    }
                }
            }
            LineKind::Blockquote => {
                let quote_text = if bytes.starts_with(b"> ") {
                    &bytes[2..]
                } else if bytes.starts_with(b">") {
                    &bytes[1..]
                } else {
                    bytes
                };
                let q_y = band_y + 4;
                let q_h = 28;
                if y as isize >= q_y && (y as isize) < q_y + q_h as isize {
                    // The quote's bar sits in the margin.
                    if x >= pad && x <= pad + 3 {
                        return THEME.primary;
                    }
                    if let Some(px) = text_px(x, y, text_x + 14, (q_y + 4) as usize, 1, quote_text, muted_color, band_bg, false) {
                        return px;
                    }
                }
            }
            LineKind::Image => {
                // An image occupies its placed rectangle at the block's
                // left inset. A refused image draws a card that names the
                // reason; nothing is drawn as though it were the picture.
                let image_x = pad + line.style.padding_left as usize;
                let inside = y as isize >= band_y
                    && (y as isize) < band_y + line_h as isize
                    && x >= pad
                    && x < pad + column;
                if inside {
                    let reason = if line.image_index == 255 {
                        Some(images::ImageRefusal::LimitReached)
                    } else if (line.image_index as usize) < app.image_count {
                        match app.images[line.image_index as usize].status {
                            images::ImageStatus::Decoded => None,
                            images::ImageStatus::Refused(refusal) => Some(refusal),
                        }
                    } else {
                        Some(images::ImageRefusal::LimitReached)
                    };
                    let slot = if line.image_index != 255 && (line.image_index as usize) < app.image_count {
                        Some(&app.images[line.image_index as usize])
                    } else {
                        None
                    };
                    if let (Some(slot), None) = (slot, reason) {
                        if let Some(bitmap) = slot.bitmap {
                            let ix = x.checked_sub(image_x);
                            let iy = (y as isize) - band_y;
                            if let Some(ix) = ix {
                                if iy >= 0
                                    && ix < slot.display_w
                                    && (iy as usize) < slot.display_h
                                {
                                    let argb = bitmap.pixel(ix, iy as usize, slot.display_w, slot.display_h);
                                    let alpha = (argb >> 24) & 0xFF;
                                    if alpha == 0xFF {
                                        return argb;
                                    }
                                    if alpha > 0 {
                                        // The blend resolves the alpha
                                        // against the band; the surface
                                        // pixel is opaque.
                                        return blend_toward(argb, band_bg, 255 - alpha)
                                            | 0xFF00_0000;
                                    }
                                }
                            }
                        }
                    } else if let Some(reason) = reason {
                        // A card is positioned from its top edge. While the
                        // image's top is scrolled above the viewport that
                        // edge is negative, and a placeholder card cannot be
                        // clipped into place; the band keeps its background
                        // until the top edge is visible.
                        let card_h = slot
                            .map(|slot| slot.display_h)
                            .unwrap_or(images::PLACEHOLDER_HEIGHT);
                        if band_y >= 0 {
                            let card_w = column
                                .saturating_sub(line.style.padding_left as usize)
                                .max(64);
                            let card = Rect::new(image_x, band_y as usize, card_w, card_h);
                            if let Some(card_bg) = draw_rounded_rect(
                                x,
                                y,
                                card,
                                R_SMALL,
                                THEME.surface_container_low,
                                Some(THEME.border),
                                1,
                            ) {
                                if let Some(slot) = slot {
                                    if slot.alt_len > 0 {
                                        let mut alt_buf = [0u8; 64];
                                        let alt_len = font::ellipsize(
                                            &slot.alt[..slot.alt_len.min(slot.alt.len())],
                                            1,
                                            card_w.saturating_sub(24),
                                            &mut alt_buf,
                                        );
                                        if let Some(px) = text_px(
                                            x,
                                            y,
                                            image_x + 12,
                                            band_y as usize + 8,
                                            1,
                                            &alt_buf[..alt_len],
                                            text_color,
                                            card_bg,
                                            true,
                                        ) {
                                            return px;
                                        }
                                    }
                                }
                                let reason_y = band_y as usize
                                    + if slot.map_or(0, |s| s.alt_len) > 0 {
                                        32
                                    } else {
                                        18
                                    };
                                if let Some(px) = text_px(
                                    x,
                                    y,
                                    image_x + 12,
                                    reason_y,
                                    1,
                                    reason.placeholder(),
                                    THEME.on_surface_variant,
                                    card_bg,
                                    false,
                                ) {
                                    return px;
                                }
                                return card_bg;
                            }
                        }
                    }
                }
            }
            LineKind::Paragraph => {
                let p_y = band_y + 4;
                let scale = if line.style.font_scale >= 2 { 2 } else { 1 };
                if y as isize >= p_y && (y as isize) < p_y + font::line_height(scale) as isize {
                    if let Some(px) = text_px(x, y, text_x, p_y as usize, scale, bytes, text_color, band_bg, false) {
                        return px;
                    }
                }
            }
            LineKind::Empty => {}
        }

        // Backgrounds and borders are painted after the glyph lookup so
        // text is never covered by the box that carries it.
        if let Some(px) = paint_style_band(x, y, band_y, line_h, line) {
            return px;
        }
        current_y += line_h as isize;
    }

    // The end of the document.
    //
    // WHY this says so: the reader holds a bounded number of lines, and a
    // page with more lines than that is an excerpt. Showing the first
    // lines without saying so presents a fragment as the article.
    if app.doc_total_lines > app.doc_line_count && app.doc_line_count > 0 {
        let note_y = current_y + 12;
        if y as isize >= note_y && (y as isize) < note_y + font::line_height(1) as isize {
            let mut buf = [0u8; 48];
            let mut n = 0usize;
            n = append_text(&mut buf, n, b"Showing the first ");
            n = append_u64_text(&mut buf, n, app.doc_line_count as u64);
            n = append_text(&mut buf, n, b" of ");
            n = append_u64_text(&mut buf, n, app.doc_total_lines as u64);
            n = append_text(&mut buf, n, b" lines");
            if let Some(px) = text_px(x, y, browser::CONTENT_PADDING, note_y as usize, 1, &buf[..n], THEME.on_surface_variant, THEME.surface, false) {
                return px;
            }
        }
    }

    // Scroll Position Indicator (on right edge x 712..716)
    //
    // WHY the platform's scroll model draws it: the thumb's position and
    // its size both come from the scroll state, so the thumb cannot leave
    // its track and it cannot be drawn for a document that does not
    // scroll.
    if x >= 712 && x < 716 && y >= 120 && y < 1340 {
        let track_h = 1340 - 120;
        let min_thumb_h = 60;
        if let Some((thumb_y, thumb_h)) = app.scroll.thumb_rect(120, track_h, min_thumb_h) {
            if y >= thumb_y && y < thumb_y + thumb_h {
                return THEME.on_surface_variant;
            }
        }
        return THEME.outline_variant;
    }

    THEME.surface
}

/// Paint the box a styled block contributes behind its text.
///
/// WHY this runs after the glyph lookup: a background or border drawn first
/// would be overwritten by the text pass only where glyphs exist, so the box
/// would hide its own label. Returning `None` outside the band keeps every
/// other pixel of the viewport on the existing path.
fn paint_style_band(
    x: usize,
    y: usize,
    band_y: isize,
    line_h: usize,
    line: &DocLine,
) -> Option<u32> {
    let pad = browser::CONTENT_PADDING;
    let column = browser::CONTENT_WIDTH;
    // A blank separator carries the style of the block it follows, because
    // the marker that restores the enclosing style comes after it. It is
    // not part of the block's box, or the box would gain a second bottom
    // border and a coloured gap below the text.
    if line.kind == LineKind::Empty {
        return None;
    }
    if (y as isize) < band_y || (y as isize) >= band_y + line_h as isize {
        return None;
    }
    if x < pad || x >= pad + column {
        return None;
    }
    if line.style.has_border {
        let bottom = band_y + line_h as isize - 1;
        let top_edge = line.block_start && (y as isize) == band_y;
        let bottom_edge = line.block_end && (y as isize) == bottom;
        let side = x == pad || x == pad + column - 1;
        if top_edge || bottom_edge || side {
            return Some(line.style.border_color);
        }
    }
    if line.style.has_background {
        return Some(line.style.background);
    }
    None
}

// -----------------------------------------------------------------------------
// Sheet Overlays Renderer (Tab Switcher, Bookmarks, History, Menu)
// -----------------------------------------------------------------------------
/// The file name a download is listed under, without its directory.
fn download_name(path: &[u8]) -> &[u8] {
    match path.iter().rposition(|&b| b == b'/') {
        Some(slash) => &path[slash + 1..],
        None => path,
    }
}

fn render_sheet_overlay(x: usize, y: usize, app: &BrowserApp) -> Option<u32> {
    match app.overlay {
        SheetOverlay::None => None,
        SheetOverlay::TabSwitcher => {
            // Dark dim overlay across viewport
            if y < TOOLBAR_Y1 || y >= NAV_BAR_Y0 {
                return None;
            }
            // Tab Switcher Header (y 120..170)
            if y >= 120 && y < 170 {
                let header = b"Open Tabs";
                if let Some(px) = text_px(x, y, 32, 134, 1, header, THEME.on_surface, THEME.surface, true) {
                    return Some(px);
                }
                // "+ New Tab" button in the header.
                if let Some(c) = draw_rounded_rect(x, y, NEW_TAB_BTN, R_SMALL, THEME.primary, None, 0) {
                    let label = b"+ New Tab";
                    if let Some(px) = text_px(x, y, 560, 138, 1, label, THEME.on_primary, c, true) {
                        return Some(px);
                    }
                    return Some(c);
                }
                return Some(THEME.surface);
            }

            // Tab cards stacked vertically (4 slots)
            let card_h = 110;
            let card_gap = 16;
            for i in 0..MAX_TABS {
                let card_y = 180 + i * (card_h + card_gap);
                let card_rect = Rect::new(24, card_y, W - 48, card_h);
                let tab = app.tabs.tab(i);
                let is_active = app.tabs.active_tab_index() == i;

                if let Some(tab_data) = tab {
                    let border_col = if is_active { THEME.primary } else { THEME.border };
                    let border_w = if is_active { 2 } else { 1 };
                    if let Some(c) = draw_rounded_rect(x, y, card_rect, CARD.radius, CARD.fill, Some(border_col), border_w) {
                        // Tab badge `[i+1]`
                        let badge_rect = Rect::new(40, card_y + 16, 28, 24);
                        if let Some(bc) = draw_rounded_rect(x, y, badge_rect, R_SMALL, THEME.surface_container, None, 0) {
                            let digit = [b'1' + (i as u8)];
                            if let Some(px) = text_px(x, y, 48, card_y + 20, 1, &digit, THEME.primary, bc, true) {
                                return Some(px);
                            }
                            return Some(bc);
                        }

                        // Tab Title
                        //
                        // WHY both runs are ellipsized here: the title and the
                        // URL are stored at their full length, and drawing
                        // them unmeasured let them run through the close glyph
                        // and off the card.
                        let title = tab_data.title_bytes();
                        let mut tbuf = [0u8; 64];
                        let tn = font::ellipsize(title, 1, 540, &mut tbuf);
                        if let Some(px) = text_px(x, y, 78, card_y + 20, 1, &tbuf[..tn], THEME.on_surface, c, true) {
                            return Some(px);
                        }

                        // Tab URL
                        let url = tab_data.url();
                        let mut ubuf = [0u8; 128];
                        let un = font::ellipsize(url, 1, 580, &mut ubuf);
                        if let Some(px) = text_px(x, y, 40, card_y + 54, 1, &ubuf[..un], THEME.on_surface_variant, c, false) {
                            return Some(px);
                        }

                        // Close button: the target is painted, so the box a
                        // person can see is the box that answers the tap.
                        //
                        // WHY the box carries an edge: the tile is white and the
                        // card it sits on is the raised surface, so the fill was
                        // 1.16:1 against the card -- a target nobody could find.
                        // The edge is the platform's secondary ink, 4.59:1 on
                        // that card, and the rectangle is still the one
                        // `tab_close_rect` answers a tap in.
                        let close_rect = tab_close_rect(card_y);
                        if let Some(cc) = draw_rounded_rect(
                            x,
                            y,
                            close_rect,
                            R_SMALL,
                            THEME.surface_container,
                            Some(THEME.on_surface_variant),
                            1,
                        ) {
                            if close_glyph(x, y, close_rect.x + close_rect.w / 2, close_rect.y + close_rect.h / 2) {
                                return Some(THEME.on_surface_variant);
                            }
                            return Some(cc);
                        }
                        return Some(c);
                    }
                } else {
                    // Empty slot dashed/dim card
                    if let Some(c) = draw_rounded_rect(x, y, card_rect, CARD.radius, CARD.fill, CARD.border, 1) {
                        let label = b"+ Empty Tab Slot";
                        if let Some(px) = text_px(x, y, 40, card_y + 44, 1, label, THEME.on_surface_variant, c, false) {
                            return Some(px);
                        }
                        return Some(c);
                    }
                }
            }
            Some(THEME.surface)
        }
        SheetOverlay::Bookmarks => {
            if y < TOOLBAR_Y1 || y >= NAV_BAR_Y0 {
                return None;
            }
            // Bookmarks Header
            if y >= 120 && y < 170 {
                let header = b"Saved Bookmarks";
                if let Some(px) = text_px(x, y, 32, 134, 1, header, THEME.on_surface, THEME.surface, true) {
                    return Some(px);
                }
                // Close button 'X' at x=650, y=140
                if close_glyph(x, y, 660, 145) {
                    return Some(THEME.on_surface_variant);
                }
                return Some(THEME.surface);
            }

            // The filter row.
            //
            // WHY the list has a filter: a list of everything a person has
            // saved cannot be found in without one, and a bookmark the
            // person cannot find is a bookmark they did not keep.
            if y >= FILTER_Y && y < FILTER_Y + FILTER_H {
                if let Some(px) = filter_row_pixel(x, y, app, b"Filter bookmarks") {
                    return Some(px);
                }
                return Some(THEME.surface);
            }

            // List of bookmarks
            let count = app.bookmarks.len();
            let filter = app.filter_bytes();
            if count == 0 {
                let empty_msg = b"No bookmarks saved yet.";
                if let Some(px) = text_px(x, y, 32, 250, 1, empty_msg, THEME.on_surface_variant, THEME.surface, false) {
                    return Some(px);
                }
            } else {
                let row_h = BOOKMARK_ROW_PITCH;
                let mut shown = 0usize;
                for i in 0..count {
                    if let Some(rec) = app.bookmarks.record(i) {
                        if !browser::contains_ignore_case(rec.title_bytes(), filter)
                            && !browser::contains_ignore_case(rec.url_bytes(), filter)
                        {
                            continue;
                        }
                        let r_y = LIST_Y + shown * row_h;
                        let r_rect = Rect::new(24, r_y, W - 48, BOOKMARK_CARD_H);
                        if let Some(c) = draw_rounded_rect(x, y, r_rect, R_CHIP, THEME.surface_container_high, Some(THEME.border), 1) {
                            if star_glyph(x, y, 48, r_y + 35, true) {
                                return Some(THEME.primary);
                            }
                            if let Some(px) = text_px(x, y, 70, r_y + 16, 1, rec.title_bytes(), THEME.on_surface, c, true) {
                                return Some(px);
                            }
                            if let Some(px) = text_px(x, y, 70, r_y + 42, 1, rec.url_bytes(), THEME.on_surface_variant, c, false) {
                                return Some(px);
                            }
                            return Some(c);
                        }
                        shown += 1;
                    }
                }
                if shown == 0 {
                    let empty_msg = b"No bookmark matches the filter.";
                    if let Some(px) = text_px(x, y, 32, 250, 1, empty_msg, THEME.on_surface_variant, THEME.surface, false) {
                        return Some(px);
                    }
                }
            }
            Some(THEME.surface)
        }
        SheetOverlay::History => {
            if y < TOOLBAR_Y1 || y >= NAV_BAR_Y0 {
                return None;
            }
            // History Header
            if y >= 120 && y < 170 {
                let header = b"Navigation History";
                if let Some(px) = text_px(x, y, 32, 134, 1, header, THEME.on_surface, THEME.surface, true) {
                    return Some(px);
                }
                if close_glyph(x, y, 660, 145) {
                    return Some(THEME.on_surface_variant);
                }
                return Some(THEME.surface);
            }

            // The filter row.
            if y >= FILTER_Y && y < FILTER_Y + FILTER_H {
                if let Some(px) = filter_row_pixel(x, y, app, b"Filter history") {
                    return Some(px);
                }
                return Some(THEME.surface);
            }

            let count = app.history.len();
            if count == 0 {
                let empty_msg = b"No history recorded yet.";
                if let Some(px) = text_px(x, y, 32, 250, 1, empty_msg, THEME.on_surface_variant, THEME.surface, false) {
                    return Some(px);
                }
            } else {
                let mut drawn = false;
                for row in 0..HISTORY_ROWS {
                    let Some(visit) = history_row(app, row) else {
                        break;
                    };
                    drawn = true;
                    let r_y = LIST_Y + row * HISTORY_ROW_PITCH;
                    let r_rect = Rect::new(HISTORY_CARD_X, r_y, HISTORY_CARD_W, HISTORY_CARD_H);
                    if let Some(c) = draw_rounded_rect(x, y, r_rect, R_SMALL, THEME.surface_container_high, Some(THEME.border), 1) {
                        // WHY the address is ellipsized: a recorded address
                        // fills its buffer, and drawing it unmeasured ran it
                        // through the card edge and off the screen. The width
                        // is the card's inner width: the inset named beside the
                        // text on each side of it.
                        let mut line = [0u8; 128];
                        // WHY a page that named itself gets two lines: the
                        // name is what the person remembers and the address is
                        // what the log was kept under. A page that named
                        // nothing has only the address to show, so it keeps the
                        // one line it always had.
                        if visit.title.is_empty() {
                            let n = font::ellipsize(visit.url, HISTORY_RUN_SCALE, HISTORY_TEXT_MAX, &mut line);
                            if let Some(px) = text_px(
                                x,
                                y,
                                HISTORY_TEXT_X,
                                r_y + HISTORY_SINGLE_Y,
                                HISTORY_RUN_SCALE,
                                &line[..n],
                                THEME.on_surface,
                                c,
                                false,
                            ) {
                                return Some(px);
                            }
                        } else {
                            let n = font::ellipsize(visit.title, HISTORY_RUN_SCALE, HISTORY_TEXT_MAX, &mut line);
                            if let Some(px) = text_px(
                                x,
                                y,
                                HISTORY_TEXT_X,
                                r_y + HISTORY_BLOCK_Y,
                                HISTORY_RUN_SCALE,
                                &line[..n],
                                THEME.on_surface,
                                c,
                                true,
                            ) {
                                return Some(px);
                            }
                            let n = font::ellipsize(visit.url, HISTORY_RUN_SCALE, HISTORY_TEXT_MAX, &mut line);
                            if let Some(px) = text_px(
                                x,
                                y,
                                HISTORY_TEXT_X,
                                r_y + HISTORY_SECOND_Y,
                                HISTORY_RUN_SCALE,
                                &line[..n],
                                THEME.on_surface_variant,
                                c,
                                false,
                            ) {
                                return Some(px);
                            }
                        }
                        return Some(c);
                    }
                }
                if !drawn {
                    let empty_msg = b"No page matches the filter.";
                    if let Some(px) = text_px(x, y, 32, 250, 1, empty_msg, THEME.on_surface_variant, THEME.surface, false) {
                        return Some(px);
                    }
                }
            }
            Some(THEME.surface)
        }
        SheetOverlay::Downloads => {
            if y < TOOLBAR_Y1 || y >= NAV_BAR_Y0 {
                return None;
            }
            // Downloads Header
            if y >= 120 && y < 170 {
                let header = b"Downloads";
                if let Some(px) = text_px(x, y, 32, 134, 1, header, THEME.on_surface, THEME.surface, true) {
                    return Some(px);
                }
                if close_glyph(x, y, 660, 145) {
                    return Some(THEME.on_surface_variant);
                }
                return Some(THEME.surface);
            }

            // The pages the browser saved and the files it downloaded.
            //
            // WHY the list is the platform's index and not the app's own
            // counter: the index is what the file manager and the launcher
            // read, so this list and theirs name the same files.
            if app.download_count == 0 {
                let empty_msg = b"Nothing downloaded yet. Save a page or download a file.";
                if let Some(px) = text_px(x, y, 32, 220, 1, empty_msg, THEME.on_surface_variant, THEME.surface, false) {
                    return Some(px);
                }
            } else {
                let row_h = 72;
                for i in 0..app.download_count {
                    let r_y = 180 + i * row_h;
                    let r_rect = Rect::new(24, r_y, W - 48, row_h - 10);
                    let entry = &app.downloads[i];
                    if let Some(c) = draw_rounded_rect(x, y, r_rect, R_CHIP, THEME.surface_container_high, Some(THEME.border), 1) {
                        // WHY the path is ellipsized: the saved path fills its
                        // buffer, and drawing it unmeasured ran it through the
                        // card and off the screen. The width is the card's inner
                        // width: twenty pixels of inset on each side of the text
                        // at x 44.
                        // WHY only the file name is shown: the directory is the
                        // same for every row, and printing it pushed the part
                        // that tells downloads apart into the ellipsis.
                        let shown = download_name(&entry.path[..entry.path_len]);
                        let mut path = [0u8; 96];
                        let n = font::ellipsize(shown, 1, r_rect.w - 40, &mut path);
                        if let Some(px) = text_px(x, y, 44, r_y + 12, 1, &path[..n], THEME.on_surface, c, true) {
                            return Some(px);
                        }
                        let mut size_buf = [0u8; 48];
                        let mut n = 0usize;
                        n = append_text(
                            &mut size_buf,
                            n,
                            if entry.is_page {
                                b"Saved page, "
                            } else {
                                b"Downloaded file, "
                            },
                        );
                        n = append_u64_text(&mut size_buf, n, entry.length);
                        n = append_text(&mut size_buf, n, b" bytes");
                        if let Some(px) = text_px(x, y, 44, r_y + 40, 1, &size_buf[..n], THEME.on_surface_variant, c, false) {
                            return Some(px);
                        }
                        return Some(c);
                    }
                }
                if app.downloads_error {
                    let msg = b"No installed app handles this file's type.";
                    if let Some(px) = text_px(x, y, 32, 180 + app.download_count * 72 + 12, 1, msg, THEME.error, THEME.surface, false) {
                        return Some(px);
                    }
                }
            }
            Some(THEME.surface)
        }
        SheetOverlay::Tools => {
            if y < TOOLBAR_Y1 || y >= NAV_BAR_Y0 {
                return None;
            }
            if y >= 120 && y < 170 {
                if let Some(px) = text_px(x, y, 32, 134, 1, b"Tools", THEME.on_surface, THEME.surface, true) {
                    return Some(px);
                }
                if close_glyph(x, y, 660, 145) {
                    return Some(THEME.on_surface_variant);
                }
                return Some(THEME.surface);
            }
            let caption = b"Uploads, measurements, and samples run on this device; nothing is estimated.";
            if let Some(px) = text_px(x, y, 32, 182, 1, caption, THEME.on_surface_variant, THEME.surface, false) {
                return Some(px);
            }
            let rows: [(&[u8], &[u8]); 4] = [
                (
                    b"Upload a File",
                    b"Send a document from the content index to the gateway.",
                ),
                (
                    b"Speed Test",
                    b"Measure download, upload, and first-byte time.",
                ),
                (
                    b"Compatibility Lab",
                    b"Fetch four fixed sites and record what came back.",
                ),
                (
                    b"Test Files",
                    b"Download the gateway's media samples for other apps.",
                ),
            ];
            for (index, (title, body)) in rows.iter().enumerate() {
                let top = 220 + index * 116;
                let rect = Rect::new(24, top, W - 48, 100);
                if let Some(c) = draw_rounded_rect(x, y, rect, CARD.radius, CARD.fill, CARD.border, 1) {
                    if let Some(px) = text_px(x, y, 44, top + 18, 1, title, THEME.on_surface, THEME.surface, true) {
                        return Some(px);
                    }
                    if let Some(px) = text_px(x, y, 44, top + 52, 1, body, THEME.on_surface_variant, THEME.surface, false) {
                        return Some(px);
                    }
                    return Some(c);
                }
            }
            Some(THEME.surface)
        }
        SheetOverlay::Settings => {
            if y < TOOLBAR_Y1 || y >= NAV_BAR_Y0 {
                return None;
            }
            if y >= 120 && y < 170 {
                if let Some(px) = text_px(x, y, 32, 134, 1, b"Settings", THEME.on_surface, THEME.surface, true) {
                    return Some(px);
                }
                if close_glyph(x, y, 660, 145) {
                    return Some(THEME.on_surface_variant);
                }
                return Some(THEME.surface);
            }
            let caption = b"Browser configuration, search engine, and device settings.";
            if let Some(px) = text_px(x, y, 32, 182, 1, caption, THEME.on_surface_variant, THEME.surface, false) {
                return Some(px);
            }
            let rows: [(&[u8], &[u8]); 4] = [
                (
                    b"Search Engine",
                    b"Default search: DuckDuckGo / InterSearch.",
                ),
                (
                    b"Homepage",
                    b"Default start page: Sovereign New Tab (about:blank).",
                ),
                (
                    b"Security & Sandbox",
                    b"Zero ambient authority with explicit Ring-3 capability ladder.",
                ),
                (
                    b"System Settings",
                    b"Tap to open system-wide InterOS configuration.",
                ),
            ];
            for (index, (title, body)) in rows.iter().enumerate() {
                let top = 220 + index * 116;
                let rect = Rect::new(24, top, W - 48, 100);
                if let Some(c) = draw_rounded_rect(x, y, rect, CARD.radius, CARD.fill, CARD.border, 1) {
                    if let Some(px) = text_px(x, y, 44, top + 18, 1, title, THEME.on_surface, THEME.surface, true) {
                        return Some(px);
                    }
                    if let Some(px) = text_px(x, y, 44, top + 52, 1, body, THEME.on_surface_variant, THEME.surface, false) {
                        return Some(px);
                    }
                    return Some(c);
                }
            }
            Some(THEME.surface)
        }
        SheetOverlay::UploadPicker => {
            if y < TOOLBAR_Y1 || y >= NAV_BAR_Y0 {
                return None;
            }
            if y >= 120 && y < 170 {
                if let Some(px) = text_px(x, y, 32, 134, 1, b"Upload a File", THEME.on_surface, THEME.surface, true) {
                    return Some(px);
                }
                if close_glyph(x, y, 660, 145) {
                    return Some(THEME.on_surface_variant);
                }
                return Some(THEME.surface);
            }
            let caption = b"Pick a document. An upload is complete only when the gateway confirms every byte.";
            if let Some(px) = text_px(x, y, 32, 182, 1, caption, THEME.on_surface_variant, THEME.surface, false) {
                return Some(px);
            }
            if app.upload_count == 0 {
                if let Some(px) = text_px(
                    x,
                    y,
                    32,
                    236,
                    1,
                    b"The content index holds no documents yet.",
                    THEME.on_surface_variant,
                    THEME.surface,
                    false,
                ) {
                    return Some(px);
                }
            } else {
                let row_h = 84;
                for i in 0..app.upload_count {
                    let r_y = 220 + i * row_h;
                    let entry = &app.uploads[i];
                    let rect = Rect::new(24, r_y, W - 48, row_h - 10);
                    if let Some(c) = draw_rounded_rect(x, y, rect, R_CHIP, THEME.surface_container_high, Some(THEME.border), 1) {
                        let mut name = [0u8; 64];
                        let n = font::ellipsize(&entry.path[..entry.path_len], 1, 600, &mut name);
                        if let Some(px) = text_px(x, y, 44, r_y + 12, 1, &name[..n], THEME.on_surface, c, true) {
                            return Some(px);
                        }
                        let mut meta = [0u8; 72];
                        let m = append_text(&mut meta, 0, &entry.mime[..entry.mime_len]);
                        let m = append_text(&mut meta, m, b", ");
                        let m = append_u64_text(&mut meta, m, entry.length);
                        let m = append_text(&mut meta, m, b" bytes");
                        if let Some(px) = text_px(x, y, 44, r_y + 42, 1, &meta[..m], THEME.on_surface_variant, c, false) {
                            return Some(px);
                        }
                        return Some(c);
                    }
                }
            }
            if app.upload_state != TransferState::Idle {
                let panel = Rect::new(24, 1000, W - 48, 190);
                if let Some(c) = draw_rounded_rect(x, y, panel, CARD.radius, CARD.fill, CARD.border, 1) {
                    let state: &[u8] = match app.upload_state {
                        TransferState::Idle => b"",
                        TransferState::Running => b"UPLOADING",
                        TransferState::Complete => b"UPLOAD COMPLETE",
                        TransferState::Refused => b"UPLOAD REFUSED",
                        TransferState::Failed => b"UPLOAD FAILED",
                    };
                    let state_color = if app.upload_state == TransferState::Complete {
                        THEME.primary
                    } else {
                        THEME.on_surface_variant
                    };
                    if let Some(px) = text_px(x, y, 44, 1016, 1, state, state_color, c, true) {
                        return Some(px);
                    }
                    let mut name = [0u8; 64];
                    let n = font::ellipsize(&app.upload_name[..app.upload_name_len], 1, 600, &mut name);
                    if let Some(px) = text_px(x, y, 44, 1050, 1, &name[..n], THEME.on_surface, c, false) {
                        return Some(px);
                    }
                    let mut line = [0u8; 128];
                    let n = match app.upload_state {
                        TransferState::Running => {
                            append_text(&mut line, 0, b"Sending; no byte count is confirmed yet.")
                        }
                        TransferState::Complete => {
                            let n = append_text(&mut line, 0, b"Sent ");
                            let n = append_u64_text(&mut line, n, app.upload_sent as u64);
                            let n = append_text(&mut line, n, b" bytes. The gateway confirmed ");
                            let n = append_u64_text(&mut line, n, app.upload_confirmed);
                            append_text(&mut line, n, b" bytes.")
                        }
                        TransferState::Refused => {
                            append_text(&mut line, 0, b"Larger than the 4 MiB limit; nothing was sent.")
                        }
                        TransferState::Failed | TransferState::Idle => {
                            append_text(&mut line, 0, b"The upload did not complete; no byte count was confirmed.")
                        }
                    };
                    if let Some(px) = text_px(x, y, 44, 1082, 1, &line[..n], THEME.on_surface_variant, c, false) {
                        return Some(px);
                    }
                    if app.upload_state == TransferState::Failed && app.upload_error != 0 {
                        let mut code = [0u8; 40];
                        let n = append_text(&mut code, 0, b"Transport error ");
                        let n = append_signed_text(&mut code, n, app.upload_error);
                        if let Some(px) = text_px(x, y, 44, 1114, 1, &code[..n], THEME.on_surface_variant, c, false) {
                            return Some(px);
                        }
                    }
                    return Some(c);
                }
            }
            Some(THEME.surface)
        }
        SheetOverlay::SpeedTest => {
            if y < TOOLBAR_Y1 || y >= NAV_BAR_Y0 {
                return None;
            }
            if y >= 120 && y < 170 {
                if let Some(px) = text_px(x, y, 32, 134, 1, b"Speed Test", THEME.on_surface, THEME.surface, true) {
                    return Some(px);
                }
                if close_glyph(x, y, 660, 145) {
                    return Some(THEME.on_surface_variant);
                }
                return Some(THEME.surface);
            }
            let caption = b"Every number is measured on this device through the Studio gateway. Nothing is estimated.";
            if let Some(px) = text_px(x, y, 32, 182, 1, caption, THEME.on_surface_variant, THEME.surface, false) {
                return Some(px);
            }
            if app.speed_running {
                if let Some(px) = text_px(
                    x,
                    y,
                    32,
                    228,
                    1,
                    b"Measuring download, first byte, and upload...",
                    THEME.on_surface,
                    THEME.surface,
                    true,
                ) {
                    return Some(px);
                }
                return Some(THEME.surface);
            }
            let mut line1 = [0u8; 96];
            let mut line2 = [0u8; 96];
            let mut top = 228;
            for row in 0..3usize {
                let (title, state, bytes, ms, bps, confirmed) = match row {
                    0 => (
                        b"DOWNLOAD".as_slice(),
                        app.speed.download,
                        app.speed.download_bytes,
                        app.speed.download_ms,
                        app.speed.download_bps,
                        0u64,
                    ),
                    1 => (
                        b"FIRST BYTE".as_slice(),
                        app.speed.latency,
                        0usize,
                        app.speed.latency_ms,
                        0u64,
                        0u64,
                    ),
                    _ => (
                        b"UPLOAD".as_slice(),
                        app.speed.upload,
                        app.speed.upload_bytes,
                        app.speed.upload_ms,
                        app.speed.upload_bps,
                        app.speed.upload_confirmed,
                    ),
                };
                let n1;
                let n2;
                match state {
                    TransferState::Complete => {
                        if row == 1 {
                            n1 = if ms == 0 {
                                append_text(&mut line1, 0, b"First byte under 1 ms")
                            } else {
                                let n = append_text(&mut line1, 0, b"First byte in ");
                                let n = append_u64_text(&mut line1, n, ms);
                                append_text(&mut line1, n, b" ms")
                            };
                            n2 = if ms == 0 {
                                append_text(&mut line2, 0, b"Below the clock's 1 ms resolution.")
                            } else {
                                append_text(&mut line2, 0, b"Round trip through the gateway.")
                            };
                        } else if ms == 0 {
                            n1 = {
                                let n = append_text(&mut line1, 0, b"");
                                let n = append_u64_text(&mut line1, n, bytes as u64);
                                let n = append_text(&mut line1, n, b" bytes sent");
                                if row == 2 {
                                    let n = append_text(&mut line1, n, b", ");
                                    let n = append_u64_text(&mut line1, n, confirmed);
                                    append_text(&mut line1, n, b" confirmed")
                                } else {
                                    n
                                }
                            };
                            n2 = append_text(&mut line2, 0, b"Elapsed time was below the clock's 1 ms resolution.");
                        } else {
                            n1 = {
                                let n = append_text(&mut line1, 0, b"");
                                let n = append_u64_text(&mut line1, n, bytes as u64);
                                let n = append_text(&mut line1, n, b" bytes in ");
                                let n = append_u64_text(&mut line1, n, ms);
                                let n = append_text(&mut line1, n, b" ms");
                                if row == 2 {
                                    let n = append_text(&mut line1, n, b", ");
                                    let n = append_u64_text(&mut line1, n, confirmed);
                                    append_text(&mut line1, n, b" confirmed")
                                } else {
                                    n
                                }
                            };
                            n2 = append_rate_text(&mut line2, 0, bps);
                        }
                    }
                    TransferState::Failed => {
                        n1 = append_text(&mut line1, 0, b"Not measured.");
                        n2 = if app.speed.error == -1000 {
                            append_text(&mut line2, 0, b"The clock was unavailable, so no rate was computed.")
                        } else {
                            append_text(&mut line2, 0, b"The gateway did not complete the transfer.")
                        };
                    }
                    _ => {
                        n1 = append_text(&mut line1, 0, b"Not run.");
                        n2 = append_text(&mut line2, 0, b"Run the test from Tools.");
                    }
                }
                if let Some(px) = info_card_pixel(x, y, top, title, &line1[..n1], &line2[..n2]) {
                    return Some(px);
                }
                top += 160;
            }
            Some(THEME.surface)
        }
        SheetOverlay::CompatibilityLab => {
            if y < TOOLBAR_Y1 || y >= NAV_BAR_Y0 {
                return None;
            }
            if y >= 120 && y < 170 {
                if let Some(px) = text_px(x, y, 32, 134, 1, b"Compatibility Lab", THEME.on_surface, THEME.surface, true) {
                    return Some(px);
                }
                if close_glyph(x, y, 660, 145) {
                    return Some(THEME.on_surface_variant);
                }
                return Some(THEME.surface);
            }
            let caption = b"Each row is the gateway's answer to this reader. No JavaScript runs and nothing is saved.";
            if let Some(px) = text_px(x, y, 32, 182, 1, caption, THEME.on_surface_variant, THEME.surface, false) {
                return Some(px);
            }
            if app.lab_running {
                if let Some(px) = text_px(x, y, 32, 228, 1, b"Fetching the fixed addresses...", THEME.on_surface, THEME.surface, true) {
                    return Some(px);
                }
                return Some(THEME.surface);
            }
            let mut line1 = [0u8; 96];
            let mut line2 = [0u8; 96];
            for index in 0..LAB_URLS.len() {
                let result = &app.lab[index];
                let top = 220 + index * 160;
                let mut name = [0u8; 64];
                let n = font::ellipsize(LAB_URLS[index], 1, 600, &mut name);
                let n1;
                let n2;
                match result.state {
                    TransferState::Complete => {
                        let mut n = append_text(&mut line1, 0, b"HTTP ");
                        n = append_u64_text(&mut line1, n, result.status as u64);
                        n = append_text(&mut line1, n, b", ");
                        n = append_u64_text(&mut line1, n, result.bytes as u64);
                        n = append_text(&mut line1, n, b" bytes, ");
                        n1 = append_text(&mut line1, n, &result.content_type[..result.content_type_len]);
                        let n = append_u64_text(&mut line2, 0, result.reader_lines as u64);
                        n2 = append_text(&mut line2, n, b" reader lines extracted.");
                    }
                    TransferState::Refused => {
                        n1 = if result.status != 0 {
                            let n = append_text(&mut line1, 0, b"HTTP ");
                            let n = append_u64_text(&mut line1, n, result.status as u64);
                            let n = append_text(&mut line1, n, b", ");
                            append_text(&mut line1, n, &result.content_type[..result.content_type_len])
                        } else {
                            append_text(&mut line1, 0, b"The gateway refused the fetch.")
                        };
                        n2 = if result.error == -2 {
                            append_text(&mut line2, 0, b"The response is a file, not a reader document.")
                        } else {
                            append_text(&mut line2, 0, b"The origin did not return a page.")
                        };
                    }
                    TransferState::Failed => {
                        if result.status != 0 {
                            let mut n = append_text(&mut line1, 0, b"HTTP ");
                            n = append_u64_text(&mut line1, n, result.status as u64);
                            n = append_text(&mut line1, n, b", ");
                            let content_type: &[u8] = if result.content_type_len == 0 {
                                b"no content type".as_slice()
                            } else {
                                &result.content_type[..result.content_type_len]
                            };
                            n1 = append_text(&mut line1, n, content_type);
                            n2 = append_text(&mut line2, 0, b"The gateway answered without a document.");
                        } else {
                            n1 = append_text(&mut line1, 0, b"No response.");
                            let n = append_text(&mut line2, 0, b"Transport error ");
                            n2 = append_signed_text(&mut line2, n, result.error);
                        }
                    }
                    _ => {
                        n1 = append_text(&mut line1, 0, b"Not fetched.");
                        n2 = append_text(&mut line2, 0, b"The gateway returned no readable answer.");
                    }
                }
                if let Some(px) = info_card_pixel(x, y, top, &name[..n], &line1[..n1], &line2[..n2]) {
                    return Some(px);
                }
            }
            Some(THEME.surface)
        }
        SheetOverlay::TestFiles => {
            if y < TOOLBAR_Y1 || y >= NAV_BAR_Y0 {
                return None;
            }
            if y >= 120 && y < 170 {
                if let Some(px) = text_px(x, y, 32, 134, 1, b"Test Files", THEME.on_surface, THEME.surface, true) {
                    return Some(px);
                }
                if close_glyph(x, y, 660, 145) {
                    return Some(THEME.on_surface_variant);
                }
                return Some(THEME.surface);
            }
            let caption = b"Built by the gateway. Tap one to download it into shared storage.";
            if let Some(px) = text_px(x, y, 32, 182, 1, caption, THEME.on_surface_variant, THEME.surface, false) {
                return Some(px);
            }
            let row_h = 84;
            for (index, (name, content_type)) in TEST_FIXTURES.iter().enumerate() {
                let r_y = 220 + index * row_h;
                let rect = Rect::new(24, r_y, W - 48, row_h - 10);
                if let Some(c) = draw_rounded_rect(x, y, rect, R_CHIP, THEME.surface_container_high, Some(THEME.border), 1) {
                    if let Some(px) = text_px(x, y, 44, r_y + 12, 1, name, THEME.on_surface, c, true) {
                        return Some(px);
                    }
                    if let Some(px) = text_px(x, y, 44, r_y + 42, 1, content_type, THEME.on_surface_variant, c, false) {
                        return Some(px);
                    }
                    return Some(c);
                }
            }
            Some(THEME.surface)
        }
        SheetOverlay::Menu => {
            // Kebab Overflow Menu dropdown card at top right (x 440..696, y 116..532)
            if let Some(c) = draw_rounded_rect(
                x,
                y,
                MENU_CARD,
                R_CONTROL,
                THEME.surface_container_high,
                Some(THEME.border),
                2,
            ) {
                for (idx, item) in MENU_ITEMS.iter().enumerate() {
                    let item_y = MENU_ROW_Y0 + idx * MENU_ROW_H;
                    if let Some(px) = text_px(x, y, 464, item_y, 1, item, THEME.on_surface, c, false) {
                        return Some(px);
                    }
                    if idx + 1 < MENU_ITEMS.len() && y == item_y + 30 {
                        return Some(THEME.outline_variant);
                    }
                }
                return Some(c);
            }
            None
        }
    }
}

/// A card in a results sheet: a title and two lines of evidence.
///
/// WHY one helper: the speed test and the compatibility lab are both a
/// list of measured rows, and a row that drew its own card would drift
/// from the next one.
fn info_card_pixel(
    x: usize,
    y: usize,
    top: usize,
    title: &[u8],
    line1: &[u8],
    line2: &[u8],
) -> Option<u32> {
    let rect = Rect::new(24, top, W - 48, 150);
    let c = draw_rounded_rect(x, y, rect, CARD.radius, CARD.fill, CARD.border, 1)?;
    if let Some(px) = text_px(x, y, 44, top + 16, 1, title, THEME.on_surface_variant, c, false) {
        return Some(px);
    }
    if let Some(px) = text_px(x, y, 44, top + 52, 1, line1, THEME.on_surface, c, true) {
        return Some(px);
    }
    if let Some(px) = text_px(x, y, 44, top + 90, 1, line2, THEME.on_surface_variant, c, false) {
        return Some(px);
    }
    Some(c)
}

/// The pixel of the list filter row at (`x`, `y`).
///
/// WHY the row carries the filter as text: a person has to be able to see
/// what the list is being narrowed to, and a filter that was invisible
/// could not be corrected either.
fn filter_row_pixel(x: usize, y: usize, app: &BrowserApp, hint: &[u8]) -> Option<u32> {
    let rect = Rect::new(24, FILTER_Y, W - 48, FILTER_H);
    let c = draw_rounded_rect(x, y, rect, R_CONTROL, THEME.surface_container_high, Some(THEME.border), 1)?;
    // The magnifier.
    let cx = 46isize;
    let cy = (FILTER_Y + FILTER_H / 2) as isize;
    let dx = x as isize - cx;
    let dy = y as isize - cy;
    if dx * dx + dy * dy <= 25 {
        return Some(THEME.on_surface_variant);
    }
    if dx >= 6 && dx <= 12 && dy >= 6 && dy <= 12 {
        return Some(THEME.on_surface_variant);
    }
    let filter = app.filter_bytes();
    if filter.is_empty() {
        if let Some(px) = text_px(x, y, 68, FILTER_Y + 12, 1, hint, THEME.on_surface_variant, c, false) {
            return Some(px);
        }
    } else {
        if let Some(px) = text_px(x, y, 68, FILTER_Y + 12, 1, filter, THEME.on_surface, c, true) {
            return Some(px);
        }
    }
    // The clear control, only when there is something to clear.
    if !filter.is_empty() {
        if close_glyph(x, y, W - 48, FILTER_Y + FILTER_H / 2) {
            return Some(THEME.on_surface_variant);
        }
    }
    Some(c)
}

// -----------------------------------------------------------------------------
// Bottom Navigation Bar (y 1350..1410)
// -----------------------------------------------------------------------------
fn bottom_bar_pixel(x: usize, y: usize, app: &BrowserApp) -> u32 {
    if y == NAV_BAR_Y0 {
        return THEME.outline_variant;
    }

    // WHY each block returns its own box: the bar used to discard the pixel the
    // control was painted with and fall through to the bar's own colour, so the
    // tile and the hairline below were never on the screen at all. Returning the
    // control's pixel is what makes the box the tap already answered visible.
    // 1. Back Button `<`
    if let Some(c) = chrome_control_px(x, y, BACK_BTN, BTN_R) {
        let cx = BACK_BTN.x + BACK_BTN.w / 2;
        let cy = BACK_BTN.y + BACK_BTN.h / 2;
        let active = app.tabs.can_go_back();
        let col = nav_glyph_ink(active);
        if let Some(c) = std::design::back_button::chevron_ink(x, y, cx, cy, col) {
            return c;
        }
        return c;
    }

    // 2. Forward Button `>`
    if let Some(c) = chrome_control_px(x, y, FWD_BTN, BTN_R) {
        let cx = FWD_BTN.x + FWD_BTN.w / 2;
        let cy = FWD_BTN.y + FWD_BTN.h / 2;
        let active = app.tabs.can_go_forward();
        let col = nav_glyph_ink(active);
        let mirrored_x = if (x as isize) <= cx as isize {
            cx + (cx - x)
        } else {
            cx.saturating_sub(x - cx)
        };
        if let Some(c) = std::design::back_button::chevron_ink(mirrored_x, y, cx, cy, col) {
            return c;
        }
        return c;
    }

    // 3. Home / New Tab `+` Button
    if let Some(c) = chrome_control_px(x, y, HOME_TAB_BTN, BTN_R) {
        let cx = HOME_TAB_BTN.x + HOME_TAB_BTN.w / 2;
        let cy = HOME_TAB_BTN.y + HOME_TAB_BTN.h / 2;
        let dx = (x as isize - cx as isize).abs();
        let dy = (y as isize - cy as isize).abs();
        if (dx <= 1 && dy <= 7) || (dy <= 1 && dx <= 7) {
            return THEME.on_surface;
        }
        return c;
    }

    // 4. Bookmark Star Button
    if let Some(c) = chrome_control_px(x, y, ADD_BOOK_BTN, BTN_R) {
        let cx = ADD_BOOK_BTN.x + ADD_BOOK_BTN.w / 2;
        let cy = ADD_BOOK_BTN.y + ADD_BOOK_BTN.h / 2;
        let filled = app.bookmarks.len() + app.bookmarks.unsaved_len() > 0;
        if star_glyph(x, y, cx, cy, filled) {
            return THEME.on_surface;
        }
        return c;
    }

    // 5. Tabs Counter `[N]`
    if let Some(c) = chrome_control_px(x, y, TABS_BTN, BTN_R) {
        let cx = TABS_BTN.x + TABS_BTN.w / 2;
        let cy = TABS_BTN.y + TABS_BTN.h / 2;
        if let Some(px) = tab_badge_pixel(x, y, cx, cy, app.tabs.tab_count()) {
            return px;
        }
        return c;
    }

    THEME.surface
}

// -----------------------------------------------------------------------------
// Pixel Dispatch
// -----------------------------------------------------------------------------
fn get_browser_pixel(x: usize, y: usize, app: &BrowserApp, f: &Frame) -> u32 {
    let base = get_browser_pixel_content(x, y, app, f);
    if app.overlay == SheetOverlay::None {
        return base;
    }
    // WHY the scrim: the sheet's own card drew straight over the content
    // with nothing dimming it, so a dropdown read as a layout fault
    // rather than a layer. The scrim is the content pixel blended toward
    // the background, so the content underneath is still visible but
    // clearly behind.
    if let Some(overlay_px) = render_sheet_overlay(x, y, app) {
        return overlay_px;
    }
    blend_toward(base, THEME.surface, 150)
}

/// The screen without the sheet overlay applied.
///
/// WHY this is separate: an open sheet dims the content behind it, and
/// that dim has to be computed from the content pixel, so the content has
/// to be renderable on its own.
fn get_browser_pixel_content(x: usize, y: usize, app: &BrowserApp, f: &Frame) -> u32 {
    if y < STATUS_H {
        // WHY the band is the SDK's and not the browser's own: the clock's
        // place, its type rung and the three instrument marks have to be the
        // same on every screen or they are not chrome. This app had forty-eight
        // blank rows here, which read as a layout fault rather than as the
        // platform's inset, and left the person using the browser with no way
        // to see the time.
        let band = components::StatusBar::new(W);
        let clock = if f.clock_ok { Some(&f.clock[..]) } else { None };
        return band
            .pixel(x, y, clock, &components::Instruments::unmeasured(), &THEME)
            .unwrap_or(THEME.surface);
    }
    if y < TOOLBAR_Y1 {
        return toolbar_pixel(x, y, app, f);
    }
    if y >= NAV_BAR_Y0 && y < NAV_BAR_Y1 {
        return bottom_bar_pixel(x, y, app);
    }
    if y >= NAV_BAR_Y1 {
        if GESTURE_PILL.contains(x, y) {
            return THEME.border;
        }
        return THEME.surface;
    }

    // Active Sheet Overlay if present
    // Active Sheet Overlay if present.
    //
    // WHY the scrim: the sheet's own card drew straight over the
    // content with nothing dimming it, so a dropdown read as a layout
    // fault rather than a layer. The scrim is the content pixel blended
    // toward the background, so the content underneath is still visible
    // but clearly behind.

    // Soft Keyboard and Accessory Strip overlay when keyboard is open
    if app.keyboard_open() && y >= KB_ACC_Y0 && y < KB_Y1 {
        if y <= KB_ACC_Y1 {
            return render_accessory_bar(x, y, app);
        } else {
            return render_browser_soft_keyboard(x, y, app);
        }
    }

    // Search and Direct Navigation suggestion cards when address is focused
    if app.address_focused && y >= 116 && y < KB_ACC_Y0 {
        if app.address_len > 0 {
            // Search suggestion card: y: 130..198
            let s_rect = Rect::new(24, 130, W - 48, 68);
            if let Some(c) = draw_rounded_rect(x, y, s_rect, CARD.radius, CARD.fill, CARD.border, 1) {
                let icon_box = Rect::new(38, 144, 40, 40);
                if let Some(ic) = draw_rounded_rect(x, y, icon_box, R_SMALL, THEME.surface_container, None, 0) {
                    let dx = (x as isize) - 58;
                    let dy = (y as isize) - 164;
                    if (dx * dx + dy * dy >= 9 && dx * dx + dy * dy <= 25) || (dx >= 3 && dx <= 6 && dy >= 3 && dy <= 6) {
                        return THEME.primary;
                    }
                    return ic;
                }
                // The search suggestion names the host the query goes to.
                //
                // WHY: the card used to read "Search Web" over a request that
                // was sent to one host, so the screen promised a web search
                // the browser does not perform.
                if let Some(px) = text_px(x, y, 92, 144, 1, browser::SEARCH_LABEL, THEME.on_surface_variant, c, false) {
                    return px;
                }
                // The candidate is the string the card offers, and it is
                // fitted to the row with the platform's mark.
                //
                // WHY it was cut to thirty-two bytes and drawn as it was:
                // the candidate stopped at the edge of the row with nothing
                // to say that it continued, so it read as a whole address.
                let q = &f.suggest[..f.suggest_len];
                if let Some(px) = text_px(x, y, 92, 168, 1, q, THEME.primary, c, true) {
                    return px;
                }
                return c;
            }

            // Direct Navigate card: y: 208..276
            let d_rect = Rect::new(24, 208, W - 48, 68);
            if let Some(c) = draw_rounded_rect(x, y, d_rect, CARD.radius, CARD.fill, CARD.border, 1) {
                let icon_box = Rect::new(38, 222, 40, 40);
                if let Some(ic) = draw_rounded_rect(x, y, icon_box, R_SMALL, THEME.surface_container_high, None, 0) {
                    let dx = (x as isize) - 58;
                    let dy = (y as isize) - 242;
                    if dx * dx + dy * dy <= 16 {
                        return THEME.on_surface;
                    }
                    return ic;
                }
                if let Some(px) = text_px(x, y, 92, 222, 1, b"Open Address", THEME.on_surface_variant, c, false) {
                    return px;
                }
                let q = &f.suggest[..f.suggest_len];
                if let Some(px) = text_px(x, y, 92, 246, 1, q, THEME.on_surface, c, true) {
                    return px;
                }
                return c;
            }
            return THEME.surface;
        } else {
            // When address_len == 0, show Quick Destinations
            if y >= 130 && y < 156 {
                if let Some(px) = text_px(x, y, 28, 134, 1, b"Quick links", THEME.on_surface_variant, THEME.surface, true) {
                    return px;
                }
                return THEME.surface;
            }
            // The quick-link row.
            //
            // WHY the platform's pill and not a card the app drew itself: the
            // app had its own rounded rectangle with its own radius, and a
            // control the platform also offers was not the platform's
            // control. The pill is painted by the platform, so the row is
            // the row the shell would draw.
            //
            // WHY the gemini link is gone: the browser reads pages over
            // HTTP and cannot fetch a gemini:// address, so the link led to
            // an error screen. The row offers only what the app can open.
            if y >= QUICK_LINK_Y && y < QUICK_LINK_Y + QUICK_LINK_H {
                if let Some(px) = quick_link_pixel(x, y) {
                    return px;
                }
                return THEME.surface;
            }
            // The addresses the pills stand for.
            if y >= QUICK_LINK_URL_Y && y < QUICK_LINK_URL_Y + QUICK_LINK_URL_H {
                if let Some(px) = quick_link_url_pixel(x, y) {
                    return px;
                }
                return THEME.surface;
            }
            return THEME.surface;
        }
    }

    // Content Viewport (y 116..1350)
    if f.is_home {
        home_page_pixel(x, y, app)
    } else {
        web_page_pixel(x, y, app, f)
    }
}

fn render_browser_ui(surface: u32, app: &BrowserApp) {
    render_browser_band(surface, app, 0, H);
}

/// Paints the quick-link row with the platform's own pill.
///
/// WHY the background is the surface and not transparency: the row sits on
/// the screen's background, and a transparent gap between the pills would
/// show whatever the layer beneath the app holds.
fn paint_quick_link_row() {
    let buffer = unsafe { &mut *core::ptr::addr_of_mut!(QUICK_LINK_ROW) };
    for pixel in buffer.iter_mut() {
        *pixel = THEME.surface;
    }
    let Ok(mut surface) = interos_std::design::draw_target::Surface::new(buffer, QUICK_LINK_W, QUICK_LINK_H)
    else {
        return;
    };
    let pill_w = (QUICK_LINK_W - QUICK_LINK_GAP) / 2;
    for (index, (label, _)) in QUICK_LINKS.iter().enumerate() {
        let rect = Rect::new(index * (pill_w + QUICK_LINK_GAP), 0, pill_w, QUICK_LINK_H);
        components::pill(&mut surface, &THEME, rect, label, 2, false);
    }
}

/// The pixel of the quick-link row at (`x`, `y`).
fn quick_link_pixel(x: usize, y: usize) -> Option<u32> {
    if x < QUICK_LINK_X || x >= QUICK_LINK_X + QUICK_LINK_W {
        return None;
    }
    let row = y.checked_sub(QUICK_LINK_Y)?;
    if row >= QUICK_LINK_H {
        return None;
    }
    let buffer = unsafe { &*core::ptr::addr_of!(QUICK_LINK_ROW) };
    Some(buffer[row * QUICK_LINK_W + (x - QUICK_LINK_X)])
}

/// The pixel of the addresses under the quick-link row at (`x`, `y`).
fn quick_link_url_pixel(x: usize, y: usize) -> Option<u32> {
    let pill_w = (QUICK_LINK_W - QUICK_LINK_GAP) / 2;
    for (index, (_, url)) in QUICK_LINKS.iter().enumerate() {
        let left = QUICK_LINK_X + index * (pill_w + QUICK_LINK_GAP);
        if x < left || x >= left + pill_w {
            continue;
        }
        let width = text_width(url, 1);
        let start = left + (pill_w.saturating_sub(width)) / 2;
        return text_px(
            x,
            y,
            start,
            QUICK_LINK_URL_Y,
            1,
            url,
            THEME.on_surface_variant,
            THEME.surface,
            false,
        );
    }
    None
}

fn render_browser_band(surface: u32, app: &BrowserApp, y0: usize, y1: usize) {
    let frame = Frame::build(app);
    // WHY the row is painted here and not per pixel: the platform's pill
    // paints into a surface, and the renderer answers one pixel at a time.
    if app.address_focused && app.address_len == 0 {
        paint_quick_link_row();
    }
    let mut chunk = [0u32; CHUNK];
    let mut offset = y0 * W;
    let end = y1.min(H) * W;
    while offset < end {
        let count = (end - offset).min(CHUNK);
        let mut x = offset % W;
        let mut y = offset / W;
        for i in 0..count {
            chunk[i] = get_browser_pixel(x, y, app, &frame);
            x += 1;
            if x == W {
                x = 0;
                y += 1;
            }
        }
        let _ = std::surface_write(surface, offset, &chunk[..count]);
        offset += count;
    }
}

// -----------------------------------------------------------------------------
// Touch & Pointer Reactor
// -----------------------------------------------------------------------------
enum Tap<'a> {
    None,
    Redraw,
    Leave(&'a str),
}

fn handle_tap(app: &mut BrowserApp, px: u32, py: u32) -> Tap<'static> {
    let (x, y) = (px as usize, py as usize);

    // Gesture Home Bar (y >= 1410): Exit to Launcher
    if y >= HOME_Y0 {
        return Tap::Leave("[BROWSER APP] Home gesture pressed. Exiting to launcher.");
    }

    // Top Omnibox Close Button
    if CLOSE_BTN.contains(x, y) {
        let _ = std::audio_haptic_click();
        return Tap::Leave("[BROWSER APP] Close control pressed. Exiting to launcher.");
    }

    // Kebab Menu Button
    if KEBAB_BTN.contains(x, y) {
        let _ = std::audio_haptic_click();
        app.overlay = if app.overlay == SheetOverlay::Menu {
            SheetOverlay::None
        } else {
            SheetOverlay::Menu
        };
        return Tap::Redraw;
    }

    // Tabs Top Badge Button
    if TABS_TOP_BTN.contains(x, y) {
        let _ = std::audio_haptic_click();
        app.overlay = if app.overlay == SheetOverlay::TabSwitcher {
            SheetOverlay::None
        } else {
            SheetOverlay::TabSwitcher
        };
        return Tap::Redraw;
    }

    // Reload / Stop Button
    if RELOAD_BTN.contains(x, y) {
        let _ = std::audio_haptic_click();
        if app.is_loading() {
            app.stop();
        } else {
            app.reload();
        }
        return Tap::Redraw;
    }

    // Back control, tested ahead of the capsule so the chip's box is never read
    // as the field that no longer covers it.
    if back_button::contains(x, y) {
        let _ = std::audio_haptic_click();
        if app.tabs.can_go_back() {
            app.go_back();
            return Tap::Redraw;
        }
        return Tap::Leave("[BROWSER APP] Back control pressed with no history. Exiting to launcher.");
    }

    // Omnibox URL Bar Tap
    if PILL.contains(x, y) {
        if app.bm_step == BookmarkStep::Idle && !app.address_focused {
            if app.current_url() == b"about:blank" {
                app.address_len = 0;
            } else {
                let n = app.current_url_len.min(app.address.len());
                app.address[..n].copy_from_slice(&app.current_url[..n]);
                app.address_len = n;
            }
        }
        app.address_focused = true;
        app.select_all = app.address_len > 0;
        app.overlay = SheetOverlay::None;
        let _ = std::audio_haptic_click();
        std::log("[BROWSER APP] Address field focused for keyboard input.").ok();
        return Tap::Redraw;
    }

    // If address bar is focused, handle keyboard, accessory bar, and suggestion taps
    if app.address_focused {
        // 1. Accessory bar chips (y: KB_ACC_Y0..KB_ACC_Y1)
        if y >= KB_ACC_Y0 && y <= KB_ACC_Y1 {
            // Chip 1: "https://" (x 24..134)
            if x >= 24 && x <= 134 {
                let _ = std::audio_haptic_click();
                let prefix = b"https://";
                if !app.address[..app.address_len].starts_with(prefix) {
                    let mut new_addr = [0u8; 128];
                    let p_len = prefix.len();
                    new_addr[..p_len].copy_from_slice(prefix);
                    let copy_n = app.address_len.min(128 - p_len);
                    new_addr[p_len..p_len + copy_n].copy_from_slice(&app.address[..copy_n]);
                    let total = p_len + copy_n;
                    app.address[..total].copy_from_slice(&new_addr[..total]);
                    app.address_len = total;
                    app.select_all = false;
                }
                return Tap::Redraw;
            }
            // Chip 2: ".com" (x 144..224)
            if x >= 144 && x <= 224 {
                let _ = std::audio_haptic_click();
                let suffix = b".com";
                if app.address_len + suffix.len() <= app.address.len() {
                    app.address[app.address_len..app.address_len + suffix.len()].copy_from_slice(suffix);
                    app.address_len += suffix.len();
                    app.select_all = false;
                }
                return Tap::Redraw;
            }
            // Chip 3: ".org" (x 234..314)
            if x >= 234 && x <= 314 {
                let _ = std::audio_haptic_click();
                let suffix = b".org";
                if app.address_len + suffix.len() <= app.address.len() {
                    app.address[app.address_len..app.address_len + suffix.len()].copy_from_slice(suffix);
                    app.address_len += suffix.len();
                    app.select_all = false;
                }
                return Tap::Redraw;
            }
            // Chip 4: "Clear" (x 324..404)
            if x >= 324 && x <= 404 {
                let _ = std::audio_haptic_click();
                app.address_len = 0;
                app.select_all = false;
                return Tap::Redraw;
            }
            // Dismiss down chevron (x 630..694)
            if x >= 630 && x <= 694 {
                let _ = std::audio_haptic_click();
                app.address_focused = false;
                app.select_all = false;
                return Tap::Redraw;
            }
            return Tap::Redraw;
        }

        // 2. Keyboard keys (y: KB_Y0..KB_Y1)
        if y >= KB_Y0 && y < KB_Y1 {
            if let Some(ch) = get_soft_keyboard_char(x, y, app.kb_shift, app.kb_symbols) {
                let _ = std::audio_haptic_click();
                match ch {
                    0xFF => {
                        app.kb_shift = !app.kb_shift;
                    }
                    0xFE => {
                        app.kb_symbols = !app.kb_symbols;
                    }
                    8 => {
                        // Backspace
                        if app.select_all {
                            app.address_len = 0;
                            app.select_all = false;
                        } else if app.address_len > 0 {
                            app.address_len -= 1;
                        }
                    }
                    b'\n' => {
                        // Go / Navigate
                        if app.bm_step != BookmarkStep::Idle {
                            let _ = app.commit_bookmark_step();
                        } else if app.address_len > 0 {
                            app.address_focused = false;
                            app.select_all = false;
                            let mut url = [0u8; 128];
                            let n = app.address_len.min(url.len());
                            url[..n].copy_from_slice(&app.address[..n]);
                            app.navigate(&url[..n]);
                        }
                    }
                    c => {
                        if app.select_all {
                            app.address[0] = c;
                            app.address_len = 1;
                            app.select_all = false;
                        } else if app.address_len < app.address.len() {
                            app.address[app.address_len] = c;
                            app.address_len += 1;
                        }
                    }
                }
                return Tap::Redraw;
            }
            return Tap::Redraw;
        }

        // 3. Typing suggestion cards
        if app.address_len > 0 && x >= 24 && x <= W - 24 {
            if y >= 130 && y <= 198 {
                let _ = std::audio_haptic_click();
                app.address_focused = false;
                app.select_all = false;
                let mut search_url = [0u8; 128];
                let n = browser::search_url(&app.address[..app.address_len], &mut search_url);
                app.navigate(&search_url[..n]);
                return Tap::Redraw;
            }
            if y >= 208 && y <= 276 {
                let _ = std::audio_haptic_click();
                app.address_focused = false;
                app.select_all = false;
                let mut url = [0u8; 128];
                let n = app.address_len.min(url.len());
                url[..n].copy_from_slice(&app.address[..n]);
                app.navigate(&url[..n]);
                return Tap::Redraw;
            }
        } else if app.address_len == 0 && x >= 24 && x <= W - 24 {
            // The quick-link row.
            //
            // WHY the gemini link is gone: the browser reads pages over
            // HTTP and cannot fetch a gemini:// address, so the link led to
            // an error screen.
            if y >= QUICK_LINK_Y && y < QUICK_LINK_Y + QUICK_LINK_H {
                let pill_w = (QUICK_LINK_W - QUICK_LINK_GAP) / 2;
                for (index, (_, url)) in QUICK_LINKS.iter().enumerate() {
                    let left = QUICK_LINK_X + index * (pill_w + QUICK_LINK_GAP);
                    if x >= left && x < left + pill_w {
                        let _ = std::audio_haptic_click();
                        app.address_focused = false;
                        app.select_all = false;
                        app.navigate(url);
                        return Tap::Redraw;
                    }
                }
            }
        }

        // 4. Outside tap: dismiss keyboard
        if !PILL.contains(x, y) {
            app.address_focused = false;
            app.select_all = false;
            // WHY the navigation bar answers anyway: the keyboard docks above the
            // bar, so its five buttons are painted while this field is focused,
            // and swallowing the tap here left a row of controls on screen that
            // answered nothing -- the same defect the bar's own band used to have,
            // one branch later. Below the bar the press path continues to them
            // after the focus is cleared.
            if y < NAV_BAR_Y0 {
                return Tap::Redraw;
            }
        }
    }

    // If a form field is focused, handle keyboard and accessory bar taps
    if app.field_focus != NO_FIELD {
        // 1. Accessory bar (y: KB_ACC_Y0..KB_ACC_Y1)
        if y >= KB_ACC_Y0 && y <= KB_ACC_Y1 {
            // Clear button (x: 24..104)
            if x >= 24 && x <= 104 {
                let _ = std::audio_haptic_click();
                app.field_clear();
                return Tap::Redraw;
            }
            // Done button (x: 600..694)
            if x >= 600 && x <= 694 {
                app.blur_field();
                return Tap::Redraw;
            }
            return Tap::Redraw;
        }

        // 2. Keyboard keys (y: KB_Y0..KB_Y1)
        if y >= KB_Y0 && y < KB_Y1 {
            if let Some(ch) = get_soft_keyboard_char(x, y, app.kb_shift, app.kb_symbols) {
                let _ = std::audio_haptic_click();
                match ch {
                    0xFF => {
                        app.kb_shift = !app.kb_shift;
                    }
                    0xFE => {
                        app.kb_symbols = !app.kb_symbols;
                    }
                    8 => {
                        app.field_backspace();
                    }
                    b'\n' => {
                        let out = unsafe { &*core::ptr::addr_of!(PAGE_LAYOUT) };
                        let idx = app.field_focus as usize;
                        if idx < out.field_count && out.fields[idx].kind == layout::FieldKind::Textarea {
                            app.field_push(b'\n');
                        } else {
                            app.submit_form(idx);
                        }
                    }
                    c => {
                        app.field_push(c);
                    }
                }
                return Tap::Redraw;
            }
            return Tap::Redraw;
        }
    }

    // Bottom Navigation Bar Buttons
    if BACK_BTN.contains(x, y) {
        let _ = std::audio_haptic_click();
        app.go_back();
        return Tap::Redraw;
    }
    if FWD_BTN.contains(x, y) {
        let _ = std::audio_haptic_click();
        app.go_forward();
        return Tap::Redraw;
    }
    if HOME_TAB_BTN.contains(x, y) {
        let _ = std::audio_haptic_click();
        let start = b"about:blank";
        app.current_url[..start.len()].copy_from_slice(start);
        app.current_url_len = start.len();
        app.status = PageStatus::Blank;
        app.address_focused = false;
        app.overlay = SheetOverlay::None;
        std::log("[BROWSER APP] Returned to Sovereign Browser New Tab start page.").ok();
        return Tap::Redraw;
    }
    if ADD_BOOK_BTN.contains(x, y) {
        let _ = std::audio_haptic_click();
        app.overlay = if app.overlay == SheetOverlay::Bookmarks {
            SheetOverlay::None
        } else {
            SheetOverlay::Bookmarks
        };
        return Tap::Redraw;
    }
    if TABS_BTN.contains(x, y) {
        let _ = std::audio_haptic_click();
        app.overlay = if app.overlay == SheetOverlay::TabSwitcher {
            SheetOverlay::None
        } else {
            SheetOverlay::TabSwitcher
        };
        return Tap::Redraw;
    }

    // Handle Active Overlays Interactions
    match app.overlay {
        SheetOverlay::Menu => {
            if MENU_CARD.contains(x, y) {
                // WHY the row is counted from the painted row origin: the menu's
                // items are MENU_ROW_H apart and the first is drawn at MENU_ROW_Y0,
                // and this used to divide by the same height from the card's top
                // edge at 116. The two origins are fourteen pixels apart, so every
                // row answered the item above the one under the finger.
                let row = (y.saturating_sub(MENU_ROW_Y0)) / MENU_ROW_H;
                // WHY the card's head strip answers no item: the first row's ink
                // starts fourteen rows below the card's own top edge, and a tap on
                // that empty padding saturated to row 0 -- it opened a new tab from
                // pixels that draw nothing and answer nothing.
                if y < MENU_ROW_Y0 {
                    return Tap::Redraw;
                }
                let _ = std::audio_haptic_click();
                match row {
                    0 => {
                        // New Tab
                        app.tabs.new_tab(b"about:blank");
                        let start = b"about:blank";
                        app.current_url[..start.len()].copy_from_slice(start);
                        app.current_url_len = start.len();
                        app.status = PageStatus::Blank;
                    }
                    1 => app.overlay = SheetOverlay::Bookmarks,
                    2 => app.overlay = SheetOverlay::History,
                    // WHY there is a downloads entry: the browser saved
                    // pages into the shared storage area and never said
                    // which, so a person could not open what they had
                    // saved.
                    3 => {
                        app.refresh_downloads();
                        app.overlay = SheetOverlay::Downloads;
                    }
                    4 => app.overlay = SheetOverlay::Settings,
                    5 => app.overlay = SheetOverlay::Tools,
                    6 => app.reload(),
                    // WHY the page is saved into the shared storage area: that is
                    // the one place every app may look, so a saved page is a
                    // document the platform's index holds and the file manager
                    // lists beside the photographs the camera wrote.
                    7 => app.save_page_to_storage(),
                    _ => app.overlay = SheetOverlay::None,
                }
                if row != 1 && row != 2 && row != 3 && row != 4 && row != 5 {
                    app.overlay = SheetOverlay::None;
                }
                // WHY the address field is released: a sheet opened from the
                // toolbar has to dismiss the soft keyboard, or the two
                // layers are drawn over each other and neither can be read.
                app.address_focused = false;
                app.select_all = false;
                return Tap::Redraw;
            }
            app.overlay = SheetOverlay::None;
            return Tap::Redraw;
        }
        SheetOverlay::TabSwitcher => {
            // New Tab button in the header.
            if NEW_TAB_BTN.contains(x, y) {
                let _ = std::audio_haptic_click();
                app.tabs.new_tab(b"about:blank");
                let start = b"about:blank";
                app.current_url[..start.len()].copy_from_slice(start);
                app.current_url_len = start.len();
                app.status = PageStatus::Blank;
                app.overlay = SheetOverlay::None;
                return Tap::Redraw;
            }

            // Tab cards interaction (4 slots)
            let card_h = 110;
            let card_gap = 16;
            for i in 0..MAX_TABS {
                let card_y = 180 + i * (card_h + card_gap);
                if y >= card_y && y < card_y + card_h && x >= 24 && x <= W - 24 {
                    let _ = std::audio_haptic_click();
                    // Close target tapped?
                    if tab_close_rect(card_y).contains(x, y) {
                        app.tabs.close_tab(i);
                        let cur = app.tabs.active_tab().url();
                        let n = cur.len().min(app.current_url.len());
                        app.current_url[..n].copy_from_slice(&cur[..n]);
                        app.current_url_len = n;
                    } else if app.tabs.tab(i).is_some() {
                        app.tabs.switch_tab(i);
                        let cur = app.tabs.active_tab().url();
                        let n = cur.len().min(app.current_url.len());
                        app.current_url[..n].copy_from_slice(&cur[..n]);
                        app.current_url_len = n;
                        app.overlay = SheetOverlay::None;
                    } else {
                        // Empty slot tapped: create new tab
                        app.tabs.new_tab(b"about:blank");
                        let start = b"about:blank";
                        app.current_url[..start.len()].copy_from_slice(start);
                        app.current_url_len = start.len();
                        app.status = PageStatus::Blank;
                        app.overlay = SheetOverlay::None;
                    }
                    return Tap::Redraw;
                }
            }
        }
        SheetOverlay::Bookmarks => {
            if x >= 640 && y >= 120 && y <= 165 {
                app.overlay = SheetOverlay::None;
                app.filter_clear();
                return Tap::Redraw;
            }
            // The filter's clear control.
            if y >= FILTER_Y && y < FILTER_Y + FILTER_H && x >= W - 96 && x <= W - 24 {
                let _ = std::audio_haptic_click();
                app.filter_clear();
                return Tap::Redraw;
            }
            let filter = app.filter_bytes();
            let mut shown = 0usize;
            for i in 0..app.bookmarks.len() {
                if let Some(rec) = app.bookmarks.record(i) {
                    if !browser::contains_ignore_case(rec.title_bytes(), filter)
                        && !browser::contains_ignore_case(rec.url_bytes(), filter)
                    {
                        continue;
                    }
                    let r_y = LIST_Y + shown * BOOKMARK_ROW_PITCH;
                    if y >= r_y && y < r_y + BOOKMARK_CARD_H && x >= 24 && x < W - 24 {
                        let _ = std::audio_haptic_click();
                        let mut url = [0u8; 64];
                        let u = rec.url_bytes();
                        let n = u.len().min(url.len());
                        url[..n].copy_from_slice(&u[..n]);
                        app.overlay = SheetOverlay::None;
                        app.filter_clear();
                        app.navigate(&url[..n]);
                        return Tap::Redraw;
                    }
                    shown += 1;
                }
            }
        }
        SheetOverlay::History => {
            if x >= 640 && y >= 120 && y <= 165 {
                app.overlay = SheetOverlay::None;
                app.filter_clear();
                return Tap::Redraw;
            }
            // The filter's clear control.
            if y >= FILTER_Y && y < FILTER_Y + FILTER_H && x >= W - 96 && x <= W - 24 {
                let _ = std::audio_haptic_click();
                app.filter_clear();
                return Tap::Redraw;
            }
            // WHY the row's box is the painter's own: the card is drawn inside
            // the pitch, so a tap just past its edge would open a row whose
            // ink is not under the finger.
            for row in 0..HISTORY_ROWS {
                let Some(visit) = history_row(app, row) else {
                    break;
                };
                let r_y = LIST_Y + row * HISTORY_ROW_PITCH;
                if y >= r_y
                    && y < r_y + HISTORY_CARD_H
                    && x >= HISTORY_CARD_X
                    && x < HISTORY_CARD_X + HISTORY_CARD_W
                {
                    let _ = std::audio_haptic_click();
                    // WHY the address is copied before the navigation: the row
                    // borrows the log that this navigation is about to append
                    // to, and a page cannot be opened from an address held in
                    // the record being written.
                    let mut url = [0u8; HISTORY_ENTRY_BYTES];
                    let n = visit.url.len().min(url.len());
                    url[..n].copy_from_slice(&visit.url[..n]);
                    app.overlay = SheetOverlay::None;
                    app.filter_clear();
                    app.navigate(&url[..n]);
                    return Tap::Redraw;
                }
            }
        }
        SheetOverlay::Downloads => {
            if x >= 640 && y >= 120 && y <= 165 {
                app.overlay = SheetOverlay::None;
                return Tap::Redraw;
            }
            // The saved pages and downloaded files.
            //
            // WHY tapping one opens it: a download the person cannot open
            // is a download they did not keep. A page is the browser's own
            // document and navigates; a file belongs to the device and is
            // handed to whichever app registered for its type.
            let row_h = 72;
            for i in 0..app.download_count {
                let r_y = 180 + i * row_h;
                if y >= r_y && y < r_y + row_h && x >= 24 && x <= W - 24 {
                    let _ = std::audio_haptic_click();
                    let entry = app.downloads[i];
                    if entry.is_page {
                        let mut url = [0u8; 128];
                        url[..7].copy_from_slice(b"file://");
                        let n = 7 + entry.path_len.min(url.len() - 7);
                        url[7..n].copy_from_slice(&entry.path[..n - 7]);
                        app.overlay = SheetOverlay::None;
                        app.navigate(&url[..n]);
                    } else if entry.mime_len > 0 {
                        let Ok(path) = core::str::from_utf8(&entry.path[..entry.path_len]) else {
                            app.downloads_error = true;
                            return Tap::Redraw;
                        };
                        let Ok(mime) = core::str::from_utf8(&entry.mime[..entry.mime_len]) else {
                            app.downloads_error = true;
                            return Tap::Redraw;
                        };
                        match std::content::intent_dispatch(std::content::IntentKind::Mime, mime, path) {
                            Ok(_) => {
                                app.downloads_error = false;
                                app.overlay = SheetOverlay::None;
                            }
                            Err(_) => {
                                app.downloads_error = true;
                            }
                        }
                    } else {
                        app.downloads_error = true;
                    }
                    return Tap::Redraw;
                }
            }
        }
        SheetOverlay::Tools => {
            if x >= 640 && y >= 120 && y <= 165 {
                app.overlay = SheetOverlay::None;
                return Tap::Redraw;
            }
            for index in 0..4usize {
                let top = 220 + index * 116;
                if y >= top && y < top + 100 && x >= 24 && x <= W - 24 {
                    let _ = std::audio_haptic_click();
                    match index {
                        0 => {
                            // WHY the picker is read when it opens: the
                            // documents on the device are the index's, and
                            // one written since the last look belongs here.
                            app.upload_state = TransferState::Idle;
                            app.upload_sent = 0;
                            app.upload_confirmed = 0;
                            app.upload_error = 0;
                            app.upload_name_len = 0;
                            app.refresh_uploads();
                            app.overlay = SheetOverlay::UploadPicker;
                        }
                        1 => {
                            // A previous run's numbers are not this run's,
                            // so the sheet opens empty rather than showing
                            // measurements from another time.
                            app.speed = SpeedResults::EMPTY;
                            app.speed_running = true;
                            app.overlay = SheetOverlay::SpeedTest;
                            app.pending = PendingTask::SpeedTest;
                        }
                        2 => {
                            app.lab = [LabResult::EMPTY; LAB_URLS.len()];
                            app.lab_running = true;
                            app.overlay = SheetOverlay::CompatibilityLab;
                            app.pending = PendingTask::Lab;
                        }
                        _ => {
                            app.overlay = SheetOverlay::TestFiles;
                        }
                    }
                    return Tap::Redraw;
                }
            }
        }
        SheetOverlay::Settings => {
            if x >= 640 && y >= 120 && y <= 165 {
                app.overlay = SheetOverlay::None;
                return Tap::Redraw;
            }
            // Row 3: Open system settings (top = 220 + 3*116 = 568 .. + 100)
            let top3 = 220 + 3 * 116;
            if y >= top3 && y < top3 + 100 && x >= 24 && x <= W - 24 {
                let _ = std::audio_haptic_click();
                std::log("[BROWSER APP] Launching system settings...").ok();
                let _ = std::app_launch("settings");
                app.overlay = SheetOverlay::None;
                return Tap::Redraw;
            }
            return Tap::Redraw;
        }
        SheetOverlay::TestFiles => {
            if x >= 640 && y >= 120 && y <= 165 {
                app.overlay = SheetOverlay::None;
                return Tap::Redraw;
            }
            let row_h = 84;
            for (index, (name, _)) in TEST_FIXTURES.iter().enumerate() {
                let r_y = 220 + index * row_h;
                if y >= r_y && y < r_y + row_h - 10 && x >= 24 && x <= W - 24 {
                    let _ = std::audio_haptic_click();
                    let mut url = [0u8; 96];
                    let mut n = append_text(&mut url, 0, FIXTURE_URL_PREFIX);
                    n = append_text(&mut url, n, name);
                    app.overlay = SheetOverlay::None;
                    app.navigate(&url[..n]);
                    return Tap::Redraw;
                }
            }
            // WHY the sheet consumes the rest: a tap that misses a row must not
            // reach the page underneath the sheet.
            return Tap::Redraw;
        }
        SheetOverlay::UploadPicker => {
            if x >= 640 && y >= 120 && y <= 165 {
                app.overlay = SheetOverlay::None;
                return Tap::Redraw;
            }
            let row_h = 84;
            for i in 0..app.upload_count {
                let r_y = 220 + i * row_h;
                if y >= r_y && y < r_y + row_h - 10 && x >= 24 && x <= W - 24 {
                    let _ = std::audio_haptic_click();
                    // WHY the work is deferred by a pass: the selected row is
                    // drawn as running once the transfer has actually started,
                    // so the screen never claims a transfer that has not begun.
                    app.upload_state = TransferState::Running;
                    app.pending = PendingTask::Upload(i);
                    return Tap::Redraw;
                }
            }
            // WHY the sheet consumes the rest: a tap that misses a row must not
            // reach the page underneath the sheet.
            return Tap::Redraw;
        }
        SheetOverlay::SpeedTest => {
            if x >= 640 && y >= 120 && y <= 165 {
                app.overlay = SheetOverlay::None;
            }
            return Tap::Redraw;
        }
        SheetOverlay::CompatibilityLab => {
            if x >= 640 && y >= 120 && y <= 165 {
                app.overlay = SheetOverlay::None;
            }
            return Tap::Redraw;
        }
        SheetOverlay::None => {}
    }

    // Start Page Favorites / Quick Access Tiles Tap (y: 278..342)
    if app.status == PageStatus::Blank && y >= 278 && y <= 342 {
        // Col 0: InterOS (x: 66..130)
        if x >= 66 && x <= 130 {
            let _ = std::audio_haptic_click();
            app.navigate(b"https://interos.tech");
            return Tap::Redraw;
        }
        // Col 1: YouTube (x: 234..298)
        if x >= 234 && x <= 298 {
            let _ = std::audio_haptic_click();
            app.navigate(b"https://m.youtube.com");
            return Tap::Redraw;
        }
        // Col 2: GitHub (x: 402..466)
        if x >= 402 && x <= 466 {
            let _ = std::audio_haptic_click();
            app.navigate(b"https://github.com");
            return Tap::Redraw;
        }
        // Col 3: Wikipedia (x: 570..634)
        if x >= 570 && x <= 634 {
            let _ = std::audio_haptic_click();
            app.navigate(b"https://wikipedia.org");
            return Tap::Redraw;
        }
    }

    // Start Page "Your pages" Cards Tap (y: 422..1340)
    if app.status == PageStatus::Blank && x >= 24 && x < W - 24 {
        let card_h = 100;
        let card_gap = 12;
        for i in 0..4 {
            let card_y = 422 + i * (card_h + card_gap);
            if y >= card_y && y < card_y + card_h {
                if let Some(row) = home_row(app, i) {
                    let _ = std::audio_haptic_click();
                    // WHY the record's address and not the card's text: the
                    // line shown is shortened to fit the card, and opening a
                    // truncation of an address is a different navigation from
                    // the one the card names.
                    let mut url = [0u8; HISTORY_ENTRY_BYTES];
                    let n = row.target.len().min(url.len());
                    url[..n].copy_from_slice(&row.target[..n]);
                    app.address_focused = false;
                    app.select_all = false;
                    app.address_len = 0;
                    app.navigate(&url[..n]);
                    return Tap::Redraw;
                }
            }
        }
    }

    // HTML Viewport Interactions
    if app.status == PageStatus::Loaded && app.html_view && y >= READER_TOP_Y && y < NAV_BAR_Y0 {
        let doc_x = (x as isize) - (browser::CONTENT_PADDING as isize);
        let doc_y = (y as isize) - (READER_TOP_Y as isize) + (app.scroll.offset as isize);

        if doc_x >= 0 && doc_x < browser::CONTENT_WIDTH as isize && doc_y >= 0 {
            let out = unsafe { &*core::ptr::addr_of!(PAGE_LAYOUT) };

            // 1. Hit test fields first
            if let Some(index) = out.field_at(doc_x as usize, doc_y as usize) {
                let field = out.fields[index];
                {
                    let dom_mut = unsafe { &mut *core::ptr::addr_of_mut!(PAGE_DOM) };
                    if let Some(onclick) = dom_mut.attr(field.node, b"onclick") {
                        let mut buf = [0u8; 512];
                        let n = onclick.len().min(buf.len());
                        buf[..n].copy_from_slice(&onclick[..n]);
                        BrowserApp::run_script(&buf[..n], dom_mut, "field onclick");
                    }
                }
                match field.kind {
                    layout::FieldKind::Select => {
                        app.cycle_select(index);
                    }
                    layout::FieldKind::Button => {
                        app.submit_form(index);
                    }
                    layout::FieldKind::Textarea => {
                        app.focus_field(index);
                    }
                    layout::FieldKind::Input => {
                        match field.input {
                            layout::InputType::Checkbox | layout::InputType::Radio => {
                                app.toggle_check(index);
                            }
                            layout::InputType::Submit | layout::InputType::Reset => {
                                app.submit_form(index);
                            }
                            _ => {
                                app.focus_field(index);
                            }
                        }
                    }
                    _ => {}
                }
                return Tap::Redraw;
            }

            // 2. Hit test links
            if let Some(link) = out.link_at(doc_x as usize, doc_y as usize) {
                let dom_mut = unsafe { &mut *core::ptr::addr_of_mut!(PAGE_DOM) };
                for nid in 0..dom_mut.node_count() as layout::NodeId {
                    if dom_mut.node(nid).link == link {
                        if let Some(onclick) = dom_mut.attr(nid, b"onclick") {
                            let mut buf = [0u8; 512];
                            let n = onclick.len().min(buf.len());
                            buf[..n].copy_from_slice(&onclick[..n]);
                            BrowserApp::run_script(&buf[..n], dom_mut, "link onclick");
                        }
                        break;
                    }
                }
                let dom = unsafe { &*core::ptr::addr_of!(PAGE_DOM) };
                if let Some(href) = dom.link_href(link) {
                    app.navigate_href(href);
                    return Tap::Redraw;
                }
            }

            // 3. Background tapped in document area: blur field
            app.blur_field();
        }

        // Tap on content area scrolls if not a link or field
        if y > 1000 {
            app.scroll.offset = (app.scroll.offset + 60).min(app.scroll.max_offset());
            return Tap::Redraw;
        } else if y < 400 && app.scroll.offset > 0 {
            app.scroll.offset = app.scroll.offset.saturating_sub(60);
            return Tap::Redraw;
        }
        return Tap::Redraw;
    }

    // Content Viewport Links Tap
    if app.status == PageStatus::Loaded && y >= 120 && y < 1340 {
        let content_start_y = READER_TOP_Y;
        let base_y = (content_start_y as isize) - (app.scroll.offset as isize);
        let mut cur_y = base_y;

        let mut clicked_target: Option<([u8; 64], usize)> = None;
        for i in 0..app.doc_line_count {
            let line = doc_line(i);
            // WHY the height comes from line_box_height: the renderer and
            // this handler walk the document using line_box_height, including margins
            // and image heights, so link tap targets perfectly match where links are drawn.
            let line_h = browser::line_box_height(line);
            let band_y = cur_y
                + if line.block_start {
                    line.style.margin_top as isize
                } else {
                    0
                };

            if line.kind == LineKind::GemtextLink && line.link_target_len > 0 {
                let g_y = band_y + 4;
                let g_h = 56;
                if y as isize >= g_y && (y as isize) < g_y + g_h as isize && x >= 24 && x <= W - 24 {
                    let mut buf = [0u8; 64];
                    let u = line.link_target_len.min(line.link_target.len()).min(64);
                    buf[..u].copy_from_slice(&line.link_target[..u]);
                    clicked_target = Some((buf, u));
                    break;
                }
            }
            cur_y += line_h as isize;
        }

        if let Some((target_buf, target_len)) = clicked_target {
            let _ = std::audio_haptic_click();
            let raw_target = &target_buf[..target_len];
            let target_uri = browser::parse_uri(raw_target);
            if target_uri.scheme != browser::UriScheme::Unknown {
                app.navigate(raw_target);
            } else {
                let mut resolved = [0u8; 128];
                let cur = &app.current_url[..app.current_url_len.min(app.current_url.len())];
                if let Ok(rn) = browser::images::resolve_subresource(cur, raw_target, &mut resolved) {
                    app.navigate(&resolved[..rn]);
                } else {
                    app.navigate(raw_target);
                }
            }
            return Tap::Redraw;
        }

        // Tap on content area scrolls if not a link.
        //
        // WHY the scroll state does the clamping: the offset used to be a
        // bare number that grew without bound, so a long page could be
        // dragged past its end and the reader would show empty space below
        // the last line.
        if y > 1000 {
            app.scroll.offset = (app.scroll.offset + 60).min(app.scroll.max_offset());
            return Tap::Redraw;
        } else if y < 400 && app.scroll.offset > 0 {
            app.scroll.offset = app.scroll.offset.saturating_sub(60);
            return Tap::Redraw;
        }
    }

    // A downloaded file's Open action.
    //
    // WHY the platform dispatches it: the registry is built from the
    // installed packages' manifests, so the app that opens the file is a
    // property of the device. The screen reports what the platform said.
    if app.status == PageStatus::Downloaded {
        let frame = Frame::build(app);
        if open_action_rect(&frame).contains(x, y) {
            let _ = std::audio_haptic_click();
            app.open_last_download();
            return Tap::Redraw;
        }
        return Tap::None;
    }

    // Download confirmation / refusal actions
    if app.status == PageStatus::DownloadPrompt {
        let frame = Frame::build(app);
        if download_confirm_rect(&frame).contains(x, y) {
            let _ = std::audio_haptic_click();
            app.accept_pending_download();
            return Tap::Redraw;
        }
        if download_cancel_rect(&frame).contains(x, y) {
            let _ = std::audio_haptic_click();
            app.refuse_pending_download();
            return Tap::Redraw;
        }
        return Tap::None;
    }

    if app.status == PageStatus::DownloadRefused {
        let frame = Frame::build(app);
        if dismiss_action_rect(&frame).contains(x, y) {
            let _ = std::audio_haptic_click();
            app.status = PageStatus::Blank;
            return Tap::Redraw;
        }
        return Tap::None;
    }

    // Offline / Error screen Reload Button tap
    if app.status.is_error()
        && ((y >= 630 && y <= 720) || (y >= 920 && y <= 968))
        && x >= (W / 2 - 90)
        && x <= (W / 2 + 90)
    {
        let _ = std::audio_haptic_click();
        app.reload();
        return Tap::Redraw;
    }

    Tap::None
}

// -----------------------------------------------------------------------------
// Native Keyboard Input Reactor
// -----------------------------------------------------------------------------
#[derive(Clone, Copy, PartialEq, Eq)]
enum Dirty {
    None,
    Toolbar,
    All,
}

fn handle_browser_key(app: &mut BrowserApp, key: std::InputKeyEvent) -> Dirty {
    use std::input_key_code;
    if !key.pressed {
        return Dirty::None;
    }
    // A tools/settings sheet is dismissed the same way every other sheet is.
    if !app.address_focused
        && key.code == input_key_code::ESCAPE
        && matches!(
            app.overlay,
            SheetOverlay::Settings
                | SheetOverlay::Tools
                | SheetOverlay::UploadPicker
                | SheetOverlay::SpeedTest
                | SheetOverlay::CompatibilityLab
                | SheetOverlay::TestFiles
        )
    {
        app.overlay = SheetOverlay::None;
        return Dirty::All;
    }
    // WHY the prompt does not own the keys while the address field is
    // focused: Escape closes the on-screen keyboard, and the prompt used to
    // read that same Escape as a refusal, silently discarding a download the
    // person had not answered yet.
    if app.status == PageStatus::DownloadPrompt && !app.address_focused {
        match key.code {
            input_key_code::ENTER => {
                app.accept_pending_download();
                return Dirty::All;
            }
            input_key_code::ESCAPE => {
                app.refuse_pending_download();
                return Dirty::All;
            }
            _ => return Dirty::None,
        }
    }
    // WHY the open list takes the keys first: a panel that is open and
    // filtered is what the person is looking at, and the address field is
    // not on the screen. A list that could not be searched was the fault
    // this closes.
    if !app.address_focused && app.filterable() {
        match key.code {
            input_key_code::ESCAPE => {
                app.overlay = SheetOverlay::None;
                app.filter_clear();
                return Dirty::All;
            }
            input_key_code::BACKSPACE => {
                app.filter_backspace();
                return Dirty::All;
            }
            code if (32..=126).contains(&code) => {
                app.filter_push(code as u8);
                return Dirty::All;
            }
            _ => return Dirty::None,
        }
    }
    if app.field_focus != NO_FIELD {
        match key.code {
            input_key_code::ENTER => {
                let out = unsafe { &*core::ptr::addr_of!(PAGE_LAYOUT) };
                let idx = app.field_focus as usize;
                if idx < out.field_count && out.fields[idx].kind == layout::FieldKind::Textarea {
                    app.field_push(b'\n');
                } else {
                    app.submit_form(idx);
                }
                return Dirty::All;
            }
            input_key_code::ESCAPE => {
                app.blur_field();
                return Dirty::All;
            }
            input_key_code::BACKSPACE => {
                app.field_backspace();
                return Dirty::All;
            }
            code if (32..=126).contains(&code) => {
                app.field_push(code as u8);
                return Dirty::All;
            }
            _ => return Dirty::None,
        }
    }

    if !app.address_focused {
        if key.code == input_key_code::ESCAPE {
            if app.is_loading() {
                app.stop();
                return Dirty::All;
            }
            if app.overlay != SheetOverlay::None {
                app.overlay = SheetOverlay::None;
                return Dirty::All;
            }
        }
        if app.overlay == SheetOverlay::None {
            if key.code == input_key_code::ARROW_LEFT {
                app.go_back();
                return Dirty::All;
            }
            if key.code == input_key_code::ARROW_RIGHT {
                app.go_forward();
                return Dirty::All;
            }
        }
        return Dirty::None;
    }
    match key.code {
        input_key_code::ENTER => {
            app.address_focused = false;
            app.select_all = false;
            if app.bm_step == BookmarkStep::Idle {
                if app.address_len > 0 {
                    let mut target = [0u8; 256];
                    let n = app.address_len.min(target.len());
                    target[..n].copy_from_slice(&app.address[..n]);
                    app.navigate(&target[..n]);
                }
            } else {
                let _ = app.commit_bookmark_step();
            }
            Dirty::All
        }
        input_key_code::ESCAPE => {
            if app.bm_step != BookmarkStep::Idle {
                app.cancel_bookmark_step();
                std::log("[BROWSER APP] Add-bookmark cancelled.").ok();
                return Dirty::All;
            }
            app.address_focused = false;
            app.select_all = false;
            Dirty::Toolbar
        }
        input_key_code::BACKSPACE => {
            if app.select_all {
                app.address_len = 0;
                app.select_all = false;
            } else {
                app.address_len = app.address_len.saturating_sub(1);
            }
            Dirty::Toolbar
        }
        code if (32..=126).contains(&code) && app.address_len < app.address.len() => {
            if app.select_all {
                app.address_len = 0;
                app.select_all = false;
            }
            app.address[app.address_len] = code as u8;
            app.address_len += 1;
            let status_moved = app.status != PageStatus::Blank;
            app.status = PageStatus::Blank;
            if status_moved {
                Dirty::All
            } else {
                Dirty::Toolbar
            }
        }
        _ => Dirty::None,
    }
}

fn destroy_and_exit(surface: u32) -> ! {
    if surface > 0 {
        // WHY the slot is resolved by type here too: a slot number is a
        // position in a table, and the position depends on the order the
        // task's capabilities were inserted in.
    }
    std::exit_with_code(0);
}

// -----------------------------------------------------------------------------
// Application Entry Point (_start)
// -----------------------------------------------------------------------------
#[no_mangle]
pub extern "C" fn _start() -> ! {
    std::log("[BROWSER APP (Ring 3)] Sovereign Decentralized Web Client online.",
    )
    .ok();

    let mut browser = BrowserApp::new();

    match browser.bookmarks.state() {
        BookmarkStoreState::Loaded => {
            std::log("[BROWSER APP] Bookmarks read back out of the settings store.").ok();
        }
        BookmarkStoreState::Empty => {
            std::log("[BROWSER APP] No saved bookmarks: the settings store holds no browser bookmarks key.").ok();
        }
        BookmarkStoreState::ReadRefused => {
            std::log("[BROWSER APP] Bookmark read REFUSED by the kernel: this task holds no filesystem capability, so no stored bookmarks were loaded and none will be shown as saved.").ok();
        }
        BookmarkStoreState::Invalid => {
            std::log("[BROWSER APP] Stored bookmark bytes failed validation; no bookmarks were loaded from them.").ok();
        }
        BookmarkStoreState::Untouched
        | BookmarkStoreState::Written
        | BookmarkStoreState::WriteRefused
        | BookmarkStoreState::Full => {
            std::log("[BROWSER APP] Bookmark store reported a state it cannot be in before any write; no bookmarks are shown as saved.").ok();
        }
    }

    std::log("[BROWSER APP] History and bookmarks are real, user-created records. Zero heap allocations in event loop.",
    )
    .ok();

    // Allocate DRM/KMS Client Surface (z=200)
    let surface = match std::surface_create(W as u32, H as u32, 200) {
        Ok(id) => {
            std::log("[BROWSER APP] DRM/KMS Client Surface allocated (z=200).").ok();
            id
        }
        Err(_) => 0,
    };

    if surface > 0 {
        render_browser_ui(surface, &browser);
        let _ = std::surface_flip();
        std::log("[BROWSER APP] Browser canvas presented to scanout buffer.").ok();
    }

    let interest_mask = std::EVENT_TOUCH_INPUT
        | std::EVENT_IPC_MSG
        | std::EVENT_NETWORK_PACKET
        | std::EVENT_TIMER_TICK;

    loop {
        // WHY the work runs after a frame has been presented: the tap
        // schedules it and returns, the touch branch draws the sheet's
        // running state, and only the next pass performs the transfer. A
        // screen that drew only the result could not say the work is
        // happening, and a screen that drew only "running" would be a
        // claim the transfer had actually begun.
        if browser.pending != PendingTask::None {
            let task = browser.pending;
            browser.pending = PendingTask::None;
            match task {
                PendingTask::Upload(index) => browser.perform_upload(index),
                PendingTask::SpeedTest => browser.perform_speed_test(),
                PendingTask::Lab => browser.perform_lab(),
                PendingTask::None => {}
            }
            if surface > 0 {
                render_browser_ui(surface, &browser);
                let _ = std::surface_flip();
            }
        }
        match std::event_poll(interest_mask, 150) {
            Ok(events) => {
                if events & std::EVENT_TOUCH_INPUT != 0 {
                    let mut input = std::InputEvent {
                        kind: 0,
                        x: 0,
                        y: 0,
                        dx: 0,
                        dy: 0,
                        scale_milli: 1000,
                    };
                    let mut dirty = Dirty::None;
                    while std::input_event_poll(&mut input).is_ok() {
                        if let Some(key) = input.key_event() {
                            let changed = handle_browser_key(&mut browser, key);
                            if changed != Dirty::None {
                                dirty = Dirty::All;
                            }
                            continue;
                        }
                        if input.kind == 4 || input.kind == 6 || input.kind == 7 || input.kind == 8 || input.kind == 10 {
                            std::log("[BROWSER APP] Navigation gesture recognized. Exiting to launcher.").ok();
                            destroy_and_exit(surface);
                        }
                        // Touch drag / vertical scroll
                        //
                        // WHY the platform's scroll model handles the drag: a
                        // bare offset grew without bound, so a long page
                        // could be dragged past its last line.
                        //
                        // WHY the delta goes in as the kernel measured it: the
                        // reader draws a document row at `screen = top + doc_y -
                        // offset`, so the page follows the finger only when a
                        // finger moving up the screen deepens the offset -- which
                        // is what `apply_drag` does with the negative `dy` the
                        // pan gesture reports. Negating it made the page travel
                        // against the finger, and against the top stop it simply
                        // refused to move.
                        if input.kind == 2 && browser.status == PageStatus::Loaded {
                            if input.dy != 0 {
                                browser.scroll.apply_drag(input.dy);
                                dirty = Dirty::All;
                            }
                        }
                        if input.kind == 1 {
                            match handle_tap(&mut browser, input.x, input.y) {
                                Tap::Leave(msg) => {
                                    std::log(msg).ok();
                                    destroy_and_exit(surface);
                                }
                                Tap::Redraw => dirty = Dirty::All,
                                Tap::None => {}
                            }
                        }
                    }
                    if dirty != Dirty::None && surface > 0 {
                        match dirty {
                            Dirty::None => {}
                            Dirty::Toolbar => {
                                render_browser_band(surface, &browser, STATUS_H, TOOLBAR_Y1);
                            }
                            Dirty::All => render_browser_ui(surface, &browser),
                        }
                        let _ = std::surface_flip();
                    }
                }
                if events & std::EVENT_IPC_MSG != 0 {
                    std::log("[BROWSER APP] IPC message event raised; no navigation intent was parsed from it.").ok();
                }
                if events & std::EVENT_NETWORK_PACKET != 0 {
                    match browser.active_socket_id {
                        Some(sock_id) => {
                            std::log("[BROWSER APP] Network packet arrived via e1000 socket.").ok();
                            let mut rx_buf = [0u8; 2048];
                            if let Ok(n) = std::net_recvfrom(sock_id, &mut rx_buf) {
                                if n > 0 {
                                    if let Some(http_resp) = browser::parse_http_response(&rx_buf[..n]) {
                                        let mut loaded = false;
                                        if !http_resp.body.is_empty() {
                                            loaded = browser.load_html_document(http_resp.body);
                                        }
                                        if !loaded {
                                            let mut doc_buf = [0u8; 2048];
                                            let doc_len = browser::html_to_reader_document(http_resp.body, &mut doc_buf);
                                            if doc_len > 0 {
                                                browser.load_document_content(&doc_buf[..doc_len]);
                                                loaded = true;
                                            }
                                        }
                                        if loaded {
                                            if surface > 0 {
                                                render_browser_ui(surface, &browser);
                                                let _ = std::surface_flip();
                                            }
                                        }
                                    }
                                }
                            }
                        }
                        None => {
                            std::log("[BROWSER APP] Network event raised with no open socket; no packet was received.").ok();
                        }
                    }
                }
                if events & std::EVENT_TIMER_TICK != 0 {
                    std::yield_now();
                }
            }
            Err(_) => {
                std::yield_now();
            }
        }
    }
}

#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    // WHY the site is logged: the task exits either way, but a panic that says
    // only "a panic happened" leaves the person watching a device that closed
    // with nothing to trace. The file and line are what the build already
    // knows, and naming them costs no heap.
    let mut msg = [0u8; 128];
    let mut n = 0;
    for &b in b"[BROWSER APP] Panic at " {
        msg[n] = b;
        n += 1;
    }
    if let Some(loc) = info.location() {
        let file = loc.file().as_bytes();
        let skip = file.len().saturating_sub(48);
        let kept = file.len() - skip;
        msg[n..n + kept].copy_from_slice(&file[skip..]);
        n += kept;
        msg[n] = b':';
        n += 1;
        let mut digits = [0u8; 10];
        let mut dlen = 0;
        let mut line = loc.line();
        if line == 0 {
            digits[0] = b'0';
            dlen = 1;
        }
        while line > 0 {
            digits[dlen] = b'0' + (line % 10) as u8;
            dlen += 1;
            line /= 10;
        }
        while dlen > 0 {
            dlen -= 1;
            msg[n] = digits[dlen];
            n += 1;
        }
    } else {
        for &b in b"unknown site" {
            msg[n] = b;
            n += 1;
        }
    }
    if let Ok(s) = core::str::from_utf8(&msg[..n]) {
        std::log(s).ok();
    } else {
        std::log("[BROWSER APP] Panic detected! Terminating task...").ok();
    }
    std::exit();
}
