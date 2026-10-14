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

mod outline_text;

const W: usize = 720;
const H: usize = 1440;
const CHUNK: usize = 4096;

// -----------------------------------------------------------------------------
// Visual Design Tokens
// -----------------------------------------------------------------------------
const BG_DARK: u32 = 0xFF05080E;
const SURFACE_DARK: u32 = 0xFF0B101D;
const SURFACE_RAISED: u32 = 0xFF141C30;
const BORDER_SUBTLE: u32 = 0xFF24324D;
const SUCCESS_EMERALD: u32 = 0xFF10B981;
const ACCENT_SKY: u32 = 0xFF38BDF8;
const TEXT_PRIMARY: u32 = 0xFFF8FAFC;
const TEXT_MUTED: u32 = 0xFF94A3B8;

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
        return run.pixel(x, y, color, BG_DARK);
    }
    std::design::font::test_text_aa(x, y, x0, y0, scale, text, color, BG_DARK, false)
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
    scale * std::design::font::GLYPH_H
}

/// Header band height: y >= HEADER_H draws the hairline rule, so a header run
/// has to fit inside 0..HEADER_H.
const HEADER_H: usize = 94;
/// Scale of the header title run; it needs run_rows(3) = 72 rows.
const TITLE_SCALE: usize = 3;
/// First console line box top. The tallest console buffer (22 lines) ends at
/// CONSOLE_TOP + 22 * CONSOLE_LINE_ROWS = 1170, clear of the pill dock at 1210.
const CONSOLE_TOP: usize = 114;
/// Console line pitch == the line box height, so full-height runs stack without
/// overlapping.
const CONSOLE_LINE_ROWS: usize = 48;
/// Lines the console buffer keeps.
///
/// WHY a named constant and not the literal 18 sprinkled through the buffer
/// code: the console band below is derived from it, and a band that drifted
/// from the buffer it repaints would either miss a line or repaint rows that
/// hold no text.
///
/// WHY 22 and not 18: a wrapped line occupies one row per piece, so the buffer
/// has to hold more logical lines than an unwrapped console did. 114 + 22*48 =
/// 1170 still clears the pill dock at 1210.
const CONSOLE_LINES: usize = 22;

/// Left edge of a console run. WHY 24 and not 28: it is the same left margin the
/// two pill dock rows use, so the console's text column lines up with them, and
/// every extra pixel here is a pixel the wrapped pieces may use.
const CONSOLE_X0: usize = 24;
/// Right edge of a console run: the surface edge. WHY W and not a named inner
/// edge: a console has no card to sit inside, so the text column runs to the
/// edge of the surface, and a run that reaches it is wrapped, never clipped.
const CONSOLE_X1: usize = W;
/// Console body scale. WHY named: the wrap width and the draw gate below both
/// scale by it, so a change here cannot leave one of them behind.
const CONSOLE_SCALE: usize = 2;

/// System CPL badge box. Grown from the old 176x40 so the scale-2
/// "CPL=3 RING" run (228 x 48) fits inside it with padding instead of being
/// cut at the right and bottom edges. Right edge 700 < 720 and it still clears
/// the title run, which ends at 116 + font::measure(b"Terminal", 3).
const BADGE_X0: usize = 440;
const BADGE_X1: usize = 700;
const BADGE_Y0: usize = 23;
const BADGE_H: usize = 48;
const BADGE_R: usize = 16;
const BADGE_TEXT: &[u8] = b"CPL=3 RING";
const BADGE_SCALE: usize = 2;

/// Quick-command pill geometry. Each pill row is 52 rows tall and its scale-2
/// label is centred in that box, so the label's 48-row box is never clipped.
const PILL_SCALE: usize = 2;
const PILL1_Y0: usize = 1222;
const PILL1_H: usize = 52;
const PILL2_Y0: usize = 1290;
const PILL2_H: usize = 52;

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
        std::log(0, msg).ok();
    }
}

/// Reads the hardware tick counter, probing each capability slot for one that
/// holds `TimerTickRead` (WHY: the slot holding that capability varies per task).
fn probe_ticks() -> Result<u64, i64> {
    let mut last = -1i64;
    for slot in 0..12usize {
        match std::get_ticks(slot) {
            Ok(t) => return Ok(t),
            Err(e) => last = e,
        }
    }
    Err(last)
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

/// Command entry in terminal history.
#[derive(Debug, Clone, Copy)]
pub struct HistoryEntry {
    pub cmd: &'static str,
    pub status: i64,
}

/// Byte capacity of one rendered console line. WHY owned bytes: command output
/// (task counts, uptime ticks, dispatch results) is only known at runtime and
/// this no_std app has no allocator, so every line carries a fixed buffer.
const LINE_CAP: usize = 128;

#[derive(Debug, Clone, Copy)]
pub struct ConsoleLine {
    pub text: [u8; LINE_CAP],
    pub len: usize,
    pub color: u32,
}

impl ConsoleLine {
    /// Builds a line from a compile-time string literal (usable in `const fn`).
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

/// Sovereign Intent Terminal Shell state engine.
pub struct TerminalApp {
    pub history: [Option<HistoryEntry>; 8],
    pub history_count: usize,
    pub total_commands_executed: u32,
    pub lines: [Option<ConsoleLine>; CONSOLE_LINES],
    pub line_count: usize,
}

impl TerminalApp {
    pub const fn new() -> Self {
        let mut lines = [None; CONSOLE_LINES];
        // WHY these are pre-wrapped by hand: `new` is a `const fn`, so it cannot
        // call the wrapping `push_line` (the wrapper takes &mut self). The four
        // banner strings are the only lines the app can open with, so wrapping
        // them here instead keeps the boot screen as wrapped as every later
        // line. "InterOS Sovereign Shell [Ring 3 CPL=3]" is 722px at scale 2 and
        // breaks after "Ring 3"; "Isolated memory, token capabilities enforced."
        // is 850px and breaks after "capabilities"; "Tap quick commands below for
        // diagnostics." is 830px and breaks after "for".
        lines[0] = Some(ConsoleLine::from_static(
            "InterOS Sovereign Shell [Ring 3",
            TEXT_MUTED,
        ));
        lines[1] = Some(ConsoleLine::from_static("CPL=3]", TEXT_MUTED));
        lines[2] = Some(ConsoleLine::from_static(
            "Isolated memory, token capabilities",
            ACCENT_SKY,
        ));
        lines[3] = Some(ConsoleLine::from_static("enforced.", ACCENT_SKY));
        lines[4] = Some(ConsoleLine::from_static(
            "Tap quick commands below for",
            TEXT_MUTED,
        ));
        lines[5] = Some(ConsoleLine::from_static("diagnostics.", TEXT_MUTED));
        lines[6] = Some(ConsoleLine::from_static(
            "interos@sovereign:~$ _",
            SUCCESS_EMERALD,
        ));
        Self {
            history: [None::<HistoryEntry>; 8],
            history_count: 0,
            total_commands_executed: 0,
            lines,
            line_count: 7,
        }
    }

    pub fn push_line(&mut self, text: &str, color: u32) {
        self.push_line_bytes(text.as_bytes(), color);
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
            self.push_console_line(piece[0], color);
            rest = &rest[piece[0].len()..];
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

    /// Records an executed command in the history ledger.
    pub fn record_history(&mut self, cmd: &'static str, status: i64) {
        let idx = self.history_count % self.history.len();
        self.history[idx] = Some(HistoryEntry { cmd, status });
        self.history_count += 1;
        self.total_commands_executed += 1;
    }

    /// Executes a built-in terminal command and renders to console.
    pub fn execute_command(&mut self, cmd_line: &'static str) {
        if self.line_count > 0
            && self.lines[self.line_count - 1].map_or(false, |l| l.as_bytes().starts_with(b"interos@"))
        {
            self.line_count -= 1;
        }

        match cmd_line {
            "help" => {
                self.push_line("interos@sovereign:~$ help", SUCCESS_EMERALD);
                self.push_line("Commands: help, status, ps, mem, caps, share, dma, ping, ls, services, cat", TEXT_PRIMARY);
                std::log(
                    0,
                    "[TERMINAL] Commands: help, status, ps, mem, caps, share, dma, ping, ls, services, cat",
                )
                .ok();
                self.record_history("help", 0);
            }
            "status" => {
                self.push_line("interos@sovereign:~$ status", SUCCESS_EMERALD);
                self.push_line("InterOS Core Mobile Platform | CPL=3 Isolated", ACCENT_SKY);
                // WHY: the uptime comes from the real 1 kHz hardware timer. If no
                // capability slot grants TimerTickRead, the screen says so instead
                // of inventing a metric.
                match probe_ticks() {
                    Ok(ticks) => {
                        self.push_counted(
                            "status: uptime ",
                            ticks,
                            " ticks since boot (1 kHz).",
                            TEXT_PRIMARY,
                        );
                        log_counted(
                            "[TERMINAL] Status: uptime ",
                            ticks,
                            " ticks read from the hardware timer.",
                        );
                        self.record_history("status", 0);
                    }
                    Err(code) => {
                        self.push_line("status: uptime unavailable; get_ticks refused.", TEXT_MUTED);
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
                self.push_line("interos@sovereign:~$ ps", SUCCESS_EMERALD);
                // WHY: the task count is read from the runtime list; a fixed
                // number would claim a measurement that was never taken.
                match count_runtime_tasks() {
                    Ok(count) => {
                        self.push_counted(
                            "ps: ",
                            count as u64,
                            " other tasks (app_runtime_list).",
                            TEXT_PRIMARY,
                        );
                        log_counted(
                            "[TERMINAL] ps: app_runtime_list reported ",
                            count as u64,
                            " task records.",
                        );
                        self.record_history("ps", 0);
                    }
                    Err(code) => {
                        self.push_line("ps: unavailable; app_runtime_list refused.", TEXT_MUTED);
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
                self.push_line("interos@sovereign:~$ mem", SUCCESS_EMERALD);
                // WHY: no heap or paging query exists on this platform, so the
                // command reports the absence instead of a plausible-sounding stat.
                // History records -1: no syscall ran, so there is no kernel code —
                // -1 marks "no data available".
                self.push_line("mem: unavailable; no heap or paging", TEXT_MUTED);
                self.push_line("metrics are exposed to this app.", TEXT_MUTED);
                std::log(0, "[TERMINAL] mem: no heap/paging metrics are exposed; nothing was queried.").ok();
                self.record_history("mem", -1);
            }
            "caps" => {
                self.push_line("interos@sovereign:~$ caps", SUCCESS_EMERALD);
                // WHY: this is a static policy statement, not an inspection: the
                // platform exposes no capability-table query to Ring 3.
                self.push_line("caps: policy: zero ambient authority", TEXT_PRIMARY);
                self.push_line("caps: (statement only; no inspection API).", TEXT_MUTED);
                std::log(0, "[TERMINAL] caps: static policy statement; no runtime capability inspection exists.").ok();
                self.record_history("caps", 0);
            }
            "share" => {
                self.push_line("interos@sovereign:~$ share", SUCCESS_EMERALD);
                // WHY: the dispatch Result decides the message — a refused intent
                // is reported as refused, never as a success.
                match std::share_sheet_dispatch(
                    "text/uri-list",
                    "interos://files/documents",
                    Some("interos.terminal"),
                ) {
                    Ok(token) => {
                        self.push_counted("Share: accepted, token ", token, ".", ACCENT_SKY);
                        log_counted("[TERMINAL APP] Share: dispatch accepted, token ", token, ".");
                        self.record_history("share", 0);
                    }
                    Err(code) => {
                        self.push_counted(
                            "Share: dispatch refused, code -",
                            code.unsigned_abs(),
                            ".",
                            TEXT_MUTED,
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
                self.push_line("interos@sovereign:~$ dma", SUCCESS_EMERALD);
                let payload = b"DMA::TERMINAL::STREAM_PACKET";
                match std::shm_pipe_transfer(3, payload, 0x01) {
                    Ok(_) => {
                        self.push_line("DMA: test payload transfer acknowledged by Pipe 3.", SUCCESS_EMERALD);
                        std::log(0, "[TERMINAL APP] DMA test payload transfer acknowledged by Pipe 3.").ok();
                        self.record_history("dma", 0);
                    }
                    Err(code) => {
                        self.push_line("DMA: Pipe 3 transfer unavailable or refused.", TEXT_MUTED);
                        std::log(0, "[TERMINAL APP] DMA test payload transfer failed or was refused.").ok();
                        self.record_history("dma", code);
                    }
                }
            }
            "ping" => {
                self.push_line("interos@sovereign:~$ ping", SUCCESS_EMERALD);
                self.push_line("ping: unavailable; no ICMP/network probe is implemented.", TEXT_MUTED);
                std::log(0, "[TERMINAL APP] ping unavailable: no network probe was executed.").ok();
                self.record_history("ping", 0);
            }
            "services" => {
                self.push_line("interos@sovereign:~$ services", SUCCESS_EMERALD);
                let mut buf = [0u8; 512];
                match std::service_list(&mut buf) {
                    Ok(n) => {
                        self.push_counted("services: snapshot returned ", n as u64, " bytes.", TEXT_PRIMARY);
                        std::log(0, "[TERMINAL APP] services command executed.").ok();
                        self.record_history("services", 0);
                    }
                    Err(code) => {
                        self.push_line("services: query refused or unavailable.", TEXT_MUTED);
                        self.record_history("services", code);
                    }
                }
            }
            "ls" | "vfs" => {
                self.push_line("interos@sovereign:~$ ls", SUCCESS_EMERALD);
                let mut buf = [0u8; 512];
                match std::fs_list(3, "/data", &mut buf) {
                    Ok(n) if n > 0 => {
                        self.push_counted("vfs(/data): ", n as u64, " bytes listed.", SUCCESS_EMERALD);
                        let mut start = 0;
                        while start < n {
                            let end = buf[start..n].iter().position(|&b| b == b'\n').map(|p| start + p).unwrap_or(n);
                            if let Ok(entry_str) = core::str::from_utf8(&buf[start..end]) {
                                if !entry_str.is_empty() {
                                    self.push_line(entry_str, TEXT_PRIMARY);
                                }
                            }
                            start = end.saturating_add(1);
                        }
                        self.record_history("ls", 0);
                    }
                    Ok(_) => {
                        self.push_line("vfs(/data): directory empty.", TEXT_MUTED);
                        self.record_history("ls", 0);
                    }
                    Err(code) => {
                        self.push_line("vfs: fs_list(/data) query refused or unavailable.", TEXT_MUTED);
                        self.record_history("ls", code);
                    }
                }
            }
            "cat" => {
                self.push_line("interos@sovereign:~$ cat", SUCCESS_EMERALD);
                let path = "/data/manifest.json";
                match std::fs_open(3, path, std::FS_READ) {
                    Ok(fd) => {
                        let mut buf = [0u8; 512];
                        match std::fs_read(3, fd, &mut buf) {
                            Ok(n) if n > 0 => {
                                self.push_counted("cat(/data/manifest.json): ", n as u64, " bytes read.", SUCCESS_EMERALD);
                                let mut start = 0;
                                while start < n {
                                    let end = buf[start..n].iter().position(|&b| b == b'\n').map(|p| start + p).unwrap_or(n);
                                    if let Ok(line_str) = core::str::from_utf8(&buf[start..end]) {
                                        if !line_str.is_empty() {
                                            self.push_line(line_str, TEXT_PRIMARY);
                                        }
                                    }
                                    start = end.saturating_add(1);
                                }
                                self.record_history("cat", 0);
                            }
                            Ok(_) => {
                                self.push_line("cat(/data/manifest.json): file empty.", TEXT_MUTED);
                                self.record_history("cat", 0);
                            }
                            Err(code) => {
                                self.push_line("cat: fs_read refused or error.", TEXT_MUTED);
                                self.record_history("cat", code);
                            }
                        }
                        let _ = std::fs_close(fd);
                    }
                    Err(code) => {
                        self.push_line("cat: fs_open(/data/manifest.json) refused or file not found.", TEXT_MUTED);
                        self.record_history("cat", code);
                    }
                }
            }
            _ => {}
        }
        self.push_line("interos@sovereign:~$ _", SUCCESS_EMERALD);
    }
}

#[inline(never)]
fn get_terminal_pixel(x: usize, y: usize, term: &TerminalApp) -> u32 {
    // 1. Status Bar & Header (y: 0..HEADER_H)
    if y < HEADER_H {
        if y >= 94 {
            return BORDER_SUBTLE;
        }

        // Circular back button disc (cx: 62, cy: 48, radius: 22)
        let dx = x as isize - 62;
        let dy = y as isize - 48;
        let dsq = dx * dx + dy * dy;
        if dsq <= 22 * 22 {
            // Anti-aliased disc border rim
            if dsq >= 20 * 20 {
                return BORDER_SUBTLE;
            }
            // Crisp vector chevron: apex at (58, 48), arms pointing up-right and down-right
            if dx <= 2 && dx >= -6 {
                let arm_dy = (dx - (-6)).abs();
                if (dy.abs() - arm_dy).abs() <= 1 {
                    return TEXT_PRIMARY;
                }
            }
            return SURFACE_RAISED;
        }

        // Header Title: "Terminal". The run is scale 3 and needs
        // run_rows(3) = 72 rows, so its line box is centred in the 94-row
        // header band instead of being gated to a 24-row window that kept
        // only a third of the glyphs.
        let title_rows = run_rows(TITLE_SCALE);
        let title_y0 = (HEADER_H - title_rows) / 2;
        if y >= title_y0 && y < title_y0 + title_rows {
            if let Some(c) = test_text(x, y, 116, title_y0, TITLE_SCALE, b"Terminal", TEXT_PRIMARY) {
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
                    return BG_DARK;
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
                SUCCESS_EMERALD,
            ) {
                return c;
            }

            if rx < 2 || ry < 2 {
                return SUCCESS_EMERALD;
            }
            return SURFACE_RAISED;
        }

        return BG_DARK;
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
            return 0xFF030509;
        }
        return BG_DARK;
    }

    // 3. Quick Command Pills Dock (y: 1210..1370)
    if y >= 1210 && y < 1370 {
        if y == 1210 {
            return BORDER_SUBTLE;
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
                    if rx < 12 && ry < 12 {
                        let cdx = (12 - rx) as isize;
                        let cdy = (12 - ry) as isize;
                        if cdx * cdx + cdy * cdy >= 144 {
                            return SURFACE_DARK;
                        }
                    }

                    // Real proportional advance per label instead of a fixed
                    // 8px-per-character cell, centred in this pill's own width.
                    let tx = x0 + std::design::font::center_x(label, PILL_SCALE, x1 - x0);
                    if let Some(c) = test_text(x, y, tx, label_y0, PILL_SCALE, label, ACCENT_SKY) {
                        return c;
                    }

                    if rx < 2 || ry < 2 {
                        return BORDER_SUBTLE;
                    }
                    return SURFACE_RAISED;
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
                    if rx < 12 && ry < 12 {
                        let cdx = (12 - rx) as isize;
                        let cdy = (12 - ry) as isize;
                        if cdx * cdx + cdy * cdy >= 144 {
                            return SURFACE_DARK;
                        }
                    }

                    let tx = x0 + std::design::font::center_x(label, PILL_SCALE, x1 - x0);
                    if let Some(c) = test_text(x, y, tx, label_y0, PILL_SCALE, label, ACCENT_SKY) {
                        return c;
                    }

                    if rx < 2 || ry < 2 {
                        return BORDER_SUBTLE;
                    }
                    return SURFACE_RAISED;
                }
            }
        }

        return SURFACE_DARK;
    }

    // Home Gesture Bar
    if y >= 1418 && y < 1424 && x >= 260 && x < 460 {
        return TEXT_MUTED;
    }

    BG_DARK
}

fn render_terminal_ui(surface: u32, term: &TerminalApp) {
    let mut chunk = [0u32; CHUNK];
    let total = W * H;
    let mut offset = 0;
    while offset < total {
        let count = (total - offset).min(CHUNK);
        let mut x = offset % W;
        let mut y = offset / W;
        for i in 0..count {
            chunk[i] = get_terminal_pixel(x, y, term);
            x += 1;
            if x == W {
                x = 0;
                y += 1;
            }
        }
        let _ = std::surface_write(6, surface, offset, &chunk[..count]);
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
    let mut chunk = [0u32; CHUNK];
    let mut offset = y0 * W;
    let end = y1.min(H) * W;
    while offset < end {
        let count = (end - offset).min(CHUNK);
        let mut x = offset % W;
        let mut y = offset / W;
        for i in 0..count {
            chunk[i] = get_terminal_pixel(x, y, term);
            x += 1;
            if x == W {
                x = 0;
                y += 1;
            }
        }
        let _ = std::surface_write(6, surface, offset, &chunk[..count]);
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
    std::log(
        0,
        "[TERMINAL APP (Ring 3)] InterOS Developer Terminal online.",
    )
    .ok();

    // Slot 6 is FrameBufferDirect
    let surface = match std::surface_create(6, W as u32, H as u32, 200) {
        Ok(id) => {
            std::log(
                0,
                "[TERMINAL APP] DRM/KMS Client Surface allocated (z=200).",
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
        let _ = std::surface_flip(6);
        std::log(
            0,
            "[TERMINAL APP] Terminal console presented to scanout buffer.",
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
                    if input.kind == 4 || input.kind == 6 || input.kind == 7 || input.kind == 8 || input.kind == 10 {
                        std::log(
                            0,
                            "[TERMINAL APP] Navigation gesture recognized. Exiting to launcher.",
                        )
                        .ok();
                        if surface > 0 {
                            let _ = std::surface_destroy(6, surface);
                        }
                        std::exit_with_code(0);
                    }
                    if input.kind == 1 {
                        // Check if Back button or Home gesture bar was tapped: exit cleanly to launcher
                        if (input.x >= 24 && input.x < 100 && input.y >= 28 && input.y < 68)
                            || (input.x >= 240 && input.x <= 480 && input.y >= 1400)
                        {
                            std::log(0, "[TERMINAL APP] Back button or home gesture pressed. Exiting to launcher.").ok();
                            if surface > 0 {
                                let _ = std::surface_destroy(6, surface);
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
                            term.execute_command(cmd);
                            dirty = dirty.widen(Dirty::Console);
                        }
                        // WHY an unmatched tap stays `Dirty::None`: it hit no
                        // pill and no exit control, so no state moved and the
                        // surface is already correct. The old loop rendered the
                        // whole surface anyway.

                        std::audio_haptic_click().ok();
                        std::log(0, "[TERMINAL APP] Normalized touch event received.").ok();
                    }
                }
                // WHY once per batch: the drain above can carry several touches
                // before the app is scheduled again, so the commands are
                // applied first and the console band is repainted and
                // presented exactly once at the end.
                if dirty != Dirty::None && surface > 0 {
                    repaint(surface, &term, dirty);
                    let _ = std::surface_flip(6);
                }
            }
        }
        std::yield_now();
    }
}

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    std::log(0, "[TERMINAL APP] Panic detected! Terminating task...").ok();
    std::exit();
}
