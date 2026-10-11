//! Quick Controls: the control centre.
//!
//! WHY this surface is a list of readings and not a grid of switches: every
//! switch that used to be here was drawn with no sensor and no syscall behind
//! it. The previous version painted four round buttons (torch, rotation, moon,
//! power) whose only behaviour was to display the literal string "N/A", and a
//! 2x2 connectivity grid whose "Airplane" cell printed "Unavailable" as a
//! compile-time constant without anything having been asked. A switch that does
//! nothing is worse than a switch that is absent, because it teaches the user
//! that this panel's controls lie.
//!
//! So the rule this file is built on is: **a row exists only where a reading
//! exists, and a control is drawn only where the shell's tap handler already
//! draws one.** Everything else is a row whose value cell names the thing that
//! is missing -- `no torch hardware`, `no radio controller` -- in the same
//! idiom `live_widgets` uses for its absent rows.
//!
//! Two controls survive, and they are the two `main.rs` can actually act on:
//! the volume capsule and the Power button. Their rectangles are derived from
//! the same constants the handler hit-tests, so the drawn control and its hit
//! region cannot drift apart.

#![allow(dead_code)]

use core::cell::UnsafeCell;

use interos_std as std;
use launcher::layout::Rect;
use launcher::text;
use launcher::theme::{
    radius, space, type_scale, Theme, SCREEN_HEIGHT, SCREEN_WIDTH, STATUS_BAR_HEIGHT,
};

use crate::screens::system_space::{counted, decimal, push, round_contains, Reading, VALUE_MAX};
use crate::LauncherState;

// -----------------------------------------------------------------------------
// Palette
// -----------------------------------------------------------------------------

/// WHY `Theme::DARK`: this surface is the dark control centre in the reference
/// sheet, and the previous version carried seven local colour constants
/// (`BG_COLOR` 0xFF111214, `TILE_BG` 0xFF25282A, `SLIDER_BG` 0xFF35393D,
/// `SLIDER_FILL` 0xFFFFFFFF, a red that is not in the theme, ...) that had each
/// drifted a few steps from the theme's own dark values. Reading the theme means
/// the page, the cards, the disabled wells, the ink and the accents are the same
/// values every other dark surface in the shell already uses.
const THEME: Theme = Theme::DARK;

/// Ink of a filled level bar.
///
/// WHY this is the theme's `text` and not a new colour: a level bar at maximum
/// has to be the brightest thing in its capsule or it does not read as "full",
/// and in the dark theme `text` is already the brightest ink available.
const LEVEL_FILL: u32 = Theme::DARK.text;

// -----------------------------------------------------------------------------
// Panel geometry
// -----------------------------------------------------------------------------

/// Left edge of every full-width block.
///
/// WHY 36 and not the theme's `CONTENT_PADDING` (40): `main.rs`'s Quick Controls
/// handler hit-tests the volume capsule with `(36..684).contains(&x)`, so a
/// control drawn outside that span would have a hit region that did not match
/// it. This number is pinned by the handler rather than chosen for looks, and
/// every block below is derived from it, so the drawn control and the hit region
/// are the same rectangle by construction rather than by agreement.
const PANEL_X: usize = 36;

/// Width of a full-width block: exactly the handler's `36..684` span.
const PANEL_W: usize = SCREEN_WIDTH - PANEL_X * 2; // 648

/// Icon box drawn at the left of every row.
const ICON: usize = 28;

/// Left inset of a row's text, measured from the row's own left edge: the icon
/// box plus `space::MD` on each side of it.
const ROW_TEXT_INSET: usize = space::MD + ICON + space::MD; // 60

/// Type scale for a row's label and value.
///
/// WHY `type_scale::BODY` (a 48-row cell) and not `CAPTION` (24): a 64-row row
/// carrying 24-row captions leaves 20 blank rows above and below every line, and
/// the result reads as a list of debris rather than a list of settings. The cell
/// now fills the row, which is the choice `live_widgets` already makes for its
/// reading rows.
const ROW_SCALE: usize = type_scale::BODY;

/// Type scale for a row's value - the reading, or the reason there isn't one.
///
/// WHY `CAPTION` and not `ROW_SCALE`: the value column is capped at half the row,
/// and at BODY the honest reasons do not survive that cap. "No radio control",
/// "No policy service" and "No rotation sensor" are what tell the user why a
/// control cannot act, and every one of them was being ellipsized into "No radio
/// cont..." - which states less than drawing nothing, because it looks like the
/// row was cut off rather than unavailable. The label keeps `ROW_SCALE`: the label
/// is what the setting *is*, and it is short enough to fit.
const VALUE_SCALE: usize = type_scale::CAPTION;

/// Row height: the row's own line box plus `space::SM` above and below it.
///
/// WHY 64 and not the previous 136: the two things a row has to hold are one
/// line box and its gutter. A row two and a half times taller than its contents
/// is a row that reads as empty, and four of them left the bottom half of the
/// surface as bare background.
const ROW_H: usize = text::GLYPH_H * ROW_SCALE + space::SM * 2; // 64

/// Vertical inset of a row's text line.
const ROW_PAD_Y: usize = space::SM;

/// Gutter between consecutive rows.
const ROW_GUTTER: usize = space::SM;

/// Ceiling for the computed list rows.
///
/// WHY the list computes its own row height at all: the band it is given is
/// fixed, but the number of rows is a property of what the platform can report,
/// and a fixed height would leave a dead band under the list the moment that
/// number changed. The floor is [`ROW_H`], which is what a row has to hold, so a
/// row can never be shorter than the type inside it.
const ROW_MAX: usize = space::XXL * 2;

/// Cell heights, taken from the type scale rather than retyped.
const BODY_H: usize = text::GLYPH_H * type_scale::BODY; // 48
const CAPTION_H: usize = text::GLYPH_H * type_scale::CAPTION; // 24

// ---- Heading block ---------------------------------------------------------

/// The panel's content, vertically centred in the space between the status bar
/// and the home indicator.
///
/// WHY centred and not top-anchored: the panel holds nine rows after the
/// readout rows were removed, which is ~924px of content in ~1344px of space.
/// Top-anchoring left a ~360px void at the foot while the sections above sat
/// tight together, which reads as a layout that ran out of things to say. A
/// shade pulled from the top edge does not have to start at the top edge; the
/// status bar stays where it is and the content floats centred beneath it, so
/// the top and bottom voids match and the panel reads as intentional.
///
/// WHY a single offset rather than re-derived positions: every Y below hangs
/// off `HEADING_Y`, so one addition moves the whole panel and no section can be
/// left behind. `CONTENT_H` is a sum of block *heights*, never of positions, so
/// there is no const-evaluation cycle through `HEADING_Y`.
const AVAIL_TOP: usize = STATUS_BAR_HEIGHT + space::LG;
const AVAIL_BOT: usize = LIST_BOTTOM;
const CONTENT_H: usize = (BODY_H + HEADING_GAP + CAPTION_H)
    + (space::LG + SECTION_H + space::SM)
    + (CONNECTIVITY_ROWS * ROW_H + (CONNECTIVITY_ROWS - 1) * ROW_GUTTER)
    + space::SM
    + (LEVEL_H + space::LG + LEVEL_H)
    + space::XL
    + SECTION_H
    + space::SM
    + FOOTER_H
    + space::LG
    + SECTION_H
    + space::SM
    + (SYSTEM_ROWS * ROW_H + (SYSTEM_ROWS - 1) * ROW_GUTTER);
const CONTENT_DY: usize = (AVAIL_BOT - AVAIL_TOP - CONTENT_H) / 2;
const HEADING_Y: usize = AVAIL_TOP + CONTENT_DY;
/// WHY `space::XS` and not `0`: the previous panel drew nothing between the
/// status rule and its first tile at y = 90, so the first thing a reader met was
/// a control. A heading is what says which surface this is.
const HEADING_GAP: usize = space::XS;
const SUMMARY_Y: usize = HEADING_Y + BODY_H + HEADING_GAP; // 104

// ---- Connectivity block ----------------------------------------------------

/// Every section label is one caption line, so all three are the same height.
const SECTION_H: usize = CAPTION_H;

const CONNECTIVITY_LABEL_Y: usize = SUMMARY_Y + CAPTION_H + space::LG; // 152
const CONNECTIVITY_TOP: usize = CONNECTIVITY_LABEL_Y + SECTION_H + space::SM; // 184
const CONNECTIVITY_ROWS: usize = 2;

// ---- Level block (brightness, volume) --------------------------------------

/// Height of a level control: a text line plus a track under it.
///
/// WHY 76: `main.rs` hit-tests the volume capsule with `(500..576).contains(&y)`,
/// so the drawn capsule has to be exactly 76 rows tall starting at 500 or the
/// drawing and the hit region differ by however much they disagree. The
/// brightness control is given the same height so the pair reads as one class.
const LEVEL_H: usize = 76;

/// WHY a level control's track sits under its text line and not inside it: the
/// handler maps a tap's x linearly across `36..684` (`((x - 36) * 100) / 648`),
/// so a track inset from the capsule's ends would put the fill somewhere other
/// than where the finger landed. The track therefore spans the full padded width
/// and the text sits above it, never over it.
const LEVEL_TRACK_H: usize = space::SM;
const LEVEL_TEXT_DY: usize = space::SM;
const LEVEL_TRACK_DY: usize = LEVEL_TEXT_DY + BODY_H + space::XS; // 60

/// Top of the level block.
///
/// WHY no value comment here: every Y on this surface moves with `CONTENT_DY`,
/// so a typed value would go stale the next time the row count changes - which
/// is exactly what happened to the old "// 400" when the connectivity block
/// lost a row and the real value became 328.
const LEVEL_TOP: usize = CONNECTIVITY_TOP
    + CONNECTIVITY_ROWS * ROW_H
    + (CONNECTIVITY_ROWS - 1) * ROW_GUTTER
    + space::SM;

const BRIGHTNESS_ROW: Rect = Rect::new(PANEL_X, LEVEL_TOP, PANEL_W, LEVEL_H);

/// WHY this is computed rather than typed, and why that is a risk worth writing
/// down: `LEVEL_TOP + LEVEL_H + space::LG` evaluates to exactly 500, which is
/// where `main.rs` expects the volume capsule. If the block above it ever moves,
/// this constant moves with it and the capsule silently stops matching the
/// handler's hit region -- it would still look right and still be dead. The fix
/// is to have the handler ask this module for the rectangle, the way the lock
/// screen exports its disc geometry.
/// The volume capsule's rectangle, exported so the handler cannot disagree with
/// the painter.
///
/// WHY it is now `pub` and why the assert that used to sit below it is gone: the
/// rectangle was private and pinned to y == 500 by a compile-time assert, because
/// `main.rs` typed the same rectangle in as a literal. That coupling meant a
/// layout change here broke the build rather than shipping silently - which was
/// the right failure mode - but it also meant the two could never be moved
/// together on purpose. The handler now reads this rectangle, so there is one
/// definition and nothing to keep in step.
pub const VOLUME_SLIDER: Rect = Rect::new(PANEL_X, LEVEL_TOP + LEVEL_H + space::LG, PANEL_W, LEVEL_H);

// ---- Footer block (a reading card, then the power button) -------------------

const DEVICE_LABEL_Y: usize = VOLUME_SLIDER.y + VOLUME_SLIDER.h + space::XL;

/// Top of the footer row, derived from the chain like every other block.
///
/// WHY derived and not the fixed 644 it used to be: that value existed because
/// `main.rs` typed the power window in as `(630..740)` / `(375..680)` literals,
/// so the button could never move without breaking the handler. The handler now
/// reads the exported `POWER_BTN`, so the footer sits where the layout puts it -
/// directly under its section label - and a future row-count change moves it
/// along with everything else instead of stranding a ~140px gap above it.
const FOOTER_Y: usize = DEVICE_LABEL_Y + SECTION_H + space::SM;
const FOOTER_H: usize = 72;

/// The Power button: the only control on this surface that opens another
/// surface, and the only one with a hit region outside the volume capsule.
///
/// WHY it is `pub`: the handler in `main.rs` hit-tests this rectangle, and the
/// volume capsule already proved that a literal typed in two places breaks the
/// first time the layout moves. One definition, read from both sides.
pub const POWER_BTN: Rect = Rect::new(408, FOOTER_Y, 272, FOOTER_H);

/// The reading card beside the Power button, bottom-aligned with it.
///
/// WHY its width stops at `space::XXL` short of the button and not `space::MD`:
/// the handler hit-tests `POWER_BTN` starting at x 408, so a card that reached
/// to 392 would put 17 px of itself inside the button's window -- a reading that
/// opens the power menu when tapped, which is exactly the drawn-control /
/// hit-region mismatch this file is trying to remove. `space::XXL` (48) is the
/// largest gutter on the scale that still clears the button, and it also leaves
/// the destructive action with the clearance it deserves.
const FOOTER_CARD: Rect = Rect::new(
    PANEL_X,
    FOOTER_Y + (FOOTER_H - ROW_H),
    POWER_BTN.x - space::XXL - PANEL_X,
    ROW_H,
);

/// WHY this is checked by the compiler rather than trusted: the card's right edge
/// has to stay left of the handler's Power window for the whole reason above.
const _: () = assert!(FOOTER_CARD.x + FOOTER_CARD.w < 375);

/// The power glyph's centre: the same icon-box arithmetic every row uses, so the
/// button and the list agree on where an icon sits.
const POWER_ICON_CX: usize = POWER_BTN.x + space::MD + ICON / 2;
const POWER_LABEL_X: usize = POWER_BTN.x + ROW_TEXT_INSET;

// ---- Reading list ----------------------------------------------------------

const SYSTEM_LABEL_Y: usize = FOOTER_Y + FOOTER_H + space::LG;
const SYSTEM_TOP: usize = SYSTEM_LABEL_Y + SECTION_H + space::SM;
const SYSTEM_ROWS: usize = 4;

/// Home indicator, drawn at the foot of the surface.
///
/// WHY the list stops short of it: the list's row height is computed from the
/// band between its own top and this pill, so a row can never be taller than the
/// type inside it and can never run off the bottom of the screen.
const INDICATOR_W: usize = space::XL * 5;
const INDICATOR_H: usize = space::XS;

const LIST_BOTTOM: usize = SCREEN_HEIGHT - space::MD - INDICATOR_H - space::XS;
const LIST_BOUNDS: Rect = Rect::new(PANEL_X, SYSTEM_TOP, PANEL_W, LIST_BOTTOM - SYSTEM_TOP);

// -----------------------------------------------------------------------------
// What this platform can actually report
// -----------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq)]
enum PanelRow {
    Wifi,
    Bluetooth,
    Cellular,
    Brightness,
    Volume,
    Torch,
    RotationLock,
    DoNotDisturb,
    Airplane,
    Sensors,
    Battery,
    Lock,
    Storage,
    Packages,
    Notifications,
}

impl PanelRow {
    fn icon(self) -> QcIcon {
        match self {
            PanelRow::Wifi => QcIcon::Wifi,
            PanelRow::Bluetooth => QcIcon::Bluetooth,
            PanelRow::Cellular => QcIcon::Cellular,
            PanelRow::Brightness => QcIcon::Sun,
            PanelRow::Volume => QcIcon::Speaker,
            PanelRow::Torch => QcIcon::Torch,
            PanelRow::RotationLock => QcIcon::Rotate,
            PanelRow::DoNotDisturb => QcIcon::Moon,
            PanelRow::Airplane => QcIcon::Airplane,
            PanelRow::Sensors => QcIcon::Sensors,
            PanelRow::Battery => QcIcon::Battery,
            PanelRow::Lock => QcIcon::Lock,
            PanelRow::Storage => QcIcon::Storage,
            PanelRow::Packages => QcIcon::Package,
            PanelRow::Notifications => QcIcon::Bell,
        }
    }

    fn label(self) -> &'static [u8] {
        match self {
            PanelRow::Wifi => b"Wi-Fi",
            PanelRow::Bluetooth => b"Bluetooth",
            PanelRow::Cellular => b"Mobile data",
            PanelRow::Brightness => b"Brightness",
            PanelRow::Volume => b"Volume",
            PanelRow::Torch => b"Flashlight",
            PanelRow::RotationLock => b"Rotation lock",
            PanelRow::DoNotDisturb => b"Do not disturb",
            PanelRow::Airplane => b"Airplane mode",
            PanelRow::Sensors => b"Sensors",
            PanelRow::Battery => b"Battery",
            PanelRow::Lock => b"Screen lock",
            PanelRow::Storage => b"Storage",
            PanelRow::Packages => b"Packages",
            PanelRow::Notifications => b"Notifications",
        }
    }

    /// Whether this row is a level control, which is what gives it a track.
    fn is_level(self) -> bool {
        matches!(self, PanelRow::Brightness | PanelRow::Volume)
    }

    /// The reading, or the reason there is none.
    ///
    /// WHY availability is decided on this line and nowhere else: the one rule
    /// this surface must not break is printing a value for hardware the platform
    /// cannot produce, and the only reliable place to enforce that is the line
    /// that decides whether the value exists.
    fn reading(self, state: &LauncherState) -> Value {
        match self {
            PanelRow::Wifi => hardware("wifi"),
            PanelRow::Bluetooth => hardware("bluetooth"),
            PanelRow::Cellular => hardware("cellular"),
            PanelRow::Sensors => hardware("sensors"),
            PanelRow::Storage => hardware("storage"),
            // WHY these two name the missing controller instead of printing
            // "unavailable": the previous panel printed "Brightness unavailable
            // - no backlight" as a caption *across a filled white capsule*, and
            // "N/A" on four buttons. The value cell is where a reader looks for
            // the reason, so it says what is missing.
            PanelRow::Brightness => Value::absent(b"No backlight controller"),
            PanelRow::Volume => Value::absent(b"No audio level bound"),
            PanelRow::Torch => Value::absent(b"No torch hardware"),
            PanelRow::RotationLock => Value::absent(b"No rotation sensor"),
            PanelRow::DoNotDisturb => Value::absent(b"No policy service"),
            PanelRow::Airplane => Value::absent(b"No radio controller"),
            PanelRow::Battery => battery(),
            PanelRow::Lock => match state.lock_state {
                std::UserLockState::Unlocked => Value::of(b"Unlocked"),
                std::UserLockState::Locked => Value::of(b"Locked"),
                std::UserLockState::Lockout => Value::of(b"Locked out"),
                std::UserLockState::Uninitialized => Value::of(b"No PIN set"),
            },
            PanelRow::Packages => {
                let (buf, len) = counted(state.app_count, b" installed");
                Value::of(&buf[..len])
            }
            PanelRow::Notifications => {
                let (buf, len) = counted(state.notification_count, b" active");
                Value::of(&buf[..len])
            }
        }
    }
}

/// One reading, plus the flag that produced it.
///
/// WHY this exists beside `system_space::Reading`: that type decides availability
/// inside `of` / `unavailable` and then exposes only `as_slice` and `skin`, so a
/// caller cannot ask it whether the value is real. This surface has to know: an
/// absent row mutes its *label* as well as its value, and only a level control
/// draws a track at all. Carrying the flag is honest; inferring it from the
/// resulting colour would be a guess that breaks the moment a theme changes.
struct Value {
    reading: Reading,
    available: bool,
}

impl Value {
    fn of(run: &[u8]) -> Value {
        Value { reading: Reading::of(run), available: true }
    }

    fn absent(reason: &[u8]) -> Value {
        Value { reading: Reading::unavailable(reason), available: false }
    }

    fn bytes(&self) -> &[u8] {
        self.reading.as_slice()
    }
}

/// The kernel's own availability answer for one hardware category.
///
/// WHY only categories the kernel names explicitly:
/// `kernel/src/settings.rs::get_hardware_status` ends in `_ => Available`, so
/// every category it does not list comes back as "available" whether or not the
/// hardware exists. Asking it about `airplane` or `dnd` would print "Available"
/// for hardware the kernel has never heard of, which is the exact fabrication
/// this panel exists to avoid. Only categories with a real arm behind them are
/// asked, and the three that return `Unavailable` on x86-virt now say so.
fn hardware(category: &str) -> Value {
    match std::settings_hardware_status(category) {
        Ok(0) => Value::of(b"Available"),
        Ok(1) => Value::of(b"Simulated"),
        Ok(_) => Value::absent(b"Unavailable"),
        Err(_) => Value::absent(b"No answer"),
    }
}

/// Index of the byte after `"key":` in a flat JSON object, if the key is there.
///
/// WHY a scanner and not a parser: the payload is a single flat object the
/// kernel formats with `core::fmt` -- the same shape `refresh_clock` in `main.rs`
/// already scans for its time fields -- and a no-std JSON parser would be a
/// large addition to a screen renderer.
fn json_value(buf: &[u8], key: &[u8]) -> Option<usize> {
    let mut i = 0usize;
    while i + key.len() + 2 < buf.len() {
        if buf[i] == b'"'
            && &buf[i + 1..i + 1 + key.len()] == key
            && buf[i + 1 + key.len()] == b'"'
            && (i == 0 || buf[i - 1] == b'{' || buf[i - 1] == b',')
        {
            let after = i + key.len() + 2;
            if after < buf.len() && buf[after] == b':' {
                return Some(after + 1);
            }
        }
        i += 1;
    }
    None
}

fn json_bool(buf: &[u8], key: &[u8]) -> Option<bool> {
    let i = json_value(buf, key)?;
    if i >= buf.len() {
        return None;
    }
    match buf[i] {
        b't' if buf[i..].starts_with(b"true") => Some(true),
        b'f' if buf[i..].starts_with(b"false") => Some(false),
        _ => None,
    }
}

fn json_i64(buf: &[u8], key: &[u8]) -> Option<i64> {
    let mut i = json_value(buf, key)?;
    let negative = i < buf.len() && buf[i] == b'-';
    if negative {
        i += 1;
    }
    let start = i;
    let mut value: i64 = 0;
    while i < buf.len() && buf[i].is_ascii_digit() {
        value = (value * 10 + (buf[i] - b'0') as i64).min(1000);
        i += 1;
    }
    if i == start {
        return None;
    }
    Some(if negative { -value } else { value })
}

/// One `battery_query()` call, decoded.
///
/// WHY the syscall's own answer is not enough: the handler formats a payload for
/// every caller and leaves `present` false when no fuel gauge is registered, so
/// `Ok` means "the kernel answered", not "there is a battery". A state of charge
/// is printed only when `present` is true *and* one came with it; `Err` is
/// reported as a refusal rather than as an absent gauge, because the shell has no
/// authority to tell those two apart from inside userspace.
fn battery() -> Value {
    // WHY 256 and not 48: the handler rejects a buffer shorter than
    // `resource::BATTERY_JSON_MAX`, which is 256, with `InvalidArguments`.
    let mut buf = [0u8; 256];
    let Ok(n) = std::battery_query(&mut buf) else {
        return Value::absent(b"Query refused");
    };
    let payload = &buf[..n.min(buf.len())];
    match json_bool(payload, b"present") {
        Some(true) => match json_i64(payload, b"soc") {
            Some(soc) if (0..=100).contains(&soc) => {
                let (mut num, mut at) = decimal(soc as usize);
                push(&mut num, &mut at, b"%");
                Value::of(&num[..at])
            }
            _ => Value::absent(b"No charge reading"),
        },
        Some(false) | None => Value::absent(b"No fuel gauge"),
    }
}

// -----------------------------------------------------------------------------
// The readings this panel is allowed to make
// -----------------------------------------------------------------------------

/// WHY each list is written out rather than derived: the order is the reading
/// order, and it has to exist once, in one place, for the layout, the summary
/// count and the painter to agree about what the panel contains.
const CONNECTIVITY_LIST: [PanelRow; CONNECTIVITY_ROWS] = [PanelRow::Wifi, PanelRow::Bluetooth];

const LEVEL_LIST: [PanelRow; 2] = [PanelRow::Brightness, PanelRow::Volume];

/// The quick toggles, and only the quick toggles.
///
/// WHY these four and not the nine this block held: sensors, battery, screen
/// lock, storage and package count are *readouts*, not controls. Nothing on this
/// surface acts when they are tapped, so a row that only reports a value is a
/// settings entry that has wandered into a control centre - and on a target with
/// no torch, no rotation sensor, no policy service and no radio controller, six
/// of the nine existed only to say so. They belong in Settings, where a list of
/// values is what the user asked for. What is left is what a control centre is
/// for, and nothing else.
const SYSTEM_LIST: [PanelRow; SYSTEM_ROWS] = [
    PanelRow::Torch,
    PanelRow::DoNotDisturb,
    PanelRow::Airplane,
    PanelRow::RotationLock,
];

const FOOTER_LIST: [PanelRow; 1] = [PanelRow::Notifications];

/// Number of rows the surface shows.
///
/// WHY it is its own constant rather than `ALL_ROWS.len()` at every use site:
/// the reading table in `measure_frame` is a fixed-size array, and a length
/// written out in its type is the one place a new row has to be remembered.
const ALL_ROWS_LEN: usize = 9;

/// Every row on the surface, in reading order. Used for the headline count, so
/// it has to be the whole panel rather than one block's worth of it.
const ALL_ROWS: [PanelRow; ALL_ROWS_LEN] = [
    PanelRow::Wifi,
    PanelRow::Bluetooth,
    PanelRow::Brightness,
    PanelRow::Volume,
    PanelRow::Torch,
    PanelRow::DoNotDisturb,
    PanelRow::Airplane,
    PanelRow::RotationLock,
    PanelRow::Notifications,
];

/// WHY this is asserted rather than assumed: `item_index` looks a row up in
/// [`ALL_ROWS`] and the headline counts every row, so a row added to a block but
/// not to the table would be counted nowhere and drawn with whichever reading
/// happened to share its position.
const _: () = assert!(
    CONNECTIVITY_LIST.len() + LEVEL_LIST.len() + SYSTEM_LIST.len() + FOOTER_LIST.len()
        == ALL_ROWS_LEN
);

/// The brightness level as a percentage, or `None` when no display-power backend
/// is bound to this shell.
///
/// WHY `None` is hardcoded rather than read from the settings store: QEMU has no
/// virtual backlight device wired to the guest, and a stored preference is not
/// proof that changing brightness changes emitted pixels. The previous panel
/// printed "Brightness unavailable - no backlight" *across a filled white
/// capsule* -- an honest caption next to a track asserting 100 %. Returning
/// `None` is what stops the fill arm from running at all.
fn brightness_level() -> Option<u32> {
    None
}

/// The master volume as a percentage, or `None` when nothing measures one.
///
/// WHY `None` even though `audio_query_volume()` always answers: the answer is
/// `AudioManager::volume_percent`, and `AudioManager::new()` seeds that field with
/// the literal `100` (`kernel/src/audio/mod.rs`). Before anything touched it the
/// panel would have printed "100 %" -- a number that was never measured, and
/// exactly what the solid white bar in the old capsule was.
/// `AudioManager::set_volume` only stores the field, and `AudioManager::play`
/// dispatches to the PC speaker without reading it, so no emitted audio scales
/// with it either. The query returns a stored preference, not a level.
fn volume_level() -> Option<u32> {
    None
}

// -----------------------------------------------------------------------------
// Text runs
// -----------------------------------------------------------------------------

/// Byte capacity of one run.
///
/// WHY 32: the longest string this surface holds is the headline count, and the
/// next longest is a row value ("No backlight controller", 23 bytes). A run
/// longer than this would be measured short of its own width and drawn off the
/// end of its column, so anything longer has to be ellipsized before it arrives
/// here rather than truncated here.
const RUN_CAP: usize = 32;

#[derive(Clone, Copy)]
struct LabelRun {
    x: usize,
    y: usize,
    w: usize,
    h: usize,
    scale: usize,
    len: usize,
    ink: u32,
    bytes: [u8; RUN_CAP],
}

impl LabelRun {
    const EMPTY: LabelRun = LabelRun {
        x: 0,
        y: 0,
        w: 0,
        h: 0,
        scale: 1,
        len: 0,
        ink: 0,
        bytes: [0u8; RUN_CAP],
    };

    /// Left-aligned at `(x, y)`, truncated with an ellipsis past `fit`.
    ///
    /// WHY every run is gated on its *measured* box rather than a typed y-window:
    /// this face's cell is `text::GLYPH_H` = 24 rows per scale step, not the 8
    /// that the older hardcoded windows were sized for. A window written for an
    /// 8-row face throws away the bottom two thirds of every glyph at
    /// `type_scale::CAPTION` and half of each one at `type_scale::BODY`, which is
    /// how "Packages" and "Appearance" used to render with their descenders
    /// sliced flat. `w` and `h` here come from `text::measure` and
    /// `text::line_height`, so the box the painter tests is the box the
    /// rasteriser can actually touch.
    fn fitted(x: usize, y: usize, scale: usize, fit: usize, bytes: &[u8], ink: u32) -> Self {
        let (buf, len) = text::ellipsize(bytes, scale, fit);
        let n = len.min(RUN_CAP);
        let mut held = [0u8; RUN_CAP];
        held[..n].copy_from_slice(&buf[..n]);
        LabelRun {
            x,
            y,
            w: text::measure(&held[..n], scale),
            h: text::line_height(scale),
            scale,
            len: n,
            ink,
            bytes: held,
        }
    }

    /// Left-aligned inside `col`, vertically centred on it.
    fn left(col: Rect, scale: usize, bytes: &[u8], ink: u32) -> Self {
        let y = col.y + col.h.saturating_sub(text::line_height(scale)) / 2;
        Self::fitted(col.x, y, scale, col.w, bytes, ink)
    }

    /// Right-aligned inside `col`, vertically centred on it.
    ///
    /// WHY the right edge comes from the *measured* width and not from the
    /// caller's guess: the previous value column was a fixed width with the run
    /// centred inside it, so a value one glyph wider than the column sat half a
    /// glyph off the right edge of every row.
    fn right(col: Rect, scale: usize, bytes: &[u8], ink: u32) -> Self {
        let (buf, len) = text::ellipsize(bytes, scale, col.w);
        let n = len.min(RUN_CAP);
        let mut held = [0u8; RUN_CAP];
        held[..n].copy_from_slice(&buf[..n]);
        let w = text::measure(&held[..n], scale);
        LabelRun {
            x: (col.x + col.w).saturating_sub(w),
            y: col.y + col.h.saturating_sub(text::line_height(scale)) / 2,
            w,
            h: text::line_height(scale),
            scale,
            len: n,
            ink,
            bytes: held,
        }
    }

    /// Centred in a run `w` wide starting at `x`, with its top edge at `y`.
    fn centred(x: usize, y: usize, w: usize, scale: usize, bytes: &[u8], ink: u32) -> Self {
        let (buf, len) = text::ellipsize(bytes, scale, w);
        let n = len.min(RUN_CAP);
        let mut held = [0u8; RUN_CAP];
        held[..n].copy_from_slice(&buf[..n]);
        LabelRun {
            x: x + text::center_x(&held[..n], scale, w),
            y,
            w: text::measure(&held[..n], scale),
            h: text::line_height(scale),
            scale,
            len: n,
            ink,
            bytes: held,
        }
    }

    #[inline]
    fn pixel(&self, px: usize, py: usize) -> Option<u32> {
        if self.len == 0 {
            return None;
        }
        if px < self.x || px >= self.x + self.w || py < self.y || py >= self.y + self.h {
            return None;
        }
        crate::test_text(
            px,
            py,
            self.x,
            self.y,
            self.scale,
            &self.bytes[..self.len],
            self.ink,
        )
    }

    /// As [`Run::pixel`], anti-aliased against `bg` so a glyph over a flat panel
    /// does not show a stair-stepped outline.
    #[inline]
    fn pixel_on(&self, px: usize, py: usize, bg: u32) -> Option<u32> {
        if self.len == 0 {
            return None;
        }
        if px < self.x || px >= self.x + self.w || py < self.y || py >= self.y + self.h {
            return None;
        }
        crate::test_text_aa(
            px,
            py,
            self.x,
            self.y,
            self.scale,
            &self.bytes[..self.len],
            self.ink,
            bg,
        )
    }
}

// -----------------------------------------------------------------------------
// Frame
// -----------------------------------------------------------------------------

#[derive(Clone, Copy)]
struct RowFrame {
    rect: Rect,
    icon: QcIcon,
    fill: u32,
    border: u32,
    ink: u32,
    label: LabelRun,
    value: LabelRun,
    /// Present only on a level control: the bar a fill would be drawn into.
    track: Option<Rect>,
    /// Width of the fill as a fraction of the track. Zero when there is no
    /// reading, which is every frame on this platform.
    fill_w: usize,
}

impl RowFrame {
    const EMPTY: RowFrame = RowFrame {
        rect: Rect::new(0, 0, 0, 0),
        icon: QcIcon::Wifi,
        fill: 0,
        border: 0,
        ink: 0,
        label: LabelRun::EMPTY,
        value: LabelRun::EMPTY,
        track: None,
        fill_w: 0,
    };
}

struct QuickControlsFrame {
    clock: LabelRun,
    heading: LabelRun,
    summary: LabelRun,
    sections: [LabelRun; 3],
    connectivity: [RowFrame; CONNECTIVITY_ROWS],
    levels: [RowFrame; 2],
    system: [RowFrame; SYSTEM_ROWS],
    footer: RowFrame,
    power_label: LabelRun,
}

impl QuickControlsFrame {
    const fn empty() -> Self {
        QuickControlsFrame {
            clock: LabelRun::EMPTY,
            heading: LabelRun::EMPTY,
            summary: LabelRun::EMPTY,
            sections: [LabelRun::EMPTY; 3],
            connectivity: [RowFrame::EMPTY; CONNECTIVITY_ROWS],
            levels: [RowFrame::EMPTY; 2],
            system: [RowFrame::EMPTY; SYSTEM_ROWS],
            footer: RowFrame::EMPTY,
            power_label: LabelRun::EMPTY,
        }
    }
}

/// Lays one row out: icon, label column, value column, and -- for a level
/// control -- the track underneath.
///
/// `value` is passed in rather than read here so that a kernel query is issued
/// once per row per frame rather than once per row per row.
/// `label_scale` is `ROW_SCALE` everywhere except the narrow footer card, where
/// a full-size "Notifications" plus its value cannot fit and the label would be
/// ellipsized into "Notifi..." - which states less than drawing nothing, because
/// it looks truncated rather than unavailable.
fn build_row(
    rect: Rect,
    item: PanelRow,
    value: &Value,
    level: Option<u32>,
    label_scale: usize,
    theme: &Theme,
) -> RowFrame {
    let (fill, value_ink) = value.reading.skin(theme);
    // WHY an absent row's *label* is muted along with its value: the previous
    // panel drew every caption in the same ink whether or not anything answered
    // it, so "Flashlight" read exactly as live as the clock. Toning the whole
    // row down is what makes "this control does nothing" legible at a glance.
    let ink = if value.available { theme.text } else { theme.text_muted };

    // WHY the text box stays at the row's scale even when the label steps down:
// the footer card is still a 64-row row, and its value already renders at
// VALUE_SCALE inside this same box. The label joins the value at its size;
// the box is the row's, not the run's.
    let text_h = text::line_height(ROW_SCALE);
    let text_y0 = if item.is_level() {
        rect.y + LEVEL_TEXT_DY
    } else {
        rect.y + (rect.h - text_h) / 2
    };
    let text_col = Rect::new(
        rect.x + ROW_TEXT_INSET,
        text_y0,
        rect.w.saturating_sub(ROW_TEXT_INSET + space::MD),
        text_h,
    );

    // WHY the value column is capped at half the row: a value long enough to eat
    // the whole row would push the label off the left edge, and the label is the
    // half of the row that says what the reading *is*.
    let value_w = text::measure(value.bytes(), VALUE_SCALE)
        .min(text_col.w / 2)
        .saturating_add(space::SM);
    let value_col = Rect::new(text_col.x + text_col.w - value_w, text_col.y, value_w, text_col.h);
    let label_col = Rect::new(
        text_col.x,
        text_col.y,
        text_col.w.saturating_sub(value_w + space::SM),
        text_col.h,
    );

    let (track, fill_w) = match level {
        Some(percent) => {
            let pad = rect.inset(space::MD);
            let track = Rect::new(pad.x, rect.y + LEVEL_TRACK_DY, pad.w, LEVEL_TRACK_H);
            (Some(track), (track.w.saturating_mul(percent.min(100) as usize)) / 100)
        }
        None => (None, 0),
    };

    RowFrame {
        rect,
        icon: item.icon(),
        fill,
        border: theme.border,
        ink,
        label: LabelRun::left(label_col, label_scale, item.label(), ink),
        value: LabelRun::right(value_col, VALUE_SCALE, value.bytes(), value_ink),
        track,
        fill_w,
    }
}

/// `N of M live readings` -- the honest headline for a panel whose job is to say
/// what the device knows.
///
/// WHY a count and not a slogan: the previous panel had no headline at all, so a
/// reader had to infer from eleven separate captions that almost nothing on the
/// page was backed by hardware. One line states it.
fn reading_summary(live: usize) -> ([u8; VALUE_MAX], usize) {
    let (mut buf, mut at) = decimal(live);
    push(&mut buf, &mut at, b" of ");
    let (total, total_len) = decimal(ALL_ROWS.len());
    push(&mut buf, &mut at, &total[..total_len]);
    push(&mut buf, &mut at, b" live readings");
    (buf, at)
}

fn measure_frame(state: &LauncherState) -> QuickControlsFrame {
    let theme = THEME;

    // WHY the clock is gated on the source flag and not on the buffer: with no
    // RTC authority granted to this shell the read can be refused and the buffer
    // keeps whatever it last held. `notifications`, `home` and `system_space`
    // gate on the same flag, so a refused read leaves this status bar empty
    // rather than showing a stale value.
    let clock_bytes: &[u8] = if state.clock_source_ok && state.clock_time[0] != b' ' {
        &state.clock_time
    } else {
        b""
    };

    // WHY every reading is taken once, up front, into a table indexed by the
    // row's position in [`ALL_ROWS`]: `measure_frame` needs the count for the
    // headline and the bytes for each row, and asking the kernel for each of the
    // fifteen twice would double this frame's syscall bill for no new
    // information -- and two answers to the same question are two chances to
    // print a number the other one did not.
    let values: [Value; ALL_ROWS_LEN] = core::array::from_fn(|i| ALL_ROWS[i].reading(state));
    let live = values.iter().filter(|v| v.available).count();
    let (summary_raw, summary_len) = reading_summary(live);

    let sections = [
        LabelRun::left(
            Rect::new(PANEL_X, CONNECTIVITY_LABEL_Y, PANEL_W, SECTION_H),
            type_scale::CAPTION,
            b"Connectivity",
            theme.text_muted,
        ),
        LabelRun::left(
            Rect::new(PANEL_X, DEVICE_LABEL_Y, PANEL_W, SECTION_H),
            type_scale::CAPTION,
            b"Device and power",
            theme.text_muted,
        ),
        LabelRun::left(
            Rect::new(PANEL_X, SYSTEM_LABEL_Y, PANEL_W, SECTION_H),
            type_scale::CAPTION,
            b"Controls and sensors",
            theme.text_muted,
        ),
    ];

    let mut connectivity = [RowFrame::EMPTY; CONNECTIVITY_ROWS];
    for (i, item) in CONNECTIVITY_LIST.iter().enumerate() {
        connectivity[i] = build_row(
            Rect::new(PANEL_X, CONNECTIVITY_TOP + i * (ROW_H + ROW_GUTTER), PANEL_W, ROW_H),
            *item,
            &values[item_index(*item)],
            None,
            ROW_SCALE,
            &theme,
        );
    }

    // WHY each level control is handed its own reading rather than one shared
    // "available" flag: brightness and audio are bound by different backends,
    // and the day one of them is bound its row must be able to light up without
    // the other being told a lie. Today both return `None`.
    let levels = [
        build_row(BRIGHTNESS_ROW, LEVEL_LIST[0], &values[item_index(LEVEL_LIST[0])], brightness_level(), ROW_SCALE, &theme),
        build_row(VOLUME_SLIDER, LEVEL_LIST[1], &values[item_index(LEVEL_LIST[1])], volume_level(), ROW_SCALE, &theme),
    ];

    // WHY the list computes its own row height: the band is fixed and the number
    // of rows is a property of the platform, so a fixed height would leave a dead
    // band under the list whenever that number changed. The floor is what a row
    // has to hold, so no row can ever be shorter than the type inside it.
    let row_h = (LIST_BOUNDS.h.saturating_sub(ROW_GUTTER * (SYSTEM_LIST.len() - 1)) / SYSTEM_LIST.len())
        .clamp(ROW_H, ROW_MAX);
    let mut system = [RowFrame::EMPTY; SYSTEM_ROWS];
    for (i, item) in SYSTEM_LIST.iter().enumerate() {
        system[i] = build_row(
            Rect::new(PANEL_X, SYSTEM_TOP + i * (row_h + ROW_GUTTER), PANEL_W, row_h),
            *item,
            &values[item_index(*item)],
            None,
            ROW_SCALE,
            &theme,
        );
    }

    // WHY the footer label steps down to the value's scale: the card is 324 wide
    // against the rows' 648, and "Notifications" at ROW_SCALE plus its value
    // cannot fit - it rendered as "Notifi...". At VALUE_SCALE the full word
    // fits beside the count, and label and value agree in size the way they do
    // in every other row.
    let footer = build_row(FOOTER_CARD, FOOTER_LIST[0], &values[item_index(FOOTER_LIST[0])], None, VALUE_SCALE, &theme);

    QuickControlsFrame {
        clock: LabelRun::left(
            Rect::new(PANEL_X + space::XS, 0, PANEL_W, STATUS_BAR_HEIGHT),
            type_scale::CAPTION,
            clock_bytes,
            theme.text,
        ),
        heading: LabelRun::left(
            Rect::new(PANEL_X, HEADING_Y, PANEL_W, BODY_H),
            type_scale::BODY,
            b"Quick Controls",
            theme.text,
        ),
        summary: LabelRun::left(
            Rect::new(PANEL_X, SUMMARY_Y, PANEL_W, CAPTION_H),
            type_scale::CAPTION,
            &summary_raw[..summary_len],
            theme.text_muted,
        ),
        sections,
        connectivity,
        levels,
        system,
        footer,
        // WHY the button's label is centred in the space to the right of the
        // glyph rather than in the whole button: the glyph sits at a fixed
        // inset from the left, and a label centred across the full 272 columns
        // sits visibly left of the button's own centre line.
        power_label: LabelRun::centred(
            POWER_LABEL_X,
            POWER_BTN.y + (POWER_BTN.h - CAPTION_H) / 2,
            (POWER_BTN.x + POWER_BTN.w - space::MD).saturating_sub(POWER_LABEL_X),
            type_scale::CAPTION,
            b"Power menu",
            theme.text,
        ),
    }
}

/// Position of `row` in [`ALL_ROWS`].
///
/// WHY a lookup and not a hand-written index per call site: the two tables have
/// to stay in step, and a mistyped index would silently give a row another row's
/// reading -- which on this panel would mean one control claiming another
/// control's hardware answer.
fn item_index(row: PanelRow) -> usize {
    ALL_ROWS.iter().position(|r| *r == row).unwrap_or(0)
}

struct FrameSlot {
    state: usize,
    at_x: usize,
    at_y: usize,
    measured: bool,
    frame: QuickControlsFrame,
}

impl FrameSlot {
    fn holds_next(&self, x: usize, y: usize, state: usize) -> bool {
        self.measured
            && self.state == state
            && ((x == self.at_x + 1 && y == self.at_y) || (x == 0 && y == self.at_y + 1))
    }
}

struct FrameMemo(UnsafeCell<FrameSlot>);
unsafe impl Sync for FrameMemo {}

/// WHY a memo and not a rebuild per pixel: `measure_frame` calls the kernel, and
/// the shell is compiled at `opt-level = 0`, so measuring on every one of a
/// million pixels would spend the whole frame budget in syscalls. The renderer
/// walks the surface in raster order, so remembering the last pixel's address is
/// enough to tell a continuing walk from a new frame. The previous version used
/// `static mut` plus `addr_of_mut!`; `live_widgets` already moved this to an
/// `UnsafeCell` behind a single `Sync` impl, and two surfaces with two
/// hand-rolled copies of it is how one of them ends up aliasing the other's
/// cache.
static FRAME_MEMO: FrameMemo = FrameMemo(UnsafeCell::new(FrameSlot {
    state: 0,
    at_x: 0,
    at_y: 0,
    measured: false,
    frame: QuickControlsFrame::empty(),
}));

fn frame_for(x: usize, y: usize, state: &LauncherState) -> &QuickControlsFrame {
    let addr = state as *const LauncherState as usize;
    let slot = unsafe { &mut *FRAME_MEMO.0.get() };
    if !slot.holds_next(x, y, addr) {
        slot.frame = measure_frame(state);
        slot.state = addr;
        slot.measured = true;
    }
    slot.at_x = x;
    slot.at_y = y;
    &slot.frame
}

// -----------------------------------------------------------------------------
// Icons
// -----------------------------------------------------------------------------

#[derive(Clone, Copy)]
enum QcIcon {
    Wifi,
    Bluetooth,
    Cellular,
    Sun,
    Speaker,
    Torch,
    Rotate,
    Moon,
    Airplane,
    Sensors,
    Battery,
    Lock,
    Storage,
    Package,
    Bell,
}

/// Whether the glyph `icon` covers the pixel at offset `(dx, dy)` from its box's
/// centre.
///
/// WHY the glyphs are computed rather than stored: a bitmap for fifteen 28-row
/// marks is fifteen fixed tables to carry in the image and to redraw whenever a
/// box moves, and this shell already rasterises type one pixel at a time. Every
/// drawer is bounded by `ICON / 2`, which is what keeps a mark inside its row.
#[inline]
fn icon_bits(icon: QcIcon, dx: i32, dy: i32) -> bool {
    let adx = dx.abs();
    let ady = dy.abs();
    let d2 = dx * dx + dy * dy;

    match icon {
        QcIcon::Wifi => {
            // WHY `dy` is biased by 2 before anything is tested: the fan of arcs
            // reaches fifteen rows above the mark's centre and the dot only five
            // below it, so drawn on the centre line the whole glyph sits in the
            // top half of its row. The radius is recomputed from the biased
            // coordinate rather than reusing `d2`, which was measured against the
            // unbiased one -- reusing it shifts the arcs but leaves the dot
            // stranded fifteen rows above where the arcs now end.
            //
            // WHY the dot is drawn only *below* the fan: the arcs are centred on
            // a point under the middle of the box, so drawing the upper half of
            // the dot put a mark above the arcs where the real Wi-Fi mark has
            // only background.
            let dy = dy - 2;
            let dd = dx * dx + dy * dy;
            if dy >= 0 && dy <= 5 && dd <= 9 {
                return true;
            }
            if dy <= 0 && adx <= 9 && (49..=81).contains(&dd) {
                return true;
            }
            if dy <= 0 && adx <= 13 && (169..=225).contains(&dd) {
                return true;
            }
            false
        }
        QcIcon::Bluetooth => {
            // WHY the rune is built from four exact diagonals and one stem: the
            // old drawer tested a stem at `dx == 0` and then four diagonal
            // predicates whose bands overlapped and filled the right half of the
            // box, so it rendered as a wedge beside a bar -- the shape that read
            // as "K|". The real mark is a zigzag down the right side plus the
            // straight stem:
            //     A(0,-12) B(7,-6) C(0,0) D(7,6) E(0,12)
            // Each leg is the set of pixels within a pixel of a line through two
            // of those points, which is what the four `(6*dx -+ 7*dy)` forms are.
            //
            // WHY `dx` is biased by -3: the stem is the left edge of the mark and
            // the chevrons are its right, so drawn on the centre the whole rune
            // sits three columns right of the middle of its row.
            let dx = dx + 3;
            if dx == 0 && ady <= 12 {
                return true;
            }
            if (0..=7).contains(&dx) {
                if (6 * dx - 7 * dy - 84).abs() <= 4
                    || (6 * dx + 7 * dy).abs() <= 4
                    || (6 * dx - 7 * dy).abs() <= 4
                    || (6 * dx + 7 * dy - 84).abs() <= 4
                {
                    return true;
                }
            }
            false
        }
        QcIcon::Cellular => {
            // Four bars of increasing height on a shared baseline, three rows per
            // step so the tallest is twelve and the shortest four.
            let k: i32 = if dx >= -12 && dx <= -8 {
                0
            } else if dx >= -6 && dx <= -2 {
                1
            } else if dx >= 0 && dx <= 4 {
                2
            } else if dx >= 6 && dx <= 10 {
                3
            } else {
                return false;
            };
            let h = (k + 1) * 3;
            dy >= 6 - h && dy <= 6
        }
        QcIcon::Sun => {
            if d2 <= 16 {
                return true;
            }
            if (adx <= 1 && ady >= 6 && ady <= 11) || (ady <= 1 && adx >= 6 && adx <= 11) {
                return true;
            }
            (adx - ady).abs() <= 1 && adx >= 5 && adx <= 9
        }
        QcIcon::Speaker => {
            // WHY `dx` is biased by -1: the box sits at the left and the wave arc
            // runs out to the right, so drawn on the centre the whole mark sits a
            // column right of the middle of its row.
            //
            // WHY the arc is clipped to `dx >= 1`: a full circle of radius 9..11
            // has its left half inside that window too, so the "wave" closed into
            // a ring and the mark rendered as a symmetric bowtie. Only the right
            // half of the circle is a wave.
            let dx = dx - 1;
            // Driver box.
            if dx >= -10 && dx <= -7 && ady <= 4 {
                return true;
            }
            // Cone: apex at the box, flaring out to the right.
            if dx >= -7 && dx <= 0 && ady <= dx + 7 {
                return true;
            }
            // One sound-wave arc.
            let wx = dx - 2;
            dx >= 1 && (81..=121).contains(&(wx * wx + dy * dy))
        }
        QcIcon::Torch => {
            // WHY both halves are outlined one pixel thick: a filled head over a
            // filled body is one solid block at this size, with no gap to read
            // as two parts, and a two-pixel outline closes up the same way.
            if adx <= 10 && dy >= -12 && dy <= -5 && (adx >= 9 || dy <= -11 || dy >= -6) {
                return true;
            }
            adx <= 5 && dy >= -4 && dy <= 11 && (adx >= 4 || dy <= -3 || dy >= 10)
        }
        QcIcon::Rotate => {
            // WHY the ring is broken: a complete ring with a mark beside it reads
            // as a plain circle. The small triangle in the gap is what makes it a
            // rotation mark -- the earlier wedge filled the whole upper-right
            // quadrant and read as a solid corner, not as an arrowhead.
            if (81..=144).contains(&d2) && !(dx >= 3 && dy <= -3) {
                return true;
            }
            if dy >= -11 && dy <= -5 {
                let span = (dy + 11) / 2;
                if dx >= 1 && dx <= 1 + span {
                    return true;
                }
            }
            false
        }
        QcIcon::Moon => {
            // WHY the carve is offset to the upper right and is nearly as large as the
            // disc: a crescent is an outer circle with a second circle taken out
            // of it, and offsetting the carve to the left produced a mark opening
            // the wrong way. Carving with a *smaller* circle leaves a bitten
            // disc rather than a crescent -- at r 10.5 against r 12 the mark was
            // still three quarters full.
            let ix = dx - 5;
            let iy = dy + 2;
            d2 <= 144 && ix * ix + iy * iy >= 121
        }
        QcIcon::Airplane => {
            if adx <= 1 && dy >= -12 && dy <= 12 {
                return true;
            }
            if dy >= -3 && dy <= 1 && adx <= 12 {
                return true;
            }
            dy >= 8 && dy <= 11 && adx <= 5
        }
        QcIcon::Sensors => {
            // A package outline, a die in the middle and four corner pins: the
            // one mark that can mean "a sensor is bound here" without claiming
            // to be one particular sensor.
            if adx <= 9 && ady <= 9 && !(adx <= 7 && ady <= 7) {
                return true;
            }
            if d2 <= 4 {
                return true;
            }
            (ady == 11 && (adx == 3 || adx == 11)) || (adx == 11 && (ady == 3 || ady == 11))
        }
        QcIcon::Battery => {
            // WHY a centred dash instead of a fill: no fuel gauge is bound, and
            // a filled battery asserts a charge level this shell never measured.
            if dx >= -11 && dx <= 8 && ady <= 6 {
                if dx <= -10 || dx >= 7 || ady >= 5 {
                    return true;
                }
                if dy == 0 && dx >= -4 && dx <= 3 {
                    return true;
                }
            }
            dx >= 9 && dx <= 12 && ady <= 3
        }
        QcIcon::Lock => {
            if dx >= -9 && dx <= 9 && dy >= -1 && dy <= 11 && !(dx >= -4 && dx <= 4 && dy >= 3 && dy <= 8)
            {
                return true;
            }
            let iy = dy + 1;
            dy <= -1 && dy >= -11 && (36..=81).contains(&(dx * dx + iy * iy))
        }
        QcIcon::Storage => {
            if !(dx >= -11 && dx <= 11 && dy >= -8 && dy <= 8) {
                return false;
            }
            if dx <= -10 || dx >= 10 || dy <= -7 || dy >= 7 {
                return true;
            }
            if dy >= 2 && dy <= 4 && dx >= -6 && dx <= 6 {
                return true;
            }
            dx >= -8 && dx <= -5 && dy >= -5 && dy <= -2
        }
        QcIcon::Package => {
            let col = (dx >= -9 && dx <= -3) || (dx >= 3 && dx <= 9);
            let row = (dy >= -9 && dy <= -3) || (dy >= 3 && dy <= 9);
            col && row
        }
        QcIcon::Bell => {
            if dy == -9 && adx <= 1 {
                return true;
            }
            if dy >= -8 && dy <= 6 {
                // The bell's body: a trapezoid two rows thick, closed at the top
                // and flared at the foot, then a clapper under it.
                let span = 3 + ((dy + 8) * 9) / 14;
                if adx <= span && (adx + 1 >= span || dy == -8 || dy == 6) {
                    return true;
                }
            }
            dy >= 7 && dy <= 10 && adx <= 2
        }
    }
}

/// The power mark: a ring open at the top, with a stem through the gap.
///
/// WHY it is its own function and not another [`QcIcon`] arm: the previous
/// drawer shared one routine between "rotate" and "power", which is how the
/// rotation glyph ended up carrying a power button's stem.
#[inline]
fn power_bits(dx: i32, dy: i32) -> bool {
    if (100..=196).contains(&(dx * dx + dy * dy)) && dy > -6 {
        return true;
    }
    dx.abs() <= 2 && dy >= -13 && dy <= -2
}

// -----------------------------------------------------------------------------
// Painter
// -----------------------------------------------------------------------------

/// Colours the pixel of one row, or returns `None` if the pixel is not in it.
///
/// WHY the track is tested before the icon: a level control's track runs the full
/// padded width under its text line, and letting the glyph win that test would
/// paint it over the bar.
fn row_pixel(x: usize, y: usize, row: &RowFrame, theme: &Theme) -> Option<u32> {
    if !round_contains(&row.rect, radius::CARD, x, y) {
        return None;
    }
    let inner = row.rect.inset(1);
    if !round_contains(&inner, radius::CARD.saturating_sub(1), x, y) {
        return Some(row.border);
    }

    if let Some(track) = row.track {
        if round_contains(&track, track.h / 2, x, y) {
            // WHY a fill is drawn only when a level was read: a bar filled to the
            // end is a claim that the level is at maximum, and with nothing read
            // there is nothing to claim. `fill_w` is zero for every reading this
            // platform can produce today; the arm is written so that the day a
            // real one exists it fills to the real proportion and no further.
            let filled = row.fill_w > 0 && x <= track.x + row.fill_w;
            return Some(if filled { LEVEL_FILL } else { theme.border });
        }
    }

    let cx = row.rect.x + space::MD + ICON / 2;
    let cy = row.rect.center_y();
    if icon_bits(row.icon, x as i32 - cx as i32, y as i32 - cy as i32) {
        return Some(row.ink);
    }
    if let Some(c) = row.label.pixel_on(x, y, row.fill) {
        return Some(c);
    }
    if let Some(c) = row.value.pixel_on(x, y, row.fill) {
        return Some(c);
    }
    Some(row.fill)
}

/// Colours the pixel of the Power button, or returns `None`.
///
/// WHY the whole button is one control and not a glyph plus a separate target:
/// `main.rs` opens the Power drawer from `(630..740, 375..680)`, so every pixel
/// inside the drawn rectangle is reachable and none outside it is. Nothing inside
/// is a second control, which is why no row is drawn inside it.
fn power_pixel(x: usize, y: usize, label: &LabelRun, theme: &Theme) -> Option<u32> {
    if !round_contains(&POWER_BTN, radius::CARD, x, y) {
        return None;
    }
    let inner = POWER_BTN.inset(1);
    if !round_contains(&inner, radius::CARD.saturating_sub(1), x, y) {
        // WHY the outline is the theme's `danger`: this is the only control here
        // that can end the session, and the red ring is the only thing on the
        // panel saying so. The previous version typed 0xFFEF4444, which is not a
        // colour the theme has.
        return Some(theme.danger);
    }
    let dx = x as i32 - POWER_ICON_CX as i32;
    let dy = y as i32 - POWER_BTN.center_y() as i32;
    if power_bits(dx, dy) {
        return Some(theme.danger);
    }
    if let Some(c) = label.pixel(x, y) {
        return Some(c);
    }
    Some(theme.surface_raised)
}

#[inline(never)]
pub fn get_quick_controls_color(x: usize, y: usize, state: &LauncherState) -> u32 {
    let theme = THEME;
    let f = frame_for(x, y, state);

    // Status bar: the clock and nothing else. WHY no battery, signal or Wi-Fi
    // mark here: this panel reports every one of those below, in a row that says
    // where the number came from, and a second, less honest copy in the chrome
    // would be one more place for the two to disagree.
    if y < STATUS_BAR_HEIGHT {
        if let Some(c) = f.clock.pixel(x, y) {
            return c;
        }
        return theme.overlay;
    }
    if y == STATUS_BAR_HEIGHT {
        return theme.overlay_border;
    }

    if let Some(c) = f.heading.pixel(x, y) {
        return c;
    }
    if let Some(c) = f.summary.pixel(x, y) {
        return c;
    }
    for section in &f.sections {
        if let Some(c) = section.pixel(x, y) {
            return c;
        }
    }

    for row in f
        .connectivity
        .iter()
        .chain(f.levels.iter())
        .chain(f.system.iter())
    {
        if let Some(c) = row_pixel(x, y, row, &theme) {
            return c;
        }
    }
    if let Some(c) = row_pixel(x, y, &f.footer, &theme) {
        return c;
    }
    if let Some(c) = power_pixel(x, y, &f.power_label, &theme) {
        return c;
    }

    // Home indicator.
    let indicator = Rect::new(
        (SCREEN_WIDTH - INDICATOR_W) / 2,
        SCREEN_HEIGHT - space::MD - INDICATOR_H,
        INDICATOR_W,
        INDICATOR_H,
    );
    if round_contains(&indicator, INDICATOR_H / 2, x, y) {
        return Theme::blend(theme.text_muted, theme.bg, 140);
    }

    theme.bg
}