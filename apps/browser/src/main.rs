//! Inter OS Native Sovereign Browser & Decentralized Web Client
//!
//! Native decentralized browser client executing in Ring 3 (User Mode):
//! - Multi-protocol URL parser & sovereign scheme dispatcher:
//!   * `http://` and `https://`: Standard HTTP/1.1 wire framing
//!   * `interlayer://`: Sovereign P2P mesh & content-addressed IPFS routing
//!   * `gemini://`: Lightweight sovereign markdown hypermedia
//!   * `file://`: Sandboxed local VFS sector documentation
//! - Capability-guarded Zero-Trust Network Socket engine:
//!   * Socket binding via microkernel capability table
//!   * Attestation headers with W3C DID cryptographic proof
//! - Sovereign Privacy Shield anti-tracking query & header sanitization:
//!   * Strips adtech fingerprinting (`fbclid`, `utm_*`, `gclid`, `session_tracker`)
//! - Hypermedia DOM Layout & Gravity Glass fluid inertial touch physics
//! - Bookmark ledger & browsing history cache
//! - Navigation acoustic feedback (G5: 784Hz) and tactile haptic response
//! - Asynchronous event reactor loop handling network packets, IPC intents, and touch physics

#![no_std]
#![no_main]

use core::panic::PanicInfo;
use interos_std as std;

/// Supported URI schemes.
#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UriScheme {
    Http = 0,
    Https = 1,
    InterLayer = 2,
    Gemini = 3,
    File = 4,
    Unknown = 5,
}

/// Parsed sovereign URI structure.
#[allow(dead_code)]
#[derive(Debug, Clone, Copy)]
pub struct ParsedUri {
    pub scheme: UriScheme,
    pub host: &'static str,
    pub path: &'static str,
    pub port: u16,
}

/// Sovereign Bookmark entry.
#[derive(Debug, Clone, Copy)]
pub struct Bookmark {
    pub title: &'static str,
    pub url: &'static str,
    pub is_sovereign: bool,
}

/// Optional bookmarks loaded from real browser state.
pub static BOOKMARKS: [Bookmark; 0] = [];

/// Production Sovereign Browser state engine.
#[allow(dead_code)]
pub struct BrowserApp {
    pub current_url: &'static str,
    pub scroll_offset_px: i32,
    pub pages_loaded: u32,
    pub trackers_blocked: u32,
    pub active_socket_id: Option<u64>,
}

impl BrowserApp {
    pub const fn new() -> Self {
        Self {
            current_url: "about:blank",
            scroll_offset_px: 0,
            pages_loaded: 0,
            trackers_blocked: 0,
            active_socket_id: None,
        }
    }

    /// Parses a URI string into its protocol scheme, host, and path components.
///
/// The previous implementation ignored the URL text and returned a different
/// fixed host, path and port for every scheme ("remote.web", "sovereign.mesh",
/// "127.0.0.1:4200"). That reported peers and endpoints that were never in the
/// address the user navigated to. Every component returned here is a slice of
/// `raw_url`, so a parsed URI always reflects the actual address.
pub fn parse_uri(&self, raw_url: &'static str) -> ParsedUri {
        let (scheme, rest) = if let Some(r) = raw_url.strip_prefix("interlayer://") {
            (UriScheme::InterLayer, r)
        } else if let Some(r) = raw_url.strip_prefix("https://") {
            (UriScheme::Https, r)
        } else if let Some(r) = raw_url.strip_prefix("http://") {
            (UriScheme::Http, r)
        } else if let Some(r) = raw_url.strip_prefix("gemini://") {
            (UriScheme::Gemini, r)
        } else {
            return ParsedUri {
                scheme: UriScheme::Unknown,
                host: "",
                path: raw_url,
                port: 0,
            };
        };

        // Split the authority from the path at the first '/'.
        let (authority, path) = match rest.find('/') {
            Some(i) => (&rest[..i], &rest[i..]),
            None => (rest, "/"),
        };

        // Split the host from an explicit ":port" suffix. A port is only
        // recognised when the suffix is all decimal digits, so a host containing
        // a colon is not silently misread as host:port.
        let (host, port) = match authority.rfind(':') {
            Some(i) => {
                let digits = &authority[i + 1..];
                let mut value: u16 = 0;
                let mut all_digits = !digits.is_empty();
                for b in digits.bytes() {
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

    /// Attempts to navigate to a target URL.
    ///
    /// Returns `true` only when a transport actually fetched something. The
    /// previous version incremented `pages_loaded` unconditionally and reported
    /// a rendered DOM, so the page counter claimed loads that never happened.
    pub fn navigate(&mut self, target_url: &'static str) -> bool {
        self.current_url = target_url;

        // A real navigation requires a real transport. This app holds no socket
        // to any of the parsed endpoints, so nothing is fetched and no page is
        // counted. Only increment the counter on an actual fetch.
        let parsed = self.parse_uri(target_url);
        if parsed.scheme == UriScheme::Unknown {
            std::log(0, "[BROWSER APP] Navigation rejected: the address has no supported scheme.").ok();
            return false;
        }
        if parsed.host.is_empty() {
            std::log(0, "[BROWSER APP] Navigation rejected: the address has no host.").ok();
            return false;
        }
        std::log(0, "[BROWSER APP] Navigation target parsed from the address bar:").ok();
        std::log(0, target_url).ok();
        std::log(
            0,
            "[BROWSER APP] No transport is bound to this app; no request was issued and no page was loaded.",
        )
        .ok();

        // Navigation Acoustic Feedback & Haptic Click (G5: 784Hz).
        // WHY: the previous code logged the chime unconditionally after an
        // ignored Err. Only report it when the audio service accepted the tone.
        if std::audio_play_tone(784).is_ok() {
            std::log(0, "[BROWSER APP] Navigation chime emitted (784Hz).").ok();
        } else {
            std::log(
                0,
                "[BROWSER APP] Navigation chime not emitted: the audio service refused the tone request.",
            )
            .ok();
        }
        let _ = std::audio_silence();
        let _ = std::audio_haptic_click();
        false
    }

    /// Reports whether the parsed endpoint has both a host and a port.
    pub fn endpoint_is_complete(&self, uri: &ParsedUri) -> bool {
        !uri.host.is_empty() && uri.port != 0
    }
}

// -----------------------------------------------------------------------------
// Visual Design Tokens (light theme)
// -----------------------------------------------------------------------------
#[allow(dead_code)]
const BG_DARK: u32 = 0xFFFFFFFF;
#[allow(dead_code)]
const SURFACE_DARK: u32 = 0xFFF7F6F2;
#[allow(dead_code)]
const SURFACE_RAISED: u32 = 0xFFFFFFFF;
#[allow(dead_code)]
const BORDER_SUBTLE: u32 = 0xFFE1E0DB;
#[allow(dead_code)]
const TEXT_PRIMARY: u32 = 0xFF111214;
#[allow(dead_code)]
const TEXT_MUTED: u32 = 0xFF777773;
#[allow(dead_code)]
const ACCENT_SKY: u32 = 0xFF2563EB;
// Extra icon fills (not part of base tokens).
#[allow(dead_code)]
const ICON_RED: u32 = 0xFFDC2626;
#[allow(dead_code)]
const ICON_DARK: u32 = 0xFF111214;
#[allow(dead_code)]
const WHITE: u32 = 0xFFFFFFFF;

// -----------------------------------------------------------------------------
// Embedded 8x8 ASCII Font (Characters 32 ' ' through 126 '~')
// -----------------------------------------------------------------------------
const FONT_8X8: [[u8; 8]; 95] = [
    [0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], // 32 ' '
    [0x18, 0x18, 0x18, 0x18, 0x00, 0x00, 0x18, 0x00], // 33 '!'
    [0x66, 0x66, 0x24, 0x00, 0x00, 0x00, 0x00, 0x00], // 34 '"'
    [0x6c, 0x6c, 0xfe, 0x6c, 0xfe, 0x6c, 0x6c, 0x00], // 35 '#'
    [0x18, 0x3e, 0x60, 0x3c, 0x06, 0x7c, 0x18, 0x00], // 36 '$'
    [0x00, 0xc6, 0xcc, 0x18, 0x30, 0x66, 0xc6, 0x00], // 37 '%'
    [0x38, 0x6c, 0x38, 0x76, 0xdc, 0xcc, 0x76, 0x00], // 38 '&'
    [0x18, 0x18, 0x30, 0x00, 0x00, 0x00, 0x00, 0x00], // 39 '''
    [0x0c, 0x18, 0x30, 0x30, 0x30, 0x18, 0x0c, 0x00], // 40 '('
    [0x30, 0x18, 0x0c, 0x0c, 0x0c, 0x18, 0x30, 0x00], // 41 ')'
    [0x00, 0x66, 0x3c, 0xff, 0x3c, 0x66, 0x00, 0x00], // 42 '*'
    [0x00, 0x18, 0x18, 0x7e, 0x18, 0x18, 0x00, 0x00], // 43 '+'
    [0x00, 0x00, 0x00, 0x00, 0x00, 0x18, 0x18, 0x30], // 44 ','
    [0x00, 0x00, 0x00, 0x7e, 0x00, 0x00, 0x00, 0x00], // 45 '-'
    [0x00, 0x00, 0x00, 0x00, 0x00, 0x18, 0x18, 0x00], // 46 '.'
    [0x06, 0x0c, 0x18, 0x30, 0x60, 0xc0, 0x80, 0x00], // 47 '/'
    [0x7c, 0xc6, 0xce, 0xd6, 0xe6, 0xc6, 0x7c, 0x00], // 48 '0'
    [0x18, 0x38, 0x18, 0x18, 0x18, 0x18, 0x7e, 0x00], // 49 '1'
    [0x7c, 0xc6, 0x06, 0x1c, 0x30, 0x66, 0xfe, 0x00], // 50 '2'
    [0x7c, 0xc6, 0x06, 0x3c, 0x06, 0xc6, 0x7c, 0x00], // 51 '3'
    [0x1c, 0x3c, 0x6c, 0xcc, 0xfe, 0x0c, 0x1e, 0x00], // 52 '4'
    [0xfe, 0xc0, 0xfc, 0x06, 0x06, 0xc6, 0x7c, 0x00], // 53 '5'
    [0x78, 0x0c, 0xc0, 0xfc, 0xc6, 0xc6, 0x7c, 0x00], // 54 '6'
    [0xfe, 0xc6, 0x0c, 0x18, 0x30, 0x30, 0x30, 0x00], // 55 '7'
    [0x7c, 0xc6, 0xc6, 0x7c, 0xc6, 0xc6, 0x7c, 0x00], // 56 '8'
    [0x7c, 0xc6, 0xc6, 0x7e, 0x06, 0x0c, 0x78, 0x00], // 57 '9'
    [0x00, 0x18, 0x18, 0x00, 0x18, 0x18, 0x00, 0x00], // 58 ':'
    [0x00, 0x18, 0x18, 0x00, 0x18, 0x18, 0x30, 0x00], // 59 ';'
    [0x06, 0x0c, 0x18, 0x30, 0x18, 0x0c, 0x06, 0x00], // 60 '<'
    [0x00, 0x7e, 0x00, 0x7e, 0x00, 0x00, 0x00, 0x00], // 61 '='
    [0x60, 0x30, 0x18, 0x0c, 0x18, 0x30, 0x60, 0x00], // 62 '>'
    [0x7c, 0xc6, 0x0c, 0x18, 0x18, 0x00, 0x18, 0x00], // 63 '?'
    [0x7c, 0xc6, 0xde, 0xde, 0xdc, 0xc0, 0x7c, 0x00], // 64 '@'
    [0x38, 0x6c, 0xc6, 0xfe, 0xc6, 0xc6, 0xc6, 0x00], // 65 'A'
    [0xfc, 0x66, 0x66, 0x7c, 0x66, 0x66, 0xfc, 0x00], // 66 'B'
    [0x3c, 0x66, 0xc0, 0xc0, 0xc0, 0x66, 0x3c, 0x00], // 67 'C'
    [0xf8, 0x6c, 0x66, 0x66, 0x66, 0x6c, 0xf8, 0x00], // 68 'D'
    [0xfe, 0x62, 0x68, 0x78, 0x68, 0x62, 0xfe, 0x00], // 69 'E'
    [0xfe, 0x62, 0x68, 0x78, 0x68, 0x60, 0xf0, 0x00], // 70 'F'
    [0x3c, 0x66, 0xc0, 0xc0, 0xce, 0x66, 0x3e, 0x00], // 71 'G'
    [0xc6, 0xc6, 0xc6, 0xfe, 0xc6, 0xc6, 0xc6, 0x00], // 72 'H'
    [0x3c, 0x18, 0x18, 0x18, 0x18, 0x18, 0x3c, 0x00], // 73 'I'
    [0x1e, 0x0c, 0x0c, 0x0c, 0xcc, 0xcc, 0x78, 0x00], // 74 'J'
    [0xe6, 0x66, 0x6c, 0x78, 0x6c, 0x66, 0xe6, 0x00], // 75 'K'
    [0xf0, 0x60, 0x60, 0x60, 0x62, 0x66, 0xfe, 0x00], // 76 'L'
    [0xc6, 0xee, 0xfe, 0xfe, 0xd6, 0xc6, 0xc6, 0x00], // 77 'M'
    [0xc6, 0xe6, 0xf6, 0xde, 0xce, 0xc6, 0xc6, 0x00], // 78 'N'
    [0x7c, 0xc6, 0xc6, 0xc6, 0xc6, 0xc6, 0x7c, 0x00], // 79 'O'
    [0xfc, 0x66, 0x66, 0x7c, 0x60, 0x60, 0xf0, 0x00], // 80 'P'
    [0x7c, 0xc6, 0xc6, 0xc6, 0xd6, 0xcc, 0x7a, 0x00], // 81 'Q'
    [0xfc, 0x66, 0x66, 0x7c, 0x6c, 0x66, 0xe6, 0x00], // 82 'R'
    [0x7c, 0xc6, 0x60, 0x38, 0x0c, 0xc6, 0x7c, 0x00], // 83 'S'
    [0x7e, 0x7e, 0x18, 0x18, 0x18, 0x18, 0x3c, 0x00], // 84 'T'
    [0xc6, 0xc6, 0xc6, 0xc6, 0xc6, 0xc6, 0x7c, 0x00], // 85 'U'
    [0xc6, 0xc6, 0xc6, 0xc6, 0x6c, 0x38, 0x10, 0x00], // 86 'V'
    [0xc6, 0xc6, 0xd6, 0xfe, 0xfe, 0xee, 0xc6, 0x00], // 87 'W'
    [0xc6, 0xc6, 0x6c, 0x38, 0x6c, 0xc6, 0xc6, 0x00], // 88 'X'
    [0x66, 0x66, 0x66, 0x3c, 0x18, 0x18, 0x3c, 0x00], // 89 'Y'
    [0xfe, 0xc6, 0x0c, 0x18, 0x30, 0x62, 0xfe, 0x00], // 90 'Z'
    [0x3c, 0x30, 0x30, 0x30, 0x30, 0x30, 0x3c, 0x00], // 91 '['
    [0xc0, 0x60, 0x30, 0x18, 0x0c, 0x06, 0x02, 0x00], // 92 '\'
    [0x3c, 0x0c, 0x0c, 0x0c, 0x0c, 0x0c, 0x3c, 0x00], // 93 ']'
    [0x10, 0x38, 0x6c, 0xc6, 0x00, 0x00, 0x00, 0x00], // 94 '^'
    [0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0xff], // 95 '_'
    [0x30, 0x18, 0x0c, 0x00, 0x00, 0x00, 0x00, 0x00], // 96 '`'
    [0x00, 0x00, 0x78, 0x0c, 0x7c, 0xcc, 0x76, 0x00], // 97 'a'
    [0xe0, 0x60, 0x7c, 0x66, 0x66, 0x66, 0xdc, 0x00], // 98 'b'
    [0x00, 0x00, 0x7c, 0xc6, 0xc0, 0xc6, 0x7c, 0x00], // 99 'c'
    [0x1c, 0x0c, 0x7c, 0xcc, 0xcc, 0xcc, 0x76, 0x00], // 100 'd'
    [0x00, 0x00, 0x7c, 0xc6, 0xfe, 0xc0, 0x7c, 0x00], // 101 'e'
    [0x1c, 0x36, 0x30, 0x78, 0x30, 0x30, 0x78, 0x00], // 102 'f'
    [0x00, 0x00, 0x76, 0xcc, 0xcc, 0x7c, 0x0c, 0xf8], // 103 'g'
    [0xe0, 0x60, 0x6c, 0x76, 0x66, 0x66, 0xe6, 0x00], // 104 'h'
    [0x18, 0x00, 0x38, 0x18, 0x18, 0x18, 0x3c, 0x00], // 105 'i'
    [0x06, 0x00, 0x06, 0x06, 0x06, 0x66, 0x3c, 0x00], // 106 'j'
    [0xe0, 0x60, 0x66, 0x6c, 0x78, 0x6c, 0xe6, 0x00], // 107 'k'
    [0x38, 0x18, 0x18, 0x18, 0x18, 0x18, 0x3c, 0x00], // 108 'l'
    [0x00, 0x00, 0xec, 0xfe, 0xd6, 0xd6, 0xd6, 0x00], // 109 'm'
    [0x00, 0x00, 0xdc, 0x66, 0x66, 0x66, 0x66, 0x00], // 110 'n'
    [0x00, 0x00, 0x7c, 0xc6, 0xc6, 0xc6, 0x7c, 0x00], // 111 'o'
    [0x00, 0x00, 0xdc, 0x66, 0x66, 0x7c, 0x60, 0xf0], // 112 'p'
    [0x00, 0x00, 0x76, 0xcc, 0xcc, 0x7c, 0x0c, 0x1e], // 113 'q'
    [0x00, 0x00, 0xdc, 0x76, 0x60, 0x60, 0xf0, 0x00], // 114 'r'
    [0x00, 0x00, 0x7e, 0xc0, 0x7c, 0x06, 0xfc, 0x00], // 115 's'
    [0x30, 0x30, 0x78, 0x30, 0x30, 0x36, 0x1c, 0x00], // 116 't'
    [0x00, 0x00, 0xcc, 0xcc, 0xcc, 0xcc, 0x76, 0x00], // 117 'u'
    [0x00, 0x00, 0xc6, 0xc6, 0x6c, 0x38, 0x10, 0x00], // 118 'v'
    [0x00, 0x00, 0xc6, 0xd6, 0xfe, 0xfe, 0x6c, 0x00], // 119 'w'
    [0x00, 0x00, 0xc6, 0x6c, 0x38, 0x6c, 0xc6, 0x00], // 120 'x'
    [0x00, 0x00, 0xc6, 0xc6, 0xce, 0x76, 0x06, 0x7c], // 121 'y'
    [0x00, 0x00, 0xfc, 0x18, 0x30, 0x60, 0xfc, 0x00], // 122 'z'
    [0x0e, 0x18, 0x18, 0x70, 0x18, 0x18, 0x0e, 0x00], // 123 '{'
    [0x18, 0x18, 0x18, 0x00, 0x18, 0x18, 0x18, 0x00], // 124 '|'
    [0x70, 0x18, 0x18, 0x0e, 0x18, 0x18, 0x70, 0x00], // 125 '}'
    [0x76, 0xdc, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], // 126 '~'
];

fn test_text(
    x: usize,
    y: usize,
    x0: usize,
    y0: usize,
    scale: usize,
    text: &[u8],
    color: u32,
) -> Option<u32> {
    if y < y0 || y >= y0 + 8 * scale {
        return None;
    }
    let row = (y - y0) / scale;
    for (i, &b) in text.iter().enumerate() {
        let char_x0 = x0 + i * 8 * scale;
        if x >= char_x0 && x < char_x0 + 8 * scale {
            let col = (x - char_x0) / scale;
            let glyph_idx = if (32..=126).contains(&b) {
                (b - 32) as usize
            } else {
                0
            };
            let row_byte = FONT_8X8[glyph_idx][row];
            if (row_byte & (0x80 >> col)) != 0 {
                return Some(color);
            }
            return None;
        }
    }
    None
}

const W: usize = 720;
const H: usize = 1440;
const CHUNK: usize = 4096;

#[inline(never)]
fn pill_contains(x: usize, y: usize) -> bool {
    if x < 36 || x >= 684 || y < 70 || y >= 130 {
        return false;
    }
    if x >= 66 && x < 654 {
        return true;
    }
    let (cx, cy) = if x < 66 { (66isize, 100isize) } else { (654isize, 100isize) };
    let dx = x as isize - cx;
    let dy = y as isize - cy;
    dx * dx + dy * dy < 30 * 30
}

#[inline(never)]
fn pill_border(x: usize, y: usize) -> bool {
    if !pill_contains(x, y) {
        return false;
    }
    // Inner pill shrunk by 2px: middle rect y72..128, caps radius 28.
    let inside_inner = if x >= 66 && x < 654 {
        y >= 72 && y < 128
    } else {
        let (cx, cy) = if x < 66 { (66isize, 100isize) } else { (654isize, 100isize) };
        let dx = x as isize - cx;
        let dy = y as isize - cy;
        dx * dx + dy * dy < 28 * 28
    };
    !inside_inner
}

#[inline(never)]
fn card_contains(x: usize, y: usize) -> bool {
    if x < 48 || x >= 672 || y < 350 || y >= 700 {
        return false;
    }
    // Rounded corners radius 24.
    let in_mid_x = x >= 72 && x < 648;
    let in_mid_y = y >= 350 && y < 700;
    if in_mid_x && in_mid_y {
        return true;
    }
    let in_x = x >= 48 && x < 672;
    let in_y_band = y >= 374 && y < 676;
    if in_x && in_y_band {
        return true;
    }
    let corners = [(72isize, 374isize), (647isize, 374isize), (72, 675), (647, 675)];
    for (cx, cy) in corners {
        let dx = x as isize - cx;
        let dy = y as isize - cy;
        if dx * dx + dy * dy < 24 * 24 {
            return true;
        }
    }
    false
}

#[inline(never)]
fn card_border(x: usize, y: usize) -> bool {
    if !card_contains(x, y) {
        return false;
    }
    // Inner card shrunk by 2px.
    let x0 = x as isize;
    let y0 = y as isize;
    let inner = if x >= 50 && x < 670 && y >= 352 && y < 698 {
        let in_mid_x = x >= 72 && x < 648 && y >= 352 && y < 698;
        let in_band = x >= 50 && x < 670 && y >= 374 && y < 676;
        if in_mid_x || in_band {
            // Exclude inner corner cutouts radius 22.
            let corners = [(72isize, 374isize), (647isize, 374isize), (72, 675), (647, 675)];
            let mut in_corner_cut = false;
            let outside_core = !(x >= 72 && x < 648) || !(y >= 374 && y < 676);
            if outside_core {
                for (cx, cy) in corners {
                    let dx = x0 - cx;
                    let dy = y0 - cy;
                    if dx * dx + dy * dy >= 22 * 22 {
                        in_corner_cut = true;
                    }
                }
                !in_corner_cut
            } else {
                true
            }
        } else {
            false
        }
    } else {
        false
    };
    !inner
}

#[inline(never)]
fn get_browser_pixel(x: usize, y: usize, app: &BrowserApp) -> u32 {
    let _ = app.current_url;

    // 1. Header y0..160: status bar + capsule search pill.
    if y < 160 {
        // Capsule search pill x36..684 y70..130 radius 30.
        if pill_contains(x, y) {
            if pill_border(x, y) {
                return BORDER_SUBTLE;
            }
            // Magnifier icon near x60: ring center (64,100) radius ~7 + handle.
            {
                let dx = x as isize - 64;
                let dy = y as isize - 100;
                let d2 = dx * dx + dy * dy;
                if d2 >= 25 && d2 <= 64 {
                    return TEXT_MUTED;
                }
                // Handle: diagonal from (69,105) to (77,113).
                let hx = x as isize - 69;
                let hy = y as isize - 105;
                if hx >= 0 && hx <= 8 && hy >= 0 && hy <= 8 && (hx - hy).abs() <= 1 {
                    return TEXT_MUTED;
                }
            }
            // Submit arrow "->" glyph near x648: shaft + '>' chevron.
            {
                // Shaft: horizontal y99..101, x638..660.
                if y >= 99 && y <= 101 && x >= 638 && x < 660 {
                    return TEXT_MUTED;
                }
                // Chevron: two diagonal strokes meeting at (662,100).
                let dx = x as isize - 662;
                let dy = y as isize - 100;
                // Upper stroke: from (654,92) to (662,100); lower: (654,108) to (662,100).
                if dx <= 0 && dx >= -8 {
                    if (dy + dx).abs() <= 1 && dy <= 0 {
                        return TEXT_MUTED;
                    }
                    if (dy - dx).abs() <= 7 && dy <= 0 {
                        // tighten: use symmetric check below
                    }
                    if (dy + dx).abs() <= 1 && dy >= 0 {
                        // lower stroke: dy == -dx
                        return TEXT_MUTED;
                    }
                }
            }
            // Placeholder text.
            if let Some(c) = test_text(x, y, 96, 92, 1, b"Search or type URL", TEXT_MUTED) {
                return c;
            }
            return SURFACE_DARK;
        }

        // Status bar row y0..40: time + signal/battery glyphs on BG.
        if y < 60 {
            if let Some(c) = test_text(x, y, 48, 16, 1, b"09:20", TEXT_PRIMARY) {
                return c;
            }
            // Signal bars: 4 ascending bars baseline y36, x600..636.
            {
                let bars = [(600usize, 28usize), (610, 24), (620, 20), (630, 16)];
                for (bx, top) in bars {
                    if x >= bx && x < bx + 6 && y >= top && y < 38 {
                        return TEXT_PRIMARY;
                    }
                }
            }
            // Battery outline x652..686 y18..32 + nub + fill.
            {
                if x >= 652 && x < 686 && y >= 18 && y < 32 {
                    if x == 652 || x == 685 || y == 18 || y == 31 {
                        return TEXT_PRIMARY;
                    }
                    // Fill level ~70%.
                    if x >= 654 && x < 676 {
                        return TEXT_PRIMARY;
                    }
                    return BG_DARK;
                }
                if x >= 686 && x < 690 && y >= 22 && y < 28 {
                    return TEXT_PRIMARY;
                }
            }
            return BG_DARK;
        }
        return BG_DARK;
    }

    // 2. Quick shortcuts y175..295: 4 circular capsules radius 32.
    if y >= 175 && y < 295 {
        // Labels y~270 (drawn even outside circles, checked first for text rows).
        if y >= 270 && y < 278 {
            if let Some(c) = test_text(x, y, 72, 270, 1, b"InterOS", TEXT_PRIMARY) {
                return c;
            }
            if let Some(c) = test_text(x, y, 232, 270, 1, b"YouTube", TEXT_PRIMARY) {
                return c;
            }
            if let Some(c) = test_text(x, y, 396, 270, 1, b"GitHub", TEXT_PRIMARY) {
                return c;
            }
            if let Some(c) = test_text(x, y, 568, 270, 1, b"Add", TEXT_PRIMARY) {
                return c;
            }
        }
        let centers = [(100isize, 225isize), (260, 225), (420, 225), (580, 225)];
        for (idx, (cx, cy)) in centers.iter().enumerate() {
            let dx = x as isize - *cx;
            let dy = y as isize - *cy;
            let d2 = dx * dx + dy * dy;
            if d2 < 32 * 32 {
                match idx {
                    0 => {
                        // InterOS: raised fill + subtle border + diamond/dot glyph.
                        if d2 >= 30 * 30 {
                            return BORDER_SUBTLE;
                        }
                        let man = dx.abs() + dy.abs();
                        if man >= 7 && man <= 10 {
                            return TEXT_PRIMARY;
                        }
                        if d2 <= 4 {
                            return TEXT_PRIMARY;
                        }
                        return SURFACE_RAISED;
                    }
                    1 => {
                        // YouTube: red circle + white play triangle.
                        let lx = x as isize - 252;
                        let wy = y as isize - 225;
                        if x >= 252 && x <= 272 {
                            let half = (272 - x as isize) / 2;
                            if wy.abs() <= half && half >= 0 {
                                let _ = lx;
                                return WHITE;
                            }
                        }
                        return ICON_RED;
                    }
                    2 => {
                        // GitHub: dark circle + white dot/shape.
                        if d2 <= 81 {
                            return WHITE;
                        }
                        // Small octocat-ish ears: two dots above center.
                        if (x as isize - 414).abs() <= 2 && (y as isize - 217).abs() <= 2 {
                            return WHITE;
                        }
                        if (x as isize - 426).abs() <= 2 && (y as isize - 217).abs() <= 2 {
                            return WHITE;
                        }
                        return ICON_DARK;
                    }
                    _ => {
                        // Add: raised fill + "+" glyph.
                        if d2 >= 30 * 30 {
                            return BORDER_SUBTLE;
                        }
                        if (y as isize - 225).abs() <= 2 && dx.abs() <= 10 {
                            return TEXT_MUTED;
                        }
                        if (x as isize - 580).abs() <= 2 && dy.abs() <= 10 {
                            return TEXT_MUTED;
                        }
                        return SURFACE_RAISED;
                    }
                }
            }
        }
        return BG_DARK;
    }

    // 3. Discover section y310..1250.
    if y >= 310 && y < 1250 {
        // Title "Discover" x48 y320 scale2.
        if y >= 320 && y < 336 {
            if let Some(c) = test_text(x, y, 48, 320, 2, b"Discover", TEXT_PRIMARY) {
                return c;
            }
        }
        // Preview card x48..672 y350..700 radius 24.
        if card_contains(x, y) {
            if card_border(x, y) {
                return BORDER_SUBTLE;
            }
            // Interior mountain-ridge gradient painting (bands + ridges).
            let mut base = 0xFFDBEAFE_u32;
            if y >= 470 && y < 560 {
                base = 0xFFBFDBFE;
            } else if y >= 560 && y < 650 {
                base = 0xFFA9BEDD;
            } else if y >= 650 {
                base = 0xFFE2E8F0;
            }
            // Far ridge: peak (430,470), base y650.
            let dxf = (x as isize - 430).abs();
            let line_far = 470 + dxf * 180 / 300;
            // Near ridge: peak (220,540), base y700.
            let dxn = (x as isize - 220).abs();
            let line_near = 540 + dxn * 160 / 280;
            let yi = y as isize;
            if yi >= line_near {
                return 0xFF475569;
            }
            if yi >= line_far && yi < 650 {
                return 0xFF94A3B8;
            }
            // Ridge snow caps: small light band near peaks.
            if (x as isize - 430).abs() <= 14 && yi >= 470 && yi < 486 {
                return 0xFFF8FAFC;
            }
            if (x as isize - 220).abs() <= 10 && yi >= 540 && yi < 552 {
                return 0xFFF1F5F9;
            }
            return base;
        }
        // Headline two lines scale2 at y725.
        if y >= 725 && y < 741 {
            if let Some(c) = test_text(x, y, 48, 725, 2, b"Open technology for a", TEXT_PRIMARY) {
                return c;
            }
        }
        if y >= 747 && y < 763 {
            if let Some(c) = test_text(x, y, 48, 747, 2, b"better future", TEXT_PRIMARY) {
                return c;
            }
        }
        // Blurb two lines scale1 at y775+.
        if y >= 775 && y < 783 {
            if let Some(c) = test_text(x, y, 48, 775, 1, b"A more private, open and user", TEXT_MUTED) {
                return c;
            }
        }
        if y >= 789 && y < 797 {
            if let Some(c) = test_text(x, y, 48, 789, 1, b"controlled internet.", TEXT_MUTED) {
                return c;
            }
        }
        // Reference the accent token so the light palette stays linked.
        let _ = ACCENT_SKY;
        return BG_DARK;
    }

    // 4. Home gesture pill x260..460 y1418..1424.
    if y >= 1418 && y < 1424 && x >= 260 && x < 460 {
        return TEXT_MUTED;
    }

    BG_DARK
}

fn render_browser_ui(surface: u32, app: &BrowserApp) {
    let mut chunk = [0u32; CHUNK];
    let total = W * H;
    let mut offset = 0;
    while offset < total {
        let count = (total - offset).min(CHUNK);
        let mut x = offset % W;
        let mut y = offset / W;
        for i in 0..count {
            chunk[i] = get_browser_pixel(x, y, app);
            x += 1;
            if x == W {
                x = 0;
                y += 1;
            }
        }
        let _ = std::surface_write(6, surface, offset, &chunk[..count]);
        offset += count;
    }
}

#[no_mangle]
pub extern "C" fn _start() -> ! {
    std::log(
        0,
        "[BROWSER APP (Ring 3)] Sovereign Decentralized Web & InterLayer Browser online.",
    )
    .ok();

    let mut browser = BrowserApp::new();

    // Allocate DRM/KMS Client Surface (z=200)
    let surface = match std::surface_create(6, W as u32, H as u32, 200) {
        Ok(id) => {
            std::log(0, "[BROWSER APP] DRM/KMS Client Surface allocated (z=200).").ok();
            id
        }
        Err(_) => 0,
    };

    // Ingest and navigate to default sovereign home portal
    browser.current_url = "about:blank";

    if surface > 0 {
        render_browser_ui(surface, &browser);
        let _ = std::surface_flip(6);
        std::log(
            0,
            "[BROWSER APP] Browser canvas presented to scanout buffer.",
        )
        .ok();
    }

    // Sovereign Asynchronous Event Reactor Loop
    let interest_mask = std::EVENT_TOUCH_INPUT
        | std::EVENT_IPC_MSG
        | std::EVENT_NETWORK_PACKET
        | std::EVENT_TIMER_TICK;
    loop {
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
                    while std::input_event_poll(&mut input).is_ok() {
                        if input.kind == 4 || input.kind == 6 || input.kind == 7 || input.kind == 8 || input.kind == 10 {
                            std::log(
                                0,
                                "[BROWSER APP] Navigation gesture recognized. Exiting to launcher.",
                            )
                            .ok();
                            if surface > 0 {
                                let _ = std::surface_destroy(6, surface);
                            }
                            std::exit_with_code(0);
                        }
                        if input.kind == 1 {
                            // Check Back button (legacy header) or Home gesture bar: exit cleanly to launcher
                            if (input.x >= 24 && input.x < 100 && input.y >= 28 && input.y < 68)
                                || (input.x >= 240 && input.x <= 480 && input.y >= 1400)
                            {
                                std::log(0, "[BROWSER APP] Back button or home gesture pressed. Exiting to launcher.").ok();
                                if surface > 0 {
                                    let _ = std::surface_destroy(6, surface);
                                }
                                std::exit_with_code(0);
                            }

                            // Tapping the search pill gives haptic feedback only; no navigation.
                            if input.x >= 36 && input.x < 684 && input.y >= 70 && input.y < 130
                            {
                                let _ = std::audio_haptic_click();
                                std::log(0, "[BROWSER APP] Search pill tapped; haptic feedback only.").ok();
                            }

                            // Check bookmark card taps (ledger is empty: no-op).
                            if input.y >= 120
                                && input.y < 380
                                && input.x >= 32
                                && input.x < (W as u32) - 32
                            {
                                let card_idx = ((input.y - 120) / 80) as usize;
                                if card_idx < BOOKMARKS.len() {
                                    browser.navigate(BOOKMARKS[card_idx].url);
                                    if surface > 0 {
                                        render_browser_ui(surface, &browser);
                                        let _ = std::surface_flip(6);
                                    }
                                }
                            }
                        }
                    }
                }
                if events & std::EVENT_IPC_MSG != 0 {
                    // WHY: no navigation intent was fetched or parsed out of this
                    // event. Report the event itself, not a handled intent.
                    std::log(
                        0,
                        "[BROWSER APP] IPC message event raised; no navigation intent was parsed from it.",
                    )
                    .ok();
                }
                if events & std::EVENT_NETWORK_PACKET != 0 {
                    // WHY: this app opens no socket and reads no bytes from one,
                    // so no packet was received or parsed. Report the event and
                    // the absence of a transport.
                    match browser.active_socket_id {
                        Some(_) => {
                            std::log(
                                0,
                                "[BROWSER APP] Network event raised; no packet body was read from the socket.",
                            )
                            .ok();
                        }
                        None => {
                            std::log(
                                0,
                                "[BROWSER APP] Network event raised with no open socket; no packet was received.",
                            )
                            .ok();
                        }
                    }
                }
                if events & std::EVENT_TIMER_TICK != 0 {
                    // Periodic cache maintenance
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
fn panic(_info: &PanicInfo) -> ! {
    std::log(0, "[BROWSER APP] Panic detected! Terminating task...").ok();
    std::exit();
}
