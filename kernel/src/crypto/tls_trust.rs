//! TLS trust anchors installed on this device by its operator.
//!
//! # Trust model
//!
//! A certificate chain verifies only against an anchor that the owner of this
//! device put here. The OS ships none, and no application can add one: the
//! store is reachable from the serial console, which is the operator's channel,
//! and from nowhere else. This is the same decision the `.rapp` publisher store
//! makes (see `package::revocation`), and for the same reason -- a root compiled
//! into the image trusts every certificate signed by the matching private key on
//! every device that image ever boots, and that private key is exactly what an
//! attacker wants.
//!
//! WHY the store lives in RAM and not in the storage journal: every journal kind
//! from 1 to 10 is already allocated, and claiming an eleventh would move the
//! on-disk slot layout that the filesystem derives its tail reservation from --
//! a format change to every data disk, for a record that is a few hundred bytes.
//! The cost falls the safe way: a reboot withdraws the anchor, the browser
//! returns to refusing every chain, and one console line puts it back.
//!
//! WHY installing an anchor grants nothing by itself: the anchor only takes part
//! in the checks the client already performs -- the signature over each chain
//! link under the scheme that link declares, the reference identity at the leaf
//! position, and the validity window against the device clock. An anchor whose
//! subject never appears as an issuer is inert.

use crate::serial_println;
use spin::Mutex;

/// How many anchors one device holds. Four is enough for the operator's own CA
/// plus a rotation or two, and small enough that the table costs the kernel
/// nothing to keep.
pub const MAX_TRUSTED_ANCHORS: usize = 4;

/// The largest anchor accepted.
///
/// WHY 2048 rather than the 512 that fit a local CA: a public root has to fit.
/// ISRG Root X1 -- the anchor behind Let's Encrypt chains, and the one a browser
/// needs to reach any ordinary HTTPS site -- measures 1391 bytes, and a 4096-bit
/// RSA root carries roughly 700 bytes more than an Ed25519 one of the same
/// subject and extensions. The previous ceiling could not hold a real root at
/// all, so the store was only ever usable for keys made here.
///
/// WHY not larger: the store is four anchors of static space, the console has to
/// type the whole thing, and every root in the popular trust stores is under this
/// ceiling. A `MAX_ANCHOR_DER_BYTES` anchor costs the kernel 2 KiB of `.bss`.
pub const MAX_ANCHOR_DER_BYTES: usize = 2048;

/// The label the operator attaches to an anchor, so a listing can say which key
/// is which instead of printing four identical blobs.
pub const ANCHOR_LABEL_BYTES: usize = 32;

/// The DER encoding of the Ed25519 `AlgorithmIdentifier` (`OID 1.3.101.112`).
const ED25519_ALGORITHM_OID: &[u8] = &[0x06, 0x03, 0x2b, 0x65, 0x70];
/// `rsaEncryption` (`1.2.840.113549.1.1.1`), the algorithm an RSA root names.
const RSA_ENCRYPTION_OID: &[u8] =
    &[0x06, 0x09, 0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x01, 0x01, 0x01];
/// `ecPublicKey` (`1.2.840.10045.2.1`), which names a curve rather than a key.
const EC_PUBLIC_KEY_OID: &[u8] = &[0x06, 0x07, 0x2a, 0x86, 0x48, 0xce, 0x3d, 0x02, 0x01];
/// `prime256v1` (`1.2.840.10045.3.1.7`), the only curve an EC certificate can
/// name that this client does arithmetic on.
const PRIME256V1_OID: &[u8] = &[0x06, 0x08, 0x2a, 0x86, 0x48, 0xce, 0x3d, 0x03, 0x01, 0x07];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrustError {
    /// Nothing was offered.
    EmptyAnchor,
    /// The anchor is larger than [`MAX_ANCHOR_DER_BYTES`].
    AnchorTooLarge,
    /// The bytes are not one DER `SEQUENCE` whose declared length covers them.
    NotDerSequence,
    /// The certificate names no key this client can verify with, so it could
    /// never vouch for a chain this device would accept.
    NoVerifiableKey,
    /// All [`MAX_TRUSTED_ANCHORS`] slots are taken.
    StoreFull,
    /// The same anchor is already installed.
    DuplicateAnchor,
    /// No anchor at that position.
    NoAnchorAtIndex,
    /// The caller's buffer cannot hold the anchor it asked for.
    BufferTooSmall,
}

impl TrustError {
    pub const fn message(self) -> &'static str {
        match self {
            TrustError::EmptyAnchor => "no certificate bytes were offered",
            TrustError::AnchorTooLarge => "certificate is larger than the anchor limit",
            TrustError::NotDerSequence => "certificate is not one complete DER sequence",
            TrustError::NoVerifiableKey => "certificate names no key this client can verify",
            TrustError::StoreFull => "the trust store is full",
            TrustError::DuplicateAnchor => "that anchor is already installed",
            TrustError::NoAnchorAtIndex => "no trust anchor at that position",
            TrustError::BufferTooSmall => "the caller's buffer is smaller than the anchor",
        }
    }
}

/// One installed anchor.
#[derive(Clone, Copy)]
struct Anchor {
    der: [u8; MAX_ANCHOR_DER_BYTES],
    len: usize,
    label: [u8; ANCHOR_LABEL_BYTES],
    label_len: usize,
}

impl Anchor {
    const fn empty() -> Self {
        Self {
            der: [0u8; MAX_ANCHOR_DER_BYTES],
            len: 0,
            label: [0u8; ANCHOR_LABEL_BYTES],
            label_len: 0,
        }
    }

    fn der(&self) -> &[u8] {
        &self.der[..self.len]
    }

    fn label(&self) -> &str {
        core::str::from_utf8(&self.label[..self.label_len]).unwrap_or("")
    }
}

/// Rejects bytes that could not be an anchor this client verifies through.
///
/// WHY a structural check at the door: the console takes hex typed or pasted by
/// a person, and a copy that lost its tail is a real and common mistake. A
/// truncated DER stored as an anchor would simply never verify anything, and the
/// operator would have no way to tell a wrong key from an unused one.
///
/// WHY the key test is a byte search rather than a parse: the kernel holds no
/// X.509 parser, and the question being answered -- "could this client ever
/// verify a chain through this certificate?" -- only needs to know whether one of
/// the algorithm identifiers the client does arithmetic on appears in it. The
/// client still parses the anchor itself, and still refuses it if the key is not
/// in the position the certificate profile requires.
pub fn check_anchor_shape(der: &[u8]) -> Result<(), TrustError> {
    if der.is_empty() {
        return Err(TrustError::EmptyAnchor);
    }
    if der.len() > MAX_ANCHOR_DER_BYTES {
        return Err(TrustError::AnchorTooLarge);
    }
    if der[0] != 0x30 {
        return Err(TrustError::NotDerSequence);
    }
    // DER lengths are definite and the smallest possible encoding: short form
    // below 0x80, then one or two length bytes above it. Anything else -- an
    // indefinite form, or three length bytes for a 2 KiB blob -- is not the
    // certificate this store was offered.
    let (header, declared) = if der.len() < 2 {
        return Err(TrustError::NotDerSequence);
    } else if der[1] < 0x80 {
        (2usize, der[1] as usize)
    } else if der[1] == 0x81 {
        if der.len() < 3 {
            return Err(TrustError::NotDerSequence);
        }
        (3usize, der[2] as usize)
    } else if der[1] == 0x82 {
        if der.len() < 4 {
            return Err(TrustError::NotDerSequence);
        }
        (4usize, ((der[2] as usize) << 8) | der[3] as usize)
    } else {
        return Err(TrustError::NotDerSequence);
    };
    // A long-form length must be the shortest form that can carry the value, or
    // the bytes are BER rather than DER.
    if declared < 0x80 && der[1] >= 0x80 {
        return Err(TrustError::NotDerSequence);
    }
    if declared > u8::MAX as usize && der[1] == 0x81 {
        return Err(TrustError::NotDerSequence);
    }
    if header + declared != der.len() {
        return Err(TrustError::NotDerSequence);
    }
    if !names_verifiable_key(der) {
        return Err(TrustError::NoVerifiableKey);
    }
    Ok(())
}

/// Whether this certificate names a key the client can check a signature with:
/// Ed25519, RSA, or EC on the one curve implemented. An EC identifier carries no
/// meaning without its curve parameter, so a certificate that names `ecPublicKey`
/// but no `prime256v1` -- a P-384 or P-521 root -- is refused here rather than
/// installed and then silently unable to vouch for anything.
fn names_verifiable_key(der: &[u8]) -> bool {
    contains_subsequence(der, ED25519_ALGORITHM_OID)
        || contains_subsequence(der, RSA_ENCRYPTION_OID)
        || (contains_subsequence(der, EC_PUBLIC_KEY_OID)
            && contains_subsequence(der, PRIME256V1_OID))
}

fn contains_subsequence(haystack: &[u8], needle: &[u8]) -> bool {
    if needle.is_empty() || haystack.len() < needle.len() {
        return false;
    }
    haystack
        .windows(needle.len())
        .any(|window| window == needle)
}

/// The anchors this device's operator installed.
pub struct TlsTrustStore {
    anchors: [Anchor; MAX_TRUSTED_ANCHORS],
    count: usize,
}

impl TlsTrustStore {
    pub const fn new() -> Self {
        Self {
            anchors: [Anchor::empty(); MAX_TRUSTED_ANCHORS],
            count: 0,
        }
    }

    /// Installs an anchor and returns its position.
    ///
    /// WHY positions are dense: a client enumerates the store by position, so a
    /// hole would read as the end of the store and the anchors behind it would go
    /// unused. Revoking therefore closes the gap rather than leaving a tombstone.
    pub fn install(&mut self, der: &[u8], label: &str) -> Result<usize, TrustError> {
        check_anchor_shape(der)?;
        if self.count >= MAX_TRUSTED_ANCHORS {
            return Err(TrustError::StoreFull);
        }
        if self.anchors[..self.count].iter().any(|a| a.der() == der) {
            return Err(TrustError::DuplicateAnchor);
        }
        let mut slot = Anchor::empty();
        slot.der[..der.len()].copy_from_slice(der);
        slot.len = der.len();
        let bytes = label.as_bytes();
        let n = bytes.len().min(ANCHOR_LABEL_BYTES);
        slot.label[..n].copy_from_slice(&bytes[..n]);
        slot.label_len = n;
        self.anchors[self.count] = slot;
        self.count += 1;
        Ok(self.count - 1)
    }

    /// Removes the anchor at `position`, shifting later anchors down.
    pub fn revoke(&mut self, position: usize) -> Result<(), TrustError> {
        if position >= self.count {
            return Err(TrustError::NoAnchorAtIndex);
        }
        for i in position..self.count - 1 {
            self.anchors[i] = self.anchors[i + 1];
        }
        self.anchors[self.count - 1] = Anchor::empty();
        self.count -= 1;
        Ok(())
    }

    pub const fn len(&self) -> usize {
        self.count
    }

    pub const fn is_empty(&self) -> bool {
        self.count == 0
    }

    /// The anchor at `position`, or `None` past the end of the store.
    pub fn anchor_der(&self, position: usize) -> Option<&[u8]> {
        if position >= self.count {
            return None;
        }
        Some(self.anchors[position].der())
    }

    pub fn anchor_label(&self, position: usize) -> Option<&str> {
        if position >= self.count {
            return None;
        }
        Some(self.anchors[position].label())
    }

    /// Removes every anchor. This is the state a reboot leaves the device in, and
    /// the state a test restores afterwards so one case cannot decide another.
    pub fn clear(&mut self) {
        self.anchors = [Anchor::empty(); MAX_TRUSTED_ANCHORS];
        self.count = 0;
    }
}

/// The device's TLS trust anchors.
pub static TLS_TRUST_STORE: Mutex<TlsTrustStore> = Mutex::new(TlsTrustStore::new());

/// Says what the device holds, in the console's own words.
///
/// WHY this is printed at boot: an operator who installed an anchor yesterday and
/// rebooted today needs to learn from the console that the anchor is gone, rather
/// than discovering it by watching a connection fail.
pub fn report_at_boot() {
    let store = TLS_TRUST_STORE.lock();
    if store.is_empty() {
        serial_println!(
            "[TLS_TRUST] No trust anchor is installed on this device; every HTTPS certificate is untrusted until the operator installs one."
        );
        return;
    }
    serial_println!(
        "[TLS_TRUST] {} operator-installed anchor(s) held in RAM; a reboot withdraws them.",
        store.len()
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;
    use alloc::vec::Vec;

    /// A well-formed outer `SEQUENCE` of exactly `total` bytes carrying the
    /// Ed25519 algorithm identifier -- the shape the store gates on, without
    /// claiming to be a whole certificate. The length header uses whichever
    /// definite short/long form DER requires for that size, so a test that
    /// changes only `total` cannot accidentally trip the shortest-form rule.
    fn anchor_bytes(total: usize) -> Vec<u8> {
        anchor_bytes_naming(total, &[ED25519_ALGORITHM_OID])
    }

    /// The same framing, naming whichever algorithm identifiers are given.
    fn anchor_bytes_naming(total: usize, oids: &[&[u8]]) -> Vec<u8> {
        let mut der: Vec<u8> = if total <= 129 {
            vec![0x30, (total - 2) as u8]
        } else if total <= 258 {
            vec![0x30, 0x81, (total - 3) as u8]
        } else {
            let content = total - 4;
            vec![0x30, 0x82, (content >> 8) as u8, content as u8]
        };
        for oid in oids {
            der.extend_from_slice(oid);
        }
        der.resize(total, 0x41);
        der
    }

    #[test]
    fn an_empty_store_trusts_nothing() {
        let store = TlsTrustStore::new();
        assert!(store.is_empty());
        assert_eq!(store.len(), 0);
        assert_eq!(store.anchor_der(0), None);
    }

    #[test]
    fn a_complete_ed25519_anchor_is_accepted_and_reads_back() {
        let der = anchor_bytes(40);
        let mut store = TlsTrustStore::new();
        assert_eq!(store.install(&der, "demo CA"), Ok(0));
        assert_eq!(store.len(), 1);
        assert_eq!(store.anchor_der(0), Some(der.as_slice()));
        assert_eq!(store.anchor_label(0), Some("demo CA"));
    }

    #[test]
    fn a_byte_that_is_not_a_sequence_is_refused() {
        let mut store = TlsTrustStore::new();
        assert_eq!(store.install(&[0x02, 0x01, 0x00], "not a cert"), Err(TrustError::NotDerSequence));
        assert_eq!(store.install(&[], "nothing"), Err(TrustError::EmptyAnchor));
        assert!(store.is_empty(), "a refused install must not occupy a slot");
    }

    #[test]
    fn a_truncated_or_extended_anchor_is_refused() {
        let der = anchor_bytes(40);
        let mut short = der.clone();
        short.pop();
        let mut long = der.clone();
        long.push(0x00);
        let mut store = TlsTrustStore::new();
        assert_eq!(store.install(&short, "cut"), Err(TrustError::NotDerSequence));
        assert_eq!(store.install(&long, "padded"), Err(TrustError::NotDerSequence));
    }

    #[test]
    fn a_key_this_client_cannot_verify_is_refused_at_the_door() {
        // Correctly framed sequences that name a key the client has no arithmetic
        // for, so the algorithm is the only thing the door can object to: Ed448,
        // and EC on a curve other than the one implemented.
        let ed448 = anchor_bytes_naming(40, &[&[0x06, 0x03, 0x2b, 0x65, 0x71]]);
        // `prime384v1` (1.3.132.0.4) beside `ecPublicKey`.
        let p384 = anchor_bytes_naming(
            60,
            &[
                EC_PUBLIC_KEY_OID,
                &[0x06, 0x05, 0x2b, 0x81, 0x04, 0x00, 0x22],
            ],
        );
        let mut store = TlsTrustStore::new();
        assert_eq!(store.install(&ed448, "ed448 root"), Err(TrustError::NoVerifiableKey));
        assert_eq!(store.install(&p384, "p384 root"), Err(TrustError::NoVerifiableKey));
        assert!(store.is_empty(), "a refused install must not occupy a slot");
    }

    #[test]
    fn an_rsa_or_ec_root_is_accepted_at_the_door() {
        // The two shapes a public HTTPS chain ends on that this client did not
        // previously accept: an RSA root names `rsaEncryption`, and an EC root
        // names `ecPublicKey` with its curve.
        let rsa = anchor_bytes_naming(1391, &[RSA_ENCRYPTION_OID]);
        let ec = anchor_bytes_naming(400, &[EC_PUBLIC_KEY_OID, PRIME256V1_OID]);
        let mut store = TlsTrustStore::new();
        assert_eq!(store.install(&rsa, "rsa root"), Ok(0));
        assert_eq!(store.install(&ec, "ec root"), Ok(1));
        assert_eq!(store.anchor_der(0), Some(rsa.as_slice()));
    }

    #[test]
    fn a_real_public_root_of_1391_bytes_fits_the_store() {
        // ISRG Root X1 measures 1391 bytes, and the ceiling that used to hold 512
        // could not contain it at all. This is the size the store now has to
        // carry, framed the way that certificate is: a two-byte length header.
        let der = anchor_bytes_naming(1391, &[ED25519_ALGORITHM_OID]);
        assert_eq!(der.len(), 1391);
        let mut store = TlsTrustStore::new();
        assert_eq!(store.install(&der, "isrg-shaped root"), Ok(0));
        assert_eq!(store.anchor_der(0).unwrap().len(), 1391);
    }

    #[test]
    fn a_length_header_that_is_not_the_shortest_form_is_refused() {
        // 0x81 carrying a length below 0x80 is BER rather than DER. The same
        // bytes with a correct header are accepted, so the header is the only
        // thing this case can be objecting to.
        let mut der: Vec<u8> = vec![0x30, 0x81, 0x07];
        der.extend_from_slice(ED25519_ALGORITHM_OID);
        der.resize(10, 0x41);
        let mut store = TlsTrustStore::new();
        assert_eq!(store.install(&der, "ber"), Err(TrustError::NotDerSequence));
        assert_eq!(store.install(&anchor_bytes(10), "der"), Ok(0));
    }

    #[test]
    fn an_oversized_anchor_is_refused() {
        // Shape-valid at one byte past what the store can hold, so the size is
        // the only thing the door can be objecting to.
        let der = anchor_bytes(MAX_ANCHOR_DER_BYTES + 1);
        let mut store = TlsTrustStore::new();
        assert_eq!(store.install(&der, "huge"), Err(TrustError::AnchorTooLarge));
        assert_eq!(check_anchor_shape(&anchor_bytes(MAX_ANCHOR_DER_BYTES)), Ok(()));
    }

    #[test]
    fn the_store_is_bounded_and_refuses_a_second_copy_of_the_same_key() {
        let mut store = TlsTrustStore::new();
        let first = anchor_bytes(20);
        assert_eq!(store.install(&first, "anchor"), Ok(0));
        assert_eq!(
            store.install(&first, "again"),
            Err(TrustError::DuplicateAnchor),
            "installing the same key twice must be named, not silently deduped"
        );
        for i in 1..MAX_TRUSTED_ANCHORS {
            assert_eq!(store.install(&anchor_bytes(20 + i), "anchor"), Ok(i));
        }
        assert_eq!(store.len(), MAX_TRUSTED_ANCHORS);
        assert_eq!(store.install(&anchor_bytes(9), "one more"), Err(TrustError::StoreFull));
    }

    #[test]
    fn revoking_closes_the_gap_so_a_client_never_stops_early() {
        let mut store = TlsTrustStore::new();
        store.install(&anchor_bytes(10), "first").unwrap();
        store.install(&anchor_bytes(20), "second").unwrap();
        store.install(&anchor_bytes(30), "third").unwrap();
        assert_eq!(store.revoke(0), Ok(()));
        assert_eq!(store.len(), 2);
        assert_eq!(store.anchor_label(0), Some("second"));
        assert_eq!(store.anchor_label(1), Some("third"));
        assert_eq!(store.anchor_label(2), None);
        assert_eq!(store.revoke(2), Err(TrustError::NoAnchorAtIndex));
    }

    #[test]
    fn clearing_returns_the_device_to_trusting_nothing() {
        let mut store = TlsTrustStore::new();
        store.install(&anchor_bytes(10), "only").unwrap();
        store.clear();
        assert!(store.is_empty());
        assert_eq!(store.anchor_der(0), None);
    }
}
