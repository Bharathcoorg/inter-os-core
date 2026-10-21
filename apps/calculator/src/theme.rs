//! The calculator's view of the platform's design system.
//!
//! WHY this module exists: the app used to carry its own palette, its own
//! radius scale, its own spacing scale, its own type scale and its own touch
//! minimum, and the renderer reached for all of them. That is a second design
//! system inside one app: a third-party app linking the SDK had no way to
//! draw a calculator that matched this one.
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
/// WHY the LIGHT variant: the platform's design document gives the
/// calculator a warm ivory background, and the light theme's surface is
/// that ivory (`#F7F6F2`). The header of this module used to call the
/// app a DARK variant while the app drew a surface the document did not
/// ask for, and a document and a drawing that disagree are two
/// descriptions of the same screen.
///
/// WHY the roles still do the work: the calculator's keys are painted from
/// the roles below, so the switch changed the surface and the ink on it
/// and nothing else. A third-party app that asks the SDK for the same
/// roles draws the same calculator.
pub const THEME: Theme = Theme::LIGHT;

/// The smallest edge an interactive target may have.
pub const MIN_TOUCH_TARGET: usize = interos_std::design::touch::MIN_TARGET;

/// The calculator's historical colour names, and the platform role each became.
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

    /// The TEXT_PRIMARY the app used to carry. Now [`Theme::on_surface`].
    pub const fn text_primary() -> u32 {
        THEME.on_surface
    }

    /// The TEXT_MUTED the app used to carry. Now [`Theme::on_surface_variant`].
    pub const fn text_muted() -> u32 {
        THEME.on_surface_variant
    }

    /// The ACCENT_AMBER the app used to carry. Now [`Theme::warning`].
    pub const fn accent_amber() -> u32 {
        THEME.warning
    }

    /// The ACCENT_AMBER_BORDER the app used to carry. Now [`Theme::warning_container`].
    pub const fn accent_amber_border() -> u32 {
        THEME.warning_container
    }

    /// The ERROR_RED the app used to carry. Now [`Theme::error`].
    pub const fn error_red() -> u32 {
        THEME.error
    }

    /// The BTN_DIGIT_BG the app used to carry. Now [`Theme::surface_container`].
    pub const fn btn_digit_bg() -> u32 {
        THEME.surface_container
    }

    /// The BTN_DIGIT_BORDER the app used to carry. Now [`Theme::border`].
    pub const fn btn_digit_border() -> u32 {
        THEME.border
    }

    /// The BTN_FN_BG the app used to carry. Now [`Theme::surface_container_high`].
    pub const fn btn_fn_bg() -> u32 {
        THEME.surface_container_high
    }

    /// The BTN_FN_BORDER the app used to carry. Now [`Theme::border`].
    pub const fn btn_fn_border() -> u32 {
        THEME.border
    }

    /// The BTN_FN_TEXT the app used to carry. Now [`Theme::on_surface_variant`].
    pub const fn btn_fn_text() -> u32 {
        THEME.on_surface_variant
    }

    /// The BTN_OP_BG the app used to carry. Now [`Theme::warning_container`].
    pub const fn btn_op_bg() -> u32 {
        THEME.warning_container
    }

    /// The BTN_OP_BORDER the app used to carry. Now [`Theme::warning_container`].
    pub const fn btn_op_border() -> u32 {
        THEME.warning_container
    }

    /// The BTN_OP_TEXT the app used to carry. Now [`Theme::warning`].
    pub const fn btn_op_text() -> u32 {
        THEME.warning
    }

    /// The BTN_CLEAR_BG the app used to carry. Now [`Theme::error_container`].
    pub const fn btn_clear_bg() -> u32 {
        THEME.error_container
    }

    /// The BTN_CLEAR_BORDER the app used to carry. Now [`Theme::error_container`].
    pub const fn btn_clear_border() -> u32 {
        THEME.error_container
    }

    /// The BTN_CLEAR_TEXT the app used to carry. Now [`Theme::error`].
    pub const fn btn_clear_text() -> u32 {
        THEME.error
    }
}
