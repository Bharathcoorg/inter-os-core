//! Canonical InterOS visual language: one theme, applied everywhere.
//!
//! WHY this is a struct and not a pile of constants: the previous design exposed
//! bare `pub const` colours, and every screen inlined its own geometry. That made
//! three things impossible, all of which were requirements:
//!
//!   * Switching appearance. Nothing could be swapped because nothing was a value.
//!   * Deriving one surface from another. A card could not ask "what is one step
//!     more raised than `surface`?" so each screen retyped a near-identical literal.
//!   * Consistency. The same idea existed as four slightly different hex codes on
//!     four screens, which is how a UI starts to look assembled rather than designed.
//!
//! A `Theme` is data. Two instances ship â€” [`Theme::LIGHT`] and [`Theme::DARK`] â€”
//! selected at runtime, and every screen reads from whichever is active. Adding a
//! third theme means adding a value, not editing sixteen screens.
//!
//! Nothing here draws. Layout lives in `layout.rs` and glyph rasterisation in
//! `text.rs`, so a theme can be swapped without touching either.

/// Corner radius scale, in logical pixels at the 720x1440 surface.
pub mod radius {
    /// Chips, badges, small controls.
    pub const CHIP: usize = 10;
    /// Buttons and interactive tiles.
    pub const CONTROL: usize = 18;
    /// Cards and list rows.
    pub const CARD: usize = 24;
    /// Sheets and full-width panels.
    pub const SHEET: usize = 32;
    /// The device screen itself.
    pub const SCREEN: usize = 44;
}

/// Spacing scale, in logical pixels. Every gap in every screen is one of these.
///
/// WHY a fixed scale rather than ad-hoc numbers: an 8pt grid is what makes a UI
/// read as designed. Off-grid values are individually unremarkable and
/// collectively the reason a layout looks slightly wrong without any single
/// element being obviously wrong.
pub mod space {
    pub const XS: usize = 4;
    pub const SM: usize = 8;
    pub const MD: usize = 16;
    pub const LG: usize = 24;
    pub const XL: usize = 32;
    pub const XXL: usize = 48;
}

/// Type scale. `FONT_8X8` rasterises at integer multiples only, so these are the
/// permitted multipliers rather than arbitrary point sizes.
///
/// WHY these are not the old values: they were sized for an 8x8 cell, where cap
/// height was 8 x scale. The proportional face has `CAP_HEIGHT = 17`, so the same
/// multiplier produced roughly double the intended cap height -- the dashboard
/// heading needed a 120px cell in a 79px gap and was painted over. Each rung is now
/// chosen from a cap height appropriate to its job.
///
/// `BADGE` and `CAPTION` coincide at 1 because a 17-row cap cannot express a
/// smaller integer step. That is the honest consequence of the face, and it is why
/// badges must be told apart from captions by ink weight rather than by size.
pub mod type_scale {
    /// Badge glyphs inside icons, e.g. "TEL".
    pub const BADGE: usize = 1;
    /// Dense metadata: status-bar clock, captions, chip labels.
    pub const CAPTION: usize = 1;
    /// Body copy and list labels. One 24px cell fits a 52px row.
    pub const BODY: usize = 2;
    /// Section headings. 3 keeps a 72px cell inside a 79px gap.
    pub const TITLE: usize = 3;
    /// The lock-screen clock and other display type. 107.9 / 17 = 6.35 -> 6.
    pub const DISPLAY: usize = 6;
}

/// Logical surface size. Every layout computation is expressed against this.
pub const SCREEN_WIDTH: usize = 720;
pub const SCREEN_HEIGHT: usize = 1440;

/// Minimum edge length of an interactive target.
///
/// WHY 48: it is the smallest square that is reliably hittable with a fingertip.
/// The lifecycle harness taps at coordinates derived from the design, so anything
/// smaller stops being a convenience and starts being a test hazard.
pub const MIN_TOUCH_TARGET: usize = 48;

pub const STATUS_BAR_HEIGHT: usize = 48;
pub const NAV_BAR_HEIGHT: usize = 64;
pub const CONTENT_PADDING: usize = 40;

/// A complete visual language.
///
/// Colours are `0xAARRGGBB`. Everything a screen draws is derived from a field
/// here, so a theme change is a value change and never a per-screen edit.
#[derive(Clone, Copy)]
pub struct Theme {
    pub name: &'static str,

    /// Page background, behind everything.
    pub bg: u32,
    /// Resting card and sheet fill.
    pub surface: u32,
    /// One elevation step above `surface`: pressed states, nested cards.
    pub surface_raised: u32,
    /// Recessed fill: wells, tracks, disabled fields.
    pub surface_sunken: u32,
    /// Hairline separators and card outlines.
    pub border: u32,

    /// Primary body and heading ink.
    pub text: u32,
    /// Secondary ink: captions, metadata, units.
    pub text_muted: u32,
    /// Ink on a saturated or dark fill.
    pub text_on_accent: u32,

    /// Brand accent.
    pub accent: u32,
    pub success: u32,
    pub warning: u32,
    pub danger: u32,

    /// Chrome that floats over content: status bar, docks, sheets.
    pub overlay: u32,
    /// Divider drawn over `overlay`.
    pub overlay_border: u32,

    /// Wallpaper ramp stops, top to bottom. The lock screen interpolates these.
    ///
    /// WHY a ramp and not a bitmap: a full-bleed photographic wallpaper at 720x1440
    /// is 3 MB of asset to carry in a binary, and no guest-side scaler can add
    /// detail that is not there. A ramp plus traced silhouettes reproduces the
    /// composition at a size the platform can actually render per frame.
    pub wallpaper_top: u32,
    pub wallpaper_mid: u32,
    pub wallpaper_bottom: u32,
    /// Ink used for type drawn over the wallpaper's upper region, where it is bright.
    pub wallpaper_ink: u32,

    /// Ink to place on a *light* backdrop that is not part of this theme, such as a
    /// status fill or a user's chosen colour.
    ///
    /// WHY this is separate from `text` and `text_on_accent`: `text` is the
    /// theme's own body ink and is therefore light on a dark theme, while
    /// `text_on_accent` is for saturated fills and can itself be near-black on a
    /// dark theme. Neither is correct as "the ink for whatever colour this pixel
    /// happens to be", which is what a contrast helper must return. Deriving the
    /// answer from `text_on_accent` returned the dark theme's own background as
    /// its ink -- invisible text on an invisible background.
    pub ink_on_light: u32,
    /// Ink to place on a dark backdrop outside this theme's palette.
    pub ink_on_dark: u32,
}

impl Theme {
    /// The light system: near-white pages, white cards, dark ink.
    pub const LIGHT: Theme = Theme {
        name: "light",
        bg: 0xFFF7F6F2,
        surface: 0xFFFFFFFF,
        surface_raised: 0xFFEFEEE9,
        surface_sunken: 0xFFE9E8E3,
        border: 0xFFE1E0DB,
        text: 0xFF111214,
        text_muted: 0xFF777773,
        text_on_accent: 0xFFFFFFFF,
        accent: 0xFF2563EB,
        success: 0xFF2E8B57,
        warning: 0xFFD97706,
        danger: 0xFFDC2626,
        overlay: 0xFFFFFFFF,
        overlay_border: 0xFFE1E0DB,
        wallpaper_top: 0xFF9CB8D6,
        wallpaper_mid: 0xFF6E7F8C,
        wallpaper_bottom: 0xFF241F1C,
        wallpaper_ink: 0xFF111214,
        ink_on_light: 0xFF111214,
        ink_on_dark: 0xFFFFFFFF,
    };

    /// The dark system: near-black pages, lifted cards, light ink.
    ///
    /// WHY the reference sheet's Activity Space, Quick Controls and Power Menu are
    /// dark while Events, Files and Gallery are light: a phone that inverts its
    /// surfaces by context reads as one system, and a phone with two unrelated
    /// palettes reads as two products.
    pub const DARK: Theme = Theme {
        name: "dark",
        bg: 0xFF111214,
        surface: 0xFF25282A,
        surface_raised: 0xFF2F3235,
        surface_sunken: 0xFF1A1C1E,
        border: 0xFF35383D,
        text: 0xFFF5F6F7,
        text_muted: 0xFF9AA0A6,
        text_on_accent: 0xFF111214,
        accent: 0xFF38BDF8,
        success: 0xFF34D399,
        warning: 0xFFFBBF24,
        danger: 0xFFF87171,
        overlay: 0xFF1A1C1E,
        overlay_border: 0xFF35383D,
        wallpaper_top: 0xFF1B2733,
        wallpaper_mid: 0xFF12161A,
        wallpaper_bottom: 0xFF07080A,
        wallpaper_ink: 0xFFF9F9F8,
        ink_on_light: 0xFF111214,
        ink_on_dark: 0xFFFFFFFF,
    };

    /// Selects a theme by name, defaulting to light for an unknown name.
    pub fn by_name(name: &str) -> Theme {
        match name {
            "dark" => Theme::DARK,
            _ => Theme::LIGHT,
        }
    }

    /// Whether this theme is dark, i.e. whether ink is lighter than its backdrop.
    pub fn is_dark(&self) -> bool {
        self.name == "dark"
    }

    /// Blends two `0xAARRGGBB` values. `t` is 0..=256, matching the wallpaper
    /// ramp's fixed-point convention so callers can pass a horizontal position
    /// straight through without rescaling.
    pub fn blend(a: u32, b: u32, t: u32) -> u32 {
        let t = t.min(256);
        let inv = 256 - t;
        let mut out = 0u32;
        for shift in [24u32, 16, 8, 0] {
            let ca = (a >> shift) & 0xFF;
            let cb = (b >> shift) & 0xFF;
            let mixed = (ca * inv + cb * t) >> 8;
            out |= mixed << shift;
        }
        out
    }

    /// The colour of `t` across a vertical ramp of three stops, `t` in 0..=256.
    pub fn ramp3(&self, t: u32) -> u32 {
        let t = t.min(256);
        if t < 128 {
            Theme::blend(self.wallpaper_top, self.wallpaper_mid, t * 2)
        } else {
            Theme::blend(self.wallpaper_mid, self.wallpaper_bottom, (t - 128) * 2)
        }
    }

    /// Applies a translucent white or black veil, for pressed states and for
    /// controls that float over the wallpaper.
    ///
    /// WHY a helper: the lock screen needed the same 27% and 36% veils in three
    /// places, and they had already drifted to 27%, 27% and 30%.
    ///
    /// The target is an OPAQUE colour in the veil's own direction: white for
    /// `light`, black otherwise. It used to be built as `(target << 24) |
    /// 0xFFFFFF`, which kept 0xFFFFFF in the RGB channels for BOTH directions,
    /// so `light == false` blended towards white instead of black: the lock
    /// screen's PIN pad backdrop (`veil(wallpaper, 85, false)`) washed out to
    /// roughly #F3F3F5 (measured: a flat 220-255 range where the wallpaper
    /// should be), and its white heading and keypad digits -- drawn at 255 on a
    /// backdrop of 243-255 -- had a contrast of 10/255 and were effectively
    /// invisible.
    pub fn veil(base: u32, percent: u32, light: bool) -> u32 {
        let p = percent.min(100);
        let t = (p * 256) / 100;
        let target = if light { 0xFFFFFFFFu32 } else { 0xFF000000u32 };
        Theme::blend(base, target, t)
    }
}

// The original constants are retained because 214 contract tests pin several of
// them by literal, and because migrating call sites is mechanical. New code must
// read from `Theme` instead. These aliases exist so existing code keeps compiling
// while screens are migrated one at a time.
pub const BG_DARK: u32 = Theme::LIGHT.bg;
pub const SURFACE_DARK: u32 = Theme::LIGHT.surface;
pub const SURFACE_RAISED: u32 = Theme::LIGHT.surface_raised;
pub const SURFACE_LIGHT: u32 = Theme::LIGHT.surface;
pub const BORDER_SUBTLE: u32 = Theme::LIGHT.border;
pub const TEXT_PRIMARY: u32 = Theme::LIGHT.text;
pub const TEXT_MUTED: u32 = Theme::LIGHT.text_muted;
pub const TEXT_DARK: u32 = Theme::LIGHT.text;
pub const ACCENT_SKY: u32 = Theme::LIGHT.accent;
pub const ACCENT_AMBER: u32 = Theme::LIGHT.warning;
pub const SUCCESS_EMERALD: u32 = Theme::LIGHT.success;
pub const DANGER_CRIMSON: u32 = Theme::LIGHT.danger;
pub const ACCENT_CYAN: u32 = Theme::LIGHT.accent;
pub const ACCENT_VIOLET: u32 = 0xFF7C3AED;
pub const LOCK_BACKDROP_TOP: u32 = Theme::LIGHT.wallpaper_top;
pub const LOCK_BACKDROP_BOTTOM: u32 = Theme::LIGHT.wallpaper_bottom;