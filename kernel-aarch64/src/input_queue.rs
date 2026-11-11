// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

//! AArch64 Input Event Queue and Serial Touch Parser
//!
//! Decodes serial touch inputs (INPUT_TOUCH, TAP, SWIPE) and enqueues 24-byte
//! InputEvent structures for user event loops.

use spin::Mutex;

#[derive(Clone, Copy)]
pub struct RawInputEvent {
    pub kind: u8, // 1 = Touch
    pub x: u32,
    pub y: u32,
    pub dx: i32,
    pub dy: i32,
    pub scale_milli: u32,
}

impl RawInputEvent {
    pub fn encode(&self, out: &mut [u8]) {
        if out.len() >= 24 {
            out[..24].fill(0);
            out[0] = self.kind;
            out[4..8].copy_from_slice(&self.x.to_le_bytes());
            out[8..12].copy_from_slice(&self.y.to_le_bytes());
            out[12..16].copy_from_slice(&self.dx.to_le_bytes());
            out[16..20].copy_from_slice(&self.dy.to_le_bytes());
            out[20..24].copy_from_slice(&self.scale_milli.to_le_bytes());
        }
    }
}

pub struct InputQueue {
    events: [Option<RawInputEvent>; 32],
    head: usize,
    tail: usize,
    count: usize,
}

impl InputQueue {
    pub const fn new() -> Self {
        Self {
            events: [None; 32],
            head: 0,
            tail: 0,
            count: 0,
        }
    }

    pub fn push(&mut self, ev: RawInputEvent) {
        if self.count < 32 {
            self.events[self.tail] = Some(ev);
            self.tail = (self.tail + 1) % 32;
            self.count += 1;
        }
    }

    pub fn pop(&mut self) -> Option<RawInputEvent> {
        if self.count > 0 {
            let ev = self.events[self.head].take();
            self.head = (self.head + 1) % 32;
            self.count -= 1;
            ev
        } else {
            None
        }
    }
}

pub static INPUT_QUEUE: Mutex<InputQueue> = Mutex::new(InputQueue::new());

pub fn post_touch(x: u32, y: u32) {
    INPUT_QUEUE.lock().push(RawInputEvent {
        kind: 1,
        x,
        y,
        dx: 0,
        dy: 0,
        scale_milli: 1000,
    });
}

static SERIAL_LINE_BUF: Mutex<([u8; 128], usize)> = Mutex::new(([0u8; 128], 0));

pub fn service_serial_commands() {
    let mut guard = SERIAL_LINE_BUF.lock();
    let (buf, len) = &mut *guard;

    while let Some(byte) = crate::uart::UART.lock().read_byte() {
        if byte == b'\n' || byte == b'\r' {
            if *len > 0 {
                process_line(&buf[..*len]);
                *len = 0;
            }
        } else if *len < buf.len() {
            buf[*len] = byte;
            *len += 1;
        }
    }
}

fn process_line(line: &[u8]) {
    let s = match core::str::from_utf8(line) {
        Ok(v) => v.trim(),
        Err(_) => return,
    };
    if s.is_empty() {
        return;
    }

    let mut parts = s.split_whitespace();
    let verb = match parts.next() {
        Some(v) => v,
        None => return,
    };

    if verb.eq_ignore_ascii_case("INPUT_TOUCH") {
        let phase = parts.next().unwrap_or("");
        let px = parts.next().and_then(|v| parse_u32(v)).unwrap_or(0);
        let py = parts.next().and_then(|v| parse_u32(v)).unwrap_or(0);
        if px < 720 && py < 1440 {
            if phase.eq_ignore_ascii_case("DOWN") || phase.eq_ignore_ascii_case("MOVE") {
                post_touch(px, py);
            }
        }
    } else if verb.eq_ignore_ascii_case("TAP") || verb.eq_ignore_ascii_case("TOUCH") {
        if let (Some(x_str), Some(y_str)) = (parts.next(), parts.next()) {
            if let (Some(x), Some(y)) = (parse_u32(x_str), parse_u32(y_str)) {
                if x < 720 && y < 1440 {
                    crate::kprintln!("[UART INPUT] Hardware touch tap received at ({}, {}).", x, y);
                    post_touch(x, y);
                }
            }
        }
    } else if verb.eq_ignore_ascii_case("SWIPE") {
        if let (Some(x1_str), Some(y1_str), Some(x2_str), Some(y2_str)) = (parts.next(), parts.next(), parts.next(), parts.next()) {
            if let (Some(x1), Some(y1), Some(_x2), Some(_y2)) = (parse_u32(x1_str), parse_u32(y1_str), parse_u32(x2_str), parse_u32(y2_str)) {
                post_touch(x1, y1);
            }
        }
    }
}

fn parse_u32(s: &str) -> Option<u32> {
    let mut val = 0u32;
    for b in s.bytes() {
        if !b.is_ascii_digit() {
            return None;
        }
        val = val.checked_mul(10)?.checked_add((b - b'0') as u32)?;
    }
    Some(val)
}
