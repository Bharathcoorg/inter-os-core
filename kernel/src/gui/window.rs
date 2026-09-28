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
        let id = WindowId(self.next_id);
        self.create_with_id(id, owner, rect, kind, modal)?;
        self.next_id = self.next_id.checked_add(1).filter(|v| *v != 0).unwrap_or(1);
        Ok(id)
    }

    /// Registers a display-backed window using the KMS surface ID as its stable
    /// cross-layer identity. This keeps the GUI and display registries coherent.
    pub fn create_with_id(&mut self, id: WindowId, owner: u64, rect: WindowRect, kind: WindowKind, modal: bool) -> Result<(), &'static str> {
        if id.0 == 0 || rect.width == 0 || rect.height == 0 { return Err("invalid window identity or bounds"); }
        if self.windows.len() >= 128 { return Err("window limit reached"); }
        self.windows.retain(|w| w.id != id);
        let z = self.windows.iter().map(|w| w.z).max().unwrap_or(0).saturating_add(1);
        self.windows.push(Window { id, owner, rect, kind, visible: true, modal, z });
        if kind != WindowKind::SystemOverlay {
            self.focused = Some(id);
        }
        if id.0 >= self.next_id {
            self.next_id = id.0.checked_add(1).filter(|v| *v != 0).unwrap_or(1);
        }
        Ok(())
    }

    pub fn set_geometry_owned(&mut self, id: WindowId, owner: u64, rect: WindowRect) -> bool {
        if rect.width == 0 || rect.height == 0 { return false; }
        if let Some(window) = self.windows.iter_mut().find(|w| w.id == id && w.owner == owner) {
            window.rect = rect;
            true
        } else {
            false
        }
    }

    pub fn destroy_owned(&mut self, id: WindowId, owner: u64) -> bool {
        self.destroy_if(id, Some(owner))
    }

    /// Removes a window by identity for trusted kernel/display lifecycle cleanup.
    pub fn destroy_if(&mut self, id: WindowId, owner: Option<u64>) -> bool {
        let Some(pos) = self.windows.iter().position(|w| w.id == id && owner.map(|v| w.owner == v).unwrap_or(true)) else { return false; };
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
        let Some(pos) = self.windows.iter().position(|w| w.id == id && w.owner == owner && w.visible && w.kind != WindowKind::SystemOverlay) else { return false; };
        let next_z = self.windows.iter().map(|w| w.z).max().unwrap_or(0).saturating_add(1);
        self.windows[pos].z = next_z;
        self.focused = Some(id);
        true
    }

    pub fn hit_test(&mut self, x: u32, y: u32) -> Option<(WindowId, u64)> {
        let target = self.windows.iter()
            .filter(|w| w.visible && w.kind != WindowKind::SystemOverlay && w.rect.contains(x, y))
            .filter(|w| w.modal || !self.windows.iter().any(|m| m.visible && m.modal && m.z > w.z))
            .max_by_key(|w| w.z)
            .map(|w| (w.id, w.owner));
        if let Some((id, _)) = target { self.focused = Some(id); }
        target
    }

    pub fn focused(&self) -> Option<WindowId> { self.focused }

    pub fn focused_owner(&self) -> Option<u64> {
        let id = self.focused?;
        self.windows.iter().find(|w| w.id == id && w.visible && w.kind != WindowKind::SystemOverlay).map(|w| w.owner)
    }

    pub fn top_visible(&self) -> Option<WindowId> {
        self.windows.iter().filter(|w| w.visible && w.kind != WindowKind::SystemOverlay).max_by_key(|w| w.z).map(|w| w.id)
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

    #[test]
    fn system_overlay_passes_through_input() {
        let mut wm = WindowManager::new();
        let app = wm.create(11, WindowRect{x:0, y:0, width:720, height:1440}, WindowKind::Application, false).unwrap();
        let _overlay = wm.create(10, WindowRect{x:0, y:0, width:720, height:96}, WindowKind::SystemOverlay, false).unwrap();
        assert_eq!(wm.focused(), Some(app));
        assert_eq!(wm.focused_owner(), Some(11));
        assert_eq!(wm.hit_test(670, 25), Some((app, 11)));
    }
}
