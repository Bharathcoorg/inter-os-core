// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

//! Android Compatibility Runtime (ACR) Cross-Zone Intent Bridge
//!
//! Provides bidirectional intent translation between isolated Android guest workloads
//! and native Inter OS sovereign intents while strictly preventing
//! cross-zone filesystem leakage and ambient capability escalation.

/// Android Intent Representation
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AndroidIntent<'a> {
    pub action: &'a str,
    pub data_uri: &'a str,
}

impl<'a> AndroidIntent<'a> {
    pub const fn new(action: &'a str, data_uri: &'a str) -> Self {
        Self { action, data_uri }
    }
}

/// Bridge Errors
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum AndroidBridgeError {
    SecurityViolation,
    UnsupportedAction,
    MalformedUri,
}

/// Sovereign Intent Mapping Result
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SovereignIntent {
    PaymentCheckout(&'static str),
    TelephonyDial(&'static str),
    BrowserUrl(&'static str),
    SecureShare(&'static str),
}

/// Cross-Zone Intent Translation Engine
pub struct AndroidIntentBridge;

impl AndroidIntentBridge {
    /// Translates an incoming Android Intent into a verified sovereign action string.
    pub fn translate_intent(intent: &AndroidIntent) -> Result<&'static str, AndroidBridgeError> {
        // Zero-Trust Boundary Guard: strictly reject attempts to access host OS paths via file:// or content://
        if intent.data_uri.contains("file://")
            || intent.data_uri.contains("content://")
            || intent.data_uri.contains("policy.json")
            || intent.data_uri.contains("..")
        {
            return Err(AndroidBridgeError::SecurityViolation);
        }

        match intent.action {
            "android.intent.action.VIEW" => {
                if intent.data_uri.starts_with("upi://") || intent.data_uri.contains("pay") {
                    Ok("INTENT_PAY")
                } else if intent.data_uri.starts_with("http://") || intent.data_uri.starts_with("https://") {
                    Ok("INTENT_BROWSER_URL")
                } else if intent.data_uri.starts_with("geo:") {
                    Ok("INTENT_MAPS_VIEW")
                } else {
                    Ok("INTENT_VIEW_DEFAULT")
                }
            }
            "android.intent.action.DIAL" | "android.intent.action.CALL" => {
                Ok("INTENT_TELEPHONY_DIAL")
            }
            "android.intent.action.SEND" => {
                Ok("INTENT_SHARE_PAYLOAD")
            }
            _ => Err(AndroidBridgeError::UnsupportedAction),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_intent_view_browser_pay_maps() {
        let i_pay = AndroidIntent::new("android.intent.action.VIEW", "upi://pay?pa=merchant@bank");
        assert_eq!(AndroidIntentBridge::translate_intent(&i_pay), Ok("INTENT_PAY"));

        let i_url = AndroidIntent::new("android.intent.action.VIEW", "https://interos.org/docs");
        assert_eq!(AndroidIntentBridge::translate_intent(&i_url), Ok("INTENT_BROWSER_URL"));

        let i_geo = AndroidIntent::new("android.intent.action.VIEW", "geo:37.7749,-122.4194");
        assert_eq!(AndroidIntentBridge::translate_intent(&i_geo), Ok("INTENT_MAPS_VIEW"));

        let i_def = AndroidIntent::new("android.intent.action.VIEW", "custom://item");
        assert_eq!(AndroidIntentBridge::translate_intent(&i_def), Ok("INTENT_VIEW_DEFAULT"));
    }

    #[test]
    fn test_intent_dial_call_send() {
        let i_dial = AndroidIntent::new("android.intent.action.DIAL", "tel:+15551234");
        assert_eq!(AndroidIntentBridge::translate_intent(&i_dial), Ok("INTENT_TELEPHONY_DIAL"));

        let i_call = AndroidIntent::new("android.intent.action.CALL", "tel:+15551234");
        assert_eq!(AndroidIntentBridge::translate_intent(&i_call), Ok("INTENT_TELEPHONY_DIAL"));

        let i_send = AndroidIntent::new("android.intent.action.SEND", "text/plain");
        assert_eq!(AndroidIntentBridge::translate_intent(&i_send), Ok("INTENT_SHARE_PAYLOAD"));
    }

    #[test]
    fn test_intent_security_violations_rejected() {
        let bad_file = AndroidIntent::new("android.intent.action.VIEW", "file:///system/etc/policy.json");
        assert_eq!(AndroidIntentBridge::translate_intent(&bad_file), Err(AndroidBridgeError::SecurityViolation));

        let bad_content = AndroidIntent::new("android.intent.action.VIEW", "content://com.android.providers/contacts");
        assert_eq!(AndroidIntentBridge::translate_intent(&bad_content), Err(AndroidBridgeError::SecurityViolation));

        let bad_traversal = AndroidIntent::new("android.intent.action.VIEW", "https://example.com/../../etc/passwd");
        assert_eq!(AndroidIntentBridge::translate_intent(&bad_traversal), Err(AndroidBridgeError::SecurityViolation));
    }

    #[test]
    fn test_intent_unsupported_action_fails_closed() {
        let unknown = AndroidIntent::new("com.malicious.ACTION_TAKEOVER", "");
        assert_eq!(AndroidIntentBridge::translate_intent(&unknown), Err(AndroidBridgeError::UnsupportedAction));
    }
}
