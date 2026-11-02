//! The header row's partition: the platform's back control, and the runs that
//! have to clear it.
//!
//! WHY this is its own module and not two literals in the renderer: the painter
//! and the tap handler have to agree on which pixels the control owns, and the
//! device showed what happens when they do not -- Notes painted no control at
//! the canonical rect, so a tap at (48, 80) fell through to the search pill,
//! answered nothing and left the person on a screen with no way back. One
//! statement of the partition is also the only version of it a host test can
//! reach: the renderer lives in the binary, which is built without a test
//! harness.

use crate::theme::{spacing, THEME};
use interos_std::design::back_button;

/// The left edge a header run may start at: one grid step clear of the control.
///
/// WHY it is derived from the control's own geometry: the rect belongs to the
/// SDK, and a run authored against a hand-copied number slides under the chip
/// the moment the platform moves it.
pub const LABEL_X: usize = back_button::X + back_button::SIZE + spacing::GRID_16;

/// The control's pixel, in the theme this app draws in.
#[inline]
pub fn back_pixel(x: usize, y: usize) -> Option<u32> {
    back_button::pixel_for_theme(x, y, &THEME)
}

/// Whether the control claims the tap -- answered before any row that shares
/// its rect.
#[inline]
pub fn back_claims(x: usize, y: usize) -> bool {
    back_button::contains(x, y)
}
