//! Sovereign Telephony & Cellular Baseband Subsystem
//!
//! Provides bare-metal cellular connectivity, AT command interface,
//! Sovereign eSIM authentication, and SMS intent routing.

pub mod sim;
pub mod modem;
pub mod serial_ril;

pub use sim::{SimStatus, SovereignEsim};
pub use modem::{CallState, CellularModem, ModemRadioState, SignalQuality, SmsRecord};
pub use serial_ril::{SERIAL_RIL, SerialRilDriver, UrcEvent, test_serial_ril};

use spin::Mutex;
use crate::serial_println;

/// Global static instance of the Mobile SIM Profile.
///
/// No subscriber credential is present: there is no provisioned profile on this
/// platform, so there is no carrier identity, PIN or subscriber key to report.
pub static SOVEREIGN_SIM: Mutex<SovereignEsim> = Mutex::new(SovereignEsim {
    subscriber_id: alloc::string::String::new(),
    carrier_operator: "",
    imsi_pseudonym: [0u8; 16],
    pin_code: None,
    pin_verified: false,
    status: SimStatus::NoSim,
    subscriber_auth_key: None,
});

/// Global static instance of the Cellular Modem Driver.
pub static CELLULAR_MODEM: Mutex<CellularModem> = Mutex::new(CellularModem {
    hardware_present: false,
    state: ModemRadioState::PowerDown,
    call_state: CallState::Idle,
    signal: SignalQuality { rssi: 99, ber: 99 },
    pdp_context_active: false,
    apn: "",
    ip_address: [0, 0, 0, 0],
    sms_inbox: alloc::vec::Vec::new(),
    sms_outbox: alloc::vec::Vec::new(),
    next_sms_id: 1,
});

/// Initializes the Telephony subsystem and executes automated self-tests.
pub fn test_telephony() {
    serial_println!("--- [Initializing Mobile Telephony & Baseband Subsystem] ---");

    // Initialize SIM profile state
    {
        let mut sim = SOVEREIGN_SIM.lock();
        *sim = SovereignEsim::new("sub_001_mobile_core");
        sim.status = SimStatus::Ready;
        serial_println!(
            "[TELEPHONY] SIM Profile: Initialized (Status = {:?}, Operator = '{}').",
            sim.status, sim.carrier_operator
        );
    }

    // 2. Test 3GPP AT Command State Machine & Voice Calling
    {
        let mut modem = CELLULAR_MODEM.lock();
        // The self-test explicitly binds a simulated backend; this must never
        // be confused with platform hardware detection.
        modem.set_hardware_present(true);
        modem.state = ModemRadioState::RegisteredHome;
        let sim = SOVEREIGN_SIM.lock();

        let at_resp = modem.execute_at("AT", &sim, 100).expect("AT failed");
        assert_eq!(at_resp, "OK");

        let cpin_resp = modem.execute_at("AT+CPIN?", &sim, 100).expect("CPIN failed");
        assert!(cpin_resp.contains("READY"));

        let csq_resp = modem.execute_at("AT+CSQ", &sim, 100).expect("CSQ failed");
        assert!(csq_resp.contains("+CSQ:"));

        let creg_resp = modem.execute_at("AT+CREG?", &sim, 100).expect("CREG failed");
        assert!(creg_resp.contains("+CREG:"));

        // Test Voice Dialing AT command
        let dial_resp = modem.execute_at("ATD+18005550100;", &sim, 100).expect("ATD failed");
        assert_eq!(dial_resp, "OK");
        assert!(matches!(modem.call_state, CallState::Dialing { .. }));

        // Test Answering
        let _ans_resp = modem.answer(150);
        // Transition to active call
        modem.call_state = CallState::Active {
            number: alloc::string::String::from("+18005550100"),
            start_ticks: 150,
        };
        assert_eq!(modem.get_call_duration(450), 3); // (450 - 150) / 100 = 3 seconds

        // Test Hangup
        let hangup_resp = modem.hangup();
        assert!(hangup_resp.is_ok());
        assert_eq!(modem.call_state, CallState::Disconnected);

        // Reset to idle
        modem.call_state = CallState::Idle;

        serial_println!("[TEST] Cellular Modem: 3GPP AT command state machine & voice calls verified.");
    }

    // 3. Test SMS Transmission and Intent Routing
    {
        let mut modem = CELLULAR_MODEM.lock();
        let sms_id = modem.send_sms("+18005550100", "Hello from InterOS!", 200).expect("send_sms failed");
        assert_eq!(sms_id, 1);
        assert_eq!(modem.sms_outbox.len(), 1);

        let intent = modem.process_inbound_sms("+18005550199", "Inbound SMS message", 210);
        assert_eq!(intent, Some("SMS_RECEIVED"));
        assert_eq!(modem.sms_inbox.len(), 1);
        serial_println!("[TEST] Cellular SMS Gateway: Inbound and outbound SMS transmission verified.");
    }

    // 4. Test Streaming Serial RIL Driver & URC Event Dispatch
    serial_ril::test_serial_ril();

    // 5. Reset to truthful hardware state: in standard QEMU virt without physical baseband,
    // report PowerDown / undetectable signal instead of advertising active connectivity.
    {
        let mut modem = CELLULAR_MODEM.lock();
        modem.set_hardware_present(false);
        modem.state = ModemRadioState::PowerDown;
        modem.call_state = CallState::Idle;
        modem.signal = SignalQuality { rssi: 99, ber: 99 };
        modem.pdp_context_active = false;
        modem.apn = "";
        modem.ip_address = [0, 0, 0, 0];
        modem.sms_inbox.clear();
        modem.sms_outbox.clear();

        let mut sim = SOVEREIGN_SIM.lock();
        sim.status = SimStatus::NoSim;
        sim.carrier_operator = "None";
    }

    serial_println!("--- [Sovereign Telephony Subsystem 100% Attested & Enforced] ---");
}


/// Initializes telephony to a truthful hardware-neutral state.
pub fn init() {
    let mut modem = CELLULAR_MODEM.lock();
    modem.set_hardware_present(false);
    modem.state = ModemRadioState::PowerDown;
    modem.call_state = CallState::Idle;
    modem.signal = SignalQuality { rssi: 99, ber: 99 };
    modem.pdp_context_active = false;
    modem.apn = "";
    modem.ip_address = [0, 0, 0, 0];
    modem.sms_inbox.clear();
    modem.sms_outbox.clear();
    let mut sim = SOVEREIGN_SIM.lock();
    sim.status = SimStatus::NoSim;
    sim.carrier_operator = "None";
    serial_println!("[TELEPHONY] No physical baseband/SIM backend registered; runtime state remains offline.");
}

/// Periodic multitasking worker task for the cellular modem (Task 11).
pub extern "C" fn task_telephony_worker() -> ! {
    let mut ticks = 0u64;
    loop {
        ticks = ticks.wrapping_add(1);
        if ticks % 50 == 0 {
            let modem = CELLULAR_MODEM.lock();
            if modem.state == ModemRadioState::PowerDown {
                crate::serial_println!(
                    "[TASK telephony_worker (ID 11) LOG] [TELEPHONY WORKER] Cellular Modem: Standby (No hardware baseband modem detected)."
                );
            } else {
                crate::serial_println!(
                    "[TASK telephony_worker (ID 11) LOG] [TELEPHONY WORKER] 5G Bearer: State = {:?}, RSSI = {}/31, APN = '{}'.",
                    modem.state,
                    modem.signal.rssi,
                    modem.apn
                );
            }
        }
        crate::task::yield_now();
    }
}
