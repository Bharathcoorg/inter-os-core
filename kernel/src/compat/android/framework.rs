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
            // Android apps are untrusted: logs must not become an unbounded kernel heap sink.
            const MAX_LOG_ENTRIES: usize = 64;
            const MAX_LOG_LINE_BYTES: usize = 512;
            let message = string_args.iter().flatten().next().unwrap_or("");
            let mut line = String::from("[ANDROID][LOG] ");
            for ch in message.chars() {
                if line.len().saturating_add(ch.len_utf8()) > MAX_LOG_LINE_BYTES { break; }
                line.push(ch);
            }
            if logs.len() >= MAX_LOG_ENTRIES { logs.remove(0); }
            logs.push(line);
            Ok(AndroidFrameworkResult::Return(0))
        }
        // Only APIs with an implemented effect are acknowledged. Lifecycle callbacks,
        // widget mutations, handlers, notifications and service lookups require real
        // guest objects/services; returning plausible values here would fabricate success.
        "android/app/Activity"
        | "android/view/View"
        | "android/view/SurfaceView"
        | "android/widget/TextView"
        | "android/widget/Button"
        | "android/os/Bundle"
        | "android/os/Looper"
        | "android/os/Handler"
        | "android/content/Context"
        | "android/app/NotificationManager"
        | "android/media/AudioManager"
        | "android/net/ConnectivityManager"
        | "android/telephony/TelephonyManager" => Err(AndroidFrameworkError::UnsupportedApi),
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
    fn unsupported_framework_api_fails_closed() {
        let mut logs = Vec::new();
        assert_eq!(
            dispatch("Landroid/os/Runtime;", "exec", &[], &mut logs),
            Err(AndroidFrameworkError::UnsupportedApi)
        );
    }
}
