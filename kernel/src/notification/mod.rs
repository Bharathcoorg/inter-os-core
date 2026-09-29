//! InterOS Sovereign Notification Subsystem
//!
//! Provides notification channels, priorities, grouping, lockscreen visibility policies,
//! per-app notification controls, Do Not Disturb (DND) filtering, and package-bound ownership.

use alloc::collections::BTreeMap;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use spin::Mutex;

/// Importance level of a notification channel.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum NotificationImportance {
    Min = 0,
    Low = 1,
    Default = 2,
    High = 3,
    Urgent = 4,
}

/// Delivery priority of a notification.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum NotificationPriority {
    Min = 0,
    Low = 1,
    Default = 2,
    High = 3,
    Urgent = 4,
}

/// Visibility of a notification on the lock screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LockscreenVisibility {
    /// Show full content on lock screen.
    Public,
    /// Show notification header and icon, but redact private content text.
    Private,
    /// Do not show this notification on the lock screen at all.
    Secret,
}

/// Registered notification channel configuration.
#[derive(Debug, Clone)]
pub struct NotificationChannel {
    pub id: String,
    pub name: String,
    pub importance: NotificationImportance,
    pub sound: bool,
    pub vibration: bool,
    pub dnd_bypass: bool,
}

/// Dispatched notification record.
#[derive(Debug, Clone)]
pub struct NotificationRecord {
    pub id: u64,
    pub package_id: String,
    pub user_id: u32,
    pub channel_id: String,
    pub title: String,
    pub body: String,
    pub priority: NotificationPriority,
    pub visibility: LockscreenVisibility,
    pub badge_count: u32,
    pub timestamp_ticks: u64,
    pub is_cancelled: bool,
}

/// Errors returned by the notification subsystem.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotificationError {
    PermissionDenied,
    ChannelNotFound,
    NotificationNotFound,
    MaxNotificationsExceeded,
    DndSuppressed,
    AppBlocked,
}

/// Notification manager state engine.
pub struct NotificationManager {
    channels: BTreeMap<String, NotificationChannel>,
    records: Vec<NotificationRecord>,
    next_id: u64,
    dnd_enabled: bool,
    app_blocks: BTreeMap<String, bool>,
    max_active: usize,
}

impl NotificationManager {
    pub const fn new() -> Self {
        Self {
            channels: BTreeMap::new(),
            records: Vec::new(),
            next_id: 1,
            dnd_enabled: false,
            app_blocks: BTreeMap::new(),
            max_active: 64,
        }
    }

    /// Registers standard system notification channels.
    pub fn seed_standard_channels(&mut self) {
        self.register_channel(NotificationChannel {
            id: "system".to_string(),
            name: "System Alerts".to_string(),
            importance: NotificationImportance::Urgent,
            sound: true,
            vibration: true,
            dnd_bypass: true,
        });

        self.register_channel(NotificationChannel {
            id: "calls".to_string(),
            name: "Incoming Calls".to_string(),
            importance: NotificationImportance::Urgent,
            sound: true,
            vibration: true,
            dnd_bypass: true,
        });

        self.register_channel(NotificationChannel {
            id: "messages".to_string(),
            name: "Messages & Chats".to_string(),
            importance: NotificationImportance::High,
            sound: true,
            vibration: true,
            dnd_bypass: false,
        });

        self.register_channel(NotificationChannel {
            id: "general".to_string(),
            name: "General Notifications".to_string(),
            importance: NotificationImportance::Default,
            sound: false,
            vibration: false,
            dnd_bypass: false,
        });
    }

    /// Registers a new notification channel.
    pub fn register_channel(&mut self, channel: NotificationChannel) {
        self.channels.insert(channel.id.clone(), channel);
    }

    /// Posts a new notification bound to a package identity.
    pub fn post(
        &mut self,
        package_id: &str,
        user_id: u32,
        channel_id: &str,
        title: &str,
        body: &str,
        priority: NotificationPriority,
        visibility: LockscreenVisibility,
        badge_count: u32,
        current_tick: u64,
    ) -> Result<u64, NotificationError> {
        // 1. Check if package notifications are blocked
        if let Some(&blocked) = self.app_blocks.get(package_id) {
            if blocked {
                return Err(NotificationError::AppBlocked);
            }
        }

        // 2. Validate channel exists
        let channel = self
            .channels
            .get(channel_id)
            .ok_or(NotificationError::ChannelNotFound)?;

        // 3. Check DND filter
        if self.dnd_enabled && !channel.dnd_bypass && priority < NotificationPriority::Urgent {
            return Err(NotificationError::DndSuppressed);
        }

        // 4. Check active capacity limit
        let active_count = self.records.iter().filter(|r| !r.is_cancelled).count();
        if active_count >= self.max_active {
            return Err(NotificationError::MaxNotificationsExceeded);
        }

        let id = self.next_id;
        self.next_id = self.next_id.saturating_add(1);

        let record = NotificationRecord {
            id,
            package_id: package_id.to_string(),
            user_id,
            channel_id: channel_id.to_string(),
            title: title.to_string(),
            body: body.to_string(),
            priority,
            visibility,
            badge_count,
            timestamp_ticks: current_tick,
            is_cancelled: false,
        };

        self.records.push(record);
        if let Some(task) = crate::task::SCHEDULER.lock().get_task(crate::task::SCHEDULER.lock().current_task_id().unwrap_or(crate::task::TaskId(0))) {
            let _ = task.id;
        }
        // A suspended package is eligible for notification-driven wakeup. The
        // scheduler still enforces its background CPU quota after delivery.
        let _ = crate::task::process::wake_package(package_id, user_id);
        crate::serial_println!(
            "[NOTIFICATION] Posted #{} from '{}' on channel '{}' (priority={:?})",
            id,
            package_id,
            channel_id,
            priority
        );
        Ok(id)
    }

    /// Cancels an active notification.
    pub fn cancel(&mut self, package_id: &str, id: u64) -> Result<(), NotificationError> {
        let record = self
            .records
            .iter_mut()
            .find(|r| r.id == id && r.package_id == package_id && !r.is_cancelled)
            .ok_or(NotificationError::NotificationNotFound)?;

        record.is_cancelled = true;
        crate::serial_println!("[NOTIFICATION] Cancelled #{} for '{}'", id, package_id);
        Ok(())
    }

    /// Cancels all active notifications for a package.
    pub fn cancel_all_for_package(&mut self, package_id: &str, user_id: u32) {
        for record in self.records.iter_mut() {
            if record.package_id == package_id && record.user_id == user_id && !record.is_cancelled {
                record.is_cancelled = true;
            }
        }
    }

    /// Queries active notifications filtered for the target user and lockscreen state.
    pub fn query_active(&self, user_id: u32, is_locked: bool) -> Vec<NotificationRecord> {
        self.records
            .iter()
            .filter(|r| !r.is_cancelled && r.user_id == user_id)
            .filter_map(|r| {
                if is_locked {
                    match r.visibility {
                        LockscreenVisibility::Public => Some(r.clone()),
                        LockscreenVisibility::Private => {
                            let mut redacted = r.clone();
                            redacted.body = "[Private Content]".to_string();
                            Some(redacted)
                        }
                        LockscreenVisibility::Secret => None,
                    }
                } else {
                    Some(r.clone())
                }
            })
            .collect()
    }

    /// Sets global Do Not Disturb (DND) state.
    pub fn set_dnd_enabled(&mut self, enabled: bool) {
        self.dnd_enabled = enabled;
        crate::serial_println!("[NOTIFICATION] Do Not Disturb set to: {}", enabled);
    }

    /// Sets whether notifications from a package are blocked.
    pub fn set_app_blocked(&mut self, package_id: &str, blocked: bool) {
        self.app_blocks.insert(package_id.to_string(), blocked);
    }

    /// Returns total active unread badge count for a package.
    pub fn get_badge_count(&self, package_id: &str, user_id: u32) -> u32 {
        self.records
            .iter()
            .filter(|r| !r.is_cancelled && r.package_id == package_id && r.user_id == user_id)
            .map(|r| r.badge_count)
            .max()
            .unwrap_or(0)
    }
}

pub static NOTIFICATION_MANAGER: Mutex<NotificationManager> = Mutex::new(NotificationManager::new());

/// Boot-time initialization of notification subsystem.
pub fn init() {
    let mut nm = NOTIFICATION_MANAGER.lock();
    nm.seed_standard_channels();
    crate::serial_println!("[OK] Sovereign Notification Subsystem initialized.");
}
