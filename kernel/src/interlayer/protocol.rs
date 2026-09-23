//! InterLayer Zero-Trust Packet Protocol
//!
//! Complies with Master Prompt Non-Negotiable Principle #14:
//! "InterLayer is infrastructure, not a requirement for the OS to boot or function offline."
//! Encapsulates cryptographic DID challenge-response, state delta relays, and intent tunneling.

use crate::crypto::hmac::HmacSha256;

pub const INTERLAYER_MAGIC: [u8; 4] = [b'I', b'L', b'A', b'Y'];
pub const INTERLAYER_VERSION: u8 = 0x01;
pub const INTERLAYER_HEADER_LEN: usize = 58;

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum PacketType {
    HandshakeChallenge = 1,
    HandshakeResponse = 2,
    StateDeltaSync = 3,
    AttestedIntentRelay = 4,
    MarketplaceAnnounce = 5,
    PackageRequest = 6,
    PackageResponse = 7,
    Unknown(u8),
}

impl From<u8> for PacketType {
    fn from(v: u8) -> Self {
        match v {
            1 => PacketType::HandshakeChallenge,
            2 => PacketType::HandshakeResponse,
            3 => PacketType::StateDeltaSync,
            4 => PacketType::AttestedIntentRelay,
            5 => PacketType::MarketplaceAnnounce,
            6 => PacketType::PackageRequest,
            7 => PacketType::PackageResponse,
            other => PacketType::Unknown(other),
        }
    }
}

/// Canonical InterLayer Packet View
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InterlayerPacket<'a> {
    pub packet_type: PacketType,
    pub sequence: u32,
    pub sender_fingerprint: [u8; 16],
    pub signature: [u8; 32],
    pub payload: &'a [u8],
}

impl<'a> InterlayerPacket<'a> {
    /// Parses an InterLayer protocol packet from a raw buffer.
    pub fn parse(buf: &'a [u8]) -> Result<Self, &'static str> {
        if buf.len() < INTERLAYER_HEADER_LEN {
            return Err("Buffer too small for InterLayer header");
        }

        if buf[0..4] != INTERLAYER_MAGIC {
            return Err("Invalid InterLayer magic bytes");
        }

        if buf[4] != INTERLAYER_VERSION {
            return Err("Unsupported InterLayer protocol version");
        }

        let packet_type = PacketType::from(buf[5]);
        let sequence = u32::from_be_bytes([buf[6], buf[7], buf[8], buf[9]]);

        let mut sender_fingerprint = [0u8; 16];
        sender_fingerprint.copy_from_slice(&buf[10..26]);

        let mut signature = [0u8; 32];
        signature.copy_from_slice(&buf[26..58]);

        let payload = &buf[INTERLAYER_HEADER_LEN..];

        Ok(InterlayerPacket {
            packet_type,
            sequence,
            sender_fingerprint,
            signature,
            payload,
        })
    }

    /// Serializes and signs an InterLayer packet with the device's master secret using HMAC-SHA256.
    pub fn serialize_signed(
        packet_type: PacketType,
        sequence: u32,
        sender_fingerprint: [u8; 16],
        payload: &[u8],
        secret_key: &[u8],
        out: &mut [u8],
    ) -> Result<usize, &'static str> {
        let total_len = INTERLAYER_HEADER_LEN + payload.len();
        if out.len() < total_len {
            return Err("Output buffer too small for InterLayer packet");
        }

        out[0..4].copy_from_slice(&INTERLAYER_MAGIC);
        out[4] = INTERLAYER_VERSION;
        out[5] = match packet_type {
            PacketType::HandshakeChallenge => 1,
            PacketType::HandshakeResponse => 2,
            PacketType::StateDeltaSync => 3,
            PacketType::AttestedIntentRelay => 4,
            PacketType::MarketplaceAnnounce => 5,
            PacketType::PackageRequest => 6,
            PacketType::PackageResponse => 7,
            PacketType::Unknown(v) => v,
        };
        out[6..10].copy_from_slice(&sequence.to_be_bytes());
        out[10..26].copy_from_slice(&sender_fingerprint);

        // Copy payload before signing
        out[INTERLAYER_HEADER_LEN..total_len].copy_from_slice(payload);

        // Compute HMAC-SHA256 signature over (header[0..26] + payload)
        out[26..58].fill(0);

        let mut sign_data = [0u8; 512];
        let sign_len = 26 + payload.len();
        if sign_len <= sign_data.len() {
            sign_data[0..26].copy_from_slice(&out[0..26]);
            sign_data[26..sign_len].copy_from_slice(payload);
            let hmac = HmacSha256::mac(secret_key, &sign_data[0..sign_len]);
            out[26..58].copy_from_slice(&hmac);
        } else {
            return Err("Payload exceeds signing buffer size");
        }

        Ok(total_len)
    }

    /// Verifies the HMAC-SHA256 authenticity of the packet using a shared secret or public key proof.
    pub fn verify_signature(&self, secret_key: &[u8]) -> bool {
        let mut sign_data = [0u8; 512];
        let sign_len = 26 + self.payload.len();
        if sign_len > sign_data.len() {
            return false;
        }

        sign_data[0..4].copy_from_slice(&INTERLAYER_MAGIC);
        sign_data[4] = INTERLAYER_VERSION;
        sign_data[5] = match self.packet_type {
            PacketType::HandshakeChallenge => 1,
            PacketType::HandshakeResponse => 2,
            PacketType::StateDeltaSync => 3,
            PacketType::AttestedIntentRelay => 4,
            PacketType::MarketplaceAnnounce => 5,
            PacketType::PackageRequest => 6,
            PacketType::PackageResponse => 7,
            PacketType::Unknown(v) => v,
        };
        sign_data[6..10].copy_from_slice(&self.sequence.to_be_bytes());
        sign_data[10..26].copy_from_slice(&self.sender_fingerprint);
        sign_data[26..sign_len].copy_from_slice(self.payload);

        let expected_hmac = HmacSha256::mac(secret_key, &sign_data[0..sign_len]);
        expected_hmac == self.signature
    }
}
