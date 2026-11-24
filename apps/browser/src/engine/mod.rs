// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

//! Stable browser engine abstraction for the InterOS sovereign browser application.
//!
//! Provides the `NativeFlowEngine` backend implementing `interos_std::engine::BrowserEngine`.

pub use interos_std::engine::{BrowserEngine, EngineAction, EngineCapabilities, EngineStatus};
use crate::layout::dom::Dom;
use crate::layout::flow::{self, TextLayout};

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
            let ry = r.y as usize;
            let rh = r.h as usize;
            if doc_y < ry || doc_y >= ry.saturating_add(rh) {
                continue;
            }
            let rx = r.x as usize;
            let rw = r.w as usize;
            if x >= rx && x < rx.saturating_add(rw) {
                return Some(r.color);
            }
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
        if let Some(color) = self.rect_color_at(x, doc_y_u) {
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
}
