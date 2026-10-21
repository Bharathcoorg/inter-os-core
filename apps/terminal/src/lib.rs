#![no_std]

pub use interos_std as std;

/// The app's view of the platform's design system.
pub mod theme;

pub const LINE_CAP: usize = 128;

/// Lines the console buffer keeps.
///
/// WHY the bound lives here and not in the app: the renderer derives the
/// console band from it, and the tests below exercise the buffer that uses
/// it. A second copy of the number in the app would be a bound no test
/// covers. 114 + 22 * 48 = 1170 still clears the quick-command dock at 1210.
pub const CONSOLE_LINES: usize = 22;

/// The boot banner, one console line each.
///
/// WHY it is here: the banner is the shell's first claim about itself, and
/// a claim no test can read is a claim nobody checks. The banner used to
/// read "InterOS Sovereign Shell [Ring 3 ISOLATED]" and "Isolated memory,
/// token capabilities enforced.": the shell cannot read an isolation state
/// - no syscall reports one - so the first line asserted a property it had
/// never observed, and the second claimed the platform enforces
/// capabilities without the shell having seen a grant or a refusal.
pub const BANNER: [&str; 6] = [
    "InterOS Sovereign Shell [Ring 3",
    "userspace]",
    "capability types are resolved by",
    "the platform; caps lists them.",
    "Tap quick commands below for",
    "diagnostics.",
];

/// The verbs the command dispatcher answers.
///
/// WHY it is a list: the help text has to describe the dispatcher, and a
/// list the help and the dispatcher both read cannot drift.
pub const DISPATCHED_COMMANDS: [&str; 13] = [
    "help", "status", "ps", "mem", "caps", "share", "dma", "ping", "services", "ls", "vfs",
    "cat", "clear",
];

/// The help line that lists the commands.
pub const HELP_COMMANDS: &str =
    "Commands: help, status, ps, mem, caps, share, dma, ping, ls, vfs, services, cat, clear";

/// The help line that says what `ls` and `cat` take.
pub const HELP_DETAIL: &str = "ls [path] lists a directory; cat [path] prints a file.";

/// The marker the capability list puts in front of a type the shell holds.
pub const HELD_MARKER: &str = "caps: held     ";

/// The marker the capability list puts in front of a type it does not hold.
pub const NOT_HELD_MARKER: &str = "caps: not held ";

/// The console line the capability list prints for one type.
///
/// WHY the shell marks every line: the list used to name only the types the
/// shell held, so a reader saw a bare list of names and a pair of counts at
/// the bottom, and the names read as the shell's whole capability set. It is
/// a subset - the types this app knows about - and every line now says which
/// of them the shell holds.
pub fn capability_line(held: bool, name: &str, out: &mut [u8; 64]) -> usize {
    let marker = if held { HELD_MARKER } else { NOT_HELD_MARKER };
    let p = marker.len().min(out.len());
    out[..p].copy_from_slice(&marker.as_bytes()[..p]);
    let r = name.len().min(out.len() - p);
    out[p..p + r].copy_from_slice(&name.as_bytes()[..r]);
    p + r
}

/// What a key press means to the shell's line editor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShellKey {
    /// Submit the line.
    Enter,
    /// Delete the last character.
    Backspace,
    /// Abandon the line.
    Escape,
    /// A printable character.
    Character(u8),
    /// Nothing the editor acts on.
    Ignored,
}

/// Classifies a key code for the line editor.
///
/// WHY this is a function and not a match arm in the app: the platform's
/// reserved key codes are not small integers. The app's arm used to write
/// `input_key_code::ENTER | 10 | 13`, which is a bitwise OR of 0x10000 with
/// 10 and 13, and that is 0x1000F - the POWER key's code. The arm matched
/// neither the Enter key the keyboard sends nor the raw 10 and 13, and
/// Backspace and Escape were the same mistake, so the shell could be typed
/// into and never run. A classification the app and the tests both call is a
/// classification that cannot drift.
pub fn classify_key(code: u32) -> ShellKey {
    use interos_std::input_key_code;
    if code == input_key_code::ENTER || code == 10 || code == 13 {
        ShellKey::Enter
    } else if code == input_key_code::BACKSPACE || code == 8 || code == 127 {
        ShellKey::Backspace
    } else if code == input_key_code::ESCAPE || code == 27 {
        ShellKey::Escape
    } else if (32..=126).contains(&code) {
        ShellKey::Character(code as u8)
    } else {
        ShellKey::Ignored
    }
}

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

    #[test]
    fn the_outline_face_is_never_wider_than_the_bitmap_face() {
        // WHY this matters: the console wraps its lines with the bitmap
        // face's advance table and draws them with the outline face, so a
        // face that is wider than the one the wrap used would put a run past
        // the column and the run would be severed at the surface edge.
        for text in [
            &b"Commands: help, status, ps, mem, caps, share, dma, ping, ls, vfs, services, cat, clear"[..],
            &b"InterOS Core Mobile Platform | Ring 3 userspace"[..],
            &b"the quick brown fox jumps over the lazy dog 0123456789"[..],
        ] {
            let bitmap = interos_std::font::measure(text, 2);
            let outline = interos_std::design::outline_text::drawn_advance(text, 2, None);
            match outline {
                Some(outline) => assert!(
                    outline <= bitmap,
                    "the outline face is wider than the bitmap face for {:?}: {} vs {}",
                    core::str::from_utf8(text).unwrap_or("<binary>"),
                    outline,
                    bitmap
                ),
                None => {}
            }
        }
    }

    #[test]
    fn the_shell_recognises_the_platforms_enter_backspace_and_escape_codes() {
        use interos_std::input_key_code;
        // WHY these three: the app's key arm used to OR the platform's
        // reserved codes with the raw ASCII fallbacks, and the OR of
        // 0x10000 with 10 and 13 is 0x1000F - the POWER key's code - so the
        // Enter arm matched nothing the keyboard sends. The shell could be
        // typed into and never run.
        assert_eq!(classify_key(input_key_code::ENTER), ShellKey::Enter);
        assert_eq!(classify_key(input_key_code::BACKSPACE), ShellKey::Backspace);
        assert_eq!(classify_key(input_key_code::ESCAPE), ShellKey::Escape);
        // And the raw ASCII fallbacks still work.
        assert_eq!(classify_key(10), ShellKey::Enter);
        assert_eq!(classify_key(13), ShellKey::Enter);
        assert_eq!(classify_key(8), ShellKey::Backspace);
        assert_eq!(classify_key(127), ShellKey::Backspace);
        assert_eq!(classify_key(27), ShellKey::Escape);
        // And the reserved codes are not the keys they were mistaken for.
        assert_ne!(input_key_code::ENTER, input_key_code::POWER);
        assert_ne!(input_key_code::BACKSPACE, input_key_code::POWER);
        assert_ne!(input_key_code::ESCAPE, input_key_code::POWER);
    }

    #[test]
    fn the_shell_types_printable_characters_and_ignores_the_rest() {
        assert_eq!(classify_key(b'a' as u32), ShellKey::Character(b'a'));
        assert_eq!(classify_key(b'Z' as u32), ShellKey::Character(b'Z'));
        assert_eq!(classify_key(b' ' as u32), ShellKey::Character(b' '));
        assert_eq!(classify_key(b'~' as u32), ShellKey::Character(b'~'));
        assert_eq!(classify_key(0), ShellKey::Ignored);
        assert_eq!(classify_key(31), ShellKey::Ignored);
    }

    #[test]
    fn the_banner_claims_no_isolation_state_the_shell_cannot_read() {
        let banner = BANNER.join(" ");
        // WHY: the banner used to read "InterOS Sovereign Shell [Ring 3
        // ISOLATED]" and "Isolated memory, token capabilities enforced." The
        // shell cannot read an isolation state - no syscall reports one.
        assert!(!banner.contains("ISOLATED"), "{}", banner);
        assert!(!banner.contains("Isolated"), "{}", banner);
        assert!(!banner.contains("enforced"), "{}", banner);
        // What it may say: the ring the shell runs in, and the one capability
        // fact it can observe.
        assert!(banner.contains("Ring 3"), "{}", banner);
        assert!(banner.contains("capability types are resolved by"), "{}", banner);
    }

    #[test]
    fn the_help_lists_every_command_the_dispatcher_answers() {
        for command in DISPATCHED_COMMANDS.iter() {
            assert!(
                HELP_COMMANDS.contains(command),
                "the help does not name the {} command",
                command
            );
        }
        // And the help says what the two commands that take an argument take.
        assert!(HELP_DETAIL.contains("ls [path]"));
        assert!(HELP_DETAIL.contains("cat [path]"));
    }

    #[test]
    fn the_capability_list_marks_every_type() {
        let mut held = [0u8; 64];
        let mut not_held = [0u8; 64];
        let n = capability_line(true, "framebuffer", &mut held);
        let m = capability_line(false, "framebuffer", &mut not_held);
        let held_line = core::str::from_utf8(&held[..n]).expect("the line is text");
        let not_held_line = core::str::from_utf8(&not_held[..m]).expect("the line is text");
        // The two must be told apart, or the list reads as a list of names
        // rather than a list of answers.
        assert_ne!(held_line, not_held_line);
        assert!(held_line.contains("held"));
        assert!(!held_line.contains("not held"));
        assert!(not_held_line.contains("not held"));
        // And both name the type they are talking about.
        assert!(held_line.contains("framebuffer"));
        assert!(not_held_line.contains("framebuffer"));
    }
}
