#![no_std]

pub use interos_std as std;

pub const LINE_CAP: usize = 128;
pub const CONSOLE_LINES: usize = 22;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ConsoleLine {
    pub text: [u8; LINE_CAP],
    pub len: usize,
    pub color: u32,
}

impl ConsoleLine {
    pub const fn from_static(text: &'static str, color: u32) -> Self {
        let bytes = text.as_bytes();
        let mut buf = [0u8; LINE_CAP];
        let mut i = 0;
        while i < bytes.len() && i < LINE_CAP {
            buf[i] = bytes[i];
            i += 1;
        }
        Self {
            text: buf,
            len: i,
            color,
        }
    }

    pub fn as_bytes(&self) -> &[u8] {
        &self.text[..self.len]
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HistoryEntry {
    pub cmd: &'static str,
    pub status: i64,
}

pub struct TerminalState {
    pub history: [Option<HistoryEntry>; 8],
    pub history_count: usize,
    pub total_commands_executed: u32,
    pub lines: [Option<ConsoleLine>; CONSOLE_LINES],
    pub line_count: usize,
}

impl TerminalState {
    pub const fn new() -> Self {
        Self {
            history: [None; 8],
            history_count: 0,
            total_commands_executed: 0,
            lines: [None; CONSOLE_LINES],
            line_count: 0,
        }
    }

    pub fn push_line(&mut self, text: &[u8], color: u32) {
        let mut line = ConsoleLine {
            text: [0; LINE_CAP],
            len: 0,
            color,
        };
        let n = text.len().min(LINE_CAP);
        line.text[..n].copy_from_slice(&text[..n]);
        line.len = n;
        if self.line_count < CONSOLE_LINES {
            self.lines[self.line_count] = Some(line);
            self.line_count += 1;
        } else {
            for i in 0..CONSOLE_LINES - 1 {
                self.lines[i] = self.lines[i + 1];
            }
            self.lines[CONSOLE_LINES - 1] = Some(line);
        }
    }

    pub fn record_history(&mut self, cmd: &'static str, status: i64) {
        let idx = self.history_count % self.history.len();
        self.history[idx] = Some(HistoryEntry { cmd, status });
        self.history_count += 1;
        self.total_commands_executed += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn console_line_from_static_and_as_bytes() {
        let line = ConsoleLine::from_static("test command", 0xFFFFFFFF);
        assert_eq!(line.as_bytes(), b"test command");
        assert_eq!(line.len, 12);
        assert_eq!(line.color, 0xFFFFFFFF);
    }

    #[test]
    fn terminal_state_push_line_scrolls_when_full() {
        let mut state = TerminalState::new();
        for i in 0..CONSOLE_LINES + 5 {
            let mut buf = [0u8; 16];
            buf[0] = b'L';
            buf[1] = b'0' + (i as u8 % 10);
            state.push_line(&buf[..2], 0xFF00FF00);
        }
        assert_eq!(state.line_count, CONSOLE_LINES);
        let last = state.lines[CONSOLE_LINES - 1].unwrap();
        // The last line pushed was i = CONSOLE_LINES + 4 = 26 -> b'L', b'6'
        assert_eq!(last.as_bytes(), b"L6");
    }

    #[test]
    fn terminal_history_circular_recording() {
        let mut state = TerminalState::new();
        state.record_history("help", 0);
        state.record_history("ps", 0);
        assert_eq!(state.history_count, 2);
        assert_eq!(state.total_commands_executed, 2);
        assert_eq!(state.history[0].unwrap().cmd, "help");
        assert_eq!(state.history[1].unwrap().cmd, "ps");
    }
}
