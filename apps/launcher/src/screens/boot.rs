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
        // A travelling comet, not a filling arc.
        //
        // WHY motion lives here and progress lives in the bar below: the old
        // ring revealed static diagonal stripes as progress advanced, so it sat
        // perfectly still whenever progress did - which is most of the time, and
        // is exactly why the boot looked frozen. A phone's boot mark says "I'm
        // working" continuously; the bar below says "how far along". One element
        // per job, so neither lies about the other's.
        //
        // WHY integer stripes and not trig: an iterative square-root in this
        // per-pixel path caused visible boot stalls before, and floats do not
        // exist in this freestanding target. The stripe hash is O(1); the head
        // advances one segment every two frames for a full revolution per
        // ~64 ticks, phone-like rather than frantic.
        let segment = ((x.wrapping_mul(3) + y.wrapping_mul(5)) / 7) % 32;
        let head = ((state.boot_frame / 2) % 32) as usize;
        // Distance behind the head, modulo the ring: 0 at the head, growing
        // backward along the travel direction through the 10-segment tail.
        let behind = (head + 32 - segment) % 32;
        if behind < 10 {
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

    /// The ring comet travels: the same pixel is dark on one frame and lit 32
    /// frames later.
    ///
    /// WHY this test exists: the previous ring revealed static stripes as
    /// progress advanced, so it sat still whenever progress did. A regression
    /// to a progress-driven ring would pass the progress test above and still
    /// freeze on device. This pins motion to `boot_frame` instead.
    ///
    /// WHY (472, 720): 112px east of the (360, 720) centre, inside the
    /// 106..118 ring band. Its stripe hash is segment 12; at frame 0 the head
    /// is at 0 (20 behind - dark) and at frame 32 the head is at 16 (4 behind -
    /// lit). If the stripe hash or the head rate ever changes, pick any ring
    /// pixel whose `(head + 32 - segment) % 32` crosses 10 between two frames.
    #[test]
    fn boot_comet_moves_with_frame_counter() {
        let mut state = LauncherState::new();
        let (x, y) = (W / 2 + 112, H / 2);
        state.boot_frame = 0;
        let dark = get_boot_screen_color(x, y, &state);
        state.boot_frame = 32;
        let lit = get_boot_screen_color(x, y, &state);
        assert_eq!(dark, Theme::DARK.surface_raised);
        assert_eq!(lit, Theme::DARK.accent);
        assert_ne!(dark, lit);
    }
}
