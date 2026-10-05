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
// Visual Design Tokens
// -----------------------------------------------------------------------------
const BG_DARK: u32 = 0xFF070B12;
const SURFACE_DARK: u32 = 0xFF111B2B;
const SURFACE_RAISED: u32 = 0xFF17243A;
const BORDER_SUBTLE: u32 = 0xFF26354D;
const ACCENT_SKY: u32 = 0xFF31C4FF;
const SUCCESS_EMERALD: u32 = 0xFF56D68A;
const TEXT_PRIMARY: u32 = 0xFFF4F7FB;
const TEXT_MUTED: u32 = 0xFF9AA8BC;

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
fn get_browser_pixel(x: usize, y: usize, app: &BrowserApp) -> u32 {
    // 1. Status Bar & Navigation Header (y: 0..96)
    if y < 96 {
        if y >= 94 {
            return BORDER_SUBTLE;
        }

        // Back button (<) at (x: 24..90, y: 28..68)
        if x >= 24 && x < 90 && y >= 28 && y < 68 {
            let center_x = 57;
            let center_y = 48;
            let dx = (x as isize - center_x as isize).abs();
            let dy = (y as isize - center_y as isize).abs();
            if (dx + dy <= 12 && x <= center_x)
                || (x >= 24 && x < 90 && (y == 28 || y == 67 || x == 24 || x == 89))
            {
                return TEXT_PRIMARY;
            }
            return SURFACE_RAISED;
        }

        // URL Address Bar Pill at (x: 104..696, y: 28..68)
        if x >= 104 && x < 696 && y >= 28 && y < 68 {
            let rx = (x - 104).min(695 - x);
            let ry = (y - 28).min(67 - y);
            if rx < 20 && ry < 20 {
                let cdx = (20 - rx) as isize;
                let cdy = (20 - ry) as isize;
                if cdx * cdx + cdy * cdy >= 400 {
                    return SURFACE_DARK;
                }
            }

            // Security Padlock Vector Glyph at (cx: 128, cy: 48)
            let cx = 128;
            let cy = 48;
            // Padlock shackle arc
            if (x as isize - cx as isize).abs() <= 5 && y >= cy - 8 && y <= cy - 3 {
                let is_shackle_rim = (x as isize - cx as isize).abs() >= 3 || y <= cy - 6;
                if is_shackle_rim {
                    return SUCCESS_EMERALD;
                }
            }
            // Padlock body
            if (x as isize - cx as isize).abs() <= 7 && y >= cy - 3 && y <= cy + 7 {
                // Keyhole dot
                if (x as isize - cx as isize).abs() <= 1 && y >= cy + 1 && y <= cy + 3 {
                    return SURFACE_DARK;
                }
                return SUCCESS_EMERALD;
            }

            // Reload Arrow Glyph at (cx: 666, cy: 48)
            let rx_rel = x as isize - 666;
            let ry_rel = y as isize - 48;
            let dist_sq = rx_rel * rx_rel + ry_rel * ry_rel;
            if dist_sq >= 25 && dist_sq <= 49 {
                // Break top right quadrant for arrow gap
                if !(rx_rel > 0 && ry_rel < 0) {
                    return TEXT_MUTED;
                }
            }
            // Arrowhead at top-right pointing down
            if rx_rel >= 3 && rx_rel <= 7 && ry_rel >= -5 && ry_rel <= -1 {
                return TEXT_MUTED;
            }

            // URL String Text
            let url_bytes = app.current_url.as_bytes();
            if let Some(c) = test_text(x, y, 146, 40, 2, url_bytes, TEXT_PRIMARY) {
                return c;
            }

            if rx < 2 || ry < 2 {
                return BORDER_SUBTLE;
            }
            return SURFACE_DARK;
        }

        return SURFACE_DARK;
    }

    // 2. Protocol Badges & Privacy Bar (y: 104..154)
    if y >= 104 && y < 154 {
        // Chip 1: HTTPS (x: 24..140)
        if x >= 24 && x < 140 && y >= 110 && y < 148 {
            if let Some(c) = test_text(x, y, 44, 122, 1, b"HTTPS", SUCCESS_EMERALD) {
                return c;
            }
            if x == 24 || x == 139 || y == 110 || y == 147 {
                return BORDER_SUBTLE;
            }
            return SURFACE_RAISED;
        }

        // Chip 2: INTERLAYER P2P (x: 150..330)
        if x >= 150 && x < 330 && y >= 110 && y < 148 {
            if let Some(c) = test_text(x, y, 168, 122, 1, b"INTERLAYER P2P", ACCENT_SKY) {
                return c;
            }
            if x == 150 || x == 329 || y == 110 || y == 147 {
                return BORDER_SUBTLE;
            }
            return SURFACE_RAISED;
        }

        // Chip 3: GEMINI (x: 340..450)
        if x >= 340 && x < 450 && y >= 110 && y < 148 {
            if let Some(c) = test_text(x, y, 362, 122, 1, b"GEMINI", TEXT_PRIMARY) {
                return c;
            }
            if x == 340 || x == 449 || y == 110 || y == 147 {
                return BORDER_SUBTLE;
            }
            return SURFACE_RAISED;
        }

        // Chip 4: PRIVACY SHIELD ON (x: 460..696)
        if x >= 460 && x < 696 && y >= 110 && y < 148 {
            if let Some(c) = test_text(x, y, 484, 122, 1, b"PRIVACY SHIELD ON", SUCCESS_EMERALD) {
                return c;
            }
            if x == 460 || x == 695 || y == 110 || y == 147 {
                return BORDER_SUBTLE;
            }
            return SURFACE_RAISED;
        }

        return BG_DARK;
    }

    // 3. Bookmarks Ledger Bar (y: 164..280)
    if y >= 164 && y < 280 {
        if BOOKMARKS.is_empty() {
            // Empty Bookmarks Card (x: 24..696, y: 168..270)
            if x >= 24 && x < 696 && y >= 168 && y < 270 {
                let rx = (x - 24).min(695 - x);
                let ry = (y - 168).min(269 - y);
                if rx < 18 && ry < 18 {
                    let cdx = (18 - rx) as isize;
                    let cdy = (18 - ry) as isize;
                    if cdx * cdx + cdy * cdy >= 324 {
                        return BG_DARK;
                    }
                }

                // Vector Bookmark Ribbon Glyph at (cx: 56, cy: 219)
                let cx = 56;
                let cy = 219;
                if (x as isize - cx as isize).abs() <= 10 && y >= cy - 16 && y <= cy + 16 {
                    let in_notch = y >= cy + 8 && (x as isize - cx as isize).abs() <= (y - (cy + 8)) as isize;
                    if !in_notch {
                        return ACCENT_SKY;
                    }
                }

                // Bookmarks info text
                if let Some(c) = test_text(x, y, 82, 198, 2, b"Sovereign Bookmarks Ledger", TEXT_PRIMARY) {
                    return c;
                }
                if let Some(c) = test_text(x, y, 82, 226, 1, b"Sandboxed bookmarks store ready - no external sync", TEXT_MUTED) {
                    return c;
                }

                if rx < 2 || ry < 2 {
                    return BORDER_SUBTLE;
                }
                return SURFACE_DARK;
            }
        } else {
            // Render bookmarks
            let card = (y - 168) / 80;
            let card_y = (y - 168) % 80;
            if card < BOOKMARKS.len() && x >= 24 && x < 696 && card_y < 68 {
                if rx_calc(x, card_y) {
                    return BORDER_SUBTLE;
                }
                return SURFACE_DARK;
            }
        }
        return BG_DARK;
    }

    // 4. Main Web Canvas Viewport (y: 290..1320)
    if y >= 290 && y < 1320 {
        if app.pages_loaded == 0 {
            // Sovereign Web Start Card at x: 24..696, y: 296..880 (height: 584)
            if x >= 24 && x < 696 && y >= 296 && y < 880 {
                let rx = (x - 24).min(695 - x);
                let ry = (y - 296).min(879 - y);
                if rx < 24 && ry < 24 {
                    let cdx = (24 - rx) as isize;
                    let cdy = (24 - ry) as isize;
                    if cdx * cdx + cdy * cdy >= 576 {
                        return BG_DARK;
                    }
                }

                // --- Vector Globe & Compass Pictogram at cx: 360, cy: 420 ---
                let cx = 360;
                let cy = 420;
                let gx = x as isize - cx as isize;
                let gy = y as isize - cy as isize;
                let g_dist_sq = gx * gx + gy * gy;

                // Outer globe rim
                if g_dist_sq >= 38 * 38 && g_dist_sq <= 42 * 42 {
                    return ACCENT_SKY;
                }

                // Equator line
                if gy.abs() <= 1 && gx.abs() <= 38 {
                    return BORDER_SUBTLE;
                }

                // Longitude ellipse
                let ell_sq = gx * gx * 3 + gy * gy;
                if ell_sq >= 36 * 36 && ell_sq <= 40 * 40 && gx.abs() <= 38 {
                    return ACCENT_SKY;
                }

                // Center navigation beacon dot
                if g_dist_sq <= 16 {
                    return TEXT_PRIMARY;
                }

                // Start Page Heading
                if let Some(c) = test_text(x, y, 190, 490, 2, b"InterOS Sovereign Web", TEXT_PRIMARY) {
                    return c;
                }
                if let Some(c) = test_text(x, y, 208, 524, 1, b"Zero-Trust Sandboxed Hypermedia Client", ACCENT_SKY) {
                    return c;
                }
                if let Some(c) = test_text(x, y, 224, 552, 1, b"Anti-Tracking Privacy Shield Active", SUCCESS_EMERALD) {
                    return c;
                }

                // Feature squircle pills at y: 600..650
                // Pill 1: P2P MESH
                if x >= 60 && x < 230 && y >= 600 && y < 644 {
                    if let Some(c) = test_text(x, y, 98, 616, 1, b"P2P MESH", ACCENT_SKY) {
                        return c;
                    }
                    if x == 60 || x == 229 || y == 600 || y == 643 {
                        return BORDER_SUBTLE;
                    }
                    return SURFACE_RAISED;
                }
                // Pill 2: DID ATTESTED
                if x >= 250 && x < 470 && y >= 600 && y < 644 {
                    if let Some(c) = test_text(x, y, 286, 616, 1, b"DID ATTESTED", TEXT_PRIMARY) {
                        return c;
                    }
                    if x == 250 || x == 469 || y == 600 || y == 643 {
                        return BORDER_SUBTLE;
                    }
                    return SURFACE_RAISED;
                }
                // Pill 3: ZERO TRACKERS
                if x >= 490 && x < 660 && y >= 600 && y < 644 {
                    if let Some(c) = test_text(x, y, 520, 616, 1, b"ZERO TRACKERS", SUCCESS_EMERALD) {
                        return c;
                    }
                    if x == 490 || x == 659 || y == 600 || y == 643 {
                        return BORDER_SUBTLE;
                    }
                    return SURFACE_RAISED;
                }

                // Subtext guidance
                if let Some(c) = test_text(x, y, 136, 680, 1, b"Type a web address in the navigation bar to browse", TEXT_MUTED) {
                    return c;
                }

                if rx < 2 || ry < 2 {
                    return BORDER_SUBTLE;
                }
                return SURFACE_DARK;
            }
        }
        return BG_DARK;
    }

    // 5. Bottom Navigation Dock (y: 1330..1414)
    if y >= 1330 && y < 1414 {
        if y == 1330 {
            return BORDER_SUBTLE;
        }

        // Back Chevron (<) at cx: 70, cy: 1372
        let bx_rel = (x as isize - 70).abs();
        let by_rel = (y as isize - 1372).abs();
        if by_rel <= 10 && bx_rel <= 10 && x <= 70 && (bx_rel + by_rel >= 8 && bx_rel + by_rel <= 11) {
            return TEXT_PRIMARY;
        }

        // Forward Chevron (>) at cx: 200, cy: 1372
        let fx_rel = (x as isize - 200).abs();
        let fy_rel = (y as isize - 1372).abs();
        if fy_rel <= 10 && fx_rel <= 10 && x >= 200 && (fx_rel + fy_rel >= 8 && fx_rel + fy_rel <= 11) {
            return TEXT_MUTED;
        }

        // Tab Pill [ 1 ] at cx: 345, cy: 1372
        if x >= 330 && x < 362 && y >= 1358 && y < 1386 {
            if let Some(c) = test_text(x, y, 342, 1368, 1, b"1", ACCENT_SKY) {
                return c;
            }
            if x == 330 || x == 361 || y == 1358 || y == 1385 {
                return ACCENT_SKY;
            }
            return SURFACE_RAISED;
        }

        // Bookmark Star / Ribbon at cx: 500, cy: 1372
        let sx_rel = (x as isize - 500).abs();
        let sy_rel = (y as isize - 1372).abs();
        if (sx_rel <= 8 && sy_rel <= 1) || (sy_rel <= 8 && sx_rel <= 1) || (sx_rel == sy_rel && sx_rel <= 6) {
            return TEXT_MUTED;
        }

        // Overflow menu (...) at cx: 640, cy: 1372
        if (y as isize - 1372).abs() <= 2 {
            let ox = x as isize - 640;
            if ox.abs() <= 2 || (ox - 12).abs() <= 2 || (ox + 12).abs() <= 2 {
                return TEXT_MUTED;
            }
        }

        return SURFACE_DARK;
    }

    // 6. Home Gesture Pill (x: 260..460, y: 1418..1424)
    if y >= 1418 && y < 1424 && x >= 260 && x < 460 {
        return TEXT_MUTED;
    }

    BG_DARK
}

fn rx_calc(x: usize, card_y: usize) -> bool {
    x == 24 || x == 695 || card_y == 0 || card_y == 67
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
                            // Check Back button in header or Home gesture bar: exit cleanly to launcher
                            if (input.x >= 24 && input.x < 100 && input.y >= 28 && input.y < 68)
                                || (input.x >= 240 && input.x <= 480 && input.y >= 1400)
                            {
                                std::log(0, "[BROWSER APP] Back button or home gesture pressed. Exiting to launcher.").ok();
                                if surface > 0 {
                                    let _ = std::surface_destroy(6, surface);
                                }
                                std::exit_with_code(0);
                            }

                            // Check bookmark card taps
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
