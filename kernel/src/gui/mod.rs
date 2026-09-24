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

pub use component::{PermissionModal, ShareSheetDrawer, UiCard};
pub use shade::{NotificationShade, QuickToggle, ShadeState};
pub use shell::MobileShell;
pub use surface::{Color, Rect, Surface};
pub use compositor::{GravityGlassCompositor, LayerType, SystemOverlayState, GRAVITY_COMPOSITOR};
pub use physics::InertialScroller;

use spin::Mutex;
use crate::framebuffer::{PHONE_X, PHONE_Y};
use crate::input::{InputEvent, TouchGesture};
use crate::serial_println;

/// Global thread-safe instance of the Mobile Shell.
pub static MOBILE_SHELL: Mutex<MobileShell> = Mutex::new(MobileShell::new());

/// Initializes the GUI subsystem and verifies capacitive touch hit testing via self-tests.
pub fn init() {
    let mut shell = MOBILE_SHELL.lock();

    // 1. Capacitive Touch Tap on Card 1 (InterOS System Diagnostics)
    let card1_tap = InputEvent::Gesture(TouchGesture::Tap {
        x: (PHONE_X + 50) as u32,
        y: (PHONE_Y + 160) as u32,
    });
    let intent1 = shell.handle_event(card1_tap);
    assert_eq!(intent1, Some("SYSTEM_DIAGNOSTICS_OK"));
    serial_println!("[TEST] Touch Screen Card Hit-Test: Tap on Card 1 dispatches 'SYSTEM_DIAGNOSTICS_OK'.");

    // 2. Capacitive Touch Tap on Card 2 (Android ACR Container)
    let card2_tap = InputEvent::Gesture(TouchGesture::Tap {
        x: (PHONE_X + 50) as u32,
        y: (PHONE_Y + 250) as u32,
    });
    let intent2 = shell.handle_event(card2_tap);
    assert_eq!(intent2, Some("ACR_AUDIT_CONTAINER"));
    serial_println!("[TEST] Touch Screen Card Hit-Test: Tap on Card 2 dispatches 'ACR_AUDIT_CONTAINER'.");

    // 2b. Capacitive Touch Tap on Card 4 (Sovereign Settings App)
    let card4_tap = InputEvent::Gesture(TouchGesture::Tap {
        x: (PHONE_X + 50) as u32,
        y: (PHONE_Y + 430) as u32,
    });
    let intent4 = shell.handle_event(card4_tap);
    assert_eq!(intent4, Some("APP_LAUNCH_SETTINGS"));
    serial_println!("[TEST] Touch Screen Card Hit-Test: Tap on Card 4 dispatches 'APP_LAUNCH_SETTINGS'.");

    // 2c. Capacitive Touch Tap on Card 7 (Sovereign Vault App)
    let card7_tap = InputEvent::Gesture(TouchGesture::Tap {
        x: (PHONE_X + 50) as u32,
        y: (PHONE_Y + 710) as u32,
    });
    let intent7 = shell.handle_event(card7_tap);
    assert_eq!(intent7, Some("APP_LAUNCH_VAULT"));
    serial_println!("[TEST] Touch Screen Card Hit-Test: Tap on Card 7 dispatches 'APP_LAUNCH_VAULT'.");

    // 2d. Capacitive Touch Tap on Card 8 (Sovereign Camera App)
    let card8_tap = InputEvent::Gesture(TouchGesture::Tap {
        x: (PHONE_X + 50) as u32,
        y: (PHONE_Y + 800) as u32,
    });
    let intent8 = shell.handle_event(card8_tap);
    assert_eq!(intent8, Some("APP_LAUNCH_CAMERA"));
    serial_println!("[TEST] Touch Screen Card Hit-Test: Tap on Card 8 dispatches 'APP_LAUNCH_CAMERA'.");

    // 2e. Capacitive Touch Tap on Card 9 (Sovereign AI Assistant App)
    let card9_tap = InputEvent::Gesture(TouchGesture::Tap {
        x: (PHONE_X + 50) as u32,
        y: (PHONE_Y + 900) as u32,
    });
    let intent9 = shell.handle_event(card9_tap);
    assert_eq!(intent9, Some("APP_LAUNCH_CALCULATOR"));
    serial_println!("[TEST] Touch Screen Card Hit-Test: Tap on Card 9 dispatches 'APP_LAUNCH_CALCULATOR'.");

    // 2f. Capacitive Touch Tap on Card 10 (Sovereign Browser App)
    let card10_tap = InputEvent::Gesture(TouchGesture::Tap {
        x: (PHONE_X + 50) as u32,
        y: (PHONE_Y + 990) as u32,
    });
    let intent10 = shell.handle_event(card10_tap);
    assert_eq!(intent10, Some("APP_LAUNCH_BROWSER"));
    serial_println!("[TEST] Touch Screen Card Hit-Test: Tap on Card 10 dispatches 'APP_LAUNCH_BROWSER'.");

    // 2g. Capacitive Touch Tap on Card 11 (Sovereign Messages App)
    let card11_tap = InputEvent::Gesture(TouchGesture::Tap {
        x: (PHONE_X + 50) as u32,
        y: (PHONE_Y + 1080) as u32,
    });
    let intent11 = shell.handle_event(card11_tap);
    assert_eq!(intent11, Some("APP_LAUNCH_MESSAGES"));
    serial_println!("[TEST] Touch Screen Card Hit-Test: Tap on Card 11 dispatches 'APP_LAUNCH_MESSAGES'.");

    // 2h. Capacitive Touch Tap on Card 12 (Sovereign Calendar App)
    let card12_tap = InputEvent::Gesture(TouchGesture::Tap {
        x: (PHONE_X + 50) as u32,
        y: (PHONE_Y + 1170) as u32,
    });
    let intent12 = shell.handle_event(card12_tap);
    assert_eq!(intent12, Some("APP_LAUNCH_CALENDAR"));
    serial_println!("[TEST] Touch Screen Card Hit-Test: Tap on Card 12 dispatches 'APP_LAUNCH_CALENDAR'.");

    // 2i. Capacitive Touch Tap on Card 13 (Sovereign Music App)
    let card13_tap = InputEvent::Gesture(TouchGesture::Tap {
        x: (PHONE_X + 50) as u32,
        y: (PHONE_Y + 1260) as u32,
    });
    let intent13 = shell.handle_event(card13_tap);
    assert_eq!(intent13, Some("APP_LAUNCH_MUSIC"));
    serial_println!("[TEST] Touch Screen Card Hit-Test: Tap on Card 13 dispatches 'APP_LAUNCH_MUSIC'.");

    // 3. Capacitive Touch Tap outside bounds (empty bezel / backdrop)
    let outside_tap = InputEvent::Gesture(TouchGesture::Tap {
        x: 50,
        y: 50,
    });
    let intent_none = shell.handle_event(outside_tap);
    assert_eq!(intent_none, None);
    serial_println!("[TEST] Touch Boundary Enforcement: Off-screen taps correctly ignored.");

    // 4. Notification Shade Pull-Down Drag Gesture (from top notch down 100px)
    let pull_down = InputEvent::Gesture(TouchGesture::Drag {
        start_x: (PHONE_X + 100) as u32,
        start_y: (PHONE_Y + 10) as u32,
        current_x: (PHONE_X + 100) as u32,
        current_y: (PHONE_Y + 150) as u32,
    });
    let intent_shade = shell.handle_event(pull_down);
    assert_eq!(intent_shade, Some("NOTIFICATION_SHADE_OPEN"));
    assert_eq!(shell.shade.state, shade::ShadeState::Open);
    serial_println!("[TEST] Notification Shade: Downward swipe (Y: 10 -> 150) expands shade (State = Open).");

    // 5. Quick Settings Toggle Tap (5G Cellular toggle - index 1)
    let pill_w = (crate::framebuffer::PHONE_W as u32) / 5;
    let toggle_tap = InputEvent::Gesture(TouchGesture::Tap {
        x: (PHONE_X as u32) + (pill_w * 1) + 10,
        y: (PHONE_Y + 80) as u32,
    });
    let intent_toggle = shell.handle_event(toggle_tap);
    assert_eq!(intent_toggle, Some("TOGGLE_CELLULAR"));
    serial_println!("[TEST] Notification Shade: Quick setting toggle '5G Cellular' hit-test PASSED.");

    shell.shade.state = shade::ShadeState::Closed; // Reset to nominal closed state

    // 6. Double-Buffered Surface & Dirty-Region Blitter Test
    let mut test_surface = Surface::new(100, 100);
    assert_eq!(test_surface.width, 100);
    assert_eq!(test_surface.height, 100);

    // Test solid fill & dirty tracking
    test_surface.clear_dirty();
    test_surface.fill_rect(Rect::new(10, 10, 30, 20), Color::rgb(255, 0, 0));
    assert_eq!(test_surface.dirty_rect, Some(Rect::new(10, 10, 30, 20)));
    assert_eq!(test_surface.get_pixel(15, 15), Color::rgb(255, 0, 0));
    assert_eq!(test_surface.get_pixel(5, 5), Color::rgba(0, 0, 0, 0));

    // Test alpha blending (50% white over red)
    test_surface.blend_rect(Rect::new(10, 10, 10, 10), Color::rgba(255, 255, 255, 128));
    let blended = test_surface.get_pixel(12, 12);
    // Red channel: (255 * 128 + 255 * 127) / 255 = 255
    // Green channel: (255 * 128 + 0 * 127) / 255 ≈ 128
    // Blue channel: (255 * 128 + 0 * 127) / 255 ≈ 128
    assert_eq!(blended.r, 255);
    assert!((blended.g as i16 - 128).abs() <= 1);
    assert!((blended.b as i16 - 128).abs() <= 1);

    // Test dirty rect union
    test_surface.mark_dirty(Rect::new(50, 50, 20, 20));
    let united = test_surface.dirty_rect.unwrap();
    assert_eq!(united.x, 10);
    assert_eq!(united.y, 10);
    assert_eq!(united.x + united.width, 70);
    assert_eq!(united.y + united.height, 70);

    serial_println!("[TEST] Graphics Surface: Backbuffer allocated, alpha blending and dirty-rect union PASSED.");

    // 7. Initialize Gravity Glass Compositor & Ghost Overlay
    compositor::init();

    // 8. Initialize Fluid Inertial Physics Touch Scroller
    physics::init();

    // 9. Interactive Permission Modal Touch Hit-Testing
    let test_modal = PermissionModal::new(
        1001,
        "interos.camera",
        "Camera Sensor Access",
        "Allows capturing live frames from camera",
        PHONE_X,
        PHONE_Y,
        crate::framebuffer::PHONE_W,
    );
    shell.open_permission_modal(test_modal);
    assert!(shell.active_modal.is_some());

    // Hit-test ALLOW button
    let allow_tap = InputEvent::Gesture(TouchGesture::Tap {
        x: (test_modal.allow_btn_x + 10) as u32,
        y: (test_modal.allow_btn_y + 10) as u32,
    });
    let intent_allow = shell.handle_event(allow_tap);
    assert_eq!(intent_allow, Some("PERMISSION_GRANTED"));
    assert!(shell.active_modal.is_none());

    // Open modal again and hit-test DENY button
    shell.open_permission_modal(test_modal);
    let deny_tap = InputEvent::Gesture(TouchGesture::Tap {
        x: (test_modal.deny_btn_x + 10) as u32,
        y: (test_modal.deny_btn_y + 10) as u32,
    });
    let intent_deny = shell.handle_event(deny_tap);
    assert_eq!(intent_deny, Some("PERMISSION_DENIED"));
    assert!(shell.active_modal.is_none());

    serial_println!("[TEST] Desktop Shell Permission Modal: Interactive ALLOW/DENY consent hit-testing PASSED.");

    // 10. Interactive Sovereign Share Sheet Drawer Hit-Testing
    let test_drawer = ShareSheetDrawer::new(
        2001,
        "image/x-vault-sector",
        "/vault/sector/201",
        PHONE_X,
        PHONE_Y,
        crate::framebuffer::PHONE_W,
        crate::framebuffer::PHONE_H,
    );
    shell.open_share_drawer(test_drawer);
    assert!(shell.active_share_drawer.is_some());

    // Hit-test Target 1 (Messages)
    let share_tap = InputEvent::Gesture(TouchGesture::Tap {
        x: (test_drawer.target1_x + 10) as u32,
        y: (test_drawer.target1_y + 10) as u32,
    });
    let intent_share = shell.handle_event(share_tap);
    assert_eq!(intent_share, Some("AMCP_SHARE_MESSAGES"));
    assert!(shell.active_share_drawer.is_none());

    // Open drawer again and hit-test outside dismissal
    shell.open_share_drawer(test_drawer);
    let outside_drawer = InputEvent::Gesture(TouchGesture::Tap {
        x: (PHONE_X + 10) as u32,
        y: (PHONE_Y + 10) as u32,
    });
    let intent_dismiss = shell.handle_event(outside_drawer);
    assert_eq!(intent_dismiss, Some("SHARE_DRAWER_DISMISSED"));
    assert!(shell.active_share_drawer.is_none());

    serial_println!("[TEST] Sovereign Share Sheet Drawer: Interactive app target hit-testing PASSED.");

    serial_println!("--- [Capacitive Touch Shell Subsystem 100% Attested & Enforced] ---");
}

