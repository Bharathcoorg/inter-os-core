//! 3GPP Cellular Modem Driver & Hayes AT Command Parser
//!
//! Controls cellular baseband radio, SIM verification, 5G/LTE registration,
//! signal quality (RSSI), voice calling state machine, SMS transmission,
//! and mobile data (PDP context) routing.

use alloc::string::{String, ToString};
use alloc::vec::Vec;
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

impl SignalQuality {
    /// Converts raw 3GPP RSSI (0..31) to approximate dBm.
    pub fn to_dbm(&self) -> i32 {
        if self.rssi == 99 {
            -113 // undetectable
        } else {
            -113 + (self.rssi as i32) * 2
        }
    }

/// Converts raw RSSI into signal bar count (0..5).
///
/// 3GPP reports 99 for "not detectable". That is the weakest possible reading,
/// not the strongest, so it must not fall through to a full bar count: with no
/// modem bound the honest answer is no bars at all.
pub fn to_bars(&self) -> u8 {
    match self.rssi {
        0..=4 => 0,
        5..=9 => 1,
        10..=14 => 2,
        15..=19 => 3,
        20..=25 => 4,
        26..=31 => 5,
        // Unknown, out of range, or explicitly "not detectable".
        _ => 0,
    }
}
}

/// Active voice call state machine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CallState {
    Idle,
    Dialing { destination: String, start_ticks: u64 },
    Alerting { destination: String, start_ticks: u64 },
    Active { number: String, start_ticks: u64 },
    Incoming { caller_id: String },
    Disconnected,
}

/// Cellular SMS message record.
#[derive(Debug, Clone)]
pub struct SmsRecord {
    pub id: u32,
    pub address: String,
    pub body: String,
    pub timestamp_ticks: u64,
    pub is_inbound: bool,
}

/// Production 3GPP Cellular Modem Controller.
pub const MAX_PHONE_NUMBER_LEN: usize = 32;
pub const MAX_SMS_BODY_LEN: usize = 1024;
pub const MAX_SMS_RECORDS: usize = 32;

pub struct CellularModem {
    /// True only when a concrete modem backend has registered with this controller.
    /// Software state alone must never make the platform report a cellular modem.
    pub hardware_present: bool,
    pub state: ModemRadioState,
    pub call_state: CallState,
    pub signal: SignalQuality,
    pub pdp_context_active: bool,
    pub apn: &'static str,
    pub ip_address: [u8; 4],
    pub sms_inbox: Vec<SmsRecord>,
    pub sms_outbox: Vec<SmsRecord>,
    pub next_sms_id: u32,
}

impl CellularModem {
    pub fn new() -> Self {
        // No modem is attached. Signal is reported as 3GPP "not detectable" so it
        // maps to zero bars, and there is no active data context or address.
        // Reporting a carrier, a strong RSSI or a live PDP context here would be
        // fabricated radio state.
        Self {
            hardware_present: false,
            state: ModemRadioState::PowerDown,
            call_state: CallState::Idle,
            signal: SignalQuality { rssi: 99, ber: 99 },
            pdp_context_active: false,
            apn: "",
            ip_address: [0; 4],
            sms_inbox: Vec::new(),
            sms_outbox: Vec::new(),
            next_sms_id: 1,
        }
    }

    /// Marks the controller online after a concrete modem backend has completed
    /// hardware discovery and registration.
    pub fn set_hardware_present(&mut self, present: bool) {
        self.hardware_present = present;
        if !present {
            self.state = ModemRadioState::PowerDown;
            self.pdp_context_active = false;
            self.call_state = CallState::Idle;
        }
    }

    /// Initiates an outgoing cellular voice call (`ATD<number>;`).
    pub fn dial(&mut self, number: &str, current_ticks: u64) -> Result<(), &'static str> {
        if !self.hardware_present {
            return Err("cellular modem backend unavailable");
        }
        if number.is_empty() || number.len() > MAX_PHONE_NUMBER_LEN { return Err("invalid phone number length"); }
        if self.state == ModemRadioState::PowerDown {
            return Err("Cellular radio is powered down");
        }
        if self.call_state != CallState::Idle && self.call_state != CallState::Disconnected {
            return Err("Line busy: active call in progress");
        }
        self.call_state = CallState::Dialing {
            destination: number.to_string(),
            start_ticks: current_ticks,
        };
        Ok(())
    }

    /// Answers an incoming voice call (`ATA`).
    pub fn answer(&mut self, current_ticks: u64) -> Result<(), &'static str> {
        if !self.hardware_present {
            return Err("cellular modem backend unavailable");
        }
        match &self.call_state {
            CallState::Incoming { caller_id } => {
                self.call_state = CallState::Active {
                    number: caller_id.clone(),
                    start_ticks: current_ticks,
                };
                Ok(())
            }
            _ => Err("No incoming call to answer"),
        }
    }

    /// Terminates or rejects an active voice call (`ATH`).
    pub fn hangup(&mut self) -> Result<(), &'static str> {
        if !self.hardware_present {
            return Err("cellular modem backend unavailable");
        }
        if self.call_state == CallState::Idle {
            return Ok(());
        }
        self.call_state = CallState::Disconnected;
        Ok(())
    }

    /// Calculates active call duration in seconds.
    pub fn get_call_duration(&self, current_ticks: u64) -> u64 {
        match &self.call_state {
            CallState::Active { start_ticks, .. } => {
                if current_ticks >= *start_ticks {
                    (current_ticks - *start_ticks) / 100 // 100 Hz system tick rate
                } else {
                    0
                }
            }
            _ => 0,
        }
    }

    /// Transmits an SMS message to a destination phone number (`AT+CMGS`).
    pub fn send_sms(&mut self, destination: &str, body: &str, current_ticks: u64) -> Result<u32, &'static str> {
        if !self.hardware_present {
            return Err("cellular modem backend unavailable");
        }
        if self.state == ModemRadioState::PowerDown {
            return Err("Cellular radio powered down");
        }
        if destination.is_empty() || destination.len() > MAX_PHONE_NUMBER_LEN || body.len() > MAX_SMS_BODY_LEN { return Err("SMS payload exceeds bound"); }
        if self.sms_outbox.len() >= MAX_SMS_RECORDS { return Err("SMS outbox full"); }
        let id = self.next_sms_id;
        self.next_sms_id = self.next_sms_id.wrapping_add(1).max(1);

        let record = SmsRecord {
            id,
            address: destination.to_string(),
            body: body.to_string(),
            timestamp_ticks: current_ticks,
            is_inbound: false,
        };
        self.sms_outbox.push(record);
        Ok(id)
    }

    /// Toggles cellular packet data (PDP Context Attach / Detach).
    pub fn set_data_enabled(&mut self, enabled: bool) -> bool {
        if !self.hardware_present {
            self.pdp_context_active = false;
            return false;
        }
        self.pdp_context_active = enabled;
        if enabled {
            if self.state == ModemRadioState::RegisteredHome {
                self.state = ModemRadioState::DataConnected;
            }
        } else if self.state == ModemRadioState::DataConnected {
            self.state = ModemRadioState::RegisteredHome;
        }
        self.pdp_context_active
    }

    /// Executes an AT command string against the modem baseband engine.
    pub fn execute_at(&mut self, cmd: &str, esim: &SovereignEsim, current_ticks: u64) -> Result<String, &'static str> {
        let trimmed = cmd.trim();

        if !self.hardware_present {
            return Err("cellular modem backend unavailable");
        }

        if trimmed == "AT" || trimmed == "ATE0" {
            return Ok("OK".to_string());
        }

        // Voice call commands
        if trimmed.starts_with("ATD") && trimmed.ends_with(';') {
            let num = &trimmed[3..trimmed.len() - 1];
            self.dial(num, current_ticks)?;
            return Ok("OK".to_string());
        }

        if trimmed == "ATA" {
            self.answer(current_ticks)?;
            return Ok("OK".to_string());
        }

        if trimmed == "ATH" {
            self.hangup()?;
            return Ok("OK".to_string());
        }

        if trimmed == "AT+CLCC" {
            // Query current call list
            use core::fmt::Write;
            let mut s = String::new();
            match &self.call_state {
                CallState::Active { number, .. } => {
                    let _ = write!(s, "+CLCC: 1,0,0,0,0,\"{}\",129\nOK", number);
                }
                CallState::Dialing { destination, .. } => {
                    let _ = write!(s, "+CLCC: 1,0,2,0,0,\"{}\",129\nOK", destination);
                }
                CallState::Incoming { caller_id } => {
                    let _ = write!(s, "+CLCC: 1,1,4,0,0,\"{}\",129\nOK", caller_id);
                }
                _ => {
                    s.push_str("OK");
                }
            }
            return Ok(s);
        }

        // SIM commands
        if trimmed == "AT+CPIN?" {
            return match esim.status {
                SimStatus::Ready => Ok("+CPIN: READY\nOK".to_string()),
                SimStatus::PinRequired => Ok("+CPIN: SIM PIN\nOK".to_string()),
                SimStatus::Blocked => Ok("+CPIN: SIM PUK\nERROR".to_string()),
                SimStatus::NoSim => Ok("+CME ERROR: 10\nERROR".to_string()),
            };
        }

        // Signal quality
        if trimmed == "AT+CSQ" {
            let mut s = String::new();
            use core::fmt::Write;
            let _ = write!(s, "+CSQ: {},{}\nOK", self.signal.rssi, self.signal.ber);
            return Ok(s);
        }

        // Network registration
        if trimmed == "AT+CREG?" || trimmed == "AT+CEREG?" {
            return match self.state {
                ModemRadioState::RegisteredHome | ModemRadioState::DataConnected => {
                    Ok("+CREG: 0,1\nOK".to_string())
                }
                ModemRadioState::Searching => Ok("+CREG: 0,2\nOK".to_string()),
                _ => Ok("+CREG: 0,0\nOK".to_string()),
            };
        }

        // Operator
        if trimmed == "AT+COPS?" {
            let mut s = String::new();
            use core::fmt::Write;
            let _ = write!(s, "+COPS: 0,0,\"{}\"\nOK", esim.carrier_operator);
            return Ok(s);
        }

        // Packet Data Protocol configuration
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

    /// Inbound SMS processor: stores message in inbox and dispatches intent.
    pub fn process_inbound_sms(&mut self, sender: &str, body: &str, current_ticks: u64) -> Option<&'static str> {
        if !self.hardware_present {
            return None;
        }
        if sender.is_empty() || sender.len() > MAX_PHONE_NUMBER_LEN || body.len() > MAX_SMS_BODY_LEN { return None; }
        if self.sms_inbox.len() >= MAX_SMS_RECORDS { return None; }
        let id = self.next_sms_id;
        self.next_sms_id = self.next_sms_id.wrapping_add(1).max(1);
        self.sms_inbox.push(SmsRecord {
            id,
            address: sender.to_string(),
            body: body.to_string(),
            timestamp_ticks: current_ticks,
            is_inbound: true,
        });
        Some("SMS_RECEIVED")
    }
}
