// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

//! RISC-V 64 Input Event Queue and Serial Touch Parser
//!
//! Decodes serial touch inputs (INPUT_TOUCH, TAP, SWIPE, NAV) and enqueues 24-byte
//! InputEvent structures for user event loops with task-targeted routing.

use spin::Mutex;

#[derive(Clone, Copy)]
pub struct RawInputEvent {
    pub kind: u8,
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

#[derive(Clone, Copy)]
pub struct QueuedInputEvent {
    pub target_task: u32,
    pub event: RawInputEvent,
}

pub struct InputQueue {
    events: [Option<QueuedInputEvent>; 32],
    len: usize,
}

impl InputQueue {
    pub const fn new() -> Self {
        Self {
            events: [None; 32],
            len: 0,
        }
    }

    pub fn push(&mut self, target_task: u32, ev: RawInputEvent) {
        if self.len < 32 {
            self.events[self.len] = Some(QueuedInputEvent {
                target_task,
                event: ev,
            });
            self.len += 1;
        }
    }

    pub fn pop_for_task(&mut self, task_pid: u32) -> Option<RawInputEvent> {
        for i in 0..self.len {
            if let Some(qe) = &self.events[i] {
                if qe.target_task == task_pid || qe.target_task == u32::MAX {
                    let ev = qe.event;
                    for j in i..(self.len - 1) {
                        self.events[j] = self.events[j + 1].take();
                    }
                    self.events[self.len - 1] = None;
                    self.len -= 1;
                    return Some(ev);
                }
            }
        }
        None
    }

    pub fn has_events_for_task(&self, task_pid: u32) -> bool {
        for i in 0..self.len {
            if let Some(qe) = &self.events[i] {
                if qe.target_task == task_pid || qe.target_task == u32::MAX {
                    return true;
                }
            }
        }
        false
    }

    pub fn clear_for_task(&mut self, task_pid: u32) -> usize {
        let mut cleared = 0;
        let mut i = 0;
        while i < self.len {
            if let Some(qe) = &self.events[i] {
                if qe.target_task == task_pid {
                    for j in i..(self.len - 1) {
                        self.events[j] = self.events[j + 1].take();
                    }
                    self.events[self.len - 1] = None;
                    self.len -= 1;
                    cleared += 1;
                    continue;
                }
            }
            i += 1;
        }
        cleared
    }
}

pub static INPUT_QUEUE: Mutex<InputQueue> = Mutex::new(InputQueue::new());

pub fn post_touch(x: u32, y: u32) {
    let target = crate::display::focused_task_pid();
    INPUT_QUEUE.lock().push(target, RawInputEvent {
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

    loop {
        let byte_opt = crate::sbi::console_getchar();
        let Some(byte) = byte_opt else { break; };
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
                    crate::println!("[UART INPUT] Hardware touch tap received at ({}, {}).", x, y);
                    post_touch(x, y);
                }
            }
        }
    } else if verb.eq_ignore_ascii_case("SWIPE") {
        if let (Some(x1_str), Some(y1_str), Some(x2_str), Some(y2_str)) = (parts.next(), parts.next(), parts.next(), parts.next()) {
            if let (Some(x1), Some(y1), Some(x2), Some(y2)) = (parse_u32(x1_str), parse_u32(y1_str), parse_u32(x2_str), parse_u32(y2_str)) {
                let dy = (y2 as i32) - (y1 as i32);
                let dx = (x2 as i32) - (x1 as i32);
                crate::println!("[UART INPUT] Hardware swipe received from ({},{}) to ({},{}) dy={}.", x1, y1, x2, y2, dy);
                let kind = if dy <= -40 && y1 >= 1000 {
                    7 // Home / Unlock gesture
                } else {
                    4 // Pan / Drag
                };
                let target = crate::display::focused_task_idx();
                INPUT_QUEUE.lock().push(target, RawInputEvent {
                    kind,
                    x: x2,
                    y: y2,
                    dx,
                    dy,
                    scale_milli: 1000,
                });
            }
        }
    } else if verb.eq_ignore_ascii_case("NAV") {
        if let Some(target_nav) = parts.next() {
            let target = crate::display::focused_task_idx();
            if target_nav.eq_ignore_ascii_case("HOME") {
                crate::println!("[UART INPUT] Hardware NAV HOME event received.");
                INPUT_QUEUE.lock().push(target, RawInputEvent {
                    kind: 7,
                    x: 360,
                    y: 1416,
                    dx: 0,
                    dy: 0,
                    scale_milli: 1000,
                });
            } else if target_nav.eq_ignore_ascii_case("BACK") {
                crate::println!("[UART INPUT] Hardware NAV BACK event received.");
                INPUT_QUEUE.lock().push(target, RawInputEvent {
                    kind: 6,
                    x: 120,
                    y: 1416,
                    dx: 0,
                    dy: 0,
                    scale_milli: 1000,
                });
            } else if target_nav.eq_ignore_ascii_case("RECENTS") {
                crate::println!("[UART INPUT] Hardware NAV RECENTS event received.");
                INPUT_QUEUE.lock().push(target, RawInputEvent {
                    kind: 8,
                    x: 600,
                    y: 1416,
                    dx: 0,
                    dy: 0,
                    scale_milli: 1000,
                });
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
