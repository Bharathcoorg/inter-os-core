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
    /// The values are the top of each range the design document gives: a phone
    /// screen is read at arm's length, and the low end of the body and section
    /// bands left the platform's labels visibly smaller than the reference.
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

    /// The same ladder at the text-size level the platform has published.
    ///
    /// WHY these are functions and the twelve constants above are constants:
    /// a constant is the ladder, and the ladder is what every layout in this
    /// tree is authored against -- the rungs are read in const contexts by the
    /// `TextStyle`s below, by `LADDER_BANDS`, and by the layout gates in the
    /// applications, and a const evaluation cannot read a setting. The
    /// person's text-size level is therefore applied by the runtime face of
    /// the same arithmetic, and an application reaches it here or through
    /// [`crate::font`] by name. An app that takes its ink from
    /// [`crate::font::cap_for_scale_user`] must take its boxes from
    /// [`crate::font::line_height_user`]: the box and the ink move together or
    /// the scaled ink is clipped by the box, which is the defect class these
    /// two are the pair for.
    ///
    /// At the default level -- [`crate::font::TEXT_SCALE_NORMAL`] -- every one
    /// of these answers the constant above it, so a surface that adopts them
    /// draws the same pixels until someone changes the setting.
    pub use crate::font::{
        ascent_for_scale_user, cap_for_scale_user, em_for_scale_user, line_height_user,
        measure_bold_user, measure_user, set_user_text_scale, user_text_scale,
    };

    /// Badge glyphs inside an icon, e.g. a call badge.
    ///
    /// WHY it exists: the shell draws a badge in the corner of an icon, and the
    /// badge is a caption at the smallest step the face can express. It was
    /// carried by the launcher alone, which meant the step existed only there.
    /// WHY the numbers are the design document's: a style is what an app
    /// copies when it asks for "a caption", so a style that reserved the
    /// bitmap cell's multiples reserved two and a half times the space a
    /// reader expects and pushed the neighbouring content out of the way.
    /// WHY the line box is the face's own and not a number of its own: it was
    /// 16, carried over from before the ladder moved to the typeface metrics,
    /// while a run at this rung reserves 22. A style is what an app copies to
    /// lay a badge out, so the stale box let the ink run into whatever sat
    /// under the icon -- and the badge sits in the corner of the icon, which
    /// is the one place with no room to spare. `CAPTION` is the same rung, and
    /// two names for one size must not reserve two different boxes for it.
    pub const BADGE: TextStyle = TextStyle {
        scale: BADGE_SCALE,
        line_height_px: font::line_height(BADGE_SCALE),
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

    /// WHY this is checked where the styles are written: a `TextStyle` is what
    /// an app copies to lay a label out, and the box it reserves is only
    /// honest if it is the box the face actually takes at that rung. `BADGE`
    /// carried 16 for a rung that reserves 22, and the ink ran into whatever
    /// sat under the icon it was badgeing. A hand-written number now fails to
    /// compile instead of failing on a screen.
    const _: () = assert!(
        BADGE.line_height_px == font::line_height(BADGE.scale)
            && CAPTION.line_height_px == font::line_height(CAPTION.scale)
            && BODY.line_height_px == font::line_height(BODY.scale)
            && SECTION.line_height_px == font::line_height(SECTION.scale)
            && TITLE.line_height_px == font::line_height(TITLE.scale)
            && HEADING.line_height_px == font::line_height(HEADING.scale)
            && DISPLAY.line_height_px == font::line_height(DISPLAY.scale)
            && BADGE.baseline_offset_px == font::ascent_for_scale(BADGE.scale)
            && CAPTION.baseline_offset_px == font::ascent_for_scale(CAPTION.scale)
            && BODY.baseline_offset_px == font::ascent_for_scale(BODY.scale)
            && SECTION.baseline_offset_px == font::ascent_for_scale(SECTION.scale)
            && TITLE.baseline_offset_px == font::ascent_for_scale(TITLE.scale)
            && HEADING.baseline_offset_px == font::ascent_for_scale(HEADING.scale)
            && DISPLAY.baseline_offset_px == font::ascent_for_scale(DISPLAY.scale)
    );
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
    /// Canonical phone profile: 720x1440 physical, 720x1440 dp logical,
    /// 320 DPI, scale factor 1.0.
    ///
    /// WHY the physical and logical sizes are equal: the guest paints its
    /// scanout in the authoring space, so one dp is one physical pixel and
    /// composition is a copy rather than a 1.5x resample. The previous profile
    /// stated 1080x2160 at 480 DPI, which described a panel the guest never
    /// painted at. 320 DPI at 1.0x is the density the platform's authoring-space
    /// descriptor names (`platform/x86_64/qemu-phone.json`, 720x1440 @ 320).
    pub const fn canonical_phone() -> Self {
        Self {
            physical_width_px: 720,
            physical_height_px: 1440,
            logical_width_dp: 720,
            logical_height_dp: 1440,
            density_dpi: 320,
            scale_factor_permille: 1000,
            orientation: Orientation::Portrait,
            // The status bar is 48 logical px and physical == logical, so the
            // inset is 48 px rather than the 48 x 1.5 = 72 the retired 1080x2160
            // scanout used.
            safe_insets_px: SafeAreaInsetsPx {
                top: Px(48),
                bottom: Px(48),
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
    /// WHY this step exists: the camera's top-row pills
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

    /// The guest's scanout, which is the authoring space itself.
    ///
    /// WHY this used to be 1080x2160 and is not any more: the guest paints at
    /// 720x1440, so a larger scanout added no detail -- the compositor had to
    /// resample every composed frame by 1.5x to fill it, and a 1.5x resample of
    /// a 1px stroke lands between physical pixels. The visible result was text
    /// and hairlines softened on a panel that had only been asked to show the
    /// same content at a larger pitch. Painting the scanout at the authoring
    /// size makes composition a straight copy: no resample, no half-covered
    /// edge columns, one physical pixel per logical pixel.
    ///
    /// WHY the two spaces are still named separately: a future higher-DPI
    /// scanout buys real detail only if apps paint at that resolution, and an
    /// app that keeps painting at `LOGICAL_*` while the scanout grows gets an
    /// upscale -- the same soft edges, at a larger surface cost. Keeping the
    /// relation explicit (`SCALE_NUM`/`SCALE_DEN`) is what lets a surface size
    /// be a decision in the code rather than a silent consequence. Today that
    /// ratio is 1, and every mapping in this module is the identity.
    ///
    /// The cost of the current size: an ARGB8888 surface at 720x1440 is 3.96MB,
    /// and the compositor holds two, so about 7.9MB of the 64MB heap before a
    /// single app surface. It is recorded here rather than discovered later.
    pub const SCANOUT_WIDTH: usize = LOGICAL_WIDTH;
    pub const SCANOUT_HEIGHT: usize = LOGICAL_HEIGHT;

    /// An FHD-class reference frame of the same 1:2 shape (1080x2160).
    ///
    /// WHY this is not the scanout: it names the size a full-HD panel of this
    /// aspect ratio would have, for documents and comparisons. Nothing in the
    /// guest maps through it and no surface is this size. Treating it as a
    /// scanout while painting at `LOGICAL_*` would reintroduce exactly the 1.5x
    /// resample this module stopped doing.
    pub const FHD_WIDTH: usize = 1080;
    pub const FHD_HEIGHT: usize = 2160;

    /// The scanout-to-authoring ratio, as a numerator over a denominator.
    ///
    /// WHY a ratio and not a single integer: the two sizes are independent
    /// decisions, and one integer cannot express a fractional ratio if the
    /// scanout ever grows again (720 to 1080 is 3/2). This ratio is exact only
    /// because the scanout and the authoring space share an aspect ratio, which
    /// `native_portrait_scanout_has_no_letterbox` pins. Today both terms are
    /// 720, so the ratio is 1 and every map below is the identity.
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
    /// WHY at a ratio of one this returns its argument: the scanout is the
    /// authoring space, so that is the correct answer, not an approximation. The
    /// multiply and divide stay so a scanout that grows again keeps the same
    /// corner-snapping rule instead of growing a second code path with different
    /// rounding.
    ///
    /// WHY round-to-nearest and not a floor: a floor maps two logical points onto
    /// one physical pixel whenever the ratio crosses an integer boundary, and a
    /// collided pixel is a dropped stroke. An unused trailing row is invisible.
    /// At 1:1 the rounding term is smaller than one denominator, so it cannot
    /// push a coordinate onto its neighbour.
    #[inline]
    pub fn logical_to_surface(lx: usize, ly: usize) -> (usize, usize) {
        (
            (lx * SCALE_NUM + SCALE_DEN / 2) / SCALE_DEN,
            (ly * SCALE_NUM + SCALE_DEN / 2) / SCALE_DEN,
        )
    }

    /// The inverse of [`logical_to_surface`]: the logical point a physical pixel
    /// belongs to.
    ///
    /// WHY the division is safe: `SCALE_NUM` is the scanout width, which is
    /// non-zero by construction, so a caller cannot divide by zero here even on
    /// a display the module knows nothing about. At the current ratio this is
    /// the identity, which is what makes an untouched pixel a logical pixel.
    #[inline]
    pub fn surface_to_logical(px: usize, py: usize) -> (usize, usize) {
        (px * SCALE_DEN / SCALE_NUM, py * SCALE_DEN / SCALE_NUM)
    }

    /// The size `w` logical pixels occupies on the physical surface, rounded up.
    ///
    /// WHY a painter needs this and not just the two coordinate maps: a box is
    /// not two points. A rectangle at logical (x, y) of logical size (w, h)
    /// occupies `(logical_to_surface(x + w, y + h).0 - logical_to_surface(x, y).0)`
    /// on the physical surface, and taking the difference of the mapped corners
    /// is the only way to get that right when the ratio is not an integer.
    ///
    /// WHY rounding up and not to nearest: a stroke narrower than a physical
    /// pixel must still cover one, or an extent of 1 can round to 0 and vanish.
    /// At 1:1 the result is `w` itself: `(w * n + n - 1) / n` is `w` for every
    /// non-negative `w`, because the remainder is smaller than `n`.
    #[inline]
    pub fn scale_extent(w: usize, h: usize) -> (usize, usize) {
        (
            (w * SCALE_NUM + SCALE_DEN - 1) / SCALE_DEN,
            (h * SCALE_NUM + SCALE_DEN - 1) / SCALE_DEN,
        )
    }

    /// True when the scanout is larger than the authoring space, which is when
    /// a painter must scale the sizes it draws with.
    ///
    /// WHY a caller needs to know: when this is true, forgetting the scale
    /// produces a correctly laid out surface with undersized text, and a caller
    /// that checks it can refuse the mismatch rather than draw the wrong thing.
    /// It is false today: the guest paints at 720x1440 and the scanout is
    /// 720x1440, so composition is a copy and an app must not scale.
    #[inline]
    pub fn scaled() -> bool {
        SCALE_NUM != SCALE_DEN
    }

    /// Scales a stroke width or border thickness to the physical scanout.
    /// Preserves at least 1px for any non-zero logical stroke.
    ///
    /// At the current 1:1 ratio a stroke is returned unchanged, including the
    /// 1px hairline the design system relies on for dividers and borders.
    #[inline]
    pub fn scale_stroke(stroke: usize) -> usize {
        if stroke == 0 {
            0
        } else {
            ((stroke * SCALE_NUM + SCALE_DEN / 2) / SCALE_DEN).max(1)
        }
    }

    /// Scales a corner radius to the physical scanout, maintaining proportional
    /// curvature. At 1:1 the radius is already in physical pixels.
    #[inline]
    pub fn scale_radius(r: usize) -> usize {
        (r * SCALE_NUM + SCALE_DEN / 2) / SCALE_DEN
    }

    /// Scales an authoring rectangle (x, y, w, h) to its exact physical scanout
    /// bounds. Uses corner-snapping so adjacent rectangles share identical
    /// boundaries with zero gap.
    ///
    /// At 1:1 this is the rectangle itself; the width still comes from the
    /// difference of the two mapped edges, not from `scale_extent`, so adjacency
    /// stays exact by construction.
    #[inline]
    pub fn scale_rect(lx: usize, ly: usize, lw: usize, lh: usize) -> (usize, usize, usize, usize) {
        let (sx0, sy0) = logical_to_surface(lx, ly);
        let (sx1, sy1) = logical_to_surface(lx + lw, ly + lh);
        (sx0, sy0, sx1 - sx0, sy1 - sy0)
    }

    /// Clamps and maps physical touch coordinates to the logical authoring
    /// canvas.
    ///
    /// The clamp is against the surface, so at 1:1 the last addressable logical
    /// coordinate is `LOGICAL_WIDTH - 1` / `LOGICAL_HEIGHT - 1` and a touch on
    /// the exclusive edge lands inside rather than past it.
    #[inline]
    pub fn touch_to_logical(px: i32, py: i32) -> (usize, usize) {
        let cx = px.max(0).min(SCANOUT_WIDTH as i32 - 1) as usize;
        let cy = py.max(0).min(SCANOUT_HEIGHT as i32 - 1) as usize;
        surface_to_logical(cx, cy)
    }

    /// Clips a logical rectangle against logical viewport bounds [0, 0, LOGICAL_WIDTH, LOGICAL_HEIGHT]
    /// and returns the mapped physical rectangle (sx, sy, sw, sh), or None if entirely clipped out.
    ///
    /// At 1:1 the returned rectangle is the clipped logical rectangle.
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

    /// Where an application's own chrome starts.
    ///
    /// WHY it is not the top of the panel: the band owns `0..STATUS_BAR_HEIGHT`
    /// and answers the pixel function before any header content is reached, so
    /// a row authored from y=0 loses the crown of its title and the top arc of
    /// its pills to a surface that was painted over it.
    pub const HEADER_TOP: usize = STATUS_BAR_HEIGHT + 8;

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
        ///
        /// At the module's own scanout, which is now 720x1440, this is the
        /// identity: the fast path below maps through `logical_to_surface` and
        /// clamps to the last addressable pixel, so the exclusive far corner
        /// lands on the edge rather than one past it.
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
    use super::theme::Theme;

    pub const X: usize = 24;
    /// WHY it starts below the band: the control is painted by the app's header
    /// row, and the band answers every pixel above it -- a button authored at
    /// y=24 had its top quarter shaved on every app that carries the band.
    pub const Y: usize = super::viewport::HEADER_TOP;
    pub const SIZE: usize = 48;
    pub const RADIUS: usize = CONTROL;

    /// The control's three colours, read out of a theme's roles: the chevron's
    /// ink, the ring's colour and the chip's fill.
    ///
    /// WHY this exists at all: every application drew this control with
    /// [`Theme::border`] as its ring and a container role as its fill, and
    /// [`Theme::border`] is the quietest line the platform draws -- `#E5E5E5`
    /// on the monochrome theme's `#FAFAFA` page, a 1.21:1 step. Its former
    /// fill, `surface_container_high`, is `#EFEEE9` on the same page: 1.11:1.
    /// Neither step survives the device's own pixel density, so what a person
    /// saw was a bare chevron with no button around it -- a control whose hit
    /// box was 48x48 and whose visible mark was nine pixels wide read as a
    /// screen with no way back. The ring is what says "this is a thing you can
    /// press", so the ring now takes the role that means exactly that.
    ///
    /// WHAT THE THREE ROLES ARE, and WHY each is the one it is:
    ///
    /// - the ring is [`Theme::outline`], the role the theme documents as "a
    ///   prominent outline: a focused field, a selected control". On the
    ///   monochrome theme that is `#0A0A0A`: 18.97:1 against the page and
    ///   17.15:1 against the chip's own fill. On the stock light theme it is
    ///   `#3A3D42` at 10.08:1, and no theme the platform ships puts its
    ///   `outline` below 9.5:1 against its own page. WCAG 1.4.11 asks 3:1 of
    ///   the visual boundary of a user interface component, so this clears the
    ///   floor by more than three times without leaving the neutral ladder.
    /// - the fill is [`Theme::surface_container_high`], the elevation step the
    ///   theme itself names "a raised card, a chip, a pressed control". It is
    ///   a one-step lift, not a statement: 1.11:1 against the monochrome page
    ///   and 1.15:1 against a white card, so the chip keeps a tone of its own
    ///   on either backdrop while [`Theme::outline`] does the visible work.
    /// - the chevron is [`Theme::on_surface`], the theme's own body ink, which
    ///   is 17.15:1 on that fill on the monochrome theme and never below
    ///   11.9:1 on any shipped theme.
    ///
    /// WHY a function and not a pair of constants: the three colours are one
    /// decision about one control, and a decision that lives in the theme
    /// follows the theme into its dark variant. An app that hard-codes the
    /// ring it thinks is dark enough is an app that draws an invisible control
    /// the moment the user turns the device dark.
    #[inline]
    pub const fn colors_for_theme(theme: &Theme) -> (u32, u32, u32) {
        (theme.on_surface, theme.outline, theme.surface_container_high)
    }

    /// [`pixel`] drawn with [`colors_for_theme`]'s colours.
    ///
    /// WHY an entry point rather than nine hand-tuned call sites: the three
    /// tokens are one decision, and a decision spelled out in nine places is
    /// nine places that can drift. This is the place; the applications call
    /// this and every Back control on the device is the same chip.
    ///
    /// WHAT IT DOES NOT CHANGE: the geometry, the radius, the hit box and
    /// [`pixel`]'s own signature. A caller that wants a ring of its own -- a
    /// control drawn over a photograph, over a dark chassis -- still has
    /// [`pixel`].
    #[inline]
    pub fn pixel_for_theme(x: usize, y: usize, theme: &Theme) -> Option<u32> {
        let (ink, ring, fill) = colors_for_theme(theme);
        pixel(x, y, ink, ring, fill)
    }

    /// Half the height the chevron inks, taken from the platform's own icon box.
    ///
    /// WHY the icon box and not a number: [`super::monochrome::ICON_BOX`] is the
    /// square every other glyph on the device is drawn to fill, and a control 48
    /// across whose mark was 17 tall is a control whose mark belonged to a smaller
    /// chip. The arms climb this many rows each way, so the mark inks the box and
    /// the row its vertex joins them on.
    const MARK_DROP: usize = super::monochrome::ICON_BOX / 2;
    /// How far each arm runs sideways from the vertex, as half its climb.
    ///
    /// WHY half: the arms are drawn at 45 degrees, so an arm that climbs
    /// [`MARK_DROP`] rows and runs the same number of columns would be a mark 27
    /// columns across on a plate 48 wide -- the shell's own nav chevron is the
    /// same shape with the same half-width-over-half-height ratio, and these two
    /// marks now read as one set rather than as a bold one and a hairline.
    const MARK_REACH: usize = MARK_DROP / 2;
    /// Half the width of an arm, measured across a row.
    ///
    /// WHY a quarter of the climb: the shell's nav mark uses the same ratio at
    /// its own size, and an arm whose rows are fewer than three apart is the
    /// hairline a person scrolling at arm's length cannot find.
    const MARK_STROKE: usize = MARK_DROP / 4;

    /// The chevron's ink footprint: width, height and the widest row, measured
    /// off [`pixel`] rather than off the constants, so a resize that quietly
    /// overlaps the ring is a failing test and not a chip with no edge.
    #[cfg(test)]
    pub fn mark_metrics() -> (usize, usize, usize) {
        let mut min_x = usize::MAX;
        let mut max_x = 0usize;
        let mut min_y = usize::MAX;
        let mut max_y = 0usize;
        let mut widest = 0usize;
        for y in Y..Y + SIZE {
            let mut row = 0usize;
            for x in X..X + SIZE {
                if pixel(x, y, INK_PROBE, RING_PROBE, FILL_PROBE) == Some(INK_PROBE) {
                    row += 1;
                    if x < min_x {
                        min_x = x;
                    }
                    if x > max_x {
                        max_x = x;
                    }
                    if y < min_y {
                        min_y = y;
                    }
                    if y > max_y {
                        max_y = y;
                    }
                }
            }
            if row > widest {
                widest = row;
            }
        }
        (max_x - min_x + 1, max_y - min_y + 1, widest)
    }

    /// The three probe colours [`mark_metrics`] tells apart. They are distinct on
    /// purpose: the mark is only found by asking which of the three a pixel came
    /// back as.
    #[cfg(test)]
    pub const INK_PROBE: u32 = 0xFF00_0001;
    #[cfg(test)]
    pub const RING_PROBE: u32 = 0xFF00_0002;
    #[cfg(test)]
    pub const FILL_PROBE: u32 = 0xFF00_0003;

    /// The chip's mark: two 45-degree arms that meet at a vertex left of the
    /// plate's centre and point the way back.
    ///
    /// WHY the arms are built from the same arithmetic as the shell's nav mark:
    /// the mark used to be two one-pixel offsets of a 45-degree line over an
    /// eight-row climb -- nine columns wide, seventeen rows tall, thirty-four
    /// pixels of ink on a plate forty-eight across. Measured off the device it
    /// was the quietest thing in the header, quieter than the 17-pixel title
    /// beside it, and a person who could not find it tapped the page and thought
    /// the app had frozen. The same construction at the size the icon ladder
    /// already uses inks seventeen columns, twenty-five rows and a hundred and
    /// twenty-five pixels.
    #[inline]
    pub fn chevron_ink(x: usize, y: usize, cx: usize, cy: usize, color: u32) -> Option<u32> {
        let dx = (x as i32) - (cx as i32);
        let dy = (y as i32) - (cy as i32);
        let climb = MARK_DROP as i32;
        if dy < -climb || dy > climb {
            return None;
        }
        let reach = MARK_REACH as i32;
        let arm = MARK_STROKE as i32;
        // Both arms answer the vertex row with the same set, so the point is one
        // place rather than a notch the ring would show through.
        let on_arm = if dy <= 0 {
            (dx + dy + reach).abs() < arm
        } else {
            (dx - dy + reach).abs() < arm
        };
        if on_arm {
            Some(color)
        } else {
            None
        }
    }

    /// Whether a pixel lies inside the chip's outline, shrunk `inset` pixels on
    /// every edge.
    ///
    /// WHY the ring is asked this way and not off the bounding box: the plate is
    /// a rounded rect, and the ring used to be `x == X || y == Y || ...` -- the
    /// four edges of a *square*. Only the flat middle of each edge is both box
    /// edge and plate edge, so the ink that survived was four straight brackets
    /// and the four corner arcs had no edge at all: measured off the device on
    /// the Settings header, the ring painted rows 12..=35 of the top edge and
    /// columns 12..=35 of the side edges and nothing on the arcs. A control
    /// whose outline stops where its own rounding begins reads as a drawing
    /// mistake rather than as a button. Asking "inside the shape, but not inside
    /// the shape one pixel in" puts the ring on the boundary the plate actually
    /// has, at every radius the theme's `Radii` may one day choose.
    #[inline]
    fn in_chip(x: i32, y: i32, inset: usize) -> bool {
        let rad = (RADIUS - inset) as i32;
        let left = (X + inset) as i32;
        let right = (X + SIZE - 1 - inset) as i32;
        let top = (Y + inset) as i32;
        let bottom = (Y + SIZE - 1 - inset) as i32;
        if x < left || x > right || y < top || y > bottom {
            return false;
        }
        let cx = if x < left + rad {
            left + rad
        } else if x > right - rad {
            right - rad
        } else {
            return true;
        };
        let cy = if y < top + rad {
            top + rad
        } else if y > bottom - rad {
            bottom - rad
        } else {
            return true;
        };
        (x - cx) * (x - cx) + (y - cy) * (y - cy) <= rad * rad
    }

    #[inline]
    pub fn pixel(x: usize, y: usize, text_color: u32, border_color: u32, bg_color: u32) -> Option<u32> {
        if x < X || x >= X + SIZE || y < Y || y >= Y + SIZE {
            return None;
        }
        let (xi, yi) = (x as i32, y as i32);
        if !in_chip(xi, yi, 0) {
            return None;
        }
        if !in_chip(xi, yi, 1) {
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

/// The bottom edge that answers "go home", and the pill that says so.
///
/// WHY one module owns it: every application wrote its own. Measured off the
/// tree, the mark that tells a person where to press was one shape and the box
/// that answered was another, in six different ways. The camera, the gallery,
/// Messages, the video player and the file manager tested the press against
/// `HOME_PILL` itself -- two hundred columns by SIX rows, so a thumb landing
/// one row above the ink did nothing at all -- while the vault, the calendar,
/// Music and Settings answered a hand-written 241 by 40 at row 1400, the
/// browser answered the thirty rows under its own toolbar, and the calculator,
/// the dialer and the notes had already arrived at a strip of their own. A
/// person presses where they can see, and the visible mark is six rows: the
/// screen is not hung, but it is indistinguishable from hung.
///
/// WHAT THE ANSWERING BOX IS NOT: the whole band. An application paints its own
/// content down here -- a list scrolled to its last row, a composer, a tab bar
/// -- and a gesture that claims every column of those rows takes the taps that
/// content was waiting for. The band is the shell's; the box an application
/// answers is the pill's neighbourhood, grown to the touch floor and reaching
/// the panel's last row, which is the house geometry the calculator, the dialer
/// and the notes had already chosen. A swipe up from the bottom edge is a
/// different gesture, and the kernel already routes it from whatever screen is
/// in front.
///
/// WHAT IT DOES NOT DECIDE: which applications leave on the gesture and which
/// close a panel first. That is the app's own stack, and the shell's back control
/// keeps answering it.
pub mod home_band {
    use super::primitives::{draw_pill, Rect};

    /// The shell's own navigation band, edge to edge: the rows a home swipe is
    /// drawn across, and the outer limit of what an application may answer.
    pub const BAND: Rect = Rect::new(
        0,
        super::viewport::LOGICAL_HEIGHT - super::viewport::NAV_BAR_HEIGHT,
        super::viewport::LOGICAL_WIDTH,
        super::viewport::NAV_BAR_HEIGHT,
    );

    /// The pill's own width and height: the mark the shell draws on the home
    /// screen, so an application paints the thing the person has already learned.
    pub const PILL_W: usize = 200;
    pub const PILL_H: usize = 6;

    /// The pill, centred on the panel's last rows.
    pub const PILL: Rect = Rect::new(
        (super::viewport::LOGICAL_WIDTH - PILL_W) / 2,
        super::viewport::LOGICAL_HEIGHT - 22,
        PILL_W,
        PILL_H,
    );

    /// The columns an answering box takes: the pill's own width, twenty rows of
    /// panel either side.
    const HIT_W: usize = 240;

    /// The rows an answering box takes: the platform's preferred target.
    const HIT_H: usize = super::touch::PREFERRED_TARGET;

    /// The box a tap on the home gesture is answered in: the pill's neighbourhood
    /// at the touch floor, centred, reaching the panel's last row.
    ///
    /// WHY this and not the pill: the pill is the ink and six rows tall, and six
    /// rows is under a seventh of the floor a thumb is guaranteed. Why not wider:
    /// the columns beside the pill belong to whatever the app painted there.
    pub const HIT: Rect = Rect::new(
        (super::viewport::LOGICAL_WIDTH - HIT_W) / 2,
        super::viewport::LOGICAL_HEIGHT - HIT_H,
        HIT_W,
        HIT_H,
    );

    /// The same box, beginning at a row the application chooses: an app whose own
    /// control holds the bottom of the screen -- an open keyboard, a toolbar --
    /// starts the box where that control stops, so it still reaches the last row
    /// and still covers the pill.
    #[inline]
    pub const fn hit_from(top: usize) -> Rect {
        Rect::new(
            (super::viewport::LOGICAL_WIDTH - HIT_W) / 2,
            top,
            HIT_W,
            super::viewport::LOGICAL_HEIGHT - top,
        )
    }

    /// Whether this contact belongs to the home gesture.
    #[inline]
    pub fn contains(x: usize, y: usize) -> bool {
        HIT.contains(x, y)
    }

    /// The pill's ink, or `None` on a pixel the band leaves empty.
    #[inline]
    pub fn pixel(x: usize, y: usize, ink: u32) -> Option<u32> {
        draw_pill(x, y, PILL, ink, None)
    }

    const _: () = assert!(
        BAND.h >= super::touch::MIN_TARGET,
        "a home band shorter than the touch floor is not a band a thumb can find",
    );
    const _: () = assert!(
        BAND.y + BAND.h == super::viewport::LOGICAL_HEIGHT,
        "the band must reach the panel's last row, or the gesture stops short of the edge",
    );
    const _: () = assert!(
        PILL.y >= BAND.y && PILL.y + PILL_H <= BAND.y + BAND.h,
        "the pill must be painted inside the rows that answer it",
    );
    const _: () = assert!(
        PILL.x + PILL_W == super::viewport::LOGICAL_WIDTH - PILL.x,
        "the pill must be centred on the panel it is drawn on",
    );
    const _: () = assert!(
        PILL.x + PILL_W < super::viewport::LOGICAL_WIDTH,
        "the pill must not run off the panel",
    );
    const _: () = assert!(
        HIT.x <= PILL.x && HIT.x + HIT_W >= PILL.x + PILL_W,
        "the answering box must cover the pill it is drawn for, column for column",
    );
    const _: () = assert!(
        HIT.y <= PILL.y && HIT.y + HIT_H >= PILL.y + PILL_H,
        "the answering box must cover the pill it is drawn for, row for row",
    );
    const _: () = assert!(
        HIT.h >= super::touch::MIN_TARGET,
        "a home box shorter than the touch floor is the six-row pill again",
    );
    const _: () = assert!(
        HIT.w >= super::touch::MIN_TARGET,
        "a home box narrower than the touch floor is not a box a thumb can aim at",
    );
    const _: () = assert!(
        HIT.y + HIT.h == super::viewport::LOGICAL_HEIGHT,
        "the answering box must reach the panel's last row",
    );
    const _: () = assert!(
        HIT.y >= BAND.y,
        "the answering box must stay inside the band the shell owns",
    );
    const _: () = assert!(
        HIT.x + HIT_W == super::viewport::LOGICAL_WIDTH - HIT.x,
        "the answering box must be centred like the pill it covers",
    );
    const _: () = assert!(
        super::viewport::LOGICAL_HEIGHT - HIT_H >= BAND.y,
        "a box of the touch floor's height must still fit inside the band",
    );
}

#[cfg(test)]
mod tests {
    use super::viewport::{
        logical_to_surface, scaled, surface_to_logical, DisplayOrientation, DisplayTransform,
        LOGICAL_HEIGHT, LOGICAL_WIDTH, SCALE_DEN, SCALE_NUM, SCANOUT_HEIGHT, SCANOUT_WIDTH,
    };

    #[test]
    fn the_scanout_is_the_authoring_space_and_needs_no_resample() {
        // The contract this module now states: composition is a copy. If this
        // fails, a mapping below that assumes identity is wrong.
        assert_eq!((SCANOUT_WIDTH, SCANOUT_HEIGHT), (LOGICAL_WIDTH, LOGICAL_HEIGHT));
        assert_eq!(SCALE_NUM, SCALE_DEN);
        assert!(!scaled(), "the scanout must not be an upscale of the authoring space");
    }

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
        // The ratio is a rational rather than a single integer so a future
        // scanout can be a fraction of the authoring space; today it is 1. The
        // round trip has to hold at any ratio: a hit test that answered for a
        // pixel other than the one the user touched is the same bug whether the
        // scanout is upscaled or exact. Every logical point must come back out
        // of the inverse, and at 1:1 it must come back unchanged.
        for lx in 0..LOGICAL_WIDTH {
            for ly in (0..LOGICAL_HEIGHT).step_by(97) {
                let (px, py) = logical_to_surface(lx, ly);
                assert_eq!((px, py), (lx, ly), "the 1:1 map is not the identity for ({lx}, {ly})");
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
    fn scale_extent_agrees_with_the_mapped_corners() {
        use super::viewport::scale_extent;
        // The extent helper and the corner map are two spellings of the same
        // agreement, and at 1:1 they must agree exactly -- a painter that took
        // one width from `scale_extent` and its edges from `logical_to_surface`
        // would otherwise draw a rectangle one pixel wider than its layout.
        for w in [0usize, 1, 2, 8, 200, 719, 720] {
            let (sw, _) = scale_extent(w, 0);
            let (x0, _) = logical_to_surface(40, 0);
            let (x1, _) = logical_to_surface(40 + w, 0);
            assert_eq!(sw, x1 - x0, "extent {w} disagrees with its mapped corners");
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
    fn coordinate_rounding_and_snapping_at_native_scale() {
        // The scanout equals the authoring space, so the map is exact rather
        // than rounded: a logical coordinate is the pixel that owns it.
        assert_eq!(logical_to_surface(0, 0), (0, 0));
        assert_eq!(logical_to_surface(1, 1), (1, 1));
        assert_eq!(logical_to_surface(2, 2), (2, 2));
        // The exclusive right/bottom edge maps to the exclusive surface edge,
        // which is what lets `scale_rect` report a full-width card as 720 wide
        // rather than 721.
        assert_eq!(logical_to_surface(LOGICAL_WIDTH, LOGICAL_HEIGHT), (720, 1440));
    }

    #[test]
    fn adjacent_rectangles_and_dividers_abut_without_seams() {
        use super::viewport::{scale_rect, scale_stroke};
        // Two adjacent cards: Card A [x: 40, w: 200] and Card B [x: 240, w: 200]
        let (ax, _, aw, _) = scale_rect(40, 100, 200, 50);
        let (bx, _, _bw, _) = scale_rect(240, 100, 200, 50);
        assert_eq!(ax + aw, bx, "Card A right edge must align exactly with Card B left edge");
        assert_eq!(aw, 200, "at 1:1 a card keeps its authored width");

        // Thin 1px divider at logical y=100
        let (dx, _dy, dw, _dh) = scale_rect(0, 100, 720, 1);
        assert_eq!((dx, dw), (0, 720));
        assert_eq!(scale_stroke(1), 1); // 1px logical stays 1px: no resample
        assert_eq!(scale_stroke(0), 0);
    }

    #[test]
    fn stroke_widths_and_corner_radii_scale_proportionally() {
        use super::viewport::{scale_radius, scale_stroke};
        // At 1:1 the authored number is already the physical number, which is
        // the point of painting the scanout in the authoring space: a 1px
        // border stays 1px instead of rounding to 2.
        assert_eq!(scale_radius(8), 8);   // radius::SMALL
        assert_eq!(scale_radius(12), 12); // radius::CONTROL
        assert_eq!(scale_radius(18), 18); // radius::CARD
        assert_eq!(scale_radius(24), 24); // radius::SHEET
        assert_eq!(scale_stroke(2), 2);
        // The property the ratio exists to preserve at any scale: a non-zero
        // stroke cannot round down to nothing.
        assert_eq!(scale_stroke(1), 1);
    }

    #[test]
    fn touch_coordinates_mapping_and_clamping() {
        use super::viewport::touch_to_logical;
        // Origin
        assert_eq!(touch_to_logical(0, 0), (0, 0));
        // Middle of the surface: at 1:1 a physical point is the logical point
        assert_eq!(touch_to_logical(360, 720), (360, 720));
        // Far edge
        assert_eq!(touch_to_logical(719, 1439), (719, 1439));
        // The exclusive edge is clamped into the last addressable pixel, so a
        // driver that reports width/height instead of width-1/height-1 still
        // produces an in-bounds logical point.
        assert_eq!(touch_to_logical(720, 1440), (719, 1439));
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
        use super::viewport::STATUS_BAR_HEIGHT;
        assert!(back_button::contains(24, back_button::Y));
        assert!(back_button::contains(71, back_button::Y + 47));
        assert!(!back_button::contains(23, back_button::Y));
        assert!(!back_button::contains(72, back_button::Y + 48));
        // The whole control clears the band that paints over it.
        assert!(back_button::Y >= STATUS_BAR_HEIGHT);
        assert!(back_button::pixel(48, back_button::Y + 24, 0xFF000000, 0xFF888888, 0xFFFFFFFF).is_some());
    }

    /// The ring is the chip's own outline, all the way round it.
    ///
    /// WHY this is a test: the ring used to be painted on the bounding box, so
    /// the four corner arcs -- a quarter of the chip's visible edge -- had no
    /// ring at all and the control read as four brackets around a soft square.
    /// Nothing in the old assertions could see that, because each one sampled a
    /// flat edge, where the box and the shape agree. This walks the whole chip
    /// and asks the one question that matters: does the outline close?
    #[test]
    fn the_back_controls_ring_is_a_closed_outline() {
        use super::back_button;
        const INK: u32 = 0xFF000000;
        const RING: u32 = 0xFF888888;
        const PLATE: u32 = 0xFFFFFFFF;

        let painted = |x: i32, y: i32| {
            if x < 0 || y < 0 {
                return None;
            }
            back_button::pixel(x as usize, y as usize, INK, RING, PLATE)
        };

        let mut arcs = 0;
        for y in back_button::Y as i32..back_button::Y as i32 + back_button::SIZE as i32 {
            for x in back_button::X as i32..back_button::X as i32 + back_button::SIZE as i32 {
                if painted(x, y) == None {
                    continue;
                }
                let on_edge = [(1, 0), (-1, 0), (0, 1), (0, -1)]
                    .iter()
                    .any(|(dx, dy)| painted(x + dx, y + dy).is_none());
                if on_edge {
                    assert_eq!(
                        painted(x, y),
                        Some(RING),
                        "({x}, {y}) bounds the chip's own silhouette but is not ring: the outline has a gap"
                    );
                    if x != back_button::X as i32
                        && x != back_button::X as i32 + back_button::SIZE as i32 - 1
                        && y != back_button::Y as i32
                        && y != back_button::Y as i32 + back_button::SIZE as i32 - 1
                    {
                        arcs += 1;
                    }
                }
            }
        }

        // The ring pixels that are not on the box's own edge are the corner
        // arcs. A square chip would score zero here, and so would the old
        // box-edge ring: the arcs are the part that used to be missing.
        assert!(
            arcs > 4 * back_button::RADIUS / 2,
            "the outline hugs the bounding box rather than the rounded plate: {arcs} arc pixels"
        );
    }

    /// The Back control's colours come from roles that are visible on the page
    /// the control sits on, in every theme the platform ships.
    ///
    /// WHY this is a test and not a comment: the defect it closes was a ring
    /// at 1.21:1 and a fill at 1.11:1 against the page, each chosen by hand at
    /// nine call sites. A contrast floor that lives only in prose is a floor
    /// the next theme falls through quietly, and the failure is invisible --
    /// the control does not error, it simply stops being a button.
    #[test]
    fn the_back_control_is_visible_on_every_shipped_theme() {
        use super::accessibility::contrast_ratio_x100;
        use super::back_button;
        use super::monochrome;
        use super::theme::Theme;

        // WCAG 1.4.11's floor for the visual boundary of a user interface
        // component, on the x100 scale the helper returns.
        const MIN_BOUNDARY_X100: u32 = 300;

        for theme in [
            Theme::LIGHT,
            Theme::DARK,
            Theme::NEUTRAL,
            monochrome::THEME,
            monochrome::THEME_DARK,
        ] {
            let (ink, ring, fill) = back_button::colors_for_theme(&theme);

            // The boundary has two sides, and both have to read: the ring
            // against the page it stands on, and against the chip it encloses.
            let ring_on_page = contrast_ratio_x100(ring, theme.surface);
            let ring_on_fill = contrast_ratio_x100(ring, fill);
            assert!(
                ring_on_page >= MIN_BOUNDARY_X100,
                "{}: the back ring {:08x} is {} on the page {:08x}, under the 3:1 floor",
                theme.name,
                ring,
                ring_on_page,
                theme.surface
            );
            assert!(
                ring_on_fill >= MIN_BOUNDARY_X100,
                "{}: the back ring {:08x} is {} against its own chip {:08x}, so the \
                 chip has no edge",
                theme.name,
                ring,
                ring_on_fill,
                fill
            );

            // The chip is a step of its own rather than the page it sits on.
            assert_ne!(
                fill, theme.surface,
                "{}: the back chip is filled with its own page, so it has no body",
                theme.name
            );

            // And the chevron reads on the chip it is drawn into.
            let ink_on_fill = contrast_ratio_x100(ink, fill);
            assert!(
                ink_on_fill >= MIN_BOUNDARY_X100,
                "{}: the chevron {:08x} is {} on the chip {:08x}",
                theme.name,
                ink,
                ink_on_fill,
                fill
            );

            // The themed entry point paints those three roles in the three
            // places they belong: the ring on the edge, the chevron in the
            // middle, the fill between them.
            let edge = back_button::X + back_button::RADIUS;
            assert_eq!(
                back_button::pixel_for_theme(edge, back_button::Y, &theme),
                Some(ring),
                "{}: the control's top edge is not its ring",
                theme.name
            );
            assert_eq!(
                back_button::pixel_for_theme(
                    back_button::X + back_button::SIZE / 2 - 4,
                    back_button::Y + back_button::SIZE / 2,
                    &theme
                ),
                Some(ink),
                "{}: the chevron's tip is not its ink",
                theme.name
            );
            assert_eq!(
                back_button::pixel_for_theme(
                    back_button::X + back_button::SIZE / 2 + 8,
                    back_button::Y + back_button::SIZE / 2,
                    &theme
                ),
                Some(fill),
                "{}: the control's body is not its fill",
                theme.name
            );
        }

        // The numbers the fix was chosen for, pinned so a later theme that
        // trades `outline` for a hairline is a test failure and not a screen
        // where the Back control has gone missing.
        let (ink, ring, fill) = back_button::colors_for_theme(&monochrome::THEME);
        assert_eq!((ink, ring, fill), (0xFF0A0A0A, 0xFF0A0A0A, 0xFFEF_EEE9));
        assert!(
            contrast_ratio_x100(ring, monochrome::THEME.surface) >= 1500,
            "the monochrome back ring is no longer the dark edge it was chosen to be"
        );
    }

    /// The chip's mark is the size the platform draws its glyphs at, and it
    /// keeps its distance from the ring.
    ///
    /// WHY measured off [`back_button::pixel`] and not off the constants: the
    /// defect this pins was a mark nobody sized -- nine columns across a plate
    /// forty-eight wide, which on the device is a scratch, not an arrow. A
    /// resized mark can also fail the other way, by running into the ring it is
    /// supposed to sit inside, and neither failure is visible in a constant.
    #[test]
    fn the_back_chip_marks_a_glyph_the_platform_sizes_and_a_ring_it_clears() {
        use super::back_button;
        use super::monochrome::ICON_BOX;

        let (width, height, widest_row) = back_button::mark_metrics();

        // The glyph fills the platform's icon box, and its arms are at least
        // three rows across so the mark survives the scanout.
        assert!(
            height >= ICON_BOX,
            "the back chip's mark is {} rows tall on a {} chip; the platform draws \
             its glyphs to fill {}",
            height,
            back_button::SIZE,
            ICON_BOX
        );
        assert!(
            width >= ICON_BOX / 2,
            "the back chip's mark is only {} columns wide, which is the scratch this \
             mark existed to replace",
            width
        );
        assert!(
            widest_row >= 3,
            "an arm {} rows across is a hairline, not a stroke",
            widest_row
        );

        // The mark stays inside the ring's own square rather than crossing it.
        let half = back_button::SIZE / 2;
        assert!(
            width / 2 < half - back_button::RADIUS / 2 && height / 2 < half - 1,
            "the back chip's {}x{} mark crowds the ring on a {} chip",
            width,
            height,
            back_button::SIZE
        );

        // The mark never crowds the ring it is supposed to sit inside: walk the
        // plate and every ink pixel answers its four neighbours with ink or with
        // the chip's own fill. A mark grown past its plate shows up here as a
        // neighbour that came back as the ring, or as nothing at all.
        let (x_lo, x_hi) = (back_button::X + 1, back_button::X + back_button::SIZE - 1);
        let (y_lo, y_hi) = (back_button::Y + 1, back_button::Y + back_button::SIZE - 1);
        for y in y_lo..y_hi {
            for x in x_lo..x_hi {
                let painted = back_button::pixel(
                    x,
                    y,
                    back_button::INK_PROBE,
                    back_button::RING_PROBE,
                    back_button::FILL_PROBE,
                );
                if painted == Some(back_button::INK_PROBE) {
                    for (nx, ny) in [(x - 1, y), (x + 1, y), (x, y - 1), (x, y + 1)] {
                        let neighbour = back_button::pixel(
                            nx,
                            ny,
                            back_button::INK_PROBE,
                            back_button::RING_PROBE,
                            back_button::FILL_PROBE,
                        );
                        assert!(
                            neighbour == Some(back_button::INK_PROBE)
                                || neighbour == Some(back_button::FILL_PROBE),
                            "the mark at ({x}, {y}) touches the ring or the plate's edge \
                             at ({nx}, {ny}), so the chip no longer has an edge of its own"
                        );
                    }
                }
            }
        }
    }

    /// The home gesture answers a box that covers the pill it paints, is on the
    /// touch floor, and stops short of the columns the application owns.
    ///
    /// WHY measured this way: the defect was not four boxes but six presses that
    /// tested the ink -- camera, gallery, Messages, video player and file manager
    /// all answered the pill's own six rows, so a thumb on row 1410 landed on a
    /// screen that reported nothing. The corners of the answering box are where
    /// that thumb lands; the column beside the box is where an app's own list row
    /// is, and the gesture must not take it.
    #[test]
    fn the_home_band_answers_a_touch_floor_box_around_the_pill_it_paints() {
        use super::home_band;
        use super::touch::{MIN_TARGET, PREFERRED_TARGET};
        use super::viewport::{LOGICAL_HEIGHT, LOGICAL_WIDTH};

        // Every corner of the box answers, and the pill's own centre with it.
        for (x, y) in [
            (home_band::HIT.x, home_band::HIT.y),
            (home_band::HIT.x + home_band::HIT.w - 1, home_band::HIT.y),
            (home_band::HIT.x, LOGICAL_HEIGHT - 1),
            (home_band::HIT.x + home_band::HIT.w - 1, LOGICAL_HEIGHT - 1),
            (LOGICAL_WIDTH / 2, home_band::PILL.y + home_band::PILL_H / 2),
        ] {
            assert!(
                home_band::contains(x, y),
                "the home box does not answer its own pixel at ({x}, {y})"
            );
        }

        // The box is a target, not the ink: the pill is six rows and the floor is
        // forty-four, and the six-row press is what the apps were doing.
        assert_eq!(home_band::PILL_H, 6);
        assert!(
            home_band::HIT.h >= MIN_TARGET,
            "the home box is {} rows, under the {} row floor",
            home_band::HIT.h,
            MIN_TARGET
        );
        assert!(home_band::HIT.h >= PREFERRED_TARGET);
        assert!(
            home_band::HIT.h > home_band::PILL_H,
            "the answering box is no taller than the ink, so the press is the ink"
        );
        assert!(
            home_band::HIT.w > home_band::PILL_W,
            "the answering box is no wider than the ink, so a thumb beside the mark misses"
        );

        // Rows above the box belong to the application, and so do the columns
        // beside it -- a list scrolled to its last row is still tappable.
        assert!(!home_band::contains(LOGICAL_WIDTH / 2, home_band::HIT.y - 1));
        assert!(!home_band::contains(home_band::HIT.x - 1, LOGICAL_HEIGHT - 1));
        assert!(!home_band::contains(
            home_band::HIT.x + home_band::HIT.w,
            LOGICAL_HEIGHT - 1
        ));

        // An app that holds the bottom of the screen with its own control starts
        // the same box where that control stops, and the pill stays inside it.
        let toolbar_top = home_band::BAND.y + 8;
        let lowered = home_band::hit_from(toolbar_top);
        assert_eq!(lowered.x, home_band::HIT.x);
        assert_eq!(lowered.w, home_band::HIT.w);
        assert_eq!(lowered.y + lowered.h, LOGICAL_HEIGHT);
        assert!(lowered.y >= toolbar_top);

        // The pill paints, and nothing outside the pill's box does.
        let ink = 0xFF80_8080;
        let mut painted = 0usize;
        for y in home_band::PILL.y..home_band::PILL.y + home_band::PILL_H {
            for x in home_band::PILL.x..home_band::PILL.x + home_band::PILL_W {
                if home_band::pixel(x, y, ink) == Some(ink) {
                    painted += 1;
                }
            }
        }
        assert!(
            painted >= home_band::PILL_W * home_band::PILL_H / 2,
            "the pill inks {} of its {} rows x columns, which is not a mark",
            painted,
            home_band::PILL_W
        );
        assert_eq!(
            home_band::pixel(home_band::PILL.x - 1, home_band::PILL.y, ink),
            None,
            "the pill painted outside its own box"
        );
    }

    #[test]
    fn display_metrics_dp_px_conversions_and_hairline() {
        use super::{DisplayMetrics, Dp, Px};
        let metrics = DisplayMetrics::canonical_phone();
        assert_eq!(metrics.hairline_px(), Px(1));
        // 1.0x: a dp is a pixel at every size, including the far corner, where
        // a 1.5x profile rounded 719 up to 1079 and lost the exactness.
        assert_eq!(metrics.dp_to_px(Dp(0)), Px(0));
        assert_eq!(metrics.dp_to_px(Dp(10)), Px(10));
        assert_eq!(metrics.dp_to_px(Dp(719)), Px(719));
        assert_eq!(metrics.dp_to_px(Dp(720)), Px(720));
        assert_eq!(metrics.dp_to_px(Dp(1440)), Px(1440));
        assert_eq!(metrics.px_to_dp(Px(0)), Dp(0));
        assert_eq!(metrics.px_to_dp(Px(719)), Dp(719));
        assert_eq!(metrics.px_to_dp(Px(720)), Dp(720));
        assert_eq!(metrics.px_to_dp(Px(1440)), Dp(1440));
        // Negative values stay symmetric, so a mirrored coordinate keeps its
        // magnitude on the way out and back.
        assert_eq!(metrics.dp_to_px(Dp(-10)), Px(-10));
        assert_eq!(metrics.px_to_dp(Px(-10)), Dp(-10));
        // The profile itself: the platform's authoring-space density, not an
        // FHD panel the guest never painted at.
        assert_eq!(metrics.density_dpi, 320);
        assert_eq!(metrics.scale_factor_permille, 1000);
        assert_eq!(metrics.physical_width_px, 720);
        assert_eq!(metrics.physical_height_px, 1440);
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
        assert_eq!(landscape.physical_width_px, 1440);
        assert_eq!(landscape.physical_height_px, 720);
        assert_eq!(landscape.logical_width_dp, 1440);
        assert_eq!(landscape.logical_height_dp, 720);
        assert_eq!(landscape.safe_insets_px.right, Px(48)); // rotated top inset
    }
}
