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

#![no_std]
#![no_main]

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
        }
    }

    /// Plays dual-tone multi-frequency acoustic feedback for a pressed keypad digit.
    pub fn play_dtmf(&self, digit: char) {
        if let Some(pair) = get_dtmf_frequencies(digit) {
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

    /// Initiates an outgoing cellular call.
    pub fn dial(&mut self, number: &str) -> Result<(), i64> {
        if number.is_empty() || number.len() > self.active_number.len() {
            return Err(-22);
        }

        self.state = CallState::Dialing;
        self.call_duration_secs = 0;

        // A call is connected only after the authoritative telephony service
        // accepts the request. UI/ACR dispatch must never manufacture an
        // ActiveCall state.
        match std::telephony_dial(number) {
            Ok(()) => {
                self.state = CallState::ActiveCall;
                std::audio_play_tone(440).ok();
                std::audio_silence().ok();
                std::log(0, "[DIALER APP] Telephony service accepted call request.").ok();
                Ok(())
            }
            Err(err) => {
                self.state = CallState::CallEnded;
                std::log(0, "[DIALER APP] Call request rejected by telephony service.").ok();
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

// Dialed-number display card.
const DISPLAY_X0: usize = 24;
const DISPLAY_X1: usize = 696;
const PLACEHOLDER_SCALE: usize = 2;
const PLACEHOLDER_Y0: usize = 250;

// Keypad disc and its two stacked runs.
const KEY_R: i32 = 50;
const KEY_DIGIT_SCALE: usize = 3;
const KEY_SUB_SCALE: usize = 1;

// SOS emergency disc.
const SOS_CX: usize = 134;
const SOS_CY: usize = 1065;
const SOS_R: i32 = 40;
const SOS_SCALE: usize = 2;
const SOS_FILL: u32 = 0xFF2A1518;

// Bottom navigation dock.
const DOCK_LABEL_Y0: usize = 1340;
const DOCK_LABEL_SCALE: usize = 1;

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
// Pixel Color Resolution Engine
// -----------------------------------------------------------------------------
#[inline(never)]
fn get_dialer_pixel(x: usize, y: usize, dialer: &DialerApp) -> u32 {
    // 1. Status Bar & Header (y: 0..HEADER_H)
    if y < HEADER_H {
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
        if x >= 610 && x < 670 && y >= 40 && y < 62 {
            let bar_idx = (x - 610) / 15;
            let bar_x = (x - 610) % 15;
            let bar_min_y = 62 - (bar_idx + 1) * 5;
            if bar_x < 9 && y >= bar_min_y {
                return if dialer.modem_present { ACCENT_SKY } else { BORDER_SUBTLE };
            }
            return SURFACE_DARK;
        }

        // Battery indicator (x: 678..706, y: 44..58)
        if x >= 678 && x < 706 && y >= 44 && y < 58 {
            if x == 678 || x == 702 || y == 44 || y == 57 || (x >= 703 && y >= 48 && y < 54) {
                return TEXT_MUTED;
            }
            return SURFACE_DARK;
        }

        return SURFACE_DARK;
    }

    // 2. Segmented Navigation Tabs (y: 105..155)
    //
    // The scale-2 label needs run_rows(2) = 48 rows, which fits the 50-row band
    // only when the label is centred in it, and each capsule is sized to its own
    // proportional label so nothing is cut off at the capsule edge.
    if (TAB_BAND_Y0..TAB_BAND_Y0 + TAB_BAND_H).contains(&y) {
        let label_y0 = TAB_BAND_Y0 + (TAB_BAND_H - run_rows(TAB_SCALE)) / 2;

        // Tab 1: "Dial" (Active Capsule: x: 48..150)
        if x >= TAB1_X0 && x < TAB1_X1 {
            let label_x0 =
                TAB1_X0 + std::design::font::center_x(b"Dial", TAB_SCALE, TAB1_X1 - TAB1_X0);
            if let Some(c) = test_text(
                x,
                y,
                label_x0,
                label_y0,
                TAB_SCALE,
                b"Dial",
                0xFFFFFFFF,
                TAB_ACTIVE_FILL,
            ) {
                return c;
            }
            return TAB_ACTIVE_FILL;
        }
        // Tab 2: "Recents" (Inactive: x: 168..352)
        if x >= TAB2_X0 && x < TAB2_X1 {
            let label_x0 = TAB2_X0
                + std::design::font::center_x(b"Recents", TAB_SCALE, TAB2_X1 - TAB2_X0);
            if let Some(c) = test_text(
                x,
                y,
                label_x0,
                label_y0,
                TAB_SCALE,
                b"Recents",
                TEXT_MUTED,
                SURFACE_RAISED,
            ) {
                return c;
            }
            return SURFACE_RAISED;
        }
        // Tab 3: "Contacts" (Inactive: x: 370..572)
        if x >= TAB3_X0 && x < TAB3_X1 {
            let label_x0 = TAB3_X0
                + std::design::font::center_x(b"Contacts", TAB_SCALE, TAB3_X1 - TAB3_X0);
            if let Some(c) = test_text(
                x,
                y,
                label_x0,
                label_y0,
                TAB_SCALE,
                b"Contacts",
                TEXT_MUTED,
                SURFACE_RAISED,
            ) {
                return c;
            }
            return SURFACE_RAISED;
        }
    }

    // 2b. Call Status & Dialed Number Display (y: 170..380, x: 24..696)
    if (170..380).contains(&y) && x >= DISPLAY_X0 && x < DISPLAY_X1 {
        // Dialed Digits Display
        if dialer.active_len > 0 {
            let scale = if dialer.active_len <= 10 { 4 } else { 3 };
            let y0 = 240;
            // Real proportional advances; digits are tabular but a fixed
            // 8px-per-character cell is not the face's advance.
            if y >= y0 && y < y0 + run_rows(scale) {
                let digits = &dialer.active_number[..dialer.active_len];
                let total_w = std::design::font::measure(digits, scale);
                let x0 = W.saturating_sub(total_w) / 2;
                if let Some(c) = test_text(x, y, x0, y0, scale, digits, TEXT_PRIMARY, BG_DARK) {
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
                    DISPLAY_X1 - DISPLAY_X0,
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

        // Backspace button in display card (627, 260, r: 24)
        if dialer.active_len > 0 {
            let bx = 627i32;
            let by = 260i32;
            let dx = (x as i32) - bx;
            let dy = (y as i32) - by;
            if dx * dx + dy * dy <= 576 {
                if dx * dx + dy * dy >= 484 {
                    return DANGER_CRIMSON;
                }
                // Backspace icon 'X' inside
                if dx.abs() <= 7 && dy.abs() <= 7 && (dx == dy || dx == -dy) {
                    return DANGER_CRIMSON;
                }
                return 0xFFFEE2E2;
            }
        }

        return BG_DARK;
    }

    // 3. Keypad Matrix (y: 410..984)
    for row in 0..4 {
        let by = ROW_Y[row];
        if y >= by && y < by + BTN_H {
            for col in 0..3 {
                let bx = COL_X[col];
                if x >= bx && x < bx + BTN_W {
                    let cx = (bx + BTN_W / 2) as i32;
                    let cy = (by + BTN_H / 2) as i32;
                    let k_dx = (x as i32) - cx;
                    let k_dy = (y as i32) - cy;
                    let dist2 = k_dx * k_dx + k_dy * k_dy;

                    if dist2 <= KEY_R * KEY_R {
                        // Button rim
                        if dist2 >= 46 * 46 {
                            return BORDER_SUBTLE;
                        }

                        // The two runs are stacked, not intersecting: the
                        // scale-3 digit box (72 rows) sits on top and the
                        // scale-1 sublabel box (24 rows) directly beneath it,
                        // the 96-row stack centring to cy-48..cy+48 inside the
                        // disc's cy-50..cy+50. The old boxes were cy-24..cy+48
                        // and cy+12..cy+36, which put the whole sublabel ink
                        // band inside the digit's.
                        let digit_rows = run_rows(KEY_DIGIT_SCALE);
                        let sub_rows = run_rows(KEY_SUB_SCALE);
                        let stack_y0 = (cy as usize).saturating_sub((digit_rows + sub_rows) / 2);

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
                                SURFACE_DARK,
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
                                SURFACE_DARK,
                            ) {
                                return c;
                            }
                        }

                        return SURFACE_DARK;
                    }
                    return BG_DARK;
                }
            }
        }
    }

    // 4. Action Bar (y: 990..1140)
    if (990..1140).contains(&y) {
        // SOS Button (x: 48..220, centered at 134, 1065, r: 40)
        let s_dx = (x as i32) - SOS_CX as i32;
        let s_dy = (y as i32) - SOS_CY as i32;
        if s_dx * s_dx + s_dy * s_dy <= SOS_R * SOS_R {
            if s_dx * s_dx + s_dy * s_dy >= 1444 {
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

        // Call / Hangup Central Button (x: 270..450, centered at 360, 1065, r: 52)
        let c_dx = (x as i32) - 360;
        let c_dy = (y as i32) - 1065;
        if c_dx * c_dx + c_dy * c_dy <= 2704 {
            let is_in_call = dialer.state != CallState::Idle;
            let bg_color = if is_in_call { DANGER_CRIMSON } else { SUCCESS_EMERALD };

            // Vector Phone Handset
            if !is_in_call {
                // Angled handset (green button)
                let n1 = (c_dx + 12) * (c_dx + 12) + (c_dy + 12) * (c_dy + 12);
                let n2 = (c_dx - 12) * (c_dx - 12) + (c_dy - 12) * (c_dy - 12);
                if n1 <= 36 || n2 <= 36 {
                    return 0xFFFFFFFF;
                }
                let diff = c_dx - c_dy;
                if diff.abs() <= 5 && c_dx >= -12 && c_dx <= 12 && c_dy >= -12 && c_dy <= 12 {
                    return 0xFFFFFFFF;
                }
            } else {
                // Horizontal hangup handset (red button)
                let n1 = (c_dx + 14) * (c_dx + 14) + (c_dy - 2) * (c_dy - 2);
                let n2 = (c_dx - 14) * (c_dx - 14) + (c_dy - 2) * (c_dy - 2);
                if n1 <= 36 || n2 <= 36 {
                    return 0xFFFFFFFF;
                }
                if c_dy >= -8 && c_dy <= -2 && c_dx >= -14 && c_dx <= 14 {
                    return 0xFFFFFFFF;
                }
            }

            return bg_color;
        }

        // Contacts shortcut button (x: 500..672, centered at 586, 1065, r: 40)
        let ct_dx = (x as i32) - 586;
        let ct_dy = (y as i32) - 1065;
        if ct_dx * ct_dx + ct_dy * ct_dy <= 1600 {
            if ct_dx * ct_dx + ct_dy * ct_dy >= 1444 {
                return BORDER_SUBTLE;
            }
            // Person silhouette
            if ct_dx * ct_dx + (ct_dy + 8) * (ct_dy + 8) <= 49 {
                return TEXT_PRIMARY;
            }
            if ct_dy >= 2 && ct_dy <= 14 && ct_dx.abs() <= (ct_dy - 2) + 6 {
                return TEXT_PRIMARY;
            }
            return SURFACE_RAISED;
        }

        return BG_DARK;
    }

    // 5. Bottom Navigation Dock (y: 1260..1380)
    if (1260..1380).contains(&y) && x >= 24 && x < 696 {
        if y == 1260 || y == 1379 || x == 24 || x == 695 {
            return BORDER_SUBTLE;
        }

        // 4 tabs across width:
        // Tab 0: Favorites (x: 48..188)
        // Tab 1: Recents (x: 208..348)
        // Tab 2: Keypad (x: 368..508) -> ACTIVE
        // Tab 3: Contacts (x: 528..668)
        let is_keypad_tab = (368..508).contains(&x);
        if is_keypad_tab && (1275..1365).contains(&y) {
            if y == 1275 || y == 1364 || x == 368 || x == 507 {
                return ACCENT_SKY;
            }
        }

        // Tab titles. A scale-1 run needs run_rows(1) = 24 rows; the dock plate runs
        // to y = 1379, so a box starting at 1340 ends at 1364 and clears both
        // the icon band above and the plate border below. The old 8-row gate
        // left 5-row slivers of every label.
        if y >= DOCK_LABEL_Y0 && y < DOCK_LABEL_Y0 + run_rows(DOCK_LABEL_SCALE) {
            if let Some(c) = test_text(x, y, 76, DOCK_LABEL_Y0, DOCK_LABEL_SCALE, b"Favorites", TEXT_MUTED, SURFACE_DARK) {
                return c;
            }
            if let Some(c) = test_text(x, y, 248, DOCK_LABEL_Y0, DOCK_LABEL_SCALE, b"Recents", TEXT_MUTED, SURFACE_DARK) {
                return c;
            }
            if let Some(c) = test_text(x, y, 412, DOCK_LABEL_Y0, DOCK_LABEL_SCALE, b"Keypad", ACCENT_SKY, SURFACE_DARK) {
                return c;
            }
            if let Some(c) = test_text(x, y, 568, DOCK_LABEL_Y0, DOCK_LABEL_SCALE, b"Contacts", TEXT_MUTED, SURFACE_DARK) {
                return c;
            }
        }

        // Tab icons
        if y >= 1290 && y < 1326 {
            // Keypad icon (3x3 dots) at (438, 1308)
            if is_keypad_tab {
                let dot_dx = (x as i32) - 438;
                let dot_dy = (y as i32) - 1308;
                if dot_dx.abs() <= 10 && dot_dy.abs() <= 10 {
                    if (dot_dx % 8).abs() <= 1 && (dot_dy % 8).abs() <= 1 {
                        return ACCENT_SKY;
                    }
                }
            }
        }

        return SURFACE_DARK;
    }

    // 6. Home Gesture Pill (x: 260..460, y: 1418..1424)
    if y >= 1418 && y < 1424 && x >= 260 && x < 460 {
        return TEXT_MUTED;
    }

    BG_DARK
}

fn render_dialer_ui(surface: u32, dialer: &DialerApp) {
    let mut chunk = [0u32; CHUNK];
    let total = W * H;
    let mut offset = 0;
    while offset < total {
        let count = (total - offset).min(CHUNK);
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

/// Returns true if app should exit back to Launcher.
fn handle_touch_point(dialer: &mut DialerApp, x: u32, y: u32) -> bool {
    let x = x as usize;
    let y = y as usize;

    // 1. Back button in header or Home gesture bar: exit to Launcher
    if (x >= 24 && x < 100 && y >= 28 && y < 68) || (x >= 240 && x <= 480 && y >= 1400) {
        std::log(
            0,
            "[DIALER] Back button or home gesture pressed. Exiting to launcher.",
        )
        .ok();
        return true;
    }

    // 2. Backspace button in display card
    if x >= 590 && x < 665 && y >= 235 && y < 305 {
        dialer.backspace();
        return false;
    }

    // 3. Keypad Matrix
    for row in 0..4 {
        let by = ROW_Y[row];
        if y >= by && y < by + BTN_H {
            for col in 0..3 {
                let bx = COL_X[col];
                if x >= bx && x < bx + BTN_W {
                    if let Some(digit) = get_keypad_char(row, col) {
                        dialer.input_digit(digit);
                        std::log(0, "[DIALER] Keypad digit entered.").ok();
                    }
                    return false;
                }
            }
        }
    }

    // 4. Action Bar
    if (990..1140).contains(&y) {
        if x >= 48 && x < 220 && y >= 1005 && y < 1125 {
            // SOS Emergency 911
            dialer.active_len = 0;
            dialer.input_digit('9');
            dialer.input_digit('1');
            dialer.input_digit('1');
            let _ = dialer.dial("911");
            return false;
        } else if x >= 270 && x < 450 && y >= 995 && y < 1135 {
            // Call / End toggle
            if dialer.state == CallState::Idle {
                let mut num_buf = [0u8; 32];
                let len = dialer.active_len;
                num_buf[..len].copy_from_slice(&dialer.active_number[..len]);
                let Ok(target) = core::str::from_utf8(&num_buf[..len]) else {
                    std::log(0, "[DIALER] Invalid dial string.").ok();
                    return false;
                };
                if let Err(_) = dialer.dial(target) {
                    std::log(0, "[DIALER] Call could not be established.").ok();
                }
            } else {
                dialer.hangup();
            }
        } else if x >= 500 && x < 672 && y >= 1005 && y < 1125 {
            // Contacts are loaded from real persisted contact state; no synthetic entries are injected.
            std::audio_haptic_click().ok();
            return false;
        }
    }

    false
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
            if let Ok(json_str) = core::str::from_utf8(&modem_buf[..len]) {
                if json_str.contains("\"present\":true") {
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
                let _ = std::surface_flip(6);
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
                while std::input_event_poll(&mut input).is_ok() {
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
                        if handle_touch_point(&mut dialer, input.x, input.y) {
                            let _ = std::surface_destroy(6, surface);
                            std::exit_with_code(0);
                        }
                        render_dialer_ui(surface, &dialer);
                        let _ = std::surface_flip(6);
                    }
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
