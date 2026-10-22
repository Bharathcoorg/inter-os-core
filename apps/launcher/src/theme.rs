//! The launcher's view of the platform's design system.
//!
//! WHY this module still exists: it used to be the launcher's own palette, its
//! own radius scale, its own spacing scale, its own type scale and its own
//! touch minimum, and the screens reached for all of them. That was a second
//! design system inside one app: the SDK's `radius::CONTROL` was 12 and this
//! module's was 18, the SDK's minimum touch target was 44 and this module's was
//! 48, and a third-party app linking the SDK had no way to draw a control that
//! matched the shell's.
//!
//! It is now an adapter and nothing else. Every value below comes from the SDK,
//! so the shell draws the platform's controls and a third-party app that asks
//! the SDK for the same roles gets the same pixels.
//!
//! WHY the legacy names are still here, as a mapping: the launcher used to
//! carry its own vocabulary for the same colours. [`legacy`] is the record of
//! which platform role each old name became, so a reader who remembers the old
//! palette can find the new one instead of wondering where `text_muted` went.

/// The platform's corner-radius scale.
///
/// WHY this is a re-export and not a copy: a copy is a second scale, and a
/// second scale is how a control ends up with visibly different corners from
/// its siblings.
pub use interos_std::design::radius;

/// The platform's spacing scale, under the shell's historical names.
pub use interos_std::design::spacing as space;

/// The platform's theme: the roles an interface asks for.
pub use interos_std::design::theme::Theme;

/// The platform's type scale, under the names the screens already use.
///
/// WHY the multipliers and not the styles: a screen asks the rasteriser for a
/// size, and the style's line box is what the layout reserves. The scale
/// constants are the size; the styles live on the theme for an app that lays
/// text out rather than painting it.
pub mod type_scale {
    pub use interos_std::design::typography::BADGE_SCALE as BADGE;
    pub use interos_std::design::typography::CAPTION_SCALE as CAPTION;
    pub use interos_std::design::typography::BODY_SCALE as BODY;
    pub use interos_std::design::typography::TITLE_SCALE as TITLE;
    pub use interos_std::design::typography::DISPLAY_SCALE as DISPLAY;
}

/// The launcher's historical colour names, and the platform role each became.
///
/// WHY this is a mapping and not a palette: a name that resolves to a role
/// cannot drift from the platform, because there is no value here to drift.
/// A second set of colour constants would drift the moment the platform's
/// palette moved.
pub mod legacy {
    use super::Theme;

    /// The page background behind everything. Now [`Theme::surface`].
    pub const fn bg(theme: &Theme) -> u32 {
        theme.surface
    }

    /// A resting card. Now [`Theme::surface_container`].
    pub const fn surface(theme: &Theme) -> u32 {
        theme.surface_container
    }

    /// A raised card or a pressed control. Now [`Theme::surface_container_high`].
    pub const fn surface_raised(theme: &Theme) -> u32 {
        theme.surface_container_high
    }

    /// A recessed fill. Now [`Theme::surface_container_low`].
    pub const fn surface_sunken(theme: &Theme) -> u32 {
        theme.surface_container_low
    }

    /// Body and heading ink. Now [`Theme::on_surface`].
    pub const fn text(theme: &Theme) -> u32 {
        theme.on_surface
    }

    /// Secondary ink. Now [`Theme::on_surface_variant`].
    pub const fn text_muted(theme: &Theme) -> u32 {
        theme.on_surface_variant
    }

    /// Ink on a brand fill. Now [`Theme::on_primary`].
    pub const fn text_on_accent(theme: &Theme) -> u32 {
        theme.on_primary
    }

    /// The brand colour. Now [`Theme::primary`].
    pub const fn accent(theme: &Theme) -> u32 {
        theme.primary
    }

    /// A failure or a destructive action. Now [`Theme::error`].
    pub const fn danger(theme: &Theme) -> u32 {
        theme.error
    }

    /// The hairline a card is outlined with. Now [`Theme::border`].
    pub const fn border(theme: &Theme) -> u32 {
        theme.border
    }
}

/// The visual language the shell draws: the platform's stock monochrome
/// system.
///
/// WHY re-exports and not copies: the shell must be the *first* thing on
/// the device that draws the stock language -- a shell with its own palette
/// is how the device ends up showing two companies' products. The light
/// binding is for the pages, the dark one for the surfaces that are dark on
/// purpose: the boot splash, the power menu, the control centre and the pin
/// keypad.
pub use interos_std::design::monochrome::THEME as SHELL;
pub use interos_std::design::monochrome::THEME_DARK as SHELL_DARK;

/// The logical surface width.
pub const SCREEN_WIDTH: usize = interos_std::design::viewport::LOGICAL_WIDTH;

/// The logical surface height.
pub const SCREEN_HEIGHT: usize = interos_std::design::viewport::LOGICAL_HEIGHT;

/// The smallest edge an interactive target may have.
///
/// WHY this is the SDK's minimum and not the shell's own: the platform's
/// minimum is the floor every app is held to, and a shell that held itself to a
/// different floor was a shell whose controls a third-party app could not
/// match.
pub const MIN_TOUCH_TARGET: usize = interos_std::design::touch::MIN_TARGET;

/// The status bar's height.
pub const STATUS_BAR_HEIGHT: usize = interos_std::design::viewport::STATUS_BAR_HEIGHT;

/// The navigation bar's height.
pub const NAV_BAR_HEIGHT: usize = interos_std::design::viewport::NAV_BAR_HEIGHT;

/// The shell's content inset.
///
/// WHY it is the platform's grid rather than the platform's default content
/// padding: the shell's surfaces are 720 wide and its cards carry a label and
/// a value, and the default inset crowds them. The value is still a step on
/// the platform's spacing scale, so it is a choice from the scale and not a
/// second scale.
pub const CONTENT_PADDING: usize = interos_std::design::spacing::GRID_40;
