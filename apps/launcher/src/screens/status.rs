//! The shell's one status bar.
//!
//! WHY this module exists: every screen used to draw its own bar, and they
//! disagreed -- the clock moved between x 24, 32, 40 and 48 and between
//! BODY and CAPTION, some screens drew no hardware glyphs at all, and the
//! task switcher drew a filled battery and four solid signal bars that no
//! sensor had measured. A person reading the bar cannot tell which screen
//! they are on from it, and a bar that changes shape is a bar that is not
//! chrome.
//!
//! This module therefore owns the canonical clock position, the honest
//! telemetry glyphs (outlined, never filled) and the bottom hairline. A
//! screen draws its clock and then asks this module for the rest.
//!
//! Platform Truth Contract: the cellular radio, Wi-Fi and fuel gauge are
//! unbound, so their glyphs carry no level. The clock is drawn only when the
//! RTC answered; there is no fallback time.

use launcher::text::Label;
use launcher::theme::{type_scale, Theme, STATUS_BAR_HEIGHT};

/// Where the status clock sits on every screen.
pub const CLOCK_X: usize = 40;
/// The clock's baseline box origin.
pub const CLOCK_Y: usize = 8;
/// The clock's type rung.
pub const CLOCK_SCALE: usize = type_scale::BODY;

/// The status bar's hardware glyphs.
pub fn telemetry_pixel(x: usize, y: usize, theme: &Theme) -> Option<u32> {
    let t = crate::screens::home::telemetry_plan();
    crate::screens::home::telemetry_pixel(x, y, &t, theme.on_surface)
}

/// The status clock, when the RTC supplied one.
pub fn clock_pixel(x: usize, y: usize, clock: Option<&Label>, theme: &Theme) -> Option<u32> {
    clock.and_then(|run| run.pixel(x, y, theme.surface))
}

/// The bar's background: the page colour, with its one-pixel divider at the
/// bottom edge.
pub fn backdrop_pixel(x: usize, y: usize, theme: &Theme) -> u32 {
    let _ = x;
    if y == STATUS_BAR_HEIGHT - 1 {
        theme.border
    } else {
        theme.surface
    }
}

/// One pixel of the canonical status bar.
pub fn pixel(x: usize, y: usize, clock: Option<&Label>, theme: &Theme) -> u32 {
    if let Some(c) = clock_pixel(x, y, clock, theme) {
        return c;
    }
    if let Some(c) = telemetry_pixel(x, y, theme) {
        return c;
    }
    backdrop_pixel(x, y, theme)
}

/// One pixel of the canonical status bar for a screen whose clock run has its
/// own painter (the shell has more than one run type, and the bar must not
/// care which one a screen keeps).
pub fn pixel_with(
    x: usize,
    y: usize,
    clock: impl Fn(usize, usize) -> Option<u32>,
    theme: &Theme,
) -> u32 {
    if let Some(c) = clock(x, y) {
        return c;
    }
    if let Some(c) = telemetry_pixel(x, y, theme) {
        return c;
    }
    backdrop_pixel(x, y, theme)
}
