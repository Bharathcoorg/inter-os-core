//! InterOS camera application, executing in Ring 3 (User Mode).
//!
//! - A capture engine that takes a still photograph and writes it into the
//!   shared storage area as a PPM (P6), and publishes it into the platform's
//!   document index, so the gallery can open the photo the user just took
//! - A mode carousel of Photo, Video and Document framing: three 216x48 pills,
//!   each painted at Scale 2 Body when the slot holds its label and at Scale 1
//!   when it does not, so no slot is ever taller than the pill it is drawn on
//! - A tone curve applied to the frame the platform returns, with six presets:
//!   Normal, Vivid, Warm, Cool, Mono, and Vintage
//! - Rule-of-Thirds 3x3 framing grid and Aspect Ratio selection (4:3, 16:9, 1:1)
//! - Flash modes (Auto, On, Off) with responsive visual pill indicators
//! - Bottom-left Gallery thumbnail disc showing live preview of the last
//!   captured photo and launching the Gallery app via intent or package launch
//! - DRM/KMS client surface rendering (z=200) with banded re-rasterisation
//! - Zero heap allocations in interactive rendering and event reactor loops

#![no_std]
#![no_main]

use core::panic::PanicInfo;
use interos_std as std;
use camera::theme::{chrome, radius, spacing, typography, Theme, CHASSIS_THEME, MIN_TOUCH_TARGET, THEME};

const STATUS_BAR_HEIGHT: usize = interos_std::design::viewport::STATUS_BAR_HEIGHT;

pub fn status_band() -> (usize, usize) {
    (0, STATUS_BAR_HEIGHT)
}

fn refresh_clock(camera: &mut CameraApp) {
    if let Some(now) = std::rtc::Reading::now() {
        camera.clock = now.clock_text();
        camera.clock_ok = true;
    } else {
        camera.clock_ok = false;
    }
}
use std::design::primitives::Rect;

use interos_std::design::outline_text;

use camera::{
    CameraApp, CameraAspectRatio, CameraFilter, CameraMode, CameraScreen, CameraStatus, CaptureOutcome, FlashMode,
    SettingsRow, TimerMode, SETTINGS_ROW_ORDER, THUMBNAIL_DIM,
};

/// The platform's corner radii, under the short names the renderer uses.
const R_CARD: usize = radius::CARD;

const W: usize = 720;
const H: usize = 1440;
const CHUNK: usize = 4096;

// -----------------------------------------------------------------------------
// Screen bands
// -----------------------------------------------------------------------------
const TOP_BAND_Y0: usize = 0;
const TOP_BAND_Y1: usize = 112;

/// The viewfinder and the dock take their geometry from the model, so the
/// rectangles the renderer paints are the rectangles the touch handler taps.
/// The mode carousel and the home strip below now obey the same rule.
const VIEWFINDER_Y0: usize = camera::VIEWFINDER_TOP;
const VIEWFINDER_Y1: usize = camera::VIEWFINDER_BOTTOM;

/// Every control aligns to the platform's 24px gutter.
const GUTTER: usize = spacing::GRID_24;
const VIEWFINDER_X0: usize = camera::VIEWFINDER_LEFT;
const VIEWFINDER_X1: usize = camera::VIEWFINDER_RIGHT;

/// Inner width of the viewfinder canvas (24px gutter on each edge).
const VIEWFINDER_W: usize = camera::VIEWFINDER_WIDTH;

/// The three chips over the viewfinder, in their row order.
/// Aspect ratio toggle (4:3 vs 16:9 vs 1:1).
const ASPECT_RECT: Rect = camera::chip_rect(0);
/// Rule-of-Thirds 3x3 framing grid toggle.
const GRID_RECT: Rect = camera::chip_rect(1);
/// Self-timer mode toggle.
const TIMER_RECT: Rect = camera::chip_rect(2);

/// The preview cut each aspect ratio letterboxes to, as (top, height) inside
/// the viewfinder the model owns.
///
/// WHY the three are named: the card the platform's verdict rides on is placed
/// at the frame's centre, and a centre that moved with the aspect ratio would
/// move the card under the user's thumb. All three cuts are centred, so they
/// share [`FRAME_CENTRE_Y`] -- the gate below is what keeps them sharing it.
const FRAME_16_9_Y: usize = VIEWFINDER_Y0;
const FRAME_16_9_H: usize = 1000;
const FRAME_4_3_Y: usize = VIEWFINDER_Y0 + 47;
const FRAME_4_3_H: usize = 906;
const FRAME_1_1_Y: usize = VIEWFINDER_Y0 + 160;
const FRAME_1_1_H: usize = 680;

/// The active frame's centre row, the same for every cut.
const FRAME_CENTRE_Y: usize = FRAME_16_9_Y + FRAME_16_9_H / 2;

/// WHY checked at compile time: the letterbox is the one thing the aspect chip
/// changes that the rest of the viewfinder is measured against. Grow a cut
/// asymmetrically, or shrink the viewfinder, and the first gate fails; a cut
/// that no longer ends inside the well fails the second.
const _: () = assert!(FRAME_4_3_Y + FRAME_4_3_H / 2 == FRAME_CENTRE_Y);
const _: () = assert!(FRAME_1_1_Y + FRAME_1_1_H / 2 == FRAME_CENTRE_Y);
const _: () = assert!(FRAME_16_9_Y + FRAME_16_9_H <= VIEWFINDER_Y1);
const _: () = assert!(FRAME_4_3_Y + FRAME_4_3_H <= VIEWFINDER_Y1);
const _: () = assert!(FRAME_1_1_Y + FRAME_1_1_H <= VIEWFINDER_Y1);

/// The three mode slots, as one box each: the pill the painter fills is the
/// box the tap handler answers.
///
/// WHY the box and not the band: the strip used to be tested as `1144..1204`
/// while the pill was drawn at `1144 + 6` for 48 rows, so the six rows above
/// and the six below every slot answered a mode change at a pixel that holds
/// chassis, and the 12px gap between two slots answered a haptic click and a
/// full redraw that changed nothing at all.
const MODE_SLOT_W: usize = 216;
const MODE_SLOT_H: usize = 48;
const MODE_SLOT_Y: usize = 1150;
const MODE_SLOT_GAP: usize = 12;
const MODE_X0: usize = GUTTER;
const MODE_X1: usize = MODE_X0 + MODE_SLOT_W + MODE_SLOT_GAP;
const MODE_X2: usize = MODE_X1 + MODE_SLOT_W + MODE_SLOT_GAP;

/// The three slots, in the order the carousel shows them.
const MODE_SLOTS: [Rect; 3] = [
    Rect::new(MODE_X0, MODE_SLOT_Y, MODE_SLOT_W, MODE_SLOT_H),
    Rect::new(MODE_X1, MODE_SLOT_Y, MODE_SLOT_W, MODE_SLOT_H),
    Rect::new(MODE_X2, MODE_SLOT_Y, MODE_SLOT_W, MODE_SLOT_H),
];

/// The modes the slots select, indexed with [`MODE_SLOTS`].
const MODE_ORDER: [CameraMode; 3] = [
    CameraMode::DocumentVision,
    CameraMode::Photo,
    CameraMode::Video,
];

/// The label each slot carries, indexed with [`MODE_SLOTS`].
const MODE_LABELS: [&[u8]; 3] = [b"DOCUMENT", b"PHOTO", b"VIDEO"];

/// What the carousel logs when each slot is chosen, indexed with
/// [`MODE_SLOTS`].
const MODE_FEEDBACK: [&'static str; 3] = [
    "[CAMERA APP] Switched to Document Vision mode.",
    "[CAMERA APP] Switched to Photo mode.",
    "[CAMERA APP] Switched to Video mode.",
];

/// The strip's painted extent, read off the slots that make it up.
const MODE_STRIP_Y0: usize = MODE_SLOT_Y;
const MODE_STRIP_Y1: usize = MODE_SLOT_Y + MODE_SLOT_H;

/// WHY checked at compile time: the three slots are only "equal slots inside
/// the 24px gutters" while the chain reaches the right gutter exactly, and the
/// strip is only a strip while it sits between the viewfinder and the dock.
/// Widen a slot, or add a fourth, and the first gate fails; move the dock or
/// the viewfinder and the last two do.
const _: () = assert!(MODE_X2 + MODE_SLOT_W == W - GUTTER);
const _: () = assert!(MODE_SLOT_H >= MIN_TOUCH_TARGET);
const _: () = assert!(MODE_STRIP_Y0 >= VIEWFINDER_Y1);
const _: () = assert!(MODE_STRIP_Y1 <= camera::DOCK_Y0);

const DOCK_Y0: usize = camera::DOCK_Y0;
const DOCK_Y1: usize = H;

/// The last row the dock band paints: everything under it is the system's home
/// strip, which the fall-through below draws.
const DOCK_BAND_Y1: usize = DOCK_Y1 - 30;

// -----------------------------------------------------------------------------
// Top control row geometry (height 48px, all >= 48px touch affordance)
// -----------------------------------------------------------------------------
/// WHY the row starts at HEADER_TOP and not at 24: the band owns the first 48
/// rows and answers the pixel function before this row is reached, so half of
/// every control here was painted over.
const TOP_ROW_Y: usize = interos_std::design::viewport::HEADER_TOP;
const TOP_ROW_H: usize = 48;

/// Back control: the SDK chip's painted box, read off the SDK and not re-mixed
/// from this row's own gutter and height.
///
/// WHY it is still named here at all: the row below is a chain of neighbours
/// gated edge-to-edge, and the chain has to start from the control it cannot
/// move. Deriving it from `back_button` keeps that chain honest while making the
/// chip impossible to re-size here by accident -- the painter asks
/// `back_button::pixel_for_theme` (24..72 by 56..104), so a `BACK_RECT` that
/// drifted from it would be a box the press answers and the screen never inks.
const BACK_RECT: Rect = Rect::new(
    std::design::back_button::X,
    std::design::back_button::Y,
    std::design::back_button::SIZE,
    std::design::back_button::SIZE,
);
/// Flash / torch pill: 116x48 (>=48px).
const FLASH_RECT: Rect = Rect::new(80, TOP_ROW_Y, 116, TOP_ROW_H);
/// Capture-geometry readout: 150x48. Reports platform geometry.
const FRAME_RECT: Rect = Rect::new(204, TOP_ROW_Y, 150, TOP_ROW_H);
/// ISP filter pill: 170x48 (>=48px).
const FILTER_RECT: Rect = Rect::new(362, TOP_ROW_Y, 170, TOP_ROW_H);
/// Capture counter badge: 156x48. Reports `total_photos`.
const SHOTS_RECT: Rect = Rect::new(540, TOP_ROW_Y, 156, TOP_ROW_H);

/// Corner radius of the floating pills.
const TOP_PILL_RADIUS: usize = radius::SOFT_PILL;

/// WHY checked at compile time: the header row is a chain of boxes whose edges
/// are four literals each, and the band that owns it ends two rows above the
/// row's own foot. Widen one control and the first gate to fail names the
/// neighbour it would have painted over; grow the row and the last two fail.
const _: () = assert!(BACK_RECT.x + BACK_RECT.w <= FLASH_RECT.x);
const _: () = assert!(FLASH_RECT.x + FLASH_RECT.w <= FRAME_RECT.x);
const _: () = assert!(FRAME_RECT.x + FRAME_RECT.w <= FILTER_RECT.x);
const _: () = assert!(FILTER_RECT.x + FILTER_RECT.w <= SHOTS_RECT.x);
const _: () = assert!(SHOTS_RECT.x + SHOTS_RECT.w <= W - GUTTER);
const _: () = assert!(TOP_ROW_Y >= STATUS_BAR_HEIGHT);
const _: () = assert!(TOP_ROW_Y + TOP_ROW_H <= TOP_BAND_Y1 - 2);
const _: () = assert!(TOP_ROW_H >= MIN_TOUCH_TARGET);
/// The flash pictogram keeps its own room, clear of the label beside it.
const _: () = assert!(FLASH_ICON_CX + 10 < FLASH_TEXT_X);
const _: () = assert!(FLASH_TEXT_X + FLASH_TEXT_W <= FLASH_RECT.x + FLASH_RECT.w);

/// The status card that carries the platform's own verdict on the camera.
///
/// WHY its width and column come from the model: the model chooses the words
/// and measures them against the column they are drawn in, and the only thing
/// that keeps a refusal notice from losing its own tail is the two of them
/// agreeing on one number.
const STATUS_CARD_W: usize = camera::STATUS_CARD_W;
/// The card's height: the stack it reserves, plus the air above and below it.
///
/// WHY derived and not pinned: the stack is the headline's line box and the
/// reason's, one box per line the model may split a reason across. Pin the
/// height and the ladder's next raise overflows the card it should be sizing.
const STATUS_STACK_GAP: usize = spacing::GRID_8;
const STATUS_HEAD_BOX: usize = std::design::font::line_height(camera::STATUS_HEAD_RUNG);
const STATUS_SUB_BOX: usize = std::design::font::line_height(camera::STATUS_REASON_RUNG);
const STATUS_STACK_H: usize = STATUS_HEAD_BOX
    + STATUS_STACK_GAP
    + camera::STATUS_REASON_LINES * STATUS_SUB_BOX;
const STATUS_CARD_V_PAD: usize = spacing::GRID_20;
const STATUS_CARD_H: usize = STATUS_STACK_H + 2 * STATUS_CARD_V_PAD;
/// The card, centred on the active frame's centre row inside the well.
const STATUS_CARD: Rect = Rect::new(
    VIEWFINDER_X0 + (VIEWFINDER_W - STATUS_CARD_W) / 2,
    FRAME_CENTRE_Y - STATUS_CARD_H / 2,
    STATUS_CARD_W,
    STATUS_CARD_H,
);
/// The gutter the card's status dot lives in.
const STATUS_ICON_GUTTER: usize = camera::STATUS_ICON_GUTTER;
/// Where the card's runs start.
const STATUS_TEXT_X: usize = STATUS_CARD.x + STATUS_ICON_GUTTER;
/// The column the card's runs are measured and clipped against.
const STATUS_TEXT_W: usize = camera::STATUS_TEXT_W;

/// The air above the block, which is the air below it.
const STATUS_STACK_PAD: usize = STATUS_CARD_V_PAD;
/// The headline's line box top.
const STATUS_HEAD_Y: usize = STATUS_CARD.y + STATUS_STACK_PAD;
/// The reason's first box: below the whole box of the run above it.
const STATUS_SUB_Y: usize = STATUS_HEAD_Y + STATUS_HEAD_BOX + STATUS_STACK_GAP;
/// The reason's second box, when the reason needed a split.
const STATUS_SUB2_Y: usize = STATUS_SUB_Y + STATUS_SUB_BOX;

/// The status dot, on the headline's cap centre inside the icon gutter.
const STATUS_DOT_CX: usize = STATUS_CARD.x + 30;
const STATUS_DOT_CY: usize = STATUS_HEAD_Y
    + std::design::font::ascent_for_scale(camera::STATUS_HEAD_RUNG)
    - std::design::font::cap_for_scale(camera::STATUS_HEAD_RUNG) as usize / 2;
/// The dot's core and the halo around it.
const STATUS_DOT_R: usize = 6;
const STATUS_DOT_HALO_R: usize = 11;

/// WHY checked at compile time: the card is dropped on the tightest letterbox
/// the aspect chip can select, so the first gate is the one a taller stack or a
/// shorter 1:1 cut breaks; the second is what a widened gutter or a deeper
/// right margin would break, and the third keeps the halo from reaching the
/// card's own hairline. The fourth is the one that fires if the model splits a
/// reason across more lines than the card reserves.
const _: () = assert!(STATUS_STACK_PAD * 2 + STATUS_STACK_H <= STATUS_CARD_H);
const _: () = assert!(STATUS_SUB2_Y + STATUS_SUB_BOX <= STATUS_CARD.y + STATUS_CARD_H);
const _: () = assert!(STATUS_CARD.y >= FRAME_1_1_Y);
const _: () = assert!(STATUS_CARD.y + STATUS_CARD_H <= FRAME_1_1_Y + FRAME_1_1_H);
const _: () = assert!(STATUS_TEXT_X + STATUS_TEXT_W <= STATUS_CARD.x + STATUS_CARD_W);
const _: () = assert!(STATUS_DOT_CX - STATUS_DOT_HALO_R >= STATUS_CARD.x);
const _: () = assert!(camera::STATUS_REASON_LINES * STATUS_SUB_BOX <= STATUS_STACK_H);

/// The self-timer ring's outer and inner radius; the digit lives inside the
/// inner one.
const TIMER_RING_OUTER_R: usize = 52;
const TIMER_RING_INNER_R: usize = 44;

/// The video mode's record chip, dropped this many rows below the top of the
/// active cut.
const REC_CHIP_W: usize = 120;
/// WHY the height is derived from the body rung and not pinned: the raised
/// rung grew the run's line box to 39 rows, and a 36-row chip no longer holds
/// it. Twice (ascent - cap / 2) is the shortest chip whose middle is deep
/// enough for `centred_text_top` to place the run rather than refuse it, so
/// the cap block sits centred and the whole line box stays inside.
const REC_CHIP_H: usize = (std::design::font::ascent_for_scale(typography::BODY_SCALE)
    - std::design::font::cap_for_scale(typography::BODY_SCALE) as usize / 2)
    * 2;
const REC_CHIP_TOP: usize = 16;
/// The dot's centre, measured into the chip, and its radius.
const REC_DOT_CX_OFF: usize = 16;
const REC_DOT_R: usize = 4;
/// Where the chip's run starts, and the margin it keeps from the hairline.
const REC_TEXT_OFF: usize = 32;
const REC_TEXT_MARGIN: usize = 8;
/// The column the run is measured against: the chip, less the run's start and
/// the margin on its far side.
const REC_TEXT_W: usize = REC_CHIP_W - REC_TEXT_OFF - REC_TEXT_MARGIN;

/// WHY checked at compile time: the chip's run used to be measured against
/// `w - 32`, which reached the chip's own right edge, so a time the face set
/// even a few rows wider inked the border. These two gates are what a wider
/// dot gutter, a deeper run start or a raised body rung would break.
const _: () = assert!(REC_TEXT_OFF + REC_TEXT_W + REC_TEXT_MARGIN <= REC_CHIP_W);
const _: () = assert!(
    centred_text_top(REC_CHIP_H, typography::BODY_SCALE)
        + std::design::font::line_height(typography::BODY_SCALE)
        <= REC_CHIP_H
);
/// WHY: the chip hangs from the top of the cut, so it has to clear the shortest
/// one the aspect chip offers, and it stands to the right of the chip row that
/// floats over the same well -- a wider chip or a longer chip row would put the
/// two on top of each other.
const _: () = assert!(REC_CHIP_TOP + REC_CHIP_H <= FRAME_1_1_H);
const _: () = assert!(TIMER_RECT.x + TIMER_RECT.w <= SHUTTER_CX - REC_CHIP_W / 2);

/// Centre of the flash pictogram inside `FLASH_RECT`.
const FLASH_ICON_CX: usize = FLASH_RECT.x + 24;
const FLASH_ICON_CY: usize = FLASH_RECT.y + FLASH_RECT.h / 2;
/// Text room to the right of the flash pictogram, inside `FLASH_RECT`.
const FLASH_TEXT_X: usize = FLASH_RECT.x + 36;
const FLASH_TEXT_W: usize = FLASH_RECT.w.saturating_sub(44);

/// The system's home indicator, as the pill this app draws.
///
/// Both numbers come from `std::design::home_band` now, because the mark every
/// application draws and the box that answers it used to be four literals
/// written by hand in this file, and they had already drifted: the pill was 200
/// columns by 6 rows while the press answered those same 200 columns from the
/// pill's own top row, so the ink and its neighbourhood were one declaration
/// that no other screen agreed with.
///
/// WHY the answering box is `hit_from(DOCK_BAND_Y1)` and not the SDK's default:
/// the shutter dock paints down to row 1410, and the default box begins at 1392
/// -- inside the dock. That was measured and reported before, when a 240x48 box
/// at 1392 closed the app on a press of blank chassis above the pill; the fix is
/// not to go back there. The box therefore claims every row the dock does not
/// own, from its last row to the foot of the canvas, and the SDK's own centred
/// 240 columns: 240x30 at 1410, which covers the mark and is 40 columns wider
/// and 8 rows deeper than the box it replaces.
///
/// WHAT IT CANNOT REACH: the 44-row touch floor. This screen gives up 30 rows to
/// the gesture, because its dock wants the other 1410. The floor-sized escape
/// here is the header's back chip, which the same press answers at the full
/// 48x48, and the pan and fling the event loop recognises still carry the
/// platform's wider swipe.
const HOME_PILL: Rect = std::design::home_band::PILL;
const HOME_HIT: Rect = std::design::home_band::hit_from(DOCK_BAND_Y1);

/// WHY checked at compile time: the indicator is bottom-anchored, so a taller
/// mark or a shorter canvas puts it off the surface, and a dock band that grew
/// past its own last row would paint over it.
const _: () = assert!(HOME_PILL.x + HOME_PILL.w <= W);
const _: () = assert!(HOME_PILL.y + HOME_PILL.h <= H);
const _: () = assert!(DOCK_BAND_Y1 <= HOME_PILL.y);
const _: () = assert!(HOME_HIT.y <= HOME_PILL.y);
const _: () = assert!(HOME_HIT.y + HOME_HIT.h == H);
const _: () = assert!(HOME_HIT.x <= HOME_PILL.x && HOME_HIT.x + HOME_HIT.w >= HOME_PILL.x + HOME_PILL.w);

// Shutter button (r=48 core, outer ring 64px), as the model places it.
const SHUTTER_CX: usize = camera::SHUTTER_CX;
const SHUTTER_CY: usize = camera::SHUTTER_CY;
const SHUTTER_RING_OUTER: usize = camera::SHUTTER_RING_OUTER;
const SHUTTER_RING_INNER: usize = camera::SHUTTER_RING_INNER;
const SHUTTER_R_CORE: usize = camera::SHUTTER_R_CORE;

/// The dock's two side controls, as the centres of their pictograms.
const PHOTOS_CX: usize = 110;
const SETTINGS_CX: usize = 610;

/// Half-extent of the dock's side buttons (diameter 64px >= 48px).
const DOCK_SIDEBTN_HALF: usize = 32;

/// WHY checked at compile time: the three dock controls share one row and are
/// tested as circles around it, so a wider disc or a moved centre would put the
/// gallery control under the shutter's ring -- a tap that answers one control
/// while the eye reads another. The last gate is the band's own: the dock must
/// keep painting the whole shutter, and stop above the home strip.
const _: () = assert!(PHOTOS_CX + DOCK_SIDEBTN_HALF < SHUTTER_CX - SHUTTER_RING_OUTER);
const _: () = assert!(SETTINGS_CX - DOCK_SIDEBTN_HALF > SHUTTER_CX + SHUTTER_RING_OUTER);
const _: () = assert!(PHOTOS_CX >= DOCK_SIDEBTN_HALF + GUTTER);
const _: () = assert!(SETTINGS_CX + DOCK_SIDEBTN_HALF <= W - GUTTER);
const _: () = assert!(SHUTTER_CY + SHUTTER_RING_OUTER < DOCK_BAND_Y1);
const _: () = assert!(SHUTTER_CY - SHUTTER_RING_OUTER >= DOCK_Y0);

// -----------------------------------------------------------------------------
// The settings surface.
// -----------------------------------------------------------------------------
/// The sheet the rows live on. WHY its height is derived: the sheet used to be
/// a remembered 960 while the plate count, the plate height and the row pitch
/// were remembered too, and none of them followed the type ladder. The bottom
/// edge is now wherever the last plate's is, and the gate beside it fails the
/// build if that edge ever reaches the mode strip.
const SHEET_TITLE_Y0: usize = 160;
/// The gap between the sheet title's line box and the hairline under it.
const SHEET_RULE_GAP: usize = 6;
const SHEET_ROW_Y0: usize = SHEET_TITLE_Y0 + SHEET_TITLE_BOX + SHEET_RULE_GAP;
/// The air above and below the three runs inside one plate.
const SHEET_PLATE_PAD: usize = 1;
/// The gap between two plates.
const SHEET_ROW_GAP: usize = 4;
/// The pitch two plates stand on: a plate and the gap under it.
const SHEET_ROW_H: usize = SHEET_PLATE_H + SHEET_ROW_GAP;
/// A plate's own height -- the box its runs are centred in, and the box the
/// tap answers.
///
/// WHY the plate and not the pitch: the row used to be tapped on the 96-row
/// pitch it is laid out on, so the 8 gap rows under a plate answered that
/// plate although no pixel of it is drawn there.
const SHEET_PLATE_H: usize = SHEET_STACK_H + SHEET_PLATE_PAD * 2;
/// How many plate pitches the sheet can show between its first row and the
/// mode strip, derived from the pitch the plates actually stand on.
///
/// WHY the list pages: the raised rungs grew the stack to 108 rows, a plate
/// 110 and a pitch 114, so nine rows reached row 1231 -- 81 rows past the
/// strip at 1150 -- and no lawful trimming of the air above the title buys
/// them back. The sheet keeps the rungs, shows the seven rows its slots hold
/// and turns a page for the rest.
const SHEET_SLOTS: usize = (MODE_STRIP_Y0 - SHEET_ROW_Y0) / SHEET_ROW_H;
/// The plates one page shows: every slot but the last, which is the pager.
const SHEET_PAGE_ROWS: usize = SHEET_SLOTS - 1;
/// How many pages the full row order needs at that window.
const SHEET_PAGES: usize = (camera::SETTINGS_ROWS + SHEET_PAGE_ROWS - 1) / SHEET_PAGE_ROWS;
/// Where the sheet ends: the last slot's pitch, so the frame is the page.
const SHEET_BOTTOM: usize = SHEET_ROW_Y0 + SHEET_SLOTS * SHEET_ROW_H;
const SHEET_RECT: Rect = Rect::new(VIEWFINDER_X0, 140, W - 2 * VIEWFINDER_X0, SHEET_BOTTOM - 140);
const SHEET_ROW_X0: usize = camera::SETTINGS_PLATE_X;
const SHEET_ROW_W: usize = camera::SETTINGS_PLATE_W;
const SHEET_TEXT_X: usize = SHEET_ROW_X0 + camera::SETTINGS_TEXT_INSET;
const SHEET_TEXT_W: usize = camera::SETTINGS_TEXT_MAX;

/// The row's runs, as the line boxes of the faces they are drawn with.
const SHEET_TITLE_BOX: usize = std::design::font::line_height(typography::BODY_SCALE);
const SHEET_VALUE_BOX: usize = std::design::font::line_height(typography::BODY_SCALE);
const SHEET_NOTE_BOX: usize = std::design::font::line_height(typography::CAPTION_SCALE);
/// The three runs as one block, so the block -- not either line -- is centred.
const SHEET_STACK_H: usize = SHEET_TITLE_BOX + SHEET_VALUE_BOX + SHEET_NOTE_BOX;
/// The air above the block, which is the air below it.
const SHEET_STACK_PAD: usize = (SHEET_PLATE_H - SHEET_STACK_H) / 2;
/// Each run's offset from the top of its plate.
const SHEET_TITLE_OFF: usize = SHEET_STACK_PAD;
const SHEET_VALUE_OFF: usize = SHEET_TITLE_OFF + SHEET_TITLE_BOX;
const SHEET_NOTE_OFF: usize = SHEET_VALUE_OFF + SHEET_VALUE_BOX;

/// The hairline under the sheet's title.
const SHEET_HAIRLINE_Y: usize = SHEET_ROW_Y0 - 6;
/// The amber accent beside the title: exactly the height of the title's caps,
/// so the bar reads as a rule against the word rather than a stripe of its own.
const SHEET_ACCENT_X: usize = SHEET_ROW_X0;
const SHEET_ACCENT_W: usize = 4;
const SHEET_ACCENT_TOP: usize = SHEET_TITLE_Y0
    + std::design::font::ascent_for_scale(typography::BODY_SCALE)
    - std::design::font::cap_for_scale(typography::BODY_SCALE) as usize;
const SHEET_ACCENT_H: usize = std::design::font::cap_for_scale(typography::BODY_SCALE) as usize;

/// The plate of the row at `index`: the box the painter fills and the tap
/// handler answers, named once.
const fn settings_row_rect(index: usize) -> Rect {
    Rect::new(
        SHEET_ROW_X0,
        SHEET_ROW_Y0 + index * SHEET_ROW_H,
        SHEET_ROW_W,
        SHEET_PLATE_H,
    )
}

/// WHY checked at compile time: the stack is only legal while the note's line
/// box still ends inside its plate -- raise a rung and that is the first gate
/// to fail. The chain gates are what a taller plate, a wider pitch or a
/// shorter sheet breaks, and they are the difference between a page whose last
/// plate fits and one that runs under the sheet's own hairline; the last two
/// keep the sheet clear of the mode strip and every row inside some page.
const _: () = assert!(SHEET_NOTE_OFF + SHEET_NOTE_BOX <= SHEET_PLATE_H);
const _: () = assert!(SHEET_TITLE_Y0 + SHEET_TITLE_BOX <= SHEET_HAIRLINE_Y);
const _: () = assert!(SHEET_ROW_Y0 + SHEET_SLOTS * SHEET_ROW_H <= SHEET_RECT.y + SHEET_RECT.h);
const _: () = assert!(
    settings_row_rect(SHEET_SLOTS - 1).y + SHEET_PLATE_H <= SHEET_RECT.y + SHEET_RECT.h
);
const _: () = assert!(SHEET_TEXT_X + SHEET_TEXT_W <= SHEET_ROW_X0 + SHEET_ROW_W);
const _: () = assert!(SHEET_ROW_X0 + SHEET_ROW_W <= SHEET_RECT.x + SHEET_RECT.w);
const _: () = assert!(SHEET_RECT.y + SHEET_RECT.h <= MODE_STRIP_Y0);
const _: () = assert!(SHEET_PAGES * SHEET_PAGE_ROWS >= camera::SETTINGS_ROWS);

const RETICLE_ARM: usize = 40;

/// Whether a touch at (dx, dy) from the shutter centre is on the shutter.
fn shutter_hit(dx: isize, dy: isize) -> bool {
    let r = SHUTTER_RING_OUTER as isize;
    dx * dx + dy * dy <= r * r
}

/// Whether a touch at (x, y) is on one of the dock's side buttons.
fn side_button_hit(x: usize, y: usize, cx: usize) -> bool {
    let dx = x as isize - cx as isize;
    let dy = y as isize - SHUTTER_CY as isize;
    (dx * dx + dy * dy) <= (DOCK_SIDEBTN_HALF * DOCK_SIDEBTN_HALF) as isize
}

// -----------------------------------------------------------------------------
// Typography provided by interos_std::design::font & outline_text
// -----------------------------------------------------------------------------

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
        if x < x0 || x - x0 >= w.min(max_w) || y < y0 || y - y0 >= h {
            return None;
        }
        return std::design::font::test_text_aa(x, y, x0, y0, scale, text, color, bg, false);
    }
    let (bx, bt, bw, bh) = outline_text::run_box(x0, y0, scale, text);
    if x < bx || x - bx >= bw || y < bt || y - bt >= bh {
        return None;
    }
    if x >= x0 + max_w {
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
    if dx.abs() <= 16 && dy.abs() <= 13 {
        if dx.abs() == 16 || dy.abs() == 13 {
            return true;
        }
        // Sun circle at (-7, -5), r=3
        let sun_dx = dx + 7;
        let sun_dy = dy + 5;
        if sun_dx * sun_dx + sun_dy * sun_dy <= 9 {
            return true;
        }
        // Mountain peaks
        if dy >= -1 && dy <= 10 {
            let m1 = (dx + 4).abs() * 2 <= (dy + 4);
            let m2 = (dx - 7).abs() * 2 <= (dy + 2);
            if m1 || m2 {
                return true;
            }
        }
    }
    false
}

/// Vector sliders pictogram for the settings button.
fn test_settings_icon(x: usize, y: usize, cx: usize, cy: usize) -> bool {
    let dx = x as isize - cx as isize;
    let dy = y as isize - cy as isize;
    let rows = [(-10i32, -6i32), (0i32, 6i32), (10i32, -2i32)];
    for (row_y, knob_x) in rows {
        let ry = dy - row_y as isize;
        if ry.abs() <= 1 && dx.abs() <= 14 {
            return true;
        }
        let kx = dx - knob_x as isize;
        if kx * kx + ry * ry <= 12 {
            return true;
        }
    }
    false
}

/// The rounded-rect body of a top-row pill.
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
    Some((chrome::SURFACE, on_border))
}

/// The inset a top-row pill keeps between its border and the label's ink.
///
/// WHY a named inset rather than the 12 the scale ladder used to divide by:
/// the same room is what decides whether an over-long label is trimmed, so a
/// number that only appeared in the ladder test let the label be measured
/// against the pill's full width and painted onto its own border.
const TOP_ROW_TEXT_PAD_X: usize = 6;

/// The top of a run whose caps block is centred in a box `h` rows tall, drawn
/// at rung `rung` with the outline face.
///
/// WHY the caps and not the line box: a run's line box carries the face's
/// ascender above the caps and its descender below the baseline, and the
/// ascender is the taller of the two. Centring the box therefore sits every
/// capitalised label a row or two low in its control, which on a 48-row pill
/// reads as a label that fell out of the capsule it is standing in.
///
/// WHY a subtraction that can be refused: a control shorter than the run's own
/// ascent has no centred placement, and answering zero leaves the run in the
/// box's top rows where the gate below can see it, rather than wrapping a
/// `usize` around the canvas.
const fn centred_text_top(h: usize, rung: usize) -> usize {
    let box_top = h / 2 + std::design::font::cap_for_scale(rung) as usize / 2;
    let ascent = std::design::font::ascent_for_scale(rung);
    if ascent > box_top {
        0
    } else {
        box_top - ascent
    }
}

/// WHY checked at compile time: the pills are one height and every label in
/// this app is centred in one, so a rung raised above what a 48-row capsule
/// holds would fail here rather than spill ink onto the chassis. The gates say
/// the run's whole line box stays inside the box it is dropped into.
const _: () = assert!(centred_text_top(TOP_ROW_H, typography::CAPTION_SCALE)
    + std::design::font::line_height(typography::CAPTION_SCALE)
    <= TOP_ROW_H);
const _: () = assert!(centred_text_top(TOP_ROW_H, typography::BODY_SCALE)
    + std::design::font::line_height(typography::BODY_SCALE)
    <= TOP_ROW_H);

/// Draws a label centred in `rect`, upgrading to Scale 2 Body where it fits.
///
/// WHY the label is trimmed instead of dropped: every `*_pixel` here answers
/// one pixel of the blit, so a label wider than its pill used to report that
/// it had nothing to draw and the pill rendered as bare chrome. An empty
/// control cannot be told apart from a control that is off, so the leading
/// word now stays and an ellipsis states that there was more.
#[inline(always)]
fn top_row_label(
    x: usize,
    y: usize,
    rect: Rect,
    label: &[u8],
    color: u32,
    bg: u32,
) -> Option<u32> {
    let room = rect.w.saturating_sub(TOP_ROW_TEXT_PAD_X * 2);
    if room == 0 {
        return None;
    }
    let scale = if std::design::font::measure(label, 2) <= room {
        2
    } else {
        1
    };
    let mut buf = [0u8; 96];
    let len = std::design::font::ellipsize(label, scale, room, &mut buf);
    if len == 0 {
        return None;
    }
    let text = &buf[..len];
    let drawn = outline_text::drawn_advance(text, scale, None)
        .unwrap_or_else(|| std::design::font::measure(text, scale))
        .min(room);
    let x0 = rect.x + (rect.w - drawn) / 2;
    let y0 = rect.y + centred_text_top(rect.h, scale);
    test_text(x, y, x0, y0, scale, text, room, color, bg)
}

/// Draws a label right of the flash pictogram, upgrading to Scale 2 Body where it fits.
///
/// Trimmed under the same rule as [`top_row_label`]: `pill_label()` is the
/// flash mode's own name and the modes are not the same length, so a label the
/// pill cannot hold at Scale 2 must say what it is rather than disappear.
#[inline(always)]
fn flash_label(x: usize, y: usize, label: &[u8], color: u32, bg: u32) -> Option<u32> {
    let scale = if std::design::font::measure(label, 2) <= FLASH_TEXT_W {
        2
    } else {
        1
    };
    let mut buf = [0u8; 96];
    let len = std::design::font::ellipsize(label, scale, FLASH_TEXT_W, &mut buf);
    if len == 0 {
        return None;
    }
    let text = &buf[..len];
    let drawn = outline_text::drawn_advance(text, scale, None)
        .unwrap_or_else(|| std::design::font::measure(text, scale))
        .min(FLASH_TEXT_W);
    let x0 = FLASH_TEXT_X + (FLASH_TEXT_W - drawn) / 2;
    let y0 = FLASH_RECT.y + centred_text_top(FLASH_RECT.h, scale);
    test_text(x, y, x0, y0, scale, text, FLASH_TEXT_W, color, bg)
}

/// The ink of the thirds grid: a quiet hairline on the well.
fn grid_ink() -> u32 {
    Theme::blend(chrome::WELL, chrome::TEXT_MUTED, 34)
}

/// The colour that states the platform's verdict, in the platform's roles.
fn status_ink(status: CameraStatus) -> u32 {
    match status {
        CameraStatus::AuthorityRefused | CameraStatus::Failed => chrome::REFUSED,
        CameraStatus::NoSensor => chrome::CAUTION,
        CameraStatus::AuthorityHeld => chrome::READY,
    }
}

/// The header overlay pills: aspect, grid and timer.
///
/// WHY this is painted after the letterbox: the pills float above the frame,
/// so a 4:3 or 1:1 matte must not erase half of them.
fn viewfinder_header_pixel(x: usize, y: usize, camera: &CameraApp) -> Option<u32> {
    if ASPECT_RECT.contains(x, y) {
        let fill = chrome::tint(chrome::SKY, 12);
        let c = std::design::primitives::draw_rounded_rect(
            x,
            y,
            ASPECT_RECT,
            TOP_PILL_RADIUS,
            fill,
            Some(chrome::tint(chrome::SKY, 70)),
            1,
        )?;
        let lbl = camera.aspect_ratio.label();
        return Some(top_row_label(x, y, ASPECT_RECT, lbl, chrome::SKY, fill).unwrap_or(c));
    }

    if GRID_RECT.contains(x, y) {
        let active = camera.guides;
        let fill = if active { chrome::tint(chrome::SKY, 14) } else { chrome::SURFACE };
        let ink = if active { chrome::SKY } else { chrome::TEXT_MUTED };
        let border = if active { chrome::SKY } else { chrome::BORDER };
        let c = std::design::primitives::draw_rounded_rect(
            x,
            y,
            GRID_RECT,
            TOP_PILL_RADIUS,
            fill,
            Some(border),
            1,
        )?;
        return Some(top_row_label(x, y, GRID_RECT, b"GRID", ink, fill).unwrap_or(c));
    }

    if TIMER_RECT.contains(x, y) {
        let active = camera.timer_mode != TimerMode::Off;
        let fill = if active { chrome::tint(chrome::SKY, 14) } else { chrome::SURFACE };
        let ink = if active { chrome::SKY } else { chrome::TEXT_MUTED };
        let border = if active { chrome::SKY } else { chrome::BORDER };
        let c = std::design::primitives::draw_rounded_rect(
            x,
            y,
            TIMER_RECT,
            TOP_PILL_RADIUS,
            fill,
            Some(border),
            1,
        )?;
        let lbl = camera.timer_mode.label();
        return Some(top_row_label(x, y, TIMER_RECT, lbl, ink, fill).unwrap_or(c));
    }
    None
}

/// The mode carousel: the selected mode sits in an amber capsule.
fn mode_strip_pixel(x: usize, y: usize, camera: &CameraApp) -> Option<u32> {
    for index in 0..MODE_SLOTS.len() {
        let rect = MODE_SLOTS[index];
        if !rect.contains(x, y) {
            continue;
        }
        let selected = camera.mode == MODE_ORDER[index];
        let fill = if selected { chrome::tint(chrome::AMBER, 14) } else { chrome::BODY };
        let border = if selected { chrome::tint(chrome::AMBER, 60) } else { chrome::BORDER };
        let ink = if selected { chrome::AMBER } else { chrome::TEXT_MUTED };
        let c = std::design::primitives::draw_rounded_rect(
            x,
            y,
            rect,
            TOP_PILL_RADIUS,
            fill,
            Some(border),
            1,
        )?;
        return Some(top_row_label(x, y, rect, MODE_LABELS[index], ink, fill).unwrap_or(c));
    }
    None
}

/// The mode slot under `(x, y)`, on the box [`mode_strip_pixel`] fills.
fn mode_slot_at(x: usize, y: usize) -> Option<usize> {
    let mut index = 0usize;
    while index < MODE_SLOTS.len() {
        if MODE_SLOTS[index].contains(x, y) {
            return Some(index);
        }
        index += 1;
    }
    None
}

#[inline(never)]
fn get_camera_pixel(x: usize, y: usize, camera: &CameraApp, page: usize) -> u32 {
    // 0. Status Band (y < STATUS_BAR_HEIGHT)
    if y < STATUS_BAR_HEIGHT {
        let band = interos_std::design::components::StatusBand::new(W);
        let clock = if camera.clock_ok { Some(&camera.clock[..]) } else { None };
        return band
            .pixel(x, y, clock, &interos_std::design::components::Instruments::unmeasured(), &THEME)
            .unwrap_or(THEME.surface);
    }

    // 1. Status Bar & Top Controls (y: 0..112)
    if y < TOP_BAND_Y1 {
        // WHY the control is asked before the band's own surfaces: it owns rows
        // 56..104 and every row it does not claim falls through to raw chassis,
        // so a chip asked after them is a chip that never reaches the scanout.
        // WHY [`CHASSIS_THEME`] and not the chassis hairline drawn here before:
        // that ring measured 1.90:1 over the band on a plate 1.09:1 above it, so
        // the control read as a bare chevron with no pressable thing around it,
        // while the dark roles put the ring at 18.37:1 and the chevron at 15.06:1
        // over the plate it sits on.
        if let Some(c) = std::design::back_button::pixel_for_theme(x, y, &CHASSIS_THEME) {
            return c;
        }

        if y >= TOP_BAND_Y1 - 2 {
            return chrome::BORDER;
        }

        // Flash / torch pill (116x48px)
        if let Some((fill, on_border)) = top_pill_body(x, y, FLASH_RECT) {
            let is_lit = camera.flash_available && camera.flash == FlashMode::On;
            let icon_color = if is_lit {
                chrome::AMBER
            } else if !camera.flash_available || camera.flash == FlashMode::Off {
                chrome::TEXT_MUTED
            } else {
                chrome::SKY // Auto
            };

            if test_lightning(x, y, FLASH_ICON_CX, FLASH_ICON_CY) {
                return icon_color;
            }

            let label: &[u8] = if !camera.flash_available {
                b"OFF"
            } else {
                camera.flash.pill_label()
            };
            if let Some(c) = flash_label(x, y, label, icon_color, fill) {
                return c;
            }

            if on_border {
                return if is_lit {
                    chrome::AMBER
                } else if camera.flash_available {
                    chrome::tint(chrome::SKY, 60)
                } else {
                    chrome::BORDER
                };
            }
            return fill;
        }

        // Capture geometry readout (150x48px)
        if let Some((_fill, on_border)) = top_pill_body(x, y, FRAME_RECT) {
            let (label, label_len) = camera.frame_readout();
            let label = &label[..label_len];
            let color = if camera.status == CameraStatus::AuthorityRefused {
                chrome::REFUSED
            } else {
                chrome::TEXT
            };
            if let Some(c) = top_row_label(x, y, FRAME_RECT, label, color, chrome::SURFACE) {
                return c;
            }
            if on_border {
                return chrome::BORDER;
            }
            return chrome::SURFACE;
        }

        // ISP filter pill (170x48px): a toggle whose name is a setting, so it
        // carries the sky ink of an adjustable control rather than the amber
        // of the capture row.
        if let Some((fill, on_border)) = top_pill_body(x, y, FILTER_RECT) {
            let filter_name: &[u8] = match camera.filter {
                CameraFilter::Normal => b"NORMAL",
                CameraFilter::Vivid => b"VIVID",
                CameraFilter::Warm => b"WARM",
                CameraFilter::Cool => b"COOL",
                CameraFilter::Mono => b"MONO",
                CameraFilter::Vintage => b"VINTAGE",
            };
            if let Some(c) = top_row_label(x, y, FILTER_RECT, filter_name, chrome::SKY, fill) {
                return c;
            }
            if on_border {
                return chrome::tint(chrome::SKY, 60);
            }
            return fill;
        }

        // Capture counter badge (156x48px)
        if let Some((_fill, on_border)) = top_pill_body(x, y, SHOTS_RECT) {
            let label = &camera.shots_label[..camera.shots_label_len];
            if let Some(c) = top_row_label(x, y, SHOTS_RECT, label, chrome::TEXT, chrome::SURFACE) {
                return c;
            }
            if on_border {
                return chrome::BORDER;
            }
            return chrome::SURFACE;
        }

        return chrome::BODY;
    }

    // 2. The settings surface (when open)
    if camera.screen == CameraScreen::Settings && SHEET_RECT.contains(x, y) {
        return settings_pixel(x, y, camera, page);
    }

    // 3. Viewfinder Canvas (y: VIEWFINDER_Y0..VIEWFINDER_Y1, inside gutters)
    if (VIEWFINDER_Y0..VIEWFINDER_Y1).contains(&y) && x >= VIEWFINDER_X0 && x < VIEWFINDER_X1 {
        let vf = Rect::new(
            VIEWFINDER_X0,
            VIEWFINDER_Y0,
            VIEWFINDER_W,
            VIEWFINDER_Y1 - VIEWFINDER_Y0,
        );
        if let Some(c) = std::design::primitives::draw_rounded_rect(
            x,
            y,
            vf,
            R_CARD,
            chrome::WELL,
            Some(chrome::BORDER_STRONG),
            1,
        ) {
            // Dynamic aspect ratio framing bounds
            let (active_y0, active_h) = match camera.aspect_ratio {
                CameraAspectRatio::Ratio16_9 => (FRAME_16_9_Y, FRAME_16_9_H),
                CameraAspectRatio::Ratio4_3 => (FRAME_4_3_Y, FRAME_4_3_H),
                CameraAspectRatio::Square => (FRAME_1_1_Y, FRAME_1_1_H),
            };
            let active_y1 = active_y0 + active_h;

            // The matte outside the active frame is darker. The header pills
            // are painted on top of it so a 4:3 or 1:1 mask cannot eat them.
            if y < active_y0 || y >= active_y1 {
                if let Some(c) = viewfinder_header_pixel(x, y, camera) {
                    return c;
                }
                return chrome::LETTERBOX;
            }

            let rel_frame_y = y - active_y0;
            let rel_frame_x = x - VIEWFINDER_X0;

            // The pills float above the frame, including its edge hairline.
            if let Some(c) = viewfinder_header_pixel(x, y, camera) {
                return c;
            }

            // The active frame's edge: a hairline that separates it from the matte.
            if y == active_y0
                || y == active_y1 - 1
                || x == VIEWFINDER_X0
                || x == VIEWFINDER_X1 - 1
            {
                return chrome::BORDER_STRONG;
            }

            // 3.1 3x3 Rule-of-Thirds grid inside the active frame
            if camera.guides {
                let is_third_x =
                    rel_frame_x == VIEWFINDER_W / 3 || rel_frame_x == (VIEWFINDER_W * 2) / 3;
                let is_third_y =
                    rel_frame_y == active_h / 3 || rel_frame_y == (active_h * 2) / 3;
                if is_third_x || is_third_y {
                    return grid_ink();
                }

                // Center focus reticle, amber like the shutter
                let ret_cx = VIEWFINDER_W / 2;
                let ret_cy = active_h / 2;
                let dx = (rel_frame_x as isize - ret_cx as isize).abs();
                let dy = (rel_frame_y as isize - ret_cy as isize).abs();
                let is_reticle = (dx <= RETICLE_ARM as isize && dy <= RETICLE_ARM as isize)
                    && ((dx >= 36 && dy <= 12) || (dy >= 36 && dx <= 12));
                let is_crosshair = (dx <= 4 && dy <= 1) || (dy <= 4 && dx <= 1);
                if is_reticle || is_crosshair {
                    return chrome::AMBER;
                }
            }

            // 3.2 Self-Timer Countdown Readout
            if camera.countdown_ticks > 0 {
                let center_x: usize = VIEWFINDER_X0 + VIEWFINDER_W / 2;
                let center_y: usize = active_y0 + active_h / 2;
                let c_dx = x as isize - center_x as isize;
                let c_dy = y as isize - center_y as isize;
                let dist_sq = (c_dx * c_dx + c_dy * c_dy) as usize;
                if dist_sq <= TIMER_RING_OUTER_R * TIMER_RING_OUTER_R
                    && dist_sq >= TIMER_RING_INNER_R * TIMER_RING_INNER_R
                {
                    return chrome::AMBER;
                }
                if dist_sq < TIMER_RING_INNER_R * TIMER_RING_INNER_R {
                    let sec = ((camera.countdown_ticks + 9) / 10).clamp(1, 9) as u8;
                    let digit = [b'0' + sec];
                    // WHY the disc is the centring box: the digit used to be
                    // clipped to 64 columns measured from its own centred left
                    // edge, which ran eight columns past the ring's outer edge,
                    // and its box was centred by half a remembered line height
                    // rather than by the caps. The disc is what the eye reads
                    // the number inside, so the disc is the box now.
                    let disc = TIMER_RING_INNER_R * 2;
                    let lx =
                        camera::centred_x(&digit, typography::BODY_SCALE, center_x - TIMER_RING_INNER_R, disc);
                    let ly = center_y
                        - TIMER_RING_INNER_R
                        + centred_text_top(disc, typography::BODY_SCALE);
                    if let Some(tc) = test_text(
                        x,
                        y,
                        lx,
                        ly,
                        typography::BODY_SCALE,
                        &digit,
                        disc,
                        chrome::AMBER,
                        chrome::SURFACE,
                    ) {
                        return tc;
                    }
                    return chrome::SURFACE;
                }
            }

            // 3.3 Video mode indicator / readout. The readout stays 00:00:
            // this platform exposes no video stream, so nothing is invented.
            if camera.mode == CameraMode::Video && camera.countdown_ticks == 0 {
                let rec_rect = Rect::new(
                    VIEWFINDER_X0 + VIEWFINDER_W / 2 - REC_CHIP_W / 2,
                    active_y0 + REC_CHIP_TOP,
                    REC_CHIP_W,
                    REC_CHIP_H,
                );
                if rec_rect.contains(x, y) {
                    if let Some(rc) = std::design::primitives::draw_rounded_rect(
                        x,
                        y,
                        rec_rect,
                        radius::CHIP,
                        chrome::SURFACE,
                        Some(chrome::RECORD),
                        1,
                    ) {
                        let dot_cx = rec_rect.x + REC_DOT_CX_OFF;
                        let dot_cy = rec_rect.y + rec_rect.h / 2;
                        let dot_dx = x as isize - dot_cx as isize;
                        let dot_dy = y as isize - dot_cy as isize;
                        if (dot_dx * dot_dx + dot_dy * dot_dy) <= (REC_DOT_R * REC_DOT_R) as isize {
                            return chrome::RECORD;
                        }
                        let text = b"00:00";
                        let lx = rec_rect.x + REC_TEXT_OFF;
                        let ly = rec_rect.y + centred_text_top(REC_CHIP_H, typography::BODY_SCALE);
                        if let Some(tc) = test_text(
                            x,
                            y,
                            lx,
                            ly,
                            typography::BODY_SCALE,
                            text,
                            REC_TEXT_W,
                            chrome::TEXT,
                            chrome::SURFACE,
                        ) {
                            return tc;
                        }
                        return rc;
                    }
                }
            }

            // 3.4 The platform's verdict, as a status card with a status dot.
            // WHY the strings come from the model: this demo holds no camera
            // authority, so the card states the platform's own answer. It is
            // not a live image, not sensor telemetry and not a capture.
            if camera.total_photos == 0 && camera.countdown_ticks == 0 {
                let ink = status_ink(camera.status);
                if STATUS_CARD.contains(x, y) {
                    if let Some(card_c) = std::design::primitives::draw_rounded_rect(
                        x,
                        y,
                        STATUS_CARD,
                        R_CARD,
                        chrome::SURFACE,
                        Some(chrome::tint(ink, 45)),
                        1,
                    ) {
                        // Icon-like status dot with a quiet halo.
                        let dot_dx = x as isize - STATUS_DOT_CX as isize;
                        let dot_dy = y as isize - STATUS_DOT_CY as isize;
                        let dot_r2 = dot_dx * dot_dx + dot_dy * dot_dy;
                        if dot_r2 <= (STATUS_DOT_HALO_R * STATUS_DOT_HALO_R) as isize {
                            if dot_r2 <= (STATUS_DOT_R * STATUS_DOT_R) as isize {
                                return ink;
                            }
                            return chrome::tint(ink, 26);
                        }

                        // WHY the bands are the line boxes: the card used
                        // to split itself at rows counted off its own middle
                        // (`abs_mid + 15`, `abs_mid + 20`), which were neither
                        // the runs' boxes nor a gap of their own -- the
                        // headline's band reached eighteen rows past its box,
                        // five rows belonged to neither run, and the pair sat
                        // with 27 rows of air above it and 18 below. Each band
                        // is now the box its run is drawn into, and the air
                        // between them is STATUS_STACK_GAP. The reason owns
                        // two boxes because the model splits it at a word when
                        // one column cannot hold it; an unsplit reason leaves
                        // the second box empty.
                        if y >= STATUS_HEAD_Y && y < STATUS_HEAD_Y + STATUS_HEAD_BOX {
                            if let Some(tc) = test_text(
                                x,
                                y,
                                STATUS_TEXT_X,
                                STATUS_HEAD_Y,
                                camera::STATUS_HEAD_RUNG,
                                &camera.head[..camera.head_len],
                                STATUS_TEXT_W,
                                chrome::TEXT,
                                chrome::SURFACE,
                            ) {
                                return tc;
                            }
                        } else if y >= STATUS_SUB_Y && y < STATUS_SUB_Y + STATUS_SUB_BOX {
                            if let Some(tc) = test_text(
                                x,
                                y,
                                STATUS_TEXT_X,
                                STATUS_SUB_Y,
                                camera::STATUS_REASON_RUNG,
                                &camera.sub[..camera.sub_len],
                                STATUS_TEXT_W,
                                chrome::TEXT_MUTED,
                                chrome::SURFACE,
                            ) {
                                return tc;
                            }
                        } else if y >= STATUS_SUB2_Y && y < STATUS_SUB2_Y + STATUS_SUB_BOX {
                            if let Some(tc) = test_text(
                                x,
                                y,
                                STATUS_TEXT_X,
                                STATUS_SUB2_Y,
                                camera::STATUS_REASON_RUNG,
                                &camera.sub2[..camera.sub2_len],
                                STATUS_TEXT_W,
                                chrome::TEXT_MUTED,
                                chrome::SURFACE,
                            ) {
                                return tc;
                            }
                        }
                        return card_c;
                    }
                }
            }

            return c;
        }
    }

    // 4. Mode Selector Carousel (three 216x48 slots, 12px apart)
    if (MODE_STRIP_Y0..MODE_STRIP_Y1).contains(&y) {
        if let Some(c) = mode_strip_pixel(x, y, camera) {
            return c;
        }
        return chrome::BODY;
    }

    // 5. Bottom Shutter Dock & Hardware Controls (y: DOCK_Y0..DOCK_BAND_Y1)
    if (DOCK_Y0..DOCK_BAND_Y1).contains(&y) {
        // 5.0 What the last press of the shutter answered, in the platform's
        // own words, in the strip above the ring. Painted first because the
        // caption is the answer the control owes the user.
        if let Some(c) = camera.shutter_notice_pixel(x, y, camera.shutter_notice_ink(), chrome::BODY) {
            return c;
        }

        // 5.1 Bottom-left Gallery Thumbnail Disc (radius 32, diameter 64px >= 48px)
        if side_button_hit(x, y, PHOTOS_CX) {
            let dx = x as isize - PHOTOS_CX as isize;
            let dy = y as isize - SHUTTER_CY as isize;
            let r2 = (dx * dx + dy * dy) as usize;
            let disc_r = 32usize;
            if r2 <= disc_r * disc_r {
                // Crisp outer ring
                if r2 >= (disc_r - 2) * (disc_r - 2) {
                    return chrome::BORDER_STRONG;
                }
                // Live preview thumbnail of recently captured photo
                if camera.has_thumbnail {
                    let tx = (((dx + disc_r as isize) * THUMBNAIL_DIM as isize) / (disc_r as isize * 2))
                        .clamp(0, (THUMBNAIL_DIM - 1) as isize) as usize;
                    let ty = (((dy + disc_r as isize) * THUMBNAIL_DIM as isize) / (disc_r as isize * 2))
                        .clamp(0, (THUMBNAIL_DIM - 1) as isize) as usize;
                    let idx = (ty * THUMBNAIL_DIM + tx) * 3;
                    let r = camera.thumbnail[idx] as u32;
                    let g = camera.thumbnail[idx + 1] as u32;
                    let b = camera.thumbnail[idx + 2] as u32;
                    return (0xFF << 24) | (r << 16) | (g << 8) | b;
                }
                // Fallback landscape pictogram
                if test_gallery_icon(x, y, PHOTOS_CX, SHUTTER_CY) {
                    return chrome::SKY;
                }
                return chrome::SURFACE;
            }
        }

        // 5.2 Center Shutter Button (r=48 core, outer ring 64px)
        let dx = x as isize - SHUTTER_CX as isize;
        let dy = y as isize - SHUTTER_CY as isize;
        let dist_sq = dx * dx + dy * dy;
        let refused = camera.status == CameraStatus::AuthorityRefused;

        let outer = (SHUTTER_RING_OUTER * SHUTTER_RING_OUTER) as isize;
        let inner = (SHUTTER_RING_INNER * SHUTTER_RING_INNER) as isize;
        let core = (SHUTTER_R_CORE * SHUTTER_R_CORE) as isize;

        // Outer ring: white, amber on feedback, grey when refused.
        if dist_sq <= outer && dist_sq >= inner {
            return if refused {
                chrome::DISABLED
            } else if camera.shutter_feedback_ticks > 0 {
                chrome::AMBER
            } else {
                chrome::TEXT
            };
        }
        // Concentric track gap: the chassis shows through.
        if dist_sq < inner && dist_sq >= core {
            return chrome::BODY;
        }
        // Inner tactile shutter core (r=48)
        if dist_sq < core {
            if refused {
                // A red slash: the control is present, the authority is not.
                if (dx + dy).abs() <= 2 && dx.abs() <= 34 {
                    return chrome::REFUSED;
                }
                return chrome::SURFACE;
            }
            return match camera.mode {
                CameraMode::Video => chrome::RECORD,
                // The shutter is the primary control, so it carries the amber.
                CameraMode::Photo | CameraMode::DocumentVision => chrome::AMBER,
            };
        }

        // 5.3 Bottom-right Settings Button (radius 32, diameter 64px >= 48px)
        if side_button_hit(x, y, SETTINGS_CX) {
            let dx = x as isize - SETTINGS_CX as isize;
            let dy = y as isize - SHUTTER_CY as isize;
            let r2 = (dx * dx + dy * dy) as usize;
            let disc_r = 32usize;
            if r2 <= disc_r * disc_r {
                if test_settings_icon(x, y, SETTINGS_CX, SHUTTER_CY) {
                    return chrome::SKY;
                }
                if r2 >= (disc_r - 2) * (disc_r - 2) {
                    return chrome::BORDER_STRONG;
                }
                return chrome::SURFACE;
            }
        }

        return chrome::BODY;
    }

    // The system's home indicator: the pill, and the strip below it the tap
    // handler answers, both read off `std::design::home_band`.
    if HOME_PILL.contains(x, y) {
        return chrome::TEXT_DIM;
    }

    chrome::BODY
}

/// The settings surface: a dark sheet with amber-accented rows.
///
/// `page` windows the row order into the sheet's content plates; the last
/// slot is the pager that turns it.
fn settings_pixel(x: usize, y: usize, camera: &CameraApp, page: usize) -> u32 {
    if let Some(c) = std::design::primitives::draw_rounded_rect(
        x,
        y,
        SHEET_RECT,
        radius::SHEET,
        chrome::SURFACE,
        Some(chrome::BORDER_STRONG),
        1,
    ) {
        // Amber accent bar beside the title.
        if x >= SHEET_ACCENT_X
            && x < SHEET_ACCENT_X + SHEET_ACCENT_W
            && y >= SHEET_ACCENT_TOP
            && y < SHEET_ACCENT_TOP + SHEET_ACCENT_H
        {
            return chrome::AMBER;
        }

        // Title, on the Body scale, inside its own line box.
        if y >= SHEET_TITLE_Y0 && y < SHEET_TITLE_Y0 + SHEET_TITLE_BOX {
            if let Some(tc) = test_text(
                x,
                y,
                SHEET_TEXT_X,
                SHEET_TITLE_Y0,
                typography::BODY_SCALE,
                b"CAMERA SETTINGS",
                SHEET_TEXT_W,
                chrome::TEXT,
                chrome::SURFACE,
            ) {
                return tc;
            }
        }

        // Hairline between the title and the first row.
        if y >= SHEET_HAIRLINE_Y && y < SHEET_HAIRLINE_Y + 2 {
            if x >= SHEET_ROW_X0 && x < SHEET_ROW_X0 + SHEET_ROW_W {
                return chrome::BORDER;
            }
        }

        // The rows: each one's plate is the box `settings_row_rect` names, so
        // the painter fills exactly what the tap handler answers. The slot
        // wears the row order's entry at `page * SHEET_PAGE_ROWS + slot`.
        for slot in 0..SHEET_PAGE_ROWS {
            let row_index = page * SHEET_PAGE_ROWS + slot;
            if row_index >= camera::SETTINGS_ROWS {
                continue;
            }
            let plate = settings_row_rect(slot);
            if !plate.contains(x, y) {
                continue;
            }
            let row = SETTINGS_ROW_ORDER[row_index];
            let row_y = plate.y;
            let plate_c = std::design::primitives::draw_rounded_rect(
                x,
                y,
                plate,
                radius::CHIP,
                chrome::SURFACE_HIGH,
                Some(chrome::BORDER),
                1,
            );
            let title = camera.settings_row_title(row);
            // WHY the bands are the line boxes and the block is centred: the
            // title and the value are body-scale runs (39 rows each) and the
            // note is a caption (30), so 108 rows of type stand in a plate 110
            // tall. The plate used to start them at row 2, which left no air
            // under the block at all and let the note's descenders ink the
            // plate's own bottom hairline.
            if y < row_y + SHEET_VALUE_OFF {
                let title_scale = camera::SETTINGS_TITLE_RUNG;
                if let Some(tc) = test_text(
                    x,
                    y,
                    SHEET_TEXT_X,
                    row_y + SHEET_TITLE_OFF,
                    title_scale,
                    title,
                    SHEET_TEXT_W,
                    chrome::TEXT,
                    chrome::SURFACE_HIGH,
                ) {
                    return tc;
                }
            } else if y < row_y + SHEET_NOTE_OFF {
                let len = camera.settings_value_lens[row_index];
                let value = &camera.settings_values[row_index][..len];
                let val_scale = camera::SETTINGS_VALUE_RUNG;
                if let Some(tc) = test_text(
                    x,
                    y,
                    SHEET_TEXT_X,
                    row_y + SHEET_VALUE_OFF,
                    val_scale,
                    value,
                    SHEET_TEXT_W,
                    chrome::SKY,
                    chrome::SURFACE_HIGH,
                ) {
                    return tc;
                }
            } else {
                let note = camera.settings_row_note(row);
                if !note.is_empty() {
                    if let Some(tc) = test_text(
                        x,
                        y,
                        SHEET_TEXT_X,
                        row_y + SHEET_NOTE_OFF,
                        typography::CAPTION_SCALE,
                        note,
                        SHEET_TEXT_W,
                        chrome::TEXT_MUTED,
                        chrome::SURFACE_HIGH,
                    ) {
                        return tc;
                    }
                }
            }
            if let Some(plate_c) = plate_c {
                return plate_c;
            }
        }
        // The pager: the sheet's last slot, named in `settings_row_rect` so
        // the plate the painter fills is the plate the tap handler answers.
        // Its label says which way the page turns, in the caption rung.
        let pager = settings_row_rect(SHEET_PAGE_ROWS);
        if pager.contains(x, y) {
            if let Some(plate_c) = std::design::primitives::draw_rounded_rect(
                x,
                y,
                pager,
                radius::CHIP,
                chrome::SURFACE_HIGH,
                Some(chrome::BORDER),
                1,
            ) {
                let label: &[u8] =
                    if page + 1 < SHEET_PAGES { b"MORE SETTINGS" } else { b"FIRST SETTINGS" };
                let lx = camera::centred_x(label, typography::CAPTION_SCALE, pager.x, pager.w);
                let ly = pager.y + centred_text_top(pager.h, typography::CAPTION_SCALE);
                if let Some(tc) = test_text(
                    x,
                    y,
                    lx,
                    ly,
                    typography::CAPTION_SCALE,
                    label,
                    pager.w,
                    chrome::SKY,
                    chrome::SURFACE_HIGH,
                ) {
                    return tc;
                }
                return plate_c;
            }
        }
        return c;
    }
    chrome::BODY
}

fn render_camera_ui(surface: u32, camera: &CameraApp, page: usize) {
    render_camera_band(surface, camera, 0, H, page);
}

fn render_camera_band(surface: u32, camera: &CameraApp, y0: usize, y1: usize, page: usize) {
    let mut chunk = [0u32; CHUNK];
    let mut offset = y0 * W;
    let end = y1.min(H) * W;
    while offset < end {
        let count = (end - offset).min(CHUNK);
        let mut x = offset % W;
        let mut y = offset / W;
        for i in 0..count {
            chunk[i] = get_camera_pixel(x, y, camera, page);
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

#[derive(Clone, Copy, PartialEq, Eq)]
enum Dirty {
    None,
    TopRow,
    Viewfinder,
    Dock,
    All,
}

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
    None,
    Redraw(Dirty),
    Leave(&'static str),
}

fn exit_to_launcher(surface: u32) -> ! {
    if surface > 0 {
        let _ = std::surface_destroy(surface);
    }
    std::exit_with_code(0);
}

fn open_photo_library(camera: &CameraApp) {
    match camera.open_photo_library() {
        Ok(task_id) => {
            let mut buf = [0u8; 96];
            let prefix = b"[CAMERA APP] Gallery service active (PID ";
            let p = prefix.len().min(buf.len());
            buf[..p].copy_from_slice(&prefix[..p]);
            let mut n = p + camera::append_decimal(task_id as u32, &mut buf[p..]);
            if n < buf.len() {
                buf[n] = b')';
                n += 1;
            }
            if let Ok(msg) = core::str::from_utf8(&buf[..n]) {
                std::log(msg).ok();
            }
        }
        Err(camera::ERR_NO_PHOTO) => {
            std::log("[CAMERA APP] No photo captured yet; launching Gallery app directly.").ok();
            if let Ok(task_id) = std::service_package_launch("gallery") {
                let mut buf = [0u8; 96];
                let prefix = b"[CAMERA APP] Gallery service launched via package (PID ";
                let p = prefix.len().min(buf.len());
                buf[..p].copy_from_slice(&prefix[..p]);
                let mut n = p + camera::append_decimal(task_id as u32, &mut buf[p..]);
                if n < buf.len() {
                    buf[n] = b')';
                    n += 1;
                }
                if let Ok(msg) = core::str::from_utf8(&buf[..n]) {
                    std::log(msg).ok();
                }
            } else {
                std::log("[CAMERA APP] Gallery service package launch unavailable.").ok();
            }
        }
        Err(code) => {
            log_capture_code(b"[CAMERA APP] Intent dispatch refused, code -", code);
            std::log("[CAMERA APP] Falling back to service package launch for Gallery...").ok();
            if let Ok(task_id) = std::service_package_launch("gallery") {
                let mut buf = [0u8; 96];
                let prefix = b"[CAMERA APP] Gallery service launched via package (PID ";
                let p = prefix.len().min(buf.len());
                buf[..p].copy_from_slice(&prefix[..p]);
                let mut n = p + camera::append_decimal(task_id as u32, &mut buf[p..]);
                if n < buf.len() {
                    buf[n] = b')';
                    n += 1;
                }
                if let Ok(msg) = core::str::from_utf8(&buf[..n]) {
                    std::log(msg).ok();
                }
            }
        }
    }
}

fn log_capture_code(prefix: &[u8], code: i64) {
    let mut buf = [0u8; 96];
    let p = prefix.len().min(buf.len());
    buf[..p].copy_from_slice(&prefix[..p]);
    let n = p + camera::append_decimal(code.unsigned_abs() as u32, &mut buf[p..]);
    if let Ok(msg) = core::str::from_utf8(&buf[..n]) {
        std::log(msg).ok();
    }
}

/// The content slot whose plate is under `(x, y)`, on the box
/// [`settings_pixel`] fills. The pager slot is not a content row: it is
/// answered where the page is turned.
fn settings_row_at(x: usize, y: usize) -> Option<usize> {
    let mut index = 0usize;
    while index < SHEET_PAGE_ROWS {
        if settings_row_rect(index).contains(x, y) {
            return Some(index);
        }
        index += 1;
    }
    None
}

fn handle_settings_tap(camera: &mut CameraApp, x: usize, y: usize, page: &mut usize) -> Tap {
    if settings_row_rect(SHEET_PAGE_ROWS).contains(x, y) {
        *page = (*page + 1) % SHEET_PAGES;
        std::log("[CAMERA APP] Settings page turned.").ok();
        return Tap::Redraw(Dirty::All);
    }
    let slot = match settings_row_at(x, y) {
        Some(slot) => slot,
        None => return Tap::None,
    };
    let index = *page * SHEET_PAGE_ROWS + slot;
    if index >= camera::SETTINGS_ROWS {
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
                        "[CAMERA APP] Flash mode toggled: ON."
                    } else {
                        "[CAMERA APP] Flash mode toggled: OFF."
                    })
                    .ok();
                }
                None => {
                    std::log("[CAMERA APP] Flash unavailable: hardware backend refused request; flash is off.")
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
                "[CAMERA APP] 3x3 Rule-of-Thirds framing grid enabled."
            } else {
                "[CAMERA APP] Framing grid disabled."
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
            Tap::None
        }
    }
}

fn handle_camera_touch(camera: &mut CameraApp, x: u32, y: u32, page: &mut usize) -> Tap {
    let x = x as usize;
    let y = y as usize;

    // 1. Back control or the home gesture strip: exit to the Launcher
    //
    // WHY the chip is asked on its own: `BACK_RECT.contains` used to sit here as
    // a second clause. Now that `BACK_RECT` is the SDK's rect the two answer the
    // identical box, so spelling it twice only invited the second copy to drift
    // from the painted chip. The layout chain below still gates `BACK_RECT`
    // against the flash pill, which is what the name is for.
    if std::design::back_button::contains(x, y) || HOME_HIT.contains(x, y) {
        if camera.screen == CameraScreen::Settings {
            camera.screen = CameraScreen::Viewfinder;
            return Tap::Redraw(Dirty::Viewfinder);
        }
        return Tap::Leave("[CAMERA] Back control or home gesture pressed. Exiting to launcher.");
    }

    // 2. The settings surface, when it is open
    if camera.screen == CameraScreen::Settings {
        return handle_settings_tap(camera, x, y, page);
    }

    // 3. Flash mode toggle pill (Auto -> On -> Off -> Auto)
    if FLASH_RECT.contains(x, y) {
        match camera.toggle_flash() {
            Some(on) => {
                std::log(if on {
                    "[CAMERA APP] Flash mode toggled: ON."
                } else {
                    "[CAMERA APP] Flash mode toggled: OFF."
                })
                .ok();
            }
            None => {
                std::log("[CAMERA APP] Flash unavailable: hardware backend refused request; flash is off.")
                    .ok();
            }
        }
        let _ = std::audio_haptic_click();
        return Tap::Redraw(Dirty::TopRow);
    }

    // 4. ISP filter toggle pill (Normal -> Vivid -> Warm -> Cool -> Mono -> Vintage)
    if FILTER_RECT.contains(x, y) {
        camera.filter = camera.filter.next();
        std::audio_haptic_click().ok();
        return Tap::Redraw(Dirty::TopRow);
    }

    // 5. Viewfinder header overlays:
    // 5.1 Aspect ratio toggle pill (4:3 -> 16:9 -> 1:1 -> 4:3)
    if camera.screen == CameraScreen::Viewfinder && ASPECT_RECT.contains(x, y) {
        camera.aspect_ratio = camera.aspect_ratio.toggle();
        std::audio_haptic_click().ok();
        std::log("[CAMERA APP] Aspect ratio switched.").ok();
        return Tap::Redraw(Dirty::All);
    }

    // 5.2 Rule-of-Thirds Grid toggle pill
    if camera.screen == CameraScreen::Viewfinder && GRID_RECT.contains(x, y) {
        camera.guides = !camera.guides;
        camera.refresh_settings_values();
        std::audio_haptic_click().ok();
        std::log(if camera.guides {
            "[CAMERA APP] 3x3 Rule-of-Thirds framing grid enabled."
        } else {
            "[CAMERA APP] Framing grid disabled."
        })
        .ok();
        return Tap::Redraw(Dirty::Viewfinder);
    }

    // 5.3 Self-Timer toggle pill (Off -> 3s -> 10s -> Off)
    if camera.screen == CameraScreen::Viewfinder && TIMER_RECT.contains(x, y) {
        camera.timer_mode = camera.timer_mode.next();
        std::audio_haptic_click().ok();
        std::log("[CAMERA APP] Self-timer mode switched.").ok();
        return Tap::Redraw(Dirty::Viewfinder);
    }

    // 6. Mode Selector Carousel: the slot under the finger, on the box the
    // strip paints.
    if let Some(index) = mode_slot_at(x, y) {
        camera.mode = MODE_ORDER[index];
        std::log(MODE_FEEDBACK[index]).ok();
        std::audio_haptic_click().ok();
        return Tap::Redraw(Dirty::All);
    }

    // 7. Bottom-left Gallery Thumbnail Disc (radius 32, diameter 64px >= 48px)
    if side_button_hit(x, y, PHOTOS_CX) {
        std::audio_haptic_click().ok();
        open_photo_library(camera);
        return Tap::Redraw(Dirty::Viewfinder);
    }

    // 8. Bottom-right Settings Button (radius 32, diameter 64px >= 48px)
    if side_button_hit(x, y, SETTINGS_CX) {
        camera.screen = CameraScreen::Settings;
        // WHY the sheet opens on its first page: a stale page from a previous
        // visit has no meaning once the surface has been closed.
        *page = 0;
        camera.refresh_settings_values();
        std::audio_haptic_click().ok();
        return Tap::Redraw(Dirty::Viewfinder);
    }

    // 9. Center Shutter Button (r=48 core, outer ring 64px)
    if shutter_hit(x as isize - SHUTTER_CX as isize, y as isize - SHUTTER_CY as isize) {
        if camera.status == CameraStatus::AuthorityRefused {
            std::log("[CAMERA APP] Shutter unavailable: kernel refused camera authority for this task.").ok();
            // WHY the dock repaints for a press that did nothing: the press
            // has an answer and the user has to be able to read it. Before
            // this the refusal reached the serial log only, which left the
            // control looking broken rather than refused.
            camera.refuse_shutter();
            return Tap::Redraw(Dirty::Dock);
        }

        // If self-timer is enabled, begin countdown
        if camera.timer_mode != TimerMode::Off && camera.countdown_ticks == 0 {
            camera.countdown_ticks = camera.timer_mode.seconds() * 10;
            camera.shutter_feedback_ticks = 4;
            // WHY the caption goes when a countdown starts: the strip under
            // the shutter answers the press that just happened, and a press
            // that is counting down has not been refused.
            camera.clear_shutter_notice();
            std::log("[CAMERA APP] Self-timer countdown initiated.").ok();
            std::audio_haptic_click().ok();
            return Tap::Redraw(Dirty::Viewfinder);
        }

        camera.shutter_feedback_ticks = 4;
        std::log("[CAMERA APP] Shutter haptic feedback pulse dispatched.").ok();
        let outcome = if camera.mode == CameraMode::Video {
            camera.stream_video_frame()
        } else {
            camera.capture_still_photo()
        };
        match outcome {
            CaptureOutcome::Published => {
                std::log("[CAMERA APP] The photo is on the device and the document index describes it.").ok();
            }
            CaptureOutcome::WrittenNotIndexed => {
                std::log("[CAMERA APP] The photo is on the device, but the document index did not describe it.").ok();
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
                std::log("[CAMERA APP] The camera HAL exposes no video stream; this platform cannot deliver a video frame.").ok();
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
    // The settings sheet's page, held here rather than in the model: it is
    // this renderer's window over the row order, and the row order is the
    // model's. Zero heap, one usize, reset when the surface is reopened.
    let mut sheet_page = 0usize;
    refresh_clock(&mut camera);
    camera.refresh_shots_label();
    camera.refresh_status_text();

    camera.probe_camera_authority();
    match camera.status {
        CameraStatus::AuthorityRefused => {
            std::log("[CAMERA APP] The platform refused camera authority for this task; the viewfinder says so.").ok();
        }
        CameraStatus::NoSensor => {
            std::log("[CAMERA APP] Camera authority held, but the camera HAL reported that no sensor is bound.").ok();
        }
        CameraStatus::AuthorityHeld => {
            std::log("[CAMERA APP] The platform granted camera authority; capture controls are armed.").ok();
        }
        CameraStatus::Failed => {
            std::log("[CAMERA APP] The camera authority query failed; the viewfinder says so.").ok();
        }
    }

    std::log("[CAMERA APP] Step 1: Allocating application surface (z=200)...").ok();
    let surface = match std::surface_create(W as u32, H as u32, 200) {
        Ok(s) => {
            std::log("[CAMERA APP] Step 2: Surface created successfully.").ok();
            s
        }
        Err(_) => {
            std::log("[CAMERA APP] Surface allocation unavailable; falling back to headless execution.").ok();
            std::exit_with_code(0);
        }
    };

    camera.mode = CameraMode::Photo;
    render_camera_ui(surface, &camera, sheet_page);
    let _ = std::surface_flip();
    std::log("[CAMERA APP] Step 3: Initial frame presented to hardware scanout.").ok();

    loop {
        if let Ok(events) = std::event_poll(std::EVENT_TOUCH_INPUT | std::EVENT_TIMER_TICK, 100) {
            let _ = &events;

            if (events & std::EVENT_TIMER_TICK) != 0 {
                let mut needs_redraw = false;

                if camera.shutter_feedback_ticks > 0 {
                    camera.shutter_feedback_ticks = camera.shutter_feedback_ticks.saturating_sub(1);
                    if surface > 0 {
                        render_camera_band(surface, &camera, DOCK_Y0, DOCK_Y1, sheet_page);
                        needs_redraw = true;
                    }
                }

                if camera.countdown_ticks > 0 {
                    camera.countdown_ticks -= 1;
                    if camera.countdown_ticks == 0 {
                        camera.shutter_feedback_ticks = 4;
                        let _ = camera.capture_still_photo();
                        camera.refresh_settings_values();
                        if surface > 0 {
                            render_camera_ui(surface, &camera, sheet_page);
                            let _ = std::surface_flip();
                        }
                    } else if surface > 0 {
                        render_camera_band(surface, &camera, VIEWFINDER_Y0, VIEWFINDER_Y1, sheet_page);
                        needs_redraw = true;
                    }
                }

                if needs_redraw && surface > 0 {
                    let _ = std::surface_flip();
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
                    if input.kind == 4 || input.kind == 6 || input.kind == 7 || input.kind == 8 || input.kind == 10 {
                        std::log("[CAMERA APP] Navigation gesture recognized. Returning to launcher.").ok();
                        exit_to_launcher(surface);
                    }
                    if input.kind == 1 {
                        match handle_camera_touch(&mut camera, input.x, input.y, &mut sheet_page) {
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
                            render_camera_band(surface, &camera, TOP_BAND_Y0, TOP_BAND_Y1, sheet_page)
                        }
                        Dirty::Viewfinder => render_camera_band(
                            surface,
                            &camera,
                            VIEWFINDER_Y0,
                            VIEWFINDER_Y1,
                            sheet_page,
                        ),
                        Dirty::Dock => render_camera_band(surface, &camera, DOCK_Y0, DOCK_Y1, sheet_page),
                        Dirty::All => render_camera_ui(surface, &camera, sheet_page),
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
