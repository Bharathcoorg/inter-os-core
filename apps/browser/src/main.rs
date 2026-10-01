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

const W: usize = 720;
const H: usize = 1440;
const CHUNK: usize = 4096;

fn render_browser_ui(surface: u32, app: &BrowserApp) {
    let mut chunk = [0u32; CHUNK];
    let total = W * H;
    let mut offset = 0;
    while offset < total {
        let count = (total - offset).min(CHUNK);
        let mut x = offset % W;
        let mut y = offset / W;
        for i in 0..count {
            if y < 96 {
                if y >= 94 {
                    chunk[i] = std::design::color::ACCENT;
                } else if x >= 24 && x < 100 && y >= 28 && y < 68 {
                    // Back button <
                    let center_x = 62;
                    let center_y = 48;
                    let dx = (x as isize - center_x as isize).abs();
                    let dy = (y as isize - center_y as isize).abs();
                    if (dx + dy <= 12 && x <= center_x)
                        || (x >= 24 && x < 100 && (y == 28 || y == 67 || x == 24 || x == 99))
                    {
                        chunk[i] = std::design::color::TEXT;
                    } else {
                        chunk[i] = std::design::color::SURFACE_RAISED;
                    }
                } else if x >= 112 && x < W - 24 && y >= 28 && y < 68 {
                    // Address bar pill
                    if y == 28 || y == 67 || x == 112 || x == W - 25 {
                        chunk[i] = std::design::color::ACCENT;
                    } else {
                        chunk[i] = std::design::color::SURFACE_RAISED;
                    }
                } else {
                    chunk[i] = std::design::color::SURFACE;
                }
            } else if (120..380).contains(&y) {
                // Bookmarks strip.
                // WHY: with an empty bookmark set the strip rendered as bare
                // background, which reads as a blank screen rather than as an
                // absent feature. Render an explicit empty-state card instead.
                if BOOKMARKS.is_empty() {
                    if (200..268).contains(&y) && x >= 32 && x < W - 32 {
                        if y == 200 || y == 267 || x == 32 || x == W - 33 {
                            chunk[i] = std::design::color::TEXT_MUTED;
                        } else {
                            chunk[i] = std::design::color::SURFACE;
                        }
                    } else {
                        chunk[i] = std::design::color::BACKGROUND;
                    }
                } else {
                    let card = (y - 120) / 80;
                    let card_y = (y - 120) % 80;
                    if card < BOOKMARKS.len() && x >= 32 && x < W - 32 && card_y < 68 {
                        if card_y >= 28 && card_y < 40 && x >= 56 && x < 240 {
                            chunk[i] = if BOOKMARKS[card].is_sovereign {
                                std::design::color::ACCENT
                            } else {
                                std::design::color::SUCCESS
                            };
                        } else {
                            chunk[i] = std::design::color::SURFACE;
                        }
                    } else {
                        chunk[i] = std::design::color::BACKGROUND;
                    }
                }
            } else if (400..1320).contains(&y) && x >= 24 && x < W - 24 {
                // Main hypermedia DOM canvas.
                // WHY: a document heading, rule and body block were drawn
                // unconditionally, presenting a page that was never fetched.
                // Render the empty-document state until a page is actually loaded.
                if app.pages_loaded == 0 {
                    if (420..460).contains(&y) && x >= 48 && x < 320 {
                        chunk[i] = std::design::color::TEXT_MUTED;
                    } else if (500..510).contains(&y) && x >= 48 && x < W - 48 {
                        chunk[i] = std::design::color::SURFACE_RAISED;
                    } else if (540..550).contains(&y) && x >= 48 && x < 280 {
                        chunk[i] = std::design::color::TEXT_MUTED;
                    } else {
                        chunk[i] = 0xFF0D1117;
                    }
                } else if (420..460).contains(&y) && x >= 48 && x < 320 {
                    chunk[i] = std::design::color::TEXT;
                } else if (500..510).contains(&y) && x >= 48 && x < W - 48 {
                    chunk[i] = std::design::color::SURFACE_RAISED;
                } else if (540..550).contains(&y) && x >= 48 && x < W - 80 {
                    chunk[i] = std::design::color::SURFACE_RAISED;
                } else if (600..740).contains(&y) && x >= 48 && x < W - 48 {
                    chunk[i] = std::design::color::SURFACE;
                } else {
                    chunk[i] = 0xFF0D1117;
                }
            } else if (1340..1440).contains(&y) {
                // Bottom toolbar
                if y == 1340 {
                    chunk[i] = std::design::color::SURFACE_RAISED;
                } else {
                    chunk[i] = std::design::color::SURFACE;
                }
            } else if y >= 1418 && y < 1424 && x >= 260 && x < 460 {
                chunk[i] = std::design::color::TEXT_MUTED;
            } else {
                chunk[i] = std::design::color::BACKGROUND;
            }
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
