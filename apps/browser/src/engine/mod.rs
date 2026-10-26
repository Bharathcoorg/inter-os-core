//! Stable browser engine abstraction for the InterOS sovereign browser application.
//!
//! Provides the `NativeFlowEngine` backend implementing `interos_std::engine::BrowserEngine`.

pub use interos_std::engine::{BrowserEngine, EngineAction, EngineCapabilities, EngineStatus};
use crate::css::Stylesheet;
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
}

impl BrowserEngine for NativeFlowEngine {
    fn name(&self) -> &'static str {
        "InterOS Native Flow Engine"
    }

    fn version(&self) -> &'static str {
        "1.0.0-sovereign"
    }

    fn status(&self) -> EngineStatus {
        EngineStatus::Ready
    }

    fn capabilities(&self) -> EngineCapabilities {
        EngineCapabilities::native_flow()
    }

    fn load_url(&mut self, url: &str) -> Result<(), &'static str> {
        let bytes = url.as_bytes();
        let len = bytes.len().min(self.current_url.len());
        self.current_url[..len].copy_from_slice(&bytes[..len]);
        self.current_url_len = len;
        self.scroll_y = 0;
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
        let sheet = Stylesheet::empty();
        let width = (self.viewport_w.saturating_sub(32) as usize).max(32);
        flow::build(&self.dom, &sheet, width, &mut self.layout);
        self.scroll_y = 0;
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
        let doc_y = y as i32 + self.scroll_y;
        if doc_y < 0 {
            return 0xFF121212;
        }

        let doc_y_u = doc_y as usize;
        // Check rect items (backgrounds, borders)
        for i in 0..self.layout.rect_count {
            let r = &self.layout.rects[i];
            let rx = r.x as usize;
            let ry = r.y as usize;
            let rw = r.w as usize;
            let rh = r.h as usize;
            if x >= rx && x < rx.saturating_add(rw) && doc_y_u >= ry && doc_y_u < ry.saturating_add(rh) {
                return r.color;
            }
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
    }
}
