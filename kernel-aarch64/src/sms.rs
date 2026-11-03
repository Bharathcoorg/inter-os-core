// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

//! Autonomous Cellular SMS & Emergency PDU Dispatcher (AArch64)
//!
//! Provides bare-metal 3GPP TS 23.040 Short Message Service (SMS) Point-to-Point (PP)
//! PDU encoding/decoding, GSM 7-bit septet packing, BCD semi-octet address formatting,
//! 3GPP TS 23.041 Cell Broadcast Service (CMAS/ETWS) emergency alert dispatching,
//! and on-device anti-phishing/threat heuristic filtering.
//!
//! Enforces Zero Ambient Authority: Any SMS submission, inbox inspection, or emergency
//! broadcast subscription requires an unforgeable CapabilityToken (`CapabilityType::SmsDispatchDirect`).

extern crate alloc;
use alloc::string::String;
use alloc::vec::Vec;
use crate::capability::{CapabilityTable, CapabilityType, Rights};

/// 3GPP TS 23.040 Constants
pub const SMS_MAX_USER_DATA_CHARS: usize = 160;
pub const SMS_MAX_OCTETS: usize = 175;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SmsError {
    CapabilityDenied,
    InvalidPduLength,
    InvalidPhoneNumber,
    UserDataTooLong,
    InvalidDcs,
    CorruptedPdu,
    BufferOverflow,
    PhishingThreatBlocked,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EmergencySeverity {
    Presidential,
    ExtremeThreat,
    SevereThreat,
    AmberAlert,
    TestMessage,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EmergencyBroadcastAlert {
    pub message_id: u16,
    pub serial_number: u16,
    pub severity: EmergencySeverity,
    pub geographic_scope: u8,
    pub warning_text: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThreatLevel {
    Safe,
    Suspicious,
    HighPhishingRisk,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SmsSubmitPdu {
    pub destination_number: String,
    pub message_text: String,
    pub service_center: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SmsDeliverPdu {
    pub originating_number: String,
    pub message_text: String,
    pub timestamp_year: u8,
    pub timestamp_month: u8,
    pub timestamp_day: u8,
    pub timestamp_hour: u8,
    pub timestamp_minute: u8,
    pub timestamp_second: u8,
    pub threat_level: ThreatLevel,
    pub threat_score: u8,
}

/// Packs 7-bit ASCII characters into 8-bit octets according to 3GPP TS 23.040.
pub fn pack_7bit(text: &str) -> Vec<u8> {
    let bytes = text.as_bytes();
    let mut packed = Vec::new();
    let mut bit_pos = 0;
    let mut current_byte = 0u8;

    for &byte in bytes {
        let septet = byte & 0x7F;
        let shift = bit_pos % 8;

        current_byte |= (septet << shift) as u8;
        if shift > 0 {
            packed.push(current_byte);
            current_byte = (septet >> (8 - shift)) as u8;
        }

        bit_pos += 7;
        if bit_pos % 8 == 0 && bit_pos > 0 && shift == 1 {
            // Byte was finalized
        }
    }

    if bit_pos % 8 != 0 {
        packed.push(current_byte);
    }

    packed
}

/// Unpacks 8-bit octets into 7-bit ASCII characters according to 3GPP TS 23.040.
pub fn unpack_7bit(octets: &[u8], num_chars: usize) -> Result<String, SmsError> {
    let mut result = String::new();
    let mut bit_pos = 0;

    for _ in 0..num_chars {
        let byte_idx = bit_pos / 8;
        let shift = bit_pos % 8;

        if byte_idx >= octets.len() {
            break;
        }

        let mut septet = (octets[byte_idx] >> shift) & 0x7F;
        if shift > 1 && byte_idx + 1 < octets.len() {
            let next_bits = (octets[byte_idx + 1] << (8 - shift)) & 0x7F;
            septet |= next_bits;
        }

        let ascii_char = if septet >= 32 && septet <= 126 {
            septet as char
        } else if septet == 10 || septet == 13 {
            septet as char
        } else {
            '?'
        };

        result.push(ascii_char);
        bit_pos += 7;
    }

    Ok(result)
}

/// Encodes a telephone number into BCD semi-octet format.
/// Returns (length_in_digits, type_of_address, bcd_octets)
pub fn encode_address(phone_number: &str) -> Result<(u8, u8, Vec<u8>), SmsError> {
    let (is_intl, clean_num) = if let Some(stripped) = phone_number.strip_prefix('+') {
        (true, stripped)
    } else {
        (false, phone_number)
    };

    let digits: Vec<u8> = clean_num.bytes().filter(|&b| b.is_ascii_digit()).map(|b| b - b'0').collect();
    if digits.is_empty() {
        return Err(SmsError::InvalidPhoneNumber);
    }

    let toa: u8 = if is_intl { 0x91 } else { 0x81 }; // 0x91 = International (E.164), 0x81 = National
    let mut bcd_octets = Vec::new();

    let mut i = 0;
    while i < digits.len() {
        let d1 = digits[i];
        let d2 = if i + 1 < digits.len() { digits[i + 1] } else { 0x0F }; // Pad with 0xF
        let octet = (d2 << 4) | (d1 & 0x0F);
        bcd_octets.push(octet);
        i += 2;
    }

    Ok((digits.len() as u8, toa, bcd_octets))
}

/// Decodes BCD semi-octet phone number into standard string.
pub fn decode_address(num_digits: usize, toa: u8, octets: &[u8]) -> Result<String, SmsError> {
    let mut num = String::new();
    if toa == 0x91 {
        num.push('+');
    }

    let mut extracted = 0;
    for &octet in octets {
        let d1 = octet & 0x0F;
        if extracted < num_digits && d1 < 10 {
            num.push((b'0' + d1) as char);
            extracted += 1;
        }
        let d2 = (octet >> 4) & 0x0F;
        if extracted < num_digits && d2 < 10 {
            num.push((b'0' + d2) as char);
            extracted += 1;
        }
    }

    Ok(num)
}

/// In-kernel anti-phishing heuristic evaluator.
pub struct AntiPhishingFilter;

impl AntiPhishingFilter {
    pub fn assess_threat(message: &str) -> (ThreatLevel, u8) {
        let mut score: u8 = 0;
        let lower = message;

        // Check for urgency/banking impersonation keywords
        let suspicious_keywords = [
            "urgent", "locked", "bank", "verify", "password", "suspended",
            "wire transfer", "login immediately", "identity alert", "claim reward"
        ];

        for kw in suspicious_keywords {
            if contains_ignore_case(lower, kw) {
                score = score.saturating_add(25);
            }
        }

        // Check for suspicious URL patterns
        if lower.contains("http://") || lower.contains(".xyz") || lower.contains(".top") || lower.contains("bit.ly") {
            score = score.saturating_add(40);
        }

        let level = if score >= 60 {
            ThreatLevel::HighPhishingRisk
        } else if score >= 25 {
            ThreatLevel::Suspicious
        } else {
            ThreatLevel::Safe
        };

        (level, score)
    }
}

fn contains_ignore_case(haystack: &str, needle: &str) -> bool {
    let h_bytes = haystack.as_bytes();
    let n_bytes = needle.as_bytes();
    if n_bytes.is_empty() || h_bytes.len() < n_bytes.len() {
        return false;
    }

    for i in 0..=h_bytes.len() - n_bytes.len() {
        let mut matched = true;
        for j in 0..n_bytes.len() {
            let h = h_bytes[i + j].to_ascii_lowercase();
            let n = n_bytes[j].to_ascii_lowercase();
            if h != n {
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

/// Sovereign SMS and Emergency PDU Dispatcher
pub struct SmsDispatcher {
    pub outbound_queue: Vec<SmsSubmitPdu>,
    pub inbound_inbox: Vec<SmsDeliverPdu>,
    pub emergency_alerts: Vec<EmergencyBroadcastAlert>,
    pub total_messages_sent: u64,
    pub total_messages_received: u64,
    pub total_emergency_alerts: u64,
    pub total_phishing_blocked: u64,
}

impl SmsDispatcher {
    pub const fn new() -> Self {
        Self {
            outbound_queue: Vec::new(),
            inbound_inbox: Vec::new(),
            emergency_alerts: Vec::new(),
            total_messages_sent: 0,
            total_messages_received: 0,
            total_emergency_alerts: 0,
            total_phishing_blocked: 0,
        }
    }

    /// Composes and serializes an SMS-SUBMIT PDU into binary octets for baseband AT+CMGS.
    pub fn compose_submit_pdu(
        &mut self,
        destination: &str,
        message: &str,
        caps: &CapabilityTable,
    ) -> Result<(Vec<u8>, usize), SmsError> {
        // Enforce Zero Ambient Authority
        caps.check(CapabilityType::SmsDispatchDirect, Rights::WRITE)
            .map_err(|_| SmsError::CapabilityDenied)?;

        if message.len() > SMS_MAX_USER_DATA_CHARS {
            return Err(SmsError::UserDataTooLong);
        }

        let (digit_count, toa, addr_octets) = encode_address(destination)?;
        let packed_ud = pack_7bit(message);

        let mut pdu = Vec::new();
        // 1. SMSC info length = 0 (use baseband default SMSC)
        pdu.push(0x00);
        // 2. First octet of SMS-SUBMIT: TP-MTI = 01 (SMS-SUBMIT), TP-VPF = 00 (no VP)
        pdu.push(0x01);
        // 3. TP-MR (Message Reference) = 00
        pdu.push(0x00);
        // 4. TP-DA Destination Address:
        pdu.push(digit_count);
        pdu.push(toa);
        pdu.extend_from_slice(&addr_octets);
        // 5. TP-PID (Protocol Identifier) = 00 (Default)
        pdu.push(0x00);
        // 6. TP-DCS (Data Coding Scheme) = 00 (GSM 7-bit default alphabet)
        pdu.push(0x00);
        // 7. TP-UDL (User Data Length in septets/characters)
        pdu.push(message.len() as u8);
        // 8. TP-UD (Packed User Data)
        pdu.extend_from_slice(&packed_ud);

        let submit_struct = SmsSubmitPdu {
            destination_number: String::from(destination),
            message_text: String::from(message),
            service_center: None,
        };

        self.outbound_queue.push(submit_struct);
        self.total_messages_sent += 1;

        // Baseband length parameter for AT+CMGS is PDU length excluding SMSC length byte
        let cmgs_len = pdu.len() - 1;
        Ok((pdu, cmgs_len))
    }

    /// Ingests and decodes an incoming raw SMS-DELIVER PDU from baseband modem.
    pub fn ingest_deliver_pdu(
        &mut self,
        pdu: &[u8],
        caps: &CapabilityTable,
    ) -> Result<SmsDeliverPdu, SmsError> {
        // Enforce Zero Ambient Authority
        caps.check(CapabilityType::SmsDispatchDirect, Rights::READ)
            .map_err(|_| SmsError::CapabilityDenied)?;

        if pdu.len() < 12 {
            return Err(SmsError::InvalidPduLength);
        }

        let mut offset = 0;
        // 1. SMSC Length
        let smsc_len = pdu[offset] as usize;
        offset += 1 + smsc_len;
        if offset >= pdu.len() {
            return Err(SmsError::CorruptedPdu);
        }

        // 2. First octet of SMS-DELIVER: TP-MTI lower 2 bits must be 00
        let first_octet = pdu[offset];
        offset += 1;
        if (first_octet & 0x03) != 0x00 {
            return Err(SmsError::CorruptedPdu);
        }

        // 3. TP-OA Originating Address
        let digit_count = pdu[offset] as usize;
        let toa = pdu[offset + 1];
        offset += 2;
        let bcd_len = (digit_count + 1) / 2;
        if offset + bcd_len > pdu.len() {
            return Err(SmsError::CorruptedPdu);
        }
        let originating_number = decode_address(digit_count, toa, &pdu[offset..offset + bcd_len])?;
        offset += bcd_len;

        // 4. TP-PID
        offset += 1;
        // 5. TP-DCS
        let dcs = pdu[offset];
        offset += 1;
        if dcs != 0x00 {
            // Non-GSM 7-bit not supported in basic parser
            return Err(SmsError::InvalidDcs);
        }

        // 6. TP-SCTS (Service Center Time Stamp) - 7 bytes semi-octet BCD
        if offset + 7 > pdu.len() {
            return Err(SmsError::CorruptedPdu);
        }
        let scts = &pdu[offset..offset + 7];
        let parse_bcd = |b: u8| ((b & 0x0F) * 10) + ((b >> 4) & 0x0F);
        let year = parse_bcd(scts[0]);
        let month = parse_bcd(scts[1]);
        let day = parse_bcd(scts[2]);
        let hour = parse_bcd(scts[3]);
        let minute = parse_bcd(scts[4]);
        let second = parse_bcd(scts[5]);
        offset += 7;

        // 7. TP-UDL
        if offset >= pdu.len() {
            return Err(SmsError::CorruptedPdu);
        }
        let udl = pdu[offset] as usize;
        offset += 1;

        // 8. TP-UD (Unpack 7-bit user data)
        let message_text = unpack_7bit(&pdu[offset..], udl)?;

        // Run Anti-Phishing Security Threat Assessment
        let (threat_level, threat_score) = AntiPhishingFilter::assess_threat(&message_text);
        if threat_level == ThreatLevel::HighPhishingRisk {
            self.total_phishing_blocked += 1;
        }

        let deliver = SmsDeliverPdu {
            originating_number,
            message_text,
            timestamp_year: year,
            timestamp_month: month,
            timestamp_day: day,
            timestamp_hour: hour,
            timestamp_minute: minute,
            timestamp_second: second,
            threat_level,
            threat_score,
        };

        self.inbound_inbox.push(deliver.clone());
        self.total_messages_received += 1;

        Ok(deliver)
    }

    /// Dispatches an Emergency Cell Broadcast Alert (CMAS / ETWS) with system priority.
    pub fn dispatch_emergency_broadcast(
        &mut self,
        message_id: u16,
        serial_number: u16,
        severity: EmergencySeverity,
        geographic_scope: u8,
        warning_text: &str,
        caps: &CapabilityTable,
    ) -> Result<EmergencyBroadcastAlert, SmsError> {
        // Enforce Zero Ambient Authority (requires INVOKE rights)
        caps.check(CapabilityType::SmsDispatchDirect, Rights::INVOKE)
            .map_err(|_| SmsError::CapabilityDenied)?;

        let alert = EmergencyBroadcastAlert {
            message_id,
            serial_number,
            severity,
            geographic_scope,
            warning_text: String::from(warning_text),
        };

        self.emergency_alerts.push(alert.clone());
        self.total_emergency_alerts += 1;

        Ok(alert)
    }
}
