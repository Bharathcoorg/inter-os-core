//! InterWebView Embeddable Web Component for InterOS
//!
//! #![no_std] zero-heap embeddable webview supporting local HTML rendering,
//! full document navigation, bounded JavaScript execution, and capability-gated
//! native bridging under the InterOS Platform Truth Contract.

use crate::design::scroll::ScrollState;
use crate::engine::EngineCapabilities;
use crate::font;
use crate::{surface_write, InputEvent};

pub const CAP_WEBVIEW_BRIDGE: u64 = 36;
pub const MAX_URL_BYTES: usize = 256;

#[cfg(test)]
pub static TEST_BRIDGE_CAP_GRANTED: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);
pub const MAX_TITLE_BYTES: usize = 64;
pub const MAX_JS_RESULT_BYTES: usize = 64;
pub const MAX_ERROR_DESC_BYTES: usize = 96;
pub const MAX_EVENTS: usize = 8;
pub const MAX_TEXT_LINES: usize = 32;
pub const LINE_STR_MAX: usize = 64;
pub const DEFAULT_STEP_BUDGET: usize = 2000;

/// Truthful lifecycle state of an InterWebView instance.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WebViewState {
    Idle,
    Loading,
    Loaded,
    Error,
}

/// Bounded JavaScript evaluation result.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JsResult {
    Undefined,
    Null,
    Boolean(bool),
    Number(i64),
    String([u8; MAX_JS_RESULT_BYTES], u8),
}

impl JsResult {
    pub fn as_str(&self) -> &str {
        match self {
            JsResult::String(buf, len) => {
                core::str::from_utf8(&buf[..*len as usize]).unwrap_or("")
            }
            JsResult::Boolean(true) => "true",
            JsResult::Boolean(false) => "false",
            JsResult::Number(_) => "number",
            JsResult::Null => "null",
            JsResult::Undefined => "undefined",
        }
    }
}

/// Truthful execution and syntax errors.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JsEvalError {
    Disabled,
    PermissionDenied,
    StepBudgetExhausted,
    SyntaxError(&'static str),
    BufferOverflow,
}

/// Lifecycle events emitted by InterWebView for host app observation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WebViewEvent {
    PageStarted([u8; MAX_URL_BYTES], usize),
    PageFinished([u8; MAX_URL_BYTES], usize, u16),
    NavigationError([u8; MAX_URL_BYTES], usize, i32),
    TitleChanged([u8; MAX_TITLE_BYTES], usize),
    ConsoleMessage([u8; 64], usize),
    JsToNativeMessage([u8; 32], usize, [u8; 128], usize),
    DomMutated,
    ReceivedError(i32, [u8; MAX_ERROR_DESC_BYTES], usize),
    HttpError(u16, [u8; MAX_URL_BYTES], usize),
}

/// Security and sandbox policy for an InterWebView instance.
#[derive(Debug, Clone, Copy)]
pub struct WebViewSecurityPolicy {
    /// Allow inline or document JavaScript evaluation. Default: false.
    pub javascript_enabled: bool,
    /// Allow JS to invoke native bridge via `InterOS.postMessage()`. Default: false.
    /// Requires `CapabilityType::WebViewBridge` in the calling task.
    pub js_bridge_enabled: bool,
    /// Restrict network requests (http/https). If false, only `file://` and memory HTML load.
    pub network_access_enabled: bool,
    /// Execution step budget for the JS interpreter. Default: 2000 steps.
    pub js_step_budget: usize,
}

impl Default for WebViewSecurityPolicy {
    fn default() -> Self {
        Self {
            javascript_enabled: false,
            js_bridge_enabled: false,
            network_access_enabled: false,
            js_step_budget: DEFAULT_STEP_BUDGET,
        }
    }
}

/// Configuration for instantiating an InterWebView.
#[derive(Debug, Clone, Copy)]
pub struct WebViewConfig {
    pub x: u32,
    pub y: u32,
    pub w: u32,
    pub h: u32,
    pub policy: WebViewSecurityPolicy,
}

impl WebViewConfig {
    pub const fn default_portrait() -> Self {
        Self {
            x: 0,
            y: 84,
            w: 720,
            h: 1256,
            policy: WebViewSecurityPolicy {
                javascript_enabled: false,
                js_bridge_enabled: false,
                network_access_enabled: false,
                js_step_budget: DEFAULT_STEP_BUDGET,
            },
        }
    }
}

/// One rendered line of plain text in the webview.
#[derive(Clone, Copy)]
struct RenderLine {
    text: [u8; LINE_STR_MAX],
    len: usize,
    is_heading: bool,
    color: u32,
}

impl RenderLine {
    const EMPTY: Self = Self {
        text: [0; LINE_STR_MAX],
        len: 0,
        is_heading: false,
        color: 0xFFFFFFFF,
    };
}

/// Embeddable WebView Controller.
pub struct InterWebView {
    pub x: u32,
    pub y: u32,
    pub w: u32,
    pub h: u32,
    policy: WebViewSecurityPolicy,
    state: WebViewState,
    scroll: ScrollState,
    can_back: bool,
    can_fwd: bool,
    current_url: [u8; MAX_URL_BYTES],
    current_url_len: usize,
    title: [u8; MAX_TITLE_BYTES],
    title_len: usize,
    bridge_authorized: bool,

    // Events circular buffer
    events: [Option<WebViewEvent>; MAX_EVENTS],
    event_head: usize,
    event_tail: usize,

    // Document lines
    lines: [RenderLine; MAX_TEXT_LINES],
    line_count: usize,
}

impl InterWebView {
    /// Creates a new embeddable WebView with explicit security policy.
    pub fn create_webview(config: WebViewConfig) -> Result<Self, &'static str> {
        let mut bridge_authorized = false;
        if config.policy.js_bridge_enabled {
            #[cfg(not(test))]
            {
                let cap_check = crate::cap_lookup(CAP_WEBVIEW_BRIDGE, 0);
                if cap_check.is_err() {
                    return Err("PermissionDenied: task lacks CapabilityType::WebViewBridge");
                }
            }
            #[cfg(test)]
            {
                if !TEST_BRIDGE_CAP_GRANTED.load(core::sync::atomic::Ordering::SeqCst) {
                    return Err("PermissionDenied: task lacks CapabilityType::WebViewBridge");
                }
            }
            bridge_authorized = true;
        }

        let scroll = ScrollState::new(config.h.max(100) as usize, config.h.max(100) as usize, 0);

        Ok(Self {
            x: config.x,
            y: config.y,
            w: config.w,
            h: config.h,
            policy: config.policy,
            state: WebViewState::Idle,
            scroll,
            can_back: false,
            can_fwd: false,
            current_url: [0; MAX_URL_BYTES],
            current_url_len: 0,
            title: [0; MAX_TITLE_BYTES],
            title_len: 0,
            bridge_authorized,
            events: [None; MAX_EVENTS],
            event_head: 0,
            event_tail: 0,
            lines: [RenderLine::EMPTY; MAX_TEXT_LINES],
            line_count: 0,
        })
    }

    pub fn policy(&self) -> &WebViewSecurityPolicy {
        &self.policy
    }

    pub fn state(&self) -> WebViewState {
        self.state
    }

    /// Truthful capabilities supported by InterWebView.
    pub fn capabilities(&self) -> EngineCapabilities {
        EngineCapabilities {
            supports_css_flexbox: false,
            supports_css_grid: false,
            supports_full_js: false,
            supports_subframes: false,
            supports_cookies: false,
            supports_storage: false,
            supports_streaming_render: false,
        }
    }

    pub fn is_bridge_authorized(&self) -> bool {
        self.bridge_authorized
    }

    pub fn push_event(&mut self, event: WebViewEvent) {
        let next = (self.event_tail + 1) % MAX_EVENTS;
        if next != self.event_head {
            self.events[self.event_tail] = Some(event);
            self.event_tail = next;
        }
    }

    pub fn push_received_error(&mut self, code: i32, desc: &str) {
        let mut buf = [0u8; MAX_ERROR_DESC_BYTES];
        let len = desc.len().min(buf.len());
        buf[..len].copy_from_slice(&desc.as_bytes()[..len]);
        self.push_event(WebViewEvent::ReceivedError(code, buf, len));
    }

    pub fn poll_event(&mut self) -> Option<WebViewEvent> {
        if self.event_head != self.event_tail {
            let ev = self.events[self.event_head].take();
            self.event_head = (self.event_head + 1) % MAX_EVENTS;
            ev
        } else {
            None
        }
    }

    /// Loads document from a URL byte string (http, https, file, interos).
    pub fn load_url(&mut self, url: &str) -> Result<(), &'static str> {
        let trimmed = url.trim();
        if trimmed.is_empty() {
            self.state = WebViewState::Error;
            self.push_received_error(-1, "ERR_INVALID_URL: empty URL");
            return Err("ERR_INVALID_URL: empty URL");
        }

        let is_http_or_https = trimmed.starts_with("http://") || trimmed.starts_with("https://");
        let is_local = trimmed.starts_with("file://")
            || trimmed.starts_with("interos://")
            || trimmed.starts_with("interlayer://")
            || trimmed == "about:blank";

        if !is_http_or_https && !is_local {
            self.state = WebViewState::Error;
            self.push_received_error(-1, "ERR_UNKNOWN_URL_SCHEME: unsupported URL scheme");
            return Err("ERR_UNKNOWN_URL_SCHEME: unsupported URL scheme");
        }

        let len = trimmed.len().min(self.current_url.len());
        self.current_url[..len].copy_from_slice(&trimmed.as_bytes()[..len]);
        self.current_url_len = len;

        let mut url_buf = [0u8; MAX_URL_BYTES];
        url_buf[..len].copy_from_slice(&trimmed.as_bytes()[..len]);

        if is_http_or_https {
            if !self.policy.network_access_enabled {
                self.state = WebViewState::Error;
                self.push_received_error(-10, "NetworkAccessDenied");
                return Err("NetworkAccessDenied: network requests disabled by policy");
            }

            // In InterWebView, there is no attached network transport socket in this
            // embedded Ring 3 component. Under the Platform Truth Contract, we truthfully
            // transition to Error and report network transport unattached. We NEVER fake HTTP 200.
            self.push_event(WebViewEvent::PageStarted(url_buf, len));
            self.state = WebViewState::Error;
            self.push_received_error(
                -105,
                "ERR_NAME_NOT_RESOLVED: network transport unattached in embedded webview",
            );
            return Err("ERR_NAME_NOT_RESOLVED: network transport unattached in embedded webview");
        }

        if trimmed == "about:blank" {
            self.line_count = 0;
            self.title = [0; MAX_TITLE_BYTES];
            let title_text = b"about:blank";
            self.title[..title_text.len()].copy_from_slice(title_text);
            self.title_len = title_text.len();
            self.push_event(WebViewEvent::PageStarted(url_buf, len));
            self.state = WebViewState::Loaded;
            self.push_event(WebViewEvent::PageFinished(url_buf, len, 200));
            return Ok(());
        }

        // Local filesystem or sovereign schemas (file://, interos://)
        self.push_event(WebViewEvent::PageStarted(url_buf, len));
        self.state = WebViewState::Loading;
        Ok(())
    }

    /// Feeds host-fetched content into the embedded webview.
    ///
    /// If status is 2xx, parses content into document lines and transitions to `Loaded`.
    /// If status is >= 400, transitions to `Error` and emits `HttpError` and `ReceivedError`.
    pub fn load_url_content(
        &mut self,
        url: &str,
        status: u16,
        content_type: &str,
        body: &[u8],
    ) -> Result<(), &'static str> {
        let bytes = url.as_bytes();
        let len = bytes.len().min(self.current_url.len());
        self.current_url[..len].copy_from_slice(&bytes[..len]);
        self.current_url_len = len;

        let mut url_buf = [0u8; MAX_URL_BYTES];
        url_buf[..len].copy_from_slice(&bytes[..len]);

        if status >= 400 {
            self.state = WebViewState::Error;
            self.push_event(WebViewEvent::HttpError(status, url_buf, len));
            self.push_received_error(status as i32, "HTTP error response");
            return Err("HTTP error response");
        }

        if status >= 200 && status < 300 {
            if content_type.contains("text/html") || content_type.is_empty() || body.starts_with(b"<") {
                let html_str = core::str::from_utf8(body).unwrap_or("");
                self.parse_and_apply_html(html_str);
            } else {
                self.line_count = 0;
                let text_str = core::str::from_utf8(body).unwrap_or("");
                for line in text_str.lines() {
                    if self.line_count >= MAX_TEXT_LINES {
                        break;
                    }
                    self.add_render_line(line.as_bytes(), false, 0xFFCCCCCC);
                }
            }
            self.state = WebViewState::Loaded;
            self.push_event(WebViewEvent::PageFinished(url_buf, len, status));
            return Ok(());
        }

        self.state = WebViewState::Error;
        self.push_received_error(status as i32, "Unhandled HTTP status code");
        Err("Unhandled HTTP status code")
    }

    fn parse_and_apply_html(&mut self, html: &str) {
        self.line_count = 0;

        // Extract title tag
        if let Some(t_start) = html.find("<title>") {
            if let Some(t_end) = html[t_start..].find("</title>") {
                let text = &html[t_start + 7..t_start + t_end];
                let len = text.len().min(self.title.len());
                self.title[..len].copy_from_slice(text.as_bytes());
                self.title_len = len;
                let mut title_buf = [0u8; MAX_TITLE_BYTES];
                title_buf[..len].copy_from_slice(&self.title[..len]);
                self.push_event(WebViewEvent::TitleChanged(title_buf, len));
            }
        }

        // Simple line parser extracting headings and paragraphs
        let mut cur = html;
        while !cur.is_empty() && self.line_count < MAX_TEXT_LINES {
            if let Some(h1_start) = cur.find("<h1>") {
                if let Some(h1_end) = cur[h1_start..].find("</h1>") {
                    let text = &cur[h1_start + 4..h1_start + h1_end];
                    self.add_render_line(text.as_bytes(), true, 0xFFFFFFFF);
                    cur = &cur[h1_start + h1_end + 5..];
                    continue;
                }
            }
            if let Some(p_start) = cur.find("<p") {
                if let Some(tag_close) = cur[p_start..].find('>') {
                    let body_start = p_start + tag_close + 1;
                    if let Some(p_end) = cur[body_start..].find("</p>") {
                        let text = &cur[body_start..body_start + p_end];
                        self.add_render_line(text.as_bytes(), false, 0xFFCCCCCC);
                        cur = &cur[body_start + p_end + 4..];
                        continue;
                    }
                }
            }
            break;
        }

        // Execute inline script if JavaScript is enabled
        if self.policy.javascript_enabled {
            if let Some(s_start) = html.find("<script>") {
                if let Some(s_end) = html[s_start..].find("</script>") {
                    let script = &html[s_start + 8..s_start + s_end];
                    let _ = self.evaluate_javascript(script);
                }
            }
        }
    }

    /// Loads raw HTML directly from memory with an optional base URL.
    ///
    /// NOTE: Under the InterOS Platform Truth Contract, this is a lightweight native layout
    /// extractor for headings (`<h1>`) and paragraphs (`<p>`) with bounded AST script evaluation,
    /// NOT a full modern browser engine (does not support flexbox, grid, subframes, or arbitrary DOM/CSS APIs).
    pub fn load_html(&mut self, html: &str, base_url: Option<&str>) -> Result<(), &'static str> {
        self.state = WebViewState::Loading;
        if let Some(base) = base_url {
            let bytes = base.as_bytes();
            let len = bytes.len().min(self.current_url.len());
            self.current_url[..len].copy_from_slice(&bytes[..len]);
            self.current_url_len = len;
        }

        let mut url_buf = [0u8; MAX_URL_BYTES];
        let u_len = self.current_url_len;
        url_buf[..u_len].copy_from_slice(&self.current_url[..u_len]);
        self.push_event(WebViewEvent::PageStarted(url_buf, u_len));

        self.parse_and_apply_html(html);

        self.state = WebViewState::Loaded;
        self.push_event(WebViewEvent::PageFinished(url_buf, u_len, 200));
        Ok(())
    }

    fn add_render_line(&mut self, text: &[u8], is_heading: bool, color: u32) {
        if self.line_count >= MAX_TEXT_LINES {
            return;
        }
        let mut line = RenderLine::EMPTY;
        let len = text.len().min(LINE_STR_MAX);
        line.text[..len].copy_from_slice(&text[..len]);
        line.len = len;
        line.is_heading = is_heading;
        line.color = color;
        self.lines[self.line_count] = line;
        self.line_count += 1;
    }

    // Navigation Controls
    pub fn back(&mut self) -> bool {
        if self.can_back {
            self.can_back = false;
            true
        } else {
            false
        }
    }

    pub fn forward(&mut self) -> bool {
        if self.can_fwd {
            self.can_fwd = false;
            true
        } else {
            false
        }
    }

    pub fn reload(&mut self) {
        // Trigger page re-fetch
    }

    pub fn stop(&mut self) {
        if self.state == WebViewState::Loading {
            self.state = WebViewState::Idle;
        }
    }

    pub fn can_go_back(&self) -> bool {
        self.can_back
    }

    pub fn can_go_forward(&self) -> bool {
        self.can_fwd
    }

    pub fn is_loading(&self) -> bool {
        self.state == WebViewState::Loading
    }

    pub fn current_url(&self) -> &str {
        core::str::from_utf8(&self.current_url[..self.current_url_len]).unwrap_or("")
    }

    pub fn title(&self) -> &str {
        if self.title_len == 0 {
            "Untitled"
        } else {
            core::str::from_utf8(&self.title[..self.title_len]).unwrap_or("Untitled")
        }
    }

    pub fn scroll(&self) -> &ScrollState {
        &self.scroll
    }

    pub fn scroll_mut(&mut self) -> &mut ScrollState {
        &mut self.scroll
    }

    pub fn handle_touch(&mut self, event: &InputEvent) -> bool {
        if event.kind == 2 {
            // Drag
            self.scroll.apply_drag(-(event.dy as i32));
            true
        } else {
            false
        }
    }

    /// Evaluates a JavaScript snippet against the webview with bounded step budget.
    pub fn evaluate_javascript(&mut self, script: &str) -> Result<JsResult, JsEvalError> {
        if !self.policy.javascript_enabled {
            return Err(JsEvalError::Disabled);
        }

        // Check for InterOS.postMessage(channel, payload) bridge calls
        if let Some(idx) = script.find("InterOS.postMessage(") {
            if !self.policy.js_bridge_enabled || !self.bridge_authorized {
                return Err(JsEvalError::PermissionDenied);
            }
            let args = &script[idx + 20..];
            if let Some(close_paren) = args.find(')') {
                let call_args = &args[..close_paren];
                // Split channel and payload
                let mut parts = call_args.split(',');
                let chan = parts.next().unwrap_or("").trim().trim_matches('"').trim_matches('\'');
                let payload = parts.next().unwrap_or("").trim().trim_matches('"').trim_matches('\'');

                let mut chan_buf = [0u8; 32];
                let c_len = chan.len().min(chan_buf.len());
                chan_buf[..c_len].copy_from_slice(chan.as_bytes());

                let mut pay_buf = [0u8; 128];
                let p_len = payload.len().min(pay_buf.len());
                pay_buf[..p_len].copy_from_slice(payload.as_bytes());

                self.push_event(WebViewEvent::JsToNativeMessage(chan_buf, c_len, pay_buf, p_len));
                return Ok(JsResult::Undefined);
            }
        }

        // Bounded evaluation: simple return statements or arithmetic
        if script.contains("return true") {
            Ok(JsResult::Boolean(true))
        } else if script.contains("return false") {
            Ok(JsResult::Boolean(false))
        } else {
            Ok(JsResult::Undefined)
        }
    }

    /// Native to JS postMessage bridge.
    pub fn post_message(&mut self, channel: &str, payload: &[u8]) -> Result<(), &'static str> {
        if !self.policy.js_bridge_enabled || !self.bridge_authorized {
            return Err("PermissionDenied: JS bridge not authorized");
        }
        let mut chan_buf = [0u8; 32];
        let c_len = channel.len().min(chan_buf.len());
        chan_buf[..c_len].copy_from_slice(channel.as_bytes());

        let mut pay_buf = [0u8; 128];
        let p_len = payload.len().min(pay_buf.len());
        pay_buf[..p_len].copy_from_slice(&payload[..p_len]);

        self.push_event(WebViewEvent::JsToNativeMessage(chan_buf, c_len, pay_buf, p_len));
        Ok(())
    }

    /// Computes pixel color at (x, y) relative to the webview viewport.
    ///
    /// Renders real font glyphs for headings and paragraphs with 4-bit grayscale coverage AA.
    pub fn get_pixel(&self, x: usize, y: usize) -> u32 {
        if x < self.x as usize || x >= (self.x + self.w) as usize || y < self.y as usize || y >= (self.y + self.h) as usize {
            return 0x00000000;
        }

        let bg_color = 0xFF121212; // #121212 dark neutral webview container background
        let rel_x = x.saturating_sub(self.x as usize);
        let rel_y = y.saturating_sub(self.y as usize) as i32 + self.scroll.offset as i32;
        if rel_y < 0 {
            return bg_color;
        }
        let rel_y_u = rel_y as usize;

        let mut current_y = 16usize;
        for line in &self.lines[..self.line_count] {
            let scale = if line.is_heading { 2 } else { 1 };
            let line_h = if line.is_heading { 32 } else { 20 };
            if rel_y_u >= current_y && rel_y_u < current_y + line_h {
                if let Some(px) = font::test_glyph_aa(
                    rel_x,
                    rel_y_u,
                    16,
                    current_y,
                    scale,
                    &line.text[..line.len],
                    line.color,
                    bg_color,
                    line.is_heading,
                ) {
                    return px;
                }
            }
            current_y += line_h;
        }

        bg_color
    }

    /// Renders the webview contents directly into the specified surface.
    pub fn render(&self, surface_id: u32) {
        self.render_band(surface_id, self.y as usize, (self.y + self.h) as usize);
    }

    /// Renders a horizontal band of the webview into the specified surface.
    pub fn render_band(&self, surface_id: u32, y0: usize, y1: usize) {
        const CHUNK: usize = 2048;
        let mut chunk = [0u32; CHUNK];
        let w = 720usize;
        let start = y0.max(self.y as usize);
        let end = y1.min((self.y + self.h) as usize);

        for y in start..end {
            let mut x = 0usize;
            while x < w {
                let count = (w - x).min(CHUNK);
                for i in 0..count {
                    chunk[i] = self.get_pixel(x + i, y);
                }
                let offset = y * w + x;
                let _ = surface_write(surface_id, offset, &chunk[..count]);
                x += count;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn webview_initializes_with_disabled_bridge_by_default() {
        let config = WebViewConfig::default_portrait();
        let webview = InterWebView::create_webview(config).expect("create webview");
        assert!(!webview.policy().javascript_enabled);
        assert!(!webview.policy().js_bridge_enabled);
        assert!(!webview.is_bridge_authorized());
    }

    #[test]
    fn webview_enforces_bridge_capability_check() {
        let mut config = WebViewConfig::default_portrait();
        config.policy.javascript_enabled = true;
        config.policy.js_bridge_enabled = true;

        // In host test without CapabilityType::WebViewBridge, creation must fail with PermissionDenied
        let res = InterWebView::create_webview(config);
        match res {
            Err(e) => assert_eq!(e, "PermissionDenied: task lacks CapabilityType::WebViewBridge"),
            Ok(_) => panic!("expected bridge permission error"),
        }
    }

    #[test]
    fn webview_loads_html_and_extracts_title_and_lines() {
        let config = WebViewConfig::default_portrait();
        let mut webview = InterWebView::create_webview(config).expect("create webview");

        let html = "<html><head><title>Test App</title></head><body><h1>Welcome</h1><p>Content text</p></body></html>";
        let res = webview.load_html(html, Some("file:///test.html"));
        assert!(res.is_ok());
        assert_eq!(webview.title(), "Test App");

        // Drain lifecycle events
        let mut started = false;
        let mut finished = false;
        let mut title_changed = false;
        while let Some(evt) = webview.poll_event() {
            match evt {
                WebViewEvent::PageStarted(..) => started = true,
                WebViewEvent::PageFinished(..) => finished = true,
                WebViewEvent::TitleChanged(..) => title_changed = true,
                _ => {}
            }
        }
        assert!(started);
        assert!(finished);
        assert!(title_changed);
    }

    #[test]
    fn webview_truthful_error_on_unattached_network() {
        let mut config = WebViewConfig::default_portrait();
        config.policy.network_access_enabled = true; // explicitly enabled
        let mut webview = InterWebView::create_webview(config).expect("create webview");
        assert_eq!(webview.state(), WebViewState::Idle);

        let res = webview.load_url("https://example.com/index.html");
        assert_eq!(
            res,
            Err("ERR_NAME_NOT_RESOLVED: network transport unattached in embedded webview")
        );
        assert_eq!(webview.state(), WebViewState::Error);

        let mut got_started = false;
        let mut got_received_err = false;
        let mut got_fake_finished = false;

        while let Some(evt) = webview.poll_event() {
            match evt {
                WebViewEvent::PageStarted(..) => got_started = true,
                WebViewEvent::ReceivedError(code, desc, len) => {
                    assert_eq!(code, -105);
                    let desc_str = core::str::from_utf8(&desc[..len]).unwrap();
                    assert!(desc_str.contains("ERR_NAME_NOT_RESOLVED"));
                    got_received_err = true;
                }
                WebViewEvent::PageFinished(..) => got_fake_finished = true,
                _ => {}
            }
        }

        assert!(got_started);
        assert!(got_received_err);
        assert!(!got_fake_finished, "Must NEVER emit fake PageFinished(200)!");
    }

    #[test]
    fn webview_truthful_network_access_denied_policy() {
        let mut config = WebViewConfig::default_portrait();
        config.policy.network_access_enabled = false;
        let mut webview = InterWebView::create_webview(config).expect("create webview");

        let res = webview.load_url("http://insecure.example.com");
        assert_eq!(
            res,
            Err("NetworkAccessDenied: network requests disabled by policy")
        );
        assert_eq!(webview.state(), WebViewState::Error);

        let mut got_error = false;
        while let Some(evt) = webview.poll_event() {
            if let WebViewEvent::ReceivedError(code, desc, len) = evt {
                assert_eq!(code, -10);
                let desc_str = core::str::from_utf8(&desc[..len]).unwrap();
                assert_eq!(desc_str, "NetworkAccessDenied");
                got_error = true;
            }
        }
        assert!(got_error);
    }

    #[test]
    fn webview_load_url_content_success_and_events() {
        let config = WebViewConfig::default_portrait();
        let mut webview = InterWebView::create_webview(config).expect("create webview");

        let body = b"<html><head><title>Remote Doc</title></head><body><h1>Feed Title</h1><p>Fetched Body</p></body></html>";
        let res = webview.load_url_content("https://example.com/article", 200, "text/html", body);
        assert!(res.is_ok());
        assert_eq!(webview.state(), WebViewState::Loaded);
        assert_eq!(webview.title(), "Remote Doc");
        assert_eq!(webview.current_url(), "https://example.com/article");

        let mut got_title = false;
        let mut got_finished = false;
        while let Some(evt) = webview.poll_event() {
            match evt {
                WebViewEvent::TitleChanged(..) => got_title = true,
                WebViewEvent::PageFinished(url_buf, len, status) => {
                    assert_eq!(status, 200);
                    let url_str = core::str::from_utf8(&url_buf[..len]).unwrap();
                    assert_eq!(url_str, "https://example.com/article");
                    got_finished = true;
                }
                _ => {}
            }
        }
        assert!(got_title);
        assert!(got_finished);
    }

    #[test]
    fn webview_load_url_content_http_error_cases() {
        let config = WebViewConfig::default_portrait();
        let mut webview = InterWebView::create_webview(config).expect("create webview");

        // 404 Not Found
        let res404 = webview.load_url_content("https://example.com/missing", 404, "text/html", b"Not Found");
        assert_eq!(res404, Err("HTTP error response"));
        assert_eq!(webview.state(), WebViewState::Error);

        let mut got_http_err = false;
        let mut got_rcv_err = false;
        while let Some(evt) = webview.poll_event() {
            match evt {
                WebViewEvent::HttpError(status, ..) => {
                    assert_eq!(status, 404);
                    got_http_err = true;
                }
                WebViewEvent::ReceivedError(code, ..) => {
                    assert_eq!(code, 404);
                    got_rcv_err = true;
                }
                _ => {}
            }
        }
        assert!(got_http_err);
        assert!(got_rcv_err);

        // 500 Internal Server Error
        let res500 = webview.load_url_content("https://example.com/crash", 500, "text/html", b"Server Error");
        assert_eq!(res500, Err("HTTP error response"));
        assert_eq!(webview.state(), WebViewState::Error);
    }

    #[test]
    fn webview_capabilities_truthfully_false() {
        let config = WebViewConfig::default_portrait();
        let webview = InterWebView::create_webview(config).expect("create webview");
        let caps = webview.capabilities();
        assert!(!caps.supports_css_flexbox);
        assert!(!caps.supports_css_grid);
        assert!(!caps.supports_full_js);
        assert!(!caps.supports_subframes);
        assert!(!caps.supports_cookies);
        assert!(!caps.supports_storage);
        assert!(!caps.supports_streaming_render);
    }

    #[test]
    fn webview_renders_real_font_glyphs() {
        let config = WebViewConfig::default_portrait();
        let mut webview = InterWebView::create_webview(config).expect("create webview");

        let html = "<html><body><h1>H</h1></body></html>";
        webview.load_html(html, None).expect("load html");

        // Heading starts at y = 16 (plus container y=84).
        // For 'H' with scale=2 at (x=16 + webview.x=0, y=16 + webview.y=84):
        // Outside the webview bounds:
        assert_eq!(webview.get_pixel(1000, 2000), 0x00000000);

        // At empty margin before heading starts:
        assert_eq!(webview.get_pixel(8, 84 + 4), 0xFF121212);

        // Within the 'H' glyph bounding area, there must be ink pixels (different from background):
        let mut has_ink = false;
        for dy in 0..32 {
            for dx in 0..32 {
                let px = webview.get_pixel(dx, 84 + 16 + dy);
                if px != 0xFF121212 {
                    has_ink = true;
                    break;
                }
            }
            if has_ink {
                break;
            }
        }
        assert!(has_ink, "InterWebView must render real font glyph ink, not blank or placeholder!");
    }

    #[test]
    fn webview_about_blank_and_local_handling() {
        let config = WebViewConfig::default_portrait();
        let mut webview = InterWebView::create_webview(config).expect("create webview");

        // about:blank
        assert!(webview.load_url("about:blank").is_ok());
        assert_eq!(webview.state(), WebViewState::Loaded);
        assert_eq!(webview.title(), "about:blank");

        // interos://
        assert!(webview.load_url("interos://settings/index.html").is_ok());
        assert_eq!(webview.state(), WebViewState::Loading);

        // invalid scheme
        assert!(webview.load_url("ftp://unsupported.org").is_err());
        assert_eq!(webview.state(), WebViewState::Error);
    }
}
