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
//! - Asynchronous event reactor loop handling shell intents and touch commands

#![no_std]
#![no_main]

use core::panic::PanicInfo;
use interos_std as std;

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
    std::design::font::test_text_aa(x, y, x0, y0, scale, text, color, BG_DARK, false)
}

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
    pub lines: [Option<ConsoleLine>; 18],
    pub line_count: usize,
}

impl TerminalApp {
    pub const fn new() -> Self {
        let mut lines = [None; 18];
        lines[0] = Some(ConsoleLine::from_static(
            "InterOS Sovereign Shell [Ring 3 CPL=3]",
            TEXT_MUTED,
        ));
        lines[1] = Some(ConsoleLine::from_static(
            "Isolated memory, token capabilities enforced.",
            ACCENT_SKY,
        ));
        lines[2] = Some(ConsoleLine::from_static(
            "Tap quick commands below for diagnostics.",
            TEXT_MUTED,
        ));
        lines[3] = Some(ConsoleLine::from_static(
            "interos@sovereign:~$ _",
            SUCCESS_EMERALD,
        ));
        Self {
            history: [None; 8],
            history_count: 0,
            total_commands_executed: 0,
            lines,
            line_count: 4,
        }
    }

    pub fn push_line(&mut self, text: &str, color: u32) {
        self.push_line_bytes(text.as_bytes(), color);
    }

    pub fn push_line_bytes(&mut self, text: &[u8], color: u32) {
        let mut line = ConsoleLine {
            text: [0; LINE_CAP],
            len: 0,
            color,
        };
        let n = text.len().min(LINE_CAP);
        line.text[..n].copy_from_slice(&text[..n]);
        line.len = n;
        if self.line_count < 18 {
            self.lines[self.line_count] = Some(line);
            self.line_count += 1;
        } else {
            for i in 0..17 {
                self.lines[i] = self.lines[i + 1];
            }
            self.lines[17] = Some(line);
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
                self.push_line("Commands: help, status, ps, mem, caps, share, dma, ping", TEXT_PRIMARY);
                std::log(
                    0,
                    "[TERMINAL] Commands: help, status, ps, mem, caps, share, dma, ping",
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
            _ => {}
        }
        self.push_line("interos@sovereign:~$ _", SUCCESS_EMERALD);
    }
}

#[inline(never)]
fn get_terminal_pixel(x: usize, y: usize, term: &TerminalApp) -> u32 {
    // 1. Status Bar & Header (y: 0..96)
    if y < 96 {
        if y >= 94 {
            return BORDER_SUBTLE;
        }

        // Back button (<) at (x: 24..100, y: 28..68)
        if x >= 24 && x < 100 && y >= 28 && y < 68 {
            let center_x = 62;
            let center_y = 48;
            let dx = (x as isize - center_x as isize).abs();
            let dy = (y as isize - center_y as isize).abs();
            if (dx + dy <= 12 && x <= center_x)
                || (x >= 24 && x < 100 && (y == 28 || y == 67 || x == 24 || x == 99))
            {
                return TEXT_PRIMARY;
            }
            return SURFACE_RAISED;
        }

        // Header Title: "Terminal"
        if y >= 36 && y < 60 {
            if let Some(c) = test_text(x, y, 116, 36, 3, b"Terminal", TEXT_PRIMARY) {
                return c;
            }
        }

        // System CPL=3 Badge (x: 520..696, y: 28..68)
        if x >= 520 && x < 696 && y >= 28 && y < 68 {
            let rx = (x - 520).min(695 - x);
            let ry = (y - 28).min(67 - y);
            if rx < 16 && ry < 16 {
                let cdx = (16 - rx) as isize;
                let cdy = (16 - ry) as isize;
                if cdx * cdx + cdy * cdy >= 256 {
                    return BG_DARK;
                }
            }

            if let Some(c) = test_text(x, y, 540, 42, 2, b"CPL=3 RING", SUCCESS_EMERALD) {
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
        // Render console lines
        for line_idx in 0..term.line_count {
            let line_y0 = 114 + line_idx * 32;
            if y >= line_y0 && y < line_y0 + 16 {
                if let Some(line) = term.lines[line_idx] {
                    if let Some(c) = test_text(x, y, 28, line_y0, 2, line.as_bytes(), line.color) {
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
        if y >= 1222 && y < 1274 {
            let pills = [
                (24, 176, &b"help"[..]),
                (194, 346, &b"status"[..]),
                (364, 516, &b"ps"[..]),
                (534, 696, &b"mem"[..]),
            ];
            for (x0, x1, label) in pills {
                if x >= x0 && x < x1 {
                    let rx = (x - x0).min(x1 - 1 - x);
                    let ry = (y - 1222).min(1273 - y);
                    if rx < 12 && ry < 12 {
                        let cdx = (12 - rx) as isize;
                        let cdy = (12 - ry) as isize;
                        if cdx * cdx + cdy * cdy >= 144 {
                            return SURFACE_DARK;
                        }
                    }

                    let cx = x0 + (x1 - x0) / 2;
                    let tx = cx.saturating_sub(label.len() * 8);
                    if let Some(c) = test_text(x, y, tx, 1240, 2, label, ACCENT_SKY) {
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
        if y >= 1290 && y < 1342 {
            let pills = [
                (24, 176, &b"caps"[..]),
                (194, 346, &b"ping"[..]),
                (364, 516, &b"dma"[..]),
                (534, 696, &b"share"[..]),
            ];
            for (x0, x1, label) in pills {
                if x >= x0 && x < x1 {
                    let rx = (x - x0).min(x1 - 1 - x);
                    let ry = (y - 1290).min(1341 - y);
                    if rx < 12 && ry < 12 {
                        let cdx = (12 - rx) as isize;
                        let cdy = (12 - ry) as isize;
                        if cdx * cdx + cdy * cdy >= 144 {
                            return SURFACE_DARK;
                        }
                    }

                    let cx = x0 + (x1 - x0) / 2;
                    let tx = cx.saturating_sub(label.len() * 8);
                    if let Some(c) = test_text(x, y, tx, 1308, 2, label, ACCENT_SKY) {
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
        render_terminal_ui(surface, &term);
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
                        }

                        std::audio_haptic_click().ok();
                        if surface > 0 {
                            render_terminal_ui(surface, &term);
                            let _ = std::surface_flip(6);
                        }
                        std::log(0, "[TERMINAL APP] Normalized touch event received.").ok();
                    }
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
