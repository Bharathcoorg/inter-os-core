//! Guest-owned startup presentation rendered into the native portrait surface.
//! This is the post-userspace boot animation; bootloader/kernel output remains guest-owned.

use crate::{LauncherState, H, W};
use launcher::theme::Theme;

pub fn get_boot_screen_color(x: usize, y: usize, state: &LauncherState) -> u32 {
    let cx = W / 2;
    let cy = H / 2;
    let dx = x as i32 - cx as i32;
    let dy = y as i32 - cy as i32;
    let distance_sq = (dx * dx + dy * dy) as u32;
    let inner_sq = 42u32 * 42;
    let outer_sq = 58u32 * 58;
    let ring_sq = 118u32 * 118;
    let ring_inner_sq = 106u32 * 106;

    // Keep this per-pixel path O(1): render_band calls it for every pixel in the
    // 720x1440 surface, so an iterative square-root here caused visible boot stalls.
    if distance_sq >= inner_sq && distance_sq <= outer_sq {
        return Theme::DARK.accent;
    }
    if distance_sq >= ring_inner_sq && distance_sq <= ring_sq {
        // A rotating-looking segmented ring without floating point or trig in the
        // freestanding userspace hot path.
        let segment = ((x.wrapping_mul(3) + y.wrapping_mul(5)) / 7) % 32;
        let lit_segments = (state.boot_progress as usize * 32 / 1000).max(1);
        if segment < lit_segments {
            return Theme::DARK.accent;
        }
        return Theme::DARK.surface_raised;
    }

    // Deterministic progress indicator beneath the logo.
    let bar_width = W * 2 / 3;
    let bar_left = (W - bar_width) / 2;
    let bar_y = cy + 190;
    if y >= bar_y && y < bar_y + 8 && x >= bar_left && x < bar_left + bar_width {
        let filled = bar_width * state.boot_progress as usize / 1000;
        return if x - bar_left < filled {
            Theme::DARK.accent
        } else {
            Theme::DARK.surface_raised
        };
    }

    Theme::DARK.bg
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn boot_progress_changes_progress_indicator() {
        let mut state = LauncherState::new();
        let x = W / 3 + 1;
        let y = H / 2 + 190 + 2;
        state.boot_progress = 0;
        let empty = get_boot_screen_color(x, y, &state);
        state.boot_progress = 1000;
        let full = get_boot_screen_color(x, y, &state);
        assert_ne!(empty, full);
    }

    #[test]
    fn boot_renderer_has_bounded_dimensions() {
        let state = LauncherState::new();
        assert_eq!(get_boot_screen_color(0, 0, &state), Theme::DARK.bg);
    }
}
