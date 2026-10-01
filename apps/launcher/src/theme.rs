//! Canonical stock InterOS smartphone visual tokens.
//!
//! Keep consumer UI styling in one place. Alternate themes are intentionally
//! outside this contract so every system surface shares the same geometry.

pub const BG_DARK: u32 = 0xFFF7F8FA;
pub const SURFACE_DARK: u32 = 0xFFFFFFFF;
pub const SURFACE_RAISED: u32 = 0xFFF0F2F5;
pub const SURFACE_LIGHT: u32 = 0xFFFFFFFF;
pub const BORDER_SUBTLE: u32 = 0xFFE2E8F0;
pub const TEXT_PRIMARY: u32 = 0xFF0F172A;
pub const TEXT_MUTED: u32 = 0xFF64748B;
pub const TEXT_DARK: u32 = 0xFF0F172A;
pub const ACCENT_SKY: u32 = 0xFF0284C7;
pub const ACCENT_AMBER: u32 = 0xFFF59E0B;
pub const SUCCESS_EMERALD: u32 = 0xFF10B981;
pub const DANGER_CRIMSON: u32 = 0xFFDC2626;
pub const ACCENT_CYAN: u32 = 0xFF0284C7;
pub const ACCENT_VIOLET: u32 = 0xFF7C3AED;

// Lock screen backdrop.
//
// The lock screen is the one surface that draws opaque white type and
// translucent dark control overlays, so it needs a dark canvas to stay legible.
// Everything above the lock screen (home, app views, settings) stays on the
// light palette defined above, matching a conventional mobile colour scheme.
pub const LOCK_BACKDROP_TOP: u32 = 0xFF0B1220;
pub const LOCK_BACKDROP_BOTTOM: u32 = 0xFF05080F;

pub const SCREEN_WIDTH: usize = 720;
pub const SCREEN_HEIGHT: usize = 1440;
pub const CONTENT_PADDING: usize = 40;
pub const MIN_TOUCH_TARGET: usize = 48;
pub const STATUS_BAR_HEIGHT: usize = 48;
pub const NAV_BAR_HEIGHT: usize = 64;
