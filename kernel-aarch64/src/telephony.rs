//! Streaming Serial RIL (Radio Interface Layer) Engine (AArch64)
//!
//! Provides bare-metal character-stream processing for cellular baseband modems
//! (Qualcomm Snapdragon X55/X65, MediaTek Dimensity, Quectel RM500Q)
//! communicating over high-speed UART or USB CDC-ACM character devices.

extern crate alloc;
use alloc::string::String;
use alloc::vec::Vec;

pub const MAX_AT_LINE_LEN: usize = 256;
pub const RX_BUFFER_CAPACITY: usize = 1024;
pub const TX_QUEUE_CAPACITY: usize = 8;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UrcEvent {
    IncomingCall { caller_id: Option<String> },
    NetworkRegistration { status: u8 },
    SignalQuality { rssi: u8, ber: u8 },
    InboundSms { index: u32 },
    Raw(String),
}

pub struct SerialRilDriver {
    #[allow(dead_code)]
    rx_buf: [u8; RX_BUFFER_CAPACITY],
    #[allow(dead_code)]
    rx_head: usize,
    #[allow(dead_code)]
    rx_tail: usize,
    current_line: [u8; MAX_AT_LINE_LEN],
    current_line_len: usize,
    pub pending_urcs: Vec<UrcEvent>,
    pub last_response: Option<String>,
    pub transaction_succeeded: bool,
    pub transaction_failed: bool,
    pub tx_queue: Vec<String>,
    /// False until a physical baseband has been bound. Signal strength,
    /// registration state and call alerts may only be surfaced when a modem
    /// actually produced them.
    pub hardware_present: bool,
}

impl SerialRilDriver {
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
            hardware_present: false,
        }
    }

    /// Binds the driver to a baseband reachable on a concrete transport.
    pub fn set_hardware_present(&mut self, present: bool) {
        self.hardware_present = present;
        if !present {
            self.pending_urcs.clear();
            self.last_response = None;
        }
    }

    /// Feeds bytes received from the physical baseband UART.
    ///
    /// Fails closed when no modem is bound. Callers that need to exercise the
    /// AT framer itself use [`SerialRilDriver::feed_rx_bytes`] directly.
    pub fn ingest_modem_rx(&mut self, bytes: &[u8]) -> Result<usize, &'static str> {
        if !self.hardware_present {
            return Err("SerialRIL: modem hardware unavailable");
        }
        Ok(self.feed_rx_bytes(bytes))
    }

    /// Reads the current signal quality reported by the modem.
    pub fn read_signal_quality(&self) -> Result<u8, &'static str> {
        if !self.hardware_present {
            return Err("SerialRIL: modem hardware unavailable");
        }
        for urc in &self.pending_urcs {
            if let UrcEvent::SignalQuality { rssi, .. } = urc {
                return Ok(*rssi);
            }
        }
        Err("SerialRIL: no signal quality reported by modem")
    }

    pub fn enqueue_command(&mut self, cmd: &str) -> Result<(), &'static str> {
        if self.tx_queue.len() >= TX_QUEUE_CAPACITY {
            return Err("SerialRil: Tx queue full");
        }
        let mut full_cmd = String::from(cmd.trim());
        full_cmd.push_str("\r\n");
        self.tx_queue.push(full_cmd);
        Ok(())
    }

    pub fn feed_rx_bytes(&mut self, bytes: &[u8]) -> usize {
        let mut framed_lines = 0;

        for &byte in bytes {
            if byte == b'\r' || byte == b'\n' {
                if self.current_line_len > 0 {
                    let mut temp_buf = [0u8; MAX_AT_LINE_LEN];
                    let len = self.current_line_len;
                    temp_buf[..len].copy_from_slice(&self.current_line[..len]);
                    self.current_line_len = 0;
                    if let Ok(line_str) = core::str::from_utf8(&temp_buf[..len]) {
                        self.process_framed_line(line_str.trim());
                        framed_lines += 1;
                    }
                }
            } else if self.current_line_len < MAX_AT_LINE_LEN {
                self.current_line[self.current_line_len] = byte;
                self.current_line_len += 1;
            } else {
                self.current_line_len = 0;
            }
        }

        framed_lines
    }

    fn process_framed_line(&mut self, line: &str) {
        if line.is_empty() {
            return;
        }

        if line == "OK" {
            self.transaction_succeeded = true;
            self.transaction_failed = false;
        } else if line == "ERROR" || line.starts_with("+CME ERROR:") || line.starts_with("+CMS ERROR:") {
            self.transaction_succeeded = false;
            self.transaction_failed = true;
        } else if line.starts_with("+CSQ:") {
            if let Some(csq_str) = line.strip_prefix("+CSQ:") {
                let parts: Vec<&str> = csq_str.trim().split(',').collect();
                if parts.len() >= 2 {
                    let rssi = parts[0].trim().parse::<u8>().unwrap_or(0);
                    let ber = parts[1].trim().parse::<u8>().unwrap_or(99);
                    self.pending_urcs.push(UrcEvent::SignalQuality { rssi, ber });
                }
            }
        } else if line.starts_with("+CRING:") || line == "RING" {
            self.pending_urcs.push(UrcEvent::IncomingCall { caller_id: None });
        } else {
            self.last_response = Some(String::from(line));
        }
    }
}
