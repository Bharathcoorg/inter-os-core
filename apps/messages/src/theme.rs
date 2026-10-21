//! The messages's view of the platform's design system.
//!
//! WHY this module exists: the app used to carry its own palette, its own
//! radius scale, its own spacing scale, its own type scale and its own touch
//! minimum, and the renderer reached for all of them. That is a second design
//! system inside one app: a third-party app linking the SDK had no way to
//! draw a messages that matched this one.
//!
//! It is now an adapter and nothing else. Every value below comes from the
//! SDK, so the app draws the platform's controls and a third-party app that
//! asks the SDK for the same roles gets the same pixels.
//!
//! WHY the historical names are still here, as a mapping: the app used to
//! carry its own vocabulary for the same colours. [`legacy`] is the record of
//! which platform role each old name became, so a reader who remembers the old
//! palette can find the new one instead of wondering where the old name went.

/// The platform's corner-radius scale.
pub use interos_std::design::radius;

/// The platform's spacing scale.
pub use interos_std::design::spacing;

/// The platform's theme: the roles an interface asks for.
pub use interos_std::design::theme::Theme;

/// The platform's type scale.
pub use interos_std::design::typography;

/// The platform's type scale, under the names the renderer already uses.
///
/// WHY the multipliers and not the styles: the renderer asks the rasteriser
/// for a size, and the style's line box is what the layout reserves. The scale
/// constants are the size; the styles live on the theme for an app that lays
/// text out rather than painting it.
pub mod type_scale {
    pub use interos_std::design::typography::BADGE_SCALE as BADGE;
    pub use interos_std::design::typography::CAPTION_SCALE as CAPTION;
    pub use interos_std::design::typography::BODY_SCALE as BODY;
    pub use interos_std::design::typography::TITLE_SCALE as TITLE;
    pub use interos_std::design::typography::DISPLAY_SCALE as DISPLAY;
}

/// The theme this app draws in.
///
    /// WHY this palette: the design the app showcases describe is
    /// monochrome -- a `#FAFAFA` page, a `#FFFFFF` surface, a `#E5E5E5`
    /// hairline, and colour reserved for a status -- and the platform's
    /// design system now holds that palette, so every application and the
    /// shell draw the same pixels.
pub const THEME: Theme = Theme {
    name: "messages",
    ..interos_std::design::monochrome::THEME
};

/// The smallest edge an interactive target may have.
pub const MIN_TOUCH_TARGET: usize = interos_std::design::touch::MIN_TARGET;

/// The messages's historical colour names, and the platform role each became.
///
/// WHY this is a mapping and not a palette: a name that resolves to a role
/// cannot drift from the platform, because there is no value here to drift. A
/// second set of colour constants would drift the moment the platform's
/// palette moved.
pub mod legacy {
    use super::THEME;

    /// The BG_DARK the app used to carry. Now [`Theme::surface`].
    pub const fn bg_dark() -> u32 {
        THEME.surface
    }

    /// The SURFACE_DARK the app used to carry. Now [`Theme::surface_container`].
    pub const fn surface_dark() -> u32 {
        THEME.surface_container
    }

    /// The SURFACE_RAISED the app used to carry. Now [`Theme::surface_container_high`].
    pub const fn surface_raised() -> u32 {
        THEME.surface_container_high
    }

    /// The BORDER_SUBTLE the app used to carry. Now [`Theme::border`].
    pub const fn border_subtle() -> u32 {
        THEME.border
    }

    /// The ACCENT_SKY the app used to carry. Now [`Theme::primary`].
    pub const fn accent_sky() -> u32 {
        THEME.primary
    }

    /// The TEXT_PRIMARY the app used to carry. Now [`Theme::on_surface`].
    pub const fn text_primary() -> u32 {
        THEME.on_surface
    }

    /// The TEXT_MUTED the app used to carry. Now [`Theme::on_surface_variant`].
    pub const fn text_muted() -> u32 {
        THEME.on_surface_variant
    }

    /// The DISABLED_FILL the app used to carry. Now [`Theme::surface_container_low`].
    pub const fn disabled_fill() -> u32 {
        THEME.surface_container_low
    }
}
