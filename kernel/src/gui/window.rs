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
    last_reject: Option<&'static str>,
}

impl WindowManager {
    pub const fn new() -> Self {
        Self { windows: Vec::new(), next_id: 1, focused: None, last_reject: None }
    }

    pub fn create(&mut self, owner: u64, rect: WindowRect, kind: WindowKind, modal: bool) -> Result<WindowId, &'static str> {
        let id = WindowId(self.next_id);
        // WHY there is no counter bump here: `create_with_id` already advances
        // `next_id` past the identity it just registered, because it is also the
        // entry point for KMS surface ids that may be far above the counter.
        // Advancing a second time here skipped every other id (1, 3, 5, ...), so
        // the second window ever created was WindowId(3) and no caller could
        // name it. Ids must stay dense so a window's identity is predictable.
        self.create_with_id(id, owner, rect, kind, modal)?;
        Ok(id)
    }

    /// Registers a display-backed window using the KMS surface ID as its stable
    /// cross-layer identity. This keeps the GUI and display registries coherent.
    /// The reason the most recent window registration was refused, for the
    /// surface-create diagnostic to name. WHY it is stored and not returned:
    /// the window registrations that fail happen several locks deep, and the
    /// one caller that can print to the serial line does not hold the error.
    pub fn last_reject(&self) -> Option<&'static str> {
        self.last_reject
    }

    pub fn create_with_id(&mut self, id: WindowId, owner: u64, rect: WindowRect, kind: WindowKind, modal: bool) -> Result<(), &'static str> {
        if id.0 == 0 || rect.width == 0 || rect.height == 0 {
            self.last_reject = Some("invalid window identity or bounds");
            return Err("invalid window identity or bounds");
        }
        if self.windows.len() >= 128 {
            self.last_reject = Some("window limit reached");
            return Err("window limit reached");
        }
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

    pub fn top_visible_owner(&self) -> Option<u64> {
        self.windows.iter().filter(|w| w.visible && w.kind != WindowKind::SystemOverlay).max_by_key(|w| w.z).map(|w| w.owner)
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
        let dialog = wm.create(2, WindowRect{x:10,y:10,width:40,height:40}, WindowKind::Dialog, true).unwrap();
        // WHY the explicit id: ids are dense, so the window created second is
        // WindowId(2). It used to come back as WindowId(3) because `create`
        // advanced the counter twice, and the hard-coded expectation below is
        // what caught it.
        assert_eq!(dialog, WindowId(2), "the second window must get the second id");
        assert_eq!(wm.hit_test(5, 5), None);
        assert_eq!(wm.hit_test(20, 20).map(|v| v.0), Some(WindowId(2)));
        assert_eq!(wm.hit_test(20, 20).map(|v| v.0), Some(dialog),
            "the event must reach the modal window itself, not merely id 2");
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

    /// Logical portrait is the only space UI hit testing consumes: serial
    /// `TAP x y` is already in it, and `WindowRect.contains` is applied to it
    /// directly, with no scaling on the way in.
    const LOGICAL_PORTRAIT: WindowRect = WindowRect { x: 0, y: 0, width: 720, height: 1440 };

    /// A tap anywhere on a full-height logical portrait surface must resolve to
    /// the same window, including the lock screen shortcut band near the bottom.
    ///
    /// WHY: the lock screen draws its flashlight shortcut at (72, 1330) r=35
    /// and its kinetic unlock pill at y 1370..1405, so a window rect that stops
    /// short of the logical height silently swallows every bottom-screen tap.
    /// That is indistinguishable at the UI from "the control did nothing": the
    /// tap is routed to no window at all, so the application never even sees it.
    ///
    /// The rect must be the logical 720x1440 surface, never the size that
    /// happens to be visible on scanout (the launcher surface is composited
    /// down into a 360x720 slice of a 1280x720 framebuffer), and never a
    /// half-height 720x720 guess.
    ///
    /// Pinning assertion: `hit_test(72, 1330)` returns the same window as
    /// `hit_test(360, 700)`, and it does so only for the full-height rect.
    #[test]
    fn bottom_of_logical_portrait_routes_to_the_same_window_as_the_middle() {
        let mut wm = WindowManager::new();
        let app = wm.create(11, LOGICAL_PORTRAIT, WindowKind::Application, false).unwrap();

        assert_eq!(
            wm.hit_test(72, 1330),
            Some((app, 11)),
            "the lock screen flashlight shortcut must reach its window"
        );
        assert_eq!(
            wm.hit_test(72, 1330),
            wm.hit_test(360, 700),
            "a bottom-band tap and a mid-screen tap must resolve identically"
        );

        // Negative control: a rect sized from the displayed scanout slice or
        // from any stale half-height guess drops the band below the cut.
        let mut stale = WindowManager::new();
        let short = stale.create(11, WindowRect { x: 0, y: 0, width: 720, height: 720 }, WindowKind::Application, false).unwrap();
        assert_eq!(
            stale.hit_test(72, 1330), None,
            "a 720-tall rect must not be able to claim the y=1330 band"
        );
        assert_eq!(stale.hit_test(360, 700), Some((short, 11)));
    }

    const UNIT: WindowRect = WindowRect { x: 0, y: 0, width: 64, height: 64 };

    /// Automatic window ids must be handed out densely from 1.
    ///
    /// WHY this invariant: `WindowId` is a window's stable name in every
    /// cross-layer message, so "the window created second" has to be a knowable
    /// identity. `create` used to advance the counter a second time on top of
    /// `create_with_id`, producing 1, 3, 5, ... — the modal-dialog routing test
    /// above caught it as `Some(WindowId(3))` where `WindowId(2)` was correct.
    ///
    /// Pinning assertion: the second one. Three `create` calls plus a
    /// `create_with_id` registration must leave the next automatic id at exactly
    /// "highest id in use + 1", with no gap.
    #[test]
    fn window_ids_are_allocated_densely() {
        let mut wm = WindowManager::new();
        let a = wm.create(1, UNIT, WindowKind::Application, false).unwrap();
        let b = wm.create(2, UNIT, WindowKind::Dialog, true).unwrap();
        let c = wm.create(3, UNIT, WindowKind::Dialog, true).unwrap();
        assert_eq!([a, b, c], [WindowId(1), WindowId(2), WindowId(3)],
            "automatic ids must be dense: 1, 2, 3");

        // A display-registered surface id pulls the counter up, and the next
        // automatic id must follow it rather than collide with or skip past it.
        wm.create_with_id(WindowId(41), 7, UNIT, WindowKind::Application, false).unwrap();
        let d = wm.create(8, UNIT, WindowKind::Dialog, true).unwrap();
        assert_eq!(d, WindowId(42), "the next automatic id must be the highest id in use plus one");
    }

    /// The dialog is the input target by identity, not by a hard-coded number:
    /// whichever id `create` handed out, the modal window is the one that
    /// receives the event and the application window underneath does not.
    ///
    /// Pinning assertion: `hit_test` resolves to the dialog's own id, and the
    /// point outside the dialog still resolves to nothing at all.
    #[test]
    fn modal_priority_is_resolved_by_identity() {
        let mut wm = WindowManager::new();
        let app = wm.create(1, WindowRect{x:0,y:0,width:100,height:100}, WindowKind::Application, false).unwrap();
        let dialog = wm.create(2, WindowRect{x:10,y:10,width:40,height:40}, WindowKind::Dialog, true).unwrap();
        assert_ne!(dialog, app);
        assert_eq!(wm.hit_test(20, 20), Some((dialog, 2)), "the modal window must receive the event");
        assert_eq!(wm.hit_test(5, 5), None, "the modal window must block the window beneath it");
        assert_eq!(wm.focused(), Some(dialog));
    }
}
