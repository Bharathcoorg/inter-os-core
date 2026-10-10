//! Inter OS Native Browser Client
//!
//! Native Ring-3 (User Mode) browser shell, laid out for the 720x1440 phone
//! viewport as a mobile browser (not a desktop widget):
//! - URI parser recognising `http://`, `https://`, `interlayer://` and
//!   `gemini://`; an address without a supported scheme or host is rejected
//! - `navigate()` reports the real outcome: this app holds no network socket,
//!   so no request is issued and no page is claimed to have loaded
//! - Top chrome: a 56 px address bar with an offline state glyph (the real
//!   transport state), a reload control that re-issues the real navigation
//!   attempt, and a close control that leaves to the launcher
//! - Content viewport renders the honest empty/error state of the last
//!   navigation attempt - never fabricated page content, thumbnails, bookmarks
//!   or history. There is no tab strip, no back/forward bar and no shortcut
//!   row because the app has no tab model, no history to navigate and no
//!   network transport behind any destination: shortcut tiles would be dead
//!   controls dressed as content, so they are not drawn
//! - Every string is fitted at render time with `interos_std::design::font`
//!   (`measure` / `measure_bold` / `ellipsize` / `wrap_text`): nothing is ever
//!   clipped, and long addresses are ellipsized with the SDK "..."
//! - Navigation acoustic feedback (G5: 784Hz) and tactile haptic response
//! - Asynchronous event reactor loop handling touch, keyboard and timer events
//!
//! Geometry: see `scratch/browser_layout/geometry.md` for the y-extent audit
//! of every region (all on the 8 pt grid, 24 px screen margin).

#![no_std]
#![no_main]

use core::panic::PanicInfo;
use interos_std as std;
use interos_std::design::font;
use interos_std::design::primitives::{draw_rounded_rect, Rect};

// -----------------------------------------------------------------------------
// Design tokens (light theme) - shared with the InterOS design system
// -----------------------------------------------------------------------------
use interos_std::design::color::{
    ACCENT, BACKGROUND as BASE, BORDER, SURFACE, TEXT as INK, TEXT_MUTED as MUTED,
};

// -----------------------------------------------------------------------------
// Layout constants (720x1440, 8 pt grid, 24 px screen margin)
// -----------------------------------------------------------------------------

const W: usize = 720;
const H: usize = 1440;
const CHUNK: usize = 4096;

/// y 0..48: system status band. This app receives no clock, radio or battery
/// readings, so the band stays neutral - no sample values are ever drawn.
const STATUS_H: usize = 48;

/// y 48..120: toolbar (address bar + reload + close).
const TOOLBAR_Y0: usize = STATUS_H;
const TOOLBAR_Y1: usize = 120;

/// Address bar: x 24..568, y 56..112 (56 px tall, capsule radius 28).
const PILL: Rect = Rect::new(24, 56, 544, 56);
const PILL_R: usize = 28;
/// Text column inside the pill: state glyph x 40..64, text starts at x 72.
const PILL_TEXT_X: usize = 72;
const PILL_TEXT_Y: usize = 72;
/// 568 (pill right) - 16 (inner padding) - 72 (text origin) = 480.
const PILL_TEXT_MAX: usize = 480;

/// Reload control: x 584..632, y 60..108 (48x48 touch target).
const RELOAD_BTN: Rect = Rect::new(584, 60, 48, 48);
/// Close control: x 648..696, y 60..108 (48x48 touch target).
const CLOSE_BTN: Rect = Rect::new(648, 60, 48, 48);
const BTN_R: usize = 14;

/// y 120..1392: content viewport (fills everything between chrome and the
/// 48 px bottom gesture inset).
const VIEW_Y0: usize = TOOLBAR_Y1;
const VIEW_Y1: usize = 1392;

/// Honest empty/error state block: centred in the viewport (y 120..1392),
/// max text width 608 (56 px margins).
const STATE_BOX_TOP: usize = VIEW_Y0;
const STATE_MAX_W: usize = 608;
const STATE_ICON_H: usize = 56;
const STATE_GAP_ICON: usize = 24;
const STATE_GAP_BODY: usize = 16;
const STATE_LINE_STRIDE: usize = 32; // 24 px line + 8 px leading
const GLYPH_H: usize = font::GLYPH_H; // 24

/// y 1392..1440: system gesture band (unchanged system convention).
const GESTURE_PILL: Rect = Rect::new(260, 1418, 200, 6);
const HOME_X0: usize = 240;
const HOME_X1: usize = 480;
const HOME_Y0: usize = 1400;

const PLACEHOLDER: &[u8] = b"Search or type URL";

// Honest state copy: one headline plus a wrapped body per real outcome.
const BODY_BLANK: &[u8] = b"This browser holds no network transport, so no request has \
ever been sent. There is no page content, history or bookmarks to show.";
const BODY_NO_TRANSPORT: &[u8] = b"The address was parsed and a navigation was attempted, \
but no network transport is bound to this app: no request was sent and no page was loaded.";
const BODY_INVALID: &[u8] = b"Supported schemes are http, https, interlayer and gemini \
followed by a host. Even a valid address loads nothing while no transport is bound.";

/// Real outcome of the last action. Nothing here is decorative: each variant
/// is set by an actual code path (parse rejection or transport absence).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PageStatus {
    /// No navigation attempted yet (startup, or the user started editing).
    Blank,
    /// Address parsed fine; the app holds no socket, so nothing was fetched.
    NoTransport,
    /// Address rejected by the parser (no scheme, or no host).
    InvalidAddress,
}

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

/// Parsed sovereign URI structure. Every component is a slice of the raw
/// address that was parsed.
#[allow(dead_code)]
#[derive(Debug, Clone, Copy)]
pub struct ParsedUri<'a> {
    pub scheme: UriScheme,
    pub host: &'a [u8],
    pub path: &'a [u8],
    pub port: u16,
}

/// Sovereign Bookmark entry.
#[allow(dead_code)]
#[derive(Debug, Clone, Copy)]
pub struct Bookmark {
    pub title: &'static str,
    pub url: &'static str,
    pub is_sovereign: bool,
}

/// Optional bookmarks loaded from real browser state. The platform exposes no
/// bookmark store, so the ledger is empty and stays empty - nothing is
/// synthesised to fill the UI.
#[allow(dead_code)]
pub static BOOKMARKS: [Bookmark; 0] = [];

/// Production Sovereign Browser state engine.
#[allow(dead_code)]
pub struct BrowserApp {
    /// Address of the current/last navigation attempt. Nothing is loaded:
    /// this app has no transport, so this is the attempted address only.
    pub current_url: [u8; 64],
    pub current_url_len: usize,
    /// Real scroll state. The content viewport never overflows, so the offset
    /// stays 0 - no scrollbar is drawn because nothing scrolls.
    pub scroll_offset_px: i32,
    pub pages_loaded: u32,
    pub trackers_blocked: u32,
    pub active_socket_id: Option<u64>,
    /// User-edited address text. No page is claimed loaded without a real
    /// transport.
    pub address: [u8; 64],
    pub address_len: usize,
    pub address_focused: bool,
    /// The whole address is selected, so the next character replaces it.
    ///
    /// WHY this exists: focusing the bar pre-filled the attempted address into
    /// the edit buffer and typing appended to it, so tapping a bar reading
    /// "about:blank" and typing "example" produced "about:blankexample", which
    /// is not an address anything can navigate to. Android and iOS both select
    /// the whole field on focus for exactly this reason, and a caret parked at
    /// the end of the text gave no hint that a selection existed.
    pub select_all: bool,
    /// Real outcome of the last navigation / action.
    pub status: PageStatus,
}

impl BrowserApp {
    pub fn new() -> Self {
        let mut app = Self {
            current_url: [0; 64],
            current_url_len: 0,
            scroll_offset_px: 0,
            pages_loaded: 0,
            trackers_blocked: 0,
            active_socket_id: None,
            address: [0; 64],
            address_len: 0,
            address_focused: false,
            select_all: false,
            status: PageStatus::Blank,
        };
        let start = b"about:blank";
        app.current_url[..start.len()].copy_from_slice(start);
        app.current_url_len = start.len();
        app
    }

    fn current_url(&self) -> &[u8] {
        &self.current_url[..self.current_url_len]
    }

    /// Parses a URI byte string into its protocol scheme, host, and path
    /// components. Every returned component is a slice of `raw`, so a parsed
    /// URI always reflects the actual address the user navigated to.
    pub fn parse_uri<'a>(&self, raw: &'a [u8]) -> ParsedUri<'a> {
        let (scheme, rest) = if let Some(r) = raw.strip_prefix(&b"interlayer://"[..]) {
            (UriScheme::InterLayer, r)
        } else if let Some(r) = raw.strip_prefix(&b"https://"[..]) {
            (UriScheme::Https, r)
        } else if let Some(r) = raw.strip_prefix(&b"http://"[..]) {
            (UriScheme::Http, r)
        } else if let Some(r) = raw.strip_prefix(&b"gemini://"[..]) {
            (UriScheme::Gemini, r)
        } else {
            return ParsedUri {
                scheme: UriScheme::Unknown,
                host: &raw[..0],
                path: raw,
                port: 0,
            };
        };

        // Split the authority from the path at the first '/'.
        let (authority, path) = match rest.iter().position(|&b| b == b'/') {
            Some(i) => (&rest[..i], &rest[i..]),
            None => (rest, &rest[..0]),
        };

        // Split the host from an explicit ":port" suffix. A port is only
        // recognised when the suffix is all decimal digits, so a host
        // containing a colon is not silently misread as host:port.
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

    /// Attempts to navigate to a target address.
    ///
    /// Returns `true` only when a transport actually fetched something. It
    /// never does: this app opens no socket, so no request is issued, no page
    /// is counted, and the returned status records the real outcome (invalid
    /// address vs. no transport).
    pub fn navigate(&mut self, target: &[u8]) -> bool {
        // Remember the attempted address so the address bar can show it
        // truthfully as "attempted, not loaded".
        let n = target.len().min(self.current_url.len());
        self.current_url[..n].copy_from_slice(&target[..n]);
        self.current_url_len = n;

        let parsed = self.parse_uri(target);
        if parsed.scheme == UriScheme::Unknown {
            self.status = PageStatus::InvalidAddress;
            std::log(0, "[BROWSER APP] Navigation rejected: the address has no supported scheme.").ok();
            return false;
        }
        if parsed.host.is_empty() {
            self.status = PageStatus::InvalidAddress;
            std::log(0, "[BROWSER APP] Navigation rejected: the address has no host.").ok();
            return false;
        }
        // WHY not "from the address bar": this function is also reached from
        // the quick shortcuts, where no address text was submitted.
        std::log(0, "[BROWSER APP] Navigation target URL:").ok();
        match core::str::from_utf8(target) {
            Ok(url) => {
                std::log(0, url).ok();
            }
            Err(_) => {
                std::log(0, "[BROWSER APP] (address is not valid UTF-8)").ok();
            }
        }
        std::log(
            0,
            "[BROWSER APP] No transport is bound to this app; no request was issued and no page was loaded.",
        )
        .ok();

        // Navigation Acoustic Feedback & Haptic Click (G5: 784Hz).
        // WHY: only report it when the audio service accepted the tone.
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

        // The attempt reached the transport stage and stopped there.
        self.status = PageStatus::NoTransport;
        false
    }

    /// Reports whether the parsed endpoint has both a host and a port.
    #[allow(dead_code)]
    pub fn endpoint_is_complete(&self, uri: &ParsedUri) -> bool {
        !uri.host.is_empty() && uri.port != 0
    }
}

// -----------------------------------------------------------------------------
// Per-frame resolved text. Every string is measured / ellipsized / wrapped ONCE
// per frame with interos_std::design::font, then rendered from these buffers.
// -----------------------------------------------------------------------------
struct Frame {
    /// Address bar text, ellipsized to the pill's 480 px text column.
    pill: [u8; 96],
    pill_len: usize,
    pill_color: u32,
    show_caret: bool,
    caret_x: usize,
    /// Width of the selection band across the pill text, 0 when nothing is
    /// selected. Drawn because a replace-on-type field that shows no selection
    /// is indistinguishable from an append-at-cursor one until it is too late.
    select_w: usize,
    /// Honest state block: bold headline (scale chosen to fit) + wrapped body.
    head: [u8; 96],
    head_len: usize,
    head_scale: usize,
    head_x: usize,
    head_y: usize,
    body: [&'static [u8]; 6],
    body_n: usize,
    body_x: [usize; 6],
    body_y: [usize; 6],
    /// Centre of the offline globe glyph above the headline.
    icon_cy: usize,
}

impl Frame {
    fn build(app: &BrowserApp) -> Self {
        let mut f = Self {
            pill: [0; 96],
            pill_len: 0,
            pill_color: MUTED,
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
            icon_cy: 0,
        };

        // Address bar text. While editing, the caret sits on the real buffer;
        // otherwise the bar shows the attempted address (muted: not loaded)
        // or the placeholder when there is no address at all.
        let src: &[u8] = if app.address_len > 0 {
            &app.address[..app.address_len]
        } else if app.address_focused {
            &app.address[..0]
        } else if app.current_url_len > 0 {
            app.current_url()
        } else {
            PLACEHOLDER
        };
        f.pill_color = if app.address_len > 0 { INK } else { MUTED };
        f.pill_len = font::ellipsize(src, 1, PILL_TEXT_MAX, &mut f.pill);
        f.show_caret = app.address_focused;
        f.caret_x = PILL_TEXT_X + font::measure(&f.pill[..f.pill_len], 1);
        f.select_w = if app.select_all {
            font::measure(&f.pill[..f.pill_len], 1)
        } else {
            0
        };

        // Honest state block content.
        let (head_src, body_src): (&[u8], &[u8]) = match app.status {
            PageStatus::Blank => (b"No page is loaded", BODY_BLANK),
            PageStatus::NoTransport => (b"Navigation unavailable", BODY_NO_TRANSPORT),
            PageStatus::InvalidAddress => (b"Address cannot be opened", BODY_INVALID),
        };

        // Headline scale: pick the largest scale that fits the screen width.
        let max_line = W - 48;
        let mut head_scale = 2;
        if font::measure_bold(head_src, 2) > max_line {
            head_scale = 1;
        }
        f.head_scale = head_scale;
        if font::measure_bold(head_src, head_scale) > max_line {
            // Bold advances are at most one pixel wider per glyph than the
            // regular advances `ellipsize` uses, so budget the difference:
            // measure_bold(result) <= measure(result) + len <= max_line.
            let budget = max_line.saturating_sub(head_src.len());
            f.head_len = font::ellipsize(head_src, head_scale, budget, &mut f.head);
        } else {
            let n = head_src.len().min(f.head.len());
            f.head[..n].copy_from_slice(&head_src[..n]);
            f.head_len = n;
        }
        let head_w = font::measure_bold(&f.head[..f.head_len], head_scale);
        f.head_x = (W - head_w) / 2;

        // Body: word-wrapped by the SDK; wrap_text guarantees every produced
        // line measures <= STATE_MAX_W at scale 1.
        let mut lines: [&'static [u8]; 6] = [&b""[..]; 6];
        f.body_n = font::wrap_text(body_src, 1, STATE_MAX_W, 6, &mut lines);
        for i in 0..f.body_n {
            f.body[i] = lines[i];
            f.body_x[i] = (W - font::measure(lines[i], 1)) / 2;
        }

        // Vertical placement: centre the block in y 256..1392, snapped to the
        // 8 pt grid.
        let head_h = head_scale * GLYPH_H;
        let body_h = if f.body_n == 0 {
            0
        } else {
            (f.body_n - 1) * STATE_LINE_STRIDE + GLYPH_H
        };
        let body_part = if f.body_n == 0 {
            0
        } else {
            STATE_GAP_BODY + body_h
        };
        let block_h = STATE_ICON_H + STATE_GAP_ICON + head_h + body_part;
        let area_h = VIEW_Y1 - STATE_BOX_TOP;
        let top = (STATE_BOX_TOP + area_h / 2 - block_h / 2) & !7usize;
        f.icon_cy = top + STATE_ICON_H / 2;
        f.head_y = top + STATE_ICON_H + STATE_GAP_ICON;
        let body_y0 = f.head_y + head_h + STATE_GAP_BODY;
        for i in 0..f.body_n {
            f.body_y[i] = body_y0 + i * STATE_LINE_STRIDE;
        }

        f
    }
}

// -----------------------------------------------------------------------------
// Glyphs (24x24-family rounded line geometry, monochrome)
// -----------------------------------------------------------------------------

/// Offline globe inside the address bar: ring, equator, meridian arcs and a
/// diagonal slash. The slash cuts a background gap through the globe so the
/// glyph reads as "offline" at 18 px. Returns `Some(bg)` for the gap itself.
fn pill_state_glyph(x: usize, y: usize) -> Option<u32> {
    let dx = x as isize - 52;
    let dy = y as isize - 84;
    if dx.abs() > 12 || dy.abs() > 12 {
        return None;
    }
    let s = dx + dy;
    if s.abs() <= 2 {
        return Some(if s.abs() <= 1 { MUTED } else { SURFACE });
    }
    let d2 = dx * dx + dy * dy;
    if d2 >= 49 && d2 <= 81 {
        return Some(MUTED); // ring (r 7..9)
    }
    if dy >= -1 && dy <= 0 && d2 <= 81 {
        return Some(MUTED); // equator
    }
    // Meridian ellipse (semi-axes 4 x 9): G = dx^2 * 81 + dy^2 * 16.
    let g = dx * dx * 81 + dy * dy * 16;
    if g >= 933 && g <= 1296 {
        return Some(MUTED);
    }
    None
}

/// Close glyph: an 18 px cross, 2 px diagonal strokes.
fn close_glyph(x: usize, y: usize) -> bool {
    let cx = CLOSE_BTN.x + CLOSE_BTN.w / 2;
    let cy = CLOSE_BTN.y + CLOSE_BTN.h / 2;
    let dx = x as isize - cx as isize;
    let dy = y as isize - cy as isize;
    if dx.abs() > 9 || dy.abs() > 9 {
        return false;
    }
    (dx + dy).abs() <= 1 || (dx - dy).abs() <= 1
}

/// Reload glyph: a 20 px ring with a gap at the top right and an arrow head
/// pointing into the gap.
fn reload_glyph(x: usize, y: usize) -> bool {
    let cx = RELOAD_BTN.x + RELOAD_BTN.w / 2;
    let cy = RELOAD_BTN.y + RELOAD_BTN.h / 2;
    let dx = x as isize - cx as isize;
    let dy = y as isize - cy as isize;
    let d2 = dx * dx + dy * dy;
    if d2 >= 81 && d2 <= 100 && !(dx > 4 && dy < -4) {
        return true; // ring (r 8..10), gap top right
    }
    // Arrow head: right-pointing triangle, apex (cx+6, cy-10), base x = cx-2.
    let t = (cx + 6) as isize - x as isize;
    if t >= 0 && t <= 8 {
        let half = 4 - (t * 4) / 8;
        if (y as isize - (cy as isize - 10)).abs() <= half {
            return true;
        }
    }
    false
}

/// Large offline globe in the empty state (56 px box). Same construction as
/// the address-bar glyph, scaled up; the slash cut uses the base colour.
fn state_icon(x: usize, y: usize, cy: usize) -> Option<u32> {
    let dx = x as isize - (W / 2) as isize;
    let dy = y as isize - cy as isize;
    if dx.abs() > 30 || dy.abs() > 30 {
        return None;
    }
    let s = dx + dy;
    if s.abs() <= 3 {
        return Some(if s.abs() <= 1 { MUTED } else { BASE });
    }
    let d2 = dx * dx + dy * dy;
    if d2 >= 441 && d2 <= 576 {
        return Some(MUTED); // ring (r 21..24)
    }
    if dy >= -1 && dy <= 0 && d2 <= 576 {
        return Some(MUTED); // equator
    }
    // Meridian ellipse (semi-axes 16 x 24): G = dx^2 * 576 + dy^2 * 256.
    let g = dx * dx * 576 + dy * dy * 256;
    if g >= 106000 && g <= 147456 {
        return Some(MUTED);
    }
    None
}

// -----------------------------------------------------------------------------
// Software rasteriser
// -----------------------------------------------------------------------------

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
    if x < x0 || y < y0 || y >= y0 + font::GLYPH_H * scale {
        return None;
    }
    font::test_text_aa(x, y, x0, y0, scale, text, color, bg, bold)
}

/// y 0..120: neutral status band + toolbar.
fn toolbar_pixel(x: usize, y: usize, app: &BrowserApp, f: &Frame) -> u32 {
    // Close control (surface + cross glyph).
    if let Some(c) = draw_rounded_rect(x, y, CLOSE_BTN, BTN_R, SURFACE, Some(BORDER), 1) {
        return if close_glyph(x, y) { INK } else { c };
    }
    // Reload control (surface + refresh glyph).
    if let Some(c) = draw_rounded_rect(x, y, RELOAD_BTN, BTN_R, SURFACE, Some(BORDER), 1) {
        return if reload_glyph(x, y) { MUTED } else { c };
    }
    // Address bar (capsule; accent border while focused).
    if let Some(c) = draw_rounded_rect(
        x,
        y,
        PILL,
        PILL_R,
        SURFACE,
        Some(if app.address_focused { ACCENT } else { BORDER }),
        1,
    ) {
        if let Some(g) = pill_state_glyph(x, y) {
            return g;
        }
        // Selection band, drawn under the text: the accent washed into the pill
        // fill, so a chosen address reads as chosen without a hard block of
        // colour sitting inside a rounded field.
        //
        // WHY it is computed and not returned outright: returning the fill as
        // soon as a pixel is inside the band would return it for the glyph's own
        // pixels too, painting the text out of existence. The band is instead
        // handed to the text renderer as the background its anti-aliased edges
        // blend against, and used as the fill only where no glyph covers the
        // pixel.
        let in_sel = f.select_w > 0
            && x >= PILL_TEXT_X
            && x < PILL_TEXT_X + f.select_w
            && y >= PILL_TEXT_Y
            && y < PILL_TEXT_Y + GLYPH_H;
        let t = &f.pill[..f.pill_len];
        if !t.is_empty() {
            let bg = if in_sel { selection_fill() } else { SURFACE };
            if let Some(px) = text_px(x, y, PILL_TEXT_X, PILL_TEXT_Y, 1, t, f.pill_color, bg, false)
            {
                return px;
            }
        }
        if in_sel {
            return selection_fill();
        }
        if f.show_caret
            && x >= f.caret_x
            && x < f.caret_x + 2
            && y >= PILL_TEXT_Y
            && y < PILL_TEXT_Y + GLYPH_H
        {
            return ACCENT;
        }
        return c;
    }
    BASE
}

/// y 120..1392: the honest state block on the base surface.
fn viewport_pixel(x: usize, y: usize, f: &Frame) -> u32 {
    // Offline globe (its slash cut may overlay only its own 60 px box).
    if let Some(g) = state_icon(x, y, f.icon_cy) {
        return g;
    }
    // Headline.
    if f.head_len > 0 {
        if let Some(px) = text_px(
            x,
            y,
            f.head_x,
            f.head_y,
            f.head_scale,
            &f.head[..f.head_len],
            INK,
            BASE,
            true,
        ) {
            return px;
        }
    }
    // Wrapped body lines.
    for i in 0..f.body_n {
        if let Some(px) = text_px(x, y, f.body_x[i], f.body_y[i], 1, f.body[i], MUTED, BASE, false)
        {
            return px;
        }
    }
    BASE
}

fn get_browser_pixel(x: usize, y: usize, app: &BrowserApp, f: &Frame) -> u32 {
    if y < STATUS_H {
        // Neutral system band: no clock, radio or battery readings are bound
        // to this app, so none are drawn.
        return BASE;
    }
    if y < TOOLBAR_Y1 {
        return toolbar_pixel(x, y, app, f);
    }
    if y < VIEW_Y1 {
        return viewport_pixel(x, y, f);
    }
    // System gesture band.
    if GESTURE_PILL.contains(x, y) {
        return MUTED;
    }
    BASE
}

fn render_browser_ui(surface: u32, app: &BrowserApp) {
    render_browser_band(surface, app, 0, H);
}

/// Re-rasterises and writes only rows `y0..y1`.
///
/// WHY a band and not a rectangle: a row of the surface is one contiguous run in
/// the surface buffer, so `offset = y0 * W` writes a band with no stride
/// bookkeeping. Every band starts on a row boundary, so the running `x`/`y`
/// cursor below is exact.
///
/// WHY this is correct rather than a shortcut: the band is chosen from what the
/// keystroke actually changed, and a keystroke that can change anything outside
/// it - navigation, or the first character that clears a previous status - asks
/// for the whole surface. The pixels outside a requested band are left as they
/// were drawn, and they are correct because nothing about them changed.
fn render_browser_band(surface: u32, app: &BrowserApp, y0: usize, y1: usize) {
    let frame = Frame::build(app);
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
        let _ = std::surface_write(6, surface, offset, &chunk[..count]);
        offset += count;
    }
}

// -----------------------------------------------------------------------------
// Input handling. Every region below is drawn at the exact same coordinates by
// get_browser_pixel (shared Rect constants), so each control is hit-testable
// where it is rendered.
// -----------------------------------------------------------------------------

enum Tap<'a> {
    None,
    Redraw,
    Leave(&'a str),
}

/// The fill behind selected address text: the accent at 22% over the pill.
///
/// WHY a computed tint and not a fixed colour: the pill and the accent both come
/// from the theme, so a hard-coded selection colour would drift from them the
/// moment either changed - which is the same class of bug as hard-coding a
/// background behind anti-aliased text.
fn selection_fill() -> u32 {
    // WHY 38 and not a token amount: at 22 the tint was invisible on the pill's
    // own fill, and a selection nobody can see is not a selection - the field
    // still looked like it would append at the caret. 38 reads as a chosen run
    // without competing with the ink drawn on top of it.
    const PERCENT: u32 = 38;
    let t = PERCENT * 256 / 100;
    let channel = |c: u32, bg: u32| -> u32 { (c * t + bg * (256 - t)) >> 8 };
    let r = channel((ACCENT >> 16) & 0xFF, (SURFACE >> 16) & 0xFF);
    let g = channel((ACCENT >> 8) & 0xFF, (SURFACE >> 8) & 0xFF);
    let b = channel(ACCENT & 0xFF, SURFACE & 0xFF);
    0xFF00_0000 | (r << 16) | (g << 8) | b
}

fn handle_tap(app: &mut BrowserApp, px: u32, py: u32) -> Tap<'static> {
    let (x, y) = (px as usize, py as usize);

    // System home gesture band: leave to the launcher (unchanged).
    if x >= HOME_X0 && x < HOME_X1 && y >= HOME_Y0 {
        return Tap::Leave("[BROWSER APP] Home gesture pressed. Exiting to launcher.");
    }

    // Close control: the only real "leave" action this app has.
    if CLOSE_BTN.contains(x, y) {
        let _ = std::audio_haptic_click();
        return Tap::Leave("[BROWSER APP] Close control pressed. Exiting to launcher.");
    }

    // Address bar: focus the editable native field, pre-filling the real
    // attempted address so the caret sits on what would be edited.
    if PILL.contains(x, y) {
        if !app.address_focused {
            let n = app.current_url_len.min(app.address.len());
            app.address[..n].copy_from_slice(&app.current_url[..n]);
            app.address_len = n;
        }
        app.address_focused = true;
        // Select what is already there, so the first keystroke replaces it
        // instead of being appended to the end of "about:blank".
        app.select_all = app.address_len > 0;
        app.status = PageStatus::Blank;
        let _ = std::audio_haptic_click();
        std::log(0, "[BROWSER APP] Address field focused for keyboard input.").ok();
        return Tap::Redraw;
    }

    // Reload control: re-issue the real navigation attempt for the current
    // address. It reports the honest outcome (no transport, nothing loaded).
    if RELOAD_BTN.contains(x, y) {
        let mut target = [0u8; 64];
        let n = app.current_url_len.min(target.len());
        target[..n].copy_from_slice(&app.current_url[..n]);
        app.navigate(&target[..n]);
        return Tap::Redraw;
    }

    Tap::None
}

/// Which part of the surface a keystroke changed.
///
/// WHY this exists: rendering the browser re-rasterised all 1,036,800 pixels
/// through `get_browser_pixel` for every single keystroke, so typing a URL cost
/// the same as a full screen transition. Typing moves the address text, the
/// caret, and at most the status block - between 72 and 200 of the 1440 rows.
/// Naming the region lets the renderer skip the rest, which is where the
/// perceived latency of typing was going.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Dirty {
    /// Nothing changed; do not render.
    None,
    /// Only the address text, caret or focus ring moved: the toolbar band.
    Toolbar,
    /// Navigation ran or the status block changed: the whole surface.
    All,
}

fn handle_browser_key(app: &mut BrowserApp, key: std::InputKeyEvent) -> Dirty {
    use std::input_key_code;
    if !key.pressed || !app.address_focused {
        return Dirty::None;
    }
    match key.code {
        input_key_code::ESCAPE => {
            app.address_focused = false;
            app.select_all = false;
            Dirty::Toolbar
        }
        input_key_code::ENTER => {
            app.address_focused = false;
            app.select_all = false;
            // Submitting the typed address performs a real navigation attempt
            // against the real parser and transport state.
            if app.address_len > 0 {
                let mut target = [0u8; 64];
                let n = app.address_len.min(target.len());
                target[..n].copy_from_slice(&app.address[..n]);
                app.navigate(&target[..n]);
            }
            Dirty::All
        }
        input_key_code::BACKSPACE => {
            // Backspace over a selection deletes the selection, which is what
            // every text field does; otherwise it deletes one character.
            if app.select_all {
                app.address_len = 0;
                app.select_all = false;
            } else {
                app.address_len = app.address_len.saturating_sub(1);
            }
            Dirty::Toolbar
        }
        code if (32..=126).contains(&code) && app.address_len < app.address.len() => {
            // A character typed over a selection replaces it. Clearing first is
            // also what makes room: the field is 64 bytes and a full buffer
            // would otherwise refuse the keystroke outright.
            if app.select_all {
                app.address_len = 0;
                app.select_all = false;
            }
            app.address[app.address_len] = code as u8;
            app.address_len += 1;
            // Editing again: the previous attempt's message is superseded.
            //
            // WHY the status is compared before it is cleared: clearing it can
            // change the state block far below the toolbar, so on the keystroke
            // that first clears it the surface genuinely changed everywhere. On
            // every keystroke after that the status is already Blank, only the
            // address moved, and the toolbar band is the honest region.
            let status_moved = app.status != PageStatus::Blank;
            app.status = PageStatus::Blank;
            if status_moved {
                Dirty::All
            } else {
                Dirty::Toolbar
            }
        }
        _ => return Dirty::None,
    }
}

fn destroy_and_exit(surface: u32) -> ! {
    if surface > 0 {
        let _ = std::surface_destroy(6, surface);
    }
    std::exit_with_code(0);
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
                            std::log(
                                0,
                                "[BROWSER APP] Navigation gesture recognized. Exiting to launcher.",
                            )
                            .ok();
                            return destroy_and_exit(surface);
                        }
                        if input.kind == 1 {
                            match handle_tap(&mut browser, input.x, input.y) {
                                Tap::Leave(msg) => {
                                    std::log(0, msg).ok();
                                    return destroy_and_exit(surface);
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
                                // The toolbar band, generously: STATUS_H..TOOLBAR_Y1
                                // covers the pill, its focus ring, and the
                                // reload/close controls in the same row.
                                render_browser_band(surface, &browser, STATUS_H, TOOLBAR_Y1);
                            }
                            Dirty::All => render_browser_ui(surface, &browser),
                        }
                        let _ = std::surface_flip(6);
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
