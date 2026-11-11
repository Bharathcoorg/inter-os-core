// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

//! RISC-V 64 Kernel Input Event Queue & Serial Command Processor

use spin::Mutex;

#[derive(Debug, Clone, Copy)]
pub struct InputEvent {
    pub kind: u8,
    pub _pad: [u8; 3],
    pub x: u32,
    pub y: u32,
    pub dx: i32,
    pub dy: i32,
    pub scale_milli: u32,
}

impl InputEvent {
    pub const fn empty() -> Self {
        Self {
            kind: 0,
            _pad: [0; 3],
            x: 0,
            y: 0,
            dx: 0,
            dy: 0,
            scale_milli: 1000,
        }
    }

    pub fn encode(&self, out: &mut [u8; 24]) {
        out.fill(0);
        out[0] = self.kind;
        out[4..8].copy_from_slice(&self.x.to_le_bytes());
        out[8..12].copy_from_slice(&self.y.to_le_bytes());
        out[12..16].copy_from_slice(&self.dx.to_le_bytes());
        out[16..20].copy_from_slice(&self.dy.to_le_bytes());
        out[20..24].copy_from_slice(&self.scale_milli.to_le_bytes());
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
        let _slot = parts.next().and_then(|s| s.parse::<u8>().ok()).unwrap_or(0);
        let phase = parts.next().unwrap_or("0");
        let x = parts.next().and_then(|s| s.parse::<u32>().ok()).unwrap_or(0);
        let y = parts.next().and_then(|s| s.parse::<u32>().ok()).unwrap_or(0);

        let kind = match phase {
            "down" | "1" => 1,
            "move" | "2" => 2,
            "up" | "0" => 3,
            _ => 1,
        };

        let ev = InputEvent {
            kind,
            _pad: [0; 3],
            x,
            y,
            dx: 0,
            dy: 0,
            scale_milli: 1000,
        };
        INPUT_QUEUE.lock().push(ev);
    } else if trimmed.starts_with("TAP:") {
        let payload = &trimmed["TAP:".len()..];
        let mut parts = payload.split(',');
        let x = parts.next().and_then(|s| s.parse::<u32>().ok()).unwrap_or(0);
        let y = parts.next().and_then(|s| s.parse::<u32>().ok()).unwrap_or(0);

        let down_ev = InputEvent {
            kind: 1,
            _pad: [0; 3],
            x,
            y,
            dx: 0,
            dy: 0,
            scale_milli: 1000,
        };
        let up_ev = InputEvent {
            kind: 3,
            _pad: [0; 3],
            x,
            y,
            dx: 0,
            dy: 0,
            scale_milli: 1000,
        };
        let mut q = INPUT_QUEUE.lock();
        q.push(down_ev);
        q.push(up_ev);
    }
}
