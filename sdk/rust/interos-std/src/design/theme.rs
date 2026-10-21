//! The platform theme: the roles an interface asks for, not raw colours.
//!
//! WHY this module exists: [`super::color`] is a palette. It says which colours
//! exist, and nothing about which one a card is filled with, which one a caption
//! is inked in, or which one marks the selected tab. Every app that wanted a card
//! therefore chose a colour itself, and each app chose a different one. A
//! third-party app linking this SDK had no way to ask what the platform
//! considered a card, because the platform had no answer to give.
//!
//! A theme is that answer, and it is expressed in *roles*. An app asks for
//! `theme.surface_container` and gets the fill a card carries on this platform,
//! in the variant the user has selected. The value behind the role is a
//! platform decision, so changing the palette is a change here and not a change
//! in every app that draws a card.
//!
//! WHY a struct and not a pile of constants: a constant cannot be swapped, and a
//! theme that cannot be swapped cannot be followed into a dark variant. Two
//! instances ship -- [`Theme::LIGHT`] and [`Theme::DARK`] -- and an app reads
//! whichever one it is handed.
//!
//! WHY the roles also carry the non-colour tokens: an interface that asks for a
//! surface and then reaches for a spacing step from somewhere else is still
//! assembling itself out of two systems. The type scale, the spacing scale, the
//! radii and the touch minimum live on the theme for the same reason the colours
//! do: so that one value describes the whole look of an app.

use super::color;
use super::radius;
use super::spacing;
use super::touch;
use super::typography;
use super::walls;

/// The type scale: the sizes type may be drawn at, with their line boxes.
///
/// WHY this is a view of [`super::typography`] and not a second scale: the type
/// scale already exists, with the line heights and baselines the rasteriser
/// needs. A copy of the multipliers alone would let an app pick a size the
/// rasteriser has no line box for.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct TypeScale {
    /// Badge glyphs inside an icon, e.g. a call badge.
    pub badge: typography::TextStyle,
    /// Dense metadata: a status-bar clock, a caption, a chip label.
    pub caption: typography::TextStyle,
    /// Body copy and list labels.
    pub body: typography::TextStyle,
    /// Section headings.
    pub title: typography::TextStyle,
    /// Display type: a lock-screen clock.
    pub display: typography::TextStyle,
}

impl TypeScale {
    /// The platform's type scale.
    pub const STANDARD: TypeScale = TypeScale {
        badge: typography::BADGE,
        caption: typography::CAPTION,
        body: typography::BODY,
        title: typography::TITLE,
        display: typography::DISPLAY,
    };
}

/// The spacing scale: the gaps an interface may put between things.
///
/// WHY this is a view of [`super::spacing`] and not a second scale: an 8pt grid
/// is what makes a layout read as designed, and a second copy of the steps is
/// how a grid stops being one.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Spacing {
    /// 4 logical pixels: the gap inside a control.
    pub xs: usize,
    /// 8 logical pixels: the gap between related items.
    pub sm: usize,
    /// 16 logical pixels: the gap between a control and its label.
    pub md: usize,
    /// 24 logical pixels: the gap between a card and the content around it.
    pub lg: usize,
    /// 32 logical pixels: the gap between sections.
    pub xl: usize,
    /// 48 logical pixels: the gap between major blocks.
    pub xxl: usize,
}

impl Spacing {
    /// The platform's spacing scale.
    pub const STANDARD: Spacing = Spacing {
        xs: spacing::XS,
        sm: spacing::SM,
        md: spacing::MD,
        lg: spacing::LG,
        xl: spacing::XL,
        xxl: spacing::XXL,
    };
}

/// The corner radii: how round a shape is allowed to be.
///
/// WHY this is a view of [`super::radius`] and not a second scale: two scales
/// that carry the same names at different values is a trap -- a screen that
/// reaches for the wrong one draws a control with visibly different corners
/// from its siblings, and nothing in the type system objects.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Radii {
    /// 8: small surfaces and icon tiles.
    pub small: usize,
    /// 10: chips and badges.
    pub chip: usize,
    /// 12: buttons, interactive tiles and text fields.
    pub control: usize,
    /// 16: a large control -- a 40-48px pill that is not a capsule.
    pub soft_pill: usize,
    /// 18: cards and list rows.
    pub card: usize,
    /// 24: sheets and full-width panels.
    pub sheet: usize,
    /// 32: large panels and dialogs.
    pub major: usize,
    /// 44: the device's own display surface.
    pub screen: usize,
    /// 999: a fully rounded pill; clamped to half the shape's height.
    pub pill: usize,
}

impl Radii {
    /// The platform's radius scale.
    pub const STANDARD: Radii = Radii {
        small: radius::SMALL,
        chip: radius::CHIP,
        control: radius::CONTROL,
        soft_pill: radius::SOFT_PILL,
        card: radius::CARD,
        sheet: radius::SHEET,
        major: radius::MAJOR,
        screen: radius::SCREEN,
        pill: radius::PILL,
    };
}

/// The touch targets: how large an interactive thing may be drawn and hit.
///
/// WHY the minimum and the preferred size are different numbers: the minimum is
/// the floor below which a target stops being reliably hittable, and the
/// preferred size is what a layout should use when it has the room.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Touch {
    /// The smallest edge length an interactive target may have.
    pub min_target: usize,
    /// The edge length a target should have when there is room for it.
    pub preferred_target: usize,
    /// The edge length of the single most important action on a screen.
    pub primary_action: usize,
}

impl Touch {
    /// The platform's touch targets.
    pub const STANDARD: Touch = Touch {
        min_target: touch::MIN_TARGET,
        preferred_target: touch::PREFERRED_TARGET,
        primary_action: touch::PRIMARY_ACTION,
    };
}

/// A complete visual language.
///
/// Colours are `0xAARRGGBB`. Every field is a *role*: it names the job the
/// colour does, so the same field resolves to a different colour in a different
/// variant and the app does not change.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Theme {
    /// The variant's name, e.g. `"light"`.
    pub name: &'static str,
    /// Whether this variant is the dark one.
    ///
    /// WHY a field and not a name comparison: a third theme could be dark
    /// without being called "dark", and a surface that asked about its name
    /// would get the wrong answer.
    pub dark: bool,

    // --- Surfaces -----------------------------------------------------------

    /// The page background, behind everything.
    pub surface: u32,
    /// A resting card, sheet or panel.
    pub surface_container: u32,
    /// One elevation step above [`Theme::surface_container`]: a raised card, a
    /// chip, a pressed control.
    pub surface_container_high: u32,
    /// One step below [`Theme::surface_container`]: a well, a track, a disabled
    /// field.
    pub surface_container_low: u32,
    /// A card that floats over a picture or a gradient, e.g. a home-screen
    /// widget over the wallpaper.
    ///
    /// WHY it is its own role: a card over the wallpaper is mixed with what is
    /// behind it, and a card on a flat page is not, so one value cannot serve
    /// both.
    pub surface_frosted: u32,
    /// Chrome that floats over content: a status bar, a dock, a sheet.
    pub overlay: u32,
    /// The divider drawn over [`Theme::overlay`].
    pub overlay_border: u32,

    // --- Ink ----------------------------------------------------------------

    /// Body and heading ink.
    pub on_surface: u32,
    /// Secondary ink: captions, metadata, units.
    pub on_surface_variant: u32,
    /// The least prominent ink: a subtitle, a unit, a disabled label.
    pub muted: u32,
    /// Ink on a [`Theme::primary`] fill.
    pub on_primary: u32,
    /// Ink on a [`Theme::primary_container`] fill.
    pub on_primary_container: u32,
    /// The ink to place on a light backdrop that is not part of this theme.
    ///
    /// WHY it is separate from [`Theme::on_surface`]: `on_surface` is the
    /// theme's own body ink and is light on a dark theme, while this is the ink
    /// for *whatever colour this pixel happens to be*, which a contrast helper
    /// must return.
    pub ink_on_light: u32,
    /// The ink to place on a dark backdrop outside this theme's palette.
    pub ink_on_dark: u32,

    // --- Lines --------------------------------------------------------------

    /// A prominent outline: a focused field, a selected control.
    pub outline: u32,
    /// A hairline divider: the quietest line the platform draws.
    pub outline_variant: u32,
    /// The hairline a card is outlined with.
    pub border: u32,

    // --- Brand --------------------------------------------------------------

    /// The brand colour: the fill of a primary action.
    pub primary: u32,
    /// A low-emphasis container tinted with the brand colour.
    pub primary_container: u32,
    /// A secondary emphasis tone, for content that must not read as the primary
    /// action.
    pub secondary: u32,
    /// The expressive accent, for a highlight that is not a brand action.
    pub accent: u32,

    // --- Status -------------------------------------------------------------

    /// A failure, a refusal, a destructive action.
    pub error: u32,
    /// A low-emphasis container tinted with the failure colour.
    ///
    /// WHY it is its own role: a destructive control that is filled with the
    /// failure colour shouts; a destructive control that is filled with the
    /// failure colour's own tint says the same thing at the volume the rest of
    /// the interface speaks at. A calculator's "clear" key and a dialer's "SOS"
    /// key are that control, and neither of them is a page.
    pub error_container: u32,
    /// A success, a healthy reading.
    pub success: u32,
    /// A low-emphasis container tinted with the success colour.
    ///
    /// WHY it is its own role: the same reasoning as [`Theme::error_container`]
    /// -- a control that carries a success is tinted rather than filled, and
    /// a dialer's call key is that control.
    pub success_container: u32,
    /// The ink on a success-tinted container.
    pub on_success_container: u32,
    /// A caution, a reading that is outside the expected range.
    pub warning: u32,
    /// A low-emphasis container tinted with the caution colour.
    ///
    /// WHY it is its own role: the same reasoning as [`Theme::error_container`]
    /// -- a warning is a state the interface reports, and a control that
    /// carries one is tinted rather than filled.
    pub warning_container: u32,
    /// The alarm red: the emergency action, the failed credential.
    ///
    /// WHY it is the same in both variants: this is a platform colour rather
    /// than a themed one. An alarm that changes hue with the theme is an alarm
    /// that can be mistaken for an ordinary error.
    pub emergency: u32,

    // --- Wallpaper ----------------------------------------------------------

    /// The top stop of the wallpaper ramp.
    pub wallpaper_top: u32,
    /// The middle stop of the wallpaper ramp.
    pub wallpaper_mid: u32,
    /// The bottom stop of the wallpaper ramp.
    pub wallpaper_bottom: u32,
    /// Ink for type drawn over the wallpaper's upper region.
    pub wallpaper_ink: u32,

    // --- Metrics ------------------------------------------------------------

    /// The type scale.
    pub type_scale: TypeScale,
    /// The spacing scale.
    pub spacing: Spacing,
    /// The corner radii.
    pub radii: Radii,
    /// The touch targets.
    pub touch: Touch,
}

impl Theme {
    /// The light system: near-white pages, white cards, dark ink.
    pub const LIGHT: Theme = Theme {
        name: "light",
        dark: false,

        surface: color::BACKGROUND,
        surface_container: color::SURFACE,
        surface_container_high: color::SURFACE_RAISED,
        surface_container_low: color::SURFACE_SUNKEN,
        surface_frosted: 0xFFF5F5F0,
        overlay: color::SURFACE,
        overlay_border: color::BORDER,

        on_surface: color::TEXT,
        on_surface_variant: color::TEXT_MUTED,
        muted: 0xFF6B7280,
        on_primary: 0xFFFFFFFF,
        on_primary_container: 0xFF0A2A5E,
        ink_on_light: 0xFF111214,
        ink_on_dark: 0xFFFFFFFF,

        outline: 0xFF3A3D42,
        outline_variant: 0xFFE5E5DF,
        border: color::BORDER,

        primary: color::ACCENT,
        primary_container: 0xFFDCE7FB,
        secondary: 0xFF5B6472,
        accent: 0xFF7C3AED,

        error: color::DANGER,
        error_container: 0xFFFBE0E0,
        success: color::SUCCESS,
        success_container: 0xFFD1FAE5,
        on_success_container: 0xFF059669,
        warning: color::WARNING,
        warning_container: 0xFFFCEFD8,
        emergency: color::EMERGENCY,

        wallpaper_top: 0xFF9CB8D6,
        wallpaper_mid: 0xFF6E7F8C,
        wallpaper_bottom: 0xFF241F1C,
        wallpaper_ink: 0xFF111214,

        type_scale: TypeScale::STANDARD,
        spacing: Spacing::STANDARD,
        radii: Radii::STANDARD,
        touch: Touch::STANDARD,
    };

    /// The dark system: near-black pages, lifted cards, light ink.
    ///
    /// WHY the same roles resolve to different colours here: a role names a
    /// job, and the job reads the same in either variant. An app that asks for
    /// `surface_container` in the dark theme gets the dark theme's card fill,
    /// without knowing that a dark theme exists.
    pub const DARK: Theme = Theme {
        name: "dark",
        dark: true,

        surface: color::DARK_BACKGROUND,
        surface_container: color::DARK_SURFACE,
        surface_container_high: color::DARK_SURFACE_RAISED,
        surface_container_low: color::DARK_SURFACE_SUNKEN,
        surface_frosted: 0xFF2A2D30,
        overlay: color::DARK_SURFACE_SUNKEN,
        overlay_border: color::DARK_BORDER,

        on_surface: color::DARK_TEXT,
        on_surface_variant: color::DARK_TEXT_MUTED,
        muted: 0xFF8A9199,
        on_primary: 0xFF111214,
        on_primary_container: 0xFFD6E6FF,
        ink_on_light: 0xFF111214,
        ink_on_dark: 0xFFFFFFFF,

        outline: 0xFFB4B9C2,
        outline_variant: 0xFF2A2D30,
        border: color::DARK_BORDER,

        primary: color::DARK_ACCENT,
        primary_container: 0xFF16324F,
        secondary: 0xFFB4BECD,
        accent: 0xFFB794F6,

        error: color::DARK_DANGER,
        error_container: 0xFF3A1418,
        success: color::DARK_SUCCESS,
        success_container: 0xFF12372A,
        on_success_container: 0xFFA7F3D0,
        warning: color::DARK_WARNING,
        warning_container: 0xFF3A2C0A,
        emergency: color::EMERGENCY,

        wallpaper_top: 0xFF1B2733,
        wallpaper_mid: 0xFF12161A,
        wallpaper_bottom: 0xFF07080A,
        wallpaper_ink: 0xFFF9F9F8,

        type_scale: TypeScale::STANDARD,
        spacing: Spacing::STANDARD,
        radii: Radii::STANDARD,
        touch: Touch::STANDARD,
    };

    /// Selects a theme by name, defaulting to light for an unknown name.
    ///
    /// WHY a default and not a refusal: a theme name that arrives from a
    /// settings store is untrusted text, and a surface that refused to draw
    /// because of it would be a worse failure than drawing the light theme.
    pub fn by_name(name: &str) -> Theme {
        match name {
            "dark" => Theme::DARK,
            _ => Theme::LIGHT,
        }
    }

    /// Whether this theme is dark, i.e. whether its ink is lighter than its
    /// backdrop.
    pub const fn is_dark(&self) -> bool {
        self.dark
    }

    /// Blends two `0xAARRGGBB` values. `t` is 0..=256.
    ///
    /// WHY the SDK's blend and not a second one: the wallpaper ramp, the veil
    /// and the compositor's own mixing all have to agree to the bit, or a
    /// gradient shows a seam where two implementations meet.
    pub fn blend(a: u32, b: u32, t: u32) -> u32 {
        walls::blend(a, b, t)
    }

    /// Applies a translucent white or black veil, for pressed states and for
    /// controls that float over the wallpaper.
    ///
    /// WHY the target is an OPAQUE colour in the veil's own direction: white
    /// for a light backdrop, black otherwise. Building the target as
    /// `(target << 24) | 0xFFFFFF` keeps white in the colour channels for both
    /// directions, so a dark veil blends towards white instead of black and
    /// light ink drawn on the result is invisible.
    pub fn veil(base: u32, percent: u32, light: bool) -> u32 {
        let p = percent.min(100);
        let t = (p * 256) / 100;
        let target = if light { 0xFFFFFFFFu32 } else { 0xFF000000u32 };
        Theme::blend(base, target, t)
    }

    /// The colour of `t` across the wallpaper ramp, `t` in 0..=256.
    pub fn ramp3(&self, t: u32) -> u32 {
        let t = t.min(256);
        if t < 128 {
            Theme::blend(self.wallpaper_top, self.wallpaper_mid, t * 2)
        } else {
            Theme::blend(self.wallpaper_mid, self.wallpaper_bottom, (t - 128) * 2)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Radii, Spacing, Theme, TypeScale, Touch};

    /// Every role the platform names, in the order the theme declares them.
    const ROLES: &[&str] = &[
        "surface",
        "surface_container",
        "surface_container_high",
        "surface_container_low",
        "surface_frosted",
        "overlay",
        "overlay_border",
        "on_surface",
        "on_surface_variant",
        "muted",
        "on_primary",
        "on_primary_container",
        "ink_on_light",
        "ink_on_dark",
        "outline",
        "outline_variant",
        "border",
        "primary",
        "primary_container",
        "secondary",
        "accent",
        "error",
        "error_container",
        "success",
        "success_container",
        "on_success_container",
        "warning",
        "warning_container",
        "emergency",
        "wallpaper_top",
        "wallpaper_mid",
        "wallpaper_bottom",
        "wallpaper_ink",
    ];

    fn role(theme: &Theme, name: &str) -> u32 {
        match name {
            "surface" => theme.surface,
            "surface_container" => theme.surface_container,
            "surface_container_high" => theme.surface_container_high,
            "surface_container_low" => theme.surface_container_low,
            "surface_frosted" => theme.surface_frosted,
            "overlay" => theme.overlay,
            "overlay_border" => theme.overlay_border,
            "on_surface" => theme.on_surface,
            "on_surface_variant" => theme.on_surface_variant,
            "muted" => theme.muted,
            "on_primary" => theme.on_primary,
            "on_primary_container" => theme.on_primary_container,
            "ink_on_light" => theme.ink_on_light,
            "ink_on_dark" => theme.ink_on_dark,
            "outline" => theme.outline,
            "outline_variant" => theme.outline_variant,
            "border" => theme.border,
            "primary" => theme.primary,
            "primary_container" => theme.primary_container,
            "secondary" => theme.secondary,
            "accent" => theme.accent,
            "error" => theme.error,
            "error_container" => theme.error_container,
            "success" => theme.success,
            "success_container" => theme.success_container,
            "on_success_container" => theme.on_success_container,
            "warning" => theme.warning,
            "warning_container" => theme.warning_container,
            "emergency" => theme.emergency,
            "wallpaper_top" => theme.wallpaper_top,
            "wallpaper_mid" => theme.wallpaper_mid,
            "wallpaper_bottom" => theme.wallpaper_bottom,
            "wallpaper_ink" => theme.wallpaper_ink,
            other => panic!("unknown role {other}"),
        }
    }

    #[test]
    fn every_role_is_present_and_opaque_in_both_variants() {
        for theme in [Theme::LIGHT, Theme::DARK] {
            for name in ROLES {
                let value = role(&theme, name);
                assert_ne!(value, 0, "{} is unset in the {} theme", name, theme.name);
                assert_eq!(
                    value >> 24,
                    0xFF,
                    "{} is not opaque in the {} theme",
                    name,
                    theme.name
                );
            }
        }
    }

    #[test]
    fn the_roles_that_must_differ_do_differ() {
        // WHY this list and not "every role differs": the light theme's
        // floating chrome and its resting card are the same white, and forcing
        // them apart would invent a colour the platform does not have. These
        // pairs are the ones where a collision would make an interface
        // unreadable or make two roles indistinguishable.
        const MUST_DIFFER: &[(&str, &str)] = &[
            ("surface", "surface_container"),
            ("surface", "surface_container_low"),
            ("surface_container", "surface_container_high"),
            ("surface_container_high", "surface_container_low"),
            ("surface_container", "surface_frosted"),
            ("on_surface", "on_surface_variant"),
            ("on_surface_variant", "muted"),
            ("on_surface", "surface"),
            ("on_surface", "surface_container"),
            ("on_primary", "primary"),
            ("on_primary_container", "primary_container"),
            ("primary", "primary_container"),
            ("primary", "secondary"),
            ("primary", "accent"),
            ("secondary", "accent"),
            ("primary", "error"),
            ("primary", "success"),
            ("primary", "warning"),
            ("error", "success"),
            ("error", "warning"),
            ("success", "warning"),
            ("error", "error_container"),
            ("success", "success_container"),
            ("warning", "warning_container"),
            ("error_container", "warning_container"),
            ("primary_container", "error_container"),
            ("primary_container", "warning_container"),
            ("border", "outline_variant"),
            ("border", "outline"),
            ("wallpaper_top", "wallpaper_mid"),
            ("wallpaper_mid", "wallpaper_bottom"),
        ];

        for theme in [Theme::LIGHT, Theme::DARK] {
            for (a, b) in MUST_DIFFER {
                assert_ne!(
                    role(&theme, a),
                    role(&theme, b),
                    "{} and {} are the same colour in the {} theme",
                    a,
                    b,
                    theme.name
                );
            }
        }
    }

    #[test]
    fn the_two_variants_are_two_systems() {
        // WHY these three are exempt: the two polarity helpers describe the
        // surface a pixel lands on rather than the theme it came from, and the
        // emergency red is a platform colour that must not change hue with the
        // theme. Every other role must resolve differently in the two variants,
        // or the "dark" theme is a rename of the light one and an app that
        // follows it draws light ink on a light surface.
        for name in ROLES {
            if *name == "ink_on_light" || *name == "ink_on_dark" || *name == "emergency" {
                continue;
            }
            assert_ne!(
                role(&Theme::LIGHT, name),
                role(&Theme::DARK, name),
                "{} is the same in both variants",
                name
            );
        }
        // And the polarity helpers are the same in both, because they describe
        // the surface they land on rather than the theme they came from.
        assert_eq!(Theme::LIGHT.ink_on_light, Theme::DARK.ink_on_light);
        assert_eq!(Theme::LIGHT.ink_on_dark, Theme::DARK.ink_on_dark);
        assert_eq!(Theme::LIGHT.emergency, Theme::DARK.emergency);
    }

    #[test]
    fn the_ink_on_a_surface_is_never_the_surface() {
        for theme in [Theme::LIGHT, Theme::DARK] {
            assert_ne!(theme.on_surface, theme.surface);
            assert_ne!(theme.on_surface, theme.surface_container);
            assert_ne!(theme.on_surface_variant, theme.surface);
            assert_ne!(theme.on_surface_variant, theme.surface_container);
            assert_ne!(theme.muted, theme.surface);
            assert_ne!(theme.muted, theme.surface_container);
        }
    }

    #[test]
    fn the_metrics_are_the_platforms_own() {
        // WHY this matters: the theme is the only thing an app has to reach a
        // token through, so a metric that quietly differed from the scale it
        // claims to be would be a second scale wearing the first one's name.
        assert_eq!(Theme::LIGHT.touch.min_target, super::super::touch::MIN_TARGET);
        assert_eq!(Theme::LIGHT.touch.min_target, 44);
        assert_eq!(Theme::LIGHT.touch.preferred_target, 48);
        assert_eq!(Theme::LIGHT.touch.primary_action, 56);

        assert_eq!(Theme::LIGHT.radii.control, super::super::radius::CONTROL);
        assert_eq!(Theme::LIGHT.radii.card, super::super::radius::CARD);
        assert_eq!(Theme::LIGHT.radii.pill, super::super::radius::PILL);
        assert_eq!(Theme::LIGHT.radii.chip, super::super::radius::CHIP);

        assert_eq!(Theme::LIGHT.spacing.md, super::super::spacing::MD);
        assert_eq!(Theme::LIGHT.spacing.xxl, super::super::spacing::XXL);

        assert_eq!(Theme::LIGHT.type_scale.body.scale, super::super::typography::BODY.scale);
        assert_eq!(Theme::LIGHT.type_scale.caption.scale, super::super::typography::CAPTION.scale);
    }

    #[test]
    fn the_metric_views_are_the_scales_they_claim_to_be() {
        assert_eq!(Radii::STANDARD.control, 12);
        assert_eq!(Radii::STANDARD.card, 18);
        assert_eq!(Radii::STANDARD.pill, 999);
        assert_eq!(Spacing::STANDARD.md, 16);
        assert_eq!(Touch::STANDARD.min_target, 44);
        assert_eq!(TypeScale::STANDARD.title.scale, 3);
    }

    #[test]
    fn a_theme_is_selected_by_name_and_defaults_to_light() {
        assert_eq!(Theme::by_name("dark").name, "dark");
        assert!(Theme::by_name("dark").is_dark());
        assert_eq!(Theme::by_name("light").name, "light");
        assert!(!Theme::by_name("light").is_dark());
        assert_eq!(Theme::by_name("nonsense").name, "light");
    }

    #[test]
    fn the_wallpaper_ramp_spans_top_to_bottom() {
        for theme in [Theme::LIGHT, Theme::DARK] {
            assert_eq!(theme.ramp3(0), theme.wallpaper_top);
            assert_eq!(theme.ramp3(256), theme.wallpaper_bottom);
            assert_ne!(theme.ramp3(128), theme.ramp3(0));
            assert_eq!(theme.ramp3(300), theme.ramp3(256));
        }
    }

    #[test]
    fn blending_endpoints_are_exact() {
        assert_eq!(Theme::blend(0xFF102030, 0xFFA0B0C0, 0), 0xFF102030);
        assert_eq!(Theme::blend(0xFF102030, 0xFFA0B0C0, 256), 0xFFA0B0C0);
    }

    #[test]
    fn a_dark_veil_darkens_and_a_light_veil_lightens() {
        let base = 0xFF808080;
        assert!(Theme::veil(base, 50, false) < base);
        assert!(Theme::veil(base, 50, true) > base);
    }
}
