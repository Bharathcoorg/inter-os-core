//! Video player design tokens, built strictly on InterOS Theme::DARK.

pub use interos_std::design::radius;
pub use interos_std::design::spacing;
pub use interos_std::design::theme::{Theme, TypeScale};
pub use interos_std::design::typography;

/// Canonical dark theme for media playback surfaces.
pub const THEME: Theme = Theme::DARK;

/// The platform's standard type scale.
pub const TYPE_SCALE: TypeScale = TypeScale::STANDARD;

/// The platform's type scale rungs under standard names.
pub mod type_scale {
    pub use interos_std::design::typography::BADGE_SCALE as BADGE;
    pub use interos_std::design::typography::CAPTION_SCALE as CAPTION;
    pub use interos_std::design::typography::BODY_SCALE as BODY;
    pub use interos_std::design::typography::TITLE_SCALE as TITLE;
    pub use interos_std::design::typography::SECTION_SCALE as SECTION;
    pub use interos_std::design::typography::HEADING_SCALE as HEADING;
    pub use interos_std::design::typography::DISPLAY_SCALE as DISPLAY;
}

/// Surface background (#111214).
pub const COLOR_BG: u32 = 0xFF111214;
/// Elevated card/canvas background (#25282A).
pub const COLOR_SURFACE: u32 = 0xFF25282A;
/// Dark surface sunken (#16181A).
pub const COLOR_SURFACE_SUNKEN: u32 = 0xFF16181A;
/// Dark surface card fill for sheets (#1E2124).
pub const COLOR_SHEET_CARD: u32 = 0xFF1E2124;
/// Border / hairline color (#35393D).
pub const COLOR_BORDER: u32 = 0xFF35393D;
/// Primary text and high-contrast glyphs (#FFFFFF).
pub const COLOR_TEXT_PRIMARY: u32 = 0xFFFFFFFF;
/// Secondary / muted text (#8A9199).
pub const COLOR_TEXT_MUTED: u32 = 0xFF8A9199;
/// Transport Play disc fill (#FFFFFF).
pub const COLOR_PLAY_FILL: u32 = 0xFFFFFFFF;
/// Transport Play glyph on white disc (#111214).
pub const COLOR_PLAY_GLYPH: u32 = 0xFF111214;
/// Scrubber active progress bar fill (#FFFFFF).
pub const COLOR_PROGRESS_ACTIVE: u32 = 0xFFFFFFFF;
/// Scrubber inactive background bar (#35393D).
pub const COLOR_PROGRESS_BG: u32 = 0xFF35393D;
/// Disabled / unavailable control ink.
pub const COLOR_DISABLED: u32 = 0xFF4A4E54;
/// Accent / highlight color.
pub const COLOR_ACCENT: u32 = 0xFFE0E0E0;

/// Minimum touch target size (44px, preferred 48px).
pub const MIN_TOUCH_TARGET: usize = interos_std::design::touch::MIN_TARGET;
