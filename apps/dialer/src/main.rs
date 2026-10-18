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
//! - The segmented tabs and the bottom dock are both live. Dial, Recents and
//!   Contacts are three real sections; the dock's Favorites slot opens the
//!   contacts list filtered to the contacts the user starred.
//! - Recents is backed by a real ledger of every dial attempt this session made
//!   and the outcome the telephony service actually returned for it.
//! - Contacts is backed by a real, user-created store (`ContactStore`): browse,
//!   add, edit, delete, star, and dial by name or by an entered number.
//! - The signal bars are drawn from the real `"bars"` field of the modem
//!   telemetry, not from a boolean. The battery is drawn only when
//!   `battery_query` actually returned a state-of-charge; otherwise nothing is
//!   drawn there, as the browser does for telemetry it does not receive.
//! - The call control is drawn recessed while the number field is empty, because
//!   there is nothing it could dial.
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

mod outline_text;

use core::panic::PanicInfo;
use interos_std as std;
use interos_std::design::color;

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
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DialerSection {
    /// The 3x4 dial pad and the call controls.
    Keypad = 0,
    /// The real ledger of dial attempts made in this session.
    Recents = 1,
    /// The user's contacts: browse, add, edit, delete, star and dial.
    Contacts = 2,
}

/// Real outcome of the last call attempt, drawn under the dialed number.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CallOutcome {
    /// No attempt yet.
    Idle,
    /// A dial request is with the telephony service.
    Dialing,
    /// The telephony service accepted the call.
    Active,
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
            CallOutcome::Active => b"Call active",
            CallOutcome::Refused => b"Call refused by the telephony service",
            CallOutcome::EmergencyRefused => b"Emergency call refused by the telephony service",
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
const STORE_CAP_READ: usize = 3;
const STORE_CAP_WRITE: usize = 7;

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
        match std::settings_get(STORE_CAP_READ, STORE_KEY, &mut encoded) {
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
        match std::settings_set(STORE_CAP_WRITE, STORE_KEY, &encoded[..encoded_len]) {
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
        if name.is_empty() || number.is_empty() || self.next_id == 0 || self.next_id as usize > MAX_CONTACTS
        {
            return Err(-22);
        }
        let id = self.next_id;
        let mut rec = ContactRecord::from_parts(id, name, number, false);
        if !rec.valid() {
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
        // WHY the record is appended before the write: `persist` encodes the
        // durable list, so the new contact has to be in it for the write to mean
        // "this list, plus this contact, is now what storage holds".
        self.records[self.len as usize] = rec;
        self.len += 1;
        self.next_id += 1;
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
const RECENTS_CAP: usize = 5;

/// Production Telephony Dialer state manager.
pub struct DialerApp {
    pub state: CallState,
    pub active_number: [u8; 32],
    pub active_len: usize,
    pub is_muted: bool,
    pub is_speaker: bool,
    pub call_duration_secs: u32,
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
    /// Real ledger of dial attempts, newest first.
    pub recents: [CallRecord; RECENTS_CAP],
    pub recents_len: usize,
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
    /// True when the Contacts list shows only starred contacts. Set by the dock's
    /// Favorites slot, so that slot opens something real rather than reporting
    /// that no store exists.
    pub contacts_fav_only: bool,
}

impl DialerApp {
    pub const fn new() -> Self {
        Self {
            state: CallState::Idle,
            active_number: [0u8; 32],
            active_len: 0,
            is_muted: false,
            is_speaker: false,
            call_duration_secs: 0,
            calls_completed: 0,
            // WHY: fail closed. Until the telephony service reports a baseband,
            // no inbound-call path may be claimed.
            modem_present: false,
            section: DialerSection::Keypad,
            outcome: CallOutcome::Idle,
            signal_bars: 0,
            battery_soc: None,
            recents: [CallRecord {
                number: [0; 32],
                len: 0,
                accepted: false,
                emergency: false,
            }; RECENTS_CAP],
            recents_len: 0,
            // WHY the store is empty here and filled by `load()` in `_start`:
            // `new()` is a `const fn` so it can be called before the scheduler and
            // the storage service exist, and a syscall from it would fault. The
            // load must happen once the app is running, which `_start` does.
            contacts: ContactStore::EMPTY,
            contact_sel: None,
            contact_edit: None,
            editor_last_cell: None,
            contacts_fav_only: false,
        }
    }

    /// Plays dual-tone multi-frequency acoustic feedback for a pressed keypad digit.
    pub fn play_dtmf(&self, digit: char) {
        if let Some(pair) = get_dtmf_frequencies(digit) {
            // WHY only the high tone: the audio service exposes a single
            // oscillator (`audio_play_tone`), so only one of the two CCITT
            // frequencies can ever be emitted. The low tone is reported by
            // `get_dtmf_frequencies` and drawn in the key sublabel, never
            // claimed to have sounded.
            std::audio_play_tone(pair.high_freq).ok();
            std::audio_haptic_click().ok();
            std::audio_silence().ok();
        }
    }

    /// Appends a dialed digit to the accumulator and plays its DTMF tone.
    pub fn input_digit(&mut self, digit: char) {
        if self.active_len < self.active_number.len() {
            self.active_number[self.active_len] = digit as u8;
            self.active_len += 1;
            self.play_dtmf(digit);
        }
    }

    /// Removes the last entered digit.
    pub fn backspace(&mut self) {
        if self.active_len > 0 {
            self.active_len -= 1;
            std::audio_haptic_click().ok();
        }
    }

    /// Clears the entry without placing a call.
    pub fn clear_entry(&mut self) {
        if self.active_len > 0 {
            self.active_len = 0;
            let _ = std::audio_haptic_click();
        }
    }

    /// Appends one real dial attempt to the recents ledger, newest first.
    fn record_call(&mut self, number: &str, accepted: bool, emergency: bool) {
        let n = number.len().min(self.active_number.len());
        let mut entry = CallRecord {
            number: [0u8; 32],
            len: n,
            accepted,
            emergency,
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
        self.call_duration_secs = 0;

        // A call is connected only after the authoritative telephony service
        // accepts the request. UI/ACR dispatch must never manufacture an
        // ActiveCall state.
        match std::telephony_dial(number) {
            Ok(()) => {
                self.state = CallState::ActiveCall;
                self.outcome = CallOutcome::Active;
                std::audio_play_tone(440).ok();
                std::audio_silence().ok();
                std::log(0, "[DIALER APP] Telephony service accepted call request.").ok();
                self.record_call(number, true, emergency);
                Ok(())
            }
            Err(err) => {
                self.state = CallState::CallEnded;
                self.outcome = if emergency {
                    CallOutcome::EmergencyRefused
                } else {
                    CallOutcome::Refused
                };
                std::log(0, "[DIALER APP] Call request rejected by telephony service.").ok();
                self.record_call(number, false, emergency);
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
                std::log(
                    0,
                    "[DIALER APP] Call Terminated: telephony service released the channel.",
                )
                .ok();
            } else {
                std::log(
                    0,
                    "[DIALER APP] Call Terminated locally: the telephony service refused the hangup, so no channel was released.",
                )
                .ok();
            }
        } else {
            // WHY: no call was ever established, so there is no channel to
            // release and no completed call to count. Reporting a released
            // channel here would be a success claim for a rejected operation.
            std::log(
                0,
                "[DIALER APP] Hangup with no connected call; no channel was released.",
            )
            .ok();
        }
    }

    /// Steps the section. All three tabs are live now.
    pub fn step_section(&mut self) -> DialerSection {
        self.section = match self.section {
            DialerSection::Keypad => DialerSection::Recents,
            DialerSection::Recents => DialerSection::Contacts,
            DialerSection::Contacts => DialerSection::Keypad,
        };
        self.section
    }

    /// Opens the Contacts section, optionally filtered to starred contacts.
    pub fn open_contacts(&mut self, favourites_only: bool) {
        self.section = DialerSection::Contacts;
        self.contacts_fav_only = favourites_only;
        self.contact_edit = None;
        self.contact_sel = None;
        self.editor_last_cell = None;
    }

    /// Opens the editor for a new contact.
    pub fn start_add_contact(&mut self) {
        self.contact_edit = Some(ContactEditor::open_new());
        self.editor_last_cell = None;
    }

    /// Opens the editor on an existing contact.
    ///
    /// WHY the index is carried, not the record: an edit must write back to the
    /// row the user tapped, and an index is the only thing that still identifies
    /// that row after a delete moves the others.
    pub fn start_edit_contact(&mut self, index: usize) {
        if index >= ContactEditor::UNSAVED_BASE {
            let inner = index - ContactEditor::UNSAVED_BASE;
            match self.contacts.unsaved_record(inner) {
                Some(rec) => {
                    self.contact_edit =
                        Some(ContactEditor::open_record(rec, Some(index)));
                    self.editor_last_cell = None;
                }
                None => {
                    std::log(
                        0,
                        "[DIALER APP] Edit requested for a contact row that no longer exists; the editor was not opened.",
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
                std::log(
                    0,
                    "[DIALER APP] Edit requested for a contact row that no longer exists; the editor was not opened.",
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
            std::log(
                0,
                "[DIALER APP] Contact not saved: a contact needs both a name and a number.",
            )
            .ok();
            return;
        }
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
                std::log(
                    0,
                    "[DIALER APP] Contact written to the settings store and acknowledged.",
                )
                .ok();
            }
            Err(_) => {
                std::log(
                    0,
                    "[DIALER APP] Contact was NOT saved: the kernel refused the settings write. It is held in the unsaved group for this session only.",
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
                    std::log(
                        0,
                        "[DIALER APP] Unsaved contact dropped. It was never written to storage, so nothing had to be removed from it.",
                    )
                    .ok();
                    self.contact_sel = None;
                }
                Err(_) => {
                    std::log(
                        0,
                        "[DIALER APP] The contact row tapped no longer exists; nothing was deleted.",
                    )
                    .ok();
                }
            }
            return;
        }
        match self.contacts.remove(sel) {
            Ok(()) => {
                std::log(
                    0,
                    "[DIALER APP] Contact removed and the store write was acknowledged.",
                )
                .ok();
                self.contact_sel = None;
            }
            Err(_) => {
                std::log(
                    0,
                    "[DIALER APP] NOT deleted from storage: the kernel refused the store write, so the stored copy is unchanged.",
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
            std::log(
                0,
                "[DIALER APP] A contact that was never saved cannot be starred in storage; the star stays with the saved copy.",
            )
            .ok();
            return;
        }
        match self.contacts.toggle_favourite(sel) {
            Ok(()) => {
                std::log(0, "[DIALER APP] Favourite flag written and acknowledged.").ok();
            }
            Err(_) => {
                std::log(
                    0,
                    "[DIALER APP] Favourite flag changed for this session only: the kernel refused the store write.",
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
        std::log(
            0,
            "[DIALER APP] Contact number placed in the entry; press the call control to dial it.",
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
            std::log(
                0,
                "[DIALER APP] The telephony service refused this call; the outcome is shown under the number.",
            )
            .ok();
        } else {
            std::log(0, "[DIALER APP] The telephony service accepted the call request.").ok();
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
// 32-bit ARGB8888 Design Color Tokens (Screen 9 Modern Light Theme)
// -----------------------------------------------------------------------------
const BG_DARK: u32 = 0xFFF7F6F2;
const SURFACE_DARK: u32 = 0xFFFFFFFF;
const SURFACE_RAISED: u32 = 0xFFEFEEE9;
const BORDER_SUBTLE: u32 = 0xFFE1E0DB;
const TEXT_PRIMARY: u32 = 0xFF111214;
const TEXT_MUTED: u32 = 0xFF777773;
#[allow(dead_code)]
const DISABLED_FILL: u32 = 0xFFE6E5E0;
#[allow(dead_code)]
const DISABLED_INK: u32 = 0xFFA9A8A3;
const ACCENT_SKY: u32 = 0xFF2563EB;
const SUCCESS_EMERALD: u32 = 0xFF2E8B57;
const DANGER_CRIMSON: u32 = 0xFFDC2626;
#[allow(dead_code)]
const WARNING_AMBER: u32 = 0xFFF59E0B;

// -----------------------------------------------------------------------------
// Canonical InterOS Typography
// -----------------------------------------------------------------------------

/// Anti-aliased run draw, clipped to the run's own line box.
///
/// `bg` is the local fill the text actually sits on â€” a tab capsule, the SOS
/// disc, the keypad disc, the dock plate â€” not the page background. Blending
/// every glyph edge toward one hardcoded colour tinted the AA fringe of any run
/// drawn on a darker local fill.
#[inline(always)]
fn test_text(
    x: usize,
    y: usize,
    x0: usize,
    y0: usize,
    scale: usize,
    text: &[u8],
    color: u32,
    bg: u32,
) -> Option<u32> {
    if scale == 0 || text.is_empty() {
        return None;
    }
    // A run owns exactly `run_rows(scale)` rows; testing only that box keeps a
    // call site from slicing the glyphs with a shorter hand-written window.
    if x < x0 || y < y0 || y >= y0 + run_rows(scale) {
        return None;
    }
    // The outline face, chosen ONCE for the run rather than once per pixel.
    //
    // WHY the order matters here: this funnel is called for every pixel of every
    // label on the surface, so a per-pixel choice would draw the glyph from both
    // faces at once -- an outline edge with a hard bitmap ghost one pixel off,
    // on every string. The launcher's first version of this change did exactly
    // that, and no test caught it; only looking at the capture did.
    if let Some(run) = outline_text::OutlineRun::place_run(
        x0,
        y0 + std::design::font::BASELINE * scale,
        scale,
        text,
        false,
    ) {
        // WHY the window is the run's and not the cell's: at the em that matches
        // this scale the outline's ascent reaches 18px above the cell origin and
        // its descent 4px past the bottom, so the cell-shaped test above would
        // clip the top of tall glyphs with nothing looking wrong. The run's own
        // bounds admit exactly its ink, so widening to them cannot put a pixel
        // where a glyph is not.
        let (rx, rtop, rw, rh) = run.bounds();
        if x >= rx && x - rx < rw && y >= rtop && y - rtop < rh {
            return run.pixel(x, y, color, bg);
        }
        // Inside the cell window but outside the run's ink: the background shows
        // through, exactly as an uninked bitmap pixel would.
        return None;
    }
    std::design::font::test_text_aa(x, y, x0, y0, scale, text, color, bg, false)
}

/// Rows a `scale`-high text run occupies on the shared bitmap face.
/// WHY a local `const fn`: this is exactly `font::line_height(scale)`, but that
/// helper is not `const`, and every run box below is derived from a
/// compile-time constant.
const fn run_rows(scale: usize) -> usize {
    scale * std::design::font::GLYPH_H
}

/// Header band height: y >= HEADER_H draws the hairline rule, so a header run
/// has to fit inside 0..HEADER_H.
const HEADER_H: usize = 94;
const TITLE_SCALE: usize = 3;

// Segmented navigation tabs (y: 104..156). Symmetrically balanced 200px pills
// across the 720px viewport (48px margins, 12px gaps).
const TAB_SCALE: usize = 2;
const TAB_BAND_Y0: usize = 104;
const TAB_BAND_H: usize = 52;
const TAB_W: usize = 200;
const TAB1_X0: usize = 48;
const TAB1_X1: usize = TAB1_X0 + TAB_W; // 248
const TAB2_X0: usize = 260;
const TAB2_X1: usize = TAB2_X0 + TAB_W; // 460
const TAB3_X0: usize = 472;
const TAB3_X1: usize = TAB3_X0 + TAB_W; // 672
const TAB_ACTIVE_FILL: u32 = 0xFF111214;
const TAB_LABELS: [&[u8]; 3] = [b"Dial", b"Recents", b"Contacts"];

// Dialed-number display card.
const DISPLAY_X0: usize = 24;
const DISPLAY_X1: usize = 696;
const DISPLAY_Y0: usize = 170;
const DISPLAY_Y1: usize = 380;
const DISPLAY_W: usize = DISPLAY_X1 - DISPLAY_X0;
const PLACEHOLDER_SCALE: usize = 2;
const PLACEHOLDER_Y0: usize = 250;
/// Real call-outcome caption under the dialed number. A scale-1 run owns
/// run_rows(1) = 24 rows, so 340..364 clears the 96-row scale-4 digit box at
/// 236..332 and stays inside the card's 380-row bottom edge.
const OUTCOME_Y0: usize = 340;

// Backspace control in the display card. Hit-tested against exactly the disc
// that is drawn (the old hit box was a 75x70 rectangle around a 48 px circle).
const BACKSPACE_CX: usize = 627;
const BACKSPACE_CY: usize = 260;
const BACKSPACE_R: i32 = 24;

// Keypad layout metrics: scale 2 digit, scale 1 sublabel, 48px radius circular discs.
const KEY_DIGIT_SCALE: usize = 2;
const KEY_SUB_SCALE: usize = 1;
const KEY_RADIUS: usize = 48;

// SOS emergency disc.
const SOS_CX: usize = 134;
const SOS_CY: usize = 1065;
const SOS_R: i32 = 40;
const SOS_SCALE: usize = 2;
const SOS_FILL: u32 = 0xFF2A1518;

// Central call control.
const CALL_CX: usize = 360;
const CALL_CY: usize = 1065;
const CALL_R: i32 = 52;

// Contacts shortcut control. The platform exposes no contact store, so this is
// drawn recessed and says so on tap.
const CONTACTS_CX: usize = 586;
const CONTACTS_CY: usize = 1065;
const CONTACTS_R: i32 = 40;

// Action bar band (the three discs above live in it).
const ACTION_Y0: usize = 990;
const ACTION_Y1: usize = 1140;

// Recents ledger geometry. WHY it starts at 424 rather than 410: the first row
// needs room for a scale-2 run (48 rows) plus a scale-1 run (24 rows) inside an
// 96 px row, and 410 would push the last of five rows past the action bar.
const RECENTS_Y0: usize = 424;
const RECENTS_ROW_H: usize = 96;
const CONTENT_Y0: usize = 410;
const CONTENT_Y1: usize = 1140;

// Bottom navigation dock.
const DOCK_Y0: usize = 1260;
const DOCK_Y1: usize = 1380;
const DOCK_ITEM_W: usize = 140;
/// WHY these x positions: they are the four 140 px slots the previous renderer
/// hard-coded its labels against, kept so the dock does not move.
const DOCK_ITEM_X0: [usize; 4] = [48, 208, 368, 528];
const DOCK_LABELS: [&[u8]; 4] = [b"Favorites", b"Recents", b"Keypad", b"Contacts"];
const DOCK_LABEL_Y0: usize = 1340;
const DOCK_LABEL_SCALE: usize = 1;
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
/// Contact list band. 8 rows of 66px = 528px, which is exactly the room the
/// store's capacity and the surface both allow.
const LIST_Y0: usize = 410;
const CONTACT_ROW_H: usize = 66;
/// Footer band: the honest capacity/durability notice.
const NOTICE_Y0: usize = 938;
const NOTICE_Y1: usize = 990;
/// Star hit circle on the right edge of every contact row.
const STAR_CX: usize = 660;
const STAR_R: i32 = 20;
/// Action discs in the contacts action row.
const C_ACTION_Y: usize = 1065;

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
        (3, 1) => b"+",
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
fn header_pixel(x: usize, y: usize, dialer: &DialerApp) -> u32 {
    if y >= HEADER_H {
        return BORDER_SUBTLE;
    }

    // Circular back button (cx: 62, cy: 48, radius: 22)
    let back_cx = 62i32;
    let back_cy = 48i32;
    let b_dx = x as i32 - back_cx;
    let b_dy = y as i32 - back_cy;
    let b_d2 = b_dx * b_dx + b_dy * b_dy;
    if b_d2 <= 22 * 22 {
        if b_d2 >= 20 * 20 {
            return BORDER_SUBTLE;
        }
        // Left-pointing chevron: tip at (58, 48), wings to (64, 42) and (64, 54)
        if y >= 42 && y <= 54 {
            let arm = (y as i32 - 48).abs();
            let target_x = 58 + arm;
            if x as i32 >= target_x && x as i32 <= target_x + 2 {
                return TEXT_PRIMARY;
            }
        }
        return SURFACE_RAISED;
    }

    // Header Title: "Phone". The run is scale 3, so it needs
    // run_rows(3) = 72 rows; the 94-row header band has room, so the line
    // box is centred in it instead of being gated to a 24-row window that
    // kept only 18 of the glyph's 57 ink rows.
    let title_rows = run_rows(TITLE_SCALE);
    let title_y0 = (HEADER_H - title_rows) / 2;
    if y >= title_y0 && y < title_y0 + title_rows {
        if let Some(c) = test_text(x, y, 120, title_y0, TITLE_SCALE, b"Phone", TEXT_PRIMARY, SURFACE_DARK) {
            return c;
        }
    }

    // Cellular signal indicator (x: 610..670, y: 40..62)
    //
    // WHY the bar count is real: the previous renderer filled all four bars from
    // the `modem_present` boolean, so a present-but-idle radio drew a full
    // signal. `signal_bars` is the `"bars"` field of the modem telemetry.
    if x >= 610 && x < 670 && y >= 40 && y < 62 {
        let bar_idx = (x - 610) / 15;
        let bar_x = (x - 610) % 15;
        let bar_min_y = 62 - (bar_idx + 1) * 5;
        if bar_x < 9 && y >= bar_min_y {
            return if (bar_idx as u8) < dialer.signal_bars {
                ACCENT_SKY
            } else {
                BORDER_SUBTLE
            };
        }
        return SURFACE_DARK;
    }

    // Battery indicator (x: 678..706, y: 44..58)
    //
    // WHY it is drawn only when the fuel gauge answered: this app holds no
    // power-control capability, so `battery_query` is refused and there is no
    // state of charge to draw. An always-drawn empty shell read as "battery at
    // 0%", which is a hardware claim this app cannot make. The browser takes the
    // same approach for telemetry it does not receive: draw nothing.
    if x >= 678 && x < 706 && y >= 44 && y < 58 {
        if let Some(soc) = dialer.battery_soc {
            let pct = soc.min(100) as usize;
            let inner_x0 = 681usize;
            let inner_x1 = 701usize;
            let inner_w = inner_x1 - inner_x0;
            let fill_w = inner_w * pct / 100;
            // Outline.
            if x == 678 || x == 702 || y == 44 || y == 57 || (x >= 703 && y >= 48 && y < 54) {
                return TEXT_MUTED;
            }
            // Real fill from the reported state of charge.
            if x >= inner_x0 && x < inner_x0 + fill_w && y >= 46 && y < 56 {
                return TEXT_PRIMARY;
            }
        }
        return SURFACE_DARK;
    }

    SURFACE_DARK
}

/// Segmented tabs. All three sections exist, so all three are live.
fn tabs_pixel(x: usize, y: usize, dialer: &DialerApp) -> u32 {
    if !(TAB_BAND_Y0..TAB_BAND_Y0 + TAB_BAND_H).contains(&y) {
        return BG_DARK;
    }
    let label_y0 = TAB_BAND_Y0 + (TAB_BAND_H.saturating_sub(run_rows(TAB_SCALE))) / 2;
    let ranges = [(TAB1_X0, TAB1_X1), (TAB2_X0, TAB2_X1), (TAB3_X0, TAB3_X1)];
    for (i, (x0, x1)) in ranges.iter().enumerate() {
        if x < *x0 || x >= *x1 {
            continue;
        }
        let w = x1 - x0;
        let active = match dialer.section {
            DialerSection::Keypad => i == 0,
            DialerSection::Recents => i == 1,
            DialerSection::Contacts => i == 2,
        };
        let (fill, border, ink) = if active {
            (TAB_ACTIVE_FILL, None, 0xFFFFFFFF)
        } else {
            (SURFACE_RAISED, Some(BORDER_SUBTLE), TEXT_MUTED)
        };
        // WHY a pill and not the old square fill: the design draws segmented
        // tabs as capsules. The text is only asked inside the pill so a glyph
        // near the label's ends cannot ink the curved cut-outs.
        let pill = std::design::primitives::Rect::new(*x0, TAB_BAND_Y0, w, TAB_BAND_H);
        if std::design::primitives::draw_pill(x, y, pill, fill, border).is_none() {
            continue;
        }
        let label_x0 = *x0 + std::design::font::center_x(TAB_LABELS[i], TAB_SCALE, w);
        if let Some(c) = test_text(x, y, label_x0, label_y0, TAB_SCALE, TAB_LABELS[i], ink, fill) {
            return c;
        }
        return fill;
    }
    BG_DARK
}

/// Dialed-number card: the number, the placeholder, the backspace disc and the
/// real outcome caption.
fn display_pixel(x: usize, y: usize, dialer: &DialerApp) -> u32 {
    if !(DISPLAY_Y0..DISPLAY_Y1).contains(&y) || x < DISPLAY_X0 || x >= DISPLAY_X1 {
        return BG_DARK;
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
        20,
        SURFACE_DARK,
        Some(BORDER_SUBTLE),
        1,
    ) {
        Some(c) => c,
        None => return BG_DARK,
    };

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
                    if let Some(c) = test_text(x, y, cx, 190, 1, &buf[..n], ACCENT_SKY, SURFACE_DARK) {
                        return c;
                    }
                }
            }
        }

        let scale = if dialer.active_len <= 10 { 4 } else { 3 };
        let y0 = 236;
        if y >= y0 && y < y0 + run_rows(scale) {
            let digits = &dialer.active_number[..dialer.active_len];
            let mut buf = [0u8; 32];
            let n = std::design::font::ellipsize(digits, scale, DISPLAY_W - 120, &mut buf);
            let total_w = std::design::font::measure(&buf[..n], scale);
            let x0 = DISPLAY_X0 + (DISPLAY_W.saturating_sub(total_w)) / 2;
            if let Some(c) = test_text(x, y, x0, y0, scale, &buf[..n], TEXT_PRIMARY, SURFACE_DARK) {
                return c;
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
            b"Enter a phone number",
            TEXT_MUTED,
            SURFACE_DARK,
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
                CallOutcome::Refused | CallOutcome::EmergencyRefused => DANGER_CRIMSON,
                _ => TEXT_MUTED,
            };
            if let Some(c) = test_text(x, y, cx, OUTCOME_Y0, 1, &buf[..n], ink, SURFACE_DARK) {
                return c;
            }
        }
    }

    // Backspace circular disc
    if dialer.active_len > 0 {
        let dx = (x as i32) - BACKSPACE_CX as i32;
        let dy = (y as i32) - BACKSPACE_CY as i32;
        let d2 = dx * dx + dy * dy;
        let r = 22i32;
        if d2 <= r * r {
            if d2 >= (r - 2) * (r - 2) {
                return 0xFFFCA5A5;
            }
            // Backspace icon 'X' inside
            if dx.abs() <= 6 && dy.abs() <= 6 && (dx == dy || dx == -dy) {
                return DANGER_CRIMSON;
            }
            return 0xFFFEE2E2;
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
                        return BG_DARK;
                    }

                    // Disc border
                    if dist_sq >= (r - 2) * (r - 2) {
                        return BORDER_SUBTLE;
                    }

                    // Bottom lip elevation depth
                    if dy >= r - 4 && dist_sq < (r - 2) * (r - 2) {
                        return 0xFFF0EFEB;
                    }

                    let disc_fill = SURFACE_DARK;

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
                        (stack_y0, stack_y0 + digit_rows - 4)
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
                            digit_bytes,
                            TEXT_PRIMARY,
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
                            sub_buf,
                            TEXT_MUTED,
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
    BG_DARK
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
fn recents_pixel(x: usize, y: usize, dialer: &DialerApp) -> u32 {
    if dialer.recents_len == 0 {
        // Headline: a scale-2 run owns 48 rows (620..668).
        let head: &[u8] = b"No calls yet";
        let head_x = DISPLAY_X0 + std::design::font::center_x(head, 2, DISPLAY_W);
        if let Some(c) = test_text(x, y, head_x, 620, 2, head, TEXT_PRIMARY, BG_DARK) {
            return c;
        }
        // Body: a scale-1 run owns 24 rows (680..704).
        let body: &[u8] = b"Every dial attempt and its real outcome is listed here";
        let mut buf = [0u8; 64];
        let n = std::design::font::ellipsize(body, 1, DISPLAY_W, &mut buf);
        if n > 0 {
            let w = std::design::font::measure(&buf[..n], 1);
            let bx = DISPLAY_X0 + (DISPLAY_W.saturating_sub(w)) / 2;
            if let Some(c) = test_text(x, y, bx, 680, 1, &buf[..n], TEXT_MUTED, BG_DARK) {
                return c;
            }
        }
        return BG_DARK;
    }

    for i in 0..dialer.recents_len {
        let row_y = RECENTS_Y0 + i * RECENTS_ROW_H;
        if y < row_y || y >= row_y + RECENTS_ROW_H {
            continue;
        }
        let rec = &dialer.recents[i];
        // Text first, plate second: returning the plate colour before the runs
        // were tested would paint every row's number out of existence.
        // Number, scale 2: run_rows(2) = 48 rows at row_y + 8.
        let mut nbuf = [0u8; 32];
        let nn = std::design::font::ellipsize(&rec.number[..rec.len], 2, DISPLAY_W - 96, &mut nbuf);
        let nx = DISPLAY_X0 + 24;
        if let Some(c) = test_text(
            x,
            y,
            nx,
            row_y + 8,
            2,
            &nbuf[..nn],
            TEXT_PRIMARY,
            SURFACE_DARK,
        ) {
            return c;
        }
        // Outcome, scale 1: run_rows(1) = 24 rows at row_y + 56.
        let out: &[u8] = match (rec.emergency, rec.accepted) {
            (true, true) => b"Emergency - accepted by telephony",
            (true, false) => b"Emergency - refused by telephony",
            (false, true) => b"Accepted by telephony",
            (false, false) => b"Refused by telephony",
        };
        let ox = DISPLAY_X0 + 24;
        if let Some(c) = test_text(x, y, ox, row_y + 56, 1, out, TEXT_MUTED, SURFACE_DARK) {
            return c;
        }
        // Row plate and divider.
        if x >= DISPLAY_X0 && x < DISPLAY_X1 {
            if y == row_y + RECENTS_ROW_H - 1 {
                return BORDER_SUBTLE;
            }
            if rec.emergency {
                return 0xFFF3E9EC;
            }
            return SURFACE_DARK;
        }
    }
    BG_DARK
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
        return BG_DARK;
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
                color::SURFACE,
                Some(if focused {
                    color::ACCENT
                } else {
                    color::BORDER
                }),
                1,
            ) else {
                return BG_DARK;
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
                    &lbuf[..ln],
                    color::TEXT_MUTED,
                    fill,
                ) {
                    return c;
                }
            }
            let mut vbuf = [0u8; 64];
            let vn = std::design::font::ellipsize(&buf[..len], 2, DISPLAY_W - 96, &mut vbuf);
            let vy = fy + 36;
            if vn > 0 {
                if let Some(c) = test_text(
                    x,
                    y,
                    DISPLAY_X0 + 24,
                    vy,
                    2,
                    &vbuf[..vn],
                    color::TEXT,
                    fill,
                ) {
                    return c;
                }
            }
            if focused {
                let cx = DISPLAY_X0 + 24 + std::design::font::measure(&vbuf[..vn], 2);
                if x >= cx && x < cx + 2 && (vy..vy + run_rows(2)).contains(&y) {
                    return color::ACCENT;
                }
            }
            return fill;
        }
        return BG_DARK;
    }

    // Browse: the selected contact, or the honest empty state.
    let Some(sel) = dialer.contact_sel else {
        let head: &[u8] = b"No contact selected";
        let hx = DISPLAY_X0 + std::design::font::center_x(head, 2, DISPLAY_W);
        if let Some(c) = test_text(x, y, hx, 206, 2, head, TEXT_PRIMARY, BG_DARK) {
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
            if let Some(c) = test_text(x, y, bx, 286, 1, &buf[..n], TEXT_MUTED, BG_DARK) {
                return c;
            }
        }
        return BG_DARK;
    };

    let rec = if sel >= ContactEditor::UNSAVED_BASE {
        dialer
            .contacts
            .unsaved_record(sel - ContactEditor::UNSAVED_BASE)
    } else {
        dialer.contacts.record(sel)
    };
    let Some(rec) = rec else {
        return BG_DARK;
    };

    // Star state, so the card agrees with the row.
    if sel < ContactEditor::UNSAVED_BASE && rec.is_favourite() {
        let label: &[u8] = b"Starred";
        let lw = std::design::font::measure(label, 1);
        let lx = DISPLAY_X1 - 24 - lw;
        if let Some(c) = test_text(x, y, lx, 196, 1, label, color::WARNING, BG_DARK) {
            return c;
        }
    }

    let mut nbuf = [0u8; CONTACT_NAME_MAX];
    let nn = std::design::font::ellipsize(rec.name_bytes(), 2, DISPLAY_W - 48, &mut nbuf);
    let nw = std::design::font::measure(&nbuf[..nn], 2);
    let nx = DISPLAY_X0 + (DISPLAY_W.saturating_sub(nw)) / 2;
    if let Some(c) = test_text(x, y, nx, 196, 2, &nbuf[..nn], TEXT_PRIMARY, BG_DARK) {
        return c;
    }

    let mut ubuf = [0u8; CONTACT_NUM_MAX];
    let un = std::design::font::ellipsize(rec.number_bytes(), 2, DISPLAY_W - 48, &mut ubuf);
    let uw = std::design::font::measure(&ubuf[..un], 2);
    let ux = DISPLAY_X0 + (DISPLAY_W.saturating_sub(uw)) / 2;
    if let Some(c) = test_text(x, y, ux, 252, 2, &ubuf[..un], TEXT_MUTED, BG_DARK) {
        return c;
    }

    // The three row actions, as labelled chips. WHY chips and not a menu: the app
    // has no menu surface, and a chip is a tap target the user can see.
    let chips: [(&[u8], u32, u32); 3] = [
        (b"CALL", color::SUCCESS, color::SURFACE),
        (b"EDIT", color::ACCENT, color::SURFACE),
        (b"DELETE", color::DANGER, color::SURFACE),
    ];
    let chip_gap = 12usize;
    let chip_w = (DISPLAY_W - 2 * chip_gap) / 3;
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
            let ln = std::design::font::ellipsize(label, 1, chip_w - 16, &mut lbuf);
            if ln > 0 {
                let lw = std::design::font::measure(&lbuf[..ln], 1);
                let lx = cx0 + (chip_w.saturating_sub(lw)) / 2;
                let ly = chip_y + (56 - run_rows(1)) / 2;
                if let Some(c) = test_text(x, y, lx, ly, 1, &lbuf[..ln], *ink, *fill) {
                    return c;
                }
            }
            return *fill;
        }
    }

    BG_DARK
}

/// The contact list rows, then the honest capacity/durability notice.
///
/// WHY saved and unsaved rows are drawn as two groups with a divider between
/// them: the durable list is a statement about storage, and the unsaved list is
/// not. Mixing them would let a contact the kernel refused to store look exactly
/// like one it accepted.
fn contacts_list_pixel(x: usize, y: usize, dialer: &DialerApp) -> u32 {
    // Row plates. Saved rows come first, then unsaved ones, because the durable
    // list is the one the user asked the device to keep.
    let total_rows = dialer.contacts.len() + dialer.contacts.unsaved_len();
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
        if !(cursor..cursor + CONTACT_ROW_H).contains(&y) {
            cursor += CONTACT_ROW_H;
            row += 1;
            continue;
        }
        let selected = dialer.contact_sel == Some(sel);
        let plate = if selected {
            color::SURFACE_RAISED
        } else {
            color::SURFACE
        };
        if std::design::primitives::draw_rounded_rect(
            x,
            y,
            std::design::primitives::Rect::new(DISPLAY_X0, cursor, DISPLAY_W, CONTACT_ROW_H),
            12,
            plate,
            Some(color::BORDER),
            1,
        )
        .is_none()
        {
            return BG_DARK;
        }
        // Avatar disc with the contact's initial.
        let av_cx = DISPLAY_X0 + 30;
        let av_cy = cursor + CONTACT_ROW_H / 2;
        let dx = x as i32 - av_cx as i32;
        let dy = y as i32 - av_cy as i32;
    if dx * dx + dy * dy <= 20 * 20 {
            let initial = [rec.initial()];
                let iw = std::design::font::measure(&initial, 1);
                let ix = (av_cx as i32 - iw as i32 / 2).max(0) as usize;
                let iy = (av_cy as i32 - run_rows(1) as i32 / 2).max(0) as usize;
                if let Some(t) = test_text(x, y, ix, iy, 1, &initial, color::TEXT_MUTED, plate) {
                    return t;
                }
                return plate;
            }
            let tx = DISPLAY_X0 + 68;
            let mut nbuf = [0u8; CONTACT_NAME_MAX];
            let nn = std::design::font::ellipsize(rec.name_bytes(), 1, DISPLAY_W - 150, &mut nbuf);
            if nn > 0 {
                if let Some(t) =
                    test_text(x, y, tx, cursor + 8, 1, &nbuf[..nn], color::TEXT, plate)
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
                    cursor + 36,
                    1,
                    &ubuf[..un],
                    color::TEXT_MUTED,
                    plate,
                ) {
                    return t;
                }
            }
            // Star. Filled for a saved favourite, outlined for an unsaved one:
            // an unsaved contact cannot be starred in storage, and drawing it
            // filled would claim otherwise.
            let sdx = x as i32 - STAR_CX as i32;
            let sdy = y as i32 - (cursor + CONTACT_ROW_H / 2) as i32;
            if sdx * sdx + sdy * sdy <= STAR_R * STAR_R {
                return if rec.is_favourite() {
                    color::WARNING
                } else {
                    color::BORDER
                };
            }
            return plate;
        }


    // Notice band under the list.
    if (NOTICE_Y0..NOTICE_Y1).contains(&y) {
        return contacts_notice_pixel(x, y, dialer);
    }
    BG_DARK
}

/// The honest capacity / durability notice under the contact list.
///
/// WHY this band always says something: the list on its own cannot tell the user
/// whether it is what storage holds, whether the store is full, or whether every
/// row in it is session-only. All three have to be visible without a log reader.
fn contacts_notice_pixel(x: usize, y: usize, dialer: &DialerApp) -> u32 {
    if !(NOTICE_Y0..NOTICE_Y1).contains(&y) || x < DISPLAY_X0 || x >= DISPLAY_X1 {
        return BG_DARK;
    }
    let used = dialer.contacts.len();
    let unsaved = dialer.contacts.unsaved_len();
    let state = dialer.contacts.state;

    // The first line is the durability statement, which is the one that matters.
    let durable = state == ContactStoreState::Written || state == ContactStoreState::Loaded;
    let ink = if durable {
        color::TEXT_MUTED
    } else {
        color::WARNING
    };
    if let Some(c) = test_text(
        x,
        y,
        DISPLAY_X0 + 24,
        NOTICE_Y0 + 4,
        1,
        state.as_str().as_bytes(),
        ink,
        BG_DARK,
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
        line[n] = b'0' + (fav_count as u8).min(9);
        n += 1;
        let mid = b" of ";
        line[n..n + mid.len()].copy_from_slice(mid);
        n += mid.len();
        line[n] = b'0' + (used as u8).min(9);
        n += 1;
    } else {
        let head = b"Saved ";
        line[n..n + head.len()].copy_from_slice(head);
        n += head.len();
        line[n] = b'0' + (used as u8).min(9);
        n += 1;
        let mid = b" of 8";
        line[n..n + mid.len()].copy_from_slice(mid);
        n += mid.len();
    }
    if unsaved > 0 {
        let tail = b" | unsaved ";
        line[n..n + tail.len()].copy_from_slice(tail);
        n += tail.len();
        line[n] = b'0' + (unsaved as u8).min(9);
        n += 1;
    }
    if used >= MAX_CONTACTS && !dialer.contacts_fav_only {
        let tail = b" | STORE FULL";
        line[n..n + tail.len()].copy_from_slice(tail);
        n += tail.len();
    }
    let mut buf = [0u8; 96];
    let tn = std::design::font::ellipsize(&line[..n], 1, DISPLAY_W - 48, &mut buf);
    if tn > 0 {
        if let Some(c) = test_text(
            x,
            y,
            DISPLAY_X0 + 24,
            NOTICE_Y0 + 34,
            1,
            &buf[..tn],
            color::TEXT_MUTED,
            BG_DARK,
        ) {
            return c;
        }
    }
    BG_DARK
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
    if s_d2 >= 1444 {
        return Some(0xFFE11D48);
    }
    // "SOS" is scale 2, so it needs run_rows(2) = 48 rows and its measured 76px
    // width must fit the 80px disc. Both are centred on the disc.
    let sos_rows = run_rows(SOS_SCALE);
    let sos_x0 = SOS_CX.saturating_sub(std::design::font::measure(b"SOS", SOS_SCALE) / 2);
    let sos_y0 = SOS_CY.saturating_sub(sos_rows / 2);
    if y >= sos_y0 && y < sos_y0 + sos_rows {
        if let Some(c) = test_text(x, y, sos_x0, sos_y0, SOS_SCALE, b"SOS", 0xFFF43F5E, SOS_FILL) {
            return Some(c);
        }
    }
    let _ = dialer;
    Some(SOS_FILL)
}

/// The contacts action row: the emergency disc is unchanged, and the other two
/// discs carry the section's primary and secondary actions.
fn contacts_action_pixel(x: usize, y: usize, dialer: &DialerApp) -> u32 {
    if !(ACTION_Y0..ACTION_Y1).contains(&y) {
        return BG_DARK;
    }
    if let Some(c) = emergency_dial_pixel(x, y, dialer) {
        return c;
    }

    let editing = dialer.contact_edit.is_some();
    let has_sel = dialer.contact_sel.is_some();
    // Centre disc: SAVE while editing, CALL with a selection, ADD otherwise.
    let centre: (Option<u32>, &[u8]) = if editing {
        (Some(color::SUCCESS), b"SAVE")
    } else if has_sel {
        (Some(color::SUCCESS), b"CALL")
    } else {
        (Some(color::ACCENT), b"ADD")
    };
    // Right disc: DELETE while editing an existing contact, CANCEL while editing
    // a new one, EDIT with a selection, and the favourites filter otherwise.
    let right: (Option<u32>, &[u8]) = if editing {
        let exists = matches!(dialer.contact_edit.and_then(|e| e.target), Some(_));
        if exists {
            (Some(color::DANGER), b"DELETE")
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
            return color::BORDER;
        }
        let mut lbuf = [0u8; 16];
        let ln = std::design::font::ellipsize(label, 1, 2 * r as usize - 12, &mut lbuf);
        if ln > 0 {
            let lw = std::design::font::measure(&lbuf[..ln], 1);
            let lx = (cx as i32 - lw as i32 / 2).max(0) as usize;
            let ly = C_ACTION_Y - run_rows(1) / 2;
            let text_color = if fill == color::SURFACE_RAISED {
                color::TEXT
            } else {
                color::SURFACE
            };
            if let Some(c) = test_text(x, y, lx, ly, 1, &lbuf[..ln], text_color, fill) {
                return c;
            }
        }
        return fill;
    }
    BG_DARK
}

/// Emergency / call / contacts row. Only drawn in the Keypad section.
fn action_pixel(x: usize, y: usize, dialer: &DialerApp) -> u32 {
    if !(ACTION_Y0..ACTION_Y1).contains(&y) {
        return BG_DARK;
    }
    if let Some(c) = emergency_dial_pixel(x, y, dialer) {
        return c;
    }

    // Call / Hangup Central Button (centre 360, 1065, r 52)
    let c_dx = (x as i32) - CALL_CX as i32;
    let c_dy = (y as i32) - CALL_CY as i32;
    let c_d2 = c_dx * c_dx + c_dy * c_dy;
    if c_d2 <= CALL_R * CALL_R {
        let in_call = dialer.state != CallState::Idle;
        let can_call = in_call || dialer.active_len > 0;

        let (bg_color, border_color, glyph) = if in_call {
            (DANGER_CRIMSON, 0xFFB91C1C, 0xFFFFFFFF)
        } else if can_call {
            (SUCCESS_EMERALD, 0xFF15803D, 0xFFFFFFFF)
        } else {
            // Fresh soft mint circle with emerald icon when idle
            (0xFFD1FAE5, 0xFFA7F3D0, 0xFF059669)
        };

        if c_d2 >= (CALL_R - 2) * (CALL_R - 2) {
            return border_color;
        }

        // Vector Phone Handset
        if !in_call {
            // Angled handset (green button)
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
            // Horizontal hangup handset (red button)
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

    // Contacts shortcut (centre 586, 1065, r 40)
    let ct_dx = (x as i32) - CONTACTS_CX as i32;
    let ct_dy = (y as i32) - CONTACTS_CY as i32;
    let ct_d2 = ct_dx * ct_dx + ct_dy * ct_dy;
    if ct_d2 <= CONTACTS_R * CONTACTS_R {
        if ct_d2 >= (CONTACTS_R - 2) * (CONTACTS_R - 2) {
            return BORDER_SUBTLE;
        }
        // Person silhouette
        if ct_dx * ct_dx + (ct_dy + 8) * (ct_dy + 8) <= 49 {
            return TEXT_PRIMARY;
        }
        if ct_dy >= 2 && ct_dy <= 14 && ct_dx.abs() <= (ct_dy - 2) + 6 {
            return TEXT_PRIMARY;
        }
        return SURFACE_DARK;
    }

    BG_DARK
}

/// Bottom navigation dock. All four slots are live: Favorites opens the contacts
/// list filtered to the contacts the user starred, and the other three select a
/// section.
fn dock_pixel(x: usize, y: usize, dialer: &DialerApp) -> u32 {
    if !(DOCK_Y0..DOCK_Y1).contains(&y) {
        return BG_DARK;
    }
    // Top hairline divider
    if y == DOCK_Y0 {
        return BORDER_SUBTLE;
    }

    let mut slot = 4usize;
    for (i, x0) in DOCK_ITEM_X0.iter().enumerate() {
        if x >= *x0 && x < *x0 + DOCK_ITEM_W {
            slot = i;
            break;
        }
    }
    if slot == 4 {
        return SURFACE_DARK;
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

    let ink = if active { ACCENT_SKY } else { TEXT_MUTED };
    let plate = SURFACE_DARK;

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
                        color::WARNING
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
            DOCK_LABELS[slot],
            ink,
            plate,
        ) {
            return c;
        }
    }

    // Active pill background under icon
    if active {
        let pill_rect = std::design::primitives::Rect::new(cx - 32, cy - 16, 64, 32);
        if let Some(p_color) = std::design::primitives::draw_pill(x, y, pill_rect, 0xFFDBEAFE, None) {
            return p_color;
        }
    }

    plate
}

#[inline(never)]
fn get_dialer_pixel(x: usize, y: usize, dialer: &DialerApp) -> u32 {
    // 1. Status bar & header (y: 0..94)
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
    if (DISPLAY_Y0..DISPLAY_Y1).contains(&y) {
        return match dialer.section {
            DialerSection::Contacts => contacts_card_pixel(x, y, dialer),
            _ => display_pixel(x, y, dialer),
        };
    }

    // 4. Section content (y: 410..1140): the dial pad and the call controls in
    //    the Keypad section, the real call ledger in Recents, and the contact
    //    store -- the list while browsing, or the dial pad typed as a multi-tap
    //    text entry -- in Contacts.
    if (CONTENT_Y0..CONTENT_Y1).contains(&y) {
        return match dialer.section {
            DialerSection::Keypad => {
                let c = keypad_pixel(x, y);
                if c != BG_DARK {
                    return c;
                }
                action_pixel(x, y, dialer)
            }
            DialerSection::Recents => recents_pixel(x, y, dialer),
            DialerSection::Contacts => {
                if dialer.contact_edit.is_some() {
                    // The editor reuses the real dial pad, so its geometry, its
                    // hit test and its sublabels are the same ones the keypad
                    // section uses. Only what a press produces changes.
                    let c = editor_keypad_pixel(x, y, dialer);
                    if c != BG_DARK {
                        return c;
                    }
                    return contacts_action_pixel(x, y, dialer);
                }
                if (LIST_Y0..NOTICE_Y0).contains(&y) {
                    return contacts_list_pixel(x, y, dialer);
                }
                contacts_action_pixel(x, y, dialer)
            }
        };
    }

    // 5. Bottom navigation dock (y: 1260..1380)
    if (DOCK_Y0..DOCK_Y1).contains(&y) {
        return dock_pixel(x, y, dialer);
    }

    // 6. Home Gesture Pill (x: 260..460, y: 1418..1424)
    if y >= 1418 && y < 1424 && x >= 260 && x < 460 {
        return TEXT_MUTED;
    }

    BG_DARK
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
        let _ = std::surface_write(6, surface, offset, &chunk[..count]);
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
        std::log(
            0,
            "[DIALER] Call requested with no number entered; no telephony request was issued.",
        )
        .ok();
        return Dirty::None;
    }
    let mut num_buf = [0u8; 32];
    let len = dialer.active_len;
    num_buf[..len].copy_from_slice(&dialer.active_number[..len]);
    let Ok(target) = core::str::from_utf8(&num_buf[..len]) else {
        std::log(0, "[DIALER] Invalid dial string.").ok();
        return Dirty::None;
    };
    if dialer.dial(target).is_ok() {
        std::log(0, "[DIALER] Telephony service accepted the call.").ok();
    } else {
        std::log(0, "[DIALER] Call could not be established.").ok();
    }
    Dirty::All
}

fn handle_tap(dialer: &mut DialerApp, px: u32, py: u32) -> Tap<'static> {
    let x = px as usize;
    let y = py as usize;

    // 1. Back control in the header, or the home gesture bar: exit to launcher.
    if (x >= 24 && x < 100 && y >= 28 && y < 68) || (x >= 240 && x <= 480 && y >= 1400) {
        return Tap::Leave("[DIALER APP] Back control or home gesture pressed. Exiting to launcher.");
    }

    // 2. Segmented tabs. All three are live sections.
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
            dialer.open_contacts(dialer.contacts_fav_only);
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
                1 => {
                    dialer.section = DialerSection::Recents;
                    return Tap::Redraw(Dirty::All);
                }
                2 => {
                    dialer.section = DialerSection::Keypad;
                    return Tap::Redraw(Dirty::All);
                }
                // Favorites and Contacts both open the contact list; Favorites
                // filters it to the starred contacts, which is what the slot
                // says it shows.
                0 => {
                    dialer.open_contacts(true);
                    std::log(0, "[DIALER APP] Favorites: showing starred contacts.").ok();
                    return Tap::Redraw(Dirty::All);
                }
                _ => {
                    dialer.open_contacts(false);
                    return Tap::Redraw(Dirty::All);
                }
            }
        }
    }

    // 4. Backspace disc: hit-tested against exactly the drawn circle.
    if dialer.active_len > 0 && in_circle(x, y, BACKSPACE_CX, BACKSPACE_CY, BACKSPACE_R) {
        dialer.backspace();
        return Tap::Redraw(if dialer.active_len == 0 { Dirty::All } else { Dirty::Display });
    }

    // 5. Section content: the keypad and action row in the Keypad section, the
    if dialer.section == DialerSection::Recents {
        if y >= RECENTS_Y0 && y < RECENTS_Y0 + dialer.recents_len * RECENTS_ROW_H {
            let row_idx = (y - RECENTS_Y0) / RECENTS_ROW_H;
            if row_idx < dialer.recents_len {
                let rec = &dialer.recents[row_idx];
                dialer.active_number = [0u8; 32];
                let n = rec.len.min(32);
                dialer.active_number[..n].copy_from_slice(&rec.number[..n]);
                dialer.active_len = n;
                dialer.section = DialerSection::Keypad;
                std::log(0, "[DIALER APP] Loaded number from recents.").ok();
                let _ = std::audio_haptic_click();
                return Tap::Redraw(Dirty::All);
            }
        }
        return Tap::None;
    }

    if dialer.section == DialerSection::Contacts {
        return handle_contacts_tap(dialer, x, y);
    }

    if dialer.section == DialerSection::Keypad {
        // Keypad Matrix
        for row in 0..4 {
            let by = ROW_Y[row];
            if y >= by && y < by + BTN_H {
                for col in 0..3 {
                    let bx = COL_X[col];
                    if x >= bx && x < bx + BTN_W {
                        if let Some(digit) = get_keypad_char(row, col) {
                            dialer.input_digit(digit);
                            std::log(0, "[DIALER APP] Keypad digit entered.").ok();
                        }
                        return Tap::Redraw(if dialer.active_len == 1 { Dirty::All } else { Dirty::Display });
                    }
                }
            }
        }

        // Action Bar: three discs, each hit-tested against its drawn circle.
        if (ACTION_Y0..ACTION_Y1).contains(&y) {
            if in_circle(x, y, SOS_CX, SOS_CY, SOS_R) {
                let _ = dialer.dial_emergency();
                if dialer.outcome == CallOutcome::EmergencyRefused {
                    std::log(
                        0,
                        "[DIALER APP] Emergency call refused by the telephony service; the outcome is shown under the number.",
                    )
                    .ok();
                } else {
                    std::log(
                        0,
                        "[DIALER APP] Telephony service accepted the emergency call.",
                    )
                    .ok();
                }
                return Tap::Redraw(Dirty::All);
            }
            if in_circle(x, y, CALL_CX, CALL_CY, CALL_R) {
                return Tap::Redraw(toggle_call(dialer));
            }
            if in_circle(x, y, CONTACTS_CX, CONTACTS_CY, CONTACTS_R) {
                dialer.open_contacts(false);
                let _ = std::audio_haptic_click();
                return Tap::Redraw(Dirty::All);
            }
        }
    }

    Tap::None
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
                    return Tap::Redraw(Dirty::Fields);
                }
            }
        }

        // Action row: the emergency disc keeps its own hit test, then the two
        // editing discs.
        if (ACTION_Y0..ACTION_Y1).contains(&y) {
            if in_circle(x, y, SOS_CX, SOS_CY, SOS_R) {
                let _ = dialer.dial_emergency();
                if dialer.outcome == CallOutcome::EmergencyRefused {
                    std::log(
                        0,
                        "[DIALER APP] Emergency call refused by the telephony service; the outcome is shown under the number.",
                    )
                    .ok();
                } else {
                    std::log(
                        0,
                        "[DIALER APP] Telephony service accepted the emergency call.",
                    )
                    .ok();
                }
                return Tap::Redraw(Dirty::All);
            }
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
    // Card chips: CALL / EDIT / DELETE.
    let chip_gap = 12usize;
    let chip_w = (DISPLAY_W - 2 * chip_gap) / 3;
    let chip_y = 312;
    if (chip_y..chip_y + 56).contains(&y) && x >= DISPLAY_X0 && x < DISPLAY_X1 {
        for i in 0..3 {
            let cx0 = DISPLAY_X0 + i * (chip_w + chip_gap);
            if x < cx0 || x >= cx0 + chip_w {
                continue;
            }
            if dialer.contact_sel.is_none() {
                // WHY a chip with no selection is inert rather than defaulting to
                // row 0: there is no contact the user pointed at, and guessing
                // one would call, edit or delete a contact nobody chose.
                std::log(
                    0,
                    "[DIALER APP] That action needs a selected contact; tap a row first.",
                )
                .ok();
                return Tap::Redraw(Dirty::None);
            }
            match i {
                0 => dialer.dial_selected_contact(),
                1 => dialer.start_edit_contact(dialer.contact_sel.unwrap_or(0)),
                _ => dialer.delete_selected_contact(),
            }
            return Tap::Redraw(Dirty::All);
        }
    }

    // Action row: SOS unchanged, then ADD / CALL and FAVS / EDIT.
    if (ACTION_Y0..ACTION_Y1).contains(&y) {
        if in_circle(x, y, SOS_CX, SOS_CY, SOS_R) {
            let _ = dialer.dial_emergency();
            if dialer.outcome == CallOutcome::EmergencyRefused {
                std::log(
                    0,
                    "[DIALER APP] Emergency call refused by the telephony service; the outcome is shown under the number.",
                )
                .ok();
            } else {
                std::log(
                    0,
                    "[DIALER APP] Telephony service accepted the emergency call.",
                )
                .ok();
            }
            return Tap::Redraw(Dirty::All);
        }
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
                std::log(
                    0,
                    if dialer.contacts_fav_only {
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
            if y < cursor || y >= cursor + CONTACT_ROW_H {
                cursor += CONTACT_ROW_H;
                row += 1;
                continue;
            }
            if in_circle(x, y, STAR_CX, cursor + CONTACT_ROW_H / 2, STAR_R) {
                dialer.contact_sel = Some(sel);
                dialer.toggle_selected_favourite();
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
            std::log(0, "[DIALER APP] Section switched by keyboard.").ok();
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
    std::log(
        0,
        "[DIALER APP (Ring 3)] Sovereign Dialer & Telephony online.",
    )
    .ok();

    let mut dialer = DialerApp::new();

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
                std::log(
                    0,
                    "[DIALER APP] Baseband Modem: Hardware detected and online.",
                )
                .ok();
            } else {
                // WHY: the payload simply did not list a present modem.
                // "Offline" would assert a baseband state the HAL never
                // reported, so state only what the report actually said.
                std::log(
                    0,
                    "[DIALER APP] Baseband Modem: the telemetry report did not list a present modem.",
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
                    std::log(
                        0,
                        "[DIALER APP] Modem telemetry carried no signal bar count; the meter shows no bars.",
                    )
                    .ok();
                }
            }
        }
        Ok(_) => {
            std::log(
                0,
                "[DIALER APP] Telephony query returned an empty payload; no baseband reported.",
            )
            .ok();
        }
        Err(_) => {
            std::log(
                0,
                "[DIALER APP] Telephony query refused by the kernel; no cellular baseband is available to this app.",
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
                        std::log(
                            0,
                            "[DIALER APP] Fuel gauge reported no state of charge; the battery indicator is left blank.",
                        )
                        .ok();
                    }
                }
            } else {
                std::log(
                    0,
                    "[DIALER APP] No battery fuel gauge is present; the battery indicator is left blank.",
                )
                .ok();
            }
        }
        Ok(_) => {
            std::log(
                0,
                "[DIALER APP] Battery query returned an empty payload; the battery indicator is left blank.",
            )
            .ok();
        }
        Err(_) => {
            std::log(
                0,
                "[DIALER APP] Battery query refused by the kernel; the battery indicator is left blank.",
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
        ContactStoreState::Loaded => { std::log(
            0,
            "[DIALER APP] Contacts read back out of the settings store.",
        )
        .ok();
        }
        ContactStoreState::Empty => { std::log(
            0,
            "[DIALER APP] No saved contacts: the settings store holds no dialer contacts key.",
        )
        .ok();
        }
        ContactStoreState::ReadRefused => { std::log(
            0,
            "[DIALER APP] Contacts read REFUSED by the kernel: no stored contacts were loaded, and none will be shown as saved.",
        )
        .ok();
        }
        ContactStoreState::Invalid => { std::log(
            0,
            "[DIALER APP] Stored contact bytes failed validation; no contacts were loaded from them.",
        )
        .ok();
        }
        ContactStoreState::Untouched | ContactStoreState::Written
        | ContactStoreState::WriteRefused | ContactStoreState::Full => {
            std::log(
                0,
                "[DIALER APP] Contacts store reported a state it cannot be in before any write; no contacts are shown as saved.",
            )
            .ok();
        }
    }

    // 3. Allocate dedicated KMS Client Surface (z=200)
    std::log(
        0,
        "[DIALER APP] Step 1: Allocating application surface (z=200)...",
    )
    .ok();
    let surface = match std::surface_create(6, W as u32, H as u32, 200) {
        Ok(s) => {
            std::log(0, "[DIALER APP] Step 2: Surface created successfully.").ok();
            s
        }
        Err(_) => {
            std::log(
                0,
                "[DIALER APP] Display surface unavailable; refusing headless call simulation.",
            )
            .ok();
            std::exit_with_code(1);
        }
    };

    // 4. Render an empty dialer; numbers are entered by the user.
    render_dialer_ui(surface, &dialer);
    let _ = std::surface_flip(6);
    std::log(
        0,
        "[DIALER APP] Step 3: Initial frame presented to hardware scanout.",
    )
    .ok();

    // 5. Calls are initiated only from user input; no automatic call is placed at startup.

    // 6. Interactive Event Reactor Loop
    loop {
        if let Ok(events) = std::event_poll(
            std::EVENT_TOUCH_INPUT | std::EVENT_TIMER_TICK | std::EVENT_MODEM_URC,
            100,
        ) {
            if (events & std::EVENT_TIMER_TICK) != 0 {
                if dialer.state == CallState::ActiveCall {
                    dialer.call_duration_secs += 1;
                }
                // WHY no flip here: nothing on the surface is derived from the
                // call timer, so re-presenting an unchanged frame costs a full
                // scanout copy for no visible difference.
            }

            if (events & std::EVENT_MODEM_URC) != 0 {
                // WHY: the previous code logged an inbound tower alert and called
                // it a call for any MODEM_URC bit. The URC vector was never read,
                // no call setup message was decoded, and no baseband is attached,
                // so no inbound call occurred. Report the event only.
                if dialer.modem_present {
                    std::log(
                        0,
                        "[DIALER APP] Baseband URC signalled; no call setup message was decoded from it.",
                    )
                    .ok();
                } else {
                    std::log(
                        0,
                        "[DIALER APP] Modem URC event raised with no attached baseband; no call was received.",
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
                                std::log(0, msg).ok();
                                let _ = std::surface_destroy(6, surface);
                                std::exit_with_code(0);
                            }
                            Tap::Redraw(d) => dirty = merge(dirty, d),
                            Tap::None => {}
                        }
                        continue;
                    }
                    if input.kind == 4 || input.kind == 6 || input.kind == 7 || input.kind == 8 || input.kind == 10 {
                        std::log(
                            0,
                            "[DIALER APP] Navigation gesture recognized. Returning to launcher.",
                        )
                        .ok();
                        let _ = std::surface_destroy(6, surface);
                        std::exit_with_code(0);
                    }
                    if input.kind == 1 {
                        match handle_tap(&mut dialer, input.x, input.y) {
                            Tap::Leave(msg) => {
                                std::log(0, msg).ok();
                                let _ = std::surface_destroy(6, surface);
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
                        Dirty::Display => render_dialer_band(surface, &dialer, DISPLAY_Y0, DISPLAY_Y1),
                        // The editor's fields live in the display-card band, so
                        // typing into them repaints exactly that band.
                        Dirty::Fields => render_dialer_band(surface, &dialer, DISPLAY_Y0, DISPLAY_Y1),
                        Dirty::All => render_dialer_ui(surface, &dialer),
                    }
                    let _ = std::surface_flip(6);
                }
            }
        }

        std::yield_now();
    }
}

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    std::log(0, "[DIALER APP] Panic detected! Terminating task...").ok();
    std::exit_with_code(1);
}
