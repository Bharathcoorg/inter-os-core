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

/// Truthful outcome of executing a document's inline `<script>` elements.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ScriptRunReport {
    pub executed_count: usize,
    pub mutated: bool,
    pub unsupported_module: bool,
    pub skipped_external: usize,
    pub truncated: bool,
    pub last_error: Option<crate::js::JsError>,
    /// The timers the page scheduled that have not run yet.
    ///
    /// WHY they are reported: a timer with a delay outlives the run that
    /// scheduled it, so the caller has to learn which callbacks are waiting
    /// and how long they asked to wait.
    pub timers: [crate::js::ScheduledTimer; crate::js::MAX_TIMERS],
    pub timer_count: usize,
}

/// Runs a page's inline classic scripts, then dispatches `DOMContentLoaded` to
/// any listener they registered.
///
/// WHY the scripts share one runtime: a browser runs all of a page's scripts
/// in one global scope, so a function declared in one script is visible to the
/// next and a listener registered by any of them fires on the load event.
/// `<script src>` tags are skipped without consuming inline script slots,
/// `<script type="module">` is explicitly rejected/skipped without poisoning
/// classic scripts, and inline scripts that exceed [`SCRIPT_BUF_BYTES`] are
/// refused whole rather than truncated mid-statement.
pub fn run_page_scripts(dom: &mut Dom) -> ScriptRunReport {
    run_page_scripts_inner(dom, None, &[])
}

/// Runs a page's inline scripts with the page's storage attached.
///
/// WHY the origin travels with the store: the page may only reach the
/// entries its own origin wrote, so the runtime is told which origin the
/// document was served from.
pub fn run_page_scripts_with_storage(
    dom: &mut Dom,
    storage: &mut crate::WebStorage,
    origin: &[u8],
) -> ScriptRunReport {
    run_page_scripts_inner(dom, Some(storage), origin)
}

/// Runs one script buffer with the store attached, or without one.
fn run_script_buffer(
    script: &[u8],
    dom: &mut Dom,
    storage: &mut Option<&mut crate::WebStorage>,
    origin: &[u8],
) -> Result<crate::js::ScriptRun, crate::js::JsError> {
    match storage.as_mut() {
        Some(store) => crate::js::execute_script_reporting_with_storage(
            script,
            dom,
            crate::js::DEFAULT_STEP_BUDGET,
            b"DOMContentLoaded",
            &mut *store,
            origin,
        ),
        None => crate::js::execute_script_reporting(
            script,
            dom,
            crate::js::DEFAULT_STEP_BUDGET,
            b"DOMContentLoaded",
        ),
    }
}

/// Collects the page's inline script text into `buf` and returns its length.
///
/// WHY the text is gathered into one buffer: a browser runs all of a page's
/// classic scripts in one global scope, so a function declared in one script
/// is visible to the next.
fn collect_page_scripts(dom: &mut Dom, buf: &mut [u8; SCRIPT_BUF_BYTES]) -> usize {
    let node_count = dom.node_count();
    let mut len = 0usize;
    for id in 0..node_count as NodeId {
        let nid = id as NodeId;
        if !(dom.node(nid).is_element() && dom.tag(nid).eq_ignore_ascii_case(b"script")) {
            continue;
        }
        if let Some(kind) = dom.attr(nid, b"type") {
            if kind.eq_ignore_ascii_case(b"module") {
                continue;
            }
            let is_js = kind.is_empty()
                || kind.eq_ignore_ascii_case(b"text/javascript")
                || kind.eq_ignore_ascii_case(b"application/javascript")
                || kind.eq_ignore_ascii_case(b"text/ecmascript")
                || kind.eq_ignore_ascii_case(b"application/ecmascript");
            if !is_js {
                continue;
            }
        }
        if dom.has_attr(nid, b"src") {
            continue;
        }
        let body = dom.script_body(nid);
        if body.is_empty() {
            continue;
        }
        let sep = usize::from(len > 0);
        if sep + body.len() > buf.len().saturating_sub(len) {
            continue;
        }
        if sep > 0 {
            buf[len] = b'\n';
            len += 1;
        }
        buf[len..len + body.len()].copy_from_slice(body);
        len += body.len();
    }
    len
}

/// Runs the page's named function, for a timer that came due.
///
/// WHY the page's program is parsed again: the timer outlived the run that
/// scheduled it, and that run's function arena is gone. The page's program
/// is the only place the function still exists, so it is parsed again and
/// its functions are defined without running the page's other statements.
pub fn run_named_callback(dom: &mut Dom, name: &[u8]) -> Result<bool, crate::js::JsError> {
    let mut buf = [0u8; SCRIPT_BUF_BYTES];
    let len = collect_page_scripts(dom, &mut buf);
    if len == 0 {
        return Err(crate::js::JsError::ReferenceError);
    }
    crate::js::execute_named_callback(
        &buf[..len],
        dom,
        crate::js::DEFAULT_STEP_BUDGET,
        name,
    )
}

fn run_page_scripts_inner(
    dom: &mut Dom,
    mut storage: Option<&mut crate::WebStorage>,
    origin: &[u8],
) -> ScriptRunReport {
    let mut report = ScriptRunReport::default();
    let node_count = dom.node_count();
    let mut scripts: [NodeId; MAX_PAGE_SCRIPTS] = [NO_NODE; MAX_PAGE_SCRIPTS];
    let mut script_count = 0usize;
    for id in 0..node_count as NodeId {
        if dom.node(id).is_element() && dom.tag(id).eq_ignore_ascii_case(b"script") {
            if let Some(t) = dom.attr(id, b"type") {
                if t.eq_ignore_ascii_case(b"module") {
                    report.unsupported_module = true;
                    continue;
                }
                let is_js = t.is_empty()
                    || t.eq_ignore_ascii_case(b"text/javascript")
                    || t.eq_ignore_ascii_case(b"application/javascript")
                    || t.eq_ignore_ascii_case(b"text/ecmascript")
                    || t.eq_ignore_ascii_case(b"application/ecmascript");
                if !is_js {
                    continue;
                }
            }
            if dom.has_attr(id, b"src") {
                report.skipped_external += 1;
                continue;
            }
            if script_count < scripts.len() {
                scripts[script_count] = id;
                script_count += 1;
            } else {
                report.truncated = true;
            }
        }
    }
    if script_count == 0 {
        return report;
    }

    let mut buf = [0u8; SCRIPT_BUF_BYTES];
    let mut len = 0usize;
    let mut included: [NodeId; MAX_PAGE_SCRIPTS] = [NO_NODE; MAX_PAGE_SCRIPTS];
    let mut included_count = 0usize;
    for i in 0..script_count {
        let body = dom.script_body(scripts[i]);
        if body.is_empty() {
            continue;
        }
        let sep = usize::from(len > 0);
        let need = sep + body.len();
        if need > buf.len().saturating_sub(len) {
            report.truncated = true;
            continue;
        }
        if sep > 0 {
            buf[len] = b'\n';
            len += 1;
        }
        buf[len..len + body.len()].copy_from_slice(body);
        len += body.len();
        included[included_count] = scripts[i];
        included_count += 1;
    }
    if len == 0 {
        return report;
    }
    match run_script_buffer(&buf[..len], dom, &mut storage, origin) {
        Ok(run) => {
            report.executed_count = included_count;
            report.mutated = run.mutated;
            report.timers = run.timers;
            report.timer_count = run.timer_count;
        }
        Err(err) => {
            report.last_error = Some(err);
            // If parsing/capacity failed on the combined buffer across multiple
            // classic scripts, isolate each script so one bad tag cannot poison
            // the remaining valid scripts on the page.
            if included_count > 1
                && matches!(
                    err,
                    crate::js::JsError::UnsupportedSyntax(_)
                        | crate::js::JsError::UnexpectedToken
                        | crate::js::JsError::CapacityExceeded
                )
            {
                for i in 0..included_count {
                    let body = dom.script_body(included[i]);
                    let n = body.len();
                    if n == 0 || n > buf.len() {
                        continue;
                    }
                    buf[..n].copy_from_slice(body);
                    match run_script_buffer(&buf[..n], dom, &mut storage, origin) {
                        Ok(run) => {
                            report.executed_count += 1;
                            report.mutated |= run.mutated;
                            report.timers = run.timers;
                            report.timer_count = run.timer_count;
                        }
                        Err(e) => {
                            report.last_error = Some(e);
                        }
                    }
                }
            }
        }
    }
    report
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
    /// The storage the documents this engine loads may reach.
    ///
    /// WHY the engine keeps its own: a document's storage belongs to the
    /// origin it was served from, so the store has to outlive a single
    /// document and sit with the engine that loads them.
    storage: crate::WebStorage,
    title: [u8; 64],
    title_len: usize,
    status: EngineStatus,
    script_report: ScriptRunReport,
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
            storage: crate::WebStorage::new(),
            title: [0u8; 64],
            title_len: 0,
            status: EngineStatus::Ready,
            script_report: ScriptRunReport::default(),
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

    pub fn script_report(&self) -> ScriptRunReport {
        self.script_report
    }

    pub fn script_notice(&self) -> Option<&'static str> {
        if self.script_report.unsupported_module {
            Some("unsupported script type: module")
        } else if self.script_report.truncated {
            Some("script buffer capacity exceeded")
        } else {
            None
        }
    }

    pub fn relayout(&mut self) {
        self.layout = TextLayout::new();
        self.dom.collect_names();
        let sheet = crate::css::parse_stylesheet(self.dom.css(), self.dom.token_store());
        let width = (self.viewport_w.saturating_sub(32) as usize).max(32);
        flow::build(&self.dom, &sheet, width, &mut self.layout);
        let max_scroll = (self.layout.height as i32)
            .saturating_sub(self.viewport_h as i32)
            .max(0);
        self.scroll_y = self.scroll_y.clamp(0, max_scroll);
    }

    fn run_node_onclick(&mut self, node: NodeId) -> bool {
        let Some(onclick) = self.dom.attr(node, b"onclick") else {
            return false;
        };
        let mut buf = [0u8; SCRIPT_BUF_BYTES];
        if onclick.len() > buf.len() {
            self.script_report.truncated = true;
            return false;
        }
        let n = onclick.len();
        buf[..n].copy_from_slice(&onclick[..n]);
        match crate::js::execute_script_with_mutations(
            &buf[..n],
            &mut self.dom,
            crate::js::DEFAULT_STEP_BUDGET,
        ) {
            Ok((_, mutated)) => {
                self.script_report.mutated |= mutated;
                mutated
            }
            Err(err) => {
                self.script_report.last_error = Some(err);
                false
            }
        }
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
        // WHY the address is refused rather than cut: the engine used to keep
        // the first 256 bytes of a longer address, so the document it showed
        // was attributed to an address that named a different page. The
        // address is refused whole, and the caller keeps the page it had.
        if bytes.len() > self.current_url.len() {
            return Err("address too long");
        }
        self.current_url[..bytes.len()].copy_from_slice(bytes);
        self.current_url_len = bytes.len();
        // Fail closed: starting a navigation abandons the on-screen document now.
        // If the fetch that follows fails, the viewport must show an empty/Loading
        // state -- never the previous page's DOM/title laid out under the new URL,
        // which would attribute stale content to the new origin.
        self.dom = Dom::new();
        self.layout = TextLayout::new();
        self.title_len = 0;
        self.script_report = ScriptRunReport::default();
        self.scroll_y = 0;
        self.status = EngineStatus::Loading;
        Ok(())
    }

    fn load_html(&mut self, html: &str, base_url: Option<&str>) -> Result<(), &'static str> {
        if let Some(base) = base_url {
            let bytes = base.as_bytes();
            // WHY the base address is refused rather than cut: a cut base
            // address names a different page than the one the document came
            // from, and every relative link on the page would then resolve
            // against an address that was never asked for.
            if bytes.len() > self.current_url.len() {
                return Err("base address too long");
            }
            self.current_url[..bytes.len()].copy_from_slice(bytes);
            self.current_url_len = bytes.len();
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
        //
        // WHY the origin is taken from the address the document was served
        // from: the page's storage belongs to that origin, and a document
        // with no address has no origin and so no storage.
        let mut origin = [0u8; crate::MAX_STORAGE_ORIGIN_LEN];
        let origin_len = crate::origin_of(
            &self.current_url[..self.current_url_len],
            &mut origin,
        );
        self.script_report = run_page_scripts_with_storage(
            &mut self.dom,
            &mut self.storage,
            &origin[..origin_len],
        );

        self.scroll_y = 0;
        self.relayout();
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

        // 2. Form controls (buttons and inputs)
        for i in 0..self.layout.field_count {
            let field = &self.layout.fields[i];
            let fx = field.x as usize;
            let fy = field.y as usize;
            let fw = field.w as usize;
            let fh = field.h as usize;
            if fw == 0 || fh == 0 || x < fx || x >= fx + fw || doc_y_u < fy || doc_y_u >= fy + fh {
                continue;
            }
            let on_border = x == fx || x == fx + fw - 1 || doc_y_u == fy || doc_y_u == fy + fh - 1;
            let is_button = field.kind == crate::layout::FieldKind::Button
                || (field.kind == crate::layout::FieldKind::Input
                    && (field.input == crate::layout::InputType::Submit
                        || field.input == crate::layout::InputType::Reset));
            if is_button {
                let bg_col = 0xFFE2E8F0;
                let fg_col = 0xFF1E293B;
                let border_col = 0xFF64748B;
                let raw = field.value.get(&self.dom);
                let label: &[u8] = if !raw.is_empty() {
                    raw
                } else if field.input == crate::layout::InputType::Reset {
                    b"Reset"
                } else {
                    b"Button"
                };
                let tw = interos_std::font::measure(label, 1) as usize;
                let lh = interos_std::font::line_height(1);
                let tx = fx + fw.saturating_sub(tw) / 2;
                let ty = fy + fh.saturating_sub(lh) / 2;
                if let Some(px) = interos_std::font::test_glyph_aa(
                    x,
                    doc_y_u,
                    tx,
                    ty,
                    1,
                    label,
                    fg_col,
                    bg_col,
                    true,
                ) {
                    return px;
                }
                return if on_border { border_col } else { bg_col };
            }
            return if on_border { 0xFF94A3B8 } else { 0xFFF8FAFC };
        }

        // 3. Check rect items (backgrounds, borders)
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
            for nid in 0..self.dom.node_count() as NodeId {
                if self.dom.node(nid).link == link_id {
                    if self.run_node_onclick(nid) {
                        self.relayout();
                    }
                    break;
                }
            }
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
                let field = self.layout.fields[field_idx];
                if self.run_node_onclick(field.node) {
                    self.relayout();
                }
                if field.kind == crate::layout::FieldKind::Button
                    || (field.kind == crate::layout::FieldKind::Input
                        && matches!(
                            field.input,
                            crate::layout::InputType::Submit
                                | crate::layout::InputType::Reset
                                | crate::layout::InputType::Checkbox
                                | crate::layout::InputType::Radio
                        ))
                {
                    return EngineAction::None;
                }
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
        assert!(caps.supports_css_flexbox);
        assert!(caps.supports_css_grid);
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

    const DYNAMIC_HTML: &str = "<html><head><title>Dynamic App</title></head><body>\
<h1>Interactive</h1>\
<button id=\"btn\" onclick=\"var target = document.getElementById('out'); target.textContent = 'CLICKED';\">Trigger</button>\
<script>\
function init() {\
  var p = document.createElement('p');\
  p.id = 'out';\
  p.textContent = 'READY';\
  document.body.appendChild(p);\
}\
document.addEventListener('DOMContentLoaded', init);\
</script>\
</body></html>";

    /// Acceptance: `DYNAMIC_HTML` exercises `createElement`, `appendChild`,
    /// `p.id = 'out'`, `textContent`, `DOMContentLoaded`, button `<button>`
    /// rendering, `onclick` live mutation resolved via `getElementById('out')`
    /// (without any harness `set_attr` workaround), and relayout + repaint.
    #[test]
    fn dynamic_html_end_to_end_load_button_onclick_relayout_and_repaint() {
        let mut engine = NativeFlowEngine::new(720, 1234);
        assert!(engine.load_html(DYNAMIC_HTML, None).is_ok());
        assert_eq!(engine.title(), "Dynamic App");
        assert!(engine.script_report().mutated);
        assert_eq!(engine.script_report().last_error, None);

        // 1. DOMContentLoaded created <p id="out">READY</p> and laid it out.
        let ready_run = engine.layout().runs[..engine.layout().run_count]
            .iter()
            .find(|r| r.bytes(engine.dom()).windows(5).any(|w| w == b"READY"))
            .expect("DOMContentLoaded must build and lay out READY");

        let mut ready_ink = false;
        for y in ready_run.y as usize..ready_run.y as usize + ready_run.h as usize {
            for x in ready_run.x as usize..ready_run.x as usize + ready_run.w as usize {
                let px = engine.render_pixel(x, y);
                if px != 0xFFFFFFFF && px != 0xFF121212 {
                    ready_ink = true;
                    break;
                }
            }
            if ready_ink {
                break;
            }
        }
        assert!(ready_ink, "READY must rasterize to glyph ink");

        // 2. <button> child text node is in FieldItem.value and renders to pixels.
        let btn = engine.layout().fields[..engine.layout().field_count]
            .iter()
            .find(|f| f.kind == crate::layout::FieldKind::Button)
            .expect("button field must be laid out");
        assert_eq!(btn.value.get(engine.dom()), b"Trigger");

        let bx = (btn.x as usize) + (btn.w as usize) / 2;
        let by = (btn.y as usize) + (btn.h as usize) / 2;
        let mut btn_ink = false;
        for y in btn.y as usize..btn.y as usize + btn.h as usize {
            for x in btn.x as usize..btn.x as usize + btn.w as usize {
                let px = engine.render_pixel(x, y);
                if px == 0xFF1E293B {
                    btn_ink = true;
                    break;
                }
            }
            if btn_ink {
                break;
            }
        }
        assert!(btn_ink, "<button> label 'Trigger' must rasterize inside button");

        // 3. Tapping the button executes onclick -> getElementById('out') -> relayout.
        let action = engine.handle_touch(bx as u32, by as u32, 1);
        assert_eq!(action, EngineAction::None);
        assert_eq!(engine.script_report().last_error, None);

        let clicked_run = engine.layout().runs[..engine.layout().run_count]
            .iter()
            .find(|r| r.bytes(engine.dom()).windows(7).any(|w| w == b"CLICKED"))
            .expect("onclick mutation must relayout CLICKED");
        let still_ready = engine.layout().runs[..engine.layout().run_count]
            .iter()
            .any(|r| r.bytes(engine.dom()).windows(5).any(|w| w == b"READY"));
        assert!(!still_ready, "old READY run must be replaced by CLICKED after relayout");

        let mut clicked_ink = false;
        for y in clicked_run.y as usize..clicked_run.y as usize + clicked_run.h as usize {
            for x in clicked_run.x as usize..clicked_run.x as usize + clicked_run.w as usize {
                let px = engine.render_pixel(x, y);
                if px != 0xFFFFFFFF && px != 0xFF121212 {
                    clicked_ink = true;
                    break;
                }
            }
            if clicked_ink {
                break;
            }
        }
        assert!(clicked_ink, "CLICKED must rasterize to glyph ink after relayout");
    }

    #[test]
    fn module_script_is_skipped_with_unsupported_status_without_poisoning_classic() {
        let html = "<html><body>\
<script type=\"module\">import { x } from './app.js';</script>\
<script>var p = document.createElement('p'); p.textContent = 'CLASSIC'; document.body.appendChild(p);</script>\
</body></html>";
        let mut engine = NativeFlowEngine::new(720, 1234);
        assert!(engine.load_html(html, None).is_ok());
        assert!(engine.script_report().unsupported_module);
        assert_eq!(engine.script_notice(), Some("unsupported script type: module"));
        assert_eq!(engine.script_report().last_error, None);
        let has_classic = engine.layout().runs[..engine.layout().run_count]
            .iter()
            .any(|r| r.bytes(engine.dom()).windows(7).any(|w| w == b"CLASSIC"));
        assert!(has_classic, "classic script must still run when module script is present");
    }

    #[test]
    fn external_script_src_does_not_consume_inline_slots() {
        let mut buf = [0u8; 2048];
        let mut pos = 0usize;
        let head = b"<html><body>";
        buf[pos..pos + head.len()].copy_from_slice(head);
        pos += head.len();
        let ext = b"<script src=\"/s.js\"></script>";
        for _ in 0..MAX_PAGE_SCRIPTS {
            buf[pos..pos + ext.len()].copy_from_slice(ext);
            pos += ext.len();
        }
        let tail = b"<script>var p = document.createElement('p'); p.textContent = 'INLINE_OK'; document.body.appendChild(p);</script></body></html>";
        buf[pos..pos + tail.len()].copy_from_slice(tail);
        pos += tail.len();
        let html = core::str::from_utf8(&buf[..pos]).unwrap();
        let mut engine = NativeFlowEngine::new(720, 1234);
        assert!(engine.load_html(html, None).is_ok());
        assert_eq!(engine.script_report().skipped_external, MAX_PAGE_SCRIPTS);
        assert!(!engine.script_report().truncated);
        let ok = engine.layout().runs[..engine.layout().run_count]
            .iter()
            .any(|r| r.bytes(engine.dom()).windows(9).any(|w| w == b"INLINE_OK"));
        assert!(ok, "inline script after MAX_PAGE_SCRIPTS external scripts must run");
    }

    #[test]
    fn oversized_inline_script_is_not_truncated_mid_statement() {
        let mut buf = [0u8; 8192];
        let mut pos = 0usize;
        let head = b"<html><body><script>var x = 1; ";
        buf[pos..pos + head.len()].copy_from_slice(head);
        pos += head.len();
        let stmt = b"x = x + 1; ";
        while pos <= head.len() + SCRIPT_BUF_BYTES {
            buf[pos..pos + stmt.len()].copy_from_slice(stmt);
            pos += stmt.len();
        }
        let tail = b"var bad = document.createElement('p'); bad.textContent = 'BAD'; document.body.appendChild(bad);</script><script>var ok = document.createElement('p'); ok.textContent = 'SAFE'; document.body.appendChild(ok);</script></body></html>";
        buf[pos..pos + tail.len()].copy_from_slice(tail);
        pos += tail.len();
        let html = core::str::from_utf8(&buf[..pos]).unwrap();
        let mut engine = NativeFlowEngine::new(720, 1234);
        assert!(engine.load_html(html, None).is_ok());
        assert!(engine.script_report().truncated);
        assert_eq!(engine.script_report().last_error, None);
        let has_safe = engine.layout().runs[..engine.layout().run_count]
            .iter()
            .any(|r| r.bytes(engine.dom()).windows(4).any(|w| w == b"SAFE"));
        assert!(has_safe, "subsequent fitting script runs without mid-statement syntax error");
    }
}
