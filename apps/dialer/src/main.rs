//! Inter OS Native Sovereign Telephony & Dialer App
//!
//! Native telephony dialer executing in Ring 3 (User Mode):
//! - Dual-Tone Multi-Frequency (DTMF) acoustic tone synthesis for keypad keys (0-9, *, #)
//! - Call State Machine: Idle, Dialing, Ringing, ActiveCall, CallHeld, CallEnded
//! - Cellular Baseband Modem HAL telemetry: 5G Carrier, RSSI, Cell ID, 3GPP AT state
//! - Local Contacts Book & Call History ledger
//! - Emergency E911/112 override pipeline
//! - Android Compatibility Runtime (ACR) cross-zone intent bridge dispatch
//! - DRM/KMS Client Surface Rendering (z=200) with Mobile Phone Keypad & Status Display
//! - Asynchronous event reactor loop handling touch input, modem URCs, and timers
//!
//! Typed input (mirrors `apps/browser/src/main.rs`):
//! - The dialed-number display is now a real keyboard target. `0-9`, `*` and `#`
//!   append and play the real DTMF tone, BACKSPACE deletes, ENTER dials, ESCAPE
//!   clears the entry and TAB steps the section. The app previously had no key
//!   handler at all.
//!
//! Live vs. honestly-disabled controls (nothing here is drawn and inert any more):
//! - The segmented tabs and the bottom dock were both drawn and both dead. They
//!   now drive one real `DialerSection` state: Keypad and Recents are live;
//!   Contacts is drawn in the recessed treatment because the platform exposes no
//!   contact store, and a tap on it reports that rather than doing nothing.
//! - Recents is backed by a real ledger of every dial attempt this session made
//!   and the outcome the telephony service actually returned for it.
//! - The signal bars are drawn from the real `"bars"` field of the modem
//!   telemetry, not from a boolean. The battery is drawn only when
//!   `battery_query` actually returned a state-of-charge; otherwise nothing is
//!   drawn there, as the browser does for telemetry it does not receive.
//! - The call control is drawn recessed while the number field is empty, because
//!   there is nothing it could dial.
//!
//! Banded rendering:
//! - `Dirty` + `render_dialer_band(surface, app, y0, y1)` mirror the browser. The
//!   previous renderer pushed all 1,036,800 pixels through `get_dialer_pixel` for
//!   every touch event and every keystroke.

#![no_std]
#![no_main]

mod outline_text;

use core::panic::PanicInfo;
use interos_std as std;

const W: usize = 720;
const H: usize = 1440;
const CHUNK: usize = 4096;

/// Current execution state of the telephony call engine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CallState {
    Idle = 0,
    Dialing = 1,
    Ringing = 2,
    ActiveCall = 3,
    CallHeld = 4,
    CallEnded = 5,
}

/// Which content the tabs and the dock select.
///
/// WHY an enum with no `Contacts` arm: the platform exposes no contact store
/// (`CONTACTS_BOOK` is a zero-length array by design), so there is no contacts
/// screen to select. The tab and the dock slot that used to imply one are drawn
/// in the recessed, unavailable treatment instead.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DialerSection {
    /// The 3x4 dial pad and the call controls.
    Keypad = 0,
    /// The real ledger of dial attempts made in this session.
    Recents = 1,
}

/// Real outcome of the last call attempt, drawn under the dialed number.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CallOutcome {
    /// No attempt yet.
    Idle,
    /// A dial request is with the telephony service.
    Dialing,
    /// The telephony service accepted the call.
    Active,
    /// The telephony service refused the call.
    Refused,
    /// An emergency dial attempt was refused by the telephony service.
    EmergencyRefused,
}

impl CallOutcome {
    /// Caption drawn under the dialed number. Empty for `Idle` - an empty run is
    /// never drawn at all, so an idle dialer shows no status line rather than a
    /// placeholder that could be mistaken for one.
    fn caption(&self) -> &'static [u8] {
        match self {
            CallOutcome::Idle => b"",
            CallOutcome::Dialing => b"Calling...",
            CallOutcome::Active => b"Call active",
            CallOutcome::Refused => b"Call refused by the telephony service",
            CallOutcome::EmergencyRefused => b"Emergency call refused by the telephony service",
        }
    }
}

/// One real entry in the recents ledger.
#[derive(Clone, Copy)]
pub struct CallRecord {
    pub number: [u8; 32],
    pub len: usize,
    /// `true` only when `telephony_dial` returned `Ok`.
    pub accepted: bool,
    /// `true` when the entry came from the emergency control.
    pub emergency: bool,
}

/// Keypad DTMF frequency pair (low frequency, high frequency in Hz).
#[derive(Debug, Clone, Copy)]
pub struct DtmfPair {
    pub low_freq: u32,
    pub high_freq: u32,
}

/// Resolves standard CCITT dual-tone frequencies for telephone keypad keys.
pub fn get_dtmf_frequencies(key: char) -> Option<DtmfPair> {
    match key {
        '1' => Some(DtmfPair {
            low_freq: 697,
            high_freq: 1209,
        }),
        '2' => Some(DtmfPair {
            low_freq: 697,
            high_freq: 1336,
        }),
        '3' => Some(DtmfPair {
            low_freq: 697,
            high_freq: 1477,
        }),
        '4' => Some(DtmfPair {
            low_freq: 770,
            high_freq: 1209,
        }),
        '5' => Some(DtmfPair {
            low_freq: 770,
            high_freq: 1336,
        }),
        '6' => Some(DtmfPair {
            low_freq: 770,
            high_freq: 1477,
        }),
        '7' => Some(DtmfPair {
            low_freq: 852,
            high_freq: 1209,
        }),
        '8' => Some(DtmfPair {
            low_freq: 852,
            high_freq: 1336,
        }),
        '9' => Some(DtmfPair {
            low_freq: 852,
            high_freq: 1477,
        }),
        '0' => Some(DtmfPair {
            low_freq: 941,
            high_freq: 1336,
        }),
        '*' => Some(DtmfPair {
            low_freq: 941,
            high_freq: 1209,
        }),
        '#' => Some(DtmfPair {
            low_freq: 941,
            high_freq: 1477,
        }),
        _ => None,
    }
}

/// Sovereign Contact Record.
#[derive(Debug, Clone, Copy)]
pub struct Contact {
    pub name: &'static str,
    pub number: &'static str,
    pub did: &'static str,
    pub is_emergency: bool,
}

/// Contacts are populated only from real user/contact state.
pub static CONTACTS_BOOK: [Contact; 0] = [];

/// Entries the recents ledger keeps. Bounded so the ledger stays a fixed-size
/// stack allocation; the oldest entry is dropped once it is full.
const RECENTS_CAP: usize = 5;

/// Production Telephony Dialer state manager.
pub struct DialerApp {
    pub state: CallState,
    pub active_number: [u8; 32],
    pub active_len: usize,
    pub is_muted: bool,
    pub is_speaker: bool,
    pub call_duration_secs: u32,
    pub calls_completed: u32,
    /// Whether the telephony service actually reported a baseband present.
    pub modem_present: bool,
    /// Section the tabs and the dock select.
    pub section: DialerSection,
    /// Real outcome of the last call attempt.
    pub outcome: CallOutcome,
    /// Signal bars as the modem HAL reported them (0..=4), never invented from
    /// the `present` boolean.
    pub signal_bars: u8,
    /// State of charge as `battery_query` reported it. `None` when the kernel
    /// refused or reported no fuel gauge, in which case nothing is drawn.
    pub battery_soc: Option<u8>,
    /// Real ledger of dial attempts, newest first.
    pub recents: [CallRecord; RECENTS_CAP],
    pub recents_len: usize,
}

impl DialerApp {
    pub const fn new() -> Self {
        Self {
            state: CallState::Idle,
            active_number: [0u8; 32],
            active_len: 0,
            is_muted: false,
            is_speaker: false,
            call_duration_secs: 0,
            calls_completed: 0,
            // WHY: fail closed. Until the telephony service reports a baseband,
            // no inbound-call path may be claimed.
            modem_present: false,
            section: DialerSection::Keypad,
            outcome: CallOutcome::Idle,
            signal_bars: 0,
            battery_soc: None,
            recents: [CallRecord {
                number: [0; 32],
                len: 0,
                accepted: false,
                emergency: false,
            }; RECENTS_CAP],
            recents_len: 0,
        }
    }

    /// Plays dual-tone multi-frequency acoustic feedback for a pressed keypad digit.
    pub fn play_dtmf(&self, digit: char) {
        if let Some(pair) = get_dtmf_frequencies(digit) {
            // WHY only the high tone: the audio service exposes a single
            // oscillator (`audio_play_tone`), so only one of the two CCITT
            // frequencies can ever be emitted. The low tone is reported by
            // `get_dtmf_frequencies` and drawn in the key sublabel, never
            // claimed to have sounded.
            std::audio_play_tone(pair.high_freq).ok();
            std::audio_haptic_click().ok();
            std::audio_silence().ok();
        }
    }

    /// Appends a dialed digit to the accumulator and plays its DTMF tone.
    pub fn input_digit(&mut self, digit: char) {
        if self.active_len < self.active_number.len() {
            self.active_number[self.active_len] = digit as u8;
            self.active_len += 1;
            self.play_dtmf(digit);
        }
    }

    /// Removes the last entered digit.
    pub fn backspace(&mut self) {
        if self.active_len > 0 {
            self.active_len -= 1;
            std::audio_haptic_click().ok();
        }
    }

    /// Clears the entry without placing a call.
    pub fn clear_entry(&mut self) {
        if self.active_len > 0 {
            self.active_len = 0;
            let _ = std::audio_haptic_click();
        }
    }

    /// Appends one real dial attempt to the recents ledger, newest first.
    fn record_call(&mut self, number: &str, accepted: bool, emergency: bool) {
        let n = number.len().min(self.active_number.len());
        let mut entry = CallRecord {
            number: [0u8; 32],
            len: n,
            accepted,
            emergency,
        };
        let mut idx = 0usize;
        while idx < n {
            entry.number[idx] = number.as_bytes()[idx];
            idx += 1;
        }
        if self.recents_len < RECENTS_CAP {
            self.recents_len += 1;
            for i in (1..self.recents_len).rev() {
                self.recents[i] = self.recents[i - 1];
            }
        } else {
            // The ledger is full: the oldest entry is dropped so the newest
            // attempt is always the one on screen.
            for i in (1..RECENTS_CAP).rev() {
                self.recents[i] = self.recents[i - 1];
            }
        }
        self.recents[0] = entry;
    }

    /// Initiates an outgoing cellular call.
    pub fn dial(&mut self, number: &str) -> Result<(), i64> {
        self.dial_internal(number, false)
    }

    /// Places an emergency call after entering the real emergency number.
    ///
    /// The platform's emergency pipeline is E911/112; the digits below are the
    /// number that is actually sent, so the ledger and the status line can both
    /// name it.
    pub fn dial_emergency(&mut self) -> Result<(), i64> {
        self.active_len = 0;
        self.input_digit('9');
        self.input_digit('1');
        self.input_digit('1');
        self.dial_internal("911", true)
    }

    fn dial_internal(&mut self, number: &str, emergency: bool) -> Result<(), i64> {
        if number.is_empty() || number.len() > self.active_number.len() {
            return Err(-22);
        }

        self.state = CallState::Dialing;
        self.outcome = CallOutcome::Dialing;
        self.call_duration_secs = 0;

        // A call is connected only after the authoritative telephony service
        // accepts the request. UI/ACR dispatch must never manufacture an
        // ActiveCall state.
        match std::telephony_dial(number) {
            Ok(()) => {
                self.state = CallState::ActiveCall;
                self.outcome = CallOutcome::Active;
                std::audio_play_tone(440).ok();
                std::audio_silence().ok();
                std::log(0, "[DIALER APP] Telephony service accepted call request.").ok();
                self.record_call(number, true, emergency);
                Ok(())
            }
            Err(err) => {
                self.state = CallState::CallEnded;
                self.outcome = if emergency {
                    CallOutcome::EmergencyRefused
                } else {
                    CallOutcome::Refused
                };
                std::log(0, "[DIALER APP] Call request rejected by telephony service.").ok();
                self.record_call(number, false, emergency);
                Err(err)
            }
        }
    }

    /// Terminates an active call.
    ///
    /// Only a call that actually reached the telephony service counts as
    /// completed and only that case releases a channel.
    pub fn hangup(&mut self) {
        let had_connected_call = self.state == CallState::ActiveCall;
        self.state = CallState::CallEnded;
        let _ = std::audio_haptic_click();
        self.state = CallState::Idle;
        self.active_len = 0;
        if had_connected_call {
            self.calls_completed += 1;
            // WHY: this line used to claim the channel was released to the
            // modem pool, but hangup() issued no telephony syscall at all, so no
            // modem state was ever touched. Ask the service and report the
            // outcome it actually returned.
            if std::telephony_hangup().is_ok() {
                std::log(
                    0,
                    "[DIALER APP] Call Terminated: telephony service released the channel.",
                )
                .ok();
            } else {
                std::log(
                    0,
                    "[DIALER APP] Call Terminated locally: the telephony service refused the hangup, so no channel was released.",
                )
                .ok();
            }
        } else {
            // WHY: no call was ever established, so there is no channel to
            // release and no completed call to count. Reporting a released
            // channel here would be a success claim for a rejected operation.
            std::log(
                0,
                "[DIALER APP] Hangup with no connected call; no channel was released.",
            )
            .ok();
        }
    }

    /// Steps the section. Contacts is deliberately not a target.
    pub fn step_section(&mut self) -> DialerSection {
        self.section = match self.section {
            DialerSection::Keypad => DialerSection::Recents,
            DialerSection::Recents => DialerSection::Keypad,
        };
        self.section
    }
}

// -----------------------------------------------------------------------------
// 32-bit ARGB8888 Design Color Tokens (Screen 9 Modern Light Theme)
// -----------------------------------------------------------------------------
const BG_DARK: u32 = 0xFFF7F6F2;
const SURFACE_DARK: u32 = 0xFFFFFFFF;
const SURFACE_RAISED: u32 = 0xFFEFEEE9;
const BORDER_SUBTLE: u32 = 0xFFE1E0DB;
const TEXT_PRIMARY: u32 = 0xFF111214;
const TEXT_MUTED: u32 = 0xFF777773;
/// One step below `SURFACE_RAISED`, so a control that cannot act reads as
/// pushed in rather than merely tinted.
const DISABLED_FILL: u32 = 0xFFE6E5E0;
/// The disabled ink, one step below `TEXT_MUTED`.
const DISABLED_INK: u32 = 0xFFA9A8A3;
const ACCENT_SKY: u32 = 0xFF2563EB;
const SUCCESS_EMERALD: u32 = 0xFF2E8B57;
const DANGER_CRIMSON: u32 = 0xFFDC2626;
#[allow(dead_code)]
const WARNING_AMBER: u32 = 0xFFF59E0B;

// -----------------------------------------------------------------------------
// Canonical InterOS Typography
// -----------------------------------------------------------------------------

/// Anti-aliased run draw, clipped to the run's own line box.
///
/// `bg` is the local fill the text actually sits on — a tab capsule, the SOS
/// disc, the keypad disc, the dock plate — not the page background. Blending
/// every glyph edge toward one hardcoded colour tinted the AA fringe of any run
/// drawn on a darker local fill.
#[inline(always)]
fn test_text(
    x: usize,
    y: usize,
    x0: usize,
    y0: usize,
    scale: usize,
    text: &[u8],
    color: u32,
    bg: u32,
) -> Option<u32> {
    if scale == 0 || text.is_empty() {
        return None;
    }
    // A run owns exactly `run_rows(scale)` rows; testing only that box keeps a
    // call site from slicing the glyphs with a shorter hand-written window.
    if x < x0 || y < y0 || y >= y0 + run_rows(scale) {
        return None;
    }
    // The outline face, chosen ONCE for the run rather than once per pixel.
    //
    // WHY the order matters here: this funnel is called for every pixel of every
    // label on the surface, so a per-pixel choice would draw the glyph from both
    // faces at once -- an outline edge with a hard bitmap ghost one pixel off,
    // on every string. The launcher's first version of this change did exactly
    // that, and no test caught it; only looking at the capture did.
    if let Some(run) = outline_text::OutlineRun::place_run(
        x0,
        y0 + std::design::font::BASELINE * scale,
        scale,
        text,
        false,
    ) {
        // WHY the window is the run's and not the cell's: at the em that matches
        // this scale the outline's ascent reaches 18px above the cell origin and
        // its descent 4px past the bottom, so the cell-shaped test above would
        // clip the top of tall glyphs with nothing looking wrong. The run's own
        // bounds admit exactly its ink, so widening to them cannot put a pixel
        // where a glyph is not.
        let (rx, rtop, rw, rh) = run.bounds();
        if x >= rx && x - rx < rw && y >= rtop && y - rtop < rh {
            return run.pixel(x, y, color, bg);
        }
        // Inside the cell window but outside the run's ink: the background shows
        // through, exactly as an uninked bitmap pixel would.
        return None;
    }
    std::design::font::test_text_aa(x, y, x0, y0, scale, text, color, bg, false)
}

/// Rows a `scale`-high text run occupies on the shared bitmap face.
/// WHY a local `const fn`: this is exactly `font::line_height(scale)`, but that
/// helper is not `const`, and every run box below is derived from a
/// compile-time constant.
const fn run_rows(scale: usize) -> usize {
    scale * std::design::font::GLYPH_H
}

/// Header band height: y >= HEADER_H draws the hairline rule, so a header run
/// has to fit inside 0..HEADER_H.
const HEADER_H: usize = 94;
const TITLE_SCALE: usize = 3;

// Segmented navigation tabs (y: 105..155). Each capsule is sized to its own
// label plus 2 * TAB_PAD, because the shared face is proportional: a fixed
// 150px window truncated "Recents" (152px) and "Contacts" (170px).
const TAB_SCALE: usize = 2;
const TAB_PAD: usize = 16;
const TAB_BAND_Y0: usize = 105;
const TAB_BAND_H: usize = 50;
const TAB1_X0: usize = 48;
const TAB1_X1: usize = TAB1_X0 + 70 + 2 * TAB_PAD; // "Dial" measures 70px at scale 2
const TAB2_X0: usize = TAB1_X1 + 18;
const TAB2_X1: usize = TAB2_X0 + 152 + 2 * TAB_PAD; // "Recents" measures 152px
const TAB3_X0: usize = TAB2_X1 + 18;
const TAB3_X1: usize = TAB3_X0 + 170 + 2 * TAB_PAD; // "Contacts" measures 170px
const TAB_ACTIVE_FILL: u32 = 0xFF111214;
const TAB_LABELS: [&[u8]; 3] = [b"Dial", b"Recents", b"Contacts"];

// Dialed-number display card.
const DISPLAY_X0: usize = 24;
const DISPLAY_X1: usize = 696;
const DISPLAY_Y0: usize = 170;
const DISPLAY_Y1: usize = 380;
const DISPLAY_W: usize = DISPLAY_X1 - DISPLAY_X0;
const PLACEHOLDER_SCALE: usize = 2;
const PLACEHOLDER_Y0: usize = 250;
/// Real call-outcome caption under the dialed number. A scale-1 run owns
/// run_rows(1) = 24 rows, so 344..368 clears the 96-row scale-4 digit box at
/// 240..336 and stays inside the card's 380-row bottom edge.
const OUTCOME_Y0: usize = 344;

// Backspace control in the display card. Hit-tested against exactly the disc
// that is drawn (the old hit box was a 75x70 rectangle around a 48 px circle).
const BACKSPACE_CX: usize = 627;
const BACKSPACE_CY: usize = 260;
const BACKSPACE_R: i32 = 24;

// Keypad digit scale: scale 2 against the reference's ~34px caps.
const KEY_DIGIT_SCALE: usize = 2;
const KEY_SUB_SCALE: usize = 1;

// SOS emergency disc.
const SOS_CX: usize = 134;
const SOS_CY: usize = 1065;
const SOS_R: i32 = 40;
const SOS_SCALE: usize = 2;
const SOS_FILL: u32 = 0xFF2A1518;

// Central call control.
const CALL_CX: usize = 360;
const CALL_CY: usize = 1065;
const CALL_R: i32 = 52;

// Contacts shortcut control. The platform exposes no contact store, so this is
// drawn recessed and says so on tap.
const CONTACTS_CX: usize = 586;
const CONTACTS_CY: usize = 1065;
const CONTACTS_R: i32 = 40;

// Action bar band (the three discs above live in it).
const ACTION_Y0: usize = 990;
const ACTION_Y1: usize = 1140;

// Recents ledger geometry. WHY it starts at 424 rather than 410: the first row
// needs room for a scale-2 run (48 rows) plus a scale-1 run (24 rows) inside an
// 96 px row, and 410 would push the last of five rows past the action bar.
const RECENTS_Y0: usize = 424;
const RECENTS_ROW_H: usize = 96;
const CONTENT_Y0: usize = 410;
const CONTENT_Y1: usize = 1140;

// Bottom navigation dock.
const DOCK_Y0: usize = 1260;
const DOCK_Y1: usize = 1380;
const DOCK_ITEM_W: usize = 140;
/// WHY these x positions: they are the four 140 px slots the previous renderer
/// hard-coded its labels against, kept so the dock does not move.
const DOCK_ITEM_X0: [usize; 4] = [48, 208, 368, 528];
const DOCK_LABELS: [&[u8]; 4] = [b"Favorites", b"Recents", b"Keypad", b"Contacts"];
const DOCK_LABEL_Y0: usize = 1340;
const DOCK_LABEL_SCALE: usize = 1;
/// Icon band inside the dock plate: a scale-1 label below, a 36 px glyph above.
const DOCK_ICON_Y0: usize = 1290;
const DOCK_ICON_H: usize = 36;

// Keypad layout constants
const BTN_W: usize = 180;
const BTN_H: usize = 124;
const COL_X: [usize; 3] = [48, 270, 492];
const ROW_Y: [usize; 4] = [410, 560, 710, 860];

fn get_keypad_char(row: usize, col: usize) -> Option<char> {
    match (row, col) {
        (0, 0) => Some('1'),
        (0, 1) => Some('2'),
        (0, 2) => Some('3'),
        (1, 0) => Some('4'),
        (1, 1) => Some('5'),
        (1, 2) => Some('6'),
        (2, 0) => Some('7'),
        (2, 1) => Some('8'),
        (2, 2) => Some('9'),
        (3, 0) => Some('*'),
        (3, 1) => Some('0'),
        (3, 2) => Some('#'),
        _ => None,
    }
}

fn get_keypad_sublabel(row: usize, col: usize) -> &'static [u8] {
    match (row, col) {
        (0, 1) => b"ABC",
        (0, 2) => b"DEF",
        (1, 0) => b"GHI",
        (1, 1) => b"JKL",
        (1, 2) => b"MNO",
        (2, 0) => b"PQRS",
        (2, 1) => b"TUV",
        (2, 2) => b"WXYZ",
        (3, 1) => b"+",
        _ => b"",
    }
}

// -----------------------------------------------------------------------------
// Telemetry parsing. Both helpers read a real JSON field out of a real syscall
// payload; neither guesses a default.
// -----------------------------------------------------------------------------

/// Reads `"<key>":<digits>` out of a JSON payload.
///
/// WHY: the modem HAL emits a real `"bars"` field and the fuel gauge emits a
/// real `"soc"` field. Parsing them is the difference between a signal meter
/// that reflects the radio and one that reflects a boolean.
fn json_u8(json: &[u8], key: &str) -> Option<u8> {
    let pat = key.as_bytes();
    let mut i = 0usize;
    while i + pat.len() < json.len() {
        if &json[i..i + pat.len()] == pat
            && json[i + pat.len()] == b':'
            && i + pat.len() > 0
            && json[i + pat.len() - 1] == b'"'
        {
            let mut j = i + pat.len() + 1;
            let mut v: u32 = 0;
            let mut any = false;
            while j < json.len() && json[j].is_ascii_digit() {
                v = v.saturating_mul(10).saturating_add((json[j] - b'0') as u32);
                any = true;
                j += 1;
            }
            if any && v <= 255 {
                return Some(v as u8);
            }
        }
        i += 1;
    }
    None
}

/// True when the payload contains the exact literal `"present":true`.
fn json_present_true(json: &[u8]) -> bool {
    let pat = b"\"present\":true";
    if json.len() < pat.len() {
        return false;
    }
    let mut i = 0usize;
    while i + pat.len() <= json.len() {
        if &json[i..i + pat.len()] == pat {
            return true;
        }
        i += 1;
    }
    false
}

// -----------------------------------------------------------------------------
// Pixel Color Resolution Engine
// -----------------------------------------------------------------------------

/// Header band: back control, title, real signal bars, real battery.
fn header_pixel(x: usize, y: usize, dialer: &DialerApp) -> u32 {
    if y >= HEADER_H {
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

    // Header Title: "Phone". The run is scale 3, so it needs
    // run_rows(3) = 72 rows; the 94-row header band has room, so the line
    // box is centred in it instead of being gated to a 24-row window that
    // kept only 18 of the glyph's 57 ink rows.
    let title_rows = run_rows(TITLE_SCALE);
    let title_y0 = (HEADER_H - title_rows) / 2;
    if y >= title_y0 && y < title_y0 + title_rows {
        if let Some(c) = test_text(x, y, 120, title_y0, TITLE_SCALE, b"Phone", TEXT_PRIMARY, SURFACE_DARK) {
            return c;
        }
    }

    // Cellular signal indicator (x: 610..670, y: 40..62)
    //
    // WHY the bar count is real: the previous renderer filled all four bars from
    // the `modem_present` boolean, so a present-but-idle radio drew a full
    // signal. `signal_bars` is the `"bars"` field of the modem telemetry.
    if x >= 610 && x < 670 && y >= 40 && y < 62 {
        let bar_idx = (x - 610) / 15;
        let bar_x = (x - 610) % 15;
        let bar_min_y = 62 - (bar_idx + 1) * 5;
        if bar_x < 9 && y >= bar_min_y {
            return if (bar_idx as u8) < dialer.signal_bars {
                ACCENT_SKY
            } else {
                BORDER_SUBTLE
            };
        }
        return SURFACE_DARK;
    }

    // Battery indicator (x: 678..706, y: 44..58)
    //
    // WHY it is drawn only when the fuel gauge answered: this app holds no
    // power-control capability, so `battery_query` is refused and there is no
    // state of charge to draw. An always-drawn empty shell read as "battery at
    // 0%", which is a hardware claim this app cannot make. The browser takes the
    // same approach for telemetry it does not receive: draw nothing.
    if x >= 678 && x < 706 && y >= 44 && y < 58 {
        if let Some(soc) = dialer.battery_soc {
            let pct = soc.min(100) as usize;
            let inner_x0 = 681usize;
            let inner_x1 = 701usize;
            let inner_w = inner_x1 - inner_x0;
            let fill_w = inner_w * pct / 100;
            // Outline.
            if x == 678 || x == 702 || y == 44 || y == 57 || (x >= 703 && y >= 48 && y < 54) {
                return TEXT_MUTED;
            }
            // Real fill from the reported state of charge.
            if x >= inner_x0 && x < inner_x0 + fill_w && y >= 46 && y < 56 {
                return TEXT_PRIMARY;
            }
        }
        return SURFACE_DARK;
    }

    SURFACE_DARK
}

/// Segmented tabs. `Contacts` is drawn in the recessed treatment because there
/// is no contact store behind it.
fn tabs_pixel(x: usize, y: usize, dialer: &DialerApp) -> u32 {
    if !(TAB_BAND_Y0..TAB_BAND_Y0 + TAB_BAND_H).contains(&y) {
        return BG_DARK;
    }
    let label_y0 = TAB_BAND_Y0 + (TAB_BAND_H - run_rows(TAB_SCALE)) / 2;
    let ranges = [(TAB1_X0, TAB1_X1), (TAB2_X0, TAB2_X1), (TAB3_X0, TAB3_X1)];
    for (i, (x0, x1)) in ranges.iter().enumerate() {
        if x < *x0 || x >= *x1 {
            continue;
        }
        let w = x1 - x0;
        // Tab 2 is the only section the contacts tab could ever have opened, and
        // no contact store exists, so it never takes the active treatment.
        let active = match dialer.section {
            DialerSection::Keypad => i == 0,
            DialerSection::Recents => i == 1,
        };
        let (fill, ink) = if active {
            (TAB_ACTIVE_FILL, 0xFFFFFFFF)
        } else if i == 2 {
            (DISABLED_FILL, DISABLED_INK)
        } else {
            (SURFACE_RAISED, TEXT_MUTED)
        };
        // WHY a pill and not the old square fill: the design draws segmented
        // tabs as capsules. The text is only asked inside the pill so a glyph
        // near the label's ends cannot ink the curved cut-outs.
        let pill = std::design::primitives::Rect::new(*x0, TAB_BAND_Y0, w, TAB_BAND_H);
        if std::design::primitives::draw_pill(x, y, pill, fill, None).is_none() {
            continue;
        }
        let label_x0 = *x0 + std::design::font::center_x(TAB_LABELS[i], TAB_SCALE, w);
        if let Some(c) = test_text(x, y, label_x0, label_y0, TAB_SCALE, TAB_LABELS[i], ink, fill) {
            return c;
        }
        return fill;
    }
    BG_DARK
}

/// Dialed-number card: the number, the placeholder, the backspace disc and the
/// real outcome caption.
fn display_pixel(x: usize, y: usize, dialer: &DialerApp) -> u32 {
    if !(DISPLAY_Y0..DISPLAY_Y1).contains(&y) || x < DISPLAY_X0 || x >= DISPLAY_X1 {
        return BG_DARK;
    }

    if dialer.active_len > 0 {
        let scale = if dialer.active_len <= 10 { 4 } else { 3 };
        let y0 = 240;
        // Real proportional advances; digits are tabular but a fixed
        // 8px-per-character cell is not the face's advance.
        if y >= y0 && y < y0 + run_rows(scale) {
            let digits = &dialer.active_number[..dialer.active_len];
            let mut buf = [0u8; 32];
            let n = std::design::font::ellipsize(digits, scale, DISPLAY_W - 64, &mut buf);
            let total_w = std::design::font::measure(&buf[..n], scale);
            let x0 = DISPLAY_X0 + (DISPLAY_W.saturating_sub(total_w)) / 2;
            if let Some(c) = test_text(x, y, x0, y0, scale, &buf[..n], TEXT_PRIMARY, BG_DARK) {
                return c;
            }
        }
    } else if y >= PLACEHOLDER_Y0 && y < PLACEHOLDER_Y0 + run_rows(PLACEHOLDER_SCALE) {
        // Placeholder: scale 2 needs run_rows(2) = 48 rows, not the 16 the
        // old gate admitted, and the card centres the label on its measured
        // width instead of a hand-typed x0 that parked it ~95px right of
        // centre.
        let x0 = DISPLAY_X0
            + std::design::font::center_x(
                b"Enter a phone number",
                PLACEHOLDER_SCALE,
                DISPLAY_W,
            );
        if let Some(c) = test_text(
            x,
            y,
            x0,
            PLACEHOLDER_Y0,
            PLACEHOLDER_SCALE,
            b"Enter a phone number",
            TEXT_MUTED,
            BG_DARK,
        ) {
            return c;
        }
    }

    // Real outcome of the last call attempt.
    let caption = dialer.outcome.caption();
    if !caption.is_empty() {
        let mut buf = [0u8; 48];
        let n = std::design::font::ellipsize(caption, 1, DISPLAY_W - 32, &mut buf);
        if n > 0 {
            let w = std::design::font::measure(&buf[..n], 1);
            let cx = DISPLAY_X0 + (DISPLAY_W.saturating_sub(w)) / 2;
            if let Some(c) = test_text(x, y, cx, OUTCOME_Y0, 1, &buf[..n], TEXT_MUTED, BG_DARK) {
                return c;
            }
        }
    }

    // Backspace disc, drawn only when there is something to delete.
    if dialer.active_len > 0 {
        let dx = (x as i32) - BACKSPACE_CX as i32;
        let dy = (y as i32) - BACKSPACE_CY as i32;
        let d2 = dx * dx + dy * dy;
        if d2 <= BACKSPACE_R * BACKSPACE_R {
            if d2 >= (BACKSPACE_R - 3) * (BACKSPACE_R - 3) {
                return DANGER_CRIMSON;
            }
            // Backspace icon 'X' inside
            if dx.abs() <= 7 && dy.abs() <= 7 && (dx == dy || dx == -dy) {
                return DANGER_CRIMSON;
            }
            return 0xFFFEE2E2;
        }
    }

    BG_DARK
}

/// The 3x4 dial pad. Only drawn in the Keypad section.
///
/// WHY it takes no state: the keys are identical whether or not a digit is
/// already in the entry, so typing never changes a pixel in this region. That is
/// exactly why the display band is the only band a keystroke repaints.
///
/// WHY bare digits and no discs: the design draws the pad as unadorned type.
/// The digit is scale 2 against the reference's ~34px caps; the sublabel stays
/// scale 1, the smallest legible step. Hit-testing still uses the 180x124
/// cells below, so the targets do not move with the chrome.
fn keypad_pixel(x: usize, y: usize) -> u32 {
    for row in 0..4 {
        let by = ROW_Y[row];
        if y >= by && y < by + BTN_H {
            for col in 0..3 {
                let bx = COL_X[col];
                if x >= bx && x < bx + BTN_W {
                    let cx = (bx + BTN_W / 2) as i32;
                    let cy = (by + BTN_H / 2) as i32;

                    // The two runs are stacked, not intersecting: the scale-2
                    // digit box (48 rows) sits on top and the scale-1 sublabel
                    // box (24 rows) directly beneath it, the 72-row stack
                    // centred on the cell. The old boxes were cy-24..cy+48
                    // and cy+12..cy+36, which put the whole sublabel ink
                    // band inside the digit's.
                    let digit_rows = run_rows(KEY_DIGIT_SCALE);
                    let sub_rows = run_rows(KEY_SUB_SCALE);
                    let stack_y0 =
                        (cy as usize).saturating_sub((digit_rows + sub_rows) / 2);

                    // Digit text
                    if let Some(digit) = get_keypad_char(row, col) {
                        let digit_byte = [digit as u8];
                        let digit_x0 = (cx as usize).saturating_sub(
                            std::design::font::measure(&digit_byte, KEY_DIGIT_SCALE) / 2,
                        );
                        if let Some(c) = test_text(
                            x,
                            y,
                            digit_x0,
                            stack_y0,
                            KEY_DIGIT_SCALE,
                            &digit_byte,
                            TEXT_PRIMARY,
                            BG_DARK,
                        ) {
                            return c;
                        }
                    }

                    // Sublabel text
                    let sublabel = get_keypad_sublabel(row, col);
                    if !sublabel.is_empty() {
                        let sub_w = std::design::font::measure(sublabel, KEY_SUB_SCALE);
                        let sub_x = (cx as usize).saturating_sub(sub_w / 2);
                        if let Some(c) = test_text(
                            x,
                            y,
                            sub_x,
                            stack_y0 + digit_rows,
                            KEY_SUB_SCALE,
                            sublabel,
                            TEXT_MUTED,
                            BG_DARK,
                        ) {
                            return c;
                        }
                    }

                    return BG_DARK;
                }
            }
        }
    }
    BG_DARK
}

/// Recents ledger, or the honest empty state when nothing has been dialled.
fn recents_pixel(x: usize, y: usize, dialer: &DialerApp) -> u32 {
    if dialer.recents_len == 0 {
        // Headline: a scale-2 run owns 48 rows (620..668).
        let head: &[u8] = b"No calls yet";
        let head_x = DISPLAY_X0 + std::design::font::center_x(head, 2, DISPLAY_W);
        if let Some(c) = test_text(x, y, head_x, 620, 2, head, TEXT_PRIMARY, BG_DARK) {
            return c;
        }
        // Body: a scale-1 run owns 24 rows (680..704).
        let body: &[u8] = b"Every dial attempt and its real outcome is listed here";
        let mut buf = [0u8; 64];
        let n = std::design::font::ellipsize(body, 1, DISPLAY_W, &mut buf);
        if n > 0 {
            let w = std::design::font::measure(&buf[..n], 1);
            let bx = DISPLAY_X0 + (DISPLAY_W.saturating_sub(w)) / 2;
            if let Some(c) = test_text(x, y, bx, 680, 1, &buf[..n], TEXT_MUTED, BG_DARK) {
                return c;
            }
        }
        return BG_DARK;
    }

    for i in 0..dialer.recents_len {
        let row_y = RECENTS_Y0 + i * RECENTS_ROW_H;
        if y < row_y || y >= row_y + RECENTS_ROW_H {
            continue;
        }
        let rec = &dialer.recents[i];
        // Text first, plate second: returning the plate colour before the runs
        // were tested would paint every row's number out of existence.
        // Number, scale 2: run_rows(2) = 48 rows at row_y + 8.
        let mut nbuf = [0u8; 32];
        let nn = std::design::font::ellipsize(&rec.number[..rec.len], 2, DISPLAY_W - 96, &mut nbuf);
        let nx = DISPLAY_X0 + 24;
        if let Some(c) = test_text(
            x,
            y,
            nx,
            row_y + 8,
            2,
            &nbuf[..nn],
            TEXT_PRIMARY,
            SURFACE_DARK,
        ) {
            return c;
        }
        // Outcome, scale 1: run_rows(1) = 24 rows at row_y + 56.
        let out: &[u8] = match (rec.emergency, rec.accepted) {
            (true, true) => b"Emergency - accepted by telephony",
            (true, false) => b"Emergency - refused by telephony",
            (false, true) => b"Accepted by telephony",
            (false, false) => b"Refused by telephony",
        };
        let ox = DISPLAY_X0 + 24;
        if let Some(c) = test_text(x, y, ox, row_y + 56, 1, out, TEXT_MUTED, SURFACE_DARK) {
            return c;
        }
        // Row plate and divider.
        if x >= DISPLAY_X0 && x < DISPLAY_X1 {
            if y == row_y + RECENTS_ROW_H - 1 {
                return BORDER_SUBTLE;
            }
            if rec.emergency {
                return 0xFFF3E9EC;
            }
            return SURFACE_DARK;
        }
    }
    BG_DARK
}

/// Emergency / call / contacts row. Only drawn in the Keypad section.
fn action_pixel(x: usize, y: usize, dialer: &DialerApp) -> u32 {
    if !(ACTION_Y0..ACTION_Y1).contains(&y) {
        return BG_DARK;
    }

    // SOS Button (centre 134, 1065, r 40)
    let s_dx = (x as i32) - SOS_CX as i32;
    let s_dy = (y as i32) - SOS_CY as i32;
    let s_d2 = s_dx * s_dx + s_dy * s_dy;
    if s_d2 <= SOS_R * SOS_R {
        if s_d2 >= 1444 {
            return 0xFFE11D48;
        }
        // "SOS" is scale 2, so it needs run_rows(2) = 48 rows and its
        // measured 76px width must fit the 80px disc. Both are centred on
        // the disc: the old x0 = 122 ran 20px past the disc's right edge.
        let sos_rows = run_rows(SOS_SCALE);
        let sos_x0 =
            SOS_CX.saturating_sub(std::design::font::measure(b"SOS", SOS_SCALE) / 2);
        let sos_y0 = SOS_CY.saturating_sub(sos_rows / 2);
        if y >= sos_y0 && y < sos_y0 + sos_rows {
            if let Some(c) = test_text(
                x,
                y,
                sos_x0,
                sos_y0,
                SOS_SCALE,
                b"SOS",
                0xFFF43F5E,
                SOS_FILL,
            ) {
                return c;
            }
        }
        return SOS_FILL;
    }

    // Call / Hangup Central Button (centre 360, 1065, r 52)
    let c_dx = (x as i32) - CALL_CX as i32;
    let c_dy = (y as i32) - CALL_CY as i32;
    if c_dx * c_dx + c_dy * c_dy <= CALL_R * CALL_R {
        let in_call = dialer.state != CallState::Idle;
        // WHY the recessed treatment: with an empty number field there is
        // nothing to dial, so `dial("")` returned -22 and the control did
        // nothing at all while still looking live.
        let can_call = in_call || dialer.active_len > 0;
        let bg_color = if in_call {
            DANGER_CRIMSON
        } else if can_call {
            SUCCESS_EMERALD
        } else {
            DISABLED_FILL
        };
        let glyph = if can_call { 0xFFFFFFFF } else { DISABLED_INK };

        // Vector Phone Handset
        if !in_call {
            // Angled handset (green button)
            let n1 = (c_dx + 12) * (c_dx + 12) + (c_dy + 12) * (c_dy + 12);
            let n2 = (c_dx - 12) * (c_dx - 12) + (c_dy - 12) * (c_dy - 12);
            if n1 <= 36 || n2 <= 36 {
                return glyph;
            }
            let diff = c_dx - c_dy;
            if diff.abs() <= 5 && c_dx >= -12 && c_dx <= 12 && c_dy >= -12 && c_dy <= 12 {
                return glyph;
            }
        } else {
            // Horizontal hangup handset (red button)
            let n1 = (c_dx + 14) * (c_dx + 14) + (c_dy - 2) * (c_dy - 2);
            let n2 = (c_dx - 14) * (c_dx - 14) + (c_dy - 2) * (c_dy - 2);
            if n1 <= 36 || n2 <= 36 {
                return glyph;
            }
            if c_dy >= -8 && c_dy <= -2 && c_dx >= -14 && c_dx <= 14 {
                return glyph;
            }
        }

        return bg_color;
    }

    // Contacts shortcut (centre 586, 1065, r 40)
    //
    // WHY recessed: no contact store is exposed, so there is nothing behind this
    // control. It used to look live, emit a haptic click and do nothing at all.
    let ct_dx = (x as i32) - CONTACTS_CX as i32;
    let ct_dy = (y as i32) - CONTACTS_CY as i32;
    if ct_dx * ct_dx + ct_dy * ct_dy <= CONTACTS_R * CONTACTS_R {
        if ct_dx * ct_dx + ct_dy * ct_dy >= 1444 {
            return BORDER_SUBTLE;
        }
        // Person silhouette
        if ct_dx * ct_dx + (ct_dy + 8) * (ct_dy + 8) <= 49 {
            return DISABLED_INK;
        }
        if ct_dy >= 2 && ct_dy <= 14 && ct_dx.abs() <= (ct_dy - 2) + 6 {
            return DISABLED_INK;
        }
        return DISABLED_FILL;
    }

    BG_DARK
}

/// Bottom navigation dock. Every slot now either drives the section or is drawn
/// in the recessed, unavailable treatment.
fn dock_pixel(x: usize, y: usize, dialer: &DialerApp) -> u32 {
    if !(DOCK_Y0..DOCK_Y1).contains(&y) || x < 24 || x >= 696 {
        return BG_DARK;
    }
    if y == DOCK_Y0 || y == DOCK_Y1 - 1 || x == 24 || x == 695 {
        return BORDER_SUBTLE;
    }

    // WHY this mapping: slots 1 (Recents) and 2 (Keypad) are the two sections
    // that exist. Slots 0 (Favorites) and 3 (Contacts) have no backing store in
    // the platform, so they are drawn recessed and report that when tapped.
    // The old renderer hard-coded slot 2 as the active tab and drew all four
    // labels as if every slot were live.
    let mut slot = 4usize;
    for (i, x0) in DOCK_ITEM_X0.iter().enumerate() {
        if x >= *x0 && x < *x0 + DOCK_ITEM_W {
            slot = i;
            break;
        }
    }
    if slot == 4 {
        return SURFACE_DARK;
    }
    let live = slot == 1 || slot == 2;
    let active = match (slot, dialer.section) {
        (1, DialerSection::Recents) | (2, DialerSection::Keypad) => true,
        _ => false,
    };

    if live && active && y >= 1275 && y < 1365 {
        if y == 1275 || y == 1364 || x == DOCK_ITEM_X0[slot] || x == DOCK_ITEM_X0[slot] + DOCK_ITEM_W - 1
        {
            return ACCENT_SKY;
        }
    }

    let ink = if !live {
        DISABLED_INK
    } else if active {
        ACCENT_SKY
    } else {
        TEXT_MUTED
    };
    // WHY the plate never changes tone: in this light theme the dock plate is
    // already a step below the page, and the selection is carried by the ink and
    // the accent outline above. A filled slot would fight the label legibility
    // the earlier dark-surface version relied on.
    let plate = SURFACE_DARK;

    // Icon band. A scale-1 label needs run_rows(1) = 24 rows; the dock plate runs
    // to y = 1379, so a box starting at 1340 ends at 1364 and clears both the
    // icon band above and the plate border below. The old 8-row gate left
    // 5-row slivers of every label.
    let label_x = DOCK_ITEM_X0[slot]
        + std::design::font::center_x(DOCK_LABELS[slot], DOCK_LABEL_SCALE, DOCK_ITEM_W);
    if y >= DOCK_LABEL_Y0 && y < DOCK_LABEL_Y0 + run_rows(DOCK_LABEL_SCALE) {
        if let Some(c) = test_text(
            x,
            y,
            label_x,
            DOCK_LABEL_Y0,
            DOCK_LABEL_SCALE,
            DOCK_LABELS[slot],
            ink,
            plate,
        ) {
            return c;
        }
    }

    // Icons are drawn only for the live slots: there is no icon to show for a
    // store that does not exist.
    if live && y >= DOCK_ICON_Y0 && y < DOCK_ICON_Y0 + DOCK_ICON_H {
        let cx = DOCK_ITEM_X0[slot] + DOCK_ITEM_W / 2;
        let cy = DOCK_ICON_Y0 + DOCK_ICON_H / 2;
        if slot == 2 {
            // Keypad icon: a 3x3 dot grid.
            let dot_dx = (x as i32) - cx as i32;
            let dot_dy = (y as i32) - cy as i32;
            if dot_dx.abs() <= 10 && dot_dy.abs() <= 10 {
                if (dot_dx % 8).abs() <= 1 && (dot_dy % 8).abs() <= 1 {
                    return ink;
                }
            }
        } else {
            // Recents icon: a clock face with two hands.
            let d2 = (x as i32 - cx as i32).pow(2) + (y as i32 - cy as i32).pow(2);
            if (144..=169).contains(&d2) {
                return ink;
            }
            if y == cy && x >= cx - 7 && x <= cx {
                return ink;
            }
            if x == cx && y >= cy - 9 && y <= cy + 1 {
                return ink;
            }
        }
    }

    plate
}

#[inline(never)]
fn get_dialer_pixel(x: usize, y: usize, dialer: &DialerApp) -> u32 {
    // 1. Status bar & header (y: 0..94)
    if y < HEADER_H + 1 {
        return header_pixel(x, y, dialer);
    }

    // 2. Segmented navigation tabs (y: 105..155)
    if (TAB_BAND_Y0..TAB_BAND_Y0 + TAB_BAND_H).contains(&y) {
        return tabs_pixel(x, y, dialer);
    }

    // 3. Call status & dialed number display (y: 170..380)
    if (DISPLAY_Y0..DISPLAY_Y1).contains(&y) {
        return display_pixel(x, y, dialer);
    }

    // 4. Section content (y: 410..1140): the dial pad and the call controls in
    //    the Keypad section, the real call ledger in the Recents section.
    if (CONTENT_Y0..CONTENT_Y1).contains(&y) {
        return match dialer.section {
            DialerSection::Keypad => {
                let c = keypad_pixel(x, y);
                if c != BG_DARK {
                    return c;
                }
                action_pixel(x, y, dialer)
            }
            DialerSection::Recents => recents_pixel(x, y, dialer),
        };
    }

    // 5. Bottom navigation dock (y: 1260..1380)
    if (DOCK_Y0..DOCK_Y1).contains(&y) {
        return dock_pixel(x, y, dialer);
    }

    // 6. Home Gesture Pill (x: 260..460, y: 1418..1424)
    if y >= 1418 && y < 1424 && x >= 260 && x < 460 {
        return TEXT_MUTED;
    }

    BG_DARK
}

fn render_dialer_ui(surface: u32, dialer: &DialerApp) {
    render_dialer_band(surface, dialer, 0, H);
}

/// Re-rasterises and writes only rows `y0..y1`.
///
/// WHY a band and not a rectangle: a row of the surface is one contiguous run in
/// the surface buffer, so `offset = y0 * W` writes a band with no stride
/// bookkeeping. Every band starts on a row boundary, so the running `x`/`y`
/// cursor below is exact.
///
/// WHY this is correct rather than a shortcut: the band is chosen from what the
/// keystroke actually changed. Typing a digit moves the number and nothing else
/// - between 170 and 380 of the 1440 rows - and the keypad discs beside it are
/// identical before and after, so they are left exactly as drawn.
fn render_dialer_band(surface: u32, dialer: &DialerApp, y0: usize, y1: usize) {
    let mut chunk = [0u32; CHUNK];
    let mut offset = y0 * W;
    let end = y1.min(H) * W;
    while offset < end {
        let count = (end - offset).min(CHUNK);
        let mut x = offset % W;
        let mut y = offset / W;
        for i in 0..count {
            chunk[i] = get_dialer_pixel(x, y, dialer);
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
// Input handling. Every region below is drawn at the exact same coordinates by
// get_dialer_pixel (shared constants), so each control is hit-testable where it
// is rendered.
// -----------------------------------------------------------------------------

/// Which part of the surface an action changed.
///
/// WHY this exists: rendering the dialer re-rasterised all 1,036,800 pixels
/// through `get_dialer_pixel` for every touch event and every keystroke, so
/// typing a number cost the same as a full screen transition.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Dirty {
    /// Nothing changed; do not render.
    None,
    /// Only the dialed number / outcome line / backspace disc moved.
    Display,
    /// Section, call state or telemetry changed: the whole surface.
    All,
}

/// Merges two dirty regions reported in the same input batch.
///
/// WHY this exists rather than "last one wins": one `input_event_poll` drain can
/// carry two keypresses, one of which reaches the display card and one of which
/// steps the section. Keeping only one would leave the other stale.
fn merge(a: Dirty, b: Dirty) -> Dirty {
    if a == Dirty::None {
        return b;
    }
    if b == Dirty::None || a == b {
        return a;
    }
    Dirty::All
}

enum Tap<'a> {
    None,
    Redraw(Dirty),
    Leave(&'a str),
}

fn in_circle(x: usize, y: usize, cx: usize, cy: usize, r: i32) -> bool {
    let dx = (x as i32) - cx as i32;
    let dy = (y as i32) - cy as i32;
    dx * dx + dy * dy <= r * r
}

/// Places the entered number, or hangs up an active call.
fn toggle_call(dialer: &mut DialerApp) -> Dirty {
    if dialer.state != CallState::Idle {
        dialer.hangup();
        return Dirty::All;
    }
    if dialer.active_len == 0 {
        // Unreachable from a tap: the control is drawn recessed while the entry
        // is empty. Reachable from the keyboard, so it still reports honestly.
        std::log(
            0,
            "[DIALER] Call requested with no number entered; no telephony request was issued.",
        )
        .ok();
        return Dirty::None;
    }
    let mut num_buf = [0u8; 32];
    let len = dialer.active_len;
    num_buf[..len].copy_from_slice(&dialer.active_number[..len]);
    let Ok(target) = core::str::from_utf8(&num_buf[..len]) else {
        std::log(0, "[DIALER] Invalid dial string.").ok();
        return Dirty::None;
    };
    if dialer.dial(target).is_ok() {
        std::log(0, "[DIALER] Telephony service accepted the call.").ok();
    } else {
        std::log(0, "[DIALER] Call could not be established.").ok();
    }
    Dirty::All
}

fn handle_tap(dialer: &mut DialerApp, px: u32, py: u32) -> Tap<'static> {
    let x = px as usize;
    let y = py as usize;

    // 1. Back control in the header, or the home gesture bar: exit to launcher.
    if (x >= 24 && x < 100 && y >= 28 && y < 68) || (x >= 240 && x <= 480 && y >= 1400) {
        return Tap::Leave("[DIALER APP] Back control or home gesture pressed. Exiting to launcher.");
    }

    // 2. Segmented tabs. Slot 2 (Contacts) has no store behind it.
    if (TAB_BAND_Y0..TAB_BAND_Y0 + TAB_BAND_H).contains(&y) {
        if x >= TAB1_X0 && x < TAB1_X1 {
            dialer.section = DialerSection::Keypad;
            let _ = std::audio_haptic_click();
            return Tap::Redraw(Dirty::All);
        }
        if x >= TAB2_X0 && x < TAB2_X1 {
            dialer.section = DialerSection::Recents;
            let _ = std::audio_haptic_click();
            return Tap::Redraw(Dirty::All);
        }
        if x >= TAB3_X0 && x < TAB3_X1 {
            std::log(
                0,
                "[DIALER APP] Contacts tab tapped: the platform exposes no contact store, so there is no contacts screen to open.",
            )
            .ok();
            return Tap::Redraw(Dirty::None);
        }
    }

    // 3. Bottom dock. Slots 1 and 2 select a section; 0 and 3 report honestly.
    if (DOCK_Y0..DOCK_Y1).contains(&y) && x >= 24 && x < 696 {
        for slot in 0..4 {
            let x0 = DOCK_ITEM_X0[slot];
            if x < x0 || x >= x0 + DOCK_ITEM_W {
                continue;
            }
            match slot {
                1 => {
                    dialer.section = DialerSection::Recents;
                    let _ = std::audio_haptic_click();
                    return Tap::Redraw(Dirty::All);
                }
                2 => {
                    dialer.section = DialerSection::Keypad;
                    let _ = std::audio_haptic_click();
                    return Tap::Redraw(Dirty::All);
                }
                _ => {
                    std::log(
                        0,
                        "[DIALER APP] Dock slot tapped: no favorites or contacts store exists on this platform, so the slot is unavailable.",
                    )
                    .ok();
                    return Tap::Redraw(Dirty::None);
                }
            }
        }
    }

    // 4. Backspace disc: hit-tested against exactly the drawn circle.
    if dialer.active_len > 0 && in_circle(x, y, BACKSPACE_CX, BACKSPACE_CY, BACKSPACE_R) {
        dialer.backspace();
        return Tap::Redraw(Dirty::Display);
    }

    // 5. Keypad and action row are only live in the Keypad section; in Recents
    //    the same rows belong to the call ledger.
    if dialer.section == DialerSection::Keypad {
        // Keypad Matrix
        for row in 0..4 {
            let by = ROW_Y[row];
            if y >= by && y < by + BTN_H {
                for col in 0..3 {
                    let bx = COL_X[col];
                    if x >= bx && x < bx + BTN_W {
                        if let Some(digit) = get_keypad_char(row, col) {
                            dialer.input_digit(digit);
                            std::log(0, "[DIALER APP] Keypad digit entered.").ok();
                        }
                        return Tap::Redraw(Dirty::Display);
                    }
                }
            }
        }

        // Action Bar: three discs, each hit-tested against its drawn circle.
        if (ACTION_Y0..ACTION_Y1).contains(&y) {
            if in_circle(x, y, SOS_CX, SOS_CY, SOS_R) {
                let _ = dialer.dial_emergency();
                if dialer.outcome == CallOutcome::EmergencyRefused {
                    std::log(
                        0,
                        "[DIALER APP] Emergency call refused by the telephony service; the outcome is shown under the number.",
                    )
                    .ok();
                } else {
                    std::log(
                        0,
                        "[DIALER APP] Telephony service accepted the emergency call.",
                    )
                    .ok();
                }
                return Tap::Redraw(Dirty::All);
            }
            if in_circle(x, y, CALL_CX, CALL_CY, CALL_R) {
                return Tap::Redraw(toggle_call(dialer));
            }
            if in_circle(x, y, CONTACTS_CX, CONTACTS_CY, CONTACTS_R) {
                std::log(
                    0,
                    "[DIALER APP] Contacts shortcut tapped: no contact store is exposed by this platform.",
                )
                .ok();
                return Tap::Redraw(Dirty::None);
            }
        }
    }

    Tap::None
}

/// Mirrors `handle_browser_key` in `apps/browser/src/main.rs`: printable ASCII,
/// backspace, escape, enter and tab routed through the dialer's own state.
fn handle_dialer_key(dialer: &mut DialerApp, key: std::InputKeyEvent) -> Tap<'static> {
    use std::input_key_code;
    if !key.pressed {
        return Tap::None;
    }
    match key.code {
        input_key_code::BACKSPACE => {
            dialer.backspace();
            Tap::Redraw(Dirty::Display)
        }
        input_key_code::ESCAPE => {
            dialer.clear_entry();
            Tap::Redraw(Dirty::Display)
        }
        input_key_code::ENTER => Tap::Redraw(toggle_call(dialer)),
        input_key_code::TAB => {
            dialer.step_section();
            std::log(0, "[DIALER APP] Section switched by keyboard.").ok();
            Tap::Redraw(Dirty::All)
        }
        // Dial pad keys only. The keypad's own sublabel letters (ABC, DEF, ...)
        // are not part of the number entry, so accepting them would let the
        // display hold something no telephony service could be asked to dial.
        // `key.code` is a u32, so the literals are widened to match rather than
        // being cast down.
        code @ (48u32..=57u32) | code @ (42u32 | 35u32) => {
            dialer.input_digit(code as u8 as char);
            Tap::Redraw(Dirty::Display)
        }
        _ => Tap::None,
    }
}

#[no_mangle]
pub extern "C" fn _start() -> ! {
    std::log(
        0,
        "[DIALER APP (Ring 3)] Sovereign Dialer & Telephony online.",
    )
    .ok();

    let mut dialer = DialerApp::new();

    // 1. Query Cellular Modem Baseband Telemetry
    // WHY: this app holds no telephony authority, so the query is normally
    // refused. The previous code logged nothing on refusal, leaving an unlogged
    // absence. Report the real outcome and fail closed on modem presence.
    let mut modem_buf = [0u8; 128];
    match std::telephony_query(&mut modem_buf) {
        Ok(len) if len > 0 => {
            let payload = &modem_buf[..len];
            if json_present_true(payload) {
                dialer.modem_present = true;
                std::log(
                    0,
                    "[DIALER APP] Baseband Modem: Hardware detected and online.",
                )
                .ok();
            } else {
                // WHY: the payload simply did not list a present modem.
                // "Offline" would assert a baseband state the HAL never
                // reported, so state only what the report actually said.
                std::log(
                    0,
                    "[DIALER APP] Baseband Modem: the telemetry report did not list a present modem.",
                )
                .ok();
            }
            // WHY parse the real field: the signal meter used to fill all four
            // bars from the `present` boolean, which is a full-strength reading
            // for a radio that may be idle. `"bars"` is what the HAL measured.
            match json_u8(payload, "\"bars\"") {
                Some(bars) => {
                    dialer.signal_bars = if bars > 4 { 4 } else { bars };
                }
                None => {
                    dialer.signal_bars = 0;
                    std::log(
                        0,
                        "[DIALER APP] Modem telemetry carried no signal bar count; the meter shows no bars.",
                    )
                    .ok();
                }
            }
        }
        Ok(_) => {
            std::log(
                0,
                "[DIALER APP] Telephony query returned an empty payload; no baseband reported.",
            )
            .ok();
        }
        Err(_) => {
            std::log(
                0,
                "[DIALER APP] Telephony query refused by the kernel; no cellular baseband is available to this app.",
            )
            .ok();
        }
    }

    // 1b. Battery fuel gauge. WHY query it at all: the header used to draw an
    // empty battery shell on every launch, which reads as "0%". Either the real
    // state of charge is drawn or nothing is drawn there.
    let mut battery_buf = [0u8; 256];
    match std::battery_query(&mut battery_buf) {
        Ok(len) if len > 0 => {
            let payload = &battery_buf[..len];
            if json_present_true(payload) {
                match json_u8(payload, "\"soc\"") {
                    Some(soc) => {
                        dialer.battery_soc = Some(if soc > 100 { 100 } else { soc });
                    }
                    None => {
                        dialer.battery_soc = None;
                        std::log(
                            0,
                            "[DIALER APP] Fuel gauge reported no state of charge; the battery indicator is left blank.",
                        )
                        .ok();
                    }
                }
            } else {
                std::log(
                    0,
                    "[DIALER APP] No battery fuel gauge is present; the battery indicator is left blank.",
                )
                .ok();
            }
        }
        Ok(_) => {
            std::log(
                0,
                "[DIALER APP] Battery query returned an empty payload; the battery indicator is left blank.",
            )
            .ok();
        }
        Err(_) => {
            std::log(
                0,
                "[DIALER APP] Battery query refused by the kernel; the battery indicator is left blank.",
            )
            .ok();
        }
    }

    // Emergency contacts are provided by the real contacts/modem subsystem when available.

    // 3. Allocate dedicated KMS Client Surface (z=200)
    std::log(
        0,
        "[DIALER APP] Step 1: Allocating application surface (z=200)...",
    )
    .ok();
    let surface = match std::surface_create(6, W as u32, H as u32, 200) {
        Ok(s) => {
            std::log(0, "[DIALER APP] Step 2: Surface created successfully.").ok();
            s
        }
        Err(_) => {
            std::log(
                0,
                "[DIALER APP] Display surface unavailable; refusing headless call simulation.",
            )
            .ok();
            std::exit_with_code(1);
        }
    };

    // 4. Render an empty dialer; numbers are entered by the user.
    render_dialer_ui(surface, &dialer);
    let _ = std::surface_flip(6);
    std::log(
        0,
        "[DIALER APP] Step 3: Initial frame presented to hardware scanout.",
    )
    .ok();

    // 5. Calls are initiated only from user input; no automatic call is placed at startup.

    // 6. Interactive Event Reactor Loop
    loop {
        if let Ok(events) = std::event_poll(
            std::EVENT_TOUCH_INPUT | std::EVENT_TIMER_TICK | std::EVENT_MODEM_URC,
            100,
        ) {
            if (events & std::EVENT_TIMER_TICK) != 0 {
                if dialer.state == CallState::ActiveCall {
                    dialer.call_duration_secs += 1;
                }
                // WHY no flip here: nothing on the surface is derived from the
                // call timer, so re-presenting an unchanged frame costs a full
                // scanout copy for no visible difference.
            }

            if (events & std::EVENT_MODEM_URC) != 0 {
                // WHY: the previous code logged an inbound tower alert and called
                // it a call for any MODEM_URC bit. The URC vector was never read,
                // no call setup message was decoded, and no baseband is attached,
                // so no inbound call occurred. Report the event only.
                if dialer.modem_present {
                    std::log(
                        0,
                        "[DIALER APP] Baseband URC signalled; no call setup message was decoded from it.",
                    )
                    .ok();
                } else {
                    std::log(
                        0,
                        "[DIALER APP] Modem URC event raised with no attached baseband; no call was received.",
                    )
                    .ok();
                }
            }

            if (events & std::EVENT_TOUCH_INPUT) != 0 {
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
                    // Keyboard first: the normalized pipeline multiplexes keys
                    // and touches through the same poll, so a key must be
                    // recognised before the touch branch sees kind != 1.
                    if let Some(key) = input.key_event() {
                        match handle_dialer_key(&mut dialer, key) {
                            Tap::Leave(msg) => {
                                std::log(0, msg).ok();
                                let _ = std::surface_destroy(6, surface);
                                std::exit_with_code(0);
                            }
                            Tap::Redraw(d) => dirty = merge(dirty, d),
                            Tap::None => {}
                        }
                        continue;
                    }
                    if input.kind == 4 || input.kind == 6 || input.kind == 7 || input.kind == 8 || input.kind == 10 {
                        std::log(
                            0,
                            "[DIALER APP] Navigation gesture recognized. Returning to launcher.",
                        )
                        .ok();
                        let _ = std::surface_destroy(6, surface);
                        std::exit_with_code(0);
                    }
                    if input.kind == 1 {
                        match handle_tap(&mut dialer, input.x, input.y) {
                            Tap::Leave(msg) => {
                                std::log(0, msg).ok();
                                let _ = std::surface_destroy(6, surface);
                                std::exit_with_code(0);
                            }
                            Tap::Redraw(d) => dirty = merge(dirty, d),
                            Tap::None => {}
                        }
                    }
                }
                if dirty != Dirty::None {
                    match dirty {
                        Dirty::None => {}
                        Dirty::Display => render_dialer_band(surface, &dialer, DISPLAY_Y0, DISPLAY_Y1),
                        Dirty::All => render_dialer_ui(surface, &dialer),
                    }
                    let _ = std::surface_flip(6);
                }
            }
        }

        std::yield_now();
    }
}

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    std::log(0, "[DIALER APP] Panic detected! Terminating task...").ok();
    std::exit_with_code(1);
}
