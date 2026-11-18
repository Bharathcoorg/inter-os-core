// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

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
#[repr(u8)]
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
    RateLimited,
}

/// Notification manager state engine.
pub struct NotificationManager {
    channels: BTreeMap<String, NotificationChannel>,
    records: Vec<NotificationRecord>,
    next_id: u64,
    dnd_enabled: bool,
    app_blocks: BTreeMap<String, bool>,
    max_active: usize,
    // Per-package notification flood control. The broker remains authoritative;
    // package identity is already verified at the syscall boundary.
    rate_state: BTreeMap<String, (u64, u16)>,
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
            rate_state: BTreeMap::new(),
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

        // 2. Bound notification floods without imposing a global device-wide
        // rate limit. A package gets 20 posts per 1000 ticks; the window is
        // anchored to the first accepted post and resets after it expires.
        const RATE_WINDOW_TICKS: u64 = 1000;
        const RATE_LIMIT: u16 = 20;
        let rate = self.rate_state.entry(package_id.to_string()).or_insert((current_tick, 0));
        if current_tick.saturating_sub(rate.0) >= RATE_WINDOW_TICKS {
            *rate = (current_tick, 0);
        }
        if rate.1 >= RATE_LIMIT {
            return Err(NotificationError::RateLimited);
        }

        // 3. Validate channel exists
        let channel = self
            .channels
            .get(channel_id)
            .ok_or(NotificationError::ChannelNotFound)?;

        // 4. Check DND filter
        if self.dnd_enabled && !channel.dnd_bypass && priority < NotificationPriority::Urgent {
            return Err(NotificationError::DndSuppressed);
        }

        // 5. Check active capacity limit
        if self.records.len() >= self.max_active {
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
        };

        self.records.push(record);
        if let Some(rate) = self.rate_state.get_mut(package_id) {
            rate.1 = rate.1.saturating_add(1);
        }
        // A suspended package is eligible for notification-driven wakeup. The
        // scheduler still enforces its background CPU quota after delivery.
        let _ = crate::task::process::wake_package(package_id, user_id);
        crate::serial_println!(
            "[NOTIFICATION] Posted #{} from '{}' on channel '{}' for user {} (priority={:?})",
            id,
            package_id,
            channel_id,
            user_id,
            priority
        );
        Ok(id)
    }

    /// Cancels an active notification.
    ///
    /// WHY the record is dropped rather than marked: marking was the only way to
    /// cancel, and nothing ever read a marked record back -- every query already
    /// filtered them out -- so a device that posted and cleared notifications for a
    /// week kept every one it had ever posted, four heap strings each, in a Vec the
    /// shade had stopped showing. max_active bounded what the person could see, never
    /// what the kernel held, and each remaining reader paid for the whole trail on
    /// every call.
    pub fn cancel(&mut self, package_id: &str, id: u64) -> Result<(), NotificationError> {
        let before = self.records.len();
        self.records.retain(|r| {
            !(r.id == id && (package_id.is_empty() || r.package_id == package_id))
        });
        if self.records.len() == before {
            return Err(NotificationError::NotificationNotFound);
        }
        crate::serial_println!("[NOTIFICATION] Cancelled #{} for '{}'", id, package_id);
        Ok(())
    }

    /// Cancels all notifications a package owns for one account.
    ///
    /// WHY `user_id` must be the established owner and never a default: records
    /// are filed per account, so a caller that guesses the uid destroys a
    /// different account's notifications -- the task-exit path once guessed 0
    /// when the owner could not be resolved. A caller that cannot establish the
    /// owner must cancel nothing and surface the failure instead; see
    /// `task::scheduler::reclaim_task`, which carries the uid captured at
    /// termination for exactly this reason.
    pub fn cancel_all_for_package(&mut self, package_id: &str, user_id: u32) {
        self.records
            .retain(|r| !(r.package_id == package_id && r.user_id == user_id));
    }

    /// Removes every notification owned by a package across all users. Package
    /// removal must not leave stale badges or lock-screen entries behind.
    pub fn cancel_all_for_package_all_users(&mut self, package_id: &str) {
        self.records.retain(|r| r.package_id != package_id);
    }

    /// Cancels all notifications for a user.
    pub fn cancel_all_for_user(&mut self, user_id: u32) {
        self.records.retain(|r| r.user_id != user_id);
        crate::serial_println!("[NOTIFICATION] Cancelled all notifications for user {}", user_id);
    }

    /// Queries active notifications filtered for the target user and lockscreen state.
    pub fn query_active(&self, user_id: u32, is_locked: bool) -> Vec<NotificationRecord> {
        self.records
            .iter()
            .filter(|r| r.user_id == user_id)
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

    /// Drops package-specific notification policy when the package is removed.
    pub fn remove_app_policy(&mut self, package_id: &str) {
        self.app_blocks.remove(package_id);
        self.rate_state.remove(package_id);
    }

    /// Returns total active unread badge count for a package.
    pub fn get_badge_count(&self, package_id: &str, user_id: u32) -> u32 {
        self.records
            .iter()
            .filter(|r| r.package_id == package_id && r.user_id == user_id)
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
    let current_ticks = crate::interrupts::get_system_ticks();
    let _ = nm.post(
        "system",
        crate::user::UserId::OWNER.0,
        "system",
        "InterOS System",
        "Kernel initialized. Sovereign security active.",
        NotificationPriority::Default,
        LockscreenVisibility::Public,
        0,
        current_ticks,
    );
    let _ = nm.post(
        "system",
        0,
        "system",
        "InterOS System",
        "Kernel initialized. Sovereign security active.",
        NotificationPriority::Default,
        LockscreenVisibility::Public,
        0,
        current_ticks,
    );
    crate::serial_println!("[OK] Sovereign Notification Subsystem initialized.");
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Posts inside one rate window: a package may post 20 notifications per 1000
    /// ticks, so `window` is what keeps a loop of posts from being refused for a
    /// reason the test is not about.
    fn post(nm: &mut NotificationManager, package: &str, window: u64, _seq: u64) -> u64 {
        nm.post(
            package,
            1000,
            "system",
            "a title",
            "a body",
            NotificationPriority::Default,
            LockscreenVisibility::Public,
            1,
            window * 1000 + 1,
        )
        .expect("post accepted")
    }

    #[test]
    fn a_cancelled_notification_leaves_nothing_behind() {
        // WHY this test exists: cancellation used to mark the record and keep it, so
        // the shade stopped showing it while the kernel went on holding its four
        // heap strings forever. The store now has to be empty, not merely quiet.
        let mut nm = NotificationManager::new();
        nm.seed_standard_channels();
        let id = post(&mut nm, "interos.calendar", 0, 1);
        assert_eq!(nm.records.len(), 1);
        assert!(nm.cancel("interos.calendar", id).is_ok());
        assert_eq!(nm.records.len(), 0, "a cancelled record is still in the store");
        assert!(nm.query_active(1000, false).is_empty());
        assert_eq!(
            nm.get_badge_count("interos.calendar", 1000),
            0,
            "the badge survived a notification the person dismissed"
        );
        // Cancelling it a second time is not a success, and not a panic either.
        assert_eq!(nm.cancel("interos.calendar", id), Err(NotificationError::NotificationNotFound));
    }

    #[test]
    fn a_week_of_posts_and_clears_does_not_grow_the_store() {
        // WHY this test exists: a notification centre is opened, read and cleared
        // many times a session, and each cycle used to add a record that nothing
        // could ever remove. The bound has to be the memory, not the screen.
        let mut nm = NotificationManager::new();
        nm.seed_standard_channels();
        for cycle in 0..200u64 {
            let id = post(&mut nm, "interos.messages", cycle / 20, cycle);
            assert!(nm.cancel("interos.messages", id).is_ok());
        }
        assert_eq!(
            nm.records.len(),
            0,
            "{} record(s) survived 200 post-and-clear cycles",
            nm.records.len()
        );
    }

    #[test]
    fn the_active_limit_bounds_the_memory_as_well_as_the_screen() {
        // WHY this test exists: max_active was checked against the count of
        // uncancelled records while cancelled ones were retained, so the number
        // bounded what the person could see and not what the kernel held. With the
        // trail gone the same limit is both, which is what a phone-sized heap needs.
        let mut nm = NotificationManager::new();
        nm.seed_standard_channels();
        for seq in 0..64u64 {
            post(&mut nm, "interos.notes", seq / 20, seq);
        }
        assert_eq!(nm.records.len(), 64);
        assert_eq!(
            nm.post(
                "interos.notes",
                1000,
                "system",
                "one too many",
                "this one is refused",
                NotificationPriority::Default,
                LockscreenVisibility::Public,
                1,
                10_001,
            ),
            Err(NotificationError::MaxNotificationsExceeded)
        );
        assert_eq!(nm.records.len(), 64, "a refused post still entered the store");
        nm.cancel_all_for_user(1000);
        assert!(nm.records.is_empty(), "clearing a user's shade left their records held");
    }

    #[test]
    fn removing_a_package_removes_its_records_not_just_its_badge() {
        // WHY this test exists: package removal already promised to leave no stale
        // badges or lock-screen entries behind, but it only marked the records, so an
        // uninstalled app's history stayed resident for the rest of the session.
        let mut nm = NotificationManager::new();
        nm.seed_standard_channels();
        for seq in 0..3u64 {
            post(&mut nm, "interos.vault", 0, seq);
        }
        post(&mut nm, "interos.notes", 0, 9);
        assert_eq!(nm.records.len(), 4);
        nm.cancel_all_for_package_all_users("interos.vault");
        assert_eq!(nm.records.len(), 1, "an uninstalled package still holds records");
        assert_eq!(nm.records[0].package_id, "interos.notes");
    }

    /// Posts one record of each lock-screen visibility class for `uid`.
    fn post_visibility_classes(nm: &mut NotificationManager, uid: u32) {
        for (pkg, visibility) in [
            ("interos.public", LockscreenVisibility::Public),
            ("interos.private", LockscreenVisibility::Private),
            ("interos.secret", LockscreenVisibility::Secret),
        ] {
            nm.post(
                pkg,
                uid,
                "system",
                "title",
                "sensitive body",
                NotificationPriority::Default,
                visibility,
                1,
                1,
            )
            .expect("post accepted");
        }
    }

    /// Redaction is a function of the lock state passed in, and of nothing else.
    ///
    /// WHY this is pinned here as well as at the syscall: the store is the only
    /// place that decides what a locked reader may see, and the syscall now
    /// supplies the kernel's own lock state instead of the caller's flag. Both
    /// halves have to hold: the store must redact when told the device is
    /// locked, and it must not leak a Secret record or a Private body.
    #[test]
    fn locked_queries_redact_private_and_hide_secret() {
        let mut nm = NotificationManager::new();
        nm.seed_standard_channels();
        post_visibility_classes(&mut nm, 1000);

        let locked = nm.query_active(1000, true);
        assert_eq!(locked.len(), 2, "a Secret record must not reach a locked reader");
        assert!(locked.iter().all(|r| r.visibility != LockscreenVisibility::Secret));
        let private = locked
            .iter()
            .find(|r| r.visibility == LockscreenVisibility::Private)
            .expect("a Private record is shown with its body redacted");
        assert_eq!(private.body, "[Private Content]");
        assert_ne!(private.body, "sensitive body");
        let public = locked
            .iter()
            .find(|r| r.visibility == LockscreenVisibility::Public)
            .expect("a Public record is shown in full");
        assert_eq!(public.body, "sensitive body");

        let unlocked = nm.query_active(1000, false);
        assert_eq!(unlocked.len(), 3);
        assert!(unlocked.iter().any(|r| r.visibility == LockscreenVisibility::Secret));
        assert!(unlocked.iter().all(|r| r.body == "sensitive body"));
    }

    /// Locking again after an unlocked read must redact again.
    ///
    /// WHY: the unlocked answer is a full clone of the record. If a query ever
    /// cached or returned that clone on a later locked read, the private body
    /// would survive the transition -- stale state exposing private content is
    /// the same leak as never redacting at all.
    #[test]
    fn lock_unlock_lock_transitions_never_reuse_an_unlocked_answer() {
        let mut nm = NotificationManager::new();
        nm.seed_standard_channels();
        post_visibility_classes(&mut nm, 1000);

        for _ in 0..3 {
            assert_eq!(nm.query_active(1000, false).len(), 3);
            let locked = nm.query_active(1000, true);
            assert_eq!(locked.len(), 2);
            assert!(locked.iter().all(|r| r.body != "sensitive body"
                || r.visibility == LockscreenVisibility::Public));
        }
        // The store itself was never mutated by a read: the original bodies are
        // still there for the next unlocked query, and only there.
        assert_eq!(nm.records.len(), 3);
        assert!(nm.records.iter().all(|r| r.body == "sensitive body"));
    }

    /// One account's lock state says nothing about another account's records.
    #[test]
    fn a_query_never_returns_another_accounts_records() {
        let mut nm = NotificationManager::new();
        nm.seed_standard_channels();
        post_visibility_classes(&mut nm, 1000);
        post_visibility_classes(&mut nm, 1001);

        assert!(nm.query_active(1000, false).iter().all(|r| r.user_id == 1000));
        assert!(nm.query_active(1001, true).iter().all(|r| r.user_id == 1001));
        assert!(nm.query_active(4242, false).is_empty());
        assert!(nm.query_active(4242, true).is_empty());
    }

    /// Cancelling a package for one account must leave every other account's
    /// records for the same package untouched.
    ///
    /// WHY this is pinned: the task-exit reclaim once passed uid 0 whenever the
    /// real owner could not be resolved, which cancelled user 0's shade for the
    /// package instead of the dead task's own. The store is where that guess
    /// lands, so the scoping has to hold here, not only at the caller.
    #[test]
    fn cancelling_a_package_for_one_account_leaves_the_others_intact() {
        let mut nm = NotificationManager::new();
        nm.seed_standard_channels();
        for uid in [0u32, 1000] {
            nm.post(
                "interos.messages",
                uid,
                "system",
                "title",
                "body",
                NotificationPriority::Default,
                LockscreenVisibility::Public,
                1,
                1,
            )
            .expect("post accepted");
        }
        assert_eq!(nm.records.len(), 2);

        nm.cancel_all_for_package("interos.messages", 1000);

        assert_eq!(nm.records.len(), 1, "a cancellation reached beyond its account");
        assert_eq!(nm.records[0].user_id, 0, "user 0's record was destroyed by another account's cancellation");
        assert_eq!(nm.get_badge_count("interos.messages", 0), 1);
        assert_eq!(nm.get_badge_count("interos.messages", 1000), 0);
    }
}
