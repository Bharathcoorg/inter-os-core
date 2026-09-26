//! Streaming Serial RIL (Radio Interface Layer) Engine
//!
//! Provides bare-metal character-stream processing for cellular baseband modems
//! (Qualcomm Snapdragon X55/X65, MediaTek Helio/Dimensity M70/M80, Quectel RM500Q)
//! communicating over high-speed UART or USB CDC-ACM character devices.
//!
//! Implements 3GPP TS 27.007 Hayes AT command framing, line buffered response matching,
//! and real-time Unsolicited Result Code (URC) dispatch into the sovereign event reactor.

use alloc::string::{String, ToString};
use alloc::vec::Vec;
use spin::Mutex;

/// Standard maximum size of a single AT response line or command.
pub const MAX_AT_LINE_LEN: usize = 256;
/// Capacity of the inbound serial character ring buffer.
pub const RX_BUFFER_CAPACITY: usize = 1024;
/// Capacity of the outbound command queue.
pub const TX_QUEUE_CAPACITY: usize = 8;
pub const URC_QUEUE_CAPACITY: usize = 32;

/// Unsolicited Result Code (URC) events emitted spontaneously by cellular baseband towers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UrcEvent {
    /// Inbound voice call ring alert (`+CRING: VOICE` or `RING`)
    IncomingCall { caller_id: Option<String> },
    /// Cellular network registration state changed (`+CREG: <n>,<stat>`)
    NetworkRegistration { status: u8 },
    /// Signal quality report update (`+CSQ: <rssi>,<ber>`)
    SignalQuality { rssi: u8, ber: u8 },
    /// Inbound SMS text alert (`+CMTI: "SM",<index>`)
    InboundSms { index: u32 },
    /// Raw unhandled URC string
    Raw(String),
}

/// Asynchronous streaming Serial RIL controller.
#[allow(dead_code)]
pub struct SerialRilDriver {
    /// Inbound raw character buffer accumulator
    rx_buf: [u8; RX_BUFFER_CAPACITY],
    rx_head: usize,
    rx_tail: usize,
    /// Current line being assembled
    current_line: [u8; MAX_AT_LINE_LEN],
    current_line_len: usize,
    /// Pending unsolicited result codes waiting for userspace consumption
    pending_urcs: Vec<UrcEvent>,
    /// Last solicited response text for the active command transaction
    last_response: Option<String>,
    /// Flag indicating whether the last transaction completed with OK
    transaction_succeeded: bool,
    /// Flag indicating whether the last transaction failed with ERROR / CME ERROR
    transaction_failed: bool,
    /// Outgoing command buffer queue
    tx_queue: Vec<String>,
}

impl SerialRilDriver {
    /// Creates a new serial RIL driver instance.
    pub const fn new() -> Self {
        Self {
            rx_buf: [0u8; RX_BUFFER_CAPACITY],
            rx_head: 0,
            rx_tail: 0,
            current_line: [0u8; MAX_AT_LINE_LEN],
            current_line_len: 0,
            pending_urcs: Vec::new(),
            last_response: None,
            transaction_succeeded: false,
            transaction_failed: false,
            tx_queue: Vec::new(),
        }
    }

    /// Enqueues an AT command for serial transmission to the modem baseband.
    pub fn enqueue_command(&mut self, cmd: &str) -> Result<(), &'static str> {
        if self.tx_queue.len() >= TX_QUEUE_CAPACITY {
            return Err("SerialRil: Tx queue full");
        }
        let mut full_cmd = String::from(cmd.trim());
        full_cmd.push_str("\r\n");
        if full_cmd.len() > MAX_AT_LINE_LEN { return Err("SerialRil: command too long"); }
        self.tx_queue.push(full_cmd);
        Ok(())
    }

    /// Retrieves the next command string ready to be written to the physical UART port.
    pub fn pop_tx_bytes(&mut self) -> Option<String> {
        if self.tx_queue.is_empty() {
            None
        } else {
            Some(self.tx_queue.remove(0))
        }
    }

    /// Feeds raw incoming bytes received from the modem's UART COM port into the streaming parser.
    /// Frames complete lines ending in `\r\n` or `\n` and dispatches them.
    pub fn feed_rx_bytes(&mut self, bytes: &[u8]) -> usize {
        let mut framed_lines = 0;

        for &byte in bytes {
            if byte == b'\r' {
                // Ignore carriage return; wait for newline
                continue;
            } else if byte == b'\n' {
                // End of line reached: process framed line
                if self.current_line_len > 0 {
                    let mut temp = [0u8; MAX_AT_LINE_LEN];
                    let len = self.current_line_len;
                    temp[..len].copy_from_slice(&self.current_line[..len]);
                    self.current_line_len = 0;
                    if let Ok(line_str) = core::str::from_utf8(&temp[..len]) {
                        self.process_framed_line(line_str);
                        framed_lines += 1;
                    }
                }

            } else {
                // Accumulate byte into current line
                if self.current_line_len < MAX_AT_LINE_LEN - 1 {
                    self.current_line[self.current_line_len] = byte;
                    self.current_line_len += 1;
                }
            }
        }

        framed_lines
    }

    /// Processes a single complete line received from the cellular baseband.
    fn process_framed_line(&mut self, line: &str) {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            return;
        }

        // 1. Check for command transaction completion status
        if trimmed == "OK" {
            self.transaction_succeeded = true;
            return;
        }

        if trimmed == "ERROR" || trimmed.starts_with("+CME ERROR:") || trimmed.starts_with("+CMS ERROR:") {
            self.transaction_failed = true;
            return;
        }

        // 2. Check for Unsolicited Result Codes (URCs)
        if self.try_parse_urc(trimmed) {
            return;
        }

        // 3. Otherwise, line is part of the command's data response
        self.last_response = Some(trimmed.to_string());
    }

    /// Attempts to parse an unsolicited result code (URC) from an inbound line.
    fn try_parse_urc(&mut self, line: &str) -> bool {
        if line.starts_with("RING") || line.starts_with("+CRING:") {
            self.push_urc(UrcEvent::IncomingCall { caller_id: None });
            self.notify_event_reactor();
            return true;
        }

        if line.starts_with("+CREG:") {
            // Format: +CREG: <n>,<stat> or +CREG: <stat>
            let status = if let Some(comma_pos) = line.find(',') {
                let after_comma = &line[comma_pos + 1..].trim();
                after_comma.parse::<u8>().unwrap_or(0)
            } else {
                1
            };
            self.push_urc(UrcEvent::NetworkRegistration { status: status.min(5) });
            self.notify_event_reactor();
            return true;
        }

        if line.starts_with("+CSQ:") {
            // Format: +CSQ: <rssi>,<ber>
            let mut rssi = 0u8;
            let mut ber = 99u8;
            if let Some(colon_pos) = line.find(':') {
                let rest = line[colon_pos + 1..].trim();
                let mut parts = rest.split(',');
                if let Some(p1) = parts.next() {
                    rssi = p1.trim().parse::<u8>().unwrap_or(0);
                }
                if let Some(p2) = parts.next() {
                    ber = p2.trim().parse::<u8>().unwrap_or(99);
                }
            }
            self.push_urc(UrcEvent::SignalQuality { rssi: rssi.min(99), ber: ber.min(99) });
            self.notify_event_reactor();
            return true;
        }

        if line.starts_with("+CMTI:") {
            // Format: +CMTI: "SM",<index>
            let idx = if let Some(comma_pos) = line.find(',') {
                let after = line[comma_pos + 1..].trim();
                after.parse::<u32>().unwrap_or(0)
            } else {
                0
            };
            self.push_urc(UrcEvent::InboundSms { index: idx });
            self.notify_event_reactor();
            return true;
        }

        false
    }

    fn push_urc(&mut self, event: UrcEvent) {
        if self.pending_urcs.len() < URC_QUEUE_CAPACITY { self.pending_urcs.push(event); self.notify_event_reactor(); }
    }

    /// Signals the sovereign event reactor that a modem URC has arrived.
    fn notify_event_reactor(&self) {
        // Broadcast EVENT_MODEM_URC (0x40) to any subscribed telephony task
        let mut reactor = crate::event::EVENT_REACTOR.lock();
        reactor.broadcast_event(crate::event::EVENT_MODEM_URC);
    }

    /// Pops the oldest pending URC event from the queue.
    pub fn pop_urc(&mut self) -> Option<UrcEvent> {
        if self.pending_urcs.is_empty() {
            None
        } else {
            Some(self.pending_urcs.remove(0))
        }
    }

    /// Returns the number of unconsumed URC events.
    pub fn pending_urc_count(&self) -> usize {
        self.pending_urcs.len()
    }
}

/// Global static instance of the Serial RIL driver.
pub static SERIAL_RIL: Mutex<SerialRilDriver> = Mutex::new(SerialRilDriver::new());

/// Executes self-test verifying the streaming Serial RIL parser and URC detection.
pub fn test_serial_ril() {
    let mut ril = SerialRilDriver::new();

    // 1. Test Command Enqueueing
    ril.enqueue_command("AT+CSQ").expect("Enqueue failed");
    let tx = ril.pop_tx_bytes().expect("Pop failed");
    assert_eq!(tx, "AT+CSQ\r\n");

    // 2. Test Inbound Feed with Chunked Streaming (Simulating UART bytes arriving in fragments)
    let fragment_1 = b"\r\n+CSQ: ";
    let fragment_2 = b"31,99\r\n\r\nOK\r\n";

    let n1 = ril.feed_rx_bytes(fragment_1);
    assert_eq!(n1, 0, "Incomplete line should not frame");

    let n2 = ril.feed_rx_bytes(fragment_2);
    assert_eq!(n2, 2, "Should frame both +CSQ and OK");
    assert!(ril.transaction_succeeded, "Transaction must succeed on OK");

    // Verify parsed URC
    let urc = ril.pop_urc().expect("URC must be present");
    assert_eq!(urc, UrcEvent::SignalQuality { rssi: 31, ber: 99 });

    // 3. Test Inbound Call Ring Alert
    let ring_stream = b"\r\n+CRING: VOICE\r\n";
    ril.feed_rx_bytes(ring_stream);
    let ring_urc = ril.pop_urc().expect("Ring URC must be present");
    assert_eq!(ring_urc, UrcEvent::IncomingCall { caller_id: None });

    crate::serial_println!(
        "[TEST] Streaming Serial RIL: UART chunked AT framing, +CSQ signal parse, and +CRING URC dispatch PASSED."
    );
}
