// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

//! RISC-V 64 Kernel Input Event Queue & Serial Command Processor

use spin::Mutex;

#[derive(Debug, Clone, Copy)]
pub struct InputEvent {
    pub ev_type: u16,
    pub code: u16,
    pub value: i32,
    pub x: u32,
    pub y: u32,
    pub slot: u8,
    pub timestamp_ms: u64,
}

impl InputEvent {
    pub const fn empty() -> Self {
        Self {
            ev_type: 0,
            code: 0,
            value: 0,
            x: 0,
            y: 0,
            slot: 0,
            timestamp_ms: 0,
        }
    }

    pub fn encode(&self, out: &mut [u8; 24]) {
        out[0..2].copy_from_slice(&self.ev_type.to_le_bytes());
        out[2..4].copy_from_slice(&self.code.to_le_bytes());
        out[4..8].copy_from_slice(&self.value.to_le_bytes());
        out[8..12].copy_from_slice(&self.x.to_le_bytes());
        out[12..16].copy_from_slice(&self.y.to_le_bytes());
        out[16] = self.slot;
        out[17..24].copy_from_slice(&self.timestamp_ms.to_le_bytes()[..7]);
    }
}

pub struct InputQueue {
    buffer: [InputEvent; 64],
    head: usize,
    tail: usize,
    count: usize,
}

impl InputQueue {
    pub const fn new() -> Self {
        Self {
            buffer: [InputEvent::empty(); 64],
            head: 0,
            tail: 0,
            count: 0,
        }
    }

    pub fn push(&mut self, ev: InputEvent) -> bool {
        if self.count >= 64 {
            return false;
        }
        self.buffer[self.tail] = ev;
        self.tail = (self.tail + 1) % 64;
        self.count += 1;
        true
    }

    pub fn pop(&mut self) -> Option<InputEvent> {
        if self.count == 0 {
            return None;
        }
        let ev = self.buffer[self.head];
        self.head = (self.head + 1) % 64;
        self.count -= 1;
        Some(ev)
    }
}

pub static INPUT_QUEUE: Mutex<InputQueue> = Mutex::new(InputQueue::new());

pub fn parse_input_line(line: &str) {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return;
    }

    if trimmed.starts_with("INPUT_TOUCH:") {
        let payload = &trimmed["INPUT_TOUCH:".len()..];
        let mut parts = payload.split(',');
        let slot = parts.next().and_then(|s| s.parse::<u8>().ok()).unwrap_or(0);
        let phase = parts.next().unwrap_or("0");
        let x = parts.next().and_then(|s| s.parse::<u32>().ok()).unwrap_or(0);
        let y = parts.next().and_then(|s| s.parse::<u32>().ok()).unwrap_or(0);

        let value = match phase {
            "down" | "1" => 1,
            "move" | "2" => 2,
            "up" | "0" => 0,
            _ => 1,
        };

        let ev = InputEvent {
            ev_type: 3, // EV_ABS
            code: 0x35, // ABS_MT_POSITION_X
            value,
            x,
            y,
            slot,
            timestamp_ms: 0,
        };
        INPUT_QUEUE.lock().push(ev);
    } else if trimmed.starts_with("TAP:") {
        let payload = &trimmed["TAP:".len()..];
        let mut parts = payload.split(',');
        let x = parts.next().and_then(|s| s.parse::<u32>().ok()).unwrap_or(0);
        let y = parts.next().and_then(|s| s.parse::<u32>().ok()).unwrap_or(0);

        let down_ev = InputEvent {
            ev_type: 3,
            code: 0x35,
            value: 1,
            x,
            y,
            slot: 0,
            timestamp_ms: 0,
        };
        let up_ev = InputEvent {
            ev_type: 3,
            code: 0x35,
            value: 0,
            x,
            y,
            slot: 0,
            timestamp_ms: 0,
        };
        let mut q = INPUT_QUEUE.lock();
        q.push(down_ev);
        q.push(up_ev);
    }
}
