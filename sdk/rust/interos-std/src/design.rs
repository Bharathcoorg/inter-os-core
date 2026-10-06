//! InterOS Core visual design tokens shared by native Rust applications.
//!
//! These are semantic tokens, not app-specific styling. Apps should compose
//! surfaces, spacing and typography from this module so the launcher, system
//! line and native applications retain one visual language.

pub mod color {
    // Canonical light system
    pub const BACKGROUND: u32 = 0xFFF7F6F2;
    pub const SURFACE: u32 = 0xFFFFFFFF;
    pub const SURFACE_SOFT: u32 = 0xFFEFEEE9;
    pub const SURFACE_RAISED: u32 = 0xFFEFEEE9;
    pub const BORDER: u32 = 0xFFE1E0DB;
    pub const TEXT: u32 = 0xFF111214;
    pub const TEXT_MUTED: u32 = 0xFF777773;

    // Canonical dark surfaces
    pub const DARK_BACKGROUND: u32 = 0xFF111214;
    pub const DARK_SURFACE: u32 = 0xFF25282A;
    pub const DARK_BORDER: u32 = 0xFF35383D;
    pub const DARK_TEXT: u32 = 0xFFF5F6F7;
    pub const DARK_TEXT_MUTED: u32 = 0xFF9AA0A6;

    // Restrained semantic accents
    pub const ACCENT: u32 = 0xFF2563EB;
    pub const SUCCESS: u32 = 0xFF2E8B57;
    pub const WARNING: u32 = 0xFFD97706;
    pub const DANGER: u32 = 0xFFDC2626;
    pub const EMERGENCY: u32 = 0xFFFF5555;
}

pub mod spacing {
    pub const GRID_4: usize = 4;
    pub const GRID_8: usize = 8;
    pub const GRID_12: usize = 12;
    pub const GRID_16: usize = 16;
    pub const GRID_20: usize = 20;
    pub const GRID_24: usize = 24;
    pub const GRID_32: usize = 32;
    pub const GRID_40: usize = 40;
    pub const GRID_48: usize = 48;
    pub const GRID_64: usize = 64;

    pub const XS: usize = 4;
    pub const SM: usize = 8;
    pub const MD: usize = 16;
    pub const LG: usize = 24;
    pub const XL: usize = 32;
    pub const XXL: usize = 48;
}

pub mod radius {
    pub const SMALL: usize = 8;
    pub const CONTROL: usize = 12;
    pub const CARD: usize = 18;
    pub const SHEET: usize = 24;
    pub const MAJOR: usize = 32;
    pub const PILL: usize = 999;

    pub const SM: u32 = 12;
    pub const MD: u32 = 18;
    pub const LG: u32 = 24;
}

pub mod touch {
    pub const MIN_TARGET: usize = 44;
    pub const PREFERRED_TARGET: usize = 48;
    pub const PRIMARY_ACTION: usize = 56;
}

pub mod motion {
    pub const PRESS_FEEDBACK_MS: u32 = 100;
    pub const QUICK_CONTROLS_MS: u32 = 150;
    pub const STANDARD_TRANSITION_MS: u32 = 200;
    pub const MAJOR_TRANSITION_MS: u32 = 300;
    pub const ACTIVITY_EXPANSION_MS: u32 = 280;

    pub const FAST: u32 = 120;
    pub const STANDARD: u32 = 220;
    pub const EMPHASIZED: u32 = 360;
}
