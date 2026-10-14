//! Deterministic 3GPP TS 23.038 GSM 7-bit Default Alphabet PDU packer
//! and on-device privacy/phishing heuristic analyzer for InterOS Messages.

use interos_std::ai::{cosine_similarity_q7, MatryoshkaDim};

/// Result of on-device heuristic inspection for unsolicited or urgent messages.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SecurityBadge {
    /// Normal message with no detected urgency or credential triggers.
    VerifiedSafe,
    /// High-urgency trigger words detected ("urgent", "immediately", "action required").
    SuspiciousUrgency,
    /// Credential or financial transfer trigger detected ("password", "pin", "verify account", "otp").
    PotentialPhishing,
}

impl SecurityBadge {
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::VerifiedSafe => "VERIFIED CLEAN",
            Self::SuspiciousUrgency => "SUSPICIOUS URGENCY",
            Self::PotentialPhishing => "POTENTIAL PHISHING",
        }
    }

    pub const fn color(&self) -> u32 {
        match self {
            Self::VerifiedSafe => 0xFF2E8B57,      // Emerald Green
            Self::SuspiciousUrgency => 0xFFF59E0B,  // Amber Warning
            Self::PotentialPhishing => 0xFFEF4444,  // Crimson Threat
        }
    }
}

/// Packs an ASCII/GSM-7 septet slice (each u8 in 0..=127) into compact 8-bit octets
/// according to 3GPP TS 23.038 Section 6.1.2.
///
/// Returns the number of octets written into `out`.
pub fn pack_gsm7_septets(septets: &[u8], out: &mut [u8]) -> usize {
    if septets.is_empty() || out.is_empty() {
        return 0;
    }
    let mut out_idx = 0;
    let mut shift = 0;
    let mut carry = 0u8;

    for &s in septets {
        let val = s & 0x7F;
        if shift == 0 {
            carry = val;
            shift = 1;
        } else {
            let octet = carry | ((val << (8 - shift)) & 0xFF);
            if out_idx < out.len() {
                out[out_idx] = octet;
                out_idx += 1;
            }
            if shift == 7 {
                shift = 0;
                carry = 0;
            } else {
                carry = val >> shift;
                shift += 1;
            }
        }
    }
    if shift > 0 && out_idx < out.len() {
        out[out_idx] = carry;
        out_idx += 1;
    }
    out_idx
}

/// Unpacks 8-bit octets into 7-bit septets according to 3GPP TS 23.038.
///
/// Returns the number of septets written into `out`.
pub fn unpack_gsm7_septets(octets: &[u8], num_septets: usize, out: &mut [u8]) -> usize {
    let mut octet_idx = 0;
    let mut out_idx = 0;
    let mut shift = 0;
    let mut prev = 0u8;

    while out_idx < num_septets && out_idx < out.len() {
        if shift == 7 {
            out[out_idx] = prev;
            out_idx += 1;
            shift = 0;
            prev = 0;
            continue;
        }
        if octet_idx >= octets.len() {
            break;
        }
        let cur = octets[octet_idx];
        octet_idx += 1;

        let val = ((cur << shift) | prev) & 0x7F;
        out[out_idx] = val;
        out_idx += 1;

        prev = cur >> (7 - shift);
        shift += 1;
    }
    out_idx
}

/// Evaluates a raw message text buffer for phishing or deceptive urgency triggers.
/// Operates with `#![no_std]`, zero allocation, and bounded execution time.
pub fn inspect_message_heuristics(msg: &[u8]) -> SecurityBadge {
    if msg.is_empty() {
        return SecurityBadge::VerifiedSafe;
    }

    // High risk phishing/credential theft patterns
    const PHISHING_TRIGGERS: [&[u8]; 8] = [
        b"password",
        b"passcode",
        b"pin code",
        b"otp",
        b"verify account",
        b"wire transfer",
        b"bank account",
        b"wallet seed",
    ];

    // Artificial urgency triggers
    const URGENCY_TRIGGERS: [&[u8]; 7] = [
        b"urgent",
        b"immediately",
        b"action required",
        b"suspended",
        b"blocked",
        b"final notice",
        b"act now",
    ];

    // Case-insensitive substring matching helper without heap allocation
    let contains_ci = |needle: &[u8]| -> bool {
        if needle.is_empty() || needle.len() > msg.len() {
            return false;
        }
        for i in 0..=(msg.len() - needle.len()) {
            let mut matched = true;
            for j in 0..needle.len() {
                let m = msg[i + j].to_ascii_lowercase();
                let n = needle[j].to_ascii_lowercase();
                if m != n {
                    matched = false;
                    break;
                }
            }
            if matched {
                return true;
            }
        }
        false
    };

    for trigger in PHISHING_TRIGGERS.iter() {
        if contains_ci(trigger) {
            return SecurityBadge::PotentialPhishing;
        }
    }

    for trigger in URGENCY_TRIGGERS.iter() {
        if contains_ci(trigger) {
            return SecurityBadge::SuspiciousUrgency;
        }
    }

    // Secondary semantic layer: evaluate text embedding projection against threat centroids
    evaluate_embedding_threat(msg, MatryoshkaDim::Dim128)
}

/// Projects a text message into a Q7 fixed-point embedding vector of dimension `dim`.
///
/// Deterministic hash-based projection mapping tokens into a bounded unit-normalized
/// vector representation for fast on-device threat classification without heap allocations.
pub fn project_text_embedding_q7(msg: &[u8], dim: MatryoshkaDim, out: &mut [i8]) -> usize {
    let d = dim.as_usize();
    let target_len = if out.len() < d { out.len() } else { d };
    if target_len == 0 || msg.is_empty() {
        return 0;
    }
    out[..target_len].fill(0);

    let mut token_start = None;
    for i in 0..=msg.len() {
        let is_sep = i == msg.len()
            || msg[i] == b' '
            || msg[i] == b'\t'
            || msg[i] == b'\n'
            || msg[i] == b','
            || msg[i] == b'.'
            || msg[i] == b'!'
            || msg[i] == b'?'
            || msg[i] == b':'
            || msg[i] == b';';
        if is_sep {
            if let Some(start) = token_start {
                let token = &msg[start..i];
                if !token.is_empty() {
                    let mut hash: u32 = 0x811c9dc5;
                    for &b in token {
                        hash ^= b.to_ascii_lowercase() as u32;
                        hash = hash.wrapping_mul(0x01000193);
                    }
                    let bin = (hash as usize) % target_len;
                    let sign = if (hash & 0x80) != 0 { 1i8 } else { -1i8 };
                    out[bin] = out[bin].saturating_add(sign.saturating_mul(24));
                }
                token_start = None;
            }
        } else if token_start.is_none() {
            token_start = Some(i);
        }
    }

    target_len
}

/// Evaluates an input message embedding vector against canonical threat centroids using
/// `MatryoshkaDim` dimensionality truncation and `cosine_similarity_q7`.
pub fn evaluate_embedding_threat(msg: &[u8], dim: MatryoshkaDim) -> SecurityBadge {
    if msg.is_empty() {
        return SecurityBadge::VerifiedSafe;
    }

    let d = dim.as_usize().min(128);
    let effective_dim = match MatryoshkaDim::from_u32(d as u32) {
        Some(m) => m,
        None => MatryoshkaDim::Dim128,
    };

    let mut msg_vec = [0i8; 128];
    project_text_embedding_q7(msg, effective_dim, &mut msg_vec);

    let mut phishing_ref = [0i8; 128];
    project_text_embedding_q7(
        b"verify password pin otp bank account transfer security credential login",
        effective_dim,
        &mut phishing_ref,
    );

    let mut urgency_ref = [0i8; 128];
    project_text_embedding_q7(
        b"urgent immediately action required suspend block notice alert terminate",
        effective_dim,
        &mut urgency_ref,
    );

    let sim_phish = cosine_similarity_q7(&msg_vec[..d], &phishing_ref[..d]);
    let sim_urgency = cosine_similarity_q7(&msg_vec[..d], &urgency_ref[..d]);

    // Q15 fixed-point threshold: 12000 is ~0.366 cosine similarity
    if sim_phish >= 12000 {
        SecurityBadge::PotentialPhishing
    } else if sim_urgency >= 12000 {
        SecurityBadge::SuspiciousUrgency
    } else {
        SecurityBadge::VerifiedSafe
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_gsm7_pack_and_unpack_roundtrip() {
        let original = b"hello world 123";
        let mut packed = [0u8; 32];
        let packed_len = pack_gsm7_septets(original, &mut packed);

        // 15 septets * 7 bits = 105 bits -> 14 octets
        assert_eq!(packed_len, 14);

        let mut unpacked = [0u8; 32];
        let unpacked_len = unpack_gsm7_septets(&packed[..packed_len], original.len(), &mut unpacked);

        assert_eq!(unpacked_len, original.len());
        assert_eq!(&unpacked[..unpacked_len], original);
    }

    #[test]
    fn test_gsm7_standard_8_character_packing() {
        // Exactly 8 characters (8 * 7 = 56 bits = 7 octets)
        let text = b"12345678";
        let mut packed = [0u8; 16];
        let packed_len = pack_gsm7_septets(text, &mut packed);
        assert_eq!(packed_len, 7);

        let mut unpacked = [0u8; 16];
        let unpacked_len = unpack_gsm7_septets(&packed[..packed_len], 8, &mut unpacked);
        assert_eq!(unpacked_len, 8);
        assert_eq!(&unpacked[..8], text);
    }

    #[test]
    fn test_heuristics_detect_phishing_and_urgency() {
        assert_eq!(
            inspect_message_heuristics(b"Please send your OTP to restore access"),
            SecurityBadge::PotentialPhishing
        );
        assert_eq!(
            inspect_message_heuristics(b"Action required: Account will be suspended"),
            SecurityBadge::SuspiciousUrgency
        );
        assert_eq!(
            inspect_message_heuristics(b"Meeting at 3pm in room B"),
            SecurityBadge::VerifiedSafe
        );
    }

    #[test]
    fn test_embedding_threat_evaluation() {
        assert_eq!(
            evaluate_embedding_threat(
                b"Please verify your login and password credential immediately",
                MatryoshkaDim::Dim128
            ),
            SecurityBadge::PotentialPhishing
        );
        assert_eq!(
            evaluate_embedding_threat(
                b"Action required: account alert suspend immediately",
                MatryoshkaDim::Dim128
            ),
            SecurityBadge::SuspiciousUrgency
        );
        assert_eq!(
            evaluate_embedding_threat(
                b"Hey are we still meeting for lunch tomorrow",
                MatryoshkaDim::Dim128
            ),
            SecurityBadge::VerifiedSafe
        );
    }
}
