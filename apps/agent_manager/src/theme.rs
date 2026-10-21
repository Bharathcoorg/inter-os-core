//! The agent manager's own theme.
//!
//! WHY the app has a theme of its own: the agent manager is a
//! supervisor, and a supervisor is read rather than used. The platform's
//! neutral theme is a dark instrument panel, which is the right choice
//! for a shell and the wrong one for a screen whose job is to report
//! what a device holds in a list a person reads.
//!
//! The tokens are the app's own:
//!
//! | role | value |
//! |---|---|
//! | background | `#FAFAFA` |
//! | surface | `#FFFFFF` |
//! | primary text | `#0A0A0A` |
//! | secondary text | `#6B6B6B` |
//! | border | `#E5E5E5` |
//! | success | `#22C55E` |
//! | warning | `#F59E0B` |
//! | error | `#EF4444` |
//!
//! WHY colour is scarce: the app's job is to report a device's state,
//! and a screen where everything is coloured reports nothing. Colour
//! appears in exactly three places: a status a person has to be able to
//! see, the identity of the model that is selected, and the primary
//! action. Every other surface is black, white or grey.

use interos_std::design::theme::{Radii, Spacing, Theme, Touch, TypeScale};

/// The agent manager's theme.
pub const THEME: Theme = Theme {
    name: "agent-manager",
    dark: false,

    // The page and the surfaces on it.
    surface: 0xFFFAFAFA,
    surface_container: 0xFFFFFFFF,
    surface_container_high: 0xFFFFFFFF,
    surface_container_low: 0xFFF4F4F4,
    surface_frosted: 0xFFFFFFFF,
    overlay: 0xFFFFFFFF,
    overlay_border: 0xFFE5E5E5,

    // The ink.
    on_surface: 0xFF0A0A0A,
    on_surface_variant: 0xFF6B6B6B,
    muted: 0xFFA3A3A3,
    on_primary: 0xFFFFFFFF,
    on_primary_container: 0xFF0A0A0A,
    ink_on_light: 0xFF0A0A0A,
    ink_on_dark: 0xFFFFFFFF,

    // The hairlines. WHY there are two: the outline is the edge of a
    // control and the variant is the edge of a row, and a list whose
    // every divider is the same weight as its own border reads as a
    // grid of boxes.
    outline: 0xFF0A0A0A,
    outline_variant: 0xFFEFEFEF,
    border: 0xFFE5E5E5,

    // The primary is black, because the app's one filled control is the
    // one a person presses, and a screen with a coloured button on it
    // reads as a marketing page.
    primary: 0xFF0A0A0A,
    primary_container: 0xFFEFEFEF,
    secondary: 0xFF6B6B6B,
    accent: 0xFF0A0A0A,

    // The three status colours, and only these three.
    error: 0xFFEF4444,
    error_container: 0xFFFDECEC,
    success: 0xFF22C55E,
    success_container: 0xFFEAF9EF,
    on_success_container: 0xFF15803D,
    warning: 0xFFF59E0B,
    warning_container: 0xFFFEF6E7,
    emergency: 0xFFEF4444,

    // The app draws no wallpaper of its own, but the roles have to
    // answer, and an app that inherits the platform's wallpaper paints
    // a picture behind a list of measurements.
    wallpaper_top: 0xFFFAFAFA,
    wallpaper_mid: 0xFFF4F4F4,
    wallpaper_bottom: 0xFFEDEDED,
    wallpaper_ink: 0xFF0A0A0A,

    type_scale: TypeScale::STANDARD,
    spacing: Spacing::STANDARD,
    radii: Radii::STANDARD,
    touch: Touch::STANDARD,
};
