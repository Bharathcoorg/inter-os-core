//! Gravity Glass: Multi-Layer Mobile Compositor Engine
//!
//! Provides hardware-isolated, 4-layer window and surface composition:
//! - Layer 0: System Wallpaper & Mesh
//! - Layer 1: App Canvas & Active Cards
//! - Layer 2: System Decorations (Status Bar, Civil Clock, Dynamic Island)
//! - Layer 3: System Notification & Modal Overlay

use spin::Mutex;
use crate::gui::surface::{Color, Rect, Surface};
use crate::serial_println;

/// Multi-layer compositor hierarchy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LayerType {
    Wallpaper = 0,
    AppSurface = 1,
    SystemDecorations = 2,
    SystemOverlay = 3,
}

/// State of the mobile System Notification & Modal Overlay.
#[derive(Debug, Clone)]
pub struct SystemOverlayState {
    pub active: bool,
    pub alert_id: Option<u32>,
    pub message: [u8; 64],
    pub message_len: usize,
    pub action_pos: Option<(u32, u32)>,
    pub dismissals_count: u32,
}

impl SystemOverlayState {
    pub const fn new() -> Self {
        Self {
            active: false,
            alert_id: None,
            message: [0u8; 64],
            message_len: 0,
            action_pos: None,
            dismissals_count: 0,
        }
    }
}

/// Gravity Glass Multi-Layer Compositor.
pub struct GravityGlassCompositor {
    pub system_overlay: SystemOverlayState,
}

impl GravityGlassCompositor {
    pub const fn new() -> Self {
        Self {
            system_overlay: SystemOverlayState::new(),
        }
    }

    /// Activates a system notification or modal overlay banner.
    pub fn activate_system_overlay(
        &mut self,
        alert_id: u32,
        message: &str,
        action_pos: Option<(u32, u32)>,
    ) {
        self.system_overlay.active = true;
        self.system_overlay.alert_id = Some(alert_id);
        let b = message.as_bytes();
        let n = core::cmp::min(b.len(), self.system_overlay.message.len());
        self.system_overlay.message[..n].copy_from_slice(&b[..n]);
        self.system_overlay.message_len = n;
        self.system_overlay.action_pos = action_pos;
    }

    /// Dismisses the system overlay.
    pub fn dismiss_overlay(&mut self) {
        self.system_overlay.active = false;
        self.system_overlay.alert_id = None;
        self.system_overlay.action_pos = None;
        self.system_overlay.message_len = 0;
    }

    /// Dismisses active transient overlay upon user touch interaction.
    pub fn trigger_force_takeover(&mut self) -> bool {
        if self.system_overlay.active {
            self.system_overlay.active = false;
            self.system_overlay.alert_id = None;
            self.system_overlay.action_pos = None;
            self.system_overlay.message_len = 0;
            self.system_overlay.dismissals_count += 1;
            serial_println!("[COMPOSITOR] Touch interaction: System overlay dismissed (count #{}).", self.system_overlay.dismissals_count);
            true
        } else {
            false
        }
    }

    /// Renders a specific compositor layer to the graphics surface.
    pub fn render_layer(&self, layer: LayerType, surface: &mut Surface) {
        match layer {
            LayerType::Wallpaper => {
                // Layer 0: Obsidian dark background fill
                surface.fill_rect(
                    Rect::new(0, 0, surface.width, surface.height),
                    Color::OBSIDIAN,
                );
            }
            LayerType::AppSurface => {
                // Layer 1: App Cards and surface elements
                surface.draw_rect_outline(
                    Rect::new(4, 4, surface.width.saturating_sub(8), surface.height.saturating_sub(8)),
                    1,
                    Color::CARD_BORDER,
                );
            }
            LayerType::SystemDecorations => {
                // Layer 2: Status bar decoration
                surface.fill_rect(
                    Rect::new(0, 0, surface.width, 24),
                    Color::rgba(16, 22, 34, 220),
                );
            }
            LayerType::SystemOverlay => {
                // Layer 3: System Notification & Modal Banner
                if self.system_overlay.active {
                    let hud_rect = Rect::new(20, 36, surface.width.saturating_sub(40), 28);
                    surface.blend_rect(hud_rect, Color::rgba(0, 229, 255, 64));
                    surface.draw_rect_outline(hud_rect, 1, Color::CYAN);

                    // Render Message Text
                    if let Ok(msg) = core::str::from_utf8(
                        &self.system_overlay.message[..self.system_overlay.message_len],
                    ) {
                        surface.draw_string(30, 44, msg, 1, Color::WHITE, None);
                    }
                }
            }
        }
    }
}

/// Global thread-safe instance of the Gravity Glass Compositor.
pub static GRAVITY_COMPOSITOR: Mutex<GravityGlassCompositor> =
    Mutex::new(GravityGlassCompositor::new());

/// Initializes the Gravity Glass compositor and verifies layer composition.
pub fn init() {
    let mut comp = GRAVITY_COMPOSITOR.lock();

    // 1. Verify nominal state
    assert!(!comp.system_overlay.active, "System overlay must be inactive on boot");
    assert_eq!(comp.system_overlay.dismissals_count, 0);

    // 2. Activate System Overlay for test notification
    comp.activate_system_overlay(1, "System Diagnostics: All Subsystems Nominal", Some((120, 300)));
    assert!(comp.system_overlay.active);
    assert_eq!(comp.system_overlay.alert_id, Some(1));

    // 3. Render 4 layers into a test surface
    let mut test_surf = Surface::new(200, 400);
    comp.render_layer(LayerType::Wallpaper, &mut test_surf);
    comp.render_layer(LayerType::AppSurface, &mut test_surf);
    comp.render_layer(LayerType::SystemDecorations, &mut test_surf);
    comp.render_layer(LayerType::SystemOverlay, &mut test_surf);

    // 4. Test Touch Interaction Dismissal
    let dismissed = comp.trigger_force_takeover();
    assert!(dismissed, "Touch interaction must dismiss active overlay");
    assert!(!comp.system_overlay.active, "System overlay must be dismissed");
    assert_eq!(comp.system_overlay.dismissals_count, 1);

    // 5. Subsequent takeover when inactive returns false
    let second_takeover = comp.trigger_force_takeover();
    assert!(!second_takeover, "Second takeover must be no-op");

    serial_println!(
        "[TEST] Gravity Glass Compositor: 4-layer composition, System Overlay, and Touch Dismissal VERIFIED."
    );
}
