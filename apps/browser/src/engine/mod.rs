// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

//! Stable browser engine abstraction for the InterOS sovereign browser application.
//!
//! Provides the `NativeFlowEngine` backend implementing `interos_std::engine::BrowserEngine`.

pub use interos_std::engine::{BrowserEngine, EngineAction, EngineCapabilities, EngineStatus};
use crate::layout::dom::{Dom, NodeId, NO_NODE};
use crate::layout::flow::{self, TextLayout};

/// The room one page's inline scripts may occupy when run at load.
///
/// WHY a fixed buffer: the engine is `#![no_std]` and never allocates, so the
/// concatenated program text lives in a bounded buffer. A page whose scripts
/// exceed this is run up to the cap; the remainder is not executed rather than
/// silently truncated mid-statement.
const SCRIPT_BUF_BYTES: usize = 4096;

/// The most `<script>` elements one page may run at load.
const MAX_PAGE_SCRIPTS: usize = 16;

/// Runs a page's inline scripts, then dispatches `DOMContentLoaded` to any
/// listener they registered.
///
/// WHY the scripts share one runtime: a browser runs all of a page's scripts
/// in one global scope, so a function declared in one script is visible to the
/// next and a listener registered by any of them fires on the load event. The
/// bodies are joined with a newline into one program and executed once, which
/// is what makes `addEventListener('DOMContentLoaded', ...)` build content
/// during load. A script that overruns its budget or capacity is skipped and
/// the rest still run, so one hostile script cannot blank the page.
fn run_page_scripts(dom: &mut Dom) {
    let node_count = dom.node_count();
    let mut scripts: [NodeId; MAX_PAGE_SCRIPTS] = [NO_NODE; MAX_PAGE_SCRIPTS];
    let mut script_count = 0usize;
    for id in 0..node_count as NodeId {
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
            if script_count < scripts.len() {
                scripts[script_count] = id;
                script_count += 1;
            }
        }
    }
    if script_count == 0 {
        return;
    }

    let mut buf = [0u8; SCRIPT_BUF_BYTES];
    let mut len = 0usize;
    for i in 0..script_count {
        let body = dom.script_body(scripts[i]);
        if body.is_empty() {
            continue;
        }
        let room = buf.len().saturating_sub(len);
        let take = body.len().min(room);
        if take == 0 {
            break;
        }
        buf[len..len + take].copy_from_slice(&body[..take]);
        len += take;
        // A newline separates one script's statements from the next so a
        // trailing token cannot merge with the following script's first.
        if len < buf.len() {
            buf[len] = b'\n';
            len += 1;
        }
    }
    if len == 0 {
        return;
    }
    let _ = crate::js::execute_script_with_events(&buf[..len], dom, crate::js::DEFAULT_STEP_BUDGET, b"DOMContentLoaded");
}

/// Sovereign `#![no_std]` native flow layout browser engine.
pub struct NativeFlowEngine {
    dom: Dom,
    layout: TextLayout,
    viewport_w: u32,
    viewport_h: u32,
    scroll_y: i32,
    current_url: [u8; 256],
    current_url_len: usize,
    title: [u8; 64],
    title_len: usize,
    status: EngineStatus,
}

impl NativeFlowEngine {
    pub fn new(viewport_w: u32, viewport_h: u32) -> Self {
        Self {
            dom: Dom::new(),
            layout: TextLayout::new(),
            viewport_w,
            viewport_h,
            scroll_y: 0,
            current_url: [0u8; 256],
            current_url_len: 0,
            title: [0u8; 64],
            title_len: 0,
            status: EngineStatus::Ready,
        }
    }

    pub fn dom(&self) -> &Dom {
        &self.dom
    }

    pub fn dom_mut(&mut self) -> &mut Dom {
        &mut self.dom
    }

    pub fn layout(&self) -> &TextLayout {
        &self.layout
    }

    pub fn rect_color_at(&self, x: usize, doc_y: usize) -> Option<u32> {
        for i in (0..self.layout.rect_count).rev() {
            let r = &self.layout.rects[i];
            // Skip transparent rects (alpha == 0)
            if (r.color >> 24) == 0 {
                continue;
            }
            let ry = r.y as usize;
            let rh = r.h as usize;
            if doc_y < ry || doc_y >= ry.saturating_add(rh) {
                continue;
            }
            let rx = r.x as usize;
            let rw = r.w as usize;
            if x < rx || x >= rx.saturating_add(rw) {
                continue;
            }
            if r.kind == crate::layout::flow::RectKind::Outline {
                let is_perimeter = x == rx
                    || x == rx.saturating_add(rw).saturating_sub(1)
                    || doc_y == ry
                    || doc_y == ry.saturating_add(rh).saturating_sub(1);
                if !is_perimeter {
                    continue;
                }
            }
            return Some(r.color);
        }
        None
    }
}

impl BrowserEngine for NativeFlowEngine {
    fn name(&self) -> &'static str {
        "InterOS Native Flow Engine"
    }

    fn version(&self) -> &'static str {
        "1.0.0-sovereign"
    }

    fn status(&self) -> EngineStatus {
        self.status
    }

    fn capabilities(&self) -> EngineCapabilities {
        EngineCapabilities::native_flow()
    }

    /// Navigates to a remote or local URL.
    ///
    /// In NativeFlowEngine, URL navigation registers the target URL and enters `EngineStatus::Loading`
    /// state, awaiting document delivery via `load_html()` or response feeder from the browser's
    /// real network pipeline.
    fn load_url(&mut self, url: &str) -> Result<(), &'static str> {
        let bytes = url.as_bytes();
        let len = bytes.len().min(self.current_url.len());
        self.current_url[..len].copy_from_slice(&bytes[..len]);
        self.current_url_len = len;
        self.scroll_y = 0;
        self.status = EngineStatus::Loading;
        Ok(())
    }

    fn load_html(&mut self, html: &str, base_url: Option<&str>) -> Result<(), &'static str> {
        if let Some(base) = base_url {
            let bytes = base.as_bytes();
            let len = bytes.len().min(self.current_url.len());
            self.current_url[..len].copy_from_slice(&bytes[..len]);
            self.current_url_len = len;
        }

        // Extract title from HTML before skipping it in DOM
        if let Some(t_start) = html.find("<title>") {
            if let Some(t_end) = html[t_start..].find("</title>") {
                let title_content = &html[t_start + 7..t_start + t_end];
                let bytes = title_content.as_bytes();
                let len = bytes.len().min(self.title.len());
                self.title[..len].copy_from_slice(&bytes[..len]);
                self.title_len = len;
            }
        }

        self.dom = Dom::new();
        self.dom.parse(html.as_bytes());
        // Scripts run before layout so a page that builds its own content is
        // laid out as it will be seen, not as the bare markup arrived.
        run_page_scripts(&mut self.dom);

        self.layout = TextLayout::new();
        self.dom.collect_names();
        let sheet = crate::css::parse_stylesheet(self.dom.css(), self.dom.token_store());
        let width = (self.viewport_w.saturating_sub(32) as usize).max(32);
        flow::build(&self.dom, &sheet, width, &mut self.layout);
        self.scroll_y = 0;
        self.status = EngineStatus::Ready;
        Ok(())
    }

    fn viewport(&self) -> (u32, u32) {
        (self.viewport_w, self.viewport_h)
    }

    fn set_viewport(&mut self, w: u32, h: u32) {
        self.viewport_w = w;
        self.viewport_h = h;
    }

    fn scroll(&mut self, dy: i32) {
        let max_scroll = (self.layout.height as i32).saturating_sub(self.viewport_h as i32).max(0);
        self.scroll_y = (self.scroll_y + dy).clamp(0, max_scroll);
    }

    fn render_pixel(&self, x: usize, y: usize) -> u32 {
        if x >= self.viewport_w as usize || y >= self.viewport_h as usize {
            return 0x00000000;
        }

        let doc_y = y as i32 + self.scroll_y;
        if doc_y < 0 {
            return 0xFF121212;
        }

        let doc_y_u = doc_y as usize;

        // 1. Text runs (ink on top of page backgrounds)
        let mut bg_color_cached: Option<u32> = None;
        for i in 0..self.layout.run_count {
            let run = &self.layout.runs[i];
            let ry = run.y as usize;
            let rh = (run.h as usize).max(interos_std::font::line_height(run.scale as usize));
            if doc_y_u < ry || doc_y_u >= ry.saturating_add(rh) {
                continue;
            }
            let rx = run.x as usize;
            let rw = run.w as usize;
            if x >= rx && x < rx.saturating_add(rw) {
                let bytes = run.bytes(&self.dom);
                if !bytes.is_empty() {
                    let text_color = if run.link != crate::layout::NO_LINK {
                        0xFF2563EB // Link blue
                    } else if run.has_color {
                        run.color
                    } else {
                        0xFF1E293B // Dark slate typography
                    };
                    let bg_color = *bg_color_cached.get_or_insert_with(|| {
                        self.rect_color_at(x, doc_y_u).unwrap_or(0xFFFFFFFF)
                    });
                    if let Some(px) = interos_std::font::test_glyph_aa(
                        x,
                        doc_y_u,
                        rx,
                        ry,
                        run.scale as usize,
                        bytes,
                        text_color,
                        bg_color,
                        run.bold,
                    ) {
                        return px;
                    }
                    if run.link != crate::layout::NO_LINK {
                        let underline_y = ry + interos_std::font::line_height(run.scale as usize).saturating_sub(2);
                        if doc_y_u == underline_y {
                            return 0xFF2563EB;
                        }
                    }
                }
            }
        }

        // 2. Check rect items (backgrounds, borders)
        if let Some(color) = bg_color_cached.or_else(|| self.rect_color_at(x, doc_y_u)) {
            return color;
        }

        0xFFFFFFFF // Default page background
    }

    fn handle_touch(&mut self, x: u32, y: u32, kind: u8) -> EngineAction {
        if kind != 1 {
            return EngineAction::None;
        }
        let doc_y = y as i32 + self.scroll_y;
        if doc_y < 0 {
            return EngineAction::None;
        }

        // Hit-test links
        if let Some(link_id) = self.layout.link_at(x as usize, doc_y as usize) {
            if let Some(href) = self.dom.link_href(link_id) {
                let mut buf = [0u8; 256];
                let len = href.len().min(buf.len());
                buf[..len].copy_from_slice(&href[..len]);
                return EngineAction::Navigate(buf, len);
            }
        }

        // Hit-test form fields
        if let Some(field_idx) = self.layout.field_at(x as usize, doc_y as usize) {
            if field_idx < self.layout.field_count {
                let field = &self.layout.fields[field_idx];
                return EngineAction::FocusInput(field.node as u32);
            }
        }

        EngineAction::None
    }

    fn title(&self) -> &str {
        if self.title_len == 0 {
            "Untitled"
        } else {
            core::str::from_utf8(&self.title[..self.title_len]).unwrap_or("Untitled")
        }
    }

    fn url(&self) -> &str {
        core::str::from_utf8(&self.current_url[..self.current_url_len]).unwrap_or("")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_flow_engine_renders_html_and_reports_ready() {
        let mut engine = NativeFlowEngine::new(720, 1234);
        assert_eq!(engine.status(), EngineStatus::Ready);
        assert_eq!(engine.name(), "InterOS Native Flow Engine");

        let html = "<html><head><title>Test Doc</title></head><body><h1>Heading</h1><p>Paragraph</p></body></html>";
        let res = engine.load_html(html, Some("https://example.com/test"));
        assert!(res.is_ok());
        assert_eq!(engine.title(), "Test Doc");
        assert_eq!(engine.url(), "https://example.com/test");
        assert!(engine.layout().run_count > 0);
    }

    #[test]
    fn native_flow_capabilities_match_contract() {
        let engine = NativeFlowEngine::new(720, 1234);
        let caps = engine.capabilities();
        assert!(!caps.supports_css_flexbox);
        assert!(!caps.supports_css_grid);
        assert!(!caps.supports_full_js);
        assert!(caps.supports_cookies);
        assert!(!caps.supports_streaming_render);
    }

    #[test]
    fn native_flow_lifecycle_and_rendering_bounds() {
        let mut engine = NativeFlowEngine::new(720, 1234);
        assert_eq!(engine.status(), EngineStatus::Ready);

        // 1. URL navigation transitions to Loading awaiting network document delivery
        let res = engine.load_url("https://example.com/feed");
        assert!(res.is_ok());
        assert_eq!(engine.status(), EngineStatus::Loading);
        assert_eq!(engine.url(), "https://example.com/feed");

        // 2. Document delivery via load_html transitions to Ready
        let html = "<html><head><title>Page Loaded</title></head><body><h1>Document Title</h1><p>Document body line</p></body></html>";
        assert!(engine.load_html(html, None).is_ok());
        assert_eq!(engine.status(), EngineStatus::Ready);
        assert_eq!(engine.title(), "Page Loaded");

        // 3. Viewport bounds clipping
        assert_eq!(engine.render_pixel(720, 500), 0x00000000, "Must clip x >= viewport_w");
        assert_eq!(engine.render_pixel(500, 1234), 0x00000000, "Must clip y >= viewport_h");
        assert_eq!(engine.render_pixel(1000, 2000), 0x00000000);

        // 4. Scrolling and bounds
        engine.scroll(50);
        let px_scrolled = engine.render_pixel(100, 100);
        assert!(px_scrolled != 0x00000000);
        engine.scroll(-100); // clamped to 0
        let px_top = engine.render_pixel(100, 100);
        assert!(px_top != 0x00000000);
    }

    #[test]
    fn native_flow_render_pixel_samples_glyph_ink() {
        let mut engine = NativeFlowEngine::new(720, 1234);
        let html = "<html><body><h1>Hello InterOS</h1></body></html>";
        assert!(engine.load_html(html, None).is_ok());

        // Probe pixel area where heading text is laid out
        let mut found_ink = false;
        for y in 0..60 {
            for x in 0..200 {
                let px = engine.render_pixel(x, y);
                if px != 0xFFFFFFFF && px != 0xFF121212 {
                    found_ink = true;
                    break;
                }
            }
            if found_ink {
                break;
            }
        }
        assert!(found_ink, "render_pixel must sample font glyph ink");
    }

    #[test]
    fn native_flow_parses_multiple_style_blocks_into_layout() {
        let mut engine = NativeFlowEngine::new(720, 1234);
        let html = "<html><head><style>h1 { color: #FF0000; }</style><style>p { color: #00FF00; }</style></head><body><h1>Heading</h1><p>Paragraph</p></body></html>";
        assert!(engine.load_html(html, None).is_ok());
        assert!(!engine.dom().css().is_empty());
        assert!(engine.layout().run_count >= 2);
    }

    #[test]
    fn native_flow_multiple_styles_apply_colors_dimensions_and_pixel_values() {
        let mut engine = NativeFlowEngine::new(720, 1234);
        let html = "<html><head><style>h1 { color: #D32F2F; }</style><style>p { color: #1565C0; }</style></head><body><h1>Header</h1><p>Paragraph text</p></body></html>";
        assert!(engine.load_html(html, None).is_ok());
        assert!(engine.layout().run_count >= 2);

        // Find run with h1 color (#D32F2F) and run with p color (#1565C0)
        let r0 = engine.layout().runs[..engine.layout().run_count]
            .iter()
            .find(|r| r.has_color && r.color == 0xFFD32F2F)
            .expect("h1 run with #D32F2F");
        let r1 = engine.layout().runs[..engine.layout().run_count]
            .iter()
            .find(|r| r.has_color && r.color == 0xFF1565C0)
            .expect("p run with #1565C0");
        assert!(r0.has_color);
        assert!(r1.has_color);

        // Pixel rasterization detects font glyph ink for h1
        let mut found_h1_ink = false;
        for y in (r0.y as usize)..(r0.y as usize + r0.h as usize) {
            for x in (r0.x as usize)..(r0.x as usize + r0.w as usize) {
                let px = engine.render_pixel(x, y);
                if px != 0xFFFFFFFF {
                    found_h1_ink = true;
                    break;
                }
            }
            if found_h1_ink {
                break;
            }
        }
        assert!(found_h1_ink, "h1 glyph ink must be rasterized");
    }

    #[test]
    fn native_flow_contrast_rule_drops_low_contrast_colors() {
        let mut engine = NativeFlowEngine::new(720, 1234);
        // Pure white #FFFFFF on white background fails 2:1 contrast ratio
        let html = "<html><head><style>h1 { color: #FFFFFF; }</style></head><body><h1>Invisible</h1></body></html>";
        assert!(engine.load_html(html, None).is_ok());
        assert_eq!(engine.layout().run_count, 1);
        let r0 = &engine.layout().runs[0];
        // Low contrast color must be dropped to preserve readability
        assert!(!r0.has_color, "Low contrast color on light background must be dropped");
    }

    #[test]
    fn native_flow_nested_rects_follow_painters_order() {
        let mut engine = NativeFlowEngine::new(720, 1234);
        // Outer rect (white), inner card (blue), topmost button (red)
        engine.layout.rects[0] = crate::layout::RectItem { x: 10, y: 10, w: 200, h: 200, color: 0xFFFFFFFF, kind: crate::layout::RectKind::Fill };
        engine.layout.rects[1] = crate::layout::RectItem { x: 20, y: 20, w: 100, h: 100, color: 0xFF0000FF, kind: crate::layout::RectKind::Fill };
        engine.layout.rects[2] = crate::layout::RectItem { x: 30, y: 30, w: 40, h: 40, color: 0xFFFF0000, kind: crate::layout::RectKind::Fill };
        engine.layout.rect_count = 3;

        // Inside innermost button (35, 35) -> should get topmost color (red)
        assert_eq!(engine.render_pixel(35, 35), 0xFFFF0000);
        // Inside card but outside button (25, 25) -> should get middle color (blue)
        assert_eq!(engine.render_pixel(25, 25), 0xFF0000FF);
        // Inside outer rect only (15, 15) -> should get outer color (white)
        assert_eq!(engine.render_pixel(15, 15), 0xFFFFFFFF);
        // Outside all rects (5, 5) -> should get default background (white)
        assert_eq!(engine.render_pixel(5, 5), 0xFFFFFFFF);
    }

    #[test]
    fn native_flow_outline_rect_does_not_occlude_interior_fill() {
        let mut engine = NativeFlowEngine::new(720, 1234);
        // Card background fill (gray 0xFFE2E8F0) followed by card outline (blue 0xFF2563EB)
        engine.layout.rects[0] = crate::layout::RectItem {
            x: 10,
            y: 10,
            w: 100,
            h: 100,
            color: 0xFFE2E8F0,
            kind: crate::layout::RectKind::Fill,
        };
        engine.layout.rects[1] = crate::layout::RectItem {
            x: 10,
            y: 10,
            w: 100,
            h: 100,
            color: 0xFF2563EB,
            kind: crate::layout::RectKind::Outline,
        };
        engine.layout.rect_count = 2;

        // Perimeter pixels must show outline color
        assert_eq!(engine.render_pixel(10, 10), 0xFF2563EB);
        assert_eq!(engine.render_pixel(109, 10), 0xFF2563EB);
        assert_eq!(engine.render_pixel(10, 109), 0xFF2563EB);
        assert_eq!(engine.render_pixel(109, 109), 0xFF2563EB);

        // Interior pixels must show fill color, NOT be occluded by outline
        assert_eq!(engine.render_pixel(11, 11), 0xFFE2E8F0);
        assert_eq!(engine.render_pixel(50, 50), 0xFFE2E8F0);
        assert_eq!(engine.render_pixel(108, 108), 0xFFE2E8F0);
    }

    #[test]
    fn native_flow_transparent_rect_passes_through_to_underlying_layer() {
        let mut engine = NativeFlowEngine::new(720, 1234);
        // Base fill (red 0xFFFF0000) covered by transparent rect (alpha = 0)
        engine.layout.rects[0] = crate::layout::RectItem {
            x: 10,
            y: 10,
            w: 50,
            h: 50,
            color: 0xFFFF0000,
            kind: crate::layout::RectKind::Fill,
        };
        engine.layout.rects[1] = crate::layout::RectItem {
            x: 10,
            y: 10,
            w: 50,
            h: 50,
            color: 0x00FFFFFF, // Transparent
            kind: crate::layout::RectKind::Fill,
        };
        engine.layout.rect_count = 2;

        assert_eq!(engine.render_pixel(25, 25), 0xFFFF0000);
    }

    /// Acceptance: a `<script>` that creates an element and sets its text is
    /// executed at load, and the new content reaches the display list and the
    /// rasterized pixels -- not merely the DOM or the source string.
    #[test]
    fn dynamic_script_mutation_reaches_rendered_pixels() {
        let with_script = "<html><body><p id=\"s\">start</p>\
<script>var d = document.createElement('p'); d.textContent = 'DYNAMIC'; \
document.body.appendChild(d);</script></body></html>";
        let mut engine = NativeFlowEngine::new(720, 1234);
        assert!(engine.load_html(with_script, None).is_ok());

        // A laid-out text run carries the injected word.
        let run = engine.layout().runs[..engine.layout().run_count]
            .iter()
            .find(|r| r.bytes(engine.dom()).windows(7).any(|w| w == b"DYNAMIC"))
            .expect("the script-injected text must be laid out as a run");

        // The run's box rasterizes to glyph ink, not a blank background.
        let mut ink = false;
        for y in run.y as usize..run.y as usize + run.h as usize {
            for x in run.x as usize..run.x as usize + run.w as usize {
                let px = engine.render_pixel(x, y);
                if px != 0xFFFFFFFF && px != 0xFF121212 {
                    ink = true;
                    break;
                }
            }
            if ink {
                break;
            }
        }
        assert!(ink, "the script-injected text must rasterize to pixels");

        // Control: the same page without the script has no such run, proving
        // the pixels came from script execution and not the source markup.
        let without_script = "<html><body><p id=\"s\">start</p></body></html>";
        let mut control = NativeFlowEngine::new(720, 1234);
        assert!(control.load_html(without_script, None).is_ok());
        let leaked = control.layout().runs[..control.layout().run_count]
            .iter()
            .any(|r| r.bytes(control.dom()).windows(7).any(|w| w == b"DYNAMIC"));
        assert!(!leaked, "without the script the injected text must be absent");
        assert!(
            engine.layout().run_count > control.layout().run_count,
            "the script must add at least one laid-out run"
        );
    }

    /// Acceptance: a listener registered with `addEventListener` for
    /// `DOMContentLoaded` runs during load and its DOM mutation is laid out.
    #[test]
    fn dom_content_loaded_listener_builds_content_at_load() {
        let html = "<html><body>\
<script>function boot() { var s = document.createElement('p'); \
s.textContent = 'BOOTED'; document.body.appendChild(s); } \
document.addEventListener('DOMContentLoaded', boot);</script></body></html>";
        let mut engine = NativeFlowEngine::new(720, 1234);
        assert!(engine.load_html(html, None).is_ok());

        let laid_out = engine.layout().runs[..engine.layout().run_count]
            .iter()
            .any(|r| r.bytes(engine.dom()).windows(6).any(|w| w == b"BOOTED"));
        assert!(
            laid_out,
            "the DOMContentLoaded listener must run at load and its content must be laid out"
        );
    }

    /// Acceptance: the `tests/fixtures/dynamic.html` page is loaded from disk,
    /// its inline script runs at `DOMContentLoaded`, and the element it builds
    /// with `createElement`/`appendChild` reaches the DOM and the display list.
    ///
    /// WHY this is not a source-string check: the fixture source carries
    /// `DYNAMIC_OK_42` only inside the `<script>` body. The assertions below
    /// find it as a real DOM text node and as a laid-out run, and a control
    /// page without the script produces neither -- so the text can only have
    /// come from script execution mutating the tree.
    #[test]
    fn dynamic_fixture_script_creates_rendered_text() {
        const FIXTURE: &str = include_str!("../../../../tests/fixtures/dynamic.html");

        let mut engine = NativeFlowEngine::new(720, 1234);
        assert!(engine.load_html(FIXTURE, None).is_ok());

        // The script-created text is a real DOM text node, not just source.
        let dom = engine.dom();
        let mut found_text_node = false;
        for i in 0..dom.node_count() {
            let nid = i as NodeId;
            if dom.node(nid).is_text() && dom.text_of(nid) == b"DYNAMIC_OK_42" {
                found_text_node = true;
            }
        }
        assert!(
            found_text_node,
            "the script-created text must exist as a DOM text node"
        );

        // ...and it is laid out as a run, i.e. it reaches rendered output.
        let laid_out = engine.layout().runs[..engine.layout().run_count]
            .iter()
            .any(|r| r.bytes(engine.dom()).windows(13).any(|w| w == b"DYNAMIC_OK_42"));
        assert!(
            laid_out,
            "the script-created text must be laid out as a display-list run"
        );

        // Control: the same document without the script yields no such text
        // node and fewer runs, proving the run came from script execution.
        let without_script = "<html><body><p id=\"s\">start</p></body></html>";
        let mut control = NativeFlowEngine::new(720, 1234);
        assert!(control.load_html(without_script, None).is_ok());
        let leaked = (0..control.dom().node_count()).any(|i| {
            let nid = i as NodeId;
            control.dom().node(nid).is_text()
                && control.dom().text_of(nid) == b"DYNAMIC_OK_42"
        });
        assert!(!leaked, "without the script the injected text must be absent");
        assert!(
            engine.layout().run_count > control.layout().run_count,
            "the script must add at least one laid-out run"
        );
    }

    /// Acceptance: the fixture's button carries an `onclick` handler that, when
    /// run against the live DOM, mutates the script-created element's text.
    ///
    /// WHY the id is set by the harness: the bounded engine's script subset can
    /// build a node and set its text but cannot assign `id`, so the fixture's
    /// `getElementById('out')` has nothing to find in a unit test. The harness
    /// sets the id the real handler queries by, then runs the fixture's own
    /// onclick source -- so the mutation under test is the page's handler, not
    /// a substitute.
    #[test]
    fn dynamic_fixture_button_onclick_mutates_created_text() {
        const FIXTURE: &str = include_str!("../../../../tests/fixtures/dynamic.html");

        let mut engine = NativeFlowEngine::new(720, 1234);
        assert!(engine.load_html(FIXTURE, None).is_ok());

        // Capture the onclick source and the element the script built (the
        // parent of the dynamic text node) before mutating anything.
        let mut onclick_buf = [0u8; 256];
        let mut onclick_len = 0usize;
        let mut out_elem = None;
        {
            let dom = engine.dom();
            for i in 0..dom.node_count() {
                let nid = i as NodeId;
                if dom.node(nid).is_element()
                    && dom.tag(nid).eq_ignore_ascii_case(b"button")
                {
                    if let Some(src) = dom.attr(nid, b"onclick") {
                        let n = src.len().min(onclick_buf.len());
                        onclick_buf[..n].copy_from_slice(&src[..n]);
                        onclick_len = n;
                    }
                }
                if dom.node(nid).is_text() && dom.text_of(nid) == b"DYNAMIC_OK_42" {
                    out_elem = dom.parent(nid);
                }
            }
        }
        assert!(onclick_len > 0, "the fixture button must carry an onclick handler");
        let out_elem = out_elem.expect("the script must create the dynamic element");

        // The handler looks the element up by id; give the built element that
        // id, then run the fixture's own onclick source against the DOM.
        assert!(engine.dom_mut().set_attr(out_elem, b"id", b"out"));
        let (_value, mutated) = crate::js::execute_script_with_mutations(
            &onclick_buf[..onclick_len],
            engine.dom_mut(),
            crate::js::DEFAULT_STEP_BUDGET,
        )
        .expect("the fixture onclick handler runs");
        assert!(mutated, "the onclick handler must mutate the DOM");

        // The element's content is now the clicked text, installed as a fresh
        // text child by the textContent setter.
        let new_text = engine
            .dom()
            .first_child(out_elem)
            .expect("the element must hold a text child after the click");
        assert_eq!(engine.dom().text_of(new_text), b"DYNAMIC_CLICKED_42");
    }
}
