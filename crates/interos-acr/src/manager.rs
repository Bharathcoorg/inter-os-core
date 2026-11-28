// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

//! Unprivileged Ring-3 Android Compatibility Runtime Application Manager
//!
//! Replaces the legacy Ring-0 `ANDROID_RUNTIME` singleton with a userspace coordinator
//! managing isolated Android application domains, lifecycle states, VM execution,
//! message loopers, and view surface rendering.

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;
use interos_android_format::apk::{is_valid_package_name, AcrAuditStatus, ApkPackage};
use interos_android_format::dex::DexPackage;
use crate::runtime::{DalvikVmError, DalvikVmState};
use crate::looper::{Choreographer, Looper, TaskRecord};
use crate::framework::ActivityLifecycleState;
use crate::view::{MotionEvent, View};
use crate::draw::{DrawList, render_draw_list_to_buffer};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AcrError {
    ApkParseFailed,
    SignatureVerificationFailed,
    AuditRejected,
    DexNotFound,
    DexParseFailed,
    MethodExecutionFailed,
    AppNotFound,
    OutOfMemory,
    /// The APK declared a package name that cannot be an application
    /// identity, so no private directory may be made from it.
    InvalidPackageName,
    /// The application's own click handler did not run to completion.
    CallbackExecutionFailed,
}

/// The private directory an application keeps its own files in.
///
/// WHY the name is checked before it becomes a path: the directory is made
/// from the name the APK declared, and a name carrying a path separator or a
/// parent reference would place the application's files outside its own
/// sandbox -- in another application's directory, or in the platform's. A
/// name that cannot be an application identity gets no directory at all,
/// and the manager refuses to launch such an application.
///
/// WHY the directory depends only on the package name: a process that is
/// recreated after it was killed finds the same private files in the same
/// place, which is what the application was told it had.
fn private_data_dir(package_name: &str) -> String {
    if !is_valid_package_name(package_name) {
        return String::new();
    }
    alloc::format!("/data/data/{}", package_name)
}

/// Runs the application's own activity lifecycle steps.
///
/// WHY each step is run separately: Android calls `onCreate`, then
/// `onStart`, then `onResume`, and an application builds its state in those
/// calls. The manager used to run only `onCreate` and then declare the
/// activity resumed, so the code between those points never ran. A step the
/// APK does not define is not a failure: the framework's default runs in its
/// place, and the lifecycle still advances.
fn run_activity_lifecycle(
    vm: &mut DalvikVmState,
    dex: &DexPackage,
    activity_class: &str,
) -> Result<ActivityLifecycleState, AcrError> {
    let mut state = ActivityLifecycleState::Uninitialized;
    for (method, next) in [
        ("onCreate", ActivityLifecycleState::Created),
        ("onStart", ActivityLifecycleState::Started),
        ("onResume", ActivityLifecycleState::Resumed),
    ] {
        match vm.invoke_method(dex, activity_class, method) {
            Ok(_) => {}
            // The APK does not define this step. The framework's default
            // runs in its place and the lifecycle still advances.
            Err(DalvikVmError::MethodNotFound) => {}
            Err(_) => return Err(AcrError::MethodExecutionFailed),
        }
        state = next;
    }
    Ok(state)
}

/// An isolated Android application instance managed by the Ring-3 ACR manager.
pub struct AcrAppInstance {
    pub pid: u32,
    pub task_id: u64,
    pub uid: u32,
    pub package_name: String,
    pub target_sdk: u32,
    pub activity_state: ActivityLifecycleState,
    pub vm: DalvikVmState,
    pub looper: Looper,
    pub choreographer: Choreographer,
    pub task_record: TaskRecord,
    pub root_view: Option<View>,
    pub memory_quota_bytes: usize,
    pub surface_id: u32,
    pub classes_dex: Vec<u8>,
    /// The parsed, validated DEX, owned by the instance and dropped when the
    /// instance is replaced or torn down. `None` until a successful load.
    pub parsed_dex: Option<DexPackage>,
    pub data_dir: String,
    /// The permissions the APK audit granted this application, by name.
    ///
    /// WHY the instance carries them: a service that answers this
    /// application has to be able to say whether the application asked for
    /// it, and the audit is the only thing that can answer that.
    pub granted_permissions: Vec<String>,
}

impl AcrAppInstance {
    pub fn new(
        pid: u32,
        task_id: u64,
        package_name: &str,
        target_sdk: u32,
        surface_id: u32,
        memory_quota_bytes: usize,
        classes_dex: Vec<u8>,
    ) -> Self {
        let uid = 10000 + (pid.saturating_sub(1000));
        let data_dir = private_data_dir(package_name);
        Self {
            pid,
            task_id,
            uid,
            package_name: String::from(package_name),
            target_sdk,
            activity_state: ActivityLifecycleState::Created,
            vm: DalvikVmState::new(),
            looper: Looper::new(),
            choreographer: Choreographer::new(),
            task_record: TaskRecord::new(pid, package_name),
            root_view: None,
            memory_quota_bytes,
            surface_id,
            classes_dex,
            parsed_dex: None,
            data_dir,
            granted_permissions: Vec::new(),
        }
    }

    pub fn new_with_uid(
        pid: u32,
        task_id: u64,
        uid: u32,
        package_name: &str,
        target_sdk: u32,
        surface_id: u32,
        memory_quota_bytes: usize,
        classes_dex: Vec<u8>,
    ) -> Self {
        let data_dir = private_data_dir(package_name);
        Self {
            pid,
            task_id,
            uid,
            package_name: String::from(package_name),
            target_sdk,
            activity_state: ActivityLifecycleState::Created,
            vm: DalvikVmState::new(),
            looper: Looper::new(),
            choreographer: Choreographer::new(),
            task_record: TaskRecord::new(pid, package_name),
            root_view: None,
            memory_quota_bytes,
            surface_id,
            classes_dex,
            parsed_dex: None,
            data_dir,
            granted_permissions: Vec::new(),
        }
    }

    /// Records the permissions the audit granted this application.
    ///
    /// WHY the instance owns this and not the manager: the VM that runs the
    /// application has to carry the same grants, and an instance that knew
    /// its grants and its VM could disagree would be a way to hold a
    /// permission the audit never granted.
    pub fn grant_permissions(&mut self, permissions: Vec<String>) {
        self.vm.set_granted_permissions(permissions.iter().cloned());
        self.granted_permissions = permissions;
    }

    /// Renders the root view hierarchy to a frame buffer surface.
    pub fn render_to_surface(&self, buffer: &mut [u32], buf_w: u32, buf_h: u32) -> usize {
        let Some(root) = &self.root_view else {
            return 0;
        };
        let mut draw_list = DrawList::new();
        root.draw(&mut draw_list);
        render_draw_list_to_buffer(&draw_list, buffer, buf_w, buf_h);
        draw_list.len()
    }
}

/// System-wide Ring-3 ACR application manager.
pub struct AcrManager {
    next_pid: u32,
    pub apps: BTreeMap<u32, AcrAppInstance>,
    pub active_pid: Option<u32>,
}

impl AcrManager {
    pub const fn new() -> Self {
        Self {
            next_pid: 1000,
            apps: BTreeMap::new(),
            active_pid: None,
        }
    }

    /// Verifies, installs, and launches an Android APK in an isolated Ring-3 domain.
    pub fn install_and_launch(
        &mut self,
        apk_bytes: &[u8],
        surface_id: u32,
        memory_quota_bytes: usize,
    ) -> Result<u32, AcrError> {
        self.install_and_launch_with_task_id(apk_bytes, surface_id, memory_quota_bytes, None)
    }

    /// Verifies, installs, and launches an Android APK with a kernel-assigned TaskId.
    pub fn install_and_launch_with_task_id(
        &mut self,
        apk_bytes: &[u8],
        surface_id: u32,
        memory_quota_bytes: usize,
        assigned_task_id: Option<u64>,
    ) -> Result<u32, AcrError> {
        let apk = ApkPackage::parse(apk_bytes).map_err(|_| AcrError::ApkParseFailed)?;
        if !apk.is_signed {
            return Err(AcrError::SignatureVerificationFailed);
        }

        // WHY the identity is checked before the audit: the package name is
        // the application's identity, and it becomes the directory the
        // application's private files live in. A name that cannot be an
        // identity is refused with that reason, rather than with the audit's
        // general refusal, and it never reaches the point where a directory
        // is derived from it.
        if !is_valid_package_name(&apk.manifest.package_name) {
            return Err(AcrError::InvalidPackageName);
        }

        let profile = apk.audit_and_create_container(self.next_pid as u64);
        if profile.status != AcrAuditStatus::Approved {
            return Err(AcrError::AuditRejected);
        }

        // WHY the identity is checked again here: the audit refuses an
        // unusable name, but this is the place the name becomes a directory
        // and a uid, so the refusal is repeated where it matters and the
        // reason is named.
        if !is_valid_package_name(&profile.package_name) {
            return Err(AcrError::InvalidPackageName);
        }

        if !apk.has_classes_dex || apk.classes_dex.is_empty() {
            return Err(AcrError::DexNotFound);
        }
        let dex_bytes = apk.classes_dex;
        let parsed_dex = DexPackage::parse(&dex_bytes).map_err(|_| AcrError::DexParseFailed)?;
        parsed_dex.audit_bytecode().map_err(|_| AcrError::DexParseFailed)?;

        let pid = self.next_pid;
        self.next_pid += 1;

        // Use real kernel-assigned TaskId when available
        let task_id = assigned_task_id.unwrap_or(pid as u64);

        // The activity lifecycle is the application's own code, run at the
        // points Android runs it: onCreate, then onStart, then onResume.
        let mut vm = DalvikVmState::new();
        let main_activity_class = alloc::format!("L{}/MainActivity;", profile.package_name.replace('.', "/"));
        let activity_state = run_activity_lifecycle(&mut vm, &parsed_dex, &main_activity_class)?;

        let mut instance = AcrAppInstance::new(
            pid,
            task_id,
            &profile.package_name,
            profile.target_sdk,
            surface_id,
            memory_quota_bytes,
            dex_bytes,
        );
        instance.vm = vm;
        instance.parsed_dex = Some(parsed_dex);
        instance.activity_state = activity_state;
        // WHY the VM is told what the audit granted and which directory the
        // application owns: both come from the platform's own audit of the
        // manifest, never from the application. The VM refuses a service the
        // application did not ask for, and answers `getFilesDir` with this
        // application's own directory.
        instance.grant_permissions(profile.granted_permissions.clone());
        instance.vm.set_private_data_dir(instance.data_dir.clone());

        self.apps.insert(pid, instance);
        self.active_pid = Some(pid);
        Ok(pid)
    }

    /// Whether the audit granted this application the named permission.
    ///
    /// WHY this asks the instance and not the manifest: the manifest says what
    /// the application asked for, and the audit says what it was given. A
    /// permission the audit refused is not held, however loudly the manifest
    /// asked for it.
    pub fn has_permission(&self, pid: u32, permission: &str) -> bool {
        self.apps
            .get(&pid)
            .map(|app| app.granted_permissions.iter().any(|held| held == permission))
            .unwrap_or(false)
    }

    /// Whether the active application holds the named permission.
    pub fn active_has_permission(&self, permission: &str) -> bool {
        self.active_pid
            .map(|pid| self.has_permission(pid, permission))
            .unwrap_or(false)
    }

    /// Dispatches a touch event to the active application's View hierarchy.
    ///
    /// WHY the callback's failure is not swallowed here: the application's
    /// click handler is real DEX bytecode, and a handler that threw did not
    /// run to completion. [`AcrManager::dispatch_touch_checked`] reports that
    /// failure; this entry point keeps the touch's own answer.
    pub fn dispatch_touch(&mut self, pid: u32, event: &MotionEvent) -> bool {
        match self.dispatch_touch_checked(pid, event) {
            Ok(consumed) => consumed,
            // The view took the touch and the application's own handler then
            // failed, so the touch was consumed either way.
            Err(AcrError::CallbackExecutionFailed) => true,
            Err(_) => false,
        }
    }

    /// Dispatches a touch and reports what happened to the application's own
    /// callback.
    ///
    /// WHY this exists beside [`AcrManager::dispatch_touch`]: a callback
    /// that fails is a fact the caller needs. Reporting the touch as
    /// consumed with nothing said about the handler that threw would tell
    /// the caller the application did something it did not do.
    pub fn dispatch_touch_checked(&mut self, pid: u32, event: &MotionEvent) -> Result<bool, AcrError> {
        let Some(app) = self.apps.get_mut(&pid) else {
            return Err(AcrError::AppNotFound);
        };
        let consumed = match app.root_view.as_mut() {
            Some(root) => root.dispatch_touch_event(event),
            None => false,
        };
        // Execute the application-defined click callback (a real DEX method),
        // not a fabricated label, through the VM.
        let pending = app
            .root_view
            .as_mut()
            .and_then(|root| root.take_pending_click());
        if let Some(method_index) = pending {
            let dex = app.parsed_dex.as_ref();
            let vm = &mut app.vm;
            let Some(dex) = dex else {
                return Err(AcrError::CallbackExecutionFailed);
            };
            let Some(m) = dex.methods.iter().find(|m| m.method_index as u32 == method_index) else {
                // The view named a callback the application's own DEX does
                // not hold. The click was delivered to a handler that cannot
                // be found, which is a failure, not a no-op.
                return Err(AcrError::CallbackExecutionFailed);
            };
            let class_name = m.class_name.clone();
            let method_name = m.method_name.clone();
            vm.invoke_method(dex, &class_name, &method_name)
                .map_err(|_| AcrError::CallbackExecutionFailed)?;
        }
        Ok(consumed)
    }

    /// Drives a Choreographer vsync tick and Looper message processing for an application.
    pub fn tick_frame(&mut self, pid: u32, frame_time_nanos: u64, current_ticks: u64) -> usize {
        let Some(app) = self.apps.get_mut(&pid) else {
            return 0;
        };
        let messages_processed = app.looper.loop_once(current_ticks);
        let callbacks_run = app.choreographer.do_frame(frame_time_nanos);
        messages_processed + callbacks_run
    }

    /// Stops and terminates an application domain.
    ///
    /// WHY the active application falls back to another running one: the
    /// manager used to leave no active application at all when the active
    /// one was terminated, so a second application that was still running
    /// could never be shown again. The fallback keeps the screen attached to
    /// an application that is actually there.
    pub fn terminate_app(&mut self, pid: u32) -> Option<AcrAppInstance> {
        if self.active_pid == Some(pid) {
            self.active_pid = self.apps.keys().find(|other| **other != pid).copied();
        }
        self.apps.remove(&pid)
    }

    /// Brings a running application back to the front.
    ///
    /// WHY this exists: terminating an application and launching it again is
    /// a process recreation -- the new process has a new pid and a new VM,
    /// while the application's private directory is the same one, because it
    /// is made from the package name alone. Selecting the running instance
    /// again is the other half of that story, and the manager had no way to
    /// ask for it.
    pub fn activate_app(&mut self, pid: u32) -> bool {
        if !self.apps.contains_key(&pid) {
            return false;
        }
        self.active_pid = Some(pid);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use interos_android_format::apk::create_signed_test_apk;
    use crate::view::View;

    #[test]
    fn test_acr_manager_lifecycle_and_rendering() {
        let mut manager = AcrManager::new();
        let apk_bytes = create_signed_test_apk("org.whispersystems.signal", &["android.permission.INTERNET"]);

        let pid = manager.install_and_launch(&apk_bytes, 1, 1024 * 1024).expect("install and launch");
        assert_eq!(pid, 1000);
        assert_eq!(manager.active_pid, Some(1000));

        let app = manager.apps.get_mut(&pid).expect("app instance");
        assert_eq!(app.activity_state, ActivityLifecycleState::Resumed);
        assert_eq!(app.package_name, "org.whispersystems.signal");
        assert_eq!(app.uid, 10000);
        assert_eq!(app.task_id, 1000);
        assert_eq!(app.data_dir, "/data/data/org.whispersystems.signal");

        // Attach a root View with a button and text view
        let mut root = View::linear_layout(1, true);
        root.background_color = Some(0xFF000000);
        let tv = View::text_view(2, "Signal on InterOS", 14, 0xFFFFFFFF);
        let btn = View::button(3, "Send", 14, 0xFF00FF00);
        root.add_child(tv);
        root.add_child(btn);

        root.measure(
            crate::view::MeasureSpec::make(360, crate::view::MeasureMode::Exactly),
            crate::view::MeasureSpec::make(640, crate::view::MeasureMode::Exactly),
        );
        root.layout(0, 0, 360, 640);
        app.root_view = Some(root);

        // Render to surface buffer
        let mut buffer = alloc::vec![0u32; 360 * 640];
        let ops = app.render_to_surface(&mut buffer, 360, 640);
        assert!(ops > 0, "Must have generated draw operations");

        // Tick frame and looper
        let processed = manager.tick_frame(pid, 16_666_666, 100);
        assert_eq!(processed, 0); // No pending messages or callbacks yet

        // Post a message and frame callback
        let app = manager.apps.get_mut(&pid).unwrap();
        app.looper.post(1, 100);
        app.choreographer.post_frame_callback(42);

        let processed2 = manager.tick_frame(pid, 33_333_333, 100);
        assert_eq!(processed2, 2); // 1 message + 1 frame callback

        // Terminate app
        let terminated = manager.terminate_app(pid);
        assert!(terminated.is_some());
        assert_eq!(manager.active_pid, None);
        assert!(manager.apps.is_empty());
    }

    /// The activity lifecycle runs the application's own code at each step.
    ///
    /// WHY this matters: the manager used to run only `onCreate` and then
    /// declare the activity resumed, so the code an application puts in
    /// `onStart` and `onResume` never ran at all. A DEX whose `onResume`
    /// carries no code cannot be run to completion, so the launch has to
    /// fail rather than report a resumed activity.
    #[test]
    fn the_lifecycle_runs_every_step_the_application_defines() {
        let package = "org.example.lifecycle";
        let main_class = alloc::format!("L{}/MainActivity;", package.replace('.', "/"));

        // Every step defined and runnable: the activity reaches resumed.
        let dex = interos_android_format::dex::create_lifecycle_test_dex(&main_class, None);
        let apk = interos_android_format::apk::create_signed_test_apk_with_dex(package, &[], &dex);
        let mut manager = AcrManager::new();
        let pid = manager
            .install_and_launch(&apk, 1, 1024 * 1024)
            .expect("a lifecycle DEX whose steps all run launches");
        assert_eq!(
            manager.apps.get(&pid).expect("app").activity_state,
            ActivityLifecycleState::Resumed
        );

        // `onStart` carries no code: running it fails, so the launch fails.
        let dex = interos_android_format::dex::create_lifecycle_test_dex(&main_class, Some("onStart"));
        let apk = interos_android_format::apk::create_signed_test_apk_with_dex(package, &[], &dex);
        let mut manager = AcrManager::new();
        assert_eq!(
            manager.install_and_launch(&apk, 1, 1024 * 1024),
            Err(AcrError::MethodExecutionFailed),
            "a lifecycle step that cannot run must fail the launch"
        );
        assert!(manager.apps.is_empty(), "a failed launch leaves no application running");

        // `onResume` carries no code: the activity never reaches resumed.
        let dex = interos_android_format::dex::create_lifecycle_test_dex(&main_class, Some("onResume"));
        let apk = interos_android_format::apk::create_signed_test_apk_with_dex(package, &[], &dex);
        let mut manager = AcrManager::new();
        assert_eq!(
            manager.install_and_launch(&apk, 1, 1024 * 1024),
            Err(AcrError::MethodExecutionFailed),
            "a lifecycle step that cannot run must fail the launch"
        );
    }

    /// A package name that is not an application identity is refused.
    ///
    /// WHY this matters: the name becomes the directory the application's
    /// private files live in. A name carrying a parent reference would put
    /// that directory outside the application's own sandbox.
    #[test]
    fn a_package_name_that_cannot_be_an_identity_is_refused() {
        let dex = interos_android_format::dex::create_lifecycle_test_dex(
            "Lorg/example/evil/MainActivity;",
            None,
        );
        let apk = interos_android_format::apk::create_signed_test_apk_with_dex(
            "../evil",
            &[],
            &dex,
        );
        let mut manager = AcrManager::new();
        assert_eq!(
            manager.install_and_launch(&apk, 1, 1024 * 1024),
            Err(AcrError::InvalidPackageName)
        );
        assert!(manager.apps.is_empty());
    }

    /// A click callback that cannot run is reported, not swallowed.
    ///
    /// WHY this matters: the application's click handler is real bytecode.
    /// A handler that throws did not do what the touch asked, and the caller
    /// has to hear about it.
    #[test]
    fn a_click_callback_that_fails_is_reported() {
        let package = "org.example.callback";
        let main_class = alloc::format!("L{}/MainActivity;", package.replace('.', "/"));
        let dex = interos_android_format::dex::create_lifecycle_test_dex(&main_class, None);
        let apk = interos_android_format::apk::create_signed_test_apk_with_dex(package, &[], &dex);
        let mut manager = AcrManager::new();
        let pid = manager.install_and_launch(&apk, 1, 1024 * 1024).expect("launch");

        // A button whose callback names a method the DEX does not hold.
        let mut btn = View::button(2, "Send", 14, 0xFF00FF00);
        btn.set_on_click_callback(4242);
        btn.layout_width = crate::view::LayoutDimension::MatchParent;
        btn.layout_height = crate::view::LayoutDimension::MatchParent;
        btn.measure(
            crate::view::MeasureSpec::make(360, crate::view::MeasureMode::Exactly),
            crate::view::MeasureSpec::make(640, crate::view::MeasureMode::Exactly),
        );
        btn.layout(0, 0, 360, 640);
        manager.apps.get_mut(&pid).expect("app").root_view = Some(btn);

        // A touch is pressed and released: the release is what hands the
        // application's own callback to the runtime.
        let down = MotionEvent {
            action: crate::view::MotionAction::Down,
            x: 10,
            y: 10,
            timestamp_ms: 0,
        };
        let up = MotionEvent {
            action: crate::view::MotionAction::Up,
            x: 10,
            y: 10,
            timestamp_ms: 16,
        };
        assert!(manager.dispatch_touch_checked(pid, &down).expect("the press is delivered"));
        assert_eq!(
            manager.dispatch_touch_checked(pid, &up),
            Err(AcrError::CallbackExecutionFailed),
            "a callback the DEX does not hold must be reported"
        );

        // The plain entry point keeps the touch's own answer.
        assert!(manager.dispatch_touch(pid, &up));
    }

    /// A terminated application can be created again with its own files.
    ///
    /// WHY this matters: a process that is killed and started again is a new
    /// process -- a new pid and a new virtual machine -- but it is the same
    /// application, so its private directory is the same directory.
    #[test]
    fn a_recreated_process_keeps_its_private_directory_and_gets_a_fresh_one() {
        let package = "org.example.recreated";
        let main_class = alloc::format!("L{}/MainActivity;", package.replace('.', "/"));
        let dex = interos_android_format::dex::create_lifecycle_test_dex(&main_class, None);
        let apk = interos_android_format::apk::create_signed_test_apk_with_dex(package, &[], &dex);
        let mut manager = AcrManager::new();

        let first = manager.install_and_launch(&apk, 1, 1024 * 1024).expect("first launch");
        let first_dir = manager.apps.get(&first).expect("app").data_dir.clone();
        assert_eq!(first_dir, "/data/data/org.example.recreated");

        // Another application is running when the first one is terminated.
        let other = manager.install_and_launch(&apk, 2, 1024 * 1024).expect("second launch");
        assert_ne!(first, other);
        assert_eq!(manager.active_pid, Some(other));

        manager.terminate_app(first);
        assert_eq!(
            manager.active_pid,
            Some(other),
            "the screen stays attached to an application that is still running"
        );

        // The application is created again: a new pid, the same directory.
        let again = manager.install_and_launch(&apk, 1, 1024 * 1024).expect("relaunch");
        assert_ne!(again, first);
        assert_eq!(manager.apps.get(&again).expect("app").data_dir, first_dir);
        assert_eq!(manager.apps.get(&again).expect("app").pid, again);

        // A running application can be brought back to the front.
        assert!(manager.activate_app(other));
        assert_eq!(manager.active_pid, Some(other));
        assert!(!manager.activate_app(9999));
    }

    /// A service answers only an application that asked for it.
    ///
    /// WHY this matters: the audit grants permissions from the manifest, and
    /// a service that answers an application that never asked for it is an
    /// ambient authority -- the application would reach a device capability
    /// without having declared it.
    #[test]
    fn a_service_answers_only_an_application_that_asked_for_it() {
        let package = "org.example.permissions";
        let main_class = alloc::format!("L{}/MainActivity;", package.replace('.', "/"));
        let dex = interos_android_format::dex::create_lifecycle_test_dex(&main_class, None);

        let with_permission = interos_android_format::apk::create_signed_test_apk_with_dex(
            package,
            &["android.permission.ACCESS_FINE_LOCATION"],
            &dex,
        );
        let without_permission =
            interos_android_format::apk::create_signed_test_apk_with_dex(package, &[], &dex);

        let mut manager = AcrManager::new();
        let asking = manager.install_and_launch(&with_permission, 1, 1024 * 1024).expect("launch");
        let asking_dex = interos_android_format::dex::DexPackage::parse(&dex).expect("the dex the application runs");

        assert!(
            manager.has_permission(asking, "android.permission.ACCESS_FINE_LOCATION"),
            "the audit granted the permission the manifest declared"
        );
        assert!(
            !manager.has_permission(asking, "android.permission.CAMERA"),
            "a permission the manifest never declared is not held"
        );

        // The application that asked is answered: this device has no location
        // provider, so the honest answer is "no provider", not a refusal.
        let asked = manager.apps.get_mut(&asking).expect("app").vm.dispatch_framework_method(
            &asking_dex,
            "Landroid/location/LocationManager;",
            "isProviderEnabled",
            &[],
            &[],
        );
        assert_eq!(asked, Ok(0), "an application that asked is answered");

        // The application that did not ask is refused, and the refusal is a
        // SecurityException the application can see.
        let silent = manager.install_and_launch(&without_permission, 2, 1024 * 1024).expect("launch");
        let silent_dex = interos_android_format::dex::DexPackage::parse(&dex).expect("the dex the application runs");
        assert!(!manager.has_permission(silent, "android.permission.ACCESS_FINE_LOCATION"));
        assert!(!manager.active_has_permission("android.permission.ACCESS_FINE_LOCATION"));

        let refused = manager.apps.get_mut(&silent).expect("app").vm.dispatch_framework_method(
            &silent_dex,
            "Landroid/location/LocationManager;",
            "isProviderEnabled",
            &[],
            &[],
        );
        assert!(
            matches!(refused, Err(DalvikVmError::UnhandledException(_))),
            "an application that did not ask is refused, and not answered"
        );
    }

    /// An application is told where its own files live.
    ///
    /// WHY this matters: a path shared by every application puts one
    /// application's files where another application can reach them.
    #[test]
    fn an_application_is_told_where_its_own_files_live() {
        let package = "org.example.privatefiles";
        let main_class = alloc::format!("L{}/MainActivity;", package.replace('.', "/"));
        let dex = interos_android_format::dex::create_lifecycle_test_dex(&main_class, None);
        let apk = interos_android_format::apk::create_signed_test_apk_with_dex(package, &[], &dex);
        let mut manager = AcrManager::new();

        let pid = manager.install_and_launch(&apk, 1, 1024 * 1024).expect("launch");
        let app = manager.apps.get_mut(&pid).expect("app");
        assert_eq!(app.vm.private_data_dir, "/data/data/org.example.privatefiles");
        assert_eq!(app.data_dir, app.vm.private_data_dir);
    }
}
