//! InterOS camera application, executing in Ring 3 (User Mode).
//!
//! - A capture engine that takes a still photograph and writes it into the
//!   shared storage area as a PPM (P6), and publishes it into the platform's
//!   document index, so the gallery can open the photo the user just took
//! - A mode carousel of Photo, Video and Document framing. The Video mode is
//!   in the carousel and says the platform has no stream to give, rather than
//!   offering a mode the camera HAL cannot serve
//! - An image signal processor that is arithmetic the app performs on the
//!   frame it was handed: a tone curve and colour filters. No model runs over
//!   the frame and none is claimed
//! - A capture readout that says which fact the platform stated: the camera
//!   authority was refused, or the camera HAL has no sensor bound, or a frame
//!   has not been captured yet
//! - A settings surface that states what the platform supports, including the
//!   format the camera writes, where the last photo went and what the last
//!   press of the shutter produced
//! - DRM/KMS client surface rendering (z=200) with a viewfinder canvas, a
//!   mode carousel, a settings surface and a circular shutter
//! - Affordances whose backing capability is absent are drawn in a recessed
//!   unavailable treatment and are not hit-tested, instead of being painted
//!   live and ignored
//! - Banded rendering (`render_camera_band`): only the rows an input actually
//!   dirtied are re-rasterised, instead of all 1,036,800 pixels on every event
//! - Asynchronous event reactor loop handling touch controls and timer
//!   intervals

#![no_std]
#![no_main]

use core::panic::PanicInfo;
use interos_std as std;
use camera::theme::{MIN_TOUCH_TARGET, THEME, Theme, radius};
use std::design::primitives::Rect;

use interos_std::design::components;
use interos_std::design::outline_text;

use camera::{
    CameraApp, CameraFilter, CameraMode, CameraScreen, CameraStatus, CaptureOutcome, FlashMode,
    SettingsRow, SETTINGS_ROW_ORDER, VIEWFINDER_TEXT_W,
};

/// The platform's corner radii, under the short names the renderer uses.
///
/// WHY the aliases exist: the corner test names its radius four times, and a
/// number that appears four times is a number that can drift from the
/// platform. The alias resolves to the platform's step, so the drift is the
/// platform's and not this app's.
const R_CARD: usize = radius::CARD;

/// The platform's card treatment for the settings sheet.
///
/// WHY the platform's card and not a fill the app chose: the sheet is a card,
/// and a card that invents its own fill and its own hairline is a card a
/// third-party app cannot match.
const SHEET_CARD: components::CardStyle =
    components::card_style(&THEME, components::Elevation::Raised);

const W: usize = 720;
const H: usize = 1440;
const CHUNK: usize = 4096;

// -----------------------------------------------------------------------------
// Visual Design Tokens
//
// WHY there is no palette here any more: the app used to carry its own
// colours, which is a second design system inside one app. Every colour now
// resolves through the platform's theme, so a third-party app that asks the
// SDK for the same roles draws the same camera. See `camera::theme` for the
// mapping from the old names to the platform's roles.
// -----------------------------------------------------------------------------

/// Ink for a control that is painted but has nothing behind it.
///
/// WHY the border token and not a grey invented here: it is the design system's
/// own hairline, so "unavailable" reads the same in this app as it does in the
/// file manager and the gallery, and it stays in step if the token ever moves.
const DISABLED: u32 = THEME.border;

// -----------------------------------------------------------------------------
// Screen bands - the single source of truth for both drawing and the
// `Dirty` -> band mapping used by `render_camera_band`.
// -----------------------------------------------------------------------------
const TOP_BAND_Y0: usize = 0;
/// WHY 110 and not 68: the top row ends at 68, but the hairline that closes the
/// header is drawn at 108..109, so a band that stopped at the pill would leave a
/// stale divider on screen.
const TOP_BAND_Y1: usize = 110;

const VIEWFINDER_Y0: usize = 120;
const VIEWFINDER_Y1: usize = 1120;

/// Inner width of the viewfinder canvas, which is inset 20px from each edge of
/// the surface (`x >= 20 && x < W - 20`).
const VIEWFINDER_W: usize = W - 40;

const MODE_STRIP_Y0: usize = 1150;
const MODE_STRIP_Y1: usize = 1200;

const DOCK_Y0: usize = 1210;
const DOCK_Y1: usize = H;

// -----------------------------------------------------------------------------
// Top control row geometry
//
// WHY these are named constants: the old renderer and the old hit test each
// transcribed the same five boxes independently (`x >= 130 && x < 250` and
// friends), so a control and its tap region were two separate sets of literals
// that could - and did - drift. Every box below is drawn from it and hit-tested
// from it.
// -----------------------------------------------------------------------------
const TOP_ROW_Y: usize = 28;
const TOP_ROW_H: usize = 40;

/// Back control, 52 wide: the shell's minimum touch target.
const BACK_RECT: Rect = Rect::new(24, TOP_ROW_Y, 52, TOP_ROW_H);
/// Flash / torch pill.
const FLASH_RECT: Rect = Rect::new(92, TOP_ROW_Y, 120, TOP_ROW_H);
/// Capture-geometry readout. NOT a control: it reports what the platform said.
const FRAME_RECT: Rect = Rect::new(228, TOP_ROW_Y, 160, TOP_ROW_H);
/// ISP filter pill (a real, local software filter - genuinely actionable).
const FILTER_RECT: Rect = Rect::new(404, TOP_ROW_Y, 140, TOP_ROW_H);
/// Capture counter badge. NOT a control: it reports `total_photos`.
const SHOTS_RECT: Rect = Rect::new(560, TOP_ROW_Y, 136, TOP_ROW_H);

/// Corner radius of the top-row pills.
///
/// WHY 16 and not `draw_pill`'s half-height: these are rounded rectangles, not
/// capsules. The old shape drew a 16px corner on a 40px control, and switching
/// to `draw_pill` would round them to 20px and change every control in the row.
const TOP_PILL_RADIUS: usize = 16;

/// Text baseline row for every scale-1 label in the top row.
///
/// WHY derived: a scale-1 run owns the face's 16-pixel line box, so the
/// label is centred inside the 40px control without its descenders
/// clipping against the pill's edges.
const TOP_ROW_TEXT_Y0: usize = TOP_ROW_Y + (TOP_ROW_H - std::design::font::line_height(1)) / 2;

/// Centre of the flash pictogram inside `FLASH_RECT`.
const FLASH_ICON_CX: usize = FLASH_RECT.x + 24;
const FLASH_ICON_CY: usize = FLASH_RECT.y + FLASH_RECT.h / 2;
/// Text room to the right of the flash pictogram, inside `FLASH_RECT`.
const FLASH_TEXT_X: usize = FLASH_RECT.x + 32;
const FLASH_TEXT_W: usize = FLASH_RECT.w - 32 - 8;

/// The system home gesture strip: a 200x6 indicator drawn inside a 240x44 box.
///
/// WHY the hit box is larger than the ink, and why both are named: 6px of ink
/// is not a touch target, so the tap region has to be the whole strip - but the
/// old code hit-tested `x 240..480 && y >= 1400` with no upper bound on y, a
/// region that was never drawn, so a tap well above the indicator exited the app.
const HOME_HIT: Rect = Rect::new(240, 1396, 240, MIN_TOUCH_TARGET);
const HOME_PILL: Rect = Rect::new(260, 1418, 200, 6);

// Shutter button, as concentric radii about its centre.
const SHUTTER_CX: usize = 360;
const SHUTTER_CY: usize = 1310;
/// Outer white metallic ring.
const SHUTTER_RING_OUTER: usize = 64;
/// Concentric track gap between the ring and the core.
const SHUTTER_RING_INNER: usize = 56;
/// Inner tactile shutter core.
const SHUTTER_R_CORE: usize = 48;

/// The dock's two side controls, as the centres of their pictograms.
///
/// WHY they are controls now: the camera used to paint a gallery thumbnail and
/// a lens-flip glyph in the dock and hit-test neither, because there was
/// nothing behind them to open and no lens to flip. The photo roll now exists
/// - the camera writes into the shared storage area and the platform's
/// document index describes it - and the settings surface the camera lacked
/// now has a place to live.
const PHOTOS_CX: usize = 110;
const SETTINGS_CX: usize = 610;

/// Half-extent of the dock's side buttons.
const DOCK_SIDEBTN_HALF: usize = 36;

// -----------------------------------------------------------------------------
// The settings surface.
//
// WHY a sheet over the viewfinder and not a new screen: the settings are the
// camera's own, and keeping the viewfinder visible behind them means the
// surface never pretends the camera stopped existing.
// -----------------------------------------------------------------------------

/// The sheet's card.
const SHEET_RECT: Rect = Rect::new(24, 140, 672, 960);
/// The sheet's title row.
const SHEET_TITLE_Y0: usize = 160;
/// The first settings row.
const SHEET_ROW_Y0: usize = 200;
/// The height of one settings row.
const SHEET_ROW_H: usize = 96;
/// The left edge of the settings text.
const SHEET_TEXT_X: usize = 48;
/// The width of the settings text column.
const SHEET_TEXT_W: usize = camera::SETTINGS_TEXT_MAX;
/// The left edge of a settings row's own surface.
///
/// WHY the row is inset from the card: the platform's list rows carry their
/// own padding, and a row drawn flush to the card's edge would read as part
/// of the card rather than as a control.
const SHEET_ROW_X0: usize = 36;
/// The width of a settings row's own surface.
const SHEET_ROW_W: usize = 648;
/// The gap between one row's surface and the next.
const SHEET_ROW_GAP: usize = 8;

// -----------------------------------------------------------------------------
// The viewfinder's framing guides.
//
// WHY they are a setting and not decoration: the brackets, the reticle and
// the thirds grid used to be drawn behind a counter that only advanced when a
// video frame arrived, and no video frame ever arrives on this platform, so
// the guides could never be seen. They are drawn now because the user asked
// for them, and they are drawn in the platform's own ink so they read the
// same as every other guide in the system.
// -----------------------------------------------------------------------------

/// Half-extent of the corner brackets.
const BRACKET_ARM: usize = 30;
/// Half-extent of the focus reticle.
const RETICLE_ARM: usize = 40;
/// The reticle's centre, in viewfinder-relative coordinates.
const RETICLE_CX: usize = 340;
const RETICLE_CY: usize = 500;

/// Whether a touch at (dx, dy) from the shutter centre is on the shutter.
///
/// WHY derived from the drawn radii rather than a hand-written box: the old hit
/// test was `y 1220..1400 && x 280..440`, a rectangle that included 24px above
/// and 26px below the painted ring and none of the dock beside it.
fn shutter_hit(dx: isize, dy: isize) -> bool {
    let r = SHUTTER_RING_OUTER as isize;
    dx * dx + dy * dy <= r * r
}

/// Whether a touch at (dx, dy) is on one of the dock's side buttons.
fn side_button_hit(x: usize, y: usize, cx: usize) -> bool {
    let dx = (x as isize - cx as isize).abs();
    let dy = (y as isize - SHUTTER_CY as isize).abs();
    dx <= DOCK_SIDEBTN_HALF as isize && dy <= DOCK_SIDEBTN_HALF as isize
}

// -----------------------------------------------------------------------------
// Typography provided by interos_std::design::font
// -----------------------------------------------------------------------------

/// Rasterises one text run into the pixel at `(x, y)`.
///
/// WHY the outline face comes first: the mode carousel and the viewfinder's
/// absence headline run at scale 2, so on the bitmap face each is a 16x24
/// cell magnified by an integer -- a soft ramp where an edge belongs.
/// `outline_text::place_run` rasterises the real outline at the em that matches
/// the cell's cap height, and answers for the pixel itself. When it declines --
/// a run at scale 1, whose cell is already at its native size, a run wider than
/// the column the caller reserved, a glyph whose ink would leave that column,
/// or a weight this app draws bold -- the bitmap face below draws the run
/// exactly as it did before, so a refusal costs the edge quality and never the
/// text.
///
/// WHY the box test comes first: rasterising is the expensive part and most
/// pixels this app asks about are nowhere near a run. Gating on
/// `outline_text::run_box` -- the union of both faces' extents, not the bitmap
/// cell's -- means a run is only placed on a pixel it can ink, so the shared
/// coverage buffer is written once per run rather than once per pixel.
///
/// WHY the run answers alone when it is placed: the two faces are alternatives,
/// not layers. Falling through to the bitmap face after an outline run had
/// already inked a pixel would double-ink every partially covered edge.
///
/// WHY the background is a parameter and not the surface colour: a run's edge
/// is blended against whatever is under it, and a run that sits on a pill, on
/// the viewfinder or on the settings card is not on the surface colour. A
/// blended edge computed against the wrong background is a visible halo.
#[inline(always)]
fn test_text(
    x: usize,
    y: usize,
    x0: usize,
    y0: usize,
    scale: usize,
    text: &[u8],
    max_w: usize,
    color: u32,
    bg: u32,
) -> Option<u32> {
    if scale < 2 {
        let w = std::design::font::measure(text, scale);
        let h = std::design::font::line_height(scale);
        if x < x0 || x - x0 >= w || y < y0 || y - y0 >= h {
            return None;
        }
        return std::design::font::test_text_aa(x, y, x0, y0, scale, text, color, bg, false);
    }
    let (bx, bt, bw, bh) = outline_text::run_box(x0, y0, scale, text);
    if x < bx || x - bx >= bw || y < bt || y - bt >= bh {
        return None;
    }
    if let Some(run) = outline_text::place_run(x0, y0, scale, text, Some(max_w), false) {
        return run.pixel(x, y, color, bg);
    }
    std::design::font::test_text_aa(x, y, x0, y0, scale, text, color, bg, false)
}

/// Vector lightning bolt pictogram for flash pill
fn test_lightning(x: usize, y: usize, cx: usize, cy: usize) -> bool {
    let dx = x as isize - cx as isize;
    let dy = y as isize - cy as isize;
    if dy >= -10 && dy <= 10 && dx >= -6 && dx <= 6 {
        let is_top = dy < 0 && dx >= (dy / 2) && dx <= (dy / 2 + 4);
        let is_bot = dy >= 0 && dx <= (dy / 2) && dx >= (dy / 2 - 4);
        is_top || is_bot
    } else {
        false
    }
}

/// Vector landscape pictogram for the photo library button.
fn test_gallery_icon(x: usize, y: usize, cx: usize, cy: usize) -> bool {
    let dx = x as isize - cx as isize;
    let dy = y as isize - cy as isize;
    // Sun circle at (-8, -8), r=4
    let sun_dx = dx + 8;
    let sun_dy = dy + 8;
    if sun_dx * sun_dx + sun_dy * sun_dy <= 16 {
        return true;
    }
    // Mountain triangles
    if (dy >= -2) && dy <= 12 {
        let m1 = (dx + 4).abs() <= (dy + 2);
        let m2 = (dx - 10).abs() <= (dy - 2);
        if m1 || m2 {
            return true;
        }
    }
    false
}

/// Vector sliders pictogram for the settings button.
///
/// WHY a sliders glyph and not a gear: a gear promises a configuration screen,
/// and this is one - but a gear drawn as a circle of teeth at this size is a
/// smudge. Three bars with a knob on each read as "adjust" at 36 pixels.
fn test_settings_icon(x: usize, y: usize, cx: usize, cy: usize) -> bool {
    let dx = x as isize - cx as isize;
    let dy = y as isize - cy as isize;
    let rows = [(-12i32, -8i32), (0i32, 8i32), (12i32, 0i32)];
    for (row_y, knob_x) in rows {
        let ry = dy - row_y as isize;
        if ry.abs() <= 1 && dx.abs() <= 14 {
            return true;
        }
        let kx = dx - knob_x as isize;
        if ry.abs() <= 4 && kx.abs() <= 4 {
            return true;
        }
    }
    false
}

/// The rounded-rect body of a top-row pill.
///
/// Returns `(fill, inset_x, inset_y)` when `(x, y)` is inside the pill, or `None`
/// when it is outside - including the transparent corner pixels, exactly as the
/// old inline test did. WHY a helper: those eight lines were copy-pasted into
/// each of the four pill branches, so a change to the corner geometry had to be
/// applied four times and one of them could be missed.
#[inline(always)]
fn top_pill_body(x: usize, y: usize, rect: Rect) -> Option<(u32, bool)> {
    if !rect.contains(x, y) {
        return None;
    }
    let rx = (x - rect.x).min(rect.x + rect.w - 1 - x);
    let ry = (y - rect.y).min(rect.y + rect.h - 1 - y);
    let r = TOP_PILL_RADIUS as isize;
    let mut on_border = false;
    if (rx as isize) < r && (ry as isize) < r {
        let dx = r - rx as isize;
        let dy = r - ry as isize;
        let d2 = dx * dx + dy * dy;
        if d2 >= r * r {
            return None;
        }
        if d2 >= (r - 2) * (r - 2) {
            on_border = true;
        }
    } else if rx < 2 || ry < 2 {
        on_border = true;
    }
    Some((THEME.surface_container, on_border))
}

/// Draws a scale-1 label centred in `rect` at the top row's text row, or
/// reports that this pixel is not part of the run.
#[inline(always)]
fn top_row_label(
    x: usize,
    y: usize,
    rect: Rect,
    label: &[u8],
    color: u32,
    bg: u32,
) -> Option<u32> {
    let w = std::design::font::measure(label, 1);
    if w > rect.w {
        // WHY draw nothing rather than overflow: a label wider than its control
        // would run into the next pill, and a truncated run would lie about the
        // value.
        return None;
    }
    let x0 = rect.x + (rect.w - w) / 2;
    test_text(x, y, x0, TOP_ROW_TEXT_Y0, 1, label, rect.w, color, bg)
}

/// Draws a scale-1 label right of the flash pictogram.
#[inline(always)]
fn flash_label(x: usize, y: usize, label: &[u8], color: u32, bg: u32) -> Option<u32> {
    let w = std::design::font::measure(label, 1);
    if w > FLASH_TEXT_W {
        return None;
    }
    let x0 = FLASH_TEXT_X + (FLASH_TEXT_W - w) / 2;
    test_text(x, y, x0, TOP_ROW_TEXT_Y0, 1, label, FLASH_TEXT_W, color, bg)
}

/// The ink of the framing guides.
///
/// WHY the theme's own roles and not the colours the guides used to carry:
/// the brackets were a white literal and the reticle a dark literal, so they
/// did not follow the theme and the brackets were invisible against the
/// surface they sat on. The bracket is the muted ink, the grid is that ink
/// thinned over the viewfinder, and the reticle is the warning accent.
fn guide_ink() -> u32 {
    THEME.on_surface_variant
}

/// The ink of the thirds grid.
fn grid_ink() -> u32 {
    Theme::blend(THEME.surface_container_low, THEME.on_surface_variant, 40)
}

/// The ink of the viewfinder's border.
fn viewfinder_border_ink() -> u32 {
    Theme::blend(THEME.surface, THEME.on_surface, 204)
}

#[inline(never)]
fn get_camera_pixel(x: usize, y: usize, camera: &CameraApp) -> u32 {
    // 1. Status Bar & Top Controls (y: 0..110)
    if y < TOP_BAND_Y1 {
        if y >= TOP_BAND_Y1 - 2 {
            return THEME.border;
        }

        // Back control. WHY an outline measured from the rectangle's own edges
        // instead of the old `dx + dy <= 12` stroke: that stroke's apex landed
        // 22px right of the button's centre, so the glyph was not centred in the
        // thing it labelled. The chevron is now centred on `BACK_RECT`, and the
        // box is inset by one pixel so it is never clipped by `contains`.
        if let Some(c) = std::design::back_button::pixel(x, y, THEME.on_surface, THEME.border, THEME.surface_container) {
            return c;
        }

        // Flash / torch pill. This is a real control: it issues
        // `camera_toggle_flash` and reports what the backend answered.
        if let Some((fill, on_border)) = top_pill_body(x, y, FLASH_RECT) {
            let is_lit =
                camera.flash_available && (camera.flash == FlashMode::Torch || camera.flash == FlashMode::On);
            let icon_color = if is_lit {
                THEME.warning
            } else if !camera.flash_available {
                THEME.on_surface_variant
            } else {
                THEME.on_surface
            };

            if test_lightning(x, y, FLASH_ICON_CX, FLASH_ICON_CY) {
                return icon_color;
            }

            let label: &[u8] = if !camera.flash_available {
                &b"OFF"[..]
            } else if is_lit {
                &b"TORCH"[..]
            } else {
                &b"AUTO"[..]
            };
            if let Some(c) = flash_label(x, y, label, icon_color, fill) {
                return c;
            }

            if on_border {
                return if is_lit { THEME.warning } else { THEME.border };
            }
            return fill;
        }

        // Capture geometry. WHY this is no longer "48MP PRO": that string
        // advertised a resolution and a shooting mode the hardware never
        // reported and this app can never produce, and it was painted as if it
        // were a control. It now reports what the platform said: "NO AUTH" when
        // the platform refused this app the camera authority, "NO SENSOR" when
        // the camera HAL reported that no sensor is bound, "NO FRAME" until a
        // capture succeeds, then the real `WxH`.
        if let Some((_fill, on_border)) = top_pill_body(x, y, FRAME_RECT) {
            let (label, label_len) = camera.frame_readout();
            let label = &label[..label_len];
            // WHY muted ink and not `DISABLED`: this pill is a readout, not a
            // control, and it has to stay legible - "NO SENSOR" is the answer to
            // a question the user is asking. Only its outline is recessed, to
            // signal that there is nothing to press.
            let color = if camera.status == CameraStatus::AuthorityRefused {
                THEME.error
            } else {
                THEME.primary
            };
            if let Some(c) = top_row_label(x, y, FRAME_RECT, label, color, THEME.surface_container) {
                return c;
            }
            if on_border {
                return DISABLED;
            }
            return THEME.surface_container;
        }

        // ISP filter pill. This is a real control: the filter is applied to
        // the captured frame, so selecting one changes what the next capture
        // is tone-mapped with.
        if let Some((fill, on_border)) = top_pill_body(x, y, FILTER_RECT) {
            let (filter_name, filter_color) = match camera.filter {
                CameraFilter::Normal => (&b"NORMAL"[..], THEME.primary),
                CameraFilter::Monochrome => (&b"MONO"[..], THEME.on_surface_variant),
                CameraFilter::HighContrast => (&b"CONTRAST"[..], THEME.on_surface),
                CameraFilter::Sepia => (&b"SEPIA"[..], THEME.warning),
                CameraFilter::InvertedRed => (&b"INVERT"[..], THEME.error),
            };
            if let Some(c) = top_row_label(x, y, FILTER_RECT, filter_name, filter_color, fill) {
                return c;
            }
            if on_border {
                return filter_color;
            }
            return fill;
        }

        // Capture counter. WHY this is no longer a green "AI" badge: "AI"
        // asserted a running neural-vision pipeline that this app does not have
        // - it forwards nothing to any inference broker, and no such service is
        // bound to it. The badge now reports `total_photos`, a counter that only
        // advances when `camera_capture` actually returned a frame.
        if let Some((_fill, on_border)) = top_pill_body(x, y, SHOTS_RECT) {
            let label = &camera.shots_label[..camera.shots_label_len];
            // WHY the count is ink and not success green: the label says how
            // many frames this camera has taken, which is neither good nor
            // bad. Green here made a normal state look like a verdict, and
            // the theme keeps green for the platform's own status.
            if let Some(c) = top_row_label(x, y, SHOTS_RECT, label, THEME.on_surface, THEME.surface_container) {
                return c;
            }
            if on_border {
                return THEME.border;
            }
            return THEME.surface_container;
        }

        return THEME.surface;
    }

    // 2. The settings surface.
    //
    // WHY it is drawn *before* the viewfinder and not after it: the
    // viewfinder's own window is a rounded rectangle that covers
    // 20..700 by 120..1120, and the settings sheet sits entirely inside
    // it. The viewfinder returns a pixel for every pixel of that window,
    // so a sheet drawn after it is never reached -- the button opened the
    // settings and the screen did not change. Drawn first, the sheet is
    // over the viewfinder, which is what the settings are: a sheet over
    // the camera, with the camera still visible behind it.
    if camera.screen == CameraScreen::Settings && SHEET_RECT.contains(x, y) {
        return settings_pixel(x, y, camera);
    }

    // 3. Viewfinder Canvas (y: VIEWFINDER_Y0..VIEWFINDER_Y1, x: 20..700)
    if (VIEWFINDER_Y0..VIEWFINDER_Y1).contains(&y) && x >= 20 && x < W - 20 {
        let rel_y = y - VIEWFINDER_Y0;
        let rel_x = x - 20;

        // The viewfinder's window.
        let vf = Rect::new(20, VIEWFINDER_Y0, VIEWFINDER_W, VIEWFINDER_Y1 - VIEWFINDER_Y0);
        if let Some(c) = std::design::primitives::draw_rounded_rect(
            x,
            y,
            vf,
            R_CARD,
            THEME.surface_container_low,
            Some(viewfinder_border_ink()),
            1,
        ) {
            // The framing guides. WHY they are drawn when the user asked for
            // them and not behind a counter that never moves: the old code drew
            // them only once a video frame had arrived, and no video frame
            // arrives on this platform, so the guides were unreachable. They are
            // a framing aid the user turns on in the settings surface, and they
            // are painted in the platform's own ink.
            if camera.guides {
                if let Some(c) = guide_pixel(rel_x, rel_y) {
                    return c;
                }
            }

            // The absence panel. WHY it states which fact the platform gave:
            // "no sensor" and "no authority" are different problems with
            // different remedies, and a screen that says the first when the
            // second is true sends the user after the wrong thing.
            if camera.total_photos == 0 {
                if y >= 465 && y < 560 {
                    if y < 525 {
                        if let Some(c) = test_text(
                            x,
                            y,
                            camera.head_x,
                            470,
                            2,
                            &camera.head[..camera.head_len],
                            VIEWFINDER_TEXT_W,
                            THEME.on_surface,
                            THEME.surface_container_low,
                        ) {
                            return c;
                        }
                    } else if y >= 528 {
                        if let Some(c) = test_text(
                            x,
                            y,
                            camera.sub_x,
                            530,
                            1,
                            &camera.sub[..camera.sub_len],
                            VIEWFINDER_TEXT_W,
                            THEME.on_surface_variant,
                            THEME.surface_container_low,
                        ) {
                            return c;
                        }
                    }
                }
            }
            return c;
        }
    }

    // 4. Mode Selector Carousel (y: 1150..1200, 50px tall, >=48px touch)
    // WHY three equal slots in 20..700 (outer 20px margins, aligned with the
    // viewfinder): S0 20..246, S1 247..473, S2 474..700, each 226 wide with
    // 1px separators. The old row inked DOCUMENTS at 108..352 and PHOTO at
    // 320..456 at scale 2, a 32px overlap, and y0=1166 left only 34px of the
    // 50px strip for 48px glyphs so descenders clipped at 1200. Now each
    // label is centered in its own slot via measure: DOCUMENT=220px at
    // scale 2 fits its 226px slot (23..243), PHOTO=136 (292..428),
    // VIDEO=118 (528..646); gaps 49px/100px, never overlapping. Step-down:
    // try scale 2, drop the whole row to scale 1 if any label misses its
    // slot; never truncate mid-word. y0 centers the line height in the strip.
    if (MODE_STRIP_Y0..MODE_STRIP_Y1).contains(&y) {
        const S0_X: usize = 20;
        const S0_W: usize = 226;
        const S1_X: usize = 247;
        const S1_W: usize = 226;
        const S2_X: usize = 474;
        const S2_W: usize = 226;
        const DOC: &[u8] = b"DOCUMENT";
        const PHO: &[u8] = b"PHOTO";
        const VID: &[u8] = b"VIDEO";

        let mut scale = 2usize;
        if std::design::font::measure(DOC, 2) > S0_W
            || std::design::font::measure(PHO, 2) > S1_W
            || std::design::font::measure(VID, 2) > S2_W
        {
            scale = 1;
        }
        let lh = std::design::font::line_height(scale);
        // Strip is 50px tall: y0 = MODE_STRIP_Y0 + (50 - lh) / 2 -> 1151 at
        // scale 2 (inks 1151..1199 inside 1150..1200), 1163 at scale 1.
        let y0 = MODE_STRIP_Y0 + (50usize.saturating_sub(lh)) / 2;

        if x < S1_X {
            // Document (slot 0)
            let is_doc = camera.mode == CameraMode::DocumentVision;
            let doc_color = if is_doc { THEME.on_surface } else { THEME.on_surface_variant };
            let doc_w = std::design::font::measure(DOC, scale);
            let doc_x = S0_X + (S0_W.saturating_sub(doc_w)) / 2;
            if let Some(c) = test_text(x, y, doc_x, y0, scale, DOC, S0_W, doc_color, THEME.surface) {
                return c;
            }
        } else if x < S2_X {
            // Photo (slot 1)
            let is_photo = camera.mode == CameraMode::Photo;
            let photo_color = if is_photo { THEME.on_surface } else { THEME.on_surface_variant };
            let pho_w = std::design::font::measure(PHO, scale);
            let pho_x = S1_X + (S1_W.saturating_sub(pho_w)) / 2;
            if let Some(c) = test_text(x, y, pho_x, y0, scale, PHO, S1_W, photo_color, THEME.surface) {
                return c;
            }
        } else {
            // Video (slot 2)
            let is_vid = camera.mode == CameraMode::Video;
            let vid_color = if is_vid { THEME.on_surface } else { THEME.on_surface_variant };
            let vid_w = std::design::font::measure(VID, scale);
            let vid_x = S2_X + (S2_W.saturating_sub(vid_w)) / 2;
            if let Some(c) = test_text(x, y, vid_x, y0, scale, VID, S2_W, vid_color, THEME.surface) {
                return c;
            }
        }

        // Active mode indicator underline, 64px centered in the active slot.
        // All-caps labels carry no descender ink, so rows 1194..1198 stay clear.
        let is_active_mode = match camera.mode {
            CameraMode::Photo => x >= 328 && x < 392,
            CameraMode::Video => x >= 555 && x < 619,
            CameraMode::DocumentVision => x >= 101 && x < 165,
        };
        if is_active_mode && y >= 1194 && y < 1198 {
            // WHY the ink and not the warning accent: the underline says
            // which of three modes is showing, and that is a position, not
            // a warning. An orange underline on every camera screen spends
            // the one colour the theme reserves for something being wrong.
            return THEME.on_surface;
        }

        return THEME.surface;
    }

    // 5. Bottom Shutter Dock & Hardware Controls (y: DOCK_Y0..DOCK_Y1)
    if (DOCK_Y0..DOCK_Y1 - 30).contains(&y) {
        // The photo library button. WHY it is a control now: the camera writes
        // its photos into the shared storage area and the platform's document
        // index describes them, so there is something behind it to open.
        if side_button_hit(x, y, PHOTOS_CX) {
            if test_gallery_icon(x, y, PHOTOS_CX, SHUTTER_CY) {
                return THEME.on_surface_variant;
            }
            let dx = (x as isize - PHOTOS_CX as isize).abs();
            let dy = (y as isize - SHUTTER_CY as isize).abs();
            if dx == DOCK_SIDEBTN_HALF as isize || dy == DOCK_SIDEBTN_HALF as isize {
                return THEME.border;
            }
            return THEME.surface_container;
        }

        // Center Shutter Button. WHY the disabled treatment: `camera_capture` is
        // refused with PermissionDenied unless the task holds `SystemControl` on
        // the camera service object or `HardwarePortIo` on port 0xCA00, and this
        // task is spawned with neither. Painting the full-brightness ring and
        // core made a button that provably cannot capture look armed.
        let dx = x as isize - SHUTTER_CX as isize;
        let dy = y as isize - SHUTTER_CY as isize;
        let dist_sq = dx * dx + dy * dy;

        let outer = (SHUTTER_RING_OUTER * SHUTTER_RING_OUTER) as isize;
        let inner = (SHUTTER_RING_INNER * SHUTTER_RING_INNER) as isize;
        let core = (SHUTTER_R_CORE * SHUTTER_R_CORE) as isize;

        // Outer metallic ring.
        if dist_sq <= outer && dist_sq >= inner {
            return if camera.status == CameraStatus::AuthorityRefused {
                DISABLED
            } else {
                THEME.on_surface
            };
        }
        // Concentric track gap.
        if dist_sq < inner && dist_sq >= core {
            return THEME.surface;
        }
        // Inner tactile shutter core.
        if dist_sq < core {
            if camera.status == CameraStatus::AuthorityRefused {
                return THEME.surface_container;
            } else if camera.mode == CameraMode::Video {
                return THEME.error;
            } else if camera.mode == CameraMode::DocumentVision {
                return THEME.primary;
            } else {
                return THEME.on_surface;
            }
        }

        // The settings button. WHY it replaced the lens-flip glyph: the camera
        // HAL exposes `capture_frame` and nothing that selects between a front
        // and a rear sensor, so there is no lens to flip and the old glyph was
        // a control that looked armed and only pretended to be. The settings
        // surface the camera needed now has a place to live.
        if side_button_hit(x, y, SETTINGS_CX) {
            if test_settings_icon(x, y, SETTINGS_CX, SHUTTER_CY) {
                return THEME.on_surface_variant;
            }
            let dx = (x as isize - SETTINGS_CX as isize).abs();
            let dy = (y as isize - SHUTTER_CY as isize).abs();
            if dx == DOCK_SIDEBTN_HALF as isize || dy == DOCK_SIDEBTN_HALF as isize {
                return THEME.border;
            }
            return THEME.surface_container;
        }

        return THEME.surface;
    }

    // Home gesture bar at bottom
    if HOME_PILL.contains(x, y) {
        return THEME.on_surface_variant;
    }

    THEME.surface
}

/// The framing guides, in viewfinder-relative coordinates.
///
/// WHY the ink is the theme's: the guides used to carry colour literals of
/// their own - a translucent white for the brackets, a translucent white for
/// the reticle and a translucent white for the grid - which meant they did
/// not follow the theme and the brackets were invisible against the surface
/// they sat on.
fn guide_pixel(rel_x: usize, rel_y: usize) -> Option<u32> {
    // The corner brackets.
    let is_top_left = (rel_x <= BRACKET_ARM && rel_y <= 4) || (rel_x <= 4 && rel_y <= BRACKET_ARM);
    let is_top_right =
        (rel_x >= 650 && rel_y <= 4) || (rel_x >= 676 && rel_y <= BRACKET_ARM);
    let is_bot_left =
        (rel_x <= BRACKET_ARM && rel_y >= 996) || (rel_x <= 4 && rel_y >= 970);
    let is_bot_right = (rel_x >= 650 && rel_y >= 996) || (rel_x >= 676 && rel_y >= 970);
    if is_top_left || is_top_right || is_bot_left || is_bot_right {
        return Some(guide_ink());
    }

    // The focus reticle.
    let dx = (rel_x as isize - RETICLE_CX as isize).abs();
    let dy = (rel_y as isize - RETICLE_CY as isize).abs();
    let is_reticle = (dx <= RETICLE_ARM as isize && dy <= RETICLE_ARM as isize)
        && ((dx >= 36 && dy <= 12) || (dy >= 36 && dx <= 12));
    let is_crosshair = (dx <= 4 && dy <= 1) || (dy <= 4 && dx <= 1);
    if is_reticle || is_crosshair {
        return Some(THEME.warning);
    }

    // The thirds grid.
    let is_grid = (rel_x == 226 || rel_x == 453) || (rel_y == 333 || rel_y == 666);
    if is_grid {
        return Some(grid_ink());
    }

    None
}

/// The settings surface.
///
/// WHY the values are pre-rendered: the renderer is called once per pixel, and
/// formatting a value there would run tens of thousands of times for one row.
fn settings_pixel(x: usize, y: usize, camera: &CameraApp) -> u32 {
    // The card, in the platform's own treatment.
    if let Some(c) = std::design::primitives::draw_rounded_rect(
        x,
        y,
        SHEET_RECT,
        SHEET_CARD.radius,
        SHEET_CARD.fill,
        SHEET_CARD.border,
        1,
    ) {
        // The title.
        if y >= SHEET_TITLE_Y0 && y < SHEET_TITLE_Y0 + 24 {
            if let Some(c) = test_text(
                x,
                y,
                SHEET_TEXT_X,
                SHEET_TITLE_Y0,
                1,
                b"CAMERA SETTINGS",
                SHEET_TEXT_W,
                THEME.on_surface,
                SHEET_CARD.fill,
            ) {
                return c;
            }
        }

        // The rows.
        for (index, row) in SETTINGS_ROW_ORDER.iter().enumerate() {
            let row_y = SHEET_ROW_Y0 + index * SHEET_ROW_H;
            if y < row_y || y >= row_y + SHEET_ROW_H {
                continue;
            }
            // The platform's list-row treatment, so a settings row here is a
            // settings row anywhere else in the system.
            let style = components::list_row_style(&THEME, false);
            // The row's own surface. WHY the row is drawn under its own text
            // and not left on the card: a settings row that is a card with
            // text on it is not the platform's row, and the user would be
            // looking at a different control in every other app.
            let plate = std::design::primitives::draw_rounded_rect(
                x,
                y,
                std::design::primitives::Rect::new(
                    SHEET_ROW_X0,
                    row_y,
                    SHEET_ROW_W,
                    SHEET_ROW_H - SHEET_ROW_GAP,
                ),
                style.radius,
                style.fill,
                None,
                0,
            );
            let title = camera.settings_row_title(*row);
            if y < row_y + 24 {
                if let Some(c) =
                    test_text(x, y, SHEET_TEXT_X, row_y, 1, title, SHEET_TEXT_W, THEME.on_surface_variant, style.fill)
                {
                    return c;
                }
            } else if y < row_y + 52 {
                let len = camera.settings_value_lens[index];
                let value = &camera.settings_values[index][..len];
                if let Some(c) =
                    test_text(x, y, SHEET_TEXT_X, row_y + 28, 1, value, SHEET_TEXT_W, THEME.on_surface, style.fill)
                {
                    return c;
                }
            } else {
                let note = camera.settings_row_note(*row);
                if !note.is_empty() {
                    if let Some(c) = test_text(
                        x,
                        y,
                        SHEET_TEXT_X,
                        row_y + 56,
                        1,
                        note,
                        SHEET_TEXT_W,
                        THEME.on_surface_variant,
                        style.fill,
                    ) {
                        return c;
                    }
                }
            }
            // The row's own surface, where no glyph landed.
            if let Some(plate) = plate {
                return plate;
            }
        }
        return c;
    }
    THEME.surface
}

fn render_camera_ui(surface: u32, camera: &CameraApp) {
    render_camera_band(surface, camera, 0, H);
}

/// Re-rasterises and writes only rows `y0..y1`.
///
/// WHY a band and not a rectangle: a row of the surface is one contiguous run in
/// the surface buffer, so `offset = y0 * W` writes a whole band with no stride
/// bookkeeping. Every band starts on a row boundary, so the running `x`/`y`
/// cursor below is exact.
///
/// WHY this is correct rather than a shortcut: the band is named after what the
/// touch actually moved (see `Dirty`), and every row outside it is correct
/// because nothing about it changed. The old renderer re-rasterised all
/// 1,036,800 pixels on every touch event, including touches that changed no
/// state at all.
fn render_camera_band(surface: u32, camera: &CameraApp, y0: usize, y1: usize) {
    let mut chunk = [0u32; CHUNK];
    let mut offset = y0 * W;
    let end = y1.min(H) * W;
    while offset < end {
        let count = (end - offset).min(CHUNK);
        let mut x = offset % W;
        let mut y = offset / W;
        for i in 0..count {
            chunk[i] = get_camera_pixel(x, y, camera);
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

/// Which rows of the surface a touch actually changed.
///
/// WHY this exists: the old loop rendered the whole surface after every tap and
/// flipped it, whether the tap had changed the flash state, the ISP filter, the
/// mode or nothing at all. Naming the region lets a tap that changes one pill
/// cost one band.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Dirty {
    /// Nothing moved; the surface is already correct, so do not render at all.
    None,
    /// Flash or ISP filter changed: the top control row only.
    TopRow,
    /// The viewfinder changed: the guides, the absence panel or the settings
    /// sheet, which is drawn over the viewfinder.
    Viewfinder,
    /// More than one band moved: the whole surface.
    ///
    /// WHY there is no `ModeStrip` variant even though the strip is one band:
    /// `mode` is read twice - by the carousel's label colour *and* by the shutter
    /// core's fill - so no single band covers it. Naming a band that would
    /// leave half the change stale is worse than naming none.
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

enum Tap {
    /// Nothing on screen moved.
    None,
    /// The model moved; render the named region.
    Redraw(Dirty),
    /// Leave to the launcher, logging this exact reason first.
    Leave(&'static str),
}

fn exit_to_launcher(surface: u32) -> ! {
    if surface > 0 {
        // WHY no slot argument: a slot is a position in this task's
        // capability table, and the kernel authorises the release by the
        // capability's type. The surface must be handed back or it stays a
        // leaked KMS surface for the life of the device.
        let _ = std::surface_destroy(surface);
    }
    std::exit_with_code(0);
}

/// Hands the last photo to the app that opens images and reports what the
/// platform answered.
///
/// WHY the refusal is reported and not swallowed: an app that holds no
/// intent-dispatch authority cannot open anything, and a button that says it
/// opened the gallery when it did not is the defect this app is being fixed
/// for.
fn open_photo_library(camera: &CameraApp) {
    match camera.open_photo_library() {
        Ok(_) => {
            std::log("[CAMERA APP] The platform handed the photo to the app that handles images.")
                .ok();
        }
        Err(camera::ERR_NO_PHOTO) => {
            std::log("[CAMERA APP] No photo to open: this app has not written one yet.").ok();
        }
        Err(code) => {
            let mut buf = [0u8; 96];
            let prefix = b"[CAMERA APP] The platform refused to hand the photo over, code -";
            let p = prefix.len().min(buf.len());
            buf[..p].copy_from_slice(&prefix[..p]);
            let n = p + camera::append_decimal(code.unsigned_abs() as u32, &mut buf[p..]);
            if let Ok(msg) = core::str::from_utf8(&buf[..n]) {
                std::log(msg).ok();
            }
            std::log("[CAMERA APP] This app holds no intent-dispatch authority, so it cannot open the gallery.")
                .ok();
        }
    }
}

/// Handles a tap on the settings surface.
///
/// WHY the rows are answered here and not in the main handler: the settings
/// surface is a screen of its own, and a row that changed nothing was a row
/// that looked like a control.
/// Writes a log line that ends with the platform's own refusal code.
///
/// WHY the code is carried through untouched: it is the kernel's answer,
/// not a constant written here, and a message that softened it would be
/// reporting something the platform did not say.
fn log_capture_code(prefix: &[u8], code: i64) {
    let mut buf = [0u8; 96];
    let p = prefix.len().min(buf.len());
    buf[..p].copy_from_slice(&prefix[..p]);
    let n = p + camera::append_decimal(code.unsigned_abs() as u32, &mut buf[p..]);
    if let Ok(msg) = core::str::from_utf8(&buf[..n]) {
        std::log(msg).ok();
    }
}

fn handle_settings_tap(camera: &mut CameraApp, x: usize, y: usize) -> Tap {
    if !SHEET_RECT.contains(x, y) {
        return Tap::None;
    }
    if y < SHEET_ROW_Y0 {
        return Tap::None;
    }
    let index = (y - SHEET_ROW_Y0) / SHEET_ROW_H;
    if index >= SETTINGS_ROW_ORDER.len() {
        return Tap::None;
    }
    let row = SETTINGS_ROW_ORDER[index];
    match row {
        SettingsRow::Filter => {
            camera.filter = camera.filter.next();
            camera.refresh_settings_values();
            std::log("[CAMERA APP] The image processing for the next capture changed.").ok();
            Tap::Redraw(Dirty::All)
        }
        SettingsRow::Flash => {
            match camera.toggle_flash() {
                Some(on) => {
                    std::log(if on {
                        "[CAMERA APP] Flash hardware reported torch ON."
                    } else {
                        "[CAMERA APP] Flash hardware reported torch OFF."
                    })
                    .ok();
                }
                None => {
                    std::log("[CAMERA APP] Flash unavailable: the camera hardware backend refused the toggle; flash is off.")
                        .ok();
                }
            }
            camera.refresh_settings_values();
            Tap::Redraw(Dirty::All)
        }
        SettingsRow::Guides => {
            camera.guides = !camera.guides;
            camera.refresh_settings_values();
            std::log(if camera.guides {
                "[CAMERA APP] The framing guides are on."
            } else {
                "[CAMERA APP] The framing guides are off."
            })
            .ok();
            Tap::Redraw(Dirty::Viewfinder)
        }
        SettingsRow::OpenLibrary => {
            open_photo_library(camera);
            Tap::Redraw(Dirty::Viewfinder)
        }
        SettingsRow::Close => {
            camera.screen = CameraScreen::Viewfinder;
            Tap::Redraw(Dirty::Viewfinder)
        }
        SettingsRow::Format
        | SettingsRow::Photos
        | SettingsRow::LastPhoto
        | SettingsRow::LastCapture
        => {
            // These rows report what the platform said; they have nothing to
            // change.
            Tap::None
        }
    }
}

fn handle_camera_touch(camera: &mut CameraApp, x: u32, y: u32) -> Tap {
    let x = x as usize;
    let y = y as usize;

    // 1. Back control or the home gesture strip: exit to the Launcher.
    // WHY the named rectangles: the old test was `x 24..100 && y 28..68 ||
    // x 240..480 && y >= 1400`, a second transcription of the header's geometry
    // with no upper bound on y.
    if std::design::back_button::contains(x, y) || BACK_RECT.contains(x, y) || HOME_HIT.contains(x, y) {
        if camera.screen == CameraScreen::Settings {
            // The back control closes the settings before it leaves the app:
            // the settings are a sheet over the camera, not a separate place.
            camera.screen = CameraScreen::Viewfinder;
            return Tap::Redraw(Dirty::Viewfinder);
        }
        return Tap::Leave("[CAMERA] Back control or home gesture pressed. Exiting to launcher.");
    }

    // 2. The settings surface, when it is open.
    if camera.screen == CameraScreen::Settings {
        return handle_settings_tap(camera, x, y);
    }

    // 3. Flash mode toggle pill - a real control, backed by `camera_toggle_flash`.
    //
    // WHY: the flash pill previously advanced the local mode and then
    // discarded the `Err` from camera_toggle_flash, so the UI showed the next
    // flash mode (ending in a lit Torch) even though the backend refused the
    // request. Report the real outcome and keep the pill fail-closed.
    if FLASH_RECT.contains(x, y) {
        match camera.toggle_flash() {
            Some(on) => {
                std::log(if on {
                        "[CAMERA APP] Flash hardware reported torch ON."
                    } else {
                        "[CAMERA APP] Flash hardware reported torch OFF."
                    },
                )
                .ok();
            }
            None => {
                std::log("[CAMERA APP] Flash unavailable: the camera hardware backend refused the toggle; flash is off.",
                )
                .ok();
            }
        }
        let _ = std::audio_haptic_click();
        return Tap::Redraw(Dirty::TopRow);
    }

    // 4. ISP filter toggle pill - a real control: the filter is applied to the
    //    captured frame, so it changes the next capture rather than only a
    //    label.
    if FILTER_RECT.contains(x, y) {
        camera.filter = camera.filter.next();
        std::audio_haptic_click().ok();
        return Tap::Redraw(Dirty::TopRow);
    }

    // 5. Mode Selector Carousel: three equal 226px slots in 20..700, matching the
    //    render slots exactly (50px tall, >=48).
    if (MODE_STRIP_Y0..MODE_STRIP_Y1).contains(&y) {
        if x >= 20 && x < 246 {
            camera.mode = CameraMode::DocumentVision;
            std::log("[CAMERA APP] Switched to Document Vision mode.").ok();
        } else if x >= 247 && x < 473 {
            camera.mode = CameraMode::Photo;
            std::log("[CAMERA APP] Switched to Photo mode.").ok();
        } else if x >= 474 && x < 700 {
            camera.mode = CameraMode::Video;
            std::log("[CAMERA APP] Switched to Video mode.").ok();
        }
        std::audio_haptic_click().ok();
        // WHY the whole surface and not just the strip: `mode` is read in two
        // places - the mode carousel's label colour and the shutter core's fill
        // (crimson for Video, sky for Document). Rendering only the strip left
        // the shutter painted in the previous mode's colour, which is exactly
        // the class of bug banding is supposed to prevent.
        return Tap::Redraw(Dirty::All);
    }

    // 6. The photo library button. WHY it is a control now: the camera writes
    //    its photos into the shared storage area and the platform's document
    //    index describes them, so there is something behind it to open.
    if side_button_hit(x, y, PHOTOS_CX) {
        open_photo_library(camera);
        return Tap::Redraw(Dirty::Viewfinder);
    }

    // 7. The settings button.
    if side_button_hit(x, y, SETTINGS_CX) {
        camera.screen = CameraScreen::Settings;
        camera.refresh_settings_values();
        std::audio_haptic_click().ok();
        return Tap::Redraw(Dirty::Viewfinder);
    }

    // 8. Shutter Button. WHY gated on the authority probe: the shutter is drawn
    //    recessed when the kernel refused camera authority, and a recessed
    //    control that still fired would be the dead affordance again. WHY the
    //    circle test: it is derived from the radii the renderer draws, where the
    //    old rectangle included 24px above and 26px below the painted ring.
    if shutter_hit(x as isize - SHUTTER_CX as isize, y as isize - SHUTTER_CY as isize) {
        if camera.status == CameraStatus::AuthorityRefused {
            std::log("[CAMERA APP] Shutter unavailable: the kernel refused camera authority for this task.",
            )
            .ok();
            return Tap::None;
        }
        // WHY report the real outcome: `let _ =` here discarded the refusal, so a
        // press that captured nothing was indistinguishable from one that did.
        // The real `Err` code is carried through untouched - it is the kernel's
        // answer, never a constant written here.
        let outcome = if camera.mode == CameraMode::Video {
            camera.stream_video_frame()
        } else {
            camera.capture_still_photo()
        };
        // WHY each outcome is reported in its own words: "captured and
        // indexed", "captured but not indexed", "captured but not written"
        // and "not captured" are four different events. A screen that
        // called the last three of them a refusal would send the user
        // after the wrong thing.
        match outcome {
            CaptureOutcome::Published => {
                std::log(
                    "[CAMERA APP] The photo is on the device and the document index describes it.",
                )
                .ok();
            }
            CaptureOutcome::WrittenNotIndexed => {
                std::log(
                    "[CAMERA APP] The photo is on the device, but the document index did not describe it.",
                )
                .ok();
            }
            CaptureOutcome::WriteRefused(code) => {
                log_capture_code(
                    b"[CAMERA APP] The frame was captured but the platform refused to write it, code -",
                    code,
                );
            }
            CaptureOutcome::CaptureRefused(code) => {
                log_capture_code(b"[CAMERA APP] Capture refused, code -", code);
            }
            CaptureOutcome::NoVideoStream => {
                std::log(
                    "[CAMERA APP] The camera HAL exposes no video stream; this platform cannot deliver a video frame.",
                )
                .ok();
            }
            CaptureOutcome::None => {
                std::log("[CAMERA APP] The shutter produced no frame.").ok();
            }
        }
        camera.refresh_settings_values();
        return Tap::Redraw(Dirty::All);
    }

    Tap::None
}

#[no_mangle]
pub extern "C" fn _start() -> ! {
    std::log("[CAMERA APP (Ring 3)] Camera online.").ok();

    let mut camera = CameraApp::new();
    camera.refresh_shots_label();
    camera.refresh_status_text();

    // Ask the kernel whether this task may drive a camera at all.
    //
    // WHY before the first frame: the shutter, the capture-geometry readout and
    // the flash pill are all drawn from this answer, so the very first presented
    // frame already tells the truth. The probe is a read-only autofocus query
    // and changes no hardware state.
    camera.probe_camera_authority();
    match camera.status {
        CameraStatus::AuthorityRefused => {
            std::log("[CAMERA APP] The platform refused camera authority for this task; the viewfinder says so.",
            )
            .ok();
        }
        CameraStatus::NoSensor => {
            std::log("[CAMERA APP] Camera authority held, but the camera HAL reported that no sensor is bound.",
            )
            .ok();
        }
        CameraStatus::AuthorityHeld => {
            std::log("[CAMERA APP] The platform granted camera authority; capture controls are armed.",
            )
            .ok();
        }
        CameraStatus::Failed => {
            std::log("[CAMERA APP] The camera authority query failed; the viewfinder says so.",
            )
            .ok();
        }
    }

    // Allocate dedicated KMS Client Surface (z=200)
    std::log("[CAMERA APP] Step 1: Allocating application surface (z=200)...",
    )
    .ok();
    // WHY the slot is resolved by type: a slot number is a position in a
    // table, and the position depends on the order this task's
    // capabilities were inserted in. Six happened to hold the framebuffer
    // here, and would not in an app with one more capability.
    let surface = match std::surface_create(W as u32, H as u32, 200) {
        Ok(s) => {
            std::log("[CAMERA APP] Step 2: Surface created successfully.").ok();
            s
        }
        Err(_) => {
            std::log("[CAMERA APP] Surface allocation unavailable; falling back to headless execution.",
            )
            .ok();
            std::exit_with_code(0);
        }
    };

    // Render initial UI and present
    camera.mode = CameraMode::Photo;
    render_camera_ui(surface, &camera);
    let _ = std::surface_flip();
    std::log("[CAMERA APP] Step 3: Initial frame presented to hardware scanout.",
    )
    .ok();

    // Interactive Event Reactor Loop
    loop {
        if let Ok(events) = std::event_poll(std::EVENT_TOUCH_INPUT | std::EVENT_TIMER_TICK, 100) {
            // WHY the timer tick carries no work: the only thing the camera
            // used to do on a tick was ask for a video frame, and the camera
            // HAL has no stream to give, so every tick asked for a frame that
            // could not arrive. The video mode now says so when the shutter is
            // pressed, which is the moment the user is actually asking.
            let _ = &events;

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
                    if input.kind == 4 || input.kind == 6 || input.kind == 7 || input.kind == 8 || input.kind == 10 {
                        std::log("[CAMERA APP] Navigation gesture recognized. Returning to launcher.",
                        )
                        .ok();
                        exit_to_launcher(surface);
                    }
                    if input.kind == 1 {
                        match handle_camera_touch(&mut camera, input.x, input.y) {
                            Tap::Leave(msg) => {
                                std::log(msg).ok();
                                exit_to_launcher(surface);
                            }
                            Tap::Redraw(d) => dirty = dirty.widen(d),
                            Tap::None => {}
                        }
                    }
                }
                if dirty != Dirty::None {
                    match dirty {
                        Dirty::TopRow => {
                            render_camera_band(surface, &camera, TOP_BAND_Y0, TOP_BAND_Y1)
                        }
                        Dirty::Viewfinder => render_camera_band(
                            surface,
                            &camera,
                            VIEWFINDER_Y0,
                            VIEWFINDER_Y1,
                        ),
                        Dirty::All => render_camera_ui(surface, &camera),
                        Dirty::None => {}
                    }
                    let _ = std::surface_flip();
                }
            }
        }

        std::yield_now();
    }
}

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    std::log("[CAMERA APP] Panic detected! Terminating task...").ok();
    std::exit_with_code(1);
}
