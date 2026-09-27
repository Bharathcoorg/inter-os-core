//! InterOS-native window management.
//!
//! Owns application window focus, stacking, visibility, modal priority, and
//! input target selection. Rendering remains the compositor's responsibility.

use alloc::vec::Vec;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WindowKind { Application, Dialog, SystemOverlay, Fullscreen }

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WindowId(pub u32);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WindowRect {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

impl WindowRect {
    pub const fn contains(self, x: u32, y: u32) -> bool {
        let px = x as i32;
        let py = y as i32;
        px >= self.x && py >= self.y &&
            px < self.x.saturating_add(self.width as i32) &&
            py < self.y.saturating_add(self.height as i32)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Window {
    pub id: WindowId,
    pub owner: u64,
    pub rect: WindowRect,
    pub kind: WindowKind,
    pub visible: bool,
    pub modal: bool,
    pub z: u32,
}

pub struct WindowManager {
    windows: Vec<Window>,
    next_id: u32,
    focused: Option<WindowId>,
}

impl WindowManager {
    pub const fn new() -> Self {
        Self { windows: Vec::new(), next_id: 1, focused: None }
    }

    pub fn create(&mut self, owner: u64, rect: WindowRect, kind: WindowKind, modal: bool) -> Result<WindowId, &'static str> {
        if rect.width == 0 || rect.height == 0 { return Err("window has empty bounds"); }
        if self.windows.len() >= 128 { return Err("window limit reached"); }
        let id = WindowId(self.next_id);
        self.next_id = self.next_id.wrapping_add(1).max(1);
        let z = self.windows.iter().map(|w| w.z).max().unwrap_or(0).saturating_add(1);
        self.windows.push(Window { id, owner, rect, kind, visible: true, modal, z });
        self.focused = Some(id);
        Ok(id)
    }

    pub fn destroy_owned(&mut self, id: WindowId, owner: u64) -> bool {
        let Some(pos) = self.windows.iter().position(|w| w.id == id && w.owner == owner) else { return false; };
        self.windows.remove(pos);
        if self.focused == Some(id) { self.focused = self.top_visible(); }
        true
    }

    pub fn set_visible_owned(&mut self, id: WindowId, owner: u64, visible: bool) -> bool {
        let Some(pos) = self.windows.iter().position(|w| w.id == id && w.owner == owner) else { return false; };
        self.windows[pos].visible = visible;
        if !visible && self.focused == Some(id) { self.focused = self.top_visible(); }
        true
    }

    pub fn focus_owned(&mut self, id: WindowId, owner: u64) -> bool {
        let Some(pos) = self.windows.iter().position(|w| w.id == id && w.owner == owner && w.visible) else { return false; };
        let next_z = self.windows.iter().map(|w| w.z).max().unwrap_or(0).saturating_add(1);
        self.windows[pos].z = next_z;
        self.focused = Some(id);
        true
    }

    pub fn hit_test(&mut self, x: u32, y: u32) -> Option<(WindowId, u64)> {
        let target = self.windows.iter()
            .filter(|w| w.visible && w.rect.contains(x, y))
            .filter(|w| w.modal || !self.windows.iter().any(|m| m.visible && m.modal && m.z > w.z))
            .max_by_key(|w| w.z)
            .map(|w| (w.id, w.owner));
        if let Some((id, _)) = target { self.focused = Some(id); }
        target
    }

    pub fn focused(&self) -> Option<WindowId> { self.focused }

    pub fn top_visible(&self) -> Option<WindowId> {
        self.windows.iter().filter(|w| w.visible).max_by_key(|w| w.z).map(|w| w.id)
    }

    pub fn snapshot(&self, out: &mut [Window]) -> usize {
        let n = out.len().min(self.windows.len());
        out[..n].copy_from_slice(&self.windows[..n]);
        n
    }

    pub fn validate(&self) -> bool {
        self.windows.iter().all(|w| w.z > 0 && w.rect.width > 0 && w.rect.height > 0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn modal_blocks_background_input() {
        let mut wm = WindowManager::new();
        let app = wm.create(1, WindowRect{x:0,y:0,width:100,height:100}, WindowKind::Application, false).unwrap();
        let _dialog = wm.create(2, WindowRect{x:10,y:10,width:40,height:40}, WindowKind::Dialog, true).unwrap();
        assert_eq!(wm.hit_test(5, 5), None);
        assert_eq!(wm.hit_test(20, 20).map(|v| v.0), Some(WindowId(2)));
        assert!(wm.focus_owned(app, 1));
    }
}
