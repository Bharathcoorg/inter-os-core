// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

//! Sovereign eSIM & Decentralized Subscriber Identity Subsystem
//!
//! Replaces legacy physical SIM cards and telco carrier tracking with
//! decentralized identity (DID) linked subscriber profiles.
//! Enforces zero-knowledge subscriber authentication over cellular networks.

use alloc::string::{String, ToString};
use crate::crypto::{HmacSha256, Sha256};

/// Mobile SIM hardware / profile state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SimStatus {
    NoSim,
    PinRequired,
    Ready,
    Blocked,
}

/// Mobile eSIM Profile.
pub struct SovereignEsim {
    pub subscriber_id: String,
    /// Carrier identity as provisioned by a real profile. Empty until one is.
    pub carrier_operator: &'static str,
    pub imsi_pseudonym: [u8; 16],
    /// Subscriber PIN. `None` until a profile supplies one, so there is no
    /// default credential any four digits would satisfy.
    pub pin_code: Option<[u8; 4]>,
    pub pin_verified: bool,
    pub status: SimStatus,
    /// Per-profile subscriber authentication key. `None` until provisioned;
    /// a key embedded in the image would be a shared secret for every device.
    pub subscriber_auth_key: Option<[u8; 32]>,
}

impl SovereignEsim {
    pub fn new(subscriber_id: &str) -> Self {
        let mut imsi = [0u8; 16];
        let hash = Sha256::digest(subscriber_id.as_bytes());
        imsi.copy_from_slice(&hash[0..16]);

        Self {
            subscriber_id: subscriber_id.to_string(),
            // No carrier, PIN or subscriber key is known until a profile is
            // provisioned into a secure element. Reporting a plausible carrier
            // name or a fixed PIN would be fabricated subscriber state.
            carrier_operator: "",
            imsi_pseudonym: imsi,
            pin_code: None,
            pin_verified: false,
            status: SimStatus::NoSim,
            subscriber_auth_key: None,
        }
    }

    /// Installs the credential material a provisioned profile carries.
    pub fn provision(
        &mut self,
        carrier: &'static str,
        pin: [u8; 4],
        auth_key: [u8; 32],
    ) {
        self.carrier_operator = carrier;
        self.pin_code = Some(pin);
        self.subscriber_auth_key = Some(auth_key);
    }

    /// Verifies the subscriber PIN code using constant-time comparison to prevent timing attacks.
    pub fn verify_pin(&mut self, pin: &[u8; 4]) -> bool {
        match self.pin_code.as_ref() {
            Some(expected) if crate::crypto::constant_time_eq_4(expected, pin) => {
                self.pin_verified = true;
                self.status = SimStatus::Ready;
                true
            }
            _ => false,
        }
    }

    /// Generates a zero-knowledge subscriber authentication token for 5G cellular bearer access.
    ///
    /// Returns `None` until the profile's subscriber key has been provisioned.
    pub fn generate_auth_token(&self, network_challenge: &[u8; 16]) -> Option<[u8; 32]> {
        let key = self.subscriber_auth_key.as_ref()?;
        let mut msg = [0u8; 32];
        msg[0..16].copy_from_slice(&self.imsi_pseudonym);
        msg[16..32].copy_from_slice(network_challenge);
        Some(HmacSha256::mac(key, &msg))
    }
}
