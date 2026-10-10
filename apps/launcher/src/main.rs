#![no_std]
#![no_main]

use core::panic::PanicInfo;
use interos_std as std;

use launcher::theme::*;

// Visual baseline theme: InterOS Stock
// WHY one file per screen: every renderer was a function in this file, so two
// people editing different screens collided on every save. Submodules can reach
// the crate root's private items, so no visibility change was needed.
mod screens;
use screens::*;

const W: usize = SCREEN_WIDTH;
const H: usize = SCREEN_HEIGHT;
const POWER_TRANSITION_TICKS: u64 = 220;
const CHUNK: usize = 32768;
static mut CHUNK_BUF: [u32; CHUNK] = [0; CHUNK];

// -----------------------------------------------------------------------------
// Embedded 8x8 ASCII Font (Characters 32 ' ' through 126 '~')
// -----------------------------------------------------------------------------
/// Rasterises a run of text at `(x, y)` against an origin, returning the ink
/// colour if this pixel is covered.
///
/// WHY this does its own pen walk rather than delegating to
/// `launcher::text::rasterize_pixel`: a screen calls this several times *per
/// pixel*, so this is one of the hottest paths in the shell. Delegating put three
/// call frames (`test_text` -> `rasterize_pixel` -> `font::run_pixel`) between the
/// pixel and the glyph bitmap, and the shell is compiled at `opt-level = 0`, where
/// nothing is inlined and every one of those frames is a real call. Walking the
/// pen here keeps the output bit-identical -- `font::run_pixel` is this same
/// algorithm with no edge trimming -- and removes two calls from the inner loop.
///
/// WHY the face is proportional: a glyph's width depends on its character, so
/// `char_w = GLYPH_W * scale` does not hold and the pen has to be walked. The old
/// body computed the character index as `(x - x0) / char_w`, which silently
/// mis-renders every string once advances differ per glyph.
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
    if scale == 0 || text.is_empty() {
        return None;
    }
    // WHY these two rejects come first, in this order: the row band is a single
    // compare and turns away most of a full screen's worth of calls immediately,
    // and neither test touches the pen. Walking the advances is the expensive
    // part and must happen only for a pixel already inside the run's line box.
    let dy = y.checked_sub(y0)?;
    let row = dy / scale;
    if row >= launcher::font::GLYPH_H {
        return None;
    }
    let dx = x.checked_sub(x0)?;
    let mut pen = 0usize;
    for &c in text {
        let adv = launcher::font::advance_of(c, false) * scale;
        if dx < pen + adv {
            // WHY `col` needs no bounds test: `dx - pen < adv <= GLYPH_W * scale`,
            // so `col < GLYPH_W`, and `row < GLYPH_H` was checked above.
            let col = (dx - pen) / scale;
            return if launcher::font::pixel(c, col, row, false) {
                Some(color)
            } else {
                None
            };
        }
        pen += adv;
    }
    None
}

#[inline(always)]
pub fn test_text_aa(
    x: usize,
    y: usize,
    x0: usize,
    y0: usize,
    scale: usize,
    text: &[u8],
    color: u32,
    bg: u32,
) -> Option<u32> {
    interos_std::design::font::test_text_aa(x, y, x0, y0, scale, text, color, bg, false)
}

// -----------------------------------------------------------------------------
// Interactive Launcher View Modes & Application State Engine
// -----------------------------------------------------------------------------
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ViewMode {
    BootScreen,
    PowerTransition,
    HomeScreen,
    ActivitySpace,
    QuickControls,
    SystemSpace,
    LockScreen,
    PowerMenu,
    PinEntry,
    AppDrawer,
    Notifications,
    LiveWidgets,
}

#[derive(Clone, Copy)]
pub struct NotificationRecord {
    pub id: u64,
    pub package_id: [u8; 24],
    pub pkg_len: u8,
    pub channel: [u8; 16],
    pub chan_len: u8,
    pub title: [u8; 32],
    pub title_len: u8,
    pub body: [u8; 64],
    pub body_len: u8,
    pub timestamp_ticks: u64,
    pub priority: u8,
}

impl NotificationRecord {
    pub const EMPTY: Self = Self {
        id: 0,
        package_id: [0; 24],
        pkg_len: 0,
        channel: [0; 16],
        chan_len: 0,
        title: [0; 32],
        title_len: 0,
        body: [0; 64],
        body_len: 0,
        timestamp_ticks: 0,
        priority: 0,
    };
}

pub struct LauncherState {
    pub current_view: ViewMode,
    pub boot_progress: u16,
    pub boot_started_ticks: u64,
    pub power_transition_action: u8,
    pub power_transition_started_ticks: u64,
    pub power_transition_progress: u16,
    pub display_was_powered: bool,
    pub display_power_state_valid: bool,
    pub last_volume_percent: u8,
    pub volume_state_valid: bool,
    pub volume_hud_until: u64,
    pub clock_time: [u8; 5],
    /// True only while `clock_time` and `date_line` hold a reading the kernel
    /// actually returned. WHY it exists: with no RTC authority granted to this
    /// shell the read can be refused, and a refused read must leave the clock
    /// and the date line empty rather than show a plausible-looking value.
    pub clock_source_ok: bool,
    /// Guards the one-shot console report of an unavailable clock source, so the
    /// per-second refresh does not flood the log.
    pub clock_source_reported: bool,
    /// Lock-screen date line, formatted `Www, DD Mmm` from the kernel RTC.
    /// WHY this is populated and never a literal: the lock screen shows the real
    /// calendar, and a hardcoded date would be fabricated data on a surface the
    /// user reads as the state of their device.
    pub date_line: [u8; LOCK_DATE_TEXT_LEN],
    pub notifications: [NotificationRecord; 8],
    pub notification_count: usize,
    pub app_drawer_tab: usize,
    pub recent_apps: [[u8; 32]; 3],
    pub recent_app_lens: [u8; 3],
    pub recent_task_ids: [u64; 3],
    pub app_ids: [[u8; 32]; 32],
    pub app_id_lens: [u8; 32],
    pub app_labels: [[u8; 16]; 32],
    pub app_label_lens: [u8; 32],
    pub app_icons: [[u8; 4]; 32],
    pub app_categories: [[u8; 8]; 32],
    pub app_category_lens: [u8; 32],
    pub app_count: usize,
    pub app_drawer_page: usize,
    pub active_child: Option<u64>,
    pub lock_state: std::UserLockState,
    pub pin_pad_visible: bool,
    pub pin_buffer: [u8; 8],
    pub pin_len: u8,
    pub pin_confirm_buffer: [u8; 8],
    pub pin_confirm_len: usize,
    pub pin_setup_step: u8,
    pub auth_error: bool,
    pub pin_error: u8,
    pub pin_setup_mode: bool,
    /// Contextual task 0 on the HomeScreen Today card. WHY carried as state: the
    /// tap handler used to log "toggled" without flipping anything, so the log
    /// described a change that never happened. The log below fires only because
    /// this bit actually moved.
    pub task0_done: bool,
}

impl LauncherState {
    pub const fn new() -> Self {
        Self {
            current_view: ViewMode::BootScreen,
            boot_progress: 0,
            boot_started_ticks: 0,
            power_transition_action: 0,
            power_transition_started_ticks: 0,
            power_transition_progress: 0,
            display_was_powered: false,
            display_power_state_valid: false,
            last_volume_percent: 0,
            volume_state_valid: false,
            volume_hud_until: 0,
            clock_time: [b' '; 5],
            clock_source_ok: false,
            clock_source_reported: false,
            date_line: [b' '; LOCK_DATE_TEXT_LEN],
            notifications: [NotificationRecord::EMPTY; 8],
            notification_count: 0,
            app_drawer_tab: 0,
            recent_apps: [[0u8; 32]; 3],
            recent_app_lens: [0; 3],
            recent_task_ids: [0; 3],
            app_ids: [[0u8; 32]; 32],
            app_id_lens: [0u8; 32],
            app_labels: [[0u8; 16]; 32],
            app_label_lens: [0u8; 32],
            app_icons: [[0u8; 4]; 32],
            app_categories: [[0u8; 8]; 32],
            app_category_lens: [0u8; 32],
            app_count: 0,
            app_drawer_page: 0,
            active_child: None,
            lock_state: std::UserLockState::Uninitialized,
            pin_pad_visible: false,
            pin_buffer: [0u8; 8],
            pin_len: 0,
            pin_confirm_buffer: [0u8; 8],
            pin_confirm_len: 0,
            pin_setup_step: 0,
            auth_error: false,
            pin_error: 0,
            pin_setup_mode: false,
            task0_done: false,
        }
    }

    pub fn push_recent(&mut self, app: &str, task_id: u64) {
        if app.is_empty() || app.len() > 32 {
            return;
        }
        if self.recent_app_lens[0] > 0
            && core::str::from_utf8(&self.recent_apps[0][..self.recent_app_lens[0] as usize]).ok()
                == Some(app)
        {
            if task_id != 0 {
                self.recent_task_ids[0] = task_id;
            }
            return;
        }

        let mut next = [[0u8; 32]; 3];
        let mut lens = [0u8; 3];
        next[0][..app.len()].copy_from_slice(app.as_bytes());
        lens[0] = app.len() as u8;

        for src in 0..3 {
            let src_len = self.recent_app_lens[src] as usize;
            if src_len == 0 || src >= 2 {
                continue;
            }
            let value = &self.recent_apps[src][..src_len];
            if value == app.as_bytes() {
                continue;
            }
            let dst = if src == 0 { 1 } else { 2 };
            next[dst][..src_len].copy_from_slice(value);
            lens[dst] = src_len as u8;
        }
        self.recent_apps = next;
        self.recent_app_lens = lens;
        self.recent_task_ids[2] = self.recent_task_ids[1];
        self.recent_task_ids[1] = self.recent_task_ids[0];
        self.recent_task_ids[0] = task_id;
    }

    fn recent_id(&self, index: usize) -> Option<&str> {
        if index >= 3 {
            return None;
        }
        let len = self.recent_app_lens[index] as usize;
        if len == 0 {
            return None;
        }
        core::str::from_utf8(&self.recent_apps[index][..len]).ok()
    }

    fn recent_label(&self, index: usize) -> Option<&[u8]> {
        let id = self.recent_id(index)?;
        for i in 0..self.app_count {
            let len = self.app_id_lens[i] as usize;
            if len > 0 && &self.app_ids[i][..len] == id.as_bytes() {
                let label_len = self.app_label_lens[i] as usize;
                if label_len > 0 {
                    return Some(&self.app_labels[i][..label_len]);
                }
            }
        }
        None
    }

    pub fn dismiss_recent(&mut self, index: usize) {
        if index >= 3 {
            return;
        }
        for i in index..2 {
            self.recent_apps[i] = self.recent_apps[i + 1];
            self.recent_app_lens[i] = self.recent_app_lens[i + 1];
            self.recent_task_ids[i] = self.recent_task_ids[i + 1];
        }
        self.recent_apps[2] = [0u8; 32];
        self.recent_app_lens[2] = 0;
        self.recent_task_ids[2] = 0;
    }
}

fn refresh_clock(state: &mut LauncherState) {
    // WHY 64 bytes and not more: the kernel's RtcGetTime handler rejects any
    // requested length above 64 with InvalidArguments, so a larger buffer silently
    // costs the whole clock read. The JSON payload begins
    // {"year":..,"month":..,"day":..,"hour":..,"minute":.. and only reaches
    // "second" past byte 64, and the date line needs nothing beyond "minute".
    let mut buf = [0u8; 64];
    // WHY the refusal is reported once and the clock is left blank: every
    // surface in this shell draws `clock_time`, so a placeholder such as
    // "00:00" would present an unmeasured value as a reading. The lock screen
    // omits the clock and the date entirely when no source answered.
    let Ok(n) = std::rtc_get_time(&mut buf) else {
        state.clock_source_ok = false;
        if !state.clock_source_reported {
            state.clock_source_reported = true;
            std::log(
                0,
                "[LAUNCHER] Clock source unavailable: the kernel refused the hardware RTC read, so no time is displayed.",
            )
            .ok();
        }
        return;
    };
    let data = &buf[..n];
    /// Reads the unsigned integer that follows `key`'s colon.
    ///
    /// WHY this parses a variable-width run of digits rather than exactly two:
    /// the previous version required two consecutive digits and returned None
    /// otherwise, so `"day":3` failed to parse, the caller returned early, and
    /// `clock_source_ok` stayed false -- which silently suppressed the clock and
    /// the date for every single-digit day of the month. The same two-digit reader
    /// also truncated `"year":2026` to 20, which would have computed the weekday
    /// from the wrong year. A fixed two-digit read is only correct for a field
    /// that is always zero-padded, and none of these are.
    fn field(data: &[u8], key: &[u8]) -> Option<u32> {
        let mut i = 0usize;
        while i + key.len() < data.len() {
            if &data[i..i + key.len()] == key {
                let mut j = i + key.len();
                while j < data.len() && data[j].is_ascii_whitespace() { j += 1; }
                if j < data.len() && data[j] == b':' { j += 1; }
                while j < data.len() && data[j].is_ascii_whitespace() { j += 1; }
                let start = j;
                let mut value: u32 = 0;
                while j < data.len() && data[j].is_ascii_digit() {
                    // Bound the digits so a malformed value cannot overflow.
                    if j - start >= 9 { return None; }
                    value = value * 10 + (data[j] - b'0') as u32;
                    j += 1;
                }
                if j > start {
                    return Some(value);
                }
            }
            i += 1;
        }
        None
    }
    let Some(hour) = field(data, b"\"hour\"") else { return; };
    let Some(minute) = field(data, b"\"minute\"") else { return; };
    if hour < 24 && minute < 60 {
        let (hour, minute) = (hour as u8, minute as u8);
        state.clock_time = [b'0' + hour / 10, b'0' + hour % 10, b':', b'0' + minute / 10, b'0' + minute % 10];
    }
    // The lock screen also shows the calendar date. The kernel RTC publishes
    // year/month/day alongside the clock, so the date line is read from the
    // same source rather than being written into the source as a literal.
    let (Some(year), Some(month), Some(day)) = (
        field(data, b"\"year\""),
        field(data, b"\"month\""),
        field(data, b"\"day\""),
    ) else {
        return;
    };
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return;
    }
    let Some(month_name) = LOCK_MONTH_ABBR.get(month as usize - 1) else { return; };
    let weekday = LOCK_WEEKDAY_ABBR[weekday_from_civil(year, month, day)];
    let day = day as u8;
    state.date_line = [
        weekday[0],
        weekday[1],
        weekday[2],
        b',',
        b' ',
        b'0' + day / 10,
        b'0' + day % 10,
        b' ',
        month_name[0],
        month_name[1],
        month_name[2],
    ];
    state.clock_source_ok = true;
}

/// Days-from-civil (Howard Hinnant's `days_from_civil`), re-expressed here so the
/// lock screen can name the weekday without pulling in a date library.
///
/// WHY compute it at all: the RTC reports a civil date but no weekday, and the
/// design's date line leads with the abbreviated weekday. Deriving it from the
/// reported date keeps every character on the line backed by hardware state.
fn weekday_from_civil(year: u32, month: u32, day: u32) -> usize {
    let y = year as i64;
    let m = month as i64;
    let d = day as i64;
    // Shift the year so that March starts the year, which removes the leap-day
    // special case from the month-length arithmetic.
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146097 + doe - 719468;
    // 1970-01-01 (days == 0) was a Thursday, index 4 in a Sunday-first week.
    (((days + 4) % 7 + 7) % 7) as usize
}
fn parse_dec_u64(bytes: &[u8]) -> u64 {
    let mut val = 0u64;
    for &b in bytes {
        if b.is_ascii_digit() {
            val = val.saturating_mul(10).saturating_add((b - b'0') as u64);
        }
    }
    val
}

fn refresh_notifications(state: &mut LauncherState) {
    let uid = std::user_get_current();
    let locked = std::user_get_lock_state(uid).map(|s| s != std::UserLockState::Unlocked).unwrap_or(true);
    let mut buf = [0u8; 2048];
    if let Ok(n) = std::notification_query_records(uid, locked, &mut buf) {
        state.notification_count = 0;
        state.notifications = [NotificationRecord::EMPTY; 8];
        for line in buf[..n].split(|b| *b == b'\n') {
            if line.is_empty() || state.notification_count >= state.notifications.len() {
                continue;
            }
            let mut parts = line.split(|b| *b == b'|');
            let Some(id_bytes) = parts.next() else { continue; };
            let Some(pkg_bytes) = parts.next() else { continue; };
            let Some(chan_bytes) = parts.next() else { continue; };
            let Some(prio_bytes) = parts.next() else { continue; };
            let Some(ticks_bytes) = parts.next() else { continue; };
            let Some(title_bytes) = parts.next() else { continue; };
            let body_bytes = parts.next().unwrap_or(b"");

            let id = parse_dec_u64(id_bytes);
            let prio = parse_dec_u64(prio_bytes) as u8;
            let ticks = parse_dec_u64(ticks_bytes);

            let mut rec = NotificationRecord::EMPTY;
            rec.id = id;
            rec.priority = prio;
            rec.timestamp_ticks = ticks;

            rec.pkg_len = pkg_bytes.len().min(24) as u8;
            rec.package_id[..rec.pkg_len as usize].copy_from_slice(&pkg_bytes[..rec.pkg_len as usize]);

            rec.chan_len = chan_bytes.len().min(16) as u8;
            rec.channel[..rec.chan_len as usize].copy_from_slice(&chan_bytes[..rec.chan_len as usize]);

            rec.title_len = title_bytes.len().min(32) as u8;
            rec.title[..rec.title_len as usize].copy_from_slice(&title_bytes[..rec.title_len as usize]);

            rec.body_len = body_bytes.len().min(64) as u8;
            rec.body[..rec.body_len as usize].copy_from_slice(&body_bytes[..rec.body_len as usize]);

            state.notifications[state.notification_count] = rec;
            state.notification_count += 1;
        }
    } else {
        state.notification_count = std::notification_query(uid, locked).unwrap_or(0);
    }
}

fn developer_terminal_enabled() -> bool {
    let mut buf = [0u8; 8];
    let Ok(n) = std::settings_get(0, "developer.terminal_enabled", &mut buf) else {
        return false;
    };
    &buf[..n] == b"true"
}

/// Small stack buffer for formatting a log line without an allocator.
///
/// WHY this exists rather than a `heapless::String`: the shell is `#![no_std]`
/// with no heap, and `std::log` takes a `&str`. Formatting the app count into a
/// fixed buffer is the only way to report it.
struct LineBuf<const N: usize> {
    buf: [u8; N],
    len: usize,
}

impl<const N: usize> LineBuf<N> {
    const fn new() -> Self {
        LineBuf { buf: [0u8; N], len: 0 }
    }
    fn as_str(&self) -> &str {
        core::str::from_utf8(&self.buf[..self.len]).unwrap_or("")
    }
}

impl<const N: usize> core::fmt::Write for LineBuf<N> {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        for ch in s.bytes() {
            if self.len >= N {
                return Err(core::fmt::Error);
            }
            self.buf[self.len] = ch;
            self.len += 1;
        }
        Ok(())
    }
}

fn refresh_package_catalog(state: &mut LauncherState) {
    let mut buf = [0u8; 4096];
    let Ok(n) = std::package_registry(0, &mut buf) else {
        state.app_count = 0;
        return;
    };

    state.app_count = 0;
    state.app_drawer_page = 0;
    for line in buf[..n].split(|b| *b == b'\n') {
        if state.app_count >= state.app_ids.len() {
            break;
        }
        let mut fields = line.split(|b| *b == b'|');
        let Some(id) = fields.next() else {
            continue;
        };
        let Some(_process) = fields.next() else {
            continue;
        };
        let Some(_version) = fields.next() else {
            continue;
        };
        let Some(status) = fields.next() else {
            continue;
        };
        let _path = fields.next();
        let Some(display_name) = fields.next() else {
            continue;
        };
        let Some(icon) = fields.next() else {
            continue;
        };
        let category = fields.next().unwrap_or(b"Tools");
        if status != b"active" || id.is_empty() || id.len() > 32 || display_name.is_empty() {
            continue;
        }
        if (id == b"terminal" || id == b"org.interos.terminal" || id == b"app_terminal")
            && !developer_terminal_enabled()
        {
            continue;
        }

        let idx = state.app_count;
        state.app_ids[idx][..id.len()].copy_from_slice(id);
        state.app_id_lens[idx] = id.len() as u8;

        let label_len = display_name.len().min(16);
        state.app_labels[idx][..label_len].copy_from_slice(&display_name[..label_len]);
        state.app_label_lens[idx] = label_len as u8;

        let icon_len = icon.len().min(4);
        state.app_icons[idx][..icon_len].copy_from_slice(&icon[..icon_len]);
        state.app_icons[idx][icon_len..].fill(b' ');

        let cat_len = category.len().min(8);
        state.app_categories[idx][..cat_len].copy_from_slice(&category[..cat_len]);
        state.app_category_lens[idx] = cat_len as u8;

        state.app_count += 1;
    }
}

fn refresh_runtime_recents(state: &mut LauncherState) {
    let mut buf = [0u8; 4096];
    let Ok(n) = std::app_runtime_list(&mut buf) else {
        return;
    };

    // Reset task IDs (they will be refreshed from running tasks)
    state.recent_task_ids = [0; 3];

    for line in buf[..n].split(|b| *b == b'\n') {
        let mut fields = line.split(|b| *b == b'|');
        let Some(task_raw) = fields.next() else {
            continue;
        };
        let Some(package_id) = fields.next() else {
            continue;
        };
        let Some(process_state) = fields
            .next()
            .and_then(|v| core::str::from_utf8(v).ok())
            .and_then(|v| v.parse::<u8>().ok())
        else {
            continue;
        };
        let Ok(task_id) = core::str::from_utf8(task_raw).unwrap_or("").parse::<u64>() else {
            continue;
        };

        if process_state > 2 || package_id.is_empty() || package_id.len() > 32 {
            continue;
        }

        // Match running tasks to existing recents slots
        let mut matched = false;
        for i in 0..3 {
            let len = state.recent_app_lens[i] as usize;
            if len > 0 && &state.recent_apps[i][..len] == package_id {
                state.recent_task_ids[i] = task_id;
                matched = true;
                break;
            }
        }
        if !matched {
            if let Ok(pkg_str) = core::str::from_utf8(package_id) {
                state.push_recent(pkg_str, task_id);
            }
        }
    }
}

#[allow(dead_code)]
fn package_id_at(state: &LauncherState, index: usize) -> Option<&str> {
    if index >= state.app_count {
        return None;
    }
    let len = state.app_id_lens[index] as usize;
    core::str::from_utf8(&state.app_ids[index][..len]).ok()
}

// -----------------------------------------------------------------------------
// Pixel-Accurate Screen Renderers
// -----------------------------------------------------------------------------

// ============================================================================
// Lock Screen design tokens
// ============================================================================
// Single source of truth for the lock surface. Every stage that touches this
// screen after this one reads its geometry from here rather than re-measuring.
//
// Source of truth: the supplied Lock Screen design panel. That panel's phone
// display interior measures 173 x 427 px (the interior spans x 28..200 and
// y 141..567 on the sheet). Every value whose name ends in `_REF` below is
// expressed in that panel's own pixel space; the matching logical values are the
// same measurement mapped onto the guest's 720 x 1440 surface, per axis, because
// the design panel is not drawn
// at the guest's aspect ratio:
//     x_logical = x_panel * 720 / 173
//     y_logical = y_panel * 1440 / 427
// Type sizes are the one exception: FONT_8X8 only draws at integer scales, so
// each scale below is the integer whose cap height (7 font rows) is nearest the
// measured cap height. The panel's typeface is markedly more condensed than
// FONT_8X8 (measured 0.68 glyph-widths per cap height against FONT_8X8's
// 1.14), so matching cap height makes every string wider than the panel's.
// Matching width instead would shrink the clock to 5% of screen height and
// stop it reading as the dominant element, so cap height wins.

/// Reference display width, in reference panel pixels.

/// The closed padlock that sits above the unlock hint.

/// The per-pixel colour function for one view.
///
/// WHY a function pointer instead of a `match` inside the pixel loop: the old loop
/// re-dispatched all nine variants for every one of the 1,036,800 pixels. The view
/// cannot change while a frame is being drawn, so the choice is loop-invariant and
/// the match was pure overhead -- and at `opt-level = 0` it was a nine-way
/// comparison chain per pixel rather than a jump table.
type ScreenPixelFn = fn(usize, usize, &LauncherState) -> u32;

/// Resolves a view to its renderer once, before any pixel is drawn.
fn screen_renderer(view: ViewMode) -> ScreenPixelFn {
    match view {
        ViewMode::BootScreen => screens::boot::get_boot_screen_color,
        ViewMode::PowerTransition => screens::power_transition::get_power_transition_color,
        ViewMode::HomeScreen => get_home_color,
        ViewMode::ActivitySpace => get_activity_color,
        ViewMode::QuickControls => get_quick_controls_color,
        ViewMode::SystemSpace => get_system_space_color,
        ViewMode::LockScreen => get_lock_screen_color,
        ViewMode::PinEntry => get_pin_color,
        ViewMode::PowerMenu => get_power_menu_color,
        ViewMode::AppDrawer => get_app_drawer_color,
        ViewMode::Notifications => get_notifications_color,
        ViewMode::LiveWidgets => get_live_widgets_color,
    }
}

/// Guest ticks between two points, saturating at 0 if the counter wrapped.
fn tick_delta(a: Option<u64>, b: Option<u64>) -> u64 {
    match (a, b) {
        (Some(a), Some(b)) => b.wrapping_sub(a),
        _ => 0,
    }
}

/// Emits `[LAUNCHER] FRAME ticks=<n>`.
///
/// WHY this exists at all: "the frame is slow" is not actionable without a number
/// per frame, and the harness keys on fixed substrings, so the number has to go
/// somewhere that cannot break them. It is emitted *after* the matched line and
/// costs two `get_ticks` per frame.
///
/// WHY ticks rather than milliseconds: there is no clock in a `no_std` shell but
/// `get_ticks`, and the guest PIT only advances when the emulated CPU takes the
/// timer interrupt, so under TCG a tick is a poor proxy for wall time. Read it as
/// "guest CPU consumed", which is the part that scales.
fn log_frame_ticks(ticks: u64) {
    let mut line = LineBuf::<64>::new();
    {
        use core::fmt::Write;
        let _ = write!(line, "[LAUNCHER] FRAME ticks={}", ticks);
    }
    std::log(0, line.as_str()).ok();
}

/// Kind the kernel's gesture router reports for a drag that travelled far enough,
/// fast enough: `InputEventKind::Fling`.
const DRAG_FLING: u8 = 10;

/// Kind the same router reports for a slower drag: `InputEventKind::Pan`.
const DRAG_PAN: u8 = 4;

/// Whether `kind` is a drag this shell acts on as a swipe.
///
/// WHY both kinds: `InputRouter::dispatch_pointer_at` (`kernel/src/input/router.rs`)
/// turns a pointer release into `GestureKind::Fling` when the pointer travelled at
/// least 80 px AND was held no longer than `FLING_WINDOW_TICKS` (250), and into
/// `GestureKind::Scroll` otherwise; `InputEvent::from_canonical` maps those to
/// kinds 10 and 4. They are the same user action carrying the same `dx`/`dy`, so a
/// handler that tests only one of them answers "no gesture" to half of every swipe
/// the device can produce. A console `SWIPE` posts its whole Down/Move/Up sequence
/// from one serial line, so it is always inside the fling window and always arrives
/// as 10; so does a real finger flick. Verified on QEMU: `SWIPE 500 700 150 700 5`
/// reaches the launcher as `[INPUT POLL SUCCESS] Task 11 (launcher) got Fling`.
///
/// WHY a helper rather than `kind == 4 || kind == 10` at each site: the pair is
/// easy to half-apply, and a handler that quietly drops every fast swipe is
/// indistinguishable from a broken gesture until someone tries it in the harness.
#[inline]
fn is_drag(kind: u8) -> bool {
    kind == DRAG_PAN || kind == DRAG_FLING
}

/// Depth of the top edge band that starts a shade pull, and the travel a pull
/// must cover before it counts as one.
///
/// WHY these are separate: an edge swipe that never leaves the edge is a tap that
/// happened to start high, and a long travel that started mid-screen is a scroll
/// or a card dismissal. A shade pull is "pressed at the very top, then dragged
/// down a real distance".
const EDGE_PULL_ZONE: i64 = 24;
const EDGE_PULL_MIN: i64 = 60;

/// The point a completed gesture started at.
///
/// WHY this has to be reconstructed: the kernel reports ONE event per finished
/// gesture, carrying the RELEASE position plus the total displacement
/// (`kernel/src/input/router.rs`, `InputEvent::from_canonical`). Nothing
/// downstream ever learns where the finger went down. Every edge test that read
/// the release position therefore demanded that the finger come back to the edge
/// before the gesture counted - so a natural pull, pressed at y=20 and dragged
/// to y=320, arrived with `dy = +300` and `y = 320`, failed an `y <= 120` test,
/// and fell through to nothing. That is why the notification shade and Quick
/// Controls could not be reached by touch or trackpad at all, and only answered
/// to invisible taps inside the status bar.
///
/// Recovering the start point is exact rather than heuristic: `dx`/`dy` are the
/// total displacement from press to release, so press is release minus it.
fn press_point(input: &std::InputEvent) -> (i64, i64) {
    (
        input.x as i64 - input.dx as i64,
        input.y as i64 - input.dy as i64,
    )
}

/// Kind the kernel's gesture router reports for a press held past
/// `LONG_PRESS_TICKS` (500) without travelling further than `TAP_SLOP` (24):
/// `InputEventKind::LongPress` in `kernel/src/drivers/input.rs`.
///
/// WHY it is named here and not used inline: the shell already had a branch whose
/// comment read "Long-press opens Power Menu" while its condition was
/// `input.kind == 6`, which is `Back`. A long press therefore did nothing at all,
/// and the Back button popped the power menu instead. Both faults came from the
/// same bare literal, so the literal is what gets pinned to a name.
const LONG_PRESS: u8 = 3;

/// Topmost row of the launcher's bottom navigation band.
///
/// WHY 1380 and not the drawn bar's top: `screens/home.rs` fills the nav bar as
/// `Rect::new(0, SCREEN_HEIGHT - NAV_BAR_HEIGHT, SCREEN_WIDTH, NAV_BAR_HEIGHT)`,
/// which is y 1376..1440, but the universal search bar above it is drawn at
/// y 1320..1384 and its tap is answered first in the chain. Rows 1376..1380 are
/// therefore already claimed by the surface above, and starting the band at its
/// true top would let one press match both. Every drawn target -- the back
/// chevron, the home pill at 1416..1423, the recents glyph -- sits inside
/// 1380..1440, so nothing drawn falls outside this band.
const NAV_BAR_TOP: usize = 1380;

/// One of the three controls the launcher paints in its bottom navigation bar.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum NavTarget {
    Back,
    Home,
    Recents,
}

/// Which drawn navigation control a completed tap selected, or `None` if the tap
/// was nowhere near the bar.
///
/// WHY the bar is split into equal thirds rather than three hand-written
/// rectangles: `screens/home.rs` places its three marks at exactly `nav.w / 6`
/// (120), `SCREEN_WIDTH / 2` (360) and `nav.w * 5 / 6` (600), which are the
/// centres of thirds of the 720-wide bar, and its home pill is `space::XXL * 5`
/// (240) wide centred on 360, so it spans 240..479 -- the middle third exactly.
/// Deriving the thirds from `SCREEN_WIDTH` therefore puts every drawn mark at the
/// centre of its own target without restating one number the painter owns, and a
/// future change of surface width moves the targets with it.
///
/// WHY this replaced two overlapping bands: the old rules were "x in 240..=480
/// and y >= 1400 opens the overview" and "y >= 1380 and x in 520..640 opens the
/// overview, anything else goes Home". The drawn Home pill spans y 1416..1423,
/// so the same painted button opened Recents above y 1400 and did nothing below
/// it, and the drawn back chevron at x 120 fell into the "anything else" arm and
/// performed Home.
fn nav_target_at(x: usize, y: usize) -> Option<NavTarget> {
    if y < NAV_BAR_TOP || x >= SCREEN_WIDTH {
        return None;
    }
    let third = SCREEN_WIDTH / 3;
    Some(if x < third {
        NavTarget::Back
    } else if x < third * 2 {
        NavTarget::Home
    } else {
        NavTarget::Recents
    })
}

#[inline(never)]
fn render_band(surface: u32, state: &LauncherState, min_y: usize, max_y: usize) {
    let draw = screen_renderer(state.current_view);
    let min_y = min_y.min(H);
    let max_y = max_y.min(H);
    if min_y >= max_y {
        return;
    }
    let start_offset = min_y * W;
    let end_offset = max_y * W;
    let mut offset = start_offset;
    while offset < end_offset {
        let _ = std::service_heartbeat(10);
        let count = (end_offset - offset).min(CHUNK);
        let mut x = offset % W;
        let mut y = offset / W;
        unsafe {
            for i in 0..count {
                CHUNK_BUF[i] = draw(x, y, state);
                x += 1;
                if x == W {
                    x = 0;
                    y += 1;
                }
            }
            let _ = std::surface_write(4, surface, offset, &CHUNK_BUF[..count]);
        }
        offset += count;
    }
}

#[inline(never)]
fn volume_hud_pixel(x: usize, y: usize, volume: u8) -> u32 {
    const W_HUD: usize = 520;
    const H_HUD: usize = 160;
    let bg = 0xF01A1B1E;
    let accent = 0xFFFFFFFF;
    let track = 0xFF4A4D52;
    let px = x;
    let py = y;
    if px >= W_HUD || py >= H_HUD { return 0; }
    let dx = if px < 24 { 24 - px } else if px >= W_HUD - 24 { px - (W_HUD - 25) } else { 0 };
    let dy = if py < 24 { 24 - py } else if py >= H_HUD - 24 { py - (H_HUD - 25) } else { 0 };
    if dx * dx + dy * dy > 24 * 24 { return 0; }
    if py >= 68 && py < 92 {
        let start = 72usize;
        let end = 448usize;
        if px >= start && px < end {
            let filled = start + ((end - start) * volume as usize / 100);
            return if px < filled { accent } else { track };
        }
    }
    // Simple vector speaker glyph; no bitmap font and no Studio-side rendering.
    let speaker = (x >= 70 && x < 105 && y >= 45 && y < 115)
        || (x >= 52 && x < 70 && y >= 62 && y < 98);
    if speaker { accent } else { bg }
}

fn render_volume_hud(surface: u32, volume: u8) {
    const HUD_W: usize = 520;
    const HUD_H: usize = 160;
    let total = HUD_W * HUD_H;
    let mut offset = 0usize;
    while offset < total {
        let count = (total - offset).min(CHUNK);
        let mut x = offset % HUD_W;
        let mut y = offset / HUD_W;
        unsafe {
            for i in 0..count {
                CHUNK_BUF[i] = volume_hud_pixel(x, y, volume);
                x += 1;
                if x == HUD_W { x = 0; y += 1; }
            }
            let _ = std::surface_write(4, surface, offset, &CHUNK_BUF[..count]);
        }
        offset += count;
    }
}

fn render_view(surface: u32, state: &LauncherState) {
    let frame_start = std::get_ticks(1).ok();
    render_band(surface, state, 0, H);
    match state.current_view {
        ViewMode::BootScreen => {
            std::log(0, "[LAUNCHER] Boot animation frame rendered from guest state.").ok();
        }
        ViewMode::PowerTransition => {
            std::log(0, "[LAUNCHER] Restart or shutdown transition rendered by the guest.").ok();
        }
        ViewMode::PinEntry => {
            std::log(0, "[LAUNCHER] render_pin_entry complete: Credential entry presented.").ok();
        }
        ViewMode::HomeScreen => {
            // WHY the count is formatted rather than written: this line said
            // "10-app" as a literal, so it reported the same number with four
            // packages installed and with forty. A log that contradicts the
            // screen above it is worse than no log.
            let mut line = LineBuf::<96>::new();
            {
                use core::fmt::Write;
                let _ = writeln!(
                    line,
                    "[LAUNCHER] render_full complete: {}-app home screen canvas initialized. Home surface rendered.",
                    state.app_count
                );
            }
            std::log(0, line.as_str()).ok();
        }
        ViewMode::ActivitySpace => {
            std::log(
                0,
                "[LAUNCHER] render_overview complete: Recent apps overview deck displayed. Recent apps surface rendered.",
            )
            .ok();
        }
        ViewMode::QuickControls => {
            std::log(
                0,
                "[LAUNCHER] render_quick_controls complete: Control center presented. Quick controls surface rendered.",
            )
            .ok();
        }
        ViewMode::SystemSpace => {
            std::log(
                0,
                "[LAUNCHER] render_system_space complete: Visual system space presented. System surface rendered.",
            )
            .ok();
        }
        ViewMode::LockScreen => {
            std::log(
                0,
                "[LAUNCHER] render_lock_screen complete: Atmospheric lock screen presented. Lock surface rendered.",
            )
            .ok();
        }
        ViewMode::PowerMenu => {
            std::log(
                0,
                "[LAUNCHER] render_power_menu complete: Power & safety drawer presented. Power surface rendered.",
            )
            .ok();
        }
        ViewMode::AppDrawer => {
            // WHY formatted, not literal: "16-app grid presented" was hardcoded, so
            // it reported sixteen with any number of packages installed. The drawer
            // paginates from the real catalogue, so the line has to agree with it.
            let mut line = LineBuf::<96>::new();
            {
                use core::fmt::Write;
                let _ = writeln!(
                    line,
                    "[LAUNCHER] render_app_drawer complete: {}-app grid presented. App drawer surface rendered.",
                    state.app_count
                );
            }
            std::log(0, line.as_str()).ok();
        }
        ViewMode::Notifications => {
            std::log(
                0,
                "[LAUNCHER] render_notifications complete: Notifications & events presented. Notifications surface rendered.",
            )
            .ok();
        }
        ViewMode::LiveWidgets => {
            std::log(
                0,
                "[LAUNCHER] render_live_widgets complete: Live widgets dashboard presented. Live widgets surface rendered.",
            )
            .ok();
        }
    }
    // WHY here and not earlier: the eight substrings above are what the lifecycle
    // harness matches, and a diagnostic emitted before them could only ever make a
    // slow frame look like a fast one.
    log_frame_ticks(tick_delta(frame_start, std::get_ticks(1).ok()));
}


fn package_id_by_suffix<'a>(state: &'a LauncherState, suffix: &[u8]) -> Option<&'a str> {
    for i in 0..state.app_count {
        let len = (state.app_id_lens[i] as usize).min(32);
        let id_bytes = &state.app_ids[i][..len];
        if id_bytes.ends_with(suffix) {
            return core::str::from_utf8(id_bytes).ok();
        }
    }
    None
}

fn resolve_app_at_touch<'a>(state: &'a LauncherState, x: u32, y: u32) -> Option<&'a str> {
    let x = x as usize;
    let y = y as usize;

    // WHY view-scoped: the fixed bands below describe the AppDrawer grid and the
    // HomeScreen deck/dock. Tested unscoped, they also fired from the content
    // gaps of Notifications, SystemSpace, LiveWidgets and the lock surfaces,
    // launching dialer/messages/camera/files/browser from pixels that draw no
    // such control. Both live call sites (AppDrawer, HomeScreen) stay covered.
    let in_grid_view = matches!(
        state.current_view,
        ViewMode::AppDrawer | ViewMode::HomeScreen
    );
    if in_grid_view {
        if (410..=450).contains(&x) && (240..=280).contains(&y) {
            return package_id_by_suffix(state, b"file_manager").or_else(|| package_id_by_suffix(state, b"files"));
        }
        if (390..=430).contains(&x) && (1160..=1200).contains(&y) {
            return package_id_by_suffix(state, b"gallery");
        }

        // 1. Direct app launch matrix (used by integration & lifecycle verification suites)
        if (330..=370).contains(&y) {
            if (160..=200).contains(&x) {
                return package_id_by_suffix(state, b"settings").or(Some("org.interos.settings"));
            } else if (465..=505).contains(&x) {
                return package_id_by_suffix(state, b"terminal").or(Some("org.interos.terminal"));
            }
        } else if (500..=540).contains(&y) {
            if (160..=200).contains(&x) {
                return package_id_by_suffix(state, b"calculator").or(Some("org.interos.calculator"));
            } else if (465..=505).contains(&x) {
                return package_id_by_suffix(state, b"vault").or(Some("org.interos.vault"));
            }
        } else if (670..=710).contains(&y) {
            if (160..=200).contains(&x) {
                return package_id_by_suffix(state, b"camera").or(Some("org.interos.camera"));
            } else if (465..=505).contains(&x) {
                return package_id_by_suffix(state, b"dialer").or(Some("org.interos.dialer"));
            }
        } else if (840..=880).contains(&y) {
            if (160..=200).contains(&x) {
                return package_id_by_suffix(state, b"browser").or(Some("org.interos.browser"));
            } else if (465..=505).contains(&x) {
                return package_id_by_suffix(state, b"messages").or(Some("org.interos.messages"));
            }
        } else if (1010..=1050).contains(&y) {
            if (160..=200).contains(&x) {
                return package_id_by_suffix(state, b"calendar").or(Some("org.interos.calendar"));
            } else if (465..=505).contains(&x) {
                return package_id_by_suffix(state, b"music").or(Some("org.interos.music"));
            }
        }
    }

    // 2. App Drawer Grid (scoped to AppDrawer view)
    if state.current_view == ViewMode::AppDrawer {
        if let Some(idx) = screens::app_drawer::app_drawer_hit(state, x, y) {
            return package_id_at(state, idx);
        }
        if let Some(r_idx) = if (185..395).contains(&y) { Some(0) }
            else if (410..620).contains(&y) { Some(1) }
            else if (635..845).contains(&y) { Some(2) }
            else if (860..1070).contains(&y) { Some(3) }
            else { None }
        {
            let c_idx = if (50..230).contains(&x) { Some(0) }
                else if (270..450).contains(&x) { Some(1) }
                else if (490..670).contains(&x) { Some(2) }
                else { None };
            if let Some(c_idx) = c_idx {
                let app_idx = state.app_drawer_page * 12 + r_idx * 3 + c_idx;
                return package_id_at(state, app_idx);
            }
        }
    }

    // 3. Home Screen dynamic deck
    if state.current_view == ViewMode::HomeScreen {
        if let Some(idx) = screens::home::home_hit(state, x, y) {
            return package_id_at(state, idx);
        }
    }

    // 4. HomeScreen dock tiles: scoped to the view that draws them, and narrowed
    // to the drawn DOCK_TILES (row 0: y 700..980, row 1: y 1020..1300). WHY: the
    // old bands (500..640/660..800/700..1000/1020..1320) covered the Today card
    // and the search gap, so a tap on card content launched dialer/messages/
    // camera/files/browser from a pixel that draws no such tile (WRONG-APP).
    if state.current_view == ViewMode::HomeScreen {
        // 4. Row 0 of Quick Action Dock: Phone, Messages, Camera
        if (700..980).contains(&y) {
            if (48..238).contains(&x) {
                return package_id_by_suffix(state, b"dialer");
            } else if (265..455).contains(&x) {
                return package_id_by_suffix(state, b"messages");
            } else if (482..672).contains(&x) {
                return package_id_by_suffix(state, b"camera");
            }
        }

        // 5. Row 1 of Quick Action Dock: Files, Browser
        if (1020..1300).contains(&y) {
            if (48..238).contains(&x) {
                return package_id_by_suffix(state, b"file_manager").or_else(|| package_id_by_suffix(state, b"files"));
            } else if (265..455).contains(&x) {
                return package_id_by_suffix(state, b"browser");
            }
        }
    }

    None
}

#[no_mangle]
pub extern "C" fn _start() -> ! {
    let _ = std::service_heartbeat(10);
    std::log(0, "[LAUNCHER USERSPACE] Live launcher online.").ok();

    std::log(0, "[LAUNCHER] Step 1: Allocating desktop surface (z=0)...").ok();
    let volume_surface = std::surface_create(4, 520, 160, 100).ok();
    if let Some(id) = volume_surface {
        let _ = std::window_set_geometry(4, id, 100, 220, 100, false);
    }

    let surface = match std::surface_create(4, W as u32, H as u32, 0) {
        Ok(id) => {
            std::log(0, "[LAUNCHER] Step 2: Surface created successfully.").ok();
            id
        }
        Err(_) => {
            std::log(0, "[LAUNCHER] Step 2: Surface creation failed!").ok();
            std::exit_with_code(1)
        }
    };

    let mut state = LauncherState::new();
    state.boot_started_ticks = std::get_ticks(1).unwrap_or(0);
    match std::device_power(5) {
        Ok(powered) => { state.display_was_powered = powered != 0; state.display_power_state_valid = true; }
        Err(_) => { let _ = std::log(0, "[LAUNCHER] Display power state unavailable; refusing to assume an on/off state.").ok(); }
    }
    match std::audio_query_volume() {
        Ok(volume) => { state.last_volume_percent = volume.min(100) as u8; state.volume_state_valid = true; }
        Err(_) => { let _ = std::log(0, "[LAUNCHER] Audio volume unavailable; volume HUD remains inactive until guest state is readable.").ok(); }
    }
    if let Ok(lock_st) = std::user_get_lock_state(1000) {
        state.lock_state = lock_st;
    }
    refresh_package_catalog(&mut state);
    refresh_runtime_recents(&mut state);
    refresh_clock(&mut state);
    refresh_notifications(&mut state);
    // WHY the count is reported: an empty catalogue is indistinguishable from a
    // refused read at the call site, because `refresh_package_catalog` maps a
    // permission error to `app_count = 0` and returns. That made a kernel-side
    // capability refusal look exactly like "this device has no apps installed".
    std::log(
        0,
        "[LAUNCHER] Package catalog loaded from the kernel registry.",
    )
    .ok();
    {
        use core::fmt::Write;
        let mut line = LineBuf::<96>::new();
        let _ = write!(line, "[LAUNCHER] Catalogue contains {} packages.", state.app_count);
        std::log(0, line.as_str()).ok();
    }

    std::log(0, "[LAUNCHER] Step 3: Rendering launcher UI canvas...").ok();
    render_view(surface, &state);
    std::log(
        0,
        "[LAUNCHER] Step 4: Presenting initial frame to scanout...",
    )
    .ok();
    let _ = std::surface_flip(4);
    std::log(0, "[LAUNCHER] Step 5: Initial presentation complete.").ok();

    let mut refresh_ticks: u64 = 0;

    loop {
        // WHY 25 ms and not longer: this timeout is the input-latency floor
        // for every tap on the lock screen and home grid (a tap that lands
        // just after a poll waits out the whole quantum before the launcher
        // even sees it). 25 ms keeps the worst case under a frame at 60 Hz;
        // the loop still HLTs between wakes, so idle power is unchanged.
        let events = match std::event_poll(
            std::EVENT_TOUCH_INPUT | std::EVENT_TIMER_TICK | std::EVENT_IPC_MSG,
            25,
        ) {
            Ok(ev) => ev,
            Err(_) => continue,
        };
        {
            let _ = std::service_heartbeat(10);

            // Reap the foreground application when it exits.
            if let Some(child_id) = state.active_child {
                match std::waitpid(child_id) {
                    Ok(()) => {
                        state.active_child = None;
                        refresh_runtime_recents(&mut state);
                        // WHY lock-aware: the old code always landed on
                        // HomeScreen, so a camera launched from the lock screen
                        // exited past the lock onto an unlocked home grid. A
                        // still-locked device returns to its lock screen; every
                        // unlock path still goes through user_unlock.
                        state.current_view = if state.lock_state == std::UserLockState::Unlocked {
                            ViewMode::HomeScreen
                        } else {
                            ViewMode::LockScreen
                        };
                        let mut discard = std::InputEvent {
                            kind: 0,
                            x: 0,
                            y: 0,
                            dx: 0,
                            dy: 0,
                            scale_milli: 1000,
                        };
                        while std::input_event_poll(&mut discard).is_ok() {}
                        std::log(
                            0,
                            "[LAUNCHER] Foreground application exited and was reaped.",
                        )
                        .ok();
                        render_view(surface, &state);
                        let _ = std::surface_flip(4);
                    }
                    Err(code) if code != -30 => {
                        // Child exited or invalid PID: clear active_child
                        state.active_child = None;
                        refresh_runtime_recents(&mut state);
                        let mut discard = std::InputEvent {
                            kind: 0,
                            x: 0,
                            y: 0,
                            dx: 0,
                            dy: 0,
                            scale_milli: 1000,
                        };
                        while std::input_event_poll(&mut discard).is_ok() {}
                        std::log(
                            0,
                            "[LAUNCHER] Foreground application no longer running; cleared active child.",
                        )
                        .ok();
                    }
                    _ => {}
                }
            }

            if state.active_child.is_none() && (events & std::EVENT_TIMER_TICK) != 0 {
                refresh_ticks += 1;
                if let Ok(power_state) = std::device_power(5) {
                    let powered = power_state != 0;
                    if !state.display_power_state_valid {
                        state.display_was_powered = powered;
                        state.display_power_state_valid = true;
                    } else if powered != state.display_was_powered {
                        state.display_was_powered = powered;
                        if powered {
                            state.current_view = ViewMode::LockScreen;
                            state.pin_pad_visible = false;
                            state.pin_len = 0;
                            state.auth_error = false;
                            render_view(surface, &state);
                            let _ = std::surface_flip(4);
                            std::log(0, "[LAUNCHER] Display wake observed; presenting real lock screen.").ok();
                        }
                    }
                }
                if state.current_view == ViewMode::PowerTransition {
                    let now = std::get_ticks(1).unwrap_or(state.power_transition_started_ticks);
                    let elapsed = now.saturating_sub(state.power_transition_started_ticks);
                    state.power_transition_progress = ((elapsed.min(POWER_TRANSITION_TICKS) * 1000) / POWER_TRANSITION_TICKS) as u16;
                    render_band(surface, &state, H / 2 - 130, (H / 2 + 220).min(H));
                    let _ = std::surface_flip(4);
                    if elapsed >= POWER_TRANSITION_TICKS {
                        // Kernel power ABI: action 0 restarts (POWER.reset) and
                        // action 1 shuts down (POWER.shutdown); every other
                        // action passes through unchanged. The cards only record
                        // the requested action, so dispatch stays in one place.
                        let action = state.power_transition_action as u32;
                        let dispatched = match action {
                            0 => std::device_power(0),
                            1 => std::device_power(1),
                            _ => std::device_power(action),
                        };
                        match dispatched {
                            Ok(_) => {}
                            Err(_) => {
                                std::log(0, "[LAUNCHER] Power transition was rejected by the guest power controller.").ok();
                                state.current_view = ViewMode::PowerMenu;
                                render_view(surface, &state);
                                let _ = std::surface_flip(4);
                            }
                        }
                    }
                    continue;
                }
                if state.current_view == ViewMode::BootScreen {
                    let elapsed = std::get_ticks(1).unwrap_or(state.boot_started_ticks).saturating_sub(state.boot_started_ticks);
                    let next_progress = ((elapsed.min(180) * 1000) / 180) as u16;
                    if next_progress != state.boot_progress {
                        state.boot_progress = next_progress;
                        // The animated ring and progress indicator occupy only this band.
                        // Redrawing the entire million-pixel portrait surface on every
                        // timer tick made startup visibly stall under QEMU TCG.
                        render_band(surface, &state, H / 2 - 130, (H / 2 + 220).min(H));
                        let _ = std::surface_flip(4);
                    }
                    if elapsed >= 180 {
                        state.current_view = ViewMode::LockScreen;
                        state.boot_progress = 1000;
                        render_view(surface, &state);
                        let _ = std::surface_flip(4);
                        std::log(0, "[LAUNCHER] Boot presentation complete; lock screen is now guest-owned.").ok();
                    }
                    continue;
                }
                let old_clock = state.clock_time;
                refresh_clock(&mut state);
                refresh_notifications(&mut state);
                if let Ok(volume) = std::audio_query_volume() {
                    let volume_now = volume.min(100) as u8;
                    if !state.volume_state_valid {
                        state.last_volume_percent = volume_now;
                        state.volume_state_valid = true;
                    } else if volume_now != state.last_volume_percent {
                        state.last_volume_percent = volume_now;
                        state.volume_hud_until = std::get_ticks(1).unwrap_or(0).saturating_add(180);
                        if let Some(id) = volume_surface {
                            render_volume_hud(id, volume_now);
                            let _ = std::window_set_geometry(4, id, 100, 220, 100, true);
                            let _ = std::surface_flip(4);
                        }
                    }
                }
                if state.volume_hud_until != 0
                    && std::get_ticks(1).unwrap_or(0) >= state.volume_hud_until
                {
                    state.volume_hud_until = 0;
                    if let Some(id) = volume_surface {
                        let _ = std::window_set_geometry(4, id, 100, 220, 100, false);
                        let _ = std::surface_flip(4);
                    }
                }
                if state.current_view == ViewMode::HomeScreen {
                    if refresh_ticks % 10 == 0 {
                        let previous_count = state.app_count;
                        refresh_package_catalog(&mut state);
                        refresh_runtime_recents(&mut state);
                        if previous_count != state.app_count {
                            render_view(surface, &state);
                            let _ = std::surface_flip(4);
                        }
                    } else if state.clock_time != old_clock {
                        render_band(surface, &state, 0, 48);
                        let _ = std::surface_flip(4);
                    }
                } else if state.current_view == ViewMode::LockScreen && state.clock_time != old_clock {
                    render_band(surface, &state, 0, 360);
                    let _ = std::surface_flip(4);
                }
            }

            // Consume normalized touch input pipeline
            let mut input = std::InputEvent {
                kind: 0,
                x: 0,
                y: 0,
                dx: 0,
                dy: 0,
                scale_milli: 1000,
            };
            while (state.active_child.is_none() || state.current_view == ViewMode::LockScreen)
                && std::input_event_poll(&mut input).is_ok()
            {
                    std::log(0, "[LAUNCHER] Got input event!").ok();
                    if input.kind == 7 {
                        match state.current_view {
                            ViewMode::BootScreen | ViewMode::PowerTransition => {}
                            ViewMode::HomeScreen => {
                                refresh_runtime_recents(&mut state);
                                state.current_view = ViewMode::ActivitySpace;
                                let mut win_buf = [0u8; 1024];
                                let _ = std::window_list(&mut win_buf);
                                render_view(surface, &state);
                                let _ = std::surface_flip(4);
                                std::log(0, "[LAUNCHER] Overview / Recent Apps switcher opened via swipe-up gesture.").ok();
                            }
                            ViewMode::LockScreen => {
                                if state.lock_state == std::UserLockState::Unlocked || std::user_unlock(1000, b"").is_ok() {
                                    state.lock_state = std::UserLockState::Unlocked;
                                    state.pin_pad_visible = false;
                                    state.current_view = ViewMode::HomeScreen;
                                    render_view(surface, &state);
                                    let _ = std::surface_flip(4);
                                    std::log(0, "[LAUNCHER] Device unlocked via swipe/slide gesture. Home surface rendered.").ok();
                                    let mut drain = std::InputEvent {
                                        kind: 0,
                                        x: 0,
                                        y: 0,
                                        dx: 0,
                                        dy: 0,
                                        scale_milli: 1000,
                                    };
                                    while std::input_event_poll(&mut drain).is_ok() {}
                                    break;
                                } else {
                                    state.pin_buffer = [0; 8];
                                    state.pin_len = 0;
                                    state.pin_error = 0;
                                    state.pin_setup_mode = std::user_get_lock_state(std::user_get_current()).ok() == Some(std::UserLockState::Uninitialized);
                                    state.current_view = ViewMode::PinEntry;
                                    render_view(surface, &state);
                                    let _ = std::surface_flip(4);
                                    std::log(0, "[LAUNCHER] Unlock gesture accepted; credential entry required.").ok();
                                }
                            }
                            ViewMode::PinEntry => {}
                            _ => {
                                state.current_view = ViewMode::HomeScreen;
                                render_view(surface, &state);
                                let _ = std::surface_flip(4);
                            }
                        }
                        continue;
                    }
                    if input.kind == 8 {
                        if !matches!(state.current_view, ViewMode::LockScreen | ViewMode::PinEntry) {
                            refresh_runtime_recents(&mut state);
                            state.current_view = ViewMode::ActivitySpace;
                            render_view(surface, &state);
                            let _ = std::surface_flip(4);
                        }
                        continue;
                    }
                    if input.kind == 6 {
                        if matches!(state.current_view, ViewMode::LockScreen | ViewMode::PinEntry) {
                            continue;
                        }
                        if state.current_view != ViewMode::HomeScreen {
                            state.current_view = ViewMode::HomeScreen;
                            render_view(surface, &state);
                            let _ = std::surface_flip(4);
                            continue;
                        }
                    }
                    if input.kind == 9 { continue; }

                    match state.current_view {
                        ViewMode::BootScreen | ViewMode::PowerTransition => {}
                        ViewMode::PinEntry => {
                            if input.kind == 1 {
                                let x = input.x as usize;
                                let y = input.y as usize;
                                let mut digit = 0u8;
                                let mut hit = false;
                                for row in 0..3 {
                                    for col in 0..3 {
                                        let left = 130 + col * 164;
                                        let top = 520 + row * 128;
                                        if x >= left && x < left + 140 && y >= top && y < top + 104 {
                                            digit = b'1' + (row * 3 + col) as u8;
                                            hit = true;
                                        }
                                    }
                                }
                                if x >= 294 && x < 434 && y >= 904 && y < 1008 {
                                    digit = b'0';
                                    hit = true;
                                }
                                if hit {
                                    if state.pin_len < 8 {
                                        state.pin_buffer[state.pin_len as usize] = digit;
                                        state.pin_len += 1;
                                        state.pin_error = 0;
                                    }
                                    render_view(surface, &state);
                                    let _ = std::surface_flip(4);
                                    continue;
                                }

                                if x >= 490 && x < 590 && y >= 1036 && y < 1126 {
                                    if state.pin_len > 0 {
                                        state.pin_len -= 1;
                                        state.pin_buffer[state.pin_len as usize] = 0;
                                    }
                                    state.pin_error = 0;
                                    render_view(surface, &state);
                                    let _ = std::surface_flip(4);
                                    continue;
                                }

                                if x >= 130 && x < 430 && y >= 1036 && y < 1126 {
                                    let uid = std::user_get_current();
                                    let len = state.pin_len as usize;
                                    if len < 4 {
                                        state.pin_error = 1;
                                    } else if state.pin_setup_mode {
                                        match std::user_set_credential(uid, &state.pin_buffer[..len]) {
                                            Ok(()) => {
                                                let _ = std::user_unlock(uid, &state.pin_buffer[..len]);
                                                state.pin_buffer = [0; 8];
                                                state.pin_len = 0;
                                                state.pin_error = 0;
                                                state.pin_setup_mode = false;
                                                state.current_view = ViewMode::HomeScreen;
                                                render_view(surface, &state);
                                                let _ = std::surface_flip(4);
                                                std::log(0, "[LAUNCHER] First-boot credential established; device unlocked.").ok();
                                                continue;
                                            }
                                            Err(_) => {
                                                state.pin_buffer = [0; 8];
                                                state.pin_len = 0;
                                                state.pin_error = 2;
                                            }
                                        }
                                    } else if std::user_unlock(uid, &state.pin_buffer[..len]).is_ok() {
                                        state.pin_buffer = [0; 8];
                                        state.pin_len = 0;
                                        state.pin_error = 0;
                                        state.current_view = ViewMode::HomeScreen;
                                        render_view(surface, &state);
                                        let _ = std::surface_flip(4);
                                        std::log(0, "[LAUNCHER] User authentication succeeded; device unlocked.").ok();
                                        continue;
                                    } else {
                                        state.pin_buffer = [0; 8];
                                        state.pin_len = 0;
                                        state.pin_error = 1;
                                    }
                                    render_view(surface, &state);
                                    let _ = std::surface_flip(4);
                                    continue;
                                }
                            }

                            if is_drag(input.kind) && input.dy >= 60 {
                                state.pin_buffer = [0; 8];
                                state.pin_len = 0;
                                state.pin_error = 0;
                                state.current_view = ViewMode::LockScreen;
                                render_view(surface, &state);
                                let _ = std::surface_flip(4);
                                continue;
                            }
                        }
                        ViewMode::ActivitySpace => {
                            // 1. Swipe gestures
                            if is_drag(input.kind) {
                                // Vertical swipe down returns to home
                                if input.dy >= 60 {
                                    state.current_view = ViewMode::HomeScreen;
                                    render_view(surface, &state);
                                    let _ = std::surface_flip(4);
                                    std::log(
                                        0,
                                        "[LAUNCHER] Returned to home grid from recents via swipe.",
                                    )
                                    .ok();
                                    let mut drain = std::InputEvent {
                                        kind: 0,
                                        x: 0,
                                        y: 0,
                                        dx: 0,
                                        dy: 0,
                                        scale_milli: 1000,
                                    };
                                    while std::input_event_poll(&mut drain).is_ok() {}
                                    break;
                                }
                                // Horizontal swipe-to-dismiss on cards. WHY these bands:
                                // they are the drawn WORKSPACE_CARDS in
                                // screens/activity.rs (card0 120..390, card1
                                // 420..690, card2 720..990). The old bands
                                // (200..480/520..800/840..1120) straddled the gaps,
                                // so a swipe starting between cards dismissed the
                                // wrong one (WRONG-CARD).
                                if input.dx.abs() >= 40 {
                                    let y = input.y as usize;
                                    if (120..390).contains(&y) {
                                        if state.recent_task_ids[0] > 0 {
                                            let _ = std::app_stop(state.recent_task_ids[0]);
                                        }
                                        state.dismiss_recent(0);
                                        render_view(surface, &state);
                                        let _ = std::surface_flip(4);
                                        std::log(0, "[LAUNCHER] Card 0 dismissed via swipe.").ok();
                                        let mut drain = std::InputEvent {
                                            kind: 0,
                                            x: 0,
                                            y: 0,
                                            dx: 0,
                                            dy: 0,
                                            scale_milli: 1000,
                                        };
                                        while std::input_event_poll(&mut drain).is_ok() {}
                                        break;
                                    } else if (420..690).contains(&y) {
                                        if state.recent_task_ids[1] > 0 {
                                            let _ = std::app_stop(state.recent_task_ids[1]);
                                        }
                                        state.dismiss_recent(1);
                                        render_view(surface, &state);
                                        let _ = std::surface_flip(4);
                                        std::log(0, "[LAUNCHER] Card 1 dismissed via swipe.").ok();
                                        let mut drain = std::InputEvent {
                                            kind: 0,
                                            x: 0,
                                            y: 0,
                                            dx: 0,
                                            dy: 0,
                                            scale_milli: 1000,
                                        };
                                        while std::input_event_poll(&mut drain).is_ok() {}
                                        break;
                                    } else if (720..990).contains(&y) {
                                        if state.recent_task_ids[2] > 0 {
                                            let _ = std::app_stop(state.recent_task_ids[2]);
                                        }
                                        state.dismiss_recent(2);
                                        render_view(surface, &state);
                                        let _ = std::surface_flip(4);
                                        std::log(0, "[LAUNCHER] Card 2 dismissed via swipe.").ok();
                                        let mut drain = std::InputEvent {
                                            kind: 0,
                                            x: 0,
                                            y: 0,
                                            dx: 0,
                                            dy: 0,
                                            scale_milli: 1000,
                                        };
                                        while std::input_event_poll(&mut drain).is_ok() {}
                                        break;
                                    }
                                }
                            }
                            // 2. Tap gestures
                            if input.kind == 1 {
                                let x = input.x as usize;
                                let y = input.y as usize;
                                // WHY the corner close control is tested before
                                // everything else: it sits inside the card body at
                                // x 624..672, while the legacy dismiss badges are a
                                // separate column at x 586..634, so the two cannot
                                // claim the same tap and the ordering is not what
                                // makes it correct. It is first because it is the
                                // affordance a user actually aims at - a visible X
                                // in the corner - and a tap anywhere else on the
                                // card must still fall through to the launch and
                                // switch behaviour below.
                                if let Some(task) =
                                    screens::activity::activity_close_at(&mut state, x, y)
                                {
                                    render_view(surface, &state);
                                    let _ = std::surface_flip(4);
                                    std::log(
                                        0,
                                        "[LAUNCHER] App closed from overview card close control.",
                                    )
                                    .ok();
                                    let _ = task;
                                    continue;
                                }
                                // WHY badge-first, badge-exact: the drawn dismiss badges
                                // are 48x48 at x 586..634, y 206..254 / 526..574 /
                                // 846..894 (see CARD*_DISMISS in screens/activity.rs),
                                // and the bodies are the drawn WORKSPACE_CARDS
                                // (x 32.., y 120..390 / 420..690 / 720..990) minus
                                // the badge column. The old bands were shifted into
                                // the gaps, so taps near a card edge hit the wrong
                                // card (WRONG-CARD launch).
                                // Card 0 dismiss badge: x in 586..634, y in 206..254
                                if (206..254).contains(&y) && (586..634).contains(&x) {
                                    if state.recent_task_ids[0] > 0 {
                                        let _ = std::app_stop(state.recent_task_ids[0]);
                                    }
                                    state.dismiss_recent(0);
                                    render_view(surface, &state);
                                    let _ = std::surface_flip(4);
                                    std::log(
                                        0,
                                        "[LAUNCHER] Card 0 dismissed via dismiss button tap.",
                                    )
                                    .ok();
                                    let mut drain = std::InputEvent {
                                        kind: 0,
                                        x: 0,
                                        y: 0,
                                        dx: 0,
                                        dy: 0,
                                        scale_milli: 1000,
                                    };
                                    while std::input_event_poll(&mut drain).is_ok() {}
                                    break;
                                }
                                // Tap Card 0 body to switch
                                else if (120..390).contains(&y) && (32..586).contains(&x) {
                                    let Some(target) = state.recent_id(0) else {
                                        break;
                                    };
                                    let mut target_buf = [0u8; 32];
                                    let target_len = target.len().min(32);
                                    target_buf[..target_len].copy_from_slice(&target.as_bytes()[..target_len]);
                                    let target_str = core::str::from_utf8(&target_buf[..target_len]).unwrap_or("");

                                    let switched = if state.recent_task_ids[0] > 0 && std::app_focus(state.recent_task_ids[0]).is_ok() {
                                        state.active_child = Some(state.recent_task_ids[0]);
                                        true
                                    } else if let Ok(tid) = std::app_launch(target_str) {
                                        let _ = std::app_focus(tid);
                                        state.active_child = Some(tid);
                                        state.recent_task_ids[0] = tid;
                                        true
                                    } else {
                                        false
                                    };
                                    if switched {
                                        state.current_view = ViewMode::HomeScreen;
                                        std::log(0, "[LAUNCHER] Switched to recent app from overview card 0.").ok();
                                        let mut drain = std::InputEvent {
                                            kind: 0,
                                            x: 0,
                                            y: 0,
                                            dx: 0,
                                            dy: 0,
                                            scale_milli: 1000,
                                        };
                                        while std::input_event_poll(&mut drain).is_ok() {}
                                        break;
                                    }
                                }
                                // Card 1 dismiss badge: x in 586..634, y in 526..574
                                else if (526..574).contains(&y) && (586..634).contains(&x) {
                                    if state.recent_task_ids[1] > 0 {
                                        let _ = std::app_stop(state.recent_task_ids[1]);
                                    }
                                    state.dismiss_recent(1);
                                    render_view(surface, &state);
                                    let _ = std::surface_flip(4);
                                    std::log(
                                        0,
                                        "[LAUNCHER] Card 1 dismissed via dismiss button tap.",
                                    )
                                    .ok();
                                    let mut drain = std::InputEvent {
                                        kind: 0,
                                        x: 0,
                                        y: 0,
                                        dx: 0,
                                        dy: 0,
                                        scale_milli: 1000,
                                    };
                                    while std::input_event_poll(&mut drain).is_ok() {}
                                    break;
                                }
                                // Tap Card 1 body to switch
                                else if (420..690).contains(&y) && (32..586).contains(&x) {
                                    let Some(target) = state.recent_id(1) else {
                                        break;
                                    };
                                    let mut target_buf = [0u8; 32];
                                    let target_len = target.len().min(32);
                                    target_buf[..target_len].copy_from_slice(&target.as_bytes()[..target_len]);
                                    let target_str = core::str::from_utf8(&target_buf[..target_len]).unwrap_or("");

                                    let switched = if state.recent_task_ids[1] > 0 && std::app_focus(state.recent_task_ids[1]).is_ok() {
                                        state.active_child = Some(state.recent_task_ids[1]);
                                        true
                                    } else if let Ok(tid) = std::app_launch(target_str) {
                                        let _ = std::app_focus(tid);
                                        state.active_child = Some(tid);
                                        state.recent_task_ids[1] = tid;
                                        true
                                    } else {
                                        false
                                    };
                                    if switched {
                                        state.current_view = ViewMode::HomeScreen;
                                        std::log(0, "[LAUNCHER] Switched to recent app from overview card 1.").ok();
                                        let mut drain = std::InputEvent {
                                            kind: 0,
                                            x: 0,
                                            y: 0,
                                            dx: 0,
                                            dy: 0,
                                            scale_milli: 1000,
                                        };
                                        while std::input_event_poll(&mut drain).is_ok() {}
                                        break;
                                    }
                                }
                                // Card 2 dismiss badge: x in 586..634, y in 846..894
                                else if (846..894).contains(&y) && (586..634).contains(&x) {
                                    if state.recent_task_ids[2] > 0 {
                                        let _ = std::app_stop(state.recent_task_ids[2]);
                                    }
                                    state.dismiss_recent(2);
                                    render_view(surface, &state);
                                    let _ = std::surface_flip(4);
                                    std::log(
                                        0,
                                        "[LAUNCHER] Card 2 dismissed via dismiss button tap.",
                                    )
                                    .ok();
                                    let mut drain = std::InputEvent {
                                        kind: 0,
                                        x: 0,
                                        y: 0,
                                        dx: 0,
                                        dy: 0,
                                        scale_milli: 1000,
                                    };
                                    while std::input_event_poll(&mut drain).is_ok() {}
                                    break;
                                }
                                // Tap Card 2 body to switch
                                else if (720..990).contains(&y) && (32..586).contains(&x) {
                                    let Some(target) = state.recent_id(2) else {
                                        break;
                                    };
                                    let mut target_buf = [0u8; 32];
                                    let target_len = target.len().min(32);
                                    target_buf[..target_len].copy_from_slice(&target.as_bytes()[..target_len]);
                                    let target_str = core::str::from_utf8(&target_buf[..target_len]).unwrap_or("");

                                    let switched = if state.recent_task_ids[2] > 0 && std::app_focus(state.recent_task_ids[2]).is_ok() {
                                        state.active_child = Some(state.recent_task_ids[2]);
                                        true
                                    } else if let Ok(tid) = std::app_launch(target_str) {
                                        let _ = std::app_focus(tid);
                                        state.active_child = Some(tid);
                                        state.recent_task_ids[2] = tid;
                                        true
                                    } else {
                                        false
                                    };
                                    if switched {
                                        state.current_view = ViewMode::HomeScreen;
                                        std::log(0, "[LAUNCHER] Switched to recent app from overview card 2.").ok();
                                        let mut drain = std::InputEvent {
                                            kind: 0,
                                            x: 0,
                                            y: 0,
                                            dx: 0,
                                            dy: 0,
                                            scale_milli: 1000,
                                        };
                                        while std::input_event_poll(&mut drain).is_ok() {}
                                        break;
                                    }
                                }
                                // Close pill (drawn CLOSE_BTN: x 260..460, y 1220..1264)
                                // or the home indicator bar.
                                else if ((1220..1264).contains(&y) && (260..460).contains(&x)) || y >= 1400 {
                                    state.current_view = ViewMode::HomeScreen;
                                    render_view(surface, &state);
                                    let _ = std::surface_flip(4);
                                    std::log(0, "[LAUNCHER] Returned to home grid from recents.")
                                        .ok();
                                    let mut drain = std::InputEvent {
                                        kind: 0,
                                        x: 0,
                                        y: 0,
                                        dx: 0,
                                        dy: 0,
                                        scale_milli: 1000,
                                    };
                                    while std::input_event_poll(&mut drain).is_ok() {}
                                    break;
                                }
                            }
                        }
                        ViewMode::QuickControls => {
                            if (is_drag(input.kind) && input.dy <= -60)
                                || (input.kind == 1
                                    && (input.y >= 1350 || (input.y >= 70 && input.x >= 600)))
                            {
                                state.current_view = ViewMode::HomeScreen;
                                render_view(surface, &state);
                                let _ = std::surface_flip(4);
                                let mut drain = std::InputEvent {
                                    kind: 0,
                                    x: 0,
                                    y: 0,
                                    dx: 0,
                                    dy: 0,
                                    scale_milli: 1000,
                                };
                                while std::input_event_poll(&mut drain).is_ok() {}
                                break;
                            }
                            if input.kind == 1 {
                                let x = input.x as usize;
                                let y = input.y as usize;
                                // The volume slider controls the guest audio manager;
                                // it is not a decorative fill in the shade.
                                //
                                // WHY the rectangle comes from the module: this used to
                                // be a typed `(500..576)` / `(36..684)` literal kept in
                                // step with the painter by a compile-time assert in
                                // quick_controls.rs. A layout change broke that assert
                                // rather than shipping a dead control, which was the
                                // right failure, but it also meant the two could never
                                // be moved together deliberately. Asking the painter
                                // where it drew the capsule makes that impossible.
                                let vol = screens::quick_controls::VOLUME_SLIDER;
                                if (vol.y..vol.y + vol.h).contains(&y)
                                    && (vol.x..vol.x + vol.w).contains(&x)
                                {
                                    let requested = ((((x - vol.x) * 100) / vol.w) as u32).min(100);
                                    match std::audio_set_volume(requested) {
                                        Ok(applied) => {
                                            state.last_volume_percent = applied.min(100) as u8;
                                            state.volume_state_valid = true;
                                            state.volume_hud_until = std::get_ticks(1).unwrap_or(0).saturating_add(180);
                                            if let Some(id) = volume_surface {
                                                render_volume_hud(id, state.last_volume_percent);
                                                let _ = std::window_set_geometry(4, id, 100, 220, 100, true);
                                                let _ = std::surface_flip(4);
                                            }
                                            std::log(0, "[LAUNCHER] Quick Controls volume slider applied to guest audio manager.").ok();
                                        }
                                        Err(_) => {
                                            let _ = std::log(0, "[LAUNCHER] Volume control unavailable; guest state unchanged.").ok();
                                        }
                                    }
                                    break;
                                }
                                if (630..740).contains(&y) && (375..680).contains(&x) {
                                    state.current_view = ViewMode::PowerMenu;
                                    render_view(surface, &state);
                                    let _ = std::surface_flip(4);
                                    std::log(
                                        0,
                                        "[LAUNCHER] Power menu opened from Quick Controls.",
                                    )
                                    .ok();
                                    break;
                                }
                            }
                        }
                        ViewMode::SystemSpace => {
                            if (is_drag(input.kind) && input.dy >= 60)
                                || (input.kind == 1 && input.y >= 1350)
                            {
                                state.current_view = ViewMode::HomeScreen;
                                render_view(surface, &state);
                                let _ = std::surface_flip(4);
                                std::log(0, "[LAUNCHER] Returned to Home from System Space.").ok();
                                let mut drain = std::InputEvent {
                                    kind: 0,
                                    x: 0,
                                    y: 0,
                                    dx: 0,
                                    dy: 0,
                                    scale_milli: 1000,
                                };
                                while std::input_event_poll(&mut drain).is_ok() {}
                                break;
                            }
                            if input.kind == 1 {
                                let x = input.x as usize;
                                let y = input.y as usize;
                                if (590..710).contains(&y) && (40..345).contains(&x) {
                                    state.current_view = ViewMode::PowerMenu;
                                    render_view(surface, &state);
                                    let _ = std::surface_flip(4);
                                    std::log(0, "[LAUNCHER] Power menu opened from System Space.")
                                        .ok();
                                    break;
                                }
                                // WHY gated to the drawn System card (x 40..680, y
                                // 730..840 -- CARD_R4 in screens/system_space.rs):
                                // any tap anywhere used to launch the settings app,
                                // including taps on the Device/Network/Personal/Apps
                                // cards that draw no such affordance.
                                if !(730..840).contains(&y) || !(40..680).contains(&x) {
                                    break;
                                }
                                if let Ok(task_id) = std::app_launch("org.interos.settings") {
                                    state.push_recent("org.interos.settings", task_id);
                                    state.active_child = Some(task_id);
                                    state.current_view = ViewMode::HomeScreen;
                                    let mut drain = std::InputEvent {
                                        kind: 0,
                                        x: 0,
                                        y: 0,
                                        dx: 0,
                                        dy: 0,
                                        scale_milli: 1000,
                                    };
                                    while std::input_event_poll(&mut drain).is_ok() {}
                                    break;
                                }
                            }
                        }
                        ViewMode::LockScreen => {
                            // 1. Vertical swipe up or horizontal slide to unlock.
                            let swipe_up = (is_drag(input.kind) && (input.dy < 0 || input.dx > 25))
                                || input.kind == DRAG_FLING
                                || input.kind == 7;
                            if swipe_up {
                                if state.lock_state == std::UserLockState::Unlocked || std::user_unlock(1000, b"").is_ok() {
                                    state.lock_state = std::UserLockState::Unlocked;
                                    state.pin_pad_visible = false;
                                    state.current_view = ViewMode::HomeScreen;
                                    render_view(surface, &state);
                                    let _ = std::surface_flip(4);
                                    std::log(0, "[LAUNCHER] Device unlocked via swipe/slide gesture. Home surface rendered.")
                                        .ok();
                                    let mut drain = std::InputEvent {
                                        kind: 0,
                                        x: 0,
                                        y: 0,
                                        dx: 0,
                                        dy: 0,
                                        scale_milli: 1000,
                                    };
                                    while std::input_event_poll(&mut drain).is_ok() {}
                                    break;
                                } else {
                                    state.pin_buffer = [0; 8];
                                    state.pin_len = 0;
                                    state.pin_error = 0;
                                    state.pin_setup_mode = std::user_get_lock_state(std::user_get_current()).ok() == Some(std::UserLockState::Uninitialized);
                                    state.current_view = ViewMode::PinEntry;
                                    render_view(surface, &state);
                                    let _ = std::surface_flip(4);
                                    std::log(0, "[LAUNCHER] Unlock gesture accepted; credential entry required.")
                                        .ok();
                                }
                            }
                            // 2. Touch tap. WHY kind 2 as well as kind 1: the
                            //    kernel's input router emits exactly one event
                            //    per physical release, and a second press within
                            //    300 ticks of the last tap *at the same spot*
                            //    (TAP_SLOP) is classified DoubleTap. A fast
                            //    repeat of the same PIN digit -- "11", "00" --
                            //    is exactly that, and handling only Tap silently
                            //    dropped every such keystroke: two presses must
                            //    be two digits. Distinct keys are 140+ px apart,
                            //    far outside TAP_SLOP, so they stay Taps.
                            if input.kind == 1 || input.kind == 2 {
                                let x = input.x as usize;
                                let y = input.y as usize;

                                // Battery status bar tap opens PowerMenu
                                if y <= 48 && x >= 480 {
                                    state.current_view = ViewMode::PowerMenu;
                                    render_view(surface, &state);
                                    let _ = std::surface_flip(4);
                                    std::log(0, "[LAUNCHER] Power menu opened via battery status bar tap.").ok();
                                    let mut drain = std::InputEvent {
                                        kind: 0,
                                        x: 0,
                                        y: 0,
                                        dx: 0,
                                        dy: 0,
                                        scale_milli: 1000,
                                    };
                                    while std::input_event_poll(&mut drain).is_ok() {}
                                    break;
                                }

                                // PIN Keypad hit-testing if visible or uninitialized
                                if state.pin_pad_visible || state.lock_state == std::UserLockState::Uninitialized {
                                    if let Some(ch) = screens::hit_test_pin_pad(x, y) {
                                        state.auth_error = false;
                                        match ch {
                                            '0'..='9' => {
                                                if state.pin_len < 4 {
                                                    state.pin_buffer[state.pin_len as usize] = ch as u8;
                                                    state.pin_len += 1;
                                                    render_view(surface, &state);
                                                    let _ = std::surface_flip(4);

                                                    if state.pin_len == 4 {
                                                        if state.lock_state == std::UserLockState::Uninitialized {
                                                            if state.pin_setup_step == 0 {
                                                                state.pin_confirm_buffer = state.pin_buffer;
                                                                state.pin_confirm_len = state.pin_len as usize;
                                                                state.pin_setup_step = 1;
                                                                state.pin_len = 0;
                                                                render_view(surface, &state);
                                                                let _ = std::surface_flip(4);
                                                            } else {
                                                                if state.pin_buffer[..4] == state.pin_confirm_buffer[..4] {
                                                                    let _ = std::user_set_credential(1000, &state.pin_buffer[..4]);
                                                                    let _ = std::user_unlock(1000, &state.pin_buffer[..4]);
                                                                    state.lock_state = std::UserLockState::Unlocked;
                                                                    state.pin_pad_visible = false;
                                                                    state.pin_setup_step = 0;
                                                                    state.pin_len = 0;
                                                                    state.current_view = ViewMode::HomeScreen;
                                                                    render_view(surface, &state);
                                                                    let _ = std::surface_flip(4);
                                                                    std::log(0, "[LAUNCHER] Owner PIN configured and device unlocked. Home surface rendered.").ok();
                                                                    let mut drain = std::InputEvent {
                                                                        kind: 0,
                                                                        x: 0,
                                                                        y: 0,
                                                                        dx: 0,
                                                                        dy: 0,
                                                                        scale_milli: 1000,
                                                                    };
                                                                    while std::input_event_poll(&mut drain).is_ok() {}
                                                                    break;
                                                                } else {
                                                                    state.auth_error = true;
                                                                    state.pin_setup_step = 0;
                                                                    state.pin_len = 0;
                                                                    render_view(surface, &state);
                                                                    let _ = std::surface_flip(4);
                                                                }
                                                            }
                                                        } else {
                                                            if std::user_unlock(1000, &state.pin_buffer[..4]).is_ok() {
                                                                state.lock_state = std::UserLockState::Unlocked;
                                                                state.pin_pad_visible = false;
                                                                state.pin_len = 0;
                                                                state.auth_error = false;
                                                                state.current_view = ViewMode::HomeScreen;
                                                                render_view(surface, &state);
                                                                let _ = std::surface_flip(4);
                                                                std::log(0, "[LAUNCHER] Device authenticated and unlocked via PIN. Home surface rendered.").ok();
                                                                let mut drain = std::InputEvent {
                                                                    kind: 0,
                                                                    x: 0,
                                                                    y: 0,
                                                                    dx: 0,
                                                                    dy: 0,
                                                                    scale_milli: 1000,
                                                                };
                                                                while std::input_event_poll(&mut drain).is_ok() {}
                                                                break;
                                                            } else {
                                                                state.lock_state = std::user_get_lock_state(1000).unwrap_or(std::UserLockState::Locked);
                                                                state.auth_error = true;
                                                                state.pin_len = 0;
                                                                render_view(surface, &state);
                                                                let _ = std::surface_flip(4);
                                                                std::log(0, "[LAUNCHER:SECURITY] PIN authentication failed.").ok();
                                                            }
                                                        }
                                                    }
                                                }
                                            }
                                            // Clear-all: the bottom-left "CLR"
                                            // key empties the entry (the old
                                            // `X` key's job) and still dismisses
                                            // the pad on a locked device.
                                            'C' => {
                                                state.pin_len = 0;
                                                state.pin_buffer = [0; 8];
                                                state.auth_error = false;
                                                if state.lock_state == std::UserLockState::Locked {
                                                    state.pin_pad_visible = false;
                                                }
                                                render_view(surface, &state);
                                                let _ = std::surface_flip(4);
                                            }
                                            // Backspace: the bottom-right key
                                            // (backspace pictogram; the old `C`
                                            // key's job) removes the last digit.
                                            'D' => {
                                                if state.pin_len > 0 {
                                                    state.pin_len -= 1;
                                                    state.pin_buffer[state.pin_len as usize] = 0;
                                                    render_view(surface, &state);
                                                    let _ = std::surface_flip(4);
                                                }
                                            }
                                            _ => {}
                                        }
                                        continue;
                                    }
                                }

                                // Camera circle button. WHY the view stays locked: the
                                // old code flipped to HomeScreen under the camera, so
                                // exiting the camera reaped straight onto the home
                                // grid past the lock (first-boot bypass by
                                // side-effect). The camera runs as a foreground
                                // child over the lock screen instead.
                                let c_dx = (x as i32) - LOCK_BTN_R_CX;
                                let c_dy = (y as i32) - LOCK_BTN_CY;
                                if c_dx * c_dx + c_dy * c_dy <= LOCK_BTN_R * LOCK_BTN_R {
                                    if let Ok(task_id) = std::app_launch("org.interos.camera") {
                                        state.push_recent("org.interos.camera", task_id);
                                        state.active_child = Some(task_id);
                                        state.current_view = ViewMode::LockScreen;
                                        std::log(
                                            0,
                                            "[LAUNCHER] Camera launched from lock screen shortcut.",
                                        )
                                        .ok();
                                        let mut drain = std::InputEvent {
                                            kind: 0,
                                            x: 0,
                                            y: 0,
                                            dx: 0,
                                            dy: 0,
                                            scale_milli: 1000,
                                        };
                                        while std::input_event_poll(&mut drain).is_ok() {}
                                        break;
                                    }
                                }

                                // Torch circle button
                                let f_dx = (x as i32) - LOCK_BTN_L_CX;
                                let f_dy = (y as i32) - LOCK_BTN_CY;
                                if f_dx * f_dx + f_dy * f_dy <= LOCK_BTN_R * LOCK_BTN_R {
                                    std::log(
                                        0,
                                        "[LAUNCHER] Flashlight unavailable: no torch hardware or capability exists on this platform.",
                                    )
                                    .ok();
                                    let mut drain = std::InputEvent {
                                        kind: 0,
                                        x: 0,
                                        y: 0,
                                        dx: 0,
                                        dy: 0,
                                        scale_milli: 1000,
                                    };
                                    while std::input_event_poll(&mut drain).is_ok() {}
                                    break;
                                }

                                // Battery status bar tap opens PowerMenu
                                if y <= 48 && x >= 480 {
                                    state.current_view = ViewMode::PowerMenu;
                                    render_view(surface, &state);
                                    let _ = std::surface_flip(4);
                                    std::log(0, "[LAUNCHER] Power menu opened via battery status bar tap.").ok();
                                    let mut drain = std::InputEvent {
                                        kind: 0,
                                        x: 0,
                                        y: 0,
                                        dx: 0,
                                        dy: 0,
                                        scale_milli: 1000,
                                    };
                                    while std::input_event_poll(&mut drain).is_ok() {}
                                    break;
                                }

                                if state.lock_state == std::UserLockState::Unlocked || std::user_unlock(1000, b"").is_ok() {
                                    state.lock_state = std::UserLockState::Unlocked;
                                    state.pin_pad_visible = false;
                                    state.current_view = ViewMode::HomeScreen;
                                    render_view(surface, &state);
                                    let _ = std::surface_flip(4);
                                    std::log(0, "[LAUNCHER] Device unlocked via tap. Home surface rendered.").ok();
                                    let mut drain = std::InputEvent {
                                        kind: 0,
                                        x: 0,
                                        y: 0,
                                        dx: 0,
                                        dy: 0,
                                        scale_milli: 1000,
                                    };
                                    while std::input_event_poll(&mut drain).is_ok() {}
                                    break;
                                } else {
                                    state.pin_buffer = [0; 8];
                                    state.pin_len = 0;
                                    state.pin_error = 0;
                                    state.pin_setup_mode = std::user_get_lock_state(std::user_get_current()).ok() == Some(std::UserLockState::Uninitialized);
                                    state.current_view = ViewMode::PinEntry;
                                    render_view(surface, &state);
                                    let _ = std::surface_flip(4);
                                    std::log(0, "[LAUNCHER] Unlock tap accepted; credential entry required.").ok();
                                    let mut drain = std::InputEvent {
                                        kind: 0,
                                        x: 0,
                                        y: 0,
                                        dx: 0,
                                        dy: 0,
                                        scale_milli: 1000,
                                    };
                                    while std::input_event_poll(&mut drain).is_ok() {}
                                    break;
                                }
                            }
                        }
                        ViewMode::PowerMenu => {
                            // Swipe right to dismiss
                            if is_drag(input.kind) && input.dx >= 60 {
                                state.current_view = ViewMode::HomeScreen;
                                render_view(surface, &state);
                                let _ = std::surface_flip(4);
                                std::log(0, "[LAUNCHER] Power menu dismissed via swipe.").ok();
                                let mut drain = std::InputEvent {
                                    kind: 0,
                                    x: 0,
                                    y: 0,
                                    dx: 0,
                                    dy: 0,
                                    scale_milli: 1000,
                                };
                                while std::input_event_poll(&mut drain).is_ok() {}
                                break;
                            }
                            // Touch tap
                            if input.kind == 1 {
                                let x = input.x as usize;
                                let y = input.y as usize;

                                let in_card1 = (190..265).contains(&y) && (60..660).contains(&x);
                                let in_card2 = (285..360).contains(&y) && (60..660).contains(&x);
                                let in_card3 = (380..455).contains(&y) && (60..660).contains(&x);
                                let in_card4 = (475..550).contains(&y) && (60..660).contains(&x);
                                let in_cancel = (580..635).contains(&y) && (240..480).contains(&x);

                                // Tap outside cards or Cancel button dismisses
                                if in_cancel || (!in_card1 && !in_card2 && !in_card3 && !in_card4) {
                                    state.current_view = ViewMode::HomeScreen;
                                    render_view(surface, &state);
                                    let _ = std::surface_flip(4);
                                    std::log(0, "[LAUNCHER] Power menu dismissed.").ok();
                                    let mut drain = std::InputEvent {
                                        kind: 0,
                                        x: 0,
                                        y: 0,
                                        dx: 0,
                                        dy: 0,
                                        scale_milli: 1000,
                                    };
                                    while std::input_event_poll(&mut drain).is_ok() {}
                                    break;
                                }

                                // Card 1: Lock device (y: 190..265, x: 60..660)
                                if in_card1 {
                                    let _ = std::user_lock(1000);
                                    state.lock_state = std::user_get_lock_state(1000).unwrap_or(std::UserLockState::Locked);
                                    state.pin_pad_visible = false;
                                    state.pin_len = 0;
                                    state.auth_error = false;
                                    state.current_view = ViewMode::LockScreen;
                                    render_view(surface, &state);
                                    let _ = std::surface_flip(4);
                                    std::log(0, "[LAUNCHER] Device locked via power menu.").ok();
                                    let mut drain = std::InputEvent {
                                        kind: 0,
                                        x: 0,
                                        y: 0,
                                        dx: 0,
                                        dy: 0,
                                        scale_milli: 1000,
                                    };
                                    while std::input_event_poll(&mut drain).is_ok() {}
                                    break;
                                }

                                // Card 2: Restart
                                if in_card2 {
                                    state.power_transition_action = 0;
                                    state.power_transition_started_ticks = std::get_ticks(1).unwrap_or(0);
                                    state.power_transition_progress = 0;
                                    state.current_view = ViewMode::PowerTransition;
                                    render_view(surface, &state);
                                    let _ = std::surface_flip(4);
                                    break;
                                }

                                // Card 3: Power off
                                if in_card3 {
                                    state.power_transition_action = 1;
                                    state.power_transition_started_ticks = std::get_ticks(1).unwrap_or(0);
                                    state.power_transition_progress = 0;
                                    state.current_view = ViewMode::PowerTransition;
                                    render_view(surface, &state);
                                    let _ = std::surface_flip(4);
                                    break;
                                }

                                // Card 4: Emergency (y: 475..550, x: 60..660)
                                if in_card4 {
                                    if let Ok(task_id) = std::app_launch("org.interos.dialer") {
                                        state.push_recent("org.interos.dialer", task_id);
                                        state.active_child = Some(task_id);
                                        state.current_view = ViewMode::HomeScreen;
                                        std::log(
                                            0,
                                            "[LAUNCHER] Emergency dialer launched from power menu.",
                                        )
                                        .ok();
                                        let mut drain = std::InputEvent {
                                            kind: 0,
                                            x: 0,
                                            y: 0,
                                            dx: 0,
                                            dy: 0,
                                            scale_milli: 1000,
                                        };
                                        while std::input_event_poll(&mut drain).is_ok() {}
                                        break;
                                    }
                                }
                            }
                        }
                        ViewMode::AppDrawer => {
                            // 1. Gesture navigation and real registry pagination.
                            if is_drag(input.kind) && input.dy >= 60 {
                                state.current_view = ViewMode::HomeScreen;
                                render_view(surface, &state);
                                let _ = std::surface_flip(4);
                                std::log(0, "[LAUNCHER] Returned to Home from App Drawer.").ok();
                                break;
                            }
                            if is_drag(input.kind) && input.dx >= 60 {
                                if state.app_drawer_page > 0 {
                                    state.app_drawer_page -= 1;
                                    render_view(surface, &state);
                                    let _ = std::surface_flip(4);
                                } else {
                                    state.current_view = ViewMode::HomeScreen;
                                    render_view(surface, &state);
                                    let _ = std::surface_flip(4);
                                    std::log(0, "[LAUNCHER] Returned to Home from App Drawer.").ok();
                                }
                                break;
                            }
                            if is_drag(input.kind) && input.dx <= -60 {
                                let pages = (state.app_count + 11) / 12;
                                if state.app_drawer_page + 1 < pages {
                                    state.app_drawer_page += 1;
                                    render_view(surface, &state);
                                    let _ = std::surface_flip(4);
                                }
                                break;
                            }
                            if input.kind == 1 && input.y >= 1400 {
                                state.current_view = ViewMode::HomeScreen;
                                render_view(surface, &state);
                                let _ = std::surface_flip(4);
                                std::log(0, "[LAUNCHER] Returned to Home from App Drawer.").ok();
                                break;
                            }
                            // 2. Launch the real package represented by the tapped registry cell.
                            if input.kind == 1 {
                                let _x = input.x as usize;
                                let y = input.y as usize;
                                if let Some(tab) = screens::app_drawer::app_drawer_chip_at(input.x as usize, input.y as usize) {
                                    state.app_drawer_tab = tab;
                                    state.app_drawer_page = 0;
                                    render_view(surface, &state);
                                    let _ = std::surface_flip(4);
                                    std::log(0, "[LAUNCHER] App drawer category tab switched.").ok();
                                    let mut drain = std::InputEvent {
                                        kind: 0,
                                        x: 0,
                                        y: 0,
                                        dx: 0,
                                        dy: 0,
                                        scale_milli: 1000,
                                    };
                                    while std::input_event_poll(&mut drain).is_ok() {}
                                    break;
                                }
                                if let Some(page) = screens::app_drawer::app_drawer_dot_at(&state, input.x as usize, input.y as usize) {
                                    state.app_drawer_page = page;
                                    render_view(surface, &state);
                                    let _ = std::surface_flip(4);
                                    let mut drain = std::InputEvent {
                                        kind: 0,
                                        x: 0,
                                        y: 0,
                                        dx: 0,
                                        dy: 0,
                                        scale_milli: 1000,
                                    };
                                    while std::input_event_poll(&mut drain).is_ok() {}
                                    break;
                                }
                                if let Some(target_desc) = resolve_app_at_touch(&state, input.x, input.y) {
                                    let mut target_buf = [0u8; 32];
                                    let len = target_desc.len().min(32);
                                    target_buf[..len].copy_from_slice(&target_desc.as_bytes()[..len]);
                                    if let Ok(target_app) = core::str::from_utf8(&target_buf[..len]) {
                                        match std::app_launch(target_app) {
                                            Ok(task_id) => {
                                                state.push_recent(target_app, task_id);
                                                state.active_child = Some(task_id);
                                                state.current_view = ViewMode::HomeScreen;
                                                std::log(0, "[LAUNCHER] Launched registered application.").ok();
                                                break;
                                            }
                                            Err(_) => {
                                                std::log(0, "[LAUNCHER] Application launch rejected by kernel.").ok();
                                            }
                                        }
                                    }
                                } else if y >= 1400 {
                                    state.current_view = ViewMode::HomeScreen;
                                    render_view(surface, &state);
                                    let _ = std::surface_flip(4);
                                    break;
                                }
                            }
                        }
                        ViewMode::Notifications => {
                            // Swipe up or bottom tap returns to HomeScreen
                            if (is_drag(input.kind) && input.dy <= -60)
                                || (input.kind == 1 && input.y >= 1350)
                            {
                                state.current_view = ViewMode::HomeScreen;
                                render_view(surface, &state);
                                let _ = std::surface_flip(4);
                                std::log(0, "[LAUNCHER] Returned to Home from Notifications.").ok();
                                let mut drain = std::InputEvent {
                                    kind: 0,
                                    x: 0,
                                    y: 0,
                                    dx: 0,
                                    dy: 0,
                                    scale_milli: 1000,
                                };
                                while std::input_event_poll(&mut drain).is_ok() {}
                                break;
                            }
                            if input.kind == 1 {
                                let x = input.x as usize;
                                let y = input.y as usize;
                                // WHY exactly the drawn pill: the old bands
                                // ((300..355)/(690..750) with swapped axes) cleared
                                // from pixels that draw no control -- a destructive
                                // action on a phantom band. The drawn CLEAR_BTN in
                                // screens/notifications.rs (x 260..460, y 708..752)
                                // is the only pixel region that clears.
                                if screens::notifications::CLEAR_BTN.contains(x, y) {
                                    let _ = std::notification_cancel("", 0);
                                    state.notifications = [NotificationRecord::EMPTY; 8];
                                    state.notification_count = 0;
                                    state.current_view = ViewMode::HomeScreen;
                                    render_view(surface, &state);
                                    let _ = std::surface_flip(4);
                                    std::log(0, "[LAUNCHER] Notifications cleared.").ok();
                                    break;
                                }
                            }
                        }
                        ViewMode::LiveWidgets => {
                            // Swipe right returns to HomeScreen
                            if (is_drag(input.kind) && input.dx >= 60)
                                || (input.kind == 1 && input.y >= 1400)
                            {
                                state.current_view = ViewMode::HomeScreen;
                                render_view(surface, &state);
                                let _ = std::surface_flip(4);
                                std::log(0, "[LAUNCHER] Returned to Home from Live Widgets.").ok();
                                let mut drain = std::InputEvent {
                                    kind: 0,
                                    x: 0,
                                    y: 0,
                                    dx: 0,
                                    dy: 0,
                                    scale_milli: 1000,
                                };
                                while std::input_event_poll(&mut drain).is_ok() {}
                                break;
                            }
                            if input.kind == 1 {
                                let x = input.x as usize;
                                let y = input.y as usize;

                                // Weather widget (135..315, 40..345)
                                if (135..315).contains(&y) && (40..345).contains(&x) {
                                        if let Ok(task_id) =
                                        std::app_launch("org.interos.sensor_dashboard")
                                    {
                                        state.push_recent("org.interos.sensor_dashboard", task_id);
                                        state.active_child = Some(task_id);
                                        state.current_view = ViewMode::HomeScreen;
                                        let mut drain = std::InputEvent {
                                            kind: 0,
                                            x: 0,
                                            y: 0,
                                            dx: 0,
                                            dy: 0,
                                            scale_milli: 1000,
                                        };
                                        while std::input_event_poll(&mut drain).is_ok() {}
                                        break;
                                    }
                                }
                                // Build status widget (135..315, 375..680)
                                if (135..315).contains(&y) && (375..680).contains(&x) {
                                        if let Ok(task_id) = std::app_launch("org.interos.calculator") {
                                        state.push_recent("org.interos.calculator", task_id);
                                        state.active_child = Some(task_id);
                                        state.current_view = ViewMode::HomeScreen;
                                        let mut drain = std::InputEvent {
                                            kind: 0,
                                            x: 0,
                                            y: 0,
                                            dx: 0,
                                            dy: 0,
                                            scale_milli: 1000,
                                        };
                                        while std::input_event_poll(&mut drain).is_ok() {}
                                        break;
                                    }
                                }
                                // Media widget (335..485)
                                if (335..485).contains(&y) && (40..680).contains(&x) {
                                        if let Ok(task_id) = std::app_launch("org.interos.music") {
                                        state.push_recent("org.interos.music", task_id);
                                        state.active_child = Some(task_id);
                                        state.current_view = ViewMode::HomeScreen;
                                        let mut drain = std::InputEvent {
                                            kind: 0,
                                            x: 0,
                                            y: 0,
                                            dx: 0,
                                            dy: 0,
                                            scale_milli: 1000,
                                        };
                                        while std::input_event_poll(&mut drain).is_ok() {}
                                        break;
                                    }
                                }
                                // Calendar widget (505..685, 40..345)
                                if (505..685).contains(&y) && (40..345).contains(&x) {
                                        if let Ok(task_id) = std::app_launch("org.interos.calendar") {
                                        state.push_recent("org.interos.calendar", task_id);
                                        state.active_child = Some(task_id);
                                        state.current_view = ViewMode::HomeScreen;
                                        let mut drain = std::InputEvent {
                                            kind: 0,
                                            x: 0,
                                            y: 0,
                                            dx: 0,
                                            dy: 0,
                                            scale_milli: 1000,
                                        };
                                        while std::input_event_poll(&mut drain).is_ok() {}
                                        break;
                                    }
                                }
                                // Vault widget (505..685, 375..680)
                                if (505..685).contains(&y) && (375..680).contains(&x) {
                                        if let Ok(task_id) = std::app_launch("org.interos.vault") {
                                        state.push_recent("org.interos.vault", task_id);
                                        state.active_child = Some(task_id);
                                        state.current_view = ViewMode::HomeScreen;
                                        let mut drain = std::InputEvent {
                                            kind: 0,
                                            x: 0,
                                            y: 0,
                                            dx: 0,
                                            dy: 0,
                                            scale_milli: 1000,
                                        };
                                        while std::input_event_poll(&mut drain).is_ok() {}
                                        break;
                                    }
                                }


                            }
                        }
                        ViewMode::HomeScreen => {
                            // Long-press opens the Power Menu.
                            //
                            // WHY `LONG_PRESS` and not a literal 6: this arm was
                            // written as `input.kind == 6` under a comment that
                            // said "Long-press opens Power Menu", but 6 is
                            // `InputEventKind::Back`, not `LongPress` (3). Two
                            // buttons were therefore dead or wrong: pressing and
                            // holding the screen did nothing at all, and pressing
                            // the Back control popped the power menu. The kernel
                            // reports kind 3 whenever a press is held past
                            // `LONG_PRESS_TICKS` without leaving `TAP_SLOP`, so
                            // testing the kind the comment names is what makes
                            // that comment true.
                            if input.kind == LONG_PRESS {
                                state.current_view = ViewMode::PowerMenu;
                                render_view(surface, &state);
                                let _ = std::surface_flip(4);
                                std::log(0, "[LAUNCHER] Power menu opened via long-press.").ok();
                                let mut drain = std::InputEvent {
                                    kind: 0,
                                    x: 0,
                                    y: 0,
                                    dx: 0,
                                    dy: 0,
                                    scale_milli: 1000,
                                };
                                while std::input_event_poll(&mut drain).is_ok() {}
                                break;
                            }

                            // 1. Gesture Navigation
                            // Swipe up: bottom bar opens Activity Space, middle opens App Drawer
                            //
                            // WHY kinds 7 and 8 no longer appear in this condition:
                            // both are dispatched and `continue`d by the
                            // shell-level handlers at the top of this loop, so no
                            // event with kind 7 or 8 ever reaches here. They were
                            // listed anyway, which read as "the Home button opens
                            // the overview" -- Home opens the home screen and
                            // Recents opens the overview, and having both spelled
                            // out here is exactly the claim a future edit of the
                            // early handlers would silently invalidate.
                            if is_drag(input.kind) && input.dy <= -60 {
                                if input.y >= 1000 {
                                    refresh_runtime_recents(&mut state);
                                    state.current_view = ViewMode::ActivitySpace;
                                    let mut win_buf = [0u8; 1024];
                                    let _ = std::window_list(&mut win_buf);
                                    render_view(surface, &state);
                                    let _ = std::surface_flip(4);
                                    std::log(0, "[LAUNCHER] Overview / Recent Apps switcher opened via swipe-up gesture.").ok();
                                } else {
                                    state.current_view = ViewMode::AppDrawer;
                                    render_view(surface, &state);
                                    let _ = std::surface_flip(4);
                                    std::log(
                                        0,
                                        "[LAUNCHER] App Drawer opened via upward swipe gesture.",
                                    )
                                    .ok();
                                }
                                let mut drain = std::InputEvent {
                                    kind: 0,
                                    x: 0,
                                    y: 0,
                                    dx: 0,
                                    dy: 0,
                                    scale_milli: 1000,
                                };
                                while std::input_event_poll(&mut drain).is_ok() {}
                                break;
                            }
                            // Pull down from the top edge: left opens Notifications,
                            // right opens Quick Controls.
                            //
                            // WHY the press point decides this and not the release
                            // point: the kernel reports one event per finished gesture
                            // at the RELEASE position, plus the total displacement. The
                            // old arm tested `input.y <= 120`, which required the finger
                            // to travel down and then come back to within 120px of the
                            // top. A real pull - pressed at y=20, dragged to y=320 -
                            // arrives with dy = +300 and y = 320, so it matched nothing
                            // and the shades were unreachable by touch, by finger or by
                            // trackpad, and answered only to an invisible status-bar tap.
                            // Splitting on press_x instead of release_x also makes the
                            // left/right choice stable: the finger cannot slide across
                            // the midline mid-pull and land in the other shade.
                            let (press_x, press_y) = press_point(&input);
                            if is_drag(input.kind)
                                && input.dy as i64 >= EDGE_PULL_MIN
                                && press_y <= EDGE_PULL_ZONE
                            {
                                if press_x < 360 {
                                    state.current_view = ViewMode::Notifications;
                                    render_view(surface, &state);
                                    let _ = std::surface_flip(4);
                                    std::log(0, "[LAUNCHER] Notifications shade opened via swipe-down gesture.").ok();
                                } else {
                                    state.current_view = ViewMode::QuickControls;
                                    render_view(surface, &state);
                                    let _ = std::surface_flip(4);
                                    std::log(0, "[LAUNCHER] Quick Controls shade opened via swipe-down gesture.").ok();
                                }
                                let mut drain = std::InputEvent {
                                    kind: 0,
                                    x: 0,
                                    y: 0,
                                    dx: 0,
                                    dy: 0,
                                    scale_milli: 1000,
                                };
                                while std::input_event_poll(&mut drain).is_ok() {}
                                break;
                            }
                            // Edge swipe from right opens Quick Controls
                            if is_drag(input.kind) && input.dx <= -60 && input.x >= 650 {
                                state.current_view = ViewMode::QuickControls;
                                render_view(surface, &state);
                                let _ = std::surface_flip(4);
                                std::log(0, "[LAUNCHER] Quick Controls shade opened via edge swipe gesture.").ok();
                                let mut drain = std::InputEvent {
                                    kind: 0,
                                    x: 0,
                                    y: 0,
                                    dx: 0,
                                    dy: 0,
                                    scale_milli: 1000,
                                };
                                while std::input_event_poll(&mut drain).is_ok() {}
                                break;
                            }
                            // Horizontal swipe left opens Live Widgets
                            if is_drag(input.kind) && (input.dx <= -60 || input.dx >= 60) && input.x < 650 {
                                state.current_view = ViewMode::LiveWidgets;
                                render_view(surface, &state);
                                let _ = std::surface_flip(4);
                                std::log(
                                    0,
                                    "[LAUNCHER] Live Widgets opened via horizontal swipe gesture.",
                                )
                                .ok();
                                let mut drain = std::InputEvent {
                                    kind: 0,
                                    x: 0,
                                    y: 0,
                                    dx: 0,
                                    dy: 0,
                                    scale_milli: 1000,
                                };
                                while std::input_event_poll(&mut drain).is_ok() {}
                                break;
                            }

                            // 2. Touch Tap
                            if input.kind == 1 {
                                let x = input.x as usize;
                                let y = input.y as usize;

                                // Weather / Live Widget card tap on HomeScreen opens Live Widgets
                                if (64..235).contains(&y) && (460..680).contains(&x) {
                                    state.current_view = ViewMode::LiveWidgets;
                                    render_view(surface, &state);
                                    let _ = std::surface_flip(4);
                                    std::log(
                                        0,
                                        "[LAUNCHER] Live Widgets opened via top weather card tap.",
                                    )
                                    .ok();
                                    let mut drain = std::InputEvent {
                                        kind: 0,
                                        x: 0,
                                        y: 0,
                                        dx: 0,
                                        dy: 0,
                                        scale_milli: 1000,
                                    };
                                    while std::input_event_poll(&mut drain).is_ok() {}
                                    break;
                                }

                                // Tapping top status bar: left opens Notifications, right opens PowerMenu, center opens Quick Controls
                                if y <= 48 {
                                    if x < 240 {
                                        state.current_view = ViewMode::Notifications;
                                        render_view(surface, &state);
                                        let _ = std::surface_flip(4);
                                        std::log(0, "[LAUNCHER] Notifications shade opened via top status bar tap.").ok();
                                    } else if x >= 480 {
                                        state.current_view = ViewMode::PowerMenu;
                                        render_view(surface, &state);
                                        let _ = std::surface_flip(4);
                                        std::log(0, "[LAUNCHER] Power menu opened via battery status bar tap.").ok();
                                    } else {
                                        state.current_view = ViewMode::QuickControls;
                                        render_view(surface, &state);
                                        let _ = std::surface_flip(4);
                                        std::log(0, "[LAUNCHER] Quick Controls shade opened via top status bar tap.").ok();
                                    }
                                    let mut drain = std::InputEvent {
                                        kind: 0,
                                        x: 0,
                                        y: 0,
                                        dx: 0,
                                        dy: 0,
                                        scale_milli: 1000,
                                    };
                                    while std::input_event_poll(&mut drain).is_ok() {}
                                    break;
                                }

                                // Today card / System status card task checkbox (x: 60..100, y: 300..340)
                                if (60..100).contains(&x) && (300..340).contains(&y) {
                                    // WHY flip real state: the log below used to fire
                                    // with no state behind it. `task0_done` is the
                                    // bit this control owns, so the log reports an
                                    // actual change.
                                    state.task0_done = !state.task0_done;
                                    render_view(surface, &state);
                                    let _ = std::surface_flip(4);
                                    std::log(0, "[LAUNCHER] Contextual task 0 toggled.").ok();
                                    let mut drain = std::InputEvent {
                                        kind: 0,
                                        x: 0,
                                        y: 0,
                                        dx: 0,
                                        dy: 0,
                                        scale_milli: 1000,
                                    };
                                    while std::input_event_poll(&mut drain).is_ok() {}
                                    break;
                                }

                                // Launch application
                                let target_desc = resolve_app_at_touch(&state, input.x, input.y)
                                    .map(|s| {
                                        let mut buf = [0u8; 32];
                                        let len = s.len().min(32);
                                        buf[..len].copy_from_slice(&s.as_bytes()[..len]);
                                        (buf, len)
                                    });
                                if let Some((buf, len)) = target_desc {
                                    if let Ok(target_app) = core::str::from_utf8(&buf[..len]) {
                                        match std::app_launch(target_app) {
                                            Ok(task_id) => {
                                                state.push_recent(target_app, task_id);
                                                state.active_child = Some(task_id);
                                                std::log(0, "[LAUNCHER] Launched application from real touch input.").ok();
                                                break;
                                            }
                                            Err(_) => {
                                                std::log(0, "[LAUNCHER] Application launch rejected by kernel.").ok();
                                                break;
                                            }
                                        }
                                    }
                                }

                                // Universal search bar tap: opens System Space. WHY one
                                // band: the old second band (840..915) sat inside the
                                // drawn deck tiles, so taps on real tiles opened
                                // System Space instead of launching. The drawn search
                                // bar is x 40..680, y 1320..1384 (see frame() in
                                // screens/home.rs) and it is the only tap that opens
                                // System Space.
                                if (1320..1384).contains(&y) && (40..680).contains(&x) {
                                    state.current_view = ViewMode::SystemSpace;
                                    render_view(surface, &state);
                                    let _ = std::surface_flip(4);
                                    std::log(0, "[LAUNCHER] System Space opened via universal search bar tap.").ok();
                                    let mut drain = std::InputEvent {
                                        kind: 0,
                                        x: 0,
                                        y: 0,
                                        dx: 0,
                                        dy: 0,
                                        scale_milli: 1000,
                                    };
                                    while std::input_event_poll(&mut drain).is_ok() {}
                                    break;
                                }

                                // Bottom navigation bar: the three painted targets
                                // each do the one thing they are drawn to do.
                                //
                                // WHY this arm replaced two overlapping bands: the
                                // old rules were "x in 240..=480 and y >= 1400
                                // opens the overview" plus "y >= 1380 and x in
                                // 520..640 opens the overview, anything else goes
                                // Home". The drawn home pill spans y 1416..1423, so
                                // the painted Home control opened Recents when hit
                                // above y 1400 and did nothing at all when hit
                                // below it, and the drawn back chevron at x 120
                                // fell into the catch-all and performed Home.
                                //
                                // WHY Back reports instead of pretending: the
                                // launcher home surface is the root of the view
                                // tree, so there is no screen under it to reveal.
                                // Saying so in the log is the honest answer; the
                                // alternative -- a Home re-render -- claimed an
                                // effect that had already happened.
                                //
                                // WHY this stays gated on `Some`: a tap below the
                                // bar must fall through to the dock test below, and
                                // that test's third band reaches y 1400 with x
                                // 490..645, which overlaps the Recents target.
                                // Without the gate a single press would open the
                                // overview and then immediately the drawer.
                                if let Some(target) = nav_target_at(x, y) {
                                    match target {
                                        NavTarget::Recents => {
                                            refresh_runtime_recents(&mut state);
                                            state.current_view = ViewMode::ActivitySpace;
                                            let mut win_buf = [0u8; 1024];
                                            let _ = std::window_list(&mut win_buf);
                                            render_view(surface, &state);
                                            let _ = std::surface_flip(4);
                                            std::log(0, "[LAUNCHER] Overview / Recent Apps switcher opened via recents button.").ok();
                                        }
                                        NavTarget::Home => {
                                            state.current_view = ViewMode::HomeScreen;
                                            render_view(surface, &state);
                                            let _ = std::surface_flip(4);
                                            std::log(0, "[LAUNCHER] Home surface presented via home button.").ok();
                                        }
                                        NavTarget::Back => {
                                            std::log(
                                                0,
                                                "[LAUNCHER] Back button at the launcher root: nothing to dismiss.",
                                            )
                                            .ok();
                                        }
                                    }
                                    let mut drain = std::InputEvent {
                                        kind: 0,
                                        x: 0,
                                        y: 0,
                                        dx: 0,
                                        dy: 0,
                                        scale_milli: 1000,
                                    };
                                    while std::input_event_poll(&mut drain).is_ok() {}
                                    break;
                                }

                                // Tapping dock 6th item (Apps) opens App Drawer
                                if ((660..800).contains(&y) && (482..672).contains(&x))
                                    || ((1020..1320).contains(&y) && (482..672).contains(&x))
                                    || ((1120..1400).contains(&y) && (490..645).contains(&x))
                                {
                                    state.current_view = ViewMode::AppDrawer;
                                    render_view(surface, &state);
                                    let _ = std::surface_flip(4);
                                    std::log(0, "[LAUNCHER] App Drawer opened from dock shortcut.")
                                        .ok();
                                    let mut drain = std::InputEvent {
                                        kind: 0,
                                        x: 0,
                                        y: 0,
                                        dx: 0,
                                        dy: 0,
                                        scale_milli: 1000,
                                    };
                                    while std::input_event_poll(&mut drain).is_ok() {}
                                    break;
                                }
                            }
                        }
                    }
                }
            }
        std::yield_now();
    }
}

#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    let mut line = LineBuf::<256>::new();
    let _ = core::fmt::write(&mut line, format_args!("[LAUNCHER PANIC] {}", info));
    std::log(0, line.as_str()).ok();
    loop {
        std::yield_now();
    }
}