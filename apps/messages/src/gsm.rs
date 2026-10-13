//! Deterministic 3GPP TS 23.038 GSM 7-bit Default Alphabet PDU packer
//! and on-device privacy/phishing heuristic analyzer for InterOS Messages.

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
            carry = val >> shift;
            shift += 1;
            if shift == 8 {
                if out_idx < out.len() {
                    out[out_idx] = carry;
                    out_idx += 1;
                }
                shift = 0;
                carry = 0;
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

    SecurityBadge::VerifiedSafe
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
}
