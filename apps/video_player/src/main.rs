// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

//! InterOS Native Video Player (Ring-3 User Process)
//!
//! Production-grade VLC-class media player for InterOS:
//! - Immersive dark theme (#111214) with banded zero-heap rendering
//! - Video Library & Playlist Drawer/Sheet:
//!   Scans /storage/videos and /storage/downloads, displays title, file size,
//!   codec ("PPM P6", "MJPEG", "RAW RGB24", "H.264"), resolution and duration.
//! - Playback Speed Selector: 0.5x, 0.75x, 1.0x, 1.25x, 1.5x, 2.0x
//! - Orientation Mode: Portrait (centered 16:9 canvas) <-> Landscape (fullscreen 720x1440 with translucent HUD)
//! - Picture-in-Picture (PIP): 280x158 floating preview card with live playback during library browsing
//! - Audio Volume & Mute HUD: Volume bar, speaker icon, mute toggle, system audio control syscalls
//! - Repeat Modes (None, Single, All) and Aspect Ratio cycling (Fit, Fill, Stretch, 16:9, 4:3)
//! - Typography: Scale 2 Body for titles, timestamps, playlist rows, and options
//! - Touch Targets: All hit areas >= 48px with comfortable spacing
//! - Platform Truth Contract: Never fakes hardware H.264 decode without physical silicon:
//!   "No hardware H.264 decoder bound. Software decode unavailable."

#![no_std]
#![no_main]

use core::panic::PanicInfo;
use interos_std as std;

use std::design::back_button;
use std::design::font;
use std::design::outline_text;
use std::design::primitives::{draw_circle, draw_rounded_rect, Rect};
use std::design::accessibility::{DynamicType, UserTextScale};

use video_player::avi::{self, AviChunk, AviError, AviInfo, AviStream};
use video_player::theme::{
    COLOR_ACCENT, COLOR_BG, COLOR_BORDER, COLOR_PLAY_FILL, COLOR_PLAY_GLYPH,
    COLOR_PROGRESS_ACTIVE, COLOR_PROGRESS_BG, COLOR_SHEET_CARD, COLOR_SURFACE,
    COLOR_SURFACE_SUNKEN, COLOR_TEXT_MUTED, COLOR_TEXT_PRIMARY, MIN_TOUCH_TARGET, THEME,
};
use video_player::{
    blend_argb, format_file_size, format_resolution, format_time, map_canvas_to_source,
    map_coords_to_source, scrubber_seek_seconds, scrubber_thumb_x, volume_seek_percent,
    volume_thumb_x, AspectRatio, DecoderStatus, LoopMode, Orientation, PlaybackSpeed,
    PlaybackState, Playlist, VideoCodec, CANVAS_H, CANVAS_W, NO_HW_H264_MSG,
};

// -----------------------------------------------------------------------------
// The type ladder, asked at the text-size level the person chose
// -----------------------------------------------------------------------------
//
// WHY THIS BLOCK EXISTS AT ALL. `font::cap_for_scale` and `font::line_height`
// are `const fn`s, so a `const HEADER_LINE_Y: usize = .. + font::line_height(2)`
// is frozen at the value the ladder had when the binary was compiled: it can
// never see the setting. The runtime faces of that same arithmetic
// (`font::line_height_user`, `cap_for_scale_user`, `measure_user`) read the one
// process-wide cell the person's setting is published into, and they are what a
// surface that re-derives its geometry at draw time has to call. Below, every
// sum this screen lays text into is written once as a `const fn` over a level --
// so the compile-time gates can still see all four rungs -- and read at draw
// time through the published level.
//
// WHY the platform level keeps every pixel it kept before: `*_at(level)` is the
// ladder's own arithmetic at that level, so at `PLATFORM_LEVEL` each derivation
// below returns exactly the constant it replaced. The gates beside each one pin
// that, and pinning it is what stops "adopted the published helpers" from
// silently resizing a screen that never asked for a text size.
//
// WHY a clamp instead of an assert where the plate cannot grow: the media
// properties card and the portrait header band are bounded by something other
// than their own text -- the card by the system home band, the header by the
// canvas below it -- so there the row step is the smaller of "the rung's box
// plus the air it wants" and "what the plate can actually hold". The bound is
// checked at every selectable level, so a rung can tighten the leading but can
// never make one row's ink ride over the next row's.

/// The ladder's own level: one hundred per cent, and what a person who never
/// touched Text size draws at.
const PLATFORM_LEVEL: u8 = font::TEXT_SCALE_NORMAL;
/// The largest level the setting can hold -- the rung every gate below is
/// measured at as well as the platform's, because a row that only fits at one
/// hundred per cent does not fit the setting at all.
const EXTRA_LARGE_LEVEL: u8 = font::TEXT_SCALE_EXTRA_LARGE;

/// The line box rung `scale` reserves at `level`.
const fn line_at(scale: usize, level: u8) -> usize {
    font::line_height_for_user_scale(scale, level)
}

/// The cap height rung `scale` is inked at on `level`.
const fn cap_at(scale: usize, level: u8) -> u32 {
    font::cap_for_user_scale(scale, level)
}

/// The em rung `scale` is rasterised at on `level`.
const fn em_at(scale: usize, level: u8) -> u32 {
    font::em_for_user_scale(scale, level)
}

/// The ascent rung `scale` reserves on `level` -- the rows a run's ink actually
/// takes above its baseline, which is what decides whether two stacked runs
/// share a pixel.
const fn ascent_at(scale: usize, level: u8) -> usize {
    font::ascent_for_user_scale(scale, level)
}

/// The level this process draws at: what `display.text_scale` published at
/// startup, and `PLATFORM_LEVEL` for a process that published nothing.
#[inline]
fn level() -> u8 {
    font::user_text_scale()
}

/// The line box a rung reserves at the published level.
#[inline]
fn line_u(scale: usize) -> usize {
    line_at(scale, level())
}

/// The advance a run takes at `level`, in the weight it is drawn in.
const _: () = assert!(line_at(2, PLATFORM_LEVEL) == font::line_height(2));
/// WHY both rungs and not just the body one: the landscape scrim and the
/// header's rule are each the pair of line boxes plus their air, and the
/// platform-equality of the second rung is what says a scrim that never asked
/// for a text size is exactly as tall as the one drawn before the setting
/// could be read at all.
const _: () = assert!(line_at(1, PLATFORM_LEVEL) == font::line_height(1));

#[inline]
fn run_w_at(text: &[u8], scale: usize, bold: bool, level: u8) -> usize {
    if bold {
        font::measure_bold_user_scale(text, scale, level)
    } else {
        font::measure_user_scale(text, scale, level)
    }
}

/// The advance a run takes at the published level, in the weight it is drawn in.
#[inline]
fn run_w(text: &[u8], scale: usize, bold: bool) -> usize {
    run_w_at(text, scale, bold, level())
}

/// The run, trimmed to `max_w` at the published level, in `out`.
///
/// WHY an app-local fit and not `font::ellipsize`: `ellipsize` measures with the
/// ladder's advance, so a run it declared short enough is a run that walks past
/// its column the moment the person raises the text size. The elision
/// convention is the fleet's own -- keep the head, drop the tail, mark it with
/// `...` -- and only the measurement moves.
///
/// WHY the run is never silently clipped: what does not fit comes back with the
/// marker on it, and a `max_w` that cannot hold even the marker answers zero
/// length, which the caller's paint gate reads as "nothing here" rather than
/// "half a word there".
fn fit_run(text: &[u8], scale: usize, bold: bool, max_w: usize, out: &mut [u8]) -> usize {
    if run_w(text, scale, bold) <= max_w {
        let n = text.len().min(out.len());
        out[..n].copy_from_slice(&text[..n]);
        return n;
    }
    const DOTS: &[u8] = b"...";
    let dots_w = run_w(DOTS, scale, bold);
    if dots_w > max_w || out.len() < 3 {
        return 0;
    }
    let budget = max_w - dots_w;
    let em = em_at(scale, level());
    let mut used = 0usize;
    let mut fit = 0usize;
    for (i, &c) in text.iter().enumerate() {
        let mut adv = std::design::typeface::advance_px(c, em) as usize;
        if adv == 0 {
            // A byte the face has no glyph for still walks the bitmap cell's
            // advance, or the cursor never reaches the end of the budget.
            adv = 8 * scale;
        }
        if used + adv > budget || fit + 3 > out.len() {
            break;
        }
        used += adv;
        fit = i + 1;
    }
    out[..fit].copy_from_slice(&text[..fit]);
    out[fit..fit + 3].copy_from_slice(DOTS);
    fit + 3
}

/// Reads `display.text_scale` and hands the rung to the renderer.
///
/// WHY before the first frame is written: the helpers above read one published
/// cell, so a surface that published nothing draws the platform's ladder forever
/// -- which is precisely the defect this screen used to have.
///
/// WHY `if let` and not `unwrap_or(Normal)`: a value this app cannot read is not
/// a level the person chose, and the rung the process already holds -- Normal,
/// the ladder -- stands rather than a guess being published.
///
/// WHY once at launch and once per re-show, and never per frame: the store is
/// a syscall, and putting it inside the paint budget is what the published
/// cell exists to avoid; a re-show is the one moment the held rung can be
/// stale, because Settings answers no notification when it writes.
fn publish_text_scale() {
    let mut buf = [0u8; 32];
    if let Ok(n) = std::settings_get("display.text_scale", &mut buf) {
        if let Some(scale) = UserTextScale::from_settings_value(&buf[..n]) {
            DynamicType::new(scale).publish();
        }
    }
}

// -----------------------------------------------------------------------------
// Surface & Screen Geometry
// -----------------------------------------------------------------------------

const W: usize = 720;
const H: usize = 1440;
const CHUNK: usize = 4096;

// Band 0: Top Bar (Back, Title, Orientation, Library / Kebab)
const BAND0_Y0: usize = 0;
/// The band's bottom: the header's rule, plus the air the gate below asks for.
/// WHY derived and not a remembered 120: the band's two runs come from the type
/// ladder, and when the ladder's rungs grew the pair ended past the boundary --
/// the rule was then authored in band 1, whose repaint never draws it.
/// WHY a function and not a constant: the pair's boxes follow the person's
/// text-size level, so the row this band stops answering has to move with them;
/// a constant here is the reason a scaled header used to be answered by the
/// canvas band beneath it. `band0_y1_at` is the same sum written over a level,
/// which is what the gates beside it can read.
const fn band0_y1_at(level: u8) -> usize {
    header_rule_y_at(level) + 4
}

#[inline]
fn band0_y1() -> usize {
    band0_y1_at(level())
}

/// The first row this app's own chrome may author from.
///
/// WHY not the top of the panel: this app paints no status band today, so chrome
/// authored at row 32 has its ink while nobody is above it. The platform's
/// content boundary is where every sibling screen starts, and the day this app
/// adopts the shared band the band answers the pixel function before this painter
/// is reached -- a badge authored above the boundary is then a badge whose crown
/// simply does not exist.
const HEADER_TOP: usize = std::design::viewport::HEADER_TOP;
/// The header's title row, the line under it, and the rule that closes the band.
///
/// WHY derived from the ladder rather than written out: the title draws on the
/// body rung and the line under it on the caption rung, so the second run's top
/// is exactly the first run's own line box, and the rule sits under whichever box
/// is the taller of the two. Two literal rows for one run is how a value came to
/// read four rows high on the sheet below.
const HEADER_TITLE_Y: usize = HEADER_TOP;
/// The second run's top, the rule under both, and the sums written over a level.
const fn header_line_y_at(level: u8) -> usize {
    HEADER_TITLE_Y + line_at(2, level)
}

const fn header_rule_y_at(level: u8) -> usize {
    header_line_y_at(level) + line_at(1, level) + 2
}

#[inline]
fn header_line_y() -> usize {
    header_line_y_at(level())
}

#[inline]
fn header_rule_y() -> usize {
    header_rule_y_at(level())
}

/// WHY these numbers are the same at the platform level as the constants they
/// replace: the published helpers are the ladder's arithmetic at a level, and a
/// screen that never asked for a text size must not be resized by the code that
/// reads one.
const _: () = assert!(line_at(2, PLATFORM_LEVEL) == font::line_height(2));
const _: () = assert!(header_rule_y_at(PLATFORM_LEVEL) == HEADER_RULE_Y_PLATFORM);
/// The platform's own sums, kept as the number the header drew before the
/// setting could be read at all -- what the equality gate above is measured
/// against, and what the gates below clear the taller rung against.
const HEADER_LINE_Y_PLATFORM: usize = HEADER_TITLE_Y + font::line_height(2);
const HEADER_RULE_Y_PLATFORM: usize = HEADER_LINE_Y_PLATFORM + font::line_height(1) + 2;
/// The widest the centred portrait title may be.
///
/// WHY it is derived from the control that stands to its right: the run is
/// centred on the panel, so half its width reaches either side of the middle, and
/// the first control that owns those rows is the one it must not reach. A bound
/// wider than that is a bound on a run whose tail is claimed by a control tested
/// before it, and the tail then never exists.
const HEADER_TITLE_MAX_W: usize = 2 * (ORIENT_BTN.x - W / 2);
/// WHY checked at compile time: the header branch answers every pixel below
/// `BAND0_Y1`, so a rule that reached past it loses the band it belongs to, and a
/// run whose box reached the rule is a run sharing pixels with a hairline.
const _: () = assert!(HEADER_TITLE_Y >= std::design::viewport::STATUS_BAR_HEIGHT);
/// WHAT THIS GATES: at every level the setting can hold, the header's two runs
/// keep their own boxes and the band keeps its rule, and the canvas the band
/// hands its rows to still fits band 1 at the rung the band reached.
///
/// WHY a loop over the four levels and not one assert at the platform rung: the
/// asserts these replaced could only ever see one hundred per cent, and a raised
/// text size is exactly the case where the header's pair outgrows the band that
/// paints it -- the defect the scaled ink would otherwise land on.
const fn header_bands_clear_at_every_level() -> bool {
    let mut level = 0u8;
    while level <= EXTRA_LARGE_LEVEL {
        // The caption run starts only once the title's whole box is behind it,
        // and the rule only once the caption's whole box is behind it.
        if header_line_y_at(level) < HEADER_TITLE_Y + line_at(2, level) {
            return false;
        }
        if header_rule_y_at(level) < header_line_y_at(level) + line_at(1, level) {
            return false;
        }
        if header_rule_y_at(level) + 4 > band0_y1_at(level) {
            return false;
        }
        // The band's foot and the canvas's top are one number, so the canvas has
        // to still stand inside the band that repaints it at the largest rung.
        if canvas_y0_at(level) + CANVAS_H > BAND1_Y1 {
            return false;
        }
        level += 1;
    }
    true
}
const _: () = assert!(header_bands_clear_at_every_level());

// Band 1: Centered 16:9 Video Canvas (680x382). Its top is `band0_y1()` at draw
// time -- the same sum as the header's rule, at the rung the header's two runs
// are drawn at -- so the band that answers those rows always ends where they do.
const BAND1_Y1: usize = 560;

// Band 2: Scrubber Seek Bar (Elapsed, Seek Bar, Remaining)
const BAND2_Y0: usize = 560;
const BAND2_Y1: usize = 670;
/// WHY checked at compile time: the two running-time numbers this band paints are
/// the body rung, and the band's own rows are not derived from it -- so the gate
/// is what says a raised text size still reserves the whole box of both runs
/// inside the band that answers them, at the largest level the setting holds.
const _: () = assert!(606 + line_at(2, EXTRA_LARGE_LEVEL) <= BAND2_Y1);

// Band 3: Transport Controls (Prev, 88px Play/Pause, Next)
const BAND3_Y0: usize = 670;
const BAND3_Y1: usize = 810;

// Band 4: Audio Volume HUD (Speaker, Bar, % Label)
const BAND4_Y0: usize = 810;
const BAND4_Y1: usize = 890;

// Band 5: Secondary Controls & Navigation
const BAND5_Y0: usize = 890;
/// WHY read off the band rather than authored as 1400: the rows the platform's
/// home box answers are not this app's to repaint as its own content.
const BAND5_Y1: usize = HOME_HIT.y;

// Band 6: Home Gesture Bar
const BAND6_Y0: usize = HOME_HIT.y;
const _BAND6_Y1: usize = 1440;

// -----------------------------------------------------------------------------
// Control Rectangles (All Touch Targets >= 48px)
// -----------------------------------------------------------------------------

// WHY every control here stands on `HEADER_TOP`: the painter and the press test
// the same rectangle, so the row these carry is the row the tap lands on, and
// the rule under the header is the foot they must stop above.
//
// The top-left control is deliberately NOT authored here. It is the platform's
// Back chip, and `back_button` owns that rect outright: `X` 24, `Y` = the same
// `HEADER_TOP` = 56, `SIZE` 48 with `RADIUS` 12, and a left-pointing chevron
// rather than a cross. What this screen drew here before was a 52x52 square of
// page colour with a bare ✕ in it: a sweep of that rect on the running guest
// found 93 pixels that were not the page, where every other app's chip footprint
// is 2120 -- the control a person has learned to look for was absent, and the
// mark that was there said "discard", not "back".
const ORIENT_BTN: Rect = Rect::new(568, HEADER_TOP, 52, 52);
const KEBAB_BTN: Rect = Rect::new(644, HEADER_TOP, 52, 52);
// WHY the chip is gated against the rule and not against the band: the painter
// answers all of band 0 (rows 0..120), so the hairline under the title is the
// last thing the chip can genuinely collide with. Its foot lands at row 104,
// where the square it replaced reached 108.
// WHY the platform rung is the case that has to hold: raising the text size only
// ever moves the rule *down*, so a chip that clears it at one hundred per cent
// clears it at every selectable level -- and the gate still binds.
const _: () = assert!(back_button::Y + back_button::SIZE < header_rule_y_at(PLATFORM_LEVEL));
/// WHY checked at compile time: these two are the only presses on the header
/// row the app itself authors, and both now wear a plate the eye can find -- the
/// gates hold the plate inside the band that repaints it, clear of each other,
/// clear of the gutter, and over the platform's minimum target on both axes.
const _: () = assert!(ORIENT_BTN.w >= MIN_TOUCH_TARGET && ORIENT_BTN.h >= MIN_TOUCH_TARGET);
const _: () = assert!(KEBAB_BTN.w >= MIN_TOUCH_TARGET && KEBAB_BTN.h >= MIN_TOUCH_TARGET);
const _: () = assert!(ORIENT_BTN.x + ORIENT_BTN.w <= KEBAB_BTN.x);
const _: () = assert!(KEBAB_BTN.x + KEBAB_BTN.w <= W - 24);
const _: () = assert!(ORIENT_BTN.y + ORIENT_BTN.h <= header_rule_y_at(PLATFORM_LEVEL));

/// The air the landscape HUD's top bar keeps around its two runs.
///
/// WHY one constant serves both edges: the bar is the title's line box, the
/// badge's line box under it, and this much free row above and below -- so the
/// number that separates the ink from the scrim is authored once and the bar's
/// height is read off the same pair. Written twice it drifts, and the drift is
/// the bar clipping the badge while the top edge stays roomy.
const LAND_BAR_AIR: usize = 12;

/// The landscape HUD's top bar, and the controls that stand inside it.
///
/// WHY they are placed from `HEADER_TOP` and not from a literal 24: landscape
/// draws on the same 720x1440 surface the portrait player uses, so the system
/// status band still owns the top 48 rows. The bar began at y=24 and its back
/// control at y=36, which left the first twelve rows of that control under the
/// status bar -- ink the user could not see, on a band the press still answered.
/// Both orientations now stand on the one header row.
///
/// WHY the height is derived from the ladder rather than remembered: the bar is
/// the scrim the two runs are painted inside, and a run whose box reaches the
/// bar's foot is a run whose tail is answered by the video underneath -- the
/// scrim stops asking for those pixels, so the tail is drawn over a frame the
/// painter already returned. The pair's boxes come from the type ladder, so the
/// wall that has to hold them is the ladder's sum plus the air on either side.
/// The scrim's height at `level`: the two runs' line boxes and the air on either
/// side. WHY the pair and not one of them: the bar answers every row between its
/// edges, so a badge whose box reaches the bar's foot is a badge whose tail is
/// painted by the video the scrim stopped asking for.
const fn land_bar_h_at(level: u8) -> usize {
    LAND_BAR_AIR + line_at(2, level) + line_at(1, level) + LAND_BAR_AIR
}

/// The bar at the platform's rung -- the rect the gates below read. The painter
/// and the press take [`land_bar`], which is the same sum at the published rung,
/// so a raised text size moves the scrim with the two runs it holds rather than
/// clipping the second of them.
const LAND_BAR: Rect = Rect::new(24, HEADER_TOP, 672, land_bar_h_at(PLATFORM_LEVEL));

/// WHAT THIS GATES: at every selectable level the bar holds both runs, the chip
/// stands inside the bar, and the bar's foot stops above the lower scrim.
///
/// WHY a loop and not the three asserts this replaced: those could only ever see
/// one hundred per cent, and the landscape HUD is the one surface here whose
/// scrim is exactly as tall as its two runs.
const fn landscape_bar_holds_at_every_level() -> bool {
    let mut level = 0u8;
    while level <= EXTRA_LARGE_LEVEL {
        let h = land_bar_h_at(level);
        // Both runs, end to end, inside the scrim.
        if LAND_BAR_AIR + line_at(2, level) + line_at(1, level) > h {
            return false;
        }
        // The chip is centred by halving the difference between the bar's rows
        // and the plate's, so the bar has to be at least as tall as the plate.
        if h < back_button::SIZE {
            return false;
        }
        // The bar grows downward into the strip it shares with the transport
        // panel; that panel's rows are blended from the frame on their own terms.
        if LAND_BAR.y + h > LAND_PANEL.y {
            return false;
        }
        level += 1;
    }
    true
}
const _: () = assert!(landscape_bar_holds_at_every_level());

/// The landscape scrim at the rung this frame draws at.
#[inline]
fn land_bar() -> Rect {
    Rect::new(LAND_BAR.x, LAND_BAR.y, LAND_BAR.w, land_bar_h_at(level()))
}
/// The landscape HUD's lower scrim: the transport, scrubber and volume panel the
/// same `hud_visible` branch paints.
///
/// WHY it is named here instead of built where the painter uses it: the bar's
/// foot grows downward into the strip between the two scrims, and that strip is
/// the one place the taller bar can still collide with something it does not
/// own. A gate needs both bounds to exist in a const context, and the panel's
/// top row only existed inside the per-pixel branch.
const LAND_PANEL: Rect = Rect::new(24, 1100, 672, 280);
// WHY the bar is gated against the chip before the chip is centred in it: the
// centre is found by halving the difference between the bar's rows and the
// plate's, and that difference is a `usize` -- a bar the ladder ever made
// shorter than the chip would panic the const evaluator on the subtraction
// rather than name the bound that broke.
const _: () = assert!(LAND_BAR.h >= back_button::SIZE);
/// The landscape HUD's own back control: the platform chip's 48x48 plate, stood
/// 12 columns inside the scrim's left edge and centred in the bar's rows.
///
/// WHY the position is the app's and the size is the platform's: the chip's home
/// rect starts at `back_button::X` = 24 and `back_button::Y` = 56, which are
/// exactly the scrim's own left edge and top row, so a landscape copy of it
/// would sit flush on the bar with no inset on either side. The bar is the
/// layout owner here. What the bar may not own is the shape:
/// `back_button::pixel` draws the plate, the 1px ring and the chevron, and it is
/// handed this rect's point translated into the chip's own coordinate space.
///
/// WHY the square went from 52 to 48: the plate is 48 across, so the old rect's
/// outer two columns and rows on every side were tappable area the user could
/// not see -- the same mismatch the portrait control had, in miniature.
///
/// WHY the inset is a named constant and not a literal: 36 is the one number the
/// bar owns about this control -- 12 columns of scrim air, which is
/// `LAND_BAR_AIR` stood on the scrim's own left edge -- and the painter and the
/// press both reach the plate through `LAND_BACK_BTN`. Naming it makes the inset
/// a layout decision with a name, and the gate under it proves the plate the
/// painter shifts into `back_button`'s space and the rect the press tests are
/// the same 48x48 box.
const LAND_BACK_X: usize = LAND_BAR.x + LAND_BAR_AIR;
const LAND_BACK_BTN: Rect = Rect::new(
    LAND_BACK_X,
    LAND_BAR.y + (land_bar_h_at(PLATFORM_LEVEL) - back_button::SIZE) / 2,
    back_button::SIZE,
    back_button::SIZE,
);

/// The plate at the rung this frame's scrim reaches: the same 48x48 the chip
/// paints, centred in the bar's rows, so a taller scrim centres the control
/// rather than leaving it pinned to the platform's rows.
#[inline]
fn land_back_btn() -> Rect {
    Rect::new(
        LAND_BACK_X,
        LAND_BAR.y + (land_bar_h_at(level()) - back_button::SIZE) / 2,
        back_button::SIZE,
        back_button::SIZE,
    )
}

/// WHY the centring is gated at every level and not assumed: both the bar's rows
/// and the plate's are `usize`, and the difference is halved -- a bar ever made
/// shorter than the plate would subtract past zero at run time instead of
/// drawing a button, which is the case the const gate above could only ever see
/// at one hundred per cent.
const fn land_back_centred_at_every_level() -> bool {
    let mut level = 0u8;
    while level <= EXTRA_LARGE_LEVEL {
        let y = LAND_BAR.y + (land_bar_h_at(level) - back_button::SIZE) / 2;
        if y < back_button::Y {
            return false;
        }
        if y + back_button::SIZE > LAND_BAR.y + land_bar_h_at(level) {
            return false;
        }
        level += 1;
    }
    true
}
const _: () = assert!(land_back_centred_at_every_level());
// WHY the shift is gated rather than assumed: both differences are `usize`, so
// a rect ever stood left of or above the chip's own origin the painter would
// subtract past zero instead of drawing a button.
const _: () = assert!(LAND_BACK_X >= back_button::X && LAND_BACK_BTN.y >= back_button::Y);
/// The title run, and the badge run one line box below it.
///
/// WHY the badge stands on the title's own box: the two runs are one column of
/// text, and the row the second one starts on is where the first one's descent
/// ends. A gap between them is a gap the bar then has to hold twice.
const fn land_title_y_at(_level: u8) -> usize {
    LAND_BAR.y + LAND_BAR_AIR
}

const fn land_badge_y_at(level: u8) -> usize {
    land_title_y_at(level) + line_at(2, level)
}

#[inline]
fn land_title_y() -> usize {
    land_title_y_at(level())
}

#[inline]
fn land_badge_y() -> usize {
    land_badge_y_at(level())
}

const _: () = assert!(LAND_BAR.y >= std::design::viewport::STATUS_BAR_HEIGHT);
const _: () = assert!(LAND_BACK_BTN.y + LAND_BACK_BTN.h <= LAND_BAR.y + LAND_BAR.h);
const _: () = assert!(land_badge_y_at(PLATFORM_LEVEL) + line_at(1, PLATFORM_LEVEL)
    <= LAND_BAR.y + land_bar_h_at(PLATFORM_LEVEL));
// Painted == hit, by construction: the painter hands `back_button::pixel` the
// point shifted by the plate's origin and `back_button`'s own, so the ink it can
// produce is exactly the rect the press covers -- never a wider rect, which is
// what the 52x52 square used to be.
//
// WHY this is pinned: the HUD painter moves a point by the difference between
// this rect's origin and `back_button`'s own, and that shift is only a faithful
// copy of the chip while the two rects are the same size. Were this rect larger,
// its extra rows would land outside the plate and show scrim where the user read
// a button.
const _: () = assert!(
    LAND_BACK_BTN.w == back_button::SIZE && LAND_BACK_BTN.h == back_button::SIZE
);

/// The landscape HUD's lower panel, control by control.
///
/// WHY these are named here instead of written where they are used: the
/// scrubber, the transport and the volume row were each authored twice -- once
/// as the literals the painter fills, once as the literals the press answers --
/// and the two copies had drifted apart. The play control was answered four
/// columns and four rows outside its own disc, and both bar bands were measured
/// in rows the platform's minimum target does not cover (41 for the scrubber,
/// 31 for the volume), so a thumb landing on the middle of a band it could not
/// reach did nothing at all.
const LAND_SEEK_X: usize = 120;
const LAND_SEEK_Y: usize = 1128;
const LAND_SEEK_W: usize = 460;
const LAND_SEEK_H: usize = 8;
const LAND_SEEK: Rect = Rect::new(LAND_SEEK_X, LAND_SEEK_Y, LAND_SEEK_W, LAND_SEEK_H);
/// Where the landscape remaining-time run starts, one gap right of the track.
const LAND_REMAIN_X: usize = 600;
/// The scrubber's touch band: the track's own columns plus the air between it
/// and the top of the scrim it is painted on.
///
/// WHY it stops where the panel starts rather than centring on the track: the
/// rows above `LAND_PANEL` are live video, and the press there belongs to the
/// HUD toggle, not to a seek. The band therefore takes every row the scrim owns
/// down to the transport -- which is what makes it a target the platform's
/// minimum covers without inventing a strip the panel never painted.
///
/// WHY it stops short of `LAND_REMAIN_X`: the portrait scrubber already gives
/// the remaining-time number its own columns; a band that ran under the first
/// strokes of that number would jump the playback on a press meant to read it.
const LAND_SEEK_HIT: Rect = Rect::new(LAND_SEEK_X - 10, LAND_PANEL.y, LAND_SEEK_W + 20, 52);

const LAND_TRANSPORT_CY: usize = 1210;
const LAND_PLAY_CX: usize = 360;
const LAND_PLAY_R: usize = 36;
const LAND_STEP_CX_LEFT: usize = 230;
const LAND_STEP_CX_RIGHT: usize = 490;
const LAND_STEP_R: usize = 24;
/// Each transport press: the bounding box of the disc that paints it, from the
/// same centre and radius the painter is handed.
const LAND_PLAY_HIT: Rect =
    Rect::new(LAND_PLAY_CX - LAND_PLAY_R, LAND_TRANSPORT_CY - LAND_PLAY_R, LAND_PLAY_R * 2, LAND_PLAY_R * 2);
const LAND_PREV_HIT: Rect = Rect::new(
    LAND_STEP_CX_LEFT - LAND_STEP_R,
    LAND_TRANSPORT_CY - LAND_STEP_R,
    LAND_STEP_R * 2,
    LAND_STEP_R * 2,
);
const LAND_NEXT_HIT: Rect = Rect::new(
    LAND_STEP_CX_RIGHT - LAND_STEP_R,
    LAND_TRANSPORT_CY - LAND_STEP_R,
    LAND_STEP_R * 2,
    LAND_STEP_R * 2,
);

const LAND_VOL_X: usize = 120;
const LAND_VOL_Y: usize = 1290;
const LAND_VOL_W: usize = 460;
const LAND_VOL_H: usize = 8;
const LAND_VOL: Rect = Rect::new(LAND_VOL_X, LAND_VOL_Y, LAND_VOL_W, LAND_VOL_H);
/// The volume band's foot: the panel's own bottom edge is 54 rows under the
/// track, so the band takes 52 of them and leaves the scrim's last rows to the
/// video the tap toggle still answers.
const LAND_VOL_HIT: Rect = Rect::new(LAND_VOL_X - 10, LAND_VOL_Y - 16, LAND_VOL_W + 20, 52);

/// The landscape mute control: the plate the painter fills and the press
/// answers, with its glyph centred on the plate rather than on a remembered
/// column.
///
/// WHY the plate exists at all: this control used to be answered as a 52x52
/// square around a bare speaker cone, and the cone is the only ink the scrim
/// ever put there -- on a translucent panel over live video that reads as a
/// mark on the picture, not as a button. The portrait mute control is now drawn
/// the same way, and both are the app's own plate style.
const LAND_MUTE_BTN: Rect = Rect::new(40, 1274, 52, 52);
const LAND_MUTE_CX: usize = LAND_MUTE_BTN.x + LAND_MUTE_BTN.w / 2;
const LAND_MUTE_CY: usize = LAND_MUTE_BTN.y + LAND_MUTE_BTN.h / 2;

/// WHY checked at compile time: the first four gates are the painting itself --
/// every band answers rows the scrim fills, and the transport presses are the
/// discs that draw them, so a control can no longer be answered where it is not
/// inked. The rest are the platform minimum and the collisions a taller band or
/// a wider plate would create: the scrubber meeting the transport, the volume
/// meeting the mute plate, the remaining-time run meeting the seek band's right
/// edge, and the mute plate leaving the panel it is painted on.
const _: () = assert!(
    LAND_SEEK.y >= LAND_SEEK_HIT.y
        && LAND_SEEK.y + LAND_SEEK.h <= LAND_SEEK_HIT.y + LAND_SEEK_HIT.h
);
const _: () = assert!(
    LAND_SEEK_HIT.x <= LAND_SEEK.x && LAND_SEEK_HIT.x + LAND_SEEK_HIT.w >= LAND_SEEK.x + LAND_SEEK.w
);
const _: () = assert!(LAND_SEEK_HIT.x + LAND_SEEK_HIT.w <= LAND_REMAIN_X);
const _: () = assert!(
    LAND_VOL.y >= LAND_VOL_HIT.y && LAND_VOL.y + LAND_VOL.h <= LAND_VOL_HIT.y + LAND_VOL_HIT.h
);
const _: () = assert!(
    LAND_VOL_HIT.x <= LAND_VOL.x && LAND_VOL_HIT.x + LAND_VOL_HIT.w >= LAND_VOL.x + LAND_VOL.w
);
const _: () = assert!(LAND_PLAY_HIT.x == LAND_PLAY_CX - LAND_PLAY_R && LAND_PLAY_HIT.w == LAND_PLAY_R * 2);
const _: () = assert!(
    LAND_PREV_HIT.x == LAND_STEP_CX_LEFT - LAND_STEP_R && LAND_PREV_HIT.w == LAND_STEP_R * 2
);
const _: () = assert!(
    LAND_NEXT_HIT.x == LAND_STEP_CX_RIGHT - LAND_STEP_R && LAND_NEXT_HIT.w == LAND_STEP_R * 2
);
const _: () = assert!(
    LAND_PLAY_HIT.y == LAND_TRANSPORT_CY - LAND_PLAY_R && LAND_PLAY_HIT.h == LAND_PLAY_R * 2
);
const _: () = assert!(LAND_SEEK_HIT.h >= MIN_TOUCH_TARGET && LAND_VOL_HIT.h >= MIN_TOUCH_TARGET);
const _: () = assert!(
    LAND_PLAY_HIT.w >= MIN_TOUCH_TARGET
        && LAND_PREV_HIT.w >= MIN_TOUCH_TARGET
        && LAND_NEXT_HIT.w >= MIN_TOUCH_TARGET
);
const _: () = assert!(LAND_MUTE_BTN.w >= MIN_TOUCH_TARGET && LAND_MUTE_BTN.h >= MIN_TOUCH_TARGET);
const _: () = assert!(LAND_SEEK_HIT.y + LAND_SEEK_HIT.h <= LAND_PREV_HIT.y);
const _: () = assert!(LAND_PLAY_HIT.y + LAND_PLAY_HIT.h <= LAND_VOL_HIT.y);
const _: () = assert!(LAND_MUTE_BTN.x + LAND_MUTE_BTN.w <= LAND_VOL_HIT.x);
const _: () = assert!(
    LAND_MUTE_BTN.x >= LAND_PANEL.x && LAND_MUTE_BTN.y + LAND_MUTE_BTN.h <= LAND_PANEL.y + LAND_PANEL.h
);
const _: () = assert!(LAND_VOL_HIT.y + LAND_VOL_HIT.h <= LAND_PANEL.y + LAND_PANEL.h);
const _: () = assert!(LAND_VOL.x + LAND_VOL.w <= LAND_PANEL.x + LAND_PANEL.w);
const _: () = assert!(
    LAND_PREV_HIT.x + LAND_PREV_HIT.w <= LAND_PLAY_HIT.x
        && LAND_PLAY_HIT.x + LAND_PLAY_HIT.w <= LAND_NEXT_HIT.x
);

const CANVAS_X0: usize = (W - CANVAS_W) / 2; // 20
const CANVAS_Y0: usize = 148;
/// The plate at the platform's rung, kept for the gates that name it; the painter
/// takes [`canvas_rect`], which is the same rect at the rung the header band reached.
const CANVAS_RECT: Rect = Rect::new(CANVAS_X0, CANVAS_Y0, CANVAS_W, CANVAS_H);
const _: () = assert!(CANVAS_RECT.y + CANVAS_RECT.h <= BAND1_Y1);
/// The canvas's top row at `level`.
///
/// WHY it follows the header band: band 0 answers rows up to its own rule, and
/// the rule sits under two runs whose boxes the person's text size sets. A
/// canvas that stayed at row 148 while the header reached 165 would have its
/// first seventeen rows answered by the header branch instead of by itself.
///
/// WHY the second clamp: the canvas is the one plate here whose height is the
/// media's, not the type's -- `CANVAS_H` is the decoded frame -- so the run-time
/// guard holds it inside the band that repaints it at every selectable level,
/// which the gate beside [`header_bands_clear_at_every_level`] proves cannot
/// bite at any of them.
const fn canvas_y0_at(level: u8) -> usize {
    let mut y = CANVAS_Y0;
    if band0_y1_at(level) > y {
        y = band0_y1_at(level);
    }
    if y > BAND1_Y1 - CANVAS_H {
        y = BAND1_Y1 - CANVAS_H;
    }
    y
}

#[inline]
fn canvas_rect() -> Rect {
    Rect::new(CANVAS_X0, canvas_y0_at(level()), CANVAS_W, CANVAS_H)
}

// Scrubber
const SEEK_X: usize = 124;
const SEEK_Y: usize = 614;
const SEEK_W: usize = 472;
const SEEK_H: usize = 8;
const SEEK_RECT: Rect = Rect::new(SEEK_X, SEEK_Y, SEEK_W, SEEK_H);
/// Where the remaining-time run starts, one gap to the right of the track.
const REMAIN_X: usize = SEEK_X + SEEK_W + 12;
/// The scrubber's touch band.
///
/// WHY it stops two columns short of `REMAIN_X`: the band is padded past the
/// 8-row track so a thumb lands on the seek, but it used to run to x=610 while
/// the remaining-time number starts at 608 -- so the first strokes of that
/// number answered a seek and jumped the playback.
const SEEK_HIT: Rect = Rect::new(SEEK_X - 14, 584, (REMAIN_X - 2) - (SEEK_X - 14), 68);
const _: () = assert!(SEEK_HIT.x + SEEK_HIT.w <= REMAIN_X);
const _: () = assert!(SEEK_RECT.x >= SEEK_HIT.x && SEEK_RECT.x + SEEK_RECT.w <= SEEK_HIT.x + SEEK_HIT.w);

// Transport
const TRANSPORT_CY: usize = 740;
const PLAY_CX: usize = 360;
const PLAY_R: usize = 44; // 88px circle
const PLAY_HIT: Rect = Rect::new(PLAY_CX - PLAY_R, TRANSPORT_CY - PLAY_R, PLAY_R * 2, PLAY_R * 2);

const PREV_CX: usize = 180;
const PREV_R: usize = 28;
const NEXT_CX: usize = 540;
const NEXT_R: usize = 28;
/// WHY the two side presses are the discs' own bounding boxes: each used to be
/// a 64x64 square stood four columns and four rows outside the 56x56 circle it
/// paints, so the ring of a press the user could not see restarted the track or
/// skipped it. `PLAY_HIT` below already answers its disc exactly; the pair now
/// follows the same derivation from the same centre and radius the painter is
/// handed, and neither disc needed enlarging -- both are already over the
/// platform's minimum target on both axes.
const PREV_HIT: Rect = Rect::new(PREV_CX - PREV_R, TRANSPORT_CY - PREV_R, PREV_R * 2, PREV_R * 2);
const NEXT_HIT: Rect = Rect::new(NEXT_CX - NEXT_R, TRANSPORT_CY - NEXT_R, NEXT_R * 2, NEXT_R * 2);
const _: () = assert!(PREV_HIT.w == PREV_R * 2 && PREV_HIT.h == PREV_R * 2);
const _: () = assert!(NEXT_HIT.w == NEXT_R * 2 && NEXT_HIT.h == NEXT_R * 2);
const _: () = assert!(
    PREV_HIT.w >= MIN_TOUCH_TARGET
        && NEXT_HIT.w >= MIN_TOUCH_TARGET
        && PLAY_HIT.w >= MIN_TOUCH_TARGET
        && PREV_HIT.h >= MIN_TOUCH_TARGET
);
const _: () = assert!(PREV_HIT.x + PREV_HIT.w <= PLAY_HIT.x && PLAY_HIT.x + PLAY_HIT.w <= NEXT_HIT.x);
const _: () = assert!(SEEK_HIT.y + SEEK_HIT.h <= PREV_HIT.y);
const _: () = assert!(PREV_HIT.y + PREV_HIT.h <= VOL_HIT.y);

// Volume HUD
const _VOL_CY: usize = 850;
const VOL_MUTE_BTN: Rect = Rect::new(24, 824, 52, 52);
const VOL_BAR_X: usize = 96;
const VOL_BAR_Y: usize = 846;
const VOL_BAR_W: usize = 490;
const VOL_BAR_H: usize = 8;
const VOL_BAR_RECT: Rect = Rect::new(VOL_BAR_X, VOL_BAR_Y, VOL_BAR_W, VOL_BAR_H);
const VOL_HIT: Rect = Rect::new(88, 824, 510, 52);
/// WHY checked at compile time: the mute plate and the bar's touch band share one
/// row and answer in that order, so the two must not overlap and the band must
/// hold the track it seeks. Both bands here are padded past the 8-row tracks
/// they answer on purpose -- the gates say the padding reaches the platform's
/// minimum, which is the only reason it is allowed at all.
const _: () = assert!(VOL_MUTE_BTN.w >= MIN_TOUCH_TARGET && VOL_MUTE_BTN.h >= MIN_TOUCH_TARGET);
const _: () = assert!(VOL_MUTE_BTN.x + VOL_MUTE_BTN.w <= VOL_HIT.x);
const _: () = assert!(VOL_MUTE_BTN.y == VOL_HIT.y && VOL_MUTE_BTN.h == VOL_HIT.h);
const _: () = assert!(
    VOL_BAR_RECT.y >= VOL_HIT.y && VOL_BAR_RECT.y + VOL_BAR_RECT.h <= VOL_HIT.y + VOL_HIT.h
);
const _: () = assert!(VOL_HIT.w >= MIN_TOUCH_TARGET && VOL_HIT.h >= MIN_TOUCH_TARGET);
const _: () = assert!(SEEK_HIT.w >= MIN_TOUCH_TARGET && SEEK_HIT.h >= MIN_TOUCH_TARGET);

// Secondary Row 1 (y: 900, h: 52)
const SPEED_BTN: Rect = Rect::new(24, 900, 154, 52);
const ASPECT_BTN: Rect = Rect::new(194, 900, 154, 52);
const LOOP_BTN: Rect = Rect::new(364, 900, 154, 52);
const PIP_BTN: Rect = Rect::new(534, 900, 162, 52);

// Secondary Row 2 (y: 968, h: 52)
const LIB_BTN: Rect = Rect::new(24, 968, 326, 52);
const INFO_BTN: Rect = Rect::new(366, 968, 330, 52);

/// WHY checked at compile time: these six are painted as plates from the same
/// literals the press answers, so the only way one of them can stop working is a
/// layout edit -- the chain says the rows stay edge-to-edge inside the gutters
/// and every one of them over the platform's minimum on both axes.
const _: () = assert!(SPEED_BTN.h >= MIN_TOUCH_TARGET && PIP_BTN.w >= MIN_TOUCH_TARGET);
const _: () = assert!(LIB_BTN.h >= MIN_TOUCH_TARGET && INFO_BTN.h >= MIN_TOUCH_TARGET);
const _: () = assert!(SPEED_BTN.x + SPEED_BTN.w <= ASPECT_BTN.x);
const _: () = assert!(ASPECT_BTN.x + ASPECT_BTN.w <= LOOP_BTN.x);
const _: () = assert!(LOOP_BTN.x + LOOP_BTN.w <= PIP_BTN.x);
const _: () = assert!(PIP_BTN.x + PIP_BTN.w <= W - 24);
const _: () = assert!(LIB_BTN.x + LIB_BTN.w <= INFO_BTN.x);
const _: () = assert!(INFO_BTN.x + INFO_BTN.w <= W - 24);

// PIP Floating Preview Card (Bottom Right, 280x158)
const PIP_W: usize = 280;
const PIP_H: usize = 158;
const PIP_X: usize = W - PIP_W - 24; // 416
const PIP_Y: usize = 1186;
const PIP_RECT: Rect = Rect::new(PIP_X, PIP_Y, PIP_W, PIP_H);

// Media Properties Sheet Card
const SHEET_RECT: Rect = Rect::new(24, 1036, 672, 340);

/// The sheet's label column: where its runs start and how wide they may be.
///
/// WHY the column has a bound at all: `Location:` is the longest label this
/// sheet authors and measures 86px on the body rung, and a label that is not
/// trimmed to its column walks into the value it names. The trim matters more
/// than the measurement suggests, because a run the outline face refuses is
/// re-rendered at the bitmap cell -- 32px a character, so the same nine bytes
/// would run to 288 and straight through the value beside them.
const SHEET_LABEL_X: usize = SHEET_RECT.x + 24;
const SHEET_LABEL_COL_W: usize = 96;
/// The value column's left edge, derived from the label column rather than
/// written beside it: the air between the two is what keeps a trimmed label and
/// its value from sharing a pixel.
const SHEET_VALUE_X: usize = SHEET_LABEL_X + SHEET_LABEL_COL_W + 20;
/// The widest a value may be: from its column to the sheet's own inset.
const SHEET_VALUE_MAX_W: usize = SHEET_RECT.x + SHEET_RECT.w - 24 - SHEET_VALUE_X;

/// The sheet's rows. WHY one pitch derived from the ladder: every label draws on
/// the body rung, so the row step is that line box and the air under it, and the
/// five rows the sheet carries are that step from the first.
const SHEET_HEADER_Y: usize = SHEET_RECT.y + 20;
/// The first row's offset inside the card, and the air a row wants under it.
const SHEET_ROW0_DY: usize = 60;
const SHEET_ROW_AIR: usize = 4;
/// The row step at `level`: the body rung's line box and the air under it --
/// or, when the card cannot hold five of those, the largest step that keeps the
/// last row's whole box inside the plate.
///
/// WHY a clamp and not an assert: the card's foot is bounded by the system home
/// band, not by its own text (`SHEET_RECT.y + SHEET_RECT.h <= HOME_HIT.y` below),
/// so at the largest setting the plate cannot simply grow. Tightening the
/// leading is the only move left that keeps every row's *ink* whole, and
/// [`sheet_rows_clear_at_every_level`] is the gate that proves the tightened
/// step still leaves more rows between two labels than a label's own ascent
/// takes -- so a rung costs this sheet air, never legibility, and never a
/// clipped value.
const fn sheet_row_step_at(level: u8) -> usize {
    let wanted = line_at(2, level) + SHEET_ROW_AIR;
    let room = (SHEET_RECT.h - SHEET_ROW0_DY - line_at(2, level)) / 4;
    if wanted <= room {
        wanted
    } else {
        room
    }
}

#[inline]
fn sheet_row_step() -> usize {
    sheet_row_step_at(level())
}

/// The row `i` (zero at the card's first row) starts at the published level.
#[inline]
fn sheet_row_y(i: usize) -> usize {
    SHEET_RECT.y + SHEET_ROW0_DY + i * sheet_row_step()
}

const _: () = assert!(sheet_row_step_at(PLATFORM_LEVEL) == font::line_height(2) + 4);

/// The offset a caption-rung value needs to stand on its body-rung label's
/// baseline.
///
/// WHY not a literal two or four: the two rungs' ascents differ by exactly this,
/// and a value authored at the label's box top rather than at the shared baseline
/// reads several rows high next to the word it answers.
const fn sheet_small_dy_at(level: u8) -> usize {
    ascent_at(2, level) - ascent_at(1, level)
}

#[inline]
fn sheet_small_dy() -> usize {
    sheet_small_dy_at(level())
}

const _: () = assert!(sheet_small_dy_at(PLATFORM_LEVEL) == 7);

/// WHAT THIS GATES: at every level the setting can hold, the sheet's five rows
/// keep their whole boxes inside the card, and two consecutive labels keep their
/// ink apart -- which is the pair of facts the clamp above trades in.
const fn sheet_rows_clear_at_every_level() -> bool {
    let mut level = 0u8;
    while level <= EXTRA_LARGE_LEVEL {
        let step = sheet_row_step_at(level);
        // The last row reserves its whole box inside the plate.
        if SHEET_ROW0_DY + 4 * step + line_at(2, level) > SHEET_RECT.h {
            return false;
        }
        // A label's ascent, its caption riding that same baseline, and the step
        // to the next row: the step has to exceed the ascent the row is drawn at,
        // or a row's ink would land on the row under it.
        if step <= ascent_at(2, level) + SHEET_ROW_AIR {
            return false;
        }
        level += 1;
    }
    true
}
const _: () = assert!(sheet_rows_clear_at_every_level());

/// WHY checked at compile time: the columns are the sheet's whole claim about
/// itself, and a label column that reached its value column, or a last row whose
/// box reached past the card, is the defect these rows replace.
const _: () = assert!(SHEET_LABEL_X + SHEET_LABEL_COL_W < SHEET_VALUE_X);
const _: () = assert!(SHEET_VALUE_X + SHEET_VALUE_MAX_W == SHEET_RECT.x + SHEET_RECT.w - 24);
/// The system home indicator, and the strip that answers a press on it.
///
/// Both come from `std::design::home_band` now. They used to be authored here as
/// three numbers: the painter inked a 200x6 `HOME_RECT` while the press answered
/// every column from row 1410 and the swipe answered every column from 1400, so
/// the mark and the box that quits were three different claims about one control.
const HOME_HIT: Rect = std::design::home_band::HIT;
const HOME_PILL: Rect = std::design::home_band::PILL;

/// WHY checked at compile time: the answering box is the platform's floor-sized
/// one only because nothing this app paints reaches into it -- the drawer's last
/// row and the info sheet's foot both end above its first.
const _: () = assert!(LIB_VIEW_Y1 <= HOME_HIT.y);
const _: () = assert!(SHEET_RECT.y + SHEET_RECT.h <= HOME_HIT.y);
const _: () = assert!(LAND_PANEL.y + LAND_PANEL.h <= HOME_HIT.y);
const _: () = assert!(HOME_PILL.y >= HOME_HIT.y && HOME_PILL.y + HOME_PILL.h <= H);
const _: () = assert!(HOME_HIT.x <= HOME_PILL.x && HOME_HIT.x + HOME_HIT.w >= HOME_PILL.x + HOME_PILL.w);

// Video Library Drawer / Sheet Layout
/// The drawer's list viewport, and the row it starts on.
///
/// WHY under the header's rule: the drawer's title and its line are painted by
/// the same band the list is tested against, so the list has to start below the
/// hairline that closes the band rather than under the title's line box.
/// The drawer's list viewport, and the row it starts on.
///
/// WHY under the header's rule: the drawer's title and its line are painted by
/// the same band the list is tested against, so the list has to start below the
/// hairline that closes the band rather than under the title's line box -- and
/// WHY a function: that hairline sits under two runs whose boxes follow the
/// person's text size, so a list authored at a constant row starts inside the
/// header it is supposed to sit below.
const fn lib_list_y0_at(level: u8) -> usize {
    header_rule_y_at(level) + 4
}

#[inline]
fn lib_list_y0() -> usize {
    lib_list_y0_at(level())
}

/// One playlist row, at `level`.
///
/// WHY the sum is written in ascents and not in the remembered 68: the row stacks
/// its title's ink, then its meta line's ink, and the meta run rides three rows
/// below the title's baseline -- the same riding the properties sheet's caption
/// uses. At the platform's rung that sum is `10 + 31 + 3 + 24`, which is the 68
/// this screen drew before the setting could be read, and the gate below pins it;
/// at a raised rung each part grows with the type, which is what keeps a scaled
/// row's meta line off the title of the row under it.
const LIB_ROW_TITLE_DY: usize = 10;
const LIB_ROW_META_RIDE: usize = 3;

const fn lib_meta_dy_at(level: u8) -> usize {
    LIB_ROW_TITLE_DY + ascent_at(2, level) + LIB_ROW_META_RIDE
}

const fn lib_row_h_at(level: u8) -> usize {
    lib_meta_dy_at(level) + ascent_at(1, level)
}

#[inline]
fn lib_meta_dy() -> usize {
    lib_meta_dy_at(level())
}

#[inline]
fn lib_row_h() -> usize {
    lib_row_h_at(level())
}

const LIB_ROW_H: usize = lib_row_h_at(PLATFORM_LEVEL);
const _: () = assert!(lib_meta_dy_at(PLATFORM_LEVEL) == 44);
const _: () = assert!(LIB_ROW_H == 68);
/// WHAT THIS GATES: a row reserves the whole of both its runs at every level, and
/// the first row still stands inside the viewport that answers a press on it.
const fn library_rows_clear_at_every_level() -> bool {
    let mut level = 0u8;
    while level <= EXTRA_LARGE_LEVEL {
        if lib_row_h_at(level) < lib_meta_dy_at(level) + ascent_at(1, level) {
            return false;
        }
        if lib_list_y0_at(level) + lib_row_h_at(level) > LIB_VIEW_Y1 {
            return false;
        }
        level += 1;
    }
    true
}
const _: () = assert!(library_rows_clear_at_every_level());
/// The bottom of the library list's viewport.
const LIB_VIEW_Y1: usize = 1390;

// -----------------------------------------------------------------------------
// Static Bounded Buffers (Zero Dynamic Allocations)
// -----------------------------------------------------------------------------

static mut CHUNK_BUF: [u32; CHUNK] = [0; CHUNK];
static mut CANVAS_PIXELS: [u32; CANVAS_W * CANVAS_H] = [0; CANVAS_W * CANVAS_H];
static mut FILE_SCRATCH: [u8; 64 * 1024] = [0; 64 * 1024];

/// Holds one AVI video frame payload. Bounded because the reader must be able
/// to refuse a frame, not grow to meet it.
static mut AVI_FRAME_BUF: [u8; avi::MAX_FRAME_BYTES] = [0; avi::MAX_FRAME_BYTES];

/// One decoded MJPEG frame at native screen resolution. The JPEG decoder
/// writes RGBA for the whole frame, so this is the largest frame this player
/// can decode; larger ones are reported, not scaled from a partial buffer.
static mut AVI_RGBA_BUF: [u8; 720 * 1440 * 4] = [0; 720 * 1440 * 4];

/// The kernel refuses an `FsRead` whose buffer is larger than 64 KiB.
const FS_READ_CHUNK: usize = 64 * 1024;

/// A capability-gated file as an AVI byte source.
///
/// WHY a wrapper and not a whole-file read: `fs_read` is sequential and capped
/// at 64 KiB per call, so the container is pulled through it as it is parsed.
struct FsSource {
    fd: u32,
}

impl FsSource {
    fn open(path: &str) -> Result<Self, AviError> {
        std::fs_open(path, std::FS_READ)
            .map(|fd| Self { fd })
            .map_err(|_| AviError::Io)
    }
}

impl avi::Source for FsSource {
    fn read(&mut self, out: &mut [u8]) -> Result<usize, AviError> {
        let cap = out.len().min(FS_READ_CHUNK);
        std::fs_read(self.fd, &mut out[..cap]).map_err(|_| AviError::Io)
    }
}

impl Drop for FsSource {
    fn drop(&mut self) {
        let _ = std::fs_close(self.fd);
    }
}

/// Playback state of one open AVI file.
pub struct AviPlayback {
    stream: AviStream<FsSource>,
    info: AviInfo,
    /// Absolute tick at which the next frame is due.
    deadline_ms: u64,
    /// Interval currently in force, already scaled by the speed selector.
    interval_ms: u32,
    /// Frames consumed, including those skipped to catch up.
    next_index: u32,
    /// Frames decoded and put on the canvas.
    shown: u32,
    /// Frames skipped because the clock was more than one interval ahead.
    skipped: u32,
    /// The last decoded frame, kept so an aspect-ratio change can rescale it
    /// without reopening the file and losing the position.
    last_w: u32,
    last_h: u32,
    last_rgba_len: usize,
    audio_cap: Option<u64>,
    pcm_conv: Option<interos_std::wav::PcmConverter>,
}

impl AviPlayback {
    fn elapsed_secs(&self) -> u32 {
        let interval = self.info.frame_interval_us() as u64;
        if interval == 0 {
            return 0;
        }
        let secs = self.next_index as u64 * interval / 1_000_000;
        secs.min(u32::MAX as u64) as u32
    }
}

/// What one pass over an open AVI changed.
pub struct AviTick {
    drawn: bool,
    ended: bool,
    failed: bool,
}

/// Scales a decoded RGBA frame into the app's ARGB canvas.
fn canvas_fill_from_rgba(aspect: AspectRatio, w: u32, h: u32, rgba: &[u8]) {
    if w == 0 || h == 0 {
        return;
    }
    let canvas = unsafe { &mut *core::ptr::addr_of_mut!(CANVAS_PIXELS) };
    for cy in 0..CANVAS_H {
        for cx in 0..CANVAS_W {
            canvas[cy * CANVAS_W + cx] =
                match map_canvas_to_source(cx, cy, w, h, aspect) {
                    Some((sx, sy)) => {
                        let idx = (sy * w as usize + sx) * 4;
                        if idx + 3 < rgba.len() {
                            0xFF000000
                                | ((rgba[idx] as u32) << 16)
                                | ((rgba[idx + 1] as u32) << 8)
                                | rgba[idx + 2] as u32
                        } else {
                            0xFF000000
                        }
                    }
                    // Letterbox / pillarbox bars.
                    None => 0xFF000000,
                };
        }
    }
}

/// Decodes one MJPEG payload with the platform image service and presents it.
fn show_avi_frame(payload: &[u8], aspect: AspectRatio) -> Result<(u32, u32, usize), ()> {
    let rgba = unsafe { &mut *core::ptr::addr_of_mut!(AVI_RGBA_BUF) };
    let (w, h, len) = std::image::decode_jpeg(payload, rgba)?;
    if len == 0 {
        return Err(());
    }
    canvas_fill_from_rgba(aspect, w, h, &rgba[..len]);
    Ok((w, h, len))
}

// -----------------------------------------------------------------------------
// Video Player Application State
// -----------------------------------------------------------------------------

pub struct VideoApp {
    pub state: PlaybackState,
    pub aspect: AspectRatio,
    pub loop_mode: LoopMode,
    pub speed: PlaybackSpeed,
    pub orientation: Orientation,
    pub status: DecoderStatus,
    pub playlist: Playlist,
    pub elapsed_sec: u32,
    pub duration_sec: u32,
    pub has_frame: bool,
    pub library_open: bool,
    pub info_open: bool,
    pub pip_mode: bool,
    pub hud_visible: bool,
    pub volume_percent: u32,
    pub is_muted: bool,
    pub saved_volume: u32,
    /// Whether the system actually reported its volume. Until it does, the
    /// bar shows an unknown state instead of inventing a level.
    pub volume_known: bool,
    pub speed_accum_ms: u32,
    pub library_scroll: usize,
    pub frame_counter: u32,
    /// The open AVI stream, when the loaded file is an AVI this player can
    /// actually decode. Dropping it closes the file descriptor.
    pub avi: Option<AviPlayback>,
    /// The token of the last dispatched intent this app handled.
    pub last_intent_token: u64,
}

impl VideoApp {
    pub const fn new() -> Self {
        Self {
            state: PlaybackState::Stopped,
            aspect: AspectRatio::Fit,
            loop_mode: LoopMode::None,
            speed: PlaybackSpeed::Speed1_0x,
            orientation: Orientation::Portrait,
            status: DecoderStatus::Idle,
            playlist: Playlist::new(),
            elapsed_sec: 0,
            duration_sec: 0,
            has_frame: false,
            library_open: false,
            info_open: false,
            pip_mode: false,
            hud_visible: true,
            volume_percent: 0,
            is_muted: false,
            saved_volume: 0,
            volume_known: false,
            speed_accum_ms: 0,
            library_scroll: 0,
            frame_counter: 0,
            avi: None,
            last_intent_token: 0,
        }
    }

    pub fn current_title(&self) -> &str {
        if let Some(item) = self.playlist.current_item() {
            item.title_str()
        } else {
            "Video Player"
        }
    }

    pub fn current_path(&self) -> &str {
        if let Some(item) = self.playlist.current_item() {
            item.path_str()
        } else {
            "/storage/videos/"
        }
    }

    pub fn current_codec(&self) -> VideoCodec {
        if let Some(item) = self.playlist.current_item() {
            item.codec
        } else {
            VideoCodec::Unknown
        }
    }

    pub fn remaining_sec(&self) -> u32 {
        self.duration_sec.saturating_sub(self.elapsed_sec)
    }

    /// Seeks to a keyframe position corresponding to `target_sec`.
    /// For MJPEG inside AVI, every frame is an intra/keyframe, so skipping to
    /// `target_frame` lands on a valid keyframe and displays it immediately.
    pub fn seek_to_sec(&mut self, target_sec: u32) {
        let clamped = target_sec.min(self.duration_sec);
        let (is_avi, path_buf, path_len) = if let Some(item) = self.playlist.current_item() {
            let mut buf = [0u8; video_player::PATH_CAP];
            let len = item.path_len.min(buf.len());
            buf[..len].copy_from_slice(&item.path[..len]);
            (item.codec == VideoCodec::Avi, buf, len)
        } else {
            self.elapsed_sec = clamped;
            return;
        };

        if is_avi {
            let path = core::str::from_utf8(&path_buf[..path_len]).unwrap_or("");
            if !path.is_empty() {
                self.open_avi(path);
                if let Some(pb) = self.avi.as_mut() {
                    let interval_us = pb.info.frame_interval_us();
                    let target_frame = if interval_us > 0 {
                        (clamped as u64 * 1_000_000 / interval_us as u64) as u32
                    } else {
                        0
                    };
                    let frame_buf = unsafe { &mut *core::ptr::addr_of_mut!(AVI_FRAME_BUF) };
                    for _ in 1..target_frame {
                        if pb.stream.next_frame(frame_buf).is_err() {
                            break;
                        }
                        pb.next_index = pb.next_index.saturating_add(1);
                    }
                    if target_frame > 0 {
                        if let Ok(Some(len)) = pb.stream.next_frame(frame_buf) {
                            pb.next_index = pb.next_index.saturating_add(1);
                            if let Ok((w, h, n)) = show_avi_frame(&frame_buf[..len], self.aspect) {
                                pb.last_w = w;
                                pb.last_h = h;
                                pb.last_rgba_len = n;
                                pb.shown = target_frame;
                                self.has_frame = true;
                            }
                        }
                    }
                    self.elapsed_sec = clamped;
                }
            }
            return;
        }
        self.elapsed_sec = clamped;
    }

    pub fn load_current_track(&mut self) {
        self.close_avi();
        let Some(item) = self.playlist.current_item() else {
            self.status = DecoderStatus::Idle;
            self.has_frame = false;
            self.duration_sec = 0;
            self.elapsed_sec = 0;
            return;
        };

        self.duration_sec = item.duration_sec;
        self.elapsed_sec = 0;
        self.frame_counter = 0;
        self.speed_accum_ms = 0;

        // Platform Truth Contract Enforcement:
        // If an H.264 / MP4 stream is opened and no hardware decoder is present,
        // truthfully report the exact contract string and refuse to fake pixels.
        if item.codec == VideoCodec::H264 {
            self.status = DecoderStatus::HardwareUnavailable(NO_HW_H264_MSG);
            self.has_frame = false;
            return;
        }

        let path = item.path_str();
        if path.is_empty() {
            self.status = DecoderStatus::Idle;
            self.has_frame = false;
            return;
        }

        // AVI is read as a stream, not as a whole file: the header decides the
        // size and frame rate, and frames are pulled one at a time. The path
        // is copied out first so opening the stream does not hold the
        // playlist entry borrowed.
        if item.codec == VideoCodec::Avi {
            let mut path_buf = [0u8; video_player::PATH_CAP];
            let len = item.path_len.min(path_buf.len());
            path_buf[..len].copy_from_slice(&item.path[..len]);
            let path = core::str::from_utf8(&path_buf[..len]).unwrap_or("");
            self.open_avi(path);
            return;
        }

        let read_bytes = unsafe {
            let buf = &mut *core::ptr::addr_of_mut!(FILE_SCRATCH);
            if let Ok(fd) = std::fs_open(path, std::FS_READ) {
                let n = std::fs_read(fd, buf).unwrap_or(0);
                let _ = std::fs_close(fd);
                n
            } else {
                0
            }
        };

        if read_bytes == 0 {
            self.status = DecoderStatus::Idle;
            self.has_frame = false;
            return;
        }

        let data = unsafe { &(&*core::ptr::addr_of!(FILE_SCRATCH))[..read_bytes] };

        match item.codec {
            VideoCodec::PpmP6 => {
                if let Ok((w, h, offset)) = video_player::parse_ppm_header(data) {
                    let pixel_count = (w as usize) * (h as usize);
                    let rgb_bytes = &data[offset..];
                    if rgb_bytes.len() >= pixel_count * 3 {
                        unsafe {
                            let canvas = &mut *core::ptr::addr_of_mut!(CANVAS_PIXELS);
                            for cy in 0..CANVAS_H {
                                for cx in 0..CANVAS_W {
                                    if let Some((sx, sy)) =
                                        map_canvas_to_source(cx, cy, w, h, self.aspect)
                                    {
                                        let idx = (sy * (w as usize) + sx) * 3;
                                        if idx + 2 < rgb_bytes.len() {
                                            let r = rgb_bytes[idx] as u32;
                                            let g = rgb_bytes[idx + 1] as u32;
                                            let b = rgb_bytes[idx + 2] as u32;
                                            canvas[cy * CANVAS_W + cx] =
                                                0xFF000000 | (r << 16) | (g << 8) | b;
                                        } else {
                                            canvas[cy * CANVAS_W + cx] = 0xFF000000;
                                        }
                                    } else {
                                        canvas[cy * CANVAS_W + cx] = 0xFF000000;
                                    }
                                }
                            }
                        }
                        self.has_frame = true;
                        self.status = DecoderStatus::Ready;
                        return;
                    }
                }
                self.status = DecoderStatus::CorruptData;
                self.has_frame = false;
            }
            VideoCodec::RawRgb | VideoCodec::RawRgba => {
                if let Ok((w, h, is_rgba, offset)) = video_player::parse_raw_header(data) {
                    let stride = if is_rgba { 4 } else { 3 };
                    let raw_bytes = &data[offset..];
                    unsafe {
                        let canvas = &mut *core::ptr::addr_of_mut!(CANVAS_PIXELS);
                        for cy in 0..CANVAS_H {
                            for cx in 0..CANVAS_W {
                                if let Some((sx, sy)) =
                                    map_canvas_to_source(cx, cy, w, h, self.aspect)
                                {
                                    let idx = (sy * (w as usize) + sx) * stride;
                                    if idx + 2 < raw_bytes.len() {
                                        let r = raw_bytes[idx] as u32;
                                        let g = raw_bytes[idx + 1] as u32;
                                        let b = raw_bytes[idx + 2] as u32;
                                        canvas[cy * CANVAS_W + cx] =
                                            0xFF000000 | (r << 16) | (g << 8) | b;
                                    } else {
                                        canvas[cy * CANVAS_W + cx] = 0xFF000000;
                                    }
                                } else {
                                    canvas[cy * CANVAS_W + cx] = 0xFF000000;
                                }
                            }
                        }
                    }
                    self.has_frame = true;
                    self.status = DecoderStatus::Ready;
                    return;
                }
                self.status = DecoderStatus::CorruptData;
                self.has_frame = false;
            }
            VideoCodec::Mjpeg => {
                if let Ok(info) = video_player::parse_mjpeg_frame(data, 0) {
                    if info.width > 0 && info.height > 0 {
                        self.status = DecoderStatus::Ready;
                        self.has_frame = true;
                        return;
                    }
                }
                self.status = DecoderStatus::CorruptData;
                self.has_frame = false;
            }
            _ => {
                self.status = DecoderStatus::Idle;
                self.has_frame = false;
            }
        }
    }

    /// Opens an AVI through the capability-gated filesystem and shows its
    /// first decoded frame. The transport is left as the caller set it.
    pub fn open_avi(&mut self, path: &str) {
        self.close_avi();
        self.has_frame = false;

        let source = match FsSource::open(path) {
            Ok(source) => source,
            Err(e) => {
                self.status = DecoderStatus::Error(e.message());
                std::log("[VIDEO PLAYER] AVI open failed at the filesystem boundary.").ok();
                return;
            }
        };
        let stream = match AviStream::open(source) {
            Ok(stream) => stream,
            Err(e) => {
                // Typed refusal: a malformed header or an undecodable codec is
                // reported as such, never drawn as a black but "playing" frame.
                self.status = DecoderStatus::Error(e.message());
                std::log("[VIDEO PLAYER] AVI refused: header or codec not decodable.").ok();
                return;
            }
        };

        let info = stream.info();
        if self.playlist.current < self.playlist.count {
            let item = &mut self.playlist.items[self.playlist.current];
            // The header is the only measured source for these values.
            item.width = info.width;
            item.height = info.height;
            item.duration_sec = info.duration_secs();
        }
        self.duration_sec = info.duration_secs();
        self.elapsed_sec = 0;
        self.frame_counter = 0;
        self.speed_accum_ms = 0;

        let mut audio_cap = None;
        let mut pcm_conv = None;
        if info.is_pcm_audio() {
            let wav_info = interos_std::wav::WavInfo {
                format_tag: info.audio_format_tag,
                sample_rate: info.audio_sample_rate,
                channels: info.audio_channels,
                bits_per_sample: info.audio_bits_per_sample,
                data_offset: 0,
                data_len: 0,
            };
            if let Ok(conv) = interos_std::wav::PcmConverter::new(wav_info) {
                pcm_conv = Some(conv);
            }
            if let Ok(cap) = std::capability_handle(std::cap_type::AUDIO_STREAM, 0) {
                if std::audio_stream_open(cap, std::AUDIO_STREAM_FORMAT_NATIVE).is_ok() {
                    audio_cap = Some(cap);
                }
            }
        }

        let now = std::get_ticks().unwrap_or(0);
        let interval = avi::scale_interval_millis(
            info.frame_interval_us(),
            self.speed.multiplier_milli(),
        )
        .max(1);
        self.avi = Some(AviPlayback {
            stream,
            info,
            deadline_ms: now,
            interval_ms: interval,
            next_index: 0,
            shown: 0,
            skipped: 0,
            last_w: 0,
            last_h: 0,
            last_rgba_len: 0,
            audio_cap,
            pcm_conv,
        });
        self.status = DecoderStatus::Ready;
        self.show_first_avi_frame();

        if let Some(pb) = self.avi.as_ref() {
            if pb.info.is_pcm_audio() {
                std::log("[VIDEO PLAYER] AVI has PCM audio stream; software audio decode active.").ok();
            } else if pb.info.audio_streams > 0 {
                // Stated in the log and on screen (media info + canvas notice).
                std::log("[VIDEO PLAYER] AVI has an audio stream; audio playback is unavailable and is not claimed.").ok();
            }
            std::log("[VIDEO PLAYER] AVI/MJPEG open: frames are pulled sequentially at the header's timing.").ok();
        }
    }

    /// Decodes and shows the first payload without advancing the clock, so a
    /// loaded file shows its own picture and Play resumes from a known frame.
    fn show_first_avi_frame(&mut self) {
        let aspect = self.aspect;
        let mut decoded: Option<(u32, u32, usize)> = None;
        let mut ended = false;
        let mut error: Option<&'static str> = None;

        if let Some(pb) = self.avi.as_mut() {
            let frame_buf = unsafe { &mut *core::ptr::addr_of_mut!(AVI_FRAME_BUF) };
            for _ in 0..4 {
                match pb.stream.next_frame(frame_buf) {
                    Ok(Some(len)) => {
                        pb.next_index = pb.next_index.saturating_add(1);
                        if let Ok(frame) = show_avi_frame(&frame_buf[..len], aspect) {
                            decoded = Some(frame);
                            break;
                        }
                        std::log("[VIDEO PLAYER] An MJPEG payload was refused by the decoder; trying the next one.").ok();
                    }
                    Ok(None) => {
                        ended = true;
                        break;
                    }
                    Err(AviError::FrameTooLarge) => {
                        pb.next_index = pb.next_index.saturating_add(1);
                        pb.skipped = pb.skipped.saturating_add(1);
                    }
                    Err(e) => {
                        error = Some(e.message());
                        break;
                    }
                }
            }
            if decoded.is_none() && !ended && error.is_none() {
                error = Some("AVI: no frame could be decoded.");
            }
        }

        if let Some((w, h, n)) = decoded {
            if let Some(pb) = self.avi.as_mut() {
                pb.last_w = w;
                pb.last_h = h;
                pb.last_rgba_len = n;
                pb.shown = 1;
            }
            self.has_frame = true;
            self.frame_counter = 1;
        }
        if ended {
            self.close_avi();
            self.has_frame = false;
            self.elapsed_sec = self.duration_sec;
            self.state = PlaybackState::Stopped;
            std::log("[VIDEO PLAYER] AVI has no decodable video frames.").ok();
        }
        if let Some(msg) = error {
            self.close_avi();
            self.has_frame = false;
            self.status = DecoderStatus::Error(msg);
        }
    }

    /// Advances the open AVI once per timer wake-up.
    ///
    /// WHY frames are skipped instead of decoding every one: the decoder and
    /// the compositor can be slower than the file's frame interval. Decoding
    /// every payload would play in slow motion, so a frame whose deadline is
    /// already more than one interval in the past is consumed and dropped.
    pub fn tick_avi(&mut self) -> AviTick {
        let now = std::get_ticks().unwrap_or(0);
        let aspect = self.aspect;
        let speed_milli = self.speed.multiplier_milli();
        let mut drawn = false;
        let mut ended = false;
        let mut error: Option<&'static str> = None;
        let elapsed: u32;

        {
            let Some(pb) = self.avi.as_mut() else {
                return AviTick { drawn: false, ended: false, failed: false };
            };
            let interval =
                avi::scale_interval_millis(pb.info.frame_interval_us(), speed_milli).max(1);
            pb.interval_ms = interval;
            let frame_buf = unsafe { &mut *core::ptr::addr_of_mut!(AVI_FRAME_BUF) };
            let mut guard = 0;
            loop {
                if now < pb.deadline_ms {
                    break;
                }
                let behind = now - pb.deadline_ms;
                let skip = behind >= interval as u64 && pb.shown > 0;
                match pb.stream.next_chunk(frame_buf) {
                    Ok(Some(AviChunk::Audio(len))) => {
                        if let (Some(cap), Some(conv)) = (pb.audio_cap, pb.pcm_conv.as_mut()) {
                            let mut native_pcm = [0u8; 4096];
                            let (_, out_n) = conv.convert(&frame_buf[..len], &mut native_pcm);
                            if out_n > 0 {
                                let _ = std::audio_stream_write(cap, &native_pcm[..out_n]);
                            }
                        }
                        guard += 1;
                        if guard >= 64 {
                            break;
                        }
                        continue;
                    }
                    Ok(Some(AviChunk::Video(len))) => {
                        pb.next_index = pb.next_index.saturating_add(1);
                        pb.deadline_ms = pb.deadline_ms.saturating_add(interval as u64);
                        if skip {
                            pb.skipped = pb.skipped.saturating_add(1);
                        } else {
                            match show_avi_frame(&frame_buf[..len], aspect) {
                                Ok((w, h, n)) => {
                                    pb.last_w = w;
                                    pb.last_h = h;
                                    pb.last_rgba_len = n;
                                    pb.shown = pb.shown.saturating_add(1);
                                    drawn = true;
                                }
                                Err(_) => {
                                    error = Some("AVI: MJPEG frame could not be decoded.");
                                }
                            }
                        }
                    }
                    Ok(None) => {
                        ended = true;
                        break;
                    }
                    Err(AviError::FrameTooLarge) => {
                        pb.next_index = pb.next_index.saturating_add(1);
                        pb.deadline_ms = pb.deadline_ms.saturating_add(interval as u64);
                        pb.skipped = pb.skipped.saturating_add(1);
                    }
                    Err(e) => {
                        error = Some(e.message());
                        break;
                    }
                }
                guard += 1;
                if drawn || ended || error.is_some() || guard >= 32 {
                    break;
                }
            }
            elapsed = pb.elapsed_secs();
        }

        self.elapsed_sec = elapsed;
        if drawn {
            self.has_frame = true;
            self.status = DecoderStatus::Ready;
            self.frame_counter = self.frame_counter.saturating_add(1);
        }
        if error.is_some() {
            self.close_avi();
            self.has_frame = false;
            self.state = PlaybackState::Stopped;
            if let Some(msg) = error {
                self.status = DecoderStatus::Error(msg);
            }
            std::log("[VIDEO PLAYER] AVI playback stopped on a reported frame/read error.").ok();
        }
        if ended {
            self.finish_avi();
        }
        AviTick { drawn, ended, failed: error.is_some() }
    }

    /// Called at end of stream: honours the loop mode, or stops.
    fn finish_avi(&mut self) {
        self.close_avi();
        self.speed_accum_ms = 0;
        if self.duration_sec > 0 {
            self.elapsed_sec = self.duration_sec;
        }
        std::log("[VIDEO PLAYER] AVI end of stream; the file was closed.").ok();
        match self.loop_mode {
            LoopMode::LoopOne => {
                self.load_current_track();
                self.state = if self.avi.is_some() {
                    PlaybackState::Playing
                } else {
                    PlaybackState::Stopped
                };
            }
            LoopMode::LoopAll => {
                if self.playlist.next(LoopMode::LoopAll).is_some() {
                    self.load_current_track();
                    self.state = if self.avi.is_some() {
                        PlaybackState::Playing
                    } else {
                        PlaybackState::Stopped
                    };
                } else {
                    self.state = PlaybackState::Stopped;
                }
            }
            LoopMode::None => {
                self.state = PlaybackState::Stopped;
            }
        }
    }

    /// Adopts a new speed without replaying the frames the old rate covered.
    pub fn retime_avi(&mut self) {
        if let Some(pb) = self.avi.as_mut() {
            pb.interval_ms = avi::scale_interval_millis(
                pb.info.frame_interval_us(),
                self.speed.multiplier_milli(),
            )
            .max(1);
            pb.deadline_ms = std::get_ticks()
                .unwrap_or(0)
                .saturating_add(pb.interval_ms as u64);
        }
    }

    /// Rescales the frame already decoded, e.g. after an aspect-ratio change.
    pub fn rescale_current_avi_frame(&mut self) {
        let Some(pb) = self.avi.as_ref() else {
            return;
        };
        if pb.last_rgba_len == 0 {
            return;
        }
        let (w, h, n, aspect) = (pb.last_w, pb.last_h, pb.last_rgba_len, self.aspect);
        let rgba = unsafe { &*core::ptr::addr_of!(AVI_RGBA_BUF) };
        canvas_fill_from_rgba(aspect, w, h, &rgba[..n]);
        self.has_frame = true;
    }

    pub fn close_avi(&mut self) {
        // Dropping the playback closes the descriptor through FsSource::drop and audio stream.
        if let Some(pb) = self.avi.take() {
            if let Some(cap) = pb.audio_cap {
                let _ = std::audio_stream_close(cap);
            }
        }
    }

    /// True when the open AVI declares an uncompressed PCM audio stream.
    pub fn avi_audio_pcm(&self) -> bool {
        self.avi
            .as_ref()
            .map_or(false, |pb| pb.info.is_pcm_audio())
    }

    /// True when the open AVI declares an audio stream that cannot be decoded.
    /// The UI says so rather than showing a mute control as if
    /// it were driving that audio.
    pub fn avi_audio_unavailable(&self) -> bool {
        self.avi
            .as_ref()
            .map_or(false, |pb| pb.info.audio_streams > 0 && !pb.info.is_pcm_audio())
    }

    pub fn rescan_storage(&mut self) {
        let mut list_buf = [0u8; 4096];

        // 1. Scan /storage/videos
        if let Ok(bytes) = std::fs_list("/storage/videos", &mut list_buf) {
            let _ = video_player::parse_directory_listing(
                b"/storage/videos",
                &list_buf[..bytes],
                &mut self.playlist,
            );
        }

        // 2. Scan /storage/downloads
        if let Ok(bytes) = std::fs_list("/storage/downloads", &mut list_buf) {
            let _ = video_player::parse_directory_listing(
                b"/storage/downloads",
                &list_buf[..bytes],
                &mut self.playlist,
            );
        }

        // Probe dimensions from headers
        for i in 0..self.playlist.count {
            let item = &mut self.playlist.items[i];
            if item.codec == VideoCodec::H264 {
                // No box parser reads this frame size or running time yet,
                // so both stay unread rather than being filled with a
                // plausible-looking 1080p/3m35s.
                continue;
            }
            let path_str = item.path_str();
            if item.codec == VideoCodec::Avi {
                // The container header must be walked, not sniffed from 64
                // bytes; dimensions and running time come from `strf`/`avih`.
                if let Ok(source) = FsSource::open(path_str) {
                    if let Ok(stream) = AviStream::open(source) {
                        let info = stream.info();
                        item.width = info.width;
                        item.height = info.height;
                        item.duration_sec = info.duration_secs();
                    }
                }
                continue;
            }
            if let Ok(fd) = std::fs_open(path_str, std::FS_READ) {
                let mut hdr = [0u8; 64];
                if let Ok(n) = std::fs_read(fd, &mut hdr) {
                    if n > 0 {
                        match item.codec {
                            VideoCodec::PpmP6 => {
                                if let Ok((w, h, _)) = video_player::parse_ppm_header(&hdr[..n]) {
                                    item.width = w;
                                    item.height = h;
                                }
                            }
                            VideoCodec::RawRgb | VideoCodec::RawRgba => {
                                if let Ok((w, h, _, _)) = video_player::parse_raw_header(&hdr[..n])
                                {
                                    item.width = w;
                                    item.height = h;
                                }
                            }
                            VideoCodec::Mjpeg => {
                                if let Ok(info) = video_player::parse_mjpeg_frame(&hdr[..n], 0) {
                                    item.width = info.width;
                                    item.height = info.height;
                                }
                            }
                            _ => {}
                        }
                    }
                }
                let _ = std::fs_close(fd);
            }
        }
    }

    /// Opens the video file a caller dispatched to this player.
    ///
    /// WHY the storage is rescanned first: the payload is a document in the
    /// shared storage area, and the playlist is the app's list of the files
    /// that area holds. Scanning is how a document becomes an entry that can
    /// be selected.
    pub fn handle_pending_intent(&mut self) -> bool {
        let mut buf = [0u8; 320];
        let n = match std::content::intent_pending(&mut buf) {
            Ok(n) => n,
            Err(_) => return false,
        };
        if n == 0 {
            return false;
        }
        let Some(pending) = std::content::parse_pending(&buf[..n]) else {
            return false;
        };
        // WHY the token is remembered: the pending slot is peeked, not taken,
        // so it stays readable until another dispatch replaces it. Without
        // this the player would reload the same file on every pass.
        if pending.token == self.last_intent_token {
            return false;
        }
        self.last_intent_token = pending.token;
        if pending.kind != std::content::IntentKind::Mime {
            return false;
        }
        let payload = pending.payload;
        let name = payload
            .as_bytes()
            .rsplit(|b| *b == b'/')
            .next()
            .unwrap_or(b"");
        if name.is_empty() || !video_player::has_video_suffix(name) {
            std::log("[VIDEO PLAYER] A dispatched document is not a video this player can list.").ok();
            return false;
        }
        self.rescan_storage();
        for i in 0..self.playlist.count {
            if self.playlist.items[i].path_str() == payload {
                self.playlist.select(i);
                self.load_current_track();
                std::log("[VIDEO PLAYER] Opened the video file a caller dispatched to this player.").ok();
                return true;
            }
        }
        std::log("[VIDEO PLAYER] The dispatched video file was not listed in the shared storage.").ok();
        false
    }
}

/// In `out`, the running time a known duration names, or the unread marker.
///
/// WHY the marker exists: a duration of zero is what a header nobody has read
/// yields, and drawing "0:00" for it would claim the media is empty instead of
/// reporting that its length is unknown.
fn duration_label(known: bool, seconds: u32, negative: bool, out: &mut [u8; 12]) -> usize {
    if !known {
        out[..5].copy_from_slice(b"--:--");
        return 5;
    }
    format_time(seconds, negative, out)
}

// -----------------------------------------------------------------------------
// Pixel Shaders & Typography
// -----------------------------------------------------------------------------

/// One run's ink, at the level `level` names.
///
/// WHY the box, the placement and the fallback all move together: the box is the
/// line box the layout reserved, the placement is the cap the run is rasterised
/// at, and the fallback is the size the bitmap face draws when the outline face
/// refuses the run. Moving one and not the others is the clipped-text class of
/// defect -- a scaled cap inside the platform's box loses its descenders, and a
/// platform cap inside a scaled box leaves a gap where the line used to be. At
/// [`PLATFORM_LEVEL`] all three answer exactly what this function answered before
/// the setting could be read: the run box, `line_height` and `measure` are the
/// ladder's own numbers there, which the gate beside [`line_at`] pins.
///
/// WHY the width band is wider at a scaled level: `16 * scale` is the bitmap
/// cell's widest byte, and a run at a scaled em walks past it, so the band would
/// otherwise reject pixels of the very run it enlarged. Every caller fits its run
/// to its own column with [`fit_run`] before asking, so widening the band here
/// cannot let ink reach past a column -- it only stops the cell grid from cutting
/// a run the outline face draws wider than the grid.
#[inline(always)]
fn draw_text_at(
    x: usize,
    y: usize,
    x0: usize,
    y0: usize,
    scale: usize,
    s: &[u8],
    color: u32,
    bg: u32,
    bold: bool,
    level: u8,
) -> Option<u32> {
    if scale == 0 || s.is_empty() {
        return None;
    }
    if level == PLATFORM_LEVEL {
        let (bx, bt, bw, bh) = outline_text::run_box(x0, y0, scale, s);
        let bitmap_h = line_at(scale, PLATFORM_LEVEL);
        let bitmap_w = if bold {
            font::measure_bold(s, scale)
        } else {
            font::measure(s, scale)
        };
        let top = bt.min(y0);
        let height = bh.max(bitmap_h);
        let width = bw.max(bitmap_w) + usize::from(bold);
        if x < bx || x - bx >= width || y < top || y - top >= height {
            return None;
        }
        if let Some(run) = outline_text::place_run(x0, y0, scale, s, None, bold) {
            return run.pixel(x, y, color, bg);
        }
        return font::test_text_aa(x, y, x0, y0, scale, s, color, bg, bold);
    }
    let h = line_at(scale, level);
    let width = (run_w_at(s, scale, bold, level) + 2)
        .min(s.len().saturating_mul(em_at(scale, level) as usize + 4) + 2);
    if x < x0 || x - x0 >= width || y < y0 || y - y0 >= h {
        return None;
    }
    let cap = cap_at(scale, level);
    if let Some(run) = outline_text::place_run_cap(x0, y0, cap, s, None, bold) {
        return run.pixel(x, y, color, bg);
    }
    std::design::monochrome::text_px(x, y, x0, y0, cap, s, color, bg, bold)
}

/// [`draw_text_at`] at the published level.
#[inline(always)]
fn draw_text(
    x: usize,
    y: usize,
    x0: usize,
    y0: usize,
    scale: usize,
    s: &[u8],
    color: u32,
    bg: u32,
) -> Option<u32> {
    draw_text_at(x, y, x0, y0, scale, s, color, bg, false, level())
}

/// [`draw_text_at`] at the published level, in the bold weight.
#[inline(always)]
fn draw_text_bold(
    x: usize,
    y: usize,
    x0: usize,
    y0: usize,
    scale: usize,
    s: &[u8],
    color: u32,
    bg: u32,
) -> Option<u32> {
    draw_text_at(x, y, x0, y0, scale, s, color, bg, true, level())
}

/// The properties sheet's label column: a body-rung run, trimmed to its column
/// before it is asked.
///
/// WHY the trim happens here rather than at each call: the bound belongs to the
/// column, not to the string, and a literal label is only short for as long as
/// nobody renames it.
#[inline(always)]
fn sheet_label(x: usize, y: usize, row_y: usize, s: &[u8]) -> Option<u32> {
    let mut buf = [0u8; 24];
    let n = fit_run(s, 2, false, SHEET_LABEL_COL_W, &mut buf);
    draw_text(
        x,
        y,
        SHEET_LABEL_X,
        row_y,
        2,
        &buf[..n],
        COLOR_TEXT_MUTED,
        COLOR_SHEET_CARD,
    )
}

/// A value on the body rung, sharing its row's box top with the label it answers.
#[inline(always)]
fn sheet_value_bold(x: usize, y: usize, row_y: usize, s: &[u8]) -> Option<u32> {
    let mut buf = [0u8; 32];
    let n = fit_run(s, 2, true, SHEET_VALUE_MAX_W, &mut buf);
    draw_text_bold(
        x,
        y,
        SHEET_VALUE_X,
        row_y,
        2,
        &buf[..n],
        COLOR_TEXT_PRIMARY,
        COLOR_SHEET_CARD,
    )
}

/// A value on the caption rung, stood on its label's baseline rather than on the
/// label's box top.
#[inline(always)]
fn sheet_value_caption(x: usize, y: usize, row_y: usize, s: &[u8]) -> Option<u32> {
    let mut buf = [0u8; 48];
    let n = fit_run(s, 1, false, SHEET_VALUE_MAX_W, &mut buf);
    draw_text(
        x,
        y,
        SHEET_VALUE_X,
        row_y + sheet_small_dy(),
        1,
        &buf[..n],
        COLOR_TEXT_PRIMARY,
        COLOR_SHEET_CARD,
    )
}

// -----------------------------------------------------------------------------
// Pixel Generator across Entire Surface
// -----------------------------------------------------------------------------

fn get_video_pixel(x: usize, y: usize, app: &VideoApp) -> u32 {
    // WHY ahead of every mode below: the fullscreen video and the open drawer
    // both answer a press in this box, so both have to wear its mark.
    if let Some(ink) = std::design::home_band::pixel(x, y, COLOR_TEXT_MUTED) {
        return ink;
    }

    // =========================================================================
    // 1. LANDSCAPE MODE (Fullscreen 720x1440 with Translucent On-Screen HUD)
    // =========================================================================
    if app.orientation == Orientation::Landscape {
        // Base: Fullscreen video rendering
        let video_pixel: u32 = if let DecoderStatus::HardwareUnavailable(msg) = app.status {
            let mw = run_w(msg.as_bytes(), 1, true);
            let mx = (W.saturating_sub(mw)) / 2;
            if let Some(col) = draw_text_bold(
                x,
                y,
                mx,
                H / 2 - 10,
                1,
                msg.as_bytes(),
                0xFFE05A68,
                COLOR_SURFACE_SUNKEN,
            ) {
                return col;
            }
            COLOR_SURFACE_SUNKEN
        } else if let DecoderStatus::Error(msg) = app.status {
            let mw = run_w(msg.as_bytes(), 1, true);
            let mx = (W.saturating_sub(mw)) / 2;
            if let Some(col) = draw_text_bold(
                x,
                y,
                mx,
                H / 2 - 10,
                1,
                msg.as_bytes(),
                0xFFE05A68,
                COLOR_SURFACE_SUNKEN,
            ) {
                return col;
            }
            COLOR_SURFACE_SUNKEN
        } else if app.has_frame {
            let src_w = app.playlist.current_item().map_or(680, |it| it.width.max(1));
            let src_h = app.playlist.current_item().map_or(382, |it| it.height.max(1));
            if let Some((sx, sy)) = map_coords_to_source(x, y, W, H, src_w, src_h, app.aspect) {
                let cx = (sx * CANVAS_W) / (src_w as usize).max(1);
                let cy = (sy * CANVAS_H) / (src_h as usize).max(1);
                let idx = cy.min(CANVAS_H - 1) * CANVAS_W + cx.min(CANVAS_W - 1);
                unsafe { (*core::ptr::addr_of!(CANVAS_PIXELS))[idx] }
            } else {
                0xFF000000 // Pillarbox / Letterbox black
            }
        } else {
            // Idle state in landscape
            let msg = b"NO MEDIA OPEN";
            let mw = run_w(msg, 2, true);
            let mx = (W.saturating_sub(mw)) / 2;
            if let Some(col) = draw_text_bold(x, y, mx, H / 2 - 20, 2, msg, COLOR_TEXT_PRIMARY, COLOR_SURFACE) {
                return col;
            }
            COLOR_SURFACE
        };

        // Translucent HUD Overlays
        if app.hud_visible {
            // Top Translucent Bar
            let top_bar = land_bar();
            if top_bar.contains(x, y) {
                let bg_hud = blend_argb(video_pixel, COLOR_SURFACE_SUNKEN, 210);

                // The back control: the platform chip, pressed here returns the
                // player to Portrait.
                //
                // WHY the subtraction: `back_button::pixel` only answers inside
                // its own rect, and this copy stands at `LAND_BACK_BTN`'s origin.
                // Shifting the point by the difference of the two origins hands
                // the SDK the very pixel it was drawn for, and `LAND_BACK_BTN` is
                // the chip's own 48x48, so the shift is exact rather than an
                // approximation. The plate is then the same shape, ring and
                // chevron as the one portrait puts in the corner.
                //
                // WHY the scrim still shows: the SDK returns None on the four
                // corner cut-outs of the rounded rect, and this falls through to
                // `bg_hud` there, which is what a rounded plate over a live frame
                // should look like.
                let back_hit = land_back_btn();
                if back_hit.contains(x, y) {
                    let (ink, ring, fill) = back_button::colors_for_theme(&THEME);
                    let lx = x - back_hit.x + back_button::X;
                    let ly = y - back_hit.y + back_button::Y;
                    if let Some(c) = back_button::pixel(lx, ly, ink, ring, fill) {
                        return c;
                    }
                    return bg_hud;
                }

                // Title (Scale 2 Body Bold)
                let title = app.current_title().as_bytes();
                let mut tbuf = [0u8; 48];
                let tn = fit_run(title, 2, true, 420, &mut tbuf);
                if let Some(col) = draw_text_bold(x, y, 104, land_title_y(), 2, &tbuf[..tn], COLOR_TEXT_PRIMARY, bg_hud) {
                    return col;
                }

                // Codec & Speed Badge
                let c_label = app.current_codec().label().as_bytes();
                let s_label = app.speed.label().as_bytes();
                let mut badge_buf = [0u8; 32];
                let mut bl = 0;
                badge_buf[bl..bl + c_label.len()].copy_from_slice(c_label);
                bl += c_label.len();
                badge_buf[bl..bl + 3].copy_from_slice(b" - ");
                bl += 3;
                badge_buf[bl..bl + s_label.len()].copy_from_slice(s_label);
                bl += s_label.len();
                if let Some(col) = draw_text(x, y, 104, land_badge_y(), 1, &badge_buf[..bl], COLOR_TEXT_MUTED, bg_hud) {
                    return col;
                }

                return bg_hud;
            }

            // Bottom Translucent Controls Panel -- `LAND_PANEL`, the bound the
            // top bar's foot is gated against above.
            let bot_panel = LAND_PANEL;
            if bot_panel.contains(x, y) {
                let bg_hud = blend_argb(video_pixel, COLOR_SURFACE_SUNKEN, 220);

                // Scrubber: Timestamps & Bar
                let mut ebuf = [0u8; 12];
                let elen = format_time(app.elapsed_sec, false, &mut ebuf);
                if let Some(col) = draw_text(x, y, 40, 1120, 2, &ebuf[..elen], COLOR_TEXT_PRIMARY, bg_hud) {
                    return col;
                }

                let mut rbuf = [0u8; 12];
                let rlen = duration_label(
                    app.duration_sec > 0,
                    app.remaining_sec(),
                    true,
                    &mut rbuf,
                );
                if let Some(col) = draw_text(x, y, LAND_REMAIN_X, 1120, 2, &rbuf[..rlen], COLOR_TEXT_MUTED, bg_hud) {
                    return col;
                }

                let l_seek_rect = LAND_SEEK;
                let l_thumb_x =
                    scrubber_thumb_x(LAND_SEEK_X, LAND_SEEK_W, app.elapsed_sec, app.duration_sec);
                let dx = (x as i32) - (l_thumb_x as i32);
                let dy = (y as i32) - ((LAND_SEEK_Y + LAND_SEEK_H / 2) as i32);
                if dx * dx + dy * dy <= 64 {
                    return COLOR_TEXT_PRIMARY;
                }
                if l_seek_rect.contains(x, y) {
                    return if x <= l_thumb_x { COLOR_PROGRESS_ACTIVE } else { COLOR_PROGRESS_BG };
                }

                // Transport: Play/Pause, and the two track controls beside it.
                // Each disc is drawn from the same centre and radius the press
                // tests, so a tap answers the ink and nothing beyond it.
                let t_cy = LAND_TRANSPORT_CY;
                if let Some(col) = draw_circle(
                    x,
                    y,
                    LAND_PLAY_CX,
                    t_cy,
                    LAND_PLAY_R,
                    COLOR_PLAY_FILL,
                    None,
                ) {
                    let pdx = (x as i32) - (LAND_PLAY_CX as i32);
                    let pdy = (y as i32) - (t_cy as i32);
                    if app.state == PlaybackState::Playing {
                        if (pdx >= -8 && pdx <= -3 && pdy.abs() <= 12) || (pdx >= 3 && pdx <= 8 && pdy.abs() <= 12) {
                            return COLOR_PLAY_GLYPH;
                        }
                    } else if pdx >= -6 && pdx <= 12 {
                        let hh = (12 - pdx) * 12 / 18;
                        if pdy.abs() <= hh {
                            return COLOR_PLAY_GLYPH;
                        }
                    }
                    return col;
                }

                if let Some(col) = draw_circle(
                    x,
                    y,
                    LAND_STEP_CX_LEFT,
                    t_cy,
                    LAND_STEP_R,
                    COLOR_SURFACE,
                    Some(COLOR_BORDER),
                ) {
                    let pdx = (x as i32) - (LAND_STEP_CX_LEFT as i32);
                    let pdy = (y as i32) - (t_cy as i32);
                    if pdx >= -8 && pdx <= -6 && pdy.abs() <= 8 {
                        return COLOR_TEXT_PRIMARY;
                    }
                    if pdx > -6 && pdx <= 6 && pdy.abs() <= (pdx + 6) * 8 / 12 {
                        return COLOR_TEXT_PRIMARY;
                    }
                    return col;
                }
                if let Some(col) = draw_circle(
                    x,
                    y,
                    LAND_STEP_CX_RIGHT,
                    t_cy,
                    LAND_STEP_R,
                    COLOR_SURFACE,
                    Some(COLOR_BORDER),
                ) {
                    let pdx = (x as i32) - (LAND_STEP_CX_RIGHT as i32);
                    let pdy = (y as i32) - (t_cy as i32);
                    if pdx >= -6 && pdx < 6 && pdy.abs() <= (6 - pdx) * 8 / 12 {
                        return COLOR_TEXT_PRIMARY;
                    }
                    if pdx >= 6 && pdx <= 8 && pdy.abs() <= 8 {
                        return COLOR_TEXT_PRIMARY;
                    }
                    return col;
                }

                // Volume Bar in Landscape
                let l_vol_thumb = volume_thumb_x(
                    LAND_VOL_X,
                    LAND_VOL_W,
                    if app.is_muted || !app.volume_known { 0 } else { app.volume_percent },
                );
                let vdx = (x as i32) - (l_vol_thumb as i32);
                let vdy = (y as i32) - ((LAND_VOL_Y + LAND_VOL_H / 2) as i32);
                if vdx * vdx + vdy * vdy <= 36 {
                    return COLOR_TEXT_PRIMARY;
                }
                if LAND_VOL.contains(x, y) {
                    return if x <= l_vol_thumb { COLOR_PROGRESS_ACTIVE } else { COLOR_PROGRESS_BG };
                }

                // Mute plate, with the speaker centred on it -- the same control
                // the press answers, and the same state ink the portrait HUD uses
                // so a press that muted the output visibly says so.
                if let Some(col) =
                    draw_rounded_rect(x, y, LAND_MUTE_BTN, 12, COLOR_SURFACE, Some(COLOR_BORDER), 1)
                {
                    let spk_dx = (x as i32) - (LAND_MUTE_CX as i32);
                    let spk_dy = (y as i32) - (LAND_MUTE_CY as i32);
                    if spk_dx >= -8 && spk_dx <= -3 && spk_dy.abs() <= 5 {
                        return COLOR_TEXT_PRIMARY;
                    }
                    if spk_dx > -3 && spk_dx <= 5 && spk_dy.abs() <= (spk_dx + 3) * 8 / 8 {
                        return COLOR_TEXT_PRIMARY;
                    }
                    if app.is_muted || !app.volume_known {
                        if spk_dx >= 6 && spk_dx <= 14 && spk_dy.abs() <= 6
                            && (((spk_dx - 10) - spk_dy).abs() <= 1 || ((spk_dx - 10) + spk_dy).abs() <= 1)
                        {
                            return 0xFFE05A68;
                        }
                    } else if spk_dx > 4 {
                        let d_arc = spk_dx * spk_dx + spk_dy * spk_dy;
                        if d_arc >= 64 && d_arc <= 100 {
                            return COLOR_TEXT_PRIMARY;
                        }
                    }
                    return col;
                }

                return bg_hud;
            }
        }

        return video_pixel;
    }

    // =========================================================================
    // 2. VIDEO LIBRARY & PLAYLIST DRAWER / SHEET
    // =========================================================================
    if app.library_open || (app.pip_mode && !PIP_RECT.contains(x, y)) {
        // Drawer Top Bar (y: 0..LIB_LIST_Y0)
        if y < lib_list_y0() {
            // The back control, asked first because it is the one occupant of
            // this row the app does not own: its rect, radius, ring and chevron
            // all come from `back_button`, and pressing it closes the drawer --
            // or leaves picture-in-picture -- exactly as the ✕ it replaced did.
            // The drawer's title starts at column 96, 24 clear of the chip's
            // right edge at 72, so the two never share a pixel.
            if let Some(c) = back_button::pixel_for_theme(x, y, &THEME) {
                return c;
            }

            // Header Title: Scale 2 Body Bold
            let lib_title: &[u8] = if app.pip_mode {
                b"VIDEO LIBRARY (PIP ACTIVE)"
            } else {
                b"VIDEO LIBRARY"
            };
            if let Some(col) = draw_text_bold(
                x,
                y,
                96,
                HEADER_TITLE_Y,
                2,
                lib_title,
                COLOR_TEXT_PRIMARY,
                COLOR_BG,
            ) {
                return col;
            }

            // Subtitle: Path and count
            let mut sub_buf = [0u8; 64];
            let count_val = app.playlist.count;
            let mut digits = [0u8; 10];
            let mut dlen = 0;
            let mut v = count_val;
            if v == 0 {
                digits[0] = b'0';
                dlen = 1;
            } else {
                while v > 0 && dlen < 10 {
                    digits[dlen] = b'0' + (v % 10) as u8;
                    v /= 10;
                    dlen += 1;
                }
            }
            let mut sn = 0;
            for i in 0..dlen {
                sub_buf[sn] = digits[dlen - 1 - i];
                sn += 1;
            }
            let tail = b" videos in /storage/videos & downloads";
            for &b in tail {
                if sn < sub_buf.len() {
                    sub_buf[sn] = b;
                    sn += 1;
                }
            }
            if let Some(col) = draw_text(
                x,
                y,
                96,
                header_line_y(),
                1,
                &sub_buf[..sn],
                COLOR_TEXT_MUTED,
                COLOR_BG,
            ) {
                return col;
            }

            // Hairline separator
            if y == header_rule_y() {
                return COLOR_BORDER;
            }
            return COLOR_BG;
        }

        // Playlist Rows (y: LIB_LIST_Y0..LIB_VIEW_Y1)
        //
        // WHY the offset is in pixels: a drag delivers a few pixels per event,
        // and a row-quantised offset discarded every drag shorter than a row,
        // so a list longer than the screen could not be scrolled at all.
        let content_y = (y - lib_list_y0()) + app.library_scroll;
        let row_idx = content_y / lib_row_h();

        // WHY the band is closed at `LIB_VIEW_Y1`: the press path only selects a
        // row above that line, so a row painted below it was a visible track the
        // user could tap and nothing answered.
        if row_idx < app.playlist.count && y < LIB_VIEW_Y1 {
            let row_y0 = lib_list_y0() + row_idx * lib_row_h() - app.library_scroll;
            let is_current = row_idx == app.playlist.current;
            let row_bg = if is_current { COLOR_SURFACE } else { COLOR_BG };

            // Left accent bar on current playing item
            if is_current && x < 8 {
                return COLOR_ACCENT;
            }

            // Row separator hairline
            if y == row_y0 + lib_row_h() - 1 {
                return COLOR_BORDER;
            }

            let item = &app.playlist.items[row_idx];

            // Playing / Track Indicator Icon (x: 24..48, y: row_y0 + 20..36)
            let icon_cx = 36;
            let icon_cy = row_y0 + 32;
            let idx_x = (x as i32) - (icon_cx as i32);
            let idx_y = (y as i32) - (icon_cy as i32);
            if is_current && app.state == PlaybackState::Playing {
                // Play triangle, only while the decoder is actually playing:
                // the accent bar already marks the current row, and a
                // triangle on a refused file claimed playback that was not
                // happening.
                if idx_x >= -4 && idx_x <= 6 && idx_y.abs() <= (6 - idx_x) * 6 / 10 {
                    return COLOR_TEXT_PRIMARY;
                }
            } else {
                // Small filmstrip dot
                if idx_x * idx_x + idx_y * idx_y <= 9 {
                    return COLOR_TEXT_MUTED;
                }
            }

            // Line 1: Title in Scale 2 Body
            let title_bytes = item.title_str().as_bytes();
            let mut tbuf = [0u8; 48];
            let tn = fit_run(title_bytes, 2, true, 600, &mut tbuf);
            let title_col = if is_current { COLOR_TEXT_PRIMARY } else { 0xFFE0E0E0 };
            if let Some(col) = draw_text_bold(x, y, 64, row_y0 + LIB_ROW_TITLE_DY, 2, &tbuf[..tn], title_col, row_bg) {
                return col;
            }

            // Line 2: Subtitle Metadata: [File Size] - [Codec] - [Resolution] - [Duration]
            //
            // WHY the separator is a spaced hyphen and not the typographic bullet
            // this line used to claim: the run is painted on the bitmap rung when
            // the outline face refuses it, and a bullet is not in that cell, so
            // the mark between two measurements would render as mojibake.
            let mut meta_buf = [0u8; 64];
            let mut ml = 0;

            // Size
            let mut s_buf = [0u8; 16];
            let sn = format_file_size(item.file_size, &mut s_buf);
            meta_buf[ml..ml + sn].copy_from_slice(&s_buf[..sn]);
            ml += sn;

            meta_buf[ml..ml + 3].copy_from_slice(b" - ");
            ml += 3;

            // Codec
            let c_label = item.codec.label().as_bytes();
            meta_buf[ml..ml + c_label.len()].copy_from_slice(c_label);
            ml += c_label.len();

            meta_buf[ml..ml + 3].copy_from_slice(b" - ");
            ml += 3;

            // Resolution
            let mut r_buf = [0u8; 16];
            let rn = format_resolution(item.width, item.height, &mut r_buf);
            meta_buf[ml..ml + rn].copy_from_slice(&r_buf[..rn]);
            ml += rn;

            meta_buf[ml..ml + 3].copy_from_slice(b" - ");
            ml += 3;

            // Duration
            let mut d_buf = [0u8; 12];
            let dn = duration_label(item.duration_sec > 0, item.duration_sec, false, &mut d_buf);
            meta_buf[ml..ml + dn].copy_from_slice(&d_buf[..dn]);
            ml += dn;

            if let Some(col) = draw_text(x, y, 64, row_y0 + lib_meta_dy(), 1, &meta_buf[..ml], COLOR_TEXT_MUTED, row_bg) {
                return col;
            }

            return row_bg;
        }

        return COLOR_BG;
    }

    // =========================================================================
    // 3. PICTURE-IN-PICTURE (PIP) FLOATING PREVIEW CARD
    // =========================================================================
    if app.pip_mode && PIP_RECT.contains(x, y) {
        // 14px rounded rectangle border
        if let Some(col) = draw_rounded_rect(x, y, PIP_RECT, 14, COLOR_SURFACE_SUNKEN, Some(COLOR_ACCENT), 2) {
            // Decoded video preview inside PIP card
            let pip_cx = x - PIP_X;
            let pip_cy = y - PIP_Y;

            if app.has_frame {
                let sx = (pip_cx * CANVAS_W) / PIP_W;
                let sy = (pip_cy * CANVAS_H) / PIP_H;
                let idx = sy.min(CANVAS_H - 1) * CANVAS_W + sx.min(CANVAS_W - 1);
                unsafe {
                    return (*core::ptr::addr_of!(CANVAS_PIXELS))[idx];
                }
            }

            // PIP Badge on top-left
            let badge = b"PIP";
            if let Some(t_col) = draw_text_bold(x, y, PIP_X + 12, PIP_Y + 12, 1, badge, COLOR_TEXT_PRIMARY, COLOR_SURFACE_SUNKEN) {
                return t_col;
            }

            return col;
        }
    }

    // =========================================================================
    // 4. PORTRAIT MODE UI BANDS
    // =========================================================================

    // -------------------------------------------------------------------------
    // Band 0: Top Bar (Back, Title, Orientation, Kebab)
    // -------------------------------------------------------------------------
    if y < band0_y1() {
        // The platform Back chip -- the same plate, ring and chevron every other
        // screen in this device puts in the top-left corner, painted by
        // `back_button::pixel_for_theme` with this app's own `THEME` (the dark
        // media theme in src/theme.rs: ink #F5F6F7, ring #B4B9C2, plate #2F3235
        // on the #111214 page). It is asked before the title and before the two
        // controls to its right because it is the one rect on this row the app
        // does not author, and it owns columns 24..72 outright.
        if let Some(c) = back_button::pixel_for_theme(x, y, &THEME) {
            return c;
        }

        // Orientation Mode Button (⛶ Fullscreen / Landscape Toggle)
        //
        // WHY the plate: both header controls were answered as a 52x52 square
        // whose only ink was the mark in the middle, on a page colour identical
        // to the square's own "background" -- so the row read as two glyphs
        // floating over the title rather than as the two buttons beside it that
        // the press genuinely answers. They now stand on the same plate the
        // secondary row below uses.
        if let Some(col) =
            draw_rounded_rect(x, y, ORIENT_BTN, 12, COLOR_SURFACE, Some(COLOR_BORDER), 1)
        {
            let cx = ORIENT_BTN.x + ORIENT_BTN.w / 2;
            let cy = ORIENT_BTN.y + ORIENT_BTN.h / 2;
            let dx = (x as i32) - (cx as i32);
            let dy = (y as i32) - (cy as i32);
            // Draw orientation corner marks
            if (dx.abs() >= 7 && dx.abs() <= 11 && dy.abs() >= 7 && dy.abs() <= 11)
                || (dx.abs() == 11 && dy.abs() <= 11)
                || (dy.abs() == 11 && dx.abs() <= 11)
            {
                return COLOR_TEXT_PRIMARY;
            }
            return col;
        }

        // Kebab Overflow (⋮)
        if let Some(col) =
            draw_rounded_rect(x, y, KEBAB_BTN, 12, COLOR_SURFACE, Some(COLOR_BORDER), 1)
        {
            let cx = KEBAB_BTN.x + KEBAB_BTN.w / 2;
            let cy = KEBAB_BTN.y + KEBAB_BTN.h / 2;
            let dx = (x as i32) - (cx as i32);
            let dy = (y as i32) - (cy as i32);
            if dx * dx <= 4 && (dy.abs() <= 2 || (dy - 9).abs() <= 2 || (dy + 9).abs() <= 2) {
                return COLOR_TEXT_PRIMARY;
            }
            return col;
        }

        // Title in Scale 2 Body Bold
        let title_bytes = app.current_title().as_bytes();
        let mut tbuf = [0u8; 48];
        let tn = fit_run(title_bytes, 2, true, HEADER_TITLE_MAX_W, &mut tbuf);
        let tw = run_w(&tbuf[..tn], 2, true);
        let tx = (W.saturating_sub(tw)) / 2;
        if let Some(col) = draw_text_bold(
            x,
            y,
            tx,
            HEADER_TITLE_Y,
            2,
            &tbuf[..tn],
            COLOR_TEXT_PRIMARY,
            COLOR_BG,
        ) {
            return col;
        }

        // Subtitle line (Format badge + Speed)
        let codec_label = app.current_codec().label().as_bytes();
        let speed_label = app.speed.label().as_bytes();
        let mut sub_buf = [0u8; 32];
        let mut sl = 0;
        sub_buf[sl..sl + codec_label.len()].copy_from_slice(codec_label);
        sl += codec_label.len();
        sub_buf[sl..sl + 3].copy_from_slice(b" - ");
        sl += 3;
        sub_buf[sl..sl + speed_label.len()].copy_from_slice(speed_label);
        sl += speed_label.len();

        let sw = run_w(&sub_buf[..sl], 1, false);
        let sx = (W.saturating_sub(sw)) / 2;
        if let Some(col) = draw_text(
            x,
            y,
            sx,
            header_line_y(),
            1,
            &sub_buf[..sl],
            COLOR_TEXT_MUTED,
            COLOR_BG,
        ) {
            return col;
        }

        // Hairline divider
        if y == header_rule_y() {
            return COLOR_BORDER;
        }

        return COLOR_BG;
    }

    // -------------------------------------------------------------------------
    // Band 1: 16:9 Video Canvas (680x382, r=16)
    // -------------------------------------------------------------------------
    if y >= band0_y1() && y < BAND1_Y1 {
        // The canvas plate at the rung this frame's header band reached. It is a
        // local, standing in for the platform-level constant of the same name,
        // so every row this branch places -- the mask, the border, the notices
        // and the decoded frame -- is placed by the geometry the band above it
        // actually answered, and not by a rung that has already moved.
        let canvas = canvas_rect();
        if canvas.contains(x, y) {
            // Apply 16px corner radius mask
            let r = 16i32;
            let in_left = x < canvas.x + 16;
            let in_right = x >= canvas.x + canvas.w - 16;
            let in_top = y < canvas.y + 16;
            let in_bottom = y >= canvas.y + canvas.h - 16;

            if (in_left || in_right) && (in_top || in_bottom) {
                let cx = if in_left {
                    canvas.x + 16
                } else {
                    canvas.x + canvas.w - 16 - 1
                } as i32;
                let cy = if in_top {
                    canvas.y + 16
                } else {
                    canvas.y + canvas.h - 16 - 1
                } as i32;
                let dx = (x as i32) - cx;
                let dy = (y as i32) - cy;
                if dx * dx + dy * dy > r * r {
                    return COLOR_BG;
                }
            }

            // 1px Border
            if x == canvas.x
                || x == canvas.x + canvas.w - 1
                || y == canvas.y
                || y == canvas.y + canvas.h - 1
            {
                return COLOR_BORDER;
            }

            // Platform Truth Contract: Report missing hardware decoder
            if let DecoderStatus::HardwareUnavailable(msg) = app.status {
                let mw = run_w(msg.as_bytes(), 1, true);
                let mx = (W.saturating_sub(mw)) / 2;
                if let Some(col) = draw_text_bold(
                    x,
                    y,
                    mx,
                    canvas.y + 176,
                    1,
                    msg.as_bytes(),
                    0xFFE05A68,
                    COLOR_SURFACE_SUNKEN,
                ) {
                    return col;
                }
                return COLOR_SURFACE_SUNKEN;
            }

            // A typed container/decode refusal is shown verbatim; the canvas
            // never falls back to black while claiming to play.
            if let DecoderStatus::Error(msg) = app.status {
                let mw = run_w(msg.as_bytes(), 1, true);
                let mx = (W.saturating_sub(mw)) / 2;
                if let Some(col) = draw_text_bold(
                    x,
                    y,
                    mx,
                    canvas.y + 176,
                    1,
                    msg.as_bytes(),
                    0xFFE05A68,
                    COLOR_SURFACE_SUNKEN,
                ) {
                    return col;
                }
                return COLOR_SURFACE_SUNKEN;
            }

            // An AVI that carries audio is stated as such: this player has no
            // audio decoder and does not pretend the volume bar drives it.
            if app.avi_audio_unavailable() {
                let notice = b"Audio stream: unavailable";
                let nx = CANVAS_X0 + 12;
                let ny = canvas.y + CANVAS_H - 28;
                // WHY the gate is the run's real box: `draw_text` answers inside
                // the caption rung's line box, and a gate shorter than that box
                // hands the run's last rows back to the surface behind it -- the
                // string reads whole only for as long as nobody writes a
                // descender into it.
                if x >= nx
                    && x < nx + run_w(notice, 1, false)
                    && y >= ny
                    && y < ny + line_u(1)
                {
                    if let Some(col) = draw_text(
                        x,
                        y,
                        nx,
                        ny,
                        1,
                        notice,
                        COLOR_TEXT_PRIMARY,
                        COLOR_SURFACE_SUNKEN,
                    ) {
                        return col;
                    }
                    return COLOR_SURFACE_SUNKEN;
                }
            }

            // Real Decoded Video Frame
            if app.has_frame {
                let cx = x - CANVAS_X0;
                let cy = y - canvas.y;
                let pixel_idx = cy * CANVAS_W + cx;
                unsafe {
                    return (*core::ptr::addr_of!(CANVAS_PIXELS))[pixel_idx];
                }
            }

            // Idle Placeholder
            let msg1 = b"NO MEDIA OPEN";
            let mw1 = run_w(msg1, 2, true);
            let mx1 = (W.saturating_sub(mw1)) / 2;
            if let Some(col) = draw_text_bold(x, y, mx1, canvas.y + 150, 2, msg1, COLOR_TEXT_PRIMARY, COLOR_SURFACE) {
                return col;
            }

            let msg2 = b"Supported: AVI (MJPEG), PPM P6, RAW RGB24";
            let mw2 = run_w(msg2, 1, false);
            let mx2 = (W.saturating_sub(mw2)) / 2;
            if let Some(col) = draw_text(x, y, mx2, canvas.y + 190, 1, msg2, COLOR_TEXT_MUTED, COLOR_SURFACE) {
                return col;
            }

            return COLOR_SURFACE;
        }

        return COLOR_BG;
    }

    // -------------------------------------------------------------------------
    // Band 2: Scrubber (Scale 2 Elapsed, Seek Bar, Scale 2 Remaining)
    // -------------------------------------------------------------------------
    if y >= BAND2_Y0 && y < BAND2_Y1 {
        // Elapsed Time Text in Scale 2 Body
        let mut ebuf = [0u8; 12];
        let elen = format_time(app.elapsed_sec, false, &mut ebuf);
        if let Some(col) = draw_text(x, y, 24, 606, 2, &ebuf[..elen], COLOR_TEXT_PRIMARY, COLOR_BG) {
            return col;
        }

        // Remaining Time Text in Scale 2 Body
        let mut rbuf = [0u8; 12];
        let rlen = duration_label(app.duration_sec > 0, app.remaining_sec(), true, &mut rbuf);
        if let Some(col) = draw_text(x, y, REMAIN_X, 606, 2, &rbuf[..rlen], COLOR_TEXT_MUTED, COLOR_BG) {
            return col;
        }

        // Seek Bar Track
        let thumb_x = scrubber_thumb_x(SEEK_X, SEEK_W, app.elapsed_sec, app.duration_sec);

        // Circular Thumb (r=9)
        let dx = (x as i32) - (thumb_x as i32);
        let dy = (y as i32) - ((SEEK_Y + SEEK_H / 2) as i32);
        if dx * dx + dy * dy <= 81 {
            return COLOR_TEXT_PRIMARY;
        }

        // Bar Line
        if SEEK_RECT.contains(x, y) {
            return if x <= thumb_x {
                COLOR_PROGRESS_ACTIVE
            } else {
                COLOR_PROGRESS_BG
            };
        }

        return COLOR_BG;
    }

    // -------------------------------------------------------------------------
    // Band 3: Transport Controls (Prev, 88px Play/Pause, Next)
    // -------------------------------------------------------------------------
    if y >= BAND3_Y0 && y < BAND3_Y1 {
        // Previous Button (|◀◀)
        if let Some(col) = draw_circle(x, y, PREV_CX, TRANSPORT_CY, PREV_R, COLOR_SURFACE, Some(COLOR_BORDER)) {
            let dx = (x as i32) - (PREV_CX as i32);
            let dy = (y as i32) - (TRANSPORT_CY as i32);
            if dx >= -10 && dx <= -8 && dy.abs() <= 10 {
                return COLOR_TEXT_PRIMARY;
            }
            if dx > -8 && dx <= 8 {
                let half_h = (dx + 8) * 10 / 16;
                if dy.abs() <= half_h {
                    return COLOR_TEXT_PRIMARY;
                }
            }
            return col;
        }

        // Play / Pause 88px Circular Disc (White Fill, Dark Glyph)
        if let Some(col) = draw_circle(x, y, PLAY_CX, TRANSPORT_CY, PLAY_R, COLOR_PLAY_FILL, None) {
            let dx = (x as i32) - (PLAY_CX as i32);
            let dy = (y as i32) - (TRANSPORT_CY as i32);

            if app.state == PlaybackState::Playing {
                if (dx >= -10 && dx <= -4 && dy.abs() <= 14) || (dx >= 4 && dx <= 10 && dy.abs() <= 14) {
                    return COLOR_PLAY_GLYPH;
                }
            } else if dx >= -8 && dx <= 14 {
                let half_h = (14 - dx) * 14 / 22;
                if dy.abs() <= half_h {
                    return COLOR_PLAY_GLYPH;
                }
            }
            return col;
        }

        // Next Button (▶▶|)
        if let Some(col) = draw_circle(x, y, NEXT_CX, TRANSPORT_CY, NEXT_R, COLOR_SURFACE, Some(COLOR_BORDER)) {
            let dx = (x as i32) - (NEXT_CX as i32);
            let dy = (y as i32) - (TRANSPORT_CY as i32);
            if dx >= -8 && dx < 8 {
                let half_h = (8 - dx) * 10 / 16;
                if dy.abs() <= half_h {
                    return COLOR_TEXT_PRIMARY;
                }
            }
            if dx >= 8 && dx <= 10 && dy.abs() <= 10 {
                return COLOR_TEXT_PRIMARY;
            }
            return col;
        }

        return COLOR_BG;
    }

    // -------------------------------------------------------------------------
    // Band 4: Audio Volume HUD (Speaker, Bar, % Label)
    // -------------------------------------------------------------------------
    if y >= BAND4_Y0 && y < BAND4_Y1 {
        // Speaker / Mute Button
        //
        // WHY the plate: the press answers a 52x52 square and the only ink in it
        // was the cone, so on the page's own colour the control read as a mark
        // beside the bar rather than as the button that mutes it. Same plate as
        // the header controls and the secondary row.
        if let Some(col) =
            draw_rounded_rect(x, y, VOL_MUTE_BTN, 12, COLOR_SURFACE, Some(COLOR_BORDER), 1)
        {
            let cx = VOL_MUTE_BTN.x + VOL_MUTE_BTN.w / 2;
            let cy = VOL_MUTE_BTN.y + VOL_MUTE_BTN.h / 2;
            let dx = (x as i32) - (cx as i32);
            let dy = (y as i32) - (cy as i32);

            // Speaker cone
            if dx >= -10 && dx <= -4 && dy.abs() <= 6 {
                return COLOR_TEXT_PRIMARY;
            }
            if dx > -4 && dx <= 4 && dy.abs() <= (dx + 4) * 10 / 8 {
                return COLOR_TEXT_PRIMARY;
            }

            if app.is_muted || !app.volume_known {
                // Cross mark on muted, and on a level the system never reported.
                if dx >= 6 && dx <= 14 && dy.abs() <= 6 && ((dx - 10) - dy).abs() <= 1 {
                    return 0xFFE05A68;
                }
                if dx >= 6 && dx <= 14 && dy.abs() <= 6 && ((dx - 10) + dy).abs() <= 1 {
                    return 0xFFE05A68;
                }
            } else {
                // Sound wave arcs
                let d_arc = dx * dx + dy * dy;
                if dx > 4 && d_arc >= 64 && d_arc <= 100 {
                    return COLOR_TEXT_PRIMARY;
                }
            }
            return col;
        }

        // Volume Bar
        let eff_vol = if app.is_muted || !app.volume_known { 0 } else { app.volume_percent };
        let thumb_x = volume_thumb_x(VOL_BAR_X, VOL_BAR_W, eff_vol);

        // Circular Thumb (r=7)
        let dx = (x as i32) - (thumb_x as i32);
        let dy = (y as i32) - ((VOL_BAR_Y + VOL_BAR_H / 2) as i32);
        if dx * dx + dy * dy <= 49 {
            return COLOR_TEXT_PRIMARY;
        }

        if VOL_BAR_RECT.contains(x, y) {
            return if x <= thumb_x {
                COLOR_PROGRESS_ACTIVE
            } else {
                COLOR_PROGRESS_BG
            };
        }

        // Volume % Text in Scale 2 Body
        let mut vbuf = [0u8; 8];
        let mut vn = 0;
        if app.is_muted {
            vbuf[..4].copy_from_slice(b"MUTE");
            vn = 4;
        } else if !app.volume_known {
            vbuf[..2].copy_from_slice(b"--");
            vn = 2;
        } else {
            let mut v = app.volume_percent;
            let mut digits = [0u8; 4];
            let mut dlen = 0;
            if v == 0 {
                digits[0] = b'0';
                dlen = 1;
            } else {
                while v > 0 && dlen < 4 {
                    digits[dlen] = b'0' + (v % 10) as u8;
                    v /= 10;
                    dlen += 1;
                }
            }
            for i in 0..dlen {
                vbuf[vn] = digits[dlen - 1 - i];
                vn += 1;
            }
            if vn < vbuf.len() {
                vbuf[vn] = b'%';
                vn += 1;
            }
        }
        // The volume read-out, stood on its own HUD row.
        //
        // WHY the smaller of the two numbers: band 4 answers rows up to
        // `BAND4_Y1` and nothing below it, so a body-rung box taller than the
        // band's remaining rows -- which is what the largest setting makes --
        // would have its last rows painted by band 5 instead. Moving the run up
        // to the highest row that still reserves its whole box inside band 4 keeps
        // the ink whole at every selectable level and leaves the platform's rung
        // exactly where it was (836 against 851 at one hundred per cent).
        let vol_y = 836usize.min(BAND4_Y1 - line_u(2));
        if let Some(col) = draw_text(x, y, 610, vol_y, 2, &vbuf[..vn], COLOR_TEXT_PRIMARY, COLOR_BG) {
            return col;
        }

        return COLOR_BG;
    }

    // -------------------------------------------------------------------------
    // Band 5: Secondary Controls & Info Sheet
    // -------------------------------------------------------------------------
    if y >= BAND5_Y0 && y < BAND6_Y0 {
        // Speed Button (Scale 2 Body)
        if let Some(col) = draw_rounded_rect(x, y, SPEED_BTN, 12, COLOR_SURFACE, Some(COLOR_BORDER), 1) {
            let label = app.speed.label().as_bytes();
            let lw = run_w(label, 2, true);
            let lx = SPEED_BTN.x + (SPEED_BTN.w.saturating_sub(lw)) / 2;
            let ly = SPEED_BTN.y + (SPEED_BTN.h.saturating_sub(line_u(2))) / 2;
            if let Some(t_col) = draw_text_bold(x, y, lx, ly, 2, label, COLOR_TEXT_PRIMARY, COLOR_SURFACE) {
                return t_col;
            }
            return col;
        }

        // Aspect Ratio Button (Scale 2 Body)
        if let Some(col) = draw_rounded_rect(x, y, ASPECT_BTN, 12, COLOR_SURFACE, Some(COLOR_BORDER), 1) {
            let label = app.aspect.label().as_bytes();
            let lw = run_w(label, 2, true);
            let lx = ASPECT_BTN.x + (ASPECT_BTN.w.saturating_sub(lw)) / 2;
            let ly = ASPECT_BTN.y + (ASPECT_BTN.h.saturating_sub(line_u(2))) / 2;
            if let Some(t_col) = draw_text_bold(x, y, lx, ly, 2, label, COLOR_TEXT_PRIMARY, COLOR_SURFACE) {
                return t_col;
            }
            return col;
        }

        // Loop Mode Button (Scale 2 Body)
        if let Some(col) = draw_rounded_rect(x, y, LOOP_BTN, 12, COLOR_SURFACE, Some(COLOR_BORDER), 1) {
            let label = app.loop_mode.label().as_bytes();
            let lw = run_w(label, 2, true);
            let lx = LOOP_BTN.x + (LOOP_BTN.w.saturating_sub(lw)) / 2;
            let ly = LOOP_BTN.y + (LOOP_BTN.h.saturating_sub(line_u(2))) / 2;
            if let Some(t_col) = draw_text_bold(x, y, lx, ly, 2, label, COLOR_TEXT_PRIMARY, COLOR_SURFACE) {
                return t_col;
            }
            return col;
        }

        // PIP Mode Button (Scale 2 Body)
        let pip_fill = if app.pip_mode { COLOR_ACCENT } else { COLOR_SURFACE };
        let pip_ink = if app.pip_mode { COLOR_PLAY_GLYPH } else { COLOR_TEXT_PRIMARY };
        if let Some(col) = draw_rounded_rect(x, y, PIP_BTN, 12, pip_fill, Some(COLOR_BORDER), 1) {
            let label = b"PIP";
            let lw = run_w(label, 2, true);
            let lx = PIP_BTN.x + (PIP_BTN.w.saturating_sub(lw)) / 2;
            let ly = PIP_BTN.y + (PIP_BTN.h.saturating_sub(line_u(2))) / 2;
            if let Some(t_col) = draw_text_bold(x, y, lx, ly, 2, label, pip_ink, pip_fill) {
                return t_col;
            }
            return col;
        }

        // Library Drawer Button (Scale 2 Body)
        if let Some(col) = draw_rounded_rect(x, y, LIB_BTN, 12, COLOR_SURFACE, Some(COLOR_BORDER), 1) {
            let label = b"LIBRARY";
            let lw = run_w(label, 2, true);
            let lx = LIB_BTN.x + (LIB_BTN.w.saturating_sub(lw)) / 2;
            let ly = LIB_BTN.y + (LIB_BTN.h.saturating_sub(line_u(2))) / 2;
            if let Some(t_col) = draw_text_bold(x, y, lx, ly, 2, label, COLOR_TEXT_PRIMARY, COLOR_SURFACE) {
                return t_col;
            }
            return col;
        }

        // Info Sheet Button (Scale 2 Body)
        let info_fill = if app.info_open { COLOR_BORDER } else { COLOR_SURFACE };
        if let Some(col) = draw_rounded_rect(x, y, INFO_BTN, 12, info_fill, Some(COLOR_BORDER), 1) {
            let label = b"MEDIA INFO";
            let lw = run_w(label, 2, true);
            let lx = INFO_BTN.x + (INFO_BTN.w.saturating_sub(lw)) / 2;
            let ly = INFO_BTN.y + (INFO_BTN.h.saturating_sub(line_u(2))) / 2;
            if let Some(t_col) = draw_text_bold(x, y, lx, ly, 2, label, COLOR_TEXT_PRIMARY, info_fill) {
                return t_col;
            }
            return col;
        }

        // Media Info Sheet Card Overlay
        if app.info_open && SHEET_RECT.contains(x, y) {
            if let Some(col) = draw_rounded_rect(x, y, SHEET_RECT, 16, COLOR_SHEET_CARD, Some(COLOR_BORDER), 1) {
                // Header (Scale 2 Body Bold)
                let h_text = b"MEDIA PROPERTIES";
                if let Some(t_col) = draw_text_bold(
                    x,
                    y,
                    SHEET_LABEL_X,
                    SHEET_HEADER_Y,
                    2,
                    h_text,
                    COLOR_TEXT_PRIMARY,
                    COLOR_SHEET_CARD,
                ) {
                    return t_col;
                }

                // Format
                let c_text = app.current_codec().label().as_bytes();
                if let Some(t_col) = sheet_label(x, y, sheet_row_y(0), b"Format:") {
                    return t_col;
                }
                if let Some(t_col) = sheet_value_bold(x, y, sheet_row_y(0), c_text) {
                    return t_col;
                }

                // Resolution
                if let Some(t_col) = sheet_label(x, y, sheet_row_y(1), b"Canvas:") {
                    return t_col;
                }
                // The frame size comes from the selected file's own header;
                // a file whose header nobody read says Unknown.
                let (src_w, src_h) = app
                    .playlist
                    .current_item()
                    .map(|item| (item.width, item.height))
                    .unwrap_or((0, 0));
                let mut res_buf = [0u8; 24];
                let res_len = format_resolution(src_w, src_h, &mut res_buf);
                if let Some(t_col) =
                    sheet_value_bold(x, y, sheet_row_y(1), &res_buf[..res_len])
                {
                    return t_col;
                }

                // Decoder Pipeline
                if let Some(t_col) = sheet_label(x, y, sheet_row_y(2), b"Decoder:") {
                    return t_col;
                }
                // Only the formats this app genuinely decodes claim a
                // decoder. Everything else says so instead of naming a
                // pipeline that would have to invent its output.
                let dec_label: &[u8] = if let DecoderStatus::HardwareUnavailable(_) = app.status {
                    b"Unavailable (No HW H.264)"
                } else if let DecoderStatus::Error(_) = app.status {
                    b"Unavailable (refused)"
                } else {
                    match app.current_codec() {
                        VideoCodec::Mjpeg => b"Software (MJPEG)",
                        VideoCodec::Avi => b"Software (AVI/MJPEG)",
                        VideoCodec::PpmP6 | VideoCodec::RawRgb | VideoCodec::RawRgba => {
                            b"Software (built-in)"
                        }
                        _ => b"No decoder bound",
                    }
                };
                if let Some(t_col) = sheet_value_bold(x, y, sheet_row_y(2), dec_label) {
                    return t_col;
                }

                // Audio: an AVI's audio stream is named but not claimed as
                // playable; the player has no audio decoder for it.
                if let Some(t_col) = sheet_label(x, y, sheet_row_y(4), b"Audio:") {
                    return t_col;
                }
                let audio_label: &[u8] = if app.current_codec() == VideoCodec::Avi {
                    if app.avi_audio_pcm() {
                        b"Software (PCM)"
                    } else if app.avi_audio_unavailable() {
                        b"Unavailable (not implemented)"
                    } else {
                        b"Not present in this file"
                    }
                } else {
                    b"Unavailable (no decoder)"
                };
                if let Some(t_col) = sheet_value_caption(x, y, sheet_row_y(4), audio_label) {
                    return t_col;
                }

                // Location / Path
                if let Some(t_col) = sheet_label(x, y, sheet_row_y(3), b"Location:") {
                    return t_col;
                }
                let path_bytes = app.current_path().as_bytes();
                if let Some(t_col) = sheet_value_caption(x, y, sheet_row_y(3), path_bytes) {
                    return t_col;
                }

                return col;
            }
        }

        return COLOR_BG;
    }

    // -------------------------------------------------------------------------
    // Band 6: Home Gesture Bar
    // -------------------------------------------------------------------------
    if y >= BAND6_Y0 {
        return COLOR_BG;
    }

    COLOR_BG
}

// -----------------------------------------------------------------------------
// Banded Surface Renderer
// -----------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Dirty {
    None,
    TopBar,    // Band 0
    Canvas,    // Band 1
    Scrubber,  // Band 2
    Transport, // Band 3
    Volume,    // Band 4
    Secondary, // Band 5
    All,       // Full repaint
}

fn merge_dirty(a: Dirty, b: Dirty) -> Dirty {
    if a == Dirty::None {
        return b;
    }
    if b == Dirty::None || a == b {
        return a;
    }
    Dirty::All
}

/// Toggles the transport without claiming playback on an unknown timeline.
///
/// WHY the running time gates the Playing state: the position advances only
/// over a known duration, so a file whose duration was never read would sit
/// in Playing without moving. Paused is the honest state for that file. An
/// open AVI is exempt: its own frames decide when it is moving.
fn toggle_playback(app: &mut VideoApp) {
    if app.state == PlaybackState::Playing {
        app.state = PlaybackState::Paused;
    } else if app.avi.is_some() || app.duration_sec > 0 {
        app.state = PlaybackState::Playing;
        if let Some(pb) = app.avi.as_mut() {
            // Resume from now instead of bursting through the frames the
            // pause covered.
            pb.deadline_ms = std::get_ticks().unwrap_or(0);
        }
    } else {
        app.state = PlaybackState::Paused;
        std::log("[VIDEO APP] Running time unknown; the position cannot advance.").ok();
    }
}

fn render_video_band(surface: u32, app: &VideoApp, y0: usize, y1: usize) {
    let mut offset = y0 * W;
    let end = y1.min(H) * W;
    while offset < end {
        let count = (end - offset).min(CHUNK);
        let mut x = offset % W;
        let mut y = offset / W;
        unsafe {
            let chunk = &mut *core::ptr::addr_of_mut!(CHUNK_BUF);
            for i in 0..count {
                chunk[i] = get_video_pixel(x, y, app);
                x += 1;
                if x == W {
                    x = 0;
                    y += 1;
                }
            }
            let _ = std::surface_write(surface, offset, &chunk[..count]);
        }
        offset += count;
    }
}

fn render_video_ui(surface: u32, app: &VideoApp) {
    render_video_band(surface, app, 0, H);
}

fn repaint(surface: u32, app: &VideoApp, dirty: Dirty) {
    if app.orientation == Orientation::Landscape || app.library_open || app.pip_mode {
        render_video_ui(surface, app);
        return;
    }
    match dirty {
        Dirty::None => {}
        Dirty::TopBar => render_video_band(surface, app, BAND0_Y0, band0_y1()),
        Dirty::Canvas => render_video_band(surface, app, band0_y1(), BAND1_Y1),
        Dirty::Scrubber => render_video_band(surface, app, BAND2_Y0, BAND2_Y1),
        Dirty::Transport => render_video_band(surface, app, BAND3_Y0, BAND3_Y1),
        Dirty::Volume => render_video_band(surface, app, BAND4_Y0, BAND4_Y1),
        Dirty::Secondary => render_video_band(surface, app, BAND5_Y0, BAND5_Y1),
        Dirty::All => render_video_ui(surface, app),
    }
}

// -----------------------------------------------------------------------------
// Event Handling & Interaction
// -----------------------------------------------------------------------------

enum EventAction {
    None,
    Redraw(Dirty),
    Exit(&'static str),
}

fn handle_touch(app: &mut VideoApp, px: u32, py: u32) -> EventAction {
    let x = px as usize;
    let y = py as usize;

    // Home gesture: Exit to launcher
    if HOME_HIT.contains(x, y) {
        return EventAction::Exit("[VIDEO PLAYER] Home gesture recognized. Exiting.");
    }

    // =========================================================================
    // 1. LANDSCAPE MODE TOUCH DISPATCH
    // =========================================================================
    if app.orientation == Orientation::Landscape {
        if app.hud_visible {
            // Orientation button in top bar -> Return to Portrait
            if land_back_btn().contains(x, y) {
                app.orientation = Orientation::Portrait;
                app.aspect = AspectRatio::Fit;
                let _ = std::audio_haptic_click();
                return EventAction::Redraw(Dirty::All);
            }

            // Scrubber seek in landscape: the band `LAND_SEEK_HIT` names, which
            // is the box the panel paints the track inside and is gated to the
            // platform's minimum height.
            if LAND_SEEK_HIT.contains(x, y) {
                let seek_sec =
                    scrubber_seek_seconds(x, LAND_SEEK_X, LAND_SEEK_W, app.duration_sec);
                app.seek_to_sec(seek_sec);
                return EventAction::Redraw(Dirty::All);
            }

            // Play/Pause button in landscape
            if LAND_PLAY_HIT.contains(x, y) {
                toggle_playback(app);
                let _ = std::audio_haptic_click();
                return EventAction::Redraw(Dirty::All);
            }

            // Previous and Next controls in landscape. They are drawn beside
            // the play control, so a tap on one has to do what it looks like.
            if LAND_PREV_HIT.contains(x, y) {
                if app.avi.is_some() {
                    app.load_current_track();
                    app.state = if app.has_frame {
                        PlaybackState::Playing
                    } else {
                        PlaybackState::Stopped
                    };
                } else if app.playlist.prev().is_some() {
                    app.load_current_track();
                    app.state = if app.has_frame {
                        PlaybackState::Playing
                    } else {
                        PlaybackState::Stopped
                    };
                }
                let _ = std::audio_haptic_click();
                return EventAction::Redraw(Dirty::All);
            }
            if LAND_NEXT_HIT.contains(x, y) {
                if app.playlist.next(app.loop_mode).is_some() {
                    app.load_current_track();
                    app.state = if app.has_frame {
                        PlaybackState::Playing
                    } else {
                        PlaybackState::Stopped
                    };
                }
                let _ = std::audio_haptic_click();
                return EventAction::Redraw(Dirty::All);
            }

            // Volume bar in landscape: the band the panel paints the track
            // inside, gated to the platform's minimum height.
            if LAND_VOL_HIT.contains(x, y) {
                let pct = volume_seek_percent(x, LAND_VOL_X, LAND_VOL_W);
                app.volume_percent = pct;
                app.is_muted = false;
                app.volume_known = true;
                let _ = std::audio_set_volume(pct);
                return EventAction::Redraw(Dirty::All);
            }

            // Speaker mute in landscape: the plate the HUD now paints.
            if LAND_MUTE_BTN.contains(x, y) {
                app.is_muted = !app.is_muted;
                app.volume_known = true;
                let eff_vol = if app.is_muted { 0 } else { app.volume_percent };
                let _ = std::audio_set_volume(eff_vol);
                let _ = std::audio_haptic_click();
                return EventAction::Redraw(Dirty::All);
            }
        }

        // Tap on open canvas toggles HUD visibility
        app.hud_visible = !app.hud_visible;
        return EventAction::Redraw(Dirty::All);
    }

    // =========================================================================
    // 2. VIDEO LIBRARY & PIP TOUCH DISPATCH
    // =========================================================================
    if app.library_open || app.pip_mode {
        // Tapping floating PIP card -> Exit PIP mode and restore normal player
        if app.pip_mode && PIP_RECT.contains(x, y) {
            app.pip_mode = false;
            app.library_open = false;
            let _ = std::audio_haptic_click();
            return EventAction::Redraw(Dirty::All);
        }

        // The platform Back chip. It is tested through `back_button::contains`
        // rather than an app rect so the press answers the very plate the drawer
        // painter put there -- columns 24..72, rows 56..104, no invisible margin.
        if back_button::contains(x, y) {
            if app.library_open {
                app.library_open = false;
                let _ = std::audio_haptic_click();
                return EventAction::Redraw(Dirty::All);
            } else if app.pip_mode {
                app.pip_mode = false;
                let _ = std::audio_haptic_click();
                return EventAction::Redraw(Dirty::All);
            }
        }

        // Video row selection
        if y >= lib_list_y0() && y < LIB_VIEW_Y1 {
            let row_idx = ((y - lib_list_y0()) + app.library_scroll) / lib_row_h();
            if row_idx < app.playlist.count {
                app.playlist.select(row_idx);
                app.load_current_track();
                // WHY the state follows the decoder: selecting a row used to
                // claim playback even when the file was refused (an H.264
                // stream with no hardware decoder, or a malformed container),
                // so the screen read "playing" over an error canvas. A row
                // is playing only when a frame was actually decoded.
                app.state = if app.has_frame {
                    PlaybackState::Playing
                } else {
                    PlaybackState::Stopped
                };
                let _ = std::audio_haptic_click();
                if !app.pip_mode {
                    app.library_open = false;
                }
                return EventAction::Redraw(Dirty::All);
            }
        }

        // WHY the drawer keeps the rest of its own canvas: whenever the drawer is
        // showing, the painter covers every pixel outside the rows with it, so a
        // press on that background used to fall through to the portrait controls
        // hidden underneath -- a seek or a volume change the user could not see
        // happening. In picture-in-picture it was worse still: the card answers
        // its own press above, so everything outside it is drawer.
        return EventAction::None;
    }

    // =========================================================================
    // 3. PORTRAIT NORMAL MODE TOUCH DISPATCH
    // =========================================================================

    // Top Bar: the platform Back control -> Exit to launcher. Tested through
    // `back_button::contains` for the same reason it is painted through
    // `back_button::pixel_for_theme`: one rect, 24..72 by 56..104, is what a
    // person sees and what the press answers.
    if back_button::contains(x, y) {
        return EventAction::Exit("[VIDEO PLAYER] Back pressed. Exiting.");
    }

    // Top Bar: Orientation Mode button -> Toggle Landscape
    if ORIENT_BTN.contains(x, y) {
        app.orientation = Orientation::Landscape;
        app.hud_visible = true;
        let _ = std::audio_haptic_click();
        return EventAction::Redraw(Dirty::All);
    }

    // Top Bar: Kebab Button -> Toggle Library Drawer
    if KEBAB_BTN.contains(x, y) {
        app.library_open = !app.library_open;
        let _ = std::audio_haptic_click();
        return EventAction::Redraw(Dirty::All);
    }

    // Scrubber Touch / Seek
    if SEEK_HIT.contains(x, y) {
        let seek_sec = scrubber_seek_seconds(x, SEEK_X, SEEK_W, app.duration_sec);
        app.seek_to_sec(seek_sec);
        return EventAction::Redraw(Dirty::All);
    }

    // Play/Pause Button
    if PLAY_HIT.contains(x, y) {
        toggle_playback(app);
        let _ = std::audio_haptic_click();
        return EventAction::Redraw(Dirty::All);
    }

    // Prev Button
    if PREV_HIT.contains(x, y) {
        if app.avi.is_some() {
            // Restarting reopens and re-reads the stream; it is the only
            // "previous" a non-seekable container can honour.
            app.load_current_track();
            app.state = if app.avi.is_some() {
                PlaybackState::Playing
            } else {
                PlaybackState::Stopped
            };
            let _ = std::audio_haptic_click();
            return EventAction::Redraw(Dirty::All);
        }
        if app.elapsed_sec > 3 {
            app.elapsed_sec = 0;
            return EventAction::Redraw(Dirty::Scrubber);
        }
        if app.playlist.prev().is_some() {
            app.load_current_track();
            let _ = std::audio_haptic_click();
            return EventAction::Redraw(Dirty::All);
        }
    }

    // Next Button
    if NEXT_HIT.contains(x, y) {
        if app.playlist.next(app.loop_mode).is_some() {
            app.load_current_track();
            if app.avi.is_some() {
                app.state = PlaybackState::Playing;
            }
            let _ = std::audio_haptic_click();
            return EventAction::Redraw(Dirty::All);
        }
    }

    // Volume HUD: Speaker Mute Toggle
    if VOL_MUTE_BTN.contains(x, y) {
        app.is_muted = !app.is_muted;
        app.volume_known = true;
        let eff_vol = if app.is_muted { 0 } else { app.volume_percent };
        let _ = std::audio_set_volume(eff_vol);
        let _ = std::audio_haptic_click();
        return EventAction::Redraw(Dirty::Volume);
    }

    // Volume HUD: Volume Bar Seek
    if VOL_HIT.contains(x, y) {
        let pct = volume_seek_percent(x, VOL_BAR_X, VOL_BAR_W);
        app.volume_percent = pct;
        app.is_muted = false;
        app.volume_known = true;
        let _ = std::audio_set_volume(pct);
        return EventAction::Redraw(Dirty::Volume);
    }

    // Secondary: Speed Toggle
    if SPEED_BTN.contains(x, y) {
        app.speed = app.speed.next();
        app.retime_avi();
        let _ = std::audio_haptic_click();
        return EventAction::Redraw(Dirty::All);
    }

    // Secondary: Aspect Ratio Toggle
    if ASPECT_BTN.contains(x, y) {
        app.aspect = app.aspect.next();
        let _ = std::audio_haptic_click();
        if app.avi.is_some() {
            // Rescale the frame already decoded; reopening would restart.
            app.rescale_current_avi_frame();
        } else {
            app.load_current_track();
        }
        return EventAction::Redraw(Dirty::All);
    }

    // Secondary: Loop Mode Toggle
    if LOOP_BTN.contains(x, y) {
        app.loop_mode = app.loop_mode.next();
        if app.loop_mode != LoopMode::None
            && app.state == PlaybackState::Stopped
            && app.duration_sec > 0
            && app.elapsed_sec >= app.duration_sec
        {
            app.load_current_track();
            app.state = PlaybackState::Playing;
        }
        let _ = std::audio_haptic_click();
        return EventAction::Redraw(Dirty::All);
    }

    // Secondary: PIP Toggle
    if PIP_BTN.contains(x, y) {
        app.pip_mode = !app.pip_mode;
        let _ = std::audio_haptic_click();
        return EventAction::Redraw(Dirty::All);
    }

    // Secondary: Library Drawer Button
    if LIB_BTN.contains(x, y) {
        app.library_open = !app.library_open;
        let _ = std::audio_haptic_click();
        return EventAction::Redraw(Dirty::All);
    }

    // Secondary: Media Info Button
    if INFO_BTN.contains(x, y) {
        app.info_open = !app.info_open;
        let _ = std::audio_haptic_click();
        return EventAction::Redraw(Dirty::All);
    }

    EventAction::None
}

// -----------------------------------------------------------------------------
// Application Entrypoint (#![no_std], #![no_main])
// -----------------------------------------------------------------------------

#[no_mangle]
pub extern "C" fn _start() -> ! {
    std::log("[VIDEO PLAYER] InterOS Video Player starting (Ring-3 user mode)...").ok();

    // The person's text size, published before a single pixel is written.
    //
    // WHY here and not in the paint loop: the whole screen lays its rows out from
    // the published rung, so a frame that drew before this call would be a frame
    // that drew at the ladder and then resized -- and a settings query per glyph
    // is a syscall inside the frame budget.
    publish_text_scale();

    let mut app = VideoApp::new();

    // Query system volume. A failed query leaves the level unknown rather
    // than showing a made-up 80%.
    if let Ok(vol) = std::audio_query_volume() {
        app.volume_percent = vol.min(100);
        app.saved_volume = app.volume_percent;
        app.volume_known = true;
    }

    // Scan real video storage from /storage/videos and /storage/downloads
    app.rescan_storage();
    // A dispatch that launched this app is already waiting, so it wins over
    // the first entry the scan happened to find.
    app.handle_pending_intent();
    if app.playlist.count > 0 {
        app.load_current_track();
    }

    // Allocate client display surface (z=200 for foreground application)
    let surface = match std::surface_create(W as u32, H as u32, 200) {
        Ok(id) => {
            std::log("[VIDEO PLAYER] DRM/KMS Client Surface allocated (z=200).").ok();
            id
        }
        Err(_) => 0,
    };

    if surface > 0 {
        render_video_ui(surface, &app);
        let _ = std::surface_flip();
        std::log("[VIDEO PLAYER] Video canvas presented to scanout buffer.").ok();
    }

    let interest_mask = std::EVENT_TOUCH_INPUT | std::EVENT_IPC_MSG | std::EVENT_TIMER_TICK;

    loop {
        // A playing AVI needs to wake near its frame interval; idle screens
        // keep the long timeout so the task sleeps instead of spinning.
        let poll_timeout = if app.state == PlaybackState::Playing && app.avi.is_some() {
            10
        } else {
            150
        };
        match std::event_poll(interest_mask, poll_timeout) {
            Ok(events) => {
                if events & std::EVENT_TOUCH_INPUT != 0 {
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
                        // Keyboard input support
                        if let Some(key) = input.key_event() {
                            if key.pressed {
                                match key.code {
                                    std::input_key_code::ESCAPE => {
                                        app.close_avi();
                                        if surface > 0 {
                                            let _ = std::surface_destroy(surface);
                                        }
                                        std::exit_with_code(0);
                                    }
                                    code if code == b' ' as u32 => {
                                        toggle_playback(&mut app);
                                        dirty = merge_dirty(dirty, Dirty::All);
                                    }
                                    std::input_key_code::ARROW_LEFT => {
                                        if app.avi.is_some() {
                                            std::log("[VIDEO PLAYER] A sequential AVI cannot seek; position unchanged.").ok();
                                        } else {
                                            app.elapsed_sec = app.elapsed_sec.saturating_sub(5);
                                            dirty = merge_dirty(dirty, Dirty::Scrubber);
                                        }
                                    }
                                    std::input_key_code::ARROW_RIGHT => {
                                        if app.avi.is_some() {
                                            std::log("[VIDEO PLAYER] A sequential AVI cannot seek; position unchanged.").ok();
                                        } else {
                                            app.elapsed_sec =
                                                (app.elapsed_sec + 5).min(app.duration_sec);
                                            dirty = merge_dirty(dirty, Dirty::Scrubber);
                                        }
                                    }
                                    std::input_key_code::ARROW_UP => {
                                        app.volume_percent = (app.volume_percent + 10).min(100);
                                        app.is_muted = false;
                                        app.volume_known = true;
                                        let _ = std::audio_set_volume(app.volume_percent);
                                        dirty = merge_dirty(dirty, Dirty::Volume);
                                    }
                                    std::input_key_code::ARROW_DOWN => {
                                        app.volume_percent = app.volume_percent.saturating_sub(10);
                                        app.volume_known = true;
                                        let _ = std::audio_set_volume(app.volume_percent);
                                        dirty = merge_dirty(dirty, Dirty::Volume);
                                    }
                                    code if code == b'm' as u32 || code == b'M' as u32 => {
                                        app.is_muted = !app.is_muted;
                                        app.volume_known = true;
                                        let eff_vol =
                                            if app.is_muted { 0 } else { app.volume_percent };
                                        let _ = std::audio_set_volume(eff_vol);
                                        dirty = merge_dirty(dirty, Dirty::Volume);
                                    }
                                    code if code == b's' as u32 || code == b'S' as u32 => {
                                        app.speed = app.speed.next();
                                        app.retime_avi();
                                        dirty = merge_dirty(dirty, Dirty::All);
                                    }
                                    code if code == b'o' as u32 || code == b'O' as u32 => {
                                        app.orientation = app.orientation.toggle();
                                        dirty = merge_dirty(dirty, Dirty::All);
                                    }
                                    code if code == b'p' as u32 || code == b'P' as u32 => {
                                        app.pip_mode = !app.pip_mode;
                                        dirty = merge_dirty(dirty, Dirty::All);
                                    }
                                    _ => {}
                                }
                            }
                            continue;
                        }

                        // Navigation gestures: drag down/up outside canvas
                        if input.kind == 4
                            || input.kind == 6
                            || input.kind == 7
                            || input.kind == 8
                            || input.kind == 10
                        {
                            // A drag over the open library is the list's own
                            // gesture; only a drag anywhere else is the system
                            // navigation gesture.
                            let over_library = app.library_open
                                && (input.y as usize) >= lib_list_y0()
                                && (input.y as usize) < LIB_VIEW_Y1;
                            if (input.kind == 4 || input.kind == 10) && over_library {
                                let content_h = app.playlist.count * lib_row_h();
                                let viewport_h = LIB_VIEW_Y1 - lib_list_y0();
                                let max = content_h.saturating_sub(viewport_h);
                                if input.dy < 0 {
                                    app.library_scroll = app
                                        .library_scroll
                                        .saturating_add((-input.dy) as usize)
                                        .min(max);
                                } else {
                                    app.library_scroll =
                                        app.library_scroll.saturating_sub(input.dy as usize);
                                }
                                dirty = merge_dirty(dirty, Dirty::All);
                                continue;
                            }
                            if HOME_HIT.contains(input.x as usize, input.y as usize) {
                                app.close_avi();
                                if surface > 0 {
                                    let _ = std::surface_destroy(surface);
                                }
                                std::exit_with_code(0);
                            }
                        }

                        // Touch tap (kind 1) or drag (kind 2)
                        let in_seek = SEEK_HIT.contains(input.x as usize, input.y as usize);
                        let in_vol = VOL_HIT.contains(input.x as usize, input.y as usize);
                        if input.kind == 1 || (input.kind == 2 && (in_seek || in_vol)) {
                            match handle_touch(&mut app, input.x, input.y) {
                                EventAction::Exit(msg) => {
                                    std::log(msg).ok();
                                    app.close_avi();
                                    if surface > 0 {
                                        let _ = std::surface_destroy(surface);
                                    }
                                    std::exit_with_code(0);
                                }
                                EventAction::Redraw(d) => {
                                    dirty = merge_dirty(dirty, d);
                                }
                                EventAction::None => {}
                            }
                        }
                    }

                    if dirty != Dirty::None && surface > 0 {
                        repaint(surface, &app, dirty);
                        let _ = std::surface_flip();
                    }
                }

                // Timer tick for playback progression taking speed into account
                if events & std::EVENT_TIMER_TICK != 0 {
                    // A dispatch may reach this app while it is already
                    // running, so the slot is polled rather than pushed.
                    if app.handle_pending_intent() && surface > 0 {
                        // WHY the level is re-adopted here: a dispatch that
                        // finds this process alive is how the app is re-shown
                        // without a launch, and Settings writes the store and
                        // tells no one -- so the frame drawn straight after a
                        // re-show would otherwise stand at the rung the person
                        // left before the app was hidden. One settings query
                        // per re-show, and never inside the paint loop.
                        publish_text_scale();
                        repaint(surface, &app, Dirty::All);
                        let _ = std::surface_flip();
                    }
                    if app.state == PlaybackState::Playing && app.avi.is_some() {
                        // Real frame playback: the AVI's own clock decides
                        // which frames are shown, skipped, or the last one.
                        let tick = app.tick_avi();
                        if surface > 0 {
                            if tick.drawn {
                                if app.orientation == Orientation::Landscape
                                    || app.library_open
                                    || app.pip_mode
                                {
                                    repaint(surface, &app, Dirty::All);
                                } else {
                                    repaint(surface, &app, Dirty::Canvas);
                                    repaint(surface, &app, Dirty::Scrubber);
                                }
                                let _ = std::surface_flip();
                            } else if tick.ended || tick.failed {
                                repaint(surface, &app, Dirty::All);
                                let _ = std::surface_flip();
                            }
                        }
                    } else if app.state == PlaybackState::Playing && app.duration_sec > 0 {
                        let delta_ms = (150 * app.speed.multiplier_milli()) / 1000;
                        app.speed_accum_ms += delta_ms;

                        let mut advanced = false;
                        while app.speed_accum_ms >= 1000 {
                            app.speed_accum_ms -= 1000;
                            if app.elapsed_sec < app.duration_sec {
                                app.elapsed_sec += 1;
                                advanced = true;
                            } else {
                                // Reached end of track
                                match app.loop_mode {
                                    LoopMode::LoopOne => {
                                        app.elapsed_sec = 0;
                                        advanced = true;
                                    }
                                    LoopMode::LoopAll => {
                                        app.playlist.next(app.loop_mode);
                                        app.load_current_track();
                                        if surface > 0 {
                                            repaint(surface, &app, Dirty::All);
                                            let _ = std::surface_flip();
                                        }
                                        advanced = false;
                                    }
                                    LoopMode::None => {
                                        app.state = PlaybackState::Stopped;
                                        if surface > 0 {
                                            repaint(surface, &app, Dirty::All);
                                            let _ = std::surface_flip();
                                        }
                                        advanced = false;
                                    }
                                }
                            }
                        }

                        if advanced && surface > 0 {
                            if app.orientation == Orientation::Landscape || app.pip_mode {
                                repaint(surface, &app, Dirty::All);
                            } else {
                                repaint(surface, &app, Dirty::Scrubber);
                            }
                            let _ = std::surface_flip();
                        }
                    }
                }
            }
            Err(_) => {
                std::yield_now();
            }
        }
    }
}

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    std::log("[VIDEO PLAYER] Panic occurred. Terminating task...").ok();
    std::exit();
}
