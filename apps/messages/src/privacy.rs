//! The per-thread privacy setting.
//!
//! WHY this lives in the platform's settings store: the setting is a
//! preference about one conversation that has to outlive the app that set
//! it, and the settings store is the one place on the device where a
//! preference is kept between runs. A flag kept in the app's own memory
//! would be forgotten the moment the app closed, and a privacy control that
//! forgets itself is not a privacy control.
//!
//! WHY the setting is per thread and not global: the design document asks
//! for a control the user can apply to one contact, and a global switch
//! would not be that control.

use interos_std as std;

/// The prefix of the settings key that holds a thread's privacy setting.
pub const PRIVACY_KEY_PREFIX: &str = "messages.privacy.v1.";

/// The longest settings key the platform accepts.
pub const KEY_CAP: usize = 96;

/// The value stored when a thread's previews are hidden.
pub const HIDDEN_VALUE: &[u8] = b"1";

/// The value stored when a thread's previews are shown.
pub const SHOWN_VALUE: &[u8] = b"0";

/// Writes the settings key that holds `peer`'s privacy setting into `out`
/// and returns its length.
///
/// WHY the handle is sanitised: the settings store keys its records with an
/// `=` separator and forbids a newline in a key, and a peer handle is
/// whatever the user typed. A handle with an `=` in it would otherwise
/// split one record into two and the setting would be written under a key
/// the store can never find again.
pub fn key(peer: &[u8], out: &mut [u8; KEY_CAP]) -> usize {
    let mut at = 0usize;
    for &byte in PRIVACY_KEY_PREFIX.as_bytes() {
        if at >= out.len() {
            return at;
        }
        out[at] = byte;
        at += 1;
    }
    for &byte in peer {
        if at >= out.len() {
            return at;
        }
        let safe = match byte {
            b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'+' | b'.' | b'-' | b'_' => byte,
            _ => b'_',
        };
        out[at] = safe;
        at += 1;
    }
    at
}

/// Whether the thread with `peer` hides its previews in the thread list.
///
/// A thread the app has never stored a preference for shows its previews:
/// the setting is a choice the user makes, and the default is the one the
/// app had before the control existed.
pub fn previews_hidden(peer: &[u8]) -> bool {
    let mut key_buf = [0u8; KEY_CAP];
    let key_len = key(peer, &mut key_buf);
    let Ok(key_str) = core::str::from_utf8(&key_buf[..key_len]) else {
        return false;
    };
    let mut value = [0u8; 8];
    match std::settings_get(key_str, &mut value) {
        Ok(n) => &value[..n] == HIDDEN_VALUE,
        Err(_) => false,
    }
}

/// Stores whether the thread with `peer` hides its previews.
///
/// Returns whether the platform accepted the write. A refused write leaves
/// the previous value in place, so a caller that says "saved" when the write
/// was refused is wrong about a fact the user can check.
pub fn set_previews_hidden(peer: &[u8], hidden: bool) -> bool {
    let mut key_buf = [0u8; KEY_CAP];
    let key_len = key(peer, &mut key_buf);
    let Ok(key_str) = core::str::from_utf8(&key_buf[..key_len]) else {
        return false;
    };
    let value = if hidden { HIDDEN_VALUE } else { SHOWN_VALUE };
    std::settings_set(key_str, value).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_key_names_the_thread_the_setting_belongs_to() {
        let mut buf = [0u8; KEY_CAP];
        let len = key(b"+15550000", &mut buf);
        let text = core::str::from_utf8(&buf[..len]).expect("the key is text");
        assert_eq!(text, "messages.privacy.v1.+15550000");
    }

    #[test]
    fn a_handle_that_would_break_the_settings_store_is_sanitised() {
        // WHY: the store separates a key from its value with an `=` and
        // forbids a newline in a key, so a handle that carries one of those
        // characters would be stored under a key the store can never read
        // back.
        let mut buf = [0u8; KEY_CAP];
        let len = key(b"a=b\nc", &mut buf);
        let text = core::str::from_utf8(&buf[..len]).expect("the key is text");
        assert!(!text.contains('='));
        assert!(!text.contains('\n'));
        assert_eq!(text, "messages.privacy.v1.a_b_c");
    }

    #[test]
    fn two_handles_that_differ_only_in_sanitised_characters_share_a_key() {
        // WHY this matters: two different contacts whose handles differ only
        // in a character the store cannot carry would otherwise share one
        // setting, and hiding one would hide the other.
        let mut a = [0u8; KEY_CAP];
        let mut b = [0u8; KEY_CAP];
        let la = key(b"a=b", &mut a);
        let lb = key(b"a_b", &mut b);
        assert_eq!(&a[..la], &b[..lb]);
    }

    #[test]
    fn the_key_never_exceeds_the_platform_s_limit() {
        let mut buf = [0u8; KEY_CAP];
        let long = [b'9'; 128];
        let len = key(&long, &mut buf);
        assert!(len <= KEY_CAP);
        assert_eq!(len, KEY_CAP);
    }
}
