//! Bounded Android framework surface.
//!
//! This is intentionally a compatibility ABI, not a copied Android framework.
//! Calls are translated into InterOS services or explicit lifecycle state hooks.
//! Unsupported framework APIs fail closed instead of silently becoming host access.

use alloc::string::String;
use alloc::vec::Vec;
use spin::Mutex;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AndroidFrameworkError {
    UnsupportedApi,
    PermissionDenied,
    InvalidArguments,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AndroidFrameworkService {
    Activity,
    Window,
    Input,
    Audio,
    Notification,
    Storage,
    Network,
    Telephony,
}

impl AndroidFrameworkService {
    pub const fn handle(self) -> u32 {
        match self {
            Self::Activity => 1,
            Self::Window => 2,
            Self::Input => 3,
            Self::Audio => 4,
            Self::Notification => 5,
            Self::Storage => 6,
            Self::Network => 7,
            Self::Telephony => 8,
        }
    }

    pub const fn service_name(self) -> &'static str {
        match self {
            Self::Activity => "activity",
            Self::Window => "window",
            Self::Input => "input",
            Self::Audio => "audio",
            Self::Notification => "notification",
            Self::Storage => "storage",
            Self::Network => "connectivity",
            Self::Telephony => "phone",
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            "activity" => Some(Self::Activity),
            "window" => Some(Self::Window),
            "input" => Some(Self::Input),
            "audio" => Some(Self::Audio),
            "notification" => Some(Self::Notification),
            "storage" => Some(Self::Storage),
            "connectivity" => Some(Self::Network),
            "phone" => Some(Self::Telephony),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AndroidFrameworkResult {
    Return(u32),
    ReturnString(&'static str),
    Service(AndroidFrameworkService),
}

/// Sovereign Android Activity Lifecycle State Machine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActivityLifecycleState {
    Uninitialized,
    Created,
    Started,
    Resumed,
    Paused,
    Stopped,
    Destroyed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FrameworkLifecycleManager {
    state: ActivityLifecycleState,
    finishing: bool,
    transition_count: u32,
}

impl FrameworkLifecycleManager {
    pub const fn new() -> Self {
        Self {
            state: ActivityLifecycleState::Uninitialized,
            finishing: false,
            transition_count: 0,
        }
    }

    pub fn current_state(&self) -> ActivityLifecycleState {
        self.state
    }

    pub fn is_finishing(&self) -> bool {
        self.finishing
    }

    pub fn reset(&mut self) {
        self.state = ActivityLifecycleState::Uninitialized;
        self.finishing = false;
        self.transition_count = 0;
    }

    pub fn transition_to(&mut self, target: ActivityLifecycleState) -> Result<(), AndroidFrameworkError> {
        let valid = match target {
            ActivityLifecycleState::Created => matches!(
                self.state,
                ActivityLifecycleState::Uninitialized | ActivityLifecycleState::Destroyed
            ),
            ActivityLifecycleState::Started => matches!(
                self.state,
                ActivityLifecycleState::Created | ActivityLifecycleState::Stopped
            ),
            ActivityLifecycleState::Resumed => matches!(
                self.state,
                ActivityLifecycleState::Started | ActivityLifecycleState::Paused
            ),
            ActivityLifecycleState::Paused => matches!(self.state, ActivityLifecycleState::Resumed),
            ActivityLifecycleState::Stopped => matches!(
                self.state,
                ActivityLifecycleState::Paused | ActivityLifecycleState::Started
            ),
            ActivityLifecycleState::Destroyed => matches!(
                self.state,
                ActivityLifecycleState::Stopped
                    | ActivityLifecycleState::Created
                    | ActivityLifecycleState::Paused
            ),
            ActivityLifecycleState::Uninitialized => true,
        };

        if !valid {
            return Err(AndroidFrameworkError::InvalidArguments);
        }

        self.state = target;
        self.transition_count = self.transition_count.saturating_add(1);
        if target == ActivityLifecycleState::Destroyed {
            self.finishing = false;
        }
        Ok(())
    }

    pub fn finish(&mut self) -> Result<(), AndroidFrameworkError> {
        if matches!(
            self.state,
            ActivityLifecycleState::Uninitialized | ActivityLifecycleState::Destroyed
        ) {
            return Err(AndroidFrameworkError::InvalidArguments);
        }
        self.finishing = true;
        Ok(())
    }
}

pub static LIFECYCLE_MANAGER: Mutex<FrameworkLifecycleManager> =
    Mutex::new(FrameworkLifecycleManager::new());

fn push_bounded_log(logs: &mut Vec<String>, msg: &str) {
    const MAX_LOG_ENTRIES: usize = 64;
    const MAX_LOG_LINE_BYTES: usize = 512;
    let mut line = String::from(msg);
    if line.len() > MAX_LOG_LINE_BYTES {
        line.truncate(MAX_LOG_LINE_BYTES);
    }
    if logs.len() >= MAX_LOG_ENTRIES {
        logs.remove(0);
    }
    logs.push(line);
}

pub fn dispatch(
    class_name: &str,
    method_name: &str,
    string_args: &[Option<&str>],
    logs: &mut Vec<String>,
) -> Result<AndroidFrameworkResult, AndroidFrameworkError> {
    let class = class_name.strip_prefix('L').and_then(|s| s.strip_suffix(';')).unwrap_or(class_name);

    match class {
        "android/util/Log" if matches!(method_name, "v" | "d" | "i" | "w" | "e") => {
            // Android apps are untrusted: logs must not become an unbounded kernel heap sink.
            const MAX_LOG_ENTRIES: usize = 64;
            const MAX_LOG_LINE_BYTES: usize = 512;
            let message = string_args.get(1).and_then(|value| *value).or_else(|| string_args.first().and_then(|value| *value)).unwrap_or("");
            let mut line = String::from("[ANDROID][LOG] ");
            for ch in message.chars() {
                if line.len().saturating_add(ch.len_utf8()) > MAX_LOG_LINE_BYTES { break; }
                line.push(ch);
            }
            if logs.len() >= MAX_LOG_ENTRIES { logs.remove(0); }
            logs.push(line);
            Ok(AndroidFrameworkResult::Return(0))
        }
        // UI Lifecycle: android/app/Activity
        "android/app/Activity" => {
            match method_name {
                "onCreate" => {
                    let mut lm = LIFECYCLE_MANAGER.lock();
                    lm.transition_to(ActivityLifecycleState::Created)?;
                    push_bounded_log(logs, "[ANDROID][LIFECYCLE] Activity::onCreate -> Created");
                    Ok(AndroidFrameworkResult::Return(0))
                }
                "onStart" => {
                    let mut lm = LIFECYCLE_MANAGER.lock();
                    lm.transition_to(ActivityLifecycleState::Started)?;
                    push_bounded_log(logs, "[ANDROID][LIFECYCLE] Activity::onStart -> Started");
                    Ok(AndroidFrameworkResult::Return(0))
                }
                "onResume" => {
                    let mut lm = LIFECYCLE_MANAGER.lock();
                    lm.transition_to(ActivityLifecycleState::Resumed)?;
                    push_bounded_log(logs, "[ANDROID][LIFECYCLE] Activity::onResume -> Resumed");
                    Ok(AndroidFrameworkResult::Return(0))
                }
                "onPause" => {
                    let mut lm = LIFECYCLE_MANAGER.lock();
                    lm.transition_to(ActivityLifecycleState::Paused)?;
                    push_bounded_log(logs, "[ANDROID][LIFECYCLE] Activity::onPause -> Paused");
                    Ok(AndroidFrameworkResult::Return(0))
                }
                "onStop" => {
                    let mut lm = LIFECYCLE_MANAGER.lock();
                    lm.transition_to(ActivityLifecycleState::Stopped)?;
                    push_bounded_log(logs, "[ANDROID][LIFECYCLE] Activity::onStop -> Stopped");
                    Ok(AndroidFrameworkResult::Return(0))
                }
                "onDestroy" => {
                    let mut lm = LIFECYCLE_MANAGER.lock();
                    lm.transition_to(ActivityLifecycleState::Destroyed)?;
                    push_bounded_log(logs, "[ANDROID][LIFECYCLE] Activity::onDestroy -> Destroyed");
                    Ok(AndroidFrameworkResult::Return(0))
                }
                "finish" => {
                    let mut lm = LIFECYCLE_MANAGER.lock();
                    lm.finish()?;
                    push_bounded_log(logs, "[ANDROID][LIFECYCLE] Activity::finish");
                    Ok(AndroidFrameworkResult::Return(0))
                }
                "isFinishing" => {
                    let lm = LIFECYCLE_MANAGER.lock();
                    Ok(AndroidFrameworkResult::Return(if lm.is_finishing() { 1 } else { 0 }))
                }
                // Unbacked Activity methods fail closed by design
                _ => Err(AndroidFrameworkError::UnsupportedApi),
            }
        }
        // Context and Service Surface Resolution
        "android/content/Context" => {
            match method_name {
                "getSystemService" => {
                    let service_name = string_args.iter().flatten().copied().find(|s| !s.is_empty()).unwrap_or("");
                    if service_name.is_empty() {
                        return Err(AndroidFrameworkError::UnsupportedApi);
                    }
                    if let Some(service) = AndroidFrameworkService::from_name(service_name) {
                        Ok(AndroidFrameworkResult::Service(service))
                    } else {
                        Err(AndroidFrameworkError::UnsupportedApi)
                    }
                }
                "getFilesDir" => Ok(AndroidFrameworkResult::ReturnString("/data/data/files")),
                "getDataDir" => Ok(AndroidFrameworkResult::ReturnString("/data/data")),
                _ => Err(AndroidFrameworkError::UnsupportedApi),
            }
        }
        // Service calls: NotificationManager
        "android/app/NotificationManager" => {
            match method_name {
                "notify" => {
                    let message = string_args.get(1).and_then(|v| *v)
                        .or_else(|| string_args.first().and_then(|v| *v))
                        .unwrap_or("Android Notification");
                    let mut nm = crate::notification::NOTIFICATION_MANAGER.lock();
                    let _ = nm.post(
                        "android.compat",
                        0,
                        "general",
                        "Android Alert",
                        message,
                        crate::notification::NotificationPriority::Default,
                        crate::notification::LockscreenVisibility::Public,
                        0,
                        0,
                    );
                    push_bounded_log(logs, "[ANDROID][NOTIFICATION] notify");
                    Ok(AndroidFrameworkResult::Return(0))
                }
                "cancel" => {
                    let mut nm = crate::notification::NOTIFICATION_MANAGER.lock();
                    nm.cancel_all_for_package("android.compat", 0);
                    push_bounded_log(logs, "[ANDROID][NOTIFICATION] cancel");
                    Ok(AndroidFrameworkResult::Return(0))
                }
                _ => Err(AndroidFrameworkError::UnsupportedApi),
            }
        }
        // Service calls: AudioManager
        "android/media/AudioManager" => {
            match method_name {
                "getStreamVolume" => {
                    let vol = crate::audio::AUDIO_MANAGER.lock().volume_percent.unwrap_or(0);
                    Ok(AndroidFrameworkResult::Return(vol as u32))
                }
                "setStreamVolume" => {
                    let mut audio = crate::audio::AUDIO_MANAGER.lock();
                    let target_vol = string_args.first()
                        .and_then(|s| *s)
                        .and_then(|s| s.parse::<u8>().ok())
                        .unwrap_or(10);
                    let confirmed = audio.set_volume(target_vol);
                    push_bounded_log(logs, "[ANDROID][AUDIO] setStreamVolume");
                    Ok(AndroidFrameworkResult::Return(confirmed as u32))
                }
                _ => Err(AndroidFrameworkError::UnsupportedApi),
            }
        }
        // Service calls: TelephonyManager
        "android/telephony/TelephonyManager" => {
            match method_name {
                "getNetworkOperator" => {
                    let sim = crate::telephony::SOVEREIGN_SIM.lock();
                    let op = sim.carrier_operator;
                    if op.is_empty() {
                        Ok(AndroidFrameworkResult::ReturnString(""))
                    } else {
                        Ok(AndroidFrameworkResult::ReturnString(op))
                    }
                }
                "getSimState" => {
                    let sim = crate::telephony::SOVEREIGN_SIM.lock();
                    let code = match sim.status {
                        crate::telephony::SimStatus::NoSim => 1,
                        crate::telephony::SimStatus::PinRequired => 2,
                        crate::telephony::SimStatus::Blocked => 4,
                        crate::telephony::SimStatus::Ready => 5,
                    };
                    Ok(AndroidFrameworkResult::Return(code))
                }
                // Zero Ambient Authority: hardware identifiers denied
                "getDeviceId" | "getImei" | "getLine1Number" => Err(AndroidFrameworkError::PermissionDenied),
                _ => Err(AndroidFrameworkError::UnsupportedApi),
            }
        }
        // Service calls: ConnectivityManager
        "android/net/ConnectivityManager" => {
            match method_name {
                "getActiveNetwork" => {
                    let link_up = crate::net::NETWORK_DEVICE.lock().info().link_up;
                    if link_up {
                        Ok(AndroidFrameworkResult::Return(1))
                    } else {
                        Ok(AndroidFrameworkResult::Return(0))
                    }
                }
                _ => Err(AndroidFrameworkError::UnsupportedApi),
            }
        }
        // Sovereign native microG shims: pure Rust, fail-closed, zero-heap
        "com/google/android/gms/common/GoogleApiAvailability" => {
            match method_name {
                // ConnectionResult.SUCCESS = 0
                "isGooglePlayServicesAvailable" => Ok(AndroidFrameworkResult::Return(0)),
                _ => Err(AndroidFrameworkError::UnsupportedApi),
            }
        }
        "com/google/android/gms/ads/identifier/AdvertisingIdClient$Info" => {
            match method_name {
                // Enforce limit ad tracking enabled (true = 1) and zeroed UUID to prevent cross-app tracking
                "isLimitAdTrackingEnabled" => Ok(AndroidFrameworkResult::Return(1)),
                "getId" => Ok(AndroidFrameworkResult::ReturnString("00000000-0000-0000-0000-000000000000")),
                _ => Err(AndroidFrameworkError::UnsupportedApi),
            }
        }
        "com/google/android/gms/ads/identifier/AdvertisingIdClient" => {
            match method_name {
                "getAdvertisingIdInfo" => Ok(AndroidFrameworkResult::Return(1)),
                _ => Err(AndroidFrameworkError::UnsupportedApi),
            }
        }
        // Unimplemented UI widgets, layout engines, and asynchronous message handlers
        // FAIL CLOSED by design — they must NEVER fabricate synthetic success.
        "android/view/View"
        | "android/view/SurfaceView"
        | "android/widget/TextView"
        | "android/widget/Button"
        | "android/os/Bundle"
        | "android/os/Looper"
        | "android/os/Handler" => Err(AndroidFrameworkError::UnsupportedApi),
        _ => Err(AndroidFrameworkError::UnsupportedApi),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn activity_ui_lifecycle_state_machine_contract() {
        let mut logs = Vec::new();
        LIFECYCLE_MANAGER.lock().reset();

        // 1. Initial valid sequence: onCreate -> onStart -> onResume
        assert_eq!(
            dispatch("Landroid/app/Activity;", "onCreate", &[], &mut logs),
            Ok(AndroidFrameworkResult::Return(0))
        );
        assert_eq!(LIFECYCLE_MANAGER.lock().current_state(), ActivityLifecycleState::Created);

        assert_eq!(
            dispatch("Landroid/app/Activity;", "onStart", &[], &mut logs),
            Ok(AndroidFrameworkResult::Return(0))
        );
        assert_eq!(LIFECYCLE_MANAGER.lock().current_state(), ActivityLifecycleState::Started);

        assert_eq!(
            dispatch("Landroid/app/Activity;", "onResume", &[], &mut logs),
            Ok(AndroidFrameworkResult::Return(0))
        );
        assert_eq!(LIFECYCLE_MANAGER.lock().current_state(), ActivityLifecycleState::Resumed);

        // 2. Out-of-order transition: calling onCreate while Resumed MUST fail
        assert_eq!(
            dispatch("Landroid/app/Activity;", "onCreate", &[], &mut logs),
            Err(AndroidFrameworkError::InvalidArguments)
        );

        // 3. Pause, Stop, Destroy sequence
        assert_eq!(
            dispatch("Landroid/app/Activity;", "onPause", &[], &mut logs),
            Ok(AndroidFrameworkResult::Return(0))
        );
        assert_eq!(LIFECYCLE_MANAGER.lock().current_state(), ActivityLifecycleState::Paused);

        assert_eq!(
            dispatch("Landroid/app/Activity;", "finish", &[], &mut logs),
            Ok(AndroidFrameworkResult::Return(0))
        );
        assert_eq!(
            dispatch("Landroid/app/Activity;", "isFinishing", &[], &mut logs),
            Ok(AndroidFrameworkResult::Return(1))
        );

        assert_eq!(
            dispatch("Landroid/app/Activity;", "onStop", &[], &mut logs),
            Ok(AndroidFrameworkResult::Return(0))
        );
        assert_eq!(LIFECYCLE_MANAGER.lock().current_state(), ActivityLifecycleState::Stopped);

        assert_eq!(
            dispatch("Landroid/app/Activity;", "onDestroy", &[], &mut logs),
            Ok(AndroidFrameworkResult::Return(0))
        );
        assert_eq!(LIFECYCLE_MANAGER.lock().current_state(), ActivityLifecycleState::Destroyed);
        assert_eq!(
            dispatch("Landroid/app/Activity;", "isFinishing", &[], &mut logs),
            Ok(AndroidFrameworkResult::Return(0))
        );

        // Logs contain bounded lifecycle records
        assert!(logs.iter().any(|l| l.contains("[ANDROID][LIFECYCLE] Activity::onCreate")));
        assert!(logs.iter().any(|l| l.contains("[ANDROID][LIFECYCLE] Activity::onDestroy")));
    }

    #[test]
    fn common_framework_calls_are_bounded() {
        let mut logs = Vec::new();
        assert_eq!(
            dispatch("Landroid/util/Log;", "d", &[Some("hello")], &mut logs),
            Ok(AndroidFrameworkResult::Return(0))
        );
        assert_eq!(logs.len(), 1);
        assert_eq!(
            dispatch("Landroid/content/Context;", "getSystemService", &[], &mut logs),
            Err(AndroidFrameworkError::UnsupportedApi)
        );
    }

    #[test]
    fn android_logs_are_bounded_by_entry_count_and_line_size() {
        let mut logs = Vec::new();
        let oversized = "x".repeat(4096);
        for _ in 0..80 {
            dispatch("Landroid/util/Log;", "i", &[Some(&oversized)], &mut logs).unwrap();
        }
        assert_eq!(logs.len(), 64);
        assert!(logs.iter().all(|line| line.len() <= 512));
    }

    #[test]
    fn service_surface_resolution_and_dispatch() {
        let mut logs = Vec::new();

        // 1. Context getSystemService resolution for supported services
        for (name, expected) in [
            ("window", AndroidFrameworkService::Window),
            ("activity", AndroidFrameworkService::Activity),
            ("input", AndroidFrameworkService::Input),
            ("audio", AndroidFrameworkService::Audio),
            ("notification", AndroidFrameworkService::Notification),
            ("storage", AndroidFrameworkService::Storage),
            ("connectivity", AndroidFrameworkService::Network),
            ("phone", AndroidFrameworkService::Telephony),
        ] {
            assert_eq!(
                dispatch("Landroid/content/Context;", "getSystemService", &[Some(name)], &mut logs),
                Ok(AndroidFrameworkResult::Service(expected)),
                "getSystemService({name}) must resolve to {expected:?}"
            );
        }

        // Unknown service must fail closed
        assert_eq!(
            dispatch("Landroid/content/Context;", "getSystemService", &[Some("unsupported_service")], &mut logs),
            Err(AndroidFrameworkError::UnsupportedApi)
        );

        // 2. NotificationManager notify and cancel
        assert_eq!(
            dispatch("Landroid/app/NotificationManager;", "notify", &[Some("tag"), Some("Alert text")], &mut logs),
            Ok(AndroidFrameworkResult::Return(0))
        );
        assert_eq!(
            dispatch("Landroid/app/NotificationManager;", "cancel", &[], &mut logs),
            Ok(AndroidFrameworkResult::Return(0))
        );

        // 3. AudioManager getStreamVolume and setStreamVolume
        assert_eq!(
            dispatch("Landroid/media/AudioManager;", "setStreamVolume", &[Some("70")], &mut logs),
            Ok(AndroidFrameworkResult::Return(70))
        );
        assert_eq!(
            dispatch("Landroid/media/AudioManager;", "getStreamVolume", &[], &mut logs),
            Ok(AndroidFrameworkResult::Return(70))
        );

        // 4. TelephonyManager getSimState and getNetworkOperator
        let sim_res = dispatch("Landroid/telephony/TelephonyManager;", "getSimState", &[], &mut logs);
        assert!(matches!(sim_res, Ok(AndroidFrameworkResult::Return(_))));

        let op_res = dispatch("Landroid/telephony/TelephonyManager;", "getNetworkOperator", &[], &mut logs);
        assert!(matches!(op_res, Ok(AndroidFrameworkResult::ReturnString(_))));

        // 5. ConnectivityManager getActiveNetwork
        let net_res = dispatch("Landroid/net/ConnectivityManager;", "getActiveNetwork", &[], &mut logs);
        assert!(matches!(net_res, Ok(AndroidFrameworkResult::Return(0 | 1))));

        // 6. Context files directory
        assert_eq!(
            dispatch("Landroid/content/Context;", "getFilesDir", &[], &mut logs),
            Ok(AndroidFrameworkResult::ReturnString("/data/data/files"))
        );
    }

    #[test]
    fn unimplemented_surfaces_fail_closed_and_never_fabricate_success() {
        let mut logs = Vec::new();
        for (class, method) in [
            // Widget mutations (require guest UI toolkit)
            ("Landroid/widget/TextView;", "setText"),
            ("Landroid/widget/TextView;", "setTextColor"),
            ("Landroid/widget/Button;", "<init>"),
            ("Landroid/view/View;", "invalidate"),
            ("Landroid/view/View;", "requestLayout"),
            ("Landroid/view/SurfaceView;", "<init>"),
            // Asynchronous message queues
            ("Landroid/os/Handler;", "post"),
            ("Landroid/os/Handler;", "postDelayed"),
            ("Landroid/os/Looper;", "getMainLooper"),
            // Complex guest structures
            ("Landroid/os/Bundle;", "<init>"),
            // Unimplemented activity methods
            ("Landroid/app/Activity;", "setContentView"),
            ("Landroid/app/Activity;", "startActivity"),
            // Unimplemented notification methods
            ("Landroid/app/NotificationManager;", "createNotificationChannelGroup"),
            // Ambient / unrestricted OS access
            ("Landroid/content/Context;", "sendBroadcast"),
            ("Landroid/os/Runtime;", "exec"),
        ] {
            assert_eq!(
                dispatch(class, method, &[], &mut logs),
                Err(AndroidFrameworkError::UnsupportedApi),
                "{class}->{method} must not return synthetic success"
            );
        }

        // Sensitive hardware queries fail with PermissionDenied (Zero Ambient Authority)
        for (class, method) in [
            ("Landroid/telephony/TelephonyManager;", "getDeviceId"),
            ("Landroid/telephony/TelephonyManager;", "getImei"),
            ("Landroid/telephony/TelephonyManager;", "getLine1Number"),
        ] {
            assert_eq!(
                dispatch(class, method, &[], &mut logs),
                Err(AndroidFrameworkError::PermissionDenied),
                "{class}->{method} must deny ambient hardware telemetry"
            );
        }
    }

    #[test]
    fn unsupported_framework_api_fails_closed() {
        let mut logs = Vec::new();
        assert_eq!(
            dispatch("Landroid/os/Runtime;", "exec", &[], &mut logs),
            Err(AndroidFrameworkError::UnsupportedApi)
        );
    }

    #[test]
    fn microg_native_shims_and_fail_closed_boundaries() {
        let mut logs = Vec::new();

        // 1. GoogleApiAvailability: isGooglePlayServicesAvailable returns SUCCESS (0)
        assert_eq!(
            dispatch("Lcom/google/android/gms/common/GoogleApiAvailability;", "isGooglePlayServicesAvailable", &[], &mut logs),
            Ok(AndroidFrameworkResult::Return(0))
        );

        // 2. AdvertisingIdClient$Info: isLimitAdTrackingEnabled returns true (1), getId returns zeroed UUID
        assert_eq!(
            dispatch("Lcom/google/android/gms/ads/identifier/AdvertisingIdClient$Info;", "isLimitAdTrackingEnabled", &[], &mut logs),
            Ok(AndroidFrameworkResult::Return(1))
        );
        assert_eq!(
            dispatch("Lcom/google/android/gms/ads/identifier/AdvertisingIdClient$Info;", "getId", &[], &mut logs),
            Ok(AndroidFrameworkResult::ReturnString("00000000-0000-0000-0000-000000000000"))
        );

        // 3. AdvertisingIdClient: getAdvertisingIdInfo returns instance (1)
        assert_eq!(
            dispatch("Lcom/google/android/gms/ads/identifier/AdvertisingIdClient;", "getAdvertisingIdInfo", &[], &mut logs),
            Ok(AndroidFrameworkResult::Return(1))
        );

        // 4. Unbacked methods on microG classes MUST fail closed
        assert_eq!(
            dispatch("Lcom/google/android/gms/common/GoogleApiAvailability;", "makeGooglePlayServicesAvailable", &[], &mut logs),
            Err(AndroidFrameworkError::UnsupportedApi)
        );
        assert_eq!(
            dispatch("Lcom/google/android/gms/common/GoogleApiAvailability;", "showErrorDialogFragment", &[], &mut logs),
            Err(AndroidFrameworkError::UnsupportedApi)
        );
        assert_eq!(
            dispatch("Lcom/google/android/gms/ads/identifier/AdvertisingIdClient$Info;", "unsupportedMethod", &[], &mut logs),
            Err(AndroidFrameworkError::UnsupportedApi)
        );

        // 5. Unbacked microG / Play Services classes MUST fail closed
        assert_eq!(
            dispatch("Lcom/google/android/gms/location/LocationServices;", "getFusedLocationProviderClient", &[], &mut logs),
            Err(AndroidFrameworkError::UnsupportedApi)
        );
        assert_eq!(
            dispatch("Lcom/google/android/gms/auth/api/signin/GoogleSignIn;", "getClient", &[], &mut logs),
            Err(AndroidFrameworkError::UnsupportedApi)
        );
    }
}
