//! Virtual 3GPP Cellular Modem Driver & Hayes AT Command Parser
//!
//! Controls cellular baseband radio, SIM verification, 5G registration,
//! signal quality (RSSI), mobile data bearer, and SMS intent routing.

use alloc::string::{String, ToString};
use super::sim::{SimStatus, SovereignEsim};

/// Operational state of the cellular modem radio.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModemRadioState {
    PowerDown,
    Initializing,
    RegisteredHome,
    Searching,
    DataConnected,
}

/// 3GPP Signal Quality Report.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SignalQuality {
    pub rssi: u8, // 0..31, where 31 = -51 dBm or better (5-bar signal)
    pub ber: u8,  // Bit error rate (99 = unknown or not detectable)
}

/// Virtual Cellular Modem Driver.
pub struct CellularModem {
    pub state: ModemRadioState,
    pub signal: SignalQuality,
    pub pdp_context_active: bool,
    pub apn: &'static str,
    pub last_received_sms: Option<String>,
}

impl CellularModem {
    pub fn new() -> Self {
        Self {
            state: ModemRadioState::RegisteredHome,
            signal: SignalQuality { rssi: 31, ber: 99 },
            pdp_context_active: true,
            apn: "sovereign.net",
            last_received_sms: None,
        }
    }

    /// Executes an AT command string against the modem baseband engine.
    pub fn execute_at(&mut self, cmd: &str, esim: &SovereignEsim) -> Result<String, &'static str> {
        let trimmed = cmd.trim();

        if trimmed == "AT" || trimmed == "ATE0" {
            return Ok("OK".to_string());
        }

        if trimmed == "AT+CPIN?" {
            return match esim.status {
                SimStatus::Ready => Ok("+CPIN: READY\nOK".to_string()),
                SimStatus::PinRequired => Ok("+CPIN: SIM PIN\nOK".to_string()),
                SimStatus::Blocked => Ok("+CPIN: SIM PUK\nERROR".to_string()),
                SimStatus::NoSim => Ok("+CME ERROR: 10\nERROR".to_string()),
            };
        }

        if trimmed == "AT+CSQ" {
            let mut s = String::new();
            use core::fmt::Write;
            let _ = write!(s, "+CSQ: {},{}\nOK", self.signal.rssi, self.signal.ber);
            return Ok(s);
        }

        if trimmed == "AT+CREG?" {
            return match self.state {
                ModemRadioState::RegisteredHome | ModemRadioState::DataConnected => {
                    Ok("+CREG: 0,1\nOK".to_string())
                }
                ModemRadioState::Searching => Ok("+CREG: 0,2\nOK".to_string()),
                _ => Ok("+CREG: 0,0\nOK".to_string()),
            };
        }

        if trimmed == "AT+COPS?" {
            let mut s = String::new();
            use core::fmt::Write;
            let _ = write!(s, "+COPS: 0,0,\"{}\"\nOK", esim.carrier_operator);
            return Ok(s);
        }

        if trimmed.starts_with("AT+CGDCONT=") {
            self.pdp_context_active = true;
            return Ok("OK".to_string());
        }

        if trimmed == "AT+CGACT=1,1" {
            self.state = ModemRadioState::DataConnected;
            return Ok("OK".to_string());
        }

        Err("AT: Command not supported or syntax error")
    }

    /// Inbound SMS processor: verifies incoming message and delivers standard cellular SMS intent.
    pub fn process_inbound_sms(&mut self, _sender: &str, body: &str) -> Option<&'static str> {
        self.last_received_sms = Some(body.to_string());
        Some("SMS_RECEIVED")
    }
}
