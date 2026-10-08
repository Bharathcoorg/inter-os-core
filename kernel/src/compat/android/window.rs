//! Android application window bridge.
//!
//! Android activities do not get a browser-owned surface. A verified Android
//! workload receives an ordinary InterOS compositor surface owned by its guest
//! task. The bridge keeps Android lifecycle state separate from native launcher
//! state while using the same KMS/window-manager path.

use alloc::collections::BTreeMap;
use crate::task::TaskId;

pub const ANDROID_SURFACE_WIDTH: u32 = 720;
pub const ANDROID_SURFACE_HEIGHT: u32 = 1320;
const MAX_ANDROID_WINDOWS: usize = 16;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AndroidActivityState {
    Created,
    Started,
    Resumed,
    Paused,
    Stopped,
    Destroyed,
}

#[derive(Debug, Clone, Copy)]
pub struct AndroidWindow {
    pub surface_id: u32,
    pub owner: TaskId,
    pub state: AndroidActivityState,
    pub focused: bool,
}

pub struct AndroidWindowManager {
    windows: BTreeMap<u32, AndroidWindow>,
}

impl AndroidWindowManager {
    pub const fn new() -> Self {
        Self { windows: BTreeMap::new() }
    }

    pub fn create(&mut self, owner: TaskId) -> Option<u32> {
        if self.windows.len() >= MAX_ANDROID_WINDOWS {
            return None;
        }
        let mut kms = crate::drivers::display_kms::DRM_KMS.lock();
        let surface = kms.create_surface(
            ANDROID_SURFACE_WIDTH,
            ANDROID_SURFACE_HEIGHT,
            20,
            owner.0,
        );
        if surface == 0 {
            return None;
        }
        self.windows.insert(surface, AndroidWindow {
            surface_id: surface,
            owner,
            state: AndroidActivityState::Created,
            focused: true,
        });
        Some(surface)
    }

    pub fn set_state(&mut self, surface_id: u32, state: AndroidActivityState) -> bool {
        let Some(window) = self.windows.get_mut(&surface_id) else { return false; };
        window.state = state;
        window.focused = matches!(state, AndroidActivityState::Resumed);
        true
    }

    pub fn focus(&mut self, surface_id: u32) -> bool {
        let Some(window) = self.windows.get_mut(&surface_id) else { return false; };
        window.focused = true;
        true
    }

    pub fn destroy(&mut self, surface_id: u32) -> bool {
        let Some(window) = self.windows.remove(&surface_id) else { return false; };
        let mut kms = crate::drivers::display_kms::DRM_KMS.lock();
        kms.destroy_surface_owned(window.surface_id, Some(window.owner.0))
    }

    pub fn destroy_owner(&mut self, owner: TaskId) -> usize {
        let ids: alloc::vec::Vec<u32> = self.windows.values()
            .filter(|w| w.owner == owner)
            .map(|w| w.surface_id)
            .collect();
        let mut removed = 0;
        for id in ids {
            if self.destroy(id) { removed += 1; }
        }
        removed
    }

    pub fn get(&self, surface_id: u32) -> Option<AndroidWindow> {
        self.windows.get(&surface_id).copied()
    }

    pub fn count(&self) -> usize {
        self.windows.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lifecycle_states_are_explicit() {
        assert_ne!(AndroidActivityState::Created, AndroidActivityState::Destroyed);
        assert_ne!(AndroidActivityState::Resumed, AndroidActivityState::Stopped);
    }
}
