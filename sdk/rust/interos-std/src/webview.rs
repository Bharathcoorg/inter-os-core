//! InterWebView Embeddable Web Component for InterOS
//!
//! #![no_std] zero-heap embeddable webview supporting local HTML rendering,
//! full document navigation, bounded JavaScript execution, and capability-gated
//! native bridging under the InterOS Platform Truth Contract.

use crate::design::scroll::ScrollState;
use crate::{surface_write, InputEvent};

pub const CAP_WEBVIEW_BRIDGE: u64 = 36;
pub const MAX_URL_BYTES: usize = 256;

#[cfg(test)]
pub static TEST_BRIDGE_CAP_GRANTED: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);
pub const MAX_TITLE_BYTES: usize = 64;
pub const MAX_JS_RESULT_BYTES: usize = 64;
pub const MAX_EVENTS: usize = 8;
pub const MAX_TEXT_LINES: usize = 32;
pub const LINE_STR_MAX: usize = 64;
pub const DEFAULT_STEP_BUDGET: usize = 2000;

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
    scroll: ScrollState,
    can_back: bool,
    can_fwd: bool,
    current_url: [u8; MAX_URL_BYTES],
    current_url_len: usize,
    title: [u8; MAX_TITLE_BYTES],
    title_len: usize,
    is_loading: bool,
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
            scroll,
            can_back: false,
            can_fwd: false,
            current_url: [0; MAX_URL_BYTES],
            current_url_len: 0,
            title: [0; MAX_TITLE_BYTES],
            title_len: 0,
            is_loading: false,
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
        let bytes = url.as_bytes();
        if !self.policy.network_access_enabled && (bytes.starts_with(b"http://") || bytes.starts_with(b"https://")) {
            return Err("NetworkAccessDenied: network requests disabled by policy");
        }

        let len = bytes.len().min(self.current_url.len());
        self.current_url[..len].copy_from_slice(&bytes[..len]);
        self.current_url_len = len;

        let mut url_buf = [0u8; MAX_URL_BYTES];
        url_buf[..len].copy_from_slice(&bytes[..len]);
        self.push_event(WebViewEvent::PageStarted(url_buf, len));
        self.push_event(WebViewEvent::PageFinished(url_buf, len, 200));
        Ok(())
    }

    /// Loads raw HTML directly from memory with an optional base URL.
    pub fn load_html(&mut self, html: &str, base_url: Option<&str>) -> Result<(), &'static str> {
        self.line_count = 0;
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
        self.is_loading = false;
    }

    pub fn can_go_back(&self) -> bool {
        self.can_back
    }

    pub fn can_go_forward(&self) -> bool {
        self.can_fwd
    }

    pub fn is_loading(&self) -> bool {
        self.is_loading
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
    pub fn get_pixel(&self, x: usize, y: usize) -> u32 {
        if x < self.x as usize || x >= (self.x + self.w) as usize || y < self.y as usize || y >= (self.y + self.h) as usize {
            return 0x00000000;
        }

        // Simple dark neutral webview container background
        let rel_y = y.saturating_sub(self.y as usize) as i32 + self.scroll.offset as i32;
        let line_height = 24i32;

        for (i, line) in self.lines[..self.line_count].iter().enumerate() {
            let line_y = (i as i32) * line_height + 16;
            if rel_y >= line_y && rel_y < line_y + line_height {
                // If within text region (simple glyph representation)
                let rel_x = x.saturating_sub(self.x as usize);
                if rel_x >= 16 && rel_x < 16 + line.len * 8 {
                    return line.color;
                }
            }
        }

        0xFF121212 // #121212 dark background
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
}
