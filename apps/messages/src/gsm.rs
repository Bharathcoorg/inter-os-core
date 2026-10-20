//! Deterministic 3GPP TS 23.038 GSM 7-bit Default Alphabet PDU packer
//! and on-device privacy/phishing heuristic analyzer for InterOS Messages.

use interos_std::ai::{cosine_similarity_q7, MatryoshkaDim};

/// Standard GSM 03.38 / 23.040 SMS character bounds.
pub const SINGLE_SMS_MAX_CHARS: usize = 160;
pub const MULTIPART_SMS_SEGMENT_MAX_CHARS: usize = 153;

/// Computes the number of SMS concatenated parts required for a given character length.
pub const fn sms_parts_required(char_len: usize) -> usize {
    if char_len == 0 {
        0
    } else if char_len <= SINGLE_SMS_MAX_CHARS {
        1
    } else {
        (char_len + MULTIPART_SMS_SEGMENT_MAX_CHARS - 1) / MULTIPART_SMS_SEGMENT_MAX_CHARS
    }
}

/// Returns true if a text of length `char_len` exceeds a single SMS boundary.
pub const fn is_multipart_sms(char_len: usize) -> bool {
    char_len > SINGLE_SMS_MAX_CHARS
}

/// Threat severity level evaluated by the local offline heuristic engine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThreatLevel {
    /// Normal message with no detected urgency, phishing, or suspicious links.
    Safe,
    /// High-urgency trigger words or unverified external links detected.
    Suspicious,
    /// Credential theft, bank transfer, suspicious IP address or high-risk phishing detected.
    HighRisk,
}

impl ThreatLevel {
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Safe => "VERIFIED CLEAN",
            Self::Suspicious => "SUSPICIOUS URGENCY",
            Self::HighRisk => "POTENTIAL PHISHING",
        }
    }

    pub const fn color(&self) -> u32 {
        match self {
            Self::Safe => 0xFF2E8B57,       // Emerald Green
            Self::Suspicious => 0xFFF59E0B, // Amber Warning
            Self::HighRisk => 0xFFEF4444,   // Crimson Threat
        }
    }
}

/// Reason flags identified during offline threat analysis.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ThreatFlags {
    pub phishing_keyword: bool,
    pub urgency_social_eng: bool,
    pub suspicious_link: bool,
    pub ip_address: bool,
}

/// Full threat assessment outcome.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ThreatAssessment {
    pub level: ThreatLevel,
    pub flags: ThreatFlags,
}

/// Legacy / badge representation for compatibility with earlier UI hooks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SecurityBadge {
    VerifiedSafe,
    SuspiciousUrgency,
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
            Self::VerifiedSafe => 0xFF2E8B57,
            Self::SuspiciousUrgency => 0xFFF59E0B,
            Self::PotentialPhishing => 0xFFEF4444,
        }
    }
}

impl From<ThreatLevel> for SecurityBadge {
    fn from(level: ThreatLevel) -> Self {
        match level {
            ThreatLevel::Safe => SecurityBadge::VerifiedSafe,
            ThreatLevel::Suspicious => SecurityBadge::SuspiciousUrgency,
            ThreatLevel::HighRisk => SecurityBadge::PotentialPhishing,
        }
    }
}

impl From<SecurityBadge> for ThreatLevel {
    fn from(badge: SecurityBadge) -> Self {
        match badge {
            SecurityBadge::VerifiedSafe => ThreatLevel::Safe,
            SecurityBadge::SuspiciousUrgency => ThreatLevel::Suspicious,
            SecurityBadge::PotentialPhishing => ThreatLevel::HighRisk,
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

/// Case-insensitive substring matching helper without heap allocation.
fn contains_ci(msg: &[u8], needle: &[u8]) -> bool {
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
}

/// Checks if the message contains an IPv4 address pattern (e.g. "192.168.1.1").
fn contains_ipv4_address(msg: &[u8]) -> bool {
    if msg.len() < 7 {
        return false;
    }
    for i in 0..msg.len().saturating_sub(6) {
        if i > 0 && msg[i - 1].is_ascii_digit() {
            continue;
        }
        let mut idx = i;
        let mut octets = 0;
        let mut valid = true;
        while octets < 4 && idx < msg.len() {
            let start_num = idx;
            let mut val = 0u32;
            while idx < msg.len() && msg[idx].is_ascii_digit() {
                val = val * 10 + (msg[idx] - b'0') as u32;
                idx += 1;
            }
            let digits = idx - start_num;
            if digits == 0 || digits > 3 || val > 255 {
                valid = false;
                break;
            }
            octets += 1;
            if octets < 4 {
                if idx < msg.len() && msg[idx] == b'.' {
                    idx += 1;
                } else {
                    valid = false;
                    break;
                }
            }
        }
        if valid && octets == 4 {
            if idx == msg.len() || !msg[idx].is_ascii_digit() {
                return true;
            }
        }
    }
    false
}

/// High-risk phishing and credential theft patterns.
const PHISHING_PATTERNS: [&[u8]; 14] = [
    b"account suspended",
    b"verify password",
    b"urgent wire",
    b"bank transfer",
    b"login immediately",
    b"password",
    b"passcode",
    b"pin code",
    b"otp",
    b"verify account",
    b"wire transfer",
    b"bank account",
    b"wallet seed",
    b"credit card",
];

/// Artificial urgency and social engineering triggers.
const URGENCY_PATTERNS: [&[u8]; 9] = [
    b"urgent",
    b"immediately",
    b"action required",
    b"suspended",
    b"blocked",
    b"final notice",
    b"act now",
    b"immediate action",
    b"account locked",
];

/// Suspicious external links and URL shorteners.
const SUSPICIOUS_LINK_PATTERNS: [&[u8]; 8] = [
    b"http://",
    b"https://",
    b"bit.ly/",
    b"tinyurl.com/",
    b"t.co/",
    b".xyz/",
    b".top/",
    b"www.",
];

/// Comprehensive offline threat analysis evaluating urgent social engineering,
/// phishing keywords, external URL links, and raw IP addresses.
///
/// Operates with `#![no_std]`, zero allocation, and bounded execution time.
pub fn analyze_threat(msg: &[u8]) -> ThreatAssessment {
    let mut flags = ThreatFlags::default();
    if msg.is_empty() {
        return ThreatAssessment {
            level: ThreatLevel::Safe,
            flags,
        };
    }

    // 1. Phishing keywords
    for pattern in PHISHING_PATTERNS.iter() {
        if contains_ci(msg, pattern) {
            flags.phishing_keyword = true;
            break;
        }
    }

    // 2. Urgent social engineering triggers
    for pattern in URGENCY_PATTERNS.iter() {
        if contains_ci(msg, pattern) {
            flags.urgency_social_eng = true;
            break;
        }
    }

    // 3. Suspicious external URL links
    for pattern in SUSPICIOUS_LINK_PATTERNS.iter() {
        if contains_ci(msg, pattern) {
            flags.suspicious_link = true;
            break;
        }
    }

    // 4. Raw IPv4 addresses
    if contains_ipv4_address(msg) {
        flags.ip_address = true;
    }

    // Determine threat level:
    // HighRisk: Explicit phishing keywords, or IP address, or link combined with urgency
    if flags.phishing_keyword || flags.ip_address || (flags.suspicious_link && flags.urgency_social_eng) {
        return ThreatAssessment {
            level: ThreatLevel::HighRisk,
            flags,
        };
    }

    // Suspicious: Urgency or suspicious link present
    if flags.urgency_social_eng || flags.suspicious_link {
        return ThreatAssessment {
            level: ThreatLevel::Suspicious,
            flags,
        };
    }

    // Secondary semantic layer: evaluate text embedding projection against threat centroids
    let sem_badge = evaluate_embedding_threat(msg, MatryoshkaDim::Dim128);
    let level = match sem_badge {
        SecurityBadge::PotentialPhishing => {
            flags.phishing_keyword = true;
            ThreatLevel::HighRisk
        }
        SecurityBadge::SuspiciousUrgency => {
            flags.urgency_social_eng = true;
            ThreatLevel::Suspicious
        }
        SecurityBadge::VerifiedSafe => ThreatLevel::Safe,
    };

    ThreatAssessment { level, flags }
}

/// Evaluates a raw message text buffer for phishing or deceptive urgency triggers.
/// Operates with `#![no_std]`, zero allocation, and bounded execution time.
pub fn inspect_message_heuristics(msg: &[u8]) -> SecurityBadge {
    analyze_threat(msg).level.into()
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
    fn test_gsm7_alphabet_punctuation_roundtrip() {
        let alphabet = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ abcdefghijklmnopqrstuvwxyz 0123456789 .,!?:;@#";
        let mut packed = [0u8; 128];
        let packed_len = pack_gsm7_septets(alphabet, &mut packed);

        let mut unpacked = [0u8; 128];
        let unpacked_len = unpack_gsm7_septets(&packed[..packed_len], alphabet.len(), &mut unpacked);
        assert_eq!(unpacked_len, alphabet.len());
        assert_eq!(&unpacked[..unpacked_len], alphabet);
    }

    #[test]
    fn test_gsm7_multipart_length_boundaries() {
        assert_eq!(sms_parts_required(0), 0);
        assert_eq!(sms_parts_required(1), 1);
        assert_eq!(sms_parts_required(160), 1);
        assert!(!is_multipart_sms(160));

        // 161 chars requires 2 parts
        assert_eq!(sms_parts_required(161), 2);
        assert!(is_multipart_sms(161));

        // 153 * 2 = 306 chars requires 2 parts
        assert_eq!(sms_parts_required(306), 2);
        // 307 chars requires 3 parts
        assert_eq!(sms_parts_required(307), 3);
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
    fn test_threat_analysis_specific_flags() {
        // Phishing keywords
        let t1 = analyze_threat(b"Account suspended: please verify password immediately");
        assert_eq!(t1.level, ThreatLevel::HighRisk);
        assert!(t1.flags.phishing_keyword);
        assert!(t1.flags.urgency_social_eng);

        // Bank transfer / urgent wire
        let t2 = analyze_threat(b"Urgent wire needed for bank transfer");
        assert_eq!(t2.level, ThreatLevel::HighRisk);
        assert!(t2.flags.phishing_keyword);

        // Suspicious IP address
        let t3 = analyze_threat(b"Login at http://192.168.1.100 now");
        assert_eq!(t3.level, ThreatLevel::HighRisk);
        assert!(t3.flags.ip_address);

        // Suspicious external link with urgency
        let t4 = analyze_threat(b"Urgent action: visit bit.ly/test3");
        assert_eq!(t4.level, ThreatLevel::HighRisk);
        assert!(t4.flags.suspicious_link);
        assert!(t4.flags.urgency_social_eng);

        // Suspicious external link alone
        let t5 = analyze_threat(b"Check out https://example.com for photos");
        assert_eq!(t5.level, ThreatLevel::Suspicious);
        assert!(t5.flags.suspicious_link);

        // Legitimate clean message
        let t6 = analyze_threat(b"Hey Sarah, are we still meeting for lunch tomorrow?");
        assert_eq!(t6.level, ThreatLevel::Safe);
        assert!(!t6.flags.phishing_keyword);
        assert!(!t6.flags.urgency_social_eng);
        assert!(!t6.flags.suspicious_link);
        assert!(!t6.flags.ip_address);
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
