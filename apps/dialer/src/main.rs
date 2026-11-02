//! Inter OS Native Sovereign Telephony & Dialer App
//!
//! Native telephony dialer executing in Ring 3 (User Mode):
//! - Dual-Tone Multi-Frequency (DTMF) acoustic tone synthesis for keypad keys (0-9, *, #)
//! - Call State Machine: Idle, Dialing, Ringing, ActiveCall, CallHeld, CallEnded
//! - Cellular Baseband Modem HAL telemetry: 5G Carrier, RSSI, Cell ID, 3GPP AT state
//! - Local Contacts Book & Call History ledger
//! - Emergency E911/112 override pipeline
//! - Android Compatibility Runtime (ACR) cross-zone intent bridge dispatch
//! - DRM/KMS Client Surface Rendering (z=200) with Mobile Phone Keypad & Status Display
//! - Asynchronous event reactor loop handling touch input, modem URCs, and timers
//!
//! Typed input (mirrors `apps/browser/src/main.rs`):
//! - The dialed-number display is now a real keyboard target. `0-9`, `*` and `#`
//!   append and play the real DTMF tone, BACKSPACE deletes, ENTER dials, ESCAPE
//!   clears the entry and TAB steps the section. The app previously had no key
//!   handler at all.
//!
//! Live vs. honestly-disabled controls (nothing here is drawn and inert any more):
//! - The segmented tabs and the bottom dock are both live. Dial, Recents,
//!   Contacts and Settings are four real sections; the dock's Favorites slot
//!   opens the contacts list filtered to the contacts the user starred.
//! - Recents is backed by a real ledger of every dial attempt this session made
//!   and the outcome the telephony service actually returned for it.
//! - Contacts is backed by a real, user-created store (`ContactStore`): browse,
//!   add, edit, delete, star, and dial by name or by an entered number.
//! - Both long lists scroll. The recents ledger and the contact list are taller
//!   than the window they are shown in, so they carry the platform's scroll
//!   model and a visible thumb; a row below the fold used to be unreachable.
//! - The signal bars are drawn from the real `"bars"` field of the modem
//!   telemetry, not from a boolean. The battery is drawn only when
//!   `battery_query` actually returned a state-of-charge; otherwise nothing is
//!   drawn there, as the browser does for telemetry it does not receive.
//! - The call control is drawn recessed while the number field is empty, because
//!   there is nothing it could dial.
//!
//! THE SETTINGS SURFACE AND WHAT THE PLATFORM ACTUALLY SUPPORTS:
//! - The ringtone picker is built from the platform's document index, so it
//!   offers the audio documents the device really holds, and choosing one
//!   writes the device's `audio.ringtone` slot.
//! - The keypad-tone toggle is a real preference, stored in the settings store.
//! - The default-SIM choice is NOT offered: the platform exposes one SIM
//!   profile and no subscription-selection call, so the row says so instead of
//!   offering a choice that cannot be made.
//! - The call-duration row says that the platform's telephony telemetry
//!   carries no duration, because the app no longer counts one itself.
//!
//! THE CALL DURATION IS NOT COUNTED, AND NOT SHOWN:
//! the platform's telephony telemetry reports the modem's call state and its
//! signal readings, and no duration. The app used to keep a counter of its
//! own, which could only ever be the app's own ticks rather than a fact about
//! the call, so the counter is gone and the surface says the platform reports
//! no duration.
//!
//! WHAT THE CONTACT STORE IS AND IS NOT (read before changing any of it):
//! the records are real user input held in fixed-size arrays, and every mutation
//! is pushed through the platform's own durable store (syscall 83, the settings
//! store) before it is allowed to change the durable list. The store never
//! fabricates, seeds or samples a contact, and it never claims a write it did not
//! make.
//!
//! A contact only joins the durable list after the kernel acknowledges the write,
//! so the durable list is exactly what storage holds. When the kernel refuses the
//! write, the contact is held in a separate, visibly-labelled "unsaved" group
//! instead of being silently promoted to the durable list or silently dropped:
//! the notice in that group names the refusal, and `std::log` prints the exact
//! error code the syscall returned. See `ContactStoreState` for the wording.
//!
//! WHY the settings store and not a VFS file: the contact list is one flat array
//! of equal-sized records, so it already is a small binary value, and
//! `settings_set` commits it to persistent storage through the same path the
//! calendar, messages and music stores already use. A VFS file would buy nothing
//! -- the app would still read and rewrite the whole blob -- and it needs a
//! namespaced path that does not exist before the first save. No new syscall and
//! no kernel or SDK change is involved.
//!
//! Banded rendering:
//! - `Dirty` + `render_dialer_band(surface, app, y0, y1)` mirror the browser. The
//!   previous renderer pushed all 1,036,800 pixels through `get_dialer_pixel` for
//!   every touch event and every keystroke.

#![no_std]
#![no_main]

use interos_std::design::components;
use interos_std::design::outline_text;
use interos_std::design::scroll::ScrollState;

use core::panic::PanicInfo;
use interos_std as std;
use dialer::theme::{type_scale, THEME};
use interos_std::design::color;
use dialer::ModemCallState;
use dialer::{
    contacts_scroll_extents, dtmf_enabled, parse_audio_documents, put_decimal,
    recents_scroll_extents, set_dtmf_enabled, RingtoneChoice, SettingsRow,
    CONTACT_ROW_H as LIB_CONTACT_ROW_H, CONTACTS_LIST_Y0 as LIB_CONTACTS_LIST_Y0,
    CONTACTS_VIEWPORT_Y1 as LIB_CONTACTS_VIEWPORT_Y1, RECENTS_ROW_H as LIB_RECENTS_ROW_H,
    RECENTS_VIEWPORT_Y1 as LIB_RECENTS_VIEWPORT_Y1, RECENTS_Y0 as LIB_RECENTS_Y0,
};

/// The platform's corner radii, under the short names the renderer uses.
///
/// WHY the aliases exist: the corner test names its radius four times, and a
/// number that appears four times is a number that can drift from the
/// platform. The alias resolves to the platform's step, so the drift is the
/// platform's and not this app's.

/// The platform's card treatment, at the elevation the app's cards sit at.
///
/// WHY the app's cards are the platform's cards: a card that invents its own
/// fill and its own hairline is a card a third-party app cannot match, and
/// the platform already says what a card looks like.
const CARD: components::CardStyle =
    components::card_style(&THEME, components::Elevation::Resting);

const W: usize = 720;
const H: usize = 1440;
const CHUNK: usize = 4096;

/// Current execution state of the telephony call engine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CallState {
    Idle = 0,
    Dialing = 1,
    Ringing = 2,
    ActiveCall = 3,
    CallHeld = 4,
    CallEnded = 5,
}

/// Which content the tabs and the dock select.
///
/// WHY `Contacts` exists now: the app owns a real contact store
/// (`ContactStore`), so the tab and both dock slots that used to imply a
/// contacts screen have something real behind them.
///
/// WHY `Settings` exists now: the dialer's design document asks for a
/// settings surface, and the platform supports some of what it asks for: the
/// device's ringtone slot and a settings store. The surface also says which
/// of the requested settings the platform does not offer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DialerSection {
    /// The 3x4 dial pad and the call controls.
    Keypad = 0,
    /// The real ledger of dial attempts made in this session.
    Recents = 1,
    /// The user's contacts: browse, add, edit, delete, star and dial.
    Contacts = 2,
    /// The dialer's own settings: the ringtone, the keypad tone and the
    /// choices the platform does not offer.
    Settings = 3,
}

/// Real outcome of the last call attempt, drawn under the dialed number.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CallOutcome {
    /// No attempt yet.
    Idle,
    /// The telephony service accepted the request and the modem is placing
    /// the call.
    Dialing,
    /// The modem itself reported the call as connected.
    Connected,
    /// The modem reported the call as ended.
    Ended,
    /// The telephony service accepted the request but the modem did not
    /// report a call state this build knows.
    Unconfirmed,
    /// The telephony service refused the call.
    Refused,
    /// An emergency dial attempt was refused by the telephony service.
    EmergencyRefused,
}

impl CallOutcome {
    /// Caption drawn under the dialed number. Empty for `Idle` - an empty run is
    /// never drawn at all, so an idle dialer shows no status line rather than a
    /// placeholder that could be mistaken for one.
    fn caption(&self) -> &'static [u8] {
        match self {
            CallOutcome::Idle => b"",
            CallOutcome::Dialing => b"Calling...",
            CallOutcome::Connected => b"Call connected",
            CallOutcome::Ended => b"Call ended",
            CallOutcome::Unconfirmed => b"Call requested; the modem reported no call state",
            CallOutcome::Refused => b"Call refused by the telephony service",
            CallOutcome::EmergencyRefused => b"Emergency call refused by the telephony service",
        }
    }

    /// The line that says the platform reports no call duration.
    ///
    /// WHY it is drawn only for a connected call: a duration is a fact about a
    /// call that is up, and the platform's telephony telemetry carries the
    /// modem's call state and its signal readings and no duration at all. The
    /// app used to keep a counter of its own and show it, which was the app's
    /// ticks rather than a fact about the call.
    fn duration_note(&self) -> &'static [u8] {
        match self {
            CallOutcome::Connected => b"The platform reports no call duration",
            _ => b"",
        }
    }
}

/// Asks the modem what state its call is in.
///
/// WHY the modem and not the app: only the modem knows whether a call
/// connected. The telephony service's acceptance of the request says the
/// request was taken, and nothing more.
fn query_modem_call_state() -> ModemCallState {
    let mut buf = [0u8; 256];
    match std::telephony_query(&mut buf) {
        Ok(n) if n > 0 => dialer::parse_modem_call_state(&buf[..n]),
        _ => ModemCallState::Other,
    }
}

/// The settings key the dialer hands a recipient to the messaging app under.
///
/// WHY a settings key and not an argument: `app_launch` takes an app id and
/// nothing else, so the platform's settings store is the only channel the
/// two apps share.
const HANDOFF_KEY: &str = "dialer.handoff.recipient.v1";

/// The messaging app this app hands a recipient to.
const MESSAGING_APP_ID: &str = "org.interos.messages";

/// The message type the hand-off is sent under.
///
/// WHY a number and not a name: the IPC message type is a number on the wire,
/// and this app is the only thing that both sends and receives it.
const HANDOFF_MSG_TYPE: u32 = 0x4449_414C;

/// Logs a line that carries a platform error code.
///
/// WHY the code is in the line and not beside it: a report of a refusal that
/// does not carry the number the platform returned is not a report.
fn log_with_code(prefix: &str, code: i64) {
    let mut buf = [0u8; 160];
    let mut n = 0usize;
    for byte in prefix.as_bytes() {
        if n >= buf.len() {
            break;
        }
        buf[n] = *byte;
        n += 1;
    }
    if n < buf.len() {
        buf[n] = b' ';
        n += 1;
    }
    if code < 0 && n < buf.len() {
        buf[n] = b'-';
        n += 1;
    }
    n += put_decimal(code.unsigned_abs() as u32, &mut buf[n..]);
    if let Ok(text) = core::str::from_utf8(&buf[..n]) {
        std::log(text).ok();
    }
}

/// Finds the messaging app's task id in the platform's runtime list.
///
/// WHY the runtime list and not a fixed id: task ids are assigned by the
/// kernel when the task is created, so a number written into an app is a
/// number that is wrong on the next boot. The platform publishes the list.
///
/// WHY the parse: the list is one `task_id|package_id|process_state|task_state|parent_task_id`
/// record per line.
fn messaging_task_id() -> Option<u64> {
    let mut buf = [0u8; 1024];
    let n = std::app_runtime_list(&mut buf).ok()?;
    let text = core::str::from_utf8(&buf[..n]).ok()?;
    for line in text.lines() {
        let mut fields = line.split('|');
        let task_id = fields.next()?;
        let package = fields.next()?;
        if package.trim() != MESSAGING_APP_ID {
            continue;
        }
        if let Ok(id) = u64::from_str_radix(task_id.trim(), 16) {
            return Some(id);
        }
    }
    None
}

/// Hands the entered number to the messaging app and opens it.
///
/// WHY the IPC message comes first: this app holds an IPC-send capability, and
/// a capability that is never used is a capability the platform granted for
/// nothing. The recipient is sent to the messaging app over IPC when the
/// platform's authority allows it.
///
/// WHY the settings store is still there: the IPC-send capability this task
/// holds is scoped to one object - the task that launched it - and the kernel
/// refuses a message to any other destination. The messaging app is not that
/// task, so the message is refused with PermissionDenied and the settings
/// store remains the channel the two apps share. The refusal is reported
/// rather than swallowed, because a refused hand-off is a fact the user needs.
fn hand_off_to_messaging(number: &[u8]) -> bool {
    if number.is_empty() || number.len() > 32 {
        return false;
    }
    // The settings store refuses a value containing 0x0A, so the number is
    // written only when it is plain text.
    if number.iter().any(|&b| b == 0x0A) {
        return false;
    }

    // The capability path: hand the recipient to the messaging app over IPC.
    let mut delivered_by_ipc = false;
    match messaging_task_id() {
        Some(task_id) => match std::ipc_send(task_id, HANDOFF_MSG_TYPE, number) {
            Ok(()) => {
                delivered_by_ipc = true;
                std::log("[DIALER APP] Handed the recipient to the messaging app over IPC.").ok();
            }
            Err(code) => {
                log_with_code(
                    "[DIALER APP] The platform refused the IPC hand-off, code",
                    code,
                );
                std::log("[DIALER APP] This app's IPC-send authority covers the task that launched it, not the messaging app, so the message could not be sent to it.")
                    .ok();
            }
        },
        None => {
            std::log("[DIALER APP] The messaging app is not running, so there is no task to hand the recipient to over IPC.")
                .ok();
        }
    }

    if std::settings_set(HANDOFF_KEY, number).is_err() {
        std::log("[DIALER APP] The settings store refused the recipient hand-off; the messaging app was not opened.",
        )
        .ok();
        return false;
    }
    match std::app_launch(MESSAGING_APP_ID) {
        Ok(_) => {
            std::log("[DIALER APP] Wrote the recipient to the shared settings store and opened the messaging app.",
            )
            .ok();
            let _ = delivered_by_ipc;
            true
        }
        Err(_) => {
            std::log("[DIALER APP] The messaging app could not be launched; the number was not sent anywhere.",
            )
            .ok();
            false
        }
    }
}

/// One real entry in the recents ledger.
#[derive(Clone, Copy)]
pub struct CallRecord {
    pub number: [u8; 32],
    pub len: usize,
    /// `true` only when `telephony_dial` returned `Ok`.
    pub accepted: bool,
    /// `true` when the entry came from the emergency control.
    pub emergency: bool,
    /// The outcome the platform reported for this attempt.
    ///
    /// WHY the ledger keeps the outcome and not only a yes/no: a call the
    /// telephony service accepted and a call the modem reported as connected
    /// are different facts, and a history that flattened them into "accepted"
    /// could not say which one happened.
    pub outcome: CallOutcome,
}

/// Keypad DTMF frequency pair (low frequency, high frequency in Hz).
#[derive(Debug, Clone, Copy)]
pub struct DtmfPair {
    pub low_freq: u32,
    pub high_freq: u32,
}

/// Resolves standard CCITT dual-tone frequencies for telephone keypad keys.
pub fn get_dtmf_frequencies(key: char) -> Option<DtmfPair> {
    match key {
        '1' => Some(DtmfPair {
            low_freq: 697,
            high_freq: 1209,
        }),
        '2' => Some(DtmfPair {
            low_freq: 697,
            high_freq: 1336,
        }),
        '3' => Some(DtmfPair {
            low_freq: 697,
            high_freq: 1477,
        }),
        '4' => Some(DtmfPair {
            low_freq: 770,
            high_freq: 1209,
        }),
        '5' => Some(DtmfPair {
            low_freq: 770,
            high_freq: 1336,
        }),
        '6' => Some(DtmfPair {
            low_freq: 770,
            high_freq: 1477,
        }),
        '7' => Some(DtmfPair {
            low_freq: 852,
            high_freq: 1209,
        }),
        '8' => Some(DtmfPair {
            low_freq: 852,
            high_freq: 1336,
        }),
        '9' => Some(DtmfPair {
            low_freq: 852,
            high_freq: 1477,
        }),
        '0' => Some(DtmfPair {
            low_freq: 941,
            high_freq: 1336,
        }),
        '*' => Some(DtmfPair {
            low_freq: 941,
            high_freq: 1209,
        }),
        '#' => Some(DtmfPair {
            low_freq: 941,
            high_freq: 1477,
        }),
        _ => None,
    }
}

/// Sovereign Contact Record.
#[derive(Debug, Clone, Copy)]
pub struct Contact {
    pub name: &'static str,
    pub number: &'static str,
    pub did: &'static str,
    pub is_emergency: bool,
}

/// Contacts are populated only from real user input, so this stays empty. It is
/// the app's "no bundled demo content" guard and is asserted by
/// `tests/app_truth_contract.sh`, which greps for this exact line.
#[allow(dead_code)]
pub static CONTACTS_BOOK: [Contact; 0] = [];

// -----------------------------------------------------------------------------
// Contact store: bounded fixed-size records in the platform's settings store
// -----------------------------------------------------------------------------

/// Longest contact name the record holds.
///
/// WHY 16 bytes: the whole store has to fit one settings value, and the kernel
/// refuses a value longer than 512 bytes, so the record width is what bounds the
/// capacity. 16 printable characters covers the names a phone contact list
/// actually carries while leaving room for a real number.
const CONTACT_NAME_MAX: usize = 16;

/// Longest phone number the record holds.
///
/// WHY 16 bytes: the E.164 international number format is at most 15 digits, so
/// 16 holds a full number plus the leading `+`.
const CONTACT_NUM_MAX: usize = 16;

/// Bounded number of contacts the store holds.
///
/// WHY 8: a record is `CONTACT_RECORD_BYTES` wide, the settings value cap is 512
/// bytes, and the contacts list only has room for eight 66px rows on a 1440px
/// surface, so 12 is what both the storage cap and the screen allow. The store
/// reports that it is full instead of dropping a contact on the floor.
///
/// Binary escaping for 0x0A: settings store rejects values with 0x0A ('\n').
/// With `escape_store_blob`, any 0x0A (or 0xFE) byte is escaped so the < 10
/// contact count ceiling is completely lifted.
pub const MAX_CONTACTS: usize = 12;

/// Writes the decimal form of `value` into `line` at `n`, returning the new
/// length.
///
/// WHY this exists: the notice band used to write a count as a single
/// digit byte, `b'0' + count`, which printed a colon for ten and a wrong
/// number for anything above nine.
fn put_u16(line: &mut [u8], n: usize, value: u16) -> usize {
    if value == 0 {
        if n < line.len() {
            line[n] = b'0';
            return n + 1;
        }
        return n;
    }
    let mut digits = [0u8; 5];
    let mut len = 0usize;
    let mut v = value;
    while v > 0 && len < digits.len() {
        digits[len] = b'0' + (v % 10) as u8;
        v /= 10;
        len += 1;
    }
    let mut out = n;
    while len > 0 {
        len -= 1;
        if out < line.len() {
            line[out] = digits[len];
            out += 1;
        }
    }
    out
}

/// Copies `src` into `out` and returns how many bytes were written.
///
/// WHY this exists: the app has no allocator, so every piece of text it shows
/// is copied into a fixed buffer, and the copy has to say how much of it it
/// used.
pub fn copy_bytes(out: &mut [u8], src: &[u8]) -> usize {
    let n = core::cmp::min(out.len(), src.len());
    out[..n].copy_from_slice(&src[..n]);
    n
}

pub const ESCAPE_BYTE: u8 = 0xFE;
pub const ESCAPED_NEWLINE: u8 = 0x8A;
pub const ESCAPED_ESCAPE: u8 = 0xEE;

/// Escapes any 0x0A ('\n') or 0xFE bytes in a raw store blob so the serialized
/// settings value never contains a newline.
pub fn escape_store_blob(raw: &[u8], out: &mut [u8]) -> Result<usize, ()> {
    let mut pos = 0usize;
    for &b in raw {
        if b == b'\n' {
            if pos + 2 > out.len() {
                return Err(());
            }
            out[pos] = ESCAPE_BYTE;
            out[pos + 1] = ESCAPED_NEWLINE;
            pos += 2;
        } else if b == ESCAPE_BYTE {
            if pos + 2 > out.len() {
                return Err(());
            }
            out[pos] = ESCAPE_BYTE;
            out[pos + 1] = ESCAPED_ESCAPE;
            pos += 2;
        } else {
            if pos + 1 > out.len() {
                return Err(());
            }
            out[pos] = b;
            pos += 1;
        }
    }
    Ok(pos)
}

/// Unescapes a store blob read from the settings store. Handles both escaped
/// modern payloads and legacy unescaped blobs transparently.
pub fn unescape_store_blob(encoded: &[u8], out: &mut [u8]) -> Result<usize, ()> {
    let mut in_pos = 0usize;
    let mut out_pos = 0usize;
    while in_pos < encoded.len() {
        let b = encoded[in_pos];
        if b == ESCAPE_BYTE {
            in_pos += 1;
            if in_pos >= encoded.len() {
                return Err(());
            }
            match encoded[in_pos] {
                ESCAPED_NEWLINE => {
                    if out_pos >= out.len() {
                        return Err(());
                    }
                    out[out_pos] = b'\n';
                    out_pos += 1;
                }
                ESCAPED_ESCAPE => {
                    if out_pos >= out.len() {
                        return Err(());
                    }
                    out[out_pos] = ESCAPE_BYTE;
                    out_pos += 1;
                }
                _ => return Err(()),
            }
            in_pos += 1;
        } else {
            if out_pos >= out.len() {
                return Err(());
            }
            out[out_pos] = b;
            out_pos += 1;
            in_pos += 1;
        }
    }
    Ok(out_pos)
}

/// Record width, pinned: 1 id + 1 flags + `CONTACT_NAME_MAX` name bytes
/// (0x00-padded) + `CONTACT_NUM_MAX` number bytes (0x00-padded) = 36 bytes.
///
/// WHY the width is written out rather than derived: `#![no_std]` with no
/// allocator means the store is a fixed byte array, so this encoding is the
/// store's on-storage contract. Changing any field width orphans every contact
/// already saved.
///
/// WHY the strings are 0x00-padded instead of length-prefixed: a length byte can
/// itself be 10, which is exactly the 0x0A the settings store refuses. Padding
/// with 0x00 and taking the length from the first zero keeps every byte in the
/// record in the set {0x00, printable ASCII, `+`, `*`, `#`, id 1..=8, flags 0|1},
/// none of which is 0x0A.
const CONTACT_RECORD_BYTES: usize = 1 + 1 + CONTACT_NAME_MAX + CONTACT_NUM_MAX;

/// Blob width: 1 version byte + 1 count byte + `MAX_CONTACTS` records
/// = 290 bytes, inside both the kernel's 512-byte value cap and the
/// `settings_get` output cap of 512 bytes.
const STORE_BLOB_BYTES: usize = 2 + MAX_CONTACTS * CONTACT_RECORD_BYTES;

/// Store format version. A blob that does not start with this byte is not ours.
const STORE_VERSION: u8 = 1;

/// Settings key the whole contact store is written under.
const STORE_KEY: &str = "dialer.contacts.v1";

/// WHY slot 3 for read and slot 7 for write: `spawn_app_dialer` in
/// `kernel/src/task/user.rs` grants `FileSystemRead` with `Rights::READ` in slot 3,
/// and `FileSystemWrite` with `Rights::WRITE` in slot 7. `sys_settings_get` is
/// authorized by slot 3 (or 7), while `sys_settings_set` strictly requires a
/// `FileSystemWrite` capability, which is slot 7.

/// Byte cost of the whole feature, stated once so it can be re-checked:
/// 8 contacts x 36 bytes = 288 bytes of RAM for the records, plus a 290-byte
/// staging buffer that exists only for the duration of one save. No heap, no
/// allocator, no `Vec`, no `String`.
pub const CONTACT_STORE_RAM_BYTES: usize = MAX_CONTACTS * CONTACT_RECORD_BYTES;

/// One user-created contact, exactly as it is stored.
///
/// WHY `[u8; N]` arrays and not `&'static str`: the old `Contact` held borrowed
/// static strings, which can only ever describe a compile-time constant. A
/// contact the user types at runtime needs owned, bounded storage, and a
/// `#![no_std]` binary with no allocator gets that from fixed arrays.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct ContactRecord {
    /// 1..=MAX_CONTACTS. Never 0, and never 10 (see `MAX_CONTACTS`).
    id: u8,
    /// Bit 0 = starred. The values are 0 and 1 only.
    flags: u8,
    /// Printable ASCII, 0x00-padded to `CONTACT_NAME_MAX`.
    name: [u8; CONTACT_NAME_MAX],
    /// Digits and `+*#`, 0x00-padded to `CONTACT_NUM_MAX`.
    number: [u8; CONTACT_NUM_MAX],
}

impl ContactRecord {
    const EMPTY: ContactRecord = ContactRecord {
        id: 0,
        flags: 0,
        name: [0; CONTACT_NAME_MAX],
        number: [0; CONTACT_NUM_MAX],
    };

    const FLAG_FAVOURITE: u8 = 1 << 0;

    fn from_parts(id: u8, name: &[u8], number: &[u8], favourite: bool) -> Self {
        let mut rec = ContactRecord::EMPTY;
        rec.id = id;
        rec.set_favourite(favourite);
        let n = name.len().min(CONTACT_NAME_MAX);
        rec.name[..n].copy_from_slice(&name[..n]);
        let m = number.len().min(CONTACT_NUM_MAX);
        rec.number[..m].copy_from_slice(&number[..m]);
        rec
    }

    /// True when the record holds a name and a number the store can hand back.
    ///
    /// WHY the byte sets are enforced here and not only at the editor: this is
    /// the one function that reads bytes back out of storage, so it is the one
    /// place a malformed or foreign blob can be rejected. Without it a corrupt
    /// value would surface as a contact nobody typed.
    fn valid(&self) -> bool {
        (1..=MAX_CONTACTS as u8).contains(&self.id)
            && self.flags & !Self::FLAG_FAVOURITE == 0
            && self.name_bytes().iter().all(|b| (0x20..=0x7E).contains(b))
            && self
                .number
                .iter()
                .take(self.number_len())
                .all(|b| b.is_ascii_digit() || *b == b'+' || *b == b'*' || *b == b'#')
    }

    /// Name length, taken as the offset of the first 0x00 pad byte.
    fn name_len(&self) -> usize {
        self.name.iter().position(|&b| b == 0).unwrap_or(CONTACT_NAME_MAX)
    }

    fn number_len(&self) -> usize {
        self.number
            .iter()
            .position(|&b| b == 0)
            .unwrap_or(CONTACT_NUM_MAX)
    }

    fn name_bytes(&self) -> &[u8] {
        &self.name[..self.name_len()]
    }

    fn number_bytes(&self) -> &[u8] {
        &self.number[..self.number_len()]
    }

    fn is_favourite(&self) -> bool {
        self.flags & Self::FLAG_FAVOURITE != 0
    }

    fn set_favourite(&mut self, on: bool) {
        self.flags = (self.flags & !Self::FLAG_FAVOURITE) | (on as u8) * Self::FLAG_FAVOURITE;
    }

    /// First letter of the name, upper-cased, for the list avatar. Falls back to
    /// `#` for a name that starts with something that has no upper case.
    fn initial(&self) -> u8 {
        let b = self.name_bytes().first().copied().unwrap_or(b'#');
        if b.is_ascii_lowercase() {
            b - 32
        } else {
            b
        }
    }

    fn encode(&self, out: &mut [u8]) {
        out[0] = self.id;
        out[1] = self.flags;
        out[2..2 + CONTACT_NAME_MAX].copy_from_slice(&self.name);
        out[2 + CONTACT_NAME_MAX..2 + CONTACT_NAME_MAX + CONTACT_NUM_MAX].copy_from_slice(&self.number);
    }

    /// Decodes one record, rejecting anything the store could not have written.
    fn decode(bytes: &[u8]) -> Option<ContactRecord> {
        if bytes.len() < CONTACT_RECORD_BYTES {
            return None;
        }
        let mut rec = ContactRecord::EMPTY;
        rec.id = bytes[0];
        rec.flags = bytes[1];
        rec.name.copy_from_slice(&bytes[2..2 + CONTACT_NAME_MAX]);
        let num_at = 2 + CONTACT_NAME_MAX;
        rec.number
            .copy_from_slice(&bytes[num_at..num_at + CONTACT_NUM_MAX]);
        if rec.valid() {
            Some(rec)
        } else {
            None
        }
    }
}

/// What the store's last contact with the platform storage actually did.
///
/// WHY an enum rather than a "success flag" and a message: the list, the logs and
/// the footer must all say the same thing about storage, and two parallel values
/// can disagree with each other. One value is turned into words in exactly one
/// place, and it only ever reports what a syscall returned.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ContactStoreState {
    /// The store has not been read yet.
    Untouched,
    /// The settings store holds no dialer key, so there is nothing to load.
    Empty,
    /// Contacts were read back out of the settings store.
    Loaded,
    /// The kernel refused the read.
    ReadRefused,
    /// The stored bytes were not a store this app wrote.
    Invalid,
    /// The last write was acknowledged, so the durable list is what storage holds.
    Written,
    /// The kernel refused the write. Nothing was promoted to the durable list.
    WriteRefused,
    /// The store is at its capacity and refused another contact.
    Full,
}

impl ContactStoreState {
    /// The words this state is reported in, to the log and to the screen.
    fn as_str(self) -> &'static str {
        match self {
            ContactStoreState::Untouched => "CONTACT STORE NOT READ YET",
            ContactStoreState::Empty => "NO SAVED CONTACTS",
            ContactStoreState::Loaded => "CONTACTS READ FROM THE SETTINGS STORE",
            ContactStoreState::ReadRefused => "SETTINGS READ REFUSED: NO SAVED CONTACTS",
            ContactStoreState::Invalid => "STORED CONTACT DATA FAILED VALIDATION",
            ContactStoreState::Written => "SAVED TO THE SETTINGS STORE",
            ContactStoreState::WriteRefused => "NOT SAVED: THE KERNEL REFUSED THE WRITE",
            ContactStoreState::Full => "STORE FULL: NO CONTACT ADDED",
        }
    }
}

/// The user's contacts: bounded records plus an honest account of the last load
/// or save.
///
/// WHY there are two lists: a contact only joins the durable list after the
/// kernel acknowledges the write, so the durable list is always exactly what
/// storage holds. A write the kernel refuses cannot be dropped (that would delete
/// what the user just typed) and cannot be promoted (that would claim a
/// durability the device does not have), so it goes in `unsaved`, which is drawn
/// and labelled separately.
#[derive(Clone, Copy)]
pub struct ContactStore {
    records: [ContactRecord; MAX_CONTACTS],
    len: u8,
    unsaved: [ContactRecord; MAX_CONTACTS],
    unsaved_len: u8,
    state: ContactStoreState,
    /// The exact error code from the last refused write, for the log.
    last_error: i64,
    /// Id to hand to the next record. Monotonic within a session so a delete
    /// never re-issues an id that another open record still carries.
    next_id: u8,
}

impl ContactStore {
    const EMPTY: ContactStore = ContactStore {
        records: [ContactRecord::EMPTY; MAX_CONTACTS],
        len: 0,
        unsaved: [ContactRecord::EMPTY; MAX_CONTACTS],
        unsaved_len: 0,
        state: ContactStoreState::Untouched,
        last_error: 0,
        next_id: 1,
    };

    /// Reads the store back out of the platform settings store.
    ///
    /// WHY the whole blob is rejected when any record fails validation: the blob
    /// is written as a single settings value, so a valid store never splits
    /// across reads. Accepting a prefix would present a partial contact list as
    /// the stored one, so the load reports what it actually has instead.
    fn load() -> Self {
        let mut store = ContactStore::EMPTY;
        let mut encoded = [0u8; STORE_BLOB_BYTES + 32];
        match std::settings_get(STORE_KEY, &mut encoded) {
            Ok(0) => store.state = ContactStoreState::Empty,
            Ok(n) => {
                let mut blob = [0u8; STORE_BLOB_BYTES];
                let decoded_len = match unescape_store_blob(&encoded[..n], &mut blob) {
                    Ok(len) => len,
                    Err(_) => {
                        store.state = ContactStoreState::Invalid;
                        return store;
                    }
                };
                let mut records = [ContactRecord::EMPTY; MAX_CONTACTS];
                let mut count = 0usize;
                let mut rejected = false;
                if decoded_len > STORE_BLOB_BYTES || blob[0] != STORE_VERSION {
                    rejected = true;
                } else {
                    let header_count = blob[1] as usize;
                    if header_count > MAX_CONTACTS
                        || decoded_len != 2 + header_count * CONTACT_RECORD_BYTES
                    {
                        rejected = true;
                    } else {
                        for i in 0..header_count {
                            let start = 2 + i * CONTACT_RECORD_BYTES;
                            match ContactRecord::decode(
                                &blob[start..start + CONTACT_RECORD_BYTES],
                            ) {
                                Some(rec) => {
                                    records[i] = rec;
                                    count += 1;
                                }
                                None => {
                                    rejected = true;
                                    break;
                                }
                            }
                        }
                    }
                }
                if rejected {
                    store.state = ContactStoreState::Invalid;
                } else if count == 0 {
                    store.state = ContactStoreState::Empty;
                } else {
                    store.records = records;
                    store.len = count as u8;
                    // WHY the id cursor starts past whatever was loaded: re-using
                    // an id that is already on disk would make a later update
                    // ambiguous about which record it meant.
                    store.next_id = records
                        .iter()
                        .take(count)
                        .map(|r| r.id)
                        .max()
                        .unwrap_or(0)
                        .saturating_add(1);
                    store.state = ContactStoreState::Loaded;
                }
            }
            Err(code) => {
                store.state = ContactStoreState::ReadRefused;
                store.last_error = code;
            }
        }
        store
    }

    /// Encodes the durable list into one settings value and writes it.
    ///
    /// WHY the whole blob is rewritten every time: the store is small enough that
    /// a record-at-a-time append would need a cursor the settings store does not
    /// have, and a whole-value write leaves no half-updated record behind.
    ///
    /// `escape_store_blob` encodes any 0x0A bytes so the settings store never
    /// sees a newline, lifting the previous 8-contact limit and allowing full
    /// capacity up to `MAX_CONTACTS`.
    fn persist(&mut self) -> Result<(), i64> {
        let mut raw_blob = [0u8; STORE_BLOB_BYTES];
        raw_blob[0] = STORE_VERSION;
        raw_blob[1] = self.len;
        for i in 0..self.len as usize {
            let start = 2 + i * CONTACT_RECORD_BYTES;
            self.records[i].encode(&mut raw_blob[start..start + CONTACT_RECORD_BYTES]);
        }
        let raw_used = 2 + self.len as usize * CONTACT_RECORD_BYTES;
        let mut encoded = [0u8; STORE_BLOB_BYTES + 32];
        let encoded_len = escape_store_blob(&raw_blob[..raw_used], &mut encoded).map_err(|_| -22)?;
        if encoded[..encoded_len].iter().any(|&b| b == b'\n') {
            self.state = ContactStoreState::WriteRefused;
            self.last_error = -22;
            return Err(-22);
        }
        match std::settings_set(STORE_KEY, &encoded[..encoded_len]) {
            Ok(()) => {
                self.state = ContactStoreState::Written;
                self.last_error = 0;
                Ok(())
            }
            Err(code) => {
                self.state = ContactStoreState::WriteRefused;
                self.last_error = code;
                Err(code)
            }
        }
    }

    /// Adds a contact. The durable list only gains it once the write is
    /// acknowledged; otherwise it lands in `unsaved` and the refusal is recorded.
    fn add(&mut self, name: &[u8], number: &[u8]) -> Result<(), i64> {
        if name.is_empty() || number.is_empty() {
            return Err(-22);
        }
        if self.len as usize >= MAX_CONTACTS {
            // WHY the capacity is checked against the durable list only: a store
            // that is full of acknowledged contacts cannot take another one, but
            // unsaved records do not occupy storage, so they never make it full.
            self.state = ContactStoreState::Full;
            self.last_error = -28;
            return Err(-28);
        }
        // WHY a free id is searched after the monotonic cursor runs out: the
        // cursor only moves forward, so twelve saves over the life of the
        // device exhausted it even after contacts were deleted, and ADD then
        // refused forever with an empty-looking store. A deleted record's id
        // is free, and the selection and editor are cleared on delete, so
        // re-issuing it cannot collide with a live reference.
        let id = if self.next_id != 0 && self.next_id as usize <= MAX_CONTACTS {
            self.next_id
        } else {
            let mut free = 0u8;
            for candidate in 1..=MAX_CONTACTS as u8 {
                if !self.records[..self.len as usize]
                    .iter()
                    .any(|r| r.id == candidate)
                {
                    free = candidate;
                    break;
                }
            }
            if free == 0 {
                self.state = ContactStoreState::Full;
                self.last_error = -28;
                return Err(-28);
            }
            free
        };
        let mut rec = ContactRecord::from_parts(id, name, number, false);
        if !rec.valid() {
            return Err(-22);
        }
        // WHY the record is appended before the write: `persist` encodes the
        // durable list, so the new contact has to be in it for the write to mean
        // "this list, plus this contact, is now what storage holds".
        self.records[self.len as usize] = rec;
        self.len += 1;
        if id == self.next_id {
            self.next_id = self.next_id.saturating_add(1);
        }
        match self.persist() {
            Ok(()) => Ok(()),
            Err(code) => {
                // The write was refused, so the record must not stay in the
                // durable list: that list is a statement about storage. It moves
                // to `unsaved`, where it is drawn and labelled as session-only.
                self.len -= 1;
                self.records[self.len as usize] = ContactRecord::EMPTY;
                if (self.unsaved_len as usize) < MAX_CONTACTS {
                    rec.id = id;
                    self.unsaved[self.unsaved_len as usize] = rec;
                    self.unsaved_len += 1;
                }
                Err(code)
            }
        }
    }

    /// Replaces the record at `index` and writes the store.
    fn update(&mut self, index: usize, name: &[u8], number: &[u8]) -> Result<(), i64> {
        if index >= self.len as usize || name.is_empty() || number.is_empty() {
            return Err(-22);
        }
        let rec = ContactRecord::from_parts(self.records[index].id, name, number, self.records[index].is_favourite());
        if !rec.valid() {
            return Err(-22);
        }
        self.records[index] = rec;
        self.persist()
    }

    /// Removes the record at `index` and writes the store.
    fn remove(&mut self, index: usize) -> Result<(), i64> {
        if index >= self.len as usize {
            return Err(-22);
        }
        for i in index..self.len as usize - 1 {
            self.records[i] = self.records[i + 1];
        }
        self.records[self.len as usize - 1] = ContactRecord::EMPTY;
        self.len -= 1;
        self.persist()
    }

    /// Toggles the star on the record at `index` and writes the store.
    fn toggle_favourite(&mut self, index: usize) -> Result<(), i64> {
        if index >= self.len as usize {
            return Err(-22);
        }
        let on = !self.records[index].is_favourite();
        self.records[index].set_favourite(on);
        self.persist()
    }

    /// Removes an entry from the unsaved list, which needs no write because
    /// storage never held it.
    fn remove_unsaved(&mut self, index: usize) -> Result<(), i64> {
        if index >= self.unsaved_len as usize {
            return Err(-22);
        }
        for i in index..self.unsaved_len as usize - 1 {
            self.unsaved[i] = self.unsaved[i + 1];
        }
        self.unsaved[self.unsaved_len as usize - 1] = ContactRecord::EMPTY;
        self.unsaved_len -= 1;
        Ok(())
    }

    /// Appends to the unsaved list.
    ///
    /// WHY this exists separately from `add`: an entry that editing could not
    /// persist still has to be editable, and the unsaved list is the only place
    /// that can hold it without making a claim about storage.
    fn add_unsaved(&mut self, name: &[u8], number: &[u8]) -> Result<(), i64> {
        if name.is_empty() || number.is_empty() || self.unsaved_len as usize >= MAX_CONTACTS {
            return Err(-28);
        }
        let rec = ContactRecord::from_parts(0, name, number, false);
        if !rec.valid() {
            return Err(-22);
        }
        self.unsaved[self.unsaved_len as usize] = rec;
        self.unsaved_len += 1;
        Ok(())
    }

    fn len(&self) -> usize {
        self.len as usize
    }

    fn unsaved_len(&self) -> usize {
        self.unsaved_len as usize
    }

    fn record(&self, index: usize) -> Option<&ContactRecord> {
        if index < self.len as usize {
            Some(&self.records[index])
        } else {
            None
        }
    }

    fn unsaved_record(&self, index: usize) -> Option<&ContactRecord> {
        if index < self.unsaved_len as usize {
            Some(&self.unsaved[index])
        } else {
            None
        }
    }

    /// Index of the first contact whose number starts with `digits`.
    ///
    /// WHY a prefix and not an exact match: a phone shows the contact behind the
    /// digits typed so far, which is the difference between dialling a contact
    /// and dialling a number.
    fn match_by_number_prefix(&self, digits: &[u8]) -> Option<usize> {
        if digits.is_empty() {
            return None;
        }
        (0..self.len as usize).find(|&i| {
            let num = self.records[i].number_bytes();
            num.len() >= digits.len() && &num[..digits.len()] == digits
        })
    }

    /// Number of starred contacts, for the dock's Favorites badge.
    fn favourites(&self) -> usize {
        (0..self.len as usize)
            .filter(|&i| self.records[i].is_favourite())
            .count()
    }
}

/// Which field of the contact editor the caret is in.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ContactField {
    Name,
    Number,
}

/// The add/edit contact editor.
///
/// WHY the buffers are fixed arrays: `#![no_std]` with no allocator, so a text
/// field is a bounded byte array. The width is the same
/// `CONTACT_NAME_MAX`/`CONTACT_NUM_MAX` the storage record uses, so what the user
/// can type is exactly what the store can hold.
#[derive(Clone, Copy)]
pub struct ContactEditor {
    /// `None` while adding a new contact, `Some(i)` while editing durable record `i`,
    /// `Some(MAX_CONTACTS + i)` while editing an unsaved record. The tag keeps
    /// durable and unsaved records from ever addressing the same slot.
    target: Option<usize>,
    name: [u8; CONTACT_NAME_MAX],
    name_len: usize,
    number: [u8; CONTACT_NUM_MAX],
    number_len: usize,
    field: ContactField,
}

impl ContactEditor {
    /// Tag offset that marks a target as living in the unsaved list.
    const UNSAVED_BASE: usize = MAX_CONTACTS;

    const BLANK: ContactEditor = ContactEditor {
        target: None,
        name: [0; CONTACT_NAME_MAX],
        name_len: 0,
        number: [0; CONTACT_NUM_MAX],
        number_len: 0,
        field: ContactField::Name,
    };

    fn open_new() -> Self {
        ContactEditor::BLANK
    }

    fn open_with_number(number: &[u8]) -> Self {
        let mut ed = ContactEditor::BLANK;
        let m = number.len().min(CONTACT_NUM_MAX);
        ed.number[..m].copy_from_slice(&number[..m]);
        ed.number_len = m;
        ed.field = ContactField::Name;
        ed
    }

    fn open_record(rec: &ContactRecord, target: Option<usize>) -> Self {
        let mut ed = ContactEditor::BLANK;
        ed.target = target;
        let n = rec.name_bytes().len().min(CONTACT_NAME_MAX);
        ed.name[..n].copy_from_slice(&rec.name_bytes()[..n]);
        ed.name_len = n;
        let m = rec.number_bytes().len().min(CONTACT_NUM_MAX);
        ed.number[..m].copy_from_slice(&rec.number_bytes()[..m]);
        ed.number_len = m;
        ed
    }

    fn active_len(&self) -> usize {
        match self.field {
            ContactField::Name => self.name_len,
            ContactField::Number => self.number_len,
        }
    }

    fn active_cap(&self) -> usize {
        match self.field {
            ContactField::Name => CONTACT_NAME_MAX,
            ContactField::Number => CONTACT_NUM_MAX,
        }
    }

    /// Appends one byte to the focused field.
    ///
    /// WHY it refuses rather than truncates: a truncated number is a number the
    /// user did not type, and the store would then hold a contact that dials
    /// somewhere else.
    fn push(&mut self, byte: u8) {
        if self.active_len() >= self.active_cap() {
            return;
        }
        match self.field {
            ContactField::Name => {
                self.name[self.name_len] = byte;
                self.name_len += 1;
            }
            ContactField::Number => {
                self.number[self.number_len] = byte;
                self.number_len += 1;
            }
        }
    }

    fn backspace(&mut self) {
        match self.field {
            ContactField::Name => self.name_len = self.name_len.saturating_sub(1),
            ContactField::Number => self.number_len = self.number_len.saturating_sub(1),
        }
    }
}

/// Entries the recents ledger keeps. Bounded so the ledger stays a fixed-size
/// stack allocation; the oldest entry is dropped once it is full.
///
/// WHY sixteen and not five: the ledger is a call history, and a history of
/// five calls is not one. The platform's own call-log ledger holds sixteen
/// (see `dialer::MAX_CALL_LOGS`), and the list scrolls, so the entries below
/// the fold are reachable.
const RECENTS_CAP: usize = 16;

/// Which view of the settings surface is showing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettingsView {
    /// The dialer's own settings.
    Main,
    /// The ringtone picker, opened from the ringtone row.
    RingtonePicker,
}

/// Production Telephony Dialer state manager.
pub struct DialerApp {
    pub state: CallState,
    pub active_number: [u8; 32],
    pub active_len: usize,
    pub is_muted: bool,
    pub is_speaker: bool,
    pub calls_completed: u32,
    /// Whether the telephony service actually reported a baseband present.
    pub modem_present: bool,
    /// Section the tabs and the dock select.
    pub section: DialerSection,
    /// Real outcome of the last call attempt.
    pub outcome: CallOutcome,
    /// Signal bars as the modem HAL reported them (0..=4), never invented from
    /// the `present` boolean.
    pub signal_bars: u8,
    /// State of charge as `battery_query` reported it. `None` when the kernel
    /// refused or reported no fuel gauge, in which case nothing is drawn.
    pub battery_soc: Option<u8>,
    pub clock: [u8; 5],
    pub clock_ok: bool,
    /// Real ledger of dial attempts, newest first.
    pub recents: [CallRecord; RECENTS_CAP],
    pub recents_len: usize,
    /// How far the recents ledger is scrolled.
    pub recents_scroll: ScrollState,
    /// How far the contact list is scrolled.
    pub contacts_scroll: ScrollState,
    /// How far the ringtone picker is scrolled.
    pub ringtone_scroll: ScrollState,
    /// The user's contacts, read back out of the platform settings store.
    pub contacts: ContactStore,
    /// Row selected in the Contacts section, or `None` when nothing is selected.
    /// `Some(i)` addresses a durable record, `Some(UNSAVED_BASE + i)` an unsaved one.
    pub contact_sel: Option<usize>,
    /// `Some` while the contact editor is open, `None` in browse mode.
    pub contact_edit: Option<ContactEditor>,
    /// Which keypad cell produced the last typed character, so a repeat tap on
    /// the same cell cycles the character instead of appending a second one.
    ///
    /// WHY this is a cell and not a timeout: a timeout needs a tick rate the app
    /// does not know, and a lost timeout either duplicates a character or refuses
    /// one. "Same cell as last" is deterministic and needs no clock.
    pub editor_last_cell: Option<(usize, usize)>,
    /// True when SAVE was pressed on an incomplete contact. Cleared on the
    /// next keystroke or when the editor opens; drives the on-screen hint.
    pub editor_error: bool,
    /// True when the Contacts list shows only starred contacts. Set by the dock's
    /// Favorites slot, so that slot opens something real rather than reporting
    /// that no store exists.
    pub contacts_fav_only: bool,
    /// The name of the document the device's ringtone slot names.
    ///
    /// WHY the slot and not a ringtone this app chose: the ringtone is a
    /// device-wide choice, and the file manager and the gallery are the apps
    /// that put a document into it. This app plays whatever the slot holds, so
    /// it reads the slot and shows what is in it.
    pub ringtone: [u8; 64],
    /// How many bytes of the ringtone's name are meaningful.
    pub ringtone_len: usize,
    /// Absolute path of the confirmed ringtone document.
    pub ringtone_path: [u8; 128],
    pub ringtone_path_len: usize,
    /// Which view of the settings surface is showing.
    pub settings_view: SettingsView,
    /// The audio documents the platform's index holds, which the ringtone
    /// picker offers.
    pub ringtone_choices: [RingtoneChoice; RINGTONE_CHOICE_CAP],
    /// How many of the ringtone choices are real.
    pub ringtone_choice_count: usize,
    /// Whether the keypad sounds while a digit is entered.
    pub dtmf_enabled: bool,
    /// Whether the audio service has already refused a keypad tone this
    /// session, so the refusal is reported once rather than on every key.
    pub dtmf_refusal_reported: bool,
    /// The settings rows' values, pre-rendered.
    ///
    /// WHY they are pre-rendered: the renderer runs once per pixel of the
    /// surface, and formatting a value there would run tens of thousands of
    /// times for one row of text.
    pub settings_values: [[u8; 48]; 4],
    /// How many bytes of each settings value are used.
    pub settings_value_lens: [usize; 4],
}

/// How many audio documents the ringtone picker can hold.
pub const RINGTONE_CHOICE_CAP: usize = 8;

impl DialerApp {
    pub const fn new() -> Self {
        Self {
            state: CallState::Idle,
            active_number: [0u8; 32],
            active_len: 0,
            is_muted: false,
            is_speaker: false,
            calls_completed: 0,
            // WHY: fail closed. Until the telephony service reports a baseband,
            // no inbound-call path may be claimed.
            modem_present: false,
            section: DialerSection::Keypad,
            outcome: CallOutcome::Idle,
            signal_bars: 0,
            battery_soc: None,
            clock: [b' '; 5],
            clock_ok: false,
            recents: [CallRecord {
                number: [0; 32],
                len: 0,
                accepted: false,
                emergency: false,
                outcome: CallOutcome::Idle,
            }; RECENTS_CAP],
            recents_len: 0,
            recents_scroll: ScrollState::new(0, 0, 0),
            contacts_scroll: ScrollState::new(0, 0, 0),
            ringtone_scroll: ScrollState::new(0, 0, 0),
            // WHY the store is empty here and filled by `load()` in `_start`:
            // `new()` is a `const fn` so it can be called before the scheduler and
            // the storage service exist, and a syscall from it would fault. The
            // load must happen once the app is running, which `_start` does.
            contacts: ContactStore::EMPTY,
            contact_sel: None,
            contact_edit: None,
            editor_last_cell: None,
            editor_error: false,
            contacts_fav_only: false,
            ringtone: [0u8; 64],
            ringtone_len: 0,
            ringtone_path: [0u8; 128],
            ringtone_path_len: 0,
            settings_view: SettingsView::Main,
            ringtone_choices: [RingtoneChoice::EMPTY; RINGTONE_CHOICE_CAP],
            ringtone_choice_count: 0,
            dtmf_enabled: true,
            dtmf_refusal_reported: false,
            settings_values: [[0u8; 48]; 4],
            settings_value_lens: [0; 4],
        }
    }

    /// Reads the device's ringtone slot and confirms the document it names.
    ///
    /// WHY the document is confirmed against the platform's index and not
    /// taken on the slot's word: a slot that names a document the device does
    /// not hold is a slot that points at nothing, and the dialer must say so
    /// rather than show a name that cannot be played.
    pub fn refresh_ringtone(&mut self) {
        self.ringtone_len = 0;
        let mut buf = [0u8; 128];
        let Ok(n) = std::settings_get("audio.ringtone", &mut buf) else {
            std::log("[DIALER APP] The ringtone slot could not be read; the device's default ringtone is in use.").ok();
            return;
        };
        if n == 0 || n > buf.len() {
            return;
        }
        let Ok(path) = core::str::from_utf8(&buf[..n]) else {
            std::log("[DIALER APP] The ringtone slot does not hold a path; the device's default ringtone is in use.").ok();
            return;
        };
        let path = path.trim();
        if path.is_empty() {
            return;
        }
        let mut raw = [0u8; 2048];
        let Some(record) = std::content::content_index_find_path(path, &mut raw) else {
            std::log("[DIALER APP] The ringtone slot names a document the platform does not hold; the device's default ringtone is in use.").ok();
            return;
        };
        let name = record.path.rsplit('/').next().unwrap_or(&record.path);
        let len = name.len().min(self.ringtone.len());
        self.ringtone[..len].copy_from_slice(&name.as_bytes()[..len]);
        self.ringtone_len = len;
        let plen = record.path.len().min(self.ringtone_path.len());
        self.ringtone_path[..plen].copy_from_slice(&record.path.as_bytes()[..plen]);
        self.ringtone_path_len = plen;
        std::log("[DIALER APP] The ringtone slot names a document the platform holds.").ok();
    }

    /// Plays the currently configured ringtone or fallback alert tone.
    pub fn play_ringtone(&mut self) {
        if self.ringtone_path_len > 0 {
            if let Ok(path_str) = core::str::from_utf8(&self.ringtone_path[..self.ringtone_path_len]) {
                if let Ok(cap) = std::capability_handle(std::cap_type::AUDIO_STREAM, 0) {
                    if std::audio_stream_open(cap, std::AUDIO_STREAM_FORMAT_NATIVE).is_ok() {
                        if let Ok(fd) = std::fs_open(path_str, std::FS_READ) {
                            let mut buf = [0u8; 4096];
                            for _ in 0..16 {
                                match std::fs_read(fd, &mut buf) {
                                    Ok(0) => break,
                                    Ok(n) => {
                                        let _ = std::audio_stream_write(cap, &buf[..n]);
                                    }
                                    Err(_) => break,
                                }
                            }
                            let _ = std::fs_close(fd);
                            std::log("[DIALER APP] Playing configured ringtone over audio stream.").ok();
                            return;
                        }
                    }
                }
            }
        }
        if std::audio_play_tone(880).is_ok() {
            let _ = std::audio_silence();
            std::log("[DIALER APP] Emitted fallback ringtone alert tone.").ok();
        } else {
            std::log("[DIALER APP] Ringtone playback skipped: audio service unavailable.").ok();
        }
    }

    /// Plays notification audio feedback.
    pub fn play_notification(&mut self) {
        if let Ok(cap) = std::capability_handle(std::cap_type::AUDIO_STREAM, 0) {
            if std::audio_stream_open(cap, std::AUDIO_STREAM_FORMAT_NATIVE).is_ok() {
                let mut pcm = [0i16; 512];
                for i in 0..256 {
                    let val = if (i / 16) % 2 == 0 { 4000i16 } else { -4000i16 };
                    pcm[i * 2] = val;
                    pcm[i * 2 + 1] = val;
                }
                let bytes = unsafe {
                    core::slice::from_raw_parts(pcm.as_ptr() as *const u8, pcm.len() * 2)
                };
                let _ = std::audio_stream_write(cap, bytes);
                std::log("[DIALER APP] Notification chime streamed over audio output.").ok();
                return;
            }
        }
        if std::audio_play_tone(1046).is_ok() {
            let _ = std::audio_silence();
            std::log("[DIALER APP] Notification chime tone played.").ok();
        } else {
            std::log("[DIALER APP] Notification chime skipped: audio service unavailable.").ok();
        }
    }

    /// Simulates an incoming phone call from caller.
    pub fn simulate_incoming_call(&mut self, caller: &[u8]) {
        self.state = CallState::Ringing;
        self.active_len = 0;
        let n = caller.len().min(self.active_number.len());
        self.active_number[..n].copy_from_slice(&caller[..n]);
        self.active_len = n;
        self.play_ringtone();
        std::log("[DIALER APP] Incoming call simulated; ringing state active.").ok();
    }

    /// Plays dual-tone multi-frequency acoustic feedback for a pressed keypad digit.
    ///
    /// WHY the refusal is reported: this task holds no audio authority, so the
    /// audio service refuses the tone with PermissionDenied. The old code
    /// discarded the answer, so a keypad that could not be heard looked exactly
    /// like one that could. The refusal is reported once per session, because a
    /// report on every key press is noise rather than information.
    pub fn play_dtmf(&mut self, digit: char) {
        if !self.dtmf_enabled {
            return;
        }
        if let Some(pair) = get_dtmf_frequencies(digit) {
            // WHY only the high tone: the audio service exposes a single
            // oscillator (`audio_play_tone`), so only one of the two CCITT
            // frequencies can ever be emitted. The low tone is reported by
            // `get_dtmf_frequencies` and drawn in the key sublabel, never
            // claimed to have sounded.
            match std::audio_play_tone(pair.high_freq) {
                Ok(()) => {
                    std::audio_haptic_click().ok();
                    std::audio_silence().ok();
                }
                Err(code) => {
                    if !self.dtmf_refusal_reported {
                        self.dtmf_refusal_reported = true;
                        log_with_code(
                            "[DIALER APP] The audio service refused the keypad tone, code",
                            code,
                        );
                        std::log("[DIALER APP] This app holds no audio authority, so the keypad tone is silent; the key is still shown.")
                            .ok();
                    }
                }
            }
        }
    }

    /// Recomputes the scroll extents of the lists.
    ///
    /// WHY this is a step and not something the renderer does: the extents are
    /// a fact about how many rows there are and how tall the window is, and the
    /// renderer runs once per pixel of the surface.
    pub fn refresh_scrolls(&mut self) {
        let recents = recents_scroll_extents(self.recents_len);
        self.recents_scroll = recents.state();
        let rows = self.contacts_visible_rows();
        let contacts = contacts_scroll_extents(rows);
        self.contacts_scroll = contacts.state();
    }

    /// How many contact rows the list shows, after the favourites filter.
    ///
    /// WHY the filter is counted here and not in the renderer: a filtered-out
    /// row is not a row, and a scroll extent that counted it would offer the
    /// user a thumb for content that is not there.
    pub fn contacts_visible_rows(&self) -> usize {
        let mut count = 0usize;
        for i in 0..self.contacts.len() {
            if let Some(record) = self.contacts.record(i) {
                if !self.contacts_fav_only || record.is_favourite() {
                    count += 1;
                }
            }
        }
        for i in 0..self.contacts.unsaved_len() {
            if let Some(record) = self.contacts.unsaved_record(i) {
                if !self.contacts_fav_only || record.is_favourite() {
                    count += 1;
                }
            }
        }
        count
    }

    /// Reads the audio documents the platform's index holds.
    ///
    /// WHY the index and not a list this app keeps: the ringtone slot names a
    /// document on the device, and the documents on the device are the
    /// platform's. A picker over an app's own list would offer files the
    /// platform does not hold.
    pub fn refresh_ringtone_choices(&mut self) {
        self.ringtone_choice_count = 0;
        let mut raw = [0u8; 2048];
        match std::content::content_index_search_mime("audio/", &mut raw) {
            Ok(n) => {
                self.ringtone_choice_count = parse_audio_documents(
                    &raw[..n],
                    &mut self.ringtone_choices,
                );
                let count = self.ringtone_choice_count;
                std::log("[DIALER APP] The platform's index holds").ok();
                let mut buf = [0u8; 16];
                let written = put_decimal(count as u32, &mut buf);
                if let Ok(text) = core::str::from_utf8(&buf[..written]) {
                    std::log(text).ok();
                }
                std::log("[DIALER APP] audio documents for the ringtone picker.").ok();
            }
            Err(code) => {
                log_with_code(
                    "[DIALER APP] The platform refused the document index query, code",
                    code,
                );
            }
        }
        let rows = self.ringtone_choice_count;
        self.ringtone_scroll = crate::ringtone_scroll_extents(rows);
    }

    /// Writes a document into the device's ringtone slot.
    ///
    /// WHY the slot and not a local preference: the ringtone is a device-wide
    /// choice, and the settings app, the file manager and the gallery all read
    /// the same slot.
    pub fn set_ringtone(&mut self, path: &[u8]) -> Result<(), i64> {
        std::settings_set("audio.ringtone", path)?;
        self.refresh_ringtone();
        Ok(())
    }

    /// Applies a drag to the list under the pointer, if there is one.
    ///
    /// WHY the gesture scrolls rather than leaving the app: a drag on a list
    /// is how a list is read, and the platform's own apps treat a pan over a
    /// list as a scroll. The scroll model owns the offset and the thumb, so
    /// the two cannot disagree.
    pub fn apply_scroll(&mut self, _x: usize, y: usize, dy: i32) -> bool {
        match self.section {
            DialerSection::Recents => {
                if y >= RECENTS_Y0 && y < RECENTS_VIEWPORT_Y1 {
                    self.recents_scroll.apply_drag(dy);
                    return true;
                }
            }
            DialerSection::Contacts => {
                // The editor is a text entry, so a drag there is not a scroll.
                if self.contact_edit.is_none() && y >= LIST_Y0 && y < LIST_VIEWPORT_Y1 {
                    self.contacts_scroll.apply_drag(dy);
                    return true;
                }
            }
            DialerSection::Settings => {
                if self.settings_view == SettingsView::RingtonePicker
                    && y >= RINGTONE_ROW_Y0
                    && y < RINGTONE_VIEWPORT_Y1
                {
                    self.ringtone_scroll.apply_drag(dy);
                    return true;
                }
            }
            DialerSection::Keypad => {}
        }
        false
    }

    /// Turns the keypad tone on or off and stores the choice.
    pub fn toggle_dtmf(&mut self) -> Result<bool, i64> {
        let next = !self.dtmf_enabled;
        set_dtmf_enabled(next)?;
        self.dtmf_enabled = next;
        Ok(next)
    }

    /// Rebuilds the settings surface's values.
    pub fn refresh_settings_values(&mut self) {
        let mut scratch = [0u8; 48];
        for (index, row) in SettingsRow::ORDER.iter().enumerate() {
            let len = self.settings_row_value(*row, &mut scratch);
            self.settings_value_lens[index] = len;
            let n = len.min(self.settings_values[index].len());
            self.settings_values[index][..n].copy_from_slice(&scratch[..n]);
        }
    }

    /// The value of a settings row.
    ///
    /// WHY each value is a fact the platform supplied: the ringtone is the
    /// document the device's slot names, the keypad tone is the preference the
    /// settings store holds, and the two rows the platform cannot answer say
    /// so rather than showing a value that would be the app's own invention.
    fn settings_row_value(&self, row: SettingsRow, out: &mut [u8; 48]) -> usize {
        match row {
            SettingsRow::Ringtone => {
                if self.ringtone_len == 0 {
                    copy_bytes(out, b"None chosen")
                } else {
                    copy_bytes(out, &self.ringtone[..self.ringtone_len])
                }
            }
            SettingsRow::KeypadTone => {
                copy_bytes(out, if self.dtmf_enabled { b"On" } else { b"Off" })
            }
            SettingsRow::DefaultSim => copy_bytes(out, b"One SIM, no choice"),
            SettingsRow::CallDuration => copy_bytes(out, b"Not reported"),
        }
    }

    /// Appends a dialed digit to the accumulator and plays its DTMF tone.
    pub fn input_digit(&mut self, digit: char) {
        // WHY entry is frozen while a call is up: the keypad is a DTMF pad
        // during a call, and appending the tones to the number on the card
        // claimed the call's number had changed.
        if self.state != CallState::Idle {
            self.play_dtmf(digit);
            return;
        }
        if self.active_len < self.active_number.len() {
            // WHY the last outcome is dropped on edit: a refusal caption is a
            // fact about the number that was dialed. Once the person changes
            // that number, leaving the caption underneath it claims the new
            // number was the one refused. An in-call number is not editable.
            self.outcome = CallOutcome::Idle;
            self.active_number[self.active_len] = digit as u8;
            self.active_len += 1;
            self.play_dtmf(digit);
        }
    }

    /// Removes the last entered digit.
    pub fn backspace(&mut self) {
        if self.state != CallState::Idle {
            return;
        }
        if self.active_len > 0 {
            if self.state == CallState::Idle {
                self.outcome = CallOutcome::Idle;
            }
            self.active_len -= 1;
            self.active_number[self.active_len] = 0;
            let _ = std::audio_haptic_click();
        }
    }

    /// Clears the entry without placing a call.
    pub fn clear_entry(&mut self) {
        if self.active_len > 0 {
            if self.state == CallState::Idle {
                self.outcome = CallOutcome::Idle;
            }
            self.active_number.fill(0);
            self.active_len = 0;
            let _ = std::audio_haptic_click();
        }
    }

    /// Checks for a phone number handed over by another application (e.g. Messages)
    /// in `dialer.handoff.number.v1` and populates `active_number`.
    pub fn take_handoff_number(&mut self) {
        let mut buf = [0u8; 32];
        match std::settings_get("dialer.handoff.number.v1", &mut buf) {
            Ok(n) if n > 0 => {
                let n = n.min(self.active_number.len()).min(buf.len());
                self.active_number[..n].copy_from_slice(&buf[..n]);
                self.active_len = n;
                self.section = DialerSection::Keypad;
                let _ = std::settings_set("dialer.handoff.number.v1", &[]);
                std::log("[DIALER APP] Number handed off from another app; populated on keypad.").ok();
            }
            _ => {}
        }
    }

    /// Appends one real dial attempt to the recents ledger, newest first.
    fn record_call(&mut self, number: &str, outcome: CallOutcome, emergency: bool) {
        let n = number.len().min(self.active_number.len());
        let mut entry = CallRecord {
            number: [0u8; 32],
            len: n,
            accepted: !matches!(
                outcome,
                CallOutcome::Refused | CallOutcome::EmergencyRefused
            ),
            emergency,
            outcome,
        };
        let mut idx = 0usize;
        while idx < n {
            entry.number[idx] = number.as_bytes()[idx];
            idx += 1;
        }
        if self.recents_len < RECENTS_CAP {
            self.recents_len += 1;
            for i in (1..self.recents_len).rev() {
                self.recents[i] = self.recents[i - 1];
            }
        } else {
            // The ledger is full: the oldest entry is dropped so the newest
            // attempt is always the one on screen.
            for i in (1..RECENTS_CAP).rev() {
                self.recents[i] = self.recents[i - 1];
            }
        }
        self.recents[0] = entry;
        // WHY the scroll is recomputed: the ledger grew, so the list is
        // taller than its window and the thumb has to say so.
        self.refresh_scrolls();
    }

    /// Initiates an outgoing cellular call.
    pub fn dial(&mut self, number: &str) -> Result<(), i64> {
        self.dial_internal(number, false)
    }

    /// Places an emergency call after entering the real emergency number.
    ///
    /// The platform's emergency pipeline is E911/112; the digits below are the
    /// number that is actually sent, so the ledger and the status line can both
    /// name it.
    pub fn dial_emergency(&mut self) -> Result<(), i64> {
        self.active_len = 0;
        self.input_digit('9');
        self.input_digit('1');
        self.input_digit('1');
        self.dial_internal("911", true)
    }

    fn dial_internal(&mut self, number: &str, emergency: bool) -> Result<(), i64> {
        if number.is_empty() || number.len() > self.active_number.len() {
            return Err(-22);
        }

        self.state = CallState::Dialing;
        self.outcome = CallOutcome::Dialing;

        // A call is connected only when the modem itself says so. The
        // telephony service accepts the request and puts the modem into its
        // own dialling state, and the old app reported that acceptance as a
        // connected call. The modem's own answer is read instead.
        match std::telephony_dial(number) {
            Ok(()) => {
                let modem = query_modem_call_state();
                self.state = match modem {
                    ModemCallState::Connected => CallState::ActiveCall,
                    // WHY an ended modem state is idle: the call is over, so
                    // the central control has to offer a call again. CallEnded
                    // only differed from Idle by keeping the red hangup glyph.
                    ModemCallState::Ended => CallState::Idle,
                    _ => CallState::Dialing,
                };
                self.outcome = match modem {
                    ModemCallState::Connected => CallOutcome::Connected,
                    ModemCallState::Ended => CallOutcome::Ended,
                    ModemCallState::Dialing => CallOutcome::Dialing,
                    _ => CallOutcome::Unconfirmed,
                };
                // WHY there is no tone here: the app used to play a bare 440 Hz
                // tone on every accepted request. That tone was a placeholder
                // for a ring-back that this platform does not have, it was not
                // described as one, and the audio service refuses the call
                // anyway because this task holds no audio authority - so the
                // screen said "calling" and the device said nothing. The
                // call's own state is on the card instead.
                std::log("[DIALER APP] Telephony service accepted the call request.").ok();
                if self.outcome == CallOutcome::Connected {
                    std::log("[DIALER APP] The modem reported the call as connected. The platform's telephony telemetry carries no call duration, so the app shows none.",
                    )
                    .ok();
                }
                self.record_call(number, self.outcome, emergency);
                Ok(())
            }
            Err(err) => {
                // No call exists after a refusal, so the central control
                // returns to the call glyph; the refusal stays on the card.
                self.state = CallState::Idle;
                self.outcome = if emergency {
                    CallOutcome::EmergencyRefused
                } else {
                    CallOutcome::Refused
                };
                std::log("[DIALER APP] Call request rejected by telephony service.").ok();
                self.record_call(number, self.outcome, emergency);
                Err(err)
            }
        }
    }

    /// Terminates an active call.
    ///
    /// Only a call that actually reached the telephony service counts as
    /// completed and only that case releases a channel.
    pub fn hangup(&mut self) {
        let had_connected_call = self.state == CallState::ActiveCall;
        self.state = CallState::CallEnded;
        let _ = std::audio_haptic_click();
        self.state = CallState::Idle;
        self.active_len = 0;
        if had_connected_call {
            self.calls_completed += 1;
            // WHY: this line used to claim the channel was released to the
            // modem pool, but hangup() issued no telephony syscall at all, so no
            // modem state was ever touched. Ask the service and report the
            // outcome it actually returned.
            if std::telephony_hangup().is_ok() {
                std::log("[DIALER APP] Call Terminated: telephony service released the channel.",
                )
                .ok();
            } else {
                std::log("[DIALER APP] Call Terminated locally: the telephony service refused the hangup, so no channel was released.",
                )
                .ok();
            }
        } else {
            // WHY: no call was ever established, so there is no channel to
            // release and no completed call to count. Reporting a released
            // channel here would be a success claim for a rejected operation.
            std::log("[DIALER APP] Hangup with no connected call; no channel was released.",
            )
            .ok();
        }
        // WHY the outcome is cleared: the card returns to the keypad, and a
        // leftover "Call ended" or refusal caption would describe a call that
        // no longer exists. The recents ledger keeps the record.
        self.outcome = CallOutcome::Idle;
    }

    /// Steps the section. All four tabs are live now.
    pub fn step_section(&mut self) -> DialerSection {
        self.section = match self.section {
            DialerSection::Keypad => DialerSection::Recents,
            DialerSection::Recents => DialerSection::Contacts,
            DialerSection::Contacts => DialerSection::Settings,
            DialerSection::Settings => DialerSection::Keypad,
        };
        if self.section == DialerSection::Settings {
            self.settings_view = SettingsView::Main;
            self.refresh_settings_values();
        }
        self.section
    }

    /// Opens the Contacts section, optionally filtered to starred contacts.
    pub fn open_contacts(&mut self, favourites_only: bool) {
        self.section = DialerSection::Contacts;
        self.contacts_fav_only = favourites_only;
        self.contact_edit = None;
        self.contact_sel = None;
        self.editor_last_cell = None;
        // WHY the scrolls are recomputed here: the favourites filter changes
        // how many rows the list has, and a scroll extent that counted a
        // filtered-out row would offer a thumb for content that is not there.
        self.refresh_scrolls();
    }

    /// Opens the editor for a new contact.
    pub fn start_add_contact(&mut self) {
        self.contact_edit = Some(ContactEditor::open_new());
        self.editor_last_cell = None;
        self.editor_error = false;
    }

    /// Opens the editor for a new contact with pre-filled dialed phone number.
    pub fn start_add_contact_with_number(&mut self, number: &[u8]) {
        self.section = DialerSection::Contacts;
        self.contact_edit = Some(ContactEditor::open_with_number(number));
        self.editor_last_cell = None;
        self.editor_error = false;
    }

    /// Opens the editor on an existing contact.
    ///
    /// WHY the index is carried, not the record: an edit must write back to the
    /// row the user tapped, and an index is the only thing that still identifies
    /// that row after a delete moves the others.
    pub fn start_edit_contact(&mut self, index: usize) {
        self.editor_error = false;
        if index >= ContactEditor::UNSAVED_BASE {
            let inner = index - ContactEditor::UNSAVED_BASE;
            match self.contacts.unsaved_record(inner) {
                Some(rec) => {
                    self.contact_edit =
                        Some(ContactEditor::open_record(rec, Some(index)));
                    self.editor_last_cell = None;
                }
                None => {
                    std::log("[DIALER APP] Edit requested for a contact row that no longer exists; the editor was not opened.",
                    )
                    .ok();
                }
            }
            return;
        }
        match self.contacts.record(index) {
            Some(rec) => {
                self.contact_edit =
                    Some(ContactEditor::open_record(rec, Some(index)));
                self.editor_last_cell = None;
            }
            None => {
                std::log("[DIALER APP] Edit requested for a contact row that no longer exists; the editor was not opened.",
                )
                .ok();
            }
        }
    }

    /// Closes the editor, discarding what was typed.
    pub fn cancel_edit_contact(&mut self) {
        self.contact_edit = None;
        self.editor_last_cell = None;
    }

    /// Commits the editor.
    ///
    /// WHY the outcome is logged either way: a save the kernel refuses must be as
    /// loud as a save it accepts, or the user cannot tell that the contact they
    /// just added will not survive a reboot.
    pub fn commit_edit_contact(&mut self) {
        let Some(ed) = self.contact_edit else {
            return;
        };
        if ed.name_len == 0 || ed.number_len == 0 {
            self.editor_error = true;
            std::log("[DIALER APP] Contact not saved: a contact needs both a name and a number.",
            )
            .ok();
            return;
        }
        self.editor_error = false;
        let name = &ed.name[..ed.name_len];
        let number = &ed.number[..ed.number_len];
        let outcome = match ed.target {
            Some(t) if t >= ContactEditor::UNSAVED_BASE => {
                let inner = t - ContactEditor::UNSAVED_BASE;
                // WHY an edit of an unsaved contact is written as a fresh entry
                // in the unsaved list rather than an update of it: the unsaved
                // list is not a statement about storage, so it can be rewritten
                // freely, and the entry keeps its place in the same list.
                let _ = inner;
                self.contacts.add_unsaved(name, number)
            }
            Some(t) => self.contacts.update(t, name, number),
            None => self.contacts.add(name, number),
        };
        self.contact_edit = None;
        self.editor_last_cell = None;
        match outcome {
            Ok(()) => {
                // WHY the filter is cleared and the new row selected: SAVE used
                // to close the editor with no visible change, and a new contact
                // is never a favourite, so under the Favorites filter the list
                // it had just been added to still looked empty.
                self.contacts_fav_only = false;
                if ed.target.is_none() {
                    self.contact_sel = self.contacts.len().checked_sub(1);
                }
                self.refresh_scrolls();
                std::log("[DIALER APP] Contact written to the settings store and acknowledged.",
                )
                .ok();
            }
            Err(_) => {
                std::log("[DIALER APP] Contact was NOT saved: the kernel refused the settings write. It is held in the unsaved group for this session only.",
                )
                .ok();
            }
        }
    }

    /// Deletes the selected contact.
    pub fn delete_selected_contact(&mut self) {
        let Some(sel) = self.contact_sel else {
            return;
        };
        if sel >= ContactEditor::UNSAVED_BASE {
            let inner = sel - ContactEditor::UNSAVED_BASE;
            match self.contacts.remove_unsaved(inner) {
                Ok(()) => {
                    std::log("[DIALER APP] Unsaved contact dropped. It was never written to storage, so nothing had to be removed from it.",
                    )
                    .ok();
                    self.contact_sel = None;
                }
                Err(_) => {
                    std::log("[DIALER APP] The contact row tapped no longer exists; nothing was deleted.",
                    )
                    .ok();
                }
            }
            return;
        }
        match self.contacts.remove(sel) {
            Ok(()) => {
                std::log("[DIALER APP] Contact removed and the store write was acknowledged.",
                )
                .ok();
                self.contact_sel = None;
            }
            Err(_) => {
                std::log("[DIALER APP] NOT deleted from storage: the kernel refused the store write, so the stored copy is unchanged.",
                )
                .ok();
            }
        }
    }

    /// Toggles the star on the selected contact.
    pub fn toggle_selected_favourite(&mut self) {
        let Some(sel) = self.contact_sel else {
            return;
        };
        if sel >= ContactEditor::UNSAVED_BASE {
            std::log("[DIALER APP] A contact that was never saved cannot be starred in storage; the star stays with the saved copy.",
            )
            .ok();
            return;
        }
        match self.contacts.toggle_favourite(sel) {
            Ok(()) => {
                std::log("[DIALER APP] Favourite flag written and acknowledged.").ok();
            }
            Err(_) => {
                std::log("[DIALER APP] Favourite flag changed for this session only: the kernel refused the store write.",
                )
                .ok();
            }
        }
    }

    /// Fills the entry with the selected contact's number and returns to the
    /// keypad, ready to dial.
    ///
    /// WHY this is how "dial a contact by name" works: the call control, the
    /// emergency path and the recents ledger all take the entered number, so a
    /// contact dial goes through exactly the same real `telephony_dial` request a
    /// typed number does. Nothing about the contact changes what is dialled.
    pub fn stage_selected_contact_number(&mut self) {
        let Some(sel) = self.contact_sel else {
            return;
        };
        let rec = if sel >= ContactEditor::UNSAVED_BASE {
            self.contacts
                .unsaved_record(sel - ContactEditor::UNSAVED_BASE)
        } else {
            self.contacts.record(sel)
        };
        let Some(rec) = rec else {
            return;
        };
        let number = rec.number_bytes();
        let n = number.len().min(self.active_number.len());
        self.active_number[..n].copy_from_slice(&number[..n]);
        self.active_len = n;
        self.section = DialerSection::Keypad;
        std::log("[DIALER APP] Contact number placed in the entry; press the call control to dial it.",
        )
        .ok();
    }

    /// Dials the selected contact's number immediately.
    pub fn dial_selected_contact(&mut self) {
        self.stage_selected_contact_number();
        let n = self.active_len;
        let mut buf = [0u8; 32];
        buf[..n].copy_from_slice(&self.active_number[..n]);
        let number = core::str::from_utf8(&buf[..n]).unwrap_or("");
        // `emergency` is false: this is an ordinary call to the number the user
        // chose. The emergency path is reached only through the SOS disc, so a
        // contact dial can never present itself as an emergency call.
        let _ = self.dial_internal(number, false);
        if self.outcome == CallOutcome::EmergencyRefused {
            std::log("[DIALER APP] The telephony service refused this call; the outcome is shown under the number.",
            )
            .ok();
        } else {
            std::log("[DIALER APP] The telephony service accepted the call request.").ok();
        }
    }

    // ---- Editor input: multi-tap over the real dial pad ---------------------

    /// Characters one keypad cell cycles through, in tap order.
    ///
    /// WHY the digit closes each cycle: this is exactly how a phone keypad has
    /// always worked, and it means the name field can reach its own digits
    /// without a second keyboard.
    fn t9_cycle(row: usize, col: usize) -> &'static [u8] {
        match get_keypad_char(row, col) {
            Some('1') => b"1",
            Some('2') => b"ABC2",
            Some('3') => b"DEF3",
            Some('4') => b"GHI4",
            Some('5') => b"JKL5",
            Some('6') => b"MNO6",
            Some('7') => b"PQRS7",
            Some('8') => b"TUV8",
            Some('9') => b"WXYZ9",
            Some('*') => b"*",
            // WHY the space leads: a contact name is the only field with more
            // than one word in it, so the first press should be the common case.
            Some('0') => b" 0",
            Some('#') => b"#",
            _ => b"",
        }
    }

    /// Feeds one keypad press into the open editor.
    ///
    /// In the name field the cell multi-taps: pressing the same cell again
    /// replaces the character it just typed with the next one in its cycle, and
    /// pressing a different cell appends. In the number field the keypad behaves
    /// exactly as it does on the main screen, so a number is typed the same way
    /// everywhere in the app.
    fn editor_press_cell(&mut self, row: usize, col: usize) {
        let Some(mut ed) = self.contact_edit else {
            return;
        };
        match ed.field {
            ContactField::Number => {
                if let Some(digit) = get_keypad_char(row, col) {
                    ed.push(digit as u8);
                }
                self.contact_edit = Some(ed);
                self.editor_last_cell = None;
            }
            ContactField::Name => {
                let cycle = Self::t9_cycle(row, col);
                if !cycle.is_empty() {
                    let repeats = self.editor_last_cell == Some((row, col));
                    if repeats && ed.name_len > 0 {
                        // WHY the current character is looked up in this cell's
                        // own cycle: it is the only cycle that could have produced
                        // it, so a miss means the character came from somewhere
                        // else and must be appended to rather than replaced.
                        let current = ed.name[ed.name_len - 1];
                        let at = cycle.iter().position(|&b| b == current).unwrap_or(0);
                        ed.name[ed.name_len - 1] = cycle[(at + 1) % cycle.len()];
                    } else {
                        ed.push(cycle[0]);
                    }
                    self.editor_last_cell = Some((row, col));
                }
                self.contact_edit = Some(ed);
            }
        }
    }
}

// -----------------------------------------------------------------------------
// Visual Design Tokens
//
// WHY there is no palette here any more: the app used to carry its own
// colours, which is a second design system inside one app. Every colour now
// resolves through the platform's theme, so a third-party app that asks the
// SDK for the same roles draws the same dialer. See `dialer::theme` for the
// mapping from the old names to the platform's roles.
// -----------------------------------------------------------------------------

// -----------------------------------------------------------------------------
// Canonical InterOS Typography
// -----------------------------------------------------------------------------

/// Anti-aliased run draw, clipped to the run's own line box.
///
/// `bg` is the local fill the text actually sits on â€” a tab capsule, the SOS
/// disc, the keypad disc, the dock plate â€” not the page background. Blending
/// every glyph edge toward one hardcoded colour tinted the AA fringe of any run
/// drawn on a darker local fill.
///
/// `max_w` is the width of the column the run is drawn in. WHY it is a
/// parameter: the outline face advances by the designer's table and the bitmap
/// face by the cell, so a run that was ellipsized against the bitmap measure
/// can be wider in the outline face than the column it was given. The platform's
/// own placement refuses a run that does not fit the column it was told about,
/// and the bitmap face - which respects the ellipsize - draws it instead.
#[inline(always)]
fn test_text(
    x: usize,
    y: usize,
    x0: usize,
    y0: usize,
    scale: usize,
    text: &[u8],
    max_w: usize,
    color: u32,
    bg: u32,
) -> Option<u32> {
    if scale == 0 || text.is_empty() {
        return None;
    }
    // The outline face, chosen ONCE for the run rather than once per pixel.
    if let Some(run) = outline_text::place_run(x0, y0, scale, text, Some(max_w), false) {
        let (rx, rtop, rw, rh) = run.bounds();
        if x >= rx && x - rx < rw && y >= rtop && y - rtop < rh {
            return run.pixel(x, y, color, bg);
        }
        return None;
    }
    // Bitmap fallback box
    if x >= x0 && x < x0 + max_w && y >= y0 && y < y0 + run_rows(scale) {
        return std::design::font::test_text_aa(x, y, x0, y0, scale, text, color, bg, false);
    }
    None
}

#[inline(always)]
fn test_text_bold(
    x: usize,
    y: usize,
    x0: usize,
    y0: usize,
    scale: usize,
    text: &[u8],
    max_w: usize,
    color: u32,
    bg: u32,
) -> Option<u32> {
    if scale == 0 || text.is_empty() {
        return None;
    }
    if let Some(run) = outline_text::place_run(x0, y0, scale, text, Some(max_w), true) {
        let (rx, rtop, rw, rh) = run.bounds();
        if x >= rx && x - rx < rw && y >= rtop && y - rtop < rh {
            return run.pixel(x, y, color, bg);
        }
        return None;
    }
    if x >= x0 && x < x0 + max_w && y >= y0 && y < y0 + run_rows(scale) {
        return std::design::font::test_text_aa(x, y, x0, y0, scale, text, color, bg, true);
    }
    None
}

/// The backdrop colour of a contact's initial disc.
///
/// WHY this is one neutral colour and not a hash of the name: a colour
/// picked from the name looks like a contact's own chosen identity, and the
/// app has never received a contact photo or a profile colour. The disc
/// shows the contact's real initial against a neutral surface.
fn contact_avatar_color(_name: &[u8]) -> u32 {
    THEME.on_surface // Slate
}

/// Rows a run at `scale` reserves: the platform's own line box for that rung,
/// which is the outline face's ascent plus descent rather than the bitmap cell.
/// WHY a local `const fn`: every run box below is derived from a compile-time
/// constant, and this names the rung's box once for all of them.
const fn run_rows(scale: usize) -> usize {
    std::design::font::line_height(scale)
}

/// The row the app's own chrome starts at. WHY not the top of the panel: the
/// status band owns `0..STATUS_BAR_HEIGHT` and answers the pixel function before
/// any header content is reached, so a control authored at y=24 lost its top half
/// to a surface painted over it.
const HEADER_TOP: usize = interos_std::design::viewport::HEADER_TOP;

/// Header band height: y >= HEADER_H draws the hairline rule, so a header run
/// has to fit inside 0..HEADER_H.
///
/// WHY 112 and not 94: the back control is 48 tall and starts at `HEADER_TOP`, so
/// its last row is 103 and the rule has to fall below that.
const HEADER_H: usize = 112;
const TITLE_SCALE: usize = type_scale::TITLE;
/// The row the header's controls are centred on.
const HEADER_ROW: usize = HEADER_TOP + std::design::back_button::SIZE / 2;
/// The title's line box top.
///
/// WHY the caps are centred on the control's row rather than the box: the box
/// at this rung is sixty-three rows and the button beside it is forty-eight, so
/// centring the box puts the letters below the button and keeping the box
/// inside the row puts them above it. Centring the cap height is the one
/// placement where the word and the button read as a single row of chrome.
const TITLE_Y0: usize = HEADER_ROW
    - std::design::font::ascent_for_scale(TITLE_SCALE)
    + std::design::font::cap_for_scale(TITLE_SCALE) as usize / 2;

/// The header title's column, named so the ringtone caption can be bounded
/// against its right edge rather than against a number written down beside it.
const HEADER_TITLE_X0: usize = 120;
const HEADER_TITLE_MAX_W: usize = 240;
/// The ringtone caption's trailing edge: the signal bars begin just right of it.
const RINGTONE_X1: usize = 600;
/// WHY derived from the title: the caption is right-aligned to `RINGTONE_X1`, so
/// a caption no wider than the gap between that edge and the title's right edge
/// can never start where the title's ink is. This is the one bound both the
/// placer and the drawer use, so a name neither silently re-faces at another
/// size nor lands inside the title.
const RINGTONE_MAX_W: usize = RINGTONE_X1 - (HEADER_TITLE_X0 + HEADER_TITLE_MAX_W);
const _: () =
    assert!(RINGTONE_X1 - RINGTONE_MAX_W >= HEADER_TITLE_X0 + HEADER_TITLE_MAX_W);

// Segmented navigation tabs (y: 120..164). Symmetrically balanced pills across
// the 720px viewport (48px margins, 12px gaps).
//
// WHY the band starts at 120 and not 104: the header's own rule is the row
// `HEADER_H` names, and the tabs are the next surface down -- a band that began on
// the rule row drew its pills one row under a hairline that was never meant to
// cross them. 44 rows is the platform's minimum touch target, so the band did not
// have to keep its old 52.
//
// WHY four pills and not three: the dialer's design document asks for a
// settings surface - a ringtone picker, a keypad-tone choice and the choice
// of SIM the platform does not offer - and a section the user cannot reach is
// not a surface. The four pills are 147px wide with 12px gaps, which still
// leaves the longest label ("Contacts") its full width.
const TAB_SCALE: usize = dialer::tab_row::SCALE;
const TAB_BAND_Y0: usize = dialer::tab_row::BAND_Y0;
const TAB_BAND_H: usize = dialer::tab_row::BAND_H;
const TAB_W: usize = dialer::tab_row::COL_W;
const TAB1_X0: usize = dialer::tab_row::col_x0(0);
const TAB1_X1: usize = dialer::tab_row::col_x1(0);
const TAB2_X0: usize = dialer::tab_row::col_x0(1);
const TAB2_X1: usize = dialer::tab_row::col_x1(1);
const TAB3_X0: usize = dialer::tab_row::col_x0(2);
const TAB3_X1: usize = dialer::tab_row::col_x1(2);
const TAB4_X0: usize = dialer::tab_row::col_x0(3);
const TAB4_X1: usize = dialer::tab_row::col_x1(3);
const TAB_LABELS: [&[u8]; 4] = dialer::tab_row::LABELS;

// The painter and the lib's tab gate read the same geometry; these ties fail the
// build if the band, the surface or the row ever drifts from what `tab_row`
// publishes, so the gate can no longer pass a row the screen does not paint.
const _: () = assert!(W == dialer::tab_row::SURFACE_W);
const _: () = assert!(TAB4_X1 == dialer::tab_row::row_right_edge());
const _: () = assert!(dialer::tab_row::rung_band_h() <= TAB_BAND_H);

// Dialed-number display card.
const DISPLAY_X0: usize = 24;
const DISPLAY_X1: usize = 696;
const DISPLAY_Y0: usize = 170;
const DISPLAY_Y1: usize = 380;
const DISPLAY_W: usize = DISPLAY_X1 - DISPLAY_X0;
/// The leading-plus control on the dialed-number card.
///
/// WHY a disc and not a keypad sublabel: the keypad has no long-press, so a
/// "+" printed under the 0 key was a label for a key that typed 0. The disc
/// is a real control, offered only where a leading "+" is valid.
const PLUS_CX: usize = DISPLAY_X0 + 44;
const PLUS_CY: usize = 236;
const PLUS_R: i32 = 20;
const PLACEHOLDER_SCALE: usize = type_scale::BODY;
const PLACEHOLDER_Y0: usize = 250;
/// Real call-outcome caption under the dialed number. A scale-1 run owns
/// run_rows(1) = 27 rows, so 350..377 clears the 63-row scale-4 digit box at
/// 215..278 and stays inside the card's 380-row bottom edge.
const OUTCOME_Y0: usize = 350;

// Backspace control in the action row (to the right of Call button, active when active_len > 0).
const BACKSPACE_CX: usize = 582;
const BACKSPACE_CY: usize = 1065;
const BACKSPACE_R: i32 = 42;

// Keypad layout metrics: scale 2 digit, scale 1 sublabel, 48px radius circular discs.
const KEY_DIGIT_SCALE: usize = type_scale::SECTION;
const KEY_SUB_SCALE: usize = type_scale::CAPTION;
const KEY_RADIUS: usize = 48;

// SOS emergency disc (radius 42px with scale 1 bold vector text comfortably centered inside).
const SOS_CX: usize = 138;
const SOS_CY: usize = 1065;
const SOS_R: i32 = 42;
const SOS_SCALE: usize = type_scale::CAPTION;

// Central call control.
const CALL_CX: usize = 360;
const CALL_CY: usize = 1065;
const CALL_R: i32 = 52;
#[allow(dead_code)]
const CALL_PILL_W: usize = 160;
#[allow(dead_code)]
const CALL_PILL_H: usize = 76;
const CALL_PILL_R: i32 = 38;
const CALL_PILL_HALF_SPINE: i32 = 42;

#[inline(always)]
fn in_call_pill(x: usize, y: usize) -> bool {
    let dx = (x as i32 - CALL_CX as i32).abs();
    let dy = y as i32 - CALL_CY as i32;
    let dx_clamped = (dx - CALL_PILL_HALF_SPINE).max(0);
    dx_clamped * dx_clamped + dy * dy <= CALL_PILL_R * CALL_PILL_R
}

// Contacts action row editing shortcut (for contacts section edit/delete).
const CONTACTS_CX: usize = 582;
const CONTACTS_CY: usize = 1065;
const CONTACTS_R: i32 = 42;

// Action bar band (the three discs above live in it).
const ACTION_Y0: usize = 990;
const ACTION_Y1: usize = 1140;

// Recents ledger geometry. WHY it starts at 424 rather than 410: the first row
// needs room for its two scale-1 runs, 27 rows each, inside an
// 96 px row, and 410 would push the last of five rows past the action bar.
//
// WHY the numbers come from the library: the height of a list and the height
// of the window it is shown in are one fact, and the scroll model is built
// from them. Two copies of the same number in two files is a copy that drifts.
const RECENTS_Y0: usize = LIB_RECENTS_Y0;
const RECENTS_ROW_H: usize = LIB_RECENTS_ROW_H;
/// The bottom of the recents list's window.
const RECENTS_VIEWPORT_Y1: usize = LIB_RECENTS_VIEWPORT_Y1;
const CONTENT_Y0: usize = 410;
const CONTENT_Y1: usize = 1140;

// Bottom navigation dock.
const DOCK_Y0: usize = 1260;
const DOCK_Y1: usize = 1380;
const DOCK_ITEM_W: usize = 140;
/// WHY these x positions: they are the four 140 px slots the previous renderer
/// hard-coded its labels against, kept so the dock does not move.
const DOCK_ITEM_X0: [usize; 4] = [48, 208, 368, 528];
// WHY these four: the dock's icons are a star, a clock, a keypad grid and a
// person, in that order. A label was once changed to "Voicemail" while the
// person icon still opened the contact list, so the dock promised a service
// the platform does not have and mislabeled two working destinations.
const DOCK_LABELS: [&[u8]; 4] = [b"Favorites", b"Recents", b"Keypad", b"Contacts"];
const DOCK_LABEL_Y0: usize = 1340;
const DOCK_LABEL_SCALE: usize = type_scale::CAPTION;
/// Icon band inside the dock plate: a scale-1 label below, a 36 px glyph above.
const DOCK_ICON_Y0: usize = 1290;
const DOCK_ICON_H: usize = 36;

// Keypad layout constants
const BTN_W: usize = 180;
const BTN_H: usize = 124;
const COL_X: [usize; 3] = [48, 270, 492];
const ROW_Y: [usize; 4] = [410, 560, 710, 860];

// Contacts section geometry.
//
// WHY the whole layout is pinned here instead of flowing: `#![no_std]` with no
// allocator means the painter and the hit-tester must agree on every boundary
// from the same constants, or a tap lands on a control that is not there.
/// Selected-contact card, reusing the dialer's display-card band.
const CARD_Y0: usize = 170;
const CARD_Y1: usize = 380;
/// Contact list band. The band is 528px tall, which is eight rows of 66px, and
/// the store can hold twelve saved contacts plus twelve unsaved ones, so the
/// list is taller than its window and has to scroll.
const LIST_Y0: usize = LIB_CONTACTS_LIST_Y0;
const CONTACT_ROW_H: usize = LIB_CONTACT_ROW_H;
/// The bottom of the contact list's window.
const LIST_VIEWPORT_Y1: usize = LIB_CONTACTS_VIEWPORT_Y1;
/// Footer band: the honest capacity/durability notice.
const NOTICE_Y0: usize = 938;
/// The rung both notice lines are drawn at, named once so the band's height and
/// the two painters cannot disagree about it.
const NOTICE_SCALE: usize = type_scale::CAPTION;
/// The band's own top pad: the rows between its edge and the first line's box.
const NOTICE_TOP_PAD: usize = 4;
/// Rows one notice line reserves -- the rung's own line box, asked of the ladder
/// rather than written down, so a change to the face moves the layout with it.
const NOTICE_LINE_ROWS: usize = run_rows(NOTICE_SCALE);
const NOTICE_LINE1_Y: usize = NOTICE_Y0 + NOTICE_TOP_PAD;
/// The second line sits one whole box below the first, so the two never share a
/// row however the ladder is scaled.
const NOTICE_LINE2_Y: usize = NOTICE_LINE1_Y + NOTICE_LINE_ROWS;
/// The band's end: every row both boxes ink, and nothing else.
const NOTICE_Y1: usize = NOTICE_Y0 + NOTICE_TOP_PAD + 2 * NOTICE_LINE_ROWS;
/// WHY these are compile-time gates: the band was once 52 hand-written rows under
/// a ladder whose caption box had grown to 27, so the second line ran past its
/// end and the `NOTICE_Y0..NOTICE_Y1` gate stopped answering its baseline rows.
/// Deriving the band from the rung makes that impossible to author again, and
/// these two make it fail to build rather than fail to render if the band is ever
/// pinned by hand. The second is the one the action row cares about: the notice
/// may widen downwards, but not until it covers a row the discs paint -- their
/// topmost inked row is `C_ACTION_Y - CALL_R`.
const _: () = assert!(NOTICE_Y1 >= NOTICE_LINE2_Y + NOTICE_LINE_ROWS);
const _: () = assert!(NOTICE_Y1 <= C_ACTION_Y - CALL_R as usize);
/// Star hit circle on the right edge of every contact row.
const STAR_CX: usize = 660;
const STAR_R: i32 = 20;
/// Action discs in the contacts action row.
const C_ACTION_Y: usize = 1065;

// Settings section geometry.
//
// WHY the settings surface is a section and not a dialog: a dialog needs a
// focus model and a dismissal rule the platform does not have, and a section
// the user can leave with the same back control as every other section is
// one the platform can express.
const SETTINGS_TITLE_Y0: usize = 416;
const SETTINGS_ROW_Y0: usize = 452;
const SETTINGS_ROW_H: usize = 96;
const SETTINGS_TEXT_X: usize = 48;

/// The settings row's two columns: the title on the leading edge, the value on
/// the trailing edge.
///
/// WHY the value is bounded by the title's right edge: the value is
/// right-aligned to the row's trailing padding, so a value wider than the gap
/// between that edge and the title's column drops left into the title's ink.
/// One bound, named from the title, is used by both the trim and the draw, so a
/// long value loses its tail (with an ellipsis) rather than its place.
const SETTINGS_ROW_PAD: usize = std::design::spacing::MD;
const SETTINGS_TITLE_MAX_W: usize = 300;
const SETTINGS_COL_GAP: usize = 12;
const SETTINGS_VALUE_X1: usize = DISPLAY_X1 - SETTINGS_ROW_PAD;
const SETTINGS_TITLE_X1: usize = DISPLAY_X0 + SETTINGS_ROW_PAD + SETTINGS_TITLE_MAX_W;
const SETTINGS_VALUE_MAX_W: usize =
    SETTINGS_VALUE_X1 - SETTINGS_TITLE_X1 - SETTINGS_COL_GAP;
const _: () = assert!(
    SETTINGS_VALUE_X1 - SETTINGS_VALUE_MAX_W >= SETTINGS_TITLE_X1 + SETTINGS_COL_GAP
);

/// The ringtone picker's rows and the window they are shown in.
const RINGTONE_ROW_H: usize = 72;
const RINGTONE_ROW_Y0: usize = 452;
const RINGTONE_VIEWPORT_Y1: usize = 1028;
/// The row that leaves the picker.
const RINGTONE_BACK_Y0: usize = 1040;
const RINGTONE_BACK_H: usize = 64;
/// The column each picker run is drawn in: the row's inner width between its
/// two leading paddings. Both the name and its path are measured and trimmed to
/// this one bound so neither drops to the bitmap face at an unmeasured width.
const RINGTONE_TEXT_MAX_W: usize = DISPLAY_W - 2 * SETTINGS_ROW_PAD;

/// The contact editor's field: the value run is bounded by the space left of
/// the first control disc, so a long number never paints under the Clear disc
/// and its tap target.
///
/// WHY derived from the disc: the clear disc's centre is inset from the row's
/// trailing edge and its outer extent is its touch radius, so the value's right
/// edge has to stop at that left extent. Writing the number down is how the two
/// came to overlap.
const FIELD_TEXT_X0: usize = DISPLAY_X0 + 24;
const FIELD_CLEAR_DX: usize = 88;
const FIELD_DISC_R: usize = 22;
const FIELD_VALUE_GAP: usize = 12;
const FIELD_VALUE_MAX_W: usize =
    (DISPLAY_X1 - FIELD_CLEAR_DX) - FIELD_DISC_R - FIELD_TEXT_X0 - FIELD_VALUE_GAP;
const _: () = assert!(
    FIELD_TEXT_X0 + FIELD_VALUE_MAX_W <= (DISPLAY_X1 - FIELD_CLEAR_DX) - FIELD_DISC_R
);

/// The scroll extents of the ringtone picker's list.
fn ringtone_scroll_extents(rows: usize) -> ScrollState {
    ScrollState::new(
        rows * RINGTONE_ROW_H,
        RINGTONE_VIEWPORT_Y1 - RINGTONE_ROW_Y0,
        0,
    )
}

/// The width of the scroll indicator's track.
const SCROLL_TRACK_X: usize = 690;
/// The width of the scroll indicator's thumb.
const SCROLL_THUMB_W: usize = 4;

/// Draws the platform's scroll indicator beside a list.
///
/// WHY the indicator is drawn whenever the list scrolls and never otherwise:
/// an indicator on a list that fits says there is more when there is not, and
/// an indicator that is missing on a list that does not fit says the rest of
/// the list does not exist.
#[inline(always)]
fn scroll_indicator(x: usize, y: usize, scroll: &ScrollState, track_y: usize, track_h: usize) -> Option<u32> {
    let (thumb_y, thumb_h) = scroll.thumb_rect(track_y, track_h, 24)?;
    if x >= SCROLL_TRACK_X && x < SCROLL_TRACK_X + SCROLL_THUMB_W && y >= thumb_y && y < thumb_y + thumb_h {
        return Some(THEME.primary);
    }
    None
}

fn get_keypad_char(row: usize, col: usize) -> Option<char> {
    match (row, col) {
        (0, 0) => Some('1'),
        (0, 1) => Some('2'),
        (0, 2) => Some('3'),
        (1, 0) => Some('4'),
        (1, 1) => Some('5'),
        (1, 2) => Some('6'),
        (2, 0) => Some('7'),
        (2, 1) => Some('8'),
        (2, 2) => Some('9'),
        (3, 0) => Some('*'),
        (3, 1) => Some('0'),
        (3, 2) => Some('#'),
        _ => None,
    }
}

fn get_keypad_sublabel(row: usize, col: usize) -> &'static [u8] {
    match (row, col) {
        (0, 1) => b"ABC",
        (0, 2) => b"DEF",
        (1, 0) => b"GHI",
        (1, 1) => b"JKL",
        (1, 2) => b"MNO",
        (2, 0) => b"PQRS",
        (2, 1) => b"TUV",
        (2, 2) => b"WXYZ",
        _ => b"",
    }
}

// -----------------------------------------------------------------------------
// Telemetry parsing. Both helpers read a real JSON field out of a real syscall
// payload; neither guesses a default.
// -----------------------------------------------------------------------------

/// Reads `"<key>":<digits>` out of a JSON payload.
///
/// WHY: the modem HAL emits a real `"bars"` field and the fuel gauge emits a
/// real `"soc"` field. Parsing them is the difference between a signal meter
/// that reflects the radio and one that reflects a boolean.
fn json_u8(json: &[u8], key: &str) -> Option<u8> {
    let pat = key.as_bytes();
    let mut i = 0usize;
    while i + pat.len() < json.len() {
        if &json[i..i + pat.len()] == pat
            && json[i + pat.len()] == b':'
            && i + pat.len() > 0
            && json[i + pat.len() - 1] == b'"'
        {
            let mut j = i + pat.len() + 1;
            let mut v: u32 = 0;
            let mut any = false;
            while j < json.len() && json[j].is_ascii_digit() {
                v = v.saturating_mul(10).saturating_add((json[j] - b'0') as u32);
                any = true;
                j += 1;
            }
            if any && v <= 255 {
                return Some(v as u8);
            }
        }
        i += 1;
    }
    None
}

/// True when the payload contains the exact literal `"present":true`.
fn json_present_true(json: &[u8]) -> bool {
    let pat = b"\"present\":true";
    if json.len() < pat.len() {
        return false;
    }
    let mut i = 0usize;
    while i + pat.len() <= json.len() {
        if &json[i..i + pat.len()] == pat {
            return true;
        }
        i += 1;
    }
    false
}

// -----------------------------------------------------------------------------
// Pixel Color Resolution Engine
// -----------------------------------------------------------------------------

/// Header band: back control, title, real signal bars, real battery.
const STATUS_BAR_HEIGHT: usize = interos_std::design::viewport::STATUS_BAR_HEIGHT;

pub fn status_band() -> (usize, usize) {
    (0, STATUS_BAR_HEIGHT)
}

fn refresh_clock(dialer: &mut DialerApp) {
    if let Some(now) = std::rtc::Reading::now() {
        dialer.clock = now.clock_text();
        dialer.clock_ok = true;
    } else {
        dialer.clock_ok = false;
    }
}

fn header_pixel(x: usize, y: usize, dialer: &DialerApp) -> u32 {
    if y < STATUS_BAR_HEIGHT {
        let band = interos_std::design::components::StatusBand::new(W);
        let clock = if dialer.clock_ok { Some(&dialer.clock[..]) } else { None };
        let cellular = if dialer.signal_bars > 0 {
            interos_std::design::components::Reading::Measured((dialer.signal_bars as u32) * 25)
        } else {
            interos_std::design::components::Reading::Unbound
        };
        let battery = match dialer.battery_soc {
            Some(soc) => interos_std::design::components::Reading::Measured(soc as u32),
            None => interos_std::design::components::Reading::Unbound,
        };
        let instruments = interos_std::design::components::Instruments {
            cellular,
            wifi: interos_std::design::components::Reading::Unbound,
            battery,
        };
        return band
            .pixel(x, y, clock, &instruments, &THEME)
            .unwrap_or(THEME.surface);
    }
    if y >= HEADER_H {
        return THEME.border;
    }

    // Standardized back button squircle (24..72, 24..72)
    if let Some(c) = std::design::back_button::pixel_for_theme(x, y, &THEME) {
        return c;
    }

    // Header Title: "Phone". The run is on the title rung, so it needs
    // run_rows(TITLE_SCALE) = 63 rows, and its caps are centred on the row the
    // back control stands in rather than the box -- a title centred from 0
    // would sit with half its box inside the band that paints over it.
    let title_rows = run_rows(TITLE_SCALE);
    if y >= TITLE_Y0 && y < TITLE_Y0 + title_rows {
        if let Some(c) = test_text(x, y, HEADER_TITLE_X0, TITLE_Y0, TITLE_SCALE, b"Phone", HEADER_TITLE_MAX_W, THEME.on_surface, THEME.surface_container) {
            return c;
        }
    }

    // The ringtone the device's ringtone slot names.
    //
    // WHY it is drawn and not assumed: the slot is filled by another app, and
    // a user who has chosen a ringtone must be able to see which one the
    // device will play. A user who has not chosen one sees nothing, because
    // the slot is empty and there is nothing to name.
    //
    // WHY ellipsize and one bound: the name is measured, trimmed to the column
    // the title leaves it, and right-aligned from that trimmed run, so a long
    // name loses its tail rather than its place -- it never jumps to the bitmap
    // face at a different size and never reaches back into the title's ink.
    if dialer.ringtone_len > 0 {
        let label = &dialer.ringtone[..dialer.ringtone_len];
        let mut rbuf = [0u8; 64];
        let n = std::design::font::ellipsize(label, 1, RINGTONE_MAX_W, &mut rbuf);
        let disp = if n > 0 { &rbuf[..n] } else { label };
        let width = std::design::font::measure(disp, 1);
        if let Some(c) = test_text(
            x,
            y,
            RINGTONE_X1 - width,
            HEADER_TOP + 8,
            1,
            disp,
            RINGTONE_MAX_W,
            THEME.on_surface_variant,
            THEME.surface_container,
        ) {
            return c;
        }
    }

    // Cellular signal indicator (x: 610..670, on the ringtone caption's row)
    //
    // WHY the bar count is real: the previous renderer filled all four bars from
    // the `modem_present` boolean, so a present-but-idle radio drew a full
    // signal. `signal_bars` is the `"bars"` field of the modem telemetry.
    if x >= 610 && x < 670 && y >= HEADER_TOP + 8 && y < HEADER_TOP + 30 {
        let bar_idx = (x - 610) / 15;
        let bar_x = (x - 610) % 15;
        let bar_min_y = (HEADER_TOP + 30) - (bar_idx + 1) * 5;
        if bar_x < 9 && y >= bar_min_y {
            return if (bar_idx as u8) < dialer.signal_bars {
                THEME.primary
            } else {
                THEME.border
            };
        }
        return THEME.surface_container;
    }

    // Battery indicator (x: 678..706, centred on the same row)
    //
    // WHY it is drawn only when the fuel gauge answered: this app holds no
    // power-control capability, so `battery_query` is refused and there is no
    // state of charge to draw. An always-drawn empty shell read as "battery at
    // 0%", which is a hardware claim this app cannot make. The browser takes the
    // same approach for telemetry it does not receive: draw nothing.
    if x >= 678 && x < 706 && y >= HEADER_TOP + 12 && y < HEADER_TOP + 26 {
        if let Some(soc) = dialer.battery_soc {
            let pct = soc.min(100) as usize;
            let inner_x0 = 681usize;
            let inner_x1 = 701usize;
            let inner_w = inner_x1 - inner_x0;
            let fill_w = inner_w * pct / 100;
            // Outline.
            if x == 678
                || x == 702
                || y == HEADER_TOP + 12
                || y == HEADER_TOP + 25
                || (x >= 703 && y >= HEADER_TOP + 16 && y < HEADER_TOP + 22)
            {
                return THEME.on_surface_variant;
            }
            // Real fill from the reported state of charge.
            if x >= inner_x0 && x < inner_x0 + fill_w && y >= HEADER_TOP + 14 && y < HEADER_TOP + 24 {
                return THEME.on_surface;
            }
        }
        return THEME.surface_container;
    }

    THEME.surface_container
}

/// Segmented tabs. All four sections exist, so all four are live.
fn tabs_pixel(x: usize, y: usize, dialer: &DialerApp) -> u32 {
    if !(TAB_BAND_Y0..TAB_BAND_Y0 + TAB_BAND_H).contains(&y) {
        return THEME.surface;
    }
    let label_y0 = TAB_BAND_Y0 + (TAB_BAND_H.saturating_sub(run_rows(TAB_SCALE))) / 2;
    let ranges = [
        (TAB1_X0, TAB1_X1),
        (TAB2_X0, TAB2_X1),
        (TAB3_X0, TAB3_X1),
        (TAB4_X0, TAB4_X1),
    ];
    for (i, (x0, x1)) in ranges.iter().enumerate() {
        if x < *x0 || x >= *x1 {
            continue;
        }
        let w = x1 - x0;
        let active = match dialer.section {
            DialerSection::Keypad => i == 0,
            DialerSection::Recents => i == 1,
            DialerSection::Contacts => i == 2,
            DialerSection::Settings => i == 3,
        };
        let (fill, border, ink) = if active {
            // The platform's segmented-control grammar: the selected segment
            // is filled with the accent and its label is the ink on the
            // accent.
            (THEME.primary, None, THEME.on_primary)
        } else {
            (THEME.surface_container_high, Some(THEME.border), THEME.on_surface_variant)
        };
        // WHY a pill and not the old square fill: the design draws segmented
        // tabs as capsules. The text is only asked inside the pill so a glyph
        // near the label's ends cannot ink the curved cut-outs.
        let pill = std::design::primitives::Rect::new(*x0, TAB_BAND_Y0, w, TAB_BAND_H);
        if std::design::primitives::draw_pill(x, y, pill, fill, border).is_none() {
            continue;
        }
        let label_x0 = *x0 + std::design::font::center_x(TAB_LABELS[i], TAB_SCALE, w);
        let label_px = if active {
            test_text_bold(x, y, label_x0, label_y0, TAB_SCALE, TAB_LABELS[i],TAB_W,  ink, fill)
        } else {
            test_text(x, y, label_x0, label_y0, TAB_SCALE, TAB_LABELS[i],TAB_W,  ink, fill)
        };
        if let Some(c) = label_px {
            return c;
        }
        return fill;
    }
    THEME.surface
}

/// Dialed-number card: the number, the placeholder, the backspace disc and the
/// real outcome caption.
fn display_pixel(x: usize, y: usize, dialer: &DialerApp) -> u32 {
    if !(DISPLAY_Y0..DISPLAY_Y1).contains(&y) || x < DISPLAY_X0 || x >= DISPLAY_X1 {
        return THEME.surface;
    }

    let card_rect = std::design::primitives::Rect::new(
        DISPLAY_X0,
        DISPLAY_Y0,
        DISPLAY_W,
        DISPLAY_Y1 - DISPLAY_Y0,
    );
    let card_fill = match std::design::primitives::draw_rounded_rect(
        x,
        y,
        card_rect,
        CARD.radius,
        CARD.fill,
        CARD.border,
        1,
    ) {
        Some(c) => c,
        None => return THEME.surface,
    };

    // The leading-plus control. A "+" is only valid as the first character of
    // an international number, so the disc is offered exactly while the entry
    // is empty and no call is up.
    if dialer.state == CallState::Idle && dialer.active_len == 0 {
        let p_dx = (x as i32) - PLUS_CX as i32;
        let p_dy = (y as i32) - PLUS_CY as i32;
        if p_dx * p_dx + p_dy * p_dy <= PLUS_R * PLUS_R {
            if p_dx * p_dx + p_dy * p_dy >= (PLUS_R - 2) * (PLUS_R - 2) {
                return THEME.border;
            }
            if (p_dx.abs() <= 1 && p_dy.abs() <= 8) || (p_dy.abs() <= 1 && p_dx.abs() <= 8) {
                return THEME.on_surface;
            }
            return THEME.surface_container_high;
        }
    }

    if dialer.active_len > 0 {
        // Matched contact name
        if let Some(index) = dialer
            .contacts
            .match_by_number_prefix(&dialer.active_number[..dialer.active_len])
        {
            if let Some(rec) = dialer.contacts.record(index) {
                let mut buf = [0u8; CONTACT_NAME_MAX];
                let n = std::design::font::ellipsize(rec.name_bytes(), 1, DISPLAY_W - 48, &mut buf);
                if n > 0 {
                    let w = std::design::font::measure(&buf[..n], 1);
                    let cx = DISPLAY_X0 + (DISPLAY_W.saturating_sub(w)) / 2;
                    if let Some(c) = test_text(x, y, cx, 190, 1, &buf[..n],DISPLAY_W - 48,  THEME.primary, THEME.surface_container) {
                        return c;
                    }
                }
            }
        }

        let scale = if dialer.active_len <= 10 { 4 } else { 3 };
        let y0 = 215;
        let digits = &dialer.active_number[..dialer.active_len];
        let mut buf = [0u8; 32];
        let n = std::design::font::ellipsize(digits, scale, DISPLAY_W - 48, &mut buf);
        let total_w = std::design::font::measure(&buf[..n], scale);
        let x0 = DISPLAY_X0 + (DISPLAY_W.saturating_sub(total_w)) / 2;
        if let Some(c) = test_text(x, y, x0, y0, scale, &buf[..n],DISPLAY_W - 48,  THEME.on_surface, THEME.surface_container) {
            return c;
        }

        // Clear (X) disc on the right of the dialed number
        let clear_cx = DISPLAY_X1 - 44;
        let clear_cy = 236;
        let clear_r = 20i32;
        let c_dx = (x as i32) - clear_cx as i32;
        let c_dy = (y as i32) - clear_cy as i32;
        if c_dx * c_dx + c_dy * c_dy <= clear_r * clear_r {
            if c_dx * c_dx + c_dy * c_dy >= (clear_r - 2) * (clear_r - 2) {
                return THEME.border;
            }
            let ix = c_dx.abs();
            let iy = c_dy.abs();
            if ix <= 6 && iy <= 6 && (ix == iy || ix == iy + 1 || ix == iy - 1) {
                return THEME.on_surface_variant;
            }
            return THEME.surface_container_high;
        }

        // Action Pills: [+ Add Contact] and [Send Message]
        //
        // WHY the pills give way to the call's own state while a call is up:
        // what the platform knows about the call is the most important thing
        // on the card, and the platform's answer is that its telemetry carries
        // no call duration. The app used to keep a counter of its own and
        // show it, which was the app's ticks rather than a fact about the call.
        let duration_note = dialer.outcome.duration_note();
        if !duration_note.is_empty() {
            if y >= 305 && y < 305 + run_rows(1) {
                let w = std::design::font::measure(duration_note, 1);
                let x0 = DISPLAY_X0 + (DISPLAY_W.saturating_sub(w)) / 2;
                if let Some(c) = test_text(
                    x,
                    y,
                    x0,
                    305,
                    1,
                    duration_note,DISPLAY_W - 48, 
                    THEME.on_surface_variant,
                    THEME.surface_container,
                ) {
                    return c;
                }
            }
        } else {
        let p1_x0 = 80usize;
        let p1_w = 250usize;
        let p1_y0 = 295usize;
        let p1_h = 44usize;
        if x >= p1_x0 && x < p1_x0 + p1_w && y >= p1_y0 && y < p1_y0 + p1_h {
            if let Some(fill) = std::design::primitives::draw_pill(
                x,
                y,
                std::design::primitives::Rect::new(p1_x0, p1_y0, p1_w, p1_h),
                THEME.surface_container,
                Some(THEME.border),
            ) {
                let label = b"+ Add Contact";
                let lw = std::design::font::measure(label, 2);
                let lx = p1_x0 + (p1_w.saturating_sub(lw)) / 2;
                let ly = p1_y0 + (p1_h.saturating_sub(run_rows(2))) / 2;
                if let Some(c) = test_text(x, y, lx, ly, 2, label, 250, THEME.primary, fill) {
                    return c;
                }
                return fill;
            }
        }

        let p2_x0 = 390usize;
        let p2_w = 250usize;
        let p2_y0 = 295usize;
        let p2_h = 44usize;
        if x >= p2_x0 && x < p2_x0 + p2_w && y >= p2_y0 && y < p2_y0 + p2_h {
            if let Some(fill) = std::design::primitives::draw_pill(
                x,
                y,
                std::design::primitives::Rect::new(p2_x0, p2_y0, p2_w, p2_h),
                THEME.surface_container,
                Some(THEME.border),
            ) {
                let label = b"Send Message";
                let lw = std::design::font::measure(label, 2);
                let lx = p2_x0 + (p2_w.saturating_sub(lw)) / 2;
                let ly = p2_y0 + (p2_h.saturating_sub(run_rows(2))) / 2;
                if let Some(c) = test_text(x, y, lx, ly, 2, label, 250, THEME.success, fill) {
                    return c;
                }
                return fill;
            }
        }
        }
    } else if y >= PLACEHOLDER_Y0 && y < PLACEHOLDER_Y0 + run_rows(PLACEHOLDER_SCALE) {
        let x0 = DISPLAY_X0
            + std::design::font::center_x(
                b"Enter a phone number",
                PLACEHOLDER_SCALE,
                DISPLAY_W,
            );
        if let Some(c) = test_text(
            x,
            y,
            x0,
            PLACEHOLDER_Y0,
            PLACEHOLDER_SCALE,
            b"Enter a phone number",DISPLAY_W - 48, 
            THEME.on_surface_variant,
            THEME.surface_container,
        ) {
            return c;
        }
    }

    // Real outcome of the last call attempt.
    let caption = dialer.outcome.caption();
    if !caption.is_empty() {
        let mut buf = [0u8; 48];
        let n = std::design::font::ellipsize(caption, 1, DISPLAY_W - 32, &mut buf);
        if n > 0 {
            let w = std::design::font::measure(&buf[..n], 1);
            let cx = DISPLAY_X0 + (DISPLAY_W.saturating_sub(w)) / 2;
            let ink = match dialer.outcome {
                CallOutcome::Refused | CallOutcome::EmergencyRefused => THEME.error,
                _ => THEME.on_surface_variant,
            };
            if let Some(c) = test_text(x, y, cx, OUTCOME_Y0, 1, &buf[..n],DISPLAY_W - 48,  ink, THEME.surface_container) {
                return c;
            }
        }
    }

    card_fill
}

/// The 3x4 dial pad. Only drawn in the Keypad section.
///
/// Each keypad cell renders a 48px radius circular button disc with subtle
/// border rim, raised pure-white surface, and depth elevation.
/// Cells with sublabels stack the digit and sublabel; cells without (1, *, #)
/// center the digit vertically in the disc.
fn keypad_pixel_at(x: usize, y: usize, editing: Option<&DialerApp>) -> u32 {
    for row in 0..4 {
        let by = ROW_Y[row];
        if y >= by && y < by + BTN_H {
            for col in 0..3 {
                let bx = COL_X[col];
                if x >= bx && x < bx + BTN_W {
                    let cx = (bx + BTN_W / 2) as i32;
                    let cy = (by + BTN_H / 2) as i32;

                    let dx = x as i32 - cx;
                    let dy = y as i32 - cy;
                    let dist_sq = dx * dx + dy * dy;
                    let r = KEY_RADIUS as i32;

                    if dist_sq > r * r {
                        return THEME.surface;
                    }

                    // Disc border
                    if dist_sq >= (r - 2) * (r - 2) {
                        return THEME.border;
                    }

                    let disc_fill = THEME.surface_container;

                    let editing_name = matches!(
                        editing,
                        Some(d) if d.contact_edit.is_some_and(|e| e.field == ContactField::Name)
                    );

                    let digit_rows = run_rows(KEY_DIGIT_SCALE);
                    let sub_rows = run_rows(KEY_SUB_SCALE);

                    let digit_buf: [u8; 1] = if editing_name {
                        let cycle = DialerApp::t9_cycle(row, col);
                        [match editing.and_then(|d| d.editor_last_cell) {
                            Some((lr, lc)) if (lr, lc) == (row, col) => {
                                let ed = editing.and_then(|d| d.contact_edit);
                                match ed {
                                    Some(ed) if ed.name_len > 0 => {
                                        let current = ed.name[ed.name_len - 1];
                                        let at =
                                            cycle.iter().position(|&b| b == current).unwrap_or(0);
                                        cycle[(at + 1) % cycle.len()]
                                    }
                                    _ => cycle[0],
                                }
                            }
                            _ => cycle[0],
                        }]
                    } else {
                        [get_keypad_char(row, col).map(|d| d as u8).unwrap_or(0)]
                    };
                    let has_digit = editing_name || get_keypad_char(row, col).is_some();
                    let digit_bytes: &[u8] = if has_digit { &digit_buf } else { &[] };
                    let sub_buf: &[u8] = if editing_name {
                        DialerApp::t9_cycle(row, col)
                    } else {
                        get_keypad_sublabel(row, col)
                    };

                    let has_sub = !sub_buf.is_empty();

                    let (digit_y0, sub_y0) = if has_sub {
                        let stack_y0 = (cy as usize).saturating_sub((digit_rows + sub_rows) / 2);
                        (stack_y0, stack_y0 + digit_rows)
                    } else {
                        let offset = if digit_bytes == b"*" { 4 } else { 0 };
                        let center_y0 = (cy as usize).saturating_sub(digit_rows / 2) + offset;
                        (center_y0, 0)
                    };

                    // Digit text
                    if !digit_bytes.is_empty() {
                        let digit_x0 = (cx as usize).saturating_sub(
                            std::design::font::measure(digit_bytes, KEY_DIGIT_SCALE) / 2,
                        );
                        if let Some(c) = test_text(
                            x,
                            y,
                            digit_x0,
                            digit_y0,
                            KEY_DIGIT_SCALE,
                            digit_bytes,120, 
                            THEME.on_surface,
                            disc_fill,
                        ) {
                            return c;
                        }
                    }

                    // Sublabel text
                    if has_sub {
                        let sub_w = std::design::font::measure(sub_buf, KEY_SUB_SCALE);
                        let sub_x = (cx as usize).saturating_sub(sub_w / 2);
                        if let Some(c) = test_text(
                            x,
                            y,
                            sub_x,
                            sub_y0,
                            KEY_SUB_SCALE,
                            sub_buf,120, 
                            THEME.on_surface_variant,
                            disc_fill,
                        ) {
                            return c;
                        }
                    }

                    return disc_fill;
                }
            }
        }
    }
    THEME.surface
}

/// The keypad section's pad: a number pad, unchanged.
fn keypad_pixel(x: usize, y: usize) -> u32 {
    keypad_pixel_at(x, y, None)
}

/// The contact editor's pad: the same cells, with the multi-tap cycle visible.
fn editor_keypad_pixel(x: usize, y: usize, dialer: &DialerApp) -> u32 {
    keypad_pixel_at(x, y, Some(dialer))
}

/// Recents ledger, or the honest empty state when nothing has been dialled.
///
/// WHY the ledger scrolls: it holds sixteen attempts and the window holds
/// five, so the entries below the fold were unreachable. The scroll model is
/// the platform's, and the thumb beside the list says how much is below.
fn recents_pixel(x: usize, y: usize, dialer: &DialerApp) -> u32 {
    if dialer.recents_len == 0 {
        // Headline: a scale-1 run owns 27 rows (620..647).
        let head: &[u8] = b"No calls yet";
        let head_x = DISPLAY_X0 + std::design::font::center_x(head, 1, DISPLAY_W);
        if let Some(c) = test_text_bold(x, y, head_x, 620, 1, head,DISPLAY_W,  THEME.on_surface, THEME.surface) {
            return c;
        }
        // Body: a scale-1 run owns 27 rows (680..707).
        let body: &[u8] = b"Every dial attempt and its real outcome is listed here";
        let mut buf = [0u8; 64];
        let n = std::design::font::ellipsize(body, 1, DISPLAY_W, &mut buf);
        if n > 0 {
            let w = std::design::font::measure(&buf[..n], 1);
            let bx = DISPLAY_X0 + (DISPLAY_W.saturating_sub(w)) / 2;
            if let Some(c) = test_text(x, y, bx, 680, 1, &buf[..n],DISPLAY_W,  THEME.on_surface_variant, THEME.surface) {
                return c;
            }
        }
        return THEME.surface;
    }

    let offset = dialer.recents_scroll.offset;
    for i in 0..dialer.recents_len {
        let row_y = RECENTS_Y0 + i * RECENTS_ROW_H;
        // The row's place in the scrolled list, and whether it is on screen.
        let scrolled_y = row_y.saturating_sub(offset);
        if y < scrolled_y || y >= scrolled_y + RECENTS_ROW_H {
            continue;
        }
        let rec = &dialer.recents[i];
        // Text first, plate second: returning the plate colour before the runs
        // were tested would paint every row's number out of existence.
        //
        // WHY an emergency row inverts: its plate is the dark on-surface
        // colour that marks the call, so its ink has to be the light ink
        // that belongs on a dark fill. Drawing the ordinary dark ink over
        // the dark plate made the number and outcome invisible.
        let (ink, sub_ink, row_fill) = if rec.emergency {
            (THEME.ink_on_dark, THEME.ink_on_dark, THEME.on_surface)
        } else {
            (
                THEME.on_surface,
                THEME.on_surface_variant,
                THEME.surface_container,
            )
        };
        // Number, scale 1: run_rows(1) = 27 rows at row_y + 8.
        let mut nbuf = [0u8; 32];
        let nn = std::design::font::ellipsize(&rec.number[..rec.len], 1, DISPLAY_W - 96, &mut nbuf);
        let nx = DISPLAY_X0 + 24;
        if let Some(c) = test_text_bold(
            x,
            y,
            nx,
            scrolled_y + 14,
            1,
            &nbuf[..nn],DISPLAY_W - 96, 
            ink,
            row_fill,
        ) {
            return c;
        }
        // Outcome, scale 1: run_rows(1) = 27 rows at row_y + 56.
        let out: &[u8] = if rec.emergency {
            if rec.accepted {
                b"Emergency - accepted by telephony"
            } else {
                b"Emergency - refused by telephony"
            }
        } else {
            match rec.outcome {
                CallOutcome::Connected => b"Call connected - the platform reports no duration",
                CallOutcome::Ended => b"Call ended",
                CallOutcome::Unconfirmed => b"Accepted; the modem reported no call state",
                CallOutcome::Dialing => b"Accepted by telephony",
                _ => b"Refused by telephony",
            }
        };
        let ox = DISPLAY_X0 + 24;
        if let Some(c) = test_text(x, y, ox, scrolled_y + 46, 1, out,DISPLAY_W - 96,  sub_ink, row_fill) {
            return c;
        }
        // Row plate and divider, in the platform's list-row treatment.
        //
        // WHY the platform's treatment: the row's fill, its corner radius and
        // its divider are the platform's, so a row in this app looks like a row
        // in the file manager or the gallery.
        let style = components::list_row_style(&THEME, false);
        let plate_fill = if rec.emergency { row_fill } else { style.fill };
        if x >= DISPLAY_X0 && x < DISPLAY_X1 {
            if let Some(plate) = std::design::primitives::draw_rounded_rect(
                x,
                y,
                std::design::primitives::Rect::new(DISPLAY_X0, scrolled_y, DISPLAY_W, RECENTS_ROW_H),
                style.radius,
                plate_fill,
                None,
                0,
            ) {
                return plate;
            }
        }
    }

    // The scroll indicator, drawn beside the list it belongs to.
    if let Some(c) = scroll_indicator(
        x,
        y,
        &dialer.recents_scroll,
        RECENTS_Y0,
        RECENTS_VIEWPORT_Y1 - RECENTS_Y0,
    ) {
        return c;
    }
    THEME.surface
}

/// The settings surface: the dialer's own settings, and the ringtone picker.
///
/// WHY each row states what the platform supports: the surface is where the
/// user looks for what the phone can do, so a row that offers a choice the
/// platform cannot make is a lie told in the place the user trusts.
fn settings_pixel(x: usize, y: usize, dialer: &DialerApp) -> u32 {
    if !(CONTENT_Y0..CONTENT_Y1).contains(&y) {
        return THEME.surface;
    }
    if dialer.settings_view == SettingsView::RingtonePicker {
        return ringtone_picker_pixel(x, y, dialer);
    }

    // The title.
    if y >= SETTINGS_TITLE_Y0 && y < SETTINGS_TITLE_Y0 + run_rows(1) {
        if let Some(c) = test_text(
            x,
            y,
            SETTINGS_TEXT_X,
            SETTINGS_TITLE_Y0,
            1,
            b"DIALER SETTINGS",DISPLAY_W - 48, 
            THEME.on_surface,
            THEME.surface,
        ) {
            return c;
        }
    }

    for (index, row) in SettingsRow::ORDER.iter().enumerate() {
        let row_y = SETTINGS_ROW_Y0 + index * SETTINGS_ROW_H;
        if y < row_y || y >= row_y + SETTINGS_ROW_H {
            continue;
        }
        // The platform's list-row treatment.
        let style = components::list_row_style(&THEME, false);
        if std::design::primitives::draw_rounded_rect(
            x,
            y,
            std::design::primitives::Rect::new(DISPLAY_X0, row_y, DISPLAY_W, SETTINGS_ROW_H),
            style.radius,
            style.fill,
            None,
            0,
        )
        .is_none()
        {
            return THEME.surface;
        }

        // The value, at the trailing edge, in the platform's list-row position.
        let len = dialer.settings_value_lens[index];
        let value = &dialer.settings_values[index][..len];
        let mut vbuf = [0u8; 64];
        let vn = std::design::font::ellipsize(value, 1, SETTINGS_VALUE_MAX_W, &mut vbuf);
        let vdisp = if vn > 0 { &vbuf[..vn] } else { value };
        let value_w = std::design::font::measure(vdisp, 1);
        let value_x = DISPLAY_X1.saturating_sub(style.pad + value_w);
        if let Some(c) = test_text(
            x,
            y,
            value_x,
            row_y + (SETTINGS_ROW_H - run_rows(1)) / 2,
            1,
            vdisp,
            SETTINGS_VALUE_MAX_W,
            THEME.on_surface_variant,
            style.fill,
        ) {
            return c;
        }

        // The title.
        if y >= row_y + 8 && y < row_y + 8 + run_rows(1) {
            if let Some(c) = test_text(
                x,
                y,
                DISPLAY_X0 + style.pad,
                row_y + 8,
                1,
                row.title(),
                SETTINGS_TITLE_MAX_W,
                THEME.on_surface,
                style.fill,
            ) {
                return c;
            }
        }

        // The note: what the platform supports about this setting.
        if y >= row_y + 36 && y < row_y + 36 + run_rows(1) {
            let note_w = DISPLAY_W
                .saturating_sub(2 * style.pad)
                .saturating_sub(value_w)
                .saturating_sub(12);
            let mut nbuf = [0u8; 64];
            let n = std::design::font::ellipsize(row.note(), 1, note_w, &mut nbuf);
            if n > 0 {
                if let Some(c) = test_text(
                    x,
                    y,
                    DISPLAY_X0 + style.pad,
                    row_y + 36,
                    1,
                    &nbuf[..n],note_w, 
                    THEME.on_surface_variant,
                    style.fill,
                ) {
                    return c;
                }
            }
        }
        return style.fill;
    }
    THEME.surface
}

/// The ringtone picker: the audio documents the platform's index holds.
///
/// WHY the picker is a list of the platform's documents and not a list of
/// sounds: the ringtone slot names a document on the device, so the choices
/// are the documents the device holds.
fn ringtone_picker_pixel(x: usize, y: usize, dialer: &DialerApp) -> u32 {
    // The title.
    if y >= SETTINGS_TITLE_Y0 && y < SETTINGS_TITLE_Y0 + run_rows(1) {
        if let Some(c) = test_text(
            x,
            y,
            SETTINGS_TEXT_X,
            SETTINGS_TITLE_Y0,
            1,
            b"CHOOSE A RINGTONE",DISPLAY_W - 48, 
            THEME.on_surface,
            THEME.surface,
        ) {
            return c;
        }
    }

    let offset = dialer.ringtone_scroll.offset;
    if dialer.ringtone_choice_count == 0 {
        // An empty picker needs to say why it is empty; the title alone left
        // the screen looking like the list had failed to load.
        if let Some(c) = test_text(
            x,
            y,
            DISPLAY_X0 + 24,
            RINGTONE_ROW_Y0 + 8,
            1,
            b"No audio documents on this device",
            DISPLAY_W - 48,
            THEME.on_surface,
            THEME.surface,
        ) {
            return c;
        }
        if let Some(c) = test_text(
            x,
            y,
            DISPLAY_X0 + 24,
            RINGTONE_ROW_Y0 + 40,
            1,
            b"The platform's index lists no audio file for this slot.",
            DISPLAY_W - 48,
            THEME.on_surface_variant,
            THEME.surface,
        ) {
            return c;
        }
        return THEME.surface;
    }
    for i in 0..dialer.ringtone_choice_count {
        let row_y = RINGTONE_ROW_Y0 + i * RINGTONE_ROW_H;
        let scrolled_y = row_y.saturating_sub(offset);
        if y < scrolled_y || y >= scrolled_y + RINGTONE_ROW_H {
            continue;
        }
        let style = components::list_row_style(&THEME, false);
        if std::design::primitives::draw_rounded_rect(
            x,
            y,
            std::design::primitives::Rect::new(DISPLAY_X0, scrolled_y, DISPLAY_W, RINGTONE_ROW_H),
            style.radius,
            style.fill,
            None,
            0,
        )
        .is_none()
        {
            return THEME.surface;
        }
        if y == scrolled_y + RINGTONE_ROW_H - 1 && x >= DISPLAY_X0 && x < DISPLAY_X1 {
            return style.divider;
        }
        let choice = &dialer.ringtone_choices[i];
        let name = &choice.name[..choice.name_len];
        let mut nbuf = [0u8; 64];
        let nn = std::design::font::ellipsize(name, 1, RINGTONE_TEXT_MAX_W, &mut nbuf);
        let ndisp = if nn > 0 { &nbuf[..nn] } else { name };
        if let Some(c) = test_text(
            x,
            y,
            DISPLAY_X0 + style.pad,
            scrolled_y + 8,
            1,
            ndisp,
            RINGTONE_TEXT_MAX_W,
            THEME.on_surface,
            style.fill,
        ) {
            return c;
        }
        let path = &choice.path[..choice.path_len];
        let mut pbuf = [0u8; 64];
        let n = std::design::font::ellipsize(path, 1, RINGTONE_TEXT_MAX_W, &mut pbuf);
        if n > 0 {
            if let Some(c) = test_text(
                x,
                y,
                DISPLAY_X0 + style.pad,
                scrolled_y + 34,
                1,
                &pbuf[..n],
                RINGTONE_TEXT_MAX_W,
                THEME.on_surface_variant,
                style.fill,
            ) {
                return c;
            }
        }
        return style.fill;
    }

    // The row that leaves the picker.
    if y >= RINGTONE_BACK_Y0 && y < RINGTONE_BACK_Y0 + RINGTONE_BACK_H {
        let style = components::list_row_style(&THEME, false);
        if std::design::primitives::draw_rounded_rect(
            x,
            y,
            std::design::primitives::Rect::new(DISPLAY_X0, RINGTONE_BACK_Y0, DISPLAY_W, RINGTONE_BACK_H),
            style.radius,
            style.fill,
            None,
            0,
        )
        .is_none()
        {
            return THEME.surface;
        }
        if let Some(c) = test_text(
            x,
            y,
            DISPLAY_X0 + style.pad,
            RINGTONE_BACK_Y0 + (RINGTONE_BACK_H - run_rows(1)) / 2,
            1,
            b"Back to the settings",DISPLAY_W - 48, 
            THEME.on_surface,
            style.fill,
        ) {
            return c;
        }
        return style.fill;
    }

    // The scroll indicator, drawn beside the list it belongs to.
    if let Some(c) = scroll_indicator(
        x,
        y,
        &dialer.ringtone_scroll,
        RINGTONE_ROW_Y0,
        RINGTONE_VIEWPORT_Y1 - RINGTONE_ROW_Y0,
    ) {
        return c;
    }
    THEME.surface
}

// -----------------------------------------------------------------------------
// Contacts section: card, list, notice and action row
// -----------------------------------------------------------------------------

/// The selected-contact card, and the editor's two fields while it is open.
///
/// WHY the card is the dialer's existing display-card band: the sections then
/// share one region instead of each owning its own, and the renderer stays a
/// top-to-bottom walk of the same rows.
fn contacts_card_pixel(x: usize, y: usize, dialer: &DialerApp) -> u32 {
    if !(CARD_Y0..CARD_Y1).contains(&y) || x < DISPLAY_X0 || x >= DISPLAY_X1 {
        return THEME.surface;
    }

    // Editor: two labelled fields. WHY the caret is a 2px accent bar and not a
    // blinking block: the app repaints on input, not on a timer, so there is no
    // phase to blink with, and a bar that only exists when the field is focused
    // is the honest version of a caret.
    if let Some(ed) = dialer.contact_edit {
        let fields: [(usize, &[u8], ContactField, &[u8], usize); 2] = [
            (
                CARD_Y0 + 6,
                b"Name",
                ContactField::Name,
                &ed.name,
                ed.name_len,
            ),
            (
                CARD_Y0 + 108,
                b"Number",
                ContactField::Number,
                &ed.number,
                ed.number_len,
            ),
        ];
        for (fy, label, field, buf, len) in fields {
            if !(fy..fy + 96).contains(&y) {
                continue;
            }
            let focused = ed.field == field;
            let Some(fill) = std::design::primitives::draw_rounded_rect(
                x,
                y,
                std::design::primitives::Rect::new(DISPLAY_X0, fy, DISPLAY_W, 96),
                18,
                THEME.surface_container,
                Some(if focused {
                    THEME.primary
                } else {
                    THEME.border
                }),
                1,
            ) else {
                return THEME.surface;
            };
            let mut lbuf = [0u8; 16];
            let ln = std::design::font::ellipsize(label, 1, DISPLAY_W - 48, &mut lbuf);
            if ln > 0 {
                if let Some(c) = test_text(
                    x,
                    y,
                    DISPLAY_X0 + 24,
                    fy + 8,
                    1,
                    &lbuf[..ln],DISPLAY_W - 48, 
                    THEME.on_surface_variant,
                    fill,
                ) {
                    return c;
                }
            }
            let mut vbuf = [0u8; 64];
            let vn = std::design::font::ellipsize(&buf[..len], 2, FIELD_VALUE_MAX_W, &mut vbuf);
            let vy = fy + 36;
            if vn > 0 {
                if let Some(c) = test_text(
                    x,
                    y,
                    FIELD_TEXT_X0,
                    vy,
                    2,
                    &vbuf[..vn],
                    FIELD_VALUE_MAX_W,
                    THEME.on_surface,
                    fill,
                ) {
                    return c;
                }
            }
            if focused {
                let cx = FIELD_TEXT_X0 + std::design::font::measure(&vbuf[..vn], 2);
                if x >= cx && x < cx + 2 && (vy..vy + run_rows(2)).contains(&y) {
                    return THEME.primary;
                }

                // Touch Clear (X) button at DISPLAY_X1 - 88
                let clr_cx = DISPLAY_X1 - FIELD_CLEAR_DX;
                let clr_cy = fy + 48;
                let clr_dx = (x as i32) - clr_cx as i32;
                let clr_dy = (y as i32) - clr_cy as i32;
                if clr_dx * clr_dx + clr_dy * clr_dy <= 18 * 18 {
                    if clr_dx * clr_dx + clr_dy * clr_dy >= 16 * 16 {
                        return THEME.border;
                    }
                    let ix = clr_dx.abs();
                    let iy = clr_dy.abs();
                    if ix <= 5 && iy <= 5 && (ix == iy || ix == iy + 1 || ix == iy - 1) {
                        return THEME.on_surface_variant;
                    }
                    return THEME.surface_container_high;
                }

                // Touch Backspace (⌫) button at DISPLAY_X1 - 44
                let bs_cx = DISPLAY_X1 - 44;
                let bs_cy = fy + 48;
                let bs_dx = (x as i32) - bs_cx as i32;
                let bs_dy = (y as i32) - bs_cy as i32;
                if bs_dx * bs_dx + bs_dy * bs_dy <= 18 * 18 {
                    if bs_dx * bs_dx + bs_dy * bs_dy >= 16 * 16 {
                        return THEME.border;
                    }
                    // WHY a left chevron and a bar and not the clear button's
                    // X: both discs sit inside one field, and two identical
                    // X glyphs read as two clear buttons with no backspace.
                    let iy = bs_dy.abs();
                    let on_chevron = bs_dx >= -7 && bs_dx <= -2 && iy == (-2 - bs_dx);
                    let on_bar = bs_dy == 0 && bs_dx >= -2 && bs_dx <= 7;
                    if on_chevron || on_bar {
                        return THEME.on_surface;
                    }
                    return THEME.surface_container_high;
                }
            }
            return fill;
        }
        return THEME.surface;
    }

    // Browse: the selected contact, or the honest empty state.
    let Some(sel) = dialer.contact_sel else {
        let head: &[u8] = b"No contact selected";
        let hx = DISPLAY_X0 + std::design::font::center_x(head, 2, DISPLAY_W);
        if let Some(c) = test_text(x, y, hx, 206, 2, head,DISPLAY_W - 48,  THEME.on_surface, THEME.surface) {
            return c;
        }
        // WHY this says what to press: an empty card is a dead end unless it
        // names the control that fills it.
        let body: &[u8] = b"Press ADD to create one, or tap a row to select it";
        let mut buf = [0u8; 64];
        let n = std::design::font::ellipsize(body, 1, DISPLAY_W - 48, &mut buf);
        if n > 0 {
            let w = std::design::font::measure(&buf[..n], 1);
            let bx = DISPLAY_X0 + (DISPLAY_W.saturating_sub(w)) / 2;
            if let Some(c) = test_text(x, y, bx, 286, 1, &buf[..n],DISPLAY_W - 48,  THEME.on_surface_variant, THEME.surface) {
                return c;
            }
        }
        return THEME.surface;
    };

    let rec = if sel >= ContactEditor::UNSAVED_BASE {
        dialer
            .contacts
            .unsaved_record(sel - ContactEditor::UNSAVED_BASE)
    } else {
        dialer.contacts.record(sel)
    };
    let Some(rec) = rec else {
        return THEME.surface;
    };

    // Star toggle pill, so the card agrees with the row and allows direct toggling.
    let star_rect = (DISPLAY_X1 - 130, 186, 110, 36);
    let is_fav = sel < ContactEditor::UNSAVED_BASE && rec.is_favourite();
    let (star_label, star_color) = if is_fav {
        (b"Starred" as &[u8], THEME.warning)
    } else {
        (b"Favorite" as &[u8], THEME.on_surface_variant)
    };
    if (star_rect.0..star_rect.0 + star_rect.2).contains(&x) && (star_rect.1..star_rect.1 + star_rect.3).contains(&y) {
        if let Some(fill) = std::design::primitives::draw_pill(
            x,
            y,
            std::design::primitives::Rect::new(star_rect.0, star_rect.1, star_rect.2, star_rect.3),
            THEME.surface_container,
            Some(THEME.border),
        ) {
            let lw = std::design::font::measure(star_label, 1);
            let lx = star_rect.0 + (star_rect.2.saturating_sub(lw)) / 2;
            let ly = star_rect.1 + (star_rect.3.saturating_sub(run_rows(1))) / 2;
            if let Some(c) = test_text(x, y, lx, ly, 1, star_label, star_rect.2, star_color, fill) {
                return c;
            }
            return fill;
        }
    }

    let mut nbuf = [0u8; CONTACT_NAME_MAX];
    let nn = std::design::font::ellipsize(rec.name_bytes(), 2, DISPLAY_W - 48, &mut nbuf);
    let nw = std::design::font::measure(&nbuf[..nn], 2);
    let nx = DISPLAY_X0 + (DISPLAY_W.saturating_sub(nw)) / 2;
    if let Some(c) = test_text(x, y, nx, 196, 2, &nbuf[..nn],DISPLAY_W - 48,  THEME.on_surface, THEME.surface) {
        return c;
    }

    let mut ubuf = [0u8; CONTACT_NUM_MAX];
    let un = std::design::font::ellipsize(rec.number_bytes(), 2, DISPLAY_W - 48, &mut ubuf);
    let uw = std::design::font::measure(&ubuf[..un], 2);
    let ux = DISPLAY_X0 + (DISPLAY_W.saturating_sub(uw)) / 2;
    if let Some(c) = test_text(x, y, ux, 252, 2, &ubuf[..un],DISPLAY_W - 48,  THEME.on_surface_variant, THEME.surface) {
        return c;
    }

    // The three row actions, as labelled chips. WHY chips and not a menu: the app
    // has no menu surface, and a chip is a tap target the user can see.
    let chips: [(&[u8], u32, u32); 4] = [
        (b"CALL", THEME.success, THEME.surface_container),
        (b"MSG", THEME.primary, THEME.surface_container),
        (b"EDIT", THEME.surface_container_high, THEME.on_surface),
        (b"DELETE", THEME.error, THEME.surface_container),
    ];
    let chip_gap = 12usize;
    let chip_w = (DISPLAY_W - 3 * chip_gap) / 4;
    let chip_y = 312;
    for (i, (label, fill, ink)) in chips.iter().enumerate() {
        let cx0 = DISPLAY_X0 + i * (chip_w + chip_gap);
        if !(cx0..cx0 + chip_w).contains(&x) || !(chip_y..chip_y + 56).contains(&y) {
            continue;
        }
        if let Some(_base) = std::design::primitives::draw_pill(
            x,
            y,
            std::design::primitives::Rect::new(cx0, chip_y, chip_w, 56),
            *fill,
            None,
        ) {
            let mut lbuf = [0u8; 16];
            let ln = std::design::font::ellipsize(label, 2, chip_w - 16, &mut lbuf);
            if ln > 0 {
                let lw = std::design::font::measure(&lbuf[..ln], 2);
                let lx = cx0 + (chip_w.saturating_sub(lw)) / 2;
                let ly = chip_y + (56 - run_rows(2)) / 2;
                if let Some(c) = test_text(x, y, lx, ly, 2, &lbuf[..ln], chip_w, *ink, *fill) {
                    return c;
                }
            }
            return *fill;
        }
    }

    THEME.surface
}

/// The contact list rows, then the honest capacity/durability notice.
///
/// WHY saved and unsaved rows are drawn as two groups with a divider between
/// them: the durable list is a statement about storage, and the unsaved list is
/// not. Mixing them would let a contact the kernel refused to store look exactly
/// like one it accepted.
///
/// WHY the list scrolls: the store holds twelve contacts and the unsaved group
/// can hold twelve more, while the window holds eight rows. The rows below the
/// fold were unreachable, so the list carries the platform's scroll model and a
/// thumb beside it.
fn contacts_list_pixel(x: usize, y: usize, dialer: &DialerApp) -> u32 {
    // Row plates. Saved rows come first, then unsaved ones, because the durable
    // list is the one the user asked the device to keep.
    let total_rows = dialer.contacts.len() + dialer.contacts.unsaved_len();
    let offset = dialer.contacts_scroll.offset;
    let mut row = 0usize;
    let mut cursor = LIST_Y0;
    while row < total_rows {
        let sel = if row < dialer.contacts.len() {
            row
        } else {
            ContactEditor::UNSAVED_BASE + (row - dialer.contacts.len())
        };
        let rec = if sel >= ContactEditor::UNSAVED_BASE {
            dialer
                .contacts
                .unsaved_record(sel - ContactEditor::UNSAVED_BASE)
        } else {
            dialer.contacts.record(sel)
        };
        let Some(rec) = rec else {
            row += 1;
            continue;
        };
        // WHY the favourites filter hides rows instead of greying them: a row the
        // user asked not to see is not a row at all, and the notice below states
        // the filter so the shorter list is never mistaken for a lost contact.
        if dialer.contacts_fav_only && !rec.is_favourite() {
            row += 1;
            continue;
        }
        // The row's place on screen, once the list is scrolled.
        let scrolled_y = cursor.saturating_sub(offset);
        if !(scrolled_y..scrolled_y + CONTACT_ROW_H).contains(&y) {
            cursor += CONTACT_ROW_H;
            row += 1;
            continue;
        }
        let selected = dialer.contact_sel == Some(sel);
        // The platform's list-row treatment: the fill, the corner radius and the
        // divider are the platform's, so a row here looks like a row anywhere
        // else in the system.
        let style = components::list_row_style(&THEME, selected);
        if std::design::primitives::draw_rounded_rect(
            x,
            y,
            std::design::primitives::Rect::new(DISPLAY_X0, scrolled_y, DISPLAY_W, CONTACT_ROW_H),
            style.radius,
            style.fill,
            None,
            0,
        )
        .is_none()
        {
            return THEME.surface;
        }
        // The divider, in the platform's divider ink.
        if y == scrolled_y + CONTACT_ROW_H - 1 && x >= DISPLAY_X0 && x < DISPLAY_X1 {
            return style.divider;
        }
        // Vibrant avatar disc with the contact's initial.
        let av_cx = DISPLAY_X0 + 30;
        let av_cy = scrolled_y + CONTACT_ROW_H / 2;
        let dx = x as i32 - av_cx as i32;
        let dy = y as i32 - av_cy as i32;
        let d2 = dx * dx + dy * dy;
        if d2 <= 20 * 20 {
            let av_bg = contact_avatar_color(rec.name_bytes());
            if d2 >= 18 * 18 {
                return std::design::font::blend(av_bg, THEME.ink_on_dark, 48);
            }
            let initial = [rec.initial()];
            let iw = std::design::font::measure(&initial, 1);
            let ix = (av_cx as i32 - iw as i32 / 2).max(0) as usize;
            let iy = (av_cy as i32 - run_rows(1) as i32 / 2).max(0) as usize;
            if let Some(t) = test_text_bold(x, y, ix, iy, 1, &initial,24,  THEME.ink_on_dark, av_bg) {
                return t;
            }
            return av_bg;
        }
            let tx = DISPLAY_X0 + 68;
            let mut nbuf = [0u8; CONTACT_NAME_MAX];
            let nn = std::design::font::ellipsize(rec.name_bytes(), 1, DISPLAY_W - 150, &mut nbuf);
            if nn > 0 {
                if let Some(t) =
                    test_text(x, y, tx, scrolled_y + 8, 1, &nbuf[..nn],DISPLAY_W - 150,  THEME.on_surface, style.fill)
                {
                    return t;
                }
            }
            let mut ubuf = [0u8; CONTACT_NUM_MAX];
            let un = std::design::font::ellipsize(rec.number_bytes(), 1, DISPLAY_W - 150, &mut ubuf);
            if un > 0 {
                if let Some(t) = test_text(
                    x,
                    y,
                    tx,
                    scrolled_y + 36,
                    1,
                    &ubuf[..un],DISPLAY_W - 150, 
                    THEME.on_surface_variant,
                    style.fill,
                ) {
                    return t;
                }
            }
            // Star. Filled for a saved favourite, outlined for an unsaved one:
            // an unsaved contact cannot be starred in storage, and drawing it
            // filled would claim otherwise.
            let sdx = x as i32 - STAR_CX as i32;
            let sdy = y as i32 - (scrolled_y + CONTACT_ROW_H / 2) as i32;
            if sdx * sdx + sdy * sdy <= STAR_R * STAR_R {
                // WHY a star and not a disc: this control toggles a favourite,
                // and an unstarred contact used to be a plain grey circle that
                // read as decoration. A filled star means starred; an outlined
                // one is the same shape waiting to be filled.
                let on_star = (sdy <= 0 && sdx.abs() <= 12 + sdy.abs() / 2)
                    || (sdy > 0 && sdx.abs() <= 12 - sdy.abs());
                if on_star {
                    if rec.is_favourite() {
                        return THEME.warning;
                    }
                    let inside = (sdy <= 0 && sdx.abs() <= 8 + sdy.abs() / 2)
                        || (sdy > 0 && sdx.abs() <= 8 - sdy.abs());
                    if !inside {
                        return THEME.on_surface_variant;
                    }
                }
                return style.fill;
            }
            return style.fill;
        }


    // Notice band under the list.
    if (NOTICE_Y0..NOTICE_Y1).contains(&y) {
        return contacts_notice_pixel(x, y, dialer);
    }

    // The scroll indicator, drawn beside the list it belongs to.
    if let Some(c) = scroll_indicator(
        x,
        y,
        &dialer.contacts_scroll,
        LIST_Y0,
        LIST_VIEWPORT_Y1 - LIST_Y0,
    ) {
        return c;
    }
    THEME.surface
}

/// The honest capacity / durability notice under the contact list.
///
/// WHY this band always says something: the list on its own cannot tell the user
/// whether it is what storage holds, whether the store is full, or whether every
/// row in it is session-only. All three have to be visible without a log reader.
fn contacts_notice_pixel(x: usize, y: usize, dialer: &DialerApp) -> u32 {
    if !(NOTICE_Y0..NOTICE_Y1).contains(&y) || x < DISPLAY_X0 || x >= DISPLAY_X1 {
        return THEME.surface;
    }
    let used = dialer.contacts.len();
    let unsaved = dialer.contacts.unsaved_len();
    let state = dialer.contacts.state;

    // The first line is the durability statement, which is the one that matters.
    let durable = state == ContactStoreState::Written || state == ContactStoreState::Loaded;
    let ink = if durable {
        THEME.on_surface_variant
    } else {
        THEME.warning
    };
    if let Some(c) = test_text(
        x,
        y,
        DISPLAY_X0 + 24,
        NOTICE_LINE1_Y,
        NOTICE_SCALE,
        state.as_str().as_bytes(),DISPLAY_W - 48, 
        ink,
        THEME.surface,
    ) {
        return c;
    }

    // The second line is capacity, plus the unsaved count when there is one.
    let mut line = [0u8; 96];
    let mut n = 0usize;
    if dialer.contacts_fav_only {
        let mut fav_count = 0usize;
        for i in 0..dialer.contacts.len() {
            if let Some(r) = dialer.contacts.record(i) {
                if r.is_favourite() { fav_count += 1; }
            }
        }
        for i in 0..dialer.contacts.unsaved_len() {
            if let Some(r) = dialer.contacts.unsaved_record(i) {
                if r.is_favourite() { fav_count += 1; }
            }
        }
        let head = b"Favorites only: ";
        line[..head.len()].copy_from_slice(head);
        n += head.len();
        n = put_u16(&mut line, n, fav_count as u16);
        let mid = b" of ";
        line[n..n + mid.len()].copy_from_slice(mid);
        n += mid.len();
        n = put_u16(&mut line, n, used as u16);
    } else {
        let head = b"Saved ";
        line[n..n + head.len()].copy_from_slice(head);
        n += head.len();
        n = put_u16(&mut line, n, used as u16);
        // WHY the capacity is formatted from the constant and not written
        // as the literal "8": the store holds twelve contacts, so the notice
        // used to tell the user it was full at eight while the "STORE FULL"
        // flag only appeared at twelve. The two lines disagreed about the
        // same store.
        let mid = b" of ";
        line[n..n + mid.len()].copy_from_slice(mid);
        n += mid.len();
        n = put_u16(&mut line, n, MAX_CONTACTS as u16);
    }
    if unsaved > 0 {
        let tail = b" | unsaved ";
        line[n..n + tail.len()].copy_from_slice(tail);
        n += tail.len();
        // WHY a decimal formatter and not a single digit: the old code
        // clamped the count to nine, so a store holding ten contacts
        // printed "9" -- a number the store never held.
        n = put_u16(&mut line, n, unsaved as u16);
    }
    if used >= MAX_CONTACTS && !dialer.contacts_fav_only {
        let tail = b" | STORE FULL";
        line[n..n + tail.len()].copy_from_slice(tail);
        n += tail.len();
    }
    let mut buf = [0u8; 96];
    let tn = std::design::font::ellipsize(&line[..n], NOTICE_SCALE, DISPLAY_W - 48, &mut buf);
    if tn > 0 {
        if let Some(c) = test_text(
            x,
            y,
            DISPLAY_X0 + 24,
            NOTICE_LINE2_Y,
            NOTICE_SCALE,
            &buf[..tn],DISPLAY_W - 48, 
            THEME.on_surface_variant,
            THEME.surface,
        ) {
            return c;
        }
    }
    THEME.surface
}

/// The SOS / emergency disc.
///
/// WHY it is its own function: the keypad action row and the contacts action row
/// both draw it, and the SOS path is the one control whose behaviour must never
/// change with what else is on screen. Extracting it means one geometry, one hit
/// test and one log line for both, so a refactor of either section cannot make
/// the emergency control drift.
fn emergency_dial_pixel(x: usize, y: usize, dialer: &DialerApp) -> Option<u32> {
    let s_dx = (x as i32) - SOS_CX as i32;
    let s_dy = (y as i32) - SOS_CY as i32;
    let s_d2 = s_dx * s_dx + s_dy * s_dy;
    if s_d2 > SOS_R * SOS_R {
        return None;
    }
    if s_d2 >= (SOS_R - 2) * (SOS_R - 2) {
        return Some(THEME.error);
    }
    // "SOS" is scale 1 bold vector outline, comfortably centered in the disc with clean margins.
    let sos_rows = run_rows(SOS_SCALE);
    let sos_w = std::design::font::measure(b"SOS", SOS_SCALE);
    let sos_x0 = SOS_CX.saturating_sub(sos_w / 2);
    let sos_y0 = SOS_CY.saturating_sub(sos_rows / 2);
    if y >= sos_y0 && y < sos_y0 + sos_rows {
        if let Some(c) = test_text_bold(x, y, sos_x0, sos_y0, SOS_SCALE, b"SOS",60,  THEME.emergency, THEME.error_container) {
            return Some(c);
        }
    }
    let _ = dialer;
    Some(THEME.error_container)
}

/// The contacts action row: the section's primary and secondary actions.
///
/// WHY the emergency disc is not here: SOS belongs to the keypad where a call
/// is being placed. Next to SAVE and CANCEL in a contact editor it read as one
/// more editing control, and an accidental tap would dial emergency services
/// from a screen where nobody expects a call.
fn contacts_action_pixel(x: usize, y: usize, dialer: &DialerApp) -> u32 {
    if !(ACTION_Y0..ACTION_Y1).contains(&y) {
        return THEME.surface;
    }

    let editing = dialer.contact_edit.is_some();
    let has_sel = dialer.contact_sel.is_some();
    // Centre disc: SAVE while editing, CALL with a selection, ADD otherwise.
    let centre: (Option<u32>, &[u8]) = if editing {
        (Some(THEME.success), b"SAVE")
    } else if has_sel {
        (Some(THEME.success), b"CALL")
    } else {
        (Some(THEME.primary), b"ADD")
    };
    // Right disc: DELETE while editing an existing contact, CANCEL while editing
    // a new one, EDIT with a selection, and the favourites filter otherwise.
    let right: (Option<u32>, &[u8]) = if editing {
        let exists = matches!(dialer.contact_edit.and_then(|e| e.target), Some(_));
        if exists {
            (Some(THEME.error), b"DELETE")
        } else {
            (Some(color::SURFACE_RAISED), b"CANCEL")
        }
    } else if has_sel {
        (Some(color::SURFACE_RAISED), b"EDIT")
    } else if dialer.contacts_fav_only {
        (Some(color::SURFACE_RAISED), b"ALL")
    } else {
        (Some(color::SURFACE_RAISED), b"FAVS")
    };

    for (cx, (fill, label)) in [
        (CALL_CX, centre),
        (CONTACTS_CX, right),
    ] {
        let dx = x as i32 - cx as i32;
        let dy = y as i32 - C_ACTION_Y as i32;
        let r = if cx == CALL_CX { CALL_R } else { CONTACTS_R };
        if dx * dx + dy * dy > r * r {
            continue;
        }
        let fill = fill.unwrap_or(color::SURFACE_RAISED);
        if dx * dx + dy * dy >= (r - 3) * (r - 3) {
            return THEME.border;
        }
        let mut lbuf = [0u8; 16];
        let ln = std::design::font::ellipsize(label, 1, 2 * r as usize - 12, &mut lbuf);
        if ln > 0 {
            let lw = std::design::font::measure(&lbuf[..ln], 1);
            let lx = (cx as i32 - lw as i32 / 2).max(0) as usize;
            let ly = C_ACTION_Y - run_rows(1) / 2;
            // WHY the success disc takes dark ink: the theme's green is light
            // enough that white label text on it is about 2.3:1, below the
            // platform's own contrast floor. The near-black ink on the same
            // green is above 5:1.
            let text_color = if fill == color::SURFACE_RAISED {
                THEME.on_surface
            } else if fill == THEME.success {
                THEME.ink_on_light
            } else {
                THEME.on_primary
            };
            if let Some(c) = test_text(x, y, lx, ly, 1, &lbuf[..ln],160,  text_color, fill) {
                return c;
            }
        }
        return fill;
    }
    THEME.surface
}

/// Emergency / call / contacts row. Only drawn in the Keypad section.
fn action_pixel(x: usize, y: usize, dialer: &DialerApp) -> u32 {
    if !(ACTION_Y0..ACTION_Y1).contains(&y) {
        return THEME.surface;
    }
    if let Some(c) = emergency_dial_pixel(x, y, dialer) {
        return c;
    }

    // Call / Hangup Central Button: wide stadium pill (~160x76) in inverted surface (black)
    let c_dx = (x as i32) - CALL_CX as i32;
    let c_dy = (y as i32) - CALL_CY as i32;
    let dx_clamped = (c_dx.abs() - CALL_PILL_HALF_SPINE).max(0);
    let c_d2 = dx_clamped * dx_clamped + c_dy * c_dy;
    if c_d2 <= CALL_PILL_R * CALL_PILL_R {
        let in_call = dialer.state != CallState::Idle;
        let (bg_color, glyph) = if in_call {
            (THEME.error, THEME.ink_on_dark)
        } else if dialer.active_len > 0 {
            (THEME.inverted_surface(), THEME.ink_on_dark)
        } else {
            // WHY the empty keypad recesses the pill: an all-black call button
            // over an empty field promises an action the tap handler refuses.
            (THEME.surface_container_high, THEME.muted)
        };

        // Vector Phone Handset
        if !in_call {
            let n1 = (c_dx + 12) * (c_dx + 12) + (c_dy + 12) * (c_dy + 12);
            let n2 = (c_dx - 12) * (c_dx - 12) + (c_dy - 12) * (c_dy - 12);
            if n1 <= 36 || n2 <= 36 {
                return glyph;
            }
            let diff = c_dx - c_dy;
            if diff.abs() <= 5 && c_dx >= -12 && c_dx <= 12 && c_dy >= -12 && c_dy <= 12 {
                return glyph;
            }
        } else {
            let n1 = (c_dx + 14) * (c_dx + 14) + (c_dy - 2) * (c_dy - 2);
            let n2 = (c_dx - 14) * (c_dx - 14) + (c_dy - 2) * (c_dy - 2);
            if n1 <= 36 || n2 <= 36 {
                return glyph;
            }
            if c_dy >= -8 && c_dy <= -2 && c_dx >= -14 && c_dx <= 14 {
                return glyph;
            }
        }

        return bg_color;
    }

    // Backspace button (centre 582, 1065, r 42). Active only when dialed number has digits.
    if dialer.active_len > 0 {
        let b_dx = (x as i32) - BACKSPACE_CX as i32;
        let b_dy = (y as i32) - BACKSPACE_CY as i32;
        let b_d2 = b_dx * b_dx + b_dy * b_dy;
        if b_d2 <= BACKSPACE_R * BACKSPACE_R {
            if b_d2 >= (BACKSPACE_R - 2) * (BACKSPACE_R - 2) {
                return THEME.border;
            }
            // Crisp vector backspace tag glyph
            let in_tag = if b_dx >= -15 && b_dx <= -4 {
                b_dy.abs() <= (b_dx + 15)
            } else if b_dx > -4 && b_dx <= 15 {
                b_dy.abs() <= 11
            } else {
                false
            };
            if in_tag {
                let on_rim = if b_dx == -15 || b_dx == 15 || (b_dx > -4 && b_dy.abs() >= 10) {
                    true
                } else if b_dx >= -15 && b_dx <= -4 {
                    let diff = b_dy.abs() - (b_dx + 15);
                    diff == 0 || diff == -1
                } else {
                    false
                };
                let cx = b_dx - 5;
                let cy = b_dy;
                let on_x = cx.abs() <= 4 && cy.abs() <= 4 && (cx == cy || cx == -cy || cx == cy + 1 || cx == cy - 1);
                if on_rim || on_x {
                    return THEME.on_surface;
                }
            }
            return THEME.surface_container;
        }
    }

    THEME.surface
}

/// Bottom navigation dock. All four slots are live: Favorites opens the contacts
/// list filtered to the contacts the user starred, and the other three select a
/// section.
fn dock_pixel(x: usize, y: usize, dialer: &DialerApp) -> u32 {
    if !(DOCK_Y0..DOCK_Y1).contains(&y) {
        return THEME.surface;
    }
    // Top hairline divider
    if y == DOCK_Y0 {
        return THEME.border;
    }

    let mut slot = 4usize;
    for (i, x0) in DOCK_ITEM_X0.iter().enumerate() {
        if x >= *x0 && x < *x0 + DOCK_ITEM_W {
            slot = i;
            break;
        }
    }
    if slot == 4 {
        return THEME.surface_container;
    }

    let active = match (slot, dialer.section) {
        (1, DialerSection::Recents) | (2, DialerSection::Keypad) => true,
        (0, DialerSection::Contacts) | (3, DialerSection::Contacts) => {
            if slot == 0 {
                dialer.contacts_fav_only
            } else {
                !dialer.contacts_fav_only
            }
        }
        _ => false,
    };

    let cx = DOCK_ITEM_X0[slot] + DOCK_ITEM_W / 2;
    let cy = DOCK_ICON_Y0 + DOCK_ICON_H / 2;

    let ink = if active { THEME.primary } else { THEME.on_surface_variant };
    let plate = THEME.surface_container;

    // Icon band
    if y >= DOCK_ICON_Y0 && y < DOCK_ICON_Y0 + DOCK_ICON_H {
        match slot {
            2 => {
                // Keypad icon: a 3x3 dot grid.
                let dot_dx = (x as i32) - cx as i32;
                let dot_dy = (y as i32) - cy as i32;
                if dot_dx.abs() <= 10 && dot_dy.abs() <= 10 {
                    if (dot_dx % 8).abs() <= 1 && (dot_dy % 8).abs() <= 1 {
                        return ink;
                    }
                }
            }
            1 => {
                // Recents icon: a clock face with two hands.
                let d2 = (x as i32 - cx as i32).pow(2) + (y as i32 - cy as i32).pow(2);
                if (144..=169).contains(&d2) {
                    return ink;
                }
                if y == cy && x >= cx - 7 && x <= cx {
                    return ink;
                }
                if x == cx && y >= cy - 9 && y <= cy + 1 {
                    return ink;
                }
            }
            0 => {
                // Favorites icon: a five-point star.
                let sdx = x as i32 - cx as i32;
                let sdy = y as i32 - cy as i32;
                let on = sdy <= 0 && sdx.abs() as i32 <= 12 + sdy.abs() / 2
                    || sdy > 0 && sdx.abs() as i32 <= 12 - sdy.abs();
                if on && sdx.abs() <= 14 && sdy.abs() <= 13 {
                    return if dialer.contacts.favourites() > 0 {
                        THEME.warning
                    } else {
                        ink
                    };
                }
            }
            _ => {
                // Contacts icon: a person silhouette.
                let pdx = x as i32 - cx as i32;
                let pdy = y as i32 - cy as i32;
                if pdx * pdx + (pdy + 8) * (pdy + 8) <= 49 {
                    return ink;
                }
                if pdy >= 2 && pdy <= 14 && pdx.abs() <= (pdy - 2) + 6 {
                    return ink;
                }
            }
        }
    }

    // Label band
    let label_x = DOCK_ITEM_X0[slot]
        + std::design::font::center_x(DOCK_LABELS[slot], DOCK_LABEL_SCALE, DOCK_ITEM_W);
    if y >= DOCK_LABEL_Y0 && y < DOCK_LABEL_Y0 + run_rows(DOCK_LABEL_SCALE) {
        if let Some(c) = test_text(
            x,
            y,
            label_x,
            DOCK_LABEL_Y0,
            DOCK_LABEL_SCALE,
            DOCK_LABELS[slot],DOCK_ITEM_W, 
            ink,
            plate,
        ) {
            return c;
        }
    }

    // Active pill background under icon
    if active {
        let pill_rect = std::design::primitives::Rect::new(cx - 32, cy - 16, 64, 32);
        if let Some(p_color) = std::design::primitives::draw_pill(x, y, pill_rect, THEME.primary_container, None) {
            return p_color;
        }
    }

    plate
}

#[inline(never)]
/// The heading that owns the number band outside the keypad.
///
/// Recents lists its own history and Settings draws its own title below this
/// band, so Settings leaves the band plain surface.
fn section_band_pixel(x: usize, y: usize, dialer: &DialerApp) -> u32 {
    if dialer.section != DialerSection::Recents {
        return THEME.surface;
    }
    let x0 = DISPLAY_X0 + 24;
    if let Some(c) = test_text_bold(
        x,
        y,
        x0,
        DISPLAY_Y0 + 12,
        2,
        b"Call history",
        DISPLAY_W - 48,
        THEME.on_surface,
        THEME.surface,
    ) {
        return c;
    }
    if let Some(c) = test_text(
        x,
        y,
        x0,
        DISPLAY_Y0 + 72,
        1,
        b"Newest call first",
        DISPLAY_W - 48,
        THEME.on_surface_variant,
        THEME.surface,
    ) {
        return c;
    }
    THEME.surface
}

fn get_dialer_pixel(x: usize, y: usize, dialer: &DialerApp) -> u32 {
    // 1. Status bar & header (y: 0..HEADER_H, the rule on the last row)
    if y < HEADER_H + 1 {
        return header_pixel(x, y, dialer);
    }

    // 2. Segmented navigation tabs (y: 105..155)
    if (TAB_BAND_Y0..TAB_BAND_Y0 + TAB_BAND_H).contains(&y) {
        return tabs_pixel(x, y, dialer);
    }

    // 3. Call status & dialed number display (y: 170..380). In the Contacts
    //    section the same band is the selected-contact card or the editor's
    //    fields, so every section owns the band rather than fighting over it.
    //    Recents and Settings get their own heading instead of the dialed
    //    number, which used to follow the user out of the keypad. While a call
    //    is up the card stays in every section, because it carries the only
    //    description of that call.
    if (DISPLAY_Y0..DISPLAY_Y1).contains(&y) {
        return match dialer.section {
            DialerSection::Contacts => contacts_card_pixel(x, y, dialer),
            DialerSection::Keypad => display_pixel(x, y, dialer),
            _ if dialer.state != CallState::Idle => display_pixel(x, y, dialer),
            _ => section_band_pixel(x, y, dialer),
        };
    }

    // 4. Section content (y: 410..1140): the dial pad and the call controls in
    //    the Keypad section, the real call ledger in Recents, the contact
    //    store -- the list while browsing, or the dial pad typed as a multi-tap
    //    text entry -- in Contacts, and the dialer's own settings in Settings.
    if (CONTENT_Y0..CONTENT_Y1).contains(&y) {
        return match dialer.section {
            DialerSection::Keypad => {
                let c = keypad_pixel(x, y);
                if c != THEME.surface {
                    return c;
                }
                action_pixel(x, y, dialer)
            }
            DialerSection::Recents => recents_pixel(x, y, dialer),
            DialerSection::Settings => settings_pixel(x, y, dialer),
            DialerSection::Contacts => {
                if dialer.contact_edit.is_some() {
                    // The editor reuses the real dial pad, so its geometry, its
                    // hit test and its sublabels are the same ones the keypad
                    // section uses. Only what a press produces changes.
                    let c = editor_keypad_pixel(x, y, dialer);
                    if c != THEME.surface {
                        return c;
                    }
                    return contacts_action_pixel(x, y, dialer);
                }
                if (LIST_Y0..NOTICE_Y1).contains(&y) {
                    return contacts_list_pixel(x, y, dialer);
                }
                contacts_action_pixel(x, y, dialer)
            }
        };
    }

    // 4b. Editor refusal hint in the free band above the dock.
    //
    // WHY it exists: SAVE on an incomplete contact used to refuse with a log
    // line only, so the button looked dead. The hint names the two things the
    // store requires and disappears on the next keystroke.
    if dialer.section == DialerSection::Contacts
        && dialer.contact_edit.is_some()
        && dialer.editor_error
        && (CONTENT_Y1..DOCK_Y0).contains(&y)
    {
        let msg = b"A contact needs a name and a number";
        let w = std::design::font::measure(msg, 1);
        let x0 = (W.saturating_sub(w)) / 2;
        if let Some(c) = test_text(x, y, x0, 1180, 1, msg, W, THEME.error, THEME.surface) {
            return c;
        }
        return THEME.surface;
    }

    // 5. Bottom navigation dock (y: 1260..1380)
    if (DOCK_Y0..DOCK_Y1).contains(&y) {
        return dock_pixel(x, y, dialer);
    }

    // 6. Home Gesture Pill (x: 260..460, y: 1418..1424)
    if y >= 1418 && y < 1424 && x >= 260 && x < 460 {
        return THEME.on_surface_variant;
    }

    THEME.surface
}

fn render_dialer_ui(surface: u32, dialer: &DialerApp) {
    render_dialer_band(surface, dialer, 0, H);
}

/// Re-rasterises and writes only rows `y0..y1`.
///
/// WHY a band and not a rectangle: a row of the surface is one contiguous run in
/// the surface buffer, so `offset = y0 * W` writes a band with no stride
/// bookkeeping. Every band starts on a row boundary, so the running `x`/`y`
/// cursor below is exact.
///
/// WHY this is correct rather than a shortcut: the band is chosen from what the
/// keystroke actually changed. Typing a digit moves the number and nothing else
/// - between 170 and 380 of the 1440 rows - and the keypad discs beside it are
/// identical before and after, so they are left exactly as drawn.
fn render_dialer_band(surface: u32, dialer: &DialerApp, y0: usize, y1: usize) {
    let mut chunk = [0u32; CHUNK];
    let mut offset = y0 * W;
    let end = y1.min(H) * W;
    while offset < end {
        let count = (end - offset).min(CHUNK);
        let mut x = offset % W;
        let mut y = offset / W;
        for i in 0..count {
            chunk[i] = get_dialer_pixel(x, y, dialer);
            x += 1;
            if x == W {
                x = 0;
                y += 1;
            }
        }
        let _ = std::surface_write(surface, offset, &chunk[..count]);
        offset += count;
    }
}

// -----------------------------------------------------------------------------
// Input handling. Every region below is drawn at the exact same coordinates by
// get_dialer_pixel (shared constants), so each control is hit-testable where it
// is rendered.
// -----------------------------------------------------------------------------

/// Which part of the surface an action changed.
///
/// WHY this exists: rendering the dialer re-rasterised all 1,036,800 pixels
/// through `get_dialer_pixel` for every touch event and every keystroke, so
/// typing a number cost the same as a full screen transition.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Dirty {
    /// Nothing changed; do not render.
    None,
    /// Only the dialed number / outcome line / backspace disc moved.
    Display,
    /// Only the contact editor's fields moved.
    Fields,
    /// Section, call state or telemetry changed: the whole surface.
    All,
}

/// Merges two dirty regions reported in the same input batch.
///
/// WHY this exists rather than "last one wins": one `input_event_poll` drain can
/// carry two keypresses, one of which reaches the display card and one of which
/// steps the section. Keeping only one would leave the other stale.
fn merge(a: Dirty, b: Dirty) -> Dirty {
    if a == Dirty::None {
        return b;
    }
    if b == Dirty::None || a == b {
        return a;
    }
    Dirty::All
}

enum Tap<'a> {
    None,
    Redraw(Dirty),
    Leave(&'a str),
}

fn in_circle(x: usize, y: usize, cx: usize, cy: usize, r: i32) -> bool {
    let dx = (x as i32) - cx as i32;
    let dy = (y as i32) - cy as i32;
    dx * dx + dy * dy <= r * r
}

/// Places the entered number, or hangs up an active call.
fn toggle_call(dialer: &mut DialerApp) -> Dirty {
    if dialer.state != CallState::Idle {
        dialer.hangup();
        return Dirty::All;
    }
    if dialer.active_len == 0 {
        // Unreachable from a tap: the control is drawn recessed while the entry
        // is empty. Reachable from the keyboard, so it still reports honestly.
        std::log("[DIALER] Call requested with no number entered; no telephony request was issued.",
        )
        .ok();
        return Dirty::None;
    }
    let mut num_buf = [0u8; 32];
    let len = dialer.active_len;
    num_buf[..len].copy_from_slice(&dialer.active_number[..len]);
    let Ok(target) = core::str::from_utf8(&num_buf[..len]) else {
        std::log("[DIALER] Invalid dial string.").ok();
        return Dirty::None;
    };
    if dialer.dial(target).is_ok() {
        std::log("[DIALER] Telephony service accepted the call.").ok();
    } else {
        std::log("[DIALER] Call could not be established.").ok();
    }
    Dirty::All
}

fn handle_tap(dialer: &mut DialerApp, px: u32, py: u32) -> Tap<'static> {
    let x = px as usize;
    let y = py as usize;

    // 1. Back control in the header, or the home gesture bar: exit to launcher.
    //
    // WHY nested surfaces close first: the header control is the only Back on
    // the screen, and leaving the app from an open contact editor or ringtone
    // picker threw away what the user had typed or chosen. The home gesture
    // still leaves from anywhere.
    if x >= 240 && x <= 480 && y >= 1400 {
        return Tap::Leave("[DIALER APP] Home gesture pressed. Exiting to launcher.");
    }
    if std::design::back_button::contains(x, y)
        || (x >= 24 && x < 100 && y >= HEADER_TOP && y < HEADER_TOP + 48)
    {
        if dialer.section == DialerSection::Contacts && dialer.contact_edit.is_some() {
            dialer.cancel_edit_contact();
            return Tap::Redraw(Dirty::All);
        }
        if dialer.section == DialerSection::Settings
            && dialer.settings_view == SettingsView::RingtonePicker
        {
            dialer.settings_view = SettingsView::Main;
            dialer.refresh_settings_values();
            return Tap::Redraw(Dirty::All);
        }
        return Tap::Leave("[DIALER APP] Back control pressed. Exiting to launcher.");
    }

    // 2. Segmented tabs. All four are live sections.
    if (TAB_BAND_Y0..TAB_BAND_Y0 + TAB_BAND_H).contains(&y) {
        if x >= TAB1_X0 && x < TAB1_X1 {
            dialer.section = DialerSection::Keypad;
            let _ = std::audio_haptic_click();
            return Tap::Redraw(Dirty::All);
        }
        if x >= TAB2_X0 && x < TAB2_X1 {
            dialer.section = DialerSection::Recents;
            let _ = std::audio_haptic_click();
            return Tap::Redraw(Dirty::All);
        }
        if x >= TAB3_X0 && x < TAB3_X1 {
            // WHY the Contacts tab clears the favourites filter: the tab is
            // the unfiltered contact list, while the Favorites dock slot is
            // the filtered one. Carrying the filter over made both controls
            // read active and left the tab showing a subset of its own list.
            dialer.open_contacts(false);
            let _ = std::audio_haptic_click();
            return Tap::Redraw(Dirty::All);
        }
        if x >= TAB4_X0 && x < TAB4_X1 {
            dialer.section = DialerSection::Settings;
            dialer.settings_view = SettingsView::Main;
            dialer.refresh_settings_values();
            let _ = std::audio_haptic_click();
            return Tap::Redraw(Dirty::All);
        }
    }

    // 3. Bottom dock. All four slots have a real destination.
    if (DOCK_Y0..DOCK_Y1).contains(&y) && x >= 24 && x < 696 {
        for slot in 0..4 {
            let x0 = DOCK_ITEM_X0[slot];
            if x < x0 || x >= x0 + DOCK_ITEM_W {
                continue;
            }
            let _ = std::audio_haptic_click();
            match slot {
                // The slot order is the icon order: star, clock, keypad,
                // person. Favorites and Contacts both open the contact list;
                // Favorites filters it to the starred contacts.
                0 => {
                    dialer.open_contacts(true);
                    std::log("[DIALER APP] Favorites: showing starred contacts.").ok();
                    return Tap::Redraw(Dirty::All);
                }
                1 => {
                    dialer.section = DialerSection::Recents;
                    return Tap::Redraw(Dirty::All);
                }
                2 => {
                    dialer.section = DialerSection::Keypad;
                    return Tap::Redraw(Dirty::All);
                }
                _ => {
                    dialer.open_contacts(false);
                    return Tap::Redraw(Dirty::All);
                }
            }
        }
    }



    // 5. Section content: the keypad and action row in the Keypad section, the
    if dialer.section == DialerSection::Recents {
        // The ledger scrolls, so the row under the tap is the one at the
        // scrolled position, not the one at the unshifted position.
        let offset = dialer.recents_scroll.offset;
        if y >= RECENTS_Y0 && y < RECENTS_VIEWPORT_Y1 {
            let content_y = y + offset;
            if content_y >= RECENTS_Y0 {
                let row_idx = (content_y - RECENTS_Y0) / RECENTS_ROW_H;
                if row_idx < dialer.recents_len {
                    let rec = &dialer.recents[row_idx];
                    dialer.active_number = [0u8; 32];
                    let n = rec.len.min(32);
                    dialer.active_number[..n].copy_from_slice(&rec.number[..n]);
                    dialer.active_len = n;
                    dialer.section = DialerSection::Keypad;
                    std::log("[DIALER APP] Loaded number from recents.").ok();
                    let _ = std::audio_haptic_click();
                    return Tap::Redraw(Dirty::All);
                }
            }
        }
        return Tap::None;
    }

    if dialer.section == DialerSection::Settings {
        return handle_settings_tap(dialer, x, y);
    }

    if dialer.section == DialerSection::Contacts {
        return handle_contacts_tap(dialer, x, y);
    }

    if dialer.section == DialerSection::Keypad {
        // Leading-plus control on the dialed-number card.
        if dialer.state == CallState::Idle
            && dialer.active_len == 0
            && in_circle(x, y, PLUS_CX, PLUS_CY, 24)
        {
            dialer.input_digit('+');
            let _ = std::audio_haptic_click();
            return Tap::Redraw(Dirty::Display);
        }
        // Clear (X) disc on dialed number card
        if dialer.active_len > 0 && in_circle(x, y, DISPLAY_X1 - 44, 236, 24) {
            dialer.clear_entry();
            return Tap::Redraw(Dirty::Display);
        }
        // Action pills in display card when active_len > 0
        if dialer.active_len > 0 && y >= 295 && y < 345 {
            if x >= 80 && x < 330 {
                let mut num_buf = [0u8; 32];
                let len = dialer.active_len.min(32);
                num_buf[..len].copy_from_slice(&dialer.active_number[..len]);
                dialer.start_add_contact_with_number(&num_buf[..len]);
                let _ = std::audio_haptic_click();
                return Tap::Redraw(Dirty::All);
            }
            if x >= 390 && x < 640 {
                let mut num_buf = [0u8; 32];
                let len = dialer.active_len.min(32);
                num_buf[..len].copy_from_slice(&dialer.active_number[..len]);
                if hand_off_to_messaging(&num_buf[..len]) {
                    let _ = std::audio_haptic_click();
                }
                return Tap::Redraw(Dirty::Display);
            }
        }

        // Keypad Matrix
        for row in 0..4 {
            let by = ROW_Y[row];
            if y >= by && y < by + BTN_H {
                for col in 0..3 {
                    let bx = COL_X[col];
                    if x >= bx && x < bx + BTN_W {
                        if let Some(digit) = get_keypad_char(row, col) {
                            dialer.input_digit(digit);
                            std::log("[DIALER APP] Keypad digit entered.").ok();
                        }
                        return Tap::Redraw(Dirty::Display);
                    }
                }
            }
        }

        // Action Bar: SOS, Call, and Backspace discs
        if (ACTION_Y0..ACTION_Y1).contains(&y) {
            if in_circle(x, y, SOS_CX, SOS_CY, SOS_R) {
                let _ = dialer.dial_emergency();
                if dialer.outcome == CallOutcome::EmergencyRefused {
                    std::log("[DIALER APP] Emergency call refused by the telephony service; the outcome is shown under the number.",
                    )
                    .ok();
                } else {
                    std::log("[DIALER APP] Telephony service accepted the emergency call.",
                    )
                    .ok();
                }
                return Tap::Redraw(Dirty::All);
            }
            if in_call_pill(x, y) {
                return Tap::Redraw(toggle_call(dialer));
            }
            if dialer.active_len > 0 && in_circle(x, y, BACKSPACE_CX, BACKSPACE_CY, BACKSPACE_R) {
                dialer.backspace();
                return Tap::Redraw(Dirty::Display);
            }
        }
    }

    Tap::None
}

/// The settings surface's hit-testing.
///
/// WHY each row answers for itself: a row that changes nothing was a row that
/// looked like a control. The two rows the platform cannot answer say so when
/// they are pressed, so the user learns the limit from the app and not from a
/// guess.
fn handle_settings_tap(dialer: &mut DialerApp, x: usize, y: usize) -> Tap<'static> {
    if dialer.settings_view == SettingsView::RingtonePicker {
        return handle_ringtone_picker_tap(dialer, x, y);
    }
    if y < SETTINGS_ROW_Y0 {
        return Tap::None;
    }
    let index = (y - SETTINGS_ROW_Y0) / SETTINGS_ROW_H;
    if index >= SettingsRow::ORDER.len() {
        return Tap::None;
    }
    match SettingsRow::ORDER[index] {
        SettingsRow::Ringtone => {
            dialer.refresh_ringtone_choices();
            dialer.settings_view = SettingsView::RingtonePicker;
            Tap::Redraw(Dirty::All)
        }
        SettingsRow::KeypadTone => {
            match dialer.toggle_dtmf() {
                Ok(on) => {
                    std::log(if on {
                        "[DIALER APP] The keypad tone is on."
                    } else {
                        "[DIALER APP] The keypad tone is off."
                    })
                    .ok();
                }
                Err(code) => {
                    log_with_code(
                        "[DIALER APP] The settings store refused the keypad-tone preference, code",
                        code,
                    );
                }
            }
            dialer.refresh_settings_values();
            Tap::Redraw(Dirty::All)
        }
        SettingsRow::DefaultSim => {
            std::log("[DIALER APP] The platform exposes one SIM profile and no subscription-selection call, so a default SIM cannot be chosen here.")
                .ok();
            Tap::None
        }
        SettingsRow::CallDuration => {
            std::log("[DIALER APP] The platform's telephony telemetry carries no call duration, so the app shows none.")
                .ok();
            Tap::None
        }
    }
}

/// The ringtone picker's hit-testing.
fn handle_ringtone_picker_tap(dialer: &mut DialerApp, _x: usize, y: usize) -> Tap<'static> {
    // The row that leaves the picker.
    if y >= RINGTONE_BACK_Y0 && y < RINGTONE_BACK_Y0 + RINGTONE_BACK_H {
        dialer.settings_view = SettingsView::Main;
        dialer.refresh_settings_values();
        return Tap::Redraw(Dirty::All);
    }
    if y < RINGTONE_ROW_Y0 || y >= RINGTONE_VIEWPORT_Y1 {
        return Tap::None;
    }
    // The list scrolls, so the row under the tap is the one at the scrolled
    // position.
    let offset = dialer.ringtone_scroll.offset;
    let content_y = y + offset;
    if content_y < RINGTONE_ROW_Y0 {
        return Tap::None;
    }
    let index = (content_y - RINGTONE_ROW_Y0) / RINGTONE_ROW_H;
    if index >= dialer.ringtone_choice_count {
        return Tap::None;
    }
    let choice = dialer.ringtone_choices[index];
    let path = &choice.path[..choice.path_len];
    match dialer.set_ringtone(path) {
        Ok(()) => {
            dialer.play_ringtone();
            dialer.settings_view = SettingsView::Main;
            dialer.refresh_settings_values();
            std::log("[DIALER APP] The device's ringtone slot now names the chosen document.")
                .ok();
            Tap::Redraw(Dirty::All)
        }
        Err(code) => {
            log_with_code(
                "[DIALER APP] The settings store refused the ringtone write, code",
                code,
            );
            Tap::None
        }
    }
}

/// Contacts section hit-testing: the card's chips and editor fields, the contact
/// rows and their stars, and the action row.
///
/// WHY it is separate from `handle_tap`: the section has more controls than the
/// keypad and the recents ledger together, and folding them into `handle_tap`
/// would put one function two screens deep in branches.
fn handle_contacts_tap(dialer: &mut DialerApp, x: usize, y: usize) -> Tap<'static> {
    // While the editor is open, the tab band and the dock still work, so the
    // user is never trapped in a half-finished contact.
    if (TAB_BAND_Y0..TAB_BAND_Y0 + TAB_BAND_H).contains(&y)
        || ((DOCK_Y0..DOCK_Y1).contains(&y) && x >= 24 && x < 696)
    {
        return handle_tap(dialer, x as u32, y as u32);
    }

    let _ = std::audio_haptic_click();

    // --- Editor mode -------------------------------------------------------
    if let Some(ed) = dialer.contact_edit {
        let fy = match ed.field {
            ContactField::Name => CARD_Y0 + 6,
            ContactField::Number => CARD_Y0 + 108,
        };
        // Touch Clear (X) button at DISPLAY_X1 - 88
        if in_circle(x, y, DISPLAY_X1 - 88, fy + 48, 22) {
            let mut next = ed;
            match next.field {
                ContactField::Name => {
                    next.name = [0; CONTACT_NAME_MAX];
                    next.name_len = 0;
                }
                ContactField::Number => {
                    next.number = [0; CONTACT_NUM_MAX];
                    next.number_len = 0;
                }
            }
            dialer.contact_edit = Some(next);
            dialer.editor_last_cell = None;
            let _ = std::audio_haptic_click();
            return Tap::Redraw(Dirty::Fields);
        }
        // Touch Backspace (⌫) button at DISPLAY_X1 - 44
        if in_circle(x, y, DISPLAY_X1 - 44, fy + 48, 22) {
            let mut next = ed;
            next.backspace();
            dialer.contact_edit = Some(next);
            dialer.editor_last_cell = None;
            let _ = std::audio_haptic_click();
            return Tap::Redraw(Dirty::Fields);
        }

        // Field taps move the caret. WHY each field also picks the keypad face:
        // a name needs letters and a number needs digits, so focusing a field
        // brings up the cycle that can type into it.
        if (CARD_Y0 + 6..CARD_Y0 + 102).contains(&y)
            && x >= DISPLAY_X0
            && x < DISPLAY_X1
            && ed.field != ContactField::Name
        {
            let mut next = ed;
            next.field = ContactField::Name;
            dialer.contact_edit = Some(next);
            dialer.editor_last_cell = None;
            return Tap::Redraw(Dirty::All);
        }
        if (CARD_Y0 + 108..CARD_Y0 + 204).contains(&y)
            && x >= DISPLAY_X0
            && x < DISPLAY_X1
            && ed.field != ContactField::Number
        {
            let mut next = ed;
            next.field = ContactField::Number;
            dialer.contact_edit = Some(next);
            dialer.editor_last_cell = None;
            return Tap::Redraw(Dirty::All);
        }

        // The dial pad, typed as text.
        if (ROW_Y[0]..ROW_Y[3] + BTN_H).contains(&y) {
            for row in 0..4 {
                if y < ROW_Y[row] || y >= ROW_Y[row] + BTN_H {
                    continue;
                }
                for col in 0..3 {
                    let bx = COL_X[col];
                    if x < bx || x >= bx + BTN_W {
                        continue;
                    }
                    dialer.editor_press_cell(row, col);
                    dialer.editor_error = false;
                    return Tap::Redraw(Dirty::Fields);
                }
            }
        }

        // Action row: the two editing discs. The emergency disc lives on the
        // keypad only; it is not drawn here, so it is not hit-tested here.
        if (ACTION_Y0..ACTION_Y1).contains(&y) {
            if in_circle(x, y, CALL_CX, CALL_CY, CALL_R) {
                dialer.commit_edit_contact();
                return Tap::Redraw(Dirty::All);
            }
                if in_circle(x, y, CONTACTS_CX, CONTACTS_CY, CONTACTS_R) {
                    // WHY this disc is DELETE for an existing contact and CANCEL
                    // for a new one: deleting is a destructive act on something
                    // already stored, and cancelling is the only safe meaning for
                    // a record that exists only in the editor.
                    let deletes = matches!(ed.target, Some(_));
                    if deletes {
                        dialer.contact_sel = ed.target;
                        dialer.delete_selected_contact();
                        dialer.contact_edit = None;
                        dialer.editor_last_cell = None;
                    } else {
                        dialer.cancel_edit_contact();
                    }
                    return Tap::Redraw(Dirty::All);
                }
        }
        return Tap::None;
    }

    // --- Browse mode -------------------------------------------------------
    // Star toggle pill
    if dialer.contact_sel.is_some() && (186..222).contains(&y) && x >= DISPLAY_X1 - 130 && x < DISPLAY_X1 - 20 {
        dialer.toggle_selected_favourite();
        let _ = std::audio_haptic_click();
        return Tap::Redraw(Dirty::All);
    }

    // Card chips: CALL / MSG / EDIT / DELETE.
    let chip_gap = 12usize;
    let chip_w = (DISPLAY_W - 3 * chip_gap) / 4;
    let chip_y = 312;
    if (chip_y..chip_y + 56).contains(&y) && x >= DISPLAY_X0 && x < DISPLAY_X1 {
        for i in 0..4 {
            let cx0 = DISPLAY_X0 + i * (chip_w + chip_gap);
            if x < cx0 || x >= cx0 + chip_w {
                continue;
            }
            if dialer.contact_sel.is_none() {
                std::log("[DIALER APP] That action needs a selected contact; tap a row first.",
                )
                .ok();
                return Tap::Redraw(Dirty::None);
            }
            let sel = dialer.contact_sel.unwrap_or(0);
            let rec = if sel >= ContactEditor::UNSAVED_BASE {
                dialer.contacts.unsaved_record(sel - ContactEditor::UNSAVED_BASE)
            } else {
                dialer.contacts.record(sel)
            };
            match i {
                0 => dialer.dial_selected_contact(),
                1 => {
                    if let Some(r) = rec {
                        let _ = hand_off_to_messaging(r.number_bytes());
                    }
                }
                2 => dialer.start_edit_contact(sel),
                _ => dialer.delete_selected_contact(),
            }
            return Tap::Redraw(Dirty::All);
        }
    }

    // Action row: ADD / CALL and FAVS / EDIT. The emergency disc is keypad-only.
    if (ACTION_Y0..ACTION_Y1).contains(&y) {
        if in_circle(x, y, CALL_CX, CALL_CY, CALL_R) {
            if dialer.contact_sel.is_some() {
                dialer.dial_selected_contact();
            } else {
                dialer.start_add_contact();
            }
            return Tap::Redraw(Dirty::All);
        }
        if in_circle(x, y, CONTACTS_CX, CONTACTS_CY, CONTACTS_R) {
            if dialer.contact_sel.is_some() {
                dialer.start_edit_contact(dialer.contact_sel.unwrap_or(0));
            } else {
                dialer.contacts_fav_only = !dialer.contacts_fav_only;
                // WHY the scrolls are recomputed here: the filter changes how
                // many rows the list has, and the extent from the other list
                // would leave the new list scrolled past its own end.
                dialer.refresh_scrolls();
                std::log(if dialer.contacts_fav_only {
                        "[DIALER APP] Showing starred contacts only."
                    } else {
                        "[DIALER APP] Showing all contacts."
                    },
                )
                .ok();
            }
            return Tap::Redraw(Dirty::All);
        }
    }

    // List rows: the star toggles, the body selects.
    if (LIST_Y0..NOTICE_Y0).contains(&y) && x >= DISPLAY_X0 && x < DISPLAY_X1 {
        let total = dialer.contacts.len() + dialer.contacts.unsaved_len();
        // The list scrolls, so the row under the tap is the one at the scrolled
        // position, and the star moves with it.
        let offset = dialer.contacts_scroll.offset;
        let mut row = 0usize;
        let mut cursor = LIST_Y0;
        while row < total {
            let sel = if row < dialer.contacts.len() {
                row
            } else {
                ContactEditor::UNSAVED_BASE + (row - dialer.contacts.len())
            };
            let rec = if sel >= ContactEditor::UNSAVED_BASE {
                dialer
                    .contacts
                    .unsaved_record(sel - ContactEditor::UNSAVED_BASE)
            } else {
                dialer.contacts.record(sel)
            };
            let Some(rec) = rec else {
                row += 1;
                continue;
            };
            if dialer.contacts_fav_only && !rec.is_favourite() {
                row += 1;
                continue;
            }
            let scrolled_y = cursor.saturating_sub(offset);
            if y < scrolled_y || y >= scrolled_y + CONTACT_ROW_H {
                cursor += CONTACT_ROW_H;
                row += 1;
                continue;
            }
            if in_circle(x, y, STAR_CX, scrolled_y + CONTACT_ROW_H / 2, STAR_R) {
                dialer.contact_sel = Some(sel);
                dialer.toggle_selected_favourite();
                dialer.refresh_scrolls();
                return Tap::Redraw(Dirty::All);
            }
            // WHY the row stays selected and the action changes rather than a
            // tap both selecting and acting: one tap, one meaning, and the card
            // shows what the next tap on CALL will do.
            dialer.contact_sel = Some(sel);
            return Tap::Redraw(Dirty::All);
        }
    }

    Tap::None
}

/// Mirrors `handle_browser_key` in `apps/browser/src/main.rs`: printable ASCII,
/// backspace, escape, enter and tab routed through the dialer's own state.
fn handle_dialer_key(dialer: &mut DialerApp, key: std::InputKeyEvent) -> Tap<'static> {
    use std::input_key_code;

    // The contact editor is a text field, so when it is open a physical keyboard
    // types into it before any of the dialer's own verbs are considered. This is
    // the same precedence the browser uses for its address field.
    if let Some(ed) = dialer.contact_edit {
        if !key.pressed {
            return Tap::None;
        }
        let mut next = ed;
        match key.code {
            input_key_code::ESCAPE => {
                dialer.cancel_edit_contact();
                return Tap::Redraw(Dirty::All);
            }
            input_key_code::BACKSPACE => {
                next.backspace();
                dialer.contact_edit = Some(next);
                dialer.editor_last_cell = None;
                return Tap::Redraw(Dirty::Fields);
            }
            input_key_code::ENTER => {
                dialer.contact_edit = Some(next);
                dialer.commit_edit_contact();
                return Tap::Redraw(Dirty::All);
            }
            input_key_code::TAB => {
                next.field = match next.field {
                    ContactField::Name => ContactField::Number,
                    ContactField::Number => ContactField::Name,
                };
                dialer.contact_edit = Some(next);
                dialer.editor_last_cell = None;
                return Tap::Redraw(Dirty::All);
            }
            // Printable ASCII only, and only inside the field's own width, so a
            // name can never overflow into the number field's bytes.
            code if (32..=126).contains(&code) => {
                if next.active_len() < next.active_cap() {
                    next.push(code as u8);
                    dialer.contact_edit = Some(next);
                    dialer.editor_last_cell = None;
                    return Tap::Redraw(Dirty::Fields);
                }
                return Tap::None;
            }
            _ => return Tap::None,
        }
    }

    if !key.pressed {
        return Tap::None;
    }
    match key.code {
        input_key_code::BACKSPACE => {
            dialer.backspace();
            Tap::Redraw(Dirty::Display)
        }
        input_key_code::ESCAPE => {
            dialer.clear_entry();
            Tap::Redraw(Dirty::Display)
        }
        input_key_code::ENTER => Tap::Redraw(toggle_call(dialer)),
        input_key_code::TAB => {
            dialer.step_section();
            std::log("[DIALER APP] Section switched by keyboard.").ok();
            Tap::Redraw(Dirty::All)
        }
        // Dial pad keys only. The keypad's own sublabel letters (ABC, DEF, ...)
        // are not part of the number entry, so accepting them would let the
        // display hold something no telephony service could be asked to dial.
        // `key.code` is a u32, so the literals are widened to match rather than
        // being cast down.
        code @ (48u32..=57u32) | code @ (42u32 | 35u32) => {
            dialer.input_digit(code as u8 as char);
            Tap::Redraw(Dirty::Display)
        }
        _ => Tap::None,
    }
}

#[no_mangle]
pub extern "C" fn _start() -> ! {
    std::log("[DIALER APP (Ring 3)] Sovereign Dialer & Telephony online.",
    )
    .ok();

    let mut dialer = DialerApp::new();
    refresh_clock(&mut dialer);

    // WHY the ringtone is read here: the ringtone slot is a device-wide
    // choice that the file manager and the gallery fill, and this app plays
    // whatever is in it.
    dialer.refresh_ringtone();

    // WHY the keypad-tone preference is read here: it is the user's choice,
    // stored in the settings store, and the keypad has to honour it from the
    // first key press.
    dialer.dtmf_enabled = dtmf_enabled();

    // 1. Query Cellular Modem Baseband Telemetry
    // WHY: this app holds no telephony authority, so the query is normally
    // refused. The previous code logged nothing on refusal, leaving an unlogged
    // absence. Report the real outcome and fail closed on modem presence.
    let mut modem_buf = [0u8; 128];
    match std::telephony_query(&mut modem_buf) {
        Ok(len) if len > 0 => {
            let payload = &modem_buf[..len];
            if json_present_true(payload) {
                dialer.modem_present = true;
                std::log("[DIALER APP] Baseband Modem: Hardware detected and online.",
                )
                .ok();
            } else {
                // WHY: the payload simply did not list a present modem.
                // "Offline" would assert a baseband state the HAL never
                // reported, so state only what the report actually said.
                std::log("[DIALER APP] Baseband Modem: the telemetry report did not list a present modem.",
                )
                .ok();
            }
            // WHY parse the real field: the signal meter used to fill all four
            // bars from the `present` boolean, which is a full-strength reading
            // for a radio that may be idle. `"bars"` is what the HAL measured.
            match json_u8(payload, "\"bars\"") {
                Some(bars) => {
                    dialer.signal_bars = if bars > 4 { 4 } else { bars };
                }
                None => {
                    dialer.signal_bars = 0;
                    std::log("[DIALER APP] Modem telemetry carried no signal bar count; the meter shows no bars.",
                    )
                    .ok();
                }
            }
        }
        Ok(_) => {
            std::log("[DIALER APP] Telephony query returned an empty payload; no baseband reported.",
            )
            .ok();
        }
        Err(_) => {
            std::log("[DIALER APP] Telephony query refused by the kernel; no cellular baseband is available to this app.",
            )
            .ok();
        }
    }

    // 1b. Battery fuel gauge. WHY query it at all: the header used to draw an
    // empty battery shell on every launch, which reads as "0%". Either the real
    // state of charge is drawn or nothing is drawn there.
    let mut battery_buf = [0u8; 256];
    match std::battery_query(&mut battery_buf) {
        Ok(len) if len > 0 => {
            let payload = &battery_buf[..len];
            if json_present_true(payload) {
                match json_u8(payload, "\"soc\"") {
                    Some(soc) => {
                        dialer.battery_soc = Some(if soc > 100 { 100 } else { soc });
                    }
                    None => {
                        dialer.battery_soc = None;
                        std::log("[DIALER APP] Fuel gauge reported no state of charge; the battery indicator is left blank.",
                        )
                        .ok();
                    }
                }
            } else {
                std::log("[DIALER APP] No battery fuel gauge is present; the battery indicator is left blank.",
                )
                .ok();
            }
        }
        Ok(_) => {
            std::log("[DIALER APP] Battery query returned an empty payload; the battery indicator is left blank.",
            )
            .ok();
        }
        Err(_) => {
            std::log("[DIALER APP] Battery query refused by the kernel; the battery indicator is left blank.",
            )
            .ok();
        }
    }

    // Emergency contacts are provided by the real contacts/modem subsystem when available.

    // 2b. Contact store. WHY it is loaded before the surface exists: the first
    // frame has to show the contacts that are really stored, and a store read
    // after the first paint would show an empty list that then fills in.
    //
    // WHY the read result is reported rather than assumed: the dialer is the only
    // place that knows whether the settings store answered, refused, or held
    // bytes this app did not write, and the notice band under the list says the
    // same thing on screen.
    dialer.contacts = ContactStore::load();
    match dialer.contacts.state {
        ContactStoreState::Loaded => { std::log("[DIALER APP] Contacts read back out of the settings store.",
        )
        .ok();
        }
        ContactStoreState::Empty => { std::log("[DIALER APP] No saved contacts: the settings store holds no dialer contacts key.",
        )
        .ok();
        }
        ContactStoreState::ReadRefused => { std::log("[DIALER APP] Contacts read REFUSED by the kernel: no stored contacts were loaded, and none will be shown as saved.",
        )
        .ok();
        }
        ContactStoreState::Invalid => { std::log("[DIALER APP] Stored contact bytes failed validation; no contacts were loaded from them.",
        )
        .ok();
        }
        ContactStoreState::Untouched | ContactStoreState::Written
        | ContactStoreState::WriteRefused | ContactStoreState::Full => {
            std::log("[DIALER APP] Contacts store reported a state it cannot be in before any write; no contacts are shown as saved.",
            )
            .ok();
        }
    }

    // WHY the scroll extents and the settings values are computed before the
    // first frame: the first frame the user sees has to show the real state of
    // the lists and the real value of every setting, not a default that is
    // corrected a moment later.
    dialer.refresh_scrolls();
    dialer.refresh_settings_values();
    dialer.take_handoff_number();

    // 3. Allocate dedicated KMS Client Surface (z=200)
    std::log("[DIALER APP] Step 1: Allocating application surface (z=200)...",
    )
    .ok();
    // WHY the slot is resolved by type: a capability slot is a position in
    // this app's table, and the position depends on the order the kernel
    // inserted the capabilities in. The literal 6 happens to hold the
    // framebuffer in this app's table today.
    let surface = match std::surface_create(W as u32, H as u32, 200) {
        Ok(s) => {
            std::log("[DIALER APP] Step 2: Surface created successfully.").ok();
            s
        }
        Err(_) => {
            std::log("[DIALER APP] Display surface unavailable; refusing headless call simulation.",
            )
            .ok();
            std::exit_with_code(1);
        }
    };

    // 4. Render an empty dialer; numbers are entered by the user.
    render_dialer_ui(surface, &dialer);
    let _ = std::surface_flip();
    std::log("[DIALER APP] Step 3: Initial frame presented to hardware scanout.",
    )
    .ok();

    // 5. Calls are initiated only from user input; no automatic call is placed at startup.

    // 6. Interactive Event Reactor Loop
    loop {
        if let Ok(events) = std::event_poll(
            std::EVENT_TOUCH_INPUT | std::EVENT_TIMER_TICK | std::EVENT_MODEM_URC | std::EVENT_IPC_MSG,
            30,
        ) {
            if (events & (std::EVENT_TIMER_TICK | std::EVENT_IPC_MSG)) != 0 {
                let prev_len = dialer.active_len;
                dialer.take_handoff_number();
                if dialer.active_len != prev_len {
                    render_dialer_band(surface, &dialer, DISPLAY_Y0, DISPLAY_Y1);
                    render_dialer_band(surface, &dialer, ACTION_Y0, ACTION_Y1);
                    let _ = std::surface_flip();
                }
                let mut sim_caller = [0u8; 32];
                if let Ok(n) = std::settings_get("dialer.simulate_incoming", &mut sim_caller) {
                    if n > 0 && n <= sim_caller.len() {
                        let _ = std::settings_set("dialer.simulate_incoming", b"");
                        dialer.simulate_incoming_call(&sim_caller[..n]);
                        render_dialer_ui(surface, &dialer);
                        let _ = std::surface_flip();
                    }
                }
                let mut sim_notif = [0u8; 8];
                if let Ok(n) = std::settings_get("dialer.simulate_notification", &mut sim_notif) {
                    if n > 0 {
                        let _ = std::settings_set("dialer.simulate_notification", b"");
                        dialer.play_notification();
                    }
                }
                // WHY there is no work here any more: the tick used to advance
                // a call-duration counter the app kept itself. The platform's
                // telephony telemetry carries no duration, so the counter was
                // the app's own ticks rather than a fact about the call, and it
                // is gone. Nothing on the surface is derived from a timer, so
                // re-presenting an unchanged frame would cost a full scanout
                // copy for no visible difference.
            }

            if (events & std::EVENT_MODEM_URC) != 0 {
                // WHY: the previous code logged an inbound tower alert and called
                // it a call for any MODEM_URC bit. The URC vector was never read,
                // no call setup message was decoded, and no baseband is attached,
                // so no inbound call occurred. Report the event only.
                if dialer.modem_present {
                    std::log("[DIALER APP] Baseband URC signalled; no call setup message was decoded from it.",
                    )
                    .ok();
                } else {
                    std::log("[DIALER APP] Modem URC event raised with no attached baseband; no call was received.",
                    )
                    .ok();
                }
            }

            if (events & std::EVENT_TOUCH_INPUT) != 0 {
                let mut input = std::InputEvent {
                    kind: 0,
                    x: 0,
                    y: 0,
                    dx: 0,
                    dy: 0,
                    scale_milli: 1000,
                };
                let mut dirty = Dirty::None;
                while std::input_event_poll(&mut input).is_ok() {
                    // Keyboard first: the normalized pipeline multiplexes keys
                    // and touches through the same poll, so a key must be
                    // recognised before the touch branch sees kind != 1.
                    if let Some(key) = input.key_event() {
                        match handle_dialer_key(&mut dialer, key) {
                            Tap::Leave(msg) => {
                                std::log(msg).ok();
                                let _ = std::surface_destroy(surface);
                                std::exit_with_code(0);
                            }
                            Tap::Redraw(d) => dirty = merge(dirty, d),
                            Tap::None => {}
                        }
                        continue;
                    }
                    if input.kind == 4 || input.kind == 10 {
                        // A drag over a list scrolls it; a drag anywhere else
                        // is the system's navigation gesture.
                        if dialer.apply_scroll(input.x as usize, input.y as usize, input.dy) {
                            dirty = merge(dirty, Dirty::All);
                            continue;
                        }
                    }
                    if input.kind == 4 || input.kind == 6 || input.kind == 7 || input.kind == 8 || input.kind == 10 {
                        std::log("[DIALER APP] Navigation gesture recognized. Returning to launcher.",
                        )
                        .ok();
                        let _ = std::surface_destroy(surface);
                        std::exit_with_code(0);
                    }
                    // WHY kind 2 is answered as a tap too: the gesture router
                    // labels the second of two quick taps at the same point a
                    // DoubleTap, and drops it from the Tap stream. The keypad
                    // and the backspace disc have no double-tap action, so
                    // ignoring it silently lost a digit: typing three digits
                    // and tapping backspace three times deleted only two.
                    if input.kind == 1 || input.kind == 2 {
                        match handle_tap(&mut dialer, input.x, input.y) {
                            Tap::Leave(msg) => {
                                std::log(msg).ok();
                                let _ = std::surface_destroy(surface);
                                std::exit_with_code(0);
                            }
                            Tap::Redraw(d) => dirty = merge(dirty, d),
                            Tap::None => {}
                        }
                    }
                }
                if dirty != Dirty::None {
                    match dirty {
                        Dirty::None => {}
                        Dirty::Display => {
                            render_dialer_band(surface, &dialer, DISPLAY_Y0, DISPLAY_Y1);
                            render_dialer_band(surface, &dialer, ACTION_Y0, ACTION_Y1);
                        }
                        // The editor's fields live in the display-card band, so
                        // typing into them repaints exactly that band.
                        Dirty::Fields => render_dialer_band(surface, &dialer, DISPLAY_Y0, DISPLAY_Y1),
                        Dirty::All => render_dialer_ui(surface, &dialer),
                    }
                    let _ = std::surface_flip();
                }
            }
        }

        std::yield_now();
    }
}

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    std::log("[DIALER APP] Panic detected! Terminating task...").ok();
    std::exit_with_code(1);
}
