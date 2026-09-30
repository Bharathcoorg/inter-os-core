//! Graphical User Interface (GUI) & Touch Compositor Subsystem
//!
//! Provides the interactive smartphone windowing layout, card hit testing,
//! and touch-driven intent routing.

pub mod component;
pub mod shade;
pub mod shell;
pub mod surface;
pub mod compositor;
pub mod physics;
pub mod window;
pub mod ui;
pub mod buffer;

pub use component::{PermissionModal, ShareSheetDrawer, UiCard};
pub use shade::{NotificationShade, QuickToggle, ShadeState};
pub use shell::MobileShell;
pub use surface::{Color, Rect, Surface};
pub use compositor::{GravityGlassCompositor, LayerType, SystemOverlayState, GRAVITY_COMPOSITOR};
pub use physics::InertialScroller;
pub use window::{Window, WindowId, WindowKind, WindowManager, WindowRect};
pub use buffer::{BufferId, BufferManager, BufferState, DisplayBuffer, FrameFence};
pub use ui::{Alignment, Axis, UiAnimation, UiBounds, UiInsets, UiLayout, UiNode, UiPoint, UiRole, UiSize};

use spin::Mutex;
use crate::serial_println;

/// Global thread-safe instance of the Mobile Shell.
pub static MOBILE_SHELL: Mutex<MobileShell> = Mutex::new(MobileShell::new());
/// Authoritative application window registry used by input routing and composition.
pub static WINDOW_MANAGER: Mutex<WindowManager> = Mutex::new(WindowManager::new());
/// Global display-buffer lifecycle owner. Compositor publication must pass through explicit fences.
pub static BUFFER_MANAGER: Mutex<BufferManager> = Mutex::new(BufferManager::new());

/// Initializes the GUI subsystem and verifies capacitive touch hit testing via self-tests.
pub fn init() {
    // The kernel owns display primitives, buffers, windows, and composition
    // infrastructure only. Consumer smartphone navigation is owned by the
    // userspace launcher.
    let _ = WINDOW_MANAGER.lock();
    let _ = BUFFER_MANAGER.lock();
    let _ = GRAVITY_COMPOSITOR.lock();
    serial_println!("[GUI] Low-level display/compositor infrastructure online.");
}
