#![no_std]

pub use interos_std as std;

pub const CONTACT_NAME_MAX: usize = 18;
pub const CONTACT_NUM_MAX: usize = 16;
pub const CONTACT_RECORD_BYTES: usize = 1 + 1 + CONTACT_NAME_MAX + CONTACT_NUM_MAX; // 36 bytes

/// Maximum durable contacts. Previously capped at 8 due to unescaped 0x0A ('\n')
/// values in the length/id fields failing settings storage checks.
pub const MAX_CONTACTS: usize = 12;

pub const STORE_BLOB_BYTES: usize = 2 + MAX_CONTACTS * CONTACT_RECORD_BYTES;
pub const STORE_VERSION: u8 = 1;

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

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct ContactRecord {
    pub id: u8,
    pub flags: u8,
    pub name: [u8; CONTACT_NAME_MAX],
    pub number: [u8; CONTACT_NUM_MAX],
}

impl ContactRecord {
    pub const EMPTY: ContactRecord = ContactRecord {
        id: 0,
        flags: 0,
        name: [0; CONTACT_NAME_MAX],
        number: [0; CONTACT_NUM_MAX],
    };

    pub const FLAG_FAVOURITE: u8 = 1 << 0;

    pub fn from_parts(id: u8, name: &[u8], number: &[u8], favourite: bool) -> Self {
        let mut rec = ContactRecord::EMPTY;
        rec.id = id;
        rec.set_favourite(favourite);
        let n = name.len().min(CONTACT_NAME_MAX);
        rec.name[..n].copy_from_slice(&name[..n]);
        let m = number.len().min(CONTACT_NUM_MAX);
        rec.number[..m].copy_from_slice(&number[..m]);
        rec
    }

    pub fn valid(&self) -> bool {
        (1..=MAX_CONTACTS as u8).contains(&self.id)
            && self.flags & !Self::FLAG_FAVOURITE == 0
            && self.name_bytes().iter().all(|b| (0x20..=0x7E).contains(b))
            && self
                .number
                .iter()
                .take(self.number_len())
                .all(|b| b.is_ascii_digit() || *b == b'+' || *b == b'*' || *b == b'#')
    }

    pub fn name_len(&self) -> usize {
        self.name.iter().position(|&b| b == 0).unwrap_or(CONTACT_NAME_MAX)
    }

    pub fn number_len(&self) -> usize {
        self.number
            .iter()
            .position(|&b| b == 0)
            .unwrap_or(CONTACT_NUM_MAX)
    }

    pub fn name_bytes(&self) -> &[u8] {
        &self.name[..self.name_len()]
    }

    pub fn number_bytes(&self) -> &[u8] {
        &self.number[..self.number_len()]
    }

    pub fn is_favourite(&self) -> bool {
        self.flags & Self::FLAG_FAVOURITE != 0
    }

    pub fn set_favourite(&mut self, on: bool) {
        self.flags = (self.flags & !Self::FLAG_FAVOURITE) | (on as u8) * Self::FLAG_FAVOURITE;
    }

    pub fn encode(&self, out: &mut [u8]) {
        out[0] = self.id;
        out[1] = self.flags;
        out[2..2 + CONTACT_NAME_MAX].copy_from_slice(&self.name);
        out[2 + CONTACT_NAME_MAX..2 + CONTACT_NAME_MAX + CONTACT_NUM_MAX].copy_from_slice(&self.number);
    }

    pub fn decode(bytes: &[u8]) -> Option<ContactRecord> {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escape_and_unescape_roundtrip_with_0x0a() {
        let raw = [STORE_VERSION, 10, 0x0A, 42, 0xFE, 99];
        let mut enc = [0u8; 32];
        let enc_len = escape_store_blob(&raw, &mut enc).expect("escape");
        // Must contain NO 0x0A bytes
        assert!(!enc[..enc_len].iter().any(|&b| b == b'\n'));

        let mut dec = [0u8; 32];
        let dec_len = unescape_store_blob(&enc[..enc_len], &mut dec).expect("unescape");
        assert_eq!(&dec[..dec_len], &raw[..]);
    }

    #[test]
    fn contact_store_10_contacts_ceiling_lifted() {
        // Create 10 valid contact records
        let mut raw = [0u8; STORE_BLOB_BYTES];
        raw[0] = STORE_VERSION;
        raw[1] = 10; // count = 10 is 0x0A!

        for i in 1..=10 {
            let name = b"Contact";
            let num = b"+123456789";
            let rec = ContactRecord::from_parts(i as u8, name, num, i % 2 == 0);
            assert!(rec.valid());
            let offset = 2 + (i - 1) * CONTACT_RECORD_BYTES;
            rec.encode(&mut raw[offset..offset + CONTACT_RECORD_BYTES]);
        }
        let raw_len = 2 + 10 * CONTACT_RECORD_BYTES;

        // Escaped output must have no 0x0A
        let mut enc = [0u8; STORE_BLOB_BYTES + 32];
        let enc_len = escape_store_blob(&raw[..raw_len], &mut enc).expect("escape");
        assert!(!enc[..enc_len].iter().any(|&b| b == b'\n'));

        // Unescaped matches raw
        let mut dec = [0u8; STORE_BLOB_BYTES];
        let dec_len = unescape_store_blob(&enc[..enc_len], &mut dec).expect("unescape");
        assert_eq!(dec_len, raw_len);
        assert_eq!(&dec[..dec_len], &raw[..raw_len]);

        // Verify the 10th record (id = 10, which was 0x0A) decodes properly
        let offset_10 = 2 + 9 * CONTACT_RECORD_BYTES;
        let rec10 = ContactRecord::decode(&dec[offset_10..offset_10 + CONTACT_RECORD_BYTES]).expect("rec 10 decode");
        assert_eq!(rec10.id, 10);
    }

    #[test]
    fn legacy_unescaped_blob_decodes_identically() {
        let legacy = [STORE_VERSION, 2, 1, 0, b'A', 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, b'1', 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0];
        let mut dec = [0u8; 64];
        let dec_len = unescape_store_blob(&legacy, &mut dec).expect("unescape legacy");
        assert_eq!(&dec[..dec_len], &legacy[..]);
    }
}
