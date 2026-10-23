//! Userspace-facing input event pipeline.
//!
//! Hardware-specific touch/keyboard drivers feed normalized events here.
//! The pipeline deliberately exposes events, not raw MMIO/controller state.

use alloc::collections::VecDeque;
use crate::drivers::touch_gestures::RecognizedGesture;
use crate::drivers::DisplayDriver;
use spin::Mutex;
use crate::task::TaskId;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum InputEventKind {
    Tap = 1,
    DoubleTap = 2,
    LongPress = 3,
    Pan = 4,
    Pinch = 5,
    Back = 6,
    Home = 7,
    Recents = 8,
    Cancel = 9,
    Fling = 10,
    Key = 11,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct InputEvent {
    pub kind: InputEventKind,
    pub x: u32,
    pub y: u32,
    pub dx: i32,
    pub dy: i32,
    pub scale_milli: u32,
}

impl InputEvent {
    pub fn from_key(event: crate::input::KeyEvent) -> Self {
        let code = match event.code {
            crate::input::KeyCode::Character(c) => c as u32,
            crate::input::KeyCode::Enter => 0x10000,
            crate::input::KeyCode::Escape => 0x10001,
            crate::input::KeyCode::Backspace => 0x10002,
            crate::input::KeyCode::Tab => 0x10003,
            crate::input::KeyCode::ArrowUp => 0x10004,
            crate::input::KeyCode::ArrowDown => 0x10005,
            crate::input::KeyCode::ArrowLeft => 0x10006,
            crate::input::KeyCode::ArrowRight => 0x10007,
            crate::input::KeyCode::Home => 0x10008,
            crate::input::KeyCode::End => 0x10009,
            crate::input::KeyCode::PageUp => 0x1000A,
            crate::input::KeyCode::PageDown => 0x1000B,
            crate::input::KeyCode::Unknown(code) => code,
        };
        let modifiers =
            (event.shift as u32)
            | ((event.ctrl as u32) << 1)
            | ((event.alt as u32) << 2)
            | ((event.meta as u32) << 3);
        Self {
            kind: InputEventKind::Key,
            x: code,
            y: if event.action == crate::input::KeyAction::Down { 1 } else { 0 },
            scale_milli: modifiers,
            ..Self::EMPTY
        }
    }

    pub fn from_canonical(gesture: crate::input::GestureEvent) -> Self {
        let kind = match gesture.kind {
            crate::input::GestureKind::Tap => InputEventKind::Tap,
            crate::input::GestureKind::DoubleTap => InputEventKind::DoubleTap,
            crate::input::GestureKind::LongPress => InputEventKind::LongPress,
            crate::input::GestureKind::Scroll => InputEventKind::Pan,
            crate::input::GestureKind::Fling => InputEventKind::Fling,
            crate::input::GestureKind::Back => InputEventKind::Back,
            crate::input::GestureKind::Home => InputEventKind::Home,
            crate::input::GestureKind::Recents => InputEventKind::Recents,
            crate::input::GestureKind::Cancel => InputEventKind::Cancel,
            crate::input::GestureKind::Pinch => InputEventKind::Pinch,
        };
        Self {
            kind,
            x: gesture.position.x.max(0) as u32,
            y: gesture.position.y.max(0) as u32,
            dx: gesture.delta_x,
            dy: gesture.delta_y,
            scale_milli: gesture.scale_milli,
            ..Self::EMPTY
        }
    }

    pub const EMPTY: Self = Self {
        kind: InputEventKind::Tap,
        x: 0,
        y: 0,
        dx: 0,
        dy: 0,
        scale_milli: 1000,
    };

    pub fn from_gesture(gesture: RecognizedGesture) -> Self {
        match gesture {
            RecognizedGesture::Tap { x, y } => Self { kind: InputEventKind::Tap, x, y, ..Self::EMPTY },
            RecognizedGesture::DoubleTap { x, y } => Self { kind: InputEventKind::DoubleTap, x, y, ..Self::EMPTY },
            RecognizedGesture::LongPress { x, y } => Self { kind: InputEventKind::LongPress, x, y, ..Self::EMPTY },
            RecognizedGesture::Pan { start_x, start_y, current_x, current_y, dx, dy } => {
                let disp_x = current_x as i32 - start_x as i32;
                let disp_y = current_y as i32 - start_y as i32;
                Self {
                    kind: InputEventKind::Pan,
                    x: current_x,
                    y: current_y,
                    dx: if disp_x != 0 { disp_x } else { dx },
                    dy: if disp_y != 0 { disp_y } else { dy },
                    ..Self::EMPTY
                }
            }
            RecognizedGesture::Pinch { center_x, center_y, scale_factor } => Self {
                kind: InputEventKind::Pinch, x: center_x, y: center_y, scale_milli: (scale_factor * 1000.0) as u32, ..Self::EMPTY
            },
        }
    }
}

pub struct InputPipeline {
    queue: VecDeque<(TaskId, InputEvent)>,
    service_queue: VecDeque<InputEvent>,
}

impl InputPipeline {
    pub const fn new() -> Self {
        Self {
            queue: VecDeque::new(),
            service_queue: VecDeque::new(),
        }
    }

    pub fn push(&mut self, event: InputEvent) {
        if self.service_queue.len() < 128 { self.service_queue.push_back(event); }
    }

    pub fn route_to_task(&mut self, task: TaskId, event: InputEvent) {
        if self.queue.len() < 128 { self.queue.push_back((task, event)); }
    }

    pub fn pop_for_task(&mut self, task: TaskId) -> Option<InputEvent> {
        let index = self.queue.iter().position(|(owner, _)| *owner == task)?;
        self.queue.remove(index).map(|(_, event)| event)
    }

    pub fn pop_service(&mut self) -> Option<InputEvent> { self.service_queue.pop_front() }

    pub fn clear(&mut self) {
        self.queue.clear();
        self.service_queue.clear();
    }
}

pub static INPUT_PIPELINE: Mutex<InputPipeline> = Mutex::new(InputPipeline::new());

/// Moves recognized hardware gestures into the stable userspace event queue.
/// Routes normalized input events to the currently focused surface owner.
/// Brings the shell back to the front of the screen.
///
/// WHY the platform does this and not the application: the gesture is the
/// platform's -- it belongs to the edge of the screen and every system
/// reserves it -- and an application that does not implement it leaves a
/// person who has opened an application with no way home. The shell's own
/// surfaces come forward and the foreground application's surfaces go
/// behind them, so the application keeps its state and the person gets
/// their desk back.
fn go_home() {
    let Some(shell) = crate::syscall::shell_task_id() else {
        return;
    };
    let mut kms = crate::drivers::display_kms::DRM_KMS.lock();
    apply_home_to_surfaces(&mut kms, shell);
    let mut display = crate::drivers::KmsDisplayDriver::new(&mut kms);
    let _ = display.composite_and_flip();
}

/// Brings the shell's surfaces forward and the others' back.
///
/// WHY this is a function over the compositor and not a sequence of calls
/// on it: the change is a decision about which surfaces the screen shows,
/// and a decision that cannot be run in a test is a decision nobody can
/// check.
fn apply_home_to_surfaces(
    kms: &mut crate::drivers::display_kms::DrmKmsController,
    shell: u64,
) {
    // WHY the shell comes forward first: the application's surfaces are
    // hidden after, and a frame in between would show the application on
    // top of the shell for one flip.
    kms.set_surfaces_visibility_for_task(shell, true);
    let owners: alloc::vec::Vec<u64> = kms
        .surfaces
        .iter()
        .filter(|s| s.owner_task != shell)
        .map(|s| s.owner_task)
        .collect();
    for owner in owners {
        kms.set_surfaces_visibility_for_task(owner, false);
    }
}

/// Whether the shell is the surface that owns the screen.
///
/// WHY this is asked at all: the shell handles its own home gesture (it
/// draws the unlocked desk), so the platform's action must not run a
/// second time on top of the shell's own handling.
fn shell_owns_the_screen() -> bool {
    let Some(shell) = crate::syscall::shell_task_id() else {
        return false;
    };
    let kms = crate::drivers::display_kms::DRM_KMS.lock();
    kms.surfaces
        .iter()
        .filter(|s| s.visible && s.input_enabled)
        .max_by_key(|s| s.z_order)
        .map(|s| s.owner_task == shell)
        .unwrap_or(false)
}

pub fn dispatch_to_focused_window(event: InputEvent) {
    // WHY the home, back and recents gestures are taken here: this is the one point where the
    // platform knows both what was recognised and who is in front. An
    // application in front that does not implement the gesture would
    // otherwise swallow it, and the person would be stuck in the app.
    if (event.kind == InputEventKind::Home || event.kind == InputEventKind::Back || event.kind == InputEventKind::Recents) && !shell_owns_the_screen() {
        go_home();
    }
    let owner = {
        // WHY only the power key is a system key here: the system owns the
        // screen lock, and the power key turns the device off whatever is
        // on it. The arrow keys are navigation keys, and an app with a
        // scrollable list has to receive them -- the down arrow was
        // captured here as a lock key, so the list could not be scrolled
        // with the keyboard at all, and the key was spent on a surface
        // nobody was looking at.
        let is_system_power_lock = event.kind == InputEventKind::Key && event.x == 0x1000F;
        let top_kms_owner = {
            let mut target_owner = None;
            for _ in 0..1000 {
                if let Some(kms) = crate::drivers::display_kms::DRM_KMS.try_lock() {
                    if is_system_power_lock {
                        target_owner = kms.surfaces.iter()
                            .find(|s| s.z_order == 0)
                            .or_else(|| kms.surfaces.iter().min_by_key(|s| s.z_order))
                            .map(|s| s.owner_task);
                    } else {
                        target_owner = kms.surfaces.iter()
                            .filter(|s| s.visible && s.input_enabled)
                            .max_by_key(|s| s.z_order)
                            .map(|s| s.owner_task);
                    }
                    break;
                }
                core::hint::spin_loop();
            }
            target_owner
        };

        let resolved = if let Some(top_task) = top_kms_owner {
            Some(top_task)
        } else {
            let target = {
                let mut target_val = None;
                for _ in 0..1000 {
                    if let Some(mut wm) = crate::gui::WINDOW_MANAGER.try_lock() {
                        target_val = match event.kind {
                            InputEventKind::Tap | InputEventKind::DoubleTap | InputEventKind::LongPress =>
                                wm.hit_test(event.x, event.y).map(|(_, owner)| owner),
                            InputEventKind::Pan | InputEventKind::Pinch =>
                                wm.focused_owner().or_else(|| wm.top_visible_owner()),
                            _ => wm.focused_owner().or_else(|| wm.top_visible_owner()),
                        };
                        break;
                    }
                    core::hint::spin_loop();
                }
                target_val
            };
            target.or_else(|| {
                for _ in 0..1000 {
                    if let Some(kms) = crate::drivers::display_kms::DRM_KMS.try_lock() {
                        // WHY visibility is required here as well as in the primary
                        // path: an app that exited without tearing its surface down
                        // leaves a hidden, still-input-enabled surface, and routing
                        // to it silently dropped every event until that app's task id
                        // was reused. The hit path above only hits visible windows;
                        // this fallback must not disagree with it.
                        return kms
                            .surfaces
                            .iter()
                            .rev()
                            .find(|s| s.input_enabled && s.visible)
                            .map(|s| s.owner_task);
                    }
                    core::hint::spin_loop();
                }
                None
            })
        };
        crate::serial_println!("[INPUT DISPATCH] event={:?} (x={}, y={}) resolved={:?}",
            event.kind, event.x, event.y, resolved);
        resolved
    };
    if let Some(owner) = owner {
        crate::serial_println!("[INPUT DISPATCH] Routing to task {}", owner);
        for _ in 0..1000 {
            if let Some(mut pipe) = INPUT_PIPELINE.try_lock() {
                pipe.route_to_task(TaskId(owner), event);
                break;
            }
            core::hint::spin_loop();
        }
        let mut msg = [0u8; 24];
        msg[0] = event.kind as u8;
        msg[4..8].copy_from_slice(&event.x.to_le_bytes());
        msg[8..12].copy_from_slice(&event.y.to_le_bytes());
        msg[12..16].copy_from_slice(&event.dx.to_le_bytes());
        msg[16..20].copy_from_slice(&event.dy.to_le_bytes());
        msg[20..24].copy_from_slice(&event.scale_milli.to_le_bytes());
        for _ in 0..1000 {
            if let Some(mut router) = crate::ipc::IPC_ROUTER.try_lock() {
                let _ = router.send_direct(TaskId(0), TaskId(owner), 0x494E_5054, &msg);
                break;
            }
            core::hint::spin_loop();
        }
        crate::event::post_event(TaskId(owner), crate::event::EVENT_TOUCH_INPUT);
        crate::serial_println!("[INPUT DISPATCH] Dispatched to task {} complete", owner);
    }
}

pub fn pump_touch_events() {
    let gestures: alloc::vec::Vec<_> = {
        let mut g = alloc::vec::Vec::new();
        for _ in 0..1000 {
            if let Some(mut touch) = crate::drivers::touch_gestures::TOUCH_ENGINE.try_lock() {
                g = touch.gesture_queue.drain(..).collect();
                break;
            }
            core::hint::spin_loop();
        }
        g
    };
    if gestures.is_empty() {
        return;
    }
    for gesture in gestures {
        crate::serial_println!("[INPUT PUMP] gesture={:?}", gesture);
        let event = InputEvent::from_gesture(gesture);
        for _ in 0..1000 {
            if let Some(mut pipe) = INPUT_PIPELINE.try_lock() {
                pipe.push(event);
                break;
            }
            core::hint::spin_loop();
        }
        dispatch_to_focused_window(event);
    }
    crate::event::broadcast_event(crate::event::EVENT_TOUCH_INPUT);
}

#[cfg(test)]
mod tests {    #[test]
    fn the_home_gesture_brings_the_shell_back() {
        let mut kms = crate::drivers::display_kms::DrmKmsController::new(720, 1440);
        kms.set_display_mode(720, 1440, 60).expect("mode");
        let shell = kms.create_surface(720, 1440, 0, 11);
        let app = kms.create_surface(720, 1440, 200, 14);
        assert_ne!(shell, 0);
        assert_ne!(app, 0);

        // The application is in front.
        kms.set_surfaces_visibility_for_task(11, false);
        kms.set_surfaces_visibility_for_task(14, true);

        super::apply_home_to_surfaces(&mut kms, 11);

        let shell_visible = kms.surfaces.iter().find(|s| s.id == shell).map(|s| s.visible);
        let app_visible = kms.surfaces.iter().find(|s| s.id == app).map(|s| s.visible);
        assert_eq!(shell_visible, Some(true), "the shell has to come back");
        assert_eq!(app_visible, Some(false), "the application has to go behind");
    }

    #[test]
    fn canonical_system_gesture_encoding_is_stable() {
        let event = InputEvent::from_canonical(crate::input::GestureEvent {
            kind: crate::input::GestureKind::Home,
            position: crate::input::Point { x: 360, y: 1200 },
            delta_x: 0,
            delta_y: -300,
            pointer_id: 0,
            scale_milli: 1000,
        });
        assert_eq!(event.kind, InputEventKind::Home);
        assert_eq!((event.x, event.y, event.dy), (360, 1200, -300));
    }

    #[test]
    fn canonical_pinch_encoding_preserves_scale() {
        let event = InputEvent::from_canonical(crate::input::GestureEvent {
            kind: crate::input::GestureKind::Pinch,
            position: crate::input::Point { x: 360, y: 720 },
            delta_x: 40,
            delta_y: 0,
            pointer_id: 1,
            scale_milli: 1250,
        });
        assert_eq!(event.kind, InputEventKind::Pinch);
        assert_eq!(event.scale_milli, 1250);
    }


    use super::{InputEvent, InputEventKind};
    use crate::drivers::touch_gestures::RecognizedGesture;

    #[test]
    fn gesture_is_normalized_without_raw_device_details() {
        let e = InputEvent::from_gesture(RecognizedGesture::Pan {
            start_x: 10, start_y: 20, current_x: 30, current_y: 50, dx: 20, dy: 30,
        });
        assert_eq!(e.kind, InputEventKind::Pan);
        assert_eq!((e.x, e.y, e.dx, e.dy), (30, 50, 20, 30));
    }

    #[test]
    fn swipe_up_displacement_preserved() {
        let e = InputEvent::from_gesture(RecognizedGesture::Pan {
            start_x: 360, start_y: 1420, current_x: 360, current_y: 1100, dx: 0, dy: -320,
        });
        assert_eq!(e.kind, InputEventKind::Pan);
        assert_eq!((e.x, e.y, e.dx, e.dy), (360, 1100, 0, -320));
    }
}
