// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

//! Shared Contact Store Reader & Interlink for Sovereign Messages.
//! Reads `dialer.contacts.v1` directly from the settings store with zero heap allocation.

pub const STORE_KEY: &str = "dialer.contacts.v1";
pub const STORE_VERSION: u8 = 1;
pub const MAX_SAVED_CONTACTS: usize = 12;
pub const CONTACT_NAME_MAX: usize = 16;
pub const CONTACT_NUM_MAX: usize = 16;
pub const CONTACT_RECORD_BYTES: usize = 1 + 1 + CONTACT_NAME_MAX + CONTACT_NUM_MAX;
pub const STORE_BLOB_BYTES: usize = 2 + MAX_SAVED_CONTACTS * CONTACT_RECORD_BYTES;

pub const ESCAPE_BYTE: u8 = 0xFE;
pub const ESCAPED_NEWLINE: u8 = 0x8A;
pub const ESCAPED_ESCAPE: u8 = 0xEE;

/// Settings key for handing off a phone number to Dialer.
pub const DIALER_HANDOFF_KEY: &str = "dialer.handoff.number.v1";
pub const DIALER_APP_ID: &str = "org.interos.dialer";

/// Unescapes a store blob read from the settings store. Handles both escaped
/// payloads and raw blobs transparently.
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

/// A contact record read from the persistent contact store.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct SavedContact {
    pub id: u8,
    pub flags: u8,
    pub name: [u8; CONTACT_NAME_MAX],
    pub name_len: usize,
    pub number: [u8; CONTACT_NUM_MAX],
    pub number_len: usize,
}

impl SavedContact {
    pub const EMPTY: Self = Self {
        id: 0,
        flags: 0,
        name: [0; CONTACT_NAME_MAX],
        name_len: 0,
        number: [0; CONTACT_NUM_MAX],
        number_len: 0,
    };

    pub fn name(&self) -> &[u8] {
        &self.name[..self.name_len]
    }

    pub fn number(&self) -> &[u8] {
        &self.number[..self.number_len]
    }

    pub fn is_favourite(&self) -> bool {
        self.flags & 1 != 0
    }
}

/// Parses an unescaped raw contact store blob into bounded `SavedContact` records.
pub fn parse_contacts_blob(blob: &[u8], out: &mut [SavedContact; MAX_SAVED_CONTACTS]) -> usize {
    if blob.len() < 2 || blob[0] != STORE_VERSION {
        return 0;
    }
    let count = (blob[1] as usize).min(MAX_SAVED_CONTACTS);
    if blob.len() < 2 + count * CONTACT_RECORD_BYTES {
        return 0;
    }
    let mut loaded = 0usize;
    for i in 0..count {
        let start = 2 + i * CONTACT_RECORD_BYTES;
        let id = blob[start];
        let flags = blob[start + 1];
        if id == 0 {
            continue;
        }
        let name_bytes = &blob[start + 2..start + 2 + CONTACT_NAME_MAX];
        let num_start = start + 2 + CONTACT_NAME_MAX;
        let num_bytes = &blob[num_start..num_start + CONTACT_NUM_MAX];

        let name_len = name_bytes.iter().position(|&b| b == 0).unwrap_or(CONTACT_NAME_MAX);
        let num_len = num_bytes.iter().position(|&b| b == 0).unwrap_or(CONTACT_NUM_MAX);

        if name_len == 0 || num_len == 0 {
            continue;
        }

        out[loaded].id = id;
        out[loaded].flags = flags;
        out[loaded].name[..name_len].copy_from_slice(&name_bytes[..name_len]);
        out[loaded].name_len = name_len;
        out[loaded].number[..num_len].copy_from_slice(&num_bytes[..num_len]);
        out[loaded].number_len = num_len;
        loaded += 1;
    }
    loaded
}

/// Loads saved contacts from the system settings store.
pub fn load_saved_contacts(out: &mut [SavedContact; MAX_SAVED_CONTACTS]) -> usize {
    let mut encoded = [0u8; STORE_BLOB_BYTES + 32];
    let n = match interos_std::settings_get(STORE_KEY, &mut encoded) {
        Ok(n) if n > 0 => n,
        _ => return 0,
    };
    let mut blob = [0u8; STORE_BLOB_BYTES];
    let decoded_len = match unescape_store_blob(&encoded[..n], &mut blob) {
        Ok(len) => len,
        Err(_) => return 0,
    };
    parse_contacts_blob(&blob[..decoded_len], out)
}

/// Normalized phone number comparison ignoring spaces, hyphens, and parenthesis.
pub fn phone_numbers_match(a: &[u8], b: &[u8]) -> bool {
    if a.is_empty() || b.is_empty() {
        return false;
    }
    if a == b {
        return true;
    }
    let mut it_a = a.iter().filter(|&&c| c.is_ascii_digit());
    let mut it_b = b.iter().filter(|&&c| c.is_ascii_digit());
    let mut count = 0usize;
    loop {
        match (it_a.next(), it_b.next()) {
            (Some(x), Some(y)) if x == y => {
                count += 1;
                continue;
            }
            (None, None) => return count > 0,
            _ => return false,
        }
    }
}

/// Finds a saved contact whose phone number matches `number`.
pub fn find_contact_by_number<'a>(
    contacts: &'a [SavedContact],
    count: usize,
    number: &[u8],
) -> Option<&'a SavedContact> {
    for c in &contacts[..count.min(contacts.len())] {
        if phone_numbers_match(c.number(), number) {
            return Some(c);
        }
    }
    None
}

/// Launches the Dialer to call the specified number, writing it to `dialer.handoff.number.v1`.
pub fn hand_off_call_to_dialer(number: &[u8]) -> bool {
    if number.is_empty() || number.len() > 32 {
        return false;
    }
    if interos_std::settings_set(DIALER_HANDOFF_KEY, number).is_err() {
        return false;
    }
    interos_std::app_launch(DIALER_APP_ID).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_phone_numbers_match_variations() {
        assert!(phone_numbers_match(b"+15551234567", b"+15551234567"));
        assert!(phone_numbers_match(b"+1 555-123-4567", b"+15551234567"));
        assert!(phone_numbers_match(b"(555) 123-4567", b"5551234567"));
        assert!(!phone_numbers_match(b"1234", b"5678"));
        assert!(!phone_numbers_match(b"", b""));
    }

    #[test]
    fn test_contacts_blob_parsing() {
        let mut blob = [0u8; 2 + CONTACT_RECORD_BYTES];
        blob[0] = STORE_VERSION;
        blob[1] = 1; // 1 record
        let start = 2;
        blob[start] = 1; // id
        blob[start + 1] = 1; // flags (favourite)
        blob[start + 2..start + 7].copy_from_slice(b"Alice");
        let num_start = start + 2 + CONTACT_NAME_MAX;
        blob[num_start..num_start + 7].copy_from_slice(b"5550100");

        let mut out = [SavedContact::EMPTY; MAX_SAVED_CONTACTS];
        let loaded = parse_contacts_blob(&blob, &mut out);
        assert_eq!(loaded, 1);
        assert_eq!(out[0].id, 1);
        assert_eq!(out[0].name(), b"Alice");
        assert_eq!(out[0].number(), b"5550100");
        assert!(out[0].is_favourite());

        let matched = find_contact_by_number(&out, loaded, b"555-0100");
        assert!(matched.is_some());
        assert_eq!(matched.unwrap().name(), b"Alice");
    }
}
