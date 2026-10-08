//! Bounded Android framework surface.
//!
//! This is intentionally a compatibility ABI, not a copied Android framework.
//! Calls are translated into InterOS services or explicit no-op lifecycle hooks.
//! Unsupported framework APIs fail closed instead of silently becoming host access.

use alloc::string::String;
use alloc::vec::Vec;

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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AndroidFrameworkResult {
    Return(u32),
    Service(AndroidFrameworkService),
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
            let message = string_args.iter().flatten().next().unwrap_or("");
            let mut line = String::from("[ANDROID][LOG] ");
            line.push_str(message);
            logs.push(line);
            Ok(AndroidFrameworkResult::Return(0))
        }
        "android/app/Activity" if matches!(
            method_name,
            "onCreate" | "onStart" | "onResume" | "onPause" | "onStop" | "onDestroy"
        ) => Ok(AndroidFrameworkResult::Return(0)),
        "android/view/View" if matches!(method_name, "invalidate" | "requestLayout" | "setVisibility") => {
            Ok(AndroidFrameworkResult::Return(0))
        }
        "android/view/SurfaceView" if method_name == "<init>" => {
            Ok(AndroidFrameworkResult::Return(1))
        }
        "android/widget/TextView" if matches!(method_name, "<init>" | "setText" | "setTextColor" | "setTextSize") => {
            Ok(AndroidFrameworkResult::Return(0))
        }
        "android/widget/Button" if method_name == "<init>" => Ok(AndroidFrameworkResult::Return(0)),
        "android/os/Bundle" if method_name == "<init>" => Ok(AndroidFrameworkResult::Return(0)),
        "android/os/Looper" if method_name == "getMainLooper" => Ok(AndroidFrameworkResult::Return(1)),
        "android/os/Handler" if matches!(method_name, "<init>" | "post" | "postDelayed") => {
            Ok(AndroidFrameworkResult::Return(1))
        }
        "android/content/Context" if method_name == "getSystemService" => {
            Ok(AndroidFrameworkResult::Service(AndroidFrameworkService::Activity))
        }
        "android/content/Context" if method_name == "getFilesDir" => {
            Ok(AndroidFrameworkResult::Return(1))
        }
        "android/app/NotificationManager" if matches!(method_name, "notify" | "cancel") => {
            Ok(AndroidFrameworkResult::Return(0))
        }
        "android/media/AudioManager" if matches!(method_name, "getStreamVolume" | "setStreamVolume") => {
            Ok(AndroidFrameworkResult::Service(AndroidFrameworkService::Audio))
        }
        "android/net/ConnectivityManager" if method_name == "getActiveNetwork" => {
            Ok(AndroidFrameworkResult::Service(AndroidFrameworkService::Network))
        }
        "android/telephony/TelephonyManager" if matches!(method_name, "getNetworkOperator" | "getSimState") => {
            Ok(AndroidFrameworkResult::Service(AndroidFrameworkService::Telephony))
        }
        _ => Err(AndroidFrameworkError::UnsupportedApi),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
            Ok(AndroidFrameworkResult::Service(AndroidFrameworkService::Activity))
        );
    }

    #[test]
    fn unsupported_framework_api_fails_closed() {
        let mut logs = Vec::new();
        assert_eq!(
            dispatch("Landroid/os/Runtime;", "exec", &[], &mut logs),
            Err(AndroidFrameworkError::UnsupportedApi)
        );
    }
}
