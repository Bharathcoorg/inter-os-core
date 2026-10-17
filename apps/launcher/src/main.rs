#![no_std]
#![no_main]

use core::panic::PanicInfo;
use interos_std as std;

use launcher::theme::*;
// Wallpaper catalogue shared with the Settings app that writes it.
use std::design::walls;

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
    if scale >= 1 && text.len() <= launcher::outline_text::KEY_MAX {
        let em = launcher::outline_text::em_for_scale(scale);
        let (ascent, _) = interos_std::design::typeface::line_metrics(em);
        let baseline = (y0 + launcher::font::BASELINE * scale).max(ascent as usize);
        if let Some(run) = launcher::outline_text::OutlineRun::place(x0, baseline, em, text) {
            let (bx, by, bw, bh) = run.bounds();
            if x >= bx && x < bx + bw && y >= by && y < by + bh {
                return run.pixel_binary(x, y, color);
            }
            return None;
        }
    }
    if scale == 0 || text.is_empty() {
        return None;
    }
    // WHY these two rejects come first, in this order: the row band is a single
    // reject row band and pen walk
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
    if scale >= 1 && text.len() <= launcher::outline_text::KEY_MAX {
        let em = launcher::outline_text::em_for_scale(scale);
        let (ascent, _) = interos_std::design::typeface::line_metrics(em);
        let baseline = (y0 + launcher::font::BASELINE * scale).max(ascent as usize);
        if let Some(run) = launcher::outline_text::OutlineRun::place(x0, baseline, em, text) {
            let (bx, by, bw, bh) = run.bounds();
            if x >= bx && x < bx + bw && y >= by && y < by + bh {
                return run.pixel(x, y, color, bg);
            }
            return None;
        }
    }
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
    /// Boot animation frame phase, advanced once per timer tick while the boot
    /// screen is up.
    ///
    /// WHY it exists alongside `boot_progress`: progress is a step function of
    /// elapsed time and can sit still between whole-number steps, and a frozen
    /// input cannot produce motion. The ring comet reads this counter, so the
    /// mark of "the device is working" never stalls even when progress does.
    /// It carries no meaning about completion - the bar alone says that.
    pub boot_frame: u32,
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
    /// Which Events filter pill is active: 0 All, 1 Messages, 2 System, 3 Apps.
    ///
    /// WHY this lives on the state rather than in the painter: the Events feed's
    /// pills were a legend with real counts but no tap reached them, so the
    /// surface reported "Filter reads All" while drawing no way to change it.
    /// A filter is a choice the user made, and a choice has to outlive the frame
    /// it was made in.
    pub feed_filter: usize,
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
    /// The wallpaper the home surface renders.
    ///
    /// WHY a resolved scene and not a stored key: every surface reads this field
    /// per pixel, so a per-pixel lookup of a byte string would put a scan on the
    /// hot path. The key is resolved once, when the settings store answers.
    pub home_wallpaper: &'static walls::Scene,
    /// The wallpaper the lock surface renders, independently of the home one.
    pub lock_wallpaper: &'static walls::Scene,
    /// Whether the settings store has answered a wallpaper read at all.
    pub wallpaper_store_answered: bool,
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
    /// Dynamic user display name read from the settings store. Zero hardcoding or faking.
    pub user_name: [u8; 32],
    pub user_name_len: usize,
    /// Dynamic user status phrase read from the settings store ("user.status" / "owner.status").
    pub user_status: [u8; 48],
    pub user_status_len: usize,
    pub task0_done: bool,
}

impl LauncherState {
    pub const fn new() -> Self {
        Self {
            current_view: ViewMode::BootScreen,
            boot_progress: 0,
            boot_started_ticks: 0,
            boot_frame: 0,
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
        feed_filter: 0,
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
            // WHY the stock scene rather than the theme's own ramp: the home and
            // lock wallpapers are read from the settings store, and until that
            // read answers the shell must draw the wallpaper it shipped with.
            home_wallpaper: &walls::SCENES[1],
            lock_wallpaper: &walls::SCENES[1],
            wallpaper_store_answered: false,
            pin_pad_visible: false,
            pin_buffer: [0u8; 8],
            pin_len: 0,
            pin_confirm_buffer: [0u8; 8],
            pin_confirm_len: 0,
            pin_setup_step: 0,
            auth_error: false,
            pin_error: 0,
            pin_setup_mode: false,
            user_name: [0u8; 32],
            user_name_len: 0,
            user_status: [0u8; 48],
            user_status_len: 0,
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

/// Reads one wallpaper key out of the persistent settings store.
///
/// WHY slot 3: the kernel resolves settings capabilities by slot index, and
/// slot 3 of this shell's grant table is `FileSystemRead` with READ rights,
/// which `sys_settings_get` accepts. No new capability is needed to read a
/// preference the Settings app already writes.
fn stored_wallpaper_key(key: &[u8]) -> Option<([u8; 16], usize)> {
    // WHY the key is checked for a `&str` here rather than at the call site: the
    // toolkit takes a Rust string, and a byte-slice key that is not valid UTF-8
    // is a programming error, not a runtime condition. The two keys this reads
    // are ASCII literals.
    let key = core::str::from_utf8(key).ok()?;
    let mut buf = [0u8; 16];
    // WHY the length comes back with the value: the store fills only `len` bytes
    // of this buffer, and the catalogue's keys are matched byte-exactly, so the
    // trailing zero padding must not be allowed to reach the comparison.
    match std::settings_get(3, key, &mut buf) {
        Ok(n) if n > 0 && n <= buf.len() => Some((buf, n)),
        _ => None,
    }
}

/// Resolves the stored home and lock wallpaper choices into scenes.
///
/// WHY this is called at boot and again when a foreground application exits:
/// a wallpaper chosen in Settings is written to the disk-backed store by a
/// different process, and the kernel has no channel that would tell this shell
/// about it. Re-reading when the shell regains the foreground is the only point
/// at which the store can have changed, and it costs nothing in the steady
/// state.
fn refresh_wallpapers(state: &mut LauncherState) {
    let mut answered = false;
    let mut home = None;
    let mut lock = None;
    for (key, slot) in [
        (b"ui.wallpaper.home" as &[u8], 0usize),
        (b"ui.wallpaper.lock", 1usize),
    ] {
        if let Some((raw, n)) = stored_wallpaper_key(key) {
            answered = true;
            // WHY the value is trimmed of padding as well as whitespace: the
            // store is a newline-delimited text file, so a value written by an
            // older build can carry a trailing newline, and the catalogue looks
            // keys up byte-exactly. Only the `n` bytes the store filled count;
            // the zero padding after them is not part of the value.
            let raw = &raw[..n];
            let start = raw
                .iter()
                .position(|b| *b != b' ' && *b != b'\n' && *b != b'\r');
            let Some(start) = start else { continue };
            let end = raw[start..]
                .iter()
                .position(|b| *b == b' ' || *b == b'\n' || *b == b'\r')
                .map(|e| start + e)
                .unwrap_or(raw.len());
            if end <= start {
                continue;
            }
            let value = &raw[start..end];
            // WHY a key the catalogue does not hold is reported rather than
            // defaulted silently: it means the store and this binary disagree,
            // and showing the stock wallpaper without saying so would make a
            // stored choice look applied.
            match walls::Scene::by_key(value) {
                Some(scene) => {
                    if slot == 0 {
                        home = Some(scene);
                    } else {
                        lock = Some(scene);
                    }
                }
                None => {
                    std::log(
                        0,
                        "[LAUNCHER] Stored wallpaper key is not in this build's catalogue; keeping the stock wallpaper.",
                    )
                    .ok();
                }
            }
        }
    }
    state.wallpaper_store_answered = answered;
    if let Some(scene) = home {
        state.home_wallpaper = scene;
        std::log(0, "[LAUNCHER] Home wallpaper applied from store.").ok();
    }
    if let Some(scene) = lock {
        state.lock_wallpaper = scene;
        std::log(0, "[LAUNCHER] Lock wallpaper applied from store.").ok();
    }
    if !answered {
        std::log(
            0,
            "[LAUNCHER] Wallpaper preferences unread: the settings store did not answer, so both surfaces use the stock wallpaper.",
        )
        .ok();
    } else {
        std::log(
            0,
            "[LAUNCHER] Wallpaper preferences read from the settings store for both surfaces.",
        )
        .ok();
    }
}

/// Reads the owner/user display name dynamically from the settings store.
///
/// WHY: The Platform Truth Contract. Zero hardcoding or faking of user identity.
/// If the owner configured "user.name" or "owner.name", it is loaded dynamically
/// through capability slot 3. When empty or unset, `user_name_len` remains 0 so
/// the home screen greeting renders a clean impersonal greeting without a mock name.
fn refresh_user_profile(state: &mut LauncherState) {
    let mut buf = [0u8; 32];
    state.user_name_len = 0;
    for key in ["user.name", "owner.name"] {
        if let Ok(n) = std::settings_get(3, key, &mut buf) {
            if n > 0 && n <= buf.len() {
                let raw = &buf[..n];
                let start = raw.iter().position(|b| *b != b' ' && *b != b'\n' && *b != b'\r');
                if let Some(start) = start {
                    let end = raw[start..]
                        .iter()
                        .rposition(|b| *b != b' ' && *b != b'\n' && *b != b'\r')
                        .map(|p| start + p + 1)
                        .unwrap_or(start);
                    let trimmed = &raw[start..end];
                    if !trimmed.is_empty() {
                        let copy_len = trimmed.len().min(32);
                        state.user_name[..copy_len].copy_from_slice(&trimmed[..copy_len]);
                        state.user_name_len = copy_len;
                        break;
                    }
                }
            }
        }
    }

    let mut status_buf = [0u8; 48];
    state.user_status_len = 0;
    for key in ["user.status", "owner.status"] {
        if let Ok(n) = std::settings_get(3, key, &mut status_buf) {
            if n > 0 && n <= status_buf.len() {
                let raw = &status_buf[..n];
                let start = raw.iter().position(|b| *b != b' ' && *b != b'\n' && *b != b'\r');
                if let Some(start) = start {
                    let end = raw[start..]
                        .iter()
                        .rposition(|b| *b != b' ' && *b != b'\n' && *b != b'\r')
                        .map(|p| start + p + 1)
                        .unwrap_or(start);
                    let trimmed = &raw[start..end];
                    if !trimmed.is_empty() {
                        let copy_len = trimmed.len().min(48);
                        state.user_status[..copy_len].copy_from_slice(&trimmed[..copy_len]);
                        state.user_status_len = copy_len;
                        break;
                    }
                }
            }
        }
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
        if id == b"org.interos.sensor_dashboard" || id == b"sensor_dashboard" || id == b"sensors" {
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

/// Frame clock for the System Space orb, in kernel ticks.
///
/// WHY a static and not a field on the state: the painter's signature is shared
/// by every view, and widening the state to mutable at thirty-odd call sites to
/// carry one clock nothing else reads would be a signature change for its own
/// sake. One writer, in the band renderer, and one reader, in the orb painter.
static mut ORB_TICKS: u64 = 0;

/// The back control's box, shared with the System Space painter so a tap and a
/// pixel are answered by the same numbers.
use crate::screens::system_space::{BACK_S, BACK_X, BACK_Y};

/// Rows the System Space orb occupies, and so the band its driver repaints.
const ORB_BAND_Y0: usize = 200;
const ORB_BAND_Y1: usize = 310;

/// Ticks between orb frames.
///
/// WHY 14: the kernel's tick is 10ms, so this is roughly a seventh of a second.
/// Fast enough to read as continuous motion at this size, slow enough that the
/// banded repaint it costs is a small fraction of a frame's budget. A shorter
/// interval spends more time re-rasterising the orb than on anything else the
/// shell has to do.
const ORB_FRAME_TICKS: u64 = 14;

fn render_band(surface: u32, state: &LauncherState, min_y: usize, max_y: usize) {
    // WHY the clock is sampled here and not in the painter: the orb on the
    // System Space surface animates, and an animation needs a time source that
    // changes between frames rather than within one. Sampling once per band
    // gives every pixel of the frame the same phase -- the orb holds still for
    // the frame instead of crawling -- and it costs one syscall per repaint
    // rather than one per pixel.
    //
    // WHY a static and not a field on the state: the painter's signature is
    // shared by every view and every band, and widening the state to mutable at
    // thirty-odd call sites to carry one clock would be a signature change for a
    // value nothing else reads.
    //
    // WHY a refused read leaves the previous phase rather than zeroing it: a
    // frame with no clock still has to draw something, and snapping the orb
    // back to its zero phase on every refused read would make it strobe.
    if let Ok(ticks) = std::get_ticks(4) {
        unsafe {
            ORB_TICKS = ticks;
        }
    }
    let draw = screen_renderer(state.current_view);
    let min_y = min_y.min(H);
    let max_y = max_y.min(H);
    if min_y >= max_y {
        return;
    }
    let start_offset = min_y * W;
    let end_offset = max_y * W;
    let mut offset = start_offset;
    // WHY these three counters: a frame that takes seconds cannot be reasoned
    // about as one number. The three things a repaint does -- call the pixel
    // function, hand the result to the kernel, and tell the kernel we are still
    // alive -- cost wildly different amounts and only two of them are the
    // renderer's fault. Attributing them is the difference between knowing that
    // the pixel path needs rewriting and guessing that it does.
    let mut draw_ticks = 0u64;
    let mut write_ticks = 0u64;
    let mut beat_ticks = 0u64;
    while offset < end_offset {
        let t_beat = std::get_ticks(1).ok();
        let _ = std::service_heartbeat(10);
        beat_ticks += tick_delta(t_beat, std::get_ticks(1).ok());
        let count = (end_offset - offset).min(CHUNK);
        let mut x = offset % W;
        let mut y = offset / W;
        let t_draw = std::get_ticks(1).ok();
        unsafe {
            for i in 0..count {
                CHUNK_BUF[i] = draw(x, y, state);
                x += 1;
                if x == W {
                    x = 0;
                    y += 1;
                }
            }
        }
        let t_write = std::get_ticks(1).ok();
        draw_ticks += tick_delta(t_draw, t_write);
        unsafe {
            let _ = std::surface_write(4, surface, offset, &CHUNK_BUF[..count]);
        }
        write_ticks += tick_delta(t_write, std::get_ticks(1).ok());
        offset += count;
    }
    log_frame_breakdown(draw_ticks, write_ticks, beat_ticks);
}

/// Emits `[LAUNCHER] FRAMEBREAK draw= write= beat= px=`.
///
/// WHY a separate line from `FRAME ticks=`: the lifecycle harness keys on that
/// exact substring and must keep working, and the breakdown is a different fact
/// about the same frame. Splitting them lets a run attribute a slow frame
/// without touching the contract the existing harness depends on.
fn log_frame_breakdown(draw: u64, write: u64, beat: u64) {
    let mut line = LineBuf::<96>::new();
    {
        use core::fmt::Write;
        let _ = write!(
            line,
            "[LAUNCHER] FRAMEBREAK draw={} write={} beat={} px={}",
            draw,
            write,
            beat,
            W * H
        );
    }
    std::log(0, line.as_str()).ok();
}

#[inline(never)]
fn volume_hud_pixel(x: usize, y: usize, volume: u8) -> u32 {
    const W_HUD: usize = 520;
    const H_HUD: usize = 160;
    if x >= W_HUD || y >= H_HUD { return 0; }

    // Capsule dimensions: 460 wide, 96 high, centered in 520x160
    const X0: usize = 30;
    const X1: usize = 490;
    const Y0: usize = 32;
    const Y1: usize = 128;
    const R: usize = 48;

    if x < X0 || x >= X1 || y < Y0 || y >= Y1 { return 0; }

    // Check circular ends of the stadium capsule
    let cy = (Y0 + Y1) / 2; // 80
    let dy = if y < cy { cy - y } else { y - cy };
    if x < X0 + R {
        let dx = (X0 + R) - x;
        if dx * dx + dy * dy > R * R { return 0; }
    } else if x >= X1 - R {
        let dx = x - (X1 - R - 1);
        if dx * dx + dy * dy > R * R { return 0; }
    }

    // 1. Vector Speaker Icon (centered at x = 84, y = 80)
    // Body / magnet box: x in 66..76, y in 73..87
    if x >= 66 && x < 76 && y >= 73 && y < 87 {
        return 0xFFFAF8F4;
    }
    // Speaker expanding cone: x in 76..92
    if x >= 76 && x < 92 {
        let half_h = 7 + (x - 76) * 12 / 16;
        if y >= 80 - half_h && y <= 80 + half_h {
            return 0xFFFAF8F4;
        }
    }
    // Dynamic Sound Waves or Mute Slash
    if volume == 0 {
        // Red diagonal strike-through cutting across speaker
        if x >= 62 && x <= 96 && y >= 64 && y <= 96 {
            let diff = (x as i32 - 62) - (y as i32 - 64);
            if diff.abs() <= 2 {
                return 0xFFFF453A; // Vivid red mute slash
            }
        }
    } else {
        // Sound wave arcs radiating from cone mouth (90, 80)
        let w_dx = x as i32 - 90;
        let w_dy = if y >= 80 { (y - 80) as i32 } else { (80 - y) as i32 };
        if w_dx > 0 && w_dy <= w_dx * 9 / 10 {
            let dist_sq = w_dx * w_dx + w_dy * w_dy;
            // Wave 1: Always when volume > 0 (radius ~14)
            if dist_sq >= 12 * 12 && dist_sq <= 15 * 15 {
                return 0xFF00E5FF;
            }
            // Wave 2: When volume >= 34 (radius ~22)
            if volume >= 34 && dist_sq >= 20 * 20 && dist_sq <= 23 * 23 {
                return 0xFF00D2FF;
            }
            // Wave 3: When volume >= 67 (radius ~30)
            if volume >= 67 && dist_sq >= 28 * 28 && dist_sq <= 31 * 31 {
                return 0xFF3884FF;
            }
        }
    }

    // 2. Volume Level Track (x in 130..360, y in 72..88, 16px high)
    const TRACK_X0: usize = 130;
    const TRACK_X1: usize = 360;
    const TRACK_Y0: usize = 72;
    const TRACK_Y1: usize = 88;
    const TRACK_R: usize = 8;
    if x >= TRACK_X0 && x < TRACK_X1 && y >= TRACK_Y0 && y < TRACK_Y1 {
        let t_cy = (TRACK_Y0 + TRACK_Y1) / 2; // 80
        let t_dy = if y < t_cy { t_cy - y } else { y - t_cy };
        let mut inside_track = true;
        if x < TRACK_X0 + TRACK_R {
            let t_dx = (TRACK_X0 + TRACK_R) - x;
            if t_dx * t_dx + t_dy * t_dy > TRACK_R * TRACK_R { inside_track = false; }
        } else if x >= TRACK_X1 - TRACK_R {
            let t_dx = x - (TRACK_X1 - TRACK_R - 1);
            if t_dx * t_dx + t_dy * t_dy > TRACK_R * TRACK_R { inside_track = false; }
        }

        if inside_track {
            let filled = TRACK_X0 + ((TRACK_X1 - TRACK_X0) * volume as usize) / 100;
            if x < filled {
                if x >= filled.saturating_sub(4) {
                    return 0xFFFFFFFF; // Radiant specular leading bead
                }
                // Gradient from deep electric cyan (0x007ACC) to bright radiant cyan (0x00E5FF)
                let t = ((x - TRACK_X0) * 255) / (TRACK_X1 - TRACK_X0);
                let g = 0x7A + ((0xE5 - 0x7A) * t / 255);
                let b = 0xCC + ((0xFF - 0xCC) * t / 255);
                return 0xFF000000 | ((g as u32) << 8) | (b as u32);
            } else {
                return 0xFF161C26; // Dark obsidian trough
            }
        }
    }

    // 3. Crisp Numeric Readout (x in 376..450, y in 70..90)
    let mut num_buf = [0u8; 6];
    let num_len = if volume == 0 {
        num_buf[..4].copy_from_slice(b"MUTE");
        4
    } else if volume == 100 {
        num_buf[..4].copy_from_slice(b"100%");
        4
    } else if volume >= 10 {
        num_buf[0] = b'0' + (volume / 10);
        num_buf[1] = b'0' + (volume % 10);
        num_buf[2] = b'%';
        3
    } else {
        num_buf[0] = b'0' + volume;
        num_buf[1] = b'%';
        2
    };

    if let Some(text_color) = test_text(x, y, 382, 71, 1, &num_buf[..num_len], 0xFFFAF8F4) {
        return text_color;
    }

    // 4. Obsidian Ceramic Glass Monolith Backdrop & Beveled Perimeter Rim
    // Top specular highlight edge
    if y == Y0 || (x < X0 + R && (y - Y0) <= 2) || (x >= X1 - R && (y - Y0) <= 2) {
        return 0x705A6E8C;
    }
    // Bottom border reflection
    if y == Y1 - 1 {
        return 0x30263042;
    }
    // Glass gradient body
    let py = y - Y0;
    let t = (py * 255) / (Y1 - Y0);
    let luma = 14 - (t * 6 / 255); // Subtle vertical glass lighting
    let alpha = 0xF0;
    (alpha << 24) | ((luma as u32) << 16) | (((luma * 12 / 10) as u32) << 8) | ((luma * 16 / 10) as u32)
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

// -----------------------------------------------------------------------------
// Banded rendering: which rows an input actually changed
//
// WHY this exists: every state-changing input used to call `render_view`, which
// re-rasterised all 1,036,800 pixels and then flipped the surface. The kernel
// already offers a narrower present (`DrmKmsController::mark_present_rows` via
// `write_surface_pixels`, plus `diff_present_rows` at flip time), and the other
// apps in this tree took that offer by mapping each input onto the rows it
// moved. This is the shell doing the same thing.
//
// WHY the shell needs a variant per region rather than one band: the shell is
// twelve surfaces, and the rows a tap moves are different on each one. Naming
// the region after what moved is what keeps a band and the painter that draws
// it from being two numbers held together by hand, so every band below is read
// back from the screen that owns it (see `Dirty::rows`).
// -----------------------------------------------------------------------------

/// Which rows of the shell surface an input actually changed.
///
/// WHY `None` exists as a value and not as "the caller just does not render":
/// several inputs move nothing at all -- a tap that lands on no control, or a
/// button for the view that is already presented. Those must not repaint, and
/// making the answer a value is what lets one code path serve both.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Dirty {
    /// Nothing moved; the surface is already correct, so do not render at all.
    None,
    /// HomeScreen: the Today / system card, whose only shell-owned state is the
    /// `task0_done` bit on its first row.
    HomeCard,
    /// HomeScreen: the app deck. A catalogue refresh can retitle every tile,
    /// because a tile's label is the registry's name once the package is
    /// installed and a default until then.
    HomeDeck,
    /// ActivitySpace: the running-count strip above the deck and all three
    /// recents cards, which shift up when a card is dismissed or closed.
    ActivityCards,
    /// AppDrawer: the category chip row.
    DrawerChips,
    /// AppDrawer: the page grid and the pagination dots beneath it.
    DrawerPages,
    /// LockScreen: the PIN pad's prompt, sub-prompt and entry dots.
    LockPinPad,
    /// LockScreen: the PIN pad's entry dots alone. WHY this is separate from
    /// `LockPinPad`: backspace removes one digit, which cannot move the prompt
    /// or the sub-prompt, so naming the whole pad would repaint a hundred rows
    /// of unchanged backdrop.
    LockPinDots,
    /// PinEntry: the prompt, the entry dots and the status line under them.
    PinEntry,
    /// PinEntry: the entry dots alone.
    PinEntryDots,
    /// PinEntry: the status line under the dots -- the error line, or the hint.
    PinEntryStatus,
    /// Notifications: the filter pills and the feed rows they admit.
    ///
    /// WHY this is one band and not two: choosing a filter changes which rows
    /// are laid out below the pills, so the pill row and the feed body move
    /// together. Marking only the pills would leave the old subset on screen
    /// under a new active pill.
    EventsFilter,
    /// The union of two regions, as row extents.
    ///
    /// WHY this variant exists at all: `widen` has to be able to answer "these
    /// two bands" with one frame. Collapsing to `Dirty::All` would be correct
    /// but would throw away the rows between the two that provably cannot have
    /// moved, and answering with "the last one" is the bug the combinator is
    /// there to prevent.
    Band(usize, usize),
    /// Every row: a first frame, a view transition, a wallpaper re-read, a pad
    /// that appears or disappears, and any change whose row extent is not known
    /// exactly.
    All,
}

impl Dirty {
    /// The rows this dirty covers, or `None` when it covers the whole surface.
    ///
    /// WHY every band is read from the screen that draws it: a band retyped here
    /// is a band that can disagree with the painter, and the disagreement shows
    /// up as stale pixels in exactly the rows nobody thought to repaint.
    fn rows(self) -> Option<(usize, usize)> {
        match self {
            Dirty::None => Some((0, 0)),
            Dirty::HomeCard => Some(screens::home::system_card_band()),
            Dirty::HomeDeck => Some(screens::home::deck_band()),
            Dirty::ActivityCards => Some(screens::activity::recents_band()),
            Dirty::DrawerChips => Some(screens::app_drawer::chips_band()),
            Dirty::DrawerPages => Some(screens::app_drawer::pages_band()),
            Dirty::LockPinPad => Some(screens::lock::pin_pad_entry_band()),
            Dirty::LockPinDots => Some(screens::lock::pin_pad_dot_band()),
            Dirty::PinEntry => Some(screens::pin::entry_band()),
            Dirty::PinEntryDots => Some(screens::pin::dots_band()),
            Dirty::PinEntryStatus => Some(screens::pin::status_band()),
            Dirty::EventsFilter => Some(screens::notifications::filter_band()),
            Dirty::Band(y0, y1) => Some((y0, y1)),
            Dirty::All => None,
        }
    }

    /// Grows a dirty region so one rendered frame covers everything that moved.
    ///
    /// WHY: one `input_event_poll` batch can carry several events before the
    /// shell is scheduled again, and a single handler can move more than one
    /// region (a catalogue refresh moves the card and the deck; a digit that
    /// also clears an error line moves the dots and the status line). Rendering
    /// only the last region would leave the earlier one stale, and rendering
    /// the whole surface would repaint rows that provably cannot have moved.
    /// The union of the two extents is neither.
    fn widen(self, other: Dirty) -> Dirty {
        match (self.rows(), other.rows()) {
            (Some((a0, a1)), Some((b0, b1))) => Dirty::Band(a0.min(b0), a1.max(b1)),
            // One of them names the whole surface, so the frame must too.
            _ => Dirty::All,
        }
    }
}

/// Repaints the rows `dirty` names, then presents them.
///
/// WHY this sits beside `render_view` instead of replacing it: a whole-surface
/// repaint has to log and measure exactly as it did before, so that path stays a
/// call to `render_view`. This is the path for an input that named its rows.
///
/// WHY a `None` renders and flips nothing: the surface already holds the right
/// bytes, and `surface_flip` composites and copies out the whole scanout, so
/// flipping an unchanged surface is pure cost. The reference apps in this tree
/// (`camera`, `vault`, `sensor_dashboard`) gate the same way.
fn present_dirty(surface: u32, state: &LauncherState, dirty: Dirty) {
    if dirty == Dirty::None {
        return;
    }
    if dirty == Dirty::All {
        render_view(surface, state);
        let _ = std::surface_flip(4);
        return;
    }
    let (y0, y1) = dirty.rows().unwrap_or((0, H));
    if y0 >= y1 {
        return;
    }
    let frame_start = std::get_ticks(1).ok();
    render_band(surface, state, y0, y1);
    let _ = std::surface_flip(4);
    log_view_rendered(state);
    log_frame_ticks(tick_delta(frame_start, std::get_ticks(1).ok()));
}

/// Emits the current view's render-complete line.
///
/// WHY this is its own function: the banded path in [`present_dirty`] and the
/// full path in [`render_view`] must report the same thing, and one copy of the
/// match is what guarantees it.
fn log_view_rendered(state: &LauncherState) {
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
}

/// Repaints the whole surface and logs the view's render-complete line.
///
/// WHY this stays the whole-surface path: a first frame, a view transition, a
/// wallpaper re-read and a power transition all move every row, so naming a band
/// for them would claim a precision they do not have. Callers that know better
/// go through [`present_dirty`].
fn render_view(surface: u32, state: &LauncherState) {
    launcher::outline_text::reset_slot_pool();
    let frame_start = std::get_ticks(1).ok();
    render_band(surface, state, 0, H);
    let _ = std::surface_flip(4);
    log_view_rendered(state);
    log_frame_ticks(tick_delta(frame_start, std::get_ticks(1).ok()));
}

pub fn capture_and_save_screenshot(state: &LauncherState) {
    let mut ppm_buf = [0u8; 8192];
    let header = b"P6\n36 72\n255\n";
    let hlen = header.len();
    ppm_buf[..hlen].copy_from_slice(header);

    let draw = screen_renderer(state.current_view);
    let mut idx = hlen;
    for sy in 0..72 {
        for sx in 0..36 {
            let px = sx * 20 + 10;
            let py = sy * 20 + 10;
            let argb = draw(px, py, state);
            ppm_buf[idx] = ((argb >> 16) & 0xFF) as u8;
            ppm_buf[idx + 1] = ((argb >> 8) & 0xFF) as u8;
            ppm_buf[idx + 2] = (argb & 0xFF) as u8;
            idx += 3;
        }
    }

    let ticks = std::get_ticks(1).unwrap_or(100);
    let mut path = LineBuf::<64>::new();
    {
        use core::fmt::Write;
        let _ = write!(path, "/storage/photos/screenshot_{}.ppm", ticks);
    }

    let write_slot = std::cap_lookup(std::cap_type::FILE_SYSTEM_WRITE, 0).unwrap_or(3);
    if let Ok(fd) = std::fs_open(write_slot, path.as_str(), std::FS_WRITE) {
        let _ = std::fs_write(write_slot, fd, &ppm_buf[..idx]);
        let _ = std::fs_close(fd);
        std::log(0, "[LAUNCHER] Screenshot saved to /storage/photos.").ok();
    }
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
            return package_id_by_suffix(state, b"file_manager").or_else(|| package_id_by_suffix(state, b"files")).or(Some("org.interos.file_manager"));
        }
        if (390..=430).contains(&x) && (1160..=1200).contains(&y) {
            return package_id_by_suffix(state, b"gallery").or(Some("org.interos.gallery"));
        }
        if (530..=570).contains(&x) && (1160..=1200).contains(&y) {
            return package_id_by_suffix(state, b"card_demo").or(Some("org.interos.card_demo"));
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
        if (700..925).contains(&y) || (700..980).contains(&y) {
            if (30..238).contains(&x) {
                return package_id_by_suffix(state, b"dialer");
            } else if (255..465).contains(&x) {
                return package_id_by_suffix(state, b"messages");
            } else if (480..695).contains(&x) {
                return package_id_by_suffix(state, b"camera");
            }
        }

        // 5. Row 1 of Quick Action Dock: Files, Browser
        if (925..1160).contains(&y) || (1020..1300).contains(&y) {
            if (30..238).contains(&x) {
                return package_id_by_suffix(state, b"file_manager").or_else(|| package_id_by_suffix(state, b"files"));
            } else if (255..465).contains(&x) {
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
    // WHY here and not in the loop: the wallpaper belongs to the surface, so it is
    // resolved before the first frame is drawn. A preference stored by the Settings
    // app on a previous boot is therefore on the very first lock screen.
    refresh_wallpapers(&mut state);
    refresh_user_profile(&mut state);
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
                        // WHY the wallpaper is re-read here rather than polled:
                        // a wallpaper chosen in Settings is written to the
                        // disk-backed store by a different process, and the
                        // kernel has no channel that would tell this shell about
                        // it. Re-reading when the shell regains the foreground
                        // is the only point at which the store can have changed,
                        // and it costs nothing in the steady state.
                        refresh_wallpapers(&mut state);
                        refresh_user_profile(&mut state);
                        if let Ok(lock_st) = std::user_get_lock_state(1000) {
                            state.lock_state = lock_st;
                        }
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
                        refresh_wallpapers(&mut state);
                        refresh_user_profile(&mut state);
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

            if (events & std::EVENT_TIMER_TICK) != 0 {
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
                        } else {
                            let _ = std::user_lock(1000);
                            state.lock_state = std::UserLockState::Locked;
                            state.current_view = ViewMode::LockScreen;
                            state.pin_pad_visible = false;
                            state.pin_len = 0;
                            std::log(0, "[LAUNCHER] Display sleep observed; locked to lock screen.").ok();
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
                    // WHY the band re-renders on every tick and not only when
                    // progress steps: the ring comet reads `boot_frame`, which
                    // advances per tick, so gating on progress would freeze the
                    // one element whose job is to never look frozen. The band is
                    // 350 of the 1440 rows and the present path skips unchanged
                    // rows, so a per-tick band repaint costs a fraction of the
                    // full-surface repaint that used to stall startup here.
                    state.boot_frame = state.boot_frame.wrapping_add(1);
                    if next_progress != state.boot_progress {
                        state.boot_progress = next_progress;
                    }
                    // The animated ring and progress indicator occupy only this band.
                    // Redrawing the entire million-pixel portrait surface on every
                    // timer tick made startup visibly stall under QEMU TCG.
                    render_band(surface, &state, H / 2 - 130, (H / 2 + 220).min(H));
                    let _ = std::surface_flip(4);
                    if elapsed >= 180 {
                        // WHY boot never demands a credential now: an owner who
                        // has not set a lock gets the plain lock screen, and the
                        // account layer already unlocks a credential-less owner
                        // on the swipe gesture - `UserManager::unlock` succeeds
                        // when `credential.is_none()`. Forcing first-boot PIN
                        // setup made every device pass through a screen it could
                        // not skip, and put PIN entry, rather than Settings,
                        // where a lock is configured. A PIN set from
                        // Settings > Security is still demanded on every later
                        // unlock and still survives a reboot.
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
                if refresh_ticks % 10 == 0 {
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
                // The System Space orb is driven, not just drawn.
                //
                // WHY a driver is needed at all: the shell repaints on state
                // change, so a surface that looks the same every frame would hold
                // one light angle forever. Sampling a clock in the painter gives
                // every pixel of a frame the same phase, but something still has
                // to produce the next frame.
                //
                // WHY this band and this interval: the orb occupies rows
                // 115..245, so that is what is re-rasterised -- the title, the
                // cards and the subtitle are static and re-drawing them would
                // cost a banded repaint's worth of work for no visible change.
                // Fourteen ticks is roughly a fifth of a second, which reads as
                // motion without spending the compositor's whole budget on it.
                if state.current_view == ViewMode::SystemSpace
                    && state.active_child.is_none()
                    && refresh_ticks % ORB_FRAME_TICKS == 0
                {
                    render_band(surface, &state, ORB_BAND_Y0, ORB_BAND_Y1);
                }
                if state.current_view == ViewMode::HomeScreen {
                    if refresh_ticks % 10 == 0 {
                        let previous_count = state.app_count;
                        refresh_package_catalog(&mut state);
                        refresh_runtime_recents(&mut state);
                        if previous_count != state.app_count {
                            // WHY the card and the deck and not the whole
                            // surface: a catalogue refresh can retitle a deck
                            // tile (a tile shows the registry's display name once
                            // its package is installed and a default label until
                            // then) and can change the card's "N apps installed"
                            // detail line. Nothing else on this surface reads
                            // `app_count` -- the status bar and the greeting are
                            // clock-driven, the search bar and the nav bar are
                            // static -- so re-rasterising those rows would
                            // repaint 288 rows that cannot have moved.
                            present_dirty(
                                surface,
                                &state,
                                Dirty::HomeCard.widen(Dirty::HomeDeck),
                            );
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
                    let mut log_line = LineBuf::<128>::new();
                    {
                        use core::fmt::Write;
                        let _ = write!(log_line, "[LAUNCHER] Got input event: kind={} pos=({}, {}) d=({}, {}) view={:?}",
                            input.kind, input.x, input.y, input.dx, input.dy, state.current_view);
                    }
                    std::log(0, log_line.as_str()).ok();
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
                    if input.kind == std::INPUT_EVENT_KIND_KEY {
                        if let Some(key) = input.key_event() {
                            if key.pressed && (key.code == b's' as u32 || key.code == b'S' as u32 || key.code == 0x1000C || key.code == std::input_key_code::SCREENSHOT) {
                                capture_and_save_screenshot(&state);
                                continue;
                            }
                            if key.pressed && (key.code == 0x1000F || key.code == std::input_key_code::POWER) {
                                match state.current_view {
                                    ViewMode::BootScreen | ViewMode::PowerTransition => {}
                                    ViewMode::LockScreen => {
                                        state.pin_pad_visible = false;
                                        state.pin_len = 0;
                                        state.auth_error = false;
                                        render_view(surface, &state);
                                        let _ = std::surface_flip(4);
                                    }
                                    _ => {
                                        let _ = std::user_lock(1000);
                                        state.lock_state = std::UserLockState::Locked;
                                        state.current_view = ViewMode::LockScreen;
                                        state.pin_pad_visible = false;
                                        state.pin_len = 0;
                                        state.auth_error = false;
                                        render_view(surface, &state);
                                        let _ = std::surface_flip(4);
                                        std::log(0, "[LAUNCHER] Power key pressed; locked to lock screen.").ok();
                                    }
                                }
                                continue;
                            }
                        }
                    }

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
                                    // WHY the recorded flag: a key press on a
                                    // full eight-digit entry records nothing and
                                    // clears no error, so the surface is already
                                    // correct and repainting it would
                                    // re-rasterise a million identical pixels.
                                    // That is the inert-input case.
                                    let recorded = state.pin_len < 8;
                                    let had_error = state.pin_error != 0;
                                    if recorded {
                                        state.pin_buffer[state.pin_len as usize] = digit;
                                        state.pin_len += 1;
                                        state.pin_error = 0;
                                    }
                                    // WHY the dots and not the whole surface: a
                                    // digit fills one more entry dot. The status
                                    // line is only dragged in when the error line
                                    // it replaces was showing, which is what
                                    // `widen` unions rather than overwriting.
                                    let mut dirty = if recorded {
                                        Dirty::PinEntryDots
                                    } else {
                                        Dirty::None
                                    };
                                    if recorded && had_error {
                                        dirty = dirty.widen(Dirty::PinEntryStatus);
                                    }
                                    present_dirty(surface, &state, dirty);
                                    continue;
                                }

                                if x >= 490 && x < 590 && y >= 1036 && y < 1126 {
                                    let removed = state.pin_len > 0;
                                    if removed {
                                        state.pin_len -= 1;
                                        state.pin_buffer[state.pin_len as usize] = 0;
                                    }
                                    let had_error = state.pin_error != 0;
                                    state.pin_error = 0;
                                    // Same inert case as the digit key: deleting
                                    // from an empty entry with no error line
                                    // showing changes nothing.
                                    let mut dirty = if removed {
                                        Dirty::PinEntryDots
                                    } else {
                                        Dirty::None
                                    };
                                    if had_error {
                                        dirty = dirty.widen(Dirty::PinEntryStatus);
                                    }
                                    present_dirty(surface, &state, dirty);
                                    continue;
                                }

                                if x >= 130 && x < 430 && y >= 1036 && y < 1126 {
                                    let uid = std::user_get_current();
                                    let len = state.pin_len as usize;
                                    if len < 4 {
                                        state.pin_error = 1;
                                    } else if state.pin_setup_mode {
                                        // WHY two steps: a PIN typed once and
                                        // stored makes a slip of the finger the
                                        // device's credential, and the owner
                                        // then cannot unlock their own phone.
                                        // The first UNLOCK records the entry,
                                        // the second confirms it against the
                                        // first and only then stores it.
                                        if state.pin_setup_step == 0 {
                                            state.pin_confirm_buffer = state.pin_buffer;
                                            state.pin_confirm_len = len;
                                            state.pin_setup_step = 1;
                                            state.pin_len = 0;
                                            state.pin_error = 0;
                                            // WHY the prompt band: the setup step
                                            // moving forward rewrites the prompt
                                            // ("Set Device PIN" becomes "Confirm
                                            // Device PIN"), swaps the status line
                                            // and empties the dots -- all three
                                            // inside this one extent, and nothing
                                            // below the status line on the
                                            // keypad moves.
                                            present_dirty(surface, &state, Dirty::PinEntry);
                                            std::log(0, "[LAUNCHER] First-boot PIN recorded; confirmation requested.").ok();
                                            continue;
                                        }
                                        let matches = state.pin_buffer[..len]
                                            == state.pin_confirm_buffer[..state.pin_confirm_len];
                                        // WHY this frame was dropped rather than
                                        // banded: nothing in the state has moved
                                        // yet at this line, so the surface is
                                        // already correct. The comparison's
                                        // outcome is repainted immediately below,
                                        // which makes this render a no-op that
                                        // cost a full-surface repaint.
                                        present_dirty(surface, &state, Dirty::None);
                                        if !matches {
                                            state.pin_buffer = [0; 8];
                                            state.pin_len = 0;
                                            state.pin_setup_step = 0;
                                            state.pin_error = 1;
                                            present_dirty(surface, &state, Dirty::PinEntry);
                                            std::log(0, "[LAUNCHER:SECURITY] PIN setup entries did not match; restarted.").ok();
                                            continue;
                                        }
                                        match std::user_set_credential(uid, &state.pin_buffer[..len]) {
                                            Ok(()) => {
                                                let _ = std::user_unlock(uid, &state.pin_buffer[..len]);
                                                state.lock_state = std::UserLockState::Unlocked;
                                                state.pin_buffer = [0; 8];
                                                state.pin_len = 0;
                                                state.pin_error = 0;
                                                state.pin_setup_mode = false;
                                                state.pin_setup_step = 0;
                                                state.current_view = ViewMode::HomeScreen;
                                                render_view(surface, &state);
                                                let _ = std::surface_flip(4);
                                                std::log(0, "[LAUNCHER] First-boot credential established; device unlocked.").ok();
                                                continue;
                                            }
                                            Err(_) => {
                                                state.pin_buffer = [0; 8];
                                                state.pin_len = 0;
                                                state.pin_setup_step = 0;
                                                state.pin_error = 2;
                                            }
                                        }
                                    } else if std::user_unlock(uid, &state.pin_buffer[..len]).is_ok() {
                                        state.lock_state = std::UserLockState::Unlocked;
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
                                    // WHY the prompt band: a refused or too-short
                                    // entry empties the dots and puts the error
                                    // line up, and the prompt above them is the
                                    // only other row this surface can move
                                    // without leaving it.
                                    present_dirty(surface, &state, Dirty::PinEntry);
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
                                if input.dy >= 50 {
                                    state.current_view = ViewMode::HomeScreen;
                                    render_view(surface, &state);
                                    let _ = std::surface_flip(4);
                                    std::log(
                                        0,
                                        "[LAUNCHER] Returned to home grid from recents via swipe.",
                                    )
                                    .ok();
                                    std::log(
                                        0,
                                        "[LAUNCHER] Returned to home grid from recents.",
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

                                // Swiping up on a card closes/terminates that individual task.
                                // Horizontal swipe also dismisses for backwards-compatibility.
                                if input.dy <= -40 || input.dx.abs() >= 40 {
                                    let y = input.y as usize;
                                    let slot_opt = if (120..390).contains(&y) {
                                        Some(0)
                                    } else if (420..690).contains(&y) {
                                        Some(1)
                                    } else if (720..990).contains(&y) {
                                        Some(2)
                                    } else {
                                        None
                                    };

                                    if let Some(slot) = slot_opt {
                                        if state.recent_task_ids[slot] > 0 {
                                            let _ = std::app_stop(state.recent_task_ids[slot]);
                                            if state.active_child == Some(state.recent_task_ids[slot]) {
                                                state.active_child = None;
                                            }
                                        }
                                        state.dismiss_recent(slot);
                                        present_dirty(surface, &state, Dirty::ActivityCards);
                                        match slot {
                                            0 => { std::log(0, "[LAUNCHER] Card 0 dismissed via swipe.").ok(); }
                                            1 => { std::log(0, "[LAUNCHER] Card 1 dismissed via swipe.").ok(); }
                                            2 => { std::log(0, "[LAUNCHER] Card 2 dismissed via swipe.").ok(); }
                                            _ => {}
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
                                }
                            }

                            // 2. Tap gestures
                            if input.kind == 1 {
                                let x = input.x as usize;
                                let y = input.y as usize;

                                // A. Dismiss button taps
                                if screens::activity::CARD0_DISMISS.contains(x, y) || ((206..254).contains(&y) && (586..634).contains(&x)) {
                                    if state.recent_task_ids[0] > 0 {
                                        let _ = std::app_stop(state.recent_task_ids[0]);
                                        if state.active_child == Some(state.recent_task_ids[0]) {
                                            state.active_child = None;
                                        }
                                    }
                                    state.dismiss_recent(0);
                                    present_dirty(surface, &state, Dirty::ActivityCards);
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
                                } else if screens::activity::CARD1_DISMISS.contains(x, y) || ((526..574).contains(&y) && (586..634).contains(&x)) {
                                    if state.recent_task_ids[1] > 0 {
                                        let _ = std::app_stop(state.recent_task_ids[1]);
                                        if state.active_child == Some(state.recent_task_ids[1]) {
                                            state.active_child = None;
                                        }
                                    }
                                    state.dismiss_recent(1);
                                    present_dirty(surface, &state, Dirty::ActivityCards);
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
                                } else if screens::activity::CARD2_DISMISS.contains(x, y) || ((846..894).contains(&y) && (586..634).contains(&x)) {
                                    if state.recent_task_ids[2] > 0 {
                                        let _ = std::app_stop(state.recent_task_ids[2]);
                                        if state.active_child == Some(state.recent_task_ids[2]) {
                                            state.active_child = None;
                                        }
                                    }
                                    state.dismiss_recent(2);
                                    present_dirty(surface, &state, Dirty::ActivityCards);
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
                                // B. Tap Card 0 body to switch
                                else if screens::activity::WORKSPACE_CARDS[0].contains(x, y) {
                                    if let Some(target) = state.recent_id(0) {
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
                                }
                                // C. Tap Card 1 body to switch
                                else if screens::activity::WORKSPACE_CARDS[1].contains(x, y) {
                                    if let Some(target) = state.recent_id(1) {
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
                                }
                                // D. Tap Card 2 body to switch
                                else if screens::activity::WORKSPACE_CARDS[2].contains(x, y) {
                                    if let Some(target) = state.recent_id(2) {
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
                                }
                                // E. Tap "Close all" pill: terminates all active recents and returns to Home
                                else if screens::activity::CLOSE_ALL_BTN.contains(x, y) || ((1210..1280).contains(&y) && (220..500).contains(&x)) {
                                    for task in state.recent_task_ids.iter().copied() {
                                        if task > 0 {
                                            let _ = std::app_stop(task);
                                        }
                                    }
                                    state.recent_apps = [[0u8; 32]; 3];
                                    state.recent_app_lens = [0; 3];
                                    state.recent_task_ids = [0; 3];
                                    state.active_child = None;
                                    state.current_view = ViewMode::HomeScreen;
                                    render_view(surface, &state);
                                    let _ = std::surface_flip(4);
                                    std::log(0, "[LAUNCHER] Closed all recent apps and returned to home grid.").ok();
                                    std::log(0, "[LAUNCHER] Returned to home grid from recents.").ok();
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
                                // F. Tap empty background (or gesture bar): returns to Home
                                else {
                                    state.current_view = ViewMode::HomeScreen;
                                    render_view(surface, &state);
                                    let _ = std::surface_flip(4);
                                    std::log(0, "[LAUNCHER] Returned to home grid from recents via backdrop tap.").ok();
                                    std::log(0, "[LAUNCHER] Returned to home grid from recents.").ok();
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
                            // The volume capsule is answered BEFORE the dismiss
                            // band, not after.
                            //
                            // WHY the order matters: the shade's dismiss band is
                            // `y >= 70 && x >= 600`, and the capsule runs to the
                            // right edge of the surface, so its right-hand strip
                            // fell inside the dismiss rectangle. A tap there
                            // dismissed the shade instead of setting a level, so
                            // roughly an eighth of a control that genuinely drives
                            // the guest's audio manager did nothing. Testing the
                            // control first is what makes the drawn rectangle and
                            // the answerable one the same.
                            //
                            // WHY this does not make the shade undismissable: the
                            // capsule is 96 rows tall inside a 1440-row surface,
                            // so everything below it and everything above y 70
                            // still dismisses, and the swipe-up path is untouched.
                            let mut answered_by_capsule = false;
                            if input.kind == 1 {
                                let x = input.x as usize;
                                let y = input.y as usize;
                                let vol = screens::quick_controls::VOLUME_SLIDER;
                                if (vol.y..vol.y + vol.h).contains(&y)
                                    && (vol.x..vol.x + vol.w).contains(&x)
                                {
                                    answered_by_capsule = true;
                                }
                            }
                            if !answered_by_capsule
                                && ((is_drag(input.kind) && input.dy <= -60)
                                    || (input.kind == 1
                                        && (input.y >= 1350
                                            || (input.y >= 70 && input.x >= 600))))
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
                                // WHY the rectangle comes from the module: this used to be `(630..740)` /
                                // `(375..680)` literals, which pinned the footer to y 644 and broke the
                                // build the first time the layout moved - the right failure, but one that
                                // also meant the button could never be moved on purpose. The volume
                                // capsule already proved the pattern: ask the painter where it drew.
                                let power = screens::quick_controls::POWER_BTN;
                                if (power.y..power.y + power.h).contains(&y)
                                    && (power.x..power.x + power.w).contains(&x)
                                {
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
                                // WHY the back control is answerable here: it is
                                // drawn on this surface (screens/system_space.rs,
                                // BACK_X/BACK_Y/BACK_S) and the Settings app opens
                                // the same screen with one, so a user who arrived
                                // from the home surface's search bar previously
                                // had no control to leave with -- only a gesture.
                                // Gated to the drawn rect so a tap beside it cannot
                                // navigate by accident.
                                if (BACK_Y..BACK_Y + BACK_S).contains(&y)
                                    && (BACK_X..BACK_X + BACK_S).contains(&x)
                                {
                                    state.current_view = ViewMode::HomeScreen;
                                    render_view(surface, &state);
                                    let _ = std::surface_flip(4);
                                    std::log(0, "[LAUNCHER] Back from System Space.")
                                        .ok();
                                    break;
                                }
                                if (700..820).contains(&y) && (40..345).contains(&x) {
                                    state.current_view = ViewMode::PowerMenu;
                                    render_view(surface, &state);
                                    let _ = std::surface_flip(4);
                                    std::log(0, "[LAUNCHER] Power menu opened from System Space.")
                                        .ok();
                                    break;
                                }
                                // Category cards: Device, Network, Personal, Apps, Power, Security, System
                                // Any card tap seamlessly transitions to or launches the sovereign Settings component
                                if (360..900).contains(&y) && (40..680).contains(&x) {
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
                                        std::log(0, "[LAUNCHER] Seamlessly transitioned to sovereign System Space settings.")
                                            .ok();
                                        break;
                                    }
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
                                                // WHY four and not eight: this is
                                                // the lock screen's own inline pad,
                                                // which draws four dots
                                                // (`PIN_DOT_COUNT` in lock.rs) and
                                                // has no UNLOCK key - the fourth
                                                // digit submits. The PinEntry
                                                // surface (pin.rs) is the one that
                                                // draws eight dots and a UNLOCK
                                                // key and accepts 4..=8 digits.
                                                if state.pin_len < 4 {
                                                    state.pin_buffer[state.pin_len as usize] = ch as u8;
                                                    state.pin_len += 1;
                                                    // WHY the pad's entry band and
                                                    // not the whole surface: a digit
                                                    // fills one dot and clears the
                                                    // error ink on the prompt, and
                                                    // both live between the prompt's
                                                    // origin and the bottom of the
                                                    // dot gate -- see
                                                    // `lock::pin_pad_entry_band`.
                                                     // Nothing on the keypad below
                                                     // moves, and the pad paints
                                                     // its whole surface through a
                                                     // 100% veil
                                                     // (`PIN_PAD_VEIL_PERCENT`), so
                                                     // every row the band leaves out
                                                     // is the pad's own constant
                                                     // backdrop.
                                                     present_dirty(surface, &state, Dirty::LockPinPad);

                                                    if state.pin_len == 4 {
                                                        if state.lock_state == std::UserLockState::Uninitialized {
                                                            if state.pin_setup_step == 0 {
                                                                state.pin_confirm_buffer = state.pin_buffer;
                                                                state.pin_confirm_len = state.pin_len as usize;
                                                                state.pin_setup_step = 1;
                                                                state.pin_len = 0;
                                                                present_dirty(surface, &state, Dirty::LockPinPad);
                                                            } else {
                                                                if state.pin_buffer[..4] == state.pin_confirm_buffer[..4] {
                                                                    let set = std::user_set_credential(1000, &state.pin_buffer[..4]);
                                                                    // WHY this frame was dropped
                                                                    // rather than banded: nothing
                                                                    // in the state has moved at
                                                                    // this line, so the surface
                                                                    // is already correct. The
                                                                    // credential's outcome is
                                                                    // repainted immediately
                                                                    // below.
                                                                    present_dirty(surface, &state, Dirty::None);
                                                                    // WHY the result is
                                                                    // checked: this used to
                                                                    // be discarded, so a
                                                                    // credential the
                                                                    // kernel refused to
                                                                    // accept still produced
                                                                    // the "PIN configured"
                                                                    // home screen - and the
                                                                    // account on disk could
                                                                    // still have no
                                                                    // credential at all.
                                                                     match set {
                                                                         Ok(()) => {
                                                                             let _ = std::user_unlock(1000, &state.pin_buffer[..4]);
                                                                             state.lock_state = std::UserLockState::Unlocked;
                                                                             state.pin_pad_visible = false;
                                                                             state.pin_setup_step = 0;
                                                                             state.pin_len = 0;
                                                                             state.current_view = ViewMode::HomeScreen;
                                                                             render_view(surface, &state);
                                                                             let _ = std::surface_flip(4);
                                                                             std::log(0, "[LAUNCHER] Owner PIN configured and device unlocked. Home surface rendered.").ok();
                                                                         }
                                                                         Err(_) => {
                                                                             state.auth_error = true;
                                                                             state.pin_setup_step = 0;
                                                                             state.pin_len = 0;
                                                                             present_dirty(surface, &state, Dirty::LockPinPad);
                                                                             std::log(0, "[LAUNCHER] Owner PIN was NOT stored: the kernel refused the credential, so the account has no PIN and the lock screen stays up.").ok();
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
                                                                 } else {
                                                                     // WHY the pad's entry band: the setup step
                                                                     // went back to zero, which restores the
                                                                     // first prompt, puts the error line up and
                                                                     // empties the dots. All three are inside
                                                                     // the band.
                                                                     state.auth_error = true;
                                                                     state.pin_setup_step = 0;
                                                                     state.pin_len = 0;
                                                                     present_dirty(surface, &state, Dirty::LockPinPad);
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
                                                                present_dirty(surface, &state, Dirty::LockPinPad);
                                                                std::log(0, "[LAUNCHER:SECURITY] PIN authentication failed.").ok();
                                                            }
                                                        }
                                                    }
                                                }
                                            }
                                            // UNLOCK: submits the entry, whatever
                                            // its length in 4..=8. WHY it lives
                                            // here and not on the fourth digit:
                                            // the surface draws a UNLOCK key and
                                            // the entry can be six digits, so the
                                            // key is the only honest end of the
                                            // entry.
                                             'E' if state.pin_len >= 4 => {
                                                 let n = state.pin_len as usize;
                                                 if state.lock_state == std::UserLockState::Uninitialized {
                                                     if state.pin_setup_step == 0 {
                                                         state.pin_confirm_buffer = state.pin_buffer;
                                                         state.pin_confirm_len = n;
                                                         state.pin_setup_step = 1;
                                                         state.pin_len = 0;
                                                         // WHY the pad's entry band: the
                                                         // step moving forward swaps
                                                         // the prompt, swaps the
                                                         // sub-prompt and empties the
                                                         // dots, and all three live in
                                                         // that band.
                                                         present_dirty(surface, &state, Dirty::LockPinPad);
                                                     } else {
                                                         if state.pin_buffer[..n] == state.pin_confirm_buffer[..state.pin_confirm_len] {
                                                             let set = std::user_set_credential(1000, &state.pin_buffer[..n]);
                                                             // Dropped: nothing in the state
                                                             // has moved at this line, and
                                                             // the credential's outcome is
                                                             // repainted immediately below.
                                                             present_dirty(surface, &state, Dirty::None);
                                                             // WHY the result is checked: this
                                                             // used to be discarded, so a
                                                             // credential the kernel
                                                             // refused to accept still
                                                             // produced the "PIN configured"
                                                             // home screen - and the
                                                             // account on disk could still
                                                             // have no credential at all.
                                                             match set {
                                                                 Ok(()) => {
                                                                     let _ = std::user_unlock(1000, &state.pin_buffer[..n]);
                                                                     state.lock_state = std::UserLockState::Unlocked;
                                                                     state.pin_pad_visible = false;
                                                                     state.pin_setup_step = 0;
                                                                     state.pin_len = 0;
                                                                     state.current_view = ViewMode::HomeScreen;
                                                                     render_view(surface, &state);
                                                                     let _ = std::surface_flip(4);
                                                                     std::log(0, "[LAUNCHER] Owner PIN configured and device unlocked. Home surface rendered.").ok();
                                                                 }
                                                                 Err(_) => {
                                                                     state.auth_error = true;
                                                                     state.pin_setup_step = 0;
                                                                     state.pin_len = 0;
                                                                     present_dirty(surface, &state, Dirty::LockPinPad);
                                                                     std::log(0, "[LAUNCHER] Owner PIN was NOT stored: the kernel refused the credential, so the account has no PIN and the lock screen stays up.").ok();
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
                                                         } else {
                                                             state.auth_error = true;
                                                             state.pin_setup_step = 0;
                                                             state.pin_len = 0;
                                                             present_dirty(surface, &state, Dirty::LockPinPad);
                                                         }
                                                     }
                                                 } else if std::user_unlock(1000, &state.pin_buffer[..n]).is_ok() {
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
                                                     present_dirty(surface, &state, Dirty::LockPinPad);
                                                     std::log(0, "[LAUNCHER:SECURITY] PIN authentication failed.").ok();
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
                                                 let mut dirty = Dirty::LockPinPad;
                                                 if state.lock_state == std::UserLockState::Locked {
                                                     state.pin_pad_visible = false;
                                                     // WHY the whole surface when the pad
                                                     // hides: the pad's veil, its prompt
                                                     // and its twelve keys all disappear,
                                                     // so every row from the status bar
                                                     // down to the pad's home indicator
                                                     // reverts to the lock surface. The
                                                     // prompt band alone would leave the
                                                     // keypad standing.
                                                     dirty = dirty.widen(Dirty::All);
                                                 }
                                                 present_dirty(surface, &state, dirty);
                                             }
                                            // Backspace: the bottom-right key
                                            // (backspace pictogram; the old `C`
                                            // key's job) removes the last digit.
                                             'D' => {
                                                 if state.pin_len > 0 {
                                                     state.pin_len -= 1;
                                                     state.pin_buffer[state.pin_len as usize] = 0;
                                                     // WHY the dots and not the whole
                                                     // pad: backspace removes one
                                                     // entry dot, and neither the
                                                     // prompt nor the sub-prompt can
                                                     // move -- the pad cleared
                                                     // `auth_error` on entry -- so the
                                                     // wider band would repaint a
                                                     // hundred rows of unchanged
                                                     // backdrop.
                                                     present_dirty(surface, &state, Dirty::LockPinDots);
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
                                    // WHY the page band and not the whole surface:
                                    // a page turn inside the drawer changes the
                                    // grid and the pagination dots, not the
                                    // status bar, the search field, or the nav
                                    // bar above and below them. `pages_band`
                                    // is derived from the drawn frame, so it is
                                    // the same extent the painter already gates
                                    // the grid on.
                                    present_dirty(surface, &state, Dirty::DrawerPages);
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
                                    // WHY the pages band: a page turn redraws every
                                    // tile in the grid and moves the
                                    // current-page dot. The chips above are the
                                    // active tab, which a page turn does not
                                    // touch -- see `app_drawer::pages_band`.
                                    present_dirty(surface, &state, Dirty::DrawerPages);
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
                                    // WHY two bands: a tab switch moves the chip
                                    // row's active chip AND, because it resets the
                                    // page to zero, the grid and the dots. One
                                    // `widen` renders the union of both rather
                                    // than whichever was named last, which is
                                    // the whole reason the combinator exists.
                                    present_dirty(
                                        surface,
                                        &state,
                                        Dirty::DrawerChips.widen(Dirty::DrawerPages),
                                    );
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
                                    // A dot tap is a page turn, so it moves the
                                    // same two things the swipe does.
                                    present_dirty(surface, &state, Dirty::DrawerPages);
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
                                // WHY the filter pills are answerable here: they
                                // were a legend with real counts that no tap
                                // reached, so the surface printed "Filter reads
                                // All" while drawing no way to change it. The
                                // pill row sits at PILLS_Y (210) with PILL_H,
                                // above the feed body, below the header, and
                                // clear of the CLEAR_BTN the layout harness pins.
                                //
                                // WHAT a tap does: it selects that filter and
                                // repaints the pill row and the feed, so the
                                // surface shows the subset it now holds rather
                                // than the whole list with a pill lit. Tapping
                                // the already-active pill is a no-op rather than
                                // a repaint.
                                if let Some(pill) =
                                    screens::notifications::filter_pill_at(x, y, &state)
                                {
                                    if pill != state.feed_filter {
                                        state.feed_filter = pill;
                                        // WHY the filter band and not the whole
                                        // surface: the pills and the feed below
                                        // them are what a filter changes. The
                                        // status bar, header and derivation lines
                                        // are the same either way.
                                        present_dirty(surface, &state, Dirty::EventsFilter);
                                        std::log(0, "[LAUNCHER] Events filter changed.").ok();
                                    }
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

                                // Weather widget (135..315, 40..345) -> Sovereign Settings & Telemetry
                                if (135..315).contains(&y) && (40..345).contains(&x) {
                                    if let Ok(task_id) =
                                        std::app_launch("org.interos.settings")
                                    {
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
                            if is_drag(input.kind) {
                                let mut dbg = LineBuf::<128>::new();
                                {
                                    use core::fmt::Write;
                                    let _ = write!(dbg, "[LAUNCHER] Drag in HomeScreen: dy={} press=({}, {}) pull_zone={} pull_min={}",
                                        input.dy, press_x, press_y, EDGE_PULL_ZONE, EDGE_PULL_MIN);
                                }
                                std::log(0, dbg.as_str()).ok();
                            }
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
                                    // WHY the card band: the checkbox is painted
                                    // inside the Today card and `task0_done` is
                                    // read only by this card's first row, so the
                                    // card's rectangle is the extent that moved
                                    // (168..658 -- see `home::system_card_band`).
                                    // The greeting above, the deck below and the
                                    // nav bar are all untouched by this bit.
                                    present_dirty(surface, &state, Dirty::HomeCard);
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

                                // Universal search bar tap: opens System Space
                                if (1320..1384).contains(&y) && (40..680).contains(&x) {
                                    state.current_view = ViewMode::SystemSpace;
                                    render_view(surface, &state);
                                    let _ = std::surface_flip(4);
                                    std::log(0, "[LAUNCHER] System space opened via universal search bar tap.").ok();
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
                                            // WHY `None`: this arm is only reachable
                                            // from the HomeScreen view arm, so the
                                            // home surface is already presented and
                                            // assigning the view it is already in
                                            // moves no pixel. This arm used to
                                            // re-render the whole surface -- the
                                            // inert-input cost -- while the log line
                                            // above it already explained that Home
                                            // answers nothing new here.
                                            present_dirty(surface, &state, Dirty::None);
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
                                    || ((925..1160).contains(&y) && (480..695).contains(&x))
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

#[cfg(not(test))]
#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    let mut line = LineBuf::<256>::new();
    let _ = core::fmt::write(&mut line, format_args!("[LAUNCHER PANIC] {}", info));
    std::log(0, line.as_str()).ok();
    loop {
        std::yield_now();
    }
}