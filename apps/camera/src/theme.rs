// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

//! The camera's view of the platform's design system.
//!
//! WHY this module exists: the app used to carry its own palette, its own
//! radius scale, its own spacing scale, its own type scale and its own touch
//! minimum, and the renderer reached for all of them. That is a second design
//! system inside one app: a third-party app linking the SDK had no way to
//! draw a camera that matched this one.
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
    pub use interos_std::design::typography::SECTION_SCALE as SECTION;
    pub use interos_std::design::typography::HEADING_SCALE as HEADING;
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
    name: "camera",
    ..interos_std::design::monochrome::THEME
};

/// The theme a control that stands on the chassis is cut from: the dark half
/// of the platform's monochrome ladder.
///
/// WHY not [`THEME`], which the rest of the fleet hands the platform's back
/// control: the chip takes its ring, its plate and its chevron from the theme
/// it is given, and the light roles put a `#0A0A0A` outline on this app's
/// `#0E0F11` band -- a measured 1.03:1 step, so the control's own boundary is
/// one hundredth of a step off the surface it stands on and the button is not
/// there. The dark variant trades paper for ink: ring and chevron carry
/// `#FAFAFA`, 18.37:1 against the band and 15.06:1 against the chip's own
/// `#232323` plate, against the 3:1 WCAG 1.4.11 floor for a non-text control.
///
/// WHY the plate is allowed to stay quiet: `#232323` over `#0E0F11` is 1.22:1,
/// under the 1.3:1 step a divider must earn, because the plate is not the
/// divider here -- the ring is, and the ring clears the floor by more than
/// five times. The same reasoning and the same arithmetic as
/// `apps/webview_demo`, whose band is dark for the same reason.
pub const CHASSIS_THEME: Theme = interos_std::design::monochrome::THEME_DARK;

/// The camera's own chassis palette.
///
/// WHY this sits beside [`THEME`] instead of replacing it: `THEME` is the
/// platform's surface-and-ink language, and it stays the source of every
/// text and surface role that must read as a platform sheet. A viewfinder
/// app is not a sheet, though -- it is an instrument with a dark body -- so
/// the body, the well and the ink that sits on them come from here, while
/// the *meaning* of a colour still comes from the platform: the amber is
/// the platform's caution role, the refusal red is its error role, and the
/// ready emerald is its success role. Nothing here forks a [`Theme`]; these
/// are the values the camera's painter needs and nothing else.
pub mod chrome {
    use super::Theme;

    /// The chassis: the top band, the mode strip and the shutter dock.
    pub const BODY: u32 = 0xFF0E0F11;
    /// The viewfinder's active area, one step above the letterbox.
    pub const WELL: u32 = 0xFF111417;
    /// The bars that mask an aspect ratio narrower than the well.
    pub const LETTERBOX: u32 = 0xFF07080A;
    /// A floating control: a top-row pill, the status card, the sheet.
    pub const SURFACE: u32 = 0xFF17191C;
    /// A raised control: a settings row, a disc that carries a glyph.
    pub const SURFACE_HIGH: u32 = 0xFF202328;
    /// A hairline on the chassis.
    pub const BORDER: u32 = 0xFF2C3036;
    /// A hairline that has to be seen: the well's edge, the sheet's edge.
    pub const BORDER_STRONG: u32 = 0xFF3D424A;
    /// A control that is present but cannot act.
    pub const DISABLED: u32 = 0xFF4A4F57;

    /// Body ink on the chassis.
    pub const TEXT: u32 = 0xFFF5F6F7;
    /// Secondary ink: a reason, an unselected mode, a note.
    pub const TEXT_MUTED: u32 = 0xFF9AA0A6;
    /// The least prominent ink: the system gesture bar.
    pub const TEXT_DIM: u32 = 0xFF6C7178;

    /// The shutter and the selected mode. The platform's caution role is
    /// the amber the design system already names, and it reads on charcoal.
    pub const AMBER: u32 = super::THEME.warning;
    /// An informational control: flash, filter, timer, aspect, grid.
    pub const SKY: u32 = interos_std::design::color::DARK_ACCENT;
    /// The platform refused the camera: the status card's dot and headline.
    pub const REFUSED: u32 = super::THEME.error;
    /// The platform granted authority and the camera is armed.
    pub const READY: u32 = super::THEME.success;
    /// Authorised but not usable: no sensor is bound.
    pub const CAUTION: u32 = super::THEME.warning;
    /// The video mode's record core.
    pub const RECORD: u32 = super::THEME.error;

    /// A low-strength wash of `ink` over the chassis surface.
    ///
    /// WHY this is a helper: the selected mode's fill, the status dot's halo
    /// and the refused card's hairline are all the same decision -- a colour
    /// said quietly on `SURFACE` -- and three copies of the blend would let
    /// the three drift apart.
    pub fn tint(ink: u32, percent: u32) -> u32 {
        Theme::blend(SURFACE, ink, (percent.min(100) * 256) / 100)
    }
}

/// The smallest edge an interactive target may have.
pub const MIN_TOUCH_TARGET: usize = interos_std::design::touch::MIN_TARGET;

/// The camera's historical colour names, and the platform role each became.
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

    /// The SURFACE_RAISED the app used to carry. Now [`Theme::surface_container`].
    pub const fn surface_raised() -> u32 {
        THEME.surface_container
    }

    /// The BORDER_SUBTLE the app used to carry. Now [`Theme::border`].
    pub const fn border_subtle() -> u32 {
        THEME.border
    }

    /// The ACCENT_AMBER the app used to carry. Now [`Theme::warning`].
    pub const fn accent_amber() -> u32 {
        THEME.warning
    }

    /// The ACCENT_SKY the app used to carry. Now [`Theme::primary`].
    pub const fn accent_sky() -> u32 {
        THEME.primary
    }

    /// The SUCCESS_EMERALD the app used to carry. Now [`Theme::success`].
    pub const fn success_emerald() -> u32 {
        THEME.success
    }

    /// The DANGER_CRIMSON the app used to carry. Now [`Theme::error`].
    pub const fn danger_crimson() -> u32 {
        THEME.error
    }

    /// The TEXT_PRIMARY the app used to carry. Now [`Theme::on_surface`].
    pub const fn text_primary() -> u32 {
        THEME.on_surface
    }

    /// The TEXT_MUTED the app used to carry. Now [`Theme::on_surface_variant`].
    pub const fn text_muted() -> u32 {
        THEME.on_surface_variant
    }
}
