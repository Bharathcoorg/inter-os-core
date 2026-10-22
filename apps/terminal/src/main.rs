//! Inter OS Native Sovereign Intent Terminal App
//!
//! Full production interactive terminal shell executing in Ring 3 (User Mode):
//! - Comprehensive Command Line Interpreter (CLI) supporting:
//!   - `help`: Command documentation and syntax guide
//!   - `status`: Identity banner plus real uptime ticks (`std::get_ticks`)
//!   - `ps`: Task record count as reported by `std::app_runtime_list`
//!   - `mem`: Honest refusal — no heap/paging metrics are exposed to this app
//!   - `caps`: Static capability policy statement (no runtime inspection API)
//!   - `share <mime> <uri> <target>`: Sovereign Share Sheet cross-app bus dispatch
//!   - `dma <pipe> <payload>`: Zero-Copy DMA shared memory pipe streaming
//!   - `ping`: Local loopback diagnostic
//! - DRM/KMS Client Surface Rendering (z=200) & Terminal Canvas Presentation
//! - Circular command history ledger
//! - Banded rendering (`render_terminal_band`): only the rows an input
//!   actually dirtied are re-rasterised, instead of all 1,036,800 pixels on
//!   every event
//! - Asynchronous event reactor loop handling shell intents and touch commands

#![no_std]
#![no_main]

use core::panic::PanicInfo;
use interos_std as std;
use terminal::theme::{radius, spacing, type_scale, typography, Theme, THEME};
use terminal::{
    capability_line, classify_key, ConsoleLine, HistoryEntry, ShellKey, BANNER, CONSOLE_LINES,
    HELP_COMMANDS, HELP_DETAIL, LINE_CAP,
};

use interos_std::design::outline_text;

const W: usize = 720;
const H: usize = 1440;
const CHUNK: usize = 4096;

// -----------------------------------------------------------------------------
// Visual Design Tokens
// -----------------------------------------------------------------------------

// -----------------------------------------------------------------------------
// Embedded 8x8 ASCII Font (Characters 32 ' ' through 126 '~')
// -----------------------------------------------------------------------------
// Typography provided by interos_std::design::font

/// Rasterises one text run into the pixel at `(x, y)`.
///
/// WHY the outline face comes first: the console is drawn at scale 2 and the
/// header title at scale 3, so on the bitmap face both are a 16x24 cell
/// magnified by an integer -- a soft ramp where an edge belongs.
/// `outline_text::place_run` rasterises the real outline at the em that matches
/// the cell's cap height (17 rows at scale 1, so 17*N*2048/1456 px per em), and
/// answers for the pixel itself. When it declines -- a run wider than the
/// coverage buffer, a glyph whose ink would leave this caller's window, or a
/// weight this app draws bold -- the bitmap face below draws the run exactly as
/// it did before, so a refusal costs the edge quality and never the text.
///
/// WHY the run answers alone when it is placed: the two faces are alternatives,
/// not layers. Falling through to the bitmap face after an outline run had
/// already inked a pixel would double-ink every partially covered edge.
#[inline(always)]
fn test_text(
    x: usize,
    y: usize,
    x0: usize,
    y0: usize,
    scale: usize,
    text: &[u8],
    color: u32,
) -> Option<u32> {
    if let Some(run) = outline_text::place_run(x0, y0, scale, text, None, false) {
        return run.pixel(x, y, color, THEME.surface);
    }
    std::design::font::test_text_aa(x, y, x0, y0, scale, text, color, THEME.surface, false)
}

// -----------------------------------------------------------------------------
// Text run geometry
// -----------------------------------------------------------------------------

/// Rows a `scale`-high text run occupies on the shared bitmap face.
/// WHY a local `const fn`: this is exactly `font::line_height(scale)`, but that
/// helper is not `const`, and every run box below is derived from a
/// compile-time constant. Gating a run on anything shorter than this silently
/// slices the glyphs, so the height is computed once and reused for both the
/// gate and the pitch of repeated runs.
const fn run_rows(scale: usize) -> usize {
    std::design::font::line_height(scale)
}

/// Header band height: y >= HEADER_H draws the hairline rule, so a header run
/// has to fit inside 0..HEADER_H.
///
/// WHY 94 and not a token sum: the band is the title's own 72-row line box
/// plus the 22 rows the title is centred in, and the 22 is the header's own
/// padding rather than a step of the platform's spacing scale. The scale has
/// no 22, and inventing one for a single band would be a second scale.
const HEADER_H: usize = 94;
/// Scale of the header title run; it needs run_rows(3) = 72 rows.
const TITLE_SCALE: usize = type_scale::TITLE;
/// First console line box top. The tallest console buffer (22 lines) ends at
/// CONSOLE_TOP + 22 * CONSOLE_LINE_ROWS = 1170, clear of the pill dock at 1210.
const CONSOLE_TOP: usize = 114;
/// Console line pitch == the line box height, so full-height runs stack without
/// overlapping.
///
/// WHY the platform's body line box: the pitch is not an arbitrary number, it
/// is the height of the line the console draws, and the platform's type scale
/// already says how tall a body run is.
const CONSOLE_LINE_ROWS: usize = typography::BODY.line_height_px;
/// Left edge of a console run. WHY 24 and not 28: it is the same left margin the
/// two pill dock rows use, so the console's text column lines up with them, and
/// every extra pixel here is a pixel the wrapped pieces may use.
const CONSOLE_X0: usize = spacing::GRID_24;
/// Right edge of a console run: the surface edge. WHY W and not a named inner
/// edge: a console has no card to sit inside, so the text column runs to the
/// edge of the surface, and a run that reaches it is wrapped, never clipped.
const CONSOLE_X1: usize = W;
/// Console body scale. WHY named: the wrap width and the draw gate below both
/// scale by it, so a change here cannot leave one of them behind.
const CONSOLE_SCALE: usize = type_scale::BODY;

/// System CPL badge box. Grown from the old 176x40 so the scale-2
/// "CPL=3 RING" run (228 x 48) fits inside it with padding instead of being
/// cut at the right and bottom edges. Right edge 700 < 720 and it still clears
/// the title run, which ends at 116 + font::measure(b"Terminal", 3).
const BADGE_X0: usize = 440;
const BADGE_X1: usize = 700;
const BADGE_Y0: usize = 23;
/// The badge's height: the platform's preferred target, and exactly the height
/// of the run it holds.
const BADGE_H: usize = std::design::touch::PREFERRED_TARGET;
/// The badge's corner radius: the platform's soft-pill step, the shape a
/// 48-row control takes when it is rounded but not a capsule.
const BADGE_R: usize = radius::SOFT_PILL;
/// The badge states the shell's isolation, which the app can see, and
/// nothing it cannot.
///
/// WHY the privilege level is gone: the app used to print "CPL=3 RING" and
/// "CPL=3 ISOLATED" from a compile-time constant. It never queried the
/// task's privilege level, so the badge asserted a number it had not
/// observed -- and it contradicted the shell's own `caps` command, which
/// admits that no capability inspection API is exposed.
const BADGE_TEXT: &[u8] = b"RING 3";
const BADGE_SCALE: usize = type_scale::BODY;

/// Quick-command pill geometry. Each pill row is 52 rows tall and its scale-2
/// label is centred in that box, so the label's 48-row box is never clipped.
///
/// WHY the height is derived: the label is a body run, so the box that holds
/// it is the body line box plus a hair of breathing room, and the platform's
/// scale already says how tall a body run is.
const PILL_SCALE: usize = type_scale::BODY;
const PILL1_Y0: usize = 1222;
const PILL1_H: usize = typography::BODY.line_height_px + spacing::GRID_4;
const PILL2_Y0: usize = 1290;
const PILL2_H: usize = typography::BODY.line_height_px + spacing::GRID_4;
/// The quick-command pills' corner radius: the platform's control step, the
/// rounding a button takes.
const PILL_R: usize = radius::CONTROL;

// -----------------------------------------------------------------------------
// Counted Text Assembly (no allocator in this no_std app)
// -----------------------------------------------------------------------------

/// Formats `v` as decimal ASCII into `out`; returns the bytes written.
fn fmt_u64(mut v: u64, out: &mut [u8]) -> usize {
    if out.is_empty() {
        return 0;
    }
    if v == 0 {
        out[0] = b'0';
        return 1;
    }
    let mut digits = [0u8; 20];
    let mut n = 0usize;
    while v > 0 && n < 20 {
        digits[n] = b'0' + (v % 10) as u8;
        v /= 10;
        n += 1;
    }
    let n = n.min(out.len());
    for i in 0..n {
        out[i] = digits[n - 1 - i];
    }
    n
}

/// Builds `prefix ++ decimal(value) ++ suffix` into `buf` and returns its length.
/// WHY: measured values (task counts, ticks, dispatch tokens) only exist at
/// runtime, so they are assembled into a stack buffer before being displayed.
fn build_counted(buf: &mut [u8], prefix: &[u8], value: u64, suffix: &[u8]) -> usize {
    let p = prefix.len().min(buf.len());
    buf[..p].copy_from_slice(&prefix[..p]);
    let v = fmt_u64(value, &mut buf[p..]);
    let n = p + v;
    let s = suffix.len().min(buf.len().saturating_sub(n));
    buf[n..n + s].copy_from_slice(&suffix[..s]);
    n + s
}

/// Logs `prefix ++ decimal(value) ++ suffix` as one kernel log line.
fn log_counted(prefix: &str, value: u64, suffix: &str) {
    let mut buf = [0u8; 192];
    let n = build_counted(&mut buf, prefix.as_bytes(), value, suffix.as_bytes());
    if let Ok(msg) = core::str::from_utf8(&buf[..n]) {
        std::log(msg).ok();
    }
}

/// Reads the hardware tick counter, probing each capability slot for one that
/// holds `TimerTickRead` (WHY: the slot holding that capability varies per task).
fn probe_ticks() -> Result<u64, i64> {
    // WHY there is one call and not a loop over slots: the kernel
    // authorises the tick counter by the capability's type, so the
    // shell names the authority it wants and the kernel finds it.
    std::get_ticks()
}

/// Counts the newline-terminated task records `app_runtime_list` returned,
/// passing the syscall's own error code through when the query is refused.
/// WHY: the syscall reports a byte length, not a task count, so records are
/// counted from the payload; a truncated trailing record that never received
/// its terminator is not counted as a whole task.
fn count_runtime_tasks() -> Result<usize, i64> {
    let mut buf = [0u8; 2048];
    let n = std::app_runtime_list(&mut buf)?;
    let payload = &buf[..n.min(buf.len())];
    let mut count = 0usize;
    let mut in_record = false;
    for b in payload.iter() {
        if *b == b'\n' {
            if in_record {
                count += 1;
            }
            in_record = false;
        } else {
            in_record = true;
        }
    }
    Ok(count)
}

/// The shell's prompt.
///
/// WHY it names nothing but the shell: the old prompt carried a user account
/// and a host name, which asserted an identity the shell had never read. The
/// shell knows its own name and nothing else, so the prompt says that and
/// stops.
pub const PROMPT: &[u8] = b"interos:$ ";

/// The directory `ls` lists when the user does not name one.
///
/// WHY the storage root: the kernel's app path policy refuses `/data`
/// before the path is examined, so listing it could only ever produce a
/// refusal dressed up as an empty directory.
const LS_PATH: &str = "/storage";

/// The capability types this app knows how to ask for.
///
/// WHY a fixed list: the platform resolves a capability slot by its type
/// but offers no way to enumerate a task's whole table, so the most the
/// shell can report is which of the types it knows about it holds. The
/// list is the app's own table, and the `caps` command says so.
pub const KNOWN_CAPABILITIES: [(&str, u64); 8] = [
    ("serial output", std::cap_type::SERIAL_OUTPUT),
    ("ipc send", std::cap_type::IPC_SEND),
    ("timer tick read", std::cap_type::TIMER_TICK_READ),
    ("filesystem read", std::cap_type::FILE_SYSTEM_READ),
    ("filesystem write", std::cap_type::FILE_SYSTEM_WRITE),
    ("event poll", std::cap_type::EVENT_POLL),
    ("ipc receive", std::cap_type::IPC_RECEIVE),
    ("framebuffer", std::cap_type::FRAMEBUFFER_DIRECT),
];

/// Sovereign Intent Terminal Shell state engine.
pub struct TerminalApp {
    pub history: [Option<HistoryEntry>; 8],
    pub history_count: usize,
    pub total_commands_executed: u32,
    pub lines: [Option<ConsoleLine>; CONSOLE_LINES],
    pub line_count: usize,
    pub input_buf: [u8; 64],
    pub input_len: usize,
}

impl TerminalApp {
    pub const fn new() -> Self {
        let mut lines = [None; CONSOLE_LINES];
        // WHY these are pre-wrapped by hand: `new` is a `const fn`, so it cannot
        // call the wrapping `push_line` (the wrapper takes &mut self). The
        // banner strings are the only lines the app can open with, so
        // wrapping them here instead keeps the boot screen as wrapped as
        // every later line.
        //
        // WHY the banner says what the shell can see and nothing else: it
        // used to read "InterOS Sovereign Shell [Ring 3 ISOLATED]" and
        // "Isolated memory, token capabilities enforced." The shell cannot
        // read an isolation state - no syscall reports one - so the banner
        // asserted a property it had never observed, and the second line
        // claimed the platform enforces capabilities without the shell
        // having seen a refusal or a grant. The banner now names the shell's
        // privilege ring, which is a fact about where a Ring 3 app runs, and
        // the one capability fact the shell can observe: the platform
        // resolves a capability by its type.
        lines[0] = Some(ConsoleLine::from_static(BANNER[0], THEME.on_surface_variant));
        lines[1] = Some(ConsoleLine::from_static(BANNER[1], THEME.on_surface_variant));
        lines[2] = Some(ConsoleLine::from_static(BANNER[2], THEME.primary));
        lines[3] = Some(ConsoleLine::from_static(BANNER[3], THEME.primary));
        lines[4] = Some(ConsoleLine::from_static(BANNER[4], THEME.on_surface_variant));
        lines[5] = Some(ConsoleLine::from_static(BANNER[5], THEME.on_surface_variant));
        lines[6] = Some(ConsoleLine::from_static(
            "interos:$ _",
            THEME.success,
        ));
        Self {
            history: [None::<HistoryEntry>; 8],
            history_count: 0,
            total_commands_executed: 0,
            lines,
            line_count: 7,
            input_buf: [0; 64],
            input_len: 0,
        }
    }

    pub fn push_line(&mut self, text: &str, color: u32) {
        self.push_line_bytes(text.as_bytes(), color);
    }

    /// Echoes a command the way a shell does: the prompt, then the command.
    pub fn push_echo(&mut self, cmd: &str) {
        let mut line = [0u8; 64];
        let p = PROMPT.len();
        if p > line.len() {
            return;
        }
        line[..p].copy_from_slice(PROMPT);
        let n = p + cmd.len().min(line.len() - p);
        line[p..n].copy_from_slice(&cmd.as_bytes()[..n - p]);
        self.push_line_bytes(&line[..n], THEME.success);
    }

    pub fn push_line_bytes(&mut self, text: &[u8], color: u32) {
        // WHY wrap here rather than draw fitted: the console is a per-pixel
        // painter whose type helper applies no right-edge clip, so a run wider
        // than the console column is not cut at the column - it runs to the edge
        // of the surface and is severed there, which is what every long line of
        // this app's own output used to do ("...token capabilities e"). Fitting
        // each line with an ellipsis would hide the tail instead of showing it,
        // so the text is wrapped onto as many rows as it needs, the way a shell
        // does, and no piece is ever wider than the column.
        let mut rest = text;
        while !rest.is_empty() {
            let mut piece: [&[u8]; 1] = [&[]];
            let n = std::design::font::wrap_text(rest, CONSOLE_SCALE, CONSOLE_X1 - CONSOLE_X0, 1, &mut piece);
            // WHY the guard: wrap_text returns 0 only for empty input, and a
            // non-empty piece is what advances the cursor, so a zero here would
            // spin the loop forever.
            if n == 0 || piece[0].is_empty() {
                break;
            }
            // WHY the offset is computed and not taken from the piece's
            // length: `wrap_text` skips the leading spaces of the text it is
            // given, so the piece it returns starts after them. Advancing by
            // the piece's length therefore restarted the remainder one
            // character too early, and every wrapped line repeated the first
            // character of the next one.
            let mut skip = 0usize;
            while skip < rest.len() && rest[skip] == b' ' {
                skip += 1;
            }
            let consumed = skip + piece[0].len();
            if consumed == 0 || consumed > rest.len() {
                break;
            }
            self.push_console_line(piece[0], color);
            rest = &rest[consumed..];
        }
    }

    /// Appends one already-wrapped piece as the newest console row, scrolling the
    /// oldest one off the top when the buffer is full.
    fn push_console_line(&mut self, text: &[u8], color: u32) {
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

    /// Pushes `prefix ++ decimal(value) ++ suffix` as a single console line.
    pub fn push_counted(&mut self, prefix: &str, value: u64, suffix: &str, color: u32) {
        let mut buf = [0u8; LINE_CAP];
        let n = build_counted(&mut buf, prefix.as_bytes(), value, suffix.as_bytes());
        self.push_line_bytes(&buf[..n], color);
    }

    /// Lists a directory the user named.
    ///
    /// WHY the path is reported in the output: the shell does not know what
    /// the user meant to list, and a line that does not name the path could
    /// be read as a claim about a different directory.
    pub fn list_path(&mut self, path: &str) {
        if path.is_empty() {
            self.push_line("ls: needs a path, e.g. ls /storage", THEME.on_surface_variant);
            self.record_history("ls", -1);
            return;
        }
        let mut buf = [0u8; 512];
        let listed = std::fs_list(path, &mut buf);
        match listed {
            Ok(n) if n > 0 => {
                self.push_counted("vfs: ", n as u64, " bytes listed.", THEME.success);
                let mut start = 0;
                while start < n {
                    let end = buf[start..n]
                        .iter()
                        .position(|&b| b == b'\n')
                        .map(|p| start + p)
                        .unwrap_or(n);
                    if let Ok(entry_str) = core::str::from_utf8(&buf[start..end]) {
                        if !entry_str.is_empty() {
                            self.push_line(entry_str, THEME.on_surface);
                        }
                    }
                    start = end.saturating_add(1);
                }
                self.record_history("ls", 0);
            }
            Ok(_) => {
                self.push_line("vfs: directory empty.", THEME.on_surface_variant);
                self.record_history("ls", 0);
            }
            Err(code) => {
                self.push_line("vfs: fs_list query refused or unavailable.", THEME.on_surface_variant);
                self.record_history("ls", code);
            }
        }
    }

    /// Prints a file the user named.
    pub fn cat_path(&mut self, path: &str) {
        if path.is_empty() {
            self.push_line("cat: needs a path, e.g. cat /storage/photos/name.ppm", THEME.on_surface_variant);
            self.record_history("cat", -1);
            return;
        }
        match std::fs_open(path, std::FS_READ) {
            Ok(fd) => {
                let mut buf = [0u8; 512];
                match std::fs_read(fd, &mut buf) {
                    Ok(n) if n > 0 => {
                        self.push_counted("cat: ", n as u64, " bytes read.", THEME.success);
                        let mut start = 0;
                        while start < n {
                            let end = buf[start..n]
                                .iter()
                                .position(|&b| b == b'\n')
                                .map(|p| start + p)
                                .unwrap_or(n);
                            if let Ok(line_str) = core::str::from_utf8(&buf[start..end]) {
                                if !line_str.is_empty() {
                                    self.push_line(line_str, THEME.on_surface);
                                }
                            }
                            start = end.saturating_add(1);
                        }
                        self.record_history("cat", 0);
                    }
                    Ok(_) => {
                        self.push_line("cat: file empty.", THEME.on_surface_variant);
                        self.record_history("cat", 0);
                    }
                    Err(code) => {
                        self.push_line("cat: fs_read refused or error.", THEME.on_surface_variant);
                        self.record_history("cat", code);
                    }
                }
                let _ = std::fs_close(fd);
            }
            Err(code) => {
                self.push_line("cat: fs_open refused or file not found.", THEME.on_surface_variant);
                self.record_history("cat", code);
            }
        }
    }

    /// Records an executed command in the history ledger.
    pub fn record_history(&mut self, cmd: &'static str, status: i64) {        let idx = self.history_count % self.history.len();
        self.history[idx] = Some(HistoryEntry { cmd, status });
        self.history_count += 1;
        self.total_commands_executed += 1;
    }

    /// Updates the terminal prompt line to display the current input buffer contents with cursor.
    fn sync_prompt_line(&mut self) {
        let mut buf = [0u8; LINE_CAP];
        let prefix = PROMPT;
        buf[..prefix.len()].copy_from_slice(prefix);
        let n = self.input_len.min(LINE_CAP - prefix.len() - 1);
        buf[prefix.len()..prefix.len() + n].copy_from_slice(&self.input_buf[..n]);
        buf[prefix.len() + n] = b'_';
        let total_len = prefix.len() + n + 1;

        let mut line = ConsoleLine {
            text: [0; LINE_CAP],
            len: total_len,
            color: THEME.success,
        };
        line.text[..total_len].copy_from_slice(&buf[..total_len]);

        if self.line_count > 0
            && self.lines[self.line_count - 1].map_or(false, |l| l.as_bytes().starts_with(b"interos:"))
        {
            self.lines[self.line_count - 1] = Some(line);
        } else if self.line_count < CONSOLE_LINES {
            self.lines[self.line_count] = Some(line);
            self.line_count += 1;
        } else {
            for i in 0..CONSOLE_LINES - 1 {
                self.lines[i] = self.lines[i + 1];
            }
            self.lines[CONSOLE_LINES - 1] = Some(line);
        }
    }

    /// Handles keyboard input events routed to this terminal.
    pub fn handle_key(&mut self, key: std::InputKeyEvent) -> bool {
        if !key.pressed {
            return false;
        }

        if key.ctrl {
            if key.code == b'c' as u32 || key.code == b'C' as u32 || key.code == 3 {
                self.input_len = 0;
                if self.line_count > 0
                    && self.lines[self.line_count - 1].map_or(false, |l| l.as_bytes().starts_with(b"interos:"))
                {
                    self.line_count -= 1;
                }
                self.push_echo("^C");
                self.push_line("interos:$ _", THEME.success);
                return true;
            }
            if key.code == b'l' as u32 || key.code == b'L' as u32 || key.code == 12 {
                self.lines = [None; CONSOLE_LINES];
                self.line_count = 0;
                self.sync_prompt_line();
                return true;
            }
            if key.code == b'u' as u32 || key.code == b'U' as u32 || key.code == 21 {
                self.input_len = 0;
                self.sync_prompt_line();
                return true;
            }
        }

        match classify_key(key.code) {
            ShellKey::Enter => {
                let mut cmd_buf = [0u8; 64];
                let len = self.input_len.min(64);
                cmd_buf[..len].copy_from_slice(&self.input_buf[..len]);
                self.input_len = 0;
                if let Ok(cmd_str) = core::str::from_utf8(&cmd_buf[..len]) {
                    self.execute_command(cmd_str);
                } else {
                    self.execute_command("");
                }
                true
            }
            ShellKey::Backspace => {
                if self.input_len > 0 {
                    self.input_len -= 1;
                    self.sync_prompt_line();
                    true
                } else {
                    false
                }
            }
            ShellKey::Escape => {
                if self.input_len > 0 {
                    self.input_len = 0;
                    self.sync_prompt_line();
                    true
                } else {
                    false
                }
            }
            ShellKey::Character(code) => {
                if self.input_len < self.input_buf.len() {
                    self.input_buf[self.input_len] = code;
                    self.input_len += 1;
                    self.sync_prompt_line();
                    true
                } else {
                    false
                }
            }
            ShellKey::Ignored => false,
        }
    }

    /// Executes a built-in terminal command and renders to console.
    pub fn execute_command(&mut self, cmd_line: &str) {
        if self.line_count > 0
            && self.lines[self.line_count - 1].map_or(false, |l| l.as_bytes().starts_with(b"interos:"))
        {
            self.line_count -= 1;
        }

        let cmd = cmd_line.trim();

        // `cat <path>` and `ls <path>` take an argument, so they are handled
        // before the verb match: the verb alone does not say what to read or
        // where to look.
        //
        // WHY the path is the user's to give: the kernel's app path policy
        // refuses `/data` before the path is examined, so a hard-coded target
        // could only ever produce a refusal. Naming a path the app is
        // allowed to read is what makes the command useful.
        if let Some(arg) = cmd.strip_prefix("cat ") {
            self.push_echo(cmd);
            self.cat_path(arg.trim());
            return;
        }
        if let Some(arg) = cmd.strip_prefix("ls ") {
            self.push_echo(cmd);
            self.list_path(arg.trim());
            return;
        }

        match cmd {
            "help" => {
                self.push_echo("help");
                // WHY every command is listed: the dispatcher also answers
                // `vfs` and `clear`, and the old list left them out, so the
                // help was a worse description of the shell than the shell
                // itself.
                self.push_line(HELP_COMMANDS, THEME.on_surface);
                self.push_line(HELP_DETAIL, THEME.on_surface_variant);
                std::log("[TERMINAL] Commands: help, status, ps, mem, caps, share, dma, ping, ls, vfs, services, cat, clear",
                )
                .ok();
                self.record_history("help", 0);
            }
            "status" => {
                self.push_echo("status");
                // WHY the isolation word is gone: the shell never queries
                // the task's CPL -- no syscall reports it to a Ring 3 app --
                // and nothing reports an isolation state either, so naming
                // one asserted a property the shell had not observed. The
                // shell's own `caps` command reports what it can resolve.
                self.push_line("InterOS Core Mobile Platform | Ring 3", THEME.primary);
                // WHY: the uptime comes from the platform's real tick
                // counter. If no capability slot grants TimerTickRead, the
                // screen says so instead of inventing a metric.
                match probe_ticks() {
                    Ok(ticks) => {
                        self.push_counted(
                            "status: uptime ",
                            ticks,
                            // WHY the rate is not printed: the tick counter
                            // reports a count, never a frequency. Claiming
                            // "1 kHz" asserted a rate the app cannot
                            // measure.
                            " ticks since boot.",
                            THEME.on_surface,
                        );
                        log_counted(
                            "[TERMINAL] Status: uptime ",
                            ticks,
                            " ticks read from the hardware timer.",
                        );
                        self.record_history("status", 0);
                    }
                    Err(code) => {
                        self.push_line("status: uptime unavailable; get_ticks refused.", THEME.on_surface_variant);
                        log_counted(
                            "[TERMINAL] Status: uptime unavailable, get_ticks error ",
                            code.unsigned_abs(),
                            ".",
                        );
                        self.record_history("status", code);
                    }
                }
            }
            "ps" => {
                self.push_echo("ps");
                // WHY: the task count is read from the runtime list; a fixed
                // number would claim a measurement that was never taken.
                match count_runtime_tasks() {
                    Ok(count) => {
                        self.push_counted(
                            "ps: ",
                            count as u64,
                            " other tasks (app_runtime_list).",
                            THEME.on_surface,
                        );
                        log_counted(
                            "[TERMINAL] ps: app_runtime_list reported ",
                            count as u64,
                            " task records.",
                        );
                        self.record_history("ps", 0);
                    }
                    Err(code) => {
                        self.push_line("ps: unavailable; app_runtime_list refused.", THEME.on_surface_variant);
                        log_counted(
                            "[TERMINAL] ps: app_runtime_list refused, error ",
                            code.unsigned_abs(),
                            "; task count unavailable.",
                        );
                        self.record_history("ps", code);
                    }
                }
            }
            "mem" => {
                self.push_echo("mem");
                // WHY: no heap or paging query exists on this platform, so the
                // command reports the absence instead of a plausible-sounding stat.
                // History records -1: no syscall ran, so there is no kernel code —
                // -1 marks "no data available".
                self.push_line("mem: unavailable; no heap or paging", THEME.on_surface_variant);
                self.push_line("metrics are exposed to this app.", THEME.on_surface_variant);
                std::log("[TERMINAL] mem: no heap/paging metrics are exposed; nothing was queried.").ok();
                self.record_history("mem", -1);
            }
            "caps" => {
                self.push_echo("caps");
                // WHY this is an inspection and not a statement: the platform
                // resolves a capability slot by its type, so the shell can ask
                // which of the capabilities it knows about it actually holds.
                // It cannot enumerate the whole table -- no syscall does that
                // -- so the list below is the set of types this app knows
                // about, and every line says which of them the shell holds.
                //
                // WHY every line is marked: the list used to name only the
                // types the shell held, so a reader saw a bare list of names
                // and a pair of counts at the bottom, and the names read as
                // the shell's whole capability set. It is a subset of the
                // types the app knows about, and it now says so on every
                // line.
                self.push_line("caps: the types this app knows about:", THEME.on_surface);
                let mut held = 0u32;
                let mut absent = 0u32;
                let mut buf = [0u8; 64];
                for (name, cap_type) in KNOWN_CAPABILITIES.iter() {
                    let is_held = std::cap_lookup(*cap_type, 0).is_ok();
                    let n = capability_line(is_held, name, &mut buf);
                    self.push_line_bytes(
                        &buf[..n],
                        if is_held {
                            THEME.on_surface
                        } else {
                            THEME.on_surface_variant
                        },
                    );
                    if is_held {
                        held += 1;
                    } else {
                        absent += 1;
                    }
                }
                self.push_counted("caps: held ", held as u64, " of the types this app knows about.", THEME.on_surface_variant);
                self.push_counted("caps: not held ", absent as u64, " of them.", THEME.on_surface_variant);
                std::log("[TERMINAL] caps: capability slots resolved by type from the app's own table.",
                )
                .ok();
                self.record_history("caps", 0);
            }
            "share" => {
                self.push_echo("share");
                // WHY: the dispatch Result decides the message — a refused intent
                // is reported as refused, never as a success.
                match std::share_sheet_dispatch(
                    "text/uri-list",
                    "interos://files/documents",
                    Some("interos.terminal"),
                ) {
                    Ok(token) => {
                        self.push_counted("Share: accepted, token ", token, ".", THEME.primary);
                        log_counted("[TERMINAL APP] Share: dispatch accepted, token ", token, ".");
                        self.record_history("share", 0);
                    }
                    Err(code) => {
                        self.push_counted(
                            "Share: dispatch refused, code -",
                            code.unsigned_abs(),
                            ".",
                            THEME.on_surface_variant,
                        );
                        log_counted(
                            "[TERMINAL APP] Share: dispatch refused, code -",
                            code.unsigned_abs(),
                            ".",
                        );
                        self.record_history("share", code);
                    }
                }
            }
            "dma" => {
                self.push_echo("dma");
                let payload = b"DMA::TERMINAL::STREAM_PACKET";
                match std::shm_pipe_transfer(3, payload, 0x01) {
                    Ok(_) => {
                        self.push_line("DMA: test payload transfer acknowledged by Pipe 3.", THEME.success);
                        std::log("[TERMINAL APP] DMA test payload transfer acknowledged by Pipe 3.").ok();
                        self.record_history("dma", 0);
                    }
                    Err(code) => {
                        self.push_line("DMA: Pipe 3 transfer unavailable or refused.", THEME.on_surface_variant);
                        std::log("[TERMINAL APP] DMA test payload transfer failed or was refused.").ok();
                        self.record_history("dma", code);
                    }
                }
            }
            "ping" => {
                self.push_echo("ping");
                self.push_line("ping: unavailable; no ICMP/network probe is implemented.", THEME.on_surface_variant);
                std::log("[TERMINAL APP] ping unavailable: no network probe was executed.").ok();
                self.record_history("ping", 0);
            }
            "services" => {
                self.push_echo("services");
                let mut buf = [0u8; 512];
                match std::service_list(&mut buf) {
                    Ok(n) => {
                        self.push_counted("services: snapshot returned ", n as u64, " bytes.", THEME.on_surface);
                        std::log("[TERMINAL APP] services command executed.").ok();
                        self.record_history("services", 0);
                    }
                    Err(code) => {
                        self.push_line("services: query refused or unavailable.", THEME.on_surface_variant);
                        self.record_history("services", code);
                    }
                }
            }
            "ls" | "vfs" => {
                self.push_echo("ls");
                // WHY the storage root and not the data directory: the
                // kernel's app path policy refuses `/data` before the path is
                // examined, so listing it could only ever produce a refusal
                // dressed up as an empty directory. `/storage` is a path this
                // app is allowed to list.
                self.list_path(LS_PATH);
            }
            "cat" => {
                self.push_echo("cat");
                self.push_line("cat: needs a path, e.g. cat /storage/photos/name.ppm", THEME.on_surface_variant);
                self.record_history("cat", -1);
            }
            "clear" => {
                self.lines = [None; CONSOLE_LINES];
                self.line_count = 0;
                self.record_history("clear", 0);
            }
            "" => {
                self.push_echo("");
            }
            unknown => {
                let mut pbuf = [0u8; LINE_CAP];
                let prefix = PROMPT;
                pbuf[..prefix.len()].copy_from_slice(prefix);
                let ubytes = unknown.as_bytes();
                let un = ubytes.len().min(LINE_CAP - prefix.len());
                pbuf[prefix.len()..prefix.len() + un].copy_from_slice(&ubytes[..un]);
                self.push_line_bytes(&pbuf[..prefix.len() + un], THEME.success);

                let mut err_buf = [0u8; LINE_CAP];
                let err_pre = b"terminal: command not found: ";
                err_buf[..err_pre.len()].copy_from_slice(err_pre);
                let en = ubytes.len().min(LINE_CAP - err_pre.len());
                err_buf[err_pre.len()..err_pre.len() + en].copy_from_slice(&ubytes[..en]);
                self.push_line_bytes(&err_buf[..err_pre.len() + en], THEME.on_surface_variant);

                self.record_history("unknown", -1);
            }
        }
        self.push_line("interos:$ _", THEME.success);
    }
}

#[inline(never)]
fn get_terminal_pixel(x: usize, y: usize, term: &TerminalApp, scanline_wash: u32) -> u32 {
    // 1. Status Bar & Header (y: 0..HEADER_H)
    if y < HEADER_H {
        if y >= 94 {
            return THEME.border;
        }

        // Standardized back button squircle (24..72, 24..72)
        if let Some(c) = std::design::back_button::pixel(x, y, THEME.on_surface, THEME.border, THEME.surface_container_high) {
            return c;
        }

        // Header Title: "Terminal". The run is scale 3 and needs
        // run_rows(3) = 72 rows, so its line box is centred in the 94-row
        // header band instead of being gated to a 24-row window that kept
        // only a third of the glyphs.
        let title_rows = run_rows(TITLE_SCALE);
        let title_y0 = (HEADER_H - title_rows) / 2;
        if y >= title_y0 && y < title_y0 + title_rows {
            if let Some(c) = test_text(x, y, 116, title_y0, TITLE_SCALE, b"Terminal", THEME.on_surface) {
                return c;
            }
        }

        // System CPL=3 Badge (x: BADGE_X0..BADGE_X1, y: BADGE_Y0..)
        if x >= BADGE_X0 && x < BADGE_X1 && y >= BADGE_Y0 && y < BADGE_Y0 + BADGE_H {
            let rx = (x - BADGE_X0).min(BADGE_X1 - 1 - x);
            let ry = (y - BADGE_Y0).min(BADGE_Y0 + BADGE_H - 1 - y);
            if rx < BADGE_R && ry < BADGE_R {
                let cdx = (BADGE_R - rx) as isize;
                let cdy = (BADGE_R - ry) as isize;
                if cdx * cdx + cdy * cdy >= (BADGE_R * BADGE_R) as isize {
                    return THEME.surface;
                }
            }

            // Centre the label in the badge box with real proportional
            // advances instead of a fixed 8px cell, so the whole run lands
            // inside the capsule horizontally and vertically.
            let badge_label_x0 =
                BADGE_X0 + std::design::font::center_x(BADGE_TEXT, BADGE_SCALE, BADGE_X1 - BADGE_X0);
            let badge_label_y0 = BADGE_Y0 + (BADGE_H - run_rows(BADGE_SCALE)) / 2;
            if let Some(c) = test_text(
                x,
                y,
                badge_label_x0,
                badge_label_y0,
                BADGE_SCALE,
                BADGE_TEXT,
                THEME.success,
            ) {
                return c;
            }

            if rx < 2 || ry < 2 {
                return THEME.success;
            }
            return THEME.surface_container_high;
        }

        return THEME.surface;
    }

    // 2. Terminal Console Canvas (y: 96..1200)
    if y >= 96 && y < 1200 {
        // Render console lines.
        //
        // A console run is scale 2, so its line box is CONSOLE_LINE_ROWS = 48
        // rows. The old gate admitted 16 of those rows (a third of the glyphs)
        // while the old 32px pitch would have overlapped full boxes, so the
        // pitch and the gate are now the same row count: runs stack without
        // intersecting. 22 lines end at 114 + 22 * 48 = 1170, clear of the
        // quick-command dock that starts at y = 1210.
        //
        // WHY no right-edge test on the run: the buffer only holds pieces that
        // `push_line_bytes` already wrapped to CONSOLE_X1 - CONSOLE_X0, so every
        // run fits the column by construction and a clip here would only hide a
        // bug rather than fix one.
        for line_idx in 0..term.line_count {
            let line_y0 = CONSOLE_TOP + line_idx * CONSOLE_LINE_ROWS;
            if y >= line_y0 && y < line_y0 + CONSOLE_LINE_ROWS {
                if let Some(line) = term.lines[line_idx] {
                    if let Some(c) = test_text(
                        x,
                        y,
                        CONSOLE_X0,
                        line_y0,
                        CONSOLE_SCALE,
                        line.as_bytes(),
                        line.color,
                    ) {
                        return c;
                    }
                }
            }
        }

        // Subtle CRT scanline effect
        if y % 3 == 0 {
            return scanline_wash;
        }
        return THEME.surface;
    }

    // 3. Quick Command Pills Dock (y: 1210..1370)
    if y >= 1210 && y < 1370 {
        if y == 1210 {
            return THEME.border;
        }

        // Row 1 (y: 1222..1274)
        if (PILL1_Y0..PILL1_Y0 + PILL1_H).contains(&y) {
            // Scale-2 label needs run_rows(2) = 48 rows; centre it in the
            // 52-row pill so its descenders are not cut by the pill bottom.
            let label_y0 = PILL1_Y0 + (PILL1_H - run_rows(PILL_SCALE)) / 2;
            let pills = [
                (24, 176, &b"help"[..]),
                (194, 346, &b"status"[..]),
                (364, 516, &b"ps"[..]),
                (534, 696, &b"mem"[..]),
            ];
            for (x0, x1, label) in pills {
                if x >= x0 && x < x1 {
                    let rx = (x - x0).min(x1 - 1 - x);
                    let ry = (y - PILL1_Y0).min(PILL1_Y0 + PILL1_H - 1 - y);
                    if rx < PILL_R && ry < PILL_R {
                        let cdx = (PILL_R - rx) as isize;
                        let cdy = (PILL_R - ry) as isize;
                        if cdx * cdx + cdy * cdy >= (PILL_R * PILL_R) as isize {
                            return THEME.surface_container;
                        }
                    }

                    // Real proportional advance per label instead of a fixed
                    // 8px-per-character cell, centred in this pill's own width.
                    let tx = x0 + std::design::font::center_x(label, PILL_SCALE, x1 - x0);
                    if let Some(c) = test_text(x, y, tx, label_y0, PILL_SCALE, label, THEME.primary) {
                        return c;
                    }

                    if rx < 2 || ry < 2 {
                        return THEME.border;
                    }
                    return THEME.surface_container_high;
                }
            }
        }

        // Row 2 (y: 1290..1342)
        if (PILL2_Y0..PILL2_Y0 + PILL2_H).contains(&y) {
            let label_y0 = PILL2_Y0 + (PILL2_H - run_rows(PILL_SCALE)) / 2;
            let pills = [
                (24, 176, &b"caps"[..]),
                (194, 346, &b"ping"[..]),
                (364, 516, &b"dma"[..]),
                (534, 696, &b"share"[..]),
            ];
            for (x0, x1, label) in pills {
                if x >= x0 && x < x1 {
                    let rx = (x - x0).min(x1 - 1 - x);
                    let ry = (y - PILL2_Y0).min(PILL2_Y0 + PILL2_H - 1 - y);
                    if rx < PILL_R && ry < PILL_R {
                        let cdx = (PILL_R - rx) as isize;
                        let cdy = (PILL_R - ry) as isize;
                        if cdx * cdx + cdy * cdy >= (PILL_R * PILL_R) as isize {
                            return THEME.surface_container;
                        }
                    }

                    let tx = x0 + std::design::font::center_x(label, PILL_SCALE, x1 - x0);
                    if let Some(c) = test_text(x, y, tx, label_y0, PILL_SCALE, label, THEME.primary) {
                        return c;
                    }

                    if rx < 2 || ry < 2 {
                        return THEME.border;
                    }
                    return THEME.surface_container_high;
                }
            }
        }

        return THEME.surface_container;
    }

    // Home Gesture Bar
    if y >= 1418 && y < 1424 && x >= 260 && x < 460 {
        return THEME.on_surface_variant;
    }

    THEME.surface
}

/// The CRT scanline wash: the page colour, darkened.
///
/// WHY it is derived and not a colour of its own: the wash is the page's own
/// colour with a third of its light taken out, so it follows the theme
/// instead of being a second page colour the platform does not name. It is
/// computed once per frame because it is a per-frame constant and the
/// renderer asks for it a third of a million times.
fn scanline_wash() -> u32 {
    Theme::veil(THEME.surface, 37, false)
}

fn render_terminal_ui(surface: u32, term: &TerminalApp) {
    let wash = scanline_wash();
    let mut chunk = [0u32; CHUNK];
    let total = W * H;
    let mut offset = 0;
    while offset < total {
        let count = (total - offset).min(CHUNK);
        let mut x = offset % W;
        let mut y = offset / W;
        for i in 0..count {
            chunk[i] = get_terminal_pixel(x, y, term, wash);
            x += 1;
            if x == W {
                x = 0;
                y += 1;
            }
        }
        let _ = std::surface_write(surface, offset, &chunk[..count]);
        offset += count;
    }
}

// -----------------------------------------------------------------------------
// Screen band - the single source of truth for the `Dirty` -> band mapping
// used by the input path below.
// -----------------------------------------------------------------------------

/// Rows the console owns: the first line box through the last of the buffer,
/// `CONSOLE_TOP + CONSOLE_LINES * CONSOLE_LINE_ROWS` = 1170.
///
/// WHY the buffer's extent and not the 96..1200 canvas the renderer gates on:
/// the rows below the last line box only ever hold the background and the CRT
/// scanline wash, and neither is a function of state, so they are left exactly
/// as drawn instead of being re-rasterised for a command that cannot reach
/// them.
const CONSOLE_Y0: usize = CONSOLE_TOP;
const CONSOLE_Y1: usize = CONSOLE_TOP + CONSOLE_LINES * CONSOLE_LINE_ROWS;

/// Re-rasterises and writes only rows `y0..y1`.
///
/// WHY a band and not a rectangle: a row of the surface is one contiguous run in
/// the surface buffer, so `offset = y0 * W` writes a whole band with no stride
/// bookkeeping. Every band starts on a row boundary, so the running `x`/`y`
/// cursor below is exact.
///
/// WHY this is correct rather than a shortcut: the band is named after what the
/// input actually moved (see `Dirty`), and every row outside it is correct
/// because nothing about it changed. The old renderer re-rasterised all
/// 1,036,800 pixels on every touch event, including touches that ran no command
/// at all.
fn render_terminal_band(surface: u32, term: &TerminalApp, y0: usize, y1: usize) {
    let wash = scanline_wash();
    let mut chunk = [0u32; CHUNK];
    let mut offset = y0 * W;
    let end = y1.min(H) * W;
    while offset < end {
        let count = (end - offset).min(CHUNK);
        let mut x = offset % W;
        let mut y = offset / W;
        for i in 0..count {
            chunk[i] = get_terminal_pixel(x, y, term, wash);
            x += 1;
            if x == W {
                x = 0;
                y += 1;
            }
        }
        let _ = std::surface_write(surface, offset, &chunk[..count]);
        offset += count;
    }
}

// -----------------------------------------------------------------------------
// Input handling
// -----------------------------------------------------------------------------

/// Which rows of the surface an input actually changed.
///
/// WHY this exists: the old loop called `render_terminal_ui` after every tap,
/// so a touch on blank canvas cost the same 1,036,800-pixel repaint as a
/// command that scrolled the whole console.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Dirty {
    /// Nothing moved; the surface is already correct, so do not render at all.
    None,
    /// A quick command ran and pushed console lines: the console band only.
    ///
    /// WHY there is no band for the header or the pill dock: the title, the
    /// CPL badge and every pill label are compile-time strings, and the
    /// command ledger is never drawn, so a command cannot move a pixel outside
    /// the console.
    Console,
    /// More than one band moved, or the first frame: the whole surface.
    All,
}

/// Grow a dirty region so one rendered frame covers everything a poll batch
/// changed.
///
/// WHY: `event_poll` can deliver several touches before the app is scheduled
/// again; rendering only the last region would leave the earlier one stale.
impl Dirty {
    fn widen(self, other: Dirty) -> Dirty {
        match (self, other) {
            (Dirty::None, d) | (d, Dirty::None) => d,
            (a, b) if a == b => a,
            _ => Dirty::All,
        }
    }
}

/// Maps a `Dirty` onto the band it re-rasterises.
fn repaint(surface: u32, term: &TerminalApp, dirty: Dirty) {
    match dirty {
        Dirty::None => {}
        Dirty::Console => render_terminal_band(surface, term, CONSOLE_Y0, CONSOLE_Y1),
        Dirty::All => render_terminal_ui(surface, term),
    }
}

#[no_mangle]
pub extern "C" fn _start() -> ! {
    std::log("[TERMINAL APP (Ring 3)] InterOS Developer Terminal online.",
    )
    .ok();

    // WHY the slot is resolved by type: a slot number is a position in the
    // capability table, and the position depends on the order the kernel
    // inserted this app's capabilities in.
    let surface = match std::surface_create(W as u32, H as u32, 200) {
        Ok(id) => {
            std::log("[TERMINAL APP] DRM/KMS Client Surface allocated (z=200).",
            )
            .ok();
            id
        }
        Err(_) => 0,
    };

    let mut term = TerminalApp::new();

    // Render & present terminal UI
    if surface > 0 {
        repaint(surface, &term, Dirty::All);
        let _ = std::surface_flip();
        std::log("[TERMINAL APP] Terminal console presented to scanout buffer.",
        )
        .ok();
    }

    // 8. Terminal Asynchronous Event Loop
    loop {
        let interest = std::EVENT_TOUCH_INPUT | std::EVENT_TIMER_TICK | std::EVENT_IPC_MSG;
        if let Ok(triggered) = std::event_poll(interest, 100) {
            if (triggered & std::EVENT_TOUCH_INPUT) != 0 {
                let mut input = std::InputEvent {
                    kind: 0,
                    x: 0,
                    y: 0,
                    dx: 0,
                    dy: 0,
                    scale_milli: 1000,
                };
                let mut dirty = Dirty::None;
                while std::input_event_poll(&mut input).is_ok() {
                    // Keyboard first: the normalized pipeline multiplexes
                    // keys and touches through the same poll, so a key must be
                    // recognised before the touch branch below sees kind != 1.
                    if let Some(key) = input.key_event() {
                        if term.handle_key(key) {
                            dirty = dirty.widen(Dirty::Console);
                        }
                        continue;
                    }
                    if input.kind == 4 || input.kind == 6 || input.kind == 7 || input.kind == 8 || input.kind == 10 {
                        std::log("[TERMINAL APP] Navigation gesture recognized. Exiting to launcher.",
                        )
                        .ok();
                        if surface > 0 {
                            let _ = std::surface_destroy(surface);
                        }
                        std::exit_with_code(0);
                    }
                    if input.kind == 1 {
                        // Check if Back button or Home gesture bar was tapped: exit cleanly to launcher
                        if std::design::back_button::contains(input.x as usize, input.y as usize)
                            || (input.x >= 24 && input.x < 100 && input.y >= 28 && input.y < 68)
                            || (input.x >= 240 && input.x <= 480 && input.y >= 1400)
                        {
                            std::log("[TERMINAL APP] Back button or home gesture pressed. Exiting to launcher.").ok();
                            if surface > 0 {
                                let _ = std::surface_destroy(surface);
                            }
                            std::exit_with_code(0);
                        }

                        // Check quick command pills
                        let qcmd = if (1222..1274).contains(&input.y) {
                            if (24..176).contains(&input.x) {
                                Some("help")
                            } else if (194..346).contains(&input.x) {
                                Some("status")
                            } else if (364..516).contains(&input.x) {
                                Some("ps")
                            } else if (534..696).contains(&input.x) {
                                Some("mem")
                            } else {
                                None
                            }
                        } else if (1290..1342).contains(&input.y) {
                            if (24..176).contains(&input.x) {
                                Some("caps")
                            } else if (194..346).contains(&input.x) {
                                Some("ping")
                            } else if (364..516).contains(&input.x) {
                                Some("dma")
                            } else if (534..696).contains(&input.x) {
                                Some("share")
                            } else {
                                None
                            }
                        } else {
                            None
                        };

                        if let Some(cmd) = qcmd {
                            term.input_len = 0;
                            term.execute_command(cmd);
                            dirty = dirty.widen(Dirty::Console);
                        }
                        // WHY an unmatched tap stays `Dirty::None`: it hit no
                        // pill and no exit control, so no state moved and the
                        // surface is already correct. The old loop rendered the
                        // whole surface anyway.

                        std::audio_haptic_click().ok();
                        std::log("[TERMINAL APP] Normalized touch event received.").ok();
                    }
                }
                // WHY once per batch: the drain above can carry several touches
                // before the app is scheduled again, so the commands are
                // applied first and the console band is repainted and
                // presented exactly once at the end.
                if dirty != Dirty::None && surface > 0 {
                    repaint(surface, &term, dirty);
                    let _ = std::surface_flip();
                }
            }
        }
        std::yield_now();
    }
}

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    std::log("[TERMINAL APP] Panic detected! Terminating task...").ok();
    std::exit();
}
