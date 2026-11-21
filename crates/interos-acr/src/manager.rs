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
use interos_android_format::apk::{AcrAuditStatus, ApkPackage};
use interos_android_format::dex::DexPackage;
use crate::runtime::DalvikVmState;
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
    pub data_dir: String,
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
        let data_dir = alloc::format!("/data/data/{}", package_name);
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
            data_dir,
        }
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

        let profile = apk.audit_and_create_container(self.next_pid as u64);
        if profile.status != AcrAuditStatus::Approved {
            return Err(AcrError::AuditRejected);
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

        // Execute MainActivity->onCreate if found
        let mut vm = DalvikVmState::new();
        let main_activity_class = alloc::format!("L{}/MainActivity;", profile.package_name.replace('.', "/"));
        if parsed_dex.methods.iter().any(|m| m.class_name == main_activity_class && m.method_name == "onCreate") {
            let _ = vm.invoke_method(&parsed_dex, &main_activity_class, "onCreate")
                .map_err(|_| AcrError::MethodExecutionFailed)?;
        }

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
        instance.activity_state = ActivityLifecycleState::Resumed;

        self.apps.insert(pid, instance);
        self.active_pid = Some(pid);
        Ok(pid)
    }

    /// Dispatches a touch event to the active application's View hierarchy.
    pub fn dispatch_touch(&mut self, pid: u32, event: &MotionEvent) -> bool {
        if let Some(app) = self.apps.get_mut(&pid) {
            if let Some(root) = &mut app.root_view {
                return root.dispatch_touch_event(event);
            }
        }
        false
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
    pub fn terminate_app(&mut self, pid: u32) -> Option<AcrAppInstance> {
        if self.active_pid == Some(pid) {
            self.active_pid = None;
        }
        self.apps.remove(&pid)
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
}
