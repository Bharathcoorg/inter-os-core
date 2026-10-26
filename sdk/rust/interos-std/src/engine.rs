//! Browser Engine Abstraction for InterOS
//!
//! Provides the core traits, capabilities, and lifecycle states for web document
//! rendering engines (including native flow layout and external modern backends
//! like Servo in interos-apps).

/// Engine status under the InterOS Platform Truth Contract.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EngineStatus {
    Ready,
    Loading,
    Unavailable(&'static str),
    NotImplemented,
    Error(&'static str),
}

/// Explicit capabilities declared by a browser engine backend.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EngineCapabilities {
    pub supports_css_flexbox: bool,
    pub supports_css_grid: bool,
    pub supports_full_js: bool,
    pub supports_subframes: bool,
    pub supports_cookies: bool,
    pub supports_storage: bool,
    pub supports_streaming_render: bool,
}

impl EngineCapabilities {
    /// Capabilities of the native sovereign `#![no_std]` flow engine.
    pub const fn native_flow() -> Self {
        Self {
            supports_css_flexbox: false,
            supports_css_grid: false,
            supports_full_js: false, // Bounded AST interpreter
            supports_subframes: false,
            supports_cookies: true,
            supports_storage: false,
            supports_streaming_render: true,
        }
    }

    /// Full modern capabilities expected from upstream Servo or modern web engines.
    pub const fn modern_web() -> Self {
        Self {
            supports_css_flexbox: true,
            supports_css_grid: true,
            supports_full_js: true,
            supports_subframes: true,
            supports_cookies: true,
            supports_storage: true,
            supports_streaming_render: true,
        }
    }
}

/// Actions yielded by user interaction with an engine surface.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EngineAction {
    None,
    Navigate([u8; 256], usize),
    OpenExternal([u8; 256], usize),
    FocusInput(u32),
    Download([u8; 256], usize),
}

/// Unified, versioned browser engine interface separating UI from document rendering.
pub trait BrowserEngine {
    /// Human-readable engine identifier (e.g. "Native Flow", "Servo Modern Web").
    fn name(&self) -> &'static str;

    /// Engine implementation version.
    fn version(&self) -> &'static str;

    /// Truthful runtime status.
    fn status(&self) -> EngineStatus;

    /// Explicit capabilities supported by this backend.
    fn capabilities(&self) -> EngineCapabilities;

    /// Navigates to a remote or local URL.
    fn load_url(&mut self, url: &str) -> Result<(), &'static str>;

    /// Renders raw HTML markup from memory.
    fn load_html(&mut self, html: &str, base_url: Option<&str>) -> Result<(), &'static str>;

    /// Returns the current (width, height) viewport bounds.
    fn viewport(&self) -> (u32, u32);

    /// Sets the viewport dimensions.
    fn set_viewport(&mut self, w: u32, h: u32);

    /// Applies vertical/horizontal scroll delta.
    fn scroll(&mut self, dy: i32);

    /// Computes pixel color (0x00RRGGBB or 0xFFAABBGGRR) at viewport coordinates.
    fn render_pixel(&self, x: usize, y: usize) -> u32;

    /// Dispatches touch input to the document.
    fn handle_touch(&mut self, x: u32, y: u32, kind: u8) -> EngineAction;

    /// Current document title.
    fn title(&self) -> &str;

    /// Current document URL.
    fn url(&self) -> &str;
}
