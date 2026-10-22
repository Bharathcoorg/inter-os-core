//! InterOS Core visual design tokens shared by native Rust applications.
//!
//! These are semantic tokens, not app-specific styling. Apps should compose
//! surfaces, spacing and typography from this module so the launcher, system
//! line and native applications retain one visual language.

pub mod walls;
pub mod face;
pub mod typeface;
pub mod draw_target;
pub mod outline_text;
pub mod display_list;
pub mod scroll;
pub mod theme;
pub mod components;
pub mod monochrome;
pub mod node;
pub mod accessibility;
#[cfg(test)]
mod cap_ladder;

pub use theme::{AccentPalette, StockApp, Theme};

pub mod typography {
    use crate::font;

    pub const BASELINE_GRID_DP: usize = 8;

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct TextStyle {
        pub scale: usize,
        pub line_height_px: usize,
        pub baseline_offset_px: usize,
    }

    /// The permitted multipliers, named on their own.
    ///
    /// WHY these are separate from the styles below: the rasteriser draws at
    /// integer multiples only, so a caller that wants "a caption" needs the
    /// number as well as the line box, and a style that carried the number
    /// only inside a struct would force every such caller to unpack it.
    pub const BADGE_SCALE: usize = 1;
    pub const CAPTION_SCALE: usize = 1;
    pub const BODY_SCALE: usize = 2;
    /// A section heading: the card title and the heading above a group.
    pub const SECTION_SCALE: usize = 3;
    /// A page title: the name of the screen.
    pub const TITLE_SCALE: usize = 4;
    /// A page heading: the largest a title may be.
    pub const HEADING_SCALE: usize = 5;
    pub const DISPLAY_SCALE: usize = 6;

    /// The cap heights, in pixels, of the platform's type scale.
    ///
    /// WHY these exist beside the scales: the scales are *multiples* of the
    /// bitmap face's sixteen-pixel cell, and the multiples available are
    /// sixteen, thirty-two, forty-eight and ninety-six. The design
    /// document's scale asks for a caption of twelve to fourteen pixels, a
    /// section heading of twenty to twenty-four, a title of twenty-eight to
    /// thirty-two and a page heading of thirty-six to forty-four -- so two
    /// of the six sizes the document names had no rung to sit on, and an
    /// app that wanted one either overshot or had to reach for a different
    /// face. These name the sizes directly, in the units a reader sees, and
    /// `outline_text::place_run_cap` draws them.
    ///
    /// The values are the middle of each range the design document gives.
    /// The cap heights, in pixels, of the platform's type scale.
    ///
    /// WHY a cap height and not a font size: measuring the reference screens
    /// settles the reading of the document's numbers. On the reference
    /// calculator -- a seven-hundred and twenty pixel wide screen -- the
    /// status clock stands thirteen pixels tall, a key label fifteen, the
    /// title twenty-three and the result thirty-one, and each of those
    /// falls inside the range the document gives for the role it plays. A
    /// ladder written in font sizes would have drawn every one of them a
    /// third smaller.
    ///
    /// The em for a rung follows from its cap height, and the renderer
    /// draws it with the outline face.
    pub const CAPTION_CAP_PX: u32 = font::cap_for_scale(CAPTION_SCALE);
    pub const BODY_CAP_PX: u32 = font::cap_for_scale(BODY_SCALE);
    pub const SECTION_CAP_PX: u32 = font::cap_for_scale(3);
    pub const TITLE_CAP_PX: u32 = font::cap_for_scale(4);
    pub const HEADING_CAP_PX: u32 = font::cap_for_scale(5);
    pub const DISPLAY_CAP_PX: u32 = font::cap_for_scale(6);

    /// The size, in pixels of an em, each rung draws at.
    pub const CAPTION_EM_PX: u32 = font::em_for_scale(CAPTION_SCALE);
    pub const BODY_EM_PX: u32 = font::em_for_scale(BODY_SCALE);
    pub const SECTION_EM_PX: u32 = font::em_for_scale(3);
    pub const TITLE_EM_PX: u32 = font::em_for_scale(4);
    pub const HEADING_EM_PX: u32 = font::em_for_scale(5);
    pub const DISPLAY_EM_PX: u32 = font::em_for_scale(6);

    /// The line boxes, in pixels, that go with the cap heights above.
    ///
    /// WHY the line box is named and not derived: a line box is the space a
    /// run reserves, and it is a layout decision -- two labels on one row
    /// have to agree on it -- rather than a property of the glyphs.
    pub const CAPTION_LINE_PX: usize = font::line_height(CAPTION_SCALE);
    pub const BODY_LINE_PX: usize = font::line_height(BODY_SCALE);
    pub const SECTION_LINE_PX: usize = font::line_height(3);
    pub const TITLE_LINE_PX: usize = font::line_height(4);
    pub const HEADING_LINE_PX: usize = font::line_height(5);
    pub const DISPLAY_LINE_PX: usize = font::line_height(6);

    /// Badge glyphs inside an icon, e.g. a call badge.
    ///
    /// WHY it exists: the shell draws a badge in the corner of an icon, and the
    /// badge is a caption at the smallest step the face can express. It was
    /// carried by the launcher alone, which meant the step existed only there.
    /// WHY the numbers are the design document's: a style is what an app
    /// copies when it asks for "a caption", so a style that reserved the
    /// bitmap cell's multiples reserved two and a half times the space a
    /// reader expects and pushed the neighbouring content out of the way.
    /// The line boxes and baselines are the ones the document names, and
    /// the baseline is the row the renderer sits the ink on at that rung.
    pub const BADGE: TextStyle = TextStyle {
        scale: BADGE_SCALE,
        line_height_px: 16,
        baseline_offset_px: font::ascent_for_scale(BADGE_SCALE),
    };

    pub const CAPTION: TextStyle = TextStyle {
        scale: CAPTION_SCALE,
        line_height_px: font::line_height(CAPTION_SCALE),
        baseline_offset_px: font::ascent_for_scale(CAPTION_SCALE),
    };

    pub const BODY: TextStyle = TextStyle {
        scale: BODY_SCALE,
        line_height_px: font::line_height(BODY_SCALE),
        baseline_offset_px: font::ascent_for_scale(BODY_SCALE),
    };

    pub const TITLE: TextStyle = TextStyle {
        scale: TITLE_SCALE,
        line_height_px: font::line_height(TITLE_SCALE),
        baseline_offset_px: font::ascent_for_scale(TITLE_SCALE),
    };

    /// A section heading: the title of a card and the heading above a group
    /// of rows.
    pub const SECTION: TextStyle = TextStyle {
        scale: SECTION_SCALE,
        line_height_px: font::line_height(SECTION_SCALE),
        baseline_offset_px: font::ascent_for_scale(SECTION_SCALE),
    };

    /// A page heading: the largest a title may be drawn.
    pub const HEADING: TextStyle = TextStyle {
        scale: HEADING_SCALE,
        line_height_px: font::line_height(HEADING_SCALE),
        baseline_offset_px: font::ascent_for_scale(HEADING_SCALE),
    };

    pub const DISPLAY: TextStyle = TextStyle {
        scale: DISPLAY_SCALE,
        line_height_px: font::line_height(6),
        baseline_offset_px: font::ascent_for_scale(6),
    };
}

/// Canonical display unit: Logical Density-independent Pixels (dp).
#[repr(transparent)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct Dp(pub i32);

impl core::ops::Add for Dp {
    type Output = Self;
    #[inline]
    fn add(self, rhs: Self) -> Self {
        Self(self.0 + rhs.0)
    }
}

impl core::ops::Sub for Dp {
    type Output = Self;
    #[inline]
    fn sub(self, rhs: Self) -> Self {
        Self(self.0 - rhs.0)
    }
}

impl core::ops::AddAssign for Dp {
    #[inline]
    fn add_assign(&mut self, rhs: Self) {
        self.0 += rhs.0;
    }
}

impl core::ops::SubAssign for Dp {
    #[inline]
    fn sub_assign(&mut self, rhs: Self) {
        self.0 -= rhs.0;
    }
}

impl core::ops::Neg for Dp {
    type Output = Self;
    #[inline]
    fn neg(self) -> Self {
        Self(-self.0)
    }
}

/// Canonical display unit: Physical Device Pixels (px).
#[repr(transparent)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct Px(pub i32);

impl core::ops::Add for Px {
    type Output = Self;
    #[inline]
    fn add(self, rhs: Self) -> Self {
        Self(self.0 + rhs.0)
    }
}

impl core::ops::Sub for Px {
    type Output = Self;
    #[inline]
    fn sub(self, rhs: Self) -> Self {
        Self(self.0 - rhs.0)
    }
}

impl core::ops::AddAssign for Px {
    #[inline]
    fn add_assign(&mut self, rhs: Self) {
        self.0 += rhs.0;
    }
}

impl core::ops::SubAssign for Px {
    #[inline]
    fn sub_assign(&mut self, rhs: Self) {
        self.0 -= rhs.0;
    }
}

impl core::ops::Neg for Px {
    type Output = Self;
    #[inline]
    fn neg(self) -> Self {
        Self(-self.0)
    }
}

/// Display physical orientation.
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Orientation {
    #[default]
    Portrait = 0,
    LandscapeRight = 1,
    PortraitInverted = 2,
    LandscapeLeft = 3,
}

impl Orientation {
    #[inline]
    pub const fn is_portrait(self) -> bool {
        matches!(self, Orientation::Portrait | Orientation::PortraitInverted)
    }

    #[inline]
    pub const fn is_landscape(self) -> bool {
        matches!(self, Orientation::LandscapeRight | Orientation::LandscapeLeft)
    }
}

/// System safe area insets in physical pixels.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct SafeAreaInsetsPx {
    pub top: Px,
    pub bottom: Px,
    pub left: Px,
    pub right: Px,
}

impl SafeAreaInsetsPx {
    pub const ZERO: Self = Self {
        top: Px(0),
        bottom: Px(0),
        left: Px(0),
        right: Px(0),
    };
}

/// Canonical Display Metrics Contract for InterOS.
///
/// Separates physical raster dimensions, logical design units (dp), density,
/// orientation, safe areas, and refresh capabilities.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DisplayMetrics {
    pub physical_width_px: u32,
    pub physical_height_px: u32,
    pub logical_width_dp: u32,
    pub logical_height_dp: u32,
    pub density_dpi: u32,
    pub scale_factor_permille: u32,
    pub orientation: Orientation,
    pub safe_insets_px: SafeAreaInsetsPx,
    pub refresh_rate_hz: u16,
}

impl DisplayMetrics {
    /// Canonical flagship phone profile (1080x2160 FHD+ physical, 720x1440 dp logical, 480 DPI).
    pub const fn canonical_phone() -> Self {
        Self {
            physical_width_px: 1080,
            physical_height_px: 2160,
            logical_width_dp: 720,
            logical_height_dp: 1440,
            density_dpi: 480,
            scale_factor_permille: 1500,
            orientation: Orientation::Portrait,
            safe_insets_px: SafeAreaInsetsPx {
                top: Px(72),
                bottom: Px(72),
                left: Px(0),
                right: Px(0),
            },
            refresh_rate_hz: 60,
        }
    }

    /// Converts a logical DP measurement to physical pixels using rounding half-up.
    #[inline]
    pub const fn dp_to_px(&self, dp: Dp) -> Px {
        let permille = self.scale_factor_permille as i64;
        let v = dp.0 as i64;
        let sign = if v >= 0 { 1 } else { -1 };
        let abs_v = if v >= 0 { v } else { -v };
        let px = (abs_v * permille + 500) / 1000;
        Px((sign * px) as i32)
    }

    /// Converts physical pixels to logical DP using rounding half-up.
    #[inline]
    pub const fn px_to_dp(&self, px: Px) -> Dp {
        let permille = self.scale_factor_permille as i64;
        if permille == 0 {
            return Dp(0);
        }
        let v = px.0 as i64;
        let sign = if v >= 0 { 1 } else { -1 };
        let abs_v = if v >= 0 { v } else { -v };
        let dp = (abs_v * 1000 + (permille / 2)) / permille;
        Dp((sign * dp) as i32)
    }

    /// Returns the exact physical pixel hairline width (always 1 physical pixel).
    #[inline]
    pub const fn hairline_px(&self) -> Px {
        Px(1)
    }

    /// Frame budget in nanoseconds for the configured refresh rate.
    #[inline]
    pub const fn frame_budget_ns(&self) -> u64 {
        Self::frame_budget_ns_for_hz(self.refresh_rate_hz)
    }

    /// Frame budget in nanoseconds for a specified refresh rate (e.g. 60, 120, 144 Hz).
    #[inline]
    pub const fn frame_budget_ns_for_hz(hz: u16) -> u64 {
        if hz == 0 {
            16_666_666
        } else {
            1_000_000_000 / (hz as u64)
        }
    }

    /// Returns a new `DisplayMetrics` adapted for the given `Orientation`.
    #[inline]
    pub fn with_orientation(&self, new_orientation: Orientation) -> Self {
        let was_portrait = self.orientation.is_portrait();
        let will_be_portrait = new_orientation.is_portrait();

        let (new_pw, new_ph, new_lw, new_lh) = if was_portrait != will_be_portrait {
            (
                self.physical_height_px,
                self.physical_width_px,
                self.logical_height_dp,
                self.logical_width_dp,
            )
        } else {
            (
                self.physical_width_px,
                self.physical_height_px,
                self.logical_width_dp,
                self.logical_height_dp,
            )
        };

        let new_insets = match new_orientation {
            Orientation::Portrait => SafeAreaInsetsPx {
                top: self.safe_insets_px.top,
                bottom: self.safe_insets_px.bottom,
                left: self.safe_insets_px.left,
                right: self.safe_insets_px.right,
            },
            Orientation::LandscapeRight => SafeAreaInsetsPx {
                top: self.safe_insets_px.left,
                bottom: self.safe_insets_px.right,
                left: self.safe_insets_px.bottom,
                right: self.safe_insets_px.top,
            },
            Orientation::PortraitInverted => SafeAreaInsetsPx {
                top: self.safe_insets_px.bottom,
                bottom: self.safe_insets_px.top,
                left: self.safe_insets_px.right,
                right: self.safe_insets_px.left,
            },
            Orientation::LandscapeLeft => SafeAreaInsetsPx {
                top: self.safe_insets_px.right,
                bottom: self.safe_insets_px.left,
                left: self.safe_insets_px.top,
                right: self.safe_insets_px.bottom,
            },
        };

        Self {
            physical_width_px: new_pw,
            physical_height_px: new_ph,
            logical_width_dp: new_lw,
            logical_height_dp: new_lh,
            density_dpi: self.density_dpi,
            scale_factor_permille: self.scale_factor_permille,
            orientation: new_orientation,
            safe_insets_px: new_insets,
            refresh_rate_hz: self.refresh_rate_hz,
        }
    }
}

/// The platform palette: the colours that exist.
///
/// WHY a palette and not the interface: this module says which colours the
/// platform has. It does not say which one a card is filled with, which one a
/// caption is inked in, or which one marks the selected tab -- that is
/// [`super::theme`]'s job, and an interface asks the theme for a role.
///
/// WHY the light and dark halves are one module: they are the same palette
/// read against two surfaces, so a colour that has no dark counterpart is a gap
/// in the palette rather than a separate scheme.
pub mod color {
    // Canonical light system
    pub const BACKGROUND: u32 = 0xFFF7F6F2;
    pub const SURFACE: u32 = 0xFFFFFFFF;
    pub const SURFACE_RAISED: u32 = 0xFFEFEEE9;
    /// The same fill as [`SURFACE_RAISED`], under the name an app outside the
    /// design-system work still uses.
    ///
    /// WHY it points at the other constant and does not repeat the number: two
    /// spellings of one value is how a palette stops being a palette, and an
    /// alias carries the value without carrying a second copy of it. The
    /// duplicate spelling goes when the app that uses it is migrated.
    pub const SURFACE_SOFT: u32 = SURFACE_RAISED;
    /// A recessed fill: a well, a track, a disabled field.
    pub const SURFACE_SUNKEN: u32 = 0xFFE9E8E3;
    pub const BORDER: u32 = 0xFFE1E0DB;
    pub const TEXT: u32 = 0xFF111214;
    pub const TEXT_MUTED: u32 = 0xFF525458;

    // Canonical dark surfaces
    pub const DARK_BACKGROUND: u32 = 0xFF111214;
    pub const DARK_SURFACE: u32 = 0xFF25282A;
    /// One elevation step above the dark surface.
    pub const DARK_SURFACE_RAISED: u32 = 0xFF2F3235;
    /// One step below the dark surface.
    pub const DARK_SURFACE_SUNKEN: u32 = 0xFF1A1C1E;
    pub const DARK_BORDER: u32 = 0xFF35383D;
    pub const DARK_TEXT: u32 = 0xFFF5F6F7;
    pub const DARK_TEXT_MUTED: u32 = 0xFF9AA0A6;

    // Restrained semantic accents
    pub const ACCENT: u32 = 0xFF2563EB;
    pub const SUCCESS: u32 = 0xFF2E8B57;
    pub const WARNING: u32 = 0xFFD97706;
    pub const DANGER: u32 = 0xFFDC2626;
    pub const EMERGENCY: u32 = 0xFFFF5555;

    // The same accents against the dark surfaces: a tint that reads on a
    // near-black page has to be lighter than the tint that reads on white.
    pub const DARK_ACCENT: u32 = 0xFF38BDF8;
    pub const DARK_SUCCESS: u32 = 0xFF34D399;
    pub const DARK_WARNING: u32 = 0xFFFBBF24;
    pub const DARK_DANGER: u32 = 0xFFF87171;

    // Vibrant stock application accents (Light surfaces)
    pub const VIBRANT_PHONE: u32 = 0xFF10B981;       // Emerald green
    pub const VIBRANT_MESSAGES: u32 = 0xFF0284C7;    // Electric blue
    pub const VIBRANT_FILES: u32 = 0xFFEA580C;       // Amber / orange
    pub const VIBRANT_MUSIC: u32 = 0xFFF43F5E;       // Coral red
    pub const VIBRANT_SETTINGS: u32 = 0xFF64748B;    // Steel blue
    pub const VIBRANT_BROWSER: u32 = 0xFF6366F1;     // Indigo
    pub const VIBRANT_VAULT: u32 = 0xFFEAB308;       // Gold
    pub const VIBRANT_CAMERA: u32 = 0xFFF59E0B;      // Gold / warm amber
    pub const VIBRANT_CALCULATOR: u32 = 0xFF0D9488;  // Teal
    pub const VIBRANT_CALENDAR: u32 = 0xFF0284C7;    // Cerulean / sky blue
    pub const VIBRANT_NOTES: u32 = 0xFFD97706;       // Warm amber
    pub const VIBRANT_GALLERY: u32 = 0xFF8B5CF6;     // Iris / purple
    pub const VIBRANT_AGENT: u32 = 0xFF7C3AED;       // Electric violet

    // Vibrant stock application accents (Dark surfaces)
    pub const DARK_VIBRANT_PHONE: u32 = 0xFF34D399;       // Soft emerald
    pub const DARK_VIBRANT_MESSAGES: u32 = 0xFF38BDF8;    // Electric sky blue
    pub const DARK_VIBRANT_FILES: u32 = 0xFFFB923C;       // Soft warm orange
    pub const DARK_VIBRANT_MUSIC: u32 = 0xFFFB7185;       // Soft coral red
    pub const DARK_VIBRANT_SETTINGS: u32 = 0xFF94A3B8;    // Soft steel blue
    pub const DARK_VIBRANT_BROWSER: u32 = 0xFF818CF8;     // Soft indigo
    pub const DARK_VIBRANT_VAULT: u32 = 0xFFFACC15;       // Soft gold
    pub const DARK_VIBRANT_CAMERA: u32 = 0xFFFBBF24;      // Soft gold / amber
    pub const DARK_VIBRANT_CALCULATOR: u32 = 0xFF2DD4BF;  // Soft teal
    pub const DARK_VIBRANT_CALENDAR: u32 = 0xFF38BDF8;    // Soft cerulean
    pub const DARK_VIBRANT_NOTES: u32 = 0xFFFBBF24;       // Soft warm amber
    pub const DARK_VIBRANT_GALLERY: u32 = 0xFFA78BFA;     // Soft iris
    pub const DARK_VIBRANT_AGENT: u32 = 0xFFB794F6;       // Soft violet

    // Semantic color token aliases
    pub const EMERALD_GREEN: u32 = VIBRANT_PHONE;
    pub const ELECTRIC_BLUE: u32 = VIBRANT_MESSAGES;
    pub const AMBER_ORANGE: u32 = VIBRANT_FILES;
    pub const CORAL_RED: u32 = VIBRANT_MUSIC;
    pub const STEEL_BLUE: u32 = VIBRANT_SETTINGS;
    pub const INDIGO: u32 = VIBRANT_BROWSER;
    pub const GOLD: u32 = VIBRANT_VAULT;

    pub const DARK_EMERALD_GREEN: u32 = DARK_VIBRANT_PHONE;
    pub const DARK_ELECTRIC_BLUE: u32 = DARK_VIBRANT_MESSAGES;
    pub const DARK_AMBER_ORANGE: u32 = DARK_VIBRANT_FILES;
    pub const DARK_CORAL_RED: u32 = DARK_VIBRANT_MUSIC;
    pub const DARK_STEEL_BLUE: u32 = DARK_VIBRANT_SETTINGS;
    pub const DARK_INDIGO: u32 = DARK_VIBRANT_BROWSER;
    pub const DARK_GOLD: u32 = DARK_VIBRANT_VAULT;

    pub use super::theme::AccentPalette;

    /// Color gamut profiles supported by the display compositor.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum ColorSpace {
        SrgbStandard,
        DisplayP3WideGamut,
    }

    /// Wide-gamut Display P3 semantic tokens for flagship OLED/Mini-LED displays.
    /// P3 expands the color volume by ~25% compared to standard sRGB, rendering
    /// deeper greens, vibrant reds, and rich cinematic tones.
    pub mod p3 {
        pub const ACCENT: u32 = 0xFF1D5DE8;
        pub const SUCCESS: u32 = 0xFF27995A;
        pub const WARNING: u32 = 0xFFE07300;
        pub const DANGER: u32 = 0xFFE81B1B;
        pub const VIBRANT_CYAN: u32 = 0xFF00B4D8;
        pub const VIBRANT_PURPLE: u32 = 0xFF7B2CBF;
        pub const EMERALD_GREEN: u32 = 0xFF00C853;
        pub const ELECTRIC_BLUE: u32 = 0xFF0091EA;
        pub const AMBER_ORANGE: u32 = 0xFFFF6D00;
        pub const CORAL_RED: u32 = 0xFFFF1744;
        pub const STEEL_BLUE: u32 = 0xFF546E7A;
        pub const INDIGO: u32 = 0xFF651FFF;
        pub const GOLD: u32 = 0xFFFFD600;
    }
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
    // WHY these are the only names: this module also carried `SM`/`MD`/`LG`,
    // which were the same three numbers as `CONTROL`/`CARD`/`SHEET` under
    // different names and a different type. Two spellings of one value is how a
    // scale stops being a scale -- a reader cannot tell whether `radius::SM`
    // means something distinct from `radius::CONTROL`, and cannot tell which one
    // a screen meant. Nothing referenced the short names; they are gone rather
    // than deprecated, because a deprecation here would leave the ambiguity in
    // place for every reader who greps for either spelling.
    pub const SMALL: usize = 8;
    /// Chips and badges: the small round things that sit inside a row.
    ///
    /// WHY this step exists: the launcher drew its chips and badges at 10 and
    /// kept the number to itself, so a third-party app had no step for the
    /// thing it could see on screen and had to invent one.
    pub const CHIP: usize = 10;
    /// Buttons, interactive tiles and text fields.
    pub const CONTROL: usize = 12;
    /// A large control: a 40-48px pill that is rounded but not a capsule.
    ///
    /// WHY this step exists: the terminal's badge, the camera's top-row pills
    /// and the browser's chrome all rounded their corners at 16 and kept the
    /// number to themselves, so a third-party app had no step for the shape it
    /// could see on screen and had to invent one. It is between
    /// [`CONTROL`] and [`CARD`] because that is where it sits: too round for a
    /// text field, too square for a card.
    pub const SOFT_PILL: usize = 16;
    /// Cards and list rows.
    pub const CARD: usize = 18;
    /// Sheets and full-width panels.
    pub const SHEET: usize = 24;
    /// Large panels and dialogs.
    pub const MAJOR: usize = 32;
    /// The rounded corner of the device's own display surface.
    pub const SCREEN: usize = 44;
    /// A fully rounded pill; clamped to half the shape's height.
    pub const PILL: usize = 999;
}

pub mod touch {
    pub const MIN_TARGET: usize = 44;
    pub const PREFERRED_TARGET: usize = 48;
    pub const PRIMARY_ACTION: usize = 56;
}

pub mod motion;

pub mod viewport {
    /// Canonical portrait smartphone logical viewport dimensions (9:18 standard).
    ///
    /// WHY this is the authoring space and not the screen: every app in this
    /// tree lays out in these coordinates, and the reference design system's
    /// spacing, type rungs and touch targets are all expressed against them. An
    /// app that renders into a surface of this size needs no coordinate changes
    /// at all, which is what makes it the safe default.
    pub const LOGICAL_WIDTH: usize = 720;
    pub const LOGICAL_HEIGHT: usize = 1440;

    /// Hardware / QEMU scanout dimensions.
    ///
    /// WHY 1080x2160 and not the reference's 1080x2400: the reference is authored
    /// at 9:20 while this guest's authoring space is 1:2, and a scale between two
    /// different aspect ratios is not uniform -- it would stretch one axis more
    /// than the other, so every box on every screen would be subtly wrong and no
    /// single ratio could express it. 1080x2160 is the same 1:2 shape, so the
    /// ratio is exact on both axes, and it is FHD-class width with 2.33M pixels.
    ///
    /// WHY these are separate constants rather than aliases: raising the scanout
    /// buys real detail only if apps paint at that resolution, and an app that
    /// keeps painting at `LOGICAL_*` while the scanout grows gets an upscale --
    /// the same soft edges, at a larger surface cost. Keeping the two numbers
    /// distinct means a surface size is a decision that appears in the code
    /// instead of a silent consequence.
    ///
    /// The cost, measured against this guest's own limits: an ARGB8888 surface of
    /// this size is 8.9MB and the compositor holds two, so 17.8MB of the 64MB heap
    /// before a single app surface. That is the number that decides whether this
    /// stays, and it is recorded here rather than discovered later.
    pub const SCANOUT_WIDTH: usize = 1080;
    pub const SCANOUT_HEIGHT: usize = 2160;
    pub const FHD_WIDTH: usize = 1080;
    pub const FHD_HEIGHT: usize = 2160;

    /// The authoring-to-scanout ratio, as a numerator over a denominator.
    ///
    /// WHY a ratio and not a single integer: 1080/720 is 1.5, and a single
    /// integer cannot express it. Making it a rational is what lets the ratio
    /// stay exact rather than being rounded to 1 -- which would leave the scanout
    /// at FHD while every app still painted at HD, an upscale with an FHD
    /// surface cost.
    pub const SCALE_NUM: usize = SCANOUT_WIDTH;
    pub const SCALE_DEN: usize = LOGICAL_WIDTH;

    /// Maps a logical coordinate to the pixel that owns it in the physical
    /// surface, rounded to nearest.
    ///
    /// WHY this exists: it is the one conversion an app needs in order to render
    /// at a higher scanout without re-authoring its coordinates. A painter that
    /// receives a physical (px, py) divides back through `surface_to_logical` to
    /// get the logical point its layout already knows, and then draws with a size
    /// scaled by the ratio. Everything the app already computes -- boxes, hit
    /// tests, advances -- stays in the authoring space.
    ///
    /// WHY round-to-nearest and not a floor: a floor maps two logical points onto
    /// one physical pixel whenever the ratio crosses an integer boundary, and a
    /// collided pixel is a dropped stroke. An unused trailing row is invisible.
    #[inline]
    pub fn logical_to_surface(lx: usize, ly: usize) -> (usize, usize) {
        (
            (lx * SCALE_NUM + SCALE_DEN / 2) / SCALE_DEN,
            (ly * SCALE_NUM + SCALE_DEN / 2) / SCALE_DEN,
        )
    }

    /// The inverse of [`logical_to_surface`]: the logical point a physical pixel
    /// belongs to.
    #[inline]
    pub fn surface_to_logical(px: usize, py: usize) -> (usize, usize) {
        (px * SCALE_DEN / SCALE_NUM, py * SCALE_DEN / SCALE_NUM)
    }

    /// The size `w` logical pixels occupies on the physical surface.
    ///
    /// WHY a painter needs this and not just the two coordinate maps: a box is
    /// not two points. A rectangle at logical (x, y) of logical size (w, h)
    /// occupies `(logical_to_surface(x + w, y + h).0 - logical_to_surface(x, y).0)`
    /// on the physical surface, and taking the difference of the mapped corners
    /// is the only way to get that right when the ratio is not an integer.
    #[inline]
    pub fn scale_extent(w: usize, h: usize) -> (usize, usize) {
        (
            (w * SCALE_NUM + SCALE_DEN - 1) / SCALE_DEN,
            (h * SCALE_NUM + SCALE_DEN - 1) / SCALE_DEN,
        )
    }

    /// True when the scanout is larger than the authoring space, which is what
    /// raising it to FHD does.
    ///
    /// WHY a caller needs to know: when this is true a painter must scale the
    /// sizes it draws with, and forgetting that produces a correctly laid out
    /// surface with undersized text. A caller that checks it can refuse the
    /// mismatch rather than draw the wrong thing.
    #[inline]
    pub fn scaled() -> bool {
        SCALE_NUM != SCALE_DEN
    }

    /// Scales a stroke width or border thickness to the physical scanout.
    /// Preserves at least 1px for any non-zero logical stroke.
    #[inline]
    pub fn scale_stroke(stroke: usize) -> usize {
        if stroke == 0 {
            0
        } else {
            ((stroke * SCALE_NUM + SCALE_DEN / 2) / SCALE_DEN).max(1)
        }
    }

    /// Scales a corner radius to the physical scanout, maintaining proportional curvature.
    #[inline]
    pub fn scale_radius(r: usize) -> usize {
        (r * SCALE_NUM + SCALE_DEN / 2) / SCALE_DEN
    }

    /// Scales an authoring rectangle (x, y, w, h) to its exact physical scanout bounds.
    /// Uses corner-snapping so adjacent rectangles share identical boundaries with zero gap.
    #[inline]
    pub fn scale_rect(lx: usize, ly: usize, lw: usize, lh: usize) -> (usize, usize, usize, usize) {
        let (sx0, sy0) = logical_to_surface(lx, ly);
        let (sx1, sy1) = logical_to_surface(lx + lw, ly + lh);
        (sx0, sy0, sx1 - sx0, sy1 - sy0)
    }

    /// Clamps and maps physical touch coordinates to the logical authoring canvas.
    #[inline]
    pub fn touch_to_logical(px: i32, py: i32) -> (usize, usize) {
        let cx = px.max(0).min(SCANOUT_WIDTH as i32 - 1) as usize;
        let cy = py.max(0).min(SCANOUT_HEIGHT as i32 - 1) as usize;
        surface_to_logical(cx, cy)
    }

    /// Clips a logical rectangle against logical viewport bounds [0, 0, LOGICAL_WIDTH, LOGICAL_HEIGHT]
    /// and returns the mapped physical rectangle (sx, sy, sw, sh), or None if entirely clipped out.
    #[inline]
    pub fn clip_and_scale_rect(
        lx: i32,
        ly: i32,
        lw: u32,
        lh: u32,
    ) -> Option<(usize, usize, usize, usize)> {
        let rx0 = lx.max(0).min(LOGICAL_WIDTH as i32);
        let ry0 = ly.max(0).min(LOGICAL_HEIGHT as i32);
        let rx1 = (lx + lw as i32).max(0).min(LOGICAL_WIDTH as i32);
        let ry1 = (ly + lh as i32).max(0).min(LOGICAL_HEIGHT as i32);
        if rx1 <= rx0 || ry1 <= ry0 {
            return None;
        }
        let (sx0, sy0) = logical_to_surface(rx0 as usize, ry0 as usize);
        let (sx1, sy1) = logical_to_surface(rx1 as usize, ry1 as usize);
        Some((sx0, sy0, sx1 - sx0, sy1 - sy0))
    }

    /// System safe areas and chrome metrics.
    pub const STATUS_BAR_HEIGHT: usize = 48;
    pub const NAV_BAR_HEIGHT: usize = 64;
    pub const GESTURE_BAR_HEIGHT: usize = 48;
    pub const CONTENT_PADDING: usize = 24;

    /// Display orientation mode.
    #[derive(Clone, Copy, PartialEq, Eq, Debug)]
    pub enum DisplayOrientation {
        Portrait,
        LandscapeLeft,
        LandscapeRight,
    }

    /// Logical safe area insets for mobile applications.
    #[derive(Clone, Copy, PartialEq, Eq, Debug)]
    pub struct Insets {
        pub top: usize,
        pub bottom: usize,
        pub left: usize,
        pub right: usize,
    }

    impl Insets {
        pub const STANDARD: Insets = Insets {
            top: STATUS_BAR_HEIGHT,
            bottom: GESTURE_BAR_HEIGHT,
            left: 0,
            right: 0,
        };

        pub const FULLSCREEN: Insets = Insets {
            top: 0,
            bottom: 0,
            left: 0,
            right: 0,
        };
    }

    /// Unified display coordinate transformer between logical mobile canvas and scanout.
    #[derive(Clone, Copy, PartialEq, Eq, Debug)]
    pub struct DisplayTransform {
        pub orientation: DisplayOrientation,
        pub scanout_w: usize,
        pub scanout_h: usize,
    }

    impl DisplayTransform {
        pub const fn new(orientation: DisplayOrientation) -> Self {
            Self {
                orientation,
                scanout_w: SCANOUT_WIDTH,
                scanout_h: SCANOUT_HEIGHT,
            }
        }

        /// Map logical portrait (lx, ly) in 720x1440 to scanout destination (sx, sy).
        pub fn logical_to_scanout(&self, lx: usize, ly: usize) -> Option<(usize, usize)> {
            if lx >= LOGICAL_WIDTH || ly >= LOGICAL_HEIGHT || self.scanout_w == 0 || self.scanout_h == 0 {
                return None;
            }

            match self.orientation {
                DisplayOrientation::Portrait => {
                    // WHY no letterbox here: the scanout and the authoring space
                    // are the same aspect ratio, so an aspect fit places the frame
                    // at the full scanout with no bars -- centring it adds nothing
                    // and centring a frame that already fills the screen is a bug
                    // that renders as a black border. The previous code computed
                    // `dst_w`/`dst_h` by an aspect fit regardless, so when the two
                    // ratios agreed it still applied the centred offsets, which is
                    // what the failing test caught: logical (0, 0) mapped to
                    // (0, 120) instead of (0, 0).
                    //
                    // The transform carries its own scanout size so a caller can
                    // describe a display that differs from the module's, and the
                    // aspect-fit branch below is what serves that.
                    if self.scanout_w == SCANOUT_WIDTH && self.scanout_h == SCANOUT_HEIGHT {
                        let (sx, sy) = logical_to_surface(lx, ly);
                        return Some((sx.min(self.scanout_w - 1), sy.min(self.scanout_h - 1)));
                    }
                    let (dst_w, dst_h) = if LOGICAL_WIDTH * self.scanout_h <= LOGICAL_HEIGHT * self.scanout_w {
                        (
                            (LOGICAL_WIDTH * self.scanout_h / LOGICAL_HEIGHT).max(1).min(self.scanout_w),
                            self.scanout_h,
                        )
                    } else {
                        (
                            self.scanout_w,
                            (LOGICAL_HEIGHT * self.scanout_w / LOGICAL_WIDTH).max(1).min(self.scanout_h),
                        )
                    };
                    let off_x = (self.scanout_w - dst_w) / 2;
                    let off_y = (self.scanout_h - dst_h) / 2;
                    let sx = off_x + lx * dst_w / LOGICAL_WIDTH;
                    let sy = off_y + ly * dst_h / LOGICAL_HEIGHT;
                    Some((sx.min(self.scanout_w - 1), sy.min(self.scanout_h - 1)))
                }
                DisplayOrientation::LandscapeRight => {
                    let sx = ly * self.scanout_w / LOGICAL_HEIGHT;
                    let sy = self.scanout_h - 1 - lx * self.scanout_h / LOGICAL_WIDTH;
                    Some((sx.min(self.scanout_w - 1), sy.min(self.scanout_h - 1)))
                }
                DisplayOrientation::LandscapeLeft => {
                    let sx = self.scanout_w - 1 - ly * self.scanout_w / LOGICAL_HEIGHT;
                    let sy = lx * self.scanout_h / LOGICAL_WIDTH;
                    Some((sx.min(self.scanout_w - 1), sy.min(self.scanout_h - 1)))
                }
            }
        }
    }
}

pub mod primitives {
    #[derive(Clone, Copy, PartialEq, Eq, Debug)]
    pub struct Rect {
        pub x: usize,
        pub y: usize,
        pub w: usize,
        pub h: usize,
    }

    impl Rect {
        pub const fn new(x: usize, y: usize, w: usize, h: usize) -> Self {
            Self { x, y, w, h }
        }

        pub fn contains(&self, px: usize, py: usize) -> bool {
            px >= self.x && px < self.x + self.w && py >= self.y && py < self.y + self.h
        }

        pub fn inset(&self, pad: usize) -> Self {
            Self {
                x: self.x + pad,
                y: self.y + pad,
                w: self.w.saturating_sub(pad * 2),
                h: self.h.saturating_sub(pad * 2),
            }
        }
    }

    /// Anti-aliased rounded rectangle rasterizer with 1px border support.
    #[inline]
    pub fn draw_rounded_rect(
        x: usize,
        y: usize,
        rect: Rect,
        radius: usize,
        fill: u32,
        border: Option<u32>,
        border_w: usize,
    ) -> Option<u32> {
        if !rect.contains(x, y) {
            return None;
        }
        let r = radius.min(rect.w / 2).min(rect.h / 2);
        if r == 0 {
            if let Some(b_color) = border {
                if border_w > 0
                    && (x < rect.x + border_w
                        || x >= rect.x + rect.w - border_w
                        || y < rect.y + border_w
                        || y >= rect.y + rect.h - border_w)
                {
                    return Some(b_color);
                }
            }
            return Some(fill);
        }

        let in_left = x < rect.x + r;
        let in_right = x >= rect.x + rect.w - r;
        let in_top = y < rect.y + r;
        let in_bottom = y >= rect.y + rect.h - r;

        if (in_left || in_right) && (in_top || in_bottom) {
            let cx = if in_left { rect.x + r } else { rect.x + rect.w - r - 1 } as i32;
            let cy = if in_top { rect.y + r } else { rect.y + rect.h - r - 1 } as i32;
            let dx = (x as i32) - cx;
            let dy = (y as i32) - cy;
            let dist_sq = dx * dx + dy * dy;
            let r_i32 = r as i32;
            let r_sq = r_i32 * r_i32;

            if dist_sq > r_sq {
                return None;
            }

            if let Some(b_color) = border {
                if border_w > 0 {
                    let inner_r = r_i32.saturating_sub(border_w as i32);
                    if dist_sq >= inner_r * inner_r {
                        return Some(b_color);
                    }
                }
            }
            return Some(fill);
        }

        if let Some(b_color) = border {
            if border_w > 0
                && (x < rect.x + border_w
                    || x >= rect.x + rect.w - border_w
                    || y < rect.y + border_w
                    || y >= rect.y + rect.h - border_w)
            {
                return Some(b_color);
            }
        }
        Some(fill)
    }

    /// Renders a capsule / pill (fully rounded ends).
    #[inline]
    pub fn draw_pill(
        x: usize,
        y: usize,
        rect: Rect,
        fill: u32,
        border: Option<u32>,
    ) -> Option<u32> {
        let r = rect.h / 2;
        draw_rounded_rect(x, y, rect, r, fill, border, 1)
    }

    /// Renders a circular button or badge.
    #[inline]
    pub fn draw_circle(
        x: usize,
        y: usize,
        cx: usize,
        cy: usize,
        radius: usize,
        fill: u32,
        border: Option<u32>,
    ) -> Option<u32> {
        let dx = (x as i32) - (cx as i32);
        let dy = (y as i32) - (cy as i32);
        let dist_sq = dx * dx + dy * dy;
        let r_i32 = radius as i32;
        let r_sq = r_i32 * r_i32;
        if dist_sq > r_sq {
            return None;
        }
        if let Some(b_color) = border {
            let inner_r = r_i32.saturating_sub(1);
            if dist_sq >= inner_r * inner_r {
                return Some(b_color);
            }
        }
        Some(fill)
    }

    /// Renders a crisp 1px divider.
    #[inline]
    pub fn draw_divider(
        x: usize,
        y: usize,
        x0: usize,
        y0: usize,
        len: usize,
        is_horizontal: bool,
        color: u32,
    ) -> Option<u32> {
        if is_horizontal {
            if y == y0 && x >= x0 && x < x0 + len {
                Some(color)
            } else {
                None
            }
        } else {
            if x == x0 && y >= y0 && y < y0 + len {
                Some(color)
            } else {
                None
            }
        }
    }
}

pub use crate::font;

pub mod back_button {
    use super::radius::*;

    pub const X: usize = 24;
    pub const Y: usize = 24;
    pub const SIZE: usize = 48;
    pub const RADIUS: usize = CONTROL;

    #[inline]
    pub fn chevron_ink(x: usize, y: usize, cx: usize, cy: usize, color: u32) -> Option<u32> {
        let dx = (x as i32) - (cx as i32);
        let dy = (y as i32) - (cy as i32);
        if dx >= -4 && dx <= 4 && dy >= -8 && dy <= 8 {
            let arm_top = dx == -dy - 4 || dx == -dy - 3;
            let arm_bot = dx == dy - 4 || dx == dy - 3;
            if arm_top || arm_bot {
                return Some(color);
            }
        }
        None
    }

    #[inline]
    pub fn pixel(x: usize, y: usize, text_color: u32, border_color: u32, bg_color: u32) -> Option<u32> {
        if x < X || x >= X + SIZE || y < Y || y >= Y + SIZE {
            return None;
        }
        let rad = RADIUS;
        let left = X + rad;
        let right = X + SIZE - 1 - rad;
        let top = Y + rad;
        let bottom = Y + SIZE - 1 - rad;
        let in_box = (x >= left && x <= right) || (y >= top && y <= bottom);
        let in_corners = {
            let cx = if x < left { left } else { right };
            let cy = if y < top { top } else { bottom };
            let dx = x as i32 - cx as i32;
            let dy = y as i32 - cy as i32;
            dx * dx + dy * dy <= (rad as i32) * (rad as i32)
        };
        if !in_box && !in_corners {
            return None;
        }

        let is_border = x == X || x == X + SIZE - 1 || y == Y || y == Y + SIZE - 1;
        if is_border {
            return Some(border_color);
        }

        if let Some(c) = chevron_ink(x, y, X + SIZE / 2, Y + SIZE / 2, text_color) {
            return Some(c);
        }
        Some(bg_color)
    }

    #[inline]
    pub fn contains(x: usize, y: usize) -> bool {
        x >= X && x < X + SIZE && y >= Y && y < Y + SIZE
    }
}

#[cfg(test)]
mod tests {
    use super::viewport::{
        logical_to_surface, surface_to_logical, DisplayOrientation, DisplayTransform,
        LOGICAL_HEIGHT, LOGICAL_WIDTH, SCANOUT_HEIGHT, SCANOUT_WIDTH,
    };

    #[test]
    fn portrait_transform_uses_the_actual_scanout_dimensions() {
        let mut transform = DisplayTransform::new(DisplayOrientation::Portrait);
        transform.scanout_w = 1280;
        transform.scanout_h = 720;

        assert_eq!(transform.logical_to_scanout(0, 0), Some((460, 0)));
        assert_eq!(transform.logical_to_scanout(719, 1439), Some((819, 719)));
    }

    #[test]
    fn native_portrait_scanout_has_no_letterbox() {
        let transform = DisplayTransform::new(DisplayOrientation::Portrait);
        // The corner that matters is the origin: a centred frame that does not
        // need centring shows up as logical (0, 0) not mapping to (0, 0), which
        // is a black border along two edges. The far corner is derived rather
        // than pinned, because it changes with the scanout and pinning it
        // re-derives the ratio by hand in a test.
        assert_eq!(transform.logical_to_scanout(0, 0), Some((0, 0)));
        let far = logical_to_surface(LOGICAL_WIDTH - 1, LOGICAL_HEIGHT - 1);
        assert_eq!(
            transform.logical_to_scanout(LOGICAL_WIDTH - 1, LOGICAL_HEIGHT - 1),
            Some((far.0.min(SCANOUT_WIDTH - 1), far.1.min(SCANOUT_HEIGHT - 1)))
        );
        // And the frame fills the scanout rather than sitting inside it.
        assert_eq!(SCANOUT_WIDTH * LOGICAL_HEIGHT, SCANOUT_HEIGHT * LOGICAL_WIDTH,
                   "authoring space and scanout must share an aspect ratio or this is an upscale");
    }

    #[test]
    fn logical_to_surface_round_trips_through_surface_to_logical() {
        // The reason the ratio is a rational rather than an integer: 1080/720 is
        // 1.5, and rounding it to 1 would leave an FHD scanout with HD content.
        // Every logical point must come back out of the inverse mapping, or a
        // hit test would answer for a pixel that is not where the user touched.
        for lx in 0..LOGICAL_WIDTH {
            for ly in (0..LOGICAL_HEIGHT).step_by(97) {
                let (px, py) = logical_to_surface(lx, ly);
                let (bx, by) = surface_to_logical(px, py);
                assert_eq!((bx, by), (lx, ly), "round trip failed for ({lx}, {ly})");
            }
        }
    }

    #[test]
    fn scale_extent_never_collides_two_logical_points_onto_one_pixel() {
        // A collided pixel is a dropped stroke. Round-to-nearest in
        // `logical_to_surface` is what prevents it, and this pins the property
        // rather than a table of cases.
        let mut prev = None;
        for lx in 0..LOGICAL_WIDTH {
            let (px, _) = logical_to_surface(lx, 0);
            if let Some((at, _)) = prev {
                assert!(px > at, "logical {lx} collided onto pixel {px}");
            }
            prev = Some((px, ()));
        }
    }

    #[test]
    fn landscape_rotation_maps_logical_corners_to_scanout_corners() {
        let mut right = DisplayTransform::new(DisplayOrientation::LandscapeRight);
        right.scanout_w = 1280;
        right.scanout_h = 720;
        assert_eq!(right.logical_to_scanout(0, 0), Some((0, 719)));
        assert_eq!(right.logical_to_scanout(719, 1439), Some((1279, 0)));

        let mut left = DisplayTransform::new(DisplayOrientation::LandscapeLeft);
        left.scanout_w = 1280;
        left.scanout_h = 720;
        assert_eq!(left.logical_to_scanout(0, 0), Some((1279, 0)));
        assert_eq!(left.logical_to_scanout(719, 1439), Some((0, 719)));
    }

    #[test]
    fn invalid_points_and_zero_scanout_are_rejected() {
        let mut transform = DisplayTransform::new(DisplayOrientation::Portrait);
        transform.scanout_w = 0;
        assert_eq!(transform.logical_to_scanout(0, 0), None);

        transform.scanout_w = 720;
        transform.scanout_h = 1440;
        assert_eq!(transform.logical_to_scanout(720, 0), None);
        assert_eq!(transform.logical_to_scanout(0, 1440), None);
    }

    #[test]
    fn coordinate_rounding_and_snapping_at_1_5x_scale() {
        assert_eq!(logical_to_surface(0, 0), (0, 0));
        assert_eq!(logical_to_surface(1, 1), (2, 2));
        assert_eq!(logical_to_surface(2, 2), (3, 3));
        assert_eq!(logical_to_surface(720, 1440), (1080, 2160));
    }

    #[test]
    fn adjacent_rectangles_and_dividers_abut_without_seams() {
        use super::viewport::{scale_rect, scale_stroke};
        // Two adjacent cards: Card A [x: 40, w: 200] and Card B [x: 240, w: 200]
        let (ax, _, aw, _) = scale_rect(40, 100, 200, 50);
        let (bx, _, _bw, _) = scale_rect(240, 100, 200, 50);
        assert_eq!(ax + aw, bx, "Card A right edge must align exactly with Card B left edge");

        // Thin 1px divider at logical y=100
        let (dx, _dy, dw, _dh) = scale_rect(0, 100, 720, 1);
        assert_eq!((dx, dw), (0, 1080));
        assert_eq!(scale_stroke(1), 2); // 1px logical -> 2px physical (rounded 1.5)
        assert_eq!(scale_stroke(0), 0);
    }

    #[test]
    fn stroke_widths_and_corner_radii_scale_proportionally() {
        use super::viewport::{scale_radius, scale_stroke};
        assert_eq!(scale_radius(8), 12);  // radius::SMALL (8) * 1.5 = 12
        assert_eq!(scale_radius(12), 18); // radius::CONTROL (12) * 1.5 = 18
        assert_eq!(scale_radius(18), 27); // radius::CARD (18) * 1.5 = 27
        assert_eq!(scale_radius(24), 36); // radius::SHEET (24) * 1.5 = 36
        assert_eq!(scale_stroke(2), 3);   // 2px logical -> 3px physical
    }

    #[test]
    fn touch_coordinates_mapping_and_clamping() {
        use super::viewport::touch_to_logical;
        // Origin
        assert_eq!(touch_to_logical(0, 0), (0, 0));
        // Center
        assert_eq!(touch_to_logical(540, 1080), (360, 720));
        // Far edge
        assert_eq!(touch_to_logical(1079, 2159), (719, 1439));
        // Negative coords clamped
        assert_eq!(touch_to_logical(-50, -20), (0, 0));
        // Overshot coords clamped
        assert_eq!(touch_to_logical(2000, 3000), (719, 1439));
    }

    #[test]
    fn clipping_at_logical_and_physical_boundaries() {
        use super::viewport::{clip_and_scale_rect, scale_rect};
        // Completely inside
        assert_eq!(clip_and_scale_rect(10, 20, 100, 50), Some(scale_rect(10, 20, 100, 50)));
        // Straddling left edge (x=-20, w=50 -> visible [0, 30))
        assert_eq!(clip_and_scale_rect(-20, 20, 50, 50), Some(scale_rect(0, 20, 30, 50)));
        // Entirely off left edge
        assert_eq!(clip_and_scale_rect(-100, 20, 50, 50), None);
        // Entirely off bottom edge
        assert_eq!(clip_and_scale_rect(10, 1500, 50, 50), None);
    }

    #[test]
    fn font_glyph_advance_scaling_consistency() {
        let logical_advances = [8usize, 12, 10, 14, 6];
        let mut logical_total = 0;
        let mut physical_total_snapped = 0;
        for &adv in &logical_advances {
            logical_total += adv;
            let (sx, _) = logical_to_surface(logical_total, 0);
            physical_total_snapped = sx;
        }
        let (direct_sx, _) = logical_to_surface(logical_total, 0);
        assert_eq!(physical_total_snapped, direct_sx, "snapped glyph run must match total logical run");
    }

    #[test]
    fn back_button_geometry_and_ink() {
        use super::back_button;
        assert!(back_button::contains(24, 24));
        assert!(back_button::contains(71, 71));
        assert!(!back_button::contains(23, 24));
        assert!(!back_button::contains(72, 72));
        assert!(back_button::pixel(48, 48, 0xFF000000, 0xFF888888, 0xFFFFFFFF).is_some());
    }

    #[test]
    fn display_metrics_dp_px_conversions_and_hairline() {
        use super::{DisplayMetrics, Dp, Px};
        let metrics = DisplayMetrics::canonical_phone();
        assert_eq!(metrics.hairline_px(), Px(1));
        assert_eq!(metrics.dp_to_px(Dp(0)), Px(0));
        assert_eq!(metrics.dp_to_px(Dp(10)), Px(15));
        assert_eq!(metrics.dp_to_px(Dp(720)), Px(1080));
        assert_eq!(metrics.dp_to_px(Dp(1440)), Px(2160));
        assert_eq!(metrics.px_to_dp(Px(0)), Dp(0));
        assert_eq!(metrics.px_to_dp(Px(15)), Dp(10));
        assert_eq!(metrics.px_to_dp(Px(1080)), Dp(720));
        assert_eq!(metrics.px_to_dp(Px(2160)), Dp(1440));
    }

    #[test]
    fn display_metrics_refresh_rates_and_orientation() {
        use super::{DisplayMetrics, Orientation, Px};
        let portrait = DisplayMetrics::canonical_phone();
        assert_eq!(portrait.frame_budget_ns(), 16_666_666);
        assert_eq!(DisplayMetrics::frame_budget_ns_for_hz(60), 16_666_666);
        assert_eq!(DisplayMetrics::frame_budget_ns_for_hz(120), 8_333_333);
        assert_eq!(DisplayMetrics::frame_budget_ns_for_hz(144), 6_944_444);

        let landscape = portrait.with_orientation(Orientation::LandscapeRight);
        assert_eq!(landscape.physical_width_px, 2160);
        assert_eq!(landscape.physical_height_px, 1080);
        assert_eq!(landscape.logical_width_dp, 1440);
        assert_eq!(landscape.logical_height_dp, 720);
        assert_eq!(landscape.safe_insets_px.right, Px(72)); // rotated top inset
    }
}
