//! Guest-owned restart and shutdown transition surface.
//! The transition is rendered by InterOS before the real power syscall is issued.

use crate::{LauncherState, H, W};
use launcher::theme::Theme;

pub fn get_power_transition_color(x: usize, y: usize, state: &LauncherState) -> u32 {
    let progress = state.power_transition_progress.min(1000) as usize;
    let cx = W / 2;
    let cy = H / 2;
    let dx = x as i32 - cx as i32;
    let dy = y as i32 - cy as i32;
    let d2 = (dx * dx + dy * dy) as usize;
    let ring_outer = 110usize * 110;
    let ring_inner = 96usize * 96;
    let bg = Theme::DARK.surface;
    if d2 >= ring_inner && d2 <= ring_outer {
        let segment = ((x * 3 + y * 5) / 7) % 32;
        let lit = progress * 32 / 1000;
        return if segment < lit { Theme::DARK.primary } else { Theme::DARK.surface_container_high };
    }
    let logo_outer = 48usize * 48;
    let logo_inner = 32usize * 32;
    if d2 >= logo_inner && d2 <= logo_outer {
        let intensity = 1000usize.saturating_sub(progress);
        return blend(bg, Theme::DARK.primary, intensity);
    }
    let line_y = cy + 180;
    if y >= line_y && y < line_y + 8 {
        let width = W * 2 / 3;
        let left = (W - width) / 2;
        if x >= left && x < left + width {
            return if x - left < width * progress / 1000 { Theme::DARK.primary } else { Theme::DARK.surface_container_high };
        }
    }
    bg
}

fn blend(a: u32, b: u32, amount: usize) -> u32 {
    let amount = amount.min(1000) as u32;
    let mut out = 0u32;
    for shift in [0, 8, 16] {
        let av = (a >> shift) & 0xff;
        let bv = (b >> shift) & 0xff;
        let value = (av * (1000 - amount) + bv * amount) / 1000;
        out |= value << shift;
    }
    0xff00_0000 | out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transition_progress_changes_guest_rendered_pixels() {
        let mut state = LauncherState::new();
        state.power_transition_progress = 0;
        let start = get_power_transition_color(W / 2, H / 2 + 180, &state);
        state.power_transition_progress = 1000;
        let end = get_power_transition_color(W / 2, H / 2 + 180, &state);
        assert_ne!(start, end);
    }
}
