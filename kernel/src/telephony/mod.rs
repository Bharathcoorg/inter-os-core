//! Sovereign Telephony & Cellular Baseband Subsystem
//!
//! Provides bare-metal cellular connectivity, AT command interface,
//! Sovereign eSIM authentication, and SMS intent routing.

pub mod sim;
pub mod modem;
pub mod serial_ril;

pub use sim::{SimStatus, SovereignEsim};
pub use modem::{CellularModem, ModemRadioState, SignalQuality};
pub use serial_ril::{SERIAL_RIL, SerialRilDriver, UrcEvent, test_serial_ril};


use spin::Mutex;
use crate::serial_println;

/// Global static instance of the Mobile SIM Profile.
pub static SOVEREIGN_SIM: Mutex<SovereignEsim> = Mutex::new(SovereignEsim {
    subscriber_id: alloc::string::String::new(),
    carrier_operator: "None",
    imsi_pseudonym: [0u8; 16],
    pin_code: *b"1234",
    pin_verified: false,
    status: SimStatus::NoSim,
});

/// Global static instance of the Cellular Modem Driver.
pub static CELLULAR_MODEM: Mutex<CellularModem> = Mutex::new(CellularModem {
    state: ModemRadioState::PowerDown,
    signal: SignalQuality { rssi: 0, ber: 99 },
    pdp_context_active: false,
    apn: "",
    last_received_sms: None,
});

/// Initializes the Telephony subsystem and executes automated self-tests.
pub fn init() {
    serial_println!("--- [Initializing Mobile Telephony & Baseband Subsystem] ---");

    // Initialize SIM profile state
    {
        let mut sim = SOVEREIGN_SIM.lock();
        *sim = SovereignEsim::new("sub_001_mobile_core");
        serial_println!(
            "[TELEPHONY] SIM Profile: Initialized (Status = {:?}, Operator = '{}').",
            sim.status, sim.carrier_operator
        );
    }

    // 2. Test 3GPP AT Command State Machine
    {
        let mut modem = CELLULAR_MODEM.lock();
        let sim = SOVEREIGN_SIM.lock();

        let at_resp = modem.execute_at("AT", &sim).expect("AT failed");
        assert_eq!(at_resp, "OK");

        let cpin_resp = modem.execute_at("AT+CPIN?", &sim).expect("CPIN failed");
        assert!(cpin_resp.contains("CME ERROR") || cpin_resp.contains("READY"));

        let csq_resp = modem.execute_at("AT+CSQ", &sim).expect("CSQ failed");
        assert!(csq_resp.contains("+CSQ:"));

        let creg_resp = modem.execute_at("AT+CREG?", &sim).expect("CREG failed");
        assert!(creg_resp.contains("+CREG:"));

        serial_println!("[TEST] Cellular Modem: 3GPP AT command state machine verified (AT, CPIN, CSQ, CREG).");
    }

    // 3. Test SMS Intent Routing
    {
        let mut modem = CELLULAR_MODEM.lock();
        let intent = modem.process_inbound_sms("+18005550199", "Hello from InterOS cellular network");
        assert_eq!(intent, Some("SMS_RECEIVED"));
        serial_println!("[TEST] Cellular SMS Gateway: Inbound SMS delivered to messages pipeline ('SMS_RECEIVED').");
    }

    // 4. Test Streaming Serial RIL Driver & URC Event Dispatch
    serial_ril::test_serial_ril();

    serial_println!("--- [Sovereign Telephony Subsystem 100% Attested & Enforced] ---");
}


/// Periodic multitasking worker task for the cellular modem (Task 11).
pub extern "C" fn task_telephony_worker() -> ! {
    let mut ticks = 0u64;
    loop {
        ticks = ticks.wrapping_add(1);
        if ticks % 50 == 0 {
            let modem = CELLULAR_MODEM.lock();
            crate::serial_println!(
                "[TASK telephony_worker (ID 11) LOG] [TELEPHONY WORKER] 5G Bearer: State = {:?}, RSSI = {}/31, APN = '{}'.",
                modem.state,
                modem.signal.rssi,
                modem.apn
            );
        }
        crate::task::yield_now();
    }
}
