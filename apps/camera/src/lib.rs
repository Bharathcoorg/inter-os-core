// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

#![no_std]

//! The camera application's model.
//!
//! WHY the model lives in the library and not in the binary: the binary is
//! the renderer, and a renderer cannot be unit tested on the host. Every
//! fact the camera states about the platform is decided here, so the tests
//! can pin it, and the renderer only draws what the model decided.

pub use interos_std as std;

/// The app's view of the platform's design system.
pub mod theme;

use core::ptr::addr_of_mut;
use interos_std::design::primitives::Rect;

// ---------------------------------------------------------------------------
// The platform's error codes the camera has to tell apart.
// ---------------------------------------------------------------------------

/// The platform refused the call because the caller does not hold the
/// capability the call needs.
pub const ERR_PERMISSION_DENIED: i64 = -1;

/// The platform authorised the call and the device behind it is not there.
pub const ERR_DEVICE_ERROR: i64 = -12;

/// The platform refused the call because an argument was not usable, which is
/// what a frame that does not fit the app's buffer comes back as.
pub const ERR_INVALID_ARGUMENTS: i64 = -3;

/// Viewfinder aspect ratio configuration (4:3 portrait, 16:9 cinematic, 1:1 square).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CameraAspectRatio {
    Ratio4_3,
    Ratio16_9,
    Square,
}

impl CameraAspectRatio {
    pub const fn label(self) -> &'static [u8] {
        match self {
            CameraAspectRatio::Ratio4_3 => b"4:3",
            CameraAspectRatio::Ratio16_9 => b"16:9",
            CameraAspectRatio::Square => b"1:1",
        }
    }

    pub const fn toggle(self) -> CameraAspectRatio {
        match self {
            CameraAspectRatio::Ratio4_3 => CameraAspectRatio::Ratio16_9,
            CameraAspectRatio::Ratio16_9 => CameraAspectRatio::Square,
            CameraAspectRatio::Square => CameraAspectRatio::Ratio4_3,
        }
    }
}

/// Self-timer countdown modes (Off, 3s, 10s).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimerMode {
    Off = 0,
    Sec3 = 1,
    Sec10 = 2,
}

impl TimerMode {
    pub const fn label(self) -> &'static [u8] {
        match self {
            TimerMode::Off => b"TIMER",
            TimerMode::Sec3 => b"3s",
            TimerMode::Sec10 => b"10s",
        }
    }

    pub const fn seconds(self) -> u32 {
        match self {
            TimerMode::Off => 0,
            TimerMode::Sec3 => 3,
            TimerMode::Sec10 => 10,
        }
    }

    pub const fn next(self) -> TimerMode {
        match self {
            TimerMode::Off => TimerMode::Sec3,
            TimerMode::Sec3 => TimerMode::Sec10,
            TimerMode::Sec10 => TimerMode::Off,
        }
    }
}

// ---------------------------------------------------------------------------
// Modes, filters and flash.
// ---------------------------------------------------------------------------

/// The operating mode the shutter is in.
///
/// WHY exactly these three: the mode decides what the shutter does, and the
/// shutter can take a still, take a video frame, or frame a document. The QR
/// scanner and the night mode that used to sit in this enum were never
/// reachable from the carousel and had no code behind them, so a variant that
/// named them was a claim about features the app does not have.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CameraMode {
    /// A still photograph.
    Photo = 0,
    /// A single video frame.
    Video = 1,
    /// A document framing.
    DocumentVision = 2,
}

impl CameraMode {
    /// The label the mode carousel shows.
    pub const fn label(self) -> &'static [u8] {
        match self {
            CameraMode::Photo => b"PHOTO",
            CameraMode::Video => b"VIDEO",
            CameraMode::DocumentVision => b"DOCUMENT",
        }
    }

    /// The next mode in the carousel.
    pub const fn next(self) -> CameraMode {
        match self {
            CameraMode::Photo => CameraMode::Video,
            CameraMode::Video => CameraMode::DocumentVision,
            CameraMode::DocumentVision => CameraMode::Photo,
        }
    }
}

/// The image processing the capture pipeline applies to a frame.
///
/// WHY there is no "neural" filter here: a filter that claims a model ran
/// over the frame is a claim about work this app never does. Every filter
/// below is calibrated ISP arithmetic the app performs on the pixels it was handed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CameraFilter {
    /// Normal: Neutral balanced color.
    Normal = 0,
    /// Vivid: High chroma and punchy contrast.
    Vivid = 1,
    /// Warm: Golden hour warm color temperature.
    Warm = 2,
    /// Cool: Cinematic cool blue-cyan tint.
    Cool = 3,
    /// Mono: High-contrast artistic black & white.
    Mono = 4,
    /// Vintage: Muted analog film curve with lifted matte blacks.
    Vintage = 5,
}

impl CameraFilter {
    /// The label the filter readout shows.
    pub const fn label(self) -> &'static [u8] {
        match self {
            CameraFilter::Normal => b"NORMAL",
            CameraFilter::Vivid => b"VIVID",
            CameraFilter::Warm => b"WARM",
            CameraFilter::Cool => b"COOL",
            CameraFilter::Mono => b"MONO",
            CameraFilter::Vintage => b"VINTAGE",
        }
    }

    /// The next filter in the readout's cycle.
    pub const fn next(self) -> CameraFilter {
        match self {
            CameraFilter::Normal => CameraFilter::Vivid,
            CameraFilter::Vivid => CameraFilter::Warm,
            CameraFilter::Warm => CameraFilter::Cool,
            CameraFilter::Cool => CameraFilter::Mono,
            CameraFilter::Mono => CameraFilter::Vintage,
            CameraFilter::Vintage => CameraFilter::Normal,
        }
    }
}

/// The flash modes supported by the camera hardware.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FlashMode {
    /// The platform decides whether the flash fires.
    Auto = 0,
    /// The flash fires for the capture.
    On = 1,
    /// The flash is turned off.
    Off = 2,
}

impl FlashMode {
    /// The label the flash readout shows.
    pub const fn label(self) -> &'static [u8] {
        match self {
            FlashMode::Auto => b"FLASH AUTO",
            FlashMode::On => b"FLASH ON",
            FlashMode::Off => b"FLASH OFF",
        }
    }

    /// Short pill label.
    pub const fn pill_label(self) -> &'static [u8] {
        match self {
            FlashMode::Auto => b"AUTO",
            FlashMode::On => b"ON",
            FlashMode::Off => b"OFF",
        }
    }

    /// The next flash state in the readout's cycle.
    pub const fn next(self) -> FlashMode {
        match self {
            FlashMode::Auto => FlashMode::On,
            FlashMode::On => FlashMode::Off,
            FlashMode::Off => FlashMode::Auto,
        }
    }
}

// ---------------------------------------------------------------------------
// What the platform has told the camera about itself.
// ---------------------------------------------------------------------------

/// What the platform has told the camera about the camera.
///
/// WHY these are separate states: "the platform refused this app the camera
/// capability" and "the camera HAL has no sensor bound" are different facts
/// with different remedies, and a screen that says "no sensor" for both
/// sends the user looking for a piece of hardware that is not the problem.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CameraStatus {
    /// The platform refused this app's camera authority.
    AuthorityRefused,
    /// This app holds the authority and the camera HAL reported that no
    /// sensor is bound to it.
    NoSensor,
    /// This app holds the authority; no frame has been captured yet.
    AuthorityHeld,
    /// The platform answered with an error this app has no state for.
    Failed,
}

impl CameraStatus {
    /// The short label the capture readout carries.
    pub const fn label(self) -> &'static [u8] {
        match self {
            CameraStatus::AuthorityRefused => b"NO AUTH",
            CameraStatus::NoSensor => b"NO SENSOR",
            CameraStatus::AuthorityHeld => b"NO FRAME",
            CameraStatus::Failed => b"CAMERA ERR",
        }
    }

    /// The headline the viewfinder shows.
    pub const fn headline(self) -> &'static [u8] {
        match self {
            CameraStatus::AuthorityRefused => b"Camera authority refused",
            CameraStatus::NoSensor => b"No camera sensor",
            CameraStatus::AuthorityHeld => b"No frame yet",
            CameraStatus::Failed => b"Capture refused",
        }
    }

    /// The line under the headline: why the camera is in this state.
    pub const fn reason(self) -> &'static [u8] {
        match self {
            CameraStatus::AuthorityRefused => {
                b"The platform denied this app the camera capability"
            }
            CameraStatus::NoSensor => b"The camera HAL reported that no sensor is bound",
            CameraStatus::AuthorityHeld => {
                b"The platform granted camera authority; no frame captured"
            }
            CameraStatus::Failed => b"The platform refused the capture",
        }
    }
}

/// Works out which fact an error code from the platform states.
///
/// WHY the code is read rather than the absence of a frame: the capture call
/// answers with a refusal when this app holds no camera authority, and with
/// a device error when the authority is held and the camera HAL has no
/// sensor. Both come back from the same call, so the code is the only thing
/// that tells the two apart.
pub fn status_from_error(code: i64) -> CameraStatus {
    match code {
        ERR_PERMISSION_DENIED => CameraStatus::AuthorityRefused,
        ERR_DEVICE_ERROR => CameraStatus::NoSensor,
        _ => CameraStatus::Failed,
    }
}

// ---------------------------------------------------------------------------
// The frame the platform returned.
// ---------------------------------------------------------------------------

/// How the pixels of a frame are laid out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrameFormat {
    /// Four bytes per pixel: red, green, blue, alpha.
    Rgba8888,
    /// Three bytes per pixel: red, green, blue.
    Rgb888,
    /// One byte per pixel: luminance.
    Gray8,
}

impl FrameFormat {
    /// How many bytes one pixel of this format takes.
    pub const fn bytes_per_pixel(self) -> usize {
        match self {
            FrameFormat::Rgba8888 => 4,
            FrameFormat::Rgb888 => 3,
            FrameFormat::Gray8 => 1,
        }
    }
}

/// The media type the camera's photos are written in.
///
/// WHY this is the type the platform sniffs from a PPM header: the gallery
/// declares a handler for it, so a photo the camera wrote is a photo the
/// platform can hand to the gallery that opens images.
pub const PHOTO_MIME: &str = "image/x-portable-pixmap";

/// The app's error code for "there is no photo to open".
///
/// WHY a local code and not a platform one: no photo is not something the
/// platform refused; it is a fact about this app's own state.
pub const ERR_NO_PHOTO: i64 = -2;

/// The surface the camera draws on.
pub const SURFACE_W: usize = 720;

/// The width of the card the platform's verdict is drawn on.
///
/// WHY the model owns it: the words are chosen here and the column they are
/// drawn into is this width less the dot's gutter and the card's right
/// margin, so only here can the two be checked against each other. The
/// renderer builds the card from these numbers, which is what the camera's
/// own viewfinder geometry already does.
pub const STATUS_CARD_W: usize = 520;

/// The gutter the card's status dot lives in.
pub const STATUS_ICON_GUTTER: usize = 64;

/// The margin the card keeps on its right.
pub const STATUS_TEXT_MARGIN: usize = theme::spacing::GRID_24;

/// The column the card's runs are drawn in.
pub const STATUS_TEXT_W: usize = STATUS_CARD_W - STATUS_ICON_GUTTER - STATUS_TEXT_MARGIN;

/// The rung the card's headline is drawn with.
pub const STATUS_HEAD_RUNG: usize = theme::typography::BODY_SCALE;

/// The rung the card's reason is drawn with.
pub const STATUS_REASON_RUNG: usize = theme::typography::CAPTION_SCALE;

/// How many lines the card reserves for the reason.
///
/// WHY the card reserves the worst case rather than each verdict's own: the
/// card floats over the viewfinder and the verdict changes under it; a card
/// that resized as the words changed would move the frame lines with it.
pub const STATUS_REASON_LINES: usize = 2;

/// The x a settings plate stands at: the sheet's own frame, one grid in.
///
/// WHY the model owns the plate: the renderer fills the plate and taps it, and
/// the model measures the row's runs against the column inside it. Splitting
/// those two numbers across the crates is what let the status card clip a
/// refusal the gate had just passed.
pub const SETTINGS_PLATE_X: usize = VIEWFINDER_GUTTER + theme::spacing::GRID_12;

/// The width of a settings plate: the surface less the gutter on each side.
pub const SETTINGS_PLATE_W: usize = SURFACE_W - 2 * SETTINGS_PLATE_X;

/// The inset the plate's runs stand off its own left edge.
pub const SETTINGS_TEXT_INSET: usize = theme::spacing::GRID_12;

/// The margin the plate keeps on its right.
pub const SETTINGS_TEXT_MARGIN: usize = theme::spacing::GRID_24;

/// The width the settings surface's text is measured against.
///
/// WHY it is the plate's own column and not a remembered number: a label that
/// does not fit its column is a label that runs off the plate it is drawn on,
/// and the settings surface is where the camera states what the platform
/// supports.
pub const SETTINGS_TEXT_MAX: usize =
    SETTINGS_PLATE_W - SETTINGS_TEXT_INSET - SETTINGS_TEXT_MARGIN;

/// The rung a plate's title is set in.
///
/// WHY the model names it: the painter used to measure the title against the
/// column and, when it did not fit, repaint it one rung down -- so a row whose
/// label happened to be a few pixels wide stood in smaller type than the row
/// above it, and the sheet read as a page of inconsistent sizes. The plate's
/// stack reserves the body rung's line box either way, so the downgrade drew a
/// caption inside a body's box.
pub const SETTINGS_TITLE_RUNG: usize = theme::typography::BODY_SCALE;

/// The rung a plate's value is set in.
pub const SETTINGS_VALUE_RUNG: usize = theme::typography::BODY_SCALE;

/// The rung a plate's note is set in.
pub const SETTINGS_NOTE_RUNG: usize = theme::typography::CAPTION_SCALE;

// ---------------------------------------------------------------------------
// The viewfinder and the chips that float over it.
// ---------------------------------------------------------------------------

/// The gutter the viewfinder keeps from the edge of the surface.
pub const VIEWFINDER_GUTTER: usize = interos_std::design::spacing::GRID_24;

/// The viewfinder's left edge.
pub const VIEWFINDER_LEFT: usize = VIEWFINDER_GUTTER;

/// The viewfinder's top edge.
pub const VIEWFINDER_TOP: usize = 120;

/// The viewfinder's right edge.
pub const VIEWFINDER_RIGHT: usize = SURFACE_W - VIEWFINDER_GUTTER;

/// The viewfinder's bottom edge.
pub const VIEWFINDER_BOTTOM: usize = 1120;

/// The viewfinder's width.
pub const VIEWFINDER_WIDTH: usize = VIEWFINDER_RIGHT - VIEWFINDER_LEFT;

/// The viewfinder's height.
pub const VIEWFINDER_HEIGHT: usize = VIEWFINDER_BOTTOM - VIEWFINDER_TOP;

/// The viewfinder's corner radius: the platform's card radius.
///
/// WHY the chips are measured against it: the viewfinder's rounded corners
/// reach `VIEWFINDER_RADIUS` pixels in from its edges, so a chip that sits
/// inside that band is drawn inside the corner the same way a chip that
/// hangs over the edge is cut by it. A chip has to clear the radius, not
/// merely be inside the rectangle.
pub const VIEWFINDER_RADIUS: usize = interos_std::design::radius::CARD;

/// The width of a chip at the top of the viewfinder.
pub const CHIP_W: usize = 72;

/// The height of a chip at the top of the viewfinder.
pub const CHIP_H: usize = 48;

/// The gap between two chips.
pub const CHIP_GAP: usize = 12;

/// The chip row's left edge.
///
/// WHY it is a gutter in from the viewfinder's edge: the row used to start
/// twelve pixels from it, which put the first chip inside the viewfinder's
/// corner band, so the chip read as cut by the viewfinder's own border.
pub const CHIP_ROW_X0: usize = VIEWFINDER_LEFT + VIEWFINDER_GUTTER;

/// The chip row's top edge.
///
/// WHY it is clear of the viewfinder's top border by more than the corner
/// radius: the chips float over the frame, and a chip that starts inside
/// the corner band is drawn against the border it is supposed to float
/// above.
pub const CHIP_ROW_Y: usize = VIEWFINDER_TOP + 64;

/// The chip at `index`: 0 is the aspect ratio, 1 the framing grid, 2 the
/// self-timer.
///
/// WHY the chips live here and not in the renderer: the rectangle that is
/// drawn and the rectangle that is tapped have to be the same rectangle, and
/// a pair of copies of the same four numbers is a pair that can drift.
pub const fn chip_rect(index: usize) -> Rect {
    Rect::new(
        CHIP_ROW_X0 + index * (CHIP_W + CHIP_GAP),
        CHIP_ROW_Y,
        CHIP_W,
        CHIP_H,
    )
}

/// How many chips the viewfinder carries.
pub const CHIP_COUNT: usize = 3;

// ---------------------------------------------------------------------------
// The shutter, and the refusal caption that answers it.
// ---------------------------------------------------------------------------

/// The shutter's centre column.
pub const SHUTTER_CX: usize = SURFACE_W / 2;

/// The shutter's centre row.
pub const SHUTTER_CY: usize = 1310;

/// The shutter's outer ring.
pub const SHUTTER_RING_OUTER: usize = 64;

/// The shutter's inner ring.
pub const SHUTTER_RING_INNER: usize = 56;

/// The shutter's core.
pub const SHUTTER_R_CORE: usize = 48;

/// The rows the refusal caption's box needs at its rung, whatever its bytes are.
///
/// WHY read from the renderer and not from the rung's line box alone: a caption
/// is drawn by whichever face fits, and the bitmap cell's baseline can sit below
/// the outline ascent, which makes the inked band taller than `line_height`.
/// [`row_band`] is the union of the two, so this is the number the dock has to
/// hold rather than the number one of the two faces needs.
pub const SHUTTER_NOTICE_ROWS: usize =
    interos_std::design::outline_text::row_band(0, SHUTTER_NOTICE_SCALE).1;

/// The air between the caption's box and the shutter's outer ring.
pub const SHUTTER_NOTICE_AIR: usize = 4;

/// The top of the dock: the band the shutter and the refusal caption live in.
///
/// WHY derived from the caption's rung: the strip between the dock's top edge
/// and the shutter's ring is the caption's only room, and it used to be a
/// remembered 1210 while the caption was a remembered 36 rows -- so when the
/// ladder raised the body rung the caption no longer fitted and nothing at the
/// dock noticed until the test suite did. Deriving the edge from the band the
/// text needs means a future rung moves the dock instead of clipping the answer
/// the user caused.
pub const DOCK_Y0: usize =
    SHUTTER_CY - SHUTTER_RING_OUTER - SHUTTER_NOTICE_ROWS - SHUTTER_NOTICE_AIR;

/// The refusal caption's line box: in the dock, above the shutter's ring.
///
/// WHY it is here and not on the viewfinder: the caption answers a press of
/// the shutter, so it belongs next to the shutter, and the strip between
/// the dock's top edge and the shutter's outer ring is the only free space
/// there. That strip is [`SHUTTER_NOTICE_ROWS`] plus [`SHUTTER_NOTICE_AIR`]
/// by construction.
pub const SHUTTER_NOTICE_Y: usize = DOCK_Y0;

/// The type scale of the refusal caption: the body scale, so a refusal the
/// user caused cannot be missed.
pub const SHUTTER_NOTICE_SCALE: usize = 2;

/// The width the refusal caption is measured against.
pub const SHUTTER_NOTICE_MAX_W: usize = SURFACE_W - VIEWFINDER_GUTTER * 2;

/// The longest refusal caption the camera carries.
pub const SHUTTER_NOTICE_MAX: usize = 96;

/// Left x that centres `text` at `scale` inside `[x0, x0 + available)`.
///
/// WHY the measured width and not `font::measure`: a run is placed through
/// the outline face when it can be, and the outline face advances by its own
/// table rather than the bitmap cell's. Both widths are decided by
/// `outline_text::drawn_advance`, which mirrors the placement's own
/// refusals, so the centring uses the width of the run that is drawn.
///
/// WHY the rung the person chose: a caption centred on the platform's advance is
/// centred on a run the ladder no longer inks -- the plate grows but the word
/// drifts off its optical middle. At the authored level this is the ladder path,
/// byte for byte.
pub fn centred_x(text: &[u8], scale: usize, x0: usize, available: usize) -> usize {
    let lvl = draw_level();
    if lvl != PLATFORM_LEVEL {
        let w = run_w_at(text, scale, lvl);
        return x0 + available.saturating_sub(w) / 2;
    }
    let w = std::design::outline_text::drawn_advance(text, scale, Some(available))
        .unwrap_or_else(|| std::design::font::measure(text, scale));
    x0 + available.saturating_sub(w) / 2
}

// -----------------------------------------------------------------------------
// The person's text size, and the geometry that follows it
//
// WHY this block exists: every box below was authored as the platform's type
// ladder read at one hundred per cent, so a person who raised "Text size" in
// Settings got a *smaller* reading surface here -- the ink moves through the
// runtime helpers, the plate does not move at all. Settings writes the level,
// `DynamicType::publish` hands it to the renderer, and the faces under
// [`rows_at`] answer at it. A layout that moves its ink but keeps its box is the
// defect this app had; a layout that moves its box to a rung the renderer never
// got is the same defect mirrored, so the rung is read from the one cell the
// renderer draws from.
// -----------------------------------------------------------------------------

/// The text-size level every box in this file was authored against.
pub const PLATFORM_LEVEL: u8 = interos_std::design::font::TEXT_SCALE_NORMAL;

/// The highest level the ladder offers, and therefore the level the gates sweep
/// to. [`UserTextScale::from_u8`] clamps anything above it.
pub const MAX_LEVEL: u8 = 3;

/// The smallest rung the ladder carries; a walk that steps a label down to fit a
/// fixed plate never goes past it.
pub const MIN_RUNG: usize = 1;

/// The later of two layout edges.
///
/// WHY a helper rather than `Ord::max`: `max` is not callable in a `const`
/// context on this toolchain, and several edges here must fold at compile time.
pub const fn later(a: usize, b: usize) -> usize {
    if a > b {
        a
    } else {
        b
    }
}

/// The cap height a rung inks at on a level.
pub const fn cap_at(scale: usize, level: u8) -> u32 {
    interos_std::design::font::cap_for_user_scale(scale, level)
}

/// The line box a rung reserves on a level.
pub const fn line_at(scale: usize, level: u8) -> usize {
    interos_std::design::font::line_height_for_user_scale(scale, level)
}

/// The ascent a rung's baseline sits above its box's top on a level.
pub const fn ascent_at(scale: usize, level: u8) -> usize {
    interos_std::design::font::ascent_for_user_scale(scale, level)
}

/// The pixels-per-em a rung is rasterised at on a level.
pub const fn em_at(scale: usize, level: u8) -> u32 {
    interos_std::design::font::em_for_user_scale(scale, level)
}

/// The rows a run at `scale` can ink on `level`: [`outline_text::row_band`]'s
/// union of the outline line box and the bitmap cell, asked at a level rather
/// than at the ladder.
///
/// WHY a restatement and not a call: `row_band` is a `const fn` of the ladder, so
/// a plate sized from it can never see the setting. The walk below is the same
/// one -- the lower of the two faces' baselines, the deeper of the two faces'
/// bottoms -- with every metric read through the `*_user_scale` faces, and the
/// gate beside the identity checks proves the two answers agree at the level they
/// were authored at.
pub const fn rows_at(scale: usize, level: u8) -> usize {
    if scale == 0 {
        return 0;
    }
    let em = em_at(scale, level);
    let (outline_ascent, outline_descent) =
        interos_std::design::typeface::line_metrics(em);
    let outline_ascent = outline_ascent as usize;
    let outline_descent = outline_descent as usize;
    let line_ascent = ascent_at(scale, level);
    let baseline = later(outline_ascent, line_ascent);
    later(baseline + outline_descent, line_at(scale, level))
}

/// An authored vertical stride between two stacked runs, grown by the rows the
/// rung's box gained above the level the stride was authored at.
pub const fn grown(stride: usize, scale: usize, level: u8) -> usize {
    stride + rows_at(scale, level).saturating_sub(rows_at(scale, PLATFORM_LEVEL))
}

/// A plate that reserves a rung's box plus its authored air, never below the
/// height it was drawn with.
pub const fn plate_at(authored: usize, scale: usize, level: u8) -> usize {
    let air = authored.saturating_sub(rows_at(scale, PLATFORM_LEVEL));
    later(authored, rows_at(scale, level) + air)
}

/// The rung `scale` takes inside a fixed plate `available` rows deep, at the
/// level this app draws at: the requested rung while its line box fits, and the
/// largest rung below it that the plate does hold. Never below [`MIN_RUNG`].
///
/// WHY a run-time walk: several of this app's controls are chrome of a fixed
/// height -- a 48-row capsule, the shutter's disc -- and a fixed plate cannot
/// hold a run at every selectable level. The rung gives way rather than the ink;
/// the gates beside each caller loop the four levels so the bound is still a
/// build-time fact.
pub fn fit_rung(want: usize, available: usize) -> usize {
    let level = level();
    fit_rung_at(want, available, level)
}

/// [`fit_rung`] as a `const fn` of a level, so a gate can sweep every level.
pub const fn fit_rung_at(want: usize, available: usize, level: u8) -> usize {
    let mut rung = want;
    while rung > MIN_RUNG && line_at(rung, level) > available {
        rung -= 1;
    }
    if rung < MIN_RUNG {
        MIN_RUNG
    } else {
        rung
    }
}

/// The rung `scale` takes inside a fixed plate `available` rows deep, measured by
/// the inked band a run reserves rather than its line box, at the level `lvl`
/// names: the requested rung while the band fits, and the largest rung below it
/// that the plate does hold. Never below [`MIN_RUNG`].
///
/// WHY a band rather than the line box: the dock's refusal strip is sized from
/// [`SHUTTER_NOTICE_ROWS`], which is the outline-and-bitmap union the renderer
/// actually inks (`outline_text::row_band`), not `line_height`. The SDK has no
/// level-aware `row_band_at`, so this restates the band's own walk (the same
/// union [`rows_at`] folds) against the plate, and the identity gate beside
/// [`rows_at`] proves it answers the platform's number at the authored level.
pub const fn fit_rung_rows_at(want: usize, available: usize, lvl: u8) -> usize {
    let mut rung = want;
    while rung > MIN_RUNG && rows_at(rung, lvl) > available {
        rung -= 1;
    }
    if rung < MIN_RUNG {
        MIN_RUNG
    } else {
        rung
    }
}

/// The rung `scale` takes inside a fixed plate `max_w` columns wide, at the
/// level this app draws at: the requested rung while its widest advance fits, and
/// the largest below it that does. Never below [`MIN_RUNG`].
pub fn fit_rung_width(want: usize, text: &[u8], max_w: usize) -> usize {
    let level = level();
    let mut rung = want;
    while rung > MIN_RUNG && run_w_at(text, rung, level) > max_w {
        rung -= 1;
    }
    if rung < MIN_RUNG {
        MIN_RUNG
    } else {
        rung
    }
}

/// The advance a run takes at `scale`, at the level `lvl` names.
pub fn run_w_at(text: &[u8], scale: usize, lvl: u8) -> usize {
    if scale == 0 || text.is_empty() {
        return 0;
    }
    if lvl == PLATFORM_LEVEL {
        return interos_std::design::font::measure(text, scale);
    }
    let w = interos_std::design::font::measure_user_scale(text, scale, lvl);
    if w == 0 {
        interos_std::design::font::measure(text, scale)
    } else {
        w
    }
}

/// The advance a run takes at `scale`, at the level this app draws at.
#[inline]
pub fn run_w(text: &[u8], scale: usize) -> usize {
    run_w_at(text, scale, level())
}

/// `font::ellipsize` at the rung the person chose.
///
/// WHY a restatement and not a call: `font::ellipsize` spends its trim budget in
/// the ladder's em, so a run cut to a lane at one rung and inked at another is an
/// ellipsis past the plate's edge. At the authored level this *is*
/// `font::ellipsize`, byte for byte.
pub fn ellipsize_run(text: &[u8], scale: usize, max_w: usize, out: &mut [u8]) -> usize {
    let lvl = level();
    if lvl == PLATFORM_LEVEL {
        return interos_std::design::font::ellipsize(text, scale, max_w, out);
    }
    if text.is_empty() || scale == 0 {
        return 0;
    }
    if run_w_at(text, scale, lvl) <= max_w {
        let len = text.len().min(out.len());
        out[..len].copy_from_slice(&text[..len]);
        return len;
    }
    const DOTS: &[u8] = b"...";
    let dots_w = run_w_at(DOTS, scale, lvl);
    if dots_w > max_w || out.len() < 3 {
        return 0;
    }
    let em = em_at(scale, lvl);
    let mut used = 0usize;
    let mut fit = 0usize;
    for (i, &c) in text.iter().enumerate() {
        let mut adv = interos_std::design::typeface::advance_px(c, em) as usize;
        if adv == 0 {
            adv = interos_std::design::font::advance_of(c, false) * scale;
        }
        if used + adv > max_w - dots_w || fit + 1 + 3 > out.len() {
            break;
        }
        used += adv;
        fit = i + 1;
    }
    out[..fit].copy_from_slice(&text[..fit]);
    out[fit..fit + 3].copy_from_slice(DOTS);
    fit + 3
}

/// The advance of the run the surface can actually draw, at this app's rung.
///
/// WHY it restates [`outline_text::drawn_advance`]: that function folds `scale`
/// through the platform's em, so a run centred with it at a raised rung is centred
/// on the width the painter no longer inks. At the authored level this is the
/// platform's answer.
pub fn run_drawn_advance(text: &[u8], scale: usize, available: usize) -> usize {
    let lvl = level();
    if lvl == PLATFORM_LEVEL {
        return std::design::outline_text::drawn_advance(text, scale, Some(available))
            .unwrap_or_else(|| interos_std::design::font::measure(text, scale));
    }
    let w = run_w_at(text, scale, lvl);
    if w > available {
        interos_std::design::font::measure(text, scale)
    } else {
        w
    }
}

/// The rows the refusal caption may ink: the whole strip between the dock's top
/// edge and the shutter's outer ring, the caption's air included.
///
/// WHY the air is spent: `DOCK_Y0` is derived as the ring's upper edge less the
/// caption's band less [`SHUTTER_NOTICE_AIR`], so the room the caption owns is the
/// authored band plus its four rows of air. Spending the air is the cheapest give
/// the dock has, and the mode carousel stands a few rows above the strip, so the
/// room cannot open further than that.
pub const SHUTTER_NOTICE_ROOM: usize = SHUTTER_CY - SHUTTER_RING_OUTER - DOCK_Y0;

/// The rung the shutter's refusal caption draws at on `lvl`: the body rung while
/// the room between the dock's top edge and the shutter's ring holds its inked
/// band, else the largest rung below it the room does hold.
///
/// WHY stepped rather than grown: the caption lives in that room and nowhere else
/// -- [`SHUTTER_NOTICE_ROWS`] plus [`SHUTTER_NOTICE_AIR`] by construction -- and
/// the room cannot open to hold a taller run without moving the dock into the mode
/// carousel. The rung gives way rather than the ink, measured by the band the
/// renderer inks ([`rows_at`]), not the line box alone. At the authored level this
/// is the body rung unchanged.
pub const fn shutter_notice_rung_at(lvl: u8) -> usize {
    fit_rung_rows_at(SHUTTER_NOTICE_SCALE, SHUTTER_NOTICE_ROOM, lvl)
}
pub fn shutter_notice_rung() -> usize {
    shutter_notice_rung_live()
}

// WHY swept over every selectable level: the strip is a fixed plate, so the honest
// fact is that the rung the notice answers never inks past it. The band a stepped
// rung reserves may overrun the room only from the ladder's floor rung, and only by
// one row -- measured, rung 1 stands 44 rows at Extra Large against a 43-row room
// and no smaller rung exists -- and [`shutter_notice_band`] clamps the plate to the
// room so that row is refused rather than inked over the ring. At the authored level
// the wanted body rung stands and its band is the strip unchanged, so adopting the
// setting moves no pixel for a reader who never raised it.
const _: () = assert!(shutter_notice_holds_at_every_level());
const fn shutter_notice_holds_at_every_level() -> bool {
    let mut level = 0;
    while level <= MAX_LEVEL as usize {
        let lvl = level as u8;
        let rung = shutter_notice_rung_at(lvl);
        if rung < MIN_RUNG || rung > SHUTTER_NOTICE_SCALE {
            return false;
        }
        let band = rows_at(rung, lvl);
        if band > SHUTTER_NOTICE_ROOM && rung != MIN_RUNG {
            return false;
        }
        if band > SHUTTER_NOTICE_ROOM + 1 {
            return false;
        }
        if lvl == PLATFORM_LEVEL && (rung != SHUTTER_NOTICE_SCALE || band != SHUTTER_NOTICE_ROWS)
        {
            return false;
        }
        level += 1;
    }
    true
}


// -----------------------------------------------------------------------------
// The rung the painter and the hit test both ask for
// -----------------------------------------------------------------------------

// The rung's metrics, resolved once per rung rather than once per pixel.
//
// WHY the cells: every number below is a chain of integer divisions -- the em
// from the cap height, the ascent and descent from the em -- and QEMU's TCG
// makes each division a helper call. A painter asks them for most of the pixels
// it writes, so asked per pixel the walk costs more than the ink and the frame
// never reaches scanout; see `6923b1ab perf(file_manager)`. The answer cannot
// move until the person moves "Text size", so it is carried in one cell per
// kind with the level it was resolved at stamped in its top byte: a hit is one
// load and a shift, and the walk runs once per setting change.
//
// WHY a byte per rung: the ladder's tallest box (rung seven at Extra Large)
// stands 210 rows, so one rung's metric fits a byte and eight rungs fit a word.
// A rung past the ladder's table is not carried here and is answered by the
// walk, which is what it did before this block existed.
type MetricCell = core::sync::atomic::AtomicUsize;
const STALE: usize = 0xFF << 24;
const RUNG_TABLE: usize = 8;

/// `cell`, resolved at the level the renderer draws at, by `at`.
///
/// WHY the level travels inside the value: a cell that only holds a number
/// cannot say which rung it was answered for, and a painter that repaints after
/// a setting change would keep the stale box. The top byte is the level, the
/// rest is the answer.
#[inline(always)]
pub fn rung_cell(cell: &MetricCell, at: fn(u8) -> usize) -> usize {
    let lvl = draw_level();
    let held = cell.load(core::sync::atomic::Ordering::Acquire);
    if held >> 24 == lvl as usize {
        return held & 0x00FF_FFFF;
    }
    let value = at(lvl);
    cell.store(((lvl as usize) << 24) | value, core::sync::atomic::Ordering::Release);
    value
}

static CAP_CELLS: [MetricCell; RUNG_TABLE] = [
    MetricCell::new(STALE), MetricCell::new(STALE), MetricCell::new(STALE), MetricCell::new(STALE),
    MetricCell::new(STALE), MetricCell::new(STALE), MetricCell::new(STALE), MetricCell::new(STALE),
];
static LINE_CELLS: [MetricCell; RUNG_TABLE] = [
    MetricCell::new(STALE), MetricCell::new(STALE), MetricCell::new(STALE), MetricCell::new(STALE),
    MetricCell::new(STALE), MetricCell::new(STALE), MetricCell::new(STALE), MetricCell::new(STALE),
];
static ASCENT_CELLS: [MetricCell; RUNG_TABLE] = [
    MetricCell::new(STALE), MetricCell::new(STALE), MetricCell::new(STALE), MetricCell::new(STALE),
    MetricCell::new(STALE), MetricCell::new(STALE), MetricCell::new(STALE), MetricCell::new(STALE),
];
static ROWS_CELLS: [MetricCell; RUNG_TABLE] = [
    MetricCell::new(STALE), MetricCell::new(STALE), MetricCell::new(STALE), MetricCell::new(STALE),
    MetricCell::new(STALE), MetricCell::new(STALE), MetricCell::new(STALE), MetricCell::new(STALE),
];
static NOTICE_RUNG_CELL: MetricCell = MetricCell::new(STALE);
static NOTICE_BAND_CELL: MetricCell = MetricCell::new(STALE);

/// One rung's metric, walked only when the rung the person chose moved.
#[inline(always)]
fn metric(cells: &[MetricCell; RUNG_TABLE], scale: usize, at: fn(usize, u8) -> usize) -> usize {
    if scale >= RUNG_TABLE {
        return at(scale, draw_level());
    }
    let cell = &cells[scale];
    let lvl = draw_level();
    let held = cell.load(core::sync::atomic::Ordering::Acquire);
    if held >> 24 == lvl as usize {
        return held & 0x00FF_FFFF;
    }
    let value = at(scale, lvl);
    cell.store(((lvl as usize) << 24) | value, core::sync::atomic::Ordering::Release);
    value
}

/// [`cap_at`] in the width the cells carry.
fn cap_rows_at(scale: usize, lvl: u8) -> usize {
    cap_at(scale, lvl) as usize
}

/// The line box a run at `scale` reserves at this app's rung.
#[inline]
pub fn run_line(scale: usize) -> usize {
    metric(&LINE_CELLS, scale, line_at)
}

/// The rows a run at `scale` can ink at this app's rung.
#[inline]
pub fn run_rows(scale: usize) -> usize {
    metric(&ROWS_CELLS, scale, rows_at)
}

/// The cap height a run at `scale` is rasterised at, at this app's rung.
#[inline]
pub fn run_cap(scale: usize) -> u32 {
    metric(&CAP_CELLS, scale, cap_rows_at) as u32
}

/// The ascent a run at `scale` reserves at this app's rung.
#[inline]
pub fn run_ascent(scale: usize) -> usize {
    metric(&ASCENT_CELLS, scale, ascent_at)
}

/// The rows the caption may ink, and the rung it inks them at.
#[inline]
pub fn shutter_notice_rung_live() -> usize {
    rung_cell(&NOTICE_RUNG_CELL, shutter_notice_rung_at)
}

/// The band the stepped caption reserves, clamped to the room the dock holds.
#[inline]
pub fn shutter_notice_band() -> usize {
    rung_cell(&NOTICE_BAND_CELL, |lvl| {
        rows_at(shutter_notice_rung_at(lvl), lvl).min(SHUTTER_NOTICE_ROOM)
    })
}

/// The rung this process draws at: the level `publish_text_scale` handed the
/// renderer from `display.text_scale`, clamped by [`UserTextScale::from_u8`] to
/// the ladder's largest selectable rung.
///
/// WHY no further downward clamp: every control the camera drops on the fixed
/// 1440-row panel is either chrome of a fixed height -- a 48-row capsule, the
/// shutter's disc, the dock strip -- whose label the renderer steps down with
/// [`fit_rung`], or a stack that grows inside a bound the ladder never overruns:
/// the status card is centred on a 680-row letterbox and its tallest derived
/// stand is 202 rows, and the settings sheet paginates, so a taller plate turns
/// a page rather than running under the mode strip. The panel answers every
/// selectable rung, so the person's rung is drawn as asked and nothing gives.
#[inline]
pub fn draw_level() -> u8 {
    interos_std::design::font::user_text_scale()
}

/// The same value the geometry helpers below read.
#[inline]
fn level() -> u8 {
    draw_level()
}

// WHY checked here: the derivations above are only a faithful restatement of the
// ladder if every rung answers the platform's own number at the platform's level.
const _: () = assert!(rows_at(theme::typography::BODY_SCALE, PLATFORM_LEVEL)
    == interos_std::design::outline_text::row_band(0, theme::typography::BODY_SCALE).1);
const _: () = assert!(rows_at(theme::typography::CAPTION_SCALE, PLATFORM_LEVEL)
    == interos_std::design::outline_text::row_band(0, theme::typography::CAPTION_SCALE).1);
const _: () = assert!(line_at(theme::typography::BODY_SCALE, PLATFORM_LEVEL)
    == interos_std::design::font::line_height(theme::typography::BODY_SCALE));
const _: () = assert!(cap_at(theme::typography::BODY_SCALE, PLATFORM_LEVEL)
    == interos_std::design::font::cap_for_scale(theme::typography::BODY_SCALE));
const _: () = assert!(ascent_at(theme::typography::BODY_SCALE, PLATFORM_LEVEL)
    == interos_std::design::font::ascent_for_scale(theme::typography::BODY_SCALE));

// WHY swept: the rung step-down is the half of this change that keeps a raised
// rung from spilling a fixed plate's ink, so it is checked against every level a
// renderer can be handed -- the rung it answers never drops below the ladder's
// smallest, and at the authored level every wanted rung stands unchanged (a fixed
// plate that held its rung at one hundred per cent still holds it, so adopting the
// setting moves no pixel for a reader who never raised it).
const _: () = assert!(fit_rung_holds_at_every_level());
const fn fit_rung_holds_at_every_level() -> bool {
    // The body rung a 48-row capsule, the shutter's disc and the dock strip carry.
    let want = theme::typography::BODY_SCALE;
    // The authored strip every fixed plate holds it at one hundred per cent.
    let authored = rows_at(want, PLATFORM_LEVEL);
    let mut level = 0;
    while level <= MAX_LEVEL as usize {
        let lvl = level as u8;
        // Never below the smallest rung.
        if fit_rung_at(want, authored, lvl) < MIN_RUNG {
            return false;
        }
        // The rung only ever steps down, never up past what was asked.
        if fit_rung_at(want, authored, lvl) > want {
            return false;
        }
        // At the authored level the wanted rung stands: the plate was drawn to
        // hold it, so `line_at(want, PLATFORM) <= authored`.
        if lvl == PLATFORM_LEVEL && fit_rung_at(want, authored, lvl) != want {
            return false;
        }
        level += 1;
    }
    true
}

/// WHY swept: a plate that shrinks at Compact or a box that runs backwards at
/// Extra Large is invisible to an assertion written at the authored level alone.
const _: () = assert!(rows_monotone_up_to_max_level());
const fn rows_monotone_up_to_max_level() -> bool {
    let rungs = [
        theme::typography::CAPTION_SCALE,
        theme::typography::BODY_SCALE,
        theme::typography::SECTION_SCALE,
    ];
    let mut rung = 0;
    while rung < rungs.len() {
        let scale = rungs[rung];
        let mut level = 1usize;
        while level <= MAX_LEVEL as usize {
            if rows_at(scale, level as u8) < rows_at(scale, (level - 1) as u8) {
                return false;
            }
            level += 1;
        }
        rung += 1;
    }
    true
}

/// The byte offset a notice's reason splits at, so neither half loses a word.
///
/// WHY the split is at a word boundary and not at the column edge: the reason
/// is the platform's own answer to a call this app was refused, and a notice
/// that states a refusal with the refusal's own word cut off says the opposite
/// of what it means. The return value is where the first line ends, which is
/// the space between the two words; the second line starts after it, so the
/// space belongs to neither line. A reason that fits whole returns its own
/// length and there is no second line. A reason whose first word alone is
/// wider than the column has no boundary to break at and is returned whole --
/// the gate over the verdicts is what keeps that case from being reachable.
pub fn notice_break(text: &[u8], column: usize, scale: usize) -> usize {
    let lvl = draw_level();
    if run_w_at(text, scale, lvl) <= column {
        return text.len();
    }
    let mut split = 0usize;
    for (index, byte) in text.iter().enumerate() {
        if *byte == b' ' && run_w_at(&text[..index], scale, lvl) <= column {
            split = index;
        }
    }
    if split == 0 {
        return text.len();
    }
    split
}

/// Works out a frame's format from the geometry the platform reported and the
/// number of bytes it returned.
///
/// WHY this and not a claim: the capture call reports the frame's width and
/// its height but not its pixel format, so the only way to know how many
/// bytes per pixel the frame carries is to divide the bytes the platform
/// actually returned by the pixels the platform actually reported. A frame
/// whose length matches no known layout is refused rather than guessed at.
pub fn frame_format(width: u32, height: u32, payload_len: usize) -> Option<FrameFormat> {
    let pixels = (width as usize).checked_mul(height as usize)?;
    if pixels == 0 {
        return None;
    }
    if payload_len % pixels != 0 {
        return None;
    }
    match payload_len / pixels {
        4 => Some(FrameFormat::Rgba8888),
        3 => Some(FrameFormat::Rgb888),
        1 => Some(FrameFormat::Gray8),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// The file the camera writes.
// ---------------------------------------------------------------------------

/// The directory the camera writes its photos into.
///
/// WHY this directory: it is the directory the platform's media-type
/// sniffer and the gallery already look at, so a photo the camera writes is
/// a document the platform can describe and the gallery can open.
pub const PHOTO_DIR: &str = "/storage/photos";

/// The largest path the camera builds.
pub const PHOTO_PATH_MAX: usize = 48;

/// Writes the path of the `index`-th photo into `out` and returns its length.
///
/// WHY the camera does not seal its photos into a private sector: a sector
/// only the camera can read is a photograph the user cannot open. The photo
/// goes into the shared storage area under a name the platform's document
/// index can describe.
pub fn photo_path(index: u32, out: &mut [u8; PHOTO_PATH_MAX]) -> usize {
    let mut n = 0usize;
    n += copy_bytes(&mut out[n..], PHOTO_DIR.as_bytes());
    n += copy_bytes(&mut out[n..], b"/shot-");
    n += append_decimal(index, &mut out[n..]);
    n += copy_bytes(&mut out[n..], b".ppm");
    n
}

/// Appends a decimal number to `out` and returns how many bytes it wrote.
pub fn append_decimal(mut value: u32, out: &mut [u8]) -> usize {
    if value == 0 {
        if out.is_empty() {
            return 0;
        }
        out[0] = b'0';
        return 1;
    }
    let mut digits = [0u8; 10];
    let mut count = 0usize;
    while value > 0 && count < digits.len() {
        digits[count] = b'0' + (value % 10) as u8;
        value /= 10;
        count += 1;
    }
    let mut written = 0usize;
    let mut i = count;
    while i > 0 {
        i -= 1;
        if written >= out.len() {
            break;
        }
        out[written] = digits[i];
        written += 1;
    }
    written
}

/// The header of the file the camera writes.
///
/// WHY a PPM and not a private container: the platform's media-type sniffer
/// reads a document's own bytes, and "P6" is a format it recognises, so a
/// photo the camera writes is a document the platform can describe and the
/// gallery can open. A private header would have made every photo a
/// document the platform could not classify.
///
/// WHY the header is built from the geometry that was captured: a header
/// that names a format the file does not hold is a lie the platform would
/// carry into the document index.
pub fn ppm_header(width: u32, height: u32, out: &mut [u8; 32]) -> usize {
    let mut n = 0usize;
    for byte in b"P6\n".iter() {
        if n >= out.len() {
            return n;
        }
        out[n] = *byte;
        n += 1;
    }
    n += append_decimal(width, &mut out[n..]);
    if n < out.len() {
        out[n] = b' ';
        n += 1;
    }
    n += append_decimal(height, &mut out[n..]);
    for byte in b"\n255\n".iter() {
        if n >= out.len() {
            return n;
        }
        out[n] = *byte;
        n += 1;
    }
    n
}

/// What became of a captured photo.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PublishState {
    /// The photo is on the device and the document index describes it.
    Published,
    /// The photo is on the device, but the document index did not take it.
    WrittenNotIndexed,
    /// The platform refused the write, with the code it answered.
    Refused(i64),
    /// No photo has been captured yet.
    None,
}

impl PublishState {
    /// The caption the settings surface shows.
    pub const fn caption(self) -> &'static [u8] {
        match self {
            PublishState::Published => b"Published in the document index",
            PublishState::WrittenNotIndexed => b"Written; the document index did not take it",
            PublishState::Refused(_) => b"The platform refused the write",
            PublishState::None => b"No photo captured yet",
        }
    }

    /// The platform's error code, when the platform refused the write.
    pub const fn error_code(self) -> Option<i64> {
        match self {
            PublishState::Refused(code) => Some(code),
            _ => None,
        }
    }
}

/// Decides what became of a photo from what the platform answered.
///
/// WHY this is a function and not a branch in the renderer: the difference
/// between "the photo is on the device", "the photo is on the device but the
/// index did not take it" and "there is no photo" is a fact about the
/// platform, and it is the fact the user is shown.
///
/// WHY the write's error code travels with the state: a refusal the user
/// cannot name is a refusal they cannot act on, and the code is the only
/// thing the platform gave.
pub fn publish_state(write: Result<(), i64>, indexed: bool) -> PublishState {
    match write {
        Err(code) => PublishState::Refused(code),
        Ok(()) if indexed => PublishState::Published,
        Ok(()) => PublishState::WrittenNotIndexed,
    }
}

/// What a press of the shutter produced.
///
/// WHY this is one value and not a success flag: "the platform refused the
/// capture", "the frame was captured but the photo did not reach storage" and
/// "the photo is on the device and the document index describes it" are three
/// different facts. A capture the platform refused and a capture that
/// succeeded but could not be saved are not the same event, and a screen that
/// called both of them a refusal would send the user after the wrong thing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CaptureOutcome {
    /// The frame was captured and the photo is on the device, described by the
    /// platform's document index.
    Published,
    /// The frame was captured and the photo is on the device, but the document
    /// index did not describe it.
    WrittenNotIndexed,
    /// The frame was captured, but the platform refused to write the photo.
    WriteRefused(i64),
    /// The platform refused the capture itself.
    CaptureRefused(i64),
    /// The camera HAL exposes no video stream, so there is no frame to take.
    NoVideoStream,
    /// The shutter has not been pressed yet.
    None,
}

impl CaptureOutcome {
    /// Whether the frame itself was captured.
    ///
    /// WHY this is asked and not inferred: a refusal to write still leaves a
    /// frame in the app's buffer, and a frame that was never taken does not.
    pub const fn captured(self) -> bool {
        matches!(
            self,
            CaptureOutcome::Published | CaptureOutcome::WrittenNotIndexed | CaptureOutcome::WriteRefused(_)
        )
    }

    /// Whether the photo reached the device and the document index.
    pub const fn published(self) -> bool {
        matches!(self, CaptureOutcome::Published)
    }

    /// The platform's error code, when the platform refused something.
    pub const fn error_code(self) -> Option<i64> {
        match self {
            CaptureOutcome::WriteRefused(code) | CaptureOutcome::CaptureRefused(code) => Some(code),
            _ => None,
        }
    }

    /// The line the log and the settings surface carry.
    pub const fn caption(self) -> &'static [u8] {
        match self {
            CaptureOutcome::Published => {
                b"The photo is on the device and the index describes it"
            }
            CaptureOutcome::WrittenNotIndexed => {
                b"The photo is on the device; the index did not describe it"
            }
            CaptureOutcome::WriteRefused(_) => {
                b"The frame was captured; the write was refused"
            }
            CaptureOutcome::CaptureRefused(_) => b"The platform refused the capture",
            CaptureOutcome::NoVideoStream => b"The camera HAL exposes no video stream",
            CaptureOutcome::None => b"No capture yet",
        }
    }
}

// ---------------------------------------------------------------------------
// The app.
// ---------------------------------------------------------------------------

/// The screens the camera shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CameraScreen {
    /// The viewfinder and the shutter.
    Viewfinder,
    /// The settings surface.
    Settings,
}

/// The rows the camera's settings surface shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettingsRow {
    /// The format the camera writes.
    Format,
    /// The image processing the next capture uses.
    Filter,
    /// The flash and torch state the hardware reported.
    Flash,
    /// Whether the framing guides are drawn.
    Guides,
    /// How many photos are on the device.
    Photos,
    /// Where the last photo was written.
    LastPhoto,
    /// What the last press of the shutter produced.
    LastCapture,
    /// Open the photo library.
    OpenLibrary,
    /// Close the settings.
    Close,
}

/// How many rows the settings surface shows.
pub const SETTINGS_ROWS: usize = 9;

/// The order the settings surface shows its rows in.
pub const SETTINGS_ROW_ORDER: [SettingsRow; SETTINGS_ROWS] = [
    SettingsRow::Format,
    SettingsRow::Filter,
    SettingsRow::Flash,
    SettingsRow::Guides,
    SettingsRow::Photos,
    SettingsRow::LastPhoto,
    SettingsRow::LastCapture,
    SettingsRow::OpenLibrary,
    SettingsRow::Close,
];

/// The smallest thumbnail dimension for the gallery disc preview.
pub const THUMBNAIL_DIM: usize = 32;

/// The camera's state.
pub struct CameraApp {
    /// The mode the shutter is in.
    pub mode: CameraMode,
    /// The flash state the app asks for.
    pub flash: FlashMode,
    /// Whether the flash/torch hardware backend answered a toggle request.
    ///
    /// WHY: this app holds neither `SystemControl` on the camera service object
    /// nor `HardwarePortIo` on port 0xCA00, so `camera_toggle_flash` is refused
    /// with PermissionDenied. The previous code discarded that error, kept a lit
    /// `Torch` state locally and rendered it as an active amber pill.
    pub flash_available: bool,
    /// The image processing the next capture uses.
    pub filter: CameraFilter,
    /// The exposure bias in stops.
    pub exposure_bias: i16,
    /// How many frames the camera has captured.
    pub total_photos: u32,
    /// How many of those frames are on the device as documents.
    pub photos_published: u32,
    /// What the platform has told the camera about the camera.
    pub status: CameraStatus,
    /// The error code the platform last answered with.
    pub last_error: i64,
    /// The geometry of the last captured frame.
    pub frame_label: [u8; 16],
    /// How many bytes of the geometry label are used.
    pub frame_label_len: usize,
    /// The shots readout in the top row.
    pub shots_label: [u8; 16],
    /// How many bytes of the shots readout are used.
    pub shots_label_len: usize,
    /// The path of the last photo written to the storage area.
    pub last_photo: [u8; PHOTO_PATH_MAX],
    /// How many bytes of the last photo's path are used.
    pub last_photo_len: usize,
    /// What became of the last publication.
    pub publish: PublishState,
    /// What the last press of the shutter produced.
    ///
    /// WHY this is kept beside `publish`: a capture the platform refused and a
    /// capture that succeeded but could not be written are different facts, and
    /// the settings surface has to be able to say which one happened.
    pub capture: CaptureOutcome,
    /// The code the platform answered the last document-index refresh with.
    ///
    /// WHY it is kept: a photo that is on the device but that the index does
    /// not describe is a photo the gallery cannot show, and the reason the
    /// index did not take it is a capability the app may not hold.
    pub index_error: i64,
    /// Whether the framing guides are drawn over the viewfinder.
    pub guides: bool,
    /// Viewfinder aspect ratio configuration (4:3 vs 16:9 vs 1:1).
    pub aspect_ratio: CameraAspectRatio,
    /// Tactile visual feedback timer ticks remaining after shutter press.
    pub shutter_feedback_ticks: u32,
    /// The refusal the last press of the shutter produced, in the platform's
    /// own words.
    ///
    /// WHY it is kept: the refusal used to reach the serial log only, so a
    /// press of the shutter that the platform refused changed nothing on the
    /// surface and the user was left with a control that looked broken.
    pub shutter_notice: [u8; SHUTTER_NOTICE_MAX],
    /// How many bytes of the refusal caption are used.
    pub shutter_notice_len: usize,
    /// Where the refusal caption starts.
    pub shutter_notice_x: usize,
    /// Self-timer countdown mode (Off, 3s, 10s).
    pub timer_mode: TimerMode,
    /// Self-timer countdown ticks remaining before capture.
    pub countdown_ticks: u32,
    /// Downsampled thumbnail preview of the most recently captured photo (RGB888).
    pub thumbnail: [u8; THUMBNAIL_DIM * THUMBNAIL_DIM * 3],
    /// Whether a valid thumbnail preview is ready to display.
    pub has_thumbnail: bool,
    /// Which screen the app is showing.
    pub screen: CameraScreen,
    /// The headline the viewfinder shows.
    pub head: [u8; 48],
    /// How many bytes of the headline are used.
    pub head_len: usize,
    /// The first line of the reason the camera is in this state.
    pub sub: [u8; 96],
    /// How many bytes of the reason's first line are used.
    pub sub_len: usize,
    /// The second line of the reason, when the reason needs two.
    pub sub2: [u8; 96],
    /// How many bytes of the reason's second line are used.
    pub sub2_len: usize,
    /// The values of the settings rows, pre-rendered.
    ///
    /// WHY they are pre-rendered: the renderer runs once per pixel of the
    /// surface, and formatting a value there would run tens of thousands of
    /// times for one row of text.
    pub settings_values: [[u8; 64]; SETTINGS_ROWS],
    /// How many bytes of each settings value are used.
    pub settings_value_lens: [usize; SETTINGS_ROWS],
    pub clock: [u8; 5],
    pub clock_ok: bool,
}

impl CameraApp {
    /// The camera's initial state.
    pub const fn new() -> Self {
        Self {
            mode: CameraMode::Photo,
            flash: FlashMode::Auto,
            flash_available: true,
            filter: CameraFilter::Normal,
            exposure_bias: 0,
            total_photos: 0,
            photos_published: 0,
            status: CameraStatus::AuthorityHeld,
            last_error: 0,
            frame_label: [0; 16],
            frame_label_len: 0,
            shots_label: [0; 16],
            shots_label_len: 0,
            last_photo: [0; PHOTO_PATH_MAX],
            last_photo_len: 0,
            publish: PublishState::None,
            capture: CaptureOutcome::None,
            index_error: 0,
            guides: false,
            aspect_ratio: CameraAspectRatio::Ratio4_3,
            shutter_feedback_ticks: 0,
            shutter_notice: [0; SHUTTER_NOTICE_MAX],
            shutter_notice_len: 0,
            shutter_notice_x: 0,
            timer_mode: TimerMode::Off,
            countdown_ticks: 0,
            thumbnail: [0; THUMBNAIL_DIM * THUMBNAIL_DIM * 3],
            has_thumbnail: false,
            screen: CameraScreen::Viewfinder,
            head: [0; 48],
            head_len: 0,
            sub: [0; 96],
            sub_len: 0,
            sub2: [0; 96],
            sub2_len: 0,
            settings_values: [[0; 64]; SETTINGS_ROWS],
            settings_value_lens: [0; SETTINGS_ROWS],
            clock: [b' '; 5],
            clock_ok: false,
        }
    }

    /// Rebuilds the settings surface's values.
    ///
    /// WHY this is a step: the values name the format the camera writes, where
    /// the last photo went and what became of the last publication, and the
    /// renderer reads them instead of formatting them per pixel.
    pub fn refresh_settings_values(&mut self) {
        let mut scratch = [0u8; 64];
        for (index, row) in SETTINGS_ROW_ORDER.iter().enumerate() {
            let len = self.settings_row_value(*row, &mut scratch);
            self.settings_value_lens[index] = len;
            copy_bytes(&mut self.settings_values[index], &scratch[..len]);
        }
    }

    /// Toggles the flash mode (Auto -> On -> Off -> Auto) and commands
    /// the hardware backend if turning on.
    ///
    /// Returns `None` when the flash backend is absent or refused the request.
    /// In that case the local mode is forced back to `Off` so neither the model
    /// nor the UI can claim an emitting torch that no hardware produced.
    pub fn toggle_flash(&mut self) -> Option<bool> {
        self.flash = self.flash.next();
        match self.flash {
            FlashMode::On => {
                match std::camera_toggle_flash() {
                    Ok(new_state) => {
                        self.flash_available = true;
                        Some(new_state)
                    }
                    Err(_) => {
                        self.flash_available = false;
                        self.flash = FlashMode::Off;
                        None
                    }
                }
            }
            FlashMode::Auto => {
                self.flash_available = true;
                Some(true)
            }
            FlashMode::Off => {
                let _ = std::camera_toggle_flash();
                Some(false)
            }
        }
    }

    /// The video frame path.
    ///
    /// WHY there is no video stream here: the camera HAL exposes one frame
    /// capture and no stream source, so a video shutter press has nothing to
    /// deliver. The app says so rather than pretending a frame arrived.
    pub fn stream_video_frame(&mut self) -> CaptureOutcome {
        self.capture = CaptureOutcome::NoVideoStream;
        self.show_capture_outcome();
        CaptureOutcome::NoVideoStream
    }

    /// Asks the platform whether this app holds camera authority.
    ///
    /// WHY this asks about the authority and not about a sensor: the
    /// platform's answer to a call this app is not authorised to make is a
    /// refusal, and a refusal is the fact the user needs to see. Whether a
    /// sensor is bound is only knowable by asking for a frame, and asking
    /// for a frame the user did not ask for would take a photograph.
    pub fn probe_camera_authority(&mut self) {
        match std::camera_autofocus_state() {
            Ok(_) => {
                self.status = CameraStatus::AuthorityHeld;
                self.last_error = 0;
            }
            Err(code) => {
                self.status = status_from_error(code);
                self.last_error = code;
            }
        }
        self.refresh_status_text();
    }

    /// Rebuilds the viewfinder's text and the places it is drawn.
    ///
    /// WHY this is a step and not something the renderer does per pixel: the
    /// renderer is called once per pixel of the surface, and measuring a run
    /// there would run tens of thousands of times for one line of text. The
    /// text only changes when the platform's answer changes, so it is rebuilt
    /// when that happens and only read afterwards.
    pub fn refresh_status_text(&mut self) {
        self.head_len = copy_bytes(&mut self.head, self.status.headline());
        let mut reason = [0u8; 96];
        let reason_len = self.status_reason(&mut reason);
        let split = notice_break(&reason[..reason_len], STATUS_TEXT_W, STATUS_REASON_RUNG);
        self.sub_len = copy_bytes(&mut self.sub, &reason[..split]);
        let rest = if split < reason_len { split + 1 } else { split };
        self.sub2_len = copy_bytes(&mut self.sub2, &reason[rest..reason_len]);
    }

    /// Rebuilds the shots readout after the counter moves.
    pub fn refresh_shots_label(&mut self) {
        let mut buf = [0u8; 16];
        let mut n = append_decimal(self.total_photos, &mut buf);
        let suffix: &[u8] = if self.total_photos == 1 { b" SHOT" } else { b" SHOTS" };
        n += copy_bytes(&mut buf[n..], suffix);
        self.shots_label_len = copy_bytes(&mut self.shots_label, &buf[..n]);
    }

    /// Records the refusal the shutter produced, so the surface can show it.
    ///
    /// WHY the caption is measured here and not in the renderer: the renderer
    /// is called once per pixel of the surface, and measuring a run there
    /// would run tens of thousands of times for one line of text.
    pub fn set_shutter_notice(&mut self, text: &[u8]) {
        let len = copy_bytes(&mut self.shutter_notice, text);
        self.shutter_notice_len = len;
        self.shutter_notice_x = centred_x(
            &self.shutter_notice[..len],
            shutter_notice_rung(),
            VIEWFINDER_LEFT,
            SHUTTER_NOTICE_MAX_W,
        );
    }

    /// Clears the refusal caption.
    pub fn clear_shutter_notice(&mut self) {
        self.shutter_notice_len = 0;
    }

    /// The colour the caption is painted in.
    ///
    /// WHY it comes from the platform's verdict and not from the caption: a
    /// refusal and a capture the index simply did not describe are different
    /// facts, and the user reads the colour before the words. Painting the
    /// amber line red would say the platform refused something it accepted.
    pub fn shutter_notice_ink(&self) -> u32 {
        use crate::theme::chrome;
        match self.status {
            CameraStatus::AuthorityRefused | CameraStatus::Failed => chrome::REFUSED,
            CameraStatus::NoSensor | CameraStatus::AuthorityHeld => chrome::CAUTION,
        }
    }

    /// Puts the answer the last press of the shutter produced onto the surface.
    ///
    /// WHY it is one rule over the whole outcome rather than a call at each
    /// refusal: a caption that is only cleared by the branches that remember
    /// to clear it is a caption that outlives the press it answered, and the
    /// user then reads a refusal under a shutter that has since worked.
    ///
    /// WHY a published photo says nothing: the thumbnail disc fills with the
    /// frame and the counter climbs, so the strip the caption uses would only
    /// repeat what the two already show.
    fn show_capture_outcome(&mut self) {
        if matches!(self.capture, CaptureOutcome::Published | CaptureOutcome::None) {
            self.clear_shutter_notice();
        } else {
            self.set_shutter_notice(self.capture.caption());
        }
    }

    /// Records why the shutter refused, in the platform's own words.
    ///
    /// WHY the headline and not a sentence of the app's own: the platform is
    /// what refused the capture, and the app has no more to add than the
    /// platform said.
    pub fn refuse_shutter(&mut self) {
        let headline = self.status.headline();
        self.set_shutter_notice(headline);
    }

    /// The pixel the refusal caption paints at `(x, y)`, if any.
    ///
    /// WHY this is a step in the model and not in the renderer: the caption is
    /// the answer the user gets for a press of the shutter, and the answer
    /// has to be the same on every surface the camera draws.
    pub fn shutter_notice_pixel(&self, x: usize, y: usize, ink: u32, bg: u32) -> Option<u32> {
        if self.shutter_notice_len == 0 {
            return None;
        }
        let text = &self.shutter_notice[..self.shutter_notice_len];
        let rung = shutter_notice_rung();
        let lvl = draw_level();
        if lvl == PLATFORM_LEVEL {
            let (bx, bt, bw, bh) = std::design::outline_text::run_box(
                self.shutter_notice_x,
                SHUTTER_NOTICE_Y,
                rung,
                text,
            );
            if x < bx || x - bx >= bw || y < bt || y - bt >= bh {
                return None;
            }
            let run = std::design::outline_text::place_run(
                self.shutter_notice_x,
                SHUTTER_NOTICE_Y,
                rung,
                text,
                Some(SHUTTER_NOTICE_MAX_W),
                false,
            )?;
            return run.pixel(x, y, ink, bg);
        }
        // The caption at the rung the strip holds, inked at the level the person
        // chose: the outline face at the rung's cap height, the bitmap face filling
        // in when the outline refuses, and the band grown to the rung so a raised
        // level's descenders are not cut by the authored box. WHY the band is asked
        // from [`shutter_notice_band`] and not from the rung's own rows: the room is
        // the plate, and at Extra Large the floor rung's band is one row deeper than
        // it -- refused here, so no caption ink ever reaches the ring's edge.
        let cap = run_cap(rung);
        let band = shutter_notice_band();
        if let Some(run) = std::design::outline_text::place_run_cap(
            self.shutter_notice_x,
            SHUTTER_NOTICE_Y,
            cap,
            text,
            Some(SHUTTER_NOTICE_MAX_W),
            false,
        ) {
            let (rx, rtop, rw, rh) = run.bounds();
            if x >= rx && x - rx < rw && y >= rtop && y - rtop < rh && y < SHUTTER_NOTICE_Y + band {
                return run.pixel(x, y, ink, bg);
            }
            return None;
        }
        if x >= self.shutter_notice_x
            && x < self.shutter_notice_x + SHUTTER_NOTICE_MAX_W
            && y >= SHUTTER_NOTICE_Y
            && y < SHUTTER_NOTICE_Y + band
        {
            return interos_std::design::monochrome::text_px(
                x,
                y,
                self.shutter_notice_x,
                SHUTTER_NOTICE_Y,
                cap,
                text,
                ink,
                bg,
                false,
            );
        }
        None
    }

    /// The label the capture readout carries.
    ///
    /// WHY the geometry wins once a frame exists: after a capture the
    /// platform has told the app how big the frame was, and that is a more
    /// useful thing to read than the fact that no frame has been taken.
    pub fn frame_readout(&self) -> (&[u8], usize) {
        if self.total_photos > 0 && self.frame_label_len > 0 {
            (&self.frame_label[..self.frame_label_len], self.frame_label_len)
        } else {
            (self.status.label(), self.status.label().len())
        }
    }

    /// The line under the viewfinder's headline, with the error code in it
    /// when the platform refused the capture.
    pub fn status_reason(&self, out: &mut [u8; 96]) -> usize {
        let mut n = 0usize;
        for byte in self.status.reason().iter() {
            if n >= out.len() {
                return n;
            }
            out[n] = *byte;
            n += 1;
        }
        if self.status == CameraStatus::Failed && self.last_error != 0 {
            if n < out.len() {
                out[n] = b' ';
                n += 1;
            }
            if n < out.len() {
                out[n] = b'(';
                n += 1;
            }
            if self.last_error < 0 {
                if n < out.len() {
                    out[n] = b'-';
                    n += 1;
                }
                n += append_decimal((-(self.last_error as i64)) as u32, &mut out[n..]);
            } else {
                n += append_decimal(self.last_error as u32, &mut out[n..]);
            }
            if n < out.len() {
                out[n] = b')';
                n += 1;
            }
        }
        n
    }

    /// Takes a photograph and publishes it.
    ///
    /// WHY the photo is written into the shared storage area: a photograph
    /// that only the camera can read is a photograph the user cannot open, so
    /// the capture goes into the document index like every other document on
    /// the device.
    ///
    /// WHY the answer is a [`CaptureOutcome`] and not a `Result`: a refusal to
    /// capture and a capture that could not be saved are different facts, and
    /// the caller has to be able to say which one happened.
    pub fn capture_still_photo(&mut self) -> CaptureOutcome {
        let mut meta = [0u32; 4];
        let payload_len = {
            let buffer = unsafe { &mut *addr_of_mut!(FRAME_BUFFER) };
            match std::camera_capture(buffer, &mut meta) {
                Ok(n) => n,
                Err(code) => {
                    self.status = status_from_error(code);
                    self.last_error = code;
                    self.refresh_status_text();
                    self.capture = CaptureOutcome::CaptureRefused(code);
                    self.show_capture_outcome();
                    return self.capture;
                }
            }
        };

        let width = meta[0];
        let height = meta[1];
        let Some(format) = frame_format(width, height, payload_len) else {
            self.status = CameraStatus::Failed;
            self.last_error = ERR_INVALID_ARGUMENTS;
            self.refresh_status_text();
            self.capture = CaptureOutcome::CaptureRefused(ERR_INVALID_ARGUMENTS);
            self.show_capture_outcome();
            return self.capture;
        };

        {
            let buffer = unsafe { &mut *addr_of_mut!(FRAME_BUFFER) };
            apply_isp_tone_curve(
                self.filter,
                self.exposure_bias,
                format.bytes_per_pixel(),
                &mut buffer[..payload_len],
            );
        }

        self.generate_thumbnail(width, height, format);

        self.total_photos = self.total_photos.saturating_add(1);
        self.frame_label_len = 0;
        self.frame_label_len += append_decimal(width, &mut self.frame_label[self.frame_label_len..]);
        if self.frame_label_len < self.frame_label.len() {
            self.frame_label[self.frame_label_len] = b'x';
            self.frame_label_len += 1;
        }
        self.frame_label_len += append_decimal(height, &mut self.frame_label[self.frame_label_len..]);
        self.status = CameraStatus::AuthorityHeld;
        self.last_error = 0;
        self.refresh_shots_label();
        self.refresh_status_text();

        self.capture = match self.publish_photo(width, height, format) {
            Ok(true) => CaptureOutcome::Published,
            Ok(false) => CaptureOutcome::WrittenNotIndexed,
            Err(code) => CaptureOutcome::WriteRefused(code),
        };
        let outcome = self.capture;
        self.show_capture_outcome();
        outcome
    }

    /// Downsamples the captured frame to a circular preview thumbnail for the gallery disc.
    pub fn generate_thumbnail(&mut self, width: u32, height: u32, format: FrameFormat) {
        if width == 0 || height == 0 {
            return;
        }
        let source = unsafe { &*addr_of_mut!(FRAME_BUFFER) };
        let channels = format.bytes_per_pixel();
        let total_pixels = (width as usize) * (height as usize);
        if source.len() < total_pixels * channels {
            return;
        }

        let w = width as usize;
        let h = height as usize;
        for ty in 0..THUMBNAIL_DIM {
            let sy = (ty * h) / THUMBNAIL_DIM;
            for tx in 0..THUMBNAIL_DIM {
                let sx = (tx * w) / THUMBNAIL_DIM;
                let base = (sy * w + sx) * channels;
                if base + channels <= source.len() {
                    let (r, g, b) = match format {
                        FrameFormat::Rgba8888 | FrameFormat::Rgb888 => {
                            (source[base], source[base + 1], source[base + 2])
                        }
                        FrameFormat::Gray8 => (source[base], source[base], source[base]),
                    };
                    let out_idx = (ty * THUMBNAIL_DIM + tx) * 3;
                    self.thumbnail[out_idx] = r;
                    self.thumbnail[out_idx + 1] = g;
                    self.thumbnail[out_idx + 2] = b;
                }
            }
        }
        self.has_thumbnail = true;
    }

    /// Hands the last photo to the app that opens images, or launches the Gallery app.
    ///
    /// WHY the platform's intent registry and service package launch: the gallery
    /// declares that it handles this media type, so intent dispatch opens the exact
    /// photo. If that is refused or unavailable, direct service package launch opens
    /// the Gallery so the user can inspect their collection.
    pub fn open_photo_library(&self) -> Result<u64, i64> {
        // Try intent dispatch with the photo path first if one was captured
        if self.last_photo_len > 0 {
            if let Ok(path) = core::str::from_utf8(&self.last_photo[..self.last_photo_len]) {
                if let Ok(task_id) = std::content::intent_dispatch(std::content::IntentKind::Mime, PHOTO_MIME, path) {
                    return Ok(task_id);
                }
            }
        }
        // Fall back to direct service package launch
        if let Ok(task_id) = std::service_package_launch("gallery") {
            return Ok(task_id);
        }
        if let Ok(task_id) = std::service_package_launch("/packages/gallery.rapp") {
            return Ok(task_id);
        }
        if self.last_photo_len == 0 {
            Err(ERR_NO_PHOTO)
        } else {
            Err(ERR_PERMISSION_DENIED)
        }
    }

    /// Writes the captured frame into the shared storage area and asks the
    /// platform to describe it.
    ///
    /// WHY `Ok(true)` and `Ok(false)` are different answers: the file being
    /// on the device and the document index describing it are two facts, and a
    /// photo the gallery cannot find is not a photo the user can open. `Err`
    /// carries the code the platform refused the write with.
    pub fn publish_photo(&mut self, width: u32, height: u32, format: FrameFormat) -> Result<bool, i64> {
        let mut path = [0u8; PHOTO_PATH_MAX];
        let path_len = photo_path(self.photos_published + 1, &mut path);
        let path_str = match core::str::from_utf8(&path[..path_len]) {
            Ok(s) => s,
            Err(_) => return Err(ERR_INVALID_ARGUMENTS),
        };

        let fd = match std::fs_open(path_str, std::FS_WRITE) {
            Ok(fd) => fd,
            Err(code) => {
                self.publish = PublishState::Refused(code);
                return Err(code);
            }
        };

        let mut header = [0u8; 32];
        let header_len = ppm_header(width, height, &mut header);
        if let Err(code) = std::fs_write(fd, &header[..header_len]) {
            let _ = std::fs_close(fd);
            self.publish = PublishState::Refused(code);
            return Err(code);
        }

        if let Err(code) = self.write_pixels(fd, width, height, format) {
            let _ = std::fs_close(fd);
            self.publish = PublishState::Refused(code);
            return Err(code);
        }

        if let Err(code) = std::fs_close(fd) {
            self.publish = PublishState::Refused(code);
            return Err(code);
        }

        copy_bytes(&mut self.last_photo, &path[..path_len]);
        self.last_photo_len = path_len;
        self.photos_published = self.photos_published.saturating_add(1);

        // The index is rebuilt from the storage area, so a photo that was
        // written is a photo the platform can find. The refresh is asked for
        // explicitly so the photo does not wait for the next index query.
        //
        // WHY the refusal is remembered: a camera that holds no index authority
        // gets a photo on the device that nothing else can find, and the user
        // has to be told that is why the gallery cannot show the photo.
        let refreshed = match std::content::content_index_refresh() {
            Ok(_) => true,
            Err(code) => {
                self.index_error = code;
                false
            }
        };
        let mut raw = [0u8; 2048];
        let found = std::content::content_index_find_path(path_str, &mut raw).is_some();
        let indexed = refreshed && found;
        self.publish = publish_state(Ok(()), indexed);
        if indexed {
            Ok(true)
        } else {
            Ok(false)
        }
    }

    /// Writes the frame's pixels into the open file, converting whatever the
    /// platform returned into the three channels a PPM holds.
    fn write_pixels(
        &mut self,
        fd: u32,
        width: u32,
        height: u32,
        format: FrameFormat,
    ) -> Result<(), i64> {
        let source = unsafe { &*addr_of_mut!(FRAME_BUFFER) };
        let channels = format.bytes_per_pixel();
        let mut scratch = [0u8; 1024];
        let mut written_rows = 0u32;
        let row_bytes = (width as usize) * channels;
        while written_rows < height {
            let row = written_rows as usize;
            let start = row * row_bytes;
            if start >= source.len() {
                break;
            }
            let end = core::cmp::min(start + row_bytes, source.len());
            let row_pixels = (end - start) / channels;
            let mut out = 0usize;
            let mut pixel = 0usize;
            while pixel < row_pixels && out + 3 <= scratch.len() {
                let base = start + pixel * channels;
                let (r, g, b) = match format {
                    FrameFormat::Rgba8888 => (source[base], source[base + 1], source[base + 2]),
                    FrameFormat::Rgb888 => (source[base], source[base + 1], source[base + 2]),
                    FrameFormat::Gray8 => (source[base], source[base], source[base]),
                };
                scratch[out] = r;
                scratch[out + 1] = g;
                scratch[out + 2] = b;
                out += 3;
                pixel += 1;
            }
            if out == 0 {
                break;
            }
            std::fs_write(fd, &scratch[..out])?;
            written_rows += 1;
        }
        Ok(())
    }

    /// The title of a settings row.
    pub fn settings_row_title(&self, row: SettingsRow) -> &'static [u8] {
        match row {
            SettingsRow::Format => b"Photo format",
            SettingsRow::Filter => b"Image processing",
            SettingsRow::Flash => b"Flash and torch",
            SettingsRow::Guides => b"Framing guides",
            SettingsRow::Photos => b"Photos on the device",
            SettingsRow::LastPhoto => b"Last photo",
            SettingsRow::LastCapture => b"Last capture",
            SettingsRow::OpenLibrary => b"Open the photo library",
            SettingsRow::Close => b"Close",
        }
    }

    /// The value of a settings row, copied into `out`.
    pub fn settings_row_value(&self, row: SettingsRow, out: &mut [u8; 64]) -> usize {
        match row {
            SettingsRow::Format => copy_bytes(out, b"PPM (P6), 8 bits per channel"),
            SettingsRow::Filter => copy_bytes(out, self.filter.label()),
            SettingsRow::Flash => copy_bytes(out, self.flash.label()),
            SettingsRow::Guides => copy_bytes(out, if self.guides { b"On" } else { b"Off" }),
            SettingsRow::Photos => {
                let mut n = 0usize;
                n += copy_bytes(&mut out[n..], b"Saved ");
                n += append_decimal(self.photos_published, &mut out[n..]);
                n += copy_bytes(&mut out[n..], b" of ");
                n += append_decimal(self.total_photos, &mut out[n..]);
                n
            }
            SettingsRow::LastPhoto => {
                if self.last_photo_len == 0 {
                    copy_bytes(out, b"None yet")
                } else {
                    copy_bytes(out, &self.last_photo[..self.last_photo_len])
                }
            }
            SettingsRow::LastCapture => copy_bytes(out, self.capture.caption()),
            SettingsRow::OpenLibrary => copy_bytes(out, self.publish.caption()),
            SettingsRow::Close => copy_bytes(out, b"Back to the viewfinder"),
        }
    }

    /// The note under a settings row: what the platform actually supports.
    pub fn settings_row_note(&self, row: SettingsRow) -> &'static [u8] {
        match row {
            SettingsRow::Format => b"Written into the shared storage area",
            SettingsRow::Filter => b"The top row changes the filter for the next capture",
            SettingsRow::Flash => b"The camera hardware reports the state it can reach",
            SettingsRow::Guides => b"Brackets, reticle and thirds over the viewfinder",
            SettingsRow::Photos => b"Frames this app captured",
            SettingsRow::LastPhoto => b"The path the last photo was written to",
            SettingsRow::LastCapture => b"What the last press of the shutter produced",
            SettingsRow::OpenLibrary => b"Hand the photo to the gallery that handles it",
            SettingsRow::Close => b"",
        }
    }
}

/// Copies `src` into `out` and returns how many bytes were written.
pub fn copy_bytes(out: &mut [u8], src: &[u8]) -> usize {
    let n = core::cmp::min(out.len(), src.len());
    out[..n].copy_from_slice(&src[..n]);
    n
}

/// The buffer the camera captures into.
///
/// WHY 64 KiB: the app has no allocator, so the frame the camera HAL hands
/// over has to fit in a fixed buffer. 64 KiB holds a 128x128 frame of four
/// bytes per pixel and nothing larger, and the platform says so when a frame
/// does not fit rather than truncating it.
pub const FRAME_BUFFER_BYTES: usize = 64 * 1024;

static mut FRAME_BUFFER: [u8; FRAME_BUFFER_BYTES] = [0; FRAME_BUFFER_BYTES];

/// Applies the image processing to a frame the platform returned.
///
/// WHY the channel count is a parameter: the platform does not report the
/// frame's pixel format, and the app works the format out from the bytes it
/// was handed. A filter that assumed four bytes per pixel would corrupt a
/// frame that is packed differently.
pub fn apply_isp_tone_curve(
    filter: CameraFilter,
    exposure_bias: i16,
    channels: usize,
    buf: &mut [u8],
) {
    if channels == 0 {
        return;
    }
    let mut i = 0usize;
    while i + channels <= buf.len() {
        let r = buf[i];
        let g = if channels >= 3 { buf[i + 1] } else { r };
        let b = if channels >= 3 { buf[i + 2] } else { r };
        let (mut r, mut g, mut b) = (r, g, b);
        match filter {
            CameraFilter::Normal => {}
            CameraFilter::Vivid => {
                let l = (r as i32 * 30 + g as i32 * 59 + b as i32 * 11) / 100;
                let sat_r = l + ((r as i32 - l) * 135) / 100;
                let sat_g = l + ((g as i32 - l) * 135) / 100;
                let sat_b = l + ((b as i32 - l) * 135) / 100;
                r = contrast(sat_r.clamp(0, 255) as u8);
                g = contrast(sat_g.clamp(0, 255) as u8);
                b = contrast(sat_b.clamp(0, 255) as u8);
            }
            CameraFilter::Warm => {
                let wr = ((r as u32 * 116) / 100).min(255) as u8;
                let wg = ((g as u32 * 105) / 100).min(255) as u8;
                let wb = ((b as u32 * 88) / 100).min(255) as u8;
                r = wr;
                g = wg;
                b = wb;
            }
            CameraFilter::Cool => {
                let cr = ((r as u32 * 88) / 100).min(255) as u8;
                let cg = ((g as u32 * 104) / 100).min(255) as u8;
                let cb = ((b as u32 * 118) / 100).min(255) as u8;
                r = cr;
                g = cg;
                b = cb;
            }
            CameraFilter::Mono => {
                let l = ((r as u32 * 30 + g as u32 * 59 + b as u32 * 11) / 100) as u8;
                let c = contrast(l);
                r = c;
                g = c;
                b = c;
            }
            CameraFilter::Vintage => {
                let vr = (24 + (r as u32 * 205) / 255).min(255) as u8;
                let vg = (20 + (g as u32 * 200) / 255).min(255) as u8;
                let vb = (30 + (b as u32 * 185) / 255).min(255) as u8;
                r = vr;
                g = vg;
                b = vb;
            }
        }
        if exposure_bias != 0 {
            let bias = exposure_bias as i32 * 16;
            r = (r as i32 + bias).clamp(0, 255) as u8;
            g = (g as i32 + bias).clamp(0, 255) as u8;
            b = (b as i32 + bias).clamp(0, 255) as u8;
        }
        buf[i] = r;
        if channels >= 3 {
            buf[i + 1] = g;
            buf[i + 2] = b;
        }
        i += channels;
    }
}

/// Steepens one channel.
fn contrast(v: u8) -> u8 {
    let x = v as i32;
    let out = (x - 128) * 3 / 2 + 128;
    out.clamp(0, 255) as u8
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn aspect_ratio_selector_cycles_correctly() {
        assert_eq!(CameraAspectRatio::Ratio4_3.label(), b"4:3");
        assert_eq!(CameraAspectRatio::Ratio16_9.label(), b"16:9");
        assert_eq!(CameraAspectRatio::Square.label(), b"1:1");

        assert_eq!(CameraAspectRatio::Ratio4_3.toggle(), CameraAspectRatio::Ratio16_9);
        assert_eq!(CameraAspectRatio::Ratio16_9.toggle(), CameraAspectRatio::Square);
        assert_eq!(CameraAspectRatio::Square.toggle(), CameraAspectRatio::Ratio4_3);
    }

    #[test]
    fn timer_mode_cycles_correctly() {
        assert_eq!(TimerMode::Off.label(), b"TIMER");
        assert_eq!(TimerMode::Sec3.label(), b"3s");
        assert_eq!(TimerMode::Sec10.label(), b"10s");

        assert_eq!(TimerMode::Off.next(), TimerMode::Sec3);
        assert_eq!(TimerMode::Sec3.next(), TimerMode::Sec10);
        assert_eq!(TimerMode::Sec10.next(), TimerMode::Off);

        assert_eq!(TimerMode::Off.seconds(), 0);
        assert_eq!(TimerMode::Sec3.seconds(), 3);
        assert_eq!(TimerMode::Sec10.seconds(), 10);
    }

    #[test]
    fn flash_mode_cycles_correctly() {
        assert_eq!(FlashMode::Auto.label(), b"FLASH AUTO");
        assert_eq!(FlashMode::On.label(), b"FLASH ON");
        assert_eq!(FlashMode::Off.label(), b"FLASH OFF");

        assert_eq!(FlashMode::Auto.next(), FlashMode::On);
        assert_eq!(FlashMode::On.next(), FlashMode::Off);
        assert_eq!(FlashMode::Off.next(), FlashMode::Auto);
    }

    #[test]
    fn filter_suite_cycles_all_calibrated_presets() {
        assert_eq!(CameraFilter::Normal.label(), b"NORMAL");
        assert_eq!(CameraFilter::Vivid.label(), b"VIVID");
        assert_eq!(CameraFilter::Warm.label(), b"WARM");
        assert_eq!(CameraFilter::Cool.label(), b"COOL");
        assert_eq!(CameraFilter::Mono.label(), b"MONO");
        assert_eq!(CameraFilter::Vintage.label(), b"VINTAGE");

        assert_eq!(CameraFilter::Normal.next(), CameraFilter::Vivid);
        assert_eq!(CameraFilter::Vivid.next(), CameraFilter::Warm);
        assert_eq!(CameraFilter::Warm.next(), CameraFilter::Cool);
        assert_eq!(CameraFilter::Cool.next(), CameraFilter::Mono);
        assert_eq!(CameraFilter::Mono.next(), CameraFilter::Vintage);
        assert_eq!(CameraFilter::Vintage.next(), CameraFilter::Normal);
    }

    #[test]
    fn the_refusal_and_the_absent_sensor_are_different_facts() {
        // The two facts must not read the same on the screen.
        assert_ne!(
            CameraStatus::AuthorityRefused.label(),
            CameraStatus::NoSensor.label()
        );
        assert_ne!(
            CameraStatus::AuthorityRefused.headline(),
            CameraStatus::NoSensor.headline()
        );
        assert_ne!(
            CameraStatus::AuthorityRefused.reason(),
            CameraStatus::NoSensor.reason()
        );
        // And each one has to say why.
        assert!(core::str::from_utf8(CameraStatus::AuthorityRefused.reason())
            .unwrap()
            .contains("denied"));
        assert!(core::str::from_utf8(CameraStatus::NoSensor.reason())
            .unwrap()
            .contains("no sensor"));
    }

    #[test]
    fn the_error_code_decides_which_fact_the_platform_stated() {
        assert_eq!(
            status_from_error(ERR_PERMISSION_DENIED),
            CameraStatus::AuthorityRefused
        );
        assert_eq!(status_from_error(ERR_DEVICE_ERROR), CameraStatus::NoSensor);
        assert_eq!(status_from_error(ERR_INVALID_ARGUMENTS), CameraStatus::Failed);
    }

    #[test]
    fn a_frame_that_does_not_match_its_geometry_is_refused() {
        assert_eq!(frame_format(2, 2, 16), Some(FrameFormat::Rgba8888));
        assert_eq!(frame_format(2, 2, 12), Some(FrameFormat::Rgb888));
        assert_eq!(frame_format(2, 2, 4), Some(FrameFormat::Gray8));
        // A frame that is not a whole number of pixels is not a frame.
        assert_eq!(frame_format(2, 2, 15), None);
        // A frame whose length matches no known layout is not guessed at.
        assert_eq!(frame_format(2, 2, 32), None);
        assert_eq!(frame_format(0, 4, 0), None);
    }

    #[test]
    fn the_header_names_the_format_that_is_actually_written() {
        let mut header = [0u8; 32];
        let n = ppm_header(3, 5, &mut header);
        assert_eq!(&header[..n], b"P6\n3 5\n255\n");
        // The platform's media-type sniffer reads these bytes, so the header
        // has to start with the magic it looks for.
        assert_eq!(&header[..2], b"P6");
    }

    #[test]
    fn the_photo_is_written_into_the_photos_directory() {
        let mut path = [0u8; PHOTO_PATH_MAX];
        let n = photo_path(1, &mut path);
        assert_eq!(&path[..n], b"/storage/photos/shot-1.ppm");
        let n = photo_path(42, &mut path);
        assert_eq!(&path[..n], b"/storage/photos/shot-42.ppm");
        // Every photo the camera writes has to be somewhere the platform's
        // media-type sniffer and the gallery look.
        assert!(core::str::from_utf8(&path[..n])
            .unwrap()
            .starts_with("/storage/photos/"));
    }

    #[test]
    fn the_publication_reports_what_the_platform_answered() {
        assert_eq!(publish_state(Ok(()), true), PublishState::Published);
        assert_eq!(publish_state(Ok(()), false), PublishState::WrittenNotIndexed);
        assert_eq!(publish_state(Err(-13), false), PublishState::Refused(-13));
        // WHY the refusal's own code is carried through: the camera reports
        // the platform's answer, not a constant of its own, so a caller can
        // tell one refusal from another.
        assert_eq!(PublishState::Refused(-13).error_code(), Some(-13));
        assert_eq!(PublishState::Published.error_code(), None);
        // The three captions have to be three different facts.
        assert_ne!(
            PublishState::Published.caption(),
            PublishState::WrittenNotIndexed.caption()
        );
        assert_ne!(
            PublishState::WrittenNotIndexed.caption(),
            PublishState::Refused(-13).caption()
        );
    }

    #[test]
    fn the_settings_surface_describes_every_row() {
        let app = CameraApp::new();
        let mut value = [0u8; 64];
        for row in SETTINGS_ROW_ORDER.iter() {
            let title = app.settings_row_title(*row);
            assert!(!title.is_empty());
            let n = app.settings_row_value(*row, &mut value);
            assert!(n > 0);
        }
        // The format the app writes is the format the header names.
        let n = app.settings_row_value(SettingsRow::Format, &mut value);
        assert_eq!(&value[..n], b"PPM (P6), 8 bits per channel");
    }

    #[test]
    fn the_capture_readout_reports_the_state_until_a_frame_exists() {
        let mut app = CameraApp::new();
        app.status = CameraStatus::AuthorityRefused;
        let (label, _) = app.frame_readout();
        assert_eq!(label, CameraStatus::AuthorityRefused.label());
        app.total_photos = 1;
        app.frame_label = *b"1280x720\0\0\0\0\0\0\0\0";
        app.frame_label_len = 8;
        let (label, len) = app.frame_readout();
        assert_eq!(&label[..len], b"1280x720");
    }

    #[test]
    fn the_isp_filter_leaves_the_alpha_channel_alone() {
        let mut pixels = [255u8, 0, 0, 255, 0, 255, 0, 255];
        apply_isp_tone_curve(CameraFilter::Mono, 0, 4, &mut pixels);
        assert_eq!(pixels[0], 50);
        assert_eq!(pixels[1], 50);
        assert_eq!(pixels[2], 50);
        assert_eq!(pixels[3], 255);
    }

    #[test]
    fn the_isp_filter_respects_the_frame_it_was_given() {
        // A frame that is packed three bytes per pixel is filtered three
        // bytes at a time, so the next pixel is not shifted.
        let mut pixels = [255u8, 0, 0, 0, 255, 0];
        apply_isp_tone_curve(CameraFilter::Mono, 0, 3, &mut pixels);
        // The first pixel is red (contrast(76)=50), the second green (contrast(150)=161),
        // and each keeps its own three bytes.
        assert_eq!(&pixels[..3], &[50, 50, 50]);
        assert_eq!(&pixels[3..6], &[161, 161, 161]);
    }

    #[test]
    fn thumbnail_generation_downsamples_frame_to_disc_buffer() {
        let mut app = CameraApp::new();
        assert!(!app.has_thumbnail);
        let (w, h) = (4u32, 4u32);
        // Fill static FRAME_BUFFER with pattern
        unsafe {
            let buf = &mut *addr_of_mut!(FRAME_BUFFER);
            for i in 0..64 {
                buf[i] = (i % 256) as u8;
            }
        }
        app.generate_thumbnail(w, h, FrameFormat::Rgb888);
        assert!(app.has_thumbnail);
        // Verify buffer has values
        assert_eq!(app.thumbnail.len(), THUMBNAIL_DIM * THUMBNAIL_DIM * 3);
    }

    #[test]
    fn every_line_the_camera_draws_fits_the_column_it_is_drawn_in() {
        // The card draws its headline at the body rung and its reason at the
        // caption rung, both inside the card's own column, and the renderer
        // clips a run at the column's right edge rather than wrapping it. WHY
        // the column and not the viewfinder: a run measured against anything
        // wider than what it is painted into is a run that loses its own tail
        // on the screen, which is exactly how a refusal notice came to read
        // "the camera capabilit".
        let mut app = CameraApp::new();
        for status in [
            CameraStatus::AuthorityRefused,
            CameraStatus::NoSensor,
            CameraStatus::AuthorityHeld,
            CameraStatus::Failed,
        ] {
            app.status = status;
            app.last_error = -12345;
            let head = status.headline();
            assert!(
                std::design::font::measure(head, STATUS_HEAD_RUNG) <= STATUS_TEXT_W,
                "the headline {:?} needs {} px and the card's column is {} px",
                core::str::from_utf8(head).unwrap(),
                std::design::font::measure(head, STATUS_HEAD_RUNG),
                STATUS_TEXT_W
            );
            let mut reason = [0u8; 96];
            let len = app.status_reason(&mut reason);
            let split = notice_break(&reason[..len], STATUS_TEXT_W, STATUS_REASON_RUNG);
            let rest = if split < len { split + 1 } else { split };
            let (first, second) = (&reason[..split], &reason[rest..len]);
            assert!(
                std::design::font::measure(first, STATUS_REASON_RUNG) <= STATUS_TEXT_W,
                "the reason's first line {:?} needs {} px and the card's column is {} px",
                core::str::from_utf8(first).unwrap(),
                std::design::font::measure(first, STATUS_REASON_RUNG),
                STATUS_TEXT_W
            );
            assert!(
                std::design::font::measure(second, STATUS_REASON_RUNG) <= STATUS_TEXT_W,
                "the reason's second line {:?} needs {} px, so the card needs a third line",
                core::str::from_utf8(second).unwrap(),
                std::design::font::measure(second, STATUS_REASON_RUNG)
            );
        }
    }

    #[test]
    fn the_status_notice_splits_at_words_and_keeps_them() {
        // WHY this is its own gate: the card reserves two lines for the reason,
        // and a reason that needs a third is only safe if every one of its
        // words fits the column on its own -- otherwise the split has nowhere
        // to go and the notice loses a word.
        let mut app = CameraApp::new();
        for status in [
            CameraStatus::AuthorityRefused,
            CameraStatus::NoSensor,
            CameraStatus::AuthorityHeld,
            CameraStatus::Failed,
        ] {
            app.status = status;
            app.last_error = -12345;
            let mut reason = [0u8; 96];
            let len = app.status_reason(&mut reason);
            let mut start = 0usize;
            while start < len {
                let end = match reason[start..len].iter().position(|b| *b == b' ') {
                    Some(offset) => start + offset,
                    None => len,
                };
                assert!(
                    std::design::font::measure(&reason[start..end], STATUS_REASON_RUNG)
                        <= STATUS_TEXT_W,
                    "the word {:?} alone needs {} px and the card's column is {} px",
                    core::str::from_utf8(&reason[start..end]).unwrap(),
                    std::design::font::measure(&reason[start..end], STATUS_REASON_RUNG),
                    STATUS_TEXT_W
                );
                start = end + 1;
            }
            let split = notice_break(&reason[..len], STATUS_TEXT_W, STATUS_REASON_RUNG);
            let rest = if split < len { split + 1 } else { split };
            let (first, second) = (&reason[..split], &reason[rest..len]);
            assert_eq!(
                first.len() + second.len() + usize::from(!second.is_empty()),
                len,
                "the split dropped a byte of the reason"
            );
        }
    }

    #[test]
    fn the_refusal_notice_keeps_its_own_words_on_the_card_that_states_it() {
        // The regression this pins: the platform's refusal read "The platform
        // denied this app the camera capabilit" on the screen, because the
        // column clipped it and the only gate measured the run against a
        // region wider than the card. WHY the longest verdict is the one to
        // ask about: it is the one the card cannot hold on a single line.
        let mut app = CameraApp::new();
        app.status = CameraStatus::AuthorityRefused;
        app.last_error = 0;
        app.refresh_status_text();
        let first = &app.sub[..app.sub_len];
        let second = &app.sub2[..app.sub2_len];
        assert!(
            std::design::font::measure(first, STATUS_REASON_RUNG) <= STATUS_TEXT_W,
            "the refusal's first line needs {} px and the card's column is {} px",
            std::design::font::measure(first, STATUS_REASON_RUNG),
            STATUS_TEXT_W
        );
        assert!(
            std::design::font::measure(second, STATUS_REASON_RUNG) <= STATUS_TEXT_W,
            "the refusal's second line needs {} px and the card's column is {} px",
            std::design::font::measure(second, STATUS_REASON_RUNG),
            STATUS_TEXT_W
        );
        assert!(!second.is_empty(), "the refusal fits one line again, so the card reserves a second line for nothing");
        assert_eq!(
            first.len() + second.len() + 1,
            app.status.reason().len(),
            "the two lines are not the whole sentence: {first:?} + {second:?}"
        );
        assert_eq!(
            second,
            b"capability",
            "the word that was clipped off the card is the one the notice is about"
        );
    }

    #[test]
    fn every_settings_line_fits_the_rung_it_is_set_in() {
        // The gate used to measure every run at the caption rung, which is the
        // rung the painter falls back to -- so it proved the fallback and never
        // the type the sheet actually intends, and the only thing that kept the
        // rows legible was a runtime downgrade nobody could see.
        let app = CameraApp::new();
        let mut value = [0u8; 64];
        for row in SETTINGS_ROW_ORDER.iter() {
            let title = app.settings_row_title(*row);
            assert!(
                std::design::font::measure(title, SETTINGS_TITLE_RUNG) <= SETTINGS_TEXT_MAX,
                "the settings title {:?} does not fit its column at the body rung",
                core::str::from_utf8(title).unwrap()
            );
            let note = app.settings_row_note(*row);
            assert!(
                std::design::font::measure(note, SETTINGS_NOTE_RUNG) <= SETTINGS_TEXT_MAX,
                "the settings note {:?} does not fit its column at the caption rung",
                core::str::from_utf8(note).unwrap()
            );
            let n = app.settings_row_value(*row, &mut value);
            assert!(
                std::design::font::measure(&value[..n], SETTINGS_VALUE_RUNG) <= SETTINGS_TEXT_MAX,
                "the settings value {:?} does not fit its column at the body rung",
                core::str::from_utf8(&value[..n]).unwrap()
            );
        }
    }

    #[test]
    fn the_readout_says_which_fact_the_platform_stated() {
        // The capture readout is the first thing the user reads, so the three
        // states have to be told apart there as well as in the viewfinder.
        assert_eq!(CameraStatus::AuthorityRefused.label(), b"NO AUTH");
        assert_eq!(CameraStatus::NoSensor.label(), b"NO SENSOR");
        assert_eq!(CameraStatus::AuthorityHeld.label(), b"NO FRAME");
        // And the refusal and the absent sensor are not the same words.
        assert_ne!(
            CameraStatus::AuthorityRefused.label(),
            CameraStatus::NoSensor.label()
        );
    }

    #[test]
    fn the_chip_row_clears_the_viewfinders_own_corner() {
        // The row used to be laid out in the renderer, twelve pixels inside a
        // viewfinder whose corner reaches eighteen, so the first chip was cut
        // by the frame it floats above.
        let mut left_edge_of = [0usize; CHIP_COUNT];
        for index in 0..CHIP_COUNT {
            let chip = chip_rect(index);
            assert_eq!(chip.w, CHIP_W);
            assert_eq!(chip.h, CHIP_H);
            assert!(chip.h >= 48, "a chip has to stay a touch target");
            assert!(
                chip.x >= VIEWFINDER_LEFT + VIEWFINDER_RADIUS,
                "chip {} hangs over the viewfinder's left corner",
                index
            );
            assert!(
                chip.y >= VIEWFINDER_TOP + VIEWFINDER_RADIUS,
                "chip {} hangs over the viewfinder's top corner",
                index
            );
            assert!(chip.x + chip.w <= VIEWFINDER_RIGHT);
            assert!(chip.y + chip.h <= VIEWFINDER_BOTTOM);
            left_edge_of[index] = chip.x;
        }
        for index in 1..CHIP_COUNT {
            assert!(
                left_edge_of[index] > left_edge_of[index - 1] + CHIP_W,
                "chips {} and {} overlap",
                index - 1,
                index
            );
        }
    }

    #[test]
    fn every_shutter_answer_fits_the_strip_it_is_drawn_in() {
        let outcomes = [
            CaptureOutcome::Published,
            CaptureOutcome::WrittenNotIndexed,
            CaptureOutcome::WriteRefused(-13),
            CaptureOutcome::CaptureRefused(-13),
            CaptureOutcome::NoVideoStream,
            CaptureOutcome::None,
        ];
        // The only room under the shutter is the strip between the dock's top
        // edge and the outer ring.
        let strip = SHUTTER_CY - SHUTTER_RING_OUTER - SHUTTER_NOTICE_Y;
        assert!(strip > 0, "the dock leaves the caption no room at all");
        for outcome in outcomes {
            let text = outcome.caption();
            assert!(
                text.len() <= SHUTTER_NOTICE_MAX,
                "the caption {:?} is cut by the buffer it is copied into",
                core::str::from_utf8(text).unwrap()
            );
            assert!(
                std::design::font::measure(text, SHUTTER_NOTICE_SCALE) <= SHUTTER_NOTICE_MAX_W,
                "the caption {:?} does not fit between the gutters",
                core::str::from_utf8(text).unwrap()
            );
            let (_, _, _, height) =
                std::design::outline_text::run_box(0, SHUTTER_NOTICE_Y, SHUTTER_NOTICE_SCALE, text);
            assert!(
                height <= strip,
                "the caption {:?} runs into the shutter",
                core::str::from_utf8(text).unwrap()
            );
        }
    }

    #[test]
    fn the_caption_lands_in_the_dock_and_not_on_the_shutter() {
        let mut app = CameraApp::new();
        app.status = CameraStatus::AuthorityRefused;
        app.refuse_shutter();
        let text = &app.shutter_notice[..app.shutter_notice_len];
        let (left, top, width, height) = std::design::outline_text::run_box(
            app.shutter_notice_x,
            SHUTTER_NOTICE_Y,
            SHUTTER_NOTICE_SCALE,
            text,
        );
        assert!(
            left >= VIEWFINDER_LEFT && left + width <= VIEWFINDER_RIGHT,
            "the caption runs past the gutters the rest of the surface is drawn inside"
        );
        assert!(
            top >= DOCK_Y0 && top + height <= SHUTTER_CY - SHUTTER_RING_OUTER,
            "the caption runs into the shutter it is answering"
        );
        assert_eq!(app.shutter_notice_ink(), theme::chrome::REFUSED);
        app.status = CameraStatus::AuthorityHeld;
        assert_eq!(app.shutter_notice_ink(), theme::chrome::CAUTION);
    }

    #[test]
    fn the_caption_answers_the_last_press_and_not_the_first() {
        let mut app = CameraApp::new();
        assert_eq!(app.shutter_notice_len, 0, "no press of the shutter has happened yet");
        assert!(
            app.shutter_notice_pixel(100, SHUTTER_NOTICE_Y, 0xFF20_2020, 0xFF00_0000).is_none(),
            "an empty caption must not paint over the dock"
        );

        app.status = CameraStatus::AuthorityRefused;
        app.refuse_shutter();
        assert_eq!(
            app.shutter_notice_len,
            CameraStatus::AuthorityRefused.headline().len(),
            "the platform's own words are the answer the user gets"
        );

        app.capture = CaptureOutcome::CaptureRefused(-13);
        app.show_capture_outcome();
        assert_eq!(&app.shutter_notice[..app.shutter_notice_len], CaptureOutcome::CaptureRefused(-13).caption());

        // A press that worked cannot leave yesterday's refusal under it.
        app.capture = CaptureOutcome::Published;
        app.show_capture_outcome();
        assert_eq!(app.shutter_notice_len, 0, "a published photo needs no caption");
    }

    fn srgb_luminance(argb: u32) -> f64 {
        let channel = |c: u32| {
            let c = c as f64 / 255.0;
            if c <= 0.04045 {
                c / 12.92
            } else {
                ((c + 0.055) / 1.055).powf(2.4)
            }
        };
        0.2126 * channel((argb >> 16) & 0xFF)
            + 0.7152 * channel((argb >> 8) & 0xFF)
            + 0.0722 * channel(argb & 0xFF)
    }

    fn wcag(a: u32, b: u32) -> f64 {
        let (la, lb) = (srgb_luminance(a), srgb_luminance(b));
        let (hi, lo) = if la >= lb { (la, lb) } else { (lb, la) };
        (hi + 0.05) / (lo + 0.05)
    }

    /// WHY the assertion is on pixels and not on role names: the chip is only
    /// real if the ARGB it emits clears WCAG 1.4.11's 3:1 floor for a non-text
    /// control against the chassis it stands on, and the roles that clear it on
    /// a light page are the roles that fail here.
    #[test]
    fn the_back_chip_paints_a_boundary_a_person_can_see_on_the_chassis() {
        use interos_std::design::back_button;

        let band = theme::chrome::BODY;
        let ring = back_button::pixel_for_theme(back_button::X, 80, &theme::CHASSIS_THEME)
            .expect("the SDK owns a border pixel at the chip's left edge");
        let plate = back_button::pixel_for_theme(30, 60, &theme::CHASSIS_THEME)
            .expect("the SDK fills the chip's interior");
        let chevron = back_button::pixel_for_theme(48, 76, &theme::CHASSIS_THEME)
            .expect("the SDK draws an arm at the glyph's apex");

        assert!(wcag(ring, band) >= 3.0, "ring {} is {} over the band", ring, wcag(ring, band));
        assert!(wcag(chevron, plate) >= 3.0, "chevron is {} over the plate", wcag(chevron, plate));

        // WHY a failure is asserted too: the fleet hands this control the light
        // roles, whose `#0A0A0A` outline is a measured 1.03:1 step off the
        // `#0E0F11` band -- a boundary no evaluator can see. Pinning that number
        // is what stops the chip flipping back to invisible with the theme.
        let light_ring = back_button::pixel_for_theme(back_button::X, 80, &theme::THEME)
            .expect("the light theme owns the same geometry");
        assert!(wcag(light_ring, band) < 3.0, "the light ring reads at {} on the chassis", wcag(light_ring, band));
    }
}
