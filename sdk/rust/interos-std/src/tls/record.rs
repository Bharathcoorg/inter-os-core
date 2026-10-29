//! TLS 1.3 Record Layer (RFC 8446 Section 5)
//!
//! Provides bounded, authenticated encryption and decryption of TLS records
//! using ChaCha20-Poly1305 AEAD. Pure `#![no_std]`, zero heap allocations.

use crate::crypto::chacha20_poly1305::{ChaCha20Poly1305, KEY_LEN, NONCE_LEN, TAG_LEN};
use super::TlsError;

pub const RECORD_HEADER_LEN: usize = 5;
pub const TLS_LEGACY_VERSION: u16 = 0x0303;
pub const MAX_PLAINTEXT_LEN: usize = 16384;
pub const MAX_CIPHERTEXT_LEN: usize = MAX_PLAINTEXT_LEN + 256;

/// The largest plaintext a peer seals into one record.
///
/// WHY it is one byte above the fragment limit: the inner content type travels
/// inside the protected plaintext, so a server that fills a fragment to
/// `MAX_PLAINTEXT_LEN` seals that many bytes plus the type. Measured against a
/// live OpenSSL server, the biggest record it answers a page-sized write with
/// carries a length field of 16401 — fragment, type, and tag.
pub const MAX_PROTECTED_LEN: usize = MAX_PLAINTEXT_LEN + 1;

/// The largest whole record a reader has to be able to hold.
pub const MAX_RECORD_WIRE_LEN: usize = RECORD_HEADER_LEN + MAX_PROTECTED_LEN + TAG_LEN;

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum ContentType {
    Invalid = 0,
    ChangeCipherSpec = 20,
    Alert = 21,
    Handshake = 22,
    ApplicationData = 23,
}

impl TryFrom<u8> for ContentType {
    type Error = TlsError;
    fn try_from(v: u8) -> Result<Self, Self::Error> {
        match v {
            20 => Ok(Self::ChangeCipherSpec),
            21 => Ok(Self::Alert),
            22 => Ok(Self::Handshake),
            23 => Ok(Self::ApplicationData),
            _ => Err(TlsError::InvalidContentType),
        }
    }
}

/// Computes the 12-byte per-record nonce by XORing the IV with the 64-bit sequence number.
pub fn compute_nonce(iv: &[u8; NONCE_LEN], seq: u64) -> [u8; NONCE_LEN] {
    let mut nonce = *iv;
    let seq_bytes = seq.to_be_bytes();
    for i in 0..8 {
        nonce[4 + i] ^= seq_bytes[i];
    }
    nonce
}

/// TLS 1.3 Record Layer state machine for both sending and receiving.
pub struct RecordCipher {
    key: [u8; KEY_LEN],
    iv: [u8; NONCE_LEN],
    seq: u64,
}

impl RecordCipher {
    pub const fn new(key: [u8; KEY_LEN], iv: [u8; NONCE_LEN]) -> Self {
        Self { key, iv, seq: 0 }
    }

    pub fn sequence_number(&self) -> u64 {
        self.seq
    }

    /// Encrypts an inner TLS 1.3 plaintext record.
    ///
    /// Inner plaintext structure: `plaintext || content_type as u8`.
    /// The resulting encrypted record is written to `out` with a 5-byte header,
    /// followed by ciphertext and 16-byte Poly1305 tag.
    /// Returns the total record length in bytes.
    pub fn encrypt_record(
        &mut self,
        inner_type: ContentType,
        plaintext: &[u8],
        out: &mut [u8],
    ) -> Result<usize, TlsError> {
        let inner_len = plaintext.len() + 1; // +1 for inner ContentType byte
        let ciphertext_len = inner_len + TAG_LEN;
        let total_record_len = RECORD_HEADER_LEN + ciphertext_len;

        if total_record_len > out.len() {
            return Err(TlsError::BufferTooSmall);
        }
        if inner_len > MAX_PLAINTEXT_LEN {
            return Err(TlsError::RecordTooLarge);
        }

        // 1. Construct 5-byte outer record header (legacy TLS 1.2 framing)
        out[0] = ContentType::ApplicationData as u8;
        out[1..3].copy_from_slice(&TLS_LEGACY_VERSION.to_be_bytes());
        out[3..5].copy_from_slice(&(ciphertext_len as u16).to_be_bytes());

        // 2. Compute per-record nonce
        let nonce = compute_nonce(&self.iv, self.seq);
        self.seq = self.seq.wrapping_add(1);

        // 3. Prepare inner plaintext in-place inside `out`
        // We write plaintext at out[5..5 + plaintext.len()], inner_type at out[5 + plaintext.len()]
        let payload_start = RECORD_HEADER_LEN;
        out[payload_start..payload_start + plaintext.len()].copy_from_slice(plaintext);
        out[payload_start + plaintext.len()] = inner_type as u8;

        // 4. AEAD encryption
        let mut tag = [0u8; TAG_LEN];
        let mut aad = [0u8; RECORD_HEADER_LEN];
        aad.copy_from_slice(&out[..RECORD_HEADER_LEN]);

        // Temporary scratch for ciphertext + tag
        // Since we want zero heap allocation, we can encrypt directly or in a stack buffer.
        // For no_std safety, we encrypt in-place chunk by chunk or using a stack buffer.
        // Let's copy inner plaintext to a stack buffer or encrypt in place.
        // Notice: ChaCha20Poly1305::encrypt needs separate plaintext and ciphertext buffers,
        // or we can encrypt in place by doing it in a local buffer for small packets or chunking.
        let mut inner_buf = [0u8; 2048];
        if inner_len <= inner_buf.len() {
            inner_buf[..plaintext.len()].copy_from_slice(plaintext);
            inner_buf[plaintext.len()] = inner_type as u8;
            ChaCha20Poly1305::encrypt(
                &self.key,
                &nonce,
                &aad,
                &inner_buf[..inner_len],
                &mut out[payload_start..payload_start + inner_len],
                &mut tag,
            );
            out[payload_start + inner_len..total_record_len].copy_from_slice(&tag);
            Ok(total_record_len)
        } else {
            Err(TlsError::RecordTooLarge)
        }
    }

    /// Decrypts a TLS 1.3 record.
    ///
    /// Validates the 5-byte header, verifies the Poly1305 tag, and strips the inner ContentType byte.
    /// Returns `(ContentType, payload_length)`.
    pub fn decrypt_record<'a>(
        &mut self,
        record: &[u8],
        out: &mut [u8],
    ) -> Result<(ContentType, usize), TlsError> {
        if record.len() < RECORD_HEADER_LEN + TAG_LEN + 1 {
            return Err(TlsError::BadRecordLength);
        }

        let outer_type = record[0];
        let legacy_ver = u16::from_be_bytes([record[1], record[2]]);
        let record_len = u16::from_be_bytes([record[3], record[4]]) as usize;

        if legacy_ver != TLS_LEGACY_VERSION {
            return Err(TlsError::InvalidRecordVersion);
        }
        if outer_type != ContentType::ApplicationData as u8 {
            return Err(TlsError::InvalidContentType);
        }
        if record.len() < RECORD_HEADER_LEN + record_len {
            return Err(TlsError::BadRecordLength);
        }
        if record_len < TAG_LEN + 1 {
            return Err(TlsError::BadRecordLength);
        }

        let ciphertext_len = record_len - TAG_LEN;
        if ciphertext_len > out.len() {
            return Err(TlsError::BufferTooSmall);
        }
        if ciphertext_len > MAX_PROTECTED_LEN {
            return Err(TlsError::RecordTooLarge);
        }

        let aad = &record[..RECORD_HEADER_LEN];
        let ciphertext = &record[RECORD_HEADER_LEN..RECORD_HEADER_LEN + ciphertext_len];
        let mut tag = [0u8; TAG_LEN];
        tag.copy_from_slice(&record[RECORD_HEADER_LEN + ciphertext_len..RECORD_HEADER_LEN + record_len]);

        let nonce = compute_nonce(&self.iv, self.seq);
        self.seq = self.seq.wrapping_add(1);

        // WHY the caller's buffer is also the decryption target: an intermediate
        // frame of the reader's own would cap every record at its size, and a
        // server fills a record to the protocol limit, not to this reader's.
        ChaCha20Poly1305::decrypt(
            &self.key,
            &nonce,
            aad,
            ciphertext,
            &tag,
            &mut out[..ciphertext_len],
        ).map_err(|_| TlsError::DecryptionFailed)?;

        // Find the inner content type by scanning backwards past any zero padding (RFC 8446 Section 5.4)
        let mut idx = ciphertext_len;
        while idx > 0 && out[idx - 1] == 0 {
            idx -= 1;
        }
        if idx == 0 {
            return Err(TlsError::InvalidContentType);
        }

        let inner_type_byte = out[idx - 1];
        let inner_type = ContentType::try_from(inner_type_byte)?;
        Ok((inner_type, idx - 1))
    }

    /// Verifies the record already sitting in `buf` and unmasks it where it lies.
    /// Returns `(ContentType, payload_length)`, with the payload left at
    /// `buf[RECORD_HEADER_LEN..]`.
    ///
    /// WHY this exists beside `decrypt_record`: a reader that wants the whole
    /// protocol's largest record would otherwise need a wire buffer and a
    /// plaintext buffer of the same size. The tag covers the ciphertext, so the
    /// record is fully authenticated before a byte of it is turned into
    /// plaintext, and one buffer can be both.
    ///
    /// WHY the payload is not slid to the front: copying up to 16 KiB per record
    /// to make the indexing uniform costs a pass over the document for the shape
    /// of a slice.
    pub fn open_record_in_place(&mut self, buf: &mut [u8]) -> Result<(ContentType, usize), TlsError> {
        if buf.len() < RECORD_HEADER_LEN + TAG_LEN + 1 {
            return Err(TlsError::BadRecordLength);
        }

        let outer_type = buf[0];
        let legacy_ver = u16::from_be_bytes([buf[1], buf[2]]);
        let record_len = u16::from_be_bytes([buf[3], buf[4]]) as usize;

        if legacy_ver != TLS_LEGACY_VERSION {
            return Err(TlsError::InvalidRecordVersion);
        }
        if outer_type != ContentType::ApplicationData as u8 {
            return Err(TlsError::InvalidContentType);
        }
        if buf.len() < RECORD_HEADER_LEN + record_len {
            return Err(TlsError::BadRecordLength);
        }
        if record_len < TAG_LEN + 1 {
            return Err(TlsError::BadRecordLength);
        }

        let ciphertext_len = record_len - TAG_LEN;
        if ciphertext_len > MAX_PROTECTED_LEN {
            return Err(TlsError::RecordTooLarge);
        }

        let mut tag = [0u8; TAG_LEN];
        tag.copy_from_slice(&buf[RECORD_HEADER_LEN + ciphertext_len..RECORD_HEADER_LEN + record_len]);

        let nonce = compute_nonce(&self.iv, self.seq);
        self.seq = self.seq.wrapping_add(1);

        let (aad, body) = buf.split_at_mut(RECORD_HEADER_LEN);
        ChaCha20Poly1305::open_in_place(&self.key, &nonce, aad, &mut body[..ciphertext_len], &tag)
            .map_err(|_| TlsError::DecryptionFailed)?;

        let mut idx = ciphertext_len;
        while idx > 0 && body[idx - 1] == 0 {
            idx -= 1;
        }
        if idx == 0 {
            return Err(TlsError::InvalidContentType);
        }

        let inner_type = ContentType::try_from(body[idx - 1])?;
        Ok((inner_type, idx - 1))
    }
}
