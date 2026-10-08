//! Guest-owned boot animation for the canonical smartphone shell.
//!
//! This is deliberately rendered by the guest compositor. Device Studio never
//! substitutes a boot screen, so reboot evidence remains a real guest lifecycle.

use crate::{LauncherState, H, W};
use launcher::theme::Theme;

pub fn get_boot_screen_color(x: usize, y: usize, state: &LauncherState) -> u32 {
    let cx = (W / 2) as i32;
    let cy = (H / 2) as i32;
    let dx = x as i32 - cx;
    let dy = y as i32 - cy;
    let distance_sq = (dx * dx + dy * dy).max(0) as u32;
    // Integer square-root keeps the boot renderer freestanding and deterministic.
    let mut distance = 0u32;
    while distance.saturating_add(1).saturating_mul(distance.saturating_add(1)) <= distance_sq {
        distance += 1;
    }
    let distance = distance as usize;

    let progress = state.boot_progress as usize;
    let ring = 118usize;
    let thickness = 10usize;
    let angle = ((y as u32).wrapping_mul(257) ^ (x as u32).wrapping_mul(131)) % 360;
    let sweep = progress.saturating_mul(360) / 1000;
    let active = distance >= ring.saturating_sub(thickness)
        && distance <= ring + thickness
        && angle as usize <= sweep;

    let bg = Theme::DARK.bg;
    if active {
        Theme::DARK.accent
    } else if distance < 42 {
        Theme::DARK.surface_raised
    } else {
        bg
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn boot_progress_changes_rendered_state() {
        let mut state = LauncherState::new();
        state.boot_progress = 100;
        let a = get_boot_screen_color(478, 719, &state);
        state.boot_progress = 900;
        let b = get_boot_screen_color(478, 719, &state);
        assert_ne!(a, b);
    }
}
