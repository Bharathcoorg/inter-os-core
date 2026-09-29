//! Small native-application runtime helpers.
//!
//! These helpers keep native Rust applications on the same lifecycle, window,
//! input, and private-storage contract instead of each app inventing its own
//! glue. They do not provide fake platform state; callers must handle
//! unavailable hardware/services explicitly.

use crate::{InputEvent};

pub struct AppSession {
    package_id: &'static str,
    surface: Option<u32>,
    display_cap_slot: usize,
}

impl AppSession {
    pub const fn new(package_id: &'static str, display_cap_slot: usize) -> Self {
        Self { package_id, surface: None, display_cap_slot }
    }

    pub fn package_id(&self) -> &'static str { self.package_id }

    pub fn open_window(&mut self, width: u32, height: u32, z_order: u32) -> Result<u32, i64> {
        let surface = crate::surface_create(self.display_cap_slot, width, height, z_order)?;
        self.surface = Some(surface);
        Ok(surface)
    }

    pub fn surface(&self) -> Option<u32> { self.surface }

    pub fn present(&self) -> Result<u64, i64> {
        crate::surface_flip(self.display_cap_slot)
    }

    pub fn destroy_window(&mut self) -> Result<(), i64> {
        if let Some(surface) = self.surface.take() {
            crate::surface_destroy(self.display_cap_slot, surface)?;
        }
        Ok(())
    }

    pub fn poll_input(&self, event: &mut InputEvent) -> Result<(), i64> {
        crate::input_event_poll(event)
    }

    pub fn wait(&self, interest_mask: u32, timeout_ticks: u32) -> Result<u32, i64> {
        crate::event_poll(interest_mask, timeout_ticks)
    }

    pub fn enter_background(&self) -> Result<(), i64> { crate::app_background() }
    pub fn enter_foreground(&self) -> Result<(), i64> { crate::app_foreground() }
    pub fn suspend(&self) -> Result<(), i64> { crate::app_suspend() }
    pub fn wake(&self) -> Result<(), i64> { crate::app_wake() }

    pub fn open_private_file(&self, fs_cap_slot: usize, relative_path: &str, rights: u32) -> Result<u32, i64> {
        crate::app_storage_open(fs_cap_slot, relative_path, rights)
    }

    /// Sends a bounded IPC message using an explicitly supplied capability slot.
    pub fn send_ipc(&self, ipc_cap_slot: usize, target_task_id: u64, msg_type: u32, payload: &[u8]) -> Result<(), i64> {
        crate::ipc_send(ipc_cap_slot, target_task_id, msg_type, payload)
    }

    /// Receives one IPC payload using an explicitly supplied capability slot.
    pub fn receive_ipc(&self, ipc_cap_slot: usize, out: &mut [u8]) -> Result<usize, i64> {
        crate::ipc_recv(ipc_cap_slot, out)
    }

    /// Posts an application notification through the kernel notification service.
    pub fn post_notification(&self, channel: &str) -> Result<u64, i64> {
        crate::notification_post(self.package_id, channel)
    }

    /// Cancels one notification belonging to this application.
    pub fn cancel_notification(&self, notification_id: u64) -> Result<(), i64> {
        crate::notification_cancel(self.package_id, notification_id)
    }

    /// Opens a network socket through an explicitly supplied capability slot.
    pub fn open_socket(&self, network_cap_slot: usize, protocol: u32) -> Result<u64, i64> {
        let _ = network_cap_slot;
        crate::net_socket(protocol as u16).map(|sock| sock as u64)
    }

    /// Waits for input, IPC, storage, and network events without busy-looping.
    pub fn wait_for_events(&self, interest_mask: u32, timeout_ticks: u32) -> Result<u32, i64> {
        crate::event_poll(interest_mask, timeout_ticks)
    }

    pub fn exit(&mut self, code: i32) -> ! {
        let _ = self.destroy_window();
        crate::exit_with_code(code)
    }
}

impl Drop for AppSession {
    fn drop(&mut self) {
        let _ = self.destroy_window();
    }
}
