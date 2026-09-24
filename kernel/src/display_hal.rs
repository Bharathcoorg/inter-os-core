//! Display Hardware Abstraction Layer (DisplayHal)
//!
//! Provides a clean, minimal trait interface for raw display scanout operations.
//! This HAL is the ONLY kernel-level display primitive. All consumer-facing UI
//! (home screen, app views, notification shade, etc.) should use this HAL
//! through the compositor/shell layer — never by directly writing pixels.
//!
//! The `FramebufferWriter` in `framebuffer.rs` implements this trait and currently
//! also contains bootstrap consumer UI rendering for development purposes.
//! In production, consumer rendering will migrate to userspace compositor processes.

/// Pixel format supported by the display hardware.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DisplayPixelFormat {
    Rgb888,
    Bgr888,
    Grayscale8,
}

/// Display hardware information.
#[derive(Debug, Clone, Copy)]
pub struct DisplayInfo {
    pub width: usize,
    pub height: usize,
    pub stride: usize,
    pub bytes_per_pixel: usize,
    pub pixel_format: DisplayPixelFormat,
}

/// Display Hardware Abstraction Layer trait.
///
/// Kernel-space components and the compositor use this trait to interact with
/// the raw display scanout buffer. This is intentionally minimal — it provides
/// only primitive pixel and rectangle operations.
///
/// Consumer UI rendering (app chrome, status bars, animations) belongs in
/// the compositor shell layer, NOT in HAL implementations.
pub trait DisplayHal {
    /// Returns display hardware information.
    fn display_info(&self) -> Option<DisplayInfo>;

    /// Writes a single pixel at (x, y) with the given RGB color.
    fn set_pixel(&mut self, x: usize, y: usize, r: u8, g: u8, b: u8);

    /// Fills a rectangle with a solid color.
    fn fill_rect(&mut self, x: usize, y: usize, w: usize, h: usize, r: u8, g: u8, b: u8);

    /// Draws a 1px outline rectangle.
    fn draw_rect_outline(&mut self, x: usize, y: usize, w: usize, h: usize, thickness: usize, r: u8, g: u8, b: u8);

    /// Clears the entire display to a solid color.
    fn clear(&mut self, r: u8, g: u8, b: u8);

    /// Draws an 8x8 bitmap character at the given position with optional scaling.
    fn draw_char(&mut self, x: usize, y: usize, ch: char, scale: usize, r: u8, g: u8, b: u8);

    /// Draws a string of characters at the given position.
    fn draw_text(&mut self, x: usize, y: usize, text: &str, scale: usize, r: u8, g: u8, b: u8);
}
